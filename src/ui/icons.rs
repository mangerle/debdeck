//! 图标位图：应用图标的加载缓存与窗口图标。

use std::sync::Arc;

use gpui_kit::component::Theme;
use gpui_kit::{FontWeight, IntoElement, ParentElement as _, RenderImage, Styled as _, div, px};
use smallvec::smallvec;

use crate::app::IconCache;
use crate::backend::desktop::resolve_icon_path;

/// 取图标纹理，未命中缓存时才读盘解码。
///
/// 解析失败的图标名也要缓存（`None`），否则每一次重绘都会对同一个不存在的文件
/// 重试一次 `stat` + 打开。
pub(crate) fn load_icon_texture(
    cache: &mut IconCache,
    icon_name: &str,
) -> Option<Arc<RenderImage>> {
    if icon_name.is_empty() {
        return None;
    }
    if let Some(cached) = cache.get(icon_name) {
        return cached.clone();
    }
    let texture = resolve_icon_path(icon_name).and_then(|path| {
        image::open(&path).ok().map(|image| {
            let frame = image::Frame::new(image.to_rgba8());
            Arc::new(RenderImage::new(smallvec![frame]))
        })
    });
    cache.insert(icon_name.to_string(), texture.clone());
    texture
}

/// 没有图标时的兜底：色块 + 首字母。
pub(crate) fn draw_letter_icon(name: &str, theme: &Theme) -> impl IntoElement {
    let letter = name
        .chars()
        .next()
        .map_or_else(|| "?".to_string(), |ch| ch.to_uppercase().to_string());
    div()
        .w(px(44.0))
        .h(px(44.0))
        .rounded(px(10.0))
        .bg(theme.primary)
        .text_color(theme.primary_foreground)
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(letter)
}

/// 窗口图标：随二进制内嵌，不依赖安装路径。
///
/// GPUI 只在 X11 上使用它；Wayland 由 `app_id` 和 `.desktop` 文件的图标接管。
pub fn window_icon() -> Option<Arc<image::RgbaImage>> {
    let image = image::load_from_memory(include_bytes!("../../assets/icon.png"))
        .ok()?
        .to_rgba8();
    Some(Arc::new(image))
}
