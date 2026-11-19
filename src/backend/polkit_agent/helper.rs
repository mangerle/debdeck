//! `polkit-agent-helper-1` 会话与 PAM 文本协议。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::backend::{BackendError, Result};
use nix::unistd::{Uid, User};
use zbus::zvariant::OwnedValue;

use crate::util::{CommandError, wait_child};

use super::{HELPER_SOCKET, SETUID_HELPER};

/// helper 输出事件容量。PAM 提示数量有限，保留足够余量避免阻塞 helper。
const HELPER_EVENT_CAPACITY: usize = 32;
/// helper 输入队列容量。同一提示只允许一个密码回执在途。
const HELPER_INPUT_CAPACITY: usize = 4;
/// helper 认证结束后等待子进程退出的时限。
const HELPER_EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// 认证 helper 读到的内容。
pub(super) enum HelperEvent {
    /// PAM 请求输入。
    Prompt { prompt: String, echo: bool },
    /// PAM 返回错误信息。
    Error(String),
    /// PAM 返回说明信息。
    Info(String),
    /// 认证成功。
    Success,
    /// 认证失败。
    Failure,
    /// helper 管道关闭。
    Closed,
}

/// 交给认证 helper 的回应。
enum HelperInput {
    Password(String),
    Cancel,
}

/// 与一次 PAM 认证对应的 helper 通道和后台线程。
pub(super) struct HelperSession {
    pub(super) events: async_channel::Receiver<HelperEvent>,
    input: async_channel::Sender<HelperInput>,
    handle: Option<JoinHandle<()>>,
}

impl HelperSession {
    /// 连接 helper 并启动后台协议循环。
    pub(super) fn start(
        cookie: &str,
        identities: &[(String, HashMap<String, OwnedValue>)],
    ) -> Result<Self> {
        let username = identity_username(identities)?;
        let (read_end, write_end, child) = open_helper(&username, cookie)?;
        let (event_tx, event_rx) = async_channel::bounded(HELPER_EVENT_CAPACITY);
        let (input_tx, input_rx) = async_channel::bounded(HELPER_INPUT_CAPACITY);
        let handle = thread::spawn(move || {
            helper_loop(HelperLoop {
                read_end,
                write_end,
                events: &event_tx,
                input: &input_rx,
                child,
            });
        });
        Ok(Self {
            events: event_rx,
            input: input_tx,
            handle: Some(handle),
        })
    }

    /// 提交密码。
    pub(super) fn reply(&self, password: String) {
        if self
            .input
            .try_send(HelperInput::Password(password))
            .is_err()
        {
            tracing::warn!("密码回执未能送入认证 helper，输入队列已满或 helper 已退出");
        }
    }

    /// 请求取消当前 PAM 会话。
    pub(super) fn cancel(&self) {
        if self.input.try_send(HelperInput::Cancel).is_err() {
            tracing::warn!("取消指令未能送入认证 helper，输入队列已满或 helper 已退出");
        }
    }
}

impl Drop for HelperSession {
    fn drop(&mut self) {
        // 同时关闭输出事件与输入端：前者解除 helper 线程可能的事件背压，
        // 后者让阻塞中的 PAM 密码读取立即返回，再等待线程与子进程收尾。
        self.events.close();
        self.input.close();
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            tracing::error!("polkit helper 线程发生 panic，已隔离");
        }
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
        let user =
            User::from_uid(Uid::from_raw(uid)).map_err(|error| BackendError::Authentication {
                detail: format!("查询系统用户失败：{error}"),
            })?;
        if let Some(user) = user {
            return Ok(user.name);
        }
    }
    Err(BackendError::Authentication {
        detail: "polkit 没有提供可登录的 unix-user 身份".to_string(),
    })
}

type HelperChild = Option<Child>;
type HelperRead = Box<dyn Read + Send>;
type HelperWrite = Box<dyn Write + Send>;
type HelperStreams = (HelperRead, HelperWrite, HelperChild);

/// 连接 socket 激活的 helper；旧系统回退到 setuid helper。
fn open_helper(username: &str, cookie: &str) -> Result<HelperStreams> {
    if let Some(streams) = open_socket_helper(username, cookie)? {
        return Ok(streams);
    }
    open_setuid_helper(username, cookie)
}

fn open_socket_helper(username: &str, cookie: &str) -> Result<Option<HelperStreams>> {
    if !Path::new(HELPER_SOCKET).exists() {
        return Ok(None);
    }
    match UnixStream::connect(HELPER_SOCKET) {
        Ok(stream) => {
            let mut write = stream.try_clone().map_err(|source| {
                BackendError::io("复制 polkit helper socket", "polkit helper socket", source)
            })?;
            write_socket_credentials(&mut write, username, cookie)?;
            Ok(Some((Box::new(stream), Box::new(write), None)))
        }
        Err(error) => {
            tracing::warn!("连接 polkit helper socket 失败，回退 setuid helper：{error}");
            Ok(None)
        }
    }
}

