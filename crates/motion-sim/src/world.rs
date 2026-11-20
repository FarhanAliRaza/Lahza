//! The rapier world a bake steps: bodies, colliders and joints made from a
//! `SimInput`, in metres.

use lahza_motion_core::mesh::{Mesh, MeshParams};
use lahza_motion_core::sim::{BodyInput, BodyKind, JointKind, Pose, ShapeInput, SimEvent, SimInput};
use rapier3d::prelude::*;

/// Gives a collider's mesh: the renderer's loader for glTF files, the
/// core's generator for the rest.
pub type Meshes<'a> = &'a mut dyn FnMut(&MeshParams) -> Result<Mesh, String>;

/// The core's generator alone (no files).
pub fn core_meshes(params: &MeshParams) -> Result<Mesh, String> {
    use lahza_motion_core::mesh::MeshShape;
    match &params.shape {
        MeshShape::Gltf { src, .. } => Err(format!("{src}: a glTF collider needs the renderer's loader")),
        MeshShape::Slang(_) => Err("a Slang mesh as a collider needs the renderer's generator".into()),
        _ => lahza_motion_core::mesh::generate(params),
    }
}

pub(crate) fn v3(v: [f64; 3], scale: f64) -> Vector {
    Vector::new((v[0] * scale) as f32, (v[1] * scale) as f32, (v[2] * scale) as f32)
}

pub(crate) fn quat(q: [f64; 4]) -> Rotation {
    Rotation::from_xyzw(q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32).normalize()
}

/// A rotation that takes x to `axis` (unit).
pub(crate) fn x_to(axis: [f64; 3]) -> Rotation {
    Rotation::from_rotation_arc(Vector::X, v3(axis, 1.0).normalize_or(Vector::X))
}

/// SplitMix64: the same numbers everywhere.
pub(crate) struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    /// −1..1.
    pub fn signed(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

}

pub struct World {
    pub w: PhysicsWorld,
    /// px per metre.
    pub unit: f64,
    /// One per `SimInput::bodies`.
    pub handles: Vec<RigidBodyHandle>,
    /// The body joints to the world hold on to.
    pub anchor: RigidBodyHandle,
    pub ground: Option<RigidBodyHandle>,
    /// Whether each dynamic body has been let go.
    pub released: Vec<bool>,
    /// One per `SimInput::joints`.
    pub joints: Vec<ImpulseJointHandle>,
    /// Seconds per step.
    pub dt: f64,
    /// One per `SimInput::ragdolls`.
    pub rigs: Vec<crate::ragdoll::Rig>,
    /// Per ragdoll: which of its bones may touch (`ragdoll::Groups`).
    pub(crate) touch: Vec<Vec<bool>>,
    /// Per ragdoll, per bone: a behaviour is placing it this step, and it
    /// passes through the rest of its body.
    pub(crate) free: Vec<Vec<bool>>,
    /// Per ragdoll, per bone: the one bone it passes through this step,
    /// plus one (an aimed forearm: the chest its arm hangs from); 0: none.
    pub(crate) slip: Vec<Vec<u32>>,
    /// Every body's speed before the last step (what it hit with).
    pub(crate) before: Vec<Vector>,
    /// Pairs of bodies touching after the last step (for first touches).
    touching: std::collections::BTreeSet<(u32, u32)>,
    /// Pairs of bodies (by handle) that never touch: a rope's links and
    /// what it is tied to.
    pub(crate) apart: std::collections::BTreeSet<(u32, u32)>,
    pub events: Vec<SimEvent>,
}

pub(crate) fn pose(p: &Pose, unit: f64) -> rapier3d::math::Pose {
    rapier3d::math::Pose::from_parts(v3(p.p, 1.0 / unit), quat(p.q))
}

