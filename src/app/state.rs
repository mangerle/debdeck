//! 应用状态：把过去平铺的 24 个字段按职责分成四簇。
//!
//! * [`Catalog`] —— 从系统读到的数据以及由它派生的列表条目，是唯一的「事实」来源；
//! * [`ListView`] —— 纯视图状态（搜索、筛选、排序、选中），不持有数据；
//! * [`Dialogs`] —— 同一时刻的模态与提示；
//! * 其余（`paths`/`tasks`/`rx`/`icons` 与输入框、行快照）是运行环境与绘制资源。
//!
//! 拆分的核心动机是 [`ListView::visible`]：它存的是 `Catalog::entries` 的**下标**，
//! 因此「entries 变了就必须重算 visible」这条不变量必须有人守着。现在它由
//! [`ListView`] 与 [`Catalog::revision`] 共同保证，而不是靠调用点记得置一个布尔位。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use gpui_kit::component::input::InputState;
use gpui_kit::component::select::{SearchableVec, SelectState};
use gpui_kit::{Entity, RenderImage, SharedString};

use crate::model::{AppEntry, RepoPackage};
use crate::runtime::{BackendEvent, CancelToken, OperationId, TaskManager};
use crate::util::AppPaths;

use super::catalog::Catalog;
use super::dialogs::Dialogs;
use super::list::ListView;

/// 列表筛选维度。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Installed,
    Gui,
    Held,
}

impl Filter {
    pub(crate) const ALL: [Filter; 4] = [Filter::All, Filter::Installed, Filter::Gui, Filter::Held];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "全部应用",
            Self::Installed => "已安装应用",
            Self::Gui => "界面应用",
            Self::Held => "已保持版本",
        }
    }
}

/// 排序维度。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortKey {
    Name,
    Size,
    Updated,
}

/// 详情页的标签页。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailTab {
    Info,
    Files,
}

/// 状态栏与空列表提示用到的统计。
#[derive(Default)]
pub(crate) struct Counts {
    pub(crate) total: usize,
    pub(crate) installed: usize,
    pub(crate) gui: usize,
    pub(crate) held: usize,
    /// 可升级的包数，用于提示「有 N 个应用可更新」。
    pub(crate) upgradable: usize,
}

/// 加载阶段：首屏数据是否可用决定界面走「加载页」还是「主界面」。
#[derive(Clone, Debug)]
pub(crate) enum LoadState {
    Loading {
        stage: String,
    },
    Ready,
    /// 读取失败：仍保留最后一次阶段文字，重试按钮由此状态出现。
    Failed {
        stage: String,
        error: String,
    },
}

