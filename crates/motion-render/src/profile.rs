//! `--profile`: where a render's time goes, from one run.
//!
//! Three views of the same frames: CPU stages (wall clock), GPU passes
//! (timestamps around every pass) and the path tracer's work per layer
//! (rays that hit its surfaces, density lookups in its volumes). Stages
//! nest by name: "gpu frame › wait" is part of "gpu frame".

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The tracer counts on one pixel in this many (an 8 × 8 grid), so the
/// counting does not slow the pass it measures; counts are scaled back up.
pub(crate) const COUNT_EVERY: u64 = 64;

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub frames: u64,
    /// CPU stages, ms summed over the frames.
    pub cpu: BTreeMap<String, f64>,
    /// GPU passes by name, ms summed over the frames.
    pub gpu: BTreeMap<String, f64>,
    /// Path tracer work by layer id.
    pub layers: BTreeMap<String, LayerWork>,
    /// Ray queries the tracer made (camera, bounce and shadow rays).
    pub rays: u64,
    /// Triangle candidates those queries handed back to the shader (every
    /// triangle a ray's traversal touches, not just the nearest).
    pub candidates: u64,
    /// Density lookups that found no medium (one pixel in 64, scaled).
    pub density_empty: u64,
    /// Lockstep use in density work: every pixel's lookups, and the lane
    /// slots its 8 × 4 block spent (32 × the busiest pixel's lookups).
    pub lane_lookups: u64,
    pub lane_slots: u64,
    /// Ray queries and density lookups by how many times the path had
    /// scattered inside a volume (0, 1, 2, 3 or more).
    pub rays_by_scatter: [u64; 4],
    pub lookups_by_scatter: [u64; 4],
    /// The GPU's mesh pool.
    pub pool: PoolStats,
}

/// The GPU's mesh pool (`pool.rs`): uploads and evictions summed over the
/// frames, its size after the last one.
#[derive(Clone, Copy, Debug, Default)]
pub struct PoolStats {
    pub uploads: u64,
    pub upload_bytes: u64,
    /// Slang meshes the GPU generated in the pool (nothing uploaded).
    pub generated: u64,
    pub evictions: u64,
    pub meshes: u64,
    pub bytes: u64,
    pub capacity: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LayerWork {
    /// Rays that stopped on this layer's surfaces (or boundaries).
    pub hits: u64,
    /// Density lookups in this layer's volume (noise or grid).
    pub density: u64,
}

impl Profile {
    /// Adds the time since `since` to `stage`; returns now, to chain stages.
    pub fn stage(&mut self, stage: &str, since: Instant) -> Instant {
        let now = Instant::now();
        self.add(stage, now - since);
        now
    }

    pub fn add(&mut self, stage: &str, d: Duration) {
        *self.cpu.entry(stage.to_string()).or_default() += d.as_secs_f64() * 1000.0;
    }

    /// Adds another profile's stages, passes and counts (not its frames).
    pub fn merge(&mut self, other: Profile) {
        for (k, v) in other.cpu {
            *self.cpu.entry(k).or_default() += v;
        }
        for (k, v) in other.gpu {
            *self.gpu.entry(k).or_default() += v;
        }
        for (k, v) in other.layers {
            let l = self.layers.entry(k).or_default();
            l.hits += v.hits;
            l.density += v.density;
        }
        self.rays += other.rays;
        self.candidates += other.candidates;
        self.density_empty += other.density_empty;
        self.lane_lookups += other.lane_lookups;
        self.lane_slots += other.lane_slots;
        for d in 0..4 {
            self.rays_by_scatter[d] += other.rays_by_scatter[d];
            self.lookups_by_scatter[d] += other.lookups_by_scatter[d];
        }
        let p = other.pool;
        self.pool.uploads += p.uploads;
        self.pool.upload_bytes += p.upload_bytes;
        self.pool.generated += p.generated;
        self.pool.evictions += p.evictions;
        if p.capacity > 0 {
            (self.pool.meshes, self.pool.bytes, self.pool.capacity) = (p.meshes, p.bytes, p.capacity);
        }
    }

    pub fn add_gpu(&mut self, passes: &[(String, Duration)]) {
        for (name, d) in passes {
            *self.gpu.entry(name.clone()).or_default() += d.as_secs_f64() * 1000.0;
        }
    }

