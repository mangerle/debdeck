//! `pkexec` 子进程与输出读取。
//!
//! 安装可能在认证后持续数分钟，不能复用一次性命令工具；这里保留交互期间的
//! 子进程句柄，但仍在析构时强制终止并回收，且对 stdout/stderr 设置固定上限。

use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::backend::{BackendError, Result};
use async_channel::TrySendError;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use crate::util::CommandError;

use super::controller::ControllerEvent;

/// 单个输出管道保留的最大字节数。
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// 单次读取块大小。
const READ_CHUNK: usize = 4096;
/// `SIGTERM` 后等待 `pkexec` 进程组自然退出的宽限时间，超时后改用 `SIGKILL`。
const TERM_GRACE: Duration = Duration::from_millis(200);
/// 输出队列满时的重试间隔。控制器是唯一消费者，短暂退避即可恢复背压，
/// 同时避免紧密自旋占满 CPU。
const OUTPUT_RETRY_INTERVAL: Duration = Duration::from_millis(1);

/// 已结束的 `pkexec` 进程。
pub(super) struct ProcessExit {
    /// 子进程退出状态。
    pub status: ExitStatus,
    /// stdout 与 stderr 合并后的文本。
    pub output: String,
}

/// 运行中的 `pkexec` 及其阻塞等待线程。
///
/// 由专用等待线程阻塞在 `Child::wait` 上，退出瞬间通过事件总线唤醒控制器，
/// 彻底消除定时轮询与僵尸进程残留风险。
pub(super) struct PkexecProcess {
    pid: u32,
    reaped: Arc<AtomicBool>,
    done: Arc<(Mutex<bool>, Condvar)>,
    output_stopped: Arc<AtomicBool>,
    wait_thread: Option<JoinHandle<()>>,
    finished: bool,
}

impl PkexecProcess {
    /// 启动 pkexec 并派生阻塞等待线程。父进程是本安装进程，正好与注册代理时的 subject 对应。
    pub(super) fn start(
        args: &[String],
        events: async_channel::Sender<ControllerEvent>,
    ) -> Result<Self> {
        let pkexec = "/usr/bin/pkexec";
        if !Path::new(pkexec).is_file() {
            return Err(BackendError::missing_program("启动管理员认证", "pkexec"));
        }
        let mut command = Command::new(pkexec);
        command.arg("--disable-internal-agent").args(args);
        Self::spawn_command(command, pkexec, events)
    }

    fn spawn_command(
        mut command: Command,
        program: &str,
        events: async_channel::Sender<ControllerEvent>,
    ) -> Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().map_err(|source| {
            BackendError::command(
                "启动管理员操作进程",
                CommandError::Spawn {
                    command: program.to_string(),
                    source,
                },
            )
        })?;
        let pid = child.id();
        let stdout = Arc::new(Mutex::new(Vec::with_capacity(64 * 1024)));
        let stderr = Arc::new(Mutex::new(Vec::with_capacity(64 * 1024)));
        let output_stopped = Arc::new(AtomicBool::new(false));
        let stdout_thread = spawn_output_reader(
            child.stdout.take(),
            Arc::clone(&stdout),
            events.clone(),
            Arc::clone(&output_stopped),
        );
        let stderr_thread = spawn_output_reader(
            child.stderr.take(),
            Arc::clone(&stderr),
            events.clone(),
            Arc::clone(&output_stopped),
        );

        let reaped = Arc::new(AtomicBool::new(false));
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let wait_thread = spawn_waiter(WaitContext {
            child,
            stdout,
            stderr,
            stdout_thread,
            stderr_thread,
            reaped: Arc::clone(&reaped),
            done: Arc::clone(&done),
            events,
        });

