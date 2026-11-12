//! Slang meshes the GPU generates itself, straight into its mesh pool
//! (`MeshParams::gpu_generator`): the author's functions run as a compute
//! kernel (`slang_mesh::Kernel`), one thread per point and per triangle,
//! and Lahza's own kernels (meshgen.wgsl) finish the rows the way
//! `SlangShape::generate` and `Pooled::write` make them on the CPU. A
//! frame of animated geometry then sends the GPU a few numbers, not the
//! mesh.
//!
//! The CPU never sees the points, so what it would have reported (values
//! that are not numbers, triangles naming points the mesh does not have)
//! the kernels count per mesh; the counts come back with the frame.

use crate::mesh3d::{Pooled, Source};
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use lahza_motion_core::mesh::{MeshShape, PART_FRONT};
use lahza_motion_core::slang_mesh::{SlangShape, ENTRY, GPU_PARAMS, GROUP};
use std::collections::HashMap;
use std::sync::Arc;

/// The author's kernel's uniform (`LahzaMeshPass` in `slang_mesh`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct KernelPass {
    counts: [u32; 4],
    time: [f32; 4],
    params: [[f32; 4]; GPU_PARAMS / 4],
}

/// Named as Slang names the kernel's globals.
#[derive(blade_macros::ShaderData)]
struct KernelData {
    u_mesh_0: KernelPass,
    b_mesh_0: gpu::BufferPiece,
}

/// `GenPass` in meshgen.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GenPass {
    counts: [u32; 4],
    more: [u32; 4],
    lo: [f32; 4],
    hi: [f32; 4],
}

#[derive(blade_macros::ShaderData)]
struct GenData {
    u_gen: GenPass,
    b_rows: gpu::BufferPiece,
    b_status: gpu::BufferPiece,
}

/// `WeldPass` in meshweld.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WeldPass {
    counts: [u32; 4],
}

#[derive(blade_macros::ShaderData)]
struct WeldData {
    u_weld: WeldPass,
    b_rows: gpu::BufferPiece,
    b_table: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct NormalsData {
    u_gen: GenPass,
    b_words: gpu::BufferPiece,
}

const HAS_NORMALS: u32 = 1;
const HAS_UVS: u32 = 2;
const TWO_SIDED: u32 = 4;

/// Meshes a frame reports on one by one; the rest of a frame's share the
/// last slot.
const SLOTS: usize = 64;
const STATUS_BYTES: u64 = (SLOTS * 16) as u64;
/// Authors' pipelines kept beyond those the current frame uses.
const KEEP: usize = 16;

/// Lahza's own kernels and what they count into.
struct Finish {
    points: gpu::ComputePipeline,
    triangles: gpu::ComputePipeline,
    normals: gpu::ComputePipeline,
    /// The same for a mesh a deformer moved.
    deform_points: gpu::ComputePipeline,
    deform_triangles: gpu::ComputePipeline,
    /// A generated mesh readied for a deformer (meshweld.wgsl).
    weld_sum: gpu::ComputePipeline,
    weld_write: gpu::ComputePipeline,
    /// A generated mesh subdivided for a deformer (meshsubdivide.wgsl).
    sub_edges: gpu::ComputePipeline,
    sub_split: gpu::ComputePipeline,
    status: gpu::Buffer,
}

/// The counts of one frame's meshes, on their way back.
struct Report {
    frame: u64,
    host: gpu::Buffer,
    /// Per slot: the meshes counted there (function, points, whether a
    /// deformer's: refused whole where a point is not a number).
    meshes: Vec<Vec<(String, u32, bool)>>,
}

/// A mesh to generate: where its rows are in the pool, and for one a
/// deformer moves, where the mesh before the move is (`DeformBase`).
pub(crate) struct Job {
    pub at: u32,
    pub before: u32,
    /// For a deformer moving a mesh the GPU generated: that mesh's first
    /// row, readied for the deformer at `before` (`meshweld.wgsl`).
    pub weld: Option<u32>,
    /// The subdivisions the deformer asks for of that generated mesh, one
    /// after the other (`meshsubdivide.wgsl`); `weld` is the last's.
    pub levels: Vec<Level>,
    pub mesh: Arc<Pooled>,
}

/// One subdivision: the mesh at row `from` (`points`, `triangles`) into
/// the rows at `to`.
pub(crate) struct Level {
    pub from: u32,
    pub to: u32,
    pub points: u32,
    pub triangles: u32,
}

/// `SubPass` in meshsubdivide.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SubPass {
    counts: [u32; 4],
    more: [u32; 4],
}

