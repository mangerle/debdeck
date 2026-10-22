//! 纯文本提示框：把不适合塞进按钮提示的长信息展示出来。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::v_flex;
use gpui_kit::*;

use crate::app::DebDeckApp;

use super::{actions, body, overlay, title};

/// 错误与警告文本可能很长，限制高度并允许滚动。
const MAX_BODY_HEIGHT: f32 = 320.0;

/// 绘制消息框；只有关闭一条出路。
pub(crate) fn show(
    _app: &mut DebDeckApp,
    title_text: &str,
    message: &str,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let close = Button::new("close-message")
        .label("关闭")
        .primary()
        .on_click(cx.listener(|this, _, _, cx| {
            this.dialogs.modal = None;
            cx.notify();
        }));

    let card = v_flex()
        .gap(px(12.0))
        .child(title(title_text.to_string()))
        .child(
            div()
                .id("message-body")
                .max_h(px(MAX_BODY_HEIGHT))
                .overflow_y_scroll()
                .child(body(message.to_string())),
        )
        .child(actions().child(close))
        .into_any_element();

    overlay(card, true, cx)
}
