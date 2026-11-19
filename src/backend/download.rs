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

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::result::Result as StdResult;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::backend::{BackendError, Result};
use sha2::{Digest, Sha256};

use crate::util::hex_lower;

/// deb 包外层是 ar 归档，其魔数固定为 `!<arch>\n`。
const DEB_MAGIC: &[u8; 8] = b"!<arch>\n";
/// 流式读写块大小：128 KiB 在吞吐与内存占用之间取折中。
const CHUNK: usize = 128 * 1024;
/// 建立连接的独立超时，避免 DNS 或代理握手长期挂起。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 单次下载的总时限。大包允许较长时间，但必须有限，防止协程或线程永久占用。
const REQUEST_TIMEOUT: Duration = Duration::from_mins(30);

/// 全局复用的阻塞式 HTTP 客户端，保留连接池、Keep-Alive 与 DNS 缓存。
static HTTP_CLIENT: OnceLock<StdResult<reqwest::blocking::Client, String>> = OnceLock::new();

/// 一次下载请求的完整参数。
///
/// 收敛为结构体后，调用点不必平铺 URL、目标路径、取消标志、校验和与进度回调；
/// 也避免后续增加限速或镜像策略时继续拉长参数列表。
pub struct DownloadOptions<'a, F> {
    /// 下载地址，仅支持 HTTP 与 HTTPS。
    pub url: &'a str,
    /// 最终保存路径。
    pub destination: &'a Path,
    /// worker 读取的取消标志。
    pub cancel: &'a AtomicBool,
    /// 软件源给出的 SHA256；缺失或空串时跳过校验。
    pub expected_sha256: Option<&'a str>,
    /// 进度回调，参数为已收字节与总字节；总字节未知时为 0。
    pub progress: F,
}

/// 下载安装包并完成大小、摘要与 deb 魔数校验。
///
/// # Errors
///
/// URL 协议不受支持、网络请求失败、取消、写入失败、大小或摘要不匹配、
/// 或下载结果不是 deb 归档时返回错误。失败路径会删除未完成的 `.part` 文件。
pub fn download_file<F>(options: DownloadOptions<'_, F>) -> Result<PathBuf>
where
    F: FnMut(u64, u64),
{
    let DownloadOptions {
        url,
        destination,
        cancel,
        expected_sha256,
        mut progress,
    } = options;
    validate_url(url)?;
    let mut response = fetch_response(url)?;
    let total = response.content_length().unwrap_or(0);
    let (part_path, mut file) = create_part_file(destination)?;
    let mut stream = DownloadStream {
        response: &mut response,
        file: &mut file,
        cancel,
        total,
        progress: &mut progress,
    };
    let (received, hasher) = match stream_response(&mut stream) {
        Ok(result) => result,
        Err(error) => {
            drop(file);
            let _ = fs::remove_file(&part_path);
            return Err(error);
        }
    };
    // sync 失败不影响随后 rename 的可见原子性，故忽略。
    file.sync_all().ok();
    drop(file);
    verify_size(received, total, &part_path)?;
    verify_hash(hasher, expected_sha256, &part_path)?;
    fs::rename(&part_path, destination)
        .map_err(|source| BackendError::io("保存下载文件", destination.display(), source))?;
    if let Err(error) = validate_deb(destination) {
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    Ok(destination.to_path_buf())
}

fn validate_url(url: &str) -> Result<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(BackendError::invalid(
            "下载安装包",
            format!("不支持的下载地址：{url}"),
        ));
    }
    Ok(())
}

fn fetch_response(url: &str) -> Result<reqwest::blocking::Response> {
    let client = http_client()?;
    client
        .get(url)
        .send()
        .map_err(|error| BackendError::network("连接软件源", url, error.to_string()))?
        .error_for_status()
        .map_err(|error| BackendError::network("检查软件源响应", url, error.to_string()))
}

