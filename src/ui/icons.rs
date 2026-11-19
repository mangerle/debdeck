//! 图标位图：应用图标的纹理包装、字母占位图与内嵌窗口图标。

use std::sync::Arc;

use gpui_kit::component::Theme;
use gpui_kit::{FontWeight, IntoElement, ParentElement as _, RenderImage, Styled as _, div, px};
use smallvec::smallvec;

/// 详情页应用图标的逻辑尺寸。
pub(crate) const DETAIL_ICON_SIZE: f32 = 44.0;
/// 按 2x 缩放准备详情图标纹理。
pub(crate) const DETAIL_ICON_TEXTURE_SIZE: u32 = 88;

/// 将后台解码并转为 BGRA 字节序的位图包装为 GPUI 纹理。
pub(crate) fn render_image_from_bgra(bgra: image::RgbaImage) -> Arc<RenderImage> {
    let frame = image::Frame::new(bgra);
    Arc::new(RenderImage::new(smallvec![frame]))
}

/// GPUI 的 `RenderImage` 按 BGRA 上传纹理；内嵌 PNG 解码得到的是 RGBA。
fn to_bgra(mut image: image::RgbaImage) -> image::RgbaImage {
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    image
}

/// 没有图标时的兜底：色块 + 首字母。
pub(crate) fn draw_letter_icon(name: &str, theme: &Theme) -> impl IntoElement {
    let letter = name
        .chars()
        .next()
        .map_or_else(|| "?".to_string(), |ch| ch.to_uppercase().to_string());
    div()
        .w(px(DETAIL_ICON_SIZE))
        .h(px(DETAIL_ICON_SIZE))
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

/// 标题栏图标的逻辑尺寸。
pub(crate) const APP_ICON_SIZE: f32 = 24.0;
/// 按 2x 缩放准备纹理，避免 GPU 从 512px 原图直接缩到 24px。
const APP_ICON_TEXTURE_SIZE: u32 = 48;

/// 随二进制内嵌的应用图标，不依赖安装路径。
fn embedded_icon() -> Option<image::RgbaImage> {
    image::load_from_memory(include_bytes!("../../assets/icon.png"))
        .ok()
        .map(|image| image.to_rgba8())
}

/// 标题栏使用的 GPUI 图像纹理；启动时只需解码和高质量缩放一次。
pub(crate) fn app_icon_texture() -> Option<Arc<RenderImage>> {
    let icon = embedded_icon()?;
    let icon = image::imageops::resize(
        &icon,
        APP_ICON_TEXTURE_SIZE,
        APP_ICON_TEXTURE_SIZE,
        image::imageops::FilterType::Lanczos3,
    );
    let frame = image::Frame::new(to_bgra(icon));
    Some(Arc::new(RenderImage::new(smallvec![frame])))
}

/// 窗口图标：随二进制内嵌，不依赖安装路径。
///
/// GPUI 只在 X11 上使用它；Wayland 由 `app_id` 和 `.desktop` 文件的图标接管。
pub fn window_icon() -> Option<Arc<image::RgbaImage>> {
    embedded_icon().map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_app_icon_decodes() {
        assert!(app_icon_texture().is_some());
    }
}
