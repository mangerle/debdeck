//! 操作进度框：运行中显示进度，结束后显示结果。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::app::{DebDeckApp, Operation};
use crate::runtime::AuthPrompt;
use crate::ui::password_input::PasswordInputState;
use crate::util::format_bytes;

use super::{actions, body, overlay, title};

/// 绘制操作框。
pub(crate) fn show(
    operation: &Operation,
    password_input: &Entity<PasswordInputState>,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let dismissible = operation.result.is_some();
    let card = match &operation.result {
        None => match operation.auth_prompt.as_ref() {
            Some(prompt) => auth_prompt(operation, prompt, password_input, cx),
            None => running(operation, cx),
        },
        Some(Ok(message)) => success(operation, message, cx),
        Some(Err(error)) => failure(operation, error, cx),
    };
    overlay(card, dismissible, cx)
}

/// 需要管理员认证：在弹窗内部直接提供密码输入框与错误反馈。
fn auth_prompt(
    operation: &Operation,
    prompt: &AuthPrompt,
    password_input: &Entity<PasswordInputState>,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let danger = cx.theme().danger;
    let muted = cx.theme().muted_foreground;
    let message = if prompt.message.trim().is_empty() {
        "执行该操作需要管理员权限，请输入密码以继续。".to_string()
    } else {
        prompt.message.clone()
    };
    let prompt_label = if prompt.prompt.trim().is_empty() {
        "密码：".to_string()
    } else {
        prompt.prompt.clone()
    };

    let cancel = Button::new("cancel-operation-auth")
        .label("取消")
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| {
            this.dialogs.request_cancel();
            cx.notify();
        }));
    let submit = Button::new("submit-operation-auth")
        .label("授权并继续")
        .primary()
        .on_click(cx.listener(|this, _, _, cx| {
            this.submit_operation_password(cx);
        }));

    v_flex()
        .gap(px(10.0))
        .child(title(operation.title.clone()))
        .child(div().w_full().min_w(px(0.0)).child(body(message)))
        .children(prompt.error.as_ref().map(|error| {
            div()
                .text_xs()
                .text_color(danger)
                .whitespace_normal()
                .child(error.clone())
        }))
        .child(div().text_xs().text_color(muted).child(prompt_label))
        .child(password_input.clone())
        .child(actions().child(cancel).child(submit))
        .into_any_element()
}

/// 运行中：转圈 + 阶段文字 + 进度条（有总量时）。
fn running(operation: &Operation, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let mut card = v_flex()
        .gap(px(10.0))
        .child(title(operation.title.clone()))
        .child(
            h_flex()
                .w_full()
                .gap(px(8.0))
                .items_start()
                .child(div().pt(px(2.0)).child(Spinner::new().small()))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(body(operation.message.clone())),
                ),
        );

    if let Some((received, total)) = operation.progress {
        card = card.child(progress_block(received, total, cx));
    }

    let cancel = operation
        .is_cancellable()
        .then(|| cancel_button(operation, cx));
    if operation.cancel_requested {
        card = card.child(div().text_xs().text_color(muted).child("正在取消，请稍候…"));
    }

    card.child(actions().children(cancel)).into_any_element()
}

fn cancel_button(operation: &Operation, cx: &mut Context<DebDeckApp>) -> AnyElement {
    Button::new("cancel-operation")
        .label("取消")
        .ghost()
        .disabled(operation.cancel_requested)
        .on_click(cx.listener(|this, _, _, cx| {
            this.dialogs.request_cancel();
            cx.notify();
        }))
        .into_any_element()
}

fn progress_block(received: u64, total: u64, cx: &Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    if total == 0 {
        return div()
            .text_xs()
            .text_color(muted)
            .child(format!("已下载 {}", format_bytes(received)))
            .into_any_element();
    }

    v_flex()
        .gap(px(4.0))
        .child(Progress::new("operation-progress").value(progress_percent(received, total)))
        .child(
            h_flex()
                .justify_between()
                .text_xs()
                .text_color(muted)
                .child(format!(
                    "{} / {}",
                    format_bytes(received),
                    format_bytes(total)
                )),
        )
        .into_any_element()
}

fn success(operation: &Operation, message: &str, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let success = cx.theme().success;
    let open_dir = if matches!(&operation.result, Some(Ok(_))) {
        operation.open_dir.clone()
    } else {
        None
    };
    let open = open_dir.map(|path| {
        Button::new("open-operation-dir")
            .label("打开目录")
            .secondary()
            .on_click(cx.listener(move |_this, _, _, cx| {
                DebDeckApp::open_directory(path.clone(), cx);
                cx.notify();
            }))
            .into_any_element()
    });

    v_flex()
        .gap(px(12.0))
        .child(title(operation.title.clone()))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(success)
                .child("操作完成"),
        )
        .child(body(message.to_string()))
        .child(
            actions().children(open).child(
                Button::new("close-operation")
                    .label("关闭")
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dialogs.modal = None;
                        cx.notify();
                    })),
            ),
        )
        .into_any_element()
}

fn failure(operation: &Operation, error: &str, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let danger = cx.theme().danger;
    v_flex()
        .gap(px(12.0))
        .child(title(operation.title.clone()))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(danger)
                .child("操作失败"),
        )
        .child(
            div()
                .id("operation-error")
                .max_h(px(260.0))
                .overflow_y_scroll()
                .child(body(error.to_string())),
        )
        .child(
            actions().child(
                Button::new("close-failed-operation")
                    .label("关闭")
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dialogs.modal = None;
                        cx.notify();
                    })),
            ),
        )
        .into_any_element()
}

/// 进度百分比。仅用于绘制：字节数远小于 2^53，`f64 → f32` 的截断不可见。
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "进度值仅用于图形绘制，调用方已保证总量非零且数值远小于 2^53"
)]
fn progress_percent(received: u64, total: u64) -> f32 {
    (received.min(total) as f64 / total as f64 * 100.0) as f32
}
