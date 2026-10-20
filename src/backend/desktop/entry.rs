//! `.desktop` 文件解析。

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use crate::model::DesktopEntry;
use crate::util::command_exists;

/// 读取并解析一个桌面条目文件；不合法或不应显示时返回 `None`。
pub fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
    let content = fs::read_to_string(path).ok()?;
    let mut entry = parse_desktop_str(&content, path)?;
    entry.path = path.to_path_buf();
    Some(entry)
}

/// 从文本解析桌面条目。`path` 仅用于在缺少 `Name` 时回落到文件名。
pub fn parse_desktop_str(content: &str, path: &Path) -> Option<DesktopEntry> {
    let values = parse_kv(content);
    if values.is_empty() {
        return None;
    }
    // 只接受应用类条目：Link/Directory 类型没有可执行的启动语义。
    let entry_type = values.get("Type").map_or("Application", String::as_str);
    if entry_type != "Application" {
        return None;
    }
    if is_true(values.get("NoDisplay")) || is_true(values.get("Hidden")) {
        return None;
    }

    let exec = values.get("Exec")?.trim().to_string();
    if exec.is_empty() {
        return None;
    }
    // TryExec 指向的程序不存在时，条目虽然合法也启动不了，直接不展示。
    if let Some(try_exec) = values.get("TryExec")
        && !try_exec.trim().is_empty()
        && !command_exists(try_exec.trim())
    {
        return None;
    }
    if !show_in_allowed(&values) {
        return None;
    }

    let name = localized_value(&values, "Name")
        .filter(|value| !value.is_empty())
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })?;
    Some(DesktopEntry {
        path: path.to_path_buf(),
        name,
        comment: localized_value(&values, "Comment").unwrap_or_default(),
        exec,
        icon: values.get("Icon").cloned().unwrap_or_default(),
        terminal: is_true(values.get("Terminal")),
    })
}

/// 解析出 `[Desktop Entry]` 段内的键值对，忽略注释、其他分节与空行。
fn parse_kv(content: &str) -> HashMap<String, String> {
    let mut in_section = false;
    let mut values = HashMap::new();
    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == "[Desktop Entry]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_string(), unescape_value(value.trim()));
        }
    }
    values
}

/// 当前会话的桌面环境标识，用于 `OnlyShowIn`/`NotShowIn` 匹配。
fn current_desktops() -> Vec<String> {
    let mut desktops = Vec::new();
    for key in [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
    ] {
        if let Ok(value) = env::var(key) {
            for part in value.split([':', ';']) {
                let part = part.trim().to_lowercase();
                if !part.is_empty() {
                    desktops.push(part);
                }
            }
        }
    }
    desktops.sort();
    desktops.dedup();
    desktops
}

/// 按规范匹配 `OnlyShowIn`/`NotShowIn`；拿不到会话信息时一律放行，
/// 宁多勿漏（漏掉应用比显示一个不该出现的应用更难排查）。
fn show_in_allowed(values: &HashMap<String, String>) -> bool {
    let current = current_desktops();
    if current.is_empty() {
        return true;
    }
    if let Some(only_show_in) = values.get("OnlyShowIn") {
        let allowed = split_list(only_show_in);
        if !allowed.is_empty()
            && !allowed
                .iter()
                .any(|name| current.contains(&name.to_lowercase()))
        {
            return false;
        }
    }
    if let Some(not_show_in) = values.get("NotShowIn") {
        let blocked = split_list(not_show_in);
        if blocked
            .iter()
            .any(|name| current.contains(&name.to_lowercase()))
        {
            return false;
        }
    }
    true
}

/// 分号分隔的字符串列表，逐项去空白并转小写。
fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// 按环境变量推导本地化查找顺序：`LANGUAGE` > `LC_MESSAGES` > `LC_ALL` > `LANG`，
/// 每个 locale 再补上语言主名（`zh_CN.UTF-8` 同时给出 `zh_CN` 与 `zh`）。
///
/// 早期实现硬编码了中文 locale 列表，会让英文会话也拿到中文名。
fn locale_candidates() -> Vec<String> {
    let mut out = Vec::new();
    for key in ["LANGUAGE", "LC_MESSAGES", "LC_ALL", "LANG"] {
        let Ok(value) = env::var(key) else { continue };
        // LANGUAGE 是冒号分隔的优先级列表，其余是单个 locale。
        for raw in value.split(':') {
            push_normalized(&mut out, raw);
        }
    }
    out
}

