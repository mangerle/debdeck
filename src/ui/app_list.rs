//! 应用列表：单列名称 + 状态徽标。
//!
//! 一张表有几万行，全部绘制会掉帧；GPUI 的 `uniform_list` 只画视口内的行。
//! 渲染闭包持有 `Arc<Vec<AppEntry>>` 与可见下标的快照，不借出整个 `Catalog`，
//! 也不会在搜索时为每一行复制字符串。版本、大小、更新时间等元数据在右侧详情展示。

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;

use crate::app::{DebDeckApp, Filter, RowStatus};
use crate::model::AppEntry;
use crate::ui::theme::status_color;

const ROW_HEIGHT: f32 = 46.0;

/// 绘制列表；选中通过回调写回 [`DebDeckApp`]。
pub(crate) fn show(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    if app.visible.is_empty() {
        return empty_hint(app, cx).into_any_element();
    }

    let entries = app.entries.clone();
    let visible = app.visible.clone();
    let selected = app.list.selected.clone();
    let this = cx.entity();

    let list = uniform_list("app_rows", visible.len(), move |range, _window, cx| {
        let theme = cx.theme();
        let row_border = theme.table_row_border;
        let row_even = theme.table_even;
        let row_hover = theme.table_hover;
        let row_active = theme.table_active;
        let row_active_border = theme.table_active_border;
        let muted = theme.muted_foreground;
        let foreground = theme.foreground;

        range
            .map(|index| {
                let entry = &entries[visible[index]];
                let status = RowStatus::of(entry);
                let badge_color = status_color(theme, status);
                let is_selected = selected.as_deref() == Some(entry.pkg.as_str());
                let entity = this.clone();
                let pkg = entry.pkg.clone();

                let mut element = h_flex()
                    .id(("app-row", index))
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .px(px(10.0))
                    .items_center()
                    .border_b_1()
                    .border_color(row_border)
                    .cursor_pointer()
                    .hover(move |this| this.bg(row_hover))
                    .on_click(move |_, window, cx| {
                        entity.update(cx, |app, cx| {
                            app.select_pkg(Some(pkg.clone()), window, cx);
                        });
                    });

                if is_selected {
                    element = element
                        .bg(row_active)
                        .border_l_2()
                        .border_color(row_active_border);
                } else if index % 2 == 1 {
                    element = element.bg(row_even);
                }

                element
                    .child(name_cell(entry, foreground, muted))
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(999.0))
                            .bg(badge_color.opacity(0.14))
                            .text_color(badge_color)
                            .text_xs()
                            .child(status.label()),
                    )
                    .into_any_element()
            })
            .collect()
    });

    v_flex()
        .size_full()
        .child(list.flex_1().w_full())
        .into_any_element()
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
        Some("当前没有界面应用，可切换到“全部应用”查看命令行软件包")
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
