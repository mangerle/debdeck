//! 独立安装窗口的绘制：确认 → 内嵌授权 → 安装 → 结果。
//!
//! 视觉上沿用主界面的 Ubuntu 橙主题：灰色卡片承载包信息，状态用图标 +
//! 色彩表达，底部固定操作栏。密码框属于这个窗口本身，不再调用桌面自带的
//! polkit 对话框。
//!
//! 窗口刻意做小：它只回答一个问题「这个包要不要装」。因此标题条不放最小化、
//! 最大化、关闭按钮 —— 唯一的出口是内容区里的「取消 / 确认」。
//!
//! 布局上分三段：标题条、可滚动的内容区、贴底的按钮栏。按钮栏不参与滚动，
//! 所以内容再长也不会把「取消 / 确认」挤出窗口。窗口高度在首帧测量后固定，
//! 避免阶段切换时窗口跳动，见 [`crate::app::InstallApp::fit_window`]。

use std::sync::atomic::Ordering;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputContentType};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::app::{InstallApp, PasswordUi, Phase};
use crate::model::DebFile;
use crate::util::{APP_NAME, format_bytes, format_kib};

/// 标题条高度。
pub(crate) const HEADER_HEIGHT: f32 = 38.0;
/// 内容区上留白。
pub(crate) const CONTENT_PADDING: f32 = 12.0;
/// 按钮栏高度：按钮 32 加底部留白 12。
pub(crate) const ACTION_BAR_HEIGHT: f32 = 44.0;
/// 内容之外还要留出的高度。
///
/// 一份是滚动条占掉的宽度：内容正好填满可视区时滚动条会冒出来，内容随之变窄、
/// 文字重新折行、变得更高，于是永远差一点显示不全。另一份是让人喘口气的余量，
/// 内容不贴着按钮栏。
pub(crate) const CONTENT_SLACK: f32 = 40.0;

pub(crate) fn show(
    app: &mut InstallApp,
    window: &mut Window,
    cx: &mut Context<InstallApp>,
) -> impl IntoElement {
    if let Some(PasswordUi { clear, focus, mask }) = app.take_password_ui() {
        app.password().update(cx, |state, cx| {
            if let Some(mask) = mask {
                state.set_masked(mask, window, cx);
            }
            if clear {
                state.set_value("", window, cx);
            }
            if focus {
                state.focus(window, cx);
            }
        });
    }

    let note = app.note().map(str::to_string);
    let file = app.file().cloned().map_err(str::to_string);
    let phase = app.phase().clone();
    let subtitle = match &file {
        Ok(file) => format!("安装 {}", file.metadata.name),
        Err(_) => "安装软件包".to_string(),
    };

    let title_bar = header(&subtitle, app.app_icon(), window, cx);

    // 内容与按钮分开返回：按钮栏钉在窗口底部，内容单独滚动。
    let (content, actions): (AnyElement, Option<AnyElement>) = match file {
        Ok(file) => match phase {
            Phase::Confirm => (confirm(&file, cx), Some(confirm_actions(app, cx))),
            Phase::Authorizing => (
                running(&file, "正在请求管理员授权…", note.as_deref(), cx),
                None,
            ),
            Phase::NeedPassword {
                message,
                prompt,
                echo,
                error,
            } => (
                need_password(
                    app,
                    &file,
                    &message,
                    &prompt,
                    echo,
                    error.as_deref(),
                    note.as_deref(),
                    cx,
                ),
                Some(password_actions(cx)),
            ),
            Phase::Installing => (
                running(&file, "授权成功，正在安装软件包…", note.as_deref(), cx),
                None,
            ),
            Phase::Done(result) => match result {
                Ok(message) => (success(&file, &message, cx), Some(close_actions(cx))),
                Err(error) => (
                    failure("安装失败", "安装包未能写入系统。", &error, cx),
                    Some(close_actions(cx)),
                ),
            },
        },
        Err(error) => (
            failure("无法安装安装包", "文件预检未通过。", &error, cx),
            Some(close_actions(cx)),
        ),
    };

    div()
        .id("install-window")
        .size_full()
        .bg(cx.theme().background)
        .text_color(cx.theme().foreground)
        .child(
            v_flex()
                .size_full()
                .child(title_bar)
                .child(
                    div()
                        .id("install-content-scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .track_scroll(&app.content_handle)
                        .px(px(16.0))
                        .pt(px(CONTENT_PADDING))
                        .child(content),
                )
                .children(
                    actions
                        .map(|actions| div().px(px(16.0)).pb(px(CONTENT_PADDING)).child(actions)),
                ),
        )
        .into_any_element()
}

/// 标题条：应用图标 + 名称 + 副标题，同时是窗口的拖动区。
///
/// 不画最小化 / 最大化 / 关闭：安装窗口只有「取消 / 确认」两个动作，
/// 多余的窗口按钮只会挤掉本来就不宽裕的内容区。
fn header(
    subtitle: &str,
    app_icon: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    window: &mut Window,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    // 按下鼠标只记下意图，等指针真的移动了再发起窗口移动 —— 单纯点一下
    // 标题条不该被当成拖动。做法与 gpui-kit 的 TitleBar 一致。
    let state = window.use_state(cx, |_, _| DragState { should_move: false });
    let theme = cx.theme();

    h_flex()
        .id("install-title-bar")
        .h(px(HEADER_HEIGHT))
        .flex_shrink_0()
        .items_center()
        .gap(px(8.0))
        .px(px(14.0))
        .border_b_1()
        .border_color(theme.title_bar_border)
        .bg(theme.title_bar)
        .children(app_icon.map(|icon| {
            img(icon)
                .w(px(18.0))
                .h(px(18.0))
                .rounded(px(4.5))
                .object_fit(ObjectFit::Cover)
        }))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(APP_NAME),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .truncate()
                .child(subtitle.to_string()),
        )
        .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| {
            state.should_move = false;
        }))
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| {
                state.should_move = true;
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| {
                state.should_move = false;
            }),
        )
        .on_mouse_move(window.listener_for(&state, |state, _, window, _| {
            if state.should_move {
                state.should_move = false;
                window.start_window_move();
            }
        }))
        .into_any_element()
}

