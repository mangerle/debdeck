//! deb822 段落格式（RFC2822 风格）的唯一编解码实现。
//!
//! dpkg status、APT 软件包索引、`.sources` 源配置都使用这个格式：
//!
//! * 段落之间用空行分隔；
//! * 字段行是 `Name: value`；
//! * 以空格或制表符开头的行是**上一个字段值的续行**，解析时保留换行、去掉前导空白；
//! * 描述里的单独 `.` 表示一个空行；
//! * 需容忍 CRLF 与结尾没有空行的情形。
//!
//! 本模块刻意不认识「软件包」概念（不出现 Package/Version 之类的字段名），
//! 因此 `backend` 与 `model` 都可以依赖它而不会形成反向依赖。
//! 小文本使用 [`parse_stanzas`]；大文件使用 [`for_each_stanza`] 逐段消费，
//! 不要在调用方另写解析循环。

use std::io::{self, BufRead};

/// 一个段落：有序、可重复的字段列表。
///
/// 保留顺序是因为 dpkg 控制字段有约定顺序，重建 control 文件时要照抄。
#[derive(Clone, Debug, Default)]
pub struct Stanza {
    pub fields: Vec<(String, String)>,
}

impl Stanza {
    /// 首个同名取值。
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// 取首行（多行字段中的第一行），用于版本、架构这类单行语义字段。
    pub fn get_first_line(&self, key: &str) -> String {
        self.get(key)
            .and_then(|value| value.lines().next())
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// 把所有行用单个空格拼成一行，用于 `Depends` 这类被换行折开的列表字段。
    pub fn get_joined(&self, key: &str) -> String {
        self.get(key)
            .map(|value| {
                value
                    .lines()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string()
            })
            .unwrap_or_default()
    }

    /// `Description` 去掉缩进后的完整描述（不含摘要行）。
    pub fn description(&self) -> String {
        let Some(value) = self.get("Description") else {
            return String::new();
        };
        let mut lines = value.lines();
        let mut result = lines.next().unwrap_or_default().trim().to_string();
        for line in lines {
            result.push('\n');
            let line = line.trim_start();
            if line != "." {
                result.push_str(line);
            }
        }
        result.trim_end().to_string()
    }

    /// `Description` 的摘要行。
    pub fn summary(&self) -> String {
        self.get("Description")
            .and_then(|value| value.lines().next())
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

/// 把整份文本解析成段落列表。
pub fn parse_stanzas(text: &str) -> Vec<Stanza> {
    let stanza_capacity = text
        .lines()
        .filter(|line| line.trim().is_empty())
        .count()
        .saturating_add(1);
    let mut stanzas = Vec::with_capacity(stanza_capacity);
    let mut builder = StanzaBuilder::default();
    for raw_line in text.lines() {
        builder.push_line(raw_line);
        if let Some(stanza) = builder.take_on_empty_line(raw_line) {
            stanzas.push(stanza);
        }
    }
    if let Some(stanza) = builder.finish() {
        stanzas.push(stanza);
    }
    stanzas
}

/// 从流中逐段读取 deb822 数据，避免大索引在内存中形成完整字符串副本。
///
/// # Errors
///
/// 底层读取器返回 I/O 错误时立即中止；已经交给回调的段落不受影响。
pub fn for_each_stanza(reader: impl BufRead, mut on_stanza: impl FnMut(Stanza)) -> io::Result<()> {
    let mut builder = StanzaBuilder::default();
    for line in reader.lines() {
        let raw_line = line?;
        builder.push_line(&raw_line);
        if let Some(stanza) = builder.take_on_empty_line(&raw_line) {
            on_stanza(stanza);
        }
    }
    if let Some(stanza) = builder.finish() {
        on_stanza(stanza);
    }
    Ok(())
}

struct StanzaBuilder {
    fields: Vec<(String, String)>,
    last_value: Option<usize>,
}

impl Default for StanzaBuilder {
    fn default() -> Self {
        Self {
            fields: Vec::with_capacity(16),
            last_value: None,
        }
    }
}

impl StanzaBuilder {
    fn push_line(&mut self, raw_line: &str) {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(' ') || line.starts_with('\t') {
            if let Some(index) = self.last_value
                && !line.is_empty()
            {
                let value = &mut self.fields[index].1;
                value.push('\n');
                value.push_str(line.trim_start());
            }
            return;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if name.is_empty() {
                return;
            }
            self.fields
                .push((name.to_string(), value.trim_start().to_string()));
            self.last_value = Some(self.fields.len().saturating_sub(1));
        }
    }

    fn take_on_empty_line(&mut self, raw_line: &str) -> Option<Stanza> {
        if raw_line.trim_end_matches('\r').is_empty() {
            self.take()
        } else {
            None
        }
    }

    fn take(&mut self) -> Option<Stanza> {
        if self.fields.is_empty() {
            return None;
        }
        self.last_value = None;
        Some(Stanza {
            fields: std::mem::take(&mut self.fields),
        })
    }

    fn finish(&mut self) -> Option<Stanza> {
        self.take()
    }
}

/// 按 deb822 规则写出一个字段：首行顶格，续行前置一个空格，空行写成 ` .`。
pub fn write_field(output: &mut String, key: &str, value: &str) {
    let mut lines = value.lines();
    let first = lines.next().unwrap_or_default();
    output.push_str(key);
    output.push_str(": ");
    output.push_str(first.trim_end());
    output.push('\n');
    for line in lines {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            output.push_str(" .\n");
        } else {
            output.push(' ');
            output.push_str(trimmed.trim_start());
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_stanzas_and_continuations() {
        let text = "Package: hello\nVersion: 1.0\nDescription: short summary\n long line one\n .\n long line two\n\nPackage: world\nVersion: 2.0\n";
        let stanzas = parse_stanzas(text);
        assert_eq!(stanzas.len(), 2);
        assert_eq!(stanzas[0].get("Package"), Some("hello"));
        assert_eq!(stanzas[0].summary(), "short summary");
        assert_eq!(
            stanzas[0].description(),
            "short summary\nlong line one\n\nlong line two"
        );
        assert_eq!(stanzas[1].get("Version"), Some("2.0"));
    }

    #[test]
    fn handles_crlf_and_eof_without_blank_line() {
        let text = "Package: app\r\nVersion: 1\r\n";
        let stanzas = parse_stanzas(text);
        assert_eq!(stanzas.len(), 1);
        assert_eq!(stanzas[0].get("Version"), Some("1"));
    }

    #[test]
    fn joins_folded_list_fields() {
        let stanzas = parse_stanzas("Depends: libc6 (>= 2.39),\n libfoo,\n libbar\n\n");
        assert_eq!(
            stanzas[0].get_joined("Depends"),
            "libc6 (>= 2.39), libfoo, libbar"
        );
        assert_eq!(stanzas[0].get_first_line("Depends"), "libc6 (>= 2.39),");
    }

    #[test]
    fn writes_folded_field() {
        let mut output = String::new();
        write_field(&mut output, "Description", "one\n\ntwo");
        assert_eq!(output, "Description: one\n .\n two\n");
    }
}
