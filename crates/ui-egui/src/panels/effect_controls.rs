//! Effect Controls: the selected clip's effects (fixed Motion/Opacity/Time Remapping first, as in
//! Premiere, then standard effects), generated from parameter schemas, with stopwatches and a
//! keyframe lane on the right. Also hosts the Color Grading panel body (same editor, grouped).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use filmcraft_project::{ClipId, EffectInstance, ParamKind, ParamValue, TrackItem, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::state::KeyframeRef;
use crate::theme::Tokens;
use crate::widgets::HotNumberOptions;

#[path = "effect_controls_selection.rs"]
mod selection;
pub(crate) use selection::Interaction;

const ROW_H: f32 = 22.0;
/// The narrowest the effect list and the keyframe area can be dragged (#643): the list's rows are
/// laid out for at least 260 points (values from 150 points in, the reset button at the end).
const MIN_LIST_W: f32 = 260.0;
const MIN_LANE_W: f32 = 60.0;
/// Keyframe lane header: the time ruler (beside the Source / Sequence pills) and the clip's bar.
const RULER_H: f32 = 24.0;
const CLIP_BAR_H: f32 = 18.0;
/// Distance between the keyframe navigator's buttons (◀ ◆ ▶).
const NAV_STEP: f32 = 16.0;
/// The Y curve of a point parameter's value graph (X uses the accent colour).
const Y_CURVE: Color32 = Color32::from_rgb(0x60, 0xc0, 0x80);
/// Properties panel: the keyframe diamonds' column, from a row's right edge (the arrows sit one
/// [`NAV_STEP`] to either side of it, and a section's reset button above it).
const PROPS_NAV_X: f32 = 26.0;
/// Properties panel: where a row's value fields end, left of the navigator.
const PROPS_VALUE_R: f32 = PROPS_NAV_X + NAV_STEP + 12.0;

/// Timeline time at which the clip shows media time `m` (where a keyframe sits in the sequence).
fn timeline_time_of(it: &TrackItem, m: Tick) -> Tick {
    let offset = if it.reverse { it.source_out() - m - Tick(1) } else { m - it.source_in };
    it.start + Tick((offset.0 as f64 / it.speed.abs().max(1e-6)).round() as i64)
}

// The press frame can include movement before the button went down. Start from the press
// origin so that only gesture motion enters the preview, including same-frame movement.
fn graph_drag_offset(ui: &egui::Ui, response: &egui::Response, previous: egui::Vec2) -> egui::Vec2 {
    if response.drag_started_by(egui::PointerButton::Primary) {
        response
            .interact_pointer_pos()
            .zip(ui.input(|i| i.pointer.press_origin()))
            .map_or_else(|| response.drag_delta(), |(position, origin)| position - origin)
    } else {
        previous + response.drag_delta()
    }
}

/// Whether `k` names a keyframe of the clip `it` (its effect, parameter and media time).
fn keyframe_exists(it: &TrackItem, k: &KeyframeRef) -> bool {
    let Some(e) = it.effects.get(k.effect) else { return false };
    let param = match k.mask {
        Some(m) => e.masks.get(m).and_then(|m| m.param(&k.param)),
        None => e.params.get(&k.param),
    };
    param.is_some_and(|p| p.keyframes.iter().any(|kf| kf.time == k.time))
}

/// The clip's media time the panel shows its values at: the playhead's, held inside the clip.
fn effect_time_now(it: &TrackItem, ph: Tick) -> Tick {
    it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)))
}

/// Whether `k` sits on a row the panel draws disabled at the playhead `ph` (Scale Width while
/// Uniform Scale is on, #4381). Such keyframes can't be selected, so a group drag or a selection's
/// interpolation change leaves them alone.
fn keyframe_disabled(it: &TrackItem, k: &KeyframeRef, ph: Tick) -> bool {
    k.mask.is_none() && it.effects.get(k.effect).is_some_and(|e| ignored_by_uniform_scale(it, e, &k.param, effect_time_now(it, ph)))
}

/// Edit ▸ Select All (⌘A) with Effect Controls focused selects every keyframe the panel shows
/// instead of the Timeline's clips (#1241). False when the panel is not focused or shows none.
pub(crate) fn select_all_keyframes(app: &mut FilmcraftApp) -> bool {
    if app.ui.focused != crate::dock::PanelKind::EffectControls {
        return false;
    }
    let mut keys = Vec::new();
    let ph = app.session.playhead();
    for (clip, it, _) in selected_clips(app) {
        let mt = effect_time_now(&it, ph);
        for (effect, e) in it.effects.iter().enumerate() {
            let key = |param: &str, mask, time| KeyframeRef { clip: clip.0, effect, param: param.to_string(), mask, time };
            // not a disabled row's (Scale Width while Uniform Scale is on)
            for (param, p) in e.params.iter().filter(|(param, _)| !ignored_by_uniform_scale(&it, e, param, mt)) {
                keys.extend(p.keyframes.iter().map(|kf| key(param, None, kf.time)));
            }
            for (m, mask) in e.masks.iter().enumerate() {
                for param in filmcraft_project::mask::MASK_PARAMS {
                    keys.extend(mask.param(param).into_iter().flat_map(|p| &p.keyframes).map(|kf| key(param, Some(m), kf.time)));
                }
            }
        }
    }
    if keys.is_empty() {
        return false;
    }
    app.ui.keyframe_selection = keys;
    true
}

fn selected_clip(app: &FilmcraftApp) -> Option<(ClipId, TrackItem, TrackKind)> {
    selected_clips(app).into_iter().next()
}

/// The selected clip (video first) and the audio linked to it when that audio is selected or
/// linked selection is on — so clip audio FX (Parametric EQ, Custom Setup) stay visible on A/V pairs.
pub(crate) fn selected_clips(app: &FilmcraftApp) -> Vec<(ClipId, TrackItem, TrackKind)> {
    let Some(seq) = app.session.active_sequence() else { return Vec::new() };
    let picked: Vec<(ClipId, &TrackItem, TrackKind)> = app
        .session
        .state
        .selection
        .iter()
        .filter_map(|c| {
            let (tid, it) = seq.find_item(*c)?;
            Some((*c, it, seq.track(tid)?.kind))
        })
        .collect();
    let Some(&(clip, it, kind)) = picked.iter().find(|p| p.2 == TrackKind::Video).or(picked.first()) else { return Vec::new() };
    let mut out = vec![(clip, it.clone(), kind)];
    if kind == TrackKind::Video {
        let linked = picked.iter().find(|p| p.2 == TrackKind::Audio && p.1.link.is_some() && p.1.link == it.link).copied().or_else(|| {
            if !app.session.state.linked_selection {
                return None;
            }
            let link = it.link?;
            seq.audio_tracks.iter().find_map(|t| t.items.iter().find(|i| i.link == Some(link)).map(|i| (i.id, i, TrackKind::Audio)))
        });
        if let Some((ac, ai, ak)) = linked {
            out.push((ac, ai.clone(), ak));
        }
    }
    out
}

/// An effect (or preset) dragged from the Effects panel and released over this panel is applied to
/// the clip shown here, so it need not be aimed at the clip in the Timeline (#2326). Transitions
/// need an edge, so they stay Timeline drops.
fn drop_effect(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, clips: &[(ClipId, TrackItem, TrackKind)]) {
    let Some(effect) = crate::panels::dragged_effect(ui) else { return };
    let is_transition = filmcraft_project::find_effect(&effect)
        .is_some_and(|d| matches!(d.kind, filmcraft_project::EffectKind::VideoTransition | filmcraft_project::EffectKind::AudioTransition));
    let ctx = ui.ctx().clone();
    let over = ctx.pointer_hover_pos().is_some_and(|p| rect.contains(p) && ctx.layer_id_at(p) == Some(ui.layer_id()));
    if is_transition || !over {
        return;
    }
    app.auto.add("effectControls.dropTarget", rect, "drop an effect here");
    ui.painter().rect_stroke(rect.shrink(1.0), 3.0, Stroke::new(2.0, app.tokens.accent), egui::StrokeKind::Inside);
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    // the video clip first; its linked audio clip takes audio effects the video clip refuses
    let mut result = Err(filmcraft_engine::EngineError::NoSequence);
    for (clip, _, _) in clips {
        let params_clips = json!([clip.0]);
        result = if let Some(name) = effect.strip_prefix("preset:") {
            app.session.execute("presets.apply", json!({"preset": name, "clips": params_clips}))
        } else if let Some(name) = effect.strip_prefix(crate::panels::LUMETRI_PRESET_PREFIX) {
            app.session.execute("lumetri.applyPreset", json!({"name": name, "clips": params_clips}))
        } else {
            app.session.execute("effects.apply", json!({"effect": effect, "clips": params_clips}))
        };
        if result.is_ok() {
            break;
        }
    }
    if let Err(e) = result {
        app.ui.status = e.to_string();
    }
    crate::panels::clear_drag(ui);
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let clips = selected_clips(app);
    // keep only selected keyframes that still exist on a clip shown here, on a row that isn't
    // disabled (selected before Uniform Scale was turned on, say)
    let ph = app.session.playhead();
    app.ui.keyframe_selection.retain(|k| clips.iter().any(|(c, it, _)| c.0 == k.clip && keyframe_exists(it, k) && !keyframe_disabled(it, k, ph)));
    let Some((clip, it, _)) = clips.first().cloned() else {
        // a transition clicked in the Timeline (#430)
        if let Some(id) = crate::panels::transition_controls::selected(app) {
            crate::panels::transition_controls::effect_controls(app, ui, rect, id);
            return;
        }
        crate::dock::placeholder(ui, rect, &t, tl!("(no clip selected)"));
        return;
    };
    drop_effect(app, ui, rect, &clips);
    // Keep the immutable project snapshot alive while UI commands mutate the session.
    // Cloning a sequence here copies every clip and effect on every redraw.
    let project = app.session.project.clone();
    let Some(seq) = app.session.state.active_sequence.and_then(|id| project.sequence(id)) else {
        crate::dock::placeholder(ui, rect, &t, tl!("(no sequences)"));
        return;
    };
    // the divider between the effect list and the keyframe area; dragging it sets the list's width
    // (#643), each side keeping a minimum width
    // (the keyframe area has 4 points of margin left of it and 6 right)
    let max_list = (rect.width() - MIN_LANE_W - 10.0).max(MIN_LIST_W);
    let list_w = if app.ui.effect_controls_split > 0.0 { app.ui.effect_controls_split } else { (rect.width() * 0.58).max(260.0) };
    let list_w = list_w.clamp(MIN_LIST_W, max_list);
    let split = rect.min.x + list_w;
    let head = Rect::from_min_size(rect.min + vec2(8.0, 4.0), vec2(split - rect.min.x - 12.0, 24.0));
    // Premiere: two pill tabs — "Source · clip" and "Sequence · clip" (active)
    let pill = |ui: &mut egui::Ui, r: Rect, text: &str, active: bool| {
        ui.painter().rect_filled(r, 4.0, if active { t.pill_active_bg } else { t.panel_bg });
        if !active {
            ui.painter().rect_stroke(r, 4.0, Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
        }
        let cp = ui.painter().with_clip_rect(r.shrink(2.0));
        cp.text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, text, Tokens::ui(11.5), if active { t.text } else { t.text_dim });
    };
    let half = (head.width() - 6.0) / 2.0;
    pill(ui, Rect::from_min_size(head.min, vec2(half, 24.0)), &tlf!("Source · {name}", name = it.name), false);
    pill(ui, Rect::from_min_size(head.min + vec2(half + 6.0, 0.0), vec2(half, 24.0)), &format!("{} · {}", seq_name(app), it.name), true);
    // keyframe lane header, as Premiere's: a time ruler over the clip's stretch of the sequence,
    // which the playhead's handle rides on, and the clip's bar under it
    let lane = Rect::from_min_max(pos2(split + 4.0, rect.min.y + 4.0), pos2(rect.max.x - 6.0, rect.max.y - 26.0));
    ui.painter().rect_filled(lane, 0.0, t.tl_bg);
    let dur = it.duration.0.max(1) as f64;
    let lx = |tk: Tick| -> f32 { lane.min.x + (((tk - it.start).0 as f64 / dur) as f32).clamp(0.0, 1.0) * lane.width() };
    let rate = seq.settings.frame_rate;
    let ruler = Rect::from_min_max(lane.min, pos2(lane.max.x, lane.min.y + RULER_H));
    let bar = Rect::from_min_max(pos2(lane.min.x, ruler.max.y + 4.0), pos2(lane.max.x, ruler.max.y + 4.0 + CLIP_BAR_H));
    let scrub = Rect::from_min_max(ruler.min, bar.max);
    app.auto.add("effectControls.lane", lane, "keyframe lane");
    app.auto.add("effectControls.ruler", scrub, "time ruler");
    let body = Rect::from_min_max(pos2(rect.min.x, head.max.y + 4.0), pos2(split, rect.max.y - 26.0));
    let mut actions: Vec<(String, Value)> = Vec::new();
    let selection_area = Rect::from_min_max(pos2(lane.min.x, bar.max.y), lane.max).intersect(ui.clip_rect());
    let mut keys = Interaction::begin(app, ui, selection_area, clip.0);
    // the rows span the list and the keyframe area: the wheel scrolls anywhere over them and the
    // scrollbar sits at the panel's right edge (#643)
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(body.min, pos2(lane.max.x, body.max.y))).id_salt("ec-body"));
    bui.set_clip_rect(Rect::from_min_max(body.min, pos2(rect.max.x, body.max.y)));
    let scroll_out = egui::ScrollArea::vertical().id_salt("ec-scroll").auto_shrink([false, false]).show(&mut bui, |bui| {
        for (clip, it, kind) in &clips {
            let clip = *clip;
            let (hr, _) = bui.allocate_exact_size(vec2(body.width(), 18.0), Sense::hover());
            let heading = if *kind == TrackKind::Video { tl!("Video") } else { tl!("Audio") };
            bui.painter().text(pos2(hr.min.x + 8.0, hr.center().y), Align2::LEFT_CENTER, heading, Tokens::semibold(11.5), t.text_dim);
            // a graphic with Responsive Design – Time: its clip effects below keep media time (#3361)
            if *kind == TrackKind::Video && it.graphic.as_deref().is_some_and(|g| g.has_responsive_time()) {
                let hint = tl!("Responsive Design – Time remaps the graphic's layer keyframes; clip effects like Opacity and Motion keep media time");
                let ir = Rect::from_center_size(pos2(hr.max.x - 14.0, hr.center().y), vec2(18.0, 18.0));
                // a small button: keyboard-focusable, with an accessible name, tooltip on hover or focus
                let resp = bui.put(ir, egui::Button::new("").frame(false));
                icons::paint(bui.painter(), ir.shrink(2.0), Icon::Info, t.text_dim);
                if resp.ctx.has_accesskit_node(resp.id) {
                    resp.ctx.accesskit_node_builder(resp.id, |node| node.set_label(hint));
                }
                if resp.has_focus() {
                    resp.show_tooltip_text(hint);
                } else {
                    resp.on_hover_text(hint);
                }
                app.auto.add("effectControls.graphicTimeHint", ir, hint);
            }
            let mt_now = effect_time_now(it, ph);
            // Premiere lists the fixed effects (Motion, Opacity, Time Remapping / Volume…) first.
            let mut order: Vec<usize> = (0..it.effects.len()).collect();
            order.sort_by_key(|i| !it.effects[*i].def().is_some_and(|d| d.intrinsic));
            let mut after_fixed = false;
            for idx in order {
                let e = &it.effects[idx];
                let Some(def) = e.def() else { continue };
                // the first effect added from the library after the fixed ones gets a line on top
                // (#2328); drawn on its edge, so no row moves
                let first_added = if def.intrinsic {
                    after_fixed = true;
                    false
                } else {
                    std::mem::take(&mut after_fixed)
                };
                let key = format!("ec:{}:{}", clip.0, idx);
                let open = app.ui.expanded_fx.contains(&key);
                let selected = app.session.state.selected_effect.as_ref().is_some_and(|s| s.clip == clip && s.index(&it.effects) == Some(idx));
                let (r, resp) = bui.allocate_exact_size(vec2(body.width(), ROW_H), Sense::click());
                if selected {
                    bui.painter().rect_filled(crate::panels::effect_controls::row_fill(r), 0.0, t.row_selected);
                } else if resp.hovered() {
                    bui.painter().rect_filled(crate::panels::effect_controls::row_fill(r), 0.0, t.hover);
                }
                row_line(bui, r, &lane, &t);
                if first_added {
                    let line = Rect::from_min_max(pos2(r.min.x, r.min.y - 1.0), pos2(r.max.x, r.min.y + 1.0));
                    bui.painter().rect_filled(line, 0.0, t.accent.gamma_multiply(0.8));
                    let which = if *kind == TrackKind::Video { "video" } else { "audio" };
                    app.auto.add(&format!("effectControls.addedEffects.{which}"), line, "Added effects");
                }
                // only the arrow twirls the effect open or shut; the rest of the header selects it
                let tw = Rect::from_center_size(pos2(r.min.x + 10.0, r.center().y), vec2(10.0, 10.0));
                icons::paint(bui.painter(), tw, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_dim);
                let twresp = bui.interact(tw.expand(3.0), egui::Id::unique(("fx-twirl", clip.0, idx)), Sense::click());
                app.auto.add(&format!("effectControls.effect.{}.twirl", e.effect), tw, "twirl");
                // fx enable toggle
                let fxr = Rect::from_center_size(pos2(r.min.x + 28.0, r.center().y), vec2(18.0, 14.0));
                let fxresp = bui.interact(fxr, egui::Id::unique(("fxen", clip.0, idx)), Sense::click());
                bui.painter().text(fxr.center(), Align2::CENTER_CENTER, "fx", Tokens::semibold(10.5), if e.enabled { t.text } else { t.text_faint });
                if !e.enabled {
                    bui.painter().line_segment([fxr.left_bottom(), fxr.right_top()], Stroke::new(1.0, t.text_faint));
                }
                if fxresp.clicked() {
                    actions.push(("effects.toggleEnabled".into(), json!({"clip": clip.0, "index": idx})));
                }
                // (the reset button sits at the row's right end)
                row_label(bui, pos2(r.min.x + 42.0, r.center().y), crate::i18n::t(def.name), r.max.x - 26.0 - (r.min.x + 42.0), Tokens::ui(12.0), t.text);
                // reset button
                let rr = Rect::from_center_size(pos2(r.max.x - 14.0, r.center().y), vec2(16.0, 16.0));
                let rresp = bui.interact(rr, egui::Id::unique(("fxreset", clip.0, idx)), Sense::click()).on_hover_text(tl!("Reset Effect"));
                icons::paint(bui.painter(), rr.shrink(2.0), Icon::Reset, if rresp.hovered() { t.text } else { t.text_dim });
                if rresp.clicked() {
                    actions.push(("effects.reset".into(), json!({"clip": clip.0, "index": idx})));
                }
                app.auto.add(&format!("effectControls.effect.{}", e.effect), r, def.name);
                if twresp.clicked() {
                    if open {
                        app.ui.expanded_fx.retain(|k| *k != key);
                    } else {
                        app.ui.expanded_fx.push(key.clone());
                    }
                } else if resp.clicked() && !fxresp.clicked() && !rresp.clicked() {
                    actions.push(("effects.select".into(), json!({"clip": clip.0, "effect": idx})));
                }
                let mut save_preset = false;
                let fx_id = e.effect.clone();
                resp.context_menu(|ui| {
                    let sp = ui.button(tl!("Save Preset…"));
                    app.auto.add(&format!("effectControls.effect.{fx_id}.savePreset"), sp.rect, "Save Preset…");
                    if sp.clicked() {
                        save_preset = true;
                        ui.close();
                    }
                    if !def.intrinsic && ui.button(tl!("Clear")).clicked() {
                        actions.push(("effects.remove".into(), json!({"clip": clip.0, "index": idx})));
                        ui.close();
                    }
                });
                if save_preset {
                    crate::panels::presets::open_save(app, clip.0, vec![idx], def.name);
                }
                if !open {
                    continue;
                }
                if crate::panels::audio_fx_editor::has_editor(&e.effect) {
                    custom_setup_row(app, bui, body, &lane, clip, idx, &e.effect);
                }
                for pd in &def.params {
                    let mut rows = |bui: &mut egui::Ui| {
                        param_row(app, bui, body, clip, idx, e, None, pd, mt_now, &mut actions, &lane, &lx, it, &mut keys);
                        if app.ui.expanded_fx.contains(&graph_key(clip, idx, pd.id))
                            && let Some(param) = e.params.get(pd.id)
                            && param.is_animated()
                            && graphable(&param.value)
                        {
                            graph_rows(app, bui, body, clip, idx, None, pd, param, &lane, it, &mut actions);
                        }
                    };
                    // Scale Width while Uniform Scale is on: greyed out, neither its value nor its
                    // graphs dragged or typed (#4381). Only that row gets a scope: an empty one
                    // (a parameter the instance lacks) would still add a row gap.
                    if ignored_by_uniform_scale(it, e, pd.id, mt_now) {
                        bui.add_enabled_ui(false, rows);
                    } else {
                        rows(bui);
                    }
                }
                if crate::panels::masks::maskable(e) {
                    crate::panels::masks::effect_rows(app, bui, body, clip, idx, e, mt_now, &mut actions, &lane, &lx, it, &mut keys);
                }
            }
        }
    });
    let _ = scroll_out;
    // the ruler and the clip's bar, painted after the rows so rows scrolled up under them (their
    // lines and keyframes) stay hidden
    ui.painter().rect_filled(Rect::from_min_max(lane.min, pos2(lane.max.x, bar.max.y)), 0.0, t.tl_bg);
    paint_ruler(ui.painter(), ruler, &it, rate, seq.settings.drop_frame, &t);
    ui.painter().rect_filled(bar.shrink2(vec2(0.0, 2.0)), 2.0, t.clip_bar_bg);
    ui.painter().with_clip_rect(bar).text(pos2(bar.min.x + 4.0, bar.center().y), Align2::LEFT_CENTER, &it.name, Tokens::ui(10.0), t.text);
    // click or drag anywhere on the ruler (or the bar) to move the playhead (after the rows, which
    // reach under it, so it gets the pointer first)
    let sresp = ui.interact(scrub, egui::Id::unique(("ec-scrub", clip.0)), Sense::click_and_drag());
    if (sresp.dragged() || sresp.clicked())
        && let Some(pos) = sresp.interact_pointer_pos()
    {
        let f = ((pos.x - lane.min.x) / lane.width()).clamp(0.0, 1.0) as f64;
        let tk = rate.snap_nearest(it.start + Tick((f * it.duration.0 as f64) as i64));
        app.stop_for_scrub();
        app.session.set_playhead(tk);
    }
    // the divider: a line the height of the panel, a resize cursor and a wider grab area
    let divider = Rect::from_min_max(pos2(split - 2.0, rect.min.y), pos2(split + 4.0, rect.max.y - 26.0));
    let dresp = ui.interact(divider, egui::Id::unique("ec-divider"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    if dresp.dragged() {
        app.ui.effect_controls_split = (list_w + dresp.drag_delta().x).clamp(MIN_LIST_W, max_list);
    }
    let dcol = if dresp.hovered() || dresp.dragged() { t.accent } else { t.separator };
    ui.painter().line_segment([pos2(split + 1.5, rect.min.y), pos2(split + 1.5, rect.max.y - 26.0)], Stroke::new(1.0, dcol));
    app.auto.add("effectControls.divider", divider, "divider");
    // playhead: a handle on the ruler and a line down the lane, while it is on the clip
    if ph >= it.start && ph <= it.end() {
        let px = lx(ph);
        let (top, tip) = (ruler.max.y - 13.0, ruler.max.y);
        let head = vec![pos2(px - 5.0, top), pos2(px + 5.0, top), pos2(px + 5.0, tip - 5.0), pos2(px, tip), pos2(px - 5.0, tip - 5.0)];
        ui.painter().add(egui::Shape::convex_polygon(head, t.playhead, Stroke::NONE));
        ui.painter().line_segment([pos2(px, tip), pos2(px, lane.max.y)], Stroke::new(1.0, t.playhead));
        app.auto.add("effectControls.playhead", Rect::from_min_max(pos2(px - 5.0, top), pos2(px + 5.0, tip)), "playhead");
    }
    // footer timecode
    let tc = filmcraft_time::format_time(ph, seq.settings.frame_rate, seq.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48000);
    ui.painter().text(pos2(rect.min.x + 10.0, rect.max.y - 13.0), Align2::LEFT_CENTER, tc, Tokens::mono(13.0), t.timecode);
    keys.finish(app, ui);
    run(app, ui.ctx(), actions);
}

/// The thin line under a row, across the names, the values and the keyframe lane, so a row can be
/// followed across the panel, as in Premiere (#640). It runs through the middle of the spacing
/// between rows, so each row sits centred between its two lines.
pub(crate) fn row_line(ui: &egui::Ui, r: Rect, lane: &Rect, t: &Tokens) {
    let y = r.max.y + ui.spacing().item_spacing.y / 2.0;
    ui.painter().line_segment([pos2(r.min.x, y), pos2(lane.max.x, y)], Stroke::new(1.0, t.separator));
}

/// A row's label, cut off with "…" where it would run into what follows it, as Premiere does when
/// the effect list is narrow (#643).
pub(crate) fn row_label(ui: &egui::Ui, left_center: Pos2, text: &str, max_w: f32, font: egui::FontId, color: Color32) {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), egui::TextFormat::simple(font, color));
    job.wrap.max_width = max_w.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(pos2(left_center.x, left_center.y - g.size().y / 2.0), g, color);
}

