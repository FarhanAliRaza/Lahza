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
}

/// Samples per pixel of the hardware visibility pass: the rasteriser's own
/// pattern is the standard 8x one.
const VIS_SAMPLES: u32 = 8;
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
        }
    }
    fn ensure(&mut self, context: &gpu::Context, size: usize) -> Result<(), String> {
        if size > MAX_BUFFER {
            return Err(format!("{} needs {size} bytes, over the GPU buffer limit", self.name));
        }
        if size > self.capacity {
            let next = Self::with_host(context, self.name, size.next_power_of_two(), self.host_memory);
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

pub struct Gpu {
    context: gpu::Context,
    encoder: gpu::CommandEncoder,
    raster: gpu::ComputePipeline,
    /// The raster pipeline with a project's Slang surfaces (`surfaces.rs`),
    /// by program key: built when the program changes, the last destroyed.
    slang_raster: Option<(u64, gpu::ComputePipeline)>,
    /// Hardware mesh visibility, where the device has 8x MSAA (else the
    /// raster shader finds the nearest triangles itself).
    vis: Option<VisPipelines>,
    vis_atlas: VisAtlas,
    vis_pages: Buffer,
    /// Generated meshes kept on the device across frames.
    pool: crate::pool::MeshPool,
    /// Shader layers' kernels.
    shaders: crate::shader::Pipelines,
    blur: gpu::ComputePipeline,
    resolve: gpu::ComputePipeline,
    prims: Buffer,
    ranges: Buffer,
    indices: Buffer,
    stops: Buffer,
    segs: Buffer,
    footage: Buffer,
    ops: Buffer,
    mesh: Buffer,
    bins: Buffer,
    atlas: Buffer,
    images: Buffer,
    groups: Buffer,
    groups_tmp: Buffer,
    accum: Buffer,
    output: Buffer,
    atlas_generation: u64,
    images_generation: u64,
    pending: Option<gpu::SyncPoint>,
    pub label: String,
    /// The path tracer, where the device has ray queries.
    tracer: Option<crate::pt::Tracer>,
    pub(crate) trace_error: Option<String>,
    /// With profiling: this frame's stages and GPU pass timestamps (the
    /// renderer takes them after each frame).
    pub(crate) profile: Option<crate::profile::Profile>,
    /// The encoder was started at the end of the last frame (to read its
    /// timestamps) and is ready to record.
    started: bool,
}

/// Concatenates sub-sample display lists into one upload. Returns the
/// merged buffers and each sample's targets (main, groups) re-based.
/// Group texels are not concatenated: a sample's groups are read only by
/// its own passes, which run before the next sample's, so every sample
/// draws its groups in the same texels (eight sub-frames of a full-frame
/// transition would otherwise need eight frames' worth, past the buffer
/// limit).
pub fn merge(samples: &[DisplayList]) -> (DisplayList, Vec<(Target, Vec<crate::list::Group>, Vec<MeshDraw>)>) {
    let mut out = DisplayList::default();
    let mut targets = Vec::new();
    for list in samples {
        let prim_base = out.prims.len() as u32;
        let index_base = out.indices.len() as u32;
        let range_base = (out.ranges.len() / 2) as u32;
        let stop_base = (out.stops.len() / 2) as f32;
        let seg_base = out.segments.len() as u32;
        let ops_base = out.ops.len() as u32;
        let mesh_base = out.mesh.len() as u32;
        let bins_base = out.bins.len() as u32;
        let trace_base = out.traces.len() as u32;
        let shader_base = out.shaders.len() as u32;
        out.shaders.extend_from_slice(&list.shaders);
        let id_base = out.mesh_ids;
        out.mesh_ids += list.mesh_ids;
        out.pool.extend(list.pool.iter().map(|r| crate::mesh3d::PoolRef { row: r.row + mesh_base, ..r.clone() }));
        out.traces.extend(list.traces.iter().cloned());
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
        out.group_texels = out.group_texels.max(list.group_texels);
        let mut main = list.main();
        main.range_base += range_base;
        let groups = list
            .groups
            .iter()
            .map(|g| {
                let mut g = *g;
                g.target.range_base += range_base;
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
        let vis = (caps.sample_count_mask & VIS_SAMPLES != 0).then(|| {
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
                    multisample_state: gpu::MultisampleState { sample_count: VIS_SAMPLES, ..Default::default() },
                })
            };
            VisPipelines {
                clear: make("mesh visibility clear", gpu::CompareFunction::Always),
                opaque: make("mesh visibility opaque", gpu::CompareFunction::GreaterEqual),
                see: make("mesh visibility see-through", gpu::CompareFunction::Greater),
            }
        });
        // Without the pass the raster shader still binds an atlas.
        let vis_atlas = VisAtlas::new(&context, 16, 16, if vis.is_some() { VIS_SAMPLES } else { 4 }, vis.is_some());
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
        let encoder = context.create_command_encoder(gpu::CommandEncoderDesc { name: "motion", buffer_count: 1 });
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
        Ok(Self {
            prims: Buffer::new(&context, "motion prims", 1 << 16, true),
            ranges: Buffer::new(&context, "motion ranges", 1 << 16, true),
            indices: Buffer::new(&context, "motion indices", 1 << 16, true),
            stops: Buffer::new(&context, "motion stops", 1 << 12, true),
            segs: Buffer::new(&context, "motion segments", 1 << 12, true),
            footage: Buffer::new(&context, "motion footage", 1 << 12, true),
            ops: Buffer::new(&context, "motion post ops", 1 << 12, true),
            mesh: Buffer::new(&context, "motion mesh", 1 << 12, true),
            bins: Buffer::new(&context, "motion mesh bins", 1 << 12, true),
            atlas: Buffer::new(&context, "motion atlas", (ATLAS_SIZE * ATLAS_SIZE) as usize, true),
            images: Buffer::new(&context, "motion images", 1 << 12, true),
            groups: Buffer::new(&context, "motion groups", 1 << 12, false),
            groups_tmp: Buffer::new(&context, "motion groups tmp", 1 << 12, false),
            accum: Buffer::new(&context, "motion accum", 1 << 12, false),
            output: Buffer::readback(&context, "motion output", 1 << 12),
            atlas_generation: 0,
            images_generation: 0,
            pending: None,
            vis_pages: Buffer::new(&context, "motion visibility pages", 1 << 12, true),
            pool: crate::pool::MeshPool::new(&context),
            shaders: Default::default(),
            context,
            encoder,
            raster,
            slang_raster: None,
            vis,
            vis_atlas,
            blur,
            resolve,
            label,
            tracer,
            trace_error,
            profile: profile.then(crate::profile::Profile::default),
            started: false,
        })
    }

    /// Whether meshes go through the hardware visibility pass (their
    /// display lists then hold shared triangle records).
    pub fn has_vis(&self) -> bool {
        self.vis.is_some()
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

    /// Renders weighted sub-samples (weights sum to 1) to straight RGBA8.
    pub fn render(&mut self, samples: &[DisplayList], fonts: &Fonts, images: &Images, footage: &[u32]) -> Result<Vec<u8>, String> {
        let main = samples.first().ok_or("nothing to render")?.main();
        let (w, h) = (main.width, main.height);
        if w.div_ceil(TILE) > 65535 || h.div_ceil(TILE) > 65535 {
            return Err("canvas exceeds the GPU dispatch limit".into());
        }
        let (mut list, targets) = merge(samples);
        let weight = 1.0 / samples.len() as f32;
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
            if self.vis_atlas.width != atlas_width || need > self.vis_atlas.height || need * 4 < self.vis_atlas.height {
                self.vis_atlas.destroy(&self.context);
                self.vis_atlas = VisAtlas::new(&self.context, atlas_width, need, VIS_SAMPLES, true);
            }
        }
        let ctx = &self.context;
        let recording = std::time::Instant::now();
        if let Some(t) = self.tracer.as_mut() {
            t.begin_frame(&self.context);
        }
        if !std::mem::take(&mut self.started) {
            self.encoder.start();
        }
        let mut in_tracer = std::time::Duration::ZERO;
        // Instances' meshes into the pool (where they are goes into their
        // rows), then each mesh primitive's visibility draws.
        let pooling = std::time::Instant::now();
        self.pool.resolve(ctx, &mut self.encoder, &list.pool, &mut list.mesh)?;
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
        self.prims.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.prims))?;
        self.ranges.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.ranges))?;
        self.indices.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.indices))?;
        self.stops.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.stops))?;
        self.segs.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.segments))?;
        // Footage frames change every frame and upload every time.
        self.footage.upload(ctx, &mut self.encoder, bytemuck::cast_slice(footage))?;
        self.ops.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.ops))?;
        self.mesh.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.mesh))?;
        // Tile bins are only read when the raster shader finds the nearest
        // triangles itself.
        if !use_vis {
            self.bins.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.bins))?;
        }
        self.vis_pages.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&vis_table))?;
        if self.atlas_generation != fonts.generation {
            self.atlas.upload(ctx, &mut self.encoder, &fonts.atlas)?;
            self.atlas_generation = fonts.generation;
        }
        if self.images_generation != images.generation {
            self.images.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&images.texels))?;
            self.images_generation = images.generation;
        }
        let group_bytes = list.group_texels as usize * 16;
        self.groups.ensure(ctx, group_bytes)?;
        self.groups_tmp.ensure(ctx, group_bytes)?;
        self.accum.ensure(ctx, (w * h) as usize * 16)?;
        self.output.ensure(ctx, (w * h) as usize * 4)?;
        let raster_data = |pass: Pass, target: gpu::Buffer, groups: gpu::Buffer, s: &Self| RasterData {
            u_pass: pass,
            b_prims: s.prims.device.into(),
            b_ranges: s.ranges.device.into(),
            b_indices: s.indices.device.into(),
            b_stops: s.stops.device.into(),
            b_atlas: s.atlas.device.into(),
            b_images: s.images.device.into(),
            b_groups: groups.into(),
            b_segs: s.segs.device.into(),
            b_footage: s.footage.device.into(),
            b_ops: s.ops.device.into(),
            b_mesh: s.mesh.device.into(),
            b_bins: s.bins.device.into(),
            b_target: target.into(),
            t_vis_ids: s.vis_atlas.ids_view,
            t_vis_depth: s.vis_atlas.depth_view,
            b_vis: s.vis_pages.device.into(),
            b_pool: s.pool.buffer.into(),
        };
        // Paths traced this frame: how long the GPU may take.
        let mut paths = 0u64;
        for (i, (main, groups, _)) in targets.iter().enumerate() {
            for (gi, g) in groups.iter().enumerate() {
                let t = g.target;
                if let Some(ti) = g.trace {
                    paths += t.width as u64 * t.height as u64 * list.traces[ti as usize].options.spp.max(1) as u64;
                    let tracer = self.tracer.as_mut().ok_or("a traced group, but no path tracer")?;
                    let entered = std::time::Instant::now();
                    tracer.trace(&self.context, &mut self.encoder, &list.traces[ti as usize], g.offset, (t.width, t.height), self.groups.device, self.images.device, self.profile.as_mut())?;
                    in_tracer += entered.elapsed();
                    continue;
                }
                // A shader layer's kernel paints it; one that did not build
                // (the project says why) is rasterised empty: transparent.
                if let Some(si) = g.shader {
                    if self.shaders.dispatch(&self.context, &mut self.encoder, &list.shaders[si as usize], g, self.groups.device)? {
                        continue;
                    }
                }
                if let Some((group_plans, _)) = plans.get(i) {
                    self.record_vis(&group_plans[gi], &draws, &list.prims);
                }
                let data = raster_data(
                    Pass { size: [t.width, t.height, t.tiles_x, t.range_base], dest: [g.offset, 0, use_vis as u32, 0], background: [0.0; 4], weight: [1.0, 0.0, 0.0, 0.0] },
                    self.groups.device,
                    // Outer groups sample inner ones in other regions of
                    // the same buffer; regions never overlap.
                    self.groups.device,
                    self,
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
                    // Same as the CPU path: no blur pass (σ = 0 would make
                    // the Gaussian weights NaN).
                    continue;
                }
                let radius = (g.sigma * 3.0).ceil();
                let norm: f32 = (-(radius as i32)..=radius as i32).map(|d| (-(d * d) as f32 / (2.0 * g.sigma * g.sigma)).exp()).sum();
                for (horizontal, src, dst) in [(1u32, self.groups.device, self.groups_tmp.device), (0, self.groups_tmp.device, self.groups.device)] {
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
            if let Some((_, main_plan)) = plans.get(i) {
                self.record_vis(main_plan, &draws, &list.prims);
            }
            let data = raster_data(
                Pass {
                    size: [main.width, main.height, main.tiles_x, main.range_base],
                    dest: [0, if i == 0 { 2 } else { 1 }, use_vis as u32, 0],
                    background: main.background,
                    weight: [weight, 0.0, 0.0, 0.0],
                },
                self.accum.device,
                self.groups.device,
                self,
            );
            let mut pass = self.encoder.compute("main raster");
            let mut pc = pass.with(match &self.slang_raster {
                Some((_, p)) if slang => p,
                _ => &self.raster,
            });
            pc.bind(0, &data);
            pc.dispatch([main.tiles_x, main.tiles_y, 1]);
        }
        {
            let data = ResolveData {
                u_resolve: ResolvePass { size: [w, h, targets.len() as u32, 0] },
                b_accum: self.accum.device.into(),
                b_output: self.output.device.into(),
            };
            let mut pass = self.encoder.compute("resolve");
            let mut pc = pass.with(&self.resolve);
            pc.bind(0, &data);
            pc.dispatch([w.div_ceil(16), h.div_ceil(16), 1]);
        }
        let host = self.output.host.unwrap();
        self.encoder.transfer("readback").copy_buffer_to_buffer(self.output.device.into(), host.into(), (w * h * 4) as u64);
        let submitted = std::time::Instant::now();
        let sync = self.context.submit(&mut self.encoder);
        self.pending = Some(sync.clone());
        if !self.context.wait_for(&sync, frame_timeout_ms(paths)) {
            // `pending` stays set: dropping this device waits for the work.
            return Err("GPU timed out".into());
        }
        self.pending = None;
        self.pool.after_frame(&self.context);
        self.shaders.end_frame(&self.context);
        if let Some(p) = self.profile.as_mut() {
            let (used, size) = self.pool.bytes();
            p.pool.uploads += std::mem::take(&mut self.pool.uploads);
            p.pool.upload_bytes += std::mem::take(&mut self.pool.upload_bytes);
            p.pool.evictions += std::mem::take(&mut self.pool.evictions);
            (p.pool.meshes, p.pool.bytes, p.pool.capacity) = (self.pool.meshes() as u64, used as u64, size as u64);
            p.add("gpu frame › record + upload", submitted - recording - in_tracer);
            p.stage("gpu frame › wait for GPU", submitted);
            // Starting the encoder reads the finished frame's timestamps.
            self.encoder.start();
            self.started = true;
            p.add_gpu(self.encoder.timings());
            if let Some(t) = self.tracer.as_mut() {
                t.collect_profile(p);
            }
        }
        self.context.sync_buffer(host);
        // SAFETY: the copy completed and the mapping holds w·h·4 bytes.
        let bytes = unsafe { std::slice::from_raw_parts(host.data(), (w * h * 4) as usize) }.to_vec();
        Ok(bytes)
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
        // The old variant's last frame is done: every frame is waited for,
        // and one that timed out is waited for here.
        if let Some(sync) = self.pending.take() {
            if !self.context.wait_for(&sync, 60_000) {
                self.pending = Some(sync);
            }
        }
        if let Some((_, mut old)) = self.slang_raster.replace((program.key, pipeline)) {
            if self.pending.is_none() {
                self.context.destroy_compute_pipeline(&mut old);
            }
        }
        self.context.save_pipeline_cache();
        Ok(())
    }

    /// Records one target's visibility pass: clears its pages, draws each
    /// layer list's opaque meshes into the list's opaque page, then each
    /// see-through mesh into its own page over that list's opaque meshes.
    fn record_vis(&mut self, plan: &TargetVis, vis_prims: &std::collections::HashMap<u32, VisPrim>, prims: &[Prim]) {
        let Some(vis) = self.vis.as_ref() else { return };
        if plan.pages.is_empty() {
            return;
        }
        let atlas = &self.vis_atlas;
        let (mesh, prims_buf, groups, pool) = (self.mesh.device, self.prims.device, self.groups.device, self.pool.buffer);
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
        // Work still running (a frame that timed out) writes into these
        // buffers: freeing them under it crashes the process. Wait for it
        // (a lost device returns at once).
        if let Some(sync) = self.pending.take() {
            for _ in 0..30 {
                if self.context.wait_for(&sync, 60_000) {
                    break;
                }
            }
        }
        if let Some(mut t) = self.tracer.take() {
            t.destroy(&self.context);
        }
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
            &mut self.atlas,
            &mut self.images,
            &mut self.groups,
            &mut self.groups_tmp,
            &mut self.accum,
            &mut self.output,
            &mut self.vis_pages,
        ] {
            b.destroy(&self.context);
        }
        self.vis_atlas.destroy(&self.context);
        self.pool.destroy(&self.context);
        self.shaders.destroy(&self.context);
        if let Some(mut v) = self.vis.take() {
            for p in [&mut v.clear, &mut v.opaque, &mut v.see] {
                self.context.destroy_render_pipeline(p);
            }
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
