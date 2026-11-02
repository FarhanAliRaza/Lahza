//! Vulkan compute path on `blade-graphics`, headless. One submission per
//! output frame: every motion-blur sub-sample's groups are rasterised and
//! blurred, its main target accumulated, then the result is resolved to
//! RGBA8 and read back.

use crate::font::{Fonts, ATLAS_SIZE};
use crate::images::Images;
use crate::list::{DisplayList, Prim, Target, TILE};
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
    /// A buffer read back by the CPU: its twin is host-cached memory
    /// (reading write-combined upload memory costs tens of ms per frame).
    fn readback(context: &gpu::Context, name: &'static str, size: usize) -> Self {
        Self::with_host(context, name, size, Some(gpu::Memory::Shared))
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
}

/// Concatenates sub-sample display lists into one upload. Returns the
/// merged buffers and each sample's targets (main, groups) re-based.
pub fn merge(samples: &[DisplayList]) -> (DisplayList, Vec<(Target, Vec<crate::list::Group>)>) {
    let mut out = DisplayList::default();
    let mut targets = Vec::new();
    for list in samples {
        let prim_base = out.prims.len() as u32;
        let index_base = out.indices.len() as u32;
        let range_base = (out.ranges.len() / 2) as u32;
        let stop_base = (out.stops.len() / 2) as f32;
        let texel_base = out.group_texels;
        let seg_base = out.segments.len() as u32;
        let ops_base = out.ops.len() as u32;
        let mesh_base = out.mesh.len() as u32;
        let bins_base = out.bins.len() as u32;
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
                    p.geom2 = p.geom2.map(|v| rebase(v, mesh_base));
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
        out.group_texels += list.group_texels;
        let mut main = list.main();
        main.range_base += range_base;
        let groups = list
            .groups
            .iter()
            .map(|g| {
                let mut g = *g;
                g.target.range_base += range_base;
                g.offset += texel_base;
                g
            })
            .collect();
        targets.push((main, groups));
    }
    out.main = samples.first().and_then(|s| s.main);
    (out, targets)
}

impl Gpu {
    /// Opens a headless Vulkan context. Software rasterisers (llvmpipe) are
    /// rejected unless `allow_software`; the CPU path is faster than them.
    pub fn new(allow_software: bool) -> Result<Self, String> {
        // SAFETY: headless context; no external handles or shared ownership.
        let context = catch_unwind(AssertUnwindSafe(|| unsafe { gpu::Context::init(gpu::ContextDesc::default()) }))
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
            context,
            encoder,
            raster,
            blur,
            resolve,
            label,
        })
    }

    /// Renders weighted sub-samples (weights sum to 1) to straight RGBA8.
    pub fn render(&mut self, samples: &[DisplayList], fonts: &Fonts, images: &Images, footage: &[u32]) -> Result<Vec<u8>, String> {
        let main = samples.first().ok_or("nothing to render")?.main();
        let (w, h) = (main.width, main.height);
        if w.div_ceil(TILE) > 65535 || h.div_ceil(TILE) > 65535 {
            return Err("canvas exceeds the GPU dispatch limit".into());
        }
        let (list, targets) = merge(samples);
        let weight = 1.0 / samples.len() as f32;
        let ctx = &self.context;
        self.encoder.start();
        self.prims.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.prims))?;
        self.ranges.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.ranges))?;
        self.indices.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.indices))?;
        self.stops.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.stops))?;
        self.segs.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.segments))?;
        // Footage frames change every frame and upload every time.
        self.footage.upload(ctx, &mut self.encoder, bytemuck::cast_slice(footage))?;
        self.ops.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.ops))?;
        self.mesh.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.mesh))?;
        self.bins.upload(ctx, &mut self.encoder, bytemuck::cast_slice(&list.bins))?;
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
        };
        for (i, (main, groups)) in targets.iter().enumerate() {
            for g in groups {
                let t = g.target;
                let data = raster_data(
                    Pass { size: [t.width, t.height, t.tiles_x, t.range_base], dest: [g.offset, 0, 0, 0], background: [0.0; 4], weight: [1.0, 0.0, 0.0, 0.0] },
                    self.groups.device,
                    // Outer groups sample inner ones in other regions of
                    // the same buffer; regions never overlap.
                    self.groups.device,
                    self,
                );
                {
                    let mut pass = self.encoder.compute("group raster");
                    let mut pc = pass.with(&self.raster);
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
            let data = raster_data(
                Pass {
                    size: [main.width, main.height, main.tiles_x, main.range_base],
                    dest: [0, if i == 0 { 2 } else { 1 }, 0, 0],
                    background: main.background,
                    weight: [weight, 0.0, 0.0, 0.0],
                },
                self.accum.device,
                self.groups.device,
                self,
            );
            let mut pass = self.encoder.compute("main raster");
            let mut pc = pass.with(&self.raster);
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
        let sync = self.context.submit(&mut self.encoder);
        self.pending = Some(sync.clone());
        if !self.context.wait_for(&sync, 30_000) {
            return Err("GPU timed out".into());
        }
        self.pending = None;
        self.context.sync_buffer(host);
        // SAFETY: the copy completed and the mapping holds w·h·4 bytes.
        let bytes = unsafe { std::slice::from_raw_parts(host.data(), (w * h * 4) as usize) }.to_vec();
        Ok(bytes)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some(sync) = self.pending.take() {
            let _ = self.context.wait_for(&sync, 5000);
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
        ] {
            b.destroy(&self.context);
        }
        self.context.destroy_compute_pipeline(&mut self.raster);
        self.context.destroy_compute_pipeline(&mut self.blur);
        self.context.destroy_compute_pipeline(&mut self.resolve);
        self.context.destroy_command_encoder(&mut self.encoder);
    }
}
