//! Path tracer for export: hardware ray queries through blade-graphics.
//!
//! The raster renderer stays the live preview. For export, each layer list
//! that holds meshes is traced instead of rasterised: the list builder turns
//! it into a [`Trace`] (instances of cached meshes with their materials,
//! cards for flat 3D layers, the lights and the camera) and reserves an
//! offscreen group for it; the GPU traces the group (see `pt.wgsl`) before
//! the raster composites it in the list's place, so 2D layers, effects and
//! everything around the 3D content work exactly as in the preview.
//!
//! Geometry: one bottom-level structure per mesh, in the mesh's own space,
//! cached across frames by its parameters; per frame a top-level structure
//! places them (object → camera space). Cards are one non-opaque structure
//! per trace, alpha-tested against their pictures.

use crate::mesh3d::Backdrop;
use blade_graphics as gpu;
use gpu::ShaderData as _;
use lahza_motion_core::mesh::{Mesh, PARTS};
use std::collections::HashMap;
use std::sync::Arc;

/// Samples and bounces: the quality knob.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceOptions {
    /// Paths per pixel.
    pub spp: u32,
    /// Surface interactions per path.
    pub bounces: u32,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self { spp: 64, bounces: 6 }
    }
}

/// One mesh layer in a traced list.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceInstance {
    pub mesh: Arc<Mesh>,
    /// Identifies the geometry across frames (the mesh parameters).
    pub key: Vec<u8>,
    /// Object → camera space, row-major 3 × 4.
    pub transform: [[f32; 4]; 3],
    /// Material rows (`mesh3d::MAT_ROWS` per material, see `mesh3d::material_rows`).
    pub materials: Vec<[f32; 4]>,
    /// Per part: (first material, count): a triangle of part p and segment
    /// s uses material first + s mod count.
    pub parts: [(u32, u32); PARTS],
}

/// A flat 3D layer: its picture on a quad (camera space, corners in order).
#[derive(Clone, Debug, PartialEq)]
pub struct TraceCard {
    pub view: [[f32; 3]; 4],
    pub picture: Backdrop,
}

/// A traced layer list: what the GPU needs to render its group.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub instances: Vec<TraceInstance>,
    pub cards: Vec<TraceCard>,
    /// The light block (see `mesh3d::light_rows`).
    pub lights: Vec<[f32; 4]>,
    /// Camera: x = focal · x / z + centre, then `root` (canvas → target).
    pub focal: f32,
    pub centre: [f32; 2],
    /// Canvas → target (row-major 3 × 3, homogeneous) and its inverse.
    pub root: [f32; 9],
    pub inv_root: [f32; 9],
    /// The group: texel (x, y) is target point origin + factor · (x + ½, y + ½).
    pub origin: [f32; 2],
    pub factor: [f32; 2],
    pub options: TraceOptions,
    /// Seeds the samples (the frame's time): the same frame renders the same.
    pub seed: u32,
}

impl Trace {
    /// Bytes that determine the traced picture (frame cache key).
    pub fn signature(&self, out: &mut Vec<u8>) {
        let f = |out: &mut Vec<u8>, v: &[f32]| out.extend(v.iter().flat_map(|x| x.to_bits().to_le_bytes()));
        for i in &self.instances {
            out.extend_from_slice(&i.key);
            f(out, &i.transform.concat());
            f(out, &i.materials.concat());
        }
        for c in &self.cards {
            f(out, &c.view.concat());
            out.extend(c.picture.offset.to_le_bytes());
        }
        f(out, &self.lights.concat());
        f(out, &[self.focal, self.centre[0], self.centre[1]]);
        f(out, &self.root);
        f(out, &[self.origin[0], self.origin[1], self.factor[0], self.factor[1]]);
        out.extend(self.options.spp.to_le_bytes());
        out.extend(self.options.bounces.to_le_bytes());
        out.extend(self.seed.to_le_bytes());
    }
}

/// Whether this machine can trace rays from compute shaders.
pub fn supported(context: &gpu::Context) -> bool {
    context.capabilities().ray_query.contains(gpu::ShaderVisibility::COMPUTE)
}

const SHADER: &str = include_str!("pt.wgsl");

/// The custom index marking the cards' instance.
const CARD_INSTANCE: u32 = 0x00FF_FFFF;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Group width, height, texel offset, samples per pixel.
    size: [u32; 4],
    /// Bounces, frame seed, card instance custom index, cards.
    opts: [u32; 4],
    /// focal, centre x, centre y, 0.
    cam: [f32; 4],
    /// Group origin (x, y) and factor (x, y) in target px.
    place: [f32; 4],
    root: [[f32; 4]; 3],
    inv_root: [[f32; 4]; 3],
}

#[derive(blade_macros::ShaderData)]
struct TraceData {
    u: Uniforms,
    acc: gpu::AccelerationStructure,
    b_inst: gpu::BufferPiece,
    b_attr: gpu::BufferPiece,
    b_idx: gpu::BufferPiece,
    b_tag: gpu::BufferPiece,
    b_mat: gpu::BufferPiece,
    b_light: gpu::BufferPiece,
    b_card: gpu::BufferPiece,
    b_groups: gpu::BufferPiece,
    b_guides: gpu::BufferPiece,
}

/// A mesh's bottom-level structure and where its attributes live.
struct Geometry {
    blas: gpu::AccelerationStructure,
    positions: gpu::Buffer,
    indices: gpu::Buffer,
    /// Offsets (in vertices, indices and triangles) into the shared arrays.
    attr_base: u32,
    idx_base: u32,
    tag_base: u32,
    used: u64,
}

/// GPU side of the path tracer, owned by `gpu::Gpu`.
pub struct Tracer {
    pipeline: gpu::ComputePipeline,
    geometry: HashMap<Vec<u8>, Geometry>,
    /// Per-vertex [position, normal] rows, indices and triangle tags of every
    /// cached mesh, uploaded again when a mesh is added.
    attrs: Vec<[f32; 4]>,
    idx: Vec<u32>,
    tags: Vec<u32>,
    shared: Option<[gpu::Buffer; 3]>,
    /// Buffers and structures of the frame in flight, freed by the next
    /// `begin_frame` (the frame's work has finished by then).
    retired: Vec<gpu::Buffer>,
    retired_structures: Vec<gpu::AccelerationStructure>,
    frame: u64,
    /// Scratch addresses must be aligned to this for builds.
    alignment: u64,
    /// Record hits by hand (see `cast_ray` in pt.wgsl): software Vulkan
    /// (Mesa lavapipe) reports committed hits with the wrong instance.
    manual_commit: bool,
}

