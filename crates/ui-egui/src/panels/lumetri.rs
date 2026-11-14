//! Color Grading panel: Basic Correction, Creative, Curves (RGB + hue curves), Color Wheels,
//! HSL Secondary, Vignette — editing the selected clip's Color Grading effect through engine commands.
//!
//! In an HDR sequence (Rec. 2100 PQ / HLG working space) Basic Correction shows HDR White and
//! HDR Specular and Curves shows HDR Range (cd/m²): the sliders and curves then span 0 … that
//! many nits (see `filmcraft_color::grade`). Automation ids for those and HSL Secondary ▸ Refine:
//! `lumetri.param.<id>` (`hdr_white`, `hdr_specular`, `curves_hdr_range`, `hsl_denoise`,
//! `hsl_blur`); Color Match: `lumetri.match.comparisonView` (toggles the Program monitor's
//! Comparison View, whose reference frame Apply Match then uses).

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{ClipId, EffectInstance, ParamValue};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
#[allow(unused_imports)]
use crate::icons::{self, Icon};
use crate::native_dialogs::FileDialog;
use crate::theme::Tokens;

type Actions = Vec<Value>;

pub fn active_color_clip(app: &FilmcraftApp) -> Option<(ClipId, filmcraft_project::TrackItem)> {
    let seq = app.session.active_sequence()?;
    if let Some(res) = app.session.state.selection.iter().find_map(|c| {
        let (tid, it) = seq.find_item(*c)?;
        (seq.track(tid)?.kind == filmcraft_project::TrackKind::Video).then(|| (*c, it.clone()))
    }) {
        return Some(res);
    }
    let ph = app.session.playhead();
    for track in seq.video_tracks.iter().rev() {
        if !track.locked && !track.muted {
            for item in &track.items {
                if item.start <= ph && ph < item.end() {
                    return Some((item.id, item.clone()));
                }
            }
        }
    }
    None
}

fn selected(app: &FilmcraftApp) -> Option<(ClipId, filmcraft_project::TrackItem)> {
    let seq = app.session.active_sequence()?;
    app.session.state.selection.iter().find_map(|c| {
        let (tid, it) = seq.find_item(*c)?;
        (seq.track(tid)?.kind == filmcraft_project::TrackKind::Video).then(|| (*c, it.clone()))
    })
}

#[derive(Clone, Copy)]
pub struct LumetriToolDef {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub tooltip: &'static str,
    pub param: &'static str,
    pub sec: &'static str,
}

pub const ALL_TOOLS: [LumetriToolDef; 7] = [
    LumetriToolDef {
        id: "Basic Correction",
        icon: "⬡",
        label: "Primaries",
        tooltip: "Color wheels (Lift, Gamma, Gain, Offset) and primary exposure/color",
        param: "basic_on",
        sec: "basic",
    },
    LumetriToolDef {
        id: "Color Wheels & Match",
        icon: "◎",
        label: "Wheels & Match",
        tooltip: "Color wheels and automatic Shot Match",
        param: "wheels_on",
        sec: "wheels",
    },
    LumetriToolDef {
        id: "Curves",
        icon: "∿",
        label: "Curves",
        tooltip: "Custom RGB and Hue/Sat/Lum curves",
        param: "curves_on",
        sec: "curves",
    },
    LumetriToolDef {
        id: "Power Windows / Máscara",
        icon: "⬚",
        label: "Power Windows",
        tooltip: "Geometric and Bezier shape masks",
        param: "masks_on",
        sec: "masks",
    },
    LumetriToolDef {
        id: "HSL Secondary",
        icon: "✑",
        label: "Qualifier",
        tooltip: "HSL secondary color isolator",
        param: "hsl_on",
        sec: "hsl",
    },
    LumetriToolDef {
        id: "Creative",
        icon: "🎞",
        label: "Creative & LUTs",
        tooltip: "Film looks, LUTs and intensity",
        param: "creative_on",
        sec: "creative",
    },
    LumetriToolDef {
        id: "Vignette",
        icon: "◐",
        label: "Vignette",
        tooltip: "Edge vignette adjustments",
        param: "vignette_on",
        sec: "vignette",
    },
];

impl LumetriToolDef {
    pub fn panel_kind(&self) -> crate::dock::PanelKind {
        match self.id {
            "Basic Correction" => crate::dock::PanelKind::ColorPrimaries,
            "Color Wheels & Match" => crate::dock::PanelKind::ColorWheels,
            "Curves" => crate::dock::PanelKind::ColorCurves,
            "Power Windows / Máscara" => crate::dock::PanelKind::ColorMasks,
            "HSL Secondary" => crate::dock::PanelKind::ColorQualifier,
            "Creative" => crate::dock::PanelKind::ColorCreative,
            "Vignette" => crate::dock::PanelKind::ColorVignette,
            _ => crate::dock::PanelKind::LumetriColor,
        }
    }
}

pub fn tool_for_panel(panel: crate::dock::PanelKind) -> Option<&'static str> {
    match panel {
        crate::dock::PanelKind::ColorPrimaries => Some("Basic Correction"),
        crate::dock::PanelKind::ColorWheels => Some("Color Wheels & Match"),
        crate::dock::PanelKind::ColorCurves => Some("Curves"),
        crate::dock::PanelKind::ColorMasks => Some("Power Windows / Máscara"),
        crate::dock::PanelKind::ColorQualifier => Some("HSL Secondary"),
        crate::dock::PanelKind::ColorCreative => Some("Creative"),
        crate::dock::PanelKind::ColorVignette => Some("Vignette"),
        _ => None,
    }
}

pub fn draw_tool_icon(painter: &egui::Painter, center: Pos2, tool_id: &str, color: Color32) {
    match tool_id {
        "Basic Correction" => {
            // Three mini fader sliders with knobs
            for col in 0..3 {
                let x = center.x + (col as f32 - 1.0) * 4.5;
                painter.line_segment([pos2(x, center.y - 5.5), pos2(x, center.y + 5.5)], Stroke::new(1.0, color.gamma_multiply(0.5)));
                let pip_y = center.y + match col { 0 => -1.5, 1 => 2.0, _ => -0.5 };
                painter.circle_filled(pos2(x, pip_y), 1.6, color);
            }
        }
        "Color Wheels & Match" => {
            // Color wheel circle with central reticle
            painter.circle_stroke(center, 5.5, Stroke::new(1.2, color));
            painter.circle_filled(center, 1.8, color);
        }
        "Curves" => {
            // Smooth tone curve
            let p0 = pos2(center.x - 5.5, center.y + 4.5);
            let p1 = pos2(center.x - 2.0, center.y + 2.5);
            let p2 = pos2(center.x + 2.0, center.y - 2.5);
            let p3 = pos2(center.x + 5.5, center.y - 4.5);
            painter.line_segment([p0, p1], Stroke::new(1.3, color));
            painter.line_segment([p1, p2], Stroke::new(1.3, color));
            painter.line_segment([p2, p3], Stroke::new(1.3, color));
        }
        "Power Windows / Máscara" => {
            // Mask / Power Window rectangle
            let m_rect = Rect::from_center_size(center, vec2(11.0, 10.0));
            painter.rect_stroke(m_rect, 2.0, Stroke::new(1.2, color), StrokeKind::Inside);
        }
        "HSL Secondary" => {
            // Eyedropper / Qualifier tool
            let tip = pos2(center.x - 4.5, center.y + 4.5);
            let top = pos2(center.x + 4.5, center.y - 4.5);
            painter.line_segment([tip, top], Stroke::new(1.5, color));
            painter.circle_filled(tip, 1.3, color);
            painter.circle_filled(top, 2.0, color);
        }
        "Creative" => {
            // Film frame with sprockets
            let f_rect = Rect::from_center_size(center, vec2(12.0, 10.0));
            painter.rect_stroke(f_rect, 1.5, Stroke::new(1.0, color), StrokeKind::Inside);
            painter.line_segment([pos2(f_rect.min.x + 3.0, f_rect.min.y), pos2(f_rect.min.x + 3.0, f_rect.max.y)], Stroke::new(0.8, color.gamma_multiply(0.5)));
            painter.line_segment([pos2(f_rect.max.x - 3.0, f_rect.min.y), pos2(f_rect.max.x - 3.0, f_rect.max.y)], Stroke::new(0.8, color.gamma_multiply(0.5)));
        }
        "Vignette" => {
            // Vignette aperture lens
            painter.circle_stroke(center, 5.5, Stroke::new(1.0, color.gamma_multiply(0.6)));
            painter.circle_filled(center, 3.2, color);
        }
        _ => {
            painter.circle_filled(center, 3.0, color);
        }
    }
}

