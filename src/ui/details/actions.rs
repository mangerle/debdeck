//! 详情页操作按钮。

use std::path::Path;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::h_flex;
use gpui_kit::component::{ActiveTheme as _, Disableable as _};
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement as _, Styled as _, div, px};

use crate::app::{Action, Catalog, DebDeckApp};
use crate::model::{DesktopEntry, MenuEntryState};
use crate::util::AppPaths;

use super::DetailContext;

/// 当前详情页可执行动作的前置条件。
///
/// 这些布尔值描述四个彼此独立的能力，而非同一状态机的互斥分支；
/// 分别对应安装状态、桌面入口、下载来源和软件源加载状态。
#[allow(
    clippy::struct_excessive_bools,
    reason = "四个布尔值分别描述独立能力，组合成单一状态机会制造不自然的枚举"
)]
#[derive(Clone, Copy)]
struct ActionAvailability {
    installed: bool,
    has_desktop: bool,
    downloadable: bool,
    repository_loading: bool,
}

/// 操作按钮行。每个「不能点」都要给出原因。
pub(super) fn show_actions(
    catalog: &Catalog,
    paths: &AppPaths,
    context: &DetailContext,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let availability = ActionAvailability {
        installed: context.installed.is_some(),
        has_desktop: context.desktop.is_some(),
        downloadable: context
            .available
            .iter()
            .any(|package| package.download_url().is_some()),
        repository_loading: catalog.repository_loading(),
    };
    let border = cx.theme().border;
    h_flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .py(px(10.0))
        .child(open_button(context, availability, cx))
        .children(upgrade_button(context, cx))
        .child(uninstall_button(context, availability, cx))
        .child(hold_button(context, availability, cx))
        .child(div().w(px(1.0)).h(px(18.0)).bg(border).flex_shrink_0())
        .child(download_button(context, availability, cx))
        .child(build_button(context, availability, cx))
        .children(menu_button(context, cx))
        .child(downloads_button(paths, cx))
        .into_any_element()
}

fn open_button(
    context: &DetailContext,
    availability: ActionAvailability,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let desktop = context.desktop.cloned();
    let name = context.entry.display_name.clone();
    let tooltip = if !availability.installed {
        "应用尚未安装"
    } else if !availability.has_desktop {
        "该软件包没有可用的桌面启动项"
    } else {
        "启动应用"
    };
    Button::new("open-app")
        .label("打开")
        .primary()
        .disabled(!(availability.installed && availability.has_desktop))
        .tooltip(tooltip)
        .on_click(cx.listener(move |this, _, _, cx| {
            open_application(this, desktop.clone(), name.clone(), cx);
        }))
        .into_any_element()
}

fn open_application(
    app: &mut DebDeckApp,
    desktop: Option<DesktopEntry>,
    name: String,
    cx: &mut Context<DebDeckApp>,
) {
    if let Some(desktop) = desktop {
        app.handle_action(Action::Open { desktop, name }, cx);
    }
}

#[derive(Clone, Copy)]
struct MenuToggle<'a> {
    pkg: &'a str,
    name: &'a str,
    path: &'a Path,
    hidden: bool,
    enabled: bool,
}

fn toggle_menu_visibility(
    app: &mut DebDeckApp,
    toggle: MenuToggle<'_>,
    cx: &mut Context<DebDeckApp>,
) {
    if toggle.enabled {
        app.handle_action(
            Action::SetMenuVisibility {
                pkg: toggle.pkg.to_string(),
                name: toggle.name.to_string(),
                path: toggle.path.to_path_buf(),
                hidden: toggle.hidden,
            },
            cx,
        );
    }
}

fn upgrade_button(context: &DetailContext, cx: &mut Context<DebDeckApp>) -> Option<AnyElement> {
    if !context.entry.has_update {
        return None;
    }
    let pkg = context.entry.pkg.clone();
    let name = context.entry.display_name.clone();
    let held = context.entry.is_held;
    let tooltip = if held {
        "已保持版本，请先取消保持后再更新"
    } else {
        "通过 apt-get 更新此应用到最新版本"
    };
    Some(
        Button::new("upgrade-app")
            .label("更新")
            .primary()
            .disabled(held)
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.handle_action(
                    Action::UpgradePackage {
                        pkg: pkg.clone(),
                        name: name.clone(),
                    },
                    cx,
                );
            }))
            .into_any_element(),
    )
}

