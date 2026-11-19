// FilmCraft GPU effect stage: the standard effects of `filmcraft_render::gpufx::FxOp`, one
// compute pass each, reading the working image (`src`, linear premultiplied RGBA f32) and writing
// the next one (`dst`). The math mirrors `FxOp::apply` (the CPU reference) operation by
// operation; parameters arrive evaluated (keyframes, defaults and clamps applied on the CPU).

struct U {
    i0: vec4<u32>,  // op, width, height, unused
    i1: vec4<u32>,  // integer parameters (box radius, repeat, steps, channel, rect…)
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
    p4: vec4<f32>,
    p5: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;
// Unsharp Mask: the image before blurring.
@group(0) @binding(3) var aux: texture_2d<f32>;
@group(0) @binding(4) var mask_tex: texture_2d<f32>;

const OP_BRIGHTNESS_CONTRAST: u32 = 1u;
const OP_PROC_AMP: u32 = 2u;
const OP_TINT: u32 = 3u;
const OP_BLACK_WHITE: u32 = 4u;
const OP_COLOR_BALANCE: u32 = 5u;
const OP_LEAVE_COLOR: u32 = 6u;
const OP_CHANGE_TO_COLOR: u32 = 7u;
const OP_COLOR_PASS: u32 = 8u;
const OP_GAMMA: u32 = 9u;
const OP_LEVELS: u32 = 10u;
const OP_EXTRACT: u32 = 11u;
const OP_INVERT: u32 = 12u;
const OP_INVERT_ALPHA: u32 = 13u;
const OP_POSTERIZE: u32 = 14u;
const OP_ASC_CDL: u32 = 15u;
const OP_CHANNEL_MIX: u32 = 16u;
const OP_COLOR_REPLACE: u32 = 17u;
const OP_ALPHA_ADJUST: u32 = 18u;
const OP_VIGNETTE: u32 = 19u;
const OP_BOX: u32 = 20u;
const OP_DIRECTIONAL: u32 = 22u;
const OP_UNSHARP: u32 = 23u;
const OP_CROP: u32 = 24u;
const OP_RESAMPLE: u32 = 25u;
const OP_HFLIP: u32 = 26u;
const OP_VFLIP: u32 = 27u;
const OP_MIRROR: u32 = 28u;
const OP_OFFSET: u32 = 29u;
const OP_VIDEO_LIMITER: u32 = 30u;
const OP_LUMETRI: u32 = 31u;
const OP_BOKEH_DOWN: u32 = 32u;
const OP_BOKEH_BOOST: u32 = 33u;
const OP_BOKEH: u32 = 34u;
const OP_BOKEH_UP: u32 = 35u;
const OP_THREE_WAY: u32 = 36u;
const OP_COLOR_WHEELS: u32 = 37u;
const OP_LUMETRI_LOOK: u32 = 38u;
const OP_LUT_3D: u32 = 39u;
const OP_LUMETRI_CURVES: u32 = 40u;
const OP_HSL_KEY: u32 = 41u;
const OP_HSL_MEDIAN: u32 = 42u;
const OP_HSL_APPLY: u32 = 43u;
const OP_MASK_MIX: u32 = 44u;
const OP_MIDTONE_DETAIL: u32 = 45u;
const OP_COLOR_SLICE: u32 = 46u;
const OP_COLOR_WARPER: u32 = 47u;
const OP_HSL_MORPH: u32 = 48u;
const OP_NODE_KEY_FINISH: u32 = 49u;
const OP_RESTORE_ORIG: u32 = 50u;
const OP_NODE_KEY_MIX: u32 = 51u;
const OP_DAG_KEY_COMBINE: u32 = 52u;
const OP_DAG_PARALLEL: u32 = 53u;
const OP_DAG_LAYER: u32 = 54u;
const OP_DAG_SPLIT: u32 = 55u;
const OP_DAG_COMBINE: u32 = 56u;
const OP_LOG_WHEELS: u32 = 57u;
const OP_HDR_WHEELS: u32 = 58u;
const OP_RGB_MIXER: u32 = 59u;
const OP_CST: u32 = 60u;
const OP_NODE_SIZING: u32 = 61u;

fn size() -> vec2<i32> {
    return vec2<i32>(i32(u.i0.y), i32(u.i0.z));
}

fn ld(p: vec2<i32>) -> vec4<f32> {
    return textureLoad(src, p, 0);
}

// `Image::get_or_clear`
fn ld_or_clear(p: vec2<i32>) -> vec4<f32> {
    let s = size();
    if p.x < 0 || p.y < 0 || p.x >= s.x || p.y >= s.y {
        return vec4(0.0);
    }
    return textureLoad(src, p, 0);
}

// `Image::sample_bilinear`: pixel centres at +0.5, transparent outside.
fn sample_bilinear(x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let i = vec2<i32>(i32(x0), i32(y0));
    let a = ld_or_clear(i);
    let b = ld_or_clear(i + vec2(1, 0));
    let c = ld_or_clear(i + vec2(0, 1));
    let d = ld_or_clear(i + vec2(1, 1));
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// `Image::sample_bilinear_clamped`
fn sample_clamped(x: f32, y: f32) -> vec4<f32> {
    let s = size();
    let fx = clamp(x - 0.5, 0.0, f32(s.x) - 1.0);
    let fy = clamp(y - 0.5, 0.0, f32(s.y) - 1.0);
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let x1 = min(x0 + 1, s.x - 1);
    let y1 = min(y0 + 1, s.y - 1);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let a = ld(vec2(x0, y0));
    let b = ld(vec2(x1, y0));
    let c = ld(vec2(x0, y1));
    let d = ld(vec2(x1, y1));
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// ---- Bokeh Blur works on the top-left `r` pixels of the textures (the halved image)

// `Image::get_clamped` within `r`
fn ld_in(p: vec2<i32>, r: vec2<i32>) -> vec4<f32> {
    return textureLoad(src, clamp(p, vec2(0), r - 1), 0);
}

// `Image::get_or_clear` within `r`
fn ld_or_clear_in(p: vec2<i32>, r: vec2<i32>) -> vec4<f32> {
    if p.x < 0 || p.y < 0 || p.x >= r.x || p.y >= r.y {
        return vec4(0.0);
    }
    return textureLoad(src, p, 0);
}

// `Image::sample_bilinear` within `r`
fn sample_bilinear_in(x: f32, y: f32, r: vec2<i32>) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let i = vec2<i32>(i32(x0), i32(y0));
    let a = ld_or_clear_in(i, r);
    let b = ld_or_clear_in(i + vec2(1, 0), r);
    let c = ld_or_clear_in(i + vec2(0, 1), r);
    let d = ld_or_clear_in(i + vec2(1, 1), r);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// Bokeh Blur's passes (`vfx::bokeh`); i1.xy = the size of the image they read.
fn bokeh(op: u32, p: vec2<i32>) -> vec4<f32> {
    let r = vec2<i32>(i32(u.i1.x), i32(u.i1.y));
    switch op {
        case OP_BOKEH_DOWN: {
            // `Image::downsample2`: the mean of each 2x2 block (edge-clamped)
            let o = max(r / 2, vec2(1));
            if p.x >= o.x || p.y >= o.y {
                return vec4(0.0);
            }
            let q = p * 2;
            return (ld_in(q, r) + ld_in(q + vec2(1, 0), r) + ld_in(q + vec2(0, 1), r) + ld_in(q + vec2(1, 1), r)) * 0.25;
        }
        case OP_BOKEH_BOOST: {
            if p.x >= r.x || p.y >= r.y {
                return vec4(0.0);
            }
            let o = ld(p);
            if o.a <= 1e-6 {
                return o;
            }
            let l = luma709(enc(o.rgb / o.a));
            let k = 1.0 + u.p0.x * smoothstep_fx(u.p0.y, 1.0, l);
            return vec4(o.rgb * k, o.a);
        }
        case OP_BOKEH: {
            // the iris gather through its kernel (`vfx::bokeh_kernel`): i1.z entries in aux,
            // 64 a row, each (dx, dy, weight); the reads edge-clamped within the halved image
            if p.x >= r.x || p.y >= r.y {
                return vec4(0.0);
            }
            var acc = vec4(0.0);
            for (var k = 0u; k < u.i1.z; k++) {
                let c = textureLoad(aux, vec2(i32(k % 64u), i32(k / 64u)), 0);
                acc += ld_in(p + vec2<i32>(c.xy), r) * c.z;
            }
            return acc;
        }
        default: {
            // OP_BOKEH_UP: `Image::transformed` by the scale back to the whole image
            let pc = vec2<f32>(p) + 0.5;
            let uu = u.p0.x * pc.x + u.p0.z;
            let vv = u.p0.y * pc.y + u.p0.w;
            if uu < -1.0 || vv < -1.0 || uu > f32(r.x) + 1.0 || vv > f32(r.y) + 1.0 {
                return vec4(0.0);
            }
            return sample_bilinear_in(uu, vv, r);
        }
    }
}

// ---- colour helpers (`filmcraft_color`, `effects::enc` / `dec`)

fn linear_to_srgb1(v: f32) -> f32 {
    if v <= 0.0031308 {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_to_linear1(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn enc(c: vec3<f32>) -> vec3<f32> {
    let m = max(c, vec3(0.0));
    return vec3(linear_to_srgb1(m.x), linear_to_srgb1(m.y), linear_to_srgb1(m.z));
}

fn dec(c: vec3<f32>) -> vec3<f32> {
    let m = clamp(c, vec3(0.0), vec3(1.0));
    return vec3(srgb_to_linear1(m.x), srgb_to_linear1(m.y), srgb_to_linear1(m.z));
}

fn luma709(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

// `f32::powf` for x >= 0 (WGSL leaves pow(0, y) undefined).
fn powf(x: f32, y: f32) -> f32 {
    if x == 0.0 {
        if y == 0.0 {
            return 1.0;
        }
        if y > 0.0 {
            return 0.0;
        }
        return 3.0e38;
    }
    return pow(x, y);
}

// `f32::round`: halves away from zero (WGSL `round` rounds them to even).
fn round_away(x: f32) -> f32 {
    let a = abs(x);
    let f = floor(a);
    let r = select(f, f + 1.0, a - f >= 0.5);
    return select(r, -r, x < 0.0);
}

// `rem_euclid(1.0)`
fn fract_euclid(x: f32) -> f32 {
    return x - floor(x);
}

// `vfx::smoothstep` (not WGSL's: a degenerate edge pair divides by 1e-6)
fn smoothstep_fx(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / max(e1 - e0, 1e-6), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn rgb_to_hsl(c: vec3<f32>) -> vec3<f32> {
    let r = c.x; let g = c.y; let b = c.z;
    let mx = max(max(r, g), b);
    let mn = min(min(r, g), b);
    let l = (mx + mn) / 2.0;
    if abs(mx - mn) < 1e-7 {
        return vec3(0.0, 0.0, l);
    }
    let d = mx - mn;
    var s: f32;
    if l > 0.5 {
        s = d / (2.0 - mx - mn);
    } else {
        s = d / (mx + mn);
    }
    var h: f32;
    if mx == r {
        h = (g - b) / d + select(0.0, 6.0, g < b);
    } else if mx == g {
        h = (b - r) / d + 2.0;
    } else {
        h = (r - g) / d + 4.0;
    }
    return vec3(h / 6.0, s, l);
}

fn hsl_channel(p: f32, q: f32, t0: f32) -> f32 {
    let t = fract_euclid(t0);
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    return p;
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> vec3<f32> {
    if s <= 0.0 {
        return vec3(l);
    }
    var q: f32;
    if l < 0.5 {
        q = l * (1.0 + s);
    } else {
        q = l + s - l * s;
    }
    let p = 2.0 * l - q;
    return vec3(hsl_channel(p, q, h + 1.0 / 3.0), hsl_channel(p, q, h), hsl_channel(p, q, h - 1.0 / 3.0));
}

fn linear_to_enc_limiter(v: f32) -> f32 {
    if v <= 1.0 {
        return linear_to_srgb1(max(v, 0.0));
    }
    return 1.0 + (v - 1.0) / 2.4;
}

fn enc_to_linear_limiter(v: f32) -> f32 {
    if v <= 1.0 {
        return srgb_to_linear1(max(v, 0.0));
    }
    return 1.0 + (v - 1.0) * 2.4;
}

fn knee_limiter(v: f32, max_val: f32, comp: f32) -> f32 {
    let k = max_val * (1.0 - comp);
    if comp <= 0.0 || v <= k {
        return min(v, max_val);
    }
    let r = max_val - k;
    return k + r * (1.0 - exp(-(v - k) / r));
}

fn to_rgb_709(y: f32, cb: f32, cr: f32) -> vec3<f32> {
    let kr = 0.2126;
    let kb = 0.0722;
    let kg = 1.0 - kr - kb;
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / kg;
    return vec3(r, g, b);
}

fn limit_video(v: vec3<f32>, max_val: f32, comp: f32, axis: u32) -> vec3<f32> {
    let kr = 0.2126;
    let kb = 0.0722;
    let kg = 1.0 - kr - kb;
    var y = kr * v.x + kg * v.y + kb * v.z;
    var cb = (v.z - y) / (2.0 * (1.0 - kb));
    var cr = (v.x - y) / (2.0 * (1.0 - kr));

    if axis != 1u {
        y = knee_limiter(max(y, 0.0), max_val, comp);
    }

    if axis != 0u {
        let rgb = to_rgb_709(y, cb, cr);
        var s = 1.0;
        let d = rgb - vec3(y);

        if rgb.x > max_val && d.x > 1e-6 {
            s = min(s, max((max_val - y) / d.x, 0.0));
        }
        if rgb.x < 0.0 && d.x < -1e-6 {
            s = min(s, max(y / (-d.x), 0.0));
        }

        if rgb.y > max_val && d.y > 1e-6 {
            s = min(s, max((max_val - y) / d.y, 0.0));
        }
        if rgb.y < 0.0 && d.y < -1e-6 {
            s = min(s, max(y / (-d.y), 0.0));
        }

        if rgb.z > max_val && d.z > 1e-6 {
            s = min(s, max((max_val - y) / d.z, 0.0));
        }
        if rgb.z < 0.0 && d.z < -1e-6 {
            s = min(s, max(y / (-d.z), 0.0));
        }

        if axis == 3u && s < 1.0 - 1e-4 {
            s = min(knee_limiter(s, 1.0, max(comp, 0.03)), 1.0);
        }
        cb *= s;
        cr *= s;
    }

    let out_rgb = to_rgb_709(y, cb, cr);
    return clamp(out_rgb, vec3(0.0), vec3(max_val));
}

// ---- per-pixel colour effects on straight colour (`Image::map_rgb`)

fn color_op(op: u32, c: vec3<f32>) -> vec3<f32> {
    switch op {
        case OP_BRIGHTNESS_CONTRAST: {
            let br = u.p0.x; let co = u.p0.y;
            return dec((enc(c) - 0.5) * co + 0.5 + br);
        }
        case OP_PROC_AMP: {
            let br = u.p0.x; let co = u.p0.y; let hue = u.p0.z; let sat = u.p0.w;
            var hsl = rgb_to_hsl(enc(c));
            hsl.x = fract_euclid(hsl.x + hue);
            hsl.y = clamp(hsl.y * sat, 0.0, 1.0);
            return dec((hsl_to_rgb(hsl.x, hsl.y, hsl.z) - 0.5) * co + 0.5 + br);
        }
        case OP_TINT: {
            let bl = u.p0.xyz; let wh = u.p1.xyz; let amt = u.p0.w;
            let l = linear_to_srgb1(max(luma709(c), 0.0));
            let t = bl + (wh - bl) * l;
            let e = enc(c);
            return dec(e + (t - e) * amt);
        }
        case OP_BLACK_WHITE: {
            return vec3(luma709(c));
        }
        case OP_COLOR_BALANCE: {
            let e = enc(c);
            let l = 0.2126 * e.x + 0.7152 * e.y + 0.0722 * e.z;
            let ws = (1.0 - l) * (1.0 - l);
            let wh = l * l;
            let wm = 1.0 - ws - wh;
            var o = e + u.p0.xyz * ws + u.p1.xyz * max(wm, 0.0) + u.p2.xyz * wh;
            if u.i1.x != 0u {
                let l2 = 0.2126 * o.x + 0.7152 * o.y + 0.0722 * o.z;
                o = o + (l - l2);
            }
            return dec(o);
        }
        case OP_THREE_WAY: {
            if u.i1.x != 0u {
                return c;
            }
            let e = enc(c);
            let l = 0.2126 * e.x + 0.7152 * e.y + 0.0722 * e.z;
            let ts = clamp((l - u.p3.x) / max(u.p3.y - u.p3.x, 1e-4), 0.0, 1.0);
            let th = clamp((l - u.p3.z) / max(u.p3.w - u.p3.z, 1e-4), 0.0, 1.0);
            var ws = 1.0 - ts * ts * (3.0 - 2.0 * ts);
            var wh = th * th * (3.0 - 2.0 * th);
            var wm = max(1.0 - ws - wh, 0.0);
            let total = ws + wm + wh;
            ws = ws / total;
            wh = wh / total;
            wm = wm / total;
            let lvl = u.p0.w * ws + u.p1.w * wm + u.p2.w * wh;
            let o = e + u.p0.xyz * ws + u.p1.xyz * wm + u.p2.xyz * wh + vec3(lvl);
            let l2 = 0.2126 * o.x + 0.7152 * o.y + 0.0722 * o.z;
            // no upper clamp: HDR values keep their headroom
            let r = max(vec3(l2) + (o - vec3(l2)) * u.p4.x, vec3(0.0));
            return vec3(srgb_to_linear1(r.x), srgb_to_linear1(r.y), srgb_to_linear1(r.z));
        }
        case OP_LEAVE_COLOR: {
            let amt = u.p0.x; let kh = u.p0.y; let tol = u.p0.z; let soft = u.p0.w;
            let h = rgb_to_hsl(enc(c)).x;
            let d = min(abs(h - kh), 1.0 - abs(h - kh)) * 2.0;
            let keep = 1.0 - clamp((d - tol) / soft, 0.0, 1.0);
            let l = luma709(c);
            let k = amt * (1.0 - keep);
            return c + (l - c) * k;
        }
        case OP_CHANGE_TO_COLOR: {
            let fh = u.p0.x; let th = u.p0.y; let tol = u.p0.z; let soft = u.p0.w;
            var hsl = rgb_to_hsl(enc(c));
            let d = min(abs(hsl.x - fh), 1.0 - abs(hsl.x - fh));
            let w = 1.0 - clamp((d - tol) / soft, 0.0, 1.0);
            hsl.x = fract_euclid(hsl.x + (th - fh) * w);
            return dec(hsl_to_rgb(hsl.x, hsl.y, hsl.z));
        }
        case OP_COLOR_PASS: {
            let e = enc(c);
            let k = e - u.p0.xyz;
            let d = sqrt(k.x * k.x + k.y * k.y + k.z * k.z);
            let passes = (d <= u.p0.w * 1.2) != (u.i1.x != 0u);
            if passes {
                return c;
            }
            return vec3(luma709(c));
        }
        case OP_GAMMA: {
            let e = enc(c);
            let g = u.p0.x;
            return dec(vec3(powf(max(e.x, 0.0), g), powf(max(e.y, 0.0), g), powf(max(e.z, 0.0), g)));
        }
        case OP_LEVELS: {
            let ib = u.p0.x; let iw = u.p0.y; let ob = u.p0.z; let ow = u.p0.w; let g = u.p1.x;
            let v = clamp((enc(c) - ib) / (iw - ib), vec3(0.0), vec3(1.0));
            return dec(ob + vec3(powf(v.x, g), powf(v.y, g), powf(v.z, g)) * (ow - ob));
        }
        case OP_EXTRACT: {
            let lo = u.p0.x; let hi = u.p0.y; let soft = u.p0.z;
            let l = linear_to_srgb1(max(luma709(c), 0.0));
            // Hard cutoffs avoid both division by zero and an artificial narrow fade
            // that amplifies transfer-function rounding relative to the CPU reference.
            var inside = select(0.0, 1.0, l > lo && l < hi);
            if soft > 0.0 {
                inside = min(clamp((l - lo) / soft, 0.0, 1.0), clamp((hi - l) / soft, 0.0, 1.0));
            }
            return vec3(select(inside, 1.0 - inside, u.i1.x != 0u));
        }
        case OP_INVERT: {
            let ch = u.i1.x;
            let blend = u.p0.x;
            let e = enc(c);
            var o = e;
            if ch == 0u || ch == 1u { o.x = 1.0 - e.x; }
            if ch == 0u || ch == 2u { o.y = 1.0 - e.y; }
            if ch == 0u || ch == 3u { o.z = 1.0 - e.z; }
            return dec(o + (e - o) * blend);
        }
        case OP_POSTERIZE: {
            let n = u.p0.x;
            let e = enc(c) * n;
            return dec(vec3(round_away(e.x), round_away(e.y), round_away(e.z)) / n);
        }
        case OP_ASC_CDL: {
            let e = enc(c);
            let s = u.p0.xyz; let o = u.p1.xyz; let pw = max(u.p2.xyz, vec3(0.0)); let sat = u.p0.w;
            let q = clamp(e * s + o, vec3(0.0), vec3(1.0));
            let v = vec3(powf(q.x, pw.x), powf(q.y, pw.y), powf(q.z, pw.z));
            let l = luma709(v);
            return dec(clamp(l + sat * (v - l), vec3(0.0), vec3(1.0)));
        }
        case OP_CHANNEL_MIX: {
            let v = enc(c);
            let r = u.p0; let g = u.p1; let b = u.p2;
            return dec(clamp(vec3(r.x * v.x + r.y * v.y + r.z * v.z + r.w, g.x * v.x + g.y * v.y + g.z * v.z + g.w, b.x * v.x + b.y * v.y + b.z * v.z + b.w), vec3(0.0), vec3(1.0)));
        }
        case OP_COLOR_REPLACE: {
            let v = enc(c);
            let t = u.p0.xyz; let sim = u.p0.w;
            let dd = v - t;
            let d = sqrt(dd.x * dd.x + dd.y * dd.y + dd.z * dd.z);
            let k = 1.0 - smoothstep_fx(sim * 0.85, max(sim, 1e-4), d);
            if k <= 0.0 {
                return c;
            }
            var repl = u.p1.xyz;
            if u.i1.x == 0u {
                let rh = u.p2.xyz;
                repl = hsl_to_rgb(rh.x, rh.y, rgb_to_hsl(v).z);
            }
            return dec(v + (repl - v) * k);
        }
        default: {
            return c;
        }
    }
}

fn apply_contrast_pivot_wgsl(v: vec3<f32>, contrast: f32, pivot: f32) -> vec3<f32> {
    let p = clamp(pivot, 0.05, 0.95);
    let k = 1.0 + contrast;
    let q = clamp(v, vec3(0.0), vec3(1.0));
    if k < 1.0 {
        return clamp(vec3(p) + (q - vec3(p)) * k, vec3(0.0), vec3(1.0));
    }
    if abs(p - 0.5) < 1e-4 {
        let sc = q * q * (vec3(3.0) - 2.0 * q);
        return q + (sc - q) * (k - 1.0);
    }
    let gamma = 0.6931471805599453 / (-log(p));
    let w = pow(max(q, vec3(1e-7)), vec3(gamma));
    let sw = w * w * (vec3(3.0) - 2.0 * w);
    let s = select(vec3(0.0), pow(max(sw, vec3(1e-7)), vec3(1.0 / gamma)), q > vec3(0.0));
    return q + (s - q) * (k - 1.0);
}

fn lumetri_op(c: vec3<f32>, p: vec2<i32>) -> vec3<f32> {
    let s = size();
    let creative_on = u.i1.x != 0u;
    let vignette_on = u.i1.y != 0u;

    // white balance + exposure in linear light
    let ge = u.p0.xyz;
    let lin = c * ge;
    var v = enc(lin);

    // whites / blacks: endpoints
    let b0 = u.p0.w;
    let w0 = u.p1.x;
    let denom = max(w0 - b0, 1e-3);
    v = (v - vec3(b0)) / denom;

    // highlights / shadows: luma-weighted lift/compress, hue preserving
    let sh_k = u.p1.y;
    let hl_k = u.p1.z;
    let l = luma709(v);
    let ws_val = clamp(1.0 - l, 0.0, 1.0);
    let ws = ws_val * ws_val * ws_val;
    let whl_val = clamp(l, 0.0, 1.0);
    let whl = whl_val * whl_val * whl_val;
    let nl = max(l + sh_k * ws + hl_k * whl, 0.0);
    // below a luma of 0.02 (LIFT_LUMA_FLOOR in effects.rs) the hue-preserving scale nl / l is held
    // at that luma's and the rest of the lift is added equally (lift_scale in effects.rs)
    var k = 1.0;
    if l > 1e-5 {
        k = min(nl / l, 1.0 + max(nl - l, 0.0) / 0.02);
    }
    let add = nl - max(l, 0.0) * k;
    v = v * k + vec3(add);

    // contrast: smooth S-curve around pivot
    let contrast = u.p1.w;
    let pivot = u.p3.w;
    if abs(contrast) > 1e-4 {
        v = apply_contrast_pivot_wgsl(v, contrast, pivot);
    }

    // faded film: lift blacks and compress
    let faded = u.p2.x;
    if faded > 0.0 {
        v = v * (1.0 - 0.25 * faded) + vec3(0.12 * faded);
    }

    // split tone
    if creative_on {
        let st_k = u.p4.xyz;
        let ht_k = u.p5.xyz;
        let l2 = clamp(luma709(v), 0.0, 1.0);
        v = v + st_k * (1.0 - l2) + ht_k * l2;
    }

    // primary hue rotation
    let primary_hue = u.p5.w;
    if abs(primary_hue) > 1e-5 {
        var hh = rgb_to_hsl(clamp(v, vec3(0.0), vec3(1.0)));
        hh.x = fract_euclid(hh.x + primary_hue);
        v = hsl_to_rgb(hh.x, hh.y, hh.z);
    }

    // saturation, vibrance & color boost
    let sat = u.p2.y;
    let vib = u.p2.z;
    let color_boost = u.p4.w;
    let l3 = luma709(v);
    let cur_sat = clamp(max(max(v.x, v.y), v.z) - min(min(v.x, v.y), v.z), 0.0, 1.0);
    let inv_s = 1.0 - cur_sat;
    let sat_mult = max(sat * (1.0 + vib * inv_s + color_boost * inv_s * sqrt(inv_s)), 0.0);
    v = vec3(l3) + (v - vec3(l3)) * sat_mult;

    // vignette
    let va = u.p2.w;
    if vignette_on && abs(va) > 1e-4 {
        let vmid = u.p3.x;
        let vfeather = u.p3.y;
        let vround_aspect = u.p3.z;
        let w = f32(s.x);
        let h = f32(s.y);
        let nx = (f32(p.x) / w - 0.5) * 2.0 * vround_aspect;
        let ny = (f32(p.y) / h - 0.5) * 2.0;
        let d = sqrt(nx * nx + ny * ny) / 1.4142135623730951;
        let edge = clamp((d - vmid * 0.9) / (max(vfeather, 0.01) * 0.9), 0.0, 1.0);
        let e2 = edge * edge * (3.0 - 2.0 * edge);
        let k = 1.0 + va * 0.2 * e2;
        if va < 0.0 {
            v = v * max(k, 0.0);
        } else {
            v = v + (vec3(1.0) - v) * (k - 1.0);
        }
    }

    return dec(v);
}

fn wheel_smoothstep_wgsl(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / max(b - a, 1e-4), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn color_wheels_op(c: vec3<f32>) -> vec3<f32> {
    var v = enc(c);
    let grade_model = u.i1.x;
    let lift_k = u.p0.xyz;
    let gamma_k = u.p1.xyz;
    let gain_k = u.p2.xyz;
    let offset_k = u.p3.xyz;

    if grade_model == 0u {
        let g = max(vec3(1.0) + gain_k, vec3(0.01));
        var x = v * g;
        x = x + lift_k * (vec3(1.0) - x);
        let rx = select(x.x, pow(max(x.x, 1e-12), exp2(-gamma_k.x * 2.0)), abs(gamma_k.x) > 1e-6 && x.x > 0.0);
        let ry = select(x.y, pow(max(x.y, 1e-12), exp2(-gamma_k.y * 2.0)), abs(gamma_k.y) > 1e-6 && x.y > 0.0);
        let rz = select(x.z, pow(max(x.z, 1e-12), exp2(-gamma_k.z * 2.0)), abs(gamma_k.z) > 1e-6 && x.z > 0.0);
        v = vec3(rx, ry, rz) + offset_k;
    } else {
        let l = clamp(0.2126 * v.x + 0.7152 * v.y + 0.0722 * v.z, 0.0, 1.0);
        let wsh = (1.0 - l) * (1.0 - l);
        let whi = l * l;
        let wmid = max(1.0 - wsh - whi, 0.0);

        v += lift_k * wsh;
        v += gamma_k * wmid;
        v *= vec3(1.0) + gain_k * whi;
        v += offset_k;
    }

    return dec(v);
}

fn log_wheels_op(c: vec3<f32>) -> vec3<f32> {
    var v = enc(c);
    let l = clamp(0.2126 * v.x + 0.7152 * v.y + 0.0722 * v.z, 0.0, 1.0);
    let low_r = u.p0.w;
    let high_r = u.p1.w;
    let w_sh = 1.0 - wheel_smoothstep_wgsl(0.0, low_r, l);
    let w_hi = wheel_smoothstep_wgsl(high_r, 1.0, l);
    let w_mid = clamp(1.0 - w_sh - w_hi, 0.0, 1.0);
    v += u.p0.xyz * w_sh + u.p1.xyz * w_mid + u.p2.xyz * w_hi + u.p3.xyz;
    return dec(v);
}

fn hdr_wheels_op(c: vec3<f32>) -> vec3<f32> {
    let z0 = u.p0.xyz;
    let z1 = vec3(u.p0.w, u.p1.x, u.p1.y);
    let z2 = vec3(u.p1.z, u.p1.w, u.p2.x);
    let z3 = vec3(u.p2.y, u.p2.z, u.p2.w);
    let z4 = u.p3.xyz;
    let z5 = vec3(u.p3.w, u.p4.x, u.p4.y);
    let gl = vec3(u.p4.z, u.p4.w, u.p5.x);
    let sigma = max(u.p5.y, 0.25);
    let wheel_range = u.p5.z;

    let y = max(0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z, 1e-5);
    let ev = log2(y / 0.18) - wheel_range;
    let inv_2s2 = 0.5 / (sigma * sigma);
    let w0 = 1.0 - wheel_smoothstep_wgsl(-4.5 - sigma, -4.5 + sigma, ev);
    let w1 = exp(-((ev + 2.5) * (ev + 2.5)) * inv_2s2);
    let w2 = exp(-((ev + 1.0) * (ev + 1.0)) * inv_2s2);
    let w3 = exp(-((ev - 0.5) * (ev - 0.5)) * inv_2s2);
    let w4 = exp(-((ev - 2.0) * (ev - 2.0)) * inv_2s2);
    let w5 = wheel_smoothstep_wgsl(3.5 - sigma, 3.5 + sigma, ev);
    let shift = clamp(gl + w0 * z0 + w1 * z1 + w2 * z2 + w3 * z3 + w4 * z4 + w5 * z5, vec3(-12.0), vec3(12.0));
    return dec(enc(c * exp2(shift)));
}

fn rgb_mixer_op(c: vec3<f32>) -> vec3<f32> {
    let v = enc(c);
    let out = vec3(dot(u.p0.xyz, v), dot(u.p1.xyz, v), dot(u.p2.xyz, v));
    return dec(out);
}

fn cst_pq_eotf(e: f32) -> f32 {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let p = pow(clamp(e, 0.0, 1.0), 1.0 / m2);
    return pow(max(p - c1, 0.0) / max(c2 - c3 * p, 1e-6), 1.0 / m1);
}

fn cst_pq_inv(y: f32) -> f32 {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let p = pow(clamp(y, 0.0, 1.0), m1);
    return pow((c1 + c2 * p) / (1.0 + c3 * p), m2);
}

fn cst_decode_ch(gamma: u32, v: f32) -> f32 {
    switch gamma {
        case 0u, 1u: {
            return select(srgb_to_linear1(v), -srgb_to_linear1(-v), v < 0.0);
        }
        case 2u: {
            return v;
        }
        case 3u: {
            return cst_pq_eotf(v) * 10000.0 / 203.0;
        }
        case 4u: {
            let x = clamp(v, 0.0, 1.0);
            let a = 0.17883277;
            let b = 0.28466892;
            let c = 0.55991073;
            return select((exp((x - c) / a) + b) / 12.0, x * x / 3.0, x <= 0.5);
        }
        case 5u: {
            let di_c = 0.07329248;
            let di_m = 10.44426855;
            return select(exp2(v / di_c - 7.0) - 0.0075, v / di_m, v <= 0.02740668);
        }
        case 6u: {
            let a = 10.5402377416545;
            let b = 0.0729055341958355;
            return select(min(exp2(v * 17.52 - 9.72), 65504.0), (v - b) / a, v <= 0.155251141552511);
        }
        case 7u: {
            return select(
                (v * 1023.0 - 95.0) * 0.01125 / (171.2102946929 - 95.0),
                pow(10.0, (v * 1023.0 - 420.0) / 261.5) * 0.19 - 0.01,
                v >= 171.2102946929 / 1023.0,
            );
        }
        case 8u: {
            return select((v - 0.125) / 5.6, pow(10.0, (v - 0.598206) / 0.241514) - 0.00873, v >= 0.181);
        }
        case 9u: {
            let s = 0.24136077;
            let b = 0.092864125;
            let raw = select(
                -(pow(10.0, (b - v) / s) - 1.0) / 87.09937546,
                (pow(10.0, (v - b) / s) - 1.0) / 87.09937546,
                v >= b,
            );
            return raw * 0.9;
        }
        case 10u: {
            var x = 0.0;
            if v < 0.097465473 {
                x = -(pow(10.0, (0.12783901 - v) / 0.36726845) - 1.0) / 14.98325;
            } else if v <= 0.15277891 {
                x = (v - 0.12512219) / 1.9754798;
            } else {
                x = (pow(10.0, (v - 0.12240537) / 0.36726845) - 1.0) / 14.98325;
            }
            return x * 0.9;
        }
        case 11u: {
            let cut = 0.010591 * 5.367655 + 0.092809;
            return select((v - 0.092809) / 5.367655, (pow(10.0, (v - 0.385537) / 0.247190) - 0.052272) / 5.555556, v > cut);
        }
        case 12u: {
            let a = (262144.0 - 16.0) / 117.45;
            let b = (1023.0 - 95.0) / 1023.0;
            let c = 95.0 / 1023.0;
            let s = (7.0 * 0.6931471805599453 * exp2(7.0 - 14.0 * c / b)) / (a * b);
            let t = (exp2(14.0 * (-c / b) + 6.0) - 64.0) / a;
            return select(v * s + t, (exp2(14.0 * (v - c) / b + 6.0) - 64.0) / a, v >= 0.0);
        }
        case 13u: {
            let r0 = -0.05641088;
            let rt = 0.01;
            let c_al = 47.28711236;
            let pt = c_al * (rt - r0) * (rt - r0);
            if v >= pt {
                return exp2((v - 0.69336945) / 0.08550479) - 0.00964052;
            } else if v >= 0.0 {
                return sqrt(v / c_al) + r0;
            }
            return r0;
        }
        case 14u: {
            return select((v - 0.0929) / 6.025, (pow(10.0, 3.89616 * v - 2.27752) - 0.0108) / 0.9892, v > 0.14);
        }
        case 15u: {
            return select((v - 0.092864) / 8.735631, (pow(10.0, (v - 0.790453) / 0.344676) - 0.009468) / 0.555556, v >= 0.100537775223865);
        }
        default: {
            return select((v - 0.092864) / 8.799461, (pow(10.0, (v - 0.384316) / 0.245281) - 0.064829) / 5.555556, v >= 0.100686685370811);
        }
    }
}

fn cst_encode_ch(gamma: u32, x: f32) -> f32 {
    switch gamma {
        case 0u, 1u: {
            return select(linear_to_srgb1(x), -linear_to_srgb1(-x), x < 0.0);
        }
        case 2u: {
            return x;
        }
        case 3u: {
            return cst_pq_inv(max(x, 0.0) * 203.0 / 10000.0);
        }
        case 4u: {
            let v = clamp(x, 0.0, 1.0);
            return select(0.17883277 * log(max(12.0 * v - 0.28466892, 1e-6)) + 0.55991073, sqrt(3.0 * v), v <= 1.0 / 12.0);
        }
        case 5u: {
            return select((log2(max(x + 0.0075, 1e-6)) + 7.0) * 0.07329248, x * 10.44426855, x <= 0.00262409);
        }
        case 6u: {
            return select((log2(max(x, 1e-6)) + 9.72) / 17.52, 10.5402377416545 * x + 0.0729055341958355, x <= 0.0078125);
        }
        case 7u: {
            return select((x * (171.2102946929 - 95.0) / 0.01125 + 95.0) / 1023.0, (420.0 + log2(max((x + 0.01) / 0.19, 1e-6)) / log2(10.0) * 261.5) / 1023.0, x >= 0.01125);
        }
        case 8u: {
            return select(5.6 * x + 0.125, 0.241514 * log(max(x + 0.00873, 1e-6)) / log(10.0) + 0.598206, x >= 0.01);
        }
        case 9u: {
            let r = x / 0.9;
            return select(-0.24136077 * log(max(-r * 87.09937546 + 1.0, 1e-6)) / log(10.0) + 0.092864125, 0.24136077 * log(max(r * 87.09937546 + 1.0, 1e-6)) / log(10.0) + 0.092864125, r >= 0.0);
        }
        case 10u: {
            let r = x / 0.9;
            if r < -0.014 {
                return -0.36726845 * log(max(-r * 14.98325 + 1.0, 1e-6)) / log(10.0) + 0.12783901;
            } else if r <= 0.014 {
                return 1.9754798 * r + 0.12512219;
            }
            return 0.36726845 * log(max(r * 14.98325 + 1.0, 1e-6)) / log(10.0) + 0.12240537;
        }
        case 11u: {
            return select(5.367655 * x + 0.092809, 0.247190 * log(max(5.555556 * x + 0.052272, 1e-6)) / log(10.0) + 0.385537, x > 0.010591);
        }
        case 12u: {
            let a = (262144.0 - 16.0) / 117.45;
            let b = (1023.0 - 95.0) / 1023.0;
            let c = 95.0 / 1023.0;
            let s = (7.0 * 0.6931471805599453 * exp2(7.0 - 14.0 * c / b)) / (a * b);
            let t = (exp2(14.0 * (-c / b) + 6.0) - 64.0) / a;
            return select((x - t) / s, ((log2(max(a * x + 64.0, 1e-6)) - 6.0) / 14.0) * b + c, x >= t);
        }
        case 13u: {
            if x >= 0.01 {
                return 0.08550479 * log2(max(x + 0.00964052, 1e-6)) + 0.69336945;
            } else if x >= -0.05641088 {
                let d = x + 0.05641088;
                return 47.28711236 * d * d;
            }
            return 0.0;
        }
        case 14u: {
            return select(0.256663 * log(max(x * 0.9892 + 0.0108, 1e-6)) / log(10.0) + 0.584555, 6.025 * x + 0.0929, x <= 0.0078);
        }
        case 15u: {
            return select(8.735631 * x + 0.092864, 0.344676 * log(max(0.555556 * x + 0.009468, 1e-6)) / log(10.0) + 0.790453, x >= 0.00089);
        }
        default: {
            return select(8.799461 * x + 0.092864, 0.245281 * log(max(5.555556 * x + 0.064829, 1e-6)) / log(10.0) + 0.384316, x >= 0.000889);
        }
    }
}

fn cst_bt2390_ratio(lum: f32, src_peak: f32, dst_peak: f32) -> f32 {
    if dst_peak >= src_peak || lum <= 1e-6 {
        return 1.0;
    }
    let nits = lum * 203.0;
    let smax = max(cst_pq_inv(src_peak / 10000.0), 1e-5);
    let e1 = min(cst_pq_inv(nits / 10000.0) / smax, 1.0);
    let max_lum = cst_pq_inv(dst_peak / 10000.0) / smax;
    let ks = 1.5 * max_lum - 0.5;
    var e2 = e1;
    if e1 >= ks {
        let t = (e1 - ks) / max(1.0 - ks, 1e-5);
        let t2 = t * t;
        let t3 = t2 * t;
        e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks) + (-2.0 * t3 + 3.0 * t2) * max_lum;
    }
    let mapped_nits = max(cst_pq_eotf(e2 * smax) * 10000.0, 0.0);
    return (mapped_nits / 203.0) / lum;
}

fn cst_compress_ch(v: f32, ach: f32, thr: f32, scale: f32, pwr: f32) -> f32 {
    let dist = (ach - v) / ach;
    if dist <= thr {
        return v;
    }
    let nd = dist - thr;
    let comp = thr + scale * (nd / scale) / pow(1.0 + pow(nd / scale, pwr), 1.0 / pwr);
    return max(ach * (1.0 - comp), 0.0);
}

fn cst_op(c: vec3<f32>) -> vec3<f32> {
    let v = enc(c);
    let in_g = u.i1.x;
    let out_g = u.i1.y;
    let tone_map = u.i1.z;
    let gamut_map = u.i1.w;
    var lin = vec3(cst_decode_ch(in_g, v.x), cst_decode_ch(in_g, v.y), cst_decode_ch(in_g, v.z));
    lin = vec3(dot(u.p0.xyz, lin), dot(u.p1.xyz, lin), dot(u.p2.xyz, lin));
    let out_luma = vec3(u.p0.w, u.p1.w, u.p2.w);

    if tone_map != 0u && u.p3.y < u.p3.x {
        var m = 0.0;
        if tone_map == 1u {
            m = max(max(max(lin.x, lin.y), lin.z), 0.0);
        } else {
            m = max(dot(out_luma, lin), 0.0);
        }
        lin *= cst_bt2390_ratio(m, u.p3.x, u.p3.y);
    }

    if gamut_map == 1u {
        let ach = max(max(lin.x, lin.y), lin.z);
        if ach <= 1e-6 {
            lin = max(lin, vec3(0.0));
        } else {
            let thr = 0.8;
            let lim = 1.25;
            let pwr = 1.2;
            let scale = (lim - thr) / max((1.0 - thr) / pow(max(lim - 1.0, 1e-4) / (lim - thr), -pwr) - 1.0, 1e-5);
            lin = vec3(
                cst_compress_ch(lin.x, ach, thr, scale, pwr),
                cst_compress_ch(lin.y, ach, thr, scale, pwr),
                cst_compress_ch(lin.z, ach, thr, scale, pwr),
            );
        }
    } else if gamut_map == 2u {
        let mn = min(min(lin.x, lin.y), lin.z);
        if mn < 0.0 {
            let y = max(dot(out_luma, lin), 0.0);
            if y <= 1e-6 {
                lin = vec3(0.0);
            } else {
                let k = clamp(y / (y - mn), 0.0, 1.0);
                lin = vec3(y) + (lin - vec3(y)) * k;
            }
        }
    }

    let out_v = vec3(cst_encode_ch(out_g, lin.x), cst_encode_ch(out_g, lin.y), cst_encode_ch(out_g, lin.z));
    return dec(out_v);
}

fn node_sizing_px(p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let w = f32(s.x);
    let h = f32(s.y);
    let cx = w * 0.5;
    let cy = h * 0.5;
    let px = f32(p.x) + 0.5;
    let py = f32(p.y) + 0.5;
    let blanking = u.p1.w;
    if blanking > 0.0 {
        let target_h = min(w / blanking, h);
        let bar = 0.5 * (h - target_h);
        if py < bar || py > h - bar {
            return vec4(0.0, 0.0, 0.0, 1.0);
        }
    }
    let dx = px - cx - u.p0.x * cx;
    let dy = py - cy - u.p0.y * cy;
    let cos_r = u.p0.w;
    let sin_r = u.p1.z;
    let rx = dx * cos_r + dy * sin_r;
    let ry = -dx * sin_r + dy * cos_r;
    let sx_off = rx * u.p0.z * u.p1.x;
    let sy_off = ry * u.p0.z * u.p1.y;
    let sx = cx + select(sx_off, -sx_off, u.i1.x != 0u);
    let sy = cy + select(sy_off, -sy_off, u.i1.y != 0u);
    return sample_bilinear(sx, sy);
}

fn s_curve_wgsl(v: f32, k: f32) -> f32 {
    let x = clamp(v, 0.0, 1.0);
    return x + (x * x * (3.0 - 2.0 * x) - x) * k;
}

fn s_curve3_wgsl(c: vec3<f32>, k: f32) -> vec3<f32> {
    return vec3<f32>(
        s_curve_wgsl(c.x, k),
        s_curve_wgsl(c.y, k),
        s_curve_wgsl(c.z, k),
    );
}

fn apply_look_wgsl(look: u32, c: vec3<f32>) -> vec3<f32> {
    let l = 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
    switch look {
        case 1u: {
            let shadow = vec3<f32>(0.0, 0.08, 0.1);
            let high = vec3<f32>(0.1, 0.04, -0.06);
            let col = c + shadow * (1.0 - l) + high * l;
            return s_curve3_wgsl(col, 0.35);
        }
        case 2u: {
            let col = vec3<f32>(c.x * 1.06 + 0.02, c.y * 1.0 + 0.01, c.z * 0.9);
            return mix(col, vec3<f32>(l), 0.15) * 0.94 + vec3<f32>(0.04);
        }
        case 3u: {
            let col = vec3<f32>(c.x * 0.9, c.y * 0.98, c.z * 1.1 + 0.02);
            return s_curve3_wgsl(col, 0.2);
        }
        case 4u: {
            let col = mix(c, vec3<f32>(l), 0.55);
            return s_curve3_wgsl(col, 0.6);
        }
        case 5u: {
            let col = mix(c, vec3<f32>(l), 0.25);
            return vec3<f32>(0.08) + col * 0.84;
        }
        case 6u: {
            return s_curve3_wgsl(vec3<f32>(l), 0.3);
        }
        case 7u: {
            let col = vec3<f32>(c.x * 1.1 + 0.03, c.y * 1.02 + 0.01, c.z * 0.82);
            return s_curve3_wgsl(col, 0.15);
        }
        case 8u: {
            let col = vec3<f32>(c.x * 0.75, c.y * 0.85, c.z * 1.15);
            return col * 0.8;
        }
        default: {
            return c;
        }
    }
}

fn lumetri_look_op(c: vec3<f32>) -> vec3<f32> {
    var v = enc(c);
    let look = u.i1.x;
    let look_k = u.p0.x;
    let lk = apply_look_wgsl(look, v);
    v = v + (lk - v) * look_k;
    return dec(v);
}

fn lut_at(r: u32, g: u32, b: u32, n: u32) -> vec3<f32> {
    return textureLoad(aux, vec2<i32>(i32(r + g * n), i32(b)), 0).rgb;
}

fn tetra_lut(c: vec3<f32>, n: u32, dmin: vec3<f32>, dmax: vec3<f32>) -> vec3<f32> {
    let m = f32(n - 1u);
    let t = clamp((c - dmin) / (dmax - dmin), vec3(0.0), vec3(1.0)) * m;
    let i = min(vec3<u32>(t), vec3(n - 2u));
    let f = t - vec3<f32>(i);
    let fr = f.x;
    let fg = f.y;
    let fb = f.z;
    let c000 = lut_at(i.x, i.y, i.z, n);
    let c111 = lut_at(i.x + 1u, i.y + 1u, i.z + 1u, n);
    var ca: vec3<f32>;
    var cb: vec3<f32>;
    var w: vec4<f32>;
    if (fr > fg) {
        if (fg > fb) {
            ca = lut_at(i.x + 1u, i.y, i.z, n); cb = lut_at(i.x + 1u, i.y + 1u, i.z, n); w = vec4(1.0 - fr, fr - fg, fg - fb, fb);
        } else if (fr > fb) {
            ca = lut_at(i.x + 1u, i.y, i.z, n); cb = lut_at(i.x + 1u, i.y, i.z + 1u, n); w = vec4(1.0 - fr, fr - fb, fb - fg, fg);
        } else {
            ca = lut_at(i.x, i.y, i.z + 1u, n); cb = lut_at(i.x + 1u, i.y, i.z + 1u, n); w = vec4(1.0 - fb, fb - fr, fr - fg, fg);
        }
    } else if (fb > fg) {
        ca = lut_at(i.x, i.y, i.z + 1u, n); cb = lut_at(i.x, i.y + 1u, i.z + 1u, n); w = vec4(1.0 - fb, fb - fg, fg - fr, fr);
    } else if (fb > fr) {
        ca = lut_at(i.x, i.y + 1u, i.z, n); cb = lut_at(i.x, i.y + 1u, i.z + 1u, n); w = vec4(1.0 - fg, fg - fb, fb - fr, fr);
    } else {
        ca = lut_at(i.x, i.y + 1u, i.z, n); cb = lut_at(i.x + 1u, i.y, i.z, n); w = vec4(1.0 - fg, fg - fr, fr - fb, fb);
    }
    return w.x * c000 + w.y * ca + w.z * cb + w.w * c111;
}

fn lut_3d_op(c: vec3<f32>) -> vec3<f32> {
    let n = u.i1.x;
    let dmin = u.p0.xyz;
    let intensity = u.p0.w;
    let dmax = u.p1.xyz;
    let in_c = enc(c);
    let lut_c = tetra_lut(in_c, n, dmin, dmax);
    let blended = in_c + (lut_c - in_c) * intensity;
    return dec(blended);
}

const CURVE_LEN: u32 = 1024u;

fn sample_curve(row: i32, ch: u32, x: f32) -> f32 {
    let p = clamp(x, 0.0, 1.0) * f32(CURVE_LEN - 1u);
    let i = min(u32(p), CURVE_LEN - 1u);
    let j = min(i + 1u, CURVE_LEN - 1u);
    let a = textureLoad(aux, vec2<i32>(i32(i), row), 0)[ch];
    let b = textureLoad(aux, vec2<i32>(i32(j), row), 0)[ch];
    return a + (b - a) * (p - f32(i));
}

fn apply_soft_clip_wgsl(v_in: vec3<f32>, low: f32, high: f32) -> vec3<f32> {
    var v = v_in;
    if low > 1e-5 {
        let kl = clamp(low * 0.35, 1e-5, 0.35);
        let toe = kl * exp(-(kl - v) / kl);
        v = select(v, toe, v < vec3(kl));
    }
    if high > 1e-5 {
        let kh = clamp(high * 0.35, 1e-5, 0.35);
        let s = 1.0 - kh;
        let sh = s + kh * (1.0 - exp(-(v - s) / kh));
        v = select(v, sh, v > vec3(s));
    }
    return v;
}

fn lumetri_curves_op(c: vec3<f32>) -> vec3<f32> {
    var v = enc(c);
    let m = u.i1.x;
    if (m & 1u) != 0u {
        v = vec3(sample_curve(0, 0u, v.x), sample_curve(0, 0u, v.y), sample_curve(0, 0u, v.z));
    }
    if (m & 2u) != 0u {
        v.x = sample_curve(0, 1u, v.x);
    }
    if (m & 4u) != 0u {
        v.y = sample_curve(0, 2u, v.y);
    }
    if (m & 8u) != 0u {
        v.z = sample_curve(0, 3u, v.z);
    }
    let low_soft = u.p0.x;
    let high_soft = u.p0.y;
    if low_soft > 1e-5 || high_soft > 1e-5 {
        v = apply_soft_clip_wgsl(v, low_soft, high_soft);
    }
    if (m & 0x3F0u) != 0u {
        let uc = clamp(v, vec3(0.0), vec3(1.0));
        var h = rgb_to_hsl(uc);
        let h0 = h.x;
        let s0 = h.y;
        let l0 = h.z;
        if (m & 16u) != 0u {
            h.x = fract_euclid(h.x + (sample_curve(1, 0u, h0) - 0.5));
        }
        var sm = 1.0;
        if (m & 32u) != 0u {
            sm *= sample_curve(1, 1u, h0) * 2.0;
        }
        if (m & 64u) != 0u {
            sm *= sample_curve(1, 2u, l0) * 2.0;
        }
        if (m & 128u) != 0u {
            sm *= sample_curve(1, 3u, s0) * 2.0;
        }
        h.y = clamp(h.y * sm, 0.0, 1.0);
        if (m & 256u) != 0u {
            h.z = clamp(h.z + (sample_curve(2, 0u, h0) - 0.5) * 0.5, 0.0, 1.0);
        }
        if (m & 512u) != 0u {
            h.z = clamp(h.z + (sample_curve(2, 1u, s0) - 0.5) * 0.5, 0.0, 1.0);
        }
        v = hsl_to_rgb(h.x, h.y, h.z);
    }
    return dec(v);
}

fn cs_band_weight(h: f32, center: f32, hw: f32) -> f32 {
    let d0 = abs(h - center);
    let d = min(d0, 1.0 - d0);
    if d >= hw {
        return 0.0;
    }
    return 0.5 + 0.5 * cos(3.141592653589793 * d / hw);
}

fn color_slice_op(c: vec3<f32>) -> vec3<f32> {
    var h = rgb_to_hsl(clamp(enc(c), vec3(0.0), vec3(1.0)));
    let gate = smoothstep(0.0, 0.22, h.y);
    if gate <= 1e-5 {
        return c;
    }
    let w0 = cs_band_weight(h.x, 0.0, 1.0 / 6.0) * gate;
    let w1 = cs_band_weight(h.x, 1.0 / 6.0, 1.0 / 6.0) * gate;
    let w2 = cs_band_weight(h.x, 2.0 / 6.0, 1.0 / 6.0) * gate;
    let w3 = cs_band_weight(h.x, 3.0 / 6.0, 1.0 / 6.0) * gate;
    let w4 = cs_band_weight(h.x, 4.0 / 6.0, 1.0 / 6.0) * gate;
    let w5 = cs_band_weight(h.x, 5.0 / 6.0, 1.0 / 6.0) * gate;
    let w6 = cs_band_weight(h.x, 25.0 / 360.0, 32.0 / 360.0) * gate;
    let norm = max(w0 + w1 + w2 + w3 + w4 + w5 + w6, 1.0);
    let sat_d = (w0 * u.p0.x + w1 * u.p0.w + w2 * u.p1.z + w3 * u.p2.y + w4 * u.p3.x + w5 * u.p3.w + w6 * u.p4.z) / norm;
    let den_d = (w0 * u.p0.y + w1 * u.p1.x + w2 * u.p1.w + w3 * u.p2.z + w4 * u.p3.y + w5 * u.p4.x + w6 * u.p4.w) / norm;
    let hue_d = (w0 * u.p0.z + w1 * u.p1.y + w2 * u.p2.x + w3 * u.p2.w + w4 * u.p3.z + w5 * u.p4.y + w6 * u.p5.x) / norm;
    h.x = fract_euclid(h.x + hue_d);
    h.y = clamp(h.y * (1.0 + sat_d), 0.0, 1.0);
    let rgb = hsl_to_rgb(h.x, h.y, h.z);
    let den_scale = clamp(1.0 - den_d * 0.45 * smoothstep(0.0, 0.35, h.y), 0.15, 1.85);
    return dec(rgb * den_scale);
}

const CW_W: u32 = 128u;
const CW_H: u32 = 64u;

fn sample_warper_2d(u_c: f32, v_c: f32) -> vec4<f32> {
    let fx = clamp(u_c, 0.0, 1.0) * f32(CW_W - 1u);
    let fy = clamp(v_c, 0.0, 1.0) * f32(CW_H - 1u);
    let x0 = u32(fx);
    let y0 = u32(fy);
    let x1 = min(x0 + 1u, CW_W - 1u);
    let y1 = min(y0 + 1u, CW_H - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let p00 = textureLoad(aux, vec2<i32>(i32(x0), i32(y0)), 0);
    let p10 = textureLoad(aux, vec2<i32>(i32(x1), i32(y0)), 0);
    let p01 = textureLoad(aux, vec2<i32>(i32(x0), i32(y1)), 0);
    let p11 = textureLoad(aux, vec2<i32>(i32(x1), i32(y1)), 0);
    return mix(mix(p00, p10, tx), mix(p01, p11, tx), ty);
}

fn color_warper_op(c: vec3<f32>) -> vec3<f32> {
    var h = rgb_to_hsl(clamp(enc(c), vec3(0.0), vec3(1.0)));
    let m = u.i1.x;
    if (m & 1u) != 0u {
        let s = sample_warper_2d(h.x, h.y);
        let s2 = clamp(sqrt(s.x * s.x + s.y * s.y), 0.0, 1.0);
        if s2 > 1e-4 {
            h.x = fract_euclid(atan2(s.y, s.x) / 6.283185307179586);
        }
        h.y = s2;
    }
    if (m & 2u) != 0u {
        let s = sample_warper_2d(h.x, h.z);
        h.y = clamp(h.y * (1.0 + s.z * 1.5), 0.0, 1.0);
        h.z = clamp(h.z + s.w * 0.4, 0.0, 1.0);
    }
    return dec(hsl_to_rgb(h.x, h.y, h.z));
}

fn range_gate_wgsl(val: f32, lo: f32, hi: f32, soft: f32) -> f32 {
    var low_g = 1.0;
    if lo > 0.001 {
        low_g = clamp((val - lo) / soft, 0.0, 1.0);
    }
    var high_g = 1.0;
    if hi < 0.999 {
        high_g = clamp((hi - val) / soft, 0.0, 1.0);
    }
    return min(low_g, high_g);
}

fn hsl_key_px(p: vec2<i32>) -> vec4<f32> {
    let o = ld(p);
    if o.a <= 1e-6 {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    let uv = clamp(enc(o.rgb / o.a), vec3(0.0), vec3(1.0));
    let mode = u.i1.x;
    let soft = u.p1.z;
    var m: f32 = 0.0;
    if mode == 1u {
        let r_g = range_gate_wgsl(uv.r, u.p2.y, u.p3.x, u.p3.w);
        let g_g = range_gate_wgsl(uv.g, u.p2.z, u.p3.y, u.p3.w);
        let b_g = range_gate_wgsl(uv.b, u.p2.w, u.p3.z, u.p3.w);
        m = r_g * g_g * b_g;
    } else if mode == 2u {
        let y = luma709(uv);
        m = range_gate_wgsl(y, u.p1.x, u.p1.y, u.p2.x);
    } else if mode == 3u {
        let n_pts = min(u.i1.y, 64u);
        if n_pts > 0u {
            let yp = luma709(uv);
            let cb = uv.b - yp;
            let cr = uv.r - yp;
            var w_pos: f32 = 0.0;
            var w_neg: f32 = 0.0;
            for (var idx = 0u; idx < n_pts; idx++) {
                let s = textureLoad(aux, vec2<i32>(i32(idx), 0), 0);
                let sy = luma709(s.rgb);
                let scb = s.b - sy;
                let scr = s.r - sy;
                let dy = (yp - sy) * 0.45;
                let dcb = cb - scb;
                let dcr = cr - scr;
                let dist = sqrt(dy * dy + dcb * dcb + dcr * dcr);
                let w = 1.0 - clamp((dist - abs(s.w)) / soft, 0.0, 1.0);
                if s.w >= 0.0 {
                    w_pos = max(w_pos, w);
                } else {
                    w_neg = max(w_neg, w);
                }
            }
            m = clamp(w_pos * (1.0 - w_neg), 0.0, 1.0);
        }
    } else {
        let h = rgb_to_hsl(uv);
        let hc = u.p0.x;
        let hr = u.p0.y;
        let smin = u.p0.z;
        let smax = u.p0.w;
        let lmin = u.p1.x;
        let lmax = u.p1.y;
        let sat_soft = u.p1.w;
        let luma_soft = u.p2.x;
        let d0 = abs(h.x - hc);
        let dh = min(d0, 1.0 - d0);
        let mh = 1.0 - clamp((dh - hr * 0.5) / soft, 0.0, 1.0);
        let ms = range_gate_wgsl(h.y, smin, smax, sat_soft);
        let ml = range_gate_wgsl(h.z, lmin, lmax, luma_soft);
        m = mh * ms * ml;
    }
    return vec4(m, 0.0, 0.0, 1.0);
}

fn hsl_morph_px(p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let orig_m = ld(p).r;
    let r = clamp(i32(u.i1.x), 1, 3);
    let dilate = u.i1.y != 0u;
    let strength = clamp(u.p0.x, 0.0, 1.0);
    var best = orig_m;
    let y0 = max(0, p.y - r);
    let y1 = min(s.y - 1, p.y + r);
    let x0 = max(0, p.x - r);
    let x1 = min(s.x - 1, p.x + r);
    for (var yy = y0; yy <= y1; yy++) {
        for (var xx = x0; xx <= x1; xx++) {
            let v = ld(vec2(xx, yy)).r;
            if dilate {
                best = max(best, v);
            } else {
                best = min(best, v);
            }
        }
    }
    return vec4(orig_m + (best - orig_m) * strength, 0.0, 0.0, 1.0);
}

fn node_key_finish_px(p: vec2<i32>) -> vec4<f32> {
    var m: f32 = 1.0;
    if u.i1.y != 0u {
        m = ld(p).r;
        let clean_b = u.p0.x;
        let clean_w = u.p0.y;
        if clean_b > 1e-4 || clean_w > 1e-4 {
            let lo = clamp(clean_b, 0.0, 0.99) * 0.85;
            let hi = max(lo + 1e-3, 1.0 - clamp(clean_w, 0.0, 0.99) * 0.85);
            m = clamp((m - lo) / (hi - lo), 0.0, 1.0);
        } else {
            m = clamp(m, 0.0, 1.0);
        }
        if u.p0.z > 0.5 {
            m = 1.0 - m;
        }
    }
    if u.i1.x != 0u {
        m *= textureLoad(mask_tex, p, 0).r;
    }
    m = clamp(m * u.p0.w, 0.0, 1.0);
    return vec4(m, 0.0, 0.0, 1.0);
}

fn hsl_median_px(p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let orig_m = ld(p).r;
    let r = clamp(i32(u.i1.x), 1, 3);
    let denoise = clamp(u.p0.x, 0.0, 1.0);
    var win: array<f32, 49>;
    var count: i32 = 0;
    let y0 = max(0, p.y - r);
    let y1 = min(s.y - 1, p.y + r);
    let x0 = max(0, p.x - r);
    let x1 = min(s.x - 1, p.x + r);
    for (var yy = y0; yy <= y1; yy++) {
        for (var xx = x0; xx <= x1; xx++) {
            win[count] = ld(vec2(xx, yy)).r;
            count++;
        }
    }
    if count == 0 {
        return vec4(orig_m, 0.0, 0.0, 1.0);
    }
    let mid_idx = count / 2;
    for (var i = 0; i <= mid_idx; i++) {
        var min_j = i;
        var min_v = win[i];
        for (var j = i + 1; j < count; j++) {
            if win[j] < min_v {
                min_v = win[j];
                min_j = j;
            }
        }
        if min_j != i {
            win[min_j] = win[i];
            win[i] = min_v;
        }
    }
    let med = win[mid_idx];
    return vec4(orig_m + (med - orig_m) * denoise, 0.0, 0.0, 1.0);
}

fn apply_highlight_px(uv: vec3<f32>, m: f32, show_mask: u32) -> vec3<f32> {
    switch show_mask {
        case 1u: {
            let g = rgb_to_hsl(uv).z;
            return vec3(g) + (uv - vec3(g)) * m;
        }
        case 2u: {
            return uv * m;
        }
        case 3u: {
            return vec3(m);
        }
        default: {
            return uv;
        }
    }
}

fn hsl_apply_px(p: vec2<i32>) -> vec4<f32> {
    let full_unmasked = u.i1.y != 0u;
    var o = ld(p);
    var m: f32 = 1.0;
    if !full_unmasked {
        o = textureLoad(aux, p, 0);
        m = ld(p).r;
    }
    let a = o.a;
    if a <= 1e-6 {
        return o;
    }
    let uv = clamp(enc(o.rgb / a), vec3(0.0), vec3(1.0));
    let show_mask = u.i1.x;
    var out_c: vec3<f32>;
    if show_mask > 0u && !full_unmasked {
        out_c = apply_highlight_px(uv, m, show_mask);
    } else {
        let htemp = u.p0.x;
        let htint = u.p0.y;
        let hsat = u.p0.z;
        let hshift = u.p0.w;
        var hh = rgb_to_hsl(uv);
        hh.x = fract_euclid(hh.x + hshift);
        hh.y = clamp(hh.y * hsat, 0.0, 1.0);
        var c2 = hsl_to_rgb(hh.x, hh.y, hh.z);
        c2.x *= 1.0 + 0.25 * htemp;
        c2.z *= 1.0 - 0.25 * htemp;
        c2.y *= 1.0 - 0.2 * htint;
        out_c = uv + (c2 - uv) * m;
    }
    return vec4(dec(out_c) * a, a);
}

fn mask_mix_px(p: vec2<i32>) -> vec4<f32> {
    let cur_px = ld(p);
    let orig_px = textureLoad(aux, p, 0);
    let c = textureLoad(mask_tex, p, 0).r;
    return orig_px + (cur_px - orig_px) * c;
}

fn dag_key_combine_px(p: vec2<i32>) -> vec4<f32> {
    var m: f32 = 1.0;
    if u.i1.x != 0u {
        var qm = ld(p).r;
        let clean_b = u.p0.x;
        let clean_w = u.p0.y;
        if clean_b > 1e-4 || clean_w > 1e-4 {
            let lo = clamp(clean_b, 0.0, 0.99) * 0.85;
            let hi = max(lo + 1e-3, 1.0 - clamp(clean_w, 0.0, 0.99) * 0.85);
            qm = clamp((qm - lo) / (hi - lo), 0.0, 1.0);
        } else {
            qm = clamp(qm, 0.0, 1.0);
        }
        if u.p0.z > 0.5 {
            qm = 1.0 - qm;
        }
        m *= qm;
    }
    if u.i1.y != 0u {
        m *= clamp(textureLoad(mask_tex, p, 0).r, 0.0, 1.0);
    }
    if u.i1.z != 0u {
        var uk = clamp(textureLoad(aux, p, 0).r, 0.0, 1.0);
        if u.i1.w != 0u {
            uk = 1.0 - uk;
        }
        uk = clamp(uk * u.p0.w, 0.0, 1.0);
        m *= uk;
    }
    if u.p1.y > 0.5 {
        m = 1.0 - m;
    }
    m = clamp(m * u.p1.x, 0.0, 1.0);
    return vec4(m, m, m, 1.0);
}

fn dag_parallel_px(p: vec2<i32>) -> vec4<f32> {
    let acc = ld(p);
    let br = textureLoad(aux, p, 0);
    if u.i1.x != 0u {
        let bs = textureLoad(mask_tex, p, 0);
        return vec4(max(acc.rgb + (br.rgb - bs.rgb), vec3(0.0)), acc.a);
    }
    return vec4(acc.rgb + (br.rgb - acc.rgb) * u.p0.x, acc.a);
}

fn dag_blend_ch(b: f32, t: f32, mode: u32) -> f32 {
    let bc = clamp(b, 0.0, 1.0);
    let tc = clamp(t, 0.0, 1.0);
    switch mode {
        case 1u: { return b * t; }
        case 2u: { return 1.0 - (1.0 - bc) * (1.0 - tc); }
        case 3u: { return select(1.0 - 2.0 * (1.0 - bc) * (1.0 - tc), 2.0 * bc * tc, bc < 0.5); }
        case 4u: { return (1.0 - 2.0 * tc) * bc * bc + 2.0 * tc * bc; }
        case 5u: { return select(1.0 - 2.0 * (1.0 - bc) * (1.0 - tc), 2.0 * bc * tc, tc < 0.5); }
        case 6u: { return b + t; }
        case 7u: { return max(b - t, 0.0); }
        case 8u: { return min(b, t); }
        case 9u: { return max(b, t); }
        default: { return t; }
    }
}

fn dag_layer_px(p: vec2<i32>) -> vec4<f32> {
    let bot = ld(p);
    let top = textureLoad(aux, p, 0);
    var k: f32 = 1.0;
    if u.i1.y != 0u {
        k = clamp(textureLoad(mask_tex, p, 0).r, 0.0, 1.0);
        if u.i1.z != 0u {
            k = 1.0 - k;
        }
    }
    k = clamp(k * u.p0.x, 0.0, 1.0);
    let mode = u.i1.x;
    var blended: vec3<f32>;
    if mode == 10u {
        let bh = rgb_to_hsl(clamp(bot.rgb, vec3(0.0), vec3(1.0)));
        let th = rgb_to_hsl(clamp(top.rgb, vec3(0.0), vec3(1.0)));
        blended = hsl_to_rgb(th.x, th.y, bh.z);
    } else if mode == 11u {
        let bh = rgb_to_hsl(clamp(bot.rgb, vec3(0.0), vec3(1.0)));
        let th = rgb_to_hsl(clamp(top.rgb, vec3(0.0), vec3(1.0)));
        blended = hsl_to_rgb(bh.x, bh.y, th.z);
    } else {
        blended = vec3(
            dag_blend_ch(bot.r, top.r, mode),
            dag_blend_ch(bot.g, top.g, mode),
            dag_blend_ch(bot.b, top.b, mode),
        );
    }
    return vec4(bot.rgb + (blended - bot.rgb) * k, bot.a);
}

fn dag_split_px(p: vec2<i32>) -> vec4<f32> {
    let o = ld(p);
    let space = u.i1.x;
    let ch = u.i1.y;
    var v: f32 = 0.0;
    if space == 1u {
        let y = 0.2126 * o.r + 0.7152 * o.g + 0.0722 * o.b;
        if ch == 0u {
            v = y;
        } else if ch == 1u {
            v = ((o.b - y) / 1.8556) + 0.5;
        } else {
            v = ((o.r - y) / 1.5748) + 0.5;
        }
    } else if space == 2u {
        let hsl = rgb_to_hsl(clamp(o.rgb, vec3(0.0), vec3(1.0)));
        v = select(select(hsl.z, hsl.y, ch == 1u), hsl.x, ch == 0u);
    } else {
        v = select(select(o.b, o.g, ch == 1u), o.r, ch == 0u);
    }
    return vec4(v, v, v, o.a);
}

fn dag_combine_px(p: vec2<i32>) -> vec4<f32> {
    let i0 = ld(p);
    let c0 = i0.r;
    let c1 = textureLoad(aux, p, 0).r;
    let c2 = textureLoad(mask_tex, p, 0).r;
    let space = u.i1.x;
    var rgb = vec3(c0, c1, c2);
    if space == 1u {
        let y = c0;
        let cb = c1 - 0.5;
        let cr = c2 - 0.5;
        let r = max(y + 1.5748 * cr, 0.0);
        let b = max(y + 1.8556 * cb, 0.0);
        let g = max((y - 0.2126 * r - 0.0722 * b) / 0.7152, 0.0);
        rgb = vec3(r, g, b);
    } else if space == 2u {
        rgb = hsl_to_rgb(fract_euclid(c0), clamp(c1, 0.0, 1.0), clamp(c2, 0.0, 1.0));
    }
    return vec4(rgb, i0.a);
}

fn node_key_mix_px(p: vec2<i32>) -> vec4<f32> {
    let graded = ld(p);
    let orig = textureLoad(aux, p, 0);
    let show_mask = u.i1.x;
    let has_mask_tex = u.i1.y != 0u;
    var k = u.p0.x;
    if has_mask_tex {
        k *= textureLoad(mask_tex, p, 0).r;
    }
    k = clamp(k, 0.0, 1.0);
    if show_mask > 0u {
        let a = orig.a;
        if a <= 1e-6 {
            return orig;
        }
        let uv = clamp(enc(orig.rgb / a), vec3(0.0), vec3(1.0));
        return vec4(dec(apply_highlight_px(uv, k, show_mask)) * a, a);
    }
    return orig + (graded - orig) * k;
}

// One output pixel of every op but the running-sum box blur.
fn pixel(op: u32, p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let pc = vec2<f32>(p) + 0.5;
    if op >= OP_BOKEH_DOWN && op <= OP_BOKEH_UP {
        return bokeh(op, p);
    }
    switch op {
        case OP_INVERT_ALPHA: {
            let o = ld(p);
            let a = o.a;
            let na = 1.0 - a;
            // (the CPU's threshold; `na / a` is only used when a > 1e-6)
            var k = 0.0;
            if a > 1e-6 {
                k = na / a;
            }
            let blend = u.p0.x;
            return vec4(o.rgb * k, na * (1.0 - blend) + a * blend);
        }
        case OP_ALPHA_ADJUST: {
            let o = ld(p);
            let c = select(o.rgb / o.a, vec3(0.0), o.a <= 1e-6);
            var a = select(o.a, 1.0, u.i1.x != 0u);
            if u.i1.y != 0u {
                a = 1.0 - a;
            }
            a = clamp(a * u.p0.x, 0.0, 1.0);
            if u.i1.z != 0u {
                return vec4(vec3(srgb_to_linear1(a)), 1.0);
            }
            return vec4(c * a, a);
        }
        case OP_BOX: {
            // one box pass along x (i1.z = 0) or y (1): mean over 2r+1 pixels, clamped to the
            // edge (repeat) or skipping pixels outside, as `effects::box_rows`
            let r = i32(u.i1.x);
            let repeat = u.i1.y != 0u;
            let vertical = u.i1.z != 0u;
            let n = select(s.x, s.y, vertical);
            let x = select(p.x, p.y, vertical);
            var acc = vec4(0.0);
            for (var i = x - r; i <= x + r; i++) {
                if repeat || (i >= 0 && i < n) {
                    let j = clamp(i, 0, n - 1);
                    acc += ld(select(vec2(j, p.y), vec2(p.x, j), vertical));
                }
            }
            return acc * (1.0 / f32(2 * r + 1));
        }
        case OP_DIRECTIONAL: {
            let steps = u.i1.x;
            let dx = u.p0.x; let dy = u.p0.y;
            var acc = vec4(0.0);
            for (var i = 0u; i < steps; i++) {
                let t = f32(i) / f32(steps - 1u) - 0.5;
                acc += sample_clamped(pc.x + dx * t, pc.y + dy * t);
            }
            return acc / f32(steps);
        }
        case OP_UNSHARP: {
            // src: the blurred image, aux: the original. Each colour channel on its own, as
            // `effects::unsharp_boxes`, written as vector math: FXC (DX12 without dxcompiler.dll)
            // aborts on a write to `o[k]` inside a loop ("internal error: compilation aborted
            // unexpectedly"), which took the whole `fx` pipeline down with it.
            let o = textureLoad(aux, p, 0);
            let bq = ld(p);
            let amount = u.p0.x; let th = u.p0.y;
            let d = o.rgb - bq.rgb;
            let sharpened = max(o.rgb + d * amount, vec3(0.0));
            return vec4(select(o.rgb, sharpened, abs(d) >= vec3(th * o.a)), o.a);
        }
        case OP_CROP: {
            let x0 = u.p0.x; let x1 = u.p0.y; let y0 = u.p0.z; let y1 = u.p0.w; let fe = u.p1.x;
            let d = min(min(pc.x - x0, x1 - pc.x), min(pc.y - y0, y1 - pc.y));
            var a: f32;
            if fe > 0.0 {
                a = clamp(d / fe, 0.0, 1.0);
            } else {
                a = clamp(d + 0.5, 0.0, 1.0);
            }
            let o = ld(p);
            return select(o, o * a, a < 1.0);
        }
        case OP_RESAMPLE: {
            // inverse map p0 = (a b c d), p1 = (e f opacity valid); rect i1 = x0 x1 y0 y1
            if u.p1.w == 0.0 || u32(p.x) < u.i1.x || u32(p.x) >= u.i1.y || u32(p.y) < u.i1.z || u32(p.y) >= u.i1.w {
                return vec4(0.0);
            }
            let m0 = u.p0; let m1 = u.p1;
            let uu = m0.x * pc.x + m0.z * pc.y + m1.x;
            let vv = m0.y * pc.x + m0.w * pc.y + m1.y;
            if uu < -1.0 || vv < -1.0 || uu > f32(s.x) + 1.0 || vv > f32(s.y) + 1.0 {
                return vec4(0.0);
            }
            return sample_bilinear(uu, vv) * m1.z;
        }
        case OP_HFLIP: {
            return ld(vec2(s.x - 1 - p.x, p.y));
        }
        case OP_VFLIP: {
            return ld(vec2(p.x, s.y - 1 - p.y));
        }
        case OP_MIRROR: {
            let c = u.p0.xy; let nrm = u.p0.zw;
            let d = (pc.x - c.x) * nrm.x + (pc.y - c.y) * nrm.y;
            if d > 0.0 {
                return sample_bilinear(pc.x - 2.0 * d * nrm.x, pc.y - 2.0 * d * nrm.y);
            }
            return ld(p);
        }
        case OP_OFFSET: {
            let w = f32(s.x); let h = f32(s.y);
            // dx, dy in 0..w / 0..h, so the shifted coordinate is within (-w, w + 0.5)
            var uu = pc.x - u.p0.x;
            var vv = pc.y - u.p0.y;
            if uu < 0.0 { uu += w; }
            if uu >= w { uu -= w; }
            if vv < 0.0 { vv += h; }
            if vv >= h { vv -= h; }
            let q = sample_clamped(uu, vv);
            let o = ld(p);
            return q + (o - q) * u.p0.z;
        }
        case OP_VIGNETTE: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let amt = u.p0.x;
            if abs(amt) < 1e-5 {
                return o;
            }
            let mid = u.p0.y;
            let round = u.p0.z;
            let feather = u.p0.w;
            let tgt = u.p1.xyz;
            let w = f32(s.x);
            let h = f32(s.y);
            let aspect = w / h;
            var nx = pc.x / w * 2.0 - 1.0;
            let ny = pc.y / h * 2.0 - 1.0;
            if round > 0.0 {
                nx *= 1.0 + (aspect - 1.0) * round;
            }
            let p_exp = select(2.0, 2.0 + (-round) * 6.0, round < 0.0);
            let d = powf(powf(abs(nx), p_exp) + powf(abs(ny), p_exp), 1.0 / p_exp) / powf(2.0, 1.0 / p_exp);
            let edge = smoothstep_fx(mid - feather * 0.5, mid + feather * 0.5, d);
            let c = o.rgb / a;
            let ec = enc(c);
            let res = dec(ec + (tgt - ec) * (edge * abs(amt)));
            return vec4(res * a, a);
        }
        case OP_VIDEO_LIMITER: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
            let v = vec3(linear_to_enc_limiter(c.x), linear_to_enc_limiter(c.y), linear_to_enc_limiter(c.z));
            let max_val = u.p0.x;
            let comp = u.p0.y;
            let axis = u.i1.x;
            let warn = u.i1.y != 0u;
            let wc = u.p0.zw;
            let wc_b = u.p1.x;
            let warning_col = vec3(wc.x, wc.y, wc_b);
            let out_enc = limit_video(v, max_val, comp, axis);
            let diff = abs(v - out_enc);
            if warn && (diff.x > 1e-4 || diff.y > 1e-4 || diff.z > 1e-4) {
                return vec4(warning_col * a, a);
            }
            let res = vec3(enc_to_linear_limiter(out_enc.x), enc_to_linear_limiter(out_enc.y), enc_to_linear_limiter(out_enc.z));
            return vec4(res * a, a);
        }
        case OP_LUMETRI: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
            let res = lumetri_op(c, p);
            return vec4(res * a, a);
        }
        case OP_COLOR_WHEELS: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
            let res = color_wheels_op(c);
            return vec4(res * a, a);
        }
        case OP_LOG_WHEELS: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let res = log_wheels_op(o.rgb / a);
            return vec4(res * a, a);
        }
        case OP_HDR_WHEELS: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let res = hdr_wheels_op(o.rgb / a);
            return vec4(res * a, a);
        }
        case OP_RGB_MIXER: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let res = rgb_mixer_op(o.rgb / a);
            return vec4(res * a, a);
        }
        case OP_CST: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let res = cst_op(o.rgb / a);
            return vec4(res * a, a);
        }
        case OP_NODE_SIZING: {
            return node_sizing_px(p);
        }
        case OP_LUMETRI_LOOK: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
            let res = lumetri_look_op(c);
            return vec4(res * a, a);
        }
        case OP_LUT_3D: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
            let res = lut_3d_op(c);
            return vec4(res * a, a);
        }
        case OP_LUMETRI_CURVES: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
            let res = lumetri_curves_op(c);
            return vec4(res * a, a);
        }
        case OP_HSL_KEY: {
            return hsl_key_px(p);
        }
        case OP_HSL_MEDIAN: {
            return hsl_median_px(p);
        }
        case OP_HSL_APPLY: {
            return hsl_apply_px(p);
        }
        case OP_MASK_MIX: {
            return mask_mix_px(p);
        }
        case OP_MIDTONE_DETAIL: {
            let o = textureLoad(aux, p, 0);
            if o.a <= 1e-6 {
                return o;
            }
            let bq = ld(p);
            let c = enc(o.rgb / o.a);
            let l = clamp(luma709(c), 0.0, 1.0);
            let gain = u.p0.x * 0.8 * 4.0 * l * (1.0 - l);
            let d = o.rgb - bq.rgb;
            return vec4(max(o.rgb + d * gain, vec3(0.0)), o.a);
        }
        case OP_COLOR_SLICE: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let res = color_slice_op(o.rgb / a);
            return vec4(res * a, a);
        }
        case OP_COLOR_WARPER: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let res = color_warper_op(o.rgb / a);
            return vec4(res * a, a);
        }
        case OP_HSL_MORPH: {
            return hsl_morph_px(p);
        }
        case OP_NODE_KEY_FINISH: {
            return node_key_finish_px(p);
        }
        case OP_RESTORE_ORIG: {
            return textureLoad(aux, p, 0);
        }
        case OP_NODE_KEY_MIX: {
            return node_key_mix_px(p);
        }
        case OP_DAG_KEY_COMBINE: {
            return dag_key_combine_px(p);
        }
        case OP_DAG_PARALLEL: {
            return dag_parallel_px(p);
        }
        case OP_DAG_LAYER: {
            return dag_layer_px(p);
        }
        case OP_DAG_SPLIT: {
            return dag_split_px(p);
        }
        case OP_DAG_COMBINE: {
            return dag_combine_px(p);
        }
        default: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            return vec4(color_op(op, o.rgb / a) * a, a);
        }
    }
}

