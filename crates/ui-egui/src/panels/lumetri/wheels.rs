use egui::{Color32, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::ColorPanelCtx;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WheelKind {
    Lift,
    Gamma,
    Gain,
    Offset,
}

/// DaVinci Resolve 4-Wheel layout: Lift (Shadows), Gamma (Midtones), Gain (Highlights), Offset (Temp/Tint/Exp).
pub(super) fn resolve_wheels_grid(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let cards = [
        (WheelKind::Lift, "Lift", tl!("Shadows")),
        (WheelKind::Gamma, "Gamma", tl!("Midtones")),
        (WheelKind::Gain, "Gain", tl!("Highlights")),
        (WheelKind::Offset, "Offset", tl!("Global")),
    ];
    let avail = ui.available_width().max(140.0);
    let gap = ui.spacing().item_spacing.x;
    if avail >= 640.0 {
        let col_w = ((avail - gap * 3.0) / 4.0).floor().max(80.0);
        ui.horizontal_top(|ui| {
            for (kind, title, sub) in cards {
                ui.allocate_ui_with_layout(vec2(col_w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(col_w);
                    resolve_wheel_card(ui, current_panel, kind, title, sub, ctx);
                });
            }
        });
    } else if avail >= 320.0 {
        let col_w = ((avail - gap) / 2.0).floor().max(80.0);
        for pair in cards.chunks(2) {
            ui.horizontal_top(|ui| {
                for &(kind, title, sub) in pair {
                    ui.allocate_ui_with_layout(vec2(col_w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_max_width(col_w);
                        resolve_wheel_card(ui, current_panel, kind, title, sub, ctx);
                    });
                }
            });
            ui.add_space(3.0);
        }
    } else {
        for (kind, title, sub) in cards {
            ui.allocate_ui_with_layout(vec2(avail, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.set_max_width(avail);
                resolve_wheel_card(ui, current_panel, kind, title, sub, ctx);
            });
            ui.add_space(3.0);
        }
    }
}

/// Vector-drawn circular reset arrow button (avoids selectable text glyph / I-beam cursor).
pub(super) fn paint_reset_icon_button(ui: &mut egui::Ui, color: Color32, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tooltip));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let c = rect.center();
        if resp.hovered() {
            p.circle_filled(c, 7.5, Color32::from_white_alpha(22));
        }
        let col = if resp.hovered() { Color32::WHITE } else { color };
        let r = 4.3_f32;
        let start = -2.35_f32;
        let end = 2.15_f32;
        let steps = 14;
        let pts: Vec<egui::Pos2> = (0..=steps)
            .map(|i| {
                let a = start + (end - start) * (i as f32 / steps as f32);
                pos2(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.35, col)));
        let tip = pos2(c.x + r * start.cos(), c.y + r * start.sin());
        p.line_segment([tip, tip + vec2(3.2, -0.4)], Stroke::new(1.35, col));
        p.line_segment([tip, tip + vec2(0.4, 3.2)], Stroke::new(1.35, col));
    }
    resp
}