#[derive(blade_macros::ShaderData)]
struct SubData {
    u_sub: SubPass,
    b_rows: gpu::BufferPiece,
    b_table: gpu::BufferPiece,
}

/// Places of a subdivision's edge table: twice the edges' claims.
fn edge_places(triangles: u32) -> u32 {
    (triangles as u64 * 6).next_power_of_two().clamp(64, 1 << 31) as u32
}

/// The kernel that makes a pooled mesh on the GPU, its WGSL and the
/// author's function: a generator's, or a deformer's.
fn kernel(mesh: &Pooled) -> Option<(&lahza_motion_core::slang_mesh::Kernel, &Arc<str>, &str)> {
    match &mesh.source {
        Source::Generated(params) => params.gpu_generator().and_then(|(s, wgsl)| Some((&**s.gpu.as_ref()?, wgsl, s.function.as_str()))),
        Source::Deformed { params, .. } | Source::DeformedGenerated { params, .. } => params.gpu_deformer().and_then(|(d, wgsl)| Some((&**d.gpu.as_ref()?, wgsl, d.function.as_str()))),
        Source::Mesh(_) => None,
    }
}

#[derive(Default)]
pub(crate) struct Generators {
    /// Authors' kernels by `Kernel::key`: the pipeline and the frame that
    /// last used it; None when its WGSL did not build here.
    kernels: HashMap<u64, Option<(gpu::ComputePipeline, u64)>>,
    finish: Option<Finish>,
    reports: Vec<Report>,
    spare: Vec<gpu::Buffer>,
    /// The hash table welds and subdivisions work in and its bytes;
    /// tables outgrown, with the frame whose work may still use each.
    table: Option<(gpu::Buffer, usize)>,
    old_tables: Vec<(u64, gpu::Buffer)>,
    /// Meshes generated since the profile last took the count.
    pub generated: u64,
}

/// The Slang generator of a pooled mesh the GPU makes.
pub(crate) fn shape(mesh: &Pooled) -> Option<&SlangShape> {
    match &mesh.source {
        Source::Generated(params) => match &params.shape {
            MeshShape::Slang(s) => Some(s),
            _ => None,
        },
        Source::Mesh(_) | Source::Deformed { .. } | Source::DeformedGenerated { .. } => None,
    }
}

impl Generators {
    /// Whether the GPU can generate `mesh` now: its kernel's pipeline is
    /// there (built on first use, for frame `frame`). Err: why not, once
    /// per kernel (the caller generates the mesh on the CPU instead).
    pub fn ready(&mut self, context: &gpu::Context, mesh: &Pooled, frame: u64) -> Result<bool, String> {
        let Some((kernel, wgsl, function)) = kernel(mesh) else { return Ok(false) };
        if let Some(known) = self.kernels.get(&kernel.key) {
            return Ok(known.is_some());
        }
        let source = crate::shader::unbound(wgsl);
        match context.try_create_shader(gpu::ShaderDesc { source: &source }) {
            Ok(shader) => {
                let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh", data_layouts: &[&<KernelData as gpu::ShaderData>::layout()], compute: shader.at(ENTRY) });
                self.kernels.insert(kernel.key, Some((pipeline, frame)));
                Ok(true)
            }
            Err(e) => {
                self.kernels.insert(kernel.key, None);
                Err(format!("slang mesh '{function}': its GPU kernel is not WGSL this GPU takes ({e}); the CPU generates it"))
            }
        }
    }

