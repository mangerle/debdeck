//! 右侧详情页：头部、操作按钮、标签页。
//!
//! 拆成 [`info`]（元数据）与 [`files`]（文件清单）两个子模块，本页只负责
//! 「取数据 + 布局 + 收集动作」。所有数据都按引用放进 [`DetailContext`]，绘制时不再
//! 克隆文件清单和版本列表。

mod actions;
pub mod files;
mod header;
pub mod info;

use actions::show_actions;
use header::show_header;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::app::{DebDeckApp, DetailTab, ListView};
use crate::model::{AppEntry, DesktopEntry, InstalledPackage, RepoPackage};

/// 一次绘制所需、已经从各处取好的数据。
pub(crate) struct DetailContext<'a> {
    pub(crate) entry: AppEntry,
    pub(crate) installed: Option<&'a InstalledPackage>,
    /// 软件源里该包的全部候选版本（降序，由 `RepositoryData` 保证）。
    pub(crate) available: &'a [RepoPackage],
    pub(crate) desktop: Option<&'a DesktopEntry>,
}

/// 绘制详情页。
pub(crate) fn show(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let Some(entry) = app.list.selected_entry(&app.catalog).cloned() else {
        return placeholder(cx);
    };

    let installed = app.catalog.installed_package(&entry.pkg);
    let available = app.catalog.versions(&entry.pkg);
    let desktop = installed.and_then(|package| package.desktop.as_ref());
    let context = DetailContext {
        entry,
        installed,
        available,
        desktop,
    };

    let header = show_header(&mut app.icons, &context, cx);
    let actions = show_actions(&app.catalog, &app.paths, &context, cx);
    let tabs = show_tabs(&app.list, &context, cx);
    let body = match app.list.detail_tab {
        DetailTab::Info => info::show(&context, cx),
        DetailTab::Files => files::show(
            &context,
            files::FileView {
                files: &app.files,
                total: app.files_total,
                input: &app.files_input,
            },
            cx,
        ),
    };

    v_flex()
        .size_full()
        .min_w(px(0.0))
        .px(px(14.0))
        .py(px(8.0))
        .child(header)
        .child(actions)
        .child(tabs)
        .child(v_flex().flex_1().min_h(px(0.0)).pt(px(10.0)).child(body))
        .into_any_element()
}

fn placeholder(cx: &mut Context<DebDeckApp>) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_lg()
                .text_color(cx.theme().muted_foreground)
                .child("请选择一个应用查看详情"),
        )
        .into_any_element()
}

/// 标签页切换条。激活项用一段品牌色下划线标出。
fn show_tabs(list: &ListView, context: &DetailContext, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let border = cx.theme().border;
    let files_label = format!(
        "文件 ({})",
        context.installed.map_or(0, |package| package.files.len())
    );
    let active = list.detail_tab;

    h_flex()
        .gap(px(18.0))
        .border_b_1()
        .border_color(border)
        .child(tab_button(
            TabSpec {
                label: "信息",
                id: "detail-tab-info",
                tab: DetailTab::Info,
                selected: active == DetailTab::Info,
            },
            cx,
        ))
        .child(tab_button(
            TabSpec {
                label: &files_label,
                id: "detail-tab-files",
                tab: DetailTab::Files,
                selected: active == DetailTab::Files,
            },
            cx,
        ))
        .into_any_element()
}

#[derive(Clone, Copy)]
struct TabSpec<'a> {
    label: &'a str,
    id: &'static str,
    tab: DetailTab,
    selected: bool,
}

fn tab_button(spec: TabSpec<'_>, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let theme = cx.theme();
    let foreground = theme.foreground;
    let muted = theme.muted_foreground;
    let underline = if spec.selected {
        theme.primary
    } else {
        theme.transparent
    };

    div()
        .id(spec.id)
        .pb(px(8.0))
        .border_b_2()
        .border_color(underline)
        .text_sm()
        .font_weight(if spec.selected {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .text_color(if spec.selected { foreground } else { muted })
        .cursor_pointer()
        .hover(move |this| this.text_color(foreground))
        .child(spec.label.to_string())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.list.detail_tab = spec.tab;
            cx.notify();
        }))
        .into_any_element()
}

/// 左侧标签 + 右侧取值的一行。
pub(crate) fn field_row(label: &str, value: &str, cx: &Context<DebDeckApp>) -> AnyElement {
    field_with_value(
        label,
        div()
            .flex_1()
            .min_w(px(0.0))
            .text_sm()
            .whitespace_normal()
            .child(value.to_string())
            .into_any_element(),
        cx,
    )
}

/// 与 [`field_row`] 同布局，但允许调用方自己画右侧的内容（链接、按钮等）。
pub(crate) fn field_with_value(
    label: &str,
    child: AnyElement,
    cx: &Context<DebDeckApp>,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    h_flex()
        .items_start()
        .gap(px(10.0))
        .py(px(3.0))
        .child(
            div()
                .w(px(76.0))
                .flex_shrink_0()
                .text_sm()
                .text_color(muted)
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w(px(0.0)).child(child))
        .into_any_element()
}

/// 按字符数截断，超出用省略号；避免长 URL 撑破布局。
pub(crate) fn shorten_text(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(max_chars).collect();
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_long_text_by_chars() {
        assert_eq!(shorten_text("abc", 5), "abc");
        assert_eq!(shorten_text("中文测试用的字符串", 4), "中文测试…");
    }
}
