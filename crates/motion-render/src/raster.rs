//! CPU rasteriser. It mirrors `motion.wgsl` function for function, so the
//! fallback draws the same picture as the GPU. Tiles rows run in parallel.

use crate::font::{ATLAS_SIZE, SPREAD};
use crate::list::*;

pub struct Sources<'a> {
    pub atlas: &'a [u8],
    pub images: &'a [u32],
    pub groups: &'a [[f32; 4]],
    pub footage: &'a [u32],
    pub ops: &'a [[f32; 4]],
}

type V2 = [f32; 2];

fn len(v: V2) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn sd_ellipse(p: V2, r: V2) -> f32 {
    if (r[0] - r[1]).abs() < 1e-4 {
        return len(p) - r[0];
    }
    let r = [r[0].max(1e-4), r[1].max(1e-4)];
    let k0 = len([p[0] / r[0], p[1] / r[1]]);
    let k1 = len([p[0] / (r[0] * r[0]), p[1] / (r[1] * r[1])]);
    if k1 < 1e-9 {
        return -r[0].min(r[1]);
    }
    k0 * (k0 - 1.0) / k1
}

fn sd_rect(p: V2, b: V2, r: f32) -> f32 {
    let q = [p[0].abs() - b[0] + r, p[1].abs() - b[1] + r];
    len([q[0].max(0.0), q[1].max(0.0)]) + q[0].max(q[1]).min(0.0) - r
}

fn sd_capsule(p: V2, b: V2) -> f32 {
    let r = b[0].min(b[1]);
    if b[0] >= b[1] {
        len([(p[0].abs() - (b[0] - r)).max(0.0), p[1]]) - r
    } else {
        len([p[0], (p[1].abs() - (b[1] - r)).max(0.0)]) - r
    }
}

fn sd_arc(p: V2, g: [f32; 4], full: bool) -> f32 {
    let (rc, ht, mid, half) = (g[0], g[1], g[2], g[3]);
    if full {
        return (len(p) - rc).abs() - ht;
    }
    let (s, c) = mid.sin_cos();
    let x = p[0] * c + p[1] * s;
    let y = p[1] * c - p[0] * s;
    let q = [x.abs(), -y];
    let sc = [half.sin(), half.cos()];
    let d = if sc[1] * q[0] > sc[0] * q[1] {
        len([q[0] - sc[0] * rc, q[1] - sc[1] * rc])
    } else {
        (len(q) - rc).abs()
    };
    d - ht
}

fn sd_line(p: V2, g: [f32; 4]) -> f32 {
    let dx = (g[0] - p[0]).max(p[0] - g[1]).max(0.0);
    len([dx, p[1]]) - g[2]
}

fn sd_star(p: V2, g: [f32; 4]) -> f32 {
    let (radius, k, n, round) = (g[0], g[1], g[2].max(3.0), g[3]);
    let r = (radius - round).max(1e-4);
    let an = std::f32::consts::PI / n;
    let theta = p[0].atan2(-p[1]);
    let mut b = theta + an;
    b -= 2.0 * an * (b / (2.0 * an)).floor();
    b = (b - an).abs();
    let l = len(p);
    let q = [l * b.cos(), l * b.sin()];
    let a = [r, 0.0];
    let bb = [k * r * an.cos(), k * r * an.sin()];
    let e = [bb[0] - a[0], bb[1] - a[1]];
    let w = [q[0] - a[0], q[1] - a[1]];
    let t = clamp01((w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]).max(1e-9));
    let dist = len([w[0] - e[0] * t, w[1] - e[1] * t]);
    let cross = e[0] * w[1] - e[1] * w[0];
    (if cross > 0.0 { -dist } else { dist }) - round
}

