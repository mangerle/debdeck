//! 后台 worker：数据刷新、系统操作与网络下载。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use async_channel::Receiver;

use crate::backend::desktop::load_icon_bgra;
use crate::backend::fs_layout::FsLayout;
use crate::backend::self_update::{
    DownloadedUpdate, UpdateEvent, UpdateInfo, UpdateStage, check_update, confirm_update_success,
};
use crate::backend::{apt, download, dpkg, privileged, repack, shell};
use crate::runtime::events::{
    BackendEvent, CatalogUpdate, EventSink, OperationId, UpgradableReport, report_error,
};
use crate::util::{Throttle, current_arch, format_bytes};

use super::{
    CancelToken, CatalogJob, IconJob, InstalledPackage, NetJob, ScanJob, SystemJob, UpdateJob,
    Worker,
};

/// 下载进度事件的最小间隔：字节回调每 128 KiB 一次，不节流会淹掉界面。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);

/// 询问 APT 可升级集合。失败不致命，但要留下日志并让上层知道这是降级判定。
fn upgradable_report() -> UpgradableReport {
    match apt::list_upgradable() {
        Ok(packages) => UpgradableReport::Queried(packages),
        Err(error) => {
            tracing::warn!(
                "查询可升级软件包失败，退回本地版本比较：{}",
                report_error(&error)
            );
            UpgradableReport::Failed
        }
    }
}

pub(super) fn spawn_catalog(
    rx: Receiver<CatalogJob>,
    worker: Worker,
    pending: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || run_catalog_worker(rx, worker, pending))
}

pub(super) fn spawn_scan(
    rx: Receiver<ScanJob>,
    worker: Worker,
    pending: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || run_scan_worker(rx, worker, pending))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "线程闭包必须取得队列接收端与共享标记的所有权"
)]
fn run_catalog_worker(rx: Receiver<CatalogJob>, worker: Worker, pending: Arc<AtomicBool>) {
    let Worker { layout, sink, .. } = worker;
    while let Ok(job) = rx.recv_blocking() {
        pending.store(false, Ordering::Relaxed);
        if matches!(job, CatalogJob::Load) {
            sink.send(BackendEvent::Stage("正在读取已安装应用…".to_string()));
        }
        send_catalog_update(&layout, &sink);
    }
}

fn send_catalog_update(layout: &FsLayout, sink: &EventSink) {
    let mut on_stage = |stage: &str| sink.send(BackendEvent::Stage(stage.to_string()));
    let installed =
        dpkg::load_installed(layout, &mut on_stage).map_err(|error| report_error(&error));
    let upgradable = upgradable_report();
    sink.send(BackendEvent::CatalogUpdated(
        CatalogUpdate::default()
            .installed(installed)
            .upgradable(upgradable),
    ));
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "线程闭包必须取得队列接收端与共享标记的所有权"
)]
fn run_scan_worker(rx: Receiver<ScanJob>, worker: Worker, pending: Arc<AtomicBool>) {
    let Worker {
        layout,
        native_arch,
        sink,
    } = worker;
    while rx.recv_blocking().is_ok() {
        pending.store(false, Ordering::Relaxed);
        sink.send(BackendEvent::Stage("正在解析 APT 软件源…".to_string()));
        let native_arch = native_arch.get_or_init(current_arch);
        let repository = apt::load_repository(&layout, native_arch);
        sink.send(BackendEvent::CatalogUpdated(
            CatalogUpdate::default().repository(repository),
        ));
    }
}

pub(super) fn spawn_system(rx: Receiver<SystemJob>, worker: Worker) -> JoinHandle<()> {
    thread::spawn(move || {
        let Worker { layout, sink, .. } = worker;
        while let Ok(job) = rx.recv_blocking() {
            handle_system_job(job, &layout, &sink);
        }
    })
}

fn handle_system_job(job: SystemJob, layout: &FsLayout, sink: &EventSink) {
    match job {
        SystemJob::Uninstall { id, pkg } => handle_uninstall(id, &pkg, sink),
        SystemJob::Hold { id, pkg, hold } => handle_hold(id, &pkg, hold, sink),
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
            layout,
            sink,
        }),
    }
}

fn handle_uninstall(id: OperationId, pkg: &str, sink: &EventSink) {
    let result = privileged::uninstall_package(pkg)
        .map(|()| format!("软件包 {pkg} 已卸载。"))
        .map_err(|error| report_error(&error));
    if result.is_ok() {
        shell::notify("应用已卸载", pkg);
    }
    sink.send(BackendEvent::OperationFinished {
        id,
        refresh_installed: true,
        result,
    });
}

