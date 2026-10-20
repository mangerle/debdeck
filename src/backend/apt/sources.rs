//! APT 软件源配置的两种格式（`sources.list` 与 deb822 的 `sources`）。
//!
//! 解析结果展开成 `(uri, suite, component)` 三元组，用来把
//! `/var/lib/apt/lists` 里的索引文件名反查回它的源地址，从而拼出下载 URL。
//!
//! # 支持的 URI 方案
//!
//! 只有 `http(s)` 参与地址拼接。`file:`、`cdrom:` 等本地源**不会**被支持：
//! 它们的索引文件命名规则与网络源不同，凭猜测拼接会产出一个看似能用、实则错误
//! 的下载地址。这类源会被记录成一条警告，让「为什么这个包不能下载」在界面上可见。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::deb822::parse_stanzas;

use crate::backend::fs_layout::FsLayout;

/// 一个展开后的源：单个 suite/component 组合。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AptSource {
    pub uri: String,
    pub suite: String,
    pub component: String,
}

impl AptSource {
    /// 索引文件名里用于匹配该源的 URI 片段：去掉协议与尾斜杠，`/` 换成 `_`。
    ///
    /// 这是 APT 在 `lists` 目录里的命名方式，例如
    /// `https://mirrors.aliyun.com/ubuntu/` → `mirrors.aliyun.com_ubuntu`。
    pub fn uri_key(&self) -> Option<String> {
        let (_, rest) = self.uri.split_once("://")?;
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() {
            return None;
        }
        Some(rest.replace('/', "_"))
    }
}

/// 解析过程的累积器：去重后的源列表 + 面向用户的警告。
#[derive(Default)]
struct Collector {
    sources: Vec<AptSource>,
    seen: HashSet<AptSource>,
    warnings: Vec<String>,
    /// 需要跳过的 URI 方案，按方案聚合，避免每个包一行警告。
    unsupported: HashSet<String>,
}

