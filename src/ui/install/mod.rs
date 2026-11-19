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
mod content;
mod result;

use std::sync::Arc;

use gpui_kit::RenderImage;
use gpui_kit::component::input::InputState;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;

use crate::app::{InstallApp, PasswordUi, Phase};
use crate::model::DebFile;
use crate::ui::APP_ICON_SIZE;
use crate::util::APP_NAME;

use actions::{close_actions, confirm_actions, password_actions};
use content::{PasswordView, confirm, need_password, running};
use result::{failure, success};

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

/// 绘制安装窗口。
pub(crate) fn show(
    app: &mut InstallApp,
    window: &mut Window,
    cx: &mut Context<InstallApp>,
) -> impl IntoElement {
    apply_password_ui(app, window, cx);
    let note = app.note().map(str::to_string);
    let file = app.file().cloned().map_err(str::to_string);
    let phase = app.phase().clone();
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
            content,
            actions,
        },
        cx,
    )
}

fn apply_password_ui(app: &mut InstallApp, window: &mut Window, cx: &mut Context<InstallApp>) {
    if let Some(PasswordUi { clear, focus, mask }) = app.take_password_ui() {
        app.password().update(cx, |state, cx| {
            update_password_state(state, &PasswordUi { clear, focus, mask }, window, cx);
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
    file: &Result<DebFile, String>,
    app_icon: Option<Arc<RenderImage>>,
    window: &mut Window,
    cx: &mut Context<InstallApp>,
) -> AnyElement {
    let subtitle = match file {
        Ok(file) => format!("安装 {}", file.metadata.name),
        Err(_) => "安装软件包".to_string(),
    };
    header(&subtitle, app_icon, window, cx)
}

struct ContentSpec<'a> {
    app: &'a mut InstallApp,
    file: &'a Result<DebFile, String>,
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
        Ok(file) => render_phase(
            PhaseSpec {
                app,
                file,
                phase,
                note,
            },
            cx,
        ),
        Err(error) => (
            failure("无法安装安装包", "文件预检未通过。", error, cx),
            Some(close_actions(cx)),
        ),
    }
}

struct PhaseSpec<'a> {
    app: &'a mut InstallApp,
    file: &'a DebFile,
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
        phase,
        note,
    } = spec;
    match phase {
        Phase::Confirm => (confirm(file, cx), Some(confirm_actions(app, cx))),
        Phase::Authorizing => (running(file, "正在请求管理员授权…", note, cx), None),
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
        Phase::Installing => (running(file, "授权成功，正在安装软件包…", note, cx), None),
        Phase::Done(Ok(message)) => (success(file, &message, cx), Some(close_actions(cx))),
        Phase::Done(Err(error)) => (
            failure("安装失败", "安装包未能写入系统。", &error, cx),
            Some(close_actions(cx)),
        ),
    }
}

struct WindowShell<'a> {
    app: &'a InstallApp,
    title_bar: AnyElement,
    content: AnyElement,
    actions: Option<AnyElement>,
}

fn window_shell(shell: WindowShell<'_>, cx: &mut Context<InstallApp>) -> AnyElement {
    let WindowShell {
        app,
        title_bar,
        content,
        actions,
    } = shell;
    v_flex()
        .id("install-window")
        .size_full()
        .bg(cx.theme().background)
        .text_color(cx.theme().foreground)
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
