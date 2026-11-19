//! 独立安装窗口的绘制：确认 → 内嵌授权 → 安装 → 结果。
//!
//! 视觉上沿用主界面的 Ubuntu 橙主题：灰色卡片承载包信息，状态用图标 +
//! 色彩表达，底部固定操作栏。密码框属于这个窗口本身，不再调用桌面自带的
//! polkit 对话框。
//!
//! 窗口刻意做小：它只回答一个问题「这个包要不要装」。因此标题条不放最小化、
//! 最大化、关闭按钮，唯一的出口是内容区里的「取消 / 确认」。
//!
//! 布局上分三段：标题条、可滚动的内容区、贴底的按钮栏。按钮栏不参与滚动，
//! 所以内容再长也不会把「取消 / 确认」挤出窗口。窗口高度在首帧测量后固定，
//! 避免阶段切换时窗口跳动，见 [`crate::app::InstallApp::fit_window`]。

mod actions;
mod auth;
mod content;
mod progress;
mod result;

use std::sync::Arc;

use gpui_kit::RenderImage;
use gpui_kit::component::input::InputState;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::app::{InstallApp, InstallFile, PasswordUi, Phase};
use crate::model::DebFile;
use crate::runtime::install::InstalledStatus;
use crate::ui::APP_ICON_SIZE;
use crate::util::APP_NAME;

use actions::{close_actions, confirm_actions, password_actions};
use content::{PasswordView, confirm, need_password, running_auth, running_install};
use progress::step_indicator;
use result::{failure, success};

/// 标题条高度。
pub(crate) const HEADER_HEIGHT: f32 = 38.0;
/// 顶部步骤条高度（含上下内边距）。
pub(crate) const STEP_BAR_HEIGHT: f32 = 30.0;
/// 内容区上留白。
pub(crate) const CONTENT_PADDING: f32 = 12.0;
/// 按钮栏高度：按钮 32 加底部留白 12。
pub(crate) const ACTION_BAR_HEIGHT: f32 = 44.0;
/// 内容之外还要留出的紧凑余量（避免紧贴按钮栏或因行高/边框误差触发滚动条）。
pub(crate) const CONTENT_SLACK: f32 = 24.0;

/// 绘制安装窗口。
pub(crate) fn show(
    app: &mut InstallApp,
    window: &mut Window,
    cx: &mut Context<InstallApp>,
) -> impl IntoElement {
    apply_password_ui(app, window, cx);
    let note = app.note().map(str::to_string);
    let file = app.file().clone();
    let phase = app.phase().clone();
    let step_bar = build_step_bar(&file, &phase, cx);
    let center_content = matches!(
        phase,
        Phase::Authorizing | Phase::Installing | Phase::Done(Ok(_))
    );
    let title_bar = build_header(&file, app.app_icon(), window, cx);
    let (content, actions) = render_content(
        ContentSpec {
            app,
            file: &file,
            phase,
            note: note.as_deref(),
        },
        cx,
    );
    window_shell(
        WindowShell {
            app,
            title_bar,
            step_bar,
            content,
            actions,
            center_content,
        },
        cx,
    )
}

fn build_step_bar(
    file: &InstallFile,
    phase: &Phase,
    cx: &Context<InstallApp>,
) -> Option<AnyElement> {
    if !matches!(file, InstallFile::Ready { .. }) {
        return None;
    }
    let active = match phase {
        Phase::Confirm => 0,
        Phase::Authorizing | Phase::NeedPassword { .. } => 1,
        Phase::Installing => 2,
        Phase::Done(Ok(_)) => 3,
        Phase::Done(Err(_)) => return None,
    };
    Some(
        div()
            .px(px(16.0))
            .pt(px(10.0))
            .child(step_indicator(active, cx))
            .into_any_element(),
    )
}

fn apply_password_ui(app: &mut InstallApp, window: &mut Window, cx: &mut Context<InstallApp>) {
    if let Some(PasswordUi { clear, focus, mask }) = app.take_password_ui() {
        let is_secret = mask.unwrap_or(true);
        app.secret_password().update(cx, |state, cx| {
            if clear {
                state.clear(cx);
            }
            if focus && is_secret {
                state.focus(window, cx);
            }
        });
        app.password().update(cx, |state, cx| {
            update_password_state(
                state,
                &PasswordUi {
                    clear,
                    focus: focus && !is_secret,
                    mask,
                },
                window,
                cx,
            );
        });
    }
}

fn update_password_state(
    state: &mut InputState,
    update: &PasswordUi,
    window: &mut Window,
    cx: &mut Context<InputState>,
) {
    if let Some(mask) = update.mask {
        state.set_masked(mask, window, cx);
    }
    if update.clear {
        state.set_value("", window, cx);
    }
    if update.focus {
        state.focus(window, cx);
    }
}

fn build_header(
    file: &InstallFile,
    app_icon: Option<Arc<RenderImage>>,
    window: &mut Window,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let subtitle = match file {
        InstallFile::Ready { file, .. } => format!("安装 {}", file.metadata.name),
        InstallFile::Inspecting | InstallFile::Failed(_) => "安装软件包".to_string(),
    };
    header(&subtitle, app_icon, window, cx)
}

struct ContentSpec<'a> {
    app: &'a mut InstallApp,
    file: &'a InstallFile,
    phase: Phase,
    note: Option<&'a str>,
}

