//! What a rectangle seen in perspective says: the lens, the rectangle's
//! proportions, and where the camera is against its plane.
//!
//! A real rectangle (a table top, a book, a screen) marked by its four
//! corners, in order: top-left, top-right, bottom-right, bottom-left as
//! they lie on the surface. With a pinhole camera whose centre is the
//! picture's, the two sides' directions are at right angles in space,
//! and that one fact per frame gives the focal length (Zhang's
//! calibration, one plane). It says nothing when a pair of sides is
//! parallel in the picture (the rectangle seen square on): then the lens
//! and the rectangle's proportions trade against each other, and one of
//! them has to be given.

use crate::{Quad, H3};

const UNIT: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Unit square → the quad, in coordinates centred on `centre` and in
/// units of `height`, scaled so its two first columns have mean length 1.
fn columns(quad: &Quad, centre: [f64; 2], height: f64) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let q = quad.map(|p| [(p[0] - centre[0]) / height, (p[1] - centre[1]) / height]);
    let h = H3::from_quads(&UNIT, &q)?.0;
    let (c1, c2, c3) = ([h[0], h[3], h[6]], [h[1], h[4], h[7]], [h[2], h[5], h[8]]);
    Some((c1, c2, c3))
}

/// The lens a tracked rectangle implies.
#[derive(Clone, Copy, Debug)]
pub struct Lens {
    /// Focal length in pixels (for a picture `height` tall).
    pub focal: f64,
    /// How much perspective the estimate rests on: the typical product
    /// of the two sides' foreshortening. Under about 0.01 the rectangle
    /// is seen too squarely for the focal length to mean much.
    pub strength: f64,
}

/// The focal length from one or more views of a rectangle. `aspect`
/// (width ÷ height of the real rectangle), when known, adds a second
/// fact per view and covers the square-on case.
pub fn focal(quads: &[Quad], centre: [f64; 2], height: f64, aspect: Option<f64>) -> Option<Lens> {
    let (mut aa, mut ab, mut strength, mut n) = (0.0f64, 0.0f64, 0.0f64, 0usize);
    for quad in quads {
        let Some((h1, h2, _)) = columns(quad, centre, height) else { continue };
        // Sides at right angles: (h1·h2 in the picture) φ + h1z h2z = 0,
        // φ = 1 / focal².
        let k = 1.0 / ((h1[0] * h1[0] + h1[1] * h1[1]).sqrt() * (h2[0] * h2[0] + h2[1] * h2[1]).sqrt()).max(1e-12);
        let (a, b) = ((h1[0] * h2[0] + h1[1] * h2[1]) * k, h1[2] * h2[2] * k);
        aa += a * a;
        ab += a * b;
        strength += b.abs();
        n += 1;
        if let Some(r) = aspect {
            // Sides in proportion: |K⁻¹h1| = r |K⁻¹h2|.
            let (a, b) = (((h1[0] * h1[0] + h1[1] * h1[1]) - r * r * (h2[0] * h2[0] + h2[1] * h2[1])) * k, (h1[2] * h1[2] - r * r * h2[2] * h2[2]) * k);
            aa += a * a;
            ab += a * b;
            strength += b.abs();
        }
    }
    if n == 0 || aa <= 1e-18 {
        return None;
    }
    let phi = -ab / aa;
    let strength = strength / n as f64;
    if !(phi.is_finite() && phi > 0.0) {
        return None;
    }
    let f = (1.0 / phi).sqrt();
    // Between a 2° and a 150° field of view.
    (0.13..=30.0).contains(&f).then_some(Lens { focal: f * height, strength })
}

