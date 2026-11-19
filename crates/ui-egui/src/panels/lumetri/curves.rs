use egui::{Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use super::ColorPanelCtx;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CurveDomain {
    CustomRgb,
    HueVs,
    LumVsSat,
    SatVs,
}

impl CurveDomain {
    fn from_id(id: &str, hue: bool) -> Self {
        if !hue {
            Self::CustomRgb
        } else if id.starts_with("hue_vs_") {
            Self::HueVs
        } else if id == "luma_vs_sat" || id == "lum_vs_sat" {
            Self::LumVsSat
        } else {
            Self::SatVs
        }
    }
}

fn paint_vertical_axis_bar(p: &egui::Painter, bar: Rect, id: &str, domain: CurveDomain) {
    let n = 32usize;
    for i in 0..n {
        let f0 = i as f32 / n as f32;
        let f1 = (i + 1) as f32 / n as f32;
        let v = 1.0 - (f0 + f1) * 0.5;
        let c = match domain {
            CurveDomain::CustomRgb | CurveDomain::LumVsSat => {
                let g = (v * 210.0 + 20.0).clamp(0.0, 255.0) as u8;
                Color32::from_rgb(g, g, g)
            }
            CurveDomain::HueVs if id == "hue_vs_hue" => {
                let rgb = filmcraft_color::hsl_to_rgb(v, 0.75, 0.50);
                Color32::from_rgb((rgb[0].clamp(0.0, 1.0) * 255.0) as u8, (rgb[1].clamp(0.0, 1.0) * 255.0) as u8, (rgb[2].clamp(0.0, 1.0) * 255.0) as u8)
            }
            CurveDomain::HueVs if id == "hue_vs_sat" => {
                let r = (100.0 + v * 135.0).clamp(0.0, 255.0) as u8;
                let g = (100.0 + v * 45.0).clamp(0.0, 255.0) as u8;
                let b = (110.0 + v * 90.0).clamp(0.0, 255.0) as u8;
                Color32::from_rgb(r, g, b)
            }
            CurveDomain::HueVs | CurveDomain::SatVs => {
                let g = (v * 215.0 + 18.0).clamp(0.0, 255.0) as u8;
                Color32::from_rgb(g, g, g)
            }
        };
        p.rect_filled(Rect::from_min_max(pos2(bar.min.x, bar.min.y + bar.height() * f0), pos2(bar.max.x, bar.min.y + bar.height() * f1 + 0.5)), 0.0, c);
    }
}

fn paint_domain_bar(p: &egui::Painter, area: Rect, domain: CurveDomain) {
    let n = 48usize;
    let bar_h = 7.0;
    let y0 = area.max.y - bar_h;
    for i in 0..n {
        let f0 = i as f32 / n as f32;
        let f1 = (i + 1) as f32 / n as f32;
        let c = match domain {
            CurveDomain::HueVs => {
                let rgb = filmcraft_color::hsl_to_rgb(f0, 0.82, 0.50);
                Color32::from_rgb((rgb[0].clamp(0.0, 1.0) * 255.0) as u8, (rgb[1].clamp(0.0, 1.0) * 255.0) as u8, (rgb[2].clamp(0.0, 1.0) * 255.0) as u8)
            }
            CurveDomain::LumVsSat => {
                let v = (f0 * 235.0 + 14.0).clamp(0.0, 255.0) as u8;
                Color32::from_rgb(v, v, v)
            }
            CurveDomain::SatVs => {
                let r = (105.0 + f0 * 140.0).clamp(0.0, 255.0) as u8;
                let g = (105.0 - f0 * 35.0).clamp(0.0, 255.0) as u8;
                let b = (110.0 + f0 * 60.0).clamp(0.0, 255.0) as u8;
                Color32::from_rgb(r, g, b)
            }
            CurveDomain::CustomRgb => continue,
        };
        p.rect_filled(Rect::from_min_max(pos2(area.min.x + area.width() * f0, y0), pos2(area.min.x + area.width() * f1 + 0.5, area.max.y)), 0.0, c);
    }
}

/// Curve editor: drag points (Shift = fine), click the curve to add, double-click or Delete/Backspace to remove.
/// Returns its rectangle (its automation element, `lumetri.curve.<param>`).
fn paint_curve_backdrop(p: &egui::Painter, area: Rect, domain: CurveDomain, hist: Option<&[[f32; 64]; 4]>, domain_hist: Option<&[f32; 64]>, hist_ch: usize) {
    p.rect_filled(area, 3.0, Color32::from_rgb(0x14, 0x15, 0x18));
    if domain == CurveDomain::CustomRgb {
        if let Some(ch_bins) = hist {
            let bin_w = area.width() / 64.0;
            let max_h = area.height() * 0.58;
            let paint_hist = |bins: &[f32; 64], hcol: Color32| {
                for (i, &val) in bins.iter().enumerate() {
                    if val > 0.004 {
                        let x0 = area.min.x + i as f32 * bin_w;
                        let bh = (val.clamp(0.0, 1.0) * max_h).max(1.0);
                        p.rect_filled(Rect::from_min_max(pos2(x0, area.max.y - bh), pos2(x0 + bin_w + 0.5, area.max.y)), 0.0, hcol);
                    }
                }
            };
            if hist_ch == 0 {
                paint_hist(&ch_bins[1], Color32::from_rgba_unmultiplied(220, 75, 75, 26));
                paint_hist(&ch_bins[2], Color32::from_rgba_unmultiplied(75, 200, 95, 26));
                paint_hist(&ch_bins[3], Color32::from_rgba_unmultiplied(80, 130, 245, 26));
                paint_hist(&ch_bins[0], Color32::from_rgba_unmultiplied(185, 195, 212, 40));
            } else if let Some(bins) = ch_bins.get(hist_ch) {
                let tint = match hist_ch {
                    1 => Color32::from_rgba_unmultiplied(230, 80, 80, 54),
                    2 => Color32::from_rgba_unmultiplied(80, 205, 95, 54),
                    3 => Color32::from_rgba_unmultiplied(85, 135, 250, 54),
                    _ => Color32::from_rgba_unmultiplied(165, 175, 195, 40),
                };
                paint_hist(bins, tint);
            }
        }
    } else if let Some(bins) = domain_hist {
        let bin_w = area.width() / 64.0;
        let plot_bottom = area.max.y - 7.0;
        let max_h = (area.height() - 10.0) * 0.68;
        for (i, &val) in bins.iter().enumerate() {
            if val > 0.004 {
                let f = i as f32 / 63.0;
                let x0 = area.min.x + i as f32 * bin_w;
                let bh = (val.clamp(0.0, 1.0) * max_h).max(1.5);
                let bar_y0 = plot_bottom - bh;
                let (fill, cap) = match domain {
                    CurveDomain::HueVs => {
                        let rgb = filmcraft_color::hsl_to_rgb(f, 0.78, 0.52);
                        let r = (rgb[0].clamp(0.0, 1.0) * 255.0) as u8;
                        let g = (rgb[1].clamp(0.0, 1.0) * 255.0) as u8;
                        let b = (rgb[2].clamp(0.0, 1.0) * 255.0) as u8;
                        (Color32::from_rgba_unmultiplied(r, g, b, 52), Color32::from_rgba_unmultiplied(r, g, b, 130))
                    }
                    CurveDomain::LumVsSat => {
                        let lum = (75.0 + f * 170.0).clamp(0.0, 255.0) as u8;
                        (Color32::from_rgba_unmultiplied(lum, lum, lum, 50), Color32::from_rgba_unmultiplied(lum, lum, lum, 125))
                    }
                    CurveDomain::SatVs => {
                        let r = (120.0 + f * 125.0).clamp(0.0, 255.0) as u8;
                        let g = (125.0 - f * 25.0).clamp(0.0, 255.0) as u8;
                        let b = (135.0 + f * 55.0).clamp(0.0, 255.0) as u8;
                        (Color32::from_rgba_unmultiplied(r, g, b, 50), Color32::from_rgba_unmultiplied(r, g, b, 125))
                    }
                    CurveDomain::CustomRgb => (Color32::TRANSPARENT, Color32::TRANSPARENT),
                };
                p.rect_filled(Rect::from_min_max(pos2(x0, bar_y0), pos2(x0 + bin_w + 0.5, plot_bottom)), 0.0, fill);
                p.line_segment([pos2(x0, bar_y0), pos2(x0 + bin_w, bar_y0)], Stroke::new(1.2, cap));
            }
        }
    }
    for i in 1..4 {
        let f = i as f32 / 4.0;
        p.line_segment(
            [pos2(area.min.x + area.width() * f, area.min.y), pos2(area.min.x + area.width() * f, area.max.y)],
            Stroke::new(1.0, Color32::from_rgb(0x28, 0x29, 0x2e)),
        );
        p.line_segment(
            [pos2(area.min.x, area.min.y + area.height() * f), pos2(area.max.x, area.min.y + area.height() * f)],
            Stroke::new(1.0, Color32::from_rgb(0x28, 0x29, 0x2e)),
        );
    }
    if domain == CurveDomain::CustomRgb {
        p.line_segment([pos2(area.min.x, area.max.y), pos2(area.max.x, area.min.y)], Stroke::new(1.0, Color32::from_rgb(0x36, 0x38, 0x40)));
    }
    p.rect_stroke(area, 3.0, Stroke::new(1.0, Color32::from_rgb(0x30, 0x32, 0x38)), egui::StrokeKind::Inside);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn curve_editor(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    id: &str,
    hue: bool,
    col: Color32,
    hist: Option<&[[f32; 64]; 4]>,
    domain_hist: Option<&[f32; 64]>,
    hist_ch: usize,
    overlay_channels: Option<&[(&str, Color32)]>,
    gang_rgb: bool,
    show_hist: bool,
    ctx: &mut ColorPanelCtx<'_>,
    mut side_extra: impl FnMut(&mut egui::Ui, &mut ColorPanelCtx<'_>),
) -> Rect {
    let t = ctx.t;
    let domain = CurveDomain::from_id(id, hue);
    let pts: Vec<[f32; 2]> = ctx.e.param(id).and_then(|p| p.value.as_curve().map(|c| c.to_vec())).unwrap_or_default();
    let avail_w = ui.available_width().max(1.0);
    // Use viewport height (scroll-invariant) so scrolling never enlarges the curve plot.
    let vp_h = ui.clip_rect().height().max(220.0);
    let h = if hue { (vp_h - 86.0).clamp(170.0, 340.0) } else { (vp_h - 86.0).clamp(175.0, 420.0) };
    let two_col = avail_w >= 460.0;
    let side_w = if two_col { (avail_w * 0.36).clamp(185.0, 290.0) } else { avail_w };
    let max_plot_w = if two_col { (avail_w - side_w - 14.0).max(200.0) } else { avail_w - 8.0 };
    let graph_w = if hue { max_plot_w.min((h * 2.35).max(320.0)) } else { max_plot_w.min((h * 1.55).max(240.0)) };

    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, id);
    let drag_key = egui::Id::unique(("curve-drag", scope));
    let sel_key = egui::Id::unique(("curve-sel", scope));
    let focus_key = egui::Id::unique(("curve-focus", scope));
    let span_key = egui::Id::unique(("curve-span", scope));
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_key));
    let mut selected_pt: Option<usize> = ui.data(|d| d.get_temp(sel_key));
    let mut has_focus: bool = ui.data(|d| d.get_temp(focus_key)).unwrap_or(false);
    let mut span_val: f32 = ui.data(|d| d.get_temp(span_key)).unwrap_or(0.075);
    let mut new_pts = pts.clone();
    let mut changed = false;
    let mut did_reset_or_delete = false;

    let top_y = ui.cursor().min.y;
    let left_x = ui.cursor().min.x;
    let mut area = Rect::NOTHING;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let (vbar_rect, _) = ui.allocate_exact_size(vec2(5.0, h), Sense::hover());
        paint_vertical_axis_bar(ui.painter(), vbar_rect, id, domain);
        let (plot_rect, resp) = ui.allocate_exact_size(vec2(graph_w, h), Sense::click_and_drag());
        area = plot_rect;
        let p = ui.painter();
        let hist_arg = if show_hist { hist } else { None };
        let dom_arg = if show_hist { domain_hist } else { None };
        paint_curve_backdrop(p, area, domain, hist_arg, dom_arg, hist_ch);
        let to_screen = |q: [f32; 2]| pos2(area.min.x + q[0] * area.width(), area.max.y - q[1] * area.height());
        let from_screen =
            |s: Pos2| [((s.x - area.min.x) / area.width().max(1.0)).clamp(0.0, 1.0), ((area.max.y - s.y) / area.height().max(1.0)).clamp(0.0, 1.0)];

        if hue {
            p.line_segment([to_screen([0.0, 0.5]), to_screen([1.0, 0.5])], Stroke::new(1.0, Color32::from_rgb(0x4b, 0x4d, 0x55)));
            let guide_col = t.text_faint;
            p.text(pos2(area.min.x + 5.0, area.min.y + 4.0), egui::Align2::LEFT_TOP, "+", crate::theme::Tokens::ui(10.0), guide_col);
            p.text(pos2(area.min.x + 5.0, area.max.y - 11.0), egui::Align2::LEFT_BOTTOM, "−", crate::theme::Tokens::ui(10.0), guide_col);
            paint_domain_bar(p, area, domain);
        }

        let draw_pts: Vec<[f32; 2]> = if hue && pts.is_empty() { vec![[0.0, 0.5], [1.0, 0.5]] } else { pts.clone() };
        if !hue && let Some(others) = overlay_channels {
            for &(oid, ocol) in others {
                if oid == id {
                    continue;
                }
                let opts: Vec<[f32; 2]> = ctx.e.param(oid).and_then(|p| p.value.as_curve().map(|c| c.to_vec())).unwrap_or_default();
                let olut = filmcraft_render::effects::curve_lut(&opts, 128);
                let oline: Vec<Pos2> = olut.iter().enumerate().map(|(i, y)| to_screen([i as f32 / 127.0, y.clamp(0.0, 1.0)])).collect();
                p.add(egui::Shape::line(oline, Stroke::new(1.0, ocol.gamma_multiply(0.35))));
            }
        }
        let lut = match (hue, id.starts_with("hue_vs_")) {
            (true, true) => filmcraft_render::effects::hue_lut(&pts, 128),
            (true, false) => filmcraft_render::effects::vs_sat_lut(&pts, 128),
            _ => None,
        }
        .unwrap_or_else(|| filmcraft_render::effects::curve_lut(&draw_pts, 128));
        let line: Vec<Pos2> = lut.iter().enumerate().map(|(i, y)| to_screen([i as f32 / 127.0, y.clamp(0.0, 1.0)])).collect();

        // Subtle delta shading on "Vs" curves relative to neutral center line
        if hue && line.len() >= 2 {
            let mid_y = to_screen([0.0, 0.5]).y;
            for i in 0..(line.len() - 1) {
                let a = line[i];
                let b = line[i + 1];
                if (a.y - mid_y).abs() > 0.7 || (b.y - mid_y).abs() > 0.7 {
                    let top = a.y.min(b.y).min(mid_y);
                    let bot = a.y.max(b.y).max(mid_y);
                    p.rect_filled(Rect::from_min_max(pos2(a.x, top), pos2(b.x, bot)), 0.0, Color32::from_rgba_unmultiplied(180, 200, 235, 18));
                }
            }
        }
        p.add(egui::Shape::line(line.clone(), Stroke::new(1.6, col)));

        let nearest = |pos: Pos2| {
            pts.iter().enumerate().map(|(i, q)| (i, to_screen(*q).distance(pos))).filter(|(_, d)| *d < 9.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
        };
        if ui.input(|inp| inp.pointer.any_pressed()) && !resp.hovered() {
            has_focus = false;
        }
        for (i, q) in pts.iter().enumerate() {
            let sp = to_screen(*q);
            let is_active = dragging == Some(i) || selected_pt == Some(i);
            p.circle_filled(sp, 4.0, if is_active { t.hot_text } else { col });
            if selected_pt == Some(i) {
                p.circle_stroke(sp, 6.5, Stroke::new(1.0, t.hot_text));
            }
        }
        if resp.drag_started()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            has_focus = true;
            let near = nearest(pos);
            dragging = near.or_else(|| pick_or_insert_curve_point(&mut new_pts, from_screen(pos), &mut changed));
            selected_pt = dragging;
        }
        if resp.dragged()
            && let Some(i) = dragging
            && let Some(cur_pt) = new_pts.get(i).copied()
        {
            let shift = ui.ctx().input(|inp| inp.modifiers.shift);
            let next = if shift {
                let d = resp.drag_delta() * 0.25;
                [(cur_pt[0] + d.x / area.width().max(1.0)).clamp(0.0, 1.0), (cur_pt[1] - d.y / area.height().max(1.0)).clamp(0.0, 1.0)]
            } else if let Some(pos) = resp.interact_pointer_pos() {
                from_screen(pos)
            } else {
                cur_pt
            };
            let min_x = i.checked_sub(1).and_then(|j| new_pts.get(j)).map(|q| (q[0] + 0.002).min(1.0)).unwrap_or(0.0);
            let max_x = new_pts.get(i + 1).map(|q| (q[0] - 0.002).max(0.0)).unwrap_or(1.0);
            let next = [next[0].clamp(min_x, max_x.max(min_x)), next[1].clamp(0.0, 1.0)];
            if (next[0] - cur_pt[0]).abs() > 1e-5 || (next[1] - cur_pt[1]).abs() > 1e-5 {
                new_pts[i] = next;
                changed = true;
            }
        }
        if resp.drag_stopped() {
            dragging = None;
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            has_focus = true;
            dragging = None;
            match nearest(pos) {
                Some(i) => {
                    selected_pt = Some(i);
                }
                None if line.iter().any(|pt| pt.distance(pos) < 9.0) => {
                    selected_pt = pick_or_insert_curve_point(&mut new_pts, from_screen(pos), &mut changed);
                }
                _ => {
                    selected_pt = None;
                }
            }
        }
        if resp.double_clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && let Some(i) = nearest(pos)
        {
            has_focus = true;
            if let Some(rest) = without_point(&pts, i, hue) {
                new_pts = rest;
                selected_pt = None;
                dragging = None;
                changed = true;
                did_reset_or_delete = true;
            }
        } else if resp.double_clicked() && !hue {
            has_focus = true;
            new_pts = vec![[0.0, 0.0], [1.0, 1.0]];
            selected_pt = None;
            dragging = None;
            changed = true;
            did_reset_or_delete = true;
        }
        if has_focus
            && let Some(i) = selected_pt
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|inp| inp.key_pressed(egui::Key::Delete) || inp.key_pressed(egui::Key::Backspace))
            && let Some(rest) = without_point(&new_pts, i, hue)
        {
            new_pts = rest;
            selected_pt = None;
            dragging = None;
            changed = true;
            did_reset_or_delete = true;
        }
        if let Some(active_i) = dragging.or(selected_pt)
            && let Some(q) = new_pts.get(active_i)
        {
            p.text(
                pos2(area.min.x + 14.0, area.min.y + 4.0),
                egui::Align2::LEFT_TOP,
                format_curve_readout(id, hue, ctx, *q),
                crate::theme::Tokens::ui(9.5),
                t.text_dim,
            );
        }
    });

    let mut draw_inspector = |ui: &mut egui::Ui| {
        egui::Frame::new()
            .fill(Color32::from_rgb(0x19, 0x1a, 0x1f))
            .stroke(Stroke::new(1.0, Color32::from_rgb(0x2b, 0x2d, 0x35)))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width((side_w - 20.0).max(150.0));
                let active_idx = dragging.or(selected_pt);

                // Point inspector header + prev/next/delete
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!("Point")).strong().small().color(t.text_dim));
                    let len = new_pts.len();
                    let prev_b = ui.add_enabled(len > 0, egui::Button::new(egui::RichText::new("◀").small()).frame(false));
                    if prev_b.clicked() && len > 0 {
                        selected_pt = Some(active_idx.map(|i| if i == 0 { len - 1 } else { i - 1 }).unwrap_or(0));
                    }
                    let next_b = ui.add_enabled(len > 0, egui::Button::new(egui::RichText::new("▶").small()).frame(false));
                    if next_b.clicked() && len > 0 {
                        selected_pt = Some(active_idx.map(|i| (i + 1) % len).unwrap_or(0));
                    }
                    if let Some(i) = active_idx
                        && let Some(rest) = without_point(&new_pts, i, hue)
                    {
                        let del_b = ui.small_button("×").on_hover_text(tl!("Delete point"));
                        if del_b.clicked() {
                            new_pts = rest;
                            selected_pt = None;
                            dragging = None;
                            changed = true;
                            did_reset_or_delete = true;
                        }
                    }
                });

                if let Some(i) = dragging.or(selected_pt)
                    && let Some(cur_pt) = new_pts.get(i).copied()
                {
                    let (x_scale, x_unit, x_dec, y_scale, y_offset, y_unit, y_dec) = curve_axis_units(id, hue, ctx);
                    let min_x = i.checked_sub(1).and_then(|j| new_pts.get(j)).map(|q| (q[0] + 0.002).min(1.0)).unwrap_or(0.0) as f64;
                    let max_x = new_pts.get(i + 1).map(|q| (q[0] - 0.002).max(0.0)).unwrap_or(1.0) as f64;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("X").size(10.0).color(t.text_dim));
                        let (rx, nx) = crate::widgets::hot_number(
                            ui,
                            "Curve X",
                            egui::Id::unique(("curve-num-x", scope, i)),
                            cur_pt[0] as f64 * x_scale,
                            crate::widgets::HotNumberOptions::new(x_scale / 200.0, (min_x * x_scale, max_x.max(min_x) * x_scale), x_dec, x_unit, t),
                        );
                        ctx.auto_rows.push(("lumetri.curve.pt_x".into(), rx.rect, "Curve Point X"));
                        if let Some(vx) = nx {
                            let norm_x = (vx / x_scale).clamp(min_x, max_x.max(min_x)) as f32;
                            if (norm_x - cur_pt[0]).abs() > 1e-4 {
                                new_pts[i][0] = norm_x;
                                changed = true;
                                did_reset_or_delete = true;
                            }
                        }
                        ui.label(egui::RichText::new("Y").size(10.0).color(t.text_dim));
                        let y_lo = (0.0 - y_offset) * y_scale;
                        let y_hi = (1.0 - y_offset) * y_scale;
                        let (ry, ny) = crate::widgets::hot_number(
                            ui,
                            "Curve Y",
                            egui::Id::unique(("curve-num-y", scope, i)),
                            (cur_pt[1] as f64 - y_offset) * y_scale,
                            crate::widgets::HotNumberOptions::new(y_scale / 200.0, (y_lo, y_hi), y_dec, y_unit, t),
                        );
                        ctx.auto_rows.push(("lumetri.curve.pt_y".into(), ry.rect, "Curve Point Y"));
                        if let Some(vy) = ny {
                            let norm_y = (vy / y_scale + y_offset).clamp(0.0, 1.0) as f32;
                            if (norm_y - cur_pt[1]).abs() > 1e-4 {
                                new_pts[i][1] = norm_y;
                                changed = true;
                                did_reset_or_delete = true;
                            }
                        }
                    });
                } else {
                    ui.label(egui::RichText::new(tl!("Click curve to add or select a point")).size(10.0).color(t.text_faint));
                }

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                if !hue {
                    ui.label(egui::RichText::new(tl!("Presets")).strong().small().color(t.text_dim));
                    let presets: &[(&str, &str, &[[f32; 2]])] = &[
                        ("linear", "Linear", &[[0.0, 0.0], [1.0, 1.0]]),
                        ("soft_s", "Soft S", &[[0.0, 0.0], [0.25, 0.21], [0.75, 0.79], [1.0, 1.0]]),
                        ("strong_s", "Strong S", &[[0.0, 0.0], [0.25, 0.16], [0.75, 0.84], [1.0, 1.0]]),
                        ("matte", "Matte", &[[0.0, 0.08], [0.25, 0.27], [0.75, 0.76], [1.0, 0.96]]),
                    ];
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                        for &(pid, plbl, ppts) in presets {
                            let b = ui.button(egui::RichText::new(crate::i18n::t(plbl)).small());
                            ctx.auto_rows.push((format!("lumetri.curve.preset.{pid}"), b.rect, plbl));
                            if b.clicked() {
                                new_pts = ppts.to_vec();
                                changed = true;
                                did_reset_or_delete = true;
                                selected_pt = None;
                                dragging = None;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    side_extra(ui, ctx);
                } else if domain == CurveDomain::HueVs {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tl!("Hue Vectors")).strong().small().color(t.text_dim));
                        for &(slbl, s) in &[("Narrow", 0.045f32), ("Normal", 0.075), ("Wide", 0.12)] {
                            if ui.selectable_label((span_val - s).abs() < 0.01, egui::RichText::new(crate::i18n::t(slbl)).size(9.5)).clicked() {
                                span_val = s;
                            }
                        }
                    });
                    const SWATCHES: [(&str, f32, [u8; 3]); 6] = [
                        ("Red", 0.0, [225, 65, 65]),
                        ("Yellow", 1.0 / 6.0, [225, 200, 55]),
                        ("Green", 2.0 / 6.0, [65, 205, 90]),
                        ("Cyan", 3.0 / 6.0, [55, 205, 220]),
                        ("Blue", 4.0 / 6.0, [75, 125, 240]),
                        ("Magenta", 5.0 / 6.0, [215, 75, 205]),
                    ];
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(5.0, 4.0);
                        for (si, (slbl, hx, [sr, sg, sb])) in SWATCHES.iter().enumerate() {
                            let fill = Color32::from_rgb(*sr, *sg, *sb);
                            let (srect, sresp) = ui.allocate_exact_size(vec2(52.0, 20.0), Sense::click());
                            ui.painter().rect_filled(srect, 3.0, Color32::from_rgb(0x22, 0x24, 0x2b));
                            ui.painter().circle_filled(pos2(srect.min.x + 10.0, srect.center().y), 4.5, fill);
                            ui.painter().text(
                                pos2(srect.min.x + 18.0, srect.center().y),
                                egui::Align2::LEFT_CENTER,
                                crate::i18n::t(slbl),
                                crate::theme::Tokens::ui(10.0),
                                t.text,
                            );
                            ctx.auto_rows.push((format!("lumetri.curve.swatch.{si}"), srect, slbl));
                            if sresp.on_hover_text(tl!("Add Hue Control Point")).clicked() {
                                let add_anchor = |pts: &mut Vec<[f32; 2]>, x: f32| {
                                    let xc = x.clamp(0.0, 1.0);
                                    if !pts.iter().any(|p| (p[0] - xc).abs() < 0.025) {
                                        pts.push([xc, 0.5]);
                                    }
                                };
                                if *hx > span_val {
                                    add_anchor(&mut new_pts, *hx - span_val);
                                }
                                add_anchor(&mut new_pts, *hx);
                                if *hx < 1.0 - span_val {
                                    add_anchor(&mut new_pts, *hx + span_val);
                                }
                                new_pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                                selected_pt =
                                    new_pts.iter().enumerate().min_by(|a, b| (a.1[0] - *hx).abs().total_cmp(&(b.1[0] - *hx).abs())).map(|(idx, _)| idx);
                                changed = true;
                                did_reset_or_delete = true;
                            }
                        }
                    });
                } else {
                    ui.label(egui::RichText::new(tl!("Quick Anchors")).strong().small().color(t.text_dim));
                    let anchors: &[(&str, f32)] = if domain == CurveDomain::LumVsSat {
                        &[("Shadows", 0.15), ("Midtones", 0.50), ("Highlights", 0.85)]
                    } else {
                        &[("Low", 0.20), ("Mid", 0.50), ("High", 0.80)]
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                        for (ai, &(albl, ax)) in anchors.iter().enumerate() {
                            let b = ui.small_button(crate::i18n::t(albl));
                            ctx.auto_rows.push((format!("lumetri.curve.anchor.{ai}"), b.rect, albl));
                            if b.clicked() {
                                if !new_pts.iter().any(|p| (p[0] - ax).abs() < 0.025) {
                                    new_pts.push([ax, 0.5]);
                                    new_pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                                }
                                selected_pt = new_pts.iter().enumerate().min_by(|a, b| (a.1[0] - ax).abs().total_cmp(&(b.1[0] - ax).abs())).map(|(idx, _)| idx);
                                changed = true;
                                did_reset_or_delete = true;
                            }
                        }
                    });
                    if domain == CurveDomain::LumVsSat {
                        ui.add_space(4.0);
                        if ui.small_button(tl!("Desat Shadows & Highlights")).clicked() {
                            new_pts = vec![[0.0, 0.25], [0.20, 0.50], [0.80, 0.50], [1.0, 0.30]];
                            changed = true;
                            did_reset_or_delete = true;
                        }
                    } else {
                        ui.add_space(4.0);
                        if ui.small_button(tl!("Soft Boost & Tame Peaks")).clicked() {
                            new_pts = vec![[0.0, 0.50], [0.35, 0.62], [0.75, 0.50], [1.0, 0.38]];
                            changed = true;
                            did_reset_or_delete = true;
                        }
                    }
                }
            });
    };

    if two_col {
        let insp_rect = Rect::from_min_size(pos2(left_x + graph_w + 16.0, top_y), vec2(side_w, h.max(240.0)));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(insp_rect).id_salt(("curve-insp", scope)));
        child.set_clip_rect(insp_rect.intersect(ui.clip_rect()));
        draw_inspector(&mut child);
        let insp_bottom = child.cursor().min.y;
        if insp_bottom > ui.cursor().min.y {
            ui.add_space(insp_bottom - ui.cursor().min.y);
        }
    } else {
        ui.add_space(4.0);
        draw_inspector(ui);
    }

    ui.data_mut(|d| {
        if let Some(i) = dragging {
            d.insert_temp(drag_key, i);
        } else {
            d.remove::<usize>(drag_key);
        }
        if let Some(i) = selected_pt {
            d.insert_temp(sel_key, i);
        } else {
            d.remove::<usize>(sel_key);
        }
        d.insert_temp(focus_key, has_focus);
        d.insert_temp(span_key, span_val);
    });

    if changed {
        let merge = ui.input(|i| i.pointer.any_down()) && !did_reset_or_delete;
        emit_curve_change(id, hue, gang_rgb, &new_pts, merge, ctx);
    }
    ui.add_space(4.0);
    area
}

