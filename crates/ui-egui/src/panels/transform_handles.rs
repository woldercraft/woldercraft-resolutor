//! On-screen transform handles on the Program monitor (#639). Selecting Motion in Effect Controls
//! (its header or one of its properties), or double-clicking a clip's picture in the Program
//! monitor, draws a box around the clip's picture with eight handles and the anchor point. The
//! Transform effect gets the same box for its own properties while it is the selected effect.
//!
//! Dragging inside the box changes Position. A handle changes Scale about the anchor point; with
//! Uniform Scale off a side handle changes only the width and a top or bottom handle only the
//! height. Dragging just outside the box changes Rotation. The anchor point moves without moving
//! the picture: Position changes by the same amount. The values go through `effects.setParam` at
//! the playhead while the drag goes on, as dragging them in Effect Controls does, so a property
//! with its stopwatch on gets a keyframe there, and one drag is one undo step.
//!
//! Corner Pin gets a handle on each of its four corners while it is the selected effect (#776):
//! dragging one moves that corner, and dragging inside the outline through them moves all four,
//! through `effects.setParam` in the same way.
//!
//! Automation ids: `program.transform.box`, `program.transform.handle.<n>` (0–3 the corners TL,
//! TR, BR, BL, 4–7 the middles of the top, right, bottom and left edges, as for graphic layers),
//! `program.transform.rotate.<n>` (just outside corner `n`), `program.transform.anchor`;
//! `program.cornerPin.box`, `program.cornerPin.handle.<n>` (0–3 Upper Left, Upper Right, Lower
//! Right, Lower Left).

use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_geom::{Affine, Vec2};
use filmcraft_project::{ClipId, EffectDef, EffectInstance, TrackItem};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::panels::graphics::{frame_to_screen, handle_points, unscale};
use crate::state::Tool;

/// How close (in points) the pointer must be to a handle or the anchor point to grab it: well
/// beyond the drawn squares, so the small handles stay easy to take hold of.
const GRAB: f32 = 8.0;
/// The drawn handles, a little smaller than the graphic layers' (7 and 5 points) for precision:
/// the side of a corner square and of an edge square.
const CORNER: f32 = 5.0;
const EDGE: f32 = 4.0;
/// The anchor point, Premiere's ⊕: a circle of this radius with the cross inside it, in thin lines
/// whose crossing marks the exact point; and how close the pointer must be to take hold of it.
const ANCHOR_RADIUS: f32 = 7.0;
const ANCHOR_ARM: f32 = ANCHOR_RADIUS;
const ANCHOR_GRAB: f32 = 11.0;
/// The small anchor point beside the pointer while it is over the anchor point.
const BADGE_RADIUS: f32 = 3.0;
const BADGE_ARM: f32 = 5.0;
/// How far outside the box (from its edges and corners) the pointer rotates the picture.
const ROTATE_REACH: f32 = 24.0;
/// The `merge` key of a drag's changes: one undo step, whichever parameters the drag changes.
const MERGE: &str = "programTransform";
/// The same for a Corner Pin corner.
const PIN_MERGE: &str = "programCornerPin";
/// Corner Pin's corners, in the order of the box's (TL, TR, BR, BL).
const PIN_CORNERS: [&str; 4] = ["upper_left", "upper_right", "lower_right", "lower_left"];
/// How far inside the monitor's clip (which reaches 12 points past the picture area) a handle
/// that would be off the monitor stops, so that all of it shows and can be grabbed.
const REACH_INSET: f32 = 12.0 + CORNER;

/// What a press on the box takes hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// Handle `n` (see [`handle_points`]).
    Handle(usize),
    Rotate,
    Anchor,
}

/// The selected effect's box on the monitor and the values it is drawn from.
#[derive(Clone, Copy, Debug)]
struct Placement {
    /// Where the effect's Position is measured → screen: the sequence frame for Motion, the clip
    /// (through Motion) for Transform.
    outer: Affine,
    /// Clip pixels → screen.
    to_screen: Affine,
    /// The picture's box: TL, TR, BR, BL.
    quad: [Pos2; 4],
    /// Where the eight handles (see [`handle_points`]) are drawn and grabbed: on the box, or at
    /// the monitor's edge for those beyond it (see [`within`]).
    handles: [Pos2; 8],
    anchor_at: Pos2,
    position: Vec2,
    anchor: Vec2,
    /// Scale (Motion) or Scale Height (Transform), and Scale Width, in percent.
    height: f64,
    width: f64,
    rotation: f64,
    uniform: bool,
}

/// A clip and the index of one of its effects.
type Target = (ClipId, usize);

#[derive(Clone, Copy, Debug)]
struct Drag {
    sel: Target,
    grab: Grab,
    /// Where the button went down, and the box then: every frame of the drag sets the values the
    /// pointer's travel from there gives.
    start: Pos2,
    from: Placement,
    /// Rotate: the pointer's angle about the anchor point last frame and the turn so far, in
    /// degrees (a drag can go round more than once).
    angle: f64,
    turned: f64,
}

/// Scale's parameter: Motion's `scale` is the height when Uniform Scale is off.
fn height_param(effect: &str) -> &'static str {
    if effect == "transform" { "scale_height" } else { "scale" }
}

fn sp(m: &Affine, v: Vec2) -> Pos2 {
    let q = m.apply(v);
    pos2(q.x as f32, q.y as f32)
}

/// The box of a clip `size` pixels large that `m` places on the screen.
fn quad_of(m: &Affine, size: (u32, u32)) -> [Pos2; 4] {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| sp(m, Vec2::new(x, y)))
}

fn inside(quad: &[Pos2; 4], p: Pos2) -> bool {
    let sides = quad.iter().zip(quad.iter().cycle().skip(1)).map(|(a, b)| (*b - *a).x * (p - *a).y - (*b - *a).y * (p - *a).x);
    let (mut pos, mut neg) = (false, false);
    for s in sides {
        pos |= s > 0.0;
        neg |= s < 0.0;
    }
    pos != neg
}

