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
pub mod profile;
pub mod pt;
pub mod model;
mod ngx;
mod oidn;
mod mesh_gen;
mod pool;
pub mod raster;
pub mod shader;
pub mod sheet;
mod surfaces;

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
    /// Hardware GPU only: a missing or failing GPU is an error, never a
    /// silent switch to the much slower CPU.
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
    /// Record where the time goes (`Renderer::profile`): CPU stages, GPU
    /// pass timestamps and the path tracer's work per layer.
    pub profile: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            motion_blur: true,
            backend: Backend::Gpu,
            allow_software_gpu: std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some(),
            path: None,
            profile: false,
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
    /// The fast mode: the shutter of the frame being drawn, in frames (0: no motion blur).
    shutter: f32,
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
    /// Frames begun with `push` and not yet taken with `pop`, oldest first.
    queue: std::collections::VecDeque<Queued>,
    /// Counts the GPU devices this renderer has had: a queued frame begun
    /// on an earlier one (lost since) is drawn again when it is taken.
    device: u64,
    pub stats: Stats,
    /// With `RenderOptions::profile`: where the frames' time went.
    pub profile: Option<profile::Profile>,
    threads: usize,
    /// CPU frame buffers, kept between frames: a fresh 33 MB buffer per
    /// 1080p frame costs more in page faults than drawing into it.
    scratch: Scratch,
}

/// A frame between `Renderer::push` and `pop`.
struct Queued {
    /// What determines its picture (`DisplayList::signature`s).
    key: Vec<u8>,
    /// The scene at its samples, to draw it again if its device is lost.
    states: Vec<FrameState>,
    size: (u32, u32),
    work: Work,
}

