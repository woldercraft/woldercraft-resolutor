//! Standard effects with a GPU implementation (`filmcraft_gpu` runs them in WGSL).
//!
//! Each effect here is evaluated in two steps: [`FxOp::eval`] reads the effect's parameters at a
//! time (keyframes, defaults, `px_scale`, the working image size) into plain numbers, and
//! [`FxOp::apply`] runs the CPU reference on a working [`Image`]. `effects::apply` uses exactly
//! these two steps, so the CPU render and the GPU plan agree on every parameter (and on how
//! hostile values are clamped) by construction. The GPU compositor receives the evaluated
//! [`FxOp`]s in a [`crate::plan::LayerFx`] and reproduces `apply` per pixel; the plan only hands an
//! op to the GPU when [`FxOp::gpu_ok`] says the shader covers it (finite numbers, no minifying
//! resample), otherwise the clip is rendered on the CPU as before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use filmcraft_color::{hsl_to_rgb, linear_to_srgb, luma709, rgb_to_hsl};
use filmcraft_geom::Affine;
use filmcraft_project::EffectInstance;
use rayon::prelude::*;

use crate::effects::{
    CW_LUT_H, CW_LUT_W, ColorSliceParams, ColorWarperTable, CstParams, FxCtx, HdrWheelsParams, LogWheelsParams, NodeSizingParams, RgbMixerParams,
    apply_color_slice_unit, apply_color_warper_unit, apply_contrast_pivot, apply_cst_unit, apply_hdr_wheels_linear, apply_log_wheels_unit,
    apply_lum_mix_offset, apply_primaries_wheels_unit, apply_rgb_mixer_unit, apply_sat_vib_boost, apply_soft_clip, b, build_warper_table, choice, color,
    curve_lut, curve_param, dec, enc, f, gaussian_boxes, grade_model_from_effect, grade_space, hue_lut, is_identity_curve, lift_scale, midtone_detail_boxes,
    on, point, text, vs_sat_lut,
};
use crate::image::Image;

pub const LUMETRI_CURVE_LEN: usize = 1024;

/// Packed 1024×3 RGBA curve LUTs for GPU Lumetri Curves (`OP_LUMETRI_CURVES`).
#[derive(Clone, Debug, PartialEq)]
pub struct LumetriCurveTable {
    /// 3 rows of 1024 RGBA f32 texels:
    /// row 0 = [luma, red, green, blue],
    /// row 1 = [hue_vs_hue, hue_vs_sat, luma_vs_sat, sat_vs_sat],
    /// row 2 = [hue_vs_luma, sat_vs_luma, 0, 0].
    pub texels: Arc<Vec<[f32; 4]>>,
    /// Bitmask of active curves (0: luma .. 9: svl).
    pub active_mask: u32,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct CurveCacheKey([Option<Vec<[u32; 2]>>; 10]);

fn curve_table_cache() -> &'static Mutex<(HashMap<CurveCacheKey, (Arc<Vec<[f32; 4]>>, u64)>, u64)> {
    static CACHE: OnceLock<Mutex<(HashMap<CurveCacheKey, (Arc<Vec<[f32; 4]>>, u64)>, u64)>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new((HashMap::new(), 0)))
}

fn build_curve_table(e: &EffectInstance) -> Option<LumetriCurveTable> {
    let tone_ids = ["curve_luma", "curve_red", "curve_green", "curve_blue"];

    let mut active_mask = 0u32;
    let mut raw: [Option<Vec<[f32; 2]>>; 10] = std::array::from_fn(|_| None);
    for (i, id) in tone_ids.iter().enumerate() {
        if let Some(c) = curve_param(e, id).filter(|c| !is_identity_curve(c)) {
            active_mask |= 1u32 << i;
            raw[i] = Some(c);
        }
    }
    for (idx, id) in ["hue_vs_hue", "hue_vs_sat", "luma_vs_sat", "sat_vs_sat", "hue_vs_luma", "sat_vs_luma"].iter().enumerate() {
        let valid = curve_param(e, id).filter(|c| !c.is_empty());
        if let Some(c) = valid {
            active_mask |= 1u32 << (4 + idx);
            raw[4 + idx] = Some(c);
        }
    }
    if active_mask == 0 {
        return None;
    }

    let key = CurveCacheKey(raw.each_ref().map(|opt| opt.as_ref().map(|v| v.iter().map(|p| [p[0].to_bits(), p[1].to_bits()]).collect())));
    {
        let mut guard = curve_table_cache().lock().unwrap_or_else(PoisonError::into_inner);
        let (cache, tick) = &mut *guard;
        *tick = tick.wrapping_add(1);
        let now = *tick;
        if let Some((texels, last_used)) = cache.get_mut(&key) {
            *last_used = now;
            return Some(LumetriCurveTable { texels: texels.clone(), active_mask });
        }
    }

    let mut texels = vec![[0.0f32; 4]; LUMETRI_CURVE_LEN * 3];
    for (c_idx, slot) in raw[0..4].iter().enumerate() {
        if let Some(pts) = slot {
            let l = curve_lut(pts, LUMETRI_CURVE_LEN);
            for (x, &v) in l.iter().enumerate() {
                texels[x][c_idx] = v;
            }
        }
    }
    if let Some(pts) = &raw[4]
        && let Some(l) = hue_lut(pts, LUMETRI_CURVE_LEN)
    {
        for (x, &v) in l.iter().enumerate() {
            texels[LUMETRI_CURVE_LEN + x][0] = v;
        }
    }
    if let Some(pts) = &raw[5]
        && let Some(l) = hue_lut(pts, LUMETRI_CURVE_LEN)
    {
        for (x, &v) in l.iter().enumerate() {
            texels[LUMETRI_CURVE_LEN + x][1] = v;
        }
    }
    if let Some(pts) = &raw[6]
        && let Some(l) = vs_sat_lut(pts, LUMETRI_CURVE_LEN)
    {
        for (x, &v) in l.iter().enumerate() {
            texels[LUMETRI_CURVE_LEN + x][2] = v;
        }
    }
    if let Some(pts) = &raw[7]
        && let Some(l) = vs_sat_lut(pts, LUMETRI_CURVE_LEN)
    {
        for (x, &v) in l.iter().enumerate() {
            texels[LUMETRI_CURVE_LEN + x][3] = v;
        }
    }
    if let Some(pts) = &raw[8]
        && let Some(l) = hue_lut(pts, LUMETRI_CURVE_LEN)
    {
        for (x, &v) in l.iter().enumerate() {
            texels[LUMETRI_CURVE_LEN * 2 + x][0] = v;
        }
    }
    if let Some(pts) = &raw[9]
        && let Some(l) = vs_sat_lut(pts, LUMETRI_CURVE_LEN)
    {
        for (x, &v) in l.iter().enumerate() {
            texels[LUMETRI_CURVE_LEN * 2 + x][1] = v;
        }
    }

    let arc = Arc::new(texels);
    let mut guard = curve_table_cache().lock().unwrap_or_else(PoisonError::into_inner);
    let (cache, tick) = &mut *guard;
    *tick = tick.wrapping_add(1);
    let now = *tick;
    while cache.len() >= 32 {
        let Some(oldest) = cache.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone()) else {
            break;
        };
        cache.remove(&oldest);
    }
    cache.insert(key, (arc.clone(), now));
    Some(LumetriCurveTable { texels: arc, active_mask })
}

#[derive(Clone, Debug, PartialEq)]
pub struct HslSecondaryParams {
    /// 0 = Node Key (masks the entire Lumetri node), 1 = Secondary Only (legacy 4 internal sliders).
    pub key_scope: u32,
    /// 0 = HSL, 1 = RGB, 2 = Luma, 3 = 3D.
    pub mode: u32,
    pub hc: f32,
    pub hr: f32,
    pub smin: f32,
    pub smax: f32,
    pub lmin: f32,
    pub lmax: f32,
    pub soft: f32,
    pub sat_soft: f32,
    pub luma_soft: f32,
    pub rgb_min: [f32; 3],
    pub rgb_max: [f32; 3],
    pub rgb_soft: f32,
    /// Up to 64 `[r, g, b, tol_signed]` points (`tol_signed > 0` add, `< 0` subtract).
    pub samples_3d: Arc<Vec<[f32; 4]>>,
    pub show_mask: u32,
    pub clean_black: f32,
    pub clean_white: f32,
    pub in_out: f32,
    pub in_out_r: u32,
    pub invert_key: bool,
    pub key_gain: f32,
    pub htemp: f32,
    pub htint: f32,
    pub hsat: f32,
    pub hshift: f32,
    pub denoise: f32,
    pub denoise_r: u32,
    pub blur_rx: Vec<u32>,
    pub blur_ry: Vec<u32>,
}

/// Precomputed mask coverage for [`FxOp::Masked`].
#[derive(Clone, Debug, PartialEq)]
pub struct MaskCoverage {
    pub w: u32,
    pub h: u32,
    pub cov: Arc<Vec<f32>>,
}

/// A bilinear resample of the working image through an affine map (`Image::transformed` at a
/// magnification or mild minification: no mip pre-filter).
#[derive(Clone, Debug, PartialEq)]
pub struct Resample {
    /// Destination pixel → source pixel (None: singular map, the result is transparent).
    pub inv: Option<Affine>,
    /// Destination pixels written: x0..x1, y0..y1 (the rest becomes transparent).
    pub rect: [u32; 4],
    /// Alpha scale applied afterwards (Transform's Opacity; 1 = none).
    pub opacity: f32,
}

/// One evaluated effect.
#[derive(Clone, Debug, PartialEq)]
pub enum FxOp {
    BrightnessContrast {
        br: f32,
        co: f32,
    },
    ProcAmp {
        br: f32,
        co: f32,
        hue: f32,
        sat: f32,
    },
    Tint {
        black: [f32; 3],
        white: [f32; 3],
        amount: f32,
    },
    BlackWhite,
    ColorBalance {
        sh: [f32; 3],
        md: [f32; 3],
        hi: [f32; 3],
        preserve: bool,
    },
    /// Three-Way Color Corrector: per tonal range an encoded-RGB offset (the wheel) and a level
    /// shift (`[r, g, b, level]`), the range edges `[shadow lo, shadow hi, highlight lo,
    /// highlight hi]` on encoded luma, and a saturation factor.
    ThreeWay {
        sh: [f32; 4],
        md: [f32; 4],
        hi: [f32; 4],
        range: [f32; 4],
        sat: f32,
    },
    LeaveColor {
        amount: f32,
        key_hue: f32,
        tol: f32,
        soft: f32,
    },
    ChangeToColor {
        from_hue: f32,
        to_hue: f32,
        tol: f32,
        soft: f32,
    },
    ColorPass {
        key: [f32; 3],
        sim: f32,
        reverse: bool,
    },
    Gamma {
        g: f32,
    },
    Levels {
        ib: f32,
        iw: f32,
        ob: f32,
        ow: f32,
        g: f32,
    },
    Extract {
        lo: f32,
        hi: f32,
        soft: f32,
        invert: bool,
    },
    Invert {
        channel: u32,
        blend: f32,
    },
    Posterize {
        n: f32,
    },
    /// Three box passes per axis approximating a Gaussian (radii per pass; empty: axis untouched).
    Gaussian {
        rx: Vec<u32>,
        ry: Vec<u32>,
        repeat: bool,
    },
    /// `steps` bilinear (edge-clamped) taps along `(dx, dy)` centred on the pixel.
    DirectionalBlur {
        dx: f32,
        dy: f32,
        steps: u32,
    },
    /// Bokeh Blur: `levels` halvings of the working image, highlights boosted (`boost` × a
    /// smoothstep from `threshold`), an iris-shaped gather of `rings` rings out to radius `rr`
    /// (`sides` 0 = circle, rotated by `rot` radians), then scaled back up. `rings` 0: no-op.
    Bokeh {
        levels: u32,
        boost: f32,
        threshold: f32,
        sides: u32,
        rot: f32,
        rr: f32,
        rings: u32,
    },
    /// Unsharp mask over a repeat-edge Gaussian of the given box radii (per axis).
    Unsharp {
        rx: Vec<u32>,
        ry: Vec<u32>,
        amount: f32,
        threshold: f32,
    },
    /// Crop / Edge Feather: alpha falloff `feather` px inside the rectangle (0: half-pixel AA).
    Crop {
        x0: f32,
        x1: f32,
        y0: f32,
        y1: f32,
        feather: f32,
    },
    Resample(Resample),
    HFlip,
    VFlip,
    Mirror {
        cx: f32,
        cy: f32,
        nx: f32,
        ny: f32,
    },
    /// Wrap-around shift (dx, dy already reduced into 0..w, 0..h), mixed with the original.
    Offset {
        dx: f32,
        dy: f32,
        blend: f32,
    },
    /// ASC CDL on display-encoded colour (slope, offset, power, saturation).
    AscCdl {
        slope: [f32; 3],
        offset: [f32; 3],
        power: [f32; 3],
        sat: f32,
    },
    /// Channel Mixer: rows (r, g, b, constant) per output channel, on display-encoded colour.
    ChannelMix {
        m: [[f32; 4]; 3],
    },
    /// Color Replace: `sim` already ×1.2; `replace_hsl` the HSL of the replacement colour.
    ColorReplace {
        sim: f32,
        solid: bool,
        target: [f32; 3],
        replace: [f32; 3],
        replace_hsl: [f32; 3],
    },
    AlphaAdjust {
        opacity: f32,
        ignore: bool,
        invert: bool,
        mask_only: bool,
    },
    Vignette {
        amount: f32,
        midpoint: f32,
        roundness: f32,
        feather: f32,
        target: [f32; 3],
    },
    VideoLimiter {
        max: f32,
        comp: f32,
        axis: u32,
        warn: bool,
        warning_color: [f32; 3],
    },
    Lumetri(Box<LumetriOp>),
    /// Compiled non-linear [`filmcraft_project::ColorGraph`] DAG executed across register slots.
    ColorDag(Arc<crate::color_graph::ColorDagOp>),
    /// Any GPU effect restricted to active mask coverage (`lerp(before, after, coverage)`).
    Masked {
        inner: Box<FxOp>,
        mask: MaskCoverage,
    },
}

