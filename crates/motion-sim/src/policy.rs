//! What a network that sets a ragdoll's motors reads and gives, and
//! running one from an ONNX file.
//!
//! Once a frame (the bake's sample rate) the policy is given one
//! observation and gives one action; the targets hold for the steps of
//! that frame. Both are flat f32, for a ragdoll of B bones (its root
//! first, in `RagdollInput::bones` order) with F feet (bones named
//! `foot`, in that order). Lengths are metres, angles radians.
//!
//! Observation, `observation_len` = 16 + 11·(B − 1) + F:
//!
//! | at | what |
//! |---|---|
//! | 0..3 | up (against gravity) in the root's frame |
//! | 3..6 | the root's velocity, in its frame |
//! | 6..9 | the root's turning (angular velocity), in its frame |
//! | 9 | the root's height over its lowest foot, along up |
//! | 10..13 | the nearest body that is not its own and can move: where it is from the root, in the root's frame (zeros: none) |
//! | 13..16 | that body's velocity against the root's, in the root's frame |
//! | then 11 per bone but the root | its turn against its parent now (quaternion x, y, z, w: 4), its turning against its parent, in the parent's frame (3), the clip's turn for it now (4) |
//! | then 1 per foot | 1 when the foot touches something, else 0 |
//!
//! Action, `action_len` = 3·(B − 1): per bone but the root a turn (axis
//! times angle, in its parent's frame, each part held within ±`REACH`)
//! made on top of the clip's target for it. Zeros are the clip.
//!
//! The file is ONNX with one input `[1, observation_len]` and a first
//! output `[1, action_len]`, both f32, run by the ONNX Runtime the app
//! bundles (Linux x86-64). Nothing here trains one.

use crate::world::World;
use lahza_motion_core::sim::{quat_mul, quat_rotate, BodyKind, RagdollInput, SimInput};

/// The most a policy may turn a target from the clip's, per part (radians).
pub const REACH: f64 = 1.5;

pub fn observation_len(r: &RagdollInput, feet: usize) -> usize {
    16 + 11 * (r.bones.len() - 1) + feet
}

pub fn action_len(r: &RagdollInput) -> usize {
    3 * (r.bones.len() - 1)
}

