//! 应用层：状态组合 + 更新逻辑 + 进程启动。
//!
//! # 为什么启动流程放在这里而不是 `main.rs`
//!
//! `lib.rs` 只把 [`run`] 暴露为公开接口，其余模块一律 `pub(crate)`。这样
//! 「二进制能碰到什么」是由 crate 的可见性决定的，而不是靠约定；`main.rs` 因此
//! 退化成一行的转发。
//!
//! # 与 `ui` 的唯一 upward 引用
//!
//! `run` 与 `DebDeckApp::new` 会调用 `crate::ui` 读取窗口和标题栏图标，
//! 这是启动期的一次性资源，不参与每帧绘制。绘制期的依赖方向仍然是单向的
//! `ui → app`。

mod actions;
mod catalog;
mod dialogs;
mod install;
mod list;
mod self_update;
mod state;
mod update;

pub(crate) use catalog::Catalog;
pub(crate) use dialogs::Dialogs;
pub(crate) use install::{InstallApp, PasswordUi, Phase};
pub(crate) use list::ListView;
pub(crate) use self_update::{SelfUpdatePrimaryAction, SelfUpdateSnapshot};
// UI 层需要的状态类型集中再导出：`state` 本身保持私有，避免绘制代码绕过
// `app` 的门面去抓内部模块。
pub(crate) use state::{
    Action, Confirm, DebDeckApp, DetailTab, DownloadDialog, Filter, IconCache, ModalState,
    Operation, RowStatus, SortKey, SortSelect, refresh_allowed,
};

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::component::IndexPath;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::select::{SearchableVec, SelectState};
use gpui_kit::component::{ActiveTheme as _, TitleBar, h_flex, v_flex};
use gpui_kit::*;

use crate::backend::FsLayout;
use crate::backend::shell;
use crate::runtime::{BackendEvent, InstanceLock, TaskManager, Waker};
use crate::ui::{APP_ICON_SIZE, DETAIL_ICON_TEXTURE_SIZE, window_icon};
use crate::update::{RecoveryStatus, check_and_recover_on_startup, restart_current_process};
use crate::util::{APP_ID, APP_NAME, AppPaths};

mod refresh_action {
    gpui_kit::actions!(debdeck, [Refresh]);
}

use refresh_action::Refresh;

/// 启动 DebDeck。
///
/// 带 `.deb` 参数时进入安装模式：只开一个紧凑的安装窗口，不碰主窗口，也不
/// 抢主实例锁——主窗口可以同时运行，安装完成后由 dpkg 监视触发刷新。
/// 没有安装参数时才是常规的主窗口，并保持单实例语义。
pub fn run() {
    init_tracing();

    let mut install_args = deb_args();
    if !install_args.is_empty() {
        if install_args.len() > 1 {
            tracing::warn!(
                "一次启动只处理一个安装包，忽略其余 {} 个",
                install_args.len() - 1
            );
        }
        run_install_window(install_args.remove(0));
        return;
    }

    let instance = match InstanceLock::acquire() {
        Ok(instance) => instance,
        Err(message) => {
            tracing::error!("{message}");
            shell::notify("DebDeck 已在运行", "已有 DebDeck 实例正在运行。");
            return;
        }
    };

    match check_and_recover_on_startup() {
        Ok(RecoveryStatus::Normal) => {
            tracing::debug!("自更新启动自检通过，没有待确认的升级");
        }
        Ok(RecoveryStatus::PendingConfirmation { attempts }) => {
            tracing::info!("新版本处于健康观察期，当前为第 {attempts} 次启动尝试");
        }
        Ok(RecoveryStatus::RolledBack { from_version }) => {
            tracing::warn!("新版本 {from_version} 连续启动失败，已自动回滚到上一稳定版本");
        }
        Err(error) => tracing::warn!("自更新启动自检失败，继续以当前版本启动：{error}"),
    }

    let restart_requested = Arc::new(AtomicBool::new(false));
    let restart_flag = Arc::clone(&restart_requested);
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| run_main_window(cx, restart_flag));

    // 自更新重启前必须释放单实例锁，否则新进程会被旧实例误判为重复启动。
    drop(instance);
    if restart_requested.load(Ordering::Relaxed)
        && let Err(error) = restart_current_process()
    {
        tracing::error!("更新已安装，但自动重启失败：{error}");
    }
}

/// 安装模式：单窗口、单安装包。
fn run_install_window(path: PathBuf) {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| open_install_window(cx, path));
}

