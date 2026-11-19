//! 安装认证会话的事件驱动状态机。
//!
//! 控制器只做三件事：把 D-Bus 授权请求接到 helper、把 helper/PAM 事件转给 UI、
//! 把 UI 密码回执写回 helper。所有外部事件源（D-Bus、helper、UI 回执、pkexec
//! 进程退出、总超时）全部汇聚到单一有界通道，阻塞等待事件唤醒，不使用定时轮询；
//! 窗口关闭或总超时会先给挂起的 D-Bus 调用明确结果，再终止并回收子进程。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backend::Result as BackendResult;
use nix::unistd::Uid;
use zbus::OwnedVariant;

use crate::backend::{privileged, shell};

use super::agent::register_agent;
use super::guards::{ResponseForwarder, TimeoutGuard};
use super::helper::{HelperEvent, HelperSession};
use super::process::{PkexecProcess, ProcessExit, process_start_time};
use super::{AuthPrompt, AuthResponse, INSTALL_TIMEOUT, InstallEvent};

/// 控制器统一事件总线容量：涵盖 D-Bus 授权、PAM 提示、实时输出、UI 回执与进程退出。
const CONTROLLER_EVENT_CAPACITY: usize = 256;

/// 汇聚到控制器的所有异步事件源。
pub(super) enum ControllerEvent {
    Auth(AuthRequest),
    Helper(HelperEvent),
    Response(AuthResponse),
    ProcessOutput(String),
    ProcessExited(BackendResult<ProcessExit>),
    Timeout,
    Stop,
}

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

/// 会话结束时的成功文案与桌面通知配置。
pub(super) struct SessionOutcome {
    pub(super) success_message: String,
    pub(super) notify: Option<(String, String)>,
}

/// 启动一次控制器的完整上下文。
pub(super) struct ControllerInput<'a> {
    pub(super) args: Vec<String>,
    pub(super) outcome: SessionOutcome,
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
        outcome: SessionOutcome {
            success_message: format!("软件包 {pkg} 已安装。"),
            notify: Some(("应用已安装".to_string(), pkg)),
        },
        events,
        responses,
        stop,
    });
}

