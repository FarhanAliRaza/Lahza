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
    /// Inside an instancer: the bake's track of each copy (`<id>#<copy>`,
    /// two instancers deep `<id>#<copy>#<copy>`).
    pub copies: Option<(Arc<Bake>, HashMap<[u32; 2], usize>)>,
    /// The layer has a `body` (else it is only placed by the bake: a
    /// ragdoll's bone).
    pub declared: bool,
}

impl CBody {
    /// For a layer whose children play a clip by the bake (a crowd's
    /// agent): the children's content time at the layer's content time `t`.
    pub(super) fn clock(&self, t: f64) -> Option<f64> {
        let (bake, track) = self.bake.as_ref()?;
        let tr = bake.tracks.get(*track)?;
        let x = bake.position(t - self.shift);
        (x >= tr.from as f64 - 1e-9).then(|| tr.clock_at(x)).flatten().map(|c| c + self.shift)
    }

    /// The layer's world matrix from the bake at content time `t`.
    pub(super) fn baked(&self, t: f64, copy: Option<([u32; 2], f64)>) -> Option<Mat4> {
        match copy {
            // (A copy plays `delay` later: the bake's time is the timeline's.)
            Some((k, delay)) => {
                let (bake, tracks) = self.copies.as_ref()?;
                bake.world(*tracks.get(&k)?, t + delay - self.shift)
            }
            None => {
                let (bake, track) = self.bake.as_ref()?;
                bake.world(*track, t - self.shift)
            }
        }
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
    /// Per crowd: when its clip starts (timeline seconds).
    pub(super) crowd_clips: Vec<Option<f64>>,
    /// Per crowd: when the character is in its guard's pose.
    pub(super) crowd_guards: Vec<Option<f64>>,
    /// And in each of its guard's other poses.
    pub(super) crowd_guard_more: Vec<Vec<f64>>,
    pub(super) ragdolls: Vec<super::ragdoll::CRagdoll>,
    pub bake: Option<Arc<Bake>>,
}

impl<'a> Compiler<'a> {
    pub(super) fn body(&mut self, c: &Common, id: &str, shift: f64) -> Option<Box<CBody>> {
        let bake = self.bake.as_ref().and_then(|b| Some((b.clone(), b.track(id)?)));
        let Some(spec) = c.body.clone() else {
            // A layer the bake has a track for (a ragdoll's bone).
            return bake.map(|bake| Box::new(CBody { spec: Body::default(), shift, release: None, pushes: Vec::new(), bake: Some(bake), copies: None, declared: false }));
        };
        // Its copies' tracks, when it is inside an instancer.
        let copies = self.bake.as_ref().and_then(|b| {
            let prefix = format!("{id}#");
            let mut tracks: HashMap<[u32; 2], usize> = HashMap::new();
            for (n, t) in b.tracks.iter().enumerate() {
                let Some(rest) = t.id.strip_prefix(&prefix) else { continue };
                let mut parts = rest.split('#').map(|k| k.parse::<u32>().ok());
                let key = match (parts.next().flatten(), parts.next(), parts.next()) {
                    (Some(a), None, _) => [a, u32::MAX],
                    (Some(a), Some(Some(b)), None) => [a, b],
                    _ => continue,
                };
                tracks.insert(key, n);
            }
            (!tracks.is_empty()).then(|| (b.clone(), tracks))
        });
        let release = spec.release.as_ref().and_then(|r| self.time(r, id, "body.release")).map(|t| t - shift);
        let pushes = spec.pushes.iter().enumerate().map(|(i, p)| self.time(&p.at, id, &format!("body.pushes.{i}.at")).unwrap_or(0.0) - shift).collect();
        Some(Box::new(CBody { spec, shift, release, pushes, bake, copies, declared: true }))
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
        let crowd_clips = settings.crowds.iter().enumerate().map(|(n, c)| c.clip.as_ref().and_then(|r| self.time(r, id, &format!("simulation.crowds.{n}.clip"))).map(|t| t - pre_roll)).collect();
        let crowd_guards = settings.crowds.iter().enumerate().map(|(n, c)| c.guard.as_ref().and_then(|g| self.time(&g.pose, id, &format!("simulation.crowds.{n}.guard.pose"))).map(|t| t - pre_roll)).collect();
        let crowd_guard_more = settings.crowds.iter().enumerate().map(|(n, c)| c.guard.iter().flat_map(|g| &g.poses).enumerate().filter_map(|(k, g)| self.time(&g.pose, id, &format!("simulation.crowds.{n}.guard.poses.{k}.pose"))).map(|t| t - pre_roll).collect()).collect();
        CSim { settings, start: from, end: to, motors, blasts, crowd_clips, crowd_guards, crowd_guard_more, ragdolls: self.ragdolls(scene, start, pre_roll), bake: self.bake.clone() }
    }
}

/// The content time a comp's children are at: the comp's, less its
/// offset; or, for a crowd's agent, what the bake says its clip is at.
pub(super) fn child_time(l: &CLayer, t: f64, tc: f64, space: &Space) -> f64 {
    let baked = if space.keyed { None } else { l.body.as_ref().and_then(|b| b.clock(t)) };
    baked.unwrap_or(tc) - l.offset
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
            visit(children, child_time(l, clock(step, tc), tc, space), step, &Space { world, ..*space }, below, f);
        }
    }
}