/// The handles at `points` pulled into `view`, the part of the monitor on screen: a picture moved
/// past the monitor's edge keeps every handle in reach (#1393). The picture stays where it is;
/// only the squares stop at the edge. A `view` with no room leaves them where they are.
fn within<const N: usize>(points: [Pos2; N], view: Rect) -> [Pos2; N] {
    if !(view.is_finite() && view.is_positive()) {
        return points;
    }
    points.map(|p| pos2(p.x.max(view.min.x).min(view.max.x), p.y.max(view.min.y).min(view.max.y)))
}

/// The box of effect `e` (Motion or Transform) of clip `it` at media time `mt`, on the monitor
/// picture `pic` showing a `frame`-sized sequence, with its handles kept inside `reach`.
fn placement(app: &FilmcraftApp, it: &TrackItem, e: &EffectInstance, mt: Tick, pic: Rect, reach: Rect, frame: (u32, u32)) -> Option<Placement> {
    let seq = app.session.active_sequence()?;
    let project = &app.session.project;
    let size = filmcraft_render::source_size(project, it.item).unwrap_or(frame);
    let view = frame_to_screen(pic, frame);
    let motion = view.then_apply(&filmcraft_render::motion_matrix(seq, it, size, filmcraft_render::source_par(project, it.item), mt));
    // "auto" points (NaN) are where the render puts them
    let point = |id: &str, auto: Vec2| {
        let v = e.param(id).map_or(auto, |p| p.vec2_at(mt));
        Vec2::new(if v.x.is_nan() { auto.x } else { v.x }, if v.y.is_nan() { auto.y } else { v.y })
    };
    let centre = Vec2::new(f64::from(size.0) / 2.0, f64::from(size.1) / 2.0);
    let uniform = e.param("uniform_scale").and_then(|p| p.value.as_bool()).unwrap_or(true);
    let (height, width, rotation) = (e.f64_at(height_param(&e.effect), mt), e.f64_at("scale_width", mt), e.f64_at("rotation", mt));
    let (outer, to_screen, position, anchor) = if e.effect == "transform" {
        let (anchor, position) = (point("anchor", centre), point("position", centre));
        let (sh, sw) = (height / 100.0, if uniform { height } else { width } / 100.0);
        // as the render places it (`filmcraft_render::gpufx`, "transform")
        let axis = e.f64_at("skew_axis", mt);
        let shear = Affine { c: e.f64_at("skew", mt).to_radians().tan(), ..Affine::IDENTITY };
        let skew = Affine::rotate_deg(axis).then_apply(&shear).then_apply(&Affine::rotate_deg(-axis));
        let own = Affine::translate(position.x, position.y)
            .then_apply(&Affine::rotate_deg(rotation))
            .then_apply(&skew)
            .then_apply(&Affine::scale(sw, sh))
            .then_apply(&Affine::translate(-anchor.x, -anchor.y));
        (motion, motion.then_apply(&own), position, anchor)
    } else {
        let frame_centre = Vec2::new(f64::from(seq.settings.width) / 2.0, f64::from(seq.settings.height) / 2.0);
        (view, motion, point("position", frame_centre), point("anchor", centre))
    };
    let quad = quad_of(&to_screen, size);
    let anchor_at = sp(&to_screen, anchor);
    quad.iter().chain([&anchor_at]).all(|p| p.x.is_finite() && p.y.is_finite()).then_some(Placement {
        outer,
        to_screen,
        quad,
        handles: within(handle_points(&quad), reach),
        anchor_at,
        position,
        anchor,
        height,
        width,
        rotation,
        uniform,
    })
}

/// The selected effect (its clip and index), its clip and its definition, while it is an enabled
/// effect of a selected clip under the playhead.
fn selected_effect(app: &FilmcraftApp) -> Option<(Target, &TrackItem, &EffectInstance, &'static EffectDef)> {
    let sel = app.session.state.selected_effect.as_ref()?;
    if !app.session.state.selection.contains(&sel.clip) {
        return None;
    }
    let seq = app.session.active_sequence()?;
    let (_, it) = seq.find_item(sel.clip)?;
    if !it.range().contains(app.session.playhead()) {
        return None;
    }
    let index = sel.index(&it.effects)?;
    let e = it.effects.get(index).filter(|e| e.enabled)?;
    Some(((sel.clip, index), it, e, e.def()?))
}

/// The topmost enabled video clip selected in the timeline that covers the playhead.
fn selected_timeline_clip(app: &FilmcraftApp) -> Option<(ClipId, &TrackItem)> {
    let seq = app.session.active_sequence()?;
    let t = app.session.playhead();
    seq.video_tracks
        .iter()
        .rev()
        .filter(|tr| tr.enabled)
        .filter_map(|tr| tr.item_at(t))
        .find(|it| it.enabled && app.session.state.selection.contains(&it.id))
        .map(|it| (it.id, it))
}

/// Whether the Program monitor's Resolve-style On-Screen Controls are currently active.
fn overlay_mode(app: &FilmcraftApp) -> Option<crate::state::ViewerOverlayMode> {
    (app.ui.program.viewer_overlay_enabled && !app.playback.playing).then_some(app.ui.program.viewer_overlay_mode)
}

/// The selected effect (its clip and index) and its box, while it is Motion or Transform of a
/// selected clip under the playhead — or Motion of the selected timeline clip when Transform
/// On-Screen Controls are enabled.
fn selected_placement(app: &FilmcraftApp, pic: Rect, reach: Rect, frame: (u32, u32)) -> Option<(Target, &'static EffectDef, Placement, bool)> {
    if let Some((sel, it, e, def)) = selected_effect(app).filter(|s| s.3.id == "motion" || s.3.id == "transform") {
        let pl = placement(app, it, e, it.effect_time_at(app.session.playhead()), pic, reach, frame)?;
        return Some((sel, def, pl, true));
    }
    if overlay_mode(app) == Some(crate::state::ViewerOverlayMode::Transform)
        && let Some((clip, it)) = selected_timeline_clip(app)
        && let Some((idx, e)) = it.effects.iter().enumerate().find(|(_, e)| e.effect == "motion" && e.enabled)
        && let Some(def) = e.def()
    {
        let pl = placement(app, it, e, it.effect_time_at(app.session.playhead()), pic, reach, frame)?;
        return Some(((clip, idx), def, pl, false));
    }
    None
}

