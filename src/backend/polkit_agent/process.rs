//! `pkexec` 子进程与输出读取。
//!
//! 安装可能在认证后持续数分钟，不能复用一次性命令工具；这里保留交互期间的
//! 子进程句柄，但仍在析构时强制终止并回收，且对 stdout/stderr 设置固定上限。

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::backend::{BackendError, Result};
use std::os::unix::process::CommandExt;

use crate::util::{CommandError, terminate_child};

/// 单个输出管道保留的最大字节数。
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// 单次读取块大小。
const READ_CHUNK: usize = 4096;

/// 已结束的 `pkexec` 进程。
pub(super) struct ProcessExit {
    /// 子进程退出状态。
    pub status: ExitStatus,
    /// stdout 与 stderr 合并后的文本。
    pub output: String,
}

/// 运行中的 `pkexec` 及其输出读取线程。
pub(super) struct PkexecProcess {
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    finished: bool,
}

impl PkexecProcess {
    /// 启动 pkexec。父进程是本安装进程，正好与注册代理时的 subject 对应。
    pub(super) fn start(args: &[String]) -> Result<Self> {
        let pkexec = "/usr/bin/pkexec";
        if !Path::new(pkexec).is_file() {
            return Err(BackendError::missing_program("启动管理员认证", "pkexec"));
        }
        let mut command = Command::new(pkexec);
        command
            .arg("--disable-internal-agent")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().map_err(|source| {
            BackendError::command(
                "启动 pkexec",
                CommandError::Spawn {
                    command: pkexec.to_string(),
                    source,
                },
            )
        })?;
        let stdout = Arc::new(Mutex::new(Vec::with_capacity(64 * 1024)));
        let stderr = Arc::new(Mutex::new(Vec::with_capacity(64 * 1024)));
        let stdout_thread = spawn_output_reader(child.stdout.take(), Arc::clone(&stdout));
        let stderr_thread = spawn_output_reader(child.stderr.take(), Arc::clone(&stderr));
        Ok(Self {
            child,
            stdout,
            stderr,
            stdout_thread,
            stderr_thread,
            finished: false,
        })
    }

    /// 查询子进程是否退出；退出时等待输出线程并返回完整结果。
    pub(super) fn poll(&mut self) -> Result<Option<ProcessExit>> {
        match self.child.try_wait().map_err(|source| {
            BackendError::command(
                "等待 pkexec",
                CommandError::Wait {
                    command: "pkexec".to_string(),
                    source,
                },
            )
        })? {
            Some(status) => {
                self.finished = true;
                let output = self.finish_output();
                Ok(Some(ProcessExit { status, output }))
            }
            None => Ok(None),
        }
    }

    /// 终止进程组并等待所有输出线程结束。
    pub(super) fn terminate(&mut self) {
        if !self.finished {
            terminate_child(&mut self.child);
            self.finished = true;
        }
        Self::join_reader(&mut self.stdout_thread);
        Self::join_reader(&mut self.stderr_thread);
    }

    fn finish_output(&mut self) -> String {
        Self::join_reader(&mut self.stdout_thread);
        Self::join_reader(&mut self.stderr_thread);
        collect_output(&self.stdout, &self.stderr)
    }

    fn join_reader(handle: &mut Option<JoinHandle<()>>) {
        if let Some(handle) = handle.take()
            && handle.join().is_err()
        {
            tracing::warn!("pkexec 输出读取线程发生 panic，已隔离");
        }
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
) -> Option<JoinHandle<()>> {
    let mut pipe = pipe?;
    Some(thread::spawn(move || drain_pipe(&mut pipe, &buffer)))
}

fn drain_pipe(pipe: &mut impl Read, buffer: &Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0_u8; READ_CHUNK];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if let Ok(mut buffer) = buffer.lock() {
                    let keep = OUTPUT_LIMIT.saturating_sub(buffer.len()).min(count);
                    buffer.extend_from_slice(&chunk[..keep]);
                }
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
        let handle = spawn_output_reader(Some(Cursor::new(data)), Arc::clone(&buffer));
        handle.unwrap().join().unwrap();
        assert_eq!(buffer.lock().unwrap().len(), OUTPUT_LIMIT);
    }
}