/// Make a dropdown or a checkbox in a row fit inside the row, centred between its row lines.
pub(crate) fn fit_to_row(ui: &mut egui::Ui) {
    ui.spacing_mut().interact_size.y = ROW_H - 4.0;
    ui.spacing_mut().button_padding.y = 1.0;
}

/// Premiere's "Custom Setup ▸ Edit…" row: opens the effect's Clip Fx Editor window.
fn custom_setup_row(app: &mut FilmcraftApp, ui: &mut egui::Ui, body: Rect, lane: &Rect, clip: ClipId, idx: usize, effect: &str) {
    let t = app.tokens;
    let (r, _) = ui.allocate_exact_size(vec2(body.width(), ROW_H), Sense::hover());
    row_line(ui, r, lane, &t);
    ui.painter().text(pos2(r.min.x + 42.0, r.center().y), Align2::LEFT_CENTER, tl!("Custom Setup"), Tokens::ui(12.0), t.text_dim);
    let br = Rect::from_min_size(pos2(r.min.x + 160.0, r.min.y + 2.0), vec2(60.0, ROW_H - 4.0));
    let resp = ui.interact(br, egui::Id::unique(("fx-custom-setup", clip.0, idx)), Sense::click());
    ui.painter().rect_filled(br, 3.0, if resp.hovered() { t.hover } else { t.field_bg });
    ui.painter().rect_stroke(br, 3.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    ui.painter().text(br.center(), Align2::CENTER_CENTER, tl!("Edit…"), Tokens::ui(11.5), t.text);
    app.auto.add(&format!("effectControls.effect.{effect}.edit"), br, "Edit…");
    if resp.clicked() {
        crate::panels::audio_fx_editor::open(app, crate::panels::audio_fx_editor::FxTarget::Clip { clip: clip.0, index: idx });
    }
}

fn seq_name(app: &FilmcraftApp) -> String {
    app.session.state.active_sequence.and_then(|s| app.session.project.item(s)).map(|i| i.name.clone()).unwrap_or_default()
}

/// The keyframe lane's time ruler: ticks and sequence timecode across the clip's stretch of the
/// timeline (the lane shows exactly the clip, so the ruler starts at the clip's start).
fn paint_ruler(p: &egui::Painter, ruler: Rect, it: &TrackItem, rate: filmcraft_time::FrameRate, drop_frame: bool, t: &Tokens) {
    let p = p.with_clip_rect(ruler);
    let dur = it.duration.0.max(1) as f64;
    let frame_px = ruler.width() as f64 * rate.frame_duration().0.max(1) as f64 / dur;
    let base = rate.timecode_base();
    let steps = [
        1,
        2,
        5,
        10,
        base / 2,
        base,
        base * 2,
        base * 5,
        base * 10,
        base * 15,
        base * 30,
        base * 60,
        base * 120,
        base * 300,
        base * 600,
        base * 1800,
        base * 3600,
    ];
    // labels far enough apart to read; the small ticks divide the labelled ones evenly
    let label_step = steps.iter().copied().find(|s| *s > 0 && *s as f64 * frame_px >= 80.0);
    let minor = steps.iter().copied().find(|s| *s > 0 && *s as f64 * frame_px >= 8.0 && label_step.is_none_or(|l| l % s == 0));
    let (f0, f1) = (rate.frame_at(it.start), rate.frame_at(it.end()));
    let base_y = ruler.max.y - 1.0;
    for (step, h, labelled) in [(minor, 3.0, false), (label_step, 7.0, true)] {
        let Some(step) = step else { continue };
        let mut f = f0.div_euclid(step).saturating_mul(step);
        // a step is at least 8 px wide, so this covers any ruler; the cap is for damaged numbers
        for _ in 0..4096 {
            if f > f1 {
                break;
            }
            let x = ruler.min.x + ((rate.tick_of(f) - it.start).0 as f64 / dur) as f32 * ruler.width();
            p.line_segment([pos2(x, base_y - h), pos2(x, base_y)], Stroke::new(1.0, t.tl_ruler_tick));
            if labelled {
                let label = filmcraft_time::format_time(rate.tick_of(f), rate, drop_frame, filmcraft_time::TimeDisplay::Timecode, 48000);
                p.text(pos2(x, ruler.min.y + 7.0), Align2::CENTER_CENTER, label, Tokens::ui(10.0), t.tl_ruler_text);
            }
            f = f.saturating_add(step);
        }
    }
}

/// Premiere's keyframe navigator, centred on `at`: a diamond that adds a keyframe at the playhead
/// or removes the one there (filled while the playhead is on a keyframe) and, once the parameter
/// is animated, arrows to the previous and the next keyframe. `param` is `None` while the effect
/// is not on the clip yet. Automation ids: `<auto>.addKeyframe`, `.prevKeyframe`, `.nextKeyframe`.
/// Returns whether the diamond was clicked; the arrows queue their own `playhead.set`.
#[allow(clippy::too_many_arguments)]
fn keyframe_nav(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    at: Pos2,
    param: Option<&filmcraft_project::Param>,
    mt: Tick,
    it: &TrackItem,
    id: egui::Id,
    auto: &str,
    tip: &str,
    actions: &mut Vec<(String, Value)>,
) -> bool {
    let t = app.tokens;
    if let Some(param) = param.filter(|p| p.is_animated()) {
        for (d, target, name, tip) in
            [(-1.0, param.prev_keyframe(mt), "prevKeyframe", "Go to previous keyframe"), (1.0, param.next_keyframe(mt), "nextKeyframe", "Go to next keyframe")]
        {
            // the arrows almost the row's height to click, as Premiere's
            let r = Rect::from_center_size(at + vec2(d * NAV_STEP, 0.0), vec2(14.0, 20.0));
            let resp = ui.interact(r, id.with(name), Sense::click()).on_hover_text(crate::i18n::t(tip));
            let c = r.center();
            let pts = vec![c + vec2(-3.5 * d, -5.5), c + vec2(-3.5 * d, 5.5), c + vec2(3.5 * d, 0.0)];
            ui.painter().add(egui::Shape::convex_polygon(pts, if target.is_some() { t.text } else { t.text_faint }, Stroke::NONE));
            app.auto.add(&format!("{auto}.{name}"), r, tip);
            if resp.clicked()
                && let Some(k) = target
            {
                actions.push(("playhead.set".into(), json!({"time": timeline_time_of(it, k).0})));
            }
        }
    }
    let r = Rect::from_center_size(at, vec2(15.0, 15.0));
    let resp = ui.interact(r.expand(1.5), id.with("key"), Sense::click()).on_hover_text(crate::i18n::t(tip));
    if param.is_some_and(|p| p.keyframes.iter().any(|k| k.time == mt)) {
        icons::paint(ui.painter(), r, Icon::Keyframe, t.hot_text);
    } else {
        let s = 4.7;
        let outline = vec![at + vec2(0.0, -s), at + vec2(s, 0.0), at + vec2(0.0, s), at + vec2(-s, 0.0)];
        ui.painter().add(egui::Shape::closed_line(outline, Stroke::new(1.2, if resp.hovered() { t.text } else { t.text_dim })));
    }
    app.auto.add(&format!("{auto}.addKeyframe"), r, tip);
    resp.clicked()
}

/// A Properties row's keyframe navigator for `effect`.`param`, with Premiere's tooltips. The
/// diamond is `effects.addKeyframe`: it adds a keyframe at the playhead (turning animation on) or
/// removes the one there, so the value falls back to what the remaining keyframes give. An effect
/// the clip does not have yet (Crop) is applied first.
#[allow(clippy::too_many_arguments)]
fn properties_nav(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    row: Rect,
    clip: ClipId,
    it: &TrackItem,
    mt: Tick,
    effect: &str,
    param: &str,
    actions: &mut Vec<(String, Value)>,
) {
    let prm = it.effect(effect).and_then(|e| e.param(param));
    let tip = match prm {
        Some(p) if p.keyframes.iter().any(|k| k.time == mt) => "Remove keyframe",
        Some(p) if p.is_animated() => "Add keyframe",
        _ => "Turn on animation and add keyframe",
    };
    let at = pos2(row.max.x - PROPS_NAV_X, row.center().y);
    let id = egui::Id::unique(("props-nav", effect, param, clip.0));
    if keyframe_nav(app, ui, at, prm, mt, it, id, &format!("properties.{effect}.{param}"), tip, actions) {
        if it.effect(effect).is_none() {
            actions.push(("effects.apply".into(), json!({"clips": [clip.0], "effect": effect})));
        }
        actions.push(("effects.addKeyframe".into(), json!({"clip": clip.0, "effect": effect, "param": param})));
    }
}

/// Whether `param` is Scale Width with the effect's Uniform Scale on at media time `mt` of `it`:
/// Motion, Transform and graphic layers then scale both axes by Scale (Transform's Scale Height)
/// and the render ignores Scale Width. Uniform Scale is read exactly as the render reads it, so the
/// row is greyed out exactly when Scale Width does nothing, even with a keyframe on it (Uniform
/// Scale can't be animated, but `effects.addKeyframe` adds one) or with it missing or non-boolean:
/// - Motion (`filmcraft_render::motion_matrix`): the stored value, keyframes aside; missing = on;
/// - Transform (`gpufx`, `vfx::distort`): the stored value, keyframes aside; missing = off;
/// - graphic layers (`filmcraft_project::graphic::eval_layer`): keyframes too, at the layer's time
///   (Responsive Design – Time); missing = the definition's default (on).
fn ignored_by_uniform_scale(it: &TrackItem, e: &EffectInstance, param: &str, mt: Tick) -> bool {
    if param != "scale_width" {
        return false;
    }
    let uniform = e.param("uniform_scale");
    if filmcraft_project::graphic::is_layer(e) {
        let t = filmcraft_render::graphic_clip::graphic_time(it, mt).0;
        let value = match uniform {
            Some(p) => Some(p.value_at(t)),
            None => e.def().and_then(|d| d.param("uniform_scale")).map(|d| d.default.clone()),
        };
        return value.and_then(|v| v.as_bool()).unwrap_or(false);
    }
    uniform.and_then(|p| p.value.as_bool()).unwrap_or(e.effect == "motion")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn param_row(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    body: Rect,
    clip: ClipId,
    idx: usize,
    e: &EffectInstance,
    mask: Option<usize>,
    pd: &filmcraft_project::ParamDef,
    mt: Tick,
    actions: &mut Vec<(String, Value)>,
    lane: &Rect,
    lx: &dyn Fn(Tick) -> f32,
    it: &TrackItem,
    keys: &mut Interaction,
) {
    let _ = lx;
    let t = app.tokens;
    let param = match mask {
        Some(k) => e.masks.get(k).and_then(|m| m.param(pd.id)),
        None => e.params.get(pd.id),
    };
    let Some(param) = param else { return };
    // mask parameters: automation ids / widget ids get a `m<k>.` prefix, commands a `mask` field
    let pkey: String = match mask {
        Some(k) => format!("mask{k}.{}", pd.id),
        None => pd.id.to_string(),
    };
    let pkey = pkey.as_str();
    let with_mask = |mut v: Value| -> Value {
        if let Some(k) = mask {
            v["mask"] = json!(k);
        }
        v
    };
    let (r, row) = ui.allocate_exact_size(vec2(body.width(), ROW_H), if mask.is_none() { Sense::click() } else { Sense::hover() });
    row_line(ui, r, lane, &t);
    // clicking one of an effect's properties selects the effect, as clicking its header does
    if row.clicked() {
        actions.push(("effects.select".into(), json!({"clip": clip.0, "effect": idx})));
    }
    let mut x = r.min.x + 26.0;
    // twirl-down for the value/velocity graphs (animated scalar and point params)
    if param.is_animated() && graphable(&param.value) {
        let key = graph_key(clip, idx, pkey);
        let open = app.ui.expanded_fx.contains(&key);
        let tw = Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(12.0, 12.0));
        let tresp = ui.interact(tw.expand(2.0), egui::Id::unique(("twirl", clip.0, idx, pkey)), Sense::click()).on_hover_text(tl!("Show graphs"));
        icons::paint(ui.painter(), tw, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_dim);
        app.auto.add(&format!("effectControls.{}.{}.graphs", e.effect, pkey), tw, "Show graphs");
        if tresp.clicked() {
            if open {
                app.ui.expanded_fx.retain(|k| *k != key);
            } else {
                app.ui.expanded_fx.push(key);
            }
        }
    }
    if pd.animatable {
        let sw = Rect::from_center_size(pos2(x, r.center().y), vec2(14.0, 14.0));
        let resp = ui.interact(sw, egui::Id::unique(("sw", clip.0, idx, pkey)), Sense::click()).on_hover_text(tl!("Toggle animation"));
        icons::paint(ui.painter(), sw, Icon::Stopwatch, if param.is_animated() { t.accent } else { t.text_dim });
        if resp.clicked() {
            actions.push(("effects.toggleAnimation".into(), with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id}))));
        }
        app.auto.add(&format!("effectControls.{}.{}.stopwatch", e.effect, pkey), sw, "Toggle animation");
    }
    x += 14.0;
    let vx = r.min.x + (r.width() * 0.5).max(150.0);
    row_label(ui, pos2(x, r.center().y), crate::i18n::t(pd.label), vx - 6.0 - x, Tokens::ui(12.0), t.text);
    // double-clicking a property's name resets it, as the reset button at the row's end does (a
    // click on the value itself starts typing, so the name is the double-click target)
    if mask.is_none() {
        let name = Rect::from_min_max(pos2(x, r.min.y), pos2(vx - 6.0, r.max.y));
        // the instance index tells two copies of one effect apart; only the visible part of a row
        // scrolled partly out of view is registered
        let shown = name.intersect(ui.clip_rect());
        if shown.is_positive() {
            app.auto.add(&format!("effectControls.{}.{idx}.{}.name", e.effect, pkey), shown, crate::i18n::t(pd.label));
        }
        if row.double_clicked() && row.interact_pointer_pos().is_some_and(|p| name.contains(p)) {
            actions.push(("effects.resetParam".into(), json!({"clip": clip.0, "effect": idx, "param": pd.id})));
        }
    }
    let value = param.value_at(mt);
    let mut vui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(vx, r.min.y + 1.0), pos2(r.max.x - 26.0, r.max.y - 1.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let id = egui::Id::unique(("pv", clip.0, idx, pkey));
    let set = param_value_widget(app, &mut vui, e, pd, &value, clip, idx, mask, pkey, id, t, actions);
    if let Some(v) = set {
        actions.push(("effects.setParam".into(), with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": v}))));
    }
    // keyframe navigator ◀ ◆ ▶ (when animated)
    let eff_json = json!(idx);
    if pd.animatable && param.is_animated() {
        let at = pos2(r.max.x - 42.0, r.center().y);
        let auto = format!("effectControls.{}.{}", e.effect, pkey);
        if keyframe_nav(app, ui, at, Some(param), mt, it, egui::Id::unique(("knav", clip.0, idx, pkey)), &auto, "Add/Remove Keyframe", actions) {
            actions.push(("effects.addKeyframe".into(), with_mask(json!({"clip": clip.0, "effect": eff_json, "param": pd.id}))));
        }
    }
    // reset to the default, under the effect's own reset button (an animated parameter keeps its
    // keyframes and gets one at the playhead)
    if mask.is_none() {
        let rr = Rect::from_center_size(pos2(r.max.x - 14.0, r.center().y), vec2(12.0, 12.0));
        let rresp = ui.interact(rr, egui::Id::unique(("param-reset", clip.0, idx, pkey)), Sense::click()).on_hover_text(tl!("Reset Parameter"));
        icons::paint(ui.painter(), rr, Icon::Reset, if rresp.hovered() { t.text } else { t.text_dim });
        app.auto.add(&format!("effectControls.{}.{}.reset", e.effect, pkey), rr, "Reset Parameter");
        if rresp.clicked() {
            actions.push(("effects.resetParam".into(), json!({"clip": clip.0, "effect": eff_json, "param": pd.id})));
        }
    }
    // keyframes in the lane: draggable diamonds; right-click for interpolation
    if param.is_animated() {
        let y = r.center().y;
        let dur = it.duration.0.max(1) as f64;
        for k in &param.keyframes {
            let tl = timeline_time_of(it, k.time);
            let f = ((tl - it.start).0 as f64 / dur) as f32;
            if !(-0.01..=1.01).contains(&f) {
                continue;
            }
            let id = egui::Id::unique(("kf", clip.0, idx, pkey, k.time.0));
            let original = Rect::from_center_size(pos2(lane.min.x + f * lane.width(), y), vec2(15.0, 15.0));
            let resp = ui.interact(original.expand(2.0), id, Sense::click_and_drag());
            let reference = KeyframeRef { clip: clip.0, effect: idx, param: pd.id.to_string(), mask, time: k.time };
            let kr = keys.key(app, ui, reference.clone(), it, *lane, original, &resp, k.interp);
            app.auto.add(&format!("effectControls.{}.{}.keyframe.{}", e.effect, pkey, k.time.0), kr, "keyframe");
            let selected = app.ui.keyframe_selection.contains(&reference);
            let col = if selected { t.hot_text } else { Color32::from_rgb(0xb0, 0xb0, 0xb0) };
            selection::paint(ui.painter(), kr, k.interp, col);
            if resp.clicked_by(egui::PointerButton::Primary) {
                actions.push(("playhead.set".into(), json!({"time": tl.0})));
            }
            resp.context_menu(|ui| {
                keyframe_menu(app, ui, KeyframeRef { clip: clip.0, effect: idx, param: pd.id.to_string(), mask, time: k.time }, k.interp, actions);
            });
        }
    }
}

/// The value editor of one parameter row: number, point, checkbox, choice, colour (with the
/// eyedropper), path or text field. Returns the edited value, if any.
#[allow(clippy::too_many_arguments)]
fn param_value_widget(
    app: &mut FilmcraftApp,
    vui: &mut egui::Ui,
    e: &EffectInstance,
    pd: &filmcraft_project::ParamDef,
    value: &ParamValue,
    clip: ClipId,
    idx: usize,
    mask: Option<usize>,
    pkey: &str,
    id: egui::Id,
    t: Tokens,
    actions: &mut Vec<(String, Value)>,
) -> Option<Value> {
    let mut set: Option<Value> = None;
    match (&pd.kind, value) {
        (ParamKind::Float { min, max, soft_min, soft_max, unit, decimals }, ParamValue::Float(v)) => {
            let speed = ((soft_max - soft_min) / 400.0).max(0.01);
            let (_, nv) =
                crate::widgets::hot_number(vui, crate::i18n::t(pd.label), id, *v, HotNumberOptions::new(speed, (*min, *max), *decimals as usize, unit, &t));
            if let Some(nv) = nv {
                set = Some(json!(nv));
            }
        }
        (ParamKind::Angle, ParamValue::Float(v)) => {
            let (_, nv) = crate::widgets::hot_number(vui, crate::i18n::t(pd.label), id, *v, HotNumberOptions::new(0.5, (-36000.0, 36000.0), 1, "°", &t));
            if let Some(nv) = nv {
                set = Some(json!(nv));
            }
        }
        (ParamKind::Point, ParamValue::Vec2(p)) => {
            let (_, nx) = crate::widgets::hot_number(
                vui,
                &format!("{} X", crate::i18n::t(pd.label)),
                id.with("x"),
                p.x,
                HotNumberOptions::new(1.0, (-100_000.0, 100_000.0), 1, "", &t),
            );
            let (_, ny) = crate::widgets::hot_number(
                vui,
                &format!("{} Y", crate::i18n::t(pd.label)),
                id.with("y"),
                p.y,
                HotNumberOptions::new(1.0, (-100_000.0, 100_000.0), 1, "", &t),
            );
            if nx.is_some() || ny.is_some() {
                set = Some(json!([nx.unwrap_or(p.x), ny.unwrap_or(p.y)]));
            }
        }
        (ParamKind::Bool, ParamValue::Bool(b)) => {
            let mut v = *b;
            fit_to_row(vui);
            if crate::widgets::accessible_name(vui.checkbox(&mut v, ""), crate::i18n::t(pd.label)).changed() {
                set = Some(json!(v));
            }
        }
        (ParamKind::Choice(opts), ParamValue::Choice(c)) => {
            let mut sel = *c as usize;
            fit_to_row(vui);
            // (narrower when the effect list is narrow, so it stays left of the divider)
            let w = vui.available_width().min(130.0);
            let combo = craft_ui::ComboBox::from_id_salt(id).selected_text(opts.get(sel).map_or("", |o| crate::i18n::t(o))).width(w).show_ui(vui, |ui| {
                for (i, o) in opts.iter().enumerate() {
                    if ui.selectable_value(&mut sel, i, crate::i18n::t(o)).changed() {
                        set = Some(json!(i));
                    }
                }
            });
            crate::widgets::accessible_name(combo.response, crate::i18n::t(pd.label));
        }
        (ParamKind::Color, ParamValue::Color(c)) => {
            let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
            if egui::color_picker::color_edit_button_rgba(vui, &mut rgba, egui::color_picker::Alpha::Opaque).changed() {
                set = Some(json!([rgba.r(), rgba.g(), rgba.b(), rgba.a()]));
            }
            // Eyedropper: arm it, then click a pixel in the Program monitor (Esc cancels)
            let target = crate::state::Eyedropper { clip: clip.0, effect: idx, param: pd.id.to_string(), mask, kind: crate::state::EyedropperKind::ColorParam };
            let armed = app.ui.eyedropper.as_ref() == Some(&target);
            let drop = icons::button(vui, Icon::Eyedropper, 18.0, armed, &t, tl!("Eyedropper (pick a color from the Program monitor)"));
            app.auto.add(&format!("effectControls.{}.{}.eyedropper", e.effect, pkey), drop.rect, "Eyedropper");
            if drop.clicked() {
                app.ui.eyedropper = if armed { None } else { Some(target) };
            }
        }
        (ParamKind::Path, _) => {
            crate::panels::masks::path_value(app, vui, clip, idx, mask, actions);
        }
        // a text layer's Font and Font Style pick from the installed families and styles, as the
        // Text panel's drop-downs do (#3539)
        (ParamKind::Text, ParamValue::Text(current)) if e.effect == filmcraft_project::graphic::TEXT_LAYER && matches!(pd.id, "font" | "font_style") => {
            if !filmcraft_text::fonts::system_scanned() {
                filmcraft_text::fonts::scan_system();
            }
            let fams = filmcraft_text::families();
            let salt = ("ec-font", clip.0, idx, pd.id);
            // the value column can be narrower than the usual drop-down; keep the button inside it
            let w = 150.0_f32.min(vui.available_width()).max(40.0);
            let prefix = format!("effectControls.text.{}.{}.option", idx, pd.id);
            let combo = if pd.id == "font" {
                craft_ui::ComboBox::from_id_salt(salt).selected_text(current).width(w).height(400.0).show_ui(vui, |ui| {
                    for (f, _) in &fams {
                        let r = ui.selectable_label(f == current, f);
                        app.auto.add(&format!("{prefix}.{f}"), r.rect, f);
                        if r.clicked() {
                            set = Some(json!(f));
                        }
                    }
                })
            } else {
                let family = match e.param("font").map(|p| &p.value) {
                    Some(ParamValue::Text(f)) => f.as_str(),
                    _ => "",
                };
                let mut styles: Vec<&str> =
                    fams.iter().find(|(f, _)| f.eq_ignore_ascii_case(family)).map(|(_, s)| s.iter().map(String::as_str).collect()).unwrap_or_default();
                for extra in ["Bold", "Italic", "Bold Italic"] {
                    if !styles.contains(&extra) {
                        styles.push(extra);
                    }
                }
                craft_ui::ComboBox::from_id_salt(salt).selected_text(current).width(w).show_ui(vui, |ui| {
                    for st in styles {
                        let r = ui.selectable_label(st == current, st);
                        app.auto.add(&format!("{prefix}.{st}"), r.rect, st);
                        if r.clicked() {
                            set = Some(json!(st));
                        }
                    }
                })
            };
            let r = crate::widgets::accessible_name(combo.response, crate::i18n::t(pd.label));
            app.auto.add(&format!("effectControls.text.{}.{}", idx, pd.id), r.rect, pd.label);
        }
        (ParamKind::Text, ParamValue::Text(s)) if !pd.id.ends_with("_lut") => {
            // edited in a buffer; committed (one undo step) when the field loses focus
            let mut buf = vui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| s.clone());
            let r = crate::widgets::accessible_name(vui.add(egui::TextEdit::singleline(&mut buf).desired_width(160.0)), crate::i18n::t(pd.label));
            app.auto.add(&format!("effectControls.text.{}.{}", idx, pd.id), r.rect, pd.label);
            crate::widgets::select_all_on_focus(vui, &r, &buf);
            if r.lost_focus() {
                if buf != *s {
                    set = Some(json!(buf));
                }
                vui.data_mut(|d| d.remove::<String>(id));
            } else if r.has_focus() {
                vui.data_mut(|d| d.insert_temp(id, buf));
            } else {
                vui.data_mut(|d| d.remove::<String>(id));
            }
        }
        // Color Grading's Input LUT / Look LUT: the Color Grading panel's menu (it showed only the name, #1123)
        (ParamKind::Text, ParamValue::Text(cur)) => {
            let input = pd.id == "input_lut";
            let auto_id = format!("effectControls.{}.{}", e.effect, pkey);
            let (rect, pick) = crate::panels::lumetri::lut_menu(vui, app, ("fx-lut", clip.0, idx, pkey), cur, input, 160.0, &auto_id);
            app.auto.add(&auto_id, rect, pd.label);
            match pick {
                Some(crate::panels::lumetri::LutPick::Lut(r)) => set = Some(json!(r)),
                Some(crate::panels::lumetri::LutPick::Browse) => {
                    let (param, clip, mask, label) = (pd.id.to_string(), clip.0, mask, pd.label);
                    app.pick_ui(crate::native_dialogs::FileDialog::open_file(tl!("LUT"), &["cube", "3dl"]), move |app, paths| {
                        let Some(path) = paths.into_iter().next() else { return };
                        // importing into the LUT library and setting the parameter undo as one step
                        let r = app.session.edit_group(label, |s| {
                            let r = s.execute("lut.import", json!({"path": path}))?["ref"].clone();
                            let mut p = json!({"clip": clip, "effect": idx, "param": param, "value": r});
                            if let Some(k) = mask {
                                p["mask"] = json!(k);
                            }
                            s.execute("effects.setParam", p)
                        });
                        if let Err(err) = r {
                            app.ui.status = err.to_string();
                        }
                    });
                }
                None => {}
            }
        }
        _ => {
            vui.label(param_text(&app.session.project, pd.id, value));
        }
    }
    set
}

/// Shared by the lane diamonds and graph points: commands use media time and preserve the mask.
/// The rows and their order are Premiere's (Cut, Copy and Paste have no keyframe command yet, and
/// Select All waits for Clear to act on the whole selection); `current` is the keyframe's
/// interpolation, the checked row.
fn keyframe_menu(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    target: KeyframeRef,
    current: filmcraft_project::Interpolation,
    actions: &mut Vec<(String, Value)>,
) {
    use filmcraft_project::Interpolation;
    let mut params = json!({"clip": target.clip, "effect": target.effect, "param": target.param, "mediaTime": target.time.0});
    if let Some(mask) = target.mask {
        params["mask"] = json!(mask);
    }
    // A selected keyframe stands for the whole selection, as when dragging it (#1916).
    let interpolated = if app.ui.keyframe_selection.contains(&target) { json!({"keyframes": app.ui.keyframe_selection}) } else { params.clone() };
    let response = crate::menus::checked_entry(ui, tl!("Clear"), true, None);
    app.auto.add("effectControls.keyframeMenu.clear", response.rect, "Clear");
    if response.clicked() {
        actions.push(("effects.deleteKeyframe".into(), params.clone()));
        ui.close();
    }
    for group in [
        &[
            (tl!("Linear"), "linear", Interpolation::Linear),
            (tl!("Bezier"), "bezier", Interpolation::Bezier),
            (tl!("Auto Bezier"), "autoBezier", Interpolation::AutoBezier),
            (tl!("Continuous Bezier"), "continuousBezier", Interpolation::ContinuousBezier),
            (tl!("Hold"), "hold", Interpolation::Hold),
        ][..],
        &[(tl!("Ease In"), "easeIn", Interpolation::EaseIn), (tl!("Ease Out"), "easeOut", Interpolation::EaseOut)],
    ] {
        ui.separator();
        for &(label, key, interpolation) in group {
            let response = crate::menus::checked_entry(ui, label, true, Some(current == interpolation));
            app.auto.add(&format!("effectControls.keyframeMenu.{key}"), response.rect, label);
            if response.clicked() {
                let mut params = interpolated.clone();
                params["interpolation"] = json!(key);
                actions.push(("effects.setInterpolation".into(), params));
                ui.close();
            }
        }
    }
}

/// Color Grading panel: edits (or adds) the Color Grading effect on the selected clip, grouped by section.
pub fn lumetri_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let Some((clip, it, _)) = selected_clip(app) else {
        crate::dock::placeholder(ui, rect, &t, tl!("Select a clip to grade"));
        return;
    };
    let idx = it.effects.iter().position(|e| e.effect == "lumetri");
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)).id_salt("lumetri"));
    bui.label(egui::RichText::new(tlf!("Master · {name}", name = it.name)).color(t.text_dim));
    let Some(idx) = idx else {
        if bui.button(tl!("Add Color Grading to clip")).clicked() {
            let _ = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}));
        }
        return;
    };
    let e = it.effects[idx].clone();
    let Some(def) = e.def() else { return };
    let ph = app.session.playhead();
    let mt = it.effect_time_at(ph.clamp(it.start, it.end() - Tick(1)));
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut bui, |ui| {
        let mut groups: Vec<&str> = Vec::new();
        for p in &def.params {
            if let Some(g) = p.group
                && !groups.contains(&g)
            {
                groups.push(g);
            }
        }
        for g in groups {
            let key = format!("lumetri:{g}");
            let open = app.ui.expanded_fx.contains(&key);
            let (resp, now_open) = crate::widgets::section_header(ui, egui::Id::unique(&key), crate::i18n::t(g), open, &t, true);
            app.auto.add(&format!("lumetri.section.{g}"), resp.rect, g);
            if now_open != open {
                if now_open {
                    app.ui.expanded_fx.push(key.clone());
                } else {
                    app.ui.expanded_fx.retain(|k| *k != key);
                }
            }
            if !now_open {
                continue;
            }
            for pd in def.params.iter().filter(|p| p.group == Some(g)) {
                let v = e.params.get(pd.id).map(|p| p.value_at(mt)).unwrap_or(pd.default.clone());
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.add_sized(vec2(110.0, 18.0), egui::Label::new(egui::RichText::new(crate::i18n::t(pd.label)).size(12.0)));
                    if let (ParamKind::Float { min, max, soft_min, soft_max, .. }, ParamValue::Float(x)) = (&pd.kind, &v) {
                        let mut val = *x;
                        let s = ui.add(egui::Slider::new(&mut val, *soft_min..=*soft_max).show_value(false));
                        let (_, nv) = crate::widgets::hot_number(
                            ui,
                            crate::i18n::t(pd.label),
                            egui::Id::unique(("lum", pd.id)),
                            val,
                            HotNumberOptions::new((soft_max - soft_min) / 300.0, (*min, *max), 1, "", &t),
                        );
                        if s.changed() {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": val}));
                        } else if let Some(nv) = nv {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": nv}));
                        }
                        if s.double_clicked() {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": pd.default.as_f64().unwrap_or(0.0)}));
                        }
                    } else if let ParamValue::Color(c) = v {
                        let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
                        if egui::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::color_picker::Alpha::Opaque).changed() {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": [rgba.r(), rgba.g(), rgba.b(), 1.0]}));
                        }
                    }
                });
            }
        }
    });
    run(app, ui.ctx(), actions.into_iter().map(|a| ("effects.setParam".to_string(), a)).collect());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum InspectorTab {
    #[default]
    Video,
    Audio,
    ClipProps,
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Preset quick-toggle effects shown in Video -> Effects and Audio -> Effects.
const PRESET_VIDEO_EFFECTS: [&str; 4] = ["gaussian_blur", "sharpen", "vignette", "drop_shadow"];
const PRESET_AUDIO_EFFECTS: [&str; 4] = ["parametric_eq", "compressor", "denoiser", "reverb"];

#[derive(Clone, Default)]
struct EffectPickerState {
    open: bool,
    is_audio: bool,
    clip: u64,
    query: String,
    category: Option<String>,
    just_opened: bool,
}

/// Render a preset effect row when the effect is not yet added to the clip (toggle OFF -> applies).
fn inspector_preset_effect_row(bui: &mut egui::Ui, clip: ClipId, eid: &str, t: Tokens, actions: &mut Vec<(String, Value)>) {
    let Some(def) = filmcraft_project::find_effect(eid) else { return };
    let (hr, _) = bui.allocate_exact_size(vec2(bui.available_width(), 26.0), Sense::hover());
    bui.painter().rect_filled(hr, 4.0, t.tl_header_bg);
    let en_r = Rect::from_center_size(pos2(hr.min.x + 32.0, hr.center().y), vec2(26.0, 14.0));
    let en_resp = bui.interact(en_r, egui::Id::unique(("props-preset-en", clip.0, eid)), Sense::click()).on_hover_text(tl!("Enable effect"));
    paint_toggle_switch(bui, en_r, false, &t);
    if en_resp.clicked() {
        actions.push(("effects.apply".into(), json!({"clips": [clip.0], "effect": eid})));
    }
    bui.painter().text(pos2(hr.min.x + 52.0, hr.center().y), Align2::LEFT_CENTER, crate::i18n::t(def.name), Tokens::ui(12.0), t.text_dim);
    bui.add_space(3.0);
}

/// Floating searchable effect picker window opened by `+ Add Effect`.
fn inspector_effect_picker_window(ctx: &egui::Context, t: Tokens, actions: &mut Vec<(String, Value)>) {
    let id = egui::Id::unique("inspector-effect-picker");
    let mut st = ctx.data(|d| d.get_temp::<EffectPickerState>(id)).unwrap_or_default();
    if !st.open {
        return;
    }
    let kind = if st.is_audio { filmcraft_project::EffectKind::Audio } else { filmcraft_project::EffectKind::Video };
    let title = if st.is_audio { tl!("Add Audio Effect") } else { tl!("Add Video Effect") };
    let mut open = st.open;
    let mut close_now = false;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close_now = true;
    }

    egui::Window::new(title)
        .id(egui::Id::unique("inspector-effect-picker-win"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(vec2(340.0, 420.0))
        .min_size(vec2(260.0, 240.0))
        .show(ctx, |ui| {
            let search_resp = ui.add(egui::TextEdit::singleline(&mut st.query).hint_text(tl!("Search effects…")).desired_width(f32::INFINITY));
            if st.just_opened {
                search_resp.request_focus();
                st.just_opened = false;
            }
            ui.add_space(6.0);

            // Category filter chips
            let mut categories: Vec<&'static str> = Vec::new();
            for def in filmcraft_project::effect_defs() {
                if def.kind == kind
                    && !def.intrinsic
                    && def.id != "lumetri"
                    && !filmcraft_project::graphic::is_layer_id(def.id)
                    && let Some(&cat) = def.category.last()
                    && !categories.contains(&cat)
                {
                    categories.push(cat);
                }
            }
            ui.horizontal_wrapped(|cui| {
                if cui.selectable_label(st.category.is_none(), tl!("All")).clicked() {
                    st.category = None;
                }
                for cat in &categories {
                    let sel = st.category.as_deref() == Some(*cat);
                    if cui.selectable_label(sel, crate::i18n::t(cat)).clicked() {
                        st.category = Some((*cat).to_string());
                    }
                }
            });
            ui.separator();

            let q = st.query.trim().to_ascii_lowercase();
            let shift = ctx.input(|i| i.modifiers.shift || i.modifiers.command);
            egui::ScrollArea::vertical().id_salt("picker-fx-list").auto_shrink([false, false]).show(ui, |sui| {
                let mut any = false;
                for def in filmcraft_project::effect_defs() {
                    if def.kind != kind || def.intrinsic || def.id == "lumetri" || filmcraft_project::graphic::is_layer_id(def.id) {
                        continue;
                    }
                    let cat_name = def.category.last().copied().unwrap_or("");
                    if let Some(cat) = &st.category
                        && cat_name != cat
                    {
                        continue;
                    }
                    let label = crate::i18n::t(def.name);
                    if !q.is_empty() {
                        let matches = label.to_ascii_lowercase().contains(&q)
                            || def.name.to_ascii_lowercase().contains(&q)
                            || def.id.to_ascii_lowercase().contains(&q)
                            || filmcraft_project::search_aliases(def).iter().any(|a| a.to_ascii_lowercase().contains(&q));
                        if !matches {
                            continue;
                        }
                    }
                    any = true;
                    let (rr, rresp) = sui.allocate_exact_size(vec2(sui.available_width(), 28.0), Sense::click());
                    if rresp.hovered() {
                        sui.painter().rect_filled(rr, 4.0, t.hover);
                    }
                    sui.painter().text(pos2(rr.min.x + 8.0, rr.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text);
                    sui.painter().text(pos2(rr.max.x - 8.0, rr.center().y), Align2::RIGHT_CENTER, crate::i18n::t(cat_name), Tokens::ui(10.5), t.text_dim);
                    if rresp.clicked() {
                        actions.push(("effects.apply".into(), json!({"clips": [st.clip], "effect": def.id})));
                        if !shift {
                            close_now = true;
                        }
                    }
                }
                if !any {
                    sui.add_space(12.0);
                    sui.label(egui::RichText::new(tl!("No matching effects.")).size(11.5).color(t.text_dim));
                }
            });
        });

    if !open || close_now {
        st.open = false;
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

/// Paint a compact DaVinci Resolve-style pill toggle switch.
fn paint_toggle_switch(ui: &egui::Ui, rect: Rect, on: bool, t: &Tokens) {
    let r = rect.height() * 0.5;
    let bg = if on { t.accent } else { Color32::from_rgba_unmultiplied(255, 255, 255, 28) };
    ui.painter().rect_filled(rect, r, bg);
    let knob_r = (r - 2.0).max(2.0);
    let knob_x = if on { rect.max.x - r } else { rect.min.x + r };
    ui.painter().circle_filled(pos2(knob_x, rect.center().y), knob_r, Color32::WHITE);
}

/// Render inline editable parameters for a non-intrinsic effect instance inside the Inspector.
#[allow(clippy::too_many_arguments)]
fn inspector_added_effect_card(
    app: &mut FilmcraftApp,
    bui: &mut egui::Ui,
    clip: ClipId,
    it: &TrackItem,
    mt: Tick,
    idx: usize,
    e: &EffectInstance,
    t: Tokens,
    actions: &mut Vec<(String, Value)>,
) {
    let Some(def) = filmcraft_project::find_effect(&e.effect) else { return };
    if def.intrinsic || filmcraft_project::graphic::is_layer_id(def.id) {
        return;
    }
    let key = format!("props:fx:{}:{}", clip.0, idx);
    let open = app.ui.expanded_fx.contains(&key);
    let (hr, hresp) = bui.allocate_exact_size(vec2(bui.available_width(), 28.0), Sense::click());
    bui.painter().rect_filled(hr, 4.0, if hresp.hovered() { t.hover } else { t.tl_header_bg });
    icons::paint(
        bui.painter(),
        Rect::from_center_size(pos2(hr.min.x + 10.0, hr.center().y), vec2(10.0, 10.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        t.text_dim,
    );

    // DaVinci Resolve-style On/Off pill switch
    let en_r = Rect::from_center_size(pos2(hr.min.x + 32.0, hr.center().y), vec2(26.0, 14.0));
    let en_resp = bui.interact(en_r, egui::Id::unique(("props-fx-en", clip.0, idx)), Sense::click()).on_hover_text(tl!("Toggle effect"));
    paint_toggle_switch(bui, en_r, e.enabled, &t);
    if en_resp.clicked() {
        actions.push(("effects.toggleEnabled".into(), json!({"clip": clip.0, "index": idx})));
    }

    let name_col = if e.enabled { t.text } else { t.text_faint };
    bui.painter().text(pos2(hr.min.x + 52.0, hr.center().y), Align2::LEFT_CENTER, crate::i18n::t(def.name), Tokens::semibold(12.0), name_col);

    // Remove button
    let rm_r = Rect::from_center_size(pos2(hr.max.x - 14.0, hr.center().y), vec2(16.0, 16.0));
    let rm_resp = bui.interact(rm_r, egui::Id::unique(("props-fx-rm", clip.0, idx)), Sense::click()).on_hover_text(tl!("Remove effect"));
    bui.painter().text(rm_r.center(), Align2::CENTER_CENTER, "×", Tokens::semibold(13.0), if rm_resp.hovered() { t.text } else { t.text_dim });
    if rm_resp.clicked() {
        actions.push(("effects.remove".into(), json!({"clip": clip.0, "index": idx})));
    }

    // Reset button
    let rs_r = Rect::from_center_size(pos2(hr.max.x - 34.0, hr.center().y), vec2(14.0, 14.0));
    let rs_resp = bui.interact(rs_r, egui::Id::unique(("props-fx-rs", clip.0, idx)), Sense::click()).on_hover_text(tl!("Reset effect"));
    icons::paint(bui.painter(), rs_r, Icon::Reset, if rs_resp.hovered() { t.text } else { t.text_dim });
    if rs_resp.clicked() {
        actions.push(("effects.reset".into(), json!({"clip": clip.0, "index": idx})));
    }

    if hresp.clicked() && !en_resp.hovered() && !rm_resp.hovered() && !rs_resp.hovered() {
        if open {
            app.ui.expanded_fx.retain(|k| *k != key);
        } else {
            app.ui.expanded_fx.push(key);
        }
    }

    if !open {
        bui.add_space(3.0);
        return;
    }

    for pd in def.params.iter() {
        if matches!(pd.kind, ParamKind::Path) {
            continue;
        }
        let val = e.params.get(pd.id).map(|p| p.value_at(mt)).unwrap_or_else(|| pd.default.clone());
        let (r, _) = bui.allocate_exact_size(vec2(bui.available_width(), 26.0), Sense::hover());
        bui.painter().text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::t(pd.label), Tokens::ui(11.5), t.text_dim);
        let mut vui = bui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(pos2(r.min.x + 146.0, r.min.y + 3.0), pos2(r.max.x - PROPS_VALUE_R, r.max.y - 3.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let wid = egui::Id::unique(("props-fx-param", clip.0, idx, pd.id));
        if let Some(nv) = param_value_widget(app, &mut vui, e, pd, &val, clip, idx, None, pd.id, wid, t, actions) {
            actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": nv})));
        }
        properties_nav(app, bui, r, clip, it, mt, &e.effect, pd.id, actions);
    }
    bui.add_space(4.0);
}

/// Video Inspector dock tab: Transform, Crop, Speed, and inline Video Effects manager.
pub fn inspector_video_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    inspector_panel(app, ui, rect, InspectorTab::Video);
}

/// Audio Inspector dock tab: Volume, Pan, and inline Audio Effects manager.
pub fn inspector_audio_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    inspector_panel(app, ui, rect, InspectorTab::Audio);
}

/// Properties Inspector dock tab: clip timing, media codecs, resolution, file size, and location.
pub fn inspector_properties_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    inspector_panel(app, ui, rect, InspectorTab::ClipProps);
}

/// Legacy alias for the Properties Inspector tab.
pub fn properties_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    inspector_properties_panel(app, ui, rect);
}

fn inspector_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, active_tab: InspectorTab) {
    let t = app.tokens;
    let clips = selected_clips(app);
    let Some((clip, it, kind)) = clips.first().cloned() else {
        crate::dock::placeholder(ui, rect, &t, tl!("Select a clip to see its properties"));
        return;
    };
    let ph = app.session.playhead();
    let mt = it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
    let mut actions: Vec<(String, Value)> = Vec::new();
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 8.0))).id_salt("props"));

    // Clean DaVinci Resolve-style clip header: colour swatch + clip name
    let (hr, _) = bui.allocate_exact_size(vec2(bui.available_width(), 26.0), Sense::hover());
    let sw = Rect::from_center_size(pos2(hr.min.x + 8.0, hr.center().y), vec2(12.0, 12.0));
    let lc = app.session.prefs.labels.rgb(it.label);
    bui.painter().rect_filled(sw, 2.0, Color32::from_rgb(lc[0], lc[1], lc[2]));
    let mut name_job = egui::text::LayoutJob::simple_singleline(it.name.clone(), Tokens::semibold(12.5), t.text);
    name_job.wrap.max_width = (hr.width() - 28.0).max(40.0);
    name_job.wrap.max_rows = 1;
    let name_g = bui.painter().layout_job(name_job);
    bui.painter().with_clip_rect(Rect::from_min_max(pos2(hr.min.x + 22.0, hr.min.y), hr.max)).galley(
        pos2(hr.min.x + 22.0, hr.center().y - name_g.size().y * 0.5),
        name_g,
        t.text,
    );
    bui.add_space(4.0);

    let section = |ui: &mut egui::Ui,
                   app: &mut FilmcraftApp,
                   name: &str,
                   toggle: Option<bool>,
                   reset: Option<(usize, u64)>,
                   actions: &mut Vec<(String, Value)>|
     -> (bool, bool) {
        ui.add_space(4.0);
        let key = format!("props:{name}");
        let open = app.ui.expanded_fx.contains(&key);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        ui.painter().rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(255, 255, 255, 10));
        ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, t.separator));
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(r.min.x + 8.0, r.center().y), vec2(10.0, 10.0)),
            if open { Icon::ChevronDown } else { Icon::ChevronRight },
            t.text_dim,
        );
        let mut title_x = r.min.x + 20.0;
        let mut toggle_hovered = false;
        let mut toggle_clicked = false;
        if let Some(enabled) = toggle {
            let sw_r = Rect::from_center_size(pos2(title_x + 13.0, r.center().y), vec2(26.0, 14.0));
            let sw_resp = ui.interact(sw_r, egui::Id::unique(("props-sec-en", name, clip.0)), Sense::click()).on_hover_text(tl!("Toggle section"));
            toggle_hovered = sw_resp.hovered();
            toggle_clicked = sw_resp.clicked();
            paint_toggle_switch(ui, sw_r, enabled, &t);
            title_x += 34.0;
        }
        ui.painter().text(pos2(title_x, r.center().y), Align2::LEFT_CENTER, crate::i18n::t(name), Tokens::semibold(12.5), t.text);
        let mut reset_hovered = false;
        if let Some((idx, c)) = reset {
            let rr = Rect::from_center_size(pos2(r.max.x - PROPS_NAV_X, r.center().y), vec2(14.0, 14.0));
            icons::paint(ui.painter(), rr, Icon::Reset, t.text_dim);
            let r_resp = ui.interact(rr, egui::Id::unique(("props-reset", name, c)), Sense::click()).on_hover_text(tl!("Reset effect"));
            reset_hovered = r_resp.hovered();
            if r_resp.clicked() {
                actions.push(("effects.reset".into(), json!({"clip": c, "index": idx})));
            }
        }
        if resp.clicked() && !toggle_hovered && !reset_hovered {
            if open {
                app.ui.expanded_fx.retain(|k| *k != key);
            } else {
                app.ui.expanded_fx.push(key);
            }
        }
        (open, toggle_clicked)
    };

    let show_video = active_tab == InspectorTab::Video;
    let show_audio = active_tab == InspectorTab::Audio;
    let show_meta = active_tab == InspectorTab::ClipProps;

    egui::ScrollArea::vertical().id_salt("props-scroll").auto_shrink([false, false]).show(&mut bui, |bui| {
        bui.set_max_width((bui.available_width() - 12.0).max(1.0));
        let eff_idx = |id: &str| it.effects.iter().position(|e| e.effect == id);

        // ---- VIDEO INSPECTOR ----
        if show_video {
            if kind == TrackKind::Video {
                let motion_idx = eff_idx("motion");
                let motion_on = motion_idx.and_then(|i| it.effects.get(i)).is_some_and(|e| e.enabled);
                let (motion_open, motion_toggled) = section(bui, app, "Transform", Some(motion_on), motion_idx.map(|i| (i, clip.0)), &mut actions);
                if motion_toggled && let Some(i) = motion_idx {
                    actions.push(("effects.toggleEnabled".into(), json!({"clip": clip.0, "index": i})));
                }
                if motion_open {
                    render_inspector_transform_rows(app, bui, clip, &it, mt, t, &mut actions);
                }

                let crop = eff_idx("crop");
                let crop_on = crop.and_then(|i| it.effects.get(i)).is_some_and(|e| e.enabled);
                let (crop_open, crop_toggled) = section(bui, app, "Crop", Some(crop_on), crop.map(|i| (i, clip.0)), &mut actions);
                if crop_toggled {
                    if let Some(i) = crop {
                        actions.push(("effects.toggleEnabled".into(), json!({"clip": clip.0, "index": i})));
                    } else {
                        actions.push(("effects.apply".into(), json!({"clips": [clip.0], "effect": "crop"})));
                    }
                }
                if crop_open {
                    render_inspector_crop_rows(app, bui, clip, &it, mt, crop.is_some(), t, &mut actions);
                }

                // Speed (Velocidad) — basic video effect inside Video Inspector
                let speed_saved_id = egui::Id::unique(("pp-speed-saved", clip.0));
                let speed_active = (it.speed - 1.0).abs() > 0.001 || it.reverse;
                let (speed_open, speed_toggled) = section(bui, app, "Speed", Some(speed_active), None, &mut actions);
                if speed_toggled {
                    if speed_active {
                        bui.ctx().data_mut(|d| d.insert_temp(speed_saved_id, (it.speed * 100.0, it.reverse)));
                        actions.push((
                            "clip.speedDuration".into(),
                            json!({"clips": [clip.0], "speed": 100.0, "reverse": false, "ripple": false, "maintainPitch": it.maintain_pitch}),
                        ));
                    } else {
                        let (saved_pct, saved_rev) = bui.ctx().data(|d| d.get_temp::<(f64, bool)>(speed_saved_id)).unwrap_or((100.0, false));
                        let target_pct = if (saved_pct - 100.0).abs() > 0.1 || saved_rev { saved_pct } else { 150.0 };
                        actions.push((
                            "clip.speedDuration".into(),
                            json!({"clips": [clip.0], "speed": target_pct, "reverse": saved_rev, "ripple": false, "maintainPitch": it.maintain_pitch}),
                        ));
                    }
                }
                if speed_open {
                    render_inspector_speed(bui, clip, &it, t, &mut actions);
                }

                // Applied & preset Video Effects inside Video Inspector
                let vfx_indices: Vec<usize> = it
                    .effects
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| {
                        e.effect != "crop"
                            && e.effect != "lumetri"
                            && filmcraft_project::find_effect(&e.effect).is_some_and(|d| !d.intrinsic && !filmcraft_project::graphic::is_layer_id(d.id))
                    })
                    .map(|(i, _)| i)
                    .collect();
                let vfx_any_enabled = vfx_indices.iter().any(|&i| it.effects.get(i).is_some_and(|e| e.enabled));
                let (vfx_open, vfx_toggled) = section(bui, app, "Effects", Some(vfx_any_enabled), None, &mut actions);
                if vfx_toggled {
                    for &i in &vfx_indices {
                        if let Some(e) = it.effects.get(i)
                            && e.enabled == vfx_any_enabled
                        {
                            actions.push(("effects.toggleEnabled".into(), json!({"clip": clip.0, "index": i})));
                        }
                    }
                }
                if vfx_open {
                    render_inspector_vfx_list(app, bui, clip, &it, mt, &vfx_indices, t, &mut actions);
                }
            } else if active_tab == InspectorTab::Video {
                bui.add_space(8.0);
                bui.label(egui::RichText::new(tl!("No video track on selected clip.")).size(11.5).color(t.text_dim));
            }
        }

        // ---- AUDIO INSPECTOR ----
        if show_audio {
            if let Some((aclip, ait, _)) = clips.iter().find(|c| c.2 == TrackKind::Audio) {
                let aclip = *aclip;
                let amt = ait.effect_time_at(ph.clamp(ait.start, (ait.end() - Tick(1)).max(ait.start)));
                let vol_idx = ait.effects.iter().position(|e| e.effect == "volume");
                let vol_on = vol_idx.and_then(|i| ait.effects.get(i)).is_some_and(|e| e.enabled);
                let (audio_open, audio_toggled) = section(bui, app, "Audio", Some(vol_on), vol_idx.map(|i| (i, aclip.0)), &mut actions);
                if audio_toggled && let Some(i) = vol_idx {
                    actions.push(("effects.toggleEnabled".into(), json!({"clip": aclip.0, "index": i})));
                }
                if audio_open {
                    render_inspector_audio_level_pan(app, bui, aclip, ait, amt, t, &mut actions);

                    // Applied & preset Audio Effects
                    let afx_indices: Vec<usize> = ait
                        .effects
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| filmcraft_project::find_effect(&e.effect).is_some_and(|d| !d.intrinsic))
                        .map(|(i, _)| i)
                        .collect();
                    let afx_any_enabled = afx_indices.iter().any(|&i| ait.effects.get(i).is_some_and(|e| e.enabled));
                    let (afx_open, afx_toggled) = section(bui, app, "Effects", Some(afx_any_enabled), None, &mut actions);
                    if afx_toggled {
                        for &i in &afx_indices {
                            if let Some(e) = ait.effects.get(i)
                                && e.enabled == afx_any_enabled
                            {
                                actions.push(("effects.toggleEnabled".into(), json!({"clip": aclip.0, "index": i})));
                            }
                        }
                    }
                    if afx_open {
                        render_inspector_audio_fx(app, bui, aclip, ait, amt, &afx_indices, t, &mut actions);
                    }
                }
            } else if active_tab == InspectorTab::Audio {
                bui.add_space(8.0);
                bui.label(egui::RichText::new(tl!("No audio track on selected clip.")).size(11.5).color(t.text_dim));
            }
        }

        // ---- CLIP PROPERTIES (METADATA: CODECS, DURATION, SIZE, LOCATION) ----
        if show_meta && section(bui, app, "Clip Properties", None, None, &mut actions).0 {
            render_inspector_clip_meta(app, ui, bui, clip, &it, t);
        }
    });
    inspector_effect_picker_window(ui.ctx(), t, &mut actions);
    run(app, ui.ctx(), actions);
}