fn emit_curve_change(id: &str, hue: bool, gang_rgb: bool, pts: &[[f32; 2]], merge: bool, ctx: &mut ColorPanelCtx<'_>) {
    let v: Vec<Value> = pts.iter().map(|q| json!([q[0], q[1]])).collect();
    if !hue && gang_rgb && matches!(id, "curve_red" | "curve_green" | "curve_blue") {
        for cid in ["curve_red", "curve_green", "curve_blue"] {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": cid, "value": v, "merge": merge}));
        }
    } else {
        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id, "value": v, "merge": merge}));
    }
}

fn curve_axis_units(id: &str, hue: bool, ctx: &ColorPanelCtx<'_>) -> (f64, &'static str, usize, f64, f64, &'static str, usize) {
    if !hue {
        if ctx.hdr && id == "curve_luma" {
            let nits = ctx.e.param("curves_hdr_range").map(|p| p.f64_at(ctx.mt)).unwrap_or(1000.0);
            return (nits, "nits", 0, nits, 0.0, "nits", 0);
        }
        return (100.0, "%", 0, 100.0, 0.0, "%", 0);
    }
    match id {
        "hue_vs_hue" => (360.0, "°", 0, 360.0, 0.5, "°", 0),
        "hue_vs_sat" | "hue_vs_luma" => (360.0, "°", 0, 200.0, 0.5, "%", 0),
        "sat_vs_luma" => (100.0, "%", 0, 100.0, 0.5, "%", 0),
        _ => (100.0, "%", 0, 200.0, 0.5, "%", 0),
    }
}

