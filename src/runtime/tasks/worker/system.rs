//! 系统任务 worker：提权操作（卸载、保持版本、APT 更新与升级）与离线重打包。
//!
//! 提权操作统一经内嵌 polkit 认证会话执行，认证提示与实时输出转发为后端事件；
//! 所有系统任务在同一线程串行，避免两个 apt 事务互相抢锁。

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use async_channel::{Receiver, Sender};

use crate::backend::fs_layout::FsLayout;
use crate::backend::polkit_agent::{self, AuthResponse, InstallEvent};
use crate::backend::{Result as BackendResult, privileged, repack, shell};
use crate::runtime::events::{BackendEvent, EventSink, OperationId, RefreshScope, report_error};

use super::super::{CancelToken, InstalledPackage, SystemJob, Worker};

pub(in crate::runtime::tasks) fn spawn_system(
    rx: Receiver<SystemJob>,
    worker: Worker,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(job) = rx.recv_blocking() {
            if worker.stopping.load(Ordering::Relaxed) {
                break;
            }
            handle_system_job(job, &worker);
        }
    })
}

fn handle_system_job(job: SystemJob, worker: &Worker) {
    match job {
        SystemJob::Uninstall { id, pkg } => handle_uninstall(id, &pkg, worker),
        SystemJob::Hold { id, pkg, hold } => handle_hold(id, &pkg, hold, worker),
        SystemJob::AptUpdate { id } => handle_apt_update(id, worker),
        SystemJob::UpgradeAll { id } => handle_upgrade_all(id, worker),
        SystemJob::UpgradePackage { id, pkg } => handle_upgrade_package(id, &pkg, worker),
        SystemJob::Build {
            id,
            package,
            out_dir,
            cancel,
        } => handle_build(&BuildJob {
            id,
            package: &package,
            out_dir: &out_dir,
            cancel: &cancel,
            layout: &worker.layout,
            sink: &worker.sink,
        }),
    }
}

struct PrivilegedJobSpec {
    id: OperationId,
    args: BackendResult<Vec<String>>,
    success_message: String,
    notify: Option<(String, String)>,
    refresh: RefreshScope,
}

fn handle_uninstall(id: OperationId, pkg: &str, worker: &Worker) {
    run_privileged_job(
        PrivilegedJobSpec {
            id,
            args: privileged::uninstall_args(pkg),
            success_message: format!("软件包 {pkg} 已卸载。"),
            notify: Some(("应用已卸载".to_string(), pkg.to_string())),
            refresh: RefreshScope::Installed,
        },
        worker,
    );
}

fn handle_hold(id: OperationId, pkg: &str, hold: bool, worker: &Worker) {
    let success_message = if hold {
        format!("已保持 {pkg} 的当前版本。")
    } else {
        format!("已取消保持 {pkg} 的版本。")
    };
    let notify_body = if hold {
        format!("{pkg} 已设为保持版本")
    } else {
        format!("{pkg} 将重新跟随 APT 升级")
    };
    run_privileged_job(
        PrivilegedJobSpec {
            id,
            args: privileged::hold_args(pkg, hold),
            success_message,
            notify: Some(("版本状态已更新".to_string(), notify_body)),
            refresh: RefreshScope::Installed,
        },
        worker,
    );
}

fn handle_apt_update(id: OperationId, worker: &Worker) {
    run_privileged_job(
        PrivilegedJobSpec {
            id,
            args: Ok(privileged::apt_update_args()),
            success_message: "软件源索引已同步。".to_string(),
            notify: Some((
                "软件源已同步".to_string(),
                "APT 软件包索引已更新。".to_string(),
            )),
            refresh: RefreshScope::All,
        },
        worker,
    );
}

fn handle_upgrade_all(id: OperationId, worker: &Worker) {
    run_privileged_job(
        PrivilegedJobSpec {
            id,
            args: Ok(privileged::upgrade_all_args()),
            success_message: "全部可升级应用已更新完成。".to_string(),
            notify: Some((
                "系统更新完成".to_string(),
                "所有可升级软件包已更新。".to_string(),
            )),
            refresh: RefreshScope::Installed,
        },
        worker,
    );
}

fn handle_upgrade_package(id: OperationId, pkg: &str, worker: &Worker) {
    run_privileged_job(
        PrivilegedJobSpec {
            id,
            args: privileged::upgrade_package_args(pkg),
            success_message: format!("软件包 {pkg} 已更新到最新版本。"),
            notify: Some(("应用已更新".to_string(), pkg.to_string())),
            refresh: RefreshScope::Installed,
        },
        worker,
    );
}

