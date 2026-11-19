//! DebDeck 自身版本更新状态机与动作编排。
//!
//! # 设计原理
//! - 更新状态随主窗口生命周期存在：启动检查只点亮标题栏版本入口，用户打开
//!   弹窗后才显示完整流程，不打断正在管理软件包的操作。
//! - 检查、下载、安装都投递到独立有界任务队列；UI 只消费事件，不在渲染或
//!   点击回调里执行网络与安装器 I/O。
//! - 已检查到的版本、已下载的包与安装成功状态分别保留，下载或安装失败后可以
//!   直接重试，不必重新请求前一步。
//!
//! # 代价与局限
//! 下载进度只保留最新百分比与阶段文案，不保存历史曲线；更新任务与普通软件包
//! 下载隔离，因此不会因为其他下载排队而阻塞自身升级。

use std::sync::atomic::Ordering;

use gpui_kit::Context;

use crate::runtime::{BackendEvent, DownloadedUpdate, TaskSubmitError, UpdateInfo};

use super::state::{DebDeckApp, ModalState};

/// 更新日志展示上限：远端清单属于外部输入，超长正文不允许直接铺满内存与界面。
const MAX_RELEASE_NOTES_CHARS: usize = 64 * 1024;

/// 更新流程的互斥阶段。
#[derive(Default)]
pub(crate) enum SelfUpdateStatus {
    /// 尚未检查。
    #[default]
    Idle,
    /// 正在读取远端清单。
    Checking,
    /// 当前已是最新版本。
    UpToDate,
    /// 发现新版本，等待下载。
    Available,
    /// 正在下载并验签。
    Downloading {
        /// 目标版本号。
        version: String,
        /// 完成百分比。
        percent: Option<f32>,
        /// 当前阶段文案。
        message: String,
    },
    /// 已下载并验签，等待安装。
    Ready,
    /// 正在执行安装。
    Installing {
        /// 目标版本号。
        version: String,
        /// 当前阶段文案。
        message: String,
    },
    /// 安装完成，等待重启。
    Installed {
        /// 已安装的版本号。
        version: String,
    },
    /// 最近一次检查、下载或安装失败。
    Failed {
        /// 可直接展示的失败原因。
        message: String,
    },
}

/// 更新弹窗主动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelfUpdatePrimaryAction {
    /// 检查远端清单。
    Check,
    /// 下载并验签。
    Download,
    /// 安装已下载的包。
    Install,
    /// 重启应用以加载新版本。
    Restart,
    /// 当前阶段不可重复触发。
    Busy,
}

/// 更新弹窗渲染所需的只读快照。
pub(crate) struct SelfUpdateSnapshot {
    pub(crate) status: String,
    pub(crate) detail: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) current_version: &'static str,
    pub(crate) target_version: Option<String>,
    pub(crate) published: Option<String>,
    pub(crate) progress: Option<f32>,
    pub(crate) indeterminate: bool,
    pub(crate) primary: SelfUpdatePrimaryAction,
    pub(crate) failed: bool,
}

/// DebDeck 自身更新状态。
pub(crate) struct SelfUpdate {
    status: SelfUpdateStatus,
    available: Option<UpdateInfo>,
    downloaded: Option<DownloadedUpdate>,
    installed: Option<String>,
}

impl Default for SelfUpdate {
    fn default() -> Self {
        Self {
            status: SelfUpdateStatus::Idle,
            available: None,
            downloaded: None,
            installed: None,
        }
    }
}

impl SelfUpdate {
    /// 标题栏是否应显示“可更新”提示。
    pub(crate) fn has_update(&self) -> bool {
        self.available.is_some() || self.installed.is_some()
    }

    /// 当前发现的新版本号。
    pub(crate) fn available_version(&self) -> Option<String> {
        if self.installed.is_some() {
            return None;
        }
        self.available.as_ref().map(UpdateInfo::version)
    }

    /// 是否处于不可重复触发的在途状态。
    pub(crate) fn is_busy(&self) -> bool {
        matches!(
            self.status,
            SelfUpdateStatus::Checking
                | SelfUpdateStatus::Downloading { .. }
                | SelfUpdateStatus::Installing { .. }
        )
    }

