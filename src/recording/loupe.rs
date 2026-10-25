//! A local magnifying lens over the composed screen and pointer.
//!
//! The optical model is independently authored: a smooth radial bevel bends
//! the image, disperses RGB at the edge, reflects the surrounding screen, and
//! catches a light above the canvas. CPU and WGSL use the same equations.
#[cfg(test)]
use image::Rgba;
use image::RgbaImage;
use std::sync::OnceLock;

use super::viewport::LoupeFrame;

#[derive(Clone, Copy, Debug)]
pub(crate) struct LoupeGeometry {
    pub center: [f32; 2],
    pub radius: f32,
    pub magnification: f32,
    pub bevel: f32,
    pub chromatic_aberration: f32,
    pub presence: f32,
}

impl LoupeGeometry {
    pub fn new(frame: LoupeFrame, center: (f64, f64), radius: f64) -> Option<Self> {
        let values = [
            center.0,
            center.1,
            radius,
            frame.magnification,
            frame.bevel,
            frame.chromatic_aberration,
            frame.presence,
        ];
        if values.iter().any(|v| !v.is_finite()) || radius <= 0.0 || frame.presence <= 0.0 {
            return None;
        }
        Some(Self {
            center: [center.0 as f32, center.1 as f32],
            radius: radius as f32,
            magnification: frame.magnification.max(1.0) as f32,
            bevel: frame.bevel.clamp(0.0, 0.5) as f32,
            chromatic_aberration: frame.chromatic_aberration.clamp(0.0, 1.0) as f32,
            presence: frame.presence.clamp(0.0, 1.0) as f32,
        })
    }

    pub fn shape(self) -> [f32; 4] {
        [
            self.center[0],
            self.center[1],
            self.radius,
            self.magnification,
        ]
    }

    pub fn optics(self) -> [f32; 4] {
        [self.bevel, self.chromatic_aberration, self.presence, 0.0]
    }
}

#[inline]
fn smooth(a: f32, b: f32, value: f32) -> f32 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Pixel centers use canvas coordinates, shared with the GPU compositor.
#[inline]
pub(crate) fn sample(image: &RgbaImage, p: [f32; 2]) -> [f32; 4] {
    let x = (p[0] - 0.5).clamp(0.0, image.width().saturating_sub(1) as f32);
    let y = (p[1] - 0.5).clamp(0.0, image.height().saturating_sub(1) as f32);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let a = image.get_pixel(x0, y0);
    let b = image.get_pixel(x1, y0);
    let c = image.get_pixel(x0, y1);
    let d = image.get_pixel(x1, y1);
    std::array::from_fn(|i| {
        mix(
            mix(a[i] as f32, b[i] as f32, x - x0 as f32),
            mix(c[i] as f32, d[i] as f32, x - x0 as f32),
            y - y0 as f32,
        )
    })
}

/// Apply after the screen and pointer, before the camera and canvas overlays.
/// Sampling an immutable screen makes the result independent of traversal.
#[cfg(test)]
pub(crate) fn paint(image: &mut RgbaImage, lens: LoupeGeometry) {
    let screen = image.clone();
    paint_with_sampler(image, lens, &|point| sample(&screen, point));
}

