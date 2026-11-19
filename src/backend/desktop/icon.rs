//! 应用图标查找。
//!
//! # 已知限制（有意为之）
//!
//! `Cargo.toml` 里 `image` 只启用 `png` 特性，用体积换取可控依赖，因此本项目
//! **只能解码 PNG**。图标名为 `foo.svg` 或主题里只有 SVG 时，会去找同名
//! `foo.png`；都找不到就返回 `None`，界面回落到字母图标。若将来要支持 SVG，
//! 应引入矢量解码器并统一在 `ui/icons.rs` 里处理，而不是改这里的返回类型。
//!
//! # 查找顺序
//!
//! 遵循 freedesktop 图标主题规范的常用子集：用户目录优先于系统目录，
//! 主题内按尺寸从大到小，最后回落到 `pixmaps`。完整的 `index.theme`
//! `Inherits=` 继承链解析不在范围内 —— 目标机器上的 Ubuntu 主题（Yaru/hicolor）
//! 都在下面的列表里，收益不足以引入整套主题引擎。

use std::env;
use std::path::{Path, PathBuf};

use crate::util::home_dir;

/// 主题内按尺寸查找的顺序：优先大图标，缩放后更清晰。
const SIZES: [&str; 6] = ["256x256", "128x128", "64x64", "48x48", "32x32", "24x24"];

/// 视为「带扩展名的图标名」的后缀集合。
const IMAGE_EXTENSIONS: [&str; 4] = ["svg", "png", "xpm", "ico"];

/// 查找的图标主题。hicolor 是所有主题的兜底，Yaru 是 Ubuntu 默认主题。
const THEMES: [&str; 3] = ["hicolor", "Yaru", "Adwaita"];

/// 图标名可能带扩展名（不规范但现实存在），主题目录按无扩展名的基名索引。
fn icon_base_name(icon: &str) -> &str {
    let path = Path::new(icon);
    let looks_like_image = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        });
    if looks_like_image {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(icon)
    } else {
        // `org.gnome.TextEditor` 的最后一段不是扩展名，必须原样保留。
        icon
    }
}

fn is_png(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

/// 图标搜索根目录：`$XDG_DATA_HOME`（默认 `~/.local/share`）优先，
/// 然后是 `$XDG_DATA_DIRS`（默认 `/usr/local/share:/usr/share`）。
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(3);
    match env::var_os("XDG_DATA_HOME") {
        Some(value) if !value.is_empty() => dirs.push(PathBuf::from(value)),
        _ => dirs.push(home_dir().join(".local/share")),
    }
    match env::var_os("XDG_DATA_DIRS") {
        Some(value) if !value.is_empty() => dirs.extend(env::split_paths(&value)),
        _ => {
            dirs.push(PathBuf::from("/usr/local/share"));
            dirs.push(PathBuf::from("/usr/share"));
        }
    }
    dirs.retain(|dir| dir.is_absolute());
    dirs
}

/// 把 `.desktop` 里的 `Icon=` 值解析成一个可解码的 PNG 路径。
pub fn resolve_icon_path(icon: &str) -> Option<PathBuf> {
    if icon.is_empty() {
        return None;
    }
    let requested = Path::new(icon);
    if requested.is_absolute() {
        // 绝对路径只在确实是 PNG 时才返回，否则调用方会花一次磁盘 IO 换到失败。
        return (is_png(requested) && requested.is_file()).then(|| requested.to_path_buf());
    }

    let base = icon_base_name(icon);
    for dir in data_dirs() {
        for theme in THEMES {
            for size in SIZES {
                let candidate = dir
                    .join("icons")
                    .join(theme)
                    .join(size)
                    .join("apps")
                    .join(format!("{base}.png"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        // pixmaps 是不分主题、不分尺寸的兜底位置。
        let pixmap = dir.join("pixmaps").join(format!("{base}.png"));
        if pixmap.is_file() {
            return Some(pixmap);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_icon_extensions_before_lookup() {
        assert_eq!(icon_base_name("firefox"), "firefox");
        assert_eq!(icon_base_name("firefox.svg"), "firefox");
        assert_eq!(
            icon_base_name("org.gnome.TextEditor.png"),
            "org.gnome.TextEditor"
        );
        // 名字里的点不算扩展名（如 org.gnome.TextEditor 的结尾不是图片后缀）。
        assert_eq!(
            icon_base_name("org.gnome.TextEditor"),
            "org.gnome.TextEditor"
        );
    }

    #[test]
    fn ignores_non_png_absolute_paths() {
        assert_eq!(resolve_icon_path(""), None);
        assert!(resolve_icon_path("/usr/share/icons/hicolor/scalable/apps/absent.svg").is_none());
        assert!(resolve_icon_path("/definitely/not/here.png").is_none());
    }

    #[test]
    fn data_dirs_are_absolute_and_non_empty() {
        let dirs = data_dirs();
        assert!(!dirs.is_empty(), "XDG 数据目录解析结果不应为空");
        assert!(dirs.iter().all(|dir| dir.is_absolute()));
    }
}