/// The keyed pose at sample position `x` (fractional).
pub(crate) fn keyed_at(b: &BodyInput, x: f64) -> Pose {
    let last = b.keyed.len() - 1;
    let x = x.clamp(0.0, last as f64);
    let i = (x.floor() as usize).min(last);
    let j = (i + 1).min(last);
    let f = x - i as f64;
    let (a, c) = (&b.keyed[i], &b.keyed[j]);
    let dot: f64 = (0..4).map(|k| a.q[k] * c.q[k]).sum();
    let s = if dot < 0.0 { -1.0 } else { 1.0 };
    let mut q: [f64; 4] = std::array::from_fn(|k| a.q[k] + (s * c.q[k] - a.q[k]) * f);
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    q = q.map(|v| v / n);
    Pose { p: std::array::from_fn(|k| a.p[k] + (c.p[k] - a.p[k]) * f), q }
}

/// Speed (px/s) and spin (radians/s, the scene's axes) that carry pose
/// `a` to `b` in `dt` seconds.
pub(crate) fn velocity_between(a: &Pose, b: &Pose, dt: f64) -> ([f64; 3], [f64; 3]) {
    let v = std::array::from_fn(|k| (b.p[k] - a.p[k]) / dt);
    let inv = [-a.q[0], -a.q[1], -a.q[2], a.q[3]];
    let mut d = lahza_motion_core::sim::quat_mul(b.q, inv);
    if d[3] < 0.0 {
        d = d.map(|x| -x);
    }
    let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let w = if s < 1e-12 { [0.0; 3] } else { [0, 1, 2].map(|k| d[k] / s * 2.0 * libm::atan2(s, d[3]) / dt) };
    (v, w)
}

fn collider(c: &lahza_motion_core::sim::ColliderInput, unit: f64, meshes: Meshes, id: &str) -> Result<ColliderBuilder, String> {
    let m = 1.0 / unit;
    let rigid = |b: ColliderBuilder| {
        let (p, _) = lahza_motion_core::sim::decompose(c.local);
        b.position(pose(&p, unit))
    };
    let mut points = |params: &MeshParams| -> Result<(Vec<Vector>, Vec<[u32; 3]>), String> {
        let mesh = meshes(params).map_err(|e| format!("{id}: {e}"))?;
        let pts = mesh.positions.iter().map(|p| {
            let q = c.local.apply(p[0] as f64, p[1] as f64, p[2] as f64);
            v3([q[0], q[1], q[2]], m)
        });
        Ok((pts.collect(), mesh.tris))
    };
    Ok(match &c.shape {
        ShapeInput::Box { half } => rigid(ColliderBuilder::cuboid((half[0] * m) as f32, (half[1] * m) as f32, (half[2] * m) as f32)),
        ShapeInput::Ball { radius } => rigid(ColliderBuilder::ball((radius * m) as f32)),
        ShapeInput::Capsule { radius, half } => rigid(ColliderBuilder::capsule_y((half * m) as f32, (radius * m) as f32)),
        ShapeInput::Cylinder { radius, half } => rigid(ColliderBuilder::cylinder((half * m) as f32, (radius * m) as f32)),
        ShapeInput::Hull(params) => {
            let (pts, _) = points(params)?;
            ColliderBuilder::convex_hull(&pts).ok_or_else(|| format!("{id}: its mesh has no convex hull (it is flat or empty): give \"shape\" a box"))?
        }
        ShapeInput::Pieces(params) => {
            let (pts, tris) = points(params)?;
            if tris.is_empty() {
                return Err(format!("{id}: its mesh has no triangles to cut into pieces"));
            }
            ColliderBuilder::convex_decomposition(&pts, &tris)
        }
        ShapeInput::Trimesh(params) => {
            let (pts, tris) = points(params)?;
            ColliderBuilder::trimesh(pts, tris).map_err(|e| format!("{id}: its triangles make no collider: {e}"))?
        }
    })
}

