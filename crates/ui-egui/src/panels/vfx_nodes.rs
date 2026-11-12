//! DaVinci Resolve / Fusion-style VFX Node Pipeline panel (`PanelKind::VfxNodes`).
//!
//! Visualizes the active clip's visual effects stack (EffectCraft VFX `"ec.*"` and standard
//! video effects) as an interactive serial node chain:
//! `[ IN ] ──► [ 01: Glow ] ──► [ 02: Turbulent Displace ] ──► ... ──► [ OUT ]`
//!
//! Clicking a node selects it in `Effect Controls` (`effects.select`) and reveals its interactive
//! 2D gizmos on the `Program Monitor`. Effects dragged from the `Effects` library panel can also
//! be dropped directly onto the node graph canvas to append them to the chain.
//!
//! Keyboard shortcuts:
//! - `Alt + S`: Add Quick VFX Node (Glow)
//! - `Ctrl + D` / `Cmd + D`: Bypass Active Node
//! - `Shift + D`: Bypass All VFX Nodes
//! - `Delete` / `Backspace`: Delete Active Node
//! - `Alt + Home`: Reset Active Node
//! - `Alt + F` / `Z`: Fit all nodes to canvas (Ajustar)
//! - `Ctrl + =` / `Ctrl + +`: Zoom In
//! - `Ctrl + -`: Zoom Out
//! - `Ctrl + 0`: Reset Zoom (100%)

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{ClipId, EffectDef, EffectInstance, EffectKind};
use serde_json::json;

use crate::FilmcraftApp;
use crate::panels::lumetri::active_color_clip;
use crate::theme::Tokens;

/// Featured EffectCraft VFX quick-add presets shown in the `+ VFX` popup menu.
const QUICK_VFX_PRESETS: &[(&str, &str, &str)] = &[
    ("ec.stylize.glow", "Glow", "Stylize"),
    ("ec.blur.gaussian", "Gaussian Blur", "Blur & Sharpen"),
    ("ec.distort.turbulentdisplace", "Turbulent Displace", "Distort"),
    ("ec.color.colorama", "Colorama", "Color Correction"),
    ("ec.perspective.dropshadow", "Drop Shadow", "Perspective"),
    ("ec.generate.lensflare", "Lens Flare", "Generate"),
    ("ec.noise.fractal", "Fractal Noise", "Noise & Grain"),
    ("ec.sim.ccparticleworld", "Particle World", "Simulation"),
    ("ec.keying.keylight", "Keylight (1.2)", "Keying"),
    ("ec.time.echo", "Echo", "Time"),
    ("ec.distort.opticscompensation", "Optics Compensation", "Distort"),
    ("ec.stylize.findedges", "Find Edges", "Stylize"),
];

/// Compute category & capability badges for a VFX node card.
fn detect_vfx_badges(e: &EffectInstance, def: &EffectDef) -> Vec<(&'static str, &'static str)> {
    let mut badges = Vec::new();
    let sub = def.category.get(1).copied().unwrap_or("");
    let cat_badge = match sub {
        "Blur & Sharpen" => ("BLUR", "Blur & Sharpen"),
        "Stylize" | "Lights & Glows" => ("STYL", "Stylize / Glow"),
        "Distort" => ("DIST", "Distort"),
        "Color Correction" => ("COL", "Color Correction"),
        "Generate" => ("GEN", "Generate"),
        "Keying" | "Matte" => ("KEY", "Keying & Matte"),
        "Simulation" => ("SIM", "Simulation"),
        "Perspective" => ("3D", "Perspective"),
        "Noise & Grain" => ("NOIS", "Noise & Grain"),
        "Time" => ("TIME", "Time"),
        "Transition" => ("TRNS", "Transition"),
        "Channel" | "3D Channel" => ("CHN", "Channel"),
        _ => ("VFX", "Visual Effect"),
    };
    badges.push(cat_badge);

    if e.effect.starts_with("ec.") {
        badges.push(("EC", "EffectCraft VFX Engine"));
    }
    if def.accelerated {
        badges.push(("GPU", "GPU Accelerated"));
    } else if def.float32 {
        badges.push(("32f", "32-bit Float Pipeline"));
    }
    if e.params.values().any(|p| p.is_animated() || !p.keyframes.is_empty()) {
        badges.push(("KF", "Animated Keyframes"));
    }
    if !e.masks.is_empty() {
        badges.push(("MSK", "Effect Mask"));
    }
    badges
}

/// Draw a smooth cubic bezier connection line between two pins.
fn draw_bezier_wire(painter: &egui::Painter, p0: Pos2, p1: Pos2, stroke: Stroke) {
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
}

/// Select a VFX node and focus it in `Effect Controls`.
fn select_vfx_node(app: &mut FilmcraftApp, clip: ClipId, node_idx: usize, fx_idx: usize) {
    app.ui.active_vfx_node = node_idx;
    let key = format!("{}:{fx_idx}", clip.0);
    app.ui.collapsed_fx.retain(|k| *k != key);
    let _ = app.session.execute("effects.select", json!({"clip": clip.0, "effect": fx_idx}));
}

