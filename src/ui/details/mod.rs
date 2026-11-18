//! 右侧详情页：头部、操作按钮、标签页。
//!
//! 拆成 [`info`]（元数据）与 [`files`]（文件清单）两个子模块，本页只负责
//! 「取数据 + 布局 + 收集动作」。所有数据都按引用放进 [`DetailContext`]，绘制时不再
//! 克隆文件清单和版本列表。

pub mod files;
pub mod info;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window,
    div, img, px,
};

use crate::app::{Action, Catalog, DebDeckApp, DetailTab, IconCache, ListView};
use crate::model::{AppEntry, DesktopEntry, InstalledPackage, MenuEntryState};
use crate::ui::entry_status;
use crate::ui::icons::{DETAIL_ICON_SIZE, draw_letter_icon, load_icon_texture};

/// 一次绘制所需、已经从各处取好的数据。
pub(crate) struct DetailContext<'a> {
    pub(crate) entry: AppEntry,
    pub(crate) installed: Option<&'a InstalledPackage>,
    /// 软件源里该包的全部候选版本（降序，由 `RepositoryData` 保证）。
    pub(crate) available: &'a [crate::model::RepoPackage],
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
        DetailTab::Files => {
            files::show(&context, &app.files, app.files_total, &app.files_input, cx)
        }
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

/// 图标 + 名称 + 状态徽标。头部与下面的操作区之间用一条细线分开。
fn show_header(
    icons: &mut IconCache,
    context: &DetailContext,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let entry = &context.entry;
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let border = theme.border;
    let (status, color) = entry_status(theme, entry);

    let icon = match context
        .desktop
        .and_then(|desktop| load_icon_texture(icons, &desktop.icon))
    {
        Some(texture) => img(texture)
            .w(px(DETAIL_ICON_SIZE))
            .h(px(DETAIL_ICON_SIZE))
            .rounded(px(10.0))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => draw_letter_icon(&entry.display_name, theme).into_any_element(),
    };

    h_flex()
        .items_center()
        .gap(px(12.0))
        .pb(px(12.0))
        .border_b_1()
        .border_color(border)
        .child(icon)
        .child(
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
                ),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(999.0))
                .bg(color.opacity(0.16))
                .text_color(color)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .child(status),
        )
        .into_any_element()
}

