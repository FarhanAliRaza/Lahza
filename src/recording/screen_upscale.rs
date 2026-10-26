//! Tiled GPU UI reconstruction, independently trained on generated screen/text
//! artwork. A 2× model and a separately trained two-stage 4× cascade use no
//! Screen Studio weights.
//! The compositor owns the context and submission lifetime; this module only
//! records commands. See assets/upscaler/README.md for provenance and differences.
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use gpu::ShaderData as _;

pub const MAX_OUTPUT_PIXELS: u64 = 24_000_000;
pub const MAX_OUTPUT_SIDE: u32 = 8192;
const TILE: u32 = 128;
const HALO: u32 = 4;
const STRIDE: u32 = TILE + HALO * 2;
const PARAM_SLOT: usize = 256;
#[used]
static MODEL_LICENSE: &str = concat!(
    include_str!("../../assets/upscaler/LICENSE"),
    "\nBlade graphics:\n",
    include_str!("../../vendor/blade-graphics/LICENSE")
);
const WEIGHTS: &[u8] = include_bytes!("../../assets/upscaler/anime4k-cnn-x2-s.f32");
const UI_WEIGHTS: [&[u8]; 3] = [
    include_bytes!("../../assets/upscaler/ui-cnn-2x.f32"),
    include_bytes!("../../assets/upscaler/ui-cnn-4x-first.f32"),
    include_bytes!("../../assets/upscaler/ui-cnn-4x.f32"),
];

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
    original: [u32; 4],
    mapping: [u32; 4],
}

#[derive(blade_macros::ShaderData)]
struct ConvData {
    b_up_params: gpu::BufferPiece,
    b_up_source: gpu::BufferPiece,
    b_up_features: gpu::BufferPiece,
    b_up_result: gpu::BufferPiece,
    b_up_weights: gpu::BufferPiece,
}

