//! Color Grading panel: Basic Correction, Creative, Curves (RGB + hue curves), Color Wheels,
//! HSL Secondary, Vignette — editing the selected clip's Color Grading effect through engine commands.
//!
//! In an HDR sequence (Rec. 2100 PQ / HLG working space) Basic Correction shows HDR White and
//! HDR Specular and Curves shows HDR Range (cd/m²): the sliders and curves then span 0 … that
//! many nits (see `filmcraft_color::grade`). Automation ids for those and HSL Secondary ▸ Refine:
//! `lumetri.param.<id>` (`hdr_white`, `hdr_specular`, `curves_hdr_range`, `hsl_denoise`,
//! `hsl_blur`); Color Match: `lumetri.match.comparisonView` (toggles the Program monitor's
//! Comparison View, whose reference frame Apply Match then uses).

mod curves;
mod knobs;
mod match_view;
mod toolbar;
mod wheels;

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{ClipId, EffectInstance, ParamValue};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::native_dialogs::FileDialog;
use crate::theme::Tokens;

pub(crate) use match_view::{LutPick, lut_menu};

type Actions = Vec<Value>;

pub(super) struct ColorPanelCtx<'a> {
    pub e: &'a EffectInstance,
    pub clip: ClipId,
    pub scope_id: u64,
    pub idx: usize,
    pub mt: Tick,
    pub hdr: bool,
    pub t: &'a Tokens,
    pub actions: &'a mut Actions,
    pub lut_actions: &'a mut Vec<(&'static str, Value)>,
    pub mask_actions: &'a mut Vec<(String, Value)>,
    pub reset_sections: &'a mut Vec<&'static str>,
    pub reset_wheels: &'a mut Vec<&'static str>,
    pub auto_rows: &'a mut Vec<(String, Rect, &'static str)>,
    pub sections: &'a mut Vec<&'static str>,
    pub is_secondary: bool,
    pub is_group_mode: bool,
    pub knob_hovered: bool,
}

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
    LumetriToolDef { id: "Curves", icon: "∿", label: "Curves", tooltip: "Custom RGB and Hue/Sat/Lum curves", param: "curves_on", sec: "curves" },
    LumetriToolDef { id: "Power Windows", icon: "⬚", label: "Power Windows", tooltip: "Geometric and Bezier shape masks", param: "masks_on", sec: "masks" },
    LumetriToolDef { id: "HSL Secondary", icon: "✑", label: "Qualifier", tooltip: "HSL secondary color isolator", param: "hsl_on", sec: "hsl" },
    LumetriToolDef { id: "Creative", icon: "🎞", label: "Creative & LUTs", tooltip: "Film looks, LUTs and intensity", param: "creative_on", sec: "creative" },
    LumetriToolDef { id: "Vignette", icon: "◐", label: "Vignette", tooltip: "Edge vignette adjustments", param: "vignette_on", sec: "vignette" },
];

pub(super) fn normalize_tool_id(id: &str) -> &str {
    match id {
        "Power Windows / Máscara" => "Power Windows",
        other => other,
    }
}

impl LumetriToolDef {
    pub fn panel_kind(&self) -> crate::dock::PanelKind {
        match self.id {
            "Basic Correction" => crate::dock::PanelKind::ColorPrimaries,
            "Color Wheels & Match" => crate::dock::PanelKind::ColorWheels,
            "Curves" => crate::dock::PanelKind::ColorCurves,
            "Power Windows" => crate::dock::PanelKind::ColorMasks,
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
        crate::dock::PanelKind::ColorMasks => Some("Power Windows"),
        crate::dock::PanelKind::ColorQualifier => Some("HSL Secondary"),
        crate::dock::PanelKind::ColorCreative => Some("Creative"),
        crate::dock::PanelKind::ColorVignette => Some("Vignette"),
        _ => None,
    }
}

pub fn draw_tool_icon(painter: &egui::Painter, center: Pos2, tool_id: &str, color: Color32) {
    match tool_id {
        "Basic Correction" => {
            for col in 0..3 {
                let x = center.x + (col as f32 - 1.0) * 4.5;
                painter.line_segment([pos2(x, center.y - 5.5), pos2(x, center.y + 5.5)], Stroke::new(1.0, color.gamma_multiply(0.5)));
                let pip_y = center.y
                    + match col {
                        0 => -1.5,
                        1 => 2.0,
                        _ => -0.5,
                    };
                painter.circle_filled(pos2(x, pip_y), 1.6, color);
            }
        }
        "Color Wheels & Match" => {
            painter.circle_stroke(center, 5.5, Stroke::new(1.2, color));
            painter.circle_filled(center, 1.8, color);
        }
        "Curves" => {
            let p0 = pos2(center.x - 5.5, center.y + 4.5);
            let p1 = pos2(center.x - 2.0, center.y + 2.5);
            let p2 = pos2(center.x + 2.0, center.y - 2.5);
            let p3 = pos2(center.x + 5.5, center.y - 4.5);
            painter.line_segment([p0, p1], Stroke::new(1.3, color));
            painter.line_segment([p1, p2], Stroke::new(1.3, color));
            painter.line_segment([p2, p3], Stroke::new(1.3, color));
        }
        "Power Windows" => {
            let m_rect = Rect::from_center_size(center, vec2(11.0, 10.0));
            painter.rect_stroke(m_rect, 2.0, Stroke::new(1.2, color), StrokeKind::Inside);
        }
        "HSL Secondary" => {
            let tip = pos2(center.x - 4.5, center.y + 4.5);
            let top = pos2(center.x + 4.5, center.y - 4.5);
            painter.line_segment([tip, top], Stroke::new(1.5, color));
            painter.circle_filled(tip, 1.3, color);
            painter.circle_filled(top, 2.0, color);
        }
        "Creative" => {
            let f_rect = Rect::from_center_size(center, vec2(12.0, 10.0));
            painter.rect_stroke(f_rect, 1.5, Stroke::new(1.0, color), StrokeKind::Inside);
            painter.line_segment([pos2(f_rect.min.x + 3.0, f_rect.min.y), pos2(f_rect.min.x + 3.0, f_rect.max.y)], Stroke::new(0.8, color.gamma_multiply(0.5)));
            painter.line_segment([pos2(f_rect.max.x - 3.0, f_rect.min.y), pos2(f_rect.max.x - 3.0, f_rect.max.y)], Stroke::new(0.8, color.gamma_multiply(0.5)));
        }
        "Vignette" => {
            painter.circle_stroke(center, 5.5, Stroke::new(1.0, color.gamma_multiply(0.6)));
            painter.circle_filled(center, 3.2, color);
        }
        _ => {
            painter.circle_filled(center, 3.0, color);
        }
    }
}

pub fn get_tool_def(id: &str) -> Option<LumetriToolDef> {
    let norm = normalize_tool_id(id);
    ALL_TOOLS.iter().find(|t| t.id == norm).copied()
}

fn render_paste_all_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, clip: ClipId) {
    let candidates: Vec<ClipId> =
        if !app.ui.selected_color_clips.is_empty() { app.ui.selected_color_clips.clone() } else { app.session.state.selection.clone() };
    let targets: Vec<u64> = app
        .session
        .active_sequence()
        .map(|seq| {
            candidates
                .into_iter()
                .filter(|cid| *cid != clip && seq.video_tracks.iter().any(|tr| !tr.locked && tr.items.iter().any(|it| it.id == *cid)))
                .map(|cid| cid.0)
                .collect()
        })
        .unwrap_or_default();
    if targets.is_empty() {
        return;
    }
    let pa_btn = ui
        .add(egui::Button::new(egui::RichText::new(tl!("Paste All")).small()).frame(false))
        .on_hover_text(tl!("Paste this clip's color grade onto all other selected clips"));
    app.auto.add("lumetri.pasteAll", pa_btn.rect, "Paste Grade to Selected Clips");
    if pa_btn.clicked()
        && let Err(err) = app.session.execute("color.batchPasteGrade", json!({"from": clip.0, "clips": targets}))
    {
        app.ui.status = err.to_string();
    }
}

type NodeStripMeta = (usize, bool, Option<String>, Vec<(&'static str, &'static str)>);

