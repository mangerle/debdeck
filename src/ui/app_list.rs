//! 应用列表：单列名称 + 状态圆点 + 状态徽标。
//!
//! 一张表有几万行，全部绘制会掉帧；GPUI 的 `uniform_list` 只画视口内的行。
//! 渲染闭包持有 `Arc<Vec<AppEntry>>` 与可见下标的快照，不借出整个 `Catalog`，
//! 也不会在搜索时为每一行复制字符串。版本、大小、更新时间等元数据在右侧详情展示。
//!
//! 行不再做斑马纹：靠悬停与选中两档底色区分就够了，圆角内缩的行看起来也更像
//! 一张张卡片，而不是一张表格。

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::App;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;

use crate::app::{DebDeckApp, Filter, RowStatus};
use crate::model::AppEntry;
use crate::ui::theme::status_color;

const ROW_HEIGHT: f32 = 44.0;

/// 绘制列表；选中通过回调写回 [`DebDeckApp`]。
pub(crate) fn show(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    if app.visible.is_empty() {
        return empty_hint(app, cx).into_any_element();
    }
    v_flex()
        .size_full()
        .child(app_list(app, cx))
        .into_any_element()
}

fn app_list(app: &DebDeckApp, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let state = AppListRows {
        entries: app.entries.clone(),
        visible: app.visible.clone(),
        selected: app.list.selected.clone(),
        entity: cx.entity(),
    };
    uniform_list(
        "app_rows",
        state.visible.len(),
        move |range, _window, cx| render_app_rows(&state, range, cx),
    )
    .flex_1()
    .w_full()
    .px(px(6.0))
    .py(px(6.0))
    .into_any_element()
}

struct AppListRows {
    entries: Arc<Vec<AppEntry>>,
    visible: Arc<Vec<usize>>,
    selected: Option<String>,
    entity: Entity<DebDeckApp>,
}

fn render_app_rows(state: &AppListRows, range: Range<usize>, cx: &mut App) -> Vec<AnyElement> {
    let theme = cx.theme();
    range
        .map(|index| {
            let entry = &state.entries[state.visible[index]];
            app_row(ListRowSpec {
                entry,
                index,
                selected: state.selected.as_deref() == Some(entry.pkg.as_str()),
                entity: state.entity.clone(),
                theme,
            })
        })
        .collect()
}

struct ListRowSpec<'a> {
    entry: &'a AppEntry,
    index: usize,
    selected: bool,
    entity: Entity<DebDeckApp>,
    theme: &'a gpui_kit::component::Theme,
}

fn app_row(spec: ListRowSpec<'_>) -> AnyElement {
    let status = RowStatus::of(spec.entry);
    let badge_color = status_color(spec.theme, status);
    let pkg = spec.entry.pkg.clone();
    let hover = spec.theme.list_hover;
    let active = spec.theme.list_active;
    let mut row = h_flex()
        .id(("app-row", spec.index))
        .h(px(ROW_HEIGHT))
        .w_full()
        .px(px(10.0))
        .items_center()
        .gap(px(10.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(move |this| this.bg(hover))
        .on_click(move |_, window, cx| {
            spec.entity.update(cx, |app, cx| {
                app.select_pkg(Some(pkg.clone()), window, cx);
            });
        });
    if spec.selected {
        row = row.bg(active);
    }
    row.child(status_dot(badge_color))
        .child(name_cell(
            spec.entry,
            spec.theme.foreground,
            spec.theme.muted_foreground,
        ))
        .children(badge(status, badge_color))
        .into_any_element()
}

/// 左侧状态圆点：一眼看出这一行是装好的、可更新的还是还没装的。
fn status_dot(color: Hsla) -> AnyElement {
    div()
        .w(px(7.0))
        .h(px(7.0))
        .rounded(px(999.0))
        .bg(color)
        .flex_shrink_0()
        .into_any_element()
}

/// 右侧徽标。只标出需要留意的两种状态，「已安装 / 未安装」交给圆点表达。
fn badge(status: RowStatus, color: Hsla) -> Option<AnyElement> {
    match status {
        RowStatus::Held | RowStatus::Update => Some(
            div()
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(999.0))
                .bg(color.opacity(0.16))
                .text_color(color)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .child(status.label())
                .into_any_element(),
        ),
        RowStatus::Installed | RowStatus::NotInstalled => None,
    }
}

fn name_cell(entry: &AppEntry, foreground: Hsla, muted: Hsla) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(1.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .truncate()
                .child(entry.display_name.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child(entry.pkg.clone()),
        )
        .into_any_element()
}

/// 空列表时的解释文案 —— 区分「没搜到」与「被筛掉了」，否则用户无从下手。
fn empty_hint(app: &DebDeckApp, cx: &Context<DebDeckApp>) -> AnyElement {
    let hint = if !app.list.search.is_empty() {
        Some("试试切换筛选条件，或清空搜索关键词")
    } else if app.list.filter == Filter::Gui {
        Some("当前没有界面应用，可切换到“全部”查看命令行软件包")
    } else {
        None
    };

    let muted = cx.theme().muted_foreground;
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .child(div().text_lg().text_color(muted).child("没有匹配的应用"))
        .children(hint.map(|hint| {
            div()
                .text_sm()
                .text_color(muted)
                .child(hint)
                .into_any_element()
        }))
        .into_any_element()
}
