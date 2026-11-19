//! DebDeck：Ubuntu 的 APT/dpkg 应用管理器（Rust + GPUI）。
//!
//! # 分层与依赖方向
//!
//! 依赖只能从上层指向下层，同层之间不互相引用：
//!
//! ```text
//! ui ──► app ──► runtime ──► backend ──► model / deb822 / util
//! ```
//!
//! * `model`、`deb822`、`util` —— 纯数据与纯函数，不做 IO。
//! * `backend` —— 唯一读取系统文件、执行子进程与网络请求的层；所有路径经
//!   `backend::FsLayout` 注入，便于 fixture 测试。
//! * `runtime` —— 线程、任务队列与事件通道；把 backend 的结果转成 `BackendEvent`。
//! * `app` —— 应用状态与更新逻辑（ Elm 式：状态 + 事件 + `Action`）。
//! * `ui` —— 只做绘制：读状态、产出 `Action`，不直接调用 backend。
//!
//! 只有 [`app::run`] 是对外接口；`main.rs` 调用它，其余模块对 crate 外部不可见。

pub mod app;

pub(crate) mod backend;
pub(crate) mod deb822;
pub(crate) mod model;
pub(crate) mod runtime;
pub(crate) mod ui;
pub(crate) mod update;
pub(crate) mod util;