/// Corner Pin's four corners on the monitor and the values they are drawn from.
#[derive(Clone, Copy, Debug)]
struct Pins {
    /// Clip pixels (where Corner Pin's points are measured) → screen.
    to_screen: Affine,
    /// The corners in clip pixels, in the order of [`PIN_CORNERS`].
    points: [Vec2; 4],
    /// Where they are on screen, and where their handles are drawn and grabbed (kept in reach).
    at: [Pos2; 4],
    handles: [Pos2; 4],
}

/// The selected effect (its clip and index) and its corners, while it is Corner Pin of a selected
/// clip under the playhead.
fn selected_pins(app: &FilmcraftApp, pic: Rect, reach: Rect, frame: (u32, u32)) -> Option<(Target, Pins)> {
    let (sel, it, e) = if let Some((sel, it, e, _)) = selected_effect(app).filter(|s| s.3.id == "corner_pin") {
        (sel, it, e)
    } else if overlay_mode(app) == Some(crate::state::ViewerOverlayMode::EffectOverlay)
        && let Some((clip, it)) = selected_timeline_clip(app)
        && let Some((idx, e)) = it.effects.iter().enumerate().find(|(_, e)| e.effect == "corner_pin" && e.enabled)
    {
        ((clip, idx), it, e)
    } else {
        return None;
    };
    let seq = app.session.active_sequence()?;
    let project = &app.session.project;
    let mt = it.effect_time_at(app.session.playhead());
    let size = filmcraft_render::source_size(project, it.item).unwrap_or(frame);
    // Corner Pin warps the clip's own pixels before Motion places them (`filmcraft_render::vfx::corner_pin`)
    let to_screen = frame_to_screen(pic, frame).then_apply(&filmcraft_render::motion_matrix(seq, it, size, filmcraft_render::source_par(project, it.item), mt));
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let points = PIN_CORNERS.map(|id| {
        let v = e.param(id).map_or(Vec2::new(f64::NAN, f64::NAN), |p| p.vec2_at(mt));
        // "auto" points (NaN) are the clip's corners, where the render puts them
        let (fx, fy) = filmcraft_project::effect::auto_point("corner_pin", id).unwrap_or((0.5, 0.5));
        Vec2::new(if v.x.is_nan() { w * fx } else { v.x }, if v.y.is_nan() { h * fy } else { v.y })
    });
    let at = points.map(|p| sp(&to_screen, p));
    at.iter().all(|p| p.x.is_finite() && p.y.is_finite()).then_some((sel, Pins { to_screen, points, at, handles: within(at, reach) }))
}

/// A drag of Corner Pin's corner `n`, or of all four (`None`, a drag inside the outline).
#[derive(Clone, Copy, Debug)]
struct PinDrag {
    sel: Target,
    n: Option<usize>,
    start: Pos2,
    from: Pins,
}

/// Corner Pin's corners on the Program monitor: the outline through them and a handle on each.
/// Dragging a handle moves its corner; dragging inside the outline moves all four together.
fn pin_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, sel: Target, pins: &Pins, actions: &mut Vec<(String, Value)>) {
    let accent = app.tokens.accent;
    let id = egui::Id::unique("program-corner-pin");
    let grab = vec2(GRAB, GRAB) * 2.0;
    ui.painter().add(egui::Shape::closed_line(pins.at.to_vec(), Stroke::new(1.0, accent)));
    // the outline first, so the handles (on top) take a press on them
    let outline = Rect::from_points(&pins.at);
    app.auto.add("program.cornerPin.box", outline, "Corner Pin");
    let body = ui.interact(outline, id.with("box"), Sense::drag());
    let inside_at = |p: Option<Pos2>| p.is_some_and(|p| inside(&pins.at, p));
    pin_drag(ui, &body, sel, None, pins, inside_at(ui.input(|i| i.pointer.press_origin())), CursorIcon::Move, actions);
    if body.hovered() && inside_at(body.hover_pos()) {
        ui.ctx().set_cursor_icon(CursorIcon::Move);
    }
    // each corner resizes outward from the middle of the outline, as the Motion box's corners do
    let mid = pins.at.iter().fold(Pos2::ZERO, |m, p| m + p.to_vec2() / 4.0);
    for (n, c) in pins.handles.iter().enumerate() {
        let hr = Rect::from_center_size(*c, vec2(CORNER, CORNER) * 1.4);
        ui.painter().rect_filled(hr, 0.0, Color32::WHITE);
        ui.painter().rect_stroke(hr, 0.0, Stroke::new(1.0, accent), StrokeKind::Middle);
        let name = PIN_CORNERS.get(n).copied().unwrap_or_default();
        app.auto.add(&format!("program.cornerPin.handle.{n}"), Rect::from_center_size(*c, grab), name);
        let resp = ui.interact(Rect::from_center_size(*c, grab), id.with(n), Sense::drag());
        let cursor = pins.at.get(n).map_or(CursorIcon::Move, |p| resize_cursor(*p - mid));
        pin_drag(ui, &resp, sel, Some(n), pins, true, cursor, actions);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(cursor);
        }
    }
}

