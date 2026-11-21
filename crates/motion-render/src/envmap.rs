//! Image-based lighting: an equirectangular environment image, prefiltered
//! once per image.
//!
//! - The raster gets a short stack of levels blurred for roughness (the
//!   same blur the preset lobes use: exp((cos − 1) / (1.2 r²))) and a
//!   diffuse level (the cosine convolution, as irradiance / π).
//! - The path tracer gets the sharp map and a table for sampling
//!   directions by brightness (a marginal CDF over rows and a conditional
//!   CDF per row, weighted by each texel's solid angle).
//!
//! Directions are world space (y down). A direction's place on the image:
//! u = ½ + atan2(x, z) / 2π (the image centre is +z, away from the
//! viewer), v = acos(−y) / π (top row is straight up); the environment's
//! rotation turns it about the vertical axis.

use std::f64::consts::PI;
use std::path::Path;

/// Raster levels: (width, height, roughness); the first is sharp.
const LEVELS: [(usize, usize, f64); 6] = [(256, 128, 0.0), (64, 32, 0.2), (32, 16, 0.4), (32, 16, 0.6), (32, 16, 0.8), (32, 16, 1.0)];
const DIFFUSE: (usize, usize) = (32, 16);
/// The traced map's largest size and the sampling table's.
const TRACE_MAX: (usize, usize) = (1024, 512);
const SAMPLING: (usize, usize) = (256, 128);

pub struct EnvMap {
    /// [levels, diffuse offset, diffuse w, diffuse h], per level
    /// [offset, w, h, roughness], then the texels (linear RGB).
    /// Offsets count from the first row.
    pub raster: Vec<[f32; 4]>,
    /// [w, h, sampling w, sampling h], the sharp map (w × h), the marginal
    /// CDF (one row per sampling row: [cdf, 0, 0, 0]), then per sampling
    /// texel [cdf within its row, probability of the texel, 0, 0].
    pub trace: Vec<[f32; 4]>,
}

/// Maps are cached and shared: the same map is the same allocation.
impl PartialEq for EnvMap {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl std::fmt::Debug for EnvMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EnvMap({} raster rows, {} traced rows)", self.raster.len(), self.trace.len())
    }
}

/// A linear RGB equirectangular image.
struct Equirect {
    w: usize,
    h: usize,
    px: Vec<[f32; 3]>,
}

impl Equirect {
    /// Box-filtered down to w × h (or kept when already that small).
    fn resized(&self, w: usize, h: usize) -> Equirect {
        if w >= self.w && h >= self.h {
            return Equirect { w: self.w, h: self.h, px: self.px.clone() };
        }
        let mut px = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            let (y0, y1) = (y * self.h / h, ((y + 1) * self.h / h).max(y * self.h / h + 1));
            for x in 0..w {
                let (x0, x1) = (x * self.w / w, ((x + 1) * self.w / w).max(x * self.w / w + 1));
                let mut sum = [0.0f32; 3];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let p = self.px[sy * self.w + sx];
                        sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f32;
                px[y * w + x] = sum.map(|v| v / n);
            }
        }
        Equirect { w, h, px }
    }

    /// The world direction through texel (x, y)'s centre, and its solid
    /// angle.
    fn dir(&self, x: usize, y: usize) -> ([f64; 3], f64) {
        let v = (y as f64 + 0.5) / self.h as f64;
        let u = (x as f64 + 0.5) / self.w as f64;
        let theta = v * PI;
        let phi = (u - 0.5) * 2.0 * PI;
        let (st, ct) = theta.sin_cos();
        let d = [st * phi.sin(), -ct, st * phi.cos()];
        (d, (2.0 * PI / self.w as f64) * (PI / self.h as f64) * st)
    }

    /// Convolved with `kernel(cos)` over the sphere and normalised by the
    /// kernel's own integral (`normalise`), or by π (cosine: irradiance / π).
    fn convolved(&self, w: usize, h: usize, kernel: impl Fn(f64) -> f64, normalise: bool) -> Equirect {
        let src: Vec<([f64; 3], f64, [f32; 3])> = (0..self.h).flat_map(|y| (0..self.w).map(move |x| (x, y))).map(|(x, y)| {
            let (d, a) = self.dir(x, y);
            (d, a, self.px[y * self.w + x])
        }).collect();
        let grid = Equirect { w, h, px: Vec::new() };
        let px = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let (n, _) = grid.dir(x, y);
                let (mut sum, mut weight) = ([0.0f64; 3], 0.0f64);
                for (d, a, c) in &src {
                    let k = kernel(n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) * a;
                    if k > 0.0 {
                        sum = [sum[0] + c[0] as f64 * k, sum[1] + c[1] as f64 * k, sum[2] + c[2] as f64 * k];
                        weight += k;
                    }
                }
                let norm = if normalise { weight.max(1e-12) } else { PI };
                sum.map(|v| (v / norm) as f32)
            })
            .collect();
        Equirect { w, h, px }
    }
}

