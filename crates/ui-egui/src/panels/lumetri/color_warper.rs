use egui::{Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use filmcraft_render::effects::{CW_GRID_LEN, CW_HUE_SECTORS, CW_RADIAL_RINGS};
use serde_json::{Value, json};

use super::ColorPanelCtx;
use super::wheels::paint_reset_icon_button;

/// DaVinci Resolve-style Color Warper: Hue-Saturation Spiderweb + Chroma-Luma 2D grid.
pub(super) fn show(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let mode_key = egui::Id::unique(("lumetri-cw-mode", current_panel, ctx.is_secondary));
    let mut mode: usize = ui.data(|d| d.get_temp(mode_key)).unwrap_or(0);

    ui.horizontal_wrapped(|ui| {
        let r0 = ui.selectable_label(mode == 0, egui::RichText::new(tl!("Hue-Sat")).small());
        ctx.auto_rows.push(("lumetri.warper.mode.hue_sat".into(), r0.rect, "Hue-Sat Spiderweb"));
        if r0.clicked() {
            mode = 0;
        }
        let r1 = ui.selectable_label(mode == 1, egui::RichText::new(tl!("Chroma-Luma")).small());
        ctx.auto_rows.push(("lumetri.warper.mode.chroma_luma".into(), r1.rect, "Chroma-Luma Grid"));
        if r1.clicked() {
            mode = 1;
        }
        let param_id = if mode == 0 { "cw_hs_grid" } else { "cw_cl_grid" };
        let rst = paint_reset_icon_button(ui, t.text_dim, tl!("Reset grid"));
        ctx.auto_rows.push(("lumetri.warper.reset".into(), rst.rect, "Reset Color Warper grid"));
        if rst.clicked() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": param_id, "value": [], "merge": false}));
        }
        ui.label(egui::RichText::new(tl!("Drag nodes (Shift = fine, Double-click = reset)")).size(9.5).color(t.text_faint));
    });
    ui.data_mut(|d| d.insert_temp(mode_key, mode));
    ui.add_space(4.0);

    if mode == 0 {
        hue_sat_spiderweb(ui, current_panel, ctx);
    } else {
        chroma_luma_grid(ui, current_panel, ctx);
    }
}

fn read_grid(ctx: &ColorPanelCtx<'_>, param: &str) -> Vec<[f32; 2]> {
    let mut pts = ctx.e.param(param).and_then(|p| p.value.as_curve()).map(|c| c.to_vec()).unwrap_or_default();
    if pts.len() < CW_GRID_LEN {
        pts.resize(CW_GRID_LEN, [0.0, 0.0]);
    } else if pts.len() > CW_GRID_LEN {
        pts.truncate(CW_GRID_LEN);
    }
    pts
}

fn emit_grid(ctx: &mut ColorPanelCtx<'_>, param: &str, pts: &[[f32; 2]], merge: bool) {
    let is_zero = pts.iter().all(|p| p[0].abs() <= 1e-5 && p[1].abs() <= 1e-5);
    let val: Vec<Value> = if is_zero { Vec::new() } else { pts.iter().map(|p| json!([p[0], p[1]])).collect() };
    ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": param, "value": val, "merge": merge}));
}

