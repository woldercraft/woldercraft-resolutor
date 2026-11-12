//! Woldercraft Resolutor brand mark and wordmark (`assets/app-icon/`), decoded once and kept as a texture.

use egui::{Align2, Color32, Pos2, Rect, pos2, vec2};

use crate::theme::Tokens;

const WR_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.filmcraft.png");

/// The Woldercraft Resolutor (`WR`) square icon texture.
pub fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("woldercraft-resolutor-logo");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory_with_format(WR_ICON_PNG, image::ImageFormat::Png).ok()?.to_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    let tex = ctx.load_texture("woldercraft-resolutor-logo", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// Draw the square `WR` icon `size` points tall with its left edge at `left_center` (vertically centred).
pub fn paint_logo(ui: &egui::Ui, left_center: Pos2, size: f32) -> Option<Rect> {
    let tex = logo(ui.ctx())?;
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - size * 0.5), vec2(size, size));
    ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    Some(r)
}

/// Backward-compatible accessor for the brand texture.
pub fn wordmark(ctx: &egui::Context, _dark: bool) -> Option<egui::TextureHandle> {
    logo(ctx)
}

/// Draw the `WR` logo icon plus the `"Woldercraft Resolutor"` wordmark `height` points tall with its
/// left edge at `left_center` (vertically centred). Returns the combined drawn rect.
pub fn paint_wordmark(ui: &egui::Ui, left_center: Pos2, height: f32, dark: bool) -> Option<Rect> {
    let icon_size = (height * 1.35).max(20.0);
    let icon_rect = paint_logo(ui, left_center, icon_size)?;
    let text_col = if dark { Color32::from_rgb(235, 240, 250) } else { Color32::from_rgb(20, 24, 34) };
    let font = Tokens::semibold((height * 0.82).clamp(13.0, 19.0));
    let galley = ui.painter().layout_no_wrap("Resolutor".to_string(), font, text_col);
    let tx = icon_rect.max.x + 8.0;
    let ty = left_center.y - galley.size().y * 0.5;
    let text_w = galley.size().x;
    ui.painter().galley(pos2(tx, ty), galley, text_col);
    let _ = Align2::LEFT_CENTER;
    Some(Rect::from_min_max(icon_rect.min, pos2(tx + text_w, icon_rect.max.y)))
}
