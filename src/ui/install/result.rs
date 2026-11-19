//! 安装结果页：成功摘要与失败详情。

use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::app::InstallApp;
use crate::model::DebFile;

use super::actions::icon_tile;

/// 成功：结果说明。
pub(super) fn success(file: &DebFile, message: &str, cx: &mut Context<InstallApp>) -> AnyElement {
    let success = cx.theme().success;
    let muted = cx.theme().muted_foreground;
    v_flex()
        .w_full()
        .items_center()
        .gap(px(10.0))
        .child(icon_tile(IconName::CircleCheck, success, 56.0, true))
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child("安装完成"),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .text_center()
                .whitespace_normal()
                .child(message.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("{} 现在可以从应用菜单启动。", file.metadata.name)),
        )
        .into_any_element()
}

/// 失败/无法安装：错误详情卡片。
pub(super) fn failure(
    title: &str,
    subtitle: &str,
    message: &str,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .gap(px(10.0))
        .child(failure_heading(
            title,
            subtitle,
            theme.muted_foreground,
            theme.danger,
        ))
        .child(failure_details(
            message,
            theme.muted,
            theme.border,
            theme.mono_font_family.clone(),
        ))
        .into_any_element()
}

fn failure_heading(title: &str, subtitle: &str, muted: Hsla, danger: Hsla) -> AnyElement {
    h_flex()
        .items_start()
        .gap(px(12.0))
        .child(icon_tile(IconName::CircleX, danger, 44.0, false))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .whitespace_normal()
                        .child(subtitle.to_string()),
                ),
        )
        .into_any_element()
}

fn failure_details(message: &str, muted: Hsla, border: Hsla, mono: SharedString) -> AnyElement {
    div()
        .id("install-error")
        .min_h(px(60.0))
        .max_h(px(160.0))
        .overflow_y_scroll()
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(muted)
        .border_1()
        .border_color(border)
        .child(
            div()
                .text_xs()
                .font_family(mono)
                .whitespace_normal()
                .child(message.to_string()),
        )
        .into_any_element()
}