/// Boxed payload for [`FxOp::Lumetri`] so the enum stays small on the stack.
#[derive(Clone, Debug, PartialEq)]
pub struct LumetriOp {
    pub input_lut: Option<Arc<filmcraft_color::Lut3d>>,
    pub look_lut: Option<Arc<filmcraft_color::Lut3d>>,
    pub gains: [f32; 3],
    pub exposure: f32,
    pub contrast: f32,
    pub pivot: f32,
    pub color_boost: f32,
    pub midtone_detail: f32,
    pub midtone_rx: Vec<u32>,
    pub midtone_ry: Vec<u32>,
    pub primary_hue: f32,
    pub hl: f32,
    pub sh: f32,
    pub wh: f32,
    pub bl: f32,
    pub sat: f32,
    pub creative_on: bool,
    pub faded: f32,
    pub vib: f32,
    pub st: [f32; 3],
    pub ht: [f32; 3],
    pub vignette_on: bool,
    pub va: f32,
    pub vmid: f32,
    pub vround: f32,
    pub vfeather: f32,
    pub aspect: f32,
    pub look: u32,
    pub look_k: f32,
    pub sharpen: f32,
    pub sharpen_rx: Vec<u32>,
    pub sharpen_ry: Vec<u32>,
    pub wheels_on: bool,
    pub grade_model: u32,
    pub ws: [f32; 3],
    pub wm: [f32; 3],
    pub wh_rgb: [f32; 3],
    pub wo: [f32; 3],
    pub ls: f32,
    pub lm: f32,
    pub lh: f32,
    pub lo: f32,
    pub log_wheels: Option<Box<LogWheelsParams>>,
    pub hdr_wheels: Option<Box<HdrWheelsParams>>,
    pub rgb_mixer: Option<Box<RgbMixerParams>>,
    pub cst: Option<Box<CstParams>>,
    pub sizing: Option<Box<NodeSizingParams>>,
    pub curves: Option<LumetriCurveTable>,
    pub low_soft: f32,
    pub high_soft: f32,
    pub color_slice: Option<ColorSliceParams>,
    pub warper: Option<ColorWarperTable>,
    pub hsl: Option<Box<HslSecondaryParams>>,
    pub key_gain: f32,
    pub show_mask: u32,
    pub node_mask: Option<MaskCoverage>,
    pub grade_space: filmcraft_color::GradeSpace,
    pub hdr_specular: f32,
    pub gpu_capable: bool,
}

/// Effect ids [`FxOp::eval`] understands (the GPU-capable standard effects).
pub const GPU_EFFECTS: &[&str] = &[
    "brightness_contrast",
    "proc_amp",
    "tint",
    "black_white",
    "color_balance",
    "three_way_color",
    "leave_color",
    "change_to_color",
    "color_pass",
    "gamma_correction",
    "levels",
    "extract",
    "invert",
    "posterize",
    "gaussian_blur",
    "gaussian_blur_legacy",
    "camera_blur",
    "directional_blur",
    "directional_blur_legacy",
    "bokeh_blur",
    "sharpen",
    "unsharp_mask",
    "crop",
    "edge_feather",
    "transform",
    "horizontal_flip",
    "vertical_flip",
    "mirror",
    "offset",
    "asc_cdl",
    "channel_mix",
    "color_replace",
    "alpha_adjust",
    "vignette",
    "video_limiter",
    "lumetri",
    "ec.blur.gaussian",
    "ec.blur.directional",
    "ec.blur.sharpen",
    "ec.blur.unsharp",
    "ec.color.tint",
    "ec.color.levels",
    "ec.stylize.posterize",
    "ec.distort.mirror",
    "ec.distort.offset",
];

#[allow(clippy::too_many_arguments)]
#[inline]
fn apply_phase2_wheels_and_mixer(
    mut v: [f32; 3],
    wheels_active: bool,
    grade_model: u32,
    lift_k: [f32; 3],
    gamma_k: [f32; 3],
    gain_k: [f32; 3],
    offset_k: [f32; 3],
    log_wheels: Option<&LogWheelsParams>,
    hdr_wheels: Option<&HdrWheelsParams>,
    rgb_mixer: Option<&RgbMixerParams>,
) -> [f32; 3] {
    if wheels_active {
        v = apply_primaries_wheels_unit(enc(dec(v)), grade_model, lift_k, gamma_k, gain_k, offset_k);
    }
    if let Some(lw) = log_wheels {
        v = apply_log_wheels_unit(enc(dec(v)), lw);
    }
    if let Some(hw) = hdr_wheels {
        v = enc(apply_hdr_wheels_linear(dec(v), hw));
    }
    if let Some(rm) = rgb_mixer {
        v = apply_rgb_mixer_unit(enc(dec(v)), rm);
    }
    v
}

/// `Image::transformed` without the mip path: the destination rectangle it writes and the inverse.
fn resample(w: usize, h: usize, m: &Affine, opacity: f32) -> Resample {
    let Some(inv) = m.inverse() else { return Resample { inv: None, rect: [0; 4], opacity } };
    let b = m.bounds(&filmcraft_geom::Rect::new(0.0, 0.0, w as f64, h as f64));
    let y0 = (b.y.floor().max(0.0) as usize).min(h);
    let y1 = (b.bottom().ceil().max(0.0) as usize).min(h);
    let x0 = (b.x.floor().max(0.0) as usize).min(w);
    let x1 = (b.right().ceil().max(0.0) as usize).min(w);
    Resample { inv: Some(inv), rect: [x0 as u32, x1 as u32, y0.min(y1) as u32, y1 as u32], opacity }
}

/// Whether `Image::transformed` would take its mip (downsample) path for `m` on a `w`×`h` image.
fn minifies(m: &Affine, w: usize, h: usize) -> bool {
    let sx = (m.a * m.a + m.b * m.b).sqrt();
    let sy = (m.c * m.c + m.d * m.d).sqrt();
    let minify = 1.0 / sx.min(sy).max(1e-6);
    // (a non-finite factor: keep such maps off the GPU; `apply` still hands them to the CPU's
    // `Image::transformed`, which decides as before)
    !minify.is_finite() || minify >= 2.0 && w >= 4 && h >= 4
}

/// Whether `ec.color.levels` per-channel controls (R/G/B/A) are all at identity defaults.
fn ec_levels_channels_identity(e: &EffectInstance, cx: &FxCtx) -> bool {
    const PREFIXES: [&str; 4] = ["red", "green", "blue", "alpha"];
    const DEFAULTS: [(&str, f32); 5] = [("InBlack", 0.0), ("InWhite", 1.0), ("Gamma", 1.0), ("OutBlack", 0.0), ("OutWhite", 1.0)];
    for pre in PREFIXES {
        for (suffix, def) in DEFAULTS {
            let key = format!("{pre}{suffix}");
            if let Some(p) = e.param(&key)
                && (p.f64_at(cx.t) as f32 - def).abs() > 1e-6
            {
                return false;
            }
        }
    }
    true
}

impl FxOp {
    /// A Three-Way Color Corrector with neutral controls (and any other op: false): it leaves the
    /// image untouched, HDR values included.
    pub fn is_identity_three_way(&self) -> bool {
        matches!(self, FxOp::ThreeWay { sh, md, hi, sat, .. }
            if [sh, md, hi].iter().all(|w| w.iter().all(|v| *v == 0.0)) && *sat == 1.0)
    }

