//! Whether the camera has moved between two frames.
//!
//! A camera on a tripod settles what no look at a region can: when the
//! scene behind is where it was, a surface of the scene is where it was
//! too, whatever moves over it and however little it has on it (a blank
//! table with hands and cups on it has nothing of its own to follow;
//! followed by its look it goes where the cups go).
//!
//! The picture is cut into blocks. Under a still camera the blocks that
//! show the scene itself are unchanged, almost every pixel of them, while
//! those a person moves through are not; under a camera that moved, no
//! block with anything in it is unchanged. So a fair share of unchanged
//! blocks says the camera is still, however much else moves.

use crate::image::Gray;

const COLUMNS: usize = 12;
const ROWS: usize = 8;
/// A gradient under this (grey levels a pixel, of 255) says nothing.
const MIN_GRADIENT: f32 = 3.0;
/// Noise and compression: differences this small are none.
const NOISE: f32 = 1.2;
/// A block needs this many telling pixels to have a say.
const MIN_MARKS: usize = 20;
/// A block is unchanged when this share of its telling pixels is.
const UNCHANGED: f64 = 0.85;

/// The pixels of a reference frame that would show a move: those with
/// some gradient, on every other row and column, by block.
pub struct StillTest {
    w: usize,
    h: usize,
    /// Per block: (index, value, what 0.4 px of shift changes it by).
    blocks: Vec<Vec<(u32, f32, f32)>>,
}

/// The middle of some values and their spread about it (medians).
fn middle(v: &mut [f32]) -> (f32, f32) {
    let mid = v.len() / 2;
    let m = *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    for x in v.iter_mut() {
        *x = (*x - m).abs();
    }
    (m, *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1)
}

impl StillTest {
    pub fn new(reference: &Gray) -> Self {
        let img = reference.smoothed();
        let (w, h) = (img.w, img.h);
        let mut blocks = vec![Vec::new(); COLUMNS * ROWS];
        for y in (1..h.saturating_sub(1)).step_by(2) {
            for x in (1..w.saturating_sub(1)).step_by(2) {
                let gx = (img.at(x + 1, y) - img.at(x - 1, y)) * 0.5;
                let gy = (img.at(x, y + 1) - img.at(x, y - 1)) * 0.5;
                let g = (gx * gx + gy * gy).sqrt();
                if g >= MIN_GRADIENT {
                    blocks[(y * ROWS / h) * COLUMNS + x * COLUMNS / w].push(((y * w + x) as u32, img.at(x, y), 0.4 * g));
                }
            }
        }
        for b in &mut blocks {
            if b.len() < MIN_MARKS {
                b.clear();
            }
        }
        StillTest { w, h, blocks }
    }

    /// The share of the reference's blocks (those with something in
    /// them) that `frame` leaves unchanged. A pixel is unchanged when it
    /// differs by less than 0.4 of its gradient, which is what a shift of
    /// 0.4 px makes, and by no more than noise does. Exposure is matched
    /// block by block first (each block's middle value and spread: in a
    /// block that shows the same thing these are the exposure's change
    /// exactly, whatever moves elsewhere). 1 when no block has anything
    /// in it (a blank frame cannot be said to have moved); 0 for a frame
    /// of another size.
    pub fn share(&self, frame: &Gray) -> f64 {
        if frame.w != self.w || frame.h != self.h {
            return 0.0;
        }
        let telling = self.blocks.iter().filter(|b| !b.is_empty()).count();
        if telling == 0 {
            return 1.0;
        }
        let img = frame.smoothed();
        let mut unchanged = 0usize;
        let mut scratch: Vec<f32> = Vec::new();
        for block in self.blocks.iter().filter(|b| !b.is_empty()) {
            scratch.clear();
            scratch.extend(block.iter().map(|m| m.1));
            let (mr, sr) = middle(&mut scratch);
            scratch.clear();
            scratch.extend(block.iter().map(|m| img.px[m.0 as usize]));
            let (mf, sf) = middle(&mut scratch);
            let gain = if sf > 1.0 && sr > 1.0 { (sr / sf).clamp(0.6, 1.6) } else { 1.0 };
            let same = block.iter().filter(|&&(i, v, tolerance)| ((img.px[i as usize] - mf) * gain + mr - v).abs() < tolerance + NOISE).count();
            if same as f64 >= UNCHANGED * block.len() as f64 {
                unchanged += 1;
            }
        }
        unchanged as f64 / telling as f64
    }
}

/// The share of unchanged blocks from which the camera is held to be
/// still. On four tripod shots with a person working at a table it was
/// never under 0.19; on two hand-held shots it was under 0.03 from the
/// third frame on.
pub const STILL: f64 = 0.08;