impl Collector {
    fn insert(&mut self, source: AptSource) {
        if self.seen.insert(source.clone()) {
            self.sources.push(source);
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    /// 记录一个不支持的 URI，返回其方案名。
    fn note_unsupported(&mut self, uri: &str) {
        let scheme = uri.split(':').next().unwrap_or(uri).to_lowercase();
        self.unsupported.insert(scheme);
    }

    fn finish(mut self) -> (Vec<AptSource>, Vec<String>) {
        let mut schemes: Vec<&String> = self.unsupported.iter().collect();
        schemes.sort();
        if !schemes.is_empty() {
            self.warn(format!(
                "已跳过不支持的源类型：{}（仅 http/https 源可在线获取安装包）",
                schemes
                    .iter()
                    .map(|scheme| format!("{scheme}:"))
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        (self.sources, self.warnings)
    }
}

/// 读取 `/etc/apt/sources.list` 与 `sources.list.d/*`。
pub fn read_sources(layout: &FsLayout, native_arch: &str) -> (Vec<AptSource>, Vec<String>) {
    let mut collector = Collector::default();

    if layout.apt_sources_list.is_file() {
        match fs::read_to_string(&layout.apt_sources_list) {
            Ok(content) => parse_sources_list(&content, native_arch, &mut collector),
            Err(error) => collector.warn(format!(
                "读取 {} 失败：{error}",
                layout.apt_sources_list.display()
            )),
        }
    }

    let mut files: Vec<_> = fs::read_dir(&layout.apt_sources_dir)
        .map(|dir| {
            dir.filter_map(std::io::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext == "list" || ext == "sources")
                })
                .collect()
        })
        .unwrap_or_default();
    // 按文件名排序，保证解析顺序（以及警告顺序）稳定可复现。
    files.sort();

    for path in files {
        let Ok(content) = fs::read_to_string(&path) else {
            collector.warn(format!("读取 {} 失败", path.display()));
            continue;
        };
        if is_deb822_sources(&path) {
            parse_deb822_sources(&content, native_arch, &mut collector);
        } else {
            parse_sources_list(&content, native_arch, &mut collector);
        }
    }

    collector.finish()
}

/// deb822 格式（`.sources`）与传统格式（`.list`）的区分。
fn is_deb822_sources(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "sources")
}

/// 解析一行传统源配置，例如
/// `deb [arch=amd64 signed-by=/x.gpg] https://host/ubuntu noble main universe`。
fn parse_sources_line(line: &str, native_arch: &str, collector: &mut Collector) {
    let mut tokens = line.split_whitespace().peekable();
    let Some(kind) = tokens.next() else { return };
    // `deb-src` 是源码包，不参与应用列表；带 `-[signed-by]` 之类后缀的历史写法也一并忽略。
    if kind != "deb" {
        return;
    }

    // 方括号里的选项：只关心 arch=，其余（signed-by、trusted 等）与索引解析无关。
    let mut arches: Vec<String> = Vec::new();
    if tokens.peek().is_some_and(|token| token.starts_with('[')) {
        let mut options = String::new();
        for token in &mut tokens {
            options.push_str(token);
            if token.ends_with(']') {
                break;
            }
            options.push(' ');
        }
        for option in options
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split_whitespace()
        {
            if let Some((key, value)) = option.split_once('=')
                && key.eq_ignore_ascii_case("arch")
            {
                arches = value
                    .split(',')
                    .map(|arch| arch.trim().to_lowercase())
                    .collect();
            }
        }
    }
    if !arches.is_empty()
        && !arches
            .iter()
            .any(|arch| arch == native_arch || arch == "all")
    {
        // 该源不提供本机架构，跳过即可，不是异常。
        return;
    }

    let Some(uri) = tokens.next() else { return };
    let Some(suite) = tokens.next() else { return };
    if !uri.contains("://") {
        collector.note_unsupported(uri);
        return;
    }
    let components: Vec<String> = tokens.map(ToString::to_string).collect();
    let components = if components.is_empty() {
        // flat repository（无 component 段）；APT 用 `/` 占位。
        vec![String::new()]
    } else {
        components
    };
    for component in components {
        collector.insert(AptSource {
            uri: uri.to_string(),
            suite: suite.to_string(),
            component,
        });
    }
}

fn parse_sources_list(content: &str, native_arch: &str, collector: &mut Collector) {
    for raw_line in content.lines() {
        // 传统格式里 `#` 之后的内容是行内注释。
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        parse_sources_line(line, native_arch, collector);
    }
}

fn parse_deb822_sources(content: &str, native_arch: &str, collector: &mut Collector) {
    for stanza in parse_stanzas(content) {
        if stanza
            .get("Enabled")
            .is_some_and(|value| value.eq_ignore_ascii_case("no"))
        {
            continue;
        }
        let types: Vec<&str> = stanza
            .get("Types")
            .unwrap_or("deb")
            .split_whitespace()
            .collect();
        if !types.contains(&"deb") {
            continue;
        }
        let arches: Vec<String> = stanza
            .get_joined("Architectures")
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if !arches.is_empty()
            && !arches
                .iter()
                .any(|arch| arch == native_arch || arch == "all")
        {
            continue;
        }
        let Some(uris) = stanza.get("URIs") else {
            continue;
        };
        let suites = stanza.get("Suites").unwrap_or_default();
        let components: Vec<String> = stanza
            .get("Components")
            .unwrap_or_default()
            .split_whitespace()
            .map(ToString::to_string)
            .collect();
        let components = if components.is_empty() {
            vec![String::new()]
        } else {
            components
        };

        for uri in uris.split_whitespace() {
            let uri = uri.replace("${ARCH}", native_arch);
            if !uri.contains("://") {
                collector.note_unsupported(&uri);
                continue;
            }
            for suite in suites.split_whitespace() {
                for component in &components {
                    collector.insert(AptSource {
                        uri: uri.clone(),
                        suite: suite.to_string(),
                        component: component.clone(),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(content: &str, arch: &str) -> (Vec<AptSource>, Vec<String>) {
        let mut collector = Collector::default();
        parse_sources_list(content, arch, &mut collector);
        collector.finish()
    }

    #[test]
    fn parses_plain_and_option_lines() {
        let (sources, warnings) = collect(
            "deb http://archive.ubuntu.com/ubuntu noble main restricted\n\
             deb [arch=amd64 signed-by=/etc/apt/key.gpg] https://host/pub noble-updates universe\n\
             deb-src http://archive.ubuntu.com/ubuntu noble main\n\
             # deb http://commented.example noble main\n\
             \n",
            "amd64",
        );
        assert!(warnings.is_empty());
        assert_eq!(sources.len(), 3);
        assert!(sources.iter().any(|s| s.component == "universe"
            && s.suite == "noble-updates"
            && s.uri == "https://host/pub"));
        assert_eq!(
            sources[0].uri_key().as_deref(),
            Some("archive.ubuntu.com_ubuntu")
        );
    }

    #[test]
    fn skips_foreign_architecture_sources() {
        let (sources, _) = collect("deb [arch=i386] http://host/deb noble main\n", "amd64");
        assert!(sources.is_empty());
    }

    #[test]
    fn reports_unsupported_schemes_once() {
        let (sources, warnings) = collect(
            "deb file:/srv/mirror noble main\ndeb cdrom:[Ubuntu]/ noble main\n",
            "amd64",
        );
        assert!(sources.is_empty());
        assert_eq!(warnings.len(), 1, "同类问题应聚合成一条：{warnings:?}");
        assert!(warnings[0].contains("file") && warnings[0].contains("cdrom"));
    }

    #[test]
    fn deb822_expands_suites_and_components() {
        let content = "Types: deb\nURIs: https://example.com/ubuntu/\nSuites: noble noble-updates\nComponents: main universe\n\nTypes: deb\nURIs: https://other.example/debian\nSuites: stable\nComponents: main\nEnabled: no\n";
        let mut collector = Collector::default();
        parse_deb822_sources(content, "amd64", &mut collector);
        let (sources, warnings) = collector.finish();
        assert!(warnings.is_empty());
        assert_eq!(sources.len(), 4, "禁用的段落不应展开");
        assert!(sources.iter().any(|source| source.suite == "noble-updates"
            && source.component == "universe"
            && source.uri == "https://example.com/ubuntu/"));
    }

    #[test]
    fn deb822_honours_architectures_and_variables() {
        let content = "Types: deb\nURIs: https://example.com/${ARCH}/\nSuites: stable\nComponents: main\nArchitectures: amd64 i386\n\nTypes: deb\nURIs: https://arm.example/deb\nSuites: stable\nComponents: main\nArchitectures: arm64\n";
        let mut collector = Collector::default();
        parse_deb822_sources(content, "amd64", &mut collector);
        let (sources, _) = collector.finish();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].uri, "https://example.com/amd64/");
    }
}
