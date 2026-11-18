//! 状态栏：统计、软件源状态与快捷入口。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::h_flex;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::*;

use crate::app::{Action, DebDeckApp, refresh_allowed};

/// 绘制状态栏。
pub(crate) fn show(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let counts = app.catalog.counts();
    let visible = app.list.visible().len();
    let warnings = app.catalog.repository_warnings().to_vec();
    let degraded = app.catalog.upgradable_degraded();
    let loading = app.catalog.repository_loading();
    let has_packages = app.catalog.has_packages();
    let allowed = refresh_allowed(&app.catalog, &app.dialogs);
    let downloads = app.paths.downloads.clone();

    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let background = cx.theme().status_bar;

    let stats = div().text_xs().text_color(muted).truncate().child(format!(
        "共 {} 个应用 · 已安装 {} · 界面 {} · 保持 {} · 可更新 {} · 显示 {visible}",
        counts.total, counts.installed, counts.gui, counts.held, counts.upgradable,
    ));

    let right = h_flex()
        .gap(px(8.0))
        .items_center()
        .child(
            Button::new("open-downloads")
                .label("下载目录")
                .compact()
                .ghost()
                .tooltip(downloads.display().to_string())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.handle_action(Action::OpenDownloads, cx);
                })),
        )
        .children((!warnings.is_empty()).then(|| {
            Button::new("repository-warnings")
                .label(format!("软件源警告 {}", warnings.len()))
                .compact()
                .ghost()
                .tooltip("点击查看明细")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.dialogs.set_message("软件源警告", warnings.join("\n"));
                    cx.notify();
                }))
                .into_any_element()
        }))
        .child(repository_status(loading, has_packages, allowed, cx));

    h_flex()
        .h(px(32.0))
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .px(px(12.0))
        .gap(px(12.0))
        .border_t_1()
        .border_color(border)
        .bg(background)
        .child(
            h_flex()
                .gap(px(10.0))
                .items_center()
                .min_w_0()
                .child(stats)
                .children(degraded.then(|| {
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child("更新状态未确认（apt 查询失败，按版本号比较推断）")
                        .into_any_element()
                })),
        )
        .child(right)
        .into_any_element()
}

/// 右侧的软件源状态。索引为空时不写「已加载」——那是误导，改成可点击的重试入口。
fn repository_status(
    loading: bool,
    has_packages: bool,
    allowed: bool,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    if loading {
        return h_flex()
            .gap(px(6.0))
            .items_center()
            .child(Spinner::new().small())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("正在加载软件源…"),
            )
            .into_any_element();
    }

    if !has_packages {
        let label = if allowed {
            "软件源不可用，点此重试"
        } else {
            "软件源不可用"
        };
        return Button::new("repository-retry")
            .label(label)
            .compact()
            .ghost()
            .disabled(!allowed)
            .on_click(cx.listener(|this, _, _, cx| {
                this.handle_action(Action::Refresh, cx);
            }))
            .into_any_element();
    }

    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .w(px(6.0))
                .h(px(6.0))
                .rounded(px(999.0))
                .bg(cx.theme().success),
        )
        .child("软件源已加载")
        .into_any_element()
}