    /// Evaluate `e` at `cx.t` for a `w`×`h` working image. None for effects without a GPU
    /// implementation (and for disabled effects).
    #[allow(clippy::cognitive_complexity)]
    pub fn eval(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<FxOp> {
        if !e.enabled {
            return None;
        }
        Some(match e.effect.as_str() {
            "brightness_contrast" => FxOp::BrightnessContrast { br: f(e, "brightness", cx) / 100.0 * 0.4, co: 1.0 + f(e, "contrast", cx) / 100.0 },
            "proc_amp" => FxOp::ProcAmp {
                br: f(e, "brightness", cx) / 100.0 * 0.4,
                co: f(e, "contrast", cx) / 100.0,
                hue: f(e, "hue", cx) / 360.0,
                sat: f(e, "saturation", cx) / 100.0,
            },
            "tint" => {
                let (bl, wh) = (color(e, "black", cx), color(e, "white", cx));
                FxOp::Tint { black: [bl[0], bl[1], bl[2]], white: [wh[0], wh[1], wh[2]], amount: f(e, "amount", cx) / 100.0 }
            }
            "black_white" => FxOp::BlackWhite,
            "color_balance" => {
                let g = |k: &str| f(e, k, cx) / 100.0 * 0.25;
                FxOp::ColorBalance {
                    sh: [g("shadow_r"), g("shadow_g"), g("shadow_b")],
                    md: [g("mid_r"), g("mid_g"), g("mid_b")],
                    hi: [g("hi_r"), g("hi_g"), g("hi_b")],
                    preserve: b(e, "preserve"),
                }
            }
            "three_way_color" => {
                // a wheel: its hue as an RGB direction without the grey component, scaled by Balance
                let wheel = |hue: &str, bal: &str, lvl: &str| {
                    let h = f(e, hue, cx).rem_euclid(360.0) / 360.0;
                    let c = filmcraft_color::hsl_to_rgb(h, 1.0, 0.5);
                    let m = (c[0] + c[1] + c[2]) / 3.0;
                    let k = f(e, bal, cx).clamp(0.0, 100.0) / 100.0 * 0.5;
                    [(c[0] - m) * k, (c[1] - m) * k, (c[2] - m) * k, f(e, lvl, cx) / 100.0 * 0.25]
                };
                // the edges of a range: the threshold ± half its softness, on 0..1 luma
                let edges = |thr: &str, soft: &str| {
                    let (t, s) = (f(e, thr, cx).clamp(0.0, 100.0) / 100.0, f(e, soft, cx).clamp(0.0, 100.0) / 100.0 * 0.5);
                    ((t - s).clamp(0.0, 1.0), (t + s).clamp(0.0, 1.0))
                };
                let (s0, s1) = edges("shadow_threshold", "shadow_softness");
                let (h0, h1) = edges("hi_threshold", "hi_softness");
                FxOp::ThreeWay {
                    sh: wheel("shadow_hue", "shadow_balance", "shadow_level"),
                    md: wheel("mid_hue", "mid_balance", "mid_level"),
                    hi: wheel("hi_hue", "hi_balance", "hi_level"),
                    range: [s0, s1, h0, h1],
                    sat: f(e, "saturation", cx) / 100.0,
                }
            }
            "leave_color" => {
                let key = color(e, "color", cx);
                FxOp::LeaveColor {
                    amount: f(e, "amount", cx) / 100.0,
                    key_hue: rgb_to_hsl(key[0], key[1], key[2])[0],
                    tol: f(e, "tolerance", cx) / 100.0,
                    soft: f(e, "softness", cx) / 100.0 + 1e-4,
                }
            }
            "change_to_color" => {
                let (from, to) = (color(e, "from", cx), color(e, "to", cx));
                FxOp::ChangeToColor {
                    from_hue: rgb_to_hsl(from[0], from[1], from[2])[0],
                    to_hue: rgb_to_hsl(to[0], to[1], to[2])[0],
                    tol: f(e, "hue_tol", cx) / 100.0,
                    soft: f(e, "softness", cx) / 100.0 * 0.3 + 1e-4,
                }
            }
            "color_pass" => {
                let key = color(e, "color", cx);
                FxOp::ColorPass { key: [key[0], key[1], key[2]], sim: f(e, "similarity", cx) / 100.0, reverse: b(e, "reverse") }
            }
            "gamma_correction" => FxOp::Gamma { g: f(e, "gamma", cx) / 10.0 },
            "levels" => {
                let ib = f(e, "in_black", cx) / 255.0;
                FxOp::Levels {
                    ib,
                    iw: (f(e, "in_white", cx) / 255.0).max(ib + 1e-3),
                    ob: f(e, "out_black", cx) / 255.0,
                    ow: f(e, "out_white", cx) / 255.0,
                    g: 100.0 / f(e, "gamma", cx).max(1.0),
                }
            }
            "extract" => {
                let softness = f(e, "softness", cx);
                FxOp::Extract {
                    lo: f(e, "black", cx) / 255.0,
                    hi: f(e, "white", cx) / 255.0,
                    // Zero softness is a hard cutoff. A tiny substitute fade amplifies
                    // CPU/GPU transfer-function rounding and introduces unwanted grey edges.
                    soft: if softness <= 0.0 { 0.0 } else { softness / 100.0 * 0.2 + 1e-4 },
                    invert: b(e, "invert"),
                }
            }
            "invert" => FxOp::Invert { channel: choice(e, "channel"), blend: f(e, "blend", cx) / 100.0 },
            "posterize" => FxOp::Posterize { n: f(e, "levels", cx).max(2.0) - 1.0 },
            "gaussian_blur" | "gaussian_blur_legacy" => {
                let r = f(e, "blurriness", cx) * cx.px_scale * 0.5;
                let dims = choice(e, "dimensions");
                let (rx, ry) = gaussian_boxes(w, h, if dims == 2 { 0.0 } else { r }, if dims == 1 { 0.0 } else { r });
                FxOp::Gaussian { rx, ry, repeat: b(e, "repeat_edge") }
            }
            "camera_blur" => {
                let r = f(e, "percent", cx) * cx.px_scale * 0.3;
                let (rx, ry) = gaussian_boxes(w, h, r, r);
                FxOp::Gaussian { rx, ry, repeat: true }
            }
            "directional_blur" | "directional_blur_legacy" => {
                let len = f(e, "length", cx) * cx.px_scale * 2.0;
                let dir = (f(e, "direction", cx) as f64).to_radians();
                if len < 0.5 {
                    FxOp::DirectionalBlur { dx: 0.0, dy: 0.0, steps: 0 }
                } else {
                    let steps = (len.ceil() as usize).clamp(2, 64) as u32;
                    FxOp::DirectionalBlur { dx: (dir.sin() as f32) * len, dy: (-dir.cos() as f32) * len, steps }
                }
            }
            "bokeh_blur" => {
                let r = f(e, "amount", cx) * cx.px_scale;
                if !r.is_finite() || r < 0.5 {
                    return Some(FxOp::Bokeh { levels: 0, boost: 0.0, threshold: 0.0, sides: 0, rot: 0.0, rr: 0.0, rings: 0 });
                }
                // Large radii work on a halved (and halved again…) image: the result is smooth.
                let (mut ww, mut wh, mut down, mut levels) = (w, h, 1.0f32, 0u32);
                while r / down > 12.0 && ww >= 8 && wh >= 8 {
                    (ww, wh) = ((ww / 2).max(1), (wh / 2).max(1));
                    down *= 2.0;
                    levels += 1;
                }
                let rr = r / down;
                FxOp::Bokeh {
                    levels,
                    boost: f(e, "highlight", cx) / 100.0 * 6.0,
                    threshold: f(e, "threshold", cx) / 100.0,
                    sides: [0u32, 3, 4, 5, 6, 8][crate::vfx::chv(e, "shape").min(5) as usize],
                    rot: f(e, "rotation", cx).to_radians(),
                    rr,
                    rings: (rr.ceil() as usize).clamp(1, 12) as u32,
                }
            }
            "sharpen" => {
                // below a sigma of ~0.58 the three boxes all round to radius 0 and the mask is a
                // no-op, so Sharpen vanished at 1/2 playback resolution and below (#1351)
                let r = cx.px_scale.max(0.6);
                let (rx, ry) = gaussian_boxes(w, h, r, r);
                FxOp::Unsharp { rx, ry, amount: f(e, "amount", cx) / 100.0, threshold: 0.0 }
            }
            "unsharp_mask" => {
                let r = f(e, "radius", cx) * cx.px_scale;
                let (rx, ry) = gaussian_boxes(w, h, r, r);
                FxOp::Unsharp { rx, ry, amount: f(e, "amount", cx) / 100.0, threshold: f(e, "threshold", cx) / 255.0 }
            }
            "crop" => {
                let l = f(e, "left", cx) / 100.0;
                let t = f(e, "top", cx) / 100.0;
                let r = f(e, "right", cx) / 100.0;
                let bt = f(e, "bottom", cx) / 100.0;
                if b(e, "zoom") && l + r < 0.99 && t + bt < 0.99 {
                    let (fw, fh) = (w as f64, h as f64);
                    let m = Affine::scale(1.0 / (1.0 - (l + r) as f64), 1.0 / (1.0 - (t + bt) as f64))
                        .then_apply(&Affine::translate(-(l as f64) * fw, -(t as f64) * fh));
                    if minifies(&m, w, h) {
                        // negative crops zoom out: the CPU's mip path (see `transform`)
                        return Some(FxOp::Resample(Resample { inv: Some(m), rect: [u32::MAX; 4], opacity: 1.0 }));
                    }
                    FxOp::Resample(resample(w, h, &m, 1.0))
                } else {
                    FxOp::crop_rect(w, h, l, t, r, bt, f(e, "feather", cx) * cx.px_scale)
                }
            }
            "edge_feather" => {
                let amt = f(e, "amount", cx) / 100.0 * (w.min(h) as f32) * 0.5;
                FxOp::crop_rect(w, h, 0.0, 0.0, 0.0, 0.0, amt)
            }
            "transform" => {
                // With its own shutter angle the CPU blurs the motion over seven samples
                // (`vfx::distort::transform_fx`, under the same condition); the GPU op places one
                // picture, so this stays on the CPU rather than losing the blur
                if crate::vfx::bv(e, "shutter_override") && crate::vfx::fv(e, "shutter_angle", cx) > 0.5 {
                    return None;
                }
                let img = Image { w, h, px: Vec::new() };
                let anchor = point(e, "anchor", cx, &img);
                let pos = point(e, "position", cx, &img);
                let sh = f(e, "scale_height", cx) as f64 / 100.0;
                let sw = if b(e, "uniform_scale") { sh } else { f(e, "scale_width", cx) as f64 / 100.0 };
                let rot = f(e, "rotation", cx) as f64;
                let skew = (f(e, "skew", cx) as f64).to_radians().tan();
                let skew_axis = f(e, "skew_axis", cx) as f64;
                let op = f(e, "opacity", cx) / 100.0;
                let sk = Affine::rotate_deg(skew_axis)
                    .then_apply(&Affine { a: 1.0, b: 0.0, c: skew, d: 1.0, e: 0.0, f: 0.0 })
                    .then_apply(&Affine::rotate_deg(-skew_axis));
                let m = Affine::translate(pos.x, pos.y)
                    .then_apply(&Affine::rotate_deg(rot))
                    .then_apply(&sk)
                    .then_apply(&Affine::scale(sw, sh))
                    .then_apply(&Affine::translate(-anchor.x, -anchor.y));
                if minifies(&m, w, h) {
                    // the CPU's mip path (`Image::transformed` with the forward map; `rect`
                    // u32::MAX marks it): CPU only
                    return Some(FxOp::Resample(Resample { inv: Some(m), rect: [u32::MAX; 4], opacity: op }));
                }
                FxOp::Resample(resample(w, h, &m, op))
            }
            "horizontal_flip" => FxOp::HFlip,
            "vertical_flip" => FxOp::VFlip,
            "mirror" => {
                let img = Image { w, h, px: Vec::new() };
                let c = point(e, "center", cx, &img);
                let ang = (f(e, "angle", cx) as f64).to_radians();
                FxOp::Mirror { cx: c.x as f32, cy: c.y as f32, nx: ang.cos() as f32, ny: ang.sin() as f32 }
            }
            "offset" => {
                let img = Image { w, h, px: Vec::new() };
                let s = point(e, "shift", cx, &img);
                let (dx, dy) = (s.x - w as f64 / 2.0, s.y - h as f64 / 2.0);
                FxOp::Offset { dx: dx.rem_euclid(w.max(1) as f64) as f32, dy: dy.rem_euclid(h.max(1) as f64) as f32, blend: f(e, "blend", cx) / 100.0 }
            }
            "asc_cdl" => {
                use crate::vfx::fv;
                FxOp::AscCdl {
                    slope: [fv(e, "r_slope", cx), fv(e, "g_slope", cx), fv(e, "b_slope", cx)],
                    offset: [fv(e, "r_offset", cx), fv(e, "g_offset", cx), fv(e, "b_offset", cx)],
                    power: [fv(e, "r_power", cx), fv(e, "g_power", cx), fv(e, "b_power", cx)],
                    sat: fv(e, "saturation", cx),
                }
            }
            "channel_mix" => {
                use crate::vfx::{bv, fv};
                let g = |k: &str| fv(e, k, cx) / 100.0;
                let mut m = [[g("rr"), g("rg"), g("rb"), g("rc")], [g("gr"), g("gg"), g("gb"), g("gc")], [g("br"), g("bg"), g("bb"), g("bc")]];
                if bv(e, "monochrome") {
                    m = [m[0]; 3];
                }
                FxOp::ChannelMix { m }
            }
            "color_replace" => {
                use crate::vfx::{bv, cv, fv};
                let t = cv(e, "target", cx);
                let r = cv(e, "replace", cx);
                FxOp::ColorReplace {
                    sim: fv(e, "similarity", cx) / 100.0 * 1.2,
                    solid: bv(e, "solid"),
                    target: [t[0], t[1], t[2]],
                    replace: [r[0], r[1], r[2]],
                    replace_hsl: rgb_to_hsl(r[0], r[1], r[2]),
                }
            }
            "alpha_adjust" => {
                use crate::vfx::{bv, fv};
                FxOp::AlphaAdjust { opacity: fv(e, "opacity", cx) / 100.0, ignore: bv(e, "ignore"), invert: bv(e, "invert"), mask_only: bv(e, "mask_only") }
            }
            "vignette" => {
                use crate::vfx::{cv, fv};
                let amt = fv(e, "amount", cx) / 100.0;
                let mid = fv(e, "midpoint", cx) / 100.0;
                let round = fv(e, "roundness", cx) / 100.0;
                let raw_feather = fv(e, "feather", cx);
                let feather = if raw_feather.is_finite() { (raw_feather / 100.0).max(0.01) } else { raw_feather };
                let col = cv(e, "color", cx);
                let target = if amt < 0.0 { [col[0], col[1], col[2]] } else { [1.0; 3] };
                FxOp::Vignette { amount: amt, midpoint: mid, roundness: round, feather, target }
            }
            "video_limiter" => {
                use crate::vfx::{bv, cv};
                use filmcraft_project::ParamValue;
                let clip_level = match e.param("clip_level").map(|p| p.value.clone()).or_else(|| crate::vfx::def_value(e, "clip_level")) {
                    Some(ParamValue::Choice(c)) => c as f32,
                    Some(ParamValue::Float(f)) => f as f32,
                    _ => 0.0,
                };
                let comp_val = match e.param("compression").map(|p| p.value.clone()).or_else(|| crate::vfx::def_value(e, "compression")) {
                    Some(ParamValue::Choice(c)) => [0.0, 0.03, 0.05, 0.10, 0.20][(c as usize).min(4)],
                    Some(ParamValue::Float(f)) => f as f32,
                    _ => 0.03,
                };
                let axis = match e.param("axis").map(|p| p.value.clone()).or_else(|| crate::vfx::def_value(e, "axis")) {
                    Some(ParamValue::Choice(c)) => c,
                    Some(ParamValue::Float(f)) => {
                        if f.is_nan() {
                            u32::MAX
                        } else {
                            f as u32
                        }
                    }
                    _ => 3,
                };
                let warn = bv(e, "gamut_warning");
                let c = cv(e, "warning_color", cx);
                let wc = dec([c[0], c[1], c[2]]);
                FxOp::VideoLimiter { max: 1.0 + clip_level / 100.0, comp: comp_val, axis, warn, warning_color: wc }
            }
            "lumetri" => {
                use crate::effects::wheel_rgb;
                let (basic_on, creative_on, vignette_on) = (on(e, "basic_on"), on(e, "creative_on"), on(e, "vignette_on"));
                let gs = grade_space(e, cx, "hdr_white");
                let is_hdr = gs.is_hdr();
                let has_input_lut = basic_on && !text(e, "input_lut").is_empty();
                let has_look_lut = creative_on && !text(e, "look_lut").is_empty();
                let look = if creative_on && !has_look_lut { choice(e, "look") } else { 0 };
                let look_k = if creative_on { f(e, "look_intensity", cx) / 100.0 } else { 0.0 };
                let sharpen = if creative_on { f(e, "sharpen", cx) / 100.0 } else { 0.0 };
                let (sharpen_rx, sharpen_ry) = if creative_on && sharpen.abs() > 1e-3 {
                    let r = 1.2 * cx.px_scale.max(0.35);
                    gaussian_boxes(w, h, r, r)
                } else {
                    (Vec::new(), Vec::new())
                };

                let curves_on = on(e, "curves_on");
                let curves = if curves_on { build_curve_table(e) } else { None };
                let low_soft = if curves_on { (f(e, "curve_low_soft", cx) / 100.0).clamp(0.0, 1.0) } else { 0.0 };
                let high_soft = if curves_on { (f(e, "curve_high_soft", cx) / 100.0).clamp(0.0, 1.0) } else { 0.0 };
                let color_slice = ColorSliceParams::from_effect(e, cx.t);
                let warper = build_warper_table(e);

                let wheels_on = on(e, "wheels_on");
                let grade_model = grade_model_from_effect(e);
                let lum_mix = if basic_on { (e.param("lum_mix").map_or(100.0, |p| p.f64_at(cx.t)) as f32 / 100.0).clamp(0.0, 1.0) } else { 1.0 };
                let (ws, wm, wh_rgb, wo, ls, lm, lh, lo) = if wheels_on {
                    let v2 = |id: &str| e.param(id).map(|p| p.vec2_at(cx.t)).unwrap_or_default();
                    let (ws, wm, wh_rgb, wo) = (
                        apply_lum_mix_offset(wheel_rgb(v2("wheel_shadows")), lum_mix),
                        apply_lum_mix_offset(wheel_rgb(v2("wheel_midtones")), lum_mix),
                        apply_lum_mix_offset(wheel_rgb(v2("wheel_highlights")), lum_mix),
                        apply_lum_mix_offset(wheel_rgb(v2("wheel_offset")), lum_mix),
                    );
                    let (ls, lm, lh, lo) = (
                        f(e, "wheel_shadows_l", cx) / 100.0,
                        f(e, "wheel_midtones_l", cx) / 100.0,
                        f(e, "wheel_highlights_l", cx) / 100.0,
                        f(e, "wheel_offset_l", cx) / 100.0,
                    );
                    (ws, wm, wh_rgb, wo, ls, lm, lh, lo)
                } else {
                    ([0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3], 0.0, 0.0, 0.0, 0.0)
                };
                let log_wheels = LogWheelsParams::from_effect(e, cx.t, lum_mix).map(Box::new);
                let hdr_wheels = HdrWheelsParams::from_effect(e, cx.t, lum_mix).map(Box::new);
                let rgb_mixer = RgbMixerParams::from_effect(e, cx.t).map(Box::new);
                let cst = CstParams::from_effect(e, cx.t).map(Box::new);
                let sizing = NodeSizingParams::from_effect(e, cx.t).map(Box::new);

                let key_gain = (e.param("key_gain").map_or(100.0, |p| p.f64_at(cx.t)) as f32 / 100.0).clamp(0.0, 1.0);
                let show_mask = choice(e, "hsl_show_mask");
                let hsl = crate::effects::qualifier_params_from_effect(e, cx.t, cx.px_scale, w, h).map(Box::new);

                let input_lut_3d = if has_input_lut { crate::luts::resolve_baked_cube(cx.project, text(e, "input_lut"), 33) } else { None };
                let look_lut_3d = if has_look_lut { crate::luts::resolve_baked_cube(cx.project, text(e, "look_lut"), 33) } else { None };

                let hsl_ok = hsl.as_ref().is_none_or(|h| h.denoise_r <= 3 && h.in_out_r <= 3);

                let bf = |id: &str| if basic_on { f(e, id, cx) } else { 0.0 };
                let temp = bf("temperature") / 100.0;
                let tint = bf("tint") / 100.0;
                let exposure = 2f32.powf(bf("exposure"));
                let contrast = bf("contrast") / 100.0;
                let pivot = if basic_on { (e.param("contrast_pivot").map_or(50.0, |p| p.f64_at(cx.t)) as f32) / 100.0 } else { 0.5 };
                let color_boost = bf("color_boost") / 100.0;
                let midtone_detail = bf("midtone_detail") / 100.0;
                let (midtone_rx, midtone_ry) = if basic_on && midtone_detail.abs() > 1e-3 {
                    let r = 2.4 * cx.px_scale.max(0.35);
                    gaussian_boxes(w, h, r, r)
                } else {
                    (Vec::new(), Vec::new())
                };
                let primary_hue = if basic_on { ((e.param("primary_hue").map_or(50.0, |p| p.f64_at(cx.t)) as f32) - 50.0) / 100.0 } else { 0.0 };
                let hl = bf("highlights") / 100.0;
                let sh = bf("shadows") / 100.0;
                let wh = bf("whites") / 100.0;
                let bl = bf("blacks") / 100.0;
                let hdr_specular = if is_hdr { bf("hdr_specular") / 100.0 } else { 0.0 };
                let sat = if basic_on { f(e, "saturation", cx) / 100.0 } else { 1.0 } * if creative_on { f(e, "creative_sat", cx) / 100.0 } else { 1.0 };
                let vib = if creative_on { f(e, "vibrance", cx) / 100.0 } else { 0.0 };
                let faded = if creative_on { f(e, "faded_film", cx) / 100.0 } else { 0.0 };
                let st = color(e, "shadow_tint", cx);
                let ht = color(e, "highlight_tint", cx);
                let va = if vignette_on { f(e, "vignette_amount", cx) } else { 0.0 };
                let vmid = f(e, "vignette_midpoint", cx) / 100.0;
                let vround = f(e, "vignette_roundness", cx) / 100.0;
                let vfeather = f(e, "vignette_feather", cx) / 100.0;
                let gains = [1.0 + 0.35 * temp, 1.0 - 0.3 * tint, 1.0 - 0.35 * temp];
                let aspect = if h > 0 { (w as f32 / h as f32).max(1e-4) } else { 1.0 };

                let wheels_active = wheels_on
                    && (ws.iter().chain(&wm).chain(&wh_rgb).chain(&wo).any(|v| v.abs() > 1e-5)
                        || ls.abs() > 1e-5
                        || lm.abs() > 1e-5
                        || lh.abs() > 1e-5
                        || lo.abs() > 1e-5);
                let is_basic_hdr_only = !has_input_lut
                    && !has_look_lut
                    && look == 0
                    && !wheels_active
                    && curves.is_none()
                    && color_slice.is_none()
                    && warper.is_none()
                    && hsl.is_none()
                    && log_wheels.is_none()
                    && hdr_wheels.is_none()
                    && rgb_mixer.is_none()
                    && cst.is_none()
                    && sizing.is_none()
                    && sharpen == 0.0
                    && midtone_detail.abs() <= 1e-3;
                let gpu_capable = (!is_hdr || is_basic_hdr_only)
                    && (!has_input_lut || input_lut_3d.is_some())
                    && (!has_look_lut || look_lut_3d.is_some())
                    && look <= 8
                    && hsl_ok;

                FxOp::Lumetri(Box::new(LumetriOp {
                    input_lut: input_lut_3d,
                    look_lut: look_lut_3d,
                    gains,
                    exposure,
                    contrast,
                    pivot,
                    color_boost,
                    midtone_detail,
                    midtone_rx,
                    midtone_ry,
                    primary_hue,
                    hl,
                    sh,
                    wh,
                    bl,
                    sat,
                    creative_on,
                    faded,
                    vib,
                    st: [st[0], st[1], st[2]],
                    ht: [ht[0], ht[1], ht[2]],
                    vignette_on,
                    va,
                    vmid,
                    vround,
                    vfeather,
                    aspect,
                    look,
                    look_k,
                    sharpen,
                    sharpen_rx,
                    sharpen_ry,
                    wheels_on,
                    grade_model,
                    ws,
                    wm,
                    wh_rgb,
                    wo,
                    ls,
                    lm,
                    lh,
                    lo,
                    log_wheels,
                    hdr_wheels,
                    rgb_mixer,
                    cst,
                    sizing,
                    curves,
                    low_soft,
                    high_soft,
                    color_slice,
                    warper,
                    hsl,
                    key_gain,
                    show_mask,
                    node_mask: None,
                    grade_space: gs,
                    hdr_specular,
                    gpu_capable,
                }))
            }
            "ec.blur.gaussian" => {
                let r = f(e, "blurriness", cx) * cx.px_scale * 0.5;
                let dims = choice(e, "dimensions");
                let (rx, ry) = gaussian_boxes(w, h, if dims == 2 { 0.0 } else { r }, if dims == 1 { 0.0 } else { r });
                FxOp::Gaussian { rx, ry, repeat: b(e, "repeatEdge") }
            }
            "ec.blur.directional" => {
                let len = f(e, "length", cx) * cx.px_scale;
                let dir = (f(e, "direction", cx) as f64).to_radians();
                if !len.is_finite() || len < 0.5 {
                    FxOp::DirectionalBlur { dx: 0.0, dy: 0.0, steps: 0 }
                } else {
                    let steps = (len.ceil() as usize).clamp(2, 64) as u32;
                    FxOp::DirectionalBlur { dx: (dir.sin() as f32) * len, dy: (-dir.cos() as f32) * len, steps }
                }
            }
            "ec.blur.sharpen" => {
                let r = cx.px_scale.max(0.6);
                let (rx, ry) = gaussian_boxes(w, h, r, r);
                FxOp::Unsharp { rx, ry, amount: f(e, "amount", cx) / 50.0, threshold: 0.0 }
            }
            "ec.blur.unsharp" => {
                let r = (f(e, "radius", cx) * cx.px_scale).max(0.1);
                let (rx, ry) = gaussian_boxes(w, h, r, r);
                FxOp::Unsharp { rx, ry, amount: f(e, "amount", cx) / 100.0, threshold: f(e, "threshold", cx) / 255.0 }
            }
            "ec.color.tint" => {
                let (bl, wh) = (color(e, "black", cx), color(e, "white", cx));
                FxOp::Tint { black: [bl[0], bl[1], bl[2]], white: [wh[0], wh[1], wh[2]], amount: f(e, "amount", cx) / 100.0 }
            }
            "ec.color.levels" => {
                if b(e, "noClip") || choice(e, "clipToOutputBlack") == 0 || choice(e, "clipToOutputWhite") == 0 || !ec_levels_channels_identity(e, cx) {
                    return None;
                }
                let ib = f(e, "inBlack", cx);
                let iw = f(e, "inWhite", cx);
                if !ib.is_finite() || !iw.is_finite() || iw <= ib + 1e-4 {
                    return None;
                }
                FxOp::Levels { ib, iw, ob: f(e, "outBlack", cx), ow: f(e, "outWhite", cx), g: 1.0 / f(e, "gamma", cx).max(0.01) }
            }
            "ec.stylize.posterize" => FxOp::Posterize { n: f(e, "level", cx).round().clamp(2.0, 255.0) - 1.0 },
            "ec.distort.mirror" => {
                let img = Image { w, h, px: Vec::new() };
                let c = point(e, "center", cx, &img);
                let ang = (f(e, "angle", cx) as f64).to_radians();
                FxOp::Mirror { cx: c.x as f32, cy: c.y as f32, nx: ang.cos() as f32, ny: ang.sin() as f32 }
            }
            "ec.distort.offset" => {
                let img = Image { w, h, px: Vec::new() };
                let s = point(e, "shift", cx, &img);
                let (dx, dy) = (s.x - w as f64 / 2.0, s.y - h as f64 / 2.0);
                FxOp::Offset { dx: dx.rem_euclid(w.max(1) as f64) as f32, dy: dy.rem_euclid(h.max(1) as f64) as f32, blend: f(e, "blend", cx) / 100.0 }
            }
            _ => return None,
        })
    }

    /// Evaluate `e`, wrapping in [`FxOp::Masked`] if `e` carries active masks.
    pub fn eval_masked(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<FxOp> {
        let mut op = Self::eval(e, cx, w, h)?;
        let masks_on = e.param("masks_on").and_then(|p| p.value.as_bool()).unwrap_or(true);
        if masks_on && let Some(cov) = crate::mask::effect_coverage(&e.masks, cx.t, cx.px_scale, w, h) {
            let mask = MaskCoverage { w: w as u32, h: h as u32, cov: Arc::new(cov) };
            if let FxOp::Lumetri(ref mut lum) = op
                && (lum.hsl.as_ref().is_some_and(|h| h.key_scope == 0) || lum.show_mask > 0 || lum.key_gain < 0.9999)
            {
                lum.node_mask = Some(mask);
                return Some(op);
            }
            Some(FxOp::Masked { inner: Box::new(op), mask })
        } else {
            Some(op)
        }
    }

    fn crop_rect(w: usize, h: usize, l: f32, t: f32, r: f32, b: f32, feather: f32) -> FxOp {
        let (w, h) = (w as f32, h as f32);
        FxOp::Crop { x0: l * w, x1: w * (1.0 - r), y0: t * h, y1: h * (1.0 - b), feather: feather.max(0.0) }
    }

    /// Whether the GPU shader reproduces [`apply`](Self::apply) for this op: every number finite
    /// (the CPU's NaN / infinity behaviour is left to the CPU) and resamples that the CPU does
    /// not pre-filter.
    pub fn gpu_ok(&self) -> bool {
        let fin = |v: &[f32]| v.iter().all(|x| x.is_finite());
        match self {
            FxOp::BrightnessContrast { br, co } => fin(&[*br, *co]),
            FxOp::ProcAmp { br, co, hue, sat } => fin(&[*br, *co, *hue, *sat]),
            FxOp::Tint { black, white, amount } => fin(black) && fin(white) && amount.is_finite(),
            FxOp::BlackWhite | FxOp::HFlip | FxOp::VFlip => true,
            FxOp::ColorBalance { sh, md, hi, .. } => fin(sh) && fin(md) && fin(hi),
            FxOp::ThreeWay { sh, md, hi, range, sat } => fin(sh) && fin(md) && fin(hi) && fin(range) && sat.is_finite(),
            FxOp::LeaveColor { amount, key_hue, tol, soft } => fin(&[*amount, *key_hue, *tol, *soft]),
            FxOp::ChangeToColor { from_hue, to_hue, tol, soft } => fin(&[*from_hue, *to_hue, *tol, *soft]),
            FxOp::ColorPass { key, sim, .. } => fin(key) && sim.is_finite(),
            FxOp::Gamma { g } => g.is_finite(),
            FxOp::Levels { ib, iw, ob, ow, g } => fin(&[*ib, *iw, *ob, *ow, *g]),
            FxOp::Extract { lo, hi, soft, .. } => fin(&[*lo, *hi, *soft]),
            FxOp::Invert { blend, .. } => blend.is_finite(),
            FxOp::Posterize { n } => n.is_finite(),
            FxOp::Gaussian { .. } => true,
            FxOp::DirectionalBlur { dx, dy, .. } => fin(&[*dx, *dy]),
            FxOp::Bokeh { boost, threshold, sides, rot, rr, rings, .. } => fin(&[*boost, *threshold, *rot, *rr]) && *sides <= 8 && *rings <= 12,
            FxOp::Unsharp { amount, threshold, .. } => fin(&[*amount, *threshold]),
            FxOp::Crop { x0, x1, y0, y1, feather } => fin(&[*x0, *x1, *y0, *y1, *feather]),
            FxOp::Resample(r) => {
                r.rect[0] != u32::MAX
                    && r.opacity.is_finite()
                    && r.inv.as_ref().is_none_or(|m| [m.a, m.b, m.c, m.d, m.e, m.f].iter().all(|v| v.is_finite() && v.abs() < 1e7))
            }
            FxOp::Mirror { cx, cy, nx, ny } => fin(&[*cx, *cy, *nx, *ny]),
            FxOp::Offset { dx, dy, blend } => fin(&[*dx, *dy, *blend]),
            FxOp::AscCdl { slope, offset, power, sat } => fin(slope) && fin(offset) && fin(power) && sat.is_finite(),
            FxOp::ChannelMix { m } => m.iter().all(|r| fin(r)),
            FxOp::ColorReplace { sim, target, replace, replace_hsl, .. } => sim.is_finite() && fin(target) && fin(replace) && fin(replace_hsl),
            FxOp::AlphaAdjust { opacity, .. } => opacity.is_finite(),
            FxOp::Vignette { amount, midpoint, roundness, feather, target } => fin(&[*amount, *midpoint, *roundness, *feather]) && fin(target),
            FxOp::VideoLimiter { max, comp, axis, warning_color, .. } => *axis <= 3 && max.is_finite() && comp.is_finite() && fin(warning_color),
            FxOp::Lumetri(op) => {
                let LumetriOp {
                    gains,
                    exposure,
                    contrast,
                    pivot,
                    color_boost,
                    midtone_detail,
                    primary_hue,
                    hl,
                    sh,
                    wh,
                    bl,
                    sat,
                    faded,
                    vib,
                    st,
                    ht,
                    va,
                    vmid,
                    vround,
                    vfeather,
                    aspect,
                    look_k,
                    sharpen,
                    ws,
                    wm,
                    wh_rgb,
                    ls,
                    lm,
                    lh,
                    curves,
                    low_soft,
                    high_soft,
                    color_slice,
                    warper,
                    hsl,
                    key_gain,
                    gpu_capable,
                    ..
                } = op.as_ref();
                *gpu_capable
                    && key_gain.is_finite()
                    && fin(gains)
                    && fin(&[
                        *exposure,
                        *contrast,
                        *pivot,
                        *color_boost,
                        *midtone_detail,
                        *primary_hue,
                        *hl,
                        *sh,
                        *wh,
                        *bl,
                        *sat,
                        *faded,
                        *vib,
                        *look_k,
                        *sharpen,
                        *low_soft,
                        *high_soft,
                    ])
                    && fin(st)
                    && fin(ht)
                    && fin(&[*va, *vmid, *vround, *vfeather, *aspect])
                    && fin(ws)
                    && fin(wm)
                    && fin(wh_rgb)
                    && fin(&[*ls, *lm, *lh])
                    && curves.as_ref().is_none_or(|c| c.texels.len() == LUMETRI_CURVE_LEN * 3)
                    && color_slice.as_ref().is_none_or(|cs| cs.0.iter().all(|b| fin(b)))
                    && warper.as_ref().is_none_or(|cw| cw.texels.len() == CW_LUT_W * CW_LUT_H)
                    && hsl.as_ref().is_none_or(|h| {
                        h.denoise_r <= 3
                            && h.in_out_r <= 3
                            && h.samples_3d.iter().all(|s| fin(s))
                            && fin(&[
                                h.hc,
                                h.hr,
                                h.smin,
                                h.smax,
                                h.lmin,
                                h.lmax,
                                h.soft,
                                h.sat_soft,
                                h.luma_soft,
                                h.rgb_soft,
                                h.clean_black,
                                h.clean_white,
                                h.in_out,
                                h.key_gain,
                                h.htemp,
                                h.htint,
                                h.hsat,
                                h.hshift,
                                h.denoise,
                            ])
                            && fin(&h.rgb_min)
                            && fin(&h.rgb_max)
                    })
            }
            FxOp::ColorDag(dag) => {
                !dag.instrs.is_empty()
                    && (dag.num_slots as usize) <= crate::color_graph::MAX_DAG_SLOTS
                    && dag.instrs.iter().all(|ins| match ins {
                        crate::color_graph::DagInstr::Corrector { op, key_input_gain, key_output_gain, .. } => {
                            key_input_gain.is_finite() && key_output_gain.is_finite() && FxOp::Lumetri(op.clone()).gpu_ok()
                        }
                        crate::color_graph::DagInstr::ParallelCombine { branches, .. } => branches.len() >= 2,
                        crate::color_graph::DagInstr::LayerComposite { branches, .. } => {
                            branches.len() >= 2 && branches.iter().all(|(_, _, _, g)| g.is_finite())
                        }
                        crate::color_graph::DagInstr::SplitChannel { channel, .. } => *channel < 3,
                        crate::color_graph::DagInstr::CombineChannels { .. } => true,
                    })
            }
            FxOp::Masked { inner, mask } => {
                inner.gpu_ok()
                    && mask.w > 0
                    && mask.h > 0
                    && mask.cov.len() == (mask.w as usize).saturating_mul(mask.h as usize)
                    && mask.cov.iter().all(|v| v.is_finite())
            }
        }
    }

    /// The CPU reference.
    pub fn apply(&self, img: &mut Image) {
        if img.w == 0 || img.h == 0 {
            return;
        }
        match self {
            FxOp::BrightnessContrast { br, co } => {
                let (br, co) = (*br, *co);
                img.map_rgb(|c, _, _| {
                    let c = enc(c);
                    dec(c.map(|v| (v - 0.5) * co + 0.5 + br))
                });
            }
            FxOp::ProcAmp { br, co, hue, sat } => {
                let (br, co, hue, sat) = (*br, *co, *hue, *sat);
                img.map_rgb(|c, _, _| {
                    let c = enc(c);
                    let mut hsl = rgb_to_hsl(c[0], c[1], c[2]);
                    hsl[0] = (hsl[0] + hue).rem_euclid(1.0);
                    hsl[1] = (hsl[1] * sat).clamp(0.0, 1.0);
                    let c = hsl_to_rgb(hsl[0], hsl[1], hsl[2]);
                    dec(c.map(|v| (v - 0.5) * co + 0.5 + br))
                });
            }
            FxOp::Tint { black: bl, white: wh, amount } => {
                let amt = *amount;
                img.map_rgb(|c, _, _| {
                    let l = linear_to_srgb(luma709(c[0], c[1], c[2]).max(0.0));
                    let t = [bl[0] + (wh[0] - bl[0]) * l, bl[1] + (wh[1] - bl[1]) * l, bl[2] + (wh[2] - bl[2]) * l];
                    let e = enc(c);
                    dec([e[0] + (t[0] - e[0]) * amt, e[1] + (t[1] - e[1]) * amt, e[2] + (t[2] - e[2]) * amt])
                });
            }
            FxOp::BlackWhite => img.map_rgb(|c, _, _| {
                let l = luma709(c[0], c[1], c[2]);
                [l, l, l]
            }),
            FxOp::ColorBalance { sh, md, hi, preserve } => {
                let preserve = *preserve;
                img.map_rgb(|c, _, _| {
                    let c = enc(c);
                    let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                    let ws = (1.0 - l).powi(2);
                    let wh = l.powi(2);
                    let wm = 1.0 - ws - wh;
                    let mut o = [0.0; 3];
                    for k in 0..3 {
                        o[k] = c[k] + sh[k] * ws + md[k] * wm.max(0.0) + hi[k] * wh;
                    }
                    if preserve {
                        let l2 = 0.2126 * o[0] + 0.7152 * o[1] + 0.0722 * o[2];
                        let d = l - l2;
                        o = o.map(|v| v + d);
                    }
                    dec(o)
                });
            }
            FxOp::ThreeWay { sh, md, hi, range, sat } => {
                if self.is_identity_three_way() {
                    return;
                }
                let (sh, md, hi, range, sat) = (*sh, *md, *hi, *range, *sat);
                let ramp = |l: f32, lo: f32, hi: f32| {
                    let t = ((l - lo) / (hi - lo).max(1e-4)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                };
                img.map_rgb(|c, _, _| {
                    let e = enc(c);
                    let l = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
                    let (mut ws, mut wh) = (1.0 - ramp(l, range[0], range[1]), ramp(l, range[2], range[3]));
                    // overlapping ranges (shadow edge above the highlight edge) share the pixel: weights sum to 1
                    let wm = (1.0 - ws - wh).max(0.0);
                    let total = ws + wm + wh;
                    ws /= total;
                    wh /= total;
                    let wm = wm / total;
                    let lvl = sh[3] * ws + md[3] * wm + hi[3] * wh;
                    let mut o = [0.0; 3];
                    for k in 0..3 {
                        o[k] = e[k] + sh[k] * ws + md[k] * wm + hi[k] * wh + lvl;
                    }
                    let l2 = 0.2126 * o[0] + 0.7152 * o[1] + 0.0722 * o[2];
                    // decoded without the upper clamp: values above 1 (HDR) keep their headroom
                    o.map(|v| filmcraft_color::srgb_to_linear((l2 + (v - l2) * sat).max(0.0)))
                });
            }
            FxOp::LeaveColor { amount, key_hue, tol, soft } => {
                let (amt, kh, tol, soft) = (*amount, *key_hue, *tol, *soft);
                img.map_rgb(|c, _, _| {
                    let ec = enc(c);
                    let h = rgb_to_hsl(ec[0], ec[1], ec[2])[0];
                    let d = (h - kh).abs().min(1.0 - (h - kh).abs()) * 2.0;
                    let keep = 1.0 - ((d - tol) / soft).clamp(0.0, 1.0);
                    let l = luma709(c[0], c[1], c[2]);
                    let k = amt * (1.0 - keep);
                    [c[0] + (l - c[0]) * k, c[1] + (l - c[1]) * k, c[2] + (l - c[2]) * k]
                });
            }
            FxOp::ChangeToColor { from_hue: fh, to_hue: th, tol, soft } => {
                let (fh, th, tol, soft) = (*fh, *th, *tol, *soft);
                img.map_rgb(|c, _, _| {
                    let ec = enc(c);
                    let mut hsl = rgb_to_hsl(ec[0], ec[1], ec[2]);
                    let d = (hsl[0] - fh).abs().min(1.0 - (hsl[0] - fh).abs());
                    let w = 1.0 - ((d - tol) / soft).clamp(0.0, 1.0);
                    hsl[0] = (hsl[0] + (th - fh) * w).rem_euclid(1.0);
                    dec(hsl_to_rgb(hsl[0], hsl[1], hsl[2]))
                });
            }
            FxOp::ColorPass { key, sim, reverse } => {
                let (sim, rev) = (*sim, *reverse);
                img.map_rgb(|c, _, _| {
                    let ec = enc(c);
                    let d = ((ec[0] - key[0]).powi(2) + (ec[1] - key[1]).powi(2) + (ec[2] - key[2]).powi(2)).sqrt();
                    let pass = (d <= sim * 1.2) != rev;
                    if pass {
                        c
                    } else {
                        let l = luma709(c[0], c[1], c[2]);
                        [l, l, l]
                    }
                });
            }
            FxOp::Gamma { g } => {
                let g = *g;
                img.map_rgb(|c, _, _| dec(enc(c).map(|v| v.max(0.0).powf(g))));
            }
            FxOp::Levels { ib, iw, ob, ow, g } => {
                let (ib, iw, ob, ow, g) = (*ib, *iw, *ob, *ow, *g);
                img.map_rgb(|c, _, _| dec(enc(c).map(|v| ob + (((v - ib) / (iw - ib)).clamp(0.0, 1.0)).powf(g) * (ow - ob))));
            }
            FxOp::Extract { lo, hi, soft, invert } => {
                let (lo, hi, soft, inv) = (*lo, *hi, *soft, *invert);
                img.map_rgb(|c, _, _| {
                    let l = linear_to_srgb(luma709(c[0], c[1], c[2]).max(0.0));
                    let inside = if soft <= 0.0 {
                        // Match the feathered function's limiting value at the cutoffs.
                        if l > lo && l < hi { 1.0 } else { 0.0 }
                    } else {
                        ((l - lo) / soft).clamp(0.0, 1.0).min(((hi - l) / soft).clamp(0.0, 1.0))
                    };
                    let v = if inv { 1.0 - inside } else { inside };
                    [v, v, v]
                });
            }
            FxOp::Invert { channel, blend } => {
                let (ch, blend) = (*channel, *blend);
                if ch == 4 {
                    img.px.par_chunks_mut(4).for_each(|p| {
                        let a = p[3];
                        let na = 1.0 - a;
                        let k = if a > 1e-6 { na / a } else { 0.0 };
                        for c in &mut p[..3] {
                            *c *= k;
                        }
                        p[3] = na * (1.0 - blend) + a * blend;
                    });
                } else {
                    img.map_rgb(|c, _, _| {
                        let ec = enc(c);
                        let mut o = ec;
                        for k in 0..3 {
                            if ch == 0 || ch as usize == k + 1 {
                                o[k] = 1.0 - ec[k];
                            }
                        }
                        dec([o[0] + (ec[0] - o[0]) * blend, o[1] + (ec[1] - o[1]) * blend, o[2] + (ec[2] - o[2]) * blend])
                    });
                }
            }
            FxOp::Posterize { n } => {
                let n = *n;
                img.map_rgb(|c, _, _| dec(enc(c).map(|v| (v * n).round() / n)));
            }
            FxOp::Gaussian { rx, ry, repeat } => crate::effects::box_blur(img, rx, ry, *repeat),
            FxOp::DirectionalBlur { dx, dy, steps } => crate::effects::directional_taps(img, *dx, *dy, *steps as usize),
            FxOp::Bokeh { levels, boost, threshold, sides, rot, rr, rings } => {
                if *rings > 0 {
                    crate::vfx::bokeh(img, *levels, *boost, *threshold, *sides, *rot, *rr, *rings);
                }
            }
            FxOp::Unsharp { rx, ry, amount, threshold } => crate::effects::unsharp_boxes(img, rx, ry, *amount, *threshold),
            FxOp::Crop { x0, x1, y0, y1, feather } => crate::effects::crop_px(img, *x0, *x1, *y0, *y1, *feather),
            FxOp::Resample(r) => {
                let out = match (&r.inv, r.rect[0]) {
                    (Some(m), u32::MAX) => img.transformed(img.w, img.h, m), // mip path (CPU only)
                    (Some(inv), _) => resample_cpu(img, inv, r.rect),
                    (None, _) => Image::new(img.w, img.h),
                };
                *img = out;
                img.scale_alpha(r.opacity);
            }
            FxOp::HFlip => {
                let w = img.w;
                img.px.par_chunks_mut(w * 4).for_each(|row| {
                    for x in 0..w / 2 {
                        for k in 0..4 {
                            row.swap(x * 4 + k, (w - 1 - x) * 4 + k);
                        }
                    }
                });
            }
            FxOp::VFlip => {
                let (w, h) = (img.w, img.h);
                for y in 0..h / 2 {
                    let (a, bb) = img.px.split_at_mut((h - 1 - y) * w * 4);
                    a[y * w * 4..(y + 1) * w * 4].swap_with_slice(&mut bb[..w * 4]);
                }
            }
            FxOp::Mirror { cx, cy, nx, ny } => {
                let (c, n) = ((*cx as f64, *cy as f64), (*nx as f64, *ny as f64));
                let src = img.clone();
                let w = img.w;
                img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..w {
                        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                        let d = (px - c.0) * n.0 + (py - c.1) * n.1;
                        if d > 0.0 {
                            let (rx, ry) = (px - 2.0 * d * n.0, py - 2.0 * d * n.1);
                            row[x * 4..x * 4 + 4].copy_from_slice(&src.sample_bilinear(rx as f32, ry as f32));
                        }
                    }
                });
            }
            FxOp::Offset { dx, dy, blend } => {
                let (dx, dy, blend) = (*dx as f64, *dy as f64, *blend);
                let src = img.clone();
                let (w, h) = (img.w as f64, img.h as f64);
                let wi = img.w;
                img.px.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..wi {
                        let u = (x as f64 + 0.5 - dx).rem_euclid(w);
                        let v = (y as f64 + 0.5 - dy).rem_euclid(h);
                        let p = src.sample_bilinear_clamped(u as f32, v as f32);
                        let o = src.get(x, y);
                        for k in 0..4 {
                            row[x * 4 + k] = p[k] + (o[k] - p[k]) * blend;
                        }
                    }
                });
            }
            FxOp::AscCdl { slope, offset, power, sat } => {
                let (s, o, pw, sat) = (*slope, *offset, *power, *sat);
                img.map_rgb(|c, _, _| dec(crate::vfx::cdl(enc(c), s, o, pw, sat)));
            }
            FxOp::ChannelMix { m } => {
                let m = *m;
                img.map_rgb(|c, _, _| {
                    let v = enc(c);
                    dec([0, 1, 2].map(|k| (m[k][0] * v[0] + m[k][1] * v[1] + m[k][2] * v[2] + m[k][3]).clamp(0.0, 1.0)))
                });
            }
            FxOp::ColorReplace { sim, solid, target: t, replace: r, replace_hsl: rh } => {
                let (sim, solid) = (*sim, *solid);
                img.map_rgb(|c, _, _| {
                    let v = enc(c);
                    let d = ((v[0] - t[0]).powi(2) + (v[1] - t[1]).powi(2) + (v[2] - t[2]).powi(2)).sqrt();
                    let k = 1.0 - crate::vfx::smoothstep(sim * 0.85, sim.max(1e-4), d);
                    if k <= 0.0 {
                        return c;
                    }
                    let target = if solid {
                        [r[0], r[1], r[2]]
                    } else {
                        let l = rgb_to_hsl(v[0], v[1], v[2])[2];
                        hsl_to_rgb(rh[0], rh[1], l)
                    };
                    dec(crate::vfx::lerp3(v, target, k))
                });
            }
            FxOp::AlphaAdjust { opacity, ignore, invert, mask_only } => {
                let (op, ignore, invert, mask_only) = (*opacity, *ignore, *invert, *mask_only);
                img.px.par_chunks_mut(4).for_each(|p| {
                    let c = Image::unpremul([p[0], p[1], p[2], p[3]]);
                    let mut a = if ignore { 1.0 } else { p[3] };
                    if invert {
                        a = 1.0 - a;
                    }
                    a = (a * op).clamp(0.0, 1.0);
                    if mask_only {
                        let g = filmcraft_color::srgb_to_linear(a);
                        p.copy_from_slice(&[g, g, g, 1.0]);
                    } else {
                        p.copy_from_slice(&[c[0] * a, c[1] * a, c[2] * a, a]);
                    }
                });
            }
            FxOp::Vignette { amount, midpoint, roundness, feather, target } => {
                let (amt, mid, round, feather, target) = (*amount, *midpoint, *roundness, *feather, *target);
                if !amt.is_finite() || !mid.is_finite() || !round.is_finite() || !feather.is_finite() || !target.iter().all(|c| c.is_finite()) {
                    return;
                }
                if amt.abs() < 1e-5 {
                    return;
                }
                let (w, h) = (img.w as f32, img.h as f32);
                let aspect = w / h;
                img.map_rgb(|c, x, y| {
                    let mut nx = (x as f32 + 0.5) / w * 2.0 - 1.0;
                    let ny = (y as f32 + 0.5) / h * 2.0 - 1.0;
                    // roundness 100 = circle; 0 = an ellipse following the frame; −100 = squarer
                    if round > 0.0 {
                        nx *= 1.0 + (aspect - 1.0) * round;
                    }
                    let p = if round < 0.0 { 2.0 + (-round) * 6.0 } else { 2.0 };
                    // 0 at the centre, 1 at the corners
                    let d = (nx.abs().powf(p) + ny.abs().powf(p)).powf(1.0 / p) / 2f32.powf(1.0 / p);
                    let edge = crate::vfx::smoothstep(mid - feather * 0.5, mid + feather * 0.5, d);
                    dec(crate::vfx::lerp3(enc(c), target, edge * amt.abs()))
                });
            }
            FxOp::VideoLimiter { max, comp, axis, warn, warning_color } => {
                let (max, comp, axis, warn, wc) = (*max, *comp, *axis, *warn, *warning_color);
                if !max.is_finite() || !comp.is_finite() || !wc.iter().all(|c| c.is_finite()) {
                    return;
                }
                img.map_rgb(|c, _, _| {
                    let v = [linear_to_enc(c[0]), linear_to_enc(c[1]), linear_to_enc(c[2])];
                    let out = limit(v, max, comp, axis);
                    if warn && v.iter().zip(&out).any(|(a, b)| (a - b).abs() > 1e-4) {
                        return wc;
                    }
                    out.map(enc_to_linear)
                });
            }
            FxOp::Lumetri(op) => {
                let LumetriOp {
                    input_lut,
                    look_lut,
                    gains,
                    exposure,
                    contrast,
                    pivot,
                    color_boost,
                    midtone_detail,
                    midtone_rx,
                    midtone_ry,
                    primary_hue,
                    hl,
                    sh,
                    wh,
                    bl,
                    sat,
                    creative_on,
                    faded,
                    vib,
                    st,
                    ht,
                    vignette_on,
                    va,
                    vmid,
                    vround,
                    vfeather,
                    aspect,
                    look,
                    look_k,
                    sharpen,
                    sharpen_rx,
                    sharpen_ry,
                    wheels_on,
                    grade_model,
                    ws,
                    wm,
                    wh_rgb,
                    wo,
                    ls,
                    lm,
                    lh,
                    lo,
                    log_wheels,
                    hdr_wheels,
                    rgb_mixer,
                    cst,
                    sizing,
                    curves,
                    low_soft,
                    high_soft,
                    color_slice,
                    warper,
                    hsl,
                    key_gain,
                    show_mask,
                    node_mask,
                    grade_space,
                    hdr_specular,
                    ..
                } = op.as_ref();
                let (input_lut, look_lut) = (input_lut.as_deref(), look_lut.as_deref());
                let gains = *gains;
                let (exposure, contrast, pivot, color_boost, midtone_detail, primary_hue, hl, sh, wh, bl, sat) =
                    (*exposure, *contrast, *pivot, *color_boost, *midtone_detail, *primary_hue, *hl, *sh, *wh, *bl, *sat);
                let (creative_on, faded, vib, st, ht) = (*creative_on, *faded, *vib, *st, *ht);
                let (vignette_on, va, vmid, vround, vfeather, aspect) = (*vignette_on, *va, *vmid, *vround, *vfeather, *aspect);
                let (look, look_k, sharpen) = (*look, *look_k, *sharpen);
                let (wheels_on, grade_model, ws, wm, wh_rgb, wo, ls, lm, lh, lo) = (*wheels_on, *grade_model, *ws, *wm, *wh_rgb, *wo, *ls, *lm, *lh, *lo);
                let (log_wheels, hdr_wheels, rgb_mixer, cst, sizing) =
                    (log_wheels.as_deref(), hdr_wheels.as_deref(), rgb_mixer.as_deref(), cst.as_deref(), sizing.as_deref());
                let lift_k = [ws[0] * 0.3 + ls * 0.3, ws[1] * 0.3 + ls * 0.3, ws[2] * 0.3 + ls * 0.3];
                let gamma_k = [wm[0] * 0.3 + lm * 0.3, wm[1] * 0.3 + lm * 0.3, wm[2] * 0.3 + lm * 0.3];
                let gain_k = [wh_rgb[0] * 0.5 + lh * 0.5, wh_rgb[1] * 0.5 + lh * 0.5, wh_rgb[2] * 0.5 + lh * 0.5];
                let offset_k = [wo[0] * 0.3 + lo * 0.3, wo[1] * 0.3 + lo * 0.3, wo[2] * 0.3 + lo * 0.3];
                let wheels_active = wheels_on && lift_k.iter().chain(&gamma_k).chain(&gain_k).chain(&offset_k).any(|val| val.abs() > 1e-5);
                let (low_soft, high_soft) = (*low_soft, *high_soft);
                let (key_gain, show_mask) = (*key_gain, *show_mask);
                let is_node_key = hsl.as_ref().is_some_and(|h| h.key_scope == 0);
                let is_legacy_sec = hsl.as_ref().is_some_and(|h| h.key_scope == 1);
                let window_cov = node_mask.as_ref().filter(|m| m.cov.len() == img.w.saturating_mul(img.h)).map(|m| m.cov.as_slice());
                let node_highlight = if !is_legacy_sec && (is_node_key || window_cov.is_some()) { show_mask } else { 0 };
                let need_outer_blend = is_node_key || window_cov.is_some() || key_gain < 0.9999 || node_highlight > 0;
                let orig_img = need_outer_blend.then(|| img.clone());
                let gs_sdr = filmcraft_color::GradeSpace::Sdr;
                let pre_qual_matte = if is_node_key && let (Some(q), Some(orig)) = (hsl.as_deref(), orig_img.as_ref()) {
                    Some(crate::effects::compute_qualifier_matte(orig, q, &gs_sdr))
                } else {
                    None
                };
                let (w, h) = (img.w as f32, img.h as f32);
                let (grade_space, hdr_specular) = (*grade_space, *hdr_specular);
                let hdr = grade_space.is_hdr();
                img.map_rgb(|c, x, y| {
                    let c = match input_lut {
                        Some(l) => dec(l.apply(enc(c))),
                        None => c,
                    };
                    // white balance + exposure in linear light
                    let lin = [c[0] * gains[0] * exposure, c[1] * gains[1] * exposure, c[2] * gains[2] * exposure];
                    let mut v = grade_space.encode(lin);
                    // whites / blacks: endpoints
                    let b0 = -bl * 0.15;
                    let w0 = 1.0 - wh * 0.15;
                    v = v.map(|q| (q - b0) / (w0 - b0).max(1e-3));
                    // highlights / shadows: luma-weighted lift/compress, hue preserving
                    let l = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
                    let ws_term = (1.0 - l).clamp(0.0, 1.0).powi(3);
                    let whl = l.clamp(0.0, 1.0).powi(3);
                    let nl = (l + sh * 0.35 * ws_term + hl * 0.35 * whl).max(0.0);
                    // #2094: below LIFT_LUMA_FLOOR the hue-preserving scale is held and the rest is added equally
                    let (k, add) = lift_scale(l, nl);
                    v = v.map(|q| q * k + add);
                    // contrast: smooth S-curve around pivot
                    if contrast.abs() > 1e-4 {
                        v = v.map(|q| if hdr && q > 1.0 { q } else { apply_contrast_pivot(q, contrast, pivot) });
                    }
                    if hdr && hdr_specular.abs() > 1e-4 {
                        v = v.map(|q| if q > 1.0 { 1.0 + (q - 1.0) * (1.0 + hdr_specular).max(0.0) } else { q });
                    }
                    // faded film: lift blacks and compress
                    if faded > 0.0 {
                        v = v.map(|q| q * (1.0 - 0.25 * faded) + 0.12 * faded);
                    }
                    // split tone
                    if creative_on {
                        let l2 = (0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]).clamp(0.0, 1.0);
                        for k in 0..3 {
                            v[k] += (st[k] - 0.5) * 0.3 * (1.0 - l2) + (ht[k] - 0.5) * 0.3 * l2;
                        }
                    }
                    // primary hue rotation
                    if primary_hue.abs() > 1e-5 {
                        let u = v.map(|q| q.clamp(0.0, 1.0));
                        let mut hh = rgb_to_hsl(u[0], u[1], u[2]);
                        hh[0] = (hh[0] + primary_hue).rem_euclid(1.0);
                        v = hsl_to_rgb(hh[0], hh[1], hh[2]);
                    }
                    // saturation, vibrance & color boost
                    v = apply_sat_vib_boost(v, sat, vib, color_boost);
                    // vignette
                    if vignette_on && va.abs() > 1e-4 {
                        let nx = (x as f32 / w - 0.5) * 2.0 * lumetri_vignette_aspect(aspect, vround);
                        let ny = (y as f32 / h - 0.5) * 2.0;
                        let d = (nx * nx + ny * ny).sqrt() / std::f32::consts::SQRT_2;
                        let edge = ((d - vmid * 0.9) / (vfeather.max(0.01) * 0.9)).clamp(0.0, 1.0);
                        let e2 = edge * edge * (3.0 - 2.0 * edge);
                        let k = 1.0 + va * 0.2 * e2;
                        v = v.map(|q| if va < 0.0 { q * k.max(0.0) } else { q + (1.0 - q) * (k - 1.0) });
                    }
                    if let Some(l) = look_lut {
                        if look_k.abs() > 1e-5 {
                            let mut v2 = enc(dec(v));
                            let lk = l.apply(v2);
                            v2 = [v2[0] + (lk[0] - v2[0]) * look_k, v2[1] + (lk[1] - v2[1]) * look_k, v2[2] + (lk[2] - v2[2]) * look_k];
                            v = v2;
                        }
                    } else if look > 0 && look <= 8 && look_k.abs() > 1e-5 {
                        let mut v2 = enc(dec(v));
                        let lk = crate::effects::apply_look(look, v2);
                        v2 = [v2[0] + (lk[0] - v2[0]) * look_k, v2[1] + (lk[1] - v2[1]) * look_k, v2[2] + (lk[2] - v2[2]) * look_k];
                        v = v2;
                    }
                    v = apply_phase2_wheels_and_mixer(v, wheels_active, grade_model, lift_k, gamma_k, gain_k, offset_k, log_wheels, hdr_wheels, rgb_mixer);
                    let has_curves = curves.as_ref().is_some_and(|ct| ct.active_mask != 0) || low_soft > 1e-5 || high_soft > 1e-5;
                    if has_curves {
                        v = enc(dec(v));
                        if let Some(ct) = curves
                            && ct.active_mask != 0
                        {
                            let sample_tbl = |row: usize, ch: usize, x: f32| {
                                let p = x.clamp(0.0, 1.0) * (LUMETRI_CURVE_LEN - 1) as f32;
                                let i = p as usize;
                                let j = (i + 1).min(LUMETRI_CURVE_LEN - 1);
                                let base = row * LUMETRI_CURVE_LEN;
                                let a = ct.texels[base + i][ch];
                                let b = ct.texels[base + j][ch];
                                a + (b - a) * (p - i as f32)
                            };
                            let m = ct.active_mask;
                            if m & 1 != 0 {
                                v = v.map(|q| sample_tbl(0, 0, q));
                            }
                            if m & 2 != 0 {
                                v[0] = sample_tbl(0, 1, v[0]);
                            }
                            if m & 4 != 0 {
                                v[1] = sample_tbl(0, 2, v[1]);
                            }
                            if m & 8 != 0 {
                                v[2] = sample_tbl(0, 3, v[2]);
                            }
                            if low_soft > 1e-5 || high_soft > 1e-5 {
                                v = v.map(|q| apply_soft_clip(q, low_soft, high_soft));
                            }
                            if m & 0x3F0 != 0 {
                                let u = v.map(|q| q.clamp(0.0, 1.0));
                                let mut h = rgb_to_hsl(u[0], u[1], u[2]);
                                let (h0, s0, l0) = (h[0], h[1], h[2]);
                                if m & 16 != 0 {
                                    h[0] = (h[0] + (sample_tbl(1, 0, h0) - 0.5)).rem_euclid(1.0);
                                }
                                let mut sm = 1.0;
                                if m & 32 != 0 {
                                    sm *= sample_tbl(1, 1, h0) * 2.0;
                                }
                                if m & 64 != 0 {
                                    sm *= sample_tbl(1, 2, l0) * 2.0;
                                }
                                if m & 128 != 0 {
                                    sm *= sample_tbl(1, 3, s0) * 2.0;
                                }
                                h[1] = (h[1] * sm).clamp(0.0, 1.0);
                                if m & 256 != 0 {
                                    h[2] = (h[2] + (sample_tbl(2, 0, h0) - 0.5) * 0.5).clamp(0.0, 1.0);
                                }
                                if m & 512 != 0 {
                                    h[2] = (h[2] + (sample_tbl(2, 1, s0) - 0.5) * 0.5).clamp(0.0, 1.0);
                                }
                                v = hsl_to_rgb(h[0], h[1], h[2]);
                            }
                        } else if low_soft > 1e-5 || high_soft > 1e-5 {
                            v = v.map(|q| apply_soft_clip(q, low_soft, high_soft));
                        }
                    }
                    if let Some(cs) = color_slice {
                        let u = enc(dec(v)).map(|q| q.clamp(0.0, 1.0));
                        v = apply_color_slice_unit(u, cs);
                    }
                    if let Some(cw) = warper {
                        let u = enc(dec(v)).map(|q| q.clamp(0.0, 1.0));
                        v = apply_color_warper_unit(u, cw);
                    }
                    if let Some(c) = cst {
                        v = apply_cst_unit(enc(dec(v)), c);
                    }
                    grade_space.decode(v)
                });
                if let Some(sz) = sizing {
                    sz.apply_image(img);
                }
                if is_node_key
                    && let Some(hsl) = hsl.as_deref()
                    && (hsl.htemp.abs() > 1e-4 || hsl.htint.abs() > 1e-4 || (hsl.hsat - 1.0).abs() > 1e-4 || hsl.hshift.abs() > 1e-4)
                {
                    img.map_rgb(|c, _, _| {
                        let u = enc(c).map(|q| q.clamp(0.0, 1.0));
                        dec(crate::effects::apply_hsl_secondary_color_unit(u, hsl))
                    });
                } else if !is_node_key && let Some(hsl) = hsl.as_deref() {
                    let mask = crate::effects::compute_qualifier_matte(img, hsl, &gs_sdr);
                    let iw = img.w;
                    img.map_rgb(|c, x, y| {
                        let m = mask.get(y * iw + x).copied().unwrap_or(0.0);
                        let u = enc(c).map(|q| q.clamp(0.0, 1.0));
                        let out = if hsl.show_mask > 0 {
                            crate::effects::apply_highlight_unit(u, m, hsl.show_mask)
                        } else {
                            let c2 = crate::effects::apply_hsl_secondary_color_unit(u, hsl);
                            [u[0] + (c2[0] - u[0]) * m, u[1] + (c2[1] - u[1]) * m, u[2] + (c2[2] - u[2]) * m]
                        };
                        dec(out)
                    });
                }
                if midtone_detail.abs() > 1e-3 && !midtone_rx.is_empty() {
                    midtone_detail_boxes(img, midtone_rx, midtone_ry, midtone_detail);
                }
                if sharpen.abs() > 1e-3 && !sharpen_rx.is_empty() {
                    crate::effects::unsharp_boxes(img, sharpen_rx, sharpen_ry, sharpen.max(-1.0), 0.0);
                }
                if let Some(orig) = orig_img.as_ref() {
                    crate::effects::apply_node_key_output(img, orig, pre_qual_matte.as_deref(), window_cov, node_highlight, key_gain, &gs_sdr);
                }
            }
            FxOp::ColorDag(dag) => {
                crate::color_graph::apply_color_dag_cpu(dag, img);
            }
            FxOp::Masked { inner, mask } => {
                if mask.cov.len() == img.w.saturating_mul(img.h) {
                    let orig = img.clone();
                    inner.apply(img);
                    crate::mask::mix(img, &orig, &mask.cov);
                }
            }
        }
    }
}

