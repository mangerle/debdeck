//! 安装窗口的按钮与通用小部件。

use std::sync::atomic::Ordering;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::*;

use crate::app::{InstallApp, InstallFile};
use crate::runtime::install::InstalledStatus;

/// 确认页的按钮（根据本机已安装状态动态调整主按钮文案）。
pub(super) fn confirm_actions(app: &InstallApp, cx: &mut Context<InstallApp>) -> AnyElement {
    let running_flag = app.running();
    let cancel = Button::new("install-cancel")
        .label("取消")
        .compact()
        .ghost()
        .on_click(cx.listener(move |_, _, _, cx| {
            // 安装进程还在跑时不允许关窗，否则会打断正在执行的 apt/dpkg。
            if !running_flag.load(Ordering::Relaxed) {
                cx.quit();
            }
        }));
    let label = match app.file() {
        InstallFile::Ready { installed, .. } => match installed {
            InstalledStatus::NotInstalled | InstalledStatus::OtherVersion { .. } => "安装",
            InstalledStatus::SameVersion => "重新安装",
            InstalledStatus::Upgrade { .. } => "升级",
            InstalledStatus::Downgrade { .. } => "降级安装",
        },
        _ => "安装",
    };
    let install = Button::new("install-start")
        .label(label)
        .compact()
        .primary()
        .on_click(cx.listener(|this, _, _, cx| this.start(cx)));

    action_bar().child(cancel).child(install).into_any_element()
}

/// 授权页的按钮。
pub(super) fn password_actions(cx: &mut Context<InstallApp>) -> AnyElement {
    let cancel = Button::new("install-cancel-auth")
        .label("取消")
        .compact()
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| this.cancel_auth(cx)));
    let submit = Button::new("install-submit-password")
        .label("确认")
        .compact()
        .primary()
        .on_click(cx.listener(|this, _, _, cx| this.submit_password(cx)));

    action_bar().child(cancel).child(submit).into_any_element()
}

/// 结束后的按钮：成功且存在桌面入口时提供「立即启动」主按钮。
pub(super) fn close_actions(app: &InstallApp, cx: &mut Context<InstallApp>) -> AnyElement {
    if app.launch_entry().is_some() {
        let close = Button::new("install-close")
            .label("关闭")
            .compact()
            .ghost()
            .on_click(cx.listener(|_, _, _, cx| cx.quit()));
        let launch = Button::new("install-launch")
            .label("立即启动")
            .compact()
            .primary()
            .on_click(cx.listener(|this, _, _, cx| this.launch_app(cx)));
        return action_bar().child(close).child(launch).into_any_element();
    }

    let close = Button::new("install-close")
        .label("关闭")
        .compact()
        .primary()
        .on_click(cx.listener(|_, _, _, cx| cx.quit()));
    action_bar().child(close).into_any_element()
}

/// 底部操作栏：右对齐的按钮行。
fn action_bar() -> Div {
    h_flex().w_full().justify_end().items_center().gap(px(8.0))
}

/// 彩色小提示条。
pub(super) fn notice(icon: IconName, color: Hsla, text: &str) -> AnyElement {
    h_flex()
        .w_full()
        .items_start()
        .gap(px(8.0))
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(color.opacity(0.08))
        .border_1()
        .border_color(color.opacity(0.28))
        .child(Icon::new(icon).small().text_color(color))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_xs()
                .text_color(color)
                .whitespace_normal()
                .child(text.to_string()),
        )
        .into_any_element()
}

/// 安装需要管理员授权的轻提示，不抢视觉焦点。
pub(super) fn auth_hint(cx: &Context<InstallApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .items_center()
        .gap(px(6.0))
        .child(Icon::new(IconName::LockKeyhole).xsmall().text_color(muted))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .whitespace_normal()
                .child("安装需要管理员授权，密码会在本窗口内输入。"),
        )
        .into_any_element()
}

/// 圆形/圆角图标底座。`round` 为 true 时画成圆形（用于状态结果）。
pub(super) fn icon_tile(icon: IconName, color: Hsla, size: f32, round: bool) -> AnyElement {
    let radius = if round { size } else { size * 0.28 };
    div()
        .size(px(size))
        .rounded(px(radius))
        .bg(color.opacity(0.12))
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).with_size(px(size * 0.5)).text_color(color))
        .into_any_element()
}

/// 小圆角标签：版本、架构、大小等。
pub(super) fn chip(text: String, fg: Hsla, bg: Hsla, border: Option<Hsla>) -> AnyElement {
    let mut chip = h_flex()
        .px(px(8.0))
        .py(px(2.0))
        .rounded_full()
        .bg(bg)
        .child(div().text_xs().text_color(fg).child(text));
    if let Some(border) = border {
        chip = chip.border_1().border_color(border);
    }
    chip.into_any_element()
}
