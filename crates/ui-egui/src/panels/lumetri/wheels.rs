use egui::{Color32, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::ColorPanelCtx;

#[derive(Clone, Copy)]
struct WheelSpec {
    vec_param: &'static str,
    lum_param: &'static str,
    lum_max: f32,
    lum_decimals: usize,
    lum_label: &'static str,
    id_tag: &'static str,
    reset_tag: &'static str,
    title: &'static str,
    subtitle: &'static str,
}

const PRIMARIES_SPECS: [WheelSpec; 4] = [
    WheelSpec {
        vec_param: "wheel_shadows",
        lum_param: "wheel_shadows_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "lift",
        reset_tag: "lift",
        title: "Lift",
        subtitle: "Shadows",
    },
    WheelSpec {
        vec_param: "wheel_midtones",
        lum_param: "wheel_midtones_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "gamma",
        reset_tag: "gamma",
        title: "Gamma",
        subtitle: "Midtones",
    },
    WheelSpec {
        vec_param: "wheel_highlights",
        lum_param: "wheel_highlights_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "gain",
        reset_tag: "gain",
        title: "Gain",
        subtitle: "Highlights",
    },
    WheelSpec {
        vec_param: "wheel_offset",
        lum_param: "wheel_offset_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "offset",
        reset_tag: "offset",
        title: "Offset",
        subtitle: "Global",
    },
];

const LOG_SPECS: [WheelSpec; 4] = [
    WheelSpec {
        vec_param: "log_shadows",
        lum_param: "log_shadows_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "log_shadows",
        reset_tag: "log_shadows",
        title: "Shadows",
        subtitle: "Log",
    },
    WheelSpec {
        vec_param: "log_midtones",
        lum_param: "log_midtones_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "log_midtones",
        reset_tag: "log_midtones",
        title: "Midtones",
        subtitle: "Log",
    },
    WheelSpec {
        vec_param: "log_highlights",
        lum_param: "log_highlights_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "log_highlights",
        reset_tag: "log_highlights",
        title: "Highlights",
        subtitle: "Log",
    },
    WheelSpec {
        vec_param: "log_offset",
        lum_param: "log_offset_l",
        lum_max: 100.0,
        lum_decimals: 0,
        lum_label: "L",
        id_tag: "log_offset",
        reset_tag: "log_offset",
        title: "Offset",
        subtitle: "Log",
    },
];

const HDR_SPECS: [WheelSpec; 7] = [
    WheelSpec {
        vec_param: "hdr_w_black",
        lum_param: "hdr_w_black_exp",
        lum_max: 4.0,
        lum_decimals: 2,
        lum_label: "EV",
        id_tag: "hdr_black",
        reset_tag: "hdr_black",
        title: "Black",
        subtitle: "-4.5 EV",
    },
    WheelSpec {
        vec_param: "hdr_w_dark",
        lum_param: "hdr_w_dark_exp",
        lum_max: 4.0,
        lum_decimals: 2,
        lum_label: "EV",
        id_tag: "hdr_dark",
        reset_tag: "hdr_dark",
        title: "Dark",
        subtitle: "-2.5 EV",
    },
    WheelSpec {
        vec_param: "hdr_w_shadow",
        lum_param: "hdr_w_shadow_exp",
        lum_max: 4.0,
        lum_decimals: 2,
        lum_label: "EV",
        id_tag: "hdr_shadow",
        reset_tag: "hdr_shadow",
        title: "Shadow",
        subtitle: "-1.0 EV",
    },
    WheelSpec {
        vec_param: "hdr_w_light",
        lum_param: "hdr_w_light_exp",
        lum_max: 4.0,
        lum_decimals: 2,
        lum_label: "EV",
        id_tag: "hdr_light",
        reset_tag: "hdr_light",
        title: "Light",
        subtitle: "+0.5 EV",
    },
    WheelSpec {
        vec_param: "hdr_w_highlight",
        lum_param: "hdr_w_highlight_exp",
        lum_max: 4.0,
        lum_decimals: 2,
        lum_label: "EV",
        id_tag: "hdr_highlight",
        reset_tag: "hdr_highlight",
        title: "Highlight",
        subtitle: "+2.0 EV",
    },
    WheelSpec {
        vec_param: "hdr_w_specular",
        lum_param: "hdr_w_specular_exp",
        lum_max: 4.0,
        lum_decimals: 2,
        lum_label: "EV",
        id_tag: "hdr_specular",
        reset_tag: "hdr_specular",
        title: "Specular",
        subtitle: "+3.5 EV",
    },
    WheelSpec {
        vec_param: "hdr_w_global",
        lum_param: "hdr_w_global_exp",
        lum_max: 4.0,
        lum_decimals: 2,
        lum_label: "EV",
        id_tag: "hdr_global",
        reset_tag: "hdr_global",
        title: "Global",
        subtitle: "All",
    },
];

/// Inline 4-mode switcher (`Wheels | Bars | Log | HDR`) + `LGG`/`Legacy` toggle.
pub(super) fn draw_wheels_mode_bar(ui: &mut egui::Ui, wheels_mode: &mut u8, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    for (idx, lbl, auto_id) in [
        (0u8, tl!("Wheels"), "lumetri.wheels.mode.wheels"),
        (1u8, tl!("Bars"), "lumetri.wheels.mode.bars"),
        (2u8, tl!("Log"), "lumetri.wheels.mode.log"),
        (3u8, tl!("HDR"), "lumetri.wheels.mode.hdr"),
    ] {
        let active = *wheels_mode == idx;
        let r = ui.selectable_label(active, egui::RichText::new(lbl).size(10.0).strong());
        ctx.auto_rows.push((auto_id.to_string(), r.rect, lbl));
        if r.clicked() {
            *wheels_mode = idx;
        }
    }
    if *wheels_mode <= 1 {
        ui.separator();
        let gm = ctx.e.param("grade_model").map_or(1u32, |p| match p.value {
            filmcraft_project::ParamValue::Choice(c) => c,
            _ => 1,
        });
        let next_gm = if gm == 0 { 1u32 } else { 0u32 };
        let gm_lbl = if gm == 0 { "LGG" } else { "Legacy" };
        let r = ui
            .selectable_label(gm == 0, egui::RichText::new(gm_lbl).size(9.5).color(if gm == 0 { t.accent } else { t.text_dim }))
            .on_hover_text(tl!("Switch between Lift/Gamma/Gain math and Legacy wheel math"));
        ctx.auto_rows.push(("lumetri.wheels.grade_model".to_string(), r.rect, "Grade Model"));
        if r.clicked() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "grade_model", "value": next_gm, "merge": false}));
        }
    }
}