fn run_main_window(cx: &mut App, restart_requested: Arc<AtomicBool>) {
    gpui_kit::init(cx);
    if let Err(error) = gpui_kit::open_window(window_options(), cx, move |window, cx| {
        create_main_app(window, cx, restart_requested)
    }) {
        tracing::error!("无法打开窗口：{error:#}");
        cx.quit();
    }
}

fn create_main_app(
    window: &mut Window,
    cx: &mut App,
    restart_requested: Arc<AtomicBool>,
) -> Entity<DebDeckApp> {
    crate::ui::theme::configure(cx, Some(window));
    cx.new(|cx| DebDeckApp::new(window, cx, restart_requested))
}

fn open_install_window(cx: &mut App, path: PathBuf) {
    gpui_kit::init(cx);
    if let Err(error) = gpui_kit::open_window(install_window_options(), cx, move |window, cx| {
        crate::ui::theme::configure(cx, Some(window));
        cx.new(|cx| InstallApp::new(&path, window, cx))
    }) {
        tracing::error!("无法打开安装窗口：{error:#}");
        cx.quit();
    }
}

/// 启动参数中的 `.deb` 路径（按传入顺序）。
fn deb_args() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("deb"))
        })
        .collect()
}

fn sort_from_label(label: &str) -> (SortKey, bool) {
    match label {
        "名称 ↓" => (SortKey::Name, false),
        "大小 ↑" => (SortKey::Size, true),
        "大小 ↓" => (SortKey::Size, false),
        "更新时间 ↑" => (SortKey::Updated, true),
        "更新时间 ↓" => (SortKey::Updated, false),
        _ => (SortKey::Name, true),
    }
}

fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(80.0), px(80.0)),
            size(px(1120.0), px(720.0)),
        ))),
        window_min_size: Some(size(px(880.0), px(560.0))),
        app_id: Some(APP_ID.to_string()),
        icon: window_icon(),
        // GNOME/Wayland 不支持服务端装饰；强制客户端装饰，由 TitleBar 自绘
        // 拖动区与最小化/最大化/关闭按钮。
        window_decorations: Some(WindowDecorations::Client),
        ..TitleBar::window_options()
    }
}

/// 安装窗口只放得下包信息与按钮，不复用主窗口的尺寸。
fn install_window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(160.0), px(120.0)),
            size(px(520.0), px(360.0)),
        ))),
        window_min_size: Some(size(px(460.0), px(install::MIN_WINDOW_HEIGHT))),
        app_id: Some(APP_ID.to_string()),
        icon: window_icon(),
        window_decorations: Some(WindowDecorations::Client),
        // 安装窗口自绘标题条（无最小化/最大化/关闭），由它负责拖动窗口。
        ..TitleBar::window_options()
    }
}

/// 日志只写 stderr：图形程序没有 tty，`RUST_LOG` 决定详细程度。
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
}

/// 后端事件循环：每次唤醒处理一批事件并请求一次重绘。
fn spawn_wake_loop(wake_rx: async_channel::Receiver<()>, cx: &mut Context<DebDeckApp>) {
    cx.spawn(
        async move |this, cx| {
            while wake_rx.recv().await.is_ok() && this.update(cx, update_after_wake).is_ok() {}
        },
    )
    .detach();
}

fn update_after_wake(app: &mut DebDeckApp, cx: &mut Context<DebDeckApp>) {
    app.poll_events();
    cx.notify();
}

fn start_backend() -> (
    TaskManager,
    async_channel::Receiver<BackendEvent>,
    async_channel::Receiver<()>,
) {
    // 后端线程只发事件；专门的唤醒通道把「有事件了」翻译成一次主线程重绘。
    let (wake_tx, wake_rx) = async_channel::bounded::<()>(1);
    let wake: Waker = Arc::new(move || {
        // 唤醒信号只需保留一个：队列满说明已有待处理的唤醒，重复发送没有意义。
        let _ = wake_tx.try_send(());
    });
    let layout = FsLayout::system();
    let (tasks, rx) = TaskManager::start(wake, layout);
    tasks.load_initial();
    (tasks, rx, wake_rx)
}

