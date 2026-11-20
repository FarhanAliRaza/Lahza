//! Simulation in the compiled project: a body's layer reads its place
//! from the scene's bake (`crate::sim`), and `Compiled::sim_input` resolves
//! a scene's bodies, joints and pushes to the numbers a bake is made from.

use super::*;
use crate::sim::{decompose, inverse, Axis, Bake, BlastInput, Body, BodyInput, BodyKind, BodyShape, ColliderInput, JointInput, JointKind, MotorInput, Pose, PushInput, ShapeInput, SimInput, Simulation};
use crate::mesh::MeshParams;
use std::path::Path;

/// A layer's `body`, with its times resolved.
pub(super) struct CBody {
    pub spec: Body,
    /// The layer's content time is the timeline's plus this.
    pub shift: f64,
    /// Timeline seconds (a dynamic body's release; `None`: the start).
    pub release: Option<f64>,
    /// Timeline seconds of each of `spec.pushes`.
    pub pushes: Vec<f64>,
    pub bake: Option<(Arc<Bake>, usize)>,
    /// The layer has a `body` (else it is only placed by the bake: a
    /// ragdoll's bone).
    pub declared: bool,
}

impl CBody {
    /// The layer's world matrix from the bake at content time `t`.
    pub(super) fn baked(&self, t: f64) -> Option<Mat4> {
        let (bake, track) = self.bake.as_ref()?;
        bake.world(*track, t - self.shift)
    }
}

/// A scene's simulation, with its times resolved (timeline seconds).
pub struct CSim {
    pub settings: Simulation,
    pub start: f64,
    pub end: f64,
    /// Per joint: the motor's target.
    motors: Vec<Option<Prop<f64>>>,
    blasts: Vec<f64>,
    pub(super) ragdolls: Vec<super::ragdoll::CRagdoll>,
    pub bake: Option<Arc<Bake>>,
}

impl<'a> Compiler<'a> {
    pub(super) fn body(&mut self, c: &Common, id: &str, shift: f64) -> Option<Box<CBody>> {
        let bake = self.bake.as_ref().and_then(|b| Some((b.clone(), b.track(id)?)));
        let Some(spec) = c.body.clone() else {
            // A layer the bake has a track for (a ragdoll's bone).
            return bake.map(|bake| Box::new(CBody { spec: Body::default(), shift, release: None, pushes: Vec::new(), bake: Some(bake), declared: false }));
        };
        let release = spec.release.as_ref().and_then(|r| self.time(r, id, "body.release")).map(|t| t - shift);
        let pushes = spec.pushes.iter().enumerate().map(|(i, p)| self.time(&p.at, id, &format!("body.pushes.{i}.at")).unwrap_or(0.0) - shift).collect();
        Some(Box::new(CBody { spec, shift, release, pushes, bake, declared: true }))
    }

    /// The scene's simulation settings (the defaults when it only has
    /// bodies). `start` and `end` are the scene's, content time.
    pub(super) fn simulation(&mut self, scene: &Scene, start: f64, end: f64, pre_roll: f64) -> CSim {
        let id = scene.id.as_str();
        let settings = scene.simulation.clone().unwrap_or_default();
        // Scene times are the timeline's; references resolve to content time.
        let from = settings.start.as_ref().and_then(|r| self.time(r, id, "simulation.start")).map(|t| t - pre_roll).unwrap_or(start).max(start);
        let to = settings.end.as_ref().and_then(|r| self.time(r, id, "simulation.end")).map(|t| t - pre_roll).unwrap_or(end).min(end);
        let motors = settings.joints.iter().enumerate().map(|(i, j)| j.motor.as_ref().map(|m| self.prop(&m.target, id, &format!("simulation.joints.{i}.motor.target"), start, None, pre_roll))).collect();
        let blasts = settings.blasts.iter().enumerate().map(|(i, b)| self.time(&b.at, id, &format!("simulation.blasts.{i}.at")).unwrap_or(0.0) - pre_roll).collect();
        CSim { settings, start: from, end: to, motors, blasts, ragdolls: self.ragdolls(scene, start, pre_roll), bake: self.bake.clone() }
    }
}