fn conj(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// The observation of ragdoll `r` now; `x` is the sample position (for
/// the clip's targets).
pub fn observe(world: &World, input: &SimInput, r: &RagdollInput, feet: &[usize], x: f64) -> Vec<f32> {
    let u = world.unit;
    let mut o: Vec<f32> = Vec::with_capacity(observation_len(r, feet.len()));
    let mut put = |v: &[f64]| o.extend(v.iter().map(|x| *x as f32));
    let body = |b: usize| &world.w.bodies[world.handles[b]];
    let root = world.pose(r.bones[0].body);
    let inv = conj(root.q);
    let g = input.gravity;
    let gl = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
    let up = if gl > 1e-9 { g.map(|v| -v / gl) } else { [0.0, -1.0, 0.0] };
    let (rv, rw) = (body(r.bones[0].body).linvel(), body(r.bones[0].body).angvel());
    let (rv, rw) = ([rv.x as f64, rv.y as f64, rv.z as f64], [rw.x as f64, rw.y as f64, rw.z as f64]);
    put(&quat_rotate(inv, up));
    put(&quat_rotate(inv, rv));
    put(&quat_rotate(inv, rw));
    let along = |p: [f64; 3]| (p[0] * up[0] + p[1] * up[1] + p[2] * up[2]) / u;
    let lowest = feet.iter().map(|&f| along(world.pose(r.bones[f].body).p)).fold(f64::INFINITY, f64::min);
    put(&[if lowest.is_finite() { along(root.p) - lowest } else { 0.0 }]);
    // The nearest body that is not its own and can move.
    let own = input.bodies[r.bones[0].body].group;
    let near = (0..input.bodies.len())
        .filter(|&b| input.bodies[b].kind != BodyKind::Fixed && (input.bodies[b].group == 0 || input.bodies[b].group != own))
        .map(|b| {
            let p = world.pose(b).p;
            let d = [p[0] - root.p[0], p[1] - root.p[1], p[2] - root.p[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2], b, d)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match near {
        Some((_, b, d)) => {
            let v = body(b).linvel();
            put(&quat_rotate(inv, d.map(|x| x / u)));
            put(&quat_rotate(inv, [v.x as f64 - rv[0], v.y as f64 - rv[1], v.z as f64 - rv[2]]));
        }
        None => put(&[0.0; 6]),
    }
    for (k, b) in r.bones.iter().enumerate().skip(1) {
        let parent = r.bones[b.parent.unwrap_or(0)].body;
        let pq = world.pose(parent).q;
        put(&quat_mul(conj(pq), world.pose(b.body).q));
        let (wc, wp) = (body(b.body).angvel(), body(parent).angvel());
        put(&quat_rotate(conj(pq), [(wc.x - wp.x) as f64, (wc.y - wp.y) as f64, (wc.z - wp.z) as f64]));
        put(&world.clip(r, k, x).unwrap_or(b.bind));
    }
    for &f in feet {
        let h = world.handles[r.bones[f].body];
        let touching = world.w.contact_pairs().any(|p| p.has_any_active_contact() && (world.w.colliders[p.collider1].parent() == Some(h) || world.w.colliders[p.collider2].parent() == Some(h)));
        put(&[touching as u8 as f64]);
    }
    o
}

/// An action as turns to make on top of the clip's targets: per bone, a
/// quaternion in its parent's frame (None for the root).
pub fn turns(r: &RagdollInput, action: &[f32]) -> Vec<Option<[f64; 4]>> {
    let mut out = vec![None; r.bones.len()];
    for k in 1..r.bones.len() {
        let v: [f64; 3] = std::array::from_fn(|a| (action.get((k - 1) * 3 + a).copied().unwrap_or(0.0) as f64).clamp(-REACH, REACH));
        let angle = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        out[k] = Some(if angle < 1e-9 {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            let s = libm::sin(angle / 2.0) / angle;
            [v[0] * s, v[1] * s, v[2] * s, libm::cos(angle / 2.0)]
        });
    }
    out
}

/// A policy loaded from an ONNX file.
pub struct Policy {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    session: ort::session::Session,
    pub observations: usize,
    pub actions: usize,
    /// Runs since it was loaded, and the seconds they took.
    pub runs: u64,
    pub seconds: f64,
}

impl Policy {
    /// `bytes`: the ONNX file. `observations` and `actions`: the lengths
    /// the ragdoll it is for gives and takes.
    pub fn load(bytes: &[u8], observations: usize, actions: usize) -> Result<Policy, String> {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            let session = lahza_camera_matting_runtime::load(bytes).map_err(|e| format!("the policy does not load: {e}"))?;
            let mut p = Policy { session, observations, actions, runs: 0, seconds: 0.0 };
            // Run it once on zeros: a file made for another body says so here.
            p.act(&vec![0.0; observations])?;
            (p.runs, p.seconds) = (0, 0.0);
            Ok(p)
        }
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        {
            let _ = (bytes, observations, actions);
            Err("policies run on Linux x86-64 only (the bundled ONNX Runtime)".into())
        }
    }

    pub fn act(&mut self, observation: &[f32]) -> Result<Vec<f32>, String> {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            let began = std::time::Instant::now();
            let input = ort::value::Tensor::from_array(([1usize, self.observations], observation.to_vec())).map_err(|e| e.to_string())?;
            let result = self.session.run(ort::inputs![input]).map_err(|e| format!("the policy does not run on an observation of {} numbers (16 + 11 per bone but the root + 1 per foot): {e}", self.observations))?;
            let (shape, values) = result[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            if values.len() != self.actions {
                return Err(format!("the policy gives {} numbers (shape {:?}); this body takes {} (3 per bone but the root)", values.len(), &shape[..], self.actions));
            }
            if values.iter().any(|v| !v.is_finite()) {
                return Err("the policy gave a value that is not a number".into());
            }
            let out = values.to_vec();
            self.runs += 1;
            self.seconds += began.elapsed().as_secs_f64();
            Ok(out)
        }
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        {
            let _ = observation;
            Err("policies run on Linux x86-64 only".into())
        }
    }
}
