//! 确认框：不可逆操作前的一次拦阻。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::v_flex;
use gpui_kit::*;

use crate::app::{Confirm, DebDeckApp};
use crate::util::AppPaths;

use super::{actions, body, overlay, title};

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
        Confirm::UpgradeAll { count } => ConfirmCopy {
            title: "全部更新",
            message: format!(
                "将通过 APT 升级当前全部 {count} 个可更新的软件包（`apt-get upgrade`）。\n\n已保持版本（Hold）的应用将被自动跳过，过程中可能需要管理员授权。"
            ),
            confirm_label: "开始更新",
            dangerous: false,
        },
        Confirm::UpgradePackage { pkg, name } => ConfirmCopy {
            title: "更新应用",
            message: format!(
                "将把「{name}」（软件包 {pkg}）升级到软件源中的最新版本。\n\n可能会同时更新相关的依赖包，过程中可能需要管理员授权。"
            ),
            confirm_label: "立即更新",
            dangerous: false,
        },
    }
}

/// 绘制确认框。
pub(crate) fn show(
    paths: &AppPaths,
    confirm: &Confirm,
    cx: &mut Context<DebDeckApp>,
) -> AnyElement {
    let copy = confirm_copy(confirm, paths);
    let confirm_value = confirm.clone();

    let mut confirm_button = Button::new("confirm-action").label(copy.confirm_label);
    confirm_button = if copy.dangerous {
        confirm_button.danger()
    } else {
        confirm_button.primary()
    };
    let confirm_button = confirm_button.on_click(cx.listener(move |this, _, _, cx| {
        this.start_confirmed(confirm_value.clone());
        cx.notify();
    }));

    let cancel = Button::new("cancel-confirm")
        .label("取消")
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| {
            this.dialogs.modal = None;
            cx.notify();
        }));

    let card = v_flex()
        .gap(px(12.0))
        .child(title(copy.title))
        .child(body(copy.message))
        .child(actions().child(cancel).child(confirm_button))
        .into_any_element();

    overlay(card, true, cx)
}
