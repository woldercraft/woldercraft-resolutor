//! Dedicated DaVinci Resolve / EffectCraft-style VFX Node Inspector (`VfxInspector`).
//!
//! Displays and edits **exclusively** the active node selected in `VfxNodes`:
//! - Clean node header with Pass-Through LED (`●`), node name, and `Controles | Ajustes` tabs
//! - Filled horizontal slider bars + editable numeric fields (no floating bare numbers)
//! - Always-visible `< ◆ >` keyframe controls on every animatable parameter (`Effect`, `Instance`,
//!   `Merge`, `Mask`, and `Settings.Blend`) with 1-click keyframe creation and auto-keyframe

use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{ClipId, LayerBlendMode, MergeOperator, Param, ParamKind, ParamValue, TrackItem, VfxGraph, VfxNode, VfxNodeId, VfxNodeKind};
use filmcraft_time::Tick;
use serde_json::json;

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::panels::vfx_nodes::{node_accent, node_display_name, resolve_target_clip};
use crate::theme::Tokens;

/// Entry point for the `VfxInspector` panel.
#[allow(clippy::cognitive_complexity)]
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::LEFT)));
    let ui = &mut ui;

    let Some((clip_id, _clip_name)) = resolve_target_clip(app) else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new(tl!("Select a video clip to inspect its VFX node")).color(t.text_dim));
        });
        return;
    };

    let ph = app.session.playhead();
    let (graph, item_snap) = {
        let Some(seq) = app.session.active_sequence() else { return };
        let Some((_, it)) = seq.find_item(clip_id) else { return };
        (it.effective_vfx_graph().into_owned(), it.clone())
    };

    if graph.nodes.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new(tl!("No VFX nodes on this clip. Add an effect or press Shift+Space in VFX Nodes.")).color(t.text_dim));
        });
        return;
    }

    // Resolve active node (defaulting cleanly to first node if unset or deleted)
    let active_id = app
        .ui
        .vfx_active_node_id
        .map(VfxNodeId)
        .filter(|id| graph.node(*id).is_some())
        .or_else(|| graph.nodes.first().map(|n| n.id));
    let Some(nid) = active_id else { return };
    app.ui.vfx_active_node_id = Some(nid.0);
    let Some(node) = graph.node(nid).cloned() else { return };

    let mt_now = item_snap.effect_time_at(ph.clamp(item_snap.start, (item_snap.end() - Tick(1)).max(item_snap.start)));

    // 1. Clean Node Header: [●] Title · [KIND] · [Controles | Ajustes]
    ui.add_space(4.0);
    render_node_header(app, ui, &t, clip_id, &node, &graph);
    ui.separator();

    // 2. Search & Animated-Only Filter Bar
    render_filter_bar(app, ui, &t);
    ui.separator();

    // 3. Body: Controles (0) or Ajustes (1)
    egui::ScrollArea::vertical().id_salt("vfx_inspector_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(4.0);
        if app.ui.vfx_inspector_tab == 1 {
            render_settings_tab(app, ui, &t, clip_id, &item_snap, &node, mt_now);
        } else {
            render_controls_tab(app, ui, &t, clip_id, &item_snap, &node, &graph, mt_now);
        }
        ui.add_space(8.0);
    });
}

fn render_node_header(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens, clip_id: ClipId, node: &VfxNode, graph: &VfxGraph) {
    let accent = node_accent(&node.kind);
    let title = node_display_name(node, graph);
    let kind_tag = match &node.kind {
        VfxNodeKind::Effect(_) => "FX",
        VfxNodeKind::Merge { .. } => "MERGE",
        VfxNodeKind::Mask(_) => "MASK",
        VfxNodeKind::PipeRouter => "ROUTER",
        VfxNodeKind::Instance { .. } => "INST",
    };

    ui.horizontal(|ui| {
        ui.add_space(6.0);
        // Pass-Through LED
        let (led_r, led_resp) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::click());
        let led_col = if node.enabled { Color32::from_rgb(52, 211, 153) } else { t.text_dim };
        ui.painter().circle_filled(led_r.center(), 4.5, led_col);
        app.auto.add("vfxInspector.passThrough", led_r, "Toggle Pass-Through");
        if led_resp.on_hover_text(tl!("Enable / bypass node (Ctrl+P)")).clicked() {
            let _ = app.session.execute("vfxGraph.togglePassThrough", json!({"clip": clip_id.0, "node": node.id.0}));
        }

        ui.label(egui::RichText::new(title).strong().size(13.0).color(t.text));
        ui.label(
            egui::RichText::new(kind_tag)
                .size(9.5)
                .strong()
                .color(accent)
                .background_color(accent.gamma_multiply(0.16)),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            for (tab_idx, auto_id, label, tip) in [
                (1u8, "vfxInspector.tab.settings", tl!("Settings"), tl!("Universal Blend, mask and RGBA channels")),
                (0u8, "vfxInspector.tab.controls", tl!("Controls"), tl!("Node parameters")),
            ] {
                let active = app.ui.vfx_inspector_tab == tab_idx;
                let resp = ui.selectable_label(active, egui::RichText::new(label).size(11.5).strong()).on_hover_text(tip);
                app.auto.add(auto_id, resp.rect, label);
                if resp.clicked() {
                    app.ui.vfx_inspector_tab = tab_idx;
                }
            }
        });
    });
}

