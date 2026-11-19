//! 进程启动引导、命令行参数解析、窗口配置与日志初始化。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::IndexPath;
use gpui_kit::component::TitleBar;
use gpui_kit::component::input::InputState;
use gpui_kit::component::select::{SearchableVec, SelectState};
use gpui_kit::*;

use crate::runtime::{
    InstanceLock, RecoveryStatus, check_and_recover_on_startup, notify, restart_current_process,
};
use crate::ui::window_icon;
use crate::util::APP_ID;

use super::install::{self, InstallApp};
use super::state::{Action, DebDeckApp, SortKey};

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
            notify("DebDeck 已在运行", "已有 DebDeck 实例正在运行。");
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

pub(super) fn create_inputs(
    window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> (Entity<InputState>, Entity<InputState>) {
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索应用名、包名或拼音"));
    let files = cx.new(|cx| InputState::new(window, cx).placeholder("在文件列表中查找"));
    (search, files)
}

pub(super) fn create_sort_select(
    window: &mut Window,
    cx: &mut Context<DebDeckApp>,
) -> Entity<SelectState<SearchableVec<&'static str>>> {
    const LABELS: [&str; 6] = [
        "名称 ↑",
        "名称 ↓",
        "大小 ↑",
        "大小 ↓",
        "更新时间 ↑",
        "更新时间 ↓",
    ];
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(LABELS.to_vec()),
            Some(IndexPath::new(0)),
            window,
            cx,
        )
    })
}

pub(super) fn sort_from_label(label: &str) -> (SortKey, bool) {
    match label {
        "名称 ↓" => (SortKey::Name, false),
        "大小 ↑" => (SortKey::Size, true),
        "大小 ↓" => (SortKey::Size, false),
        "更新时间 ↑" => (SortKey::Updated, true),
        "更新时间 ↓" => (SortKey::Updated, false),
        _ => (SortKey::Name, true),
    }
}

pub(super) fn bind_refresh_action(cx: &mut Context<DebDeckApp>) {
    cx.bind_keys([KeyBinding::new("f5", Refresh, None)]);
    let weak = cx.entity().downgrade();
    App::on_action(&mut *cx, move |_: &Refresh, cx: &mut App| {
        weak.update(cx, |app, cx| {
            app.handle_action(Action::Refresh, cx);
        })
        .ok();
    });
}
