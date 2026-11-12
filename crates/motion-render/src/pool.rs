//! The GPU's mesh pool: generated meshes in object space, uploaded once
//! and kept across frames for every mesh layer that shows them (display
//! lists hold only each layer's transforms and materials, `mesh3d::IROWS`).
//! A fixed budget bounds the device memory; when a new mesh does not fit,
//! the meshes used longest ago give way.

use crate::mesh3d::{DeformBase, PoolRef, Pooled, Source, PROWS};
use crate::mesh_gen::{Generators, Job};
use blade_graphics as gpu;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Device memory the pool may hold where nothing says more (`budget_for`).
/// A frame's meshes past the pool's budget are not drawn (and say so):
/// 256 MB is 8 million vertices with their triangles.
pub const MESH_POOL_BUDGET: usize = 256 << 20;
/// The most the pool is given however large the GPU.
const MOST_BUDGET: usize = 2 << 30;

/// The pool's budget on a GPU with `device_memory` bytes of its own whose
/// storage buffers may be `max_buffer` bytes (0: unknown): an eighth of
/// the memory (1.5 GB of a 12 GB card; a film of scanned rocks filled the
/// fixed 256 MB and had meshes uploaded again frame after frame), within
/// what one buffer may hold.
pub fn budget_for(device_memory: u64, max_buffer: u64) -> usize {
    let by_buffer = if max_buffer == 0 { MESH_POOL_BUDGET } else { max_buffer.min(usize::MAX as u64) as usize };
    ((device_memory / 8) as usize).min(by_buffer).clamp(MESH_POOL_BUDGET, MOST_BUDGET)
}
/// The pool's first size; it doubles as meshes arrive, up to the budget.
const FIRST_SIZE: usize = 4 << 20;
const ROW: usize = 16;

struct Entry {
    at: u32,
    rows: u32,
    /// The pool's frame that last drew it.
    used: u64,
    /// New every frame (animated Slang geometry): freed once a frame does
    /// not draw it, rather than filling the budget with past frames.
    animated: bool,
}

pub(crate) struct MeshPool {
    pub buffer: gpu::Buffer,
    /// Rows the buffer holds.
    capacity: u32,
    entries: HashMap<Arc<[u8]>, Entry>,
    /// Free ranges: first row → rows (neighbours merged).
    free: BTreeMap<u32, u32>,
    /// Rows let go of that frames still on the GPU may read: (the last
    /// frame that may, first row, rows). Free once that frame is done.
    limbo: Vec<(u64, u32, u32)>,
    /// Every frame up to this one is off the GPU.
    done: u64,
    /// Buffers a frame still reads (a pool outgrown while that frame was
    /// recorded), with the frame: freed once it is done.
    retired: Vec<(u64, gpu::Buffer)>,
    frame: u64,
    /// Counts since the profile last took them.
    pub uploads: u64,
    pub upload_bytes: u64,
    pub evictions: u64,
    pub warning: Option<String>,
    /// Slang meshes the GPU generates in the pool, not the CPU.
    pub generators: Generators,
    /// What generated meshes got wrong (values that are not numbers,
    /// triangles past their points): errors, known once their frame is
    /// back from the GPU.
    pub errors: Vec<String>,
    /// Why a mesh the GPU could have generated came from the CPU.
    pub notes: Vec<String>,
    /// Device memory it may hold (`budget_for`).
    pub budget: usize,
}

impl MeshPool {
    pub fn new(context: &gpu::Context, budget: usize) -> Self {
        let capacity = (FIRST_SIZE / ROW) as u32;
        Self {
            buffer: Self::create(context, capacity),
            capacity,
            entries: HashMap::new(),
            free: BTreeMap::from([(0, capacity)]),
            limbo: Vec::new(),
            done: 0,
            retired: Vec::new(),
            frame: 0,
            uploads: 0,
            upload_bytes: 0,
            evictions: 0,
            warning: None,
            generators: Generators::default(),
            errors: Vec::new(),
            notes: Vec::new(),
            budget: budget.max(FIRST_SIZE),
        }
    }

