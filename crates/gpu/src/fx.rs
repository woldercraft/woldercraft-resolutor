//! The GPU effect stage: standard effects of a plan layer ([`LayerFx`]) run in WGSL (`fx.wgsl`).
//!
//! A layer with effects is first drawn from its source (YUV planes or RGBA, through the
//! compositor's own `fs` shader, so decoding is the same as for a plain layer) into a working
//! texture of the size the CPU decodes it at; each [`FxOp`] then runs as compute passes that
//! ping-pong between two working textures (three for Unsharp Mask, which keeps the original);
//! the last one is drawn into the accumulator with the layer's matrix, opacity and blend mode
//! like any RGBA layer. Working textures are `Rgba32Float` (the CPU reference's precision) and
//! pooled per size; layers without effects never touch any of this.
//!
//! Box blurs (Gaussian Blur, Camera Blur, the blur inside Unsharp / Sharpen) run per pixel for
//! radii up to [`BOX_PER_PIXEL_MAX`] and as a running sum per row / column above, so a huge
//! radius costs O(1) per pixel like on the CPU (radii are capped by `gaussian_boxes`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use filmcraft_render::effects::{CW_LUT_H, CW_LUT_W, ColorWarperTable};
use filmcraft_render::gpufx::{FxOp, LUMETRI_CURVE_LEN, LumetriCurveTable, MaskCoverage};
use filmcraft_render::plan::LayerFx;

/// Working texture format.
pub(crate) const FX_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// Box radii up to this many pixels are summed per output pixel; larger ones as a running sum.
pub const BOX_PER_PIXEL_MAX: u32 = 32;

const OP_BRIGHTNESS_CONTRAST: u32 = 1;
const OP_PROC_AMP: u32 = 2;
const OP_TINT: u32 = 3;
const OP_BLACK_WHITE: u32 = 4;
const OP_COLOR_BALANCE: u32 = 5;
const OP_LEAVE_COLOR: u32 = 6;
const OP_CHANGE_TO_COLOR: u32 = 7;
const OP_COLOR_PASS: u32 = 8;
const OP_GAMMA: u32 = 9;
const OP_LEVELS: u32 = 10;
const OP_EXTRACT: u32 = 11;
const OP_INVERT: u32 = 12;
const OP_INVERT_ALPHA: u32 = 13;
const OP_POSTERIZE: u32 = 14;
const OP_ASC_CDL: u32 = 15;
const OP_CHANNEL_MIX: u32 = 16;
const OP_COLOR_REPLACE: u32 = 17;
const OP_ALPHA_ADJUST: u32 = 18;
const OP_VIGNETTE: u32 = 19;
const OP_BOX: u32 = 20;
const OP_DIRECTIONAL: u32 = 22;
const OP_UNSHARP: u32 = 23;
const OP_CROP: u32 = 24;
const OP_RESAMPLE: u32 = 25;
const OP_HFLIP: u32 = 26;
const OP_VFLIP: u32 = 27;
const OP_MIRROR: u32 = 28;
const OP_OFFSET: u32 = 29;
const OP_VIDEO_LIMITER: u32 = 30;
const OP_LUMETRI: u32 = 31;
const OP_BOKEH_DOWN: u32 = 32;
const OP_BOKEH_BOOST: u32 = 33;
const OP_BOKEH: u32 = 34;
const OP_BOKEH_UP: u32 = 35;
const OP_THREE_WAY: u32 = 36;
const OP_COLOR_WHEELS: u32 = 37;
const OP_LUMETRI_LOOK: u32 = 38;
const OP_LUT_3D: u32 = 39;
const OP_LUMETRI_CURVES: u32 = 40;
const OP_HSL_KEY: u32 = 41;
const OP_HSL_MEDIAN: u32 = 42;
const OP_HSL_APPLY: u32 = 43;
const OP_MASK_MIX: u32 = 44;
const OP_MIDTONE_DETAIL: u32 = 45;
const OP_COLOR_SLICE: u32 = 46;
const OP_COLOR_WARPER: u32 = 47;
const OP_HSL_MORPH: u32 = 48;
const OP_NODE_KEY_FINISH: u32 = 49;
const OP_RESTORE_ORIG: u32 = 50;
const OP_NODE_KEY_MIX: u32 = 51;
const OP_DAG_KEY_COMBINE: u32 = 52;
const OP_DAG_PARALLEL: u32 = 53;
const OP_DAG_LAYER: u32 = 54;
const OP_DAG_SPLIT: u32 = 55;
const OP_DAG_COMBINE: u32 = 56;
const OP_LOG_WHEELS: u32 = 57;
const OP_HDR_WHEELS: u32 = 58;
const OP_RGB_MIXER: u32 = 59;
const OP_CST: u32 = 60;
const OP_NODE_SIZING: u32 = 61;

type Target = (wgpu::Texture, wgpu::TextureView);

struct CachedTex<T> {
    _keep: Arc<T>,
    _tex: wgpu::Texture,
    view: wgpu::TextureView,
    last_used: u64,
}

fn evict_lru_tex<T>(cache: &mut HashMap<usize, CachedTex<T>>, limit: usize) {
    while cache.len() >= limit {
        let Some(&oldest_key) = cache.iter().min_by_key(|(_, v)| v.last_used).map(|(k, _)| k) else {
            break;
        };
        cache.remove(&oldest_key);
    }
}

pub(crate) struct FxStage {
    px: wgpu::ComputePipeline,
    run: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    /// Draws a layer's source into a working texture (`composite.wgsl` `fs`, no blending).
    source: wgpu::RenderPipeline,
    pool: HashMap<(u32, u32), Vec<Target>>,
    used: HashSet<(u32, u32)>,
    lut_cache: HashMap<usize, CachedTex<filmcraft_color::Lut3d>>,
    curve_cache: HashMap<usize, CachedTex<Vec<[f32; 4]>>>,
    mask_cache: HashMap<usize, CachedTex<Vec<f32>>>,
    cache_tick: u64,
    uniform_buffers: Vec<wgpu::Buffer>,
    uniform_buffer_idx: usize,
    /// Open [`begin_frame`](Self::begin_frame)s.
    frames: u32,
    /// Working textures created so far (a steady stream of frames creates none).
    created: u64,
}

/// One compute pass of a job.
struct Dispatch {
    run: bool,
    bg: wgpu::BindGroup,
    groups: (u32, u32),
}

/// The recorded work of one layer's effects (bind groups made; recorded into the frame's encoder
/// right before the layer is drawn, so layers can share pooled textures).
pub(crate) struct FxJob {
    source_bg: wgpu::BindGroup,
    source_target: wgpu::TextureView,
    dispatches: Vec<Dispatch>,
    /// The texture holding the result.
    pub(crate) result: wgpu::TextureView,
    pub(crate) result_texture: wgpu::Texture,
}