pub fn get_tool_def(id: &str) -> Option<LumetriToolDef> {
    ALL_TOOLS.iter().find(|t| t.id == id).copied()
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, current_panel: crate::dock::PanelKind) {
    let t = app.tokens;

    // Resolve active Color Group if one is selected and still exists in the sequence
    let active_group_info = app.ui.active_color_group.and_then(|gid| {
        let seq = app.session.active_sequence()?;
        let g = seq.color_group(gid)?;
        Some((gid, g.name.clone(), g.color, g.effects.clone()))
    });
    if app.ui.active_color_group.is_some() && active_group_info.is_none() {
        app.ui.active_color_group = None;
    }

    let clip_info = active_color_clip(app);
    let clip_group = clip_info.as_ref().and_then(|(cid, _)| {
        app.session
            .active_sequence()
            .and_then(|s| s.color_group_of(*cid))
            .map(|g| (g.id, g.name.clone(), g.color))
    });

    if active_group_info.is_none() && clip_info.is_none() {
        crate::dock::placeholder(ui, rect, &t, tl!("(no clip selected)"));
        return;
    }

    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 4.0))).id_salt(("lumetri2", current_panel)));
    bui.set_clip_rect(rect.intersect(ui.clip_rect()));

    let (clip, it_name, mt, idx, e, is_group_mode, group_hdr, lumetri_effects_len, node_list_meta, needs_apply_clip) =
        if let Some((gid, gname, gcol, geffects)) = active_group_info {
            let inst = geffects
                .iter()
                .find(|fx| fx.effect == "lumetri")
                .cloned()
                .or_else(|| filmcraft_project::find_effect("lumetri").map(|d| d.instance()));
            let Some(inst) = inst else {
                crate::dock::placeholder(ui, rect, &t, tl!("(color grading effect unavailable)"));
                return;
            };
            let dummy_clip = clip_info.as_ref().map(|(c, _)| *c).unwrap_or(ClipId(0));
            (dummy_clip, gname.clone(), Tick(0), 0usize, inst, true, Some((gid, gname, gcol)), 1usize, vec![(0usize, true)], false)
        } else {
            let Some((clip, it)) = clip_info else {
                return;
            };
            let lumetri_effects: Vec<(usize, &EffectInstance)> = it.effects.iter().enumerate().filter(|(_, e)| e.effect == "lumetri").collect();
            let ph = app.session.playhead();
            let mt = it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
            if lumetri_effects.is_empty() {
                let Some(default_inst) = filmcraft_project::find_effect("lumetri").map(|d| d.instance()) else {
                    crate::dock::placeholder(ui, rect, &t, tl!("(color grading effect unavailable)"));
                    return;
                };
                app.ui.active_color_node = 0;
                (clip, it.name, mt, 0usize, default_inst, false, None, 1usize, vec![(0usize, true)], true)
            } else {
                let active_node = app.ui.active_color_node.min(lumetri_effects.len().saturating_sub(1));
                app.ui.active_color_node = active_node;
                let (idx, e_ref) = lumetri_effects[active_node];
                let meta = lumetri_effects.iter().map(|&(i, fx)| (i, fx.enabled)).collect();
                let len = lumetri_effects.len();
                let cloned = e_ref.clone();
                (clip, it.name, mt, idx, cloned, false, None, len, meta, false)
            }
        };
    let active_node = if is_group_mode { 0 } else { app.ui.active_color_node };

    // HDR mode: the sequence's working space is Rec. 2100 PQ or HLG
    let hdr = app.session.active_sequence().is_some_and(|q| q.settings.color.working.is_hdr());
    let mut auto_rows: Vec<(String, Rect, &'static str)> = Vec::new();
    let mut actions: Actions = Vec::new();
    // header: clip/group context + Clip|Group toggle + reset + fx toggle
    bui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let fx = ui
                .add(egui::Button::new(egui::RichText::new("fx").italics().color(if e.enabled { t.accent } else { t.text_faint })).frame(false))
                .on_hover_text(tl!("Bypass Color Grading (on/off)"));
            if fx.clicked() {
                if let Some((gid, _, _)) = &group_hdr {
                    let _ = app.session.execute("color.groupToggleBypass", json!({"group": gid.0}));
                } else if !needs_apply_clip {
                    let _ = app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": idx}));
                }
            }
            let rst = paint_reset_icon_button(ui, t.text_dim, tl!("Reset all color adjustments"));
            if rst.clicked() {
                if let Some((gid, _, _)) = &group_hdr {
                    let _ = app.session.execute("color.groupReset", json!({"group": gid.0}));
                } else if !needs_apply_clip {
                    let _ = app.session.execute("effects.reset", json!({"clip": clip.0, "index": idx}));
                }
            }
            if let Some((gid, gname, gcol)) = group_hdr.as_ref().or(clip_group.as_ref()) {
                let grp_col = Color32::from_rgb(gcol[0], gcol[1], gcol[2]);
                let g_btn = egui::Button::new(egui::RichText::new(tlf!("Group: {name}", name = gname)).small().color(if is_group_mode { Color32::BLACK } else { grp_col }))
                    .fill(if is_group_mode { grp_col } else { Color32::TRANSPARENT })
                    .stroke(egui::Stroke::new(1.0, grp_col));
                if ui.add(g_btn).clicked() {
                    app.ui.active_color_group = Some(*gid);
                    app.ui.active_color_node = 0;
                }
                let c_btn = egui::Button::new(egui::RichText::new(tl!("Clip")).small().color(if !is_group_mode { Color32::WHITE } else { t.text_dim }))
                    .fill(if !is_group_mode { t.accent.gamma_multiply(0.35) } else { Color32::TRANSPARENT });
                if ui.add(c_btn).clicked() {
                    app.ui.active_color_group = None;
                    app.ui.active_color_node = 0;
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let hdr_lbl = if let Some((_, gname, _)) = &group_hdr {
                    tlf!("Group · {name}", name = gname)
                } else {
                    tlf!("Clip · {name}", name = it_name)
                };
                ui.add(egui::Label::new(egui::RichText::new(hdr_lbl).strong().color(t.text_dim)).truncate());
            });
        });
    });

    // DaVinci Resolve-style Node switcher bar (hidden or single-node in Group mode)
    if !is_group_mode {
        bui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Node:")).color(t.text_dim).small());
            let add_btn = ui.button(egui::RichText::new("+").small()).on_hover_text(tl!("Add Serial Node (Alt+S)"));
            app.auto.add("lumetri.node.add", add_btn.rect, "Add Serial Node");
            if add_btn.clicked() {
                let _ = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}));
                app.ui.active_color_node = lumetri_effects_len;
            }
            let nodes_max_w = ui.available_width().max(40.0);
            egui::ScrollArea::horizontal()
                .auto_shrink([false, true])
                .max_width(nodes_max_w)
                .max_height(26.0)
                .id_salt(("lumetri_nodes_scroll", current_panel))
                .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (n_idx, &(_fx_i, enabled)) in node_list_meta.iter().enumerate() {
                        let is_curr = n_idx == active_node;
                        let node_num = format!("{:02}", n_idx + 1);
                        let custom_name = app.ui.color_node_labels.get(&format!("{}:{n_idx}", clip.0)).cloned();
                        let label = custom_name.unwrap_or_else(|| format!("Node {node_num}"));
                        let text = if enabled {
                            format!("{label} ✓")
                        } else {
                            format!("{label} ✗")
                        };
                        let mut btn = egui::Button::new(egui::RichText::new(text).small());
                        if is_curr {
                            btn = btn.fill(t.accent.gamma_multiply(0.25)).stroke(egui::Stroke::new(1.0, t.accent));
                        }
                        let resp = ui.add(btn);
                        app.auto.add(&format!("lumetri.node.{n_idx}"), resp.rect, &label);
                        if resp.clicked() {
                            app.ui.active_color_node = n_idx;
                        }
                    }
                });
            });
        });
    }

    let mut sections: Vec<&'static str> = Vec::new();
    let mut lut_actions: Vec<(&'static str, Value)> = Vec::new();
    let mut mask_actions: Vec<(String, Value)> = Vec::new();

    let bar_pos = app.ui.lumetri_bar_pos.clone();
    let panel_w = bui.available_width().max(120.0);
    bui.set_max_width(panel_w);
    match bar_pos.as_str() {
        "bottom" => {
            let content_h = (bui.available_height() - 36.0).max(60.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .hscroll(false)
                .max_width(panel_w)
                .max_height(content_h)
                .id_salt(("lumetri_scroll", current_panel))
                .show(&mut bui, |ui| {
                    let inner_w = ui.available_width().min(panel_w).max(100.0);
                    ui.set_max_width(inner_w);
                    render_palette_content(ui, app, &e, current_panel, clip, idx, mt, hdr, &mut actions, &mut lut_actions, &mut mask_actions, &mut auto_rows, &mut sections);
                    ui.add_space(12.0);
                });
            bui.add_space(4.0);
            render_palette_toolbar(&mut bui, app, &e, current_panel, false, &mut sections);
        }
        "left" => {
            bui.horizontal(|ui| {
                let side_w = 160.0_f32.min((panel_w * 0.35).max(110.0));
                ui.allocate_ui_with_layout(vec2(side_w, ui.available_height()), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(side_w);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).hscroll(false).max_width(side_w).id_salt(("lumetri_side_scroll", current_panel)).show(ui, |ui| {
                        render_palette_toolbar(ui, app, &e, current_panel, true, &mut sections);
                    });
                });
                ui.separator();
                let main_w = (ui.available_width()).max(120.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .hscroll(false)
                    .max_width(main_w)
                    .id_salt(("lumetri_scroll", current_panel))
                    .show(ui, |ui| {
                        let inner_w = ui.available_width().min(main_w).max(100.0);
                        ui.set_max_width(inner_w);
                        render_palette_content(ui, app, &e, current_panel, clip, idx, mt, hdr, &mut actions, &mut lut_actions, &mut mask_actions, &mut auto_rows, &mut sections);
                        ui.add_space(12.0);
                    });
            });
        }
        _ => {
            // default: "top"
            render_palette_toolbar(&mut bui, app, &e, current_panel, false, &mut sections);
            bui.add_space(4.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .hscroll(false)
                .max_width(panel_w)
                .id_salt(("lumetri_scroll", current_panel))
                .show(&mut bui, |ui| {
                    let inner_w = ui.available_width().min(panel_w).max(100.0);
                    ui.set_max_width(inner_w);
                    render_palette_content(ui, app, &e, current_panel, clip, idx, mt, hdr, &mut actions, &mut lut_actions, &mut mask_actions, &mut auto_rows, &mut sections);
                    ui.add_space(12.0);
                });
        }
    }
    for (id, r, label) in auto_rows {
        if r.is_positive() {
            app.auto.add(&id, r, label);
        }
    }
    let has_clip_edit = !actions.is_empty()
        || !sections.is_empty()
        || !mask_actions.is_empty()
        || lut_actions.iter().any(|(cmd, p)| *cmd != "view.display" && p.get("browse").is_none());

    if group_hdr.is_none()
        && needs_apply_clip
        && has_clip_edit
        && let Ok(applied) = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}))
    {
        if let Some(new_idx) = applied
            .get("applied")
            .and_then(|a| a.get(0))
            .and_then(|o| o.get("index"))
            .and_then(|v| v.as_u64())
        {
            for a in &mut actions {
                a["effect"] = json!(new_idx);
            }
            for (_, val) in &mut mask_actions {
                val["effect"] = json!(new_idx);
            }
        }
        app.ui.active_color_node = 0;
    }

    // a dragged value is one undo step, as in Effect Controls
    if let Some((gid, _, _)) = group_hdr {
        let mapped = actions
            .into_iter()
            .map(|a| {
                (
                    "color.groupSetParam".to_string(),
                    json!({
                        "group": gid.0,
                        "param": a["param"],
                        "value": a["value"],
                        "merge": a.get("merge").and_then(|v| v.as_bool()).unwrap_or(false),
                    }),
                )
            })
            .collect();
        crate::panels::effect_controls::run(app, ui.ctx(), mapped);
    } else {
        crate::panels::effect_controls::run(app, ui.ctx(), actions.into_iter().map(|a| ("effects.setParam".to_string(), a)).collect());
        for sec in sections {
            if let Err(err) = app.session.execute("lumetri.setSection", json!({"clip": clip.0, "section": sec})) {
                app.ui.status = err.to_string();
            }
        }
    }
    for (cmd, mut p) in lut_actions {
        if cmd == "view.display" {
            // Comparison View on / off (a Program monitor display mode)
            let d = p["display"].as_str().unwrap_or("composite").to_string();
            if let Some(Err(err)) = crate::panels::monitor_view::route(app, &format!("view.display.{d}"), &json!({"monitor": "program"})) {
                app.ui.status = err;
            }
            continue;
        }
        if p.get("browse").is_some() {
            let (cmd, clip) = (cmd.to_string(), clip.0);
            app.pick_ui(FileDialog::open_file(tl!("LUT"), &["cube", "3dl"]), move |app, paths| {
                let Some(path) = paths.into_iter().next() else { return };
                if let Err(err) = app.session.execute(&cmd, json!({"path": path, "clip": clip})) {
                    app.ui.status = err.to_string();
                }
            });
            continue;
        }
        p["clip"] = json!(clip.0);
        if let Err(err) = app.session.execute(cmd, p) {
            app.ui.status = err.to_string();
        }
    }
    for (cmd, val) in mask_actions {
        if let Err(err) = app.session.execute(&cmd, val) {
            app.ui.status = err.to_string();
        }
    }
}

