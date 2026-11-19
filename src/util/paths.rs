//! 应用自身的目录布局与文件名构造。

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::process::run_command;

/// 用户可见的输出目录。全部落在下载目录下，便于用户在文件管理器里直接找到产物。
#[derive(Clone, Debug)]
pub struct AppPaths {
    /// 在线下载与 `.desktop` 产物的根目录。
    pub downloads: PathBuf,
    /// 离线重打包（repack）产物目录，位于 [`Self::downloads`] 之下。
    pub builds: PathBuf,
}

impl AppPaths {
    pub fn new() -> Self {
        let downloads = default_download_dir().join("DebDeck");
        let builds = downloads.join("repack");
        Self { downloads, builds }
    }
}

impl Default for AppPaths {
    fn default() -> Self {
        Self::new()
    }
}

/// 优先询问 `xdg-user-dir DOWNLOAD`；`xdg-user-dirs` 未安装或返回 `$HOME`
/// （即用户没有配置下载目录）时回落到 `~/Downloads`。
fn default_download_dir() -> PathBuf {
    let mut command = Command::new("xdg-user-dir");
    command.arg("DOWNLOAD");
    if let Ok(output) = run_command(&mut command, Duration::from_secs(5))
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() && path != env::var("HOME").unwrap_or_default() {
            return PathBuf::from(path);
        }
    }
    home_dir().join("Downloads")
}

/// 主目录。缺失 `HOME` 时返回根目录而不是崩溃，让上层的 IO 错误自然暴露。
pub fn home_dir() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// 把任意字符串压成单个安全的路径组件。
///
/// 包名可含 `:`（multi-arch，如 `libc6:amd64`），版本可含 `+`、`~`，这些在
/// ext4 上合法；真正危险的是分隔符与控制字符，会被替换为 `_`。
pub(crate) fn sanitize_file_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '\0' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect()
}

/// 下载产物的文件名：`包名_版本_架构.deb`。
///
/// 与 repack 产物（额外带 `_repack` 后缀）共用同一套清洗规则，
/// 保证两类文件不会因字符处理不同而互相覆盖。
pub(crate) fn package_file_name(pkg: &str, version: &str, arch: &str) -> String {
    format!(
        "{}_{}_{}.deb",
        sanitize_file_component(pkg),
        sanitize_file_component(version),
        sanitize_file_component(arch)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_package_file_names() {
        assert_eq!(
            package_file_name("a:b", "1/2", "amd64"),
            "a:b_1_2_amd64.deb"
        );
        assert_eq!(package_file_name("a\nb", "1", "amd64"), "a_b_1_amd64.deb");
    }
}
