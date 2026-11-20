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
    pub inverse_bind: Vec<Mat4>,
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

/// One point and its normal under `m` (`matrices`), as the GPU's kernel
/// computes them: the sum over the four joints in order, f32.
#[inline]
pub fn skin_point(m: &[[[f32; 4]; 3]], joints: [u16; 4], weights: [f32; 4], p: [f32; 3], n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mut q = [0.0f32; 3];
    let mut d = [0.0f32; 3];
    for k in 0..4 {
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

/// `mesh` with every point moved by its joints (`m`: `matrices`). Points
/// the skin has no weights for stay.
pub fn skin(mesh: &Mesh, data: &SkinData, m: &[[[f32; 4]; 3]]) -> Mesh {
    let mut out = mesh.clone();
    for i in 0..mesh.positions.len() {
        let (Some(j), Some(w)) = (data.joints.get(i), data.weights.get(i)) else { continue };
        let n = mesh.normals.get(i).copied().unwrap_or([0.0, 0.0, -1.0]);
        let (p, n) = skin_point(m, *j, *w, mesh.positions[i], n);
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

/// Per joint: the box of the bind-pose points it has any weight on.
pub fn joint_boxes(mesh: &Mesh, data: &SkinData) -> Vec<Option<([f32; 3], [f32; 3])>> {
    let mut boxes: Vec<Option<([f32; 3], [f32; 3])>> = vec![None; data.inverse_bind.len()];
    for (i, p) in mesh.positions.iter().enumerate() {
        let (Some(j), Some(w)) = (data.joints.get(i), data.weights.get(i)) else { continue };
        for k in 0..4 {
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

    /// A strip of four points along x bound to two joints: the root at the
    /// origin, the second at x = 10. Points at 0 and 5 follow the root,
    /// 15 the second joint, 10 both by half.
    fn strip() -> (Mesh, SkinData) {
        let mesh = Mesh { positions: vec![[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [10.0, 0.0, 0.0], [15.0, 0.0, 0.0]], normals: vec![[0.0, -1.0, 0.0]; 4], tris: vec![[0, 1, 2], [1, 2, 3]], tags: vec![0; 2], edges: vec![7; 2], uvs: vec![[0.0; 2]; 4] };
        let data = SkinData {
            joints: vec![[0, 0, 0, 0], [0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]],
            weights: vec![[1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]],
            // Mesh space → each joint's: the second joint sits at x = 10.
            inverse_bind: vec![Mat4::IDENTITY, Mat4::translate(-10.0, 0.0, 0.0)],
        };
        (mesh, data)
    }

    #[test]
    fn the_bind_pose_leaves_the_mesh_as_it_was() {
        let (mesh, data) = strip();
        let pose = [Mat4::IDENTITY, Mat4::translate(10.0, 0.0, 0.0)];
        assert_eq!(skin(&mesh, &data, &matrices(&data, &pose)), mesh);
    }

    #[test]
    fn points_follow_their_joints_by_weight() {
        let (mesh, data) = strip();
        // The second joint turned a quarter (clockwise on screen: +x → +y)
        // about its own origin (10, 0), and the root lifted 2 px.
        let pose = [Mat4::translate(0.0, -2.0, 0.0), Mat4::translate(10.0, -2.0, 0.0).then(Mat4::rotate_z(90.0))];
        let out = skin(&mesh, &data, &matrices(&data, &pose));
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

    /// `cargo test -p lahza-motion-core --release skin -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn benchmark_skinning_100k_points() {
        let n = 100_000;
        let mesh = Mesh { positions: (0..n).map(|i| [i as f32 * 0.01, (i % 7) as f32, 0.0]).collect(), normals: vec![[0.0, -1.0, 0.0]; n], tris: Vec::new(), tags: Vec::new(), edges: Vec::new(), uvs: vec![[0.0; 2]; n] };
        let data = SkinData { joints: (0..n).map(|i| [(i % 50) as u16, ((i + 1) % 50) as u16, ((i + 2) % 50) as u16, ((i + 3) % 50) as u16]).collect(), weights: vec![[0.4, 0.3, 0.2, 0.1]; n], inverse_bind: vec![Mat4::IDENTITY; 50] };
        let pose: Vec<Mat4> = (0..50).map(|j| Mat4::rotate_z(j as f64)).collect();
        let t = std::time::Instant::now();
        let m = matrices(&data, &pose);
        let out = skin(&mesh, &data, &m);
        println!("100k points, 4 joints each: {:?} ({} points)", t.elapsed(), out.positions.len());
    }
}
