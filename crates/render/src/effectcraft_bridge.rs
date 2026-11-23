//! Bridge between `filmcraft-render` and the EffectCraft VFX engine (`filmcraft-effects`).
//!
//! Converts working images (`filmcraft_render::Image`, premultiplied linear RGBA) to and from
//! EffectCraft buffers (`filmcraft_effects::Buf`, premultiplied sRGB), maps [`FxCtx`] and
//! [`EffectInstance`] parameters into [`EffectCtx`], and wires [`crate::vfx::FxEnv`] into
//! [`filmcraft_effects::EffectHost`] so temporal and multi-track VFX operate natively on
//! FilmCraft sequences while respecting WolderMod's memory bounds.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use filmcraft_color::{Transfer, WorkingSpace, from_linear, to_linear};
use filmcraft_effects::effectcraft_color::ColorSpace as EcColorSpace;
use filmcraft_effects::effectcraft_keyframe::Value;
use filmcraft_effects::effectcraft_project::ParamUi;
use filmcraft_effects::{Buf, EffectCtx, EffectEnv, EffectHost, EffectSpec, Image as EcImage, LayerPixels, MaskShape, ParamSpec, Params};
use filmcraft_project::{EffectInstance, ParamValue};
use filmcraft_time::Tick;
use rayon::prelude::*;

use crate::effects::FxCtx;
use crate::image::Image;

fn working_transfer(w: WorkingSpace) -> Transfer {
    match w {
        WorkingSpace::Rec709 => Transfer::Srgb,
        WorkingSpace::Rec2100Pq => Transfer::Pq,
        WorkingSpace::Rec2100Hlg => Transfer::Hlg,
    }
}

/// Convert a FilmCraft linear-light premultiplied `Image` to an EffectCraft sRGB-premultiplied `EcImage`.
pub fn to_ec_image(img: &Image) -> EcImage {
    to_ec_image_in(img, WorkingSpace::Rec709)
}

/// Convert a FilmCraft linear-light premultiplied `Image` to an EffectCraft `EcImage` using the
/// transfer curve of `working`.
pub fn to_ec_image_in(img: &Image, working: WorkingSpace) -> EcImage {
    let tf = working_transfer(working);
    let (w, h) = (img.w as u32, img.h as u32);
    let n = img.w.saturating_mul(img.h);
    if img.px.len() < n.saturating_mul(4) || w == 0 || h == 0 {
        return EcImage::default();
    }
    let mut data = vec![[0.0f32; 4]; n];
    data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        let src_row = &img.px[y * img.w * 4..(y + 1) * img.w * 4];
        for (x, dst) in row.iter_mut().enumerate() {
            let p = &src_row[x * 4..x * 4 + 4];
            let a = p[3];
            if (a - 1.0).abs() <= 1e-6 {
                *dst = [from_linear(p[0], tf), from_linear(p[1], tf), from_linear(p[2], tf), 1.0];
            } else if a > 1e-7 {
                let inv_a = 1.0 / a;
                *dst = [from_linear(p[0] * inv_a, tf) * a, from_linear(p[1] * inv_a, tf) * a, from_linear(p[2] * inv_a, tf) * a, a];
            } else {
                *dst = [0.0, 0.0, 0.0, a.max(0.0)];
            }
        }
    });
    EcImage { width: w, height: h, data }
}

/// Write an EffectCraft `Buf` back into a FilmCraft `Image` (cropping/aligning by `buf.offset`).
pub fn from_ec_buf(img: &mut Image, buf: &Buf) {
    from_ec_buf_in(img, buf, WorkingSpace::Rec709);
}