/// 操作按钮行。每个「不能点」都要给出原因。
#[allow(clippy::too_many_lines)]
fn show_actions(
    catalog: &Catalog,
    paths: &crate::util::AppPaths,
    context: &DetailContext,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let entry = &context.entry;
    let installed = context.installed.is_some();
    let has_desktop = context.desktop.is_some();
    let downloadable = context
        .available
        .iter()
        .any(|package| package.download_url().is_some());
    let repository_loading = catalog.repository_loading();

    let open_desktop = context.desktop.cloned();
    let open_name = entry.display_name.clone();
    let open_tooltip = if !installed {
        "应用尚未安装"
    } else if !has_desktop {
        "该软件包没有可用的桌面启动项"
    } else {
        "启动应用"
    };
    // 「打开」是这一屏的主操作，用主色按钮；其余动作退到次级样式。
    let open = Button::new("open-app")
        .label("打开")
        .primary()
        .disabled(!(installed && has_desktop))
        .tooltip(open_tooltip)
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(desktop) = open_desktop.clone() {
                this.handle_action(
                    Action::Open {
                        desktop,
                        name: open_name.clone(),
                    },
                    cx,
                );
            }
        }));

    let pkg = entry.pkg.clone();
    let name = entry.display_name.clone();
    let uninstall = Button::new("uninstall-app")
        .label("卸载")
        .secondary()
        .disabled(!installed)
        .tooltip(if installed {
            "卸载应用"
        } else {
            "应用尚未安装"
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.handle_action(
                Action::Uninstall {
                    pkg: pkg.clone(),
                    name: name.clone(),
                },
                cx,
            );
        }));

    let pkg = entry.pkg.clone();
    let name = entry.display_name.clone();
    let held = entry.is_held;
    let hold_label = if held {
        "取消保持版本"
    } else {
        "保持版本"
    };
    let hold = Button::new("hold-app")
        .label(hold_label)
        .secondary()
        .disabled(!installed)
        .tooltip(if installed {
            hold_label
        } else {
            "应用尚未安装"
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.handle_action(
                Action::ToggleHold {
                    pkg: pkg.clone(),
                    name: name.clone(),
                    held,
                },
                cx,
            );
        }));

    let pkg = entry.pkg.clone();
    let name = entry.display_name.clone();
    let download_tooltip = if downloadable {
        "选择版本并下载安装包"
    } else if repository_loading {
        "软件源仍在加载中"
    } else {
        "软件源中没有该软件包的可下载版本"
    };
    let download = Button::new("download-app")
        .label("在线获取安装包")
        .secondary()
        .disabled(!downloadable)
        .tooltip(download_tooltip)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.handle_action(
                Action::Download {
                    pkg: pkg.clone(),
                    name: name.clone(),
                },
                cx,
            );
        }));

    let pkg = entry.pkg.clone();
    let name = entry.display_name.clone();
    let build = Button::new("build-app")
        .label("离线提取安装包")
        .secondary()
        .disabled(!installed)
        .tooltip(if installed {
            "从已安装文件重新打包"
        } else {
            "应用尚未安装"
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.handle_action(
                Action::Build {
                    pkg: pkg.clone(),
                    name: name.clone(),
                },
                cx,
            );
        }));

    let desktop_path = context.desktop.map(|desktop| desktop.path.clone());
    let menu_state = context.desktop.map(|desktop| desktop.menu_state);
    let menu_toggle = match (desktop_path, menu_state) {
        (Some(path), Some(state)) => {
            let (label, tooltip, hidden, enabled) = match state {
                MenuEntryState::Visible => (
                    "在应用菜单中隐藏",
                    "从当前用户的应用菜单隐藏此启动项；同一软件包的其他 .desktop 条目不受影响",
                    true,
                    true,
                ),
                MenuEntryState::Hidden => {
                    ("在应用菜单中显示", "恢复此启动项到应用菜单", false, true)
                }
                MenuEntryState::ManagedByOther => (
                    "菜单项由用户自定义",
                    "已存在用户级 .desktop 覆盖文件，DebDeck 不会修改它",
                    false,
                    false,
                ),
            };
            let pkg = entry.pkg.clone();
            let name = entry.display_name.clone();
            Some(
                Button::new("toggle-menu-visibility")
                    .label(label)
                    .secondary()
                    .disabled(!enabled)
                    .tooltip(tooltip)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.handle_action(
                                Action::SetMenuVisibility {
                                    pkg: pkg.clone(),
                                    name: name.clone(),
                                    path: path.clone(),
                                    hidden,
                                },
                                cx,
                            );
                        }
                    })),
            )
        }
        _ => None,
    };

    let downloads = paths.downloads.clone();
    let border = cx.theme().border;

    h_flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .py(px(10.0))
        .child(open)
        .child(uninstall)
        .child(hold)
        .child(div().w(px(1.0)).h(px(18.0)).bg(border).flex_shrink_0())
        .child(download)
        .child(build)
        .children(menu_toggle)
        .child(
            Button::new("open-downloads-detail")
                .label("下载目录")
                .compact()
                .ghost()
                .tooltip(downloads.display().to_string())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.handle_action(Action::OpenDownloads, cx);
                })),
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
            "信息",
            "detail-tab-info",
            DetailTab::Info,
            active == DetailTab::Info,
            cx,
        ))
        .child(tab_button(
            &files_label,
            "detail-tab-files",
            DetailTab::Files,
            active == DetailTab::Files,
            cx,
        ))
        .into_any_element()
}

fn tab_button(
    label: &str,
    id: &'static str,
    tab: DetailTab,
    selected: bool,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let theme = cx.theme();
    let foreground = theme.foreground;
    let muted = theme.muted_foreground;
    let underline = if selected {
        theme.primary
    } else {
        theme.transparent
    };

    div()
        .id(id)
        .pb(px(8.0))
        .border_b_2()
        .border_color(underline)
        .text_sm()
        .font_weight(if selected {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .text_color(if selected { foreground } else { muted })
        .cursor_pointer()
        .hover(move |this| this.text_color(foreground))
        .child(label.to_string())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.list.detail_tab = tab;
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