/// The copy of an instancer a layer is seen in: which, and for an
/// emitter's particle its age (None while its slot is empty: the layer is
/// still visited, where the emitter is).
#[derive(Clone, Copy)]
pub(super) struct Seen {
    pub path: [u32; 2],
    pub particle: bool,
    pub age: Option<f64>,
}

/// `visit`, into instancers' copies too: `f` is also given the copy a
/// layer is in (a body in copy 3 of an instancer is `<layer>#3`).
pub(super) fn visit_copies<'l>(layers: &'l [CLayer], tc: f64, inherited: Option<Step>, space: &Space, under: Option<usize>, seen: Option<Seen>, f: &mut dyn FnMut(&'l CLayer, Mat4, f64, Option<usize>, Option<Seen>) -> Option<usize>) {
    let mut cache = vec![None; layers.len()];
    for i in 0..layers.len() {
        let l = &layers[i];
        let world = space.world.then(layer_matrix(layers, i, tc, inherited, space, &mut cache));
        let step = l.step.or(inherited);
        let below = f(l, world, clock(step, tc), under, seen);
        match &l.kind {
            CKind::Comp(children) => visit_copies(children, child_time(l, clock(step, tc), tc, space), step, &Space { world, ..*space }, below, seen, f),
            CKind::Instancer(n) if n.is_emitter() => {
                // Every slot, whether it holds a particle now or not (a
                // body is found at every sample, in the same order).
                for (j, p) in n.particles(clock(step, tc)).into_iter().enumerate() {
                    let (m, age) = match p {
                        Some((m, age)) => (m, Some(age)),
                        None => (Mat4::IDENTITY, None),
                    };
                    let seen = Seen { path: [j as u32, u32::MAX], particle: true, age };
                    visit_copies(&n.layers, age.unwrap_or(0.0), step, &Space { world: world.then(m), copy: Some((seen.path, 0.0)), ..*space }, below, Some(seen), f);
                }
            }
            CKind::Instancer(n) => {
                for (k, m, delay) in super::instancer::copy_places(n, layers, i, tc, clock(step, tc), inherited, space, &mut cache) {
                    let copy = super::copy_in(space.copy, k as u32, delay);
                    let seen = Seen { path: copy.map(|c| c.0).unwrap_or([k as u32, u32::MAX]), particle: false, age: None };
                    visit_copies(&n.layers, tc - delay, step, &Space { world: world.then(m), copy, ..*space }, below, Some(seen), f);
                }
            }
            _ => {}
        }
    }
}

