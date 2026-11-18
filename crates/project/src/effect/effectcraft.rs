//! Adapter exposing the EffectCraft VFX catalogue (`filmcraft_effects::registry()`) as
//! [`EffectDef`] entries in `filmcraft_project`.

use filmcraft_effects::effectcraft_keyframe::Value;
use filmcraft_effects::effectcraft_project::ParamUi;
use filmcraft_geom::Vec2;

use super::{EffectDef, EffectKind, ParamDef, ParamKind, vfx::TRACK_CHOICES};
use crate::keyframe::ParamValue;

/// Top-level folder key in the Effects panel for the EffectCraft VFX engine.
pub const EFFECTCRAFT_TOP_FOLDER: &str = "EffectCraft VFX";

const MASK_CHOICES: &[&str] = &["None", "Mask 1", "Mask 2", "Mask 3", "Mask 4"];

fn category_slice(cat: &'static str) -> &'static [&'static str] {
    match cat {
        "3D Channel" => &[EFFECTCRAFT_TOP_FOLDER, "3D Channel"],
        "Blur & Sharpen" => &[EFFECTCRAFT_TOP_FOLDER, "Blur & Sharpen"],
        "Channel" => &[EFFECTCRAFT_TOP_FOLDER, "Channel"],
        "Color Correction" => &[EFFECTCRAFT_TOP_FOLDER, "Color Correction"],
        "Distort" => &[EFFECTCRAFT_TOP_FOLDER, "Distort"],
        "Generate" => &[EFFECTCRAFT_TOP_FOLDER, "Generate"],
        "Immersive Video" => &[EFFECTCRAFT_TOP_FOLDER, "Immersive Video"],
        "Keying" => &[EFFECTCRAFT_TOP_FOLDER, "Keying"],
        "Matte" => &[EFFECTCRAFT_TOP_FOLDER, "Matte"],
        "Noise & Grain" => &[EFFECTCRAFT_TOP_FOLDER, "Noise & Grain"],
        "Obsolete" => &[EFFECTCRAFT_TOP_FOLDER, "Obsolete"],
        "Paint" => &[EFFECTCRAFT_TOP_FOLDER, "Paint"],
        "Perspective" => &[EFFECTCRAFT_TOP_FOLDER, "Perspective"],
        "Simulation" => &[EFFECTCRAFT_TOP_FOLDER, "Simulation"],
        "Stylize" => &[EFFECTCRAFT_TOP_FOLDER, "Stylize"],
        "Text" => &[EFFECTCRAFT_TOP_FOLDER, "Text"],
        "Time" => &[EFFECTCRAFT_TOP_FOLDER, "Time"],
        "Transition" => &[EFFECTCRAFT_TOP_FOLDER, "Transition"],
        "Utility" => &[EFFECTCRAFT_TOP_FOLDER, "Utility"],
        _ => &[EFFECTCRAFT_TOP_FOLDER, "Utility"],
    }
}

/// Convert `filmcraft_effects::registry()` into `filmcraft_project::EffectDef` definitions.
/// Called once inside the `OnceLock` initialization of [`super::effect_defs`].
pub fn defs() -> Vec<EffectDef> {
    let mut out = Vec::new();
    for spec in filmcraft_effects::registry() {
        // Audio and Expression Controls are non-visual; Lumetri is already provided natively.
        if matches!(spec.category, "Audio" | "Expression Controls") || spec.id == "ec.color.lumetri" {
            continue;
        }
        let mut params = Vec::new();
        for ps in &spec.params {
            let group = ps.id.split_once('/').map(|(g, _)| g);
            let (kind, default, animatable) = match &ps.ui {
                ParamUi::Slider { min, max, slider_min, slider_max, decimals } => (
                    ParamKind::Float { min: *min, max: *max, soft_min: *slider_min, soft_max: *slider_max, unit: "", decimals: *decimals },
                    ParamValue::Float(ps.default.as_f64()),
                    true,
                ),
                ParamUi::Percent => (
                    ParamKind::Float { min: 0.0, max: 100.0, soft_min: 0.0, soft_max: 100.0, unit: "%", decimals: 1 },
                    ParamValue::Float(ps.default.as_f64()),
                    true,
                ),
                ParamUi::Pixels => (
                    ParamKind::Float { min: -10000.0, max: 10000.0, soft_min: 0.0, soft_max: 500.0, unit: "px", decimals: 1 },
                    ParamValue::Float(ps.default.as_f64()),
                    true,
                ),
                ParamUi::Number => (
                    ParamKind::Float { min: -10000.0, max: 10000.0, soft_min: -100.0, soft_max: 100.0, unit: "", decimals: 1 },
                    ParamValue::Float(ps.default.as_f64()),
                    true,
                ),
                ParamUi::Angle => (ParamKind::Angle, ParamValue::Float(ps.default.as_f64()), true),
                ParamUi::Point | ParamUi::Point3 => (ParamKind::Point, ParamValue::Vec2(Vec2::new(f64::NAN, f64::NAN)), true),
                ParamUi::Color => (ParamKind::Color, ParamValue::Color(ps.default.as_color()), true),
                ParamUi::Checkbox => (ParamKind::Bool, ParamValue::Bool(ps.default.as_bool()), false),
                ParamUi::Popup { options } => {
                    let leaked: Vec<&'static str> = options.iter().map(|s| -> &'static str { Box::leak(s.clone().into_boxed_str()) }).collect();
                    let slice: &'static [&'static str] = Box::leak(leaked.into_boxed_slice());
                    (ParamKind::Choice(slice), ParamValue::Choice(ps.default.as_enum()), false)
                }
                ParamUi::Layer => (ParamKind::Choice(TRACK_CHOICES), ParamValue::Choice(0), false),
                ParamUi::Mask => (ParamKind::Choice(MASK_CHOICES), ParamValue::Choice(0), false),
                ParamUi::Text | ParamUi::File { .. } => {
                    let s = match &ps.default {
                        Value::Str(s) => s.clone(),
                        _ => String::new(),
                    };
                    (ParamKind::Text, ParamValue::Text(s), false)
                }
                ParamUi::Path | ParamUi::Gradient | ParamUi::Hidden => continue,
            };
            params.push(ParamDef { id: ps.id, label: ps.name, kind, default, animatable, group });
        }
        out.push(EffectDef {
            id: spec.id,
            name: spec.name,
            kind: EffectKind::Video,
            category: category_slice(spec.category),
            params,
            intrinsic: false,
            accelerated: spec.gpu,
            float32: spec.float,
            yuv: false,
        });
    }
    out
}

/// Fractional frame position `(fx, fy)` for an EffectCraft point parameter's default.
pub fn auto_point(effect: &str, param: &str) -> Option<(f64, f64)> {
    if !effect.starts_with("ec.") {
        return None;
    }
    let spec = filmcraft_effects::find(effect)?;
    let ps = spec.params.iter().find(|p| p.id == param)?;
    let [fx, fy] = ps.default.as_vec2();
    Some((fx, fy))
}
