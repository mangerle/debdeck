//! 桌面集成副作用：系统通知与文件管理器。
//!
//! 两者都是「尽力而为」：失败不应影响主流程，因此返回 `Result` 让调用方决定
//! 是否提示，而通知本身连失败都只记日志。

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::backend::{BackendError, Result};

use crate::util::{APP_ID, run_command};

/// 发送 freedesktop 系统通知。
///
/// 用于长操作（下载、重打包）完成后，即使窗口被切走用户也能收到结果。
/// `notify-send` 缺失（未装 libnotify-bin）时静默忽略，这是 Depends 里的
/// Recommends 而非硬依赖。
pub fn notify(summary: &str, body: &str) {
    let mut command = Command::new("notify-send");
    command.arg("-a").arg(APP_ID).arg(summary).arg(body);
    if let Err(error) = run_command(&mut command, Duration::from_secs(10)) {
        tracing::debug!("发送系统通知失败：{error}");
    }
}

/// 在文件管理器中打开目录（不存在则先创建）。
pub fn open_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .map_err(|source| BackendError::io("创建下载目录", path.display(), source))?;
    let mut command = Command::new("xdg-open");
    command.arg(path);
    run_command(&mut command, Duration::from_secs(10))
        .map_err(|source| BackendError::command("打开文件管理器", source))?;
    Ok(())
}
