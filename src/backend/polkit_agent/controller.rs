//! 安装认证会话的状态机。
//!
//! 控制器只做三件事：把 D-Bus 授权请求接到 helper、把 helper/PAM 事件转给 UI、
//! 把 UI 密码回执写回 helper。所有等待都有轮询边界；窗口关闭或总超时会先给
//! 挂起的 D-Bus 调用一个明确结果，再终止并回收子进程。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nix::unistd::Uid;
use zbus::OwnedVariant;

use crate::backend::{privileged, shell};

use super::agent::register_agent;
use super::helper::{HelperEvent, HelperSession};
use super::process::{PkexecProcess, ProcessExit, process_start_time};
use super::{AuthPrompt, AuthResponse, INSTALL_TIMEOUT, InstallEvent};

/// 主循环轮询间隔：既要及时响应 PAM 提示，又不能空转烧 CPU。
const POLL_INTERVAL: Duration = Duration::from_millis(40);
/// D-Bus 认证请求队列容量。正常同一时刻只有一个授权请求。
const AUTH_REQUEST_CAPACITY: usize = 8;

/// 认证代理的回调请求。
pub(super) enum AuthRequest {
    Begin {
        cookie: String,
        message: String,
        identities: Vec<(String, HashMap<String, OwnedVariant>)>,
        /// 认证结束时用它唤醒挂起的 D-Bus 调用。
        done: async_channel::Sender<AuthOutcome>,
    },
    Cancel,
}

/// `BeginAuthentication` 的最终结果。
pub(super) enum AuthOutcome {
    Authorized,
    Failed(String),
    Cancelled,
}

/// 启动一次安装控制器所需的上下文。
pub(super) struct ControllerInput<'a> {
    pub(super) args: Vec<String>,
    pub(super) pkg: String,
    pub(super) events: async_channel::Sender<InstallEvent>,
    pub(super) responses: async_channel::Receiver<AuthResponse>,
    pub(super) stop: &'a AtomicBool,
}

/// 从本地安装包启动会话所需的数据。
pub(super) struct PathSession<'a> {
    pub(super) path: &'a Path,
    pub(super) pkg: String,
    pub(super) events: async_channel::Sender<InstallEvent>,
    pub(super) responses: async_channel::Receiver<AuthResponse>,
    pub(super) stop: &'a AtomicBool,
}

/// 在后台线程完成安装包预检并启动控制器。
pub(super) fn run_from_path(session: PathSession<'_>) {
    let PathSession {
        path,
        pkg,
        events,
        responses,
        stop,
    } = session;
    let args = match privileged::local_install_args(path) {
        Ok(args) => args,
        Err(error) => {
            send_finished(&events, Err(error.to_string()));
            return;
        }
    };
    run_controller(ControllerInput {
        args,
        pkg,
        events,
        responses,
        stop,
    });
}

/// 后台控制器入口：准备认证代理与 pkexec，然后交给状态机运行。
pub(super) fn run_controller(input: ControllerInput<'_>) {
    let ControllerInput {
        args,
        pkg,
        events,
        responses,
        stop,
    } = input;
    let Some((pid, start_time, uid)) = current_subject(&events) else {
        return;
    };
    let (auth_tx, auth_rx) = async_channel::bounded::<AuthRequest>(AUTH_REQUEST_CAPACITY);
    let _registration = match register_agent(pid, start_time, uid, auth_tx) {
        Ok(registration) => registration,
        Err(error) => {
            tracing::warn!("内嵌认证不可用，回退系统认证窗口：{error}");
            fallback_to_system(&args, &pkg, &events);
            return;
        }
    };
    let mut process = match PkexecProcess::start(&args) {
        Ok(process) => process,
        Err(error) => {
            send_finished(&events, Err(error.to_string()));
            return;
        }
    };
    let mut controller = Controller {
        pkg,
        events,
        responses,
        stop,
        helper: None,
        pending_auth: None,
        policy_message: String::new(),
        pending_error: None,
        authorized: false,
    };
    controller.run(&mut process, &auth_rx);
}

