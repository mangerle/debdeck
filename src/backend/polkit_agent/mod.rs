//! 内嵌 polkit 认证代理：让管理员密码输入留在安装窗口内。
//!
//! # 为什么需要自己实现
//!
//! `pkexec` 不会自己画密码框，它把授权请求交给桌面注册的 polkit 认证代理
//! （GNOME Shell 的系统对话框）。要让密码框出现在 DebDeck 的安装窗口里，就必须
//! 自己注册一个认证代理。
//!
//! # 流程
//!
//! 1. 以本进程的 PID/启动时间注册认证代理（pkexec 的 subject 是它的父进程）；
//! 2. 启动 `pkexec --disable-internal-agent`，让授权请求落到我们注册的代理；
//! 3. polkitd 调 `BeginAuthentication`，我们连上 `polkit-agent-helper-1`
//!    （polkit 127 起是 socket 激活，旧系统回退到 setuid helper），把用户名与
//!    cookie 交给它，之后只负责把 PAM 提示转给 UI、把密码转回去；
//! 4. helper 自己把认证结果回报给 polkitd，pkexec 随后执行 apt-get。
//!
//! 任何一步失败都会回退到系统认证代理，而不是把用户卡在无法继续的窗口里。
//! 安装事件、密码回执与 helper 输出全部使用有界通道；安装控制器与 helper 子进程
//! 都在会话对象析构时等待收尾，避免窗口关闭后遗留孤儿线程或进程。

mod agent;
mod controller;
mod guards;
mod helper;
mod process;

use std::path::Path;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::backend::{BackendError, Result as BackendResult};

/// polkit 标准认证代理对象路径。
pub(super) const AGENT_PATH: &str = "/org/freedesktop/PolicyKit1/AuthenticationAgent";
/// polkit 127+ 的 socket 激活 helper；不存在时回退到 setuid helper。
pub(super) const HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";
/// 旧版本 polkit 的 setuid helper 路径。
pub(super) const SETUID_HELPER: &str = "/usr/lib/polkit-1/polkit-agent-helper-1";
/// 安装事件队列容量（含实时输出行，避免刷屏时撑爆有界队列）。
const INSTALL_EVENT_CAPACITY: usize = 256;
/// 用户回执队列容量。界面一次只提交一个密码或取消动作。
const AUTH_RESPONSE_CAPACITY: usize = 8;
/// 单次安装会话的总时限，防止 pkexec 或 PAM helper 永久挂起。
pub(super) const INSTALL_TIMEOUT: Duration = Duration::from_mins(30);

/// 安装窗口需要处理的事件。
pub enum InstallEvent {
    /// 给用户看的说明（回退系统认证、PAM 信息等）。
    Notice(String),
    /// PAM 正在等待输入。
    Prompt(AuthPrompt),
    /// 授权完成，pkexec 正在执行 apt-get。
    Authorized,
    /// `apt-get` / `dpkg` 的实时标准输出或错误行。
    Output(String),
    /// 安装结束（成功文本 / 失败详情）。
    Finished(StdResult<String, String>),
}

/// PAM 提示与 polkit 动作说明。
#[derive(Debug)]
pub struct AuthPrompt {
    /// polkit 给出的动作说明（可能为空）。
    pub message: String,
    /// PAM 提示文本。
    pub prompt: String,
    /// true 表示明文输入（例如用户名）；false 表示密码。
    pub echo: bool,
    /// 上一次认证失败的原因，紧挨着新的提示展示。
    pub error: Option<String>,
}

/// UI 对 PAM 提示的回应。
pub enum AuthResponse {
    /// 用户提交的输入文本。
    Input(String),
    /// 用户主动取消安装。
    Cancel,
}

/// 一次带内嵌认证的安装会话；`events` 与 `responses` 是 UI 的唯一接口。
pub struct InstallSession {
    /// 后台控制器发出的安装事件。
    pub events: async_channel::Receiver<InstallEvent>,
    /// UI 向后台控制器提交的密码或取消回执。
    pub responses: async_channel::Sender<AuthResponse>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for InstallSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 先关闭两条通道，让正在等待 UI 消费或等待密码的控制器立即返回，
        // 否则析构阶段直接 join 可能与满队列互相等待。
        self.events.close();
        self.responses.close();
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            tracing::error!("安装控制线程发生 panic，已隔离");
        }
    }
}

/// 启动通用提权会话：所有工作都在后台线程，UI 只消费事件并按需提交密码回执。
///
/// # Errors
/// 后台控制线程无法创建时返回错误。
pub fn start_privileged(
    args: Vec<String>,
    success_message: impl Into<String>,
    notify: Option<(String, String)>,
) -> BackendResult<InstallSession> {
    let success_message = success_message.into();
    let (events, event_rx) = async_channel::bounded(INSTALL_EVENT_CAPACITY);
    let (response_tx, response_rx) = async_channel::bounded(AUTH_RESPONSE_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = thread::Builder::new()
        .name("debdeck-privileged".to_string())
        .spawn(move || {
            controller::run_controller(controller::ControllerInput {
                args,
                outcome: controller::SessionOutcome {
                    success_message,
                    notify,
                },
                events,
                responses: response_rx,
                stop: &thread_stop,
            });
        })
        .map_err(|error| BackendError::Thread {
            operation: "创建提权控制线程",
            detail: error.to_string(),
        })?;
    Ok(InstallSession {
        events: event_rx,
        responses: response_tx,
        stop,
        handle: Some(handle),
    })
}

/// 启动安装会话：所有工作都在后台线程，UI 只消费事件。
///
/// # Errors
///
/// 后台安装线程无法创建时返回错误。安装包预检失败会在后台转换成
/// [`InstallEvent::Finished`]，避免在界面线程执行文件系统操作。
pub fn start(path: &Path, pkg: String) -> BackendResult<InstallSession> {
    let path = path.to_path_buf();
    let (events, event_rx) = async_channel::bounded(INSTALL_EVENT_CAPACITY);
    let (response_tx, response_rx) = async_channel::bounded(AUTH_RESPONSE_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = thread::Builder::new()
        .name("debdeck-install".to_string())
        .spawn(move || {
            controller::run_from_path(controller::PathSession {
                path: &path,
                pkg,
                events,
                responses: response_rx,
                stop: &thread_stop,
            });
        })
        .map_err(|error| BackendError::Thread {
            operation: "创建安装控制线程",
            detail: error.to_string(),
        })?;
    Ok(InstallSession {
        events: event_rx,
        responses: response_tx,
        stop,
        handle: Some(handle),
    })
}
