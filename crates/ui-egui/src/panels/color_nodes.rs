//! DaVinci Resolve-style Node Graph panel for color grading.
//!
//! Visualizes Lumetri color grading layers as an interactive serial node chain:
//! `[ IN ] -> [ Node 01 ] -> [ Node 02 ] -> ... -> [ OUT ]`
//!
//! Keyboard shortcuts:
//! - `Alt + S`: Add Serial Node
//! - `Ctrl + D` / `Cmd + D`: Bypass Active Node
//! - `Shift + D`: Bypass All Nodes
//! - `Delete` / `Backspace`: Delete Active Node
//! - `Alt + Home`: Reset Active Node
//! - `Alt + F` / `Z`: Fit all nodes to canvas (Ajustar)
//! - `Ctrl + =` / `Ctrl + +`: Zoom In
//! - `Ctrl + -`: Zoom Out
//! - `Ctrl + 0`: Reset Zoom (100%)

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{EffectInstance, ParamValue};
use serde_json::json;

use crate::FilmcraftApp;
use crate::panels::lumetri::active_color_clip;
use crate::theme::Tokens;

/// Analyze an effect instance to determine which color modules are active.
pub(crate) fn detect_node_badges(e: &EffectInstance) -> Vec<(&'static str, &'static str)> {
    let mut badges = Vec::new();
    let has_f = |id: &str, def: f64| {
        e.param(id)
            .and_then(|p| match p.value {
                ParamValue::Float(v) => Some((v - def).abs() > 0.001),
                _ => None,
            })
            .unwrap_or(false)
    };
    let has_v2 = |id: &str| {
        e.param(id)
            .and_then(|p| match p.value {
                ParamValue::Vec2(v) => Some(v.x.abs() > 0.001 || v.y.abs() > 0.001),
                _ => None,
            })
            .unwrap_or(false)
    };
    let has_col = |id: &str, def: [f32; 4]| {
        e.param(id)
            .and_then(|p| match p.value {
                ParamValue::Color(c) => Some((0..3).any(|k| (c[k] - def[k]).abs() > 0.002)),
                _ => None,
            })
            .unwrap_or(false)
    };
    let has_txt = |id: &str| {
        e.param(id)
            .and_then(|p| match &p.value {
                ParamValue::Text(s) => Some(!s.is_empty()),
                _ => None,
            })
            .unwrap_or(false)
    };
    let has_choice = |id: &str| {
        e.param(id)
            .and_then(|p| match p.value {
                ParamValue::Choice(c) => Some(c > 0),
                _ => None,
            })
            .unwrap_or(false)
    };
    let has_rgb_curve = |id: &str| e.param(id).and_then(|p| p.value.as_curve()).is_some_and(|pts| pts != [[0.0, 0.0], [1.0, 1.0]] && !pts.is_empty());
    let has_hue_curve = |id: &str| e.param(id).and_then(|p| p.value.as_curve()).is_some_and(|pts| pts.iter().any(|pt| (pt[1] - 0.5).abs() > 0.002));

    if e.param("basic_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && (has_f("exposure", 0.0)
            || has_f("contrast", 0.0)
            || has_f("temperature", 0.0)
            || has_f("tint", 0.0)
            || has_f("highlights", 0.0)
            || has_f("shadows", 0.0)
            || has_f("whites", 0.0)
            || has_f("blacks", 0.0)
            || has_f("saturation", 100.0)
            || has_f("hdr_white", 1000.0)
            || has_f("hdr_specular", 0.0)
            || has_txt("input_lut"))
    {
        badges.push(("PRI", "Primary"));
    }
    if e.param("creative_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && (has_txt("look_lut")
            || has_choice("look")
            || has_f("faded_film", 0.0)
            || has_f("sharpen", 0.0)
            || has_f("vibrance", 0.0)
            || has_f("creative_sat", 100.0)
            || has_col("shadow_tint", [0.5, 0.5, 0.5, 1.0])
            || has_col("highlight_tint", [0.5, 0.5, 0.5, 1.0]))
    {
        badges.push(("LUT", "Creative / LUT"));
    }
    if e.param("curves_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && (has_rgb_curve("curve_luma")
            || has_rgb_curve("curve_red")
            || has_rgb_curve("curve_green")
            || has_rgb_curve("curve_blue")
            || has_hue_curve("hue_vs_sat")
            || has_hue_curve("hue_vs_hue")
            || has_hue_curve("hue_vs_luma")
            || has_hue_curve("luma_vs_sat")
            || has_hue_curve("sat_vs_sat"))
    {
        badges.push(("CRV", "Curves"));
    }
    if e.param("wheels_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && (has_v2("wheel_shadows")
            || has_v2("wheel_midtones")
            || has_v2("wheel_highlights")
            || has_f("wheel_shadows_l", 0.0)
            || has_f("wheel_midtones_l", 0.0)
            || has_f("wheel_highlights_l", 0.0))
    {
        badges.push(("WHL", "Wheels"));
    }
    if e.param("hsl_on").and_then(|p| p.value.as_bool()).unwrap_or(false) {
        badges.push(("HSL", "HSL"));
    }
    if e.param("vignette_on").and_then(|p| p.value.as_bool()).unwrap_or(true) && has_f("vignette_amount", 0.0) {
        badges.push(("VIG", "Vignette"));
    }

    if badges.is_empty() {
        badges.push(("GRADE", "Grade"));
    }
    badges
}

