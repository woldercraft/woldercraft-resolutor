//! DaVinci Resolve / Fusion-style VFX Compositing Node Graph Editor (`VfxNodes`).
//!
//! Renders and edits [`filmcraft_project::VfxGraph`] with:
//! - Colored Fusion ports (Yellow `Main`/BG, Green `Foreground`, Blue `EffectMask`, White `Output`)
//! - Live wire dragging (`vfxGraph.connect` / `vfxGraph.disconnect`), `Shift+Drag` splice & extract
//!   (`vfxGraph.insertOnEdge` / `vfxGraph.extractNode`), and `Ctrl+Shift+Drag` knife wire cutting
//! - Floating `Shift+Space` *Select Tool* fuzzy finder, `Ctrl+P` Pass-Through, `Ctrl+T` Swap Merge,
//!   `F2` Rename, `1`/`2` Viewer dots, `Underlays`, and green dashed `Instance` links
//! - Toggleable integrated bottom `Keyframes` / `Spline` drawer strictly isolated to the active node

use std::collections::HashMap;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use filmcraft_engine::EffectSel;
use filmcraft_project::{
    ClipId, EffectKind, Param, ParamValue, TrackKind, VfxEdge, VfxEndpoint, VfxGraph, VfxInputSlot, VfxNode, VfxNodeId, VfxNodeKind, VfxPortKind,
};
use filmcraft_time::Tick;
use serde_json::json;

use crate::FilmcraftApp;
use crate::theme::Tokens;

const NODE_W: f32 = 164.0;
const NODE_H: f32 = 64.0;
const ROUTER_R: f32 = 14.0;
const IO_W: f32 = 88.0;
const IO_H: f32 = 46.0;

const COL_MAIN: Color32 = Color32::from_rgb(234, 179, 8); // Yellow
const COL_FG: Color32 = Color32::from_rgb(34, 197, 94); // Green
const COL_MASK: Color32 = Color32::from_rgb(59, 130, 246); // Blue
const COL_OUT: Color32 = Color32::from_rgb(235, 240, 248); // White

#[derive(Clone, Copy, Debug)]
struct PendingVfxWire {
    from: VfxEndpoint,
    kind: VfxPortKind,
    start_pos: Pos2,
}

#[derive(Clone, Copy, Debug)]
struct KnifeCut {
    start: Pos2,
    current: Pos2,
}

#[derive(Clone, Debug)]
struct SelectToolPopup {
    open: bool,
    query: String,
    after_node: Option<u64>,
}

/// Distance from point `p` to a horizontal-tangent cubic Bézier wire from `p0` to `p3`.
/// Shared with `color_nodes.rs` for hit-testing and wire splicing.
pub(crate) fn bezier_wire_distance(p: Pos2, p0: Pos2, p3: Pos2, steps: usize) -> f32 {
    let n = steps.max(4);
    let dx = ((p3.x - p0.x).abs() * 0.45).max(32.0);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);
    let mut prev = p0;
    let mut best = f32::MAX;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let cur = eval_cubic_bezier(p0, p1, p2, p3, t);
        best = best.min(point_segment_distance(p, prev, cur));
        prev = cur;
    }
    best
}

/// True if line segments `(a0, a1)` and `(b0, b1)` intersect. Shared with `color_nodes.rs`.
pub(crate) fn segments_intersect(a0: Pos2, a1: Pos2, b0: Pos2, b1: Pos2) -> bool {
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let d1 = cross(a0, a1, b0);
    let d2 = cross(a0, a1, b1);
    let d3 = cross(b0, b1, a0);
    let d4 = cross(b0, b1, a1);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    let on_seg = |o: Pos2, a: Pos2, p: Pos2| {
        p.x >= o.x.min(a.x) - 1e-3 && p.x <= o.x.max(a.x) + 1e-3 && p.y >= o.y.min(a.y) - 1e-3 && p.y <= o.y.max(a.y) + 1e-3
    };
    (d1.abs() < 1e-3 && on_seg(a0, a1, b0))
        || (d2.abs() < 1e-3 && on_seg(a0, a1, b1))
        || (d3.abs() < 1e-3 && on_seg(b0, b1, a0))
        || (d4.abs() < 1e-3 && on_seg(b0, b1, a1))
}

/// True if a horizontal-tangent cubic Bézier wire `(p0 -> p3)` intersects line segment `(s0, s1)`.
/// Shared with `color_nodes.rs` for `Ctrl+Shift+Drag` knife wire cutting.
pub(crate) fn bezier_intersects_segment(p0: Pos2, p3: Pos2, s0: Pos2, s1: Pos2, steps: usize) -> bool {
    let n = steps.max(6);
    let dx = ((p3.x - p0.x).abs() * 0.45).max(32.0);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);
    let mut prev = p0;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let cur = eval_cubic_bezier(p0, p1, p2, p3, t);
        if segments_intersect(prev, cur, s0, s1) {
            return true;
        }
        prev = cur;
    }
    false
}

fn eval_cubic_bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;
    Pos2::new(
        uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x,
        uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y,
    )
}

fn point_segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 <= 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn draw_bezier_wire(painter: &egui::Painter, p0: Pos2, p3: Pos2, stroke: Stroke) {
    let dx = ((p3.x - p0.x).abs() * 0.45).max(32.0);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);
    let pts: Vec<Pos2> = (0..=18).map(|i| eval_cubic_bezier(p0, p1, p2, p3, i as f32 / 18.0)).collect();
    painter.add(egui::Shape::line(pts, stroke));
}

fn draw_dashed_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    let d = b - a;
    let len = d.length();
    if len < 2.0 {
        return;
    }
    let dir = d / len;
    let mut t = 0.0f32;
    while t < len {
        let t1 = (t + 6.0).min(len);
        painter.line_segment([a + dir * t, a + dir * t1], stroke);
        t += 11.0;
    }
}

pub(crate) fn resolve_target_clip(app: &FilmcraftApp) -> Option<(ClipId, String)> {
    let seq = app.session.active_sequence()?;
    if let Some(cid) = app
        .session
        .state
        .selection
        .iter()
        .copied()
        .find(|&c| seq.find_item(c).and_then(|(tid, _)| seq.track(tid)).is_some_and(|t| t.kind == TrackKind::Video))
    {
        let (_, it) = seq.find_item(cid)?;
        let name = app.session.project.item(it.item).map(|p| p.name.clone()).unwrap_or_else(|| format!("Clip {}", cid.0));
        return Some((cid, name));
    }
    let ph = app.session.playhead();
    for track in seq.video_tracks.iter().rev() {
        if !track.locked && !track.muted {
            for item in &track.items {
                if item.start <= ph && ph < item.end() {
                    let name = app.session.project.item(item.item).map(|p| p.name.clone()).unwrap_or_else(|| format!("Clip {}", item.id.0));
                    return Some((item.id, name));
                }
            }
        }
    }
    let item = seq.video_tracks.iter().flat_map(|t| &t.items).next()?;
    let name = app.session.project.item(item.item).map(|p| p.name.clone()).unwrap_or_else(|| format!("Clip {}", item.id.0));
    Some((item.id, name))
}

fn endpoint_json(ep: VfxEndpoint) -> serde_json::Value {
    match ep {
        VfxEndpoint::MediaIn => json!("in"),
        VfxEndpoint::MediaOut => json!("out"),
        VfxEndpoint::Node(id) => json!(id.0),
    }
}