impl World {
    /// The world at sample 0.
    pub fn new(input: &SimInput, meshes: Meshes) -> Result<World, String> {
        let unit = input.unit;
        let dt = 1.0 / (input.rate * input.substeps as f64);
        let mut w = PhysicsWorld::default();
        w.gravity = v3(input.gravity, 1.0);
        w.integration_parameters.dt = dt as f32;
        // Chains of joints (a spine, a leg, a rope) want more passes to hold.
        if !input.ragdolls.is_empty() || input.bodies.iter().any(|b| !b.apart.is_empty()) {
            w.integration_parameters.num_solver_iterations = 12;
        }
        let anchor = w.insert_body(RigidBodyBuilder::fixed());
        let ground = input.ground.as_ref().map(|g| {
            let (h, _) = w.insert(
                RigidBodyBuilder::fixed().translation(Vector::new(0.0, (g.y / unit) as f32, 0.0)),
                ColliderBuilder::new(SharedShape::halfspace(Vector::new(0.0, -1.0, 0.0))).friction(g.friction as f32).restitution(g.bounce as f32),
            );
            h
        });
        let mut handles = Vec::with_capacity(input.bodies.len());
        let mut released = Vec::with_capacity(input.bodies.len());
        for b in &input.bodies {
            let start = pose(&b.keyed[0], unit);
            let builder = match b.kind {
                BodyKind::Fixed => RigidBodyBuilder::fixed(),
                BodyKind::Kinematic => RigidBodyBuilder::kinematic_position_based(),
                BodyKind::Dynamic if b.release > 0 => RigidBodyBuilder::kinematic_position_based(),
                BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
            };
            let mut builder = builder.pose(start).linear_damping(b.damping as f32).angular_damping(b.spin_damping as f32).ccd_enabled(b.ccd);
            if input.flat {
                builder = builder.enabled_translations(true, true, false).enabled_rotations(false, false, true);
            }
            // (A crowd's agent's bones are out of the world until it is
            // knocked down.)
            let h = w.insert_body(builder.enabled(b.release != u32::MAX));
            // Mass: by density, or the mass given shared by volume.
            let mut made = Vec::new();
            for c in &b.colliders {
                made.push(collider(c, unit, meshes, &b.id)?.friction(b.friction as f32).restitution(b.bounce as f32).density(1.0).build());
            }
            let volume: f32 = made.iter().map(|c| c.mass()).sum();
            let density = match b.mass {
                Some(mass) if volume > 0.0 => mass as f32 / volume,
                _ => (b.density * 1000.0) as f32,
            };
            for mut c in made {
                c.set_density(density);
                if b.group != 0 || !b.apart.is_empty() {
                    c.user_data = b.group as u128;
                    c.set_active_hooks(ActiveHooks::FILTER_CONTACT_PAIRS);
                }
                // (A crowd's agent's bones: nothing until it is knocked down.)
                if b.release == u32::MAX {
                    c.set_enabled(false);
                }
                w.insert_collider(c, Some(h));
            }
            handles.push(h);
            released.push(b.kind == BodyKind::Dynamic && b.release == 0);
        }
        let mut apart = std::collections::BTreeSet::new();
        for (i, b) in input.bodies.iter().enumerate() {
            for &o in &b.apart {
                let (x, y) = (handles[i].into_raw_parts().0, handles[o].into_raw_parts().0);
                apart.insert((x.min(y), x.max(y)));
            }
        }
        let mut world = World { w, unit, handles, anchor, ground, released, joints: Vec::new(), dt, rigs: Vec::new(), touch: Vec::new(), free: Vec::new(), slip: Vec::new(), before: Vec::new(), touching: Default::default(), apart, events: Vec::new() };
        for (i, b) in input.bodies.iter().enumerate() {
            if world.released[i] {
                world.release(input, i, b);
            }
        }
        for j in &input.joints {
            let h = world.joint(input, j);
            world.joints.push(h);
        }
        world.add_ragdolls(input);
        Ok(world)
    }

    /// Lets body `i` go with the speed its keys had, the body's own, and
    /// the take's jitter.
    pub(crate) fn release(&mut self, input: &SimInput, i: usize, b: &BodyInput) {
        let (mut v, mut spin) = velocity_between(&b.before, &b.keyed[b.release as usize], 1.0 / input.rate);
        let mut rng = Rng(input.seed ^ (i as u64 + 1).wrapping_mul(0xa24baed4963ee407));
        for k in 0..3 {
            v[k] += b.velocity[k] + input.jitter * rng.signed();
            spin[k] += (b.spin[k] + input.jitter * rng.signed()).to_radians();
        }
        if input.flat {
            v[2] = 0.0;
            spin[0] = 0.0;
            spin[1] = 0.0;
        }
        let rb = &mut self.w.bodies[self.handles[i]];
        rb.set_body_type(RigidBodyType::Dynamic, true);
        rb.set_linvel(v3(v, 1.0 / self.unit), true);
        rb.set_angvel(v3(spin, 1.0), true);
        self.released[i] = true;
    }