/// Draw a smooth cubic bezier connection line between two pins.
fn draw_bezier_wire(painter: &egui::Painter, p0: Pos2, p1: Pos2, stroke: Stroke) {
    let (c0, c1) = if p1.x >= p0.x {
        let dx = ((p1.x - p0.x) * 0.55).max(30.0);
        (pos2(p0.x + dx, p0.y), pos2(p1.x - dx, p1.y))
    } else {
        // Multi-row wrap or backward wire: S-curve around rows
        let dy = (p1.y - p0.y).abs();
        let bend_x = (dy * 0.35).max(50.0);
        (pos2(p0.x + bend_x, p0.y + dy * 0.2), pos2(p1.x - bend_x, p1.y - dy * 0.2))
    };
    let steps = 28;
    let mut prev = p0;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let u = 1.0 - t;
        let p = pos2(
            u * u * u * p0.x + 3.0 * u * u * t * c0.x + 3.0 * u * t * t * c1.x + t * t * t * p1.x,
            u * u * u * p0.y + 3.0 * u * u * t * c0.y + 3.0 * u * t * t * c1.y + t * t * t * p1.y,
        );
        painter.line_segment([prev, p], stroke);
        prev = p;
    }
}

#[derive(Default)]
struct NodeGraphActions {
    add_serial: bool,
    bypass_active: bool,
    bypass_all: bool,
    delete_active: bool,
    reset_active: bool,
    fit_view: bool,
    zoom_in: bool,
    zoom_out: bool,
    zoom_reset: bool,
}

fn read_shortcuts(ui: &egui::Ui, rect: Rect, node_count: usize) -> NodeGraphActions {
    let mut act = NodeGraphActions::default();
    if ui.ctx().egui_wants_keyboard_input() || !ui.rect_contains_pointer(rect) {
        return act;
    }
    ui.input(|i| {
        if i.modifiers.alt && i.key_pressed(egui::Key::S) {
            act.add_serial = true;
        } else if (i.modifiers.command || i.modifiers.ctrl) && !i.modifiers.shift && i.key_pressed(egui::Key::D) {
            act.bypass_active = true;
        } else if i.modifiers.shift && i.key_pressed(egui::Key::D) {
            act.bypass_all = true;
        } else if (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && node_count > 1 {
            act.delete_active = true;
        } else if i.modifiers.alt && i.key_pressed(egui::Key::Home) {
            act.reset_active = true;
        } else if (i.modifiers.alt && i.key_pressed(egui::Key::F)) || i.key_pressed(egui::Key::Z) {
            act.fit_view = true;
        } else if (i.modifiers.command || i.modifiers.ctrl) && (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
            act.zoom_in = true;
        } else if (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::Minus) {
            act.zoom_out = true;
        } else if (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::Num0) {
            act.zoom_reset = true;
        }
    });
    act
}

struct ToolbarBtn<'a> {
    w: f32,
    h: f32,
    gap: f32,
    id: &'a str,
    auto_id: &'a str,
    auto_label: &'a str,
    label: &'a str,
    tooltip: Option<&'a str>,
    active_tint: bool,
}

fn toolbar_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, bx: &mut f32, by: f32, btn: ToolbarBtn<'_>) -> bool {
    let t = app.tokens;
    let r = Rect::from_min_size(pos2(*bx, by), vec2(btn.w, btn.h));
    let mut resp = ui.interact(r, egui::Id::unique(btn.id), Sense::click());
    if let Some(tip) = btn.tooltip {
        resp = resp.on_hover_text(tip);
    }
    app.auto.add(btn.auto_id, r, btn.auto_label);
    let bg = if resp.hovered() {
        t.hover
    } else if btn.active_tint {
        t.accent.gamma_multiply(0.20)
    } else {
        Color32::from_rgb(36, 40, 48)
    };
    let stroke = if btn.active_tint { t.accent } else { Color32::from_rgb(55, 60, 72) };
    ui.painter().rect_filled(r, 4.0, bg);
    ui.painter().rect_stroke(r, 4.0, Stroke::new(1.0, stroke), StrokeKind::Inside);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, btn.label, Tokens::ui(11.0), t.text);
    *bx += btn.w + btn.gap;
    resp.clicked()
}

fn apply_zoom_step(app: &mut FilmcraftApp, canvas_r: Rect, new_zoom: f32) {
    let old_zoom = app.ui.color_node_zoom;
    let new_zoom = new_zoom.clamp(0.25, 2.5);
    let center = canvas_r.size() * 0.5;
    let old_pan = vec2(app.ui.color_node_pan[0], app.ui.color_node_pan[1]);
    let new_pan = center - (center - old_pan) * (new_zoom / old_zoom);
    app.ui.color_node_pan = [new_pan.x, new_pan.y];
    app.ui.color_node_zoom = new_zoom;
}