fn slot_str(slot: VfxInputSlot) -> &'static str {
    match slot {
        VfxInputSlot::Main => "main",
        VfxInputSlot::Foreground => "fg",
        VfxInputSlot::EffectMask => "mask",
    }
}

fn select_vfx_node(app: &mut FilmcraftApp, clip_id: ClipId, node: &VfxNode, graph: &VfxGraph) {
    app.ui.vfx_active_node_id = Some(node.id.0);
    // Synchronize `session.state.selected_effect` if this node maps to a clip effect instance
    if let Some(seq) = app.session.active_sequence()
        && let Some((_, it)) = seq.find_item(clip_id)
        && let Some(resolved) = graph.resolve_effect(node.id)
    {
        // Match by effect id + occurrence ordinal in serial_effects
        let mut ord = 0usize;
        for id in graph.topo_order().unwrap_or_else(|| graph.nodes.iter().map(|n| n.id).collect()) {
            if id == node.id {
                break;
            }
            if graph.resolve_effect(id).is_some_and(|e| e.effect == resolved.effect) {
                ord += 1;
            }
        }
        let mut seen = 0usize;
        for (idx, e) in it.effects.iter().enumerate() {
            if e.effect == resolved.effect {
                if seen == ord {
                    app.session.state.selected_effect = EffectSel::of(clip_id, &it.effects, idx);
                    return;
                }
                seen += 1;
            }
        }
    }
}

pub(crate) fn node_display_name(node: &VfxNode, graph: &VfxGraph) -> String {
    if let Some(lbl) = &node.label
        && !lbl.is_empty()
    {
        return lbl.clone();
    }
    match &node.kind {
        VfxNodeKind::Effect(fx) => fx.def().map(|d| d.name.to_string()).unwrap_or_else(|| fx.effect.clone()),
        VfxNodeKind::Merge { operator, .. } => format!("Merge ({})", operator.label()),
        VfxNodeKind::Mask(m) => m.name.clone(),
        VfxNodeKind::PipeRouter => "Router".to_string(),
        VfxNodeKind::Instance { source, .. } => {
            let base = graph.node(*source).map(|n| node_display_name(n, graph)).unwrap_or_else(|| format!("Node {}", source.0));
            format!("{base} (Inst)")
        }
    }
}

pub(crate) fn node_accent(kind: &VfxNodeKind) -> Color32 {
    match kind {
        VfxNodeKind::Effect(fx) => {
            if fx.effect.starts_with("ec.") {
                Color32::from_rgb(139, 92, 246)
            } else {
                Color32::from_rgb(56, 189, 248)
            }
        }
        VfxNodeKind::Merge { .. } => Color32::from_rgb(34, 197, 94),
        VfxNodeKind::Mask(_) => Color32::from_rgb(59, 130, 246),
        VfxNodeKind::PipeRouter => Color32::from_rgb(245, 158, 11),
        VfxNodeKind::Instance { .. } => Color32::from_rgb(16, 185, 129),
    }
}

/// Main entry point for the `VfxNodes` dock panel.
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::LEFT)));
    let ui = &mut ui;
    let Some((clip_id, clip_name)) = resolve_target_clip(app) else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("Selecciona un clip de vídeo para editar sus nodos VFX").color(t.text_dim));
        });
        return;
    };

    let (graph, clip_start, clip_dur, item_in_point, item_speed) = {
        let Some(seq) = app.session.active_sequence() else { return };
        let Some((_, it)) = seq.find_item(clip_id) else { return };
        (it.effective_vfx_graph().into_owned(), it.start, it.duration, it.effect_time_at(it.start), it.speed)
    };

    // Ensure active node stays valid if the graph has nodes
    if let Some(active) = app.ui.vfx_active_node_id
        && graph.node(VfxNodeId(active)).is_none()
    {
        app.ui.vfx_active_node_id = graph.nodes.first().map(|n| n.id.0);
    }

    let popup_id = ui.scope_id().with("vfx_select_tool_popup");
    let zoom_id = ui.scope_id().with("vfx_nodes_zoom");
    let pan_id = ui.scope_id().with("vfx_nodes_pan");

    render_toolbar(app, ui, &t, clip_id, &clip_name, &graph, popup_id, zoom_id, pan_id);
    ui.separator();

    // Split bottom area when Keyframes (1) or Spline (2) drawer is open
    let drawer_mode = app.ui.vfx_bottom_drawer;
    let total_h = ui.available_height().max(120.0);
    let drawer_h = if drawer_mode != 0 { (total_h * 0.36).clamp(110.0, 260.0) } else { 0.0 };
    let canvas_h = (total_h - drawer_h - if drawer_mode != 0 { 6.0 } else { 0.0 }).max(80.0);

    let canvas_rect = ui.allocate_exact_size(Vec2::new(ui.available_width(), canvas_h), Sense::click_and_drag()).0;
    render_graph_canvas(app, ui, &t, clip_id, &graph, canvas_rect, popup_id, zoom_id, pan_id);

    if drawer_mode != 0 {
        ui.separator();
        let active_node = app.ui.vfx_active_node_id.and_then(|id| graph.node(VfxNodeId(id)));
        render_bottom_drawer(app, ui, &t, clip_id, &graph, active_node, drawer_mode, drawer_h, clip_start, clip_dur, item_in_point, item_speed);
    }

    render_select_tool_popup(app, ui, clip_id, popup_id);
}

