//! 在线下载对话框：选版本、看保存位置、确认下载。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _};
use gpui_kit::component::{StyledExt as _, v_flex};
use gpui_kit::{
    AnyElement, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::app::{Catalog, DebDeckApp, DownloadDialog, ModalState};
use crate::model::RepoPackage;
use crate::util::format_bytes;

use super::{actions, body, overlay, title};

/// 绘制下载框。
pub(crate) fn show(
    catalog: &Catalog,
    dialog: &DownloadDialog,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    if dialog.versions.is_empty() {
        return empty_dialog(cx);
    }
    let installed = catalog.installed_version(&dialog.pkg);
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let mono = theme.mono_font_family.clone();
    let hover = theme.list_hover;
    let selection = theme.selection;
    let warning = theme.warning;
    let current = dialog.selected.min(dialog.versions.len().saturating_sub(1));
    let row_context = DownloadListContext {
        dialog,
        installed: installed.as_ref(),
        current,
        hover,
        selection,
    };
    let version_rows = dialog
        .versions
        .iter()
        .enumerate()
        .map(|(index, package)| download_row(index, package, &row_context, cx))
        .collect::<Vec<_>>();
    let card = v_flex()
        .gap(px(10.0))
        .child(title("在线获取安装包"))
        .child(body(format!("{}（{}）", dialog.name, dialog.pkg)))
        .child(div().text_xs().text_color(muted).child("版本："))
        .child(version_list(version_rows))
        .child(destination_block(dialog, muted, mono, warning))
        .child(source_block(dialog, muted))
        .child(dialog_actions(dialog, cx))
        .into_any_element();
    overlay(card, true, cx)
}

fn empty_dialog(cx: &mut Context<DebDeckApp>) -> AnyElement {
    let close = Button::new("close-empty-download")
        .label("关闭")
        .primary()
        .on_click(cx.listener(|this, _, _, cx| {
            this.dialogs.modal = None;
            cx.notify();
        }));
    let card = v_flex()
        .gap(px(12.0))
        .child(title("在线获取安装包"))
        .child(body("没有可下载的版本。"))
        .child(actions().child(close))
        .into_any_element();
    overlay(card, true, cx)
}

#[derive(Clone, Copy)]
struct DownloadListContext<'a> {
    dialog: &'a DownloadDialog,
    installed: Option<&'a (String, String)>,
    current: usize,
    hover: Hsla,
    selection: Hsla,
}

fn download_row(
    index: usize,
    package: &RepoPackage,
    context: &DownloadListContext<'_>,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    version_row(
        DownloadRowSpec {
            package,
            dialog: context.dialog,
            installed: context.installed,
            index,
            current: context.current,
            hover: context.hover,
            selection: context.selection,
        },
        cx,
    )
}

#[derive(Clone, Copy)]
struct DownloadRowSpec<'a> {
    package: &'a RepoPackage,
    dialog: &'a DownloadDialog,
    installed: Option<&'a (String, String)>,
    index: usize,
    current: usize,
    hover: Hsla,
    selection: Hsla,
}

fn version_row(spec: DownloadRowSpec<'_>, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let label = choice_label(spec.package, spec.installed);
    let mut choose = spec.dialog.clone();
    choose.selected = spec.index;
    let mut row = div()
        .id(("download-version", spec.index))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .text_sm()
        .cursor_pointer()
        .hover(move |this| this.bg(spec.hover))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.dialogs.modal = Some(ModalState::Download(choose.clone()));
            cx.notify();
        }))
        .child(label);
    if spec.index == spec.current {
        row = row.bg(spec.selection);
    }
    row.into_any_element()
}

fn version_list(rows: Vec<AnyElement>) -> AnyElement {
    div()
        .id("download-versions")
        .max_h(px(200.0))
        .overflow_y_scroll()
        .v_flex()
        .gap(px(2.0))
        .children(rows)
        .into_any_element()
}

fn destination_block(
    dialog: &DownloadDialog,
    muted: Hsla,
    mono: SharedString,
    warning: Hsla,
) -> AnyElement {
    let destination = dialog.destination();
    v_flex()
        .gap(px(3.0))
        .child(div().text_xs().text_color(muted).child("保存位置："))
        .children(destination.as_ref().map(|path| {
            div()
                .text_xs()
                .font_family(mono)
                .whitespace_normal()
                .child(path.display().to_string())
                .into_any_element()
        }))
        .children(destination.as_ref().filter(|path| path.exists()).map(|_| {
            div()
                .text_xs()
                .text_color(warning)
                .child("同名文件已存在，继续下载会覆盖它。")
                .into_any_element()
        }))
        .into_any_element()
}

fn source_block(dialog: &DownloadDialog, muted: Hsla) -> AnyElement {
    dialog
        .chosen()
        .and_then(RepoPackage::download_url)
        .map_or_else(
            || div().into_any_element(),
            |url| source_details(url, muted),
        )
}

fn source_details(url: String, muted: Hsla) -> AnyElement {
    v_flex()
        .gap(px(3.0))
        .child(div().text_xs().text_color(muted).child("下载地址："))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .whitespace_normal()
                .child(url),
        )
        .into_any_element()
}

fn dialog_actions(dialog: &DownloadDialog, cx: &mut Context<DebDeckApp>) -> AnyElement {
    let cancel = Button::new("cancel-download")
        .label("取消")
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| {
            this.dialogs.modal = None;
            cx.notify();
        }));
    let package = dialog.chosen().cloned();
    let destination = dialog.destination();
    let name = dialog.name.clone();
    let enabled = package.is_some() && destination.is_some();
    let start = Button::new("start-download")
        .label("开始下载")
        .primary()
        .disabled(!enabled)
        .on_click(cx.listener(move |this, _, _, cx| {
            if let (Some(package), Some(destination)) = (package.clone(), destination.clone()) {
                this.start_download(&name, &package, destination);
                cx.notify();
            }
        }));
    actions().child(cancel).child(start).into_any_element()
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