fn format_curve_readout(id: &str, hue: bool, ctx: &ColorPanelCtx<'_>, q: [f32; 2]) -> String {
    let (xs, xu, xd, ys, yo, yu, yd) = curve_axis_units(id, hue, ctx);
    let xv = q[0] as f64 * xs;
    let yv = (q[1] as f64 - yo) * ys;
    let y_prefix = if hue && yv > 0.0 { "+" } else { "" };
    format!("X:{xv:.xd$}{xu} Y:{y_prefix}{yv:.yd$}{yu}")
}

/// Pick an existing control point near `np[0]` (within `0.004` in normalized X) instead of
/// stacking duplicate points at the same X coordinate (e.g. along `X = 0.0` or `X = 1.0`).
fn pick_or_insert_curve_point(pts: &mut Vec<[f32; 2]>, np: [f32; 2], changed: &mut bool) -> Option<usize> {
    if let Some((idx, _)) =
        pts.iter().enumerate().filter(|(_, q)| (q[0] - np[0]).abs() < 0.004).min_by(|(_, a), (_, b)| (a[1] - np[1]).abs().total_cmp(&(b[1] - np[1]).abs()))
    {
        return Some(idx);
    }
    pts.push(np);
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    *changed = true;
    pts.iter().position(|q| *q == np)
}

