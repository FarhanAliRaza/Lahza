//! 2.5D: 4×4 world transforms, the perspective camera, and the 3×3
//! homography that maps a planar layer onto the canvas.
//!
//! Conventions: x right, y down, z away from the viewer (AE). Rotations are
//! in degrees; `rotation` (z) is clockwise on screen.

use crate::compile::Affine;

/// Projective map of the plane: (x, y, 1) → (X, Y, W), point = (X/W, Y/W).
/// Row-major.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proj(pub [f64; 9]);

impl From<Affine> for Proj {
    fn from(a: Affine) -> Self {
        let [a0, b, c, d, e, f] = a.0;
        Proj([a0, c, e, b, d, f, 0.0, 0.0, 1.0])
    }
}

impl Proj {
    pub const IDENTITY: Proj = Proj([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// self ∘ other (apply `other` first).
    pub fn then(self, other: impl Into<Proj>) -> Proj {
        let a = self.0;
        let b = other.into().0;
        let mut m = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                m[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
            }
        }
        Proj(m)
    }

    pub fn apply_w(self, x: f64, y: f64) -> (f64, f64, f64) {
        let m = self.0;
        (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5], m[6] * x + m[7] * y + m[8])
    }

    /// The image of (x, y); points behind the camera come back non-finite.
    pub fn apply(self, x: f64, y: f64) -> (f64, f64) {
        let (px, py, w) = self.apply_w(x, y);
        if w <= 1e-9 {
            return (f64::NAN, f64::NAN);
        }
        (px / w, py / w)
    }

    pub fn is_affine(self) -> bool {
        self.0[6] == 0.0 && self.0[7] == 0.0 && (self.0[8] - 1.0).abs() < 1e-12
    }

    pub fn determinant(self) -> f64 {
        let m = self.0;
        m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6])
    }

    pub fn inverse(self) -> Option<Proj> {
        let det = self.determinant();
        if det.abs() < 1e-14 || !det.is_finite() {
            return None;
        }
        let m = self.0;
        let inv = 1.0 / det;
        Some(Proj([
            (m[4] * m[8] - m[5] * m[7]) * inv,
            (m[2] * m[7] - m[1] * m[8]) * inv,
            (m[1] * m[5] - m[2] * m[4]) * inv,
            (m[5] * m[6] - m[3] * m[8]) * inv,
            (m[0] * m[8] - m[2] * m[6]) * inv,
            (m[2] * m[3] - m[0] * m[5]) * inv,
            (m[3] * m[7] - m[4] * m[6]) * inv,
            (m[1] * m[6] - m[0] * m[7]) * inv,
            (m[0] * m[4] - m[1] * m[3]) * inv,
        ]))
    }

    /// Area scale at a local point: √|det J|.
    pub fn local_scale(self, x: f64, y: f64) -> f64 {
        let (_, _, w) = self.apply_w(x, y);
        if w.abs() < 1e-12 {
            return 0.0;
        }
        (self.determinant() / (w * w * w)).abs().sqrt()
    }

    /// Largest axis scale at the local origin (for blur radii).
    pub fn max_scale(self) -> f64 {
        if self.is_affine() {
            let m = self.0;
            let sx = (m[0] * m[0] + m[3] * m[3]).sqrt();
            let sy = (m[1] * m[1] + m[4] * m[4]).sqrt();
            return sx.max(sy);
        }
        self.local_scale(0.0, 0.0)
    }
}

