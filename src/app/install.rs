//! 独立安装窗口：双击 `.deb` 时只显示这个最小界面，不打开主窗口。
//!
//! 安装进程刻意不抢主实例锁：主窗口可以与安装窗口同时存在，安装结束后主窗口
//! 会通过 dpkg 状态监视自动刷新。文件管理器按 `Exec=debdeck %f` 为每个选中
//! 文件单独启动一次，因此这里只处理命令行里的第一个 `.deb`。
//!
//! 管理员认证由 [`crate::backend::polkit_agent`] 内嵌完成：密码输入、重试、
//! 取消都留在同一个窗口里，不再弹系统认证框。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Pixels, Render, RenderImage, ScrollHandle,
    Window, px, size,
};

use crate::backend::deb_file;
use crate::backend::polkit_agent::{self, AuthResponse, InstallEvent, InstallSession};
use crate::model::DebFile;
use crate::ui::install::{ACTION_BAR_HEIGHT, CONTENT_PADDING, CONTENT_SLACK, HEADER_HEIGHT};

/// 安装窗口的最小高度。进行中和完成态的内容量不同，若不设下限，同一流程的
/// 窗口会在阶段切换时明显变矮；对齐尺寸比逐像素贴内容更稳定，也更耐看。
pub(super) const MIN_WINDOW_HEIGHT: f32 = 300.0;

/// 安装窗口的流程。
#[derive(Clone)]
pub(crate) enum Phase {
    /// 展示包信息，等待用户点「安装」。
    Confirm,
    /// 已启动 pkexec，等待 PAM 提示（或直接授权成功）。
    Authorizing,
    /// PAM 要求输入，密码框就在窗口里。
    NeedPassword {
        message: String,
        prompt: String,
        echo: bool,
        error: Option<String>,
    },
    /// 授权成功，apt-get 正在执行。
    Installing,
    Done(Result<String, String>),
}

/// 关闭按钮读取：授权/安装进行中不允许关窗，避免中断 apt/dpkg。
pub(crate) struct InstallApp {
    file: Result<DebFile, String>,
    phase: Phase,
    app_icon: Option<Arc<RenderImage>>,
    running: Arc<AtomicBool>,
    password: Entity<InputState>,
    auth_tx: Option<async_channel::Sender<AuthResponse>>,
    note: Option<String>,
    clear_password: bool,
    focus_password: bool,
    mask: Option<bool>,
    /// 首帧确定后的稳定窗口高度。平台的 resize 是异步的，不能靠读窗口实际尺寸
    /// 判断是否还需要调整；这里也借此避免安装阶段切换时反复改尺寸。
    window_height: Pixels,
    /// 内容区的滚动句柄：内容溢出时由它记录边界，窗口据此量出内容高度。
    pub(crate) content_handle: ScrollHandle,
}

/// 渲染时需要一次性应用的密码框操作。
pub(crate) struct PasswordUi {
    pub(crate) clear: bool,
    pub(crate) focus: bool,
    pub(crate) mask: Option<bool>,
}