/// Write an EffectCraft `Buf` back into a FilmCraft `Image` using the transfer curve of `working`.
pub fn from_ec_buf_in(img: &mut Image, buf: &Buf, working: WorkingSpace) {
    let tf = working_transfer(working);
    let (w, h) = (img.w, img.h);
    if w == 0 || h == 0 || img.px.len() < w.saturating_mul(h).saturating_mul(4) {
        return;
    }
    let ox = buf.offset[0].round() as isize;
    let oy = buf.offset[1].round() as isize;
    let (bw, bh) = (buf.img.width as isize, buf.img.height as isize);
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let sy = y as isize + oy;
        for x in 0..w {
            let sx = x as isize + ox;
            let dst = &mut row[x * 4..x * 4 + 4];
            if sx >= 0 && sx < bw && sy >= 0 && sy < bh {
                let src = buf.img.data[(sy as usize) * (bw as usize) + (sx as usize)];
                let a = if src[3].is_finite() { src[3].max(0.0) } else { 0.0 };
                if (a - 1.0).abs() <= 1e-6 {
                    let r = to_linear(src[0].max(0.0), tf);
                    let g = to_linear(src[1].max(0.0), tf);
                    let b = to_linear(src[2].max(0.0), tf);
                    dst[0] = if r.is_finite() { r } else { 0.0 };
                    dst[1] = if g.is_finite() { g } else { 0.0 };
                    dst[2] = if b.is_finite() { b } else { 0.0 };
                    dst[3] = 1.0;
                } else if a > 1e-7 {
                    let inv_a = 1.0 / a;
                    let r = to_linear((src[0] * inv_a).max(0.0), tf) * a;
                    let g = to_linear((src[1] * inv_a).max(0.0), tf) * a;
                    let b = to_linear((src[2] * inv_a).max(0.0), tf) * a;
                    dst[0] = if r.is_finite() { r } else { 0.0 };
                    dst[1] = if g.is_finite() { g } else { 0.0 };
                    dst[2] = if b.is_finite() { b } else { 0.0 };
                    dst[3] = a;
                } else {
                    dst.copy_from_slice(&[0.0, 0.0, 0.0, a]);
                }
            } else {
                dst.copy_from_slice(&[0.0, 0.0, 0.0, 0.0]);
            }
        }
    });
}

fn map_working_space(w: WorkingSpace) -> Option<EcColorSpace> {
    Some(match w {
        WorkingSpace::Rec709 => EcColorSpace::Rec709,
        WorkingSpace::Rec2100Pq => EcColorSpace::Rec2100Pq,
        WorkingSpace::Rec2100Hlg => EcColorSpace::Rec2100Hlg,
    })
}

fn instance_seed(cx: &FxCtx, effect_index: usize) -> u32 {
    let base = cx.env.map_or(1u64, |env| env.clip_key());
    let mixed = base ^ ((effect_index as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    ((mixed ^ (mixed >> 32)) as u32) | 1
}

struct BridgeHost<'a> {
    orig: &'a Buf,
    spec: &'static EffectSpec,
    inst: &'a EffectInstance,
    cx: &'a FxCtx<'a>,
    scale: f64,
    layer_size: [f64; 2],
    layer_cache: Mutex<HashMap<(u64, bool), LayerPixels>>,
}

impl BridgeHost<'_> {
    fn tick_at_layer_time(&self, layer_time: f64) -> Option<Tick> {
        let dt = layer_time - self.cx.seconds;
        if !dt.is_finite() {
            return None;
        }
        Some(self.cx.t + Tick::from_seconds_f64(dt))
    }
}

