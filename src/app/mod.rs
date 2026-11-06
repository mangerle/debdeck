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
//! [`run`] 会调用 `crate::ui::window_icon` 读取窗口图标 —— 这是启动期的一次性资源，
//! 不参与每帧绘制。绘制期的依赖方向仍然是单向的 `ui → app`。

mod actions;
mod catalog;
mod dialogs;
mod install;
mod list;
mod state;
mod update;

pub(crate) use catalog::Catalog;
pub(crate) use dialogs::Dialogs;
pub(crate) use install::{InstallApp, PasswordUi, Phase};
pub(crate) use list::ListView;
// UI 层需要的状态类型集中再导出：`state` 本身保持私有，避免绘制代码绕过
// `app` 的门面去抓内部模块。
pub(crate) use state::{
    Action, Confirm, DebDeckApp, DetailTab, DownloadDialog, Filter, IconCache, ModalState,
    Operation, RowStatus, SortKey, refresh_allowed,
};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::IndexPath;
use gpui_kit::component::input::InputState;
use gpui_kit::component::select::{SearchableVec, SelectState};
use gpui_kit::component::{ActiveTheme as _, TitleBar, h_flex, v_flex};
use gpui_kit::*;

use crate::backend::shell;
use crate::runtime::{InstanceLock, Waker};
use crate::ui::window_icon;
use crate::util::{APP_ID, APP_NAME, AppPaths};

gpui_kit::actions!(debdeck, [Refresh]);

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

    let _instance = match InstanceLock::acquire() {
        Ok(instance) => instance,
        Err(message) => {
            tracing::error!("{message}");
            shell::notify("DebDeck 已在运行", "已有 DebDeck 实例正在运行。");
            return;
        }
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);

            if let Err(error) = gpui_kit::open_window(window_options(), cx, |window, cx| {
                crate::ui::theme::configure(cx, Some(window));
                cx.new(|cx| DebDeckApp::new(window, cx))
            }) {
                tracing::error!("无法打开窗口：{error:#}");
                cx.quit();
            }
        });
}

/// 安装模式：单窗口、单安装包。
fn run_install_window(path: PathBuf) {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);

            if let Err(error) = gpui_kit::open_window(install_window_options(), cx, |window, cx| {
                crate::ui::theme::configure(cx, Some(window));
                cx.new(|cx| InstallApp::new(&path, window, cx))
            }) {
                tracing::error!("无法打开安装窗口：{error:#}");
                cx.quit();
            }
        });
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
    cx.spawn(async move |this, cx| {
        while wake_rx.recv().await.is_ok() {
            let alive = this
                .update(cx, |app, cx| {
                    app.poll_events();
                    cx.notify();
                })
                .is_ok();
            if !alive {
                break;
            }
        }
    })
    .detach();
}

impl DebDeckApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let paths = AppPaths::new();

        // 后端线程只发事件；专门的唤醒通道把「有事件了」翻译成一次主线程重绘。
        let (wake_tx, wake_rx) = async_channel::unbounded::<()>();
        let wake: Waker = Arc::new(move || {
            let _ = wake_tx.try_send(());
        });
        let layout = crate::backend::FsLayout::system();
        let (tasks, rx) = crate::runtime::TaskManager::start(wake, layout);
        tasks.load_initial();

        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索应用名、包名或拼音"));
        let files_input = cx.new(|cx| InputState::new(window, cx).placeholder("在文件列表中查找"));
        let sort_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![
                    "名称 ↑",
                    "名称 ↓",
                    "大小 ↑",
                    "大小 ↓",
                    "更新时间 ↑",
                    "更新时间 ↓",
                ]),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });

        cx.observe(&sort_select, |this, state, cx| {
            let Some(label) = state.read(cx).selected_value().copied() else {
                return;
            };
            let (sort, ascending) = sort_from_label(label);
            if this.list.sort != sort || this.list.ascending != ascending {
                this.list.sort = sort;
                this.list.ascending = ascending;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&search_input, |this, state, cx| {
            let value = state.read(cx).value().to_string();
            if this.list.search != value {
                this.list.search = value;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&files_input, |this, state, cx| {
            let value = state.read(cx).value().to_string();
            if this.list.files_query != value {
                this.list.files_query = value;
                cx.notify();
            }
        })
        .detach();

        spawn_wake_loop(wake_rx, cx);

        cx.bind_keys([KeyBinding::new("f5", Refresh, None)]);
        let weak = cx.entity().downgrade();
        gpui_kit::App::on_action(&mut *cx, move |_: &Refresh, cx: &mut App| {
            weak.update(cx, |app, cx| {
                app.handle_action(Action::Refresh, cx);
            })
            .ok();
        });

        Self {
            paths,
            tasks,
            rx,
            catalog: Catalog::new(),
            list: ListView::new(),
            dialogs: Dialogs::new(),
            icons: IconCache::new(),
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
        }
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
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(APP_NAME),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Ubuntu 应用管理器"),
                ),
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
