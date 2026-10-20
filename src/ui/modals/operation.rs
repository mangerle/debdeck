//! 操作进度框：运行中显示进度，结束后显示结果。

use std::sync::atomic::Ordering;

use eframe::egui::{self, Align, Id, Label, Layout, ProgressBar, RichText, ScrollArea, Ui};

use crate::app::{ModalOutcome, Operation};
use crate::ui::theme::{DANGER, SUCCESS};
use crate::util::format_bytes;

/// 绘制操作框。返回 `None` 表示用户关闭了它。
pub(crate) fn show(
    ctx: &egui::Context,
    mut operation: Operation,
    outcomes: &mut Vec<ModalOutcome>,
) -> Option<Operation> {
    let mut close = false;
    let mut cancel = false;
    let mut open_dir = false;

    let response = egui::Modal::new(Id::unique("operation_modal")).show(ctx, |ui: &mut Ui| {
        ui.set_min_width(480.0);
        ui.heading(&operation.title);
        ui.add_space(8.0);
        match &operation.result {
            None => draw_running(ui, &operation, &mut cancel),
            Some(Ok(message)) => {
                let offer_dir = should_offer_open_dir(&operation);
                draw_success(ui, message, offer_dir, &mut close, &mut open_dir);
            }
            Some(Err(error)) => draw_failure(ui, error, &mut close),
        }
    });

    if cancel {
        if let Some(token) = &operation.cancel {
            token.store(true, Ordering::Relaxed);
        }
        operation.cancel_requested = true;
    }

    let dismissed = close
        || (response.should_close() && operation.result.is_some())
        // 没有取消句柄的操作（提权命令）无法中止，Esc 不该让它悄悄消失。
        || (response.should_close() && operation.cancel.is_none());
    if dismissed {
        return None;
    }
    if open_dir && let Some(path) = operation.open_dir.clone() {
        outcomes.push(ModalOutcome::OpenDir(path));
    }
    Some(operation)
}

/// 运行中：转圈 + 阶段文字 + 进度条（有总量时）。
fn draw_running(ui: &mut Ui, operation: &Operation, cancel: &mut bool) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.add(Label::new(&operation.message).wrap());
    });

    if let Some((received, total)) = operation.progress {
        ui.add_space(10.0);
        if total > 0 {
            ui.add(
                ProgressBar::new(progress_fraction(received, total)).text(format!(
                    "{} / {}",
                    format_bytes(received),
                    format_bytes(total)
                )),
            );
        } else {
            // 源未给出 Content-Length：只显示已下载量，不画进度条。
            ui.label(format!("已下载 {}", format_bytes(received)));
        }
    }

    ui.add_space(16.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if operation.cancel.is_some()
            && ui
                .add_enabled(!operation.cancel_requested, egui::Button::new("取消"))
                .clicked()
        {
            *cancel = true;
        }
    });
    if operation.cancel_requested {
        ui.label(
            RichText::new("正在取消，请稍候…")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    }
}

fn draw_success(
    ui: &mut Ui,
    message: &str,
    offer_dir: bool,
    close: &mut bool,
    open_dir: &mut bool,
) {
    ui.colored_label(SUCCESS, RichText::new("操作完成").strong());
    ui.add_space(8.0);
    ui.add(Label::new(message).wrap());
    ui.add_space(16.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.button("关闭").clicked() {
            *close = true;
        }
        if offer_dir && ui.button("打开目录").clicked() {
            *open_dir = true;
        }
    });
}

fn draw_failure(ui: &mut Ui, error: &str, close: &mut bool) {
    ui.colored_label(DANGER, RichText::new("操作失败").strong());
    ui.add_space(8.0);
    ScrollArea::vertical()
        .id_salt("operation_error")
        .max_height(260.0)
        .show(ui, |ui| {
            ui.add(Label::new(error).wrap());
        });
    ui.add_space(16.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.button("关闭").clicked() {
            *close = true;
        }
    });
}

/// 进度百分比。仅用于绘制：字节数远小于 2^53，`f64 → f32` 的截断在进度条上不可见。
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn progress_fraction(received: u64, total: u64) -> f32 {
    (received.min(total) as f64 / total as f64) as f32
}

/// 「打开目录」按钮只在产物已生成时才有意义：失败时不应出现该按钮。
pub(crate) fn should_offer_open_dir(operation: &Operation) -> bool {
    operation.open_dir.is_some() && matches!(operation.result, Some(Ok(_)))
}