#[allow(clippy::too_many_arguments)]
fn render_node_strip(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    clip: ClipId,
    current_panel: crate::dock::PanelKind,
    t: &crate::theme::Tokens,
    lumetri_effects_len: usize,
    active_node: usize,
    needs_apply_clip: bool,
    node_list_meta: &[NodeStripMeta],
) {
    let rename_key = egui::Id::unique(("lumetri-node-rename", current_panel));
    let rename_buf_key = egui::Id::unique(("lumetri-node-rename-buf", current_panel));
    let mut renaming: Option<(u64, usize)> = ui.data(|d| d.get_temp(rename_key));
    let mut rename_buf: String = ui.data(|d| d.get_temp(rename_buf_key)).unwrap_or_default();
    let mut node_cmd: Option<(&'static str, Value)> = None;

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Node:")).color(t.text_dim).small());
        let add_btn = ui.button(egui::RichText::new("+").small()).on_hover_text(tl!("Add Serial Node (Alt+S)"));
        app.auto.add("lumetri.node.add", add_btn.rect, "Add Serial Node");
        if add_btn.clicked() {
            if let Err(err) = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"})) {
                app.ui.status = err.to_string();
            } else {
                app.ui.active_color_node = lumetri_effects_len;
            }
        }
        let nodes_max_w = ui.available_width().max(40.0);
        egui::ScrollArea::horizontal()
            .auto_shrink([false, true])
            .max_width(nodes_max_w)
            .max_height(26.0)
            .id_salt(("lumetri_nodes_scroll", current_panel))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (n_idx, (fx_i, enabled, fx_label, badges)) in node_list_meta.iter().enumerate() {
                        let fx_i = *fx_i;
                        let enabled = *enabled;
                        let is_curr = n_idx == active_node;
                        let node_num = format!("{:02}", n_idx + 1);
                        let label_key = format!("{}:{n_idx}", clip.0);
                        let custom_name = fx_label.clone().or_else(|| app.ui.color_node_labels.get(&label_key).cloned());
                        let label = custom_name.unwrap_or_else(|| format!("Node {node_num}"));

                        if renaming == Some((clip.0, n_idx)) {
                            let ed = ui.add(egui::TextEdit::singleline(&mut rename_buf).desired_width(84.0));
                            ed.request_focus();
                            if ed.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let trimmed = rename_buf.trim().to_string();
                                app.ui.color_node_labels.remove(&label_key);
                                node_cmd = if needs_apply_clip {
                                    Some(("__apply_and_rename__", json!({"clip": clip.0, "label": trimmed})))
                                } else {
                                    Some(("effects.rename", json!({"clip": clip.0, "index": fx_i, "label": trimmed})))
                                };
                                renaming = None;
                            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                renaming = None;
                            }
                            continue;
                        }

                        let badge_codes: String = badges.iter().take(2).map(|(c, _)| *c).collect::<Vec<_>>().join("·");
                        let status_glyph = if enabled { "✓" } else { "✗" };
                        let text = if badge_codes.is_empty() { format!("{label} {status_glyph}") } else { format!("{label} [{badge_codes}] {status_glyph}") };
                        let mut btn = egui::Button::new(egui::RichText::new(text).small());
                        if is_curr {
                            btn = btn.fill(t.accent.gamma_multiply(0.25)).stroke(egui::Stroke::new(1.0, t.accent));
                        }
                        let mut resp = ui.add(btn);
                        if !badges.is_empty() {
                            let hover = badges.iter().map(|(_, full)| *full).collect::<Vec<_>>().join(", ");
                            resp = resp.on_hover_text(hover);
                        }
                        app.auto.add(&format!("lumetri.node.{n_idx}"), resp.rect, &label);
                        if resp.clicked() {
                            app.ui.active_color_node = n_idx;
                        }
                        if resp.double_clicked() {
                            renaming = Some((clip.0, n_idx));
                            rename_buf = label.clone();
                        }
                        resp.context_menu(|ui| {
                            if ui.button(tl!("Rename")).clicked() {
                                renaming = Some((clip.0, n_idx));
                                rename_buf = label.clone();
                                ui.close();
                            }
                            if !needs_apply_clip {
                                let toggle_lbl = if enabled { tl!("Bypass Node") } else { tl!("Enable Node") };
                                if ui.button(toggle_lbl).clicked() {
                                    node_cmd = Some(("effects.toggleEnabled", json!({"clip": clip.0, "index": fx_i})));
                                    ui.close();
                                }
                                if ui.button(tl!("Reset Node")).clicked() {
                                    node_cmd = Some(("effects.reset", json!({"clip": clip.0, "index": fx_i})));
                                    ui.close();
                                }
                                if node_list_meta.len() > 1 && ui.button(tl!("Delete Node")).clicked() {
                                    node_cmd = Some(("effects.remove", json!({"clip": clip.0, "index": fx_i})));
                                    if app.ui.active_color_node >= node_list_meta.len().saturating_sub(1) {
                                        app.ui.active_color_node = node_list_meta.len().saturating_sub(2);
                                    }
                                    ui.close();
                                }
                            }
                        });
                    }
                });
            });
    });

    ui.data_mut(|d| {
        if let Some(r) = renaming {
            d.insert_temp(rename_key, r);
            d.insert_temp(rename_buf_key, rename_buf);
        } else {
            d.remove::<(u64, usize)>(rename_key);
            d.remove::<String>(rename_buf_key);
        }
    });
    if let Some((cmd, params)) = node_cmd {
        let res = if cmd == "__apply_and_rename__" {
            let lbl = params["label"].as_str().unwrap_or_default().to_string();
            app.session.edit_group("Rename Node", |s| {
                let applied = s.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}))?;
                let new_idx = applied.get("applied").and_then(|a| a.get(0)).and_then(|o| o.get("index")).and_then(|v| v.as_u64()).unwrap_or(0);
                s.execute("effects.rename", json!({"clip": clip.0, "index": new_idx, "label": lbl}))
            })
        } else {
            app.session.execute(cmd, params)
        };
        if let Err(err) = res {
            app.ui.status = err.to_string();
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, current_panel: crate::dock::PanelKind) {
    let t = app.tokens;

    let active_group_info = app.ui.active_color_group.and_then(|gid| {
        let seq = app.session.active_sequence()?;
        let g = seq.color_group(gid)?;
        Some((gid, g.name.clone(), g.color, g.effects.clone()))
    });
    if app.ui.active_color_group.is_some() && active_group_info.is_none() {
        app.ui.active_color_group = None;
    }

    let clip_info = active_color_clip(app);
    let clip_group =
        clip_info.as_ref().and_then(|(cid, _)| app.session.active_sequence().and_then(|s| s.color_group_of(*cid)).map(|g| (g.id, g.name.clone(), g.color)));

    if active_group_info.is_none() && clip_info.is_none() {
        crate::dock::placeholder(ui, rect, &t, tl!("(no clip selected)"));
        return;
    }

    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 4.0))).id_salt(("lumetri2", current_panel)));
    bui.set_clip_rect(rect.intersect(ui.clip_rect()));

    let (clip, scope_id, it_name, mt, idx, e, is_group_mode, group_hdr, lumetri_effects_len, node_list_meta, needs_apply_clip) =
        if let Some((gid, gname, gcol, geffects)) = active_group_info {
            let inst = geffects.iter().find(|fx| fx.effect == "lumetri").cloned().or_else(|| filmcraft_project::find_effect("lumetri").map(|d| d.instance()));
            let Some(inst) = inst else {
                crate::dock::placeholder(ui, rect, &t, tl!("(color grading effect unavailable)"));
                return;
            };
            let grp_badges = crate::panels::color_nodes::detect_node_badges(&inst);
            (ClipId(0), gid.0, gname.clone(), Tick(0), 0usize, inst, true, Some((gid, gname, gcol)), 1usize, vec![(0usize, true, None, grp_badges)], false)
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
                (clip, clip.0, it.name, mt, 0usize, default_inst, false, None, 1usize, vec![(0usize, true, None, Vec::new())], true)
            } else {
                let active_node = app.ui.active_color_node.min(lumetri_effects.len().saturating_sub(1));
                app.ui.active_color_node = active_node;
                let Some(&(idx, e_ref)) = lumetri_effects.get(active_node) else {
                    return;
                };
                let meta =
                    lumetri_effects.iter().map(|&(i, fx)| (i, fx.enabled, fx.label.clone(), crate::panels::color_nodes::detect_node_badges(fx))).collect();
                let len = lumetri_effects.len();
                let cloned = e_ref.clone();
                (clip, clip.0, it.name, mt, idx, cloned, false, None, len, meta, false)
            }
        };
    let active_node = if is_group_mode { 0 } else { app.ui.active_color_node };

    let hdr = app.session.active_sequence().is_some_and(|q| q.settings.color.working.is_hdr());
    let mut auto_rows: Vec<(String, Rect, &'static str)> = Vec::new();
    let mut actions: Actions = Vec::new();

    bui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let fx = ui
                .add(egui::Button::new(egui::RichText::new("fx").italics().color(if e.enabled { t.accent } else { t.text_faint })).frame(false))
                .on_hover_text(tl!("Bypass Color Grading (on/off)"));
            app.auto.add("lumetri.bypass", fx.rect, "Bypass Color Grading");
            if fx.clicked() {
                let res = if let Some((gid, _, _)) = &group_hdr {
                    app.session.execute("color.groupToggleBypass", json!({"group": gid.0}))
                } else if !needs_apply_clip {
                    app.session.execute("effects.toggleEnabled", json!({"clip": clip.0, "index": idx}))
                } else {
                    Ok(Value::Null)
                };
                if let Err(err) = res {
                    app.ui.status = err.to_string();
                }
            }
            let rst = wheels::paint_reset_icon_button(ui, t.text_dim, tl!("Reset all color adjustments"));
            app.auto.add("lumetri.reset", rst.rect, "Reset all color adjustments");
            if rst.clicked() {
                let res = if let Some((gid, _, _)) = &group_hdr {
                    app.session.execute("color.groupReset", json!({"group": gid.0}))
                } else if !needs_apply_clip {
                    app.session.execute("effects.reset", json!({"clip": clip.0, "index": idx}))
                } else {
                    Ok(Value::Null)
                };
                if let Err(err) = res {
                    app.ui.status = err.to_string();
                }
            }
            {
                let grade_clip_id = egui::Id::unique("lumetri-grade-clipboard");
                let has_copied = ui.ctx().data(|d| d.get_temp::<Vec<(String, Value)>>(grade_clip_id)).is_some();
                let paste_btn = ui
                    .add_enabled(has_copied, egui::Button::new(egui::RichText::new(tl!("Paste")).small()).frame(false))
                    .on_hover_text(tl!("Paste copied color grade onto this node"));
                app.auto.add("lumetri.pasteGrade", paste_btn.rect, "Paste Grade");
                if paste_btn.clicked()
                    && let Some(copied) = ui.ctx().data(|d| d.get_temp::<Vec<(String, Value)>>(grade_clip_id))
                {
                    let res = app.session.edit_group("Paste Color Grade", |s| {
                        if let Some((gid, _, _)) = &group_hdr {
                            for (param, val) in &copied {
                                s.execute("color.groupSetParam", json!({"group": gid.0, "param": param, "value": val}))?;
                            }
                        } else {
                            let target = if needs_apply_clip {
                                let applied = s.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}))?;
                                applied.get("applied").and_then(|a| a.get(0)).and_then(|o| o.get("index")).and_then(|v| v.as_u64()).unwrap_or(idx as u64)
                                    as usize
                            } else {
                                idx
                            };
                            for (param, val) in &copied {
                                s.execute("effects.setParam", json!({"clip": clip.0, "effect": target, "param": param, "value": val}))?;
                            }
                        }
                        Ok(Value::Null)
                    });
                    if let Err(err) = res {
                        app.ui.status = err.to_string();
                    }
                }
                let copy_btn =
                    ui.add(egui::Button::new(egui::RichText::new(tl!("Copy")).small()).frame(false)).on_hover_text(tl!("Copy this node's color grade"));
                app.auto.add("lumetri.copyGrade", copy_btn.rect, "Copy Grade");
                if copy_btn.clicked() {
                    let vals: Vec<(String, Value)> = e
                        .params
                        .iter()
                        .filter_map(|(k, p)| {
                            let v = match p.value_at(mt) {
                                ParamValue::Float(x) => json!(x),
                                ParamValue::Bool(b) => json!(b),
                                ParamValue::Choice(c) => json!(c),
                                ParamValue::Text(s) => json!(s),
                                ParamValue::Color(c) => json!(c),
                                ParamValue::Vec2(v) => json!([v.x, v.y]),
                                ParamValue::Curve(pts) => json!(pts),
                                _ => return None,
                            };
                            Some((k.clone(), v))
                        })
                        .collect();
                    ui.ctx().data_mut(|d| d.insert_temp(grade_clip_id, vals));
                }
                if !is_group_mode && !needs_apply_clip {
                    render_paste_all_button(app, ui, clip);
                }
            }
            if let Some((gid, gname, gcol)) = group_hdr.as_ref().or(clip_group.as_ref()) {
                let grp_col = Color32::from_rgb(gcol[0], gcol[1], gcol[2]);
                let g_btn = egui::Button::new(egui::RichText::new(tlf!("Group: {name}", name = gname)).small().color(if is_group_mode {
                    Color32::BLACK
                } else {
                    grp_col
                }))
                .fill(if is_group_mode { grp_col } else { Color32::TRANSPARENT })
                .stroke(egui::Stroke::new(1.0, grp_col));
                let g_resp = ui.add(g_btn);
                app.auto.add("lumetri.mode.group", g_resp.rect, "Group Mode");
                if g_resp.clicked() {
                    app.ui.active_color_group = Some(*gid);
                    app.ui.active_color_node = 0;
                }
                let c_btn = egui::Button::new(egui::RichText::new(tl!("Clip")).small().color(if !is_group_mode { Color32::WHITE } else { t.text_dim }))
                    .fill(if !is_group_mode { t.accent.gamma_multiply(0.35) } else { Color32::TRANSPARENT });
                let c_resp = ui.add(c_btn);
                app.auto.add("lumetri.mode.clip", c_resp.rect, "Clip Mode");
                if c_resp.clicked() {
                    app.ui.active_color_group = None;
                    app.ui.active_color_node = 0;
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let hdr_lbl = if let Some((_, gname, _)) = &group_hdr { tlf!("Group · {name}", name = gname) } else { tlf!("Clip · {name}", name = it_name) };
                ui.add(egui::Label::new(egui::RichText::new(hdr_lbl).strong().color(t.text_dim)).truncate());
            });
        });
    });

    if !is_group_mode {
        render_node_strip(app, &mut bui, clip, current_panel, &t, lumetri_effects_len, active_node, needs_apply_clip, &node_list_meta);
    }

    let mut sections: Vec<&'static str> = Vec::new();
    let mut lut_actions: Vec<(&'static str, Value)> = Vec::new();
    let mut mask_actions: Vec<(String, Value)> = Vec::new();
    let mut reset_sections: Vec<&'static str> = Vec::new();
    let mut reset_wheels: Vec<&'static str> = Vec::new();

    let bar_pos = app.ui.lumetri_bar_pos.clone();
    let panel_w = bui.available_width().max(120.0);
    bui.set_max_width(panel_w);
    match bar_pos.as_str() {
        "bottom" => {
            let content_h = (bui.available_height() - 36.0).max(60.0);
            render_scroll_content(
                &mut bui,
                app,
                current_panel,
                panel_w,
                Some(content_h),
                &e,
                clip,
                scope_id,
                idx,
                mt,
                hdr,
                &t,
                &mut actions,
                &mut lut_actions,
                &mut mask_actions,
                &mut reset_sections,
                &mut reset_wheels,
                &mut auto_rows,
                &mut sections,
            );
            bui.add_space(4.0);
            toolbar::render_palette_toolbar(&mut bui, app, &e, current_panel, false, &mut sections);
        }
        "left" => {
            bui.horizontal(|ui| {
                let side_w = 160.0_f32.min((panel_w * 0.35).max(110.0));
                ui.allocate_ui_with_layout(vec2(side_w, ui.available_height()), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(side_w);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .hscroll(false)
                        .max_width(side_w)
                        .id_salt(("lumetri_side_scroll", current_panel))
                        .show(ui, |ui| {
                            toolbar::render_palette_toolbar(ui, app, &e, current_panel, true, &mut sections);
                        });
                });
                ui.separator();
                let main_w = ui.available_width().max(120.0);
                render_scroll_content(
                    ui,
                    app,
                    current_panel,
                    main_w,
                    None,
                    &e,
                    clip,
                    scope_id,
                    idx,
                    mt,
                    hdr,
                    &t,
                    &mut actions,
                    &mut lut_actions,
                    &mut mask_actions,
                    &mut reset_sections,
                    &mut reset_wheels,
                    &mut auto_rows,
                    &mut sections,
                );
            });
        }
        _ => {
            toolbar::render_palette_toolbar(&mut bui, app, &e, current_panel, false, &mut sections);
            bui.add_space(4.0);
            render_scroll_content(
                &mut bui,
                app,
                current_panel,
                panel_w,
                None,
                &e,
                clip,
                scope_id,
                idx,
                mt,
                hdr,
                &t,
                &mut actions,
                &mut lut_actions,
                &mut mask_actions,
                &mut reset_sections,
                &mut reset_wheels,
                &mut auto_rows,
                &mut sections,
            );
        }
    }
    for (id, r, label) in auto_rows {
        if r.is_positive() {
            app.auto.add(&id, r, label);
        }
    }

    // Auto-enable HSL Secondary when user adjusts an HSL key/correction parameter while off
    let hsl_off = e.param("hsl_on").and_then(|p| p.value.as_bool()) == Some(false);
    let mut auto_enable_hsl = false;
    if hsl_off && !reset_sections.contains(&"HSL Secondary") {
        let touches_hsl = actions.iter().any(|a| a["param"].as_str().is_some_and(|p| p.starts_with("hsl_") && p != "hsl_on"));
        if touches_hsl {
            auto_enable_hsl = true;
        }
    }

    let has_clip_edit = !actions.is_empty()
        || !sections.is_empty()
        || !mask_actions.is_empty()
        || lut_actions.iter().any(|(cmd, p)| *cmd != "view.display" && p.get("browse").is_none());

    let mut target_idx = idx;
    let mut applied_clip_effect = false;
    if group_hdr.is_none() && needs_apply_clip && has_clip_edit {
        let mut init_actions = std::mem::take(&mut actions);
        let init_sections = std::mem::take(&mut sections);
        let mut init_masks = std::mem::take(&mut mask_actions);
        let mut deferred_luts = Vec::new();
        let mut init_luts = Vec::new();
        for (cmd, p) in std::mem::take(&mut lut_actions) {
            if cmd == "view.display" || p.get("browse").is_some() {
                deferred_luts.push((cmd, p));
            } else {
                init_luts.push((cmd, p));
            }
        }
        lut_actions = deferred_luts;

        let res = app.session.edit_group("Color Grading", |s| {
            let applied = s.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}))?;
            let new_idx = applied.get("applied").and_then(|a| a.get(0)).and_then(|o| o.get("index")).and_then(|v| v.as_u64()).unwrap_or(idx as u64);
            target_idx = new_idx as usize;
            if auto_enable_hsl {
                s.execute("effects.setParam", json!({"clip": clip.0, "effect": target_idx, "param": "hsl_on", "value": true}))?;
            }
            for mut a in init_actions.drain(..) {
                a["effect"] = json!(new_idx);
                s.execute("effects.setParam", a)?;
            }
            for sec in init_sections {
                s.execute("lumetri.setSection", json!({"clip": clip.0, "effect": target_idx, "section": sec}))?;
            }
            for (cmd, mut p) in init_luts {
                p["clip"] = json!(clip.0);
                p["effect"] = json!(target_idx);
                s.execute(cmd, p)?;
            }
            for (cmd, mut val) in init_masks.drain(..) {
                val["effect"] = json!(new_idx);
                s.execute(&cmd, val)?;
            }
            Ok(Value::Null)
        });
        if let Err(err) = res {
            app.ui.status = err.to_string();
        } else {
            app.ui.active_color_node = 0;
            applied_clip_effect = true;
        }
        auto_enable_hsl = false;
    }

    if auto_enable_hsl && !actions.is_empty() {
        let res = app.session.edit_group("HSL Secondary", |s| {
            if let Some((gid, _, _)) = &group_hdr {
                s.execute("color.groupSetParam", json!({"group": gid.0, "param": "hsl_on", "value": true}))?;
                for a in actions.drain(..) {
                    s.execute("color.groupSetParam", json!({"group": gid.0, "param": a["param"], "value": a["value"]}))?;
                }
            } else {
                s.execute("effects.setParam", json!({"clip": clip.0, "effect": target_idx, "param": "hsl_on", "value": true}))?;
                for mut a in actions.drain(..) {
                    a["effect"] = json!(target_idx);
                    a["merge"] = json!(false);
                    s.execute("effects.setParam", a)?;
                }
            }
            Ok(Value::Null)
        });
        if let Err(err) = res {
            app.ui.status = err.to_string();
        }
    }

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
                        "merge": a.get("merge").cloned().unwrap_or(json!(false)),
                        "begin": a.get("begin").cloned().unwrap_or(json!(false)),
                    }),
                )
            })
            .collect();
        crate::panels::effect_controls::run(app, ui.ctx(), mapped);
        for sec in sections {
            if let Err(err) = app.session.execute("lumetri.setSection", json!({"group": gid.0, "section": sec})) {
                app.ui.status = err.to_string();
            }
        }
    } else {
        crate::panels::effect_controls::run(app, ui.ctx(), actions.into_iter().map(|a| ("effects.setParam".to_string(), a)).collect());
        for sec in sections {
            if let Err(err) = app.session.execute("lumetri.setSection", json!({"clip": clip.0, "effect": target_idx, "section": sec})) {
                app.ui.status = err.to_string();
            }
        }
    }

    if (group_hdr.is_some() || !needs_apply_clip || applied_clip_effect) && (!reset_wheels.is_empty() || !reset_sections.is_empty()) {
        let group_id = group_hdr.as_ref().map(|(gid, _, _)| gid.0);
        apply_color_resets(app, clip, target_idx, group_id, e.masks.len(), &reset_wheels, &reset_sections);
    }

    dispatch_lut_and_mask_actions(
        app,
        clip,
        target_idx,
        group_hdr.as_ref().map(|(gid, _, _)| gid.0),
        needs_apply_clip && !applied_clip_effect,
        lut_actions,
        mask_actions,
    );
}

