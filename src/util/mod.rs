//! 应用身份常量与跨层工具的聚合入口。
//!
//! 本目录只放**纯函数**：不读写系统全局状态、不依赖 egui、不发起子进程
//! （`system`/`paths` 里读取环境变量与 `PATH` 的部分除外）。
//! 任何需要访问 dpkg/apt 文件系统的逻辑属于 `backend`，不要放进这里。
//!
//! 子模块按职责划分，这里统一重新导出，调用方继续使用 `crate::util::<函数>`。

pub mod deb_version;
pub mod desktop_exec;
pub mod format;
pub mod paths;
pub mod search;
pub mod system;
pub mod throttle;

/// 界面展示名称，同时用于窗口标题与加载页 logo。
pub const APP_NAME: &str = "DebDeck";
/// 应用标识：用作单实例锁文件名、`notify-send -a` 的 sender，以及
/// `.desktop` 里的 `StartupWMClass` 对齐目标。
pub const APP_ID: &str = "debdeck";

pub use deb_version::debian_version_cmp;
pub use desktop_exec::desktop_exec_args;
pub use format::{format_bytes, format_kib, format_time, hex_lower};
pub use paths::{AppPaths, home_dir};
pub use search::{build_search_text, matches_tokens, query_tokens};
pub use system::{command_exists, current_arch};
pub use throttle::Throttle;

// 文件名构造只服务于本 crate 的下载/打包路径，不对外暴露。
pub(crate) use paths::{package_file_name, sanitize_file_component};