struct ActiveSessionGuard<'a> {
    slot: &'a Mutex<Option<Sender<AuthResponse>>>,
}

impl<'a> ActiveSessionGuard<'a> {
    fn register(
        slot: &'a Mutex<Option<Sender<AuthResponse>>>,
        stopping: &AtomicBool,
        tx: &Sender<AuthResponse>,
    ) -> Self {
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(tx.clone());
        }
        // 登记后二次检查停机标志，避免与 `TaskManager::drop` 交叉竞态导致通道未被关闭。
        if stopping.load(Ordering::Relaxed) {
            tx.close();
        }
        Self { slot }
    }
}

impl Drop for ActiveSessionGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.slot.lock() {
            *guard = None;
        }
    }
}

/// 通过内嵌 polkit 认证会话执行提权任务，并将认证提示与输出实时转发为后端事件。
fn run_privileged_job(spec: PrivilegedJobSpec, worker: &Worker) {
    let sink = &worker.sink;
    let PrivilegedJobSpec {
        id,
        args,
        success_message,
        notify,
        refresh,
    } = spec;
    let args = match args {
        Ok(args) => args,
        Err(error) => {
            sink.send(BackendEvent::OperationFinished {
                id,
                refresh,
                result: Err(report_error(&error)),
            });
            return;
        }
    };
    let session = match polkit_agent::start_privileged(args, success_message, notify) {
        Ok(session) => session,
        Err(error) => {
            sink.send(BackendEvent::OperationFinished {
                id,
                refresh,
                result: Err(report_error(&error)),
            });
            return;
        }
    };
    let _guard = ActiveSessionGuard::register(
        &worker.active_privileged,
        &worker.stopping,
        &session.responses,
    );

    sink.send(BackendEvent::OperationSessionStarted {
        id,
        responder: session.responses.clone(),
    });
    while let Ok(event) = session.events.recv_blocking() {
        match event {
            InstallEvent::Notice(message) => {
                sink.send(BackendEvent::OperationProgress {
                    id,
                    message,
                    progress: None,
                });
            }
            InstallEvent::Prompt(prompt) => {
                sink.send(BackendEvent::OperationAuthPrompt { id, prompt });
            }
            InstallEvent::Authorized => {
                sink.send(BackendEvent::OperationAuthorized { id });
            }
            // apt 输出是低频文本行；日志列表负责保留完整过程，不节流可以避免
            // 静默期之前的最后一行被时间窗口吞掉。
            InstallEvent::Output(line) => {
                sink.send(BackendEvent::OperationOutput { id, line });
            }
            InstallEvent::Finished(result) => {
                sink.send(BackendEvent::OperationFinished {
                    id,
                    refresh,
                    result,
                });
                return;
            }
        }
    }
    sink.send(BackendEvent::OperationFinished {
        id,
        refresh,
        result: Err("提权会话意外中断。".to_string()),
    });
}

#[derive(Clone, Copy)]
struct BuildJob<'a> {
    id: OperationId,
    package: &'a InstalledPackage,
    out_dir: &'a PathBuf,
    cancel: &'a CancelToken,
    layout: &'a FsLayout,
    sink: &'a EventSink,
}

fn handle_build(job: &BuildJob<'_>) {
    let BuildJob {
        id,
        package,
        out_dir,
        cancel,
        layout,
        sink,
    } = job;
    let name = package.metadata.name.clone();
    let result = repack::build_package(repack::BuildOptions {
        package,
        layout,
        out_dir,
        cancel,
        progress: &mut |stage| {
            sink.send(BackendEvent::OperationProgress {
                id: *id,
                message: stage.to_string(),
                progress: None,
            });
        },
    })
    .map(|outcome| describe_outcome(&outcome))
    .map_err(|error| report_error(&error));
    if result.is_ok() {
        shell::notify("安装包构建完成", &name);
    }
    sink.send(BackendEvent::OperationFinished {
        id: *id,
        refresh: RefreshScope::None,
        result,
    });
}

/// 把重打包结果转成给用户看的说明。
fn describe_outcome(outcome: &repack::BuildOutcome) -> String {
    if outcome.skipped > 0 {
        format!(
            "安装包已生成：{}（共 {} 个文件）\n有 {} 个文件无法读取，已跳过。",
            outcome.path.display(),
            outcome.copied,
            outcome.skipped
        )
    } else {
        format!(
            "安装包已生成：{}（共 {} 个文件）",
            outcome.path.display(),
            outcome.copied
        )
    }
}
