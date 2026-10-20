//! 后端线程 → UI 的事件类型。
//!
//! # 约定
//!
//! * 事件是**单向**的：线程只发送，`app` 在下一帧统一接收，绝不反向调用。
//! * 事件里携带 `String` 而不是 `anyhow::Error`，因为 `Error` 不是 `Clone`/`Send`
//!   友好的跨线程数据；展示用的文本在发送前用 [`crate::runtime::events::report`]
//!   格式化一次。
//! * 数据类事件合并成一个 [`BackendEvent::CatalogUpdated`]：一次刷新可能同时带来
//!   已安装数据、软件源数据与可升级集合，分成三个事件会让 `app` 重建三次列表。

use std::collections::HashSet;

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
}

/// 事件发送端：把 `anyhow::Error` 转成展示文本。
pub fn report(error: &anyhow::Error) -> String {
    // `{:#}` 会拼上整个 context 链，正是排查需要的信息量。
    format!("{error:#}")
}

/// 可跨线程调用的唤醒函数，用于请求 egui 重新绘制。
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct EventSink {
    tx: std::sync::mpsc::Sender<BackendEvent>,
    wake: Waker,
}

impl EventSink {
    pub fn new(tx: std::sync::mpsc::Sender<BackendEvent>, wake: Waker) -> Self {
        Self { tx, wake }
    }

    pub fn send(&self, event: BackendEvent) {
        // 接收端已丢弃（窗口关闭）时静默忽略：线程会随进程退出。
        let _ = self.tx.send(event);
        (self.wake)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn report_keeps_context_chain() {
        let error = anyhow::anyhow!("底层原因")
            .context("读取索引失败")
            .context("解析软件源失败");
        assert_eq!(report(&error), "解析软件源失败: 读取索引失败: 底层原因");
    }
}
