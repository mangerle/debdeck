//! 独立安装窗口：双击 `.deb` 时只显示这个最小界面，不打开主窗口。
//!
//! 安装进程刻意不抢主实例锁：主窗口可以与安装窗口同时存在，安装结束后主窗口
//! 会通过 dpkg 状态监视自动刷新。文件管理器按 `Exec=debdeck %f` 为每个选中
//! 文件单独启动一次，因此这里只处理命令行里的第一个 `.deb`。
//!
//! 管理员认证与包预检均通过 [`crate::runtime::install`] 交给后台执行：
//! 密码输入、重试、取消都留在同一个窗口里，且不阻塞主线程打开窗口。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Pixels, Render, RenderImage, ScrollHandle,
    Window, px, size,
};

use crate::app::{spawn_background, spawn_event_loop};
use crate::model::{DebFile, DesktopEntry};
use crate::runtime::install::{
    AuthResponse, InstallEvent, InstallSession, InstalledStatus, find_launchable_desktop,
    inspect_deb_file, launch_installed, start_install_session,
};
use crate::ui::install::{
    ACTION_BAR_HEIGHT, CONTENT_PADDING, CONTENT_SLACK, HEADER_HEIGHT, STEP_BAR_HEIGHT,
};
use crate::ui::password_input::{PasswordInputEvent, PasswordInputState};

/// 安装窗口的最小高度，与初始窗口高度保持一致以避免阶段切换时窗口跳动闪烁。
pub(super) const MIN_WINDOW_HEIGHT: f32 = 410.0;

/// 保留在安装日志面板中的最大行数。
const MAX_INSTALL_LOGS: usize = 200;

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

/// 安装过程中的实时阶段进度。
#[derive(Clone, Debug)]
pub(crate) struct InstallStage {
    /// 给用户看的阶段描述。
    pub(crate) label: &'static str,
    /// 估算进度（`0.0..=1.0`）。
    pub(crate) progress: f32,
}

impl Default for InstallStage {
    fn default() -> Self {
        Self {
            label: "授权成功，正在准备安装…",
            progress: 0.15,
        }
    }
}

/// 安装包预检状态。
#[derive(Clone)]
pub(crate) enum InstallFile {
    /// 正在后台解析 `.deb` 元数据。
    Inspecting,
    /// 预检通过。
    Ready {
        file: Box<DebFile>,
        installed: InstalledStatus,
    },
    /// 预检失败。
    Failed(String),
}

/// 详情抽屉的当前展开目标（互斥状态，避免多个布尔标志）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailDrawer {
    None,
    Logs,
    AuthCommand,
}

