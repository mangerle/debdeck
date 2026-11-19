//! 安装窗口的流程步骤条、阶段进度条与实时日志面板。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;

use crate::app::InstallApp;
use crate::ui::log_view;

/// 三步流程指示条：确认 → 授权 → 安装。
///
/// `active_step` 为 `0`（确认）、`1`（授权）、`2`（安装）。
pub(super) fn step_indicator(active_step: usize, cx: &Context<InstallApp>) -> AnyElement {
    let primary = cx.theme().primary;
    let success = cx.theme().success;
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let steps = ["1. 确认", "2. 授权", "3. 安装"];

    let mut row = h_flex().w_full().items_center().gap(px(6.0));
    for (idx, label) in steps.iter().enumerate() {
        let (fg, dot_bg) = match idx.cmp(&active_step) {
            std::cmp::Ordering::Less => (success, success),
            std::cmp::Ordering::Equal => (primary, primary),
            std::cmp::Ordering::Greater => (muted, border),
        };
        row = row.child(
            h_flex()
                .items_center()
                .gap(px(5.0))
                .child(div().size(px(6.0)).rounded_full().bg(dot_bg))
                .child(
                    div()
                        .text_xs()
                        .font_weight(if idx == active_step {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(fg)
                        .child(*label),
                ),
        );
        if idx + 1 < steps.len() {
            row = row.child(div().flex_1().h(px(1.0)).bg(if idx < active_step {
                success.opacity(0.45)
            } else {
                border
            }));
        }
    }
    row.into_any_element()
}

/// 阶段估算进度条。
pub(super) fn progress_bar(progress: f32, cx: &Context<InstallApp>) -> AnyElement {
    let ratio = progress.clamp(0.05, 1.0);
    let primary = cx.theme().primary;
    let track = cx.theme().muted;
    let border = cx.theme().border;

    div()
        .w_full()
        .h(px(6.0))
        .rounded_full()
        .bg(track)
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .child(div().h_full().w(relative(ratio)).rounded_full().bg(primary))
        .into_any_element()
}

/// 可折叠的实时安装日志抽屉。
pub(super) fn log_drawer(
    logs: &[String],
    expanded: bool,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let toggle_label = if expanded {
        "收起安装详情 ▴"
    } else if logs.is_empty() {
        "查看安装详情 ▾"
    } else {
        "查看安装详情（有输出）▾"
    };

    let toggle = Button::new("toggle-install-logs")
        .label(toggle_label)
        .compact()
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| this.toggle_logs(cx)));

    log_view::drawer(
        toggle,
        logs,
        expanded,
        "install-live-logs",
        "正在等待输出…",
        cx.theme(),
    )
}
