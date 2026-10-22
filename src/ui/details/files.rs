//! 详情页「文件」标签：包内文件清单。
//!
//! 一个包可能有上万个文件，`uniform_list` 只绘制视口内的行；过滤结果先在
//! `DebDeckApp::ensure_files` 里生成快照，绘制闭包不接触系统数据。

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

/// 绘制文件清单。
pub(crate) fn show(
    context: &DetailContext,
    files: &Arc<Vec<SharedString>>,
    total: usize,
    input: &Entity<InputState>,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let Some(_package) = context.installed else {
        return show_not_installed(cx);
    };

    let has_query = !input.read(cx).value().is_empty();
    let count = div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(format!("共 {total} 个文件"))
        .into_any_element();

    let search_row = h_flex()
        .gap(px(8.0))
        .items_center()
        .child(Input::new(input).w(px(260.0)))
        .children(has_query.then(|| {
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
        }))
        .child(count);

    let list = if files.is_empty() {
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
    } else {
        let mono = cx.theme().mono_font_family.clone();
        let foreground = cx.theme().foreground;
        let row_hover = cx.theme().list_hover;
        let rows = files.clone();
        uniform_list("detail_files", rows.len(), move |range, _window, _cx| {
            range
                .map(|index| {
                    let mono = mono.clone();
                    let path = rows[index].clone();
                    div()
                        .id(("file-row", index))
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .px(px(4.0))
                        .flex()
                        .items_center()
                        .font_family(mono)
                        .text_xs()
                        .text_color(foreground)
                        .whitespace_nowrap()
                        .truncate()
                        .hover(move |this| this.bg(row_hover))
                        .child(path)
                        .into_any_element()
                })
                .collect()
        })
        .flex_1()
        .w_full()
        .into_any_element()
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