/// 标题条的拖动状态。
struct DragState {
    should_move: bool,
}

impl Render for DragState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 确认页：包信息卡片 + 来源提示。
fn confirm(file: &DebFile, cx: &mut Context<InstallApp>) -> AnyElement {
    v_flex()
        .w_full()
        .gap(px(10.0))
        .child(package_card(file, cx))
        .child(notice(
            IconName::TriangleAlert,
            cx.theme().warning,
            "安装包来自本地文件，其维护脚本将以管理员身份执行。请确认文件来源可信。",
        ))
        .child(auth_hint(cx))
        .into_any_element()
}

/// 确认页的按钮。
fn confirm_actions(app: &InstallApp, cx: &mut Context<InstallApp>) -> AnyElement {
    let running_flag = app.running();
    let cancel = Button::new("install-cancel")
        .label("取消")
        .compact()
        .ghost()
        .on_click(cx.listener(move |_, _, _, cx| {
            // 安装进程还在跑时不允许关窗，否则会打断正在执行的 apt/dpkg。
            if !running_flag.load(Ordering::Relaxed) {
                cx.quit();
            }
        }));
    let install = Button::new("install-start")
        .label("安装")
        .compact()
        .primary()
        .on_click(cx.listener(|this, _, _, cx| this.start(cx)));

    action_bar().child(cancel).child(install).into_any_element()
}