impl Tracer {
    pub fn new(context: &gpu::Context) -> Result<Self, String> {
        let shader = context.try_create_shader(gpu::ShaderDesc { source: SHADER })?;
        let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "trace", data_layouts: &[&TraceData::layout()], compute: shader.at("trace") });
        let manual_commit = context.device_information().is_software_emulated;
        Ok(Self { pipeline, geometry: HashMap::new(), attrs: Vec::new(), idx: Vec::new(), tags: Vec::new(), shared: None, retired: Vec::new(), retired_structures: Vec::new(), frame: 0, alignment: 256, manual_commit })
    }

    /// Frees the last frame's transient buffers and structures; call when
    /// its work has finished, before recording a new frame.
    pub fn begin_frame(&mut self, context: &gpu::Context) {
        for b in self.retired.drain(..) {
            context.destroy_buffer(b);
        }
        for a in self.retired_structures.drain(..) {
            context.destroy_acceleration_structure(a);
        }
        self.frame += 1;
        let frame = self.frame;
        let stale: Vec<Vec<u8>> = self.geometry.iter().filter(|(_, g)| frame - g.used > 120).map(|(k, _)| k.clone()).collect();
        for k in stale {
            if let Some(g) = self.geometry.remove(&k) {
                context.destroy_acceleration_structure(g.blas);
                context.destroy_buffer(g.positions);
                context.destroy_buffer(g.indices);
            }
        }
    }

    /// A shared-memory buffer holding `bytes`, freed after this frame.
    fn transient(&mut self, context: &gpu::Context, name: &str, bytes: &[u8]) -> gpu::Buffer {
        let b = context.create_buffer(gpu::BufferDesc { name, size: bytes.len().max(16) as u64, memory: gpu::Memory::Shared });
        // SAFETY: a fresh mapped buffer of at least bytes.len() bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.data(), bytes.len()) };
        context.sync_buffer(b);
        self.retired.push(b);
        b
    }

    /// Scratch for one structure build (builds in one submission each get
    /// their own), freed after this frame.
    fn scratch(&mut self, context: &gpu::Context, size: u64) -> gpu::Buffer {
        let b = context.create_buffer(gpu::BufferDesc { name: "trace scratch", size: size.max(self.alignment) + self.alignment, memory: gpu::Memory::Device });
        self.retired.push(b);
        b
    }

    /// Builds (once) the bottom-level structure of a mesh.
    fn geometry(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, key: &[u8], mesh: &Mesh) -> bool {
        if let Some(g) = self.geometry.get_mut(key) {
            g.used = self.frame;
            return true;
        }
        if mesh.tris.is_empty() {
            return false;
        }
        let indices: Vec<u32> = mesh.tris.iter().flatten().copied().collect();
        let pb = context.create_buffer(gpu::BufferDesc { name: "trace positions", size: (mesh.positions.len() * 12).max(16) as u64, memory: gpu::Memory::Shared });
        let ib = context.create_buffer(gpu::BufferDesc { name: "trace mesh indices", size: (indices.len() * 4).max(16) as u64, memory: gpu::Memory::Shared });
        // SAFETY: fresh mapped buffers of the right size.
        unsafe {
            std::ptr::copy_nonoverlapping(mesh.positions.as_ptr() as *const u8, pb.data(), mesh.positions.len() * 12);
            std::ptr::copy_nonoverlapping(indices.as_ptr() as *const u8, ib.data(), indices.len() * 4);
        }
        context.sync_buffer(pb);
        context.sync_buffer(ib);
        let m = gpu::AccelerationStructureMesh {
            vertex_data: pb.into(),
            vertex_format: gpu::VertexFormat::F32Vec3,
            vertex_stride: 12,
            vertex_count: mesh.positions.len() as u32,
            index_data: ib.into(),
            index_type: Some(gpu::IndexType::U32),
            triangle_count: mesh.tris.len() as u32,
            transform_data: gpu::Buffer::default().into(),
            is_opaque: true,
        };
        let sizes = context.get_bottom_level_acceleration_structure_sizes(std::slice::from_ref(&m));
        let blas = context.create_acceleration_structure(gpu::AccelerationStructureDesc { name: "trace blas", ty: gpu::AccelerationStructureType::BottomLevel, size: sizes.data });
        let scratch = self.scratch(context, sizes.scratch);
        encoder.acceleration_structure("trace blas").build_bottom_level(blas, std::slice::from_ref(&m), scratch.into());
        let attr_base = (self.attrs.len() / 2) as u32;
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            self.attrs.push([p[0], p[1], p[2], 0.0]);
            self.attrs.push([n[0], n[1], n[2], 0.0]);
        }
        let idx_base = self.idx.len() as u32;
        self.idx.extend_from_slice(&indices);
        let tag_base = self.tags.len() as u32;
        self.tags.extend_from_slice(&mesh.tags);
        // The shared arrays changed: upload them again (the old copies may
        // still be read by work recorded earlier in this frame).
        if let Some(old) = self.shared.take() {
            self.retired.extend(old);
        }
        self.geometry.insert(key.to_vec(), Geometry { blas, positions: pb, indices: ib, attr_base, idx_base, tag_base, used: self.frame });
        true
    }

    fn shared(&mut self, context: &gpu::Context) -> [gpu::Buffer; 3] {
        if let Some(s) = self.shared {
            return s;
        }
        let make = |name: &str, bytes: &[u8]| {
            let b = context.create_buffer(gpu::BufferDesc { name, size: bytes.len().max(16) as u64, memory: gpu::Memory::Shared });
            // SAFETY: a fresh mapped buffer of at least bytes.len() bytes.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.data(), bytes.len()) };
            context.sync_buffer(b);
            b
        };
        let s = [make("trace attrs", bytemuck::cast_slice(&self.attrs)), make("trace indices", bytemuck::cast_slice(&self.idx)), make("trace tags", bytemuck::cast_slice(&self.tags))];
        self.shared = Some(s);
        s
    }

    /// Records the trace of `t` into group texels at `offset` (w × h).
    #[allow(clippy::too_many_arguments)]
    pub fn trace(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, t: &Trace, offset: u32, size: (u32, u32), groups: gpu::Buffer) {
        let mut instances = Vec::new();
        let mut blases = Vec::new();
        let mut inst_rows: Vec<[f32; 4]> = Vec::new();
        let mut mat_rows: Vec<[f32; 4]> = Vec::new();
        let b = |v: u32| f32::from_bits(v);
        for inst in &t.instances {
            if !self.geometry(context, encoder, &inst.key, &inst.mesh) {
                continue;
            }
            let g = &self.geometry[&inst.key];
            let mat_base = (mat_rows.len() / crate::mesh3d::MAT_ROWS) as u32;
            mat_rows.extend_from_slice(&inst.materials);
            let p = |k: usize| [b(mat_base + inst.parts[k].0), b(inst.parts[k].1.max(1))];
            let custom = (inst_rows.len() / 4) as u32;
            inst_rows.push([b(g.attr_base), b(g.idx_base), b(g.tag_base), 0.0]);
            inst_rows.push([p(0)[0], p(0)[1], p(1)[0], p(1)[1]]);
            inst_rows.push([p(2)[0], p(2)[1], p(3)[0], p(3)[1]]);
            inst_rows.push([p(4)[0], p(4)[1], p(5)[0], p(5)[1]]);
            instances.push(gpu::AccelerationStructureInstance { acceleration_structure_index: blases.len() as u32, transform: inst.transform.into(), mask: 0xFF, custom_index: custom });
            blases.push(g.blas);
        }
        // Cards: one non-opaque structure per trace.
        let mut card_rows: Vec<[f32; 4]> = Vec::new();
        if !t.cards.is_empty() {
            let mut verts: Vec<[f32; 3]> = Vec::new();
            let mut idx: Vec<u32> = Vec::new();
            for c in &t.cards {
                let base = verts.len() as u32;
                verts.extend_from_slice(&c.view);
                idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                let p = c.picture;
                card_rows.push([p.origin[0] as f32, p.origin[1] as f32, p.factor[0] as f32, p.factor[1] as f32]);
                card_rows.push([b(p.offset), b(p.size.0), b(p.size.1), 0.0]);
            }
            let vb = self.transient(context, "trace card verts", bytemuck::cast_slice(&verts));
            let ib = self.transient(context, "trace card indices", bytemuck::cast_slice(&idx));
            let m = gpu::AccelerationStructureMesh {
                vertex_data: vb.into(),
                vertex_format: gpu::VertexFormat::F32Vec3,
                vertex_stride: 12,
                vertex_count: verts.len() as u32,
                index_data: ib.into(),
                index_type: Some(gpu::IndexType::U32),
                triangle_count: (idx.len() / 3) as u32,
                transform_data: gpu::Buffer::default().into(),
                is_opaque: false,
            };
            let sizes = context.get_bottom_level_acceleration_structure_sizes(std::slice::from_ref(&m));
            let blas = context.create_acceleration_structure(gpu::AccelerationStructureDesc { name: "trace cards", ty: gpu::AccelerationStructureType::BottomLevel, size: sizes.data });
            self.retired_structures.push(blas);
            let scratch = self.scratch(context, sizes.scratch);
            encoder.acceleration_structure("trace cards").build_bottom_level(blas, std::slice::from_ref(&m), scratch.into());
            instances.push(gpu::AccelerationStructureInstance { acceleration_structure_index: blases.len() as u32, transform: gpu::IDENTITY_TRANSFORM, mask: 0xFF, custom_index: CARD_INSTANCE });
            blases.push(blas);
        }
        let [attr, idx, tag] = self.shared(context);
        for v in [&mut inst_rows, &mut mat_rows, &mut card_rows] {
            if v.is_empty() {
                v.push([0.0; 4]);
            }
        }
        let mut lights = t.lights.clone();
        if lights.is_empty() {
            lights.push([0.0; 4]);
        }
        let inst_buf = self.transient(context, "trace instances", bytemuck::cast_slice(&inst_rows));
        let mat_buf = self.transient(context, "trace materials", bytemuck::cast_slice(&mat_rows));
        let light_buf = self.transient(context, "trace lights", bytemuck::cast_slice(&lights));
        let card_buf = self.transient(context, "trace cards", bytemuck::cast_slice(&card_rows));
        // Top level (an empty scene still gets one, with nothing in it).
        let tsizes = context.get_top_level_acceleration_structure_sizes(instances.len().max(1) as u32);
        let tlas = context.create_acceleration_structure(gpu::AccelerationStructureDesc { name: "trace tlas", ty: gpu::AccelerationStructureType::TopLevel, size: tsizes.data });
        self.retired_structures.push(tlas);
        let ibuf = context.create_acceleration_structure_instance_buffer(&instances, &blases);
        self.retired.push(ibuf);
        let scratch = self.scratch(context, tsizes.scratch);
        encoder.acceleration_structure("trace tlas").build_top_level(tlas, &blases, instances.len() as u32, ibuf.into(), scratch.into());
        // Guides for the denoiser: albedo, normal, depth per texel.
        let guides = context.create_buffer(gpu::BufferDesc { name: "trace guides", size: ((size.0 * size.1) as u64 * 48).max(16), memory: gpu::Memory::Device });
        self.retired.push(guides);
        let r = |m: &[f32; 9]| [[m[0], m[1], m[2], 0.0], [m[3], m[4], m[5], 0.0], [m[6], m[7], m[8], 0.0]];
        let data = TraceData {
            u: Uniforms {
                size: [size.0, size.1, offset, t.options.spp.max(1)],
                opts: [t.options.bounces.max(1), t.seed, CARD_INSTANCE, t.cards.len() as u32],
                cam: [t.focal, t.centre[0], t.centre[1], if self.manual_commit { 1.0 } else { 0.0 }],
                place: [t.origin[0], t.origin[1], t.factor[0], t.factor[1]],
                root: r(&t.root),
                inv_root: r(&t.inv_root),
            },
            acc: tlas,
            b_inst: inst_buf.into(),
            b_attr: attr.into(),
            b_idx: idx.into(),
            b_tag: tag.into(),
            b_mat: mat_buf.into(),
            b_light: light_buf.into(),
            b_card: card_buf.into(),
            b_groups: groups.into(),
            b_guides: guides.into(),
        };
        let mut pass = encoder.compute("trace");
        let mut pc = pass.with(&self.pipeline);
        pc.bind(0, &data);
        pc.dispatch([size.0.div_ceil(8), size.1.div_ceil(8), 1]);
    }

    pub fn destroy(&mut self, context: &gpu::Context) {
        self.begin_frame(context);
        if let Some(s) = self.shared.take() {
            for b in s {
                context.destroy_buffer(b);
            }
        }
        for (_, g) in self.geometry.drain() {
            context.destroy_acceleration_structure(g.blas);
            context.destroy_buffer(g.positions);
            context.destroy_buffer(g.indices);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE: &str = r#"
struct Params { size: vec4<u32> }
var<uniform> u: Params;
var acc: acceleration_structure;
var<storage, read_write> b_out: array<u32>;

@compute @workgroup_size(8, 8, 1)
fn probe(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = u.size.x;
    if id.x >= n || id.y >= n { return; }
    // Rays straight down -z through a 2 x 2 square of the xy plane.
    let p = (vec2<f32>(id.xy) + 0.5) / f32(n) * 2.0 - 1.0;
    var rq: ray_query;
    rayQueryInitialize(&rq, acc, RayDesc(RAY_FLAG_NONE, 0xFFu, 0.0, 100.0, vec3<f32>(p, 5.0), vec3<f32>(0.0, 0.0, -1.0)));
    rayQueryProceed(&rq);
    let hit = rayQueryGetCommittedIntersection(&rq);
    var v = 0u;
    if hit.kind != RAY_QUERY_INTERSECTION_NONE { v = 1u + u32(hit.t * 10.0); }
    b_out[id.y * n + id.x] = v;
}
"#;

    #[derive(blade_macros::ShaderData)]
    struct ProbeData {
        u: [u32; 4],
        acc: gpu::AccelerationStructure,
        b_out: gpu::BufferPiece,
    }

    fn context() -> Option<gpu::Context> {
        // SAFETY: headless context; no external handles.
        let context = unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.ok()?;
        if !supported(&context) {
            eprintln!("skipping: no ray queries on {}", context.device_information().device_name);
            return None;
        }
        Some(context)
    }

    /// One triangle, traced: rays inside it hit at the right distance,
    /// rays outside miss.
    #[test]
    fn rays_hit_a_triangle_through_a_tlas() {
        let Some(ctx) = context() else { return };
        // Triangle (-1,-1,0) (1,-1,0) (-1,1,0): the lower-left half of the square.
        let verts: [[f32; 3]; 3] = [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, 1.0, 0.0]];
        let indices: [u32; 3] = [0, 1, 2];
        let vb = ctx.create_buffer(gpu::BufferDesc { name: "verts", size: 36, memory: gpu::Memory::Shared });
        let ib = ctx.create_buffer(gpu::BufferDesc { name: "indices", size: 12, memory: gpu::Memory::Shared });
        // SAFETY: shared buffers are mapped and large enough.
        unsafe {
            std::ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, vb.data(), 36);
            std::ptr::copy_nonoverlapping(indices.as_ptr() as *const u8, ib.data(), 12);
        }
        let mesh = gpu::AccelerationStructureMesh {
            vertex_data: vb.into(),
            vertex_format: gpu::VertexFormat::F32Vec3,
            vertex_stride: 12,
            vertex_count: 3,
            index_data: ib.into(),
            index_type: Some(gpu::IndexType::U32),
            triangle_count: 1,
            transform_data: gpu::Buffer::default().into(),
            is_opaque: true,
        };
        let sizes = ctx.get_bottom_level_acceleration_structure_sizes(std::slice::from_ref(&mesh));
        let blas = ctx.create_acceleration_structure(gpu::AccelerationStructureDesc { name: "blas", ty: gpu::AccelerationStructureType::BottomLevel, size: sizes.data });
        let instances = [gpu::AccelerationStructureInstance { acceleration_structure_index: 0, ..Default::default() }];
        let tsizes = ctx.get_top_level_acceleration_structure_sizes(1);
        let tlas = ctx.create_acceleration_structure(gpu::AccelerationStructureDesc { name: "tlas", ty: gpu::AccelerationStructureType::TopLevel, size: tsizes.data });
        let inst = ctx.create_acceleration_structure_instance_buffer(&instances, &[blas]);
        let scratch = ctx.create_buffer(gpu::BufferDesc { name: "scratch", size: sizes.scratch.max(tsizes.scratch), memory: gpu::Memory::Device });
        let n = 16u32;
        let out = ctx.create_buffer(gpu::BufferDesc { name: "out", size: (n * n * 4) as u64, memory: gpu::Memory::Shared });
        let shader = ctx.try_create_shader(gpu::ShaderDesc { source: PROBE }).expect("probe shader");
        let pipeline = ctx.create_compute_pipeline(gpu::ComputePipelineDesc { name: "probe", data_layouts: &[&ProbeData::layout()], compute: shader.at("probe") });
        let mut enc = ctx.create_command_encoder(gpu::CommandEncoderDesc { name: "probe", buffer_count: 1 });
        enc.start();
        {
            let mut pass = enc.acceleration_structure("blas");
            pass.build_bottom_level(blas, std::slice::from_ref(&mesh), scratch.into());
        }
        {
            let mut pass = enc.acceleration_structure("tlas");
            pass.build_top_level(tlas, &[blas], 1, inst.into(), scratch.into());
        }
        {
            let mut pass = enc.compute("trace");
            let mut pc = pass.with(&pipeline);
            pc.bind(0, &ProbeData { u: [n, 0, 0, 0], acc: tlas, b_out: out.into() });
            pc.dispatch([n.div_ceil(8), n.div_ceil(8), 1]);
        }
        let sync = ctx.submit(&mut enc);
        assert!(ctx.wait_for(&sync, 60_000), "trace timed out");
        // SAFETY: the pass finished; the buffer holds n·n u32.
        let got = unsafe { std::slice::from_raw_parts(out.data() as *const u32, (n * n) as usize) }.to_vec();
        for y in 0..n {
            for x in 0..n {
                let p = [(x as f32 + 0.5) / n as f32 * 2.0 - 1.0, (y as f32 + 0.5) / n as f32 * 2.0 - 1.0];
                if (p[0] + p[1]).abs() < 0.05 {
                    continue; // on the diagonal edge: either way
                }
                let inside = p[0] + p[1] < 0.0;
                let v = got[(y * n + x) as usize];
                assert_eq!(v != 0, inside, "ray at {p:?}");
                if inside {
                    assert_eq!(v, 1 + 50, "hit at t = 5");
                }
            }
        }
        ctx.destroy_command_encoder(&mut enc);
        for b in [vb, ib, scratch, out, inst] {
            ctx.destroy_buffer(b);
        }
        ctx.destroy_acceleration_structure(blas);
        ctx.destroy_acceleration_structure(tlas);
    }
}

