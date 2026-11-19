//! 本地安装包窗口的运行时门面：收口 `.deb` 预检、本地状态查询、polkit 安装会话与桌面启动。

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::backend::deb_file;
use crate::backend::desktop::entry::parse_desktop_file;
use crate::backend::desktop::launch_desktop;
use crate::backend::polkit_agent;
use crate::model::{DebFile, DesktopEntry};
use crate::util::{command_exists, run_command};

pub(crate) use crate::backend::polkit_agent::{AuthResponse, InstallEvent, InstallSession};

/// 预检时对本机已安装状态的对比结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstalledStatus {
    /// 系统尚未安装该软件包。
    NotInstalled,
    /// 已安装相同版本，将执行覆盖重装。
    SameVersion,
    /// 已安装旧版本，将执行升级。
    Upgrade { installed: String },
    /// 已安装更高版本，将执行降级安装。
    Downgrade { installed: String },
    /// 已安装其他版本但无法确定高低顺序。
    OtherVersion { installed: String },
}

/// 预检完成后的安装包及本机状态。
#[derive(Clone, Debug)]
pub(crate) struct InspectedDeb {
    pub(crate) file: DebFile,
    pub(crate) installed: InstalledStatus,
}

/// 检查并读取本地 `.deb` 安装包的元数据、大小与本机当前安装状态。
///
/// # Errors
/// 文件不存在、格式非法或 `dpkg-deb` 解析失败时返回完整错误描述。
pub(crate) fn inspect_deb_file(path: &Path) -> Result<InspectedDeb, String> {
    let file = deb_file::inspect(path).map_err(|error| format!("{error:#}"))?;
    let installed = query_installed_status(&file.metadata.name, &file.metadata.version);
    Ok(InspectedDeb { file, installed })
}

/// 启动内嵌 polkit 授权与安装后台会话。
///
/// # Errors
/// 无法启动提权会话时返回错误描述。
pub(crate) fn start_install_session(path: &Path, pkg: String) -> Result<InstallSession, String> {
    polkit_agent::start(path, pkg).map_err(|error| format!("{error:#}"))
}

/// 安装完成后查找该软件包提供的首个可启动桌面条目。
pub(crate) fn find_launchable_desktop(pkg: &str) -> Option<DesktopEntry> {
    if !command_exists("dpkg") {
        return None;
    }
    let mut command = Command::new("dpkg");
    command.args(["-L", pkg]);
    let output = run_command(&mut command, Duration::from_secs(5)).ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.ends_with(".desktop") {
            continue;
        }
        let path = Path::new(trimmed);
        if !path.is_file() {
            continue;
        }
        if let Some(entry) = parse_desktop_file(path)
            && !entry.exec.trim().is_empty()
        {
            return Some(entry);
        }
    }
    None
}

/// 在后台启动已安装的桌面应用。
///
/// # Errors
/// 桌面文件无效或拉起子进程失败时返回错误描述。
pub(crate) fn launch_installed(entry: &DesktopEntry) -> Result<(), String> {
    launch_desktop(entry).map_err(|error| format!("{error:#}"))
}

fn query_installed_status(pkg: &str, candidate_version: &str) -> InstalledStatus {
    if !command_exists("dpkg-query") {
        return InstalledStatus::NotInstalled;
    }
    let mut command = Command::new("dpkg-query");
    command.args(["-W", "-f=${db:Status-Abbrev}\t${Version}", pkg]);
    let Ok(output) = run_command(&mut command, Duration::from_secs(5)) else {
        return InstalledStatus::NotInstalled;
    };
    if !output.status.success() {
        return InstalledStatus::NotInstalled;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some((status, installed_ver)) = text.trim().split_once('\t') else {
        return InstalledStatus::NotInstalled;
    };
    if !status.starts_with("ii") {
        return InstalledStatus::NotInstalled;
    }
    let installed_ver = installed_ver.trim();
    if installed_ver == candidate_version {
        return InstalledStatus::SameVersion;
    }
    compare_deb_versions(installed_ver, candidate_version)
}

fn compare_deb_versions(installed: &str, candidate: &str) -> InstalledStatus {
    if !command_exists("dpkg") {
        return InstalledStatus::OtherVersion {
            installed: installed.to_string(),
        };
    }
    if dpkg_compare(candidate, "gt", installed) {
        InstalledStatus::Upgrade {
            installed: installed.to_string(),
        }
    } else if dpkg_compare(candidate, "lt", installed) {
        InstalledStatus::Downgrade {
            installed: installed.to_string(),
        }
    } else {
        InstalledStatus::OtherVersion {
            installed: installed.to_string(),
        }
    }
}

fn dpkg_compare(lhs: &str, op: &str, rhs: &str) -> bool {
    let mut command = Command::new("dpkg");
    command.args(["--compare-versions", lhs, op, rhs]);
    run_command(&mut command, Duration::from_secs(3)).is_ok_and(|output| output.status.success())
}