enum Work {
    /// The same picture as the frame before it.
    Repeat,
    /// On the GPU: its flight there and the device it is on.
    Gpu { id: u64, device: u64 },
    /// Drawn when it is taken (the CPU; a frame the GPU refused).
    Later,
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
            _ => match gpu_with_retry(options.allow_software_gpu, options.profile) {
                Ok(g) => (Some(g), None),
                Err(e) => (None, Some(e)),
            },
        };
        let mut images = Images::new(root);
        if let Some(g) = &gpu {
            images.set_budget(g.image_budget());
        }
        Self {
            fonts: Fonts::new(root),
            images,
            footage: footage::Footage::new(root),
            meshes: mesh3d::MeshCache::new(root),
            gpu,
            gpu_error,
            fallbacks: Vec::new(),
            #[cfg(test)]
            fail_gpu: 0,
            options,
            last: None,
            queue: Default::default(),
            device: 0,
            stats: Stats::default(),
            shutter: 0.0,
            profile: options.profile.then(profile::Profile::default),
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
        w.extend(self.gpu.as_ref().and_then(|g| g.denoise_warning()));
        w.extend(self.gpu.as_ref().and_then(|g| g.pool_warning()));
        w.extend(self.gpu.iter().flat_map(|g| g.pool_notes().iter().cloned()));
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
        d.extend(self.gpu.iter().flat_map(|g| g.pool_errors().iter().cloned()));
        d.sort();
        d.dedup();
        d
    }

    /// The frame at timeline time `t`; panics where `try_render` errs.
    pub fn render(&mut self, compiled: &Compiled, t: f64) -> RgbaImage {
        self.try_render(compiled, t).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The frame at timeline time `t`, with motion blur when the scene asks.
    /// Errs when the backend is `Gpu` and the GPU is missing or fails.
    pub fn try_render(&mut self, compiled: &Compiled, t: f64) -> Result<RgbaImage, String> {
        let states = self.sample_states(compiled, t);
        self.try_render_states(&states)
    }

    /// The scene at the moments frame `t` samples: one, or with motion
    /// blur several across the shutter.
    fn sample_states(&mut self, compiled: &Compiled, t: f64) -> Vec<FrameState> {
        let started = Instant::now();
        let mut states = Vec::new();
        // The fast mode traces one moment (Ray Reconstruction takes no
        // blurred picture) and blurs its picture along each texel's motion
        // over the shutter instead.
        let fast = self.options.path.is_some_and(|o| o.fast);
        self.shutter = compiled.motion_blur_at(t).filter(|_| self.options.motion_blur && fast).map_or(0.0, |mb| (mb.shutter.min(720.0) / 360.0) as f32);
        match compiled.motion_blur_at(t).filter(|_| self.options.motion_blur && !fast) {
            Some(mb) => {
                let n = mb.samples.clamp(2, 64);
                let open = mb.shutter.min(720.0) / 360.0 * compiled.fps().frame();
                let at = |i: u32, of: u32| t + ((i as f64 + 0.5) / of as f64 - 0.5) * open;
                for i in 0..n {
                    states.push(compiled.frame(at(i, n)));
                }
                if states.windows(2).all(|w| w[0].layers == w[1].layers && w[0].scene == w[1].scene) {
                    // A still picture needs one sample.
                    states.truncate(1);
                } else {
                    // A sample for every `MOTION_PX_PER_SAMPLE` output
                    // pixels anything moves over the shutter, up to the
                    // scene's: a slow move is as smooth with two as with
                    // eight, at a quarter of the frame's cost.
                    let moved = states.windows(2).map(|w| motion_px(&w[0], &w[1])).sum::<Option<f64>>();
                    let need = moved.map_or(n, |d| ((d * self.options.scale / MOTION_PX_PER_SAMPLE).ceil() as u32).clamp(2, n));
                    if need < n {
                        states = (0..need).map(|i| compiled.frame(at(i, need))).collect();
                    }
                }
            }
            None => states.push(compiled.frame(t)),
        }
        if let Some(p) = self.profile.as_mut() {
            p.stage("evaluate scene", started);
        }
        states
    }

    /// `try_render_states`; panics where it errs.
    pub fn render_states(&mut self, states: &[FrameState]) -> RgbaImage {
        self.try_render_states(states).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Renders equally weighted sub-samples of one output frame.
    pub fn try_render_states(&mut self, states: &[FrameState]) -> Result<RgbaImage, String> {
        let (lists, key) = self.prepare(states)?;
        if let Some((last_key, image)) = &self.last {
            if *last_key == key {
                self.stats.cached += 1;
                return Ok(image.clone());
            }
        }
        let built = Instant::now();
        let main = lists[0].main();
        #[cfg(test)]
        let fail = self.take_failure();
        #[cfg(not(test))]
        let fail = false;
        let result = self.gpu.as_mut().map(|g| if fail { Err("ERROR_DEVICE_LOST (test)".to_string()) } else { g.render(&lists, &self.fonts, &self.images, &self.footage.dynamic) });
        self.take_gpu_profile(built);
        let bytes = self.settle(result, lists, states, built)?;
        self.stats.raster_ms += built.elapsed().as_secs_f64() * 1000.0;
        let image = RgbaImage::from_raw(main.width, main.height, bytes).expect("frame size");
        self.last = Some((key, image.clone()));
        Ok(image)
    }

    /// Adds the GPU's stages and passes since `since` to the profile.
    fn take_gpu_profile(&mut self, since: Instant) {
        if let (Some(p), Some(g)) = (self.profile.as_mut(), self.gpu.as_mut()) {
            p.add("gpu frame", since.elapsed());
            if let Some(gp) = g.profile.as_mut() {
                p.merge(std::mem::take(gp));
            }
        }
    }

    /// The display lists of a frame's samples and the key of its picture.
    fn prepare(&mut self, states: &[FrameState]) -> Result<(Vec<DisplayList>, Vec<u8>), String> {
        if self.gpu.is_none() && self.options.backend == Backend::Gpu {
            return Err(format!("GPU unavailable: {}", self.gpu_error.as_deref().unwrap_or("no device")));
        }
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
        let trace = trace.map(|o| pt::TraceOptions { spp: o.spp.div_ceil(n).max(1), temporal: o.temporal && n == 1, shutter: if o.fast { self.shutter } else { 0.0 }, ..o });
        self.images.begin_frame();
        let mut lists = self.build_lists(states, trace);
        if self.images.overflowed {
            // The image pool is full: keep only this frame's images and build again.
            self.images.compact();
            self.images.begin_frame();
            lists = self.build_lists(states, trace);
            // This frame's images alone do not fit: an error, not a frame
            // with images missing (and decoded again every frame).
            if self.images.overflowed {
                return Err(self.images.overflow_report());
            }
        }
        let keying = Instant::now();
        let mut key = Vec::new();
        for l in &lists {
            key.extend(l.signature());
        }
        // Footage pixels are part of the picture.
        key.extend(self.footage.signature());
        let built = Instant::now();
        if let Some(p) = self.profile.as_mut() {
            p.add("display lists › frame key", built - keying);
        }
        self.stats.build_ms += (built - started).as_secs_f64() * 1000.0;
        self.stats.frames += 1;
        if let Some(p) = self.profile.as_mut() {
            p.frames += 1;
            p.add("display lists", built - started);
            p.add("display lists › meshes (generate, displace)", std::mem::take(&mut self.meshes.busy));
            p.add("display lists › meshes (place on targets)", std::mem::take(&mut self.meshes.placing));
            p.add("display lists › meshes (place on targets) › triangle setup", std::mem::take(&mut self.meshes.placing_setup));
            p.add("display lists › meshes (occluders)", std::mem::take(&mut self.meshes.occluding));
            p.add("display lists › meshes (shadow maps)", std::mem::take(&mut self.meshes.shadowing));
            p.add("display lists › images (load)", std::mem::take(&mut self.images.busy));
        }
        Ok((lists, key))
    }

    /// A frame's pixels from the GPU's answer (None: there is no GPU):
    /// a failed frame is tried again on a new device, then is an error or
    /// (the "auto" backend) drawn on the CPU; without a GPU the CPU draws.
    fn settle(&mut self, result: Option<Result<Vec<u8>, String>>, lists: Vec<DisplayList>, states: &[FrameState], built: Instant) -> Result<Vec<u8>, String> {
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
                let retried = gpu_with_retry(self.options.allow_software_gpu, self.options.profile).ok().and_then(|mut g| {
                    let r = if fail_again { None } else { g.render(&lists, &self.fonts, &self.images, &self.footage.dynamic).ok() };
                    // (Frames queued on the old device are drawn again.)
                    self.gpu = Some(g);
                    self.device += 1;
                    r
                });
                match retried {
                    Some(bytes) => {
                        self.fallbacks.push(format!("GPU failed at frame {} ({e}); a new device rendered it", self.stats.frames));
                        bytes
                    }
                    None => {
                        // GPU only: this frame fails, the (new) device stays
                        // for the next ones.
                        if self.options.backend == Backend::Gpu {
                            return Err(format!("GPU failed at frame {}: {e}", self.stats.frames));
                        }
                        self.gpu = None;
                        self.device += 1;
                        self.gpu_error = Some(format!("GPU failed: {e}"));
                        self.fallbacks.push(format!("GPU failed at frame {} ({e}); this and later frames rendered on the CPU", self.stats.frames));
                        // Lists in the CPU's form (the GPU's shared triangle
                        // records are not); the CPU cannot trace, so the
                        // frame is rasterised.
                        let raster = self.build_lists(states, None);
                        self.cpu(&raster)
                    }
                }
            }
            None => {
                let bytes = self.cpu(&lists);
                if let Some(p) = self.profile.as_mut() {
                    p.add("cpu raster", built.elapsed());
                }
                bytes
            }
        };
        Ok(bytes)
    }

    /// Begins the frame at `t` and returns the oldest frame begun before
    /// it once enough are under way (None until then): frames come out of
    /// `push` and then `pop` in the order they went in. Rendering a
    /// sequence this way keeps the GPU busy: while it draws one frame the
    /// next is evaluated, built and recorded, and a traced frame at Open
    /// Image Denoise does not hold up the one being traced. (`render`
    /// draws one frame at a time; do not mix the two.)
    pub fn push(&mut self, compiled: &Compiled, t: f64) -> Result<Option<RgbaImage>, String> {
        let states = self.sample_states(compiled, t);
        let (lists, key) = self.prepare(&states)?;
        let main = lists[0].main();
        let began = Instant::now();
        let same = match self.queue.back() {
            Some(q) => q.key == key,
            None => self.last.as_ref().is_some_and(|(k, _)| *k == key),
        };
        #[cfg(test)]
        let fail = self.fail_gpu > 0;
        #[cfg(not(test))]
        let fail = false;
        let work = match self.gpu.as_mut() {
            _ if same => Work::Repeat,
            Some(g) if !fail => match g.begin(&lists, &self.fonts, &self.images, &self.footage.dynamic) {
                Ok(id) => Work::Gpu { id, device: self.device },
                // Tried again, and reported, when it is taken.
                Err(_) => Work::Later,
            },
            _ => Work::Later,
        };
        // While the GPU traces this one: the frame two before it comes
        // back from Open Image Denoise and has its picture composited,
        // and the one before it goes there (not waited for: the next
        // frame is built and recorded meanwhile).
        let before = |k: usize| match self.queue.len().checked_sub(k).and_then(|i| self.queue.get(i)) {
            Some(Queued { work: Work::Gpu { id, device }, .. }) if *device == self.device => Some(*id),
            _ => None,
        };
        let (denoised, traced) = (before(2), before(1));
        if let Some(g) = self.gpu.as_mut() {
            // (A failure is met again, and handled, when it is taken.)
            if let Some(id) = denoised {
                let _ = g.advance(id);
            }
            if let Some(id) = traced {
                let _ = g.start_denoise(id);
            }
        }
        self.queue.push_back(Queued { key, states, size: (main.width, main.height), work });
        self.take_gpu_profile(began);
        self.stats.raster_ms += began.elapsed().as_secs_f64() * 1000.0;
        let depth = if self.gpu.as_mut().is_some_and(|g| g.denoises_between()) { gpu::FLIGHTS } else { 2 };
        if self.queue.len() >= depth {
            return self.pop();
        }
        Ok(None)
    }

    /// The oldest frame begun with `push`, or None when there is none
    /// left (call until then after the last `push`).
    pub fn pop(&mut self) -> Result<Option<RgbaImage>, String> {
        let Some(q) = self.queue.pop_front() else { return Ok(None) };
        let taking = Instant::now();
        let image = match q.work {
            Work::Repeat => {
                self.stats.cached += 1;
                self.last.as_ref().map(|(_, image)| image.clone()).ok_or("a repeated frame with none before it")?
            }
            Work::Gpu { id, device } if device == self.device && self.gpu.is_some() => {
                let result = self.gpu.as_mut().map(|g| g.finish(id));
                self.take_gpu_profile(taking);
                match result {
                    Some(Ok(bytes)) => RgbaImage::from_raw(q.size.0, q.size.1, bytes).expect("frame size"),
                    // As a frame drawn on its own would be: again on this
                    // device, then on a new one.
                    _ => {
                        self.last = None;
                        self.try_render_states(&q.states)?
                    }
                }
            }
            Work::Gpu { .. } | Work::Later => {
                self.last = None;
                self.try_render_states(&q.states)?
            }
        };
        self.stats.raster_ms += taking.elapsed().as_secs_f64() * 1000.0;
        self.last = Some((q.key, image.clone()));
        Ok(Some(image))
    }

    /// With profiling: collects what the GPU has timed and not yet handed
    /// over (call before reading `profile` after the last frame).
    pub fn flush_profile(&mut self) {
        if let (Some(p), Some(g)) = (self.profile.as_mut(), self.gpu.as_mut()) {
            g.flush_profile();
            if let Some(gp) = g.profile.as_mut() {
                p.merge(std::mem::take(gp));
            }
        }
    }

    fn build_lists(&mut self, states: &[FrameState], trace: Option<pt::TraceOptions>) -> Vec<DisplayList> {
        let scale = self.options.scale;
        let compact = self.gpu.as_ref().is_some_and(|g| g.has_vis());
        states
            .iter()
            .map(|s| {
                // Seeded by the frame's time: the same frame traces the same.
                let seed = ((s.t * 1000.0).round() as i64 as u64).wrapping_mul(0x9E37_79B9) as u32;
                Builder { fonts: &mut self.fonts, images: &mut self.images, footage: &mut self.footage, meshes: &mut self.meshes, scale, trace, seed, compact }.build(s)
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
                if let Some(i) = g.shader {
                    shader::paint_cpu(&list.shaders[i as usize], g, groups, self.threads);
                    continue;
                }
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

/// Output pixels something may move between two motion-blur samples: the
/// samples of a frame are as many as its motion needs at this spacing (a
/// streak's steps are then under two pixels), at most the scene's.
const MOTION_PX_PER_SAMPLE: f64 = 1.5;

/// How far, in canvas pixels, anything on screen moves between two
/// moments, or None when that cannot be told from where the layers are:
/// layers come or go, or a shape, text or shader picture changes form.
/// (Motion blur then takes every sample the scene asks for.) Colours,
/// opacities and effects changing are not motion.
fn motion_px(a: &FrameState, b: &FrameState) -> Option<f64> {
    use lahza_motion_core::space::Proj;
    fn far(p: (f64, f64), q: (f64, f64)) -> Option<f64> {
        let d = (p.0 - q.0).hypot(p.1 - q.1);
        d.is_finite().then_some(d)
    }
    fn layers(a: &[LayerState], b: &[LayerState], ra: Proj, rb: Proj, canvas: (f64, f64)) -> Option<f64> {
        if a.len() != b.len() {
            return None;
        }
        let mut most = 0.0f64;
        for (x, y) in a.iter().zip(b) {
            if x.id != y.id {
                return None;
            }
            let (ma, mb) = (ra.then(x.matrix), rb.then(y.matrix));
            let d = match (&x.content, &y.content) {
                (Content::Group(ca), Content::Group(cb)) => layers(ca, cb, ma, mb, canvas)?,
                (Content::Mesh(p), Content::Mesh(q)) => {
                    // Its box's corners, where the camera puts them.
                    if p.params != q.params {
                        return None;
                    }
                    let (lo, hi) = p.params.local_bounds()?;
                    let (pa, pb) = (p.camera.project.then(p.world), q.camera.project.then(q.world));
                    let mut most = 0.0f64;
                    for k in 0..8 {
                        let c = [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }];
                        let (s, e) = (pa.apply(c[0], c[1], c[2]), pb.apply(c[0], c[1], c[2]));
                        if s[3] <= 1e-6 || e[3] <= 1e-6 {
                            return None;
                        }
                        most = most.max(far(ra.apply(s[0] / s[3], s[1] / s[3]), rb.apply(e[0] / e[3], e[1] / e[3]))?);
                    }
                    most
                }
                // Footage steps from frame to frame where it stands.
                (Content::Video(_), Content::Video(_)) => flat(ma, mb, canvas)?,
                (p, q) if p == q => flat(ma, mb, canvas)?,
                _ => return None,
            };
            most = most.max(d);
            match (x.matte.as_deref().and_then(|m| m.source.as_ref()), y.matte.as_deref().and_then(|m| m.source.as_ref())) {
                (Some(p), Some(q)) => most = most.max(layers(std::slice::from_ref(p), std::slice::from_ref(q), ra, rb, canvas)?),
                (None, None) => {}
                _ => return None,
            }
        }
        Some(most)
    }
    /// A flat layer under `ma` then `mb`: how far the points it shows on
    /// the canvas move (most at a corner of the canvas), either way.
    fn flat(ma: Proj, mb: Proj, canvas: (f64, f64)) -> Option<f64> {
        if ma == mb {
            return Some(0.0);
        }
        let (ia, ib) = (ma.inverse()?, mb.inverse()?);
        let mut most = 0.0f64;
        for q in [(0.0, 0.0), (canvas.0, 0.0), (canvas.0, canvas.1), (0.0, canvas.1)] {
            let (pa, pb) = (ia.apply(q.0, q.1), ib.apply(q.0, q.1));
            most = most.max(far(mb.apply(pa.0, pa.1), q)?).max(far(ma.apply(pb.0, pb.1), q)?);
        }
        Some(most)
    }
    if a.scene != b.scene || (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    layers(&a.layers, &b.layers, Proj::IDENTITY, Proj::IDENTITY, (a.width as f64, a.height as f64))
}

/// Makes the GPU renderer, trying again on transient failures (a device
/// busy or lost under contention); a missing or software-only device is not
/// retried.
fn gpu_with_retry(allow_software: bool, profile: bool) -> Result<gpu::Gpu, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        match gpu::Gpu::new(allow_software, profile) {
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