impl EnvMap {
    pub(crate) fn load(path: &Path) -> Result<EnvMap, String> {
        let img = image::open(path).map_err(|e| e.to_string())?;
        // HDR formats arrive linear; 8-bit images are sRGB.
        let linear = matches!(img, image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_));
        let rgb = img.to_rgb32f();
        let decode = |v: f32| if linear { v.max(0.0) } else { lahza_motion_core::compile::srgb_to_linear(v as f64) as f32 };
        let full = Equirect { w: rgb.width() as usize, h: rgb.height() as usize, px: rgb.pixels().map(|p| p.0.map(decode)).collect() };
        if full.w < 2 || full.h < 1 {
            return Err("empty image".into());
        }
        Ok(Self::from_equirect(&full))
    }

    fn from_equirect(full: &Equirect) -> EnvMap {
        // Raster: sharp level, blurred levels, diffuse.
        let small = full.resized(64, 32);
        let mut levels = Vec::new();
        for (w, h, r) in LEVELS {
            let level = if r == 0.0 {
                full.resized(w, h)
            } else {
                let s2 = 1.2 * r * r;
                small.convolved(w, h, move |c| ((c - 1.0) / s2).exp(), true)
            };
            levels.push((level, r));
        }
        let diffuse = small.convolved(DIFFUSE.0, DIFFUSE.1, |c| c.max(0.0), false);
        let mut raster = vec![[0.0f32; 4]; 1 + levels.len()];
        let mut at = raster.len();
        for (i, (level, r)) in levels.iter().enumerate() {
            raster[1 + i] = [at as f32, level.w as f32, level.h as f32, *r as f32];
            raster.extend(level.px.iter().map(|p| [p[0], p[1], p[2], 0.0]));
            at = raster.len();
        }
        raster[0] = [levels.len() as f32, at as f32, diffuse.w as f32, diffuse.h as f32];
        raster.extend(diffuse.px.iter().map(|p| [p[0], p[1], p[2], 0.0]));

        // Path tracer: the sharp map and the sampling table.
        let sharp = full.resized(TRACE_MAX.0, TRACE_MAX.1);
        let grid = full.resized(SAMPLING.0, SAMPLING.1);
        let (gw, gh) = (grid.w, grid.h);
        let mut trace = vec![[sharp.w as f32, sharp.h as f32, gw as f32, gh as f32]];
        trace.extend(sharp.px.iter().map(|p| [p[0], p[1], p[2], 0.0]));
        let f: Vec<f64> = (0..gh)
            .flat_map(|y| (0..gw).map(move |x| (x, y)))
            .map(|(x, y)| {
                let p = grid.px[y * gw + x];
                let lum = 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
                // A floor keeps every direction reachable.
                (lum + 1e-4) * grid.dir(x, y).1
            })
            .collect();
        let total: f64 = f.iter().sum();
        let rows: Vec<f64> = (0..gh).map(|y| f[y * gw..(y + 1) * gw].iter().sum()).collect();
        let mut acc = 0.0;
        for r in &rows {
            acc += r;
            trace.push([(acc / total) as f32, 0.0, 0.0, 0.0]);
        }
        for y in 0..gh {
            let mut acc = 0.0;
            for x in 0..gw {
                acc += f[y * gw + x];
                trace.push([(acc / rows[y].max(1e-30)) as f32, (f[y * gw + x] / total) as f32, 0.0, 0.0]);
            }
        }
        EnvMap { raster, trace }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test environment written as Radiance HDR: a red sky over a blue
    /// ground, and (optionally) a small sun `sun` times brighter, low on the
    /// −x side and toward the viewer.
    pub(crate) fn write_hdr(path: &Path, sun: f32) {
        let (w, h) = (128usize, 64usize);
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let sky = y < h / 2;
                let mut c = if sky { [0.6f32, 0.08, 0.05] } else { [0.05, 0.08, 0.6] };
                // The sun: around world direction (−0.8, −0.3, −0.5).
                let e = Equirect { w, h, px: Vec::new() };
                let (d, _) = e.dir(x, y);
                let s = [-0.8f64, -0.3, -0.5];
                let l = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
                if sun > 0.0 && (d[0] * s[0] + d[1] * s[1] + d[2] * s[2]) / l > 0.995 {
                    c = [sun, sun * 0.9, sun * 0.8];
                }
                px.push(image::Rgb(c));
            }
        }
        let file = std::fs::File::create(path).unwrap();
        image::codecs::hdr::HdrEncoder::new(std::io::BufWriter::new(file)).encode(&px, w, h).unwrap();
    }

    /// Prefiltering keeps the light: a uniform environment stays uniform at
    /// every level, and the diffuse level is that radiance.
    #[test]
    fn a_uniform_environment_stays_uniform() {
        let e = Equirect { w: 64, h: 32, px: vec![[0.5, 0.25, 1.0]; 64 * 32] };
        let m = EnvMap::from_equirect(&e);
        let n = m.raster[0][0] as usize;
        for i in 1..=n {
            let [at, w, h, _] = m.raster[i];
            for k in 0..(w * h) as usize {
                let p = m.raster[at as usize + k];
                assert!((p[0] - 0.5).abs() < 0.01 && (p[2] - 1.0).abs() < 0.02, "level {i}: {p:?}");
            }
        }
        let [_, at, w, h] = m.raster[0];
        for k in 0..(w * h) as usize {
            let p = m.raster[at as usize + k];
            assert!((p[0] - 0.5).abs() < 0.02, "diffuse: {p:?}");
        }
        // The sampling table is a CDF ending at 1.
        let [_, _, gw, gh] = m.trace[0];
        let rows_at = 1 + (m.trace[0][0] * m.trace[0][1]) as usize;
        assert!((m.trace[rows_at + gh as usize - 1][0] - 1.0).abs() < 1e-4);
        let total: f32 = (0..(gw * gh) as usize).map(|k| m.trace[rows_at + gh as usize + k][1]).sum();
        assert!((total - 1.0).abs() < 1e-3, "probabilities sum to {total}");
    }

    #[test]
    fn hdr_images_load_linear() {
        let dir = std::env::temp_dir().join(format!("lahza-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sky.hdr");
        write_hdr(&path, 0.0);
        let m = EnvMap::load(&path).unwrap();
        let first = m.raster[m.raster[1][0] as usize];
        assert!((first[0] - 0.6).abs() < 0.02 && first[2] < 0.1, "the top row is the red sky: {first:?}");
        assert!(EnvMap::load(&dir.join("missing.hdr")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
