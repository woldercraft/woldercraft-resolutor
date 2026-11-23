use filmcraft_project::ParamValue;
use serde_json::json;

use super::ColorPanelCtx;

const CST_GAMUTS: &[&str] = &[
    "Rec. 709",
    "Rec. 2020",
    "P3 D65",
    "Wide Gamut",
    "ACES AP1",
    "S-Gamut3.Cine",
    "S-Gamut3",
    "V-Gamut",
    "Cinema Gamut",
    "ARRI Wide Gamut 3",
    "ARRI Wide Gamut 4",
    "D-Gamut",
    "F-Gamut C",
];

const CST_GAMMAS: &[&str] = &[
    "Rec. 709 / Gamma 2.4",
    "sRGB",
    "Linear",
    "Rec. 2100 PQ",
    "Rec. 2100 HLG",
    "Intermediate Log",
    "ACEScct",
    "S-Log3",
    "V-Log",
    "Canon Log 2",
    "Canon Log 3",
    "ARRI LogC3",
    "ARRI LogC4",
    "Apple Log",
    "DJI D-Log",
    "F-Log",
    "F-Log2",
];

const CST_TONE_MAPS: &[&str] = &["None", "BT.2390", "Luminance"];
const CST_GAMUT_MAPS: &[&str] = &["None", "ACES Compression", "Desaturate"];

/// Color Space Transform (CST) panel: Input Color Space/Gamma → Output Color Space/Gamma + Tone/Gamut Mapping.
pub(super) fn show(ui: &mut egui::Ui, current_panel: crate::dock::PanelKind, ctx: &mut ColorPanelCtx<'_>) {
    let t = ctx.t;
    let ch = |id: &str| match ctx.e.param(id).map(|p| &p.value) {
        Some(ParamValue::Choice(c)) => *c as usize,
        _ => 0,
    };

    let in_sp = ch("cst_in_space");
    let in_gm = ch("cst_in_gamma");
    let out_sp = ch("cst_out_space");
    let out_gm = ch("cst_out_gamma");
    let tone_map = ch("cst_tone_map");
    let gamut_map = ch("cst_gamut_map");
    let peak_nits = ctx.e.param("cst_peak_nits").map_or(1000.0, |p| p.f64_at(ctx.mt));

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let swap = ui.small_button(tl!("Swap In↔Out")).on_hover_text(tl!("Swap Input and Output Color Space / Gamma"));
        ctx.auto_rows.push(("lumetri.cst.swap".into(), swap.rect, "Swap In↔Out"));
        if swap.clicked() {
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_on", "value": true, "merge": false}));
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_in_space", "value": out_sp, "merge": false}));
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_in_gamma", "value": out_gm, "merge": false}));
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_out_space", "value": in_sp, "merge": false}));
            ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_out_gamma", "value": in_gm, "merge": false}));
        }

        let rst = ui.small_button(tl!("Reset"));
        ctx.auto_rows.push(("lumetri.cst.reset".into(), rst.rect, "Reset CST"));
        if rst.clicked() {
            ctx.reset_sections.push("Color Space Transform");
        }
    });

    ui.add_space(4.0);

    if ui.available_width() >= 480.0 {
        ui.columns(2, |cols| {
            cols[0].vertical(|ui| {
                ui.label(egui::RichText::new(tl!("Input")).strong().small());
                cst_combo(ui, current_panel, "cst_in_space", "Input Color Space", tl!("Input Color Space"), CST_GAMUTS, in_sp, "in_sp", ctx);
                cst_combo(ui, current_panel, "cst_in_gamma", "Input Gamma", tl!("Input Gamma"), CST_GAMMAS, in_gm, "in_gm", ctx);
            });
            cols[1].vertical(|ui| {
                ui.label(egui::RichText::new(tl!("Output")).strong().small());
                cst_combo(ui, current_panel, "cst_out_space", "Output Color Space", tl!("Output Color Space"), CST_GAMUTS, out_sp, "out_sp", ctx);
                cst_combo(ui, current_panel, "cst_out_gamma", "Output Gamma", tl!("Output Gamma"), CST_GAMMAS, out_gm, "out_gm", ctx);
            });
        });
    } else {
        cst_combo(ui, current_panel, "cst_in_space", "Input Color Space", tl!("Input Color Space"), CST_GAMUTS, in_sp, "in_sp", ctx);
        cst_combo(ui, current_panel, "cst_in_gamma", "Input Gamma", tl!("Input Gamma"), CST_GAMMAS, in_gm, "in_gm", ctx);
        ui.add_space(2.0);
        cst_combo(ui, current_panel, "cst_out_space", "Output Color Space", tl!("Output Color Space"), CST_GAMUTS, out_sp, "out_sp", ctx);
        cst_combo(ui, current_panel, "cst_out_gamma", "Output Gamma", tl!("Output Gamma"), CST_GAMMAS, out_gm, "out_gm", ctx);
    }

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        cst_combo(ui, current_panel, "cst_tone_map", "Tone Mapping", tl!("Tone Mapping"), CST_TONE_MAPS, tone_map, "tm", ctx);
        cst_combo(ui, current_panel, "cst_gamut_map", "Gamut Mapping", tl!("Gamut Mapping"), CST_GAMUT_MAPS, gamut_map, "gm", ctx);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Peak Nits")).small().color(t.text_dim));
            let scope = (current_panel, ctx.is_group_mode, ctx.scope_id, ctx.idx, "cst_peak_nits");
            let (r, nv) = crate::widgets::hot_number(
                ui,
                "Peak Nits",
                egui::Id::unique(("cst-peak", scope)),
                peak_nits,
                crate::widgets::HotNumberOptions::new(10.0, (100.0, 10_000.0), 0, " cd/m²", t),
            );
            ctx.auto_rows.push(("lumetri.cst.cst_peak_nits".into(), r.rect, "Peak Nits"));
            if let Some(v) = nv {
                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_on", "value": true, "merge": false}));
                ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_peak_nits", "value": v, "merge": true}));
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn cst_combo(
    ui: &mut egui::Ui,
    current_panel: crate::dock::PanelKind,
    param: &str,
    auto_label: &'static str,
    ui_label: &str,
    opts: &[&str],
    cur: usize,
    salt: &str,
    ctx: &mut ColorPanelCtx<'_>,
) {
    let t = ctx.t;
    let cur_clamped = cur.min(opts.len().saturating_sub(1));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(ui_label).small().color(t.text_dim));
        let resp = craft_ui::ComboBox::from_id_salt(("cst", current_panel, ctx.is_secondary, salt, ctx.scope_id, ctx.idx))
            .selected_text(opts.get(cur_clamped).copied().unwrap_or("Rec. 709"))
            .width(170.0)
            .show_ui(ui, |ui| {
                for (i, &opt) in opts.iter().enumerate() {
                    if ui.selectable_label(i == cur_clamped, opt).clicked() {
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": "cst_on", "value": true, "merge": false}));
                        ctx.actions.push(json!({"clip": ctx.clip.0, "effect": ctx.idx, "param": param, "value": i, "merge": false}));
                    }
                }
            });
        ctx.auto_rows.push((format!("lumetri.cst.{param}"), resp.response.rect, auto_label));
    });
}
