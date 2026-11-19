//! 读取 dpkg 数据库：`/var/lib/dpkg/status` 与 `info` 目录。
//!
//! `status` 是一份 deb822 段落文件，每个包一段；`info/<包>[.<架构>].<后缀>` 存放
//! 文件清单（`.list`）、校验和（`.md5sums`）与维护脚本。multi-arch 系统上同一个
//! 包名可能有多架构实例，因此 [`info_file`] 先试不限定架构的名字，再试带架构后缀的。
//!
//! 这里一次性把每包的原始段落缓存进 [`InstalledPackage::control`]，让离线重打包
//! 不必再全量重读 `status`。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::backend::{BackendError, Result};

use crate::deb822::{Stanza, for_each_filtered_stanza, for_each_stanza};
use crate::model::{DesktopEntry, DpkgState, InstalledData, InstalledPackage, PackageMetadata};

use super::desktop::entry::parse_desktop_file_with_cache;
use super::fs_layout::FsLayout;

/// 每处理这么多个包上报一次进度，避免事件风暴又让首屏等待有反馈。
const PROGRESS_EVERY: usize = 200;

/// 读取 `dpkg/status` 时实际消费的字段白名单；其余大字段（如 `Conffiles`）流式跳过。
const STATUS_FIELDS: &[&str] = &[
    "Package",
    "Status",
    "Version",
    "Architecture",
    "Description",
    "Maintainer",
    "Homepage",
    "Pre-Depends",
    "Depends",
    "Installed-Size",
];

/// 加载全部已安装软件包。
///
/// 只有 `Status` 为 `installed` 的包会进入结果；`config-files`、`unpacked` 等
/// 中间状态不算已安装，因为它们没有完整的文件清单。
pub fn load_installed(layout: &FsLayout, progress: &mut dyn FnMut(&str)) -> Result<InstalledData> {
    let file = File::open(&layout.dpkg_status).map_err(|source| {
        BackendError::io("读取 dpkg 状态库", layout.dpkg_status.display(), source)
    })?;
    let info_dir = layout.dpkg_info_dir.canonicalize().ok();
    let list_files = scan_list_files(info_dir.as_deref());
    let mut try_exec_cache: HashMap<String, bool> = HashMap::with_capacity(128);
    let mut data = InstalledData::default();
    let mut scanned = 0usize;

    for_each_filtered_stanza(BufReader::new(file), STATUS_FIELDS, |stanza| {
        scanned += 1;
        if scanned.is_multiple_of(PROGRESS_EVERY) {
            progress(&format!("正在读取已安装应用…已扫描 {scanned} 个"));
        }
        let metadata = PackageMetadata::from_stanza(&stanza);
        let dpkg = DpkgState::parse(stanza.get("Status").unwrap_or_default());
        if metadata.name.is_empty() || !dpkg.is_installed() {
            return;
        }
        let name = metadata.name.clone();
        let list_file = locate_list_file(&list_files, &name, &metadata.arch);
        let updated = list_file.and_then(|file| file.modified);
        let desktop = list_file
            .and_then(|file| find_desktop_entry_from_list(&name, &file.path, &mut try_exec_cache));
        data.record(InstalledPackage {
            metadata,
            dpkg,
            desktop,
            updated,
        });
    })
    .map_err(|source| BackendError::io("解析 dpkg 状态库", layout.dpkg_status.display(), source))?;

    Ok(data)
}

/// 扫描 `dpkg/info` 中可用的 `.list` 文件。
///
/// 目录本身已经规范化，条目名来自目录枚举，因此每个候选只需一次
/// `symlink_metadata`：既能拒绝符号链接和非普通文件，又能顺带取得 mtime，
/// 不必再在包循环中重复解析路径和读取元数据。
fn scan_list_files(info_dir: Option<&Path>) -> HashMap<String, ListFile> {
    let Some(dir) = info_dir else {
        return HashMap::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return HashMap::new();
    };
    let mut files = HashMap::with_capacity(2048);
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name.strip_suffix(".list").is_none() {
            continue;
        }
        let owned_name = name.to_string();
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        files.insert(
            owned_name,
            ListFile {
                path,
                modified: metadata.modified().ok(),
            },
        );
    }
    files
}

/// 已确认安全的 `.list` 文件及其扫描时取得的修改时间。
struct ListFile {
    path: PathBuf,
    modified: Option<SystemTime>,
}

fn locate_list_file<'a>(
    list_files: &'a HashMap<String, ListFile>,
    pkg_name: &str,
    arch: &str,
) -> Option<&'a ListFile> {
    if !safe_info_component(pkg_name) || (!arch.is_empty() && !safe_info_component(arch)) {
        return None;
    }
    let plain_name = format!("{pkg_name}.list");
    if let Some(plain) = list_files.get(&plain_name) {
        return Some(plain);
    }
    if !arch.is_empty() {
        let qualified_name = format!("{pkg_name}:{arch}.list");
        if let Some(qualified) = list_files.get(&qualified_name) {
            return Some(qualified);
        }
    }
    None
}

