//! Vulkan compute path on `blade-graphics`, headless. One submission per
//! output frame: every motion-blur sub-sample's groups are rasterised and
//! blurred, its main target accumulated, then the result is resolved to
//! RGBA8 and read back.

use crate::font::{Fonts, ATLAS_SIZE};
use crate::images::Images;
use crate::list::{DisplayList, MeshDraw, Prim, Target, MAIN_TARGET, TILE};
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use gpu::ShaderData as _;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub const SHADER: &str = include_str!("motion.wgsl");
const MAX_BUFFER: usize = 512 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Pass {
    size: [u32; 4],
    dest: [u32; 4],
    background: [f32; 4],
    weight: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurPass {
    size: [u32; 4],
    sigma: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ResolvePass {
    size: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct VisPass {
    page: [f32; 4],
    atlas: [f32; 4],
    draw: [u32; 4],
}

#[derive(blade_macros::ShaderData)]
struct VisData {
    u_vis: VisPass,
    b_mesh: gpu::BufferPiece,
    b_prims: gpu::BufferPiece,
    b_groups: gpu::BufferPiece,
    b_pool: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct RasterData {
    u_pass: Pass,
    b_prims: gpu::BufferPiece,
    b_ranges: gpu::BufferPiece,
    b_indices: gpu::BufferPiece,
    b_stops: gpu::BufferPiece,
    b_atlas: gpu::BufferPiece,
    b_images: gpu::BufferPiece,
    b_groups: gpu::BufferPiece,
    b_segs: gpu::BufferPiece,
    b_footage: gpu::BufferPiece,
    b_ops: gpu::BufferPiece,
    b_mesh: gpu::BufferPiece,
    b_bins: gpu::BufferPiece,
    b_target: gpu::BufferPiece,
    t_vis_ids: gpu::TextureView,
    t_vis_depth: gpu::TextureView,
    b_vis: gpu::BufferPiece,
    b_pool: gpu::BufferPiece,
    t_shadow: gpu::TextureView,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShadowPass {
    /// A map's header row and its casters' first row.
    rows: [u32; 4],
}

#[derive(blade_macros::ShaderData)]
struct ShadowData {
    u_shadow: ShadowPass,
    b_mesh: gpu::BufferPiece,
    b_pool: gpu::BufferPiece,
}

/// The shadow maps a frame's lists ask the GPU for (`ShadowJob`), drawn
/// from the mesh pool into pages of one depth atlas the raster reads.
struct ShadowAtlas {
    texture: gpu::Texture,
    view: gpu::TextureView,
    width: u32,
    height: u32,
}

/// The atlas is this wide at most (maps are up to 2048 a side).
const SHADOW_ATLAS_WIDTH: u32 = 4096;

impl ShadowAtlas {
    fn new(context: &gpu::Context, width: u32, height: u32) -> Self {
        let (name, format) = ("shadow maps", gpu::TextureFormat::Depth32Float);
        let texture = context.create_texture(gpu::TextureDesc {
            name,
            format,
            size: gpu::Extent { width, height, depth: 1 },
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            usage: gpu::TextureUsage::TARGET | gpu::TextureUsage::RESOURCE,
            external: None,
        });
        let view = context.create_texture_view(texture, gpu::TextureViewDesc { name, format, dimension: gpu::ViewDimension::D2, subresources: &Default::default() });
        Self { texture, view, width, height }
    }
    fn destroy(&mut self, context: &gpu::Context) {
        context.destroy_texture_view(self.view);
        context.destroy_texture(self.texture);
    }
}

/// Where each job's page goes in an atlas `SHADOW_ATLAS_WIDTH` wide
/// (shelves, tallest first), and the atlas's size for them.
fn plan_shadows(jobs: &[crate::mesh3d::ShadowJob]) -> (Vec<[u32; 2]>, u32, u32) {
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    order.sort_by_key(|&k| std::cmp::Reverse(jobs[k].size));
    let mut at = vec![[0u32; 2]; jobs.len()];
    let (mut x, mut y, mut shelf, mut width) = (0u32, 0u32, 0u32, 16u32);
    for k in order {
        let n = jobs[k].size;
        if x + n > SHADOW_ATLAS_WIDTH {
            (x, y, shelf) = (0, y + shelf, 0);
        }
        at[k] = [x, y];
        x += n;
        width = width.max(x);
        shelf = shelf.max(n);
    }
    (at, width, (y + shelf).max(16))
}

/// Samples per pixel of the hardware visibility pass: the rasteriser's own
/// pattern is the standard 8x one.
const VIS_SAMPLES: u32 = 8;

#[cfg(test)]
thread_local! {
    /// Tests: run the visibility pass at 4x where the device has no 8x
    /// (Mesa's software Vulkan), so the pass, the mesh pool and what is
    /// built on them run there too. The raster then reads sample `s & 3`
    /// for its eight samples: edges are coarser, the rest is the same.
    pub(crate) static TEST_VIS_AT_4X: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The visibility pass's samples on a device offering `mask` (0: no pass).
fn vis_samples(mask: u32) -> u32 {
    if mask & VIS_SAMPLES != 0 {
        return VIS_SAMPLES;
    }
    #[cfg(test)]
    if mask & 4 != 0 && TEST_VIS_AT_4X.with(|t| t.get()) {
        return 4;
    }
    0
}
/// `b_vis` entry of a primitive without a page.
const VIS_NONE: i32 = -2147483647;

/// The hardware visibility atlas: per sample, the nearest mesh triangle's
/// id + 1 and its depth. Pages are laid out per target and the atlas is
/// reused target after target, so it holds one target's meshes at a time.
struct VisAtlas {
    ids: gpu::Texture,
    ids_view: gpu::TextureView,
    depth: gpu::Texture,
    depth_view: gpu::TextureView,
    width: u32,
    height: u32,
}

impl VisAtlas {
    fn new(context: &gpu::Context, width: u32, height: u32, samples: u32, target: bool) -> Self {
        let usage = if target { gpu::TextureUsage::TARGET | gpu::TextureUsage::RESOURCE } else { gpu::TextureUsage::RESOURCE };
        let make = |name: &str, format: gpu::TextureFormat| {
            let texture = context.create_texture(gpu::TextureDesc {
                name,
                format,
                size: gpu::Extent { width, height, depth: 1 },
                array_layer_count: 1,
                mip_level_count: 1,
                sample_count: samples,
                dimension: gpu::TextureDimension::D2,
                usage,
                external: None,
            });
            let view = context.create_texture_view(texture, gpu::TextureViewDesc { name, format, dimension: gpu::ViewDimension::D2, subresources: &Default::default() });
            (texture, view)
        };
        let (ids, ids_view) = make("mesh visibility ids", gpu::TextureFormat::R32Uint);
        let (depth, depth_view) = make("mesh visibility depth", gpu::TextureFormat::Depth32Float);
        Self { ids, ids_view, depth, depth_view, width, height }
    }
    fn destroy(&mut self, context: &gpu::Context) {
        context.destroy_texture_view(self.ids_view);
        context.destroy_texture_view(self.depth_view);
        context.destroy_texture(self.ids);
        context.destroy_texture(self.depth);
    }
}

/// The visibility pipelines: clear a page, draw opaque triangles (depth ≥,
/// later wins ties), draw see-through ones (strictly in front by TIE).
struct VisPipelines {
    clear: gpu::RenderPipeline,
    opaque: gpu::RenderPipeline,
    see: gpu::RenderPipeline,
}

/// A page of the atlas: the target rect it covers and its atlas corner.
#[derive(Clone, Copy)]
struct Page {
    rect: [u32; 4],
    at: [u32; 2],
}

/// One target's visibility work: its pages, and per draw (index into the
/// sample's draws) its page and its layer list's opaque page.
#[derive(Default)]
struct TargetVis {
    pages: Vec<Page>,
    /// Opaque pages: (run tag, page, draws in order).
    opaque: Vec<(u32, usize, Vec<usize>)>,
    /// See-through draws: (draw, own page, opaque page of its list).
    see: Vec<(usize, usize, Option<usize>)>,
    /// The target's draws (what the indices above refer to).
    draws: Vec<MeshDraw>,
}

/// A mesh primitive's triangles for the visibility pass: those with
/// records (cards), and its instances (pooled mesh row, triangles, canvas
/// bounds) as the pool completed their rows.
struct VisPrim {
    records: u32,
    instances: Vec<(u32, u32, [f32; 4])>,
}

impl VisPrim {
    fn new(prim: &Prim, mesh: &[[f32; 4]]) -> Self {
        let (records, n) = (prim.geom[2].to_bits(), prim.geom[3].to_bits() as usize);
        let base = (prim.geom2[0].to_bits() + records) as usize;
        let instances = (0..n)
            .map(|k| {
                let r = &mesh[base + k * crate::mesh3d::IROWS..];
                (r[13][0].to_bits(), r[13][2].to_bits(), r[19])
            })
            .collect();
        Self { records, instances }
    }

    /// The instanced draws (first instance, instances, vertices) that may
    /// touch target rect `rect` (primitive `prim` maps canvas px onto the
    /// target): runs of consecutive instances of one pooled mesh, in the
    /// batch's order, so ties resolve as one draw of the batch would. A
    /// glass bead's small backdrop draws only the instances behind it.
    fn draws(&self, prim: &Prim, rect: [u32; 4], out: &mut Vec<[u32; 3]>) {
        out.clear();
        let [ox, oy, fx, fy] = prim.fill;
        let r = rect.map(|v| v as f32);
        let mut last: Option<(u32, u32, u32)> = None;
        for (k, &(at, tris, b)) in self.instances.iter().enumerate() {
            let k = k as u32;
            let (x0, x1) = ((b[0] - ox) / fx, (b[2] - ox) / fx);
            let (y0, y1) = ((b[1] - oy) / fy, (b[3] - oy) / fy);
            // (Bounds that are not numbers miss nothing.)
            let misses = x0.min(x1) > r[2] + 1.0 || x0.max(x1) < r[0] - 1.0 || y0.min(y1) > r[3] + 1.0 || y0.max(y1) < r[1] - 1.0;
            if tris == 0 || misses {
                continue;
            }
            match out.last_mut() {
                Some(d) if last == Some((at, tris, k - 1)) => d[1] += 1,
                _ => out.push([k, 1, tris * 3]),
            }
            last = Some((at, tris, k));
        }
    }
}

/// Lays out one target's pages (shelves `width` wide) and fills `vis` (per
/// primitive offsets). Returns the work and the atlas height it needs.
fn plan_vis(draws: &[MeshDraw], prims: &[Prim], width: u32, vis: &mut [[i32; 4]]) -> (TargetVis, u32) {
    let mut out = TargetVis::default();
    let union = |a: [u32; 4], b: [u32; 4]| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
    for (i, d) in draws.iter().enumerate() {
        let tag = prims[d.prim as usize].kind[3];
        if tag & 1 == 0 {
            continue;
        }
        match out.opaque.iter_mut().find(|o| o.0 == tag) {
            Some(o) => {
                out.pages[o.1].rect = union(out.pages[o.1].rect, d.rect);
                o.2.push(i);
            }
            None => {
                out.pages.push(Page { rect: d.rect, at: [0, 0] });
                out.opaque.push((tag, out.pages.len() - 1, vec![i]));
            }
        }
    }
    for (i, d) in draws.iter().enumerate() {
        let tag = prims[d.prim as usize].kind[3];
        if tag & 1 != 0 {
            continue;
        }
        out.pages.push(Page { rect: d.rect, at: [0, 0] });
        let behind = out.opaque.iter().find(|o| o.0 == tag | 1).map(|o| o.1);
        out.see.push((i, out.pages.len() - 1, behind));
    }
    // Shelves, tallest first.
    let mut order: Vec<usize> = (0..out.pages.len()).collect();
    order.sort_by_key(|&k| std::cmp::Reverse(out.pages[k].rect[3] - out.pages[k].rect[1]));
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for k in order {
        let r = out.pages[k].rect;
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        if x + w > width {
            (x, y, shelf) = (0, y + shelf, 0);
        }
        out.pages[k].at = [x, y];
        x += w;
        shelf = shelf.max(h);
    }
    let offset = |p: &Page| [p.at[0] as i32 - p.rect[0] as i32, p.at[1] as i32 - p.rect[1] as i32];
    for (_, page, list) in &out.opaque {
        for &i in list {
            let o = offset(&out.pages[*page]);
            vis[draws[i].prim as usize] = [o[0], o[1], o[0], o[1]];
        }
    }
    for &(i, page, behind) in &out.see {
        let o = offset(&out.pages[page]);
        let b = behind.map_or([VIS_NONE, VIS_NONE], |b| offset(&out.pages[b]));
        vis[draws[i].prim as usize] = [o[0], o[1], b[0], b[1]];
    }
    (out, y + shelf)
}

#[derive(blade_macros::ShaderData)]
struct BlurData {
    u_blur: BlurPass,
    b_blur_src: gpu::BufferPiece,
    b_blur_dst: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ResolveData {
    u_resolve: ResolvePass,
    b_accum: gpu::BufferPiece,
    b_output: gpu::BufferPiece,
}

/// A device buffer with an optional host staging twin.
struct Buffer {
    device: gpu::Buffer,
    host: Option<gpu::Buffer>,
    capacity: usize,
    host_memory: Option<gpu::Memory>,
    name: &'static str,
    /// The most it may grow to, bytes.
    limit: usize,
}

impl Buffer {
    fn new(context: &gpu::Context, name: &'static str, size: usize, staged: bool) -> Self {
        Self::with_host(context, name, size, staged.then_some(gpu::Memory::Upload))
    }
    /// A buffer read back by the CPU: its twin is host-cached memory.
    /// Write-combined upload memory costs ~80 ms per 1080p frame to read,
    /// and `Shared` can land in BAR-mapped VRAM (~500 ms).
    fn readback(context: &gpu::Context, name: &'static str, size: usize) -> Self {
        Self::with_host(context, name, size, Some(gpu::Memory::Download))
    }
    fn with_host(context: &gpu::Context, name: &'static str, size: usize, host: Option<gpu::Memory>) -> Self {
        let capacity = size.max(256);
        Self {
            device: context.create_buffer(gpu::BufferDesc { name, size: capacity as u64, memory: gpu::Memory::Device }),
            host: host.map(|memory| context.create_buffer(gpu::BufferDesc { name, size: capacity as u64, memory })),
            capacity,
            host_memory: host,
            name,
            limit: MAX_BUFFER,
        }
    }
    fn ensure(&mut self, context: &gpu::Context, size: usize) -> Result<(), String> {
        if size > self.limit {
            return Err(format!("{} needs {size} bytes, over the GPU buffer limit", self.name));
        }
        if size > self.capacity {
            let mut next = Self::with_host(context, self.name, size.next_power_of_two().min(self.limit.max(size)), self.host_memory);
            next.limit = self.limit;
            self.destroy(context);
            *self = next;
        }
        Ok(())
    }
    fn upload(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, bytes: &[u8]) -> Result<(), String> {
        self.ensure(context, bytes.len().max(16))?;
        if bytes.is_empty() {
            return Ok(());
        }
        let host = self.host.expect("upload needs a staged buffer");
        // SAFETY: the mapping holds at least `capacity` ≥ bytes.len() bytes and
        // the previous submission finished before this frame started.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), host.data(), bytes.len()) };
        context.sync_buffer(host);
        encoder.transfer(self.name).copy_buffer_to_buffer(host.into(), self.device.into(), bytes.len() as u64);
        Ok(())
    }
    fn destroy(&mut self, context: &gpu::Context) {
        context.destroy_buffer(self.device);
        if let Some(h) = self.host {
            context.destroy_buffer(h);
        }
    }
}

/// Frames that may be on the GPU at once: one being traced, one at Open
/// Image Denoise, one being read back. Each has its own set of the
/// buffers a frame writes.
pub const FLIGHTS: usize = 3;

/// Command buffers the encoder cycles through: a traced frame is many
/// submissions (a band of rows each) that queue up behind each other.
const RING: usize = 64;

/// The work handed to the GPU. A frame is recorded into command buffers
/// that are submitted as they fill (`split`) without waiting for them;
/// the encoder has `RING` of them and takes one up again only once the
/// work it last held is done.
pub(crate) struct Queue {
    /// Submissions not known to have finished, oldest first.
    pending: std::collections::VecDeque<gpu::SyncPoint>,
    /// The last submission: when it is done, everything is.
    last: Option<gpu::SyncPoint>,
    /// A command buffer is being recorded.
    open: bool,
    /// LAHZA_TRACE_YIELD=1: wait for every submission before recording
    /// the next (the GPU is then idle between a trace's bands, for a
    /// desktop that stutters while a film is traced).
    wait_each: bool,
    /// Time spent waiting for room in the ring (not recording).
    waited: std::time::Duration,
}

impl Queue {
    fn new() -> Self {
        Self { pending: Default::default(), last: None, open: false, wait_each: std::env::var_os("LAHZA_TRACE_YIELD").is_some(), waited: Default::default() }
    }

    /// Starts recording, if nothing is being recorded. With `profile`,
    /// takes the pass timings of the work the command buffer last held.
    pub(crate) fn start(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, profile: Option<&mut crate::profile::Profile>) {
        if self.open {
            return;
        }
        let waiting = std::time::Instant::now();
        let mut waited = false;
        while self.pending.len() >= if self.wait_each { 1 } else { RING } {
            let Some(sync) = self.pending.pop_front() else { break };
            // (A lost device returns at once; the frame's own wait reports it.)
            context.wait_for(&sync, 600_000);
            waited = true;
        }
        if waited {
            self.waited += waiting.elapsed();
        }
        encoder.start();
        self.open = true;
        if let Some(p) = profile {
            if waited {
                p.stage("gpu frame › wait for GPU (queue full)", waiting);
            }
            p.add_gpu(encoder.timings());
        }
    }

    /// Submits what was recorded; returns when it will be done.
    pub(crate) fn submit(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder) -> gpu::SyncPoint {
        let sync = context.submit(encoder);
        self.open = false;
        self.pending.push_back(sync.clone());
        self.last = Some(sync.clone());
        sync
    }

    /// Submits what was recorded and goes on recording.
    pub(crate) fn split(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, profile: Option<&mut crate::profile::Profile>) {
        self.submit(context, encoder);
        self.start(context, encoder, profile);
    }

    /// Waits for everything submitted. False when it did not finish.
    fn idle(&mut self, context: &gpu::Context, timeout_ms: u32) -> bool {
        match &self.last {
            Some(sync) if !self.pending.is_empty() => {
                if !context.wait_for(sync, timeout_ms) {
                    return false;
                }
                self.pending.clear();
                true
            }
            _ => true,
        }
    }

    /// Ends a recording that failed part-way by submitting what there is
    /// of it (uploads and passes of a frame that will not be finished:
    /// harmless, and what they uploaded or built is then really there).
    fn discard(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder) {
        if self.open {
            self.submit(context, encoder);
        }
    }
}

/// The buffers one frame writes: frames in flight have a set each, so
/// recording a frame (the CPU fills the staging halves) does not touch
/// what another still reads on the GPU.
struct FrameSet {
    prims: Buffer,
    ranges: Buffer,
    indices: Buffer,
    stops: Buffer,
    segs: Buffer,
    footage: Buffer,
    ops: Buffer,
    mesh: Buffer,
    bins: Buffer,
    groups: Buffer,
    groups_tmp: Buffer,
    accum: Buffer,
    output: Buffer,
    vis_pages: Buffer,
    vis_atlas: VisAtlas,
    shadow_atlas: ShadowAtlas,
}

impl FrameSet {
    /// `vis`: the visibility pass's samples (0: the device has none).
    fn new(context: &gpu::Context, vis: u32) -> Self {
        Self {
            prims: Buffer::new(context, "motion prims", 1 << 16, true),
            ranges: Buffer::new(context, "motion ranges", 1 << 16, true),
            indices: Buffer::new(context, "motion indices", 1 << 16, true),
            stops: Buffer::new(context, "motion stops", 1 << 12, true),
            segs: Buffer::new(context, "motion segments", 1 << 12, true),
            footage: Buffer::new(context, "motion footage", 1 << 12, true),
            ops: Buffer::new(context, "motion post ops", 1 << 12, true),
            mesh: Buffer::new(context, "motion mesh", 1 << 12, true),
            bins: Buffer::new(context, "motion mesh bins", 1 << 12, true),
            groups: Buffer::new(context, "motion groups", 1 << 12, false),
            groups_tmp: Buffer::new(context, "motion groups tmp", 1 << 12, false),
            accum: Buffer::new(context, "motion accum", 1 << 12, false),
            output: Buffer::readback(context, "motion output", 1 << 12),
            vis_pages: Buffer::new(context, "motion visibility pages", 1 << 12, true),
            // Without the pass the raster shader still binds an atlas.
            vis_atlas: VisAtlas::new(context, 16, 16, if vis > 0 { vis } else { 4 }, vis > 0),
            // (Bound by every raster pass, drawn into only with jobs.)
            shadow_atlas: ShadowAtlas::new(context, 16, 16),
        }
    }

    fn destroy(&mut self, context: &gpu::Context) {
        for b in [
            &mut self.prims,
            &mut self.ranges,
            &mut self.indices,
            &mut self.stops,
            &mut self.segs,
            &mut self.footage,
            &mut self.ops,
            &mut self.mesh,
            &mut self.bins,
            &mut self.groups,
            &mut self.groups_tmp,
            &mut self.accum,
            &mut self.output,
            &mut self.vis_pages,
        ] {
            b.destroy(context);
        }
        self.vis_atlas.destroy(context);
        self.shadow_atlas.destroy(context);
    }
}

/// A sample's targets after `merge`: its main target, its groups and
/// where its meshes are drawn.
type Targets = Vec<(Target, Vec<crate::list::Group>, Vec<MeshDraw>)>;

/// What is left to record of a frame once its traced lists are through
/// Open Image Denoise: their filters and pictures, then the rasters that
/// composite them.
struct Rest {
    list: DisplayList,
    targets: Targets,
    plans: Vec<(Vec<TargetVis>, TargetVis)>,
    draws: std::collections::HashMap<u32, VisPrim>,
    slang: bool,
    /// Per traced list: the group texels its picture goes to (one place
    /// per sample).
    pictures: Vec<Vec<u32>>,
}

/// A frame on the GPU.
struct Flight {
    id: u64,
    set: usize,
    size: (u32, u32),
    /// Its last submission.
    sync: gpu::SyncPoint,
    /// Paths it traces (how long the GPU may take).
    paths: u64,
    /// Waiting for Open Image Denoise (`advance`).
    rest: Option<Rest>,
}

pub struct Gpu {
    context: gpu::Context,
    encoder: gpu::CommandEncoder,
    queue: Queue,
    raster: gpu::ComputePipeline,
    /// The raster pipeline with a project's Slang surfaces (`surfaces.rs`),
    /// by program key: built when the program changes, the last destroyed.
    slang_raster: Option<(u64, gpu::ComputePipeline)>,
    /// Hardware mesh visibility, where the device has 8x MSAA (else the
    /// raster shader finds the nearest triangles itself), and its samples.
    vis: Option<VisPipelines>,
    vis_samples: u32,
    /// Draws shadow maps from the mesh pool (with the visibility pass:
    /// lists for it name their maps as jobs).
    shadow: Option<gpu::RenderPipeline>,
    /// Generated meshes kept on the device across frames.
    pool: crate::pool::MeshPool,
    /// Shader layers' kernels.
    shaders: crate::shader::Pipelines,
    blur: gpu::ComputePipeline,
    resolve: gpu::ComputePipeline,
    /// The buffers frames write, one set per frame in flight (made when a
    /// frame first needs it: one frame at a time uses the first only).
    sets: Vec<FrameSet>,
    /// Frames on the GPU, oldest first.
    flights: Vec<Flight>,
    /// The next frame's number, and the last one finished.
    next: u64,
    completed: u64,
    atlas: Buffer,
    images: Buffer,
    atlas_generation: u64,
    images_generation: u64,
    /// Texels the image pool may hold on this device (`images::budget_for`).
    image_budget: usize,
    pub label: String,
    /// The path tracer, where the device has ray queries.
    tracer: Option<crate::pt::Tracer>,
    pub(crate) trace_error: Option<String>,
    /// With profiling: this frame's stages and GPU pass timestamps (the
    /// renderer takes them after each frame).
    pub(crate) profile: Option<crate::profile::Profile>,
}

/// Concatenates sub-sample display lists into one upload. Returns the
/// merged buffers and each sample's targets (main, groups) re-based.
///
/// Group texels: a sample's groups are read only by its own passes. When
/// each sample is drawn in full before the next (`separate` false), every
/// sample draws its groups in the same texels (eight sub-frames of a
/// full-frame transition would otherwise need eight frames' worth, past
/// the buffer limit). When the samples' groups are all drawn first and
/// their main targets afterwards (traced lists, which the samples share),
/// each sample keeps its own (`separate`).
pub fn merge(samples: &[DisplayList], separate: bool) -> (DisplayList, Targets) {
    let mut out = DisplayList::default();
    let mut targets = Vec::new();
    for list in samples {
        let prim_base = out.prims.len() as u32;
        let index_base = out.indices.len() as u32;
        let range_base = (out.ranges.len() / 2) as u32;
        let stop_base = (out.stops.len() / 2) as f32;
        let texel_base = if separate { out.group_texels } else { 0 };
        let seg_base = out.segments.len() as u32;
        let ops_base = out.ops.len() as u32;
        let mesh_base = out.mesh.len() as u32;
        let bins_base = out.bins.len() as u32;
        let trace_base = out.traces.len() as u32;
        let shader_base = out.shaders.len() as u32;
        out.shaders.extend_from_slice(&list.shaders);
        let id_base = out.mesh_ids;
        out.mesh_ids += list.mesh_ids;
        out.shadows.extend(list.shadows.iter().map(|j| crate::mesh3d::ShadowJob { header: j.header + mesh_base, casters: j.casters + mesh_base, ..*j }));
        out.pool.extend(list.pool.iter().map(|r| crate::mesh3d::PoolRef { row: r.row + mesh_base, ..r.clone() }));
        out.traces.extend(list.traces.iter().map(|t| {
            let mut t = t.clone();
            for c in &mut t.cards {
                c.picture.offset += texel_base;
            }
            t
        }));
        for p in &list.prims {
            let mut p: Prim = *p;
            let rebase = |v: f32, by: u32| f32::from_bits(v.to_bits() + by);
            if p.kind[0] == crate::list::PRIM_SHAPE && p.paint[1] > 0.5 {
                p.paint[2] += stop_base;
                if p.paint[1] as u32 == crate::list::PAINT_ALONG {
                    p.paint2[0] = rebase(p.paint2[0], seg_base);
                }
            }
            match p.kind[0] {
                crate::list::PRIM_MESH => {
                    p.geom[0] = rebase(p.geom[0], bins_base);
                    // Triangle ids of the visibility pass (lists for it
                    // number them; geom[1] is a tile otherwise).
                    if list.mesh_ids > 0 {
                        p.geom[1] = rebase(p.geom[1], id_base);
                    }
                    p.geom2 = p.geom2.map(|v| rebase(v, mesh_base));
                    if p.paint[3] > 0.5 {
                        p.paint[0] = rebase(p.paint[0], texel_base);
                    }
                    // Cards' pictures are relative to this base.
                    p.stroke[2] = rebase(p.stroke[2], texel_base);
                }
                crate::list::PRIM_IMAGE if p.kind[1] == 1 => p.geom2[0] = rebase(p.geom2[0], texel_base),
                crate::list::PRIM_MATTE => {
                    p.geom2[0] = rebase(p.geom2[0], texel_base);
                    p.geom2[1] = rebase(p.geom2[1], texel_base);
                }
                crate::list::PRIM_SHAPE if p.kind[1] == crate::list::SHAPE_PATH => p.geom2[0] = rebase(p.geom2[0], seg_base),
                _ => {}
            }
            if (p.kind[0] == crate::list::PRIM_IMAGE || p.kind[0] == crate::list::PRIM_MATTE) && p.paint[3].to_bits() > 0 {
                p.paint[2] = rebase(p.paint[2], ops_base);
            }
            out.prims.push(p);
        }
        out.segments.extend_from_slice(&list.segments);
        out.ops.extend_from_slice(&list.ops);
        out.mesh.extend_from_slice(&list.mesh);
        out.bins.extend_from_slice(&list.bins);
        out.indices.extend(list.indices.iter().map(|i| i + prim_base));
        for pair in list.ranges.chunks_exact(2) {
            out.ranges.push(pair[0] + index_base);
            out.ranges.push(pair[1]);
        }
        out.stops.extend_from_slice(&list.stops);
        out.group_texels = if separate { out.group_texels + list.group_texels } else { out.group_texels.max(list.group_texels) };
        let mut main = list.main();
        main.range_base += range_base;
        let groups = list
            .groups
            .iter()
            .map(|g| {
                let mut g = *g;
                g.target.range_base += range_base;
                g.offset += texel_base;
                g.trace = g.trace.map(|i| i + trace_base);
                g.shader = g.shader.map(|i| i + shader_base);
                g
            })
            .collect();
        let draws = list.mesh_draws.iter().map(|d| MeshDraw { prim: d.prim + prim_base, ..*d }).collect();
        targets.push((main, groups, draws));
    }
    out.main = samples.first().and_then(|s| s.main);
    (out, targets)
}

/// Whether the samples of a frame can share their traced pictures: every
/// sample has the same traced lists, on the same texels (a moving camera
/// moves the scene under them, not them), and the samples' groups fit the
/// buffer side by side. Each traced list is then traced sample after
/// sample into one picture and filtered once. (One sample always can.)
fn shares_traces(samples: &[DisplayList]) -> bool {
    let traced = |s: &DisplayList| -> Vec<(u32, u32, [f32; 2], [f32; 2], [f32; 9], bool)> {
        s.groups.iter().filter_map(|g| g.trace.map(|i| (g, &s.traces[i as usize]))).map(|(g, t)| (g.target.width, g.target.height, t.origin, t.factor, t.root, t.options.denoise)).collect()
    };
    let Some(first) = samples.first().map(traced) else { return false };
    let texels: u64 = samples.iter().map(|s| s.group_texels as u64).sum();
    !first.is_empty() && samples.iter().all(|s| traced(s) == first) && texels * 16 <= MAX_BUFFER as u64
}

impl Gpu {
    /// Opens a headless Vulkan context. Software rasterisers (llvmpipe) are
    /// rejected unless `allow_software`; the CPU path is faster than them.
    /// `profile` times every GPU pass (`profile`).
    pub fn new(allow_software: bool, profile: bool) -> Result<Self, String> {
        // SAFETY: headless context; no external handles or shared ownership.
        let context = catch_unwind(AssertUnwindSafe(|| unsafe { gpu::Context::init(gpu::ContextDesc { timing: profile, pipeline_cache: pipeline_cache_path(), ..Default::default() }) }))
            .map_err(|_| "Vulkan initialisation panicked".to_string())?
            .map_err(|e| format!("no Vulkan device: {e:?}"))?;
        let info = context.device_information().clone();
        if info.is_software_emulated && !allow_software {
            return Err(format!("only a software Vulkan device is available ({})", info.device_name));
        }
        let shader = context.try_create_shader(gpu::ShaderDesc { source: SHADER })?;
        let raster = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "motion raster",
            data_layouts: &[&RasterData::layout()],
            compute: shader.at("raster"),
        });
        let caps = context.capabilities();
        let vis_samples = vis_samples(caps.sample_count_mask);
        let vis = (vis_samples > 0).then(|| {
            let make = |name: &str, compare: gpu::CompareFunction| {
                context.create_render_pipeline(gpu::RenderPipelineDesc {
                    name,
                    data_layouts: &[&VisData::layout()],
                    vertex: shader.at("vis_vertex"),
                    vertex_fetches: &[],
                    primitive: gpu::PrimitiveState { topology: gpu::PrimitiveTopology::TriangleList, ..Default::default() },
                    depth_stencil: Some(gpu::DepthStencilState {
                        format: gpu::TextureFormat::Depth32Float,
                        depth_write_enabled: true,
                        depth_compare: compare,
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    fragment: Some(shader.at("vis_fragment")),
                    color_targets: &[gpu::TextureFormat::R32Uint.into()],
                    multisample_state: gpu::MultisampleState { sample_count: vis_samples, ..Default::default() },
                })
            };
            VisPipelines {
                clear: make("mesh visibility clear", gpu::CompareFunction::Always),
                opaque: make("mesh visibility opaque", gpu::CompareFunction::GreaterEqual),
                see: make("mesh visibility see-through", gpu::CompareFunction::Greater),
            }
        });
        let shadow = vis.is_some().then(|| {
            context.create_render_pipeline(gpu::RenderPipelineDesc {
                name: "shadow maps",
                data_layouts: &[&ShadowData::layout()],
                vertex: shader.at("shadow_vertex"),
                vertex_fetches: &[],
                primitive: gpu::PrimitiveState { topology: gpu::PrimitiveTopology::TriangleList, ..Default::default() },
                // The nearest surface to the light: the greatest closeness.
                depth_stencil: Some(gpu::DepthStencilState {
                    format: gpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: gpu::CompareFunction::Greater,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                fragment: None,
                color_targets: &[],
                multisample_state: Default::default(),
            })
        });
        let blur = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "motion blur",
            data_layouts: &[&BlurData::layout()],
            compute: shader.at("blur"),
        });
        let resolve = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "motion resolve",
            data_layouts: &[&ResolveData::layout()],
            compute: shader.at("resolve"),
        });
        let encoder = context.create_command_encoder(gpu::CommandEncoderDesc { name: "motion", buffer_count: RING as u32 });
        let label = format!("GPU · {}", info.device_name);
        let (tracer, trace_error) = if !crate::pt::supported(&context) {
            (None, Some(format!("{} has no ray queries", info.device_name)))
        } else {
            match crate::pt::Tracer::new(&context, profile) {
                Ok(t) => (Some(t), None),
                Err(e) => (None, Some(format!("path tracer: {e}"))),
            }
        };
        // Every pipeline exists now: keep them for the next launch.
        context.save_pipeline_cache();
        // (A software device's "own memory" is the machine's.)
        let image_budget = if info.is_software_emulated { crate::images::MAX_TEXELS } else { crate::images::budget_for(info.device_memory, info.max_storage_buffer_range) };
        let mut images = Buffer::new(&context, "motion images", 1 << 12, true);
        images.limit = images.limit.max(image_budget * 4);
        Ok(Self {
            sets: vec![FrameSet::new(&context, vis_samples)],
            flights: Vec::new(),
            next: 1,
            completed: 0,
            atlas: Buffer::new(&context, "motion atlas", (ATLAS_SIZE * ATLAS_SIZE) as usize, true),
            images,
            atlas_generation: 0,
            images_generation: 0,
            image_budget,
            pool: crate::pool::MeshPool::new(&context),
            shaders: Default::default(),
            queue: Queue::new(),
            context,
            encoder,
            raster,
            slang_raster: None,
            vis,
            vis_samples,
            shadow,
            blur,
            resolve,
            label,
            tracer,
            trace_error,
            profile: profile.then(crate::profile::Profile::default),
        })
    }

    /// Whether meshes go through the hardware visibility pass (their
    /// display lists then hold shared triangle records).
    pub fn has_vis(&self) -> bool {
        self.vis.is_some()
    }

    /// Texels the image pool may hold on this device.
    pub fn image_budget(&self) -> usize {
        self.image_budget
    }

    /// Whether this GPU can path-trace.
    pub fn can_trace(&self) -> bool {
        self.tracer.is_some()
    }

    /// Why some meshes were not drawn (the mesh pool's budget), if so.
    pub(crate) fn pool_warning(&self) -> Option<String> {
        self.pool.warning.clone()
    }

    /// Why traced frames used the built-in denoiser instead of OIDN, if so.
    pub(crate) fn denoise_warning(&self) -> Option<String> {
        self.tracer.as_ref().and_then(|t| t.denoise_warning())
    }

    /// Vertices the path tracer holds for its geometry (tests).
    #[cfg(test)]
    pub(crate) fn trace_cached_vertices(&self) -> usize {
        self.tracer.as_ref().map_or(0, |t| t.cached_vertices())
    }

    /// Tests: the tracer's kept buffers, and a stand-in for OIDN.
    #[cfg(test)]
    pub(crate) fn tracer_for_tests(&mut self) -> Option<&mut crate::pt::Tracer> {
        self.tracer.as_mut()
    }

    /// Tests: whether the device exports memory (the denoiser can share it).
    #[cfg(test)]
    pub(crate) fn exports_memory(&self) -> bool {
        self.context.has_external_memory()
    }

    /// Frames begun and not finished.
    pub fn in_flight(&self) -> usize {
        self.flights.len()
    }

    /// Whether traced frames wait for Open Image Denoise between `begin`
    /// and `finish` (a third frame in flight then keeps the GPU busy).
    pub fn denoises_between(&mut self) -> bool {
        self.tracer.as_mut().is_some_and(|t| t.wants_oidn())
    }

    /// Renders weighted sub-samples (weights sum to 1) to straight RGBA8.
    pub fn render(&mut self, samples: &[DisplayList], fonts: &Fonts, images: &Images, footage: &[u32]) -> Result<Vec<u8>, String> {
        let id = self.begin(samples, fonts, images, footage)?;
        self.finish(id)
    }

    /// Records a frame and hands it to the GPU; `finish` waits for it and
    /// returns its pixels (in the order frames were begun). Up to
    /// `FLIGHTS` frames may be begun before the first is finished: the
    /// next frame's lists are then built and recorded while the GPU works
    /// on this one. On an error the frame is not in flight.
    pub fn begin(&mut self, samples: &[DisplayList], fonts: &Fonts, images: &Images, footage: &[u32]) -> Result<u64, String> {
        let set = (0..FLIGHTS).find(|s| self.flights.iter().all(|f| f.set != *s)).ok_or("too many frames in flight")?;
        while self.sets.len() <= set {
            self.sets.push(FrameSet::new(&self.context, self.vis_samples));
        }
        let id = self.next;
        self.next += 1;
        match self.record(id, set, samples, fonts, images, footage) {
            Ok(flight) => {
                self.flights.push(flight);
                Ok(id)
            }
            Err(e) => {
                self.queue.discard(&self.context, &mut self.encoder);
                Err(e)
            }
        }
    }

    /// Waits for the frames in flight to leave the GPU (they stay to be
    /// finished): before something they share is written again.
    fn drain(&mut self, why: &str) -> Result<(), String> {
        if self.flights.is_empty() {
            return Ok(());
        }
        let waiting = std::time::Instant::now();
        let paths: u64 = self.flights.iter().map(|f| f.paths).sum();
        if !self.queue.idle(&self.context, frame_timeout_ms(paths)) {
            return Err("GPU timed out".into());
        }
        if let Some(p) = self.profile.as_mut() {
            p.stage(&format!("gpu frame › wait for GPU ({why})"), waiting);
        }
        Ok(())
    }

    fn record(&mut self, id: u64, set: usize, samples: &[DisplayList], fonts: &Fonts, images: &Images, footage: &[u32]) -> Result<Flight, String> {
        let main = samples.first().ok_or("nothing to render")?.main();
        let (w, h) = (main.width, main.height);
        if w.div_ceil(TILE) > 65535 || h.div_ceil(TILE) > 65535 {
            return Err("canvas exceeds the GPU dispatch limit".into());
        }
        let recording = std::time::Instant::now();
        let waited = self.queue.waited;
        // Traced lists the samples share are traced sample after sample
        // into one picture; the samples' own groups then sit side by side.
        let shared = shares_traces(samples);
        let (mut list, targets) = merge(samples, shared);
        if let Some(p) = self.profile.as_mut() {
            p.stage("gpu frame › record + upload › merge samples", recording);
        }
        let slang = samples.iter().find_map(|s| s.surfaces.as_ref());
        if let Some(program) = slang {
            self.surface_raster(program)?;
        }
        let slang = slang.is_some();
        // Hardware visibility: every target's pages laid out up front (one
        // atlas, reused target by target), per primitive where its page is.
        let use_vis = self.vis.is_some();
        let mut vis_table = vec![[VIS_NONE; 4]; list.prims.len().max(1)];
        let mut plans: Vec<(Vec<TargetVis>, TargetVis)> = Vec::new();
        let mut atlas_height = 16u32;
        let atlas_width = targets.iter().flat_map(|(m, g, _)| std::iter::once(m.width).chain(g.iter().map(|g| g.target.width))).max().unwrap_or(16).clamp(16, 8192);
        if use_vis {
            for (_, groups, draws) in &targets {
                let mut plan = |which: u32| {
                    let mine: Vec<MeshDraw> = draws.iter().filter(|d| d.target == which).copied().collect();
                    let (mut p, h) = plan_vis(&mine, &list.prims, atlas_width, &mut vis_table);
                    atlas_height = atlas_height.max(h);
                    // Draw indices refer to `mine`: keep it with the plan.
                    p.draws = mine;
                    p
                };
                let group_plans = (0..groups.len() as u32).map(&mut plan).collect();
                let main_plan = plan(MAIN_TARGET);
                plans.push((group_plans, main_plan));
            }
            if atlas_height > 16384 {
                return Err(format!("meshes need a {atlas_width} x {atlas_height} visibility atlas, over the GPU texture limit"));
            }
            // Grow in steps; shrink once far too big.
            let need = atlas_height.next_multiple_of(256);
            let atlas = &mut self.sets[set].vis_atlas;
            if atlas.width != atlas_width || need > atlas.height || need * 4 < atlas.height {
                atlas.destroy(&self.context);
                *atlas = VisAtlas::new(&self.context, atlas_width, need, self.vis_samples, true);
            }
        }
        // What the frames in flight share is written only while the GPU
        // is idle: new glyphs, new images, new meshes in the pool.
        if self.atlas_generation != fonts.generation || self.images_generation != images.generation || self.pool.changes(&list.pool) {
            self.drain("shared uploads")?;
        }
        if let Some(t) = self.tracer.as_mut() {
            t.begin_frame(&self.context, id, self.completed, set);
        }
        self.queue.start(&self.context, &mut self.encoder, self.profile.as_mut());
        let ctx = &self.context;
        let mut in_tracer = std::time::Duration::ZERO;
        // Instances' meshes into the pool (where they are goes into their
        // rows), then each mesh primitive's visibility draws.
        let pooling = std::time::Instant::now();
        self.pool.resolve(ctx, &mut self.encoder, id, &list.pool, &mut list.mesh)?;
        let mut draws: std::collections::HashMap<u32, VisPrim> = std::collections::HashMap::new();
        if use_vis {
            for (_, _, ds) in &targets {
                for d in ds {
                    draws.entry(d.prim).or_insert_with(|| VisPrim::new(&list.prims[d.prim as usize], &list.mesh));
                }
            }
        }
        if let Some(p) = self.profile.as_mut() {
            p.add("gpu frame › mesh pool", pooling.elapsed());
        }
        // Shadow maps the lists ask for: a page each in the frame's atlas
        // (where it is goes into the map's header, before the upload).
        let (shadow_at, shadow_w, shadow_h) = plan_shadows(&list.shadows);
        if !list.shadows.is_empty() {
            if self.shadow.is_none() {
                return Err("a list with shadow maps for the GPU, but no shadow pass".into());
            }
            if shadow_h > 16384 {
                return Err(format!("this frame's shadow maps need a {shadow_w} x {shadow_h} atlas, over the GPU texture limit"));
            }
            for (job, at) in list.shadows.iter().zip(&shadow_at) {
                list.mesh[job.header as usize + 6] = [at[0] as f32, at[1] as f32, 0.0, 0.0];
            }
            // Grown as needed; let go of once far too big.
            let atlas = &mut self.sets[set].shadow_atlas;
            if shadow_w > atlas.width || shadow_h > atlas.height || shadow_w * shadow_h * 8 < atlas.width * atlas.height {
                atlas.destroy(&self.context);
                *atlas = ShadowAtlas::new(&self.context, shadow_w.next_multiple_of(256), shadow_h.next_multiple_of(256));
            }
        }
        let uploading = std::time::Instant::now();
        let s = &mut self.sets[set];
        s.prims.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.prims))?;
        s.ranges.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.ranges))?;
        s.indices.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.indices))?;
        s.stops.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.stops))?;
        s.segs.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.segments))?;
        // Footage frames change every frame and upload every time.
        s.footage.upload(ctx, &mut self.encoder, bytemuck::cast_slice(footage))?;
        s.ops.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.ops))?;
        s.mesh.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.mesh))?;
        // Tile bins are only read when the raster shader finds the nearest
        // triangles itself.
        if !use_vis {
            s.bins.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.bins))?;
        }
        s.vis_pages.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&vis_table))?;
        if self.atlas_generation != fonts.generation {
            self.atlas.upload(ctx, &mut self.encoder, &fonts.atlas)?;
            self.atlas_generation = fonts.generation;
        }
        if self.images_generation != images.generation {
            self.images.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&images.texels))?;
            self.images_generation = images.generation;
        }
        let group_bytes = list.group_texels as usize * 16;
        s.groups.ensure(ctx, group_bytes)?;
        s.groups_tmp.ensure(ctx, group_bytes)?;
        s.accum.ensure(ctx, (w * h) as usize * 16)?;
        s.output.ensure(ctx, (w * h) as usize * 4)?;
        if let Some(p) = self.profile.as_mut() {
            p.stage("gpu frame › record + upload › uploads", uploading);
        }
        self.record_shadows(set, &list, &shadow_at);
        let (groups_buf, images_buf) = (self.sets[set].groups.device, self.images.device);
        // Paths traced this frame: how long the GPU may take.
        let mut paths = 0u64;
        let parts = targets.len() as u32;
        // Per traced list: where its picture goes (one place per sample
        // that shares it; else it is drawn as soon as it is traced).
        let mut pictures: Vec<Vec<u32>> = Vec::new();
        for (i, (_, groups, _)) in targets.iter().enumerate() {
            let mut slot = 0u32;
            for (gi, g) in groups.iter().enumerate() {
                let t = g.target;
                if let Some(ti) = g.trace {
                    let trace = &list.traces[ti as usize];
                    paths += t.width as u64 * t.height as u64 * trace.options.spp.max(1) as u64;
                    let entered = std::time::Instant::now();
                    let waited_in = self.queue.waited;
                    let tracer = self.tracer.as_mut().ok_or("a traced group, but no path tracer")?;
                    if shared {
                        tracer.part(&self.context, &mut self.encoder, &mut self.queue, trace, slot, i as u32, parts, (t.width, t.height), groups_buf, images_buf, self.profile.as_mut())?;
                        if i == 0 {
                            pictures.push(Vec::new());
                        }
                        pictures[slot as usize].push(g.offset);
                        slot += 1;
                    } else {
                        // On its own, start to finish: the next trace
                        // takes over its lighting and guides.
                        tracer.part(&self.context, &mut self.encoder, &mut self.queue, trace, 0, 0, 1, (t.width, t.height), groups_buf, images_buf, self.profile.as_mut())?;
                        let oidn = tracer.wants_oidn();
                        tracer.settle(&self.context, &mut self.encoder, 0, groups_buf, images_buf, oidn);
                        if tracer.awaits_denoise(0) {
                            in_tracer += entered.elapsed().saturating_sub(self.queue.waited - waited_in);
                            let waiting = std::time::Instant::now();
                            let sync = self.queue.submit(&self.context, &mut self.encoder);
                            let done = self.context.wait_for(&sync, frame_timeout_ms(paths));
                            self.queue.start(&self.context, &mut self.encoder, self.profile.as_mut());
                            if !done {
                                return Err("GPU timed out".into());
                            }
                            let denoising = std::time::Instant::now();
                            if let Some(p) = self.profile.as_mut() {
                                p.add("gpu frame › wait for GPU (before OIDN)", denoising - waiting);
                            }
                            let tracer = self.tracer.as_mut().ok_or("no path tracer")?;
                            tracer.denoise(&self.context, set, 0);
                            if let Some(p) = self.profile.as_mut() {
                                p.stage("gpu frame › OIDN denoise", denoising);
                            }
                        }
                        let entered = std::time::Instant::now();
                        let tracer = self.tracer.as_mut().ok_or("no path tracer")?;
                        tracer.post(&self.context, &mut self.encoder, 0, &[g.offset], groups_buf, images_buf);
                        in_tracer += entered.elapsed();
                        continue;
                    }
                    in_tracer += entered.elapsed().saturating_sub(self.queue.waited - waited_in);
                    continue;
                }
                // A shader layer's kernel paints it; one that did not build
                // (the project says why) is rasterised empty: transparent.
                if let Some(si) = g.shader {
                    if self.shaders.dispatch(&self.context, &mut self.encoder, &list.shaders[si as usize], g, groups_buf)? {
                        continue;
                    }
                }
                if let Some((group_plans, _)) = plans.get(i) {
                    self.record_vis(set, &group_plans[gi], &draws, &list.prims);
                }
                self.record_group(set, g, use_vis, slang);
            }
            if !shared {
                self.record_main(set, i, &targets, plans.get(i).map(|p| &p.1), &draws, &list.prims, use_vis, slang);
            }
        }
        let mut rest = shared.then_some(Rest { list, targets, plans, draws, slang, pictures });
        let mut waits = false;
        if let Some(r) = &rest {
            // The lists' parts are all traced: on to their filters.
            let tracer = self.tracer.as_mut().ok_or("a traced group, but no path tracer")?;
            let oidn = tracer.wants_oidn();
            for slot in 0..r.pictures.len() as u32 {
                tracer.settle(&self.context, &mut self.encoder, slot, groups_buf, images_buf, oidn);
                waits |= tracer.awaits_denoise(slot);
            }
        }
        if !waits {
            if let Some(r) = rest.take() {
                self.record_rest(set, &r);
            }
            self.record_end(set, (w, h), parts);
        }
        if let Some(p) = self.profile.as_mut() {
            // (Not the tracer's own stages, nor waits for the GPU.)
            p.add("gpu frame › record + upload", recording.elapsed().saturating_sub(in_tracer + (self.queue.waited - waited)));
        }
        let sync = self.queue.submit(&self.context, &mut self.encoder);
        Ok(Flight { id, set, size: (w, h), sync, paths, rest })
    }

    /// Rasterises group `g` of the frame in `set` (and blurs it).
    fn record_group(&mut self, set: usize, g: &crate::list::Group, use_vis: bool, slang: bool) {
        let t = g.target;
        let s = &self.sets[set];
        let data = self.raster_data(
            set,
            Pass { size: [t.width, t.height, t.tiles_x, t.range_base], dest: [g.offset, 0, use_vis as u32, self.vis_samples.saturating_sub(1)], background: [0.0; 4], weight: [1.0, 0.0, 0.0, 0.0] },
            // Outer groups sample inner ones in other regions of the same
            // buffer; regions never overlap.
            s.groups.device,
        );
        {
            let mut pass = self.encoder.compute("group raster");
            let mut pc = pass.with(match &self.slang_raster {
                Some((_, p)) if slang => p,
                _ => &self.raster,
            });
            pc.bind(0, &data);
            pc.dispatch([t.tiles_x, t.tiles_y, 1]);
        }
        if g.sigma <= 0.0 {
            // Same as the CPU path: no blur pass (σ = 0 would make the
            // Gaussian weights NaN).
            return;
        }
        let radius = (g.sigma * 3.0).ceil();
        let norm: f32 = (-(radius as i32)..=radius as i32).map(|d| (-(d * d) as f32 / (2.0 * g.sigma * g.sigma)).exp()).sum();
        for (horizontal, src, dst) in [(1u32, s.groups.device, s.groups_tmp.device), (0, s.groups_tmp.device, s.groups.device)] {
            let data = BlurData {
                u_blur: BlurPass { size: [t.width, t.height, g.offset, horizontal], sigma: [g.sigma, radius, 1.0 / norm, if g.clamp { 1.0 } else { 0.0 }] },
                b_blur_src: src.into(),
                b_blur_dst: dst.into(),
            };
            let mut pass = self.encoder.compute("group blur");
            let mut pc = pass.with(&self.blur);
            pc.bind(0, &data);
            pc.dispatch([t.width.div_ceil(16), t.height.div_ceil(16), 1]);
        }
    }

    /// Rasterises sample `i`'s main target into the frame's sum.
    #[allow(clippy::too_many_arguments)]
    fn record_main(&mut self, set: usize, i: usize, targets: &Targets, plan: Option<&TargetVis>, draws: &std::collections::HashMap<u32, VisPrim>, prims: &[Prim], use_vis: bool, slang: bool) {
        if let Some(plan) = plan {
            self.record_vis(set, plan, draws, prims);
        }
        let main = targets[i].0;
        let data = self.raster_data(
            set,
            Pass {
                size: [main.width, main.height, main.tiles_x, main.range_base],
                dest: [0, if i == 0 { 2 } else { 1 }, use_vis as u32, self.vis_samples.saturating_sub(1)],
                background: main.background,
                weight: [1.0 / targets.len() as f32, 0.0, 0.0, 0.0],
            },
            self.sets[set].accum.device,
        );
        let mut pass = self.encoder.compute("main raster");
        let mut pc = pass.with(match &self.slang_raster {
            Some((_, p)) if slang => p,
            _ => &self.raster,
        });
        pc.bind(0, &data);
        pc.dispatch([main.tiles_x, main.tiles_y, 1]);
    }

    /// The traced lists' filters and pictures, then every sample's main
    /// target over them.
    fn record_rest(&mut self, set: usize, r: &Rest) {
        let (groups_buf, images_buf) = (self.sets[set].groups.device, self.images.device);
        if let Some(tracer) = self.tracer.as_mut() {
            tracer.resume(set);
            for (slot, offsets) in r.pictures.iter().enumerate() {
                tracer.post(&self.context, &mut self.encoder, slot as u32, offsets, groups_buf, images_buf);
            }
        }
        let use_vis = self.vis.is_some();
        for i in 0..r.targets.len() {
            self.record_main(set, i, &r.targets, r.plans.get(i).map(|p| &p.1), &r.draws, &r.list.prims, use_vis, r.slang);
        }
    }

    /// The frame's sum to RGBA8 and into the host's hands.
    fn record_end(&mut self, set: usize, (w, h): (u32, u32), samples: u32) {
        let s = &self.sets[set];
        {
            let data = ResolveData {
                u_resolve: ResolvePass { size: [w, h, samples, 0] },
                b_accum: s.accum.device.into(),
                b_output: s.output.device.into(),
            };
            let mut pass = self.encoder.compute("resolve");
            let mut pc = pass.with(&self.resolve);
            pc.bind(0, &data);
            pc.dispatch([w.div_ceil(16), h.div_ceil(16), 1]);
        }
        let host = s.output.host.unwrap();
        self.encoder.transfer("readback").copy_buffer_to_buffer(s.output.device.into(), host.into(), (w * h * 4) as u64);
    }

    fn raster_data(&self, set: usize, pass: Pass, target: gpu::Buffer) -> RasterData {
        let s = &self.sets[set];
        RasterData {
            u_pass: pass,
            b_prims: s.prims.device.into(),
            b_ranges: s.ranges.device.into(),
            b_indices: s.indices.device.into(),
            b_stops: s.stops.device.into(),
            b_atlas: self.atlas.device.into(),
            b_images: self.images.device.into(),
            b_groups: s.groups.device.into(),
            b_segs: s.segs.device.into(),
            b_footage: s.footage.device.into(),
            b_ops: s.ops.device.into(),
            b_mesh: s.mesh.device.into(),
            b_bins: s.bins.device.into(),
            b_target: target.into(),
            t_vis_ids: s.vis_atlas.ids_view,
            t_vis_depth: s.vis_atlas.depth_view,
            b_vis: s.vis_pages.device.into(),
            b_pool: self.pool.buffer.into(),
            t_shadow: s.shadow_atlas.view,
        }
    }

    /// Draws the frame's shadow maps: each map's casters, from the mesh
    /// pool, into its page of the set's atlas (cleared to "nothing").
    fn record_shadows(&mut self, set: usize, list: &DisplayList, at: &[[u32; 2]]) {
        let Some(pipeline) = self.shadow.as_ref() else { return };
        if list.shadows.is_empty() {
            return;
        }
        let s = &self.sets[set];
        let (mesh, pool) = (s.mesh.device, self.pool.buffer);
        let mut pass = self.encoder.render(
            "shadow maps",
            gpu::RenderTargetSet {
                colors: &[],
                depth_stencil: Some(gpu::RenderTarget { view: s.shadow_atlas.view, init_op: gpu::InitOp::Clear(gpu::TextureColor::TransparentBlack), finish_op: gpu::FinishOp::Store }),
            },
        );
        for (job, at) in list.shadows.iter().zip(at) {
            let mut pc = pass.with(pipeline);
            pc.set_viewport(&gpu::Viewport { x: at[0] as f32, y: at[1] as f32, w: job.size as f32, h: job.size as f32, depth: 0.0..1.0 });
            pc.set_scissor_rect(&gpu::ScissorRect { x: at[0] as i32, y: at[1] as i32, w: job.size, h: job.size });
            pc.bind(0, &ShadowData { u_shadow: ShadowPass { rows: [job.header, job.casters, 0, 0] }, b_mesh: mesh.into(), b_pool: pool.into() });
            // Runs of casters that are the same pooled mesh: one draw each.
            let caster = |k: u32| {
                let r = list.mesh[(job.casters + k * crate::mesh3d::CROWS as u32 + 3) as usize];
                (r[0].to_bits(), r[2].to_bits())
            };
            let mut k = 0;
            while k < job.count {
                let (mesh_at, tris) = caster(k);
                let mut n = 1;
                while k + n < job.count && caster(k + n) == (mesh_at, tris) {
                    n += 1;
                }
                // (No triangles: its mesh found no room in the pool.)
                if tris > 0 {
                    pc.draw(0, tris * 3, k, n);
                }
                k += n;
            }
        }
    }

    /// Takes frame `id` through Open Image Denoise, if it waits for it:
    /// waits for its traces, denoises, and hands the rest of the frame to
    /// the GPU. Call it once the next frame has begun, so the GPU traces
    /// that one meanwhile. (`finish` does it otherwise.)
    pub fn advance(&mut self, id: u64) -> Result<(), String> {
        let Some(at) = self.flights.iter().position(|f| f.id == id) else { return Err("no such frame in flight".into()) };
        let Some(rest) = self.flights[at].rest.take() else { return Ok(()) };
        let (set, size, sync, paths) = (self.flights[at].set, self.flights[at].size, self.flights[at].sync.clone(), self.flights[at].paths);
        let waiting = std::time::Instant::now();
        if !self.context.wait_for(&sync, frame_timeout_ms(paths)) {
            return Err("GPU timed out".into());
        }
        let denoising = std::time::Instant::now();
        if let Some(tracer) = self.tracer.as_mut() {
            for slot in 0..rest.pictures.len() as u32 {
                tracer.denoise(&self.context, set, slot);
            }
        }
        let recording = std::time::Instant::now();
        self.queue.start(&self.context, &mut self.encoder, self.profile.as_mut());
        self.record_rest(set, &rest);
        self.record_end(set, size, rest.targets.len() as u32);
        self.flights[at].sync = self.queue.submit(&self.context, &mut self.encoder);
        if let Some(p) = self.profile.as_mut() {
            p.add("gpu frame › wait for GPU (before OIDN)", denoising - waiting);
            p.add("gpu frame › OIDN denoise", recording - denoising);
            p.stage("gpu frame › record + upload", recording);
        }
        Ok(())
    }

    /// Waits for frame `id` and returns its pixels.
    pub fn finish(&mut self, id: u64) -> Result<Vec<u8>, String> {
        self.advance(id)?;
        let at = self.flights.iter().position(|f| f.id == id).ok_or("no such frame in flight")?;
        let flight = self.flights.remove(at);
        let waiting = std::time::Instant::now();
        if !self.context.wait_for(&flight.sync, frame_timeout_ms(flight.paths)) {
            // Its work may still be running: dropping this device waits for it.
            return Err("GPU timed out".into());
        }
        // Every frame before the oldest still in flight is done.
        self.completed = self.flights.iter().map(|f| f.id).min().unwrap_or(self.next) - 1;
        self.pool.after_frame(&self.context, self.completed);
        self.shaders.end_frame(&self.context);
        if let Some(p) = self.profile.as_mut() {
            let (used, size) = self.pool.bytes();
            p.pool.uploads += std::mem::take(&mut self.pool.uploads);
            p.pool.upload_bytes += std::mem::take(&mut self.pool.upload_bytes);
            p.pool.evictions += std::mem::take(&mut self.pool.evictions);
            (p.pool.meshes, p.pool.bytes, p.pool.capacity) = (self.pool.meshes() as u64, used as u64, size as u64);
            p.stage("gpu frame › wait for GPU", waiting);
            if let Some(t) = self.tracer.as_mut() {
                t.collect_profile(flight.set, p);
            }
        }
        let host = self.sets[flight.set].output.host.unwrap();
        self.context.sync_buffer(host);
        let (w, h) = flight.size;
        // SAFETY: the copy completed and the mapping holds w·h·4 bytes.
        let bytes = unsafe { std::slice::from_raw_parts(host.data(), (w * h * 4) as usize) }.to_vec();
        Ok(bytes)
    }

    /// With profiling: the pass timings of work still in the encoder's
    /// command buffers (they are read as each is taken up again), once the
    /// GPU is idle. Call before reading the profile.
    pub fn flush_profile(&mut self) {
        if self.profile.is_none() || !self.flights.is_empty() || !self.queue.idle(&self.context, 600_000) {
            return;
        }
        // Each command buffer in turn, empty: taking it up reads what it
        // last timed.
        for _ in 0..RING {
            self.queue.start(&self.context, &mut self.encoder, self.profile.as_mut());
            self.queue.submit(&self.context, &mut self.encoder);
        }
        self.queue.idle(&self.context, 600_000);
    }
}