fn render_filter_bar(app: &mut FilmcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let w = (ui.available_width() - 40.0).max(80.0);
        let te = ui.add(
            egui::TextEdit::singleline(&mut app.ui.effect_controls_search)
                .hint_text(tl!("Filter parameters…"))
                .desired_width(w),
        );
        app.auto.add("vfxInspector.search", te.rect, "Filter parameters");
        let anim_on = app.ui.effect_controls_animated_only;
        let btn = ui
            .selectable_label(anim_on, egui::RichText::new("◆").color(if anim_on { t.hot_text } else { t.text_dim }))
            .on_hover_text(tl!("Show animated or modified parameters only"));
        app.auto.add("vfxInspector.animatedOnly", btn.rect, "Animated Only");
        if btn.clicked() {
            app.ui.effect_controls_animated_only = !anim_on;
        }
    });
}

/// Always-visible `< ◆ >` keyframe navigator for a VFX node parameter.
/// Clicking `◆` immediately adds or removes a keyframe at the playhead!
#[allow(clippy::too_many_arguments)]
fn node_keyframe_nav(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    clip_id: ClipId,
    item: &TrackItem,
    node_id: VfxNodeId,
    param_id: &str,
    param: &Param,
    mt_now: Tick,
    is_settings_blend: bool,
) {
    let t = app.tokens;
    let animated = param.is_animated();
    let on_kf = param.keyframes.iter().any(|k| k.time == mt_now);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;

        // Previous keyframe `<`
        if animated {
            let prev = param.prev_keyframe(mt_now);
            let (r, resp) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::click());
            let c = r.center();
            let pts = vec![c + vec2(2.5, -4.5), c + vec2(2.5, 4.5), c + vec2(-3.0, 0.0)];
            ui.painter().add(egui::Shape::convex_polygon(pts, if prev.is_some() { t.text } else { t.text_faint }, Stroke::NONE));
            if resp.on_hover_text(tl!("Previous keyframe")).clicked()
                && let Some(tk) = prev
            {
                let target = item.start + Tick(((tk.0 - item.effect_time_at(item.start).0) as f64 / item.speed.max(1e-6)) as i64);
                let _ = app.session.execute("transport.seek", json!({"time": target.0}));
            }
        }

        // Direct Keyframe diamond `◆` (ALWAYS visible!)
        let (dr, dresp) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::click());
        let dc = dr.center();
        let s = 4.6f32;
        if on_kf {
            let pts = vec![dc + vec2(0.0, -s), dc + vec2(s, 0.0), dc + vec2(0.0, s), dc + vec2(-s, 0.0)];
            ui.painter().add(egui::Shape::convex_polygon(pts, t.hot_text, Stroke::NONE));
        } else {
            let col = if animated {
                t.accent
            } else if dresp.hovered() {
                t.text
            } else {
                t.text_dim
            };
            let pts = vec![dc + vec2(0.0, -s), dc + vec2(s, 0.0), dc + vec2(0.0, s), dc + vec2(-s, 0.0)];
            ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.3, col)));
        }
        app.auto.add(&format!("vfxInspector.kf.{}.{param_id}", node_id.0), dr, "Toggle Keyframe");
        if dresp.on_hover_text(tl!("Add or remove keyframe at current frame")).clicked() {
            if is_settings_blend {
                let _ = app
                    .session
                    .execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node_id.0, "blendToggleKeyframe": true}));
            } else {
                let _ = app
                    .session
                    .execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node_id.0, "param": param_id, "toggleKeyframe": true}));
            }
        }

        // Next keyframe `>`
        if animated {
            let next = param.next_keyframe(mt_now);
            let (r, resp) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::click());
            let c = r.center();
            let pts = vec![c + vec2(-2.5, -4.5), c + vec2(-2.5, 4.5), c + vec2(3.0, 0.0)];
            ui.painter().add(egui::Shape::convex_polygon(pts, if next.is_some() { t.text } else { t.text_faint }, Stroke::NONE));
            if resp.on_hover_text(tl!("Next keyframe")).clicked()
                && let Some(tk) = next
            {
                let target = item.start + Tick(((tk.0 - item.effect_time_at(item.start).0) as f64 / item.speed.max(1e-6)) as i64);
                let _ = app.session.execute("transport.seek", json!({"time": target.0}));
            }
        }
    });
}

