//! 安装窗口的阶段内容。

use std::path::Path;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputContentType};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::app::InstallApp;
use crate::model::DebFile;
use crate::runtime::install::InstalledStatus;
use crate::util::{format_bytes, format_kib};

use super::actions::{auth_hint, chip, icon_tile, notice};
use super::progress::{log_drawer, progress_bar};

/// 包信息卡片的配色集合。
#[derive(Clone, Copy)]
struct CardColors {
    primary: Hsla,
    muted: Hsla,
    background: Hsla,
    border: Hsla,
}

/// 授权页的只读展示参数，把原先过长的平铺参数收敛为一个对象。
#[derive(Clone, Copy)]
pub(super) struct PasswordView<'a> {
    /// polkit 动作说明。
    pub(super) message: &'a str,
    /// PAM 提示文本。
    pub(super) prompt: &'a str,
    /// true 表示明文输入。
    pub(super) echo: bool,
    /// 上次认证失败信息。
    pub(super) error: Option<&'a str>,
    /// 过程中的辅助说明。
    pub(super) note: Option<&'a str>,
}

/// 确认页：包信息卡片 + 来源提示。
pub(super) fn confirm(
    file: &DebFile,
    installed: &InstalledStatus,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap(px(10.0))
        .child(package_card(file, installed, cx))
        .child(notice(
            IconName::TriangleAlert,
            cx.theme().warning,
            "安装包来自本地文件，其维护脚本将以管理员身份执行。请确认文件来源可信。",
        ))
        .child(auth_hint(cx))
        .into_any_element()
}

/// 包信息卡片：图标、名称、摘要、版本标签（含已安装对比）、文件路径。
fn package_card(
    file: &DebFile,
    installed: &InstalledStatus,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let theme = cx.theme();
    let colors = CardColors {
        primary: theme.primary,
        muted: theme.muted_foreground,
        background: theme.background,
        border: theme.border,
    };
    v_flex()
        .w_full()
        .gap(px(12.0))
        .p(px(14.0))
        .rounded(px(10.0))
        .bg(theme.muted)
        .border_1()
        .border_color(colors.border)
        .child(package_heading(file, colors.primary, colors.muted))
        .child(package_chips(
            file,
            installed,
            colors,
            theme.info,
            theme.warning,
        ))
        .child(div().h(px(1.0)).bg(colors.border))
        .child(package_path(
            &file.path,
            colors.muted,
            theme.mono_font_family.clone(),
        ))
        .into_any_element()
}

fn package_heading(file: &DebFile, primary: Hsla, muted: Hsla) -> AnyElement {
    let metadata = &file.metadata;
    h_flex()
        .items_start()
        .gap(px(12.0))
        .child(icon_tile(IconName::Package, primary, 44.0, false))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(metadata.name.clone()),
                )
                .children((!metadata.summary.is_empty()).then(|| {
                    div()
                        .text_xs()
                        .text_color(muted)
                        .line_clamp(2)
                        .child(metadata.summary.clone())
                })),
        )
        .child(
            h_flex()
                .items_center()
                .gap(px(4.0))
                .flex_shrink_0()
                .child(Icon::new(IconName::FileArchive).xsmall().text_color(muted))
                .child(div().text_xs().text_color(muted).child("本地 .deb")),
        )
        .into_any_element()
}

fn package_chips(
    file: &DebFile,
    installed: &InstalledStatus,
    colors: CardColors,
    info: Hsla,
    warning: Hsla,
) -> AnyElement {
    let metadata = &file.metadata;
    let mut row = h_flex()
        .gap(px(6.0))
        .flex_wrap()
        .child(chip(
            metadata.version.clone(),
            colors.primary,
            colors.primary.opacity(0.12),
            None,
        ))
        .child(chip(
            metadata.arch.to_string(),
            colors.muted,
            colors.background,
            Some(colors.border),
        ))
        .child(chip(
            format_bytes(file.file_size),
            colors.muted,
            colors.background,
            Some(colors.border),
        ))
        .child(chip(
            format!("安装后 {}", format_kib(metadata.installed_size_kb)),
            colors.muted,
            colors.background,
            Some(colors.border),
        ));

    if let Some((text, color)) = installed_status_badge(installed, info, warning) {
        row = row.child(chip(text, color, color.opacity(0.12), None));
    }
    row.into_any_element()
}