/// The left-button drag of `resp` on corner `n` (all four for `None`): the corners it moves, as
/// `effects.setParam` actions merged into one undo step, with `cursor` shown while it lasts. A
/// drag starts only when `grabbed`.
#[allow(clippy::too_many_arguments)]
fn pin_drag(
    ui: &egui::Ui,
    resp: &egui::Response,
    sel: Target,
    n: Option<usize>,
    pins: &Pins,
    grabbed: bool,
    cursor: CursorIcon,
    actions: &mut Vec<(String, Value)>,
) {
    let drag_id = egui::Id::unique("program-corner-pin").with("drag");
    let left = egui::PointerButton::Primary;
    if resp.drag_started_by(left) && grabbed {
        let start = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or_default();
        ui.data_mut(|d| d.insert_temp(drag_id, PinDrag { sel, n, start, from: *pins }));
    }
    let drag: Option<PinDrag> = ui.data(|d| d.get_temp(drag_id)).filter(|d: &PinDrag| d.sel == sel && d.n == n);
    if let Some(d) = drag
        && resp.dragged_by(left)
    {
        if resp.drag_started_by(left) || resp.drag_delta() != egui::Vec2::ZERO {
            let cur = resp.interact_pointer_pos().unwrap_or(d.start);
            let off = unscale(&d.from.to_screen, cur - d.start);
            for (k, (name, from)) in PIN_CORNERS.iter().zip(d.from.points).enumerate() {
                let v = from + off;
                // the range Effect Controls' fields take
                if n.is_none_or(|n| n == k) && v.x.is_finite() && v.y.is_finite() {
                    let value = json!([v.x.clamp(-100_000.0, 100_000.0), v.y.clamp(-100_000.0, 100_000.0)]);
                    actions.push(("effects.setParam".into(), json!({"clip": sel.0.0, "effect": sel.1, "param": name, "value": value, "merge": PIN_MERGE})));
                }
            }
        }
        ui.ctx().set_cursor_icon(cursor);
    }
    if resp.drag_stopped() && drag.is_some() {
        ui.data_mut(|d| d.remove::<PinDrag>(drag_id));
    }
}

#[derive(Clone, Copy, Debug)]
struct EcPointDrag {
    sel: Target,
    idx: usize,
    start: Pos2,
    from: Vec2,
    to_screen: Affine,
}

/// Interactive crosshair gizmos on the Program monitor for `ParamKind::Point` parameters of the
/// selected EffectCraft VFX effect (`"ec.*"`).
fn ec_point_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect, reach: Rect, frame: (u32, u32), actions: &mut Vec<(String, Value)>) {
    let (sel, eff_name, to_screen, point_items) = {
        let Some((sel, it, e, def)) = selected_effect(app).filter(|s| s.3.id.starts_with("ec.")) else {
            return;
        };
        let Some(seq) = app.session.active_sequence() else { return };
        let project = &app.session.project;
        let mt = it.effect_time_at(app.session.playhead());
        let size = filmcraft_render::source_size(project, it.item).unwrap_or(frame);
        let to_screen =
            frame_to_screen(pic, frame).then_apply(&filmcraft_render::motion_matrix(seq, it, size, filmcraft_render::source_par(project, it.item), mt));
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let items: Vec<(&'static str, &'static str, Vec2, Pos2)> = def
            .params
            .iter()
            .filter(|pd| matches!(pd.kind, filmcraft_project::ParamKind::Point))
            .take(8)
            .filter_map(|pd| {
                let v = e.param(pd.id).map_or(Vec2::new(f64::NAN, f64::NAN), |p| p.vec2_at(mt));
                let (fx, fy) = filmcraft_project::effect::auto_point(def.id, pd.id).unwrap_or((0.5, 0.5));
                let pt = Vec2::new(if v.x.is_finite() { v.x } else { w * fx }, if v.y.is_finite() { v.y } else { h * fy });
                let screen_pt = sp(&to_screen, pt);
                (screen_pt.x.is_finite() && screen_pt.y.is_finite()).then_some((pd.id, pd.label, pt, screen_pt))
            })
            .collect();
        (sel, def.name, to_screen, items)
    };
    if point_items.is_empty() {
        return;
    }
    let accent = Color32::from_rgb(160, 120, 255);
    let base_id = egui::Id::unique("program-ec-point");
    let drag_id = base_id.with("drag");
    let grab = vec2(ANCHOR_GRAB, ANCHOR_GRAB) * 1.6;
    let left = egui::PointerButton::Primary;

    for (idx, &(param_id, param_label, pt, screen_pt)) in point_items.iter().enumerate() {
        let clamped = if reach.width() > 0.0 && reach.height() > 0.0 && reach.min.x.is_finite() {
            pos2(screen_pt.x.clamp(reach.min.x, reach.max.x), screen_pt.y.clamp(reach.min.y, reach.max.y))
        } else {
            screen_pt
        };
        let painter = ui.painter();
        painter.circle_stroke(clamped, ANCHOR_RADIUS, Stroke::new(2.5, Color32::BLACK));
        painter.circle_stroke(clamped, ANCHOR_RADIUS, Stroke::new(1.2, accent));
        painter.line_segment([clamped - vec2(ANCHOR_ARM, 0.0), clamped + vec2(ANCHOR_ARM, 0.0)], Stroke::new(1.2, accent));
        painter.line_segment([clamped - vec2(0.0, ANCHOR_ARM), clamped + vec2(0.0, ANCHOR_ARM)], Stroke::new(1.2, accent));

        let hit_r = Rect::from_center_size(clamped, grab);
        app.auto.add(&format!("program.vfxPoint.{param_id}"), hit_r, param_label);
        let resp = ui.interact(hit_r, base_id.with(idx), Sense::drag()).on_hover_text(format!("{eff_name} · {param_label}"));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if resp.drag_started_by(left) {
            let start = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(clamped);
            ui.data_mut(|d| d.insert_temp(drag_id, EcPointDrag { sel, idx, start, from: pt, to_screen }));
        }
        let active_drag: Option<EcPointDrag> = ui.data(|d| d.get_temp(drag_id)).filter(|d: &EcPointDrag| d.sel == sel && d.idx == idx);
        if let Some(d) = active_drag
            && resp.dragged_by(left)
        {
            if resp.drag_started_by(left) || resp.drag_delta() != egui::Vec2::ZERO {
                let cur = resp.interact_pointer_pos().unwrap_or(d.start);
                let off = unscale(&d.to_screen, cur - d.start);
                let nv = d.from + off;
                if nv.x.is_finite() && nv.y.is_finite() {
                    let value = json!([nv.x.clamp(-100_000.0, 100_000.0), nv.y.clamp(-100_000.0, 100_000.0)]);
                    actions.push((
                        "effects.setParam".into(),
                        json!({"clip": sel.0.0, "effect": sel.1, "param": param_id, "value": value, "merge": "programVfxPoint"}),
                    ));
                }
            }
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if resp.drag_stopped() && active_drag.is_some() {
            ui.data_mut(|d| d.remove::<EcPointDrag>(drag_id));
        }
    }
}

fn grab_at(pl: &Placement, p: Pos2) -> Option<Grab> {
    if (pl.anchor_at - p).length() <= ANCHOR_GRAB {
        return Some(Grab::Anchor);
    }
    if let Some(n) = pl.handles.iter().position(|h| (*h - p).length() <= GRAB) {
        return Some(Grab::Handle(n));
    }
    if inside(&pl.quad, p) {
        return Some(Grab::Move);
    }
    // anywhere just outside the box rotates, beside an edge as beside a corner, as in Premiere
    let n = pl.quad.len();
    let near = (0..n).filter_map(|i| Some(seg_dist(p, *pl.quad.get(i)?, *pl.quad.get((i + 1) % n)?))).any(|d| d <= ROTATE_REACH);
    near.then_some(Grab::Rotate)
}

/// The distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length()
}

