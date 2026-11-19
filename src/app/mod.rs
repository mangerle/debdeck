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
mod bootstrap;
mod catalog;
mod dialogs;
mod files;
mod icons;
mod install;
mod list;
mod self_update;
mod state;
mod update;

pub use bootstrap::run;
pub(crate) use catalog::Catalog;
pub(crate) use dialogs::Dialogs;
pub(crate) use files::DetailFilesState;
pub(crate) use icons::IconCache;
pub(crate) use install::{InstallApp, InstallFile, PasswordUi, Phase};
pub(crate) use list::ListView;
pub(crate) use self_update::{SelfUpdate, SelfUpdatePrimaryAction, SelfUpdateSnapshot};
// UI 层需要的状态类型集中再导出：`state` 本身保持私有，避免绘制代码绕过
// `app` 的门面去抓内部模块。
pub(crate) use state::{
    Action, Confirm, DebDeckApp, DetailTab, DownloadDialog, Filter, ModalState, Operation,
    RowStatus, SortKey, SortSelect, refresh_allowed,
};

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use gpui_kit::component::input::InputState;
use gpui_kit::component::select::{SearchableVec, SelectState};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::*;

use crate::runtime::{BackendEvent, TaskManager, Waker};
use crate::ui::DETAIL_ICON_TEXTURE_SIZE;
use crate::ui::password_input::{PasswordInputEvent, PasswordInputState};
use crate::util::AppPaths;

use bootstrap::{bind_refresh_action, create_inputs, create_sort_select, sort_from_label};

/// 在后台执行阻塞或短时 I/O 任务，完成后回到实体主线程应用结果。
pub(super) fn spawn_background<M, T, W, F>(cx: &mut Context<M>, work: W, apply: F)
where
    M: 'static,
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    F: FnOnce(&mut M, T, &mut Context<M>) + 'static,
{
    cx.spawn(async move |this, cx| {
        let result = cx.background_executor().spawn(async move { work() }).await;
        _ = this.update(cx, move |model, cx| apply(model, result, cx));
    })
    .detach();
}

