//! Grey pictures and their pyramids.

/// One channel, row by row, values 0–255.
#[derive(Clone, Debug)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Gray {
    pub fn new(w: usize, h: usize) -> Self {
        Gray { w, h, px: vec![0.0; w * h] }
    }

    pub fn from_luma8(w: usize, h: usize, data: &[u8]) -> Self {
        Gray { w, h, px: data[..w * h].iter().map(|&v| v as f32).collect() }
    }

    /// Rec. 601 luma of packed RGB.
    pub fn from_rgb24(w: usize, h: usize, data: &[u8]) -> Self {
        let px = data[..w * h * 3].chunks_exact(3).map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
        Gray { w, h, px }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.px[y * self.w + x]
    }

    /// Bilinear; None outside the picture.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> Option<f32> {
        if !(x >= 0.0 && y >= 0.0 && x <= (self.w - 1) as f32 && y <= (self.h - 1) as f32) {
            return None;
        }
        let (x0, y0) = ((x as usize).min(self.w - 2), (y as usize).min(self.h - 2));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let i = y0 * self.w + x0;
        let (a, b, c, d) = (self.px[i], self.px[i + 1], self.px[i + self.w], self.px[i + self.w + 1]);
        Some(a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy)
    }

    /// Catmull–Rom over 4 × 4 pixels: sharper than bilinear, which blurs
    /// most at half a pixel and so pulls a fit a little toward whatever
    /// hides that blur. Bilinear within a pixel of the border; None
    /// outside the picture.
    #[inline]
    pub fn sample_cubic(&self, x: f32, y: f32) -> Option<f32> {
        if !(x >= 1.0 && y >= 1.0 && x < (self.w - 2) as f32 && y < (self.h - 2) as f32) {
            return self.sample(x, y);
        }
        let (x0, y0) = (x as usize, y as usize);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let weights = |t: f32| {
            let (t2, t3) = (t * t, t * t * t);
            [-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2]
        };
        let (wx, wy) = (weights(fx), weights(fy));
        let mut v = 0.0;
        for (j, wyj) in wy.iter().enumerate() {
            let row = &self.px[(y0 + j - 1) * self.w + x0 - 1..];
            v += wyj * (wx[0] * row[0] + wx[1] * row[1] + wx[2] * row[2] + wx[3] * row[3]);
        }
        Some(v)
    }

    /// Half the size: each pixel the mean of four.
    pub fn half(&self) -> Gray {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut out = Gray::new(w, h);
        for y in 0..h {
            let (r0, r1) = (&self.px[y * 2 * self.w..], &self.px[(y * 2 + 1) * self.w..]);
            let row = &mut out.px[y * w..(y + 1) * w];
            for (x, o) in row.iter_mut().enumerate() {
                *o = 0.25 * (r0[x * 2] + r0[x * 2 + 1] + r1[x * 2] + r1[x * 2 + 1]);
            }
        }
        out
    }

    /// A light 3×3 blur (1 2 1), so gradients are of the picture and not
    /// of its noise.
    pub fn smoothed(&self) -> Gray {
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0.0f32; w * h];
        for y in 0..h {
            let row = &self.px[y * w..(y + 1) * w];
            let out = &mut tmp[y * w..(y + 1) * w];
            for x in 0..w {
                let (l, r) = (row[x.saturating_sub(1)], row[(x + 1).min(w - 1)]);
                out[x] = 0.25 * l + 0.5 * row[x] + 0.25 * r;
            }
        }
        let mut out = Gray::new(w, h);
        for y in 0..h {
            let (u, d) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                out.px[y * w + x] = 0.25 * tmp[u * w + x] + 0.5 * tmp[y * w + x] + 0.25 * tmp[d * w + x];
            }
        }
        out
    }
}

/// A picture and its halvings; level 0 is the picture (smoothed).
#[derive(Clone, Debug)]
pub struct Pyramid {
    pub levels: Vec<Gray>,
}

/// The smallest side a level may have.
const MIN_SIDE: usize = 24;

impl Pyramid {
    pub fn new(frame: &Gray, levels: usize) -> Self {
        let mut out = vec![frame.smoothed()];
        while out.len() < levels.max(1) {
            let last = out.last().unwrap();
            if last.w / 2 < MIN_SIDE || last.h / 2 < MIN_SIDE {
                break;
            }
            let next = last.half();
            out.push(next);
        }
        Pyramid { levels: out }
    }
}

/// Level-0 coordinates of a level's pixel centre, and back.
#[inline]
pub fn to_base(v: f64, level: usize) -> f64 {
    (v + 0.5) * (1u32 << level) as f64 - 0.5
}

#[inline]
pub fn to_level(v: f64, level: usize) -> f64 {
    (v + 0.5) / (1u32 << level) as f64 - 0.5
}
