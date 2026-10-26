//! Offscreen Vulkan composition. CPU code prepares geometry and cached assets;
//! compute shaders resample and blend the moving layers. No window is required.
use super::super::screen_upscale::{EnhancedRegion, ScreenUpscaler, UpscalePlan};
use super::*;
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use gpu::ShaderData as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

const MAX_BUFFER: usize = 128 * 1024 * 1024;
static GPU_EXPORT_IN_USE: AtomicBool = AtomicBool::new(false);

/// Per-frame enhancement demand, in physical output pixels. Motion and
/// shutter trails attenuate learned detail before temporal accumulation.
#[derive(Clone, Copy, Debug)]
pub struct RenderQuality {
    pub enhance: bool,
    pub motion_pixels: f32,
    pub trail_pixels: f32,
}
impl Default for RenderQuality {
    fn default() -> Self {
        Self {
            enhance: true,
            motion_pixels: 0.0,
            trail_pixels: 0.0,
        }
    }
}
impl RenderQuality {
    pub fn strength(self) -> f32 {
        if !self.enhance || !self.motion_pixels.is_finite() || !self.trail_pixels.is_finite() {
            return 0.0;
        }
        let motion = ((28.0 - self.motion_pixels.max(0.0)) / 24.0).clamp(0.0, 1.0);
        let trail = ((8.0 - self.trail_pixels.max(0.0)) / 4.0).clamp(0.0, 1.0);
        (motion.min(trail) * 16.0).round() / 16.0
    }
}

/// Vulkan loader/driver initialization can race another context's resource
/// calls. One export owns GPU access through teardown; competing exports use
/// the existing CPU renderer immediately, keeping cancellation responsive.
struct GpuExportLease<'a>(&'a AtomicBool);

impl<'a> GpuExportLease<'a> {
    fn try_acquire(in_use: &'a AtomicBool) -> Option<Self> {
        in_use
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self(in_use))
    }
}

impl Drop for GpuExportLease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Draw {
    kind: [f32; 4],
    rect: [f32; 4],
    anchor: [f32; 4],
    smear: [f32; 4],
    color: [f32; 4],
    bounds: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    canvas: [u32; 4],
    images: [[u32; 4]; 8],
    inverse: [[f32; 4]; 3],
    viewport: [f32; 4],
    loupe_shape: [f32; 4],
    loupe_optics: [f32; 4],
    enhanced_rect: [f32; 4],
    enhanced_info: [f32; 4],
    sample_bounds: [f32; 4],
    source_mips: [[u32; 4]; sampling::LEVELS],
    geometry: [f32; 4],
    projection: [f32; 4],
    media_bounds: [u32; 4],
    camera_rect: [f32; 4],
    camera_info: [f32; 4],
    camera_crop: [f32; 4],
    draws: [Draw; 4],
}

