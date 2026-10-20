//! freedesktop 桌面条目（`.desktop`）：解析、图标查找与启动。
//!
//! 解析遵守 Desktop Entry Specification 的关键规则：只认 `[Desktop Entry]`
//! 段、`Type=Application`、排除 `NoDisplay`/`Hidden`、尊重 `TryExec`、
//! `OnlyShowIn`/`NotShowIn` 与当前会话桌面的匹配，并按环境变量里的 locale
//! 顺序挑选本地化字段。
//!
//! 子模块：[`entry`] 解析、[`icon`] 图标主题查找、[`launch`] 启动进程。

pub mod entry;
pub mod icon;
pub mod launch;

pub use entry::parse_desktop_file;
pub use icon::resolve_icon_path;
pub use launch::launch_desktop;
