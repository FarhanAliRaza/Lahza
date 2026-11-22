//! Camera solving: the scene camera (field of view, orbit, dolly, point of
//! interest, lens shift) that puts known 3D points where they appear in a
//! reference frame. Levenberg–Marquardt on the reprojection error, through
//! the engine's own camera (`space::Camera3`), so what is solved is what
//! renders; soft priors toward the starting camera keep it steady when the
//! marks cannot tell parameters apart (shift against position, say).

use crate::space::Camera3;
use serde::{Deserialize, Serialize};

/// A scene camera, as the document's `camera` block.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraParams {
    #[serde(default = "fov_default")]
    pub fov: f64,
    #[serde(default)]
    pub orbit: [f64; 2],
    #[serde(default)]
    pub dolly: f64,
    /// Point of interest; None: the canvas centre.
    #[serde(default)]
    pub position: Option<[f64; 2]>,
    #[serde(default)]
    pub shift: [f64; 2],
    /// Roll, degrees (the document camera's `rotation`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn fov_default() -> f64 {
    39.6
}

impl Default for CameraParams {
    fn default() -> Self {
        Self { fov: 39.6, orbit: [0.0, 0.0], dolly: 0.0, position: None, shift: [0.0, 0.0], rotation: 0.0 }
    }
}

/// A 3D point (scene px, like a layer's position and z) and where it is
/// seen in the frame (px).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub world: [f64; 3],
    pub image: [f64; 2],
}

/// Which parameters may move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Free {
    pub fov: bool,
    pub orbit: bool,
    pub dolly: bool,
    pub position: bool,
    pub shift: bool,
    pub roll: bool,
}

impl Default for Free {
    fn default() -> Self {
        Self { fov: true, orbit: true, dolly: true, position: true, shift: false, roll: false }
    }
}

const N: usize = 9;

fn pack(c: &CameraParams, w: f64, h: f64) -> [f64; N] {
    let p = c.position.unwrap_or([w / 2.0, h / 2.0]);
    [c.fov, c.orbit[0], c.orbit[1], c.dolly, p[0], p[1], c.shift[0], c.shift[1], c.rotation]
}

fn unpack(v: &[f64; N]) -> CameraParams {
    CameraParams { fov: v[0], orbit: [v[1], v[2]], dolly: v[3], position: Some([v[4], v[5]]), shift: [v[6], v[7]], rotation: v[8] }
}

/// Where the camera puts a world point (None: behind it).
pub fn project(c: &CameraParams, w: f64, h: f64, p: [f64; 3]) -> Option<[f64; 2]> {
    let poi = c.position.unwrap_or([w / 2.0, h / 2.0]);
    let cam = Camera3::new(w, h, (poi[0], poi[1]), 1.0, c.rotation, c.fov, (c.orbit[0], c.orbit[1]), c.dolly).shifted((c.shift[0], c.shift[1]));
    let q = cam.project.apply(p[0], p[1], p[2]);
    (q[3] > 1e-6).then(|| [q[0] / q[3], q[1] / q[3]])
}

/// Root-mean-square reprojection error, px.
pub fn rms(c: &CameraParams, w: f64, h: f64, marks: &[Mark]) -> f64 {
    let e: f64 = marks.iter().map(|m| match project(c, w, h, m.world) {
        Some(q) => (q[0] - m.image[0]).powi(2) + (q[1] - m.image[1]).powi(2),
        None => 1e8,
    }).sum();
    (e / marks.len().max(1) as f64).sqrt()
}