/// Single DaVinci Resolve color wheel card: Header + Reset, Hue Trackball, Horizontal Knurled Master Wheel, and Live Readout.
fn resolve_wheel_card(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    kind: WheelKind,
    title: &'static str,
    subtitle: &str,
    ctx: &mut ColorPanelCtx<'_>,
) {
    let t = ctx.t;
    let e = ctx.e;
    let mt = ctx.mt;
    let clip = ctx.clip;
    let idx = ctx.idx;
    let (vx, vy, lum, lum_max, id_tag) = match kind {
        WheelKind::Lift => {
            let v = e.param("wheel_shadows").map(|p| p.vec2_at(mt)).unwrap_or_default();
            let l = e.param("wheel_shadows_l").map(|p| p.f64_at(mt)).unwrap_or(0.0) as f32;
            (v.x as f32, v.y as f32, l, 100.0_f32, "lift")
        }
        WheelKind::Gamma => {
            let v = e.param("wheel_midtones").map(|p| p.vec2_at(mt)).unwrap_or_default();
            let l = e.param("wheel_midtones_l").map(|p| p.f64_at(mt)).unwrap_or(0.0) as f32;
            (v.x as f32, v.y as f32, l, 100.0_f32, "gamma")
        }
        WheelKind::Gain => {
            let v = e.param("wheel_highlights").map(|p| p.vec2_at(mt)).unwrap_or_default();
            let l = e.param("wheel_highlights_l").map(|p| p.f64_at(mt)).unwrap_or(0.0) as f32;
            (v.x as f32, v.y as f32, l, 100.0_f32, "gain")
        }
        WheelKind::Offset => {
            let temp = e.param("temperature").map(|p| p.f64_at(mt)).unwrap_or(0.0) as f32;
            let tint = e.param("tint").map(|p| p.f64_at(mt)).unwrap_or(0.0) as f32;
            let exp = e.param("exposure").map(|p| p.f64_at(mt)).unwrap_or(0.0) as f32;
            ((temp / 100.0).clamp(-1.0, 1.0), (tint / 100.0).clamp(-1.0, 1.0), exp, 5.0_f32, "offset")
        }
    };

    let section_on = match kind {
        WheelKind::Offset => e.param("basic_on").and_then(|p| p.value.as_bool()).unwrap_or(true),
        _ => e.param("wheels_on").and_then(|p| p.value.as_bool()).unwrap_or(true),
    };
    let modified = vx.abs() > 1e-3 || vy.abs() > 1e-3 || lum.abs() > 1e-3;

    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(23, 26, 32))
        .stroke(Stroke::new(
            1.0,
            if !section_on {
                Color32::from_rgb(32, 35, 42)
            } else if modified {
                Color32::from_rgb(65, 85, 118)
            } else {
                Color32::from_rgb(38, 43, 52)
            },
        ))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(4, 2));

    let inner_max = (ui.available_width() - 10.0).max(60.0);
    let frame_resp = frame.show(ui, |ui| {
        ui.set_max_width(inner_max);
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.allocate_ui_with_layout(vec2(inner_max, 16.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_max_width(inner_max);
            let rst_col = if modified { t.accent } else { t.text_faint };
            let rst = paint_reset_icon_button(ui, rst_col, tl!("Reset this wheel (or double-click)"));
            if rst.clicked() {
                ctx.reset_wheels.push(id_tag);
            }
            let title_w = ui.available_width().max(24.0);
            ui.allocate_ui_with_layout(vec2(title_w, 16.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_max_width(title_w);
                let title_col = if !section_on {
                    t.text_faint
                } else if modified {
                    Color32::WHITE
                } else {
                    t.text
                };
                ui.label(egui::RichText::new(title).strong().size(11.0).color(title_col));
                let sub_txt = if section_on { format!("({subtitle})") } else { format!("({subtitle} · off)") };
                ui.add(egui::Label::new(egui::RichText::new(sub_txt).size(9.0).color(if section_on { t.text_dim } else { t.text_faint })).truncate());
            });
        });

        ui.add_space(1.0);

        let wheel_size = (ui.available_width() * 0.72).clamp(48.0, 120.0);
        let row_w = ui.available_width();
        let (area, resp) = ui.allocate_exact_size(vec2(row_w, wheel_size), Sense::click_and_drag());
        let c = area.center();
        let r_out = wheel_size / 2.0 - 2.0;
        let r_in = r_out - (wheel_size * 0.14).clamp(7.0, 12.0);
        let p = ui.painter();

        p.circle_stroke(c, r_out + 1.0, Stroke::new(1.5, Color32::from_rgb(14, 16, 20)));

        let ring = if kind == WheelKind::Offset { offset_ring_lut() } else { color_ring_lut() };
        let mut mesh = egui::Mesh::default();
        for win in ring.windows(2) {
            let (c0, s0, col) = win[0];
            let (c1, s1, _) = win[1];
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(c + vec2(c0, -s0) * r_in, col);
            mesh.colored_vertex(c + vec2(c0, -s0) * r_out, col);
            mesh.colored_vertex(c + vec2(c1, -s1) * r_out, col);
            mesh.colored_vertex(c + vec2(c1, -s1) * r_in, col);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        p.add(mesh);

        let dish_center_col = Color32::from_rgb(16, 18, 22);
        p.circle_filled(c, r_in - 1.0, dish_center_col);
        // Subtle radial color hint inside the trackball dish
        let mut dish_mesh = egui::Mesh::default();
        let center_idx = dish_mesh.vertices.len() as u32;
        dish_mesh.colored_vertex(c, dish_center_col);
        for (cos, sin, col) in ring {
            let rim = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 26);
            dish_mesh.colored_vertex(c + vec2(*cos, -*sin) * (r_in - 1.5), rim);
        }
        for i in 0..ring.len().saturating_sub(1) as u32 {
            dish_mesh.add_triangle(center_idx, center_idx + 1 + i, center_idx + 2 + i);
        }
        p.add(dish_mesh);
        p.circle_stroke(c, (r_in - 2.0) * 0.5, Stroke::new(0.8, Color32::from_rgb(32, 36, 44)));
        let ch_r = r_in - 5.0;
        p.line_segment([c - vec2(ch_r, 0.0), c + vec2(ch_r, 0.0)], Stroke::new(0.8, Color32::from_rgb(42, 47, 56)));
        p.line_segment([c - vec2(0.0, ch_r), c + vec2(0.0, ch_r)], Stroke::new(0.8, Color32::from_rgb(42, 47, 56)));
        p.circle_filled(c, 1.5, Color32::from_rgb(75, 82, 95));

        let dish_r = (r_in - 5.0).max(12.0);
        let puck_vec = vec2(vx, -vy);
        let puck_clamped = if puck_vec.length() > 1.0 { puck_vec.normalized() } else { puck_vec };
        let puck = c + puck_clamped * dish_r;
        if vx.abs() > 1e-3 || vy.abs() > 1e-3 {
            p.line_segment([c, puck], Stroke::new(1.0, Color32::from_rgba_unmultiplied(220, 225, 235, 110)));
        }

        let puck_mag = (vx * vx + vy * vy).sqrt().clamp(0.0, 1.0);
        let puck_hue = ((-vy).atan2(vx).rem_euclid(std::f32::consts::TAU)) / std::f32::consts::TAU;
        let prgb = filmcraft_color::hsl_to_rgb(1.0 - puck_hue, puck_mag * 0.85, 0.55);
        let puck_fill = if puck_mag > 0.02 {
            Color32::from_rgb((prgb[0].clamp(0.0, 1.0) * 255.0) as u8, (prgb[1].clamp(0.0, 1.0) * 255.0) as u8, (prgb[2].clamp(0.0, 1.0) * 255.0) as u8)
        } else {
            Color32::from_rgb(55, 60, 70)
        };
        p.circle_filled(puck, 4.5, puck_fill);
        p.circle_stroke(puck, 5.0, Stroke::new(1.5, Color32::WHITE));

        let emit_2d = |actions: &mut Vec<serde_json::Value>, nx: f32, ny: f32, merge: bool| match kind {
            WheelKind::Lift => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_shadows", "value": [nx, ny], "merge": merge})),
            WheelKind::Gamma => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_midtones", "value": [nx, ny], "merge": merge})),
            WheelKind::Gain => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_highlights", "value": [nx, ny], "merge": merge})),
            WheelKind::Offset => {
                let m = if merge { json!("wheel-offset") } else { json!(false) };
                actions.push(json!({"clip": clip.0, "effect": idx, "param": "temperature", "value": (nx * 100.0).clamp(-100.0, 100.0), "merge": m}));
                actions.push(json!({"clip": clip.0, "effect": idx, "param": "tint", "value": (ny * 100.0).clamp(-100.0, 100.0), "merge": m}));
            }
        };

        if resp.double_clicked() {
            if kind == WheelKind::Offset {
                ctx.reset_wheels.push("offset_2d");
            } else {
                emit_2d(ctx.actions, 0.0, 0.0, false);
            }
        } else if resp.dragged() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            if shift {
                let d = resp.drag_delta() * 0.25 / dish_r;
                let mut cur = vec2(vx + d.x, vy - d.y);
                if cur.length() > 1.0 {
                    cur = cur.normalized();
                }
                if (cur.x - vx).abs() > 1e-4 || (cur.y - vy).abs() > 1e-4 {
                    emit_2d(ctx.actions, cur.x, cur.y, true);
                }
            } else if let Some(pos) = resp.interact_pointer_pos() {
                let mut d = (pos - c) / dish_r;
                if d.length() > 1.0 {
                    d = d.normalized();
                }
                let nx = d.x;
                let ny = -d.y;
                if (nx - vx).abs() > 1e-4 || (ny - vy).abs() > 1e-4 {
                    emit_2d(ctx.actions, nx, ny, true);
                }
            }
        } else if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let mut d = (pos - c) / dish_r;
            if d.length() > 1.0 {
                d = d.normalized();
            }
            emit_2d(ctx.actions, d.x, -d.y, false);
        }

        ui.add_space(1.0);

        let (track, lr) = ui.allocate_exact_size(vec2(row_w, 11.0), Sense::click_and_drag());
        let lp = ui.painter();
        lp.rect_filled(track, 3.0, Color32::from_rgb(15, 17, 21));
        lp.rect_stroke(track, 3.0, Stroke::new(1.0, Color32::from_rgb(44, 50, 62)), StrokeKind::Inside);

        let ticks = 16;
        for i in 1..ticks {
            let tx = track.min.x + track.width() * (i as f32 / ticks as f32);
            let is_mid = i == ticks / 2;
            lp.line_segment(
                [pos2(tx, track.min.y + if is_mid { 2.0 } else { 4.0 }), pos2(tx, track.max.y - if is_mid { 2.0 } else { 4.0 })],
                Stroke::new(1.0, if is_mid { Color32::from_rgb(90, 98, 114) } else { Color32::from_rgb(36, 40, 50) }),
            );
        }

        let frac = (lum / lum_max).clamp(-1.0, 1.0);
        let cx = track.center().x;
        let kx = cx + frac * (track.width() * 0.5 - 4.0);
        if frac.abs() > 0.005 {
            let fill_r = egui::Rect::from_min_max(pos2(cx.min(kx), track.min.y + 3.0), pos2(cx.max(kx), track.max.y - 3.0));
            let fill_col = if frac >= 0.0 { Color32::from_rgba_unmultiplied(235, 185, 70, 120) } else { Color32::from_rgba_unmultiplied(80, 150, 235, 120) };
            lp.rect_filled(fill_r, 1.5, fill_col);
        }
        lp.rect_filled(egui::Rect::from_center_size(pos2(kx, track.center().y), vec2(4.0, track.height() - 4.0)), 1.5, Color32::from_rgb(235, 240, 248));

        let emit_lum = |actions: &mut Vec<serde_json::Value>, nv: f32, merge: bool| match kind {
            WheelKind::Lift => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_shadows_l", "value": nv, "merge": merge})),
            WheelKind::Gamma => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_midtones_l", "value": nv, "merge": merge})),
            WheelKind::Gain => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_highlights_l", "value": nv, "merge": merge})),
            WheelKind::Offset => actions.push(json!({"clip": clip.0, "effect": idx, "param": "exposure", "value": nv, "merge": merge})),
        };

        if lr.double_clicked() {
            emit_lum(ctx.actions, 0.0, false);
        } else if lr.dragged() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            let mult = if shift { 0.25 } else { 1.0 };
            let step = (lr.drag_delta().x / (track.width() * 0.5).max(1.0)) * lum_max * mult;
            let nv = (lum + step).clamp(-lum_max, lum_max);
            if (nv - lum).abs() > 1e-3 {
                emit_lum(ctx.actions, nv, true);
            }
        } else if lr.clicked()
            && let Some(pos) = lr.interact_pointer_pos()
        {
            let nv = ((pos.x - cx) / (track.width() * 0.5 - 4.0).max(1.0) * lum_max).clamp(-lum_max, lum_max);
            emit_lum(ctx.actions, nv, false);
        }
        lr.on_hover_text(tl!("Drag horizontally to adjust luminance (Shift = fine, Double-click = reset)"));

        // Interactive numeric readout below Master Wheel (scrub or click to type)
        let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, idx, id_tag);
        ui.allocate_ui_with_layout(vec2(row_w, 15.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let micro = |ui: &mut egui::Ui, lbl: &str| {
                ui.label(egui::RichText::new(lbl).size(8.5).color(t.text_faint));
            };
            match kind {
                WheelKind::Offset => {
                    let temp = (vx * 100.0) as f64;
                    let tint = (vy * 100.0) as f64;
                    micro(ui, "T");
                    let (_, nt) = crate::widgets::hot_number(
                        ui,
                        "Temp",
                        egui::Id::unique(("wheel-num-t", scope)),
                        temp,
                        crate::widgets::HotNumberOptions::new(0.5, (-100.0, 100.0), 0, "", t),
                    );
                    if let Some(v) = nt {
                        ctx.actions.push(json!({"clip": clip.0, "effect": idx, "param": "temperature", "value": v, "merge": true}));
                    }
                    micro(ui, "Ti");
                    let (_, nti) = crate::widgets::hot_number(
                        ui,
                        "Tint",
                        egui::Id::unique(("wheel-num-ti", scope)),
                        tint,
                        crate::widgets::HotNumberOptions::new(0.5, (-100.0, 100.0), 0, "", t),
                    );
                    if let Some(v) = nti {
                        ctx.actions.push(json!({"clip": clip.0, "effect": idx, "param": "tint", "value": v, "merge": true}));
                    }
                    micro(ui, "EV");
                    let (_, nev) = crate::widgets::hot_number(
                        ui,
                        "Exposure",
                        egui::Id::unique(("wheel-num-ev", scope)),
                        lum as f64,
                        crate::widgets::HotNumberOptions::new(0.02, (-5.0, 5.0), 1, "", t),
                    );
                    if let Some(v) = nev {
                        emit_lum(ctx.actions, v as f32, true);
                    }
                }
                _ => {
                    let clamp_disk = |x: f32, y: f32| {
                        let v = vec2(x, y);
                        if v.length() > 1.0 {
                            let n = v.normalized();
                            (n.x, n.y)
                        } else {
                            (x, y)
                        }
                    };
                    micro(ui, "X");
                    let (_, nx) = crate::widgets::hot_number(
                        ui,
                        "X",
                        egui::Id::unique(("wheel-num-x", scope)),
                        vx as f64,
                        crate::widgets::HotNumberOptions::new(0.01, (-1.0, 1.0), 2, "", t),
                    );
                    if let Some(v) = nx {
                        let (cx, cy) = clamp_disk(v as f32, vy);
                        emit_2d(ctx.actions, cx, cy, true);
                    }
                    micro(ui, "Y");
                    let (_, ny) = crate::widgets::hot_number(
                        ui,
                        "Y",
                        egui::Id::unique(("wheel-num-y", scope)),
                        vy as f64,
                        crate::widgets::HotNumberOptions::new(0.01, (-1.0, 1.0), 2, "", t),
                    );
                    if let Some(v) = ny {
                        let (cx, cy) = clamp_disk(vx, v as f32);
                        emit_2d(ctx.actions, cx, cy, true);
                    }
                    micro(ui, "L");
                    let (_, nl) = crate::widgets::hot_number(
                        ui,
                        "Lum",
                        egui::Id::unique(("wheel-num-l", scope)),
                        lum as f64,
                        crate::widgets::HotNumberOptions::new(0.5, (-100.0, 100.0), 0, "", t),
                    );
                    if let Some(v) = nl {
                        emit_lum(ctx.actions, v as f32, true);
                    }
                }
            }
        });
    });
    ctx.auto_rows.push((format!("lumetri.wheel.{id_tag}"), frame_resp.response.rect, title));
}

