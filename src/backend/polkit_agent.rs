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

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use nix::unistd::{Uid, User};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

use crate::backend::{privileged, shell};

/// polkit 标准认证代理对象路径。
const AGENT_PATH: &str = "/org/freedesktop/PolicyKit1/AuthenticationAgent";
/// polkit 127+ 的 socket 激活 helper；不存在时回退到 setuid helper。
const HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";
const SETUID_HELPER: &str = "/usr/lib/polkit-1/polkit-agent-helper-1";

const AUTHORITY_SERVICE: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";

/// 主循环轮询间隔：既要及时响应 PAM 提示，又不能空转烧 CPU。
const POLL_INTERVAL: Duration = Duration::from_millis(40);

/// 安装窗口需要处理的事件。
pub enum InstallEvent {
    /// 给用户看的说明（回退系统认证、PAM 信息等）。
    Notice(String),
    /// PAM 正在等待输入。
    Prompt(AuthPrompt),
    /// 授权完成，pkexec 正在执行 apt-get。
    Authorized,
    /// 安装结束（成功文本 / 失败详情）。
    Finished(Result<String, String>),
}

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
    Input(String),
    Cancel,
}

/// 一次带内嵌认证的安装会话；`events` 与 `responses` 是 UI 的唯一接口。
pub struct InstallSession {
    pub events: async_channel::Receiver<InstallEvent>,
    pub responses: async_channel::Sender<AuthResponse>,
    _handle: thread::JoinHandle<()>,
}

/// 启动安装会话：所有工作都在后台线程，UI 只消费事件。
pub fn start(path: &Path, pkg: String) -> Result<InstallSession> {
    let args = privileged::local_install_args(path)?;
    let (events, event_rx) = async_channel::unbounded();
    let (response_tx, response_rx) = async_channel::unbounded();
    let handle = thread::spawn(move || {
        run_controller(args, pkg, events, response_rx);
    });
    Ok(InstallSession {
        events: event_rx,
        responses: response_tx,
        _handle: handle,
    })
}

/// 认证代理的回调请求。
enum AuthRequest {
    Begin {
        cookie: String,
        message: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
        /// 认证结束时用它唤醒挂起的 D-Bus 调用。
        done: async_channel::Sender<AuthOutcome>,
    },
    Cancel,
}

/// `BeginAuthentication` 的最终结果。
enum AuthOutcome {
    Authorized,
    Failed(String),
    Cancelled,
}

/// 认证 helper 读到的内容。
enum HelperEvent {
    Prompt { prompt: String, echo: bool },
    Error(String),
    Info(String),
    Success,
    Failure,
    Closed,
}

/// 交给认证 helper 的回应。
enum HelperInput {
    Password(String),
    Cancel,
}

