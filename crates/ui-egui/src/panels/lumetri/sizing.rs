use filmcraft_project::ParamValue;
use serde_json::json;

use super::ColorPanelCtx;

const BLANKINGS: &[&str] = &["Off", "1.85:1", "2.00:1", "2.35:1", "2.39:1", "2.40:1"];

/// Node / Input Sizing panel: Pan, Tilt, Zoom, Width, Height, Rotate, Flip H/V, and Cinema Blanking.
pub(super) fn show(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let fv = |id: &str, def: f64| ctx.e.param(id).map_or(def, |p| p.f64_at(ctx.mt));
    let bv = |id: &str| ctx.e.param(id).and_then(|p| p.value.as_bool()).unwrap_or(false);
    let blanking = match ctx.e.param("sizing_blanking").map(|p| &p.value) {
        Some(ParamValue::Choice(c)) => (*c as usize).min(BLANKINGS.len() - 1),
        _ => 0,
    };

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let mut fh = bv("sizing_flip_h");
        let r_fh = ui.checkbox(&mut fh, egui::RichText::new(tl!("Flip H")).small());
        ctx.auto_rows.push(("lumetri.sizing.flip_h".into(), r_fh.rect, "Flip Horizontal"));
        if r_fh.changed() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "sizing_flip_h", "value": fh, "merge": false}));
        }

        let mut fv_b = bv("sizing_flip_v");
        let r_fv = ui.checkbox(&mut fv_b, egui::RichText::new(tl!("Flip V")).small());
        ctx.auto_rows.push(("lumetri.sizing.flip_v".into(), r_fv.rect, "Flip Vertical"));
        if r_fv.changed() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "sizing_flip_v", "value": fv_b, "merge": false}));
        }

        ui.label(egui::RichText::new(tl!("Blanking")).small().color(t.text_dim));
        let bl_resp = craft_ui::ComboBox::from_id_salt(("sizing-blanking", current_panel, ctx.is_secondary, ctx.scope_id, ctx.idx))
            .selected_text(BLANKINGS[blanking])
            .width(88.0)
            .show_ui(ui, |ui| {
                for (i, &opt) in BLANKINGS.iter().enumerate() {
                    if ui.selectable_label(i == blanking, opt).clicked() {
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "sizing_blanking", "value": i, "merge": false}));
                    }
                }
            });
        ctx.auto_rows.push(("lumetri.sizing.blanking".into(), bl_resp.response.rect, "Blanking"));

        let rst = ui.small_button(tl!("Reset"));
        ctx.auto_rows.push(("lumetri.sizing.reset".into(), rst.rect, "Reset Sizing"));
        if rst.clicked() {
            ctx.reset_sections.push("Sizing");
        }
    });

    ui.add_space(4.0);

    let controls: &[(&str, &str, f64, (f64, f64), f64, &str)] = &[
        ("sizing_zoom", tl!("Zoom"), 100.0, (10.0, 400.0), 0.5, "%"),
        ("sizing_pan", tl!("Pan"), 0.0, (-100.0, 100.0), 0.25, "%"),
        ("sizing_tilt", tl!("Tilt"), 0.0, (-100.0, 100.0), 0.25, "%"),
        ("sizing_rotate", tl!("Rotate"), 0.0, (-180.0, 180.0), 0.25, "°"),
        ("sizing_width", tl!("Width"), 100.0, (10.0, 400.0), 0.5, "%"),
        ("sizing_height", tl!("Height"), 100.0, (10.0, 400.0), 0.5, "%"),
    ];

    let cols = if ui.available_width() >= 420.0 { 3 } else { 2 };
    for row in controls.chunks(cols) {
        ui.columns(row.len(), |cs| {
            for (ci, &(pid, lbl, def, range, step, unit)) in row.iter().enumerate() {
                let ui = &mut cs[ci];
                let val = fv(pid, def);
                let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, pid);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(lbl).small().color(t.text_dim));
                    let (r, nv) = crate::widgets::hot_number(
                        ui,
                        lbl,
                        egui::Id::unique(("sizing-num", scope)),
                        val,
                        crate::widgets::HotNumberOptions::new(step, range, 1, unit, t),
                    );
                    ctx.auto_rows.push((format!("lumetri.sizing.{pid}"), r.rect, lbl));
                    if let Some(v) = nv {
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": pid, "value": v, "merge": true}));
                    }
                });
            }
        });
        ui.add_space(2.0);
    }
}
