//! 右侧详情页：头部、操作按钮、标签页。
//!
//! 拆成 [`info`]（元数据）与 [`files`]（文件清单）两个子模块，本页只负责
//! 「取数据 + 布局 + 收集动作」。所有数据都按引用放进 [`Context`]，每帧不再克隆
//! 文件清单和版本列表。

pub mod files;
pub mod info;

use eframe::egui::{self, Align, Label, Layout, RichText, Ui, Vec2};

use crate::app::IconCache;
use crate::app::{Action, DetailTab};
use crate::app::{Catalog, ListView};
use crate::model::{AppEntry, DesktopEntry, InstalledPackage, RepoPackage};
use crate::ui::entry_status;
use crate::ui::icons::{draw_letter_icon, load_icon_texture};
use crate::util::AppPaths;

/// 一次绘制所需、已经从各处取好的数据。
pub(crate) struct Context<'a> {
    pub(crate) entry: AppEntry,
    pub(crate) installed: Option<&'a InstalledPackage>,
    /// 软件源里该包的全部候选版本（降序，由 `RepositoryData` 保证）。
    pub(crate) available: &'a [RepoPackage],
    pub(crate) desktop: Option<&'a DesktopEntry>,
}

/// 绘制详情页，返回用户触发的动作。
pub(crate) fn show(
    list: &mut ListView,
    catalog: &Catalog,
    icons: &mut IconCache,
    paths: &AppPaths,
    ui: &mut Ui,
) -> Vec<Action> {
    let Some(entry) = list.selected_entry(catalog).cloned() else {
        show_placeholder(ui);
        return Vec::new();
    };

    let installed = catalog.installed_package(&entry.pkg);
    let context = Context {
        desktop: installed.and_then(|package| package.desktop.as_ref()),
        available: catalog.versions(&entry.pkg),
        entry,
        installed,
    };

    show_header(icons, &context, ui);
    let mut actions = show_actions(catalog, paths, &context, ui);
    show_tabs(list, &context, ui);

    match list.detail_tab {
        DetailTab::Info => actions.extend(info::show(&context, ui)),
        DetailTab::Files => files::show(list, &context, ui),
    }
    actions
}

fn show_placeholder(ui: &mut Ui) {
    ui.add_space(80.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("请选择一个应用查看详情")
                .size(16.0)
                .color(ui.visuals().weak_text_color()),
        );
    });
}

/// 图标 + 名称 + 状态徽标。
fn show_header(icons: &mut IconCache, context: &Context, ui: &mut Ui) {
    let entry = &context.entry;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let texture = context
            .desktop
            .and_then(|desktop| load_icon_texture(icons, ui.ctx(), &desktop.icon));
        match texture {
            Some(texture) => {
                ui.add(egui::Image::new(&texture).fit_to_exact_size(Vec2::splat(48.0)));
            }
            None => draw_letter_icon(ui, &entry.display_name),
        }
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(&entry.display_name).size(21.0).strong());
            ui.label(
                RichText::new(&entry.pkg)
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (status, color) = entry_status(ui, entry);
            ui.colored_label(color, RichText::new(status).small().strong());
        });
    });
}

/// 操作按钮行。
///
/// 每个「不能点」都要给出原因：按钮灰掉却不说为什么，用户只会反复点。
fn show_actions(
    catalog: &Catalog,
    paths: &AppPaths,
    context: &Context,
    ui: &mut Ui,
) -> Vec<Action> {
    let entry = &context.entry;
    let installed = context.installed.is_some();
    let mut actions = Vec::new();

    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        // 「打开」要求既有安装记录又有桌面条目：CLI 工具没有可启动的入口。
        let open = ui
            .add_enabled(
                installed && context.desktop.is_some(),
                egui::Button::new("打开"),
            )
            .on_disabled_hover_text(if installed {
                "该软件包没有可用的桌面启动项"
            } else {
                "应用尚未安装"
            });
        if open.clicked()
            && let Some(desktop) = context.desktop.cloned()
        {
            actions.push(Action::Open {
                desktop,
                name: entry.display_name.clone(),
            });
        }

        let uninstall = ui
            .add_enabled(installed, egui::Button::new("卸载"))
            .on_disabled_hover_text("应用尚未安装");
        if uninstall.clicked() {
            actions.push(Action::Uninstall {
                pkg: entry.pkg.clone(),
                name: entry.display_name.clone(),
            });
        }

        let hold_label = if entry.is_held {
            "取消保持版本"
        } else {
            "保持版本"
        };
        let hold = ui
            .add_enabled(installed, egui::Button::new(hold_label))
            .on_disabled_hover_text("应用尚未安装");
        if hold.clicked() {
            actions.push(Action::ToggleHold {
                pkg: entry.pkg.clone(),
                name: entry.display_name.clone(),
                held: entry.is_held,
            });
        }

        ui.separator();

        let downloadable = context
            .available
            .iter()
            .any(|package| package.download_url().is_some());
        let download = ui
            .add_enabled(downloadable, egui::Button::new("在线获取安装包"))
            .on_disabled_hover_text(if catalog.repository_loading() {
                "软件源仍在加载中"
            } else {
                "软件源中没有该软件包的可下载版本"
            });
        if download.clicked() {
            actions.push(Action::Download {
                pkg: entry.pkg.clone(),
                name: entry.display_name.clone(),
            });
        }

        let build = ui
            .add_enabled(installed, egui::Button::new("离线提取安装包"))
            .on_disabled_hover_text("应用尚未安装");
        if build.clicked() {
            actions.push(Action::Build {
                pkg: entry.pkg.clone(),
                name: entry.display_name.clone(),
            });
        }

        if ui
            .small_button("下载目录")
            .on_hover_text(paths.downloads.display().to_string())
            .clicked()
        {
            actions.push(Action::OpenDownloads);
        }
    });
    actions
}

/// 标签页切换条。
fn show_tabs(list: &mut ListView, context: &Context, ui: &mut Ui) {
    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        if ui
            .selectable_label(list.detail_tab == DetailTab::Info, "信息")
            .clicked()
        {
            list.detail_tab = DetailTab::Info;
        }
        let files_label = format!(
            "文件 ({})",
            context.installed.map_or(0, |package| package.files.len())
        );
        if ui
            .selectable_label(list.detail_tab == DetailTab::Files, files_label)
            .clicked()
        {
            list.detail_tab = DetailTab::Files;
        }
    });
    ui.add_space(4.0);
}

/// 左侧标签 + 右侧取值的一行。
pub(crate) fn field_row(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        field_label(ui, label);
        ui.add(Label::new(value).wrap());
    });
    ui.add_space(4.0);
}

/// 与 [`field_row`] 同布局，但允许调用方自己画右侧的内容（链接、按钮等）。
pub(crate) fn field_with_value(ui: &mut Ui, label: &str, draw: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        field_label(ui, label);
        draw(ui);
    });
    ui.add_space(4.0);
}

/// 定宽的字段标签，让所有取值左对齐。
fn field_label(ui: &mut Ui, label: &str) {
    ui.add_sized(
        [88.0, 20.0],
        Label::new(RichText::new(label).color(ui.visuals().weak_text_color())),
    );
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
