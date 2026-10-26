//! Alpha-aware mip reconstruction and bounded four-tap anisotropic filtering.
//! Pixel footprints are measured in original source pixels, not canvas pixels.
use image::{Rgba, RgbaImage};
pub const LEVELS: usize = 16;
pub fn layout(width: u32, height: u32) -> ([[u32; 4]; LEVELS], usize) {
    let mut levels = [[0; 4]; LEVELS];
    levels[0] = [width.max(1), height.max(1), 0, 0];
    let mut count = 0usize;
    for level in 1..LEVELS {
        let previous = levels[level - 1];
        if previous[0] == 1 && previous[1] == 1 {
            break;
        }
        let (w, h) = (previous[0].div_ceil(2), previous[1].div_ceil(2));
        levels[level] = [w, h, count as u32, 0];
        count += w as usize * h as usize;
    }
    (levels, count)
}

pub struct Pyramid {
    levels: Vec<RgbaImage>,
}
impl Pyramid {
    pub fn new(source: &RgbaImage) -> Self {
        Self::cropped(source, (0.0, 0.0, 1.0, 1.0))
    }
    pub fn cropped(source: &RgbaImage, bounds: (f64, f64, f64, f64)) -> Self {
        let crop = [
            (bounds.0 * source.width() as f64 - 0.5).round().max(0.0) as u32,
            (bounds.1 * source.height() as f64 - 0.5).round().max(0.0) as u32,
            (bounds.2 * source.width() as f64 - 0.5).round().max(0.0) as u32,
            (bounds.3 * source.height() as f64 - 0.5).round().max(0.0) as u32,
        ];
        let mut levels = Vec::new();
        if source.width() == 0 || source.height() == 0 {
            return Self { levels };
        }
        loop {
            let previous = levels.last().unwrap_or(source);
            if previous.width() == 1 && previous.height() == 1 || levels.len() == LEVELS - 1 {
                break;
            }
            let next = RgbaImage::from_fn(
                previous.width().div_ceil(2),
                previous.height().div_ceil(2),
                |x, y| {
                    let mut sum = [0u32; 4];
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let mut sx = (x * 2 + dx).min(previous.width() - 1);
                            let mut sy = (y * 2 + dy).min(previous.height() - 1);
                            if levels.is_empty() {
                                sx = sx.clamp(
                                    crop[0].min(previous.width() - 1),
                                    crop[2].min(previous.width() - 1),
                                );
                                sy = sy.clamp(
                                    crop[1].min(previous.height() - 1),
                                    crop[3].min(previous.height() - 1),
                                );
                            }
                            let pixel = previous.get_pixel(sx, sy).0;
                            for c in 0..3 {
                                sum[c] += u32::from(pixel[c]) * u32::from(pixel[3]);
                            }
                            sum[3] += u32::from(pixel[3]);
                        }
                    }
                    Rgba([
                        ((sum[0] + sum[3] / 2) / sum[3].max(1)) as u8,
                        ((sum[1] + sum[3] / 2) / sum[3].max(1)) as u8,
                        ((sum[2] + sum[3] / 2) / sum[3].max(1)) as u8,
                        ((sum[3] + 2) / 4) as u8,
                    ])
                },
            );
            levels.push(next);
        }
        Self { levels }
    }
    pub fn sample(
        &self,
        source: &RgbaImage,
        uv: (f64, f64),
        dx: (f64, f64),
        dy: (f64, f64),
        bounds: (f64, f64, f64, f64),
    ) -> Option<[u8; 4]> {
        // Eigenvalues of J*J^T give the footprint ellipse, including shear.
        let a = dx.0 * dx.0 + dy.0 * dy.0;
        let b = dx.0 * dx.1 + dy.0 * dy.1;
        let c = dx.1 * dx.1 + dy.1 * dy.1;
        let discriminant = ((a - c) * (a - c) + 4.0 * b * b).sqrt();
        let eigen = (a + c + discriminant) * 0.5;
        let major = eigen.max(0.0).sqrt();
        let minor = ((a + c - discriminant) * 0.5).max(0.0).sqrt();
        let axis = if a >= c {
            (eigen - c, b)
        } else {
            (b, eigen - a)
        };
        let length = axis.0.hypot(axis.1);
        let vector = if length > 1e-12 {
            (axis.0 * major / length, axis.1 * major / length)
        } else {
            (major, 0.0)
        };
        if major <= 1.05 || self.levels.is_empty() {
            return None;
        }
        let footprint = minor.max(major / 4.0).max(1.0);
        let lod = footprint.log2().clamp(0.0, self.levels.len() as f64);
        let low = lod.floor() as usize;
        let high = (low + 1).min(self.levels.len());
        let taps = (major / footprint - 1e-4).ceil().clamp(1.0, 4.0) as usize;
        let mut result = [0.0; 4];
        for i in 0..taps {
            let offset = (i as f64 + 0.5) / taps as f64 - 0.5;
            let uv = (
                (uv.0 + vector.0 * offset / source.width() as f64).clamp(bounds.0, bounds.2),
                (uv.1 + vector.1 * offset / source.height() as f64).clamp(bounds.1, bounds.3),
            );
            let a = sample_level(
                if low == 0 {
                    source
                } else {
                    &self.levels[low - 1]
                },
                uv,
            );
            let b = sample_level(
                if high == 0 {
                    source
                } else {
                    &self.levels[high - 1]
                },
                uv,
            );
            for c in 0..4 {
                result[c] += (a[c] + (b[c] - a[c]) * (lod - low as f64)) / taps as f64;
            }
        }
        Some(result.map(|v| v.round().clamp(0.0, 255.0) as u8))
    }
}
fn sample_level(image: &RgbaImage, uv: (f64, f64)) -> [f64; 4] {
    let x = (uv.0 * image.width() as f64 - 0.5).clamp(0.0, (image.width() - 1) as f64);
    let y = (uv.1 * image.height() as f64 - 0.5).clamp(0.0, (image.height() - 1) as f64);
    let (ix, iy) = (x.floor() as u32, y.floor() as u32);
    let (fx, fy) = (x - ix as f64, y - iy as f64);
    std::array::from_fn(|c| {
        let a = image.get_pixel(ix, iy)[c] as f64;
        let b = image.get_pixel((ix + 1).min(image.width() - 1), iy)[c] as f64;
        let d = image.get_pixel(ix, (iy + 1).min(image.height() - 1))[c] as f64;
        let e = image.get_pixel(
            (ix + 1).min(image.width() - 1),
            (iy + 1).min(image.height() - 1),
        )[c] as f64;
        (a + (b - a) * fx) * (1.0 - fy) + (d + (e - d) * fx) * fy
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn striped_minification_reconstructs_energy_without_aliasing() {
        let image = RgbaImage::from_fn(128, 64, |x, _| {
            Rgba([if x % 2 == 0 { 0 } else { 255 }, 20, 40, 255])
        });
        let pyramid = Pyramid::new(&image);
        for x in 0..64 {
            assert_eq!(
                pyramid
                    .sample(
                        &image,
                        (x as f64 / 64.0, 0.5),
                        (4.0, 0.0),
                        (0.0, 4.0),
                        (0.0, 0.0, 1.0, 1.0)
                    )
                    .unwrap()[0],
                128
            );
        }
    }
    #[test]
    fn isotropic_sampling_is_stable_under_float_roundoff() {
        let source = RgbaImage::from_fn(96, 64, |x, _| {
            Rgba([if x % 3 == 0 { 255 } else { 0 }, 20, 40, 255])
        });
        let pyramid = Pyramid::new(&source);
        for x in 1..90 {
            let uv = (x as f64 / 96.0, 0.5);
            let a = pyramid
                .sample(&source, uv, (2.0, 0.0), (0.0, 2.0), (0.0, 0.0, 1.0, 1.0))
                .unwrap();
            let b = pyramid
                .sample(
                    &source,
                    uv,
                    (2.0000001, 0.0),
                    (0.0, 2.0),
                    (0.0, 0.0, 1.0, 1.0),
                )
                .unwrap();
            assert!(
                a[0].abs_diff(b[0]) <= 1,
                "tiny projection roundoff must not change the sampling kernel: {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn transparent_edges_do_not_add_black_to_color() {
        let image = RgbaImage::from_fn(8, 8, |x, _| {
            if x % 2 == 0 {
                Rgba([240, 80, 20, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        assert_eq!(
            Pyramid::new(&image).sample(
                &image,
                (0.5, 0.5),
                (4.0, 0.0),
                (0.0, 4.0),
                (0.0, 0.0, 1.0, 1.0)
            ),
            Some([240, 80, 20, 128])
        );
    }
}