fn current_subject(events: &async_channel::Sender<InstallEvent>) -> Option<(u32, u64, i32)> {
    let pid = std::process::id();
    let Ok(start_time) = process_start_time(pid) else {
        send_finished(events, Err("无法读取当前进程的启动时间。".to_string()));
        return None;
    };
    let Ok(uid) = i32::try_from(Uid::current().as_raw()) else {
        send_finished(events, Err("当前用户 ID 超出范围。".to_string()));
        return None;
    };
    Some((pid, start_time, uid))
}

fn fallback_to_system(args: &[String], pkg: &str, events: &async_channel::Sender<InstallEvent>) {
    send_install_event(
        events,
        InstallEvent::Notice("无法内嵌管理员认证，已改用系统密码窗口。".to_string()),
    );
    let result = privileged::run_privileged_argv(args)
        .map(|_| format!("软件包 {pkg} 已安装。"))
        .map_err(|error| error.to_string());
    if result.is_ok() {
        shell::notify("应用已安装", pkg);
    }
    send_finished(events, result);
}

struct AuthBegin {
    cookie: String,
    message: String,
    identities: Vec<(String, HashMap<String, OwnedVariant>)>,
    done: async_channel::Sender<AuthOutcome>,
}

struct Controller<'a> {
    pkg: String,
    events: async_channel::Sender<InstallEvent>,
    responses: async_channel::Receiver<AuthResponse>,
    stop: &'a AtomicBool,
    helper: Option<HelperSession>,
    pending_auth: Option<async_channel::Sender<AuthOutcome>>,
    policy_message: String,
    pending_error: Option<String>,
    authorized: bool,
}

