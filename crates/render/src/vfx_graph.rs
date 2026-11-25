//! Pull-based CPU DAG evaluator for [`filmcraft_project::VfxGraph`].
//!
//! Evaluates a non-linear VFX node graph (`Effect`, `Merge`, `Mask`, `PipeRouter`, `Instance`)
//! backwards from the requested [`VfxEndpoint`] (typically `VfxEndpoint::MediaOut`), supporting:
//! - Bounded per-frame memoization for fan-out branches and shared `Mask` nodes
//! - Global bounded LRU cache (`VFX_CACHE`, max 32 entries) keyed by node signature + upstream keys
//! - Cycle and recursion-depth guards (`MAX_EVAL_DEPTH`) obeying Never Crash rules
//! - Fusion-style transform concatenation across consecutive unmodified `"transform"` nodes
//! - Universal [`VfxNodeSettings`] (animatable `blend`, blue `EffectMask` matte, `invert_mask`,
//!   `multiply_by_mask`, and per-channel `[R, G, B, A]` checkboxes)

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use filmcraft_geom::{Affine, Vec2};
use filmcraft_project::{
    EffectInstance, LayerBlendMode, MergeOperator, Param, ParamValue, VfxEndpoint, VfxGraph, VfxInputSlot, VfxNode, VfxNodeId, VfxNodeKind, VfxNodeSettings,
};
use filmcraft_time::Tick;
use rayon::prelude::*;

use crate::Image;
use crate::color_graph::apply_layer_blend_rgb;
use crate::effects::FxCtx;

/// Hard recursion guard while traversing arbitrary project graphs.
pub const MAX_EVAL_DEPTH: usize = 64;

/// Maximum number of node outputs held in the cross-frame LRU cache.
const MAX_CACHE_ENTRIES: usize = 32;

static VFX_CACHE: OnceLock<Mutex<VecDeque<(u64, Arc<Image>)>>> = OnceLock::new();

fn vfx_cache() -> &'static Mutex<VecDeque<(u64, Arc<Image>)>> {
    VFX_CACHE.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_CACHE_ENTRIES)))
}

fn cache_get(key: u64, w: usize, h: usize) -> Option<Arc<Image>> {
    let guard = vfx_cache().lock().unwrap_or_else(PoisonError::into_inner);
    guard.iter().rev().find(|(k, im)| *k == key && im.w == w && im.h == h).map(|(_, im)| Arc::clone(im))
}

fn cache_put(key: u64, img: Arc<Image>) {
    let mut guard = vfx_cache().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(pos) = guard.iter().position(|(k, _)| *k == key) {
        guard.remove(pos);
    }
    if guard.len() >= MAX_CACHE_ENTRIES {
        guard.pop_front();
    }
    guard.push_back((key, img));
}

#[derive(Clone)]
struct EvalFrame {
    img: Arc<Image>,
    key: u64,
}

struct GraphEval<'a, 'cx> {
    graph: &'a VfxGraph,
    media_in: Arc<Image>,
    media_in_key: u64,
    cx: &'a FxCtx<'cx>,
    memo: HashMap<VfxNodeId, EvalFrame>,
    mask_memo: HashMap<VfxNodeId, Option<Arc<Vec<f32>>>>,
    visiting: HashSet<VfxNodeId>,
}

/// Evaluate `graph` up to `target` (normally `VfxEndpoint::MediaOut`), reading source pixels from
/// `media_in` and writing the final composited image back into `media_in`.
pub fn apply_vfx_graph_cpu(graph: &VfxGraph, media_in: &mut Image, cx: &FxCtx, target: VfxEndpoint) {
    if graph.is_empty() || media_in.w == 0 || media_in.h == 0 {
        return;
    }
    let media_in_key = fingerprint_image(media_in, cx);
    let media_in_arc = Arc::new(std::mem::replace(media_in, Image::new(0, 0)));
    let mut eval = GraphEval {
        graph,
        media_in: media_in_arc,
        media_in_key,
        cx,
        memo: HashMap::with_capacity(graph.nodes.len()),
        mask_memo: HashMap::new(),
        visiting: HashSet::new(),
    };
    let out = eval.eval_endpoint(target, 0);
    *media_in = (*out.img).clone();
}

