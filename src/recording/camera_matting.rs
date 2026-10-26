//! Local person segmentation with the bundled, Apache-2.0 PPHumanSeg model.
//! Bundled ONNX Runtime CPU is preferred on Linux x86_64; portable Rust
//! inference remains available on other architectures or runtime load failure.
//! The renderer invokes this on its worker, before CPU/GPU composition. No
//! frame is sent over the network and no device is opened by this module.
use super::scene::CameraOverlay;
use image::{
    imageops::{resize, FilterType},
    RgbaImage,
};
use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
    io::Cursor,
    sync::Arc,
};
use tract_onnx::prelude::*;

const SIDE: usize = 192;
const MODEL: &[u8] =
    include_bytes!("../../assets/matting/human_segmentation_pphumanseg_2023mar.onnx");
// Retained in standalone binaries as well as packaged assets.
#[used]
static MODEL_LICENSE: &str = include_str!("../../assets/matting/LICENSE");
type Model = TypedRunnableModel<TypedModel>;

enum Backend {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    Native(ort::session::Session),
    Portable(Model),
}

impl Backend {
    fn load() -> Result<Self, String> {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        match super::camera_matting_runtime::load(MODEL) {
            Ok(session) => return Ok(Self::Native(session)),
            Err(error) => eprintln!(
                "Camera cutout: bundled runtime unavailable ({error}); using portable inference"
            ),
        }
        Self::portable().map(Self::Portable)
    }

    fn portable() -> Result<Model, String> {
        (|| -> TractResult<Model> {
            let mut graph = tract_onnx::onnx()
                .model_for_read(&mut Cursor::new(MODEL))?
                .with_input_fact(0, f32::fact([1, 3, SIDE, SIDE]).into())?;
            let softmax = graph
                .nodes()
                .iter()
                .find(|node| node.name == "p2o.Softmax.0")
                .ok_or_else(|| TractError::msg("Bundled model has no expected person logits"))?;
            let logits = softmax.inputs[0];
            graph.set_output_outlets(&[logits])?;
            let threads = std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(4);
            graph
                .into_optimized()?
                .into_runnable_with_options(&PlanOptions {
                    executor: Some(tract_linalg::multithread::Executor::multithread_with_name(
                        threads,
                        "camera-cutout",
                    )),
                    ..Default::default()
                })
        })()
        .map_err(|error| format!("Could not load camera cutout model: {error:#}"))
    }

    fn infer(&mut self, values: Vec<f32>) -> Result<Vec<f32>, String> {
        match self {
            #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
            Self::Native(session) => {
                let input = ort::value::Tensor::from_array(([1usize, 3, SIDE, SIDE], values))
                    .map_err(|error| error.to_string())?;
                let result = session
                    .run(ort::inputs![input])
                    .map_err(|error| format!("Camera cutout inference: {error}"))?;
                let (shape, classes) = result[0]
                    .try_extract_tensor::<f32>()
                    .map_err(|error| error.to_string())?;
                if shape[..] != [1, 2, SIDE as i64, SIDE as i64] {
                    return Err("Camera cutout model returned an unexpected mask shape".into());
                }
                Ok(classes[SIDE * SIDE..]
                    .iter()
                    .map(|value| value.clamp(0.0, 1.0))
                    .collect())
            }
            Self::Portable(model) => {
                let input = Tensor::from_shape(&[1, 3, SIDE, SIDE], &values)
                    .map_err(|error| error.to_string())?;
                let result = model
                    .run(tvec!(input.into()))
                    .map_err(|error| format!("Camera cutout inference: {error:#}"))?;
                if result.len() != 1 || result[0].shape() != [1, SIDE, SIDE, 2] {
                    return Err("Camera cutout model returned an unexpected mask shape".into());
                }
                let classes = result[0]
                    .as_slice::<f32>()
                    .map_err(|error| error.to_string())?;
                Ok(person_probabilities(classes))
            }
        }
    }
}

struct Matte {
    key: u64,
    a: Vec<f32>,
    b: Vec<f32>,
}