fn dispatch_lut_and_mask_actions(
    app: &mut FilmcraftApp,
    clip: ClipId,
    target_idx: usize,
    group_id: Option<u64>,
    omit_effect_on_browse: bool,
    lut_actions: Vec<(&'static str, Value)>,
    mask_actions: Vec<(String, Value)>,
) {
    for (cmd, mut p) in lut_actions {
        if cmd == "view.display" {
            let d = p["display"].as_str().unwrap_or("composite").to_string();
            if let Some(Err(err)) = crate::panels::monitor_view::route(app, &format!("view.display.{d}"), &json!({"monitor": "program"})) {
                app.ui.status = err;
            }
            continue;
        }
        if p.get("browse").is_some() {
            let cmd = cmd.to_string();
            let clip_id = clip.0;
            let pass_effect = !omit_effect_on_browse;
            app.pick_ui(FileDialog::open_file(tl!("LUT"), &["cube", "3dl"]), move |app, paths| {
                let Some(path) = paths.into_iter().next() else { return };
                let payload = match group_id {
                    Some(gid) => json!({"path": path, "group": gid}),
                    None if pass_effect => json!({"path": path, "clip": clip_id, "effect": target_idx}),
                    None => json!({"path": path, "clip": clip_id}),
                };
                if let Err(err) = app.session.execute(&cmd, payload) {
                    app.ui.status = err.to_string();
                }
            });
            continue;
        }
        if let Some(gid) = group_id {
            p["group"] = json!(gid);
        } else {
            p["clip"] = json!(clip.0);
            p["effect"] = json!(target_idx);
        }
        if let Err(err) = app.session.execute(cmd, p) {
            app.ui.status = err.to_string();
        }
    }
    if group_id.is_none() {
        for (cmd, val) in mask_actions {
            if let Err(err) = app.session.execute(&cmd, val) {
                app.ui.status = err.to_string();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_scroll_content(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    current_panel: crate::dock::PanelKind,
    max_w: f32,
    max_h: Option<f32>,
    e: &EffectInstance,
    clip: ClipId,
    scope_id: u64,
    idx: usize,
    mt: Tick,
    hdr: bool,
    t: &Tokens,
    actions: &mut Actions,
    lut_actions: &mut Vec<(&'static str, Value)>,
    mask_actions: &mut Vec<(String, Value)>,
    reset_sections: &mut Vec<&'static str>,
    reset_wheels: &mut Vec<&'static str>,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
    sections: &mut Vec<&'static str>,
) {
    let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]).hscroll(false).max_width(max_w).id_salt(("lumetri_scroll", current_panel));
    if let Some(h) = max_h {
        sa = sa.max_height(h);
    }
    sa.show(ui, |ui| {
        let inner_w = ui.available_width().min(max_w).max(100.0);
        ui.set_max_width(inner_w);
        let mut ctx = ColorPanelCtx {
            e,
            clip,
            scope_id,
            idx,
            mt,
            hdr,
            t,
            actions,
            lut_actions,
            mask_actions,
            reset_sections,
            reset_wheels,
            auto_rows,
            sections,
            is_secondary: false,
            is_group_mode: app.ui.active_color_group.is_some(),
            knob_hovered: false,
        };
        render_palette_content(ui, app, current_panel, &mut ctx);
        ui.add_space(12.0);
    });
}

fn render_palette_content(ui: &mut egui::Ui, app: &mut FilmcraftApp, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let active_norm = normalize_tool_id(&app.ui.lumetri_active_tool);
    let active = if get_tool_def(active_norm).is_some() { active_norm.to_string() } else { "Basic Correction".to_string() };

    if let Some(forced) = tool_for_panel(current_panel) {
        render_single_palette(forced, current_panel, ui, app, ctx);
        return;
    }

    if app.ui.lumetri_dual_view {
        let sec_norm = normalize_tool_id(&app.ui.lumetri_secondary_tool);
        let secondary = if sec_norm != active && get_tool_def(sec_norm).is_some() {
            sec_norm.to_string()
        } else if active == "Curves" {
            "Color Wheels & Match".to_string()
        } else {
            "Curves".to_string()
        };
        if ui.available_width() >= 560.0 {
            ui.columns(2, |cols| {
                let col0_w = cols[0].available_width().max(100.0);
                cols[0].set_max_width(col0_w);
                cols[0].vertical(|ui| {
                    ui.set_max_width(col0_w);
                    ctx.is_secondary = false;
                    render_single_palette(&active, current_panel, ui, app, ctx);
                });
                let col1_w = cols[1].available_width().max(100.0);
                cols[1].set_max_width(col1_w);
                cols[1].vertical(|ui| {
                    ui.set_max_width(col1_w);
                    ctx.is_secondary = true;
                    let mut saved = std::mem::take(ctx.auto_rows);
                    render_single_palette(&secondary, current_panel, ui, app, ctx);
                    for (id, r, lbl) in ctx.auto_rows.drain(..) {
                        saved.push((id.replacen("lumetri.", "lumetri.secondary.", 1), r, lbl));
                    }
                    *ctx.auto_rows = saved;
                });
            });
        } else {
            ctx.is_secondary = false;
            render_single_palette(&active, current_panel, ui, app, ctx);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ctx.is_secondary = true;
            let mut saved = std::mem::take(ctx.auto_rows);
            render_single_palette(&secondary, current_panel, ui, app, ctx);
            for (id, r, lbl) in ctx.auto_rows.drain(..) {
                saved.push((id.replacen("lumetri.", "lumetri.secondary.", 1), r, lbl));
            }
            *ctx.auto_rows = saved;
        }
        ctx.is_secondary = false;
    } else {
        ctx.is_secondary = false;
        render_single_palette(&active, current_panel, ui, app, ctx);
    }
}

fn render_single_palette(tool_id: &str, current_panel: crate::dock::PanelKind, ui: &mut egui::Ui, app: &mut FilmcraftApp, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let Some(def) = get_tool_def(tool_id) else { return };

    let on = ctx.e.param(def.param).and_then(|p| p.value.as_bool()).unwrap_or(def.param != "hsl_on");
    let primaries_mode_id = egui::Id::unique(("lumetri_primaries_view_mode", current_panel, ctx.is_secondary));
    let hdr_avail_w = ui.available_width().max(120.0);
    let inline_modes = def.id == "Basic Correction" && hdr_avail_w >= 540.0;
    let inline_lut = def.id == "Basic Correction" && hdr_avail_w >= 840.0;
    let mut mode = if def.id == "Basic Correction" { ui.data(|d| d.get_temp::<u8>(primaries_mode_id).unwrap_or(0)) } else { 0 };
    let draw_modes = |ui: &mut egui::Ui, mode: &mut u8, auto_rows: &mut Vec<(String, Rect, &'static str)>| {
        let r_sl = ui.selectable_label(*mode == 2, tl!("Sliders")).on_hover_text(tl!("Color and Light Sliders"));
        auto_rows.push(("lumetri.mode.sliders".to_string(), r_sl.rect, "Sliders"));
        if r_sl.clicked() {
            *mode = 2;
        }
        let r_kn = ui.selectable_label(*mode == 1, tl!("Knobs")).on_hover_text(tl!("Full knobs view"));
        auto_rows.push(("lumetri.mode.knobs".to_string(), r_kn.rect, "Knobs"));
        if r_kn.clicked() {
            *mode = 1;
        }
        let r_wh = ui.selectable_label(*mode == 0, tl!("Color Wheels")).on_hover_text(tl!("Lift / Gamma / Gain / Offset wheels + Primaries"));
        auto_rows.push(("lumetri.mode.wheels".to_string(), r_wh.rect, "Color Wheels"));
        if r_wh.clicked() {
            *mode = 0;
        }
    };
    ui.allocate_ui_with_layout(vec2(hdr_avail_w, 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.set_max_width(hdr_avail_w);
        let (sw, sw_resp) = ui.allocate_exact_size(vec2(26.0, 14.0), Sense::click());
        ui.painter().rect_filled(sw, 7.0, if on { t.accent } else { Color32::from_rgb(0x55, 0x55, 0x55) });
        ui.painter().circle_filled(pos2(if on { sw.max.x - 7.0 } else { sw.min.x + 7.0 }, sw.center().y), 5.0, Color32::WHITE);
        if sw_resp.clicked() {
            ctx.sections.push(def.sec);
        }
        ctx.auto_rows.push((format!("lumetri.switch.{}", def.id), sw.expand(3.0), def.id));
        if def.id == "Power Windows" {
            ctx.auto_rows.push(("lumetri.switch.Masks".to_string(), sw.expand(3.0), "Masks on/off"));
        }
        let rst_sec = wheels::paint_reset_icon_button(ui, t.text_dim, tl!("Reset this section"));
        ctx.auto_rows.push((format!("lumetri.section_reset.{}", def.sec), rst_sec.rect, "Reset section"));
        if rst_sec.clicked() {
            ctx.reset_sections.push(def.id);
        }
        if inline_modes {
            draw_modes(ui, &mut mode, ctx.auto_rows);
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
                let cur = match ctx.e.param("input_lut").map(|p| &p.value) {
                    Some(ParamValue::Text(s)) => s.clone(),
                    _ => String::new(),
                };
                let lut_w = 160.0_f32.min((ui.available_width() - 8.0).max(60.0));
                let auto_id = if ctx.is_secondary { "lumetri.secondary.input_lut" } else { "lumetri.input_lut" };
                let (r, pick) = lut_menu(ui, app, ("lumetri-lut", current_panel, ctx.is_secondary, "input_lut"), &cur, true, lut_w, auto_id);
                match pick {
                    Some(LutPick::Lut(r)) => ctx.lut_actions.push(("lumetri.setInputLut", json!({"lut": r}))),
                    Some(LutPick::Browse) => ctx.lut_actions.push(("lumetri.setInputLut", json!({"browse": true}))),
                    None => {}
                }
                ctx.auto_rows.push(("lumetri.input_lut".to_string(), r, tl!("Input LUT")));
            }
        });
    });
    if def.id == "Basic Correction" {
        if !inline_modes {
            ui.horizontal_wrapped(|ui| {
                draw_modes(ui, &mut mode, ctx.auto_rows);
            });
        }
        ui.data_mut(|d| d.insert_temp(primaries_mode_id, mode));
    }
    ui.add_space(1.0);

    match def.id {
        "Basic Correction" => {
            let mode = ui.data(|d| d.get_temp::<u8>(primaries_mode_id).unwrap_or(0));
            if !inline_lut {
                match_view::lut_combo(ui, app, current_panel, ctx.is_secondary, ctx.e, "input_lut", tl!("Input LUT"), true, ctx.lut_actions, ctx.auto_rows);
                ui.add_space(2.0);
            }
            if !ctx.is_group_mode {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!("WB Selector:")).color(t.text_dim).small());
                    let armed = app
                        .ui
                        .eyedropper
                        .as_ref()
                        .is_some_and(|ed| ed.clip == ctx.clip.0 && ed.effect == ctx.idx && ed.kind == crate::state::EyedropperKind::LumetriWb);
                    let drop = crate::icons::button(
                        ui,
                        crate::icons::Icon::Eyedropper,
                        16.0,
                        armed,
                        t,
                        tl!("White Balance Eyedropper (click neutral grey/white in Program monitor)"),
                    );
                    ctx.auto_rows.push(("lumetri.eyedropper.wb".to_string(), drop.rect, "WB Eyedropper"));
                    if drop.clicked() {
                        arm_lumetri_eyedropper(app, ctx, "temperature", crate::state::EyedropperKind::LumetriWb);
                    }
                });
            }

            match mode {
                0 => {
                    wheels::resolve_wheels_grid(ui, current_panel, ctx);
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);
                    knobs::draw_primary_knobs_strip(ui, current_panel, true, ctx);
                }
                1 => {
                    sub(ui, t, tl!("Primary Adjustments"));
                    knobs::draw_primary_knobs_strip(ui, current_panel, true, ctx);
                }
                _ => {
                    ui.columns(2, |cols| {
                        cols[0].vertical(|ui| {
                            sub(ui, t, tl!("Color"));
                            for (id, label, grad) in [
                                ("temperature", tl!("Temperature"), Some(Gradient::Temp)),
                                ("tint", tl!("Tint"), Some(Gradient::Tint)),
                                ("saturation", tl!("Saturation"), None),
                            ] {
                                let r = slider(ui, current_panel, id, label, grad, ctx);
                                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
                            }
                        });
                        cols[1].vertical(|ui| {
                            sub(ui, t, tl!("Light"));
                            for (id, label) in [
                                ("exposure", tl!("Exposure")),
                                ("contrast", tl!("Contrast")),
                                ("highlights", tl!("Highlights")),
                                ("shadows", tl!("Shadows")),
                                ("whites", tl!("Whites")),
                                ("blacks", tl!("Blacks")),
                            ] {
                                let r = slider(ui, current_panel, id, label, None, ctx);
                                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
                            }
                        });
                    });
                }
            }

            if ctx.hdr {
                sub(ui, t, tl!("HDR"));
                for (id, label) in [("hdr_white", tl!("HDR White")), ("hdr_specular", tl!("HDR Specular"))] {
                    let r = slider(ui, current_panel, id, label, None, ctx);
                    ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
                }
            }
        }
        "Creative" => {
            let look = match ctx.e.param("look").map(|p| &p.value) {
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
                let cb = craft_ui::ComboBox::from_id_salt(("lumetri-look", current_panel, ctx.is_secondary, ctx.is_group_mode, ctx.scope_id, ctx.idx))
                    .selected_text(opts.get(look).map_or(tl!("None"), |o| crate::i18n::t(o)))
                    .width((ui.available_width() - 40.0).max(60.0))
                    .show_ui(ui, |ui| {
                        for (i, o) in opts.iter().enumerate() {
                            if ui.selectable_value(&mut sel, i, crate::i18n::t(o)).changed() {
                                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "look", "value": i}));
                            }
                        }
                    });
                ctx.auto_rows.push(("lumetri.param.look".into(), cb.response.rect, "Look"));
            });
            match_view::lut_combo(ui, app, current_panel, ctx.is_secondary, ctx.e, "look_lut", tl!("Look LUT"), false, ctx.lut_actions, ctx.auto_rows);
            let r_int = slider(ui, current_panel, "look_intensity", tl!("Intensity"), None, ctx);
            ctx.auto_rows.push(("lumetri.param.look_intensity".into(), r_int, "Intensity"));
            sub(ui, t, tl!("Adjustments"));
            for (id, label) in
                [("faded_film", tl!("Faded Film")), ("sharpen", tl!("Sharpen")), ("vibrance", tl!("Vibrance")), ("creative_sat", tl!("Saturation"))]
            {
                let r = slider(ui, current_panel, id, label, None, ctx);
                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
            }
            sub(ui, t, tl!("Tint Wheels"));
            for (id, label) in [("shadow_tint", tl!("Shadow Tint")), ("highlight_tint", tl!("Highlight Tint"))] {
                let r = color_row(ui, app, id, label, ctx);
                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
            }
        }
        "Curves" => {
            let tab_key = egui::Id::unique(("lumetri-curve-tab", current_panel, ctx.is_secondary));
            let mut curve_tab: usize = ui.data(|d| d.get_temp(tab_key)).unwrap_or(0);
            let curve_tabs = [
                ("Custom RGB", false, ""),
                ("Hue vs Hue", true, "hue_vs_hue"),
                ("Hue vs Sat", true, "hue_vs_sat"),
                ("Hue vs Lum", true, "hue_vs_luma"),
                ("Lum vs Sat", true, "luma_vs_sat"),
                ("Sat vs Sat", true, "sat_vs_sat"),
            ];
            let key = egui::Id::unique(("lumetri-curve-channel", current_panel, ctx.is_secondary));
            let ch_raw: usize = ui.data(|d| d.get_temp(key)).unwrap_or(0);
            let mut ch = ch_raw.min(3);
            let channels = [
                ("curve_luma", Color32::WHITE),
                ("curve_red", Color32::from_rgb(230, 70, 70)),
                ("curve_green", Color32::from_rgb(80, 200, 90)),
                ("curve_blue", Color32::from_rgb(80, 130, 250)),
            ];
            let (id, col) = channels.get(ch).copied().unwrap_or(("curve_luma", Color32::WHITE));

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (ti, (lbl, _, _)) in curve_tabs.iter().enumerate() {
                    let tab_resp = ui.selectable_label(curve_tab == ti, egui::RichText::new(crate::i18n::t(lbl)).small());
                    ctx.auto_rows.push((format!("lumetri.curve_tab.{ti}"), tab_resp.rect, *lbl));
                    if tab_resp.clicked() {
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
                        ctx.auto_rows.push((format!("lumetri.curve_channel.{cname}"), r, *cname));
                        if resp.on_hover_text(*cname).clicked() {
                            ch = i;
                        }
                    }
                    let rst_resp = wheels::paint_reset_icon_button(ui, t.text_dim, tl!("Reset curve"));
                    ctx.auto_rows.push(("lumetri.curve.reset".into(), rst_resp.rect, "Reset curve"));
                    if rst_resp.clicked() {
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id, "value": [[0.0, 0.0], [1.0, 1.0]]}));
                    }
                    if !ctx.is_group_mode {
                        let armed = app.ui.eyedropper.as_ref().is_some_and(|ed| {
                            ed.clip == ctx.clip.0 && ed.effect == ctx.idx && ed.kind == crate::state::EyedropperKind::LumetriCurve(id.to_string())
                        });
                        let drop =
                            crate::icons::button(ui, crate::icons::Icon::Eyedropper, 16.0, armed, t, tl!("Curve Eyedropper (pick point from Program monitor)"));
                        ctx.auto_rows.push(("lumetri.eyedropper.curve".into(), drop.rect, "Curve Eyedropper"));
                        if drop.clicked() {
                            arm_lumetri_eyedropper(app, ctx, id, crate::state::EyedropperKind::LumetriCurve(id.to_string()));
                        }
                    }
                } else {
                    let (_, _, cid) = curve_tabs.get(curve_tab).copied().unwrap_or(("", true, "hue_vs_hue"));
                    let rst_resp = wheels::paint_reset_icon_button(ui, t.text_dim, tl!("Reset curve"));
                    ctx.auto_rows.push(("lumetri.curve.reset".into(), rst_resp.rect, "Reset curve"));
                    if rst_resp.clicked() {
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": cid, "value": []}));
                    }
                    if !ctx.is_group_mode {
                        let armed = app.ui.eyedropper.as_ref().is_some_and(|ed| {
                            ed.clip == ctx.clip.0 && ed.effect == ctx.idx && ed.kind == crate::state::EyedropperKind::LumetriCurve(cid.to_string())
                        });
                        let drop =
                            crate::icons::button(ui, crate::icons::Icon::Eyedropper, 16.0, armed, t, tl!("Curve Eyedropper (pick point from Program monitor)"));
                        ctx.auto_rows.push(("lumetri.eyedropper.curve".into(), drop.rect, "Curve Eyedropper"));
                        if drop.clicked() {
                            arm_lumetri_eyedropper(app, ctx, cid, crate::state::EyedropperKind::LumetriCurve(cid.to_string()));
                        }
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_key, curve_tab));
            ui.data_mut(|d| d.insert_temp(key, ch));

            if curve_tab == 0 {
                let hist = curve_histogram(app, ui);
                let r = curves::curve_editor(ui, current_panel, id, false, col, hist.as_ref(), ctx);
                ctx.auto_rows.push((format!("lumetri.curve.{id}"), r, "Curve"));
                if ctx.hdr {
                    let r = slider(ui, current_panel, "curves_hdr_range", tl!("HDR Range"), None, ctx);
                    ctx.auto_rows.push(("lumetri.param.curves_hdr_range".into(), r, "HDR Range"));
                }
            } else {
                let (lbl, _, cid) = curve_tabs.get(curve_tab).copied().unwrap_or(("Hue vs Hue", true, "hue_vs_hue"));
                let r = curves::curve_editor(ui, current_panel, cid, true, Color32::WHITE, None, ctx);
                ctx.auto_rows.push((format!("lumetri.curve.{cid}"), r, lbl));
            }
        }
        "Color Wheels & Match" => {
            if ui.available_width() >= 920.0 {
                ui.columns(2, |cols| {
                    cols[0].vertical(|ui| {
                        wheels::resolve_wheels_grid(ui, current_panel, ctx);
                    });
                    cols[1].vertical(|ui| {
                        match_view::match_controls(ui, app, current_panel, ctx.lut_actions, ctx.auto_rows);
                    });
                });
            } else {
                wheels::resolve_wheels_grid(ui, current_panel, ctx);
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                match_view::match_controls(ui, app, current_panel, ctx.lut_actions, ctx.auto_rows);
            }
        }
        "Power Windows" => {
            if app.ui.active_color_group.is_some() {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(tl!("Power Windows are only available in Clip mode.")).color(t.text_dim).small());
                    if ui.button(egui::RichText::new(tl!("Switch to Clip Mode")).small()).clicked() {
                        app.ui.active_color_group = None;
                        app.ui.active_color_node = 0;
                    }
                });
                return;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(tl!("Tools:")).color(t.text_dim).small());
                let pen_on = app.ui.mask_pen.as_ref().is_some_and(|p| p.clip == ctx.clip.0 && p.effect == ctx.idx);

                let rect_btn = ui.button(egui::RichText::new(tl!("Linear")).small()).on_hover_text(tl!("Add rectangular mask (Linear Window)"));
                ctx.auto_rows.push(("lumetri.mask.add_linear".into(), rect_btn.rect, "Add Linear Window"));
                if rect_btn.clicked() {
                    ctx.mask_actions.push(("masks.add".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "shape": "polygon"})));
                }

                let circle_btn = ui.button(egui::RichText::new(tl!("Circle")).small()).on_hover_text(tl!("Add circular mask (Circle Window)"));
                ctx.auto_rows.push(("lumetri.mask.add_circle".into(), circle_btn.rect, "Add Circle Window"));
                if circle_btn.clicked() {
                    ctx.mask_actions.push(("masks.add".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "shape": "ellipse"})));
                }

                let mut pen_btn = egui::Button::new(egui::RichText::new(tl!("Pen")).small());
                if pen_on {
                    pen_btn = pen_btn.fill(t.accent.gamma_multiply(0.25)).stroke(egui::Stroke::new(1.0, t.accent));
                }
                let pen_resp = ui.add(pen_btn).on_hover_text(tl!("Freehand Bezier curve drawing on monitor"));
                ctx.auto_rows.push(("lumetri.mask.add_pen".into(), pen_resp.rect, "Draw Mask with Pen"));
                if pen_resp.clicked() {
                    if pen_on {
                        app.ui.mask_pen = None;
                    } else if let Some(target_idx) = ensure_lumetri_effect(app, ctx) {
                        app.ui.mask_pen = Some(crate::state::MaskPenDraft { clip: ctx.clip.0, effect: target_idx, points: vec![] });
                        app.ui.status = tl!("Click on the monitor to trace the mask; click the first point to close").into();
                    }
                }
                if pen_on && ui.button(egui::RichText::new(tl!("Cancel")).small()).clicked() {
                    app.ui.mask_pen = None;
                }
            });

            if ctx.e.masks.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tl!("No masks on this node. Click Linear, Circle, or Pen to create one.")).color(t.text_dim).small());
            } else {
                let sel_mask_idx = app
                    .session
                    .state
                    .selected_mask
                    .filter(|s| s.clip == ctx.clip && s.effect == ctx.idx)
                    .map(|s| s.mask.min(ctx.e.masks.len().saturating_sub(1)))
                    .unwrap_or(0);

                let Some(m) = ctx.e.masks.get(sel_mask_idx) else {
                    return;
                };
                ui.horizontal_wrapped(|ui| {
                    for (mi, mk) in ctx.e.masks.iter().enumerate() {
                        let mpath = mk.path_at(ctx.mt);
                        let shape_tag = if !mpath.vertices.is_empty() && mpath.vertices.iter().all(|v| !v.is_corner()) {
                            "[C]"
                        } else if mpath.vertices.len() == 4 {
                            "[L]"
                        } else {
                            "[P]"
                        };
                        let is_sel = mi == sel_mask_idx;
                        let visible = mk.mode != filmcraft_project::MaskMode::None;
                        let mut chip_btn = egui::Button::new(egui::RichText::new(format!("{shape_tag} {}", mk.name)).small());
                        if is_sel {
                            chip_btn = chip_btn.fill(t.accent.gamma_multiply(0.25)).stroke(egui::Stroke::new(1.0, t.accent));
                        }
                        let chip_resp = ui.add(chip_btn).on_hover_text(tl!("Select power window"));
                        ctx.auto_rows.push((format!("lumetri.mask.chip.{mi}"), chip_resp.rect, "Select Mask Chip"));
                        if chip_resp.clicked() {
                            ctx.mask_actions.push(("masks.select".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": mi})));
                        }
                        let vis_lbl = if visible { "✓" } else { "✗" };
                        let vis_col = if visible { t.accent } else { t.text_faint };
                        let vis_resp = ui.add(egui::Button::new(egui::RichText::new(vis_lbl).small().color(vis_col)).frame(false)).on_hover_text(if visible {
                            tl!("Disable mask (Mode: Off)")
                        } else {
                            tl!("Enable mask (Mode: Add)")
                        });
                        ctx.auto_rows.push((format!("lumetri.mask.vis.{mi}"), vis_resp.rect, "Toggle Mask Visibility"));
                        if vis_resp.clicked() {
                            let next_mode = if visible { "none" } else { "add" };
                            ctx.mask_actions.push(("masks.set".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": mi, "mode": next_mode})));
                        }
                        let del_chip =
                            ui.add(egui::Button::new(egui::RichText::new("x").small().color(t.text_dim)).frame(false)).on_hover_text(tl!("Delete mask"));
                        ctx.auto_rows.push((format!("lumetri.mask.chip_del.{mi}"), del_chip.rect, "Delete Mask Chip"));
                        if del_chip.clicked() {
                            ctx.mask_actions.push(("masks.remove".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": mi})));
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(tl!("Window:")).color(t.text_dim).small());
                    let mut chosen_mask = sel_mask_idx;
                    let mask_cb = craft_ui::ComboBox::from_id_salt(("lumetri_mask_select", current_panel, ctx.is_secondary, ctx.idx))
                        .selected_text(format!("{} ({})", m.name, sel_mask_idx + 1))
                        .show_ui(ui, |ui| {
                            for (mi, mk) in ctx.e.masks.iter().enumerate() {
                                if ui.selectable_value(&mut chosen_mask, mi, format!("{} ({})", mk.name, mi + 1)).clicked() {
                                    ctx.mask_actions.push(("masks.select".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": mi})));
                                }
                            }
                        });
                    ctx.auto_rows.push(("lumetri.mask.select".into(), mask_cb.response.rect, "Select Mask"));

                    let mut inv = m.inverted;
                    let inv_resp = ui.selectable_label(inv, tl!("Invert")).on_hover_text(tl!("Invert mask"));
                    ctx.auto_rows.push(("lumetri.mask.invert".into(), inv_resp.rect, "Invert Mask"));
                    if inv_resp.clicked() {
                        inv = !inv;
                        ctx.mask_actions.push(("masks.set".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": sel_mask_idx, "inverted": inv})));
                    }

                    let modes = [
                        (filmcraft_project::MaskMode::Add, "add", tl!("Add")),
                        (filmcraft_project::MaskMode::Subtract, "subtract", tl!("Subtract")),
                        (filmcraft_project::MaskMode::Intersect, "intersect", tl!("Intersect")),
                        (filmcraft_project::MaskMode::None, "none", tl!("Off")),
                    ];
                    let cur_mode_lbl = modes.iter().find(|(md, _, _)| *md == m.mode).map_or(tl!("Add"), |(_, _, l)| *l);
                    let mode_cb = craft_ui::ComboBox::from_id_salt(("lumetri_mask_mode", current_panel, ctx.is_secondary, ctx.idx, sel_mask_idx))
                        .selected_text(cur_mode_lbl)
                        .width(82.0)
                        .show_ui(ui, |ui| {
                            for (md, mname, mlbl) in modes {
                                if ui.selectable_label(m.mode == md, mlbl).clicked() {
                                    ctx.mask_actions
                                        .push(("masks.set".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": sel_mask_idx, "mode": mname})));
                                }
                            }
                        });
                    ctx.auto_rows.push(("lumetri.mask.mode".into(), mode_cb.response.rect, "Mask Mode"));

                    let del_resp = ui.button(egui::RichText::new("🗑").color(Color32::from_rgb(240, 100, 100))).on_hover_text(tl!("Delete mask"));
                    ctx.auto_rows.push(("lumetri.mask.delete".into(), del_resp.rect, "Delete Mask"));
                    if del_resp.clicked() {
                        ctx.mask_actions.push(("masks.remove".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": sel_mask_idx})));
                    }
                });

                let cur_feather = m.feather.f64_at(ctx.mt);
                let cur_opacity = m.opacity.f64_at(ctx.mt);
                let cur_expansion = m.expansion.f64_at(ctx.mt);

                ui.horizontal_wrapped(|ui| {
                    mask_drag_param(
                        ui,
                        current_panel,
                        ctx,
                        sel_mask_idx,
                        "feather",
                        tl!("Feather:"),
                        cur_feather,
                        10.0,
                        0.0..=500.0,
                        " px",
                        &m.feather,
                        "lumetri.mask.feather",
                        "Mask Feather",
                    );
                    mask_drag_param(
                        ui,
                        current_panel,
                        ctx,
                        sel_mask_idx,
                        "opacity",
                        tl!("Opacity:"),
                        cur_opacity,
                        100.0,
                        0.0..=100.0,
                        " %",
                        &m.opacity,
                        "lumetri.mask.opacity",
                        "Mask Opacity",
                    );
                    mask_drag_param(
                        ui,
                        current_panel,
                        ctx,
                        sel_mask_idx,
                        "expansion",
                        tl!("Expansion:"),
                        cur_expansion,
                        0.0,
                        -200.0..=200.0,
                        " px",
                        &m.expansion,
                        "lumetri.mask.expansion",
                        "Mask Expansion",
                    );
                });

                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(tl!("Tracker:")).color(t.text_dim).small());
                    let back_resp = ui.button(tl!("◀ Track Back"));
                    ctx.auto_rows.push(("lumetri.mask.track_back".into(), back_resp.rect, "Track Back"));
                    if back_resp.clicked() {
                        ctx.mask_actions.push(("masks.select".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": sel_mask_idx})));
                        ctx.mask_actions
                            .push(("masks.track".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": sel_mask_idx, "direction": "backward"})));
                    }
                    let fwd_resp = ui.button(tl!("Track Forward ▶"));
                    ctx.auto_rows.push(("lumetri.mask.track_forward".into(), fwd_resp.rect, "Track Forward"));
                    if fwd_resp.clicked() {
                        ctx.mask_actions.push(("masks.select".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": sel_mask_idx})));
                        ctx.mask_actions
                            .push(("masks.track".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": sel_mask_idx, "direction": "forward"})));
                    }
                });
            }
        }
        "HSL Secondary" => {
            ui.horizontal(|ui| {
                sub(ui, t, tl!("Key"));
                if !ctx.is_group_mode {
                    let armed = app
                        .ui
                        .eyedropper
                        .as_ref()
                        .is_some_and(|ed| ed.clip == ctx.clip.0 && ed.effect == ctx.idx && ed.kind == crate::state::EyedropperKind::LumetriHsl);
                    let drop = crate::icons::button(
                        ui,
                        crate::icons::Icon::Eyedropper,
                        16.0,
                        armed,
                        t,
                        tl!("Key Eyedropper (pick target color from Program monitor)"),
                    );
                    ctx.auto_rows.push(("lumetri.eyedropper.hsl".into(), drop.rect, "HSL Eyedropper"));
                    if drop.clicked() {
                        arm_lumetri_eyedropper(app, ctx, "hsl_hue", crate::state::EyedropperKind::LumetriHsl);
                    }
                }
            });
            draw_hsl_range_bar(ui, current_panel, ctx);
            let r_hue = slider(ui, current_panel, "hsl_hue", tl!("Hue"), Some(Gradient::Hue), ctx);
            ctx.auto_rows.push(("lumetri.param.hsl_hue".into(), r_hue, "Hue"));
            for (id, label) in [
                ("hsl_hue_range", tl!("Hue Range")),
                ("hsl_sat_min", tl!("Saturation Min")),
                ("hsl_luma_min", tl!("Luma Min")),
                ("hsl_luma_max", tl!("Luma Max")),
                ("hsl_soft", tl!("Soften")),
            ] {
                let r = slider(ui, current_panel, id, label, None, ctx);
                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
            }
            let mask = match ctx.e.param("hsl_show_mask").map(|p| &p.value) {
                Some(ParamValue::Choice(c)) => *c as usize,
                _ => 0,
            };
            ui.horizontal(|ui| {
                ui.add_sized(vec2(110.0, 20.0), egui::Label::new(egui::RichText::new(tl!("Show Mask")).color(t.text_dim)));
                let mut sel = mask;
                let modes = [tl!("Off"), tl!("Color/Gray"), tl!("Color/Black"), tl!("White/Black")];
                let cb = craft_ui::ComboBox::from_id_salt(("hsl-mask", current_panel, ctx.is_secondary, ctx.is_group_mode, ctx.scope_id, ctx.idx))
                    .selected_text(modes.get(mask).copied().unwrap_or(modes[0]))
                    .show_ui(ui, |ui| {
                        for (i, o) in modes.iter().enumerate() {
                            if ui.selectable_value(&mut sel, i, *o).changed() {
                                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "hsl_show_mask", "value": i}));
                            }
                        }
                    });
                ctx.auto_rows.push(("lumetri.param.hsl_show_mask".into(), cb.response.rect, "Show Mask"));
            });
            sub(ui, t, tl!("Refine"));
            for (id, label) in [("hsl_denoise", tl!("Denoise")), ("hsl_blur", tl!("Blur"))] {
                let r = slider(ui, current_panel, id, label, None, ctx);
                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
            }
            sub(ui, t, tl!("Correction"));
            for (id, label, grad) in [
                ("hsl_temp", tl!("Temperature"), Some(Gradient::Temp)),
                ("hsl_tint", tl!("Tint"), Some(Gradient::Tint)),
                ("hsl_sat", tl!("Saturation"), None),
                ("hsl_hue_shift", tl!("Hue Shift"), Some(Gradient::Hue)),
            ] {
                let r = slider(ui, current_panel, id, label, grad, ctx);
                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
            }
        }
        "Vignette" => {
            for (id, label) in [
                ("vignette_amount", tl!("Amount")),
                ("vignette_midpoint", tl!("Midpoint")),
                ("vignette_roundness", tl!("Roundness")),
                ("vignette_feather", tl!("Feather")),
            ] {
                let r = slider(ui, current_panel, id, label, None, ctx);
                ctx.auto_rows.push((format!("lumetri.param.{id}"), r, label));
            }
        }
        _ => {}
    }
}

/// The Color Grading panel's ≡ menu items: Export .cube… saves the grade of the clip (or active
/// Color Group) the panel shows (also an Apply Match result) as a 3D LUT (`lumetri.exportCube`;
/// automation id `panel.menu.LumetriColor.exportCube`).
pub fn panel_menu(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let group_shown = app.ui.active_color_group.filter(|gid| {
        app.session.active_sequence().and_then(|q| q.color_group(*gid)).is_some_and(|g| g.effects.iter().any(|e| e.enabled && e.effect == "lumetri"))
    });
    let clip_shown = if group_shown.is_none() {
        active_color_clip(app).filter(|(_, it)| it.effects.iter().any(|e| e.enabled && e.effect == "lumetri")).map(|(c, _)| c)
    } else {
        None
    };
    let r = ui.add_enabled(group_shown.is_some() || clip_shown.is_some(), egui::Button::new(tl!("Export .cube…")));
    app.auto.add_response("panel.menu.LumetriColor.exportCube", &r, "Export .cube…");
    if r.clicked() {
        let payload = if let Some(gid) = group_shown { Some(json!({"group": gid.0})) } else { clip_shown.map(|clip| json!({"clip": clip.0})) };
        if let Some(payload) = payload {
            if let Err(e) = crate::menus::invoke(app, ui.ctx(), "lumetri.exportCube", payload) {
                app.ui.status = e;
            }
            return true;
        }
    }
    false
}

pub(super) fn sub(ui: &mut egui::Ui, t: &Tokens, name: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(name).color(t.text).strong().size(12.0));
}

#[derive(Clone, Copy)]
enum Gradient {
    Temp,
    Tint,
    Hue,
}

/// Spectrum slider: 1 pt track (or gradient), hollow ring knob, scrubby value at the right.
fn slider(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, id: &str, label: &str, grad: Option<Gradient>, ctx: &mut ColorPanelCtx<'_>) -> Rect {
    let t = ctx.t;
    let Some(pd) = ctx.e.def().and_then(|d| d.param(id)) else { return Rect::NOTHING };
    let filmcraft_project::ParamKind::Float { min, max, soft_min, soft_max, decimals, .. } = pd.kind else { return Rect::NOTHING };
    let v = ctx.e.param(id).map(|p| p.f64_at(ctx.mt)).unwrap_or(pd.default.as_f64().unwrap_or(0.0));
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(pos2(row.min.x + 4.0, row.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text_dim);
    let show_kf = !ctx.is_group_mode && pd.animatable;
    let num_right = if show_kf { row.max.x - 18.0 } else { row.max.x };
    let track = Rect::from_min_max(pos2(row.min.x + 112.0, row.center().y - 1.0), pos2(num_right - 58.0, row.center().y + 1.0));
    let track_w = track.width().max(1.0);
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
                        Color32::from_rgb(
                            (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
                            (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
                            (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
                        )
                    }
                };
                let x0 = track.min.x + track_w * f0;
                ui.painter().rect_filled(Rect::from_min_max(pos2(x0, track.min.y - 1.0), pos2(x0 + track_w / n as f32 + 0.5, track.max.y + 1.0)), 0.0, c);
            }
        }
        None => {
            ui.painter().rect_filled(track, 0.0, Color32::from_rgb(0x6e, 0x6e, 0x6e));
        }
    }
    let soft_span = (soft_max - soft_min).max(1e-6);
    let f = ((v - soft_min) / soft_span).clamp(0.0, 1.0) as f32;
    let kx = track.min.x + f * track_w;
    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, id);
    let hit = Rect::from_min_max(pos2(track.min.x - 6.0, row.min.y), pos2(track.max.x + 6.0, row.max.y));
    let resp = ui.interact(hit, egui::Id::unique(("lslider", scope)), Sense::click_and_drag());
    ui.painter().circle_filled(pos2(kx, track.center().y), 5.0, t.panel_bg);
    ui.painter().circle_stroke(
        pos2(kx, track.center().y),
        5.0,
        Stroke::new(1.5, if resp.dragged() { t.hot_text } else { Color32::from_rgb(0xb0, 0xb0, 0xb0) }),
    );
    let mut out = None;
    let mut merge_undo = false;
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let nf = ((p.x - track.min.x) / track_w).clamp(0.0, 1.0) as f64;
        out = Some(soft_min + nf * soft_span);
        merge_undo = resp.dragged();
    }
    if resp.double_clicked() {
        out = pd.default.as_f64();
        merge_undo = false;
    }
    let mut vui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(num_right - 52.0, row.min.y + 3.0), pos2(num_right, row.max.y - 3.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let (_, nv) = crate::widgets::hot_number(
        &mut vui,
        label,
        egui::Id::unique(("lval", scope)),
        v,
        crate::widgets::HotNumberOptions::new(soft_span / 300.0, (min, max), decimals as usize, "", t),
    );
    if show_kf {
        let kf_center = pos2(row.max.x - 9.0, row.center().y);
        let kf_rect = Rect::from_center_size(kf_center, vec2(14.0, 14.0));
        let prm = ctx.e.param(id);
        let at_key = prm.is_some_and(|p| p.keyframes.iter().any(|k| k.time == ctx.mt));
        let animated = prm.is_some_and(|p| p.is_animated());
        let kf_resp = ui.interact(kf_rect, egui::Id::unique(("lkf", scope)), Sense::click()).on_hover_text(if at_key {
            tl!("Remove keyframe")
        } else {
            tl!("Add/Toggle keyframe")
        });
        let s = 4.2;
        let pts = vec![kf_center + vec2(0.0, -s), kf_center + vec2(s, 0.0), kf_center + vec2(0.0, s), kf_center + vec2(-s, 0.0)];
        if at_key {
            ui.painter().add(egui::Shape::convex_polygon(pts, t.hot_text, Stroke::NONE));
        } else {
            let stroke_col = if animated {
                t.accent
            } else if kf_resp.hovered() {
                t.text
            } else {
                t.text_faint
            };
            ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.2, stroke_col)));
        }
        ctx.auto_rows.push((format!("lumetri.keyframe.{id}"), kf_rect, "Toggle keyframe"));
        if kf_resp.clicked() {
            ctx.mask_actions.push(("effects.addKeyframe".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id})));
        }
    }
    if nv.is_some() && out.is_none() {
        merge_undo = true;
    }
    if let Some(nv) = out.or(nv) {
        let scale = 10f64.powi(decimals as i32);
        let rounded = (nv * scale).round() / scale;
        if (rounded - v).abs() > 1e-5 {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id, "value": rounded, "merge": merge_undo}));
        }
    }
    row
}

fn color_row(ui: &mut egui::Ui, app: &mut FilmcraftApp, id: &str, label: &str, ctx: &mut ColorPanelCtx<'_>) -> Rect {
    let t = ctx.t;
    let Some(pd) = ctx.e.def().and_then(|d| d.param(id)) else { return Rect::NOTHING };
    let default_c = match pd.default {
        ParamValue::Color(c) => c,
        _ => [0.5, 0.5, 0.5, 1.0],
    };
    let c = match ctx.e.param(id).map(|p| p.value_at(ctx.mt)) {
        Some(ParamValue::Color(c)) => c,
        _ => default_c,
    };
    let mut row_rect = Rect::NOTHING;
    ui.horizontal(|ui| {
        ui.add_sized(vec2(110.0, 22.0), egui::Label::new(egui::RichText::new(label).color(t.text_dim)));
        let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        let picker_resp = ui.push_id(("lumetri-col", ctx.is_secondary, ctx.is_group_mode, ctx.scope_id, ctx.idx, id), |ui| {
            egui::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::color_picker::Alpha::Opaque)
        });
        row_rect = picker_resp.response.rect;
        if picker_resp.inner.changed() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id, "value": [rgba.r(), rgba.g(), rgba.b(), rgba.a()]}));
        }
        if !ctx.is_group_mode {
            let armed = app
                .ui
                .eyedropper
                .as_ref()
                .is_some_and(|ed| ed.clip == ctx.clip.0 && ed.effect == ctx.idx && ed.param == id && ed.kind == crate::state::EyedropperKind::ColorParam);
            let drop = crate::icons::button(ui, crate::icons::Icon::Eyedropper, 16.0, armed, t, tl!("Eyedropper (pick color from Program monitor)"));
            ctx.auto_rows.push((format!("lumetri.eyedropper.{id}"), drop.rect, "Eyedropper"));
            if drop.clicked() {
                arm_lumetri_eyedropper(app, ctx, id, crate::state::EyedropperKind::ColorParam);
            }
        }
        let modified = (c[0] - default_c[0]).abs() > 1e-3 || (c[1] - default_c[1]).abs() > 1e-3 || (c[2] - default_c[2]).abs() > 1e-3;
        let rst = wheels::paint_reset_icon_button(ui, if modified { t.accent } else { t.text_faint }, tl!("Reset tint"));
        if rst.clicked() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id, "value": default_c, "merge": false}));
        }
        if !ctx.is_group_mode && pd.animatable {
            let (kf_rect, kf_resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
            let kf_center = kf_rect.center();
            let prm = ctx.e.param(id);
            let at_key = prm.is_some_and(|p| p.keyframes.iter().any(|k| k.time == ctx.mt));
            let animated = prm.is_some_and(|p| p.is_animated());
            let s = 4.2;
            let pts = vec![kf_center + vec2(0.0, -s), kf_center + vec2(s, 0.0), kf_center + vec2(0.0, s), kf_center + vec2(-s, 0.0)];
            if at_key {
                ui.painter().add(egui::Shape::convex_polygon(pts, t.hot_text, Stroke::NONE));
            } else {
                let stroke_col = if animated {
                    t.accent
                } else if kf_resp.hovered() {
                    t.text
                } else {
                    t.text_faint
                };
                ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.2, stroke_col)));
            }
            ctx.auto_rows.push((format!("lumetri.keyframe.{id}"), kf_rect, "Toggle keyframe"));
            if kf_resp.on_hover_text(if at_key { tl!("Remove keyframe") } else { tl!("Add/Toggle keyframe") }).clicked() {
                ctx.mask_actions.push(("effects.addKeyframe".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": id})));
            }
        }
    });
    row_rect
}