/// 后台控制器：串起 D-Bus 代理、pkexec 子进程与认证 helper。
///
/// 通道与参数都按值传入：线程闭包必须持有它们直到安装结束。
#[allow(
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    reason = "控制器是单次安装的线性状态机，拆开反而增加跨函数状态"
)]
fn run_controller(
    args: Vec<String>,
    pkg: String,
    events: async_channel::Sender<InstallEvent>,
    responses: async_channel::Receiver<AuthResponse>,
) {
    // pkexec 把「调用者的父进程」作为 polkit subject（见 pkexec.c 的 getppid），
    // 而 pkexec 的父进程正是本安装进程。因此代理要注册在自己的 PID 上，
    // 而不是即将启动的 pkexec 子进程上。
    let pid = std::process::id();
    let Ok(start_time) = process_start_time(pid) else {
        send_finished(&events, Err("无法读取当前进程的启动时间。".to_string()));
        return;
    };
    let Ok(uid) = i32::try_from(Uid::current().as_raw()) else {
        send_finished(&events, Err("当前用户 ID 超出范围。".to_string()));
        return;
    };

    let (auth_tx, auth_rx) = async_channel::unbounded::<AuthRequest>();
    let registration = register_agent(pid, start_time, uid, auth_tx);
    let _registration = match registration {
        Ok(registration) => registration,
        Err(error) => {
            tracing::warn!("内嵌认证不可用，回退系统认证窗口：{error:#}");
            let _ = events.try_send(InstallEvent::Notice(
                "无法内嵌管理员认证，已改用系统密码窗口。".to_string(),
            ));
            let result = privileged::run_privileged_argv(&args)
                .map(|_| format!("软件包 {pkg} 已安装。"))
                .map_err(|error| format!("{error:#}"));
            if result.is_ok() {
                shell::notify("应用已安装", &pkg);
            }
            send_finished(&events, result);
            return;
        }
    };

    let mut child = match spawn_pkexec(&args) {
        Ok(child) => child,
        Err(error) => {
            send_finished(&events, Err(format!("{error:#}")));
            return;
        }
    };

    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = spawn_output_reader(child.stdout.take(), Arc::clone(&stdout));
    let stderr_thread = spawn_output_reader(child.stderr.take(), Arc::clone(&stderr));

    let mut helper: Option<HelperSession> = None;
    let mut pending_auth: Option<async_channel::Sender<AuthOutcome>> = None;
    let mut policy_message = String::new();
    let mut pending_error: Option<String> = None;
    let mut authorized = false;

    loop {
        while let Ok(request) = auth_rx.try_recv() {
            match request {
                AuthRequest::Begin {
                    cookie,
                    message,
                    identities,
                    done,
                } => {
                    if pending_auth.is_some() {
                        let _ =
                            done.try_send(AuthOutcome::Failed("已有授权请求正在进行".to_string()));
                        continue;
                    }
                    if helper.is_none() && !authorized {
                        policy_message = if message.is_empty() {
                            "需要管理员授权".to_string()
                        } else {
                            message
                        };
                        match HelperSession::start(&cookie, &identities) {
                            Ok(session) => {
                                helper = Some(session);
                                pending_auth = Some(done);
                            }
                            Err(error) => {
                                tracing::warn!("无法启动认证 helper：{error:#}");
                                let _ = done.try_send(AuthOutcome::Failed(format!("{error:#}")));
                                stop_child(&mut child);
                                send_finished(
                                    &events,
                                    Err(format!("无法完成管理员认证：{error:#}")),
                                );
                                return;
                            }
                        }
                    } else {
                        let _ = done.try_send(AuthOutcome::Authorized);
                    }
                }
                AuthRequest::Cancel => {
                    if let Some(session) = &helper {
                        session.cancel();
                    }
                    resolve_auth(&mut pending_auth, AuthOutcome::Cancelled);
                }
            }
        }

        if let Some(session) = &helper {
            while let Ok(event) = session.events.try_recv() {
                match event {
                    HelperEvent::Prompt { prompt, echo } => {
                        let _ = events.try_send(InstallEvent::Prompt(AuthPrompt {
                            message: policy_message.clone(),
                            prompt,
                            echo,
                            error: pending_error.take(),
                        }));
                    }
                    HelperEvent::Error(text) => pending_error = Some(text),
                    HelperEvent::Info(text) => {
                        let _ = events.try_send(InstallEvent::Notice(text));
                    }
                    HelperEvent::Success => {
                        if !authorized {
                            authorized = true;
                            resolve_auth(&mut pending_auth, AuthOutcome::Authorized);
                            let _ = events.try_send(InstallEvent::Authorized);
                        }
                    }
                    HelperEvent::Failure => {
                        let _ = events.try_send(InstallEvent::Notice("密码验证失败。".to_string()));
                        resolve_auth(
                            &mut pending_auth,
                            AuthOutcome::Failed("密码验证失败".to_string()),
                        );
                    }
                    HelperEvent::Closed => {
                        if !authorized {
                            resolve_auth(
                                &mut pending_auth,
                                AuthOutcome::Failed("认证会话意外结束".to_string()),
                            );
                        }
                    }
                }
            }

            while let Ok(response) = responses.try_recv() {
                match response {
                    AuthResponse::Input(password) => session.reply(password),
                    AuthResponse::Cancel => {
                        session.cancel();
                        resolve_auth(&mut pending_auth, AuthOutcome::Cancelled);
                        stop_child(&mut child);
                        send_finished(&events, Err("已取消安装。".to_string()));
                        return;
                    }
                }
            }
        } else {
            while let Ok(response) = responses.try_recv() {
                if matches!(response, AuthResponse::Cancel) {
                    resolve_auth(&mut pending_auth, AuthOutcome::Cancelled);
                    stop_child(&mut child);
                    send_finished(&events, Err("已取消安装。".to_string()));
                    return;
                }
            }
        }

        // 先消化认证结果，再检查子进程：否则极快的命令可能在 SUCCESS
        // 还没处理前就退出，被误判成授权失败。
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Some(handle) = stdout_thread {
                    let _ = handle.join();
                }
                if let Some(handle) = stderr_thread {
                    let _ = handle.join();
                }
                let output = collect_output(&stdout, &stderr);
                let result = if status.success() {
                    Ok(format!("软件包 {pkg} 已安装。"))
                } else {
                    let code = status.code().unwrap_or(-1);
                    if output.is_empty() {
                        Err(format!("安装失败（退出码 {code}）。"))
                    } else {
                        Err(format!("安装失败（退出码 {code}）：\n{output}"))
                    }
                };
                if result.is_ok() {
                    shell::notify("应用已安装", &pkg);
                }
                // pkexec 退出前授权调用若还挂着，必须先给它一个结果。
                resolve_auth(
                    &mut pending_auth,
                    AuthOutcome::Failed("pkexec 已退出".to_string()),
                );
                send_finished(&events, result);
                return;
            }
            Ok(None) => {}
            Err(error) => {
                resolve_auth(&mut pending_auth, AuthOutcome::Failed(error.to_string()));
                send_finished(&events, Err(format!("无法等待 pkexec：{error}")));
                return;
            }
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn resolve_auth(pending: &mut Option<async_channel::Sender<AuthOutcome>>, outcome: AuthOutcome) {
    if let Some(done) = pending.take() {
        let _ = done.try_send(outcome);
    }
}

fn send_finished(events: &async_channel::Sender<InstallEvent>, result: Result<String, String>) {
    let _ = events.try_send(InstallEvent::Finished(result));
}

/// 启动 pkexec。父进程是本安装进程，正好与注册代理时的 subject 对应。
fn spawn_pkexec(args: &[String]) -> Result<Child> {
    let pkexec = "/usr/bin/pkexec";
    if !Path::new(pkexec).is_file() {
        bail!("未找到 pkexec，无法请求管理员权限。");
    }
    Command::new(pkexec)
        .arg("--disable-internal-agent")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("无法启动 pkexec")
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `/proc/<pid>/stat` 的 starttime（第 22 个字段，单位是开机后的时钟滴答）。
fn process_start_time(pid: u32) -> Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .with_context(|| format!("无法读取 /proc/{pid}/stat"))?;
    let (_, rest) = stat
        .rsplit_once(')')
        .context("无法解析进程 stat（缺少 comm 结束符）")?;
    // 去掉 comm 后，rest 的第一个字段是第 3 个字段（state），starttime 是第 22 个。
    let start_time = rest
        .split_whitespace()
        .nth(19)
        .context("进程 stat 缺少 starttime")?
        .parse()
        .context("进程 stat 的 starttime 不是数字")?;
    Ok(start_time)
}

fn spawn_output_reader(
    pipe: Option<impl Read + Send + 'static>,
    buffer: Arc<Mutex<Vec<u8>>>,
) -> Option<thread::JoinHandle<()>> {
    let mut pipe = pipe?;
    Some(thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if let Ok(mut buffer) = buffer.lock() {
                        buffer.extend_from_slice(&chunk[..count]);
                    }
                }
            }
        }
    }))
}

