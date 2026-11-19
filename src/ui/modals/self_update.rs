//! DebDeck 自身更新弹窗：版本摘要、更新日志、进度与单一主动作。

use gpui_kit::Hsla;
use gpui_kit::base::TextView;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::app::{DebDeckApp, SelfUpdatePrimaryAction, SelfUpdateSnapshot};

use super::{actions, body, overlay, title};

/// 更新日志可视高度上限；更长内容在区域内部滚动。
const MAX_NOTES_HEIGHT: f32 = 280.0;

/// 绘制更新弹窗。
pub(crate) fn show(app: &mut DebDeckApp, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let snapshot = app.self_update.snapshot();
    let close = Button::new("close-self-update")
        .label("关闭")
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| {
            this.dialogs.modal = None;
            cx.notify();
        }));
    let primary = primary_button(&snapshot, cx);
    let card = v_flex()
        .gap(px(12.0))
        .child(title("DebDeck 软件更新"))
        .child(version_row(&snapshot, cx))
        .child(status_text(&snapshot, cx))
        .children(snapshot.detail.as_ref().map(|detail| body(detail.clone())))
        .children(
            snapshot
                .published
                .as_ref()
                .map(|date| muted_text(format!("发布日期：{date}"), cx)),
        )
        .children(progress(&snapshot))
        .child(release_notes(&snapshot, cx))
        .child(actions().child(close).child(primary))
        .into_any_element();
    overlay(card, true, cx)
}

/// 当前版本与目标版本并列展示。
fn version_row(snapshot: &SelfUpdateSnapshot, cx: &Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let mut row = h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(version_meta(
            "当前版本",
            format!("v{}", snapshot.current_version),
            muted,
        ));
    if let Some(version) = snapshot.target_version.as_ref() {
        row = row.child(version_meta("最新版本", format!("v{version}"), muted));
    }
    row.into_any_element()
}

fn version_meta(label: &str, value: String, muted: Hsla) -> AnyElement {
    v_flex()
        .gap(px(2.0))
        .child(div().text_xs().text_color(muted).child(label.to_string()))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
        .into_any_element()
}

/// 状态行：失败时使用危险色。
fn status_text(snapshot: &SelfUpdateSnapshot, cx: &Context<DebDeckApp>) -> AnyElement {
    let color = if snapshot.failed {
        cx.theme().danger
    } else {
        cx.theme().foreground
    };
    div()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(snapshot.status.clone())
        .into_any_element()
}

/// 进度条：未知阶段使用不确定进度，已知百分比使用确定进度。
fn progress(snapshot: &SelfUpdateSnapshot) -> Option<AnyElement> {
    if snapshot.indeterminate {
        Some(
            Progress::new("self-update-progress")
                .loading(true)
                .w_full()
                .into_any_element(),
        )
    } else {
        snapshot.progress.map(|percent| {
            Progress::new("self-update-progress")
                .value(percent)
                .w_full()
                .into_any_element()
        })
    }
}

/// 更新日志区：限制高度并允许滚动，避免超长远端正文撑破弹窗。
fn release_notes(snapshot: &SelfUpdateSnapshot, cx: &Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let content = snapshot.notes.as_ref().map_or_else(
        || {
            div()
                .text_xs()
                .text_color(muted)
                .child("本次发布未提供更新日志。")
                .into_any_element()
        },
        |notes| {
            div()
                .id("self-update-notes")
                .max_h(px(MAX_NOTES_HEIGHT))
                .overflow_y_scroll()
                .child(TextView::markdown("self-update-notes-text", notes.clone()).selectable(true))
                .into_any_element()
        },
    );

    v_flex()
        .gap(px(6.0))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child("更新日志"),
        )
        .child(content)
        .into_any_element()
}

/// 主按钮随状态变化；在途阶段禁用，避免重复触发检查、下载或安装。
fn primary_button(snapshot: &SelfUpdateSnapshot, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let (label, primary) = match snapshot.primary {
        SelfUpdatePrimaryAction::Check => ("检查更新", false),
        SelfUpdatePrimaryAction::Download => ("下载更新", true),
        SelfUpdatePrimaryAction::Install => ("立即安装", true),
        SelfUpdatePrimaryAction::Restart => ("立即重启", true),
        SelfUpdatePrimaryAction::Busy => ("处理中…", true),
    };
    let mut button = Button::new("self-update-primary")
        .label(label)
        .on_click(cx.listener(|this, _, _, cx| {
            this.perform_self_update_action(cx);
        }));
    button = if primary {
        button.primary()
    } else {
        button.secondary()
    };
    button
        .disabled(snapshot.primary == SelfUpdatePrimaryAction::Busy)
        .into_any_element()
}

fn muted_text(text: String, cx: &Context<DebDeckApp>) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}