/// The scale factors (width, height) dragging handle `n` from `start` to `cur` gives: the
/// pointer's distance from the anchor point along the box's sides, relative to where it started.
fn scale_factors(pl: &Placement, n: usize, start: Pos2, cur: Pos2) -> (f64, f64) {
    let [tl, tr, _, bl] = pl.quad;
    let (ux, uy) = ((tr - tl).normalized(), (bl - tl).normalized());
    let (from, to) = (start - pl.anchor_at, cur - pl.anchor_at);
    let along = |dir: egui::Vec2| {
        let a = from.dot(dir);
        if a.abs() < 1.0 { 1.0 } else { f64::from((to.dot(dir) / a).max(0.0)) }
    };
    match (n, pl.uniform) {
        (0..=3, true) => {
            let f = along(from.normalized());
            (f, f)
        }
        (0..=3, false) => (along(ux), along(uy)),
        (4 | 6, true) => (along(uy), along(uy)),
        (4 | 6, false) => (1.0, along(uy)),
        (_, true) => (along(ux), along(ux)),
        (_, false) => (along(ux), 1.0),
    }
}

/// The parameter values a drag sets with the pointer at `cur`.
/// `moved` is the pointer's travel from the press, which Snap in Program Monitor may have adjusted.
fn changes(d: &Drag, effect: &str, cur: Pos2, moved: egui::Vec2) -> Vec<(&'static str, Value)> {
    let pl = &d.from;
    // the ranges Effect Controls' fields take
    let point = |v: Vec2| (v.x.is_finite() && v.y.is_finite()).then(|| json!([v.x.clamp(-100_000.0, 100_000.0), v.y.clamp(-100_000.0, 100_000.0)]));
    let number = |v: f64, max: f64| v.is_finite().then(|| json!(v.clamp(-max, max)));
    let scale = |v: f64| v.is_finite().then(|| json!(v.clamp(0.0, 10_000.0)));
    let out = match d.grab {
        Grab::Move => vec![("position", point(pl.position + unscale(&pl.outer, moved)))],
        Grab::Anchor => vec![("anchor", point(pl.anchor + unscale(&pl.to_screen, moved))), ("position", point(pl.position + unscale(&pl.outer, moved)))],
        Grab::Handle(n) => {
            let (fx, fy) = scale_factors(pl, n, d.start, cur);
            let (height, width) = (height_param(effect), "scale_width");
            match n {
                _ if pl.uniform => vec![(height, scale(pl.height * fy))],
                4 | 6 => vec![(height, scale(pl.height * fy))],
                5 | 7 => vec![(width, scale(pl.width * fx))],
                _ => vec![(height, scale(pl.height * fy)), (width, scale(pl.width * fx))],
            }
        }
        Grab::Rotate => vec![("rotation", number(pl.rotation + d.turned, 36_000.0))],
    };
    out.into_iter().filter_map(|(param, v)| Some((param, v?))).collect()
}

/// The resize cursor for a handle in direction `v` from the box's centre.
pub(crate) fn resize_cursor(v: egui::Vec2) -> CursorIcon {
    let deg = v.y.atan2(v.x).to_degrees().rem_euclid(180.0);
    if !(22.5..157.5).contains(&deg) {
        CursorIcon::ResizeHorizontal
    } else if deg < 67.5 {
        CursorIcon::ResizeNwSe
    } else if deg < 112.5 {
        CursorIcon::ResizeVertical
    } else {
        CursorIcon::ResizeNeSw
    }
}

fn centre(quad: &[Pos2; 4]) -> Pos2 {
    let [tl, _, br, _] = *quad;
    tl + (br - tl) * 0.5
}

fn angle_deg(v: egui::Vec2) -> f64 {
    f64::from(v.y.atan2(v.x).to_degrees())
}

/// Draws the box over the whole monitor (the ui's clip), not just the picture, so its handles show
/// beside the frame as well as on it.
fn draw(app: &mut FilmcraftApp, ui: &egui::Ui, pl: &Placement, name: &str) {
    let accent = app.tokens.accent;
    let painter = ui.painter();
    painter.add(egui::Shape::closed_line(pl.quad.to_vec(), Stroke::new(1.0, accent)));
    app.auto.add("program.transform.box", Rect::from_points(&pl.quad), name);
    let mid = centre(&pl.quad);
    // automation rects: the square inside the circle a press grabs in
    let grab = vec2(GRAB, GRAB) * std::f32::consts::SQRT_2;
    for (n, (q, c)) in pl.quad.iter().zip(&pl.handles).enumerate() {
        let hr = Rect::from_center_size(*c, vec2(CORNER, CORNER));
        painter.rect_filled(hr, 0.0, Color32::WHITE);
        painter.rect_stroke(hr, 0.0, Stroke::new(1.0, accent), StrokeKind::Middle);
        app.auto.add(&format!("program.transform.handle.{n}"), Rect::from_center_size(*c, grab), "scale handle");
        // just outside the corner, away from the box's centre
        let out = (*q - mid).normalized();
        app.auto.add(&format!("program.transform.rotate.{n}"), Rect::from_center_size(*q + out * 14.0, vec2(10.0, 10.0)), "rotate");
    }
    for (n, m) in pl.handles.iter().enumerate().skip(4) {
        painter.rect_filled(Rect::from_center_size(*m, vec2(EDGE, EDGE)), 0.0, Color32::WHITE);
        app.auto.add(&format!("program.transform.handle.{n}"), Rect::from_center_size(*m, grab), "scale handle");
    }
    let a = pl.anchor_at;
    painter.circle_stroke(a, ANCHOR_RADIUS, Stroke::new(1.0, accent));
    painter.line_segment([a - vec2(ANCHOR_ARM, 0.0), a + vec2(ANCHOR_ARM, 0.0)], Stroke::new(1.0, accent));
    painter.line_segment([a - vec2(0.0, ANCHOR_ARM), a + vec2(0.0, ANCHOR_ARM)], Stroke::new(1.0, accent));
    app.auto.add("program.transform.anchor", Rect::from_center_size(a, vec2(ANCHOR_GRAB, ANCHOR_GRAB) * std::f32::consts::SQRT_2), "anchor point");
}

