//! 详情页「文件」标签：包内文件清单。
//!
//! 一个包可能有上万个文件，`uniform_list` 只绘制视口内的行；过滤结果先在
//! `DebDeckApp::ensure_files` 里生成快照，绘制闭包不接触系统数据。

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::app::DebDeckApp;
use crate::ui::details::DetailContext;

/// 单行高度：等宽小字号正好 18px。
const ROW_HEIGHT: f32 = 18.0;

/// 文件页所需的只读快照。
#[derive(Clone, Copy)]
pub(crate) struct FileView<'a> {
    pub(crate) files: &'a Arc<Vec<SharedString>>,
    pub(crate) total: usize,
    pub(crate) input: &'a Entity<InputState>,
}

/// 绘制文件清单。
pub(crate) fn show(
    context: &DetailContext,
    view: FileView<'_>,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    if context.installed.is_none() {
        return show_not_installed(cx);
    }
    let has_query = !view.input.read(cx).value().is_empty();
    let search_row = search_row(view.input, view.total, has_query, cx);
    let list = if view.files.is_empty() {
        empty_list(cx)
    } else {
        file_list(view.files, cx)
    };
    v_flex()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .gap(px(4.0))
        .child(search_row)
        .child(list)
        .into_any_element()
}

fn search_row(
    input: &Entity<InputState>,
    total: usize,
    has_query: bool,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap(px(8.0))
        .items_center()
        .child(Input::new(input).w(px(260.0)))
        .children(has_query.then(|| clear_filter_button(cx)))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("共 {total} 个文件")),
        )
        .into_any_element()
}

fn clear_filter_button(cx: &mut Context<DebDeckApp>) -> AnyElement {
    Button::new("clear-file-filter")
        .label("清除")
        .compact()
        .ghost()
        .on_click(cx.listener(|this, _, window, cx| {
            this.files_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            this.list.files_query.clear();
            cx.notify();
        }))
        .into_any_element()
}

fn empty_list(cx: &Context<DebDeckApp>) -> AnyElement {
    v_flex()
        .flex_1()
        .min_h(px(0.0))
        .items_center()
        .justify_center()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("没有匹配的文件"),
        )
        .into_any_element()
}

fn file_list(files: &Arc<Vec<SharedString>>, cx: &Context<DebDeckApp>) -> AnyElement {
    let state = FileListRows {
        files: files.clone(),
        mono: cx.theme().mono_font_family.clone(),
        foreground: cx.theme().foreground,
        hover: cx.theme().list_hover,
    };
    uniform_list(
        "detail_files",
        state.files.len(),
        move |range, _window, _cx| render_file_rows(&state, range),
    )
    .flex_1()
    .w_full()
    .into_any_element()
}

struct FileListRows {
    files: Arc<Vec<SharedString>>,
    mono: SharedString,
    foreground: Hsla,
    hover: Hsla,
}

fn render_file_rows(state: &FileListRows, range: Range<usize>) -> Vec<AnyElement> {
    range
        .map(|index| {
            file_row(FileRowSpec {
                index,
                path: &state.files[index],
                mono: state.mono.clone(),
                foreground: state.foreground,
                hover: state.hover,
            })
        })
        .collect()
}

struct FileRowSpec<'a> {
    index: usize,
    path: &'a SharedString,
    mono: SharedString,
    foreground: Hsla,
    hover: Hsla,
}

fn file_row(spec: FileRowSpec<'_>) -> AnyElement {
    div()
        .id(("file-row", spec.index))
        .h(px(ROW_HEIGHT))
        .w_full()
        .px(px(4.0))
        .flex()
        .items_center()
        .font_family(spec.mono)
        .text_xs()
        .text_color(spec.foreground)
        .whitespace_nowrap()
        .truncate()
        .hover(move |this| this.bg(spec.hover))
        .child(spec.path.clone())
        .into_any_element()
}

fn show_not_installed(cx: &Context<DebDeckApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .flex_1()
        .min_h(px(0.0))
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .child(div().text_base().text_color(muted).child("该应用尚未安装"))
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .child("安装后可在这里查看它包含的文件"),
        )
        .into_any_element()
}
