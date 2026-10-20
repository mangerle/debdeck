//! 图标位图：应用图标的加载缓存与窗口图标。

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontId, Sense, Ui, Vec2};

use crate::backend::desktop::resolve_icon_path;
use crate::ui::IconCache;
use crate::ui::theme::ACCENT;

/// 取图标纹理，未命中缓存时才读盘解码。
///
/// 解析失败的图标名也要缓存（`None`），否则每一帧都会对同一个不存在的文件重试
/// 一次 `stat` + 打开。
pub(crate) fn load_icon_texture(
    cache: &mut IconCache,
    ctx: &egui::Context,
    icon_name: &str,
) -> Option<egui::TextureHandle> {
    if icon_name.is_empty() {
        return None;
    }
    if let Some(cached) = cache.get(icon_name) {
        return cached.clone();
    }
    let texture = resolve_icon_path(icon_name).and_then(|path| {
        image::open(&path).ok().map(|image| {
            let image = image.to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            ctx.load_texture(
                format!("app-icon:{icon_name}"),
                color_image,
                egui::TextureOptions::LINEAR,
            )
        })
    });
    cache.insert(icon_name.to_string(), texture.clone());
    texture
}

/// 没有图标时的兜底：色块 + 首字母。
pub(crate) fn draw_letter_icon(ui: &mut Ui, name: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(12), ACCENT);
    let letter = name
        .chars()
        .next()
        .map_or_else(|| "?".to_string(), |ch| ch.to_uppercase().to_string());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        letter,
        FontId::proportional(22.0),
        Color32::WHITE,
    );
}

/// 窗口图标：随二进制内嵌，不依赖安装路径。
pub fn window_icon() -> Option<Arc<egui::IconData>> {
    let image = image::load_from_memory(include_bytes!("../../assets/icon.png"))
        .ok()?
        .to_rgba8();
    Some(Arc::new(egui::IconData {
        rgba: image.as_raw().clone(),
        width: image.width(),
        height: image.height(),
    }))
}