    fn create(context: &gpu::Context, rows: u32) -> gpu::Buffer {
        context.create_buffer(gpu::BufferDesc { name: "motion mesh pool", size: rows as u64 * ROW as u64, memory: gpu::Memory::Device })
    }

    /// Bytes the pool's meshes take, and its buffer's size.
    pub fn bytes(&self) -> (usize, usize) {
        (self.entries.values().map(|e| e.rows as usize * ROW).sum(), self.capacity as usize * ROW)
    }

    pub fn meshes(&self) -> usize {
        self.entries.len()
    }

    /// Makes sure every instance's mesh is in the pool (uploading those
    /// that are not) and writes where it is into the instance's row of
    /// `mesh`: [vertex row, triangle row, triangles, _]. An instance whose
    /// mesh finds no room keeps 0 triangles and is not drawn.
    /// `id` is the frame being recorded (`after_frame` names it again);
    /// new meshes go through its `stage`. Frames before it may still be on
    /// the GPU: rows they read are not written (`fits` says beforehand
    /// whether the new meshes find room without them).
    pub fn resolve(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, stage: &mut crate::gpu::Stage, id: u64, refs: &[PoolRef], mesh: &mut [[f32; 4]]) -> Result<(), String> {
        self.frame += 1;
        let frame = self.frame;
        // New meshes, once each.
        let mut new: Vec<&PoolRef> = Vec::new();
        let mut seen: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
        for r in refs {
            match self.entries.get_mut(r.key()) {
                Some(e) => e.used = frame,
                None => {
                    if seen.insert(r.key()) {
                        new.push(r);
                    }
                }
            }
        }
        let stale: Vec<Arc<[u8]>> = self.entries.iter().filter(|(_, e)| e.animated && e.used < frame).map(|(k, _)| k.clone()).collect();
        for k in stale {
            if let Some(e) = self.entries.remove(&k) {
                self.retire(id, e.at, e.rows);
                self.evictions += 1;
            }
        }
        if !new.is_empty() {
            // Grow once for all of them when there is not room (fragments
            // aside); past the budget, older meshes make way instead.
            let need: usize = new.iter().map(|r| r.mesh.rows()).sum::<usize>() + self.more_rows(new.iter().map(|r| &r.mesh)).iter().map(|n| *n as usize).sum::<usize>();
            if let Some(rows) = self.grown(need) {
                let bigger = Self::create(context, rows);
                encoder.transfer("mesh pool grow").copy_buffer_to_buffer(self.buffer.into(), bigger.into(), self.capacity as u64 * ROW as u64);
                self.retired.push((id, std::mem::replace(&mut self.buffer, bigger)));
                self.release(self.capacity, rows - self.capacity);
                self.capacity = rows;
            }
            let mut rows: Vec<[f32; 4]> = Vec::with_capacity(need);
            // (staging row, pool row, rows) per placed mesh the CPU has.
            let mut copies: Vec<(usize, u32, u32)> = Vec::new();
            // Those the GPU generates where they are placed.
            let mut jobs: Vec<Job> = Vec::new();
            'meshes: for r in new {
                let n = r.mesh.rows() as u32;
                // A Slang mesh the GPU was to generate, when its kernel is
                // not one this GPU takes: from the CPU after all.
                let mut fallback: Option<lahza_motion_core::mesh::Mesh> = None;
                if !matches!(r.mesh.source, Source::Mesh(_)) {
                    let mut ready = self.generators.ready(context, &r.mesh, id).unwrap_or_else(|e| {
                        self.notes.push(e);
                        false
                    });
                    // (A generated mesh a deformer moves needs both kernels.)
                    if let Source::DeformedGenerated { generated, .. } = &r.mesh.source {
                        ready &= self.generators.ready(context, generated, id).unwrap_or_else(|e| {
                            self.notes.push(e);
                            false
                        });
                    }
                    if !ready {
                        let made = match &r.mesh.source {
                            Source::Deformed { base, params } => match &params.deform {
                                Some(d) => lahza_motion_core::slang_mesh::deform_prepared(&base.base, d),
                                None => continue,
                            },
                            Source::Generated(params) => lahza_motion_core::mesh::generate(params),
                            Source::DeformedGenerated { generated, params, .. } => match (&generated.source, &params.deform) {
                                (Source::Generated(plain), Some(d)) => lahza_motion_core::mesh::generate(plain).and_then(|mut m| lahza_motion_core::slang_mesh::deform(&mut m, d).map(|_| m)),
                                _ => continue,
                            },
                            Source::Mesh(_) => continue,
                        };
                        match made {
                            // (The GPU's subdivided mesh keeps more points
                            // than the CPU's: its rows are padded.)
                            Ok(m) if m.positions.len() <= r.mesh.verts as usize && m.tris.len() == r.mesh.tris as usize && (m.positions.len() == r.mesh.verts as usize || matches!(r.mesh.source, Source::DeformedGenerated { .. })) => fallback = Some(m),
                            Ok(_) => continue,
                            Err(e) => {
                                if !self.errors.contains(&e) {
                                    self.errors.push(e);
                                }
                                continue;
                            }
                        }
                    }
                }
                // The mesh a deformer moves: in the pool once, for every
                // frame's move of it.
                let mut before = 0;
                if let (Source::Deformed { base, .. }, None) = (&r.mesh.source, &fallback) {
                    before = match self.entries.get_mut(&base.key) {
                        Some(e) => {
                            e.used = frame;
                            e.at
                        }
                        None => {
                            let n = base.rows() as u32;
                            let Some(at) = self.alloc(n, id) else {
                                self.warning = Some("mesh memory budget reached; some meshes are not drawn".into());
                                continue;
                            };
                            copies.push((rows.len(), at, n));
                            base.write(&mut rows);
                            self.uploads += 1;
                            self.entries.insert(base.key.clone(), Entry { at, rows: n, used: frame, animated: false });
                            at
                        }
                    };
                }
                // A mesh the GPU generates and a deformer moves: the
                // generated mesh in the pool as any other, and room for it
                // readied for the deformer (this frame's only).
                let mut weld = None;
                let mut levels = Vec::new();
                if let (Source::DeformedGenerated { generated, levels: sizes, .. }, None) = (&r.mesh.source, &fallback) {
                    let from = match self.entries.get_mut(&generated.key) {
                        Some(e) => {
                            e.used = frame;
                            e.at
                        }
                        None => {
                            let n = generated.rows() as u32;
                            let Some(at) = self.alloc(n, id) else {
                                self.warning = Some("mesh memory budget reached; some meshes are not drawn".into());
                                continue;
                            };
                            jobs.push(Job { at, before: 0, weld: None, levels: Vec::new(), mesh: generated.clone() });
                            self.entries.insert(generated.key.clone(), Entry { at, rows: n, used: frame, animated: generated.animated });
                            at
                        }
                    };
                    // Subdivided as the deformer asks, level by level, in
                    // rows of this frame's.
                    let (mut from, mut points, mut triangles) = (from, generated.verts, generated.tris);
                    for (k, &(v, t)) in sizes.iter().enumerate() {
                        let n = v * PROWS as u32 + t;
                        let Some(at) = self.alloc(n, id) else {
                            self.warning = Some("mesh memory budget reached; some meshes are not drawn".into());
                            continue 'meshes;
                        };
                        let key: Arc<[u8]> = r.mesh.key.iter().copied().chain(format!(" level {k}").into_bytes()).collect::<Vec<u8>>().into();
                        self.entries.insert(key, Entry { at, rows: n, used: frame, animated: true });
                        levels.push(crate::mesh_gen::Level { from, to: at, points, triangles });
                        (from, points, triangles) = (at, v, t);
                    }
                    let n = r.mesh.verts * crate::mesh3d::BROWS as u32 + r.mesh.tris;
                    let Some(at) = self.alloc(n, id) else {
                        self.warning = Some("mesh memory budget reached; some meshes are not drawn".into());
                        continue;
                    };
                    let readied: Arc<[u8]> = r.mesh.key.iter().copied().chain(*b" readied").collect::<Vec<u8>>().into();
                    self.entries.insert(readied, Entry { at, rows: n, used: frame, animated: true });
                    (before, weld) = (at, Some(from));
                }
                let Some(at) = self.alloc(n, id) else {
                    self.warning = Some("mesh memory budget reached; some meshes are not drawn".into());
                    continue;
                };
                let from_cpu: Option<&lahza_motion_core::mesh::Mesh> = match &r.mesh.source {
                    Source::Mesh(m) => Some(m),
                    Source::Generated(_) | Source::Deformed { .. } | Source::DeformedGenerated { .. } => fallback.as_ref(),
                };
                match from_cpu {
                    Some(m) => {
                        copies.push((rows.len(), at, n));
                        Pooled::write_padded(m, r.mesh.verts as usize, &mut rows);
                        self.uploads += 1;
                    }
                    None => jobs.push(Job { at, before, weld, levels, mesh: r.mesh.clone() }),
                }
                self.entries.insert(r.mesh.key.clone(), Entry { at, rows: n, used: frame, animated: r.mesh.animated });
            }
            let bytes = rows.len() * ROW;
            self.upload_bytes += bytes as u64;
            if bytes > 0 {
                let (host, base) = stage.put(context, bytemuck::cast_slice(&rows));
                let mut pass = encoder.transfer("mesh pool upload");
                for (from, at, n) in copies {
                    pass.copy_buffer_to_buffer(host.at(base + (from * ROW) as u64), self.buffer.at(at as u64 * ROW as u64), n as u64 * ROW as u64);
                }
            }
            self.generators.run(context, encoder, id, self.buffer, &jobs);
        }
        let b = f32::from_bits;
        for r in refs {
            let row = &mut mesh[r.row as usize];
            match self.entries.get(r.key()) {
                Some(e) => {
                    let verts = r.mesh.verts * PROWS as u32;
                    row[0] = b(e.at);
                    row[1] = b(e.at + verts);
                }
                None => row[2] = b(0),
            }
        }
        Ok(())
    }

    /// The rows the pool grows to for `need` more, when they do not fit
    /// the free ones (fragments aside) and the budget allows.
    fn grown(&self, need: usize) -> Option<u32> {
        let room: usize = self.free.values().map(|&n| n as usize).sum();
        let budget = (self.budget / ROW).min(u32::MAX as usize) as u32;
        (room < need && self.capacity < budget).then(|| {
            let used = (self.capacity as usize - room + need).next_power_of_two();
            (self.capacity * 2).max(used.min(u32::MAX as usize) as u32).min(budget)
        })
    }

    /// Room for `n` rows: the first free range that fits, after evicting
    /// meshes this frame (`id`) does not draw, longest unused first.
    fn alloc(&mut self, n: u32, id: u64) -> Option<u32> {
        // Bigger than the whole pool: evicting would not make room.
        if n > self.capacity {
            return None;
        }
        loop {
            if let Some(at) = take(&mut self.free, n) {
                return Some(at);
            }
            // (Frames on the GPU may read what would be evicted: its rows
            // would not be free. `fits` has the caller wait for them.)
            if id.saturating_sub(1) > self.done {
                return None;
            }
            let oldest = self.entries.iter().filter(|(_, e)| e.used < self.frame).min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone())?;
            let e = self.entries.remove(&oldest)?;
            give(&mut self.free, e.at, e.rows);
            self.evictions += 1;
        }
    }

    fn release(&mut self, at: u32, n: u32) {
        give(&mut self.free, at, n);
    }

    /// Lets go of rows frame `id` does not read: free at once when every
    /// frame before it is off the GPU, else once they are.
    fn retire(&mut self, id: u64, at: u32, n: u32) {
        let last = id.saturating_sub(1);
        if last <= self.done {
            self.release(at, n);
        } else {
            self.limbo.push((last, at, n));
        }
    }

    /// The meshes deformers move that `meshes` (new to the pool) need
    /// there too, once each.
    fn new_bases<'a>(&self, meshes: impl Iterator<Item = &'a Arc<Pooled>>) -> Vec<&'a Arc<DeformBase>> {
        let mut seen: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
        meshes
            .filter_map(|m| match &m.source {
                Source::Deformed { base, .. } if !self.entries.contains_key(&base.key) && seen.insert(&base.key) => Some(base),
                _ => None,
            })
            .collect()
    }

    /// The rows `meshes` (new to the pool) need there beside their own:
    /// the meshes their deformers move.
    fn more_rows<'a>(&self, meshes: impl Iterator<Item = &'a Arc<Pooled>> + Clone) -> Vec<u32> {
        let mut rows: Vec<u32> = self.new_bases(meshes.clone()).iter().map(|b| b.rows() as u32).collect();
        let mut seen: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
        for m in meshes {
            if let Source::DeformedGenerated { generated, levels, .. } = &m.source {
                if !self.entries.contains_key(&generated.key) && seen.insert(&generated.key) {
                    rows.push(generated.rows() as u32);
                }
                rows.extend(levels.iter().map(|(v, t)| v * PROWS as u32 + t));
                rows.push(m.verts * crate::mesh3d::BROWS as u32 + m.tris);
            }
        }
        rows
    }

    /// Whether `resolve` finds room for these instances' new meshes while
    /// the frames before are still on the GPU: in the free rows (grown as
    /// `resolve` grows them), without those the frames may read. When
    /// not, they must be off the GPU first (`idle`).
    pub fn fits(&self, refs: &[PoolRef]) -> bool {
        let mut seen: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
        let fresh: Vec<&PoolRef> = refs.iter().filter(|r| !self.entries.contains_key(r.key()) && seen.insert(r.key())).collect();
        let mut new: Vec<u32> = fresh.iter().map(|r| r.mesh.rows() as u32).collect();
        new.extend(self.more_rows(fresh.iter().map(|r| &r.mesh)));
        if new.is_empty() {
            return true;
        }
        let mut free = self.free.clone();
        if let Some(rows) = self.grown(new.iter().map(|&n| n as usize).sum()) {
            give(&mut free, self.capacity, rows - self.capacity);
        }
        new.iter().all(|&n| take(&mut free, n).is_some())
    }

    /// Every frame before `id` is off the GPU: what they read is free.
    pub fn idle(&mut self, id: u64) {
        self.done = self.done.max(id.saturating_sub(1));
        let done = self.done;
        let (free, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.limbo).into_iter().partition(|l| l.0 <= done);
        self.limbo = waiting;
        for (_, at, n) in free {
            self.release(at, n);
        }
    }

    /// After frame `id`'s GPU work (and that of the frames before it):
    /// frees the buffers and rows they were still reading.
    pub fn after_frame(&mut self, context: &gpu::Context, id: u64) {
        self.idle(id.saturating_add(1));
        self.generators.after_frame(context, id, &mut self.errors);
        self.retired.retain(|(frame, b)| {
            if *frame <= id {
                context.destroy_buffer(*b);
            }
            *frame > id
        });
    }

    pub fn destroy(&mut self, context: &gpu::Context) {
        self.after_frame(context, u64::MAX);
        self.generators.destroy(context);
        context.destroy_buffer(self.buffer);
    }
}

/// Takes `n` rows from the free ranges: the first that fits.
fn take(free: &mut BTreeMap<u32, u32>, n: u32) -> Option<u32> {
    let (&at, &len) = free.iter().find(|(_, &len)| len >= n)?;
    free.remove(&at);
    if len > n {
        free.insert(at + n, len - n);
    }
    Some(at)
}

/// Returns rows to the free ranges, merging with their neighbours.
fn give(free: &mut BTreeMap<u32, u32>, mut at: u32, mut n: u32) {
    if let Some((&prev, &len)) = free.range(..at).next_back() {
        if prev + len == at {
            free.remove(&prev);
            at = prev;
            n += len;
        }
    }
    if let Some(len) = free.remove(&(at + n)) {
        n += len;
    }
    free.insert(at, n);
}
