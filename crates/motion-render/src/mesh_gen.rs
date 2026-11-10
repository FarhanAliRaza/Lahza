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
    status: gpu::Buffer,
}

/// The counts of one frame's meshes, on their way back.
struct Report {
    frame: u64,
    host: gpu::Buffer,
    /// Per slot: the meshes counted there (function, points).
    meshes: Vec<Vec<(String, u32)>>,
}

/// A mesh to generate: where its rows are in the pool.
pub(crate) struct Job {
    pub at: u32,
    pub mesh: Arc<Pooled>,
}

#[derive(Default)]
pub(crate) struct Generators {
    /// Authors' kernels by `Kernel::key`: the pipeline and the frame that
    /// last used it; None when its WGSL did not build here.
    kernels: HashMap<u64, Option<(gpu::ComputePipeline, u64)>>,
    finish: Option<Finish>,
    reports: Vec<Report>,
    spare: Vec<gpu::Buffer>,
    frame: u64,
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
        Source::Mesh(_) => None,
    }
}

impl Generators {
    /// Whether the GPU can generate `mesh` now: its kernel's pipeline is
    /// there (built on first use). Err: why not, once per kernel (the
    /// caller generates the mesh on the CPU instead).
    pub fn ready(&mut self, context: &gpu::Context, mesh: &Pooled) -> Result<bool, String> {
        let Some((s, wgsl)) = (match &mesh.source {
            Source::Generated(params) => params.gpu_generator(),
            Source::Mesh(_) => None,
        }) else {
            return Ok(false);
        };
        let Some(kernel) = s.gpu.as_ref() else { return Ok(false) };
        if let Some(known) = self.kernels.get(&kernel.key) {
            return Ok(known.is_some());
        }
        let source = crate::shader::unbound(wgsl);
        match context.try_create_shader(gpu::ShaderDesc { source: &source }) {
            Ok(shader) => {
                let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh", data_layouts: &[&<KernelData as gpu::ShaderData>::layout()], compute: shader.at(ENTRY) });
                self.kernels.insert(kernel.key, Some((pipeline, self.frame)));
                Ok(true)
            }
            Err(e) => {
                self.kernels.insert(kernel.key, None);
                Err(format!("slang mesh '{}': its GPU kernel is not WGSL this GPU takes ({e}); the CPU generates it", s.function))
            }
        }
    }

    fn finish(&mut self, context: &gpu::Context) -> &Finish {
        self.finish.get_or_insert_with(|| {
            let shader = context.create_shader(gpu::ShaderDesc { source: include_str!("meshgen.wgsl") });
            let normals = context.create_shader(gpu::ShaderDesc { source: include_str!("meshgen_normals.wgsl") });
            let layout = <GenData as gpu::ShaderData>::layout();
            Finish {
                points: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh points", data_layouts: &[&layout], compute: shader.at("gen_points") }),
                triangles: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh triangles", data_layouts: &[&layout], compute: shader.at("gen_triangles") }),
                normals: context.create_compute_pipeline(gpu::ComputePipelineDesc { name: "slang mesh normals", data_layouts: &[&<NormalsData as gpu::ShaderData>::layout()], compute: normals.at("gen_face_normals") }),
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
        let now = self.frame;
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
        let finish = self.finish.as_ref().expect("made above");
        let status = finish.status;
        encoder.transfer("slang mesh status").fill_buffer(status.into(), STATUS_BYTES, 0);
        let groups = |n: u32| [n.div_ceil(GROUP), 1, 1];
        {
            // The author's functions: points and triangles, as returned.
            let mut pass = encoder.compute("slang mesh");
            for (s, g) in &passes {
                let Some(Some((pipeline, used))) = s.gpu.as_ref().and_then(|k| self.kernels.get_mut(&k.key)) else { continue };
                *used = now;
                let mut params = [[0f32; 4]; GPU_PARAMS / 4];
                for (k, v) in s.params.iter().take(GPU_PARAMS).enumerate() {
                    params[k / 4][k % 4] = *v;
                }
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
        let host = self.spare.pop().unwrap_or_else(|| context.create_buffer(gpu::BufferDesc { name: "slang mesh report", size: STATUS_BYTES, memory: gpu::Memory::Download }));
        encoder.transfer("slang mesh report").copy_buffer_to_buffer(status.into(), host.into(), STATUS_BYTES);
        let mut meshes: Vec<Vec<(String, u32)>> = vec![Vec::new(); SLOTS];
        for (s, g) in &passes {
            meshes[g.more[1] as usize].push((s.function.clone(), s.vertices));
        }
        self.reports.push(Report { frame, host, meshes });
        self.generated += passes.len() as u64;
    }

    /// After frame `id`'s GPU work (and that of the frames before it):
    /// what their meshes' kernels counted, as errors (the picture is not
    /// the mesh the author asked for).
    pub fn after_frame(&mut self, context: &gpu::Context, id: u64, errors: &mut Vec<String>) {
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
                if w[0] > 0 {
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

    /// After a frame finished on the GPU: drops authors' pipelines no
    /// recent frame used once there are too many.
    pub fn end_frame(&mut self, context: &gpu::Context) {
        let frame = self.frame;
        self.frame += 1;
        if self.kernels.len() <= KEEP {
            return;
        }
        self.kernels.retain(|_, k| match k {
            Some((pipeline, used)) if *used != frame => {
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
            for p in [&mut f.points, &mut f.triangles, &mut f.normals] {
                context.destroy_compute_pipeline(p);
            }
            context.destroy_buffer(f.status);
        }
        for b in self.spare.drain(..).chain(self.reports.drain(..).map(|r| r.host)) {
            context.destroy_buffer(b);
        }
    }
}