fn handle_canvas_zoom_pan(app: &mut FilmcraftApp, ui: &mut egui::Ui, canvas_r: Rect, clip_id: u64, node_count: usize, act: &NodeGraphActions) {
    if act.zoom_in {
        apply_zoom_step(app, canvas_r, app.ui.color_node_zoom * 1.2);
    }
    if act.zoom_out {
        apply_zoom_step(app, canvas_r, app.ui.color_node_zoom / 1.2);
    }
    if act.zoom_reset {
        apply_zoom_step(app, canvas_r, 1.0);
    }

    let canvas_resp = ui.interact(canvas_r, egui::Id::unique("color_nodes_canvas"), Sense::drag());
    if !ui.rect_contains_pointer(canvas_r) {
        return;
    }

    let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll_y != 0.0 {
        let pointer_pos = ui.input(|i| i.pointer.hover_pos()).unwrap_or(canvas_r.center());
        let rel_cursor = pointer_pos - canvas_r.min;
        let old_zoom = app.ui.color_node_zoom;
        let new_zoom = (old_zoom * (scroll_y * 0.003).exp()).clamp(0.25, 2.5);
        if (new_zoom - old_zoom).abs() > 0.001 {
            let old_pan = vec2(app.ui.color_node_pan[0], app.ui.color_node_pan[1]);
            let new_pan = rel_cursor - (rel_cursor - old_pan) * (new_zoom / old_zoom);
            app.ui.color_node_pan = [new_pan.x, new_pan.y];
            app.ui.color_node_zoom = new_zoom;
        }
    }

    let is_node_dragged = (0..node_count).any(|i| ui.ctx().is_being_dragged(egui::Id::unique(("color_node_card", clip_id, i))));
    if !is_node_dragged {
        let pointer_delta = ui.input(|i| i.pointer.delta());
        let is_pan_button = ui.input(|i| {
            i.pointer.button_down(egui::PointerButton::Middle)
                || i.pointer.button_down(egui::PointerButton::Secondary)
                || (i.modifiers.alt && i.pointer.button_down(egui::PointerButton::Primary))
                || (i.key_down(egui::Key::Space) && i.pointer.button_down(egui::PointerButton::Primary))
        });
        if (is_pan_button || canvas_resp.dragged_by(egui::PointerButton::Primary)) && pointer_delta != egui::Vec2::ZERO {
            app.ui.color_node_pan[0] += pointer_delta.x;
            app.ui.color_node_pan[1] += pointer_delta.y;
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;

    // Outer frame with dark background
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(20, 22, 26));

    let active_group = app.ui.active_color_group.and_then(|gid| {
        let seq = app.session.active_sequence()?;
        let g = seq.color_group(gid)?;
        Some((gid, g.name.clone(), g.effects.clone()))
    });

    let (clip, it_name, it_effects, group_id) = if let Some((gid, gname, geffects)) = active_group {
        (filmcraft_project::ClipId(0), format!("Group: {gname}"), geffects, Some(gid))
    } else if let Some((clip, it)) = active_color_clip(app) {
        (clip, it.name, it.effects, None)
    } else {
        crate::dock::placeholder(ui, rect, &t, tl!("(no clip selected)"));
        return;
    };

    let lumetri_effects: Vec<(usize, &EffectInstance)> = it_effects.iter().enumerate().filter(|(_, e)| e.effect == "lumetri").collect();

    let node_count = lumetri_effects.len();
    let active_node = if node_count == 0 { 0 } else { app.ui.active_color_node.min(node_count - 1) };
    app.ui.active_color_node = active_node;

    let mut act = read_shortcuts(ui, rect, node_count);

    let toolbar_r = render_toolbar(app, ui, rect, clip, &it_name, &lumetri_effects, active_node, &mut act);

    if execute_graph_actions(app, clip, group_id, &lumetri_effects, active_node, &act) {
        return;
    }

    // Canvas Area
    let canvas_r = Rect::from_min_max(pos2(rect.min.x, toolbar_r.max.y), rect.max);
    if canvas_r.width() < 10.0 || canvas_r.height() < 10.0 {
        return;
    }

    handle_canvas_zoom_pan(app, ui, canvas_r, clip.0, node_count, &act);

    if node_count == 0 {
        // Prompt to add initial node
        let painter = ui.painter_at(canvas_r).with_clip_rect(canvas_r);
        let center = canvas_r.center();
        painter.text(pos2(center.x, center.y - 20.0), Align2::CENTER_CENTER, tl!("No color grading nodes on active clip"), Tokens::ui(13.0), t.text_dim);
        let btn_r = Rect::from_center_size(pos2(center.x, center.y + 15.0), vec2(180.0, 30.0));
        let btn_resp = ui.interact(btn_r, egui::Id::unique("init_node_btn"), Sense::click());
        app.auto.add("color_nodes.init_node", btn_r, "Add Color Node");
        if btn_resp.hovered() {
            painter.rect_filled(btn_r, 4.0, t.hover);
        } else {
            painter.rect_filled(btn_r, 4.0, t.accent.gamma_multiply(0.20));
        }
        painter.rect_stroke(btn_r, 4.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        painter.text(btn_r.center(), Align2::CENTER_CENTER, tl!("+ Add Initial Node"), Tokens::ui(12.0), t.text);
        if btn_resp.clicked() {
            let _ = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}));
            app.ui.active_color_node = 0;
        }
        return;
    }

    // Base node layout parameters (unscaled)
    let base_node_w = 140.0;
    let base_node_h = 78.0;
    let base_gap = 52.0;
    let base_v_gap = 36.0;
    let base_port_w = 42.0;
    let base_port_h = 32.0;

    let nodes_per_row = if node_count <= 4 { node_count.max(1) } else { 4 };
    let total_rows = node_count.div_ceil(nodes_per_row);
    let max_cols = if total_rows > 1 { nodes_per_row } else { node_count };
    let row_w = max_cols as f32 * base_node_w + (max_cols.saturating_sub(1)) as f32 * base_gap;
    let total_chain_w = base_port_w + base_gap + row_w + base_gap + base_port_w;
    let start_x = (canvas_r.width() - total_chain_w) * 0.5;
    let start_x = start_x.max(base_port_w + base_gap + 20.0);

    let total_h = total_rows as f32 * base_node_h + (total_rows.saturating_sub(1)) as f32 * base_v_gap;
    let start_y = (canvas_r.height() - total_h) * 0.5;
    let start_y = start_y.max(20.0);

    // Auto-fit if many nodes and pristine view or if fit_view requested
    let fit_view = act.fit_view || (node_count > 4 && app.ui.color_node_zoom == 1.0 && app.ui.color_node_pan == [0.0, 0.0]);

    if fit_view && node_count > 0 {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for i in 0..node_count {
            let key = format!("{}:{i}", clip.0);
            let [x, y] = app.ui.color_node_positions.get(&key).copied().unwrap_or_else(|| {
                let r = i / nodes_per_row;
                let c = i % nodes_per_row;
                [start_x + c as f32 * (base_node_w + base_gap), start_y + r as f32 * (base_node_h + base_v_gap)]
            });
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + base_node_w);
            max_y = max_y.max(y + base_node_h);
        }
        min_x -= base_port_w + base_gap;
        max_x += base_port_w + base_gap;

        let graph_w = (max_x - min_x).max(100.0);
        let graph_h = (max_y - min_y).max(80.0);
        let pad = 32.0;
        let avail_w = (canvas_r.width() - pad * 2.0).max(100.0);
        let avail_h = (canvas_r.height() - pad * 2.0).max(80.0);

        let target_zoom = (avail_w / graph_w).min(avail_h / graph_h).clamp(0.28, 1.15);
        let graph_center_x = (min_x + max_x) * 0.5;
        let graph_center_y = (min_y + max_y) * 0.5;
        let canvas_center = canvas_r.size() * 0.5;

        app.ui.color_node_pan = [canvas_center.x - graph_center_x * target_zoom, canvas_center.y - graph_center_y * target_zoom];
        app.ui.color_node_zoom = target_zoom;
    }

    let zoom = app.ui.color_node_zoom.clamp(0.25, 2.5);
    let pan = vec2(app.ui.color_node_pan[0], app.ui.color_node_pan[1]);

    let painter = ui.painter_at(canvas_r).with_clip_rect(canvas_r);

    // Draw DaVinci Resolve-style subtle dot grid panning with canvas
    let base_spacing = 24.0;
    let grid_spacing = base_spacing * zoom;
    let dot_color = Color32::from_rgb(34, 37, 44);
    if grid_spacing >= 12.0 {
        let off_x = ((pan.x % grid_spacing) + grid_spacing) % grid_spacing;
        let off_y = ((pan.y % grid_spacing) + grid_spacing) % grid_spacing;
        let mut gx = canvas_r.min.x + off_x;
        while gx < canvas_r.max.x {
            let mut gy = canvas_r.min.y + off_y;
            while gy < canvas_r.max.y {
                painter.circle_filled(pos2(gx, gy), 1.0, dot_color);
                gy += grid_spacing;
            }
            gx += grid_spacing;
        }
    }

    // Calculate scaled node rects based on stored movable positions or multi-row defaults
    let mut node_rects = Vec::with_capacity(node_count);
    let mut default_rels = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let key = format!("{}:{i}", clip.0);
        let r = i / nodes_per_row;
        let c = i % nodes_per_row;
        let default_rel = [start_x + c as f32 * (base_node_w + base_gap), start_y + r as f32 * (base_node_h + base_v_gap)];
        default_rels.push(default_rel);
        let rel_pos = app.ui.color_node_positions.get(&key).copied().unwrap_or(default_rel);
        let screen_min = canvas_r.min + pan + vec2(rel_pos[0] * zoom, rel_pos[1] * zoom);
        let screen_size = vec2(base_node_w * zoom, base_node_h * zoom);
        node_rects.push(Rect::from_min_size(screen_min, screen_size));
    }

    // Terminals
    let port_w = base_port_w * zoom;
    let port_h = base_port_h * zoom;
    let port_corner = (4.0 * zoom).clamp(2.0, 4.0);

    // Draw [ IN ] Terminal on left of Node 0
    let in_y = node_rects[0].center().y;
    let in_x = node_rects[0].min.x - (base_gap * zoom) - port_w;
    let in_rect = Rect::from_min_size(pos2(in_x, in_y - port_h * 0.5), vec2(port_w, port_h));
    painter.rect_filled(in_rect, port_corner, Color32::from_rgb(28, 30, 36));
    painter.rect_stroke(in_rect, port_corner, Stroke::new(1.0, Color32::from_rgb(50, 54, 64)), StrokeKind::Inside);
    if zoom >= 0.5 {
        painter.text(in_rect.center(), Align2::CENTER_CENTER, "IN", Tokens::ui((11.0 * zoom).clamp(8.0, 11.0)), Color32::from_rgb(140, 150, 170));
    }

    let in_pin = pos2(in_rect.max.x, in_y);
    let term_pin_r = (4.0 * zoom).clamp(2.0, 5.0);
    painter.circle_filled(in_pin, term_pin_r, Color32::from_rgb(60, 200, 80));

    // Draw [ OUT ] Terminal on right of Last Node
    let last_idx = node_count - 1;
    let out_y = node_rects[last_idx].center().y;
    let out_x = node_rects[last_idx].max.x + (base_gap * zoom);
    let out_rect = Rect::from_min_size(pos2(out_x, out_y - port_h * 0.5), vec2(port_w, port_h));
    painter.rect_filled(out_rect, port_corner, Color32::from_rgb(28, 30, 36));
    painter.rect_stroke(out_rect, port_corner, Stroke::new(1.0, Color32::from_rgb(50, 54, 64)), StrokeKind::Inside);
    if zoom >= 0.5 {
        painter.text(out_rect.center(), Align2::CENTER_CENTER, "OUT", Tokens::ui((11.0 * zoom).clamp(8.0, 11.0)), Color32::from_rgb(140, 150, 170));
    }

    let out_pin = pos2(out_rect.min.x, out_y);
    painter.circle_filled(out_pin, term_pin_r, Color32::from_rgb(60, 200, 80));

    // Draw connecting wires
    let wire_stroke_w = (2.0 * zoom).clamp(1.0, 3.0);
    // 1. IN terminal to Node 0 input pin
    let n0_in_pin = pos2(node_rects[0].min.x, node_rects[0].center().y);
    let n0_enabled = lumetri_effects[0].1.enabled;
    let w0_col = if n0_enabled { Color32::from_rgb(60, 200, 80) } else { Color32::from_rgb(80, 85, 95) };
    draw_bezier_wire(&painter, in_pin, n0_in_pin, Stroke::new(wire_stroke_w, w0_col));

    // 2. Node i output pin to Node i+1 input pin
    for i in 0..(node_count - 1) {
        let p_out = pos2(node_rects[i].max.x, node_rects[i].center().y);
        let p_in = pos2(node_rects[i + 1].min.x, node_rects[i + 1].center().y);
        let enabled = lumetri_effects[i + 1].1.enabled;
        let col = if enabled { Color32::from_rgb(60, 200, 80) } else { Color32::from_rgb(80, 85, 95) };
        draw_bezier_wire(&painter, p_out, p_in, Stroke::new(wire_stroke_w, col));
    }

    // 3. Last Node output pin to OUT terminal
    let last_out_pin = pos2(node_rects[last_idx].max.x, node_rects[last_idx].center().y);
    let last_enabled = lumetri_effects.last().map(|(_, e)| e.enabled).unwrap_or(true);
    let out_wire_color = if last_enabled { Color32::from_rgb(60, 200, 80) } else { Color32::from_rgb(80, 85, 95) };
    draw_bezier_wire(&painter, last_out_pin, out_pin, Stroke::new(wire_stroke_w, out_wire_color));

    // Render Each Node Card
    for (i, &(fx_idx, fx_inst)) in lumetri_effects.iter().enumerate() {
        paint_node_card(
            app,
            ui,
            &painter,
            NodeCardCtx { clip_id: clip.0, i, fx_idx, fx_inst, node_rect: node_rects[i], default_rel: default_rels[i], zoom, active_node, node_count },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_toolbar(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    rect: Rect,
    clip: filmcraft_project::ClipId,
    it_name: &str,
    lumetri_effects: &[(usize, &EffectInstance)],
    active_node: usize,
    act: &mut NodeGraphActions,
) -> Rect {
    let t = app.tokens;
    let node_count = lumetri_effects.len();
    let toolbar_r = Rect::from_min_size(rect.min, vec2(rect.width(), 34.0));
    ui.painter().rect_filled(toolbar_r, 0.0, Color32::from_rgb(26, 28, 33));
    ui.painter()
        .line_segment([pos2(toolbar_r.min.x, toolbar_r.max.y), pos2(toolbar_r.max.x, toolbar_r.max.y)], Stroke::new(1.0, Color32::from_rgb(38, 41, 48)));

    let mut bx = toolbar_r.min.x + 8.0;
    let by = toolbar_r.min.y + 5.0;
    let btn_h = 24.0;

    if toolbar_button(
        app,
        ui,
        &mut bx,
        by,
        ToolbarBtn {
            w: 72.0,
            h: btn_h,
            gap: 5.0,
            id: "color_nodes_add_serial_btn",
            auto_id: "color_nodes.add_serial",
            auto_label: "Add Serial Node",
            label: tl!("+ Serial"),
            tooltip: Some(tl!("Add Serial Node (Alt+S)")),
            active_tint: false,
        },
    ) {
        act.add_serial = true;
    }

    if node_count > 0 {
        let curr_enabled = lumetri_effects[active_node].1.enabled;
        let b_text = if curr_enabled { tl!("Bypass") } else { tl!("Enable") };
        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 56.0,
                h: btn_h,
                gap: 5.0,
                id: "color_nodes_bypass_btn",
                auto_id: "color_nodes.bypass_node",
                auto_label: "Bypass Node",
                label: b_text,
                tooltip: None,
                active_tint: false,
            },
        ) {
            act.bypass_active = true;
        }

        let all_bypassed = lumetri_effects.iter().all(|(_, e)| !e.enabled);
        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 36.0,
                h: btn_h,
                gap: 5.0,
                id: "color_nodes_bypass_all_btn",
                auto_id: "color_nodes.bypass_all",
                auto_label: "Bypass All Nodes",
                label: tl!("All"),
                tooltip: None,
                active_tint: all_bypassed,
            },
        ) {
            act.bypass_all = true;
        }

        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 46.0,
                h: btn_h,
                gap: 5.0,
                id: "color_nodes_rst_btn",
                auto_id: "color_nodes.reset_node",
                auto_label: "Reset Node",
                label: tl!("Reset"),
                tooltip: None,
                active_tint: false,
            },
        ) {
            act.reset_active = true;
        }

        if node_count > 1
            && toolbar_button(
                app,
                ui,
                &mut bx,
                by,
                ToolbarBtn {
                    w: 50.0,
                    h: btn_h,
                    gap: 5.0,
                    id: "color_nodes_del_btn",
                    auto_id: "color_nodes.delete_node",
                    auto_label: "Delete Node",
                    label: tl!("Delete"),
                    tooltip: None,
                    active_tint: false,
                },
            )
        {
            act.delete_active = true;
        }

        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 68.0,
                h: btn_h,
                gap: 5.0,
                id: "color_nodes_org_btn",
                auto_id: "color_nodes.organize",
                auto_label: "Organizar Nodos",
                label: tl!("Organizar"),
                tooltip: Some(tl!("Alinear y organizar nodos en serie")),
                active_tint: false,
            },
        ) {
            let prefix = format!("{}:", clip.0);
            app.ui.color_node_positions.retain(|k, _| !k.starts_with(&prefix));
            app.ui.color_node_pan = [0.0, 0.0];
            app.ui.color_node_zoom = 1.0;
            if node_count > 4 {
                act.fit_view = true;
            }
        }

        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 68.0,
                h: btn_h,
                gap: 5.0,
                id: "color_nodes_fit_btn",
                auto_id: "color_nodes.fit",
                auto_label: "Ajustar Nodos",
                label: tl!("⛶ Ajustar"),
                tooltip: Some(tl!("Ajustar nodos a la pantalla (Alt+F / Z)")),
                active_tint: false,
            },
        ) {
            act.fit_view = true;
        }

        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 24.0,
                h: btn_h,
                gap: 2.0,
                id: "color_nodes_zm_btn",
                auto_id: "color_nodes.zoom_out",
                auto_label: "Zoom Out",
                label: "-",
                tooltip: Some(tl!("Reducir zoom (Ctrl+-)")),
                active_tint: false,
            },
        ) {
            act.zoom_out = true;
        }

        let z_txt = format!("{:.0}%", app.ui.color_node_zoom * 100.0);
        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 44.0,
                h: btn_h,
                gap: 2.0,
                id: "color_nodes_zr_btn",
                auto_id: "color_nodes.zoom_reset",
                auto_label: "Zoom 100%",
                label: &z_txt,
                tooltip: Some(tl!("Restablecer zoom al 100% (Ctrl+0)")),
                active_tint: false,
            },
        ) {
            act.zoom_reset = true;
        }

        if toolbar_button(
            app,
            ui,
            &mut bx,
            by,
            ToolbarBtn {
                w: 24.0,
                h: btn_h,
                gap: 6.0,
                id: "color_nodes_zp_btn",
                auto_id: "color_nodes.zoom_in",
                auto_label: "Zoom In",
                label: "+",
                tooltip: Some(tl!("Aumentar zoom (Ctrl++)")),
                active_tint: false,
            },
        ) {
            act.zoom_in = true;
        }
    }

    let avail_right = (toolbar_r.max.x - bx - 8.0).max(0.0);
    if avail_right > 110.0 {
        let info_r = Rect::from_min_max(pos2(toolbar_r.max.x - avail_right.min(220.0), toolbar_r.min.y), pos2(toolbar_r.max.x - 8.0, toolbar_r.max.y));
        let mut right_text = format!("Master · {it_name}");
        if node_count > 0 {
            right_text = format!("Node {}/{} | {}", active_node + 1, node_count, right_text);
        }
        ui.painter().text(info_r.right_center(), Align2::RIGHT_CENTER, right_text, Tokens::ui(11.0), t.text_dim);
    }

    toolbar_r
}