/// 关闭按钮读取：授权/安装进行中不允许关窗，避免中断 apt/dpkg。
pub(crate) struct InstallApp {
    file: InstallFile,
    phase: Phase,
    app_icon: Option<Arc<RenderImage>>,
    running: Arc<AtomicBool>,
    /// 标准密码输入（不触发 IME、大圆点掩码、显隐切换）。
    secret_password: Entity<PasswordInputState>,
    /// 明文输入回退（仅当 PAM `echo == true` 时使用）。
    password: Entity<InputState>,
    auth_tx: Option<async_channel::Sender<AuthResponse>>,
    /// 持有后台控制器句柄，窗口关闭或安装结束时等待线程安全收尾。
    session: Option<InstallSession>,
    note: Option<String>,
    install_stage: InstallStage,
    logs: Vec<String>,
    drawer: DetailDrawer,
    launch_entry: Option<DesktopEntry>,
    clear_password: bool,
    focus_password: bool,
    mask: Option<bool>,
    /// 上一次实际应用的目标窗口高度，配合 `resize_budget` 防止异步 resize 抖动。
    last_desired_height: Option<Pixels>,
    resize_budget: u8,
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
        let deb_path = path.to_path_buf();
        let secret_password = cx.new(PasswordInputState::new);
        cx.subscribe(&secret_password, |this, _, event, cx| match event {
            PasswordInputEvent::Submit => this.submit_password(cx),
        })
        .detach();

        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("请输入")
        });
        cx.subscribe(&password, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_password(cx);
            }
        })
        .detach();

        spawn_background(
            cx,
            move || inspect_deb_file(&deb_path),
            |app, result, cx| {
                app.file = match result {
                    Ok(inspected) => InstallFile::Ready {
                        file: Box::new(inspected.file),
                        installed: inspected.installed,
                    },
                    Err(error) => InstallFile::Failed(error),
                };
                // 预检结果就绪后开放高度自适应预算。
                app.reset_window_fit();
                cx.notify();
            },
        );
        Self {
            file: InstallFile::Inspecting,
            phase: Phase::Confirm,
            app_icon: crate::ui::app_icon_texture(),
            running: Arc::new(AtomicBool::new(false)),
            secret_password,
            password,
            auth_tx: None,
            session: None,
            note: None,
            install_stage: InstallStage::default(),
            logs: Vec::new(),
            drawer: DetailDrawer::None,
            launch_entry: None,
            clear_password: false,
            focus_password: false,
            mask: None,
            last_desired_height: None,
            resize_budget: 0,
            content_handle: ScrollHandle::default(),
        }
    }

    fn reset_window_fit(&mut self) {
        self.last_desired_height = None;
        self.resize_budget = 4;
    }

    /// 仅在当前内容确实超出视口时单向扩高窗口（只扩不缩，上限 `620px`），避免阶段切换闪烁。
    fn fit_window(&mut self, window: &mut Window) {
        if matches!(self.file, InstallFile::Inspecting) || self.resize_budget == 0 {
            return;
        }
        let Some(bounds) = self.content_handle.bounds_for_item(0) else {
            return;
        };
        let mut chrome = HEADER_HEIGHT + CONTENT_PADDING;
        if self.has_step_bar() {
            chrome += STEP_BAR_HEIGHT;
        }
        if self.has_actions() {
            chrome += ACTION_BAR_HEIGHT;
        }
        let viewport = window.viewport_size();
        let desired = (bounds.size.height + px(chrome + CONTENT_SLACK))
            .max(viewport.height)
            .clamp(px(MIN_WINDOW_HEIGHT), px(620.0));
        if self
            .last_desired_height
            .is_some_and(|prev| (prev - desired).abs() < px(2.0))
        {
            return;
        }
        self.last_desired_height = Some(desired);
        self.resize_budget -= 1;
        if desired - viewport.height >= px(2.0) {
            window.resize(size(viewport.width, desired));
        }
    }

    fn has_step_bar(&self) -> bool {
        matches!(self.file, InstallFile::Ready { .. }) && !matches!(self.phase, Phase::Done(Err(_)))
    }

    /// 这一阶段有没有底部按钮栏。没有的话窗口就不必为它留出高度。
    fn has_actions(&self) -> bool {
        !matches!(self.phase, Phase::Authorizing | Phase::Installing)
    }

    pub(crate) fn file(&self) -> &InstallFile {
        &self.file
    }

    pub(crate) fn phase(&self) -> &Phase {
        &self.phase
    }

    pub(crate) fn app_icon(&self) -> Option<Arc<RenderImage>> {
        self.app_icon.clone()
    }

    pub(crate) fn secret_password(&self) -> &Entity<PasswordInputState> {
        &self.secret_password
    }

    pub(crate) fn password(&self) -> &Entity<InputState> {
        &self.password
    }

    pub(crate) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub(crate) fn install_stage(&self) -> &InstallStage {
        &self.install_stage
    }

    pub(crate) fn logs(&self) -> &[String] {
        &self.logs
    }

    pub(crate) fn show_logs(&self) -> bool {
        self.drawer == DetailDrawer::Logs
    }

    pub(crate) fn show_auth_command(&self) -> bool {
        self.drawer == DetailDrawer::AuthCommand
    }

    pub(crate) fn launch_entry(&self) -> Option<&DesktopEntry> {
        self.launch_entry.as_ref()
    }

    pub(crate) fn running(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }

    /// 切换安装详情日志面板的展开/收起状态。
    pub(crate) fn toggle_logs(&mut self, cx: &mut Context<Self>) {
        self.drawer = if self.drawer == DetailDrawer::Logs {
            DetailDrawer::None
        } else {
            DetailDrawer::Logs
        };
        self.reset_window_fit();
        cx.notify();
    }

    /// 切换授权页原始执行命令的展开/收起状态。
    pub(crate) fn toggle_auth_command(&mut self, cx: &mut Context<Self>) {
        self.drawer = if self.drawer == DetailDrawer::AuthCommand {
            DetailDrawer::None
        } else {
            DetailDrawer::AuthCommand
        };
        self.reset_window_fit();
        cx.notify();
    }

    /// 安装完成后直接启动该应用并退出安装窗口。
    pub(crate) fn launch_app(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.launch_entry.clone() else {
            return;
        };
        if let Err(error) = launch_installed(&entry) {
            tracing::warn!("安装后启动应用失败：{error}");
            self.note = Some(format!("无法启动应用：{error}"));
            cx.notify();
            return;
        }
        cx.quit();
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
        let InstallFile::Ready { file, .. } = &self.file else {
            return;
        };
        if !matches!(self.phase, Phase::Confirm) {
            return;
        }
        let path = file.path.clone();
        let name = file.metadata.name.clone();
        tracing::info!("启动安装会话：{name}");
        self.phase = Phase::Authorizing;
        self.running.store(true, Ordering::Relaxed);
        self.note = None;
        self.logs.clear();
        self.drawer = DetailDrawer::None;
        self.install_stage = InstallStage::default();

        match start_install_session(&path, name) {
            Ok(session) => {
                let events = session.events.clone();
                self.auth_tx = Some(session.responses.clone());
                self.session = Some(session);
                spawn_event_loop(events, cx, |app, event, cx| {
                    app.apply_install_event(event, cx);
                });
            }
            Err(error) => {
                self.phase = Phase::Done(Err(error));
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
                self.drawer = DetailDrawer::None;
                self.clear_password = true;
                self.focus_password = true;
                self.mask = Some(!prompt.echo);
            }
            InstallEvent::Authorized => {
                self.phase = Phase::Installing;
                self.note = None;
                self.install_stage = InstallStage::default();
            }
            InstallEvent::Output(line) => {
                self.push_install_log(line);
            }
            InstallEvent::Finished(result) => {
                let is_ok = result.is_ok();
                self.phase = Phase::Done(result);
                self.auth_tx = None;
                self.session = None;
                self.running.store(false, Ordering::Relaxed);
                if is_ok {
                    self.resolve_launchable_desktop(cx);
                }
            }
        }
        cx.notify();
    }

    fn push_install_log(&mut self, line: String) {
        if let Some(stage) = infer_install_stage(&line)
            && stage.progress >= self.install_stage.progress
        {
            self.install_stage = stage;
        }
        if self.logs.len() >= MAX_INSTALL_LOGS {
            self.logs.remove(0);
        }
        self.logs.push(line);
    }

    fn resolve_launchable_desktop(&mut self, cx: &mut Context<Self>) {
        let InstallFile::Ready { file, .. } = &self.file else {
            return;
        };
        let pkg = file.metadata.name.clone();
        spawn_background(
            cx,
            move || find_launchable_desktop(&pkg),
            |app, entry, cx| {
                app.launch_entry = entry;
                cx.notify();
            },
        );
    }

    /// 提交密码（Enter 与「确认」按钮共用）。
    pub(crate) fn submit_password(&mut self, cx: &mut Context<Self>) {
        let Phase::NeedPassword { echo, .. } = &self.phase else {
            return;
        };
        let value = if *echo {
            self.password.read(cx).value().to_string()
        } else {
            self.secret_password.read(cx).value().to_string()
        };
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
        self.session = None;
        self.clear_password = true;
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

/// 从 `apt-get` / `dpkg` 输出行中识别当前安装阶段与估算进度。
fn infer_install_stage(line: &str) -> Option<InstallStage> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("reading package lists") || line.contains("正在读取软件包列表") {
        return Some(InstallStage {
            label: "正在读取软件包列表…",
            progress: 0.25,
        });
    }
    if lower.contains("building dependency tree") || line.contains("正在分析软件包的依赖关系树")
    {
        return Some(InstallStage {
            label: "正在分析依赖关系…",
            progress: 0.35,
        });
    }
    if lower.contains("get:") || lower.contains("fetching") || line.contains("获取:") {
        return Some(InstallStage {
            label: "正在下载依赖软件包…",
            progress: 0.50,
        });
    }
    if lower.contains("preparing to unpack") || line.contains("正准备解包") {
        return Some(InstallStage {
            label: "准备解包软件文件…",
            progress: 0.62,
        });
    }
    if lower.contains("unpacking ") || line.contains("正在解包") {
        return Some(InstallStage {
            label: "正在解包并写入文件…",
            progress: 0.72,
        });
    }
    if lower.contains("setting up ") || line.contains("正在设置") {
        return Some(InstallStage {
            label: "正在配置软件包…",
            progress: 0.86,
        });
    }
    if lower.contains("processing triggers") || line.contains("正在处理用于") {
        return Some(InstallStage {
            label: "正在更新系统缓存与菜单…",
            progress: 0.95,
        });
    }
    None
}
