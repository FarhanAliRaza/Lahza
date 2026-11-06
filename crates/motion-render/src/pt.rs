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
    /// Filter the noise (edge-aware, guided by albedo, normal and depth).
    pub denoise: bool,
    /// Accumulate over consecutive frames (reprojected along each surface's
    /// motion): for sequences rendered in order, such as an export.
    pub temporal: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self { spp: 64, bounces: 6, denoise: true, temporal: false }
    }
}

/// À-trous passes (steps 1, 2, 4, … px).
const DENOISE_PASSES: u32 = 5;

/// One mesh layer in a traced list.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceInstance {
    /// The layer (finds its transform on the previous frame).
    pub id: Arc<str>,
    pub mesh: Arc<Mesh>,
    /// Identifies the geometry across frames (the mesh parameters).
    pub key: Vec<u8>,
    /// Object → camera space, row-major 3 × 4.
    pub transform: [[f32; 4]; 3],
    /// Object → world (world-space surface noise).
    pub world: [[f32; 4]; 3],
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
    /// The environment image (the lights' row 0 says whether it is used).
    pub env: Option<Arc<crate::envmap::EnvMap>>,
    /// Thin lens: [aperture radius, focus depth] in camera px (0: pinhole).
    pub lens: [f32; 2],
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
        out.push(self.options.denoise as u8);
        out.push(self.options.temporal as u8);
        out.extend(self.seed.to_le_bytes());
        f(out, &self.lens);
        if let Some(e) = &self.env {
            out.extend((Arc::as_ptr(e) as usize as u64).to_le_bytes());
        }
    }
}

/// Whether this machine can trace rays from compute shaders.
pub fn supported(context: &gpu::Context) -> bool {
    context.capabilities().ray_query.contains(gpu::ShaderVisibility::COMPUTE)
}

const SHADER: &str = include_str!("pt.wgsl");

/// Rows per instance: [attribute, index, tag bases], part materials (3),
/// the previous frame's object → camera transform (3), this frame's (3),
/// object → world (3).
const INST_ROWS: usize = 13;

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
    /// Lens aperture radius, focus depth, 0, 0.
    lens: [f32; 4],
    /// Denoiser pass: step px, temporal history (1), 0, 0.
    denoise: [u32; 4],
    /// The previous frame's camera (focal, centre, valid) and group
    /// placement and canvas → target, for reprojection.
    prev_cam: [f32; 4],
    prev_place: [f32; 4],
    prev_root: [[f32; 4]; 3],
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
    b_env: gpu::BufferPiece,
    b_hdr: gpu::BufferPiece,
    b_tmp: gpu::BufferPiece,
    b_hist: gpu::BufferPiece,
    b_next: gpu::BufferPiece,
    b_images: gpu::BufferPiece,
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
    temporal: gpu::ComputePipeline,
    atrous: gpu::ComputePipeline,
    resolve: gpu::ComputePipeline,
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
    /// The environment image's rows on the GPU, by the map's address.
    env: Option<(usize, Arc<crate::envmap::EnvMap>, gpu::Buffer)>,
    /// Each mesh layer's object → camera transform on the last frame.
    prev_transforms: HashMap<Arc<str>, [[f32; 4]; 3]>,
    /// Temporal history per trace of the frame (by its order).
    history: HashMap<u32, History>,
    /// The next trace's order in this frame.
    slot: u32,
}

/// What a traced group looked like on the previous frame: its
/// accumulated lighting (three rows per texel, see pt.wgsl `temporal`),
/// twice (read last frame's, write this one's), and where it was.
struct History {
    size: (u32, u32),
    buffers: [gpu::Buffer; 2],
    current: usize,
    cam: [f32; 4],
    place: [f32; 4],
    root: [[f32; 4]; 3],
}

