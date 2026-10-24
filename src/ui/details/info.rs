//! 详情页「信息」标签：软件包元数据与下载地址。

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::link::Link;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::app::Action;
use crate::model::{PackageMetadata, RepoPackage};
use crate::ui::details::{DetailContext, field_row, field_with_value, shorten_text};
use crate::ui::{installation_summary, value_or_dash};
use crate::util::{format_bytes, format_kib, format_time};

/// 绘制信息页。
#[allow(clippy::too_many_lines)]
pub(crate) fn show(
    context: &DetailContext,
    cx: &mut Context<crate::app::DebDeckApp>,
) -> AnyElement {
    // 已安装时用安装记录，未安装时用软件源最新版；两处都没有就不显示元数据块。
    let Some(info) = display_metadata(context) else {
        return div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("没有可显示的软件包信息。")
            .into_any_element();
    };

    let download = download_source(context);
    let download_url = download.and_then(RepoPackage::download_url);
    let package_size = download
        .map(|package| package.download_size)
        .filter(|size| *size > 0);

    let entry = &context.entry;
    let mut rows = Vec::new();
    rows.push(field_row("包名", &info.name, cx));
    rows.push(field_row("版本", &version_text(context, info), cx));
    rows.push(field_row("架构", &info.arch, cx));
    rows.push(field_row(
        "安装后占用",
        &if entry.installed_size_kb > 0 {
            format_kib(entry.installed_size_kb)
        } else {
            String::new()
        },
        cx,
    ));
    rows.push(field_row(
        "安装包大小",
        &package_size.map(format_bytes).unwrap_or_default(),
        cx,
    ));
    rows.push(field_row("更新时间", &format_time(entry.updated), cx));
    rows.push(field_row("维护者", &info.maintainer, cx));
    rows.push(field_row("依赖", &info.depends, cx));

    rows.push(if info.homepage.is_empty() {
        field_row("主页", "—", cx)
    } else {
        let link = Link::new("detail-homepage")
            .href(info.homepage.clone())
            .text_sm()
            .truncate()
            .child(shorten_text(&info.homepage, 70));
        field_with_value("主页", link.into_any_element(), cx)
    });

    rows.push(match download_url {
        Some(url) => {
            let copy_url = url.clone();
            let mono = cx.theme().mono_font_family.clone();
            let body = v_flex()
                .gap(px(4.0))
                .min_w(px(0.0))
                .child(
                    div()
                        .text_xs()
                        .font_family(mono)
                        .whitespace_normal()
                        .child(url),
                )
                .child(
                    h_flex().child(
                        Button::new("copy-download-url")
                            .label("复制链接")
                            .compact()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.handle_action(Action::Copy(copy_url.clone()), cx);
                            })),
                    ),
                )
                .into_any_element();
            field_with_value("下载地址", body, cx)
        }
        None => field_row("下载地址", "—", cx),
    });

    if let Some(desktop) = context.desktop {
        rows.push(field_row(
            "桌面文件",
            &desktop.path.display().to_string(),
            cx,
        ));
        rows.push(field_row("启动命令", &desktop.exec, cx));
    }
    rows.push(field_row(
        "安装状态",
        installation_summary(&context.entry),
        cx,
    ));

    let content = v_flex()
        .gap(px(10.0))
        .child(card(rows.into_iter().collect(), cx))
        .child(description_card(&info.description, cx))
        .child(div().h(px(8.0)));

    div()
        .id("detail-info-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .child(content)
        .into_any_element()
}

/// 元数据卡片：一块带边框的浅色底板，把成对的标签和取值收在一起。
fn card(rows: Vec<AnyElement>, cx: &Context<crate::app::DebDeckApp>) -> AnyElement {
    let theme = cx.theme();
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .px(px(12.0))
        .py(px(6.0))
        .children(rows)
        .into_any_element()
}

/// 描述单独一张卡片：正文可长可短，和上面的字段表区分开。
fn description_card(description: &str, cx: &Context<crate::app::DebDeckApp>) -> AnyElement {
    let theme = cx.theme();
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .px(px(12.0))
        .py(px(10.0))
        .gap(px(6.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child("描述"),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .whitespace_normal()
                .child(value_or_dash(description).to_string()),
        )
        .into_any_element()
}

/// 展示用的元数据：优先安装记录，其次软件源最新版。
pub(crate) fn display_metadata<'a>(context: &DetailContext<'a>) -> Option<&'a PackageMetadata> {
    context
        .installed
        .map(|package| &package.metadata)
        .or_else(|| context.available.first().map(|package| &package.metadata))
}

/// 用于取「安装包大小 / 下载地址」的那条源记录。
///
/// 已安装时优先取与安装版本完全一致的那条（大小才对得上），否则取最新版。
pub(crate) fn download_source<'a>(context: &DetailContext<'a>) -> Option<&'a RepoPackage> {
    match context.installed {
        Some(installed) => context
            .available
            .iter()
            .find(|package| {
                package.metadata.version == installed.metadata.version
                    && package.metadata.arch == installed.metadata.arch
            })
            .or(context.available.first()),
        None => context.available.first(),
    }
}

/// 版本行：有可更新版本时把最新版一起写出来。
pub(crate) fn version_text(context: &DetailContext, info: &PackageMetadata) -> String {
    let entry = &context.entry;
    match (&entry.latest_version, entry.has_update) {
        (Some(latest), true) if *latest != info.version => {
            format!("{}（最新 {latest}）", info.version)
        }
        _ => info.version.clone(),
    }
}