#[derive(blade_macros::ShaderData)]
struct ComposeData {
    b_params: gpu::BufferPiece,
    b_background: gpu::BufferPiece,
    b_source: gpu::BufferPiece,
    b_source_mips: gpu::BufferPiece,
    b_enhanced: gpu::BufferPiece,
    b_overlay: gpu::BufferPiece,
    b_camera: gpu::BufferPiece,
    b_cursor: gpu::BufferPiece,
    b_shadow: gpu::BufferPiece,
    b_caption: gpu::BufferPiece,
    b_watermark: gpu::BufferPiece,
    b_output: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ResizeData {
    b_params: gpu::BufferPiece,
    b_resize_source: gpu::BufferPiece,
    b_resize_temp: gpu::BufferPiece,
    b_resize_output: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct MipData {
    b_mip_params: gpu::BufferPiece,
    b_mip_source: gpu::BufferPiece,
    b_mip_output: gpu::BufferPiece,
}

/// GPU failure is sticky for this export. The same CPU geometry and inputs
/// render the failed frame, so a fallback never skips or duplicates a frame.
pub struct Renderer {
    matting: super::super::camera_matting::CameraMatting,
    camera_error: Option<String>,
    gpu: Option<Compositor>,
    label: String,
}

/// Preview workers and exports reuse one hardware context. A preview cannot
/// hold the export lease idle and force every later export onto the CPU.
pub struct SharedRenderer {
    backend: Arc<Mutex<Renderer>>,
    camera_error: Option<String>,
    label: String,
}

struct SharedRendererState {
    backend: Weak<Mutex<Renderer>>,
    // While GPUI is running, the app owns final GPU teardown. A weak cache
    // alone can miss an export whose last Arc is already being destroyed.
    application_owner: Option<Arc<Mutex<Renderer>>>,
    application_running: bool,
    shutting_down: bool,
}

static SHARED_RENDERER: Mutex<SharedRendererState> = Mutex::new(SharedRendererState {
    backend: Weak::new(),
    application_owner: None,
    application_running: false,
    shutting_down: false,
});

impl SharedRenderer {
    pub fn take_camera_error(&mut self) -> Option<String> {
        self.camera_error.take()
    }
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn compose(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
    ) -> RgbaImage {
        let mut backend = self
            .backend
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let image = backend.compose(scene, input, visible, caption);
        self.camera_error = backend.take_camera_error();
        self.label = backend.label().to_owned();
        image
    }

    pub fn compose_with_quality(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
        quality: RenderQuality,
    ) -> RgbaImage {
        let mut backend = self
            .backend
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let image = backend.compose_inner(scene, input, visible, caption, quality);
        self.camera_error = backend.take_camera_error();
        self.label = backend.label().to_owned();
        image
    }

    pub fn compose_fast(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
    ) -> RgbaImage {
        let mut backend = self
            .backend
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let image = backend.compose_inner(
            scene,
            input,
            visible,
            caption,
            RenderQuality {
                enhance: false,
                ..Default::default()
            },
        );
        self.camera_error = backend.take_camera_error();
        self.label = backend.label().to_owned();
        image
    }
}

impl Renderer {
    pub(crate) fn begin_application() {
        let mut shared = SHARED_RENDERER
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // A new GPUI application (including another UI test) gets a fresh
        // backend; handles surviving the previous app remain on the CPU.
        if shared.shutting_down {
            shared.backend = Weak::new();
        }
        shared.application_running = true;
        shared.shutting_down = false;
        shared.application_owner = shared.backend.upgrade();
    }

    pub(crate) fn shutdown_shared() {
        let backend = {
            let mut shared = SHARED_RENDERER
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            shared.shutting_down = true;
            shared.application_running = false;
            shared
                .application_owner
                .take()
                .or_else(|| shared.backend.upgrade())
        };
        if let Some(backend) = backend {
            // Finish the current frame, then destroy the GPU synchronously.
            // Any export still holding this renderer continues using the CPU.
            let mut renderer = backend.lock().unwrap_or_else(|error| error.into_inner());
            drop(renderer.gpu.take());
            renderer.label = "CPU (application shutdown)".into();
        }
    }

    pub fn shared() -> SharedRenderer {
        let mut shared = SHARED_RENDERER
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if shared.shutting_down {
            let label = "CPU (application shutdown)".to_owned();
            return SharedRenderer {
                backend: Arc::new(Mutex::new(Self {
                    gpu: None,
                    matting: Default::default(),
                    camera_error: None,
                    label: label.clone(),
                })),
                label,
                camera_error: None,
            };
        }
        let backend = shared.backend.upgrade().unwrap_or_else(|| {
            let backend = Arc::new(Mutex::new(Self::new()));
            shared.backend = Arc::downgrade(&backend);
            backend
        });
        if shared.application_running {
            shared.application_owner = Some(backend.clone());
        }
        let label = backend
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .label()
            .to_owned();
        SharedRenderer {
            backend,
            label,
            camera_error: None,
        }
    }

    pub fn new() -> Self {
        if std::env::var("LAHZA_COMPOSITOR").as_deref() == Ok("cpu") {
            return Self {
                gpu: None,
                matting: Default::default(),
                camera_error: None,
                label: "CPU".into(),
            };
        }
        match catch_unwind(AssertUnwindSafe(Compositor::new)) {
            Ok(Ok(gpu)) => {
                let label = format!("Vulkan · {}", gpu.context.device_information().device_name);
                eprintln!("Lahza composition: {label}");
                Self {
                    gpu: Some(gpu),
                    matting: Default::default(),
                    camera_error: None,
                    label,
                }
            }
            result => {
                let reason = match result {
                    Ok(Err(error)) => error,
                    _ => "GPU initialization failed".into(),
                };
                eprintln!("Lahza composition: CPU ({reason})");
                Self {
                    gpu: None,
                    matting: Default::default(),
                    camera_error: None,
                    label: "CPU".into(),
                }
            }
        }
    }

    pub fn take_camera_error(&mut self) -> Option<String> {
        self.camera_error.take()
    }
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn compose(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
    ) -> RgbaImage {
        self.compose_inner(scene, input, visible, caption, RenderQuality::default())
    }

    fn compose_inner(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
        quality: RenderQuality,
    ) -> RgbaImage {
        self.camera_error = None;
        if self.label.starts_with("Camera cutout failed:") {
            self.label = self.gpu.as_ref().map_or_else(
                || "CPU".into(),
                |gpu| format!("Vulkan · {}", gpu.context.device_information().device_name),
            );
        }
        let input = FrameInput {
            viewport: scene.camera_viewport(input.viewport),
            ..input
        };
        let camera = input
            .camera
            .filter(|_| scene.style.camera.enabled && scene.style.camera.cutout_strength() > 0.0)
            .map(|camera| self.matting.process(camera, scene.style.camera));
        let camera = match camera {
            Some(Ok(camera)) => {
                if self.label.starts_with("Camera cutout failed:") {
                    self.label = self.gpu.as_ref().map_or_else(
                        || "CPU".into(),
                        |gpu| format!("Vulkan · {}", gpu.context.device_information().device_name),
                    );
                }
                Some(camera)
            }
            Some(Err(error)) => {
                self.camera_error = Some(error.clone());
                let message = format!("Camera cutout failed: {error}");
                if self.label != message {
                    eprintln!("Lahza {message}");
                }
                self.label = message;
                None
            }
            None => None,
        };
        let input = FrameInput {
            camera: camera.as_deref().or(input.camera),
            ..input
        };
        if let Some(gpu) = &mut self.gpu {
            match catch_unwind(AssertUnwindSafe(|| {
                gpu.compose_inner(scene, input, visible, caption, quality)
            })) {
                Ok(Ok(output)) => return output,
                failure => {
                    let reason = match &failure {
                        Ok(Err(error)) => error.as_str(),
                        _ => "GPU operation failed",
                    };
                    eprintln!("Lahza composition: switching to CPU ({reason})");
                    let gpu = self.gpu.take().unwrap();
                    // A timed-out/lost device may still own submitted memory.
                    // Quarantine it until process exit instead of freeing live
                    // resources or blocking export indefinitely in a driver.
                    if gpu.pending.is_some() || failure.is_err() {
                        std::mem::forget(gpu);
                    }
                    self.label = "CPU (GPU fallback)".into();
                }
            }
        }
        scene.compose_layers(input, visible, caption)
    }
}

/// Buffers are persistent and grow only between completed submissions.
struct Buffer {
    device: gpu::Buffer,
    host: gpu::Buffer,
    capacity: usize,
    cached: Vec<u8>,
}

impl Buffer {
    fn new(context: &gpu::Context, size: usize) -> Self {
        let capacity = size.max(16);
        Self {
            device: context.create_buffer(gpu::BufferDesc {
                name: "export device",
                size: capacity as u64,
                memory: gpu::Memory::Device,
            }),
            host: context.create_buffer(gpu::BufferDesc {
                name: "export staging",
                size: capacity as u64,
                // CPU-facing staging must prefer system RAM. Shared prefers
                // device-local memory and can map VRAM over slow PCIe reads.
                // Upload buffers support transfer in both directions; we use
                // streaming loads below when their host mapping is uncached.
                memory: gpu::Memory::Upload,
            }),
            capacity,
            cached: Vec::new(),
        }
    }
    fn ensure(&mut self, context: &gpu::Context, size: usize) -> Result<(), String> {
        if size > MAX_BUFFER {
            return Err("frame exceeds the GPU buffer limit".into());
        }
        if size > self.capacity {
            let replacement = Self::new(context, size);
            self.destroy(context);
            *self = replacement;
        }
        Ok(())
    }
    fn upload(
        &mut self,
        context: &gpu::Context,
        encoder: &mut gpu::CommandEncoder,
        bytes: &[u8],
        cache: bool,
    ) -> Result<bool, String> {
        self.ensure(context, bytes.len())?;
        if cache && self.cached == bytes {
            return Ok(false);
        }
        // SAFETY: mapped host allocation is at least bytes.len(), nonoverlapping,
        // and all previous GPU work was completed before starting this frame.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.host.data(), bytes.len());
        }
        context.sync_buffer(self.host);
        encoder.transfer("upload").copy_buffer_to_buffer(
            self.host.into(),
            self.device.into(),
            bytes.len() as u64,
        );
        if cache {
            self.cached.clear();
            self.cached.extend_from_slice(bytes);
        }
        Ok(true)
    }
    fn destroy(&mut self, context: &gpu::Context) {
        context.destroy_buffer(self.host);
        context.destroy_buffer(self.device);
    }
}

/// Enhance only source pixels that are actually enlarged. The ROI includes
/// source context around the visible crop so the learned filter has no hard
/// seam at a lens edge. Physical output dimensions naturally account for DPI.
fn enhancement_plan(scene: &SceneCompositor, input: FrameInput<'_>) -> Option<(UpscalePlan, f32)> {
    let (sw, sh) = input.source.dimensions();
    if sw == 0 || sh == 0 {
        return None;
    }
    let projection = scene.projection(input.viewport);
    let (left, top, width, height) = scene.style.source_crop.visible_rect(input.viewport);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let ratio_at = |u: f64, v: f64| {
        let p = projection.project(u, v);
        let dx = projection.project(u + 0.001, v);
        let dy = projection.project(u, v + 0.001);
        ((dx.0 - p.0).hypot(dx.1 - p.1) / (0.001 * width * sw as f64))
            .max((dy.0 - p.0).hypot(dy.1 - p.1) / (0.001 * height * sh as f64))
    };
    let base_ratio = [(0.5, 0.5), (0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .map(|(u, v)| ratio_at(u, v))
        .fold(0.0_f64, f64::max);
    let lens = scene.loupe_geometry(input.viewport);
    let lens_ratio = lens.map_or(0.0, |lens| {
        let (u, v) = projection.unproject(lens.center[0] as f64, lens.center[1] as f64);
        ratio_at(u, v) * lens.magnification as f64
    });
    let ratio = base_ratio.max(lens_ratio);
    if !ratio.is_finite() || ratio <= 1.02 {
        return None;
    }
    let mut region = [1.0_f64, 1.0, 0.0, 0.0];
    let mut include = |x: f64, y: f64| {
        let (u, v) = projection.unproject(x, y);
        if u.is_finite() && v.is_finite() {
            let x = left + u.clamp(0.0, 1.0) * width;
            let y = top + v.clamp(0.0, 1.0) * height;
            region[0] = region[0].min(x);
            region[1] = region[1].min(y);
            region[2] = region[2].max(x);
            region[3] = region[3].max(y);
        }
    };
    if base_ratio > 1.02 {
        for x in [0.0, scene.width as f64 * 0.5, scene.width as f64] {
            for y in [0.0, scene.height as f64 * 0.5, scene.height as f64] {
                include(x, y);
            }
        }
    }
    if let Some(lens) = lens.filter(|_| lens_ratio > 1.02) {
        let reach = lens.radius as f64 * (1.0 / lens.magnification as f64 + lens.bevel as f64);
        for dx in [-reach, 0.0, reach] {
            for dy in [-reach, 0.0, reach] {
                include(lens.center[0] as f64 + dx, lens.center[1] as f64 + dy);
            }
        }
    }
    let x = ((region[0] * sw as f64).floor() as u32)
        .saturating_sub(12)
        .min(sw - 1);
    let y = ((region[1] * sh as f64).floor() as u32)
        .saturating_sub(12)
        .min(sh - 1);
    let right = ((region[2] * sw as f64).ceil() as u32)
        .saturating_add(12)
        .min(sw);
    let bottom = ((region[3] * sh as f64).ceil() as u32)
        .saturating_add(12)
        .min(sh);
    let mut plan = UpscalePlan {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
        scale: if ratio >= 2.5 { 4 } else { 2 },
    };
    if plan.validate((sw, sh)).is_err() {
        plan.scale = 2;
    }
    plan.validate((sw, sh)).ok()?;
    let strength = (((ratio - 1.02) / 0.18).clamp(0.0, 1.0) as f32 * 16.0).round() / 16.0;
    (strength >= 0.2).then_some((plan, strength))
}

struct Compositor {
    context: gpu::Context,
    encoder: gpu::CommandEncoder,
    pipeline: gpu::ComputePipeline,
    mip_pipeline: gpu::ComputePipeline,
    mip_buffer: Buffer,
    mip_params: Buffer,
    resize_vertical: gpu::ComputePipeline,
    resize_horizontal: gpu::ComputePipeline,
    images: [Buffer; 8],
    params: Buffer,
    output: Buffer,
    caption_temp: Buffer,
    caption_scaled: Buffer,
    caption_size: (u32, u32, u32, u32),
    shadow_key: Option<(u32, u32, Rect, f64)>,
    pending: Option<gpu::SyncPoint>,
    upscaler: Option<ScreenUpscaler>,
    enhancement_failed: bool,
    enhanced_region: Option<(EnhancedRegion, (u32, u32))>,
    // Last field: Rust drops the context and resources before releasing the
    // lease. Quarantining a lost device also retains its lease until exit.
    _lease: GpuExportLease<'static>,
}

/// Prefer a discrete GPU and reject software Vulkan implementations. Try the
/// other hardware adapters if the preferred driver's features are insufficient.
fn hardware_device_ids() -> Result<Vec<u32>, String> {
    // SAFETY: the loader and temporary instance outlive every enumeration call;
    // no logical device or resources are created on this instance.
    unsafe {
        let entry = ash::Entry::load().map_err(|error| error.to_string())?;
        let app = ash::vk::ApplicationInfo::default().api_version(ash::vk::API_VERSION_1_1);
        let instance = entry
            .create_instance(
                &ash::vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .map_err(|error| error.to_string())?;
        let devices = instance.enumerate_physical_devices().map(|devices| {
            let mut devices: Vec<_> = devices
                .into_iter()
                .filter_map(|device| {
                    let p = instance.get_physical_device_properties(device);
                    if p.limits.max_per_stage_descriptor_storage_buffers < 12
                        || p.limits.max_descriptor_set_storage_buffers < 12
                    {
                        return None;
                    }
                    let rank = match p.device_type {
                        ash::vk::PhysicalDeviceType::DISCRETE_GPU => 0,
                        ash::vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
                        ash::vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
                        _ => return None,
                    };
                    Some((rank, p.device_id))
                })
                .collect();
            devices.sort();
            devices.into_iter().map(|(_, id)| id).collect()
        });
        instance.destroy_instance(None);
        devices.map_err(|error| error.to_string())
    }
}

impl Compositor {
    fn new() -> Result<Self, String> {
        let lease = GpuExportLease::try_acquire(&GPU_EXPORT_IN_USE)
            .ok_or("another export is using the GPU")?;
        let mut context = None;
        for device_id in hardware_device_ids()? {
            // SAFETY: headless context, no external handles or shared ownership.
            if let Ok(Ok(candidate)) = catch_unwind(AssertUnwindSafe(|| unsafe {
                gpu::Context::init(gpu::ContextDesc {
                    device_id,
                    ..Default::default()
                })
            })) {
                if !candidate.device_information().is_software_emulated {
                    context = Some(candidate);
                    break;
                }
            }
        }
        let context = context.ok_or("no supported hardware Vulkan device")?;
        let shader = context.try_create_shader(gpu::ShaderDesc {
            source: include_str!("scene_gpu.wgsl"),
        })?;
        shader.check_struct_size::<Params>();
        let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "export composition",
            data_layouts: &[&ComposeData::layout()],
            compute: shader.at("compose"),
        });
        let resize_vertical = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "caption vertical",
            data_layouts: &[&ResizeData::layout()],
            compute: shader.at("resize_vertical"),
        });
        let resize_horizontal = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "caption horizontal",
            data_layouts: &[&ResizeData::layout()],
            compute: shader.at("resize_horizontal"),
        });
        let mip_shader = context.try_create_shader(gpu::ShaderDesc {
            source: include_str!("source_mips.wgsl"),
        })?;
        let mip_pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "alpha-aware source mipmaps",
            data_layouts: &[&MipData::layout()],
            compute: mip_shader.at("downsample"),
        });
        Ok(Self {
            encoder: context.create_command_encoder(gpu::CommandEncoderDesc {
                name: "export",
                buffer_count: 1,
            }),
            images: std::array::from_fn(|_| Buffer::new(&context, 16)),
            params: Buffer::new(&context, std::mem::size_of::<Params>()),
            output: Buffer::new(&context, 16),
            caption_temp: Buffer::new(&context, 16),
            caption_scaled: Buffer::new(&context, 16),
            caption_size: (0, 0, 0, 0),
            mip_pipeline,
            mip_buffer: Buffer::new(&context, 16),
            mip_params: Buffer::new(&context, 256 * sampling::LEVELS),
            shadow_key: None,
            pipeline,
            resize_vertical,
            resize_horizontal,
            context,
            pending: None,
            upscaler: None,
            enhancement_failed: false,
            enhanced_region: None,
            _lease: lease,
        })
    }

    fn compose(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
    ) -> Result<RgbaImage, String> {
        self.compose_inner(
            scene,
            input,
            visible,
            caption,
            RenderQuality {
                enhance: false,
                ..Default::default()
            },
        )
    }

    fn compose_enhanced(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
    ) -> Result<RgbaImage, String> {
        self.compose_inner(scene, input, visible, caption, RenderQuality::default())
    }

    fn compose_inner(
        &mut self,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
        quality: RenderQuality,
    ) -> Result<RgbaImage, String> {
        let input = FrameInput {
            viewport: scene.camera_viewport(input.viewport),
            ..input
        };
        let profile = std::env::var_os("LAHZA_GPU_PROFILE").is_some();
        let started = std::time::Instant::now();
        if scene.width.div_ceil(8) > 65535 || scene.height.div_ceil(8) > 65535 {
            return Err("canvas exceeds the GPU dispatch limit".into());
        }
        let output_bytes = scene.width as usize * scene.height as usize * 4;
        self.output.ensure(&self.context, output_bytes)?;
        self.encoder.start();
        let transform = scene.style.transform.with_motion(input.viewport);
        let base = if visible {
            scene.card_layer(transform, input.source, input.viewport)
        } else {
            scene.background.clone()
        };
        let projection = scene.geometry.projection(transform);
        let mut params = Params::zeroed();
        params.canvas = [scene.width, scene.height, u32::from(visible), 0];
        params.inverse = projection
            .inverse
            .map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0]);
        let (l, t, w, h) = scene.style.source_crop.visible_rect(input.viewport);
        params.viewport = [l as f32, t as f32, w as f32, h as f32];
        if visible {
            if let Some(lens) = scene.loupe_geometry(input.viewport) {
                params.loupe_shape = lens.shape();
                params.loupe_optics = lens.optics();
            }
        }
        let (l, t, r, b) = scene
            .style
            .source_crop
            .sample_bounds(input.source.width(), input.source.height());
        params.sample_bounds = [l as f32, t as f32, r as f32, b as f32];
        let g = scene.geometry;
        params.geometry = [
            g.media.width as f32,
            g.media.height as f32,
            g.radius as f32,
            g.title_height as f32,
        ];
        params.projection = [
            projection.affine_pixel_size.unwrap_or(0.0) as f32,
            scene.style.vignette as f32 / 100.0,
            f32::from(projection.axis_aligned().is_some()),
            0.0,
        ];
        let (x0, y0, x1, y1) = scene.pixel_range(&projection, 2.0);
        params.media_bounds = [x0, y0, x1, y1];
        let commands = if visible {
            input
                .pointer
                .map(|p| scene.pointer_commands(&projection, p, input.viewport))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut cursor = None;
        for (index, command) in commands.iter().enumerate() {
            params.draws[index] = draw_params(command, scene.width, scene.height);
            if let PointerCommand::Bitmap { bitmap, .. } = command {
                cursor = Some(&bitmap.image);
            }
        }
        params.canvas[3] = commands.len() as u32;
        let camera = input
            .camera
            .filter(|image| scene.style.camera.enabled && image.width() > 0 && image.height() > 0);
        if let Some(camera) = camera {
            let overlay = scene.style.camera;
            let rect = overlay.rect_for_frame(
                scene.width as f64,
                scene.height as f64,
                camera.width() as f64 / camera.height() as f64,
                input.viewport.magnification,
            );
            let crop = super::super::camera_layout::source_crop(
                overlay,
                camera.width() as f64 / camera.height() as f64,
                rect,
            );
            params.camera_crop = [
                crop.x as f32,
                crop.y as f32,
                crop.width as f32,
                crop.height as f32,
            ];
            let shadow_strength = overlay.shadow_strength();
            let shadow_enabled = shadow_strength > 0.0 && overlay.opacity() > 0.0;
            let radius = overlay.radius(rect);
            params.camera_rect = [
                rect.x as f32,
                rect.y as f32,
                rect.width as f32,
                rect.height as f32,
            ];
            params.camera_info = [
                radius as f32,
                f32::from(overlay.mirror),
                if shadow_enabled {
                    shadow_strength as f32
                } else {
                    0.0
                },
                overlay.opacity() as f32,
            ];
            if shadow_enabled {
                // A mask depends only on geometry; fades/crop/mirror do not
                // require rerasterizing or reuploading the same shadow.
                let key = (scene.width, scene.height, rect, radius);
                if self.shadow_key != Some(key) {
                    let shadow =
                        CameraShadow::new(scene.width, scene.height, scene.style.camera, rect);
                    let mut values = vec![0.0f32; output_bytes / 4];
                    for (offset, alpha) in shadow.pixels {
                        values[offset / 4] = alpha as f32;
                    }
                    self.images[5].upload(
                        &self.context,
                        &mut self.encoder,
                        bytemuck::cast_slice(&values),
                        false,
                    )?;
                    self.shadow_key = Some(key);
                }
            }
        }
        let layers = [
            Some(&base),
            Some(input.source),
            input.overlay,
            camera,
            cursor,
            None,
            caption,
            scene.watermark.as_ref(),
        ];
        let mut caption_changed = false;
        let mut source_changed = false;
        for (index, image) in layers.into_iter().enumerate() {
            if index == 5 {
                continue;
            }
            if let Some(image) = image.filter(|image| image.width() > 0 && image.height() > 0) {
                params.images[index] = [image.width(), image.height(), 1, 0];
                let changed = self.images[index].upload(
                    &self.context,
                    &mut self.encoder,
                    image.as_raw(),
                    index != 3,
                )?;
                if index == 1 {
                    source_changed = changed;
                }
                if index == 6 {
                    caption_changed = changed;
                }
            }
        }
        let mip_source = if visible {
            input.source.dimensions()
        } else {
            (1, 1)
        };
        let (mip_levels, mip_pixels) = sampling::layout(mip_source.0, mip_source.1);
        params.source_mips = mip_levels;
        self.mip_buffer.ensure(&self.context, mip_pixels * 4)?;
        let mut mip_data = vec![0u8; sampling::LEVELS * 256];
        let mip_crop = [
            (params.sample_bounds[0] * input.source.width() as f32 - 0.5)
                .round()
                .max(0.0) as u32,
            (params.sample_bounds[1] * input.source.height() as f32 - 0.5)
                .round()
                .max(0.0) as u32,
            (params.sample_bounds[2] * input.source.width() as f32 - 0.5)
                .round()
                .max(0.0) as u32,
            (params.sample_bounds[3] * input.source.height() as f32 - 0.5)
                .round()
                .max(0.0) as u32,
        ];
        for level in 1..sampling::LEVELS {
            if mip_levels[level][0] == 0 {
                break;
            }
            let mut previous = mip_levels[level - 1];
            previous[3] = (level > 1) as u32;
            mip_data[level * 256..level * 256 + 16].copy_from_slice(bytemuck::bytes_of(&previous));
            mip_data[level * 256 + 16..level * 256 + 32]
                .copy_from_slice(bytemuck::bytes_of(&mip_levels[level]));
            mip_data[level * 256 + 32..level * 256 + 48]
                .copy_from_slice(bytemuck::bytes_of(&mip_crop));
        }
        let mip_changed =
            self.mip_params
                .upload(&self.context, &mut self.encoder, &mip_data, true)?;
        if source_changed || mip_changed {
            for level in 1..sampling::LEVELS {
                let next = mip_levels[level];
                if next[0] == 0 {
                    break;
                }
                let mut pass = self.encoder.compute("source mip level");
                let mut pipeline = pass.with(&self.mip_pipeline);
                pipeline.bind(
                    0,
                    &MipData {
                        b_mip_params: self.mip_params.device.at((level * 256) as u64),
                        b_mip_source: self.images[1].device.into(),
                        b_mip_output: self.mip_buffer.device.into(),
                    },
                );
                pipeline.dispatch([next[0].div_ceil(8), next[1].div_ceil(8), 1]);
            }
        }
        if source_changed {
            self.enhanced_region = None;
        }
        let mut enhanced_buffer = self.images[1].device.into();
        if quality.strength() >= 0.2 && visible && !self.enhancement_failed {
            if let Some((plan, strength)) = enhancement_plan(scene, input)
                .map(|(plan, strength)| {
                    (
                        plan,
                        (strength.min(quality.strength()) * 16.0).round() / 16.0,
                    )
                })
                .filter(|(_, strength)| *strength >= 0.2)
            {
                if self.upscaler.is_none() {
                    match ScreenUpscaler::new(&self.context) {
                        Ok(upscaler) => self.upscaler = Some(upscaler),
                        Err(error) => {
                            eprintln!("Screen enhancement unavailable: {error}");
                            self.enhancement_failed = true;
                        }
                    }
                }
                if let Some(upscaler) = self.upscaler.as_mut() {
                    let cached = self.enhanced_region.filter(|(region, size)| {
                        region.plan.scale == plan.scale
                            && *size == input.source.dimensions()
                            && region.plan.x <= plan.x
                            && region.plan.y <= plan.y
                            && region.plan.x + region.plan.width >= plan.x + plan.width
                            && region.plan.y + region.plan.height >= plan.y + plan.height
                    });
                    let enhanced = if let Some((region, _)) = cached {
                        Ok(region)
                    } else {
                        // A small extra source margin lets adjacent shutter samples
                        // share the same inference, while preserving their own geometry.
                        let size = input.source.dimensions();
                        let x = plan.x.saturating_sub(32);
                        let y = plan.y.saturating_sub(32);
                        let expanded = UpscalePlan {
                            x,
                            y,
                            width: (plan.x + plan.width).saturating_add(32).min(size.0) - x,
                            height: (plan.y + plan.height).saturating_add(32).min(size.1) - y,
                            scale: plan.scale,
                        };
                        let plan = if expanded.validate(size).is_ok() {
                            expanded
                        } else {
                            plan
                        };
                        upscaler.encode(
                            &self.context,
                            &mut self.encoder,
                            self.images[1].device.into(),
                            input.source.dimensions(),
                            plan,
                        )
                    };
                    match enhanced {
                        Ok(region) => {
                            self.enhanced_region = Some((region, input.source.dimensions()));
                            enhanced_buffer = region.buffer;
                            let plan = region.plan;
                            params.enhanced_rect = [
                                plan.x as f32,
                                plan.y as f32,
                                plan.width as f32,
                                plan.height as f32,
                            ];
                            params.enhanced_info = [
                                region.width as f32,
                                region.height as f32,
                                plan.scale as f32,
                                strength,
                            ];
                        }
                        Err(error) => {
                            eprintln!("Screen enhancement skipped: {error}");
                            self.enhancement_failed = true;
                        }
                    }
                }
            }
        }
        self.params.upload(
            &self.context,
            &mut self.encoder,
            bytemuck::bytes_of(&params),
            false,
        )?;
        let resize = params.images[6][2] != 0
            && (params.images[6][0], params.images[6][1]) != (scene.width, scene.height);
        let caption_size = (
            params.images[6][0],
            params.images[6][1],
            scene.width,
            scene.height,
        );
        if resize && (caption_changed || self.caption_size != caption_size) {
            self.caption_temp.ensure(
                &self.context,
                params.images[6][0] as usize * scene.height as usize * 16,
            )?;
            self.caption_scaled.ensure(&self.context, output_bytes)?;
            let data = ResizeData {
                b_params: self.params.device.into(),
                b_resize_source: self.images[6].device.into(),
                b_resize_temp: self.caption_temp.device.into(),
                b_resize_output: self.caption_scaled.device.into(),
            };
            {
                let mut pass = self.encoder.compute("caption vertical");
                let mut pipeline = pass.with(&self.resize_vertical);
                pipeline.bind(0, &data);
                pipeline.dispatch([params.images[6][0].div_ceil(8), scene.height.div_ceil(8), 1]);
            }
            {
                let mut pass = self.encoder.compute("caption horizontal");
                let mut pipeline = pass.with(&self.resize_horizontal);
                pipeline.bind(0, &data);
                pipeline.dispatch([scene.width.div_ceil(8), scene.height.div_ceil(8), 1]);
            }
            self.caption_size = caption_size;
        }
        {
            let mut pass = self.encoder.compute("compose layers");
            let mut pipeline = pass.with(&self.pipeline);
            pipeline.bind(
                0,
                &ComposeData {
                    b_params: self.params.device.into(),
                    b_background: self.images[0].device.into(),
                    b_source: self.images[1].device.into(),
                    b_source_mips: self.mip_buffer.device.into(),
                    b_enhanced: enhanced_buffer,
                    b_overlay: self.images[2].device.into(),
                    b_camera: self.images[3].device.into(),
                    b_cursor: self.images[4].device.into(),
                    b_shadow: self.images[5].device.into(),
                    b_caption: if resize {
                        self.caption_scaled.device
                    } else {
                        self.images[6].device
                    }
                    .into(),
                    b_watermark: self.images[7].device.into(),
                    b_output: self.output.device.into(),
                },
            );
            pipeline.dispatch([scene.width.div_ceil(8), scene.height.div_ceil(8), 1]);
        }
        self.encoder.transfer("readback").copy_buffer_to_buffer(
            self.output.device.into(),
            self.output.host.into(),
            output_bytes as u64,
        );
        let prepared = std::time::Instant::now();
        let sync = self.context.submit(&mut self.encoder);
        self.pending = Some(sync.clone());
        if !self.context.wait_for(&sync, 5000) {
            return Err("GPU timed out".into());
        }
        self.pending = None;
        self.context.sync_buffer(self.output.host);
        let rendered = std::time::Instant::now();
        // SAFETY: the GPU copy has completed, the mapped allocation contains
        // output_bytes initialized bytes, and no submission can mutate it here.
        let bytes = copy_readback(unsafe {
            std::slice::from_raw_parts(self.output.host.data(), output_bytes)
        });
        if profile {
            eprintln!(
                "GPU frame: prepare={:.2}ms submit/wait={:.2}ms readback={:.2}ms",
                (prepared - started).as_secs_f64() * 1000.0,
                (rendered - prepared).as_secs_f64() * 1000.0,
                rendered.elapsed().as_secs_f64() * 1000.0
            );
        }
        RgbaImage::from_raw(scene.width, scene.height, bytes)
            .ok_or_else(|| "invalid GPU frame dimensions".into())
    }
}