/// Horizontal scale of the Color Grading vignette's coordinates for a Roundness of `vround`
/// (−1 … 1) on a frame `aspect` wide. At 0 the vignette is an ellipse that follows the
/// frame; positive values move it towards a circle (1 = a true circle), negative values
/// stretch it into a wider oval. Non-finite or out-of-range input never yields NaN.
pub fn lumetri_vignette_aspect(aspect: f32, vround: f32) -> f32 {
    if !aspect.is_finite() || aspect <= 0.0 || !vround.is_finite() {
        return 1.0;
    }
    aspect.powf(vround.clamp(-1.0, 1.0))
}

/// sRGB encoding that keeps values above 1 (super-whites) instead of clamping them.
#[inline]
pub(crate) fn linear_to_enc_pub(v: f32) -> f32 {
    linear_to_enc(v)
}
#[inline]
pub(crate) fn enc_to_linear_pub(v: f32) -> f32 {
    enc_to_linear(v)
}

#[inline]
fn linear_to_enc(v: f32) -> f32 {
    if v <= 1.0 { filmcraft_color::linear_to_srgb(v.max(0.0)) } else { 1.0 + (v - 1.0) / 2.4 }
}
#[inline]
fn enc_to_linear(v: f32) -> f32 {
    if v <= 1.0 { filmcraft_color::srgb_to_linear(v.max(0.0)) } else { 1.0 + (v - 1.0) * 2.4 }
}