    /// The report: each table sorted by cost, with ms per frame and share.
    pub fn report(&self) -> Value {
        let frames = self.frames.max(1) as f64;
        let table = |m: &BTreeMap<String, f64>, total: f64| -> Vec<Value> {
            let mut rows: Vec<_> = m.iter().collect();
            rows.sort_by(|a, b| b.1.total_cmp(a.1));
            rows.into_iter()
                .map(|(k, v)| json!({"stage": k, "ms_per_frame": round(v / frames), "ms_total": round(*v), "share": round(v / total.max(1e-9))}))
                .collect()
        };
        // Shares of the top-level stages (nested ones are part of theirs).
        let cpu_total: f64 = self.cpu.iter().filter(|(k, _)| !k.contains('›')).map(|(_, v)| v).sum();
        let gpu_total: f64 = self.gpu.values().sum();
        let density: u64 = self.layers.values().map(|l| l.density).sum();
        let hits: u64 = self.layers.values().map(|l| l.hits).sum();
        let mut layers: Vec<_> = self.layers.iter().collect();
        layers.sort_by(|a, b| (b.1.density, b.1.hits).cmp(&(a.1.density, a.1.hits)));
        let layers: Vec<Value> = layers
            .into_iter()
            .map(|(id, l)| {
                json!({"layer": id, "density_lookups_per_frame": l.density / self.frames.max(1), "density_share": round(l.density as f64 / (density.max(1)) as f64),
                       "ray_hits_per_frame": l.hits / self.frames.max(1), "hit_share": round(l.hits as f64 / hits.max(1) as f64)})
            })
            .collect();
        json!({
            "frames": self.frames,
            "cpu_ms_per_frame": round(cpu_total / frames),
            "gpu_ms_per_frame": round(gpu_total / frames),
            "cpu": table(&self.cpu, cpu_total),
            "gpu": table(&self.gpu, gpu_total),
            "tracer": {"ray_queries_per_frame": self.rays / self.frames.max(1), "candidates_per_ray": round(self.candidates as f64 / self.rays.max(1) as f64), "empty_lookup_share": round(self.density_empty as f64 / self.layers.values().map(|l| l.density).sum::<u64>().max(1) as f64), "lockstep_use": round(self.lane_lookups as f64 / self.lane_slots.max(1) as f64), "rays_by_volume_scatters": self.rays_by_scatter, "lookups_by_volume_scatters": self.lookups_by_scatter, "density_lookups_per_frame": density / self.frames.max(1),
                       "counted_on": format!("1 pixel in {COUNT_EVERY}, scaled"), "layers": layers},
            "mesh_pool": {"meshes": self.pool.meshes, "bytes": self.pool.bytes, "capacity": self.pool.capacity, "budget": crate::pool::MESH_POOL_BUDGET,
                          "uploads": self.pool.uploads, "upload_bytes": self.pool.upload_bytes, "generated_on_gpu": self.pool.generated, "evictions": self.pool.evictions},
            "summary": self.summary(),
        })
    }

    /// The same as readable lines (the CLI prints them).
    pub fn summary(&self) -> Vec<String> {
        let frames = self.frames.max(1) as f64;
        let mut out = vec![format!("profile: {} frames", self.frames)];
        let mut section = |title: &str, m: &BTreeMap<String, f64>, top_only_total: bool| {
            let total: f64 = m.iter().filter(|(k, _)| !top_only_total || !k.contains('›')).map(|(_, v)| v).sum();
            out.push(format!("{title} ({:.1} ms/frame)", total / frames));
            let mut rows: Vec<_> = m.iter().collect();
            rows.sort_by(|a, b| b.1.total_cmp(a.1));
            for (k, v) in rows {
                out.push(format!("  {:>9.2} ms  {:>5.1}%  {k}", v / frames, v / total.max(1e-9) * 100.0));
            }
        };
        section("CPU stages", &self.cpu, true);
        section("GPU passes", &self.gpu, false);
        if self.pool.capacity > 0 {
            let mb = |b: u64| b as f64 / (1 << 20) as f64;
            let p = self.pool;
            out.push(format!(
                "mesh pool: {} meshes, {:.1} MB in a {:.1} MB buffer (budget {:.0} MB); {} uploaded ({:.1} MB), {} generated on the GPU, {} evicted over the frames",
                p.meshes,
                mb(p.bytes),
                mb(p.capacity),
                mb(crate::pool::MESH_POOL_BUDGET as u64),
                p.uploads,
                mb(p.upload_bytes),
                p.generated,
                p.evictions
            ));
        }
        if !self.layers.is_empty() {
            let density: u64 = self.layers.values().map(|l| l.density).sum::<u64>().max(1);
            out.push(format!("tracer: {} ray queries/frame, {:.1} triangle candidates per ray, {} density lookups/frame", self.rays / self.frames.max(1), self.candidates as f64 / self.rays.max(1) as f64, density / self.frames.max(1)));
            out.push(format!(
                "  density lookups that found nothing: {:.1}%; lockstep use in density work: {:.1}% (lookups ÷ 32 × busiest pixel's, per 8×4 block)",
                self.density_empty as f64 / density as f64 * 100.0,
                self.lane_lookups as f64 / self.lane_slots.max(1) as f64 * 100.0
            ));
            let (rays, looks) = (self.rays_by_scatter.iter().sum::<u64>().max(1) as f64, self.lookups_by_scatter.iter().sum::<u64>().max(1) as f64);
            for (d, name) in ["0 (camera paths, surface bounces)", "1", "2", "3+"].iter().enumerate() {
                out.push(format!(
                    "  after {name} volume scatters: {:>5.1}% of ray queries, {:>5.1}% of density lookups",
                    self.rays_by_scatter[d] as f64 / rays * 100.0,
                    self.lookups_by_scatter[d] as f64 / looks * 100.0
                ));
            }
            let mut rows: Vec<_> = self.layers.iter().collect();
            rows.sort_by(|a, b| (b.1.density, b.1.hits).cmp(&(a.1.density, a.1.hits)));
            for (id, l) in rows {
                out.push(format!("  {:>5.1}% density  {:>12} hits/frame  {id}", l.density as f64 / density as f64 * 100.0, l.hits / self.frames.max(1)));
            }
        }
        out
    }
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