/// DaVinci Resolve / EffectCraft style filled slider bar + inline numeric box + `< ◆ >` + `↺`.
#[allow(clippy::too_many_arguments)]
fn resolve_slider_row(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    clip_id: ClipId,
    item: &TrackItem,
    node_id: VfxNodeId,
    param_id: &str,
    label: &str,
    param: &Param,
    (soft_min, soft_max, hard_min, hard_max, decimals, unit, default_val): (f64, f64, f64, f64, usize, &str, f64),
    mt_now: Tick,
    scale_display: f64,
    is_settings_blend: bool,
) {
    let cur = param.f64_at(mt_now) * scale_display;
    let mut val = cur;
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let total_w = (ui.available_width() - 6.0).max(180.0);
        let label_w = (total_w * 0.34).clamp(76.0, 130.0);
        let kf_w = if param.is_animated() { 44.0 } else { 20.0 };
        let reset_w = 18.0;
        let num_w = 56.0;
        let track_w = (total_w - label_w - kf_w - reset_w - num_w - 20.0).max(52.0);

        // Label
        let (lr, _) = ui.allocate_exact_size(vec2(label_w, 20.0), Sense::hover());
        ui.painter().with_clip_rect(lr).text(lr.left_center(), Align2::LEFT_CENTER, label, Tokens::ui(11.5), t.text);

        // Filled horizontal slider track
        let (tr, tresp) = ui.allocate_exact_size(vec2(track_w, 16.0), Sense::click_and_drag());
        let span = (soft_max - soft_min).max(1e-6);
        let frac = ((val - soft_min) / span).clamp(0.0, 1.0) as f32;
        ui.painter().rect(tr, CornerRadius::same(3), t.field_bg, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
        let fill_w = (tr.width() * frac).max(0.0);
        if fill_w > 1.0 {
            let fill_r = Rect::from_min_size(tr.min, vec2(fill_w, tr.height()));
            ui.painter().rect_filled(fill_r, CornerRadius::same(3), t.accent.gamma_multiply(0.45));
            ui.painter().line_segment([pos2(fill_r.right(), tr.top() + 2.0), pos2(fill_r.right(), tr.bottom() - 2.0)], Stroke::new(1.8, t.accent));
        }
        if (tresp.clicked() || tresp.dragged())
            && let Some(pos) = tresp.interact_pointer_pos()
        {
            let f = ((pos.x - tr.min.x) / tr.width().max(1.0)).clamp(0.0, 1.0) as f64;
            val = (soft_min + f * span).clamp(hard_min, hard_max);
            changed = true;
        }
        if tresp.double_clicked() {
            val = default_val;
            changed = true;
        }

        // Editable numeric box
        let dv = ui.add_sized(
            vec2(num_w, 18.0),
            egui::DragValue::new(&mut val)
                .range(hard_min..=hard_max)
                .speed((span / 250.0).max(0.01))
                .max_decimals(decimals)
                .suffix(unit),
        );
        if dv.changed() {
            changed = true;
        }

        // Always-visible Keyframe `< ◆ >`
        node_keyframe_nav(app, ui, clip_id, item, node_id, param_id, param, mt_now, is_settings_blend);

        // Reset `↺`
        let (rr, rresp) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::click());
        icons::paint(ui.painter(), rr.shrink(1.0), Icon::Reset, if rresp.hovered() { t.text } else { t.text_dim });
        if rresp.on_hover_text(tl!("Reset to default")).clicked() {
            val = default_val;
            changed = true;
        }
    });

    if changed {
        let out_val = val / scale_display;
        let auto_kf = param.is_animated();
        if is_settings_blend {
            let _ = app.session.execute(
                "vfxGraph.setNodeSetting",
                json!({"clip": clip_id.0, "node": node_id.0, "blend": out_val, "blendKeyframe": auto_kf, "merge": true}),
            );
        } else {
            let _ = app.session.execute(
                "vfxGraph.setNodeParam",
                json!({"clip": clip_id.0, "node": node_id.0, "param": param_id, "value": out_val, "keyframe": auto_kf, "merge": true}),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_settings_tab(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    clip_id: ClipId,
    item: &TrackItem,
    node: &VfxNode,
    mt_now: Tick,
) {
    resolve_slider_row(
        app,
        ui,
        t,
        clip_id,
        item,
        node.id,
        "__settings.blend",
        "Blend",
        &node.settings.blend,
        (0.0, 100.0, 0.0, 100.0, 1, "%", 100.0),
        mt_now,
        100.0,
        true,
    );

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let mut inv_m = node.settings.invert_mask;
        if ui.checkbox(&mut inv_m, tl!("Apply Mask Inverted")).changed() {
            let _ = app.session.execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node.id.0, "invertMask": inv_m}));
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let mut mul_m = node.settings.multiply_by_mask;
        if ui.checkbox(&mut mul_m, tl!("Multiply by Mask")).changed() {
            let _ = app
                .session
                .execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node.id.0, "multiplyByMask": mul_m}));
        }
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tl!("Channels:")).size(11.5).color(t.text_dim));
        let mut ch = node.settings.channels;
        let mut changed = false;
        for (i, (lbl, col)) in [
            ("R", Color32::from_rgb(239, 68, 68)),
            ("G", Color32::from_rgb(34, 197, 94)),
            ("B", Color32::from_rgb(59, 130, 246)),
            ("A", Color32::from_rgb(226, 232, 240)),
        ]
        .into_iter()
        .enumerate()
        {
            let btn = ui.selectable_label(ch[i], egui::RichText::new(lbl).strong().color(if ch[i] { col } else { t.text_dim }));
            if btn.clicked() {
                ch[i] = !ch[i];
                changed = true;
            }
        }
        if changed {
            let _ = app.session.execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node.id.0, "channels": ch}));
        }
    });

    ui.add_space(4.0);
    let mut lbl = node.label.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tl!("Label:")).size(11.5).color(t.text_dim));
        let te = ui.add(egui::TextEdit::singleline(&mut lbl).desired_width((ui.available_width() - 20.0).max(80.0)));
        if te.lost_focus() || (te.changed() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            let _ = app.session.execute("vfxGraph.setNodeSetting", json!({"clip": clip_id.0, "node": node.id.0, "label": lbl}));
        }
    });
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
fn render_controls_tab(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    clip_id: ClipId,
    item: &TrackItem,
    node: &VfxNode,
    graph: &VfxGraph,
    mt_now: Tick,
) {
    let q = app.ui.effect_controls_search.trim().to_ascii_lowercase();
    let anim_only = app.ui.effect_controls_animated_only;

    match &node.kind {
        VfxNodeKind::PipeRouter => {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(tl!("Cable router with no image parameters.")).color(t.text_dim));
            });
        }
        VfxNodeKind::Merge { blend_mode, operator, center, size, angle } => {
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                if ui.small_button(tl!("Swap BG / FG (Ctrl+T)")).clicked() {
                    let _ = app.session.execute("vfxGraph.swapInputs", json!({"clip": clip_id.0, "node": node.id.0}));
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.label(tl!("Operator:"));
                craft_ui::ComboBox::from_id_salt(("vfx_ins_op", node.id.0))
                    .selected_text(operator.label())
                    .show_ui(ui, |ui| {
                        for op in MergeOperator::ALL {
                            if ui.selectable_label(*operator == op, op.label()).clicked() {
                                let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": "operator", "value": op.label()}));
                            }
                        }
                    });
                ui.label(tl!("Blend Mode:"));
                craft_ui::ComboBox::from_id_salt(("vfx_ins_bm", node.id.0))
                    .selected_text(blend_mode.label())
                    .show_ui(ui, |ui| {
                        for bm in LayerBlendMode::ALL {
                            if ui.selectable_label(*blend_mode == bm, bm.label()).clicked() {
                                let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": "blendMode", "value": bm.label()}));
                            }
                        }
                    });
            });
            ui.add_space(4.0);
            // Center (Vec2 + < ◆ >)
            if (!anim_only || center.is_animated()) && (q.is_empty() || "center".contains(&q)) {
                let c = center.vec2_at(mt_now);
                let (mut cx, mut cy) = (c.x, c.y);
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label("Center");
                    let rx = ui.add(egui::DragValue::new(&mut cx).speed(0.005).prefix("X ").fixed_decimals(3));
                    let ry = ui.add(egui::DragValue::new(&mut cy).speed(0.005).prefix("Y ").fixed_decimals(3));
                    if rx.changed() || ry.changed() {
                        let _ = app.session.execute(
                            "vfxGraph.setNodeParam",
                            json!({"clip": clip_id.0, "node": node.id.0, "param": "center", "value": [cx, cy], "keyframe": center.is_animated(), "merge": true}),
                        );
                    }
                    node_keyframe_nav(app, ui, clip_id, item, node.id, "center", center, mt_now, false);
                });
            }
            if (!anim_only || size.is_animated()) && (q.is_empty() || "size".contains(&q)) {
                resolve_slider_row(app, ui, t, clip_id, item, node.id, "size", "Size", size, (0.05, 4.0, 0.01, 10.0, 2, "", 1.0), mt_now, 1.0, false);
            }
            if (!anim_only || angle.is_animated()) && (q.is_empty() || "angle".contains(&q)) {
                resolve_slider_row(app, ui, t, clip_id, item, node.id, "angle", "Angle", angle, (-180.0, 180.0, -3600.0, 3600.0, 1, "°", 0.0), mt_now, 1.0, false);
            }
        }
        VfxNodeKind::Mask(mask) => {
            if (!anim_only || mask.feather.is_animated()) && (q.is_empty() || "feather".contains(&q)) {
                resolve_slider_row(app, ui, t, clip_id, item, node.id, "feather", "Feather", &mask.feather, (0.0, 150.0, 0.0, 1000.0, 1, " px", 10.0), mt_now, 1.0, false);
            }
            if (!anim_only || mask.opacity.is_animated()) && (q.is_empty() || "opacity".contains(&q)) {
                resolve_slider_row(app, ui, t, clip_id, item, node.id, "opacity", "Opacity", &mask.opacity, (0.0, 100.0, 0.0, 100.0, 1, "%", 100.0), mt_now, 1.0, false);
            }
            if (!anim_only || mask.expansion.is_animated()) && (q.is_empty() || "expansion".contains(&q)) {
                resolve_slider_row(app, ui, t, clip_id, item, node.id, "expansion", "Expansion", &mask.expansion, (-100.0, 100.0, -1000.0, 1000.0, 1, " px", 0.0), mt_now, 1.0, false);
            }
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                let mut inv = mask.inverted;
                if ui.checkbox(&mut inv, tl!("Inverted")).changed() {
                    let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": "inverted", "value": inv}));
                }
            });
        }
        VfxNodeKind::Effect(_) | VfxNodeKind::Instance { .. } => {
            if let VfxNodeKind::Instance { source, overrides } = &node.kind {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(tlf!("Instance linked to Node #{id}", id = source.0))
                            .strong()
                            .size(11.0)
                            .color(Color32::from_rgb(16, 185, 129)),
                    );
                    if let Some(fx) = graph.resolve_effect(node.id)
                        && let Some(def) = fx.def()
                    {
                        for pd in def.params.iter().take(6) {
                            let is_over = overrides.contains_key(pd.id);
                            let lbl = if is_over { format!("{} (Indep.)", crate::i18n::t(pd.label)) } else { format!("{} (Link)", crate::i18n::t(pd.label)) };
                            if ui.small_button(lbl).on_hover_text("De-instance / Re-instance").clicked() {
                                let _ = app.session.execute("vfxGraph.deinstanceParam", json!({"clip": clip_id.0, "node": node.id.0, "param": pd.id, "reinstance": is_over}));
                            }
                        }
                    }
                });
                ui.separator();
            }

            let Some(fx) = graph.resolve_effect(node.id) else { return };
            let Some(def) = fx.def() else { return };
            let mut last_group: Option<&'static str> = None;

            for pd in &def.params {
                let localized = crate::i18n::t(pd.label);
                let prm = fx.param(pd.id).cloned().unwrap_or_else(|| Param::new(pd.default.clone()));
                if !q.is_empty() && !localized.to_ascii_lowercase().contains(&q) && !pd.id.to_ascii_lowercase().contains(&q) {
                    continue;
                }
                if anim_only && !prm.is_animated() && prm.value == pd.default {
                    continue;
                }
                if pd.group != last_group {
                    last_group = pd.group;
                    if let Some(grp) = pd.group {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(crate::i18n::t(grp)).strong().size(11.0).color(t.accent));
                        });
                    }
                }

                let val = prm.value_at(mt_now);
                match (&pd.kind, val) {
                    (ParamKind::Float { min, max, soft_min, soft_max, unit, decimals }, ParamValue::Float(_)) => {
                        let def_v = pd.default.as_f64().unwrap_or(0.0);
                        resolve_slider_row(
                            app,
                            ui,
                            t,
                            clip_id,
                            item,
                            node.id,
                            pd.id,
                            localized,
                            &prm,
                            (*soft_min, *soft_max, *min, *max, *decimals as usize, unit, def_v),
                            mt_now,
                            1.0,
                            false,
                        );
                    }
                    (ParamKind::Angle, ParamValue::Float(_)) => {
                        let def_v = pd.default.as_f64().unwrap_or(0.0);
                        resolve_slider_row(
                            app,
                            ui,
                            t,
                            clip_id,
                            item,
                            node.id,
                            pd.id,
                            localized,
                            &prm,
                            (-180.0, 180.0, -36000.0, 36000.0, 1, "°", def_v),
                            mt_now,
                            1.0,
                            false,
                        );
                    }
                    (ParamKind::Bool, ParamValue::Bool(b)) => {
                        let mut v = b;
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            if ui.checkbox(&mut v, localized).changed() {
                                let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": pd.id, "value": v, "keyframe": prm.is_animated()}));
                            }
                            if pd.animatable {
                                node_keyframe_nav(app, ui, clip_id, item, node.id, pd.id, &prm, mt_now, false);
                            }
                        });
                    }
                    (ParamKind::Choice(opts), ParamValue::Choice(c)) => {
                        let cur_lbl = opts.get(c as usize).copied().unwrap_or("");
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(localized);
                            craft_ui::ComboBox::from_id_salt(("vfx_ins_ch", node.id.0, pd.id))
                                .selected_text(cur_lbl)
                                .show_ui(ui, |ui| {
                                    for (i, opt) in opts.iter().enumerate() {
                                        if ui.selectable_label(c as usize == i, *opt).clicked() {
                                            let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": pd.id, "value": i}));
                                        }
                                    }
                                });
                            if pd.animatable {
                                node_keyframe_nav(app, ui, clip_id, item, node.id, pd.id, &prm, mt_now, false);
                            }
                        });
                    }
                    (ParamKind::Point, ParamValue::Vec2(p)) => {
                        let (mut px, mut py) = (p.x, p.y);
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(localized);
                            let rx = ui.add(egui::DragValue::new(&mut px).speed(1.0).prefix("X ").fixed_decimals(1));
                            let ry = ui.add(egui::DragValue::new(&mut py).speed(1.0).prefix("Y ").fixed_decimals(1));
                            if rx.changed() || ry.changed() {
                                let _ = app.session.execute(
                                    "vfxGraph.setNodeParam",
                                    json!({"clip": clip_id.0, "node": node.id.0, "param": pd.id, "value": [px, py], "keyframe": prm.is_animated(), "merge": true}),
                                );
                            }
                            if pd.animatable {
                                node_keyframe_nav(app, ui, clip_id, item, node.id, pd.id, &prm, mt_now, false);
                            }
                        });
                    }
                    (ParamKind::Color, ParamValue::Color(rgba)) => {
                        let mut c = rgba;
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(localized);
                            if ui.color_edit_button_rgba_unmultiplied(&mut c).changed() {
                                let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": pd.id, "value": c, "keyframe": prm.is_animated()}));
                            }
                            if pd.animatable {
                                node_keyframe_nav(app, ui, clip_id, item, node.id, pd.id, &prm, mt_now, false);
                            }
                        });
                    }
                    (ParamKind::Text, ParamValue::Text(s)) => {
                        let mut buf = s;
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(localized);
                            if ui.text_edit_singleline(&mut buf).changed() {
                                let _ = app.session.execute("vfxGraph.setNodeParam", json!({"clip": clip_id.0, "node": node.id.0, "param": pd.id, "value": buf}));
                            }
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}