fn color_ring_lut() -> &'static [(f32, f32, Color32)] {
    use std::sync::OnceLock;
    static LUT: OnceLock<Vec<(f32, f32, Color32)>> = OnceLock::new();
    LUT.get_or_init(|| {
        let n = 72usize;
        (0..=n)
            .map(|i| {
                let a = (i % n) as f32 / n as f32 * std::f32::consts::TAU;
                let rgb = filmcraft_color::hsl_to_rgb(1.0 - ((i % n) as f32 / n as f32), 0.78, 0.52);
                let col =
                    Color32::from_rgb((rgb[0].clamp(0.0, 1.0) * 255.0) as u8, (rgb[1].clamp(0.0, 1.0) * 255.0) as u8, (rgb[2].clamp(0.0, 1.0) * 255.0) as u8);
                (a.cos(), a.sin(), col)
            })
            .collect()
    })
}

/// Temp/Tint 2D ring for Offset wheel: -X Cool Blue <-> +X Warm Amber, -Y Green <-> +Y Magenta.
fn offset_ring_lut() -> &'static [(f32, f32, Color32)] {
    use std::sync::OnceLock;
    static LUT: OnceLock<Vec<(f32, f32, Color32)>> = OnceLock::new();
    LUT.get_or_init(|| {
        let n = 72usize;
        (0..=n)
            .map(|i| {
                let a = (i % n) as f32 / n as f32 * std::f32::consts::TAU;
                let (cx, cy) = (a.cos(), a.sin());
                let wx = (cx + 1.0) * 0.5;
                let wy = (cy + 1.0) * 0.5;
                let tx = [70.0 + (245.0 - 70.0) * wx, 135.0 + (155.0 - 135.0) * wx, 240.0 + (55.0 - 240.0) * wx];
                let ty = [65.0 + (225.0 - 65.0) * wy, 195.0 + (75.0 - 195.0) * wy, 90.0 + (205.0 - 90.0) * wy];
                let mx = cx.abs() / (cx.abs() + cy.abs()).max(1e-4);
                let my = 1.0 - mx;
                let col = Color32::from_rgb(
                    (tx[0] * mx + ty[0] * my).clamp(0.0, 255.0) as u8,
                    (tx[1] * mx + ty[1] * my).clamp(0.0, 255.0) as u8,
                    (tx[2] * mx + ty[2] * my).clamp(0.0, 255.0) as u8,
                );
                (cx, cy, col)
            })
            .collect()
    })
}