impl EffectHost for BridgeHost<'_> {
    fn original(&self) -> Option<&Buf> {
        Some(self.orig)
    }

    fn layer(&self, id: u64, masks_and_effects: bool) -> Option<LayerPixels> {
        {
            let cache = self.layer_cache.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(hit) = cache.get(&(id, masks_and_effects)) {
                return Some(hit.clone());
            }
        }
        let env = self.cx.env?;
        let track_idx = (id as usize).checked_sub(1)?;
        let track_img = env.track(track_idx)?;
        let ec_img = to_ec_image_in(&track_img, self.cx.working);
        let size = [f64::from(ec_img.width) / self.scale, f64::from(ec_img.height) / self.scale];
        let lp = LayerPixels { buf: Buf { img: ec_img, offset: [0.0, 0.0], scale: self.scale }, size };
        self.layer_cache.lock().unwrap_or_else(PoisonError::into_inner).insert((id, masks_and_effects), lp.clone());
        Some(lp)
    }

    fn audio(&self, _id: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
        self.cx.env?.audio_at(start, frames, rate)
    }

    fn layer_at(&self, id: u64, comp_time: f64, _masks_and_effects: bool) -> Option<LayerPixels> {
        let env = self.cx.env?;
        let track_idx = (id as usize).checked_sub(1)?;
        let dt = comp_time - self.cx.seconds;
        if !dt.is_finite() {
            return None;
        }
        let track_img = env.track_at(track_idx, dt)?;
        let ec_img = to_ec_image_in(&track_img, self.cx.working);
        let size = [f64::from(ec_img.width) / self.scale, f64::from(ec_img.height) / self.scale];
        Some(LayerPixels { buf: Buf { img: ec_img, offset: [0.0, 0.0], scale: self.scale }, size })
    }

    fn self_at(&self, layer_time: f64, _effects: usize) -> Option<Buf> {
        let env = self.cx.env?;
        let dt = layer_time - self.cx.seconds;
        if !dt.is_finite() {
            return None;
        }
        let frame_img = env.frame(dt, self.inst)?;
        Some(Buf { img: to_ec_image_in(&frame_img, self.cx.working), offset: [0.0, 0.0], scale: self.scale })
    }

    fn params_at(&self, layer_time: f64) -> Option<Params> {
        let t = self.tick_at_layer_time(layer_time)?;
        Some(build_params_at(self.spec, self.inst, t, self.layer_size))
    }

    fn param_at(&self, _effect_index: usize, id: &str, layer_time: f64) -> Option<Value> {
        let t = self.tick_at_layer_time(layer_time)?;
        let ps = self.spec.params.iter().find(|p| p.id == id)?;
        Some(eval_param(ps, self.inst, t, self.layer_size))
    }

    fn param_is_static(&self, _effect_index: usize, id: &str) -> bool {
        self.inst.params.get(id).is_some_and(|p| !p.is_animated())
    }
}

fn eval_param(ps: &ParamSpec, e: &EffectInstance, t: Tick, layer_size: [f64; 2]) -> Value {
    match e.params.get(ps.id) {
        Some(param) => {
            let pv = param.value_at(t);
            match (&ps.ui, pv) {
                (ParamUi::Point, ParamValue::Vec2(v)) => {
                    let [dfx, dfy] = ps.default.as_vec2();
                    let x = if v.x.is_finite() { v.x } else { layer_size[0] * dfx };
                    let y = if v.y.is_finite() { v.y } else { layer_size[1] * dfy };
                    Value::Vec2([x, y])
                }
                (ParamUi::Point3, ParamValue::Vec2(v)) => {
                    let [dfx, dfy, dfz] = ps.default.as_vec3();
                    let x = if v.x.is_finite() { v.x } else { layer_size[0] * dfx };
                    let y = if v.y.is_finite() { v.y } else { layer_size[1] * dfy };
                    let z = e.params.get(&format!("{}.z", ps.id)).map(|zp| zp.f64_at(t)).filter(|z| z.is_finite()).unwrap_or(dfz);
                    Value::Vec3([x, y, z])
                }
                (ParamUi::Color, ParamValue::Color(c)) => Value::Color([f64::from(c[0]), f64::from(c[1]), f64::from(c[2]), f64::from(c[3])]),
                (ParamUi::Checkbox, ParamValue::Bool(b)) => Value::Bool(b),
                (ParamUi::Popup { .. }, ParamValue::Choice(c)) => Value::Enum(c),
                (ParamUi::Layer, ParamValue::Choice(c)) => Value::Layer(if c == 0 { None } else { Some(u64::from(c)) }),
                (ParamUi::Mask, ParamValue::Choice(c)) => Value::Scalar(f64::from(c)),
                (ParamUi::Text | ParamUi::File { .. }, ParamValue::Text(s)) => Value::Str(s),
                (_, ParamValue::Float(f)) => Value::Scalar(if f.is_finite() { f } else { ps.default.as_f64() }),
                _ => ps.default.clone(),
            }
        }
        None => match &ps.ui {
            ParamUi::Point => {
                let [fx, fy] = ps.default.as_vec2();
                Value::Vec2([layer_size[0] * fx, layer_size[1] * fy])
            }
            ParamUi::Point3 => {
                let [fx, fy, fz] = ps.default.as_vec3();
                Value::Vec3([layer_size[0] * fx, layer_size[1] * fy, fz])
            }
            _ => ps.default.clone(),
        },
    }
}

