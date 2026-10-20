//! 任务编排：把可能阻塞的工作移出 UI 线程，并把结果变成事件。
//!
//! # 四条队列，各有一个明确理由
//!
//! | 队列 | 工作 | 为什么单独一条 |
//! |---|---|---|
//! | catalog | 读 dpkg status、查可升级集合 | 首屏最关键的等待，必须能与软件源扫描并行 |
//! | scan | 解析 `/var/lib/apt/lists` | 解压数百 MB 索引，不该挡住已安装列表的刷新 |
//! | system | pkexec 卸载/保持版本、离线重打包 | 提权操作必须串行，避免两个 apt 事务互相抢锁 |
//! | net | 下载安装包 | 长耗时网络操作，不该拖住本地打包 |
//!
//! 每条队列一个专属的任务类型（`CatalogJob`/`ScanJob`/`SystemJob`/`NetJob`），
//! 而不是一个大而全的 `Command`：这样「把下载任务投递给打包线程」在编译期就不可
//! 表示，worker 也不需要为不可能到达的分支写兜底。
//!
//! # 排队去重
//!
//! 已安装刷新与软件源刷新在**排队期间**幂等：重复请求不会把同一份解析工作排两次。
//! worker 取出任务时立即清除标记，因此「执行期间到达的新请求」会再排一次队 ——
//! 系统状态可能已经变了，宁可多算一次也不能漏。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::backend::{apt, download, dpkg, fs_layout::FsLayout, privileged, repack, shell};
use crate::model::InstalledPackage;
use crate::runtime::events::{
    BackendEvent, CatalogUpdate, EventSink, OperationId, UpgradableReport, Waker, report,
};
use crate::runtime::watcher::Watcher;
use crate::util::{Throttle, current_arch};

/// 取消标志：worker 在检查点读取。
pub type CancelToken = Arc<AtomicBool>;

/// 下载进度事件的最小间隔：字节回调每 128 KiB 一次，不节流会淹掉界面。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(80);

/// 一次在线下载的请求。
pub struct DownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    pub package_name: String,
    /// 软件源给出的校验和；缺失或空串时跳过校验。
    pub sha256: Option<String>,
}

#[derive(Clone, Copy)]
enum CatalogJob {
    /// 首屏读取（带阶段提示）。
    Load,
    /// 外部变更后的重新读取。
    Refresh,
}

#[derive(Clone, Copy)]
enum ScanJob {
    Rescan,
}

enum SystemJob {
    Uninstall {
        id: OperationId,
        pkg: String,
    },
    Hold {
        id: OperationId,
        pkg: String,
        hold: bool,
    },
    Build {
        id: OperationId,
        package: Box<InstalledPackage>,
        out_dir: PathBuf,
        cancel: CancelToken,
    },
}

struct NetJob {
    id: OperationId,
    request: DownloadRequest,
    cancel: CancelToken,
}

/// worker 线程共享的执行环境。
#[derive(Clone)]
struct Worker {
    layout: FsLayout,
    native_arch: String,
    sink: EventSink,
}

/// UI 侧唯一的任务入口。
pub struct TaskManager {
    next_id: AtomicU64,
    catalog_tx: Sender<CatalogJob>,
    scan_tx: Sender<ScanJob>,
    system_tx: Sender<SystemJob>,
    net_tx: Sender<NetJob>,
    /// 排队去重标记，与对应队列一起交给 worker。
    pending_installed: Arc<AtomicBool>,
    pending_repository: Arc<AtomicBool>,
    /// 丢弃本管理器时停止状态监视线程。
    _watcher: Watcher,
}

