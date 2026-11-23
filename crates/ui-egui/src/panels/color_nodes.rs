//! DaVinci Resolve-style non-linear Node Graph panel (`ColorGraph` DAG) for color grading.
//!
//! Supports four grading pipeline scopes (`Pre-Clip`, `Clip`, `Post-Clip`, `Timeline`), dual
//! `Rgb` (green) and `Key` (blue) ports and cables, and node types:
//! - `Corrector` (Serial, `Alt+S`)
//! - `Outside` (inverted Key receiver, `Alt+O`)
//! - `ParallelMixer` (`Alt+P`)
//! - `LayerMixer` (`Alt+L`)
//! - `Splitter` / `Combiner` (`Alt+Y`)
//! - `Shared` (project-wide reusable grade node)

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{
    ClipId, ColorEndpoint, ColorGraph, ColorGroupId, ColorNode, ColorNodeId, ColorNodeKind, ColorPortKind, ColorScopeKind, EffectInstance, LayerBlendMode,
    ParamValue,
};
use serde_json::{Value, json};

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
            || has_f("contrast_pivot", 50.0)
            || has_f("color_boost", 0.0)
            || has_f("midtone_detail", 0.0)
            || has_f("primary_hue", 50.0)
            || has_f("lum_mix", 100.0)
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
            || has_hue_curve("sat_vs_sat")
            || has_hue_curve("sat_vs_luma")
            || has_f("curve_low_soft", 0.0)
            || has_f("curve_high_soft", 0.0))
    {
        badges.push(("CRV", "Curves"));
    }
    if e.param("wheels_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && ([
            "wheel_shadows",
            "wheel_midtones",
            "wheel_highlights",
            "wheel_offset",
            "log_shadows",
            "log_midtones",
            "log_highlights",
            "log_offset",
            "hdr_w_black",
            "hdr_w_dark",
            "hdr_w_shadow",
            "hdr_w_light",
            "hdr_w_highlight",
            "hdr_w_specular",
            "hdr_w_global",
        ]
        .iter()
        .any(|id| has_v2(id))
            || [
                "wheel_shadows_l",
                "wheel_midtones_l",
                "wheel_highlights_l",
                "wheel_offset_l",
                "log_shadows_l",
                "log_midtones_l",
                "log_highlights_l",
                "log_offset_l",
                "hdr_w_black_exp",
                "hdr_w_dark_exp",
                "hdr_w_shadow_exp",
                "hdr_w_light_exp",
                "hdr_w_highlight_exp",
                "hdr_w_specular_exp",
                "hdr_w_global_exp",
            ]
            .iter()
            .any(|id| has_f(id, 0.0)))
    {
        badges.push(("WHL", "Wheels"));
    }
    if e.param("rgb_mix_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && (e.param("mix_monochrome").and_then(|p| p.value.as_bool()).unwrap_or(false)
            || has_f("mix_rr", 100.0)
            || has_f("mix_rg", 0.0)
            || has_f("mix_rb", 0.0)
            || has_f("mix_gr", 0.0)
            || has_f("mix_gg", 100.0)
            || has_f("mix_gb", 0.0)
            || has_f("mix_br", 0.0)
            || has_f("mix_bg", 0.0)
            || has_f("mix_bb", 100.0))
    {
        badges.push(("MIX", "RGB Mixer"));
    }
    if e.param("hsl_on").and_then(|p| p.value.as_bool()).unwrap_or(false) {
        badges.push(("HSL", "HSL"));
    }
    if e.param("vignette_on").and_then(|p| p.value.as_bool()).unwrap_or(true) && has_f("vignette_amount", 0.0) {
        badges.push(("VIG", "Vignette"));
    }
    if e.param("cs_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && filmcraft_render::effects::COLOR_SLICE_BANDS
            .iter()
            .any(|(id, _, _, _, _)| has_f(&format!("cs_{id}_sat"), 100.0) || has_f(&format!("cs_{id}_den"), 0.0) || has_f(&format!("cs_{id}_hue"), 0.0))
    {
        badges.push(("SLC", "ColorSlice"));
    }
    let has_warper = |id: &str| e.param(id).and_then(|p| p.value.as_curve()).is_some_and(|pts| pts.iter().any(|pt| pt[0].abs() > 1e-4 || pt[1].abs() > 1e-4));
    if e.param("cw_on").and_then(|p| p.value.as_bool()).unwrap_or(true) && (has_warper("cw_hs_grid") || has_warper("cw_cl_grid")) {
        badges.push(("WRP", "Color Warper"));
    }
    if e.param("cst_on").and_then(|p| p.value.as_bool()).unwrap_or(false) {
        badges.push(("CST", "Color Space Transform"));
    }
    if e.param("sizing_on").and_then(|p| p.value.as_bool()).unwrap_or(true)
        && (has_f("sizing_pan", 0.0)
            || has_f("sizing_tilt", 0.0)
            || has_f("sizing_zoom", 100.0)
            || has_f("sizing_width", 100.0)
            || has_f("sizing_height", 100.0)
            || has_f("sizing_rotate", 0.0)
            || e.param("sizing_flip_h").and_then(|p| p.value.as_bool()).unwrap_or(false)
            || e.param("sizing_flip_v").and_then(|p| p.value.as_bool()).unwrap_or(false)
            || e.param("sizing_blanking")
                .and_then(|p| match p.value {
                    filmcraft_project::ParamValue::Choice(c) => Some(c != 0),
                    _ => None,
                })
                .unwrap_or(false))
    {
        badges.push(("SIZ", "Sizing"));
    }

    if badges.is_empty() {
        badges.push(("GRADE", "Grade"));
    }
    badges
}

