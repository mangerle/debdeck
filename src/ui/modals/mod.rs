//! 模态框：一次一个，绘制完把结果交回应用层。
//!
//! 模态在绘制期间从 [`Dialogs`](crate::app::Dialogs) 取出、结束后放回；
//! 按钮回调通过 `cx.listener` 直接修改应用状态，因此绘制函数不需要持有
//! `TaskManager`，也不认识提权与文件写入。

pub mod confirm;
pub mod download;
pub mod message;
pub mod operation;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::h_flex;
use gpui_kit::*;

use crate::app::{DebDeckApp, ModalState};

/// 绘制当前模态，返回覆盖层；没有模态时返回 `None`。
pub(crate) fn show(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> Option<AnyElement> {
    let modal = app.dialogs.take_modal()?;
    let element = match &modal {
        ModalState::Confirm(confirm) => confirm::show(app, confirm, cx),
        ModalState::Download(dialog) => download::show(app, dialog, cx),
        ModalState::Operation(operation) => operation::show(app, operation, cx),
        ModalState::Message { title, message } => message::show(app, title, message, cx),
    };
    // 放回：模态在回调执行前必须仍然存在，否则后台进度事件会找不到它。
    // 回调若关闭/替换模态，会在事件阶段覆盖这次放回。
    app.dialogs.restore_modal(Some(modal));
    Some(element)
}

/// 统一的遮罩 + 卡片；`dismissible` 决定点击遮罩是否关闭。
pub(super) fn overlay(
    card: AnyElement,
    dismissible: bool,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let border = cx.theme().border;
    let foreground = cx.theme().popover_foreground;
    let surface = cx.theme().popover;

    let mut backdrop = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0.0, 0.0, 0.0, 0.38));
    if dismissible {
        backdrop = backdrop.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.dialogs.modal = None;
                cx.notify();
            }),
        );
    }

    backdrop
        .child(
            div()
                .w(px(480.0))
                .max_h(px(680.0))
                .p(px(20.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(border)
                .bg(surface)
                .text_color(foreground)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(card),
        )
        .into_any_element()
}

/// 模态标题。
pub(super) fn title(text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// 正文。新模态都允许换行，避免长错误信息撑破卡片。
pub(super) fn body(text: impl Into<SharedString>) -> Div {
    div().text_sm().whitespace_normal().child(text.into())
}

/// 右下角的按钮行。
pub(super) fn actions() -> Div {
    h_flex()
        .justify_end()
        .items_center()
        .gap(px(8.0))
        .pt(px(8.0))
}
