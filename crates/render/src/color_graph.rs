//! DAG compiler and CPU reference evaluator for [`filmcraft_project::ColorGraph`].
//!
//! Compiles a [`ColorGraph`] (at `Pre-Clip`, `Clip`, `Post-Clip`, or `Timeline` scope) into a
//! bounded slot-based [`ColorDagOp`] executed identically on the CPU ([`apply_color_dag_cpu`]) and
//! on the GPU (`crates/gpu/src/fx.rs`). Pure serial chains without Key cables take the direct
//! [`FxOp::Lumetri`] fast-path.

use std::collections::BTreeMap;
use std::sync::Arc;

use filmcraft_color::{GradeSpace, WorkingSpace, hsl_to_rgb, rgb_to_hsl};
use filmcraft_project::{ColorEndpoint, ColorGraph, ColorNodeId, ColorNodeKind, ColorPortKind, LayerBlendMode, SharedColorNode, SharedNodeId, SplitSpace};

use crate::Image;
use crate::effects::FxCtx;
use crate::gpufx::{FxOp, LumetriOp};

/// Maximum virtual texture register slots allocated by a compiled [`ColorDagOp`].
pub const MAX_DAG_SLOTS: usize = 32;

/// Compiled, topologically ordered execution plan for a [`ColorGraph`].
#[derive(Clone, Debug, PartialEq)]
pub struct ColorDagOp {
    pub instrs: Vec<DagInstr>,
    /// Virtual RGB slot containing the final output image (0 = input source unchanged).
    pub out_rgb: u8,
    /// Number of virtual slots (`1..=MAX_DAG_SLOTS`) referenced by `instrs`.
    pub num_slots: u8,
}

/// One node evaluation or channel operation in a [`ColorDagOp`].
#[derive(Clone, Debug, PartialEq)]
pub enum DagInstr {
    /// Evaluate a `Corrector`, `Outside`, or `Shared` node.
    Corrector {
        in_rgb: u8,
        in_key: Option<(u8, bool)>,
        out_rgb: u8,
        out_key: Option<u8>,
        op: Box<LumetriOp>,
        key_input_gain: f32,
        key_output_gain: f32,
        key_invert: bool,
    },
    /// Combine parallel branches (`base_rgb = Some(s)` uses additive delta from common parent `s`;
    /// `None` uses uniform average).
    ParallelCombine { base_rgb: Option<u8>, branches: Vec<u8>, out_rgb: u8 },
    /// Composite branches in layer priority order (0 = bottom, 1.. = top layers with optional key matte).
    LayerComposite { branches: Vec<(u8, Option<(u8, bool)>, LayerBlendMode, f32)>, out_rgb: u8 },
    /// Extract channel `0`, `1`, or `2` of `in_rgb` in `space` into a monochrome `[v, v, v, a]` slot.
    SplitChannel { in_rgb: u8, space: SplitSpace, channel: u8, out_rgb: u8 },
    /// Recombine 3 monochrome channel slots `in_ch = [ch0, ch1, ch2]` in `space` into an RGB slot.
    CombineChannels { in_ch: [u8; 3], space: SplitSpace, out_rgb: u8 },
}

