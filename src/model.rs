//! 与 IO 无关的领域数据。
//!
//! 这里只描述「软件包世界长什么样」：dpkg 状态、软件源条目、桌面条目，以及
//! 把已安装数据与软件源数据合并成界面条目（[`build_entries`]）的规则。
//! 读写系统的部分在 `backend`，线程与事件在 `runtime`，本模块全部是纯数据与纯函数。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::deb822::Stanza;
use crate::util::{build_search_text, debian_version_cmp};

/// 一个通过预检的本地 `.deb` 文件。
///
/// 只描述文件本身，不包含安装状态；读取与校验在 `backend::deb_file`。
#[derive(Clone, Debug)]
pub struct DebFile {
    /// 规范化后的绝对路径（符号链接已解析）。
    pub path: PathBuf,
    pub metadata: PackageMetadata,
    /// `.deb` 文件本身的字节数，不是安装后的占用。
    pub file_size: u64,
}

/// 软件包的控制元数据，字段名与 deb822 控制字段一一对应。
#[derive(Clone, Debug, Default)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub summary: String,
    pub description: String,
    pub maintainer: String,
    pub homepage: String,
    pub depends: String,
    pub installed_size_kb: u64,
}

impl PackageMetadata {
    /// 从 deb822 段落构造元数据。
    ///
    /// 这是「字段名 → 结构体」的唯一映射处：dpkg status 与 APT 索引都走这里，
    /// 避免同一格式出现两份规则不一致的解析代码。
    /// `depends` 合并 `Pre-Depends` 与 `Depends`（前者在前），因为界面只有一行
    /// 「依赖」展示位，而两者对用户而言都是「装它还需要什么」。
    pub fn from_stanza(stanza: &Stanza) -> Self {
        let pre_depends = stanza.get_joined("Pre-Depends");
        let depends = stanza.get_joined("Depends");
        let depends = match (pre_depends.is_empty(), depends.is_empty()) {
            (false, false) => format!("{pre_depends}, {depends}"),
            (false, true) => pre_depends,
            _ => depends,
        };
        Self {
            name: stanza.get_first_line("Package"),
            version: stanza.get_first_line("Version"),
            arch: stanza.get_first_line("Architecture"),
            summary: stanza.summary(),
            description: stanza.description(),
            maintainer: stanza.get_first_line("Maintainer"),
            homepage: stanza.get_first_line("Homepage"),
            depends,
            // 索引里的 Installed-Size 单位是 KiB；非数字（如带单位后缀）按 0 处理。
            installed_size_kb: stanza.get_first_line("Installed-Size").parse().unwrap_or(0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpkgSelection {
    Install,
    Hold,
    Deinstall,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpkgStatus {
    NotInstalled,
    ConfigFiles,
    Unpacked,
    HalfConfigured,
    Installed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpkgState {
    pub selection: DpkgSelection,
    pub status: DpkgStatus,
}

impl DpkgState {
    pub fn is_installed(self) -> bool {
        self.status == DpkgStatus::Installed
    }

    pub fn is_held(self) -> bool {
        self.selection == DpkgSelection::Hold
    }

    /// 解析 dpkg status 里 `Status:` 字段的三段式取值。
    ///
    /// 格式固定为 `<selection> <wanted-database-flag> <current-state>`，
    /// 中间一段恒为 `ok`，不参与判断，只取首末两段。
    pub fn parse(value: &str) -> Self {
        let mut parts = value.split_whitespace();
        // 三段中缺任何一段都视为未知，宁可显示为「未知」也不要误判成已安装。
        let selection_token = parts.next();
        let _wanted = parts.next();
        let status_token = parts.next();

        let selection = match selection_token {
            Some("hold") => DpkgSelection::Hold,
            Some("deinstall") => DpkgSelection::Deinstall,
            Some("install") => DpkgSelection::Install,
            _ => DpkgSelection::Unknown,
        };
        let status = match status_token {
            Some("installed") => DpkgStatus::Installed,
            Some("not-installed") => DpkgStatus::NotInstalled,
            Some("config-files") => DpkgStatus::ConfigFiles,
            Some("unpacked") => DpkgStatus::Unpacked,
            Some("half-configured") => DpkgStatus::HalfConfigured,
            _ => DpkgStatus::Unknown,
        };
        Self { selection, status }
    }
}

/// 软件源中的一个候选版本。
#[derive(Clone, Debug, Default)]
pub struct RepoPackage {
    pub metadata: PackageMetadata,
    pub download_size: u64,
    pub filename: String,
    pub repo_base: String,
    pub sha256: String,
}

impl RepoPackage {
    /// 从 APT 索引段落构造；`repo_base` 由调用方按索引文件溯源后填入，
    /// 因为同一个索引文件可能对应多个 URI（镜像）。
    pub fn from_stanza(stanza: &Stanza) -> Self {
        Self {
            metadata: PackageMetadata::from_stanza(stanza),
            download_size: stanza.get_first_line("Size").parse().unwrap_or(0),
            filename: stanza.get_first_line("Filename"),
            repo_base: String::new(),
            sha256: stanza.get_first_line("SHA256"),
        }
    }

    /// 拼出可下载地址；缺少 `Filename`（源里只列了元数据）时返回 `None`。
    pub fn download_url(&self) -> Option<String> {
        if self.filename.is_empty() || self.repo_base.is_empty() {
            return None;
        }
        let filename = self
            .filename
            .trim_start_matches("./")
            .trim_start_matches('/')
            .replace(' ', "%20");
        let base = self.repo_base.trim_end_matches('/');
        Some(format!("{base}/{filename}"))
    }
}

/// 桌面菜单项的当前可见性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuEntryState {
    /// 菜单里正常显示，且没有用户级覆盖文件。
    Visible,
    /// DebDeck 写入的用户级覆盖文件正在隐藏它。
    Hidden,
    /// 已存在用户级覆盖文件，但不是 DebDeck 写的；不自动修改。
    ManagedByOther,
}

#[derive(Clone, Debug)]
pub struct DesktopEntry {
    pub path: PathBuf,
    pub name: String,
    pub comment: String,
    pub exec: String,
    pub icon: String,
    pub terminal: bool,
    pub menu_state: MenuEntryState,
}

/// 一个已安装软件包的轻量视图。
///
/// 原始 `control` 段落与完整文件清单仅在「文件」标签页或离线重打包时按需读取，
/// 避免首屏启动将数千个 `.list` 与控制段落全量常驻内存。
#[derive(Clone, Debug)]
pub struct InstalledPackage {
    pub metadata: PackageMetadata,
    pub dpkg: DpkgState,
    pub desktop: Option<DesktopEntry>,
    /// 安装信息最后修改时间（取 `<包>.list` 的 mtime）。
    pub updated: Option<SystemTime>,
}

#[derive(Clone, Debug, Default)]
pub struct InstalledData {
    pub packages: HashMap<String, InstalledPackage>,
}

/// 软件源里的候选版本集合。
///
/// **不变量**：每个包对应的 `Vec<RepoPackage>` 始终按 Debian 版本号降序、
/// 同版本再按架构降序排列，因此索引 0 就是最新版本。该不变量由唯一的写入口
/// [`Self::record`] 维护，读取方不要再自行排序。
#[derive(Clone, Debug, Default)]
pub struct RepositoryData {
    pub packages: HashMap<String, Vec<RepoPackage>>,
    pub warnings: Vec<String>,
}

impl RepositoryData {
    /// 记录一个候选版本：跳过畸形条目、按（版本, 架构）去重，并维持降序不变量。
    ///
    /// 单个包的版本数通常只有个位数，插入后重排该小数组比在全部解析完再统一排序
    /// 更简单，也让「降序」这件事只有一个出处。
    pub fn record(&mut self, package: RepoPackage) {
        if package.metadata.name.is_empty() || package.metadata.version.is_empty() {
            return;
        }
        let versions = self
            .packages
            .entry(package.metadata.name.clone())
            .or_default();
        if versions.iter().any(|existing| {
            existing.metadata.version == package.metadata.version
                && existing.metadata.arch == package.metadata.arch
        }) {
            return;
        }
        versions.push(package);
        if versions.len() > 1 {
            versions.sort_by(|left, right| {
                debian_version_cmp(&right.metadata.version, &left.metadata.version)
                    .then_with(|| right.metadata.arch.cmp(&left.metadata.arch))
            });
        }
    }

    pub fn versions(&self, pkg: &str) -> &[RepoPackage] {
        self.packages
            .get(pkg)
            .map_or(&[] as &[RepoPackage], Vec::as_slice)
    }

    /// 最新版本（降序不变量的唯一消费者）。
    pub fn latest(&self, pkg: &str) -> Option<&RepoPackage> {
        self.versions(pkg).first()
    }
}

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
    let installed_pkg = installed.packages.get(name);
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
fn has_update(
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
    use super::*;

    fn metadata(name: &str, version: &str) -> PackageMetadata {
        PackageMetadata {
            name: name.to_string(),
            version: version.to_string(),
            arch: "amd64".to_string(),
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
        installed.packages.insert(
            "hello".to_string(),
            InstalledPackage {
                metadata: metadata("hello", "1.0"),
                dpkg: DpkgState {
                    selection: DpkgSelection::Install,
                    status: DpkgStatus::Installed,
                },
                desktop: None,
                updated: None,
            },
        );
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

    #[test]
    fn builds_download_url() {
        let package = RepoPackage {
            filename: "pool/main/h/hello/hello_1.0_amd64.deb".to_string(),
            repo_base: "https://example.com/ubuntu/".to_string(),
            ..Default::default()
        };
        assert_eq!(
            package.download_url().unwrap(),
            "https://example.com/ubuntu/pool/main/h/hello/hello_1.0_amd64.deb"
        );
    }

    #[test]
    fn parses_dpkg_state() {
        assert_eq!(
            DpkgState::parse("hold ok installed"),
            DpkgState {
                selection: DpkgSelection::Hold,
                status: DpkgStatus::Installed,
            }
        );
        assert!(!DpkgState::parse("install ok not-installed").is_installed());
    }

    #[test]
    fn maps_stanza_to_metadata() {
        let stanza = crate::deb822::parse_stanzas(
            "Package: demo\nVersion: 1.2.3\nArchitecture: amd64\nPre-Depends: libc6,\n libgcc-s1\nDepends: zlib1g\nInstalled-Size: 2048\nMaintainer: Demo <demo@example.com>\nHomepage: https://example.com\nDescription: demo summary\n long body\n\n",
        );
        let metadata = PackageMetadata::from_stanza(&stanza[0]);
        assert_eq!(metadata.name, "demo");
        assert_eq!(metadata.version, "1.2.3");
        assert_eq!(metadata.summary, "demo summary");
        assert_eq!(metadata.description, "demo summary\nlong body");
        assert_eq!(metadata.installed_size_kb, 2048);
        assert_eq!(metadata.homepage, "https://example.com");
        // 折行被拼成一行，且 Pre-Depends 排在 Depends 之前。
        assert_eq!(metadata.depends, "libc6, libgcc-s1, zlib1g");
    }

    #[test]
    fn keeps_versions_sorted_by_debian_version() {
        let mut repository = RepositoryData::default();
        for (name, version) in [("demo", "1.0"), ("demo", "2.0-1"), ("demo", "1.10~rc1")] {
            repository.record(RepoPackage {
                metadata: metadata(name, version),
                ..Default::default()
            });
        }
        let versions = repository.versions("demo");
        let listed: Vec<&str> = versions
            .iter()
            .map(|package| package.metadata.version.as_str())
            .collect();
        assert_eq!(listed, ["2.0-1", "1.10~rc1", "1.0"]);
        assert_eq!(repository.latest("demo").unwrap().metadata.version, "2.0-1");

        // 重复的（版本, 架构）不会插入第二次。
        repository.record(RepoPackage {
            metadata: metadata("demo", "2.0-1"),
            ..Default::default()
        });
        assert_eq!(repository.versions("demo").len(), 3);
        assert!(repository.latest("absent").is_none());
    }
}
