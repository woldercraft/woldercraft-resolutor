use egui::{Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use super::ColorPanelCtx;

/// Curve editor: drag points (Shift = fine), click the curve to add, double-click or Delete/Backspace to remove.
/// Returns its rectangle (its automation element, `lumetri.curve.<param>`).
pub(super) fn curve_editor(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    id: &str,
    hue: bool,
    col: Color32,
    hist: Option<&[f32; 64]>,
    ctx: &mut ColorPanelCtx<'_>,
) -> Rect {
    let t = ctx.t;
    let pts: Vec<[f32; 2]> = ctx.e.param(id).and_then(|p| p.value.as_curve().map(|c| c.to_vec())).unwrap_or_default();
    let w = ui.available_width().max(1.0);
    let avail_h = (ui.clip_rect().max.y - ui.cursor().min.y - 40.0).clamp(80.0, 420.0);
    let h = if hue { (w * 0.45).min(avail_h) } else { w.min(avail_h) };
    let (area, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
    let p = ui.painter();
    p.rect_filled(area, 2.0, Color32::from_rgb(0x16, 0x16, 0x16));
    if !hue && let Some(bins) = hist {
        let bin_w = area.width() / 64.0;
        let max_h = area.height() * 0.45;
        let hcol = Color32::from_rgba_unmultiplied(155, 165, 185, 32);
        for (i, &val) in bins.iter().enumerate() {
            if val > 0.005 {
                let x0 = area.min.x + i as f32 * bin_w;
                let bh = (val.clamp(0.0, 1.0) * max_h).max(1.0);
                p.rect_filled(Rect::from_min_max(pos2(x0, area.max.y - bh), pos2(x0 + bin_w + 0.5, area.max.y)), 0.0, hcol);
            }
        }
    }
    for i in 1..4 {
        let f = i as f32 / 4.0;
        p.line_segment(
            [pos2(area.min.x + area.width() * f, area.min.y), pos2(area.min.x + area.width() * f, area.max.y)],
            Stroke::new(1.0, Color32::from_rgb(0x2a, 0x2a, 0x2a)),
        );
        p.line_segment(
            [pos2(area.min.x, area.min.y + area.height() * f), pos2(area.max.x, area.min.y + area.height() * f)],
            Stroke::new(1.0, Color32::from_rgb(0x2a, 0x2a, 0x2a)),
        );
    }
    let to_screen = |q: [f32; 2]| pos2(area.min.x + q[0] * area.width(), area.max.y - q[1] * area.height());
    let from_screen = |s: Pos2| [((s.x - area.min.x) / area.width().max(1.0)).clamp(0.0, 1.0), ((area.max.y - s.y) / area.height().max(1.0)).clamp(0.0, 1.0)];

    if hue {
        // Neutral center reference line (no change at y = 0.5)
        p.line_segment([to_screen([0.0, 0.5]), to_screen([1.0, 0.5])], Stroke::new(1.0, Color32::from_rgb(0x45, 0x45, 0x4a)));
        let guide_col = t.text_faint;
        p.text(pos2(area.min.x + 5.0, area.min.y + 4.0), egui::Align2::LEFT_TOP, "+", crate::theme::Tokens::ui(10.0), guide_col);
        p.text(pos2(area.min.x + 5.0, area.max.y - 10.0), egui::Align2::LEFT_BOTTOM, "−", crate::theme::Tokens::ui(10.0), guide_col);
        let n = 36;
        for i in 0..n {
            let f0 = i as f32 / n as f32;
            let rgb = filmcraft_color::hsl_to_rgb(f0, 0.8, 0.5);
            let c = Color32::from_rgb((rgb[0].clamp(0.0, 1.0) * 255.0) as u8, (rgb[1].clamp(0.0, 1.0) * 255.0) as u8, (rgb[2].clamp(0.0, 1.0) * 255.0) as u8);
            p.rect_filled(
                Rect::from_min_max(
                    pos2(area.min.x + area.width() * f0, area.max.y - 6.0),
                    pos2(area.min.x + area.width() * (f0 + 1.0 / n as f32) + 0.5, area.max.y),
                ),
                0.0,
                c,
            );
        }
    }

    let draw_pts: Vec<[f32; 2]> = if hue && pts.is_empty() { vec![[0.0, 0.5], [1.0, 0.5]] } else { pts.clone() };
    let lut = match (hue, id.starts_with("hue_vs_")) {
        (true, true) => filmcraft_render::effects::hue_lut(&pts, 128),
        (true, false) => filmcraft_render::effects::vs_sat_lut(&pts, 128),
        _ => None,
    }
    .unwrap_or_else(|| filmcraft_render::effects::curve_lut(&draw_pts, 128));
    let line: Vec<Pos2> = lut.iter().enumerate().map(|(i, y)| to_screen([i as f32 / 127.0, y.clamp(0.0, 1.0)])).collect();
    p.add(egui::Shape::line(line.clone(), Stroke::new(1.5, col)));

    let nearest = |pos: Pos2| {
        pts.iter().enumerate().map(|(i, q)| (i, to_screen(*q).distance(pos))).filter(|(_, d)| *d < 9.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
    };
    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, id);
    let drag_key = egui::Id::unique(("curve-drag", scope));
    let sel_key = egui::Id::unique(("curve-sel", scope));
    let focus_key = egui::Id::unique(("curve-focus", scope));
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_key));
    let mut selected_pt: Option<usize> = ui.data(|d| d.get_temp(sel_key));
    let mut has_focus: bool = ui.data(|d| d.get_temp(focus_key)).unwrap_or(false);
    let mut new_pts = pts.clone();
    let mut changed = false;
    let mut did_reset_or_delete = false;

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
        dragging = near.or_else(|| {
            let np = from_screen(pos);
            new_pts.push(np);
            new_pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
            changed = true;
            new_pts.iter().position(|q| *q == np)
        });
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
                let np = from_screen(pos);
                new_pts.push(np);
                new_pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                selected_pt = new_pts.iter().position(|q| *q == np);
                changed = true;
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
    });

    // Direct numeric X/Y edit for selected point + quick curve presets for Custom RGB
    let active_idx = dragging.or(selected_pt);
    if active_idx.is_some() || !hue {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if let Some(i) = active_idx
                && let Some(cur_pt) = new_pts.get(i).copied()
            {
                let (x_scale, x_unit, x_dec, y_scale, y_offset, y_unit, y_dec) = curve_axis_units(id, hue, ctx);
                let min_x = i.checked_sub(1).and_then(|j| new_pts.get(j)).map(|q| (q[0] + 0.002).min(1.0)).unwrap_or(0.0) as f64;
                let max_x = new_pts.get(i + 1).map(|q| (q[0] - 0.002).max(0.0)).unwrap_or(1.0) as f64;
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
            }
            if !hue {
                let presets: &[(&str, &str, &[[f32; 2]])] = &[
                    ("linear", "Linear", &[[0.0, 0.0], [1.0, 1.0]]),
                    ("soft_s", "Soft S", &[[0.0, 0.0], [0.25, 0.21], [0.75, 0.79], [1.0, 1.0]]),
                    ("strong_s", "Strong S", &[[0.0, 0.0], [0.25, 0.16], [0.75, 0.84], [1.0, 1.0]]),
                    ("matte", "Matte", &[[0.0, 0.08], [0.25, 0.27], [0.75, 0.76], [1.0, 0.96]]),
                ];
                for &(pid, plbl, ppts) in presets {
                    let b = ui.button(egui::RichText::new(crate::i18n::t(plbl)).small());
                    ctx.auto_rows.push((format!("lumetri.curve.preset.{pid}"), b.rect, plbl));
                    if b.clicked() {
                        new_pts = ppts.to_vec();
                        changed = true;
                        did_reset_or_delete = true;
                        ui.data_mut(|d| {
                            d.remove::<usize>(drag_key);
                            d.remove::<usize>(sel_key);
                        });
                    }
                }
            }
        });
    }

    if changed {
        let v: Vec<Value> = new_pts.iter().map(|q| json!([q[0], q[1]])).collect();
        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id, "value": v, "merge": resp.dragged() && !did_reset_or_delete}));
    }
    ui.add_space(6.0);
    area
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
    use super::without_point;

    #[test]
    fn a_curve_point_is_removed_by_position_not_by_list_order() {
        let pts = [[0.0, 0.0], [1.0, 1.0], [0.5, 0.7]];
        assert_eq!(without_point(&pts, 2, false), Some(vec![[0.0, 0.0], [1.0, 1.0]]), "the newest point can go");
        assert_eq!(without_point(&pts, 1, false), None, "the right end stays");
        assert_eq!(without_point(&pts, 0, false), None, "the left end stays");
        assert_eq!(without_point(&pts, 0, true), Some(vec![[1.0, 1.0], [0.5, 0.7]]));
        assert_eq!(without_point(&pts, 7, false), None);
    }
}