#[allow(clippy::too_many_arguments)]
fn render_toolbar(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    clip_id: ClipId,
    clip_name: &str,
    graph: &VfxGraph,
    popup_id: egui::Id,
    zoom_id: egui::Id,
    pan_id: egui::Id,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(egui::RichText::new(format!("VFX · {clip_name}")).strong().size(12.0).color(t.text));
        ui.label(egui::RichText::new(format!("({} nodos)", graph.nodes.len())).size(11.0).color(t.text_dim));
        ui.separator();

        let add_btn = ui.small_button("+ Efecto").on_hover_text("Shift+Space: Buscar e insertar herramienta VFX");
        app.auto.add("vfxNodes.addTool", add_btn.rect, "Add VFX Tool");
        if add_btn.clicked() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    popup_id,
                    SelectToolPopup { open: true, query: String::new(), after_node: app.ui.vfx_active_node_id },
                );
            });
        }

        let after_val = app.ui.vfx_active_node_id;
        let merge_btn = ui.small_button("+ Merge").on_hover_text("Añadir nodo Merge (BG amarillo + FG verde)");
        app.auto.add("vfxNodes.addMerge", merge_btn.rect, "Add Merge Node");
        if merge_btn.clicked() {
            let mut body = json!({"clip": clip_id.0, "kind": "merge"});
            if let Some(a) = after_val {
                body["after"] = json!(a);
            }
            if let Ok(res) = app.session.execute("vfxGraph.addNode", body)
                && let Some(nid) = res["node"].as_u64()
            {
                app.ui.vfx_active_node_id = Some(nid);
            }
        }

        let mask_btn = ui.small_button("+ Mask").on_hover_text("Añadir nodo Mask reutilizable (puerto azul EffectMask)");
        app.auto.add("vfxNodes.addMask", mask_btn.rect, "Add Mask Node");
        if mask_btn.clicked() {
            let mut body = json!({"clip": clip_id.0, "kind": "mask", "maskKind": "ellipse"});
            if let Some(a) = after_val {
                body["after"] = json!(a);
            }
            if let Ok(res) = app.session.execute("vfxGraph.addNode", body)
                && let Some(nid) = res["node"].as_u64()
            {
                app.ui.vfx_active_node_id = Some(nid);
            }
        }

        let router_btn = ui.small_button("+ Router").on_hover_text("Añadir PipeRouter para organizar cables");
        app.auto.add("vfxNodes.addRouter", router_btn.rect, "Add PipeRouter");
        if router_btn.clicked() {
            let mut body = json!({"clip": clip_id.0, "kind": "pipeRouter"});
            if let Some(a) = after_val {
                body["after"] = json!(a);
            }
            let _ = app.session.execute("vfxGraph.addNode", body);
        }

        if let Some(aid) = after_val {
            let inst_btn = ui.small_button("+ Inst").on_hover_text("Crear nodo Instance vinculado al nodo seleccionado");
            app.auto.add("vfxNodes.createInstance", inst_btn.rect, "Create Instance");
            if inst_btn.clicked()
                && let Ok(res) = app.session.execute("vfxGraph.createInstance", json!({"clip": clip_id.0, "source": aid}))
                && let Some(nid) = res["node"].as_u64()
            {
                app.ui.vfx_active_node_id = Some(nid);
            }
        }

        let ul_btn = ui.small_button("+ Grupo").on_hover_text("Añadir Underlay visual para agrupar nodos");
        app.auto.add("vfxNodes.addUnderlay", ul_btn.rect, "Add Underlay");
        if ul_btn.clicked() {
            let _ = app.session.execute(
                "vfxGraph.addUnderlay",
                json!({"clip": clip_id.0, "title": "Grupo VFX", "rect": [90.0, 70.0, 380.0, 170.0]}),
            );
        }

        ui.separator();
        let org_btn = ui.small_button("Organizar").on_hover_text("Auto-organizar topológicamente de izquierda a derecha");
        app.auto.add("vfxNodes.autoLayout", org_btn.rect, "Auto Layout");
        if org_btn.clicked() {
            let _ = app.session.execute("vfxGraph.autoLayout", json!({"clip": clip_id.0}));
        }

        let fit_btn = ui.small_button("Encuadrar");
        if fit_btn.clicked() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(zoom_id, 1.0f32);
                d.insert_temp(pan_id, Vec2::ZERO);
            });
        }

        ui.separator();
        let kf_on = app.ui.vfx_bottom_drawer == 1;
        let kf_btn = ui
            .selectable_label(kf_on, "Keyframes")
            .on_hover_text("Mostrar solo las pistas de keyframes del nodo seleccionado");
        app.auto.add("vfxNodes.drawer.keyframes", kf_btn.rect, "Toggle Node Keyframes");
        if kf_btn.clicked() {
            app.ui.vfx_bottom_drawer = if kf_on { 0 } else { 1 };
        }

        let sp_on = app.ui.vfx_bottom_drawer == 2;
        let sp_btn = ui
            .selectable_label(sp_on, "Spline")
            .on_hover_text("Mostrar solo las curvas Spline del nodo seleccionado");
        app.auto.add("vfxNodes.drawer.spline", sp_btn.rect, "Toggle Node Spline");
        if sp_btn.clicked() {
            app.ui.vfx_bottom_drawer = if sp_on { 0 } else { 2 };
        }
    });
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn render_graph_canvas(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    clip_id: ClipId,
    graph: &VfxGraph,
    rect: Rect,
    popup_id: egui::Id,
    zoom_id: egui::Id,
    pan_id: egui::Id,
) {
    let mut zoom = ui.ctx().data(|d| d.get_temp::<f32>(zoom_id)).unwrap_or(1.0).clamp(0.45, 2.2);
    let mut pan = ui.ctx().data(|d| d.get_temp::<Vec2>(pan_id)).unwrap_or(Vec2::ZERO);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(20, 21, 25));

    // Subtle Fusion-style dot grid
    let step = (24.0 * zoom).max(10.0);
    let ox = (rect.left() + pan.x).rem_euclid(step);
    let oy = (rect.top() + pan.y).rem_euclid(step);
    let dot_col = Color32::from_rgba_unmultiplied(255, 255, 255, 18);
    let mut gx = rect.left() + ox;
    while gx < rect.right() {
        let mut gy = rect.top() + oy;
        while gy < rect.bottom() {
            painter.circle_filled(Pos2::new(gx, gy), 1.0, dot_col);
            gy += step;
        }
        gx += step;
    }

    let canvas_resp = ui.interact(rect, ui.scope_id().with("vfx_canvas_bg"), Sense::click_and_drag());

    // Zoom with scroll wheel & pan with middle-button drag
    if rect.contains(ui.input(|i| i.pointer.hover_pos()).unwrap_or(Pos2::new(-1.0, -1.0))) {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            zoom = (zoom * (1.0 + scroll * 0.0015)).clamp(0.45, 2.2);
        }
        if ui.input(|i| i.pointer.middle_down()) {
            pan += ui.input(|i| i.pointer.delta());
        }
    }

    let to_screen = |gp: [f32; 2]| -> Pos2 { Pos2::new(rect.left() + pan.x + gp[0] * zoom, rect.top() + pan.y + gp[1] * zoom) };
    let to_graph = |sp: Pos2| -> [f32; 2] { [(sp.x - rect.left() - pan.x) / zoom, (sp.y - rect.top() - pan.y) / zoom] };

    // Compute MediaIn & MediaOut positions relative to graph extents
    let max_x = graph.nodes.iter().map(|n| n.pos[0]).fold(120.0f32, f32::max);
    let media_in_rect = Rect::from_min_size(to_screen([12.0, 128.0]), Vec2::new(IO_W * zoom, IO_H * zoom));
    let media_out_rect = Rect::from_min_size(to_screen([max_x + 220.0, 128.0]), Vec2::new(IO_W * zoom, IO_H * zoom));

    // Draw Underlays first (behind everything)
    for ul in &graph.underlays {
        let ur = Rect::from_min_size(to_screen([ul.rect[0], ul.rect[1]]), Vec2::new(ul.rect[2] * zoom, ul.rect[3] * zoom));
        let fill = Color32::from_rgba_unmultiplied(ul.color[0], ul.color[1], ul.color[2], 38);
        let stroke = Color32::from_rgba_unmultiplied(ul.color[0], ul.color[1], ul.color[2], 140);
        painter.rect(ur, CornerRadius::same(6), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        painter.text(ur.left_top() + Vec2::new(8.0, 4.0), egui::Align2::LEFT_TOP, &ul.title, egui::FontId::proportional(11.0 * zoom), stroke);
        let close_r = Rect::from_min_size(Pos2::new(ur.right() - 18.0, ur.top() + 2.0), Vec2::splat(15.0));
        let cresp = ui.interact(close_r, ui.scope_id().with(("vfx_ul_del", ul.id)), Sense::click());
        painter.text(close_r.center(), egui::Align2::CENTER_CENTER, "×", egui::FontId::proportional(11.0), stroke);
        if cresp.clicked() {
            let _ = app.session.execute("vfxGraph.removeUnderlay", json!({"clip": clip_id.0, "underlay": ul.id}));
        }
    }

    // Map node rects
    let mut node_rects: HashMap<VfxNodeId, Rect> = HashMap::with_capacity(graph.nodes.len());
    for n in &graph.nodes {
        let r = if matches!(n.kind, VfxNodeKind::PipeRouter) {
            Rect::from_center_size(to_screen(n.pos), Vec2::splat(ROUTER_R * 2.0 * zoom))
        } else {
            Rect::from_min_size(to_screen(n.pos), Vec2::new(NODE_W * zoom, NODE_H * zoom))
        };
        node_rects.insert(n.id, r);
    }

    let port_pos = |ep: VfxEndpoint, slot: Option<VfxInputSlot>| -> Option<Pos2> {
        match (ep, slot) {
            (VfxEndpoint::MediaIn, None) => Some(Pos2::new(media_in_rect.right(), media_in_rect.center().y)),
            (VfxEndpoint::MediaOut, Some(_)) => Some(Pos2::new(media_out_rect.left(), media_out_rect.center().y)),
            (VfxEndpoint::Node(id), None) => {
                let r = *node_rects.get(&id)?;
                Some(Pos2::new(r.right(), r.center().y))
            }
            (VfxEndpoint::Node(id), Some(sl)) => {
                let r = *node_rects.get(&id)?;
                match sl {
                    VfxInputSlot::Main => Some(Pos2::new(r.left(), r.center().y)),
                    VfxInputSlot::Foreground => Some(Pos2::new(r.center().x - 14.0 * zoom, r.top())),
                    VfxInputSlot::EffectMask => Some(Pos2::new(r.center().x + 14.0 * zoom, r.top())),
                }
            }
            _ => None,
        }
    };

    // Draw dashed green Instance link lines
    for n in &graph.nodes {
        if let VfxNodeKind::Instance { source, .. } = &n.kind
            && let (Some(a), Some(b)) = (node_rects.get(source), node_rects.get(&n.id))
        {
            draw_dashed_line(&painter, a.center(), b.center(), Stroke::new(1.5, Color32::from_rgba_unmultiplied(16, 185, 129, 175)));
        }
    }

    // Track hover wire for Shift+Drag splice preview
    let ptr = ui.input(|i| i.pointer.hover_pos());
    let shift_held = ui.input(|i| i.modifiers.shift);
    let ctrl_held = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
    let hovered_splice_edge: Option<VfxEdge> = ptr.and_then(|p| {
        if !shift_held || ctrl_held {
            return None;
        }
        let mut best: Option<(f32, VfxEdge)> = None;
        for &e in &graph.edges {
            if let (Some(p0), Some(p1)) = (port_pos(e.from, None), port_pos(e.to, Some(e.to_slot))) {
                let d = bezier_wire_distance(p, p0, p1, 16);
                if d < 22.0 && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, e));
                }
            }
        }
        best.map(|(_, e)| e)
    });

    // Draw all DAG wires
    let wire_w = (2.2 * zoom).clamp(1.3, 3.2);
    for &edge in &graph.edges {
        if let (Some(p0), Some(p1)) = (port_pos(edge.from, None), port_pos(edge.to, Some(edge.to_slot))) {
            let enabled = match edge.to {
                VfxEndpoint::Node(id) => graph.node(id).is_none_or(|n| n.enabled),
                _ => true,
            };
            let is_splice = hovered_splice_edge == Some(edge);
            let col = if is_splice {
                Color32::from_rgb(56, 224, 255)
            } else if !enabled {
                Color32::from_rgb(90, 95, 108)
            } else {
                match edge.to_slot {
                    VfxInputSlot::Main => COL_MAIN,
                    VfxInputSlot::Foreground => COL_FG,
                    VfxInputSlot::EffectMask => COL_MASK,
                }
            };
            draw_bezier_wire(&painter, p0, p1, Stroke::new(if is_splice { wire_w + 1.4 } else { wire_w }, col));
        }
    }

    // Draw MediaIn & MediaOut fixed nodes
    paint_io_card(&painter, media_in_rect, "MediaIn", Color32::from_rgb(59, 130, 246), zoom);
    paint_io_card(&painter, media_out_rect, "MediaOut", Color32::from_rgb(245, 158, 11), zoom);

    // Draw all VFX Node Cards
    let rename_id = ui.scope_id().with("vfx_rename_node");
    let mut renaming: Option<(u64, String)> = ui.ctx().data(|d| d.get_temp(rename_id));
    for node in &graph.nodes {
        let Some(&nr) = node_rects.get(&node.id) else { continue };
        paint_vfx_node_card(app, ui, &painter, t, clip_id, graph, node, nr, zoom, &to_graph, hovered_splice_edge, &mut renaming);
    }
    ui.ctx().data_mut(|d| {
        if let Some(r) = renaming {
            d.insert_temp(rename_id, r);
        } else {
            d.remove::<(u64, String)>(rename_id);
        }
    });

    // Interactive ports (connect/disconnect)
    handle_vfx_ports(app, ui, &painter, clip_id, graph, &port_pos, zoom);

    // Ctrl+Shift+Drag Knife Wire Cutter
    handle_knife_cut(app, ui, &painter, clip_id, graph, &canvas_resp, &port_pos, ctrl_held && shift_held);

    // Drag-and-drop from Effects panel
    if let Some(effect_id) = ui.ctx().memory(|m| m.data.get_temp::<String>(egui::Id::unique("filmcraft_drag_effect")))
        && !ui.input(|i| i.pointer.any_down())
        && let Some(p) = ptr
        && rect.contains(p)
    {
        let gp = to_graph(p);
        let _ = app.session.execute("vfxGraph.addNode", json!({"clip": clip_id.0, "kind": "effect", "effect": effect_id, "pos": gp}));
        ui.ctx().memory_mut(|m| m.data.remove::<String>(egui::Id::unique("filmcraft_drag_effect")));
    }

    // Keyboard shortcuts when canvas is hovered or focused and no text box is focused
    let hovered = ptr.is_some_and(|p| rect.contains(p));
    if hovered && !ui.ctx().egui_wants_keyboard_input() {
        handle_vfx_shortcuts(app, ui, clip_id, graph, popup_id, rename_id);
    }

    ui.ctx().data_mut(|d| {
        d.insert_temp(zoom_id, zoom);
        d.insert_temp(pan_id, pan);
    });
}

