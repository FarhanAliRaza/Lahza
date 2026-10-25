//! Offscreen Vulkan composition. CPU code prepares geometry and cached assets;
//! compute shaders resample and blend the moving layers. No window is required.
use super::*;
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use gpu::ShaderData as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_BUFFER: usize = 128 * 1024 * 1024;
static GPU_EXPORT_IN_USE: AtomicBool = AtomicBool::new(false);

/// Vulkan loader/driver initialization can race another context's resource
/// calls. One export owns GPU access through teardown; competing exports use
/// the existing CPU renderer immediately, keeping cancellation responsive.
struct GpuExportLease<'a>(&'a AtomicBool);

impl<'a> GpuExportLease<'a> {
    fn try_acquire(in_use: &'a AtomicBool) -> Option<Self> {
        in_use.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok().map(|_| Self(in_use))
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
    sample_bounds: [f32; 4],
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

/// GPU failure is sticky for this export. The same CPU geometry and inputs
/// render the failed frame, so a fallback never skips or duplicates a frame.
pub struct Renderer {
    gpu: Option<Compositor>,
    label: String,
}

impl Renderer {
    pub fn new() -> Self {
        if std::env::var("LAHZA_COMPOSITOR").as_deref() == Ok("cpu") {
            return Self {
                gpu: None,
                label: "CPU".into(),
            };
        }
        match catch_unwind(AssertUnwindSafe(Compositor::new)) {
            Ok(Ok(gpu)) => {
                let label = format!("Vulkan · {}", gpu.context.device_information().device_name);
                eprintln!("Lahza composition: {label}");
                Self {
                    gpu: Some(gpu),
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
                    label: "CPU".into(),
                }
            }
        }
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
        if let Some(gpu) = &mut self.gpu {
            match catch_unwind(AssertUnwindSafe(|| {
                gpu.compose(scene, input, visible, caption)
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

struct Compositor {
    context: gpu::Context,
    encoder: gpu::CommandEncoder,
    pipeline: gpu::ComputePipeline,
    resize_vertical: gpu::ComputePipeline,
    resize_horizontal: gpu::ComputePipeline,
    images: [Buffer; 8],
    params: Buffer,
    output: Buffer,
    caption_temp: Buffer,
    caption_scaled: Buffer,
    caption_size: (u32, u32, u32, u32),
    shadow_key: Option<(u32, u32, CameraOverlay, Rect)>,
    pending: Option<gpu::SyncPoint>,
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
                    if p.limits.max_per_stage_descriptor_storage_buffers < 10
                        || p.limits.max_descriptor_set_storage_buffers < 10
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
            shadow_key: None,
            pipeline,
            resize_vertical,
            resize_horizontal,
            context,
            pending: None,
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
            let rect = overlay.rect_for_frame(scene.width as f64, scene.height as f64,
                camera.width() as f64 / camera.height() as f64, input.viewport.magnification);
            let crop = super::super::camera_layout::source_crop(overlay,
                camera.width() as f64 / camera.height() as f64, rect);
            params.camera_crop = [crop.x as f32, crop.y as f32, crop.width as f32, crop.height as f32];
            let shadow_enabled = overlay.shadow && overlay.layout != CameraLayout::Fullscreen;
            params.camera_rect = [
                rect.x as f32,
                rect.y as f32,
                rect.width as f32,
                rect.height as f32,
            ];
            params.camera_info = [
                scene.style.camera.radius(rect) as f32,
                f32::from(scene.style.camera.mirror),
                f32::from(shadow_enabled),
                0.0,
            ];
            if shadow_enabled {
                let key = (scene.width, scene.height, scene.style.camera, rect);
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
                    index != 1 && index != 3,
                )?;
                if index == 6 {
                    caption_changed = changed;
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
    fn gpu_export_lease_is_nonblocking_and_released_after_drop_or_unwind() {
        let in_use = AtomicBool::new(false);
        let lease = GpuExportLease::try_acquire(&in_use).unwrap();
        std::thread::scope(|scope| {
            let contenders: Vec<_> = (0..8).map(|_| scope.spawn(|| {
                assert!(GpuExportLease::try_acquire(&in_use).is_none());
            })).collect();
            for contender in contenders { contender.join().unwrap(); }
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
            if let Some(lease) = GpuExportLease::try_acquire(&GPU_EXPORT_IN_USE) { break lease; }
            assert!(std::time::Instant::now() < deadline, "previous GPU export did not finish");
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
                source: &source, overlay: None, viewport: ViewportFrame::default(),
                pointer: None, camera: None,
            };
            assert_eq!(renderer.compose(&scene, input, true, None), scene.compose_layers(input, true, None));
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
        assert!(
            max <= 6 && mean < 0.15,
            "GPU/CPU mismatch: max={max}, mean={mean:.4}"
        );
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
        for variant in 0..4 {
            let mut style = SceneStyle::default();
            style.padding = 8;
            style.transform.rotation_y = if variant == 1 { 22.0 } else { 0.0 };
            style.source_crop = if variant == 2 {
                crate::CropRect { x: 0.15, y: 0.1, width: 0.75, height: 0.8 }
            } else { crate::CropRect::UNIT };
            let scene = SceneCompositor::new(&style, 256, 192, 160, 100).unwrap();
            for presence in [0.15, 0.6, 1.0] {
                let frame = super::super::super::viewport::LoupeFrame {
                    center: NormalizedPoint { x: if variant == 3 { 0.05 } else { 0.55 }, y: 0.45 },
                    radius: 0.35, magnification: 1.0 + presence,
                    bevel: 0.2 * presence, chromatic_aberration: 0.5 * presence, presence,
                };
                let input = FrameInput { source: &source, overlay: Some(&overlay),
                    viewport: ViewportFrame { loupe: Some(frame), ..Default::default() },
                    pointer: None, camera: Some(&camera) };
                compare(&mut gpu, &scene, input, true, Some(&caption));
                compare(&mut gpu, &scene, input, false, None);
            }
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
