//! 数据缓存（Catalog）：系统读到的原始数据，以及由它派生的列表条目。
//!
//! 这里只接受 [`CatalogUpdate`] 形式的批量更新，一次更新最多重建一次条目表。
//! `revision` 单调递增，[`crate::app::list::ListView`] 用它判断自己缓存的下标是否
//! 仍然有效 —— 取代原先「谁忘了把 dirty 置位就 panic」的人肉约定。

use std::collections::{HashMap, HashSet};

use crate::model::{
    AppEntry, InstalledData, InstalledPackage, MenuEntryState, RepoPackage, RepositoryData,
};
use crate::runtime::{CatalogUpdate, UpgradableReport};

use super::state::{Counts, LoadState};

/// 「现在系统里有什么」的唯一持有者。
pub(crate) struct Catalog {
    installed: Option<InstalledData>,
    repository: Option<RepositoryData>,
    /// APT 认定的可升级包集合；`None` 表示当前只能靠本地版本比较（降级判定）。
    upgradable: Option<HashSet<String>>,
    upgradable_degraded: bool,
    entries: Vec<AppEntry>,
    /// 包名 → `entries` 下标。
    index: HashMap<String, usize>,
    counts: Counts,
    /// 每次重建条目表自增一次。
    revision: usize,
    load: LoadState,
    repository_loading: bool,
}

impl Catalog {
    pub(crate) fn new() -> Self {
        Self {
            installed: None,
            repository: None,
            upgradable: None,
            upgradable_degraded: false,
            entries: Vec::new(),
            index: HashMap::new(),
            counts: Counts::default(),
            revision: 0,
            load: LoadState::Loading {
                stage: "正在启动…".to_string(),
            },
            repository_loading: true,
        }
    }

    /// 应用一次后端更新。返回是否改变了条目表。
    pub(crate) fn apply(&mut self, update: CatalogUpdate) -> bool {
        let mut changed = false;

        if let Some(result) = update.installed {
            match result {
                Ok(data) => {
                    self.installed = Some(data);
                    self.load = LoadState::Ready;
                    changed = true;
                }
                Err(error) => {
                    self.load = LoadState::Failed {
                        stage: self.load.stage().to_string(),
                        error,
                    };
                }
            }
        }

        if let Some(repository) = update.repository {
            self.repository = Some(repository);
            self.repository_loading = false;
            changed = true;
        }

        if let Some(report) = update.upgradable {
            match report {
                UpgradableReport::Queried(names) => {
                    self.upgradable = Some(names);
                    self.upgradable_degraded = false;
                }
                UpgradableReport::Failed => {
                    // 保留 None：条目表会退回本地版本比较；同时记住这是降级判定，
                    // 状态栏要如实告诉用户「更新状态未确认」。
                    self.upgradable = None;
                    self.upgradable_degraded = true;
                }
            }
            changed = true;
        }

        if changed {
            self.rebuild();
        }
        changed
    }

    /// 阶段提示只在加载期间生效，就绪后不该再改标题。
    pub(crate) fn set_stage(&mut self, stage: String) {
        if let LoadState::Loading { stage: current } = &mut self.load {
            *current = stage;
        }
    }

