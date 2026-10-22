//! 模态与提示的持有者。
//!
//! 一次只允许一个模态：这是界面刻意接受的约束（后台操作串行进行，用户也不需要
//! 同时看两个进度框）。绘制时把模态取出来、结束再放回（[`Dialogs::take_modal`] /
//! [`Dialogs::restore_modal`]），是为了绕开「绘制代码要借用 self 的其他字段」，
//! 而不是随手写的临时状态搬运。
//!
//! `refresh_after_operation` 修的是一个真实的丢事件问题：外部 apt/dpkg 改动发生时
//! 如果正有一个操作在跑，原先的实现直接忽略这次通知，列表会一直停留在旧数据上，
//! 直到下一次文件变化。现在改为「记下来，操作结束补一次刷新」。

use std::time::{Duration, Instant};

use super::state::{ModalState, Operation};
use crate::runtime::OperationId;

/// 自动消失的轻提示。
pub(crate) struct Toast {
    pub(crate) text: String,
    pub(crate) expires: Instant,
}

/// 轻提示停留时长。
const TOAST_LIFETIME: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct Dialogs {
    pub(crate) modal: Option<ModalState>,
    toast: Option<Toast>,
    refresh_after_operation: bool,
}

impl Dialogs {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            expires: Instant::now() + TOAST_LIFETIME,
        });
    }

    /// 清理提示（定时任务到点或用户关闭时使用）。
    pub(crate) fn clear_toast(&mut self) {
        self.toast = None;
    }

    /// 当前提示文本；顺便清理已过期的提示。
    pub(crate) fn toast_text(&mut self) -> Option<String> {
        match &self.toast {
            None => None,
            Some(toast) if Instant::now() >= toast.expires => {
                self.toast = None;
                None
            }
            Some(toast) => Some(toast.text.clone()),
        }
    }

    /// 是否有操作正在执行（此时不该弹出会遮挡它的消息框）。
    pub(crate) fn is_operation_running(&self) -> bool {
        matches!(&self.modal, Some(ModalState::Operation(operation)) if operation.is_running())
    }

    /// 展示一条纯提示消息；操作进行中时放弃，避免抢走进度对话框。
    pub(crate) fn set_message(&mut self, title: impl Into<String>, message: impl Into<String>) {
        if self.is_operation_running() {
            return;
        }
        self.modal = Some(ModalState::Message {
            title: title.into(),
            message: message.into(),
        });
    }

    /// 用操作对话框替换当前模态。
    pub(crate) fn begin_operation(&mut self, operation: Operation) {
        self.modal = Some(ModalState::Operation(operation));
    }

    /// 请求取消当前可取消的操作。没有取消句柄（提权命令）时什么也不做。
    pub(crate) fn request_cancel(&mut self) {
        if let Some(ModalState::Operation(operation)) = &mut self.modal
            && let Some(token) = &operation.cancel
        {
            token.store(true, std::sync::atomic::Ordering::Relaxed);
            operation.cancel_requested = true;
        }
    }

    pub(crate) fn take_modal(&mut self) -> Option<ModalState> {
        self.modal.take()
    }

    pub(crate) fn restore_modal(&mut self, modal: Option<ModalState>) {
        if modal.is_some() {
            self.modal = modal;
        }
    }

    /// 把操作结果写回对应的操作框。返回是否命中当前操作。
    pub(crate) fn record_operation_result(
        &mut self,
        id: OperationId,
        result: Result<String, String>,
    ) -> bool {
        if let Some(ModalState::Operation(operation)) = &mut self.modal
            && operation.id == id
        {
            operation.result = Some(result);
            return true;
        }
        false
    }

    /// 更新操作框里的进度文案与百分比。
    pub(crate) fn record_operation_progress(
        &mut self,
        id: OperationId,
        message: String,
        progress: Option<(u64, u64)>,
    ) {
        if let Some(ModalState::Operation(operation)) = &mut self.modal
            && operation.id == id
        {
            operation.message = message;
            // 有些阶段（如校验、打包）没有百分比，保留上一次的数值。
            if progress.is_some() {
                operation.progress = progress;
            }
        }
    }

    /// 操作进行中收到外部变更通知：先记账。
    pub(crate) fn note_external_change(&mut self) -> bool {
        if self.is_operation_running() {
            self.refresh_after_operation = true;
            false
        } else {
            true
        }
    }

    /// 取回并清除「操作结束后需要补刷新」的标记。
    pub(crate) fn take_pending_refresh(&mut self) -> bool {
        std::mem::replace(&mut self.refresh_after_operation, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_operation() -> Operation {
        Operation {
            id: OperationId(7),
            title: "测试".to_string(),
            message: "进行中".to_string(),
            progress: None,
            result: None,
            cancel: None,
            open_dir: None,
            cancel_requested: false,
        }
    }

    #[test]
    fn message_is_suppressed_while_an_operation_runs() {
        let mut dialogs = Dialogs::new();
        dialogs.set_message("标题", "内容");
        assert!(matches!(dialogs.modal, Some(ModalState::Message { .. })));

        dialogs.begin_operation(running_operation());
        dialogs.set_message("另一个标题", "另一个内容");
        assert!(
            matches!(dialogs.modal, Some(ModalState::Operation(_))),
            "操作进行中不应被消息框顶掉"
        );
    }

    #[test]
    fn external_change_is_deferred_until_operation_finishes() {
        let mut dialogs = Dialogs::new();
        assert!(dialogs.note_external_change(), "空闲时应立即刷新");
        assert!(!dialogs.take_pending_refresh(), "空闲路径不留记账");

        dialogs.begin_operation(running_operation());
        assert!(!dialogs.note_external_change(), "操作进行中要记账");
        assert!(dialogs.take_pending_refresh(), "取回刚记下的账");
        assert!(!dialogs.take_pending_refresh(), "取过之后标记要清掉");
    }

    #[test]
    fn progress_for_other_operations_is_ignored() {
        let mut dialogs = Dialogs::new();
        dialogs.begin_operation(running_operation());
        dialogs.record_operation_progress(OperationId(99), "别人的进度".to_string(), Some((1, 2)));
        let Some(ModalState::Operation(operation)) = &dialogs.modal else {
            panic!("模态应仍是操作框");
        };
        assert_eq!(operation.message, "进行中");
        assert!(operation.progress.is_none());
    }

    #[test]
    fn toast_expires() {
        let mut dialogs = Dialogs::new();
        dialogs.set_toast("已复制");
        assert_eq!(dialogs.toast_text().as_deref(), Some("已复制"));
        // 手工把过期时间调到过去。
        if let Some(toast) = &mut dialogs.toast {
            toast.expires = Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap_or_else(Instant::now);
        }
        assert_eq!(dialogs.toast_text(), None);
        assert!(dialogs.toast.is_none());
    }
}
