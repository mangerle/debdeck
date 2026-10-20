//! APT 侧数据的读取入口：源配置 + 软件包索引 + 可升级集合。
//!
//! [`load_repository`] 是本层唯一编排者：它把「索引文件名」与「源 URI」这两个
//! 独立事实对齐，任何对不齐都要留下用户能看到的警告，而不是静默丢数据。

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

    let mut guessed = 0_usize;
    let mut unresolved = 0_usize;
    let mut read_failures = 0_usize;

    for file in files {
        let Some(file_name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let base = match index::resolve_repo_base(file_name, &sources, native_arch) {
            index::BaseResolution::Matched(base) | index::BaseResolution::Prefix(base) => base,
            index::BaseResolution::Guessed(base) => {
                guessed += 1;
                base
            }
            index::BaseResolution::Unresolved => {
                unresolved += 1;
                continue;
            }
        };

        match read_index_content(&file) {
            Some(content) => index::parse_index_into(&mut repository, &content, &base, native_arch),
            None => read_failures += 1,
        }
    }

    if guessed > 0 {
        warnings.push(format!(
            "{guessed} 个索引文件匹配不到已配置的源，已按文件名推断下载地址，可能不准确"
        ));
    }
    if unresolved > 0 {
        warnings.push(format!(
            "{unresolved} 个索引文件无法确定软件源地址，其中的软件包不支持在线获取"
        ));
    }
    if read_failures > 0 {
        warnings.push(format!("{read_failures} 个索引文件读取失败，已跳过"));
    }
    if repository.packages.is_empty() && warnings.is_empty() {
        warnings.push("软件源中没有可用的软件包信息".to_string());
    }

    repository.warnings = warnings;
    repository
}

/// 读取索引内容，失败时返回 `None`（由调用方聚合计数并给出警告）。
fn read_index_content(path: &std::path::Path) -> Option<String> {
    index::read_index_file(path)
        .inspect_err(|error| {
            tracing::warn!("读取索引 {} 失败：{error}", path.display());
        })
        .ok()
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