fn props_row(ui: &mut egui::Ui, label: &str, t: Tokens) -> (Rect, egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, label, Tokens::ui(11.5), t.text_dim);
    let vui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(r.min.x + 146.0, r.min.y + 3.0), pos2(r.max.x - PROPS_VALUE_R, r.max.y - 3.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    (r, vui)
}

#[allow(clippy::too_many_arguments)]
fn render_inspector_transform_rows(
    app: &mut FilmcraftApp,
    bui: &mut egui::Ui,
    clip: ClipId,
    it: &TrackItem,
    mt: Tick,
    t: Tokens,
    actions: &mut Vec<(String, Value)>,
) {
    let val = |eid: &str, p: &str| it.effect(eid).and_then(|e| e.param(p)).map(|p| p.value_at(mt));
    for (label, p, unit, speed, range) in [
        (tl!("Position"), "position", "", 1.0, (-100_000.0, 100_000.0)),
        (tl!("Anchor point"), "anchor", "", 1.0, (-100_000.0, 100_000.0)),
        (tl!("Scale"), "scale", " %", 0.5, (0.0, 10000.0)),
        (tl!("Rotation"), "rotation", " °", 0.5, (-36000.0, 36000.0)),
    ] {
        let (r, mut vui) = props_row(bui, label, t);
        match val("motion", p) {
            Some(ParamValue::Vec2(v)) => {
                let (_, nx) = crate::widgets::hot_number(
                    &mut vui,
                    &format!("{label} X"),
                    egui::Id::unique(("pp", p, "x", clip.0)),
                    v.x,
                    HotNumberOptions::new(speed, range, 0, " X", &t),
                );
                vui.add_space(10.0);
                let (_, ny) = crate::widgets::hot_number(
                    &mut vui,
                    &format!("{label} Y"),
                    egui::Id::unique(("pp", p, "y", clip.0)),
                    v.y,
                    HotNumberOptions::new(speed, range, 0, " Y", &t),
                );
                if nx.is_some() || ny.is_some() {
                    actions.push((
                        "effects.setParam".into(),
                        json!({"clip": clip.0, "effect": "motion", "param": p, "value": [nx.unwrap_or(v.x), ny.unwrap_or(v.y)]}),
                    ));
                }
            }
            Some(ParamValue::Float(v)) => {
                let (_, nv) = crate::widgets::hot_number(
                    &mut vui,
                    label,
                    egui::Id::unique(("pp", p, clip.0)),
                    v,
                    HotNumberOptions::new(speed, range, if p == "scale" { 0 } else { 1 }, unit, &t),
                );
                if let Some(nv) = nv {
                    actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "motion", "param": p, "value": nv})));
                }
            }
            _ => {}
        }
        properties_nav(app, bui, r, clip, it, mt, "motion", p, actions);
    }
    let (r, mut vui) = props_row(bui, tl!("Opacity"), t);
    if let Some(ParamValue::Float(v)) = val("opacity", "opacity") {
        let (_, nv) =
            crate::widgets::hot_number(&mut vui, tl!("Opacity"), egui::Id::unique(("pp-op", clip.0)), v, HotNumberOptions::new(0.5, (0.0, 100.0), 0, " %", &t));
        if let Some(nv) = nv {
            actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "opacity", "param": "opacity", "value": nv})));
        }
    }
    properties_nav(app, bui, r, clip, it, mt, "opacity", "opacity", actions);
}