fn render_tool_buttons(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    e: &EffectInstance,
    current_panel: crate::dock::PanelKind,
    vertical: bool,
    sections: &mut Vec<&'static str>,
) {
    let t = app.tokens;
    let num_tools = app.ui.lumetri_tool_order.len();
    let current_tool_id = tool_for_panel(current_panel)
        .map(String::from)
        .unwrap_or_else(|| app.ui.lumetri_active_tool.clone());

    for i in 0..num_tools {
        let tool_id = app.ui.lumetri_tool_order[i].clone();
        let Some(def) = get_tool_def(&tool_id) else { continue };

        let is_primary = def.id == current_tool_id;
        let is_secondary = current_panel == crate::dock::PanelKind::LumetriColor
            && app.ui.lumetri_dual_view
            && app.ui.lumetri_secondary_tool == def.id
            && def.id != current_tool_id;
        let is_docked_elsewhere = def.panel_kind() != current_panel
            && (app.ui.dock.contains(&def.panel_kind()) || app.ui.dock.floating.iter().any(|g| g.contains(&def.panel_kind())));
        let is_on = e.param(def.param).and_then(|p| p.value.as_bool()).unwrap_or(def.param != "hsl_on");

        let label_text = crate::i18n::t(def.label);
        let btn_w = if vertical {
            148.0
        } else {
            (label_text.chars().count() as f32 * 6.4 + 48.0).clamp(84.0, 140.0)
        };
        let btn_size = vec2(btn_w, 26.0);
        let (r, resp) = ui.allocate_exact_size(btn_size, Sense::click_and_drag());

        // Drag to dock / float window anywhere across screen
        if resp.drag_started() {
            let target_panel = def.panel_kind();
            app.begin_panel_drag(ui.ctx(), target_panel, current_panel);
        }

        // Draw button background & border
        let bg_color = if is_primary {
            t.accent.gamma_multiply(0.24)
        } else if is_secondary {
            t.accent.gamma_multiply(0.12)
        } else if resp.hovered() {
            Color32::from_rgb(38, 42, 52)
        } else {
            Color32::from_rgb(26, 29, 36)
        };

        let border_stroke = if is_primary {
            Stroke::new(1.5, t.accent)
        } else if is_secondary {
            Stroke::new(1.0, Color32::from_rgb(160, 175, 200))
        } else if is_docked_elsewhere {
            Stroke::new(1.0, Color32::from_rgb(60, 110, 160))
        } else if resp.hovered() {
            Stroke::new(1.0, Color32::from_rgb(70, 75, 90))
        } else {
            Stroke::new(1.0, Color32::from_rgb(42, 46, 56))
        };

        ui.painter().rect(r, 4.0, bg_color, border_stroke, StrokeKind::Inside);

        // 1. Clean resolution-independent 6-dot drag grip handle (no missing glyph □□)
        let dot_col = if resp.hovered() || resp.dragged() {
            Color32::from_rgb(180, 190, 210)
        } else {
            Color32::from_rgb(75, 80, 95)
        };
        let gx = r.min.x + 7.0;
        let gy = r.center().y;
        for col in 0..2 {
            for row in 0..3 {
                let px = gx + col as f32 * 3.5;
                let py = gy + (row as f32 - 1.0) * 3.5;
                ui.painter().circle_filled(pos2(px, py), 1.0, dot_col);
            }
        }

        // 2. Crisp vector icon drawn cleanly (no missing glyph □)
        let text_color = if is_primary {
            Color32::WHITE
        } else if resp.hovered() {
            t.text
        } else {
            t.text_dim
        };
        draw_tool_icon(ui.painter(), pos2(r.min.x + 22.0, r.center().y), def.id, text_color);

        // 3. Translated tool label
        let label_text = crate::i18n::t(def.label);
        ui.painter().text(
            pos2(r.min.x + 32.0, r.center().y),
            Align2::LEFT_CENTER,
            label_text,
            FontId::proportional(11.0),
            text_color,
        );

        // 4. Switch / Bypass indicator dot on the right of the button
        let sw_center = pos2(r.max.x - 11.0, r.center().y);
        let sw_rect = Rect::from_center_size(sw_center, vec2(14.0, 14.0));
        let sw_resp = ui.interact(sw_rect.expand(2.0), ui.scope_id().with("sw_btn").with(i), Sense::click());

        let dot_color = if is_on {
            Color32::from_rgb(52, 211, 153)
        } else {
            Color32::from_rgb(65, 70, 82)
        };
        ui.painter().circle_filled(sw_center, 4.0, dot_color);
        if is_on {
            ui.painter().circle_stroke(sw_center, 5.5, Stroke::new(1.0, Color32::from_rgb(52, 211, 153).gamma_multiply(0.4)));
        }
        if sw_resp.clicked() {
            sections.push(def.sec);
        }

        // 5. Click button to switch this panel exclusively to the selected tool (drag tab to detach/open alongside)
        if resp.clicked() && !sw_resp.clicked() {
            if current_panel == crate::dock::PanelKind::LumetriColor {
                let shift = ui.ctx().input(|inp| inp.modifiers.shift);
                if app.ui.lumetri_dual_view && shift && def.id != app.ui.lumetri_active_tool {
                    app.ui.lumetri_secondary_tool = def.id.to_string();
                } else {
                    app.ui.lumetri_active_tool = def.id.to_string();
                    app.ui.expanded_fx.retain(|k| !k.starts_with("lumetri2:"));
                    app.ui.collapsed_fx.retain(|k| !k.starts_with("lumetri2:"));
                }
            } else {
                app.switch_color_window(current_panel, def.panel_kind(), def.id);
            }
        }

        // Hover tooltip explaining both click and drag
        let hover_msg = format!(
            "{}\n• Clic: Cambiar esta ventana a {}\n• Arrastrar: Ubicar / acoplar ventana en cualquier lugar de la pantalla",
            def.tooltip,
            crate::i18n::t(def.label)
        );
        resp.on_hover_text(hover_msg);

        // Automation registers
        app.auto.add(&format!("lumetri.section.{}", def.id), r, def.id);
        app.auto.add(&format!("lumetri.switch.{}", def.id), sw_rect.expand(4.0), &format!("{} on/off", def.id));
        if def.id.contains("Power Windows") {
            app.auto.add("lumetri.section.Power Windows", r, "Power Windows");
            app.auto.add("lumetri.section.Masks", r, "Masks");
            app.auto.add("lumetri.switch.Power Windows", sw_rect.expand(4.0), "Power Windows on/off");
            app.auto.add("lumetri.switch.Masks", sw_rect.expand(4.0), "Masks on/off");
        }
    }
}

