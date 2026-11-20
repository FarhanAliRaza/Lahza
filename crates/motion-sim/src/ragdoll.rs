//! Ragdolls in the world: a joint from each bone to its parent, with the
//! limits the skeleton gives, and motors that pull each joint toward what
//! the clip has it at.

use crate::world::{quat, v3, x_to, World};
use lahza_motion_core::sim::{quat_mul, RagdollInput, SimInput};
use rapier3d::prelude::*;

/// A motor at full strength: rapier's stiffness and damping for it
/// (acceleration-based: the same whatever the bone weighs). Set by trial:
/// at these a joint follows a clip that turns it 450° a second within a
/// few degrees and settles without swinging; at a tenth of the pull it
/// trails by a quarter second and swings.
///
/// The motor is a joint of its own beside the one that holds the bone
/// and limits it, and its frame on the parent is turned to the target
/// each step: rapier takes a turn apart by axis, which is right near
/// zero and swings a joint far from it (a raised arm is 150° from where
/// it hangs). So the motor always aims for zero.
///
/// No motor gives more torque than `MOST` (radians per second²) would
/// need on the inertia of what hangs from its joint: a limb held against
/// a wall presses with that and no harder. A limp joint keeps `LIMP` of
/// damping: flesh, not a hinge on a bearing.
pub const PULL: f64 = 9000.0;
pub const CALM: f64 = 190.0;
pub const MOST: f64 = 600.0;
pub const LIMP: f64 = 4.0;

/// A ragdoll's joints in the world.
#[derive(Clone)]
pub struct Rig {
    /// Per bone: its joint to its parent, and its motor.
    pub joints: Vec<Option<(ImpulseJointHandle, ImpulseJointHandle)>>,
    /// Per bone: the inertia about its joint of what hangs from it (kg·m²).
    inertia: Vec<f64>,
    /// Per bone: the clip's target at the last step (its speed is the
    /// change: a motor that only knew where to be would trail it).
    last: Vec<Option<[f64; 4]>>,
}

/// Which bodies of one group (a ragdoll's bones) collide with each
/// other: per group, bone by bone (`World::touch`). Not the ones jointed
/// together, nor the ones whose shapes overlap as the skeleton stands at
/// rest (a hip's capsule and a thigh's, a chest's and a shoulder's): a
/// hand does meet the trunk. Nor, for the step, a bone a behaviour is
/// placing (`World::free`): an arm aimed at its own head is worked out
/// without the body in its way, and goes as it did before bones met. A
/// collider's `user_data` is its group, and above 32 bits its bone's
/// index plus one.
pub(crate) struct Groups<'a>(pub &'a [Vec<bool>], pub &'a [Vec<bool>], pub &'a std::collections::BTreeSet<(u32, u32)>);

impl PhysicsHooks for Groups<'_> {
    fn filter_contact_pair(&self, c: &PairFilterContext) -> Option<SolverFlags> {
        let (a, b) = (c.colliders[c.collider1].user_data, c.colliders[c.collider2].user_data);
        let (ga, gb) = (a as u32, b as u32);
        if !self.2.is_empty() {
            if let (Some(x), Some(y)) = (c.rigid_body1, c.rigid_body2) {
                let (x, y) = (x.into_raw_parts().0, y.into_raw_parts().0);
                if self.2.contains(&(x.min(y), x.max(y))) {
                    return None;
                }
            }
        }
        if ga == 0 || ga != gb {
            return Some(SolverFlags::default());
        }
        let (ia, ib) = ((a >> 32) as usize, (b >> 32) as usize);
        let may = self.0.get(ga as usize - 1)?;
        let n = (may.len() as f64).sqrt() as usize;
        if ia == 0 || ib == 0 || !may.get((ia - 1) * n + ib - 1).copied().unwrap_or(false) {
            return None;
        }
        let free = |i: usize| self.1.get(ga as usize - 1).and_then(|f| f.get(i - 1)).copied().unwrap_or(false);
        (!free(ia) && !free(ib)).then_some(SolverFlags::default())
    }
}

/// The clip's turn of a bone against its parent at sample position `x`.
fn clip_at(clip: &[[f64; 4]], x: f64) -> Option<[f64; 4]> {
    let last = clip.len().checked_sub(1)?;
    let x = x.clamp(0.0, last as f64);
    let i = (x.floor() as usize).min(last);
    let j = (i + 1).min(last);
    let f = x - i as f64;
    let (a, b) = (clip[i], clip[j]);
    let s = if (0..4).map(|k| a[k] * b[k]).sum::<f64>() < 0.0 { -1.0 } else { 1.0 };
    let q: [f64; 4] = std::array::from_fn(|k| a[k] + (s * b[k] - a[k]) * f);
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    Some(q.map(|v| v / n))
}