/// Distance to a flattened path; negative inside (non-zero winding) for
/// filled paths.
fn sd_path(prim: &Prim, l: V2, segments: &[[f32; 4]]) -> f32 {
    let first = prim.geom2[0].to_bits() as usize;
    let count = prim.geom2[1].to_bits() as usize;
    let open = prim.geom2[2] > 0.5;
    let mut best = f32::MAX;
    let mut winding = 0i32;
    for s in &segments[first..first + count] {
        let e = [s[2] - s[0], s[3] - s[1]];
        let w = [l[0] - s[0], l[1] - s[1]];
        let t = clamp01((w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]).max(1e-12));
        let dx = w[0] - e[0] * t;
        let dy = w[1] - e[1] * t;
        best = best.min(dx * dx + dy * dy);
        if !open && (s[1] <= l[1]) != (s[3] <= l[1]) {
            let x = s[0] + (l[1] - s[1]) / (s[3] - s[1]) * e[0];
            if x > l[0] {
                winding += if s[3] > s[1] { 1 } else { -1 };
            }
        }
    }
    let d = best.sqrt();
    if winding != 0 {
        -d
    } else {
        d
    }
}

fn shape_distance(prim: &Prim, l: V2, segments: &[[f32; 4]]) -> f32 {
    let g = prim.geom;
    match prim.kind[1] {
        SHAPE_PATH => sd_path(prim, l, segments),
        SHAPE_ELLIPSE => sd_ellipse(l, [g[0], g[1]]),
        SHAPE_RECT => sd_rect(l, [g[0], g[1]], g[2]),
        SHAPE_CAPSULE => sd_capsule(l, [g[0], g[1]]),
        SHAPE_ARC => sd_arc(l, g, prim.geom2[0] > 0.5),
        SHAPE_LINE => sd_line(l, g),
        _ => sd_star(l, g),
    }
}

fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn gradient(prim: &Prim, l: V2, stops: &[[f32; 4]]) -> [f32; 4] {
    let [fx, fy, tx, ty] = prim.paint2;
    let d = [tx - fx, ty - fy];
    let dd = (d[0] * d[0] + d[1] * d[1]).max(1e-9);
    let t = if prim.paint[1] < 1.5 {
        ((l[0] - fx) * d[0] + (l[1] - fy) * d[1]) / dd
    } else {
        len([l[0] - fx, l[1] - fy]) / dd.sqrt()
    };
    let base = prim.paint[2] as usize * 2;
    let count = prim.paint[3] as usize;
    let stop = |i: usize| (stops[base + i * 2][0], stops[base + i * 2 + 1]);
    let (o0, c0) = stop(0);
    if t <= o0 || count == 1 {
        return c0;
    }
    for i in 0..count - 1 {
        let (oa, ca) = stop(i);
        let (ob, cb) = stop(i + 1);
        if t <= ob {
            return mix4(ca, cb, clamp01((t - oa) / (ob - oa).max(1e-6)));
        }
    }
    stop(count - 1).1
}

fn atlas_texel(atlas: &[u8], x: i32, y: i32) -> f32 {
    atlas[(y as u32 * ATLAS_SIZE + x as u32) as usize] as f32 / 255.0
}

fn sample_atlas(atlas: &[u8], l: V2, r: [f32; 4]) -> f32 {
    let s = [l[0] - 0.5, l[1] - 0.5];
    let x0 = s[0].floor();
    let y0 = s[1].floor();
    let (fx, fy) = (s[0] - x0, s[1] - y0);
    let cx = |x: f32| (x as i32).clamp(r[0] as i32, r[2] as i32 - 1);
    let cy = |y: f32| (y as i32).clamp(r[1] as i32, r[3] as i32 - 1);
    let a = atlas_texel(atlas, cx(x0), cy(y0));
    let b = atlas_texel(atlas, cx(x0 + 1.0), cy(y0));
    let c = atlas_texel(atlas, cx(x0), cy(y0 + 1.0));
    let d = atlas_texel(atlas, cx(x0 + 1.0), cy(y0 + 1.0));
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    top + (bottom - top) * fy
}