/// One pass before texture routing: entry point, op code, integer and float parameters.
struct Step {
    run: bool,
    code: u32,
    i1: [u32; 4],
    p: [f32; 28],
    /// Combine with the preserved original in `orig` (Unsharp Mask or HSL Secondary apply).
    combine: bool,
    /// Keeps `orig` untouched across intermediate passes (Unsharp blur or HSL key/denoise/blur).
    preserve_orig: bool,
    /// Bokeh Blur's gather: its kernel (`filmcraft_render::vfx::bokeh_kernel`), read as the aux
    /// texture.
    kernel: Vec<(i32, i32, f32)>,
    lut: Option<Arc<filmcraft_color::Lut3d>>,
    curves: Option<LumetriCurveTable>,
    warper: Option<ColorWarperTable>,
    samples_3d: Option<Arc<Vec<[f32; 4]>>>,
    /// Save `dst` into `saved_key` after this pass.
    save_key: bool,
    /// Bind `saved_key` to `mask_tex` on this pass and release it afterwards.
    use_key: bool,
    /// First pass of a masked op: save `cur` into `mask_stack`.
    mask_save: bool,
    /// Closing pass of a masked op: mix `cur` with saved original from `mask_stack` using `mask`.
    mask_mix: bool,
    mask: Option<MaskCoverage>,
    /// DAG register routing (`ColorDagOp`).
    dag_save_zero: bool,
    dag_load_cur: Option<u8>,
    dag_aux: Option<u8>,
    dag_mask: Option<u8>,
    dag_save: Option<u8>,
    dag_final_cur: Option<u8>,
}

fn step(code: u32, i1: [u32; 4], p: &[f32]) -> Step {
    let mut q = [0.0; 28];
    for (d, s) in q.iter_mut().zip(p) {
        *d = *s;
    }
    Step {
        run: false,
        code,
        i1,
        p: q,
        combine: false,
        preserve_orig: false,
        kernel: Vec::new(),
        lut: None,
        curves: None,
        warper: None,
        samples_3d: None,
        save_key: false,
        use_key: false,
        mask_save: false,
        mask_mix: false,
        mask: None,
        dag_save_zero: false,
        dag_load_cur: None,
        dag_aux: None,
        dag_mask: None,
        dag_save: None,
        dag_final_cur: None,
    }
}

/// Texels a row of a kernel texture holds.
const KERNEL_ROW: usize = 64;

/// Bokeh Blur's kernel as a texture: entry `k` at (`k` % [`KERNEL_ROW`], `k` / [`KERNEL_ROW`]),
/// (dx, dy, weight, 0). At most 2500 entries: 40 rows.
fn kernel_texture(device: &wgpu::Device, queue: &wgpu::Queue, kernel: &[(i32, i32, f32)]) -> wgpu::TextureView {
    let rows = kernel.len().div_ceil(KERNEL_ROW).max(1);
    let mut texels = vec![0f32; rows * KERNEL_ROW * 4];
    for (t, &(dx, dy, wt)) in texels.as_chunks_mut::<4>().0.iter_mut().zip(kernel) {
        *t = [dx as f32, dy as f32, wt, 0.0];
    }
    let size = wgpu::Extent3d { width: KERNEL_ROW as u32, height: rows as u32, depth_or_array_layers: 1 };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("filmcraft-fx-kernel"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes: Vec<u8> = texels.iter().flat_map(|v| v.to_le_bytes()).collect();
    queue.write_texture(
        t.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some((KERNEL_ROW * 16) as u32), rows_per_image: Some(rows as u32) },
        size,
    );
    t.create_view(&Default::default())
}

/// Box passes along x (vertical = false) or y for the radii (radius 0 is a no-op on the CPU).
fn boxes(radii: &[u32], vertical: bool, repeat: bool, out: &mut Vec<Step>) {
    for &r in radii.iter().filter(|r| **r > 0) {
        let mut s = step(OP_BOX, [r, repeat as u32, vertical as u32, 0], &[]);
        s.run = r > BOX_PER_PIXEL_MAX;
        out.push(s);
    }
}

