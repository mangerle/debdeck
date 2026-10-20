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
mod list;
mod state;
mod update;

pub(crate) use catalog::Catalog;
pub(crate) use dialogs::Dialogs;
pub(crate) use list::ListView;
// UI 层需要的状态类型集中再导出：`state` 本身保持私有，避免绘制代码绕过
// `app` 的门面去抓内部模块。
pub(crate) use state::{
    Action, Confirm, DebDeckApp, DetailTab, DownloadDialog, DownloadStart, Filter, IconCache,
    LoadState, ModalOutcome, ModalState, Operation, SortKey, refresh_allowed,
};

use std::sync::Arc;

use eframe::egui::{self, Ui};

use crate::backend::shell;
use crate::runtime::{InstanceLock, Waker};
use crate::ui::window_icon;
use crate::util::{APP_ID, APP_NAME, AppPaths};

/// 启动 DebDeck：初始化日志、抢单实例锁、开窗并进入事件循环。
///
/// 已有实例在运行时不是错误：发一条通知后正常退出，返回 `Ok(())`。
pub fn run() -> eframe::Result<()> {
    init_tracing();

    let _instance = match InstanceLock::acquire() {
        Ok(instance) => instance,
        Err(message) => {
            tracing::error!("{message}");
            shell::notify("DebDeck 已在运行", "已有 DebDeck 实例正在运行。");
            return Ok(());
        }
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1400.0, 880.0])
        .with_min_inner_size([1080.0, 680.0])
        .with_app_id(APP_ID);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }

    eframe::run_native(
        APP_NAME,
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(DebDeckApp::new(cc)))),
    )
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

impl DebDeckApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // 顺序固定：先装字体再配样式，否则首帧会用缺字形的默认字体排版。
        crate::ui::configure_fonts(&cc.egui_ctx);
        crate::ui::configure_style(&cc.egui_ctx);

        // 后端线程通过 wake 请求重绘，而不是自己碰界面状态。
        let wake_ctx = cc.egui_ctx.clone();
        let wake: Waker = Arc::new(move || wake_ctx.request_repaint());
        let layout = crate::backend::FsLayout::system();
        let (tasks, rx) = crate::runtime::TaskManager::start(wake, layout);
        tasks.load_initial();

        Self {
            ctx: cc.egui_ctx.clone(),
            paths: AppPaths::new(),
            tasks,
            rx,
            catalog: Catalog::new(),
            list: ListView::new(),
            dialogs: Dialogs::new(),
            icons: state::IconCache::new(),
        }
    }
}

impl eframe::App for DebDeckApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.poll_events();

        if !self.with_view(|view| view.catalog.is_ready()) {
            let retry =
                self.with_view(|view| crate::ui::show_loading_panel(ui, view.catalog.load()));
            self.with_view(|view| crate::ui::show_toast(view, ui));
            if retry {
                self.retry_initial_load();
            }
            return;
        }

        // 借用的时间窗只覆盖绘制本身：绘制结束后再把动作交回应用层执行，
        // 这样 UI 代码不需要持有整个 App，副作用也不会发生在绘制中途。
        let actions = self.with_view(|view| {
            view.ensure_current();
            crate::ui::show(view, ui)
        });
        for action in actions {
            self.handle_action(action);
        }

        let outcomes = self.with_view(|view| crate::ui::show_modals(view, ui));
        for outcome in outcomes {
            self.apply_modal_outcome(outcome);
        }

        self.with_view(|view| crate::ui::show_toast(view, ui));
    }
}

impl DebDeckApp {
    /// 在一次性借用内完成绘制相关的工作。
    fn with_view<R>(&mut self, draw: impl FnOnce(&mut crate::ui::View<'_>) -> R) -> R {
        let mut view = crate::ui::View::new(self);
        draw(&mut view)
    }
}
