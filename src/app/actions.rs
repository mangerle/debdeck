//! 动作分发：`ui` 产出的 [`Action`] 在这里变成真正的副作用。
//!
//! 划分规则很简单：
//!
//! * **会阻塞的**（卸载、保持版本、重打包、下载）交给 [`crate::runtime::TaskManager`]
//!   排队，并立刻换成一个进度模态；
//! * **瞬时完成的**（启动程序、复制文本）直接调用后端；
//! * **文件系统或外部进程副作用**（打开目录、修改菜单覆盖文件）提交到 GPUI
//!   后台执行器，完成后再回到 UI 线程更新提示。

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use gpui_kit::{ClipboardItem, Context};

use crate::app::self_update::SelfUpdatePrimaryAction;
use crate::app::spawn_background;
use crate::app::state::{Action, Confirm, DebDeckApp, DownloadDialog, ModalState, Operation};
use crate::model::{MenuEntryState, RepoPackage};
use crate::runtime::{
    BackendEvent, DownloadRequest, launch_desktop, open_directory,
    set_menu_visibility as runtime_set_menu,
};

struct MenuVisibilityChange {
    pkg: String,
    name: String,
    path: PathBuf,
    hidden: bool,
}

fn finish_menu_visibility(
    app: &mut DebDeckApp,
    change: MenuVisibilityChange,
    result: Result<MenuEntryState, String>,
    cx: &mut Context<DebDeckApp>,
) {
    let MenuVisibilityChange {
        pkg, name, hidden, ..
    } = change;
    match result {
        Ok(state) => {
            app.catalog.set_desktop_menu_state(&pkg, state);
            let message = if hidden {
                format!("已隐藏「{name}」的应用菜单项（菜单刷新可能需要几秒）")
            } else {
                format!("已恢复「{name}」的应用菜单项")
            };
            app.set_toast(message, cx);
        }
        Err(error) => app.dialogs.set_message("无法修改应用菜单项", error),
    }
    cx.notify();
}

fn finish_open_directory(
    app: &mut DebDeckApp,
    path: &Path,
    result: Result<(), String>,
    cx: &mut Context<DebDeckApp>,
) {
    if let Err(error) = result {
        app.dialogs
            .set_message("无法打开目录", format!("{}\n{error}", path.display()));
    }
    cx.notify();
}

