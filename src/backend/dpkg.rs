//! 读取 dpkg 数据库：`/var/lib/dpkg/status` 与 `info` 目录。
//!
//! `status` 是一份 deb822 段落文件，每个包一段；`info/<包>[.<架构>].<后缀>` 存放
//! 文件清单（`.list`）、校验和（`.md5sums`）与维护脚本。multi-arch 系统上同一个
//! 包名可能有多架构实例，因此 [`info_file`] 先试不限定架构的名字，再试带架构后缀的。
//!
//! 这里一次性把每包的原始段落缓存进 [`InstalledPackage::control`]，让离线重打包
//! 不必再全量重读 `status`。

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::backend::{BackendError, Result};

use crate::deb822::parse_stanzas;
use crate::model::{DesktopEntry, DpkgState, InstalledData, InstalledPackage, PackageMetadata};

use super::desktop::parse_desktop_file;
use super::fs_layout::FsLayout;

/// 每处理这么多个包上报一次进度，避免事件风暴又让首屏等待有反馈。
const PROGRESS_EVERY: usize = 200;

/// 加载全部已安装软件包。
///
/// 只有 `Status` 为 `installed` 的包会进入结果；`config-files`、`unpacked` 等
/// 中间状态不算已安装，因为它们没有完整的文件清单。
pub fn load_installed(layout: &FsLayout, progress: &mut dyn FnMut(&str)) -> Result<InstalledData> {
    let content = fs::read_to_string(&layout.dpkg_status).map_err(|source| {
        BackendError::io("读取 dpkg 状态库", layout.dpkg_status.display(), source)
    })?;

    let stanzas = parse_stanzas(&content);
    let mut packages: HashMap<String, InstalledPackage> = HashMap::with_capacity(stanzas.len());

    for (index, stanza) in stanzas.iter().enumerate() {
        if index > 0 && index % PROGRESS_EVERY == 0 {
            progress(&format!("正在读取已安装应用…已扫描 {index} 个"));
        }
        let metadata = PackageMetadata::from_stanza(stanza);
        let dpkg = DpkgState::parse(stanza.get("Status").unwrap_or_default());
        if metadata.name.is_empty() || !dpkg.is_installed() {
            continue;
        }
        let name = metadata.name.clone();
        let files = installed_files(layout, &name, &metadata.arch).unwrap_or_default();
        let desktop = find_desktop_entry(&name, &files);
        let updated = info_file(layout, &name, &metadata.arch, "list")
            .and_then(|path| fs::metadata(path).ok())
            .and_then(|meta| meta.modified().ok());
        packages.insert(
            name,
            InstalledPackage {
                metadata,
                dpkg,
                control: stanza.clone(),
                files,
                desktop,
                updated,
            },
        );
    }

    Ok(InstalledData { packages })
}

/// `info` 目录里某个后缀的文件，优先不限定架构的名字。
///
/// 文件名来自 dpkg 状态库，仍按外部数据处理：拒绝任何路径组件，并在命中后
/// 规范化真实路径，确认它没有因符号链接逃出受控的 `dpkg_info_dir`。
pub fn info_file(layout: &FsLayout, pkg_name: &str, arch: &str, suffix: &str) -> Option<PathBuf> {
    if !safe_info_component(pkg_name)
        || !safe_info_component(suffix)
        || (!arch.is_empty() && !safe_info_component(arch))
    {
        return None;
    }

    let base = layout.dpkg_info_dir.canonicalize().ok()?;
    let plain_name = format!("{pkg_name}.{suffix}");
    if let Some(plain) = checked_info_file(&base, &plain_name)
        && plain.is_file()
    {
        return Some(plain);
    }
    if !arch.is_empty() {
        let qualified_name = format!("{pkg_name}:{arch}.{suffix}");
        if let Some(qualified) = checked_info_file(&base, &qualified_name)
            && qualified.is_file()
        {
            return Some(qualified);
        }
    }
    None
}