/// The pointer for what it would take hold of at `at` (or has hold of). There is no system
/// cursor for rotating, so a code-drawn one replaces it there.
fn cursor(ui: &egui::Ui, pl: &Placement, grab: Grab, at: Pos2, pic: Rect) {
    let icon = match grab {
        Grab::Move => CursorIcon::Move,
        // the plain arrow with a small anchor point beside it, as Premiere shows, so it's clear the
        // anchor point is under the pointer and not the picture
        Grab::Anchor => {
            let painter = ui.painter().with_clip_rect(pic.expand(24.0));
            let b = at + vec2(14.0, 16.0);
            for (col, w) in [(Color32::BLACK, 3.0), (Color32::WHITE, 1.0)] {
                painter.circle_stroke(b, BADGE_RADIUS, Stroke::new(w, col));
                painter.line_segment([b - vec2(BADGE_ARM, 0.0), b + vec2(BADGE_ARM, 0.0)], Stroke::new(w, col));
                painter.line_segment([b - vec2(0.0, BADGE_ARM), b + vec2(0.0, BADGE_ARM)], Stroke::new(w, col));
            }
            CursorIcon::Default
        }
        Grab::Handle(n) => handle_points(&pl.quad).get(n).map_or(CursorIcon::Move, |h| resize_cursor(*h - centre(&pl.quad))),
        Grab::Rotate => {
            let out = (at - centre(&pl.quad)).normalized();
            rotate_pointer(&ui.painter().with_clip_rect(pic.expand(ROTATE_REACH + 16.0)), at, if out.is_finite() { out } else { vec2(0.0, -1.0) });
            CursorIcon::None
        }
    };
    ui.ctx().set_cursor_icon(icon);
}

/// Premiere's rotate pointer: a curved double arrow at `at`, bulging along `out` (away from the
/// box), so it turns to face outward wherever the pointer is around the box.
fn rotate_pointer(painter: &egui::Painter, at: Pos2, out: egui::Vec2) {
    const R: f32 = 9.0;
    const SWEEP: f32 = 1.6;
    let c = at - out * R;
    let base = out.y.atan2(out.x);
    let on_arc = |a: f32| c + vec2(a.cos(), a.sin()) * R;
    let arc: Vec<Pos2> = (0..=12).map(|i| on_arc(base + (i as f32 / 12.0 - 0.5) * SWEEP)).collect();
    // an arrowhead at each end, pointing on along the arc
    let head = |a: f32, dir: f32| {
        let (tip, along, across) = (on_arc(a), vec2(-a.sin(), a.cos()) * dir, vec2(a.cos(), a.sin()));
        vec![tip + along * 4.5, tip - across * 3.5, tip + across * 3.5]
    };
    let heads = [head(base + SWEEP / 2.0, 1.0), head(base - SWEEP / 2.0, -1.0)];
    painter.add(egui::Shape::line(arc.clone(), Stroke::new(3.5, Color32::BLACK)));
    painter.add(egui::Shape::line(arc, Stroke::new(1.5, Color32::WHITE)));
    for h in heads {
        painter.add(egui::Shape::convex_polygon(h, Color32::WHITE, Stroke::new(1.0, Color32::BLACK)));
    }
}

/// The top-most clip at the playhead whose picture is under `p`.
fn clip_under(app: &FilmcraftApp, pic: Rect, frame: (u32, u32), p: Pos2) -> Option<ClipId> {
    let seq = app.session.active_sequence()?;
    let t = app.session.playhead();
    let view = frame_to_screen(pic, frame);
    let project = &app.session.project;
    seq.video_tracks.iter().rev().filter(|tr| tr.enabled).filter_map(|tr| tr.item_at(t)).filter(|it| it.enabled).find_map(|it| {
        let size = filmcraft_render::source_size(project, it.item).unwrap_or(frame);
        let m = view.then_apply(&filmcraft_render::motion_matrix(seq, it, size, filmcraft_render::source_par(project, it.item), it.effect_time_at(t)));
        inside(&quad_of(&m, size), p).then_some(it.id)
    })
}

const CROP_MERGE: &str = "programCrop";
const CROP_COLOR: Color32 = Color32::from_rgb(0xe5, 0x7e, 0x2a);

#[derive(Clone, Copy, Debug)]
struct CropPlacement {
    clip: ClipId,
    has_crop: bool,
    size: (u32, u32),
    to_screen: Affine,
    /// Current `[left, top, right, bottom]` in percent `0.0..=100.0`.
    edges: [f64; 4],
    quad: [Pos2; 4],
    handles: [Pos2; 8],
}

#[derive(Clone, Copy, Debug)]
struct CropDrag {
    clip: ClipId,
    handle: usize,
    start: Pos2,
    from: [f64; 4],
    applied: bool,
}