fn collect_output(stdout: &Arc<Mutex<Vec<u8>>>, stderr: &Arc<Mutex<Vec<u8>>>) -> String {
    let read = |buffer: &Arc<Mutex<Vec<u8>>>| {
        buffer
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .unwrap_or_default()
    };
    let mut parts = Vec::new();
    let stderr = read(stderr);
    if !stderr.is_empty() {
        parts.push(stderr);
    }
    let stdout = read(stdout);
    if !stdout.is_empty() {
        parts.push(stdout);
    }
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// D-Bus 认证代理
// ---------------------------------------------------------------------------

/// 注册成功后持有的连接与 subject；drop 时通知 polkitd 注销。
struct AgentRegistration {
    proxy: Proxy<'static>,
    pid: u32,
    start_time: u64,
    uid: i32,
}

impl Drop for AgentRegistration {
    fn drop(&mut self) {
        let subject = subject_value(self.pid, self.start_time, self.uid);
        let _ = self
            .proxy
            .call_method("UnregisterAuthenticationAgent", &(subject, AGENT_PATH));
    }
}

fn register_agent(
    pid: u32,
    start_time: u64,
    uid: i32,
    tx: async_channel::Sender<AuthRequest>,
) -> Result<AgentRegistration> {
    let connection = Connection::system().context("无法连接系统总线")?;
    connection
        .object_server()
        .at(AGENT_PATH, AgentIface { tx })
        .context("无法导出 polkit 认证代理对象")?;
    let proxy = Proxy::new_owned(
        connection,
        AUTHORITY_SERVICE,
        AUTHORITY_PATH,
        AUTHORITY_IFACE,
    )
    .context("无法连接 PolicyKit")?;
    let subject = subject_value(pid, start_time, uid);
    let locale = std::env::var("LANG").unwrap_or_else(|_| "C".to_string());
    proxy
        .call_method(
            "RegisterAuthenticationAgent",
            &(subject, locale, AGENT_PATH),
        )
        .context("注册 polkit 认证代理失败")?;
    tracing::info!("已注册内嵌 polkit 认证代理（安装进程 pid {pid}）");
    Ok(AgentRegistration {
        proxy,
        pid,
        start_time,
        uid,
    })
}

fn subject_value(
    pid: u32,
    start_time: u64,
    uid: i32,
) -> (&'static str, HashMap<&'static str, Value<'static>>) {
    let mut details = HashMap::new();
    details.insert("pid", Value::from(pid));
    details.insert("start-time", Value::from(start_time));
    details.insert("uid", Value::from(uid));
    ("unix-process", details)
}

/// polkit 的标准错误名；取消必须用 `Cancelled`，polkitd 才能识别为「用户取消」。
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error", impl_display = true)]
enum PolicyKitError {
    Cancelled(String),
    Failed(String),
}

struct AgentIface {
    tx: async_channel::Sender<AuthRequest>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl AgentIface {
    // 参数必须按值接收：类型由 D-Bus 反序列化生成，无法借用。
    #[allow(
        clippy::needless_pass_by_value,
        reason = "D-Bus 输入参数由 zbus 反序列化为拥有所有权的值"
    )]
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> Result<(), PolicyKitError> {
        tracing::info!("收到 polkit 认证请求：{action_id}");
        let _ = (icon_name, details);
        let (done, done_rx) = async_channel::bounded(1);
        if self
            .tx
            .send(AuthRequest::Begin {
                cookie,
                message,
                identities,
                done,
            })
            .await
            .is_err()
        {
            return Err(PolicyKitError::Failed("安装进程已退出".to_string()));
        }
        // 关键：必须等 PAM 真正结束再返回，polkitd 用这次调用的返回值
        // 判断是否授权成功；立即返回会被当成「未授权」。
        match done_rx.recv().await {
            Ok(AuthOutcome::Authorized) => Ok(()),
            Ok(AuthOutcome::Failed(reason)) => Err(PolicyKitError::Failed(reason)),
            Ok(AuthOutcome::Cancelled) | Err(_) => {
                Err(PolicyKitError::Cancelled("认证已取消".to_string()))
            }
        }
    }

    fn cancel_authentication(&self, cookie: String) {
        let _ = (cookie, self.tx.try_send(AuthRequest::Cancel));
    }
}

