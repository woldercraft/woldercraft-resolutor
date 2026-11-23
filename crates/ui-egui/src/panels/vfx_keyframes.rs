//! Dedicated per-node VFX Keyframes & Spline Curve Editor (`VfxKeyframes`).
//!
//! Displays **strictly** the animatable parameters of the single active node selected in `VfxNodes`
//! (including `Effect`, `Instance`, `Merge`, `Mask`, and `Settings.Blend`), with a non-clipping
//! time ruler, direct `◆` keyframe toggle, scrubbing, and an interactive `Spline` curve mode.

use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, pos2, vec2};
use filmcraft_project::{ParamValue, VfxNodeId};
use filmcraft_time::Tick;
use serde_json::json;

use crate::FilmcraftApp;
use crate::panels::effect_controls::paint_ruler;
use crate::panels::vfx_nodes::{active_node_params, node_display_name, resolve_target_clip};
use crate::theme::Tokens;

/// Entry point for the `VfxKeyframes` dock panel.
#[allow(clippy::cognitive_complexity)]
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::LEFT)));
    let ui = &mut ui;

    let Some((clip_id, _clip_name)) = resolve_target_clip(app) else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("Selecciona un clip de vídeo para ver sus keyframes VFX").color(t.text_dim));
        });
        return;
    };

    let Some(seq) = app.session.active_sequence().cloned() else { return };
    let Some((_, it)) = seq.find_item(clip_id).map(|(tid, i)| (tid, i.clone())) else { return };
    let graph = it.effective_vfx_graph().into_owned();
    if graph.nodes.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("Sin nodos VFX en este clip").color(t.text_dim));
        });
        return;
    }

    let active_id = app
        .ui
        .vfx_active_node_id
        .map(VfxNodeId)
        .filter(|id| graph.node(*id).is_some())
        .or_else(|| graph.nodes.first().map(|n| n.id));
    let Some(nid) = active_id else { return };
    app.ui.vfx_active_node_id = Some(nid.0);
    let Some(node) = graph.node(nid) else { return };

    let title = node_display_name(node, &graph);
    let params = active_node_params(node, &graph);
    let ph = app.session.playhead();
    let mt_now = it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
    let tc = filmcraft_time::format_time(ph, seq.settings.frame_rate, seq.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48000);

    // 1. Clean Toolbar: Mode switcher (Keyframes | Spline) + Active Node + Integrated Timecode
    let mut mode = if app.ui.vfx_bottom_drawer == 2 { 2u8 } else { 1u8 };
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let kf_btn = ui.selectable_label(mode == 1, egui::RichText::new("Keyframes").size(11.5).strong());
        app.auto.add("vfxKeyframes.mode.dope", kf_btn.rect, "Keyframes Mode");
        if kf_btn.clicked() {
            mode = 1;
            app.ui.vfx_bottom_drawer = 1;
        }
        let sp_btn = ui.selectable_label(mode == 2, egui::RichText::new("Spline").size(11.5).strong());
        app.auto.add("vfxKeyframes.mode.spline", sp_btn.rect, "Spline Mode");
        if sp_btn.clicked() {
            mode = 2;
            app.ui.vfx_bottom_drawer = 2;
        }

        ui.separator();
        ui.label(egui::RichText::new(title).strong().size(11.5).color(t.accent));
        ui.label(egui::RichText::new(format!("({} parámetros)", params.len())).size(10.5).color(t.text_dim));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tc).monospace().size(11.5).color(t.timecode));
        });
    });
    ui.separator();

    let dur_ticks = it.duration.0.max(1) as f64;
    let ph_frac = ((ph.0 - it.start.0) as f64 / dur_ticks).clamp(0.0, 1.0) as f32;
    let in_point = it.effect_time_at(it.start);
    let spd = if it.speed.abs() > 1e-6 { it.speed } else { 1.0 };

    if mode == 1 {
        let avail = ui.available_rect_before_wrap();
        let label_w = (avail.width() * 0.34).clamp(130.0, 230.0);

        // Non-clipping time ruler row aligned over the right lane
        let (ruler_row, _) = ui.allocate_exact_size(vec2(avail.width(), 22.0), Sense::hover());
        let ruler_r = Rect::from_min_max(pos2(ruler_row.left() + label_w + 6.0, ruler_row.top()), pos2(ruler_row.right() - 6.0, ruler_row.bottom()));
        if ruler_r.width() > 16.0 {
            ui.painter().rect_filled(ruler_r, 0.0, t.tl_bg);
            paint_ruler(ui.painter(), ruler_r, &it, seq.settings.frame_rate, seq.settings.drop_frame, &t);
            let sresp = ui.interact(ruler_r, ui.scope_id().with("vfx_kf_ruler_scrub"), Sense::click_and_drag());
            if (sresp.clicked() || sresp.dragged())
                && let Some(pos) = sresp.interact_pointer_pos()
            {
                let f = ((pos.x - ruler_r.left()) / ruler_r.width().max(1.0)).clamp(0.0, 1.0) as f64;
                let tk = seq.settings.frame_rate.snap_nearest(it.start + Tick((f * dur_ticks) as i64));
                app.stop_for_scrub();
                app.session.set_playhead(tk);
            }
            let px = ruler_r.left() + ph_frac * ruler_r.width();
            ui.painter().line_segment([pos2(px, ruler_r.top()), pos2(px, ruler_r.bottom())], Stroke::new(1.5, t.playhead));
        }

        egui::ScrollArea::vertical().id_salt("vfx_kf_panel_scroll").auto_shrink([false, false]).show(ui, |ui| {
            for (pid, pname, prm) in &params {
                let (row_r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
                let left_r = Rect::from_min_max(row_r.left_top(), pos2(row_r.left() + label_w, row_r.bottom()));
                let lane_r = Rect::from_min_max(pos2(left_r.right() + 6.0, row_r.top() + 1.0), pos2(row_r.right() - 6.0, row_r.bottom() - 1.0));

                let p = ui.painter();
                p.rect_filled(lane_r, CornerRadius::same(2), t.tl_bg);

                // Direct diamond `◆` on the left row
                let on_kf = prm.keyframes.iter().any(|k| k.time == mt_now);
                let kf_r = Rect::from_center_size(pos2(left_r.left() + 14.0, left_r.center().y), vec2(14.0, 14.0));
                let kf_resp = ui.interact(kf_r, ui.scope_id().with(("vfx_kf_row_tog", nid.0, pid)), Sense::click());
                let dc = kf_r.center();
                let s = 4.2f32;
                let pts = vec![dc + vec2(0.0, -s), dc + vec2(s, 0.0), dc + vec2(0.0, s), dc + vec2(-s, 0.0)];
                if on_kf {
                    p.add(egui::Shape::convex_polygon(pts, t.hot_text, Stroke::NONE));
                } else {
                    let col = if prm.is_animated() { t.accent } else { t.text_dim };
                    p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, col)));
                }
                if kf_resp.on_hover_text("Agregar / quitar keyframe").clicked() {
                    if pid == "__settings.blend" {
                        let _ = app
                            .session
                            .execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": nid.0, "blendToggleKeyframe": true}));
                    } else {
                        let _ = app
                            .session
                            .execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": nid.0, "param": pid, "toggleKeyframe": true}));
                    }
                }

                p.with_clip_rect(left_r)
                    .text(pos2(left_r.left() + 26.0, left_r.center().y), Align2::LEFT_CENTER, pname, Tokens::ui(11.5), t.text);

                // Paint keyframes along the lane
                for kf in &prm.keyframes {
                    let tl_ticks = ((kf.time.0 - in_point.0) as f64 / spd) as i64;
                    let frac = (tl_ticks as f64 / dur_ticks).clamp(0.0, 1.0) as f32;
                    let kx = lane_r.left() + frac * lane_r.width();
                    let ky = lane_r.center().y;
                    let d = 4.5f32;
                    let dpts = vec![Pos2::new(kx, ky - d), Pos2::new(kx + d, ky), Pos2::new(kx, ky + d), Pos2::new(kx - d, ky)];
                    let fill = if kf.time == mt_now { t.hot_text } else { Color32::from_rgb(96, 165, 250) };
                    p.add(egui::Shape::convex_polygon(dpts, fill, Stroke::new(1.0, Color32::WHITE)));
                }

                // Playhead line + scrub on lane click
                let px = lane_r.left() + ph_frac * lane_r.width();
                p.line_segment([pos2(px, lane_r.top()), pos2(px, lane_r.bottom())], Stroke::new(1.2, t.playhead));
                let lresp = ui.interact(lane_r, ui.scope_id().with(("vfx_kf_lane_scrub", nid.0, pid)), Sense::click_and_drag());
                if (lresp.clicked() || lresp.dragged())
                    && let Some(hp) = lresp.interact_pointer_pos()
                {
                    let f = ((hp.x - lane_r.left()) / lane_r.width().max(1.0)).clamp(0.0, 1.0) as f64;
                    let tk = seq.settings.frame_rate.snap_nearest(it.start + Tick((f * dur_ticks) as i64));
                    app.stop_for_scrub();
                    app.session.set_playhead(tk);
                }
            }
        });
    } else {
        // Spline Curve View isolated strictly to the active node
        let avail = ui.available_rect_before_wrap();
        let graph_r = Rect::from_min_max(pos2(avail.left() + 6.0, avail.top() + 4.0), pos2(avail.right() - 6.0, avail.bottom() - 6.0));
        if graph_r.width() > 20.0 && graph_r.height() > 30.0 {
            let gresp = ui.interact(graph_r, ui.scope_id().with("vfx_kf_spline_canvas"), Sense::click_and_drag());
            let p = ui.painter_at(graph_r);
            p.rect_filled(graph_r, CornerRadius::same(4), t.tl_bg);
            for i in 1..4 {
                let y = graph_r.top() + graph_r.height() * (i as f32 / 4.0);
                p.line_segment([pos2(graph_r.left(), y), pos2(graph_r.right(), y)], Stroke::new(0.8, t.separator));
            }
            let palette = [
                Color32::from_rgb(96, 165, 250),
                Color32::from_rgb(52, 211, 153),
                Color32::from_rgb(250, 204, 21),
                Color32::from_rgb(244, 114, 182),
            ];
            let mut curve_idx = 0usize;
            for (_, pname, prm) in &params {
                if !prm.is_animated() || !matches!(prm.value, ParamValue::Float(_)) {
                    continue;
                }
                let col = palette[curve_idx % palette.len()];
                curve_idx += 1;
                let mut vmin = f64::MAX;
                let mut vmax = f64::MIN;
                for kf in &prm.keyframes {
                    if let Some(v) = kf.value.as_f64() {
                        vmin = vmin.min(v);
                        vmax = vmax.max(v);
                    }
                }
                if (vmax - vmin).abs() < 1e-4 {
                    vmin -= 1.0;
                    vmax += 1.0;
                }
                let span = (vmax - vmin).max(1e-4);
                let mut pts = Vec::with_capacity(64);
                for s in 0..=64 {
                    let frac = s as f32 / 64.0;
                    let mt = Tick(in_point.0 + (frac as f64 * dur_ticks * spd) as i64);
                    let v = prm.f64_at(mt);
                    let ny = 1.0 - ((v - vmin) / span).clamp(0.0, 1.0) as f32;
                    pts.push(pos2(graph_r.left() + frac * graph_r.width(), graph_r.top() + 10.0 + ny * (graph_r.height() - 20.0)));
                }
                p.add(egui::Shape::line(pts, Stroke::new(2.0, col)));
                p.text(pos2(graph_r.left() + 8.0, graph_r.top() + 6.0 + (curve_idx - 1) as f32 * 14.0), Align2::LEFT_TOP, pname, Tokens::ui(10.5), col);
            }
            if curve_idx == 0 {
                p.text(
                    graph_r.center(),
                    Align2::CENTER_CENTER,
                    "Haz clic en ◆ en Inspector VFX para animar un parámetro y ver su curva aquí",
                    Tokens::ui(11.5),
                    t.text_dim,
                );
            }
            let px = graph_r.left() + ph_frac * graph_r.width();
            p.line_segment([pos2(px, graph_r.top()), pos2(px, graph_r.bottom())], Stroke::new(1.5, t.playhead));
            if (gresp.clicked() || gresp.dragged())
                && let Some(hp) = gresp.interact_pointer_pos()
            {
                let f = ((hp.x - graph_r.left()) / graph_r.width().max(1.0)).clamp(0.0, 1.0) as f64;
                let tk = seq.settings.frame_rate.snap_nearest(it.start + Tick((f * dur_ticks) as i64));
                app.stop_for_scrub();
                app.session.set_playhead(tk);
            }
        }
    }
}
