//! 后端线程 → UI 的事件类型。
//!
//! # 约定
//!
//! * 事件是**单向**的：线程只发送，`app` 在下一帧统一接收，绝不反向调用。
//! * 事件里携带 `String` 而不是错误对象，因为跨线程消息需要轻量且可克隆的
//!   展示文本；强类型错误在发送前通过 [`crate::runtime::events::report_error`]
//!   展开错误链。
//! * 数据类事件合并成一个 [`BackendEvent::CatalogUpdated`]：一次刷新可能同时带来
//!   已安装数据、软件源数据与可升级集合，分成三个事件会让 `app` 重建三次列表。

use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use async_channel::{Receiver, Sender, TrySendError};

use crate::backend::self_update::{DownloadedUpdate, UpdateInfo};
use crate::model::{InstalledData, RepositoryData};

/// 请求标识，用于把进度与结果路由回当前那个操作对话框。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

/// `apt list --upgradable` 的查询结论。
#[derive(Clone, Debug)]
pub enum UpgradableReport {
    /// 查询成功；集合即 APT 认定的可升级包。
    Queried(HashSet<String>),
    /// 查询失败（apt 不可用、索引过期、锁被占用）。
    ///
    /// 上层据此退回本地版本比较，并应当知道这是降级判定。
    Failed,
}

/// 一次数据刷新的产出。任何字段都可以是 `None`，表示本次没有触及它。
#[derive(Clone, Debug, Default)]
pub struct CatalogUpdate {
    pub installed: Option<Result<InstalledData, String>>,
    pub repository: Option<RepositoryData>,
    pub upgradable: Option<UpgradableReport>,
}

impl CatalogUpdate {
    /// 本次刷新带回了已安装数据。
    pub(crate) fn installed(self, result: Result<InstalledData, String>) -> Self {
        Self {
            installed: Some(result),
            ..self
        }
    }

    /// 本次刷新带回了软件源数据。
    pub(crate) fn repository(self, data: RepositoryData) -> Self {
        Self {
            repository: Some(data),
            ..self
        }
    }

    /// 本次刷新带回了可升级集合。
    pub(crate) fn upgradable(self, report: UpgradableReport) -> Self {
        Self {
            upgradable: Some(report),
            ..self
        }
    }
}

#[derive(Debug)]
pub enum BackendEvent {
    /// 加载阶段的文字提示。
    Stage(String),
    CatalogUpdated(CatalogUpdate),
    /// 外部的 dpkg/apt 操作改变了系统状态，需要重新读取。
    DpkgChanged,
    OperationProgress {
        id: OperationId,
        message: String,
        progress: Option<(u64, u64)>,
    },
    OperationFinished {
        id: OperationId,
        /// 结果影响已安装列表时（卸载、保持版本）置真，由上层安排刷新。
        refresh_installed: bool,
        /// 成功时是给用户的说明文本，失败时是错误详情。
        result: Result<String, String>,
    },
    /// 自更新清单检查完成。
    SelfUpdateChecked(Result<Option<UpdateInfo>, String>),
    /// 自更新下载进度或阶段变化。
    SelfUpdateDownloadProgress {
        /// 完成百分比；总量未知时为空。
        percent: Option<f32>,
        /// 可直接展示的中文状态。
        message: String,
    },
    /// 自更新包已下载并验签，或下载失败。
    SelfUpdateDownloaded(Result<DownloadedUpdate, String>),
    /// 自更新安装完成或失败。
    SelfUpdateInstalled(Result<(), String>),
    /// 后台解码完成的应用图标（`None` 表示不存在或解码失败）。
    IconLoaded {
        /// 图标名或路径键。
        name: String,
        /// 已缩放并转为 BGRA 字节序的位图。
        image: Option<image::RgbaImage>,
    },
}

/// 把实现标准错误特型的强类型错误链转成展示文本。
pub fn report_error(error: &(impl Error + ?Sized)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(current) = source {
        text.push('：');
        text.push_str(&current.to_string());
        source = current.source();
    }
    text
}

/// 可跨线程调用的唤醒函数，用于请求 GPUI 在下一帧处理事件。
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// 事件接收端。容量由任务管理器创建时统一设定。
pub(crate) type EventReceiver = Receiver<BackendEvent>;

/// 事件队列容量。界面每帧最多消费 64 条，256 条可覆盖短暂绘制停顿；
/// 队列满时 worker 会主动让出时间片，形成有界背压而不是无限积压内存。
pub(crate) const EVENT_QUEUE_CAPACITY: usize = 256;

/// 事件队列满时的重试间隔。
const EVENT_RETRY_INTERVAL: Duration = Duration::from_millis(2);

#[derive(Clone)]
pub struct EventSink {
    tx: Sender<BackendEvent>,
    wake: Waker,
    stopping: Arc<AtomicBool>,
}

impl EventSink {
    pub fn new(tx: Sender<BackendEvent>, wake: Waker, stopping: Arc<AtomicBool>) -> Self {
        Self { tx, wake, stopping }
    }

    /// 发送事件；队列满时在后台线程内短暂退避，停机时立即放弃。
    pub fn send(&self, event: BackendEvent) {
        let mut event = event;
        while !self.stopping.load(Ordering::Relaxed) {
            match self.tx.try_send(event) {
                Ok(()) => {
                    (self.wake)();
                    return;
                }
                Err(TrySendError::Full(returned)) => {
                    // 先唤醒界面消费积压事件，再短暂退避，避免空转。
                    (self.wake)();
                    event = returned;
                    thread::sleep(EVENT_RETRY_INTERVAL);
                }
                Err(TrySendError::Closed(_)) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn builders_set_only_their_own_field() {
        let update = CatalogUpdate::default()
            .installed(Ok(InstalledData::default()))
            .upgradable(UpgradableReport::Failed);
        assert!(update.installed.as_ref().is_some_and(Result::is_ok));
        assert!(update.repository.is_none());
        assert!(matches!(update.upgradable, Some(UpgradableReport::Failed)));
    }

    #[test]
    fn stopped_sink_does_not_block_or_queue_events() {
        let (tx, rx) = async_channel::bounded(1);
        let stopping = Arc::new(AtomicBool::new(true));
        let sink = EventSink::new(tx, Arc::new(|| {}), stopping);
        sink.send(BackendEvent::Stage("不应入队".to_string()));
        assert!(rx.try_recv().is_err());
    }

    #[derive(Debug, thiserror::Error)]
    #[error("解析软件源失败")]
    struct OuterError {
        #[source]
        source: io::Error,
    }

    #[test]
    fn report_keeps_source_chain() {
        let error = OuterError {
            source: io::Error::other("读取索引失败"),
        };
        assert_eq!(report_error(&error), "解析软件源失败：读取索引失败");
    }
}