#[cfg(test)]
mod scene_tests {
    use crate::tests::{cpu, project};
    use crate::{Backend, RenderOptions, Renderer};
    use lahza_motion_core::compile::Compiled;
    use serde_json::json;

    /// A renderer that path-traces, or None (and a note) without ray queries.
    pub(crate) fn tracer(spp: u32) -> Option<Renderer> {
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        let r = Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, path: Some(super::TraceOptions { spp, bounces: 4 }), ..Default::default() });
        // Asked for the GPU and it (or the tracer's shader) failed: a bug.
        assert!(!allow || r.is_gpu(), "no GPU: {}", r.backend_label());
        let trace_error = r.gpu.as_ref().and_then(|g| g.trace_error.clone());
        assert!(!allow || trace_error.as_deref().is_none_or(|e| !e.starts_with("path tracer:")), "the tracer failed: {trace_error:?}");
        if !r.is_gpu() || !r.can_trace() {
            eprintln!("skipping: no path tracer ({}; {:?})", r.backend_label(), trace_error);
            return None;
        }
        Some(r)
    }

    pub(crate) fn still_life() -> lahza_motion_core::document::Project {
        let mut p = project(json!([
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"},
             "transform": {"position": [160, 130], "rotation_x": 90}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"preset": "plastic", "color": "#d03734"},
             "transform": {"position": [120, 100]}},
            {"id": "cube", "kind": "mesh", "mesh": {"type": "box", "size": [50, 50], "depth": 50}, "material": {"preset": "metal", "color": "#c0c0c0"},
             "transform": {"position": [205, 105], "rotation_y": 30}}
        ]));
        p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 20]})).unwrap();
        p
    }

    /// The principled materials, traced and rasterised, to $LAHZA_PT_OUT.
    #[test]
    #[ignore]
    fn dump_materials() {
        let Some(mut r) = tracer(128) else { return };
        let mut p = project(json!([
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#c8c2b8"}, "transform": {"position": [480, 235], "rotation_x": 90}},
            {"id": "spun", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 80, "height": 30, "bevel": 6}, "material": {"preset": "brushed", "color": "#d0d4da"}, "transform": {"position": [110, 200], "rotation_x": 10}},
            {"id": "velvet", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": {"preset": "velvet", "color": "#5a1030", "sheen_color": "#ffc0d0"}, "transform": {"position": [290, 175]}},
            {"id": "gold", "kind": "mesh", "mesh": {"type": "torus", "radius": 55, "tube": 20}, "material": {"preset": "metal", "color": "#ffc356", "specular_tint": "#ff8040", "roughness": 0.25}, "transform": {"position": [450, 160], "rotation_x": 10}},
            {"id": "candy", "kind": "mesh", "mesh": {"type": "box", "size": [90, 90], "depth": 90, "bevel": 12}, "material": {"preset": "coated", "color": "#f0f0f0", "clearcoat_color": "#30a0ff"}, "transform": {"position": [620, 190], "rotation_y": 30}},
            {"id": "prism", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": {"preset": "glass", "color": "#e8f4ff", "transmission": 1, "roughness": 0, "dispersion": 0.08, "thickness": 300}, "transform": {"position": [800, 175]}}
        ]));
        p.settings.width = 960;
        p.settings.height = 300;
        p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 12]})).unwrap();
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let t0 = std::time::Instant::now();
        let traced = r.render(&c, 0.0);
        eprintln!("traced in {:?}", t0.elapsed());
        let raster = cpu().render(&c, 0.0);
        let mut both = image::RgbaImage::new(960, 600);
        image::imageops::overlay(&mut both, &traced, 0, 0);
        image::imageops::overlay(&mut both, &raster, 0, 300);
        both.save(std::env::var("LAHZA_PT_OUT").unwrap_or_else(|_| "/tmp/pt_materials.png".into())).unwrap();
    }

    #[test]
    #[ignore]
    fn dump_still_life() {
        let Some(mut r) = tracer(32) else { return };
        let p = still_life();
        let c = Compiled::new(&p);
        let t0 = std::time::Instant::now();
        let traced = r.render(&c, 0.0);
        eprintln!("traced in {:?}; warnings {:?}", t0.elapsed(), r.warnings());
        let raster = cpu().render(&c, 0.0);
        let dir = std::env::temp_dir();
        traced.save(dir.join("pt_traced.png")).unwrap();
        raster.save(dir.join("pt_raster.png")).unwrap();
    }
}