fn render_palette_toolbar(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    e: &EffectInstance,
    current_panel: crate::dock::PanelKind,
    vertical: bool,
    sections: &mut Vec<&'static str>,
) {
    for def in &ALL_TOOLS {
        if !app.ui.lumetri_tool_order.iter().any(|id| id == def.id) {
            app.ui.lumetri_tool_order.push(def.id.to_string());
        }
    }
    app.ui.lumetri_tool_order.retain(|id| ALL_TOOLS.iter().any(|def| def.id == id));

    if vertical {
        ui.vertical(|ui| {
            render_tool_buttons(ui, app, e, current_panel, true, sections);
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let cur_pos = app.ui.lumetri_bar_pos.clone();
                let r_top = ui.selectable_label(cur_pos == "top", "Top").on_hover_text(tl!("Barra Arriba"));
                app.auto.add("lumetri.bar.top", r_top.rect, "Top");
                if r_top.clicked() {
                    app.ui.lumetri_bar_pos = "top".into();
                }
                let r_bot = ui.selectable_label(cur_pos == "bottom", "Bottom").on_hover_text(tl!("Barra Abajo"));
                app.auto.add("lumetri.bar.bottom", r_bot.rect, "Bottom");
                if r_bot.clicked() {
                    app.ui.lumetri_bar_pos = "bottom".into();
                }
                let r_left = ui.selectable_label(cur_pos == "left", "Left").on_hover_text(tl!("Barra Lateral Izquierda"));
                app.auto.add("lumetri.bar.left", r_left.rect, "Left");
                if r_left.clicked() {
                    app.ui.lumetri_bar_pos = "left".into();
                }
                let dual_text = if app.ui.lumetri_dual_view { "Dual ✓" } else { "Dual" };
                let r_dual = ui.selectable_label(app.ui.lumetri_dual_view, dual_text).on_hover_text(tl!("Vista dual (dos herramientas)"));
                app.auto.add("lumetri.bar.dual", r_dual.rect, "Dual");
                if r_dual.clicked() {
                    app.ui.lumetri_dual_view = !app.ui.lumetri_dual_view;
                }
            });
        });
    } else {
        let avail_w = ui.available_width().max(120.0);
        ui.allocate_ui_with_layout(vec2(avail_w, 30.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_max_width(avail_w);
            let dual_text = if app.ui.lumetri_dual_view { "Dual ✓" } else { "Dual" };
            let r_dual = ui.selectable_label(app.ui.lumetri_dual_view, dual_text).on_hover_text(tl!("Vista dual (dos herramientas)"));
            app.auto.add("lumetri.bar.dual", r_dual.rect, "Dual");
            if r_dual.clicked() {
                app.ui.lumetri_dual_view = !app.ui.lumetri_dual_view;
            }
            let mut cur_pos = app.ui.lumetri_bar_pos.clone();
            let r_left = ui.selectable_label(cur_pos == "left", "Left").on_hover_text(tl!("Barra Lateral Izquierda"));
            app.auto.add("lumetri.bar.left", r_left.rect, "Left");
            if r_left.clicked() {
                cur_pos = "left".into();
            }
            let r_bot = ui.selectable_label(cur_pos == "bottom", "Bottom").on_hover_text(tl!("Barra Abajo"));
            app.auto.add("lumetri.bar.bottom", r_bot.rect, "Bottom");
            if r_bot.clicked() {
                cur_pos = "bottom".into();
            }
            let r_top = ui.selectable_label(cur_pos == "top", "Top").on_hover_text(tl!("Barra Arriba"));
            app.auto.add("lumetri.bar.top", r_top.rect, "Top");
            if r_top.clicked() {
                cur_pos = "top".into();
            }
            app.ui.lumetri_bar_pos = cur_pos;
            ui.separator();
            let strip_w = ui.available_width().max(40.0);
            ui.allocate_ui_with_layout(vec2(strip_w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_max_width(strip_w);
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .max_width(strip_w)
                    .max_height(28.0)
                    .id_salt(("tool_strip", current_panel))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            render_tool_buttons(ui, app, e, current_panel, false, sections);
                        });
                    });
            });
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_palette_content(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    e: &EffectInstance,
    current_panel: crate::dock::PanelKind,
    clip: ClipId,
    idx: usize,
    mt: Tick,
    hdr: bool,
    actions: &mut Actions,
    lut_actions: &mut Vec<(&'static str, Value)>,
    mask_actions: &mut Vec<(String, Value)>,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
    sections: &mut Vec<&'static str>,
) {
    let active = if get_tool_def(&app.ui.lumetri_active_tool).is_some() {
        app.ui.lumetri_active_tool.clone()
    } else {
        "Basic Correction".to_string()
    };

    if let Some(forced) = tool_for_panel(current_panel) {
        render_single_palette(forced, ui, app, e, clip, idx, mt, hdr, actions, lut_actions, mask_actions, auto_rows, sections);
        return;
    }

    if app.ui.lumetri_dual_view {
        let secondary = if app.ui.lumetri_secondary_tool != active && get_tool_def(&app.ui.lumetri_secondary_tool).is_some() {
            app.ui.lumetri_secondary_tool.clone()
        } else if active == "Curves" {
            "Color Wheels & Match".to_string()
        } else {
            "Curves".to_string()
        };
        ui.columns(2, |cols| {
            let col0_w = cols[0].available_width().max(100.0);
            cols[0].set_max_width(col0_w);
            cols[0].vertical(|ui| {
                ui.set_max_width(col0_w);
                render_single_palette(&active, ui, app, e, clip, idx, mt, hdr, actions, lut_actions, mask_actions, auto_rows, sections);
            });
            let col1_w = cols[1].available_width().max(100.0);
            cols[1].set_max_width(col1_w);
            cols[1].vertical(|ui| {
                ui.set_max_width(col1_w);
                render_single_palette(&secondary, ui, app, e, clip, idx, mt, hdr, actions, lut_actions, mask_actions, auto_rows, sections);
            });
        });
    } else {
        render_single_palette(&active, ui, app, e, clip, idx, mt, hdr, actions, lut_actions, mask_actions, auto_rows, sections);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_single_palette(
    tool_id: &str,
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    e: &EffectInstance,
    clip: ClipId,
    idx: usize,
    mt: Tick,
    hdr: bool,
    actions: &mut Actions,
    lut_actions: &mut Vec<(&'static str, Value)>,
    mask_actions: &mut Vec<(String, Value)>,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
    sections: &mut Vec<&'static str>,
) {
    let t = app.tokens;
    let Some(def) = get_tool_def(tool_id) else { return };
    let set = |actions: &mut Actions, param: &str, v: Value| actions.push(json!({"clip": clip.0, "effect": idx, "param": param, "value": v}));

    // Sleek Tool Header: [ Icon Tool Name ] [ Optional inline controls ] [ Switch Bypass ]
    let on = e.param(def.param).and_then(|p| p.value.as_bool()).unwrap_or(def.param != "hsl_on");
    let primaries_mode_id = egui::Id::unique("lumetri_primaries_view_mode");
    let hdr_avail_w = ui.available_width().max(120.0);
    let inline_modes = def.id == "Basic Correction" && hdr_avail_w >= 540.0;
    let inline_lut = def.id == "Basic Correction" && hdr_avail_w >= 840.0;
    let mut mode = if def.id == "Basic Correction" {
        ui.data(|d| d.get_temp::<u8>(primaries_mode_id).unwrap_or(0))
    } else {
        0
    };
    let mut draw_modes = |ui: &mut egui::Ui| {
        let r_sl = ui.selectable_label(mode == 2, tl!("Sliders")).on_hover_text(tl!("Sliders de Color y Luz"));
        auto_rows.push(("lumetri.mode.sliders".to_string(), r_sl.rect, "Sliders"));
        if r_sl.clicked() {
            mode = 2;
        }
        let r_kn = ui.selectable_label(mode == 1, tl!("Perillas")).on_hover_text(tl!("Vista completa de perillas"));
        auto_rows.push(("lumetri.mode.knobs".to_string(), r_kn.rect, "Perillas"));
        if r_kn.clicked() {
            mode = 1;
        }
        let r_wh = ui.selectable_label(mode == 0, tl!("Color Wheels")).on_hover_text(tl!("Ruedas Lift / Gamma / Gain / Offset + Primarias (DaVinci Resolve)"));
        auto_rows.push(("lumetri.mode.wheels".to_string(), r_wh.rect, "Color Wheels"));
        if r_wh.clicked() {
            mode = 0;
        }
    };
    ui.allocate_ui_with_layout(vec2(hdr_avail_w, 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.set_max_width(hdr_avail_w);
        let (sw, sw_resp) = ui.allocate_exact_size(vec2(26.0, 14.0), Sense::click());
        ui.painter().rect_filled(sw, 7.0, if on { t.accent } else { Color32::from_rgb(0x55, 0x55, 0x55) });
        ui.painter().circle_filled(pos2(if on { sw.max.x - 7.0 } else { sw.min.x + 7.0 }, sw.center().y), 5.0, Color32::WHITE);
        if sw_resp.clicked() {
            sections.push(def.sec);
        }
        app.auto.add(&format!("lumetri.switch.{}", def.id), sw.expand(3.0), &format!("{} on/off", def.id));
        if def.id.contains("Power Windows") {
            app.auto.add("lumetri.switch.Power Windows", sw.expand(3.0), "Power Windows on/off");
            app.auto.add("lumetri.switch.Masks", sw.expand(3.0), "Masks on/off");
        }
        if inline_modes {
            draw_modes(ui);
        }
        let left_w = ui.available_width().max(40.0);
        ui.allocate_ui_with_layout(vec2(left_w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_max_width(left_w);
            let (icon_r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            draw_tool_icon(ui.painter(), icon_r.center(), def.id, t.text);
            ui.add(egui::Label::new(egui::RichText::new(crate::i18n::t(def.id)).strong().size(12.5).color(t.text)).truncate());
            if inline_lut {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(tl!("Input LUT")).color(t.text_dim).small());
                let cur = match e.param("input_lut").map(|p| &p.value) {
                    Some(ParamValue::Text(s)) => s.clone(),
                    _ => String::new(),
                };
                let lut_w = 160.0_f32.min((ui.available_width() - 8.0).max(60.0));
                let (r, pick) = lut_menu(ui, app, ("lumetri-lut", "input_lut"), &cur, true, lut_w, "lumetri.input_lut");
                match pick {
                    Some(LutPick::Lut(r)) => lut_actions.push(("lumetri.setInputLut", json!({"lut": r}))),
                    Some(LutPick::Browse) => lut_actions.push(("lumetri.setInputLut", json!({"browse": true}))),
                    None => {}
                }
                app.auto.add("lumetri.input_lut", r, tl!("Input LUT"));
            }
        });
    });
    if def.id == "Basic Correction" {
        if !inline_modes {
            ui.horizontal_wrapped(|ui| {
                draw_modes(ui);
            });
        }
        ui.data_mut(|d| d.insert_temp(primaries_mode_id, mode));
    }
    ui.add_space(1.0);

    match def.id {
        "Basic Correction" => {
            // 0 = DaVinci Resolve Color Wheels + Primary Strip (default), 1 = Knobs, 2 = Sliders/Bars
            let mode = ui.data(|d| d.get_temp::<u8>(primaries_mode_id).unwrap_or(0));
            if !inline_lut {
                lut_combo(ui, app, e, "input_lut", tl!("Input LUT"), true, lut_actions);
                ui.add_space(2.0);
            }

            match mode {
                0 => {
                    // DaVinci Resolve Primary Color Wheels (Lift, Gamma, Gain, Offset) + Primary Knobs Strip
                    resolve_wheels_grid(ui, e, mt, &t, actions, auto_rows, clip, idx);
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);
                    draw_primary_knobs_strip(ui, e, mt, true, &t, actions, auto_rows, clip, idx);
                }
                1 => {
                    sub(ui, &t, tl!("Ajustes Primarios"));
                    draw_primary_knobs_strip(ui, e, mt, true, &t, actions, auto_rows, clip, idx);
                }
                _ => {
                    ui.columns(2, |cols| {
                        cols[0].vertical(|ui| {
                            sub(ui, &t, tl!("Color"));
                            slider(ui, e, mt, "temperature", tl!("Temperature"), Some(Gradient::Temp), &t, actions, clip, idx);
                            slider(ui, e, mt, "tint", tl!("Tint"), Some(Gradient::Tint), &t, actions, clip, idx);
                            slider(ui, e, mt, "saturation", tl!("Saturation"), None, &t, actions, clip, idx);
                            slider(ui, e, mt, "vibrance", tl!("Vibrance"), None, &t, actions, clip, idx);
                            slider(ui, e, mt, "sharpen", tl!("Sharpen"), None, &t, actions, clip, idx);
                        });
                        cols[1].vertical(|ui| {
                            sub(ui, &t, tl!("Light"));
                            for (id, label) in [
                                ("exposure", tl!("Exposure")),
                                ("contrast", tl!("Contrast")),
                                ("highlights", tl!("Highlights")),
                                ("shadows", tl!("Shadows")),
                                ("whites", tl!("Whites")),
                                ("blacks", tl!("Blacks")),
                            ] {
                                slider(ui, e, mt, id, label, None, &t, actions, clip, idx);
                            }
                        });
                    });
                }
            }

            if hdr {
                sub(ui, &t, tl!("HDR"));
                for (id, label) in [("hdr_white", tl!("HDR White")), ("hdr_specular", tl!("HDR Specular"))] {
                    let r = slider(ui, e, mt, id, label, None, &t, actions, clip, idx);
                    auto_rows.push((format!("lumetri.param.{id}"), r, label));
                }
            }
        }
        "Creative" => {
            let look = match e.param("look").map(|p| &p.value) {
                Some(ParamValue::Choice(c)) => *c as usize,
                _ => 0,
            };
            let opts = filmcraft_project::find_effect("lumetri")
                .and_then(|d| d.param("look"))
                .and_then(|p| if let filmcraft_project::ParamKind::Choice(o) = p.kind { Some(o) } else { None })
                .unwrap_or(&[]);
            ui.horizontal(|ui| {
                ui.add_sized(vec2(110.0, 20.0), egui::Label::new(egui::RichText::new(tl!("Look")).color(t.text_dim)));
                let mut sel = look;
                craft_ui::ComboBox::from_id_salt("lumetri-look")
                    .selected_text(opts.get(look).map_or(tl!("None"), |o| crate::i18n::t(o)))
                    .width((ui.available_width() - 40.0).max(60.0))
                    .show_ui(ui, |ui| {
                        for (i, o) in opts.iter().enumerate() {
                            if ui.selectable_value(&mut sel, i, crate::i18n::t(o)).changed() {
                                set(actions, "look", json!(i));
                            }
                        }
                    });
            });
            lut_combo(ui, app, e, "look_lut", tl!("Look LUT"), false, lut_actions);
            slider(ui, e, mt, "look_intensity", tl!("Intensity"), None, &t, actions, clip, idx);
            sub(ui, &t, tl!("Adjustments"));
            for (id, label) in
                [("faded_film", tl!("Faded Film")), ("sharpen", tl!("Sharpen")), ("vibrance", tl!("Vibrance")), ("creative_sat", tl!("Saturation"))]
            {
                slider(ui, e, mt, id, label, None, &t, actions, clip, idx);
            }
        }
        "Curves" => {
            let tab_key = egui::Id::unique("lumetri-curve-tab");
            let mut curve_tab: usize = ui.data(|d| d.get_temp(tab_key)).unwrap_or(0);
            let curve_tabs = [
                ("Custom RGB", false, ""),
                ("Hue vs Hue", true, "hue_vs_hue"),
                ("Hue vs Sat", true, "hue_vs_sat"),
                ("Hue vs Lum", true, "hue_vs_luma"),
                ("Lum vs Sat", true, "luma_vs_sat"),
                ("Sat vs Sat", true, "sat_vs_sat"),
            ];
            let key = egui::Id::unique("lumetri-curve-channel");
            let mut ch: usize = ui.data(|d| d.get_temp(key)).unwrap_or(0);
            let (id, col) = [
                ("curve_luma", Color32::WHITE),
                ("curve_red", Color32::from_rgb(230, 70, 70)),
                ("curve_green", Color32::from_rgb(80, 200, 90)),
                ("curve_blue", Color32::from_rgb(80, 130, 250)),
            ][ch];

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (ti, (lbl, _, _)) in curve_tabs.iter().enumerate() {
                    if ui.selectable_label(curve_tab == ti, egui::RichText::new(crate::i18n::t(lbl)).small()).clicked() {
                        curve_tab = ti;
                    }
                }
                if curve_tab == 0 {
                    ui.separator();
                    for (i, (c, cname)) in [
                        (Color32::WHITE, "Y"),
                        (Color32::from_rgb(230, 70, 70), "R"),
                        (Color32::from_rgb(80, 200, 90), "G"),
                        (Color32::from_rgb(80, 130, 250), "B"),
                    ]
                    .iter()
                    .enumerate()
                    {
                        let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 5.0, *c);
                        if ch == i {
                            ui.painter().circle_stroke(r.center(), 7.0, Stroke::new(1.5, t.hot_text));
                        }
                        if resp.on_hover_text(*cname).clicked() {
                            ch = i;
                        }
                    }
                    if paint_reset_icon_button(ui, t.text_dim, tl!("Reset curve")).clicked() {
                        set(actions, id, json!([[0.0, 0.0], [1.0, 1.0]]));
                    }
                } else {
                    let (_, _, cid) = curve_tabs[curve_tab.min(curve_tabs.len() - 1)];
                    if paint_reset_icon_button(ui, t.text_dim, tl!("Reset curve")).clicked() {
                        set(actions, cid, json!([]));
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_key, curve_tab));
            ui.data_mut(|d| d.insert_temp(key, ch));

            if curve_tab == 0 {
                let r = curve_editor(ui, e, id, false, col, &t, actions, clip, idx);
                auto_rows.push((format!("lumetri.curve.{id}"), r, "Curve"));
                if hdr {
                    let r = slider(ui, e, mt, "curves_hdr_range", tl!("HDR Range"), None, &t, actions, clip, idx);
                    auto_rows.push(("lumetri.param.curves_hdr_range".into(), r, "HDR Range"));
                }
            } else {
                let (lbl, _, cid) = curve_tabs[curve_tab.min(curve_tabs.len() - 1)];
                let r = curve_editor(ui, e, cid, true, Color32::WHITE, &t, actions, clip, idx);
                auto_rows.push((format!("lumetri.curve.{cid}"), r, lbl));
            }
        }
        "Color Wheels & Match" => {
            if ui.available_width() >= 920.0 {
                ui.columns(2, |cols| {
                    cols[0].vertical(|ui| {
                        resolve_wheels_grid(ui, e, mt, &t, actions, auto_rows, clip, idx);
                    });
                    cols[1].vertical(|ui| {
                        match_controls(ui, app, lut_actions);
                    });
                });
            } else {
                resolve_wheels_grid(ui, e, mt, &t, actions, auto_rows, clip, idx);
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                match_controls(ui, app, lut_actions);
            }
        }
        "Power Windows / Máscara" => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Herramientas:")).color(t.text_dim).small());
                let pen_on = app.ui.mask_pen.as_ref().is_some_and(|p| p.clip == clip.0 && p.effect == idx);

                // Linear / Rectangle Window
                let rect_btn = ui.button(egui::RichText::new(tl!("Linear")).small()).on_hover_text(tl!("Añadir máscara rectangular (Linear Window)"));
                app.auto.add("lumetri.mask.add_linear", rect_btn.rect, "Añadir Linear Window");
                if rect_btn.clicked() {
                    mask_actions.push(("masks.add".into(), json!({"clip": clip.0, "effect": idx, "shape": "polygon"})));
                }

                // Circle / Ellipse Window
                let circle_btn = ui.button(egui::RichText::new(tl!("Circle")).small()).on_hover_text(tl!("Añadir máscara circular (Circle Window)"));
                app.auto.add("lumetri.mask.add_circle", circle_btn.rect, "Añadir Circle Window");
                if circle_btn.clicked() {
                    mask_actions.push(("masks.add".into(), json!({"clip": clip.0, "effect": idx, "shape": "ellipse"})));
                }

                // Pen / Free-draw Curve Window
                let mut pen_btn = egui::Button::new(egui::RichText::new(tl!("Pen")).small());
                if pen_on {
                    pen_btn = pen_btn.fill(t.accent.gamma_multiply(0.25)).stroke(egui::Stroke::new(1.0, t.accent));
                }
                let pen_resp = ui.add(pen_btn).on_hover_text(tl!("Dibujo libre de curvas Bézier en monitor"));
                app.auto.add("lumetri.mask.add_pen", pen_resp.rect, "Dibujar Máscara con Pluma");
                if pen_resp.clicked() {
                    app.ui.mask_pen = if pen_on { None } else { Some(crate::state::MaskPenDraft { clip: clip.0, effect: idx, points: vec![] }) };
                    if !pen_on {
                        app.ui.status = tl!("Haz clic en el monitor para trazar la máscara; haz clic en el primer punto para cerrar").into();
                    }
                }
            });

            if e.masks.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tl!("Sin máscaras en este nodo. Haz clic en Linear, Círculo o Pluma para crear una.")).color(t.text_dim).small());
            } else {
                let sel_mask_idx = app.session.state.selected_mask
                    .filter(|s| s.clip == clip && s.effect == idx)
                    .map(|s| s.mask.min(e.masks.len() - 1))
                    .unwrap_or(0);

                let m = &e.masks[sel_mask_idx];
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!("Ventana:")).color(t.text_dim).small());
                    let mut chosen_mask = sel_mask_idx;
                    craft_ui::ComboBox::from_id_salt("lumetri_mask_select")
                        .selected_text(format!("{} ({})", m.name, sel_mask_idx + 1))
                        .show_ui(ui, |ui| {
                            for (mi, mk) in e.masks.iter().enumerate() {
                                if ui.selectable_value(&mut chosen_mask, mi, format!("{} ({})", mk.name, mi + 1)).clicked() {
                                    mask_actions.push(("masks.select".into(), json!({"clip": clip.0, "effect": idx, "mask": mi})));
                                }
                            }
                        });

                    // Invert button
                    let mut inv = m.inverted;
                    if ui.selectable_label(inv, tl!("Invertir")).on_hover_text(tl!("Invertir máscara")).clicked() {
                        inv = !inv;
                        mask_actions.push(("masks.set".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx, "inverted": inv})));
                    }

                    // Delete button
                    if ui.button(egui::RichText::new("🗑").color(Color32::from_rgb(240, 100, 100))).on_hover_text(tl!("Eliminar máscara")).clicked() {
                        mask_actions.push(("masks.remove".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx})));
                    }
                });

                // Mask sliders: Feather, Opacity, Expansion
                let cur_feather = m.feather.f64_at(mt);
                let cur_opacity = m.opacity.f64_at(mt);
                let cur_expansion = m.expansion.f64_at(mt);

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!("Calado:")).small());
                    let mut f_val = cur_feather;
                    if ui.add(egui::DragValue::new(&mut f_val).range(0.0..=500.0).speed(0.5).suffix(" px")).changed() {
                        mask_actions.push(("masks.set".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx, "feather": f_val})));
                    }

                    ui.label(egui::RichText::new(tl!("Opacidad:")).small());
                    let mut op_val = cur_opacity;
                    if ui.add(egui::DragValue::new(&mut op_val).range(0.0..=100.0).speed(0.5).suffix(" %")).changed() {
                        mask_actions.push(("masks.set".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx, "opacity": op_val})));
                    }

                    ui.label(egui::RichText::new(tl!("Expansión:")).small());
                    let mut exp_val = cur_expansion;
                    if ui.add(egui::DragValue::new(&mut exp_val).range(-200.0..=200.0).speed(0.5).suffix(" px")).changed() {
                        mask_actions.push(("masks.set".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx, "expansion": exp_val})));
                    }
                });

                // Quick Mask Tracking (Resolve style)
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!("Tracker:")).color(t.text_dim).small());
                    if ui.button(tl!("◀ Track Atrás")).clicked() {
                        mask_actions.push(("masks.select".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx})));
                        mask_actions.push(("masks.track".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx, "direction": "backward"})));
                    }
                    if ui.button(tl!("Track Adelante ▶")).clicked() {
                        mask_actions.push(("masks.select".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx})));
                        mask_actions.push(("masks.track".into(), json!({"clip": clip.0, "effect": idx, "mask": sel_mask_idx, "direction": "forward"})));
                    }
                });
            }
        }
        "HSL Secondary" => {
            sub(ui, &t, tl!("Key"));
            slider(ui, e, mt, "hsl_hue", tl!("Hue"), Some(Gradient::Hue), &t, actions, clip, idx);
            for (id, label) in [
                ("hsl_hue_range", tl!("Hue Range")),
                ("hsl_sat_min", tl!("Saturation Min")),
                ("hsl_luma_min", tl!("Luma Min")),
                ("hsl_luma_max", tl!("Luma Max")),
                ("hsl_soft", tl!("Soften")),
            ] {
                slider(ui, e, mt, id, label, None, &t, actions, clip, idx);
            }
            let mask = match e.param("hsl_show_mask").map(|p| &p.value) {
                Some(ParamValue::Choice(c)) => *c as usize,
                _ => 0,
            };
            ui.horizontal(|ui| {
                ui.add_sized(vec2(110.0, 20.0), egui::Label::new(egui::RichText::new(tl!("Show Mask")).color(t.text_dim)));
                let mut sel = mask;
                let modes = [tl!("Off"), tl!("Color/Gray"), tl!("Color/Black"), tl!("White/Black")];
                craft_ui::ComboBox::from_id_salt("hsl-mask").selected_text(modes[mask.min(3)]).show_ui(ui, |ui| {
                    for (i, o) in modes.iter().enumerate() {
                        if ui.selectable_value(&mut sel, i, *o).changed() {
                            set(actions, "hsl_show_mask", json!(i));
                        }
                    }
                });
            });
            sub(ui, &t, tl!("Refine"));
            for (id, label) in [("hsl_denoise", tl!("Denoise")), ("hsl_blur", tl!("Blur"))] {
                let r = slider(ui, e, mt, id, label, None, &t, actions, clip, idx);
                auto_rows.push((format!("lumetri.param.{id}"), r, label));
            }
            sub(ui, &t, tl!("Correction"));
            slider(ui, e, mt, "hsl_temp", tl!("Temperature"), Some(Gradient::Temp), &t, actions, clip, idx);
            slider(ui, e, mt, "hsl_tint", tl!("Tint"), Some(Gradient::Tint), &t, actions, clip, idx);
            slider(ui, e, mt, "hsl_sat", tl!("Saturation"), None, &t, actions, clip, idx);
            slider(ui, e, mt, "hsl_hue_shift", tl!("Hue Shift"), Some(Gradient::Hue), &t, actions, clip, idx);
        }
        "Vignette" => {
            for (id, label) in [
                ("vignette_amount", tl!("Amount")),
                ("vignette_midpoint", tl!("Midpoint")),
                ("vignette_roundness", tl!("Roundness")),
                ("vignette_feather", tl!("Feather")),
            ] {
                slider(ui, e, mt, id, label, None, &t, actions, clip, idx);
            }
        }
        _ => {}
    }
}

