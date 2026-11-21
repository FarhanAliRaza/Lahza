//! Skinning: a mesh whose points follow a skeleton's joints (glTF skins).
//!
//! A skinned mesh names its joints as layers (`MeshLayer::skin`: the comps
//! `model_import` makes of the file's joint nodes), so a joint is placed
//! like any layer: by keys, by a parent, by a bake. A point's place is the
//! weighted sum of where each of its joints carries it:
//!
//!   p' = Σ wⱼ · (Jⱼ · Bⱼ) · p
//!
//! with Bⱼ the joint's inverse bind matrix (mesh space → the joint's, as
//! the file bound them) and Jⱼ the joint's place now (its layer's space →
//! the mesh layer's). The CPU (`skin`) and the GPU's mesh pool
//! (`meshskin.wgsl`) work from the same `matrices`, f32 rows of Jⱼ · Bⱼ.

use crate::mesh::Mesh;
use crate::space::Mat4;

/// What a file's skin gives a mesh: per point its joints and weights, per
/// joint its inverse bind matrix (in the layer's axes and px).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SkinData {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// A second four a point (glTF's JOINTS_1 and WEIGHTS_1), when the
    /// file has them: eight joints a point.
    pub more: Option<(Vec<[u16; 4]>, Vec<[f32; 4]>)>,
    pub inverse_bind: Vec<Mat4>,
}

impl SkinData {
    /// Point `i`'s joints and weights, the second four after the first
    /// (zeros when the file has four a point).
    #[inline]
    pub fn of(&self, i: usize) -> Option<([u16; 8], [f32; 8])> {
        let (j, w) = (self.joints.get(i)?, self.weights.get(i)?);
        let (mut js, mut ws) = ([0u16; 8], [0.0f32; 8]);
        js[..4].copy_from_slice(j);
        ws[..4].copy_from_slice(w);
        if let Some((j, w)) = self.more.as_ref().and_then(|(j, w)| Some((j.get(i)?, w.get(i)?))) {
            js[4..].copy_from_slice(j);
            ws[4..].copy_from_slice(w);
        }
        Some((js, ws))
    }
}

/// Per joint: the three rows of Jⱼ · Bⱼ (x, y, z; each [a, b, c, t]).
/// `pose`: each joint's layer space → the mesh layer's. Joints the pose
/// lacks stay where the file bound them.
pub fn matrices(data: &SkinData, pose: &[Mat4]) -> Vec<[[f32; 4]; 3]> {
    data.inverse_bind
        .iter()
        .enumerate()
        .map(|(j, b)| match pose.get(j) {
            Some(p) => {
                let m = p.then(*b).0;
                std::array::from_fn(|r| std::array::from_fn(|c| m[r * 4 + c] as f32))
            }
            None => [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
        })
        .collect()
}

/// A skin's joints at one moment, as both skinners take them.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Posed {
    /// Per joint: the rows of Jⱼ · Bⱼ (`matrices`).
    pub matrices: Vec<[[f32; 4]; 3]>,
    /// For dual quaternions, per joint: its turn and move as one (the
    /// real and the dual quaternion, x y z w), then the three rows of
    /// what is left of Jⱼ · Bⱼ without them (its scale).
    pub dual: Option<Vec<[[f32; 4]; 5]>>,
}

impl Posed {
    pub fn new(data: &SkinData, pose: &[Mat4], blend: crate::document::SkinBlend) -> Posed {
        let matrices = matrices(data, pose);
        let dual = (blend == crate::document::SkinBlend::Dual).then(|| {
            data.inverse_bind
                .iter()
                .enumerate()
                .map(|(j, b)| {
                    let m = pose.get(j).map(|p| p.then(*b)).unwrap_or(Mat4::IDENTITY);
                    let (rigid, rest) = crate::sim::decompose(m);
                    let (q, t) = (rigid.q, rigid.p);
                    // ½ · (t, 0) · q.
                    let d = crate::sim::quat_mul([t[0], t[1], t[2], 0.0], q).map(|v| v * 0.5);
                    let row = |r: usize| [rest.0[r * 4] as f32, rest.0[r * 4 + 1] as f32, rest.0[r * 4 + 2] as f32, 0.0];
                    [q.map(|v| v as f32), d.map(|v| v as f32), row(0), row(1), row(2)]
                })
                .collect()
        });
        Posed { matrices, dual }
    }