fn execute_graph_actions(
    app: &mut FilmcraftApp,
    clip: filmcraft_project::ClipId,
    group_id: Option<filmcraft_project::ColorGroupId>,
    lumetri_effects: &[(usize, &EffectInstance)],
    active_node: usize,
    act: &NodeGraphActions,
) -> bool {
    let node_count = lumetri_effects.len();
    if let Some(gid) = group_id {
        if (act.bypass_active || act.bypass_all) && node_count > 0 {
            let _ = app.session.execute("color.groupToggleBypass", json!({"group": gid.0}));
            return true;
        }
        if act.reset_active && node_count > 0 {
            let _ = app.session.execute("color.groupReset", json!({"group": gid.0}));
            return true;
        }
        return false;
    }
    if act.add_serial {
        let _ = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}));
        app.ui.active_color_node = node_count;
        return true;
    }
    if act.bypass_active && node_count > 0 {
        let (idx, _) = lumetri_effects[active_node];
        let _ = app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": idx}));
        return true;
    }
    if act.bypass_all && node_count > 0 {
        let any_enabled = lumetri_effects.iter().any(|(_, e)| e.enabled);
        let _ = app.session.edit_group("Bypass All Color Nodes", |s| {
            for &(idx, e) in lumetri_effects {
                if e.enabled == any_enabled {
                    s.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": idx}))?;
                }
            }
            Ok(serde_json::Value::Null)
        });
        return true;
    }
    if act.reset_active && node_count > 0 {
        let (idx, _) = lumetri_effects[active_node];
        let _ = app.session.execute("effects.reset", json!({"clip": clip.0, "index": idx}));
        return true;
    }
    if act.delete_active && node_count > 1 {
        let (idx, _) = lumetri_effects[active_node];
        let _ = app.session.execute("effects.remove", json!({"clip": clip.0, "index": idx}));
        if active_node >= node_count - 1 {
            app.ui.active_color_node = active_node.saturating_sub(1);
        }
        return true;
    }
    false
}