    /// Puts body `i` in the world as a particle just born: where it is,
    /// with the speed of its flight (and the body's own, and the jitter).
    pub(crate) fn born(&mut self, input: &SimInput, i: usize, b: &BodyInput, life: &lahza_motion_core::sim::LifeInput) {
        let (mut v, mut spin) = velocity_between(&life.pose, &life.next, 1.0 / input.rate);
        let mut rng = Rng(input.seed ^ (i as u64 + 1).wrapping_mul(0xa24baed4963ee407) ^ (life.from as u64) << 32);
        for k in 0..3 {
            v[k] += b.velocity[k] + input.jitter * rng.signed();
            spin[k] += (b.spin[k] + input.jitter * rng.signed()).to_radians();
        }
        if input.flat {
            v[2] = 0.0;
            spin[0] = 0.0;
            spin[1] = 0.0;
        }
        let h = self.handles[i];
        let rb = &mut self.w.bodies[h];
        rb.set_enabled(true);
        rb.set_body_type(RigidBodyType::Dynamic, true);
        rb.set_position(pose(&life.pose, self.unit), true);
        rb.set_linvel(v3(v, 1.0 / self.unit), true);
        rb.set_angvel(v3(spin, 1.0), true);
        for c in rb.colliders().to_vec() {
            self.w.colliders[c].set_enabled(true);
        }
        self.released[i] = true;
    }

    /// Takes body `i` out of the world (its particle's life is over).
    pub(crate) fn gone(&mut self, i: usize) {
        let h = self.handles[i];
        let rb = &mut self.w.bodies[h];
        rb.set_enabled(false);
        for c in rb.colliders().to_vec() {
            self.w.colliders[c].set_enabled(false);
        }
        self.released[i] = false;
    }

    fn joint(&mut self, input: &SimInput, j: &lahza_motion_core::sim::JointInput) -> ImpulseJointHandle {
        let m = 1.0 / self.unit;
        let (h1, h2) = (self.handles[j.a], j.b.map(|b| self.handles[b]).unwrap_or(self.anchor));
        let (a1, a2) = (v3(j.anchor_a, m), v3(j.anchor_b, m));
        // The joint's frame on each body: x along the axis, the two frames
        // one in the world at the start (angles are from that pose).
        let f1 = rapier3d::math::Pose::from_parts(a1, x_to(j.axis_a));
        let p1 = pose(&input.bodies[j.a].keyed[0], self.unit);
        let p2 = j.b.map(|b| pose(&input.bodies[b].keyed[0], self.unit)).unwrap_or(rapier3d::math::Pose::identity());
        let mut f2 = p2.inverse() * (p1 * f1);
        f2.translation = a2;
        let rad = |d: f64| d.to_radians() as f32;
        let mut data: GenericJoint = match j.kind {
            JointKind::Fixed => FixedJointBuilder::new().local_frame1(f1).local_frame2(f2).build().into(),
            JointKind::Hinge => {
                let mut b = RevoluteJointBuilder::new(Vector::X);
                if let Some([lo, hi]) = j.limits {
                    b = b.limits([rad(lo), rad(hi)]);
                }
                b.build().into()
            }
            JointKind::Ball => {
                let mut b = SphericalJointBuilder::new();
                if let Some(s) = j.swing {
                    b = b.limits(JointAxis::AngY, [-rad(s), rad(s)]).limits(JointAxis::AngZ, [-rad(s), rad(s)]);
                }
                if let Some([lo, hi]) = j.twist {
                    b = b.limits(JointAxis::AngX, [rad(lo), rad(hi)]);
                }
                b.build().into()
            }
            JointKind::Slider => {
                let mut b = PrismaticJointBuilder::new(Vector::X);
                if let Some([lo, hi]) = j.limits {
                    b = b.limits([(lo * m) as f32, (hi * m) as f32]);
                }
                b.build().into()
            }
            JointKind::Rope => RopeJointBuilder::new((j.length * m) as f32).build().into(),
            JointKind::Spring => SpringJointBuilder::new((j.length * m) as f32, j.stiffness as f32, j.damping as f32).build().into(),
        };
        match j.kind {
            JointKind::Rope | JointKind::Spring => {
                data.set_local_anchor1(a1).set_local_anchor2(a2);
            }
            _ => {
                data.set_local_frame1(f1).set_local_frame2(f2);
            }
        }
        data.set_contacts_enabled(j.collide);
        self.w.insert_impulse_joint(h1, h2, data)
    }