@compute @workgroup_size(16, 16)
fn fx_px(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u.i0.y || id.y >= u.i0.z {
        return;
    }
    let p = vec2<i32>(id.xy);
    textureStore(dst, p, pixel(u.i0.x, p));
}

// A box pass as a running sum along one row (i1.z = 0) or column (1) per invocation: O(1) per
// pixel for any radius, the arithmetic of `effects::box_rows`.
@compute @workgroup_size(64)
fn fx_run(@builtin(global_invocation_id) id: vec3<u32>) {
    let s = size();
    let vertical = u.i1.z != 0u;
    let n = select(s.x, s.y, vertical);
    let lines = select(s.y, s.x, vertical);
    let line = i32(id.x);
    if line >= lines {
        return;
    }
    let r = i32(u.i1.x);
    let repeat = u.i1.y != 0u;
    let inv = 1.0 / f32(2 * r + 1);
    var acc = vec4(0.0);
    for (var i = -r; i <= r; i++) {
        if repeat || (i >= 0 && i < n) {
            let j = clamp(i, 0, n - 1);
            acc += ld(select(vec2(j, line), vec2(line, j), vertical));
        }
    }
    for (var x = 0; x < n; x++) {
        textureStore(dst, select(vec2(x, line), vec2(line, x), vertical), acc * inv);
        let out_i = x - r;
        let in_i = x + r + 1;
        if repeat || out_i >= 0 {
            let j = clamp(out_i, 0, n - 1);
            acc -= ld(select(vec2(j, line), vec2(line, j), vertical));
        }
        if repeat || in_i < n {
            let j = clamp(in_i, 0, n - 1);
            acc += ld(select(vec2(j, line), vec2(line, j), vertical));
        }
    }
}
