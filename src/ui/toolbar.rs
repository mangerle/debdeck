//! 顶栏：标题、搜索、筛选、刷新。

use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::component::{Disableable as _, Selectable as _};
use gpui_kit::*;

use crate::app::{Action, DebDeckApp, Filter, refresh_allowed};

/// 绘制顶栏。
///
/// `refresh_allowed` 由调用方按 [`refresh_allowed`] 算出，
/// 于是「按钮禁用」与「按了也不执行」用的是同一个判断。
pub(crate) fn show(
    app: &mut DebDeckApp,
    _window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let border = cx.theme().border;
    let background = cx.theme().background;

    let allowed = refresh_allowed(&app.catalog, &app.dialogs);
    let filter = app.list.filter;

    // 搜索时也保留分类按钮：搜索在当前所选分类里进行，切分类即切换搜索范围。
    let filter_control = h_flex()
        .gap(px(2.0))
        .children(
            Filter::ALL
                .iter()
                .copied()
                .map(|value| filter_button(value, filter, cx).into_any_element()),
        )
        .into_any_element();

    // 筛选区固定宽度：窗口变窄时也不收缩，保持四个按钮排成一行。
    let filter_slot = div().w(px(356.0)).flex_shrink_0().child(filter_control);
    // Select 内部是 size_full()，必须由外层容器约束宽度；直接给它 .w()
    // 会被内部的 size_full 吃掉，表现为排序框扩张、刷新被挤到右侧。
    let sort_control = div()
        .w(px(128.0))
        .flex_shrink_0()
        .child(Select::new(&app.sort_select).menu_width(px(160.0)));

    let search_and_actions = h_flex()
        .gap(px(8.0))
        .items_center()
        .child(Input::new(&app.search_input).w(px(380.0)).cleanable(true))
        .child(sort_control)
        .child(
            Button::new("refresh")
                .label("刷新")
                .compact()
                .disabled(!allowed)
                .tooltip("重新读取已安装应用和 APT 软件源（F5）")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.handle_action(Action::Refresh, cx);
                })),
        );

    h_flex()
        .h(px(56.0))
        .flex_shrink_0()
        .items_center()
        .gap(px(8.0))
        .px(px(14.0))
        .border_b_1()
        .border_color(border)
        .bg(background)
        .child(search_and_actions)
        .child(div().flex_1())
        .child(filter_slot)
        .into_any_element()
}

fn filter_button(value: Filter, current: Filter, cx: &mut Context<DebDeckApp>) -> Button {
    Button::new(value.label())
        .label(value.label())
        .compact()
        .selected(value == current)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.list.filter = value;
            cx.notify();
        }))
}