fn eval_lumetri_for_dag(fx: &filmcraft_project::EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<Box<LumetriOp>> {
    match FxOp::eval_masked(fx, cx, w, h).filter(FxOp::gpu_ok)? {
        FxOp::Lumetri(lum) => Some(lum),
        FxOp::Masked { inner, mask } => match *inner {
            FxOp::Lumetri(mut lum) => {
                lum.node_mask = Some(mask);
                Some(lum)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Compile a [`ColorGraph`] into GPU-capable [`FxOp`]s (using the direct linear fast-path when
/// possible, or a single [`FxOp::ColorDag`] when the graph has branches, `Outside` nodes, mixers,
/// splitters, or Key cables). Returns `None` if an active node requires a CPU-only feature (e.g. HDR).
#[allow(clippy::cognitive_complexity)]
pub fn compile_color_graph(graph: &ColorGraph, shared: &BTreeMap<SharedNodeId, SharedColorNode>, cx: &FxCtx, w: usize, h: usize) -> Option<Vec<FxOp>> {
    if !graph.has_enabled_nodes() {
        return Some(Vec::new());
    }
    if let Some(linear) = graph.as_linear_effects(shared) {
        let mut ops = Vec::with_capacity(linear.len());
        for e in &linear {
            if !e.enabled {
                continue;
            }
            let op = FxOp::eval_masked(e, cx, w, h).filter(FxOp::gpu_ok)?;
            ops.push(op);
        }
        return Some(ops);
    }

    let order = graph.topo_order().ok()?;
    let mut next_slot: u8 = 1; // Slot 0 is reserved for `ColorEndpoint::Source` RGB
    let mut alloc_slot = || -> Option<u8> {
        if (next_slot as usize) >= MAX_DAG_SLOTS {
            return None;
        }
        let s = next_slot;
        next_slot += 1;
        Some(s)
    };

    let mut rgb_out_slots: BTreeMap<(ColorEndpoint, u8), u8> = BTreeMap::new();
    let mut key_out_slots: BTreeMap<(ColorEndpoint, u8), u8> = BTreeMap::new();
    let mut node_in_rgb: BTreeMap<ColorNodeId, u8> = BTreeMap::new();
    rgb_out_slots.insert((ColorEndpoint::Source, 0), 0);

    let find_in_rgb = |to: ColorEndpoint, to_port: u8, slots: &BTreeMap<(ColorEndpoint, u8), u8>| -> Option<u8> {
        graph.edges.iter().find(|e| e.kind == ColorPortKind::Rgb && e.to == to && e.to_port == to_port).and_then(|e| slots.get(&(e.from, e.from_port)).copied())
    };
    let find_in_key = |to: ColorEndpoint, to_port: u8, slots: &BTreeMap<(ColorEndpoint, u8), u8>| -> Option<(u8, bool)> {
        graph
            .edges
            .iter()
            .find(|e| e.kind == ColorPortKind::Key && e.to == to && e.to_port == to_port)
            .and_then(|e| slots.get(&(e.from, e.from_port)).copied().map(|s| (s, e.invert_key)))
    };
    let has_downstream_key = |from: ColorNodeId| -> bool {
        let ep = ColorEndpoint::Node(from);
        graph.edges.iter().any(|e| e.kind == ColorPortKind::Key && e.from == ep)
    };

    let mut instrs = Vec::new();
    let mut last_rgb_slot = 0u8;

    for id in order {
        let node = graph.node(id)?;
        let ep = ColorEndpoint::Node(id);
        if !node.enabled {
            let passthrough_rgb = find_in_rgb(ep, 0, &rgb_out_slots).unwrap_or(0);
            for p in 0..node.kind.rgb_output_ports() {
                rgb_out_slots.insert((ep, p), passthrough_rgb);
            }
            if let Some((ks, _)) = find_in_key(ep, 0, &key_out_slots) {
                key_out_slots.insert((ep, 0), ks);
            }
            continue;
        }

        match &node.kind {
            ColorNodeKind::Corrector { effect, key_input_gain, key_output_gain, key_invert }
            | ColorNodeKind::Outside { effect, key_input_gain, key_output_gain, key_invert } => {
                let in_rgb = find_in_rgb(ep, 0, &rgb_out_slots).unwrap_or(0);
                node_in_rgb.insert(id, in_rgb);
                let in_key = find_in_key(ep, 0, &key_out_slots);
                let mut fx = (**effect).clone();
                fx.enabled = true;
                let lum = eval_lumetri_for_dag(&fx, cx, w, h)?;
                let need_key_out = has_downstream_key(id)
                    || in_key.is_some()
                    || *key_invert
                    || (*key_output_gain - 1.0).abs() > 1e-4
                    || lum.hsl.as_ref().is_some_and(|h| h.key_scope == 0)
                    || lum.node_mask.is_some();
                let out_key = if need_key_out { Some(alloc_slot()?) } else { None };
                let out_rgb = alloc_slot()?;
                instrs.push(DagInstr::Corrector {
                    in_rgb,
                    in_key,
                    out_rgb,
                    out_key,
                    op: lum,
                    key_input_gain: *key_input_gain,
                    key_output_gain: *key_output_gain,
                    key_invert: *key_invert,
                });
                rgb_out_slots.insert((ep, 0), out_rgb);
                if let Some(ks) = out_key {
                    key_out_slots.insert((ep, 0), ks);
                }
                last_rgb_slot = out_rgb;
            }
            ColorNodeKind::Shared { shared_id, key_input_gain, key_output_gain, key_invert } => {
                let in_rgb = find_in_rgb(ep, 0, &rgb_out_slots).unwrap_or(0);
                node_in_rgb.insert(id, in_rgb);
                let in_key = find_in_key(ep, 0, &key_out_slots);
                let Some(shared_node) = shared.get(shared_id) else {
                    rgb_out_slots.insert((ep, 0), in_rgb);
                    continue;
                };
                let mut fx = shared_node.effect.clone();
                fx.enabled = true;
                let lum = eval_lumetri_for_dag(&fx, cx, w, h)?;
                let need_key_out = has_downstream_key(id)
                    || in_key.is_some()
                    || *key_invert
                    || (*key_output_gain - 1.0).abs() > 1e-4
                    || lum.hsl.as_ref().is_some_and(|h| h.key_scope == 0)
                    || lum.node_mask.is_some();
                let out_key = if need_key_out { Some(alloc_slot()?) } else { None };
                let out_rgb = alloc_slot()?;
                instrs.push(DagInstr::Corrector {
                    in_rgb,
                    in_key,
                    out_rgb,
                    out_key,
                    op: lum,
                    key_input_gain: *key_input_gain,
                    key_output_gain: *key_output_gain,
                    key_invert: *key_invert,
                });
                rgb_out_slots.insert((ep, 0), out_rgb);
                if let Some(ks) = out_key {
                    key_out_slots.insert((ep, 0), ks);
                }
                last_rgb_slot = out_rgb;
            }
            ColorNodeKind::ParallelMixer { inputs } => {
                let max_p = (*inputs).clamp(2, filmcraft_project::MAX_MIXER_INPUTS) as u8;
                let mut branches = Vec::new();
                let mut upstream_parents = Vec::new();
                for p in 0..max_p {
                    if let Some(edge) = graph.edges.iter().find(|e| e.kind == ColorPortKind::Rgb && e.to == ep && e.to_port == p)
                        && let Some(&bs) = rgb_out_slots.get(&(edge.from, edge.from_port))
                    {
                        branches.push(bs);
                        let parent = match edge.from {
                            ColorEndpoint::Node(bid) => node_in_rgb.get(&bid).copied(),
                            ColorEndpoint::Source => Some(0),
                            ColorEndpoint::Output => None,
                        };
                        upstream_parents.push(parent);
                    }
                }
                if branches.is_empty() {
                    rgb_out_slots.insert((ep, 0), 0);
                } else if branches.len() == 1 {
                    rgb_out_slots.insert((ep, 0), branches[0]);
                    last_rgb_slot = branches[0];
                } else {
                    let first_parent = upstream_parents.first().copied().flatten();
                    let common_base = first_parent.filter(|&fp| upstream_parents.iter().all(|p| *p == Some(fp)));
                    let out_rgb = alloc_slot()?;
                    instrs.push(DagInstr::ParallelCombine { base_rgb: common_base, branches, out_rgb });
                    rgb_out_slots.insert((ep, 0), out_rgb);
                    last_rgb_slot = out_rgb;
                }
            }
            ColorNodeKind::LayerMixer { inputs, blend_modes, gains } => {
                let max_p = (*inputs).clamp(2, filmcraft_project::MAX_MIXER_INPUTS) as u8;
                let mut branches = Vec::new();
                let mut top_key_slot = None;
                for p in 0..max_p {
                    if let Some(rgb_s) = find_in_rgb(ep, p, &rgb_out_slots) {
                        let key_s = find_in_key(ep, p, &key_out_slots);
                        if let Some((ks, _)) = key_s {
                            top_key_slot = Some(ks);
                        }
                        let mode = blend_modes.get(p as usize).copied().unwrap_or(LayerBlendMode::Normal);
                        let gain = gains.get(p as usize).copied().unwrap_or(1.0).clamp(0.0, 1.0);
                        branches.push((rgb_s, key_s, mode, gain));
                    }
                }
                if branches.is_empty() {
                    rgb_out_slots.insert((ep, 0), 0);
                } else if branches.len() == 1 {
                    rgb_out_slots.insert((ep, 0), branches[0].0);
                    last_rgb_slot = branches[0].0;
                } else {
                    let out_rgb = alloc_slot()?;
                    instrs.push(DagInstr::LayerComposite { branches, out_rgb });
                    rgb_out_slots.insert((ep, 0), out_rgb);
                    if let Some(ks) = top_key_slot {
                        key_out_slots.insert((ep, 0), ks);
                    }
                    last_rgb_slot = out_rgb;
                }
            }
            ColorNodeKind::Splitter { space } => {
                let in_rgb = find_in_rgb(ep, 0, &rgb_out_slots).unwrap_or(0);
                for ch in 0..3u8 {
                    let out_rgb = alloc_slot()?;
                    instrs.push(DagInstr::SplitChannel { in_rgb, space: *space, channel: ch, out_rgb });
                    rgb_out_slots.insert((ep, ch), out_rgb);
                    last_rgb_slot = out_rgb;
                }
            }
            ColorNodeKind::Combiner { space } => {
                let c0 = find_in_rgb(ep, 0, &rgb_out_slots).unwrap_or(0);
                let c1 = find_in_rgb(ep, 1, &rgb_out_slots).unwrap_or(c0);
                let c2 = find_in_rgb(ep, 2, &rgb_out_slots).unwrap_or(c0);
                let out_rgb = alloc_slot()?;
                instrs.push(DagInstr::CombineChannels { in_ch: [c0, c1, c2], space: *space, out_rgb });
                rgb_out_slots.insert((ep, 0), out_rgb);
                last_rgb_slot = out_rgb;
            }
        }
    }

    let out_rgb = find_in_rgb(ColorEndpoint::Output, 0, &rgb_out_slots).unwrap_or(last_rgb_slot);
    if instrs.is_empty() || out_rgb == 0 {
        return Some(Vec::new());
    }
    Some(vec![FxOp::ColorDag(Arc::new(ColorDagOp { instrs, out_rgb, num_slots: next_slot }))])
}

/// Apply a compiled [`ColorDagOp`] to `img` on the CPU (exact reference for GPU [`ColorDagOp`]).
#[allow(clippy::cognitive_complexity)]
pub fn apply_color_dag_cpu(dag: &ColorDagOp, img: &mut Image) {
    let (w, h) = (img.w, img.h);
    let n_px = w.saturating_mul(h);
    if n_px == 0 || dag.instrs.is_empty() {
        return;
    }
    let num_slots = (dag.num_slots as usize).clamp(1, MAX_DAG_SLOTS);
    let mut slots: Vec<Option<Image>> = vec![None; num_slots];
    slots[0] = Some(img.clone());
    let gs_sdr = GradeSpace::new(WorkingSpace::Rec709, 100.0);

    for instr in &dag.instrs {
        match instr {
            DagInstr::Corrector { in_rgb, in_key, out_rgb, out_key, op, key_input_gain, key_output_gain, key_invert } => {
                let Some(src_img) = slots.get(*in_rgb as usize).and_then(|s| s.as_ref()).cloned() else {
                    continue;
                };
                let is_node_key = op.hsl.as_ref().is_some_and(|h| h.key_scope == 0);
                let is_legacy_sec = op.hsl.as_ref().is_some_and(|h| h.key_scope == 1);
                let has_local_mask = is_node_key || op.node_mask.is_some();
                let need_key = out_key.is_some()
                    || has_local_mask
                    || in_key.is_some()
                    || *key_invert
                    || (*key_output_gain * op.key_gain) < 0.9999
                    || (!is_legacy_sec && op.show_mask > 0);

                // 1. Compute combined node key matte if needed
                let matte: Option<Vec<f32>> = if need_key {
                    let qual_m = if is_node_key && let Some(hsl) = op.hsl.as_deref() {
                        Some(crate::effects::compute_qualifier_matte(&src_img, hsl, &gs_sdr))
                    } else {
                        None
                    };
                    let win_cov = op.node_mask.as_ref().filter(|m| m.cov.len() == n_px).map(|m| m.cov.as_slice());
                    let up_key_img = in_key.and_then(|(ks, _)| slots.get(ks as usize).and_then(|s| s.as_ref()));
                    let inv_edge = in_key.is_some_and(|(_, inv)| inv);
                    let in_gain = key_input_gain.clamp(0.0, 2.0);
                    let out_gain = (*key_output_gain * op.key_gain).clamp(0.0, 1.0);
                    let mut m_vec = Vec::with_capacity(n_px);
                    for i in 0..n_px {
                        let mut k = 1.0f32;
                        if let Some(qm) = &qual_m {
                            k *= qm.get(i).copied().unwrap_or(0.0);
                        }
                        if let Some(wc) = win_cov {
                            k *= wc.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                        }
                        if let Some(uk_img) = up_key_img {
                            let mut uk = uk_img.px.get(i * 4).copied().unwrap_or(1.0).clamp(0.0, 1.0);
                            if inv_edge {
                                uk = 1.0 - uk;
                            }
                            uk = (uk * in_gain).clamp(0.0, 1.0);
                            k *= uk;
                        }
                        if *key_invert {
                            k = 1.0 - k;
                        }
                        k = (k * out_gain).clamp(0.0, 1.0);
                        m_vec.push(k);
                    }
                    if let Some(ks) = *out_key
                        && (ks as usize) < slots.len()
                    {
                        let mut k_img = Image::new(w, h);
                        for (i, &kv) in m_vec.iter().enumerate() {
                            if let Some(px) = k_img.px.get_mut(i * 4..i * 4 + 4) {
                                px.copy_from_slice(&[kv, kv, kv, 1.0]);
                            }
                        }
                        slots[ks as usize] = Some(k_img);
                    }
                    Some(m_vec)
                } else {
                    None
                };

                // 2. Run unmasked Lumetri RGB passes on a working copy of `src_img`
                let mut graded = src_img.clone();
                let mut unmasked_op = (**op).clone();
                unmasked_op.node_mask = None;
                unmasked_op.key_gain = 1.0;
                if !is_legacy_sec {
                    unmasked_op.show_mask = 0;
                }
                if is_node_key && let Some(hsl) = unmasked_op.hsl.as_mut() {
                    // Secondary tint still runs inside `FxOp::Lumetri`, but outer key blend is handled below
                    let has_sec_tint = hsl.htemp.abs() > 1e-4 || hsl.htint.abs() > 1e-4 || (hsl.hsat - 1.0).abs() > 1e-4 || hsl.hshift.abs() > 1e-4;
                    if !has_sec_tint {
                        unmasked_op.hsl = None;
                    }
                }
                // Apply unmasked RGB grade (note: if `is_node_key` is still present for secondary tint,
                // we pass `key_scope = 0` with full matte applied in step 3)
                if is_node_key && unmasked_op.hsl.is_some() {
                    let saved_hsl = unmasked_op.hsl.take();
                    FxOp::Lumetri(Box::new(unmasked_op)).apply(&mut graded);
                    if let Some(hsl) = saved_hsl {
                        graded.map_rgb(|c, _, _| {
                            let u = c.map(|q| crate::gpufx::linear_to_enc_pub(q).clamp(0.0, 1.0));
                            crate::effects::apply_hsl_secondary_color_unit(u, &hsl).map(crate::gpufx::enc_to_linear_pub)
                        });
                    }
                } else {
                    FxOp::Lumetri(Box::new(unmasked_op)).apply(&mut graded);
                }

                // 3. Mix graded RGB against `src_img` using `matte` (and `op.show_mask` if active)
                if let Some(m_vec) = &matte {
                    let show_mask = if !is_legacy_sec { op.show_mask } else { 0 };
                    crate::effects::apply_node_key_output(&mut graded, &src_img, Some(m_vec.as_slice()), None, show_mask, 1.0, &gs_sdr);
                }
                if (*out_rgb as usize) < slots.len() {
                    slots[*out_rgb as usize] = Some(graded);
                }
            }
            DagInstr::ParallelCombine { base_rgb, branches, out_rgb } => {
                let Some(first_slot) = branches.first().copied() else { continue };
                let Some(mut acc) = slots.get(first_slot as usize).and_then(|s| s.as_ref()).cloned() else {
                    continue;
                };
                let base_img = base_rgb.and_then(|bs| slots.get(bs as usize).and_then(|s| s.as_ref()));
                for (idx, &b_slot) in branches.iter().enumerate().skip(1) {
                    let Some(b_img) = slots.get(b_slot as usize).and_then(|s| s.as_ref()) else {
                        continue;
                    };
                    if let Some(base) = base_img {
                        for i in 0..n_px {
                            for c in 0..3 {
                                let a = acc.px.get(i * 4 + c).copied().unwrap_or(0.0);
                                let br = b_img.px.get(i * 4 + c).copied().unwrap_or(0.0);
                                let bs = base.px.get(i * 4 + c).copied().unwrap_or(0.0);
                                if let Some(dst) = acc.px.get_mut(i * 4 + c) {
                                    *dst = (a + (br - bs)).max(0.0);
                                }
                            }
                        }
                    } else {
                        let w_branch = 1.0 / (idx + 1) as f32;
                        for i in 0..n_px {
                            for c in 0..3 {
                                let a = acc.px.get(i * 4 + c).copied().unwrap_or(0.0);
                                let br = b_img.px.get(i * 4 + c).copied().unwrap_or(0.0);
                                if let Some(dst) = acc.px.get_mut(i * 4 + c) {
                                    *dst = a + (br - a) * w_branch;
                                }
                            }
                        }
                    }
                }
                if (*out_rgb as usize) < slots.len() {
                    slots[*out_rgb as usize] = Some(acc);
                }
            }
            DagInstr::LayerComposite { branches, out_rgb } => {
                let Some(&(first_rgb, _, _, _)) = branches.first() else { continue };
                let Some(mut acc) = slots.get(first_rgb as usize).and_then(|s| s.as_ref()).cloned() else {
                    continue;
                };
                for &(top_rgb, top_key, mode, gain) in branches.iter().skip(1) {
                    let Some(top_img) = slots.get(top_rgb as usize).and_then(|s| s.as_ref()) else {
                        continue;
                    };
                    let key_img = top_key.and_then(|(ks, _)| slots.get(ks as usize).and_then(|s| s.as_ref()));
                    let inv_key = top_key.is_some_and(|(_, inv)| inv);
                    for i in 0..n_px {
                        let mut k = match key_img {
                            Some(ki) => ki.px.get(i * 4).copied().unwrap_or(1.0).clamp(0.0, 1.0),
                            None => 1.0,
                        };
                        if inv_key {
                            k = 1.0 - k;
                        }
                        k = (k * gain).clamp(0.0, 1.0);
                        let bot = [
                            acc.px.get(i * 4).copied().unwrap_or(0.0),
                            acc.px.get(i * 4 + 1).copied().unwrap_or(0.0),
                            acc.px.get(i * 4 + 2).copied().unwrap_or(0.0),
                        ];
                        let top = [
                            top_img.px.get(i * 4).copied().unwrap_or(0.0),
                            top_img.px.get(i * 4 + 1).copied().unwrap_or(0.0),
                            top_img.px.get(i * 4 + 2).copied().unwrap_or(0.0),
                        ];
                        let blended = apply_layer_blend_rgb(bot, top, mode);
                        for c in 0..3 {
                            if let Some(dst) = acc.px.get_mut(i * 4 + c) {
                                *dst = bot[c] + (blended[c] - bot[c]) * k;
                            }
                        }
                    }
                }
                if (*out_rgb as usize) < slots.len() {
                    slots[*out_rgb as usize] = Some(acc);
                }
            }
            DagInstr::SplitChannel { in_rgb, space, channel, out_rgb } => {
                let Some(src_img) = slots.get(*in_rgb as usize).and_then(|s| s.as_ref()) else {
                    continue;
                };
                let mut ch_img = Image::new(w, h);
                for i in 0..n_px {
                    let r = src_img.px.get(i * 4).copied().unwrap_or(0.0);
                    let g = src_img.px.get(i * 4 + 1).copied().unwrap_or(0.0);
                    let b = src_img.px.get(i * 4 + 2).copied().unwrap_or(0.0);
                    let a = src_img.px.get(i * 4 + 3).copied().unwrap_or(1.0);
                    let v = split_pixel_channel([r, g, b], *space, *channel);
                    if let Some(dst) = ch_img.px.get_mut(i * 4..i * 4 + 4) {
                        dst.copy_from_slice(&[v, v, v, a]);
                    }
                }
                if (*out_rgb as usize) < slots.len() {
                    slots[*out_rgb as usize] = Some(ch_img);
                }
            }
            DagInstr::CombineChannels { in_ch, space, out_rgb } => {
                let Some(img0) = slots.get(in_ch[0] as usize).and_then(|s| s.as_ref()) else { continue };
                let Some(img1) = slots.get(in_ch[1] as usize).and_then(|s| s.as_ref()) else { continue };
                let Some(img2) = slots.get(in_ch[2] as usize).and_then(|s| s.as_ref()) else { continue };
                let mut comb = Image::new(w, h);
                for i in 0..n_px {
                    let c0 = img0.px.get(i * 4).copied().unwrap_or(0.0);
                    let c1 = img1.px.get(i * 4).copied().unwrap_or(0.0);
                    let c2 = img2.px.get(i * 4).copied().unwrap_or(0.0);
                    let a = img0.px.get(i * 4 + 3).copied().unwrap_or(1.0);
                    let [r, g, b] = combine_pixel_channels([c0, c1, c2], *space);
                    if let Some(dst) = comb.px.get_mut(i * 4..i * 4 + 4) {
                        dst.copy_from_slice(&[r, g, b, a]);
                    }
                }
                if (*out_rgb as usize) < slots.len() {
                    slots[*out_rgb as usize] = Some(comb);
                }
            }
        }
    }

    if let Some(final_img) = slots.get(dag.out_rgb as usize).and_then(|s| s.as_ref()) {
        img.px.copy_from_slice(&final_img.px);
    }
}

/// Evaluate a [`ColorGraph`] on `img` on the CPU (handles both SDR compiled DAGs and HDR fallback).
pub fn apply_color_graph_cpu(graph: &ColorGraph, shared: &BTreeMap<SharedNodeId, SharedColorNode>, img: &mut Image, cx: &FxCtx) {
    if !graph.has_enabled_nodes() {
        return;
    }
    if let Some(ops) = compile_color_graph(graph, shared, cx, img.w, img.h) {
        for op in &ops {
            op.apply(img);
        }
        return;
    }
    // HDR or non-GPU fallback: evaluate serial/corrector effects in topological order
    for fx in graph.corrector_effects() {
        if fx.enabled {
            crate::mask::apply_effect(img, &fx, cx);
        }
    }
}

#[inline]
pub fn split_pixel_channel(rgb: [f32; 3], space: SplitSpace, channel: u8) -> f32 {
    match space {
        SplitSpace::Rgb => rgb.get(channel as usize).copied().unwrap_or(0.0),
        SplitSpace::YCbCr => {
            let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            match channel {
                0 => y,
                1 => ((rgb[2] - y) / 1.8556) + 0.5,
                _ => ((rgb[0] - y) / 1.5748) + 0.5,
            }
        }
        SplitSpace::Hsl => {
            let hsl = rgb_to_hsl(rgb[0].clamp(0.0, 1.0), rgb[1].clamp(0.0, 1.0), rgb[2].clamp(0.0, 1.0));
            hsl.get(channel as usize).copied().unwrap_or(0.0)
        }
    }
}

#[inline]
pub fn combine_pixel_channels(c: [f32; 3], space: SplitSpace) -> [f32; 3] {
    match space {
        SplitSpace::Rgb => c,
        SplitSpace::YCbCr => {
            let y = c[0];
            let cb = c[1] - 0.5;
            let cr = c[2] - 0.5;
            let r = (y + 1.5748 * cr).max(0.0);
            let b = (y + 1.8556 * cb).max(0.0);
            let g = ((y - 0.2126 * r - 0.0722 * b) / 0.7152).max(0.0);
            [r, g, b]
        }
        SplitSpace::Hsl => hsl_to_rgb(c[0].rem_euclid(1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)),
    }
}

#[inline]
pub fn apply_layer_blend_rgb(bot: [f32; 3], top: [f32; 3], mode: LayerBlendMode) -> [f32; 3] {
    let ch = |b: f32, t: f32| -> f32 {
        match mode {
            LayerBlendMode::Normal => t,
            LayerBlendMode::Multiply => b * t,
            LayerBlendMode::Screen => {
                let bc = b.clamp(0.0, 1.0);
                let tc = t.clamp(0.0, 1.0);
                1.0 - (1.0 - bc) * (1.0 - tc)
            }
            LayerBlendMode::Overlay => {
                let bc = b.clamp(0.0, 1.0);
                let tc = t.clamp(0.0, 1.0);
                if bc < 0.5 { 2.0 * bc * tc } else { 1.0 - 2.0 * (1.0 - bc) * (1.0 - tc) }
            }
            LayerBlendMode::SoftLight => {
                let bc = b.clamp(0.0, 1.0);
                let tc = t.clamp(0.0, 1.0);
                (1.0 - 2.0 * tc) * bc * bc + 2.0 * tc * bc
            }
            LayerBlendMode::HardLight => {
                let bc = b.clamp(0.0, 1.0);
                let tc = t.clamp(0.0, 1.0);
                if tc < 0.5 { 2.0 * bc * tc } else { 1.0 - 2.0 * (1.0 - bc) * (1.0 - tc) }
            }
            LayerBlendMode::Add => b + t,
            LayerBlendMode::Subtract => (b - t).max(0.0),
            LayerBlendMode::Darken => b.min(t),
            LayerBlendMode::Lighten => b.max(t),
            LayerBlendMode::Color | LayerBlendMode::Luminosity => t,
        }
    };
    match mode {
        LayerBlendMode::Color => {
            let bh = rgb_to_hsl(bot[0].clamp(0.0, 1.0), bot[1].clamp(0.0, 1.0), bot[2].clamp(0.0, 1.0));
            let th = rgb_to_hsl(top[0].clamp(0.0, 1.0), top[1].clamp(0.0, 1.0), top[2].clamp(0.0, 1.0));
            hsl_to_rgb(th[0], th[1], bh[2])
        }
        LayerBlendMode::Luminosity => {
            let bh = rgb_to_hsl(bot[0].clamp(0.0, 1.0), bot[1].clamp(0.0, 1.0), bot[2].clamp(0.0, 1.0));
            let th = rgb_to_hsl(top[0].clamp(0.0, 1.0), top[1].clamp(0.0, 1.0), top[2].clamp(0.0, 1.0));
            hsl_to_rgb(bh[0], bh[1], th[2])
        }
        _ => [ch(bot[0], top[0]), ch(bot[1], top[1]), ch(bot[2], top[2])],
    }
}