/// Shade the lens from continuous canvas coordinates. A scene can reproject
/// these coordinates directly into its original media instead of magnifying
/// pixels that were already reduced to preview resolution.
pub(crate) fn paint_with_sampler<S>(image: &mut RgbaImage, lens: LoupeGeometry, sample: &S)
where
    S: Fn([f32; 2]) -> [f32; 4] + Sync,
{
    if image.width() == 0 || image.height() == 0 || lens.presence <= 0.0 {
        return;
    }
    let prepared = PreparedLens::new(lens, image.width(), image.height());
    let reach = lens.radius * 1.4 + 2.0;
    let x0 = (lens.center[0] - reach).floor().max(0.0) as u32;
    let y0 = (lens.center[1] - reach).floor().max(0.0) as u32;
    let x1 = (lens.center[0] + reach)
        .ceil()
        .clamp(0.0, image.width() as f32) as u32;
    let y1 = (lens.center[1] + reach)
        .ceil()
        .clamp(0.0, image.height() as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Large previews used to shade synchronously on one core. Partition
    // disjoint rows, keeping worker count bounded and small frames serial.
    // No worker owns mutable screen input, so ordering cannot affect pixels.
    let workers = worker_count((x1 - x0) as usize * (y1 - y0) as usize);
    let stride = image.width() as usize * 4;
    let raw: &mut [u8] = image.as_mut();
    let rows = &mut raw[y0 as usize * stride..y1 as usize * stride];
    if workers == 1 {
        paint_rows(rows, stride, y0, x0, x1, sample, &prepared);
    } else {
        let chunk_rows =
            (y1 - y0) as usize / workers + usize::from((y1 - y0) as usize % workers != 0);
        std::thread::scope(|scope| {
            for (chunk, rows) in rows.chunks_mut(chunk_rows * stride).enumerate() {
                let prepared = &prepared;
                scope.spawn(move || {
                    paint_rows(
                        rows,
                        stride,
                        y0 + (chunk * chunk_rows) as u32,
                        x0,
                        x1,
                        sample,
                        prepared,
                    )
                });
            }
        });
    }
}

fn worker_count(pixels: usize) -> usize {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    let available = *WORKERS.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(1, |cores| cores.get().saturating_sub(1).clamp(1, 4))
    });
    available.min((pixels / 60_000).max(1))
}

fn paint_rows<S: Fn([f32; 2]) -> [f32; 4] + Sync>(
    rows: &mut [u8],
    stride: usize,
    y0: u32,
    x0: u32,
    x1: u32,
    sample: &S,
    prepared: &PreparedLens,
) {
    for (row_index, row) in rows.chunks_exact_mut(stride).enumerate() {
        let y = y0 + row_index as u32;
        for x in x0..x1 {
            let offset = x as usize * 4;
            let base = [
                row[offset] as f32,
                row[offset + 1] as f32,
                row[offset + 2] as f32,
                row[offset + 3] as f32,
            ];
            let color = shade(sample, prepared, [x as f32 + 0.5, y as f32 + 0.5], base);
            row[offset..offset + 4]
                .copy_from_slice(&color.map(|c| c.clamp(0.0, 255.0).round() as u8));
        }
    }
}

/// Frame-constant lighting and radius terms belong outside the pixel loop.
struct PreparedLens {
    lens: LoupeGeometry,
    radius: f32,
    inner_radius: f32,
    light: [f32; 2],
    shadow_offset: [f32; 2],
    inverse_zoom: f32,
}

impl PreparedLens {
    fn new(lens: LoupeGeometry, width: u32, height: u32) -> Self {
        let radius = lens.radius.max(1.0);
        let light = [
            width as f32 * 0.5 - lens.center[0],
            -(height as f32) * 0.5 - lens.center[1],
        ];
        let length = light[0].hypot(light[1]).max(0.001);
        let light = [light[0] / length, light[1] / length];
        Self {
            lens,
            radius,
            inner_radius: ((1.0 - lens.bevel.max(0.001)).min(0.965) * radius).min(radius - 0.5),
            light,
            shadow_offset: [light[0] * radius * 0.09, light[1] * radius * 0.09],
            inverse_zoom: 1.0 / lens.magnification,
        }
    }

    #[inline]
    fn shadow(&self, delta: [f32; 2]) -> f32 {
        let x = delta[0] + self.shadow_offset[0];
        let y = delta[1] + self.shadow_offset[1];
        let r = (x * x + y * y).sqrt() / self.radius;
        (1.0 - smooth(0.93, 1.28, r)) * 0.35 * self.lens.presence
    }
}

