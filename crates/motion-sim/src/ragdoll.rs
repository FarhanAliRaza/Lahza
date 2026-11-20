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
/// trails by a quarter second and swings. A limp joint keeps `LIMP` of
/// damping: flesh, not a hinge on a bearing.
pub const PULL: f64 = 9000.0;
pub const CALM: f64 = 190.0;
pub const LIMP: f64 = 4.0;

/// A ragdoll's joints in the world.
pub struct Rig {
    /// Per bone: its joint to its parent, and the joint's frame on the
    /// parent and on the bone (rotations).
    pub joints: Vec<Option<(ImpulseJointHandle, Rotation, Rotation)>>,
    /// Per bone: the motor's target at the last step (its speed is the
    /// change: a motor that only knew where to be would trail a moving
    /// target).
    last: Vec<Option<[f32; 3]>>,
}

/// Bodies of one group (a ragdoll's bones) do not collide with each
/// other: their capsules overlap where they join.
pub(crate) struct Groups;

impl PhysicsHooks for Groups {
    fn filter_contact_pair(&self, c: &PairFilterContext) -> Option<SolverFlags> {
        let (a, b) = (c.colliders[c.collider1].user_data, c.colliders[c.collider2].user_data);
        (a == 0 || a != b).then_some(SolverFlags::default())
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
            let mut rig = Rig { joints: Vec::with_capacity(r.bones.len()), last: vec![None; r.bones.len()] };
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
                let h = self.w.insert_impulse_joint(parent, child, data);
                rig.joints.push(Some((h, on_parent, on_child)));
            }
            self.rigs.push(rig);
        }
    }

    /// The clip's targets and each bone's strength, for the step that
    /// ends at sample position `x`. `targets`: per bone, a turn against
    /// its parent that stands for the clip's (a behaviour's, a search's).
    pub fn drive_ragdoll(&mut self, r: &RagdollInput, rig: usize, x: f64, targets: Option<&[Option<[f64; 4]>]>, scale: Option<&[f64]>) {
        for (k, b) in r.bones.iter().enumerate() {
            let Some((h, on_parent, on_child)) = self.rigs[rig].joints[k] else { continue };
            let strength = (sampled(&b.strength, x) * scale.and_then(|s| s.get(k).copied()).unwrap_or(1.0)).clamp(0.0, 1.0);
            let given = targets.and_then(|t| t.get(k).copied().flatten());
            let want = given.or_else(|| clip_at(&b.clip, x)).unwrap_or(b.bind);
            // In the joint's frames: nothing at the rest pose.
            let mut q = on_parent.inverse() * quat(want) * on_child;
            if q.w < 0.0 {
                q = -q;
            }
            let Some(joint) = self.w.impulse_joints.get_mut(h, strength > 0.0) else { continue };
            let (pull, calm) = ((PULL * strength) as f32, (LIMP + CALM * strength) as f32);
            // (A motor's place on an axis is twice the arc sine of the
            // turn's part about it.)
            let turn = [q.x, q.y, q.z].map(|v| 2.0 * v.clamp(-1.0, 1.0).asin());
            let dt = self.dt as f32;
            // (The clip's speed only: a behaviour's target is worked out
            // from the body's own state each step, and its change is not
            // a speed to follow.)
            let speed: [f32; 3] = match (self.rigs[rig].last[k], given) {
                (Some(was), None) => std::array::from_fn(|a| (turn[a] - was[a]) / dt),
                _ => [0.0; 3],
            };
            self.rigs[rig].last[k] = Some(turn);
            let axes: &[(JointAxis, usize)] = if b.hinge.is_some() { &[(JointAxis::AngX, 0)] } else { &[(JointAxis::AngX, 0), (JointAxis::AngY, 1), (JointAxis::AngZ, 2)] };
            for (axis, a) in axes {
                // (The limp part of the damping is against any turning,
                // the motor's toward the target's speed.)
                joint.data.set_motor(*axis, turn[*a], speed[*a] * (calm - LIMP as f32) / calm, pull, calm);
            }
        }
    }

    /// A ragdoll's hits that fall in the step from `t`.
    pub(crate) fn hit_ragdoll(&mut self, r: &RagdollInput, t: f64) {
        let dt = self.dt;
        for (at, bone, velocity, dur) in &r.hits {
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
