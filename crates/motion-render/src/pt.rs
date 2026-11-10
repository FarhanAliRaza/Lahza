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

/// Paths per trace dispatch, at most (see `band_rows`).
const PATHS_PER_DISPATCH: u64 = 8 << 20;

/// Rows of a `width`-texel group traced per dispatch at `spp` paths per
/// texel: whole 8-row workgroups, at most `PATHS_PER_DISPATCH` paths
/// (one band of 8 rows when even that is more).
fn band_rows(width: u32, spp: u32) -> u32 {
    let per_row = width.max(1) as u64 * spp.max(1) as u64;
    ((PATHS_PER_DISPATCH / per_row) as u32 / 8 * 8).max(8)
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
    /// The project's Slang surfaces its meshes use (traced with a variant
    /// of the trace shader, `surfaces::trace_source`).
    pub surfaces: Option<Arc<lahza_motion_core::surface::Program>>,
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
        if let Some(s) = &self.surfaces {
            out.extend(s.key.to_le_bytes());
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
    /// The rows the trace pass covers: [first, end, the profiling density
    /// experiment's repeats, 0] (the pass runs in bands).
    band: [u32; 4],
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
    b_prof: gpu::BufferPiece,
    b_bounds: gpu::BufferPiece,
    b_jobs: gpu::BufferPiece,
}

/// A mesh's bottom-level structure and where its attributes live.
struct Geometry {
    blas: gpu::AccelerationStructure,
    /// Its device address (what instances refer to it by).
    address: u64,
    positions: gpu::Buffer,
    indices: gpu::Buffer,
    /// Offsets (in vertices, indices and triangles) into the shared arrays,
    /// and how many of each it has there.
    attr_base: u32,
    idx_base: u32,
    tag_base: u32,
    counts: [u32; 3],
    used: u64,
}

/// What the tracer frees once the frames that could read it are done.
enum Retired {
    Buffer(gpu::Buffer),
    Structure(gpu::AccelerationStructure),
    Pipeline(gpu::ComputePipeline),
}

/// A buffer kept from frame to frame: made on first use and when its
/// contents outgrow it, otherwise written in place.
#[derive(Default)]
struct Kept {
    buffer: Option<gpu::Buffer>,
    capacity: u64,
}

impl Kept {
    /// Room for `size` bytes in `memory` (contents undefined when it had to grow).
    fn room(&mut self, context: &gpu::Context, name: &str, size: u64, memory: gpu::Memory, retired: &mut Vec<Retired>) -> gpu::Buffer {
        let need = size.max(16);
        match self.buffer {
            Some(b) if self.capacity >= need => b,
            _ => {
                retired.extend(self.buffer.take().map(Retired::Buffer));
                self.capacity = need.next_power_of_two();
                let b = context.create_buffer(gpu::BufferDesc { name, size: self.capacity, memory });
                self.buffer = Some(b);
                b
            }
        }
    }

    /// The buffer (host-visible) holding `bytes`.
    fn write(&mut self, context: &gpu::Context, name: &str, bytes: &[u8], retired: &mut Vec<Retired>) -> gpu::Buffer {
        let b = self.room(context, name, bytes.len() as u64, gpu::Memory::Shared, retired);
        // SAFETY: a mapped buffer of at least bytes.len() bytes that no
        // frame in flight reads (its set is this frame's alone).
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.data(), bytes.len()) };
        context.sync_buffer(b);
        b
    }

    fn destroy(&mut self, context: &gpu::Context) {
        if let Some(b) = self.buffer.take() {
            context.destroy_buffer(b);
        }
    }
}

/// One instance of a top-level structure, as Vulkan lays it out
/// (`VkAccelerationStructureInstanceKHR`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRecord {
    transform: [[f32; 4]; 3],
    /// Custom index (24 bits) and visibility mask (8).
    custom_mask: u32,
    sbt_flags: u32,
    address: u64,
}

/// One trace of a frame: a traced list at one moment (a motion-blurred
/// frame's sub-frames are a part each). Its tables and its structure over
/// the instances, kept between frames.
#[derive(Default)]
struct Unit {
    inst: Kept,
    mat: Kept,
    light: Kept,
    card: Kept,
    jobs: Kept,
    card_verts: Kept,
    card_idx: Kept,
    counts: Kept,
    instances: Kept,
    bounds: Kept,
    tlas_scratch: Kept,
    cards_scratch: Kept,
    /// The top-level structure and the instances it has room for.
    tlas: Option<(gpu::AccelerationStructure, u32)>,
    /// The cards' structure and the cards it has room for.
    cards: Option<(gpu::AccelerationStructure, u32)>,
    /// What the structures were last built from (instances, cards): the
    /// same again needs no build.
    built: Option<u64>,
}

impl Unit {
    fn destroy(&mut self, context: &gpu::Context) {
        for k in [&mut self.inst, &mut self.mat, &mut self.light, &mut self.card, &mut self.jobs, &mut self.card_verts, &mut self.card_idx, &mut self.counts, &mut self.instances, &mut self.bounds, &mut self.tlas_scratch, &mut self.cards_scratch] {
            k.destroy(context);
        }
        for (a, _) in self.tlas.take().into_iter().chain(self.cards.take()) {
            context.destroy_acceleration_structure(a);
        }
    }
}

/// A traced list's lighting and guides (see pt.wgsl `trace`): every
/// part of the frame adds to them, then they are filtered. With Open
/// Image Denoise, also what goes to it and comes back.
struct Work {
    texels: u64,
    guides: gpu::Buffer,
    hdr: [gpu::Buffer; 2],
    oidn: Option<Denoising>,
}

/// How a traced list's picture gets to Open Image Denoise and back, for
/// pictures of `size`.
#[derive(Clone, Copy)]
struct Denoising {
    size: (u32, u32),
    /// Colour, albedo and normal packed for the denoiser, on the device.
    packed: gpu::Buffer,
    way: Way,
}

#[derive(Clone, Copy)]
enum Way {
    /// Through the host: the packed picture's host copy, and the denoised
    /// colour to upload (into `packed`).
    Copied { down: gpu::Buffer, up: gpu::Buffer },
    /// In place: `packed` and `output` are memories the denoiser imported
    /// (its filter `id` reads one and writes the other on the device).
    /// `up` takes a result that came the copied way after all (the first
    /// shared picture is checked against it).
    Shared { output: gpu::Buffer, id: u64, up: Option<gpu::Buffer> },
}

impl Denoising {
    fn buffers(&self) -> impl Iterator<Item = gpu::Buffer> {
        let rest = match self.way {
            Way::Copied { down, up } => [Some(down), Some(up)],
            Way::Shared { output, up, .. } => [Some(output), up],
        };
        std::iter::once(self.packed).chain(rest.into_iter().flatten())
    }
}

impl Work {
    fn buffers(&self) -> impl Iterator<Item = gpu::Buffer> + '_ {
        [self.guides, self.hdr[0], self.hdr[1]].into_iter().chain(self.oidn.iter().flat_map(|d| d.buffers()))
    }
}

/// Whether the denoiser works on the tracer's own memory.
#[derive(Clone, Debug, PartialEq)]
enum Sharing {
    /// Not tried yet: the first shared picture is checked against the
    /// same picture denoised the copied way.
    Untried,
    /// Checked: pictures are denoised in place.
    Checked,
    /// Not possible here, or it failed (why): pictures are copied.
    Off(String),
}

/// The buffers a traced list's passes bind (those of its last part).
#[derive(Clone, Copy)]
struct Bindings {
    tlas: gpu::AccelerationStructure,
    inst: gpu::Buffer,
    attr: gpu::Buffer,
    idx: gpu::Buffer,
    tag: gpu::Buffer,
    mat: gpu::Buffer,
    light: gpu::Buffer,
    card: gpu::Buffer,
    env: gpu::Buffer,
    counts: gpu::Buffer,
    bounds: gpu::Buffer,
    jobs: gpu::Buffer,
}

/// A traced list between its parts and its picture.
struct Slot {
    size: (u32, u32),
    uniforms: Uniforms,
    bind: Bindings,
    temporal: bool,
    denoise: bool,
    hist: (gpu::Buffer, gpu::Buffer),
    /// Which of the work's two lighting buffers holds the lighting now.
    current: usize,
    /// The lighting went to Open Image Denoise and came back.
    denoised: bool,
    /// Its result is in the upload buffer (the copied way).
    upload: bool,
}

/// What one frame in flight writes from the CPU.
#[derive(Default)]
struct Set {
    units: Vec<Unit>,
    /// The next trace part's unit.
    unit: usize,
    work: Vec<Option<Work>>,
    slots: Vec<Option<Slot>>,
    /// Profiling: per part, its work counters (ray queries; per material,
    /// hits and density lookups) and the layer that owns each material.
    counts: Vec<(gpu::Buffer, Vec<Arc<str>>, usize)>,
}

/// GPU side of the path tracer, owned by `gpu::Gpu`.
///
/// A traced list is recorded in steps, so that the sub-frames of a
/// motion-blurred frame share one picture and so that frames overlap:
/// `part` traces the list at one moment into its lighting and guides (with
/// that moment's own top-level structure and tables), `settle` ends the
/// parts (temporal accumulation, and the lighting on its way to Open Image
/// Denoise), `denoise` runs OIDN once the GPU has got that far, and `post`
/// filters and writes the picture into the group.
///
/// Everything a frame writes from the CPU lives in a `Set`: frames in
/// flight use different sets, so recording one does not disturb another
/// still on the GPU. Buffers and structures are kept from frame to frame
/// rather than made and freed each time.
pub struct Tracer {
    pipeline: gpu::ComputePipeline,
    temporal: gpu::ComputePipeline,
    atrous: gpu::ComputePipeline,
    resolve: gpu::ComputePipeline,
    /// Bakes noise and grid volumes' density bounds (`bounds` in pt.wgsl).
    bounds: gpu::ComputePipeline,
    /// To and from Open Image Denoise (`oidn_pack`, `oidn_unpack`).
    pack: gpu::ComputePipeline,
    unpack: gpu::ComputePipeline,
    /// Trace pipelines specialised to a scene's features (`specialise`), by
    /// feature mask (None: that build failed); `pipeline` is the general
    /// build (all features).
    specialised: HashMap<u32, Option<gpu::ComputePipeline>>,
    /// Trace pipelines with a project's Slang surfaces
    /// (`surfaces::trace_source`): the program's key and its builds by
    /// feature mask. Another program's replace them.
    slang: Option<(u64, HashMap<u32, gpu::ComputePipeline>)>,
    /// Whether these pipelines count work (`PROFILE`).
    profile: bool,
    geometry: HashMap<Vec<u8>, Geometry>,
    /// Per-vertex [position, normal] rows, indices and triangle tags of every
    /// cached mesh, uploaded again when a mesh is added.
    attrs: Vec<[f32; 4]>,
    idx: Vec<u32>,
    tags: Vec<u32>,
    shared: Option<[gpu::Buffer; 3]>,
    /// What frames in flight may still read, with the last frame that
    /// could: freed once that frame is done (`begin_frame`).
    retired: Vec<(u64, Retired)>,
    /// The frame being recorded (the owner's count), and how many frames
    /// this tracer has seen (geometry ages by it).
    frame: u64,
    age: u64,
    /// Scratch addresses must be aligned to this for builds.
    alignment: u64,
    /// The environment image's rows on the GPU, by the map's address.
    env: Option<(usize, Arc<crate::envmap::EnvMap>, gpu::Buffer)>,
    /// Each mesh layer's object → camera transform on the last frame.
    prev_transforms: HashMap<Arc<str>, [[f32; 4]; 3]>,
    /// Temporal history per traced list of the frame (by its order).
    history: HashMap<u32, History>,
    /// Traced lists of the frame being recorded.
    slots: u32,
    sets: Vec<Set>,
    set: usize,
    /// Sixteen zero bytes, for the bindings a trace does not use.
    dummy: gpu::Buffer,
    /// Open Image Denoise, loaded on first use (None: not tried yet).
    oidn: Option<Result<crate::oidn::Denoiser, String>>,
    sharing: Sharing,
}