fn crop_placement(app: &FilmcraftApp, pic: Rect, reach: Rect, frame: (u32, u32)) -> Option<CropPlacement> {
    let (clip, it) = selected_timeline_clip(app)?;
    let seq = app.session.active_sequence()?;
    let project = &app.session.project;
    let size = filmcraft_render::source_size(project, it.item).unwrap_or(frame);
    if size.0 == 0 || size.1 == 0 || frame.0 == 0 || frame.1 == 0 {
        return None;
    }
    let t = it.effect_time_at(app.session.playhead());
    let par = filmcraft_render::source_par(project, it.item);
    let to_screen = frame_to_screen(pic, frame).then_apply(&filmcraft_render::motion_matrix(seq, it, size, par, t));
    let crop_inst = it.effects.iter().find(|e| e.effect == "crop" && e.enabled);
    let pct = |name: &str| crop_inst.map_or(0.0_f64, |e| e.f64_at(name, t)).clamp(0.0, 99.0);
    let edges = [pct("left"), pct("top"), pct("right"), pct("bottom")];
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let x0 = edges[0] / 100.0 * w;
    let y0 = edges[1] / 100.0 * h;
    let x1 = (1.0 - edges[2] / 100.0) * w;
    let y1 = (1.0 - edges[3] / 100.0) * h;
    let quad = [sp(&to_screen, Vec2::new(x0, y0)), sp(&to_screen, Vec2::new(x1, y0)), sp(&to_screen, Vec2::new(x1, y1)), sp(&to_screen, Vec2::new(x0, y1))];
    if !quad.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
        return None;
    }
    Some(CropPlacement { clip, has_crop: crop_inst.is_some(), size, to_screen, edges, quad, handles: within(handle_points(&quad), reach) })
}

fn crop_handle_at(handles: &[Pos2; 8], p: Pos2) -> Option<usize> {
    handles.iter().enumerate().min_by(|(_, a), (_, b)| a.distance(p).total_cmp(&b.distance(p))).filter(|(_, h)| h.distance(p) <= GRAB).map(|(i, _)| i)
}

fn crop_cursor(ui: &egui::Ui, handle: usize) {
    let icon = match handle {
        0 | 2 => CursorIcon::ResizeNwSe,
        1 | 3 => CursorIcon::ResizeNeSw,
        4 | 6 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeHorizontal,
    };
    ui.ctx().set_cursor_icon(icon);
}

fn crop_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect, reach: Rect, frame: (u32, u32), actions: &mut Vec<(String, Value)>) {
    let Some(pl) = crop_placement(app, pic, reach, frame) else { return };
    let p = ui.painter();
    let outline = Stroke::new(1.5, CROP_COLOR);
    for i in 0..4 {
        p.line_segment([pl.quad[i], pl.quad[(i + 1) % 4]], outline);
    }
    for (i, &h) in pl.handles.iter().enumerate() {
        let side = if i < 4 { CORNER + 1.5 } else { EDGE + 1.5 };
        let r = Rect::from_center_size(h, vec2(side, side));
        p.rect_filled(r, 0.0, CROP_COLOR);
        p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
        app.auto.add(&format!("program.crop.handle.{i}"), r, &format!("crop handle {i}"));
    }
    let box_rect = Rect::from_points(&pl.quad).union(Rect::from_points(&pl.handles));
    app.auto.add("program.crop.box", box_rect, "crop box");

    let id = egui::Id::unique("program-crop");
    let drag_id = id.with("drag");
    let resp = ui.interact(box_rect.expand(GRAB), id, Sense::click_and_drag());
    let left = egui::PointerButton::Primary;
    if resp.drag_started_by(left) {
        let pos = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(box_rect.center());
        if let Some(handle) = crop_handle_at(&pl.handles, pos) {
            ui.data_mut(|d| d.insert_temp(drag_id, CropDrag { clip: pl.clip, handle, start: pos, from: pl.edges, applied: pl.has_crop }));
        } else {
            ui.data_mut(|d| d.remove::<CropDrag>(drag_id));
        }
    }
    let drag: Option<CropDrag> = ui.data(|d| d.get_temp(drag_id)).filter(|d: &CropDrag| d.clip == pl.clip);
    if let Some(mut d) = drag
        && resp.dragged_by(left)
    {
        crop_cursor(ui, d.handle);
        if !d.applied {
            actions.push(("effects.apply".into(), json!({"clip": pl.clip.0, "effect": "Crop"})));
            d.applied = true;
            ui.data_mut(|m| m.insert_temp(drag_id, d));
        }
        let cur = resp.interact_pointer_pos().unwrap_or(d.start);
        if let Some(inv) = pl.to_screen.inverse() {
            let a = inv.apply(Vec2::new(f64::from(d.start.x), f64::from(d.start.y)));
            let b = inv.apply(Vec2::new(f64::from(cur.x), f64::from(cur.y)));
            let dx_pct = (b.x - a.x) / f64::from(pl.size.0) * 100.0;
            let dy_pct = (b.y - a.y) / f64::from(pl.size.1) * 100.0;
            let mut next = d.from;
            let touch_left = matches!(d.handle, 0 | 3 | 7);
            let touch_top = matches!(d.handle, 0 | 1 | 4);
            let touch_right = matches!(d.handle, 1 | 2 | 5);
            let touch_bottom = matches!(d.handle, 2 | 3 | 6);
            if touch_left {
                next[0] = (d.from[0] + dx_pct).clamp(0.0, (99.0 - d.from[2]).max(0.0));
            }
            if touch_top {
                next[1] = (d.from[1] + dy_pct).clamp(0.0, (99.0 - d.from[3]).max(0.0));
            }
            if touch_right {
                next[2] = (d.from[2] - dx_pct).clamp(0.0, (99.0 - next[0]).max(0.0));
            }
            if touch_bottom {
                next[3] = (d.from[3] - dy_pct).clamp(0.0, (99.0 - next[1]).max(0.0));
            }
            for (idx, param) in ["left", "top", "right", "bottom"].iter().enumerate() {
                if (next[idx] - pl.edges[idx]).abs() > 1e-3 || resp.drag_started_by(left) {
                    actions.push((
                        "effects.setParam".into(),
                        json!({"clip": pl.clip.0, "effect": "crop", "param": param, "value": next[idx], "merge": CROP_MERGE}),
                    ));
                }
            }
        }
    } else if let Some(pos) = resp.hover_pos()
        && let Some(handle) = crop_handle_at(&pl.handles, pos)
    {
        crop_cursor(ui, handle);
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<CropDrag>(drag_id));
    }
}