/// Draw a smooth cubic bezier connection line between two pins.
fn draw_bezier_wire(painter: &egui::Painter, p0: Pos2, p1: Pos2, stroke: Stroke, inverted_badge: bool) {
    let (c0, c1) = if p1.x >= p0.x {
        let dx = ((p1.x - p0.x) * 0.55).max(30.0);
        (pos2(p0.x + dx, p0.y), pos2(p1.x - dx, p1.y))
    } else {
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
    if inverted_badge {
        let mid = pos2(0.125 * p0.x + 0.375 * c0.x + 0.375 * c1.x + 0.125 * p1.x, 0.125 * p0.y + 0.375 * c0.y + 0.375 * c1.y + 0.125 * p1.y);
        painter.circle_filled(mid, 4.5, Color32::from_rgb(20, 24, 32));
        painter.circle_stroke(mid, 4.5, Stroke::new(1.3, stroke.color));
    }
}

#[derive(Default)]
struct NodeGraphActions {
    add_serial: bool,
    add_parallel: bool,
    add_layer: bool,
    add_outside: bool,
    add_splitter: bool,
    add_shared: bool,
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
        } else if i.modifiers.alt && i.key_pressed(egui::Key::P) {
            act.add_parallel = true;
        } else if i.modifiers.alt && i.key_pressed(egui::Key::L) {
            act.add_layer = true;
        } else if i.modifiers.alt && i.key_pressed(egui::Key::O) {
            act.add_outside = true;
        } else if i.modifiers.alt && i.key_pressed(egui::Key::Y) {
            act.add_splitter = true;
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
        t.accent.gamma_multiply(0.22)
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

fn handle_canvas_zoom_pan(app: &mut FilmcraftApp, ui: &mut egui::Ui, canvas_r: Rect, scope_key: u64, node_count: usize, act: &NodeGraphActions) {
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

    let is_node_dragged = (0..node_count).any(|i| ui.ctx().is_being_dragged(egui::Id::unique(("color_node_card", scope_key, i))));
    let is_pin_dragged = ui.ctx().data(|d| d.get_temp::<PendingWire>(egui::Id::unique("color_nodes_pending_wire"))).is_some();
    let is_knife = ui.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.modifiers.shift);
    if !is_node_dragged && !is_pin_dragged && !is_knife {
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

#[derive(Clone, Copy, Debug)]
struct PendingWire {
    from: ColorEndpoint,
    kind: ColorPortKind,
    from_port: u8,
    start_pos: Pos2,
}

#[allow(clippy::cognitive_complexity)]
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(20, 22, 26));

    let clip_info = active_color_clip(app);
    let clip_group = clip_info.as_ref().and_then(|(cid, _)| {
        let seq = app.session.active_sequence()?;
        seq.color_group_of(*cid).map(|g| (g.id, g.name.clone()))
    });
    let explicit_group = app.ui.active_color_group.and_then(|gid| {
        let seq = app.session.active_sequence()?;
        seq.color_group(gid).map(|g| (g.id, g.name.clone()))
    });
    let resolved_group = explicit_group.or(clip_group);

    // Sync active_color_scope with active_color_group if needed
    if app.ui.active_color_group.is_some() && app.ui.active_color_scope == ColorScopeKind::Clip {
        app.ui.active_color_scope = ColorScopeKind::PostClip;
    }
    if matches!(app.ui.active_color_scope, ColorScopeKind::PreClip | ColorScopeKind::PostClip) && resolved_group.is_none() {
        app.ui.active_color_scope = ColorScopeKind::Clip;
    }

    let scope = app.ui.active_color_scope;
    let Some(seq) = app.session.active_sequence() else {
        crate::dock::placeholder(ui, rect, &t, tl!("(no sequence open)"));
        return;
    };

    let (clip_id, scope_key, title_name, group_id, graph, has_custom_dag, lumetri_effects): (
        ClipId,
        u64,
        String,
        Option<ColorGroupId>,
        ColorGraph,
        bool,
        Vec<(usize, EffectInstance)>,
    ) = match scope {
        ColorScopeKind::Timeline => {
            let g = seq.timeline_color.clone();
            let c_fx = g.corrector_effects().into_iter().enumerate().collect();
            (ClipId(0), 0x714E_0000, tl!("Timeline").into(), None, g, true, c_fx)
        }
        ColorScopeKind::PreClip => {
            let Some((gid, gname)) = resolved_group.clone() else {
                crate::dock::placeholder(ui, rect, &t, tl!("(clip is not in a Color Group)"));
                return;
            };
            let g = seq.color_group(gid).map(|grp| grp.pre_clip.clone()).unwrap_or_default();
            let c_fx = g.corrector_effects().into_iter().enumerate().collect();
            (ClipId(0), 0x9AE0_0000 ^ gid.0, format!("Pre-Clip: {gname}"), Some(gid), g, true, c_fx)
        }
        ColorScopeKind::PostClip => {
            let Some((gid, gname)) = resolved_group.clone() else {
                crate::dock::placeholder(ui, rect, &t, tl!("(clip is not in a Color Group)"));
                return;
            };
            let grp = seq.color_group(gid);
            let g = grp.map(|gr| gr.effective_post_clip().into_owned()).unwrap_or_default();
            let has_dag = grp.is_some_and(|gr| !gr.post_clip.is_empty());
            let c_fx = g.corrector_effects().into_iter().enumerate().collect();
            (ClipId(0), 0x9057_0000 ^ gid.0, format!("Post-Clip: {gname}"), Some(gid), g, has_dag, c_fx)
        }
        ColorScopeKind::Clip => {
            let Some((cid, it)) = clip_info else {
                crate::dock::placeholder(ui, rect, &t, tl!("(no clip selected)"));
                return;
            };
            let g = it.effective_color_graph().into_owned();
            let has_dag = it.color_graph.is_some();
            let lums: Vec<(usize, EffectInstance)> =
                it.effects.iter().enumerate().filter(|(_, e)| e.effect == "lumetri").map(|(i, e)| (i, e.clone())).collect();
            (cid, cid.0, it.name, None, g, has_dag, lums)
        }
    };

    let node_count = graph.nodes.len();
    let active_node = if node_count == 0 { 0 } else { app.ui.active_color_node.min(node_count - 1) };
    app.ui.active_color_node = active_node;

    let mut act = read_shortcuts(ui, rect, node_count);
    let toolbar_r = render_toolbar(app, ui, rect, clip_id, scope_key, &title_name, resolved_group.map(|(g, _)| g), &graph, active_node, &mut act);

    if execute_graph_actions(app, scope, clip_id, group_id, &graph, &lumetri_effects, active_node, &act) {
        return;
    }

    let canvas_r = Rect::from_min_max(pos2(rect.min.x, toolbar_r.max.y), rect.max);
    if canvas_r.width() < 10.0 || canvas_r.height() < 10.0 {
        return;
    }

    handle_canvas_zoom_pan(app, ui, canvas_r, scope_key, node_count, &act);

    if node_count == 0 {
        let painter = ui.painter_at(canvas_r).with_clip_rect(canvas_r);
        let center = canvas_r.center();
        painter.text(pos2(center.x, center.y - 20.0), Align2::CENTER_CENTER, tl!("No color grading nodes in this scope"), Tokens::ui(13.0), t.text_dim);
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
            if scope == ColorScopeKind::Clip {
                let _ = app.session.execute("effects.apply", json!({"clips": [clip_id.0], "effect": "lumetri"}));
            } else {
                let _ = app.session.execute("color.graph.addNode", scope_params(scope, clip_id, group_id, json!({"kind": "serial"})));
            }
            app.ui.active_color_node = 0;
        }
        return;
    }

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
    let start_x = ((canvas_r.width() - total_chain_w) * 0.5).max(base_port_w + base_gap + 20.0);
    let total_h = total_rows as f32 * base_node_h + (total_rows.saturating_sub(1)) as f32 * base_v_gap;
    let start_y = ((canvas_r.height() - total_h) * 0.5).max(20.0);

    let fit_view = act.fit_view || (node_count > 4 && app.ui.color_node_zoom == 1.0 && app.ui.color_node_pan == [0.0, 0.0]);

    let node_rel_pos = |i: usize, n: &ColorNode, map: &std::collections::HashMap<String, [f32; 2]>| -> [f32; 2] {
        let key = format!("{scope_key}:{i}");
        if let Some(&p) = map.get(&key) {
            return p;
        }
        if has_custom_dag && (n.pos[0].abs() > 1e-3 || n.pos[1].abs() > 1e-3) {
            return n.pos;
        }
        let r = i / nodes_per_row;
        let c = i % nodes_per_row;
        [start_x + c as f32 * (base_node_w + base_gap), start_y + r as f32 * (base_node_h + base_v_gap)]
    };

    if fit_view && node_count > 0 {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for (i, n) in graph.nodes.iter().enumerate() {
            let [x, y] = node_rel_pos(i, n, &app.ui.color_node_positions);
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

    // Subtle background dot grid
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

    let mut node_rects = Vec::with_capacity(node_count);
    let mut default_rels = Vec::with_capacity(node_count);
    let mut id_to_idx = std::collections::BTreeMap::new();
    for (i, n) in graph.nodes.iter().enumerate() {
        id_to_idx.insert(n.id, i);
        let rel_pos = node_rel_pos(i, n, &app.ui.color_node_positions);
        default_rels.push(rel_pos);
        let screen_min = canvas_r.min + pan + vec2(rel_pos[0] * zoom, rel_pos[1] * zoom);
        let screen_size = vec2(base_node_w * zoom, base_node_h * zoom);
        node_rects.push(Rect::from_min_size(screen_min, screen_size));
    }

    // Terminals [ IN ] and [ OUT ]
    let port_w = base_port_w * zoom;
    let port_h = base_port_h * zoom;
    let port_corner = (4.0 * zoom).clamp(2.0, 4.0);

    let in_y = node_rects[0].center().y;
    let in_x = node_rects[0].min.x - (base_gap * zoom) - port_w;
    let in_rect = Rect::from_min_size(pos2(in_x, in_y - port_h * 0.5), vec2(port_w, port_h));
    painter.rect_filled(in_rect, port_corner, Color32::from_rgb(28, 30, 36));
    painter.rect_stroke(in_rect, port_corner, Stroke::new(1.0, Color32::from_rgb(50, 54, 64)), StrokeKind::Inside);
    if zoom >= 0.5 {
        painter.text(in_rect.center(), Align2::CENTER_CENTER, "IN", Tokens::ui((11.0 * zoom).clamp(8.0, 11.0)), Color32::from_rgb(140, 150, 170));
    }
    let in_pin = pos2(in_rect.max.x, in_y);
    let term_pin_r = (4.0 * zoom).clamp(2.5, 5.5);
    painter.circle_filled(in_pin, term_pin_r, Color32::from_rgb(60, 200, 80));

    let max_right_idx = (0..node_count).max_by(|&a, &b| node_rects[a].max.x.total_cmp(&node_rects[b].max.x)).unwrap_or(node_count - 1);
    let out_y = node_rects[max_right_idx].center().y;
    let out_x = node_rects[max_right_idx].max.x + (base_gap * zoom);
    let out_rect = Rect::from_min_size(pos2(out_x, out_y - port_h * 0.5), vec2(port_w, port_h));
    painter.rect_filled(out_rect, port_corner, Color32::from_rgb(28, 30, 36));
    painter.rect_stroke(out_rect, port_corner, Stroke::new(1.0, Color32::from_rgb(50, 54, 64)), StrokeKind::Inside);
    if zoom >= 0.5 {
        painter.text(out_rect.center(), Align2::CENTER_CENTER, "OUT", Tokens::ui((11.0 * zoom).clamp(8.0, 11.0)), Color32::from_rgb(140, 150, 170));
    }
    let out_pin = pos2(out_rect.min.x, out_y);
    painter.circle_filled(out_pin, term_pin_r, Color32::from_rgb(60, 200, 80));

    // Port coordinate resolver
    let port_pos = |ep: ColorEndpoint, kind: ColorPortKind, port: u8, is_input: bool| -> Option<Pos2> {
        match ep {
            ColorEndpoint::Source => (!is_input && kind == ColorPortKind::Rgb).then_some(in_pin),
            ColorEndpoint::Output => (is_input && kind == ColorPortKind::Rgb).then_some(out_pin),
            ColorEndpoint::Node(nid) => {
                let &idx = id_to_idx.get(&nid)?;
                let nr = node_rects.get(idx)?;
                let node = graph.nodes.get(idx)?;
                let x = if is_input { nr.min.x } else { nr.max.x };
                match kind {
                    ColorPortKind::Rgb => {
                        let count = if is_input { node.kind.rgb_input_ports() } else { node.kind.rgb_output_ports() }.max(1);
                        let frac = if count == 1 {
                            if (is_input && node.kind.key_input_ports() > 0) || (!is_input && node.kind.key_output_ports() > 0) { 0.38 } else { 0.50 }
                        } else {
                            0.22 + (port.min(count - 1) as f32 / (count - 1).max(1) as f32) * 0.56
                        };
                        Some(pos2(x, nr.min.y + nr.height() * frac))
                    }
                    ColorPortKind::Key => {
                        let count = if is_input { node.kind.key_input_ports() } else { node.kind.key_output_ports() };
                        if count == 0 {
                            return None;
                        }
                        let frac = if count == 1 { 0.74 } else { 0.30 + (port.min(count - 1) as f32 / (count - 1).max(1) as f32) * 0.56 };
                        Some(pos2(x, nr.min.y + nr.height() * frac))
                    }
                }
            }
        }
    };

    // Draw all DAG wires (Green = RGB, Blue = Key)
    let wire_stroke_w = (2.0 * zoom).clamp(1.2, 3.0);
    for edge in &graph.edges {
        if let (Some(p0), Some(p1)) = (port_pos(edge.from, edge.kind, edge.from_port, false), port_pos(edge.to, edge.kind, edge.to_port, true)) {
            let active_wire = match edge.to {
                ColorEndpoint::Node(nid) => id_to_idx.get(&nid).and_then(|&i| graph.nodes.get(i)).is_none_or(|n| n.enabled),
                ColorEndpoint::Output => true,
                ColorEndpoint::Source => false,
            };
            let col = match (edge.kind, active_wire) {
                (ColorPortKind::Rgb, true) => Color32::from_rgb(60, 200, 80),
                (ColorPortKind::Key, true) => Color32::from_rgb(65, 160, 245),
                (_, false) => Color32::from_rgb(80, 85, 95),
            };
            draw_bezier_wire(&painter, p0, p1, Stroke::new(wire_stroke_w, col), edge.kind == ColorPortKind::Key && edge.invert_key);
        }
    }

    // Render Each Node Card
    for (i, node) in graph.nodes.iter().enumerate() {
        let fx_idx = lumetri_effects.get(i).map(|(idx, _)| *idx);
        paint_node_card(
            app,
            ui,
            &painter,
            NodeCardCtx {
                scope,
                clip_id,
                group_id,
                scope_key,
                i,
                node,
                fx_idx,
                node_rect: node_rects[i],
                default_rel: default_rels[i],
                zoom,
                active_node,
                node_count,
                has_custom_dag,
            },
        );
    }

    // Interactive port pins (drag or click to connect, Alt-click / right-click input to disconnect)
    handle_interactive_pins(app, ui, &painter, scope, clip_id, group_id, &graph, &port_pos, in_pin, out_pin, zoom);

    // Shared Ctrl+Shift+Drag knife wire cutter (shared geometry with VfxNodes)
    let knife_mod = ui.input(|i| (i.modifiers.command || i.modifiers.ctrl) && i.modifiers.shift);
    let knife_id = egui::Id::unique("color_nodes_knife_cut");
    let mut knife: Option<(Pos2, Pos2)> = ui.ctx().data(|d| d.get_temp(knife_id));
    if knife_mod
        && ui.input(|i| i.pointer.primary_pressed())
        && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        && canvas_r.contains(p)
    {
        knife = Some((p, p));
    }
    if let Some((s0, s1)) = knife.as_mut() {
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            *s1 = p;
        }
        painter.line_segment([*s0, *s1], Stroke::new(2.0, Color32::from_rgb(248, 113, 113)));
        if ui.input(|i| i.pointer.primary_released()) {
            let (a0, a1) = (*s0, *s1);
            for edge in &graph.edges {
                if let (Some(p0), Some(p1)) = (port_pos(edge.from, edge.kind, edge.from_port, false), port_pos(edge.to, edge.kind, edge.to_port, true))
                    && super::vfx_nodes::bezier_intersects_segment(p0, p1, a0, a1, 16)
                {
                    let k_str = if edge.kind == ColorPortKind::Key { "key" } else { "rgb" };
                    let to_val = match edge.to {
                        ColorEndpoint::Source => json!("source"),
                        ColorEndpoint::Output => json!("output"),
                        ColorEndpoint::Node(id) => json!(id.0),
                    };
                    let _ = app.session.execute(
                        "color.graph.disconnect",
                        scope_params(scope, clip_id, group_id, json!({"kind": k_str, "to": to_val, "toPort": edge.to_port})),
                    );
                }
            }
            knife = None;
        }
    }
    ui.ctx().data_mut(|d| {
        if let Some(k) = knife {
            d.insert_temp(knife_id, k);
        } else {
            d.remove::<(Pos2, Pos2)>(knife_id);
        }
    });
}

fn scope_params(scope: ColorScopeKind, clip: ClipId, group: Option<ColorGroupId>, mut extra: Value) -> Value {
    if let Some(obj) = extra.as_object_mut() {
        obj.insert("scope".into(), json!(scope.id()));
        if clip.0 != 0 {
            obj.insert("clip".into(), json!(clip.0));
        }
        if let Some(gid) = group {
            obj.insert("group".into(), json!(gid.0));
        }
    }
    extra
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn handle_interactive_pins(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    scope: ColorScopeKind,
    clip: ClipId,
    group: Option<ColorGroupId>,
    graph: &ColorGraph,
    port_pos: &dyn Fn(ColorEndpoint, ColorPortKind, u8, bool) -> Option<Pos2>,
    in_pin: Pos2,
    out_pin: Pos2,
    zoom: f32,
) {
    let pending_id = egui::Id::unique("color_nodes_pending_wire");
    let mut pending = ui.ctx().data(|d| d.get_temp::<PendingWire>(pending_id));
    let hit_r = (7.0 * zoom).clamp(5.0, 9.0);
    let pin_draw_r = (3.6 * zoom).clamp(2.2, 5.0);

    // Collect all output and input ports
    let mut out_ports: Vec<(ColorEndpoint, ColorPortKind, u8, Pos2, String)> =
        vec![(ColorEndpoint::Source, ColorPortKind::Rgb, 0, in_pin, "color_nodes.source.out_rgb".into())];
    let mut in_ports: Vec<(ColorEndpoint, ColorPortKind, u8, Pos2, String)> =
        vec![(ColorEndpoint::Output, ColorPortKind::Rgb, 0, out_pin, "color_nodes.output.in_rgb".into())];

    for (i, n) in graph.nodes.iter().enumerate() {
        let ep = ColorEndpoint::Node(n.id);
        for p in 0..n.kind.rgb_output_ports() {
            if let Some(pos) = port_pos(ep, ColorPortKind::Rgb, p, false) {
                out_ports.push((ep, ColorPortKind::Rgb, p, pos, format!("color_nodes.node.{i}.out_rgb.{p}")));
            }
        }
        for p in 0..n.kind.key_output_ports() {
            if let Some(pos) = port_pos(ep, ColorPortKind::Key, p, false) {
                out_ports.push((ep, ColorPortKind::Key, p, pos, format!("color_nodes.node.{i}.out_key.{p}")));
            }
        }
        for p in 0..n.kind.rgb_input_ports() {
            if let Some(pos) = port_pos(ep, ColorPortKind::Rgb, p, true) {
                in_ports.push((ep, ColorPortKind::Rgb, p, pos, format!("color_nodes.node.{i}.in_rgb.{p}")));
            }
        }
        for p in 0..n.kind.key_input_ports() {
            if let Some(pos) = port_pos(ep, ColorPortKind::Key, p, true) {
                in_ports.push((ep, ColorPortKind::Key, p, pos, format!("color_nodes.node.{i}.in_key.{p}")));
            }
        }
    }

    for (ep, kind, port, pos, auto_id) in &out_ports {
        let col = if *kind == ColorPortKind::Rgb { Color32::from_rgb(60, 200, 80) } else { Color32::from_rgb(65, 160, 245) };
        painter.circle_filled(*pos, pin_draw_r, col);
        let pr = Rect::from_center_size(*pos, vec2(hit_r * 2.0, hit_r * 2.0));
        let resp = ui.interact(pr, egui::Id::unique((&auto_id, "out")), Sense::click_and_drag());
        app.auto.add(auto_id, pr, "Output Port");
        if resp.drag_started() || resp.clicked() {
            let pw = PendingWire { from: *ep, kind: *kind, from_port: *port, start_pos: *pos };
            pending = Some(pw);
            ui.ctx().data_mut(|d| d.insert_temp(pending_id, pw));
        }
    }

    let alt = ui.input(|i| i.modifiers.alt);
    for (ep, kind, port, pos, auto_id) in &in_ports {
        let col = if *kind == ColorPortKind::Rgb { Color32::from_rgb(60, 200, 80) } else { Color32::from_rgb(65, 160, 245) };
        painter.circle_filled(*pos, pin_draw_r, col);
        let pr = Rect::from_center_size(*pos, vec2(hit_r * 2.0, hit_r * 2.0));
        let resp = ui
            .interact(pr, egui::Id::unique((&auto_id, "in")), Sense::click())
            .on_hover_text(tl!("Click to connect pending wire · Alt-click or Right-click to disconnect"));
        app.auto.add(auto_id, pr, "Input Port");
        if resp.secondary_clicked() || (resp.clicked() && alt) {
            let to_val = match ep {
                ColorEndpoint::Output => json!("output"),
                ColorEndpoint::Node(id) => json!(id.0),
                ColorEndpoint::Source => json!("source"),
            };
            let k_str = if *kind == ColorPortKind::Rgb { "rgb" } else { "key" };
            let _ = app.session.execute("color.graph.disconnect", scope_params(scope, clip, group, json!({"kind": k_str, "to": to_val, "toPort": port})));
            pending = None;
            ui.ctx().data_mut(|d| d.remove::<PendingWire>(pending_id));
        } else if resp.clicked()
            && let Some(pw) = pending
            && pw.kind == *kind
        {
            let from_val = match pw.from {
                ColorEndpoint::Source => json!("source"),
                ColorEndpoint::Node(id) => json!(id.0),
                ColorEndpoint::Output => json!("output"),
            };
            let to_val = match ep {
                ColorEndpoint::Output => json!("output"),
                ColorEndpoint::Node(id) => json!(id.0),
                ColorEndpoint::Source => json!("source"),
            };
            let k_str = if *kind == ColorPortKind::Rgb { "rgb" } else { "key" };
            if let Err(e) = app.session.execute(
                "color.graph.connect",
                scope_params(scope, clip, group, json!({"kind": k_str, "from": from_val, "fromPort": pw.from_port, "to": to_val, "toPort": port})),
            ) {
                app.ui.status = e.to_string();
            }
            pending = None;
            ui.ctx().data_mut(|d| d.remove::<PendingWire>(pending_id));
        }
    }

    // Live dragged wire preview + drop onto input port
    if let Some(pw) = pending {
        if let Some(ptr) = ui.input(|i| i.pointer.latest_pos()) {
            let col = if pw.kind == ColorPortKind::Rgb { Color32::from_rgb(60, 200, 80) } else { Color32::from_rgb(65, 160, 245) };
            draw_bezier_wire(painter, pw.start_pos, ptr, Stroke::new(1.8, col.gamma_multiply(0.85)), false);
            if ui.input(|i| i.pointer.any_released())
                && let Some((ep, kind, port, _, _)) = in_ports.iter().find(|(_, k, _, pos, _)| *k == pw.kind && pos.distance(ptr) <= hit_r * 2.2)
            {
                let from_val = match pw.from {
                    ColorEndpoint::Source => json!("source"),
                    ColorEndpoint::Node(id) => json!(id.0),
                    ColorEndpoint::Output => json!("output"),
                };
                let to_val = match ep {
                    ColorEndpoint::Output => json!("output"),
                    ColorEndpoint::Node(id) => json!(id.0),
                    ColorEndpoint::Source => json!("source"),
                };
                let k_str = if *kind == ColorPortKind::Rgb { "rgb" } else { "key" };
                if let Err(e) = app.session.execute(
                    "color.graph.connect",
                    scope_params(scope, clip, group, json!({"kind": k_str, "from": from_val, "fromPort": pw.from_port, "to": to_val, "toPort": port})),
                ) {
                    app.ui.status = e.to_string();
                }
                ui.ctx().data_mut(|d| d.remove::<PendingWire>(pending_id));
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.secondary_clicked()) {
            ui.ctx().data_mut(|d| d.remove::<PendingWire>(pending_id));
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn render_toolbar(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    rect: Rect,
    clip: ClipId,
    scope_key: u64,
    it_name: &str,
    resolved_group: Option<ColorGroupId>,
    graph: &ColorGraph,
    active_node: usize,
    act: &mut NodeGraphActions,
) -> Rect {
    let t = app.tokens;
    let node_count = graph.nodes.len();
    let toolbar_r = Rect::from_min_size(rect.min, vec2(rect.width(), 58.0));
    ui.painter().rect_filled(toolbar_r, 0.0, Color32::from_rgb(26, 28, 33));
    ui.painter()
        .line_segment([pos2(toolbar_r.min.x, toolbar_r.max.y), pos2(toolbar_r.max.x, toolbar_r.max.y)], Stroke::new(1.0, Color32::from_rgb(38, 41, 48)));

    // Row 1: Primary Serial / Bypass / Reset / Delete / Organize / Fit / Zoom
    let mut bx = toolbar_r.min.x + 8.0;
    let by1 = toolbar_r.min.y + 4.0;
    let btn_h1 = 23.0;

    if toolbar_button(
        app,
        ui,
        &mut bx,
        by1,
        ToolbarBtn {
            w: 72.0,
            h: btn_h1,
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
        let curr_enabled = graph.nodes.get(active_node).is_none_or(|n| n.enabled);
        let b_text = if curr_enabled { tl!("Bypass") } else { tl!("Enable") };
        if toolbar_button(
            app,
            ui,
            &mut bx,
            by1,
            ToolbarBtn {
                w: 56.0,
                h: btn_h1,
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

        let all_bypassed = graph.nodes.iter().all(|n| !n.enabled);
        if toolbar_button(
            app,
            ui,
            &mut bx,
            by1,
            ToolbarBtn {
                w: 36.0,
                h: btn_h1,
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
            by1,
            ToolbarBtn {
                w: 46.0,
                h: btn_h1,
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
                by1,
                ToolbarBtn {
                    w: 50.0,
                    h: btn_h1,
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
            by1,
            ToolbarBtn {
                w: 68.0,
                h: btn_h1,
                gap: 5.0,
                id: "color_nodes_org_btn",
                auto_id: "color_nodes.organize",
                auto_label: "Organizar Nodos",
                label: tl!("Organizar"),
                tooltip: Some(tl!("Alinear y organizar nodos")),
                active_tint: false,
            },
        ) {
            let prefix = format!("{scope_key}:");
            app.ui.color_node_positions.retain(|k, _| !k.starts_with(&prefix));
            let _ = clip;
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
            by1,
            ToolbarBtn {
                w: 68.0,
                h: btn_h1,
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
            by1,
            ToolbarBtn {
                w: 24.0,
                h: btn_h1,
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
            by1,
            ToolbarBtn {
                w: 44.0,
                h: btn_h1,
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
            by1,
            ToolbarBtn {
                w: 24.0,
                h: btn_h1,
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
        let info_r = Rect::from_min_max(pos2(toolbar_r.max.x - avail_right.min(220.0), toolbar_r.min.y), pos2(toolbar_r.max.x - 8.0, toolbar_r.min.y + 28.0));
        let mut right_text = format!("Master · {it_name}");
        if node_count > 0 {
            right_text = format!("Node {}/{} | {}", active_node + 1, node_count, right_text);
        }
        ui.painter().text(info_r.right_center(), Align2::RIGHT_CENTER, right_text, Tokens::ui(11.0), t.text_dim);
    }

    // Row 2: Scope Tabs (Pre-Clip | Clip | Post-Clip | Timeline) + DAG Node Creators (Parallel, Layer, Outside, Split, Shared)
    let mut bx2 = toolbar_r.min.x + 8.0;
    let by2 = toolbar_r.min.y + 31.0;
    let btn_h2 = 22.0;
    let cur_scope = app.ui.active_color_scope;

    for (sc, id_sfx, lbl, w) in [
        (ColorScopeKind::PreClip, "pre_clip", tl!("Pre-Clip"), 56.0),
        (ColorScopeKind::Clip, "clip", tl!("Clip"), 42.0),
        (ColorScopeKind::PostClip, "post_clip", tl!("Post-Clip"), 60.0),
        (ColorScopeKind::Timeline, "timeline", tl!("Timeline"), 58.0),
    ] {
        let auto_id = format!("color_nodes.scope.{id_sfx}");
        let btn_id = format!("color_nodes_scope_{id_sfx}");
        if toolbar_button(
            app,
            ui,
            &mut bx2,
            by2,
            ToolbarBtn {
                w,
                h: btn_h2,
                gap: 3.0,
                id: &btn_id,
                auto_id: &auto_id,
                auto_label: sc.label(),
                label: lbl,
                tooltip: None,
                active_tint: cur_scope == sc,
            },
        ) {
            if matches!(sc, ColorScopeKind::PreClip | ColorScopeKind::PostClip) && resolved_group.is_none() {
                app.ui.status = tl!("Assign the clip to a Color Group first to grade Pre-Clip or Post-Clip").into();
            } else {
                app.ui.active_color_scope = sc;
                if sc == ColorScopeKind::Clip {
                    app.ui.active_color_group = None;
                } else if let Some(gid) = resolved_group
                    && matches!(sc, ColorScopeKind::PreClip | ColorScopeKind::PostClip)
                {
                    app.ui.active_color_group = Some(gid);
                }
                app.ui.active_color_node = 0;
            }
        }
    }

    bx2 += 6.0;
    if toolbar_button(
        app,
        ui,
        &mut bx2,
        by2,
        ToolbarBtn {
            w: 62.0,
            h: btn_h2,
            gap: 4.0,
            id: "color_nodes_add_parallel_btn",
            auto_id: "color_nodes.add_parallel",
            auto_label: "Add Parallel Node",
            label: tl!("+ Parallel"),
            tooltip: Some(tl!("Add Parallel Mixer Branch (Alt+P)")),
            active_tint: false,
        },
    ) {
        act.add_parallel = true;
    }

    if toolbar_button(
        app,
        ui,
        &mut bx2,
        by2,
        ToolbarBtn {
            w: 54.0,
            h: btn_h2,
            gap: 4.0,
            id: "color_nodes_add_layer_btn",
            auto_id: "color_nodes.add_layer",
            auto_label: "Add Layer Mixer Node",
            label: tl!("+ Layer"),
            tooltip: Some(tl!("Add Layer Mixer Branch (Alt+L)")),
            active_tint: false,
        },
    ) {
        act.add_layer = true;
    }

    if toolbar_button(
        app,
        ui,
        &mut bx2,
        by2,
        ToolbarBtn {
            w: 62.0,
            h: btn_h2,
            gap: 4.0,
            id: "color_nodes_add_outside_btn",
            auto_id: "color_nodes.add_outside",
            auto_label: "Add Outside Node",
            label: tl!("+ Outside"),
            tooltip: Some(tl!("Add Outside Node with Inverted Key (Alt+O)")),
            active_tint: false,
        },
    ) {
        act.add_outside = true;
    }

    if toolbar_button(
        app,
        ui,
        &mut bx2,
        by2,
        ToolbarBtn {
            w: 52.0,
            h: btn_h2,
            gap: 4.0,
            id: "color_nodes_add_splitter_btn",
            auto_id: "color_nodes.add_splitter",
            auto_label: "Add Splitter Combiner",
            label: tl!("+ Split"),
            tooltip: Some(tl!("Add Channel Splitter/Combiner (Alt+Y)")),
            active_tint: false,
        },
    ) {
        act.add_splitter = true;
    }

    if toolbar_button(
        app,
        ui,
        &mut bx2,
        by2,
        ToolbarBtn {
            w: 58.0,
            h: btn_h2,
            gap: 4.0,
            id: "color_nodes_add_shared_btn",
            auto_id: "color_nodes.add_shared",
            auto_label: "Add Shared Node",
            label: tl!("+ Shared"),
            tooltip: Some(tl!("Add Project-Wide Shared Node")),
            active_tint: false,
        },
    ) {
        act.add_shared = true;
    }

    toolbar_r
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn execute_graph_actions(
    app: &mut FilmcraftApp,
    scope: ColorScopeKind,
    clip: ClipId,
    group_id: Option<ColorGroupId>,
    graph: &ColorGraph,
    lumetri_effects: &[(usize, EffectInstance)],
    active_node: usize,
    act: &NodeGraphActions,
) -> bool {
    let node_count = graph.nodes.len();
    let active_nid = graph.nodes.get(active_node).map(|n| n.id);

    // Non-linear node additions always go through `color.graph.addNode`
    for (flag, kind) in
        [(act.add_parallel, "parallel"), (act.add_layer, "layer"), (act.add_outside, "outside"), (act.add_splitter, "splitter"), (act.add_shared, "shared")]
    {
        if flag {
            let mut p = json!({"kind": kind});
            if let Some(aid) = active_nid {
                p["after"] = json!(aid.0);
            }
            if let Ok(res) = app.session.execute("color.graph.addNode", scope_params(scope, clip, group_id, p))
                && let Some(new_id) = res.get("node").and_then(Value::as_u64)
            {
                app.ui.active_color_node = node_count;
                let _ = new_id;
            }
            return true;
        }
    }

    if act.add_serial {
        if scope == ColorScopeKind::Clip && lumetri_effects.len() == node_count {
            let _ = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}));
        } else {
            let mut p = json!({"kind": "serial"});
            if let Some(aid) = active_nid {
                p["after"] = json!(aid.0);
            }
            let _ = app.session.execute("color.graph.addNode", scope_params(scope, clip, group_id, p));
        }
        app.ui.active_color_node = node_count;
        return true;
    }

    if act.bypass_active && node_count > 0 {
        if scope == ColorScopeKind::Clip
            && let Some(&(idx, _)) = lumetri_effects.get(active_node)
            && lumetri_effects.len() == node_count
        {
            let _ = app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": idx}));
        } else if let Some(aid) = active_nid {
            let _ = app.session.execute("color.graph.toggleBypass", scope_params(scope, clip, group_id, json!({"node": aid.0})));
        }
        return true;
    }

    if act.bypass_all && node_count > 0 {
        if scope == ColorScopeKind::Clip && lumetri_effects.len() == node_count {
            let any_enabled = lumetri_effects.iter().any(|(_, e)| e.enabled);
            let _ = app.session.edit_group("Bypass All Color Nodes", |s| {
                for (idx, e) in lumetri_effects {
                    if e.enabled == any_enabled {
                        s.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": *idx}))?;
                    }
                }
                Ok(Value::Null)
            });
        } else {
            let any_enabled = graph.nodes.iter().any(|n| n.enabled);
            let nids: Vec<(ColorNodeId, bool)> = graph.nodes.iter().map(|n| (n.id, n.enabled)).collect();
            let _ = app.session.edit_group("Bypass All Color Nodes", |s| {
                for (nid, en) in &nids {
                    if *en == any_enabled {
                        s.execute("color.graph.toggleBypass", scope_params(scope, clip, group_id, json!({"node": nid.0})))?;
                    }
                }
                Ok(Value::Null)
            });
        }
        return true;
    }

    if act.reset_active && node_count > 0 {
        if scope == ColorScopeKind::Clip
            && let Some(&(idx, _)) = lumetri_effects.get(active_node)
            && lumetri_effects.len() == node_count
        {
            let _ = app.session.execute("effects.reset", json!({"clip": clip.0, "index": idx}));
        } else if let Some(aid) = active_nid {
            let _ = app.session.execute("color.graph.resetNode", scope_params(scope, clip, group_id, json!({"node": aid.0})));
        }
        return true;
    }

    if act.delete_active && node_count > 1 {
        if scope == ColorScopeKind::Clip
            && let Some(&(idx, _)) = lumetri_effects.get(active_node)
            && lumetri_effects.len() == node_count
        {
            let _ = app.session.execute("effects.remove", json!({"clip": clip.0, "index": idx}));
        } else if let Some(aid) = active_nid {
            let _ = app.session.execute("color.graph.deleteNode", scope_params(scope, clip, group_id, json!({"node": aid.0})));
        }
        if active_node >= node_count - 1 {
            app.ui.active_color_node = active_node.saturating_sub(1);
        }
        return true;
    }
    false
}

struct NodeCardCtx<'a> {
    scope: ColorScopeKind,
    clip_id: ClipId,
    group_id: Option<ColorGroupId>,
    scope_key: u64,
    i: usize,
    node: &'a ColorNode,
    fx_idx: Option<usize>,
    node_rect: Rect,
    default_rel: [f32; 2],
    zoom: f32,
    active_node: usize,
    node_count: usize,
    has_custom_dag: bool,
}

#[allow(clippy::cognitive_complexity)]
fn paint_node_card(app: &mut FilmcraftApp, ui: &mut egui::Ui, painter: &egui::Painter, c: NodeCardCtx<'_>) {
    let t = app.tokens;
    let i = c.i;
    let node_rect = c.node_rect;
    let zoom = c.zoom;
    let is_active = i == c.active_node;
    let is_enabled = c.node.enabled;

    let card_resp = ui.interact(node_rect, egui::Id::unique(("color_node_card", c.scope_key, i)), Sense::click_and_drag());
    app.auto.add(&format!("color_nodes.node.{i}"), node_rect, &format!("Node {:02}", i + 1));

    if card_resp.dragged() {
        let delta = card_resp.drag_delta();
        let key = format!("{}:{i}", c.scope_key);
        let pos = app.ui.color_node_positions.entry(key).or_insert(c.default_rel);
        pos[0] += delta.x / zoom;
        pos[1] += delta.y / zoom;
        if c.has_custom_dag {
            let new_pos = *pos;
            let _ = app
                .session
                .execute("color.graph.moveNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0, "pos": new_pos, "merge": true})));
        }
    }
    if card_resp.clicked() {
        app.ui.active_color_node = i;
    }

    let bg_color =
        if is_enabled { if is_active { Color32::from_rgb(32, 35, 42) } else { Color32::from_rgb(28, 30, 36) } } else { Color32::from_rgb(22, 23, 27) };
    let card_corner = (6.0 * zoom).clamp(3.0, 6.0);
    painter.rect_filled(node_rect, card_corner, bg_color);

    let header_h = (22.0 * zoom).clamp(14.0, 22.0);
    let header_rect = Rect::from_min_size(node_rect.min, vec2(node_rect.width(), header_h));
    let header_bg = match &c.node.kind {
        ColorNodeKind::Outside { .. } => Color32::from_rgb(28, 46, 68),
        ColorNodeKind::ParallelMixer { .. } => Color32::from_rgb(32, 54, 44),
        ColorNodeKind::LayerMixer { .. } => Color32::from_rgb(52, 38, 66),
        ColorNodeKind::Splitter { .. } | ColorNodeKind::Combiner { .. } => Color32::from_rgb(54, 46, 30),
        ColorNodeKind::Shared { .. } => Color32::from_rgb(30, 56, 58),
        ColorNodeKind::Corrector { .. } => {
            if is_active {
                Color32::from_rgb(38, 42, 52)
            } else {
                Color32::from_rgb(22, 24, 28)
            }
        }
    };
    painter.rect_filled(header_rect, card_corner, header_bg);
    painter.rect_filled(Rect::from_min_max(pos2(header_rect.min.x, header_rect.max.y - 3.0 * zoom), header_rect.max), 0.0, header_bg);

    let type_tag = c.node.kind.type_badge();
    let num_str = if type_tag == "COR" { format!("{:02}", i + 1) } else { format!("{:02} {type_tag}", i + 1) };
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
    let power_resp = ui.interact(power_r, egui::Id::unique(("node_power", c.scope_key, i)), Sense::click());
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
        if c.scope == ColorScopeKind::Clip
            && !c.has_custom_dag
            && let Some(fx_idx) = c.fx_idx
        {
            let _ = app.session.execute("effects.toggleEnabled", json!({"clip": c.clip_id.0, "index": fx_idx}));
        } else {
            let _ = app.session.execute("color.graph.toggleBypass", scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0})));
        }
    }

    if zoom >= 0.55 {
        let label_key = format!("{}:{i}", c.scope_key);
        let custom_label = c.node.label.clone().or_else(|| app.ui.color_node_labels.get(&label_key).cloned());
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
            let _ = app.session.execute("color.graph.addNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"kind": "serial", "after": c.node.id.0})));
            app.ui.active_color_node = c.node_count;
            ui.close();
        }
        if ui.button(tl!("Add Parallel Node (Alt+P)")).clicked() {
            let _ = app.session.execute("color.graph.addNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"kind": "parallel", "after": c.node.id.0})));
            app.ui.active_color_node = c.node_count;
            ui.close();
        }
        if ui.button(tl!("Add Layer Mixer Node (Alt+L)")).clicked() {
            let _ = app.session.execute("color.graph.addNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"kind": "layer", "after": c.node.id.0})));
            app.ui.active_color_node = c.node_count;
            ui.close();
        }
        if ui.button(tl!("Add Outside Node (Alt+O)")).clicked() {
            let _ = app.session.execute("color.graph.addNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"kind": "outside", "after": c.node.id.0})));
            app.ui.active_color_node = c.node_count;
            ui.close();
        }
        if ui.button(tl!("Add Splitter/Combiner (Alt+Y)")).clicked() {
            let _ = app.session.execute(
                "color.graph.addNode",
                scope_params(c.scope, c.clip_id, c.group_id, json!({"kind": "splitter", "after": c.node.id.0, "space": "ycbcr"})),
            );
            app.ui.active_color_node = c.node_count;
            ui.close();
        }
        ui.separator();
        if let ColorNodeKind::LayerMixer { inputs, blend_modes, .. } = &c.node.kind {
            for p in 1..*inputs {
                let cur_bm = blend_modes.get(p).copied().unwrap_or(LayerBlendMode::Normal);
                ui.menu_button(format!("Layer {p} Blend: {}", cur_bm.label()), |ui| {
                    for bm in LayerBlendMode::ALL {
                        if ui.selectable_label(bm == cur_bm, bm.label()).clicked() {
                            let _ = app.session.execute(
                                "color.graph.setLayerMixer",
                                scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0, "port": p, "blend": bm.label()})),
                            );
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
        }
        if matches!(c.node.kind, ColorNodeKind::Corrector { .. } | ColorNodeKind::Outside { .. }) && ui.button(tl!("Save as Shared Node")).clicked() {
            let _ = app.session.execute("color.graph.shareNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0})));
            ui.close();
        }
        if matches!(c.node.kind, ColorNodeKind::Shared { .. }) && ui.button(tl!("Unlink Shared Node")).clicked() {
            let _ = app.session.execute("color.graph.unlinkShared", scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0})));
            ui.close();
        }
        let toggle_label = if is_enabled { tl!("Bypass Node (Ctrl+D)") } else { tl!("Enable Node (Ctrl+D)") };
        if ui.button(toggle_label).clicked() {
            let _ = app.session.execute("color.graph.toggleBypass", scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0})));
            ui.close();
        }
        if ui.button(tl!("Reset Node")).clicked() {
            let _ = app.session.execute("color.graph.resetNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0})));
            ui.close();
        }
        if c.node_count > 1 && ui.button(tl!("Delete Node")).clicked() {
            let _ = app.session.execute("color.graph.deleteNode", scope_params(c.scope, c.clip_id, c.group_id, json!({"node": c.node.id.0})));
            if c.active_node >= c.node_count - 1 {
                app.ui.active_color_node = c.active_node.saturating_sub(1);
            }
            ui.close();
        }
    });
}

fn paint_node_badges(ui: &mut egui::Ui, painter: &egui::Painter, c: &NodeCardCtx<'_>) {
    let badges: Vec<(&'static str, &'static str)> = match &c.node.kind {
        ColorNodeKind::Corrector { effect, .. } => detect_node_badges(effect),
        ColorNodeKind::Outside { effect, .. } => {
            let mut b = vec![("OUT", "Outside Key Node")];
            b.extend(detect_node_badges(effect));
            b
        }
        ColorNodeKind::ParallelMixer { .. } => vec![("PAR", "Parallel Mixer")],
        ColorNodeKind::LayerMixer { blend_modes, .. } => {
            let bm = blend_modes.get(1).copied().unwrap_or(LayerBlendMode::Normal);
            vec![("LAY", bm.label())]
        }
        ColorNodeKind::Splitter { .. } => vec![("SPL", "Channel Splitter")],
        ColorNodeKind::Combiner { .. } => vec![("CMB", "Channel Combiner")],
        ColorNodeKind::Shared { .. } => vec![("SHR", "Shared Node")],
    };
    let zoom = c.zoom;
    let is_enabled = c.node.enabled;
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
            "SLC" => Color32::from_rgb(240, 105, 165),
            "WRP" => Color32::from_rgb(95, 200, 225),
            "OUT" => Color32::from_rgb(85, 175, 250),
            "PAR" => Color32::from_rgb(90, 220, 135),
            "LAY" => Color32::from_rgb(205, 140, 250),
            "SPL" | "CMB" => Color32::from_rgb(240, 190, 95),
            "SHR" => Color32::from_rgb(95, 230, 220),
            _ => Color32::from_rgb(140, 150, 165),
        };
        painter.text(br.center(), Align2::CENTER_CENTER, badge_text, Tokens::ui((9.0 * zoom).clamp(7.0, 9.0)), txt_col);
        ui.interact(br, egui::Id::unique(("badge", c.scope_key, c.i, badge_text)), Sense::hover()).on_hover_text(badge_tip);

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

        fx.param_mut("cs_on").unwrap().value = ParamValue::Bool(true);
        assert!(!detect_node_badges(&fx).iter().any(|(b, _)| *b == "SLC"), "default ColorSlice params do not trigger SLC badge");
        fx.param_mut("cs_red_sat").unwrap().value = ParamValue::Float(130.0);
        assert!(detect_node_badges(&fx).iter().any(|(b, _)| *b == "SLC"), "edited ColorSlice band triggers SLC badge");

        fx.param_mut("cw_on").unwrap().value = ParamValue::Bool(true);
        assert!(!detect_node_badges(&fx).iter().any(|(b, _)| *b == "WRP"), "empty Color Warper grid does not trigger WRP badge");
        fx.param_mut("cw_hs_grid").unwrap().value = ParamValue::Curve(vec![[0.05, -0.04]]);
        assert!(detect_node_badges(&fx).iter().any(|(b, _)| *b == "WRP"), "edited Color Warper grid triggers WRP badge");
    }
}
