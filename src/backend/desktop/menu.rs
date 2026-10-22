//! 用户级应用菜单可见性。
//!
//! XDG 桌面条目按数据目录优先级覆盖：在 `~/.local/share/applications` 放一个
//! 同名 `.desktop` 并写 `Hidden=true`，桌面环境就会忽略系统条目。DebDeck 用
//! `X-DebDeck-Hidden=true` 标记自己写的覆盖文件；恢复时只删除带标记的文件，
//! 不碰用户已有的自定义条目，也不修改包内文件。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::model::MenuEntryState;
use crate::util::home_dir;

const MARKER: &str = "X-DebDeck-Hidden=true";

/// 当前用户的 applications 目录（XDG_DATA_HOME 优先）。
fn applications_dir() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home_dir().join(".local/share"), PathBuf::from)
        .join("applications")
}

fn override_path_in(app_dir: &Path, desktop_path: &Path) -> Option<PathBuf> {
    desktop_path.file_name().map(|name| app_dir.join(name))
}

/// 菜单项的当前可见性。只读操作，供 Catalog 构建时使用。
pub fn menu_state(desktop_path: &Path) -> MenuEntryState {
    menu_state_in(&applications_dir(), desktop_path)
}

fn menu_state_in(app_dir: &Path, desktop_path: &Path) -> MenuEntryState {
    let Some(path) = override_path_in(app_dir, desktop_path) else {
        return MenuEntryState::Visible;
    };
    if !path.exists() {
        return MenuEntryState::Visible;
    }
    match fs::read_to_string(&path) {
        Ok(content) if is_owned(&content) => MenuEntryState::Hidden,
        _ => MenuEntryState::ManagedByOther,
    }
}

/// 隐藏或恢复应用菜单项。`hidden=true` 时创建用户级覆盖文件，
/// `hidden=false` 时只删除 DebDeck 自己写的覆盖文件。
pub fn set_hidden(desktop_path: &Path, hidden: bool) -> Result<MenuEntryState> {
    let state = set_hidden_in(&applications_dir(), desktop_path, hidden)?;
    refresh_menu();
    Ok(state)
}

fn set_hidden_in(app_dir: &Path, desktop_path: &Path, hidden: bool) -> Result<MenuEntryState> {
    let path = override_path_in(app_dir, desktop_path)
        .context("桌面条目没有文件名，无法生成用户级覆盖文件")?;

    if hidden {
        if path.exists() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("无法读取 {}", path.display()))?;
            if is_owned(&content) {
                return Ok(MenuEntryState::Hidden);
            }
            bail!("该应用已有用户自定义的桌面条目，DebDeck 不会覆盖它");
        }

        let source = fs::read_to_string(desktop_path)
            .with_context(|| format!("无法读取 {}", desktop_path.display()))?;
        let content = hidden_override(&source, desktop_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("无法创建 {}", parent.display()))?;
        }
        fs::write(&path, content).with_context(|| format!("无法写入 {}", path.display()))?;
        Ok(MenuEntryState::Hidden)
    } else {
        if !path.exists() {
            return Ok(MenuEntryState::Visible);
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        if !is_owned(&content) {
            bail!("该应用的桌面条目不是由 DebDeck 隐藏的，未做修改");
        }
        fs::remove_file(&path).with_context(|| format!("无法删除 {}", path.display()))?;
        Ok(MenuEntryState::Visible)
    }
}

fn is_owned(content: &str) -> bool {
    content.lines().any(|line| line.trim() == MARKER)
}

