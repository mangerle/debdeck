//! 需要管理员权限的包操作，统一经 `pkexec` 提权。
//!
//! # 安全约定
//!
//! 包名一律通过 [`valid_package_name`] 白名单校验后作为**独立 argv** 传给
//! pkexec，绝不拼接进 shell 命令行；这样即使调用方传入 `bad;rm -rf /` 也只会被
//! 当成一个非法包名拒绝。`pkexec` 不是 shell，但它会 exec 目标程序并原样传参。
//!
//! # 工具选择
//!
//! 卸载用 `apt-get remove`（会处理依赖关系），保持版本用 `apt-mark`（APT 官方
//! 入口，语义与 `apt list --upgradable` 一致）。两者都属于 apt 生态，避免与
//! `dpkg --set-selections` 混用造成「谁权威」的歧义。

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::backend::{BackendError, Result};

use crate::util::{command_exists, run_command};

use super::deb_file;

/// dpkg 允许的包名字符集。空串与以 `-` 开头的值被拒绝，因为它们会被
/// 当成命令行选项（例如 `--force-remove-essential`）。
fn valid_package_name(pkg: &str) -> bool {
    !pkg.is_empty()
        && !pkg.starts_with('-')
        && pkg.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.' | b':' | b'_' | b'~')
        })
}

/// 校验包名，非法时返回可直接展示给用户的错误。
fn ensure_package_name(pkg: &str) -> Result<()> {
    if valid_package_name(pkg) {
        Ok(())
    } else {
        Err(BackendError::invalid(
            "校验软件包名称",
            format!("无效的软件包名称：{pkg}"),
        ))
    }
}

/// 以管理员身份执行命令，返回合并后的输出文本。
///
/// `DEBIAN_FRONTEND=noninteractive` 防止 apt 试图弹出自带交互界面（在 pkexec
/// 下没有 tty，会直接卡死）。退出码 126/127 通常意味着用户在 polkit 弹窗里
/// 取消或系统缺少认证代理，额外给出提示。
fn run_privileged(args: &[&str]) -> Result<String> {
    if !command_exists("pkexec") {
        return Err(BackendError::missing_program("请求管理员权限", "pkexec"));
    }
    let mut command = Command::new("pkexec");
    command.args(args).env("DEBIAN_FRONTEND", "noninteractive");
    let output = run_command(&mut command, Duration::from_mins(30))
        .map_err(|source| BackendError::command("请求管理员权限", source))?;
    let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    if output.status.success() {
        return Ok(text);
    }
    let code = output.status.code().unwrap_or(-1);
    let hint = if code == 126 || code == 127 {
        "\n提示：管理员认证可能被取消，或系统没有可用的 polkit 认证代理。"
    } else {
        ""
    };
    let detail = if text.is_empty() {
        format!("命令执行失败（退出码 {code}）。{hint}")
    } else {
        format!("命令执行失败（退出码 {code}）：\n{text}{hint}")
    };
    Err(BackendError::CommandFailed {
        operation: "执行管理员命令",
        code,
        detail,
    })
}

/// 卸载软件包（保留依赖处理给 apt）。
pub fn uninstall_package(pkg: &str) -> Result<()> {
    ensure_package_name(pkg)?;
    run_privileged(&["apt-get", "remove", "-y", pkg])?;
    Ok(())
}

/// 保持或取消保持软件包版本。
pub fn set_hold_state(pkg: &str, hold: bool) -> Result<()> {
    ensure_package_name(pkg)?;
    let action = if hold { "hold" } else { "unhold" };
    run_privileged(&["apt-mark", action, pkg])?;
    Ok(())
}

/// 本地安装的 apt-get 参数（不含 `pkexec`）。
///
/// 内嵌认证代理需要自己启动 `pkexec`，因此把参数构造单独暴露出来，避免安装
/// 参数在两条调用路径上各写一份。首个参数必须是绝对路径：pkexec 不接受
/// 仅靠 `PATH` 解析的程序名。
///
/// `--reinstall` 不能省：本地 deb 的版本与已安装版本相同时，`apt-get install`
/// 会直接判定「已是最新版」并成功退出，用户点了一次安装却什么都没发生。
pub fn local_install_args(path: &Path) -> Result<Vec<String>> {
    let path = deb_file::canonicalize(path).map_err(|source| BackendError::Package { source })?;
    let path = path.to_str().ok_or_else(|| {
        BackendError::invalid(
            "构造本地安装参数",
            "安装包路径包含非 UTF-8 字符，无法传给 apt-get",
        )
    })?;
    Ok(vec![
        "/usr/bin/apt-get".to_string(),
        "install".to_string(),
        "--reinstall".to_string(),
        "-y".to_string(),
        "-o".to_string(),
        "Dpkg::Options::=--force-confdef".to_string(),
        "-o".to_string(),
        "Dpkg::Options::=--force-confold".to_string(),
        path.to_string(),
    ])
}

/// 以管理员身份执行一组命令参数（交给系统 polkit 认证代理）。
pub(crate) fn run_privileged_argv(args: &[String]) -> Result<String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_privileged(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_package_names() {
        assert!(valid_package_name("firefox"));
        assert!(valid_package_name("libc6-dev"));
        assert!(valid_package_name("g++"));
        assert!(valid_package_name("libc6:amd64"));
        assert!(!valid_package_name(""));
        assert!(!valid_package_name("--force"));
        assert!(!valid_package_name("bad package"));
        assert!(!valid_package_name("bad;rm -rf /"));
    }

    #[test]
    fn rejects_injection_before_invoking_pkexec() {
        // 恶意包名必须在任何子进程启动前被拒绝（沙箱里 pkexec 未必存在，
        // 因此这里只断言错误信息本身）。
        let error = set_hold_state("bad;rm -rf /", true).unwrap_err();
        assert!(error.to_string().contains("无效的软件包名称"), "{error}");
    }

    #[test]
    fn rejects_missing_install_file_before_pkexec() {
        let error = local_install_args(Path::new("/no/such/package.deb")).unwrap_err();
        assert!(error.to_string().contains("找不到安装包"), "{error}");
    }
}