// ---------------------------------------------------------------------------
// polkit-agent-helper-1 会话
// ---------------------------------------------------------------------------

struct HelperSession {
    events: async_channel::Receiver<HelperEvent>,
    input: async_channel::Sender<HelperInput>,
}

impl HelperSession {
    fn start(cookie: &str, identities: &[(String, HashMap<String, OwnedValue>)]) -> Result<Self> {
        let username = identity_username(identities)?;
        let (read_end, write_end, child) = open_helper(&username, cookie)?;
        let (event_tx, event_rx) = async_channel::unbounded();
        let (input_tx, input_rx) = async_channel::unbounded();
        thread::spawn(move || {
            helper_loop(read_end, write_end, &event_tx, &input_rx, child);
        });
        Ok(Self {
            events: event_rx,
            input: input_tx,
        })
    }

    fn reply(&self, password: String) {
        let _ = self.input.try_send(HelperInput::Password(password));
    }

    fn cancel(&self) {
        let _ = self.input.try_send(HelperInput::Cancel);
    }
}

/// 从 polkit 身份列表里选一个可登录的 Unix 用户。
fn identity_username(identities: &[(String, HashMap<String, OwnedValue>)]) -> Result<String> {
    for (kind, details) in identities {
        if kind != "unix-user" {
            continue;
        }
        let Some(uid) = details
            .get("uid")
            .and_then(|value| value.downcast_ref::<u32>().ok())
        else {
            continue;
        };
        let user = User::from_uid(Uid::from_raw(uid))
            .map_err(|error| anyhow::anyhow!("查询用户失败：{error}"))?;
        if let Some(user) = user {
            return Ok(user.name);
        }
    }
    bail!("polkit 没有提供可登录的 unix-user 身份");
}