/// Bodies in copies that cannot be told apart: three instancers deep, or
/// in an emitter that is itself inside an instancer (or holds one).
fn copied_bodies(layers: &[CLayer], inside: u32, emitter: bool, out: &mut Vec<String>) {
    for l in layers {
        if (inside >= 3 || emitter && inside >= 2) && l.body.as_ref().is_some_and(|b| b.declared) {
            out.push(l.id.to_string());
        }
        let (deeper, emits) = match &l.kind {
            CKind::Instancer(n) => (1, n.is_emitter()),
            _ => (0, false),
        };
        copied_bodies(l.children(), inside + deeper, emitter || emits, out);
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
    /// The instancer's copy it is in.
    copy: Option<Seen>,
    /// A particle's age at each of `worlds` (None: its slot is empty).
    ages: Vec<Option<f64>>,
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
            vary: set.vary.clamp(0.0, 1.0),
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
        copied_bodies(&scene.layers, 0, false, &mut copied);
        for id in copied {
            errors.push(format!("{id}.body: a layer three instancers deep, or in an emitter that is inside an instancer or holds one, cannot be a body (the bake names a copy `<layer>#<copy>` or `<layer>#<copy>#<copy>`, and a particle by its slot): an instancer, an instancer in an instancer, or an emitter by itself can hold bodies"));
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
            visit_copies(&scene.layers, tc, None, &space, None, None, &mut |l, world, _, under, copy| {
                let Some(body) = l.body.as_deref().filter(|b| b.declared) else { return under };
                if k == 0 {
                    found.push(Found { layer: l, body, copy, worlds: Vec::with_capacity(samples as usize + 2), ages: Vec::new(), meshes: Vec::new() });
                }
                found[n].worlds.push(world);
                if let Some(Seen { particle: true, age, .. }) = copy {
                    found[n].ages.push(age);
                }
                n += 1;
                Some(n - 1)
            });
        }
        let sample_of = |t: f64| (((t - sim.start) * rate).round().max(0.0) as u32).min(samples);
        // A particle's lives: from the sample it is first seen at (its
        // age less than at the sample before) to the one it is gone at.
        let lives: Vec<Vec<(u32, u32)>> = found
            .iter()
            .map(|f| {
                let mut out: Vec<(u32, u32)> = Vec::new();
                // (`ages[k + 1]` is sample k's.)
                for k in 0..=samples as usize {
                    let (Some(Some(age)), was) = (f.ages.get(k + 1).copied(), f.ages.get(k).copied().flatten()) else { continue };
                    match out.last_mut() {
                        Some(last) if last.1 == k as u32 && was.is_some_and(|w| w < age) => last.1 = k as u32 + 1,
                        _ => out.push((k as u32, k as u32 + 1)),
                    }
                }
                out
            })
            .collect();
        let releases: Vec<u32> = found.iter().zip(&lives).map(|(f, lives)| if !f.ages.is_empty() { lives.first().map(|l| l.0).unwrap_or(0) } else if f.body.spec.kind == BodyKind::Dynamic { f.body.release.map(sample_of).unwrap_or(0) } else { 0 }).collect();
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
            visit_copies(&scene.layers, tc, None, &space, None, None, &mut |l, world, clock, under, _| {
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
        for ((f, &release), lives) in found.iter().zip(&releases).zip(&lives) {
            // (A copy's body is `<layer>#<copy>`.)
            let id = match f.copy.map(|c| c.path) {
                Some([a, u32::MAX]) => format!("{}#{a}", f.layer.id),
                Some([a, b]) => format!("{}#{a}#{b}", f.layer.id),
                None => f.layer.id.to_string(),
            };
            let particle = !f.ages.is_empty();
            if particle && f.body.spec.kind != BodyKind::Dynamic {
                errors.push(format!("{id}.body.type: an emitter's particle is a body the simulation moves (leave \"type\" out)"));
            }
            let (spec, id) = (&f.body.spec, id.as_str());
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
                BodyKind::Dynamic if particle => vec![poses[release as usize + 1]],
                BodyKind::Dynamic => poses[1..=release as usize + 1].to_vec(),
            };
            let lives: Vec<crate::sim::LifeInput> = lives.iter().map(|&(from, to)| {
                let pose = poses[from as usize + 1];
                // (A sample on by its own flight, while it is the same particle.)
                let next = if to > from + 1 || f.ages.get(from as usize + 2).copied().flatten().zip(f.ages[from as usize + 1]).is_some_and(|(n, a)| n > a) { poses[(from as usize + 2).min(poses.len() - 1)] } else { pose };
                crate::sim::LifeInput { from, to, pose, next }
            }).collect();
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
                release: if particle { u32::MAX } else { release },
                keyed,
                before: poses[if particle { release as usize + 1 } else { release as usize }],
                rest,
                pushes,
                group: 0,
                apart: Vec::new(),
                lives,
            });
        }
        // Joints.
        let index = |id: &str| input.bodies.iter().position(|b| b.id == id);
        let start_pose = |b: usize| decompose(found[b].worlds[1]).0;
        #[allow(clippy::type_complexity)]
        let mut chains: Vec<(String, usize, Option<usize>, [f64; 3], [f64; 3], [f64; 3], [f64; 3], f64, usize, f64, Option<f64>)> = Vec::new();
        let mut prev_half = [0.0; 3];
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
            if let Some(wrong) = rope_layer(&scene.layers, j) {
                errors.push(format!("{what}.draw: {wrong}"));
            }
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
            let length = j.length.unwrap_or(apart);
            if j.links > 0 {
                if j.kind != JointKind::Rope {
                    errors.push(format!("{what}.links: only a rope is made of links"));
                } else if j.links > 256 {
                    errors.push(format!("{what}.links: {} is more than a rope takes (256)", j.links));
                } else {
                    let radius = j.radius.or_else(|| rope_radius(&scene.layers, j)).unwrap_or(4.0).max(0.1);
                    chains.push((name.clone(), a, b, anchor_a, anchor_b, world_anchor, world_b, length, j.links as usize, radius, j.mass));
                }
            }
            input.joints.push(JointInput { id: name, kind: j.kind, a, b, anchor_a, anchor_b, axis_a, axis_b, limits: j.limits, swing: j.swing, twist: j.twist, length, stiffness: j.stiffness, damping: j.damping, motor, collide: j.collide });
        }
        // A rope of links: capsules end to end along the line (or the
        // hang) between its ends, a ball joint between each two and at
        // both ends. The rope joint stays: it is what bears the load.
        let g = set.gravity;
        let gl = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        let down = if gl > 1e-9 { g.map(|v| v / gl) } else { [0.0, 1.0, 0.0] };
        for (name, a, b, anchor_a, anchor_b, from, to, length, links, radius, mass) in chains {
            let points = rope_points(from, to, length, down, links);
            let first = input.bodies.len();
            for k in 0..links {
                let (p, q) = (points[k], points[k + 1]);
                let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                let pose = Pose { p: [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0], q: crate::sim::quat_y_to(d.map(|v| v / l)) };
                input.bodies.push(BodyInput {
                    id: format!("{name}/{k}"),
                    kind: BodyKind::Dynamic,
                    colliders: vec![ColliderInput { shape: ShapeInput::Capsule { radius, half: (l / 2.0 - radius).max(0.0) }, local: Mat4::IDENTITY }],
                    mass: mass.map(|m| m / links as f64),
                    density: 1.0,
                    friction: 0.6,
                    bounce: 0.0,
                    // (A cord's own stiffness: without it the links jitter.)
                    damping: 0.2,
                    spin_damping: 1.0,
                    velocity: [0.0; 3],
                    spin: [0.0; 3],
                    ccd: false,
                    release: 0,
                    keyed: vec![pose],
                    before: pose,
                    rest: Mat4::IDENTITY,
                    pushes: Vec::new(),
                    group: 0,
                    apart: std::iter::once(a).chain(b).collect(),
                    lives: Vec::new(),
                });
                let half = [0.0, l / 2.0, 0.0];
                let top = [0.0, -l / 2.0, 0.0];
                let ball = |id: String, a: usize, b: Option<usize>, anchor_a: [f64; 3], anchor_b: [f64; 3]| JointInput { id, kind: JointKind::Ball, a, b, anchor_a, anchor_b, axis_a: [0.0, 1.0, 0.0], axis_b: [0.0, 1.0, 0.0], limits: None, swing: None, twist: None, length: 0.0, stiffness: 0.0, damping: 0.0, motor: None, collide: false };
                // Its upper end to the link before (the first: to `a`).
                match k {
                    0 => input.joints.push(ball(format!("{name}/a"), first, Some(a), top, anchor_a)),
                    _ => input.joints.push(ball(format!("{name}/{k}"), first + k, Some(first + k - 1), top, prev_half)),
                }
                prev_half = half;
                if k + 1 == links {
                    input.joints.push(ball(format!("{name}/b"), first + k, b, half, anchor_b));
                }
            }
        }
        for (b, at) in set.blasts.iter().zip(&sim.blasts) {
            input.blasts.push(BlastInput { at: *at, origin: b.origin, speed: b.speed, radius: b.radius });
        }
        self.ragdoll_input(i, &mut input, &mut errors);
        self.crowd_input(i, &mut input, &mut errors);
        // A policy's file is part of what the bake is made from.
        files.extend(set.ragdolls.iter().filter_map(|r| match &r.controller {
            Some(crate::sim::Controller::Policy { src }) => Some(src.clone()),
            _ => None,
        }));
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

