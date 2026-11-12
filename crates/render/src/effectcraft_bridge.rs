//! Bridge between `filmcraft-render` and the EffectCraft VFX engine (`filmcraft-effects`).
//!
//! Converts working images (`filmcraft_render::Image`, premultiplied linear RGBA) to and from
//! EffectCraft buffers (`filmcraft_effects::Buf`, premultiplied sRGB), maps [`FxCtx`] and
//! [`EffectInstance`] parameters into [`EffectCtx`], and wires [`crate::vfx::FxEnv`] into
//! [`filmcraft_effects::EffectHost`] so temporal and multi-track VFX operate natively on
//! FilmCraft sequences while respecting WolderMod's memory bounds.

use std::collections::HashMap;

use filmcraft_color::{linear_to_srgb, srgb_to_linear};
use filmcraft_effects::effectcraft_keyframe::Value;
use filmcraft_effects::effectcraft_project::ParamUi;
use filmcraft_effects::{Buf, EffectCtx, EffectEnv, EffectHost, Image as EcImage, LayerPixels, MaskShape, Params};
use filmcraft_project::{EffectInstance, ParamValue};
use rayon::prelude::*;

use crate::effects::FxCtx;
use crate::image::Image;

/// Convert a FilmCraft linear-light premultiplied `Image` to an EffectCraft sRGB-premultiplied `EcImage`.
pub fn to_ec_image(img: &Image) -> EcImage {
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
            if a > 1e-7 {
                let inv_a = 1.0 / a;
                *dst = [
                    linear_to_srgb(p[0] * inv_a) * a,
                    linear_to_srgb(p[1] * inv_a) * a,
                    linear_to_srgb(p[2] * inv_a) * a,
                    a,
                ];
            } else {
                *dst = [0.0, 0.0, 0.0, a.max(0.0)];
            }
        }
    });
    EcImage { width: w, height: h, data }
}

/// Write an EffectCraft `Buf` back into a FilmCraft `Image` (cropping/aligning by `buf.offset`).
pub fn from_ec_buf(img: &mut Image, buf: &Buf) {
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
                if a > 1e-7 {
                    let inv_a = 1.0 / a;
                    let r = srgb_to_linear((src[0] * inv_a).max(0.0)) * a;
                    let g = srgb_to_linear((src[1] * inv_a).max(0.0)) * a;
                    let b = srgb_to_linear((src[2] * inv_a).max(0.0)) * a;
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

struct BridgeHost<'a> {
    orig: Buf,
    inst: &'a EffectInstance,
    cx: &'a FxCtx<'a>,
    scale: f64,
    layer_size: [f64; 2],
}

impl EffectHost for BridgeHost<'_> {
    fn original(&self) -> Option<&Buf> {
        Some(&self.orig)
    }

    fn layer(&self, id: u64, _masks_and_effects: bool) -> Option<LayerPixels> {
        let env = self.cx.env?;
        let track_idx = (id as usize).checked_sub(1)?;
        let track_img = env.track(track_idx)?;
        let ec_img = to_ec_image(&track_img);
        let size = [f64::from(ec_img.width) / self.scale, f64::from(ec_img.height) / self.scale];
        Some(LayerPixels {
            buf: Buf { img: ec_img, offset: [0.0, 0.0], scale: self.scale },
            size,
        })
    }

    fn audio(&self, _id: u64, _start: f64, _frames: usize, _rate: u32) -> Option<Vec<f32>> {
        None
    }

    fn self_at(&self, layer_time: f64, _effects: usize) -> Option<Buf> {
        let env = self.cx.env?;
        let dt = layer_time - self.cx.seconds;
        if !dt.is_finite() {
            return None;
        }
        let frame_img = env.frame(dt, self.inst)?;
        let _ = self.layer_size;
        Some(Buf {
            img: to_ec_image(&frame_img),
            offset: [0.0, 0.0],
            scale: self.scale,
        })
    }
}

/// Evaluate `EffectInstance` parameters at `cx.t` into EffectCraft [`Params`].
pub fn build_params(spec: &filmcraft_effects::EffectSpec, e: &EffectInstance, cx: &FxCtx, layer_size: [f64; 2]) -> Params {
    let mut values = HashMap::with_capacity(spec.params.len());
    for ps in &spec.params {
        let val = match e.params.get(ps.id) {
            Some(param) => {
                let pv = param.value_at(cx.t);
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
                        Value::Vec3([x, y, dfz])
                    }
                    (ParamUi::Color, ParamValue::Color(c)) => {
                        Value::Color([f64::from(c[0]), f64::from(c[1]), f64::from(c[2]), f64::from(c[3])])
                    }
                    (ParamUi::Checkbox, ParamValue::Bool(b)) => Value::Bool(b),
                    (ParamUi::Popup { .. }, ParamValue::Choice(c)) => Value::Enum(c),
                    (ParamUi::Layer, ParamValue::Choice(c)) => {
                        Value::Layer(if c == 0 { None } else { Some(u64::from(c)) })
                    }
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
                _ => ps.default.clone(),
            },
        };
        values.insert(ps.id.to_string(), val);
    }
    Params { values }
}

/// Apply an EffectCraft effect (`"ec.*"`) to `img` if found in `filmcraft_effects::registry()`.
/// Returns `true` when the effect belonged to EffectCraft and was processed.
pub fn apply(img: &mut Image, e: &EffectInstance, cx: &FxCtx) -> bool {
    if !e.effect.starts_with("ec.") {
        return false;
    }
    let Some(spec) = filmcraft_effects::find(&e.effect) else {
        return false;
    };
    let scale = f64::from(cx.px_scale).clamp(0.0625, 16.0);
    let layer_size = [
        (img.w as f64 / scale).max(1.0),
        (img.h as f64 / scale).max(1.0),
    ];
    let params = build_params(spec, e, cx, layer_size);
    let masks: Vec<MaskShape> = e
        .masks
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let pts = m
                .path
                .value_at(cx.t)
                .as_path()
                .map(|verts| verts.vertices.iter().map(|v| [v.p.x * layer_size[0], v.p.y * layer_size[1]]).collect())
                .unwrap_or_default();
            MaskShape {
                name: format!("Mask {}", i + 1),
                points: pts,
                closed: true,
                inverted: m.inverted,
            }
        })
        .collect();
    let in_buf = Buf {
        img: to_ec_image(img),
        offset: [0.0, 0.0],
        scale,
    };
    let host = BridgeHost {
        orig: in_buf.clone(),
        inst: e,
        cx,
        scale,
        layer_size,
    };
    let env = EffectEnv {
        masks: &masks,
        host: Some(&host),
        comp_time: cx.seconds,
        frame_rate: if cx.frame_rate > 0.0 { cx.frame_rate } else { 30.0 },
        effect_index: 0,
        working_space: None,
        working_linear: false,
        float_depth: true,
        shutter: None,
        layer_span: None,
    };
    let ectx = EffectCtx {
        params: &params,
        time: cx.seconds,
        layer_size,
        seed: 1,
        adjustment: false,
        env,
    };
    let out_buf = (spec.render)(&ectx, in_buf);
    from_ec_buf(img, &out_buf);
    true
}