/// 复制系统条目并注入 `Hidden=true` 与 DebDeck 标记。
///
/// 复制而不是写一个只有 `Hidden=true` 的空壳，是为了保留 `Name`/`Exec`/
/// 本地化字段等，避免某些桌面环境要求覆盖文件本身仍是一条合法条目。
fn hidden_override(source: &str, desktop_path: &Path) -> Result<String> {
    let mut out = String::with_capacity(source.len() + 64);
    let mut in_main = false;
    let mut saw_main = false;
    let mut inserted = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_main && !inserted {
                out.push_str("Hidden=true\n");
                out.push_str(MARKER);
                out.push('\n');
                inserted = true;
            }
            in_main = trimmed == "[Desktop Entry]";
            saw_main |= in_main;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_main {
            let key = trimmed
                .split_once('=')
                .map_or(trimmed, |(key, _)| key.trim());
            if key == "Hidden" || key == "X-DebDeck-Hidden" {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    if in_main && !inserted {
        out.push_str("Hidden=true\n");
        out.push_str(MARKER);
        out.push('\n');
        inserted = true;
    }
    if !saw_main || !inserted {
        bail!(
            "桌面条目缺少 [Desktop Entry] 段：{}",
            desktop_path.display()
        );
    }
    Ok(out)
}

/// 让桌面环境尽快重建菜单缓存；命令缺失时静默跳过（GNOME 本身会监视目录）。
///
/// 放到后台线程：文件已经落盘，DebDeck 的状态可以立即切换；桌面菜单的刷新
/// 本来就是异步的，不该让 UI 等这个命令。
fn refresh_menu() {
    let dir = applications_dir();
    std::thread::spawn(move || {
        if let Err(error) = Command::new("update-desktop-database")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            tracing::debug!("update-desktop-database 失败：{error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("debdeck-menu-{}-{name}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn source(dir: &Path) -> PathBuf {
        let path = dir.join("system/app.desktop");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=Demo\nExec=demo\nHidden=false\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn hides_and_restores_owned_override() {
        let root = temp_dir("hide-show");
        let app_dir = root.join("apps");
        let desktop = source(&root);

        assert_eq!(menu_state_in(&app_dir, &desktop), MenuEntryState::Visible);
        assert_eq!(
            set_hidden_in(&app_dir, &desktop, true).unwrap(),
            MenuEntryState::Hidden
        );

        let override_path = app_dir.join("app.desktop");
        let content = fs::read_to_string(&override_path).unwrap();
        assert!(content.contains("Hidden=true"));
        assert!(content.contains(MARKER));
        assert!(!content.contains("Hidden=false"));
        assert_eq!(menu_state_in(&app_dir, &desktop), MenuEntryState::Hidden);

        assert_eq!(
            set_hidden_in(&app_dir, &desktop, false).unwrap(),
            MenuEntryState::Visible
        );
        assert!(!override_path.exists());
        assert_eq!(menu_state_in(&app_dir, &desktop), MenuEntryState::Visible);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_touch_foreign_override() {
        let root = temp_dir("foreign");
        let app_dir = root.join("apps");
        fs::create_dir_all(&app_dir).unwrap();
        let desktop = source(&root);
        let override_path = app_dir.join("app.desktop");
        let original = "[Desktop Entry]\nType=Application\nName=My Demo\nExec=demo-custom\n";
        fs::write(&override_path, original).unwrap();

        assert_eq!(
            menu_state_in(&app_dir, &desktop),
            MenuEntryState::ManagedByOther
        );
        assert!(set_hidden_in(&app_dir, &desktop, true).is_err());
        assert!(set_hidden_in(&app_dir, &desktop, false).is_err());
        assert_eq!(fs::read_to_string(&override_path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inserts_hidden_before_next_section() {
        let source = "[Desktop Entry]\nType=Application\nName=Demo\nExec=demo\n\n[Desktop Action New]\nName=New\nExec=demo --new\n";
        let content =
            hidden_override(source, Path::new("/usr/share/applications/app.desktop")).unwrap();
        let hidden = content.find("Hidden=true").unwrap();
        let action = content.find("[Desktop Action New]").unwrap();
        assert!(
            hidden < action,
            "Hidden 必须留在 [Desktop Entry] 段内：{content}"
        );
    }
}
