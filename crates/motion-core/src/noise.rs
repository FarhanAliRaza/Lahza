//! Seeded procedural noise in 3D (no UVs needed): value noise, fbm (value
//! noise in octaves) and cellular (distance to the nearest of one random
//! point per cell). Values are in 0..1.
//!
//! The renderers carry exact copies (`motion.wgsl`, `mesh3d.rs`,
//! `pt.wgsl`): integer hashing on the lattice, then f32 arithmetic in the
//! same order, so GPU, CPU and path tracer agree.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoiseKind {
    /// Smooth random values on a lattice.
    Value,
    /// Value noise summed over octaves (clouds, grain, rock).
    #[default]
    Fbm,
    /// Distance to the nearest scattered point (cells, crust, pebbles).
    Cellular,
}

impl NoiseKind {
    pub fn code(self) -> u32 {
        match self {
            NoiseKind::Value => 0,
            NoiseKind::Fbm => 1,
            NoiseKind::Cellular => 2,
        }
    }
}

/// A noise field: `kind` at feature size `scale` (px), `octaves` (fbm),
/// `seed`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub kind: NoiseKind,
    pub scale: f64,
    pub octaves: u32,
    pub seed: u32,
}

impl Field {
    /// The value at point p (px), 0..1.
    pub fn at(&self, p: [f64; 3]) -> f64 {
        let s = 1.0 / self.scale.max(1e-3) as f32;
        let q = [p[0] as f32 * s, p[1] as f32 * s, p[2] as f32 * s];
        sample(self.kind.code(), q, self.octaves, self.seed) as f64
    }
}

/// PCG hash (as `pcg` in the shaders).
pub fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747796405).wrapping_add(2891336453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277803737);
    (word >> 22) ^ word
}

/// A lattice point's random number in 0..1.
pub fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let h = pcg((x as u32).wrapping_mul(73856093) ^ (y as u32).wrapping_mul(19349663) ^ (z as u32).wrapping_mul(83492791) ^ pcg(seed));
    (h >> 8) as f32 / 16777216.0
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// As WGSL's mix (so the shaders' copies agree exactly).
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// Value noise at q (lattice units), 0..1.
pub fn value(q: [f32; 3], seed: u32) -> f32 {
    let (fx, fy, fz) = (q[0].floor(), q[1].floor(), q[2].floor());
    let (x, y, z) = (fx as i32, fy as i32, fz as i32);
    let (tx, ty, tz) = (fade(q[0] - fx), fade(q[1] - fy), fade(q[2] - fz));
    let h = |dx: i32, dy: i32, dz: i32| hash3(x + dx, y + dy, z + dz, seed);
    let a = mix(mix(h(0, 0, 0), h(1, 0, 0), tx), mix(h(0, 1, 0), h(1, 1, 0), tx), ty);
    let b = mix(mix(h(0, 0, 1), h(1, 0, 1), tx), mix(h(0, 1, 1), h(1, 1, 1), tx), ty);
    mix(a, b, tz)
}

/// Octaves of value noise, each twice as fine and half as strong, 0..1.
pub fn fbm(q: [f32; 3], octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut f) = (0.0f32, 1.0f32, 0.0f32, 1.0f32);
    for o in 0..octaves.clamp(1, 8) {
        sum += amp * value([q[0] * f, q[1] * f, q[2] * f], seed.wrapping_add(o * 1013));
        total += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / total
}

/// Distance to the nearest of one random point per cell (cell units),
/// clamped to 0..1.
pub fn cellular(q: [f32; 3], seed: u32) -> f32 {
    let (fx, fy, fz) = (q[0].floor(), q[1].floor(), q[2].floor());
    let mut best = 4.0f32;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y, z) = (fx as i32 + dx, fy as i32 + dy, fz as i32 + dz);
                let px = x as f32 + hash3(x, y, z, seed);
                let py = y as f32 + hash3(x, y, z, seed ^ 0x9e37);
                let pz = z as f32 + hash3(x, y, z, seed ^ 0x7f4a);
                let d = (px - q[0]) * (px - q[0]) + (py - q[1]) * (py - q[1]) + (pz - q[2]) * (pz - q[2]);
                best = best.min(d);
            }
        }
    }
    best.sqrt().min(1.0)
}

/// Kind by code (0 value, 1 fbm, 2 cellular).
pub fn sample(kind: u32, q: [f32; 3], octaves: u32, seed: u32) -> f32 {
    match kind {
        0 => value(q, seed),
        2 => cellular(q, seed),
        _ => fbm(q, octaves, seed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_seeded_smooth_and_in_range() {
        for kind in [0, 1, 2] {
            let (mut lo, mut hi) = (1.0f32, 0.0f32);
            for i in 0..2000 {
                let q = [i as f32 * 0.137, (i * 7 % 13) as f32 * 0.31, (i % 5) as f32 * 0.7];
                let v = sample(kind, q, 4, 3);
                lo = lo.min(v);
                hi = hi.max(v);
                assert_eq!(v, sample(kind, q, 4, 3), "deterministic");
                // Smooth: a tiny step changes it a little.
                let w = sample(kind, [q[0] + 1e-3, q[1], q[2]], 4, 3);
                assert!((v - w).abs() < 0.02, "kind {kind}: {v} → {w}");
            }
            assert!(lo >= 0.0 && hi <= 1.0 && hi - lo > 0.3, "kind {kind}: range {lo}..{hi}");
        }
        assert_ne!(value([0.5, 0.5, 0.5], 1), value([0.5, 0.5, 0.5], 2), "seeds differ");
    }
}