fn paint_io_card(painter: &egui::Painter, rect: Rect, label: &str, accent: Color32, zoom: f32) {
    painter.rect(rect, CornerRadius::same(6), Color32::from_rgb(33, 35, 42), Stroke::new(1.5, accent), StrokeKind::Inside);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional((11.5 * zoom).clamp(8.5, 16.0)), Color32::WHITE);
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn paint_vfx_node_card(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    t: &Tokens,
    clip_id: ClipId,
    graph: &VfxGraph,
    node: &VfxNode,
    nr: Rect,
    zoom: f32,
    to_graph: &dyn Fn(Pos2) -> [f32; 2],
    hovered_splice_edge: Option<VfxEdge>,
    renaming: &mut Option<(u64, String)>,
) {
    let is_active = app.ui.vfx_active_node_id == Some(node.id.0);
    let accent = node_accent(&node.kind);
    let resp = ui.interact(nr, ui.scope_id().with(("vfx_node_card", clip_id.0, node.id.0)), Sense::click_and_drag());
    app.auto.add(&format!("vfxNodes.node.{}", node.id.0), nr, "VFX Node");

    if resp.clicked() {
        select_vfx_node(app, clip_id, node, graph);
    }

    // Drag node (Shift+Drag onto wire splices; Shift+Drag away extracts; normal drag moves pos)
    if resp.dragged_by(egui::PointerButton::Primary) {
        let delta = resp.drag_delta();
        let next_pos = [node.pos[0] + delta.x / zoom, node.pos[1] + delta.y / zoom];
        let _ = app.session.execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node.id.0, "pos": next_pos, "merge": true}));
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) && ui.input(|i| i.modifiers.shift) {
        if let Some(e) = hovered_splice_edge
            && e.from != VfxEndpoint::Node(node.id)
            && e.to != VfxEndpoint::Node(node.id)
        {
            let _ = app.session.execute(
                "vfxGraph.insertOnEdge",
                json!({"clip": clip_id.0, "node": node.id.0, "from": endpoint_json(e.from), "to": endpoint_json(e.to), "slot": slot_str(e.to_slot)}),
            );
        } else {
            let _ = app.session.execute("vfxGraph.extractNode", json!({"clip": clip_id.0, "node": node.id.0}));
        }
        let _ = to_graph;
    }

    if matches!(node.kind, VfxNodeKind::PipeRouter) {
        let fill = if is_active { accent } else { Color32::from_rgb(56, 58, 68) };
        painter.circle(nr.center(), nr.width() * 0.45, fill, Stroke::new(1.5, if is_active { Color32::WHITE } else { accent }));
        return;
    }

    // Card background + accent stripe
    let bg = if !node.enabled {
        Color32::from_rgb(30, 31, 36)
    } else if is_active {
        Color32::from_rgb(44, 48, 60)
    } else {
        Color32::from_rgb(34, 36, 44)
    };
    let border = if is_active {
        Stroke::new(2.0, accent)
    } else {
        Stroke::new(1.0, Color32::from_rgb(66, 70, 82))
    };
    painter.rect(nr, CornerRadius::same(6), bg, border, StrokeKind::Inside);

    let stripe = Rect::from_min_max(nr.left_top(), Pos2::new(nr.left() + (4.0 * zoom).max(3.0), nr.bottom()));
    painter.rect_filled(stripe, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, if node.enabled { accent } else { t.text_dim });

    // Pass-Through LED dot (top-left)
    let led_center = Pos2::new(nr.left() + 14.0 * zoom, nr.top() + 13.0 * zoom);
    let led_rect = Rect::from_center_size(led_center, Vec2::splat(11.0 * zoom));
    let led_resp = ui.interact(led_rect, ui.scope_id().with(("vfx_node_pt", node.id.0)), Sense::click());
    painter.circle_filled(led_center, 3.8 * zoom, if node.enabled { Color32::from_rgb(52, 211, 153) } else { Color32::from_rgb(95, 100, 112) });
    if led_resp.clicked() {
        let _ = app.session.execute("vfxGraph.togglePassThrough", json!({"clip": clip_id.0, "node": node.id.0}));
    }

    // Node Title (or inline rename input)
    let title = node_display_name(node, graph);
    let title_pos = Pos2::new(nr.left() + 23.0 * zoom, nr.top() + 7.0 * zoom);
    let is_renaming = renaming.as_ref().is_some_and(|(rid, _)| *rid == node.id.0);
    if is_renaming {
        if let Some((_, buf)) = renaming.as_mut() {
            let edit_r = Rect::from_min_size(title_pos, Vec2::new(nr.width() - 32.0 * zoom, 18.0 * zoom));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_r));
            let te = child.add(egui::TextEdit::singleline(buf).desired_width(edit_r.width()));
            te.request_focus();
            if te.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let lbl = buf.clone();
                let _ = app.session.execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node.id.0, "label": lbl}));
                *renaming = None;
            }
        }
    } else {
        painter.text(
            title_pos,
            egui::Align2::LEFT_TOP,
            &title,
            egui::FontId::proportional((11.5 * zoom).clamp(8.5, 16.0)),
            if node.enabled { Color32::WHITE } else { t.text_dim },
        );
    }

    // Badges row (bottom-left) + Viewer dots `1` & `2` (bottom-right)
    let mut badges: Vec<(&str, Color32)> = Vec::new();
    if !node.enabled {
        badges.push(("PT", Color32::from_rgb(148, 163, 184)));
    }
    if let VfxNodeKind::Effect(fx) = &node.kind {
        if filmcraft_render::gpufx::GPU_EFFECTS.contains(&fx.effect.as_str()) {
            badges.push(("GPU", Color32::from_rgb(250, 204, 21)));
        }
        if fx.params.values().any(Param::is_animated) || node.settings.blend.is_animated() {
            badges.push(("KF", Color32::from_rgb(96, 165, 250)));
        }
    } else if node.settings.blend.is_animated() {
        badges.push(("KF", Color32::from_rgb(96, 165, 250)));
    }
    if matches!(node.kind, VfxNodeKind::Instance { .. }) {
        badges.push(("INST", Color32::from_rgb(16, 185, 129)));
    }
    let mut bx = nr.left() + 10.0 * zoom;
    let by = nr.bottom() - 15.0 * zoom;
    for (lbl, col) in badges {
        let br = Rect::from_min_size(Pos2::new(bx, by), Vec2::new((lbl.len() as f32 * 6.0 + 6.0) * zoom, 12.0 * zoom));
        painter.rect_filled(br, CornerRadius::same(3), col.gamma_multiply(0.22));
        painter.text(br.center(), egui::Align2::CENTER_CENTER, lbl, egui::FontId::proportional((8.5 * zoom).max(7.0)), col);
        bx += br.width() + 3.0 * zoom;
    }

    // Viewer dots [1] [2] (bottom-right)
    for v_idx in 0..2 {
        let vc = Pos2::new(nr.right() - (22.0 - v_idx as f32 * 12.0) * zoom, nr.bottom() - 9.0 * zoom);
        let vr = Rect::from_center_size(vc, Vec2::splat(10.0 * zoom));
        let active_v = app.ui.vfx_viewer_nodes[v_idx] == Some((clip_id.0, node.id.0));
        let vresp = ui.interact(vr, ui.scope_id().with(("vfx_viewer_dot", node.id.0, v_idx)), Sense::click());
        painter.circle(
            vc,
            3.8 * zoom,
            if active_v { Color32::WHITE } else { Color32::from_rgb(55, 58, 68) },
            Stroke::new(1.0, Color32::from_rgb(130, 136, 150)),
        );
        if vresp.clicked() {
            app.ui.vfx_viewer_nodes[v_idx] = if active_v { None } else { Some((clip_id.0, node.id.0)) };
        }
    }

    // Context menu
    resp.context_menu(|ui| {
        if ui.button("Pass-Through (Ctrl+P)").clicked() {
            let _ = app.session.execute("vfxGraph.togglePassThrough", json!({"clip": clip_id.0, "node": node.id.0}));
            ui.close();
        }
        if matches!(node.kind, VfxNodeKind::Merge { .. }) && ui.button("Intercambiar BG / FG (Ctrl+T)").clicked() {
            let _ = app.session.execute("vfxGraph.swapInputs", json!({"clip": clip_id.0, "node": node.id.0}));
            ui.close();
        }
        if matches!(node.kind, VfxNodeKind::Effect(_) | VfxNodeKind::Instance { .. }) && ui.button("Crear Instancia vinculada").clicked() {
            let _ = app.session.execute("vfxGraph.createInstance", json!({"clip": clip_id.0, "source": node.id.0}));
            ui.close();
        }
        if ui.button("Duplicar nodo").clicked() {
            let _ = app.session.execute("vfxGraph.duplicateNode", json!({"clip": clip_id.0, "node": node.id.0}));
            ui.close();
        }
        if ui.button("Extraer del flujo (Shift+Drag)").clicked() {
            let _ = app.session.execute("vfxGraph.extractNode", json!({"clip": clip_id.0, "node": node.id.0}));
            ui.close();
        }
        if ui.button("Renombrar (F2)").clicked() {
            *renaming = Some((node.id.0, title));
            ui.close();
        }
        ui.separator();
        if ui.button("Eliminar nodo").clicked() {
            let _ = app.session.execute("vfxGraph.removeNode", json!({"clip": clip_id.0, "node": node.id.0}));
            ui.close();
        }
    });
}