    /// Sets what follows its keys, and the motors, for the step that ends
    /// at sample position `x`.
    pub fn drive(&mut self, input: &SimInput, x: f64) {
        for (i, b) in input.bodies.iter().enumerate() {
            let follows = b.kind == BodyKind::Kinematic || (b.kind == BodyKind::Dynamic && !self.released[i]);
            if follows && b.keyed.len() > 1 {
                self.w.bodies[self.handles[i]].set_next_kinematic_position(pose(&keyed_at(b, x), self.unit));
            }
        }
        for (j, h) in input.joints.iter().zip(&self.joints) {
            let Some(motor) = &j.motor else { continue };
            let last = motor.target.len() - 1;
            let xx = x.clamp(0.0, last as f64);
            let i = (xx.floor() as usize).min(last);
            let target = motor.target[i] + (motor.target[(i + 1).min(last)] - motor.target[i]) * (xx - i as f64);
            let Some(joint) = self.w.impulse_joints.get_mut(*h, true) else { continue };
            let (axis, target) = match j.kind {
                JointKind::Hinge => (JointAxis::AngX, target.to_radians()),
                _ => (JointAxis::LinX, target / self.unit),
            };
            joint.data.set_motor_position(axis, target as f32, motor.stiffness as f32, motor.damping as f32);
            if let Some(max) = motor.max_force {
                joint.data.set_motor_max_force(axis, max as f32);
            }
        }
    }

    /// Pushes and blasts that fall in the step from `t` to `t + dt`.
    pub fn push(&mut self, input: &SimInput, t: f64) {
        let dt = self.dt;
        let m = 1.0 / self.unit;
        let within = |at: f64, dur: f64| -> f64 {
            // The share of the push this step gives.
            if dur <= dt {
                if at >= t - 1e-9 && at < t + dt - 1e-9 { 1.0 } else { 0.0 }
            } else {
                ((t + dt).min(at + dur) - t.max(at)).max(0.0) / dur
            }
        };
        for (i, b) in input.bodies.iter().enumerate() {
            if !self.released[i] {
                continue;
            }
            for p in &b.pushes {
                let share = within(p.at, p.dur);
                if share <= 0.0 {
                    continue;
                }
                let rb = &mut self.w.bodies[self.handles[i]];
                let mass = rb.mass();
                let dv = v3(p.velocity, m * share);
                match p.point {
                    Some(point) => {
                        let at = rb.position().transform_point(v3(point, m));
                        rb.apply_impulse_at_point(dv * mass, at, true);
                    }
                    None => rb.set_linvel(rb.linvel() + dv, true),
                }
                let spin = v3(p.spin.map(f64::to_radians), share);
                if spin != Vector::ZERO {
                    rb.set_angvel(rb.angvel() + spin, true);
                }
            }
        }
        for blast in &input.blasts {
            if within(blast.at, 0.0) <= 0.0 || blast.radius <= 0.0 {
                continue;
            }
            let origin = v3(blast.origin, m);
            for (i, _) in input.bodies.iter().enumerate() {
                if !self.released[i] {
                    continue;
                }
                let rb = &mut self.w.bodies[self.handles[i]];
                let d = rb.center_of_mass() - origin;
                let dist = d.length();
                let reach = (blast.radius * m) as f32;
                if dist >= reach {
                    continue;
                }
                let dir = if dist > 1e-6 { d / dist } else { Vector::new(0.0, -1.0, 0.0) };
                rb.set_linvel(rb.linvel() + dir * ((blast.speed * m) as f32 * (1.0 - dist / reach)), true);
            }
        }
    }