/// Geometry not used for this many frames is dropped. Animated meshes
/// are new geometry every frame, so the cache must not outlive them.
/// (The frames in flight, at most the two before the one being recorded,
/// used theirs later than that.)
const GEOMETRY_FRAMES: u64 = 2;

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
    /// `profile` builds the counting variant of the shader (`PROFILE`).
    pub fn new(context: &gpu::Context, profile: bool) -> Result<Self, String> {
        let source = specialise(ALL_FEATURES, profile);
        let shader = context.try_create_shader(gpu::ShaderDesc { source: &source })?;
        let make = |name: &str, entry: &str| context.create_compute_pipeline(gpu::ComputePipelineDesc { name, data_layouts: &[&TraceData::layout()], compute: shader.at(entry) });
        let dummy = context.create_buffer(gpu::BufferDesc { name: "trace nothing", size: 16, memory: gpu::Memory::Shared });
        // SAFETY: a fresh mapped buffer of 16 bytes.
        unsafe { std::ptr::write_bytes(dummy.data(), 0, 16) };
        context.sync_buffer(dummy);
        Ok(Self {
            pipeline: make("trace", "trace"),
            temporal: make("trace temporal", "temporal"),
            atrous: make("trace denoise", "atrous"),
            resolve: make("trace resolve", "resolve"),
            bounds: make("volume bounds", "bounds"),
            pack: make("oidn pack", "oidn_pack"),
            unpack: make("oidn unpack", "oidn_unpack"),
            specialised: HashMap::new(),
            slang: None,
            profile,
            geometry: HashMap::new(),
            attrs: Vec::new(),
            idx: Vec::new(),
            tags: Vec::new(),
            shared: None,
            retired: Vec::new(),
            frame: 0,
            age: 0,
            alignment: 256,
            env: None,
            prev_transforms: HashMap::new(),
            history: HashMap::new(),
            slots: 0,
            sets: Vec::new(),
            set: 0,
            dummy,
            oidn: None,
            sharing: Sharing::Untried,
        })
    }

    fn retire(&mut self, what: Retired) {
        self.retired.push((self.frame, what));
    }

    /// Starts recording frame `frame` into set `set`. Frames up to
    /// `completed` have finished on the GPU: what only they could still
    /// read is freed.
    pub(crate) fn begin_frame(&mut self, context: &gpu::Context, frame: u64, completed: u64, set: usize) {
        let (done, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.retired).into_iter().partition(|(f, _)| *f <= completed);
        self.retired = keep;
        for (_, r) in done {
            match r {
                Retired::Buffer(b) => context.destroy_buffer(b),
                Retired::Structure(a) => context.destroy_acceleration_structure(a),
                Retired::Pipeline(mut p) => context.destroy_compute_pipeline(&mut p),
            }
        }
        // History of traces the last frame no longer had (fewer lists).
        let used = self.slots;
        let gone: Vec<u32> = self.history.keys().copied().filter(|k| *k >= used).collect();
        // (What frames before this one may still read: freed after them.)
        let before = frame.saturating_sub(1);
        for k in gone {
            if let Some(h) = self.history.remove(&k) {
                self.retired.extend(h.buffers.map(|b| (before, Retired::Buffer(b))));
            }
        }
        self.frame = frame;
        self.age += 1;
        self.slots = 0;
        if self.sets.len() <= set {
            self.sets.resize_with(set + 1, Set::default);
        }
        self.set = set;
        let s = &mut self.sets[set];
        s.unit = 0;
        s.slots.clear();
        s.counts.clear();
        let age = self.age;
        let stale: Vec<Vec<u8>> = self.geometry.iter().filter(|(_, g)| age - g.used > GEOMETRY_FRAMES).map(|(k, _)| k.clone()).collect();
        if stale.is_empty() {
            return;
        }
        // (The frames in flight were recorded after these were last used.)
        for k in stale {
            if let Some(g) = self.geometry.remove(&k) {
                context.destroy_acceleration_structure(g.blas);
                context.destroy_buffer(g.positions);
                context.destroy_buffer(g.indices);
            }
        }
        // Pack the shared arrays down to the geometry that is left: they
        // only ever grew, so an animated mesh filled the GPU.
        let (mut attrs, mut idx, mut tags) = (Vec::new(), Vec::new(), Vec::new());
        for g in self.geometry.values_mut() {
            let [nv, ni, nt] = g.counts.map(|c| c as usize);
            let (a, i, t) = (g.attr_base as usize * 2, g.idx_base as usize, g.tag_base as usize);
            g.attr_base = (attrs.len() / 2) as u32;
            g.idx_base = idx.len() as u32;
            g.tag_base = tags.len() as u32;
            attrs.extend_from_slice(&self.attrs[a..a + nv * 2]);
            idx.extend_from_slice(&self.idx[i..i + ni]);
            tags.extend_from_slice(&self.tags[t..t + nt]);
        }
        (self.attrs, self.idx, self.tags) = (attrs, idx, tags);
        // The frames in flight still read the old arrays.
        self.retired.extend(self.shared.take().into_iter().flatten().map(|b| (before, Retired::Buffer(b))));
    }

    /// Vertices held for traced geometry (for tests: the cache stays bounded).
    #[cfg(test)]
    pub(crate) fn cached_vertices(&self) -> usize {
        self.attrs.len() / 2
    }

    /// Buffers the tracer holds for its traces, over every set (tests:
    /// they are kept, not made again each frame).
    #[cfg(test)]
    pub(crate) fn kept_buffers(&self) -> Vec<gpu::Buffer> {
        let mut out = Vec::new();
        for s in &self.sets {
            for u in &s.units {
                out.extend([&u.inst, &u.mat, &u.light, &u.card, &u.jobs, &u.instances, &u.tlas_scratch].iter().filter_map(|k| k.buffer));
            }
            out.extend(s.work.iter().flatten().flat_map(|w| w.buffers()));
        }
        out
    }

    /// Scratch for one bottom-level build, freed after this frame.
    fn scratch(&mut self, context: &gpu::Context, size: u64) -> gpu::Buffer {
        let b = context.create_buffer(gpu::BufferDesc { name: "trace scratch", size: size.max(self.alignment) + self.alignment, memory: gpu::Memory::Device });
        self.retire(Retired::Buffer(b));
        b
    }

    /// Builds (once) the bottom-level structure of a mesh. Returns where
    /// its attributes, indices and tags start and its structure's address;
    /// None for a mesh without triangles.
    fn geometry(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, key: &[u8], mesh: &Mesh) -> Option<([u32; 3], u64)> {
        if let Some(g) = self.geometry.get_mut(key) {
            g.used = self.age;
            return Some(([g.attr_base, g.idx_base, g.tag_base], g.address));
        }
        if mesh.tris.is_empty() {
            return None;
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
        // still be read by work recorded earlier).
        for b in self.shared.take().into_iter().flatten() {
            self.retire(Retired::Buffer(b));
        }
        let address = context.acceleration_structure_address(blas);
        self.geometry.insert(key.to_vec(), Geometry { blas, address, positions: pb, indices: ib, attr_base, idx_base, tag_base, counts: [mesh.positions.len() as u32, indices.len() as u32, mesh.tags.len() as u32], used: self.age });
        Some(([attr_base, idx_base, tag_base], address))
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

    /// The lighting and guides of traced list `slot` in this frame's set,
    /// with room for `texels`.
    fn work(&mut self, context: &gpu::Context, slot: u32, texels: u64) -> (gpu::Buffer, [gpu::Buffer; 2]) {
        let s = slot as usize;
        if self.sets[self.set].work.len() <= s {
            self.sets[self.set].work.resize_with(s + 1, || None);
        }
        if self.sets[self.set].work[s].as_ref().is_none_or(|w| w.texels < texels) {
            let old = self.sets[self.set].work[s].take();
            if let Some(d) = old.as_ref().and_then(|w| w.oidn) {
                self.release(d);
            }
            for b in old.iter().flat_map(|w| w.buffers()) {
                self.retire(Retired::Buffer(b));
            }
            let make = |name, per: u64| context.create_buffer(gpu::BufferDesc { name, size: (texels * per).max(16), memory: gpu::Memory::Device });
            self.sets[self.set].work[s] = Some(Work { texels, guides: make("trace guides", 64), hdr: [make("trace hdr", 32), make("trace hdr", 32)], oidn: None });
        }
        let w = self.sets[self.set].work[s].as_ref().unwrap();
        (w.guides, w.hdr)
    }

    /// Records the trace of `t` (traced list `slot` of the frame, its
    /// `part`-th moment of `parts`) into the list's lighting and guides
    /// (`size` texels). With `profile`, times its stages and counts its
    /// work per layer. Fails only when `t`'s Slang surfaces do not build.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn part(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, queue: &mut crate::gpu::Queue, t: &Trace, slot: u32, part: u32, parts: u32, size: (u32, u32), groups: gpu::Buffer, images: gpu::Buffer, mut profile: Option<&mut crate::profile::Profile>) -> Result<(), String> {
        let entered = std::time::Instant::now();
        self.slots = self.slots.max(slot + 1);
        let mut owners: Vec<Arc<str>> = Vec::new();
        let mut records: Vec<InstanceRecord> = Vec::new();
        let mut inst_rows: Vec<[f32; 4]> = Vec::new();
        let mut next_transforms = HashMap::new();
        let mut mat_rows: Vec<[f32; 4]> = Vec::new();
        let b = |v: u32| f32::from_bits(v);
        // Temporal history (a frame of one part: sub-frames already
        // average over the shutter) follows each layer from frame to frame.
        let temporal = t.options.temporal && parts == 1;
        let profiling = profile.is_some();
        inst_rows.reserve(t.instances.len() * INST_ROWS);
        records.reserve(t.instances.len() + 1);
        for inst in &t.instances {
            let Some(([attr_base, idx_base, tag_base], address)) = self.geometry(context, encoder, &inst.key, &inst.mesh) else { continue };
            let mat_base = (mat_rows.len() / crate::mesh3d::MAT_ROWS) as u32;
            mat_rows.extend_from_slice(&inst.materials);
            if profiling {
                owners.extend(std::iter::repeat_n(inst.id.clone(), inst.materials.len() / crate::mesh3d::MAT_ROWS));
            }
            let p = |k: usize| [b(mat_base + inst.parts[k].0), b(inst.parts[k].1.max(1))];
            let custom = (inst_rows.len() / INST_ROWS) as u32;
            inst_rows.push([b(attr_base), b(idx_base), b(tag_base), 0.0]);
            inst_rows.push([p(0)[0], p(0)[1], p(1)[0], p(1)[1]]);
            inst_rows.push([p(2)[0], p(2)[1], p(3)[0], p(3)[1]]);
            inst_rows.push([p(4)[0], p(4)[1], p(5)[0], p(5)[1]]);
            // Where it was on the previous frame (object → camera then).
            let prev = if temporal { self.prev_transforms.get(&inst.id).copied().unwrap_or(inst.transform) } else { inst.transform };
            inst_rows.extend_from_slice(&prev);
            inst_rows.extend_from_slice(&inst.transform);
            inst_rows.extend_from_slice(&inst.world);
            if temporal {
                next_transforms.insert(inst.id.clone(), inst.transform);
            }
            records.push(InstanceRecord { transform: inst.transform, custom_mask: custom & 0x00FF_FFFF | 0xFF << 24, sbt_flags: 0, address });
        }
        if let Some(p) = profile.as_deref_mut() {
            p.stage("gpu frame › trace setup › instances (geometry, rows)", entered);
        }
        let building = std::time::Instant::now();
        // This part's tables and structures (out of the tracer while they
        // are written, so retiring what they outgrow can borrow it).
        let set = self.set;
        let u = self.sets[set].unit;
        self.sets[set].unit += 1;
        if self.sets[set].units.len() <= u {
            self.sets[set].units.resize_with(u + 1, Unit::default);
        }
        let mut unit = std::mem::take(&mut self.sets[set].units[u]);
        let mut retired: Vec<Retired> = Vec::new();
        let alignment = self.alignment;
        // Cards: one non-opaque structure, alpha-tested against their pictures.
        let mut card_rows: Vec<[f32; 4]> = Vec::new();
        let mut verts: Vec<[f32; 3]> = Vec::new();
        let mut card_idx: Vec<u32> = Vec::new();
        for c in &t.cards {
            let base = verts.len() as u32;
            verts.extend_from_slice(&c.view);
            card_idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            let p = c.picture;
            card_rows.push([p.origin[0] as f32, p.origin[1] as f32, p.factor[0] as f32, p.factor[1] as f32]);
            card_rows.push([b(p.offset), b(p.size.0), b(p.size.1), 0.0]);
        }
        // What the structures are built from: the same as this unit's last
        // build (a still scene, frame after frame) needs none.
        let built = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytemuck::cast_slice::<_, u8>(&records).hash(&mut h);
            bytemuck::cast_slice::<_, u8>(&verts).hash(&mut h);
            h.finish()
        };
        let cards = t.cards.len() as u32;
        let rebuild = unit.built != Some(built) || unit.tlas.is_none_or(|(_, room)| room < records.len() as u32 + 1) || (cards > 0 && unit.cards.is_none_or(|(_, room)| room < cards));
        if cards > 0 {
            let mesh = |vb: gpu::Buffer, ib: gpu::Buffer, quads: u32| gpu::AccelerationStructureMesh {
                vertex_data: vb.into(),
                vertex_format: gpu::VertexFormat::F32Vec3,
                vertex_stride: 12,
                vertex_count: quads * 4,
                index_data: ib.into(),
                index_type: Some(gpu::IndexType::U32),
                triangle_count: quads * 2,
                transform_data: gpu::Buffer::default().into(),
                is_opaque: false,
            };
            if unit.cards.is_none_or(|(_, room)| room < cards) {
                retired.extend(unit.cards.take().map(|(a, _)| Retired::Structure(a)));
                let room = cards.next_power_of_two().max(4);
                // Sized for `room` cards (the buffers only say where they are).
                let sizes = context.get_bottom_level_acceleration_structure_sizes(&[mesh(self.dummy, self.dummy, room)]);
                unit.cards = Some((context.create_acceleration_structure(gpu::AccelerationStructureDesc { name: "trace cards", ty: gpu::AccelerationStructureType::BottomLevel, size: sizes.data }), room));
                unit.cards_scratch.room(context, "trace cards scratch", sizes.scratch.max(alignment) + alignment, gpu::Memory::Device, &mut retired);
            }
            let blas = unit.cards.unwrap().0;
            if rebuild {
                let vb = unit.card_verts.write(context, "trace card verts", bytemuck::cast_slice(&verts), &mut retired);
                let ib = unit.card_idx.write(context, "trace card indices", bytemuck::cast_slice(&card_idx), &mut retired);
                let scratch = unit.cards_scratch.buffer.unwrap();
                encoder.acceleration_structure("trace cards").build_bottom_level(blas, &[mesh(vb, ib, cards)], scratch.into());
            }
            records.push(InstanceRecord { transform: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]], custom_mask: CARD_INSTANCE | 0xFF << 24, sbt_flags: 0, address: context.acceleration_structure_address(blas) });
        }
        // Top level (an empty scene still gets one, with nothing in it).
        if unit.tlas.is_none_or(|(_, room)| room < records.len() as u32 + 1) {
            retired.extend(unit.tlas.take().map(|(a, _)| Retired::Structure(a)));
            let room = (records.len() as u32 + 1).next_power_of_two().max(16);
            let sizes = context.get_top_level_acceleration_structure_sizes(room);
            unit.tlas = Some((context.create_acceleration_structure(gpu::AccelerationStructureDesc { name: "trace tlas", ty: gpu::AccelerationStructureType::TopLevel, size: sizes.data }), room));
            unit.tlas_scratch.room(context, "trace tlas scratch", sizes.scratch.max(alignment) + alignment, gpu::Memory::Device, &mut retired);
        }
        let tlas = unit.tlas.unwrap().0;
        if rebuild {
            let count = records.len() as u32;
            if records.is_empty() {
                records.push(bytemuck::Zeroable::zeroed());
            }
            let ibuf = unit.instances.write(context, "trace tlas instances", bytemuck::cast_slice(&records), &mut retired);
            let scratch = unit.tlas_scratch.buffer.unwrap();
            encoder.acceleration_structure("trace tlas").build_top_level(tlas, &[], count, ibuf.into(), scratch.into());
            unit.built = Some(built);
        }
        // Physical noise and grid volumes get density bounds (their walk
        // steps through them; artistic ones are marched and need none): a
        // job each, and their cells' offset (+ 1) in material row 30.w.
        let mut jobs: Vec<u32> = Vec::new();
        let mut cells_total = 0usize;
        for m in 0..mat_rows.len() / crate::mesh3d::MAT_ROWS {
            let r = m * crate::mesh3d::MAT_ROWS;
            if mat_rows[r + 2][3] as u32 & crate::mesh3d::FLAG_VOLUME != 0 && mat_rows[r + 30][2] > 0.5 && mat_rows[r + 24][3] < 0.5 {
                jobs.extend([r as u32, cells_total as u32]);
                mat_rows[r + 30][3] = f32::from_bits(cells_total as u32 + 1);
                cells_total += BOUNDS_CELLS;
            }
        }
        let bounds_buf = if cells_total > 0 { unit.bounds.room(context, "trace volume bounds", (cells_total * 4) as u64, gpu::Memory::Device, &mut retired) } else { self.dummy };
        for v in [&mut inst_rows, &mut mat_rows, &mut card_rows] {
            if v.is_empty() {
                v.push([0.0; 4]);
            }
        }
        let features = scene_features(&mat_rows, t);
        let mut lights = t.lights.clone();
        if lights.is_empty() {
            lights.push([0.0; 4]);
        }
        let jobs_buf = if jobs.is_empty() { self.dummy } else { unit.jobs.write(context, "trace bounds jobs", bytemuck::cast_slice(&jobs), &mut retired) };
        let inst_buf = unit.inst.write(context, "trace instances", bytemuck::cast_slice(&inst_rows), &mut retired);
        let mat_buf = unit.mat.write(context, "trace materials", bytemuck::cast_slice(&mat_rows), &mut retired);
        let light_buf = unit.light.write(context, "trace lights", bytemuck::cast_slice(&lights), &mut retired);
        let card_buf = unit.card.write(context, "trace cards", bytemuck::cast_slice(&card_rows), &mut retired);
        // Profiling: [ray queries, triangle candidates, empty lookups, 0],
        // per 8 × 4 block [lookups summed, most], per material [hits, lookups].
        let counts = if profiling {
            let blocks = (size.0.div_ceil(8) * size.1.div_ceil(4)) as usize;
            let b = unit.counts.write(context, "trace profile", &vec![0u8; (PROF_HEADER + blocks * 2 + owners.len() * 2) * 4], &mut retired);
            self.sets[set].counts.push((b, owners, blocks));
            b
        } else {
            self.dummy
        };
        self.sets[set].units[u] = unit;
        for r in retired {
            self.retire(r);
        }
        let [attr, idx, tag] = self.shared(context);
        // The environment image: uploaded once while it stays the same.
        let env_buf = match &t.env {
            Some(e) => {
                let key = Arc::as_ptr(e) as usize;
                if self.env.as_ref().map(|(k, _, _)| *k) != Some(key) {
                    if let Some((_, _, old)) = self.env.take() {
                        self.retire(Retired::Buffer(old));
                    }
                    let b = context.create_buffer(gpu::BufferDesc { name: "trace environment", size: (e.trace.len() * 16) as u64, memory: gpu::Memory::Shared });
                    // SAFETY: a fresh mapped buffer of exactly this size.
                    unsafe { std::ptr::copy_nonoverlapping(e.trace.as_ptr() as *const u8, b.data(), e.trace.len() * 16) };
                    context.sync_buffer(b);
                    self.env = Some((key, e.clone(), b));
                }
                self.env.as_ref().map(|(_, _, b)| *b).unwrap()
            }
            None => self.dummy,
        };
        // Lighting and its variance (two rows per texel), twice (the
        // filters read one and write the other), and the guides: albedo,
        // normal, depth, motion per texel.
        let (guides, hdr) = self.work(context, slot, (size.0 * size.1) as u64);
        let r = |m: &[f32; 9]| [[m[0], m[1], m[2], 0.0], [m[3], m[4], m[5], 0.0], [m[6], m[7], m[8], 0.0]];
        if temporal {
            self.prev_transforms.extend(next_transforms);
        }
        let (hist, prev) = if temporal {
            if self.history.get(&slot).is_none_or(|h| h.size != size) {
                if let Some(old) = self.history.remove(&slot) {
                    for b in old.buffers {
                        self.retire(Retired::Buffer(b));
                    }
                }
                let hist_size = ((size.0 * size.1) as u64 * 48).max(16);
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
            ((read, write), prev)
        } else {
            ((self.dummy, self.dummy), ([0.0; 4], [0.0; 4], [[0.0; 4]; 3]))
        };
        let uniforms = Uniforms {
            size: [size.0, size.1, 0, t.options.spp.max(1)],
            opts: [t.options.bounces.max(1), t.seed, CARD_INSTANCE, t.cards.len() as u32],
            cam: [t.focal, t.centre[0], t.centre[1], 0.0],
            place: [t.origin[0], t.origin[1], t.factor[0], t.factor[1]],
            root: r(&t.root),
            inv_root: r(&t.inv_root),
            lens: [t.lens[0], t.lens[1], 0.0, 0.0],
            denoise: [0, temporal as u32, 0, 0],
            prev_cam: prev.0,
            prev_place: prev.1,
            prev_root: prev.2,
            band: [0, size.1, if profiling { density_repeat() } else { 0 }, part | parts.max(1) << 16],
        };
        let bind = Bindings { tlas, inst: inst_buf, attr, idx, tag, mat: mat_buf, light: light_buf, card: card_buf, env: env_buf, counts, bounds: bounds_buf, jobs: jobs_buf };
        let state = Slot { size, uniforms, bind, temporal, denoise: t.options.denoise, hist, current: 0, denoised: false, upload: false };
        if !jobs.is_empty() {
            let mut pass = encoder.compute("volume bounds");
            let mut pc = pass.with(&self.bounds);
            pc.bind(0, &state.data(uniforms, groups, images, guides, hdr[0], hdr[1]));
            pc.dispatch([(BOUNDS_CELLS as u32).div_ceil(64), (jobs.len() / 2) as u32, 1]);
        }
        if let Some(p) = profile.as_deref_mut() {
            p.stage("gpu frame › trace setup › tables and structures", building);
        }
        let compiling = std::time::Instant::now();
        {
            let profiled = self.profile;
            let pipeline = match &t.surfaces {
                Some(program) => self.slang_pipeline(context, program, features)?,
                None => {
                    let pipeline = self.specialised.entry(features).or_insert_with(|| {
                        // A build with only this scene's features; the general one if it fails.
                        context
                            .try_create_shader(gpu::ShaderDesc { source: &specialise(features, profiled) })
                            .ok()
                            .map(|shader| context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "trace", data_layouts: &[&TraceData::layout()], compute: shader.at("trace") }))
                    });
                    pipeline.as_ref().unwrap_or(&self.pipeline)
                }
            };
            if let Some(p) = profile.as_deref_mut() {
                p.stage("gpu frame › trace setup › pipeline", compiling);
                p.add("gpu frame › trace setup", entered.elapsed());
            }
            // In bands of rows, each its own dispatch and submission: one
            // dispatch of every path of a large frame (64 paths at 1080 x
            // 1920 is 130 million) crashes Mesa's software Vulkan past ~20
            // million and on GPUs runs long enough to trip a driver's
            // watchdog; a submission each also lets the GPU turn to the
            // desktop between bands, so it keeps drawing. The bands queue
            // up behind each other: nothing waits for them here.
            let band = band_rows(size.0, t.options.spp.max(1)).min(TRACE_BAND_ROWS);
            let bands = size.1.div_ceil(band);
            for k in 0..bands {
                let rows = [k * band, ((k + 1) * band).min(size.1)];
                {
                    let mut pass = encoder.compute("trace");
                    let mut pc = pass.with(pipeline);
                    pc.bind(0, &state.data(Uniforms { band: [rows[0], rows[1], uniforms.band[2], uniforms.band[3]], ..uniforms }, groups, images, guides, hdr[0], hdr[1]));
                    pc.dispatch([size.0.div_ceil(8), (rows[1] - rows[0]).div_ceil(8), 1]);
                }
                if k + 1 < bands {
                    queue.split(context, encoder, profile.as_deref_mut());
                }
            }
        }
        let s = slot as usize;
        if self.sets[set].slots.len() <= s {
            self.sets[set].slots.resize_with(s + 1, || None);
        }
        self.sets[set].slots[s] = Some(state);
        Ok(())
    }

    /// Whether traced lists go through Open Image Denoise (loading it on
    /// first use); else the built-in filter does all the work.
    pub(crate) fn wants_oidn(&mut self) -> bool {
        // LAHZA_DENOISER=builtin keeps the à-trous filter (to compare).
        if std::env::var("LAHZA_DENOISER").is_ok_and(|v| v == "builtin") {
            return false;
        }
        if self.oidn.is_none() {
            self.oidn = Some(crate::oidn::Denoiser::new());
        }
        matches!(self.oidn, Some(Ok(_)))
    }

    /// Tests: a stand-in for Open Image Denoise that returns the colour
    /// it is given, so the way there and back is checked without it.
    #[cfg(test)]
    pub(crate) fn pass_through_oidn(&mut self) {
        self.oidn = Some(Ok(crate::oidn::Denoiser::pass_through()));
    }

    /// Tests: pictures the denoiser was given.
    #[cfg(test)]
    pub(crate) fn oidn_runs(&self) -> u64 {
        match &self.oidn {
            Some(Ok(d)) => d.runs,
            _ => 0,
        }
    }

    /// Ends the parts of traced list `slot`: temporal accumulation and,
    /// with `oidn`, the lighting on its way to Open Image Denoise (as
    /// Blender's Cycles gives it: the full colour, lighting × albedo, with
    /// albedo and normal to guide it, so grain in the surface colour from
    /// depth of field or glass goes too), packed on the GPU and copied to
    /// the host. `denoise` takes it from there once this work is done.
    pub(crate) fn settle(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, slot: u32, groups: gpu::Buffer, images: gpu::Buffer, oidn: bool) {
        let set = self.set;
        let Some(mut state) = self.sets[set].slots.get_mut(slot as usize).and_then(Option::take) else { return };
        let texels = (state.size.0 * state.size.1) as u64;
        let (guides, hdr) = self.work(context, slot, texels);
        let groups_x = [state.size.0.div_ceil(8), state.size.1.div_ceil(8), 1];
        if state.temporal {
            let mut pass = encoder.compute("trace temporal");
            let mut pc = pass.with(&self.temporal);
            pc.bind(0, &state.data(state.uniforms, groups, images, guides, hdr[0], hdr[1]));
            pc.dispatch(groups_x);
            state.current = 1;
        }
        if oidn && state.denoise {
            let d = self.denoising(context, slot, state.size);
            {
                let mut pass = encoder.compute("oidn pack");
                let mut pc = pass.with(&self.pack);
                let mut data = state.data(state.uniforms, groups, images, guides, hdr[state.current], hdr[1 - state.current]);
                data.b_bounds = d.packed.into();
                pc.bind(0, &data);
                pc.dispatch(groups_x);
            }
            if let Way::Copied { down, .. } = d.way {
                encoder.transfer("oidn readback").copy_buffer_to_buffer(d.packed.into(), down.into(), texels * 36);
            }
            state.denoised = true;
        }
        self.sets[set].slots[slot as usize] = Some(state);
    }

    /// Lets go of what the denoiser holds of `d` (before its buffers go).
    fn release(&mut self, d: Denoising) {
        if let (Way::Shared { id, .. }, Some(Ok(denoiser))) = (d.way, self.oidn.as_mut()) {
            denoiser.release_shared(id);
        }
    }

    /// The buffers traced list `slot`'s pictures of `size` go to the
    /// denoiser through: shared with it where it and the device can
    /// (nothing is copied then), else through the host.
    fn denoising(&mut self, context: &gpu::Context, slot: u32, size: (u32, u32)) -> Denoising {
        let (set, s) = (self.set, slot as usize);
        let share = !matches!(self.sharing, Sharing::Off(_)) && context.has_external_memory() && matches!(&self.oidn, Some(Ok(d)) if d.shares_fd());
        if let Some(d) = self.sets[set].work[s].as_ref().and_then(|w| w.oidn) {
            if d.size == size && matches!(d.way, Way::Shared { .. }) == share {
                return d;
            }
            // Another size, or the other way: these go.
            self.release(d);
            for b in d.buffers() {
                self.retire(Retired::Buffer(b));
            }
            self.sets[set].work[s].as_mut().unwrap().oidn = None;
        }
        let texels = (size.0 * size.1) as u64;
        let make = |name, per: u64, memory| context.create_buffer(gpu::BufferDesc { name, size: (texels * per).max(16), memory });
        let mut made = None;
        if share {
            // Two memories the denoiser imports by file descriptor.
            let exported = gpu::Memory::External(gpu::ExternalMemorySource::Fd(None));
            let (packed, output) = (make("oidn shared input", 36, exported), make("oidn shared output", 12, exported));
            let fd = |b: gpu::Buffer| match context.get_external_buffer_source(b) {
                Some(gpu::ExternalMemorySource::Fd(Some(fd))) => Some(fd),
                _ => None,
            };
            let shared = match (fd(packed), fd(output), self.oidn.as_mut()) {
                (Some(a), Some(b), Some(Ok(denoiser))) => denoiser.share(size.0 as usize, size.1 as usize, a, b),
                _ => Err("the device did not export the denoiser's memory".into()),
            };
            match shared {
                Ok(id) => made = Some(Denoising { size, packed, way: Way::Shared { output, id, up: None } }),
                Err(e) => {
                    context.destroy_buffer(packed);
                    context.destroy_buffer(output);
                    self.sharing = Sharing::Off(e);
                }
            }
        }
        let d = made.unwrap_or_else(|| Denoising { size, packed: make("oidn packed", 36, gpu::Memory::Device), way: Way::Copied { down: make("oidn input", 36, gpu::Memory::Download), up: make("oidn output", 12, gpu::Memory::Upload) } });
        self.sets[set].work[s].as_mut().unwrap().oidn = Some(d);
        d
    }

    /// Runs Open Image Denoise on traced list `slot` of the frame in set
    /// `set`, whose `settle` has finished on the GPU. When it fails the
    /// built-in filter takes over (and a warning says why).
    pub(crate) fn denoise(&mut self, context: &gpu::Context, set: usize, slot: u32) {
        let Some(state) = self.sets[set].slots.get(slot as usize).and_then(|s| s.as_ref()) else { return };
        if !state.denoised {
            return;
        }
        let size = state.size;
        let n = (size.0 * size.1) as usize;
        let Some(d) = self.sets[set].work[slot as usize].as_ref().and_then(|w| w.oidn) else { return };
        // The denoised colour's way back: None in place, Some from the
        // upload buffer.
        let result: Result<bool, String> = (|| {
            let Some(Ok(denoiser)) = self.oidn.as_mut() else { return Err("Open Image Denoise is not loaded".into()) };
            match d.way {
                Way::Copied { down, up } => {
                    context.sync_buffer(down);
                    // SAFETY: the copy finished; the buffers hold n · 36 and
                    // n · 12 bytes of f32, and no frame in flight touches
                    // this set's.
                    let (input, output) = unsafe { (std::slice::from_raw_parts(down.data() as *const f32, n * 9), std::slice::from_raw_parts_mut(up.data() as *mut f32, n * 3)) };
                    denoiser.denoise(size.0 as usize, size.1 as usize, input, output)?;
                    context.sync_buffer(up);
                    Ok(true)
                }
                Way::Shared { output, id, .. } => {
                    let shared = denoiser.denoise_shared(id);
                    if shared.is_ok() && self.sharing == Sharing::Checked {
                        return Ok(false);
                    }
                    // The first shared picture (or one that failed): the
                    // same picture the copied way, from the host. Equal,
                    // and the shared way is trusted from here on; else it
                    // is off, and this result is uploaded.
                    let (mut input, mut theirs, mut ours) = (vec![0f32; n * 9], vec![0f32; n * 3], vec![0f32; n * 3]);
                    denoiser.read_shared(id, &mut input, &mut theirs)?;
                    denoiser.denoise(size.0 as usize, size.1 as usize, &input, &mut ours)?;
                    let same = shared.is_ok() && theirs.iter().zip(&ours).all(|(a, b)| (a - b).abs() <= 2e-3 * (1.0 + b.abs()));
                    if same {
                        self.sharing = Sharing::Checked;
                        return Ok(false);
                    }
                    self.sharing = Sharing::Off(shared.err().unwrap_or_else(|| "denoising the tracer's memory in place gave another picture than copying it".into()));
                    let up = context.create_buffer(gpu::BufferDesc { name: "oidn output", size: (n as u64 * 12).max(16), memory: gpu::Memory::Upload });
                    // SAFETY: a fresh mapped buffer of n · 12 bytes.
                    unsafe { std::ptr::copy_nonoverlapping(ours.as_ptr() as *const u8, up.data(), n * 12) };
                    context.sync_buffer(up);
                    if let Some(w) = self.sets[set].work[slot as usize].as_mut() {
                        w.oidn = Some(Denoising { way: Way::Shared { output, id, up: Some(up) }, ..d });
                    }
                    Ok(true)
                }
            }
        })();
        let state = self.sets[set].slots[slot as usize].as_mut().expect("the slot");
        match result {
            Ok(upload) => state.upload = upload,
            Err(e) => {
                state.denoised = false;
                self.oidn = Some(Err(e));
            }
        }
    }

    /// Whether traced list `slot` of this frame is waiting for `denoise`.
    pub(crate) fn awaits_denoise(&self, slot: u32) -> bool {
        self.sets[self.set].slots.get(slot as usize).is_some_and(|s| s.as_ref().is_some_and(|s| s.denoised))
    }

    /// Makes `set` the set `post` and `settle` work on (a frame taken up
    /// again after another was recorded).
    pub(crate) fn resume(&mut self, set: usize) {
        self.set = set;
    }

    /// Records the filters of traced list `slot` and its picture into
    /// the group texels at each of `offsets` (the samples of a
    /// motion-blurred frame composite the same picture, each from its own
    /// group).
    pub(crate) fn post(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, slot: u32, offsets: &[u32], groups: gpu::Buffer, images: gpu::Buffer) {
        let set = self.set;
        let Some(mut state) = self.sets[set].slots.get_mut(slot as usize).and_then(Option::take) else { return };
        let texels = (state.size.0 * state.size.1) as u64;
        let (guides, hdr) = self.work(context, slot, texels);
        let groups_x = [state.size.0.div_ceil(8), state.size.1.div_ceil(8), 1];
        if state.denoised {
            // Back as lighting over albedo: the resolve pass multiplies the
            // same albedo in again, which gives OIDN's colour exactly.
            let d = self.sets[set].work[slot as usize].as_ref().and_then(|w| w.oidn).expect("denoiser buffers");
            // Where the denoised colour is on the device: the shared
            // output, or what the host's copy is uploaded into.
            let (denoised, up) = match d.way {
                Way::Copied { up, .. } => (d.packed, Some(up)),
                Way::Shared { output, up, .. } => (output, up),
            };
            if let (true, Some(up)) = (state.upload, up) {
                encoder.transfer("oidn upload").copy_buffer_to_buffer(up.into(), denoised.into(), texels * 12);
            }
            let mut pass = encoder.compute("oidn unpack");
            let mut pc = pass.with(&self.unpack);
            let mut data = state.data(state.uniforms, groups, images, guides, hdr[state.current], hdr[1 - state.current]);
            data.b_bounds = denoised.into();
            pc.bind(0, &data);
            pc.dispatch(groups_x);
            state.current = 1 - state.current;
        }
        if state.denoise {
            // With OIDN the à-trous passes still smooth the shadow
            // catchers' visibility (mode 1), leaving OIDN's colour alone.
            for k in 0..DENOISE_PASSES {
                let mut pass = encoder.compute("trace denoise");
                let mut pc = pass.with(&self.atrous);
                let u = Uniforms { denoise: [1 << k, state.uniforms.denoise[1], 0, state.denoised as u32], ..state.uniforms };
                pc.bind(0, &state.data(u, groups, images, guides, hdr[state.current], hdr[1 - state.current]));
                pc.dispatch(groups_x);
                state.current = 1 - state.current;
            }
        }
        for &offset in offsets {
            state.uniforms.size[2] = offset;
            let mut pass = encoder.compute("trace resolve");
            let mut pc = pass.with(&self.resolve);
            pc.bind(0, &state.data(state.uniforms, groups, images, guides, hdr[state.current], hdr[1 - state.current]));
            pc.dispatch(groups_x);
        }
    }

    /// The trace pipeline with `program`'s Slang surfaces for `features`,
    /// built once; another program's are retired.
    fn slang_pipeline(&mut self, context: &gpu::Context, program: &lahza_motion_core::surface::Program, features: u32) -> Result<&gpu::ComputePipeline, String> {
        if let Some((_, old)) = self.slang.take_if(|(key, _)| *key != program.key) {
            for p in old.into_values() {
                self.retire(Retired::Pipeline(p));
            }
        }
        let profiled = self.profile;
        let (_, built) = self.slang.get_or_insert_with(|| (program.key, HashMap::new()));
        if !built.contains_key(&features) {
            let source = crate::surfaces::trace_source(&specialise(features, profiled), program)?;
            let shader = context.try_create_shader(gpu::ShaderDesc { source: &source }).map_err(|e| format!("Slang materials' trace shader: {e}"))?;
            let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "trace (Slang surfaces)", data_layouts: &[&TraceData::layout()], compute: shader.at("trace") });
            built.insert(features, pipeline);
        }
        Ok(&built[&features])
    }

    /// Adds the work counters of the frame in set `set` to `profile` by
    /// layer (call once its GPU work has finished).
    pub(crate) fn collect_profile(&mut self, set: usize, profile: &mut crate::profile::Profile) {
        let k = crate::profile::COUNT_EVERY;
        let Some(s) = self.sets.get_mut(set) else { return };
        for (buffer, owners, blocks) in s.counts.drain(..) {
            let n = PROF_HEADER + blocks * 2 + owners.len() * 2;
            // SAFETY: a mapped buffer of n u32 counters; the GPU is done with it.
            let c = unsafe { std::slice::from_raw_parts(buffer.data() as *const u32, n) };
            profile.rays += c[0] as u64 * k;
            profile.candidates += c[1] as u64 * k;
            profile.density_empty += c[2] as u64 * k;
            for d in 0..4 {
                profile.rays_by_scatter[d] += c[4 + d] as u64 * k;
                profile.lookups_by_scatter[d] += c[8 + d] as u64 * k;
            }
            for b in 0..blocks {
                let (sum, most) = (c[PROF_HEADER + b * 2] as u64, c[PROF_HEADER + 1 + b * 2] as u64);
                profile.lane_lookups += sum;
                profile.lane_slots += most * 32;
            }
            let m0 = PROF_HEADER + blocks * 2;
            for (m, id) in owners.iter().enumerate() {
                let l = profile.layers.entry(id.to_string()).or_default();
                l.hits += c[m0 + m * 2] as u64 * k;
                l.density += c[m0 + m * 2 + 1] as u64 * k;
            }
        }
    }

    /// Why traced frames were not denoised with Open Image Denoise, if so.
    pub(crate) fn denoise_warning(&self) -> Option<String> {
        match (&self.oidn, &self.sharing) {
            (Some(Err(e)), _) => Some(format!("path tracing used the built-in denoiser: {e}")),
            // (Not a fault of the picture: only slower.)
            (Some(Ok(d)), Sharing::Off(why)) if d.shares_fd() => Some(format!("Open Image Denoise's pictures are copied through the host: {why}")),
            _ => None,
        }
    }

    /// Tests: the stand-in denoiser also takes the tracer's memory in
    /// place; and whether pictures are denoised that way.
    #[cfg(test)]
    pub(crate) fn pass_through_oidn_shared(&mut self, badly: bool) {
        let mut d = crate::oidn::Denoiser::pass_through();
        d.stand_in_shares = true;
        d.stand_in_shares_badly = badly;
        self.oidn = Some(Ok(d));
    }

    #[cfg(test)]
    pub(crate) fn shares_with_oidn(&self) -> Result<bool, String> {
        match &self.sharing {
            Sharing::Untried => Ok(false),
            Sharing::Checked => Ok(true),
            Sharing::Off(why) => Err(why.clone()),
        }
    }

    /// Frees everything; the GPU must be idle.
    pub(crate) fn destroy(&mut self, context: &gpu::Context) {
        if let Some((_, old)) = self.slang.take() {
            for p in old.into_values() {
                self.retire(Retired::Pipeline(p));
            }
        }
        self.slots = 0;
        self.begin_frame(context, u64::MAX, u64::MAX, 0);
        for b in self.shared.take().into_iter().flatten() {
            context.destroy_buffer(b);
        }
        for (_, g) in self.geometry.drain() {
            context.destroy_acceleration_structure(g.blas);
            context.destroy_buffer(g.positions);
            context.destroy_buffer(g.indices);
        }
        if let Some((_, _, b)) = self.env.take() {
            context.destroy_buffer(b);
        }
        // (The denoiser lets go of the memories it shares before they go.)
        self.oidn = None;
        for mut s in std::mem::take(&mut self.sets) {
            for u in &mut s.units {
                u.destroy(context);
            }
            for b in s.work.iter().flatten().flat_map(|w| w.buffers()) {
                context.destroy_buffer(b);
            }
        }
        for (_, r) in std::mem::take(&mut self.retired) {
            match r {
                Retired::Buffer(b) => context.destroy_buffer(b),
                Retired::Structure(a) => context.destroy_acceleration_structure(a),
                Retired::Pipeline(mut p) => context.destroy_compute_pipeline(&mut p),
            }
        }
        for (_, h) in self.history.drain() {
            for b in h.buffers {
                context.destroy_buffer(b);
            }
        }
        context.destroy_buffer(self.dummy);
        for p in [&mut self.pipeline, &mut self.temporal, &mut self.atrous, &mut self.resolve, &mut self.bounds, &mut self.pack, &mut self.unpack] {
            context.destroy_compute_pipeline(p);
        }
        for mut p in self.specialised.drain().filter_map(|(_, p)| p) {
            context.destroy_compute_pipeline(&mut p);
        }
    }
}