fn build_params_at(spec: &EffectSpec, e: &EffectInstance, t: Tick, layer_size: [f64; 2]) -> Params {
    let mut values = HashMap::with_capacity(spec.params.len());
    for ps in &spec.params {
        values.insert(ps.id.to_string(), eval_param(ps, e, t, layer_size));
    }
    Params { values }
}

/// Evaluate `EffectInstance` parameters at `cx.t` into EffectCraft [`Params`].
pub fn build_params(spec: &EffectSpec, e: &EffectInstance, cx: &FxCtx, layer_size: [f64; 2]) -> Params {
    build_params_at(spec, e, cx.t, layer_size)
}

fn build_masks(e: &EffectInstance, cx: &FxCtx, layer_size: [f64; 2]) -> Vec<MaskShape> {
    e.masks
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let pts = m
                .path
                .value_at(cx.t)
                .as_path()
                .map(|verts| verts.vertices.iter().map(|v| [v.p.x * layer_size[0], v.p.y * layer_size[1]]).collect())
                .unwrap_or_default();
            MaskShape { name: format!("Mask {}", i + 1), points: pts, closed: true, inverted: m.inverted }
        })
        .collect()
}

/// Apply an EffectCraft effect (`"ec.*"`) to `img` if found in `filmcraft_effects::find()`.
/// Returns `true` when the effect belonged to EffectCraft and was processed.
pub fn apply(img: &mut Image, e: &EffectInstance, cx: &FxCtx) -> bool {
    apply_at(img, e, cx, 0)
}

/// Apply an EffectCraft effect (`"ec.*"`) at `effect_index` in the clip's stack.
pub fn apply_at(img: &mut Image, e: &EffectInstance, cx: &FxCtx, effect_index: usize) -> bool {
    if !e.effect.starts_with("ec.") {
        return false;
    }
    let Some(spec) = filmcraft_effects::find(&e.effect) else {
        return false;
    };
    let scale = f64::from(cx.px_scale).clamp(0.0625, 16.0);
    let layer_size = [(img.w as f64 / scale).max(1.0), (img.h as f64 / scale).max(1.0)];
    let in_buf = Buf { img: to_ec_image_in(img, cx.working), offset: [0.0, 0.0], scale };
    let out_buf = run_spec(spec, e, cx, effect_index, scale, layer_size, &in_buf, in_buf.clone());
    from_ec_buf_in(img, &out_buf, cx.working);
    true
}

/// Blend `fx` over `prev` using per-pixel `cov` (matching `prev`'s dimensions and offset).
fn mix_ec_masked(prev: &Buf, fx: &Buf, cov: &[f32]) -> Buf {
    let w = prev.img.width as usize;
    let fw = fx.img.width as isize;
    let fh = fx.img.height as isize;
    let ox = (fx.offset[0] - prev.offset[0]).round() as isize;
    let oy = (fx.offset[1] - prev.offset[1]).round() as isize;
    let mut out = prev.img.data.clone();
    out.par_chunks_mut(w).zip(cov.par_chunks(w)).enumerate().for_each(|(y, (row, crow))| {
        let sy = y as isize + oy;
        for x in 0..w {
            let c = crow[x];
            if c <= 0.0 {
                continue;
            }
            let sx = x as isize + ox;
            let fx_px =
                if sx >= 0 && sx < fw && sy >= 0 && sy < fh { fx.img.data[(sy as usize) * (fw as usize) + (sx as usize)] } else { [0.0, 0.0, 0.0, 0.0] };
            let o = row[x];
            row[x] = [o[0] + (fx_px[0] - o[0]) * c, o[1] + (fx_px[1] - o[1]) * c, o[2] + (fx_px[2] - o[2]) * c, o[3] + (fx_px[3] - o[3]) * c];
        }
    });
    Buf { img: EcImage { width: prev.img.width, height: prev.img.height, data: out }, offset: prev.offset, scale: prev.scale }
}