// ---------------------------------------------------------------- ropes

/// A layer drawn as a rope or spring: the joint's two ends, and what is
/// worked out once from where the bodies start.
pub(super) struct CRope {
    a: usize,
    b: Option<usize>,
    anchor: [f64; 3],
    anchor_b: Option<[f64; 3]>,
    length: Option<f64>,
    /// A rope hangs; a spring is straight.
    hangs: bool,
    /// The way things fall (unit).
    down: [f64; 3],
    /// Content time of the simulation's start.
    start: f64,
    /// The world's end when there is no `b`, and the length at the start.
    began: std::sync::OnceLock<([f64; 3], f64, Vec<f64>)>,
    /// A rope of links: the bake and its links' tracks, end to end from `a`.
    links: Option<(Arc<Bake>, Vec<usize>)>,
    /// Content time less this is the timeline's.
    shift: f64,
}

/// How many pieces a rope is drawn in.
const PIECES: usize = 12;

/// Binds the joints that are drawn to their layers (layers of the scene
/// itself, as their bodies are).
pub(super) fn bind_ropes(layers: &mut [CLayer], sim: &CSim, shift: f64) {
    for (n, j) in sim.settings.joints.iter().enumerate() {
        let Some(draw) = &j.draw else { continue };
        let name = j.id.clone().unwrap_or_else(|| format!("joint{n}"));
        let links = sim.bake.as_ref().filter(|_| j.links > 0).and_then(|b| Some((b.clone(), (0..j.links).map(|k| b.track(&format!("{name}/{k}"))).collect::<Option<Vec<usize>>>()?)));
        let at = |id: &str| layers.iter().position(|l| &*l.id == id);
        let (Some(rope), Some(a)) = (at(draw), at(&j.a)) else { continue };
        let b = j.b.as_deref().and_then(at);
        if j.b.is_some() != b.is_some() || !matches!(layers[rope].kind, CKind::Mesh(_)) {
            continue;
        }
        let g = sim.settings.gravity;
        let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        let down = if l > 1e-9 { g.map(|v| v / l) } else { [0.0, 1.0, 0.0] };
        layers[rope].rope = Some(Box::new(CRope { a, b, anchor: j.anchor, anchor_b: j.anchor_b, length: j.length, hangs: j.kind == JointKind::Rope, down, start: sim.start + shift, began: std::sync::OnceLock::new(), links, shift }));
    }
}

