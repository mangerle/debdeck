//! `/var/lib/apt/lists` 里的软件包索引：文件发现、解压与解析。
//!
//! APT 把每个源的 `Packages` 索引存成 `lists` 目录下的一个文件，文件名由
//! 「源 URI + dists + suite + component + binary-架构」拼成。解析一个索引需要
//! 三步：找到文件、按后缀解压、把段落映射成 [`RepoPackage`] 并记录来源 URI。
//!
//! 索引格式与 dpkg status 同为 deb822，因此这里复用 `deb822::parse_stanzas`，
//! 不再维护第二份手写解析循环。

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use lz4_flex::frame::FrameDecoder;

use crate::backend::fs_layout::FsLayout;
use crate::deb822::parse_stanzas;
use crate::model::{RepoPackage, RepositoryData};

use super::sources::AptSource;

/// 索引文件的压缩方式。后缀识别只有一处定义，避免「文件名规则认得、解压不认识」
/// 这类不一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compression {
    None,
    Gz,
    Lz4,
}

/// 拆出 `(去掉压缩后缀的名字, 压缩方式)`。
fn split_compression(name: &str) -> (&str, Compression) {
    for (suffix, compression) in [(".gz", Compression::Gz), (".lz4", Compression::Lz4)] {
        if let Some(stem) = name.strip_suffix(suffix) {
            return (stem, compression);
        }
    }
    (name, Compression::None)
}

/// 是否是我们能处理的索引文件。
fn is_index_file(name: &str) -> bool {
    let (stem, _) = split_compression(name);
    stem.ends_with("_Packages")
}

/// 遇到但我们无法解压的索引后缀（如 `.zst`），用于聚合成一条警告。
fn unsupported_index_suffix(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".zst").or(name.strip_suffix(".xz"))?;
    stem.ends_with("_Packages").then(|| {
        name.rsplit_once("_Packages")
            .map(|(_, suffix)| suffix.to_string())
            .unwrap_or_default()
    })
}

/// 索引文件的下载地址来源判定结果。
#[derive(Debug, PartialEq, Eq)]
pub enum BaseResolution {
    /// 文件名与某个源精确对应。
    Matched(String),
    /// 按 URI 前缀匹配（APT 有时会在文件名里追加标记）。
    Prefix(String),
    /// 没有任何源能匹配，只能从文件名反推。调用方应提示用户结果可能不准。
    Guessed(String),
    /// 完全无法确定，跳过该文件。
    Unresolved,
}

/// 在 `lists` 目录里挑选要解析的索引文件。
///
/// 优先只要与本机架构匹配的文件；一个都没有时退回全部文件（例如 multi-arch
/// 环境或架构后缀命名不同的第三方源），由解析阶段按架构过滤条目。
pub fn list_index_files(layout: &FsLayout, native_arch: &str) -> (Vec<PathBuf>, Vec<String>) {
    let Ok(dir) = fs::read_dir(&layout.apt_lists_dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut skipped: Vec<String> = Vec::new();
    let mut all: Vec<PathBuf> = Vec::new();
    for entry in dir.filter_map(std::io::Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_index_file(name) {
            all.push(path);
        } else if let Some(suffix) = unsupported_index_suffix(name) {
            skipped.push(suffix);
        }
    }

    let arch_suffix = format!("_binary-{native_arch}_Packages");
    let mut matching: Vec<PathBuf> = all
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(&arch_suffix))
        })
        .cloned()
        .collect();
    if matching.is_empty() {
        matching = all;
    }
    matching.sort();
    skipped.sort();
    skipped.dedup();
    (matching, skipped)
}

