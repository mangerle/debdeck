//! 在线下载对话框：选版本、看保存位置、确认下载。

use eframe::egui::{self, Align, Color32, Id, Label, Layout, RichText, Ui};

use crate::app::Catalog;
use crate::app::{DownloadDialog, ModalOutcome, ModalState};
use crate::model::RepoPackage;
use crate::ui::theme::{ACCENT, WARNING};
use crate::util::format_bytes;

/// 绘制下载框。返回 `Some` 表示用户还在挑选版本。
pub(crate) fn show(
    ctx: &egui::Context,
    catalog: &Catalog,
    mut dialog: DownloadDialog,
    outcomes: &mut Vec<ModalOutcome>,
) -> Option<ModalState> {
    // 版本列表为空时不该出现在这里（进入前已过滤）；真出现了就关掉，
    // 而不是在 `versions[selected]` 上 panic。
    if dialog.versions.is_empty() {
        return None;
    }
    dialog.selected = dialog.selected.min(dialog.versions.len() - 1);

    let installed = catalog.installed_version(&dialog.pkg);
    let mut start = false;
    let mut cancel = false;

    let response = egui::Modal::new(Id::unique("download_modal")).show(ctx, |ui: &mut Ui| {
        ui.set_min_width(480.0);
        ui.heading("在线获取安装包");
        ui.add_space(6.0);
        ui.label(format!("{}（{}）", dialog.name, dialog.pkg));
        ui.add_space(12.0);

        draw_version_picker(&mut dialog, installed.as_ref(), ui);

        ui.add_space(8.0);
        if let Some(destination) = dialog.destination() {
            draw_destination(&destination, ui);
        }
        if let Some(package) = dialog.chosen() {
            draw_source_url(package, ui);
        }

        ui.add_space(16.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = ui.add_enabled(
                dialog.chosen().is_some(),
                egui::Button::new(RichText::new("开始下载").color(Color32::WHITE)).fill(ACCENT),
            );
            if button.clicked() {
                start = true;
            }
            if ui.button("取消").clicked() {
                cancel = true;
            }
        });
    });

    if start {
        // 预览与实际下载共用 `dialog.destination()` 这一处计算，
        // 因此界面写的路径与落盘路径不可能不一致。
        if let (Some(package), Some(destination)) = (dialog.chosen().cloned(), dialog.destination())
        {
            outcomes.push(ModalOutcome::StartDownload(Box::new(
                crate::app::DownloadStart {
                    name: dialog.name,
                    package,
                    destination,
                },
            )));
        }
        return None;
    }
    if cancel || response.should_close() {
        return None;
    }
    Some(ModalState::Download(dialog))
}

/// 版本下拉框；点击结果直接写回 `dialog.selected`。
fn draw_version_picker(
    dialog: &mut DownloadDialog,
    installed: Option<&(String, String)>,
    ui: &mut Ui,
) {
    ui.horizontal(|ui| {
        ui.label("版本：");
        egui::ComboBox::from_id_salt("download_version")
            .width(320.0)
            .selected_text(choice_label(&dialog.versions[dialog.selected], installed))
            .show_ui(ui, |ui| {
                for index in 0..dialog.versions.len() {
                    let label = choice_label(&dialog.versions[index], installed);
                    if ui
                        .selectable_label(index == dialog.selected, label)
                        .clicked()
                    {
                        dialog.selected = index;
                    }
                }
            });
    });
}

/// 保存位置：路径 + 同名覆盖警告。
fn draw_destination(destination: &std::path::Path, ui: &mut Ui) {
    ui.label("保存位置：");
    ui.add(
        Label::new(
            RichText::new(destination.display().to_string())
                .small()
                .monospace(),
        )
        .wrap(),
    );
    if destination.exists() {
        ui.label(
            RichText::new("同名文件已存在，继续下载会覆盖它。")
                .small()
                .color(WARNING),
        );
    }
}

/// 下载地址原文：便于用户核对镜像与路径。
fn draw_source_url(package: &RepoPackage, ui: &mut Ui) {
    ui.add_space(6.0);
    ui.label("下载地址：");
    ui.add(
        Label::new(
            RichText::new(package.download_url().unwrap_or_default())
                .small()
                .color(ui.visuals().weak_text_color()),
        )
        .wrap(),
    );
}

/// 下拉项文字：`版本 (架构) · 大小 · 已安装`。
fn choice_label(package: &RepoPackage, installed: Option<&(String, String)>) -> String {
    let is_installed = installed.is_some_and(|(version, arch)| {
        *version == package.metadata.version && *arch == package.metadata.arch
    });
    let size = if package.download_size > 0 {
        format!(" · {}", format_bytes(package.download_size))
    } else {
        String::new()
    };
    format!(
        "{} ({}){}{}",
        package.metadata.version,
        package.metadata.arch,
        size,
        if is_installed { " · 已安装" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PackageMetadata;

    fn package(name: &str, version: &str, arch: &str, size: u64) -> RepoPackage {
        RepoPackage {
            metadata: PackageMetadata {
                name: name.to_string(),
                version: version.to_string(),
                arch: arch.to_string(),
                ..Default::default()
            },
            download_size: size,
            ..Default::default()
        }
    }

    #[test]
    fn label_marks_installed_and_size() {
        let package = package("demo", "1.0", "amd64", 2048);
        let installed = ("1.0".to_string(), "amd64".to_string());
        let label = choice_label(&package, Some(&installed));
        assert_eq!(label, "1.0 (amd64) · 2 KB · 已安装");
        // 架构不同不算「已安装」，即便版本号相同。
        let other_arch = ("1.0".to_string(), "i386".to_string());
        assert!(!choice_label(&package, Some(&other_arch)).contains("已安装"));
        // 没有安装记录时也不该出现标记。
        assert!(!choice_label(&package, None).contains("已安装"));
    }

    #[test]
    fn label_omits_unknown_size() {
        let package = package("demo", "1.0", "all", 0);
        assert_eq!(choice_label(&package, None), "1.0 (all)");
    }
}