fn installed_status_badge(
    installed: &InstalledStatus,
    info: Hsla,
    warning: Hsla,
) -> Option<(String, Hsla)> {
    match installed {
        InstalledStatus::NotInstalled => None,
        InstalledStatus::SameVersion => Some(("已安装同版本 · 将重新安装".to_string(), info)),
        InstalledStatus::Upgrade { installed } => {
            Some((format!("已安装 {installed} · 将升级"), info))
        }
        InstalledStatus::Downgrade { installed } => {
            Some((format!("已安装 {installed} · 将降级"), warning))
        }
        InstalledStatus::OtherVersion { installed } => Some((format!("已安装 {installed}"), info)),
    }
}

fn package_path(path: &Path, muted: Hsla, mono: SharedString) -> AnyElement {
    h_flex()
        .items_start()
        .gap(px(10.0))
        .child(div().text_xs().text_color(muted).child("文件"))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_xs()
                .font_family(mono)
                .text_color(muted)
                .truncate()
                .child(path.display().to_string()),
        )
        .into_any_element()
}

/// 授权等待中：居中旋转指示。
pub(super) fn running_auth(
    file: &DebFile,
    status: &str,
    note: Option<&str>,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let primary = cx.theme().primary;
    let muted = cx.theme().muted_foreground;
    v_flex()
        .w_full()
        .items_center()
        .gap(px(12.0))
        .child(spinner_badge(primary))
        .child(running_identity(file, status, muted))
        .children(note.map(|note| {
            div()
                .w_full()
                .child(notice(IconName::Info, cx.theme().info, note))
        }))
        .into_any_element()
}

/// 正在安装：阶段进度条 + 可折叠实时日志。
pub(super) fn running_install(
    app: &mut InstallApp,
    file: &DebFile,
    note: Option<&str>,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let primary = cx.theme().primary;
    let muted = cx.theme().muted_foreground;
    let stage = app.install_stage().clone();
    let logs = app.logs().to_vec();
    let show_logs = app.show_logs();

    v_flex()
        .w_full()
        .items_center()
        .gap(px(12.0))
        .child(spinner_badge(primary))
        .child(running_identity(file, stage.label, muted))
        .child(progress_bar(stage.progress, cx))
        .children(note.map(|note| {
            div()
                .w_full()
                .child(notice(IconName::Info, cx.theme().info, note))
        }))
        .child(log_drawer(&logs, show_logs, cx))
        .into_any_element()
}

fn spinner_badge(primary: Hsla) -> AnyElement {
    div()
        .size(px(52.0))
        .rounded_full()
        .bg(primary.opacity(0.12))
        .flex()
        .items_center()
        .justify_center()
        .child(Spinner::new().large().color(primary))
        .into_any_element()
}

fn running_identity(file: &DebFile, status: &str, muted: Hsla) -> AnyElement {
    v_flex()
        .items_center()
        .gap(px(4.0))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("正在安装 {}", file.metadata.name)),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .text_center()
                .child(status.to_string()),
        )
        .into_any_element()
}

/// 内嵌授权：中文说明 + 密码输入卡片 + 可折叠原始命令 + 重试错误。
pub(super) fn need_password(
    app: &mut InstallApp,
    file: &DebFile,
    view: PasswordView<'_>,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let primary = cx.theme().primary;
    let show_cmd = app.show_auth_command();
    let prompt = normalise_prompt(view.prompt, view.echo);

    v_flex()
        .w_full()
        .gap(px(14.0))
        .child(
            h_flex()
                .items_start()
                .gap(px(12.0))
                .child(icon_tile(IconName::LockKeyhole, primary, 44.0, false))
                .child(auth_heading(&file.metadata.name, muted)),
        )
        .child(password_card(
            PasswordCard {
                app,
                file,
                prompt,
                echo: view.echo,
                error: view.error,
                note: view.note,
            },
            cx,
        ))
        .child(auth_footer(view.message, show_cmd, muted, cx))
        .into_any_element()
}