impl Slot {
    /// The passes' bindings: `read` and `write` are the lighting buffers
    /// a pass reads and writes.
    fn data(&self, u: Uniforms, groups: gpu::Buffer, images: gpu::Buffer, guides: gpu::Buffer, read: gpu::Buffer, write: gpu::Buffer) -> TraceData {
        let b = &self.bind;
        TraceData {
            u,
            acc: b.tlas,
            b_inst: b.inst.into(),
            b_attr: b.attr.into(),
            b_idx: b.idx.into(),
            b_tag: b.tag.into(),
            b_mat: b.mat.into(),
            b_light: b.light.into(),
            b_card: b.card.into(),
            b_groups: groups.into(),
            b_guides: guides.into(),
            b_env: b.env.into(),
            b_hdr: read.into(),
            b_tmp: write.into(),
            b_hist: self.hist.0.into(),
            b_next: self.hist.1.into(),
            b_images: images.into(),
            b_prof: b.counts.into(),
            b_bounds: b.bounds.into(),
            b_jobs: b.jobs.into(),
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
    fn traces_run_in_bounded_bands_of_rows() {
        for (w, spp) in [(1080, 64), (540, 8), (1920, 1), (8000, 512), (1, 1)] {
            let rows = band_rows(w, spp);
            assert!(rows % 8 == 0 && rows >= 8, "{w} x {spp}: {rows}");
            assert!(rows == 8 || w as u64 * spp as u64 * rows as u64 <= PATHS_PER_DISPATCH, "{w} x {spp}: {rows} rows");
        }
        assert!(band_rows(1080, 64) < 1920, "a full-HD frame at 64 paths is split");
    }

    /// A traced frame gets time in proportion to its paths: a final-quality
    /// frame on a software GPU took over 30 s, timed out, and its buffers
    /// were freed while the trace still wrote to them (a crash).
    #[test]
    fn traced_frames_get_time_for_their_paths() {
        use crate::gpu::frame_timeout_ms;
        assert_eq!(frame_timeout_ms(0), 30_000);
        assert!(frame_timeout_ms(1080 * 1920 * 64) > 20 * 60_000);
        assert_eq!(frame_timeout_ms(u64::MAX), u32::MAX);
    }

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

    /// Where Open Image Denoise is installed (Blender ships it), traced
    /// frames go through it, as in Cycles: no fall-back warning, and a 4 spp
    /// still life loses most of its noise. (On the replica's bond-wire shot
    /// it removed the depth-of-field speckle the à-trous filter kept.)
    #[test]
    fn traced_frames_are_denoised_with_oidn_when_installed() {
        if crate::oidn::Denoiser::new().is_err() {
            eprintln!("skipping: no Open Image Denoise");
            return;
        }
        let Some(mut denoised) = tracer(4) else { return };
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        let mut raw = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow,
            path: Some(super::TraceOptions { spp: 4, bounces: 4, denoise: false, ..Default::default() }), ..Default::default() });
        let c = Compiled::new(&still_life());
        let (a, b) = (denoised.render(&c, 0.0), raw.render(&c, 0.0));
        assert!(!denoised.warnings().iter().any(|w| w.contains("built-in denoiser")), "{:?}", denoised.warnings());
        // Noise: the energy left after subtracting a 3×3 box blur.
        let noise = |img: &image::RgbaImage| {
            let l = |x: u32, y: u32| img.get_pixel(x, y).0[..3].iter().map(|v| *v as f64).sum::<f64>() / 3.0;
            let (w, h) = img.dimensions();
            let (mut sum, mut n) = (0.0, 0.0);
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let mean = (0..9u32).map(|k| l(x + k % 3 - 1, y + k / 3 - 1)).sum::<f64>() / 9.0;
                    sum += (l(x, y) - mean).abs();
                    n += 1.0;
                }
            }
            sum / n
        };
        let (na, nb) = (noise(&a), noise(&b));
        assert!(na < nb / 3.0, "denoised {na:.2} vs raw {nb:.2}");
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
    /// Subsurface scattering (a random walk below the surface): with a short
    /// reach it looks like the plain diffuse surface (the walk gives back
    /// the base colour's energy); a thin plate lit only from behind glows
    /// on its dark side, red most (it travels furthest), where a plain one
    /// stays dark.
    #[test]
    fn subsurface_scatters_inside() {
        let Some(mut r) = tracer(64) else { return };
        let render = |r: &mut Renderer, layers: serde_json::Value, from: [f64; 3]| {
            let mut p = project(layers);
            p.settings.width = 320;
            p.settings.height = 240;
            p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": from, "shadow": true}])).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
            r.render(&Compiled::new(&p), 0.0)
        };
        let ball = |x: f64, extra: serde_json::Value| {
            let mut m = json!({"preset": "matte", "color": "#b0b0b0", "specular": 0});
            for (k, v) in extra.as_object().unwrap() {
                m[k] = v.clone();
            }
            json!({"id": format!("b{x}"), "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": m, "transform": {"position": [x, 120]}})
        };
        // Each alone in the same place: side by side, perspective shows each from another side.
        let (a, b) = (render(&mut r, json!([ball(160.0, json!({}))]), [-0.5, -0.4, -1.0]), render(&mut r, json!([ball(160.0, json!({"subsurface": 1, "subsurface_scale": 3}))]), [-0.5, -0.4, -1.0]));
        let (plain, sss) = (disc(&a, 160.0, 120.0, 0.0, 55.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 55.0, |_, _| true));
        let l = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        // (Display values: a few percent here is about twice that in light.)
        assert!((l(sss) / l(plain) - 1.0).abs() < 0.05, "a short walk reflects as diffuse: plain {plain:?}, subsurface {sss:?}");
        let plate = |x: f64, extra: serde_json::Value| {
            let mut m = json!({"preset": "matte", "color": "#e0e0e0", "specular": 0});
            for (k, v) in extra.as_object().unwrap() {
                m[k] = v.clone();
            }
            json!({"id": format!("p{x}"), "kind": "mesh", "mesh": {"type": "box", "size": [110, 110], "depth": 8}, "material": m, "transform": {"position": [x, 120]}})
        };
        let (a, b) = (render(&mut r, json!([plate(160.0, json!({}))]), [0.0, 0.0, 1.0]), render(&mut r, json!([plate(160.0, json!({"subsurface": 1, "subsurface_scale": 20}))]), [0.0, 0.0, 1.0]));
        let (plain, sss) = (disc(&a, 160.0, 120.0, 0.0, 40.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 40.0, |_, _| true));
        assert!(l(plain) < 3.0, "a plain plate lit from behind is dark in front: {plain:?}");
        assert!(l(sss) > 20.0, "light scattered through the plate: {sss:?}");
        assert!(sss[0] > sss[2] + 10.0, "red travels furthest: {sss:?}");
    }

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

    /// The look (tone curve, exposure) is for lit meshes: flat cards and
    /// unlit materials keep their exact colours, traced as rasterised.
    #[test]
    fn traced_cards_and_unlit_meshes_keep_their_colours_under_the_look() {
        let Some(mut r) = tracer(16) else { return };
        let mut p = project(json!([
            {"id": "card", "kind": "shape", "shape": "rect", "size": [120, 120], "fill": "#e02828", "three_d": true, "transform": {"position": [90, 90]}},
            {"id": "flat", "kind": "mesh", "mesh": {"type": "sphere", "radius": 50}, "material": {"preset": "flat", "color": "#2860e0"},
             "transform": {"position": [235, 90]}}
        ]));
        p.scenes[0].look = Some(serde_json::from_value(json!({"tone": "agx", "exposure": 1.0})).unwrap());
        let c = Compiled::new(&p);
        let (a, b) = (r.render(&c, 0.0), cpu().render(&c, 0.0));
        for (x, what) in [(90.0, "card"), (235.0, "unlit sphere")] {
            let (ma, mb) = (disc(&a, x, 90.0, 0.0, 30.0, |_, _| true), disc(&b, x, 90.0, 0.0, 30.0, |_, _| true));
            for k in 0..3 {
                assert!((ma[k] - mb[k]).abs() < 4.0, "{what}: traced {ma:?}, raster {mb:?}");
            }
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

    /// A shadow catcher in front of a card (footage, a shape) lets the card
    /// show and darkens it only where a mesh's shadow falls; it used to end
    /// every path, drawing the card black.
    #[test]
    fn a_shadow_catcher_shows_the_card_behind_it() {
        let Some(mut r) = tracer(64) else { return };
        let mut p = project(json!([
            {"id": "card", "kind": "shape", "shape": "rect", "size": [300, 170], "fill": "#d02020", "three_d": true, "transform": {"position": [160, 90]}},
            {"id": "catch", "kind": "mesh", "mesh": {"type": "plane", "size": [300, 170]}, "material": {"preset": "shadow", "opacity": 0.8},
             "transform": {"position": [160, 90], "z": -2}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"preset": "matte", "color": "#ffffff"},
             "transform": {"position": [90, 90], "z": -120}}
        ]));
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [0, 0, -1], "softness": 0, "shadow": true}])).unwrap();
        let img = r.render(&Compiled::new(&p), 0.0);
        let open = disc(&img, 250.0, 90.0, 0.0, 12.0, |_, _| true);
        assert!(open[0] > 140.0 && open[1] < 70.0, "the card shows through the catcher: {open:?}");
        // The light shines straight away from the camera: the shadow lies
        // behind the ball at x 90 on the card, while the much nearer ball
        // is seen further left (x ≈ 25).
        let shade = disc(&img, 100.0, 90.0, 0.0, 12.0, |_, _| true);
        let lit = disc(&img, 210.0, 90.0, 0.0, 10.0, |_, _| true);
        assert!(shade[0] < lit[0] * 0.75, "the shadow darkens the card: {shade:?} vs {lit:?}");
    }

    /// Out of focus, a textured surface is a smooth blur at a few samples:
    /// the built-in denoiser filters the whole colour where a pixel's
    /// samples saw different colours, instead of keeping their average as
    /// if it were texture (which left defocused areas grainy).
    #[test]
    fn defocused_texture_is_smooth_at_few_samples() {
        let scene = || {
            let mut p = project(json!([{"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 600]},
                "material": {"preset": "matte", "color": "#f0e0c0", "noise": {"type": "cellular", "scale": 10, "color": "#202a80"}},
                "transform": {"position": [160, 90], "z": 500}}]));
            p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [0, 0, -1], "intensity": 1.2, "shadow": false}])).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
            p.scenes[0].camera = serde_json::from_value(json!({"aperture": 6, "focus": 0})).unwrap();
            Compiled::new(&p)
        };
        // Pixel-to-pixel change: grain, where the blur should be smooth.
        let grain = |img: &image::RgbaImage| {
            let mut sum = 0.0;
            let mut n = 0.0;
            for y in 40..140 {
                for x in 60..260 {
                    sum += (img.get_pixel(x, y).0[1] as f64 - img.get_pixel(x + 1, y).0[1] as f64).abs();
                    n += 1.0;
                }
            }
            sum / n
        };
        let Some(mut reference) = tracer(256) else { return };
        let Some(mut r) = tracer(8) else { return };
        let (want, got) = (grain(&reference.render(&scene(), 0.0)), grain(&r.render(&scene(), 0.0)));
        assert!(got < want * 2.5 + 1.0, "grain {got:.2} at 8 paths, {want:.2} at 256");
    }

    /// Thin lit haze is smooth at a few samples: every path through it
    /// gathers single-scattered light, not only the few that happen to
    /// scatter (which left bright and dark specks).
    #[test]
    fn thin_haze_is_smooth_at_few_samples() {
        if tracer(1).is_none() {
            return;
        }
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        let mut r = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow,
            path: Some(super::TraceOptions { spp: 8, bounces: 4, denoise: false, temporal: false }), ..Default::default() });
        let mut p = project(json!([{"id": "v", "kind": "mesh", "mesh": {"type": "box", "size": [300, 170], "depth": 400},
            "material": {"volume": {"density": 0.15, "color": "#ffffff", "anisotropy": 0.3}}, "transform": {"position": [160, 90]}}]));
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [0.3, -1, 0.2], "intensity": 2, "shadow": true}])).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
        let img = r.render(&Compiled::new(&p), 0.0);
        let g: Vec<f64> = (70..110).flat_map(|y| (120..200).map(move |x| (x, y))).map(|(x, y)| img.get_pixel(x, y).0[1] as f64).collect();
        let mean = g.iter().sum::<f64>() / g.len() as f64;
        let sd = (g.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / g.len() as f64).sqrt();
        assert!(mean > 20.0, "the haze is lit: {mean}");
        // Raw, undenoised, 8 paths: 41% before (37 on 90), 24% with every
        // path gathering single scattering; what is left is light scattered
        // more than once, for the denoiser.
        assert!(sd < 0.3 * mean, "noise {sd:.1} on a mean of {mean:.1}");
    }

    /// A soft shadow a catcher casts onto a card is denoised like the rest:
    /// the card keeps its exact colour and only how much light reaches it
    /// is filtered (it was left raw, speckled at export sample counts).
    #[test]
    fn soft_shadows_on_a_card_are_smooth_at_few_samples() {
        let scene = || {
            let mut p = project(json!([
                {"id": "card", "kind": "shape", "shape": "rect", "size": [320, 180], "fill": "#e8e8e8", "three_d": true, "transform": {"position": [160, 90]}},
                {"id": "catch", "kind": "mesh", "mesh": {"type": "plane", "size": [320, 180]}, "material": {"preset": "shadow", "opacity": 0.8},
                 "transform": {"position": [160, 90], "z": -2}},
                {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 40}, "material": {"preset": "matte"},
                 "transform": {"position": [160, 90], "z": -90}}
            ]));
            p.scenes[0].lights = serde_json::from_value(json!([{"type": "area", "position": [100, 20, -400], "aim": [160, 90, 0], "size": [260, 260], "shadow": true}])).unwrap();
            Compiled::new(&p)
        };
        // Pixel-to-pixel change beside the ball, across its soft shadow.
        let grain = |img: &image::RgbaImage| {
            let (mut sum, mut n) = (0.0, 0.0);
            for y in 60..150 {
                for x in 200..300 {
                    sum += (img.get_pixel(x, y).0[1] as f64 - img.get_pixel(x + 1, y).0[1] as f64).abs();
                    n += 1.0;
                }
            }
            sum / n
        };
        let Some(mut reference) = tracer(256) else { return };
        let Some(mut r) = tracer(4) else { return };
        let (want, got) = (grain(&reference.render(&scene(), 0.0)), grain(&r.render(&scene(), 0.0)));
        assert!(got < want * 2.5 + 1.0, "grain {got:.2} at 4 paths, {want:.2} at 256");
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

    /// An animated mesh is new geometry every frame: the tracer keeps the
    /// last few frames of it, not every frame since the start (its shared
    /// arrays only grew, and a minute of animated rods filled a 12 GB GPU).
    #[test]
    fn an_animated_mesh_keeps_the_trace_cache_bounded() {
        let Some(mut r) = tracer(1) else { return };
        let c = Compiled::new(&project(json!([{"id": "ball", "kind": "mesh",
            "mesh": {"type": "sphere", "radius": {"keys": [{"at": 0, "v": 50}, {"at": 2, "v": 52}]}}, "transform": {"position": [160, 90]}}])));
        let held: Vec<usize> = (0..40).map(|f| {
            r.render(&c, f as f64 / 20.0);
            r.trace_cached_vertices()
        }).collect();
        assert!(held[0] > 0, "the ball was traced");
        let most = *held.iter().max().unwrap();
        // This frame and the last two: at most about four balls' worth, not 40.
        assert!(most <= held[0] * 4, "held {most} vertices for a {}-vertex ball: {held:?}", held[0]);
    }

    /// A volume whose face lies exactly on a surface (a beam standing on a
    /// wafer): the surface under it is found and lit as with a gap between.
    #[test]
    fn a_surface_on_a_volumes_face_is_shaded_as_one_just_off_it() {
        let Some(mut r) = tracer(32) else { return };
        let mut scene = |beam_z: f64| {
            let mut p = project(json!([
                {"id": "wafer", "kind": "mesh", "mesh": {"type": "box", "size": [220, 180], "depth": 20}, "material": {"preset": "matte", "color": "#20c040", "specular": 0},
                 "transform": {"position": [160, 120]}},
                {"id": "beam", "kind": "mesh", "mesh": {"type": "box", "size": [120, 100], "depth": 60},
                 "material": {"volume": {"density": 0.3, "color": "#000000", "emission": "#2040ff"}},
                 "transform": {"position": [160, 120], "z": beam_z}}
            ]));
            p.settings.width = 320;
            p.settings.height = 240;
            p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [-0.3, -0.3, -1.0], "shadow": true}])).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#000000", "horizon": "#000000", "ground": "#000000"})).unwrap());
            r.render(&Compiled::new(&p), 0.0)
        };
        // In front of the wafer (toward the camera) and behind it.
        for side in [-1.0, 1.0] {
            let (on, off) = (scene(side * 40.0), scene(side * 42.0));
            let (a, b) = (disc(&on, 160.0, 120.0, 0.0, 35.0, |_, _| true), disc(&off, 160.0, 120.0, 0.0, 35.0, |_, _| true));
            assert!(b[1] > 60.0, "the wafer shows through the beam: {b:?}");
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 5.0, "beam at z {}: on the wafer {a:?}, 2 px off it {b:?}", side * 40.0);
            }
        }
    }

    /// A scene that moves: a ball over a floor under a turning light.
    fn rolling() -> lahza_motion_core::document::Project {
        let mut p = project(json!([
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#d0d0d0"}, "transform": {"position": [80, 60]}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 15}, "material": {"preset": "plastic", "color": "#ff2020"},
             "transform": {"position": {"keys": [{"at": 0, "v": [20, 60]}, {"at": 1, "v": [140, 60]}]}, "z": -30}}
        ]));
        p.settings.width = 160;
        p.settings.height = 120;
        p.settings.fps = lahza_motion_core::time::Fps::whole(10);
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [-0.5, -0.4, -1.0], "shadow": true}])).unwrap();
        p
    }

    fn sequence_renderer(denoise: bool, temporal: bool) -> Option<Renderer> {
        tracer(1)?;
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        Some(Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, path: Some(super::TraceOptions { spp: 8, bounces: 3, denoise, temporal }), ..Default::default() }))
    }

    /// Frames pushed through the renderer (the next begun while the GPU
    /// draws this one) are the frames it draws one at a time, in order:
    /// plain, motion-blurred, and with the denoiser between the two halves
    /// of each frame (three frames in flight).
    #[test]
    fn frames_in_flight_are_the_frames_drawn_one_at_a_time() {
        for (blur, oidn) in [(0u32, false), (4, false), (0, true), (4, true)] {
            let (Some(mut one), Some(mut many)) = (sequence_renderer(true, true), sequence_renderer(true, true)) else { return };
            if oidn {
                for r in [&mut one, &mut many] {
                    r.gpu.as_mut().unwrap().tracer_for_tests().unwrap().pass_through_oidn();
                }
            }
            let mut p = rolling();
            p.settings.motion_blur = (blur > 0).then_some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: blur });
            let c = Compiled::new(&p);
            // Frames 5 and 6 are the same moment: the second is a repeat.
            let times = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.5, 0.6];
            let want: Vec<image::RgbaImage> = times.iter().map(|t| one.render(&c, *t)).collect();
            let mut got = Vec::new();
            let mut most = 0;
            for t in times {
                got.extend(many.push(&c, t).unwrap());
                most = most.max(many.gpu.as_ref().unwrap().in_flight());
            }
            while let Some(frame) = many.pop().unwrap() {
                got.push(frame);
            }
            assert_eq!(most, if oidn { 2 } else { 1 }, "frames left on the GPU when a push returns (blur {blur}, denoiser {oidn})");
            assert_eq!(got.len(), want.len());
            for (k, (a, b)) in got.iter().zip(&want).enumerate() {
                assert!(a == b, "frame {k} differs (blur {blur}, denoiser between {oidn})");
            }
            assert!(got[1] != got[2], "the scene moves");
            assert!(many.warnings().iter().all(|w| !w.contains("GPU failed")), "{:?}", many.warnings());
        }
    }

    /// A motion-blurred frame's sub-frames are traced into one picture
    /// and denoised once, not once each; what goes to the denoiser and
    /// back is the picture (a denoiser that changes nothing leaves the
    /// frame as it is without one).
    #[test]
    fn sub_frames_are_one_picture_denoised_once() {
        let (Some(mut with), Some(mut without)) = (sequence_renderer(true, false), sequence_renderer(false, false)) else { return };
        with.gpu.as_mut().unwrap().tracer_for_tests().unwrap().pass_through_oidn();
        let mut p = rolling();
        p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: 4 });
        let c = Compiled::new(&p);
        let (a, b) = (with.render(&c, 0.5), without.render(&c, 0.5));
        assert_eq!(with.gpu.as_mut().unwrap().tracer_for_tests().unwrap().oidn_runs(), 1, "four sub-frames, one denoise");
        let worst = a.pixels().zip(b.pixels()).flat_map(|(x, y)| (0..4).map(move |k| x.0[k].abs_diff(y.0[k]))).max().unwrap();
        assert!(worst <= 2, "through a denoiser that changes nothing the frame differs by {worst}");
        // The ball streaks: half a frame of its 12 px a frame, on top of
        // its own 30 px.
        let covered = (0..160u32).filter(|&x| { let q = a.get_pixel(x, 60).0; q[0] > q[1].saturating_add(30) }).count();
        assert!((35..=48).contains(&covered), "the blurred ball covers {covered} px of the row");
    }

    /// Where the denoiser can work on the tracer's own memory (the device
    /// exports it, the denoiser imports it) nothing is copied through the
    /// host, the first picture is checked against the copied way, and the
    /// frames are the same either way. (A stand-in for the denoiser, which
    /// reads and writes the exported memory as files.)
    #[test]
    fn the_denoiser_works_on_the_tracers_memory_where_it_can() {
        let (Some(mut copied), Some(mut shared)) = (sequence_renderer(true, true), sequence_renderer(true, true)) else { return };
        if !shared.gpu.as_ref().unwrap().exports_memory() {
            eprintln!("skipping: the device does not export memory");
            return;
        }
        copied.gpu.as_mut().unwrap().tracer_for_tests().unwrap().pass_through_oidn();
        shared.gpu.as_mut().unwrap().tracer_for_tests().unwrap().pass_through_oidn_shared(false);
        let mut p = rolling();
        p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: 2 });
        let c = Compiled::new(&p);
        let times = [0.0, 0.1, 0.2, 0.3];
        let want: Vec<image::RgbaImage> = times.iter().map(|t| copied.render(&c, *t)).collect();
        // Pushed, so three frames (and three sets of shared memory) are in flight.
        let mut got = Vec::new();
        for t in times {
            got.extend(shared.push(&c, t).unwrap());
        }
        while let Some(frame) = shared.pop().unwrap() {
            got.push(frame);
        }
        let tracer = shared.gpu.as_mut().unwrap().tracer_for_tests().unwrap();
        assert_eq!(tracer.shares_with_oidn(), Ok(true), "checked on the first picture, shared from then on");
        // One run a picture, and the first picture once more to check it.
        assert_eq!(tracer.oidn_runs(), times.len() as u64 + 1);
        assert_eq!(got.len(), want.len());
        for (k, (a, b)) in got.iter().zip(&want).enumerate() {
            assert!(a == b, "frame {k} differs from the copied way");
        }
        assert!(shared.warnings().is_empty(), "{:?}", shared.warnings());
        // A denoiser whose shared pictures come out wrong (memory of
        // another device, a driver that maps it otherwise): the check on
        // the first picture catches it, that picture and the rest are
        // copied, and the frames are right all the same.
        let Some(mut bad) = sequence_renderer(true, true) else { return };
        bad.gpu.as_mut().unwrap().tracer_for_tests().unwrap().pass_through_oidn_shared(true);
        for (k, t) in times.iter().enumerate() {
            assert!(bad.render(&c, *t) == want[k], "frame {k} differs after the shared way was refused");
        }
        let refused = bad.gpu.as_mut().unwrap().tracer_for_tests().unwrap().shares_with_oidn();
        assert!(refused.as_ref().is_err_and(|why| why.contains("another picture")), "{refused:?}");
        assert!(bad.warnings().iter().any(|w| w.contains("copied through the host")), "{:?}", bad.warnings());
    }

    /// The tracer keeps its tables and structures from frame to frame
    /// instead of making and freeing them each time.
    #[test]
    fn the_tracer_keeps_its_buffers_between_frames() {
        let Some(mut r) = sequence_renderer(true, true) else { return };
        let mut p = rolling();
        p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: 2 });
        let c = Compiled::new(&p);
        r.render(&c, 0.1);
        let first = r.gpu.as_mut().unwrap().tracer_for_tests().unwrap().kept_buffers();
        assert!(first.len() >= 2 * 5 + 3, "two parts' tables and the lighting: {}", first.len());
        for k in 2..6 {
            r.render(&c, k as f64 / 10.0);
            assert!(r.gpu.as_mut().unwrap().tracer_for_tests().unwrap().kept_buffers() == first, "frame {k} made new buffers");
        }
    }

    /// A tracing renderer that counts its rays, and the ray queries a
    /// frame of `p` takes.
    fn counting(spp: u32) -> Option<Renderer> {
        tracer(spp)?;
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        Some(Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, profile: true, path: Some(super::TraceOptions { spp, bounces: 4, denoise: false, ..Default::default() }), ..Default::default() }))
    }

    fn rays(r: &mut Renderer, p: &lahza_motion_core::document::Project) -> (u64, image::RgbaImage) {
        let img = r.render(&Compiled::new(p), 0.0);
        (std::mem::take(&mut r.profile.as_mut().unwrap().rays), img)
    }

    /// A surface that reflects nothing (black, no specular) ends its paths
    /// as an unlit one does: no light sampling, no bounce.
    #[test]
    fn surfaces_that_reflect_nothing_end_their_paths() {
        let Some(mut r) = counting(8) else { return };
        let lights = json!([{"type": "directional", "from": [-0.5, -0.4, -1.0], "shadow": true}]);
        let env = json!({"preset": "studio"});
        let ball = |extra: serde_json::Value| {
            let mut m = json!({"preset": "matte", "color": "#000000", "emissive": "#ff8800", "specular": 0});
            for (k, v) in extra.as_object().unwrap() {
                m[k] = v.clone();
            }
            sphere_in(m, lights.clone(), env.clone())
        };
        let (unlit, a) = rays(&mut r, &ball(json!({"unlit": true})));
        let (dead, b) = rays(&mut r, &ball(json!({})));
        let (lit, _) = rays(&mut r, &ball(json!({"color": "#808080"})));
        assert!(lit * 2 > unlit * 3, "a lit ball samples lights and bounces: {lit} rays, unlit {unlit}");
        assert!(dead * 10 <= unlit * 11, "a black ball without specular traces as an unlit one: {dead} rays, unlit {unlit}");
        let (ca, cb) = (disc(&a, 160.0, 120.0, 0.0, 60.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 60.0, |_, _| true));
        assert!(cb[0] > 100.0 && cb[0] > cb[2] + 60.0, "and still glows: {cb:?} (unlit {ca:?})");
    }

    /// Subsurface walks are traced where they show: not when the light
    /// travels less than a texel inside, and not on paths that already
    /// scattered off a diffuse surface.
    #[test]
    fn subsurface_walks_only_where_they_show() {
        let Some(mut r) = counting(8) else { return };
        let lights = json!([{"type": "directional", "from": [-0.5, -0.4, -1.0], "shadow": true}]);
        let env = json!({"preset": "studio"});
        let ball = |extra: serde_json::Value| {
            let mut m = json!({"preset": "matte", "color": "#c8b8a0", "specular": 0});
            for (k, v) in extra.as_object().unwrap() {
                m[k] = v.clone();
            }
            sphere_in(m, lights.clone(), env.clone())
        };
        let (plain, a) = rays(&mut r, &ball(json!({})));
        let (walked, _) = rays(&mut r, &ball(json!({"subsurface": 1, "subsurface_scale": 8})));
        assert!(walked * 10 > plain * 13, "a visible walk costs rays: {walked}, plain {plain}");
        // A hundredth of a pixel: the walk leaves where it entered.
        let (tiny, b) = rays(&mut r, &ball(json!({"subsurface": 1, "subsurface_scale": 0.01})));
        assert!(tiny * 10 <= plain * 11, "light that travels 0.01 px inside is diffuse: {tiny} rays, plain {plain}");
        let (ca, cb) = (disc(&a, 160.0, 120.0, 0.0, 80.0, |_, _| true), disc(&b, 160.0, 120.0, 0.0, 80.0, |_, _| true));
        for k in 0..3 {
            assert!((ca[k] - cb[k]).abs() < 6.0, "and looks it: {cb:?}, plain {ca:?}");
        }
        // A diffuse floor fills the view; a slab above it, out of view, is
        // only found by the floor's bounced paths.
        let room = |slab: serde_json::Value| {
            let mut p = project(json!([
                {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [2000, 2000]}, "material": {"preset": "matte", "color": "#b0b0b0", "specular": 0},
                 "transform": {"position": [160, 200], "rotation_x": 90}},
                {"id": "slab", "kind": "mesh", "mesh": {"type": "box", "size": [1200, 60], "depth": 1200}, "material": slab,
                 "transform": {"position": [160, -200]}}
            ]));
            p.settings.width = 320;
            p.settings.height = 240;
            p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 40]})).unwrap();
            p.scenes[0].lights = serde_json::from_value(lights.clone()).unwrap();
            p.scenes[0].environment = Some(serde_json::from_value(env.clone()).unwrap());
            p
        };
        let (plain, _) = rays(&mut r, &room(json!({"preset": "matte", "color": "#c8b8a0", "specular": 0})));
        let (sss, _) = rays(&mut r, &room(json!({"preset": "matte", "color": "#c8b8a0", "specular": 0, "subsurface": 1, "subsurface_scale": 8})));
        assert!(sss * 100 <= plain * 104, "bounced light does not walk: {sss} rays, plain {plain}");
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

