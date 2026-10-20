//! 启动桌面条目对应的程序。

use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::model::DesktopEntry;
use crate::util::{command_exists, desktop_exec_args, home_dir};

/// 在后台启动应用。
///
/// 子进程被 detach：stdout/stderr 丢弃、脱离 stdin，并在独立线程里 `wait`
/// 回收，避免应用退出后 DebDeck 里堆积僵尸进程。
pub fn launch_desktop(entry: &DesktopEntry) -> Result<()> {
    let args = desktop_exec_args(&entry.exec);
    if args.is_empty() {
        bail!("桌面文件中没有有效的启动命令");
    }

    let mut command = if entry.terminal {
        terminal_command(&args)
    } else {
        let mut command = Command::new(&args[0]);
        command.args(&args[1..]);
        command
    };
    command
        .current_dir(home_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .with_context(|| format!("无法启动 {}", args[0]))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// 为 `Terminal=true` 的条目挑一个可用终端。
///
/// 优先 `xdg-terminal-exec`（新标准），再退回常见终端的 `-e` 语法；
/// 都没有时直接执行目标命令（会丢失终端，但不至于什么都发生不了）。
fn terminal_command(args: &[String]) -> Command {
    const CANDIDATES: [(&str, Option<&str>); 3] = [
        ("xdg-terminal-exec", None),
        ("gnome-terminal", Some("--")),
        ("konsole", Some("-e")),
    ];
    for (program, separator) in CANDIDATES {
        if command_exists(program) {
            let mut command = Command::new(program);
            if let Some(separator) = separator {
                command.arg(separator);
            }
            command.args(args);
            return command;
        }
    }
    let mut command = Command::new(&args[0]);
    command.args(&args[1..]);
    command
}