/// The real rectangle's width ÷ height, given the focal length (px).
/// The median over the views.
pub fn aspect(quads: &[Quad], centre: [f64; 2], height: f64, focal: f64) -> Option<f64> {
    let phi = (height / focal).powi(2);
    let mut all: Vec<f64> = quads.iter().filter_map(|quad| {
        let (h1, h2, _) = columns(quad, centre, height)?;
        let (n1, n2) = ((h1[0] * h1[0] + h1[1] * h1[1]) * phi + h1[2] * h1[2], (h2[0] * h2[0] + h2[1] * h2[1]) * phi + h2[2] * h2[2]);
        (n1 > 0.0 && n2 > 0.0).then(|| (n1 / n2).sqrt())
    }).filter(|a| a.is_finite()).collect();
    if all.is_empty() {
        return None;
    }
    all.sort_by(|a, b| a.total_cmp(b));
    Some(all[all.len() / 2])
}

/// Where a plane is against the camera: a point (x, y, z) of the plane's
/// own space is at R·(x, y, z) + t in camera space (x right, y down, z
/// away), and is seen at focal·(X/Z, Y/Z) + centre.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub r: [[f64; 3]; 3],
    pub t: [f64; 3],
}

impl Pose {
    pub fn project(&self, p: [f64; 3], centre: [f64; 2], focal: f64) -> Option<[f64; 2]> {
        let c: [f64; 3] = std::array::from_fn(|i| self.r[i][0] * p[0] + self.r[i][1] * p[1] + self.r[i][2] * p[2] + self.t[i]);
        (c[2] > 1e-9).then(|| [focal * c[0] / c[2] + centre[0], focal * c[1] / c[2] + centre[1]])
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The nearest rotation to a matrix (columns nearly at right angles):
/// Newton's iteration for the polar decomposition.
fn nearest_rotation(mut m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    for _ in 0..30 {
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        // Inverse transposed: the cofactors over the determinant.
        let cof = [
            [m[1][1] * m[2][2] - m[1][2] * m[2][1], m[1][2] * m[2][0] - m[1][0] * m[2][2], m[1][0] * m[2][1] - m[1][1] * m[2][0]],
            [m[0][2] * m[2][1] - m[0][1] * m[2][2], m[0][0] * m[2][2] - m[0][2] * m[2][0], m[0][1] * m[2][0] - m[0][0] * m[2][1]],
            [m[0][1] * m[1][2] - m[0][2] * m[1][1], m[0][2] * m[1][0] - m[0][0] * m[1][2], m[0][0] * m[1][1] - m[0][1] * m[1][0]],
        ];
        let mut change = 0.0f64;
        let mut next = m;
        for r in 0..3 {
            for c in 0..3 {
                next[r][c] = 0.5 * (m[r][c] + cof[r][c] / det);
                change = change.max((next[r][c] - m[r][c]).abs());
            }
        }
        m = next;
        if change < 1e-13 {
            break;
        }
    }
    Some(m)
}

/// The plane's pose from where four of its points are seen: `world`
/// (x, y on the plane, z = 0) at `image`, with the lens known. The
/// closed form from the homography; a caller that wants the least
/// reprojection error refines from here.
pub fn pose(world: &Quad, image: &Quad, centre: [f64; 2], focal: f64) -> Option<Pose> {
    let q = image.map(|p| [(p[0] - centre[0]) / focal, (p[1] - centre[1]) / focal]);
    let h = H3::from_quads(world, &q)?.0;
    let (m1, m2, m3) = ([h[0], h[3], h[6]], [h[1], h[4], h[7]], [h[2], h[5], h[8]]);
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let mut k = 2.0 / (len(m1) + len(m2));
    if !k.is_finite() {
        return None;
    }
    // The plane is in front of the camera.
    if m3[2] * k < 0.0 {
        k = -k;
    }
    let (r1, r2, t) = (m1.map(|v| v * k), m2.map(|v| v * k), m3.map(|v| v * k));
    let r3 = cross(r1, r2);
    let r = nearest_rotation([[r1[0], r2[0], r3[0]], [r1[1], r2[1], r3[1]], [r1[2], r2[2], r3[2]]])?;
    (t[2] > 1e-9).then_some(Pose { r, t })
}
