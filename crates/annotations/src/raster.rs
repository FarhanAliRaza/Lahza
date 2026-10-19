//! Decode immutable image assets once; animate their transform and opacity.
use crate::AnnotationMark;
use base64::Engine;
use resvg::tiny_skia::{FilterQuality, Mask, Paint, Pattern, Pixmap, Rect, SpreadMode, Transform};
use std::{cell::RefCell, collections::VecDeque, sync::Arc};

#[derive(Default)]
struct ImageCache {
    entries: VecDeque<(Arc<str>, Arc<Pixmap>)>,
    bytes: usize,
}

impl ImageCache {
    fn get(&mut self, data: &Arc<str>) -> Option<Arc<Pixmap>> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|(key, _)| Arc::ptr_eq(key, data) || key == data)
        {
            let entry = self.entries.remove(index)?;
            let pixels = entry.1.clone();
            self.entries.push_back(entry);
            return Some(pixels);
        }
        let png = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .ok()?;
        let pixels = Arc::new(Pixmap::decode_png(&png).ok()?);
        let bytes = data.len() + pixels.data().len();
        while self.bytes + bytes > 64 * 1024 * 1024 {
            let Some((key, old)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= key.len() + old.data().len();
        }
        self.bytes += bytes;
        self.entries.push_back((data.clone(), pixels.clone()));
        Some(pixels)
    }
}

thread_local! {
    static IMAGES: RefCell<ImageCache> = RefCell::new(ImageCache::default());
}

pub(crate) fn paint(mark: &AnnotationMark, output: &mut Pixmap) {
    let Some(data) = &mark.image_png else {
        return;
    };
    let Some(pixels) = IMAGES.with(|cache| cache.borrow_mut().get(data)) else {
        return;
    };
    let left = mark.start.x.min(mark.end.x) * output.width() as f32;
    let top = mark.start.y.min(mark.end.y) * output.height() as f32;
    let width = (mark.end.x - mark.start.x).abs() * output.width() as f32;
    let height = (mark.end.y - mark.start.y).abs() * output.height() as f32;
    if ![left, top, width, height, mark.opacity]
        .iter()
        .all(|v| v.is_finite())
        || width <= 0.0
        || height <= 0.0
    {
        return;
    }
    let transform = Transform::from_row(
        width / pixels.width() as f32,
        0.0,
        0.0,
        height / pixels.height() as f32,
        left,
        top,
    );
    let paint = Paint {
        shader: Pattern::new(
            pixels.as_ref().as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            mark.opacity.clamp(0.0, 1.0),
            transform,
        ),
        anti_alias: false,
        force_hq_pipeline: true,
        ..Default::default()
    };
    let right = left + width;
    let bottom = top + height;
    let x0 = left.floor().clamp(0.0, output.width() as f32) as u32;
    let y0 = top.floor().clamp(0.0, output.height() as f32) as u32;
    let x1 = right.ceil().clamp(0.0, output.width() as f32) as u32;
    let y1 = bottom.ceil().clamp(0.0, output.height() as f32) as u32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // Analytic pixel coverage avoids both GPUI's whole-pixel snapping and
    // the vector rasterizer's coarser edge sampling during slow motion.
    let Some(mut mask) = Mask::new(output.width(), output.height()) else {
        return;
    };
    for y in y0..y1 {
        let vertical = ((y + 1) as f32).min(bottom) - (y as f32).max(top);
        for x in x0..x1 {
            let horizontal = ((x + 1) as f32).min(right) - (x as f32).max(left);
            mask.data_mut()[(y * output.width() + x) as usize] =
                (horizontal * vertical * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    output.fill_rect(
        Rect::from_xywh(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32).unwrap(),
        &paint,
        Transform::identity(),
        Some(&mask),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_frames_reuse_decoded_pixels() {
        let pixels = Pixmap::new(32, 32).unwrap();
        let data: Arc<str> = base64::engine::general_purpose::STANDARD
            .encode(pixels.encode_png().unwrap())
            .into();
        let mut cache = ImageCache::default();
        let decoded = cache.get(&data).unwrap();
        for _ in 0..120 {
            assert!(Arc::ptr_eq(&decoded, &cache.get(&data.clone()).unwrap()));
        }
        assert!(Arc::ptr_eq(
            &decoded,
            &cache.get(&Arc::from(data.as_ref())).unwrap()
        ));
        assert_eq!(cache.entries.len(), 1);
    }
}