fn normalise_prompt(prompt: &str, echo: bool) -> &str {
    let trimmed = prompt.trim();
    let lower = trimmed.trim_end_matches(':').trim().to_ascii_lowercase();
    if matches!(lower.as_str(), "" | "password" | "密码") {
        if echo {
            "请输入请求的内容"
        } else {
            "管理员密码"
        }
    } else {
        trimmed
    }
}

fn auth_heading(pkg_name: &str, muted: Hsla) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child("需要管理员授权"),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .whitespace_normal()
                .child(format!("请输入当前用户的登录密码，以授权安装 {pkg_name}。")),
        )
        .into_any_element()
}

fn auth_footer(
    raw_message: &str,
    show_cmd: bool,
    muted: Hsla,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let theme = cx.theme();
    let bg = theme.muted;
    let border = theme.border;
    let mono = theme.mono_font_family.clone();

    let mut footer = v_flex().w_full().gap(px(6.0)).child(
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap(px(8.0))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(Icon::new(IconName::ShieldCheck).xsmall().text_color(muted))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("密码通过系统 polkit 校验，输入后立即清除。"),
                    ),
            )
            .children((!raw_message.trim().is_empty()).then(|| {
                Button::new("toggle-auth-command")
                    .label(if show_cmd {
                        "收起命令 ▴"
                    } else {
                        "查看命令 ▾"
                    })
                    .compact()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_auth_command(cx)))
            })),
    );

    if show_cmd && !raw_message.trim().is_empty() {
        footer = footer.child(
            div()
                .w_full()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(bg)
                .border_1()
                .border_color(border)
                .text_xs()
                .font_family(mono)
                .text_color(muted)
                .whitespace_normal()
                .child(raw_message.trim().to_string()),
        );
    }
    footer.into_any_element()
}

struct PasswordCard<'a> {
    app: &'a mut InstallApp,
    file: &'a DebFile,
    prompt: &'a str,
    echo: bool,
    error: Option<&'a str>,
    note: Option<&'a str>,
}

/// 密码输入卡片：中文提示语、无 IME 干扰的密码框与重试错误。
fn password_card(card: PasswordCard<'_>, cx: &mut Context<InstallApp>) -> AnyElement {
    let PasswordCard {
        app,
        file,
        prompt,
        echo,
        error,
        note,
    } = card;
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let border = theme.border;
    let input_element: AnyElement = if echo {
        Input::new(app.password())
            .w_full()
            .content_type(InputContentType::Username)
            .into_any_element()
    } else {
        app.secret_password().clone().into_any_element()
    };

    v_flex()
        .w_full()
        .gap(px(10.0))
        .p(px(14.0))
        .rounded(px(10.0))
        .bg(theme.muted)
        .border_1()
        .border_color(border)
        .child(password_prompt(
            file,
            prompt,
            PromptStyle {
                muted,
                background: theme.background,
                border,
            },
        ))
        .child(input_element)
        .children(error.map(|error| notice(IconName::CircleAlert, theme.danger, error)))
        .children(note.map(|note| notice(IconName::Info, theme.info, note)))
        .into_any_element()
}

#[derive(Clone, Copy)]
struct PromptStyle {
    muted: Hsla,
    background: Hsla,
    border: Hsla,
}

fn password_prompt(file: &DebFile, prompt: &str, style: PromptStyle) -> AnyElement {
    h_flex()
        .justify_between()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .text_xs()
                .text_color(style.muted)
                .child(prompt.to_string()),
        )
        .child(chip(
            format!("安装 {}", file.metadata.name),
            style.muted,
            style.background,
            Some(style.border),
        ))
        .into_any_element()
}