struct RefinedMatte {
    key: (u64, u8, i8),
    coverage: Vec<f32>,
    full: Arc<RgbaImage>,
}

/// Inference plans are created lazily once per renderer. A small source-frame
/// cache avoids re-running inference for paused edits and shutter subframes.
#[derive(Default)]
pub struct CameraMatting {
    model: Option<Result<Backend, String>>,
    mattes: VecDeque<Matte>,
    refined: VecDeque<RefinedMatte>,
    output: Option<((u64, u8, i8, u16), Arc<RgbaImage>)>,
}

impl CameraMatting {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn backend_name(&self) -> &'static str {
        match self.model.as_ref().and_then(|model| model.as_ref().ok()) {
            #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
            Some(Backend::Native(_)) => "ONNX Runtime CPU",
            Some(Backend::Portable(_)) => "Portable Rust CPU",
            None => "Not initialized",
        }
    }

    pub fn process(
        &mut self,
        camera: &RgbaImage,
        overlay: CameraOverlay,
    ) -> Result<Arc<RgbaImage>, String> {
        let mix = (overlay.cutout_strength().clamp(0.0, 1.0) * 65535.0).round() as u16;
        if mix == 0 {
            return Ok(Arc::new(camera.clone()));
        }
        if camera.width() == 0 || camera.height() == 0 {
            return Err("Camera frame is empty".into());
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        camera.dimensions().hash(&mut hasher);
        camera.as_raw().hash(&mut hasher);
        let key = hasher.finish();
        let settings = (
            key,
            overlay.cutout_softness.min(100),
            overlay.cutout_inset.clamp(-30, 30),
            mix,
        );
        if let Some((cached, image)) = &self.output {
            if *cached == settings {
                return Ok(image.clone());
            }
        }
        let refined_key = (key, settings.1, settings.2);
        if !self.refined.iter().any(|matte| matte.key == refined_key) {
            if !self.mattes.iter().any(|matte| matte.key == key) {
                let small = resize(camera, SIDE as u32, SIDE as u32, FilterType::Triangle);
                let mut values = vec![0.0_f32; 3 * SIDE * SIDE];
                let mut guide = Vec::with_capacity(SIDE * SIDE);
                for (i, pixel) in small.pixels().enumerate() {
                    for c in 0..3 {
                        values[c * SIDE * SIDE + i] = pixel[c] as f32 / 127.5 - 1.0;
                    }
                    guide.push(luma(pixel.0));
                }
                if self.model.is_none() {
                    self.model = Some(Backend::load());
                }
                let model = self
                    .model
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .map_err(|error| error.clone())?;
                let foreground = model.infer(values)?;
                let (a, b) = guided_coefficients(&guide, &foreground);
                self.mattes.push_front(Matte { key, a, b });
                while self.mattes.len() > 4 {
                    self.mattes.pop_back();
                }
            }
            let matte = self.mattes.iter().find(|matte| matte.key == key).unwrap();
            let mut output = camera.clone();
            let threshold = 0.5 + settings.2 as f32 / 100.0;
            let feather = 0.025 + settings.1 as f32 * 0.00425;
            let low = (threshold - feather).max(0.0);
            let high = (threshold + feather).min(1.0);
            let axis = |length: u32| -> Vec<(usize, usize, f32)> {
                (0..length)
                    .map(|i| {
                        let coordinate = ((i as f32 + 0.5) * SIDE as f32 / length as f32 - 0.5)
                            .clamp(0.0, (SIDE - 1) as f32);
                        let low = coordinate as usize;
                        (low, (low + 1).min(SIDE - 1), coordinate - low as f32)
                    })
                    .collect()
            };
            let xs = axis(camera.width());
            let ys = axis(camera.height());
            let stride = camera.width() as usize * 4;
            let threads = std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(4);
            let rows = (camera.height() as usize).div_ceil(threads).max(1);
            let mut coverage = vec![0.0_f32; camera.width() as usize * camera.height() as usize];
            let output_bytes: &mut [u8] = output.as_mut();
            std::thread::scope(|scope| {
                for (chunk_index, (chunk, mask)) in output_bytes
                    .chunks_mut(rows * stride)
                    .zip(coverage.chunks_mut(rows * camera.width() as usize))
                    .enumerate()
                {
                    let xs = &xs;
                    let ys = &ys;
                    scope.spawn(move || {
                        for (row, pixels) in chunk.chunks_mut(stride).enumerate() {
                            let (y0, y1, ty) = ys[chunk_index * rows + row];
                            for (x, pixel) in pixels.chunks_exact_mut(4).enumerate() {
                                let (x0, x1, tx) = xs[x];
                                let sample = |values: &[f32]| {
                                    let top = values[y0 * SIDE + x0] * (1.0 - tx)
                                        + values[y0 * SIDE + x1] * tx;
                                    let bottom = values[y1 * SIDE + x0] * (1.0 - tx)
                                        + values[y1 * SIDE + x1] * tx;
                                    top * (1.0 - ty) + bottom * ty
                                };
                                let guide = (pixel[0] as f32 * 0.2126
                                    + pixel[1] as f32 * 0.7152
                                    + pixel[2] as f32 * 0.0722)
                                    / 255.0;
                                let probability =
                                    (sample(&matte.a) * guide + sample(&matte.b)).clamp(0.0, 1.0);
                                let alpha = edge_coverage(probability, low, high);
                                mask[row * xs.len() + x] = alpha;
                                pixel[3] = (pixel[3] as f32 * alpha).round() as u8;
                            }
                        }
                    });
                }
            });
            self.refined.push_front(RefinedMatte {
                key: refined_key,
                coverage,
                full: Arc::new(output),
            });
            while self.refined.len() > 4
                || (self.refined.len() > 1
                    && self
                        .refined
                        .iter()
                        .map(|matte| matte.coverage.len() * 8)
                        .sum::<usize>()
                        > 64 * 1024 * 1024)
            {
                self.refined.pop_back();
            }
        }
        let refined = self
            .refined
            .iter()
            .find(|matte| matte.key == refined_key)
            .unwrap();
        let output = if mix == u16::MAX {
            refined.full.clone()
        } else {
            let mut mixed = camera.clone();
            let strength = mix as f32 / u16::MAX as f32;
            let bytes: &mut [u8] = mixed.as_mut();
            for (pixel, alpha) in bytes.chunks_exact_mut(4).zip(&refined.coverage) {
                pixel[3] = (pixel[3] as f32 * (1.0 - strength + strength * alpha)).round() as u8;
            }
            Arc::new(mixed)
        };
        self.output = Some((settings, output.clone()));
        Ok(output)
    }
}