fn push_normalized(out: &mut Vec<String>, raw: &str) {
    let raw = raw.trim();
    // C/POSIX 会话没有本地化信息，跳过。
    if raw.is_empty() || matches!(raw.to_lowercase().as_str(), "c" | "posix") {
        return;
    }
    // 去掉字符集与变体后缀：zh_CN.UTF-8@variant -> zh_CN
    let base = raw.split(['.', '@']).next().unwrap_or(raw).to_string();
    if !out.contains(&base) {
        out.push(base.clone());
    }
    if let Some((language, _)) = base.split_once('_') {
        let language = language.to_string();
        if !out.contains(&language) {
            out.push(language);
        }
    }
}

/// 取 `Key[<locale>]` 变体，全部未命中时回落到 `Key`。
fn localized_value(values: &HashMap<String, String>, key: &str) -> Option<String> {
    for locale in locale_candidates() {
        if let Some(value) = values.get(&format!("{key}[{locale}]"))
            && !value.is_empty()
        {
            return Some(value.clone());
        }
    }
    values.get(key).cloned().filter(|value| !value.is_empty())
}

fn is_true(value: Option<&String>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// 反转义桌面条目的值：`\n \t \r \s \\ \;`。
fn unescape_value(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            result.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => result.push('\n'),
            Some('t') => result.push('\t'),
            Some('r') => result.push('\r'),
            Some('s') => result.push(' '),
            Some(ch @ ('\\' | ';')) => result.push(ch),
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => result.push('\\'),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_desktop_entry() {
        let content = "
[Desktop Entry]
Type=Application
Name=Text Editor
Name[zh_CN]=文本编辑器
Comment=Edit text files
Exec=gedit %U
Icon=org.gnome.TextEditor
Terminal=false
OnlyShowIn=GNOME;Unity;
NoDisplay=false
";
        let entry = parse_desktop_str(content, Path::new("/usr/share/applications/test.desktop"))
            .expect("should parse");
        // 中文名是否被采用取决于会话 locale，两种结果都算正确，
        // 但 Exec 与图标必须稳定。
        assert!(matches!(entry.name.as_str(), "Text Editor" | "文本编辑器"));
        assert_eq!(entry.exec, "gedit %U");
        assert_eq!(entry.icon, "org.gnome.TextEditor");
        assert!(!entry.terminal);
    }

    #[test]
    fn skips_hidden_entries() {
        let content =
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n";
        assert!(parse_desktop_str(content, Path::new("hidden.desktop")).is_none());
    }

    #[test]
    fn ignores_sections_other_than_desktop_entry() {
        let content = "[Desktop Action New]\nName=New Window\nExec=app --new\n\n[Desktop Entry]\nType=Application\nName=Real\nExec=app\n";
        let entry = parse_desktop_str(content, Path::new("x.desktop")).unwrap();
        assert_eq!(entry.name, "Real");
    }

    #[test]
    fn rejects_non_application_types() {
        let content = "[Desktop Entry]\nType=Link\nName=Doc\nExec=app\nURL=https://example.com\n";
        assert!(parse_desktop_str(content, Path::new("x.desktop")).is_none());
    }

    #[test]
    fn unescapes_values() {
        let content =
            "[Desktop Entry]\nType=Application\nName=A\\sB\nExec=app\nComment=Line\\nTwo\n";
        let entry = parse_desktop_str(content, Path::new("test.desktop")).unwrap();
        assert_eq!(entry.name, "A B");
        assert_eq!(entry.comment, "Line\nTwo");
    }

    #[test]
    fn normalises_locale_candidates() {
        let mut out = Vec::new();
        push_normalized(&mut out, "zh_CN.UTF-8");
        push_normalized(&mut out, "zh_CN");
        push_normalized(&mut out, "C");
        push_normalized(&mut out, "");
        assert_eq!(out, ["zh_CN", "zh"]);
    }
}