fn create_inputs(
    window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> (Entity<InputState>, Entity<InputState>) {
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索应用名、包名或拼音"));
    let files = cx.new(|cx| InputState::new(window, cx).placeholder("在文件列表中查找"));
    (search, files)
}

fn create_sort_select(
    window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> Entity<SelectState<SearchableVec<&'static str>>> {
    cx.new(|cx| sort_select_state(window, cx))
}

fn sort_select_state(
    window: &mut Window,
    cx: &mut Context<SelectState<SearchableVec<&'static str>>>,
) -> SelectState<SearchableVec<&'static str>> {
    const LABELS: [&str; 6] = [
        "名称 ↑",
        "名称 ↓",
        "大小 ↑",
        "大小 ↓",
        "更新时间 ↑",
        "更新时间 ↓",
    ];
    SelectState::new(
        SearchableVec::new(LABELS.to_vec()),
        Some(IndexPath::new(0)),
        window,
        cx,
    )
}

fn observe_inputs(
    search_input: &Entity<InputState>,
    files_input: &Entity<InputState>,
    sort_select: &Entity<SelectState<SearchableVec<&'static str>>>,
    cx: &mut Context<DebDeckApp>,
) {
    cx.observe(sort_select, |app, state, cx| {
        update_sort_from_select(app, &state, cx);
    })
    .detach();
    observe_text_input(search_input, |this| &mut this.list.search, cx);
    observe_text_input(files_input, |this| &mut this.list.files_query, cx);
}

fn update_sort_from_select(app: &mut DebDeckApp, state: &SortSelect, cx: &mut Context<DebDeckApp>) {
    let Some(label) = state.read(cx).selected_value().copied() else {
        return;
    };
    let (sort, ascending) = sort_from_label(label);
    if app.list.sort != sort || app.list.ascending != ascending {
        app.list.sort = sort;
        app.list.ascending = ascending;
        cx.notify();
    }
}

fn observe_text_input(
    input: &Entity<InputState>,
    field: fn(&mut DebDeckApp) -> &mut String,
    cx: &mut Context<DebDeckApp>,
) {
    cx.observe(input, move |this, state, cx| {
        let value = state.read(cx).value().to_string();
        let target = field(this);
        if *target != value {
            *target = value;
            cx.notify();
        }
    })
    .detach();
}

fn bind_refresh_action(cx: &mut Context<DebDeckApp>) {
    cx.bind_keys([KeyBinding::new("f5", Refresh, None)]);
    let weak = cx.entity().downgrade();
    App::on_action(&mut *cx, move |_: &Refresh, cx: &mut App| {
        weak.update(cx, |app, cx| {
            app.handle_action(Action::Refresh, cx);
        })
        .ok();
    });
}