#[allow(clippy::too_many_arguments)]
fn mask_drag_param(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    ctx: &mut ColorPanelCtx<'_>,
    mask_idx: usize,
    param_name: &str,
    label: &str,
    cur_val: f64,
    default_val: f64,
    range: std::ops::RangeInclusive<f64>,
    suffix: &str,
    param: &filmcraft_project::Param,
    auto_id: &str,
    auto_label: &'static str,
) {
    let t = ctx.t;
    ui.label(egui::RichText::new(label).small());
    let mut val = cur_val;
    let resp = ui.add(egui::DragValue::new(&mut val).range(range).speed(0.5).suffix(suffix));
    ctx.auto_rows.push((auto_id.into(), resp.rect, auto_label));
    if resp.double_clicked() {
        ctx.mask_actions.push(("masks.set".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": mask_idx, param_name: default_val, "merge": false})));
    } else if resp.changed() {
        ctx.mask_actions.push(("masks.set".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": mask_idx, param_name: val, "merge": true})));
    }
    let (kf_rect, kf_resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
    let kf_center = kf_rect.center();
    let at_key = param.keyframes.iter().any(|k| k.time == ctx.mt);
    let animated = param.is_animated();
    let s = 3.8;
    let pts = vec![kf_center + vec2(0.0, -s), kf_center + vec2(s, 0.0), kf_center + vec2(0.0, s), kf_center + vec2(-s, 0.0)];
    if at_key {
        ui.painter().add(egui::Shape::convex_polygon(pts, t.hot_text, Stroke::NONE));
    } else {
        let stroke_col = if animated {
            t.accent
        } else if kf_resp.hovered() {
            t.text
        } else {
            t.text_faint
        };
        ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.1, stroke_col)));
    }
    let kf_auto = format!("{auto_id}.keyframe");
    ctx.auto_rows.push((kf_auto, kf_rect, "Toggle keyframe"));
    let _ = current_panel;
    if kf_resp.on_hover_text(if at_key { tl!("Remove keyframe") } else { tl!("Add/Toggle keyframe") }).clicked() {
        ctx.mask_actions.push(("effects.addKeyframe".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "mask": mask_idx, "param": param_name})));
    }
}

