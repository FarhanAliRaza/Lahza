//! CPU rasteriser. It mirrors `motion.wgsl` function for function, so the
//! fallback draws the same picture as the GPU. Tiles rows run in parallel.

use crate::font::{ATLAS_SIZE, SPREAD};
use crate::list::*;

pub struct Sources<'a> {
    pub atlas: &'a [u8],
    pub images: &'a [u32],
    pub groups: &'a [[f32; 4]],
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

fn shape_distance(prim: &Prim, l: V2) -> f32 {
    let g = prim.geom;
    match prim.kind[1] {
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
    if source == 1 {
        src.groups[i]
    } else {
        unpack(src.images[i])
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

/// Premultiplied colour of one primitive at target pixel centre `p`.
pub fn shade(prim: &Prim, p: V2, stops: &[[f32; 4]], src: &Sources) -> [f32; 4] {
    let l = [
        prim.inv_x[0] * p[0] + prim.inv_x[1] * p[1] + prim.inv_x[2],
        prim.inv_y[0] * p[0] + prim.inv_y[1] * p[1] + prim.inv_y[2],
    ];
    let aa = prim.inv_x[3];
    let opacity = prim.inv_y[3];
    let mut out = [0.0f32; 4];
    match prim.kind[0] {
        PRIM_SHAPE => {
            let d = shape_distance(prim, l);
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
            out = sample_image(src, prim, l).map(|c| c * cov);
        }
    }
    out.map(|c| c * opacity)
}

pub fn blend(d: [f32; 4], s: [f32; 4], mode: u32) -> [f32; 4] {
    match mode {
        1 => std::array::from_fn(|i| (s[i] + d[i]).min(1.0)),
        2 => std::array::from_fn(|i| s[i] * d[i] + s[i] * (1.0 - d[3]) + d[i] * (1.0 - s[3])),
        3 => std::array::from_fn(|i| s[i] + d[i] - s[i] * d[i]),
        _ => std::array::from_fn(|i| s[i] + d[i] * (1.0 - s[3])),
    }
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
                    let s = shade(prim, p, &list.stops, src);
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
pub fn blur(buf: &mut [[f32; 4]], offset: usize, w: usize, h: usize, sigma: f32, threads: usize) {
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
                                let (sx, sy) = if horizontal { (x as i32 + d, y as i32) } else { (x as i32, y as i32 + d) };
                                if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
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