#[inline]
fn knee(v: f32, max: f32, comp: f32) -> f32 {
    let k = max * (1.0 - comp);
    if comp <= 0.0 || v <= k {
        return v.min(max);
    }
    let r = max - k;
    k + r * (1.0 - (-(v - k) / r).exp())
}

pub(crate) fn limit(v: [f32; 3], max: f32, comp: f32, axis: u32) -> [f32; 3] {
    let m = filmcraft_color::Matrix::Bt709;
    let ycc = filmcraft_color::rgb_to_ycbcr(v[0], v[1], v[2], m);
    let (kr, kb) = (0.2126f32, 0.0722f32);
    let to_rgb = |y: f32, cb: f32, cr: f32| {
        let r = y + 2.0 * (1.0 - kr) * cr;
        let b = y + 2.0 * (1.0 - kb) * cb;
        let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
        [r, g, b]
    };
    let (mut y, mut cb, mut cr) = (ycc[0], ycc[1], ycc[2]);
    if axis != 1 {
        y = knee(y.max(0.0), max, comp);
    }
    if axis != 0 {
        // scale chroma so every channel fits 0…max (the gamut of legal R′G′B′)
        let rgb = to_rgb(y, cb, cr);
        let mut s = 1.0f32;
        for &ch in &rgb {
            let d = ch - y;
            if ch > max && d > 1e-6 {
                s = s.min(((max - y) / d).max(0.0));
            }
            if ch < 0.0 && d < -1e-6 {
                s = s.min((y / -d).max(0.0));
            }
        }
        if axis == 3 && s < 1.0 - 1e-4 {
            // smart limit: soft chroma compression rather than a hard cut
            s = knee(s, 1.0, comp.max(0.03)).min(1.0);
        }
        cb *= s;
        cr *= s;
    }
    to_rgb(y, cb, cr).map(|q| q.clamp(0.0, max))
}