/// The passes of an op on a `w`×`h` working image (empty: no-op). Unsharp's blur passes come
/// first, then the combine.
fn steps(op: &FxOp, w: u32, h: u32) -> Vec<Step> {
    let mut out = Vec::new();
    match op {
        FxOp::BrightnessContrast { br, co } => out.push(step(OP_BRIGHTNESS_CONTRAST, [0; 4], &[*br, *co])),
        FxOp::ProcAmp { br, co, hue, sat } => out.push(step(OP_PROC_AMP, [0; 4], &[*br, *co, *hue, *sat])),
        FxOp::Tint { black, white, amount } => {
            out.push(step(OP_TINT, [0; 4], &[black[0], black[1], black[2], *amount, white[0], white[1], white[2]]));
        }
        FxOp::BlackWhite => out.push(step(OP_BLACK_WHITE, [0; 4], &[])),
        FxOp::ColorBalance { sh, md, hi, preserve } => {
            out.push(step(OP_COLOR_BALANCE, [*preserve as u32, 0, 0, 0], &[sh[0], sh[1], sh[2], 0.0, md[0], md[1], md[2], 0.0, hi[0], hi[1], hi[2]]))
        }
        FxOp::ThreeWay { sh, md, hi, range, sat } => out.push(step(
            OP_THREE_WAY,
            [op.is_identity_three_way() as u32, 0, 0, 0],
            &[sh[0], sh[1], sh[2], sh[3], md[0], md[1], md[2], md[3], hi[0], hi[1], hi[2], hi[3], range[0], range[1], range[2], range[3], *sat],
        )),
        FxOp::LeaveColor { amount, key_hue, tol, soft } => out.push(step(OP_LEAVE_COLOR, [0; 4], &[*amount, *key_hue, *tol, *soft])),
        FxOp::ChangeToColor { from_hue, to_hue, tol, soft } => out.push(step(OP_CHANGE_TO_COLOR, [0; 4], &[*from_hue, *to_hue, *tol, *soft])),
        FxOp::ColorPass { key, sim, reverse } => out.push(step(OP_COLOR_PASS, [*reverse as u32, 0, 0, 0], &[key[0], key[1], key[2], *sim])),
        FxOp::Gamma { g } => out.push(step(OP_GAMMA, [0; 4], &[*g])),
        FxOp::Levels { ib, iw, ob, ow, g } => out.push(step(OP_LEVELS, [0; 4], &[*ib, *iw, *ob, *ow, *g])),
        FxOp::Extract { lo, hi, soft, invert } => out.push(step(OP_EXTRACT, [*invert as u32, 0, 0, 0], &[*lo, *hi, *soft])),
        FxOp::Invert { channel: 4, blend } => out.push(step(OP_INVERT_ALPHA, [0; 4], &[*blend])),
        FxOp::Invert { channel, blend } => out.push(step(OP_INVERT, [*channel, 0, 0, 0], &[*blend])),
        FxOp::Posterize { n } => out.push(step(OP_POSTERIZE, [0; 4], &[*n])),
        FxOp::Gaussian { rx, ry, repeat } => {
            boxes(rx, false, *repeat, &mut out);
            boxes(ry, true, *repeat, &mut out);
        }
        FxOp::DirectionalBlur { dx, dy, steps } => {
            if *steps >= 2 {
                out.push(step(OP_DIRECTIONAL, [*steps, 0, 0, 0], &[*dx, *dy]));
            }
        }
        FxOp::Bokeh { levels, boost, threshold, sides, rot, rr, rings } => {
            if *rings > 0 {
                // `vfx::bokeh` on the top-left part of the working textures: each halving writes
                // the half-size image there, the boost and the gather work in place, and the
                // last pass scales it back up to the whole image.
                let (mut ww, mut wh) = (w, h);
                for _ in 0..*levels {
                    out.push(step(OP_BOKEH_DOWN, [ww, wh, 0, 0], &[]));
                    (ww, wh) = ((ww / 2).max(1), (wh / 2).max(1));
                }
                if *boost > 0.0 {
                    out.push(step(OP_BOKEH_BOOST, [ww, wh, 0, 0], &[*boost, *threshold]));
                }
                let kernel = filmcraft_render::vfx::bokeh_kernel(*sides, *rot, *rr, *rings);
                let mut gather = step(OP_BOKEH, [ww, wh, kernel.len() as u32, 0], &[]);
                gather.kernel = kernel;
                out.push(gather);
                if *levels > 0 {
                    // `Image::transformed` by scale(w / ww, h / wh): its inverse, in f64 as there
                    let m = filmcraft_geom::Affine::scale(w as f64 / ww as f64, h as f64 / wh as f64);
                    if let Some(inv) = m.inverse() {
                        out.push(step(OP_BOKEH_UP, [ww, wh, 0, 0], &[inv.a as f32, inv.d as f32, inv.e as f32, inv.f as f32]));
                    }
                }
            }
        }
        FxOp::Unsharp { rx, ry, amount, threshold } => {
            boxes(rx, false, true, &mut out);
            boxes(ry, true, true, &mut out);
            for s in &mut out {
                s.preserve_orig = true;
            }
            let mut s = step(OP_UNSHARP, [0; 4], &[*amount, *threshold]);
            s.combine = true;
            out.push(s);
        }
        FxOp::Crop { x0, x1, y0, y1, feather } => out.push(step(OP_CROP, [0; 4], &[*x0, *x1, *y0, *y1, *feather])),
        FxOp::Resample(r) => {
            let op = if (r.opacity - 1.0).abs() < 1e-6 { 1.0 } else { r.opacity };
            match &r.inv {
                Some(m) => out.push(step(OP_RESAMPLE, r.rect, &[m.a as f32, m.b as f32, m.c as f32, m.d as f32, m.e as f32, m.f as f32, op, 1.0])),
                None => out.push(step(OP_RESAMPLE, [0; 4], &[0.0; 8])),
            }
        }
        FxOp::HFlip => out.push(step(OP_HFLIP, [0; 4], &[])),
        FxOp::VFlip => out.push(step(OP_VFLIP, [0; 4], &[])),
        FxOp::Mirror { cx, cy, nx, ny } => out.push(step(OP_MIRROR, [0; 4], &[*cx, *cy, *nx, *ny])),
        FxOp::Offset { dx, dy, blend } => out.push(step(OP_OFFSET, [0; 4], &[*dx, *dy, *blend])),
        FxOp::AscCdl { slope: s, offset: o, power: p, sat } => {
            out.push(step(OP_ASC_CDL, [0; 4], &[s[0], s[1], s[2], *sat, o[0], o[1], o[2], 0.0, p[0], p[1], p[2]]));
        }
        FxOp::ChannelMix { m } => {
            let p: Vec<f32> = m.iter().flatten().copied().collect();
            out.push(step(OP_CHANNEL_MIX, [0; 4], &p));
        }
        FxOp::ColorReplace { sim, solid, target: t, replace: r, replace_hsl: h } => {
            out.push(step(OP_COLOR_REPLACE, [*solid as u32, 0, 0, 0], &[t[0], t[1], t[2], *sim, r[0], r[1], r[2], 0.0, h[0], h[1], h[2]]))
        }
        FxOp::AlphaAdjust { opacity, ignore, invert, mask_only } => {
            out.push(step(OP_ALPHA_ADJUST, [*ignore as u32, *invert as u32, *mask_only as u32, 0], &[*opacity]));
        }
        FxOp::Vignette { amount, midpoint, roundness, feather, target } => {
            if amount.abs() >= 1e-5 {
                out.push(step(OP_VIGNETTE, [0; 4], &[*amount, *midpoint, *roundness, *feather, target[0], target[1], target[2]]));
            }
        }
        FxOp::VideoLimiter { max, comp, axis, warn, warning_color } => {
            out.push(step(OP_VIDEO_LIMITER, [*axis, *warn as u32, 0, 0], &[*max, *comp, warning_color[0], warning_color[1], warning_color[2]]));
        }
        FxOp::Lumetri(op) => push_lumetri_steps(op, false, &mut out),
        FxOp::ColorDag(dag) => push_color_dag_steps(dag, &mut out),
        FxOp::Masked { inner, mask } => {
            if mask.w == w && mask.h == h && mask.cov.len() == (w as usize).saturating_mul(h as usize) {
                let mut inner_steps = steps(inner, w, h);
                if let Some(first) = inner_steps.first_mut() {
                    first.mask_save = true;
                    out.extend(inner_steps);
                    let mut mix_step = step(OP_MASK_MIX, [0; 4], &[]);
                    mix_step.mask_mix = true;
                    mix_step.mask = Some(mask.clone());
                    out.push(mix_step);
                }
            }
        }
    }
    out
}