#[cfg(test)]
mod physics_tests {
    use super::scene_tests::{still_life, tracer};
    use crate::tests::{cpu, project};
    use crate::{RenderOptions, Renderer};
    use lahza_motion_core::compile::Compiled;
    use serde_json::json;

    fn mask(img: &image::RgbaImage) -> Vec<bool> {
        img.pixels().map(|p| p.0[3] > 0 && (p.0[0] > 12 || p.0[1] > 12 || p.0[2] > 12)).collect()
    }

    /// Traced and rasterised, the meshes stand in the same places: every
    /// pixel the two disagree on is within a pixel of an edge of the raster.
    /// (Unlit, so the mask measures coverage, not shading noise.)
    #[test]
    fn silhouettes_match_the_raster() {
        let Some(mut r) = tracer(8) else { return };
        let mut p = project(json!([
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 40}, "material": {"color": "#d03734", "unlit": true},
             "transform": {"position": [110, 90]}},
            {"id": "cube", "kind": "mesh", "mesh": {"type": "box", "size": [60, 60], "depth": 60}, "material": {"color": "#3050c0", "unlit": true},
             "transform": {"position": [215, 95], "rotation_y": 30, "rotation_x": 20}}
        ]));
        p.scenes[0].camera = serde_json::from_value(json!({"orbit": [10, 15], "dolly": 60})).unwrap();
        let c = Compiled::new(&p);
        let (a, b) = (mask(&r.render(&c, 0.0)), mask(&cpu().render(&c, 0.0)));
        let (w, h) = (320usize, 180usize);
        let near_edge = |i: usize| {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            (-1..=1).any(|dy| (-1..=1).any(|dx| {
                let (nx, ny) = (x + dx, y + dy);
                nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64 && b[(ny as usize) * w + nx as usize] != b[i]
            }))
        };
        let off: Vec<usize> = (0..w * h).filter(|&i| a[i] != b[i] && !near_edge(i)).collect();
        assert!(off.is_empty(), "{} pixels differ away from edges, e.g. {:?}", off.len(), off.iter().take(5).map(|i| (i % w, i / w)).collect::<Vec<_>>());
        assert!(a.iter().filter(|v| **v).count() > 3000, "the meshes were traced");
    }

    /// Fixed seeds: the same frame twice is the same picture.
    #[test]
    fn the_same_frame_traces_the_same() {
        let Some(mut r) = tracer(4) else { return };
        let c = Compiled::new(&still_life());
        let first = r.render(&c, 0.0);
        let Some(mut again) = tracer(4) else { return };
        assert_eq!(again.render(&c, 0.0), first);
    }

    /// The furnace test: a white diffuse sphere lit only by a uniform
    /// environment reflects exactly what arrives, so it is invisible
    /// against that environment's radiance (here 0.5: sRGB 188).
    #[test]
    fn a_white_sphere_vanishes_in_a_uniform_environment() {
        let Some(mut r) = tracer(64) else { return };
        let mut p = project(json!([
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 70},
             "material": {"preset": "matte", "color": "#ffffff", "roughness": 1, "metallic": 0},
             "transform": {"position": [160, 90]}}
        ]));
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "ambient", "intensity": 0.5}])).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none"})).unwrap());
        let img = r.render(&Compiled::new(&p), 0.0);
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in 50..130u32 {
            for x in 120..200u32 {
                let d = ((x as f64 - 160.0).powi(2) + (y as f64 - 90.0).powi(2)).sqrt();
                if d < 60.0 {
                    sum += img.get_pixel(x, y).0[0] as f64;
                    n += 1.0;
                }
            }
        }
        let mean = sum / n;
        assert!((mean - 188.0).abs() < 8.0, "the sphere shows: mean {mean:.1}, environment 188");
    }

    /// A sphere of `material` (radius 90 at the centre of 320 × 240).
    fn sphere_in(material: serde_json::Value, lights: serde_json::Value, environment: serde_json::Value) -> lahza_motion_core::document::Project {
        let mut p = project(json!([{"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 90}, "material": material, "transform": {"position": [160, 120]}}]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].lights = serde_json::from_value(lights).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(environment).unwrap());
        p
    }

    fn disc(img: &image::RgbaImage, cx: f64, cy: f64, r0: f64, r1: f64, keep: impl Fn(f64, f64) -> bool) -> [f64; 3] {
        let (mut sum, mut n) = ([0.0f64; 3], 0.0f64);
        for (x, y, p) in img.enumerate_pixels() {
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            let d = (dx * dx + dy * dy).sqrt();
            if d >= r0 && d < r1 && keep(dx, dy) {
                for c in 0..3 {
                    sum[c] += p.0[c] as f64;
                }
                n += 1.0;
            }
        }
        sum.map(|v| v / n.max(1.0))
    }

    /// The furnace for every lobe: white rough metals, plastics and coats
    /// in a uniform environment reflect all that arrives.
    #[test]
    fn white_materials_vanish_in_a_uniform_environment() {
        let Some(mut r) = tracer(64) else { return };
        let lights = json!([{"type": "ambient", "intensity": 0}]);
        let env = json!({"preset": "none", "sky": "#bcbcbc", "horizon": "#bcbcbc", "ground": "#bcbcbc"});
        for material in [
            json!({"preset": "metal", "color": "#ffffff", "roughness": 1}),
            json!({"preset": "metal", "color": "#ffffff", "roughness": 0.5}),
            json!({"preset": "brushed", "color": "#ffffff"}),
            json!({"preset": "plastic", "color": "#ffffff", "roughness": 0.2}),
        ] {
            let img = r.render(&Compiled::new(&sphere_in(material.clone(), lights.clone(), env.clone())), 0.0);
            let mean = disc(&img, 160.0, 120.0, 0.0, 80.0, |_, _| true)[1];
            assert!((mean - 188.0).abs() < 8.0, "{material}: mean {mean:.1}, environment 188");
        }
    }

    /// A mirror ball shows the sky in its upper half and the ground in its
    /// lower half, as bright as they are.
    #[test]
    fn a_mirror_ball_reflects_sky_and_ground() {
        let Some(mut r) = tracer(16) else { return };
        let env = json!({"preset": "none", "sky": "#e04030", "horizon": "#e04030", "ground": "#3040e0"});
        let img = r.render(&Compiled::new(&sphere_in(json!({"preset": "metal", "color": "#ffffff", "roughness": 0}), json!([{"type": "ambient", "intensity": 0}]), env)), 0.0);
        let top = disc(&img, 160.0, 120.0, 20.0, 70.0, |_, dy| dy < -20.0);
        let bottom = disc(&img, 160.0, 120.0, 20.0, 70.0, |_, dy| dy > 20.0);
        assert!((top[0] - 224.0).abs() < 14.0 && top[2] < 80.0, "sky above: {top:?}");
        assert!((bottom[2] - 224.0).abs() < 14.0 && bottom[0] < 80.0, "ground below: {bottom:?}");
    }

    /// Under one light and nothing else, tracing and rasterising agree on
    /// every material (the same BSDF, shaded two ways).
    #[test]
    fn traced_and_rasterised_materials_agree_under_one_light() {
        let Some(mut r) = tracer(64) else { return };
        let lights = json!([{"type": "directional", "from": [-0.5, -0.6, -1], "shadow": false}]);
        let env = json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"});
        for material in [
            json!({"preset": "plastic", "color": "#c03030"}),
            json!({"preset": "metal", "color": "#ffc356", "specular_tint": "#ff8040", "roughness": 0.35}),
            json!({"preset": "brushed", "color": "#d0d4da"}),
            json!({"preset": "velvet", "color": "#5a1030", "sheen_color": "#ffc0d0"}),
            json!({"preset": "coated", "color": "#f0f0f0", "clearcoat_color": "#30a0ff"}),
        ] {
            let c = Compiled::new(&sphere_in(material.clone(), lights.clone(), env.clone()));
            let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
            let (ma, mb) = (disc(&a, 160.0, 120.0, 0.0, 85.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 85.0, |_, _| true));
            let worst = (0..3).map(|k| (ma[k] - mb[k]).abs()).fold(0.0, f64::max);
            assert!(worst < 6.0, "{material}: traced {ma:?}, raster {mb:?}");
        }
    }

    /// Brushed metal: the traced highlight stretches across the grain too.
    #[test]
    fn traced_brushed_highlights_stretch() {
        let Some(mut r) = tracer(64) else { return };
        let lights = json!([{"type": "directional", "from": [0, 0, -1], "intensity": 1.5, "shadow": false}]);
        let env = json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"});
        let img = r.render(&Compiled::new(&sphere_in(json!({"preset": "brushed", "color": "#ffffff"}), lights, env)), 0.0);
        let luma = |p: &image::Rgba<u8>| 0.2126 * p.0[0] as f64 + 0.7152 * p.0[1] as f64 + 0.0722 * p.0[2] as f64;
        let max = img.pixels().map(luma).fold(0.0, f64::max);
        let pts: Vec<(f64, f64)> = img.enumerate_pixels().filter(|(_, _, p)| luma(p) > 0.5 * max).map(|(x, y, _)| (x as f64, y as f64)).collect();
        let n = pts.len() as f64;
        let (mx, my) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
        let sx = (pts.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>() / n).sqrt();
        let sy = (pts.iter().map(|p| (p.1 - my).powi(2)).sum::<f64>() / n).sqrt();
        assert!(sy > 1.8 * sx, "tall highlight: {sx:.1} × {sy:.1}");
    }

    /// A glass ball in front of a white wall: the colour deepens with the
    /// distance travelled inside (a smaller `thickness` absorbs more), and
    /// dispersion splits the wall's edge into colours.
    #[test]
    fn glass_absorbs_with_depth_and_disperses() {
        let Some(mut r) = tracer(64) else { return };
        let shot = |glass: serde_json::Value, wall: serde_json::Value, r: &mut crate::Renderer| {
            let mut p = project(json!([
                {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": wall}, "material": {"preset": "flat", "color": "#ffffff"}, "transform": {"position": [160, 120], "z": 300}},
                {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 90}, "material": glass, "transform": {"position": [160, 120]}}
            ]));
            p.settings.width = 320;
            p.settings.height = 240;
            p.scenes[0].lights = serde_json::from_value(json!([{"type": "ambient", "intensity": 0}])).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none"})).unwrap());
            r.render(&Compiled::new(&p), 0.0)
        };
        let deep = shot(json!({"preset": "glass", "color": "#80c0ff", "transmission": 1, "roughness": 0, "thickness": 40}), json!([2000, 2000]), &mut r);
        let light = shot(json!({"preset": "glass", "color": "#80c0ff", "transmission": 1, "roughness": 0, "thickness": 400}), json!([2000, 2000]), &mut r);
        let (d, l) = (disc(&deep, 160.0, 120.0, 0.0, 20.0, |_, _| true), disc(&light, 160.0, 120.0, 0.0, 20.0, |_, _| true));
        assert!(d[0] < 0.6 * l[0] && d[2] > d[0], "deeper through less thickness: {d:?} vs {l:?}");
        let fringes = |img: &image::RgbaImage| img.pixels().filter(|p| (p.0[0] as i32 - p.0[2] as i32).abs() > 60).count();
        let clear = json!({"preset": "glass", "color": "#ffffff", "transmission": 1, "roughness": 0, "thickness": 10000});
        let mut prism = clear.clone();
        prism["dispersion"] = json!(0.3);
        let plain = fringes(&shot(clear, json!([40, 2000]), &mut r));
        let split = fringes(&shot(prism, json!([40, 2000]), &mut r));
        assert!(split > plain + 150, "colour fringes: {split} with dispersion, {plain} without");
    }

    /// A light's `softness` sizes it: the shadow of a box on a floor keeps
    /// a sharp edge from a sharp light and a wide one from a soft light;
    /// and a point light's `range` fades it as the raster does.
    #[test]
    fn soft_lights_soften_shadows_and_range_fades() {
        let Some(mut r) = tracer(64) else { return };
        let shot = |softness: f64, r: &mut crate::Renderer| {
            let mut p = project(json!([
                {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#ffffff"}, "transform": {"position": [160, 120]}},
                {"id": "block", "kind": "mesh", "mesh": {"type": "box", "size": [60, 2000], "depth": 20}, "material": {"preset": "matte", "color": "#000000", "specular": 0}, "transform": {"position": [80, 120], "z": -150}}
            ]));
            p.settings.width = 320;
            p.settings.height = 240;
            p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [-1, 0, -1], "softness": softness, "shadow": true}])).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none"})).unwrap());
            r.render(&Compiled::new(&p), 0.0)
        };
        // Penumbra: columns along the middle row between 10% and 90% lit.
        let width = |img: &image::RgbaImage| {
            let row: Vec<f64> = (115..320u32).map(|x| img.get_pixel(x, 120).0[1] as f64).collect();
            let lit = row.iter().cloned().fold(0.0, f64::max);
            row.iter().filter(|v| **v > 0.1 * lit && **v < 0.9 * lit).count()
        };
        let (sharp, soft) = (width(&shot(0.0, &mut r)), width(&shot(1.0, &mut r)));
        assert!(sharp <= 3, "a sharp light's shadow edge: {sharp} px");
        assert!(soft >= sharp + 8, "a soft light's: {soft} px (sharp {sharp})");
        // Range: traced as the raster fades it.
        let mut p = project(json!([{"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [2000, 2000]}, "material": {"preset": "matte", "color": "#ffffff"}, "transform": {"position": [160, 120]}}]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "point", "position": [40, 120, -60], "range": 220, "softness": 0, "shadow": false}])).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
        let c = Compiled::new(&p);
        let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
        for x in [60u32, 150, 230, 300] {
            let (ta, tb) = (a.get_pixel(x, 120).0[1] as i32, b.get_pixel(x, 120).0[1] as i32);
            assert!((ta - tb).abs() <= 8, "x {x}: traced {ta}, raster {tb}");
        }
    }

    /// An area light: traced and rasterised light the same; its size
    /// softens shadows without changing how bright it is; mirrors show
    /// it, the camera does not.
    #[test]
    fn area_lights_light_soften_and_reflect() {
        let Some(mut r) = tracer(64) else { return };
        let dark = json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"});
        let setup = |layers: serde_json::Value, light: serde_json::Value| {
            let mut p = project(layers);
            p.settings.width = 320;
            p.settings.height = 240;
            p.scenes[0].lights = serde_json::from_value(json!([light])).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(dark.clone()).unwrap());
            Compiled::new(&p)
        };
        // Agreement on lit spheres, matte and glossy.
        for material in [json!({"preset": "matte", "color": "#d0d0d0"}), json!({"preset": "plastic", "color": "#c03030", "roughness": 0.4})] {
            let c = setup(json!([{"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 90}, "material": material, "transform": {"position": [160, 120]}}]),
                json!({"type": "area", "position": [-200, -100, -600], "size": [300, 200], "intensity": 1.5, "shadow": false}));
            assert!(c.errors.is_empty(), "{:?}", c.errors);
            let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
            let (ma, mb) = (disc(&a, 160.0, 120.0, 0.0, 85.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 85.0, |_, _| true));
            let worst = (0..3).map(|k| (ma[k] - mb[k]).abs()).fold(0.0, f64::max);
            assert!(worst < 8.0, "{material}: traced {ma:?}, raster {mb:?}");
        }
        // Size: same light on the floor, wider shadow edge.
        let shadow = |size: f64, r: &mut crate::Renderer| {
            let c = setup(json!([
                {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#ffffff"}, "transform": {"position": [160, 120]}},
                {"id": "block", "kind": "mesh", "mesh": {"type": "box", "size": [60, 2000], "depth": 20}, "material": {"preset": "matte", "color": "#000000", "specular": 0}, "transform": {"position": [80, 120], "z": -150}}
            ]), json!({"type": "area", "position": [-600, 120, -600], "aim": [160, 120, 0], "size": [size, size], "shape": "disc", "shadow": true}));
            r.render(&c, 0.0)
        };
        let (small, big) = (shadow(10.0, &mut r), shadow(300.0, &mut r));
        let lit = |img: &image::RgbaImage| img.get_pixel(20, 40).0[1] as f64;
        assert!((lit(&small) - lit(&big)).abs() < 10.0, "brightness keeps with size: {} vs {}", lit(&small), lit(&big));
        let edge = |img: &image::RgbaImage| {
            let row: Vec<f64> = (115..320u32).map(|x| img.get_pixel(x, 120).0[1] as f64).collect();
            let top = row.iter().cloned().fold(0.0, f64::max);
            row.iter().filter(|v| **v > 0.1 * top && **v < 0.9 * top).count()
        };
        assert!(edge(&big) >= edge(&small) + 8, "shadow edge: {} px big, {} px small", edge(&big), edge(&small));
        // A mirror ball shows the panel; the camera does not.
        let c = setup(json!([{"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": {"preset": "metal", "color": "#ffffff", "roughness": 0}, "transform": {"position": [160, 120]}}]),
            json!({"type": "area", "position": [160, 120, -400], "aim": [160, 120, 0], "size": [200, 200], "intensity": 0.2, "shadow": false}));
        let img = r.render(&c, 0.0);
        let centre = img.get_pixel(160, 120).0;
        assert!(centre[1] > 200, "the ball's centre mirrors the panel: {centre:?}");
        let rim = img.get_pixel(160, 175).0;
        assert!(rim[1] < 60, "its edge mirrors the dark: {rim:?}");
    }

    /// The environment's bright lobes (softboxes, the sun) are sampled
    /// directly: a matte wall lit only by a small one is clean at a few
    /// samples per pixel, and as bright as the raster has it.
    #[test]
    fn environment_lobes_are_sampled_directly() {
        let Some(mut r) = tracer(16) else { return };
        let mut p = project(json!([{"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [2000, 2000]}, "material": {"preset": "matte", "color": "#ffffff", "specular": 0}, "transform": {"position": [160, 120]}}]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "ambient", "intensity": 0}])).unwrap();
        // Sunset: a small warm sun low on one side, over a dark sky.
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "sunset", "rotation": 180, "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
        let c = Compiled::new(&p);
        let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
        let patch: Vec<f64> = (100..140u32).flat_map(|y| (140..180u32).map(move |x| (x, y))).map(|(x, y)| a.get_pixel(x, y).0[0] as f64).collect();
        let mean = patch.iter().sum::<f64>() / patch.len() as f64;
        let sd = (patch.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / patch.len() as f64).sqrt();
        let raster = b.get_pixel(160, 120).0[0] as f64;
        eprintln!("traced mean {mean:.1} sd {sd:.1}, raster {raster}");
        assert!(mean > 20.0, "lit by the sun: {mean:.1}");
        assert!(sd < 0.08 * mean, "clean: sd {sd:.1} of {mean:.1}");
        assert!((mean - raster).abs() < 0.12 * raster.max(20.0), "as bright as the raster: {mean:.1} vs {raster}");
    }

    /// Lists with what the tracer can't draw yet (wires, effects such as
    /// depth of field, mattes, blend modes) keep the raster's picture.
    #[test]
    fn lists_the_tracer_cannot_draw_are_rasterised() {
        let Some(mut r) = tracer(4) else { return };
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        let mut raster = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, ..Default::default() });
        for layer in [
            json!({"id": "cage", "kind": "mesh", "mesh": {"type": "box", "size": [80, 80], "depth": 80}, "material": {"preset": "wireframe"}, "transform": {"position": [160, 90], "rotation_y": 30}}),
            json!({"id": "soft", "kind": "mesh", "mesh": {"type": "sphere", "radius": 50}, "effects": [{"type": "glow", "radius": 12}], "transform": {"position": [160, 90]}}),
        ] {
            let c = Compiled::new(&project(json!([layer])));
            assert!(c.errors.is_empty(), "{:?}", c.errors);
            assert_eq!(r.render(&c, 0.0), raster.render(&c, 0.0), "{layer}");
        }
    }

    /// Without ray queries a path-traced render is rasterised, and says so.
    #[test]
    fn without_ray_queries_the_raster_draws_and_says_so() {
        let mut r = Renderer::new(None, RenderOptions { backend: crate::Backend::Cpu, path: Some(super::TraceOptions::default()), ..Default::default() });
        let c = Compiled::new(&still_life());
        assert_eq!(r.render(&c, 0.0), cpu().render(&c, 0.0));
        assert!(r.warnings().iter().any(|w| w.contains("path tracing needs a GPU with ray queries")), "{:?}", r.warnings());
    }
}