/// Counters before the per-block ones (pt.wgsl `PROF_HEADER`).
const PROF_HEADER: usize = 12;

/// Cells of a volume's density bounds (pt.wgsl `BOUNDS_N`³).
const BOUNDS_CELLS: usize = 32 * 32 * 32;

/// Trace features (pt.wgsl `F_*`): a scene's build compiles only those it
/// uses, so the kernel holds fewer registers and spills less.
const FEATURES: [(u32, &str); 9] = [
    (1 << 0, "F_VOLUME"),
    (1 << 1, "F_VOL_PHYSICAL"),
    (1 << 2, "F_GLASS"),
    (1 << 3, "F_SSS"),
    (1 << 4, "F_CARDS"),
    (1 << 5, "F_CATCHER"),
    (1 << 6, "F_AREA"),
    (1 << 7, "F_ENV_IMAGE"),
    (1 << 8, "F_DETAIL"),
];
const ALL_FEATURES: u32 = (1 << 9) - 1;

/// The tracer's source with only `features` on (and counting with `profile`).
fn specialise(features: u32, profile: bool) -> String {
    let mut s = SHADER.to_string();
    for (bit, name) in FEATURES {
        if features & bit == 0 {
            s = s.replacen(&format!("const {name}: bool = true;"), &format!("const {name}: bool = false;"), 1);
        }
    }
    if profile {
        s = s.replacen("const PROFILE: bool = false;", "const PROFILE: bool = true;", 1);
    }
    s
}