/// The drawn layer's radius, for a rope of links that gives none.
fn rope_radius(layers: &[CLayer], j: &crate::sim::Joint) -> Option<f64> {
    let draw = j.draw.as_ref()?;
    match &layers.iter().find(|l| &*l.id == draw.as_str())?.kind {
        CKind::Mesh(m) => m.across(),
        _ => None,
    }
}

/// `links + 1` points along a rope `length` long from `a` to `b`: evenly
/// along the line when it is taut, along the parabola it hangs in when
/// slack.
fn rope_points(a: [f64; 3], b: [f64; 3], length: f64, down: [f64; 3], links: usize) -> Vec<[f64; 3]> {
    let apart = (0..3).map(|k| (b[k] - a[k]).powi(2)).sum::<f64>().sqrt();
    let sag = crate::sim::rope_sag(length, apart);
    let at = |s: f64| -> [f64; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * s + down[k] * sag * 4.0 * s * (1.0 - s)) };
    if sag <= 0.5 {
        return (0..=links).map(|k| at(k as f64 / links as f64)).collect();
    }
    // Even steps along the curve, by its length.
    const FINE: usize = 512;
    let fine: Vec<[f64; 3]> = (0..=FINE).map(|i| at(i as f64 / FINE as f64)).collect();
    let mut along = vec![0.0; FINE + 1];
    for i in 1..=FINE {
        along[i] = along[i - 1] + (0..3).map(|k| (fine[i][k] - fine[i - 1][k]).powi(2)).sum::<f64>().sqrt();
    }
    let total = along[FINE];
    let mut out = Vec::with_capacity(links + 1);
    let mut i = 1;
    for k in 0..=links {
        let want = total * k as f64 / links as f64;
        while i < FINE && along[i] < want {
            i += 1;
        }
        let f = if along[i] > along[i - 1] { ((want - along[i - 1]) / (along[i] - along[i - 1])).clamp(0.0, 1.0) } else { 0.0 };
        out.push(std::array::from_fn(|c| fine[i - 1][c] + (fine[i][c] - fine[i - 1][c]) * f));
    }
    out
}

