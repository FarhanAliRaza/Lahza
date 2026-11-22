//! How a surface lies, from the camera's own motion.
//!
//! A worktop or a round table has no rectangle on it to read the
//! perspective from. But four points marked on it, followed while the
//! camera moves, say it all the same: there is one tilt of the surface
//! for which the four points keep one shape in every frame (seen from
//! each frame's own place), and for any other tilt the shape would have
//! to bend from frame to frame. With enough turning the same holds for
//! the lens.
//!
//! So: for a tilt (and a lens), lay the marked frame's points on that
//! plane, take their shape, and ask of every other frame how well a
//! rigid view of that shape lands on its points. The tilt that leaves
//! the least is the surface's.
//!
//! It needs the camera to have changed its place, not only turned: a
//! camera turning on the spot (or zooming) shows the same picture warped,
//! which every tilt explains equally. Then nothing is claimed.

use crate::rect;
use crate::Quad;

/// What the views say of the surface.
#[derive(Clone, Copy, Debug)]
pub struct Tilt {
    /// Focal length in pixels.
    pub focal: f64,
    /// The surface's normal in the marked frame's camera space (x right,
    /// y down, z away), toward the camera, unit length.
    pub normal: [f64; 3],
    /// The marked points on the surface, in its own flat coordinates
    /// (x, y; any unit; going round as marked).
    pub shape: Quad,
    /// What a rigid view of that shape leaves, root mean square, in
    /// pixels, over the views.
    pub error: f64,
    /// The same for a typical other tilt (the lower quartile over all
    /// tried): how much the views tell. Near `error`, they tell nothing.
    pub other: f64,
}

impl Tilt {
    /// The views single this tilt out: it fits, and the others do not.
    pub fn sure(&self) -> bool {
        self.error < 2.0 && self.other > 4.0 * self.error.max(0.1)
    }
}

fn normal_of(a: f64, b: f64) -> [f64; 3] {
    // (0, 0, −1) tilted by `a` about x, then `b` about y.
    let (sa, ca) = a.to_radians().sin_cos();
    let (sb, cb) = b.to_radians().sin_cos();
    [-ca * sb, sa, -ca * cb]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The marked points laid on the plane with this normal (at any
/// distance: only the shape matters), in the plane's own coordinates.
/// None when the plane would not face the camera under all four.
pub fn shape_for(marked: &Quad, centre: [f64; 2], focal: f64, normal: [f64; 3]) -> Option<Quad> {
    let mut on_plane = [[0.0f64; 3]; 4];
    for (k, p) in marked.iter().enumerate() {
        let ray = [(p[0] - centre[0]) / focal, (p[1] - centre[1]) / focal, 1.0];
        let facing = dot(normal, ray);
        if facing > -1e-3 {
            return None;
        }
        let s = -1.0 / facing;
        on_plane[k] = ray.map(|v| v * s);
    }
    let along = std::array::from_fn::<f64, 3, _>(|i| on_plane[1][i] - on_plane[0][i]);
    let len = dot(along, along).sqrt();
    if len < 1e-9 {
        return None;
    }
    let e1 = along.map(|v| v / len);
    // So that e1 × e2 points away from the camera, as a view's z does.
    let e2 = cross(normal.map(|v| -v), e1);
    Some(std::array::from_fn(|k| {
        let d = std::array::from_fn::<f64, 3, _>(|i| on_plane[k][i] - on_plane[0][i]);
        [dot(d, e1), dot(d, e2)]
    }))
}

/// What rigid views of `shape` leave on the views' points: the mean of
/// the squared distances, in pixels.
fn misfit(views: &[Quad], shape: &Quad, centre: [f64; 2], focal: f64) -> f64 {
    let mut sum = 0.0;
    for view in views {
        match rect::pose(shape, view, centre, focal) {
            Some(pose) => {
                for k in 0..4 {
                    match pose.project([shape[k][0], shape[k][1], 0.0], centre, focal) {
                        Some(q) => sum += (q[0] - view[k][0]).powi(2) + (q[1] - view[k][1]).powi(2),
                        None => sum += 1e6,
                    }
                }
            }
            None => sum += 4e6,
        }
    }
    sum / (4 * views.len().max(1)) as f64
}

/// The surface's tilt from `views` (the four marked points in each
/// frame, in pixels), `marked` being the view they were marked on.
/// `focal`: the lens if known (pixels); otherwise it is searched too,
/// for a picture `height` tall. None with fewer than three views, or
/// when no tilt faces the camera.
pub fn from_motion(views: &[Quad], marked: usize, centre: [f64; 2], height: f64, focal: Option<f64>) -> Option<Tilt> {
    if views.len() < 3 || marked >= views.len() {
        return None;
    }
    // Up to sixteen views, spread over the track.
    let step = (views.len() as f64 / 16.0).max(1.0);
    let picked: Vec<Quad> = (0..views.len().min(16)).map(|i| views[((i as f64 * step) as usize).min(views.len() - 1)]).collect();
    let focal_of = |fov: f64| height / 2.0 / (fov.to_radians() / 2.0).tan();
    let score = |a: f64, b: f64, f: f64| -> Option<f64> {
        let shape = shape_for(&views[marked], centre, f, normal_of(a, b))?;
        Some(misfit(&picked, &shape, centre, f))
    };
    // A coarse search, then steps halved around the best.
    let lenses: Vec<f64> = match focal {
        Some(f) => vec![f],
        None => (4..=18).map(|i| focal_of(i as f64 * 5.0)).collect(),
    };
    let mut all: Vec<f64> = Vec::new();
    let mut best: Option<(f64, f64, f64, f64)> = None;
    for &f in &lenses {
        for ia in -10..=10 {
            for ib in -10..=10 {
                let (a, b) = (ia as f64 * 7.5, ib as f64 * 7.5);
                if let Some(e) = score(a, b, f) {
                    all.push(e);
                    if best.is_none_or(|x| e < x.0) {
                        best = Some((e, a, b, f));
                    }
                }
            }
        }
    }
    let (mut e, mut a, mut b, mut f) = best?;
    let mut fov = (2.0 * (height / 2.0 / f).atan()).to_degrees();
    let mut step = 4.0;
    while step > 0.02 {
        let mut moved = false;
        let mut tries = vec![(a + step, b, fov), (a - step, b, fov), (a, b + step, fov), (a, b - step, fov)];
        if focal.is_none() {
            tries.push((a, b, (fov + step * 0.5).min(120.0)));
            tries.push((a, b, (fov - step * 0.5).max(8.0)));
        }
        for (ta, tb, tf) in tries {
            if ta.abs() > 85.0 || tb.abs() > 85.0 {
                continue;
            }
            if let Some(te) = score(ta, tb, focal_of(tf)) {
                if te < e {
                    (e, a, b, fov) = (te, ta, tb, tf);
                    moved = true;
                }
            }
        }
        if !moved {
            step *= 0.5;
        }
    }
    if focal.is_none() {
        f = focal_of(fov);
    }
    all.sort_by(|x, y| x.total_cmp(y));
    let other = all[all.len() / 4].sqrt();
    let normal = normal_of(a, b);
    let shape = shape_for(&views[marked], centre, f, normal)?;
    Some(Tilt { focal: f, normal, shape, error: e.sqrt(), other })
}
