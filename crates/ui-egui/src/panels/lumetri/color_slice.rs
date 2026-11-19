use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_render::effects::COLOR_SLICE_BANDS;
use serde_json::json;

use super::ColorPanelCtx;
use super::wheels::paint_reset_icon_button;
use crate::theme::Tokens;

/// DaVinci Resolve-style ColorSlice panel: 7 subtractive color bands (R, Y, G, C, B, M, Skin)
/// with Saturation, Density (darkening), and Hue shift controls.
pub(super) fn show(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let avail = ui.available_width().max(140.0);
    let cols = if avail >= 620.0 {
        7
    } else if avail >= 360.0 {
        4
    } else {
        2
    };
    let gap = 4.0;
    let card_w = ((avail - gap * (cols - 1) as f32) / cols as f32).floor().max(72.0);

    for row in COLOR_SLICE_BANDS.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(band_id, label, _, _, rgb) in row {
                ui.allocate_ui_with_layout(vec2(card_w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(card_w);
                    band_card(ui, current_panel, band_id, label, Color32::from_rgb(rgb[0], rgb[1], rgb[2]), ctx, t);
                });
            }
        });
        ui.add_space(4.0);
    }
}

fn band_card(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, band_id: &str, label: &str, swatch: Color32, ctx: &mut ColorPanelCtx<'_>, t: &Tokens) {
    let sat_id = format!("cs_{band_id}_sat");
    let den_id = format!("cs_{band_id}_den");
    let hue_id = format!("cs_{band_id}_hue");

    let sat = ctx.e.param(&sat_id).map_or(100.0, |p| p.f64_at(ctx.mt));
    let den = ctx.e.param(&den_id).map_or(0.0, |p| p.f64_at(ctx.mt));
    let hue = ctx.e.param(&hue_id).map_or(0.0, |p| p.f64_at(ctx.mt));

    let modified = (sat - 100.0).abs() > 0.1 || den.abs() > 0.1 || hue.abs() > 0.05;
    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(23, 26, 32))
        .stroke(Stroke::new(1.0, if modified { swatch.gamma_multiply(0.7) } else { Color32::from_rgb(38, 43, 52) }))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(5, 4));

    let inner_w = (ui.available_width() - 10.0).max(56.0);
    frame.show(ui, |ui| {
        ui.set_max_width(inner_w);
        // Header: swatch dot + band name + reset button
        ui.horizontal(|ui| {
            let (dot_r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
            ui.painter().circle_filled(dot_r.center(), 4.0, swatch);
            ui.label(egui::RichText::new(crate::i18n::t(label)).strong().size(10.5).color(if modified { Color32::WHITE } else { t.text }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let rst = paint_reset_icon_button(ui, if modified { swatch } else { t.text_faint }, tl!("Reset band"));
                ctx.auto_rows.push((format!("lumetri.colorslice.{band_id}.reset"), rst.rect, "Reset ColorSlice band"));
                if rst.clicked() {
                    for (pid, def) in [(&sat_id, 100.0), (&den_id, 0.0), (&hue_id, 0.0)] {
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": def, "merge": false}));
                    }
                }
            });
        });

        ui.add_space(2.0);
        // Dual vertical faders: Sat (left) and Den (right)
        let fader_h = 84.0;
        let col_w = ((inner_w - 6.0) * 0.5).max(20.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            vertical_fader(ui, current_panel, &sat_id, "SAT", sat, 100.0, (0.0, 200.0), swatch, col_w, fader_h, ctx);
            vertical_fader(ui, current_panel, &den_id, "DEN", den, 0.0, (-100.0, 100.0), Color32::from_rgb(175, 185, 205), col_w, fader_h, ctx);
        });

        ui.add_space(2.0);
        // Numeric Hue shift row
        let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, band_id);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("HUE").size(8.5).color(t.text_dim));
            let (rh, nh) = crate::widgets::hot_number(
                ui,
                "Hue",
                egui::Id::unique(("cs-hue", scope)),
                hue,
                crate::widgets::HotNumberOptions::new(0.2, (-30.0, 30.0), 1, "°", t),
            );
            ctx.auto_rows.push((format!("lumetri.colorslice.{band_id}.hue"), rh.rect, "Hue"));
            if let Some(v) = nh {
                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": hue_id, "value": v, "merge": true}));
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn vertical_fader(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    param_id: &str,
    short_label: &'static str,
    cur: f64,
    default: f64,
    range: (f64, f64),
    accent: Color32,
    w: f32,
    h: f32,
    ctx: &mut ColorPanelCtx<'_>,
) {
    let t = ctx.t;
    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, param_id.to_string());
    ui.allocate_ui_with_layout(vec2(w, h + 34.0), egui::Layout::top_down(egui::Align::Center), |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
        let p = ui.painter();
        let track = Rect::from_center_size(r.center(), vec2(6.0, h - 8.0));
        p.rect_filled(track, 3.0, Color32::from_rgb(15, 17, 22));
        p.rect_stroke(track, 3.0, Stroke::new(1.0, Color32::from_rgb(44, 50, 62)), StrokeKind::Inside);

        let span = (range.1 - range.0).max(1e-4);
        let norm = ((cur - range.0) / span).clamp(0.0, 1.0) as f32;
        let def_norm = ((default - range.0) / span).clamp(0.0, 1.0) as f32;
        let y_def = track.max.y - def_norm * track.height();
        let y_cur = track.max.y - norm * track.height();

        p.line_segment([pos2(track.min.x - 3.0, y_def), pos2(track.max.x + 3.0, y_def)], Stroke::new(1.0, Color32::from_rgb(80, 88, 102)));
        let fill_r = Rect::from_min_max(pos2(track.min.x + 1.0, y_cur.min(y_def)), pos2(track.max.x - 1.0, y_cur.max(y_def)));
        p.rect_filled(fill_r, 1.0, accent.gamma_multiply(0.75));
        let handle = Rect::from_center_size(pos2(track.center().x, y_cur), vec2(w.min(18.0), 5.0));
        p.rect_filled(handle, 2.0, Color32::from_rgb(235, 240, 248));

        let mut next = None;
        let mut merge = false;
        if resp.double_clicked() {
            next = Some(default);
        } else if (resp.dragged() || resp.clicked())
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let f = ((track.max.y - pos.y) / track.height().max(1.0)).clamp(0.0, 1.0) as f64;
            next = Some(range.0 + f * span);
            merge = resp.dragged();
        }
        if let Some(v) = next
            && (v - cur).abs() > 1e-3
        {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": param_id, "value": v.round(), "merge": merge}));
        }

        let (_, nv) =
            crate::widgets::hot_number(ui, short_label, egui::Id::unique(("cs-num", scope)), cur, crate::widgets::HotNumberOptions::new(0.5, range, 0, "", t));
        if let Some(v) = nv {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": param_id, "value": v, "merge": true}));
        }
        ui.label(egui::RichText::new(short_label).size(8.0).color(t.text_faint));
        ctx.auto_rows.push((format!("lumetri.param.{param_id}"), r, short_label));
    });
}
