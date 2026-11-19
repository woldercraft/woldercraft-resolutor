use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use super::ColorPanelCtx;
use crate::theme::Tokens;

struct KnobSpec<'a> {
    id: &'static str,
    label: &'a str,
    short_label: &'a str,
    range: (f64, f64),
    default: f64,
    speed: f64,
    decimals: usize,
    unit: &'a str,
    track_neg: Color32,
    track_pos: Color32,
    active_neg: Color32,
    active_pos: Color32,
}

/// Resolve-style rotary knob with color-guided dual tracks, indicator pointer, drag/scroll interaction,
/// clickable numeric readout, and double-click reset.
fn rotary_knob(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, mut spec: KnobSpec<'_>, ctx: &mut ColorPanelCtx<'_>) -> Rect {
    let t = ctx.t;
    let pd = ctx.e.def().and_then(|d| d.param(spec.id));
    if let Some(pd) = pd
        && let filmcraft_project::ParamKind::Float { min, max, decimals, unit, .. } = pd.kind
    {
        spec.range = (min, max);
        spec.decimals = decimals as usize;
        if !unit.is_empty() {
            spec.unit = unit;
        }
        if let Some(def) = pd.default.as_f64() {
            spec.default = def;
        }
    }
    let cur_val = ctx.e.param(spec.id).map(|p| p.f64_at(ctx.mt)).unwrap_or(spec.default);
    let (item_r, _) = ui.allocate_exact_size(vec2(54.0, 48.0), Sense::hover());
    let knob_hit = Rect::from_min_max(item_r.min, pos2(item_r.max.x, item_r.max.y - 14.0));
    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, spec.id);
    let resp = ui.interact(knob_hit, egui::Id::unique(("lumetri-knob", scope)), Sense::click_and_drag());
    let p = ui.painter();

    // Drag & mouse-wheel interaction
    let mut new_val = cur_val;
    let mut merge_undo = false;
    if resp.dragged() {
        let delta = resp.drag_delta();
        let shift = ui.ctx().input(|i| i.modifiers.shift);
        let factor = if shift { 0.25 } else { 1.0 };
        let step = (delta.x - delta.y) as f64 * spec.speed * factor;
        new_val = (cur_val + step).clamp(spec.range.0, spec.range.1);
        merge_undo = true;
    }
    if resp.hovered() {
        ctx.knob_hovered = true;
        let (scroll_y, shift) = ui.ctx().input(|i| (i.smooth_scroll_delta.y, i.modifiers.shift));
        if scroll_y.abs() > 0.1 {
            ui.ctx().input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            let factor = if shift { 0.25 } else { 1.0 };
            let step = (scroll_y as f64 * 0.2) * spec.speed * factor;
            new_val = (new_val + step).clamp(spec.range.0, spec.range.1);
            merge_undo = true;
        }
    }
    if resp.double_clicked() {
        new_val = spec.default;
        merge_undo = false;
    }

    let (track_col_neg, track_col_pos, active_col_neg, active_col_pos) = (spec.track_neg, spec.track_pos, spec.active_neg, spec.active_pos);

    // Top Label in small caps
    let label_pos = pos2(item_r.center().x, item_r.min.y + 5.5);
    p.text(
        label_pos,
        Align2::CENTER_CENTER,
        spec.short_label,
        Tokens::semibold(9.0),
        if resp.hovered() { Color32::from_rgb(255, 185, 60) } else { t.text_dim },
    );

    if spec.id == "temperature" || spec.id == "tint" {
        p.circle_filled(pos2(item_r.center().x - 16.0, item_r.min.y + 5.5), 2.2, track_col_neg);
        p.circle_filled(pos2(item_r.center().x + 16.0, item_r.min.y + 5.5), 2.2, track_col_pos);
    }

    let kc = pos2(item_r.center().x, item_r.min.y + 21.5);
    let radius = 10.5;
    let is_active = resp.hovered() || resp.dragged();
    let ring_color = if is_active { Color32::from_rgb(255, 175, 40) } else { Color32::from_rgb(45, 48, 58) };
    p.circle_filled(kc, radius, Color32::from_rgb(20, 23, 29));
    p.circle_stroke(kc, radius, Stroke::new(1.0, ring_color));

    let span = (spec.range.1 - spec.range.0).max(1e-4);
    let norm = ((cur_val - spec.range.0) / span).clamp(0.0, 1.0) as f32;
    let def_norm = ((spec.default - spec.range.0) / span).clamp(0.0, 1.0) as f32;
    let min_ang = -135f32.to_radians();
    let max_ang = 135f32.to_radians();
    let cur_ang = min_ang + norm * (max_ang - min_ang);
    let def_ang = min_ang + def_norm * (max_ang - min_ang);

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

    let tick_tip = kc + vec2(def_ang.sin(), -def_ang.cos()) * (radius + 1.0);
    let tick_base = kc + vec2(def_ang.sin(), -def_ang.cos()) * (radius - 2.0);
    p.line_segment([tick_base, tick_tip], Stroke::new(1.2, Color32::from_rgb(140, 150, 170)));

    let is_modified = (cur_val - spec.default).abs() > 1e-4;
    let active_color = if cur_val < spec.default { active_col_neg } else { active_col_pos };
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

    let pointer_tip = kc + vec2(cur_ang.sin(), -cur_ang.cos()) * (radius - 2.5);
    let pointer_base = kc + vec2(cur_ang.sin(), -cur_ang.cos()) * 4.0;
    p.line_segment([pointer_base, pointer_tip], Stroke::new(2.0, Color32::WHITE));
    p.circle_filled(pointer_tip, 2.0, if is_modified { active_color } else { Color32::WHITE });

    p.circle_filled(kc, 3.0, Color32::from_rgb(45, 50, 62));
    p.circle_stroke(kc, 3.0, Stroke::new(1.0, Color32::from_rgb(85, 95, 115)));

    let mut num_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(item_r.min.x, item_r.max.y - 14.0), item_r.max))
            .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
    );
    let (_, typed_val) = crate::widgets::hot_number(
        &mut num_ui,
        spec.label,
        egui::Id::unique(("lumetri-knob-num", scope)),
        cur_val,
        crate::widgets::HotNumberOptions::new(spec.speed, spec.range, spec.decimals, spec.unit.trim(), t),
    );
    if let Some(v) = typed_val {
        new_val = v;
        merge_undo = false;
    }

    if !ctx.is_group_mode && pd.is_some_and(|d| d.animatable) {
        let kf_center = pos2(item_r.max.x - 5.5, item_r.min.y + 15.0);
        let kf_rect = Rect::from_center_size(kf_center, vec2(10.0, 10.0));
        let prm = ctx.e.param(spec.id);
        let at_key = prm.is_some_and(|pr| pr.keyframes.iter().any(|k| k.time == ctx.mt));
        let animated = prm.is_some_and(|pr| pr.is_animated());
        let kf_resp = ui.interact(kf_rect, egui::Id::unique(("lumetri-knob-kf", scope)), Sense::click()).on_hover_text(if at_key {
            tl!("Remove keyframe")
        } else {
            tl!("Add/Toggle keyframe")
        });
        let s = 3.2;
        let pts = vec![kf_center + vec2(0.0, -s), kf_center + vec2(s, 0.0), kf_center + vec2(0.0, s), kf_center + vec2(-s, 0.0)];
        if at_key {
            ui.painter().add(egui::Shape::convex_polygon(pts, t.hot_text, Stroke::NONE));
        } else {
            let stroke_col = if animated {
                t.accent
            } else if kf_resp.hovered() {
                t.text
            } else {
                t.text_faint.gamma_multiply(0.7)
            };
            ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.0, stroke_col)));
        }
        ctx.auto_rows.push((format!("lumetri.knob_keyframe.{}", spec.id), kf_rect, "Toggle keyframe"));
        if kf_resp.clicked() {
            ctx.mask_actions.push(("effects.addKeyframe".into(), json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": spec.id})));
        }
    }

    let delta = (new_val - cur_val).abs();
    if delta > 1e-5 {
        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": spec.id, "value": new_val, "merge": merge_undo}));
    }

    let formatted = format!("{:.*}", spec.decimals, cur_val);
    resp.on_hover_text(tlf!(
        "{label}: {val}{unit} (Drag or scroll; Shift = fine; double-click to reset)",
        label = spec.label,
        val = formatted,
        unit = spec.unit
    ));
    item_r
}