#[allow(clippy::too_many_arguments)]
fn render_inspector_crop_rows(
    app: &mut FilmcraftApp,
    bui: &mut egui::Ui,
    clip: ClipId,
    it: &TrackItem,
    mt: Tick,
    has_crop: bool,
    t: Tokens,
    actions: &mut Vec<(String, Value)>,
) {
    for (label, p) in [(tl!("Left"), "left"), (tl!("Top"), "top"), (tl!("Right"), "right"), (tl!("Bottom"), "bottom")] {
        let (r, mut vui) = props_row(bui, label, t);
        let v = it.effect("crop").and_then(|e| e.param(p)).map(|p| p.value_at(mt)).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let (_, nv) =
            crate::widgets::hot_number(&mut vui, label, egui::Id::unique(("pp-crop", p, clip.0)), v, HotNumberOptions::new(0.2, (0.0, 100.0), 1, " %", &t));
        if let Some(nv) = nv {
            if !has_crop {
                actions.push(("effects.apply".into(), json!({"clips": [clip.0], "effect": "crop"})));
            }
            actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "crop", "param": p, "value": nv})));
        }
        properties_nav(app, bui, r, clip, it, mt, "crop", p, actions);
    }
}

fn render_inspector_speed(bui: &mut egui::Ui, clip: ClipId, it: &TrackItem, t: Tokens, actions: &mut Vec<(String, Value)>) {
    let (r, _) = bui.allocate_exact_size(vec2(bui.available_width(), 28.0), Sense::hover());
    bui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tl!("Speed"), Tokens::ui(11.5), t.text_dim);
    let mut vui = bui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(r.min.x + 146.0, r.min.y + 3.0), pos2(r.max.x - PROPS_VALUE_R, r.max.y - 3.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let speed_pct = it.speed * 100.0;
    let (_, ns) = crate::widgets::hot_number(
        &mut vui,
        tl!("Speed"),
        egui::Id::unique(("pp-speed", clip.0)),
        speed_pct,
        HotNumberOptions::new(1.0, (10.0, 1000.0), 0, " %", &t),
    );
    if let Some(ns) = ns {
        actions.push((
            "clip.speedDuration".into(),
            json!({"clips": [clip.0], "speed": ns, "reverse": it.reverse, "ripple": false, "maintainPitch": it.maintain_pitch}),
        ));
    }
    let rr = Rect::from_center_size(pos2(r.max.x - PROPS_NAV_X, r.center().y), vec2(14.0, 14.0));
    let r_resp = bui.interact(rr, egui::Id::unique(("pp-speed-reset", clip.0)), Sense::click()).on_hover_text(tl!("Reset Speed"));
    icons::paint(bui.painter(), rr, Icon::Reset, if r_resp.hovered() { t.text } else { t.text_dim });
    if r_resp.clicked() {
        actions.push(("clip.speedDuration".into(), json!({"clips": [clip.0], "speed": 100.0, "reverse": false, "ripple": false, "maintainPitch": true})));
    }

    let mut rev = it.reverse;
    let (rr_rev, _) = bui.allocate_exact_size(vec2(bui.available_width(), 24.0), Sense::hover());
    let mut cui_rev = bui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(rr_rev.min.x + 16.0, rr_rev.min.y), rr_rev.max))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if cui_rev.checkbox(&mut rev, tl!("Reverse Speed")).changed() {
        actions.push((
            "clip.speedDuration".into(),
            json!({"clips": [clip.0], "speed": speed_pct, "reverse": rev, "ripple": false, "maintainPitch": it.maintain_pitch}),
        ));
    }

    let mut pitch = it.maintain_pitch;
    let (rr_pitch, _) = bui.allocate_exact_size(vec2(bui.available_width(), 24.0), Sense::hover());
    let mut cui_pitch = bui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(rr_pitch.min.x + 16.0, rr_pitch.min.y), rr_pitch.max))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if cui_pitch.checkbox(&mut pitch, tl!("Maintain Audio Pitch")).changed() {
        actions.push((
            "clip.speedDuration".into(),
            json!({"clips": [clip.0], "speed": speed_pct, "reverse": it.reverse, "ripple": false, "maintainPitch": pitch}),
        ));
    }
}

