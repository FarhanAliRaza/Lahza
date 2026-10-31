//! Tiled GPU UI reconstruction, independently trained on natively rendered
//! screen content. Separate 2× and 4× models predict an RGB residual over
//! bilinear sampling; neither uses Screen Studio weights.
//! The compositor owns the context and submission lifetime; this module only
//! records commands. See assets/upscaler/README.md for provenance and training.
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use gpu::ShaderData as _;

pub const MAX_OUTPUT_PIXELS: u64 = 24_000_000;
pub const MAX_OUTPUT_SIDE: u32 = 8192;
const TILE: u32 = 128;
// At least the deepest model's layer count; must match HALO in the shader.
const HALO: u32 = 8;
const STRIDE: u32 = TILE + HALO * 2;
const PARAM_SLOT: usize = 256;
#[used]
static MODEL_LICENSE: &str = concat!(
    include_str!("../../assets/upscaler/LICENSE"),
    "\nBlade graphics:\n",
    include_str!("../../vendor/blade-graphics/LICENSE")
);

/// A plain 3×3 convolution network: head (RGB → channels), `layers - 2`
/// hidden convolutions, and a tail fed by the last hidden and head features.
struct Model {
    scale: u32,
    channels: u32,
    layers: u32,
    weights: &'static [u8],
}
const MODELS: [Model; 2] = [
    Model {
        scale: 2,
        channels: 16,
        layers: 6,
        weights: include_bytes!("../../assets/upscaler/ui-cnn-v2-2x.f32"),
    },
    Model {
        scale: 4,
        channels: 24,
        layers: 7,
        weights: include_bytes!("../../assets/upscaler/ui-cnn-v2-4x.f32"),
    },
];
const HEAD: u32 = 0;
const BODY: u32 = 1;
const TAIL: u32 = 2;

