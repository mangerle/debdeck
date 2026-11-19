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
//! | update | 检查、下载与安装 DebDeck 自身版本 | 自更新链路与普通包下载隔离，避免覆盖运行中的二进制时抢同一 worker |
//!
//! 每条队列一个专属的任务类型（`CatalogJob`/`ScanJob`/`SystemJob`/`NetJob`/`UpdateJob`），
//! 而不是一个大而全的 `Command`：这样「把下载任务投递给打包线程」在编译期就不可
//! 表示，worker 也不需要为不可能到达的分支写兜底。
//!
//! # 排队去重
//!
//! 已安装刷新与软件源刷新在**排队期间**幂等：重复请求不会把同一份解析工作排两次。
//! worker 取出任务时立即清除标记，因此「执行期间到达的新请求」会再排一次队 ——
//! 系统状态可能已经变了，宁可多算一次也不能漏。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use async_channel::{Sender, TrySendError};

use crate::backend::fs_layout::FsLayout;
use crate::backend::self_update::{DownloadedUpdate, UpdateInfo};
use crate::model::InstalledPackage;
use crate::runtime::events::{EVENT_QUEUE_CAPACITY, EventReceiver, EventSink, OperationId, Waker};
use crate::runtime::watcher::Watcher;

mod worker;

/// 取消标志：worker 在检查点读取。
pub type CancelToken = Arc<AtomicBool>;

/// 首屏与刷新队列容量。两类刷新在排队期间去重，因此 2 个槽位足够覆盖
/// 「执行中又到达一次刷新」的最小需求。
const CATALOG_QUEUE_CAPACITY: usize = 2;
const SCAN_QUEUE_CAPACITY: usize = 2;
/// 提权与重打包必须串行执行；容量只用于短时排队，不允许无限堆积点击。
const SYSTEM_QUEUE_CAPACITY: usize = 8;
/// 并行下载任务上限。界面同一时间只展示一个操作，但保留少量排队余量。
const NET_QUEUE_CAPACITY: usize = 4;
/// 自更新必须在同一时刻只有一个任务在途；额外槽位只承接检查、下载、安装的
/// 短时排队，界面状态机会阻止重复点击，容量仍然有界。
const UPDATE_QUEUE_CAPACITY: usize = 4;
/// 轻量按需资源队列（图标解码与单包文件清单读取）；超出直接丢弃，避免阻塞主线程。
const ASSET_QUEUE_CAPACITY: usize = 32;

