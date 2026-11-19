//! 线程与事件层：UI 线程与后端工作之间的唯一通道。
//!
//! 依赖方向：`app` 通过 [`TaskManager`] 投递任务、通过事件接收端取回结果；
//! `backend` 只被这里的 worker 线程调用。任何模块都不应该绕过 [`BackendEvent`]
//! 从后端线程直接改应用状态。

mod desktop;
mod events;
pub(crate) mod install;
mod instance;
mod tasks;
mod watcher;

pub(crate) use crate::backend::self_update::{
    DownloadedUpdate, RecoveryStatus, UpdateInfo, check_and_recover_on_startup,
    restart_current_process,
};
pub(crate) use desktop::{launch_desktop, notify, open_directory, set_menu_visibility};
pub(crate) use events::{BackendEvent, CatalogUpdate, OperationId, UpgradableReport, Waker};
pub use instance::InstanceLock;
pub(crate) use tasks::{CancelToken, DownloadRequest, TaskManager, TaskSubmitError};