/// 只接受单个普通路径组件，阻止 `../` 与绝对路径混入 dpkg info 文件名。
fn safe_info_component(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// 规范化候选路径，并确认其真实路径仍位于 dpkg info 目录内。
fn checked_info_file(base: &Path, name: &str) -> Option<PathBuf> {
    let path = base.join(name).canonicalize().ok()?;
    path.starts_with(base).then_some(path)
}

/// 包内文件清单。缺失 `.list`（异常安装）时返回 `None`，调用方按空清单处理。
pub fn installed_files(layout: &FsLayout, pkg_name: &str, arch: &str) -> Option<Vec<String>> {
    let path = info_file(layout, pkg_name, arch, "list")?;
    let content = fs::read_to_string(path).ok()?;
    Some(
        content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect(),
    )
}

/// 判断某个文件路径是否可作为该包的应用启动入口。
///
/// 只接受三个已知位置，且排除 `autostart/`（开机自启项不是「应用」）：
/// `/usr/share/applications`、`/usr/local/share/applications`，以及 deepin/spark
/// 风格的 `/opt/apps/<包名>/entries/applications`。
fn is_application_desktop(path: &str, pkg_name: &str) -> bool {
    if !path.ends_with(".desktop") || path.contains("/autostart/") {
        return false;
    }
    let opt_prefix = format!("/opt/apps/{pkg_name}/entries/applications/");
    path.starts_with("/usr/share/applications/")
        || path.starts_with("/usr/local/share/applications/")
        || path.starts_with(&opt_prefix)
}

/// 从文件清单里挑一个桌面条目作为启动入口。
///
/// 多个候选时按路径排序取第一个可解析的，保证同一台机器上结果稳定。
fn find_desktop_entry(pkg_name: &str, files: &[String]) -> Option<DesktopEntry> {
    let mut candidates: Vec<&String> = files
        .iter()
        .filter(|path| is_application_desktop(path, pkg_name))
        .collect();
    candidates.sort();
    candidates.iter().find_map(|path| {
        let mut entry = parse_desktop_file(Path::new(path))?;
        entry.menu_state = crate::backend::desktop::menu::menu_state(&entry.path);
        Some(entry)
    })
}

/// `status` 文件的修改时间，供 dpkg 变化监视轮询使用。
pub fn dpkg_status_mtime(layout: &FsLayout) -> Option<SystemTime> {
    fs::metadata(&layout.dpkg_status).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_application_desktop_paths() {
        assert!(is_application_desktop(
            "/usr/share/applications/firefox.desktop",
            "firefox"
        ));
        assert!(is_application_desktop(
            "/opt/apps/com.example.app/entries/applications/app.desktop",
            "com.example.app"
        ));
        // 别人的 /opt/apps 目录不属于这个包。
        assert!(!is_application_desktop(
            "/opt/apps/com.other/entries/applications/app.desktop",
            "com.example.app"
        ));
        // 开机自启项不是「应用」。
        assert!(!is_application_desktop(
            "/usr/share/applications/autostart/foo.desktop",
            "foo"
        ));
        assert!(!is_application_desktop(
            "/usr/share/applications/not-a-desktop.png",
            "x"
        ));
        assert!(!is_application_desktop(
            "/usr/share/man/man1/foo.1.gz",
            "foo"
        ));
    }

    #[test]
    fn rejects_info_path_traversal() {
        let root = std::env::temp_dir().join(format!("debdeck-dpkg-path-{}", std::process::id()));
        let layout = FsLayout::with_root(&root);
        fs::create_dir_all(&layout.dpkg_info_dir).unwrap();
        assert!(info_file(&layout, "../status", "amd64", "list").is_none());
        assert!(info_file(&layout, "demo", "../../etc", "list").is_none());
    }

    #[test]
    fn loads_packages_from_fixture_layout() {
        let root =
            std::env::temp_dir().join(format!("debdeck-dpkg-fixture-{}", std::process::id()));
        let layout = FsLayout::with_root(&root);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(layout.dpkg_status.parent().unwrap()).unwrap();
        fs::create_dir_all(&layout.dpkg_info_dir).unwrap();

        fs::write(
            &layout.dpkg_status,
            "Package: demo\nStatus: install ok installed\nArchitecture: amd64\nVersion: 1.2.3\nInstalled-Size: 2048\nDepends: libc6 (>= 2.39),\n libfoo\nMaintainer: Demo <demo@example.com>\nDescription: demo package\n long description\n\nPackage: ghost\nStatus: install ok not-installed\nArchitecture: amd64\nVersion: 0.1\n\nPackage: held\nStatus: hold ok installed\nArchitecture: all\nVersion: 2.0\nDescription: held package\n\n",
        )
        .unwrap();
        fs::write(
            layout.dpkg_info_dir.join("demo.list"),
            "/usr/bin/demo\n/usr/share/demo/data\n",
        )
        .unwrap();

        let data = load_installed(&layout, &mut |_| {}).unwrap();

        assert_eq!(data.packages.len(), 2, "未安装的包不应出现");
        let demo = &data.packages["demo"];
        assert_eq!(demo.metadata.version, "1.2.3");
        assert_eq!(demo.metadata.installed_size_kb, 2048);
        assert_eq!(demo.metadata.depends, "libc6 (>= 2.39), libfoo");
        assert_eq!(demo.files, ["/usr/bin/demo", "/usr/share/demo/data"]);
        assert!(demo.dpkg.is_installed());
        assert!(data.packages["held"].dpkg.is_held());
        // 原始段落被缓存，重打包可直接复用。
        assert_eq!(demo.control.get_first_line("Package"), "demo");

        let _ = fs::remove_dir_all(&root);
    }
}
