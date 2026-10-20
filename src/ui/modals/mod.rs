//! 模态框：一次一个，绘制完把结果交回应用层。
//!
//! 每个子模块的函数形态都是
//! `fn show(...) -> Option<ModalState>`：返回 `Some` 表示模态还要留着，
//! `None` 表示关闭。用户的选择被翻译成 [`ModalOutcome`] 塞进 `outcomes`，
//! 于是绘制代码既不认识 `TaskManager`，也不认识提权与文件写入。

pub mod confirm;
pub mod download;
pub mod message;
pub mod operation;

use eframe::egui::Ui;

use crate::app::{ModalOutcome, ModalState};
use crate::ui::View;

/// 绘制当前模态，返回需要执行的结果。
pub(crate) fn show(view: &mut View, _ui: &mut Ui) -> Vec<ModalOutcome> {
    let View {
        ctx,
        catalog,
        dialogs,
        paths,
        ..
    } = view;

    let Some(modal) = dialogs.take_modal() else {
        return Vec::new();
    };
    let mut outcomes = Vec::new();

    let kept = match modal {
        ModalState::Confirm(confirm) => confirm::show(&*ctx, confirm, paths, &mut outcomes),
        ModalState::Download(dialog) => download::show(&*ctx, catalog, dialog, &mut outcomes),
        ModalState::Operation(operation) => {
            operation::show(&*ctx, operation, &mut outcomes).map(ModalState::Operation)
        }
        ModalState::Message { title, message } => message::show(&*ctx, &title, &message),
    };

    dialogs.restore_modal(kept);
    outcomes
}