/// DaVinci Resolve 4-mode wheel deck: Primaries Wheels, Primaries Bars, Log Wheels, and HDR Wheels.
pub(super) fn resolve_wheels_grid(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    wheels_mode: &mut u8,
    show_mode_bar: bool,
    ctx: &mut ColorPanelCtx<'_>,
) {
    if show_mode_bar {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            draw_wheels_mode_bar(ui, wheels_mode, ctx);
        });
        ui.add_space(2.0);
    }

    match *wheels_mode {
        1 => draw_primaries_bars(ui, current_panel, ctx),
        2 => {
            draw_wheel_specs_grid(ui, current_panel, &LOG_SPECS, ctx);
            ui.add_space(3.0);
            draw_compact_param_strip(
                ui,
                current_panel,
                &[("log_low_range", tl!("Low Range"), 33.0, (0.0, 100.0), 1, ""), ("log_high_range", tl!("High Range"), 55.0, (0.0, 100.0), 1, "")],
                ctx,
            );
        }
        3 => {
            draw_compact_param_strip(
                ui,
                current_panel,
                &[("hdr_zone_falloff", tl!("Falloff"), 50.0, (0.0, 100.0), 0, "%"), ("hdr_wheel_range", tl!("Range"), 0.0, (-4.0, 4.0), 2, "EV")],
                ctx,
            );
            ui.add_space(2.0);
            draw_wheel_specs_grid(ui, current_panel, &HDR_SPECS, ctx);
        }
        _ => draw_wheel_specs_grid(ui, current_panel, &PRIMARIES_SPECS, ctx),
    }
}