fn fingerprint_image(img: &Image, cx: &FxCtx) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    img.w.hash(&mut h);
    img.h.hash(&mut h);
    img.px.len().hash(&mut h);
    cx.t.0.hash(&mut h);
    cx.px_scale.to_bits().hash(&mut h);
    let step = (img.px.len() / 64).max(1);
    for idx in (0..img.px.len()).step_by(step) {
        if let Some(v) = img.px.get(idx) {
            v.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

fn hash_param(p: &Param, t: Tick, h: &mut std::collections::hash_map::DefaultHasher) {
    match p.value_at(t) {
        ParamValue::Float(v) => {
            1u8.hash(h);
            v.to_bits().hash(h);
        }
        ParamValue::Bool(v) => {
            2u8.hash(h);
            v.hash(h);
        }
        ParamValue::Vec2(v) => {
            3u8.hash(h);
            v.x.to_bits().hash(h);
            v.y.to_bits().hash(h);
        }
        ParamValue::Color(c) => {
            4u8.hash(h);
            for x in c {
                x.to_bits().hash(h);
            }
        }
        ParamValue::Choice(c) => {
            5u8.hash(h);
            c.hash(h);
        }
        ParamValue::Text(s) => {
            6u8.hash(h);
            s.hash(h);
        }
        ParamValue::Curve(pts) => {
            7u8.hash(h);
            pts.len().hash(h);
            for [x, y] in pts {
                x.to_bits().hash(h);
                y.to_bits().hash(h);
            }
        }
        ParamValue::Path(path) => {
            8u8.hash(h);
            path.closed.hash(h);
            path.vertices.len().hash(h);
            for v in path.vertices {
                v.p.x.to_bits().hash(h);
                v.p.y.to_bits().hash(h);
            }
        }
    }
}

fn hash_masks(masks: &[filmcraft_project::Mask], t: Tick, h: &mut std::collections::hash_map::DefaultHasher) {
    masks.len().hash(h);
    for m in masks {
        (m.mode as u8).hash(h);
        m.inverted.hash(h);
        m.feather.f64_at(t).to_bits().hash(h);
        m.feather_balance.f64_at(t).to_bits().hash(h);
        m.expansion.f64_at(t).to_bits().hash(h);
        m.opacity.f64_at(t).to_bits().hash(h);
        let path = m.effective_path_at(t);
        path.closed.hash(h);
        path.vertices.len().hash(h);
        for v in &path.vertices {
            v.p.x.to_bits().hash(h);
            v.p.y.to_bits().hash(h);
        }
    }
}

fn hash_effect(fx: &EffectInstance, t: Tick, h: &mut std::collections::hash_map::DefaultHasher) {
    fx.effect.hash(h);
    fx.enabled.hash(h);
    for (k, p) in &fx.params {
        k.hash(h);
        hash_param(p, t, h);
    }
    hash_masks(&fx.masks, t, h);
}

fn hash_settings(s: &VfxNodeSettings, t: Tick, h: &mut std::collections::hash_map::DefaultHasher) {
    hash_param(&s.blend, t, h);
    s.invert_mask.hash(h);
    s.multiply_by_mask.hash(h);
    s.channels.hash(h);
}

fn is_env_sensitive_effect(id: &str) -> bool {
    matches!(
        id,
        "echo"
            | "posterize_time"
            | "warp_stabilizer"
            | "auto_reframe"
            | "track_matte"
            | "compound_blur"
            | "gradient_wipe"
            | "gradient_wipe_legacy"
            | "timecode"
            | "noise"
            | "noise_legacy"
            | "strobe_light"
    ) || id.starts_with("ec.")
}

fn is_default_settings(s: &VfxNodeSettings, t: Tick) -> bool {
    !s.invert_mask && !s.multiply_by_mask && s.channels == [true, true, true, true] && (s.blend.f64_at(t) - 1.0).abs() <= 1e-4
}

fn has_effect_mask_edge(graph: &VfxGraph, node_id: VfxNodeId) -> bool {
    let ep = VfxEndpoint::Node(node_id);
    graph.edges.iter().any(|e| e.to == ep && e.to_slot == VfxInputSlot::EffectMask)
}

fn is_concat_transform(graph: &VfxGraph, node: &VfxNode, cx: &FxCtx) -> Option<EffectInstance> {
    if !node.enabled || has_effect_mask_edge(graph, node.id) || !is_default_settings(&node.settings, cx.t) {
        return None;
    }
    let inst = graph.resolve_effect(node.id)?;
    if inst.effect != "transform" || !inst.masks.is_empty() {
        return None;
    }
    let shutter = inst.param("shutter_override").and_then(|p| p.value.as_bool()).unwrap_or(false) && inst.f64_at("shutter_angle", cx.t) > 0.5;
    if shutter { None } else { Some(inst) }
}

fn transform_affine_and_opacity(fx: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> (Affine, f32) {
    let pt = |id: &str| -> Vec2 {
        let v = fx.param(id).map(|p| p.vec2_at(cx.t)).unwrap_or(Vec2::new(f64::NAN, f64::NAN));
        Vec2::new(if v.x.is_nan() { w as f64 / 2.0 } else { v.x * cx.px_scale as f64 }, if v.y.is_nan() { h as f64 / 2.0 } else { v.y * cx.px_scale as f64 })
    };
    let anchor = pt("anchor");
    let pos = pt("position");
    let sh = fx.f64_at("scale_height", cx.t) / 100.0;
    let uniform = fx.param("uniform_scale").and_then(|p| p.value.as_bool()).unwrap_or(true);
    let sw = if uniform { sh } else { fx.f64_at("scale_width", cx.t) / 100.0 };
    let rot = fx.f64_at("rotation", cx.t);
    let skew = fx.f64_at("skew", cx.t).to_radians().tan();
    let skew_axis = fx.f64_at("skew_axis", cx.t);
    let op = (fx.f64_at("opacity", cx.t) as f32 / 100.0).clamp(0.0, 1.0);
    let sk = Affine::rotate_deg(skew_axis).then_apply(&Affine { a: 1.0, b: 0.0, c: skew, d: 1.0, e: 0.0, f: 0.0 }).then_apply(&Affine::rotate_deg(-skew_axis));
    let m = Affine::translate(pos.x, pos.y)
        .then_apply(&Affine::rotate_deg(rot))
        .then_apply(&sk)
        .then_apply(&Affine::scale(sw, sh))
        .then_apply(&Affine::translate(-anchor.x, -anchor.y));
    (m, op)
}

impl<'a, 'cx> GraphEval<'a, 'cx> {
    fn incoming(&self, to: VfxEndpoint, slot: VfxInputSlot) -> Option<VfxEndpoint> {
        self.graph.edges.iter().find(|e| e.to == to && e.to_slot == slot).map(|e| e.from)
    }

    fn eval_endpoint(&mut self, ep: VfxEndpoint, depth: usize) -> EvalFrame {
        if depth >= MAX_EVAL_DEPTH {
            return EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key };
        }
        match ep {
            VfxEndpoint::MediaIn => EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key },
            VfxEndpoint::MediaOut => {
                if let Some(src) = self.incoming(VfxEndpoint::MediaOut, VfxInputSlot::Main) {
                    self.eval_endpoint(src, depth + 1)
                } else {
                    EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key }
                }
            }
            VfxEndpoint::Node(id) => self.eval_node(id, depth),
        }
    }

    fn eval_mask_matte(&mut self, ep: VfxEndpoint, depth: usize) -> (Option<Arc<Vec<f32>>>, u64) {
        if depth >= MAX_EVAL_DEPTH {
            return (None, 0);
        }
        if let VfxEndpoint::Node(mask_id) = ep
            && let Some(node) = self.graph.node(mask_id)
            && let VfxNodeKind::Mask(mask) = &node.kind
        {
            if !node.enabled {
                return (None, 1);
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            mask_id.0.hash(&mut h);
            hash_masks(std::slice::from_ref(mask), self.cx.t, &mut h);
            let key = h.finish();
            if let Some(cached) = self.mask_memo.get(&mask_id) {
                return (cached.clone(), key);
            }
            let cov = crate::mask::effect_coverage(std::slice::from_ref(mask), self.cx.t, self.cx.px_scale, self.media_in.w, self.media_in.h).map(Arc::new);
            self.mask_memo.insert(mask_id, cov.clone());
            return (cov, key);
        }
        // Any image source connected to EffectMask converts alpha (or luma when opaque) into a matte.
        let src = self.eval_endpoint(ep, depth + 1);
        let n = self.media_in.w.saturating_mul(self.media_in.h);
        let pixels = src.img.px.as_chunks::<4>().0;
        let has_alpha_var = pixels.iter().any(|p| p[3] < 0.999);
        let mut matte = Vec::with_capacity(n);
        for p in pixels {
            let v = if has_alpha_var { p[3].clamp(0.0, 1.0) } else { (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).clamp(0.0, 1.0) };
            matte.push(v);
        }
        (Some(Arc::new(matte)), src.key ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn eval_node(&mut self, id: VfxNodeId, depth: usize) -> EvalFrame {
        if let Some(done) = self.memo.get(&id) {
            return done.clone();
        }
        if depth >= MAX_EVAL_DEPTH || !self.visiting.insert(id) {
            return EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key };
        }

        let Some(node) = self.graph.node(id).cloned() else {
            self.visiting.remove(&id);
            return EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key };
        };

        let result = self.eval_node_inner(&node, depth);
        self.visiting.remove(&id);
        self.memo.insert(id, result.clone());
        result
    }

    fn eval_node_inner(&mut self, node: &VfxNode, depth: usize) -> EvalFrame {
        let ep = VfxEndpoint::Node(node.id);
        let main_src = self.incoming(ep, VfxInputSlot::Main);

        // Pass-Through (`Ctrl+P` / disabled) passes Main (or transparent if Merge has none) untouched.
        if !node.enabled {
            return match &node.kind {
                VfxNodeKind::Mask(_) => EvalFrame { img: Arc::new(Image::new(self.media_in.w, self.media_in.h)), key: 0 },
                _ => {
                    main_src.map(|s| self.eval_endpoint(s, depth + 1)).unwrap_or_else(|| EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key })
                }
            };
        }

        match &node.kind {
            VfxNodeKind::PipeRouter => {
                main_src.map(|s| self.eval_endpoint(s, depth + 1)).unwrap_or_else(|| EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key })
            }

            VfxNodeKind::Mask(_) => {
                // Direct viewer inspection of a Mask node renders white matte over black.
                let (matte, key) = self.eval_mask_matte(ep, depth);
                let (w, h) = (self.media_in.w, self.media_in.h);
                let mut out = Image::new(w, h);
                if let Some(cov) = matte {
                    out.px.par_chunks_mut(4).zip(cov.par_iter()).for_each(|(p, &c)| {
                        let c = c.clamp(0.0, 1.0);
                        p[0] = c;
                        p[1] = c;
                        p[2] = c;
                        p[3] = c;
                    });
                }
                EvalFrame { img: Arc::new(out), key }
            }

            VfxNodeKind::Effect(_) | VfxNodeKind::Instance { .. } => self.eval_effect_node(node, main_src, depth),

            VfxNodeKind::Merge { blend_mode, operator, center, size, angle } => {
                self.eval_merge_node(node, *blend_mode, *operator, center, size, angle, main_src, depth)
            }
        }
    }

    fn eval_effect_node(&mut self, node: &VfxNode, main_src: Option<VfxEndpoint>, depth: usize) -> EvalFrame {
        let Some(fx) = self.graph.resolve_effect(node.id) else {
            return main_src.map(|s| self.eval_endpoint(s, depth + 1)).unwrap_or_else(|| EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key });
        };

        // Transform concatenation: combine consecutive upstream unmodified `"transform"` nodes in one warp pass.
        if is_concat_transform(self.graph, node, self.cx).is_some()
            && let Some(VfxEndpoint::Node(prev_id)) = main_src
            && self.graph.node(prev_id).is_some_and(|pn| is_concat_transform(self.graph, pn, self.cx).is_some())
        {
            return self.eval_concat_transforms(node.id, depth);
        }

        let base = main_src.map(|s| self.eval_endpoint(s, depth + 1)).unwrap_or_else(|| EvalFrame { img: Arc::clone(&self.media_in), key: self.media_in_key });

        let mask_ep = self.incoming(VfxEndpoint::Node(node.id), VfxInputSlot::EffectMask);
        let (mask_cov, mask_key) = match mask_ep {
            Some(mep) => self.eval_mask_matte(mep, depth + 1),
            None => (None, 0),
        };

        let mut h = std::collections::hash_map::DefaultHasher::new();
        node.id.0.hash(&mut h);
        base.key.hash(&mut h);
        mask_key.hash(&mut h);
        hash_effect(&fx, self.cx.t, &mut h);
        hash_settings(&node.settings, self.cx.t, &mut h);
        let key = h.finish();

        let use_global_cache = !is_env_sensitive_effect(&fx.effect);
        if use_global_cache && let Some(cached) = cache_get(key, self.media_in.w, self.media_in.h) {
            return EvalFrame { img: cached, key };
        }

        let mut out = (*base.img).clone();
        if fx.effect.starts_with("ec.") && filmcraft_effects::find(&fx.effect).is_some() {
            crate::effectcraft_bridge::apply_chain(&mut out, &[(node.id.0 as usize, &fx)], self.cx);
        } else {
            crate::mask::apply_effect(&mut out, &fx, self.cx);
        }
        apply_node_settings(&base.img, &mut out, &node.settings, mask_cov.as_deref().map(|v| v.as_slice()), self.cx.t);

        let arc = Arc::new(out);
        if use_global_cache {
            cache_put(key, Arc::clone(&arc));
        }
        EvalFrame { img: arc, key }
    }

    fn eval_concat_transforms(&mut self, tail_id: VfxNodeId, depth: usize) -> EvalFrame {
        let mut chain: Vec<EffectInstance> = Vec::new();
        let mut cur = VfxEndpoint::Node(tail_id);
        let mut guard = 0usize;
        while guard < MAX_EVAL_DEPTH {
            guard += 1;
            let VfxEndpoint::Node(id) = cur else { break };
            let Some(node) = self.graph.node(id) else { break };
            let Some(fx) = is_concat_transform(self.graph, node, self.cx) else { break };
            chain.push(fx);
            let Some(prev) = self.incoming(cur, VfxInputSlot::Main) else {
                cur = VfxEndpoint::MediaIn;
                break;
            };
            cur = prev;
        }
        let base = self.eval_endpoint(cur, depth + 1);
        let mut m = Affine::translate(0.0, 0.0);
        let mut op = 1.0f32;
        // `chain` is ordered tail -> head; apply head-first (`rev()`).
        for fx in chain.iter().rev() {
            let (step_m, step_op) = transform_affine_and_opacity(fx, self.cx, base.img.w, base.img.h);
            m = step_m.then_apply(&m);
            op *= step_op;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        base.key.hash(&mut h);
        for v in [m.a, m.b, m.c, m.d, m.e, m.f] {
            v.to_bits().hash(&mut h);
        }
        op.to_bits().hash(&mut h);
        let key = h.finish();
        if let Some(cached) = cache_get(key, base.img.w, base.img.h) {
            return EvalFrame { img: cached, key };
        }
        let mut out = if m.is_identity() { (*base.img).clone() } else { base.img.transformed(base.img.w, base.img.h, &m) };
        if op < 1.0 - 1e-5 {
            out.px.par_chunks_mut(4).for_each(|p| {
                for v in p.iter_mut() {
                    *v *= op;
                }
            });
        }
        let arc = Arc::new(out);
        cache_put(key, Arc::clone(&arc));
        EvalFrame { img: arc, key }
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_merge_node(
        &mut self,
        node: &VfxNode,
        blend_mode: LayerBlendMode,
        operator: MergeOperator,
        center: &Param,
        size: &Param,
        angle: &Param,
        main_src: Option<VfxEndpoint>,
        depth: usize,
    ) -> EvalFrame {
        let ep = VfxEndpoint::Node(node.id);
        let bg = main_src
            .map(|s| self.eval_endpoint(s, depth + 1))
            .unwrap_or_else(|| EvalFrame { img: Arc::new(Image::new(self.media_in.w, self.media_in.h)), key: 0 });
        let fg_src = self.incoming(ep, VfxInputSlot::Foreground);
        let Some(fg_ep) = fg_src else {
            return bg;
        };
        let fg = self.eval_endpoint(fg_ep, depth + 1);
        let mask_ep = self.incoming(ep, VfxInputSlot::EffectMask);
        let (mask_cov, mask_key) = match mask_ep {
            Some(mep) => self.eval_mask_matte(mep, depth + 1),
            None => (None, 0),
        };

        let c_val = center.vec2_at(self.cx.t);
        let s_val = (size.f64_at(self.cx.t) as f32).max(1e-4) as f64;
        let a_val = angle.f64_at(self.cx.t);

        let mut h = std::collections::hash_map::DefaultHasher::new();
        node.id.0.hash(&mut h);
        bg.key.hash(&mut h);
        fg.key.hash(&mut h);
        mask_key.hash(&mut h);
        (blend_mode as u8).hash(&mut h);
        (operator as u8).hash(&mut h);
        c_val.x.to_bits().hash(&mut h);
        c_val.y.to_bits().hash(&mut h);
        s_val.to_bits().hash(&mut h);
        a_val.to_bits().hash(&mut h);
        hash_settings(&node.settings, self.cx.t, &mut h);
        let key = h.finish();

        if let Some(cached) = cache_get(key, bg.img.w, bg.img.h) {
            return EvalFrame { img: cached, key };
        }

        let (w, ht) = (bg.img.w, bg.img.h);
        let needs_xform = (c_val.x - 0.5).abs() > 1e-4 || (c_val.y - 0.5).abs() > 1e-4 || (s_val - 1.0).abs() > 1e-4 || a_val.abs() > 1e-4;
        let fg_xformed: Arc<Image> = if needs_xform {
            let cx_px = c_val.x * w as f64;
            let cy_px = c_val.y * ht as f64;
            let m = Affine::translate(cx_px, cy_px)
                .then_apply(&Affine::rotate_deg(a_val))
                .then_apply(&Affine::scale(s_val, s_val))
                .then_apply(&Affine::translate(-(w as f64) * 0.5, -(ht as f64) * 0.5));
            Arc::new(fg.img.transformed(w, ht, &m))
        } else {
            fg.img
        };

        let mut out = (*bg.img).clone();
        composite_merge(&mut out, &fg_xformed, blend_mode, operator);
        apply_node_settings(&bg.img, &mut out, &node.settings, mask_cov.as_deref().map(|v| v.as_slice()), self.cx.t);

        let arc = Arc::new(out);
        cache_put(key, Arc::clone(&arc));
        EvalFrame { img: arc, key }
    }
}

/// Composite premultiplied `fg` onto premultiplied `bg` in place using `blend_mode` and `operator`.
fn composite_merge(bg: &mut Image, fg: &Image, blend_mode: LayerBlendMode, operator: MergeOperator) {
    let n = bg.px.len().min(fg.px.len());
    bg.px[..n].par_chunks_mut(4).zip(fg.px[..n].par_chunks(4)).for_each(|(b, f)| {
        let ad = b[3].clamp(0.0, 1.0);
        let asrc = f[3].clamp(0.0, 1.0);
        let bg_s = if ad > 1e-7 { [b[0] / ad, b[1] / ad, b[2] / ad] } else { [0.0; 3] };
        let fg_s = if asrc > 1e-7 { [f[0] / asrc, f[1] / asrc, f[2] / asrc] } else { [0.0; 3] };
        let blended = if blend_mode == LayerBlendMode::Normal { fg_s } else { apply_layer_blend_rgb(bg_s, fg_s, blend_mode) };
        let (out_a, w_overlap, w_fg, w_bg) = match operator {
            MergeOperator::Over => (asrc + ad * (1.0 - asrc), asrc * ad, asrc * (1.0 - ad), ad * (1.0 - asrc)),
            MergeOperator::In => (asrc * ad, asrc * ad, 0.0, 0.0),
            MergeOperator::Out => (asrc * (1.0 - ad), 0.0, asrc * (1.0 - ad), 0.0),
            MergeOperator::Atop => (ad, asrc * ad, 0.0, ad * (1.0 - asrc)),
            MergeOperator::Xor => (asrc * (1.0 - ad) + ad * (1.0 - asrc), 0.0, asrc * (1.0 - ad), ad * (1.0 - asrc)),
        };
        for c in 0..3 {
            b[c] = w_overlap * blended[c] + w_fg * fg_s[c] + w_bg * bg_s[c];
        }
        b[3] = out_a.clamp(0.0, 1.0);
    });
}

/// Apply Fusion-style [`VfxNodeSettings`] (`blend`, `EffectMask`, `invert_mask`, `multiply_by_mask`,
/// and `channels`) mixing between `base` (`Main` input) and `out`.
fn apply_node_settings(base: &Image, out: &mut Image, settings: &VfxNodeSettings, mask_cov: Option<&[f32]>, t: Tick) {
    let blend = (settings.blend.f64_at(t) as f32).clamp(0.0, 1.0);
    let ch = settings.channels;
    let fast_identity = mask_cov.is_none() && (blend - 1.0).abs() <= 1e-5 && !settings.multiply_by_mask && ch == [true, true, true, true];
    if fast_identity {
        return;
    }
    let n = out.px.len().min(base.px.len()) / 4;
    out.px[..n * 4].par_chunks_mut(4).zip(base.px[..n * 4].par_chunks(4)).enumerate().for_each(|(i, (o, b))| {
        let m = match mask_cov {
            Some(cov) => {
                let raw = cov.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0);
                if settings.invert_mask { 1.0 - raw } else { raw }
            }
            None => 1.0,
        };
        let k = (blend * m).clamp(0.0, 1.0);
        let mul = if settings.multiply_by_mask && mask_cov.is_some() { m } else { 1.0 };
        for c in 0..4 {
            if !ch[c] {
                o[c] = b[c];
            } else {
                o[c] = (b[c] + (o[c] - b[c]) * k) * mul;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use filmcraft_color::WorkingSpace;
    use filmcraft_project::{
        LayerBlendMode, Mask, MaskKind, MaskMode, MergeOperator, Param, ParamValue, VfxEdge, VfxEndpoint, VfxGraph, VfxInputSlot, VfxNode, VfxNodeKind,
        VfxNodeSettings, VfxPortKind, find_effect,
    };

    use super::*;

    fn cx<'a>() -> FxCtx<'a> {
        FxCtx {
            t: Tick(0),
            px_scale: 1.0,
            seconds: 0.0,
            timecode: "00:00:00:00",
            clip_name: "test",
            project: None,
            env: None,
            frame_rate: 24.0,
            working: WorkingSpace::Rec709,
        }
    }

    fn fx(id: &str) -> EffectInstance {
        find_effect(id).expect("effect exists").instance()
    }

    fn merge_node(id: VfxNodeId, blend_mode: LayerBlendMode, operator: MergeOperator) -> VfxNode {
        VfxNode {
            id,
            kind: VfxNodeKind::Merge {
                blend_mode,
                operator,
                center: Param::new(ParamValue::Vec2(Vec2::new(0.5, 0.5))),
                size: Param::new(ParamValue::Float(1.0)),
                angle: Param::new(ParamValue::Float(0.0)),
            },
            enabled: true,
            label: None,
            settings: VfxNodeSettings::default(),
            pos: [0.0, 0.0],
        }
    }

    #[test]
    fn serial_graph_matches_flat_effect_eval() {
        let mut invert = fx("invert");
        invert.params.entry("blend_with_original".to_string()).and_modify(|p| p.value = ParamValue::Float(0.0));
        let g = VfxGraph::from_serial_effects(&[invert.clone()]);

        let mut img_dag = Image::filled(8, 8, [0.2, 0.4, 0.6, 1.0]);
        let mut img_flat = img_dag.clone();
        apply_vfx_graph_cpu(&g, &mut img_dag, &cx(), VfxEndpoint::MediaOut);
        crate::mask::apply_effect(&mut img_flat, &invert, &cx());
        assert_eq!(img_dag, img_flat);
    }

    #[test]
    fn parallel_branch_merge_and_passthrough() {
        let mut g = VfxGraph::new();
        let inv_id = g.alloc_id();
        g.nodes.push(VfxNode {
            id: inv_id,
            kind: VfxNodeKind::Effect(fx("invert")),
            enabled: true,
            label: None,
            settings: VfxNodeSettings::default(),
            pos: [0.0, 0.0],
        });
        let merge_id = g.alloc_id();
        g.nodes.push(merge_node(merge_id, LayerBlendMode::Add, MergeOperator::Over));

        // Background = MediaIn, Foreground = invert(MediaIn) -> Add should give 1.0 across RGB
        g.connect(VfxEdge { from: VfxEndpoint::MediaIn, to: VfxEndpoint::Node(inv_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::MediaIn, to: VfxEndpoint::Node(merge_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::Node(inv_id), to: VfxEndpoint::Node(merge_id), to_slot: VfxInputSlot::Foreground, kind: VfxPortKind::Image })
            .unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::Node(merge_id), to: VfxEndpoint::MediaOut, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();

        let base = Image::filled(4, 4, [0.02, 0.02, 0.02, 1.0]);
        let mut img = base.clone();
        apply_vfx_graph_cpu(&g, &mut img, &cx(), VfxEndpoint::MediaOut);
        assert!(img.px[0] > 0.5 && img.px[1] > 0.5);

        // Pass-Through on Merge (`Ctrl+P`) outputs Background (`base`) untouched.
        g.node_mut(merge_id).unwrap().enabled = false;
        let mut pt_img = base.clone();
        apply_vfx_graph_cpu(&g, &mut pt_img, &cx(), VfxEndpoint::MediaOut);
        assert_eq!(pt_img, base);
    }

    #[test]
    fn shared_mask_fanout_and_settings_blend_channels() {
        let mut g = VfxGraph::new();
        let mask_id = g.alloc_id();
        let mut mask = Mask::new("box", filmcraft_project::MaskPath::rect(0.0, 0.0, 4.0, 8.0));
        mask.feather = Param::new(ParamValue::Float(0.0));
        mask.mode = MaskMode::Add;
        mask.kind = MaskKind::Path;
        g.nodes.push(VfxNode { id: mask_id, kind: VfxNodeKind::Mask(mask), enabled: true, label: None, settings: VfxNodeSettings::default(), pos: [0.0, 0.0] });

        let fx_id = g.alloc_id();
        let mut inv = fx("invert");
        inv.params.entry("blend_with_original".to_string()).and_modify(|p| p.value = ParamValue::Float(0.0));
        // Only modify Green channel at 50% blend inside left-half mask!
        g.nodes.push(VfxNode {
            id: fx_id,
            kind: VfxNodeKind::Effect(inv),
            enabled: true,
            label: None,
            settings: VfxNodeSettings {
                blend: Param::new(ParamValue::Float(0.5)),
                invert_mask: false,
                multiply_by_mask: false,
                channels: [false, true, false, false],
            },
            pos: [150.0, 0.0],
        });

        g.connect(VfxEdge { from: VfxEndpoint::MediaIn, to: VfxEndpoint::Node(fx_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::Node(mask_id), to: VfxEndpoint::Node(fx_id), to_slot: VfxInputSlot::EffectMask, kind: VfxPortKind::Mask })
            .unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::Node(fx_id), to: VfxEndpoint::MediaOut, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();

        let mut img = Image::filled(8, 8, [0.02, 0.02, 0.02, 1.0]);
        apply_vfx_graph_cpu(&g, &mut img, &cx(), VfxEndpoint::MediaOut);
        // Inside mask (x=1,y=4): R stays 0.02, G changed towards inverted value (~0.70 blended 50% -> ~0.36)
        let inside = img.get(1, 4);
        assert!((inside[0] - 0.02).abs() < 1e-4);
        assert!(inside[1] > 0.25);
        // Outside mask (x=6,y=4): untouched
        let outside = img.get(6, 4);
        assert!((outside[1] - 0.02).abs() < 1e-4);
    }

    #[test]
    fn instance_overrides_and_cycle_guard_never_panic() {
        let mut g = VfxGraph::new();
        let src_id = g.alloc_id();
        let mut inv = fx("invert");
        inv.params.entry("blend_with_original".to_string()).and_modify(|p| p.value = ParamValue::Float(0.0));
        g.nodes.push(VfxNode { id: src_id, kind: VfxNodeKind::Effect(inv), enabled: true, label: None, settings: VfxNodeSettings::default(), pos: [0.0, 0.0] });
        let inst_id = g.alloc_id();
        let mut overrides = BTreeMap::new();
        // Instance overrides blend_with_original to 100% (no-op invert)
        overrides.insert("blend_with_original".to_string(), Param::new(ParamValue::Float(100.0)));
        g.nodes.push(VfxNode {
            id: inst_id,
            kind: VfxNodeKind::Instance { source: src_id, overrides },
            enabled: true,
            label: Some("Inst".into()),
            settings: VfxNodeSettings::default(),
            pos: [200.0, 0.0],
        });
        // Even if corrupted deserialized file contains a cycle, evaluator exits cleanly without stack overflow.
        g.edges.push(VfxEdge { from: VfxEndpoint::Node(inst_id), to: VfxEndpoint::Node(src_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image });
        g.edges.push(VfxEdge { from: VfxEndpoint::Node(src_id), to: VfxEndpoint::Node(inst_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image });
        g.edges.push(VfxEdge { from: VfxEndpoint::Node(inst_id), to: VfxEndpoint::MediaOut, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image });

        let mut img = Image::filled(4, 4, [0.3, 0.3, 0.3, 1.0]);
        apply_vfx_graph_cpu(&g, &mut img, &cx(), VfxEndpoint::MediaOut);
        assert_eq!(img.w, 4);
    }

    #[test]
    fn consecutive_transforms_concatenate_cleanly() {
        let mut g = VfxGraph::new();
        let t1_id = g.alloc_id();
        let mut t1 = fx("transform");
        t1.params.entry("rotation".to_string()).and_modify(|p| p.value = ParamValue::Float(90.0));
        let t2_id = g.alloc_id();
        let mut t2 = fx("transform");
        t2.params.entry("rotation".to_string()).and_modify(|p| p.value = ParamValue::Float(-90.0));
        g.nodes.push(VfxNode { id: t1_id, kind: VfxNodeKind::Effect(t1), enabled: true, label: None, settings: VfxNodeSettings::default(), pos: [0.0, 0.0] });
        g.nodes.push(VfxNode { id: t2_id, kind: VfxNodeKind::Effect(t2), enabled: true, label: None, settings: VfxNodeSettings::default(), pos: [200.0, 0.0] });
        g.connect(VfxEdge { from: VfxEndpoint::MediaIn, to: VfxEndpoint::Node(t1_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::Node(t1_id), to: VfxEndpoint::Node(t2_id), to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();
        g.connect(VfxEdge { from: VfxEndpoint::Node(t2_id), to: VfxEndpoint::MediaOut, to_slot: VfxInputSlot::Main, kind: VfxPortKind::Image }).unwrap();

        let mut base = Image::new(8, 8);
        base.px[4 * (4 * 8 + 4) + 3] = 1.0;
        base.px[4 * (4 * 8 + 4)] = 1.0;
        let mut img = base.clone();
        apply_vfx_graph_cpu(&g, &mut img, &cx(), VfxEndpoint::MediaOut);
        // Net 0° rotation via concatenated matrix should preserve center pixel sharply!
        assert!((img.get(4, 4)[0] - 1.0).abs() < 1e-4);
    }
}