/// The layer's y along the line from `p` to `q`, its x and z across it,
/// stretched to reach (`long`: the mesh's own length).
fn piece(p: [f64; 3], q: [f64; 3], long: f64) -> Option<Mat4> {
    let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if l < 1e-9 {
        return None;
    }
    let y = d.map(|v| v / l);
    let other = if y[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let x = unit3([other[1] * y[2] - other[2] * y[1], other[2] * y[0] - other[0] * y[2], other[0] * y[1] - other[1] * y[0]]);
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let s = l / long;
    let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0];
    Some(Mat4([x[0], y[0] * s, z[0], m[0], x[1], y[1] * s, z[1], m[1], x[2], y[2] * s, z[2], m[2], 0.0, 0.0, 0.0, 1.0]))
}

/// What is wrong with a joint's `draw`, if anything.
fn rope_layer(layers: &[CLayer], j: &crate::sim::Joint) -> Option<String> {
    let draw = j.draw.as_ref()?;
    if !matches!(j.kind, JointKind::Rope | JointKind::Spring) {
        return Some("only ropes and springs are drawn".into());
    }
    for end in std::iter::once(&j.a).chain(&j.b) {
        if !layers.iter().any(|l| &*l.id == end.as_str()) {
            return Some(format!("'{end}' is inside another layer; a rope is drawn between layers of the scene itself"));
        }
    }
    match layers.iter().find(|l| &*l.id == draw.as_str()) {
        None => Some(format!("no layer '{draw}' in the scene itself (a mesh to draw the rope with)")),
        Some(l) => match &l.kind {
            CKind::Mesh(m) if m.long_way() => None,
            _ => Some(format!("'{draw}' is no cylinder, capsule or box mesh (its length along y is the rope's)")),
        },
    }
}