fn ensure_lumetri_effect(app: &mut FilmcraftApp, ctx: &ColorPanelCtx<'_>) -> Option<usize> {
    if let Some(seq) = app.session.active_sequence()
        && let Some((_, it)) = seq.find_item(ctx.clip)
        && it.effects.get(ctx.idx).is_some_and(|e| e.effect == "lumetri")
    {
        return Some(ctx.idx);
    }
    match app.session.execute("effects.apply", json!({"clips": [ctx.clip.0], "effect": "lumetri"})) {
        Ok(applied) => {
            Some(applied.get("applied").and_then(|a| a.get(0)).and_then(|o| o.get("index")).and_then(|v| v.as_u64()).unwrap_or(ctx.idx as u64) as usize)
        }
        Err(err) => {
            app.ui.status = err.to_string();
            None
        }
    }
}

fn apply_color_resets(
    app: &mut FilmcraftApp,
    clip: ClipId,
    target_idx: usize,
    group_id: Option<u64>,
    mask_count: usize,
    reset_wheels: &[&'static str],
    reset_sections: &[&'static str],
) {
    let set_param = |s: &mut filmcraft_engine::Session, param: &str, val: Value| -> filmcraft_engine::Result<Value> {
        match group_id {
            Some(gid) => s.execute("color.groupSetParam", json!({"group": gid, "param": param, "value": val, "merge": false})),
            None => s.execute("effects.setParam", json!({"clip": clip.0, "effect": target_idx, "param": param, "value": val, "merge": false})),
        }
    };
    if !reset_wheels.is_empty() {
        let res = app.session.edit_group("Reset Color Wheel", |s| {
            for &w in reset_wheels {
                let pairs: &[(&str, Value)] = match w {
                    "lift" => &[("wheel_shadows", json!([0.0, 0.0])), ("wheel_shadows_l", json!(0.0))],
                    "gamma" => &[("wheel_midtones", json!([0.0, 0.0])), ("wheel_midtones_l", json!(0.0))],
                    "gain" => &[("wheel_highlights", json!([0.0, 0.0])), ("wheel_highlights_l", json!(0.0))],
                    "offset" => &[("temperature", json!(0.0)), ("tint", json!(0.0)), ("exposure", json!(0.0))],
                    "offset_2d" => &[("temperature", json!(0.0)), ("tint", json!(0.0))],
                    _ => &[],
                };
                for (param, val) in pairs {
                    set_param(s, param, val.clone())?;
                }
            }
            Ok(Value::Null)
        });
        if let Err(err) = res {
            app.ui.status = err.to_string();
        }
    }
    if !reset_sections.is_empty() {
        let res = app.session.edit_group("Reset Color Section", |s| {
            for &sec_id in reset_sections {
                if sec_id == "Power Windows" {
                    if group_id.is_none() {
                        set_param(s, "masks_on", json!(true))?;
                        for mi in (0..mask_count).rev() {
                            s.execute("masks.remove", json!({"clip": clip.0, "effect": target_idx, "mask": mi}))?;
                        }
                    }
                    continue;
                }
                let Some(def) = get_tool_def(sec_id) else { continue };
                let Some(edef) = filmcraft_project::find_effect("lumetri") else { continue };
                for pd in edef.params.iter().filter(|p| p.group == Some(sec_id) && p.id != def.param) {
                    let val = match &pd.default {
                        ParamValue::Float(v) => json!(v),
                        ParamValue::Choice(c) => json!(c),
                        ParamValue::Text(st) => json!(st),
                        ParamValue::Bool(b) => json!(b),
                        ParamValue::Color(c) => json!(c),
                        ParamValue::Vec2(v) => json!([v.x, v.y]),
                        ParamValue::Curve(pts) => json!(pts),
                        _ => continue,
                    };
                    set_param(s, pd.id, val)?;
                }
            }
            Ok(Value::Null)
        });
        if let Err(err) = res {
            app.ui.status = err.to_string();
        }
    }
}

fn arm_lumetri_eyedropper(app: &mut FilmcraftApp, ctx: &ColorPanelCtx<'_>, param: &str, kind: crate::state::EyedropperKind) {
    let Some(target_idx) = ensure_lumetri_effect(app, ctx) else { return };
    let next = crate::state::Eyedropper { clip: ctx.clip.0, effect: target_idx, param: param.to_string(), mask: None, kind };
    let same = app.ui.eyedropper.as_ref() == Some(&next);
    app.ui.eyedropper = if same { None } else { Some(next) };
}

fn curve_histogram(app: &mut FilmcraftApp, ui: &egui::Ui) -> Option<[f32; 64]> {
    let seq_id = app.session.state.active_sequence?;
    let seq = app.session.active_sequence()?;
    let rate = seq.settings.frame_rate;
    let frame = rate.frame_at(app.session.playhead()).max(0);
    let fkey = crate::frames::FrameKey {
        target: crate::frames::Target::Sequence(seq_id),
        frame,
        size: 250,
        revision: app.session.revision,
        draft: false,
        bypass: app.ui.global_bypass,
    };
    let cache_id = egui::Id::unique("lumetri-curve-hist-cache");
    let dragging_now = ui.ctx().input(|i| i.pointer.any_down());
    if let Some((cached_key, bins)) = ui.data(|d| d.get_temp::<(crate::frames::FrameKey, [f32; 64])>(cache_id))
        && (cached_key == fkey || (dragging_now && cached_key.target == fkey.target && cached_key.frame == fkey.frame && cached_key.bypass == fkey.bypass))
    {
        return Some(bins);
    }
    let project = app.session.project.clone();
    app.frames.request(fkey, rate.tick_of(frame), 0.25, &project, 90);
    let img = app.frames.get(&fkey)?;
    let mut counts = [0u32; 64];
    for px in img.px.as_chunks::<4>().0 {
        let luma = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0;
        let bin = ((luma * 63.0).round() as usize).min(63);
        counts[bin] += 1;
    }
    let max_c = counts.iter().copied().max().unwrap_or(1).max(1) as f32;
    let mut bins = [0.0f32; 64];
    for (b, c) in bins.iter_mut().zip(counts) {
        *b = c as f32 / max_c;
    }
    ui.data_mut(|d| d.insert_temp(cache_id, (fkey, bins)));
    Some(bins)
}

fn push_hsl_bar_param(ctx: &mut ColorPanelCtx<'_>, param: &str, value: f64, merge: Option<&str>, begin: bool) {
    let mut obj = json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": param, "value": value});
    obj["merge"] = match merge {
        Some(m) => json!(m),
        None => json!(false),
    };
    if begin {
        obj["begin"] = json!(true);
    }
    ctx.actions.push(obj);
}

fn draw_hsl_range_bar(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let hue = ctx.e.param("hsl_hue").map(|p| p.f64_at(ctx.mt)).unwrap_or(0.0) as f32;
    let range = ctx.e.param("hsl_hue_range").map(|p| p.f64_at(ctx.mt)).unwrap_or(30.0) as f32;
    let sat_min = (ctx.e.param("hsl_sat_min").map(|p| p.f64_at(ctx.mt)).unwrap_or(10.0) / 100.0).clamp(0.0, 1.0) as f32;
    let luma_min = (ctx.e.param("hsl_luma_min").map(|p| p.f64_at(ctx.mt)).unwrap_or(5.0) / 100.0).clamp(0.0, 1.0) as f32;
    let luma_max = (ctx.e.param("hsl_luma_max").map(|p| p.f64_at(ctx.mt)).unwrap_or(95.0) / 100.0).clamp(0.0, 1.0) as f32;

    let w = ui.available_width().max(60.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    let hue_r = Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + 11.0));
    let half_w = (w - 4.0) * 0.5;
    let sat_r = Rect::from_min_max(pos2(r.min.x, r.min.y + 14.0), pos2(r.min.x + half_w, r.min.y + 22.0));
    let lum_r = Rect::from_min_max(pos2(r.min.x + half_w + 4.0, r.min.y + 14.0), pos2(r.max.x, r.min.y + 22.0));

    let hue_id = egui::Id::unique(("hsl_bar_hue", current_panel, ctx.is_secondary, ctx.scope_id, ctx.idx));
    let sat_id = egui::Id::unique(("hsl_bar_sat", current_panel, ctx.is_secondary, ctx.scope_id, ctx.idx));
    let lum_id = egui::Id::unique(("hsl_bar_lum", current_panel, ctx.is_secondary, ctx.scope_id, ctx.idx));
    let r_hue = ui
        .interact(hue_r, hue_id, Sense::click_and_drag())
        .on_hover_text(tl!("Drag center to shift hue, drag edges to adjust Hue Range; double-click to reset"));
    let r_sat = ui.interact(sat_r, sat_id, Sense::click_and_drag()).on_hover_text(tl!("Click or drag to adjust minimum Saturation; double-click to reset"));
    let r_lum =
        ui.interact(lum_r, lum_id, Sense::click_and_drag()).on_hover_text(tl!("Drag edges for Luma Min/Max or center to shift range; double-click to reset"));

    ctx.auto_rows.push(("lumetri.hsl.hue_bar".into(), hue_r, "HSL Hue Range Bar"));
    ctx.auto_rows.push(("lumetri.hsl.sat_bar".into(), sat_r, "HSL Saturation Min Bar"));
    ctx.auto_rows.push(("lumetri.hsl.luma_bar".into(), lum_r, "HSL Luma Range Bar"));

    let p = ui.painter();
    let n = 48;
    for i in 0..n {
        let f0 = i as f32 / n as f32;
        let deg = f0 * 360.0;
        let d_raw = (deg - hue).rem_euclid(360.0);
        let dist = d_raw.min(360.0 - d_raw);
        let in_key = dist <= range;
        let rgb = filmcraft_color::hsl_to_rgb(f0, if in_key { 0.85 } else { 0.25 }, if in_key { 0.52 } else { 0.22 });
        let col = Color32::from_rgb((rgb[0].clamp(0.0, 1.0) * 255.0) as u8, (rgb[1].clamp(0.0, 1.0) * 255.0) as u8, (rgb[2].clamp(0.0, 1.0) * 255.0) as u8);
        let x0 = hue_r.min.x + hue_r.width() * f0;
        p.rect_filled(Rect::from_min_max(pos2(x0, hue_r.min.y), pos2(x0 + hue_r.width() / n as f32 + 0.5, hue_r.max.y)), 0.0, col);
    }
    let hc = hue_r.min.x + (hue.rem_euclid(360.0) / 360.0) * hue_r.width();
    let hl = hue_r.min.x + ((hue - range).rem_euclid(360.0) / 360.0) * hue_r.width();
    let hr = hue_r.min.x + ((hue + range).rem_euclid(360.0) / 360.0) * hue_r.width();
    p.line_segment([pos2(hl, hue_r.min.y), pos2(hl, hue_r.max.y)], Stroke::new(1.2, Color32::from_white_alpha(200)));
    p.line_segment([pos2(hr, hue_r.min.y), pos2(hr, hue_r.max.y)], Stroke::new(1.2, Color32::from_white_alpha(200)));
    p.line_segment([pos2(hc, hue_r.min.y), pos2(hc, hue_r.max.y)], Stroke::new(1.8, Color32::WHITE));

    p.rect_filled(sat_r, 1.0, Color32::from_rgb(28, 30, 36));
    let sat_x = sat_r.min.x + sat_r.width() * sat_min;
    p.rect_filled(Rect::from_min_max(pos2(sat_x, sat_r.min.y), sat_r.max), 1.0, Color32::from_rgb(70, 170, 130));
    p.line_segment([pos2(sat_x, sat_r.min.y), pos2(sat_x, sat_r.max.y)], Stroke::new(1.5, Color32::WHITE));

    p.rect_filled(lum_r, 1.0, Color32::from_rgb(28, 30, 36));
    let (l0, l1) = (luma_min.min(luma_max), luma_min.max(luma_max));
    let lx0 = lum_r.min.x + lum_r.width() * l0;
    let lx1 = lum_r.min.x + lum_r.width() * l1;
    p.rect_filled(Rect::from_min_max(pos2(lx0, lum_r.min.y), pos2(lx1, lum_r.max.y)), 1.0, Color32::from_rgb(190, 195, 205));
    p.line_segment([pos2(lx0, lum_r.min.y), pos2(lx0, lum_r.max.y)], Stroke::new(1.5, Color32::WHITE));
    p.line_segment([pos2(lx1, lum_r.min.y), pos2(lx1, lum_r.max.y)], Stroke::new(1.5, Color32::WHITE));

    let hue_mode_id = hue_id.with("mode");
    if r_hue.drag_started()
        && let Some(pos) = r_hue.interact_pointer_pos()
    {
        let near_edge = (pos.x - hl).abs() <= 8.0 || (pos.x - hr).abs() <= 8.0;
        ui.data_mut(|d| d.insert_temp(hue_mode_id, near_edge));
    }
    if r_hue.double_clicked() {
        push_hsl_bar_param(ctx, "hsl_hue", 0.0, None, false);
        push_hsl_bar_param(ctx, "hsl_hue_range", 30.0, None, false);
    } else if (r_hue.dragged() || r_hue.clicked())
        && let Some(pos) = r_hue.interact_pointer_pos()
    {
        let f = ((pos.x - hue_r.min.x) / hue_r.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ptr_deg = f * 360.0;
        let range_mode = r_hue.dragged() && ui.data(|d| d.get_temp::<bool>(hue_mode_id).unwrap_or(false));
        let merge = r_hue.dragged().then_some("hsl_hue_bar");
        if range_mode {
            let d_raw = (ptr_deg - hue as f64).rem_euclid(360.0);
            let dist = (d_raw.min(360.0 - d_raw).clamp(1.0, 180.0) * 10.0).round() / 10.0;
            push_hsl_bar_param(ctx, "hsl_hue_range", dist, merge, r_hue.drag_started());
        } else {
            let nv = (ptr_deg.clamp(0.0, 360.0) * 10.0).round() / 10.0;
            push_hsl_bar_param(ctx, "hsl_hue", nv, merge, r_hue.drag_started());
        }
    }

    if r_sat.double_clicked() {
        push_hsl_bar_param(ctx, "hsl_sat_min", 10.0, None, false);
    } else if (r_sat.dragged() || r_sat.clicked())
        && let Some(pos) = r_sat.interact_pointer_pos()
    {
        let f = ((pos.x - sat_r.min.x) / sat_r.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let nv = (f * 100.0).round();
        push_hsl_bar_param(ctx, "hsl_sat_min", nv, r_sat.dragged().then_some("hsl_sat_bar"), r_sat.drag_started());
    }

    let lum_mode_id = lum_id.with("mode");
    if r_lum.drag_started()
        && let Some(pos) = r_lum.interact_pointer_pos()
    {
        let f = ((pos.x - lum_r.min.x) / lum_r.width().max(1.0)).clamp(0.0, 1.0);
        let d0 = (pos.x - lx0).abs();
        let d1 = (pos.x - lx1).abs();
        let mode: u8 = if d0 <= 7.0 && d0 <= d1 {
            0
        } else if d1 <= 7.0 {
            1
        } else if f > l0 && f < l1 {
            2
        } else if d0 <= d1 {
            0
        } else {
            1
        };
        ui.data_mut(|d| d.insert_temp(lum_mode_id, (mode, f, l0, l1)));
    }
    if r_lum.double_clicked() {
        push_hsl_bar_param(ctx, "hsl_luma_min", 5.0, None, false);
        push_hsl_bar_param(ctx, "hsl_luma_max", 95.0, None, false);
    } else if (r_lum.dragged() || r_lum.clicked())
        && let Some(pos) = r_lum.interact_pointer_pos()
    {
        let f = ((pos.x - lum_r.min.x) / lum_r.width().max(1.0)).clamp(0.0, 1.0);
        if r_lum.dragged() {
            let (mode, anchor_f, init_l0, init_l1) = ui.data(|d| d.get_temp::<(u8, f32, f32, f32)>(lum_mode_id).unwrap_or((0, f, l0, l1)));
            let beg = r_lum.drag_started();
            match mode {
                0 => {
                    let v = ((f.min(l1) * 100.0).round() as f64).clamp(0.0, 100.0);
                    push_hsl_bar_param(ctx, "hsl_luma_min", v, Some("hsl_lum_bar"), beg);
                }
                1 => {
                    let v = ((f.max(l0) * 100.0).round() as f64).clamp(0.0, 100.0);
                    push_hsl_bar_param(ctx, "hsl_luma_max", v, Some("hsl_lum_bar"), beg);
                }
                _ => {
                    let span = (init_l1 - init_l0).max(0.0);
                    let delta = f - anchor_f;
                    let nl0 = (init_l0 + delta).clamp(0.0, 1.0 - span);
                    let nl1 = nl0 + span;
                    push_hsl_bar_param(ctx, "hsl_luma_min", (nl0 * 100.0).round() as f64, Some("hsl_lum_bar"), beg);
                    push_hsl_bar_param(ctx, "hsl_luma_max", (nl1 * 100.0).round() as f64, Some("hsl_lum_bar"), false);
                }
            }
        } else if (pos.x - lx0).abs() <= (pos.x - lx1).abs() {
            let v = ((f.min(l1) * 100.0).round() as f64).clamp(0.0, 100.0);
            push_hsl_bar_param(ctx, "hsl_luma_min", v, None, false);
        } else {
            let v = ((f.max(l0) * 100.0).round() as f64).clamp(0.0, 100.0);
            push_hsl_bar_param(ctx, "hsl_luma_max", v, None, false);
        }
    }
}

fn lerp_col(a: Color32, b: Color32, f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