/// The selected effect's box (or Corner Pin's corners) on the Program monitor picture `pic`, and
/// double-clicking a clip's picture to select its Motion.
pub fn monitor_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect, frame: (u32, u32)) {
    if app.ui.tool != Tool::Selection || app.ui.mask_pen.is_some() {
        return;
    }
    let mut actions: Vec<(String, Value)> = Vec::new();
    let id = egui::Id::unique("program-transform");
    let drag_id = id.with("drag");
    let double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
    // the monitor around the picture as well as on it (its clip reaches past the picture area)
    let reach = ui.clip_rect().shrink(REACH_INSET);
    let mode = overlay_mode(app);
    if mode == Some(crate::state::ViewerOverlayMode::Crop) {
        crop_overlay(app, ui, pic, reach, frame, &mut actions);
        crate::panels::effect_controls::run(app, ui.ctx(), actions);
        return;
    }
    if mode != Some(crate::state::ViewerOverlayMode::EffectOverlay)
        && let Some((sel, def, pl, from_effect_sel)) = selected_placement(app, pic, reach, frame)
    {
        draw(app, ui, &pl, def.name);
        // (as far again beyond the rotate zone, where a click lets go of the box; and the handles
        // held at the monitor's edge)
        let area = Rect::from_points(&pl.quad).union(Rect::from_points(&pl.handles));
        let resp = ui.interact(area.expand(ROTATE_REACH * 2.0), id, Sense::click_and_drag());
        // the left button only: a middle or right drag leaves the picture alone
        let left = egui::PointerButton::Primary;
        if resp.drag_started_by(left) {
            // where the button went down (a drag starts only after the pointer has moved a little)
            let p = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(pl.anchor_at);
            match grab_at(&pl, p) {
                Some(grab) => {
                    let angle = angle_deg(p - pl.anchor_at);
                    ui.data_mut(|d| d.insert_temp(drag_id, Drag { sel, grab, start: p, from: pl, angle, turned: 0.0 }));
                }
                None => ui.data_mut(|d| d.remove::<Drag>(drag_id)),
            }
        }
        let drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id)).filter(|d: &Drag| d.sel == sel);
        if let Some(mut d) = drag
            && resp.dragged_by(left)
        {
            let cur = resp.interact_pointer_pos().unwrap_or(d.start);
            let off = cur - d.from.anchor_at;
            if d.grab == Grab::Rotate && off.length() >= 1.0 {
                let a = angle_deg(off);
                d.turned += (a - d.angle + 180.0).rem_euclid(360.0) - 180.0;
                d.angle = a;
                ui.data_mut(|m| m.insert_temp(drag_id, d));
            }
            // Snap in Program Monitor: a Move drag snaps the box to the frame and guides (hold ⌘/Ctrl to move freely)
            let mut moved = cur - d.start;
            if d.grab == Grab::Move && !ui.input(|i| i.modifiers.command) {
                let (snapped, lines) = crate::panels::monitor_view::snap_move(app, pic, frame, Rect::from_points(&d.from.quad), moved);
                moved = snapped;
                crate::panels::monitor_view::draw_snap_lines(ui.painter(), pic, &lines);
            }
            // (also when only the snapping changed: ⌘/Ctrl pressed or released with the pointer still)
            let moved_id = id.with("moved");
            let last: Option<egui::Vec2> = ui.data(|m| m.get_temp(moved_id));
            if resp.drag_started_by(left) || resp.drag_delta() != egui::Vec2::ZERO || last.is_some_and(|l| l != moved) {
                ui.data_mut(|m| m.insert_temp(moved_id, moved));
                for (param, value) in changes(&d, def.id, cur, moved) {
                    actions.push(("effects.setParam".into(), json!({"clip": sel.0.0, "effect": sel.1, "param": param, "value": value, "merge": MERGE})));
                }
            }
            cursor(ui, &pl, d.grab, cur, pic);
        } else if let Some(p) = resp.hover_pos()
            && let Some(grab) = grab_at(&pl, p)
        {
            cursor(ui, &pl, grab, p, pic);
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| {
                d.remove::<Drag>(drag_id);
                d.remove::<egui::Vec2>(id.with("moved"));
            });
        }
        // a click beside the box (not on it, a handle or a corner's rotate zone) lets go of it
        if from_effect_sel && resp.clicked() && !double && resp.interact_pointer_pos().is_some_and(|p| grab_at(&pl, p).is_none()) {
            actions.push(("effects.select".into(), json!({"none": true})));
        }
    }
    if let Some((sel, pins)) = selected_pins(app, pic, reach, frame) {
        pin_overlay(app, ui, sel, &pins, &mut actions);
    }
    ec_point_overlay(app, ui, pic, reach, frame, &mut actions);
    if double
        && app.ui.gfx_edit.is_none()
        && let Some(p) = ui.input(|i| i.pointer.interact_pos()).filter(|p| pic.contains(*p) && ui.rect_contains_pointer(pic))
        && let Some(clip) = clip_under(app, pic, frame, p)
    {
        if !app.session.state.selection.contains(&clip) {
            actions.push(("timeline.select".into(), json!({"clips": [clip.0]})));
        }
        actions.push(("effects.select".into(), json!({"clip": clip.0, "effect": "motion"})));
    }
    crate::panels::effect_controls::run(app, ui.ctx(), actions);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_past_the_monitor_stop_at_its_edge() {
        let view = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        // a box from the middle of the view to well past its bottom-right corner
        let quad = [pos2(50.0, 25.0), pos2(150.0, 25.0), pos2(150.0, 75.0), pos2(50.0, 75.0)];
        let h = within(handle_points(&quad), view);
        assert_eq!(h[0], pos2(50.0, 25.0), "a handle on the monitor stays on the box");
        assert_eq!(h[1], pos2(100.0, 25.0));
        assert_eq!(h[2], pos2(100.0, 50.0));
        assert_eq!(h[3], pos2(50.0, 50.0));
        assert_eq!(h[5], pos2(100.0, 50.0));
        assert!(h.iter().all(|p| view.contains(*p)));
        // no room to keep them in: left where they are, without crossed bounds
        let none = Rect::from_min_max(pos2(10.0, 10.0), pos2(0.0, 0.0));
        assert_eq!(within(handle_points(&quad), none), handle_points(&quad));
        let nan = Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(100.0, 50.0));
        assert_eq!(within(handle_points(&quad), nan), handle_points(&quad));
    }
}