struct NodeCardCtx<'a> {
    clip_id: u64,
    i: usize,
    fx_idx: usize,
    fx_inst: &'a EffectInstance,
    node_rect: Rect,
    default_rel: [f32; 2],
    zoom: f32,
    active_node: usize,
    node_count: usize,
}

fn paint_node_card(app: &mut FilmcraftApp, ui: &mut egui::Ui, painter: &egui::Painter, c: NodeCardCtx<'_>) {
    let t = app.tokens;
    let i = c.i;
    let node_rect = c.node_rect;
    let zoom = c.zoom;
    let is_active = i == c.active_node;
    let is_enabled = c.fx_inst.enabled;

    let card_resp = ui.interact(node_rect, egui::Id::unique(("color_node_card", c.clip_id, i)), Sense::click_and_drag());
    app.auto.add(&format!("color_nodes.node.{i}"), node_rect, &format!("Node {:02}", i + 1));

    if card_resp.dragged() {
        let delta = card_resp.drag_delta();
        let key = format!("{}:{i}", c.clip_id);
        let pos = app.ui.color_node_positions.entry(key).or_insert(c.default_rel);
        pos[0] += delta.x / zoom;
        pos[1] += delta.y / zoom;
    }
    if card_resp.clicked() {
        app.ui.active_color_node = i;
    }

    let card_pin_r = (3.5 * zoom).clamp(2.0, 5.0);
    let pin_col = if is_enabled { Color32::from_rgb(60, 200, 80) } else { Color32::from_rgb(90, 95, 105) };
    painter.circle_filled(pos2(node_rect.min.x, node_rect.center().y), card_pin_r, pin_col);
    painter.circle_filled(pos2(node_rect.max.x, node_rect.center().y), card_pin_r, pin_col);

    let bg_color =
        if is_enabled { if is_active { Color32::from_rgb(32, 35, 42) } else { Color32::from_rgb(28, 30, 36) } } else { Color32::from_rgb(22, 23, 27) };
    let card_corner = (6.0 * zoom).clamp(3.0, 6.0);
    painter.rect_filled(node_rect, card_corner, bg_color);

    let header_h = (22.0 * zoom).clamp(14.0, 22.0);
    let header_rect = Rect::from_min_size(node_rect.min, vec2(node_rect.width(), header_h));
    let header_bg = if is_active { Color32::from_rgb(38, 42, 52) } else { Color32::from_rgb(22, 24, 28) };
    painter.rect_filled(header_rect, card_corner, header_bg);
    painter.rect_filled(Rect::from_min_max(pos2(header_rect.min.x, header_rect.max.y - 3.0 * zoom), header_rect.max), 0.0, header_bg);

    let num_str = format!("{:02}", i + 1);
    let num_font_size = (11.0 * zoom).clamp(8.0, 11.0);
    painter.text(
        pos2(header_rect.min.x + (8.0 * zoom).max(4.0), header_rect.center().y),
        Align2::LEFT_CENTER,
        num_str,
        Tokens::ui(num_font_size),
        if is_active { Color32::WHITE } else { Color32::from_rgb(180, 185, 195) },
    );

    let power_box_size = (16.0 * zoom).clamp(10.0, 16.0);
    let power_center = pos2(header_rect.max.x - (12.0 * zoom).max(8.0), header_rect.center().y);
    let power_r = Rect::from_center_size(power_center, vec2(power_box_size, power_box_size));
    let power_resp = ui.interact(power_r, egui::Id::unique(("node_power", c.clip_id, i)), Sense::click());
    app.auto.add(&format!("color_nodes.node.{i}.power"), power_r, "Toggle Node Bypass");

    let led_color = if is_enabled { Color32::from_rgb(70, 210, 90) } else { Color32::from_rgb(150, 45, 45) };
    let led_r = (4.5 * zoom).clamp(2.5, 5.0);
    painter.circle_filled(power_center, led_r, led_color);

    if !is_enabled {
        let strike_d = (3.5 * zoom).clamp(2.0, 4.0);
        painter.line_segment(
            [pos2(power_center.x - strike_d, power_center.y - strike_d), pos2(power_center.x + strike_d, power_center.y + strike_d)],
            Stroke::new((1.5 * zoom).clamp(1.0, 2.0), Color32::WHITE),
        );
    }
    if power_resp.clicked() {
        let _ = app.session.execute("effects.toggleEnabled", json!({"clip": c.clip_id, "index": c.fx_idx}));
    }

    if zoom >= 0.55 {
        let label_key = format!("{}:{i}", c.clip_id);
        let custom_label = c.fx_inst.label.clone().or_else(|| app.ui.color_node_labels.get(&label_key).cloned());
        let display_label = custom_label.unwrap_or_else(|| format!("Node {:02}", i + 1));
        let label_pos = pos2(node_rect.min.x + (8.0 * zoom).max(4.0), header_rect.max.y + (5.0 * zoom).max(2.0));
        painter.text(label_pos, Align2::LEFT_TOP, display_label, Tokens::ui((11.0 * zoom).clamp(8.0, 11.0)), if is_enabled { t.text } else { t.text_faint });
    }

    if zoom >= 0.65 {
        paint_node_badges(ui, painter, &c);
    }

    let stroke = if is_active {
        Stroke::new((2.0 * zoom).clamp(1.5, 2.5), Color32::from_rgb(230, 175, 45))
    } else if card_resp.hovered() {
        Stroke::new(1.0, Color32::from_rgb(90, 95, 110))
    } else {
        Stroke::new(1.0, Color32::from_rgb(48, 52, 62))
    };
    painter.rect_stroke(node_rect, card_corner, stroke, StrokeKind::Inside);

    card_resp.context_menu(|ui| {
        if ui.button(tl!("Add Serial Node (Alt+S)")).clicked() {
            let _ = app.session.execute("effects.apply", json!({"clips": [c.clip_id], "effect": "lumetri"}));
            app.ui.active_color_node = c.node_count;
            ui.close();
        }
        let toggle_label = if is_enabled { tl!("Bypass Node (Ctrl+D)") } else { tl!("Enable Node (Ctrl+D)") };
        if ui.button(toggle_label).clicked() {
            let _ = app.session.execute("effects.toggleEnabled", json!({"clip": c.clip_id, "index": c.fx_idx}));
            ui.close();
        }
        if ui.button(tl!("Reset Node")).clicked() {
            let _ = app.session.execute("effects.reset", json!({"clip": c.clip_id, "index": c.fx_idx}));
            ui.close();
        }
        if c.node_count > 1 && ui.button(tl!("Delete Node")).clicked() {
            let _ = app.session.execute("effects.remove", json!({"clip": c.clip_id, "index": c.fx_idx}));
            if c.active_node >= c.node_count - 1 {
                app.ui.active_color_node = c.active_node.saturating_sub(1);
            }
            ui.close();
        }
    });
}