fn unpack(v: u32) -> [f32; 4] {
    [(v & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, ((v >> 16) & 255) as f32 / 255.0, ((v >> 24) & 255) as f32 / 255.0]
}

fn image_texel(src: &Sources, source: u32, offset: u32, w: i32, h: i32, x: i32, y: i32) -> [f32; 4] {
    let i = (offset as i32 + y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize;
    match source {
        1 => src.groups[i],
        2 => unpack(src.footage[i]),
        _ => unpack(src.images[i]),
    }
}

fn sample_image(src: &Sources, prim: &Prim, l: V2) -> [f32; 4] {
    let source = prim.kind[1];
    let mut offset = prim.geom2[0].to_bits();
    let (mut w, mut h) = (prim.geom[0] as i32, prim.geom[1] as i32);
    let levels = prim.geom[2] as i32;
    let aa = prim.inv_x[3];
    let lod = (aa.max(1.0).log2().floor() as i32).clamp(0, levels - 1);
    for _ in 0..lod {
        offset += (w * h) as u32;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    let k = (1u32 << lod) as f32;
    let s = [l[0] / k - 0.5, l[1] / k - 0.5];
    let x0 = s[0].floor();
    let y0 = s[1].floor();
    let (fx, fy) = (s[0] - x0, s[1] - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let a = image_texel(src, source, offset, w, h, x0, y0);
    let b = image_texel(src, source, offset, w, h, x0 + 1, y0);
    let c = image_texel(src, source, offset, w, h, x0, y0 + 1);
    let d = image_texel(src, source, offset, w, h, x0 + 1, y0 + 1);
    mix4(mix4(a, b, fx), mix4(c, d, fx), fy)
}

fn luma(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Premultiplied colour of one primitive at target pixel centre `p`.
pub fn shade(prim: &Prim, p: V2, stops: &[[f32; 4]], segments: &[[f32; 4]], src: &Sources) -> [f32; 4] {
    let l = [
        prim.inv_x[0] * p[0] + prim.inv_x[1] * p[1] + prim.inv_x[2],
        prim.inv_y[0] * p[0] + prim.inv_y[1] * p[1] + prim.inv_y[2],
    ];
    let aa = prim.inv_x[3];
    let opacity = prim.inv_y[3];
    let mut out = [0.0f32; 4];
    match prim.kind[0] {
        PRIM_SHAPE => {
            let d = shape_distance(prim, l, segments);
            if prim.kind[3] & FLAG_FILL != 0 {
                let cov = clamp01(0.5 - d / aa);
                if cov > 0.0 {
                    let c = if prim.paint[1] > 0.5 { gradient(prim, l, stops) } else { prim.fill };
                    out = c.map(|v| v * cov);
                }
            }
            if prim.kind[3] & FLAG_STROKE != 0 {
                let cov = clamp01(0.5 - (d.abs() - prim.paint[0] * 0.5) / aa);
                if cov > 0.0 {
                    let s = prim.stroke.map(|v| v * cov);
                    out = std::array::from_fn(|i| s[i] + out[i] * (1.0 - s[3]));
                }
            }
        }
        PRIM_GLYPH => {
            let r = prim.geom;
            if l[0] < r[0] || l[1] < r[1] || l[0] > r[2] || l[1] > r[3] {
                return out;
            }
            let v = sample_atlas(src.atlas, l, r);
            let d = (0.5 - v) * 2.0 * SPREAD;
            let cov = clamp01(0.5 - d / aa);
            out = prim.fill.map(|c| c * cov);
        }
        _ => {
            let (w, h) = (prim.geom[0], prim.geom[1]);
            let edge = (-l[0]).max(l[0] - w).max(-l[1]).max(l[1] - h);
            let cov = clamp01(0.5 - edge / aa.max(1e-6));
            if cov <= 0.0 {
                return out;
            }
            let mut c = if prim.paint[3].to_bits() > 0 { sample_post(src, prim, l, src.ops) } else { sample_image(src, prim, l) };
            if prim.kind[0] == PRIM_MATTE {
                let mut matte = *prim;
                matte.geom2[0] = prim.geom2[1];
                let m = sample_image(src, &matte, l);
                let k = match prim.geom2[2] as u32 {
                    0 => m[3],
                    1 => 1.0 - m[3],
                    2 => luma(m),
                    _ => 1.0 - luma(m),
                };
                c = c.map(|v| v * clamp01(k));
            } else if prim.kind[3] & FLAG_TINT_ALPHA != 0 {
                c = prim.fill.map(|f| f * c[3]);
            } else if prim.kind[3] & FLAG_TINT_MUL != 0 {
                c = std::array::from_fn(|i| c[i] * prim.fill[i]);
            }
            out = c.map(|v| v * cov);
        }
    }
    out.map(|c| c * opacity)
}

type V3 = [f32; 3];

fn lum3(c: V3) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: V3) -> V3 {
    let l = lum3(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut c = c;
    if n < 0.0 {
        c = c.map(|v| l + (v - l) * l / (l - n).max(1e-6));
    }
    if x > 1.0 {
        c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l).max(1e-6));
    }
    c
}

fn set_lum(c: V3, l: f32) -> V3 {
    let d = l - lum3(c);
    clip_color(c.map(|v| v + d))
}

fn sat3(c: V3) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: V3, s: f32) -> V3 {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx > mn {
        c.map(|v| (v - mn) * s / (mx - mn))
    } else {
        [0.0; 3]
    }
}

fn soft_light_d(b: f32) -> f32 {
    if b <= 0.25 {
        ((16.0 * b - 12.0) * b + 4.0) * b
    } else {
        b.sqrt()
    }
}

/// One channel of a separable blend mode B(backdrop, source).
fn blend_channel(b: f32, s: f32, mode: u32) -> f32 {
    match mode {
        2 => b * s,
        3 => b + s - b * s,
        4 => blend_channel(s, b, 9),
        5 => b.min(s),
        6 => b.max(s),
        7 => {
            if b <= 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }
        8 => {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }
        9 => {
            if s <= 0.5 {
                b * 2.0 * s
            } else {
                let t = 2.0 * s - 1.0;
                b + t - b * t
            }
        }
        10 => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                b + (2.0 * s - 1.0) * (soft_light_d(b) - b)
            }
        }
        11 => (b - s).abs(),
        12 => b + s - 2.0 * b * s,
        13 => (b - s).max(0.0),
        14 => {
            if s <= 0.0 {
                1.0
            } else {
                (b / s).min(1.0)
            }
        }
        _ => s,
    }
}