/// The features a trace needs: from its materials (rows as `mesh3d::material_rows`), cards and lights.
fn scene_features(mat_rows: &[[f32; 4]], t: &Trace) -> u32 {
    use crate::mesh3d::{FLAG_SHADOW_CATCHER, FLAG_VOLUME, LROWS, MAT_ROWS};
    let mut f = 0;
    for m in mat_rows.chunks_exact(MAT_ROWS) {
        let flags = m[2][3] as u32;
        if flags & FLAG_VOLUME != 0 {
            f |= 1 << 0;
            // Even volumes and physical noise or grid ones are walked exactly.
            if m[30][2] < 0.5 || m[24][3] < 0.5 {
                f |= 1 << 1;
            }
        }
        if m[3][0] > 0.0 {
            f |= 1 << 2;
        }
        if m[27][0] > 0.0 {
            f |= 1 << 3;
        }
        if flags & FLAG_SHADOW_CATCHER != 0 {
            f |= 1 << 5;
        }
        if m[9][0] > 0.5 || m[14][3] != 0.0 || [12, 13, 25, 26, 29].iter().any(|&k| m[k][3] > 0.5) {
            f |= 1 << 8;
        }
    }
    if !t.cards.is_empty() {
        f |= 1 << 4;
    }
    let count = t.lights.first().map_or(0, |r| r[0] as usize);
    if (0..count).any(|i| t.lights.get(7 + i * LROWS).is_some_and(|r| r[3] as u32 == 3)) {
        f |= 1 << 6;
    }
    if t.env.is_some() || t.lights.first().is_some_and(|r| r[2] > 0.5) {
        f |= 1 << 7;
    }
    f
}

/// Profiling experiment: `LAHZA_PROFILE_DENSITY_REPEAT=k` evaluates every
/// density lookup's noise k more times (profiling builds only), so the
/// trace's growth is the cost of the density work in place.
fn density_repeat() -> u32 {
    std::env::var("LAHZA_PROFILE_DENSITY_REPEAT").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0).min(16)
}

/// Rows per trace band (each submitted on its own, so the desktop gets the
/// GPU between them: about 70 ms each at 1080p and 16 samples).
const TRACE_BAND_ROWS: u32 = 32;
