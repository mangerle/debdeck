//! 在线获取安装包：HTTP(S) 下载 + 完整性校验。
//!
//! 落盘流程保证原子性与可验证性：
//!
//! 1. 写入 `<目标>.part`，成功校验后才 `rename` 成正式文件名，
//!    中途失败或取消不会留下看起来完整的文件；
//! 2. 有 `Content-Length` 时校验总字节数；
//! 3. 软件源提供 SHA256 时校验摘要；
//! 4. `rename` 后再检查 ar 魔数 `!<arch>`，识别「下载到的是登录门户 HTML」
//!    这类典型陷阱，并删除无效文件。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::util::hex_lower;

/// deb 包外层是 ar 归档，其魔数固定为 `!<arch>\n`。
const DEB_MAGIC: &[u8; 8] = b"!<arch>\n";
/// 流式读写块大小：128 KiB 在吞吐与内存占用之间取折中。
const CHUNK: usize = 128 * 1024;

/// 下载 `url` 到 `destination`，返回最终路径。
///
/// `cancel` 每次读循环检查一次；`expected_sha256` 为空串时视为不提供；
/// `progress` 会以（已收字节，总字节）回调，总字节未知时为 0。
pub fn download_file(
    url: &str,
    destination: &Path,
    cancel: &AtomicBool,
    expected_sha256: Option<&str>,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        bail!("不支持的下载地址：{url}");
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(30))
        .build()
        .context("无法初始化下载客户端")?;

    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("无法连接到 {url}"))?
        .error_for_status()
        .with_context(|| format!("软件源返回了错误状态：{url}"))?;
    let total = response.content_length().unwrap_or(0);

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建下载目录 {}", parent.display()))?;
    }
    let part_path = PathBuf::from(format!("{}.part", destination.display()));
    let mut file = File::create(&part_path)
        .with_context(|| format!("无法创建文件 {}", part_path.display()))?;

    let mut buffer = vec![0_u8; CHUNK];
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = fs::remove_file(&part_path);
            bail!("下载已取消");
        }
        let read = response.read(&mut buffer).context("下载数据读取失败")?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .context("写入下载文件失败")?;
        hasher.update(&buffer[..read]);
        received += read as u64;
        progress(received, total);
    }
    // sync 失败不影响正确性（随后 rename），故忽略。
    file.sync_all().ok();
    drop(file);

    if total > 0 && received != total {
        let _ = fs::remove_file(&part_path);
        bail!("下载不完整：已收到 {received} 字节，预期 {total} 字节");
    }

    if let Some(expected) = expected_sha256.filter(|value| !value.is_empty()) {
        let actual = hex_lower(&hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = fs::remove_file(&part_path);
            bail!("SHA256 校验失败：预期 {expected}，实际 {actual}");
        }
    }

    fs::rename(&part_path, destination)
        .with_context(|| format!("无法保存到 {}", destination.display()))?;
    if let Err(error) = validate_deb(destination) {
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    Ok(destination.to_path_buf())
}

/// 校验文件确实是 deb 归档，而不是被网络门户拦截下来的 HTML。
fn validate_deb(path: &Path) -> Result<()> {
    let mut file = File::open(path).context("无法读取下载的安装包")?;
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic).context("下载的文件为空")?;
    if &magic != DEB_MAGIC {
        bail!("下载的文件不是有效的 deb 安装包（可能被网络或认证页面拦截）");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_url_scheme() {
        let dir = std::env::temp_dir();
        let cancel = AtomicBool::new(false);
        let error = download_file(
            "ftp://example.com/x.deb",
            &dir.join("x.deb"),
            &cancel,
            None,
            |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("不支持的下载地址"), "{error}");
    }

    /// 真机冒烟：从当前软件源真实下载一个包。
    ///
    /// 需要网络与已 `apt update` 过的索引，因此默认忽略；`cargo test --lib -- --ignored` 会跑它。
    #[test]
    #[ignore = "需要联网与本机 APT 索引"]
    fn downloads_real_package_over_http() {
        use crate::backend::{apt, fs_layout::FsLayout};
        use crate::util::current_arch;

        let layout = FsLayout::system();
        let repository = apt::load_repository(&layout, &current_arch());
        let package = ["hello", "tree", "more"]
            .iter()
            .find_map(|name| {
                repository
                    .versions(name)
                    .iter()
                    .find(|candidate| candidate.download_url().is_some())
            })
            .expect("本机软件源里应有 hello/tree/more 之一可下载");

        let url = package.download_url().unwrap();
        let destination = std::env::temp_dir().join(format!(
            "debdeck-smoke-{}-{}.deb",
            std::process::id(),
            package.metadata.version
        ));
        let cancel = AtomicBool::new(false);
        let path = download_file(
            &url,
            &destination,
            &cancel,
            (!package.sha256.is_empty()).then_some(package.sha256.as_str()),
            |_, _| {},
        )
        .expect("下载应成功");
        assert_eq!(&fs::read(&path).unwrap()[..8], DEB_MAGIC);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn detects_non_deb_payload() {
        let dir = std::env::temp_dir().join(format!("debdeck-dl-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("portal.html");
        fs::write(&path, b"<html>login</html>").unwrap();
        assert!(validate_deb(&path).is_err());
        let good = dir.join("real.deb");
        fs::write(&good, b"!<arch>\nsomething").unwrap();
        assert!(validate_deb(&good).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
