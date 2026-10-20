//! 线程与事件层：UI 线程与后端工作之间的唯一通道。
//!
//! 依赖方向：`app` 通过 [`TaskManager`] 投递任务、通过事件接收端取回结果；
//! `backend` 只被这里的 worker 线程调用。任何模块都不应该绕过 [`BackendEvent`]
//! 从后端线程直接改应用状态。

mod events;
mod instance;
mod tasks;
mod watcher;

pub(crate) use events::{
    BackendEvent, CatalogUpdate, OperationId, UpgradableReport, Waker, report,
};
pub use instance::InstanceLock;
pub(crate) use tasks::{CancelToken, DownloadRequest, TaskManager};