    /// 是否有可复用的检查或下载成果，避免每次打开弹窗都重新请求远端。
    fn has_result(&self) -> bool {
        self.available.is_some() || self.downloaded.is_some() || self.installed.is_some()
    }

    /// 进入检查阶段并清空旧的下载成果。
    fn begin_check(&mut self) {
        self.available = None;
        self.downloaded = None;
        self.installed = None;
        self.status = SelfUpdateStatus::Checking;
    }

    /// 记录下载/安装阶段文案。
    fn apply_progress(&mut self, percent: Option<f32>, message: String) {
        match &mut self.status {
            SelfUpdateStatus::Downloading {
                percent: current,
                message: current_message,
                ..
            } => {
                if percent.is_some() {
                    *current = percent;
                }
                *current_message = message;
            }
            SelfUpdateStatus::Installing {
                message: current_message,
                ..
            } => *current_message = message,
            _ => {}
        }
    }

    /// 记录检查结果。
    fn apply_check_result(&mut self, result: Result<Option<UpdateInfo>, String>) {
        self.status = match result {
            Ok(Some(info)) => {
                self.available = Some(info);
                SelfUpdateStatus::Available
            }
            Ok(None) => SelfUpdateStatus::UpToDate,
            Err(message) => SelfUpdateStatus::Failed { message },
        };
    }

    /// 记录下载结果；失败时保留 available，允许直接重试下载。
    fn apply_download_result(&mut self, result: Result<DownloadedUpdate, String>) {
        self.status = match result {
            Ok(downloaded) => {
                self.downloaded = Some(downloaded);
                SelfUpdateStatus::Ready
            }
            Err(message) => SelfUpdateStatus::Failed { message },
        };
    }

    /// 记录安装结果；失败时保留 downloaded，允许直接重试安装。
    fn apply_install_result(&mut self, result: Result<(), String>) {
        self.status = match result {
            Ok(()) => {
                let version = self
                    .downloaded
                    .as_ref()
                    .map(DownloadedUpdate::version)
                    .or_else(|| self.available.as_ref().map(UpdateInfo::version))
                    .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
                self.installed = Some(version.clone());
                SelfUpdateStatus::Installed { version }
            }
            Err(message) => SelfUpdateStatus::Failed { message },
        };
    }

    /// 任务提交失败时把错误写入状态；调用方负责请求重绘。
    fn fail_submit(&mut self, context: &str, error: &TaskSubmitError) {
        self.status = SelfUpdateStatus::Failed {
            message: format!("{context}：{error}"),
        };
    }

    /// 当前主按钮动作由已保存的阶段成果决定。
    fn primary_action(&self) -> SelfUpdatePrimaryAction {
        if self.is_busy() {
            return SelfUpdatePrimaryAction::Busy;
        }
        if self.installed.is_some() {
            SelfUpdatePrimaryAction::Restart
        } else if self.downloaded.is_some() {
            SelfUpdatePrimaryAction::Install
        } else if self.available.is_some() {
            SelfUpdatePrimaryAction::Download
        } else {
            SelfUpdatePrimaryAction::Check
        }
    }