fn render_inspector_audio_level_pan(
    app: &mut FilmcraftApp,
    bui: &mut egui::Ui,
    aclip: ClipId,
    ait: &TrackItem,
    amt: Tick,
    t: Tokens,
    actions: &mut Vec<(String, Value)>,
) {
    let aval = |eid: &str, p: &str| ait.effect(eid).and_then(|e| e.param(p)).map(|p| p.value_at(amt));
    let (r, _) = bui.allocate_exact_size(vec2(bui.available_width(), 28.0), Sense::hover());
    bui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tl!("Level"), Tokens::ui(11.5), t.text_dim);
    let mut vui = bui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(r.min.x + 146.0, r.min.y + 3.0), pos2(r.max.x - PROPS_VALUE_R, r.max.y - 3.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let v = aval("volume", "level").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let (_, nv) =
        crate::widgets::hot_number(&mut vui, tl!("Level"), egui::Id::unique(("pp-vol", aclip.0)), v, HotNumberOptions::new(0.1, (-96.0, 15.0), 1, " dB", &t));
    if let Some(nv) = nv {
        actions.push(("effects.setParam".into(), json!({"clip": aclip.0, "effect": "volume", "param": "level", "value": nv})));
    }
    properties_nav(app, bui, r, aclip, ait, amt, "volume", "level", actions);

    let (r, _) = bui.allocate_exact_size(vec2(bui.available_width(), 28.0), Sense::hover());
    bui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tl!("Pan"), Tokens::ui(11.5), t.text_dim);
    let mut vui = bui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(r.min.x + 146.0, r.min.y + 3.0), pos2(r.max.x - PROPS_VALUE_R, r.max.y - 3.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let v = aval("panner", "balance").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let (_, nv) =
        crate::widgets::hot_number(&mut vui, tl!("Pan"), egui::Id::unique(("pp-pan", aclip.0)), v, HotNumberOptions::new(0.5, (-100.0, 100.0), 0, "", &t));
    if let Some(nv) = nv {
        actions.push(("effects.setParam".into(), json!({"clip": aclip.0, "effect": "panner", "param": "balance", "value": nv})));
    }
}

#[allow(clippy::too_many_arguments)]
fn render_inspector_audio_fx(
    app: &mut FilmcraftApp,
    bui: &mut egui::Ui,
    aclip: ClipId,
    ait: &TrackItem,
    amt: Tick,
    afx_indices: &[usize],
    t: Tokens,
    actions: &mut Vec<(String, Value)>,
) {
    bui.add_space(4.0);
    for eid in PRESET_AUDIO_EFFECTS {
        if let Some(idx) = ait.effects.iter().position(|e| e.effect == eid)
            && let Some(e) = ait.effects.get(idx)
        {
            inspector_added_effect_card(app, bui, aclip, ait, amt, idx, e, t, actions);
        } else {
            inspector_preset_effect_row(bui, aclip, eid, t, actions);
        }
    }
    for &idx in afx_indices {
        if let Some(e) = ait.effects.get(idx)
            && !PRESET_AUDIO_EFFECTS.contains(&e.effect.as_str())
        {
            inspector_added_effect_card(app, bui, aclip, ait, amt, idx, e, t, actions);
        }
    }
    bui.add_space(2.0);
    if bui.button(tl!("+ Add Effect")).clicked() {
        let pid = egui::Id::unique("inspector-effect-picker");
        bui.ctx().data_mut(|d| {
            d.insert_temp(pid, EffectPickerState { open: true, is_audio: true, clip: aclip.0, query: String::new(), category: None, just_opened: true });
        });
    }
    bui.add_space(4.0);
}

#[allow(clippy::too_many_arguments)]
fn render_inspector_vfx_list(
    app: &mut FilmcraftApp,
    bui: &mut egui::Ui,
    clip: ClipId,
    it: &TrackItem,
    mt: Tick,
    vfx_indices: &[usize],
    t: Tokens,
    actions: &mut Vec<(String, Value)>,
) {
    bui.add_space(4.0);
    for eid in PRESET_VIDEO_EFFECTS {
        if let Some(idx) = it.effects.iter().position(|e| e.effect == eid)
            && let Some(e) = it.effects.get(idx)
        {
            inspector_added_effect_card(app, bui, clip, it, mt, idx, e, t, actions);
        } else {
            inspector_preset_effect_row(bui, clip, eid, t, actions);
        }
    }
    for &idx in vfx_indices {
        if let Some(e) = it.effects.get(idx)
            && !PRESET_VIDEO_EFFECTS.contains(&e.effect.as_str())
        {
            inspector_added_effect_card(app, bui, clip, it, mt, idx, e, t, actions);
        }
    }
    bui.add_space(2.0);
    if bui.button(tl!("+ Add Effect")).clicked() {
        let pid = egui::Id::unique("inspector-effect-picker");
        bui.ctx().data_mut(|d| {
            d.insert_temp(pid, EffectPickerState { open: true, is_audio: false, clip: clip.0, query: String::new(), category: None, just_opened: true });
        });
    }
    bui.add_space(4.0);
}

fn props_kv(bui: &mut egui::Ui, clip: ClipId, label: &str, value: String, t: Tokens) {
    let (r, _) = bui.allocate_exact_size(vec2(bui.available_width(), 22.0), Sense::hover());
    bui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, label, Tokens::ui(11.5), t.text_dim);
    let val_x = r.min.x + 132.0;
    let max_w = (r.max.x - val_x - 6.0).max(40.0);
    let mut job = egui::text::LayoutJob::simple_singleline(value.clone(), Tokens::ui(11.5), t.text);
    job.wrap.max_width = max_w;
    job.wrap.max_rows = 1;
    let g = bui.painter().layout_job(job);
    bui.painter().with_clip_rect(Rect::from_min_max(pos2(val_x, r.min.y), pos2(r.max.x - 4.0, r.max.y))).galley(
        pos2(val_x, r.center().y - g.size().y * 0.5),
        g,
        t.text,
    );
    bui.interact(r, egui::Id::unique(("props-kv", label, clip.0)), Sense::hover()).on_hover_text(value);
}

