//! 详情页头部：图标、名称与状态徽标。

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, IntoElement, ObjectFit, ParentElement as _, Styled as _,
    StyledImage as _, div, img, px,
};

use crate::app::{DebDeckApp, IconCache};
use crate::model::AppEntry;
use crate::ui::entry_status;
use crate::ui::icons::{DETAIL_ICON_SIZE, draw_letter_icon};

use super::DetailContext;

/// 图标 + 名称 + 状态徽标。头部与下面的操作区之间用一条细线分开。
pub(super) fn show_header(
    icons: &IconCache,
    context: &DetailContext,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let theme = cx.theme();
    let border = theme.border;
    let muted = theme.muted_foreground;
    let (status, color) = entry_status(theme, &context.entry);
    h_flex()
        .items_center()
        .gap(px(12.0))
        .pb(px(12.0))
        .border_b_1()
        .border_color(border)
        .child(detail_icon(icons, context, cx))
        .child(header_identity(&context.entry, muted))
        .child(div().flex_1())
        .child(status_badge(&status, color))
        .into_any_element()
}

fn detail_icon(icons: &IconCache, context: &DetailContext, cx: &Context<DebDeckApp>) -> AnyElement {
    match context
        .desktop
        .and_then(|desktop| icons.get_ready(&desktop.icon))
    {
        Some(texture) => img(texture)
            .w(px(DETAIL_ICON_SIZE))
            .h(px(DETAIL_ICON_SIZE))
            .rounded(px(10.0))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => draw_letter_icon(&context.entry.display_name, cx.theme()).into_any_element(),
    }
}

fn header_identity(entry: &AppEntry, muted: Hsla) -> AnyElement {
    v_flex()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(entry.display_name.clone()),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .truncate()
                .child(entry.pkg.clone()),
        )
        .into_any_element()
}

fn status_badge(status: &str, color: Hsla) -> AnyElement {
    div()
        .flex_shrink_0()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(color.opacity(0.16))
        .text_color(color)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(status.to_string())
        .into_any_element()
}
