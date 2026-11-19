//! APT 侧数据的读取入口：源配置 + 软件包索引 + 可升级集合。
//!
//! [`load_repository`] 是本层唯一编排者：它把「索引文件名」与「源 URI」这两个
//! 独立事实对齐，任何对不齐都要留下用户能看到的警告，而不是静默丢数据。

use std::num::NonZero;
use std::path::PathBuf;

use crate::backend::fs_layout::FsLayout;
use crate::model::RepositoryData;

pub mod index;
pub mod sources;
pub mod upgradable;

pub use upgradable::list_upgradable;

/// 解析全部软件源索引。
///
/// 不返回 `Result`：单个源或索引文件损坏不应该让整块数据消失，问题以
/// [`RepositoryData::warnings`] 的形式呈现给用户。
pub fn load_repository(layout: &FsLayout, native_arch: &str) -> RepositoryData {
    let (sources, mut warnings) = sources::read_sources(layout, native_arch);
    let mut repository = RepositoryData::default();
    let (files, unsupported_suffixes) = index::list_index_files(layout, native_arch);
    append_discovery_warnings(&files, &unsupported_suffixes, layout, &mut warnings);
    let scan = scan_indexes(&files, &sources, native_arch, &mut repository);
    append_scan_warnings(&scan, &mut warnings);
    if repository.packages.is_empty() && warnings.is_empty() {
        warnings.push("软件源中没有可用的软件包信息".to_string());
    }
    repository.warnings = warnings;
    repository
}

fn append_discovery_warnings(
    files: &[PathBuf],
    unsupported_suffixes: &[String],
    layout: &FsLayout,
    warnings: &mut Vec<String>,
) {
    if files.is_empty() {
        warnings.push(format!(
            "未找到 APT 软件包索引（{}），请先执行 sudo apt update",
            layout.apt_lists_dir.display()
        ));
    }
    if !unsupported_suffixes.is_empty() {
        warnings.push(format!(
            "已忽略无法解压的索引文件，后缀：{}",
            unsupported_suffixes.join("、")
        ));
    }
}

#[derive(Default)]
struct IndexScan {
    guessed: usize,
    unresolved: usize,
    read_failures: usize,
}

fn scan_indexes(
    files: &[PathBuf],
    sources: &[sources::AptSource],
    native_arch: &str,
    repository: &mut RepositoryData,
) -> IndexScan {
    let mut scan = IndexScan::default();
    let mut jobs: Vec<(&PathBuf, String)> = Vec::with_capacity(files.len());
    for file in files {
        let Some(file_name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(base) = resolve_base(file_name, sources, native_arch, &mut scan) {
            jobs.push((file, base));
        }
    }

    if jobs.len() <= 1 {
        for (file, base) in jobs {
            if let Err(error) = index::parse_index_file_into(repository, file, &base, native_arch) {
                tracing::warn!("解析索引 {} 失败：{error}", file.display());
                scan.read_failures = scan.read_failures.saturating_add(1);
            }
        }
        return scan;
    }

    scan_indexes_parallel(&jobs, native_arch, repository, &mut scan);
    scan
}

fn scan_indexes_parallel(
    jobs: &[(&PathBuf, String)],
    native_arch: &str,
    repository: &mut RepositoryData,
    scan: &mut IndexScan,
) {
    let workers = std::thread::available_parallelism()
        .map_or(2, NonZero::get)
        .clamp(1, 4)
        .min(jobs.len());
    let chunk_size = jobs.len().div_ceil(workers);
    let partials: Vec<(RepositoryData, usize)> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk_size)
            .map(|chunk| scope.spawn(move || parse_index_chunk(chunk, native_arch)))
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });

    for (local, failures) in partials {
        scan.read_failures = scan.read_failures.saturating_add(failures);
        repository.merge(local);
    }
}

fn parse_index_chunk(chunk: &[(&PathBuf, String)], native_arch: &str) -> (RepositoryData, usize) {
    let mut local = RepositoryData::default();
    let mut failures = 0usize;
    for (file, base) in chunk {
        if let Err(error) = index::parse_index_file_into(&mut local, file, base, native_arch) {
            tracing::warn!("解析索引 {} 失败：{error}", file.display());
            failures = failures.saturating_add(1);
        }
    }
    (local, failures)
}

fn resolve_base(
    file_name: &str,
    sources: &[sources::AptSource],
    native_arch: &str,
    scan: &mut IndexScan,
) -> Option<String> {
    match index::resolve_repo_base(file_name, sources, native_arch) {
        index::BaseResolution::Matched(base) | index::BaseResolution::Prefix(base) => Some(base),
        index::BaseResolution::Guessed(base) => {
            scan.guessed = scan.guessed.saturating_add(1);
            Some(base)
        }
        index::BaseResolution::Unresolved => {
            scan.unresolved = scan.unresolved.saturating_add(1);
            None
        }
    }
}

