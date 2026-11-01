use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CropRect {
    pub const UNIT: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    pub fn validated(self) -> Self {
        if ![self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite())
            || self.width <= 0.0 || self.height <= 0.0
        {
            return Self::UNIT;
        }
        let x = self.x.clamp(0.0, 0.9999);
        let y = self.y.clamp(0.0, 0.9999);
        Self { x, y, width: self.width.clamp(0.0001, 1.0 - x), height: self.height.clamp(0.0001, 1.0 - y) }
    }

    pub fn sample_bounds(self, width: u32, height: u32) -> (f64, f64, f64, f64) {
        let crop = self.validated();
        let (w, h) = (width.max(1) as f64, height.max(1) as f64);
        let left = ((crop.x as f64 * w + 0.001).floor() + 0.5) / w;
        let top = ((crop.y as f64 * h + 0.001).floor() + 0.5) / h;
        let right = (((crop.x as f64 + crop.width as f64) * w - 0.001).ceil() - 0.5) / w;
        let bottom = (((crop.y as f64 + crop.height as f64) * h - 0.001).ceil() - 0.5) / h;
        (left, top, right.max(left), bottom.max(top))
    }

    pub fn snapped(self, width: u32, height: u32) -> Self {
        let crop = self.validated();
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let left = (crop.x * w).floor();
        let top = (crop.y * h).floor();
        let right = (crop.right() * w).ceil().min(w).max(left + 1.0);
        let bottom = (crop.bottom() * h).ceil().min(h).max(top + 1.0);
        Self { x: left / w, y: top / h, width: (right - left) / w, height: (bottom - top) / h }
    }

    /// Source coordinates shared by rendering, cursor placement, and editing.
    pub fn visible_rect(self, mut frame: crate::viewport::ViewportFrame) -> (f64, f64, f64, f64) {
        let crop = self.validated();
        let (x, y, width, height) = (crop.x as f64, crop.y as f64, crop.width as f64, crop.height as f64);
        frame.anchor.x = (frame.anchor.x - x) / width;
        frame.anchor.y = (frame.anchor.y - y) / height;
        let (left, top, visible_x, visible_y) = crate::viewport::visible_rect(frame);
        (x + left * width, y + top * height, visible_x * width, visible_y * height)
    }
}

impl Default for CropRect {
    fn default() -> Self { Self::UNIT }
}
