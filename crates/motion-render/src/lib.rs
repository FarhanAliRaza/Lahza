//! Motion engine renderer (phase 1): SDF shapes, SDF glyphs, images,
//! nested comps, Gaussian blur and shutter motion blur.
//!
//! GPU-first on `blade-graphics`/Vulkan compute; the CPU path rasterises
//! the same display list with the same math and is used when no hardware
//! device is available (or the GPU fails mid-render). Determinism: time is
//! the only input, so rendering a project twice gives identical frames.
//!
//! Performance: a frame whose display list equals the previous one's is
//! returned from cache (stepped layers and holds cost nothing), the glyph
//! atlas and images upload only when they change, and blurred layers are
//! drawn at reduced resolution before blurring.

mod envmap;
pub mod export;
pub mod font;
pub mod footage;
pub mod gpu;
pub mod images;
pub mod list;
pub mod mesh3d;
pub mod pt;
pub mod raster;
pub mod sheet;

use font::Fonts;
use image::RgbaImage;
use images::Images;
use lahza_motion_core::compile::{Compiled, Content, FrameState, LayerState};
use list::{Builder, DisplayList};
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Hardware GPU when available, else CPU.
    Auto,
    Gpu,
    Cpu,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Backend::Auto),
            "gpu" => Some(Backend::Gpu),
            "cpu" => Some(Backend::Cpu),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// Output scale relative to the project size (0.5 = half-res proxy).
    pub scale: f64,
    pub motion_blur: bool,
    pub backend: Backend,
    /// Accept llvmpipe-style software Vulkan (tests).
    pub allow_software_gpu: bool,
    /// Path-trace meshes (export); `None` rasterises them (the preview).
    /// Needs a GPU with ray queries, else the frame is rasterised and a
    /// warning says so.
    pub path: Option<pt::TraceOptions>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            motion_blur: true,
            backend: Backend::Auto,
            allow_software_gpu: std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some(),
            path: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub frames: u64,
    pub cached: u64,
    pub build_ms: f64,
    pub raster_ms: f64,
}

pub struct Renderer {
    pub fonts: Fonts,
    pub images: Images,
    pub footage: footage::Footage,
    pub meshes: mesh3d::MeshCache,
    gpu: Option<gpu::Gpu>,
    gpu_error: Option<String>,
    /// GPU failures recovered or fallen back from (reported as warnings).
    fallbacks: Vec<String>,
    /// Tests: GPU renders to fail as a lost device would.
    #[cfg(test)]
    pub(crate) fail_gpu: u32,
    options: RenderOptions,
    last: Option<(Vec<u8>, RgbaImage)>,
    pub stats: Stats,
    threads: usize,
    /// CPU frame buffers, kept between frames: a fresh 33 MB buffer per
    /// 1080p frame costs more in page faults than drawing into it.
    scratch: Scratch,
}

#[derive(Default)]
struct Scratch {
    accum: Vec<[f32; 4]>,
    groups: Vec<[f32; 4]>,
    target: Vec<[f32; 4]>,
}

impl Renderer {
    /// `root` is the project folder (images and fonts resolve against it).
    pub fn new(root: Option<&Path>, options: RenderOptions) -> Self {
        let (gpu, gpu_error) = match options.backend {
            Backend::Cpu => (None, None),
            _ => match gpu_with_retry(options.allow_software_gpu) {
                Ok(g) => (Some(g), None),
                Err(e) => (None, Some(e)),
            },
        };
        Self {
            fonts: Fonts::new(root),
            images: Images::new(root),
            footage: footage::Footage::new(root),
            meshes: mesh3d::MeshCache::new(root),
            gpu,
            gpu_error,
            fallbacks: Vec::new(),
            #[cfg(test)]
            fail_gpu: 0,
            options,
            last: None,
            stats: Stats::default(),
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
            scratch: Scratch::default(),
        }
    }

    /// Tests: whether this GPU render should fail as a lost device would.
    #[cfg(test)]
    fn take_failure(&mut self) -> bool {
        let fail = self.fail_gpu > 0;
        self.fail_gpu = self.fail_gpu.saturating_sub(1);
        fail
    }

    /// Forgets the last frame, so the next one is drawn (benchmarks).
    #[cfg(test)]
    pub fn last_invalidate(&mut self) {
        self.last = None;
    }

    pub fn options(&self) -> RenderOptions {
        self.options
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.options.scale = scale.clamp(0.05, 4.0);
    }

    pub fn set_motion_blur(&mut self, on: bool) {
        self.options.motion_blur = on;
    }

    /// Forgets the fall-back warnings (GPU failures, no path tracing), so
    /// `warnings` reports those of the frames rendered from now on.
    pub fn clear_fallbacks(&mut self) {
        self.fallbacks.clear();
    }

    /// Path-trace meshes from the next frame (`None`: rasterise them).
    pub fn set_path(&mut self, path: Option<pt::TraceOptions>) {
        if self.options.path != path {
            self.options.path = path;
            self.last = None;
        }
    }