/// Composites premultiplied `s` over `d` with blend `mode` (W3C
/// compositing; codes follow `Blend`).
pub fn blend(d: [f32; 4], s: [f32; 4], mode: u32) -> [f32; 4] {
    match mode {
        0 => std::array::from_fn(|i| s[i] + d[i] * (1.0 - s[3])),
        1 => std::array::from_fn(|i| (s[i] + d[i]).min(1.0)),
        _ => {
            let (sa, da) = (s[3], d[3]);
            let cs: V3 = std::array::from_fn(|i| if sa > 0.0 { (s[i] / sa).clamp(0.0, 1.0) } else { 0.0 });
            let cb: V3 = std::array::from_fn(|i| if da > 0.0 { (d[i] / da).clamp(0.0, 1.0) } else { 0.0 });
            let mixed: V3 = match mode {
                15 => set_lum(set_sat(cs, sat3(cb)), lum3(cb)),
                16 => set_lum(set_sat(cb, sat3(cs)), lum3(cb)),
                17 => set_lum(cs, lum3(cb)),
                18 => set_lum(cb, lum3(cs)),
                m => std::array::from_fn(|i| blend_channel(cb[i], cs[i], m)),
            };
            let mut out = [0.0; 4];
            for i in 0..3 {
                out[i] = s[i] * (1.0 - da) + d[i] * (1.0 - sa) + sa * da * mixed[i];
            }
            out[3] = sa + da - sa * da;
            out
        }
    }
}

// ------------------------------------------------------------ post program

pub const OP_EXPOSURE: f32 = 1.0;
pub const OP_BRIGHTNESS_CONTRAST: f32 = 2.0;
pub const OP_LEVELS: f32 = 3.0;
pub const OP_HUE_SATURATION: f32 = 4.0;
pub const OP_TINT: f32 = 5.0;
pub const OP_GRADIENT_MAP: f32 = 6.0;
pub const OP_INVERT: f32 = 7.0;
pub const OP_LUT: f32 = 8.0;
pub const OP_FILL: f32 = 9.0;
pub const OP_DISPLACE: f32 = 20.0;
pub const OP_DIRECTIONAL_BLUR: f32 = 21.0;
pub const OP_RGB_SPLIT: f32 = 22.0;
pub const OP_VIGNETTE: f32 = 30.0;
pub const OP_GRAIN: f32 = 31.0;

