//! 绘制层：读状态、画界面、交回动作，不碰 `backend` 与 `runtime`。
//!
//! 每个模块的函数形式都是 `fn show_x(view: &mut View, ui: &mut Ui) -> Vec<Action>`
//! （或 `Vec<ModalOutcome>`）：状态更新写成对 `View` 里某个簇的直接调用，需要产生
//! 副作用的写成返回值，由 `app` 层统一执行。这样「界面上点了什么」和「系统被改了
//! 什么」在代码上是分开的两件事，可以各自单测。
//!
//! 子模块：[`toolbar`] 顶栏、[`status_bar`] 状态栏、[`app_list`] 列表、
//! [`details`] 详情、[`modals`] 模态、[`notices`] 加载与轻提示、
//! [`icons`] 图标位图、[`theme`] 字体与配色。

pub mod app_list;
pub mod details;
pub mod icons;
pub mod modals;
pub mod notices;
pub mod status_bar;
pub mod theme;
pub mod toolbar;

use std::collections::HashMap;

use eframe::egui::{self, Ui};

pub use icons::window_icon;
pub(crate) use notices::{show_loading_panel, show_toast};
pub(crate) use theme::{configure_fonts, configure_style};

use crate::app::{Action, DebDeckApp, IconCache, ModalOutcome};
use crate::app::{Catalog, Dialogs, ListView};
use crate::model::AppEntry;
use crate::util::AppPaths;

/// 一次绘制所需的全部上下文。
///
/// 字段是彼此独立的可变借用，因此函数内部按字段解构即可同时读写多个簇，
/// 不需要把整个 `DebDeckApp` 传进每个绘制函数。
pub(crate) struct View<'a> {
    pub(crate) ctx: egui::Context,
    pub(crate) paths: &'a AppPaths,
    pub(crate) catalog: &'a mut Catalog,
    pub(crate) list: &'a mut ListView,
    pub(crate) dialogs: &'a mut Dialogs,
    pub(crate) icons: &'a mut HashMap<String, Option<egui::TextureHandle>>,
}

impl<'a> View<'a> {
    /// 从应用状态借出一次绘制所需的全部上下文。
    pub(crate) fn new(app: &'a mut DebDeckApp) -> Self {
        Self {
            ctx: app.ctx.clone(),
            paths: &app.paths,
            catalog: &mut app.catalog,
            list: &mut app.list,
            dialogs: &mut app.dialogs,
            icons: &mut app.icons,
        }
    }
}

impl View<'_> {
    /// 数据或筛选条件变了就重算可见行。
    pub(crate) fn ensure_current(&mut self) {
        let View { catalog, list, .. } = self;
        list.ensure_current(catalog);
    }
}

/// 绘制主界面（顶栏、状态栏、列表、详情），返回产生的动作。
pub(crate) fn show(view: &mut View, ui: &mut Ui) -> Vec<Action> {
    let View {
        catalog,
        dialogs,
        list,
        icons,
        paths,
        ..
    } = view;

    let refresh_allowed = crate::app::refresh_allowed(catalog, dialogs);
    let mut actions = toolbar::show(list, refresh_allowed, ui);
    let visible_count = list.visible().len();
    actions.extend(status_bar::show(
        catalog,
        dialogs,
        paths,
        visible_count,
        refresh_allowed,
        ui,
    ));
    actions.extend(main_panels(list, catalog, icons, paths, ui));
    actions
}

/// 左侧列表 + 右侧详情。两个面板各自返回自己的动作。
fn main_panels(
    list: &mut ListView,
    catalog: &Catalog,
    icons: &mut IconCache,
    paths: &AppPaths,
    ui: &mut Ui,
) -> Vec<Action> {
    egui::Panel::left("app_list_panel")
        .resizable(true)
        .default_size(760.0)
        .min_size(480.0)
        .max_size(1100.0)
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(10, 6)))
        .show(ui, |ui| {
            app_list::show(list, catalog, ui);
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::symmetric(14, 8)))
        .show(ui, |ui| details::show(list, catalog, icons, paths, ui))
        .inner
}

/// 绘制当前模态框，交回需要执行的结果。
pub(crate) fn show_modals(view: &mut View, ui: &mut Ui) -> Vec<ModalOutcome> {
    modals::show(view, ui)
}

/// 列表与详情共用的状态标签：文字 + 前景色。
///
/// 颜色全部取自 [`theme`]，不再在各处内联明暗两套 RGB。
pub(crate) fn entry_status(ui: &Ui, entry: &AppEntry) -> (String, egui::Color32) {
    let visuals = ui.visuals();
    if !entry.is_installed {
        return ("未安装".to_string(), visuals.weak_text_color());
    }
    // 顺序即优先级：保持版本比「可更新」更能描述用户的选择。
    let (label, palette) = if entry.is_held {
        ("已保持", theme::STATUS_HELD)
    } else if entry.has_update {
        ("可更新", theme::STATUS_UPDATE)
    } else {
        ("已安装", theme::STATUS_INSTALLED)
    };
    let color = if visuals.dark_mode {
        palette.dark
    } else {
        palette.light
    };
    (label.to_string(), color)
}

/// 空字符串统一渲染成占位符，界面里不留空白单元格。
pub(crate) fn value_or_dash(value: &str) -> &str {
    if value.is_empty() { "—" } else { value }
}