impl Controller<'_> {
    fn run(&mut self, process: &mut PkexecProcess, auth_rx: &async_channel::Receiver<AuthRequest>) {
        let started = Instant::now();
        loop {
            if self.stop.load(Ordering::Relaxed) {
                self.abort(process, "已取消安装。");
                return;
            }
            if started.elapsed() >= INSTALL_TIMEOUT {
                self.abort(process, "安装超时，已终止管理员操作。");
                return;
            }
            if !self.handle_auth_requests(process, auth_rx) {
                return;
            }
            self.handle_helper_events();
            if !self.handle_responses(process) {
                return;
            }
            match process.poll() {
                Ok(Some(exit)) => {
                    self.finish(process, &exit);
                    return;
                }
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(error) => {
                    self.abort(process, &format!("无法等待 pkexec：{error:#}"));
                    return;
                }
            }
        }
    }

    fn abort(&mut self, process: &mut PkexecProcess, message: &str) {
        resolve_auth(&mut self.pending_auth, AuthOutcome::Cancelled);
        process.terminate();
        send_finished(&self.events, Err(message.to_string()));
    }

    fn handle_auth_requests(
        &mut self,
        process: &mut PkexecProcess,
        auth_rx: &async_channel::Receiver<AuthRequest>,
    ) -> bool {
        while let Ok(request) = auth_rx.try_recv() {
            if !self.handle_auth_request(process, request) {
                return false;
            }
        }
        true
    }

    fn handle_auth_request(&mut self, process: &mut PkexecProcess, request: AuthRequest) -> bool {
        match request {
            AuthRequest::Begin {
                cookie,
                message,
                identities,
                done,
            } => self.begin_auth(
                process,
                AuthBegin {
                    cookie,
                    message,
                    identities,
                    done,
                },
            ),
            AuthRequest::Cancel => {
                if let Some(session) = &self.helper {
                    session.cancel();
                }
                resolve_auth(&mut self.pending_auth, AuthOutcome::Cancelled);
                true
            }
        }
    }

    fn begin_auth(&mut self, process: &mut PkexecProcess, request: AuthBegin) -> bool {
        let AuthBegin {
            cookie,
            message,
            identities,
            done,
        } = request;
        if self.pending_auth.is_some() {
            let _ = done.try_send(AuthOutcome::Failed("已有授权请求正在进行".to_string()));
            return true;
        }
        if self.helper.is_some() || self.authorized {
            let _ = done.try_send(AuthOutcome::Authorized);
            return true;
        }
        self.policy_message = if message.is_empty() {
            "需要管理员授权".to_string()
        } else {
            message
        };
        match HelperSession::start(&cookie, &identities) {
            Ok(session) => {
                self.helper = Some(session);
                self.pending_auth = Some(done);
                true
            }
            Err(error) => {
                tracing::warn!("无法启动认证 helper：{error:#}");
                let _ = done.try_send(AuthOutcome::Failed(error.to_string()));
                process.terminate();
                send_finished(&self.events, Err(format!("无法完成管理员认证：{error:#}")));
                false
            }
        }
    }

    fn handle_helper_events(&mut self) {
        let Some(session) = self.helper.take() else {
            return;
        };
        while let Ok(event) = session.events.try_recv() {
            self.handle_helper_event(event);
        }
        self.helper = Some(session);
    }

    fn handle_helper_event(&mut self, event: HelperEvent) {
        match event {
            HelperEvent::Prompt { prompt, echo } => {
                send_install_event(
                    &self.events,
                    InstallEvent::Prompt(AuthPrompt {
                        message: self.policy_message.clone(),
                        prompt,
                        echo,
                        error: self.pending_error.take(),
                    }),
                );
            }
            HelperEvent::Error(text) => self.pending_error = Some(text),
            HelperEvent::Info(text) => {
                send_install_event(&self.events, InstallEvent::Notice(text));
            }
            HelperEvent::Success => {
                if !self.authorized {
                    self.authorized = true;
                    resolve_auth(&mut self.pending_auth, AuthOutcome::Authorized);
                    send_install_event(&self.events, InstallEvent::Authorized);
                }
            }
            HelperEvent::Failure => {
                send_install_event(
                    &self.events,
                    InstallEvent::Notice("密码验证失败。".to_string()),
                );
                resolve_auth(
                    &mut self.pending_auth,
                    AuthOutcome::Failed("密码验证失败".to_string()),
                );
            }
            HelperEvent::Closed => {
                if !self.authorized {
                    resolve_auth(
                        &mut self.pending_auth,
                        AuthOutcome::Failed("认证会话意外结束".to_string()),
                    );
                }
            }
        }
    }

    fn handle_responses(&mut self, process: &mut PkexecProcess) -> bool {
        while let Ok(response) = self.responses.try_recv() {
            match response {
                AuthResponse::Input(password) => {
                    if let Some(session) = &self.helper {
                        session.reply(password);
                    }
                }
                AuthResponse::Cancel => {
                    if let Some(session) = &self.helper {
                        session.cancel();
                    }
                    resolve_auth(&mut self.pending_auth, AuthOutcome::Cancelled);
                    process.terminate();
                    send_finished(&self.events, Err("已取消安装。".to_string()));
                    return false;
                }
            }
        }
        true
    }

    fn finish(&mut self, process: &mut PkexecProcess, exit: &ProcessExit) {
        // 先回收输出线程，再释放进程组，避免返回后仍有线程持有管道。
        process.terminate();
        let result = install_result(&self.pkg, exit);
        if result.is_ok() {
            shell::notify("应用已安装", &self.pkg);
        }
        resolve_auth(
            &mut self.pending_auth,
            AuthOutcome::Failed("pkexec 已退出".to_string()),
        );
        send_finished(&self.events, result);
    }
}

fn install_result(pkg: &str, exit: &ProcessExit) -> Result<String, String> {
    if exit.status.success() {
        return Ok(format!("软件包 {pkg} 已安装。"));
    }
    let code = exit.status.code().unwrap_or(-1);
    if exit.output.is_empty() {
        Err(format!("安装失败（退出码 {code}）。"))
    } else {
        Err(format!("安装失败（退出码 {code}）：\n{}", exit.output))
    }
}

fn resolve_auth(pending: &mut Option<async_channel::Sender<AuthOutcome>>, outcome: AuthOutcome) {
    if let Some(done) = pending.take() {
        let _ = done.try_send(outcome);
    }
}

fn send_finished(events: &async_channel::Sender<InstallEvent>, result: Result<String, String>) {
    send_install_event(events, InstallEvent::Finished(result));
}

/// 发送安装事件。控制器是专用线程，队列满时阻塞形成背压；接收端关闭时静默退出。
fn send_install_event(events: &async_channel::Sender<InstallEvent>, event: InstallEvent) {
    if events.send_blocking(event).is_err() {
        tracing::debug!("安装窗口已关闭，停止发送安装事件");
    }
}