fn render_inspector_clip_meta(app: &FilmcraftApp, ui: &egui::Ui, bui: &mut egui::Ui, clip: ClipId, it: &TrackItem, t: Tokens) {
    let seq_fps = app.session.active_sequence().map(|s| s.settings.frame_rate).unwrap_or_default();
    let seq_drop = app.session.active_sequence().is_some_and(|s| s.settings.drop_frame);
    let seq_rate = app.session.active_sequence().map_or(48000, |s| s.settings.sample_rate as i64);
    let fmt_tc = |tick: Tick| filmcraft_time::format_time(tick, seq_fps, seq_drop, filmcraft_time::TimeDisplay::Timecode, seq_rate);

    props_kv(bui, clip, tl!("Timeline Start:"), fmt_tc(it.start), t);
    props_kv(bui, clip, tl!("Duration:"), fmt_tc(it.duration), t);
    props_kv(bui, clip, tl!("Speed:"), format!("{:.0}%{}", it.speed * 100.0, if it.reverse { " (Rev)" } else { "" }), t);

    let Some(proj_item) = app.session.project.item(it.item) else { return };
    props_kv(bui, clip, tl!("Type:"), crate::i18n::t(proj_item.type_label()).to_string(), t);
    let Some(m) = proj_item.as_media() else { return };
    if !m.info.container.is_empty() {
        props_kv(bui, clip, tl!("Container:"), m.info.container.to_uppercase(), t);
    }
    let size_bytes = m.info.file_size.or_else(|| m.identity.map(|i| i.size)).or_else(|| match &m.media {
        filmcraft_project::MediaRef::File { path } => std::fs::metadata(path).ok().map(|md| md.len()),
        _ => None,
    });
    if let Some(sz) = size_bytes {
        props_kv(bui, clip, tl!("File Size:"), format_bytes(sz), t);
    }
    props_kv(
        bui,
        clip,
        tl!("Source Duration:"),
        filmcraft_time::format_time(m.duration(), m.frame_rate(), false, filmcraft_time::TimeDisplay::Timecode, 48000),
        t,
    );
    if let Some(v) = &m.info.video {
        let codec = if v.codec.is_empty() { "—".to_string() } else { v.codec.clone() };
        props_kv(bui, clip, tl!("Video Codec:"), codec, t);
        props_kv(bui, clip, tl!("Resolution:"), format!("{} × {} · {} fps", v.width, v.height, v.frame_rate.label()), t);
        let (px, py) = m.pixel_aspect();
        props_kv(bui, clip, tl!("Pixel Aspect:"), format!("{px}:{py}"), t);
    }
    if let Some(a) = m.info.audio() {
        let codec = if a.codec.is_empty() { "—".to_string() } else { a.codec.clone() };
        props_kv(bui, clip, tl!("Audio Codec:"), codec, t);
        props_kv(bui, clip, tl!("Audio Format:"), format!("{} Hz · {} ch", a.sample_rate, a.channels), t);
    }
    match &m.media {
        filmcraft_project::MediaRef::File { path } => {
            let (r, _) = bui.allocate_exact_size(vec2(bui.available_width(), 24.0), Sense::hover());
            bui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tl!("Location:"), Tokens::ui(11.5), t.text_dim);
            let copy_r = Rect::from_center_size(pos2(r.max.x - 20.0, r.center().y), vec2(32.0, 18.0));
            let copy_resp = bui.interact(copy_r, egui::Id::unique(("props-copy-path", clip.0)), Sense::click()).on_hover_text(path);
            bui.painter().rect_filled(copy_r, 3.0, if copy_resp.hovered() { t.hover } else { t.tl_header_bg });
            bui.painter().text(copy_r.center(), Align2::CENTER_CENTER, tl!("Copy"), Tokens::ui(10.0), t.text);
            if copy_resp.clicked() {
                ui.ctx().copy_text(path.clone());
            }
            let val_x = r.min.x + 132.0;
            let max_w = (copy_r.min.x - val_x - 6.0).max(30.0);
            let mut job = egui::text::LayoutJob::simple_singleline(path.clone(), Tokens::ui(11.0), t.text);
            job.wrap.max_width = max_w;
            job.wrap.max_rows = 1;
            let g = bui.painter().layout_job(job);
            bui.painter().with_clip_rect(Rect::from_min_max(pos2(val_x, r.min.y), pos2(copy_r.min.x - 4.0, r.max.y))).galley(
                pos2(val_x, r.center().y - g.size().y * 0.5),
                g,
                t.text,
            );
            bui.interact(
                Rect::from_min_max(pos2(val_x, r.min.y), pos2(copy_r.min.x - 4.0, r.max.y)),
                egui::Id::unique(("props-path-tip", clip.0)),
                Sense::hover(),
            )
            .on_hover_text(path);
        }
        filmcraft_project::MediaRef::Generator(generator) => {
            props_kv(bui, clip, tl!("Source:"), format!("{generator:?}"), t);
        }
    }
}

/// Where a row's highlight (hover, selection) is painted: the row less its first point, which is
/// where the focused panel's outline runs, so the highlight doesn't cut through the outline.
pub(crate) fn row_fill(r: Rect) -> Rect {
    Rect::from_min_max(pos2(r.min.x + 1.0, r.min.y), r.max)
}

/// Run the panel's actions (also the Color Grading panel's). Parameter changes made
/// while the mouse button is down (a drag) share one undo step, which the first change of each
/// press begins (#201); typed values and clicks stay separate steps. A drag value only changes once
/// the mouse moves, after the press frame, so the press that already began a step is remembered by
/// its start time. That memory is refreshed on every frame the button stays down — including frames
/// with no value change — so a Scale scrub that pauses between percents is still one undo (#887). A
/// change that brings its own `merge` key (a Program monitor handle changing several parameters)
/// keeps it.
pub(crate) fn run(app: &mut FilmcraftApp, ctx: &egui::Context, actions: Vec<(String, Value)>) {
    let (down, press) = ctx.input(|i| (i.pointer.any_down(), i.pointer.press_start_time()));
    let key = egui::Id::unique("effect-controls-drag-step");
    let begun = ctx.data(|d| d.get_temp::<Option<f64>>(key)).flatten();
    let mut wrote = false;
    for (cmd, mut p) in actions {
        if (cmd == "effects.setParam" || cmd == "color.groupSetParam") && down {
            if p.get("merge").is_none() {
                p["merge"] = json!(true);
            }
            // only the first write of a frame begins the step: a handle that changes several
            // parameters at once (position and anchor point) sends them together
            p["begin"] = json!(begun != press && !wrote);
            wrote = true;
        }
        if let Err(e) = app.session.execute(&cmd, p) {
            app.ui.status = e.to_string();
        }
    }
    if down && (wrote || begun.is_some()) {
        ctx.data_mut(|d| d.insert_temp(key, press));
    } else if !down {
        ctx.data_mut(|d| d.remove::<Option<f64>>(key));
    }
}

pub(crate) fn graph_key(clip: ClipId, idx: usize, pid: &str) -> String {
    format!("graph:{}:{}:{}", clip.0, idx, pid)
}

/// Parameters with value/velocity graphs: scalars, and points such as Position (#239).
pub(crate) fn graphable(v: &ParamValue) -> bool {
    matches!(v, ParamValue::Float(_) | ParamValue::Vec2(_))
}

/// The curves a graphed value draws: a scalar's one, or a point's X and Y.
fn graph_components(v: &ParamValue) -> Vec<f64> {
    match v {
        ParamValue::Float(x) => vec![*x],
        ParamValue::Vec2(p) => vec![p.x, p.y],
        _ => Vec::new(),
    }
}

/// Value and velocity graphs of an animated scalar or point parameter, drawn across the keyframe
/// lane. A point draws an X and a Y curve, and its velocity graph is the speed along its path.
/// Keyframes drag vertically (value); Bezier influence handles drag horizontally.
#[allow(clippy::too_many_arguments)]
pub(crate) fn graph_rows(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    body: Rect,
    clip: ClipId,
    idx: usize,
    mask: Option<usize>,
    pd: &filmcraft_project::ParamDef,
    param: &filmcraft_project::Param,
    lane: &Rect,
    it: &TrackItem,
    actions: &mut Vec<(String, Value)>,
) {
    let t = app.tokens;
    let (vr, _) = ui.allocate_exact_size(vec2(body.width(), 110.0), Sense::hover());
    let (velr, _) = ui.allocate_exact_size(vec2(body.width(), 64.0), Sense::hover());
    row_line(ui, velr, lane, &t);
    let speed = it.speed.abs().max(1e-6);
    let dur = it.duration.0.max(1) as f64;
    let to_media = |f: f64| it.source_in + Tick((f * dur * speed) as i64);
    let x_of = |f: f32| lane.min.x + f * lane.width();
    // samples: one row of curve values per sample (a point's NaN "auto" coordinate is skipped)
    let n = (lane.width() / 2.0).max(8.0) as usize;
    let vals: Vec<Vec<f64>> = (0..=n).map(|i| graph_components(&param.value_at(to_media(i as f64 / n as f64)))).collect();
    let curves = graph_components(&param.value).len();
    let at_keys: Vec<Vec<f64>> = param.keyframes.iter().map(|k| graph_components(&k.value)).collect();
    let (mut lo, mut hi) = vals.iter().chain(&at_keys).flatten().filter(|v| v.is_finite()).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    if lo > hi {
        (lo, hi) = (0.0, 0.0);
    }
    if hi - lo < 1e-6 {
        lo -= 1.0;
        hi += 1.0;
    }
    let pad = (hi - lo) * 0.12;
    let (mut lo, mut hi) = (lo - pad, hi + pad);
    if let ParamKind::Float { min, max, .. } = pd.kind {
        lo = lo.max(min);
        hi = hi.min(max);
    }
    // While the graph stays open its range only widens: refitted to the keyframes, a keyframe
    // dragged off the top or bottom edge was drawn back on that edge, the curve unchanged (#1918)
    let range_id = egui::Id::unique(("kfg-range", clip.0, idx, mask, pd.id));
    let frame = ui.ctx().cumulative_frame_nr();
    if let Some((a, b, seen)) = ui.data(|d| d.get_temp::<(f64, f64, u64)>(range_id))
        && frame.saturating_sub(seen) <= 1
    {
        (lo, hi) = (lo.min(a), hi.max(b));
    }
    ui.data_mut(|d| d.insert_temp(range_id, (lo, hi, frame)));
    let area = Rect::from_min_max(pos2(lane.min.x, vr.min.y + 4.0), pos2(lane.max.x, vr.max.y - 4.0));
    let y_of = |v: f64| area.max.y - ((v - lo) / (hi - lo)) as f32 * area.height();
    let p = ui.painter();
    p.rect_filled(area, 0.0, t.keyframe_plot_bg);
    for g in 1..4 {
        let y = area.min.y + area.height() * g as f32 / 4.0;
        p.line_segment([pos2(area.min.x, y), pos2(area.max.x, y)], Stroke::new(1.0, t.keyframe_plot_grid));
    }
    // range labels in the property column
    let dec = if let ParamKind::Float { decimals, .. } = pd.kind { decimals as usize } else { 1 };
    p.text(pos2(vr.max.x - 30.0, area.min.y + 6.0), Align2::RIGHT_CENTER, format!("{hi:.dec$}"), Tokens::ui(10.0), t.text_dim);
    p.text(pos2(vr.max.x - 30.0, area.max.y - 6.0), Align2::RIGHT_CENTER, format!("{lo:.dec$}"), Tokens::ui(10.0), t.text_dim);
    p.text(pos2(vr.min.x + 44.0, area.center().y), Align2::LEFT_CENTER, tl!("Value"), Tokens::ui(11.0), t.text_dim);
    let curve_color = |c: usize| if c == 0 { t.accent } else { Y_CURVE };
    if curves > 1 {
        p.text(pos2(vr.min.x + 44.0, area.center().y + 14.0), Align2::LEFT_CENTER, "X", Tokens::ui(10.5), curve_color(0));
        p.text(pos2(vr.min.x + 58.0, area.center().y + 14.0), Align2::LEFT_CENTER, "Y", Tokens::ui(10.5), curve_color(1));
    }
    for c in 0..curves {
        let line: Vec<Pos2> =
            vals.iter().enumerate().filter_map(|(i, v)| v.get(c).filter(|v| v.is_finite()).map(|v| pos2(x_of(i as f32 / n as f32), y_of(*v)))).collect();
        p.add(egui::Shape::line(line, Stroke::new(1.5, curve_color(c))));
    }
    // velocity (units per second, derivative of the sampled value; a point's speed along its path)
    let secs = (dur * speed / filmcraft_time::TICKS_PER_SECOND as f64 / n as f64).max(1e-9);
    let vel: Vec<f64> = vals
        .windows(2)
        .map(|w| {
            let [a, b] = w else { return 0.0 };
            let d: Vec<f64> = a.iter().zip(b).map(|(a, b)| (b - a) / secs).collect();
            match d.as_slice() {
                [v] => *v,
                d => d.iter().map(|v| v * v).sum::<f64>().sqrt(),
            }
        })
        .collect();
    let vmax = vel.iter().filter(|v| v.is_finite()).fold(1e-6f64, |a, v| a.max(v.abs())) * 1.15;
    let varea = Rect::from_min_max(pos2(lane.min.x, velr.min.y + 2.0), pos2(lane.max.x, velr.max.y - 4.0));
    p.rect_filled(varea, 0.0, t.keyframe_plot_bg);
    let vy = |v: f64| varea.center().y - (v / vmax) as f32 * varea.height() / 2.0;
    p.line_segment([pos2(varea.min.x, varea.center().y), pos2(varea.max.x, varea.center().y)], Stroke::new(1.0, t.keyframe_plot_axis));
    let vline: Vec<Pos2> = vel.iter().enumerate().filter(|(_, v)| v.is_finite()).map(|(i, v)| pos2(x_of((i as f32 + 0.5) / n as f32), vy(*v))).collect();
    p.add(egui::Shape::line(vline, Stroke::new(1.2, Color32::from_rgb(0xd0, 0xa0, 0x40))));
    p.text(pos2(velr.min.x + 44.0, varea.center().y), Align2::LEFT_CENTER, tl!("Velocity"), Tokens::ui(11.0), t.text_dim);
    p.text(pos2(velr.max.x - 30.0, varea.min.y + 6.0), Align2::RIGHT_CENTER, format!("{vmax:.1}/s"), Tokens::ui(10.0), t.text_dim);
    let p = p.clone();
    // keyframes + handles
    draw_graph_keyframes(app, ui, &p, clip, idx, mask, pd, param, it, lane, lo, hi, area, actions);
}

/// One parameter's keyframe diamonds on its graph, with the Bezier influence handles.
#[allow(clippy::too_many_arguments)]
fn draw_graph_keyframes(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    clip: ClipId,
    idx: usize,
    mask: Option<usize>,
    pd: &filmcraft_project::ParamDef,
    param: &filmcraft_project::Param,
    it: &TrackItem,
    lane: &Rect,
    lo: f64,
    hi: f64,
    area: Rect,
    actions: &mut Vec<(String, Value)>,
) {
    let t = app.tokens;
    let speed = it.speed.abs().max(1e-6);
    let dur = it.duration.0.max(1) as f64;
    let to_f = |m: Tick| ((m - it.source_in).0 as f64 / speed / dur) as f32;
    let x_of = |f: f32| lane.min.x + f * lane.width();
    let y_of = |v: f64| area.max.y - ((v - lo) / (hi - lo)) as f32 * area.height();
    let v_of = |y: f32| lo + ((area.max.y - y) / area.height()) as f64 * (hi - lo);
    let dec = if let ParamKind::Float { decimals, .. } = pd.kind { decimals as usize } else { 1 };
    let with_mask = |mut v: Value| -> Value {
        if let Some(k) = mask {
            v["mask"] = json!(k);
        }
        v
    };
    // keyframes + handles
    let ks = &param.keyframes;
    for (i, k) in ks.iter().enumerate() {
        let f = to_f(k.time);
        if !(-0.01..=1.01).contains(&f) {
            continue;
        }
        let eases_out =
            !matches!(k.interp, filmcraft_project::Interpolation::Linear | filmcraft_project::Interpolation::Hold | filmcraft_project::Interpolation::EaseIn);
        let eases_in =
            !matches!(k.interp, filmcraft_project::Interpolation::Linear | filmcraft_project::Interpolation::Hold | filmcraft_project::Interpolation::EaseOut);
        let comps = graph_components(&k.value);
        for (ci, &v) in comps.iter().enumerate() {
            if !v.is_finite() {
                continue;
            }
            let id = egui::Id::unique(("kfg", clip.0, idx, mask, pd.id, k.time.0));
            let id = if ci == 0 { id } else { id.with(ci) };
            let dy: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
            let c = pos2(x_of(f), y_of(v) + dy);
            let r = Rect::from_center_size(c, vec2(10.0, 10.0));
            let resp = ui.interact(r.expand(2.0), id, Sense::click_and_drag());
            resp.context_menu(|ui| {
                keyframe_menu(app, ui, KeyframeRef { clip: clip.0, effect: idx, param: pd.id.to_string(), mask, time: k.time }, k.interp, actions);
            });
            let auto_id = match (comps.len(), ci) {
                (1, _) => format!("effectControls.{}.graph.keyframe.{}", pd.id, k.time.0),
                (_, 0) => format!("effectControls.{}.graph.keyframe.{}.x", pd.id, k.time.0),
                _ => format!("effectControls.{}.graph.keyframe.{}.y", pd.id, k.time.0),
            };
            app.auto.add(&auto_id, r, "keyframe value");
            // influence handles: flat (ease) tangents with length ∝ influence × neighbouring segment;
            // a point's sit on its X curve only, as both coordinates share the influence
            let (eases_out, eases_in) = (ci == 0 && eases_out, ci == 0 && eases_in);
            for (side, on, nb) in [(1.0f32, eases_out, ks.get(i + 1)), (-1.0f32, eases_in && i > 0, if i > 0 { ks.get(i - 1) } else { None })] {
                let (true, Some(nb)) = (on, nb) else { continue };
                let seg = (x_of(to_f(nb.time)) - c.x).abs();
                let infl = if side > 0.0 { k.out_influence } else { k.in_influence } as f32;
                let hid = id.with(if side > 0.0 { "out" } else { "in" });
                let hdx: f32 = ui.data(|d| d.get_temp(hid)).unwrap_or(0.0);
                let hx = c.x + side * (infl * seg + hdx * side).clamp(seg * 0.01, seg);
                let hp = pos2(hx, c.y);
                p.line_segment([c, hp], Stroke::new(1.0, t.plot_handle_dim));
                p.circle_filled(hp, 3.5, t.keyframe_handle);
                let hr = ui.interact(Rect::from_center_size(hp, vec2(10.0, 10.0)), hid.with("h"), Sense::drag());
                if hr.dragged_by(egui::PointerButton::Primary) {
                    let nx = graph_drag_offset(ui, &hr, vec2(hdx, 0.0)).x;
                    ui.data_mut(|d| d.insert_temp(hid, nx));
                }
                if hr.drag_stopped() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    ui.data_mut(|d| d.remove::<f32>(hid));
                } else if hr.drag_stopped_by(egui::PointerButton::Primary) {
                    ui.data_mut(|d| d.remove::<f32>(hid));
                    let ni = ((infl * seg + hdx * side) / seg.max(1.0)).clamp(0.01, 1.0);
                    let key = if side > 0.0 { "outInfluence" } else { "inInfluence" };
                    actions.push((
                        "effects.setKeyframe".into(),
                        with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id, "mediaTime": k.time.0, key: ni})),
                    ));
                }
            }
            p.circle_filled(c, 4.5, if resp.dragged_by(egui::PointerButton::Primary) { t.hot_text } else { t.plot_handle });
            if resp.dragged_by(egui::PointerButton::Primary) {
                let ny = graph_drag_offset(ui, &resp, vec2(0.0, dy)).y;
                ui.data_mut(|d| d.insert_temp(id, ny));
                p.text(c + vec2(8.0, -10.0), Align2::LEFT_BOTTOM, format!("{:.dec$}", v_of(c.y)), Tokens::ui(10.5), t.hot_text);
            }
            // egui ends a drag on Escape: that stop abandons it (#580)
            if resp.drag_stopped() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                ui.data_mut(|d| d.remove::<f32>(id));
            } else if resp.drag_stopped_by(egui::PointerButton::Primary) {
                ui.data_mut(|d| d.remove::<f32>(id));
                let nv = v_of(c.y);
                let nv = if let ParamKind::Float { min, max, .. } = pd.kind { nv.clamp(min, max) } else { nv };
                let value = if comps.len() == 1 {
                    json!(nv)
                } else {
                    let mut moved = comps.clone();
                    if let Some(slot) = moved.get_mut(ci) {
                        *slot = nv;
                    }
                    json!(moved)
                };
                actions.push((
                    "effects.setKeyframe".into(),
                    with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id, "mediaTime": k.time.0, "value": value})),
                ));
            }
        }
    }
}