fn handle_vfx_ports(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    clip_id: ClipId,
    graph: &VfxGraph,
    port_pos: &dyn Fn(VfxEndpoint, Option<VfxInputSlot>) -> Option<Pos2>,
    zoom: f32,
) {
    let pending_id = egui::Id::unique("vfx_nodes_pending_wire");
    let mut pending: Option<PendingVfxWire> = ui.ctx().data(|d| d.get_temp(pending_id));
    let pin_r = (5.5 * zoom).clamp(4.0, 8.0);

    // Output ports: MediaIn + every node
    let mut outputs: Vec<(VfxEndpoint, VfxPortKind)> = vec![(VfxEndpoint::MediaIn, VfxPortKind::Image)];
    for n in &graph.nodes {
        let kind = if matches!(n.kind, VfxNodeKind::Mask(_)) { VfxPortKind::Mask } else { VfxPortKind::Image };
        outputs.push((VfxEndpoint::Node(n.id), kind));
    }

    for (ep, kind) in outputs {
        let Some(p) = port_pos(ep, None) else { continue };
        let r = Rect::from_center_size(p, Vec2::splat(pin_r * 2.2));
        let resp = ui.interact(r, ui.scope_id().with(("vfx_out", endpoint_json(ep).to_string())), Sense::click_and_drag());
        let col = if kind == VfxPortKind::Mask { COL_MASK } else { COL_OUT };
        painter.rect_filled(Rect::from_center_size(p, Vec2::splat(pin_r * 1.4)), CornerRadius::same(2), col);
        if resp.drag_started() || resp.clicked() {
            pending = Some(PendingVfxWire { from: ep, kind, start_pos: p });
        }
    }

    // Input ports: MediaOut + every node
    let mut inputs: Vec<(VfxEndpoint, VfxInputSlot, Color32)> = vec![(VfxEndpoint::MediaOut, VfxInputSlot::Main, COL_MAIN)];
    for n in &graph.nodes {
        if !matches!(n.kind, VfxNodeKind::Mask(_)) {
            inputs.push((VfxEndpoint::Node(n.id), VfxInputSlot::Main, COL_MAIN));
        }
        if matches!(n.kind, VfxNodeKind::Merge { .. }) {
            inputs.push((VfxEndpoint::Node(n.id), VfxInputSlot::Foreground, COL_FG));
        }
        if !matches!(n.kind, VfxNodeKind::Mask(_) | VfxNodeKind::PipeRouter) {
            inputs.push((VfxEndpoint::Node(n.id), VfxInputSlot::EffectMask, COL_MASK));
        }
    }

    let ptr = ui.input(|i| i.pointer.hover_pos());
    let mut drop_target: Option<(VfxEndpoint, VfxInputSlot)> = None;

    for (ep, slot, col) in inputs {
        let Some(p) = port_pos(ep, Some(slot)) else { continue };
        let r = Rect::from_center_size(p, Vec2::splat(pin_r * 2.4));
        let resp = ui.interact(r, ui.scope_id().with(("vfx_in", endpoint_json(ep).to_string(), slot_str(slot))), Sense::click());
        // Paint Fusion-style directional triangle
        let tri = match slot {
            VfxInputSlot::Main => vec![Pos2::new(p.x - pin_r, p.y - pin_r * 0.8), Pos2::new(p.x + pin_r, p.y), Pos2::new(p.x - pin_r, p.y + pin_r * 0.8)],
            _ => vec![Pos2::new(p.x - pin_r * 0.8, p.y - pin_r), Pos2::new(p.x + pin_r * 0.8, p.y - pin_r), Pos2::new(p.x, p.y + pin_r)],
        };
        painter.add(egui::Shape::convex_polygon(tri, col, Stroke::NONE));

        if ptr.is_some_and(|hp| hp.distance(p) <= pin_r * 2.2) {
            drop_target = Some((ep, slot));
        }
        if (resp.clicked() && ui.input(|i| i.modifiers.alt)) || resp.secondary_clicked() {
            let _ = app.session.execute("vfxGraph.disconnect", json!({"clip": clip_id.0, "to": endpoint_json(ep), "slot": slot_str(slot)}));
        } else if resp.clicked()
            && let Some(pw) = pending.take()
        {
            let _ = app.session.execute(
                "vfxGraph.connect",
                json!({"clip": clip_id.0, "from": endpoint_json(pw.from), "to": endpoint_json(ep), "slot": slot_str(slot), "kind": if pw.kind == VfxPortKind::Mask { "mask" } else { "image" }}),
            );
        }
    }

    if let Some(pw) = pending {
        if let Some(hp) = ptr {
            let col = if pw.kind == VfxPortKind::Mask { COL_MASK } else { COL_MAIN };
            draw_bezier_wire(painter, pw.start_pos, hp, Stroke::new(2.0, col.gamma_multiply(0.85)));
        }
        if ui.input(|i| i.pointer.primary_released()) {
            if let Some((to_ep, slot)) = drop_target {
                let _ = app.session.execute(
                    "vfxGraph.connect",
                    json!({"clip": clip_id.0, "from": endpoint_json(pw.from), "to": endpoint_json(to_ep), "slot": slot_str(slot), "kind": if pw.kind == VfxPortKind::Mask { "mask" } else { "image" }}),
                );
            }
            pending = None;
        }
    }

    ui.ctx().data_mut(|d| {
        if let Some(pw) = pending {
            d.insert_temp(pending_id, pw);
        } else {
            d.remove::<PendingVfxWire>(pending_id);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_knife_cut(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    clip_id: ClipId,
    graph: &VfxGraph,
    canvas_resp: &egui::Response,
    port_pos: &dyn Fn(VfxEndpoint, Option<VfxInputSlot>) -> Option<Pos2>,
    knife_mod: bool,
) {
    let knife_id = egui::Id::unique("vfx_nodes_knife_cut");
    let mut knife: Option<KnifeCut> = ui.ctx().data(|d| d.get_temp(knife_id));
    if knife_mod && canvas_resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(p) = ui.input(|i| i.pointer.interact_pos())
    {
        knife = Some(KnifeCut { start: p, current: p });
    }
    if let Some(k) = knife.as_mut() {
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            k.current = p;
        }
        draw_dashed_line(painter, k.start, k.current, Stroke::new(2.0, Color32::from_rgb(248, 113, 113)));
        if ui.input(|i| i.pointer.primary_released()) {
            let (s0, s1) = (k.start, k.current);
            for &e in &graph.edges {
                if let (Some(p0), Some(p1)) = (port_pos(e.from, None), port_pos(e.to, Some(e.to_slot)))
                    && bezier_intersects_segment(p0, p1, s0, s1, 16)
                {
                    let _ = app.session.execute("vfxGraph.disconnect", json!({"clip": clip_id.0, "to": endpoint_json(e.to), "slot": slot_str(e.to_slot)}));
                }
            }
            knife = None;
        }
    }
    ui.ctx().data_mut(|d| {
        if let Some(k) = knife {
            d.insert_temp(knife_id, k);
        } else {
            d.remove::<KnifeCut>(knife_id);
        }
    });
}

fn handle_vfx_shortcuts(app: &mut FilmcraftApp, ui: &mut egui::Ui, clip_id: ClipId, graph: &VfxGraph, popup_id: egui::Id, rename_id: egui::Id) {
    let shift_space = ui.input(|i| i.modifiers.shift && i.key_pressed(egui::Key::Space));
    if shift_space {
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                popup_id,
                SelectToolPopup { open: true, query: String::new(), after_node: app.ui.vfx_active_node_id },
            );
        });
        return;
    }
    let Some(aid) = app.ui.vfx_active_node_id else { return };
    if ui.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::P)) {
        let _ = app.session.execute("vfxGraph.togglePassThrough", json!({"clip": clip_id.0, "node": aid}));
    } else if ui.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::T)) {
        let _ = app.session.execute("vfxGraph.swapInputs", json!({"clip": clip_id.0, "node": aid}));
    } else if ui.input(|i| i.key_pressed(egui::Key::F2)) {
        if let Some(n) = graph.node(VfxNodeId(aid)) {
            let cur = node_display_name(n, graph);
            ui.ctx().data_mut(|d| d.insert_temp(rename_id, (aid, cur)));
        }
    } else if ui.input(|i| !i.modifiers.any() && i.key_pressed(egui::Key::Num1)) {
        let cur = app.ui.vfx_viewer_nodes[0] == Some((clip_id.0, aid));
        app.ui.vfx_viewer_nodes[0] = if cur { None } else { Some((clip_id.0, aid)) };
    } else if ui.input(|i| !i.modifiers.any() && i.key_pressed(egui::Key::Num2)) {
        let cur = app.ui.vfx_viewer_nodes[1] == Some((clip_id.0, aid));
        app.ui.vfx_viewer_nodes[1] = if cur { None } else { Some((clip_id.0, aid)) };
    } else if ui.input(|i| !i.modifiers.any() && (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))) {
        let _ = app.session.execute("vfxGraph.removeNode", json!({"clip": clip_id.0, "node": aid}));
    }
}

