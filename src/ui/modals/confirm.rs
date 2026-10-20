//! 确认框：不可逆操作前的一次拦阻。

use eframe::egui::{self, Align, Color32, Id, Label, Layout, RichText, Ui};

use crate::app::{Confirm, ModalOutcome, ModalState};
use crate::ui::theme::{ACCENT, DANGER};
use crate::util::AppPaths;

/// 确认框的文案。集中成一张表，避免在三处 `match` 里各写一半。
struct ConfirmCopy {
    title: &'static str,
    message: String,
    confirm_label: &'static str,
    /// 不可逆操作（卸载）用红色按钮，其余用主色。
    dangerous: bool,
}

fn confirm_copy(confirm: &Confirm, paths: &AppPaths) -> ConfirmCopy {
    match confirm {
        Confirm::Uninstall { pkg, name } => ConfirmCopy {
            title: "卸载应用",
            message: format!(
                "将从系统中卸载「{name}」（软件包 {pkg}）。\n\n卸载需要管理员授权，系统可能会弹出密码验证窗口。"
            ),
            confirm_label: "继续卸载",
            dangerous: true,
        },
        Confirm::Hold { name, hold, .. } => {
            if *hold {
                ConfirmCopy {
                    title: "保持版本",
                    message: format!(
                        "保持「{name}」的当前版本后，APT 将不再自动升级这个软件包。\n\n不建议对系统核心组件保持版本，可能导致依赖更新时出现问题。"
                    ),
                    confirm_label: "保持版本",
                    dangerous: false,
                }
            } else {
                ConfirmCopy {
                    title: "取消保持版本",
                    message: format!("取消保持「{name}」的版本后，该软件包将重新跟随 APT 升级。"),
                    confirm_label: "取消保持",
                    dangerous: false,
                }
            }
        }
        Confirm::Build { name, .. } => ConfirmCopy {
            title: "离线提取安装包",
            message: format!(
                "将从当前已安装的文件重新打包「{name}」为 deb 安装包。\n\n生成目录：{}\n\n该功能用于离线备份，重新生成的安装包可能与原始发行版存在差异。",
                paths.builds.display()
            ),
            confirm_label: "开始提取",
            dangerous: false,
        },
    }
}

/// 绘制确认框。留下 `Some` 表示用户还没做选择。
pub(crate) fn show(
    ctx: &egui::Context,
    confirm: Confirm,
    paths: &AppPaths,
    outcomes: &mut Vec<ModalOutcome>,
) -> Option<ModalState> {
    let text = confirm_copy(&confirm, paths);
    let mut confirmed = false;
    let mut cancelled = false;

    let response = egui::Modal::new(Id::unique("confirm_modal")).show(ctx, |ui: &mut Ui| {
        ui.set_min_width(430.0);
        ui.heading(text.title);
        ui.add_space(8.0);
        ui.add(Label::new(text.message).wrap());
        ui.add_space(16.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let fill = if text.dangerous { DANGER } else { ACCENT };
            let button = egui::Button::new(RichText::new(text.confirm_label).color(Color32::WHITE))
                .fill(fill);
            if ui.add(button).clicked() {
                confirmed = true;
            }
            if ui.button("取消").clicked() {
                cancelled = true;
            }
        });
    });

    if confirmed {
        outcomes.push(ModalOutcome::Confirm(confirm));
        return None;
    }
    if cancelled || response.should_close() {
        return None;
    }
    Some(ModalState::Confirm(confirm))
}