/// 任务提交失败。有界队列满时快速失败，由界面明确提示而不是静默丢任务。
#[derive(Debug, thiserror::Error)]
pub(crate) enum TaskSubmitError {
    #[error("{task}队列已满，请等待现有任务完成后再试")]
    Full { task: &'static str },
    #[error("无法提交{task}：后台任务已停止")]
    Closed { task: &'static str },
}

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
    AptUpdate {
        id: OperationId,
    },
    UpgradeAll {
        id: OperationId,
    },
    UpgradePackage {
        id: OperationId,
        pkg: String,
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

/// DebDeck 自身版本更新任务。
enum UpdateJob {
    /// 检查远端清单并验签。
    Check,
    /// 下载并验签更新包。
    Download(UpdateInfo),
    /// 安装已下载的更新包。
    Install(DownloadedUpdate),
    /// 主窗口稳定运行后确认更新成功，清理崩溃观察期状态。
    ConfirmSuccess,
}

/// 轻量按需资源任务：图标解码或单包文件清单读取。
enum AssetJob {
    Icon {
        name: String,
        max_size: u32,
    },
    PackageFiles {
        revision: usize,
        pkg: String,
        arch: String,
    },
}

/// worker 线程共享的执行环境。
#[derive(Clone)]
struct Worker {
    layout: FsLayout,
    native_arch: Arc<OnceLock<String>>,
    sink: EventSink,
}

/// UI 侧唯一的任务入口。
pub struct TaskManager {
    next_id: AtomicU64,
    catalog_tx: Sender<CatalogJob>,
    scan_tx: Sender<ScanJob>,
    system_tx: Sender<SystemJob>,
    net_tx: Sender<NetJob>,
    update_tx: Sender<UpdateJob>,
    asset_tx: Sender<AssetJob>,
    /// 排队去重标记，与对应队列一起交给 worker。
    pending_installed: Arc<AtomicBool>,
    pending_repository: Arc<AtomicBool>,
    /// 后台 worker 句柄，退出时统一等待并检查 panic。
    workers: Vec<JoinHandle<()>>,
    /// 停机标记：事件发送端据此停止重试，避免界面销毁时卡住 worker。
    stopping: Arc<AtomicBool>,
    /// 丢弃本管理器时停止状态监视线程。
    _watcher: Watcher,
}

impl TaskManager {
    /// 使用系统默认路径布局启动全部 worker 线程，返回管理器与事件接收端。
    pub fn start_system(wake: Waker) -> (Self, EventReceiver) {
        Self::start(wake, FsLayout::system())
    }

    /// 启动全部 worker 线程，返回管理器与事件接收端。
    pub fn start(wake: Waker, layout: FsLayout) -> (Self, EventReceiver) {
        let (event_tx, event_rx) = async_channel::bounded(EVENT_QUEUE_CAPACITY);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            layout,
            native_arch: Arc::new(OnceLock::new()),
            sink: EventSink::new(event_tx, wake, Arc::clone(&stopping)),
        };

        let (catalog_tx, catalog_rx) = async_channel::bounded(CATALOG_QUEUE_CAPACITY);
        let (scan_tx, scan_rx) = async_channel::bounded(SCAN_QUEUE_CAPACITY);
        let (system_tx, system_rx) = async_channel::bounded(SYSTEM_QUEUE_CAPACITY);
        let (net_tx, net_rx) = async_channel::bounded(NET_QUEUE_CAPACITY);
        let (update_tx, update_rx) = async_channel::bounded(UPDATE_QUEUE_CAPACITY);
        let (asset_tx, asset_rx) = async_channel::bounded(ASSET_QUEUE_CAPACITY);

        let pending_installed = Arc::new(AtomicBool::new(false));
        let pending_repository = Arc::new(AtomicBool::new(false));

        let workers = vec![
            worker::spawn_catalog(catalog_rx, worker.clone(), Arc::clone(&pending_installed)),
            worker::spawn_scan(scan_rx, worker.clone(), Arc::clone(&pending_repository)),
            worker::spawn_system(system_rx, worker.clone()),
            worker::spawn_download(net_rx, worker.clone()),
            worker::spawn_update(update_rx, worker.clone()),
            worker::spawn_asset(asset_rx, worker.clone()),
        ];
        let watcher = Watcher::start(worker.layout.clone(), worker.sink.clone());

        let manager = Self {
            next_id: AtomicU64::new(1),
            catalog_tx,
            scan_tx,
            system_tx,
            net_tx,
            update_tx,
            asset_tx,
            pending_installed,
            pending_repository,
            workers,
            stopping,
            _watcher: watcher,
        };
        (manager, event_rx)
    }

    fn next_id(&self) -> OperationId {
        OperationId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// 排队去重地投递任务：标记已置位说明同样的工作还在队列里，忽略本次请求。
    fn enqueue<T>(tx: &Sender<T>, pending: &AtomicBool, job: T) {
        if pending.swap(true, Ordering::Relaxed) {
            return;
        }
        match tx.send_blocking(job) {
            Ok(()) => {}
            Err(error) => {
                pending.store(false, Ordering::Relaxed);
                tracing::warn!("提交后台刷新任务失败：{error}");
            }
        }
    }

    fn submit<T>(tx: &Sender<T>, task: &'static str, job: T) -> Result<(), TaskSubmitError> {
        tx.try_send(job).map_err(|error| match error {
            TrySendError::Full(_) => TaskSubmitError::Full { task },
            TrySendError::Closed(_) => TaskSubmitError::Closed { task },
        })
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
    pub fn uninstall(&self, pkg: String) -> Result<OperationId, TaskSubmitError> {
        let id = self.next_id();
        Self::submit(
            &self.system_tx,
            "系统操作",
            SystemJob::Uninstall { id, pkg },
        )?;
        Ok(id)
    }

    pub fn hold(&self, pkg: String, hold: bool) -> Result<OperationId, TaskSubmitError> {
        let id = self.next_id();
        Self::submit(
            &self.system_tx,
            "系统操作",
            SystemJob::Hold { id, pkg, hold },
        )?;
        Ok(id)
    }

    /// 同步 APT 软件源索引（`apt-get update`）。
    pub fn apt_update(&self) -> Result<OperationId, TaskSubmitError> {
        let id = self.next_id();
        Self::submit(&self.system_tx, "系统操作", SystemJob::AptUpdate { id })?;
        Ok(id)
    }

    /// 升级全部可升级软件包（`apt-get upgrade`）。
    pub fn upgrade_all(&self) -> Result<OperationId, TaskSubmitError> {
        let id = self.next_id();
        Self::submit(&self.system_tx, "系统操作", SystemJob::UpgradeAll { id })?;
        Ok(id)
    }

    /// 升级单个已安装软件包（`apt-get install --only-upgrade`）。
    pub fn upgrade_package(&self, pkg: String) -> Result<OperationId, TaskSubmitError> {
        let id = self.next_id();
        Self::submit(
            &self.system_tx,
            "系统操作",
            SystemJob::UpgradePackage { id, pkg },
        )?;
        Ok(id)
    }

    /// 返回（任务号，取消标志）。需要取消的操作共用同一段分配逻辑。
    fn cancellable(&self) -> (OperationId, CancelToken) {
        (self.next_id(), Arc::new(AtomicBool::new(false)))
    }

    pub fn build(
        &self,
        package: InstalledPackage,
        out_dir: PathBuf,
    ) -> Result<(OperationId, CancelToken), TaskSubmitError> {
        let (id, cancel) = self.cancellable();
        Self::submit(
            &self.system_tx,
            "系统操作",
            SystemJob::Build {
                id,
                package: Box::new(package),
                out_dir,
                cancel: Arc::clone(&cancel),
            },
        )?;
        Ok((id, cancel))
    }

    pub fn download(
        &self,
        request: DownloadRequest,
    ) -> Result<(OperationId, CancelToken), TaskSubmitError> {
        let (id, cancel) = self.cancellable();
        Self::submit(
            &self.net_tx,
            "下载任务",
            NetJob {
                id,
                request,
                cancel: Arc::clone(&cancel),
            },
        )?;
        Ok((id, cancel))
    }

    /// 检查 DebDeck 自身更新。
    pub fn check_self_update(&self) -> Result<(), TaskSubmitError> {
        Self::submit(&self.update_tx, "自更新", UpdateJob::Check)
    }

    /// 下载并验签已确认可用的新版本。
    pub fn download_self_update(&self, update: UpdateInfo) -> Result<(), TaskSubmitError> {
        Self::submit(&self.update_tx, "自更新", UpdateJob::Download(update))
    }

    /// 安装已下载并验签的更新包。
    pub fn install_self_update(&self, downloaded: DownloadedUpdate) -> Result<(), TaskSubmitError> {
        Self::submit(&self.update_tx, "自更新", UpdateJob::Install(downloaded))
    }

    /// 确认新版本已稳定启动，关闭崩溃观察期。
    pub fn confirm_self_update(&self) -> Result<(), TaskSubmitError> {
        Self::submit(&self.update_tx, "自更新", UpdateJob::ConfirmSuccess)
    }

    /// 请求在后台线程解析并解码应用图标；队列满时静默忽略，避免阻塞主线程。
    pub fn load_icon(&self, name: String, max_size: u32) -> bool {
        self.asset_tx
            .try_send(AssetJob::Icon { name, max_size })
            .is_ok()
    }

    /// 请求在后台线程按需读取单个已安装包的文件清单。
    pub fn load_package_files(&self, revision: usize, pkg: String, arch: String) -> bool {
        self.asset_tx
            .try_send(AssetJob::PackageFiles {
                revision,
                pkg,
                arch,
            })
            .is_ok()
    }
}

impl Drop for TaskManager {
    fn drop(&mut self) {
        // 先通知事件发送端停止重试，再关闭任务队列；顺序反了会让 worker
        // 在退出前继续等待已无人消费的事件。
        self.stopping.store(true, Ordering::Relaxed);
        self.catalog_tx.close();
        self.scan_tx.close();
        self.system_tx.close();
        self.net_tx.close();
        self.update_tx.close();
        self.asset_tx.close();

        for handle in self.workers.drain(..) {
            if handle.join().is_err() {
                tracing::error!("后台任务线程发生 panic，已隔离并继续退出");
            }
        }
    }
}