/// Host-visible staging may use write-combining memory. Streaming loads fetch
/// complete cache lines, as in FFmpeg's av_image_copy_plane_uc_from, instead of
/// making ordinary uncached loads for every few bytes.
/// https://ffmpeg.org/doxygen/trunk/group__lavu__picture.html
fn copy_readback(source: &[u8]) -> Vec<u8> {
    let mut output = vec![0; source.len()];
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("sse4.1") && source.as_ptr().align_offset(64) == 0 {
        // SAFETY: runtime feature check and 64-byte source alignment above;
        // both slices contain the same number of bytes and do not overlap.
        unsafe {
            copy_readback_sse41(source, &mut output);
        }
        return output;
    }
    output.copy_from_slice(source);
    output
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn copy_readback_sse41(source: &[u8], output: &mut [u8]) {
    use std::arch::x86_64::{__m128i, _mm_storeu_si128, _mm_stream_load_si128};
    let end = source.len() / 64 * 64;
    for offset in (0..end).step_by(64) {
        // Load a complete aligned cache line before storing, so PCIe reads
        // can be combined. The destination need not have SIMD alignment.
        let src = source.as_ptr().add(offset).cast::<__m128i>();
        let a = _mm_stream_load_si128(src);
        let b = _mm_stream_load_si128(src.add(1));
        let c = _mm_stream_load_si128(src.add(2));
        let d = _mm_stream_load_si128(src.add(3));
        let dst = output.as_mut_ptr().add(offset).cast::<__m128i>();
        _mm_storeu_si128(dst, a);
        _mm_storeu_si128(dst.add(1), b);
        _mm_storeu_si128(dst.add(2), c);
        _mm_storeu_si128(dst.add(3), d);
    }
    output[end..].copy_from_slice(&source[end..]);
}

impl Drop for Compositor {
    fn drop(&mut self) {
        debug_assert!(self.pending.is_none());
        if let Some(upscaler) = &mut self.upscaler {
            upscaler.destroy(&self.context);
        }
        for image in &mut self.images {
            image.destroy(&self.context);
        }
        for buffer in [
            &mut self.params,
            &mut self.output,
            &mut self.caption_temp,
            &mut self.caption_scaled,
        ] {
            buffer.destroy(&self.context);
        }
        self.context.destroy_compute_pipeline(&mut self.pipeline);
        self.context
            .destroy_compute_pipeline(&mut self.mip_pipeline);
        self.mip_buffer.destroy(&self.context);
        self.mip_params.destroy(&self.context);
        self.context
            .destroy_compute_pipeline(&mut self.resize_vertical);
        self.context
            .destroy_compute_pipeline(&mut self.resize_horizontal);
        self.context.destroy_command_encoder(&mut self.encoder);
    }
}

fn draw_params(command: &PointerCommand, width: u32, height: u32) -> Draw {
    let mut draw = Draw::zeroed();
    match command {
        PointerCommand::Disc {
            x,
            y,
            radius,
            line_width,
            color,
            opacity,
        } => {
            draw.kind = [
                if line_width.is_some() { 2.0 } else { 1.0 },
                *opacity as f32,
                0.0,
                0.0,
            ];
            draw.rect = [
                *x as f32,
                *y as f32,
                *radius as f32,
                line_width.unwrap_or(0.0) as f32,
            ];
            draw.color = [color[0] as f32, color[1] as f32, color[2] as f32, 0.0];
            draw.bounds = [
                (*x - radius - 1.0).floor().max(0.0) as f32,
                (*y - radius - 1.0).floor().max(0.0) as f32,
                (*x + radius + 1.0).ceil().min(width as f64) as f32,
                (*y + radius + 1.0).ceil().min(height as f64) as f32,
            ];
        }
        PointerCommand::Bitmap {
            bitmap,
            placement: p,
            opacity,
            shadow_only,
        } => {
            let (sin, cos) = p.tilt_degrees.to_radians().sin_cos();
            let ax = bitmap.anchor.x * p.width;
            let ay = bitmap.anchor.y * p.height;
            draw.kind = [
                3.0,
                *opacity as f32,
                f32::from(*shadow_only),
                if p.smear.0.hypot(p.smear.1) < 0.5 {
                    1.0
                } else {
                    CURSOR_SMEAR_TAPS as f32
                },
            ];
            draw.rect = [
                p.hotspot_x as f32,
                p.hotspot_y as f32,
                p.width as f32,
                p.height as f32,
            ];
            draw.anchor = [ax as f32, ay as f32, sin as f32, cos as f32];
            draw.smear = [
                (p.smear.0 * cos + p.smear.1 * sin) as f32,
                (-p.smear.0 * sin + p.smear.1 * cos) as f32,
                0.0,
                0.0,
            ];
            let corners = [
                (0.0, 0.0),
                (p.width, 0.0),
                (0.0, p.height),
                (p.width, p.height),
            ]
            .map(|(x, y)| {
                (
                    p.hotspot_x + (x - ax) * cos - (y - ay) * sin,
                    p.hotspot_y + (x - ax) * sin + (y - ay) * cos,
                )
            });
            draw.bounds = [
                (corners.iter().map(|p| p.0).fold(f64::INFINITY, f64::min)
                    - 1.0
                    - p.smear.0.max(0.0))
                .floor()
                .max(0.0) as f32,
                (corners.iter().map(|p| p.1).fold(f64::INFINITY, f64::min)
                    - 1.0
                    - p.smear.1.max(0.0))
                .floor()
                .max(0.0) as f32,
                (corners
                    .iter()
                    .map(|p| p.0)
                    .fold(f64::NEG_INFINITY, f64::max)
                    + 1.0
                    - p.smear.0.min(0.0))
                .ceil()
                .min(width as f64) as f32,
                (corners
                    .iter()
                    .map(|p| p.1)
                    .fold(f64::NEG_INFINITY, f64::max)
                    + 1.0
                    - p.smear.1.min(0.0))
                .ceil()
                .min(height as f64) as f32,
            ];
        }
    }
    draw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neural_plan_preserves_native_pixels_and_limits_work_to_enlarged_regions() {
        let source = RgbaImage::new(1920, 1080);
        let style = SceneStyle {
            padding: 0,
            corners: 0,
            shadow: 0,
            ..Default::default()
        };
        let scene = SceneCompositor::new(&style, 960, 540, 1920, 1080).unwrap();
        let mut input = FrameInput {
            source: &source,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: None,
        };
        assert!(
            enhancement_plan(&scene, input).is_none(),
            "downscaling must not run a neural pass"
        );
        input.viewport.magnification = 4.0;
        let (plan, strength) = enhancement_plan(&scene, input).unwrap();
        assert_eq!(plan.scale, 2);
        assert_eq!(strength, 1.0);
        assert!(
            plan.width < 600 && plan.height < 400,
            "enhance the visible zoom crop, not the whole frame: {plan:?}"
        );
        input.viewport.magnification = 8.0;
        assert_eq!(enhancement_plan(&scene, input).unwrap().0.scale, 4);
        input.viewport = ViewportFrame {
            loupe: Some(super::super::super::viewport::LoupeFrame {
                center: NormalizedPoint { x: 0.8, y: 0.4 },
                radius: 0.2,
                magnification: 4.0,
                bevel: 0.1,
                chromatic_aberration: 0.0,
                presence: 1.0,
            }),
            ..Default::default()
        };
        let (plan, _) = enhancement_plan(&scene, input).unwrap();
        assert_eq!(plan.scale, 2);
        assert!(
            plan.x > 1000 && plan.width < 600 && plan.height < 600,
            "lens ROI follows its source position: {plan:?}"
        );
        assert!(plan.validate(source.dimensions()).is_ok());
    }

    #[test]
    #[ignore = "requires a hardware Vulkan GPU; validates enhancement in final composition"]
    fn hardware_neural_enhancement_is_composed_beneath_annotations_and_inside_lens() {
        let mut gpu = Compositor::new().expect("hardware Vulkan compositor");
        let source = RgbaImage::from_fn(128, 96, |x, y| {
            let value = if (x / 3 + y / 5) % 2 == 0 { 25 } else { 220 };
            Rgba([value, value, value, 255])
        });
        let mut overlay = RgbaImage::new(128, 96);
        for y in 45..51 {
            for x in 60..68 {
                overlay.put_pixel(x, y, Rgba([20, 240, 90, 255]));
            }
        }
        let style = SceneStyle {
            padding: 0,
            corners: 0,
            shadow: 0,
            ..Default::default()
        };
        let scene = SceneCompositor::new(&style, 128, 96, 128, 96).unwrap();
        let input = FrameInput {
            source: &source,
            overlay: Some(&overlay),
            viewport: ViewportFrame {
                loupe: Some(super::super::super::viewport::LoupeFrame {
                    center: NormalizedPoint { x: 0.5, y: 0.5 },
                    radius: 0.32,
                    magnification: 3.0,
                    bevel: 0.12,
                    chromatic_aberration: 0.0,
                    presence: 1.0,
                }),
                ..Default::default()
            },
            pointer: None,
            camera: None,
        };
        let reference = gpu.compose(&scene, input, true, None).unwrap();
        let enhanced = gpu.compose_enhanced(&scene, input, true, None).unwrap();
        assert!(
            gpu.upscaler.is_some(),
            "the composed frame must actually dispatch the neural model"
        );
        assert_eq!(
            enhanced.get_pixel(64, 48),
            reference.get_pixel(64, 48),
            "opaque editor annotation stays above enhancement"
        );
        for y in 0..96 {
            for x in [0, 1, 126, 127] {
                assert_eq!(
                    enhanced.get_pixel(x, y),
                    reference.get_pixel(x, y),
                    "unmagnified source remains unchanged"
                );
            }
        }
        let changed = enhanced
            .pixels()
            .zip(reference.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            changed > 100,
            "neural output must reach the final lens ({changed} changed pixels)"
        );
        if let Some(path) = std::env::var_os("LAHZA_NEURAL_COMPOSITE_ARTIFACTS") {
            let path = std::path::PathBuf::from(path);
            std::fs::create_dir_all(&path).unwrap();
            reference.save(path.join("source-lens.png")).unwrap();
            enhanced.save(path.join("enhanced-lens.png")).unwrap();
        }
    }

    #[test]
    fn gpu_export_lease_is_nonblocking_and_released_after_drop_or_unwind() {
        let in_use = AtomicBool::new(false);
        let lease = GpuExportLease::try_acquire(&in_use).unwrap();
        std::thread::scope(|scope| {
            let contenders: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        assert!(GpuExportLease::try_acquire(&in_use).is_none());
                    })
                })
                .collect();
            for contender in contenders {
                contender.join().unwrap();
            }
        });
        drop(lease);
        let result = catch_unwind(|| {
            let _lease = GpuExportLease::try_acquire(&in_use).unwrap();
            panic!("GPU initialization failed");
        });
        assert!(result.is_err());
        assert!(GpuExportLease::try_acquire(&in_use).is_some());
    }

    #[test]
    fn concurrent_renderer_uses_cpu_without_waiting_for_the_gpu_owner() {
        // This test never opens a GPU. Wait only for other parallel tests to
        // finish their current export, then model an owner holding the lease.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let lease = loop {
            if let Some(lease) = GpuExportLease::try_acquire(&GPU_EXPORT_IN_USE) {
                break lease;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "previous GPU export did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let (finished, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut renderer = Renderer::new();
            assert!(renderer.gpu.is_none());
            assert_eq!(renderer.label(), "CPU");
            let scene = SceneCompositor::new(&SceneStyle::default(), 64, 48, 32, 24).unwrap();
            let source = RgbaImage::from_pixel(32, 24, Rgba([50, 100, 150, 255]));
            let input = FrameInput {
                source: &source,
                overlay: None,
                viewport: ViewportFrame::default(),
                pointer: None,
                camera: None,
            };
            assert_eq!(
                renderer.compose(&scene, input, true, None),
                scene.compose_layers(input, true, None)
            );
            finished.send(()).unwrap();
        });
        let completed = result.recv_timeout(std::time::Duration::from_secs(2));
        drop(lease);
        worker.join().unwrap();
        completed.expect("CPU fallback waited for the GPU owner");
    }

    #[test]
    fn readback_copy_preserves_aligned_unaligned_and_partial_cache_lines() {
        #[repr(align(64))]
        struct Aligned([u8; 1024]);
        let data = Aligned(std::array::from_fn(|index| (index * 17 % 251) as u8));
        for offset in [0, 1, 7, 16, 63, 64] {
            for length in [0, 1, 15, 16, 63, 64, 65, 129, 513] {
                let source = &data.0[offset..offset + length];
                assert_eq!(copy_readback(source), source);
            }
        }
    }

    #[test]
    fn enhancement_motion_policy_matches_device_pixel_thresholds() {
        let quality = |motion, trail| {
            RenderQuality {
                enhance: true,
                motion_pixels: motion,
                trail_pixels: trail,
            }
            .strength()
        };
        assert_eq!(quality(4.0, 4.0), 1.0);
        assert_eq!(quality(16.0, 0.0), 0.5);
        assert_eq!(quality(0.0, 6.0), 0.5);
        assert_eq!(quality(28.0, 0.0), 0.0);
        assert_eq!(quality(0.0, 8.0), 0.0);
        assert_eq!(quality(f32::NAN, 0.0), 0.0);
        for motion in 0..300 {
            assert_eq!((quality(motion as f32 / 10.0, 0.0) * 16.0).fract(), 0.0);
        }
    }
    #[test]
    fn mip_shader_validates_without_a_gpu() {
        let source = include_str!("source_mips.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        let flags = naga::valid::ValidationFlags::all() ^ naga::valid::ValidationFlags::BINDINGS;
        naga::valid::Validator::new(flags, naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap();
    }

    #[test]
    fn shaders_validate_without_a_gpu() {
        let source = include_str!("scene_gpu.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        let flags = naga::valid::ValidationFlags::all() ^ naga::valid::ValidationFlags::BINDINGS;
        naga::valid::Validator::new(flags, naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap();
        let (_, params) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Params"))
            .unwrap();
        let naga::TypeInner::Struct { span, .. } = params.inner else {
            panic!("Params must be a struct")
        };
        assert_eq!(span as usize, std::mem::size_of::<Params>());
        assert_eq!(module.entry_points.len(), 3);
    }

    fn pattern(width: u32, height: u32, alpha: bool) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                ((x * 3 + y * 7) % 256) as u8,
                ((x * 13 + y * 5) % 256) as u8,
                ((x * 7 + y * 11) % 256) as u8,
                if alpha {
                    ((x * 17 + y * 13) % 256) as u8
                } else {
                    255
                },
            ])
        })
    }

    fn compare(
        gpu: &mut Compositor,
        scene: &SceneCompositor,
        input: FrameInput<'_>,
        visible: bool,
        caption: Option<&RgbaImage>,
    ) {
        let expected = scene.compose_layers(input, visible, caption);
        let actual = gpu.compose(scene, input, visible, caption).unwrap();
        assert_eq!(actual.dimensions(), expected.dimensions());
        let errors: Vec<_> = actual
            .as_raw()
            .iter()
            .zip(expected.as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let max = *errors.iter().max().unwrap();
        let mean = errors.iter().map(|e| *e as f64).sum::<f64>() / errors.len() as f64;
        if max > 6 || mean >= 0.15 {
            eprintln!(
                "parity source={:?} canvas={:?} visible={visible} viewport={:?}",
                input.source.dimensions(),
                actual.dimensions(),
                input.viewport
            );
            actual.save("/tmp/lahza-parity-gpu.png").unwrap();
            expected.save("/tmp/lahza-parity-cpu.png").unwrap();
        }
        assert!(
            max <= 6 && mean < 0.15,
            "GPU/CPU mismatch: max={max}, mean={mean:.4}"
        );
    }

    #[test]
    fn shared_handles_keep_camera_errors_with_their_own_frame() {
        let backend = Arc::new(Mutex::new(Renderer {
            gpu: None,
            matting: Default::default(),
            camera_error: None,
            label: "CPU".into(),
        }));
        let mut a = SharedRenderer {
            backend: backend.clone(),
            camera_error: None,
            label: "CPU".into(),
        };
        let mut b = SharedRenderer {
            backend,
            camera_error: None,
            label: "CPU".into(),
        };
        let source = pattern(64, 48, false);
        let empty = RgbaImage::new(0, 0);
        let mut style = SceneStyle::default();
        style.camera.enabled = true;
        style.camera.layout = CameraLayout::Cutout;
        let scene = SceneCompositor::new(&style, 64, 48, 64, 48).unwrap();
        let input = FrameInput {
            source: &source,
            overlay: None,
            viewport: Default::default(),
            pointer: None,
            camera: Some(&empty),
        };
        a.compose_fast(&scene, input, true, None);
        b.compose_fast(
            &scene,
            FrameInput {
                camera: None,
                ..input
            },
            true,
            None,
        );
        assert_eq!(
            a.take_camera_error().as_deref(),
            Some("Camera frame is empty")
        );
        assert!(b.take_camera_error().is_none());
        assert!(a.take_camera_error().is_none());
    }

    #[test]
    fn cpu_fallback_keeps_the_same_frame_and_layers() {
        let scene = SceneCompositor::new(&SceneStyle::default(), 96, 64, 48, 32).unwrap();
        let source = pattern(48, 32, true);
        let caption = pattern(24, 16, true);
        let input = FrameInput {
            source: &source,
            overlay: None,
            viewport: Default::default(),
            pointer: None,
            camera: None,
        };
        let mut renderer = Renderer {
            gpu: None,
            matting: Default::default(),
            camera_error: None,
            label: "CPU".into(),
        };
        for visible in [true, false] {
            assert_eq!(
                renderer.compose(&scene, input, visible, Some(&caption)),
                scene.compose_layers(input, visible, Some(&caption))
            );
        }
    }

    #[test]
    #[ignore = "requires a hardware Vulkan GPU; verifies animated shadow pixels and mask reuse"]
    fn hardware_camera_transitions_preserve_shadows_and_reuse_fade_masks() {
        use super::super::super::camera_timeline::CameraRegion;

        let mut gpu = Compositor::new().expect("hardware Vulkan compositor");
        let source = pattern(128, 96, false);
        let camera = pattern(80, 48, true);
        let input = FrameInput {
            source: &source,
            overlay: None,
            viewport: Default::default(),
            pointer: None,
            camera: Some(&camera),
        };
        let mut authored = SceneStyle::default();
        authored.camera.layout = CameraLayout::Cutout;
        authored.camera_regions = vec![CameraRegion {
            start: 1.0,
            end: 3.0,
            transition: 1.0,
            overlay: CameraOverlay {
                layout: CameraLayout::Split,
                split_full: true,
                ..Default::default()
            },
            ..Default::default()
        }];
        for time in [1.1, 1.5, 1.9] {
            let mut evaluated = authored.clone();
            evaluated.camera = authored.camera_at(time, (256, 192), 80.0 / 48.0, 1.0);
            assert_eq!(evaluated.camera.shadow_strength(), 0.0);
            let scene = SceneCompositor::new(&evaluated, 256, 192, 128, 96).unwrap();
            compare(&mut gpu, &scene, input, true, None);
            assert!(
                gpu.shadow_key.is_none(),
                "shadowless layouts must not build a mask"
            );
        }

        // Only opacity changes when a stationary floating camera is hidden.
        // Crop/mirror also affect the image, not the shadow's geometry.
        authored.camera = CameraOverlay::default();
        authored.camera_regions[0].overlay = CameraOverlay {
            layout: CameraLayout::Hidden,
            mirror: true,
            crop_center: (0.35, 0.65),
            crop_zoom: 150,
            ..Default::default()
        };
        let mut previous_key = None;
        for time in [1.1, 1.5, 1.9] {
            let mut evaluated = authored.clone();
            evaluated.camera = authored.camera_at(time, (256, 192), 80.0 / 48.0, 1.0);
            assert_eq!(evaluated.camera.shadow_strength(), 1.0);
            let scene = SceneCompositor::new(&evaluated, 256, 192, 128, 96).unwrap();
            compare(&mut gpu, &scene, input, true, None);
            let key = gpu.shadow_key.expect("floating camera shadow mask");
            if let Some(previous) = previous_key {
                assert_eq!(key, previous, "opacity/crop changes must reuse the mask");
            }
            previous_key = Some(key);
        }
    }

    #[test]
    #[ignore = "requires a hardware Vulkan GPU; verifies GPU pixels against the CPU compositor"]
    fn hardware_composition_matches_cpu() {
        let mut gpu = Compositor::new().expect("hardware Vulkan compositor");
        eprintln!(
            "Testing composition on {}",
            gpu.context.device_information().device_name
        );
        let mut style = SceneStyle::default();
        style.padding = 9;
        style.vignette = 35;
        let source = pattern(128, 96, true);
        let overlay = pattern(64, 48, true);
        let camera = pattern(80, 48, false);
        let mut caption = pattern(48, 24, true);
        let mut pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.42, y: 0.53 },
                artwork_id: None,
                bitmap: cursor_assets::rasterize(CursorFamily::MacOs, Default::default(), 64),
                magnification: 1.2,
                tilt_degrees: 17.0,
                opacity: 0.8,
                blur_radius: 0.0,
                velocity: (0.9, -0.6),
                press: Some(super::super::super::pointer_timeline::PointerPressFrame {
                    location: NormalizedPoint { x: 0.6, y: 0.4 },
                    progress: 0.4,
                }),
            },
        };
        for variant in 0..12 {
            pointer.frame.blur_radius = if variant % 2 == 1 { 7.5 } else { 0.0 };
            style.watermark = (variant % 3 == 0).then(|| Watermark {
                text: "Lahza".into(),
                ..Default::default()
            });
            style.camera.shape = CameraShape::ALL[variant % 3];
            style.camera.aspect = CameraAspect::ALL[variant % CameraAspect::ALL.len()];
            style.camera.layout = CameraLayout::ALL[variant % CameraLayout::ALL.len()];
            style.camera.zoomed_size = Some(16);
            style.camera.crop_zoom = 150;
            style.camera.crop_center = (0.35, 0.65);
            style.camera.mirror = variant % 2 == 0;
            style.camera.enabled = variant != 9;
            style.camera.shadow = variant != 10;
            style.window_frame =
                [WindowFrame::Off, WindowFrame::Light, WindowFrame::Dark][variant % 3];
            style.pointer.family = if variant % 2 == 0 {
                CursorFamily::Recorded
            } else {
                CursorFamily::MacOs
            };
            style.border = variant % 2 == 0;
            style.transform.rotation_y = if variant % 2 == 0 { 0.0 } else { 27.0 };
            style.transform.position_x = if variant == 11 { 6.0 } else { -0.07 };
            style.source_crop = if variant > 5 {
                crate::CropRect {
                    x: 0.1,
                    y: 0.2,
                    width: 0.7,
                    height: 0.6,
                }
            } else {
                crate::CropRect::UNIT
            };
            pointer.frame.opacity = if variant == 8 { 0.0 } else { 0.8 };
            pointer.frame.velocity = if variant == 7 {
                (0.0, 0.0)
            } else {
                (0.9, -0.6)
            };
            let (w, h) = if variant % 2 == 0 {
                (256, 192)
            } else {
                (192, 256)
            };
            let scene =
                SceneCompositor::new(&style, w, h, source.width(), source.height()).unwrap();
            for zoom in [1.0, 1.8, 1.0] {
                let input = FrameInput {
                    source: &source,
                    overlay: Some(&overlay),
                    viewport: ViewportFrame {
                        magnification: zoom,
                        anchor: NormalizedPoint { x: 0.5, y: 0.5 },
                        ..Default::default()
                    },
                    pointer: Some(&pointer),
                    camera: Some(&camera),
                };
                eprintln!("variant={variant} zoom={zoom}");
                compare(&mut gpu, &scene, input, true, Some(&caption));
                compare(&mut gpu, &scene, input, false, None);
                caption.put_pixel(10, 10, Rgba([variant as u8 * 10, 128, 224, 100]));
                compare(&mut gpu, &scene, input, true, Some(&caption));
            }
        }
        // Same bytes reshaped must invalidate the resized-caption cache.
        let scene = SceneCompositor::new(&style, 128, 96, 128, 96).unwrap();
        let input = FrameInput {
            source: &source,
            overlay: None,
            viewport: Default::default(),
            pointer: None,
            camera: None,
        };
        compare(&mut gpu, &scene, input, false, Some(&caption));
        let reshaped = RgbaImage::from_raw(24, 48, caption.into_raw()).unwrap();
        compare(&mut gpu, &scene, input, false, Some(&reshaped));
        // Reuse the same GPU allocations across source/window resizes and
        // output sizes that do not divide evenly into workgroups.
        for (width, height) in [(320, 180), (72, 120), (1920, 1080)] {
            let source = pattern(width, height, true);
            let scene =
                SceneCompositor::new(&SceneStyle::default(), 318, 182, width, height).unwrap();
            let input = FrameInput {
                source: &source,
                overlay: None,
                viewport: Default::default(),
                pointer: None,
                camera: Some(&camera),
            };
            compare(&mut gpu, &scene, input, true, None);
        }
        let source = pattern(1920, 1080, false);
        let scene = SceneCompositor::new(&SceneStyle::default(), 1920, 1080, 1920, 1080).unwrap();
        let input = FrameInput {
            source: &source,
            overlay: None,
            viewport: Default::default(),
            pointer: Some(&pointer),
            camera: Some(&camera),
        };
        compare(&mut gpu, &scene, input, true, Some(&reshaped));
    }

    #[test]
    #[ignore = "requires a hardware Vulkan GPU; compares animated glass optics with the CPU"]
    fn hardware_loupe_composition_matches_cpu() {
        let mut gpu = Compositor::new().expect("hardware Vulkan compositor");
        let source = pattern(160, 100, false);
        let overlay = pattern(160, 100, true);
        let camera = pattern(80, 48, false);
        let caption = pattern(256, 192, true);
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.55, y: 0.45 },
                artwork_id: None,
                bitmap: cursor_assets::rasterize(CursorFamily::MacOs, Default::default(), 64),
                magnification: 1.2,
                tilt_degrees: 17.0,
                opacity: 0.8,
                blur_radius: 0.0,
                velocity: (0.9, -0.6),
                press: Some(super::super::super::pointer_timeline::PointerPressFrame {
                    location: NormalizedPoint { x: 0.55, y: 0.45 },
                    progress: 0.4,
                }),
            },
        };
        for variant in 0..4 {
            let mut style = SceneStyle::default();
            style.padding = 8;
            style.transform.rotation_y = if variant == 1 { 22.0 } else { 0.0 };
            style.source_crop = if variant == 2 {
                crate::CropRect {
                    x: 0.15,
                    y: 0.1,
                    width: 0.75,
                    height: 0.8,
                }
            } else {
                crate::CropRect::UNIT
            };
            let scene = SceneCompositor::new(&style, 256, 192, 160, 100).unwrap();
            for presence in [0.15, 0.6, 1.0] {
                let frame = super::super::super::viewport::LoupeFrame {
                    center: NormalizedPoint {
                        x: if variant == 3 { 0.05 } else { 0.55 },
                        y: 0.45,
                    },
                    radius: 0.35,
                    magnification: 1.0 + presence,
                    bevel: 0.2 * presence,
                    chromatic_aberration: 0.5 * presence,
                    presence,
                };
                let input = FrameInput {
                    source: &source,
                    overlay: Some(&overlay),
                    viewport: ViewportFrame {
                        loupe: Some(frame),
                        ..Default::default()
                    },
                    pointer: None,
                    camera: Some(&camera),
                };
                compare(&mut gpu, &scene, input, true, Some(&caption));
                compare(
                    &mut gpu,
                    &scene,
                    FrameInput {
                        pointer: Some(&pointer),
                        ..input
                    },
                    true,
                    Some(&caption),
                );
                compare(&mut gpu, &scene, input, false, None);
            }
        }
    }

    #[test]
    #[ignore = "isolated GPU temporal playback benchmark including camera cutout and readback"]
    fn benchmark_hardware_temporal_composition_with_cutout() {
        let mut renderer = Renderer::new();
        assert!(renderer.gpu.is_some(), "requires hardware Vulkan");
        let mut source = pattern(1920, 1080, false);
        let mut camera = pattern(640, 480, false);
        let mut style = SceneStyle::default();
        style.camera.enabled = true;
        style.camera.layout = CameraLayout::Cutout;
        let scene = SceneCompositor::new(&style, 1600, 900, 1920, 1080).unwrap();
        for samples in [1usize, 2, 6] {
            let mut elapsed = 0.0;
            for frame in 0..14 {
                source.put_pixel(900, 500, Rgba([frame as u8, 20, 30, 255]));
                camera.put_pixel(300, 200, Rgba([frame as u8, 20, 30, 255]));
                let start = std::time::Instant::now();
                let mut accumulator = None;
                for sample in 0..samples {
                    let output = renderer.compose_inner(
                        &scene,
                        FrameInput {
                            source: &source,
                            overlay: None,
                            pointer: None,
                            camera: Some(&camera),
                            viewport: ViewportFrame {
                                magnification: 1.5 + sample as f64 * 0.015,
                                ..Default::default()
                            },
                        },
                        true,
                        None,
                        RenderQuality {
                            enhance: true,
                            motion_pixels: 60.0,
                            trail_pixels: 30.0,
                        },
                    );
                    if let Some(accumulator) = &mut accumulator {
                        super::super::super::motion_blur::Accumulator::add(accumulator, &output);
                    } else {
                        accumulator =
                            Some(super::super::super::motion_blur::Accumulator::new(output));
                    }
                }
                std::hint::black_box(accumulator.unwrap().finish());
                assert!(renderer.label.starts_with("Vulkan"), "{}", renderer.label);
                if frame >= 2 {
                    elapsed += start.elapsed().as_secs_f64();
                }
            }
            eprintln!(
                "1600x900 temporal samples={samples} cutout=true: {:.2} ms/frame",
                elapsed * 1000.0 / 12.0
            );
        }
    }

    #[test]
    #[ignore = "requires a hardware Vulkan GPU; measures composition including upload and readback"]
    fn benchmark_hardware_composition() {
        let mut gpu = Compositor::new().unwrap();
        let source = pattern(1920, 1080, false);
        let camera = pattern(640, 480, false);
        let scene = SceneCompositor::new(&SceneStyle::default(), 1920, 1080, 1920, 1080).unwrap();
        let input = FrameInput {
            source: &source,
            overlay: None,
            viewport: Default::default(),
            pointer: None,
            camera: Some(&camera),
        };
        compare(&mut gpu, &scene, input, true, None);
        let frames = if std::env::var_os("LAHZA_GPU_PROFILE").is_some() {
            3
        } else {
            120
        };
        for hardware in [false, true] {
            let start = std::time::Instant::now();
            for _ in 0..frames {
                let output = if hardware {
                    gpu.compose(&scene, input, true, None).unwrap()
                } else {
                    scene.compose(input)
                };
                std::hint::black_box(output);
            }
            eprintln!(
                "1080p composition hardware={hardware}: {:.2} ms/frame",
                start.elapsed().as_secs_f64() * 1000.0 / frames as f64
            );
        }
    }
}
