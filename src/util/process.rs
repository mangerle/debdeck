//! 带超时与输出上限的子进程执行工具。
//!
//! 系统的 `Command::output()` 会无限等待，并把标准输出与标准错误全量读入内存。
//! 对本项目调用的 `apt`、`dpkg-deb`、`pkexec` 等外部进程而言，这两种行为都可能
//! 让界面线程或后台 worker 永久挂起，或被异常输出耗尽内存。本模块统一提供：
//!
//! * 总执行时限，超时后强制终止并回收子进程；
//! * 标准输出与标准错误的并发排空，避免管道写满互锁；
//! * 固定输出上限，超过上限的字节继续排空但不再保留。
//!
//! 代价是调用方拿不到超过上限的完整输出；本项目的命令输出只用于诊断，上限足够，
//! 因此优先保证进程可控与内存有界。项目只支持 Linux，进程组与信号处理直接使用
//! Unix/Linux 语义，不再声明与支持目标不符的跨平台条件分支。

use std::io::{Error as IoError, Read, Result as IoResult};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

/// 单条命令保留的最大输出量。
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// 标准输出与标准错误的单次读取块大小。
const READ_CHUNK: usize = 16 * 1024;
/// 子进程状态轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 受控子进程执行错误。
#[derive(Debug, Error)]
pub enum CommandError {
    /// 子进程无法启动。
    #[error("无法启动命令 {command}：{source}")]
    Spawn {
        /// 命令名或路径。
        command: String,
        /// 操作系统返回的启动错误。
        #[source]
        source: IoError,
    },
    /// 等待子进程状态失败。
    #[error("等待命令 {command} 失败：{source}")]
    Wait {
        /// 命令名或路径。
        command: String,
        /// 操作系统返回的等待错误。
        #[source]
        source: IoError,
    },
    /// 命令超过执行时限，已被强制终止。
    #[error("命令 {command} 在 {timeout_ms} 毫秒内未完成，已强制终止")]
    Timeout {
        /// 命令名或路径。
        command: String,
        /// 调用方设置的超时时长。
        timeout_ms: u128,
    },
    /// 读取子进程输出失败。
    #[error("读取命令 {command} 的输出失败：{source}")]
    Read {
        /// 命令名或路径。
        command: String,
        /// 管道读取错误。
        #[source]
        source: IoError,
    },
    /// 输出读取线程发生 panic。
    #[error("回收命令 {command} 的输出线程失败")]
    ReaderPanic {
        /// 命令名或路径。
        command: String,
    },
}

/// 执行命令并返回受控的标准输出结构。
///
/// # Errors
///
/// 命令无法启动、等待状态失败、超过 `timeout` 或被强制终止时返回对应错误；
/// 标准输出或标准错误读取失败时同样返回错误。
pub fn run_command(command: &mut Command, timeout: Duration) -> Result<Output, CommandError> {
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // 让子进程成为独立进程组组长；超时时可以连同 shell 派生的后代一起终止，
    // 避免只杀父进程后管道仍被孙进程持有，读取线程继续等待。
    command.process_group(0);

    let mut child = command.spawn().map_err(|source| CommandError::Spawn {
        command: program.clone(),
        source,
    })?;
    let stdout_handle = child.stdout.take().map(spawn_reader);
    let stderr_handle = child.stderr.take().map(spawn_reader);
    let status = match poll_exit(&mut child, timeout, &program) {
        Ok(status) => status,
        Err(error) => {
            let _ = join_reader(stdout_handle, &program);
            let _ = join_reader(stderr_handle, &program);
            return Err(error);
        }
    };

    let stdout = join_reader(stdout_handle, &program)?;
    let stderr = join_reader(stderr_handle, &program)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// 终止子进程所在进程组并回收组长进程。
///
/// 调用前应确保子进程通过 `process_group(0)` 成为独立进程组组长；否则该函数
/// 只会回收当前子进程，无法终止其后台后代。
pub(crate) fn terminate_child(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// 在时限内等待子进程退出；超时或等待失败时终止并回收进程组。
///
/// # Errors
///
/// 等待系统调用失败、进程超时或回收失败时返回对应错误。
pub(crate) fn wait_child(child: &mut Child, timeout: Duration) -> Result<ExitStatus, CommandError> {
    let program = format!("进程 {}", child.id());
    poll_exit(child, timeout, &program)
}

/// 轮询退出状态；超时或等待失败时强制终止进程组。
fn poll_exit(
    child: &mut Child,
    timeout: Duration,
    program: &str,
) -> Result<ExitStatus, CommandError> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                terminate_child(child);
                return Err(CommandError::Timeout {
                    command: program.to_string(),
                    timeout_ms: timeout.as_millis(),
                });
            }
            Err(source) => {
                terminate_child(child);
                return Err(CommandError::Wait {
                    command: program.to_string(),
                    source,
                });
            }
        }
    }
}

fn spawn_reader<R>(mut stream: R) -> thread::JoinHandle<IoResult<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_bounded(&mut stream))
}

fn read_bounded(stream: &mut impl Read) -> IoResult<Vec<u8>> {
    let mut output = Vec::with_capacity(READ_CHUNK);
    let mut chunk = [0_u8; READ_CHUNK];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(output);
        }
        let keep = OUTPUT_LIMIT.saturating_sub(output.len()).min(read);
        output.extend_from_slice(&chunk[..keep]);
    }
}

fn join_reader(
    handle: Option<thread::JoinHandle<IoResult<Vec<u8>>>>,
    program: &str,
) -> Result<Vec<u8>, CommandError> {
    let Some(handle) = handle else {
        return Ok(Vec::new());
    };
    handle
        .join()
        .map_err(|_| CommandError::ReaderPanic {
            command: program.to_string(),
        })?
        .map_err(|source| CommandError::Read {
            command: program.to_string(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout_stderr_and_exit_status() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf stdout; printf stderr >&2; exit 3"]);
        let output = run_command(&mut command, Duration::from_secs(5)).unwrap();
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"stderr");
        assert_eq!(output.status.code(), Some(3));
    }

    #[test]
    fn kills_command_after_timeout() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 2"]);
        let error = run_command(&mut command, Duration::from_millis(50)).unwrap_err();
        assert!(matches!(error, CommandError::Timeout { .. }));
    }
}