fn draw_compact_param_strip(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    params: &[(&'static str, &'static str, f64, (f64, f64), usize, &'static str)],
    ctx: &mut ColorPanelCtx<'_>,
) {
    let t = ctx.t;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for &(pid, lbl, def, range, decs, unit) in params {
            let val = ctx.e.param(pid).map_or(def, |p| p.f64_at(ctx.mt));
            let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, pid);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(egui::RichText::new(lbl).size(9.5).color(t.text_dim));
                let step = if decs >= 2 { 0.02 } else { 0.5 };
                let (r, nv) = crate::widgets::hot_number(
                    ui,
                    lbl,
                    egui::Id::unique(("wheel-extra", scope)),
                    val,
                    crate::widgets::HotNumberOptions::new(step, range, decs, unit, t),
                );
                ctx.auto_rows.push((format!("lumetri.param.{pid}"), r.rect, lbl));
                if let Some(v) = nv {
                    ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": v, "merge": true}));
                }
            });
        }
    });
}

fn draw_wheel_specs_grid(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, specs: &[WheelSpec], ctx: &mut ColorPanelCtx<'_>) {
    let avail = ui.available_width().max(140.0);
    let gap = ui.spacing().item_spacing.x;
    let cols = if avail >= 640.0 {
        4.min(specs.len())
    } else if avail >= 320.0 {
        2.min(specs.len())
    } else {
        1
    };
    let col_w = ((avail - gap * (cols.saturating_sub(1)) as f32) / cols.max(1) as f32).floor().max(80.0);
    for row in specs.chunks(cols.max(1)) {
        ui.horizontal_top(|ui| {
            for &spec in row {
                ui.allocate_ui_with_layout(vec2(col_w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(col_w);
                    resolve_wheel_card(ui, current_panel, spec, ctx);
                });
            }
        });
        if specs.len() > cols {
            ui.add_space(3.0);
        }
    }
}

fn wheel_vec_to_rgb(vx: f32, vy: f32) -> [f32; 3] {
    let s = 0.866_025_4_f32; // sqrt(3) / 2
    [vx, -0.5 * vx + s * vy, -0.5 * vx - s * vy]
}

fn rgb_to_wheel_vec(rgb: [f32; 3]) -> (f32, f32) {
    let vx = (2.0 * rgb[0] - rgb[1] - rgb[2]) / 3.0;
    let vy = (rgb[1] - rgb[2]) / 1.732_050_8_f32;
    let v = vec2(vx, vy);
    if v.length() > 1.0 {
        let n = v.normalized();
        (n.x, n.y)
    } else {
        (vx, vy)
    }
}

fn draw_primaries_bars(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let avail = ui.available_width().max(140.0);
    let gap = ui.spacing().item_spacing.x;
    let cols = if avail >= 640.0 {
        4
    } else if avail >= 320.0 {
        2
    } else {
        1
    };
    let col_w = ((avail - gap * (cols - 1) as f32) / cols as f32).floor().max(80.0);
    for row in PRIMARIES_SPECS.chunks(cols) {
        ui.horizontal_top(|ui| {
            for &spec in row {
                ui.allocate_ui_with_layout(vec2(col_w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(col_w);
                    draw_bar_card(ui, current_panel, spec, ctx);
                });
            }
        });
        if cols < 4 {
            ui.add_space(3.0);
        }
    }
}