/// Read-only text for parameters without an inline editor (LUT references, free text, curves).
fn param_text(project: &filmcraft_project::Project, id: &str, value: &ParamValue) -> String {
    match value {
        ParamValue::Text(s) if id.ends_with("_lut") => filmcraft_render::luts::label(Some(project), s),
        ParamValue::Text(s) if s.is_empty() => tl!("None").into(),
        ParamValue::Text(s) => s.clone(),
        ParamValue::Curve(pts) if pts.is_empty() || *pts == [[0.0, 0.0], [1.0, 1.0]] => tl!("Default").into(),
        ParamValue::Curve(pts) => tlf!("Custom ({n} points)", n = pts.len()),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod param_text_tests {
    use super::*;

    #[test]
    fn readable_values_for_text_lut_and_curve_params() {
        let p = filmcraft_project::Project::default();
        assert_eq!(param_text(&p, "input_lut", &ParamValue::Text(String::new())), "None");
        assert_eq!(param_text(&p, "look_lut", &ParamValue::Text("builtin:look-teal-orange".into())), "Teal & Orange");
        assert_eq!(param_text(&p, "label", &ParamValue::Text(String::new())), "None");
        assert_eq!(param_text(&p, "label", &ParamValue::Text("Reel 3".into())), "Reel 3");
        assert_eq!(param_text(&p, "curve_luma", &ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]])), "Default");
        assert_eq!(param_text(&p, "hue_vs_sat", &ParamValue::Curve(vec![])), "Default");
        assert_eq!(param_text(&p, "curve_luma", &ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]])), "Custom (3 points)");
    }
}

#[cfg(test)]
mod drag_undo_tests {
    use serde_json::json;

    /// #201: a drag value changes only once the mouse moves, after the press frame; two drags of
    /// the same parameter are still two undo steps.
    #[test]
    fn each_press_begins_its_own_undo_step() {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        let mut app = crate::FilmcraftApp::new(s);
        let ctx = egui::Context::default();
        let opacity = |app: &crate::FilmcraftApp| {
            let q = app.session.active_sequence().unwrap();
            let it = q.find_item(filmcraft_project::ClipId(clip)).unwrap().1;
            it.effects.iter().find(|e| e.effect == "opacity").unwrap().params["opacity"].value.as_f64().unwrap()
        };
        let start = opacity(&app);
        let pos = egui::pos2(10.0, 10.0);
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut time = 0.0;
        let mut frame = |app: &mut crate::FilmcraftApp, events: Vec<egui::Event>, value: Option<f64>| {
            time += 0.1;
            let raw = egui::RawInput { time: Some(time), events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                let actions = value.map(|v| ("effects.setParam".to_string(), json!({"clip": clip, "effect": "opacity", "param": "opacity", "value": v})));
                super::run(app, ui.ctx(), actions.into_iter().collect());
            });
            out.textures_delta.clear();
        };
        for values in [[90.0, 70.0], [50.0, 60.0]] {
            frame(&mut app, vec![egui::Event::PointerMoved(pos), button(true)], None); // the press: nothing changes yet
            for v in values {
                frame(&mut app, vec![egui::Event::PointerMoved(pos)], Some(v));
            }
            frame(&mut app, vec![button(false)], None);
        }
        assert_eq!(opacity(&app), 60.0);
        app.session.undo();
        assert_eq!(opacity(&app), 70.0, "undo takes back only the second drag");
        app.session.undo();
        assert_eq!(opacity(&app), start, "and then the first");
    }

    /// #887: a Scale hot-text drag that pauses between percents (no `setParam` that frame) is
    /// still one undo, not one step per 1%.
    #[test]
    fn a_scale_drag_with_idle_frames_is_one_undo_step() {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        let mut app = crate::FilmcraftApp::new(s);
        let ctx = egui::Context::default();
        let scale = |app: &crate::FilmcraftApp| {
            let q = app.session.active_sequence().unwrap();
            let it = q.find_item(filmcraft_project::ClipId(clip)).unwrap().1;
            it.effects.iter().find(|e| e.effect == "motion").unwrap().params["scale"].value.as_f64().unwrap()
        };
        let start = scale(&app);
        let pos = egui::pos2(10.0, 10.0);
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut time = 0.0;
        let mut frame = |app: &mut crate::FilmcraftApp, events: Vec<egui::Event>, value: Option<f64>| {
            time += 0.1;
            let raw = egui::RawInput { time: Some(time), events, ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                let actions = value.map(|v| ("effects.setParam".to_string(), json!({"clip": clip, "effect": "motion", "param": "scale", "value": v})));
                super::run(app, ui.ctx(), actions.into_iter().collect());
            });
            out.textures_delta.clear();
        };
        frame(&mut app, vec![egui::Event::PointerMoved(pos), button(true)], None);
        for v in [101.0, 150.0, 200.0, 249.0] {
            frame(&mut app, vec![egui::Event::PointerMoved(pos)], Some(v));
            frame(&mut app, vec![egui::Event::PointerMoved(pos)], None); // idle: mouse down, no 1% tick
        }
        frame(&mut app, vec![button(false)], None);
        assert_eq!(scale(&app), 249.0);
        app.session.undo();
        assert_eq!(scale(&app), start, "one undo restores the scale from before the drag");
        assert_eq!(app.session.undo(), None, "the drag must not have left extra 1% steps");
    }
}

#[cfg(test)]
mod lane_tests {
    use super::*;

    /// A context with the app's fonts (the panels use the semibold family).
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &Tokens::for_kind(Default::default()));
        ctx
    }

    /// A project file is only checked for overflow, so a damaged one can carry a clip with no
    /// (or a negative) duration. Selecting it must still draw both panels, ruler, playhead and
    /// keyframe navigators included.
    #[test]
    fn a_clip_without_a_duration_still_draws() {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
        s.execute("effects.toggleAnimation", json!({"clip": clip.0, "effect": "motion", "param": "scale"})).unwrap();
        let seq = s.state.active_sequence.unwrap();
        let mut app = crate::FilmcraftApp::new(s);
        let ctx = context();
        for duration in [0, -5 * filmcraft_time::TICKS_PER_SECOND] {
            let project = std::sync::Arc::make_mut(&mut app.session.project);
            project.sequence_mut(seq).unwrap().find_item_mut(clip).unwrap().1.duration = Tick(duration);
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(700.0, 500.0));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(&mut app, ui, rect);
                properties_panel(&mut app, ui, rect);
            });
            out.textures_delta.clear();
        }
    }

    /// The ruler takes its numbers from the project (clip times, the sequence's frame rate) and
    /// the panel's size. Anything a project that opens can carry (times within `Tick::MIN..=MAX`,
    /// a rate within `SequenceSettings::validate`, or an unset one) must not make it panic or
    /// tick forever.
    #[test]
    fn ruler_survives_damaged_numbers() {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let mut it = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let ctx = context();
        let t = Tokens::for_kind(Default::default());
        let big = i64::from(u32::MAX);
        let rates = [(24000, 1001), (30, 1), (1000, 1), (1, 1), (1, big), (big, big), (0, 0), (-25, 1)];
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            for width in [300.0, 0.0, -40.0, 0.001, 1.0e9, f32::NAN, f32::INFINITY] {
                for duration in [1, 0, -7, filmcraft_time::TICKS_PER_SECOND, Tick::MAX.0 / 2] {
                    for start in [0, Tick::MIN.0, Tick::MAX.0 / 2] {
                        for (num, den) in rates {
                            it.start = Tick(start);
                            it.duration = Tick(duration);
                            let ruler = Rect::from_min_size(pos2(10.0, 10.0), vec2(width, RULER_H));
                            paint_ruler(ui.painter(), ruler, &it, filmcraft_time::FrameRate { num, den }, false, &t);
                        }
                    }
                }
            }
        });
        out.textures_delta.clear();
    }

    /// A five-second lane 300 px wide is labelled every two seconds, in sequence timecode.
    #[test]
    fn ruler_labels_sequence_timecode() {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let mut it = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let rate = filmcraft_time::FrameRate { num: 24, den: 1 };
        it.start = rate.tick_of(24 * 60);
        it.duration = rate.tick_of(24 * 5);
        let ctx = context();
        let t = Tokens::for_kind(Default::default());
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            paint_ruler(ui.painter(), Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, RULER_H)), &it, rate, false, &t);
        });
        out.textures_delta.clear();
        fn texts(s: egui::Shape, out: &mut Vec<String>) {
            match s {
                egui::Shape::Vec(v) => v.into_iter().for_each(|s| texts(s, out)),
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                _ => {}
            }
        }
        let mut labels = Vec::new();
        out.shapes.into_iter().for_each(|c| texts(c.shape, &mut labels));
        assert_eq!(labels, ["00:01:00:00", "00:01:02:00", "00:01:04:00"]);
    }
}

#[cfg(test)]
mod graph_drag_tests {
    use super::*;
    use filmcraft_project::{Interpolation, Keyframe, Param};

    struct Driver {
        app: FilmcraftApp,
        ctx: egui::Context,
        clip: ClipId,
        time: f64,
        param: &'static str,
    }

    impl Driver {
        fn new() -> Self {
            Self::with("scale", ParamValue::Float(40.0), ParamValue::Float(80.0))
        }

        fn with(param: &'static str, from: ParamValue, to: ParamValue) -> Self {
            let mut session = filmcraft_engine::Session::default();
            session.execute("file.openDemoProject", json!({})).unwrap();
            let seq = session.state.active_sequence.unwrap();
            let clip = session.active_sequence().unwrap().video_tracks[0].items[0].id;
            let it = std::sync::Arc::make_mut(&mut session.project).sequence_mut(seq).unwrap().find_item_mut(clip).unwrap().1;
            let mut a = Keyframe::new(it.source_in + Tick(it.duration.0 / 4), from.clone());
            let mut b = Keyframe::new(it.source_in + Tick(it.duration.0 * 3 / 4), to);
            a.interp = Interpolation::Bezier;
            b.interp = Interpolation::Bezier;
            it.effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.insert(param.into(), Param { value: from, keyframes: vec![a, b] });
            let app = FilmcraftApp::new(session);
            let ctx = egui::Context::default();
            crate::theme::install(&ctx, &app.tokens);
            Self { app, ctx, clip, time: 0.0, param }
        }

        fn param(&self) -> Param {
            self.app.session.active_sequence().unwrap().find_item(self.clip).unwrap().1.effect("motion").unwrap().params[self.param].clone()
        }

        fn undo_len(&mut self) -> usize {
            self.app.session.execute("history.list", json!({})).unwrap()["undo"].as_array().unwrap().len()
        }

        // Draw the actual scalar graph and execute its emitted command, as the panel does.
        // Current paint shapes give a real held-frame position rather than previous telemetry.
        fn frame(&mut self, events: Vec<egui::Event>) -> [Pos2; 2] {
            self.time += 0.1;
            self.app.auto.begin_frame();
            let mut out = self.ctx.run_ui(egui::RawInput { time: Some(self.time), events, ..Default::default() }, |ui| {
                let it = self.app.session.active_sequence().unwrap().find_item(self.clip).unwrap().1.clone();
                let index = it.effects.iter().position(|e| e.effect == "motion").unwrap();
                let effect = &it.effects[index];
                let pd = effect.def().unwrap().param(self.param).unwrap();
                let body = Rect::from_min_size(pos2(8.0, 8.0), vec2(320.0, 200.0));
                let lane = Rect::from_min_max(pos2(360.0, 8.0), pos2(760.0, 220.0));
                let mut actions = Vec::new();
                graph_rows(&mut self.app, ui, body, self.clip, index, None, pd, &effect.params[self.param], &lane, &it, &mut actions);
                run(&mut self.app, ui.ctx(), actions);
            });
            out.textures_delta.clear();
            fn first_circle(shape: &egui::Shape, radius: f32) -> Option<Pos2> {
                match shape {
                    egui::Shape::Circle(circle) if circle.radius == radius => Some(circle.center),
                    egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| first_circle(shape, radius)),
                    _ => None,
                }
            }
            [4.5, 3.5].map(|radius| {
                out.shapes.iter().find_map(|shape| first_circle(&shape.shape, radius)).expect("scalar keyframe and outgoing influence are painted")
            })
        }
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    /// #1918, #1921: a keyframe dragged to a new value stays where it was dropped. The graph
    /// used to fit itself to the new values, so with two keyframes the one at the bottom (or
    /// the top) went straight back there and the curve looked unchanged.
    #[test]
    fn a_dropped_graph_keyframe_stays_where_it_was_dropped() {
        for (key, offset) in [(0, vec2(0.0, -20.0)), (1, vec2(0.0, 20.0))] {
            let mut d = Driver::new();
            let before = d.param();
            d.frame(vec![]);
            let id = format!("effectControls.scale.graph.keyframe.{}", before.keyframes[key].time.0);
            let [x, y, w, h] = d.app.auto.find(&id).expect("the keyframe is drawn").rect;
            let start = pos2(x + w / 2.0, y + h / 2.0);
            d.frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
            d.frame(vec![egui::Event::PointerMoved(start + offset)]);
            d.frame(vec![button(start + offset, false)]);
            d.frame(vec![]);
            d.frame(vec![]);
            let changed = d.param();
            assert_ne!(changed.keyframes[key].value, before.keyframes[key].value, "keyframe {key} takes the dropped value");
            let [x, y, w, h] = d.app.auto.find(&id).expect("the keyframe is drawn").rect;
            let now = pos2(x + w / 2.0, y + h / 2.0);
            assert!(now.distance(start + offset) < 0.5, "keyframe {key} stays where it was dropped: dropped at {:?}, drawn at {now:?}", start + offset);
        }
    }

    #[test]
    fn scalar_graph_press_starts_at_the_pointer_origin_and_release_is_one_undo_step() {
        for influence in [false, true] {
            for batched in [true, false] {
                let mut d = Driver::new();
                let before = d.param();
                let undo = d.undo_len();
                let target = usize::from(influence);
                let start = d.frame(vec![])[target];
                // A different prior cursor makes the pre-press movement observable.
                d.frame(vec![egui::Event::PointerMoved(start - vec2(80.0, 40.0))]);
                if batched {
                    d.frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
                } else {
                    d.frame(vec![egui::Event::PointerMoved(start)]);
                    d.frame(vec![button(start, true)]);
                }
                let pressed = d.frame(vec![])[target];
                assert!(pressed.distance(start) < 0.05, "press cannot jump: influence={influence}, batched={batched}, start={start:?}, pressed={pressed:?}");
                assert_eq!(d.param(), before);
                assert_eq!(d.undo_len(), undo);
                let offset = if influence { vec2(20.0, 0.0) } else { vec2(0.0, -20.0) };
                d.frame(vec![egui::Event::PointerMoved(start + offset)]);
                let preview = d.frame(vec![])[target];
                assert!(
                    preview.distance(start + offset) < 0.05,
                    "preview follows only gesture motion: influence={influence}, batched={batched}, start={start:?}, preview={preview:?}"
                );
                assert_eq!(d.param(), before, "held preview cannot commit");
                assert_eq!(d.undo_len(), undo);
                d.frame(vec![button(start + offset, false)]);
                d.frame(vec![]);
                let changed = d.param();
                assert_eq!(d.undo_len(), undo + 1);
                assert_eq!(changed.keyframes[0].time, before.keyframes[0].time);
                assert_eq!(changed.keyframes[1], before.keyframes[1]);
                if influence {
                    assert_eq!(changed.keyframes[0].value, before.keyframes[0].value);
                    assert!(changed.keyframes[0].out_influence > before.keyframes[0].out_influence);
                } else {
                    assert!(changed.keyframes[0].value.as_f64().unwrap() > before.keyframes[0].value.as_f64().unwrap());
                    assert_eq!(changed.keyframes[0].out_influence, before.keyframes[0].out_influence);
                }
                d.app.session.undo().expect("one graph gesture to undo");
                assert_eq!(d.param(), before);
                assert_eq!(d.undo_len(), undo);
                d.app.session.redo().expect("the graph gesture to redo");
                assert_eq!(d.param(), changed);
                assert_eq!(d.undo_len(), undo + 1);
            }
        }
    }

    /// #239: Position (a point) has graphs too; dragging its X keyframe moves only X, in one undo step.
    #[test]
    fn position_graph_drags_one_coordinate_of_a_point_keyframe() {
        use filmcraft_geom::Vec2;
        let mut d = Driver::with("position", ParamValue::Vec2(Vec2::new(400.0, 300.0)), ParamValue::Vec2(Vec2::new(800.0, 500.0)));
        assert!(graphable(&d.param().value));
        let before = d.param();
        let undo = d.undo_len();
        let start = d.frame(vec![])[0];
        assert!(d.app.auto.find(&format!("effectControls.position.graph.keyframe.{}.y", before.keyframes[0].time.0)).is_some());
        d.frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
        d.frame(vec![egui::Event::PointerMoved(start - vec2(0.0, 20.0))]);
        d.frame(vec![button(start - vec2(0.0, 20.0), false)]);
        d.frame(vec![]);
        let changed = d.param();
        assert_eq!(d.undo_len(), undo + 1);
        let (old, new) = (before.keyframes[0].value.as_vec2().unwrap(), changed.keyframes[0].value.as_vec2().unwrap());
        assert!(new.x > old.x, "dragging the X keyframe up raises X: {old:?} -> {new:?}");
        assert_eq!(new.y, old.y);
        assert_eq!(changed.keyframes[1], before.keyframes[1]);
    }
}

#[cfg(test)]
mod frame_hold_tests {
    //! #1675: on a Frame Hold clip with Hold Filters off the effects keep animating through the
    //! hold, so the panels that show effect values read them where the render does, as Effect
    //! Controls does, and not on the held frame.
    use super::*;
    use crate::panels::audio_fx_editor::FxTarget;
    use filmcraft_geom::Vec2;
    use filmcraft_project::{Keyframe, Param};

    #[derive(Clone, Copy, Debug)]
    enum Panel {
        Lumetri,
        Masks,
        TransformHandles,
        AudioFxEditor,
    }

    fn animate(it: &mut TrackItem, effect: &str, param: &str, from: ParamValue, to: ParamValue) {
        let (a, b) = (it.moving_source_time_at(it.start), it.moving_source_time_at(it.end() - Tick(1)));
        let e = it.effects.iter_mut().find(|e| e.effect == effect).unwrap();
        e.params.insert(param.into(), Param { value: from.clone(), keyframes: vec![Keyframe::new(a, from), Keyframe::new(b, to)] });
    }