fn hue_sat_spiderweb(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let mut pts = read_grid(ctx, "cw_hs_grid");
    let w = ui.available_width().max(120.0);
    let avail_h = (ui.clip_rect().max.y - ui.cursor().min.y - 16.0).clamp(140.0, 360.0);
    let size = w.min(avail_h);
    let (area, resp) = ui.allocate_exact_size(vec2(w, size), Sense::click_and_drag());
    let p = ui.painter();
    let center = area.center();
    let radius = (size * 0.5 - 12.0).max(40.0);

    p.rect_filled(area, 4.0, Color32::from_rgb(18, 20, 25));
    // Outer hue ring guide
    let ring_n = 72;
    for i in 0..ring_n {
        let f0 = i as f32 / ring_n as f32;
        let a0 = f0 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / ring_n as f32 * std::f32::consts::TAU;
        let rgb = filmcraft_color::hsl_to_rgb(f0, 0.8, 0.52);
        let col = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
        p.line_segment(
            [center + vec2(a0.cos(), -a0.sin()) * (radius + 3.0), center + vec2(a1.cos(), -a1.sin()) * (radius + 3.0)],
            Stroke::new(2.5, col.gamma_multiply(0.7)),
        );
    }

    // Rest rings & spokes (faint reference)
    for ri in 1..=CW_RADIAL_RINGS {
        let rr = radius * (ri as f32 / CW_RADIAL_RINGS as f32);
        p.circle_stroke(center, rr, Stroke::new(0.8, Color32::from_rgb(36, 40, 48)));
    }
    for hi in 0..CW_HUE_SECTORS {
        let a = hi as f32 / CW_HUE_SECTORS as f32 * std::f32::consts::TAU;
        p.line_segment([center, center + vec2(a.cos(), -a.sin()) * radius], Stroke::new(0.8, Color32::from_rgb(36, 40, 48)));
    }

    let node_pos = |pts: &[[f32; 2]], hi: usize, ri: usize| -> Pos2 {
        let a = (hi % CW_HUE_SECTORS) as f32 / CW_HUE_SECTORS as f32 * std::f32::consts::TAU;
        let r = (ri + 1) as f32 / CW_RADIAL_RINGS as f32;
        let d = pts[ri * CW_HUE_SECTORS + (hi % CW_HUE_SECTORS)];
        center + vec2(r * a.cos() + d[0], -(r * a.sin() + d[1])) * radius
    };

    // Warped mesh lines
    let mesh_stroke = Stroke::new(1.1, Color32::from_rgb(130, 145, 168));
    for ri in 0..CW_RADIAL_RINGS {
        for hi in 0..CW_HUE_SECTORS {
            let p0 = node_pos(&pts, hi, ri);
            let p_next_h = node_pos(&pts, (hi + 1) % CW_HUE_SECTORS, ri);
            p.line_segment([p0, p_next_h], mesh_stroke);
            if ri == 0 {
                p.line_segment([center, p0], mesh_stroke);
            } else {
                let p_prev_r = node_pos(&pts, hi, ri - 1);
                p.line_segment([p_prev_r, p0], mesh_stroke);
            }
        }
    }

    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, "cw_hs");
    let drag_key = egui::Id::unique(("cw-drag", scope));
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_key));

    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        dragging = (0..CW_GRID_LEN)
            .map(|i| {
                let (ri, hi) = (i / CW_HUE_SECTORS, i % CW_HUE_SECTORS);
                (i, node_pos(&pts, hi, ri).distance(pos))
            })
            .filter(|(_, dist)| *dist < 14.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0);
    }

    let mut changed = false;
    let mut merge = false;
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(i) = (0..CW_GRID_LEN)
            .map(|i| {
                let (ri, hi) = (i / CW_HUE_SECTORS, i % CW_HUE_SECTORS);
                (i, node_pos(&pts, hi, ri).distance(pos))
            })
            .filter(|(_, dist)| *dist < 14.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0)
    {
        pts[i] = [0.0, 0.0];
        changed = true;
    } else if resp.dragged()
        && let Some(i) = dragging
    {
        let shift = ui.ctx().input(|inp| inp.modifiers.shift);
        let factor = if shift { 0.25 } else { 1.0 };
        let delta = resp.drag_delta() * factor / radius;
        pts[i][0] = (pts[i][0] + delta.x).clamp(-0.6, 0.6);
        pts[i][1] = (pts[i][1] - delta.y).clamp(-0.6, 0.6);
        changed = true;
        merge = true;
    }
    if resp.drag_stopped() {
        dragging = None;
    }
    ui.data_mut(|d| {
        if let Some(i) = dragging {
            d.insert_temp(drag_key, i);
        } else {
            d.remove::<usize>(drag_key);
        }
    });

    // Draw control dots
    p.circle_filled(center, 3.0, Color32::from_rgb(160, 165, 175));
    for i in 0..CW_GRID_LEN {
        let (ri, hi) = (i / CW_HUE_SECTORS, i % CW_HUE_SECTORS);
        let sp = node_pos(&pts, hi, ri);
        let h_norm = hi as f32 / CW_HUE_SECTORS as f32;
        let s_norm = (ri + 1) as f32 / CW_RADIAL_RINGS as f32;
        let rgb = filmcraft_color::hsl_to_rgb(h_norm, s_norm * 0.85, 0.55);
        let dot_col = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
        let is_active = dragging == Some(i);
        p.circle_filled(sp, if is_active { 5.0 } else { 3.5 }, dot_col);
        if is_active || pts[i][0].abs() > 1e-3 || pts[i][1].abs() > 1e-3 {
            p.circle_stroke(sp, 5.5, Stroke::new(1.2, if is_active { t.hot_text } else { Color32::WHITE }));
        }
    }

    if changed {
        emit_grid(ctx, "cw_hs_grid", &pts, merge);
    }
    ctx.auto_rows.push(("lumetri.warper.hs_grid".into(), area, "Hue-Sat Spiderweb"));
}

