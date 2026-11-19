//! 主机环境探测：可执行文件查找与 dpkg 架构名。

use std::env;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::process::run_command;

/// 判断命令是否存在。含 `/` 的参数按路径直接检查（`.desktop` 里常见绝对路径），
/// 否则遍历 `PATH`。用文件系统检查代替 `which`/`command -v` 子进程调用。
pub fn command_exists(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// 当前系统的 Debian 架构名（`amd64`、`arm64`…）。
///
/// 首选 `dpkg --print-architecture`，因为它同时正确处理 multi-arch 与
/// 非主流架构；dpkg 不可用（例如未安装的容器环境）时按 Rust 目标架构回落。
pub fn current_arch() -> String {
    let mut command = Command::new("dpkg");
    command.arg("--print-architecture");
    if let Ok(output) = run_command(&mut command, Duration::from_secs(5))
        && output.status.success()
    {
        let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !arch.is_empty() {
            return arch;
        }
    }
    match env::consts::ARCH {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        "x86" => "i386".to_string(),
        other => other.to_string(),
    }
}