/// Calls `f` for every layer under `layers` (comps' children too, not an
/// instancer's copies) with its world matrix as its keys place it at
/// content time `tc`, and its own clock.
pub(super) fn visit<'l>(layers: &'l [CLayer], tc: f64, inherited: Option<Step>, space: &Space, under: Option<usize>, f: &mut dyn FnMut(&'l CLayer, Mat4, f64, Option<usize>) -> Option<usize>) {
    let mut cache = vec![None; layers.len()];
    for i in 0..layers.len() {
        let l = &layers[i];
        let world = space.world.then(layer_matrix(layers, i, tc, inherited, space, &mut cache));
        let step = l.step.or(inherited);
        let below = f(l, world, clock(step, tc), under);
        if let CKind::Comp(children) = &l.kind {
            visit(children, tc - l.offset, step, &Space { world, ..*space }, below, f);
        }
    }
}

/// Bodies among an instancer's copies: `visit` does not reach them.
fn copied_bodies(layers: &[CLayer], inside: bool, out: &mut Vec<String>) {
    for l in layers {
        if inside && l.body.as_ref().is_some_and(|b| b.declared) {
            out.push(l.id.to_string());
        }
        copied_bodies(l.children(), inside || matches!(l.kind, CKind::Instancer(_)), out);
    }
}

/// A scene's simulation against its bake.
#[derive(Clone, Debug, PartialEq)]
pub enum SimStatus {
    /// Nothing to simulate.
    None,
    /// The document's simulation is wrong: what, per problem.
    Broken(Vec<String>),
    /// Never baked.
    Unbaked { bodies: usize },
    /// Baked from something else than the document now says.
    Stale { bodies: usize },
    Fresh { bodies: usize },
}

fn has_bodies(layers: &[CLayer]) -> bool {
    layers.iter().any(|l| l.body.as_ref().is_some_and(|b| b.declared) || has_bodies(l.children()))
}

fn dir3(m: &Mat4, v: [f64; 3]) -> [f64; 3] {
    let a = m.0;
    std::array::from_fn(|r| a[r * 4] * v[0] + a[r * 4 + 1] * v[1] + a[r * 4 + 2] * v[2])
}