    /// 把当前状态投影为拥有型快照，绘制期间不再借用状态机内部数据。
    pub(crate) fn snapshot(&self) -> SelfUpdateSnapshot {
        let target_version = self.available.as_ref().map(UpdateInfo::version);
        let notes = self
            .available
            .as_ref()
            .and_then(UpdateInfo::notes)
            .and_then(|notes| bounded_release_notes(&notes));
        let published = self.available.as_ref().and_then(UpdateInfo::pub_date);
        let failed = matches!(self.status, SelfUpdateStatus::Failed { .. });

        let (status, detail, progress, indeterminate) = match &self.status {
            SelfUpdateStatus::Idle => ("尚未检查更新".to_string(), None, None, false),
            SelfUpdateStatus::Checking => ("正在检查更新…".to_string(), None, None, true),
            SelfUpdateStatus::UpToDate => ("当前已是最新版本".to_string(), None, None, false),
            SelfUpdateStatus::Available => (
                target_version.as_ref().map_or_else(
                    || "发现新版本".to_string(),
                    |version| format!("发现新版本 v{version}"),
                ),
                None,
                None,
                false,
            ),
            SelfUpdateStatus::Downloading {
                version,
                percent,
                message,
            } => (
                format!("正在下载 v{version}"),
                Some(progress_detail(*percent, message)),
                *percent,
                false,
            ),
            SelfUpdateStatus::Ready => (
                self.downloaded.as_ref().map_or_else(
                    || "更新包已下载并验签".to_string(),
                    |downloaded| format!("v{} 已下载并验签", downloaded.version()),
                ),
                None,
                Some(100.0),
                false,
            ),
            SelfUpdateStatus::Installing { version, message } => (
                format!("正在安装 v{version}"),
                Some(message.clone()),
                None,
                true,
            ),
            SelfUpdateStatus::Installed { version } => (
                format!("v{version} 已安装"),
                Some("重启应用后新版本生效。".to_string()),
                Some(100.0),
                false,
            ),
            SelfUpdateStatus::Failed { message } => {
                (format!("更新失败：{message}"), None, None, false)
            }
        };

        SelfUpdateSnapshot {
            status,
            detail,
            notes,
            current_version: env!("CARGO_PKG_VERSION"),
            target_version,
            published,
            progress,
            indeterminate,
            primary: self.primary_action(),
            failed,
        }
    }
}

impl DebDeckApp {
    /// 启动后检查一次远端清单；只更新标题栏状态，不主动弹出窗口。
    pub(crate) fn start_startup_update_check(&mut self) {
        if !matches!(self.self_update.status, SelfUpdateStatus::Idle) {
            return;
        }
        self.start_self_update_check(false);
    }

    /// 用户从标题栏打开更新弹窗；没有可复用结果时立即发起检查。
    pub(crate) fn open_self_update_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialogs.modal = Some(ModalState::SelfUpdate);
        if !self.self_update.has_result() && !self.self_update.is_busy() {
            self.start_self_update_check(true);
        }
        cx.notify();
    }

    /// 开始一次清单检查；`keep_modal` 决定用户打开弹窗后是否继续保持显示。
    fn start_self_update_check(&mut self, keep_modal: bool) {
        if self.self_update.is_busy() {
            return;
        }
        self.self_update.begin_check();
        if keep_modal {
            self.dialogs.modal = Some(ModalState::SelfUpdate);
        }
        if let Err(error) = self.tasks.check_self_update() {
            self.self_update.fail_submit("无法开始检查更新", &error);
        }
    }

    /// 下载已确认可用的新版本。
    pub(crate) fn download_self_update(&mut self, cx: &mut Context<Self>) {
        if self.self_update.is_busy() {
            return;
        }
        let Some(info) = self.self_update.available.clone() else {
            return;
        };
        let version = info.version();
        self.self_update.status = SelfUpdateStatus::Downloading {
            version,
            percent: None,
            message: "正在准备下载更新包".to_string(),
        };
        if let Err(error) = self.tasks.download_self_update(info) {
            self.self_update.fail_submit("无法开始下载更新", &error);
        }
        cx.notify();
    }

    /// 安装已下载并验签的更新包。
    pub(crate) fn install_self_update(&mut self, cx: &mut Context<Self>) {
        if self.self_update.is_busy() {
            return;
        }
        let Some(downloaded) = self.self_update.downloaded.clone() else {
            return;
        };
        let version = downloaded.version();
        self.self_update.status = SelfUpdateStatus::Installing {
            version,
            message: "正在请求管理员授权并安装更新".to_string(),
        };
        if let Err(error) = self.tasks.install_self_update(downloaded) {
            self.self_update.fail_submit("无法开始安装更新", &error);
        }
        cx.notify();
    }

    /// 用户点击“立即重启”：请求主循环退出，旧进程收尾后由入口拉起新版本。
    pub(crate) fn restart_after_self_update(&mut self, cx: &mut Context<Self>) {
        if self.self_update.installed.is_none() {
            return;
        }
        self.restart_requested.store(true, Ordering::Relaxed);
        cx.quit();
    }

    /// 当前弹窗主按钮对应的动作。
    pub(crate) fn perform_self_update_action(&mut self, cx: &mut Context<Self>) {
        match self.self_update.primary_action() {
            SelfUpdatePrimaryAction::Check => self.start_self_update_check(true),
            SelfUpdatePrimaryAction::Download => self.download_self_update(cx),
            SelfUpdatePrimaryAction::Install => self.install_self_update(cx),
            SelfUpdatePrimaryAction::Restart => self.restart_after_self_update(cx),
            SelfUpdatePrimaryAction::Busy => {}
        }
        cx.notify();
    }

    /// 应用后台自更新事件。
    pub(crate) fn apply_self_update_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::SelfUpdateChecked(result) => {
                self.self_update.apply_check_result(result);
            }
            BackendEvent::SelfUpdateDownloadProgress { percent, message } => {
                self.self_update.apply_progress(percent, message);
            }
            BackendEvent::SelfUpdateDownloaded(result) => {
                self.self_update.apply_download_result(result);
            }
            BackendEvent::SelfUpdateInstalled(result) => {
                self.self_update.apply_install_result(result);
            }
            _ => {}
        }
    }
}