fn render_select_tool_popup(app: &mut FilmcraftApp, ui: &mut egui::Ui, clip_id: ClipId, popup_id: egui::Id) {
    let Some(mut st) = ui.ctx().data(|d| d.get_temp::<SelectToolPopup>(popup_id)) else { return };
    if !st.open {
        return;
    }
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Window::new("Select Tool (Shift+Space)")
        .id(popup_id.with("win"))
        .collapsible(false)
        .resizable(false)
        .default_width(300.0)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            let te = ui.add(egui::TextEdit::singleline(&mut st.query).hint_text("Buscar efecto, Merge, Mask, Router…").desired_width(280.0));
            te.request_focus();
            let q = st.query.trim().to_ascii_lowercase();
            let mut choices: Vec<(&'static str, String, &'static str)> = vec![
                ("merge", "Merge (Composición BG + FG)".to_string(), "merge"),
                ("mask", "Mask (Máscara Elipse)".to_string(), "mask"),
                ("pipeRouter", "PipeRouter (Enrutador)".to_string(), "pipeRouter"),
            ];
            for def in filmcraft_project::effect_defs() {
                if !def.intrinsic && def.kind == EffectKind::Video && def.id != "lumetri" && !filmcraft_project::graphic::is_layer_id(def.id) {
                    choices.push(("effect", def.name.to_string(), def.id));
                }
            }
            let filtered: Vec<_> = choices
                .into_iter()
                .filter(|(_, label, id)| q.is_empty() || label.to_ascii_lowercase().contains(&q) || id.to_ascii_lowercase().contains(&q))
                .take(18)
                .collect();

            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for (i, (kind, label, id)) in filtered.iter().enumerate() {
                    let clicked = ui.selectable_label(i == 0, label).clicked() || (enter && i == 0);
                    if clicked {
                        let mut body = json!({"clip": clip_id.0, "kind": kind});
                        if *kind == "effect" {
                            body["effect"] = json!(id);
                        }
                        if let Some(a) = st.after_node {
                            body["after"] = json!(a);
                        }
                        if let Ok(res) = app.session.execute("vfxGraph.addNode", body)
                            && let Some(nid) = res["node"].as_u64()
                        {
                            app.ui.vfx_active_node_id = Some(nid);
                        }
                        close = true;
                    }
                }
            });
        });

    if close {
        ui.ctx().data_mut(|d| d.remove::<SelectToolPopup>(popup_id));
    } else {
        ui.ctx().data_mut(|d| d.insert_temp(popup_id, st));
    }
}