/// 后台控制器入口：准备认证代理、pkexec 与事件汇聚总线，然后进入纯事件驱动循环。
pub(super) fn run_controller(input: ControllerInput<'_>) {
    let ControllerInput {
        args,
        outcome,
        events,
        responses,
        stop,
    } = input;
    let Some((pid, start_time, uid)) = current_subject(&events) else {
        return;
    };
    let (ctrl_tx, ctrl_rx) = async_channel::bounded::<ControllerEvent>(CONTROLLER_EVENT_CAPACITY);
    let _registration = match register_agent(pid, start_time, uid, ctrl_tx.clone()) {
        Ok(registration) => registration,
        Err(error) => {
            tracing::warn!("内嵌认证不可用，回退系统认证窗口：{error}");
            fallback_to_system(&args, &outcome, &events);
            return;
        }
    };
    let mut process = match PkexecProcess::start(&args, ctrl_tx.clone()) {
        Ok(process) => process,
        Err(error) => {
            send_finished(&events, Err(error.to_string()));
            return;
        }
    };
    let _response_forwarder = ResponseForwarder::start(responses, ctrl_tx.clone());
    let _timeout_guard = TimeoutGuard::start(INSTALL_TIMEOUT, ctrl_tx.clone());

    let mut controller = Controller {
        outcome,
        events,
        ctrl_tx,
        stop,
        helper: None,
        pending_auth: None,
        policy_message: String::new(),
        pending_error: None,
        authorized: false,
    };
    controller.run(&mut process, &ctrl_rx);
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

fn fallback_to_system(
    args: &[String],
    outcome: &SessionOutcome,
    events: &async_channel::Sender<InstallEvent>,
) {
    send_install_event(
        events,
        InstallEvent::Notice("无法内嵌管理员认证，已改用系统密码窗口。".to_string()),
    );
    let result = privileged::run_privileged_argv(args)
        .map(|_| outcome.success_message.clone())
        .map_err(|error| error.to_string());
    if result.is_ok()
        && let Some((title, body)) = outcome.notify.as_ref()
    {
        shell::notify(title, body);
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
    outcome: SessionOutcome,
    events: async_channel::Sender<InstallEvent>,
    ctrl_tx: async_channel::Sender<ControllerEvent>,
    stop: &'a AtomicBool,
    helper: Option<HelperSession>,
    pending_auth: Option<async_channel::Sender<AuthOutcome>>,
    policy_message: String,
    pending_error: Option<String>,
    authorized: bool,
}

impl Controller<'_> {
    fn run(
        &mut self,
        process: &mut PkexecProcess,
        ctrl_rx: &async_channel::Receiver<ControllerEvent>,
    ) {
        while let Ok(event) = ctrl_rx.recv_blocking() {
            if self.stop.load(Ordering::Relaxed) {
                self.abort(process, "已取消操作。");
                return;
            }
            if !self.dispatch_event(process, event) {
                return;
            }
        }
    }

    fn dispatch_event(&mut self, process: &mut PkexecProcess, event: ControllerEvent) -> bool {
        match event {
            ControllerEvent::Auth(request) => self.handle_auth_request(process, request),
            ControllerEvent::Helper(helper_event) => {
                self.handle_helper_event(helper_event);
                true
            }
            ControllerEvent::Response(response) => self.handle_response(process, response),
            ControllerEvent::ProcessOutput(line) => {
                send_install_event(&self.events, InstallEvent::Output(line));
                true
            }
            ControllerEvent::ProcessExited(exit) => {
                process.mark_finished();
                match exit {
                    Ok(exit) => self.finish(process, &exit),
                    Err(error) => {
                        self.abort(process, &format!("无法等待 pkexec：{error:#}"));
                    }
                }
                false
            }
            ControllerEvent::Timeout => {
                self.abort(process, "安装超时，已终止管理员操作。");
                false
            }
            ControllerEvent::Stop => {
                self.abort(process, "已取消操作。");
                false
            }
        }
    }

    fn abort(&mut self, process: &mut PkexecProcess, message: &str) {
        resolve_auth(&mut self.pending_auth, AuthOutcome::Cancelled);
        process.terminate();
        send_finished(&self.events, Err(message.to_string()));
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
        match HelperSession::start(&cookie, &identities, self.ctrl_tx.clone()) {
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

    fn handle_response(&mut self, process: &mut PkexecProcess, response: AuthResponse) -> bool {
        match response {
            AuthResponse::Input(password) => {
                if let Some(session) = &self.helper {
                    session.reply(password);
                }
                true
            }
            AuthResponse::Cancel => {
                if let Some(session) = &self.helper {
                    session.cancel();
                }
                resolve_auth(&mut self.pending_auth, AuthOutcome::Cancelled);
                process.terminate();
                send_finished(&self.events, Err("已取消操作。".to_string()));
                false
            }
        }
    }

    fn finish(&mut self, process: &mut PkexecProcess, exit: &ProcessExit) {
        // 先回收输出线程，再释放进程组，避免返回后仍有线程持有管道。
        process.terminate();
        let result = command_result(&self.outcome.success_message, exit);
        if result.is_ok()
            && let Some((title, body)) = self.outcome.notify.as_ref()
        {
            shell::notify(title, body);
        }
        resolve_auth(
            &mut self.pending_auth,
            AuthOutcome::Failed("pkexec 已退出".to_string()),
        );
        send_finished(&self.events, result);
    }
}

fn command_result(success_message: &str, exit: &ProcessExit) -> Result<String, String> {
    if exit.status.success() {
        return Ok(success_message.to_string());
    }
    let code = exit.status.code().unwrap_or(-1);
    let lock_hint = if exit.output.contains("Could not get lock")
        || exit.output.contains("Unable to acquire")
    {
        "\n提示：APT 正被其他进程占用，请等待系统更新或终端 apt 操作完成后重试。"
    } else {
        ""
    };
    if exit.output.is_empty() {
        Err(format!("操作失败（退出码 {code}）。{lock_hint}"))
    } else {
        Err(format!(
            "操作失败（退出码 {code}）：\n{}{lock_hint}",
            exit.output
        ))
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
