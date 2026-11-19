//! 桌面与系统外壳动作的运行时门面：收口 `app` 对 `backend::desktop` 与 `backend::shell` 的调用。

use std::path::Path;

use crate::backend::desktop::{launch_desktop as backend_launch, menu};
use crate::backend::shell;
use crate::model::{DesktopEntry, MenuEntryState};
use crate::runtime::events::report_error;

/// 启动指定桌面条目对应的应用程序。
///
/// # Errors
/// 桌面文件无效或拉起子进程失败时返回中文错误描述。
pub(crate) fn launch_desktop(entry: &DesktopEntry) -> Result<(), String> {
    backend_launch(entry).map_err(|error| report_error(&error))
}

/// 在用户应用菜单中隐藏或恢复指定的 `.desktop` 条目。
///
/// # Errors
/// 覆写桌面文件失败或存在第三方管理条目时返回中文错误描述。
pub(crate) fn set_menu_visibility(path: &Path, hidden: bool) -> Result<MenuEntryState, String> {
    menu::set_hidden(path, hidden).map_err(|error| report_error(&error))
}

/// 在系统默认文件管理器中打开指定目录。
///
/// # Errors
/// 无法拉起文件管理器时返回中文错误描述。
pub(crate) fn open_directory(path: &Path) -> Result<(), String> {
    shell::open_directory(path).map_err(|error| report_error(&error))
}

/// 发送桌面通知。
pub(crate) fn notify(summary: &str, body: &str) {
    shell::notify(summary, body);
}