fn render_content(
    spec: ContentSpec<'_>,
    cx: &mut Context<InstallApp>,
) -> (AnyElement, Option<AnyElement>) {
    let ContentSpec {
        app,
        file,
        phase,
        note,
    } = spec;
    match file {
        InstallFile::Inspecting => (inspecting(cx), Some(close_actions(app, cx))),
        InstallFile::Ready { file, installed } => render_phase(
            PhaseSpec {
                app,
                file,
                installed,
                phase,
                note,
            },
            cx,
        ),
        InstallFile::Failed(error) => (
            failure("无法安装安装包", "文件预检未通过。", error, cx),
            Some(close_actions(app, cx)),
        ),
    }
}

fn inspecting(cx: &Context<InstallApp>) -> AnyElement {
    h_flex()
        .w_full()
        .py(px(28.0))
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(Spinner::new().small())
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("正在解析安装包信息…"),
        )
        .into_any_element()
}

struct PhaseSpec<'a> {
    app: &'a mut InstallApp,
    file: &'a DebFile,
    installed: &'a InstalledStatus,
    phase: Phase,
    note: Option<&'a str>,
}

fn render_phase(
    spec: PhaseSpec<'_>,
    cx: &mut Context<InstallApp>,
) -> (AnyElement, Option<AnyElement>) {
    let PhaseSpec {
        app,
        file,
        installed,
        phase,
        note,
    } = spec;
    match phase {
        Phase::Confirm => (confirm(file, installed, cx), Some(confirm_actions(app, cx))),
        Phase::Authorizing => (running_auth(file, "正在请求管理员授权…", note, cx), None),
        Phase::NeedPassword {
            message,
            prompt,
            echo,
            error,
        } => {
            let view = PasswordView {
                message: &message,
                prompt: &prompt,
                echo,
                error: error.as_deref(),
                note,
            };
            (
                need_password(app, file, view, cx),
                Some(password_actions(cx)),
            )
        }
        Phase::Installing => (running_install(app, file, note, cx), None),
        Phase::Done(Ok(message)) => {
            let has_launcher = app.launch_entry().is_some();
            (
                success(file, &message, has_launcher, cx),
                Some(close_actions(app, cx)),
            )
        }
        Phase::Done(Err(error)) => (
            failure("安装失败", "安装包未能写入系统。", &error, cx),
            Some(close_actions(app, cx)),
        ),
    }
}

struct WindowShell<'a> {
    app: &'a InstallApp,
    title_bar: AnyElement,
    step_bar: Option<AnyElement>,
    content: AnyElement,
    actions: Option<AnyElement>,
    center_content: bool,
}

fn window_shell(shell: WindowShell<'_>, cx: &mut Context<InstallApp>) -> AnyElement {
    let WindowShell {
        app,
        title_bar,
        step_bar,
        content,
        actions,
        center_content,
    } = shell;
    let mut inner = v_flex().w_full().child(content);
    if center_content {
        inner = inner.min_h_full().justify_center().pb(px(CONTENT_PADDING));
    }
    v_flex()
        .id("install-window")
        .size_full()
        .bg(cx.theme().background)
        .text_color(cx.theme().foreground)
        .child(title_bar)
        .children(step_bar)
        .child(
            div()
                .id("install-content-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .track_scroll(&app.content_handle)
                .px(px(16.0))
                .pt(px(CONTENT_PADDING))
                .child(inner),
        )
        .children(actions.map(|actions| div().px(px(16.0)).pb(px(CONTENT_PADDING)).child(actions)))
        .into_any_element()
}

/// 标题条：应用图标 + 名称 + 副标题，同时是窗口的拖动区。
///
/// 不画最小化 / 最大化 / 关闭：安装窗口只有「取消 / 确认」两个动作，
/// 多余的窗口按钮只会挤掉本来就不宽裕的内容区。
fn header(
    subtitle: &str,
    app_icon: Option<Arc<RenderImage>>,
    window: &mut Window,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let state = window.use_state(cx, |_, _| DragState { should_move: false });
    let theme = cx.theme();
    let bar = h_flex()
        .id("install-title-bar")
        .h(px(HEADER_HEIGHT))
        .flex_shrink_0()
        .items_center()
        .gap(px(8.0))
        .px(px(14.0))
        .border_b_1()
        .border_color(theme.title_bar_border)
        .bg(theme.title_bar)
        .children(app_icon.map(header_icon))
        .child(title_identity(theme.muted_foreground, subtitle));
    attach_drag_handlers(bar, &state, window)
}

fn header_icon(icon: Arc<RenderImage>) -> AnyElement {
    img(icon)
        .w(px(APP_ICON_SIZE))
        .h(px(APP_ICON_SIZE))
        .rounded(px(6.0))
        .object_fit(ObjectFit::Cover)
        .into_any_element()
}

fn title_identity(muted: Hsla, subtitle: &str) -> AnyElement {
    h_flex()
        .flex_1()
        .min_w(px(0.0))
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(APP_NAME),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child(subtitle.to_string()),
        )
        .into_any_element()
}

fn attach_drag_handlers(
    bar: Stateful<Div>,
    state: &Entity<DragState>,
    window: &mut Window,
) -> AnyElement {
    // 按下鼠标只记下意图，等指针真的移动了再发起窗口移动；单纯点一下标题条
    // 不应被当成拖动。做法与 gpui-kit 的 TitleBar 一致。
    bar.on_mouse_down_out(window.listener_for(state, |state, _, _, _| {
        state.should_move = false;
    }))
    .on_mouse_down(
        MouseButton::Left,
        window.listener_for(state, |state, _, _, _| {
            state.should_move = true;
        }),
    )
    .on_mouse_up(
        MouseButton::Left,
        window.listener_for(state, |state, _, _, _| {
            state.should_move = false;
        }),
    )
    .on_mouse_move(window.listener_for(state, |state, _, window, _| {
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