pub(crate) fn sampled(v: &[f64], x: f64) -> f64 {
    let Some(last) = v.len().checked_sub(1) else { return 0.0 };
    let x = x.clamp(0.0, last as f64);
    let i = (x.floor() as usize).min(last);
    v[i] + (v[(i + 1).min(last)] - v[i]) * (x - i as f64)
}

impl World {
    /// Joints for every ragdoll of `input`.
    pub(crate) fn add_ragdolls(&mut self, input: &SimInput) {
        let m = 1.0 / self.unit;
        for r in &input.ragdolls {
            // What hangs from each joint, and how far out.
            let place = |b: &lahza_motion_core::sim::BoneInput| {
                let p = &input.bodies[b.body].keyed[0];
                (p.p, p.apply(b.along.map(|v| v * b.length * 0.5)))
            };
            let inertia: Vec<f64> = (0..r.bones.len())
                .map(|k| {
                    let joint = place(&r.bones[k]).0;
                    let mut sum = 0.0;
                    for (d, bone) in r.bones.iter().enumerate() {
                        let mut cur = Some(d);
                        while let Some(c) = cur {
                            if c == k {
                                let centre = place(bone).1;
                                let far: f64 = (0..3).map(|a| ((centre[a] - joint[a]) * m).powi(2)).sum();
                                sum += bone.mass * (far + (bone.length * m).powi(2) / 12.0);
                                break;
                            }
                            cur = r.bones[c].parent;
                        }
                    }
                    sum.max(1e-4)
                })
                .collect();
            let mut rig = Rig { joints: Vec::with_capacity(r.bones.len()), inertia, last: vec![None; r.bones.len()] };
            for b in &r.bones {
                let Some(p) = b.parent else {
                    rig.joints.push(None);
                    continue;
                };
                let (parent, child) = (self.handles[r.bones[p].body], self.handles[b.body]);
                // The joint's frame: x along the bone (a hinge: along its
                // axle), the same on both at the rest pose.
                let on_child = x_to(b.hinge.unwrap_or(b.along));
                let on_parent = quat(b.bind) * on_child;
                let rad = |d: f64| d.to_radians() as f32;
                let mut data: GenericJoint = match b.hinge {
                    Some(_) => RevoluteJointBuilder::new(Vector::X).limits([0.0, rad(b.swing)]).build().into(),
                    None => SphericalJointBuilder::new().limits(JointAxis::AngX, [-rad(b.twist), rad(b.twist)]).limits(JointAxis::AngY, [-rad(b.swing), rad(b.swing)]).limits(JointAxis::AngZ, [-rad(b.swing), rad(b.swing)]).build().into(),
                };
                data.set_local_frame1(Pose::from_parts(v3(b.anchor_parent, m), on_parent)).set_local_frame2(Pose::from_parts(Vector::ZERO, on_child));
                data.set_contacts_enabled(false);
                // (The limp damping is this joint's.)
                let axes: &[JointAxis] = if b.hinge.is_some() { &[JointAxis::AngX] } else { &[JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ] };
                for axis in axes {
                    data.set_motor(*axis, 0.0, 0.0, 0.0, LIMP as f32);
                }
                let h = self.w.insert_impulse_joint(parent, child, data);
                // The motor: nothing held, three turning axes driven.
                let mut motor = GenericJointBuilder::new(JointAxesMask::empty()).build();
                motor.set_local_frame1(Pose::from_parts(v3(b.anchor_parent, m), quat(b.bind))).set_local_frame2(Pose::identity());
                motor.set_contacts_enabled(false);
                let hm = self.w.insert_impulse_joint(parent, child, motor);
                rig.joints.push(Some((h, hm)));
            }
            self.rigs.push(rig);
            // Which of its bones may touch.
            let n = r.bones.len();
            let mut may = vec![false; if r.self_collide { n * n } else { 0 }];
            // (Every bone as the skeleton stands at rest, from its root.)
            let mut rest: Vec<Pose> = Vec::with_capacity(n);
            for b in &r.bones {
                rest.push(match b.parent {
                    Some(p) if p < rest.len() => rest[p] * Pose::from_parts(v3(b.anchor_parent, m), quat(b.bind)),
                    _ => Pose::identity(),
                });
            }
            for (i, b) in r.bones.iter().enumerate() {
                let colliders: Vec<ColliderHandle> = self.w.bodies[self.handles[b.body]].colliders().to_vec();
                for h in colliders {
                    let c = &mut self.w.colliders[h];
                    c.user_data = (c.user_data as u32) as u128 | ((i as u128 + 1) << 32);
                }
            }
            if r.self_collide {
                let shapes = |i: usize| -> Vec<(Pose, SharedShape)> {
                    self.w.bodies[self.handles[r.bones[i].body]].colliders().iter().map(|h| (rest[i] * self.w.colliders[*h].position_wrt_parent().copied().unwrap_or(Pose::identity()), self.w.colliders[*h].shared_shape().clone())).collect()
                };
                let all: Vec<Vec<(Pose, SharedShape)>> = (0..n).map(shapes).collect();
                for i in 0..n {
                    for j in 0..i {
                        let jointed = r.bones[i].parent == Some(j) || r.bones[j].parent == Some(i);
                        // (A hand's breadth of the scene's metre between them at rest, or they are one flesh.)
                        let close = all[i].iter().any(|(pa, a)| all[j].iter().any(|(pb, b)| rapier3d::parry::query::distance(pa, &**a, pb, &**b).map_or(true, |d| d.distance < 0.02)));
                        (may[i * n + j], may[j * n + i]) = (!jointed && !close, !jointed && !close);
                    }
                }
            }
            self.touch.push(may);
            self.free.push(vec![false; n]);
        }
    }