/// 截断外部更新日志，限制内存与绘制测量成本。
fn bounded_release_notes(notes: &str) -> Option<String> {
    let trimmed = notes.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= MAX_RELEASE_NOTES_CHARS {
        return Some(trimmed.to_string());
    }
    let mut bounded: String = trimmed.chars().take(MAX_RELEASE_NOTES_CHARS).collect();
    bounded.push_str("\n\n……更新日志过长，已截断。");
    Some(bounded)
}

/// 下载详情：阶段文案与百分比合并成一行。
fn progress_detail(percent: Option<f32>, message: &str) -> String {
    percent.map_or_else(
        || message.to_string(),
        |value| format!("{message} · {value:.1}%"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空闲状态只允许检查，不得误显示下载或安装入口。
    #[test]
    fn idle_state_allows_check_only() {
        let update = SelfUpdate::default();
        let snapshot = update.snapshot();
        assert_eq!(snapshot.primary, SelfUpdatePrimaryAction::Check);
        assert!(!snapshot.failed);
        assert!(snapshot.progress.is_none());
        assert!(snapshot.notes.is_none());
    }

    /// 运行中的阶段必须禁用重复触发。
    #[test]
    fn busy_state_disables_primary_action() {
        let update = SelfUpdate {
            status: SelfUpdateStatus::Checking,
            ..SelfUpdate::default()
        };
        let snapshot = update.snapshot();
        assert_eq!(snapshot.primary, SelfUpdatePrimaryAction::Busy);
        assert!(snapshot.indeterminate);
    }

    /// 错误状态仍需保留重试入口，并标记失败颜色。
    #[test]
    fn failed_state_allows_retry() {
        let update = SelfUpdate {
            status: SelfUpdateStatus::Failed {
                message: "网络不可达".to_string(),
            },
            ..SelfUpdate::default()
        };
        let snapshot = update.snapshot();
        assert_eq!(snapshot.primary, SelfUpdatePrimaryAction::Check);
        assert!(snapshot.failed);
    }

    /// 超长更新日志必须截断，避免远端清单把界面拖入无界状态。
    #[test]
    fn release_notes_are_bounded() {
        let notes = "a".repeat(MAX_RELEASE_NOTES_CHARS + 32);
        let bounded = bounded_release_notes(&notes).expect("非空日志必须保留");
        assert!(bounded.chars().count() > MAX_RELEASE_NOTES_CHARS);
        assert!(bounded.ends_with("……更新日志过长，已截断。"));
    }

    /// 下载进度文案同时展示百分比与阶段文案。
    #[test]
    fn progress_message_contains_percent() {
        let message = progress_detail(Some(12.34), "正在下载");
        assert!(message.contains("12.3%"), "详情缺少百分比: {message}");
        assert!(message.contains("正在下载"));
    }
}