        Ok(Self {
            pid,
            reaped,
            done,
            output_stopped,
            wait_thread: Some(wait_thread),
            finished: false,
        })
    }

    /// 收到退出事件后调用：标记已结束并等待回收线程收尾。
    pub(super) fn mark_finished(&mut self) {
        self.finished = true;
        Self::join_thread(&mut self.wait_thread, "pkexec 等待");
    }

    /// 终止进程组并等待阻塞回收线程与输出线程全部结束。
    pub(super) fn terminate(&mut self) {
        if !self.finished {
            // 先解除输出线程的队列重试，再等待子进程退出，避免控制器在
            // 停止阶段与仍在等待满队列容量的输出线程互相阻塞。
            self.output_stopped.store(true, Ordering::Relaxed);
            if !self.reaped.load(Ordering::Acquire) {
                send_group_signal(self.pid, Signal::SIGTERM);
                if !self.wait_done(TERM_GRACE) && !self.reaped.load(Ordering::Acquire) {
                    send_group_signal(self.pid, Signal::SIGKILL);
                }
            }
            self.finished = true;
        }
        Self::join_thread(&mut self.wait_thread, "pkexec 等待");
    }

    fn wait_done(&self, timeout: Duration) -> bool {
        let (lock, condvar) = &*self.done;
        let Ok(guard) = lock.lock() else {
            return true;
        };
        if *guard {
            return true;
        }
        condvar
            .wait_timeout_while(guard, timeout, |finished| !*finished)
            .map_or(true, |(guard, _)| *guard)
    }

    fn join_thread(handle: &mut Option<JoinHandle<()>>, name: &str) {
        if let Some(handle) = handle.take()
            && handle.join().is_err()
        {
            tracing::warn!("{name}线程发生 panic，已隔离");
        }
    }
}

struct WaitContext {
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    reaped: Arc<AtomicBool>,
    done: Arc<(Mutex<bool>, Condvar)>,
    events: async_channel::Sender<ControllerEvent>,
}

fn spawn_waiter(mut ctx: WaitContext) -> JoinHandle<()> {
    thread::spawn(move || {
        let wait_result = ctx.child.wait().map_err(|source| {
            BackendError::command(
                "等待 pkexec",
                CommandError::Wait {
                    command: "pkexec".to_string(),
                    source,
                },
            )
        });
        ctx.reaped.store(true, Ordering::Release);
        {
            let (lock, condvar) = &*ctx.done;
            if let Ok(mut guard) = lock.lock() {
                *guard = true;
                condvar.notify_all();
            }
        }
        PkexecProcess::join_thread(&mut ctx.stdout_thread, "pkexec 输出读取");
        PkexecProcess::join_thread(&mut ctx.stderr_thread, "pkexec 错误输出读取");

        let event = match wait_result {
            Ok(status) => {
                let output = collect_output(&ctx.stdout, &ctx.stderr);
                ControllerEvent::ProcessExited(Ok(ProcessExit { status, output }))
            }
            Err(error) => ControllerEvent::ProcessExited(Err(error)),
        };
        let _ = ctx.events.send_blocking(event);
    })
}

fn send_group_signal(pid: u32, signal: Signal) {
    if let Ok(raw_pid) = i32::try_from(pid)
        && let Err(error) = killpg(Pid::from_raw(raw_pid), signal)
    {
        tracing::debug!("向 pkexec 进程组 {raw_pid} 发送信号 {signal} 失败：{error}");
    }
}

impl Drop for PkexecProcess {
    fn drop(&mut self) {
        if !self.finished {
            self.terminate();
        }
    }
}

/// `/proc/<pid>/stat` 的 starttime（第 22 个字段，单位是开机后的时钟滴答）。
pub(super) fn process_start_time(pid: u32) -> Result<u64> {
    let path = format!("/proc/{pid}/stat");
    let stat = fs::read_to_string(&path)
        .map_err(|source| BackendError::io("读取进程启动时间", &path, source))?;
    let (_, rest) = stat.rsplit_once(')').ok_or_else(|| {
        BackendError::invalid("读取进程启动时间", "无法解析进程 stat（缺少 comm 结束符）")
    })?;
    let numeric = rest
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| BackendError::invalid("读取进程启动时间", "进程 stat 缺少 starttime"))?;
    let start_time = numeric.parse().map_err(|_| {
        BackendError::invalid("读取进程启动时间", "进程 stat 的 starttime 不是数字")
    })?;
    Ok(start_time)
}

fn spawn_output_reader(
    pipe: Option<impl Read + Send + 'static>,
    buffer: Arc<Mutex<Vec<u8>>>,
    events: async_channel::Sender<ControllerEvent>,
    stopped: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    let mut pipe = pipe?;
    Some(thread::spawn(move || {
        drain_pipe(&mut pipe, &buffer, &events, &stopped);
    }))
}

fn drain_pipe(
    pipe: &mut impl Read,
    buffer: &Arc<Mutex<Vec<u8>>>,
    events: &async_channel::Sender<ControllerEvent>,
    stopped: &AtomicBool,
) {
    let mut chunk = [0_u8; READ_CHUNK];
    let mut pending = Vec::new();
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if let Ok(mut buffer) = buffer.lock() {
                    let keep = OUTPUT_LIMIT.saturating_sub(buffer.len()).min(count);
                    buffer.extend_from_slice(&chunk[..keep]);
                }
                emit_lines(&mut pending, &chunk[..count], events, stopped);
            }
        }
    }
    flush_pending(&mut pending, events, stopped);
}