/// Collect ONLY the animatable parameters belonging to the single active [`VfxNode`].
pub(crate) fn active_node_params<'a>(node: &'a VfxNode, graph: &'a VfxGraph) -> Vec<(String, String, Param)> {
    let mut out: Vec<(String, String, Param)> = Vec::new();
    match &node.kind {
        VfxNodeKind::Effect(_) | VfxNodeKind::Instance { .. } => {
            if let Some(fx) = graph.resolve_effect(node.id) {
                if let Some(def) = fx.def() {
                    for pd in &def.params {
                        if pd.animatable
                            && let Some(p) = fx.param(pd.id)
                        {
                            out.push((pd.id.to_string(), pd.label.to_string(), p.clone()));
                        }
                    }
                } else {
                    for (k, p) in &fx.params {
                        out.push((k.clone(), k.clone(), p.clone()));
                    }
                }
            }
        }
        VfxNodeKind::Merge { center, size, angle, .. } => {
            out.push(("center".into(), "Center".into(), center.clone()));
            out.push(("size".into(), "Size".into(), size.clone()));
            out.push(("angle".into(), "Angle".into(), angle.clone()));
        }
        VfxNodeKind::Mask(m) => {
            out.push(("feather".into(), "Feather".into(), m.feather.clone()));
            out.push(("opacity".into(), "Opacity".into(), m.opacity.clone()));
            out.push(("expansion".into(), "Expansion".into(), m.expansion.clone()));
        }
        VfxNodeKind::PipeRouter => {}
    }
    out.push(("__settings.blend".into(), "Settings ▸ Blend".into(), node.settings.blend.clone()));
    out
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn render_bottom_drawer(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    clip_id: ClipId,
    graph: &VfxGraph,
    active_node: Option<&VfxNode>,
    mode: u8,
    height: f32,
    clip_start: Tick,
    clip_dur: Tick,
    in_point: Tick,
    speed: f64,
) {
    let Some(node) = active_node else {
        ui.allocate_ui(Vec2::new(ui.available_width(), height), |ui| {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("Selecciona un nodo para ver exclusivamente sus keyframes y curvas").color(t.text_dim));
            });
        });
        return;
    };

    let title = node_display_name(node, graph);
    let params = active_node_params(node, graph);
    let ph = app.session.playhead();
    let dur_ticks = clip_dur.0.max(1) as f64;
    let ph_frac = ((ph.0 - clip_start.0) as f64 / dur_ticks).clamp(0.0, 1.0) as f32;
    let spd = if speed.abs() > 1e-6 { speed } else { 1.0 };

    ui.allocate_ui(Vec2::new(ui.available_width(), height), |ui| {
        ui.horizontal(|ui| {
            let tag = if mode == 1 { "Keyframes del nodo:" } else { "Spline del nodo:" };
            ui.label(egui::RichText::new(format!("{tag} {title}")).strong().size(11.5).color(t.accent));
            ui.label(egui::RichText::new(format!("({} parámetros aislados)", params.len())).size(10.5).color(t.text_dim));
        });

        let avail = ui.available_rect_before_wrap();
        if mode == 1 {
            // Isolated per-node Keyframes dope-sheet
            let label_w = (avail.width() * 0.30).clamp(130.0, 220.0);
            egui::ScrollArea::vertical().id_salt("vfx_node_kf_drawer").show(ui, |ui| {
                for (pid, pname, prm) in &params {
                    let row_h = 22.0;
                    let (row_r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
                    let left_r = Rect::from_min_max(row_r.left_top(), Pos2::new(row_r.left() + label_w, row_r.bottom()));
                    let lane_r = Rect::from_min_max(Pos2::new(left_r.right() + 4.0, row_r.top()), row_r.right_bottom());

                    let p = ui.painter();
                    p.rect_filled(lane_r, CornerRadius::same(2), Color32::from_rgb(24, 26, 32));
                    let kf_btn_r = Rect::from_center_size(Pos2::new(left_r.left() + 10.0, left_r.center().y), Vec2::splat(14.0));
                    let kf_resp = ui.interact(kf_btn_r, ui.scope_id().with(("vfx_kf_tog", node.id.0, pid)), Sense::click());
                    let anim_col = if prm.is_animated() { Color32::from_rgb(96, 165, 250) } else { t.text_dim };
                    p.text(kf_btn_r.center(), egui::Align2::CENTER_CENTER, "◆", egui::FontId::proportional(10.0), anim_col);
                    p.text(Pos2::new(left_r.left() + 22.0, left_r.center().y), egui::Align2::LEFT_CENTER, pname, egui::FontId::proportional(11.0), t.text);

                    if kf_resp.clicked() {
                        if pid == "__settings.blend" {
                            let _ = app.session.execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node.id.0, "blendToggleAnimation": true}));
                        } else {
                            let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": pid, "toggleAnimation": true}));
                        }
                    }

                    // Draw keyframes for this parameter only
                    for kf in &prm.keyframes {
                        let tl_ticks = ((kf.time.0 - in_point.0) as f64 / spd) as i64;
                        let frac = (tl_ticks as f64 / dur_ticks).clamp(0.0, 1.0) as f32;
                        let kx = lane_r.left() + frac * lane_r.width();
                        let ky = lane_r.center().y;
                        let d = 4.5f32;
                        let pts = vec![Pos2::new(kx, ky - d), Pos2::new(kx + d, ky), Pos2::new(kx, ky + d), Pos2::new(kx - d, ky)];
                        p.add(egui::Shape::convex_polygon(pts, Color32::from_rgb(96, 165, 250), Stroke::new(1.0, Color32::WHITE)));
                    }

                    // Playhead line + click to seek
                    let ph_x = lane_r.left() + ph_frac * lane_r.width();
                    p.line_segment([Pos2::new(ph_x, lane_r.top()), Pos2::new(ph_x, lane_r.bottom())], Stroke::new(1.2, Color32::from_rgb(239, 68, 68)));
                    let lane_resp = ui.interact(lane_r, ui.scope_id().with(("vfx_kf_lane", node.id.0, pid)), Sense::click_and_drag());
                    if (lane_resp.clicked() || lane_resp.dragged())
                        && let Some(hp) = ui.input(|i| i.pointer.interact_pos())
                    {
                        let f = ((hp.x - lane_r.left()) / lane_r.width().max(1.0)).clamp(0.0, 1.0) as f64;
                        let target_t = Tick(clip_start.0 + (f * dur_ticks) as i64);
                        let _ = app.session.execute("transport.seek", json!({"time": target_t.0}));
                    }
                }
            });
        } else {
            // Mode 2: Isolated per-node Spline Curve Viewer
            let graph_r = Rect::from_min_size(avail.left_top(), Vec2::new(avail.width(), (avail.height() - 4.0).max(60.0)));
            let gresp = ui.interact(graph_r, ui.scope_id().with("vfx_node_spline_canvas"), Sense::click_and_drag());
            let p = ui.painter_at(graph_r);
            p.rect_filled(graph_r, CornerRadius::same(4), Color32::from_rgb(22, 24, 30));
            for i in 1..4 {
                let y = graph_r.top() + graph_r.height() * (i as f32 / 4.0);
                p.line_segment([Pos2::new(graph_r.left(), y), Pos2::new(graph_r.right(), y)], Stroke::new(0.8, Color32::from_rgba_unmultiplied(255, 255, 255, 18)));
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
                let mut pts = Vec::with_capacity(48);
                for s in 0..=48 {
                    let frac = s as f32 / 48.0;
                    let mt = Tick(in_point.0 + (frac as f64 * dur_ticks * spd) as i64);
                    let v = prm.f64_at(mt);
                    let ny = 1.0 - ((v - vmin) / span).clamp(0.0, 1.0) as f32;
                    pts.push(Pos2::new(graph_r.left() + frac * graph_r.width(), graph_r.top() + 8.0 + ny * (graph_r.height() - 16.0)));
                }
                p.add(egui::Shape::line(pts, Stroke::new(1.8, col)));
                p.text(Pos2::new(graph_r.left() + 8.0, graph_r.top() + 6.0 + (curve_idx - 1) as f32 * 13.0), egui::Align2::LEFT_TOP, pname, egui::FontId::proportional(10.0), col);
            }
            if curve_idx == 0 {
                p.text(graph_r.center(), egui::Align2::CENTER_CENTER, "Activa keyframes (◆) en un parámetro del nodo para editar su curva Spline", egui::FontId::proportional(11.0), t.text_dim);
            }
            let ph_x = graph_r.left() + ph_frac * graph_r.width();
            p.line_segment([Pos2::new(ph_x, graph_r.top()), Pos2::new(ph_x, graph_r.bottom())], Stroke::new(1.4, Color32::from_rgb(239, 68, 68)));
            if (gresp.clicked() || gresp.dragged())
                && let Some(hp) = ui.input(|i| i.pointer.interact_pos())
            {
                let f = ((hp.x - graph_r.left()) / graph_r.width().max(1.0)).clamp(0.0, 1.0) as f64;
                let target_t = Tick(clip_start.0 + (f * dur_ticks) as i64);
                let _ = app.session.execute("transport.seek", json!({"time": target_t.0}));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_bezier_and_knife_intersection_geometry_works() {
        let p0 = Pos2::new(0.0, 50.0);
        let p3 = Pos2::new(200.0, 50.0);
        assert!(bezier_wire_distance(Pos2::new(100.0, 52.0), p0, p3, 16) < 3.0);
        assert!(bezier_wire_distance(Pos2::new(100.0, 120.0), p0, p3, 16) > 50.0);
        assert!(bezier_intersects_segment(p0, p3, Pos2::new(100.0, 0.0), Pos2::new(100.0, 100.0), 16));
        assert!(!bezier_intersects_segment(p0, p3, Pos2::new(100.0, 0.0), Pos2::new(100.0, 20.0), 16));
    }

    #[test]
    fn active_node_params_isolates_only_selected_node() {
        let mut g = VfxGraph::from_serial_effects(&[
            filmcraft_project::find_effect("invert").unwrap().instance(),
            filmcraft_project::find_effect("gaussian_blur").unwrap().instance(),
        ]);
        let n1 = g.node(VfxNodeId(1)).unwrap();
        let p1 = active_node_params(n1, &g);
        assert!(p1.iter().any(|(id, _, _)| id == "__settings.blend"));
        assert!(!p1.iter().any(|(id, _, _)| id == "blurriness"));

        let n2 = g.node(VfxNodeId(2)).unwrap();
        let p2 = active_node_params(n2, &g);
        assert!(p2.iter().any(|(id, _, _)| id == "blurriness"));
        assert!(p2.iter().any(|(id, _, _)| id == "__settings.blend"));
        let _ = &mut g;
    }
}