impl Tracer {
    pub fn new(context: &gpu::Context) -> Result<Self, String> {
        let shader = context.try_create_shader(gpu::ShaderDesc { source: SHADER })?;
        let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "trace", data_layouts: &[&TraceData::layout()], compute: shader.at("trace") });
        let temporal = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "trace temporal", data_layouts: &[&TraceData::layout()], compute: shader.at("temporal") });
        let atrous = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "trace denoise", data_layouts: &[&TraceData::layout()], compute: shader.at("atrous") });
        let resolve = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "trace resolve", data_layouts: &[&TraceData::layout()], compute: shader.at("resolve") });
        Ok(Self { pipeline, temporal, atrous, resolve, prev_transforms: HashMap::new(), history: HashMap::new(), slot: 0, geometry: HashMap::new(), attrs: Vec::new(), idx: Vec::new(), tags: Vec::new(), shared: None, retired: Vec::new(), retired_structures: Vec::new(), frame: 0, alignment: 256, env: None })
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
        self.slot = 0;
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
        for (i, (p, n)) in mesh.positions.iter().zip(&mesh.normals).enumerate() {
            let uv = mesh.uvs.get(i).copied().unwrap_or([0.0; 2]);
            self.attrs.push([p[0], p[1], p[2], uv[0]]);
            self.attrs.push([n[0], n[1], n[2], uv[1]]);
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
    pub fn trace(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, t: &Trace, offset: u32, size: (u32, u32), groups: gpu::Buffer, images: gpu::Buffer) {
        let mut instances = Vec::new();
        let mut blases = Vec::new();
        let mut inst_rows: Vec<[f32; 4]> = Vec::new();
        let mut next_transforms = HashMap::new();
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
            let custom = (inst_rows.len() / INST_ROWS) as u32;
            inst_rows.push([b(g.attr_base), b(g.idx_base), b(g.tag_base), 0.0]);
            inst_rows.push([p(0)[0], p(0)[1], p(1)[0], p(1)[1]]);
            inst_rows.push([p(2)[0], p(2)[1], p(3)[0], p(3)[1]]);
            inst_rows.push([p(4)[0], p(4)[1], p(5)[0], p(5)[1]]);
            // Where it was on the previous frame (object → camera then).
            let prev = self.prev_transforms.get(&inst.id).copied().unwrap_or(inst.transform);
            inst_rows.extend_from_slice(&prev);
            inst_rows.extend_from_slice(&inst.transform);
            inst_rows.extend_from_slice(&inst.world);
            next_transforms.insert(inst.id.clone(), inst.transform);
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
        // The environment image: uploaded once while it stays the same.
        let env_buf = match &t.env {
            Some(e) => {
                let key = Arc::as_ptr(e) as usize;
                if self.env.as_ref().map(|(k, _, _)| *k) != Some(key) {
                    if let Some((_, _, old)) = self.env.take() {
                        self.retired.push(old);
                    }
                    let b = context.create_buffer(gpu::BufferDesc { name: "trace environment", size: (e.trace.len() * 16) as u64, memory: gpu::Memory::Shared });
                    // SAFETY: a fresh mapped buffer of exactly this size.
                    unsafe { std::ptr::copy_nonoverlapping(e.trace.as_ptr() as *const u8, b.data(), e.trace.len() * 16) };
                    context.sync_buffer(b);
                    self.env = Some((key, e.clone(), b));
                }
                self.env.as_ref().map(|(_, _, b)| *b).unwrap()
            }
            None => self.transient(context, "trace no environment", &[0u8; 16]),
        };
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
        // Guides for the denoiser: albedo, normal, depth, motion per texel.
        let guides = context.create_buffer(gpu::BufferDesc { name: "trace guides", size: ((size.0 * size.1) as u64 * 64).max(16), memory: gpu::Memory::Device });
        self.retired.push(guides);
        // Lighting and its variance (two rows per texel), twice: the
        // denoiser's passes read one and write the other.
        let hdr_size = ((size.0 * size.1) as u64 * 32).max(16);
        let hdr = [0, 1].map(|_| {
            let b = context.create_buffer(gpu::BufferDesc { name: "trace hdr", size: hdr_size, memory: gpu::Memory::Device });
            self.retired.push(b);
            b
        });
        let r = |m: &[f32; 9]| [[m[0], m[1], m[2], 0.0], [m[3], m[4], m[5], 0.0], [m[6], m[7], m[8], 0.0]];
        // Temporal history for this trace (its order in the frame).
        let slot = self.slot;
        self.slot += 1;
        if t.options.temporal {
            self.prev_transforms.extend(next_transforms.iter().map(|(k, v)| (k.clone(), *v)));
        }
        let hist_size = ((size.0 * size.1) as u64 * 48).max(16);
        let (hist_read, hist_write, prev) = if t.options.temporal {
            let fresh = self.history.get(&slot).is_none_or(|h| h.size != size);
            if fresh {
                if let Some(old) = self.history.remove(&slot) {
                    self.retired.extend(old.buffers);
                }
                let make = || context.create_buffer(gpu::BufferDesc { name: "trace history", size: hist_size, memory: gpu::Memory::Device });
                self.history.insert(slot, History { size, buffers: [make(), make()], current: 0, cam: [0.0; 4], place: [0.0; 4], root: [[0.0; 4]; 3] });
            }
            let h = self.history.get_mut(&slot).unwrap();
            let prev = (h.cam, h.place, h.root);
            let (read, write) = (h.buffers[h.current], h.buffers[1 - h.current]);
            h.current = 1 - h.current;
            h.cam = [t.focal, t.centre[0], t.centre[1], 1.0];
            h.place = [t.origin[0], t.origin[1], t.factor[0], t.factor[1]];
            h.root = r(&t.root);
            (read, write, prev)
        } else {
            let b = self.transient(context, "trace no history", &[0u8; 16]);
            (b, b, ([0.0; 4], [0.0; 4], [[0.0; 4]; 3]))
        };
        let data = |step: u32, read: gpu::Buffer, write: gpu::Buffer| TraceData {
            u: Uniforms {
                size: [size.0, size.1, offset, t.options.spp.max(1)],
                opts: [t.options.bounces.max(1), t.seed, CARD_INSTANCE, t.cards.len() as u32],
                cam: [t.focal, t.centre[0], t.centre[1], 0.0],
                place: [t.origin[0], t.origin[1], t.factor[0], t.factor[1]],
                root: r(&t.root),
                inv_root: r(&t.inv_root),
                lens: [t.lens[0], t.lens[1], 0.0, 0.0],
                denoise: [step, t.options.temporal as u32, 0, 0],
                prev_cam: prev.0,
                prev_place: prev.1,
                prev_root: prev.2,
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
            b_env: env_buf.into(),
            b_hdr: read.into(),
            b_tmp: write.into(),
            b_hist: hist_read.into(),
            b_next: hist_write.into(),
            b_images: images.into(),
        };
        let groups_x = [size.0.div_ceil(8), size.1.div_ceil(8), 1];
        {
            let mut pass = encoder.compute("trace");
            let mut pc = pass.with(&self.pipeline);
            pc.bind(0, &data(0, hdr[0], hdr[1]));
            pc.dispatch(groups_x);
        }
        let mut current = 0;
        if t.options.temporal {
            let mut pass = encoder.compute("trace temporal");
            let mut pc = pass.with(&self.temporal);
            pc.bind(0, &data(0, hdr[0], hdr[1]));
            pc.dispatch(groups_x);
            current = 1;
        }
        if t.options.denoise {
            for k in 0..DENOISE_PASSES {
                let mut pass = encoder.compute("trace denoise");
                let mut pc = pass.with(&self.atrous);
                pc.bind(0, &data(1 << k, hdr[current], hdr[1 - current]));
                pc.dispatch(groups_x);
                current = 1 - current;
            }
        }
        let mut pass = encoder.compute("trace resolve");
        let mut pc = pass.with(&self.resolve);
        pc.bind(0, &data(0, hdr[current], hdr[1 - current]));
        pc.dispatch(groups_x);
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
        if let Some((_, _, b)) = self.env.take() {
            context.destroy_buffer(b);
        }
        for (_, h) in self.history.drain() {
            for b in h.buffers {
                context.destroy_buffer(b);
            }
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

    /// The tracer never reads a committed intersection. naga 25's SPIR-V
    /// writer shares one helper between candidate and committed reads, so
    /// with both in the shader the committed read returns the candidate: on
    /// hardware ray tracing (NVIDIA) every miss became a hit at t = 0 and
    /// every lit frame came out black. Software Vulkan (CI) can't show it,
    /// so this guards the source.
    #[test]
    fn the_tracer_reads_candidates_only() {
        assert!(SHADER.contains("rayQueryGetCandidateIntersection"));
        assert!(!SHADER.contains("rayQueryGetCommittedIntersection"), "see cast_ray: naga 25 miscompiles committed reads next to candidate reads");
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
        let r = Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, path: Some(super::TraceOptions { spp, bounces: 4, ..Default::default() }), ..Default::default() });
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

    /// The principled materials, traced and rasterised, to $LAHZA_PT_OUT
    /// ($LAHZA_PT_SPP samples, default 128; $LAHZA_PT_RAW=1: no denoiser).
    #[test]
    #[ignore]
    fn dump_materials() {
        let spp = std::env::var("LAHZA_PT_SPP").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        let Some(mut r) = tracer(spp) else { return };
        if std::env::var_os("LAHZA_PT_RAW").is_some() {
            r.options.path = Some(super::TraceOptions { spp, bounces: 4, denoise: false, temporal: false });
        }
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

    /// Area lights and an HDR environment, traced and rasterised, to
    /// $LAHZA_PT_OUT.
    #[test]
    #[ignore]
    fn dump_lights() {
        let Some(mut r) = tracer(128) else { return };
        let dir = std::env::temp_dir().join(format!("lahza-dumplights-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sun = dir.join("sun.hdr");
        crate::envmap::tests::write_hdr(&sun, 60.0);
        let mut p = project(json!([
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#c8c2b8"}, "transform": {"position": [480, 235], "rotation_x": 90}},
            {"id": "chrome", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": {"preset": "chrome"}, "transform": {"position": [260, 175]}},
            {"id": "gold", "kind": "mesh", "mesh": {"type": "torus", "radius": 55, "tube": 20}, "material": {"preset": "metal", "color": "#ffc356", "roughness": 0.2}, "transform": {"position": [480, 160], "rotation_x": 10}},
            {"id": "matte", "kind": "mesh", "mesh": {"type": "box", "size": [90, 90], "depth": 90, "bevel": 12}, "material": {"preset": "plastic", "color": "#e8e8e8"}, "transform": {"position": [700, 190], "rotation_y": 30}}
        ]));
        p.settings.width = 960;
        p.settings.height = 300;
        p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 12]})).unwrap();
        p.scenes[0].lights = serde_json::from_value(json!([
            {"type": "area", "position": [200, -150, -300], "aim": [480, 200, 0], "size": [500, 200], "intensity": 0.8, "shadow": true},
            {"type": "area", "position": [950, 100, -100], "aim": [600, 200, 0], "size": [120, 120], "shape": "disc", "color": "#7fb0ff", "intensity": 0.6}
        ])).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(json!({"image": sun.to_str().unwrap(), "intensity": 0.6})).unwrap());
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let traced = r.render(&c, 0.0);
        let raster = cpu().render(&c, 0.0);
        let mut both = image::RgbaImage::new(960, 600);
        image::imageops::overlay(&mut both, &traced, 0, 0);
        image::imageops::overlay(&mut both, &raster, 0, 300);
        both.save(std::env::var("LAHZA_PT_OUT").unwrap_or_else(|_| "/tmp/pt_lights.png".into())).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
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

    /// An environment image: traced reflections show it, a matte ball
    /// lit by it agrees with the raster, and its small bright sun is
    /// sampled directly (clean at a few samples per pixel).
    #[test]
    fn environment_images_are_traced_and_sampled() {
        let Some(mut r) = tracer(16) else { return };
        let dir = std::env::temp_dir().join(format!("lahza-envpt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (sky, sun) = (dir.join("sky.hdr"), dir.join("sun.hdr"));
        crate::envmap::tests::write_hdr(&sky, 0.0);
        crate::envmap::tests::write_hdr(&sun, 400.0);
        let none = json!([{"type": "ambient", "intensity": 0}]);
        let img = r.render(&Compiled::new(&sphere_in(json!({"preset": "metal", "color": "#ffffff", "roughness": 0}), none.clone(), json!({"image": sky.to_str().unwrap()}))), 0.0);
        let (top, bottom) = (img.get_pixel(160, 70).0, img.get_pixel(160, 170).0);
        assert!(top[0] > 150 && top[2] < 80 && bottom[2] > 150 && bottom[0] < 80, "mirrored: {top:?} {bottom:?}");
        let c = Compiled::new(&sphere_in(json!({"preset": "matte", "color": "#ffffff", "specular": 0}), none.clone(), json!({"image": sky.to_str().unwrap()})));
        let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
        // Patch means (single pixels carry the samples' noise).
        for (name, keep) in [("top", (|_: f64, dy: f64| dy < -40.0) as fn(f64, f64) -> bool), ("bottom", |_, dy| dy > 40.0), ("middle", |_, dy| dy.abs() < 20.0)] {
            let (ta, tb) = (disc(&a, 160.0, 120.0, 0.0, 85.0, keep), disc(&b, 160.0, 120.0, 0.0, 85.0, keep));
            for k in 0..3 {
                assert!((ta[k] - tb[k]).abs() <= 8.0, "{name}: traced {ta:?}, raster {tb:?}");
            }
        }
        // The sun, on a wall facing it.
        let mut p = project(json!([{"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [2000, 2000]}, "material": {"preset": "matte", "color": "#ffffff", "specular": 0}, "transform": {"position": [160, 120]}}]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].lights = serde_json::from_value(none).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(json!({"image": sun.to_str().unwrap()})).unwrap());
        let img = r.render(&Compiled::new(&p), 0.0);
        let patch: Vec<f64> = (100..140u32).flat_map(|y| (140..180u32).map(move |x| (x, y))).map(|(x, y)| img.get_pixel(x, y).0[1] as f64).collect();
        let mean = patch.iter().sum::<f64>() / patch.len() as f64;
        let sd = (patch.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / patch.len() as f64).sqrt();
        assert!(mean > 60.0 && sd < 0.08 * mean, "sunlit and clean: mean {mean:.1}, sd {sd:.1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Width (px) of a silhouette's edge along a row: columns between 10%
    /// and 90% of the way from the background to the inside.
    fn edge_width(img: &image::RgbaImage, y: u32, xs: std::ops::Range<u32>) -> usize {
        let v: Vec<f64> = xs.map(|x| img.get_pixel(x, y).0[0] as f64).collect();
        let (lo, hi) = (v.iter().cloned().fold(f64::MAX, f64::min), v.iter().cloned().fold(0.0, f64::max));
        v.iter().filter(|c| **c > lo + 0.1 * (hi - lo) && **c < lo + 0.9 * (hi - lo)).count()
    }

    /// Depth of field through a thin lens: the ball in focus keeps a sharp
    /// edge, the far one blurs about as much as the raster blurs it.
    #[test]
    fn a_lens_focuses_and_defocuses() {
        let Some(mut r) = tracer(64) else { return };
        let mut p = project(json!([
            {"id": "far", "kind": "mesh", "mesh": {"type": "sphere", "radius": 50}, "material": {"color": "#ff0000", "unlit": true}, "transform": {"position": [230, 120], "z": 600}},
            {"id": "near", "kind": "mesh", "mesh": {"type": "sphere", "radius": 50}, "material": {"color": "#ff0000", "unlit": true}, "transform": {"position": [80, 120]}}
        ]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].camera = serde_json::from_value(json!({"aperture": 1.5, "focus": "near"})).unwrap();
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let (traced, raster) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
        let near = edge_width(&traced, 120, 110..160);
        let far = edge_width(&traced, 120, 186..260);
        let raster_far = edge_width(&raster, 120, 186..260);
        assert!(near <= 3, "in focus: {near} px");
        assert!(far >= 5, "out of focus: {far} px (raster {raster_far} px)");
        assert!((far as f64) > 0.4 * raster_far as f64 && (far as f64) < 2.5 * raster_far as f64, "traced {far} px vs raster {raster_far} px");
    }

    /// Motion blur: each shutter sub-frame traces its share of the samples
    /// and a fast ball streaks.
    #[test]
    fn moving_meshes_streak() {
        let Some(mut r) = tracer(32) else { return };
        let mut p = project(json!([{"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"color": "#ff0000", "unlit": true},
            "transform": {"position": {"keys": [{"at": 0, "v": [60, 120]}, {"at": 1, "v": [260, 120]}]}}}]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 360.0, samples: 8 });
        p.settings.fps = lahza_motion_core::time::Fps::whole(4);
        let img = r.render(&Compiled::new(&p), 0.5);
        let covered = (0..320u32).filter(|&x| img.get_pixel(x, 120).0[0] > 20).count();
        assert!(covered > 90, "the streak covers {covered} px (still: 60)");
    }

    /// The denoiser: at 4 samples per pixel a softly lit floor is far
    /// smoother with it, as bright on average, and a ball's silhouette
    /// against it stays sharp.
    #[test]
    fn the_denoiser_smooths_without_blurring_edges() {
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        let with = |denoise: bool| {
            let r = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, path: Some(super::TraceOptions { spp: 4, bounces: 4, denoise, temporal: false }), ..Default::default() });
            r.can_trace().then_some(r)
        };
        let (Some(mut on), Some(mut off)) = (with(true), with(false)) else { return };
        let mut p = project(json!([
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#d0d0d0"}, "transform": {"position": [160, 120]}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 50}, "material": {"preset": "matte", "color": "#c03030"}, "transform": {"position": [160, 120], "z": -80}}
        ]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "soft"})).unwrap());
        let c = Compiled::new(&p);
        let (a, b) = (on.render(&c, 0.0), off.render(&c, 0.0));
        let stats = |img: &image::RgbaImage| {
            let v: Vec<f64> = (20..60u32).flat_map(|y| (20..60u32).map(move |x| (x, y))).map(|(x, y)| img.get_pixel(x, y).0[1] as f64).collect();
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (m, (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt())
        };
        let ((ma, sa), (mb, sb)) = (stats(&a), stats(&b));
        assert!(sa < sb / 3.0, "smoother: sd {sa:.1} with, {sb:.1} without");
        assert!((ma - mb).abs() < 0.04 * mb, "as bright: {ma:.1} with, {mb:.1} without");
        // The ball's left edge: from floor to red within a few px.
        let row: Vec<i32> = (60..130u32).map(|x| a.get_pixel(x, 120).0[1] as i32).collect();
        let floor_g = row[0];
        let ball_g = row[row.len() - 1];
        let between = row.iter().filter(|g| (**g - floor_g).abs() > (floor_g - ball_g).abs() / 5 && (**g - ball_g).abs() > (floor_g - ball_g).abs() / 5).count();
        assert!(between <= 4, "a sharp silhouette: {between} px between floor and ball ({row:?})");
    }

    /// Temporal accumulation over a sequence: a still floor flickers far
    /// less from frame to frame, and a moving ball leaves no ghost where it
    /// was.
    #[test]
    fn sequences_accumulate_without_ghosts() {
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        let make = |temporal: bool| {
            let r = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, motion_blur: false, path: Some(super::TraceOptions { spp: 2, bounces: 3, denoise: true, temporal }), ..Default::default() });
            r.can_trace().then_some(r)
        };
        let (Some(mut on), Some(mut off)) = (make(true), make(false)) else { return };
        let mut p = project(json!([
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#d0d0d0"}, "transform": {"position": [160, 120]}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"preset": "matte", "color": "#ff2020"},
             "transform": {"position": {"keys": [{"at": 0, "v": [80, 120]}, {"at": 1, "v": [240, 120]}]}, "z": -60}}
        ]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.settings.fps = lahza_motion_core::time::Fps::whole(10);
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "soft"})).unwrap());
        let c = Compiled::new(&p);
        let flicker = |r: &mut Renderer| {
            let frames: Vec<image::RgbaImage> = (0..10).map(|k| r.render(&c, k as f64 / 10.0)).collect();
            let patch = |img: &image::RgbaImage| (180..220u32).flat_map(|y| (20..60u32).map(move |x| (x, y))).map(|(x, y)| img.get_pixel(x, y).0[1] as f64).collect::<Vec<_>>();
            let (a, b) = (patch(&frames[8]), patch(&frames[9]));
            (a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64, frames)
        };
        let (still, frames) = flicker(&mut on);
        let (boil, _) = flicker(&mut off);
        assert!(still < 0.6 * boil, "flicker {still:.2} with history, {boil:.2} without");
        // Frame 9: the ball is at x ≈ 224; where it was at frame 0 (x 80)
        // is floor again, not a red trail.
        let old = frames[9].get_pixel(80, 120).0;
        assert!((old[0] as i32 - old[1] as i32).abs() < 25, "no ghost: {old:?}");
    }

    /// Surface detail traces as it rasterises: a noise-coloured ball and a
    /// checker-mapped, bump-mapped wall agree, and the denoiser keeps the
    /// checker's edges (the albedo guide carries the map).
    #[test]
    fn surface_detail_is_traced() {
        let Some(mut r) = tracer(16) else { return };
        let dir = std::env::temp_dir().join(format!("lahza-ptdetail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tex = dir.join("check.png");
        image::RgbaImage::from_fn(64, 64, |x, y| image::Rgba(if (x / 16 + y / 16) % 2 == 0 { [240, 240, 240, 255] } else { [30, 30, 30, 255] })).save(&tex).unwrap();
        let mut p = project(json!([
            {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [140, 140]}, "material": {"preset": "matte", "color": "#ffffff", "map": tex.to_str().unwrap(), "specular": 0}, "transform": {"position": [90, 120]}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": {"preset": "matte", "color": "#ffffff", "specular": 0, "noise": {"scale": 20, "color": "#ff2020", "seed": 4}}, "transform": {"position": [240, 120]}}
        ]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [0, 0, -1], "shadow": false}])).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
        let c = Compiled::new(&p);
        let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
        let changes = |img: &image::RgbaImage| (25..155u32).filter(|&x| (img.get_pixel(x, 121).0[1] > 100) != (img.get_pixel(x + 1, 121).0[1] > 100)).count();
        assert_eq!(changes(&a), changes(&b), "the checker's edges, traced and rasterised");
        assert_eq!(changes(&b), 3);
        for (name, cx) in [("wall", 90.0), ("ball", 240.0)] {
            let (ma, mb) = (disc(&a, cx, 120.0, 0.0, 50.0, |_, _| true), disc(&b, cx, 120.0, 0.0, 50.0, |_, _| true));
            for k in 0..3 {
                assert!((ma[k] - mb[k]).abs() < 10.0, "{name}: traced {ma:?}, raster {mb:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The scene's look (AgX, exposure) applies to the traced picture as to
    /// the raster.
    #[test]
    fn traced_pictures_take_the_look() {
        let Some(mut r) = tracer(32) else { return };
        let mut p = sphere_in(json!({"preset": "matte", "color": "#ff8040", "specular": 0}), json!([{"type": "directional", "from": [0, 0, -1], "intensity": 3, "shadow": false}]),
            json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"}));
        p.scenes[0].look = Some(serde_json::from_value(json!({"tone": "agx", "exposure": 0.5})).unwrap());
        let c = Compiled::new(&p);
        let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
        let (ma, mb) = (disc(&a, 160.0, 120.0, 0.0, 70.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 70.0, |_, _| true));
        for k in 0..3 {
            assert!((ma[k] - mb[k]).abs() < 8.0, "traced {ma:?}, raster {mb:?}");
        }
    }

    /// Volumes, traced: a glowing beam (no scattering) gives off what the
    /// raster's estimate gives (the same integral), and a lit box of haze
    /// scatters light toward the camera.
    #[test]
    fn volumes_are_traced() {
        let Some(mut r) = tracer(64) else { return };
        let setup = |material: serde_json::Value, lights: serde_json::Value| {
            let mut p = project(json!([{"id": "v", "kind": "mesh", "mesh": {"type": "box", "size": [160, 120], "depth": 160}, "material": material, "transform": {"position": [160, 120]}}]));
            p.settings.width = 320;
            p.settings.height = 240;
            p.scenes[0].lights = serde_json::from_value(lights).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
            Compiled::new(&p)
        };
        let glow = setup(json!({"volume": {"density": 0.8, "color": "#000000", "emission": "#ff6020"}}), json!([{"type": "ambient", "intensity": 0}]));
        let (a, b) = (r.render(&glow, 0.0), cpu().render(&glow, 0.0));
        let (ma, mb) = (disc(&a, 160.0, 120.0, 0.0, 40.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 40.0, |_, _| true));
        for k in 0..3 {
            assert!((ma[k] - mb[k]).abs() < 10.0, "glow: traced {ma:?}, raster {mb:?}");
        }
        let lit = setup(json!({"volume": {"density": 1.0, "color": "#ffffff", "anisotropy": 0.5}}), json!([{"type": "directional", "from": [0.2, -0.3, 1], "intensity": 2, "shadow": true}]));
        let img = r.render(&lit, 0.0);
        let inside = disc(&img, 160.0, 120.0, 0.0, 40.0, |_, _| true);
        let outside = disc(&img, 20.0, 20.0, 0.0, 10.0, |_, _| true);
        assert!(inside[1] > outside[1] + 20.0, "lit haze shows: {inside:?} vs {outside:?}");
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
            let id = layer["id"].as_str().unwrap();
            assert!(r.warnings().iter().any(|w| w.contains("rasterised") && w.contains(id)), "the report names '{id}': {:?}", r.warnings());
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
