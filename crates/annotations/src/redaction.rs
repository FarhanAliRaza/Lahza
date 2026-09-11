//! Pixelation and blur applied to normalized annotation regions.
use crate::{AnnotationMark, Tool};

pub fn apply_redactions(output: &mut image::RgbaImage, marks: &[AnnotationMark]) {
    let width = output.width();
    let height = output.height();
    for mark in marks
        .iter()
        .filter(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
    {
        let left = mark.start.x.min(mark.end.x).clamp(0.0, 1.0);
        let top = mark.start.y.min(mark.end.y).clamp(0.0, 1.0);
        let right = mark.start.x.max(mark.end.x).clamp(0.0, 1.0);
        let bottom = mark.start.y.max(mark.end.y).clamp(0.0, 1.0);
        let x = (left * width as f32).floor() as u32;
        let y = (top * height as f32).floor() as u32;
        let region_width = ((right - left) * width as f32).ceil() as u32;
        let region_height = ((bottom - top) * height as f32).ceil() as u32;
        if region_width == 0 || region_height == 0 {
            continue;
        }
        let region_width = region_width.min(width.saturating_sub(x));
        let region_height = region_height.min(height.saturating_sub(y));
        let crop = image::imageops::crop_imm(output, x, y, region_width, region_height).to_image();
        let processed = if mark.tool == Tool::Pixelate {
            let block = (4.0 + mark.density.clamp(0.0, 1.0) * 36.0).round() as u32;
            let small = image::imageops::resize(
                &crop,
                (region_width / block.max(1)).max(1),
                (region_height / block.max(1)).max(1),
                image::imageops::FilterType::Triangle,
            );
            image::imageops::resize(
                &small,
                region_width,
                region_height,
                image::imageops::FilterType::Nearest,
            )
        } else {
            image::imageops::blur(&crop, 2.0 + mark.density.clamp(0.0, 1.0) * 28.0)
        };
        image::imageops::replace(output, &processed, i64::from(x), i64::from(y));
    }
}