    fn finish(&mut self, context: &gpu::Context) -> &Finish {
        self.finish.get_or_insert_with(|| {
            let shader = context.create_shader(gpu::ShaderDesc { source: include_str!("meshgen.wgsl") });
            let normals = context.create_shader(gpu::ShaderDesc { source: include_str!("meshgen_normals.wgsl") });
            let layout = <GenData as gpu::ShaderData>::layout();
            let weld = context.create_shader(gpu::ShaderDesc { source: include_str!("meshweld.wgsl") });
            let weld_layout = <WeldData as gpu::ShaderData>::layout();
            let sub = context.create_shader(gpu::ShaderDesc { source: include_str!("meshsubdivide.wgsl") });
            let sub_layout = <SubData as gpu::ShaderData>::layout();
            Finish {
                points: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh points", data_layouts: &[&layout], compute: shader.at("gen_points") }),
                triangles: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh triangles", data_layouts: &[&layout], compute: shader.at("gen_triangles") }),
                normals: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh normals", data_layouts: &[&<NormalsData as gpu::ShaderData>::layout()], compute: normals.at("gen_face_normals") }),
                deform_points: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "deformed mesh points", data_layouts: &[&layout], compute: shader.at("deform_points") }),
                deform_triangles: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "deformed mesh triangles", data_layouts: &[&layout], compute: shader.at("deform_triangles") }),
                weld_sum: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh weld sums", data_layouts: &[&weld_layout], compute: weld.at("weld_sum") }),
                weld_write: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh weld", data_layouts: &[&weld_layout], compute: weld.at("weld_write") }),
                sub_edges: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh subdivide edges", data_layouts: &[&sub_layout], compute: sub.at("sub_edges") }),
                sub_split: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh subdivide", data_layouts: &[&sub_layout], compute: sub.at("sub_split") }),
                status: context.create_buffer(gpu::BufferDesc { name: "slang mesh status", size: STATUS_BYTES, memory: gpu::Memory::Device }),
            }
        })
    }

    /// Records frame `frame` generating `jobs` (each `ready`) into `pool`.
    pub fn run(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, frame: u64, pool: gpu::Buffer, jobs: &[Job]) {
        if jobs.is_empty() {
            return;
        }
        self.finish(context);
        // What each job's kernels are told.
        let passes: Vec<(&SlangShape, GenPass)> = jobs
            .iter()
            .enumerate()
            .filter_map(|(k, job)| {
                let s = shape(&job.mesh)?;
                let [lo, hi] = s.bounds?;
                let flags = if s.outputs.normals { HAS_NORMALS } else { 0 } | if s.outputs.uvs { HAS_UVS } else { 0 } | if s.two_sided { TWO_SIDED } else { 0 };
                // (`Pooled::write`'s record: part, every edge real, two-sided.)
                let fixed = PART_FRONT << 16 | 7 << 19 | (s.two_sided as u32) << 22;
                let v = |p: [f64; 3]| [p[0] as f32, p[1] as f32, p[2] as f32, 0.0];
                Some((s, GenPass { counts: [s.vertices, s.triangles, job.at, flags], more: [fixed, k.min(SLOTS - 1) as u32, 0, 0], lo: v(lo), hi: v(hi) }))
            })
            .collect();
        // Meshes deformers move: (the deformer, points, triangles, what
        // the finishing kernels are told, where the mesh before is).
        let moves: Vec<(&lahza_motion_core::slang_mesh::Deform, GenPass, u32)> = jobs
            .iter()
            .enumerate()
            .filter_map(|(k, job)| {
                let (Source::Deformed { params, .. } | Source::DeformedGenerated { params, .. }) = &job.mesh.source else { return None };
                let d = params.deform.as_ref()?;
                Some((d, GenPass { counts: [job.mesh.verts, job.mesh.tris, job.at, HAS_UVS], more: [0, k.min(SLOTS - 1) as u32, job.before, 0], lo: [0.0; 4], hi: [0.0; 4] }, job.before))
            })
            .collect();
        // The table the welds share (one after the other): places for
        // twice the points of the largest.
        let welds: Vec<(u32, &Job)> = jobs.iter().filter_map(|j| Some((j.weld?, j))).collect();
        // (A weld's place is four words, a subdivision's one.)
        let bytes = welds.iter().map(|(_, j)| (j.mesh.verts as usize * 2).next_power_of_two().max(64) * 16).chain(jobs.iter().flat_map(|j| j.levels.iter().map(|l| edge_places(l.triangles) as usize * 4))).max().unwrap_or(0);
        if bytes > self.table.as_ref().map_or(0, |t| t.1) {
            if let Some((old, _)) = self.table.take() {
                self.old_tables.push((frame, old));
            }
            self.table = Some((context.create_buffer(gpu::BufferDesc { name: "slang mesh weld table", size: bytes as u64, memory: gpu::Memory::Device }), bytes));
        }
        let table = self.table.as_ref().map(|t| t.0);
        let finish = self.finish.as_ref().expect("made above");
        let status = finish.status;
        encoder.transfer("slang mesh status").fill_buffer(status.into(), STATUS_BYTES, 0);
        let groups = |n: u32| [n.div_ceil(GROUP), 1, 1];
        let spread = |values: &[f32]| {
            let mut params = [[0f32; 4]; GPU_PARAMS / 4];
            for (k, v) in values.iter().take(GPU_PARAMS).enumerate() {
                params[k / 4][k % 4] = *v;
            }
            params
        };
        // Generators first: a deformer may move what one of them makes.
        if !passes.is_empty() {
            {
                // The author's functions: points and triangles, as returned.
                let mut pass = encoder.compute("slang mesh");
                for (s, g) in &passes {
                    let Some(Some((pipeline, used))) = s.gpu.as_ref().and_then(|k| self.kernels.get_mut(&k.key)) else { continue };
                    *used = frame;
                    let params = spread(&s.params);
                    for (what, n) in [(0, s.vertices), (1, s.triangles)] {
                        let mut pc = pass.with(pipeline);
                        pc.bind(0, &KernelData { u_mesh_0: KernelPass { counts: [g.counts[0], g.counts[1], g.counts[2], what], time: [s.time, 0.0, 0.0, 0.0], params }, b_mesh_0: pool.into() });
                        pc.dispatch(groups(n));
                    }
                }
            }
            if passes.iter().any(|(s, _)| !s.outputs.normals) {
                let mut pass = encoder.compute("slang mesh normals");
                for (s, g) in passes.iter().filter(|(s, _)| !s.outputs.normals) {
                    let mut pc = pass.with(&finish.normals);
                    pc.bind(0, &NormalsData { u_gen: *g, b_words: pool.into() });
                    pc.dispatch(groups(s.triangles));
                }
            }
            // (Triangles read their points' finished normals: a pass each.)
            for (label, pipeline, triangles) in [("slang mesh points", &finish.points, false), ("slang mesh triangles", &finish.triangles, true)] {
                let mut pass = encoder.compute(label);
                for (s, g) in &passes {
                    let mut pc = pass.with(pipeline);
                    pc.bind(0, &GenData { u_gen: *g, b_rows: pool.into(), b_status: status.into() });
                    pc.dispatch(groups(if triangles { s.triangles } else { s.vertices }));
                }
            }
        }
        // Generated meshes readied for their deformers (meshweld.wgsl):
        // from the generated mesh at `from` to the three rows a point the
        // deformer reads at `before`, its triangles as they are.
        if let Some(table) = table {
            // First subdivided as the deformer asks (meshsubdivide.wgsl):
            // a level's points are the level before's, then its edges'
            // midpoints.
            for level in jobs.iter().flat_map(|j| &j.levels) {
                let places = edge_places(level.triangles);
                {
                    let mut pass = encoder.transfer("slang mesh subdivide");
                    pass.fill_buffer(table.into(), places as u64 * 4, 0);
                    pass.copy_buffer_to_buffer(pool.at(level.from as u64 * 16), pool.at(level.to as u64 * 16), level.points as u64 * crate::mesh3d::PROWS as u64 * 16);
                }
                let data = SubData { u_sub: SubPass { counts: [level.points, level.triangles, level.from, level.to], more: [places - 1, 0, 0, 0] }, b_rows: pool.into(), b_table: table.into() };
                // (Every edge has its place before any triangle looks.)
                for (label, pipeline) in [("slang mesh subdivide edges", &finish.sub_edges), ("slang mesh subdivide", &finish.sub_split)] {
                    let mut pass = encoder.compute(label);
                    let mut pc = pass.with(pipeline);
                    pc.bind(0, &data);
                    pc.dispatch(groups(level.triangles));
                }
            }
            for (from, job) in &welds {
                let (points, triangles) = (job.mesh.verts, job.mesh.tris);
                let places = (points as usize * 2).next_power_of_two().max(64) as u32;
                {
                    let mut pass = encoder.transfer("slang mesh weld");
                    pass.fill_buffer(table.into(), places as u64 * 16, 0);
                    pass.copy_buffer_to_buffer(pool.at((from + points * crate::mesh3d::PROWS as u32) as u64 * 16), pool.at((job.before + points * crate::mesh3d::BROWS as u32) as u64 * 16), triangles as u64 * 16);
                }
                let data = WeldData { u_weld: WeldPass { counts: [points, *from, job.before, places - 1] }, b_rows: pool.into(), b_table: table.into() };
                for (label, pipeline) in [("slang mesh weld sums", &finish.weld_sum), ("slang mesh weld", &finish.weld_write)] {
                    let mut pass = encoder.compute(label);
                    let mut pc = pass.with(pipeline);
                    pc.bind(0, &data);
                    pc.dispatch(groups(points));
                }
            }
        }
        if !moves.is_empty() {
            {
                // A moved mesh's triangles are those of the mesh before.
                let mut pass = encoder.transfer("deformed mesh triangles");
                for (_, g, before) in &moves {
                    let [points, triangles, at, _] = g.counts;
                    pass.copy_buffer_to_buffer(pool.at((before + points * crate::mesh3d::BROWS as u32) as u64 * 16), pool.at((at + points * crate::mesh3d::PROWS as u32) as u64 * 16), triangles as u64 * 16);
                }
            }
            {
                // Deformers: every point of the mesh before, moved.
                let mut pass = encoder.compute("slang mesh deform");
                for (d, g, before) in &moves {
                    let Some(Some((pipeline, used))) = d.gpu.as_ref().and_then(|k| self.kernels.get_mut(&k.key)) else { continue };
                    *used = frame;
                    let mut pc = pass.with(pipeline);
                    pc.bind(0, &KernelData { u_mesh_0: KernelPass { counts: [g.counts[0], *before, g.counts[2], 0], time: [d.time, 0.0, 0.0, 0.0], params: spread(&d.params) }, b_mesh_0: pool.into() });
                    pc.dispatch(groups(g.counts[0]));
                }
            }
            {
                // (A moved mesh is shaded from its moved faces.)
                let mut pass = encoder.compute("slang mesh normals");
                for (_, g, _) in &moves {
                    let mut pc = pass.with(&finish.normals);
                    pc.bind(0, &NormalsData { u_gen: *g, b_words: pool.into() });
                    pc.dispatch(groups(g.counts[1]));
                }
            }
            for (label, pipeline, n) in [("deformed mesh points", &finish.deform_points, 0), ("deformed mesh sides", &finish.deform_triangles, 1)] {
                let mut pass = encoder.compute(label);
                for (_, g, _) in &moves {
                    let mut pc = pass.with(pipeline);
                    pc.bind(0, &GenData { u_gen: *g, b_rows: pool.into(), b_status: status.into() });
                    pc.dispatch(groups(g.counts[n]));
                }
            }
        }
        let host = self.spare.pop().unwrap_or_else(|| context.create_buffer(gpu::BufferDesc { name: "slang mesh report", size: STATUS_BYTES, memory: gpu::Memory::Download }));
        encoder.transfer("slang mesh report").copy_buffer_to_buffer(status.into(), host.into(), STATUS_BYTES);
        let mut meshes: Vec<Vec<(String, u32, bool)>> = vec![Vec::new(); SLOTS];
        for (s, g) in &passes {
            meshes[g.more[1] as usize].push((s.function.clone(), s.vertices, false));
        }
        for (d, g, _) in &moves {
            meshes[g.more[1] as usize].push((d.function.clone(), g.counts[0], true));
        }
        self.reports.push(Report { frame, host, meshes });
        self.generated += (passes.len() + moves.len()) as u64;
    }

    /// After frame `id`'s GPU work (and that of the frames before it):
    /// what their meshes' kernels counted, as errors (the picture is not
    /// the mesh the author asked for).
    pub fn after_frame(&mut self, context: &gpu::Context, id: u64, errors: &mut Vec<String>) {
        self.old_tables.retain(|(frame, b)| {
            if *frame <= id {
                context.destroy_buffer(*b);
            }
            *frame > id
        });
        let (done, waiting): (Vec<Report>, Vec<Report>) = std::mem::take(&mut self.reports).into_iter().partition(|r| r.frame <= id);
        self.reports = waiting;
        for r in done {
            context.sync_buffer(r.host);
            // SAFETY: the mapping holds `STATUS_BYTES`, and the frame that
            // wrote it has finished.
            let words: &[u32] = unsafe { std::slice::from_raw_parts(r.host.data() as *const u32, SLOTS * 4) };
            for (slot, meshes) in r.meshes.iter().enumerate().filter(|(_, m)| !m.is_empty()) {
                let w = &words[slot * 4..slot * 4 + 4];
                let names: Vec<&str> = meshes.iter().map(|m| m.0.as_str()).collect();
                let (name, one) = (names.join("', '"), meshes.len() == 1);
                let mut say = |e: String| {
                    if !errors.contains(&e) {
                        errors.push(e);
                    }
                };
                if w[0] > 0 && meshes.iter().any(|m| m.2) {
                    // A deformer's: refused whole, as the CPU refuses it.
                    let first = if one { format!(" (the first is point {})", !w[2]) } else { String::new() };
                    say(format!("deform '{name}': {} of its points moved to a value that is not a number{first}; the mesh is not drawn", w[0]));
                } else if w[0] > 0 {
                    let first = if one { format!(" (the first is point {})", !w[2]) } else { String::new() };
                    say(format!("slang mesh '{name}': {} of its points have a position, normal or UV that is not a number{first}; they are drawn at the origin", w[0]));
                }
                if w[1] > 0 {
                    let first = if one { format!(" (the first is {}_triangle({}), past the {} vertices)", meshes[0].0, !w[3], meshes[0].1) } else { String::new() };
                    say(format!("slang mesh '{name}': {} of its triangles name points it does not have{first}; they are not drawn", w[1]));
                }
            }
            self.spare.push(r.host);
        }
    }

    /// After frame `id`'s GPU work (and that of the frames before it):
    /// drops the authors' pipelines no later frame has used, once there
    /// are too many.
    pub fn end_frame(&mut self, context: &gpu::Context, id: u64) {
        if self.kernels.len() <= KEEP {
            return;
        }
        self.kernels.retain(|_, k| match k {
            Some((pipeline, used)) if *used <= id => {
                context.destroy_compute_pipeline(pipeline);
                false
            }
            _ => true,
        });
    }

    pub fn destroy(&mut self, context: &gpu::Context) {
        for (_, k) in self.kernels.drain() {
            if let Some((mut pipeline, _)) = k {
                context.destroy_compute_pipeline(&mut pipeline);
            }
        }
        if let Some(mut f) = self.finish.take() {
            for p in [&mut f.points, &mut f.triangles, &mut f.normals, &mut f.deform_points, &mut f.deform_triangles, &mut f.weld_sum, &mut f.weld_write, &mut f.sub_edges, &mut f.sub_split] {
                context.destroy_compute_pipeline(p);
            }
            context.destroy_buffer(f.status);
        }
        for b in self.spare.drain(..).chain(self.reports.drain(..).map(|r| r.host)).chain(self.table.take().map(|t| t.0)).chain(self.old_tables.drain(..).map(|t| t.1)) {
            context.destroy_buffer(b);
        }
    }
}