/// DaVinci Resolve-style primary knob bar.
pub(super) fn draw_primary_knobs_strip(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, include_wb_exp: bool, ctx: &mut ColorPanelCtx<'_>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let mut specs: Vec<(KnobSpec<'_>, &'static str, &'static str)> = Vec::new();
        if include_wb_exp {
            specs.push((
                KnobSpec {
                    id: "temperature",
                    label: tl!("Temperature"),
                    short_label: "TEMP",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(30, 125, 235),
                    track_pos: Color32::from_rgb(255, 125, 25),
                    active_neg: Color32::from_rgb(56, 189, 248),
                    active_pos: Color32::from_rgb(251, 146, 60),
                },
                "lumetri.knob.temperature",
                "Temperature",
            ));
            specs.push((
                KnobSpec {
                    id: "tint",
                    label: tl!("Tint"),
                    short_label: "TINT",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(40, 195, 75),
                    track_pos: Color32::from_rgb(230, 50, 185),
                    active_neg: Color32::from_rgb(74, 222, 128),
                    active_pos: Color32::from_rgb(244, 63, 94),
                },
                "lumetri.knob.tint",
                "Tint",
            ));
            specs.push((
                KnobSpec {
                    id: "exposure",
                    label: tl!("Exposure"),
                    short_label: "EXP",
                    range: (-5.0, 5.0),
                    default: 0.0,
                    speed: 0.02,
                    decimals: 1,
                    unit: " EV",
                    track_neg: Color32::from_rgb(70, 85, 115),
                    track_pos: Color32::from_rgb(240, 205, 50),
                    active_neg: Color32::from_rgb(120, 150, 190),
                    active_pos: Color32::from_rgb(250, 204, 21),
                },
                "lumetri.knob.exposure",
                "Exposure",
            ));
        }

        specs.extend([
            (
                KnobSpec {
                    id: "contrast",
                    label: tl!("Contrast"),
                    short_label: "CONT",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(85, 90, 105),
                    track_pos: Color32::from_rgb(170, 90, 240),
                    active_neg: Color32::from_rgb(130, 140, 160),
                    active_pos: Color32::from_rgb(192, 132, 252),
                },
                "lumetri.knob.contrast",
                "Contrast",
            ),
            (
                KnobSpec {
                    id: "contrast_pivot",
                    label: tl!("Pivot"),
                    short_label: "PIVOT",
                    range: (0.0, 100.0),
                    default: 50.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "%",
                    track_neg: Color32::from_rgb(75, 85, 105),
                    track_pos: Color32::from_rgb(155, 120, 225),
                    active_neg: Color32::from_rgb(120, 140, 175),
                    active_pos: Color32::from_rgb(192, 132, 252),
                },
                "lumetri.knob.contrast_pivot",
                "Pivot",
            ),
            (
                KnobSpec {
                    id: "midtone_detail",
                    label: tl!("Midtone Detail"),
                    short_label: "MD",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(75, 95, 120),
                    track_pos: Color32::from_rgb(225, 175, 75),
                    active_neg: Color32::from_rgb(110, 145, 185),
                    active_pos: Color32::from_rgb(245, 195, 90),
                },
                "lumetri.knob.midtone_detail",
                "Midtone Detail",
            ),
            (
                KnobSpec {
                    id: "color_boost",
                    label: tl!("Color Boost"),
                    short_label: "BOOST",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(80, 90, 105),
                    track_pos: Color32::from_rgb(65, 200, 165),
                    active_neg: Color32::from_rgb(110, 125, 145),
                    active_pos: Color32::from_rgb(52, 211, 153),
                },
                "lumetri.knob.color_boost",
                "Color Boost",
            ),
            (
                KnobSpec {
                    id: "primary_hue",
                    label: tl!("Hue"),
                    short_label: "HUE",
                    range: (0.0, 100.0),
                    default: 50.0,
                    speed: 0.25,
                    decimals: 1,
                    unit: "",
                    track_neg: Color32::from_rgb(215, 95, 135),
                    track_pos: Color32::from_rgb(95, 185, 235),
                    active_neg: Color32::from_rgb(244, 114, 182),
                    active_pos: Color32::from_rgb(56, 189, 248),
                },
                "lumetri.knob.primary_hue",
                "Hue",
            ),
            (
                KnobSpec {
                    id: "lum_mix",
                    label: tl!("Lum Mix"),
                    short_label: "LUM",
                    range: (0.0, 100.0),
                    default: 100.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "%",
                    track_neg: Color32::from_rgb(70, 80, 95),
                    track_pos: Color32::from_rgb(200, 210, 225),
                    active_neg: Color32::from_rgb(110, 125, 145),
                    active_pos: Color32::from_rgb(226, 232, 240),
                },
                "lumetri.knob.lum_mix",
                "Lum Mix",
            ),
            (
                KnobSpec {
                    id: "highlights",
                    label: tl!("Highlights"),
                    short_label: "HI",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(95, 100, 115),
                    track_pos: Color32::from_rgb(250, 240, 160),
                    active_neg: Color32::from_rgb(130, 135, 150),
                    active_pos: Color32::from_rgb(254, 240, 138),
                },
                "lumetri.knob.highlights",
                "Highlights",
            ),
            (
                KnobSpec {
                    id: "shadows",
                    label: tl!("Shadows"),
                    short_label: "SHAD",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(50, 70, 115),
                    track_pos: Color32::from_rgb(100, 165, 250),
                    active_neg: Color32::from_rgb(59, 130, 246),
                    active_pos: Color32::from_rgb(147, 197, 253),
                },
                "lumetri.knob.shadows",
                "Shadows",
            ),
            (
                KnobSpec {
                    id: "whites",
                    label: tl!("Whites"),
                    short_label: "WHITE",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(90, 100, 115),
                    track_pos: Color32::from_rgb(245, 250, 255),
                    active_neg: Color32::from_rgb(130, 140, 155),
                    active_pos: Color32::from_rgb(255, 255, 255),
                },
                "lumetri.knob.whites",
                "Whites",
            ),
            (
                KnobSpec {
                    id: "blacks",
                    label: tl!("Blacks"),
                    short_label: "BLACK",
                    range: (-100.0, 100.0),
                    default: 0.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "",
                    track_neg: Color32::from_rgb(30, 35, 45),
                    track_pos: Color32::from_rgb(130, 145, 165),
                    active_neg: Color32::from_rgb(71, 85, 105),
                    active_pos: Color32::from_rgb(148, 163, 184),
                },
                "lumetri.knob.blacks",
                "Blacks",
            ),
            (
                KnobSpec {
                    id: "saturation",
                    label: tl!("Saturation"),
                    short_label: "SAT",
                    range: (0.0, 200.0),
                    default: 100.0,
                    speed: 0.4,
                    decimals: 0,
                    unit: "%",
                    track_neg: Color32::from_rgb(90, 90, 90),
                    track_pos: Color32::from_rgb(50, 210, 120),
                    active_neg: Color32::from_rgb(100, 116, 139),
                    active_pos: Color32::from_rgb(52, 211, 153),
                },
                "lumetri.knob.saturation",
                "Saturation",
            ),
        ]);

        for (spec, auto_id, auto_lbl) in specs {
            let r = rotary_knob(ui, current_panel, spec, ctx);
            ctx.auto_rows.push((auto_id.into(), r, auto_lbl));
        }
    });
}