impl DebDeckApp {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        restart_requested: Arc<AtomicBool>,
    ) -> Self {
        let paths = AppPaths::new();
        let (tasks, rx, wake_rx) = start_backend();
        let (search_input, files_input) = create_inputs(window, cx);
        let sort_select = create_sort_select(window, cx);
        observe_inputs(&search_input, &files_input, &sort_select, cx);
        spawn_wake_loop(wake_rx, cx);
        bind_refresh_action(cx);

        let mut app = Self {
            paths,
            tasks,
            rx,
            catalog: Catalog::new(),
            list: ListView::new(),
            dialogs: Dialogs::new(),
            icons: IconCache::new(),
            app_icon: crate::ui::app_icon_texture(),
            search_input,
            files_input,
            sort_select,
            entries: Arc::new(Vec::new()),
            entries_revision: 0,
            visible: Arc::new(Vec::new()),
            selected_pkg: None,
            files: Arc::new(Vec::new()),
            files_key: None,
            files_total: 0,
            toast_generation: 0,
            self_update: self_update::SelfUpdate::default(),
            restart_requested,
            update_confirm_task: None,
        };
        app.start_startup_update_check();
        app.update_confirm_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |app, _cx| {
                if let Err(error) = app.tasks.confirm_self_update() {
                    tracing::warn!("无法提交自更新成功确认任务：{error}");
                }
            });
        }));
        app
    }

    /// 重算可见行与快照。Catalog 条目只在版本变化时复制一次；搜索/排序只
    /// 复制下标的 `Vec<usize>`，不会把 7 万条行的字符串整表克隆。
    fn ensure_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let recomputed = self.list.ensure_current(&self.catalog);
        let revision = self.catalog.revision();
        if self.entries_revision != revision {
            self.entries = Arc::new(self.catalog.entries().to_vec());
            self.entries_revision = revision;
        }
        if recomputed {
            self.visible = Arc::new(self.list.visible().to_vec());
        }

        let selection_changed = self.selected_pkg != self.list.selected;
        if selection_changed {
            self.selected_pkg = self.list.selected.clone();
            self.files_key = None;
            self.files_input
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.ensure_files();
        self.ensure_selected_icon();
    }

    /// 为当前选中项预请求图标后台解码；渲染期只读取缓存结果。
    pub(crate) fn ensure_selected_icon(&mut self) {
        let Some(pkg) = self.list.selected.as_deref() else {
            return;
        };
        let Some(icon_name) = self
            .catalog
            .installed_package(pkg)
            .and_then(|package| package.desktop.as_ref())
            .map(|desktop| desktop.icon.as_str())
        else {
            return;
        };
        self.icons
            .request_if_needed(icon_name, &self.tasks, DETAIL_ICON_TEXTURE_SIZE);
    }

    fn ensure_files(&mut self) {
        let Some(entry) = self.list.selected_entry(&self.catalog) else {
            self.files = Arc::new(Vec::new());
            self.files_key = None;
            self.files_total = 0;
            return;
        };
        let Some(package) = self.catalog.installed_package(&entry.pkg) else {
            self.files = Arc::new(Vec::new());
            self.files_key = None;
            self.files_total = 0;
            return;
        };

        self.files_total = package.files.len();
        let query = self.list.files_query.trim().to_lowercase();
        let key = (self.catalog.revision(), entry.pkg.clone(), query.clone());
        if self.files_key.as_ref() == Some(&key) {
            return;
        }

        let files = if query.is_empty() {
            package.files.clone()
        } else {
            package
                .files
                .iter()
                .filter(|path| path.to_lowercase().contains(&query))
                .cloned()
                .collect()
        };
        self.files = Arc::new(files.into_iter().map(Into::into).collect());
        self.files_key = Some(key);
    }

    /// 选中/取消选中一行；文件过滤词随选择一起清空。
    pub(crate) fn select_pkg(
        &mut self,
        pkg: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.list.selected == pkg {
            return;
        }
        self.list.select(pkg);
        self.selected_pkg = self.list.selected.clone();
        self.files_key = None;
        self.files_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.ensure_selected_icon();
        cx.notify();
    }

    /// 显示一条自动消失的提示。旧的清理任务靠代次判断，不会误清新提示。
    pub(crate) fn set_toast(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.dialogs.set_toast(text);
        self.toast_generation = self.toast_generation.wrapping_add(1);
        let generation = self.toast_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            _ = this.update(cx, |app, cx| {
                if app.toast_generation == generation {
                    app.dialogs.clear_toast();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for DebDeckApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_ui(window, cx);

        let content = if self.catalog.is_ready() {
            crate::ui::show(self, window, cx).into_any_element()
        } else {
            crate::ui::show_loading(self, window, cx).into_any_element()
        };
        let modal = crate::ui::modals::show(self, window, cx);
        let toast = crate::ui::show_toast(self, window, cx);
        let title_bar = TitleBar::new().on_close_window(|_, _, cx| cx.quit()).child(
            h_flex()
                .items_center()
                .gap(px(8.0))
                .children(self.app_icon.clone().map(|icon| {
                    img(icon)
                        .w(px(APP_ICON_SIZE))
                        .h(px(APP_ICON_SIZE))
                        .rounded(px(6.0))
                        .object_fit(ObjectFit::Cover)
                }))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(APP_NAME),
                )
                .child(div().flex_grow(1.0))
                .child(self_update_button(self, cx)),
        );

        div()
            .id("debdeck-root")
            .size_full()
            .relative()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                v_flex()
                    .size_full()
                    .child(title_bar)
                    .child(div().flex_1().min_h(px(0.0)).child(content)),
            )
            .children(modal)
            .children(toast)
    }
}

/// 标题栏版本入口：有更新时突出显示，点击打开同一更新弹窗。
fn self_update_button(app: &DebDeckApp, cx: &mut Context<DebDeckApp>) -> Button {
    let has_update = app.self_update.has_update();
    let available = app.self_update.available_version();
    let label = if let Some(version) = available.as_ref() {
        format!("可更新 v{version}")
    } else if has_update {
        "重启以完成更新".to_string()
    } else {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    };
    let mut button = Button::new("self-update-entry")
        .label(label)
        .compact()
        .tooltip("检查并安装 DebDeck 更新")
        .on_click(cx.listener(|this, _, _, cx| {
            this.open_self_update_dialog(cx);
        }));
    button = if has_update {
        button.primary()
    } else {
        button.ghost()
    };
    button
}