/// The Color Grading panel's ≡ menu items: Export .cube… saves the grade of the clip the panel
/// shows (also an Apply Match result) as a 3D LUT (`lumetri.exportCube`; automation id
/// `panel.menu.LumetriColor.exportCube`). It is enabled only when that clip has an enabled
/// Color Grading, and exports that clip even when another selected clip has one.
/// True when the menu should close.
pub fn panel_menu(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let shown = selected(app).filter(|(_, it)| it.effects.iter().any(|e| e.enabled && e.effect == "lumetri")).map(|(c, _)| c);
    let r = ui.add_enabled(shown.is_some(), egui::Button::new(tl!("Export .cube…")));
    app.auto.add_response("panel.menu.LumetriColor.exportCube", &r, "Export .cube…");
    if let (true, Some(clip)) = (r.clicked(), shown) {
        if let Err(e) = crate::menus::invoke(app, ui.ctx(), "lumetri.exportCube", json!({"clip": clip.0})) {
            app.ui.status = e;
        }
        return true;
    }
    false
}

/// LUT menu: None, built-ins, the project's LUT library, Browse… (file dialog → `lut.import`).
fn lut_combo(ui: &mut egui::Ui, app: &mut FilmcraftApp, e: &EffectInstance, param: &str, label: &str, input: bool, out: &mut Vec<(&'static str, Value)>) {
    let t = app.tokens;
    let cur = match e.param(param).map(|p| &p.value) {
        Some(ParamValue::Text(s)) => s.clone(),
        _ => String::new(),
    };
    let cmd: &'static str = if input { "lumetri.setInputLut" } else { "lumetri.setLook" };
    let mut rect = Rect::NOTHING;
    ui.horizontal(|ui| {
        ui.add_sized(vec2(110.0, 20.0), egui::Label::new(egui::RichText::new(label).color(t.text_dim)));
        let width = (ui.available_width() - 40.0).max(60.0);
        let (r, pick) = lut_menu(ui, app, ("lumetri-lut", param), &cur, input, width, &format!("lumetri.{param}"));
        match pick {
            Some(LutPick::Lut(r)) => out.push((cmd, json!({"lut": r}))),
            Some(LutPick::Browse) => out.push((cmd, json!({"browse": true}))),
            None => {}
        }
        rect = r;
    });
    app.auto.add(&format!("lumetri.{param}"), rect, label);
}

/// What was chosen in a [`lut_menu`].
pub(crate) enum LutPick {
    /// A LUT reference (`builtin:<id>`, `lib:<id>`), or "" for None.
    Lut(String),
    /// Browse…: pick a .cube / .3dl file.
    Browse,
}

/// The LUT combo box for the LUT reference `cur`: None, the built-in input LUTs (`input`) or looks,
/// the project's LUT library, Browse…. Shared by the Color Grading panel and Effect Controls (#1123).
/// The entries get automation ids `<auto_id>.lut.<reference>` (`.lut.none`) and `<auto_id>.browse`.
pub(crate) fn lut_menu(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    salt: impl std::hash::Hash + std::fmt::Debug,
    cur: &str,
    input: bool,
    width: f32,
    auto_id: &str,
) -> (Rect, Option<LutPick>) {
    let shown = filmcraft_render::luts::label(Some(&app.session.project), cur);
    let mut entries: Vec<(String, String)> = vec![(String::new(), tl!("None").into())];
    let builtins: Vec<&filmcraft_render::luts::Builtin> = if input {
        filmcraft_render::luts::input_builtins().collect()
    } else {
        filmcraft_render::luts::builtins().iter().filter(|b| b.id.starts_with("look-")).collect()
    };
    entries.extend(builtins.into_iter().map(|b| (format!("builtin:{}", b.id), b.label.clone())));
    entries.extend(app.session.project.luts.iter().map(|l| (format!("lib:{}", l.id), l.name.clone())));
    let mut pick = None;
    let r = craft_ui::ComboBox::from_id_salt(salt).selected_text(shown).truncate().width(width).show_ui(ui, |ui| {
        for (r, name) in &entries {
            let b = ui.selectable_label(r == cur, name);
            app.auto.add(&format!("{auto_id}.lut.{}", if r.is_empty() { "none" } else { r }), b.rect, name);
            if b.clicked() && r != cur {
                pick = Some(LutPick::Lut(r.clone()));
            }
        }
        ui.separator();
        let b = ui.selectable_label(false, tl!("Browse…"));
        app.auto.add(&format!("{auto_id}.browse"), b.rect, tl!("Browse…"));
        if b.clicked() {
            pick = Some(LutPick::Browse);
        }
    });
    (r.response.rect, pick)
}

fn sub(ui: &mut egui::Ui, t: &Tokens, name: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(name).color(t.text).strong().size(12.0));
}