impl LoadState {
    pub(crate) fn stage(&self) -> &str {
        match self {
            Self::Loading { stage } | Self::Failed { stage, .. } => stage,
            Self::Ready => "",
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    /// 回到加载态（重试按钮使用）。
    pub(crate) fn reset_to_loading(&mut self, stage: &str) {
        *self = Self::Loading {
            stage: stage.to_string(),
        };
    }
}

/// 当前打开的模态。
///
/// 同一时刻只有一个模态，`Download` 携带的版本列表规模有限，
/// 为它单独装箱只会让每个匹配点多一层解包噪声。
#[allow(clippy::large_enum_variant)]
pub(crate) enum ModalState {
    Confirm(Confirm),
    Download(DownloadDialog),
    Operation(Operation),
    Message { title: String, message: String },
}

/// 需要用户确认的操作。确认后再投递任务，避免误点。
#[derive(Clone)]
pub(crate) enum Confirm {
    Uninstall {
        pkg: String,
        name: String,
    },
    Hold {
        pkg: String,
        name: String,
        hold: bool,
    },
    Build {
        pkg: String,
        name: String,
    },
}

/// 在线下载对话框。
#[derive(Clone)]
pub(crate) struct DownloadDialog {
    pub(crate) pkg: String,
    pub(crate) name: String,
    /// 可选版本，按降序（由 `RepositoryData` 的不变量保证）。
    pub(crate) versions: Vec<RepoPackage>,
    pub(crate) selected: usize,
    pub(crate) destination_dir: std::path::PathBuf,
}

impl DownloadDialog {
    /// 当前选中的版本。越界（理论上不该发生）时返回 `None`，界面据此禁用按钮，
    /// 而不是直接下标取值留下 panic 风险。
    pub(crate) fn chosen(&self) -> Option<&RepoPackage> {
        self.versions.get(self.selected)
    }

    /// 保存路径。预览与实际下载共用这一处计算，避免两处规则漂移。
    pub(crate) fn destination(&self) -> Option<std::path::PathBuf> {
        let package = self.chosen()?;
        let file_name = crate::util::package_file_name(
            &package.metadata.name,
            &package.metadata.version,
            &package.metadata.arch,
        );
        Some(self.destination_dir.join(file_name))
    }
}

/// 正在执行或已有结果的操作。
pub(crate) struct Operation {
    pub(crate) id: OperationId,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) progress: Option<(u64, u64)>,
    pub(crate) result: Option<Result<String, String>>,
    pub(crate) cancel: Option<CancelToken>,
    pub(crate) open_dir: Option<std::path::PathBuf>,
    pub(crate) cancel_requested: bool,
}

impl Operation {
    pub(crate) fn is_running(&self) -> bool {
        self.result.is_none()
    }
}

/// 界面能发起的动作。
///
/// 每个动作都自带目标包名，不依赖「当前选中的是谁」：这样分发逻辑可以单测，
/// 也消除了「点击与处理之间选中项已经变了」这类跨帧竞态。
/// 所有副作用都从这里进，`ui` 层因此不需要认识 `backend`。
#[derive(Debug)]
pub(crate) enum Action {
    Open {
        desktop: crate::model::DesktopEntry,
        name: String,
    },
    Uninstall {
        pkg: String,
        name: String,
    },
    ToggleHold {
        pkg: String,
        name: String,
        /// 当前是否已保持，决定下一步是 hold 还是 unhold。
        held: bool,
    },
    Download {
        pkg: String,
        name: String,
    },
    Build {
        pkg: String,
        name: String,
    },
    /// 在应用菜单中隐藏或恢复该包的桌面启动项。
    SetMenuVisibility {
        pkg: String,
        name: String,
        path: std::path::PathBuf,
        hidden: bool,
    },
    OpenDownloads,
    Copy(String),
    /// 重新读取软件源与已安装数据。
    Refresh,
}

/// 刷新是否可用。
///
/// 规则只写在这一处：顶栏按钮、F5 快捷键与应用层的 `refresh_all` 都读它，
/// 避免同一判断在三个地方各写一遍、改一个漏两个。
pub(crate) fn refresh_allowed(catalog: &Catalog, dialogs: &Dialogs) -> bool {
    !dialogs.is_operation_running() && !catalog.repository_loading()
}

/// 图标缓存：图标名 → 解析结果（`None` 表示「确认没有图标」，避免每帧重试磁盘）。
pub(crate) type IconCache = HashMap<String, Option<Arc<RenderImage>>>;

/// 排序下拉的状态。
pub(crate) type SortSelect = Entity<SelectState<SearchableVec<&'static str>>>;

/// 状态列的语义，颜色由主题在绘制时决定。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowStatus {
    NotInstalled,
    Installed,
    Held,
    Update,
}

impl RowStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "未安装",
            Self::Installed => "已安装",
            Self::Held => "已保持",
            Self::Update => "可更新",
        }
    }

    pub(crate) fn of(entry: &crate::model::AppEntry) -> Self {
        if !entry.is_installed {
            Self::NotInstalled
        } else if entry.is_held {
            Self::Held
        } else if entry.has_update {
            Self::Update
        } else {
            Self::Installed
        }
    }
}

pub(crate) struct DebDeckApp {
    pub(crate) paths: AppPaths,
    pub(crate) tasks: TaskManager,
    pub(crate) rx: Receiver<BackendEvent>,
    pub(crate) catalog: Catalog,
    pub(crate) list: ListView,
    pub(crate) dialogs: Dialogs,
    pub(crate) icons: IconCache,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) files_input: Entity<InputState>,
    pub(crate) sort_select: SortSelect,
    /// `Catalog::entries` 的快照，随 revision 更新；虚拟列表闭包持有它。
    pub(crate) entries: Arc<Vec<AppEntry>>,
    /// 快照对应的 Catalog 版本号。
    pub(crate) entries_revision: usize,
    /// 当前可见行下标快照。
    pub(crate) visible: Arc<Vec<usize>>,
    /// 快照对应的选中项，用来判断是否需要清理文件过滤。
    pub(crate) selected_pkg: Option<String>,
    /// 当前详情页的文件快照（已按过滤词筛选）。
    pub(crate) files: Arc<Vec<SharedString>>,
    /// 文件快照的凭据：`(Catalog 版本, 包名, 过滤词)`。
    pub(crate) files_key: Option<(usize, String, String)>,
    /// 未过滤的文件总数，供「共 N 个文件」显示。
    pub(crate) files_total: usize,
    /// Toast 代次：替换旧提示后，旧的定时任务据此放弃清理。
    pub(crate) toast_generation: u64,
}