fn chroma_luma_grid(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let mut pts = read_grid(ctx, "cw_cl_grid");
    let w = ui.available_width().max(140.0);
    let h = (w * 0.55).clamp(140.0, 260.0);
    let (area, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
    let p = ui.painter();
    let pad = 12.0;
    let grid_r = area.shrink(pad);
    p.rect_filled(area, 4.0, Color32::from_rgb(18, 20, 25));

    // Hue x Luma background strip hints
    for hi in 0..CW_HUE_SECTORS {
        let x0 = grid_r.min.x + grid_r.width() * (hi as f32 / CW_HUE_SECTORS as f32);
        let x1 = grid_r.min.x + grid_r.width() * ((hi + 1) as f32 / CW_HUE_SECTORS as f32);
        let rgb = filmcraft_color::hsl_to_rgb(hi as f32 / CW_HUE_SECTORS as f32, 0.75, 0.45);
        let c = Color32::from_rgba_unmultiplied((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8, 28);
        p.rect_filled(Rect::from_min_max(pos2(x0, grid_r.min.y), pos2(x1, grid_r.max.y)), 0.0, c);
    }

    let node_pos = |pts: &[[f32; 2]], hi: usize, ri: usize| -> Pos2 {
        let rest_x = (hi as f32 + 0.5) / CW_HUE_SECTORS as f32;
        let rest_y = (ri + 1) as f32 / 5.0; // luma 0.2, 0.4, 0.6, 0.8
        let d = pts[ri * CW_HUE_SECTORS + hi];
        let x = grid_r.min.x + (rest_x + d[0] * 0.12) * grid_r.width();
        let y = grid_r.max.y - (rest_y + d[1] * 0.22).clamp(0.02, 0.98) * grid_r.height();
        pos2(x, y)
    };

    let stroke = Stroke::new(1.0, Color32::from_rgb(125, 140, 165));
    for ri in 0..CW_RADIAL_RINGS {
        for hi in 0..CW_HUE_SECTORS {
            let p0 = node_pos(&pts, hi, ri);
            if hi + 1 < CW_HUE_SECTORS {
                p.line_segment([p0, node_pos(&pts, hi + 1, ri)], stroke);
            }
            if ri + 1 < CW_RADIAL_RINGS {
                p.line_segment([p0, node_pos(&pts, hi, ri + 1)], stroke);
            }
        }
    }

    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, "cw_cl");
    let drag_key = egui::Id::unique(("cw-cl-drag", scope));
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_key));

    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        dragging = (0..CW_GRID_LEN)
            .map(|i| {
                let (ri, hi) = (i / CW_HUE_SECTORS, i % CW_HUE_SECTORS);
                (i, node_pos(&pts, hi, ri).distance(pos))
            })
            .filter(|(_, dist)| *dist < 14.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0);
    }

    let mut changed = false;
    let mut merge = false;
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(i) = (0..CW_GRID_LEN)
            .map(|i| {
                let (ri, hi) = (i / CW_HUE_SECTORS, i % CW_HUE_SECTORS);
                (i, node_pos(&pts, hi, ri).distance(pos))
            })
            .filter(|(_, dist)| *dist < 14.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0)
    {
        pts[i] = [0.0, 0.0];
        changed = true;
    } else if resp.dragged()
        && let Some(i) = dragging
    {
        let shift = ui.ctx().input(|inp| inp.modifiers.shift);
        let factor = if shift { 0.25 } else { 1.0 };
        let d = resp.drag_delta() * factor;
        pts[i][0] = (pts[i][0] + d.x / (grid_r.width() * 0.12).max(1.0)).clamp(-1.0, 1.0);
        pts[i][1] = (pts[i][1] - d.y / (grid_r.height() * 0.22).max(1.0)).clamp(-1.0, 1.0);
        changed = true;
        merge = true;
    }
    if resp.drag_stopped() {
        dragging = None;
    }
    ui.data_mut(|d| {
        if let Some(i) = dragging {
            d.insert_temp(drag_key, i);
        } else {
            d.remove::<usize>(drag_key);
        }
    });

    for i in 0..CW_GRID_LEN {
        let (ri, hi) = (i / CW_HUE_SECTORS, i % CW_HUE_SECTORS);
        let sp = node_pos(&pts, hi, ri);
        let rgb = filmcraft_color::hsl_to_rgb(hi as f32 / CW_HUE_SECTORS as f32, 0.75, (ri + 1) as f32 / 5.0);
        let dot_col = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
        let is_active = dragging == Some(i);
        p.circle_filled(sp, if is_active { 5.0 } else { 3.5 }, dot_col);
        if is_active || pts[i][0].abs() > 1e-3 || pts[i][1].abs() > 1e-3 {
            p.circle_stroke(sp, 5.5, Stroke::new(1.2, if is_active { t.hot_text } else { Color32::WHITE }));
        }
    }

    if changed {
        emit_grid(ctx, "cw_cl_grid", &pts, merge);
    }
    ctx.auto_rows.push(("lumetri.warper.cl_grid".into(), area, "Chroma-Luma Grid"));
}