#[inline]
fn shade<S: Fn([f32; 2]) -> [f32; 4]>(
    sample: &S,
    prepared: &PreparedLens,
    p: [f32; 2],
    base: [f32; 4],
) -> [f32; 4] {
    let lens = prepared.lens;
    let delta = [p[0] - lens.center[0], p[1] - lens.center[1]];
    let distance_squared = delta[0] * delta[0] + delta[1] * delta[1];
    let radius = prepared.radius;
    // The furthest contribution is the shadow: 1.28 radii plus its .09 offset.
    if distance_squared > (radius * 1.37 + 0.5).powi(2) {
        return base;
    }
    // Away from the bevel and rim all optical coefficients are exactly zero.
    // Use one bilinear read here rather than three identical RGB reads and an
    // invisible reflection. This is most of the magnified disk.
    if prepared.inner_radius >= 0.0
        && distance_squared <= prepared.inner_radius * prepared.inner_radius
    {
        let q = [
            lens.center[0] + delta[0] * prepared.inverse_zoom,
            lens.center[1] + delta[1] * prepared.inverse_zoom,
        ];
        let glass = sample(q);
        if lens.presence >= 1.0 {
            return glass;
        }
        let shadow = prepared.shadow(delta);
        let outside = [
            base[0] * (1.0 - shadow),
            base[1] * (1.0 - shadow),
            base[2] * (1.0 - shadow),
            base[3],
        ];
        return std::array::from_fn(|i| mix(outside[i], glass[i], lens.presence));
    }
    let distance = distance_squared.sqrt();
    let r = distance / radius;
    let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
    let shadow = if coverage < 1.0 || lens.presence < 1.0 {
        prepared.shadow(delta)
    } else {
        0.0
    };
    let mut outside = [
        base[0] * (1.0 - shadow),
        base[1] * (1.0 - shadow),
        base[2] * (1.0 - shadow),
        base[3],
    ];
    // Beyond the outer shine there is only the soft shadow: no normal,
    // trigonometry, source resampling, or reflection is needed.
    if coverage <= 0.0 && (r >= 1.12 || lens.bevel == 0.0) {
        return outside;
    }
    let normal = [
        delta[0] / distance.max(0.001),
        delta[1] / distance.max(0.001),
    ];
    let lighting = normal[0] * prepared.light[0] + normal[1] * prepared.light[1];
    let sheen = (1.0 - smooth(1.0, 1.12, r))
        * smooth(0.98, 1.01, r)
        * (-lighting).max(0.0).powi(3)
        * lens.bevel
        * lens.presence
        * 0.7;
    let spectrum_phase = (r * 2.0 + lighting) * std::f32::consts::TAU;
    let spectrum = [
        spectrum_phase.cos(),
        (spectrum_phase - 2.0943952).cos(),
        (spectrum_phase + 2.0943952).cos(),
    ];
    let tint: [f32; 3] = std::array::from_fn(|i| {
        255.0 * (1.0 - lens.chromatic_aberration * 0.35 * (0.5 + spectrum[i] * 0.5))
    });
    if sheen > 0.0 {
        for i in 0..3 {
            outside[i] = mix(outside[i], tint[i], sheen);
        }
    }
    if coverage <= 0.0 {
        return outside;
    }

    let bevel = lens.bevel;
    let shell = smooth(1.0 - bevel.max(0.001), 1.0, r);
    let bend = (shell * std::f32::consts::PI).sin() * bevel * 0.22;
    let scale = prepared.inverse_zoom - bend;
    let q = [
        lens.center[0] + delta[0] * scale,
        lens.center[1] + delta[1] * scale,
    ];
    let dispersion = lens.chromatic_aberration * bevel * radius * 0.055 * shell;
    let mut glass = sample(q);
    if dispersion != 0.0 {
        glass[0] = sample([q[0] + normal[0] * dispersion, q[1] + normal[1] * dispersion])[0];
        glass[2] = sample([q[0] - normal[0] * dispersion, q[1] - normal[1] * dispersion])[2];
    }
    // The rim reflects a compressed annulus just outside the lens.
    let reflection_mix = shell * bevel * 0.8;
    if reflection_mix > 0.0 {
        let reflected_radius = radius * (1.0 + (1.0 - r) * 0.65);
        let reflection = sample([
            lens.center[0] + normal[0] * reflected_radius,
            lens.center[1] + normal[1] * reflected_radius,
        ]);
        for i in 0..3 {
            glass[i] = mix(glass[i], reflection[i], reflection_mix);
        }
    }
    let rim = smooth(0.965, 0.997, r) * (1.0 - smooth(0.997, 1.006, r));
    let glint = lighting.max(0.0).powi(12) + (-lighting).max(0.0).powi(20) * 0.45;
    let highlight = (shell * glint * 0.24 + rim * (0.12 + glint * 0.62)) * (bevel * 5.0).min(1.0);
    let inner_shade = shell * bevel * 0.24 * (1.0 - lighting.max(0.0));
    for i in 0..3 {
        glass[i] *= 1.0 - inner_shade;
        glass[i] = mix(glass[i], tint[i], highlight.clamp(0.0, 1.0));
    }
    std::array::from_fn(|i| mix(outside[i], glass[i], coverage * lens.presence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::model::NormalizedPoint;

    fn lens() -> LoupeGeometry {
        LoupeGeometry::new(
            LoupeFrame {
                center: NormalizedPoint { x: 0.5, y: 0.5 },
                radius: 0.35,
                magnification: 2.0,
                bevel: 0.2,
                chromatic_aberration: 0.5,
                presence: 1.0,
            },
            (64.5, 64.5),
            40.0,
        )
        .unwrap()
    }

    fn pattern() -> RgbaImage {
        RgbaImage::from_fn(129, 129, |x, y| {
            Rgba([
                x as u8,
                y as u8,
                if (x / 4 + y / 4) % 2 == 0 { 240 } else { 20 },
                255,
            ])
        })
    }

    fn detailed_pattern(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                ((x * 3 + y * 7) % 256) as u8,
                ((x * 13 + y * 5) % 256) as u8,
                ((x * 7 + y * 11) % 256) as u8,
                255,
            ])
        })
    }

    #[test]
    fn optimized_paths_preserve_reference_optics() {
        // Captured from the original scalar renderer before fast paths.
        // Include interior, refractive rim, reflected edge, shine, and shadow
        // at full and half presence; allow only float-rounding differences.
        let positions = [
            (70, 48),
            (90, 48),
            (108, 48),
            (114, 48),
            (119, 48),
            (123, 48),
            (26, 48),
            (29, 48),
            (33, 48),
            (70, 4),
            (70, 8),
            (70, 12),
            (70, 88),
            (70, 92),
            (110, 75),
            (104, 79),
            (32, 20),
            (38, 16),
            (110, 20),
            (16, 0),
        ];
        let reference = [
            [
                [32, 124, 247, 255],
                [62, 72, 61, 255],
                [88, 106, 116, 255],
                [113, 161, 131, 255],
                [146, 203, 65, 255],
                [180, 44, 102, 255],
                [202, 93, 113, 255],
                [217, 122, 118, 255],
                [232, 143, 119, 255],
                [166, 98, 78, 255],
                [167, 57, 70, 255],
                [162, 34, 49, 255],
                [156, 204, 192, 255],
                [162, 192, 214, 255],
                [64, 11, 44, 255],
                [72, 143, 49, 255],
                [195, 3, 155, 255],
                [163, 73, 175, 255],
                [186, 217, 193, 255],
                [48, 208, 112, 255],
            ],
            [
                [30, 114, 227, 255],
                [70, 89, 86, 255],
                [105, 98, 60, 255],
                [126, 158, 85, 255],
                [164, 227, 73, 255],
                [187, 45, 105, 255],
                [167, 74, 139, 255],
                [177, 104, 150, 255],
                [190, 136, 161, 255],
                [188, 120, 49, 255],
                [88, 104, 62, 255],
                [97, 100, 70, 255],
                [102, 131, 169, 255],
                [119, 136, 203, 255],
                [76, 12, 51, 255],
                [84, 176, 55, 255],
                [215, 4, 172, 255],
                [188, 66, 176, 255],
                [200, 233, 207, 255],
                [48, 208, 112, 255],
            ],
        ];
        for (presence, expected) in [1.0, 0.5].into_iter().zip(reference) {
            let mut image = detailed_pattern(128, 96);
            paint(
                &mut image,
                LoupeGeometry {
                    center: [70.4, 48.0],
                    radius: 44.8,
                    presence,
                    ..lens()
                },
            );
            for ((x, y), rgba) in positions.into_iter().zip(expected) {
                assert!(
                    image
                        .get_pixel(x, y)
                        .0
                        .into_iter()
                        .zip(rgba)
                        .all(|(a, b)| a.abs_diff(b) <= 1),
                    "pixel ({x},{y}) at presence {presence}"
                );
            }
        }
    }

    #[test]
    fn parallel_rows_match_serial_rendering_for_edges_and_fades() {
        assert_eq!(worker_count(16_000), 1);
        assert!(worker_count(2_000_000) <= 4);
        let source = detailed_pattern(640, 360);
        for (center, presence, bevel) in [
            ([340.0, 180.0], 1.0, 0.2),
            ([340.0, 180.0], 0.4, 0.5),
            ([5.0, 350.0], 0.7, 0.0),
        ] {
            let lens = LoupeGeometry {
                center,
                radius: 224.0,
                presence,
                bevel,
                ..lens()
            };
            let mut parallel = source.clone();
            paint(&mut parallel, lens);
            let mut serial = source.clone();
            let prepared = PreparedLens::new(lens, 640, 360);
            paint_rows(
                serial.as_mut(),
                640 * 4,
                0,
                0,
                640,
                &|point| sample(&source, point),
                &prepared,
            );
            assert_eq!(parallel, serial);
        }
    }

    #[test]
    #[ignore = "manual lens-only CPU benchmark; prints median milliseconds"]
    fn benchmark_loupe_preview_sizes() {
        for (width, height) in [(640, 360), (960, 540), (1280, 720), (1920, 1080)] {
            let source = detailed_pattern(width, height);
            for presence in [1.0, 0.5] {
                let lens = LoupeGeometry {
                    center: [width as f32 * 0.55, height as f32 * 0.5],
                    radius: width as f32 * 0.35,
                    presence,
                    ..lens()
                };
                let mut times = Vec::new();
                for _ in 0..9 {
                    let mut image = source.clone();
                    let started = std::time::Instant::now();
                    paint(&mut image, lens);
                    times.push(started.elapsed().as_secs_f64() * 1000.0);
                    std::hint::black_box(image);
                }
                times.sort_by(f64::total_cmp);
                eprintln!(
                    "loupe {width}x{height} presence={presence}: median={:.2}ms",
                    times[4]
                );
            }
        }
    }

    #[test]
    fn center_magnifies_and_far_pixels_stay_unchanged() {
        let mut image = pattern();
        let original = image.clone();
        paint(&mut image, lens());
        assert_eq!(image.get_pixel(64, 64), original.get_pixel(64, 64));
        assert_eq!(image.get_pixel(84, 64), original.get_pixel(74, 64));
        assert_eq!(image.get_pixel(0, 0), original.get_pixel(0, 0));
        assert_eq!(image.get_pixel(128, 128), original.get_pixel(128, 128));
    }

    #[test]
    fn glass_has_refraction_dispersion_highlight_and_shadow() {
        let original = pattern();
        let mut glass = original.clone();
        paint(&mut glass, lens());
        let mut plain = original.clone();
        paint(
            &mut plain,
            LoupeGeometry {
                bevel: 0.0,
                chromatic_aberration: 0.0,
                ..lens()
            },
        );
        assert_ne!(glass.get_pixel(101, 64), plain.get_pixel(101, 64));
        let mut achromatic = original.clone();
        paint(
            &mut achromatic,
            LoupeGeometry {
                chromatic_aberration: 0.0,
                ..lens()
            },
        );
        assert!(glass.pixels().zip(achromatic.pixels()).any(|(a, b)| a != b));
        assert!(glass.get_pixel(64, 107)[0] < original.get_pixel(64, 107)[0]);
        assert!(glass.get_pixel(64, 24)[0] > plain.get_pixel(64, 24)[0]);
    }

    #[test]
    fn zero_presence_is_identity_and_edge_lenses_are_safe() {
        let mut image = pattern();
        let original = image.clone();
        paint(
            &mut image,
            LoupeGeometry {
                presence: 0.0,
                ..lens()
            },
        );
        assert_eq!(image, original);
        paint(
            &mut image,
            LoupeGeometry {
                center: [0.0, 0.0],
                ..lens()
            },
        );
        assert_eq!(image.dimensions(), (129, 129));
        assert!(image.pixels().all(|p| p[3] == 255));
        assert!(LoupeGeometry::new(
            LoupeFrame {
                center: NormalizedPoint { x: 0.0, y: 0.0 },
                radius: 0.35,
                magnification: 2.0,
                bevel: 0.2,
                chromatic_aberration: 0.5,
                presence: 1.0
            },
            (f64::NAN, 0.0),
            40.0
        )
        .is_none());
    }
}