/// Resolve-style rotary knob (perilla) with color-guided dual tracks, indicator pointer, drag interaction and double-click reset.
#[allow(clippy::too_many_arguments)]
fn rotary_knob(
    ui: &mut egui::Ui,
    e: &EffectInstance,
    mt: Tick,
    id: &'static str,
    label: &str,
    short_label: &str,
    range: (f64, f64),
    default: f64,
    speed: f64,
    decimals: usize,
    unit: &str,
    accent_color: Option<Color32>,
    t: &Tokens,
    actions: &mut Actions,
    clip: ClipId,
    idx: usize,
) -> Rect {
    let cur_val = e.param(id).map(|p| p.f64_at(mt)).unwrap_or(default);
    let (item_r, resp) = ui.allocate_exact_size(vec2(50.0, 42.0), Sense::click_and_drag());
    let p = ui.painter();

    // Drag interaction
    let mut new_val = cur_val;
    if resp.dragged() {
        let delta = resp.drag_delta();
        let shift = ui.ctx().input(|i| i.modifiers.shift);
        let factor = if shift { 0.25 } else { 1.0 };
        let step = (delta.x - delta.y) as f64 * speed * factor;
        new_val = (cur_val + step).clamp(range.0, range.1);
    }
    if resp.double_clicked() {
        new_val = default;
    }

    // Color theme configuration based on parameter ID (DaVinci Resolve style color cues)
    let (track_col_neg, track_col_pos, active_col_neg, active_col_pos) = match id {
        "temperature" => (
            Color32::from_rgb(30, 125, 235),  // Cool Cyan/Blue
            Color32::from_rgb(255, 125, 25),  // Warm Orange
            Color32::from_rgb(56, 189, 248),  // Glowing Cyan
            Color32::from_rgb(251, 146, 60),  // Glowing Orange
        ),
        "tint" => (
            Color32::from_rgb(40, 195, 75),   // Green
            Color32::from_rgb(230, 50, 185),  // Magenta
            Color32::from_rgb(74, 222, 128),  // Glowing Green
            Color32::from_rgb(244, 63, 94),   // Glowing Magenta
        ),
        "exposure" => (
            Color32::from_rgb(70, 85, 115),
            Color32::from_rgb(240, 205, 50),
            Color32::from_rgb(120, 150, 190),
            Color32::from_rgb(250, 204, 21),
        ),
        "contrast" => (
            Color32::from_rgb(85, 90, 105),
            Color32::from_rgb(170, 90, 240),
            Color32::from_rgb(130, 140, 160),
            Color32::from_rgb(192, 132, 252),
        ),
        "highlights" => (
            Color32::from_rgb(95, 100, 115),
            Color32::from_rgb(250, 240, 160),
            Color32::from_rgb(130, 135, 150),
            Color32::from_rgb(254, 240, 138),
        ),
        "shadows" => (
            Color32::from_rgb(50, 70, 115),
            Color32::from_rgb(100, 165, 250),
            Color32::from_rgb(59, 130, 246),
            Color32::from_rgb(147, 197, 253),
        ),
        "whites" => (
            Color32::from_rgb(90, 100, 115),
            Color32::from_rgb(245, 250, 255),
            Color32::from_rgb(130, 140, 155),
            Color32::from_rgb(255, 255, 255),
        ),
        "blacks" => (
            Color32::from_rgb(30, 35, 45),
            Color32::from_rgb(130, 145, 165),
            Color32::from_rgb(71, 85, 105),
            Color32::from_rgb(148, 163, 184),
        ),
        "saturation" => (
            Color32::from_rgb(90, 90, 90),
            Color32::from_rgb(50, 210, 120),
            Color32::from_rgb(100, 116, 139),
            Color32::from_rgb(52, 211, 153),
        ),
        "vibrance" => (
            Color32::from_rgb(70, 100, 130),
            Color32::from_rgb(40, 200, 230),
            Color32::from_rgb(94, 234, 212),
            Color32::from_rgb(34, 211, 238),
        ),
        "sharpen" => (
            Color32::from_rgb(80, 90, 100),
            Color32::from_rgb(170, 230, 60),
            Color32::from_rgb(110, 120, 130),
            Color32::from_rgb(163, 230, 53),
        ),
        _ => {
            let col = accent_color.unwrap_or_else(|| Color32::from_rgb(60, 140, 240));
            (col.gamma_multiply(0.4), col, col, col)
        }
    };

    // Top Label in small caps
    let label_pos = pos2(item_r.center().x, item_r.min.y + 5.5);
    p.text(
        label_pos,
        Align2::CENTER_CENTER,
        short_label,
        Tokens::semibold(9.0),
        if resp.hovered() { Color32::from_rgb(255, 185, 60) } else { t.text_dim },
    );

    // Draw intuitive guide dots for Temperature and Tint
    if id == "temperature" || id == "tint" {
        p.circle_filled(pos2(item_r.center().x - 16.0, item_r.min.y + 5.5), 2.2, track_col_neg);
        p.circle_filled(pos2(item_r.center().x + 16.0, item_r.min.y + 5.5), 2.2, track_col_pos);
    }

    // Dial Center & Radius
    let kc = pos2(item_r.center().x, item_r.min.y + 19.5);
    let radius = 9.5;

    // Outer Dial Ring & Background
    let is_active = resp.hovered() || resp.dragged();
    let ring_color = if is_active {
        Color32::from_rgb(255, 175, 40) // Resolve signature gold glow
    } else {
        Color32::from_rgb(45, 48, 58)
    };
    p.circle_filled(kc, radius, Color32::from_rgb(20, 23, 29));
    p.circle_stroke(kc, radius, Stroke::new(1.0, ring_color));

    // Dial Angles Math
    let span = (range.1 - range.0).max(1e-4);
    let norm = ((cur_val - range.0) / span).clamp(0.0, 1.0) as f32;
    let def_norm = ((default - range.0) / span).clamp(0.0, 1.0) as f32;

    let min_ang = -135f32.to_radians();
    let max_ang = 135f32.to_radians();
    let cur_ang = min_ang + norm * (max_ang - min_ang);
    let def_ang = min_ang + def_norm * (max_ang - min_ang);

    // Subtle background guide tracks (negative half & positive half)
    let arc_r = radius - 3.0;
    let track_steps = 8;
    let mut prev = kc + vec2(min_ang.sin(), -min_ang.cos()) * arc_r;
    for s in 1..=track_steps {
        let f = s as f32 / track_steps as f32;
        let a = min_ang + f * (def_ang - min_ang);
        let pt = kc + vec2(a.sin(), -a.cos()) * arc_r;
        p.line_segment([prev, pt], Stroke::new(1.8, track_col_neg.gamma_multiply(0.28)));
        prev = pt;
    }
    prev = kc + vec2(def_ang.sin(), -def_ang.cos()) * arc_r;
    for s in 1..=track_steps {
        let f = s as f32 / track_steps as f32;
        let a = def_ang + f * (max_ang - def_ang);
        let pt = kc + vec2(a.sin(), -a.cos()) * arc_r;
        p.line_segment([prev, pt], Stroke::new(1.8, track_col_pos.gamma_multiply(0.28)));
        prev = pt;
    }

    // Default center tick mark (0° marker)
    let tick_tip = kc + vec2(def_ang.sin(), -def_ang.cos()) * (radius + 1.0);
    let tick_base = kc + vec2(def_ang.sin(), -def_ang.cos()) * (radius - 2.0);
    p.line_segment([tick_base, tick_tip], Stroke::new(1.2, Color32::from_rgb(140, 150, 170)));

    // Active Value Arc
    let is_modified = (cur_val - default).abs() > 1e-4;
    let active_color = if cur_val < default { active_col_neg } else { active_col_pos };
    if is_modified {
        let (a_start, a_end) = if cur_ang >= def_ang { (def_ang, cur_ang) } else { (cur_ang, def_ang) };
        if (a_end - a_start).abs() > 0.02 {
            let arc_steps = 14;
            let mut prev = kc + vec2(a_start.sin(), -a_start.cos()) * arc_r;
            for s in 1..=arc_steps {
                let f = s as f32 / arc_steps as f32;
                let a = a_start + f * (a_end - a_start);
                let pt = kc + vec2(a.sin(), -a.cos()) * arc_r;
                p.line_segment([prev, pt], Stroke::new(2.6, active_color));
                prev = pt;
            }
        }
    }

    // Needle pointer line & tip dot
    let pointer_tip = kc + vec2(cur_ang.sin(), -cur_ang.cos()) * (radius - 2.5);
    let pointer_base = kc + vec2(cur_ang.sin(), -cur_ang.cos()) * 4.0;
    p.line_segment([pointer_base, pointer_tip], Stroke::new(2.0, Color32::WHITE));
    p.circle_filled(pointer_tip, 2.0, if is_modified { active_color } else { Color32::WHITE });

    // Center Hub
    p.circle_filled(kc, 3.0, Color32::from_rgb(45, 50, 62));
    p.circle_stroke(kc, 3.0, Stroke::new(1.0, Color32::from_rgb(85, 95, 115)));

    // Bottom numeric readout
    let val_text = if decimals == 0 {
        format!("{:.0}{unit}", cur_val)
    } else {
        format!("{:.1}{unit}", cur_val)
    };
    let val_pos = pos2(item_r.center().x, item_r.max.y - 6.0);
    let val_color = if is_modified {
        active_color
    } else {
        t.text_dim
    };
    p.text(val_pos, Align2::CENTER_CENTER, val_text, Tokens::ui(9.5), val_color);

    let threshold = if decimals == 0 { 0.1 } else { 0.01 };
    let delta = (new_val - cur_val).abs();
    if (delta >= threshold || resp.drag_stopped() || resp.double_clicked()) && delta > 1e-5 {
        actions.push(json!({"clip": clip.0, "effect": idx, "param": id, "value": new_val}));
    }

    resp.on_hover_text(format!("{label}: {cur_val:.1}{unit} (Arrastra o rueda; doble clic para resetear)"));
    item_r
}

#[derive(Clone, Copy)]
enum Gradient {
    Temp,
    Tint,
    Hue,
}

/// Spectrum slider: 1 pt track (or gradient), hollow ring knob, scrubby value at the right.
#[allow(clippy::too_many_arguments)]
fn slider(
    ui: &mut egui::Ui,
    e: &EffectInstance,
    mt: Tick,
    id: &str,
    label: &str,
    grad: Option<Gradient>,
    t: &Tokens,
    actions: &mut Actions,
    clip: ClipId,
    idx: usize,
) -> Rect {
    let Some(pd) = e.def().and_then(|d| d.param(id)) else { return Rect::NOTHING };
    let filmcraft_project::ParamKind::Float { min, max, soft_min, soft_max, decimals, .. } = pd.kind else { return Rect::NOTHING };
    let v = e.param(id).map(|p| p.f64_at(mt)).unwrap_or(pd.default.as_f64().unwrap_or(0.0));
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(pos2(row.min.x + 4.0, row.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text_dim);
    let track = Rect::from_min_max(pos2(row.min.x + 112.0, row.center().y - 1.0), pos2(row.max.x - 58.0, row.center().y + 1.0));
    match grad {
        Some(g) => {
            let n = 24;
            for i in 0..n {
                let f0 = i as f32 / n as f32;
                let c = match g {
                    Gradient::Temp => lerp_col(Color32::from_rgb(70, 120, 230), Color32::from_rgb(240, 150, 50), f0),
                    Gradient::Tint => lerp_col(Color32::from_rgb(60, 190, 80), Color32::from_rgb(220, 70, 200), f0),
                    Gradient::Hue => {
                        let rgb = filmcraft_color::hsl_to_rgb(f0, 0.8, 0.5);
                        Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8)
                    }
                };
                let x0 = track.min.x + track.width() * f0;
                ui.painter().rect_filled(Rect::from_min_max(pos2(x0, track.min.y - 1.0), pos2(x0 + track.width() / n as f32 + 0.5, track.max.y + 1.0)), 0.0, c);
            }
        }
        None => {
            ui.painter().rect_filled(track, 0.0, Color32::from_rgb(0x6e, 0x6e, 0x6e));
        }
    }
    let f = ((v - soft_min) / (soft_max - soft_min)).clamp(0.0, 1.0) as f32;
    let kx = track.min.x + f * track.width();
    let hit = Rect::from_min_max(pos2(track.min.x - 6.0, row.min.y), pos2(track.max.x + 6.0, row.max.y));
    let resp = ui.interact(hit, egui::Id::unique(("lslider", clip.0, id)), Sense::click_and_drag());
    ui.painter().circle_filled(pos2(kx, track.center().y), 5.0, t.panel_bg);
    ui.painter().circle_stroke(
        pos2(kx, track.center().y),
        5.0,
        Stroke::new(1.5, if resp.dragged() { t.hot_text } else { Color32::from_rgb(0xb0, 0xb0, 0xb0) }),
    );
    let mut out = None;
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let nf = ((p.x - track.min.x) / track.width()).clamp(0.0, 1.0) as f64;
        out = Some(soft_min + nf * (soft_max - soft_min));
    }
    if resp.double_clicked() {
        out = pd.default.as_f64();
    }
    let mut vui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(row.max.x - 52.0, row.min.y + 3.0), pos2(row.max.x, row.max.y - 3.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let (_, nv) = crate::widgets::hot_number(
        &mut vui,
        label,
        egui::Id::unique(("lval", clip.0, id)),
        v,
        crate::widgets::HotNumberOptions::new((soft_max - soft_min) / 300.0, (min, max), decimals as usize, "", t),
    );
    if let Some(nv) = out.or(nv) {
        let rounded = (nv * 10f64.powi(decimals as i32)).round() / 10f64.powi(decimals as i32);
        if (rounded - v).abs() > 1e-5 {
            actions.push(json!({"clip": clip.0, "effect": idx, "param": id, "value": rounded}));
        }
    }
    row
}