fn append_scan_warnings(scan: &IndexScan, warnings: &mut Vec<String>) {
    if scan.guessed > 0 {
        warnings.push(format!(
            "{} 个索引文件匹配不到已配置的源，已按文件名推断下载地址，可能不准确",
            scan.guessed
        ));
    }
    if scan.unresolved > 0 {
        warnings.push(format!(
            "{} 个索引文件无法确定软件源地址，其中的软件包不支持在线获取",
            scan.unresolved
        ));
    }
    if scan.read_failures > 0 {
        warnings.push(format!("{} 个索引文件读取失败，已跳过", scan.read_failures));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::model::RepoPackage;

    use super::*;
    use std::path::{Path, PathBuf};

    /// 造一个最小可用的 fixture 根：`etc/apt/sources.list` + `var/lib/apt/lists/<索引>`。
    fn fixture(root: &Path, sources_list: &str, index_name: &str, index_body: &str) {
        let apt_dir = root.join("etc/apt");
        let lists_dir = root.join("var/lib/apt/lists");
        fs::create_dir_all(&apt_dir).unwrap();
        fs::create_dir_all(lists_dir.join("partial")).unwrap();
        fs::write(apt_dir.join("sources.list"), sources_list).unwrap();
        fs::create_dir_all(root.join("var/lib/dpkg/info")).unwrap();
        fs::write(lists_dir.join(index_name), index_body).unwrap();
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("debdeck-apt-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn loads_repository_from_fixture() {
        let root = temp_root("load");
        fixture(
            &root,
            "deb https://example.com/ubuntu noble main\n",
            "example.com_ubuntu_dists_noble_main_binary-amd64_Packages",
            "Package: demo\nVersion: 2.0\nArchitecture: amd64\nSize: 100\nFilename: pool/main/d/demo/demo_2.0_amd64.deb\nSHA256: deadbeef\nDescription: demo\n\nPackage: old\nVersion: 1.0\nArchitecture: amd64\nFilename: pool/main/o/old/old_1.0_amd64.deb\nDescription: old\n\nPackage: other\nVersion: 1.0\nArchitecture: arm64\nFilename: no\nDescription: other\n\n",
        );

        let layout = FsLayout::with_root(&root);
        let repository = load_repository(&layout, "amd64");
        assert!(repository.warnings.is_empty(), "{:?}", repository.warnings);
        assert_eq!(repository.versions("demo").len(), 1);
        assert_eq!(
            repository
                .latest("demo")
                .map(|p| p.metadata.version.as_str()),
            Some("2.0")
        );
        assert_eq!(
            repository
                .latest("demo")
                .and_then(RepoPackage::download_url)
                .as_deref(),
            Some("https://example.com/ubuntu/pool/main/d/demo/demo_2.0_amd64.deb")
        );
        assert_eq!(repository.latest("demo").unwrap().sha256, "deadbeef");
        assert!(
            !repository.packages.contains_key("other"),
            "非本机架构不应进入列表"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn orders_versions_newest_first_across_indexes() {
        let root = temp_root("order");
        fixture(
            &root,
            "deb https://example.com/ubuntu noble main\ndeb https://example.com/ubuntu noble-updates main\n",
            "example.com_ubuntu_dists_noble_main_binary-amd64_Packages",
            "Package: demo\nVersion: 1.0\nArchitecture: amd64\nFilename: pool/1.deb\nDescription: a\n\n",
        );
        fs::write(
            root.join("var/lib/apt/lists/example.com_ubuntu_dists_noble-updates_main_binary-amd64_Packages"),
            "Package: demo\nVersion: 3.0\nArchitecture: amd64\nFilename: pool/3.deb\nDescription: b\n\n",
        )
        .unwrap();

        let repository = load_repository(&FsLayout::with_root(&root), "amd64");
        let versions: Vec<&str> = repository
            .versions("demo")
            .iter()
            .map(|package| package.metadata.version.as_str())
            .collect();
        assert_eq!(versions, ["3.0", "1.0"]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn warns_when_index_cannot_be_matched_to_a_source() {
        let root = temp_root("warn");
        fixture(
            &root,
            "deb https://example.com/ubuntu noble main\n",
            "stranger.host_repo_dists_stable_main_binary-amd64_Packages",
            "Package: ghost\nVersion: 1\nArchitecture: amd64\nFilename: pool/g.deb\nDescription: g\n\n",
        );
        fs::write(
            root.join(
                "var/lib/apt/lists/example.com_ubuntu_dists_noble_main_binary-amd64_Packages.zst",
            ),
            "compressed",
        )
        .unwrap();

        let repository = load_repository(&FsLayout::with_root(&root), "amd64");
        let joined = repository.warnings.join("\n");
        assert!(joined.contains("推断下载地址"), "{joined}");
        assert!(joined.contains("无法解压"), "{joined}");
        // 猜出来的地址仍然可用，包不会被丢弃。
        assert!(repository.packages.contains_key("ghost"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_missing_index_directory() {
        let root = temp_root("empty");
        let repository = load_repository(&FsLayout::with_root(&root), "amd64");
        assert!(
            repository
                .warnings
                .iter()
                .any(|warning| warning.contains("sudo apt update")),
            "{:?}",
            repository.warnings
        );
        let _ = fs::remove_dir_all(&root);
    }
}
