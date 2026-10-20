//! 详情页「信息」标签：软件包元数据与下载地址。

use eframe::egui::{self, Label, RichText, ScrollArea, Ui};

use crate::app::Action;
use crate::model::{PackageMetadata, RepoPackage};
use crate::ui::details::{field_row, field_with_value, shorten_text};
use crate::ui::{installation_summary, value_or_dash};
use crate::util::{format_bytes, format_kib, format_time};

use super::Context;

/// 需要渲染的普通文本字段。
struct Field {
    label: &'static str,
    value: String,
}

/// 绘制信息页，返回「复制下载地址」动作。
pub(crate) fn show(context: &Context, ui: &mut Ui) -> Vec<Action> {
    // 已安装时用安装记录，未安装时用软件源最新版；两处都没有就不显示元数据块。
    let Some(info) = display_metadata(context) else {
        ui.label("没有可显示的软件包信息。");
        return Vec::new();
    };

    let download = download_source(context);
    let download_url = download.and_then(crate::model::RepoPackage::download_url);
    let package_size = download
        .map(|package| package.download_size)
        .filter(|size| *size > 0);

    let entry = &context.entry;
    let fields = [
        Field {
            label: "包名",
            value: info.name.clone(),
        },
        Field {
            label: "版本",
            value: version_text(context, info),
        },
        Field {
            label: "架构",
            value: info.arch.clone(),
        },
        Field {
            label: "安装后占用",
            value: if entry.installed_size_kb > 0 {
                format_kib(entry.installed_size_kb)
            } else {
                String::new()
            },
        },
        Field {
            label: "安装包大小",
            value: package_size.map(format_bytes).unwrap_or_default(),
        },
        Field {
            label: "更新时间",
            value: format_time(entry.updated),
        },
        Field {
            label: "维护者",
            value: info.maintainer.clone(),
        },
        Field {
            label: "依赖",
            value: info.depends.clone(),
        },
    ];

    let mut actions = Vec::new();
    ScrollArea::vertical()
        .id_salt("detail_info_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for field in fields {
                field_row(ui, field.label, value_or_dash(&field.value));
            }

            field_with_value(ui, "主页", |ui| {
                if info.homepage.is_empty() {
                    ui.label("—");
                } else {
                    ui.add(egui::Hyperlink::from_label_and_url(
                        shorten_text(&info.homepage, 70),
                        &info.homepage,
                    ))
                    .on_hover_text(&info.homepage);
                }
            });

            field_with_value(ui, "下载地址", |ui| match &download_url {
                Some(url) => {
                    ui.vertical(|ui| {
                        ui.add(Label::new(RichText::new(url).small()).wrap());
                        if ui.small_button("复制链接").clicked() {
                            actions.push(Action::Copy(url.clone()));
                        }
                    });
                }
                None => {
                    ui.label("—");
                }
            });

            if let Some(desktop) = context.desktop {
                field_row(ui, "桌面文件", &desktop.path.display().to_string());
                field_row(ui, "启动命令", &desktop.exec);
            }
            field_row(ui, "安装状态", installation_summary(&context.entry));

            ui.add_space(14.0);
            ui.label(RichText::new("描述").strong());
            ui.add_space(4.0);
            ui.add(Label::new(value_or_dash(&info.description)).wrap());
            ui.add_space(12.0);
        });
    actions
}

/// 展示用的元数据：优先安装记录，其次软件源最新版。
fn display_metadata<'a>(context: &Context<'a>) -> Option<&'a PackageMetadata> {
    context
        .installed
        .map(|package| &package.metadata)
        .or_else(|| context.available.first().map(|package| &package.metadata))
}

/// 用于取「安装包大小 / 下载地址」的那条源记录。
///
/// 已安装时优先取与安装版本完全一致的那条（大小才对得上），否则取最新版。
fn download_source<'a>(context: &Context<'a>) -> Option<&'a RepoPackage> {
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
fn version_text(context: &Context, info: &PackageMetadata) -> String {
    let entry = &context.entry;
    match (&entry.latest_version, entry.has_update) {
        (Some(latest), true) if *latest != info.version => {
            format!("{}（最新 {latest}）", info.version)
        }
        _ => info.version.clone(),
    }
}
