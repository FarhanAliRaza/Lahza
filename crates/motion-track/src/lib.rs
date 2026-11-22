//! Tracking footage: where a thing in a video is, frame by frame.
//!
//! `plane` follows a flat region (a table top, a wall, a screen) as one
//! plane, by aligning the picture itself; `rect` reads a camera's focal
//! length and the plane's pose from a rectangle seen in perspective. No
//! GPU, no model, no I/O: the caller decodes frames and decides what the
//! track drives.

// Matrices are read by row and column.
#![allow(clippy::needless_range_loop)]

pub mod image;
pub mod plane;
pub mod rect;
pub mod still;
pub mod tilt;

#[cfg(test)]
mod tests;

/// A projective map of the picture: (x, y, 1) → (X, Y, W), row-major.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct H3(pub [f64; 9]);

pub type Quad = [[f64; 2]; 4];

impl H3 {
    pub const IDENTITY: H3 = H3([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// self ∘ other (`other` first).
    pub fn then(self, other: H3) -> H3 {
        let (a, b) = (self.0, other.0);
        let mut m = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                m[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
            }
        }
        H3(m)
    }

    pub fn apply(self, p: [f64; 2]) -> [f64; 2] {
        let m = self.0;
        let w = m[6] * p[0] + m[7] * p[1] + m[8];
        [(m[0] * p[0] + m[1] * p[1] + m[2]) / w, (m[3] * p[0] + m[4] * p[1] + m[5]) / w]
    }

    pub fn determinant(self) -> f64 {
        let m = self.0;
        m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6])
    }

    pub fn inverse(self) -> Option<H3> {
        let det = self.determinant();
        if det.abs() < 1e-18 || !det.is_finite() {
            return None;
        }
        let m = self.0;
        let k = 1.0 / det;
        Some(H3([
            (m[4] * m[8] - m[5] * m[7]) * k,
            (m[2] * m[7] - m[1] * m[8]) * k,
            (m[1] * m[5] - m[2] * m[4]) * k,
            (m[5] * m[6] - m[3] * m[8]) * k,
            (m[0] * m[8] - m[2] * m[6]) * k,
            (m[2] * m[3] - m[0] * m[5]) * k,
            (m[3] * m[7] - m[4] * m[6]) * k,
            (m[1] * m[6] - m[0] * m[7]) * k,
            (m[0] * m[4] - m[1] * m[3]) * k,
        ]))
    }

    /// Scaled so the last entry is 1 (when it can be).
    pub fn normalised(self) -> H3 {
        let k = self.0[8];
        if k.abs() < 1e-18 {
            return self;
        }
        H3(self.0.map(|v| v / k))
    }

    pub fn quad(self, q: &Quad) -> Quad {
        q.map(|p| self.apply(p))
    }

    /// The map that takes `from`'s four corners to `to`'s, or None when
    /// three of either are in a line.
    pub fn from_quads(from: &Quad, to: &Quad) -> Option<H3> {
        // Eight equations in the eight entries (the ninth is 1).
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let [x, y] = from[i];
            let [u, v] = to[i];
            a[i * 2] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[i * 2 + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        let h = solve8(a)?;
        let m = H3([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]);
        m.determinant().is_finite().then_some(m).filter(|m| m.determinant().abs() > 1e-14)
    }
}

/// Gaussian elimination with partial pivoting on an 8×8 system whose
/// right-hand side is each row's ninth entry.
pub(crate) fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let piv = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-13 {
            return None;
        }
        a.swap(col, piv);
        for row in col + 1..8 {
            let f = a[row][col] / a[col][col];
            if f != 0.0 {
                for k in col..9 {
                    a[row][k] -= f * a[col][k];
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for row in (0..8).rev() {
        let s: f64 = (row + 1..8).map(|k| a[row][k] * x[k]).sum();
        x[row] = (a[row][8] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Twice the signed area of a quad (positive: clockwise on screen, y down).
pub fn signed_area2(q: &Quad) -> f64 {
    (0..4).map(|i| {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        a[0] * b[1] - b[0] * a[1]
    }).sum()
}

/// Every corner turns the same way.
pub fn convex(q: &Quad) -> bool {
    let mut sign = 0.0f64;
    for i in 0..4 {
        let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if !cross.is_finite() || cross == 0.0 {
            return false;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}