impl Model {
    fn for_scale(scale: u32) -> &'static Model {
        MODELS.iter().find(|model| model.scale == scale).unwrap()
    }
    /// Per layer: weight offset in vec4s, input groups, output groups, kind.
    fn layers(&self) -> Vec<[u32; 4]> {
        let hidden = self.channels.div_ceil(4);
        let mut offset = 0;
        (0..self.layers)
            .map(|index| {
                let (inputs, outputs, kind) = match index {
                    0 => (1, hidden, HEAD),
                    i if i + 1 == self.layers => {
                        (hidden, (3 * self.scale * self.scale).div_ceil(4), TAIL)
                    }
                    _ => (hidden, hidden, BODY),
                };
                let layer = [offset, inputs, outputs, kind];
                offset += outputs * 9 * inputs * 4 + outputs;
                layer
            })
            .collect()
    }
    fn max_groups(&self) -> u32 {
        self.layers().iter().map(|layer| layer[2]).max().unwrap()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpscalePlan {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

impl UpscalePlan {
    pub fn validate(self, source_size: (u32, u32)) -> Result<(u32, u32), String> {
        let (sw, sh) = source_size;
        if !matches!(self.scale, 2 | 4)
            || self.width == 0
            || self.height == 0
            || self.x.checked_add(self.width).is_none_or(|x| x > sw)
            || self.y.checked_add(self.height).is_none_or(|y| y > sh)
            || sw > i32::MAX as u32
            || sh > i32::MAX as u32
        {
            return Err("invalid neural upscale region or scale".into());
        }
        let w = u64::from(self.width) * u64::from(self.scale);
        let h = u64::from(self.height) * u64::from(self.scale);
        if w > u64::from(MAX_OUTPUT_SIDE)
            || h > u64::from(MAX_OUTPUT_SIDE)
            || w * h > MAX_OUTPUT_PIXELS
        {
            return Err("neural upscale exceeds the bounded output budget".into());
        }
        Ok((w as u32, h as u32))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EnhancedRegion {
    pub buffer: gpu::BufferPiece,
    pub width: u32,
    pub height: u32,
    /// Exact original source-pixel rectangle represented by the entire buffer.
    pub plan: UpscalePlan,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct UpscaleParams {
    sizes: [u32; 4],
    region: [u32; 4],
    tile: [i32; 4],
    tile_output: [u32; 4],
    layer: [u32; 4],
}

#[derive(blade_macros::ShaderData)]
struct ConvData {
    b_up_params: gpu::BufferPiece,
    b_up_source: gpu::BufferPiece,
    b_up_features: gpu::BufferPiece,
    b_up_skip: gpu::BufferPiece,
    b_up_result: gpu::BufferPiece,
    b_up_weights: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ReconstructData {
    b_up_params: gpu::BufferPiece,
    b_up_source: gpu::BufferPiece,
    b_up_features: gpu::BufferPiece,
    b_up_output: gpu::BufferPiece,
}

struct Allocation {
    buffer: gpu::Buffer,
    size: usize,
    memory: gpu::Memory,
}
impl Allocation {
    fn new(context: &gpu::Context, size: usize, memory: gpu::Memory) -> Self {
        let size = size.max(16);
        Self {
            buffer: context.create_buffer(gpu::BufferDesc {
                name: "screen neural upscale",
                size: size as u64,
                memory,
            }),
            size,
            memory,
        }
    }
    fn ensure(&mut self, context: &gpu::Context, size: usize) {
        if size > self.size {
            let next = Self::new(context, size, self.memory);
            context.destroy_buffer(self.buffer);
            *self = next;
        }
    }
}

pub struct ScreenUpscaler {
    convolution: gpu::ComputePipeline,
    reconstruct: gpu::ComputePipeline,
    weights: [Allocation; 2],
    pub uses_f16: bool,
    params: Allocation,
    /// Head features (kept for the tail's skip) and two ping-pong buffers.
    scratch: [Allocation; 3],
    output: Allocation,
}

impl ScreenUpscaler {
    pub fn new(context: &gpu::Context) -> Result<Self, String> {
        std::hint::black_box(MODEL_LICENSE);
        let native_f16 =
            context.capabilities().shader_f16 && std::env::var_os("LAHZA_UPSCALE_F32").is_none();
        let make_source = |half: bool| {
            if half {
                include_str!("screen_upscale.wgsl").replacen(
                    "alias Math = f32;",
                    "enable f16;\nalias Math = f16;",
                    1,
                )
            } else {
                include_str!("screen_upscale.wgsl").to_owned()
            }
        };
        let (shader, uses_f16) = match context.try_create_shader(gpu::ShaderDesc {
            source: &make_source(native_f16),
        }) {
            Ok(shader) => (shader, native_f16),
            Err(_) if native_f16 => (
                context.try_create_shader(gpu::ShaderDesc {
                    source: &make_source(false),
                })?,
                false,
            ),
            Err(error) => return Err(error.to_owned()),
        };
        shader.check_struct_size::<UpscaleParams>();
        let convolution = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "Lahza UI CNN convolution",
            data_layouts: &[&ConvData::layout()],
            compute: shader.at("conv"),
        });
        let reconstruct = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "Lahza UI CNN reconstruct",
            data_layouts: &[&ReconstructData::layout()],
            compute: shader.at("reconstruct"),
        });
        let weights = MODELS.each_ref().map(|model| {
            let coefficients = model.weights;
            let [offset, inputs, outputs, _] = *model.layers().last().unwrap();
            assert_eq!(
                (offset + outputs * 9 * inputs * 4 + outputs) as usize * 16,
                coefficients.len(),
                "model coefficients must match the {}× layer table",
                model.scale
            );
            let allocation = Allocation::new(context, coefficients.len(), gpu::Memory::Shared);
            // Immutable coefficients are uploaded before their first submission.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    coefficients.as_ptr(),
                    allocation.buffer.data(),
                    coefficients.len(),
                );
            }
            context.sync_buffer(allocation.buffer);
            allocation
        });
        let groups = MODELS.iter().map(Model::max_groups).max().unwrap();
        assert!(groups <= 12, "MAX_GROUPS in the shader bounds every layer");
        Ok(Self {
            convolution,
            reconstruct,
            weights,
            uses_f16,
            params: Allocation::new(context, PARAM_SLOT, gpu::Memory::Shared),
            scratch: std::array::from_fn(|_| {
                Allocation::new(
                    context,
                    (STRIDE * STRIDE * groups * 16) as usize,
                    gpu::Memory::Device,
                )
            }),
            output: Allocation::new(context, 16, gpu::Memory::Device),
        })
    }

    /// Caller must complete earlier GPU submissions before calling this method
    /// (buffers may grow). The input upload may precede this in the same encoder.
    /// Record at most once per submission: returned storage is reused next time.
    pub fn encode(
        &mut self,
        context: &gpu::Context,
        encoder: &mut gpu::CommandEncoder,
        source: gpu::BufferPiece,
        source_size: (u32, u32),
        plan: UpscalePlan,
    ) -> Result<EnhancedRegion, String> {
        let (width, height) = plan.validate(source_size)?;
        self.output
            .ensure(context, width as usize * height as usize * 4);
        let model = Model::for_scale(plan.scale);
        let layers = model.layers();
        let weights = self.weights[if plan.scale == 4 { 1 } else { 0 }]
            .buffer
            .into();
        let tiles = tile_params(plan, source_size);
        self.params
            .ensure(context, tiles.len() * layers.len() * PARAM_SLOT);
        for (index, param) in tiles
            .iter()
            .flat_map(|tile| layers.iter().map(|layer| UpscaleParams { layer: *layer, ..*tile }))
            .enumerate()
        {
            let bytes = bytemuck::bytes_of(&param);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.params.buffer.data().add(index * PARAM_SLOT),
                    bytes.len(),
                );
            }
        }
        context.sync_buffer(self.params.buffer);
        for (tile_index, tile) in tiles.iter().enumerate() {
            // The head writes scratch 0, which stays bound as the tail's skip;
            // later layers ping-pong between scratch 1 and 2.
            let (mut features, mut result) = (1, 0);
            let mut param = self.params.buffer.at(0);
            for layer_index in 0..layers.len() {
                param = self
                    .params
                    .buffer
                    .at(((tile_index * layers.len() + layer_index) * PARAM_SLOT) as u64);
                if layer_index > 0 {
                    features = result;
                    result = if features == 1 { 2 } else { 1 };
                }
                let data = ConvData {
                    b_up_params: param,
                    b_up_source: source,
                    b_up_features: self.scratch[features].buffer.into(),
                    b_up_skip: self.scratch[0].buffer.into(),
                    b_up_result: self.scratch[result].buffer.into(),
                    b_up_weights: weights,
                };
                let mut compute = encoder.compute("neural screen tile convolution");
                let mut pipeline = compute.with(&self.convolution);
                pipeline.bind(0, &data);
                pipeline.dispatch([STRIDE.div_ceil(8), STRIDE.div_ceil(8), 1]);
            }
            let data = ReconstructData {
                b_up_params: param,
                b_up_source: source,
                b_up_features: self.scratch[result].buffer.into(),
                b_up_output: self.output.buffer.into(),
            };
            let mut compute = encoder.compute("neural screen tile reconstruction");
            let mut pipeline = compute.with(&self.reconstruct);
            pipeline.bind(0, &data);
            pipeline.dispatch([
                (tile.tile[2] as u32 * plan.scale).div_ceil(8),
                (tile.tile[3] as u32 * plan.scale).div_ceil(8),
                1,
            ]);
        }
        Ok(EnhancedRegion {
            buffer: self.output.buffer.into(),
            width,
            height,
            plan,
        })
    }

    /// Call only after the compositor's final GPU submission has completed.
    pub fn destroy(&mut self, context: &gpu::Context) {
        context.destroy_compute_pipeline(&mut self.convolution);
        context.destroy_compute_pipeline(&mut self.reconstruct);
        for buffer in [&self.params, &self.output]
            .into_iter()
            .chain(self.scratch.iter())
            .chain(self.weights.iter())
        {
            context.destroy_buffer(buffer.buffer);
        }
    }
}