type HelperChild = Option<Child>;
type HelperRead = Box<dyn Read + Send>;
type HelperWrite = Box<dyn Write + Send>;
type HelperStreams = (HelperRead, HelperWrite, HelperChild);

/// 连接 socket 激活的 helper；旧系统回退到 setuid helper。
fn open_helper(username: &str, cookie: &str) -> Result<HelperStreams> {
    if Path::new(HELPER_SOCKET).exists() {
        match UnixStream::connect(HELPER_SOCKET) {
            Ok(stream) => {
                let mut write = stream.try_clone().context("无法复制 helper socket")?;
                // 协议：先用户名，再 cookie，各一行。
                write.write_all(username.as_bytes())?;
                write.write_all(b"\n")?;
                write.write_all(cookie.as_bytes())?;
                write.write_all(b"\n")?;
                write.flush()?;
                return Ok((Box::new(stream), Box::new(write), None));
            }
            Err(error) => {
                tracing::warn!("连接 polkit helper socket 失败，回退 setuid helper：{error}");
            }
        }
    }

    if !Path::new(SETUID_HELPER).is_file() {
        bail!("未找到 polkit-agent-helper-1，无法进行管理员认证");
    }
    let mut child = Command::new(SETUID_HELPER)
        .arg(username)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动 polkit-agent-helper-1")?;
    let mut stdin = child.stdin.take().context("helper 的标准输入不可用")?;
    stdin.write_all(cookie.as_bytes())?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    let stdout = child.stdout.take().context("helper 的标准输出不可用")?;
    Ok((Box::new(stdout), Box::new(stdin), Some(child)))
}