/// 4×4 transform, row-major, acting on column vectors (x, y, z, 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [f64; 16]);

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

    pub fn then(self, other: Mat4) -> Mat4 {
        let (a, b) = (self.0, other.0);
        let mut m = [0.0; 16];
        for r in 0..4 {
            for c in 0..4 {
                m[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
            }
        }
        Mat4(m)
    }

    pub fn translate(x: f64, y: f64, z: f64) -> Mat4 {
        let mut m = Self::IDENTITY;
        m.0[3] = x;
        m.0[7] = y;
        m.0[11] = z;
        m
    }

    pub fn scale(x: f64, y: f64, z: f64) -> Mat4 {
        let mut m = Self::IDENTITY;
        m.0[0] = x;
        m.0[5] = y;
        m.0[10] = z;
        m
    }

    /// Clockwise on screen, like `Affine::rotate`.
    pub fn rotate_z(deg: f64) -> Mat4 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut m = Self::IDENTITY;
        m.0[0] = c;
        m.0[1] = -s;
        m.0[4] = s;
        m.0[5] = c;
        m
    }

    /// Tilts the top away from the viewer for positive angles.
    pub fn rotate_x(deg: f64) -> Mat4 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut m = Self::IDENTITY;
        m.0[5] = c;
        m.0[6] = s;
        m.0[9] = -s;
        m.0[10] = c;
        m
    }

    /// Turns the right side away from the viewer for positive angles.
    pub fn rotate_y(deg: f64) -> Mat4 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut m = Self::IDENTITY;
        m.0[0] = c;
        m.0[2] = -s;
        m.0[8] = s;
        m.0[10] = c;
        m
    }

    pub fn from_affine(a: Affine) -> Mat4 {
        let [a0, b, c, d, e, f] = a.0;
        Mat4([a0, c, 0.0, e, b, d, 0.0, f, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0])
    }

    /// The x/y part (for layers drawn flat).
    pub fn to_affine(self) -> Affine {
        let m = self.0;
        Affine([m[0], m[4], m[1], m[5], m[3], m[7]])
    }

    pub fn apply(self, x: f64, y: f64, z: f64) -> [f64; 4] {
        let m = self.0;
        std::array::from_fn(|r| m[r * 4] * x + m[r * 4 + 1] * y + m[r * 4 + 2] * z + m[r * 4 + 3])
    }

    /// Inverse of the upper 3×3 transposed (normals' transform), or None
    /// when it is singular.
    pub fn normal_matrix(self) -> Option<[[f64; 3]; 3]> {
        let m = self.0;
        let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if det.abs() < 1e-18 || !det.is_finite() {
            return None;
        }
        // Cofactor matrix / det = (A⁻¹)ᵀ.
        let c = |r0: usize, r1: usize, c0: usize, c1: usize| a[r0][c0] * a[r1][c1] - a[r0][c1] * a[r1][c0];
        let k = 1.0 / det;
        Some([
            [c(1, 2, 1, 2) * k, -c(1, 2, 0, 2) * k, c(1, 2, 0, 1) * k],
            [-c(0, 2, 1, 2) * k, c(0, 2, 0, 2) * k, -c(0, 2, 0, 1) * k],
            [c(0, 1, 1, 2) * k, -c(0, 1, 0, 2) * k, c(0, 1, 0, 1) * k],
        ])
    }

    /// The homography of the local z = 0 plane: columns x, y, w; rows x, y, w.
    pub fn plane(self) -> Proj {
        let m = self.0;
        Proj([m[0], m[1], m[3], m[4], m[5], m[7], m[12], m[13], m[15]])
    }
}

/// A 3D camera looking at `poi` from `distance` along +z (before orbit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera3 {
    /// World → camera space (camera at the origin, looking down +z).
    pub view: Mat4,
    /// World → screen homogeneous (X, Y, Z, W), point = (X/W, Y/W).
    pub project: Mat4,
    /// Camera space → screen: x = focal·x/z + centre.x (same for y).
    pub focal: f64,
    pub centre: (f64, f64),
}

impl Camera3 {
    /// `fov` vertical, degrees; `orbit` = [yaw, pitch] degrees around the
    /// point of interest; `dolly` moves the camera toward it (px).
    #[allow(clippy::too_many_arguments)]
    pub fn new(width: f64, height: f64, poi: (f64, f64), zoom: f64, roll: f64, fov: f64, orbit: (f64, f64), dolly: f64) -> Self {
        let f = focal(height, fov);
        let distance = f - dolly;
        let view = Mat4::translate(0.0, 0.0, distance)
            .then(Mat4::rotate_z(-roll))
            .then(Mat4::rotate_x(-orbit.1))
            .then(Mat4::rotate_y(-orbit.0))
            .then(Mat4::translate(-poi.0, -poi.1, 0.0));
        let k = zoom * f;
        let intrinsics = Mat4([k, 0.0, width / 2.0, 0.0, 0.0, k, height / 2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        Camera3 { view, project: intrinsics.then(view), focal: k, centre: (width / 2.0, height / 2.0) }
    }
}

/// Focal length in px for a vertical field of view (AE's default camera
/// is 39.6°: the z = 0 plane then maps 1:1).
pub fn focal(height: f64, fov: f64) -> f64 {
    height / 2.0 / (fov.clamp(1.0, 170.0).to_radians() / 2.0).tan()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_camera_maps_the_plane_one_to_one() {
        let cam = Camera3::new(1920.0, 1080.0, (960.0, 540.0), 1.0, 0.0, 39.6, (0.0, 0.0), 0.0);
        let h = cam.project.plane();
        let (x, y) = h.apply(100.0, 200.0);
        assert!((x - 100.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9);
        // Farther planes look smaller and closer to the centre.
        let far = cam.project.then(Mat4::translate(0.0, 0.0, 1000.0)).plane();
        let (x, _) = far.apply(0.0, 540.0);
        assert!(x > 0.0 && x < 960.0);
    }

    #[test]
    fn homography_inverse_round_trips() {
        let cam = Camera3::new(1280.0, 720.0, (640.0, 360.0), 1.2, 5.0, 50.0, (25.0, -10.0), 100.0);
        let world = Mat4::translate(600.0, 300.0, 50.0).then(Mat4::rotate_y(35.0)).then(Mat4::rotate_x(-20.0));
        let h = cam.project.then(world).plane();
        let inv = h.inverse().unwrap();
        let (sx, sy) = h.apply(30.0, -40.0);
        let (x, y) = inv.apply(sx, sy);
        assert!((x - 30.0).abs() < 1e-6 && (y + 40.0).abs() < 1e-6);
    }
}
