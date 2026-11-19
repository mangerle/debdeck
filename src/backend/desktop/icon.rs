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

/// 读取并缩放指定图标，直接输出 GPUI 所需的 BGRA 字节序位图。
///
/// 磁盘查找、PNG 解码与 Lanczos3 缩放全部在后台线程完成，避免阻塞 UI 渲染帧。
pub fn load_icon_bgra(icon: &str, max_size: u32) -> Option<image::RgbaImage> {
    let path = resolve_icon_path(icon)?;
    let image = image::open(&path).ok()?;
    let scaled = prepare_icon_image(image.to_rgba8(), max_size);
    Some(to_bgra(scaled))
}

/// 将图标缩放到目标尺寸上限，同时保持原始宽高比。
fn prepare_icon_image(image: image::RgbaImage, max_size: u32) -> image::RgbaImage {
    let (width, height) = image.dimensions();
    let longest = width.max(height);
    if longest <= max_size || max_size == 0 {
        return image;
    }

    let scaled_width =
        (u64::from(width) * u64::from(max_size) + u64::from(longest) / 2) / u64::from(longest);
    let scaled_height =
        (u64::from(height) * u64::from(max_size) + u64::from(longest) / 2) / u64::from(longest);
    let width = u32::try_from(scaled_width).unwrap_or(1).max(1);
    let height = u32::try_from(scaled_height).unwrap_or(1).max(1);
    image::imageops::resize(&image, width, height, image::imageops::FilterType::Lanczos3)
}

/// 将 RGBA 像素缓冲区原地转换为 GPUI 纹理所需的 BGRA 排列。
fn to_bgra(mut image: image::RgbaImage) -> image::RgbaImage {
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_rgba_pixels_to_bgra_layout() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 4]));
        assert_eq!(to_bgra(image).get_pixel(0, 0).0, [3, 2, 1, 4]);
    }

    #[test]
    fn prepares_icon_without_distorting_aspect_ratio() {
        let image = image::RgbaImage::from_pixel(128, 64, image::Rgba([1, 2, 3, 4]));
        let prepared = prepare_icon_image(image, 88);
        assert_eq!(prepared.dimensions(), (88, 44));
    }

    #[test]
    fn leaves_small_icon_at_original_size() {
        let image = image::RgbaImage::from_pixel(32, 32, image::Rgba([1, 2, 3, 4]));
        let prepared = prepare_icon_image(image, 88);
        assert_eq!(prepared.dimensions(), (32, 32));
    }

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
