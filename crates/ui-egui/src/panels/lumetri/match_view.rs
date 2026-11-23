use egui::{Rect, vec2};
use filmcraft_project::{EffectInstance, ParamValue};
use serde_json::{Value, json};

use super::sub;
use crate::FilmcraftApp;

/// LUT menu: None, built-ins, the project's LUT library, Browse… (file dialog → `lut.import`).
#[allow(clippy::too_many_arguments)]
pub(super) fn lut_combo(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    current_panel: crate::dock::PanelKind,
    is_secondary: bool,
    e: &EffectInstance,
    param: &str,
    label: &'static str,
    input: bool,
    out: &mut Vec<(&'static str, Value)>,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
) {
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
        let auto_id = if is_secondary { format!("lumetri.secondary.{param}") } else { format!("lumetri.{param}") };
        let (r, pick) = lut_menu(ui, app, ("lumetri-lut", current_panel, is_secondary, param), &cur, input, width, &auto_id);
        match pick {
            Some(LutPick::Lut(r)) => out.push((cmd, json!({"lut": r}))),
            Some(LutPick::Browse) => out.push((cmd, json!({"browse": true}))),
            None => {}
        }
        rect = r;
    });
    auto_rows.push((format!("lumetri.{param}"), rect, label));
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

/// Color Match: reference frame (sequence timecode), Face Detection (skin-tone protection),
/// Apply Match (`lumetri.applyMatch`).
pub(super) fn match_controls(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    current_panel: crate::dock::PanelKind,
    out: &mut Vec<(&'static str, Value)>,
    auto_rows: &mut Vec<(String, Rect, &'static str)>,
) {
    let t = app.tokens;
    let last_ref_key = egui::Id::unique(("lumetri-match-last-ref", current_panel));
    let last_ref: Option<i64> = ui.data(|d| d.get_temp(last_ref_key));
    let comparing = app.ui.program.display_mode() == Some(crate::state::DisplayMode::Comparison);

    ui.horizontal(|ui| {
        sub(ui, &t, tl!("Color Match"));
        ui.add_space(8.0);
        let b = ui.selectable_label(comparing, tl!("Comparison View"));
        auto_rows.push(("lumetri.match.comparisonView".into(), b.rect, "Comparison View"));
        if b.clicked() {
            out.push(("view.display", json!({"display": if comparing { "composite" } else { "comparison" }})));
        }
    });

    let reference = app.ui.program.compare_ref.filter(|_| comparing).map(filmcraft_time::Tick);
    let ref_tick = reference.map(|r| r.0);
    if ref_tick != last_ref
        && let (Some(r), Some(q)) = (reference, app.session.active_sequence())
    {
        app.ui.lumetri_match_ref = filmcraft_time::format_time(r, q.settings.frame_rate, q.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48000);
    }

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Reference")).color(t.text_dim).small());
        let r = crate::widgets::accessible_name(
            ui.add_enabled(reference.is_none(), egui::TextEdit::singleline(&mut app.ui.lumetri_match_ref).desired_width(85.0)),
            "Reference timecode",
        );
        auto_rows.push(("lumetri.match.reference".into(), r.rect, "Reference timecode"));
        crate::widgets::select_all_on_focus(ui, &r, &app.ui.lumetri_match_ref);
        let b = ui.button(tl!("Apply Match"));
        auto_rows.push(("lumetri.match.apply".into(), b.rect, "Apply Match"));
        if b.clicked() {
            let face = app.ui.lumetri_match_face;
            let p = match reference {
                Some(r) => json!({"referenceTime": r.0, "faceDetection": face}),
                None => json!({"referenceTimecode": app.ui.lumetri_match_ref, "faceDetection": face}),
            };
            out.push(("lumetri.applyMatch", p));
        }
    });

    let r = ui.checkbox(&mut app.ui.lumetri_match_face, tl!("Face Detection (protect skin tones)"));
    auto_rows.push(("lumetri.match.faceDetection".into(), r.rect, "Face Detection"));

    ui.data_mut(|d| {
        if let Some(rt) = ref_tick {
            d.insert_temp(last_ref_key, rt);
        } else {
            d.remove::<i64>(last_ref_key);
        }
    });
}