    /// The rows a joint takes in the GPU's pool.
    pub fn rows_per_joint(&self) -> usize {
        if self.dual.is_some() { 5 } else { 3 }
    }

    /// Its rows for the GPU's kernel.
    pub fn write(&self, out: &mut Vec<[f32; 4]>) {
        match &self.dual {
            Some(d) => out.extend(d.iter().flatten().copied()),
            None => out.extend(self.matrices.iter().flatten().copied()),
        }
    }
}

/// One point and its normal under `m` (`matrices`), as the GPU's kernel
/// computes them: the sum over its joints in order, f32.
#[inline]
pub fn skin_point(m: &[[[f32; 4]; 3]], joints: &[u16], weights: &[f32], p: [f32; 3], n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mut q = [0.0f32; 3];
    let mut d = [0.0f32; 3];
    for k in 0..joints.len().min(weights.len()) {
        let w = weights[k];
        if w == 0.0 {
            continue;
        }
        let Some(a) = m.get(joints[k] as usize) else { continue };
        for r in 0..3 {
            q[r] += w * (a[r][0] * p[0] + a[r][1] * p[1] + a[r][2] * p[2] + a[r][3]);
            d[r] += w * (a[r][0] * n[0] + a[r][1] * n[1] + a[r][2] * n[2]);
        }
    }
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let d = if l > 0.0 { [d[0] / l, d[1] / l, d[2] / l] } else { n };
    (q, d)
}