impl Gpu {
    /// Makes `slang_raster` the raster pipeline of `program` (compiling its
    /// shader variant when the program changed).
    fn surface_raster(&mut self, program: &lahza_motion_core::surface::Program) -> Result<(), String> {
        if self.slang_raster.as_ref().is_some_and(|(k, _)| *k == program.key) {
            return Ok(());
        }
        let source = crate::surfaces::raster_source(program)?;
        let shader = self.context.try_create_shader(gpu::ShaderDesc { source: &source }).map_err(|e| format!("Slang materials' raster shader: {e}"))?;
        let pipeline = self.context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "motion raster (Slang surfaces)",
            data_layouts: &[&RasterData::layout()],
            compute: shader.at("raster"),
        });
        // The old variant's last frames are done once the GPU is idle.
        let idle = self.queue.idle(&self.context, 60_000);
        if let Some((_, mut old)) = self.slang_raster.replace((program.key, pipeline)) {
            if idle {
                self.context.destroy_compute_pipeline(&mut old);
            }
        }
        self.context.save_pipeline_cache();
        Ok(())
    }

    /// Records one target's visibility pass: clears its pages, draws each
    /// layer list's opaque meshes into the list's opaque page, then each
    /// see-through mesh into its own page over that list's opaque meshes.
    fn record_vis(&mut self, set: usize, plan: &TargetVis, vis_prims: &std::collections::HashMap<u32, VisPrim>, prims: &[Prim]) {
        let Some(vis) = self.vis.as_ref() else { return };
        if plan.pages.is_empty() {
            return;
        }
        let s = &self.sets[set];
        let atlas = &s.vis_atlas;
        let (mesh, prims_buf, groups, pool) = (s.mesh.device, s.prims.device, s.groups.device, self.pool.buffer);
        let mut pass = self.encoder.render(
            "mesh visibility",
            gpu::RenderTargetSet {
                colors: &[gpu::RenderTarget { view: atlas.ids_view, init_op: gpu::InitOp::Load, finish_op: gpu::FinishOp::Store }],
                depth_stencil: Some(gpu::RenderTarget { view: atlas.depth_view, init_op: gpu::InitOp::Load, finish_op: gpu::FinishOp::Store }),
            },
        );
        // Binds a pipeline on a page for primitive `prim` (what = 0: its
        // triangles with records, 1: its instances), then draws
        // [first instance, instances, vertices] each.
        let mut draw = |pipeline: &gpu::RenderPipeline, page: &Page, prim: u32, mode: u32, what: u32, draws: &[[u32; 3]]| {
            if draws.is_empty() {
                return;
            }
            let (w, h) = (page.rect[2] - page.rect[0], page.rect[3] - page.rect[1]);
            let mut pc = pass.with(pipeline);
            pc.set_viewport(&gpu::Viewport { x: page.at[0] as f32, y: page.at[1] as f32, w: w as f32, h: h as f32, depth: 0.0..1.0 });
            pc.set_scissor_rect(&gpu::ScissorRect { x: page.at[0] as i32, y: page.at[1] as i32, w, h });
            pc.bind(
                0,
                &VisData {
                    u_vis: VisPass {
                        page: [page.rect[0] as f32, page.rect[1] as f32, w as f32, h as f32],
                        atlas: [page.at[0] as f32, page.at[1] as f32, 0.0, 0.0],
                        draw: [0, prim, mode, what],
                    },
                    b_mesh: mesh.into(),
                    b_prims: prims_buf.into(),
                    b_groups: groups.into(),
                    b_pool: pool.into(),
                },
            );
            for &[first, count, vertices] in draws {
                pc.draw(0, vertices, first, count);
            }
        };
        for page in &plan.pages {
            draw(&vis.clear, page, 0, 0, 0, &[[0, 1, 3]]);
        }
        // (pipeline, page, draw of the plan, mode) in order.
        let mut order: Vec<(&gpu::RenderPipeline, usize, usize, u32)> = Vec::new();
        for (_, page, list) in &plan.opaque {
            order.extend(list.iter().map(|&i| (&vis.opaque, *page, i, 1)));
        }
        for &(i, page, behind) in &plan.see {
            if let Some(b) = behind {
                let list = plan.opaque.iter().find(|o| o.1 == b).map(|o| &o.2[..]).unwrap_or(&[]);
                order.extend(list.iter().map(|&j| (&vis.opaque, page, j, 1)));
            }
            order.push((&vis.see, page, i, 2));
        }
        let mut runs = Vec::new();
        for (pipeline, page, i, mode) in order {
            let prim = plan.draws[i].prim;
            let Some(vp) = vis_prims.get(&prim) else { continue };
            let page = &plan.pages[page];
            if vp.records > 0 {
                draw(pipeline, page, prim, mode, 0, &[[0, 1, vp.records * 3]]);
            }
            vp.draws(&prims[prim as usize], page.rect, &mut runs);
            draw(pipeline, page, prim, mode, 1, &runs);
        }
    }
}