/// Solves the camera for `marks`, starting from `start` (and pulled gently
/// toward it); `free` says which parameters may move.
pub fn fit(w: f64, h: f64, marks: &[Mark], start: &CameraParams, free: Free) -> (CameraParams, f64) {
    let p0 = pack(start, w, h);
    let mask = [free.fov, free.orbit, free.orbit, free.dolly, free.position, free.position, free.shift, free.shift, free.roll];
    // Prior scales: how far each may wander before the pull is felt
    // (a tenth of a pixel of error per this much change).
    let scale = [40.0, 90.0, 90.0, 2000.0, w, h, w, h, 90.0];
    let residuals = |v: &[f64; N]| -> Vec<f64> {
        let c = unpack(v);
        let mut r = Vec::with_capacity(marks.len() * 2 + N);
        for m in marks {
            match project(&c, w, h, m.world) {
                Some(q) => {
                    r.push(q[0] - m.image[0]);
                    r.push(q[1] - m.image[1]);
                }
                None => {
                    r.push(1e4);
                    r.push(1e4);
                }
            }
        }
        for k in 0..N {
            r.push(if mask[k] { 0.1 * (v[k] - p0[k]) / scale[k] } else { 0.0 });
        }
        r
    };
    let cost = |r: &[f64]| r.iter().map(|x| x * x).sum::<f64>();
    let mut v = p0;
    let mut r = residuals(&v);
    let mut lambda = 1e-3;
    for _ in 0..200 {
        // Jacobian by central differences (free parameters only).
        let m = r.len();
        let mut jac = vec![[0.0f64; N]; m];
        for k in 0..N {
            if !mask[k] {
                continue;
            }
            let step = 1e-4 * scale[k].max(1.0) * 1e-2;
            let (mut a, mut b) = (v, v);
            a[k] += step;
            b[k] -= step;
            let (ra, rb) = (residuals(&a), residuals(&b));
            for i in 0..m {
                jac[i][k] = (ra[i] - rb[i]) / (2.0 * step);
            }
        }
        // (JᵀJ + λ diag) δ = −Jᵀr
        let mut jtj = [[0.0f64; N]; N];
        let mut jtr = [0.0f64; N];
        for i in 0..m {
            for a in 0..N {
                jtr[a] += jac[i][a] * r[i];
                for b in 0..N {
                    jtj[a][b] += jac[i][a] * jac[i][b];
                }
            }
        }
        let mut improved = false;
        for _ in 0..10 {
            let mut a = jtj;
            for k in 0..N {
                a[k][k] += lambda * (jtj[k][k] + 1e-9) + if mask[k] { 0.0 } else { 1.0 };
            }
            let Some(delta) = solve(a, jtr.map(|x| -x)) else { break };
            let mut nv = v;
            for k in 0..N {
                if mask[k] {
                    nv[k] += delta[k];
                }
            }
            nv[0] = nv[0].clamp(1.0, 170.0);
            let nr = residuals(&nv);
            if cost(&nr) < cost(&r) {
                let small = (cost(&r) - cost(&nr)) < 1e-12 * cost(&r).max(1e-12);
                v = nv;
                r = nr;
                lambda = (lambda * 0.3).max(1e-9);
                improved = !small;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    let c = unpack(&v);
    let e = rms(&c, w, h, marks);
    (c, e)
}

/// The camera that sees the scene as a pose says: a scene point p is at
/// R·p + t in camera space (x right, y down, z away), through a lens of
/// `fov` on a canvas w × h. The engine's camera has exactly these
/// freedoms (orbit, roll, where it looks on z = 0, how far it stands), so
/// this is the same camera written its way. None when it looks along the
/// scene's plane (no point of z = 0 is ahead of it).
pub fn from_pose(r: [[f64; 3]; 3], t: [f64; 3], w: f64, h: f64, fov: f64) -> Option<CameraParams> {
    // view = move back by d · roll · pitch · yaw · move to the point of
    // interest, so R = Rz(−roll) Rx(−pitch) Ry(−yaw).
    let pitch = r[2][1].clamp(-1.0, 1.0).asin().to_degrees();
    let yaw = -r[2][0].atan2(r[2][2]).to_degrees();
    let roll = -(-r[0][1]).atan2(r[1][1]).to_degrees();
    // The point of interest is on z = 0, straight ahead at distance d:
    // Rᵀ((0, 0, d) − t) has no z.
    if r[2][2].abs() < 1e-9 {
        return None;
    }
    let d = (r[0][2] * t[0] + r[1][2] * t[1] + r[2][2] * t[2]) / r[2][2];
    let v = [-t[0], -t[1], d - t[2]];
    let poi = [r[0][0] * v[0] + r[1][0] * v[1] + r[2][0] * v[2], r[0][1] * v[0] + r[1][1] * v[1] + r[2][1] * v[2]];
    let c = CameraParams { fov, orbit: [yaw, pitch], dolly: crate::space::focal(h, fov) - d, position: Some(poi), shift: [0.0, 0.0], rotation: roll };
    (d > 1e-6 && poi.iter().all(|v| v.is_finite()) && w > 0.0).then_some(c)
}

/// Gaussian elimination with partial pivoting.
fn solve(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let piv = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..N {
            let f = a[row][col] / a[col][col];
            for k in col..N {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let s: f64 = (row + 1..N).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(c: &CameraParams) -> Vec<Mark> {
        let pts = [[300.0, 200.0, 0.0], [1500.0, 260.0, 120.0], [900.0, 800.0, -200.0], [400.0, 900.0, 300.0], [1700.0, 950.0, 0.0], [960.0, 500.0, 600.0], [200.0, 500.0, -300.0]];
        pts.iter().map(|&p| Mark { world: p, image: project(c, 1920.0, 1080.0, p).unwrap() }).collect()
    }

    /// From the default camera, the solver finds a turned, dollied, wider
    /// camera that six marks came from, to a fraction of a pixel.
    #[test]
    fn recovers_a_known_camera() {
        let truth = CameraParams { fov: 50.0, orbit: [18.0, -12.0], dolly: 300.0, position: Some([1000.0, 520.0]), shift: [0.0, 0.0], rotation: 0.0 };
        let m = marks(&truth);
        let (c, e) = fit(1920.0, 1080.0, &m, &CameraParams::default(), Free::default());
        assert!(e < 0.2, "rms {e} px: {c:?}");
        assert!((c.fov - 50.0).abs() < 1.0 && (c.orbit[0] - 18.0).abs() < 1.0 && (c.orbit[1] + 12.0).abs() < 1.0, "{c:?}");
        // Fixed parameters stay put.
        let (c2, _) = fit(1920.0, 1080.0, &m, &CameraParams::default(), Free { fov: false, ..Free::default() });
        assert_eq!(c2.fov, 39.6);
    }

    /// A turned, rolled, dollied camera read back from its own view
    /// matrix, and found again by the solver with roll free.
    #[test]
    fn a_pose_is_the_engine_camera() {
        let (w, h) = (1920.0, 1080.0);
        for truth in [
            CameraParams { fov: 35.0, orbit: [22.0, -48.0], dolly: 150.0, position: Some([1010.0, 610.0]), shift: [0.0, 0.0], rotation: 7.0 },
            CameraParams { fov: 60.0, orbit: [-35.0, 20.0], dolly: -400.0, position: Some([400.0, 300.0]), shift: [0.0, 0.0], rotation: -25.0 },
            CameraParams { fov: 39.6, orbit: [0.0, 0.0], dolly: 0.0, position: Some([960.0, 540.0]), shift: [0.0, 0.0], rotation: 0.0 },
        ] {
            let poi = truth.position.unwrap();
            let view = Camera3::new(w, h, (poi[0], poi[1]), 1.0, truth.rotation, truth.fov, (truth.orbit[0], truth.orbit[1]), truth.dolly).view.0;
            let r = [[view[0], view[1], view[2]], [view[4], view[5], view[6]], [view[8], view[9], view[10]]];
            let t = [view[3], view[7], view[11]];
            let c = from_pose(r, t, w, h, truth.fov).unwrap();
            assert!((c.orbit[0] - truth.orbit[0]).abs() < 1e-6 && (c.orbit[1] - truth.orbit[1]).abs() < 1e-6, "{c:?}");
            assert!((c.rotation - truth.rotation).abs() < 1e-6 && (c.dolly - truth.dolly).abs() < 1e-6, "{c:?}");
            let p = c.position.unwrap();
            assert!((p[0] - poi[0]).abs() < 1e-6 && (p[1] - poi[1]).abs() < 1e-6, "{c:?}");
            // Four corners of a rectangle on z = 0, from a camera a
            // little off: the solver comes back with roll free.
            let corners = [[700.0, 400.0, 0.0], [1220.0, 400.0, 0.0], [1220.0, 680.0, 0.0], [700.0, 680.0, 0.0]];
            let m: Vec<Mark> = corners.iter().map(|&p| Mark { world: p, image: project(&truth, w, h, p).unwrap() }).collect();
            let start = CameraParams { orbit: [truth.orbit[0] + 3.0, truth.orbit[1] - 2.0], rotation: truth.rotation + 2.0, dolly: truth.dolly + 40.0, ..truth };
            let (c, e) = fit(w, h, &m, &start, Free { fov: false, roll: true, ..Free::default() });
            assert!(e < 0.05, "rms {e} px: {c:?}");
            assert!((c.rotation - truth.rotation).abs() < 0.2 && (c.orbit[1] - truth.orbit[1]).abs() < 0.2, "{c:?}");
        }
    }
}
