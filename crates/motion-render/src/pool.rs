//! The GPU's mesh pool: generated meshes in object space, uploaded once
//! and kept across frames for every mesh layer that shows them (display
//! lists hold only each layer's transforms and materials, `mesh3d::IROWS`).
//! A fixed budget bounds the device memory; when a new mesh does not fit,
//! the meshes used longest ago give way.

use crate::mesh3d::{PoolRef, PROWS};
use blade_graphics as gpu;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Device memory the pool may hold. A frame's meshes past it are not drawn
/// (and say so): 256 MB is 8 million vertices with their triangles.
pub const MESH_POOL_BUDGET: usize = 256 << 20;
/// The pool's first size; it doubles as meshes arrive, up to the budget.
const FIRST_SIZE: usize = 4 << 20;
/// Staging memory kept between frames that upload nothing.
const KEEP_STAGING: usize = 16 << 20;
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
    staging: Option<(gpu::Buffer, usize)>,
    /// Buffers the frame being drawn still reads (a pool outgrown this
    /// frame): freed once it is done.
    retired: Vec<gpu::Buffer>,
    /// A frame resolved meshes and has not finished: if it never does (it
    /// failed before or on the GPU), its uploads may not have happened.
    in_flight: bool,
    frame: u64,
    /// Counts since the profile last took them.
    pub uploads: u64,
    pub upload_bytes: u64,
    pub evictions: u64,
    pub warning: Option<String>,
}

impl MeshPool {
    pub fn new(context: &gpu::Context) -> Self {
        let capacity = (FIRST_SIZE / ROW) as u32;
        Self {
            buffer: Self::create(context, capacity),
            capacity,
            entries: HashMap::new(),
            free: BTreeMap::from([(0, capacity)]),
            staging: None,
            retired: Vec::new(),
            in_flight: false,
            frame: 0,
            uploads: 0,
            upload_bytes: 0,
            evictions: 0,
            warning: None,
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
    pub fn resolve(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, refs: &[PoolRef], mesh: &mut [[f32; 4]]) -> Result<(), String> {
        if std::mem::replace(&mut self.in_flight, true) {
            // The last frame failed: trust nothing it may have moved.
            self.entries.clear();
            self.free = BTreeMap::from([(0, self.capacity)]);
        }
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
                self.release(e.at, e.rows);
                self.evictions += 1;
            }
        }
        if new.is_empty() {
            if self.staging.as_ref().is_some_and(|s| s.1 > KEEP_STAGING) {
                if let Some((b, _)) = self.staging.take() {
                    context.destroy_buffer(b);
                }
            }
        } else {
            // Grow once for all of them when there is not room (fragments
            // aside); past the budget, older meshes make way instead.
            let need: usize = new.iter().map(|r| r.mesh.rows()).sum();
            let room: usize = self.free.values().map(|&n| n as usize).sum();
            let budget = (MESH_POOL_BUDGET / ROW) as u32;
            if room < need && self.capacity < budget {
                let used = (self.capacity as usize - room + need).next_power_of_two();
                let rows = (self.capacity * 2).max(used.min(u32::MAX as usize) as u32).min(budget);
                let bigger = Self::create(context, rows);
                encoder.transfer("mesh pool grow").copy_buffer_to_buffer(self.buffer.into(), bigger.into(), self.capacity as u64 * ROW as u64);
                self.retired.push(std::mem::replace(&mut self.buffer, bigger));
                self.release(self.capacity, rows - self.capacity);
                self.capacity = rows;
            }
            let mut rows: Vec<[f32; 4]> = Vec::with_capacity(need);
            // (staging row, pool row, rows) per placed mesh.
            let mut copies: Vec<(usize, u32, u32)> = Vec::new();
            for r in new {
                let n = r.mesh.rows() as u32;
                let Some(at) = self.alloc(n) else {
                    self.warning = Some("mesh memory budget reached; some meshes are not drawn".into());
                    continue;
                };
                copies.push((rows.len(), at, n));
                r.mesh.write(&mut rows);
                self.entries.insert(r.mesh.key.clone(), Entry { at, rows: n, used: frame, animated: r.mesh.animated });
                self.uploads += 1;
            }
            let bytes = rows.len() * ROW;
            self.upload_bytes += bytes as u64;
            if bytes > 0 {
                if self.staging.as_ref().is_none_or(|s| s.1 < bytes) {
                    if let Some((b, _)) = self.staging.take() {
                        context.destroy_buffer(b);
                    }
                    let size = bytes.next_power_of_two();
                    self.staging = Some((context.create_buffer(gpu::BufferDesc { name: "motion mesh pool staging", size: size as u64, memory: gpu::Memory::Upload }), size));
                }
                let host = self.staging.as_ref().map(|s| s.0).ok_or("mesh pool staging")?;
                // SAFETY: the mapping holds at least `bytes`, and the frame
                // that last read it has finished.
                unsafe { std::ptr::copy_nonoverlapping(rows.as_ptr() as *const u8, host.data(), bytes) };
                context.sync_buffer(host);
                let mut pass = encoder.transfer("mesh pool upload");
                for (from, at, n) in copies {
                    pass.copy_buffer_to_buffer(host.at((from * ROW) as u64), self.buffer.at(at as u64 * ROW as u64), n as u64 * ROW as u64);
                }
            }
        }
        let b = f32::from_bits;
        for r in refs {
            let row = &mut mesh[r.row as usize];
            match self.entries.get(r.key()) {
                Some(e) => {
                    let verts = (r.mesh.mesh.positions.len() * PROWS) as u32;
                    row[0] = b(e.at);
                    row[1] = b(e.at + verts);
                }
                None => row[2] = b(0),
            }
        }
        Ok(())
    }

    /// Room for `n` rows: the first free range that fits, after evicting
    /// meshes this frame does not draw, longest unused first.
    fn alloc(&mut self, n: u32) -> Option<u32> {
        // Bigger than the whole pool: evicting would not make room.
        if n > self.capacity {
            return None;
        }
        loop {
            if let Some((&at, &len)) = self.free.iter().find(|(_, &len)| len >= n) {
                self.free.remove(&at);
                if len > n {
                    self.free.insert(at + n, len - n);
                }
                return Some(at);
            }
            let oldest = self.entries.iter().filter(|(_, e)| e.used < self.frame).min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone())?;
            let e = self.entries.remove(&oldest)?;
            self.release(e.at, e.rows);
            self.evictions += 1;
        }
    }

    /// Returns rows to the free ranges, merging with their neighbours.
    fn release(&mut self, mut at: u32, mut n: u32) {
        if let Some((&prev, &len)) = self.free.range(..at).next_back() {
            if prev + len == at {
                self.free.remove(&prev);
                at = prev;
                n += len;
            }
        }
        if let Some(len) = self.free.remove(&(at + n)) {
            n += len;
        }
        self.free.insert(at, n);
    }

    /// After the frame's GPU work: frees the buffers it was still reading.
    pub fn after_frame(&mut self, context: &gpu::Context) {
        self.in_flight = false;
        for b in self.retired.drain(..) {
            context.destroy_buffer(b);
        }
    }

    pub fn destroy(&mut self, context: &gpu::Context) {
        self.after_frame(context);
        context.destroy_buffer(self.buffer);
        if let Some((b, _)) = self.staging.take() {
            context.destroy_buffer(b);
        }
    }
}