/// Apply a consecutive slice of enabled EffectCraft effects (`"ec.*"`, with or without masks)
/// in a single linear ↔ sRGB round-trip.
pub fn apply_chain(img: &mut Image, chain: &[(usize, &EffectInstance)], cx: &FxCtx) {
    if chain.is_empty() {
        return;
    }
    let scale = f64::from(cx.px_scale).clamp(0.0625, 16.0);
    let layer_size = [(img.w as f64 / scale).max(1.0), (img.h as f64 / scale).max(1.0)];
    let orig = Buf { img: to_ec_image_in(img, cx.working), offset: [0.0, 0.0], scale };
    let mut cur = orig.clone();
    for &(effect_index, e) in chain {
        if let Some(spec) = filmcraft_effects::find(&e.effect) {
            let masks_on = e.param("masks_on").and_then(|p| p.value.as_bool()).unwrap_or(true);
            if masks_on && !e.masks.is_empty() {
                if cur.offset == [0.0, 0.0] && cur.img.width as usize == img.w && cur.img.height as usize == img.h {
                    if let Some(cov) = crate::mask::effect_coverage(&e.masks, cx.t, cx.px_scale, img.w, img.h) {
                        let prev = cur.clone();
                        let fx = run_spec(spec, e, cx, effect_index, scale, layer_size, &orig, cur);
                        cur = mix_ec_masked(&prev, &fx, &cov);
                    } else {
                        cur = run_spec(spec, e, cx, effect_index, scale, layer_size, &orig, cur);
                    }
                } else {
                    from_ec_buf_in(img, &cur, cx.working);
                    crate::mask::apply_effect(img, e, cx);
                    cur = Buf { img: to_ec_image_in(img, cx.working), offset: [0.0, 0.0], scale };
                }
            } else {
                cur = run_spec(spec, e, cx, effect_index, scale, layer_size, &orig, cur);
            }
        }
    }
    from_ec_buf_in(img, &cur, cx.working);
}