fn lerp_col(a: Color32, b: Color32, f: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Curve editor: drag points, click the curve to add, double-click a point to remove. Returns its
/// rectangle (its automation element, `lumetri.curve.<param>`).
#[allow(clippy::too_many_arguments)]
fn curve_editor(ui: &mut egui::Ui, e: &EffectInstance, id: &str, hue: bool, col: Color32, t: &Tokens, actions: &mut Actions, clip: ClipId, idx: usize) -> Rect {
    let pts: Vec<[f32; 2]> = e.param(id).and_then(|p| p.value.as_curve().map(|c| c.to_vec())).unwrap_or_default();
    // as wide as the panel (it stayed 260 px wide in a wide panel, #1658), not taller than 420 px
    let w = ui.available_width().max(1.0);
    let avail_h = (ui.clip_rect().max.y - ui.cursor().min.y - 12.0).clamp(80.0, 420.0);
    let h = if hue { (w * 0.45).min(avail_h) } else { w.min(avail_h) };
    let (area, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
    let p = ui.painter();
    p.rect_filled(area, 2.0, Color32::from_rgb(0x16, 0x16, 0x16));
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
    if hue {
        let n = 36;
        for i in 0..n {
            let f0 = i as f32 / n as f32;
            let rgb = filmcraft_color::hsl_to_rgb(f0, 0.8, 0.5);
            let c = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
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
    let to_screen = |q: [f32; 2]| pos2(area.min.x + q[0] * area.width(), area.max.y - q[1] * area.height());
    let from_screen = |s: Pos2| [((s.x - area.min.x) / area.width()).clamp(0.0, 1.0), ((area.max.y - s.y) / area.height()).clamp(0.0, 1.0)];
    // curve polyline
    let draw_pts: Vec<[f32; 2]> = if hue && pts.is_empty() { vec![[0.0, 0.5], [1.0, 0.5]] } else { pts.clone() };
    // the hue curves wrap around red; Luma vs Sat and Sat vs Sat do not (same LUTs as the render)
    let lut = match (hue, id.starts_with("hue_vs_")) {
        (true, true) => filmcraft_render::effects::hue_lut(&pts, 128),
        (true, false) => filmcraft_render::effects::vs_sat_lut(&pts, 128),
        _ => None,
    }
    .unwrap_or_else(|| filmcraft_render::effects::curve_lut(&draw_pts, 128));
    let line: Vec<Pos2> = lut.iter().enumerate().map(|(i, y)| to_screen([i as f32 / 127.0, y.clamp(0.0, 1.0)])).collect();
    p.add(egui::Shape::line(line.clone(), Stroke::new(1.5, col)));
    // the point nearest `pos` within reach (drag and double-click pick the same one)
    let nearest = |pos: Pos2| {
        pts.iter().enumerate().map(|(i, q)| (i, to_screen(*q).distance(pos))).filter(|(_, d)| *d < 9.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
    };
    let drag_key = egui::Id::unique(("curve-drag", clip.0, id));
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_key));
    let mut new_pts = pts.clone();
    let mut changed = false;
    for (i, q) in pts.iter().enumerate() {
        let sp = to_screen(*q);
        p.circle_filled(sp, 4.0, if dragging == Some(i) { t.hot_text } else { col });
    }
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let near = nearest(pos);
        dragging = near.or_else(|| {
            new_pts.push(from_screen(pos));
            changed = true;
            Some(new_pts.len() - 1)
        });
    }
    if resp.dragged()
        && let (Some(i), Some(pos)) = (dragging, resp.interact_pointer_pos())
        && i < new_pts.len()
    {
        new_pts[i] = from_screen(pos);
        changed = true;
    }
    if resp.drag_stopped() {
        dragging = None;
    }
    // A plain click on the curve places a point where it landed (#3431): before, only a drag
    // created points, so the curve ignored the click the interaction promises. A click on a
    // point selects it (as a drag start does); a click off the curve adds nothing, so the RGB
    // double-click reset and the hue double-click remove keep working.
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        match nearest(pos) {
            Some(i) => dragging = Some(i),
            None if line.iter().any(|p| p.distance(pos) < 9.0) => {
                dragging = None;
                new_pts.push(from_screen(pos));
                changed = true;
            }
            _ => dragging = None,
        }
    }
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(i) = nearest(pos)
    {
        if let Some(rest) = without_point(&pts, i, hue) {
            new_pts = rest;
            changed = true;
        }
    } else if resp.double_clicked() && !hue {
        new_pts = vec![[0.0, 0.0], [1.0, 1.0]];
        changed = true;
    }
    ui.data_mut(|d| {
        if let Some(i) = dragging {
            d.insert_temp(drag_key, i);
        } else {
            d.remove::<usize>(drag_key);
        }
    });
    if changed {
        let v: Vec<Value> = new_pts.iter().map(|q| json!([q[0], q[1]])).collect();
        actions.push(json!({"clip": clip.0, "effect": idx, "param": id, "value": v}));
    }
    ui.add_space(6.0);
    area
}

/// `pts` without point `i` (double-click), or `None` for a point that stays: the leftmost and
/// rightmost points of an RGB curve. Points are stored in the order they were added, not by x, so
/// list position says nothing (the newest point could never be removed, #1658).
fn without_point(pts: &[[f32; 2]], i: usize, hue: bool) -> Option<Vec<[f32; 2]>> {
    let q = pts.get(i)?;
    let (lo, hi) = pts.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    if !hue && (q[0] <= lo || q[0] >= hi) {
        return None;
    }
    let mut rest = pts.to_vec();
    rest.remove(i);
    Some(rest)
}

/// DaVinci Resolve-style primary knob bar (full 11 knobs or 8 adjustment knobs under the 4 color wheels).
#[allow(clippy::too_many_arguments)]
fn draw_primary_knobs_strip(
    ui: &mut egui::Ui,
    e: &EffectInstance,
    mt: Tick,
    include_wb_exp: bool,
    t: &Tokens,
    actions: &mut Actions,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
    clip: ClipId,
    idx: usize,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if include_wb_exp {
            let r = rotary_knob(ui, e, mt, "temperature", tl!("Temperature"), "TEMP", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(255, 140, 50)), t, actions, clip, idx);
            auto_rows.push(("lumetri.knob.temperature".into(), r, "Temperature"));

            let r = rotary_knob(ui, e, mt, "tint", tl!("Tint"), "TINT", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(220, 80, 200)), t, actions, clip, idx);
            auto_rows.push(("lumetri.knob.tint".into(), r, "Tint"));

            let r = rotary_knob(ui, e, mt, "exposure", tl!("Exposure"), "EXP", (-5.0, 5.0), 0.0, 0.02, 1, " EV", Some(Color32::from_rgb(240, 220, 100)), t, actions, clip, idx);
            auto_rows.push(("lumetri.knob.exposure".into(), r, "Exposure"));
        }

        let r = rotary_knob(ui, e, mt, "contrast", tl!("Contrast"), "CONT", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(180, 190, 210)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.contrast".into(), r, "Contrast"));

        let r = rotary_knob(ui, e, mt, "highlights", tl!("Highlights"), "HI", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(245, 245, 245)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.highlights".into(), r, "Highlights"));

        let r = rotary_knob(ui, e, mt, "shadows", tl!("Shadows"), "SHAD", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(90, 100, 120)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.shadows".into(), r, "Shadows"));

        let r = rotary_knob(ui, e, mt, "whites", tl!("Whites"), "WHITE", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(255, 255, 255)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.whites".into(), r, "Whites"));

        let r = rotary_knob(ui, e, mt, "blacks", tl!("Blacks"), "BLACK", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(70, 75, 85)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.blacks".into(), r, "Blacks"));

        let r = rotary_knob(ui, e, mt, "saturation", tl!("Saturation"), "SAT", (0.0, 200.0), 100.0, 0.4, 0, "%", Some(Color32::from_rgb(80, 200, 120)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.saturation".into(), r, "Saturation"));

        let r = rotary_knob(ui, e, mt, "vibrance", tl!("Vibrance"), "VIB", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(100, 170, 240)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.vibrance".into(), r, "Vibrance"));

        let r = rotary_knob(ui, e, mt, "sharpen", tl!("Sharpen"), "SHRP", (-100.0, 100.0), 0.0, 0.4, 0, "", Some(Color32::from_rgb(220, 180, 100)), t, actions, clip, idx);
        auto_rows.push(("lumetri.knob.sharpen".into(), r, "Sharpen"));
    });
}

#[derive(Clone, Copy)]
enum WheelKind {
    Lift,
    Gamma,
    Gain,
    Offset,
}

/// DaVinci Resolve 4-Wheel layout: Lift (Shadows), Gamma (Midtones), Gain (Highlights), Offset (Temp/Tint/Exp).
#[allow(clippy::too_many_arguments)]
fn resolve_wheels_grid(
    ui: &mut egui::Ui,
    e: &EffectInstance,
    mt: Tick,
    t: &Tokens,
    actions: &mut Actions,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
    clip: ClipId,
    idx: usize,
) {
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
                    resolve_wheel_card(ui, e, mt, kind, title, sub, t, actions, auto_rows, clip, idx);
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
                        resolve_wheel_card(ui, e, mt, kind, title, sub, t, actions, auto_rows, clip, idx);
                    });
                }
            });
            ui.add_space(3.0);
        }
    } else {
        for (kind, title, sub) in cards {
            ui.allocate_ui_with_layout(vec2(avail, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.set_max_width(avail);
                resolve_wheel_card(ui, e, mt, kind, title, sub, t, actions, auto_rows, clip, idx);
            });
            ui.add_space(3.0);
        }
    }
}

/// Vector-drawn circular reset arrow button (avoids selectable text glyph / I-beam cursor).
fn paint_reset_icon_button(ui: &mut egui::Ui, color: Color32, tooltip: &str) -> egui::Response {
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
        // Arrowhead at the counter-clockwise leading end (`start`).
        let tip = pos2(c.x + r * start.cos(), c.y + r * start.sin());
        p.line_segment([tip, tip + vec2(3.2, -0.4)], Stroke::new(1.35, col));
        p.line_segment([tip, tip + vec2(0.4, 3.2)], Stroke::new(1.35, col));
    }
    resp
}