    /// "GPU · <device>" (plus "· path traced, N spp" when tracing) or
    /// "CPU (<why>)".
    pub fn backend_label(&self) -> String {
        match (&self.gpu, &self.gpu_error) {
            (Some(g), _) => match self.options.path {
                Some(o) if g.can_trace() => format!("{} · path traced, {} spp", g.label, o.spp),
                _ => g.label.clone(),
            },
            (None, Some(e)) => format!("CPU ({e})"),
            (None, None) => "CPU".into(),
        }
    }

    pub fn is_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// Whether `path` options will path-trace (a GPU with ray queries).
    pub fn can_trace(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.can_trace())
    }

    /// Vertices the path tracer holds for its geometry (tests).
    #[cfg(test)]
    pub(crate) fn trace_cached_vertices(&self) -> usize {
        self.gpu.as_ref().map_or(0, |g| g.trace_cached_vertices())
    }

    pub fn warnings(&self) -> Vec<String> {
        let mut w = self.fonts.warnings.clone();
        w.extend(self.images.warnings.iter().cloned());
        w.extend(self.footage.warnings.iter().cloned());
        w.extend(self.meshes.warnings.iter().cloned());
        w.extend(self.fallbacks.iter().cloned());
        w.sort();
        w.dedup();
        w
    }

    /// What could not be drawn as asked: images or LUTs that failed to load
    /// or did not fit the image pool, fonts that were substituted, footage
    /// that could not be read. These are errors: the picture is wrong.
    pub fn dropped(&self) -> Vec<String> {
        let mut d: Vec<String> = self.images.warnings.iter().cloned().collect();
        d.extend(self.fonts.warnings.iter().cloned());
        d.extend(self.footage.warnings.iter().cloned());
        d.extend(self.meshes.dropped.iter().cloned());
        d.sort();
        d.dedup();
        d
    }

    /// The frame at timeline time `t`, with motion blur when the scene asks.
    pub fn render(&mut self, compiled: &Compiled, t: f64) -> RgbaImage {
        let mut states = Vec::new();
        match compiled.motion_blur_at(t).filter(|_| self.options.motion_blur) {
            Some(mb) => {
                let n = mb.samples.clamp(2, 64);
                let open = mb.shutter.min(720.0) / 360.0 * compiled.fps().frame();
                for i in 0..n {
                    let ts = t + ((i as f64 + 0.5) / n as f64 - 0.5) * open;
                    states.push(compiled.frame(ts));
                }
                // A still picture needs one sample.
                if states.windows(2).all(|w| w[0].layers == w[1].layers && w[0].scene == w[1].scene) {
                    states.truncate(1);
                }
            }
            None => states.push(compiled.frame(t)),
        }
        self.render_states(&states)
    }

    /// Renders equally weighted sub-samples of one output frame.
    pub fn render_states(&mut self, states: &[FrameState]) -> RgbaImage {
        let started = Instant::now();
        self.footage.begin_frame();
        let trace = match (self.options.path, self.gpu.as_ref()) {
            (Some(p), Some(g)) if g.can_trace() => Some(p),
            (Some(_), g) => {
                let why = g.and_then(|g| g.trace_error.clone()).or_else(|| self.gpu_error.clone()).unwrap_or_else(|| "no GPU".into());
                let w = format!("path tracing needs a GPU with ray queries ({why}): rendered with the rasteriser");
                if !self.fallbacks.contains(&w) {
                    self.fallbacks.push(w);
                }
                None
            }
            (None, _) => None,
        };
        // Motion blur: each sub-frame traces its share of the samples, so
        // the frame costs what a still one does.
        let n = states.len().max(1) as u32;
        // Motion-blur sub-frames already average over the shutter; history
        // per sub-frame would hold a full-size buffer set for each of them.
        let trace = trace.map(|o| pt::TraceOptions { spp: o.spp.div_ceil(n).max(1), temporal: o.temporal && n == 1, ..o });
        let lists = self.build_lists(states, trace);
        let mut key = Vec::new();
        for l in &lists {
            key.extend(l.signature());
        }
        // Footage pixels are part of the picture.
        key.extend(self.footage.signature());
        let built = Instant::now();
        self.stats.build_ms += (built - started).as_secs_f64() * 1000.0;
        self.stats.frames += 1;
        if let Some((last_key, image)) = &self.last {
            if *last_key == key {
                self.stats.cached += 1;
                return image.clone();
            }
        }
        let main = lists[0].main();
        #[cfg(test)]
        let fail = self.take_failure();
        #[cfg(not(test))]
        let fail = false;
        let result = self.gpu.as_mut().map(|g| if fail { Err("ERROR_DEVICE_LOST (test)".to_string()) } else { g.render(&lists, &self.fonts, &self.images, &self.footage.dynamic) });
        let bytes = match result {
            Some(Ok(bytes)) => bytes,
            Some(Err(e)) => {
                // A lost device (contention, a driver reset) usually comes
                // back: make a new one and try the frame again before
                // falling back to the CPU for good, and say so either way.
                #[cfg(test)]
                let fail_again = self.take_failure();
                #[cfg(not(test))]
                let fail_again = false;
                let retried = gpu_with_retry(self.options.allow_software_gpu).ok().and_then(|mut g| {
                    let r = if fail_again { None } else { g.render(&lists, &self.fonts, &self.images, &self.footage.dynamic).ok() };
                    self.gpu = Some(g);
                    r
                });
                match retried {
                    Some(bytes) => {
                        self.fallbacks.push(format!("GPU failed at frame {} ({e}); a new device rendered it", self.stats.frames));
                        bytes
                    }
                    None => {
                        self.gpu = None;
                        self.gpu_error = Some(format!("GPU failed: {e}"));
                        self.fallbacks.push(format!("GPU failed at frame {} ({e}); this and later frames rendered on the CPU", self.stats.frames));
                        // The CPU cannot trace: rasterise the frame instead.
                        if trace.is_some() {
                            let raster = self.build_lists(states, None);
                            self.cpu(&raster)
                        } else {
                            self.cpu(&lists)
                        }
                    }
                }
            }
            None => self.cpu(&lists),
        };
        self.stats.raster_ms += built.elapsed().as_secs_f64() * 1000.0;
        let image = RgbaImage::from_raw(main.width, main.height, bytes).expect("frame size");
        self.last = Some((key, image.clone()));
        image
    }

    fn build_lists(&mut self, states: &[FrameState], trace: Option<pt::TraceOptions>) -> Vec<DisplayList> {
        let scale = self.options.scale;
        states
            .iter()
            .map(|s| {
                // Seeded by the frame's time: the same frame traces the same.
                let seed = ((s.t * 1000.0).round() as i64 as u64).wrapping_mul(0x9E37_79B9) as u32;
                Builder { fonts: &mut self.fonts, images: &mut self.images, footage: &mut self.footage, meshes: &mut self.meshes, scale, trace, seed }.build(s)
            })
            .collect()
    }

    fn cpu(&mut self, lists: &[DisplayList]) -> Vec<u8> {
        let main = lists[0].main();
        let (w, h) = (main.width as usize, main.height as usize);
        let weight = 1.0 / lists.len() as f32;
        let Scratch { accum, groups, target } = &mut self.scratch;
        // One sample writes every pixel; several add up into a cleared sum.
        accum.resize(w * h, [0.0; 4]);
        if lists.len() > 1 {
            accum.fill([0.0; 4]);
        }
        for list in lists {
            // Every group is drawn in full before it is read.
            groups.resize(list.group_texels as usize, [0.0; 4]);
            for g in &list.groups {
                let n = (g.target.width * g.target.height) as usize;
                // Outer groups sample inner ones, so draw into a scratch
                // buffer and copy it in place.
                target.resize(n, [0.0; 4]);
                let src = raster::Sources { atlas: &self.fonts.atlas, images: &self.images.texels, groups, footage: &self.footage.dynamic, ops: &list.ops };
                raster::raster(list, &g.target, &src, &mut target[..n], raster::Write::Replace, self.threads);
                groups[g.offset as usize..g.offset as usize + n].copy_from_slice(&target[..n]);
                raster::blur(groups, g.offset as usize, g.target.width as usize, g.target.height as usize, g.sigma, g.clamp, self.threads);
            }
            let src = raster::Sources { atlas: &self.fonts.atlas, images: &self.images.texels, groups, footage: &self.footage.dynamic, ops: &list.ops };
            let write = if lists.len() == 1 { raster::Write::Replace } else { raster::Write::Accumulate(weight) };
            // Each list has its own main target (its tile ranges sit after its groups).
            raster::raster(list, &list.main(), &src, accum, write, self.threads);
        }
        let mut out = vec![0u8; w * h * 4];
        raster::resolve(accum, &mut out, self.threads);
        out
    }

    /// Canvas bounds of the ink a text layer shows now, for `check`.
    pub fn measure(&mut self, layer: &LayerState) -> Option<[f64; 4]> {
        let Content::Text(text) = &layer.content else { return None };
        self.fonts.ink_bounds(text, layer.matrix)
    }
}

/// Makes the GPU renderer, trying again on transient failures (a device
/// busy or lost under contention); a missing or software-only device is not
/// retried.
fn gpu_with_retry(allow_software: bool) -> Result<gpu::Gpu, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        match gpu::Gpu::new(allow_software) {
            Ok(g) => return Ok(g),
            Err(e) if e.starts_with("only a software") || e.starts_with("no Vulkan device") && !e.contains("DEVICE_LOST") => return Err(e),
            Err(e) => last = e,
        }
        std::thread::sleep(std::time::Duration::from_millis(200 << attempt));
    }
    Err(last)
}

pub fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).expect("PNG encoding to memory");
    out.into_inner()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mesh_tests;