fn open_setuid_helper(username: &str, cookie: &str) -> Result<HelperStreams> {
    if !Path::new(SETUID_HELPER).is_file() {
        return Err(BackendError::missing_program(
            "管理员认证",
            "polkit-agent-helper-1",
        ));
    }
    let mut command = Command::new(SETUID_HELPER);
    command
        .arg(username)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().map_err(|source| {
        BackendError::command(
            "启动 polkit-agent-helper-1",
            CommandError::Spawn {
                command: SETUID_HELPER.to_string(),
                source,
            },
        )
    })?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(BackendError::Authentication {
            detail: "polkit helper 的标准输入不可用".to_string(),
        });
    };
    write_secret(&mut stdin, cookie)?;
    let Some(stdout) = child.stdout.take() else {
        return Err(BackendError::Authentication {
            detail: "polkit helper 的标准输出不可用".to_string(),
        });
    };
    Ok((Box::new(stdout), Box::new(stdin), Some(child)))
}

/// socket 协议先写用户名，再写 cookie，各占一行。
fn write_socket_credentials(write: &mut impl Write, username: &str, cookie: &str) -> Result<()> {
    write
        .write_all(username.as_bytes())
        .map_err(|source| BackendError::io("写入 polkit 用户名", "polkit helper socket", source))?;
    write.write_all(b"\n").map_err(|source| {
        BackendError::io("写入 polkit 用户名换行", "polkit helper socket", source)
    })?;
    write_secret(write, cookie)
}

/// setuid helper 已从 argv 取得用户名，只从标准输入读取 cookie。
fn write_secret(write: &mut impl Write, cookie: &str) -> Result<()> {
    write
        .write_all(cookie.as_bytes())
        .map_err(|source| BackendError::io("写入 polkit cookie", "polkit helper", source))?;
    write
        .write_all(b"\n")
        .map_err(|source| BackendError::io("写入 polkit cookie 换行", "polkit helper", source))?;
    write
        .flush()
        .map_err(|source| BackendError::io("刷新 polkit cookie", "polkit helper", source))?;
    Ok(())
}

fn send_helper_event(events: &async_channel::Sender<HelperEvent>, event: HelperEvent) {
    if events.send_blocking(event).is_err() {
        tracing::debug!("安装控制线程已退出，停止发送 polkit helper 事件");
    }
}

struct HelperLoop<'a> {
    read_end: HelperRead,
    write_end: HelperWrite,
    events: &'a async_channel::Sender<HelperEvent>,
    input: &'a async_channel::Receiver<HelperInput>,
    child: HelperChild,
}

fn helper_loop(mut session: HelperLoop<'_>) {
    let mut reader = BufReader::new(session.read_end);
    let mut line = String::with_capacity(256);
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => {
                send_helper_event(session.events, HelperEvent::Closed);
                break;
            }
            Ok(_) => {}
        }
        let text = c_unescape(line.trim_end_matches(['\n', '\r']));
        if !handle_line(&text, session.events, session.input, &mut session.write_end) {
            break;
        }
    }
    if let Some(mut child) = session.child.take()
        && let Err(error) = wait_child(&mut child, HELPER_EXIT_TIMEOUT)
    {
        tracing::warn!("等待 polkit-agent-helper-1 退出失败：{error}");
    }
}

/// 处理一行 PAM 协议文本；返回 false 表示会话应结束。
fn handle_line(
    text: &str,
    events: &async_channel::Sender<HelperEvent>,
    input: &async_channel::Receiver<HelperInput>,
    write_end: &mut HelperWrite,
) -> bool {
    if let Some(prompt) = text.strip_prefix("PAM_PROMPT_ECHO_OFF ") {
        tracing::info!("PAM 正在请求密码输入");
        send_helper_event(
            events,
            HelperEvent::Prompt {
                prompt: prompt.to_string(),
                echo: false,
            },
        );
        return wait_for_password(input, write_end);
    }
    if let Some(prompt) = text.strip_prefix("PAM_PROMPT_ECHO_ON ") {
        send_helper_event(
            events,
            HelperEvent::Prompt {
                prompt: prompt.to_string(),
                echo: true,
            },
        );
        return wait_for_password(input, write_end);
    }
    if let Some(error) = text.strip_prefix("PAM_ERROR_MSG ") {
        send_helper_event(events, HelperEvent::Error(error.to_string()));
    } else if let Some(info) = text.strip_prefix("PAM_TEXT_INFO ") {
        send_helper_event(events, HelperEvent::Info(info.to_string()));
    } else if text.starts_with("SUCCESS") {
        tracing::info!("管理员认证成功");
        send_helper_event(events, HelperEvent::Success);
        return false;
    } else if text.starts_with("FAILURE") {
        tracing::warn!("管理员认证失败");
        send_helper_event(events, HelperEvent::Failure);
        return false;
    } else if !text.is_empty() {
        send_helper_event(events, HelperEvent::Info(text.to_string()));
    }
    true
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
                    value = value
                        .saturating_mul(8)
                        .saturating_add(u16::from(bytes[index] - b'0'));
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