/// `info` 目录里某个后缀的文件，优先不限定架构的名字。
///
/// 文件名来自 dpkg 状态库，仍按外部数据处理：拒绝任何路径组件，并在命中后
/// 规范化真实路径，确认它没有因符号链接逃出受控的 `dpkg_info_dir`。
pub fn info_file(layout: &FsLayout, pkg_name: &str, arch: &str, suffix: &str) -> Option<PathBuf> {
    let base = layout.dpkg_info_dir.canonicalize().ok()?;
    locate_info_file_in(Some(&base), pkg_name, arch, suffix)
}

fn locate_info_file_in(
    base: Option<&Path>,
    pkg_name: &str,
    arch: &str,
    suffix: &str,
) -> Option<PathBuf> {
    let base = base?;
    if !safe_info_component(pkg_name)
        || !safe_info_component(suffix)
        || (!arch.is_empty() && !safe_info_component(arch))
    {
        return None;
    }

    let plain_name = format!("{pkg_name}.{suffix}");
    if let Some(plain) = checked_info_file(base, &plain_name)
        && plain.is_file()
    {
        return Some(plain);
    }
    if !arch.is_empty() {
        let qualified_name = format!("{pkg_name}:{arch}.{suffix}");
        if let Some(qualified) = checked_info_file(base, &qualified_name)
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

/// 按需从 `/var/lib/dpkg/status` 中流式查找单个已安装包的原始控制段落。
///
/// # Errors
/// 读取状态库失败时返回 I/O 错误。
pub fn load_package_control(
    layout: &FsLayout,
    pkg_name: &str,
    arch: &str,
) -> Result<Option<Stanza>> {
    let file = File::open(&layout.dpkg_status).map_err(|source| {
        BackendError::io("读取 dpkg 状态库", layout.dpkg_status.display(), source)
    })?;
    let mut matched = None;
    for_each_stanza(BufReader::new(file), |stanza| {
        if matched.is_some() {
            return;
        }
        let name = stanza.get_first_line("Package");
        if name != pkg_name {
            return;
        }
        let stanza_arch = stanza.get_first_line("Architecture");
        if arch.is_empty() || stanza_arch == arch {
            matched = Some(stanza);
        }
    })
    .map_err(|source| BackendError::io("解析 dpkg 状态库", layout.dpkg_status.display(), source))?;
    Ok(matched)
}

/// 从 `<包>.list` 流式扫描 `.desktop` 路径并挑选桌面入口，避免把整份文件清单读入内存。
fn find_desktop_entry_from_list(
    pkg_name: &str,
    list_path: &Path,
    try_exec_cache: &mut HashMap<String, bool>,
) -> Option<DesktopEntry> {
    let file = File::open(list_path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut candidates: Vec<String> = Vec::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim();
        if is_application_desktop(trimmed, pkg_name) {
            candidates.push(trimmed.to_string());
        }
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort();
    candidates.iter().find_map(|path| {
        let mut entry = parse_desktop_file_with_cache(Path::new(path), Some(try_exec_cache))?;
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
        let demo = data.primary("demo").expect("demo 应存在");
        assert_eq!(demo.metadata.version, "1.2.3");
        assert_eq!(demo.metadata.installed_size_kb, 2048);
        assert_eq!(demo.metadata.depends, "libc6 (>= 2.39), libfoo");
        assert!(demo.dpkg.is_installed());
        assert!(demo.updated.is_some(), "扫描 .list 时应同时取得 mtime");
        assert!(data.primary("held").unwrap().dpkg.is_held());
        // 文件清单与原始控制段落改为按需读取。
        assert_eq!(
            installed_files(&layout, "demo", "amd64").unwrap(),
            ["/usr/bin/demo", "/usr/share/demo/data"]
        );
        let control = load_package_control(&layout, "demo", "amd64")
            .unwrap()
            .expect("应能按需读回原始控制段落");
        assert_eq!(control.get_first_line("Package"), "demo");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn preserves_multi_arch_installed_packages() {
        let root =
            std::env::temp_dir().join(format!("debdeck-dpkg-multiarch-{}", std::process::id()));
        let layout = FsLayout::with_root(&root);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(layout.dpkg_status.parent().unwrap()).unwrap();
        fs::create_dir_all(&layout.dpkg_info_dir).unwrap();
        fs::write(
            &layout.dpkg_status,
            "Package: libc6\nStatus: install ok installed\nArchitecture: amd64\nVersion: 2.39\n\nPackage: libc6\nStatus: install ok installed\nArchitecture: i386\nVersion: 2.39\n\n",
        )
        .unwrap();

        let data = load_installed(&layout, &mut |_| {}).unwrap();
        let instances = data.instances("libc6");
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].metadata.arch.as_ref(), "amd64");
        assert_eq!(instances[1].metadata.arch.as_ref(), "i386");

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinked_list_files() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("debdeck-dpkg-symlink-{}", std::process::id()));
        let layout = FsLayout::with_root(&root);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&layout.dpkg_info_dir).unwrap();

        let external = root.join("outside.list");
        fs::write(&external, "/usr/bin/demo\n").unwrap();
        symlink(&external, layout.dpkg_info_dir.join("demo.list")).unwrap();

        let info_dir = layout.dpkg_info_dir.canonicalize().unwrap();
        let files = scan_list_files(Some(&info_dir));
        assert!(
            !files.contains_key("demo.list"),
            "符号链接不能作为 dpkg 清单"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
