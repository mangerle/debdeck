//! 顶栏：搜索、分类筛选、排序与刷新。
//!
//! 横向空间是稀缺资源：窗口收窄时搜索框先让步，分类、排序与刷新始终完整可见。
//! 分类切换做成一段分段控件，选中项是一颗品牌橙药丸，而不是四个散落的按钮。

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, h_flex};
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
    let allowed = refresh_allowed(&app.catalog, &app.dialogs);
    h_flex()
        .h(px(52.0))
        .flex_shrink_0()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .child(search_control(app))
        .child(filter_group(app.list.filter, cx))
        // 余下的横向空间留在分类与排序之间，两组控件不挤在一起。
        .child(div().flex_grow(1.0))
        .child(sort_control(app))
        .child(refresh_button(allowed, cx))
        .into_any_element()
}

/// 搜索框拿走大部分剩余空间，但留一个上限，免得宽窗口下拉成一条长条。
fn search_control(app: &DebDeckApp) -> AnyElement {
    div()
        .flex_grow(3.0)
        .flex_basis(px(200.0))
        .min_w(px(160.0))
        .max_w(px(460.0))
        .child(Input::new(&app.search_input).cleanable(true))
        .into_any_element()
}

fn sort_control(app: &DebDeckApp) -> AnyElement {
    // Select 内部是 size_full()，必须由外层容器约束宽度；直接给它 .w()
    // 会被内部的 size_full 吃掉，表现为排序框扩张、刷新被挤到右侧。
    div()
        .w(px(112.0))
        .flex_shrink_0()
        .child(Select::new(&app.sort_select).menu_width(px(160.0)))
        .into_any_element()
}

fn refresh_button(allowed: bool, cx: &mut Context<DebDeckApp>) -> Button {
    Button::new("refresh")
        .label("刷新")
        .compact()
        .disabled(!allowed)
        .tooltip("重新读取已安装应用和 APT 软件源（F5）")
        .on_click(cx.listener(|this, _, _, cx| {
            this.handle_action(Action::Refresh, cx);
        }))
}

/// 分类切换：分段控件。
///
/// 搜索在当前所选分类里进行，切分类即切换搜索范围，因此分类按钮始终可见。
fn filter_group(current: Filter, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .gap(px(2.0))
        .p(px(3.0))
        .rounded(px(8.0))
        .bg(theme.muted)
        .flex_shrink_0()
        .children(
            Filter::ALL
                .iter()
                .copied()
                .map(|value| filter_button(value, current, cx).into_any_element()),
        )
        .into_any_element()
}

fn filter_button(value: Filter, current: Filter, cx: &mut Context<DebDeckApp>) -> Button {
    let theme = cx.theme();
    let active = value == current;

    // 选中项是一颗品牌橙药丸：明暗两种主题下都一眼可辨。
    // 未选中项只在悬停时才显形，底色用半透明前景色，明暗主题都成立。
    let variant = if active {
        ButtonCustomVariant::new(cx)
            .color(theme.primary)
            .foreground(theme.primary_foreground)
            .hover(theme.primary_hover)
            // `ButtonCustomVariant::color` 在普通态会与透明色混合；选中态走
            // `selected()`/`active`，必须直接给品牌橙，否则会透出底下的灰色轨道。
            .active(theme.primary)
    } else {
        ButtonCustomVariant::new(cx)
            .color(theme.transparent)
            .foreground(theme.muted_foreground)
            .hover(theme.muted_foreground.opacity(0.16))
            .active(theme.muted_foreground.opacity(0.28))
    };

    Button::new(format!("filter-{}", value.label()))
        .label(value.label())
        .compact()
        .custom(variant)
        .selected(active)
        .tooltip(value.hint())
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_filter(value, window, cx);
        }))
}