/// Single DaVinci Resolve color wheel card: Header + Reset, Hue Trackball, Horizontal Knurled Master Wheel, and Live Readout.
#[allow(clippy::too_many_arguments)]
fn resolve_wheel_card(
    ui: &mut egui::Ui,
    e: &EffectInstance,
    mt: Tick,
    kind: WheelKind,
    title: &'static str,
    subtitle: &str,
    t: &Tokens,
    actions: &mut Actions,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
    clip: ClipId,
    idx: usize,
) {
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

    let modified = vx.abs() > 1e-3 || vy.abs() > 1e-3 || lum.abs() > 1e-3;

    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(23, 26, 32))
        .stroke(Stroke::new(1.0, if modified { Color32::from_rgb(65, 85, 118) } else { Color32::from_rgb(38, 43, 52) }))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(4, 2));

    let inner_max = (ui.available_width() - 10.0).max(60.0);
    let frame_resp = frame.show(ui, |ui| {
        ui.set_max_width(inner_max);
        ui.spacing_mut().item_spacing.y = 1.0;
        // Card Header: Reset anchored right, Title (Subtitle) truncated to fit strictly inside inner_max
        ui.allocate_ui_with_layout(vec2(inner_max, 16.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_max_width(inner_max);
            let rst_col = if modified { t.accent } else { t.text_faint };
            let rst = paint_reset_icon_button(ui, rst_col, tl!("Reset this wheel (or double-click)"));
            if rst.clicked() {
                match kind {
                    WheelKind::Lift => {
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_shadows", "value": [0.0, 0.0]}));
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_shadows_l", "value": 0.0}));
                    }
                    WheelKind::Gamma => {
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_midtones", "value": [0.0, 0.0]}));
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_midtones_l", "value": 0.0}));
                    }
                    WheelKind::Gain => {
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_highlights", "value": [0.0, 0.0]}));
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_highlights_l", "value": 0.0}));
                    }
                    WheelKind::Offset => {
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "temperature", "value": 0.0}));
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "tint", "value": 0.0}));
                        actions.push(json!({"clip": clip.0, "effect": idx, "param": "exposure", "value": 0.0}));
                    }
                }
            }
            let title_w = ui.available_width().max(24.0);
            ui.allocate_ui_with_layout(vec2(title_w, 16.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_max_width(title_w);
                ui.label(egui::RichText::new(title).strong().size(11.0).color(if modified { Color32::WHITE } else { t.text }));
                ui.add(egui::Label::new(egui::RichText::new(format!("({subtitle})")).size(9.0).color(t.text_dim)).truncate());
            });
        });

        ui.add_space(1.0);

        // Centered Circular Hue Trackball
        let wheel_size = ui.available_width().clamp(48.0, 56.0);
        let row_w = ui.available_width();
        let (area, resp) = ui.allocate_exact_size(vec2(row_w, wheel_size), Sense::click_and_drag());
        let c = area.center();
        let r_out = wheel_size / 2.0 - 2.0;
        let r_in = r_out - 9.0;
        let p = ui.painter();

        // Dark outer bezel
        p.circle_stroke(c, r_out + 1.0, Stroke::new(1.5, Color32::from_rgb(14, 16, 20)));

        let n = 72;
        let mut mesh = egui::Mesh::default();
        for i in 0..n {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            let rgb = filmcraft_color::hsl_to_rgb(1.0 - (i as f32 / n as f32), 0.78, 0.52);
            let col = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
            let base = mesh.vertices.len() as u32;
            for (a, r) in [(a0, r_in), (a0, r_out), (a1, r_out), (a1, r_in)] {
                mesh.colored_vertex(c + vec2(a.cos(), -a.sin()) * r, col);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        p.add(mesh);

        // Inner dark trackball well + subtle crosshair + concentric guide circle
        p.circle_filled(c, r_in - 1.0, Color32::from_rgb(16, 18, 22));
        p.circle_stroke(c, (r_in - 2.0) * 0.5, Stroke::new(0.8, Color32::from_rgb(32, 36, 44)));
        let ch_r = r_in - 5.0;
        p.line_segment([c - vec2(ch_r, 0.0), c + vec2(ch_r, 0.0)], Stroke::new(0.8, Color32::from_rgb(42, 47, 56)));
        p.line_segment([c - vec2(0.0, ch_r), c + vec2(0.0, ch_r)], Stroke::new(0.8, Color32::from_rgb(42, 47, 56)));
        p.circle_filled(c, 1.5, Color32::from_rgb(75, 82, 95));

        let dish_r = (r_in - 5.0).max(12.0);
        let puck = c + vec2(vx, -vy) * dish_r;
        if vx.abs() > 1e-3 || vy.abs() > 1e-3 {
            p.line_segment([c, puck], Stroke::new(1.0, Color32::from_rgba_unmultiplied(220, 225, 235, 110)));
        }

        // Tinted puck color reflecting hue direction
        let puck_mag = (vx * vx + vy * vy).sqrt().clamp(0.0, 1.0);
        let puck_hue = ((-vy).atan2(vx).rem_euclid(std::f32::consts::TAU)) / std::f32::consts::TAU;
        let prgb = filmcraft_color::hsl_to_rgb(1.0 - puck_hue, puck_mag * 0.85, 0.55);
        let puck_fill = if puck_mag > 0.02 {
            Color32::from_rgb((prgb[0] * 255.0) as u8, (prgb[1] * 255.0) as u8, (prgb[2] * 255.0) as u8)
        } else {
            Color32::from_rgb(55, 60, 70)
        };
        p.circle_filled(puck, 4.5, puck_fill);
        p.circle_stroke(puck, 5.0, Stroke::new(1.5, Color32::WHITE));

        // Trackball drag/click interaction (Shift = precision 0.25x delta)
        let emit_2d = |actions: &mut Actions, nx: f32, ny: f32| match kind {
            WheelKind::Lift => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_shadows", "value": [nx, ny], "merge": true})),
            WheelKind::Gamma => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_midtones", "value": [nx, ny], "merge": true})),
            WheelKind::Gain => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_highlights", "value": [nx, ny], "merge": true})),
            WheelKind::Offset => {
                actions.push(json!({"clip": clip.0, "effect": idx, "param": "temperature", "value": (nx * 100.0).clamp(-100.0, 100.0), "merge": true}));
                actions.push(json!({"clip": clip.0, "effect": idx, "param": "tint", "value": (ny * 100.0).clamp(-100.0, 100.0), "merge": true}));
            }
        };

        if resp.double_clicked() {
            emit_2d(actions, 0.0, 0.0);
        } else if resp.dragged() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            if shift {
                let d = resp.drag_delta() * 0.25 / dish_r;
                let mut cur = vec2(vx + d.x, vy - d.y);
                if cur.length() > 1.0 {
                    cur = cur.normalized();
                }
                if (cur.x - vx).abs() > 1e-4 || (cur.y - vy).abs() > 1e-4 {
                    emit_2d(actions, cur.x, cur.y);
                }
            } else if let Some(pos) = resp.interact_pointer_pos() {
                let mut d = (pos - c) / dish_r;
                if d.length() > 1.0 {
                    d = d.normalized();
                }
                let nx = d.x;
                let ny = -d.y;
                if (nx - vx).abs() > 1e-4 || (ny - vy).abs() > 1e-4 {
                    emit_2d(actions, nx, ny);
                }
            }
        } else if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let mut d = (pos - c) / dish_r;
            if d.length() > 1.0 {
                d = d.normalized();
            }
            emit_2d(actions, d.x, -d.y);
        }

        ui.add_space(1.0);

        // Horizontal Knurled Master Wheel directly under the trackball (DaVinci Resolve style)
        let (track, lr) = ui.allocate_exact_size(vec2(row_w, 11.0), Sense::click_and_drag());
        let lp = ui.painter();
        lp.rect_filled(track, 3.0, Color32::from_rgb(15, 17, 21));
        lp.rect_stroke(track, 3.0, Stroke::new(1.0, Color32::from_rgb(44, 50, 62)), StrokeKind::Inside);

        // Subtle knurled vertical ticks across the master wheel
        let ticks = 16;
        for i in 1..ticks {
            let tx = track.min.x + track.width() * (i as f32 / ticks as f32);
            let is_mid = i == ticks / 2;
            lp.line_segment(
                [pos2(tx, track.min.y + if is_mid { 2.0 } else { 4.0 }), pos2(tx, track.max.y - if is_mid { 2.0 } else { 4.0 })],
                Stroke::new(1.0, if is_mid { Color32::from_rgb(90, 98, 114) } else { Color32::from_rgb(36, 40, 50) }),
            );
        }

        // Bipolar fill + vertical needle thumb
        let frac = (lum / lum_max).clamp(-1.0, 1.0);
        let cx = track.center().x;
        let kx = cx + frac * (track.width() * 0.5 - 4.0);
        if frac.abs() > 0.005 {
            let fill_r = Rect::from_min_max(
                pos2(cx.min(kx), track.min.y + 3.0),
                pos2(cx.max(kx), track.max.y - 3.0),
            );
            let fill_col = if frac >= 0.0 {
                Color32::from_rgba_unmultiplied(235, 185, 70, 120)
            } else {
                Color32::from_rgba_unmultiplied(80, 150, 235, 120)
            };
            lp.rect_filled(fill_r, 1.5, fill_col);
        }
        lp.rect_filled(
            Rect::from_center_size(pos2(kx, track.center().y), vec2(4.0, track.height() - 4.0)),
            1.5,
            Color32::from_rgb(235, 240, 248),
        );

        let emit_lum = |actions: &mut Actions, nv: f32| match kind {
            WheelKind::Lift => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_shadows_l", "value": nv, "merge": true})),
            WheelKind::Gamma => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_midtones_l", "value": nv, "merge": true})),
            WheelKind::Gain => actions.push(json!({"clip": clip.0, "effect": idx, "param": "wheel_highlights_l", "value": nv, "merge": true})),
            WheelKind::Offset => actions.push(json!({"clip": clip.0, "effect": idx, "param": "exposure", "value": nv, "merge": true})),
        };

        if lr.double_clicked() {
            emit_lum(actions, 0.0);
        } else if lr.dragged() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            let mult = if shift { 0.25 } else { 1.0 };
            let step = (lr.drag_delta().x / (track.width() * 0.5).max(1.0)) * lum_max * mult;
            let nv = (lum + step).clamp(-lum_max, lum_max);
            if (nv - lum).abs() > 1e-3 {
                emit_lum(actions, nv);
            }
        } else if lr.clicked()
            && let Some(pos) = lr.interact_pointer_pos()
        {
            let nv = ((pos.x - cx) / (track.width() * 0.5 - 4.0).max(1.0) * lum_max).clamp(-lum_max, lum_max);
            emit_lum(actions, nv);
        }
        lr.on_hover_text(tl!("Drag horizontally to adjust luminance (Shift = fine, Double-click = reset)"));

        // Numeric Readout below Master Wheel
        let readout = match kind {
            WheelKind::Offset => format!("T:{:+.0} Ti:{:+.0} · {:+.1}EV", vx * 100.0, vy * 100.0, lum),
            _ => format!("X:{:+.2} Y:{:+.2} · L:{:+.0}", vx, vy, lum),
        };
        let (ro_r, _) = ui.allocate_exact_size(vec2(row_w, 11.0), Sense::hover());
        ui.painter().text(
            ro_r.center(),
            Align2::CENTER_CENTER,
            readout,
            FontId::monospace(9.0),
            if modified { t.accent } else { t.text_dim },
        );
    });
    auto_rows.push((format!("lumetri.wheel.{id_tag}"), frame_resp.response.rect, title));
}

/// Color Match: reference frame (sequence timecode), Face Detection (skin-tone protection),
/// Apply Match (`lumetri.applyMatch`).
fn match_controls(ui: &mut egui::Ui, app: &mut FilmcraftApp, out: &mut Vec<(&'static str, Value)>) {
    let t = app.tokens;
    let key = egui::Id::unique("lumetri-match-ref");
    let fd_key = egui::Id::unique("lumetri-match-face");
    let mut tc: String = ui.data(|d| d.get_temp(key)).unwrap_or_else(|| "00:00:00:00".to_string());
    let mut face: bool = ui.data(|d| d.get_temp(fd_key)).unwrap_or(true);
    let comparing = app.ui.program.display_mode() == Some(crate::state::DisplayMode::Comparison);

    ui.horizontal(|ui| {
        sub(ui, &t, tl!("Color Match"));
        ui.add_space(8.0);
        let b = ui.selectable_label(comparing, tl!("Comparison View"));
        app.auto.add("lumetri.match.comparisonView", b.rect, "Comparison View");
        if b.clicked() {
            out.push(("view.display", json!({"display": if comparing { "composite" } else { "comparison" }})));
        }
    });

    let reference = app.ui.program.compare_ref.filter(|_| comparing).map(filmcraft_time::Tick);
    if let (Some(r), Some(q)) = (reference, app.session.active_sequence()) {
        tc = filmcraft_time::format_time(r, q.settings.frame_rate, q.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48000);
    }

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Reference")).color(t.text_dim).small());
        let r = crate::widgets::accessible_name(ui.add(egui::TextEdit::singleline(&mut tc).desired_width(85.0)), "Reference timecode");
        app.auto.add("lumetri.match.reference", r.rect, "Reference timecode");
        crate::widgets::select_all_on_focus(ui, &r, &tc);
        let b = ui.button(tl!("Apply Match"));
        app.auto.add("lumetri.match.apply", b.rect, "Apply Match");
        if b.clicked() {
            let p = match reference {
                Some(r) => json!({"referenceTime": r.0, "faceDetection": face}),
                None => json!({"referenceTimecode": tc, "faceDetection": face}),
            };
            out.push(("lumetri.applyMatch", p));
        }
    });

    let r = ui.checkbox(&mut face, tl!("Face Detection (protect skin tones)"));
    app.auto.add("lumetri.match.faceDetection", r.rect, "Face Detection");

    ui.data_mut(|d| {
        d.insert_temp(key, tc);
        d.insert_temp(fd_key, face);
    });
}

#[cfg(test)]
mod tests {
    use super::without_point;

    #[test]
    fn a_curve_point_is_removed_by_position_not_by_list_order() {
        // #1658: added points are appended, so the newest one is last in the list
        let pts = [[0.0, 0.0], [1.0, 1.0], [0.5, 0.7]];
        assert_eq!(without_point(&pts, 2, false), Some(vec![[0.0, 0.0], [1.0, 1.0]]), "the newest point can go");
        assert_eq!(without_point(&pts, 1, false), None, "the right end stays");
        assert_eq!(without_point(&pts, 0, false), None, "the left end stays");
        // hue curves wrap around: any point can go
        assert_eq!(without_point(&pts, 0, true), Some(vec![[1.0, 1.0], [0.5, 0.7]]));
        assert_eq!(without_point(&pts, 7, false), None);
    }
}
