//! 软件包基础领域实体：本地 `.deb`、dpkg 状态、软件源候选包与桌面条目。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::deb822::Stanza;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
