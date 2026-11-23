use egui::{Color32, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::ColorPanelCtx;
use super::wheels::paint_reset_icon_button;
use crate::theme::Tokens;

const CHANNELS: [(&str, &str, Color32, [(&str, &str, f64, Color32); 3]); 3] = [
    (
        "r",
        "Red Output",
        Color32::from_rgb(230, 88, 88),
        [
            ("mix_rr", "R", 100.0, Color32::from_rgb(230, 88, 88)),
            ("mix_rg", "G", 0.0, Color32::from_rgb(92, 204, 118)),
            ("mix_rb", "B", 0.0, Color32::from_rgb(92, 152, 240)),
        ],
    ),
    (
        "g",
        "Green Output",
        Color32::from_rgb(92, 204, 118),
        [
            ("mix_gr", "R", 0.0, Color32::from_rgb(230, 88, 88)),
            ("mix_gg", "G", 100.0, Color32::from_rgb(92, 204, 118)),
            ("mix_gb", "B", 0.0, Color32::from_rgb(92, 152, 240)),
        ],
    ),
    (
        "b",
        "Blue Output",
        Color32::from_rgb(92, 152, 240),
        [
            ("mix_br", "R", 0.0, Color32::from_rgb(230, 88, 88)),
            ("mix_bg", "G", 0.0, Color32::from_rgb(92, 204, 118)),
            ("mix_bb", "B", 100.0, Color32::from_rgb(92, 152, 240)),
        ],
    ),
];

/// DaVinci Resolve-style RGB Mixer: 3×3 channel mixing matrix with Monochrome, Preserve Luminance, and Swap R↔B.
pub(super) fn show(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let mono = ctx.e.param("mix_monochrome").and_then(|p| p.value.as_bool()).unwrap_or(false);
    let preserve = ctx.e.param("mix_preserve_lum").and_then(|p| p.value.as_bool()).unwrap_or(true);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let mut mono_v = mono;
        let mr = ui.checkbox(&mut mono_v, egui::RichText::new(tl!("Monochrome")).small());
        ctx.auto_rows.push(("lumetri.rgb_mixer.monochrome".into(), mr.rect, "Monochrome"));
        if mr.changed() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "mix_monochrome", "value": mono_v, "merge": false}));
        }

        let mut pres_v = preserve;
        let pr = ui.checkbox(&mut pres_v, egui::RichText::new(tl!("Preserve Luminance")).small());
        ctx.auto_rows.push(("lumetri.rgb_mixer.preserve_lum".into(), pr.rect, "Preserve Luminance"));
        if pr.changed() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "mix_preserve_lum", "value": pres_v, "merge": false}));
        }

        let swap = ui.small_button(tl!("Swap R↔B"));
        ctx.auto_rows.push(("lumetri.rgb_mixer.swap_rb".into(), swap.rect, "Swap R↔B"));
        if swap.clicked() {
            let rr = ctx.e.param("mix_rr").map_or(100.0, |p| p.f64_at(ctx.mt));
            let rg = ctx.e.param("mix_rg").map_or(0.0, |p| p.f64_at(ctx.mt));
            let rb = ctx.e.param("mix_rb").map_or(0.0, |p| p.f64_at(ctx.mt));
            let br = ctx.e.param("mix_br").map_or(0.0, |p| p.f64_at(ctx.mt));
            let bg = ctx.e.param("mix_bg").map_or(0.0, |p| p.f64_at(ctx.mt));
            let bb = ctx.e.param("mix_bb").map_or(100.0, |p| p.f64_at(ctx.mt));
            for (pid, val) in [("mix_rr", br), ("mix_rg", bg), ("mix_rb", bb), ("mix_br", rr), ("mix_bg", rg), ("mix_bb", rb)] {
                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": val, "merge": false}));
            }
        }

        let rst = ui.small_button(tl!("Reset"));
        ctx.auto_rows.push(("lumetri.rgb_mixer.reset".into(), rst.rect, "Reset RGB Mixer"));
        if rst.clicked() {
            ctx.reset_sections.push("RGB Mixer");
        }
    });

    ui.add_space(4.0);

    let avail = ui.available_width().max(140.0);
    let cols = if avail >= 480.0 { 3 } else { 1 };
    let gap = 6.0;
    let card_w = ((avail - gap * (cols - 1) as f32) / cols as f32).floor().max(120.0);

    for row in CHANNELS.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(ch_tag, title, accent, sliders) in row {
                let active_card = !mono || ch_tag == "r";
                ui.allocate_ui_with_layout(vec2(card_w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_max_width(card_w);
                    channel_card(ui, current_panel, ch_tag, title, accent, sliders, active_card, ctx, t);
                });
            }
        });
        ui.add_space(4.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn channel_card(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    ch_tag: &str,
    title: &'static str,
    accent: Color32,
    sliders: [(&'static str, &'static str, f64, Color32); 3],
    active: bool,
    ctx: &mut ColorPanelCtx<'_>,
    t: &Tokens,
) {
    let modified = sliders.iter().any(|&(pid, _, def, _)| {
        let v = ctx.e.param(pid).map_or(def, |p| p.f64_at(ctx.mt));
        (v - def).abs() > 0.05
    });

    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(23, 26, 32))
        .stroke(Stroke::new(
            1.0,
            if !active {
                Color32::from_rgb(32, 35, 42)
            } else if modified {
                accent.gamma_multiply(0.75)
            } else {
                Color32::from_rgb(38, 43, 52)
            },
        ))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(6, 4));

    let inner_w = (ui.available_width() - 12.0).max(96.0);
    frame.show(ui, |ui| {
        ui.set_max_width(inner_w);
        ui.horizontal(|ui| {
            let (dot_r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
            ui.painter().circle_filled(dot_r.center(), 4.0, if active { accent } else { t.text_faint });
            ui.label(egui::RichText::new(crate::i18n::t(title)).strong().size(10.5).color(if !active {
                t.text_faint
            } else if modified {
                Color32::WHITE
            } else {
                t.text
            }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let rst = paint_reset_icon_button(ui, if modified && active { accent } else { t.text_faint }, tl!("Reset channel"));
                ctx.auto_rows.push((format!("lumetri.rgb_mixer.{ch_tag}.reset"), rst.rect, "Reset channel"));
                if rst.clicked() {
                    for &(pid, _, def, _) in &sliders {
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": def, "merge": false}));
                    }
                }
            });
        });

        ui.add_space(3.0);
        ui.add_enabled_ui(active, |ui| {
            for &(pid, lbl, def, col) in &sliders {
                mix_slider_row(ui, current_panel, pid, lbl, def, col, ctx, t);
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn mix_slider_row(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    pid: &'static str,
    lbl: &'static str,
    def: f64,
    col: Color32,
    ctx: &mut ColorPanelCtx<'_>,
    t: &Tokens,
) {
    let val = ctx.e.param(pid).map_or(def, |p| p.f64_at(ctx.mt));
    let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, pid);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new(lbl).strong().size(9.5).color(col));
        let track_w = (ui.available_width() - 48.0).max(40.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(track_w, 14.0), Sense::click_and_drag());
        let p = ui.painter();
        let bg = egui::Rect::from_min_max(pos2(rect.min.x, rect.center().y - 3.0), pos2(rect.max.x, rect.center().y + 3.0));
        p.rect_filled(bg, 2.0, Color32::from_rgb(15, 17, 21));
        p.rect_stroke(bg, 2.0, Stroke::new(1.0, Color32::from_rgb(44, 50, 62)), StrokeKind::Inside);

        let to_x = |v: f64| rect.min.x + ((v + 200.0) / 400.0).clamp(0.0, 1.0) as f32 * rect.width();
        let zero_x = to_x(0.0);
        let cur_x = to_x(val);
        let fill = egui::Rect::from_min_max(pos2(zero_x.min(cur_x), bg.min.y + 1.0), pos2(zero_x.max(cur_x), bg.max.y - 1.0));
        p.rect_filled(fill, 1.5, col.gamma_multiply(0.7));
        p.line_segment([pos2(zero_x, bg.min.y), pos2(zero_x, bg.max.y)], Stroke::new(1.0, Color32::from_rgb(110, 118, 132)));
        p.circle_filled(pos2(cur_x, rect.center().y), 4.0, Color32::from_rgb(235, 240, 248));

        if resp.double_clicked() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": def, "merge": false}));
        } else if (resp.dragged() || resp.clicked())
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let frac = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
            let nv = (frac * 400.0 - 200.0).clamp(-200.0, 200.0);
            if (nv - val).abs() > 1e-3 {
                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": nv, "merge": resp.dragged()}));
            }
        }
        ctx.auto_rows.push((format!("lumetri.rgb_mixer.{pid}"), rect, pid));

        let (_, nv) = crate::widgets::hot_number(
            ui,
            pid,
            egui::Id::unique(("rgb-mix-num", scope)),
            val,
            crate::widgets::HotNumberOptions::new(0.5, (-200.0, 200.0), 0, "%", t),
        );
        if let Some(v) = nv {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": v, "merge": true}));
        }
    });
}
