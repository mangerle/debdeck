//! 安装窗口与操作弹窗共用的可折叠日志视图。
//!
//! 日志内容由各自的业务状态拥有，本模块只负责稳定的滚动区域与主题化展示；
//! 两个窗口共用同一套高度、字号和留白，避免同类信息在不同弹窗中呈现不一致。

use gpui_kit::component::button::Button;
use gpui_kit::component::{Theme, v_flex};
use gpui_kit::*;

/// 绘制可折叠日志抽屉。
///
/// `toggle` 由调用方创建，以便保留各窗口稳定的控件 ID 与交互文案。
/// `viewport_id` 必须在同一窗口内唯一，确保滚动位置在重绘时保持稳定。
pub(crate) fn drawer(
    toggle: Button,
    logs: &[String],
    expanded: bool,
    viewport_id: &'static str,
    empty_text: &'static str,
    theme: &Theme,
) -> AnyElement {
    let mut container = v_flex().w_full().items_center().gap(px(6.0)).child(toggle);
    if expanded {
        container = container.child(log_viewport(logs, viewport_id, empty_text, theme));
    }
    container.into_any_element()
}

fn log_viewport(
    logs: &[String],
    viewport_id: &'static str,
    empty_text: &'static str,
    theme: &Theme,
) -> AnyElement {
    let muted = theme.muted_foreground;
    let bg = theme.muted;
    let border = theme.border;
    let mono = theme.mono_font_family.clone();

    let mut box_div = div()
        .id(viewport_id)
        .w_full()
        .min_h(px(72.0))
        .max_h(px(160.0))
        .overflow_y_scroll()
        .p(px(8.0))
        .rounded(px(8.0))
        .bg(bg)
        .border_1()
        .border_color(border);

    if logs.is_empty() {
        box_div = box_div.child(
            div()
                .text_xs()
                .font_family(mono)
                .text_color(muted)
                .child(empty_text),
        );
    } else {
        let mut lines = v_flex().w_full().gap(px(2.0));
        for line in logs {
            lines = lines.child(
                div()
                    .text_xs()
                    .font_family(mono.clone())
                    .text_color(muted)
                    .whitespace_normal()
                    .child(line.clone()),
            );
        }
        box_div = box_div.child(lines);
    }

    box_div.into_any_element()
}
