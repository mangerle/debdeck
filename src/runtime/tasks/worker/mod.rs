//! 后台 worker：数据刷新、资源读取与网络下载；系统操作与自更新见子模块。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use async_channel::Receiver;

use crate::backend::desktop::load_icon_bgra;
use crate::backend::fs_layout::FsLayout;
use crate::backend::{apt, download, dpkg, shell};
use crate::runtime::events::{
    BackendEvent, CatalogUpdate, EventSink, RefreshScope, UpgradableReport, report_error,
};
use crate::util::{Throttle, current_arch};

mod system;
mod update;

pub(super) use system::spawn_system;
pub(super) use update::spawn_update;

use super::{AssetJob, CatalogJob, NetJob, ScanJob, Worker};

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
    let ok = installed.is_ok();
    sink.send(BackendEvent::CatalogUpdated(
        CatalogUpdate::default().installed(installed),
    ));
    if !ok {
        return;
    }

    // `apt list --upgradable` 需启动外部子进程，放在首屏就绪后再增量推送，避免拖慢启动页。
    let upgradable = upgradable_report();
    sink.send(BackendEvent::CatalogUpdated(
        CatalogUpdate::default().upgradable(upgradable),
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
        ..
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

pub(super) fn spawn_download(rx: Receiver<NetJob>, worker: Worker) -> JoinHandle<()> {
    thread::spawn(move || {
        let Worker { sink, .. } = worker;
        while let Ok(job) = rx.recv_blocking() {
            handle_download(job, &sink);
        }
    })
}

pub(super) fn spawn_asset(rx: Receiver<AssetJob>, worker: Worker) -> JoinHandle<()> {
    thread::spawn(move || {
        let Worker { layout, sink, .. } = worker;
        while let Ok(job) = rx.recv_blocking() {
            match job {
                AssetJob::Icon { name, max_size } => {
                    let image = load_icon_bgra(&name, max_size);
                    sink.send(BackendEvent::IconLoaded { name, image });
                }
                AssetJob::PackageFiles {
                    revision,
                    pkg,
                    arch,
                } => {
                    let files = dpkg::installed_files(&layout, &pkg, &arch).unwrap_or_default();
                    sink.send(BackendEvent::PackageFilesLoaded {
                        revision,
                        pkg,
                        files,
                    });
                }
            }
        }
    })
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
        refresh: RefreshScope::None,
        result,
    });
}