fn uninstall_button(
    context: &DetailContext,
    availability: ActionAvailability,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let pkg = context.entry.pkg.clone();
    let name = context.entry.display_name.clone();
    Button::new("uninstall-app")
        .label("卸载")
        .secondary()
        .disabled(!availability.installed)
        .tooltip(if availability.installed {
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
        }))
        .into_any_element()
}

fn hold_button(
    context: &DetailContext,
    availability: ActionAvailability,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let pkg = context.entry.pkg.clone();
    let name = context.entry.display_name.clone();
    let held = context.entry.is_held;
    let label = if held {
        "取消保持版本"
    } else {
        "保持版本"
    };
    Button::new("hold-app")
        .label(label)
        .secondary()
        .disabled(!availability.installed)
        .tooltip(if availability.installed {
            label
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
        }))
        .into_any_element()
}

fn download_button(
    context: &DetailContext,
    availability: ActionAvailability,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let pkg = context.entry.pkg.clone();
    let name = context.entry.display_name.clone();
    let tooltip = if availability.downloadable {
        "选择版本并下载安装包"
    } else if availability.repository_loading {
        "软件源仍在加载中"
    } else {
        "软件源中没有该软件包的可下载版本"
    };
    Button::new("download-app")
        .label("在线获取安装包")
        .secondary()
        .disabled(!availability.downloadable)
        .tooltip(tooltip)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.handle_action(
                Action::Download {
                    pkg: pkg.clone(),
                    name: name.clone(),
                },
                cx,
            );
        }))
        .into_any_element()
}

fn build_button(
    context: &DetailContext,
    availability: ActionAvailability,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let pkg = context.entry.pkg.clone();
    let name = context.entry.display_name.clone();
    Button::new("build-app")
        .label("离线提取安装包")
        .secondary()
        .disabled(!availability.installed)
        .tooltip(if availability.installed {
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
        }))
        .into_any_element()
}

fn menu_button(context: &DetailContext, cx: &mut Context<DebDeckApp>) -> Option<AnyElement> {
    let desktop = context.desktop?;
    let (label, tooltip, hidden, enabled) = menu_action(desktop.menu_state);
    let pkg = context.entry.pkg.clone();
    let name = context.entry.display_name.clone();
    let path = desktop.path.clone();
    Some(
        Button::new("toggle-menu-visibility")
            .label(label)
            .secondary()
            .disabled(!enabled)
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle_menu_visibility(
                    this,
                    MenuToggle {
                        pkg: &pkg,
                        name: &name,
                        path: &path,
                        hidden,
                        enabled,
                    },
                    cx,
                );
            }))
            .into_any_element(),
    )
}

fn menu_action(state: MenuEntryState) -> (&'static str, &'static str, bool, bool) {
    match state {
        MenuEntryState::Visible => (
            "在应用菜单中隐藏",
            "从当前用户的应用菜单隐藏此启动项；同一软件包的其他 .desktop 条目不受影响",
            true,
            true,
        ),
        MenuEntryState::Hidden => ("在应用菜单中显示", "恢复此启动项到应用菜单", false, true),
        MenuEntryState::ManagedByOther => (
            "菜单项由用户自定义",
            "已存在用户级 .desktop 覆盖文件，DebDeck 不会修改它",
            false,
            false,
        ),
    }
}

fn downloads_button(paths: &AppPaths, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let downloads = paths.downloads.clone();
    Button::new("open-downloads-detail")
        .label("下载目录")
        .compact()
        .ghost()
        .tooltip(downloads.display().to_string())
        .on_click(cx.listener(|this, _, _, cx| {
            this.handle_action(Action::OpenDownloads, cx);
        }))
        .into_any_element()
}