/// 读取索引内容（按需解压）。
pub(super) fn read_index_file(path: &Path) -> std::io::Result<String> {
    let file = File::open(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let (_, compression) = split_compression(name);
    let mut buffer = Vec::new();
    match compression {
        Compression::None => BufReader::new(file).read_to_end(&mut buffer)?,
        Compression::Gz => GzDecoder::new(file).read_to_end(&mut buffer)?,
        Compression::Lz4 => FrameDecoder::new(file).read_to_end(&mut buffer)?,
    };
    // 索引里可能混入非 UTF-8 字节（损坏或部分写入），用 lossy 解码保证不致命。
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// 把索引文件名映射回源 URI。
pub fn resolve_repo_base(
    file_name: &str,
    sources: &[AptSource],
    native_arch: &str,
) -> BaseResolution {
    let (stem, _) = split_compression(file_name);
    let arch_suffix = format!("_binary-{native_arch}_Packages");
    let stem = stem.strip_suffix(arch_suffix.as_str()).unwrap_or(stem);

    // 1) 精确重建 APT 的命名并比较。
    for source in sources {
        let Some(key) = source.uri_key() else {
            continue;
        };
        let suite = source.suite.replace('/', "_");
        let component = source.component.replace('/', "_");
        let expected = format!("{key}_dists_{suite}_{component}");
        if stem == expected {
            return BaseResolution::Matched(source.uri.clone());
        }
    }

    // 2) 取最长匹配的 URI 前缀，避免多镜像时误配。
    let mut best: Option<(String, usize)> = None;
    for source in sources {
        let Some(key) = source.uri_key() else {
            continue;
        };
        if stem.starts_with(&key)
            && best
                .as_ref()
                .is_none_or(|(_, best_len)| key.len() > *best_len)
        {
            best = Some((source.uri.clone(), key.len()));
        }
    }
    if let Some((uri, _)) = best {
        return BaseResolution::Prefix(uri);
    }

    // 3) 兜底：把 `mirrors.example_ubuntu_dists_…` 里的第一段当成主机路径。
    //    第三方源与手工放进的索引会走到这里，结果可能不准，因此调用方会警告。
    let Some((prefix, _)) = stem.split_once("_dists_") else {
        return BaseResolution::Unresolved;
    };
    if prefix.is_empty() {
        return BaseResolution::Unresolved;
    }
    let host_path = prefix.replace('_', "/");
    // 没有点号的名字（如 `localhost`）不是合法主机名，拼出来只会得到一个
    // 看起来能用、实际下错的 URL，宁可不给下载地址。
    if !host_path.contains('.') {
        return BaseResolution::Unresolved;
    }
    BaseResolution::Guessed(format!("https://{host_path}/"))
}

/// 把一个索引内容解析进 `repository`。
///
/// 架构过滤在此完成：只保留本机架构与 `all`。同一（版本, 架构）的重复条目由
/// [`RepositoryData::record`] 去重。
pub fn parse_index_into(
    repository: &mut RepositoryData,
    content: &str,
    base: &str,
    native_arch: &str,
) {
    for stanza in parse_stanzas(content) {
        let mut package = RepoPackage::from_stanza(&stanza);
        if package.metadata.name.is_empty() || package.metadata.version.is_empty() {
            continue;
        }
        if package.metadata.arch != native_arch && package.metadata.arch != "all" {
            continue;
        }
        package.repo_base = base.to_string();
        repository.record(package);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn source(uri: &str, suite: &str, component: &str) -> AptSource {
        AptSource {
            uri: uri.to_string(),
            suite: suite.to_string(),
            component: component.to_string(),
        }
    }

    #[test]
    fn recognises_index_files_and_suffixes() {
        assert!(is_index_file("repo_dists_suite_main_binary-amd64_Packages"));
        assert!(is_index_file(
            "repo_dists_suite_main_binary-amd64_Packages.gz"
        ));
        assert!(is_index_file(
            "repo_dists_suite_main_binary-amd64_Packages.lz4"
        ));
        assert!(!is_index_file(
            "repo_dists_suite_main_binary-amd64_Packages.zst"
        ));
        assert!(!is_index_file("lock"));
        assert_eq!(split_compression("x_Packages.gz").0, "x_Packages");
        assert_eq!(
            unsupported_index_suffix("x_Packages.zst").as_deref(),
            Some(".zst")
        );
    }

    #[test]
    fn resolves_ubuntu_repository_base() {
        let sources = [source(
            "https://mirrors.aliyun.com/ubuntu/",
            "resolute-updates",
            "main",
        )];
        let file = "mirrors.aliyun.com_ubuntu_dists_resolute-updates_main_binary-amd64_Packages";
        assert_eq!(
            resolve_repo_base(file, &sources, "amd64"),
            BaseResolution::Matched("https://mirrors.aliyun.com/ubuntu/".to_string())
        );
    }

    #[test]
    fn prefers_longest_uri_prefix_when_exact_name_is_unknown() {
        // 源里声明的是 universe，索引却是 main：精确匹配失败，应退回最长前缀。
        let sources = [
            source("https://mirror.example/", "stable", "universe"),
            source("https://mirror.example/custom/", "stable", "universe"),
        ];
        let file = "mirror.example_custom_dists_stable_main_binary-amd64_Packages";
        assert_eq!(
            resolve_repo_base(file, &sources, "amd64"),
            BaseResolution::Prefix("https://mirror.example/custom/".to_string())
        );
    }

    #[test]
    fn guesses_or_refuses_unknown_source_names() {
        let file = "download.example_repo_dists_stable_main_binary-amd64_Packages";
        assert_eq!(
            resolve_repo_base(file, &[], "amd64"),
            BaseResolution::Guessed("https://download.example/repo/".to_string())
        );
        // 认不出的名字不再拼成错误的 https 地址。
        assert_eq!(
            resolve_repo_base("lock", &[], "amd64"),
            BaseResolution::Unresolved
        );
        assert_eq!(
            resolve_repo_base(
                "localhost_dists_stable_main_binary-amd64_Packages",
                &[],
                "amd64"
            ),
            BaseResolution::Unresolved
        );
    }

    #[test]
    fn filters_architecture_and_duplicates() {
        let content = "Package: demo\nVersion: 1:2.0-1\nArchitecture: amd64\nInstalled-Size: 1234\nSize: 5678\nSHA256: abc\nMaintainer: Demo\nHomepage: https://example.com\nDepends: libc6 (>= 2.39),\n libfoo\nFilename: ./pool/main/d/demo.deb\nDescription: demo summary\n long text\n\nPackage: foreign\nVersion: 1\nArchitecture: i386\nDescription: skip me\n\nPackage: data-pkg\nVersion: 1\nArchitecture: all\nDescription: data\n\n";
        let mut repository = RepositoryData {
            packages: HashMap::new(),
            warnings: Vec::new(),
        };
        parse_index_into(
            &mut repository,
            content,
            "https://example.com/ubuntu/",
            "amd64",
        );

        assert!(!repository.packages.contains_key("foreign"));
        assert!(repository.packages.contains_key("data-pkg"));
        let demo = &repository.packages["demo"][0];
        assert_eq!(demo.metadata.version, "1:2.0-1");
        assert_eq!(demo.metadata.depends, "libc6 (>= 2.39), libfoo");
        assert_eq!(demo.metadata.description, "demo summary\nlong text");
        assert_eq!(demo.sha256, "abc");
        assert_eq!(demo.download_size, 5678);
        assert_eq!(demo.metadata.installed_size_kb, 1234);
        // `./` 前缀会被 download_url 归一化掉。
        assert_eq!(
            demo.download_url().as_deref(),
            Some("https://example.com/ubuntu/pool/main/d/demo.deb")
        );
        assert_eq!(demo.repo_base, "https://example.com/ubuntu/");

        // 重复解析同一份索引不会产生第二条记录。
        parse_index_into(
            &mut repository,
            content,
            "https://example.com/ubuntu/",
            "amd64",
        );
        assert_eq!(repository.versions("demo").len(), 1);
    }

    #[test]
    fn reads_gzip_index_file() {
        use flate2::Compression as FlateCompression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("debdeck-gz-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("repo_Packages.gz");
        {
            let mut encoder =
                GzEncoder::new(File::create(&path).unwrap(), FlateCompression::default());
            encoder.write_all(b"Package: demo\nVersion: 1\n\n").unwrap();
        }
        let content = read_index_file(&path).unwrap();
        assert!(content.contains("Package: demo"));
        let _ = fs::remove_dir_all(&dir);
    }
}