    /// A V1 clip and an A1 clip under the playhead, their effects animated from
    /// their In to their Out, each held on its In (`Some(hold_filters)`) or not held.
    fn app(hold: Option<bool>) -> FilmcraftApp {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let v = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        // away from the In, where the held and the moving times meet
        let ph = v.start + Tick(v.duration.0 / 2);
        s.execute("playhead.set", json!({"time": ph.0})).unwrap();
        let a = s.active_sequence().unwrap().audio_tracks.iter().find_map(|t| t.item_at(ph)).unwrap().id;
        s.execute("effects.apply", json!({"clips": [v.id.0], "effect": "lumetri"})).unwrap();
        s.execute("effects.apply", json!({"clips": [a.0], "effect": "parametric_eq"})).unwrap();
        s.execute("timeline.select", json!({"clips": [v.id.0]})).unwrap();
        s.execute("masks.add", json!({"clip": v.id.0, "effect": "opacity", "shape": "ellipse"})).unwrap();
        s.execute("effects.select", json!({"clip": v.id.0, "effect": "motion"})).unwrap();
        if let Some(hold_filters) = hold {
            s.execute("clip.frameHoldOptions", json!({"clips": [v.id.0], "holdOn": "in", "holdFilters": hold_filters})).unwrap();
        }
        let seq = s.state.active_sequence.unwrap();
        let q = std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
        let it = q.find_item_mut(v.id).unwrap().1;
        animate(it, "motion", "position", ParamValue::Vec2(Vec2::new(400.0, 300.0)), ParamValue::Vec2(Vec2::new(800.0, 500.0)));
        animate(it, "lumetri", "saturation", ParamValue::Float(50.0), ParamValue::Float(150.0));
        let it = q.find_item_mut(a).unwrap().1;
        animate(it, "parametric_eq", "b1_gain", ParamValue::Float(-12.0), ParamValue::Float(12.0));
        // Frame Hold Options takes video clips; an imported edit list can hold an audio clip
        if let Some(hold_filters) = hold {
            (it.frame_hold, it.hold_filters) = (Some(it.source_in), hold_filters);
        }
        let index = it.effects.iter().position(|e| e.effect == "parametric_eq").unwrap();
        let mut app = FilmcraftApp::new(s);
        app.ui.audio_fx_editors.push(FxTarget::Clip { clip: a.0, index });
        app
    }

    /// What `panel` draws, after a few frames (the editor window sizes itself on the first one).
    fn observe(mut app: FilmcraftApp, panel: Panel) -> String {
        let q = app.session.active_sequence().unwrap();
        let frame = (q.settings.width, q.settings.height);
        let pic = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 360.0));
        // a selected mask and a selected effect exclude each other
        if let Panel::Masks = panel {
            let clip = app.session.state.selection[0];
            app.session.execute("masks.select", json!({"clip": clip.0, "effect": "opacity", "mask": 0})).unwrap();
        }
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens);
        let mut shapes = String::new();
        for i in 0..3 {
            app.auto.begin_frame();
            let mut out = ctx.run_ui(egui::RawInput { time: Some(f64::from(i) * 0.1), ..Default::default() }, |ui| match panel {
                Panel::Lumetri => {
                    crate::panels::lumetri::show(&mut app, ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(360.0, 900.0)), crate::dock::PanelKind::LumetriColor)
                }
                Panel::Masks => crate::panels::masks::monitor_overlay(&mut app, ui, pic, frame),
                Panel::TransformHandles => crate::panels::transform_handles::monitor_overlay(&mut app, ui, pic, frame),
                Panel::AudioFxEditor => crate::panels::audio_fx_editor::show(&mut app, ui.ctx()),
            });
            out.textures_delta.clear();
            shapes = format!("{:?}", out.shapes);
        }
        shapes
    }

    #[test]
    fn panels_show_effect_values_where_the_render_takes_them_on_a_frame_hold() {
        for panel in [Panel::Lumetri, Panel::Masks, Panel::TransformHandles, Panel::AudioFxEditor] {
            let moving = observe(app(None), panel);
            let held = observe(app(Some(true)), panel);
            assert!(moving != held, "{panel:?}: with Hold Filters on it shows the held frame's values");
            assert!(observe(app(Some(false)), panel) == moving, "{panel:?}: with Hold Filters off it shows the values it shows without the hold");
        }
    }
}

#[cfg(test)]
mod uniform_scale_tests {
    //! #4381: with Uniform Scale on, Scale Width does nothing (Motion and Transform scale both
    //! axes by Scale / Scale Height), so its row is greyed out and its value can't be changed.
    use super::*;

    /// Drags the value of `effect`'s Scale Width in the panel, then clicks it, and returns its
    /// value afterwards and whether the click opened it for typing.
    fn drag_scale_width(effect: &str, uniform: bool) -> (f64, bool) {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        if effect != "motion" {
            s.execute("effects.apply", json!({"clips": [clip.0], "effect": effect})).unwrap();
        }
        s.execute("effects.setParam", json!({"clip": clip.0, "effect": effect, "param": "uniform_scale", "value": uniform})).unwrap();
        s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
        let mut app = FilmcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 1600.0));
        let mut time = 0.0;
        let mut frame = |app: &mut FilmcraftApp, events: Vec<egui::Event>| {
            time += 0.1;
            app.auto.begin_frame();
            let mut out = ctx.run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| show(app, ui, rect));
            out.textures_delta.clear();
        };
        let effects = |app: &FilmcraftApp| app.session.active_sequence().unwrap().find_item(clip).unwrap().1.effects.clone();
        let idx = effects(&app).iter().position(|e| e.effect == effect).unwrap();
        let width = |app: &FilmcraftApp| effects(app)[idx].params["scale_width"].value.as_f64().unwrap();
        app.ui.expanded_fx.push(format!("ec:{}:{}", clip.0, idx));
        frame(&mut app, vec![]);
        let [x, y, w, h] = app.auto.find(&format!("effectControls.{effect}.{idx}.scale_width.name")).expect("the Scale Width row is drawn").rect;
        // the value starts 6 points right of the name
        let start = pos2(x + w + 12.0, y + h / 2.0);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut app, vec![egui::Event::PointerMoved(start), button(start, true)]);
        for dx in [20.0, 40.0, 60.0] {
            frame(&mut app, vec![egui::Event::PointerMoved(start + vec2(dx, 0.0))]);
        }
        frame(&mut app, vec![button(start + vec2(60.0, 0.0), false)]);
        frame(&mut app, vec![]);
        let dragged = width(&app);
        frame(&mut app, vec![egui::Event::PointerMoved(start), button(start, true)]);
        frame(&mut app, vec![button(start, false)]);
        frame(&mut app, vec![]);
        let pkey = egui::Id::unique(("pv", clip.0, idx, "scale_width"));
        let typing = ctx.data(|d| d.get_temp::<String>(pkey.with("editing")).is_some());
        (dragged, typing)
    }

    #[test]
    fn scale_width_is_greyed_out_while_uniform_scale_is_on() {
        for effect in ["motion", "transform"] {
            let (width, typing) = drag_scale_width(effect, false);
            assert!(width > 100.0, "{effect}: with Uniform Scale off, dragging Scale Width changes it");
            assert!(typing, "{effect}: with Uniform Scale off, a click on Scale Width types a value");
            assert_eq!(drag_scale_width(effect, true), (100.0, false), "{effect}: with Uniform Scale on, Scale Width can't be dragged or typed");
        }
    }

    /// Animates Motion's Scale Width, opens its graphs, drags its second keyframe (mid-clip, clear
    /// of the divider) in the value graph and returns that keyframe's value afterwards.
    fn drag_scale_width_graph(uniform: bool) -> f64 {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let (clip, start, dur) = {
            let it = &s.active_sequence().unwrap().video_tracks[0].items[0];
            (it.id, it.start, it.duration)
        };
        for at in [start, start + Tick(dur.0 / 2)] {
            s.execute("effects.addKeyframe", json!({"clip": clip.0, "effect": "motion", "param": "scale_width", "time": at.0})).unwrap();
        }
        s.execute("effects.setParam", json!({"clip": clip.0, "effect": "motion", "param": "uniform_scale", "value": uniform})).unwrap();
        s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
        let mut app = FilmcraftApp::new(s);
        let effects = |app: &FilmcraftApp| app.session.active_sequence().unwrap().find_item(clip).unwrap().1.effects.clone();
        let idx = effects(&app).iter().position(|e| e.effect == "motion").unwrap();
        let second = |app: &FilmcraftApp| effects(app)[idx].params["scale_width"].keyframes[1].clone();
        let key = second(&app).time;
        // opened before Uniform Scale was turned on
        app.ui.expanded_fx.push(format!("ec:{}:{}", clip.0, idx));
        app.ui.expanded_fx.push(graph_key(clip, idx, "scale_width"));
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 1600.0));
        let mut time = 0.0;
        let mut frame = |app: &mut FilmcraftApp, events: Vec<egui::Event>| {
            time += 0.1;
            app.auto.begin_frame();
            let mut out = ctx.run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| show(app, ui, rect));
            out.textures_delta.clear();
        };
        frame(&mut app, vec![]);
        let [x, y, w, h] = app.auto.find(&format!("effectControls.scale_width.graph.keyframe.{}", key.0)).expect("the value graph is drawn").rect;
        let at = pos2(x + w / 2.0, y + h / 2.0);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut app, vec![egui::Event::PointerMoved(at), button(at, true)]);
        for dy in [10.0, 20.0, 30.0] {
            frame(&mut app, vec![egui::Event::PointerMoved(at - vec2(0.0, dy))]);
        }
        frame(&mut app, vec![button(at - vec2(0.0, 30.0), false)]);
        frame(&mut app, vec![]);
        second(&app).value.as_f64().unwrap()
    }

    #[test]
    fn scale_width_graph_is_greyed_out_while_uniform_scale_is_on() {
        assert!(drag_scale_width_graph(false) > 100.0, "with Uniform Scale off, the value graph changes Scale Width");
        assert_eq!(drag_scale_width_graph(true), 100.0, "with Uniform Scale on, the value graph can't change Scale Width");
    }

    /// Keys Scale and Scale Width with Uniform Scale off, selects every keyframe (Select All), turns
    /// Uniform Scale on (before or during the drag, as an undo or the control channel can) and
    /// drags the selected second Scale keyframe right. Returns the app afterwards, whether that
    /// Scale keyframe moved and whether Scale Width's keyframes stayed put.
    fn drag_scale_keyframe(uniform_on_mid_drag: bool) -> (FilmcraftApp, bool, bool) {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let (clip, start, dur) = {
            let it = &s.active_sequence().unwrap().video_tracks[0].items[0];
            (it.id, it.start, it.duration)
        };
        for param in ["scale", "scale_width"] {
            for at in [start, start + Tick(dur.0 / 2)] {
                s.execute("effects.addKeyframe", json!({"clip": clip.0, "effect": "motion", "param": param, "time": at.0})).unwrap();
            }
        }
        let uniform = |on: bool| json!({"clip": clip.0, "effect": "motion", "param": "uniform_scale", "value": on});
        s.execute("effects.setParam", uniform(false)).unwrap();
        s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
        let mut app = FilmcraftApp::new(s);
        let effects = |app: &FilmcraftApp| app.session.active_sequence().unwrap().find_item(clip).unwrap().1.effects.clone();
        let idx = effects(&app).iter().position(|e| e.effect == "motion").unwrap();
        let times = |app: &FilmcraftApp, param: &str| effects(app)[idx].params[param].keyframes.iter().map(|k| k.time).collect::<Vec<_>>();
        app.ui.focused = crate::dock::PanelKind::EffectControls;
        assert!(select_all_keyframes(&mut app));
        assert_eq!(
            app.ui.keyframe_selection.iter().filter(|k| k.param == "scale_width").count(),
            2,
            "with Uniform Scale off, Select All takes Scale Width's keyframes"
        );
        if !uniform_on_mid_drag {
            app.session.execute("effects.setParam", uniform(true)).unwrap();
        }
        let (before, mid) = (times(&app, "scale_width"), times(&app, "scale")[1]);
        app.ui.expanded_fx.push(format!("ec:{}:{}", clip.0, idx));
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 1600.0));
        let mut time = 0.0;
        let mut frame = |app: &mut FilmcraftApp, events: Vec<egui::Event>| {
            time += 0.1;
            app.auto.begin_frame();
            let mut out = ctx.run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| show(app, ui, rect));
            out.textures_delta.clear();
        };
        frame(&mut app, vec![]);
        let [x, y, w, h] = app.auto.find(&format!("effectControls.motion.scale.keyframe.{}", mid.0)).expect("Scale's keyframe is drawn").rect;
        let at = pos2(x + w / 2.0, y + h / 2.0);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut app, vec![egui::Event::PointerMoved(at), button(at, true)]);
        for dx in [10.0, 20.0, 40.0] {
            frame(&mut app, vec![egui::Event::PointerMoved(at + vec2(dx, 0.0))]);
            if uniform_on_mid_drag && dx == 20.0 {
                app.session.execute("effects.setParam", uniform(true)).unwrap();
            }
        }
        frame(&mut app, vec![button(at + vec2(40.0, 0.0), false)]);
        frame(&mut app, vec![]);
        let moved = times(&app, "scale")[1] != mid;
        let stayed = times(&app, "scale_width") == before;
        (app, moved, stayed)
    }

    /// Scale Width keyframes selected while Uniform Scale was off (Select All) leave the selection
    /// once it's on: dragging a selected Scale keyframe doesn't move them, and Select All skips them.
    #[test]
    fn scale_width_keyframes_leave_the_selection_while_uniform_scale_is_on() {
        let (mut app, moved, stayed) = drag_scale_keyframe(false);
        assert!(moved, "the selected Scale keyframes move");
        assert!(stayed, "Scale Width's keyframes, selected before Uniform Scale was on, stay put");
        assert!(select_all_keyframes(&mut app));
        assert!(app.ui.keyframe_selection.iter().all(|k| k.param != "scale_width"), "with Uniform Scale on, Select All skips Scale Width's keyframes");
    }

    /// Uniform Scale turned on during a group drag: the drop moves the Scale keyframes, not Scale
    /// Width's, which were in the drag when it began.
    #[test]
    fn scale_width_keyframes_leave_a_drag_when_uniform_scale_turns_on_during_it() {
        let (_, moved, stayed) = drag_scale_keyframe(true);
        assert!(moved, "the selected Scale keyframes move");
        assert!(stayed, "Scale Width's keyframes, disabled during the drag, stay put");
    }

    /// Whether the render ignores the Scale Width of effect `idx` of `it` at media time `mt`: the
    /// result is the same with Scale Width at 50 % as at 200 %. Motion through
    /// `filmcraft_render::motion_matrix`, graphic layers through `item_layer_specs`.
    fn render_ignores_scale_width(seq: &filmcraft_project::Sequence, it: &TrackItem, idx: usize, mt: Tick) -> bool {
        let with = |width: f64| {
            let mut it = it.clone();
            let p = it.effects[idx].params.get_mut("scale_width").unwrap();
            (p.value, p.keyframes) = (ParamValue::Float(width), Vec::new());
            if it.effects[idx].effect == "motion" {
                format!("{:?}", filmcraft_render::motion_matrix(seq, &it, (1920, 1080), None, mt))
            } else {
                let specs = filmcraft_render::graphic_clip::item_layer_specs(&it, mt, (1920, 1080));
                format!("{:?}", specs.iter().find(|(i, _)| *i == idx).unwrap().1.transform)
            }
        };
        with(50.0) == with(200.0)
    }

    /// Uniform Scale is read as the render reads it, so Scale Width is greyed out exactly when the
    /// render ignores it: Motion and Transform read the stored value even when `effects.addKeyframe`
    /// gave Uniform Scale a keyframe that differs from it, graphic layers read the keyframe; a
    /// missing or non-boolean Uniform Scale (an older or damaged project) counts as each reads it.
    #[test]
    fn uniform_scale_counts_as_the_render_reads_it() {
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let (clip, start) = {
            let it = &s.active_sequence().unwrap().video_tracks[0].items[0];
            (it.id, it.start)
        };
        // Motion's Uniform Scale on, with a keyframe that says off (the command allows it)
        let added = s.execute("effects.addKeyframe", json!({"clip": clip.0, "effect": "motion", "param": "uniform_scale", "time": start.0})).unwrap();
        let mt = Tick(added["mediaTime"].as_i64().unwrap());
        s.execute("effects.setKeyframe", json!({"clip": clip.0, "effect": "motion", "param": "uniform_scale", "mediaTime": mt.0, "value": false})).unwrap();
        let seq = s.active_sequence().unwrap().clone();
        let it = seq.find_item(clip).unwrap().1.clone();
        let motion = it.effects.iter().position(|e| e.effect == "motion").unwrap();
        let uniform = |it: &TrackItem, idx: usize| it.effects[idx].params["uniform_scale"].clone();
        assert_eq!(uniform(&it, motion).value, ParamValue::Bool(true));
        assert_eq!(uniform(&it, motion).value_at(mt), ParamValue::Bool(false), "the keyframe differs from the stored value");
        // the same on a graphic layer and on Transform, each also with Uniform Scale missing and non-boolean
        let mut it = it;
        it.effects.push(filmcraft_project::graphic::new_text_layer("A", Default::default(), 20.0));
        it.effects.push(filmcraft_project::find_effect("transform").unwrap().instance());
        let (layer, transform) = (it.effects.len() - 2, it.effects.len() - 1);
        for idx in [layer, transform] {
            let p = it.effects[idx].params.get_mut("uniform_scale").unwrap();
            p.add_keyframe(mt);
            p.keyframes[0].value = ParamValue::Bool(false);
        }
        let variants = |idx: usize| {
            let mut missing = it.clone();
            missing.effects[idx].params.remove("uniform_scale");
            let mut odd = it.clone();
            odd.effects[idx].params.get_mut("uniform_scale").unwrap().value = ParamValue::Float(1.0);
            [("keyframed", it.clone()), ("missing", missing), ("non-boolean", odd)]
        };
        for idx in [motion, layer] {
            for (case, it) in variants(idx) {
                let e = &it.effects[idx];
                let render = render_ignores_scale_width(&seq, &it, idx, mt);
                assert_eq!(ignored_by_uniform_scale(&it, e, "scale_width", mt), render, "{} ({case})", e.effect);
            }
        }
        // the render ignores Motion's Scale Width by its stored value, a graphic layer's by the keyframe
        assert!(render_ignores_scale_width(&seq, &variants(motion)[0].1, motion, mt));
        assert!(!render_ignores_scale_width(&seq, &variants(layer)[0].1, layer, mt));
        // Transform (`gpufx`, `vfx::distort`): the stored value, missing or non-boolean = off
        for ((case, it), render) in variants(transform).into_iter().zip([true, false, false]) {
            assert_eq!(ignored_by_uniform_scale(&it, &it.effects[transform], "scale_width", mt), render, "transform ({case})");
        }
    }
}