fn draw_bar_card(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, spec: WheelSpec, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let v = ctx.e.param(spec.vec_param).map(|p| p.vec2_at(ctx.mt)).unwrap_or_default();
    let (vx, vy) = (v.x as f32, v.y as f32);
    let lum = ctx.e.param(spec.lum_param).map(|p| p.f64_at(ctx.mt)).unwrap_or(0.0) as f32;
    let rgb = wheel_vec_to_rgb(vx, vy);
    let section_on = ctx.e.param("wheels_on").and_then(|p| p.value.as_bool()).unwrap_or(true);
    let modified = vx.abs() > 1e-3 || vy.abs() > 1e-3 || lum.abs() > 1e-3;

    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(23, 26, 32))
        .stroke(Stroke::new(
            1.0,
            if !section_on {
                Color32::from_rgb(32, 35, 42)
            } else if modified {
                Color32::from_rgb(65, 85, 118)
            } else {
                Color32::from_rgb(38, 43, 52)
            },
        ))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(5, 3));

    let inner_max = (ui.available_width() - 10.0).max(60.0);
    frame.show(ui, |ui| {
        ui.set_max_width(inner_max);
        ui.allocate_ui_with_layout(vec2(inner_max, 16.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let rst = paint_reset_icon_button(ui, if modified { t.accent } else { t.text_faint }, tl!("Reset this wheel (or double-click)"));
            if rst.clicked() {
                ctx.reset_wheels.push(spec.reset_tag);
            }
            let title_w = ui.available_width().max(24.0);
            ui.allocate_ui_with_layout(vec2(title_w, 16.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(spec.title).strong().size(11.0).color(if modified { Color32::WHITE } else { t.text }));
                ui.label(egui::RichText::new(format!("({})", crate::i18n::t(spec.subtitle))).size(9.0).color(t.text_dim));
            });
        });

        ui.add_space(3.0);
        let bar_h = 84.0;
        let bar_gap = 4.0;
        let bar_w = ((inner_max - bar_gap * 3.0) / 4.0).floor().max(14.0);
        let channels = [
            ("r", "R", rgb[0], Color32::from_rgb(230, 88, 88), Some(0usize)),
            ("g", "G", rgb[1], Color32::from_rgb(92, 204, 118), Some(1usize)),
            ("b", "B", rgb[2], Color32::from_rgb(92, 152, 240), Some(2usize)),
            ("y", "Y", lum / spec.lum_max, Color32::from_rgb(228, 232, 240), None),
        ];
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = bar_gap;
            for (ch_id, lbl, norm_val, col, rgb_idx) in channels {
                ui.vertical(|ui| {
                    ui.set_max_width(bar_w);
                    let (rect, resp) = ui.allocate_exact_size(vec2(bar_w, bar_h), Sense::click_and_drag());
                    let p = ui.painter();
                    p.rect_filled(rect, 3.0, Color32::from_rgb(15, 17, 21));
                    p.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_rgb(44, 50, 62)), StrokeKind::Inside);
                    let mid_y = rect.center().y;
                    p.line_segment([pos2(rect.min.x + 2.0, mid_y), pos2(rect.max.x - 2.0, mid_y)], Stroke::new(1.0, Color32::from_rgb(76, 84, 98)));
                    let clamped = norm_val.clamp(-1.0, 1.0);
                    let val_y = mid_y - clamped * (bar_h * 0.5 - 4.0);
                    let fill = egui::Rect::from_min_max(pos2(rect.min.x + 3.0, val_y.min(mid_y)), pos2(rect.max.x - 3.0, val_y.max(mid_y)));
                    p.rect_filled(fill, 1.5, col.gamma_multiply(0.75));
                    p.rect_filled(egui::Rect::from_center_size(pos2(rect.center().x, val_y), vec2(bar_w - 4.0, 3.0)), 1.0, Color32::WHITE);

                    if resp.double_clicked() {
                        if let Some(ci) = rgb_idx {
                            let mut next = rgb;
                            next[ci] = 0.0;
                            let (nx, ny) = rgb_to_wheel_vec(next);
                            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": spec.vec_param, "value": [nx, ny], "merge": false}));
                        } else {
                            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": spec.lum_param, "value": 0.0, "merge": false}));
                        }
                    } else if (resp.dragged() || resp.clicked())
                        && let Some(pos) = resp.interact_pointer_pos()
                    {
                        let n = ((mid_y - pos.y) / (bar_h * 0.5 - 4.0).max(1.0)).clamp(-1.0, 1.0);
                        if let Some(ci) = rgb_idx {
                            let mut next = rgb;
                            next[ci] = n;
                            let (nx, ny) = rgb_to_wheel_vec(next);
                            if (nx - vx).abs() > 1e-4 || (ny - vy).abs() > 1e-4 {
                                ctx.actions
                                    .push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": spec.vec_param, "value": [nx, ny], "merge": resp.dragged()}));
                            }
                        } else {
                            let nv = n * spec.lum_max;
                            if (nv - lum).abs() > 1e-3 {
                                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": spec.lum_param, "value": nv, "merge": resp.dragged()}));
                            }
                        }
                    }
                    let _ = current_panel;
                    ctx.auto_rows.push((format!("lumetri.bars.{}.{ch_id}", spec.id_tag), rect, lbl));
                    ui.label(egui::RichText::new(lbl).strong().size(9.0).color(col));
                });
            }
        });
    });
}