/// 包信息卡片：图标、名称、摘要、版本标签、文件路径。
fn package_card(file: &DebFile, cx: &mut Context<InstallApp>) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let primary = theme.primary;
    let border = theme.border;
    let metadata = &file.metadata;
    let mono = theme.mono_font_family.clone();

    v_flex()
        .w_full()
        .gap(px(12.0))
        .p(px(14.0))
        .rounded(px(10.0))
        .bg(theme.muted)
        .border_1()
        .border_color(border)
        .child(
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
                ),
        )
        .child(
            h_flex()
                .gap(px(6.0))
                .flex_wrap()
                .child(chip(
                    metadata.version.clone(),
                    primary,
                    primary.opacity(0.12),
                    None,
                ))
                .child(chip(
                    metadata.arch.clone(),
                    muted,
                    theme.background,
                    Some(border),
                ))
                .child(chip(
                    format_bytes(file.file_size),
                    muted,
                    theme.background,
                    Some(border),
                ))
                .child(chip(
                    format!("安装后 {}", format_kib(metadata.installed_size_kb)),
                    muted,
                    theme.background,
                    Some(border),
                )),
        )
        .child(div().h(px(1.0)).bg(border))
        .child(
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
                        .child(file.path.display().to_string()),
                ),
        )
        .into_any_element()
}

/// 运行中：居中状态 + 旋转指示。
fn running(
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
        .child(
            div()
                .size(px(56.0))
                .rounded_full()
                .bg(primary.opacity(0.12))
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new().with_size(px(28.0)).color(primary)),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_center()
                .child(format!("正在安装 {}", file.metadata.name)),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .text_center()
                .child(status.to_string()),
        )
        .children(note.map(|note| {
            div()
                .w_full()
                .child(notice(IconName::Info, cx.theme().info, note))
        }))
        .child(
            h_flex()
                .items_center()
                .gap(px(6.0))
                .child(Icon::new(IconName::ShieldCheck).xsmall().text_color(muted))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("密码只在本窗口输入，不会保存。"),
                ),
        )
        .into_any_element()
}

/// 内嵌授权：polkit 说明 + 密码输入卡片 + 重试错误。
#[allow(clippy::too_many_arguments)]
fn need_password(
    app: &mut InstallApp,
    file: &DebFile,
    message: &str,
    prompt: &str,
    echo: bool,
    error: Option<&str>,
    note: Option<&str>,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let primary = theme.primary;
    let message = if message.is_empty() {
        "需要管理员授权"
    } else {
        message
    };
    let prompt = prompt.trim();
    let prompt = if prompt.is_empty() {
        if echo {
            "请输入请求的内容"
        } else {
            "密码"
        }
    } else {
        prompt
    };

    v_flex()
        .w_full()
        .gap(px(10.0))
        .child(
            h_flex()
                .items_start()
                .gap(px(12.0))
                .child(icon_tile(IconName::LockKeyhole, primary, 44.0, false))
                .child(
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
                                .line_clamp(3)
                                .child(message.to_string()),
                        ),
                ),
        )
        .child(password_card(app, file, prompt, error, note, cx))
        .child(
            h_flex()
                .items_center()
                .gap(px(6.0))
                .child(Icon::new(IconName::ShieldCheck).xsmall().text_color(muted))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .whitespace_normal()
                        .child("密码通过系统 polkit 校验，输入后立即清除，不会被保存。"),
                ),
        )
        .into_any_element()
}

/// 授权页的按钮。
fn password_actions(cx: &mut Context<InstallApp>) -> AnyElement {
    let cancel = Button::new("install-cancel-auth")
        .label("取消")
        .compact()
        .ghost()
        .on_click(cx.listener(|this, _, _, cx| this.cancel_auth(cx)));
    let submit = Button::new("install-submit-password")
        .label("确认")
        .compact()
        .primary()
        .on_click(cx.listener(|this, _, _, cx| this.submit_password(cx)));

    action_bar().child(cancel).child(submit).into_any_element()
}

/// 密码输入卡片：提示语、输入框与重试错误。
fn password_card(
    app: &mut InstallApp,
    file: &DebFile,
    prompt: &str,
    error: Option<&str>,
    note: Option<&str>,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let border = theme.border;

    v_flex()
        .w_full()
        .gap(px(8.0))
        .p(px(12.0))
        .rounded(px(10.0))
        .bg(theme.muted)
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .gap(px(10.0))
                .child(div().text_xs().text_color(muted).child(prompt.to_string()))
                .child(chip(
                    format!("安装 {}", file.metadata.name),
                    muted,
                    theme.background,
                    Some(border),
                )),
        )
        // 显式声明为密码框：掩码负责遮住字符，content_type 负责让
        // 辅助技术按密码输入框播报，并且不把内容读出去。
        .child(
            Input::new(app.password())
                .w_full()
                .content_type(InputContentType::Password),
        )
        .children(error.map(|error| notice(IconName::CircleAlert, theme.danger, error)))
        .children(note.map(|note| notice(IconName::Info, theme.info, note)))
        .into_any_element()
}