fn unit3(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 1e-12 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn point3(m: &Mat4, v: [f64; 3]) -> [f64; 3] {
    let p = m.apply(v[0], v[1], v[2]);
    [p[0], p[1], p[2]]
}

/// A round or box shape under `m` (shape space → body frame): its sizes
/// scaled, its placement rigid. `None` when `m` scales it unevenly in a
/// way the shape cannot take (a sphere squashed, anything sheared).
fn place(shape: ShapeInput, m: Mat4) -> Option<ColliderInput> {
    let (pose, rest) = decompose(m);
    let r = rest.0;
    let s = [r[0].abs(), r[5].abs(), r[10].abs()];
    let size = s[0].max(s[1]).max(s[2]).max(1e-12);
    if [r[1], r[2], r[4], r[6], r[8], r[9]].iter().any(|v| v.abs() > 1e-6 * size) {
        return None;
    }
    let even = |a: f64, b: f64| (a - b).abs() <= 1e-6 * size;
    let shape = match shape {
        ShapeInput::Box { half } => ShapeInput::Box { half: [half[0] * s[0], half[1] * s[1], half[2] * s[2]] },
        ShapeInput::Ball { radius } if even(s[0], s[1]) && even(s[1], s[2]) => ShapeInput::Ball { radius: radius * s[0] },
        ShapeInput::Capsule { radius, half } if even(s[0], s[2]) => ShapeInput::Capsule { radius: radius * s[0], half: half * s[1] },
        ShapeInput::Cylinder { radius, half } if even(s[0], s[2]) => ShapeInput::Cylinder { radius: radius * s[0], half: half * s[1] },
        _ => return None,
    };
    Some(ColliderInput { shape, local: pose.matrix() })
}

/// A collider given outright (`BodyShape`'s round and box kinds) on a
/// layer whose scale and anchor are `rest` (the layer's space → the
/// body's frame).
pub(super) fn given_shape(shape: &BodyShape, rest: Mat4) -> Option<ColliderInput> {
    let given = |shape: ShapeInput, offset: &[f64; 3], turn: Mat4| place(shape, rest.then(Mat4::translate(offset[0], offset[1], offset[2])).then(turn));
    let lay = |axis: &Axis| match axis {
        Axis::X => Mat4::rotate_z(-90.0),
        Axis::Y => Mat4::IDENTITY,
        Axis::Z => Mat4::rotate_x(-90.0),
    };
    match shape {
        BodyShape::Box { size, offset } => given(ShapeInput::Box { half: [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0] }, offset, Mat4::IDENTITY),
        BodyShape::Sphere { radius, offset } => given(ShapeInput::Ball { radius: *radius }, offset, Mat4::IDENTITY),
        BodyShape::Capsule { radius, height, axis, offset } => given(ShapeInput::Capsule { radius: *radius, half: (height / 2.0 - radius).max(0.0) }, offset, lay(axis)),
        BodyShape::Cylinder { radius, height, axis, offset } => given(ShapeInput::Cylinder { radius: *radius, half: height / 2.0 }, offset, lay(axis)),
        BodyShape::Hull | BodyShape::Pieces | BodyShape::Mesh => None,
    }
}

/// The collider a mesh is by itself: a round or box shape when the mesh
/// is one, else its hull (or its triangles when it does not move).
fn mesh_collider(params: &Arc<MeshParams>, m: Mat4, dynamic: bool, forced: Option<&BodyShape>) -> ColliderInput {
    use crate::mesh::MeshShape;
    let plain = params.displace.is_none() && params.deform.is_none() && forced.is_none();
    let simple = match &params.shape {
        MeshShape::Sphere { radius } if plain => Some(ShapeInput::Ball { radius: *radius }),
        MeshShape::Box { size, .. } if plain => Some(ShapeInput::Box { half: [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0] }),
        MeshShape::Capsule { radius, height } if plain => Some(ShapeInput::Capsule { radius: *radius, half: (height / 2.0 - radius).max(0.0) }),
        MeshShape::Cylinder { radius, top, height, .. } if plain && top == radius => Some(ShapeInput::Cylinder { radius: *radius, half: height / 2.0 }),
        // A sheet: a box a px thick each way.
        MeshShape::Plane { size } if plain => Some(ShapeInput::Box { half: [size[0] / 2.0, size[1] / 2.0, 1.0] }),
        _ => None,
    };
    if let Some(c) = simple.and_then(|s| place(s, m)) {
        return c;
    }
    let params = Arc::new(crate::sim::collider_params(params));
    let shape = match forced {
        Some(BodyShape::Mesh) => ShapeInput::Trimesh(params),
        Some(BodyShape::Pieces) => ShapeInput::Pieces(params),
        Some(_) => ShapeInput::Hull(params),
        None if dynamic => ShapeInput::Hull(params),
        None => ShapeInput::Trimesh(params),
    };
    ColliderInput { shape, local: m }
}

/// A body as gathered from the layers.
struct Found<'l> {
    layer: &'l CLayer,
    body: &'l CBody,
    /// Keyed world matrices: index 0 is one sample before the start.
    worlds: Vec<Mat4>,
    /// The meshes that are its shape: each with its world at the release.
    meshes: Vec<(Arc<MeshParams>, Mat4)>,
}

