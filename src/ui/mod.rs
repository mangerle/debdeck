//! 绘制层：读状态、画界面、交回动作，不碰 `backend` 与 `runtime`。
//!
//! 每个模块的函数形式都是 `fn show(app, window, cx) -> AnyElement`（或
//! `Option<AnyElement>`）：状态更新通过 `cx.listener` / `Entity::update` 写回
//! [`DebDeckApp`]，需要产生副作用的写成 [`crate::app::Action`]，由 `app` 层执行。
//!
//! 子模块：[`toolbar`] 顶栏、[`status_bar`] 状态栏、[`app_list`] 列表、
//! [`details`] 详情、[`modals`] 模态、[`notices`] 加载与轻提示、
//! [`icons`] 图标位图、[`theme`] 字体与配色。

pub mod app_list;
pub mod details;
pub mod icons;
pub mod install;
pub mod modals;
pub mod notices;
pub mod status_bar;
pub mod theme;
pub mod toolbar;

use gpui_kit::component::{h_resizable, resizable_panel, v_flex};
use gpui_kit::{Context, Hsla, IntoElement, ParentElement as _, Styled as _, Window, div, px};

pub use icons::window_icon;
pub(crate) use notices::{show_loading, show_toast};

use crate::app::{DebDeckApp, RowStatus};
use crate::model::AppEntry;

/// 绘制主界面（顶栏、状态栏、列表、详情）。
pub(crate) fn show(
    app: &mut DebDeckApp,
    window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> impl IntoElement {
    let toolbar = toolbar::show(app, window, cx);
    let status = status_bar::show(app, window, cx);
    let list = app_list::show(app, window, cx);
    let details = details::show(app, window, cx);

    let main = h_resizable("main_split")
        .child(
            resizable_panel()
                .size(px(400.0))
                .size_range(px(300.0)..px(760.0))
                .child(list),
        )
        .child(resizable_panel().child(details));

    v_flex()
        .size_full()
        .child(toolbar)
        .child(div().flex_1().min_h(px(0.0)).child(main))
        .child(status)
}

/// 列表与详情共用的状态标签：文字 + 前景色。
pub(crate) fn entry_status(theme: &gpui_kit::component::Theme, entry: &AppEntry) -> (String, Hsla) {
    let status = RowStatus::of(entry);
    (
        status.label().to_string(),
        theme::status_color(theme, status),
    )
}

/// 空字符串统一渲染成占位符，界面里不留空白单元格。
pub(crate) fn value_or_dash(value: &str) -> &str {
    if value.is_empty() { "—" } else { value }
}

/// 安装状态的一句话描述，详情页使用。
pub(crate) fn installation_summary(entry: &AppEntry) -> &'static str {
    match (entry.is_installed, entry.is_held) {
        (true, true) => "已安装（保持版本）",
        (true, false) => "已安装",
        (false, _) => "未安装",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(installed: bool, held: bool) -> AppEntry {
        AppEntry {
            pkg: "demo".to_string(),
            display_name: "Demo".to_string(),
            is_installed: installed,
            is_gui: false,
            is_held: installed && held,
            has_update: false,
            installed_size_kb: 0,
            updated: None,
            latest_version: None,
            search_text: String::new(),
            sort_name: "demo".to_string(),
        }
    }

    #[test]
    fn empty_values_fall_back_to_placeholder() {
        assert_eq!(value_or_dash(""), "—");
        assert_eq!(value_or_dash("amd64"), "amd64");
    }

    #[test]
    fn installation_summary_prioritises_hold() {
        assert_eq!(
            installation_summary(&entry(true, true)),
            "已安装（保持版本）"
        );
        assert_eq!(installation_summary(&entry(true, false)), "已安装");
        assert_eq!(installation_summary(&entry(false, false)), "未安装");
    }
}