impl TaskManager {
    /// 启动全部 worker 线程，返回管理器与事件接收端。
    pub fn start(wake: Waker, layout: FsLayout) -> (Self, Receiver<BackendEvent>) {
        let (event_tx, event_rx) = mpsc::channel();
        let worker = Worker {
            layout,
            native_arch: current_arch(),
            sink: EventSink::new(event_tx, wake),
        };

        let (catalog_tx, catalog_rx) = mpsc::channel();
        let (scan_tx, scan_rx) = mpsc::channel();
        let (system_tx, system_rx) = mpsc::channel();
        let (net_tx, net_rx) = mpsc::channel();

        let pending_installed = Arc::new(AtomicBool::new(false));
        let pending_repository = Arc::new(AtomicBool::new(false));

        spawn_catalog(catalog_rx, worker.clone(), Arc::clone(&pending_installed));
        spawn_scan(scan_rx, worker.clone(), Arc::clone(&pending_repository));
        spawn_system(system_rx, worker.clone());
        spawn_download(net_rx, worker.clone());
        let watcher = Watcher::start(worker.layout.clone(), worker.sink.clone());

        let manager = Self {
            next_id: AtomicU64::new(1),
            catalog_tx,
            scan_tx,
            system_tx,
            net_tx,
            pending_installed,
            pending_repository,
            _watcher: watcher,
        };
        (manager, event_rx)
    }