fn create_part_file(destination: &Path) -> Result<(PathBuf, File)> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| BackendError::io("创建下载目录", parent.display(), source))?;
    }
    let mut part_name = OsString::from(destination.as_os_str());
    part_name.push(".part");
    let part_path = PathBuf::from(part_name);
    let file = File::create(&part_path)
        .map_err(|source| BackendError::io("创建下载临时文件", part_path.display(), source))?;
    Ok((part_path, file))
}

struct DownloadStream<'a, F> {
    response: &'a mut reqwest::blocking::Response,
    file: &'a mut File,
    cancel: &'a AtomicBool,
    total: u64,
    progress: &'a mut F,
}

fn stream_response<F>(stream: &mut DownloadStream<'_, F>) -> Result<(u64, Sha256)>
where
    F: FnMut(u64, u64),
{
    let mut buffer = vec![0_u8; CHUNK];
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    loop {
        if stream.cancel.load(Ordering::Relaxed) {
            return Err(BackendError::Cancelled {
                operation: "下载安装包",
            });
        }
        let read = stream
            .response
            .read(&mut buffer)
            .map_err(|source| BackendError::io("读取下载数据", "下载响应流", source))?;
        if read == 0 {
            break;
        }
        stream
            .file
            .write_all(&buffer[..read])
            .map_err(|source| BackendError::io("写入下载文件", "下载临时文件", source))?;
        hasher.update(&buffer[..read]);
        received = received.saturating_add(read as u64);
        (stream.progress)(received, stream.total);
    }
    Ok((received, hasher))
}

fn verify_size(received: u64, total: u64, part_path: &Path) -> Result<()> {
    if total > 0 && received != total {
        let _ = fs::remove_file(part_path);
        return Err(BackendError::Integrity {
            operation: "校验下载大小",
            detail: format!("下载不完整：已收到 {received} 字节，预期 {total} 字节"),
        });
    }
    Ok(())
}

fn verify_hash(hasher: Sha256, expected: Option<&str>, part_path: &Path) -> Result<()> {
    let Some(expected) = expected.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let actual = hex_lower(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        let _ = fs::remove_file(part_path);
        return Err(BackendError::Integrity {
            operation: "校验下载摘要",
            detail: format!("SHA256 校验失败：预期 {expected}，实际 {actual}"),
        });
    }
    Ok(())
}

/// 返回进程级复用的 HTTP 客户端。初始化失败时返回包含中文原因的错误。
fn http_client() -> Result<&'static reqwest::blocking::Client> {
    let cached = HTTP_CLIENT.get_or_init(build_http_client);
    match cached {
        Ok(client) => Ok(client),
        Err(message) => Err(BackendError::network("初始化下载客户端", "", message)),
    }
}

fn build_http_client() -> StdResult<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("无法初始化全局下载客户端：{error}"))
}

/// 校验文件确实是 deb 归档，而不是被网络门户拦截下来的 HTML。
fn validate_deb(path: &Path) -> Result<()> {
    let mut file = File::open(path)
        .map_err(|source| BackendError::io("打开下载文件", path.display(), source))?;
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)
        .map_err(|source| BackendError::Integrity {
            operation: "校验下载文件",
            detail: format!("无法读取 deb 魔数：{source}"),
        })?;
    if &magic != DEB_MAGIC {
        return Err(BackendError::Integrity {
            operation: "校验下载文件",
            detail: "下载的文件不是有效的 deb 安装包（可能被网络或认证页面拦截）".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_url_scheme() {
        let dir = std::env::temp_dir();
        let destination = dir.join("x.deb");
        let cancel = AtomicBool::new(false);
        let error = download_file(DownloadOptions {
            url: "ftp://example.com/x.deb",
            destination: &destination,
            cancel: &cancel,
            expected_sha256: None,
            progress: |_, _| {},
        })
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
        let path = download_file(DownloadOptions {
            url: &url,
            destination: &destination,
            cancel: &cancel,
            expected_sha256: (!package.sha256.is_empty()).then_some(package.sha256.as_str()),
            progress: |_, _| {},
        })
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