    /// The clip's targets and each bone's strength, for the step that
    /// ends at sample position `x`. `targets`: per bone, a turn against
    /// its parent that stands for the clip's (a behaviour's, a search's).
    pub fn drive_ragdoll(&mut self, r: &RagdollInput, rig: usize, x: f64, targets: Option<&[Option<[f64; 4]>]>, scale: Option<&[f64]>) {
        // (Bones that still follow their keys need no motors.)
        if !self.released[r.bones[0].body] {
            return;
        }
        let dt = self.dt;
        let m = 1.0 / self.unit;
        for (k, b) in r.bones.iter().enumerate() {
            let Some((_, motor)) = self.rigs[rig].joints[k] else { continue };
            let strength = (sampled(&b.strength, x) * scale.and_then(|s| s.get(k).copied()).unwrap_or(1.0)).clamp(0.0, 1.0);
            let given = targets.and_then(|t| t.get(k).copied().flatten());
            let clip = clip_at(&b.clip, x);
            let want = given.or(clip).unwrap_or(b.bind);
            // The clip's own turning, in the target's frame (a behaviour's
            // target is worked out from the body each step: its change is
            // no speed to follow).
            let was = std::mem::replace(&mut self.rigs[rig].last[k], clip);
            let follow = match (given, was, clip) {
                (None, Some(a), Some(c)) => {
                    let d = quat_mul([-a[0], -a[1], -a[2], a[3]], c);
                    let d = if d[3] < 0.0 { d.map(|v| -v) } else { d };
                    let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if s < 1e-12 { [0.0; 3] } else { [0, 1, 2].map(|a| d[a] / s * 2.0 * libm::atan2(s, d[3]) / dt) }
                }
                _ => [0.0; 3],
            };
            let most = (MOST * strength * self.rigs[rig].inertia[k]) as f32;
            let Some(joint) = self.w.impulse_joints.get_mut(motor, strength > 0.0) else { continue };
            joint.data.set_local_frame1(Pose::from_parts(v3(b.anchor_parent, m), quat(want)));
            for (a, axis) in [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ].into_iter().enumerate() {
                joint.data.set_motor(axis, 0.0, follow[a] as f32, (PULL * strength) as f32, (CALM * strength) as f32);
                joint.data.set_motor_max_force(axis, most);
            }
        }
    }

    /// A ragdoll's hits that fall in the step from `t`.
    pub(crate) fn hit_ragdoll(&mut self, input: &SimInput, r: &RagdollInput, t: f64) {
        let dt = self.dt;
        for (n, (at, bone, velocity, dur)) in r.hits.iter().enumerate() {
            // Another seed is another take: with `jitter`, each blow lands
            // a little differently.
            let mut rng = crate::world::Rng(input.seed ^ (n as u64 + 1).wrapping_mul(0x9fb21c651e98df25) ^ r.bones.len() as u64);
            let velocity = &velocity.map(|v| v + input.jitter * rng.signed());
            let share = if *dur <= dt {
                if *at >= t - 1e-9 && *at < t + dt - 1e-9 { 1.0 } else { 0.0 }
            } else {
                ((t + dt).min(at + dur) - t.max(*at)).max(0.0) / dur
            };
            let body = r.bones[*bone].body;
            if share <= 0.0 || !self.released[body] {
                continue;
            }
            let rb = &mut self.w.bodies[self.handles[body]];
            rb.set_linvel(rb.linvel() + v3(*velocity, share / self.unit), true);
        }
    }

    /// The clip's turn of a bone against its parent at sample position `x`.
    pub fn clip(&self, r: &RagdollInput, bone: usize, x: f64) -> Option<[f64; 4]> {
        clip_at(&r.bones[bone].clip, x)
    }

    /// A bone's turn against its parent now.
    pub fn bone_turn(&self, r: &RagdollInput, bone: usize) -> Option<[f64; 4]> {
        let b = &r.bones[bone];
        let p = self.pose(r.bones[b.parent?].body);
        Some(quat_mul(p.inverse().q, self.pose(b.body).q))
    }
}