fn edge_coverage(probability: f32, low: f32, high: f32) -> f32 {
    let t = ((probability - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn person_probabilities(logits: &[f32]) -> Vec<f32> {
    (0..SIDE * SIDE)
        .map(|i| {
            let index = ((i % SIDE) * SIDE + i / SIDE) * 2;
            1.0 / (1.0 + (logits[index] - logits[index + 1]).clamp(-80.0, 80.0).exp())
        })
        .collect()
}

fn luma(pixel: [u8; 4]) -> f32 {
    (pixel[0] as f32 * 0.2126 + pixel[1] as f32 * 0.7152 + pixel[2] as f32 * 0.0722) / 255.0
}

/// Guided enlargement aligns the small neural mask with full-resolution
/// luminance edges. It preserves soft boundaries without a blocky 192px mask.
fn guided_coefficients(guide: &[f32], mask: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mean_i = box_mean(guide, 2);
    let mean_p = box_mean(mask, 2);
    let ii: Vec<_> = guide.iter().map(|x| x * x).collect();
    let ip: Vec<_> = guide.iter().zip(mask).map(|(i, p)| i * p).collect();
    let corr_i = box_mean(&ii, 2);
    let corr_ip = box_mean(&ip, 2);
    let a: Vec<_> = (0..guide.len())
        .map(|i| {
            (corr_ip[i] - mean_i[i] * mean_p[i])
                / ((corr_i[i] - mean_i[i] * mean_i[i]).max(0.0) + 0.01)
        })
        .collect();
    let b: Vec<_> = (0..guide.len())
        .map(|i| mean_p[i] - a[i] * mean_i[i])
        .collect();
    (box_mean(&a, 2), box_mean(&b, 2))
}

fn box_mean(values: &[f32], radius: usize) -> Vec<f32> {
    let stride = SIDE + 1;
    let mut integral = vec![0.0_f64; stride * stride];
    for y in 0..SIDE {
        let mut row = 0.0;
        for x in 0..SIDE {
            row += values[y * SIDE + x] as f64;
            integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row;
        }
    }
    let mut result = vec![0.0; SIDE * SIDE];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (x0, y0, x1, y1) = (
                x.saturating_sub(radius),
                y.saturating_sub(radius),
                (x + radius + 1).min(SIDE),
                (y + radius + 1).min(SIDE),
            );
            result[y * SIDE + x] = ((integral[y1 * stride + x1]
                - integral[y0 * stride + x1]
                - integral[y1 * stride + x0]
                + integral[y0 * stride + x0])
                / ((x1 - x0) * (y1 - y0)) as f64) as f32;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::camera_layout::CameraLayout;

    #[test]
    fn actual_model_rejects_blank_background_and_cache_preserves_source_alpha() {
        let mut matte = CameraMatting::new();
        let blank = RgbaImage::from_pixel(320, 180, image::Rgba([180, 180, 180, 200]));
        let overlay = CameraOverlay {
            layout: CameraLayout::Cutout,
            ..Default::default()
        };
        let output = matte.process(&blank, overlay).unwrap();
        assert!(output.pixels().filter(|p| p[3] < 20).count() > 320 * 180 * 9 / 10);
        assert!(output.pixels().all(|p| p[3] <= 200));
        let again = matte.process(&blank, overlay).unwrap();
        assert!(Arc::ptr_eq(&output, &again));
        assert_eq!(matte.mattes.len(), 1);
    }

    #[test]
    fn portrait_keeps_person_removes_background_and_edges_change_alpha() {
        let source = image::load_from_memory(include_bytes!("../../tests/fixtures/astronaut.png"))
            .unwrap()
            .to_rgba8();
        let mut matte = CameraMatting::new();
        let mut overlay = CameraOverlay {
            layout: CameraLayout::Cutout,
            ..Default::default()
        };
        let output = matte.process(&source, overlay).unwrap();
        eprintln!("Camera cutout backend: {}", matte.backend_name());
        // Named landmarks in the public-domain NASA portrait: face and jacket
        // must remain while the upper-right room/background disappears.
        assert!(output.get_pixel(220, 110)[3] > 230);
        assert!(output.get_pixel(220, 300)[3] > 230);
        assert!(output.get_pixel(450, 40)[3] < 25);
        assert!(output.pixels().any(|p| p[3] > 0 && p[3] < 255));
        let sum = |image: &RgbaImage| image.pixels().map(|p| p[3] as u64).sum::<u64>();
        overlay.cutout_inset = 25;
        let inset = matte.process(&source, overlay).unwrap();
        assert!(sum(&inset) < sum(&output));
        overlay.cutout_inset = -25;
        let expanded = matte.process(&source, overlay).unwrap();
        assert!(sum(&expanded) > sum(&output));
        overlay.cutout_inset = 0;
        overlay.cutout_softness = 100;
        let soft = matte.process(&source, overlay).unwrap();
        assert_ne!(*soft, *output);
        assert_eq!(
            matte.mattes.len(),
            1,
            "editing edge controls reuses neural inference"
        );
        if let Ok(path) = std::env::var("LAHZA_MATTING_ARTIFACT") {
            output.save(path).unwrap();
        }
    }

    #[test]
    fn optimized_logits_match_unmodified_graph_probability_and_orientation() {
        let source = image::load_from_memory(include_bytes!("../../tests/fixtures/astronaut.png"))
            .unwrap()
            .to_rgba8();
        let small = resize(&source, SIDE as u32, SIDE as u32, FilterType::Triangle);
        let mut values = vec![0.0; 3 * SIDE * SIDE];
        for (i, pixel) in small.pixels().enumerate() {
            for channel in 0..3 {
                values[channel * SIDE * SIDE + i] = pixel[channel] as f32 / 127.5 - 1.0;
            }
        }
        let tensor = Tensor::from_shape(&[1, 3, SIDE, SIDE], &values).unwrap();
        let official = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(MODEL))
            .unwrap()
            .with_input_fact(0, f32::fact([1, 3, SIDE, SIDE]).into())
            .unwrap()
            .into_optimized()
            .unwrap()
            .into_runnable()
            .unwrap();
        let output = official.run(tvec!(tensor.clone().into())).unwrap();
        let expected = &output[0].as_slice::<f32>().unwrap()[SIDE * SIDE..];
        let portable = Backend::portable().unwrap();
        let output = portable.run(tvec!(tensor.into())).unwrap();
        let actual = person_probabilities(output[0].as_slice::<f32>().unwrap());
        let maximum = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            maximum < 0.0003,
            "optimized/official model probability difference {maximum}"
        );
    }

    #[test]
    fn extreme_edge_controls_preserve_definite_background_and_person() {
        for softness in [0, 20, 100] {
            for inset in [-30, 0, 30] {
                let threshold = 0.5 + inset as f32 / 100.0;
                let feather = 0.025 + softness as f32 * 0.00425;
                let low = (threshold - feather).max(0.0);
                let high = (threshold + feather).min(1.0);
                assert_eq!(edge_coverage(0.0, low, high), 0.0);
                assert_eq!(edge_coverage(1.0, low, high), 1.0);
                let coverage: Vec<_> = (0..=100)
                    .map(|index| edge_coverage(index as f32 / 100.0, low, high))
                    .collect();
                assert!(coverage.windows(2).all(|pair| pair[0] <= pair[1]));
            }
        }
    }

    #[test]
    fn shutter_mix_reuses_full_resolution_coverage_and_preserves_exact_alpha() {
        let source = image::load_from_memory(include_bytes!("../../tests/fixtures/astronaut.png"))
            .unwrap()
            .to_rgba8();
        let source = resize(&source, 320, 320, FilterType::Triangle);
        let mut matte = CameraMatting::new();
        let mut overlay = CameraOverlay {
            layout: CameraLayout::Cutout,
            ..Default::default()
        };
        let full = matte.process(&source, overlay).unwrap();
        for mix in [0.15, 0.3, 0.5, 0.7, 0.85] {
            overlay.pose = Some(super::super::camera_timeline::CameraPose {
                screen: Default::default(),
                camera: Default::default(),
                radius_fraction: 0.0,
                opacity: 1.0,
                cutout: mix,
                full_bleed: 0.0,
                shadow: 0.0,
            });
            let mixed = matte.process(&source, overlay).unwrap();
            let strength = (mix * 65535.0).round() as f32 / 65535.0;
            assert_eq!(matte.mattes.len(), 1);
            assert_eq!(matte.refined.len(), 1);
            for ((source, pixel), coverage) in source
                .pixels()
                .zip(mixed.pixels())
                .zip(&matte.refined[0].coverage)
            {
                assert_eq!(
                    pixel[3],
                    (source[3] as f32 * (1.0 - strength + strength * coverage)).round() as u8
                );
            }
        }
        overlay.pose = None;
        assert!(
            Arc::ptr_eq(&full, &matte.process(&source, overlay).unwrap()),
            "returning to full cutout reuses the same RGBA allocation"
        );
    }

    #[test]
    fn cutout_zero_keeps_exact_pixels_without_initializing_model() {
        let mut matte = CameraMatting::new();
        let source = RgbaImage::from_pixel(16, 16, image::Rgba([80, 100, 120, 64]));
        assert_eq!(
            *matte.process(&source, CameraOverlay::default()).unwrap(),
            source
        );
        assert!(matte.model.is_none());
    }
}
