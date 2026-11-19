//! 非模态的反馈：加载页与自动消失的轻提示。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::v_flex;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::*;

use crate::app::DebDeckApp;
use crate::util::APP_NAME;

/// 首屏加载页。失败时显示错误详情与重试按钮 —— 只转圈不报错会让用户
/// 以为程序卡死。
pub(crate) fn show_loading(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let load = app.catalog.load();
    let body = if let Some(error) = load.error() {
        loading_failure(error, cx)
    } else {
        loading_progress(load.stage(), cx)
    };
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(
            div()
                .text_3xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child(APP_NAME),
        )
        .child(body)
        .into_any_element()
}

fn loading_failure(error: &str, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    v_flex()
        .max_w(px(680.0))
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(danger)
                .child("无法读取软件包数据"),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .whitespace_normal()
                .child(error.to_string()),
        )
        .child(
            Button::new("retry-load")
                .label("重试")
                .primary()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.retry_initial_load();
                    cx.notify();
                })),
        )
        .into_any_element()
}

fn loading_progress(stage: &str, cx: &Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .items_center()
        .gap(px(10.0))
        .child(Spinner::new().large())
        .child(div().text_sm().text_color(muted).child(stage.to_string()))
        .into_any_element()
}

/// 轻提示：贴在窗口下沿，几秒后由应用层的定时任务清理。
pub(crate) fn show_toast(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> Option<AnyElement> {
    let text = app.dialogs.toast_text()?;
    let theme = cx.theme();
    Some(
        div()
            .absolute()
            .bottom(px(48.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(14.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .text_sm()
                    .text_color(theme.primary)
                    .child(text),
            )
            .into_any_element(),
    )
}