fn tile_params(region: UpscalePlan, source: (u32, u32)) -> Vec<UpscaleParams> {
    let mut result = Vec::new();
    for y in (0..region.height).step_by(TILE as usize) {
        for x in (0..region.width).step_by(TILE as usize) {
            result.push(UpscaleParams {
                sizes: [
                    source.0,
                    source.1,
                    region.width * region.scale,
                    region.height * region.scale,
                ],
                region: [region.x, region.y, region.width, region.height],
                tile: [
                    (region.x + x) as i32 - HALO as i32,
                    (region.y + y) as i32 - HALO as i32,
                    (region.width - x).min(TILE) as i32,
                    (region.height - y).min(TILE) as i32,
                ],
                tile_output: [x, y, STRIDE, region.scale],
                layer: [0; 4],
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn source_pixel(image: &RgbaImage, x: i32, y: i32) -> [f32; 4] {
        image
            .get_pixel(
                x.clamp(0, image.width() as i32 - 1) as u32,
                y.clamp(0, image.height() as i32 - 1) as u32,
            )
            .0
            .map(|x| x as f32 / 255.0)
    }
    fn bilinear(image: &RgbaImage, x: f32, y: f32) -> [f32; 4] {
        let ix = x.floor() as i32;
        let iy = y.floor() as i32;
        let fx = x - x.floor();
        let fy = y - y.floor();
        let a = source_pixel(image, ix, iy);
        let b = source_pixel(image, ix + 1, iy);
        let c = source_pixel(image, ix, iy + 1);
        let d = source_pixel(image, ix + 1, iy + 1);
        std::array::from_fn(|i| {
            ((a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy)) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
        })
    }
    fn byte(x: f32) -> u8 {
        (x.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
    }

    // Whole-frame CPU oracle, intentionally independent of tile addressing,
    // GPU scratch buffers, dispatches and the production ROI plan.
    fn reference(image: &RgbaImage, plan: UpscalePlan) -> RgbaImage {
        let model = Model::for_scale(plan.scale);
        let weights: Vec<f32> = model
            .weights
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let w = image.width() as usize;
        let h = image.height() as usize;
        let rgb: Vec<f32> = (0..h * w * 4)
            .map(|i| {
                let p = source_pixel(image, (i / 4 % w) as i32, (i / 4 / w) as i32);
                if i % 4 == 3 { 0.0 } else { p[i % 4] }
            })
            .collect();
        // Planar-by-group features: pixel-major, `groups` vec4s per pixel.
        let convolve = |input: &[f32], skip: Option<&[f32]>, layer: [u32; 4]| {
            let [offset, inputs, outputs, kind] = layer.map(|v| v as usize);
            let mut out = vec![0.0_f32; w * h * outputs * 4];
            for y in 0..h {
                for x in 0..w {
                    for og in 0..outputs {
                        let mut sum = [0.0_f32; 4];
                        for tap in 0..9 {
                            let sx = (x as i32 + tap as i32 % 3 - 1).clamp(0, w as i32 - 1) as usize;
                            let sy = (y as i32 + tap as i32 / 3 - 1).clamp(0, h as i32 - 1) as usize;
                            for ig in 0..inputs {
                                let at = ((sy * w + sx) * inputs + ig) * 4;
                                let base = (offset + ((og * 9 + tap) * inputs + ig) * 4) * 4;
                                for col in 0..4 {
                                    let value = input[at + col] + skip.map_or(0.0, |s| s[at + col]);
                                    for row in 0..4 {
                                        sum[row] += weights[base + col * 4 + row] * value;
                                    }
                                }
                            }
                        }
                        let bias = (offset + outputs * 9 * inputs * 4 + og) * 4;
                        for row in 0..4 {
                            let v = sum[row] + weights[bias + row];
                            out[((y * w + x) * outputs + og) * 4 + row] =
                                if kind as u32 == TAIL { v } else { v.max(0.0) };
                        }
                    }
                }
            }
            out
        };
        let layers = model.layers();
        let head = convolve(&rgb, None, layers[0]);
        let mut hidden = head.clone();
        for layer in &layers[1..layers.len() - 1] {
            hidden = convolve(&hidden, None, *layer);
        }
        let tail = *layers.last().unwrap();
        let residual = convolve(&hidden, Some(&head), tail);
        let s = plan.scale;
        RgbaImage::from_fn(plan.width * s, plan.height * s, |x, y| {
            let (fx, fy) = (plan.x * s + x, plan.y * s + y);
            let base = bilinear(
                image,
                (fx as f32 + 0.5) / s as f32 - 0.5,
                (fy as f32 + 0.5) / s as f32 - 0.5,
            );
            let pixel = ((fy / s) as usize * w + (fx / s) as usize) * tail[2] as usize * 4;
            let sub = (fy % s * s + fx % s) as usize;
            let r = |c: usize| residual[pixel + c * (s * s) as usize + sub];
            Rgba([
                byte(base[0] + r(0)),
                byte(base[1] + r(1)),
                byte(base[2] + r(2)),
                byte(base[3]),
            ])
        })
    }

    fn fixture() -> RgbaImage {
        RgbaImage::from_fn(145, 35, |x, y| {
            let line = (x % 11 < 2 && y > 4 && y < 29) || y == 8 || y == 24;
            let tone = if line { 65 } else { 165 };
            Rgba([tone, tone + 15, tone + 30, ((x * 3 + y * 7) % 256) as u8])
        })
    }

    #[test]
    fn planner_rejects_invalid_and_excessive_work_and_keeps_tiles_bounded() {
        let base = UpscalePlan {
            x: 10,
            y: 3,
            width: 110,
            height: 29,
            scale: 4,
        };
        assert_eq!(base.validate((145, 35)).unwrap(), (440, 116));
        for bad in [
            UpscalePlan { scale: 3, ..base },
            UpscalePlan { width: 0, ..base },
            UpscalePlan {
                x: u32::MAX,
                ..base
            },
            UpscalePlan {
                width: 3000,
                height: 3000,
                x: 0,
                y: 0,
                scale: 2,
            },
        ] {
            assert!(bad.validate((10000, 10000)).is_err());
        }
        for param in tile_params(base, (145, 35)) {
            assert!(param.tile[2] <= TILE as i32 && param.tile[3] <= TILE as i32);
            assert_eq!(param.tile_output[2..], [STRIDE, 4]);
        }
        for model in &MODELS {
            assert!(model.layers <= HALO, "halo covers every convolution");
        }
        let groups = MODELS.iter().map(Model::max_groups).max().unwrap();
        assert!(
            STRIDE * STRIDE * groups * 16 * 3 < 16 * 1024 * 1024,
            "scratch stays below 16 MiB"
        );
    }

    #[test]
    fn reference_adds_detail_without_neural_alpha() {
        let image = fixture();
        for scale in [2, 4] {
            let plan = UpscalePlan {
                x: 3,
                y: 1,
                width: 139,
                height: 33,
                scale,
            };
            let output = reference(&image, plan);
            let mut changed = 0;
            for (x, y, pixel) in output.enumerate_pixels() {
                let base = bilinear(
                    &image,
                    plan.x as f32 + (x as f32 + 0.5) / scale as f32 - 0.5,
                    plan.y as f32 + (y as f32 + 0.5) / scale as f32 - 0.5,
                )
                .map(byte);
                assert_eq!(pixel.0[3], base[3]);
                changed += usize::from(pixel.0[0].abs_diff(base[0]) > 2);
            }
            assert!(changed > 100, "network must infer detail, not just resize");
        }
    }

    #[test]
    fn neural_shader_validates_without_a_gpu() {
        let mut module = naga::front::wgsl::parse_str(include_str!("screen_upscale.wgsl")).unwrap();
        for (index, (_, variable)) in module.global_variables.iter_mut().enumerate() {
            variable.binding = Some(naga::ResourceBinding {
                group: 0,
                binding: index as u32,
            });
        }
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        assert_eq!(module.entry_points.len(), 2);
        assert_eq!(std::mem::size_of::<UpscaleParams>(), 80);
    }

    #[test]
    fn half_precision_shader_requires_native_feature_and_validates() {
        let source = include_str!("screen_upscale.wgsl").replacen(
            "alias Math = f32;",
            "enable f16;\nalias Math = f16;",
            1,
        );
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        let flags = naga::valid::ValidationFlags::all() ^ naga::valid::ValidationFlags::BINDINGS;
        assert!(
            naga::valid::Validator::new(flags, naga::valid::Capabilities::empty())
                .validate(&module)
                .is_err()
        );
        naga::valid::Validator::new(flags, naga::valid::Capabilities::SHADER_FLOAT16)
            .validate(&module)
            .unwrap();
    }

    /// Explicitly run in isolation: production borrows the existing compositor
    /// context, while this fixture alone creates a context for hardware parity.
    #[test]
    #[ignore = "requires Vulkan; run this test alone with --ignored --test-threads=1"]
    fn hardware_neural_tiles_match_cpu_reference_for_2x_and_4x() {
        let context =
            unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.expect("Vulkan context");
        eprintln!("neural GPU: {:?}", context.device_information());
        let mut upscaler = ScreenUpscaler::new(&context).unwrap();
        eprintln!(
            "neural precision: {}",
            if upscaler.uses_f16 {
                "native f16"
            } else {
                "f32"
            }
        );
        assert_eq!(
            upscaler.uses_f16,
            context.capabilities().shader_f16 && std::env::var_os("LAHZA_UPSCALE_F32").is_none()
        );
        let mut encoder = context.create_command_encoder(gpu::CommandEncoderDesc {
            name: "neural parity",
            buffer_count: 1,
        });
        let source = std::env::var_os("LAHZA_UPSCALE_FIXTURE")
            .map(|path| image::open(path).unwrap().to_rgba8())
            .unwrap_or_else(fixture);
        assert_eq!(
            source.dimensions(),
            (145, 35),
            "optional comparison fixture must be 145×35"
        );
        let upload = Allocation::new(&context, source.as_raw().len(), gpu::Memory::Shared);
        unsafe {
            std::ptr::copy_nonoverlapping(
                source.as_raw().as_ptr(),
                upload.buffer.data(),
                source.as_raw().len(),
            );
        }
        context.sync_buffer(upload.buffer);
        for plan in [
            UpscalePlan {
                x: 0,
                y: 0,
                width: 145,
                height: 35,
                scale: 2,
            },
            UpscalePlan {
                x: 3,
                y: 2,
                width: 139,
                height: 31,
                scale: 4,
            },
            UpscalePlan {
                x: 0,
                y: 0,
                width: 145,
                height: 35,
                scale: 4,
            },
            UpscalePlan {
                x: 140,
                y: 31,
                width: 5,
                height: 4,
                scale: 2,
            },
        ] {
            encoder.start();
            let enhanced = upscaler
                .encode(
                    &context,
                    &mut encoder,
                    upload.buffer.into(),
                    source.dimensions(),
                    plan,
                )
                .unwrap();
            let length = enhanced.width as usize * enhanced.height as usize * 4;
            let readback = Allocation::new(&context, length, gpu::Memory::Upload);
            encoder
                .transfer("read neural parity")
                .copy_buffer_to_buffer(enhanced.buffer, readback.buffer.into(), length as u64);
            let completion = context.submit(&mut encoder);
            assert!(
                context.wait_for(&completion, 5000),
                "neural inference timed out"
            );
            context.sync_buffer(readback.buffer);
            let actual = unsafe { std::slice::from_raw_parts(readback.buffer.data(), length) };
            let expected = reference(&source, plan);
            let maximum = actual
                .iter()
                .zip(expected.as_raw())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                maximum <= 2,
                "{plan:?}: maximum GPU/reference difference {maximum}"
            );
            if let Some(directory) = std::env::var_os("LAHZA_UPSCALE_ARTIFACTS") {
                if plan.x == 0 && plan.y == 0 {
                    let directory = std::path::PathBuf::from(directory);
                    std::fs::create_dir_all(&directory).unwrap();
                    source.save(directory.join("source.png")).unwrap();
                    RgbaImage::from_raw(enhanced.width, enhanced.height, actual.to_vec())
                        .unwrap()
                        .save(directory.join(format!("neural-{}x.png", plan.scale)))
                        .unwrap();
                    RgbaImage::from_fn(enhanced.width, enhanced.height, |x, y| {
                        Rgba(
                            bilinear(
                                &source,
                                (x as f32 + 0.5) / plan.scale as f32 - 0.5,
                                (y as f32 + 0.5) / plan.scale as f32 - 0.5,
                            )
                            .map(byte),
                        )
                    })
                    .save(directory.join(format!("bilinear-{}x.png", plan.scale)))
                    .unwrap();
                }
            }
            for (a, b) in actual
                .chunks_exact(4)
                .zip(expected.as_raw().chunks_exact(4))
            {
                assert!(a[3].abs_diff(b[3]) <= 1);
            }
            eprintln!("neural {plan:?}: maximum reference error {maximum}/255");
            context.destroy_buffer(readback.buffer);
        }
        upscaler.destroy(&context);
        context.destroy_buffer(upload.buffer);
        context.destroy_command_encoder(&mut encoder);
    }

    #[test]
    #[ignore = "isolated GPU timing diagnostic; run alone with --ignored --test-threads=1"]
    fn hardware_neural_upscale_benchmark() {
        let context =
            unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.expect("Vulkan context");
        eprintln!("neural GPU: {:?}", context.device_information());
        let mut upscaler = ScreenUpscaler::new(&context).unwrap();
        let mut encoder = context.create_command_encoder(gpu::CommandEncoderDesc {
            name: "neural benchmark",
            buffer_count: 1,
        });
        let image = RgbaImage::from_fn(1920, 1080, |x, y| {
            Rgba([
                ((x / 7 + y / 11) % 256) as u8,
                (x % 256) as u8,
                (y % 256) as u8,
                255,
            ])
        });
        let upload = Allocation::new(&context, image.as_raw().len(), gpu::Memory::Shared);
        unsafe {
            std::ptr::copy_nonoverlapping(
                image.as_raw().as_ptr(),
                upload.buffer.data(),
                image.as_raw().len(),
            );
        }
        context.sync_buffer(upload.buffer);
        for (width, height, scale) in [(480, 270, 2), (960, 540, 2), (480, 270, 4), (1920, 1080, 2)]
        {
            let plan = UpscalePlan {
                x: 0,
                y: 0,
                width,
                height,
                scale,
            };
            let mut durations = Vec::new();
            for iteration in 0..5 {
                let start = std::time::Instant::now();
                encoder.start();
                upscaler
                    .encode(
                        &context,
                        &mut encoder,
                        upload.buffer.into(),
                        image.dimensions(),
                        plan,
                    )
                    .unwrap();
                let completion = context.submit(&mut encoder);
                assert!(
                    context.wait_for(&completion, 5000),
                    "bounded neural inference timed out"
                );
                if iteration > 0 {
                    durations.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            durations.sort_by(f64::total_cmp);
            eprintln!(
                "neural ROI {width}x{height} {scale}x: median {:.2} ms, max {:.2} ms (CPU encode + GPU completion, excludes source upload/readback)",
                (durations[1] + durations[2]) * 0.5,
                durations[3]
            );
        }
        upscaler.destroy(&context);
        context.destroy_buffer(upload.buffer);
        context.destroy_command_encoder(&mut encoder);
    }
}