/// Accept an effect dropped from the `Effects` library panel onto the VFX Nodes canvas.
fn handle_effect_drop(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, clip: ClipId, next_node_idx: usize) {
    let Some(effect) = crate::panels::dragged_effect(ui) else { return };
    let is_transition = filmcraft_project::find_effect(&effect)
        .is_some_and(|d| matches!(d.kind, EffectKind::VideoTransition | EffectKind::AudioTransition));
    let ctx = ui.ctx().clone();
    let over = ctx.pointer_hover_pos().is_some_and(|p| rect.contains(p) && ctx.layer_id_at(p) == Some(ui.layer_id()));
    if is_transition || !over {
        return;
    }
    app.auto.add("vfx_nodes.dropTarget", rect, "drop an effect here");
    ui.painter().rect_stroke(rect.shrink(1.0), 3.0, Stroke::new(2.0, Color32::from_rgb(130, 100, 255)), StrokeKind::Inside);
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    let res = if let Some(name) = effect.strip_prefix("preset:") {
        app.session.execute("presets.apply", json!({"preset": name, "clips": [clip.0]}))
    } else {
        app.session.execute("effects.apply", json!({"effect": effect, "clips": [clip.0]}))
    };
    match res {
        Ok(v) => {
            if let Some(fx_idx) = v["applied"].as_array().and_then(|a| a.first()).and_then(|e| e["index"].as_u64()) {
                select_vfx_node(app, clip, next_node_idx, fx_idx as usize);
            }
        }
        Err(e) => app.ui.status = e.to_string(),
    }
    crate::panels::clear_drag(ui);
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;

    // Outer frame with dark background
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(20, 22, 26));

    let Some((clip, it)) = active_color_clip(app) else {
        crate::dock::placeholder(ui, rect, &t, tl!("(no clip selected)"));
        return;
    };

    // Collect all non-intrinsic video effects (excluding Lumetri which belongs to Color Nodes)
    let vfx_effects: Vec<(usize, &EffectInstance, &'static EffectDef)> = it
        .effects
        .iter()
        .enumerate()
        .filter_map(|(idx, e)| {
            let def = e.def()?;
            (!def.intrinsic && def.kind == EffectKind::Video && e.effect != "lumetri").then_some((idx, e, def))
        })
        .collect();

    let node_count = vfx_effects.len();

    // Sync active_vfx_node if the user selected an effect inside Effect Controls
    if let Some(sel_idx) = app.session.state.selected_effect.as_ref().filter(|s| s.clip == clip).and_then(|s| s.index(&it.effects))
        && let Some(pos) = vfx_effects.iter().position(|(idx, _, _)| *idx == sel_idx)
    {
        app.ui.active_vfx_node = pos;
    }

    let active_node = if node_count == 0 {
        0
    } else {
        app.ui.active_vfx_node.min(node_count - 1)
    };
    app.ui.active_vfx_node = active_node;

    handle_effect_drop(app, ui, rect, clip, node_count);

    // Handle Keyboard Shortcuts when typing isn't active
    let mut add_fx_id: Option<&'static str> = None;
    let mut bypass_active = false;
    let mut bypass_all = false;
    let mut delete_active = false;
    let mut reset_active = false;
    let mut fit_view = false;
    let mut zoom_in_action = false;
    let mut zoom_out_action = false;
    let mut zoom_reset_action = false;

    if !ui.ctx().egui_wants_keyboard_input() && ui.rect_contains_pointer(rect) {
        ui.input(|i| {
            if i.modifiers.alt && i.key_pressed(egui::Key::S) {
                add_fx_id = Some("ec.stylize.glow");
            } else if (i.modifiers.command || i.modifiers.ctrl) && !i.modifiers.shift && i.key_pressed(egui::Key::D) {
                bypass_active = true;
            } else if i.modifiers.shift && i.key_pressed(egui::Key::D) {
                bypass_all = true;
            } else if (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && node_count > 0 {
                delete_active = true;
            } else if i.modifiers.alt && i.key_pressed(egui::Key::Home) {
                reset_active = true;
            } else if (i.modifiers.alt && i.key_pressed(egui::Key::F)) || i.key_pressed(egui::Key::Z) {
                fit_view = true;
            } else if (i.modifiers.command || i.modifiers.ctrl) && (i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
                zoom_in_action = true;
            } else if (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::Minus) {
                zoom_out_action = true;
            } else if (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::Num0) {
                zoom_reset_action = true;
            }
        });
    }

    // Top Toolbar
    let toolbar_h = 34.0;
    let toolbar_r = Rect::from_min_size(rect.min, vec2(rect.width(), toolbar_h));
    ui.painter().rect_filled(toolbar_r, 0.0, Color32::from_rgb(26, 28, 33));
    ui.painter().line_segment(
        [pos2(toolbar_r.min.x, toolbar_r.max.y), pos2(toolbar_r.max.x, toolbar_r.max.y)],
        Stroke::new(1.0, Color32::from_rgb(38, 41, 48)),
    );

    let mut bx = toolbar_r.min.x + 8.0;
    let by = toolbar_r.min.y + 5.0;
    let btn_h = 24.0;

    // + VFX Quick Add Button (click opens popup menu of featured EffectCraft effects)
    let add_w = 76.0;
    let add_r = Rect::from_min_size(pos2(bx, by), vec2(add_w, btn_h));
    let add_id = egui::Id::unique("vfx_nodes_add_btn");
    let add_resp = ui.interact(add_r, add_id, Sense::click()).on_hover_text(tl!("Add EffectCraft VFX Node (Alt+S)"));
    app.auto.add("vfx_nodes.add", add_r, "Add VFX Node");
    let add_bg = if add_resp.hovered() { t.hover } else { Color32::from_rgb(42, 38, 62) };
    ui.painter().rect_filled(add_r, 4.0, add_bg);
    ui.painter().rect_stroke(add_r, 4.0, Stroke::new(1.0, Color32::from_rgb(95, 80, 155)), StrokeKind::Inside);
    ui.painter().text(add_r.center(), Align2::CENTER_CENTER, tl!("+ VFX ▾"), Tokens::ui(11.0), t.text);
    let popup_id = egui::Id::unique("vfx_nodes_quick_popup");
    let mut popup_open = ui.ctx().data(|d| d.get_temp::<bool>(popup_id)).unwrap_or(false);
    let mut just_opened = false;
    if add_resp.clicked() {
        popup_open = !popup_open;
        just_opened = popup_open;
        ui.ctx().data_mut(|d| d.insert_temp(popup_id, popup_open));
    }
    if popup_open {
        let area = egui::Area::new(popup_id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(add_r.left_bottom() + vec2(0.0, 2.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(210.0);
                    ui.label(egui::RichText::new(tl!("EffectCraft VFX")).strong().size(11.0));
                    ui.separator();
                    for &(fx_id, label, cat) in QUICK_VFX_PRESETS {
                        let row_txt = format!("{}  ({})", crate::i18n::t(label), crate::i18n::t(cat));
                        if ui.button(row_txt).clicked() {
                            add_fx_id = Some(fx_id);
                        }
                    }
                });
            });
        if add_fx_id.is_some() || (!just_opened && area.response.clicked_elsewhere()) {
            ui.ctx().data_mut(|d| d.remove::<bool>(popup_id));
        }
    }
    bx += add_w + 5.0;

    if node_count > 0 {
        // Bypass Node
        let curr_enabled = vfx_effects[active_node].1.enabled;
        let bypass_w = 56.0;
        let bypass_r = Rect::from_min_size(pos2(bx, by), vec2(bypass_w, btn_h));
        let bypass_resp = ui.interact(bypass_r, egui::Id::unique("vfx_nodes_bypass_btn"), Sense::click());
        app.auto.add("vfx_nodes.bypass_node", bypass_r, "Bypass Node");
        let bypass_bg = if bypass_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(bypass_r, 4.0, bypass_bg);
        ui.painter().rect_stroke(bypass_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        let b_text = if curr_enabled { tl!("Bypass") } else { tl!("Enable") };
        ui.painter().text(bypass_r.center(), Align2::CENTER_CENTER, b_text, Tokens::ui(11.0), t.text);
        if bypass_resp.clicked() {
            bypass_active = true;
        }
        bx += bypass_w + 5.0;

        // Bypass All
        let all_w = 36.0;
        let all_r = Rect::from_min_size(pos2(bx, by), vec2(all_w, btn_h));
        let all_resp = ui.interact(all_r, egui::Id::unique("vfx_nodes_bypass_all_btn"), Sense::click());
        app.auto.add("vfx_nodes.bypass_all", all_r, "Bypass All Nodes");
        let all_bg = if all_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(all_r, 4.0, all_bg);
        ui.painter().rect_stroke(all_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        ui.painter().text(all_r.center(), Align2::CENTER_CENTER, tl!("All"), Tokens::ui(11.0), t.text);
        if all_resp.clicked() {
            bypass_all = true;
        }
        bx += all_w + 5.0;

        // Reset Node
        let rst_w = 46.0;
        let rst_r = Rect::from_min_size(pos2(bx, by), vec2(rst_w, btn_h));
        let rst_resp = ui.interact(rst_r, egui::Id::unique("vfx_nodes_rst_btn"), Sense::click());
        app.auto.add("vfx_nodes.reset_node", rst_r, "Reset Node");
        let rst_bg = if rst_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(rst_r, 4.0, rst_bg);
        ui.painter().rect_stroke(rst_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        ui.painter().text(rst_r.center(), Align2::CENTER_CENTER, tl!("Reset"), Tokens::ui(11.0), t.text);
        if rst_resp.clicked() {
            reset_active = true;
        }
        bx += rst_w + 5.0;

        // Delete Node
        let del_w = 50.0;
        let del_r = Rect::from_min_size(pos2(bx, by), vec2(del_w, btn_h));
        let del_resp = ui.interact(del_r, egui::Id::unique("vfx_nodes_del_btn"), Sense::click());
        app.auto.add("vfx_nodes.delete_node", del_r, "Delete Node");
        let del_bg = if del_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(del_r, 4.0, del_bg);
        ui.painter().rect_stroke(del_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        ui.painter().text(del_r.center(), Align2::CENTER_CENTER, tl!("Delete"), Tokens::ui(11.0), t.text);
        if del_resp.clicked() {
            delete_active = true;
        }
        bx += del_w + 5.0;

        // Organizar Nodos
        let org_w = 68.0;
        let org_r = Rect::from_min_size(pos2(bx, by), vec2(org_w, btn_h));
        let org_resp = ui.interact(org_r, egui::Id::unique("vfx_nodes_org_btn"), Sense::click()).on_hover_text(tl!("Alinear y organizar nodos en serie"));
        app.auto.add("vfx_nodes.organize", org_r, "Organizar Nodos");
        let org_bg = if org_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(org_r, 4.0, org_bg);
        ui.painter().rect_stroke(org_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        ui.painter().text(org_r.center(), Align2::CENTER_CENTER, tl!("Organizar"), Tokens::ui(11.0), t.text);
        if org_resp.clicked() {
            let prefix = format!("{}:", clip.0);
            app.ui.vfx_node_positions.retain(|k, _| !k.starts_with(&prefix));
            app.ui.vfx_node_pan = [0.0, 0.0];
            app.ui.vfx_node_zoom = 1.0;
            if node_count > 4 {
                fit_view = true;
            }
        }
        bx += org_w + 5.0;

        // ⛶ Ajustar (Fit to View)
        let fit_w = 68.0;
        let fit_r = Rect::from_min_size(pos2(bx, by), vec2(fit_w, btn_h));
        let fit_resp = ui.interact(fit_r, egui::Id::unique("vfx_nodes_fit_btn"), Sense::click()).on_hover_text(tl!("Ajustar nodos a la pantalla (Alt+F / Z)"));
        app.auto.add("vfx_nodes.fit", fit_r, "Ajustar Nodos");
        let fit_bg = if fit_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(fit_r, 4.0, fit_bg);
        ui.painter().rect_stroke(fit_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        ui.painter().text(fit_r.center(), Align2::CENTER_CENTER, tl!("⛶ Ajustar"), Tokens::ui(11.0), t.text);
        if fit_resp.clicked() {
            fit_view = true;
        }
        bx += fit_w + 5.0;

        // Zoom Out [-]
        let zm_w = 24.0;
        let zm_r = Rect::from_min_size(pos2(bx, by), vec2(zm_w, btn_h));
        let zm_resp = ui.interact(zm_r, egui::Id::unique("vfx_nodes_zm_btn"), Sense::click()).on_hover_text(tl!("Reducir zoom (Ctrl+-)"));
        app.auto.add("vfx_nodes.zoom_out", zm_r, "Zoom Out");
        let zm_bg = if zm_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(zm_r, 4.0, zm_bg);
        ui.painter().rect_stroke(zm_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        ui.painter().text(zm_r.center(), Align2::CENTER_CENTER, "-", Tokens::ui(12.0), t.text);
        if zm_resp.clicked() {
            zoom_out_action = true;
        }
        bx += zm_w + 2.0;

        // Zoom Reset [ 100% ]
        let zr_w = 44.0;
        let zr_r = Rect::from_min_size(pos2(bx, by), vec2(zr_w, btn_h));
        let zr_resp = ui.interact(zr_r, egui::Id::unique("vfx_nodes_zr_btn"), Sense::click()).on_hover_text(tl!("Restablecer zoom al 100% (Ctrl+0)"));
        app.auto.add("vfx_nodes.zoom_reset", zr_r, "Zoom 100%");
        let zr_bg = if zr_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(zr_r, 4.0, zr_bg);
        ui.painter().rect_stroke(zr_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        let z_txt = format!("{:.0}%", app.ui.vfx_node_zoom * 100.0);
        ui.painter().text(zr_r.center(), Align2::CENTER_CENTER, z_txt, Tokens::ui(10.0), t.text);
        if zr_resp.clicked() {
            zoom_reset_action = true;
        }
        bx += zr_w + 2.0;

        // Zoom In [+]
        let zp_w = 24.0;
        let zp_r = Rect::from_min_size(pos2(bx, by), vec2(zp_w, btn_h));
        let zp_resp = ui.interact(zp_r, egui::Id::unique("vfx_nodes_zp_btn"), Sense::click()).on_hover_text(tl!("Aumentar zoom (Ctrl++)"));
        app.auto.add("vfx_nodes.zoom_in", zp_r, "Zoom In");
        let zp_bg = if zp_resp.hovered() { t.hover } else { Color32::from_rgb(36, 40, 48) };
        ui.painter().rect_filled(zp_r, 4.0, zp_bg);
        ui.painter().rect_stroke(zp_r, 4.0, Stroke::new(1.0, Color32::from_rgb(55, 60, 72)), StrokeKind::Inside);
        ui.painter().text(zp_r.center(), Align2::CENTER_CENTER, "+", Tokens::ui(12.0), t.text);
        if zp_resp.clicked() {
            zoom_in_action = true;
        }
        bx += zp_w + 6.0;
    }

    // Right info: VFX node counter & clip name
    let avail_right = (toolbar_r.max.x - bx - 8.0).max(0.0);
    if avail_right > 110.0 {
        let info_r = Rect::from_min_max(pos2(toolbar_r.max.x - avail_right.min(240.0), toolbar_r.min.y), pos2(toolbar_r.max.x - 8.0, toolbar_r.max.y));
        let right_text = if node_count > 0 {
            format!("FX {}/{} | {}", active_node + 1, node_count, it.name)
        } else {
            format!("VFX · {}", it.name)
        };
        ui.painter().text(info_r.right_center(), Align2::RIGHT_CENTER, right_text, Tokens::ui(11.0), t.text_dim);
    }

    // Execute Shortcut / Action requests
    if let Some(fx_id) = add_fx_id {
        if let Ok(v) = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": fx_id}))
            && let Some(fx_idx) = v["applied"].as_array().and_then(|a| a.first()).and_then(|e| e["index"].as_u64())
        {
            select_vfx_node(app, clip, node_count, fx_idx as usize);
        }
        return;
    }
    if bypass_active && node_count > 0 {
        let (idx, _, _) = vfx_effects[active_node];
        let _ = app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": idx}));
        return;
    }
    if bypass_all && node_count > 0 {
        let any_enabled = vfx_effects.iter().any(|(_, e, _)| e.enabled);
        for &(idx, e, _) in &vfx_effects {
            if e.enabled == any_enabled {
                let _ = app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": idx}));
            }
        }
        return;
    }
    if reset_active && node_count > 0 {
        let (idx, _, _) = vfx_effects[active_node];
        let _ = app.session.execute("effects.reset", json!({"clip": clip.0, "index": idx}));
        return;
    }
    if delete_active && node_count > 0 {
        let (idx, _, _) = vfx_effects[active_node];
        let _ = app.session.execute("effects.remove", json!({"clip": clip.0, "index": idx}));
        if active_node >= node_count - 1 {
            app.ui.active_vfx_node = active_node.saturating_sub(1);
        }
        return;
    }

    // Canvas Area
    let canvas_r = Rect::from_min_max(pos2(rect.min.x, toolbar_r.max.y), rect.max);
    if canvas_r.width() < 10.0 || canvas_r.height() < 10.0 {
        return;
    }

    // Handle Zoom actions from buttons / shortcuts
    if zoom_in_action {
        let old_zoom = app.ui.vfx_node_zoom;
        let new_zoom = (old_zoom * 1.2).clamp(0.25, 2.5);
        let center = canvas_r.size() * 0.5;
        let old_pan = vec2(app.ui.vfx_node_pan[0], app.ui.vfx_node_pan[1]);
        let new_pan = center - (center - old_pan) * (new_zoom / old_zoom);
        app.ui.vfx_node_pan = [new_pan.x, new_pan.y];
        app.ui.vfx_node_zoom = new_zoom;
    }
    if zoom_out_action {
        let old_zoom = app.ui.vfx_node_zoom;
        let new_zoom = (old_zoom / 1.2).clamp(0.25, 2.5);
        let center = canvas_r.size() * 0.5;
        let old_pan = vec2(app.ui.vfx_node_pan[0], app.ui.vfx_node_pan[1]);
        let new_pan = center - (center - old_pan) * (new_zoom / old_zoom);
        app.ui.vfx_node_pan = [new_pan.x, new_pan.y];
        app.ui.vfx_node_zoom = new_zoom;
    }
    if zoom_reset_action {
        let old_zoom = app.ui.vfx_node_zoom;
        let new_zoom = 1.0;
        let center = canvas_r.size() * 0.5;
        let old_pan = vec2(app.ui.vfx_node_pan[0], app.ui.vfx_node_pan[1]);
        let new_pan = center - (center - old_pan) * (new_zoom / old_zoom);
        app.ui.vfx_node_pan = [new_pan.x, new_pan.y];
        app.ui.vfx_node_zoom = 1.0;
    }

    // Canvas Mouse Wheel Zoom & Interactive Pan
    let canvas_id = egui::Id::unique("vfx_nodes_canvas");
    let canvas_resp = ui.interact(canvas_r, canvas_id, Sense::drag());

    if ui.rect_contains_pointer(canvas_r) {
        let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_y != 0.0 {
            let pointer_pos = ui.input(|i| i.pointer.hover_pos()).unwrap_or(canvas_r.center());
            let rel_cursor = pointer_pos - canvas_r.min;
            let old_zoom = app.ui.vfx_node_zoom;
            let factor = (scroll_y * 0.003).exp();
            let new_zoom = (old_zoom * factor).clamp(0.25, 2.5);
            if (new_zoom - old_zoom).abs() > 0.001 {
                let old_pan = vec2(app.ui.vfx_node_pan[0], app.ui.vfx_node_pan[1]);
                let new_pan = rel_cursor - (rel_cursor - old_pan) * (new_zoom / old_zoom);
                app.ui.vfx_node_pan = [new_pan.x, new_pan.y];
                app.ui.vfx_node_zoom = new_zoom;
            }
        }

        let is_node_dragged = (0..node_count).any(|i| ui.ctx().is_being_dragged(egui::Id::unique(("vfx_node_card", clip.0, i))));
        if !is_node_dragged {
            let pointer_delta = ui.input(|i| i.pointer.delta());
            let is_pan_button = ui.input(|i| {
                i.pointer.button_down(egui::PointerButton::Middle)
                    || i.pointer.button_down(egui::PointerButton::Secondary)
                    || (i.modifiers.alt && i.pointer.button_down(egui::PointerButton::Primary))
                    || (i.key_down(egui::Key::Space) && i.pointer.button_down(egui::PointerButton::Primary))
            });
            if is_pan_button && pointer_delta != egui::Vec2::ZERO {
                app.ui.vfx_node_pan[0] += pointer_delta.x;
                app.ui.vfx_node_pan[1] += pointer_delta.y;
            } else if canvas_resp.dragged_by(egui::PointerButton::Primary) && pointer_delta != egui::Vec2::ZERO {
                app.ui.vfx_node_pan[0] += pointer_delta.x;
                app.ui.vfx_node_pan[1] += pointer_delta.y;
            }
        }
    }

    if node_count == 0 {
        let painter = ui.painter_at(canvas_r).with_clip_rect(canvas_r);
        let center = canvas_r.center();
        painter.text(
            pos2(center.x, center.y - 22.0),
            Align2::CENTER_CENTER,
            tl!("No VFX nodes on active clip — double-click or drag an effect from the Effects panel"),
            Tokens::ui(12.5),
            t.text_dim,
        );
        let btn_r = Rect::from_center_size(pos2(center.x, center.y + 15.0), vec2(200.0, 30.0));
        let btn_resp = ui.interact(btn_r, egui::Id::unique("init_vfx_node_btn"), Sense::click());
        app.auto.add("vfx_nodes.init_node", btn_r, "Add Initial VFX Node");
        if btn_resp.hovered() {
            painter.rect_filled(btn_r, 4.0, t.hover);
        } else {
            painter.rect_filled(btn_r, 4.0, Color32::from_rgba_unmultiplied(120, 85, 240, 48));
        }
        painter.rect_stroke(btn_r, 4.0, Stroke::new(1.0, Color32::from_rgb(135, 100, 250)), StrokeKind::Inside);
        painter.text(btn_r.center(), Align2::CENTER_CENTER, tl!("+ Add EffectCraft Glow"), Tokens::ui(12.0), t.text);
        if btn_resp.clicked()
            && let Ok(v) = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "ec.stylize.glow"}))
            && let Some(fx_idx) = v["applied"].as_array().and_then(|a| a.first()).and_then(|e| e["index"].as_u64())
        {
            select_vfx_node(app, clip, 0, fx_idx as usize);
        }
        return;
    }

    // Base node layout parameters (unscaled)
    let base_node_w = 152.0;
    let base_node_h = 80.0;
    let base_gap = 52.0;
    let base_v_gap = 36.0;
    let base_port_w = 42.0;
    let base_port_h = 32.0;

    let nodes_per_row = if node_count <= 4 { node_count.max(1) } else { 4 };
    let total_rows = (node_count + nodes_per_row - 1) / nodes_per_row;
    let max_cols = if total_rows > 1 { nodes_per_row } else { node_count };
    let row_w = max_cols as f32 * base_node_w + (max_cols.saturating_sub(1)) as f32 * base_gap;
    let total_chain_w = base_port_w + base_gap + row_w + base_gap + base_port_w;
    let start_x = ((canvas_r.width() - total_chain_w) * 0.5).max(base_port_w + base_gap + 20.0);

    let total_h = total_rows as f32 * base_node_h + (total_rows.saturating_sub(1)) as f32 * base_v_gap;
    let start_y = ((canvas_r.height() - total_h) * 0.5).max(20.0);

    if node_count > 4 && app.ui.vfx_node_zoom == 1.0 && app.ui.vfx_node_pan == [0.0, 0.0] {
        fit_view = true;
    }

    if fit_view && node_count > 0 {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for i in 0..node_count {
            let key = format!("{}:{i}", clip.0);
            let [x, y] = app.ui.vfx_node_positions.get(&key).copied().unwrap_or_else(|| {
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

        app.ui.vfx_node_pan = [
            canvas_center.x - graph_center_x * target_zoom,
            canvas_center.y - graph_center_y * target_zoom,
        ];
        app.ui.vfx_node_zoom = target_zoom;
    }

    let zoom = app.ui.vfx_node_zoom.clamp(0.25, 2.5);
    let pan = vec2(app.ui.vfx_node_pan[0], app.ui.vfx_node_pan[1]);

    let painter = ui.painter_at(canvas_r).with_clip_rect(canvas_r);

    // Subtle Fusion/Resolve dot grid panning with canvas
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
    for i in 0..node_count {
        let key = format!("{}:{i}", clip.0);
        let r = i / nodes_per_row;
        let c = i % nodes_per_row;
        let default_rel = [
            start_x + c as f32 * (base_node_w + base_gap),
            start_y + r as f32 * (base_node_h + base_v_gap),
        ];
        default_rels.push(default_rel);
        let rel_pos = app.ui.vfx_node_positions.get(&key).copied().unwrap_or(default_rel);
        let screen_min = canvas_r.min + pan + vec2(rel_pos[0] * zoom, rel_pos[1] * zoom);
        let screen_size = vec2(base_node_w * zoom, base_node_h * zoom);
        node_rects.push(Rect::from_min_size(screen_min, screen_size));
    }

    // Terminals
    let port_w = base_port_w * zoom;
    let port_h = base_port_h * zoom;
    let port_corner = (4.0 * zoom).clamp(2.0, 4.0);

    // [ IN ] Terminal
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
    painter.circle_filled(in_pin, term_pin_r, Color32::from_rgb(140, 105, 255));

    // [ OUT ] Terminal
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
    painter.circle_filled(out_pin, term_pin_r, Color32::from_rgb(140, 105, 255));

    // Connecting wires
    let wire_stroke_w = (2.0 * zoom).clamp(1.0, 3.0);
    let n0_in_pin = pos2(node_rects[0].min.x, node_rects[0].center().y);
    let n0_enabled = vfx_effects[0].1.enabled;
    let w0_col = if n0_enabled { Color32::from_rgb(140, 105, 255) } else { Color32::from_rgb(80, 85, 95) };
    draw_bezier_wire(&painter, in_pin, n0_in_pin, Stroke::new(wire_stroke_w, w0_col));

    for i in 0..(node_count - 1) {
        let p_out = pos2(node_rects[i].max.x, node_rects[i].center().y);
        let p_in = pos2(node_rects[i + 1].min.x, node_rects[i + 1].center().y);
        let enabled = vfx_effects[i + 1].1.enabled;
        let col = if enabled { Color32::from_rgb(140, 105, 255) } else { Color32::from_rgb(80, 85, 95) };
        draw_bezier_wire(&painter, p_out, p_in, Stroke::new(wire_stroke_w, col));
    }

    let last_out_pin = pos2(node_rects[last_idx].max.x, node_rects[last_idx].center().y);
    let last_enabled = vfx_effects.last().map(|(_, e, _)| e.enabled).unwrap_or(true);
    let out_wire_color = if last_enabled { Color32::from_rgb(140, 105, 255) } else { Color32::from_rgb(80, 85, 95) };
    draw_bezier_wire(&painter, last_out_pin, out_pin, Stroke::new(wire_stroke_w, out_wire_color));

    // Render Each VFX Node Card
    for (i, &(fx_idx, fx_inst, fx_def)) in vfx_effects.iter().enumerate() {
        let node_rect = node_rects[i];
        let is_active = i == active_node;
        let is_enabled = fx_inst.enabled;

        let card_id = egui::Id::unique(("vfx_node_card", clip.0, i));
        let card_resp = ui.interact(node_rect, card_id, Sense::click_and_drag());
        app.auto.add(&format!("vfx_nodes.node.{i}"), node_rect, fx_def.name);

        if card_resp.dragged() {
            let delta = card_resp.drag_delta();
            let key = format!("{}:{i}", clip.0);
            let pos = app.ui.vfx_node_positions.entry(key).or_insert(default_rels[i]);
            pos[0] += delta.x / zoom;
            pos[1] += delta.y / zoom;
        }

        if card_resp.clicked() {
            select_vfx_node(app, clip, i, fx_idx);
        }

        // Pin dots
        let card_pin_r = (3.5 * zoom).clamp(2.0, 5.0);
        let pin_col = if is_enabled { Color32::from_rgb(140, 105, 255) } else { Color32::from_rgb(90, 95, 105) };
        painter.circle_filled(pos2(node_rect.min.x, node_rect.center().y), card_pin_r, pin_col);
        painter.circle_filled(pos2(node_rect.max.x, node_rect.center().y), card_pin_r, pin_col);

        // Card background
        let bg_color = if is_enabled {
            if is_active {
                Color32::from_rgb(34, 35, 45)
            } else {
                Color32::from_rgb(28, 30, 36)
            }
        } else {
            Color32::from_rgb(22, 23, 27)
        };
        let card_corner = (6.0 * zoom).clamp(3.0, 6.0);
        painter.rect_filled(node_rect, card_corner, bg_color);

        // Header bar
        let header_h = (22.0 * zoom).clamp(14.0, 22.0);
        let header_rect = Rect::from_min_size(node_rect.min, vec2(node_rect.width(), header_h));
        let header_bg = if is_active {
            Color32::from_rgb(44, 42, 62)
        } else {
            Color32::from_rgb(22, 24, 28)
        };
        painter.rect_filled(header_rect, card_corner, header_bg);
        painter.rect_filled(
            Rect::from_min_max(pos2(header_rect.min.x, header_rect.max.y - 3.0 * zoom), header_rect.max),
            0.0,
            header_bg,
        );

        // Node index badge ("01", "02")
        let num_str = format!("{:02}", i + 1);
        let num_font_size = (11.0 * zoom).clamp(8.0, 11.0);
        painter.text(
            pos2(header_rect.min.x + (8.0 * zoom).max(4.0), header_rect.center().y),
            Align2::LEFT_CENTER,
            num_str,
            Tokens::ui(num_font_size),
            if is_active { Color32::WHITE } else { Color32::from_rgb(180, 185, 195) },
        );

        // Power bypass toggle LED
        let power_box_size = (16.0 * zoom).clamp(10.0, 16.0);
        let power_center = pos2(header_rect.max.x - (12.0 * zoom).max(8.0), header_rect.center().y);
        let power_r = Rect::from_center_size(power_center, vec2(power_box_size, power_box_size));
        let power_resp = ui.interact(power_r, egui::Id::unique(("vfx_node_power", clip.0, i)), Sense::click());
        app.auto.add(&format!("vfx_nodes.node.{i}.power"), power_r, "Toggle VFX Node Bypass");

        let led_color = if is_enabled {
            Color32::from_rgb(70, 210, 90)
        } else {
            Color32::from_rgb(150, 45, 45)
        };
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
            let _ = app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": fx_idx}));
        }

        // Effect Name Label
        if zoom >= 0.55 {
            let raw_name = crate::i18n::t(fx_def.name);
            let display_label = if raw_name.chars().count() > 18 {
                let s: String = raw_name.chars().take(16).collect();
                format!("{s}…")
            } else {
                raw_name.to_string()
            };
            let label_pos = pos2(node_rect.min.x + (8.0 * zoom).max(4.0), header_rect.max.y + (5.0 * zoom).max(2.0));
            painter.text(
                label_pos,
                Align2::LEFT_TOP,
                display_label,
                Tokens::ui((11.0 * zoom).clamp(8.0, 11.0)),
                if is_enabled { t.text } else { t.text_faint },
            );
        }

        // Badges
        if zoom >= 0.65 {
            let badges = detect_vfx_badges(fx_inst, fx_def);
            let badge_w = (28.0 * zoom).clamp(18.0, 28.0);
            let badge_h = (13.0 * zoom).clamp(9.0, 13.0);
            let mut bx = node_rect.min.x + (8.0 * zoom).max(4.0);
            let by = node_rect.max.y - (18.0 * zoom).max(12.0);

            for (badge_text, badge_tip) in badges {
                let br = Rect::from_min_size(pos2(bx, by), vec2(badge_w, badge_h));
                let b_bg = if is_enabled {
                    Color32::from_rgb(20, 22, 28)
                } else {
                    Color32::from_rgb(18, 19, 22)
                };
                painter.rect_filled(br, 2.0, b_bg);
                painter.rect_stroke(
                    br,
                    2.0,
                    Stroke::new(1.0, if is_active { Color32::from_rgb(80, 75, 110) } else { Color32::from_rgb(45, 48, 56) }),
                    StrokeKind::Inside,
                );

                let txt_col = match badge_text {
                    "EC" => Color32::from_rgb(175, 130, 255),
                    "GPU" => Color32::from_rgb(240, 190, 60),
                    "32f" => Color32::from_rgb(80, 200, 240),
                    "KF" => Color32::from_rgb(90, 220, 140),
                    "MSK" => Color32::from_rgb(240, 120, 160),
                    "STYL" | "GEN" => Color32::from_rgb(235, 160, 70),
                    "BLUR" | "DIST" => Color32::from_rgb(100, 185, 245),
                    _ => Color32::from_rgb(155, 165, 185),
                };
                painter.text(br.center(), Align2::CENTER_CENTER, badge_text, Tokens::ui((8.5 * zoom).clamp(7.0, 8.5)), txt_col);

                let badge_resp = ui.interact(br, egui::Id::unique(("vfx_badge", clip.0, i, badge_text)), Sense::hover());
                badge_resp.on_hover_text(badge_tip);

                bx += badge_w + 4.0 * zoom;
                if bx + badge_w > node_rect.max.x - 6.0 * zoom {
                    break;
                }
            }
        }

        // Card border stroke (Resolve gold/violet selection)
        let stroke = if is_active {
            Stroke::new((2.0 * zoom).clamp(1.5, 2.5), Color32::from_rgb(230, 175, 45))
        } else if card_resp.hovered() {
            Stroke::new(1.0, Color32::from_rgb(110, 100, 150))
        } else {
            Stroke::new(1.0, Color32::from_rgb(48, 52, 62))
        };
        painter.rect_stroke(node_rect, card_corner, stroke, StrokeKind::Inside);

        // Context menu on right click
        card_resp.context_menu(|ui| {
            let toggle_label = if is_enabled {
                tl!("Bypass Node (Ctrl+D)")
            } else {
                tl!("Enable Node (Ctrl+D)")
            };
            if ui.button(toggle_label).clicked() {
                let _ = app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": fx_idx}));
                ui.close();
            }
            if ui.button(tl!("Reset Node")).clicked() {
                let _ = app.session.execute("effects.reset", json!({"clip": clip.0, "index": fx_idx}));
                ui.close();
            }
            if ui.button(tl!("Save Preset…")).clicked() {
                crate::panels::presets::open_save(app, clip.0, vec![fx_idx], fx_def.name);
                ui.close();
            }
            if ui.button(tl!("Delete Node")).clicked() {
                let _ = app.session.execute("effects.remove", json!({"clip": clip.0, "index": fx_idx}));
                if active_node >= node_count - 1 {
                    app.ui.active_vfx_node = active_node.saturating_sub(1);
                }
                ui.close();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_vfx_presets_resolve_in_effect_catalogue() {
        for &(id, _label, _cat) in QUICK_VFX_PRESETS {
            assert!(
                filmcraft_project::find_effect(id).is_some(),
                "QUICK_VFX_PRESETS entry {id} not found in filmcraft_project::find_effect"
            );
        }
    }
}