/// The body of `Image::transformed` (no mip path) for a precomputed inverse and rectangle.
fn resample_cpu(src: &Image, inv: &Affine, rect: [u32; 4]) -> Image {
    let (w, h) = (src.w, src.h);
    let mut out = Image::new(w, h);
    let [x0, x1, y0, y1] = rect.map(|v| v as usize);
    let (x1, y1) = (x1.min(w), y1.min(h));
    let (x0, y0) = (x0.min(x1), y0.min(y1));
    let axis_aligned = inv.b == 0.0 && inv.c == 0.0;
    out.px.par_chunks_mut(w * 4).enumerate().skip(y0).take(y1 - y0).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in x0..x1 {
            let px = x as f64 + 0.5;
            let (u, v) =
                if axis_aligned { (inv.a * px + inv.e, inv.d * py + inv.f) } else { (inv.a * px + inv.c * py + inv.e, inv.b * px + inv.d * py + inv.f) };
            if u < -1.0 || v < -1.0 || u > w as f64 + 1.0 || v > h as f64 + 1.0 {
                continue;
            }
            row[x * 4..x * 4 + 4].copy_from_slice(&src.sample_bilinear(u as f32, v as f32));
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{ParamValue, find_effect};
    use filmcraft_time::Tick;

    #[test]
    fn extract_zero_softness_has_no_fractional_edge() {
        let cx = FxCtx {
            t: Tick::ZERO,
            px_scale: 1.0,
            seconds: 0.0,
            timecode: "",
            clip_name: "",
            project: None,
            env: None,
            frame_rate: 24.0,
            working: filmcraft_color::WorkingSpace::Rec709,
        };
        // Both sides of each cutoff, including samples inside the former artificial fade.
        let lo = 90.0 / 255.0;
        let hi = 160.0 / 255.0;
        let samples = [lo - 0.00005, lo + 0.00005, hi - 0.00005, hi + 0.00005];
        for softness in [0.0, -1.0] {
            for invert in [false, true] {
                for alpha in [1.0, 0.5] {
                    let mut e = find_effect("extract").unwrap().instance();
                    for (key, value) in [("black", 90.0), ("white", 160.0), ("softness", softness)] {
                        e.params.get_mut(key).unwrap().value = ParamValue::Float(value);
                    }
                    e.params.get_mut("invert").unwrap().value = ParamValue::Bool(invert);
                    let mut img = Image {
                        w: 4,
                        h: 1,
                        px: samples
                            .iter()
                            .flat_map(|v| {
                                let c = filmcraft_color::srgb_to_linear(*v) * alpha;
                                [c, c, c, alpha]
                            })
                            .collect(),
                    };
                    FxOp::eval(&e, &cx, 4, 1).unwrap().apply(&mut img);
                    for (pixel, inside) in img.px.as_chunks::<4>().0.iter().zip([false, true, true, false]) {
                        let value = if inside != invert { alpha } else { 0.0 };
                        assert_eq!(*pixel, [value, value, value, alpha], "softness={softness}, invert={invert}");
                    }
                }
            }
        }
    }

    /// Transform with its own shutter angle blurs the motion on the CPU
    /// (`vfx::distort::transform_fx`). The GPU op has no such blur, so that effect must leave
    /// the GPU chain instead of silently losing its blur in the Program monitor and GPU exports.
    #[test]
    fn transform_with_its_own_shutter_angle_stays_on_the_cpu() {
        let cx = FxCtx {
            t: Tick::ZERO,
            px_scale: 1.0,
            seconds: 0.0,
            timecode: "",
            clip_name: "",
            project: None,
            env: None,
            frame_rate: 24.0,
            working: filmcraft_color::WorkingSpace::Rec709,
        };
        let on_gpu = |e: &EffectInstance| FxOp::eval(e, &cx, 64, 36).is_some_and(|op| op.gpu_ok());
        let mut e = find_effect("transform").unwrap().instance();
        e.params.get_mut("position").unwrap().value = ParamValue::Vec2(filmcraft_geom::Vec2::new(40.0, 20.0));
        assert!(on_gpu(&e), "a plain transform runs on the GPU");
        e.params.get_mut("shutter_override").unwrap().value = ParamValue::Bool(true);
        assert!(!on_gpu(&e), "the default 180° shutter blurs the motion: CPU");
        e.params.get_mut("shutter_angle").unwrap().value = ParamValue::Float(0.0);
        assert!(on_gpu(&e), "a closed shutter blurs nothing");
        e.params.get_mut("shutter_override").unwrap().value = ParamValue::Bool(false);
        e.params.get_mut("shutter_angle").unwrap().value = ParamValue::Float(360.0);
        assert!(on_gpu(&e), "the angle counts only with the override");
    }

    #[test]
    fn ec_gpu_badge_matches_fxop_eval() {
        let cx = FxCtx {
            t: Tick::ZERO,
            px_scale: 1.0,
            seconds: 0.0,
            timecode: "",
            clip_name: "",
            project: None,
            env: None,
            frame_rate: 24.0,
            working: filmcraft_color::WorkingSpace::Rec709,
        };
        for d in filmcraft_project::effect_defs().iter().filter(|d| d.id.starts_with("ec.")) {
            let inst = d.instance();
            let has_gpu = FxOp::eval(&inst, &cx, 64, 36).is_some_and(|op| op.gpu_ok());
            assert_eq!(d.accelerated, has_gpu, "{} accelerated badge ({}) must match FxOp::eval ({has_gpu})", d.id, d.accelerated);
            if d.accelerated {
                assert!(GPU_EFFECTS.contains(&d.id), "{} must be listed in gpufx::GPU_EFFECTS so plan.rs uses the GPU stage", d.id);
            }
        }
    }

    #[test]
    fn ec_gpu_ops_match_cpu_semantics_and_fallback() {
        let cx = FxCtx {
            t: Tick::ZERO,
            px_scale: 1.0,
            seconds: 0.0,
            timecode: "",
            clip_name: "",
            project: None,
            env: None,
            frame_rate: 24.0,
            working: filmcraft_color::WorkingSpace::Rec709,
        };
        // Directional blur: length without extra *2 multiplier
        let mut dir = find_effect("ec.blur.directional").unwrap().instance();
        dir.params.get_mut("length").unwrap().value = ParamValue::Float(10.0);
        dir.params.get_mut("direction").unwrap().value = ParamValue::Float(90.0);
        match FxOp::eval(&dir, &cx, 64, 36).unwrap() {
            FxOp::DirectionalBlur { dx, dy, .. } => {
                assert!((dx - 10.0).abs() < 1e-4 && dy.abs() < 1e-4, "expected dx=10, dy=0, got ({dx}, {dy})");
            }
            other => panic!("unexpected op: {other:?}"),
        }
        // Sharpen: amount / 50.0
        let mut sh = find_effect("ec.blur.sharpen").unwrap().instance();
        sh.params.get_mut("amount").unwrap().value = ParamValue::Float(25.0);
        match FxOp::eval(&sh, &cx, 64, 36).unwrap() {
            FxOp::Unsharp { amount, .. } => assert!((amount - 0.5).abs() < 1e-6),
            other => panic!("unexpected op: {other:?}"),
        }
        // Posterize: rounded & clamped level - 1.0
        let mut post = find_effect("ec.stylize.posterize").unwrap().instance();
        post.params.get_mut("level").unwrap().value = ParamValue::Float(4.6);
        match FxOp::eval(&post, &cx, 64, 36).unwrap() {
            FxOp::Posterize { n } => assert!((n - 4.0).abs() < 1e-6),
            other => panic!("unexpected op: {other:?}"),
        }
        // Levels: per-channel edit or unclipped mode falls back to CPU (None)
        let mut lvl = find_effect("ec.color.levels").unwrap().instance();
        assert!(FxOp::eval(&lvl, &cx, 64, 36).is_some());
        lvl.params.get_mut("redInBlack").unwrap().value = ParamValue::Float(0.2);
        assert!(FxOp::eval(&lvl, &cx, 64, 36).is_none());
        let mut lvl_noclip = find_effect("ec.color.levels").unwrap().instance();
        lvl_noclip.params.get_mut("clipToOutputBlack").unwrap().value = ParamValue::Choice(0);
        assert!(FxOp::eval(&lvl_noclip, &cx, 64, 36).is_none());
    }
}
