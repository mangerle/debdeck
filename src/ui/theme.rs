//! 配色、字体与排版。
//!
//! 颜色不再在绘制代码里内联：主色、状态色与表面色全部来自 gpui-kit 的全局
//! [`Theme`]，这里只负责在启动时把字体与 Ubuntu 橙注入进去。

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;

use gpui_kit::component::Theme;
use gpui_kit::{App, Hsla, SharedString, hsla, px};

use crate::app::RowStatus;

/// 启动时配置主题：系统明暗、中文字体与主色。
///
/// 传入窗口是必要的：Linux 上 `window.appearance()` 才可靠，`None` 可能拿到
/// 默认的浅色外观。
pub(crate) fn configure(cx: &mut App, window: Option<&mut gpui_kit::Window>) {
    // 先跟随系统明暗，再覆盖品牌色；顺序反了会被主题重载冲掉。
    Theme::sync_system_appearance(window, cx);
    let (font_family, mono_family) = load_fonts(cx);

    Theme::update(cx, |theme| {
        theme.font_family = font_family;
        theme.mono_font_family = mono_family;
        theme.font_size = px(15.0);
        theme.mono_font_size = px(13.0);
        theme.radius = px(6.0);
        theme.radius_lg = px(10.0);
        apply_accent(theme);
    });
}

/// Ubuntu 橙。主按钮、选中行与强调文字共用这一组色。
///
/// 两档橙：品牌橙偏亮，用作色块与强调；按钮填充要更深一档，
/// 否则白字压在亮橙上对比度不够，看久了发飘。
fn apply_accent(theme: &mut Theme) {
    let accent = hsla(0.055, 0.86, 0.52, 1.0);
    let fill = hsla(0.055, 0.86, 0.42, 1.0);
    let fill_hover = hsla(0.055, 0.86, 0.38, 1.0);
    let fill_active = hsla(0.055, 0.86, 0.34, 1.0);
    let foreground = hsla(0.0, 0.0, 1.0, 1.0);

    theme.primary = fill;
    theme.primary_hover = fill_hover;
    theme.primary_active = fill_active;
    theme.primary_foreground = foreground;
    theme.accent = accent;
    theme.accent_foreground = foreground;
    theme.button_primary = fill;
    theme.button_primary_hover = fill_hover;
    theme.button_primary_active = fill_active;
    theme.button_primary_foreground = foreground;
    theme.ring = accent.opacity(0.45);
    theme.selection = accent.opacity(0.18);
    theme.list_active = accent.opacity(0.14);
    theme.table_active = accent.opacity(0.14);
    theme.table_active_border = accent;
}

/// 状态标签的颜色。`已保持` 是用户主动选择，不用警告色。
pub(crate) fn status_color(theme: &Theme, status: RowStatus) -> Hsla {
    match status {
        RowStatus::NotInstalled => theme.muted_foreground,
        RowStatus::Installed => theme.success,
        RowStatus::Held => theme.info,
        RowStatus::Update => theme.warning,
    }
}

/// 把系统中文字体注册进 GPUI，并返回 UI/等宽字体族。
fn load_fonts(cx: &mut App) -> (SharedString, SharedString) {
    let Some((path, bytes)) = find_cjk_font() else {
        tracing::warn!("未找到中文字体，界面中的中文可能无法正常显示");
        return (
            ".SystemUIFont".into(),
            default_mono_family()
                .unwrap_or_else(|| ".SystemUIFont".into())
                .into(),
        );
    };

    if let Err(error) = cx.text_system().add_fonts(vec![Cow::Owned(bytes)]) {
        tracing::warn!("注册中文字体失败（{}）：{error:#}", path.display());
    }

    let installed = cx.text_system().all_font_names();
    let requested = family_from_query("sans-serif:lang=zh-cn")
        .or_else(|| font_family_from_file(&path))
        .unwrap_or_else(|| "Noto Sans CJK SC".to_string());
    let family = if installed.iter().any(|name| name == &requested) {
        requested
    } else {
        tracing::warn!("界面字体 {requested:?} 未被字体系统识别，回退到系统 UI 字体");
        ".SystemUIFont".to_string()
    };

    let mono = default_mono_family()
        .filter(|name| installed.iter().any(|installed| installed == name))
        .unwrap_or_else(|| family.clone());

    tracing::info!(
        "界面字体：{family}；等宽字体：{mono}（中文字体 {}）",
        path.display()
    );
    (family.into(), mono.into())
}

fn default_mono_family() -> Option<String> {
    family_from_query("monospace")
}

fn family_from_query(pattern: &str) -> Option<String> {
    let output = Command::new("fc-match")
        .args(["-f", "%{family}", pattern])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    first_family(&String::from_utf8_lossy(&output.stdout))
}

fn font_family_from_file(path: &Path) -> Option<String> {
    let output = Command::new("fc-query")
        .args(["-f", "%{family}\n", path.to_str()?])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    first_family(&String::from_utf8_lossy(&output.stdout))
}

/// `fc-query`/`fc-match` 的 family 可能按字形重复多份（如 `Noto Sans CJK SC,Noto Sans CJK SC`），
/// 取第一行第一个非空项即可。
fn first_family(output: &str) -> Option<String> {
    let first = output.lines().next()?;
    let family = first
        .split(',')
        .map(str::trim)
        .find(|name| !name.is_empty())?;
    Some(family.to_string())
}

/// 按常见路径与 fontconfig 的匹配结果寻找中文字体。
pub(crate) fn find_cjk_font() -> Option<(PathBuf, Vec<u8>)> {
    let mut candidates: Vec<PathBuf> = [
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-VF.otf.ttc",
        "/usr/share/fonts/truetype/arphic/uming.ttc",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();

    if let Ok(output) = Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif:lang=zh-cn"])
        .output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            // fontconfig 的匹配结果优先：它通常是发行版为中文挑选的完整字体
            // （Ubuntu 上是 Noto Sans CJK SC），比硬编码的兜底路径更合适。
            candidates.insert(0, PathBuf::from(path));
        }
    }

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            return Some((path, bytes));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_family_skips_empty_duplicates() {
        assert_eq!(
            first_family("Noto Sans CJK SC,Noto Sans CJK SC\n").as_deref(),
            Some("Noto Sans CJK SC")
        );
        assert_eq!(first_family("\n"), None);
        assert_eq!(
            first_family(" , WQY Micro Hei\n").as_deref(),
            Some("WQY Micro Hei")
        );
    }
}