#[allow(clippy::too_many_arguments)]
fn run_spec(spec: &'static EffectSpec, e: &EffectInstance, cx: &FxCtx, effect_index: usize, scale: f64, layer_size: [f64; 2], orig: &Buf, in_buf: Buf) -> Buf {
    let params = build_params(spec, e, cx, layer_size);
    let masks = build_masks(e, cx, layer_size);
    let host = BridgeHost { orig, spec, inst: e, cx, scale, layer_size, layer_cache: Mutex::new(HashMap::new()) };
    let env = EffectEnv {
        masks: &masks,
        host: Some(&host),
        comp_time: cx.seconds,
        frame_rate: if cx.frame_rate > 0.0 { cx.frame_rate } else { 30.0 },
        effect_index,
        working_space: map_working_space(cx.working),
        working_linear: false,
        float_depth: true,
        shutter: None,
        layer_span: cx.env.map(|env| (0.0, env.clip_seconds().max(0.0))),
    };
    let ectx = EffectCtx { params: &params, time: cx.seconds, layer_size, seed: instance_seed(cx, effect_index), adjustment: false, env };
    (spec.render)(&ectx, in_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{ParamValue, find_effect};

    fn test_cx() -> FxCtx<'static> {
        FxCtx {
            t: Tick::ZERO,
            px_scale: 1.0,
            seconds: 0.25,
            timecode: "",
            clip_name: "clip",
            project: None,
            env: None,
            frame_rate: 24.0,
            working: WorkingSpace::Rec709,
        }
    }

    #[test]
    fn apply_chain_matches_sequential_apply() {
        let mut img_seq = Image::filled(16, 12, [0.4, 0.2, 0.1, 1.0]);
        img_seq.px[0] = 0.8;
        let mut img_chain = img_seq.clone();

        let mut tint = find_effect("ec.color.tint").unwrap().instance();
        tint.params.get_mut("amount").unwrap().value = ParamValue::Float(50.0);
        let mut blur = find_effect("ec.blur.gaussian").unwrap().instance();
        blur.params.get_mut("blurriness").unwrap().value = ParamValue::Float(4.0);

        let cx = test_cx();
        assert!(apply_at(&mut img_seq, &tint, &cx, 0));
        assert!(apply_at(&mut img_seq, &blur, &cx, 1));
        apply_chain(&mut img_chain, &[(0, &tint), (1, &blur)], &cx);

        for (a, b) in img_seq.px.iter().zip(&img_chain.px) {
            assert!((a - b).abs() < 1e-5, "chain mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn point3_z_param_is_forwarded_to_effectcraft() {
        let mut inst = EffectInstance {
            effect: "ec.control.point3d".to_string(),
            enabled: true,
            params: Default::default(),
            masks: Vec::new(),
            post_fader: false,
            essential: false,
            layer: None,
            label: None,
        };
        inst.params.insert("point".to_string(), filmcraft_project::Param::new(ParamValue::Vec2(filmcraft_geom::Vec2::new(10.0, 20.0))));
        inst.params.insert("point.z".to_string(), filmcraft_project::Param::new(ParamValue::Float(42.5)));
        let spec = filmcraft_effects::find("ec.control.point3d").unwrap();
        let cx = test_cx();
        let params = build_params(spec, &inst, &cx, [100.0, 100.0]);
        assert_eq!(params.get("point").map(|v| v.as_vec3()), Some([10.0, 20.0, 42.5]));
    }

    #[test]
    fn bridge_host_evaluates_params_at_and_static_flag() {
        let mut inst = find_effect("ec.color.tint").unwrap().instance();
        inst.params.get_mut("amount").unwrap().keyframes.push(filmcraft_project::Keyframe::new(Tick::from_seconds_f64(1.0), ParamValue::Float(20.0)));
        let spec = filmcraft_effects::find("ec.color.tint").unwrap();
        let cx = test_cx();
        let orig = Buf::default();
        let host = BridgeHost { orig: &orig, spec, inst: &inst, cx: &cx, scale: 1.0, layer_size: [100.0, 100.0], layer_cache: Mutex::new(HashMap::new()) };
        assert!(!host.param_is_static(0, "amount"));
        assert!(host.param_is_static(0, "black"));
        let p_later = host.param_at(0, "amount", cx.seconds + 1.0).unwrap();
        assert!((p_later.as_f64() - 20.0).abs() < 1e-6);
        assert!(host.params_at(cx.seconds + 1.0).is_some());
    }

    struct MockEnv;
    impl crate::vfx::FxEnv for MockEnv {
        fn frame(&self, _dt: f64, _upto: &EffectInstance) -> Option<Image> {
            None
        }
        fn source_frame(&self, _dt: f64, _scale: f32) -> Option<Image> {
            None
        }
        fn track(&self, _index: usize) -> Option<Image> {
            Some(Image::filled(4, 4, [0.5, 0.25, 0.125, 1.0]))
        }
        fn track_at(&self, index: usize, dt: f64) -> Option<Image> {
            if index == 0 && (dt - 0.5).abs() < 1e-6 { Some(Image::filled(4, 4, [0.25, 0.5, 0.75, 1.0])) } else { None }
        }
        fn audio_at(&self, _comp_start: f64, frames: usize, _rate: u32) -> Option<Vec<f32>> {
            Some(vec![0.25; frames * 2])
        }
        fn layer_to_output(&self) -> filmcraft_geom::Affine {
            filmcraft_geom::Affine::IDENTITY
        }
        fn clip_seconds(&self) -> f64 {
            2.0
        }
        fn clip_offset(&self) -> f64 {
            0.25
        }
        fn frame_rate(&self) -> f64 {
            24.0
        }
        fn media_timecode(&self) -> String {
            String::new()
        }
        fn file_name(&self) -> String {
            String::new()
        }
        fn sequence_name(&self) -> String {
            String::new()
        }
        fn sequence_size(&self) -> (u32, u32) {
            (4, 4)
        }
        fn source_size(&self) -> (u32, u32) {
            (4, 4)
        }
        fn clip_key(&self) -> u64 {
            7
        }
    }

    #[test]
    fn bridge_host_forwards_layer_at_and_audio() {
        let env = MockEnv;
        let mut cx = test_cx();
        cx.env = Some(&env);
        let inst = find_effect("ec.color.tint").unwrap().instance();
        let spec = filmcraft_effects::find("ec.color.tint").unwrap();
        let orig = Buf::default();
        let host = BridgeHost { orig: &orig, spec, inst: &inst, cx: &cx, scale: 1.0, layer_size: [4.0, 4.0], layer_cache: Mutex::new(HashMap::new()) };
        let lp = host.layer_at(1, cx.seconds + 0.5, false).expect("layer_at");
        assert_eq!(lp.buf.img.width, 4);
        let audio = host.audio(1, 0.0, 128, 48_000).expect("audio");
        assert_eq!(audio.len(), 256);
    }

    #[test]
    fn apply_chain_with_masked_effect_matches_sequential_mask_apply() {
        let mut img_seq = Image::filled(16, 12, [0.4, 0.2, 0.1, 1.0]);
        img_seq.px[0] = 0.8;
        let mut img_chain = img_seq.clone();

        let mut tint = find_effect("ec.color.tint").unwrap().instance();
        tint.params.get_mut("amount").unwrap().value = ParamValue::Float(80.0);
        let mut mask = filmcraft_project::Mask::new("Mask 1", filmcraft_project::MaskPath::rect(0.25, 0.25, 0.75, 0.75));
        mask.feather.value = ParamValue::Float(0.0);
        tint.masks.push(mask);

        let mut blur = find_effect("ec.blur.gaussian").unwrap().instance();
        blur.params.get_mut("blurriness").unwrap().value = ParamValue::Float(4.0);

        let cx = test_cx();
        crate::mask::apply_effect(&mut img_seq, &tint, &cx);
        assert!(apply_at(&mut img_seq, &blur, &cx, 1));
        apply_chain(&mut img_chain, &[(0, &tint), (1, &blur)], &cx);

        for (a, b) in img_seq.px.iter().zip(&img_chain.px) {
            // Antialiased mask edges blend in sRGB inside apply_chain vs linear in mask::apply_effect.
            assert!((a - b).abs() < 1e-3, "masked chain mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn hdr_working_space_round_trip_preserves_highlights() {
        for working in [WorkingSpace::Rec2100Pq, WorkingSpace::Rec2100Hlg] {
            let hdr_px = [5.0, 2.0, 0.5, 1.0];
            let mut img = Image::filled(8, 8, hdr_px);
            let mut cx = test_cx();
            cx.working = working;

            let ec_img = to_ec_image_in(&img, working);
            assert!(ec_img.data.iter().flatten().all(|v| v.is_finite() && *v <= 1.5), "expected encoded HDR within nominal range for {working:?}");

            let blur = find_effect("ec.blur.gaussian").unwrap().instance();
            apply_chain(&mut img, &[(0, &blur)], &cx);
            for (got, want) in img.px[0..4].iter().zip(&hdr_px) {
                let tol = 5e-4 * want.abs().max(1.0);
                assert!((got - want).abs() <= tol, "HDR round-trip mismatch in {working:?}: got {got}, want {want}");
            }
        }
    }
}
