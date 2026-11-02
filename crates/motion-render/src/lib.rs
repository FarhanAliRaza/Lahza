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

pub mod export;
pub mod font;
pub mod footage;
pub mod gpu;
pub mod images;
pub mod list;
pub mod mesh3d;
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
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            motion_blur: true,
            backend: Backend::Auto,
            allow_software_gpu: std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some(),
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
    options: RenderOptions,
    last: Option<(Vec<u8>, RgbaImage)>,
    pub stats: Stats,
    threads: usize,
}

impl Renderer {
    /// `root` is the project folder (images and fonts resolve against it).
    pub fn new(root: Option<&Path>, options: RenderOptions) -> Self {
        let (gpu, gpu_error) = match options.backend {
            Backend::Cpu => (None, None),
            _ => match gpu::Gpu::new(options.allow_software_gpu) {
                Ok(g) => (Some(g), None),
                Err(e) => (None, Some(e)),
            },
        };
        Self {
            fonts: Fonts::new(root),
            images: Images::new(root),
            footage: footage::Footage::new(root),
            meshes: mesh3d::MeshCache::default(),
            gpu,
            gpu_error,
            options,
            last: None,
            stats: Stats::default(),
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        }
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

    /// "GPU · <device>" or "CPU (<why>)".
    pub fn backend_label(&self) -> String {
        match (&self.gpu, &self.gpu_error) {
            (Some(g), _) => g.label.clone(),
            (None, Some(e)) => format!("CPU ({e})"),
            (None, None) => "CPU".into(),
        }
    }

    pub fn is_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    pub fn warnings(&self) -> Vec<String> {
        let mut w = self.fonts.warnings.clone();
        w.extend(self.images.warnings.iter().cloned());
        w.extend(self.footage.warnings.iter().cloned());
        w.extend(self.meshes.warnings.iter().cloned());
        w.sort();
        w.dedup();
        w
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
        let scale = self.options.scale;
        self.footage.begin_frame();
        let lists: Vec<DisplayList> = states
            .iter()
            .map(|s| Builder { fonts: &mut self.fonts, images: &mut self.images, footage: &mut self.footage, meshes: &mut self.meshes, scale }.build(s))
            .collect();
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
        let bytes = match self.gpu.as_mut().map(|g| g.render(&lists, &self.fonts, &self.images, &self.footage.dynamic)) {
            Some(Ok(bytes)) => bytes,
            Some(Err(e)) => {
                // Sticky fallback: the same lists render on the CPU.
                self.gpu = None;
                self.gpu_error = Some(format!("GPU failed: {e}"));
                self.cpu(&lists)
            }
            None => self.cpu(&lists),
        };
        self.stats.raster_ms += built.elapsed().as_secs_f64() * 1000.0;
        let image = RgbaImage::from_raw(main.width, main.height, bytes).expect("frame size");
        self.last = Some((key, image.clone()));
        image
    }

    fn cpu(&mut self, lists: &[DisplayList]) -> Vec<u8> {
        let main = lists[0].main();
        let (w, h) = (main.width as usize, main.height as usize);
        let weight = 1.0 / lists.len() as f32;
        let mut accum = vec![[0.0f32; 4]; w * h];
        for list in lists {
            let mut groups = vec![[0.0f32; 4]; list.group_texels as usize];
            for g in &list.groups {
                let n = (g.target.width * g.target.height) as usize;
                // Outer groups sample inner ones, so draw into a scratch
                // buffer and copy it in place.
                let mut target = vec![[0.0f32; 4]; n];
                let src = raster::Sources { atlas: &self.fonts.atlas, images: &self.images.texels, groups: &groups, footage: &self.footage.dynamic, ops: &list.ops };
                raster::raster(list, &g.target, &src, &mut target, raster::Write::Replace, self.threads);
                groups[g.offset as usize..g.offset as usize + n].copy_from_slice(&target);
                raster::blur(&mut groups, g.offset as usize, g.target.width as usize, g.target.height as usize, g.sigma, g.clamp, self.threads);
            }
            let src = raster::Sources { atlas: &self.fonts.atlas, images: &self.images.texels, groups: &groups, footage: &self.footage.dynamic, ops: &list.ops };
            raster::raster(list, &list.main(), &src, &mut accum, raster::Write::Accumulate(weight), self.threads);
        }
        let mut out = vec![0u8; w * h * 4];
        raster::resolve(&accum, &mut out);
        out
    }

    /// Canvas bounds of a text layer (unanimated layout), for `check`.
    pub fn measure(&mut self, layer: &LayerState) -> Option<[f64; 4]> {
        let Content::Text(text) = &layer.content else { return None };
        let layout = self.fonts.layout(text);
        let [x0, y0, x1, y1] = layout.bounds;
        let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| layer.matrix.apply(x, y));
        Some([
            pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min),
            pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
            pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max),
            pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
        ])
    }
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
