//! 状态栏：统计、软件源状态与快捷入口。

use eframe::egui::{self, Align, Layout, RichText, Ui};

use crate::app::Action;
use crate::app::{Catalog, Dialogs};
use crate::util::AppPaths;

/// 绘制状态栏。
///
/// `visible_count` 由调用方在算完可见行之后传入 —— 状态栏因此不需要认识列表状态。
pub(crate) fn show(
    catalog: &Catalog,
    dialogs: &mut Dialogs,
    paths: &AppPaths,
    visible_count: usize,
    refresh_allowed: bool,
    ui: &mut Ui,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let counts = catalog.counts();
    let warnings: Vec<String> = catalog.repository_warnings().to_vec();

    egui::Panel::bottom("status_bar").show(ui, |ui| {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.label(format!(
                "共 {} 个应用 · 已安装 {} 个 · 界面应用 {} 个 · 保持版本 {} 个 · 可更新 {} 个 · 显示 {visible_count} 个",
                counts.total, counts.installed, counts.gui, counts.held, counts.upgradable,
            ));

            // 降级判定要说出来：否则「可更新 0 个」会被读成「都已经最新」。
            if catalog.upgradable_degraded() {
                ui.label(
                    RichText::new("更新状态未确认（apt 查询失败，按版本号比较推断）")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("下载目录")
                    .on_hover_text(paths.downloads.display().to_string())
                    .clicked()
                {
                    actions.push(Action::OpenDownloads);
                }
                if !warnings.is_empty()
                    && ui
                        .small_button(format!("软件源警告 {}", warnings.len()))
                        .on_hover_text("点击查看明细")
                        .clicked()
                {
                    // 只是展示文本，留在本层完成，不必绕一圈动作。
                    dialogs.set_message("软件源警告", warnings.join("\n"));
                }
                actions.extend(draw_repository_status(
                    catalog,
                    &warnings,
                    refresh_allowed,
                    ui,
                ));
            });
        });
        ui.add_space(3.0);
    });
    actions
}

/// 右侧的软件源状态。
///
/// 索引为空时不写「已加载」——那是误导，改成可点击的重试入口。
fn draw_repository_status(
    catalog: &Catalog,
    warnings: &[String],
    refresh_allowed: bool,
    ui: &mut Ui,
) -> Option<Action> {
    if catalog.repository_loading() {
        ui.spinner();
        ui.label(
            RichText::new("正在加载软件源…")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        return None;
    }

    if !catalog.has_packages() {
        let label = if refresh_allowed {
            "软件源不可用，点此重试"
        } else {
            "软件源不可用"
        };
        let hint = warnings
            .first()
            .map_or("尚未读取软件源索引", String::as_str);
        if ui.small_button(label).on_hover_text(hint).clicked() && refresh_allowed {
            return Some(Action::Refresh);
        }
        return None;
    }

    ui.label(
        RichText::new("软件源已加载")
            .small()
            .color(ui.visuals().weak_text_color()),
    );
    None
}