/// 安装状态的一句话描述，详情页使用。
pub(crate) fn installation_summary(entry: &AppEntry) -> &'static str {
    match (entry.is_installed, entry.is_held) {
        (true, true) => "已安装（保持版本）",
        (true, false) => "已安装",
        (false, _) => "未安装",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, atomic::AtomicBool};

    use eframe::egui;

    use super::*;
    use crate::app::{Confirm, DownloadDialog, ModalState, Operation};
    use crate::deb822::Stanza;
    use crate::model::RepoPackage;
    use crate::model::{DpkgState, InstalledData, InstalledPackage, PackageMetadata};
    use crate::util::AppPaths;

    /// 造一个有内容的 Catalog：一个界面应用 + 一个可更新 + 一个未安装的源包。
    fn fixture_catalog() -> Catalog {
        let mut installed = InstalledData::default();
        for (name, version, gui) in [("firefox", "1.0", true), ("tree", "2.0", false)] {
            let mut files = vec!["/usr/bin/tool".to_string()];
            if gui {
                files.push("/usr/share/applications/firefox.desktop".to_string());
            }
            installed.packages.insert(
                name.to_string(),
                InstalledPackage {
                    metadata: PackageMetadata {
                        name: name.to_string(),
                        version: version.to_string(),
                        arch: "amd64".to_string(),
                        summary: "摘要".to_string(),
                        ..Default::default()
                    },
                    dpkg: DpkgState::parse("install ok installed"),
                    control: Stanza::default(),
                    files,
                    desktop: None,
                    updated: None,
                },
            );
        }
        Catalog::new().with_installed(installed)
    }

    /// 无头绘制一遍主界面 + 指定模态。
    ///
    /// 断言的是「一帧确实画出了东西且没 panic」：这一层此前完全无法测试，
    /// 界面接线（借用拆分、面板顺序、动作回传）出错只有肉眼看才发现。
    fn render(dialogs: Dialogs, label: &str) -> usize {
        let ctx = egui::Context::default();
        let paths = AppPaths::default();
        let mut catalog = fixture_catalog();
        let mut list = ListView::new();
        let mut dialogs = dialogs;
        let mut icons = HashMap::new();

        let had_modal = dialogs.modal.is_some();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut view = View {
                ctx: ctx.clone(),
                paths: &paths,
                catalog: &mut catalog,
                list: &mut list,
                dialogs: &mut dialogs,
                icons: &mut icons,
            };
            list_ensure_current(&mut view);
            let actions = show(&mut view, ui);
            let outcomes = show_modals(&mut view, ui);
            // 用户没点任何东西时，模态不该凭空消失。
            if had_modal && outcomes.is_empty() {
                assert!(
                    dialogs.modal.is_some(),
                    "{label}: 模态在没有用户选择的一帧后消失了"
                );
            }
            assert!(actions.is_empty(), "无交互的一帧不该产出动作");
        });
        let shapes = output.shapes.len();
        // egui 要求显式处理纹理增量，否则 FullOutput 析构时 panic（字体纹理每帧都有）。
        let mut output = output;
        output.textures_delta.clear();
        drop(output);
        shapes
    }

    fn list_ensure_current(view: &mut View) {
        view.ensure_current();
    }

    #[test]
    fn empty_values_fall_back_to_placeholder() {
        assert_eq!(value_or_dash(""), "—");
        assert_eq!(value_or_dash("amd64"), "amd64");
    }

    #[test]
    fn main_layout_renders() {
        let shapes = render(Dialogs::new(), "无模态");
        assert!(shapes > 20, "主界面只产出 {shapes} 个图元，几乎没有绘制");
    }

    #[test]
    fn modals_render() {
        for (label, modal) in [
            (
                "消息框",
                ModalState::Message {
                    title: "标题".to_string(),
                    message: "正文".to_string(),
                },
            ),
            (
                "确认框",
                ModalState::Confirm(Confirm::Uninstall {
                    pkg: "tree".to_string(),
                    name: "tree".to_string(),
                }),
            ),
            (
                "操作框",
                ModalState::Operation(Operation {
                    id: crate::runtime::OperationId(1),
                    title: "下载".to_string(),
                    message: "进行中".to_string(),
                    progress: Some((1024, 4096)),
                    result: None,
                    cancel: Some(Arc::new(AtomicBool::new(false))),
                    open_dir: None,
                    cancel_requested: false,
                }),
            ),
            // 版本列表为空的下载框属于非法状态（进入前已过滤），
            // 这里给一条真实候选，验证的是它的绘制而不是它的兜底。
            (
                "下载框",
                ModalState::Download(DownloadDialog {
                    pkg: "tree".to_string(),
                    name: "tree".to_string(),
                    versions: vec![RepoPackage {
                        metadata: PackageMetadata {
                            name: "tree".to_string(),
                            version: "2.0".to_string(),
                            arch: "amd64".to_string(),
                            ..Default::default()
                        },
                        filename: "pool/main/t/tree/tree_2.0_amd64.deb".to_string(),
                        repo_base: "https://example.com/ubuntu/".to_string(),
                        ..Default::default()
                    }],
                    selected: 0,
                    destination_dir: std::env::temp_dir(),
                }),
            ),
        ] {
            let mut dialogs = Dialogs::new();
            dialogs.modal = Some(modal);
            render(dialogs, label);
        }
    }
}
