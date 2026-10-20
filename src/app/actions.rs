//! 动作分发：`ui` 产出的 [`Action`] 在这里变成真正的副作用。
//!
//! 划分规则很简单：
//!
//! * **会阻塞的**（卸载、保持版本、重打包、下载）交给 [`crate::runtime::TaskManager`]
//!   排队，并立刻换成一个进度模态；
//! * **瞬时完成的**（启动程序、打开目录、复制文本）直接调用后端，不绕线程。

use std::path::{Path, PathBuf};

use crate::app::state::{
    Action, Confirm, DebDeckApp, DownloadDialog, ModalOutcome, ModalState, Operation,
};
use crate::backend::desktop::launch_desktop;
use crate::backend::shell;
use crate::model::RepoPackage;
use crate::runtime::{DownloadRequest, report};

impl DebDeckApp {
    pub(crate) fn handle_action(&mut self, action: Action) {
        match action {
            Action::Open { desktop, name } => match launch_desktop(&desktop) {
                Ok(()) => self.dialogs.set_toast(format!("正在打开 {name}")),
                Err(error) => self.dialogs.set_message("无法打开应用", report(&error)),
            },
            Action::OpenDownloads => {
                let path = self.paths.downloads.clone();
                self.open_directory(&path);
            }
            Action::Copy(text) => {
                self.ctx.copy_text(text);
                self.dialogs.set_toast("已复制到剪贴板");
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
            Action::Download { pkg, name } => self.open_download_dialog(pkg, name),
            Action::Refresh => self.refresh_all(),
        }
    }

    /// 在文件管理器里打开目录；失败时给出可操作的提示。
    fn open_directory(&mut self, path: &Path) {
        if let Err(error) = shell::open_directory(path) {
            self.dialogs
                .set_message("无法打开目录", format!("{}\n{error:#}", path.display()));
        }
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
        let id = self.tasks.uninstall(pkg);
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
        let id = self.tasks.hold(pkg, hold);
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
        let (id, cancel) = self.tasks.build(package, out_dir.clone());
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
        let open_dir = destination.parent().map_or_else(
            || self.paths.downloads.clone(),
            std::path::Path::to_path_buf,
        );
        let (id, cancel) = self.tasks.download(DownloadRequest {
            url,
            destination,
            package_name: package.metadata.name.clone(),
            sha256: (!package.sha256.is_empty()).then_some(package.sha256.clone()),
        });
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

    /// 应用模态框交回的结果。
    pub(crate) fn apply_modal_outcome(&mut self, outcome: ModalOutcome) {
        match outcome {
            ModalOutcome::Confirm(confirm) => self.start_confirmed(confirm),
            ModalOutcome::StartDownload(start) => {
                self.start_download(&start.name, &start.package, start.destination);
            }
            ModalOutcome::OpenDir(path) => self.open_directory(&path),
        }
    }
}