    fn next_id(&self) -> OperationId {
        OperationId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// 排队去重地投递任务：标记已置位说明同样的工作还在队列里，忽略本次请求。
    fn enqueue<T>(tx: &Sender<T>, pending: &AtomicBool, job: T) {
        if !pending.swap(true, Ordering::Relaxed) {
            let _ = tx.send(job);
        }
    }

    /// 首屏加载：已安装数据与软件源索引并行读取。
    pub fn load_initial(&self) {
        Self::enqueue(&self.catalog_tx, &self.pending_installed, CatalogJob::Load);
        Self::enqueue(&self.scan_tx, &self.pending_repository, ScanJob::Rescan);
    }

    pub fn refresh_installed(&self) {
        Self::enqueue(
            &self.catalog_tx,
            &self.pending_installed,
            CatalogJob::Refresh,
        );
    }

    pub fn refresh_repository(&self) {
        Self::enqueue(&self.scan_tx, &self.pending_repository, ScanJob::Rescan);
    }

    /// 提权与打包类任务不参与去重：每次调用都必须真的执行一次。
    pub fn uninstall(&self, pkg: String) -> OperationId {
        let id = self.next_id();
        let _ = self.system_tx.send(SystemJob::Uninstall { id, pkg });
        id
    }

    pub fn hold(&self, pkg: String, hold: bool) -> OperationId {
        let id = self.next_id();
        let _ = self.system_tx.send(SystemJob::Hold { id, pkg, hold });
        id
    }

    /// 返回（任务号，取消标志）。需要取消的操作共用同一段分配逻辑。
    fn cancellable(&self) -> (OperationId, CancelToken) {
        (self.next_id(), Arc::new(AtomicBool::new(false)))
    }

    pub fn build(&self, package: InstalledPackage, out_dir: PathBuf) -> (OperationId, CancelToken) {
        let (id, cancel) = self.cancellable();
        let _ = self.system_tx.send(SystemJob::Build {
            id,
            package: Box::new(package),
            out_dir,
            cancel: Arc::clone(&cancel),
        });
        (id, cancel)
    }

    pub fn download(&self, request: DownloadRequest) -> (OperationId, CancelToken) {
        let (id, cancel) = self.cancellable();
        let _ = self.net_tx.send(NetJob {
            id,
            request,
            cancel: Arc::clone(&cancel),
        });
        (id, cancel)
    }
}

/// 询问 APT 可升级集合。失败不致命，但要留下日志并让上层知道这是降级判定。
fn upgradable_report() -> UpgradableReport {
    match apt::list_upgradable() {
        Ok(packages) => UpgradableReport::Queried(packages),
        Err(error) => {
            tracing::warn!("查询可升级软件包失败，退回本地版本比较：{}", report(&error));
            UpgradableReport::Failed
        }
    }
}

fn spawn_catalog(rx: Receiver<CatalogJob>, worker: Worker, pending: Arc<AtomicBool>) {
    thread::spawn(move || {
        let Worker { layout, sink, .. } = worker;
        while let Ok(job) = rx.recv() {
            // 立即释放标记：执行期间到达的新请求应当再排一次队。
            pending.store(false, Ordering::Relaxed);
            if matches!(job, CatalogJob::Load) {
                sink.send(BackendEvent::Stage("正在读取已安装应用…".to_string()));
            }

            // 每个包一段进度，闭包只负责把它转成事件。
            let mut on_stage = {
                let sink = sink.clone();
                move |stage: &str| sink.send(BackendEvent::Stage(stage.to_string()))
            };
            let installed =
                dpkg::load_installed(&layout, &mut on_stage).map_err(|error| report(&error));
            let upgradable = upgradable_report();

            sink.send(BackendEvent::CatalogUpdated(
                CatalogUpdate::default()
                    .installed(installed)
                    .upgradable(upgradable),
            ));
        }
    });
}

fn spawn_scan(rx: Receiver<ScanJob>, worker: Worker, pending: Arc<AtomicBool>) {
    thread::spawn(move || {
        let Worker {
            layout,
            native_arch,
            sink,
        } = worker;
        while rx.recv().is_ok() {
            pending.store(false, Ordering::Relaxed);
            sink.send(BackendEvent::Stage("正在解析 APT 软件源…".to_string()));
            let repository = apt::load_repository(&layout, &native_arch);
            sink.send(BackendEvent::CatalogUpdated(
                CatalogUpdate::default().repository(repository),
            ));
        }
    });
}

fn spawn_system(rx: Receiver<SystemJob>, worker: Worker) {
    thread::spawn(move || {
        let Worker { layout, sink, .. } = worker;
        while let Ok(job) = rx.recv() {
            match job {
                SystemJob::Uninstall { id, pkg } => {
                    let result = privileged::uninstall_package(&pkg)
                        .map(|()| format!("软件包 {pkg} 已卸载。"))
                        .map_err(|error| report(&error));
                    if result.is_ok() {
                        shell::notify("应用已卸载", &pkg);
                    }
                    sink.send(BackendEvent::OperationFinished {
                        id,
                        refresh_installed: true,
                        result,
                    });
                }
                SystemJob::Hold { id, pkg, hold } => {
                    let result = privileged::set_hold_state(&pkg, hold)
                        .map(|()| {
                            if hold {
                                format!("已保持 {pkg} 的当前版本。")
                            } else {
                                format!("已取消保持 {pkg} 的版本。")
                            }
                        })
                        .map_err(|error| report(&error));
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
                SystemJob::Build {
                    id,
                    package,
                    out_dir,
                    cancel,
                } => {
                    let name = package.metadata.name.clone();
                    let result =
                        repack::build_package(&package, &layout, &out_dir, &cancel, &mut |stage| {
                            sink.send(BackendEvent::OperationProgress {
                                id,
                                message: stage.to_string(),
                                progress: None,
                            });
                        })
                        .map(|outcome| describe_outcome(&outcome))
                        .map_err(|error| report(&error));
                    if result.is_ok() {
                        shell::notify("安装包构建完成", &name);
                    }
                    sink.send(BackendEvent::OperationFinished {
                        id,
                        refresh_installed: false,
                        result,
                    });
                }
            }
        }
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

fn spawn_download(rx: Receiver<NetJob>, worker: Worker) {
    thread::spawn(move || {
        let Worker { sink, .. } = worker;
        while let Ok(job) = rx.recv() {
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
            let result = download::download_file(
                &request.url,
                &request.destination,
                &cancel,
                request.sha256.as_deref(),
                |received, total| {
                    // 最后一个字节必须发送，否则进度条停在 99%。
                    if received == total || throttle.touch() {
                        sink.send(BackendEvent::OperationProgress {
                            id,
                            message: "正在下载安装包…".to_string(),
                            progress: Some((received, total)),
                        });
                    }
                },
            )
            .map(|path| format!("安装包已保存到：{}", path.display()))
            .map_err(|error| report(&error));

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
    });
}