fn emit_lines(
    pending: &mut Vec<u8>,
    chunk: &[u8],
    events: &async_channel::Sender<ControllerEvent>,
    stopped: &AtomicBool,
) {
    for &byte in chunk {
        if byte == b'\n' || byte == b'\r' {
            flush_pending(pending, events, stopped);
        } else if pending.len() < 2048 {
            pending.push(byte);
        }
    }
}

fn flush_pending(
    pending: &mut Vec<u8>,
    events: &async_channel::Sender<ControllerEvent>,
    stopped: &AtomicBool,
) {
    if pending.is_empty() {
        return;
    }
    let line = String::from_utf8_lossy(pending).trim().to_string();
    pending.clear();
    if !line.is_empty() {
        send_output_event(events, stopped, line);
    }
}

/// 有界队列满时等待控制器腾出容量；只有进程终止时才放弃尚未送达的展示行。
fn send_output_event(
    events: &async_channel::Sender<ControllerEvent>,
    stopped: &AtomicBool,
    line: String,
) {
    let mut event = ControllerEvent::ProcessOutput(line);
    while !stopped.load(Ordering::Relaxed) {
        match events.try_send(event) {
            Ok(()) | Err(TrySendError::Closed(_)) => return,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(OUTPUT_RETRY_INTERVAL);
            }
        }
    }
}

fn collect_output(stdout: &Arc<Mutex<Vec<u8>>>, stderr: &Arc<Mutex<Vec<u8>>>) -> String {
    let read = |buffer: &Arc<Mutex<Vec<u8>>>| {
        buffer
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .unwrap_or_default()
    };
    let mut parts = Vec::with_capacity(2);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_start_time_of_current_process() {
        let start_time = process_start_time(std::process::id()).unwrap();
        assert!(start_time > 0);
    }

    #[test]
    fn bounded_reader_drains_without_growing_past_limit() {
        let data = vec![b'x'; OUTPUT_LIMIT.saturating_add(READ_CHUNK)];
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let (tx, _rx) = async_channel::bounded(8);
        let stopped = Arc::new(AtomicBool::new(false));
        let handle = spawn_output_reader(Some(Cursor::new(data)), Arc::clone(&buffer), tx, stopped);
        handle.unwrap().join().unwrap();
        assert_eq!(buffer.lock().unwrap().len(), OUTPUT_LIMIT);
    }

    #[test]
    fn output_reader_waits_for_capacity_without_dropping_lines() {
        let (tx, rx) = async_channel::bounded(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let handle = spawn_output_reader(
            Some(Cursor::new(b"first\nsecond\n".to_vec())),
            Arc::new(Mutex::new(Vec::new())),
            tx,
            stopped,
        );

        assert!(matches!(
            rx.recv_blocking().unwrap(),
            ControllerEvent::ProcessOutput(line) if line == "first"
        ));
        assert!(matches!(
            rx.recv_blocking().unwrap(),
            ControllerEvent::ProcessOutput(line) if line == "second"
        ));
        handle.unwrap().join().unwrap();
    }

    #[test]
    fn stopping_releases_output_reader_waiting_on_full_queue() {
        let (tx, rx) = async_channel::bounded(1);
        tx.try_send(ControllerEvent::ProcessOutput("已占用".to_string()))
            .unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let handle = spawn_output_reader(
            Some(Cursor::new(b"pending\n".to_vec())),
            Arc::new(Mutex::new(Vec::new())),
            tx,
            Arc::clone(&stopped),
        );

        thread::sleep(Duration::from_millis(10));
        stopped.store(true, Ordering::Relaxed);
        handle.unwrap().join().unwrap();
        assert!(matches!(
            rx.try_recv(),
            Ok(ControllerEvent::ProcessOutput(line)) if line == "已占用"
        ));
    }

    #[test]
    fn terminate_reaps_long_running_process() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let (events, _rx) = async_channel::bounded(8);
        let mut process = PkexecProcess::spawn_command(command, "sleep", events).unwrap();

        let started = std::time::Instant::now();
        process.terminate();

        assert!(process.reaped.load(Ordering::Acquire), "长任务必须被回收");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