/// 成功：结果说明。
fn success(file: &DebFile, message: &str, cx: &mut Context<InstallApp>) -> AnyElement {
    let success = cx.theme().success;
    let muted = cx.theme().muted_foreground;
    v_flex()
        .w_full()
        .items_center()
        .gap(px(10.0))
        .child(icon_tile(IconName::CircleCheck, success, 56.0, true))
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child("安装完成"),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .text_center()
                .whitespace_normal()
                .child(message.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("{} 现在可以从应用菜单启动。", file.metadata.name)),
        )
        .into_any_element()
}

/// 失败/无法安装：错误详情卡片。
fn failure(title: &str, subtitle: &str, message: &str, cx: &mut Context<InstallApp>) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let mono = theme.mono_font_family.clone();
    v_flex()
        .w_full()
        .gap(px(10.0))
        .child(
            h_flex()
                .items_start()
                .gap(px(12.0))
                .child(icon_tile(IconName::CircleX, theme.danger, 44.0, false))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title.to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .whitespace_normal()
                                .child(subtitle.to_string()),
                        ),
                ),
        )
        .child(
            div()
                .id("install-error")
                .min_h(px(60.0))
                .max_h(px(160.0))
                .overflow_y_scroll()
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(theme.muted)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_xs()
                        .font_family(mono)
                        .whitespace_normal()
                        .child(message.to_string()),
                ),
        )
        .into_any_element()
}

/// 结束后的按钮。
fn close_actions(cx: &mut Context<InstallApp>) -> AnyElement {
    let close = Button::new("install-close")
        .label("关闭")
        .compact()
        .primary()
        .on_click(cx.listener(|_, _, _, cx| cx.quit()));
    action_bar().child(close).into_any_element()
}

/// 底部操作栏：右对齐的按钮行。
fn action_bar() -> Div {
    h_flex().w_full().justify_end().items_center().gap(px(8.0))
}

/// 彩色小提示条。
fn notice(icon: IconName, color: Hsla, text: &str) -> AnyElement {
    h_flex()
        .w_full()
        .items_start()
        .gap(px(8.0))
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(color.opacity(0.08))
        .border_1()
        .border_color(color.opacity(0.28))
        .child(Icon::new(icon).small().text_color(color))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_xs()
                .text_color(color)
                .whitespace_normal()
                .child(text.to_string()),
        )
        .into_any_element()
}

/// 安装需要管理员授权的轻提示，不抢视觉焦点。
fn auth_hint(cx: &Context<InstallApp>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .items_center()
        .gap(px(6.0))
        .child(Icon::new(IconName::LockKeyhole).xsmall().text_color(muted))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .whitespace_normal()
                .child("安装需要管理员授权，密码会在本窗口内输入。"),
        )
        .into_any_element()
}

/// 圆形/圆角图标底座。`round` 为 true 时画成圆形（用于状态结果）。
fn icon_tile(icon: IconName, color: Hsla, size: f32, round: bool) -> AnyElement {
    let radius = if round { size } else { size * 0.28 };
    div()
        .size(px(size))
        .rounded(px(radius))
        .bg(color.opacity(0.12))
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).with_size(px(size * 0.5)).text_color(color))
        .into_any_element()
}

/// 小圆角标签：版本、架构、大小等。
fn chip(text: String, fg: Hsla, bg: Hsla, border: Option<Hsla>) -> AnyElement {
    let mut chip = h_flex()
        .px(px(8.0))
        .py(px(2.0))
        .rounded_full()
        .bg(bg)
        .child(div().text_xs().text_color(fg).child(text));
    if let Some(border) = border {
        chip = chip.border_1().border_color(border);
    }
    chip.into_any_element()
}