impl Compiled {
    /// Whether scene `i` has anything to simulate.
    pub fn has_simulation(&self, i: usize) -> bool {
        self.scenes.get(i).is_some_and(|s| has_bodies(&s.layers) || !s.sim.settings.crowds.is_empty() || !s.sim.settings.ragdolls.is_empty())
    }

    /// Scene `i`'s simulation against the bake it was compiled with.
    pub fn sim_status(&self, i: usize, dir: Option<&Path>) -> SimStatus {
        if !self.has_simulation(i) {
            return SimStatus::None;
        }
        let (input, errors) = self.sim_input(i, dir);
        if !errors.is_empty() {
            return SimStatus::Broken(errors);
        }
        let bodies = input.bodies.len();
        if input.is_empty() {
            return SimStatus::None;
        }
        match &self.scenes[i].sim.bake {
            None => SimStatus::Unbaked { bodies },
            // Without the folder, files colliders come from cannot be
            // compared: the rest still is.
            Some(b) if b.hash == input.hash() || (dir.is_none() && !input.files.is_empty()) => SimStatus::Fresh { bodies },
            Some(_) => SimStatus::Stale { bodies },
        }
    }

    /// A body layer's place from its keys alone (no bake) at timeline
    /// time `t`: its world matrix in the scene's space.
    pub fn keyed_world(&self, scene: usize, id: &str, t: f64) -> Option<Mat4> {
        let s = self.scenes.get(scene)?;
        let tc = t + s.pre_roll;
        let space = Space { keyed: true, ..self.camera(s, tc) };
        let mut out = None;
        visit(&s.layers, tc, None, &space, None, &mut |l, world, _, _| {
            if &*l.id == id {
                out = Some(world);
            }
            None
        });
        out
    }