impl DebDeckApp {
    pub(crate) fn handle_action(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::Open { desktop, name } => match launch_desktop(&desktop) {
                Ok(()) => self.set_toast(format!("正在打开 {name}"), cx),
                Err(error) => self.dialogs.set_message("无法打开应用", error),
            },
            Action::OpenDownloads => {
                let path = self.paths.downloads.clone();
                Self::open_directory(path, cx);
            }
            Action::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_toast("已复制到剪贴板", cx);
            }
            Action::Uninstall { pkg, name } => {
                self.dialogs.modal = Some(ModalState::Confirm(Confirm::Uninstall { pkg, name }));
            }
            Action::ToggleHold { pkg, name, held } => {
                self.dialogs.modal = Some(ModalState::Confirm(Confirm::Hold {
                    pkg,
                    name,
                    hold: !held,
                }));
            }
            Action::Build { pkg, name } => {
                self.dialogs.modal = Some(ModalState::Confirm(Confirm::Build { pkg, name }));
            }
            Action::SetMenuVisibility {
                pkg,
                name,
                path,
                hidden,
            } => Self::set_menu_visibility(
                MenuVisibilityChange {
                    pkg,
                    name,
                    path,
                    hidden,
                },
                cx,
            ),
            Action::Download { pkg, name } => self.open_download_dialog(pkg, name),
            Action::Refresh => self.refresh_all(),
        }
        cx.notify();
    }

    fn set_menu_visibility(change: MenuVisibilityChange, cx: &mut Context<Self>) {
        let path = change.path.clone();
        let hidden = change.hidden;
        spawn_background(
            cx,
            move || runtime_set_menu(&path, hidden),
            move |app, result, cx| finish_menu_visibility(app, change, result, cx),
        );
    }

    /// 在文件管理器里异步打开目录，完成后在 UI 线程显示错误。
    pub(crate) fn open_directory(path: PathBuf, cx: &mut Context<Self>) {
        let task_path = path.clone();
        spawn_background(
            cx,
            move || open_directory(&task_path),
            move |app, result, cx| finish_open_directory(app, &path, result, cx),
        );
    }

    /// 组装下载对话框：只列出真正有下载地址的版本。
    fn open_download_dialog(&mut self, pkg: String, name: String) {
        let versions: Vec<RepoPackage> = self
            .catalog
            .versions(&pkg)
            .iter()
            .filter(|package| package.download_url().is_some())
            .cloned()
            .collect();
        if versions.is_empty() {
            self.dialogs.set_message(
                "无法下载安装包",
                "APT 软件源中没有找到该软件包的可下载版本。请先执行 sudo apt update，或检查软件源配置。",
            );
            return;
        }

        // 默认选中当前已安装的那个版本（若存在），否则给最新版。
        let selected = self
            .catalog
            .installed_version(&pkg)
            .and_then(|(version, arch)| {
                versions.iter().position(|package| {
                    package.metadata.version == version && package.metadata.arch == arch
                })
            })
            .unwrap_or(0);

        self.dialogs.modal = Some(ModalState::Download(DownloadDialog {
            pkg,
            name,
            versions,
            selected,
            destination_dir: self.paths.downloads.clone(),
        }));
    }

    pub(crate) fn start_uninstall(&mut self, pkg: String, name: &str) {
        let id = match self.tasks.uninstall(pkg) {
            Ok(id) => id,
            Err(error) => {
                self.dialogs
                    .set_message("无法卸载软件包", error.to_string());
                return;
            }
        };
        self.dialogs.begin_operation(Operation {
            id,
            title: format!("卸载 {name}"),
            message: "正在请求管理员授权并卸载…".to_string(),
            progress: None,
            result: None,
            cancel: None,
            open_dir: None,
            cancel_requested: false,
        });
    }

    pub(crate) fn start_hold(&mut self, pkg: String, name: &str, hold: bool) {
        let id = match self.tasks.hold(pkg, hold) {
            Ok(id) => id,
            Err(error) => {
                self.dialogs
                    .set_message("无法修改版本状态", error.to_string());
                return;
            }
        };
        let title = if hold {
            format!("保持版本：{name}")
        } else {
            format!("取消保持版本：{name}")
        };
        self.dialogs.begin_operation(Operation {
            id,
            title,
            message: "正在请求管理员授权…".to_string(),
            progress: None,
            result: None,
            cancel: None,
            open_dir: None,
            cancel_requested: false,
        });
    }

    pub(crate) fn start_build(&mut self, pkg: &str, name: &str) {
        let Some(package) = self.catalog.installed_package(pkg).cloned() else {
            self.dialogs
                .set_message("无法提取安装包", "该软件包当前没有安装信息。");
            return;
        };
        let out_dir = self.paths.builds.clone();
        let (id, cancel) = match self.tasks.build(package, out_dir.clone()) {
            Ok(operation) => operation,
            Err(error) => {
                self.dialogs
                    .set_message("无法提取安装包", error.to_string());
                return;
            }
        };
        self.dialogs.begin_operation(Operation {
            id,
            title: format!("离线提取安装包：{name}"),
            message: "正在准备…".to_string(),
            progress: None,
            result: None,
            cancel: Some(cancel),
            open_dir: Some(out_dir),
            cancel_requested: false,
        });
    }

    /// `destination` 由对话框计算后传入：预览与落盘必须是同一个路径。
    pub(crate) fn start_download(
        &mut self,
        name: &str,
        package: &RepoPackage,
        destination: PathBuf,
    ) {
        let Some(url) = package.download_url() else {
            self.dialogs
                .set_message("无法下载安装包", "该版本没有可用的下载地址。");
            return;
        };
        let open_dir = destination
            .parent()
            .map_or_else(|| self.paths.downloads.clone(), Path::to_path_buf);
        let (id, cancel) = match self.tasks.download(DownloadRequest {
            url,
            destination,
            package_name: package.metadata.name.clone(),
            sha256: (!package.sha256.is_empty()).then_some(package.sha256.clone()),
        }) {
            Ok(operation) => operation,
            Err(error) => {
                self.dialogs.set_message("无法开始下载", error.to_string());
                return;
            }
        };
        self.dialogs.begin_operation(Operation {
            id,
            title: format!("下载 {name}"),
            message: "正在连接软件源…".to_string(),
            progress: Some((0, 0)),
            result: None,
            cancel: Some(cancel),
            open_dir: Some(open_dir),
            cancel_requested: false,
        });
    }

    /// 确认框按下「继续」后的分派。
    pub(crate) fn start_confirmed(&mut self, confirm: Confirm) {
        match confirm {
            Confirm::Uninstall { pkg, name } => self.start_uninstall(pkg, &name),
            Confirm::Hold { pkg, name, hold } => self.start_hold(pkg, &name, hold),
            Confirm::Build { pkg, name } => self.start_build(&pkg, &name),
        }
    }

    /// 启动后检查一次远端清单；只更新标题栏状态，不主动弹出窗口。
    pub(crate) fn start_startup_update_check(&mut self) {
        if !self.self_update.is_idle() {
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
        let Some(info) = self.self_update.available_info() else {
            return;
        };
        let version = info.version();
        self.self_update.begin_download(version);
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
        let Some(downloaded) = self.self_update.downloaded_update() else {
            return;
        };
        let version = downloaded.version();
        self.self_update.begin_install(version);
        if let Err(error) = self.tasks.install_self_update(downloaded) {
            self.self_update.fail_submit("无法开始安装更新", &error);
        }
        cx.notify();
    }

    /// 用户点击“立即重启”：请求主循环退出，旧进程收尾后由入口拉起新版本。
    pub(crate) fn restart_after_self_update(&mut self, cx: &mut Context<Self>) {
        if !self.self_update.is_installed() {
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
