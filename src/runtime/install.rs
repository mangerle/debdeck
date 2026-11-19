//! 本地安装包窗口的运行时门面：收口 `.deb` 预检与 polkit 安装会话。

use std::path::Path;

use crate::backend::deb_file;
use crate::backend::polkit_agent;
use crate::model::DebFile;

pub(crate) use crate::backend::polkit_agent::{AuthResponse, InstallEvent, InstallSession};

/// 检查并读取本地 `.deb` 安装包的元数据与大小。
///
/// # Errors
/// 文件不存在、格式非法或 `dpkg-deb` 解析失败时返回完整错误描述。
pub(crate) fn inspect_deb_file(path: &Path) -> Result<DebFile, String> {
    deb_file::inspect(path).map_err(|error| format!("{error:#}"))
}

/// 启动内嵌 polkit 授权与安装后台会话。
///
/// # Errors
/// 无法启动提权会话时返回错误描述。
pub(crate) fn start_install_session(path: &Path, pkg: String) -> Result<InstallSession, String> {
    polkit_agent::start(path, pkg).map_err(|error| format!("{error:#}"))
}
