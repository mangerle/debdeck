//! 非模态的反馈：加载页与自动消失的轻提示。

use eframe::egui::{self, Align2, Id, Label, RichText, Ui, Vec2};

use crate::app::LoadState;
use crate::ui::{
    View,
    theme::{ACCENT, DANGER},
};
use crate::util::APP_NAME;

/// 轻提示：贴在窗口下沿，几秒后自己消失。
///
/// 它同时负责清理过期提示，因此调用方不需要额外的计时器。
pub(crate) fn show_toast(view: &mut View, ui: &mut Ui) {
    let Some(text) = view.dialogs.toast_text() else {
        return;
    };
    egui::Area::new(Id::unique("toast_area"))
        .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -48.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(ui.visuals().extreme_bg_color)
                .show(ui, |ui| {
                    ui.colored_label(ACCENT, text);
                });
        });
    // 到点要有人负责让它消失：主动约一次重绘，不依赖鼠标事件。
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));
}

/// 首屏加载页。返回用户是否按了「重试」。
///
/// 加载中显示阶段文字；失败时显示错误详情与重试按钮 —— 只转圈不报错会让用户
/// 以为程序卡死。
pub(crate) fn show_loading_panel(ui: &mut Ui, load: &LoadState) -> bool {
    let mut retry = false;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(140.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(APP_NAME).size(30.0).strong());
            ui.add_space(8.0);
            if let Some(error) = load.error() {
                ui.colored_label(DANGER, "无法读取软件包数据");
                ui.add_space(8.0);
                ui.add(Label::new(error).wrap());
                ui.add_space(10.0);
                retry = ui.button("重试").clicked();
            } else {
                ui.spinner();
                ui.add_space(8.0);
                let stage = load.stage();
                ui.label(RichText::new(stage).color(ui.visuals().weak_text_color()));
            }
        });
    });
    retry
}