fn push_lumetri_steps(op: &filmcraft_render::gpufx::LumetriOp, dag_unmasked: bool, out: &mut Vec<Step>) {
    let filmcraft_render::gpufx::LumetriOp {
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
    } = op;

    let is_node_key = hsl.as_ref().is_some_and(|h| h.key_scope == 0);
    let is_legacy_sec = hsl.as_ref().is_some_and(|h| h.key_scope == 1);
    let node_highlight = if !dag_unmasked && !is_legacy_sec && (is_node_key || node_mask.is_some()) { *show_mask } else { 0 };
    let need_outer_blend = !dag_unmasked && (is_node_key || node_mask.is_some() || *key_gain < 0.9999 || node_highlight > 0);
    let start_idx = out.len();

    if !dag_unmasked
        && is_node_key
        && let Some(hsl) = hsl
    {
        push_qualifier_refine_steps(hsl, out);
        let mut finish =
            step(OP_NODE_KEY_FINISH, [u32::from(node_mask.is_some()), 1, 0, 0], &[hsl.clean_black, hsl.clean_white, f32::from(hsl.invert_key), *key_gain]);
        finish.preserve_orig = true;
        finish.save_key = true;
        finish.mask = node_mask.clone();
        out.push(finish);
        let mut restore = step(OP_RESTORE_ORIG, [0; 4], &[]);
        restore.combine = true;
        out.push(restore);
    }

    if let Some(lut) = input_lut {
        let mut s = step(
            OP_LUT_3D,
            [lut.size as u32, 0, 0, 0],
            &[lut.domain_min[0], lut.domain_min[1], lut.domain_min[2], 1.0, lut.domain_max[0], lut.domain_max[1], lut.domain_max[2], 0.0],
        );
        s.lut = Some(lut.clone());
        out.push(s);
    }

    let ge = [gains[0] * exposure, gains[1] * exposure, gains[2] * exposure];
    let b0 = -bl * 0.15;
    let w0 = 1.0 - wh * 0.15;
    let sh_k = sh * 0.35;
    let hl_k = hl * 0.35;
    let vround_aspect = filmcraft_render::gpufx::lumetri_vignette_aspect(*aspect, *vround);
    let st_k = [(st[0] - 0.5) * 0.3, (st[1] - 0.5) * 0.3, (st[2] - 0.5) * 0.3];
    let ht_k = [(ht[0] - 0.5) * 0.3, (ht[1] - 0.5) * 0.3, (ht[2] - 0.5) * 0.3];

    let (hdr_mode, hdr_a, hdr_b) = match grade_space {
        filmcraft_color::GradeSpace::Sdr => (0u32, 0.0f32, 0.0f32),
        filmcraft_color::GradeSpace::Pq { norm, .. } => (1u32, *norm, 0.0f32),
        filmcraft_color::GradeSpace::Hlg { white, gamma } => (2u32, *white, *gamma),
    };
    let p = [
        ge[0],
        ge[1],
        ge[2],
        b0,
        w0,
        sh_k,
        hl_k,
        *contrast,
        *faded,
        *sat,
        *vib,
        *va,
        *vmid,
        *vfeather,
        vround_aspect,
        *pivot,
        st_k[0],
        st_k[1],
        st_k[2],
        *color_boost,
        ht_k[0],
        ht_k[1],
        ht_k[2],
        *primary_hue,
        hdr_a,
        hdr_b,
        *hdr_specular,
        0.0,
    ];
    out.push(step(OP_LUMETRI, [*creative_on as u32, *vignette_on as u32, hdr_mode, 0], &p));

    if let Some(lut) = look_lut {
        if look_k.abs() > 1e-5 {
            let mut s = step(
                OP_LUT_3D,
                [lut.size as u32, 0, 0, 0],
                &[lut.domain_min[0], lut.domain_min[1], lut.domain_min[2], *look_k, lut.domain_max[0], lut.domain_max[1], lut.domain_max[2], 0.0],
            );
            s.lut = Some(lut.clone());
            out.push(s);
        }
    } else if *look > 0 && *look <= 8 && look_k.abs() > 1e-5 {
        out.push(step(OP_LUMETRI_LOOK, [*look, 0, 0, 0], &[*look_k]));
    }

    let lift_k = [ws[0] * 0.3 + ls * 0.3, ws[1] * 0.3 + ls * 0.3, ws[2] * 0.3 + ls * 0.3];
    let gamma_k = [wm[0] * 0.3 + lm * 0.3, wm[1] * 0.3 + lm * 0.3, wm[2] * 0.3 + lm * 0.3];
    let gain_k = [wh_rgb[0] * 0.5 + lh * 0.5, wh_rgb[1] * 0.5 + lh * 0.5, wh_rgb[2] * 0.5 + lh * 0.5];
    let offset_k = [wo[0] * 0.3 + lo * 0.3, wo[1] * 0.3 + lo * 0.3, wo[2] * 0.3 + lo * 0.3];
    let wheels_active = *wheels_on && lift_k.iter().chain(&gamma_k).chain(&gain_k).chain(&offset_k).any(|v| v.abs() > 1e-5);
    if wheels_active {
        let wheels_p = [
            lift_k[0],
            lift_k[1],
            lift_k[2],
            0.0,
            gamma_k[0],
            gamma_k[1],
            gamma_k[2],
            0.0,
            gain_k[0],
            gain_k[1],
            gain_k[2],
            0.0,
            offset_k[0],
            offset_k[1],
            offset_k[2],
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        out.push(step(OP_COLOR_WHEELS, [*grade_model, 0, 0, 0], &wheels_p));
    }

    if let Some(lw) = log_wheels.as_deref() {
        let log_p = [
            lw.shadows[0],
            lw.shadows[1],
            lw.shadows[2],
            lw.low_range,
            lw.midtones[0],
            lw.midtones[1],
            lw.midtones[2],
            lw.high_range,
            lw.highlights[0],
            lw.highlights[1],
            lw.highlights[2],
            0.0,
            lw.offset[0],
            lw.offset[1],
            lw.offset[2],
            0.0,
        ];
        out.push(step(OP_LOG_WHEELS, [0; 4], &log_p));
    }

    if let Some(hw) = hdr_wheels.as_deref() {
        let hdr_p = [
            hw.zones[0][0],
            hw.zones[0][1],
            hw.zones[0][2],
            hw.zones[1][0],
            hw.zones[1][1],
            hw.zones[1][2],
            hw.zones[2][0],
            hw.zones[2][1],
            hw.zones[2][2],
            hw.zones[3][0],
            hw.zones[3][1],
            hw.zones[3][2],
            hw.zones[4][0],
            hw.zones[4][1],
            hw.zones[4][2],
            hw.zones[5][0],
            hw.zones[5][1],
            hw.zones[5][2],
            hw.global[0],
            hw.global[1],
            hw.global[2],
            hw.sigma,
            hw.wheel_range,
            0.0,
        ];
        out.push(step(OP_HDR_WHEELS, [0; 4], &hdr_p));
    }

    if let Some(rm) = rgb_mixer.as_deref() {
        let mix_p =
            [rm.mat[0][0], rm.mat[0][1], rm.mat[0][2], 0.0, rm.mat[1][0], rm.mat[1][1], rm.mat[1][2], 0.0, rm.mat[2][0], rm.mat[2][1], rm.mat[2][2], 0.0];
        out.push(step(OP_RGB_MIXER, [0; 4], &mix_p));
    }

    let active_mask = curves.as_ref().map_or(0, |ct| ct.active_mask);
    if active_mask != 0 || *low_soft > 1e-5 || *high_soft > 1e-5 {
        let mut s = step(OP_LUMETRI_CURVES, [active_mask, 0, 0, 0], &[*low_soft, *high_soft]);
        s.curves = curves.clone();
        out.push(s);
    }

    if let Some(cs) = color_slice {
        let mut p = [0.0f32; 24];
        for (i, band) in cs.0.iter().enumerate() {
            p[i * 3] = band[0];
            p[i * 3 + 1] = band[1];
            p[i * 3 + 2] = band[2];
        }
        out.push(step(OP_COLOR_SLICE, [0; 4], &p));
    }

    if let Some(cw) = warper
        && cw.active_mask != 0
    {
        let mut s = step(OP_COLOR_WARPER, [cw.active_mask, 0, 0, 0], &[]);
        s.warper = Some(cw.clone());
        out.push(s);
    }

    if let Some(c) = cst.as_deref() {
        let p = [
            c.mat[0][0],
            c.mat[0][1],
            c.mat[0][2],
            c.out_luma[0],
            c.mat[1][0],
            c.mat[1][1],
            c.mat[1][2],
            c.out_luma[1],
            c.mat[2][0],
            c.mat[2][1],
            c.mat[2][2],
            c.out_luma[2],
            c.src_peak_nits,
            c.dst_peak_nits,
            0.0,
            0.0,
        ];
        out.push(step(OP_CST, [c.in_gamma, c.out_gamma, c.tone_map, c.gamut_map], &p));
    }

    if let Some(sz) = sizing.as_deref() {
        let p = [sz.pan, sz.tilt, sz.zoom_inv, sz.cos_r, sz.width_inv, sz.height_inv, sz.sin_r, sz.blanking_aspect];
        out.push(step(OP_NODE_SIZING, [u32::from(sz.flip_h), u32::from(sz.flip_v), 0, 0], &p));
    }

    if is_node_key
        && let Some(hsl) = hsl
        && (hsl.htemp.abs() > 1e-4 || hsl.htint.abs() > 1e-4 || (hsl.hsat - 1.0).abs() > 1e-4 || hsl.hshift.abs() > 1e-4)
    {
        out.push(step(OP_HSL_APPLY, [0, 1, 0, 0], &[hsl.htemp, hsl.htint, hsl.hsat, hsl.hshift]));
    } else if !is_node_key && let Some(hsl) = hsl {
        push_qualifier_refine_steps(hsl, out);
        if hsl.clean_black > 1e-4 || hsl.clean_white > 1e-4 || hsl.invert_key {
            let mut finish = step(OP_NODE_KEY_FINISH, [0, 1, 0, 0], &[hsl.clean_black, hsl.clean_white, f32::from(hsl.invert_key), 1.0]);
            finish.preserve_orig = true;
            out.push(finish);
        }
        let mut apply_step = step(OP_HSL_APPLY, [hsl.show_mask, 0, 0, 0], &[hsl.htemp, hsl.htint, hsl.hsat, hsl.hshift]);
        apply_step.combine = true;
        out.push(apply_step);
    }

    if midtone_detail.abs() > 1e-3 && !midtone_rx.is_empty() {
        let start = out.len();
        boxes(midtone_rx, false, true, out);
        boxes(midtone_ry, true, true, out);
        for s in &mut out[start..] {
            s.preserve_orig = true;
        }
        let mut s = step(OP_MIDTONE_DETAIL, [0; 4], &[*midtone_detail]);
        s.combine = true;
        out.push(s);
    }

    if sharpen.abs() > 1e-3 && !sharpen_rx.is_empty() {
        let start = out.len();
        boxes(sharpen_rx, false, true, out);
        boxes(sharpen_ry, true, true, out);
        for s in &mut out[start..] {
            s.preserve_orig = true;
        }
        let mut s = step(OP_UNSHARP, [0; 4], &[sharpen.max(-1.0), 0.0]);
        s.combine = true;
        out.push(s);
    }

    if need_outer_blend {
        if let Some(first) = out.get_mut(start_idx) {
            first.mask_save = true;
        }
        let mut mix_step =
            step(OP_NODE_KEY_MIX, [node_highlight, u32::from(is_node_key || node_mask.is_some()), 0, 0], &[if is_node_key { 1.0 } else { *key_gain }]);
        mix_step.mask_mix = true;
        mix_step.use_key = is_node_key;
        if !is_node_key {
            mix_step.mask = node_mask.clone();
        }
        out.push(mix_step);
    }
}

#[allow(clippy::cognitive_complexity)]
fn push_color_dag_steps(dag: &filmcraft_render::color_graph::ColorDagOp, out: &mut Vec<Step>) {
    use filmcraft_project::{LayerBlendMode, SplitSpace};
    use filmcraft_render::color_graph::{DagInstr, MAX_DAG_SLOTS};

    let dag_start = out.len();
    // Temporary key register when a Corrector needs a key mix but does not export an `out_key` slot.
    let scratch_key_slot = (dag.num_slots as usize).min(MAX_DAG_SLOTS - 1) as u8;

    for instr in &dag.instrs {
        match instr {
            DagInstr::Corrector { in_rgb, in_key, out_rgb, out_key, op, key_input_gain, key_output_gain, key_invert } => {
                let is_node_key = op.hsl.as_ref().is_some_and(|h| h.key_scope == 0);
                let is_legacy_sec = op.hsl.as_ref().is_some_and(|h| h.key_scope == 1);
                let has_local_mask = is_node_key || op.node_mask.is_some();
                let need_key = out_key.is_some()
                    || has_local_mask
                    || in_key.is_some()
                    || *key_invert
                    || (*key_output_gain * op.key_gain) < 0.9999
                    || (!is_legacy_sec && op.show_mask > 0);
                let key_slot = out_key.unwrap_or(scratch_key_slot);

                if need_key {
                    let k_start = out.len();
                    let (clean_b, clean_w, hsl_inv) = if is_node_key && let Some(hsl) = op.hsl.as_deref() {
                        push_qualifier_refine_steps(hsl, out);
                        (hsl.clean_black, hsl.clean_white, hsl.invert_key)
                    } else {
                        (0.0, 0.0, false)
                    };
                    for s in &mut out[k_start..] {
                        s.preserve_orig = false;
                    }
                    let inv_edge = in_key.is_some_and(|(_, inv)| inv);
                    let in_gain = key_input_gain.clamp(0.0, 2.0);
                    let out_gain = (*key_output_gain * op.key_gain).clamp(0.0, 1.0);
                    let mut k_comb = step(
                        OP_DAG_KEY_COMBINE,
                        [u32::from(is_node_key), u32::from(op.node_mask.is_some()), u32::from(in_key.is_some()), u32::from(inv_edge)],
                        &[clean_b, clean_w, f32::from(hsl_inv), in_gain, out_gain, f32::from(*key_invert)],
                    );
                    k_comb.mask = op.node_mask.clone();
                    k_comb.dag_aux = in_key.map(|(ks, _)| ks);
                    k_comb.dag_save = Some(key_slot);
                    out.push(k_comb);
                    if let Some(first_k) = out.get_mut(k_start) {
                        first_k.dag_load_cur = Some(*in_rgb);
                    }
                }

                let rgb_start = out.len();
                push_lumetri_steps(op, true, out);
                if let Some(first_rgb) = out.get_mut(rgb_start) {
                    first_rgb.dag_load_cur = Some(*in_rgb);
                }

                if need_key {
                    let show_mask = if !is_legacy_sec { op.show_mask } else { 0 };
                    let mut mix_s = step(OP_NODE_KEY_MIX, [show_mask, 1, 0, 0], &[1.0]);
                    mix_s.dag_aux = Some(*in_rgb);
                    mix_s.dag_mask = Some(key_slot);
                    mix_s.dag_save = Some(*out_rgb);
                    out.push(mix_s);
                } else if let Some(last_rgb) = out.last_mut() {
                    last_rgb.dag_save = Some(*out_rgb);
                }
            }
            DagInstr::ParallelCombine { base_rgb, branches, out_rgb } => {
                let Some(&first) = branches.first() else { continue };
                let p_start = out.len();
                for (idx, &b_slot) in branches.iter().enumerate().skip(1) {
                    let w_branch = 1.0 / (idx + 1) as f32;
                    let mut s = step(OP_DAG_PARALLEL, [u32::from(base_rgb.is_some()), 0, 0, 0], &[w_branch]);
                    s.dag_aux = Some(b_slot);
                    s.dag_mask = *base_rgb;
                    out.push(s);
                }
                if let Some(first_s) = out.get_mut(p_start) {
                    first_s.dag_load_cur = Some(first);
                }
                if let Some(last_s) = out.last_mut() {
                    last_s.dag_save = Some(*out_rgb);
                }
            }
            DagInstr::LayerComposite { branches, out_rgb } => {
                let Some(&(first_rgb, _, _, _)) = branches.first() else { continue };
                let l_start = out.len();
                for &(top_rgb, top_key, mode, gain) in branches.iter().skip(1) {
                    let mode_code = match mode {
                        LayerBlendMode::Normal => 0,
                        LayerBlendMode::Multiply => 1,
                        LayerBlendMode::Screen => 2,
                        LayerBlendMode::Overlay => 3,
                        LayerBlendMode::SoftLight => 4,
                        LayerBlendMode::HardLight => 5,
                        LayerBlendMode::Add => 6,
                        LayerBlendMode::Subtract => 7,
                        LayerBlendMode::Darken => 8,
                        LayerBlendMode::Lighten => 9,
                        LayerBlendMode::Color => 10,
                        LayerBlendMode::Luminosity => 11,
                    };
                    let inv_k = top_key.is_some_and(|(_, inv)| inv);
                    let mut s = step(OP_DAG_LAYER, [mode_code, u32::from(top_key.is_some()), u32::from(inv_k), 0], &[gain]);
                    s.dag_aux = Some(top_rgb);
                    s.dag_mask = top_key.map(|(ks, _)| ks);
                    out.push(s);
                }
                if let Some(first_s) = out.get_mut(l_start) {
                    first_s.dag_load_cur = Some(first_rgb);
                }
                if let Some(last_s) = out.last_mut() {
                    last_s.dag_save = Some(*out_rgb);
                }
            }
            DagInstr::SplitChannel { in_rgb, space, channel, out_rgb } => {
                let sp = match space {
                    SplitSpace::Rgb => 0,
                    SplitSpace::YCbCr => 1,
                    SplitSpace::Hsl => 2,
                };
                let mut s = step(OP_DAG_SPLIT, [sp, u32::from(*channel), 0, 0], &[]);
                s.dag_load_cur = Some(*in_rgb);
                s.dag_save = Some(*out_rgb);
                out.push(s);
            }
            DagInstr::CombineChannels { in_ch, space, out_rgb } => {
                let sp = match space {
                    SplitSpace::Rgb => 0,
                    SplitSpace::YCbCr => 1,
                    SplitSpace::Hsl => 2,
                };
                let mut s = step(OP_DAG_COMBINE, [sp, 0, 0, 0], &[]);
                s.dag_load_cur = Some(in_ch[0]);
                s.dag_aux = Some(in_ch[1]);
                s.dag_mask = Some(in_ch[2]);
                s.dag_save = Some(*out_rgb);
                out.push(s);
            }
        }
    }
    if let Some(first) = out.get_mut(dag_start) {
        first.dag_save_zero = true;
    }
    if let Some(last) = out.last_mut() {
        last.dag_final_cur = Some(dag.out_rgb);
    }
}

fn push_qualifier_refine_steps(hsl: &filmcraft_render::gpufx::HslSecondaryParams, out: &mut Vec<Step>) {
    let sample_count = hsl.samples_3d.len().min(64) as u32;
    let p = [
        hsl.hc,
        hsl.hr,
        hsl.smin,
        hsl.smax,
        hsl.lmin,
        hsl.lmax,
        hsl.soft,
        hsl.sat_soft,
        hsl.luma_soft,
        hsl.rgb_min[0],
        hsl.rgb_min[1],
        hsl.rgb_min[2],
        hsl.rgb_max[0],
        hsl.rgb_max[1],
        hsl.rgb_max[2],
        hsl.rgb_soft,
    ];
    let mut key_step = step(OP_HSL_KEY, [hsl.mode, sample_count, 0, 0], &p);
    key_step.preserve_orig = true;
    if hsl.mode == 3 && !hsl.samples_3d.is_empty() {
        key_step.samples_3d = Some(hsl.samples_3d.clone());
    }
    out.push(key_step);
    if hsl.denoise > 0.0 && hsl.denoise_r > 0 {
        let mut med_step = step(OP_HSL_MEDIAN, [hsl.denoise_r, 0, 0, 0], &[hsl.denoise]);
        med_step.preserve_orig = true;
        out.push(med_step);
    }
    if hsl.in_out.abs() > 1e-3 && hsl.in_out_r > 0 {
        let mut morph_step = step(OP_HSL_MORPH, [hsl.in_out_r, u32::from(hsl.in_out > 0.0), 0, 0], &[hsl.in_out.abs().min(1.0)]);
        morph_step.preserve_orig = true;
        out.push(morph_step);
    }
    if !hsl.blur_rx.is_empty() || !hsl.blur_ry.is_empty() {
        let blur_start = out.len();
        boxes(&hsl.blur_rx, false, true, out);
        boxes(&hsl.blur_ry, true, true, out);
        for bs in &mut out[blur_start..] {
            bs.preserve_orig = true;
        }
    }
}

impl FxStage {
    /// Whether `device` can run the stage: compute shaders with 16×16 workgroups and a storage
    /// texture (WebGL2-class devices can't; they render effects on the CPU).
    pub(crate) fn supported(device: &wgpu::Device) -> bool {
        let l = device.limits();
        l.max_storage_textures_per_shader_stage >= 1
            && l.max_compute_invocations_per_workgroup >= 256
            && l.max_compute_workgroup_size_x >= 64
            && l.max_compute_workgroup_size_y >= 16
            && l.max_compute_workgroups_per_dimension >= 1
    }

    pub(crate) fn new(device: &wgpu::Device, composite: &wgpu::ShaderModule, layer_bgl: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("filmcraft-fx"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fx.wgsl").into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: FX_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                tex(3),
                tex(4),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("fx"), bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fx"),
                layout: Some(&pl),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let spl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx-source"),
            bind_group_layouts: &[Some(layer_bgl)],
            immediate_size: 0,
        });
        let source = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx-source"),
            layout: Some(&spl),
            vertex: wgpu::VertexState { module: composite, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: composite,
                entry_point: Some("fs_fx_source"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: FX_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            px: compute("fx_px"),
            run: compute("fx_run"),
            bgl,
            source,
            pool: HashMap::new(),
            used: HashSet::new(),
            lut_cache: HashMap::new(),
            curve_cache: HashMap::new(),
            mask_cache: HashMap::new(),
            cache_tick: 0,
            uniform_buffers: Vec::new(),
            uniform_buffer_idx: 0,
            frames: 0,
            created: 0,
        }
    }

    /// Clear pooled textures and cached LUT / curve / mask textures.
    pub(crate) fn trim_uploads(&mut self) {
        self.lut_cache.clear();
        self.curve_cache.clear();
        self.mask_cache.clear();
        self.pool.clear();
    }

    fn next_cache_tick(&mut self) -> u64 {
        self.cache_tick = self.cache_tick.wrapping_add(1);
        self.cache_tick
    }

    fn lut_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lut: &Arc<filmcraft_color::Lut3d>) -> wgpu::TextureView {
        let key = Arc::as_ptr(lut) as usize;
        let tick = self.next_cache_tick();
        if let Some(entry) = self.lut_cache.get_mut(&key) {
            entry.last_used = tick;
            return entry.view.clone();
        }
        evict_lru_tex(&mut self.lut_cache, 32);
        let size = lut.size as u32;
        let width = size * size;
        let height = size;
        let row_bytes = width * 16;
        let aligned_row_bytes = row_bytes.div_ceil(256) * 256;
        let mut texels = vec![0u8; (aligned_row_bytes * height) as usize];
        for b in 0..size as usize {
            let row_offset = b * (aligned_row_bytes as usize);
            for g in 0..size as usize {
                for r in 0..size as usize {
                    let lut_idx = r + g * (size as usize) + b * (size as usize) * (size as usize);
                    let c = lut.data.get(lut_idx).copied().unwrap_or([0.0, 0.0, 0.0]);
                    let x = (r + g * (size as usize)) * 16;
                    let dst_slice = &mut texels[row_offset + x..row_offset + x + 16];
                    dst_slice[0..4].copy_from_slice(&c[0].to_le_bytes());
                    dst_slice[4..8].copy_from_slice(&c[1].to_le_bytes());
                    dst_slice[8..12].copy_from_slice(&c[2].to_le_bytes());
                    dst_slice[12..16].copy_from_slice(&1.0f32.to_le_bytes());
                }
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("filmcraft-fx-lut3d"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &texels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(aligned_row_bytes), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&Default::default());
        self.lut_cache.insert(key, CachedTex { _keep: lut.clone(), _tex: texture, view: view.clone(), last_used: tick });
        view
    }

    fn rgba32f_table_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texels: &Arc<Vec<[f32; 4]>>,
        width: u32,
        height: u32,
        label: &'static str,
    ) -> wgpu::TextureView {
        let key = Arc::as_ptr(texels) as usize;
        let tick = self.next_cache_tick();
        if let Some(entry) = self.curve_cache.get_mut(&key) {
            entry.last_used = tick;
            return entry.view.clone();
        }
        evict_lru_tex(&mut self.curve_cache, 64);
        let w = width as usize;
        let row_bytes = width * 16;
        let aligned_row_bytes = row_bytes.div_ceil(256) * 256;
        let mut bytes = vec![0u8; (aligned_row_bytes * height) as usize];
        for row in 0..height as usize {
            let row_off = row * (aligned_row_bytes as usize);
            let src_start = row * w;
            for x in 0..w {
                if let Some(c) = texels.get(src_start + x) {
                    let off = row_off + x * 16;
                    if let Some(dst) = bytes.get_mut(off..off + 16) {
                        dst[0..4].copy_from_slice(&c[0].to_le_bytes());
                        dst[4..8].copy_from_slice(&c[1].to_le_bytes());
                        dst[8..12].copy_from_slice(&c[2].to_le_bytes());
                        dst[12..16].copy_from_slice(&c[3].to_le_bytes());
                    }
                }
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(aligned_row_bytes), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&Default::default());
        self.curve_cache.insert(key, CachedTex { _keep: texels.clone(), _tex: texture, view: view.clone(), last_used: tick });
        view
    }

    fn curve_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, ct: &LumetriCurveTable) -> wgpu::TextureView {
        self.rgba32f_table_texture(device, queue, &ct.texels, LUMETRI_CURVE_LEN as u32, 3, "filmcraft-fx-curves")
    }

    fn warper_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, cw: &ColorWarperTable) -> wgpu::TextureView {
        self.rgba32f_table_texture(device, queue, &cw.texels, CW_LUT_W as u32, CW_LUT_H as u32, "filmcraft-fx-warper")
    }

    fn samples_3d_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, pts: &Arc<Vec<[f32; 4]>>) -> wgpu::TextureView {
        let width = pts.len().clamp(1, 64) as u32;
        self.rgba32f_table_texture(device, queue, pts, width, 1, "filmcraft-fx-3d-samples")
    }

    fn mask_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mask: &MaskCoverage) -> wgpu::TextureView {
        let key = Arc::as_ptr(&mask.cov) as usize;
        let tick = self.next_cache_tick();
        if let Some(entry) = self.mask_cache.get_mut(&key) {
            entry.last_used = tick;
            return entry.view.clone();
        }
        evict_lru_tex(&mut self.mask_cache, 32);
        let (width, height) = (mask.w.max(1), mask.h.max(1));
        let row_bytes = width * 4;
        let aligned_row_bytes = row_bytes.div_ceil(256) * 256;
        let mut bytes = vec![0u8; (aligned_row_bytes * height) as usize];
        for y in 0..height as usize {
            let row_off = y * (aligned_row_bytes as usize);
            let src_off = y * (width as usize);
            for x in 0..width as usize {
                if let Some(&v) = mask.cov.get(src_off + x) {
                    let off = row_off + x * 4;
                    if let Some(dst) = bytes.get_mut(off..off + 4) {
                        dst.copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("filmcraft-fx-mask"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(aligned_row_bytes), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&Default::default());
        self.mask_cache.insert(key, CachedTex { _keep: mask.cov.clone(), _tex: texture, view: view.clone(), last_used: tick });
        view
    }

    fn uniform_buffer(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> wgpu::Buffer {
        let idx = self.uniform_buffer_idx;
        self.uniform_buffer_idx = idx.saturating_add(1);
        let size = bytes.len() as u64;
        let needs_new = self.uniform_buffers.get(idx).is_none_or(|b| b.size() < size);
        if needs_new {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fx-u"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if let Some(slot) = self.uniform_buffers.get_mut(idx) {
                *slot = buf;
            } else {
                self.uniform_buffers.push(buf);
            }
        }
        let buf = match self.uniform_buffers.get(idx) {
            Some(b) => b.clone(),
            None => device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fx-u"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        };
        queue.write_buffer(&buf, 0, bytes);
        buf
    }

    /// Start a frame: pooled textures of sizes no layer uses by [`end_frame`](Self::end_frame)
    /// are released then. Frames nest (a composite of several steps is one frame around the
    /// layers of each step): the pool is trimmed when the outermost one ends.
    pub(crate) fn begin_frame(&mut self) {
        if self.frames == 0 {
            self.used.clear();
            self.uniform_buffer_idx = 0;
        }
        self.frames = self.frames.saturating_add(1);
    }

    pub(crate) fn end_frame(&mut self) {
        self.frames = self.frames.saturating_sub(1);
        if self.frames == 0 {
            let used = &self.used;
            self.pool.retain(|k, _| used.contains(k));
        }
    }

    #[cfg(test)]
    pub(crate) fn created(&self) -> u64 {
        self.created
    }

    /// No working textures held.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.pool.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn uniform_pool_len(&self) -> usize {
        self.uniform_buffers.len()
    }

    fn targets(&mut self, device: &wgpu::Device, size: (u32, u32), n: usize) -> Vec<Target> {
        self.used.insert(size);
        let v = self.pool.entry(size).or_default();
        while v.len() < n {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("filmcraft-fx"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FX_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = t.create_view(&Default::default());
            v.push((t, view));
            self.created = self.created.saturating_add(1);
        }
        v.iter().take(n).cloned().collect()
    }

    /// Prepare the passes of `fx`, whose source draw uses `source_bg` (a layer bind group whose
    /// uniforms map the frame onto the working image). None when the working image does not fit
    /// in a texture (the layer is then drawn without its effects — never larger than the source
    /// frame, which would not have uploaded either).
    #[allow(clippy::cognitive_complexity)]
    pub(crate) fn job(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fx: &LayerFx,
        source_bg: wgpu::BindGroup,
        dummy: &wgpu::TextureView,
    ) -> Option<FxJob> {
        let (w, h) = (fx.size.0.max(1), fx.size.1.max(1));
        let max = device.limits().max_texture_dimension_2d;
        if w > max || h > max {
            return None;
        }
        let all: Vec<Step> = fx.ops.iter().flat_map(|op| steps(op, w, h)).collect();
        let mut n = 2usize;
        let mut sim_orig = false;
        let mut sim_mask_depth = 0usize;
        let mut sim_key = false;
        let mut max_dag_slot = 0usize;
        for s in &all {
            for slot in [s.dag_load_cur, s.dag_aux, s.dag_mask, s.dag_save, s.dag_final_cur].into_iter().flatten() {
                max_dag_slot = max_dag_slot.max(slot as usize + 1);
            }
            if (s.preserve_orig || s.combine) && !sim_orig {
                sim_orig = true;
            }
            if s.mask_save {
                sim_mask_depth = sim_mask_depth.saturating_add(1);
            }
            if s.save_key {
                sim_key = true;
            }
            n = n.max(2 + usize::from(sim_orig) + sim_mask_depth + usize::from(sim_key) + max_dag_slot);
            if s.combine {
                sim_orig = false;
            }
            if s.mask_mix {
                sim_mask_depth = sim_mask_depth.saturating_sub(1);
            }
            if s.use_key {
                sim_key = false;
            }
        }
        let targets = self.targets(device, (w, h), n);
        let views: Vec<wgpu::TextureView> = targets.iter().map(|t| t.1.clone()).collect();
        let mut dispatches = Vec::with_capacity(all.len());
        let mut cur = 0usize;
        // Preserved original for two-stage effects (Unsharp Mask, HSL Secondary).
        let mut orig: Option<usize> = None;
        // Saved key matte produced by `OP_NODE_KEY_FINISH` before Lumetri RGB passes.
        let mut saved_key: Option<usize> = None;
        // Saved originals before masked effects (Power Windows / effect masks).
        let mut mask_stack: Vec<usize> = Vec::new();
        // Virtual register file for non-linear ColorDag execution.
        let mut dag_regs: [Option<usize>; filmcraft_render::color_graph::MAX_DAG_SLOTS] = [None; filmcraft_render::color_graph::MAX_DAG_SLOTS];
        for s in &all {
            if s.dag_save_zero {
                dag_regs = [None; filmcraft_render::color_graph::MAX_DAG_SLOTS];
                dag_regs[0] = Some(cur);
            }
            if let Some(slot) = s.dag_load_cur
                && let Some(reg_tex) = dag_regs.get(slot as usize).copied().flatten()
            {
                cur = reg_tex;
            }
            if s.mask_save {
                mask_stack.push(cur);
            }
            if (s.preserve_orig || s.combine) && orig.is_none() {
                orig = Some(cur);
            }
            let keep_orig = if s.preserve_orig || s.combine { orig } else { None };
            let dst =
                (0..n).find(|k| *k != cur && Some(*k) != keep_orig && Some(*k) != saved_key && !mask_stack.contains(k) && !dag_regs.contains(&Some(*k)))?;
            let kernel = (!s.kernel.is_empty()).then(|| kernel_texture(device, queue, &s.kernel));
            let lut_tex = s.lut.as_ref().map(|l| self.lut_texture(device, queue, l));
            let curve_tex = s
                .curves
                .as_ref()
                .map(|c| self.curve_texture(device, queue, c))
                .or_else(|| s.warper.as_ref().map(|cw| self.warper_texture(device, queue, cw)))
                .or_else(|| s.samples_3d.as_ref().map(|pts| self.samples_3d_texture(device, queue, pts)));
            let mask_tex_view = if let Some(ms) = s.dag_mask {
                dag_regs.get(ms as usize).copied().flatten().and_then(|ti| views.get(ti).cloned())
            } else if s.use_key {
                saved_key.take().and_then(|ki| views.get(ki).cloned())
            } else {
                s.mask.as_ref().map(|m| self.mask_texture(device, queue, m))
            };
            let mask_orig = if s.mask_mix { mask_stack.pop() } else { None };
            let dag_aux_tex = s.dag_aux.and_then(|aslot| dag_regs.get(aslot as usize).copied().flatten());
            let aux = if let Some(at) = dag_aux_tex {
                views.get(at)?
            } else {
                match (s.mask_mix, mask_orig, s.combine, orig, &kernel, &lut_tex, &curve_tex) {
                    (true, Some(mo), _, _, _, _, _) => views.get(mo)?,
                    (_, _, true, Some(o), _, _, _) => views.get(o)?,
                    (_, _, _, _, Some(k), _, _) => k,
                    (_, _, _, _, _, Some(lt), _) => lt,
                    (_, _, _, _, _, _, Some(ct)) => ct,
                    _ => dummy,
                }
            };
            let mask_view = mask_tex_view.as_ref().unwrap_or(dummy);
            let mut bytes = Vec::with_capacity(144);
            for v in [s.code, w, h, 0].iter().chain(&s.i1) {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            for v in &s.p {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            let buf = self.uniform_buffer(device, queue, &bytes);
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(views.get(cur)?) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(views.get(dst)?) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(aux) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(mask_view) },
                ],
            });
            let groups = if s.run {
                let lines = if s.i1[2] != 0 { w } else { h };
                (lines.div_ceil(64), 1)
            } else {
                (w.div_ceil(16), h.div_ceil(16))
            };
            dispatches.push(Dispatch { run: s.run, bg, groups });
            cur = dst;
            if s.save_key {
                saved_key = Some(cur);
            }
            if s.combine {
                orig = None;
            }
            if let Some(slot) = s.dag_save
                && let Some(reg) = dag_regs.get_mut(slot as usize)
            {
                *reg = Some(cur);
            }
            if let Some(slot) = s.dag_final_cur
                && let Some(reg_tex) = dag_regs.get(slot as usize).copied().flatten()
            {
                cur = reg_tex;
            }
        }
        Some(FxJob {
            source_bg,
            source_target: views.first()?.clone(),
            dispatches,
            result: views.get(cur)?.clone(),
            result_texture: targets.get(cur)?.0.clone(),
        })
    }

    /// Record a job (source draw, then its compute passes) into `enc`.
    pub(crate) fn record(&self, enc: &mut wgpu::CommandEncoder, job: &FxJob) {
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fx-source"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &job.source_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.source);
            pass.set_bind_group(0, &job.source_bg, &[]);
            pass.draw(0..6, 0..1);
        }
        if job.dispatches.is_empty() {
            return;
        }
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("fx"), timestamp_writes: None });
        for d in &job.dispatches {
            pass.set_pipeline(if d.run { &self.run } else { &self.px });
            pass.set_bind_group(0, &d.bg, &[]);
            pass.dispatch_workgroups(d.groups.0, d.groups.1, 1);
        }
    }
}
