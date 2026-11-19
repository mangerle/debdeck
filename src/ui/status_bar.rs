//! 状态栏：统计、软件源状态与快捷入口。

use std::path::{Path, PathBuf};

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
    let summary = StatusSummary::from_catalog(app);
    let warnings = app.catalog.repository_warnings().to_vec();
    let controls = StatusControls {
        warnings,
        downloads: app.paths.downloads.clone(),
        loading: app.catalog.repository_loading(),
        has_packages: app.catalog.has_packages(),
        allowed: refresh_allowed(&app.catalog, &app.dialogs),
    };
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let background = cx.theme().status_bar;
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
        .child(status_summary(summary, muted))
        .child(status_controls(controls, cx))
        .into_any_element()
}

#[derive(Clone, Copy)]
struct StatusSummary {
    total: usize,
    installed: usize,
    gui: usize,
    held: usize,
    upgradable: usize,
    visible: usize,
    degraded: bool,
}

impl StatusSummary {
    fn from_catalog(app: &DebDeckApp) -> Self {
        let counts = app.catalog.counts();
        Self {
            total: counts.total,
            installed: counts.installed,
            gui: counts.gui,
            held: counts.held,
            upgradable: counts.upgradable,
            visible: app.list.visible().len(),
            degraded: app.catalog.upgradable_degraded(),
        }
    }
}

fn status_summary(summary: StatusSummary, muted: Hsla) -> AnyElement {
    h_flex()
        .gap(px(10.0))
        .items_center()
        .min_w_0()
        .child(div().text_xs().text_color(muted).truncate().child(format!(
            "共 {} 个应用 · 已安装 {} · 界面 {} · 保持 {} · 可更新 {} · 显示 {}",
            summary.total,
            summary.installed,
            summary.gui,
            summary.held,
            summary.upgradable,
            summary.visible,
        )))
        .children(summary.degraded.then(|| {
            div()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child("更新状态未确认（apt 查询失败，按版本号比较推断）")
                .into_any_element()
        }))
        .into_any_element()
}

struct StatusControls {
    warnings: Vec<String>,
    downloads: PathBuf,
    loading: bool,
    has_packages: bool,
    allowed: bool,
}

fn status_controls(controls: StatusControls, cx: &mut Context<DebDeckApp>) -> AnyElement {
    h_flex()
        .gap(px(8.0))
        .items_center()
        .child(downloads_button(&controls.downloads, cx))
        .children(warnings_button(controls.warnings, cx))
        .child(repository_status(
            RepositoryState {
                loading: controls.loading,
                has_packages: controls.has_packages,
                allowed: controls.allowed,
            },
            cx,
        ))
        .into_any_element()
}

fn downloads_button(downloads: &Path, cx: &mut Context<DebDeckApp>) -> AnyElement {
    Button::new("open-downloads")
        .label("下载目录")
        .compact()
        .ghost()
        .tooltip(downloads.display().to_string())
        .on_click(cx.listener(|this, _, _, cx| {
            this.handle_action(Action::OpenDownloads, cx);
        }))
        .into_any_element()
}

fn warnings_button(warnings: Vec<String>, cx: &mut Context<DebDeckApp>) -> Option<AnyElement> {
    (!warnings.is_empty()).then(|| warning_button(warnings, cx))
}

fn warning_button(warnings: Vec<String>, cx: &mut Context<DebDeckApp>) -> AnyElement {
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
}

/// 右侧的软件源状态。索引为空时不写「已加载」——那是误导，改成可点击的重试入口。
#[derive(Clone, Copy)]
struct RepositoryState {
    loading: bool,
    has_packages: bool,
    allowed: bool,
}

fn repository_status(state: RepositoryState, cx: &mut Context<DebDeckApp>) -> AnyElement {
    if state.loading {
        repository_loading(cx)
    } else if !state.has_packages {
        repository_unavailable(state.allowed, cx)
    } else {
        repository_loaded(cx)
    }
}

fn repository_loading(cx: &Context<DebDeckApp>) -> AnyElement {
    h_flex()
        .gap(px(6.0))
        .items_center()
        .child(Spinner::new().small())
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("正在加载软件源…"),
        )
        .into_any_element()
}

fn repository_unavailable(allowed: bool, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let label = if allowed {
        "软件源不可用，点此重试"
    } else {
        "软件源不可用"
    };
    Button::new("repository-retry")
        .label(label)
        .compact()
        .ghost()
        .disabled(!allowed)
        .on_click(cx.listener(|this, _, _, cx| {
            this.handle_action(Action::Refresh, cx);
        }))
        .into_any_element()
}

fn repository_loaded(cx: &Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let success = cx.theme().success;
    div()
        .text_xs()
        .text_color(muted)
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(div().w(px(6.0)).h(px(6.0)).rounded(px(999.0)).bg(success))
        .child("软件源已加载")
        .into_any_element()
}
