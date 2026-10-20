//! 详情页「文件」标签：包内文件清单。
//!
//! 一个包可能有上万个文件，逐行绘制与逐条 `to_lowercase` 都会掉帧：
//! 没有过滤词时直接按索引取原列表，不做任何中间集合。

use eframe::egui::{Label, RichText, ScrollArea, TextEdit, Ui};

use crate::app::ListView;
use crate::model::InstalledPackage;

use super::Context;

/// 单行高度：等宽小字号正好 18px。
const ROW_HEIGHT: f32 = 18.0;

/// 绘制文件清单。
pub(crate) fn show(list: &mut ListView, context: &Context, ui: &mut Ui) {
    let Some(package) = context.installed else {
        show_not_installed(ui);
        return;
    };

    show_search_row(list, package, ui);
    ui.add_space(4.0);

    let query = list.files_query.trim().to_lowercase();
    let matches: Option<Vec<usize>> = (!query.is_empty()).then(|| {
        package
            .files
            .iter()
            .enumerate()
            .filter(|(_, path)| path.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect()
    });
    let total = matches.as_ref().map_or(package.files.len(), Vec::len);

    if total == 0 {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("没有匹配的文件").color(ui.visuals().weak_text_color()));
        });
        return;
    }

    ScrollArea::vertical()
        .id_salt("detail_files_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, total, |ui, range| {
            for row in range {
                let path: &str = match &matches {
                    Some(indices) => &package.files[indices[row]],
                    None => &package.files[row],
                };
                ui.add(Label::new(RichText::new(path).monospace().small()).truncate())
                    .on_hover_text(path);
            }
        });
}

/// 顶部过滤输入框与计数。
fn show_search_row(list: &mut ListView, package: &InstalledPackage, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.add(
            TextEdit::singleline(&mut list.files_query)
                .hint_text("在文件列表中查找")
                .desired_width(260.0),
        );
        if !list.files_query.is_empty() && ui.small_button("清除").clicked() {
            list.files_query.clear();
        }
        ui.label(
            RichText::new(format!("共 {} 个文件", package.files.len()))
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    });
}

fn show_not_installed(ui: &mut Ui) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("该应用尚未安装")
                .size(15.0)
                .color(ui.visuals().weak_text_color()),
        );
        ui.label(
            RichText::new("安装后可在这里查看它包含的文件")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    });
}