    /// 重建条目表与统计。
    fn rebuild(&mut self) {
        let empty_repository = RepositoryData::default();
        let repository = self.repository.as_ref().unwrap_or(&empty_repository);
        self.entries = match &self.installed {
            Some(installed) => {
                crate::model::build_entries(installed, repository, self.upgradable.as_ref())
            }
            None => Vec::new(),
        };

        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.pkg.clone(), position))
            .collect();

        self.counts = Counts {
            total: self.entries.len(),
            installed: self
                .entries
                .iter()
                .filter(|entry| entry.is_installed)
                .count(),
            gui: self.entries.iter().filter(|entry| entry.is_gui).count(),
            held: self.entries.iter().filter(|entry| entry.is_held).count(),
            upgradable: self.entries.iter().filter(|entry| entry.has_update).count(),
        };
        self.revision += 1;
    }

    /// 更新某个已安装包的桌面菜单可见性（只改内存视图，文件由 backend 处理）。
    pub(crate) fn set_desktop_menu_state(&mut self, pkg: &str, state: MenuEntryState) {
        if let Some(package) = self
            .installed
            .as_mut()
            .and_then(|data| data.packages.get_mut(pkg))
            && let Some(desktop) = package.desktop.as_mut()
        {
            desktop.menu_state = state;
        }
    }

    pub(crate) fn entries(&self) -> &[AppEntry] {
        &self.entries
    }

    pub(crate) fn entry(&self, pkg: &str) -> Option<&AppEntry> {
        self.index.get(pkg).map(|&position| &self.entries[position])
    }

    pub(crate) fn counts(&self) -> &Counts {
        &self.counts
    }

    /// 条目表版本号，供 [`crate::app::list::ListView`] 校验下标。
    pub(crate) fn revision(&self) -> usize {
        self.revision
    }

    /// 更新判定是否退回本地版本比较。界面上要如实标注。
    pub(crate) fn upgradable_degraded(&self) -> bool {
        self.upgradable_degraded
    }

    pub(crate) fn load(&self) -> &LoadState {
        &self.load
    }

    pub(crate) fn load_mut(&mut self) -> &mut LoadState {
        &mut self.load
    }

    pub(crate) fn is_ready(&self) -> bool {
        matches!(self.load, LoadState::Ready)
    }

    pub(crate) fn repository_loading(&self) -> bool {
        self.repository_loading
    }

    pub(crate) fn set_repository_loading(&mut self, loading: bool) {
        self.repository_loading = loading;
    }

    pub(crate) fn repository_warnings(&self) -> &[String] {
        self.repository
            .as_ref()
            .map_or(&[] as &[String], |repository| {
                repository.warnings.as_slice()
            })
    }

    /// 软件源里是否有条目。索引为空时界面不该宣称「已加载」。
    pub(crate) fn has_packages(&self) -> bool {
        self.repository
            .as_ref()
            .is_some_and(|repository| !repository.packages.is_empty())
    }

    /// 某个包在软件源里的全部候选版本（降序，由 `RepositoryData` 保证）。
    pub(crate) fn versions(&self, pkg: &str) -> &[RepoPackage] {
        self.repository
            .as_ref()
            .map_or(&[] as &[RepoPackage], |repository| repository.versions(pkg))
    }

    pub(crate) fn installed_version(&self, pkg: &str) -> Option<(String, String)> {
        self.installed
            .as_ref()
            .and_then(|data| data.packages.get(pkg))
            .map(|package| {
                (
                    package.metadata.version.clone(),
                    package.metadata.arch.clone(),
                )
            })
    }

    /// 某个包的完整已安装记录（文件清单、桌面条目等）。
    pub(crate) fn installed_package(&self, pkg: &str) -> Option<&InstalledPackage> {
        self.installed
            .as_ref()
            .and_then(|data| data.packages.get(pkg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deb822::Stanza;
    use crate::model::{DpkgState, PackageMetadata};

    fn installed_with(name: &str, version: &str) -> InstalledData {
        let mut data = InstalledData::default();
        data.packages.insert(
            name.to_string(),
            InstalledPackage {
                metadata: PackageMetadata {
                    name: name.to_string(),
                    version: version.to_string(),
                    arch: "amd64".to_string(),
                    summary: "s".to_string(),
                    ..Default::default()
                },
                dpkg: DpkgState::parse("install ok installed"),
                control: Stanza::default(),
                files: vec![],
                desktop: None,
                updated: None,
            },
        );
        data
    }

    #[test]
    fn single_update_rebuilds_once() {
        let mut catalog = Catalog::new();
        let revision_before = catalog.revision();
        let changed = catalog.apply(
            CatalogUpdate::default()
                .installed(Ok(installed_with("demo", "1.0")))
                .upgradable(UpgradableReport::Queried(HashSet::new())),
        );
        assert!(changed);
        // 两个字段合并在一个事件里，只推进一个版本号。
        assert_eq!(catalog.revision(), revision_before + 1);
        assert!(catalog.is_ready());
        assert_eq!(catalog.entries().len(), 1);
        assert_eq!(catalog.counts().installed, 1);
    }

    #[test]
    fn failed_load_keeps_stage_and_reports_error() {
        let mut catalog = Catalog::new();
        catalog.set_stage("正在读取已安装应用…".to_string());
        catalog.apply(CatalogUpdate::default().installed(Err("没有权限".to_string())));
        assert!(!catalog.is_ready());
        assert_eq!(catalog.load().error(), Some("没有权限"));
        assert_eq!(catalog.load().stage(), "正在读取已安装应用…");
        // 失败不推进版本号：条目表没变。
        assert_eq!(catalog.revision(), 0);
    }

    #[test]
    fn upgradable_failure_is_marked_degraded() {
        let mut catalog = Catalog::new();
        catalog.apply(CatalogUpdate::default().upgradable(UpgradableReport::Failed));
        assert!(catalog.upgradable_degraded());

        catalog.apply(
            CatalogUpdate::default().upgradable(UpgradableReport::Queried(HashSet::from([
                "demo".to_string()
            ]))),
        );
        assert!(!catalog.upgradable_degraded());
        assert_eq!(catalog.counts().total, 0, "还没有已安装数据时不应有条目");
    }
}