#[derive(blade_macros::ShaderData)]
struct ReconstructData {
    b_up_params: gpu::BufferPiece,
    b_up_source: gpu::BufferPiece,
    b_up_features: gpu::BufferPiece,
    b_up_alpha: gpu::BufferPiece,
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
    convolutions: [gpu::ComputePipeline; 4],
    reconstruct: gpu::ComputePipeline,
    weights: [Allocation; 3],
    pub uses_f16: bool,
    params: Allocation,
    scratch: [Allocation; 2],
    intermediate: Allocation,
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
        let convolutions = ["conv0", "conv1", "conv2", "conv3"].map(|entry| {
            context.create_compute_pipeline(gpu::ComputePipelineDesc {
                name: "Lahza UI CNN convolution",
                data_layouts: &[&ConvData::layout()],
                compute: shader.at(entry),
            })
        });
        let reconstruct = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "Lahza UI CNN reconstruct",
            data_layouts: &[&ReconstructData::layout()],
            compute: shader.at("reconstruct"),
        });
        let weights = UI_WEIGHTS.map(|coefficients| {
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
        Ok(Self {
            convolutions,
            reconstruct,
            weights,
            uses_f16,
            params: Allocation::new(context, PARAM_SLOT, gpu::Memory::Shared),
            scratch: std::array::from_fn(|_| {
                Allocation::new(
                    context,
                    (STRIDE * STRIDE * 16) as usize,
                    gpu::Memory::Device,
                )
            }),
            intermediate: Allocation::new(context, 16, gpu::Memory::Device),
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
        let passes = passes(plan, source_size);
        let params: Vec<_> = passes
            .iter()
            .enumerate()
            .flat_map(|(index, pass)| {
                tile_params(*pass, source_size)
                    .into_iter()
                    .map(move |mut params| {
                        // Preserve the quantized intermediate exactly enough that
                        // a half-precision rounding does not propagate into stage2.
                        params.mapping[2] = u32::from(plan.scale == 4 && index == 0);
                        params
                    })
            })
            .collect();
        self.params.ensure(context, params.len() * PARAM_SLOT);
        for (index, param) in params.iter().enumerate() {
            let bytes = bytemuck::bytes_of(param);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.params.buffer.data().add(index * PARAM_SLOT),
                    bytes.len(),
                );
            }
        }
        context.sync_buffer(self.params.buffer);
        if plan.scale == 4 {
            self.intermediate.ensure(
                context,
                passes[0].region.width as usize * passes[0].region.height as usize * 16,
            );
        }
        let mut index = 0;
        for (pass_index, pass) in passes.into_iter().enumerate() {
            let input = if pass_index == 0 {
                source
            } else {
                self.intermediate.buffer.into()
            };
            let output = if plan.scale == 4 && pass_index == 0 {
                self.intermediate.buffer
            } else {
                self.output.buffer
            };
            for param in tile_params(pass, source_size) {
                let param_buffer = self.params.buffer.at((index * PARAM_SLOT) as u64);
                index += 1;
                for layer in 0..4 {
                    let data = ConvData {
                        b_up_params: param_buffer,
                        b_up_source: input,
                        b_up_features: self.scratch[layer % 2].buffer.into(),
                        b_up_result: self.scratch[(layer + 1) % 2].buffer.into(),
                        b_up_weights: self.weights
                            [if plan.scale == 4 { pass_index + 1 } else { 0 }]
                        .buffer
                        .into(),
                    };
                    let mut compute = encoder.compute("neural screen tile convolution");
                    let mut pipeline = compute.with(&self.convolutions[layer]);
                    pipeline.bind(0, &data);
                    pipeline.dispatch([STRIDE.div_ceil(8), STRIDE.div_ceil(8), 1]);
                }
                let data = ReconstructData {
                    b_up_params: param_buffer,
                    b_up_source: input,
                    b_up_features: self.scratch[0].buffer.into(),
                    b_up_alpha: source,
                    b_up_output: output.into(),
                };
                let mut compute = encoder.compute("neural screen tile reconstruction");
                let mut pipeline = compute.with(&self.reconstruct);
                pipeline.bind(0, &data);
                pipeline.dispatch([
                    (param.tile[2] as u32 * 2).div_ceil(8),
                    (param.tile[3] as u32 * 2).div_ceil(8),
                    1,
                ]);
            }
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
        for pipeline in &mut self.convolutions {
            context.destroy_compute_pipeline(pipeline);
        }
        context.destroy_compute_pipeline(&mut self.reconstruct);
        for buffer in [
            &self.params,
            &self.scratch[0],
            &self.scratch[1],
            &self.intermediate,
            &self.output,
        ]
        .into_iter()
        .chain(self.weights.iter())
        {
            context.destroy_buffer(buffer.buffer);
        }
    }
}

#[derive(Clone, Copy)]
struct Pass {
    source: (u32, u32),
    region: UpscalePlan,
    original: UpscalePlan,
}
fn passes(plan: UpscalePlan, source: (u32, u32)) -> Vec<Pass> {
    if plan.scale == 2 {
        return vec![Pass {
            source,
            region: plan,
            original: plan,
        }];
    }
    // Two original pixels cover the next network's 4-pixel radius at 2x;
    // one extra pixel covers reconstruction's half-pixel bilinear footprint.
    let x = plan.x.saturating_sub(3);
    let y = plan.y.saturating_sub(3);
    let right = (plan.x + plan.width).saturating_add(3).min(source.0);
    let bottom = (plan.y + plan.height).saturating_add(3).min(source.1);
    let expanded = UpscalePlan {
        x,
        y,
        width: right - x,
        height: bottom - y,
        scale: 2,
    };
    vec![
        Pass {
            source,
            region: expanded,
            original: expanded,
        },
        Pass {
            source: (expanded.width * 2, expanded.height * 2),
            region: UpscalePlan {
                x: (plan.x - x) * 2,
                y: (plan.y - y) * 2,
                width: plan.width * 2,
                height: plan.height * 2,
                scale: 2,
            },
            original: plan,
        },
    ]
}
fn tile_params(pass: Pass, original_size: (u32, u32)) -> Vec<UpscaleParams> {
    let mut result = Vec::new();
    let region = pass.region;
    for y in (0..region.height).step_by(TILE as usize) {
        for x in (0..region.width).step_by(TILE as usize) {
            result.push(UpscaleParams {
                sizes: [
                    pass.source.0,
                    pass.source.1,
                    region.width * 2,
                    region.height * 2,
                ],
                region: [region.x, region.y, region.width, region.height],
                tile: [
                    (region.x + x) as i32 - HALO as i32,
                    (region.y + y) as i32 - HALO as i32,
                    (region.width - x).min(TILE) as i32,
                    (region.height - y).min(TILE) as i32,
                ],
                tile_output: [x, y, STRIDE, 0],
                original: [
                    original_size.0,
                    original_size.1,
                    pass.original.x,
                    pass.original.y,
                ],
                // The second stage has its own coefficients, trained on the
                // first stage's quantized output, and uses the learned residual.
                mapping: [pass.original.scale, 1000, 0, 0],
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn coefficients() -> Vec<f32> {
        WEIGHTS
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    }

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
    fn reference_2x(image: &RgbaImage, strength: f32) -> RgbaImage {
        reference_stage(image, strength, 0)
    }
    fn reference_stage(image: &RgbaImage, strength: f32, stage: usize) -> RgbaImage {
        let w = image.width() as usize;
        let h = image.height() as usize;
        let weights: Vec<f32> = UI_WEIGHTS[stage]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let mut previous = vec![[0.0_f32; 4]; w * h];
        for layer in 0..4 {
            let mut next = vec![[0.0_f32; 4]; w * h];
            for y in 0..h {
                for x in 0..w {
                    let mut sum = [0.0_f32; 4];
                    for sign in 0..if layer == 0 { 1 } else { 2 } {
                        for dx in -1..=1 {
                            for dy in -1..=1 {
                                let sx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                                let sy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                                let value = if layer == 0 {
                                    source_pixel(image, sx as i32, sy as i32)
                                } else {
                                    previous[sy * w + sx]
                                        .map(|v| (if sign == 0 { v } else { -v }).max(0.0))
                                };
                                let tap = sign * 9 + ((dx + 1) * 3 + dy + 1) as usize;
                                let offset = layer * 292 + tap * 16;
                                for row in 0..4 {
                                    let mut product = 0.0;
                                    for col in 0..4 {
                                        product += weights[offset + col * 4 + row] * value[col];
                                    }
                                    sum[row] += product;
                                }
                            }
                        }
                    }
                    for row in 0..4 {
                        sum[row] += weights[layer * 292 + 288 + row];
                    }
                    next[y * w + x] = sum;
                }
            }
            previous = next;
        }
        RgbaImage::from_fn(image.width() * 2, image.height() * 2, |x, y| {
            let base = bilinear(
                image,
                (x as f32 + 0.5) / 2.0 - 0.5,
                (y as f32 + 0.5) / 2.0 - 0.5,
            );
            let residual = previous[y as usize / 2 * w + x as usize / 2]
                [(y % 2 * 2 + x % 2) as usize]
                * strength;
            Rgba([
                byte(base[0] + residual),
                byte(base[1] + residual),
                byte(base[2] + residual),
                byte(base[3]),
            ])
        })
    }
    fn reference(image: &RgbaImage, plan: UpscalePlan) -> RgbaImage {
        let twice = reference_stage(image, 1.0, if plan.scale == 4 { 1 } else { 0 });
        let full = if plan.scale == 4 {
            reference_stage(&twice, 1.0, 2)
        } else {
            twice
        };
        let mut result = image::imageops::crop_imm(
            &full,
            plan.x * plan.scale,
            plan.y * plan.scale,
            plan.width * plan.scale,
            plan.height * plan.scale,
        )
        .to_image();
        for (x, y, pixel) in result.enumerate_pixels_mut() {
            pixel.0[3] = byte(
                bilinear(
                    image,
                    plan.x as f32 + (x as f32 + 0.5) / plan.scale as f32 - 0.5,
                    plan.y as f32 + (y as f32 + 0.5) / plan.scale as f32 - 0.5,
                )[3],
            );
        }
        result
    }

    fn fixture() -> RgbaImage {
        RgbaImage::from_fn(145, 35, |x, y| {
            let line = (x % 11 < 2 && y > 4 && y < 29) || y == 8 || y == 24;
            let tone = if line { 65 } else { 165 };
            Rgba([tone, tone + 15, tone + 30, ((x * 3 + y * 7) % 256) as u8])
        })
    }

    #[test]
    fn public_model_coefficients_match_the_pinned_glsl() {
        let source = include_str!("../../assets/upscaler/anime4k-cnn-x2-s.glsl");
        let mut expected = Vec::new();
        for block in source.split("//!DESC ").skip(1).take(4) {
            let mut layer = Vec::new();
            for line in block.lines().filter(|line| line.contains("mat4(")) {
                let numbers = line
                    .split("mat4(")
                    .nth(1)
                    .unwrap()
                    .split(')')
                    .next()
                    .unwrap();
                layer.extend(numbers.split(',').map(|v| v.trim().parse::<f32>().unwrap()));
            }
            assert!(layer.len() == 144 || layer.len() == 288);
            layer.resize(288, 0.0);
            let bias = block
                .split("result += vec4(")
                .nth(1)
                .unwrap()
                .split(')')
                .next()
                .unwrap();
            layer.extend(bias.split(',').map(|v| v.trim().parse::<f32>().unwrap()));
            expected.extend(layer);
        }
        assert_eq!(expected.len(), 1168);
        assert_eq!(
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            coefficients()
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>()
        );
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
        let p = passes(base, (145, 35));
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].region.x, 7);
        assert_eq!(p[1].region.x, 6);
        for pass in p {
            for param in tile_params(pass, (145, 35)) {
                assert!(param.tile[2] <= TILE as i32 && param.tile[3] <= TILE as i32);
                assert_eq!(param.tile_output[2], STRIDE);
            }
        }
        assert!(
            STRIDE * STRIDE * 16 * 2 < 1024 * 1024,
            "scratch stays below 1 MiB"
        );
    }

    #[test]
    fn reference_adds_luma_detail_without_neural_alpha_or_chroma() {
        let image = fixture();
        let plan = UpscalePlan {
            x: 3,
            y: 1,
            width: 139,
            height: 33,
            scale: 2,
        };
        let output = reference(&image, plan);
        let mut changed = 0;
        for (x, y, pixel) in output.enumerate_pixels() {
            let base = bilinear(
                &image,
                plan.x as f32 + (x as f32 + 0.5) / 2.0 - 0.5,
                plan.y as f32 + (y as f32 + 0.5) / 2.0 - 0.5,
            )
            .map(byte);
            assert_eq!(pixel.0[3], base[3]);
            if pixel.0[..3].iter().all(|v| *v > 0 && *v < 255) {
                assert!(
                    ((pixel.0[1] as i32 - pixel.0[0] as i32) - (base[1] as i32 - base[0] as i32))
                        .abs()
                        <= 1
                );
                assert!(
                    ((pixel.0[2] as i32 - pixel.0[1] as i32) - (base[2] as i32 - base[1] as i32))
                        .abs()
                        <= 1
                );
            }
            changed += usize::from(pixel.0[0].abs_diff(base[0]) > 2);
        }
        assert!(changed > 100, "network must infer detail, not just resize");
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
        assert_eq!(module.entry_points.len(), 5);
        assert_eq!(std::mem::size_of::<UpscaleParams>(), 96);
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
                    if plan.scale == 4 {
                        let twice = reference_2x(&source, 1.0);
                        RgbaImage::from_fn(enhanced.width, enhanced.height, |x, y| {
                            Rgba(
                                bilinear(
                                    &twice,
                                    (x as f32 + 0.5) / 2.0 - 0.5,
                                    (y as f32 + 0.5) / 2.0 - 0.5,
                                )
                                .map(byte),
                            )
                        })
                        .save(directory.join("neural-2x-bilinear-4x.png"))
                        .unwrap();
                    }
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