fn paint_node_badges(ui: &mut egui::Ui, painter: &egui::Painter, c: &NodeCardCtx<'_>) {
    let badges = detect_node_badges(c.fx_inst);
    let zoom = c.zoom;
    let is_enabled = c.fx_inst.enabled;
    let is_active = c.i == c.active_node;
    let node_rect = c.node_rect;
    let badge_w = (26.0 * zoom).clamp(18.0, 26.0);
    let badge_h = (13.0 * zoom).clamp(9.0, 13.0);
    let mut bx = node_rect.min.x + (8.0 * zoom).max(4.0);
    let by = node_rect.max.y - (18.0 * zoom).max(12.0);

    for (badge_text, badge_tip) in badges {
        let br = Rect::from_min_size(pos2(bx, by), vec2(badge_w, badge_h));
        let b_bg = if is_enabled { Color32::from_rgb(20, 22, 28) } else { Color32::from_rgb(18, 19, 22) };
        painter.rect_filled(br, 2.0, b_bg);
        painter.rect_stroke(
            br,
            2.0,
            Stroke::new(1.0, if is_active { Color32::from_rgb(70, 75, 90) } else { Color32::from_rgb(45, 48, 56) }),
            StrokeKind::Inside,
        );
        let txt_col = match badge_text {
            "PRI" => Color32::from_rgb(230, 180, 50),
            "LUT" => Color32::from_rgb(70, 180, 240),
            "CRV" => Color32::from_rgb(180, 110, 240),
            "WHL" => Color32::from_rgb(240, 120, 90),
            "HSL" => Color32::from_rgb(80, 210, 160),
            "VIG" => Color32::from_rgb(210, 160, 210),
            _ => Color32::from_rgb(140, 150, 165),
        };
        painter.text(br.center(), Align2::CENTER_CENTER, badge_text, Tokens::ui((9.0 * zoom).clamp(7.0, 9.0)), txt_col);
        ui.interact(br, egui::Id::unique(("badge", c.clip_id, c.i, badge_text)), Sense::hover()).on_hover_text(badge_tip);

        bx += badge_w + 4.0 * zoom;
        if bx + badge_w > node_rect.max.x - 6.0 * zoom {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::detect_node_badges;
    use filmcraft_geom::Vec2;
    use filmcraft_project::{ParamValue, find_effect};

    #[test]
    fn badges_reflect_vec2_wheels_and_edited_curves_only() {
        let mut fx = find_effect("lumetri").expect("lumetri def").instance();
        assert_eq!(detect_node_badges(&fx), vec![("GRADE", "Grade")], "default node shows GRADE, not false CRV");

        fx.param_mut("wheel_shadows").unwrap().value = ParamValue::Vec2(Vec2::new(0.25, -0.1));
        assert!(detect_node_badges(&fx).iter().any(|(b, _)| *b == "WHL"), "Vec2 wheel move triggers WHL badge");

        fx.param_mut("curve_luma").unwrap().value = ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
        assert!(detect_node_badges(&fx).iter().any(|(b, _)| *b == "CRV"), "edited curve triggers CRV badge");
    }
}