    /// Everything scene `i`'s bake is made from, and what is wrong with
    /// it (unknown bodies in joints, layers that have no shape). `dir` is
    /// the project's folder: files colliders are read from are hashed.
    pub fn sim_input(&self, i: usize, dir: Option<&Path>) -> (SimInput, Vec<String>) {
        let scene = &self.scenes[i];
        let sim = &scene.sim;
        let set = &sim.settings;
        let mut errors = Vec::new();
        let rate = self.settings.fps.as_f64();
        let samples = ((sim.end - sim.start) * rate - 1e-9).ceil().max(0.0) as u32;
        let substeps = (set.rate / rate - 1e-9).ceil().max(1.0) as u32;
        let mut input = SimInput {
            scene: scene.id.to_string(),
            start: sim.start,
            rate,
            samples,
            substeps,
            unit: set.unit,
            gravity: set.gravity,
            seed: set.seed,
            jitter: set.jitter,
            flat: set.flat,
            ground: set.ground.clone(),
            bodies: Vec::new(),
            joints: Vec::new(),
            blasts: Vec::new(),
            ragdolls: Vec::new(),
            crowds: Vec::new(),
            files: Vec::new(),
        };
        let mut copied = Vec::new();
        copied_bodies(&scene.layers, false, &mut copied);
        for id in copied {
            errors.push(format!("{id}.body: a layer inside an instancer cannot be a body (its copies share one layer): make the bodies layers of their own"));
        }
        if !(set.unit > 0.0) {
            errors.push(format!("{}.simulation.unit must be positive", scene.id));
            input.unit = 100.0;
        }
        // The bodies, in the document's order, with their keyed worlds at
        // every sample (and one before the first, for the speed there).
        let mut found: Vec<Found> = Vec::new();
        for k in 0..=samples as usize + 1 {
            let t = sim.start + (k as f64 - 1.0) / rate;
            let tc = t + scene.pre_roll;
            let space = Space { keyed: true, ..self.camera(scene, tc) };
            let mut n = 0;
            visit(&scene.layers, tc, None, &space, None, &mut |l, world, _, under| {
                let Some(body) = l.body.as_deref().filter(|b| b.declared) else { return under };
                if k == 0 {
                    found.push(Found { layer: l, body, worlds: Vec::with_capacity(samples as usize + 2), meshes: Vec::new() });
                }
                found[n].worlds.push(world);
                n += 1;
                Some(n - 1)
            });
        }
        let sample_of = |t: f64| (((t - sim.start) * rate).round().max(0.0) as u32).min(samples);
        let releases: Vec<u32> = found.iter().map(|f| if f.body.spec.kind == BodyKind::Dynamic { f.body.release.map(sample_of).unwrap_or(0) } else { 0 }).collect();
        // Each body's meshes (its own, or those under it that are no body
        // of their own), as they are at its release.
        let mut by_release: Vec<u32> = releases.clone();
        by_release.sort_unstable();
        by_release.dedup();
        for r in by_release {
            let t = sim.start + r as f64 / rate;
            let tc = t + scene.pre_roll;
            let space = Space { keyed: true, ..self.camera(scene, tc) };
            let mut n = 0;
            visit(&scene.layers, tc, None, &space, None, &mut |l, world, clock, under| {
                let own = l.body.as_ref().is_some_and(|b| b.declared).then(|| {
                    n += 1;
                    n - 1
                });
                let owner = own.or(under);
                if let (Some(b), CKind::Mesh(m)) = (owner, &l.kind) {
                    if releases[b] == r {
                        found[b].meshes.push((m.params_at(clock, 1.0), world));
                    }
                }
                owner
            });
        }
        let mut files: Vec<String> = Vec::new();
        for (f, &release) in found.iter().zip(&releases) {
            let (spec, id) = (&f.body.spec, &*f.layer.id);
            let dynamic = spec.kind == BodyKind::Dynamic;
            let world = f.worlds[release as usize + 1];
            let (pose, rest) = decompose(world);
            let to_body = inverse(pose.matrix());
            let mut colliders = Vec::new();
            match &spec.shape {
                Some(shape @ (BodyShape::Box { .. } | BodyShape::Sphere { .. } | BodyShape::Capsule { .. } | BodyShape::Cylinder { .. })) => colliders.extend(given_shape(shape, rest)),
                forced => {
                    if matches!(forced, Some(BodyShape::Mesh)) && dynamic {
                        errors.push(format!("{id}.body.shape: \"mesh\" (the triangles themselves) is for bodies that do not move by the simulation: use \"hull\", or type \"fixed\" or \"kinematic\""));
                    }
                    for (params, w) in &f.meshes {
                        let c = mesh_collider(params, to_body.then(*w), dynamic, forced.as_ref());
                        if let (ShapeInput::Hull(_) | ShapeInput::Trimesh(_) | ShapeInput::Pieces(_), crate::mesh::MeshShape::Gltf { src, .. }) = (&c.shape, &params.shape) {
                            files.push(src.clone());
                        }
                        colliders.push(c);
                    }
                    if f.meshes.is_empty() {
                        // A flat shape as a thin body.
                        if let CKind::Shape(s) = &f.layer.kind {
                            let size = s.size.at(sim.start + release as f64 / rate + f.body.shift);
                            let flat = match s.shape {
                                ShapeKind::Rect | ShapeKind::Field => Some(ShapeInput::Box { half: [size.x() / 2.0, size.y() / 2.0, 10.0] }),
                                ShapeKind::Circle if (size.x() - size.y()).abs() < 1e-6 => Some(ShapeInput::Ball { radius: size.x() / 2.0 }),
                                _ => None,
                            };
                            colliders.extend(flat.and_then(|s| place(s, rest)));
                        }
                    }
                }
            }
            if colliders.is_empty() {
                errors.push(format!("{id}.body: nothing to collide with: the layer is no mesh (nor a comp of meshes, a rect or a circle) and gives no \"shape\", or its shape is scaled unevenly (give \"shape\": {{\"type\": \"hull\"}})"));
            }
            let poses: Vec<Pose> = f.worlds.iter().map(|w| decompose(*w).0).collect();
            let moves = poses.windows(2).any(|w| w[0] != w[1]);
            let keyed = match spec.kind {
                BodyKind::Fixed => vec![poses[1]],
                BodyKind::Kinematic if !moves => vec![poses[1]],
                BodyKind::Kinematic => poses[1..].to_vec(),
                BodyKind::Dynamic => poses[1..=release as usize + 1].to_vec(),
            };
            let pushes = spec.pushes.iter().zip(&f.body.pushes).map(|(p, at)| PushInput { at: *at, dur: p.dur.max(0.0), velocity: p.velocity, spin: p.spin, point: p.point.map(|v| point3(&rest, v)) }).collect();
            input.bodies.push(BodyInput {
                id: id.to_string(),
                kind: spec.kind,
                colliders,
                mass: spec.mass,
                density: spec.density,
                friction: spec.friction,
                bounce: spec.bounce,
                damping: spec.damping,
                spin_damping: spec.spin_damping,
                velocity: spec.velocity,
                spin: spec.spin,
                ccd: spec.ccd,
                release,
                keyed,
                before: poses[release as usize],
                rest,
                pushes,
                group: 0,
            });
        }
        // Joints.
        let index = |id: &str| input.bodies.iter().position(|b| b.id == id);
        let start_pose = |b: usize| decompose(found[b].worlds[1]).0;
        for (n, j) in set.joints.iter().enumerate() {
            let name = j.id.clone().unwrap_or_else(|| format!("joint{n}"));
            let what = format!("{}.simulation.joints.{n}", scene.id);
            let Some(a) = index(&j.a) else {
                errors.push(format!("{what}.a: no body '{}' in the scene (a layer with \"body\")", j.a));
                continue;
            };
            let b = match &j.b {
                None => None,
                Some(id) => match index(id) {
                    Some(b) if b != a => Some(b),
                    Some(_) => {
                        errors.push(format!("{what}: a and b are the same body"));
                        continue;
                    }
                    None => {
                        errors.push(format!("{what}.b: no body '{id}' in the scene (a layer with \"body\")"));
                        continue;
                    }
                },
            };
            let (pa, rest_a) = (start_pose(a), input.bodies[a].rest);
            let anchor_a = point3(&rest_a, j.anchor);
            let axis_a = unit3(dir3(&rest_a, j.axis));
            let world_anchor = pa.apply(anchor_a);
            let world_axis = crate::sim::quat_rotate(pa.q, axis_a);
            let springy = matches!(j.kind, JointKind::Rope | JointKind::Spring);
            let (anchor_b, axis_b) = match b {
                Some(b) => {
                    let pb = start_pose(b).inverse();
                    let anchor = match (j.anchor_b, springy) {
                        (Some(v), _) => point3(&input.bodies[b].rest, v),
                        (None, true) => [0.0; 3],
                        (None, false) => pb.apply(world_anchor),
                    };
                    (anchor, crate::sim::quat_rotate(pb.q, world_axis))
                }
                None => (j.anchor_b.unwrap_or(world_anchor), world_axis),
            };
            let world_b = match b {
                Some(b) => start_pose(b).apply(anchor_b),
                None => anchor_b,
            };
            let apart = (0..3).map(|k| (world_b[k] - world_anchor[k]).powi(2)).sum::<f64>().sqrt();
            if j.motor.is_some() && !matches!(j.kind, JointKind::Hinge | JointKind::Slider) {
                errors.push(format!("{what}.motor: only hinges and sliders have motors"));
            }
            let motor = j.motor.as_ref().zip(sim.motors.get(n).and_then(|m| m.as_ref())).map(|(m, target)| MotorInput {
                target: (0..=samples).map(|k| target.at(sim.start + k as f64 / rate + scene.pre_roll)).collect(),
                stiffness: m.stiffness,
                damping: m.damping,
                max_force: m.max_force,
            });
            input.joints.push(JointInput { id: name, kind: j.kind, a, b, anchor_a, anchor_b, axis_a, axis_b, limits: j.limits, swing: j.swing, twist: j.twist, length: j.length.unwrap_or(apart), stiffness: j.stiffness, damping: j.damping, motor, collide: j.collide });
        }
        for (b, at) in set.blasts.iter().zip(&sim.blasts) {
            input.blasts.push(BlastInput { at: *at, origin: b.origin, speed: b.speed, radius: b.radius });
        }
        self.ragdoll_input(i, &mut input, &mut errors);
        if !set.crowds.is_empty() {
            errors.push(format!("{}.simulation.crowds: crowds are not built yet", scene.id));
        }
        files.sort();
        files.dedup();
        if let Some(dir) = dir {
            input.files = files.into_iter().map(|src| (src.clone(), crate::sim::file_hash(&dir.join(&src)))).collect();
        } else {
            input.files = files.into_iter().map(|src| (src, 0)).collect();
        }
        (input, errors)
    }
}