#[inline]
fn cross32(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `v` turned by the unit quaternion `q`.
#[inline]
fn turn32(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let c = cross32(u, v);
    let c = cross32(u, [c[0] + q[3] * v[0], c[1] + q[3] * v[1], c[2] + q[3] * v[2]]);
    [v[0] + 2.0 * c[0], v[1] + 2.0 * c[1], v[2] + 2.0 * c[2]]
}

/// `skin_point` with dual quaternions (`Posed::dual`): the joints' scales
/// blended as matrices, their turns and moves blended as one screw, so a
/// twisted or sharply bent joint keeps its girth. Each joint's quaternion
/// is taken on the side of the first's.
#[inline]
pub fn skin_point_dual(m: &[[[f32; 4]; 5]], joints: &[u16], weights: &[f32], p: [f32; 3], n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (mut ps, mut ns) = ([0.0f32; 3], [0.0f32; 3]);
    let (mut br, mut bd) = ([0.0f32; 4], [0.0f32; 4]);
    let mut first: Option<[f32; 4]> = None;
    for k in 0..joints.len().min(weights.len()) {
        let w = weights[k];
        if w == 0.0 {
            continue;
        }
        let Some(a) = m.get(joints[k] as usize) else { continue };
        for r in 0..3 {
            ps[r] += w * (a[2 + r][0] * p[0] + a[2 + r][1] * p[1] + a[2 + r][2] * p[2]);
            ns[r] += w * (a[2 + r][0] * n[0] + a[2 + r][1] * n[1] + a[2 + r][2] * n[2]);
        }
        let pivot = *first.get_or_insert(a[0]);
        let s = if a[0][0] * pivot[0] + a[0][1] * pivot[1] + a[0][2] * pivot[2] + a[0][3] * pivot[3] < 0.0 { -w } else { w };
        for c in 0..4 {
            br[c] += s * a[0][c];
            bd[c] += s * a[1][c];
        }
    }
    let l = (br[0] * br[0] + br[1] * br[1] + br[2] * br[2] + br[3] * br[3]).sqrt();
    if !(l > 0.0) {
        return (p, n);
    }
    let (br, bd) = (br.map(|v| v / l), bd.map(|v| v / l));
    let c = cross32([br[0], br[1], br[2]], [bd[0], bd[1], bd[2]]);
    let t: [f32; 3] = std::array::from_fn(|r| 2.0 * (br[3] * bd[r] - bd[3] * br[r] + c[r]));
    let q = turn32(br, ps);
    let d = turn32(br, ns);
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let d = if l > 0.0 { [d[0] / l, d[1] / l, d[2] / l] } else { n };
    ([q[0] + t[0], q[1] + t[1], q[2] + t[2]], d)
}

/// `mesh` with every point moved by its joints. Points the skin has no
/// weights for stay.
pub fn skin(mesh: &Mesh, data: &SkinData, posed: &Posed) -> Mesh {
    let mut out = mesh.clone();
    let count = if data.more.is_some() { 8 } else { 4 };
    for i in 0..mesh.positions.len() {
        let Some((j, w)) = data.of(i) else { continue };
        let n = mesh.normals.get(i).copied().unwrap_or([0.0, 0.0, -1.0]);
        let (p, n) = match &posed.dual {
            Some(d) => skin_point_dual(d, &j[..count], &w[..count], mesh.positions[i], n),
            None => skin_point(&posed.matrices, &j[..count], &w[..count], mesh.positions[i], n),
        };
        out.positions[i] = p;
        if let Some(slot) = out.normals.get_mut(i) {
            *slot = n;
        }
    }
    out
}

/// The box around the skinned mesh without skinning it: each joint's box
/// of the points it carries (`joint_boxes`), carried by the joint.
pub fn extent(boxes: &[Option<([f32; 3], [f32; 3])>], m: &[[[f32; 4]; 3]]) -> Option<([f64; 3], [f64; 3])> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for (b, a) in boxes.iter().zip(m) {
        let Some((blo, bhi)) = b else { continue };
        for corner in 0..8 {
            let p: [f32; 3] = std::array::from_fn(|k| if corner >> k & 1 == 0 { blo[k] } else { bhi[k] });
            for r in 0..3 {
                let v = (a[r][0] * p[0] + a[r][1] * p[1] + a[r][2] * p[2] + a[r][3]) as f64;
                lo[r] = lo[r].min(v);
                hi[r] = hi[r].max(v);
            }
        }
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

/// `extent` for dual quaternions: a point blended as a screw can lie
/// outside the box of where each joint alone would carry it (that is the
/// girth it keeps), by no more than half the box's own size.
pub fn extent_dual(boxes: &[Option<([f32; 3], [f32; 3])>], m: &[[[f32; 4]; 3]]) -> Option<([f64; 3], [f64; 3])> {
    let (lo, hi) = extent(boxes, m)?;
    let pad = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max) * 0.5;
    Some((lo.map(|v| v - pad), hi.map(|v| v + pad)))
}

/// Per joint: the box of the bind-pose points it has any weight on.
pub fn joint_boxes(mesh: &Mesh, data: &SkinData) -> Vec<Option<([f32; 3], [f32; 3])>> {
    let mut boxes: Vec<Option<([f32; 3], [f32; 3])>> = vec![None; data.inverse_bind.len()];
    for (i, p) in mesh.positions.iter().enumerate() {
        let Some((j, w)) = data.of(i) else { continue };
        for k in 0..8 {
            if w[k] == 0.0 {
                continue;
            }
            let Some(slot) = boxes.get_mut(j[k] as usize) else { continue };
            let (lo, hi) = slot.get_or_insert((*p, *p));
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::SkinBlend;

    /// A strip of four points along x bound to two joints: the root at the
    /// origin, the second at x = 10. Points at 0 and 5 follow the root,
    /// 15 the second joint, 10 both by half.
    fn strip() -> (Mesh, SkinData) {
        let mesh = Mesh { positions: vec![[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [10.0, 0.0, 0.0], [15.0, 0.0, 0.0]], normals: vec![[0.0, -1.0, 0.0]; 4], tris: vec![[0, 1, 2], [1, 2, 3]], tags: vec![0; 2], edges: vec![7; 2], uvs: vec![[0.0; 2]; 4] };
        let data = SkinData {
            joints: vec![[0, 0, 0, 0], [0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]],
            weights: vec![[1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]],
            more: None,
            // Mesh space → each joint's: the second joint sits at x = 10.
            inverse_bind: vec![Mat4::IDENTITY, Mat4::translate(-10.0, 0.0, 0.0)],
        };
        (mesh, data)
    }

    #[test]
    fn the_bind_pose_leaves_the_mesh_as_it_was() {
        let (mesh, data) = strip();
        let pose = [Mat4::IDENTITY, Mat4::translate(10.0, 0.0, 0.0)];
        assert_eq!(skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Linear)), mesh);
        // Dual quaternions too (to the last bits of a quaternion's turn).
        let dual = skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Dual));
        assert!(dual.positions.iter().zip(&mesh.positions).all(|(a, b)| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)), "{:?}", dual.positions);
    }

    #[test]
    fn points_follow_their_joints_by_weight() {
        let (mesh, data) = strip();
        // The second joint turned a quarter (clockwise on screen: +x → +y)
        // about its own origin (10, 0), and the root lifted 2 px.
        let pose = [Mat4::translate(0.0, -2.0, 0.0), Mat4::translate(10.0, -2.0, 0.0).then(Mat4::rotate_z(90.0))];
        let out = skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Linear));
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5);
        // By hand: the root's points only lift.
        assert!(near(out.positions[0], [0.0, -2.0, 0.0]) && near(out.positions[1], [5.0, -2.0, 0.0]), "{:?}", out.positions);
        // (15, 0) is 5 along the joint's x: turned to 5 along y, from (10, −2).
        assert!(near(out.positions[3], [10.0, 3.0, 0.0]), "{:?}", out.positions[3]);
        // (10, 0) is at the joint's origin: both joints put it at (10, −2).
        assert!(near(out.positions[2], [10.0, -2.0, 0.0]), "{:?}", out.positions[2]);
        // Normals turn with their joints: up (−y) becomes +x under the
        // quarter turn; half of each is their unit mean.
        assert!(near(out.normals[3], [1.0, 0.0, 0.0]) && near(out.normals[0], [0.0, -1.0, 0.0]), "{:?}", out.normals);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(out.normals[2], [h, -h, 0.0]), "{:?}", out.normals[2]);
        // The cheap box holds every skinned point.
        let (lo, hi) = extent(&joint_boxes(&mesh, &data), &matrices(&data, &pose)).unwrap();
        for p in &out.positions {
            assert!((0..3).all(|k| p[k] as f64 >= lo[k] - 1e-4 && p[k] as f64 <= hi[k] + 1e-4), "{p:?} outside {lo:?}..{hi:?}");
        }
    }

    #[test]
    fn a_twisted_joint_keeps_its_girth_with_dual_quaternions() {
        // A ring of four points round the y axis, 10 px out, each half on
        // a joint that stands and half on one twisted 180° about y.
        let ring = [[10.0f32, 0.0, 0.0], [0.0, 0.0, 10.0], [-10.0, 0.0, 0.0], [0.0, 0.0, -10.0]];
        let mesh = Mesh { positions: ring.to_vec(), normals: ring.iter().map(|p| p.map(|v| v / 10.0)).collect(), tris: vec![[0, 1, 2], [0, 2, 3]], tags: vec![0; 2], edges: vec![7; 2], uvs: vec![[0.0; 2]; 4] };
        let data = SkinData { joints: vec![[0, 1, 0, 0]; 4], weights: vec![[0.5, 0.5, 0.0, 0.0]; 4], more: None, inverse_bind: vec![Mat4::IDENTITY; 2] };
        let pose = [Mat4::IDENTITY, Mat4::rotate_y(180.0)];
        let radius = |m: &Mesh| m.positions.iter().map(|p| (p[0] * p[0] + p[2] * p[2]).sqrt()).fold(f32::INFINITY, f32::min);
        // Blended as matrices each point is the mean of itself and its
        // opposite: the ring is a point (the candy wrapper).
        let linear = skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Linear));
        assert!(radius(&linear) < 1e-4, "{:?}", linear.positions);
        // Blended as screws it is the ring turned a quarter, as wide as it was.
        let dual = skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Dual));
        assert!((radius(&dual) - 10.0).abs() < 1e-4, "{:?}", dual.positions);
        let p = dual.positions[0];
        assert!(p[0].abs() < 1e-4 && (p[2].abs() - 10.0).abs() < 1e-4 && p[1].abs() < 1e-5, "(10, 0, 0) a quarter round y: {p:?}");
        // Its normals turn with it and stay unit.
        assert!((0..4).all(|i| (0..3).all(|k| (dual.normals[i][k] - dual.positions[i][k] / 10.0).abs() < 1e-4)));
        // A joint that moves and scales: one joint alone is its matrix,
        // either way (a sixth turn about z, twice the size, moved).
        let alone = SkinData { joints: vec![[0, 0, 0, 0]; 4], weights: vec![[1.0, 0.0, 0.0, 0.0]; 4], more: None, inverse_bind: vec![Mat4::translate(3.0, -2.0, 1.0)] };
        let pose = [Mat4::translate(40.0, 5.0, -7.0).then(Mat4::rotate_z(60.0)).then(Mat4::rotate_x(25.0)).then(Mat4::scale(2.0, 2.0, 2.0))];
        let (a, b) = (skin(&mesh, &alone, &Posed::new(&alone, &pose, SkinBlend::Linear)), skin(&mesh, &alone, &Posed::new(&alone, &pose, SkinBlend::Dual)));
        for (x, y) in a.positions.iter().zip(&b.positions) {
            assert!((0..3).all(|k| (x[k] - y[k]).abs() < 2e-4), "{x:?} vs {y:?}");
        }
        // The box for the pool holds every point.
        let posed = Posed::new(&data, &[Mat4::IDENTITY, Mat4::rotate_y(180.0)], SkinBlend::Dual);
        let (lo, hi) = extent_dual(&joint_boxes(&mesh, &data), &posed.matrices).unwrap();
        assert!(dual.positions.iter().all(|p| (0..3).all(|k| p[k] as f64 >= lo[k] && p[k] as f64 <= hi[k])));
    }

    #[test]
    fn eight_joints_a_point_all_count() {
        // The strip's shared point with its two joints in the two fours.
        let (mesh, mut data) = strip();
        let four = data.clone();
        data.joints[2] = [0, 0, 0, 0];
        data.weights[2] = [0.5, 0.0, 0.0, 0.0];
        data.more = Some((vec![[0; 4], [0; 4], [1, 0, 0, 0], [0; 4]], vec![[0.0; 4], [0.0; 4], [0.5, 0.0, 0.0, 0.0], [0.0; 4]]));
        let pose = [Mat4::translate(0.0, -2.0, 0.0), Mat4::translate(14.0, -2.0, 0.0).then(Mat4::rotate_z(90.0))];
        for blend in [SkinBlend::Linear, SkinBlend::Dual] {
            let (a, b) = (skin(&mesh, &four, &Posed::new(&four, &pose, blend)), skin(&mesh, &data, &Posed::new(&data, &pose, blend)));
            assert_eq!(a, b, "{blend:?}");
        }
        // (Its second joint moved 4 px: the shared point is 2 px along.)
        let out = skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Linear));
        assert!((out.positions[2][0] - 12.0).abs() < 1e-5, "{:?}", out.positions[2]);
        // The second joint's box holds the point it only carries in the second four.
        let boxes = joint_boxes(&mesh, &data);
        assert_eq!(boxes[1].unwrap().0[0], 10.0);
    }

    /// `cargo test -p lahza-motion-core --release skin -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn benchmark_skinning_100k_points() {
        let n = 100_000;
        let mesh = Mesh { positions: (0..n).map(|i| [i as f32 * 0.01, (i % 7) as f32, 0.0]).collect(), normals: vec![[0.0, -1.0, 0.0]; n], tris: Vec::new(), tags: Vec::new(), edges: Vec::new(), uvs: vec![[0.0; 2]; n] };
        let data = SkinData { joints: (0..n).map(|i| [(i % 50) as u16, ((i + 1) % 50) as u16, ((i + 2) % 50) as u16, ((i + 3) % 50) as u16]).collect(), weights: vec![[0.4, 0.3, 0.2, 0.1]; n], more: None, inverse_bind: vec![Mat4::IDENTITY; 50] };
        let pose: Vec<Mat4> = (0..50).map(|j| Mat4::rotate_z(j as f64)).collect();
        let t = std::time::Instant::now();
        let out = skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Linear));
        println!("100k points, 4 joints each: {:?} ({} points)", t.elapsed(), out.positions.len());
        let t = std::time::Instant::now();
        let out = skin(&mesh, &data, &Posed::new(&data, &pose, SkinBlend::Dual));
        println!("the same with dual quaternions: {:?} ({} points)", t.elapsed(), out.positions.len());
        let eight = SkinData { more: Some((data.joints.clone(), vec![[0.0, 0.0, 0.0, 0.0]; n])), ..data.clone() };
        let t = std::time::Instant::now();
        let out = skin(&mesh, &eight, &Posed::new(&eight, &pose, SkinBlend::Linear));
        println!("eight joints a point (four with no weight): {:?} ({} points)", t.elapsed(), out.positions.len());
    }
}