impl InstallApp {
    pub(crate) fn new(path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file = deb_file::inspect(path).map_err(|error| format!("{error:#}"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("密码"));
        cx.subscribe(&password, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_password(cx);
            }
        })
        .detach();
        Self {
            file,
            phase: Phase::Confirm,
            app_icon: crate::ui::app_icon_texture(),
            running: Arc::new(AtomicBool::new(false)),
            password,
            auth_tx: None,
            note: None,
            clear_password: false,
            focus_password: false,
            mask: None,
            window_height: px(0.0),
            content_handle: ScrollHandle::default(),
        }
    }

    /// 首次布局后固定窗口高度。
    ///
    /// 高度取自首屏确认页的实测边界，并保留窗口初始高度，避免点击安装后在同一
    /// 流程中突然缩矮。后续阶段内容不同也沿用这个高度，超出时由内容区滚动。
    fn fit_window(&mut self, window: &mut Window) {
        if self.window_height > px(0.0) {
            return;
        }
        let Some(bounds) = self.content_handle.bounds_for_item(0) else {
            return;
        };
        let mut chrome = HEADER_HEIGHT + CONTENT_PADDING;
        if self.has_actions() {
            chrome += ACTION_BAR_HEIGHT;
        }
        let viewport = window.viewport_size();
        let desired = (bounds.size.height + px(chrome + CONTENT_SLACK))
            .max(px(MIN_WINDOW_HEIGHT))
            .max(viewport.height);
        self.window_height = desired;
        if (viewport.height - desired).abs() >= px(0.5) {
            window.resize(size(viewport.width, desired));
        }
    }

    /// 这一阶段有没有底部按钮栏。没有的话窗口就不必为它留出高度。
    fn has_actions(&self) -> bool {
        !matches!(self.phase, Phase::Authorizing | Phase::Installing)
    }

    pub(crate) fn file(&self) -> Result<&DebFile, &str> {
        self.file.as_ref().map_err(String::as_str)
    }

    pub(crate) fn phase(&self) -> &Phase {
        &self.phase
    }

    pub(crate) fn app_icon(&self) -> Option<Arc<RenderImage>> {
        self.app_icon.clone()
    }

    pub(crate) fn password(&self) -> &Entity<InputState> {
        &self.password
    }

    pub(crate) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub(crate) fn running(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }

    /// 取走一次性的密码框操作（清空/聚焦/切换掩码）。
    pub(crate) fn take_password_ui(&mut self) -> Option<PasswordUi> {
        if !self.clear_password && !self.focus_password && self.mask.is_none() {
            return None;
        }
        let request = PasswordUi {
            clear: self.clear_password,
            focus: self.focus_password,
            mask: self.mask.take(),
        };
        self.clear_password = false;
        self.focus_password = false;
        Some(request)
    }

    /// 用户确认后启动后台安装会话。
    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        let Ok(file) = &self.file else { return };
        if !matches!(self.phase, Phase::Confirm) {
            return;
        }
        let path = file.path.clone();
        let name = file.metadata.name.clone();
        tracing::info!("启动安装会话：{name}");
        self.phase = Phase::Authorizing;
        self.running.store(true, Ordering::Relaxed);
        self.note = None;

        match polkit_agent::start(&path, name) {
            Ok(session) => {
                let InstallSession {
                    events, responses, ..
                } = session;
                self.auth_tx = Some(responses);
                cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv().await {
                        if this
                            .update(cx, |app, cx| app.apply_install_event(event, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => {
                self.phase = Phase::Done(Err(format!("{error:#}")));
                self.running.store(false, Ordering::Relaxed);
            }
        }
        cx.notify();
    }

    fn apply_install_event(&mut self, event: InstallEvent, cx: &mut Context<Self>) {
        match event {
            InstallEvent::Notice(text) => self.note = Some(text),
            InstallEvent::Prompt(prompt) => {
                self.phase = Phase::NeedPassword {
                    message: prompt.message,
                    prompt: prompt.prompt,
                    echo: prompt.echo,
                    error: prompt.error,
                };
                self.clear_password = true;
                self.focus_password = true;
                self.mask = Some(!prompt.echo);
            }
            InstallEvent::Authorized => {
                self.phase = Phase::Installing;
                self.note = None;
            }
            InstallEvent::Finished(result) => {
                self.phase = Phase::Done(result);
                self.auth_tx = None;
                self.running.store(false, Ordering::Relaxed);
            }
        }
        cx.notify();
    }

    /// 提交密码（Enter 与「确认」按钮共用）。
    pub(crate) fn submit_password(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::NeedPassword { .. }) {
            return;
        }
        let value = self.password.read(cx).value().to_string();
        if let Some(tx) = &self.auth_tx {
            let _ = tx.try_send(AuthResponse::Input(value));
        }
        self.phase = Phase::Authorizing;
        self.clear_password = true;
        self.focus_password = false;
        self.note = None;
        cx.notify();
    }

    /// 用户主动取消授权。
    pub(crate) fn cancel_auth(&mut self, cx: &mut Context<Self>) {
        if let Some(tx) = &self.auth_tx {
            let _ = tx.try_send(AuthResponse::Cancel);
        }
        self.auth_tx = None;
        self.phase = Phase::Done(Err("已取消安装。".to_string()));
        self.running.store(false, Ordering::Relaxed);
        cx.notify();
    }
}

impl Render for InstallApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fit_window(window);
        crate::ui::install::show(self, window, cx)
    }
}