// ---------------------------------------------------------------- skins

fn skinned_meshes<'l>(layers: &'l [CLayer], out: &mut Vec<(&'l CLayer, &'l [Arc<str>])>, ids: &mut HashSet<&'l str>) {
    for l in layers {
        ids.insert(&l.id);
        if let CKind::Mesh(m) = &l.kind {
            if let Some((_, joints)) = &m.skin {
                out.push((l, joints));
            }
        }
        skinned_meshes(l.children(), out, ids);
    }
}

/// Whether the scene has skinned meshes; joints that are no layer of the
/// scene are errors.
pub(super) fn check_skins(layers: &[CLayer], errors: &mut Vec<String>) -> bool {
    let (mut meshes, mut ids) = (Vec::new(), HashSet::new());
    skinned_meshes(layers, &mut meshes, &mut ids);
    for (l, joints) in &meshes {
        let missing: Vec<&str> = joints.iter().map(|j| &**j).filter(|j| !ids.contains(j)).collect();
        if let Some(first) = missing.first() {
            errors.push(format!("{}.skin.joints: '{first}' is no layer of the scene{}", l.id, if missing.len() > 1 { format!(" (nor are {} more)", missing.len() - 1) } else { String::new() }));
        }
    }
    !meshes.is_empty()
}

/// Gives every skinned mesh of a frame its joints' places: each joint
/// layer's space → the mesh layer's, as keys, parents and bakes have them.
pub(super) fn pose_skins(scene: &CScene, tc: f64, space: &Space, layers: &mut [LayerState]) {
    fn each(layers: &mut [LayerState], f: &mut dyn FnMut(&mut MeshState)) {
        for l in layers {
            match &mut l.content {
                Content::Mesh(m) if m.skin.is_some() => f(m),
                Content::Group(children) => each(children, f),
                _ => {}
            }
            if let Some(src) = l.matte.as_mut().and_then(|m| m.source.as_mut()) {
                each(std::slice::from_mut(src), f);
            }
        }
    }
    let mut wanted: HashSet<Arc<str>> = HashSet::new();
    each(layers, &mut |m| {
        if let Some(s) = &m.skin {
            wanted.extend(s.ids.iter().cloned());
        }
    });
    if wanted.is_empty() {
        return;
    }
    let mut worlds: HashMap<Arc<str>, Mat4> = HashMap::with_capacity(wanted.len());
    visit(&scene.layers, tc, None, space, None, &mut |l, world, _, _| {
        if wanted.contains(&l.id) {
            worlds.insert(l.id.clone(), world);
        }
        None
    });
    each(layers, &mut |m| {
        let Some(s) = &m.skin else { return };
        let to_mesh = inverse(m.world);
        let joints = s.ids.iter().map(|id| worlds.get(id).map(|w| to_mesh.then(*w)).unwrap_or(Mat4::IDENTITY)).collect();
        m.skin = Some(Arc::new(SkinPose { index: s.index, ids: s.ids.clone(), joints }));
    });
}