/// Vector-drawn circular reset arrow button (avoids selectable text glyph / I-beam cursor).
pub(super) fn paint_reset_icon_button(ui: &mut egui::Ui, color: Color32, tooltip: &str) -> egui::Response {
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
        let tip = pos2(c.x + r * start.cos(), c.y + r * start.sin());
        p.line_segment([tip, tip + vec2(3.2, -0.4)], Stroke::new(1.35, col));
        p.line_segment([tip, tip + vec2(0.4, 3.2)], Stroke::new(1.35, col));
    }
    resp
}

/// Single DaVinci Resolve color wheel card: Header + Reset, Hue Trackball, Horizontal Knurled Master Wheel, and Live Readout.
fn resolve_wheel_card(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, spec: WheelSpec, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let e = ctx.e;
    let mt = ctx.mt;
    let clip = ctx.clip;
    let idx = ctx.idx;
    let v = e.param(spec.vec_param).map(|p| p.vec2_at(mt)).unwrap_or_default();
    let (vx, vy) = (v.x as f32, v.y as f32);
    let lum = e.param(spec.lum_param).map(|p| p.f64_at(mt)).unwrap_or(0.0) as f32;
    let lum_max = spec.lum_max;

    let section_on = e.param("wheels_on").and_then(|p| p.value.as_bool()).unwrap_or(true);
    let modified = vx.abs() > 1e-3 || vy.abs() > 1e-3 || lum.abs() > 1e-3;

    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(23, 26, 32))
        .stroke(Stroke::new(
            1.0,
            if !section_on {
                Color32::from_rgb(32, 35, 42)
            } else if modified {
                Color32::from_rgb(65, 85, 118)
            } else {
                Color32::from_rgb(38, 43, 52)
            },
        ))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(4, 2));

    let inner_max = (ui.available_width() - 10.0).max(60.0);
    let subtitle = crate::i18n::t(spec.subtitle);
    let frame_resp = frame.show(ui, |ui| {
        ui.set_max_width(inner_max);
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.allocate_ui_with_layout(vec2(inner_max, 16.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_max_width(inner_max);
            let rst_col = if modified { t.accent } else { t.text_faint };
            let rst = paint_reset_icon_button(ui, rst_col, tl!("Reset this wheel (or double-click)"));
            if rst.clicked() {
                ctx.reset_wheels.push(spec.reset_tag);
            }
            let title_w = ui.available_width().max(24.0);
            ui.allocate_ui_with_layout(vec2(title_w, 16.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_max_width(title_w);
                let title_col = if !section_on {
                    t.text_faint
                } else if modified {
                    Color32::WHITE
                } else {
                    t.text
                };
                ui.label(egui::RichText::new(spec.title).strong().size(11.0).color(title_col));
                let sub_txt = if section_on { format!("({subtitle})") } else { format!("({subtitle} · off)") };
                ui.add(egui::Label::new(egui::RichText::new(sub_txt).size(9.0).color(if section_on { t.text_dim } else { t.text_faint })).truncate());
            });
        });

        ui.add_space(1.0);

        let wheel_size = (ui.available_width() * 0.72).clamp(48.0, 120.0);
        let row_w = ui.available_width();
        let (area, resp) = ui.allocate_exact_size(vec2(row_w, wheel_size), Sense::click_and_drag());
        let c = area.center();
        let r_out = wheel_size / 2.0 - 2.0;
        let r_in = r_out - (wheel_size * 0.14).clamp(7.0, 12.0);
        let p = ui.painter();

        p.circle_stroke(c, r_out + 1.0, Stroke::new(1.5, Color32::from_rgb(14, 16, 20)));

        let ring = color_ring_lut();
        let mut mesh = egui::Mesh::default();
        for win in ring.windows(2) {
            let (c0, s0, col) = win[0];
            let (c1, s1, _) = win[1];
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(c + vec2(c0, -s0) * r_in, col);
            mesh.colored_vertex(c + vec2(c0, -s0) * r_out, col);
            mesh.colored_vertex(c + vec2(c1, -s1) * r_out, col);
            mesh.colored_vertex(c + vec2(c1, -s1) * r_in, col);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        p.add(mesh);

        let dish_center_col = Color32::from_rgb(16, 18, 22);
        p.circle_filled(c, r_in - 1.0, dish_center_col);
        let mut dish_mesh = egui::Mesh::default();
        let center_idx = dish_mesh.vertices.len() as u32;
        dish_mesh.colored_vertex(c, dish_center_col);
        for (cos, sin, col) in ring {
            let rim = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 26);
            dish_mesh.colored_vertex(c + vec2(*cos, -*sin) * (r_in - 1.5), rim);
        }
        for i in 0..ring.len().saturating_sub(1) as u32 {
            dish_mesh.add_triangle(center_idx, center_idx + 1 + i, center_idx + 2 + i);
        }
        p.add(dish_mesh);
        p.circle_stroke(c, (r_in - 2.0) * 0.5, Stroke::new(0.8, Color32::from_rgb(32, 36, 44)));
        let ch_r = r_in - 5.0;
        p.line_segment([c - vec2(ch_r, 0.0), c + vec2(ch_r, 0.0)], Stroke::new(0.8, Color32::from_rgb(42, 47, 56)));
        p.line_segment([c - vec2(0.0, ch_r), c + vec2(0.0, ch_r)], Stroke::new(0.8, Color32::from_rgb(42, 47, 56)));
        p.circle_filled(c, 1.5, Color32::from_rgb(75, 82, 95));

        let dish_r = (r_in - 5.0).max(12.0);
        let puck_vec = vec2(vx, -vy);
        let puck_clamped = if puck_vec.length() > 1.0 { puck_vec.normalized() } else { puck_vec };
        let puck = c + puck_clamped * dish_r;
        if vx.abs() > 1e-3 || vy.abs() > 1e-3 {
            p.line_segment([c, puck], Stroke::new(1.0, Color32::from_rgba_unmultiplied(220, 225, 235, 110)));
        }

        let puck_mag = (vx * vx + vy * vy).sqrt().clamp(0.0, 1.0);
        let puck_hue = ((-vy).atan2(vx).rem_euclid(std::f32::consts::TAU)) / std::f32::consts::TAU;
        let prgb = filmcraft_color::hsl_to_rgb(1.0 - puck_hue, puck_mag * 0.85, 0.55);
        let puck_fill = if puck_mag > 0.02 {
            Color32::from_rgb((prgb[0].clamp(0.0, 1.0) * 255.0) as u8, (prgb[1].clamp(0.0, 1.0) * 255.0) as u8, (prgb[2].clamp(0.0, 1.0) * 255.0) as u8)
        } else {
            Color32::from_rgb(55, 60, 70)
        };
        p.circle_filled(puck, 4.5, puck_fill);
        p.circle_stroke(puck, 5.0, Stroke::new(1.5, Color32::WHITE));

        let emit_2d = |actions: &mut Vec<serde_json::Value>, nx: f32, ny: f32, merge: bool| {
            actions.push(json!({"clip": clip.0, "effect": idx, "param": spec.vec_param, "value": [nx, ny], "merge": merge}));
        };

        if resp.double_clicked() {
            emit_2d(ctx.actions, 0.0, 0.0, false);
        } else if resp.dragged() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            if shift {
                let d = resp.drag_delta() * 0.25 / dish_r;
                let mut cur = vec2(vx + d.x, vy - d.y);
                if cur.length() > 1.0 {
                    cur = cur.normalized();
                }
                if (cur.x - vx).abs() > 1e-4 || (cur.y - vy).abs() > 1e-4 {
                    emit_2d(ctx.actions, cur.x, cur.y, true);
                }
            } else if let Some(pos) = resp.interact_pointer_pos() {
                let mut d = (pos - c) / dish_r;
                if d.length() > 1.0 {
                    d = d.normalized();
                }
                let nx = d.x;
                let ny = -d.y;
                if (nx - vx).abs() > 1e-4 || (ny - vy).abs() > 1e-4 {
                    emit_2d(ctx.actions, nx, ny, true);
                }
            }
        } else if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let mut d = (pos - c) / dish_r;
            if d.length() > 1.0 {
                d = d.normalized();
            }
            emit_2d(ctx.actions, d.x, -d.y, false);
        }

        ui.add_space(1.0);

        let (track, lr) = ui.allocate_exact_size(vec2(row_w, 11.0), Sense::click_and_drag());
        let lp = ui.painter();
        lp.rect_filled(track, 3.0, Color32::from_rgb(15, 17, 21));
        lp.rect_stroke(track, 3.0, Stroke::new(1.0, Color32::from_rgb(44, 50, 62)), StrokeKind::Inside);

        let ticks = 16;
        for i in 1..ticks {
            let tx = track.min.x + track.width() * (i as f32 / ticks as f32);
            let is_mid = i == ticks / 2;
            lp.line_segment(
                [pos2(tx, track.min.y + if is_mid { 2.0 } else { 4.0 }), pos2(tx, track.max.y - if is_mid { 2.0 } else { 4.0 })],
                Stroke::new(1.0, if is_mid { Color32::from_rgb(90, 98, 114) } else { Color32::from_rgb(36, 40, 50) }),
            );
        }

        let frac = (lum / lum_max).clamp(-1.0, 1.0);
        let cx = track.center().x;
        let kx = cx + frac * (track.width() * 0.5 - 4.0);
        if frac.abs() > 0.005 {
            let fill_r = egui::Rect::from_min_max(pos2(cx.min(kx), track.min.y + 3.0), pos2(cx.max(kx), track.max.y - 3.0));
            let fill_col = if frac >= 0.0 { Color32::from_rgba_unmultiplied(235, 185, 70, 120) } else { Color32::from_rgba_unmultiplied(80, 150, 235, 120) };
            lp.rect_filled(fill_r, 1.5, fill_col);
        }
        lp.rect_filled(egui::Rect::from_center_size(pos2(kx, track.center().y), vec2(4.0, track.height() - 4.0)), 1.5, Color32::from_rgb(235, 240, 248));

        let emit_lum = |actions: &mut Vec<serde_json::Value>, nv: f32, merge: bool| {
            actions.push(json!({"clip": clip.0, "effect": idx, "param": spec.lum_param, "value": nv, "merge": merge}));
        };

        if lr.double_clicked() {
            emit_lum(ctx.actions, 0.0, false);
        } else if lr.dragged() {
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            let mult = if shift { 0.25 } else { 1.0 };
            let step = (lr.drag_delta().x / (track.width() * 0.5).max(1.0)) * lum_max * mult;
            let nv = (lum + step).clamp(-lum_max, lum_max);
            if (nv - lum).abs() > 1e-3 {
                emit_lum(ctx.actions, nv, true);
            }
        } else if lr.clicked()
            && let Some(pos) = lr.interact_pointer_pos()
        {
            let nv = ((pos.x - cx) / (track.width() * 0.5 - 4.0).max(1.0) * lum_max).clamp(-lum_max, lum_max);
            emit_lum(ctx.actions, nv, false);
        }
        lr.on_hover_text(tl!("Drag horizontally to adjust luminance (Shift = fine, Double-click = reset)"));

        // Interactive numeric readout below Master Wheel (scrub or click to type)
        let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, idx, spec.id_tag);
        ui.allocate_ui_with_layout(vec2(row_w, 15.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let micro = |ui: &mut egui::Ui, lbl: &str| {
                ui.label(egui::RichText::new(lbl).size(8.5).color(t.text_faint));
            };
            let clamp_disk = |x: f32, y: f32| {
                let v = vec2(x, y);
                if v.length() > 1.0 {
                    let n = v.normalized();
                    (n.x, n.y)
                } else {
                    (x, y)
                }
            };
            micro(ui, "X");
            let (_, nx) = crate::widgets::hot_number(
                ui,
                "X",
                egui::Id::unique(("wheel-num-x", scope)),
                vx as f64,
                crate::widgets::HotNumberOptions::new(0.01, (-1.0, 1.0), 2, "", t),
            );
            if let Some(v) = nx {
                let (cx, cy) = clamp_disk(v as f32, vy);
                emit_2d(ctx.actions, cx, cy, true);
            }
            micro(ui, "Y");
            let (_, ny) = crate::widgets::hot_number(
                ui,
                "Y",
                egui::Id::unique(("wheel-num-y", scope)),
                vy as f64,
                crate::widgets::HotNumberOptions::new(0.01, (-1.0, 1.0), 2, "", t),
            );
            if let Some(v) = ny {
                let (cx, cy) = clamp_disk(vx, v as f32);
                emit_2d(ctx.actions, cx, cy, true);
            }
            micro(ui, spec.lum_label);
            let lum_step = if spec.lum_decimals >= 2 { 0.02 } else { 0.5 };
            let (_, nl) = crate::widgets::hot_number(
                ui,
                spec.lum_label,
                egui::Id::unique(("wheel-num-l", scope)),
                lum as f64,
                crate::widgets::HotNumberOptions::new(lum_step, (-lum_max as f64, lum_max as f64), spec.lum_decimals, "", t),
            );
            if let Some(v) = nl {
                emit_lum(ctx.actions, v as f32, true);
            }
        });
    });
    ctx.auto_rows.push((format!("lumetri.wheel.{}", spec.id_tag), frame_resp.response.rect, spec.title));
}

fn color_ring_lut() -> &'static [(f32, f32, Color32)] {
    use std::sync::OnceLock;
    static LUT: OnceLock<Vec<(f32, f32, Color32)>> = OnceLock::new();
    LUT.get_or_init(|| {
        let n = 72usize;
        (0..=n)
            .map(|i| {
                let a = (i % n) as f32 / n as f32 * std::f32::consts::TAU;
                let rgb = filmcraft_color::hsl_to_rgb(1.0 - ((i % n) as f32 / n as f32), 0.78, 0.52);
                let col =
                    Color32::from_rgb((rgb[0].clamp(0.0, 1.0) * 255.0) as u8, (rgb[1].clamp(0.0, 1.0) * 255.0) as u8, (rgb[2].clamp(0.0, 1.0) * 255.0) as u8);
                (a.cos(), a.sin(), col)
            })
            .collect()
    })
}
