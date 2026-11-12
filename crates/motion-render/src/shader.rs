//! Shader layers' pictures (`lahza_motion_core::slang_layer`): each fills
//! its group's texels, which the layer is composited from like any group.
//! The CPU calls the author's function per pixel through the project's
//! Slang library; the GPU dispatches its generated kernel, one compute
//! pipeline per kernel, kept while frames use it.

use crate::list::Group;
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use lahza_motion_core::slang_layer::{Program, MAX_PARAMS};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A shader layer's picture this frame (`Group::shader` indexes these).
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderPass {
    pub program: Arc<Program>,
    /// Seconds since the layer started.
    pub t: f32,
    /// The layer's size (px = uv · size).
    pub size: [f32; 2],
    /// The arguments after `t`.
    pub params: Vec<[f32; 4]>,
}

impl ShaderPass {
    /// Bytes that determine its pixels (frame cache key).
    pub fn signature(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.program.key.to_le_bytes());
        out.extend_from_slice(bytemuck::cast_slice(&[self.t, self.size[0], self.size[1]]));
        out.extend_from_slice(bytemuck::cast_slice(&self.params));
    }
}

/// Paints `pass` into group `g`'s texels (premultiplied), rows split over
/// `threads`. Transparent when its function did not build (an error the
/// project reports).
pub fn paint_cpu(pass: &ShaderPass, g: &Group, groups: &mut [[f32; 4]], threads: usize) {
    let (w, h) = (g.target.width as usize, g.target.height as usize);
    let out = &mut groups[g.offset as usize..g.offset as usize + w * h];
    let Some(f) = pass.program.pixel() else {
        out.fill([0.0; 4]);
        return;
    };
    let rows = h.div_ceil(threads.max(1)).max(1);
    std::thread::scope(|scope| {
        for (band, chunk) in out.chunks_mut(rows * w).enumerate() {
            scope.spawn(move || {
                for (i, texel) in chunk.iter_mut().enumerate() {
                    let (x, y) = (i % w, band * rows + i / w);
                    // As the GPU kernel computes them, in f32.
                    let u = (x as f32 + 0.5) / w as f32;
                    let v = (y as f32 + 0.5) / h as f32;
                    let mut c = [0f32; 4];
                    // SAFETY: the signature `Program::new` exported; the
                    // arguments outlive the call.
                    unsafe { f(u, v, u * pass.size[0], v * pass.size[1], pass.t, pass.params.as_ptr(), &mut c) };
                    if !c.iter().all(|v| v.is_finite()) {
                        c = [0.0; 4];
                    }
                    let a = c[3].clamp(0.0, 1.0);
                    *texel = [c[0] * a, c[1] * a, c[2] * a, a];
                }
            });
        }
    });
}

/// `wgsl` without Slang's `@binding(n) @group(n)` attributes: blade
/// binds globals by name and lays out the groups itself.
pub(crate) fn unbound(wgsl: &str) -> String {
    let mut out = String::with_capacity(wgsl.len());
    let mut rest = wgsl;
    while let Some(i) = rest.find('@') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let attr = ["@binding(", "@group("].iter().find(|a| tail.starts_with(**a));
        match attr.and_then(|a| tail[a.len()..].find(')').map(|j| a.len() + j + 1)) {
            Some(end) => rest = tail[end..].trim_start_matches(' '),
            None => {
                out.push('@');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The kernel's uniform (`LahzaShaderPass` in `slang_layer`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    size: [u32; 4],
    time: [f32; 4],
    params: [[f32; 4]; MAX_PARAMS],
}

/// Named as Slang names the kernel's globals.
#[derive(blade_macros::ShaderData)]
struct ShaderLayerData {
    u_shader_0: Uniform,
    b_pixels_0: gpu::BufferPiece,
}

/// Kernels' pipelines by WGSL hash. Projects change while the app runs:
/// pipelines no frame used lately are dropped.
#[derive(Default)]
pub struct Pipelines {
    by_hash: HashMap<u64, (gpu::ComputePipeline, u64)>,
    frame: u64,
}

/// Pipelines kept beyond those the current frame uses.
const KEEP: usize = 16;

impl Pipelines {
    /// Records `pass` filling group `g` of `groups`. Ok(false) when its
    /// kernel did not build (the caller leaves the group transparent).
    pub fn dispatch(&mut self, context: &gpu::Context, encoder: &mut gpu::CommandEncoder, pass: &ShaderPass, g: &Group, groups: gpu::Buffer) -> Result<bool, String> {
        let Some(wgsl) = pass.program.wgsl() else { return Ok(false) };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        wgsl.hash(&mut h);
        let key = h.finish();
        if !self.by_hash.contains_key(&key) {
            let source = unbound(wgsl);
            let shader = context.try_create_shader(gpu::ShaderDesc { source: &source }).map_err(|e| format!("shader '{}': WGSL {e}", pass.program.function))?;
            let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc {
                name: "shader layer",
                data_layouts: &[&<ShaderLayerData as gpu::ShaderData>::layout()],
                compute: shader.at(lahza_motion_core::slang_layer::ENTRY),
            });
            self.by_hash.insert(key, (pipeline, self.frame));
        }
        let entry = self.by_hash.get_mut(&key).expect("inserted");
        entry.1 = self.frame;
        let t = g.target;
        let mut params = [[0f32; 4]; MAX_PARAMS];
        for (slot, p) in params.iter_mut().zip(&pass.params) {
            *slot = *p;
        }
        let data = ShaderLayerData {
            u_shader_0: Uniform { size: [t.width, t.height, g.offset, 0], time: [pass.t, 0.0, pass.size[0], pass.size[1]], params },
            b_pixels_0: groups.into(),
        };
        let mut compute = encoder.compute("shader layer");
        let mut pc = compute.with(&entry.0);
        pc.bind(0, &data);
        pc.dispatch([t.width.div_ceil(16), t.height.div_ceil(16), 1]);
        Ok(true)
    }

    /// After a frame finished on the GPU: drops pipelines no recent frame
    /// used once there are too many.
    pub fn end_frame(&mut self, context: &gpu::Context) {
        let frame = self.frame;
        self.frame += 1;
        if self.by_hash.len() <= KEEP {
            return;
        }
        self.by_hash.retain(|_, (pipeline, used)| {
            let keep = *used == frame;
            if !keep {
                context.destroy_compute_pipeline(pipeline);
            }
            keep
        });
    }

    pub fn destroy(&mut self, context: &gpu::Context) {
        for (_, (mut pipeline, _)) in self.by_hash.drain() {
            context.destroy_compute_pipeline(&mut pipeline);
        }
    }
}