impl CRope {
    /// The world matrices of the rope's pieces at content time `tc`: the
    /// layer `drawn`, stretched along each.
    pub(super) fn pieces(&self, layers: &[CLayer], tc: f64, inherited: Option<Step>, space: &Space, cache: &mut [Option<Mat4>], drawn: &MeshState) -> Vec<Mat4> {
        let long = match &drawn.params.shape {
            crate::mesh::MeshShape::Cylinder { height, .. } | crate::mesh::MeshShape::Capsule { height, .. } => *height,
            crate::mesh::MeshShape::Box { size, .. } => size[1],
            _ => return Vec::new(),
        };
        if long <= 1e-9 {
            return Vec::new();
        }
        let point = |m: Mat4, p: [f64; 3]| {
            let v = m.apply(p[0], p[1], p[2]);
            [v[0], v[1], v[2]]
        };
        let (began_b, began_len, halves) = self.began.get_or_init(|| {
            // Where the bodies' keys have the ends at the simulation's start.
            let keyed = Space { keyed: true, ..*space };
            let mut fresh = vec![None; layers.len()];
            let a = point(keyed.world.then(layer_matrix(layers, self.a, self.start, inherited, &keyed, &mut fresh)), self.anchor);
            let b = match self.b {
                Some(b) => point(keyed.world.then(layer_matrix(layers, b, self.start, inherited, &keyed, &mut fresh)), self.anchor_b.unwrap_or([0.0; 3])),
                None => self.anchor_b.unwrap_or(a),
            };
            // Half each link's length: they start laid end to end from `a`.
            let mut halves = Vec::new();
            if let Some((bake, tracks)) = &self.links {
                let mut end = a;
                for &t in tracks {
                    let Some(s) = bake.tracks[t].samples.first() else { break };
                    let c = [s[0] as f64, s[1] as f64, s[2] as f64];
                    halves.push((0..3).map(|k| (c[k] - end[k]).powi(2)).sum::<f64>().sqrt());
                    end = std::array::from_fn(|k| 2.0 * c[k] - end[k]);
                }
            }
            (b, (0..3).map(|k| (b[k] - a[k]).powi(2)).sum::<f64>().sqrt(), halves)
        });
        let (began_b, began_len) = (*began_b, *began_len);
        let a = point(space.world.then(layer_matrix(layers, self.a, tc, inherited, space, cache)), self.anchor);
        let b = match self.b {
            Some(b) => point(space.world.then(layer_matrix(layers, b, tc, inherited, space, cache)), self.anchor_b.unwrap_or([0.0; 3])),
            None => began_b,
        };
        let apart = (0..3).map(|k| (b[k] - a[k]).powi(2)).sum::<f64>().sqrt();
        // A rope of links: from joint to joint, as the bake has its links.
        if let (Some((bake, tracks)), false) = (&self.links, space.keyed) {
            let x = bake.position(tc - self.shift);
            let ends: Vec<([f64; 3], [f64; 3])> = tracks.iter().zip(halves).filter_map(|(&t, &half)| {
                let pose = bake.tracks[t].pose(x)?;
                Some((pose.apply([0.0, -half, 0.0]), pose.apply([0.0, half, 0.0])))
            }).collect();
            if ends.len() == tracks.len() && !ends.is_empty() {
                let mut joints = Vec::with_capacity(ends.len() + 1);
                joints.push(a);
                for w in ends.windows(2) {
                    joints.push(std::array::from_fn(|k| (w[0].1[k] + w[1].0[k]) / 2.0));
                }
                joints.push(b);
                return joints.windows(2).filter_map(|w| piece(w[0], w[1], long)).collect();
            }
        }
        let length = self.length.unwrap_or(began_len);
        let sag = if self.hangs { crate::sim::rope_sag(length, apart) } else { 0.0 };
        let pieces = if sag > 0.5 { PIECES } else { 1 };
        let at = |s: f64| -> [f64; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * s + self.down[k] * sag * 4.0 * s * (1.0 - s)) };
        (0..pieces).filter_map(|i| piece(at(i as f64 / pieces as f64), at((i + 1) as f64 / pieces as f64), long)).collect()
    }
}

// ---------------------------------------------------------------- skins

fn skinned_meshes<'l>(layers: &'l [CLayer], out: &mut Vec<(&'l CLayer, &'l [Arc<str>])>, ids: &mut HashSet<&'l str>) {
    for l in layers {
        ids.insert(&l.id);
        if let CKind::Mesh(m) = &l.kind {
            if let Some((_, joints, _)) = &m.skin {
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
    pose_skins_in(&scene.layers, tc, None, space, layers)
}

/// `pose_skins` for the states of `under` (a scene's layers, or a comp's
/// children with the comp's space). Skins already posed are left alone.
pub(super) fn pose_skins_in(under: &[CLayer], tc: f64, inherited: Option<Step>, space: &Space, layers: &mut [LayerState]) {
    fn each(layers: &mut [LayerState], f: &mut dyn FnMut(&mut MeshState)) {
        for l in layers {
            match &mut l.content {
                Content::Mesh(m) if m.skin.as_ref().is_some_and(|s| s.joints.is_empty()) => f(m),
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
    visit(under, tc, inherited, space, None, &mut |l, world, _, _| {
        if wanted.contains(&l.id) {
            worlds.insert(l.id.clone(), world);
        }
        None
    });
    each(layers, &mut |m| {
        let Some(s) = &m.skin else { return };
        let to_mesh = inverse(m.world);
        let joints = s.ids.iter().map(|id| worlds.get(id).map(|w| to_mesh.then(*w)).unwrap_or(Mat4::IDENTITY)).collect();
        m.skin = Some(Arc::new(SkinPose { index: s.index, blend: s.blend, ids: s.ids.clone(), joints }));
    });
}