/// Integer hash → [0, 1). Same bits on CPU and GPU.
pub fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393).wrapping_add((y as u32).wrapping_mul(668_265_263)).wrapping_add(seed.wrapping_mul(1_442_695_041));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (ix, iy) = (ix as i32, iy as i32);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash(ix, iy, seed);
    let b = hash(ix + 1, iy, seed);
    let c = hash(ix, iy + 1, seed);
    let d = hash(ix + 1, iy + 1, seed);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    (top + (bottom - top) * sy) * 2.0 - 1.0
}

fn luma3(c: V3) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn apply_color(op: &[[f32; 4]], c: V3, l: V2, size: V2) -> V3 {
    let h = op[0];
    let mix3 = |a: V3, b: V3, t: f32| -> V3 { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
    match h[0] {
        OP_EXPOSURE => c.map(|v| v * h[2].exp2()),
        OP_BRIGHTNESS_CONTRAST => c.map(|v| (v - 0.5) * (1.0 + h[3]) + 0.5 + h[2]),
        OP_LEVELS => {
            let (ib, iw, g, ob, ow) = (h[2], h[3], op[1][0], op[1][1], op[1][2]);
            c.map(|v| {
                let x = clamp01((v - ib) / (iw - ib).max(1e-5));
                ob + x.powf(1.0 / g.max(1e-3)) * (ow - ob)
            })
        }
        OP_HUE_SATURATION => {
            let (hue, sat, light) = (h[2], h[3], op[1][0]);
            let (s, k) = hue.to_radians().sin_cos();
            let m = [
                [0.213 + k * 0.787 - s * 0.213, 0.715 - k * 0.715 - s * 0.715, 0.072 - k * 0.072 + s * 0.928],
                [0.213 - k * 0.213 + s * 0.143, 0.715 + k * 0.285 + s * 0.140, 0.072 - k * 0.072 - s * 0.283],
                [0.213 - k * 0.213 - s * 0.787, 0.715 - k * 0.715 + s * 0.715, 0.072 + k * 0.928 + s * 0.072],
            ];
            let r: V3 = std::array::from_fn(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]);
            let y = luma3(r);
            let r = r.map(|v| y + (v - y) * (1.0 + sat));
            if light >= 0.0 {
                r.map(|v| v + (1.0 - v) * light)
            } else {
                r.map(|v| v * (1.0 + light))
            }
        }
        OP_TINT => {
            let (b, w) = (op[1], op[2]);
            let y = clamp01(luma3(c));
            mix3(c, std::array::from_fn(|i| b[i] + (w[i] - b[i]) * y), h[2])
        }
        OP_GRADIENT_MAP => {
            let n = h[3] as usize;
            let y = clamp01(luma3(c));
            let stop = |i: usize| (op[1 + i * 2][0], [op[2 + i * 2][0], op[2 + i * 2][1], op[2 + i * 2][2]]);
            let mut g = stop(0).1;
            if y > stop(0).0 {
                g = stop(n - 1).1;
                for i in 0..n.saturating_sub(1) {
                    let (oa, ca) = stop(i);
                    let (ob, cb) = stop(i + 1);
                    if y <= ob {
                        g = mix3(ca, cb, clamp01((y - oa) / (ob - oa).max(1e-6)));
                        break;
                    }
                }
            }
            mix3(c, g, h[2])
        }
        OP_INVERT => mix3(c, c.map(|v| 1.0 - v), h[2]),
        OP_FILL => mix3(c, [op[1][0], op[1][1], op[1][2]], h[2]),
        OP_LUT => {
            let n = h[3] as usize;
            if n < 2 {
                return c;
            }
            let k = (n - 1) as f32;
            let p = c.map(|v| clamp01(v) * k);
            let i0 = p.map(|v| (v.floor() as usize).min(n - 2));
            let f: V3 = std::array::from_fn(|i| p[i] - i0[i] as f32);
            let at = |r: usize, g: usize, b: usize| -> V3 {
                let e = op[1 + r + g * n + b * n * n];
                [e[0], e[1], e[2]]
            };
            let mut out = [0.0; 3];
            for (dr, wr) in [(0, 1.0 - f[0]), (1, f[0])] {
                for (dg, wg) in [(0, 1.0 - f[1]), (1, f[1])] {
                    for (db, wb) in [(0, 1.0 - f[2]), (1, f[2])] {
                        let e = at(i0[0] + dr, i0[1] + dg, i0[2] + db);
                        for i in 0..3 {
                            out[i] += e[i] * wr * wg * wb;
                        }
                    }
                }
            }
            mix3(c, out, h[2])
        }
        OP_VIGNETTE => {
            let q = [l[0] / size[0].max(1e-6) * 2.0 - 1.0, l[1] / size[1].max(1e-6) * 2.0 - 1.0];
            let r = len(q) / std::f32::consts::SQRT_2;
            let inner = 1.0 - h[3];
            let t = clamp01((r - inner) / (1.0 - inner).max(1e-4));
            let v = t * t * (3.0 - 2.0 * t) * h[2];
            c.map(|x| x * (1.0 - v))
        }
        OP_GRAIN => {
            let size = h[3].max(1e-3);
            let n = hash((l[0] / size).floor() as i32, (l[1] / size).floor() as i32, op[1][0] as u32) - 0.5;
            c.map(|v| v + n * h[2])
        }
        _ => c,
    }
}