fn handle_hold(id: OperationId, pkg: &str, hold: bool, sink: &EventSink) {
    let result = privileged::set_hold_state(pkg, hold)
        .map(|()| {
            if hold {
                format!("已保持 {pkg} 的当前版本。")
            } else {
                format!("已取消保持 {pkg} 的版本。")
            }
        })
        .map_err(|error| report_error(&error));
    if result.is_ok() {
        let body = if hold {
            format!("{pkg} 已设为保持版本")
        } else {
            format!("{pkg} 将重新跟随 APT 升级")
        };
        shell::notify("版本状态已更新", &body);
    }
    sink.send(BackendEvent::OperationFinished {
        id,
        refresh_installed: true,
        result,
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
        refresh_installed: false,
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

pub(super) fn spawn_download(rx: Receiver<NetJob>, worker: Worker) -> JoinHandle<()> {
    thread::spawn(move || {
        let Worker { sink, .. } = worker;
        while let Ok(job) = rx.recv_blocking() {
            handle_download(job, &sink);
        }
    })
}

pub(super) fn spawn_update(rx: Receiver<UpdateJob>, worker: Worker) -> JoinHandle<()> {
    thread::spawn(move || {
        let Worker { sink, .. } = worker;
        while let Ok(job) = rx.recv_blocking() {
            handle_update_job(job, &sink);
        }
    })
}

pub(super) fn spawn_icon(rx: Receiver<IconJob>, worker: Worker) -> JoinHandle<()> {
    thread::spawn(move || {
        let Worker { sink, .. } = worker;
        while let Ok(job) = rx.recv_blocking() {
            let image = load_icon_bgra(&job.name, job.max_size);
            sink.send(BackendEvent::IconLoaded {
                name: job.name,
                image,
            });
        }
    })
}

fn handle_update_job(job: UpdateJob, sink: &EventSink) {
    match job {
        UpdateJob::Check => handle_update_check(sink),
        UpdateJob::Download(update) => handle_update_download(&update, sink),
        UpdateJob::Install(downloaded) => handle_update_install(&downloaded, sink),
        UpdateJob::ConfirmSuccess => {
            if let Err(error) = confirm_update_success() {
                tracing::warn!("确认自更新成功状态失败：{error}");
            }
        }
    }
}

fn handle_update_check(sink: &EventSink) {
    let result = check_update(None).map_err(|error| report_error(&error));
    sink.send(BackendEvent::SelfUpdateChecked(result));
}

fn handle_update_download(update: &UpdateInfo, sink: &EventSink) {
    let result = update
        .download_blocking(|event| sink.send(update_progress_event(event)))
        .map_err(|error| report_error(&error));
    sink.send(BackendEvent::SelfUpdateDownloaded(result));
}

fn handle_update_install(downloaded: &DownloadedUpdate, sink: &EventSink) {
    let result = downloaded
        .install(|event| sink.send(update_progress_event(event)))
        .map_err(|error| report_error(&error));
    sink.send(BackendEvent::SelfUpdateInstalled(result));
}

/// 把自更新事件压缩成一条界面消息；下载进度直接透传百分比。
fn update_progress_event(event: UpdateEvent) -> BackendEvent {
    let (percent, message) = match event {
        UpdateEvent::DownloadProgress {
            percent,
            speed_bytes_per_sec,
            ..
        } => {
            let speed = speed_bytes_per_sec
                .map_or_else(String::new, |bytes| format!("，{}/s", format_bytes(bytes)));
            (percent, format!("正在下载并校验更新包{speed}"))
        }
        UpdateEvent::Stage(stage) => (None, update_stage_label(stage).to_string()),
        UpdateEvent::Retrying {
            attempt,
            max_retries,
            ..
        } => (
            None,
            format!("网络请求失败，正在重试（{attempt}/{max_retries}）"),
        ),
        UpdateEvent::Failed { reason } => (None, format!("更新失败：{reason}")),
    };
    BackendEvent::SelfUpdateDownloadProgress { percent, message }
}

fn update_stage_label(stage: UpdateStage) -> &'static str {
    match stage {
        UpdateStage::CheckingManifest => "正在检查更新清单",
        UpdateStage::DownloadStarted => "开始下载更新包",
        UpdateStage::VerifyingChecksum => "正在校验更新包完整性",
        UpdateStage::VerifyingSignature => "正在校验更新签名",
        UpdateStage::ExtractingArchive => "正在解压更新包",
        UpdateStage::Installing => "正在安装新版本",
        UpdateStage::ReadyToRestart => "新版本已就绪，等待重启",
        UpdateStage::DeferredToReboot => "更新已推迟到系统重启后生效",
        UpdateStage::Completed => "更新安装完成",
    }
}

fn handle_download(job: NetJob, sink: &EventSink) {
    let NetJob {
        id,
        request,
        cancel,
    } = job;
    let mut throttle = Throttle::new(PROGRESS_INTERVAL);
    sink.send(BackendEvent::OperationProgress {
        id,
        message: "正在连接软件源…".to_string(),
        progress: Some((0, 0)),
    });
    let destination = request.destination.clone();
    let package_name = request.package_name.clone();
    let result = download::download_file(download::DownloadOptions {
        url: &request.url,
        destination: &request.destination,
        cancel: &cancel,
        expected_sha256: request.sha256.as_deref(),
        progress: |received, total| {
            // 最后一个字节必须发送，否则进度条停在 99%。
            if received == total || throttle.touch() {
                sink.send(BackendEvent::OperationProgress {
                    id,
                    message: "正在下载安装包…".to_string(),
                    progress: Some((received, total)),
                });
            }
        },
    })
    .map(|path| format!("安装包已保存到：{}", path.display()))
    .map_err(|error| report_error(&error));
    if result.is_ok() {
        let name = destination
            .file_name()
            .map_or(package_name, |name| name.to_string_lossy().to_string());
        shell::notify("安装包下载完成", &name);
    }
    sink.send(BackendEvent::OperationFinished {
        id,
        refresh_installed: false,
        result,
    });
}
