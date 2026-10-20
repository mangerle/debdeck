//! 纯文本提示框：把不适合塞进按钮提示的长信息展示出来。

use eframe::egui::{self, Align, Id, Label, Layout, ScrollArea, Ui};

use crate::app::ModalState;

/// 错误与警告文本可能很长，限制高度并允许滚动。
const MAX_BODY_HEIGHT: f32 = 320.0;

/// 绘制消息框；只有关闭一条出路，因此不需要交回任何动作。
pub(crate) fn show(ctx: &egui::Context, title: &str, message: &str) -> Option<ModalState> {
    let mut close = false;
    let response = egui::Modal::new(Id::unique("message_modal")).show(ctx, |ui: &mut Ui| {
        ui.set_min_width(460.0);
        ui.heading(title);
        ui.add_space(8.0);
        ScrollArea::vertical()
            .id_salt("message_scroll")
            .max_height(MAX_BODY_HEIGHT)
            .show(ui, |ui| {
                ui.add(Label::new(message).wrap());
            });
        ui.add_space(16.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("关闭").clicked() {
                close = true;
            }
        });
    });

    if close || response.should_close() {
        None
    } else {
        Some(ModalState::Message {
            title: title.to_string(),
            message: message.to_string(),
        })
    }
}