/// Samples a group texture through its post program (displacement,
/// directional blur, RGB split, then colour ops, vignette and grain).
fn sample_post(src: &Sources, prim: &Prim, l: V2, ops: &[[f32; 4]]) -> [f32; 4] {
    let start = prim.paint[2].to_bits() as usize;
    let end = start + prim.paint[3].to_bits() as usize;
    let size = [prim.geom[0], prim.geom[1]];
    let mut uv = l;
    let mut blur: Option<V2> = None;
    let mut split: Option<V2> = None;
    let mut i = start;
    while i < end {
        let h = ops[i];
        match h[0] {
            OP_DISPLACE => {
                let (amount, scale, time, seed) = (h[2], h[3].max(1e-3), ops[i + 1][0], ops[i + 1][1] as u32);
                let p = [uv[0] / scale + time, uv[1] / scale + time * 0.7];
                uv = [uv[0] + amount * value_noise(p[0], p[1], seed), uv[1] + amount * value_noise(p[0], p[1], seed ^ 0x9e37_79b9)];
            }
            OP_DIRECTIONAL_BLUR => blur = Some([h[2], h[3]]),
            OP_RGB_SPLIT => split = Some([h[2], h[3]]),
            _ => {}
        }
        i += h[1] as usize;
    }
    let base = |p: V2| sample_image(src, prim, p);
    let tap = |p: V2| -> [f32; 4] {
        match blur {
            Some(d) => {
                let mut acc = [0.0f32; 4];
                const N: usize = 9;
                for k in 0..N {
                    let t = k as f32 / (N - 1) as f32 - 0.5;
                    let c = base([p[0] + d[0] * t, p[1] + d[1] * t]);
                    for j in 0..4 {
                        acc[j] += c[j] / N as f32;
                    }
                }
                acc
            }
            None => base(p),
        }
    };
    let mut c = tap(uv);
    if let Some(d) = split {
        let r = tap([uv[0] + d[0], uv[1] + d[1]]);
        let b = tap([uv[0] - d[0], uv[1] - d[1]]);
        let a = c[3].max(r[3]).max(b[3]);
        c = [r[0], c[1], b[2], a];
    }
    let a = c[3];
    if a <= 0.0 {
        return c;
    }
    let mut rgb: V3 = [c[0] / a, c[1] / a, c[2] / a];
    let mut i = start;
    while i < end {
        let h = ops[i];
        let n = h[1] as usize;
        if h[0] < 20.0 || h[0] >= 30.0 {
            rgb = apply_color(&ops[i..i + n], rgb, l, size);
        }
        i += n;
    }
    let rgb = rgb.map(clamp01);
    [rgb[0] * a, rgb[1] * a, rgb[2] * a, a]
}

#[derive(Clone, Copy)]
pub enum Write {
    Replace,
    Accumulate(f32),
}

/// Rasterises one target of `list` into `out` (width × height, premultiplied).
pub fn raster(list: &DisplayList, target: &Target, src: &Sources, out: &mut [[f32; 4]], write: Write, threads: usize) {
    let w = target.width as usize;
    let rows_per_band = TILE as usize * w;
    let bands: Vec<(usize, &mut [[f32; 4]])> = out.chunks_mut(rows_per_band).enumerate().collect();
    let threads = threads.max(1);
    let per_thread = bands.len().div_ceil(threads).max(1);
    let mut bands = bands;
    std::thread::scope(|scope| {
        while !bands.is_empty() {
            let chunk: Vec<_> = bands.drain(..per_thread.min(bands.len())).collect();
            scope.spawn(move || {
                for (ty, band) in chunk {
                    raster_band(list, target, src, ty as u32, band, write);
                }
            });
        }
    });
}