    /// One step, from time `t`; first touches between bodies are noted.
    pub fn step(&mut self, input: &SimInput, t: f64) {
        let before: Vec<Vector> = self.handles.iter().map(|h| self.w.bodies[*h].linvel()).collect();
        self.before = before.clone();
        self.w.step_with_events(&crate::ragdoll::Groups(&self.touch, &self.free, &self.apart, &self.slip), &());
        let mut now = std::collections::BTreeSet::new();
        let index = |h: Option<RigidBodyHandle>| h.and_then(|h| self.handles.iter().position(|x| *x == h).map(|i| i as u32 + 1).or((Some(h) == self.ground).then_some(0)));
        for pair in self.w.contact_pairs() {
            if !pair.has_any_active_contact() {
                continue;
            }
            let (a, b) = (index(self.w.colliders[pair.collider1].parent()), index(self.w.colliders[pair.collider2].parent()));
            if let (Some(a), Some(b)) = (a, b) {
                let group = |i: u32| if i == 0 { 0 } else { input.bodies[i as usize - 1].group };
                // (A rope's links on each other are no event.)
                let link = |i: u32| i != 0 && !input.bodies[i as usize - 1].apart.is_empty();
                if a != b && (group(a) == 0 || group(a) != group(b)) && !(link(a) && link(b)) {
                    now.insert((a.min(b), a.max(b)));
                }
            }
        }
        for &(a, b) in now.difference(&self.touching) {
            let name = |i: u32| if i == 0 { "ground".to_string() } else { input.bodies[i as usize - 1].id.clone() };
            let vel = |i: u32| if i == 0 { Vector::ZERO } else { before[i as usize - 1] };
            let speed = (vel(a) - vel(b)).length() as f64 * self.unit;
            // (Things set down on each other at the start are no event.)
            if speed < 20.0 {
                continue;
            }
            // (Listed moving body first.)
            let (a, b) = if a == 0 { (b, a) } else { (a, b) };
            self.events.push(SimEvent { t: ((t + self.dt) * 1000.0).round() / 1000.0, kind: "touch".into(), a: name(a), b: name(b), speed: speed.round() });
        }
        self.touching = now;
    }

    /// A copy that goes its own way from here (a try ahead).
    pub fn fork(&self) -> World {
        let mut w = PhysicsWorld::default();
        w.gravity = self.w.gravity;
        w.integration_parameters = self.w.integration_parameters;
        w.islands = self.w.islands.clone();
        w.broad_phase = self.w.broad_phase.clone();
        w.narrow_phase = self.w.narrow_phase.clone();
        w.bodies = self.w.bodies.clone();
        w.colliders = self.w.colliders.clone();
        w.impulse_joints = self.w.impulse_joints.clone();
        w.multibody_joints = self.w.multibody_joints.clone();
        w.ccd_solver = self.w.ccd_solver.clone();
        World { w, unit: self.unit, handles: self.handles.clone(), anchor: self.anchor, ground: self.ground, released: self.released.clone(), joints: self.joints.clone(), dt: self.dt, rigs: self.rigs.clone(), touch: self.touch.clone(), free: self.free.clone(), slip: self.slip.clone(), before: self.before.clone(), touching: self.touching.clone(), apart: self.apart.clone(), events: Vec::new() }
    }

    /// Body `i`'s pose now, px.
    pub fn pose(&self, i: usize) -> Pose {
        let p = self.w.bodies[self.handles[i]].position();
        let (t, r) = (p.translation, p.rotation);
        Pose { p: [t.x as f64 * self.unit, t.y as f64 * self.unit, t.z as f64 * self.unit], q: [r.x as f64, r.y as f64, r.z as f64, r.w as f64] }
    }
}
