//! 将已安装软件包与软件源数据合并为界面列表条目。

use std::collections::HashSet;
use std::time::SystemTime;

use crate::util::{build_search_text, debian_version_cmp};

use super::package::{DesktopEntry, InstalledData, PackageMetadata};
use super::repository::RepositoryData;

/// 界面列表里的一行。
///
/// 四个布尔位是视图需要的独立状态（是否安装 / 是否有图形入口 / 是否保持版本 /
/// 是否有更新），各自单独显示，改成位域或枚举反而更难读，
/// 因此定点放行 `struct_excessive_bools`。
#[allow(
    clippy::struct_excessive_bools,
    reason = "四个布尔值分别描述独立展示属性，组合枚举不能更准确表达状态"
)]
#[derive(Clone, Debug)]
pub struct AppEntry {
    pub pkg: String,
    pub display_name: String,
    pub is_installed: bool,
    pub is_gui: bool,
    pub is_held: bool,
    pub has_update: bool,
    pub installed_size_kb: u64,
    pub updated: Option<SystemTime>,
    pub latest_version: Option<String>,
    pub search_text: String,
    pub sort_name: String,
}

/// 合并已安装与软件源数据，生成稳定排序的应用列表。
pub fn build_entries(
    installed: &InstalledData,
    repository: &RepositoryData,
    upgradable: Option<&HashSet<String>>,
) -> Vec<AppEntry> {
    let mut names: Vec<&str> = Vec::with_capacity(
        installed
            .packages
            .len()
            .saturating_add(repository.packages.len()),
    );
    names.extend(installed.packages.keys().map(String::as_str));
    names.extend(repository.packages.keys().map(String::as_str));
    names.sort_unstable();
    names.dedup();

    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        entries.push(build_entry(name, installed, repository, upgradable));
    }
    entries
}

fn build_entry(
    name: &str,
    installed: &InstalledData,
    repository: &RepositoryData,
    upgradable: Option<&HashSet<String>>,
) -> AppEntry {
    let installed_pkg = installed.primary(name);
    let installed_meta = installed_pkg.map(|pkg| &pkg.metadata);
    let latest = repository.latest(name).map(|package| &package.metadata);
    let desktop = installed_pkg.and_then(|pkg| pkg.desktop.as_ref());
    let display_name = desktop
        .map(|desktop| desktop.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| name.to_string());
    let summary = entry_summary(desktop, installed_meta, latest);
    let (search_text, sort_name) = build_search_text(&display_name, name, &summary);
    let has_update = has_update(installed_meta, latest, upgradable);
    let installed_size_kb = installed_meta
        .map(|metadata| metadata.installed_size_kb)
        .or_else(|| latest.map(|metadata| metadata.installed_size_kb))
        .unwrap_or(0);
    AppEntry {
        pkg: name.to_string(),
        display_name,
        is_installed: installed_pkg.is_some(),
        is_gui: desktop.is_some(),
        is_held: installed_pkg.is_some_and(|pkg| pkg.dpkg.is_held()),
        has_update,
        installed_size_kb,
        updated: installed_pkg.and_then(|pkg| pkg.updated),
        latest_version: latest.map(|metadata| metadata.version.clone()),
        search_text,
        sort_name,
    }
}

fn entry_summary(
    desktop: Option<&DesktopEntry>,
    installed: Option<&PackageMetadata>,
    latest: Option<&PackageMetadata>,
) -> String {
    desktop
        .map(|desktop| desktop.comment.clone())
        .filter(|comment| !comment.is_empty())
        .or_else(|| {
            installed
                .map(|metadata| metadata.summary.clone())
                .filter(|summary| !summary.is_empty())
        })
        .or_else(|| latest.map(|metadata| metadata.summary.clone()))
        .unwrap_or_default()
}

/// 更新判定以 `apt list --upgradable` 为权威；集合尚未取回时退回本地版本比较。
///
/// 两者语义有差异（apt 不把已保持版本的包算作可升级），因此 upgradable 存在时
/// 完全忽略回退分支，避免同一个列表里混用两套规则。
pub fn has_update(
    installed: Option<&PackageMetadata>,
    latest: Option<&PackageMetadata>,
    upgradable: Option<&HashSet<String>>,
) -> bool {
    match (installed, latest) {
        (Some(installed), Some(latest)) => match upgradable {
            Some(upgradable) => upgradable.contains(&installed.name),
            None => debian_version_cmp(&latest.version, &installed.version).is_gt(),
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::package::{DpkgSelection, DpkgStatus};
    use super::*;
    use crate::model::{DpkgState, InstalledPackage, RepoPackage};

    fn metadata(name: &str, version: &str) -> PackageMetadata {
        PackageMetadata {
            name: name.to_string(),
            version: version.to_string(),
            arch: crate::model::intern_arch("amd64"),
            summary: "summary".to_string(),
            ..Default::default()
        }
    }

    fn repo(name: &str, version: &str) -> RepoPackage {
        RepoPackage {
            metadata: metadata(name, version),
            ..Default::default()
        }
    }

    #[test]
    fn merges_installed_and_repository_packages() {
        let mut installed = InstalledData::default();
        installed.record(InstalledPackage {
            metadata: metadata("hello", "1.0"),
            dpkg: DpkgState {
                selection: DpkgSelection::Install,
                status: DpkgStatus::Installed,
            },
            desktop: None,
            updated: None,
        });
        let mut repository = RepositoryData::default();
        repository
            .packages
            .insert("hello".to_string(), vec![repo("hello", "2.0")]);
        repository
            .packages
            .insert("world".to_string(), vec![repo("world", "1.0")]);

        let entries = build_entries(&installed, &repository, None);
        assert_eq!(entries.len(), 2);
        let hello = entries.iter().find(|entry| entry.pkg == "hello").unwrap();
        assert!(hello.is_installed);
        assert!(hello.has_update);
        let world = entries.iter().find(|entry| entry.pkg == "world").unwrap();
        assert!(!world.is_installed);
    }
}