fn raster_band(list: &DisplayList, target: &Target, src: &Sources, ty: u32, band: &mut [[f32; 4]], write: Write) {
    let w = target.width;
    let rows = (band.len() as u32) / w;
    for tx in 0..target.tiles_x {
        let tile = target.range_base + ty * target.tiles_x + tx;
        let start = list.ranges[tile as usize * 2] as usize;
        let count = list.ranges[tile as usize * 2 + 1] as usize;
        let indices = &list.indices[start..start + count];
        for y in 0..rows {
            for x in tx * TILE..((tx + 1) * TILE).min(w) {
                let p = [x as f32 + 0.5, (ty * TILE + y) as f32 + 0.5];
                let mut c = target.background;
                for &i in indices {
                    let prim = &list.prims[i as usize];
                    let s = shade(prim, p, &list.stops, &list.segments, src);
                    if s[3] > 0.0 {
                        c = blend(c, s, prim.kind[2]);
                    }
                }
                let o = &mut band[(y * w + x) as usize];
                match write {
                    Write::Replace => *o = c,
                    Write::Accumulate(k) => {
                        for i in 0..4 {
                            o[i] += c[i] * k;
                        }
                    }
                }
            }
        }
    }
}

/// Separable Gaussian blur of one group in place (zero outside).
pub fn blur(buf: &mut [[f32; 4]], offset: usize, w: usize, h: usize, sigma: f32, clamp: bool, threads: usize) {
    if sigma <= 0.0 {
        return;
    }
    let radius = (sigma * 3.0).ceil() as i32;
    let weights: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let norm: f32 = weights.iter().sum();
    let inv = 1.0 / norm;
    let weights: Vec<f32> = weights.iter().map(|v| v * inv).collect();
    let region = &mut buf[offset..offset + w * h];
    let mut tmp = vec![[0.0f32; 4]; w * h];
    let pass = |src: &[[f32; 4]], dst: &mut [[f32; 4]], horizontal: bool| {
        let rows: Vec<(usize, &mut [[f32; 4]])> = dst.chunks_mut(w).enumerate().collect();
        let per = rows.len().div_ceil(threads.max(1)).max(1);
        let mut rows = rows;
        std::thread::scope(|scope| {
            while !rows.is_empty() {
                let chunk: Vec<_> = rows.drain(..per.min(rows.len())).collect();
                let weights = &weights;
                scope.spawn(move || {
                    for (y, row) in chunk {
                        for (x, o) in row.iter_mut().enumerate() {
                            let mut acc = [0.0f32; 4];
                            for (k, wt) in weights.iter().enumerate() {
                                let d = k as i32 - radius;
                                let (mut sx, mut sy) = if horizontal { (x as i32 + d, y as i32) } else { (x as i32, y as i32 + d) };
                                if clamp {
                                    sx = sx.clamp(0, w as i32 - 1);
                                    sy = sy.clamp(0, h as i32 - 1);
                                } else if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
                                    continue;
                                }
                                let s = src[sy as usize * w + sx as usize];
                                for i in 0..4 {
                                    acc[i] += s[i] * wt;
                                }
                            }
                            *o = acc;
                        }
                    }
                });
            }
        });
    };
    pass(region, &mut tmp, true);
    pass(&tmp, region, false);
}

/// Premultiplied float → straight RGBA8.
pub fn resolve(src: &[[f32; 4]], out: &mut [u8]) {
    for (c, o) in src.iter().zip(out.chunks_exact_mut(4)) {
        let a = c[3].clamp(0.0, 1.0);
        let k = if a > 0.0 { 1.0 / a } else { 0.0 };
        o[0] = ((c[0] * k).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        o[1] = ((c[1] * k).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        o[2] = ((c[2] * k).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        o[3] = (a * 255.0 + 0.5) as u8;
    }
}
