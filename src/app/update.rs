//! 事件接收与状态更新：后端线程的结果如何变成界面状态。
//!
//! 后端线程发送事件并唤醒主线程；[`DebDeckApp::poll_events`] 在唤醒任务里
//! 清空事件并更新数据（可能顺带触发刷新），随后请求一次重绘。界面回调直接
//! 调用 `handle_action` 产生副作用，不再经过「绘制 → 返回动作」的中间层。
//!
//! 绘制与状态更新仍然分离：后端只能改状态，不能碰窗口；同一动作的视觉反馈
//! 在下一帧出现，这是事件驱动界面的正常工作方式。

use crate::app::state::DebDeckApp;
use crate::runtime::BackendEvent;

impl DebDeckApp {
    pub(crate) fn poll_events(&mut self) {
        // 每帧最多处理 64 条事件：加载阶段的 Stage 事件很密集，设上限保证界面
        // 不会因为事件洪水而卡住。未处理的事件留在通道里，下一帧继续。
        const MAX_EVENTS_PER_FRAME: usize = 64;

        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Ok(event) = self.rx.try_recv() else { break };
            self.apply_event(event);
        }
    }

    fn apply_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::Stage(stage) => self.catalog.set_stage(stage),
            BackendEvent::CatalogUpdated(update) => {
                self.catalog.apply(update);
            }
            BackendEvent::DpkgChanged => {
                // 只有已安装数据就绪后才需要响应；操作进行中则由 Dialogs 记账。
                if self.catalog.is_ready() && self.dialogs.note_external_change() {
                    self.request_installed_refresh();
                }
            }
            BackendEvent::OperationProgress {
                id,
                message,
                progress,
            } => {
                self.dialogs
                    .record_operation_progress(id, message, progress);
            }
            BackendEvent::OperationFinished {
                id,
                refresh_installed,
                result,
            } => {
                if let Err(error) = result.as_ref() {
                    tracing::warn!("操作 {id:?} 失败：{error}");
                }
                let matched = self.dialogs.record_operation_result(id, result);
                // 只有仍然显示着的那个操作才驱动刷新；后台被替换掉的任务结果
                // 只记日志，不打断当前模态。
                if matched && refresh_installed {
                    self.request_installed_refresh();
                }
                // 操作期间积压的外部变更，现在补上。
                if matched && self.dialogs.take_pending_refresh() {
                    self.request_installed_refresh();
                }
            }
            event @ (BackendEvent::SelfUpdateChecked(_)
            | BackendEvent::SelfUpdateDownloadProgress { .. }
            | BackendEvent::SelfUpdateDownloaded(_)
            | BackendEvent::SelfUpdateInstalled(_)) => {
                self.apply_self_update_event(event);
            }
        }
    }

    /// 请求刷新已安装数据。
    ///
    /// 重复请求的去重在 `TaskManager` 里做（排队期间幂等），所以这里不再自己
    /// 维护 `refresh_in_flight` 标志。
    pub(crate) fn request_installed_refresh(&mut self) {
        if self.catalog.is_ready() {
            self.tasks.refresh_installed();
        }
    }

    /// 工具栏「刷新」：软件源与已安装数据都重读。
    pub(crate) fn refresh_all(&mut self) {
        if !self.can_refresh() {
            return;
        }
        self.catalog.set_repository_loading(true);
        self.tasks.refresh_repository();
        self.request_installed_refresh();
    }

    /// 刷新是否可用：操作进行中、或软件源正在扫描时不给点，避免排队堆积。
    pub(crate) fn can_refresh(&self) -> bool {
        !self.dialogs.is_operation_running() && !self.catalog.repository_loading()
    }

    /// 首屏加载失败后的重试。
    pub(crate) fn retry_initial_load(&mut self) {
        self.catalog.load_mut().reset_to_loading("正在重试…");
        self.tasks.load_initial();
    }
}