/// Where compiled GPU pipelines are kept between runs (the first launch
/// after a build compiles the shaders; later ones load them).
fn pipeline_cache_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    Some(base.join("lahza").join("vulkan-pipelines.bin"))
}

/// How long a frame may take on the GPU: 30 s, plus a millisecond per 100
/// traced paths (a final-quality trace on a software GPU takes minutes; 64
/// paths at 1080 x 1920 may take 22).
pub(crate) fn frame_timeout_ms(traced_paths: u64) -> u32 {
    (30_000 + traced_paths / 100).min(u32::MAX as u64) as u32
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // Work still running (a frame that timed out, frames begun and
        // never finished) writes into these buffers: freeing them under it
        // crashes the process. Wait for it (a lost device returns at once).
        for _ in 0..30 {
            if self.queue.idle(&self.context, 60_000) {
                break;
            }
        }
        if let Some(mut t) = self.tracer.take() {
            t.destroy(&self.context);
        }
        for s in &mut self.sets {
            s.destroy(&self.context);
        }
        self.atlas.destroy(&self.context);
        self.images.destroy(&self.context);
        self.pool.destroy(&self.context);
        self.shaders.destroy(&self.context);
        if let Some(mut v) = self.vis.take() {
            for p in [&mut v.clear, &mut v.opaque, &mut v.see] {
                self.context.destroy_render_pipeline(p);
            }
        }
        if let Some(mut p) = self.shadow.take() {
            self.context.destroy_render_pipeline(&mut p);
        }
        self.context.destroy_compute_pipeline(&mut self.raster);
        if let Some((_, mut p)) = self.slang_raster.take() {
            self.context.destroy_compute_pipeline(&mut p);
        }
        self.context.destroy_compute_pipeline(&mut self.blur);
        self.context.destroy_compute_pipeline(&mut self.resolve);
        self.context.destroy_command_encoder(&mut self.encoder);
    }
}