fn helper_loop(
    read_end: HelperRead,
    mut write_end: HelperWrite,
    events: &async_channel::Sender<HelperEvent>,
    input: &async_channel::Receiver<HelperInput>,
    mut child: HelperChild,
) {
    let mut reader = BufReader::new(read_end);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => {
                let _ = events.try_send(HelperEvent::Closed);
                break;
            }
            Ok(_) => {}
        }
        let text = c_unescape(line.trim_end_matches(['\n', '\r']));
        if let Some(prompt) = text.strip_prefix("PAM_PROMPT_ECHO_OFF ") {
            tracing::info!("PAM 正在请求密码输入");
            let _ = events.try_send(HelperEvent::Prompt {
                prompt: prompt.to_string(),
                echo: false,
            });
            if !wait_for_password(input, &mut write_end) {
                break;
            }
        } else if let Some(prompt) = text.strip_prefix("PAM_PROMPT_ECHO_ON ") {
            let _ = events.try_send(HelperEvent::Prompt {
                prompt: prompt.to_string(),
                echo: true,
            });
            if !wait_for_password(input, &mut write_end) {
                break;
            }
        } else if let Some(error) = text.strip_prefix("PAM_ERROR_MSG ") {
            let _ = events.try_send(HelperEvent::Error(error.to_string()));
        } else if let Some(info) = text.strip_prefix("PAM_TEXT_INFO ") {
            let _ = events.try_send(HelperEvent::Info(info.to_string()));
        } else if text.starts_with("SUCCESS") {
            tracing::info!("管理员认证成功");
            let _ = events.try_send(HelperEvent::Success);
            break;
        } else if text.starts_with("FAILURE") {
            tracing::warn!("管理员认证失败");
            let _ = events.try_send(HelperEvent::Failure);
            break;
        } else if !text.is_empty() {
            let _ = events.try_send(HelperEvent::Info(text));
        }
    }
    if let Some(mut child) = child.take() {
        let _ = child.wait();
    }
}

fn wait_for_password(
    input: &async_channel::Receiver<HelperInput>,
    write_end: &mut HelperWrite,
) -> bool {
    match input.recv_blocking() {
        Ok(HelperInput::Password(mut password)) => {
            // 密码里不可能包含换行；去掉它们，避免一次写入多行。
            password.retain(|ch| ch != '\n' && ch != '\r');
            write_end.write_all(password.as_bytes()).is_ok()
                && write_end.write_all(b"\n").is_ok()
                && write_end.flush().is_ok()
        }
        Ok(HelperInput::Cancel) | Err(_) => false,
    }
}

/// 还原 helper 用 `g_strescape` 转义过的文本（含八进制字节转义）。
fn c_unescape(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        let Some(&escaped) = bytes.get(index) else {
            out.push(b'\\');
            break;
        };
        match escaped {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'v' => out.push(0x0b),
            b'a' => out.push(0x07),
            b'\\' => out.push(b'\\'),
            b'"' => out.push(b'"'),
            b'\'' => out.push(b'\''),
            b'0'..=b'7' => {
                let mut value = 0_u16;
                let mut digits = 0;
                while digits < 3 && index < bytes.len() && (b'0'..=b'7').contains(&bytes[index]) {
                    value = value * 8 + u16::from(bytes[index] - b'0');
                    index += 1;
                    digits += 1;
                }
                out.push(u8::try_from(value).unwrap_or(b'?'));
                continue;
            }
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_start_time_of_current_process() {
        let start_time = process_start_time(std::process::id()).unwrap();
        assert!(start_time > 0);
    }

    #[test]
    fn unescapes_helper_messages() {
        assert_eq!(c_unescape(r"Password: "), "Password: ");
        assert_eq!(c_unescape(r"a\\b"), r"a\b");
        assert_eq!(c_unescape(r"line\nnext"), "line\nnext");
        // g_strescape 会把非 ASCII 字节写成八进制。
        assert_eq!(
            c_unescape(r"\350\257\267\350\276\223\345\205\245\345\257\206\347\240\201"),
            "请输入密码"
        );
    }
}