/// 持续消费异步事件通道并派发回实体主线程，直到通道关闭或实体销毁。
pub(super) fn spawn_event_loop<M, E, F>(
    rx: async_channel::Receiver<E>,
    cx: &mut Context<M>,
    mut on_event: F,
) where
    M: 'static,
    E: Send + 'static,
    F: FnMut(&mut M, E, &mut Context<M>) + 'static,
{
    cx.spawn(async move |this, cx| {
        while let Ok(event) = rx.recv().await {
            if this
                .update(cx, |model, cx| on_event(model, event, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
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
    let (tasks, rx) = TaskManager::start_system(wake);
    tasks.load_initial();
    (tasks, rx, wake_rx)
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
    cx.observe(search_input, |this, state, cx| {
        let value = state.read(cx).value().to_string();
        this.schedule_search(value, cx);
    })
    .detach();
    cx.observe(files_input, |this, state, cx| {
        let value = state.read(cx).value().to_string();
        this.set_files_query(value, cx);
    })
    .detach();
}

fn update_sort_from_select(app: &mut DebDeckApp, state: &SortSelect, cx: &mut Context<DebDeckApp>) {
    let Some(label) = state.read(cx).selected_value().copied() else {
        return;
    };
    let (sort, ascending) = sort_from_label(label);
    app.set_sort(sort, ascending, cx);
}

impl DebDeckApp {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        restart_requested: Arc<AtomicBool>,
    ) -> Self {
        let paths = AppPaths::new();
        let (tasks, rx, wake_rx) = start_backend();
        let (search_input, files_input) = create_inputs(window, cx);
        let sort_select = create_sort_select(window, cx);
        let operation_password_input = cx.new(PasswordInputState::new);
        cx.subscribe(&operation_password_input, |app, _, event, cx| {
            if matches!(event, PasswordInputEvent::Submit) {
                app.submit_operation_password(cx);
            }
        })
        .detach();
        observe_inputs(&search_input, &files_input, &sort_select, cx);
        spawn_event_loop(wake_rx, cx, |app, (), cx| {
            app.poll_events(cx);
            cx.notify();
        });
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
            operation_password_input,
            window_handle: window.window_handle(),
            files_state: DetailFilesState::new(),
            toast_generation: 0,
            self_update: self_update::SelfUpdate::default(),
            restart_requested,
            update_confirm_task: None,
            search_debounce_task: None,
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

    /// 在状态变更路径重算可见行，不在渲染函数里做状态突变。
    pub(crate) fn sync_catalog_view(&mut self, cx: &mut Context<Self>) {
        self.list.ensure_current(&self.catalog);
        if self
            .files_state
            .note_selection_if_changed(self.list.selected.as_deref())
        {
            self.schedule_clear_files_input(cx);
        }
        self.ensure_files();
        self.ensure_selected_icon();
    }

    pub(crate) fn set_filter(
        &mut self,
        filter: Filter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.list.filter == filter {
            return;
        }
        self.list.filter = filter;
        self.list.ensure_current(&self.catalog);
        if self
            .files_state
            .note_selection_if_changed(self.list.selected.as_deref())
        {
            self.flush_clear_files_input(window, cx);
        }
        self.ensure_files();
        self.ensure_selected_icon();
        cx.notify();
    }

    /// 防抖触发搜索：清空关键词立即生效，非空输入延迟 120ms 避免连续击键反复重排 7 万条记录。
    pub(crate) fn schedule_search(&mut self, search: String, cx: &mut Context<Self>) {
        if search.is_empty() {
            self.search_debounce_task = None;
            self.set_search(search, cx);
            return;
        }
        if self.list.search == search {
            self.search_debounce_task = None;
            return;
        }
        self.search_debounce_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            _ = this.update(cx, |app, cx| app.set_search(search, cx));
        }));
    }

    pub(crate) fn set_search(&mut self, search: String, cx: &mut Context<Self>) {
        if self.list.search == search {
            return;
        }
        self.list.search = search;
        self.sync_catalog_view(cx);
        cx.notify();
    }

    pub(crate) fn set_sort(&mut self, sort: SortKey, ascending: bool, cx: &mut Context<Self>) {
        if self.list.sort == sort && self.list.ascending == ascending {
            return;
        }
        self.list.sort = sort;
        self.list.ascending = ascending;
        self.sync_catalog_view(cx);
        cx.notify();
    }

    pub(crate) fn set_files_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.list.files_query == query {
            return;
        }
        self.list.files_query = query;
        self.ensure_files();
        cx.notify();
    }

    pub(crate) fn clear_files_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list.files_query.clear();
        self.files_state.invalidate_filter();
        self.files_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.ensure_files();
        cx.notify();
    }

    fn flush_clear_files_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.files_state.take_pending_input_clear() {
            self.files_input
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    fn schedule_clear_files_input(&self, cx: &mut Context<Self>) {
        let window_handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.flush_clear_files_input(window, cx);
                });
            });
        })
        .detach();
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

    pub(crate) fn ensure_files(&mut self) {
        self.files_state.sync(
            &self.catalog,
            self.list.selected.as_deref(),
            &self.list.files_query,
            &self.tasks,
        );
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
        self.files_state
            .reset_for_selection(self.list.selected.as_deref());
        self.files_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.ensure_files();
        self.ensure_selected_icon();
        cx.notify();
    }

    /// 提交当前操作弹窗里的管理员密码。
    pub(crate) fn submit_operation_password(&mut self, cx: &mut Context<Self>) {
        let password = self.operation_password_input.read(cx).value().to_string();
        if self.dialogs.submit_operation_password(password) {
            self.operation_password_input
                .update(cx, PasswordInputState::clear);
            cx.notify();
        }
    }

    /// 异步清空并聚焦操作弹窗密码输入框；`echo` 为真时按明文提示关闭掩码。
    pub(crate) fn reset_and_focus_operation_password(
        &mut self,
        echo: bool,
        cx: &mut Context<Self>,
    ) {
        self.operation_password_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_masked(!echo, cx);
        });
        let window_handle = self.window_handle;
        let input = self.operation_password_input.clone();
        cx.spawn(async move |_, cx| {
            let _ = window_handle.update(cx, |_, window, cx| {
                input.update(cx, |state, cx| state.focus(window, cx));
            });
        })
        .detach();
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
        let content = if self.catalog.is_ready() {
            crate::ui::show(self, window, cx).into_any_element()
        } else {
            crate::ui::show_loading(self, window, cx).into_any_element()
        };
        let modal = crate::ui::modals::show(self, window, cx);
        let toast = crate::ui::show_toast(self, window, cx);
        let title_bar = crate::ui::show_title_bar(self, cx);

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