/// `pts` without point `i` (double-click), or `None` for a point that stays: the leftmost and
/// rightmost points of an RGB curve.
pub(super) fn without_point(pts: &[[f32; 2]], i: usize, hue: bool) -> Option<Vec<[f32; 2]>> {
    let q = pts.get(i)?;
    let (lo, hi) = pts.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    if !hue && (q[0] <= lo || q[0] >= hi) {
        return None;
    }
    let mut rest = pts.to_vec();
    rest.remove(i);
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::{pick_or_insert_curve_point, without_point};

    #[test]
    fn a_curve_point_is_removed_by_position_not_by_list_order() {
        let pts = [[0.0, 0.0], [1.0, 1.0], [0.5, 0.7]];
        assert_eq!(without_point(&pts, 2, false), Some(vec![[0.0, 0.0], [1.0, 1.0]]), "the newest point can go");
        assert_eq!(without_point(&pts, 1, false), None, "the right end stays");
        assert_eq!(without_point(&pts, 0, false), None, "the left end stays");
        assert_eq!(without_point(&pts, 0, true), Some(vec![[1.0, 1.0], [0.5, 0.7]]));
        assert_eq!(without_point(&pts, 7, false), None);
    }

    #[test]
    fn clicking_near_existing_x_picks_point_without_stacking_duplicates() {
        let mut pts = vec![[0.0, 0.0], [1.0, 1.0]];
        let mut changed = false;
        let idx = pick_or_insert_curve_point(&mut pts, [1.0, 0.38], &mut changed);
        assert!(!changed);
        assert_eq!(idx, Some(1));
        assert_eq!(pts.len(), 2);

        let idx2 = pick_or_insert_curve_point(&mut pts, [0.5, 0.75], &mut changed);
        assert!(changed);
        assert_eq!(idx2, Some(1));
        assert_eq!(pts, vec![[0.0, 0.0], [0.5, 0.75], [1.0, 1.0]]);
    }
}
