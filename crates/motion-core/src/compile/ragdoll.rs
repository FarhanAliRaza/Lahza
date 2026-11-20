//! Ragdolls in the compiled project: a skeleton's bone layers resolved to
//! bodies (a capsule each), the joints between them, and the clip their
//! motors follow.

use super::sim::visit;
use super::*;
use crate::sim::{decompose, quat_mul, quat_rotate, BehaviourInput, BodyInput, BodyKind, BoneInput, ColliderInput, Pose, RagdollInput, ShapeInput, SimInput};

/// A ragdoll's times and keyed numbers, resolved.
pub(super) struct CRagdoll {
    /// Timeline seconds.
    release: Option<f64>,
    rest: Option<f64>,
    strength: Prop<f64>,
    strengths: Vec<Prop<f64>>,
    /// (from, to, amount) per behaviour.
    behaviours: Vec<(Option<f64>, Option<f64>, Prop<f64>)>,
    hits: Vec<f64>,
}

impl<'a> Compiler<'a> {
    pub(super) fn ragdolls(&mut self, scene: &Scene, start: f64, pre_roll: f64) -> Vec<CRagdoll> {
        let id = scene.id.as_str();
        let Some(sim) = &scene.simulation else { return Vec::new() };
        sim.ragdolls
            .iter()
            .enumerate()
            .map(|(n, r)| {
                let what = |f: &str| format!("simulation.ragdolls.{n}.{f}");
                let time = |this: &mut Self, t: &Option<TimeRef>, f: &str| t.as_ref().and_then(|t| this.time(t, id, &what(f))).map(|t| t - pre_roll);
                CRagdoll {
                    release: time(self, &r.release, "release"),
                    rest: time(self, &r.rest, "rest"),
                    strength: self.prop(&r.strength, id, &what("strength"), start, None, pre_roll),
                    strengths: r.strengths.iter().enumerate().map(|(k, s)| self.prop(&s.strength, id, &what(&format!("strengths.{k}.strength")), start, None, pre_roll)).collect(),
                    behaviours: r
                        .behaviours
                        .iter()
                        .enumerate()
                        .map(|(k, b)| (time(self, &b.from, &format!("behaviours.{k}.from")), time(self, &b.to, &format!("behaviours.{k}.to")), self.prop(&b.amount, id, &what(&format!("behaviours.{k}.amount")), start, None, pre_roll)))
                        .collect(),
                    hits: r.hits.iter().enumerate().map(|(k, h)| self.time(&h.at, id, &what(&format!("hits.{k}.at"))).unwrap_or(0.0) - pre_roll).collect(),
                }
            })
            .collect()
    }
}

/// The layers as a flat tree: each with the index of the layer it is
/// inside (comps only).
fn tree<'l>(layers: &'l [CLayer], inside: Option<usize>, out: &mut Vec<(&'l CLayer, Option<usize>)>) {
    for l in layers {
        out.push((l, inside));
        let me = out.len() - 1;
        if let CKind::Comp(children) = &l.kind {
            tree(children, Some(me), out);
        }
    }
}

fn skin_joints<'l>(layers: &'l [CLayer], out: &mut HashSet<&'l str>) {
    for l in layers {
        if let CKind::Mesh(m) = &l.kind {
            if let Some((_, joints)) = &m.skin {
                out.extend(joints.iter().map(|j| &**j));
            }
        }
        skin_joints(l.children(), out);
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The rotation that takes y to `d` (unit), as a matrix.
fn y_to(d: [f64; 3]) -> Mat4 {
    let y = [0.0, 1.0, 0.0];
    let c = dot(y, d);
    let q = if c < -1.0 + 1e-9 {
        [1.0, 0.0, 0.0, 0.0]
    } else {
        let axis = crate::sim::cross(y, d);
        let q = [axis[0], axis[1], axis[2], 1.0 + c];
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        q.map(|v| v / n)
    };
    Pose { p: [0.0; 3], q }.matrix()
}

impl Compiled {
    /// Adds scene `i`'s ragdolls to `input`: their bones as bodies, the
    /// joints' frames and limits, and per sample what the clip asks of the
    /// motors.
    pub(super) fn ragdoll_input(&self, i: usize, input: &mut SimInput, errors: &mut Vec<String>) {
        let scene = &self.scenes[i];
        let sim = &scene.sim;
        let set = &sim.settings;
        if set.ragdolls.is_empty() {
            return;
        }
        let (rate, samples) = (input.rate, input.samples as usize);
        let mut flat = Vec::new();
        tree(&scene.layers, None, &mut flat);
        let mut joints = HashSet::new();
        skin_joints(&scene.layers, &mut joints);
        let index: HashMap<&str, usize> = flat.iter().enumerate().map(|(k, (l, _))| (&*l.id, k)).collect();
        // Each ragdoll's bones, as indices into `flat`, in tree order.
        let mut all: Vec<Vec<usize>> = Vec::new();
        for (n, r) in set.ragdolls.iter().enumerate() {
            let what = format!("{}.simulation.ragdolls.{n}", scene.id);
            let Some(&root) = index.get(r.root.as_str()) else {
                errors.push(format!("{what}.root: no layer '{}' in the scene", r.root));
                all.push(Vec::new());
                continue;
            };
            let under = |mut k: usize| loop {
                if k == root {
                    return true;
                }
                match flat[k].1 {
                    Some(p) => k = p,
                    None => return false,
                }
            };
            let comp = |k: usize| matches!(flat[k].0.kind, CKind::Comp(_));
            let mut bones: Vec<usize> = if !r.bones.is_empty() {
                let mut out = Vec::new();
                for b in &r.bones {
                    match index.get(b.as_str()) {
                        Some(&k) if under(k) => out.push(k),
                        Some(_) => errors.push(format!("{what}.bones: '{b}' is not under the root '{}'", r.root)),
                        None => errors.push(format!("{what}.bones: no layer '{b}' in the scene")),
                    }
                }
                out
            } else {
                let named: Vec<usize> = (0..flat.len()).filter(|&k| under(k) && comp(k) && joints.contains(&*flat[k].0.id)).collect();
                // No skin names them: every comp under the root is a bone.
                if named.is_empty() { (0..flat.len()).filter(|&k| under(k) && comp(k)).collect() } else { named }
            };
            bones.sort_unstable();
            bones.dedup();
            if bones.is_empty() {
                errors.push(format!("{what}: no bones under '{}' (comps, as model_import makes of a skeleton's joints)", r.root));
            }
            for &k in &bones {
                if flat[k].0.body.as_ref().is_some_and(|b| b.declared) {
                    errors.push(format!("{what}: its bone '{}' has a \"body\" of its own; a ragdoll makes its bones' bodies", flat[k].0.id));
                }
                if all.iter().any(|other| other.contains(&k)) {
                    errors.push(format!("{what}: its bone '{}' is another ragdoll's too", flat[k].0.id));
                }
            }
            all.push(bones);
        }
        if !errors.is_empty() {
            return;
        }
        // Every bone's keyed world at every sample (index 0: one before
        // the start), and at each ragdoll's rest time.
        let wanted: HashMap<&str, (usize, usize)> = all.iter().enumerate().flat_map(|(r, bones)| bones.iter().enumerate().map(move |(b, &k)| (k, (r, b)))).map(|(k, v)| (&*flat[k].0.id, v)).collect();
        let at = |t: f64, out: &mut dyn FnMut(usize, usize, Mat4)| {
            let tc = t + scene.pre_roll;
            let space = Space { keyed: true, ..self.camera(scene, tc) };
            visit(&scene.layers, tc, None, &space, None, &mut |l, world, _, _| {
                if let Some(&(r, b)) = wanted.get(&*l.id) {
                    out(r, b, world);
                }
                None
            });
        };
        let mut worlds: Vec<Vec<Vec<Mat4>>> = all.iter().map(|bones| vec![Vec::with_capacity(samples + 2); bones.len()]).collect();
        for k in 0..=samples + 1 {
            at(sim.start + (k as f64 - 1.0) / rate, &mut |r, b, w| worlds[r][b].push(w));
        }
        let sample_of = |t: f64| (((t - sim.start) * rate).round().max(0.0) as u32).min(samples as u32);
        for (n, ((r, c), bones)) in set.ragdolls.iter().zip(&sim.ragdolls).zip(&all).enumerate() {
            let release = c.release.map(sample_of).unwrap_or(0);
            let mut rest: Vec<Pose> = vec![Pose::IDENTITY; bones.len()];
            at(c.rest.unwrap_or(sim.start), &mut |rr, b, w| {
                if rr == n {
                    rest[b] = decompose(w).0;
                }
            });
            // A bone's parent: the nearest bone it is inside.
            let parent: Vec<Option<usize>> = bones
                .iter()
                .map(|&k| {
                    let mut cur = flat[k].1;
                    while let Some(p) = cur {
                        if let Some(b) = bones.iter().position(|&x| x == p) {
                            return Some(b);
                        }
                        cur = flat[p].1;
                    }
                    None
                })
                .collect();
            let name = |b: usize| &*flat[bones[b]].0.id;
            // Where each bone points (its own frame) and how long it is.
            let mut shape: Vec<([f64; 3], f64, f64)> = Vec::with_capacity(bones.len());
            for b in 0..bones.len() {
                let inv = rest[b].inverse();
                let kids: Vec<[f64; 3]> = (0..bones.len()).filter(|&c| parent[c] == Some(b)).map(|c| inv.apply(rest[c].p)).collect();
                // The way in: from its parent to it, in its own frame.
                let incoming = parent[b].map(|p| quat_rotate(inv.q, sub(rest[b].p, rest[p].p))).filter(|v| len(*v) > 1e-6);
                let pick = kids.iter().copied().filter(|k| len(*k) > 1e-6).max_by(|x, y| {
                    // The child that carries on the way in; the farthest for a root.
                    let score = |k: &[f64; 3]| match incoming {
                        Some(d) => dot(*k, d) / len(d),
                        None => len(*k),
                    };
                    score(x).total_cmp(&score(y))
                });
                let (along, length) = match (pick, incoming) {
                    (Some(k), _) => (k.map(|v| v / len(k)), len(k)),
                    // A last bone (a hand, a head, a foot): on as it came,
                    // 0.6 of its parent's bone.
                    (None, Some(d)) => (d.map(|v| v / len(d)), len(d) * 0.6),
                    (None, None) => ([0.0, -1.0, 0.0], 40.0),
                };
                // Thick enough to hold what hangs beside its line (hips, shoulders).
                let beside = kids.iter().map(|k| len(sub(*k, along.map(|a| a * dot(*k, along).clamp(0.0, length))))).fold(0.0, f64::max);
                let radius = (r.thickness * length).max(beside * 0.6).max(1.0).min(length.max(2.0));
                shape.push((along, length, radius));
            }
            let volume = |(_, length, radius): &([f64; 3], f64, f64)| std::f64::consts::PI * radius * radius * ((length - 2.0 * radius).max(0.0) + radius * 4.0 / 3.0);
            // Mass by volume, but no bone much lighter than what hangs
            // from it: a light pelvis between heavy legs and a heavy trunk
            // is a joint no solver holds.
            let mut share: Vec<f64> = shape.iter().map(volume).collect();
            for b in (0..bones.len()).rev() {
                let heaviest = (0..bones.len()).filter(|&c| parent[c] == Some(b)).map(|c| share[c]).fold(0.0, f64::max);
                share[b] = share[b].max(heaviest * 0.75);
            }
            let total: f64 = share.iter().sum();
            let limit = |id: &str| r.limits.iter().find(|l| id.contains(l.bone.as_str()));
            let first_body = input.bodies.len();
            let mut out = RagdollInput { id: r.id.clone(), bones: Vec::with_capacity(bones.len()), release, behaviours: Vec::new(), hits: Vec::new(), controller: r.controller.clone() };
            for b in 0..bones.len() {
                let poses: Vec<Pose> = worlds[n][b].iter().map(|w| decompose(*w).0).collect();
                let (along, length, radius) = shape[b];
                // A capsule from the bone's origin along it (never shorter
                // than it is thick).
                let half = (length / 2.0 - radius).max(0.0);
                let mid = (half + radius).min(length.max(radius * 2.0) / 2.0);
                let local = Mat4::translate(along[0] * mid, along[1] * mid, along[2] * mid).then(y_to(along));
                let mass = r.mass * share[b] / total.max(1e-9);
                let rest_matrix = decompose(worlds[n][b][release as usize + 1]).1;
                // A shape given for the bone stands for its capsule.
                let collider = match r.shapes.iter().find(|s| name(b).contains(s.bone.as_str())) {
                    Some(s) => match super::sim::given_shape(&s.shape, rest_matrix) {
                        Some(c) => c,
                        None => {
                            errors.push(format!("{}.simulation.ragdolls.{n}.shapes: '{}' takes a box, sphere, capsule or cylinder (and no uneven scale on the bone)", scene.id, s.bone));
                            continue;
                        }
                    },
                    None => ColliderInput { shape: ShapeInput::Capsule { radius, half }, local },
                };
                input.bodies.push(BodyInput {
                    id: name(b).to_string(),
                    kind: BodyKind::Dynamic,
                    colliders: vec![collider],
                    mass: Some(mass),
                    density: 1.0,
                    friction: r.friction,
                    bounce: r.bounce,
                    damping: 0.0,
                    spin_damping: 0.0,
                    velocity: [0.0; 3],
                    spin: [0.0; 3],
                    ccd: false,
                    release,
                    keyed: poses[1..=release as usize + 1].to_vec(),
                    before: poses[release as usize],
                    rest: rest_matrix,
                    pushes: Vec::new(),
                    group: n as u32 + 1,
                });
                let (bind, anchor_parent, clip) = match parent[b] {
                    Some(p) => {
                        let inv = rest[p].inverse();
                        let parents: Vec<Pose> = worlds[n][p].iter().map(|w| decompose(*w).0).collect();
                        // The bone's turn relative to its parent, per sample.
                        let clip = (1..=samples + 1).map(|k| quat_mul(parents[k].inverse().q, poses[k].q)).collect();
                        (quat_mul(inv.q, rest[b].q), inv.apply(rest[b].p), clip)
                    }
                    None => ([0.0, 0.0, 0.0, 1.0], [0.0; 3], Vec::new()),
                };
                // Its strength: the ragdoll's, times every entry that
                // names it or a bone it hangs from.
                let mut chain = vec![b];
                while let Some(p) = parent[*chain.last().unwrap()] {
                    chain.push(p);
                }
                let scales: Vec<usize> = r.strengths.iter().enumerate().filter(|(_, s)| chain.iter().any(|&x| name(x).contains(s.bone.as_str()))).map(|(k, _)| k).collect();
                let strength = (0..=samples).map(|k| {
                    let t = sim.start + k as f64 / rate + scene.pre_roll;
                    scales.iter().fold(c.strength.at(t), |acc, &s| acc * c.strengths[s].at(t)).clamp(0.0, 1.0)
                });
                let l = limit(name(b));
                out.bones.push(BoneInput {
                    body: first_body + b,
                    parent: parent[b],
                    anchor_parent,
                    along,
                    length,
                    swing: l.map(|l| l.swing).unwrap_or_else(crate::sim::default_swing),
                    twist: l.map(|l| l.twist).unwrap_or_else(crate::sim::default_twist),
                    hinge: l.and_then(|l| l.hinge).filter(|h| len(*h) > 1e-9).map(|h| h.map(|v| v / len(h))),
                    bind,
                    clip,
                    strength: strength.collect(),
                    mass,
                });
            }
            for (h, at) in r.hits.iter().zip(&c.hits) {
                match (0..bones.len()).find(|&b| name(b) == h.bone).or_else(|| (0..bones.len()).find(|&b| name(b).contains(h.bone.as_str()))) {
                    Some(b) => out.hits.push((*at, b, h.velocity, h.dur.max(0.0))),
                    None => errors.push(format!("{}.simulation.ragdolls.{n}.hits: no bone '{}' (it has {})", scene.id, h.bone, (0..bones.len()).map(name).collect::<Vec<_>>().join(", "))),
                }
            }
            for (bh, (from, to, amount)) in r.behaviours.iter().zip(&c.behaviours) {
                let target_body = bh.params.get("target").and_then(|t| t.as_str()).and_then(|id| input.bodies.iter().position(|b| b.id == id));
                if let Some(id) = bh.params.get("target").and_then(|t| t.as_str()) {
                    if target_body.is_none() {
                        errors.push(format!("{}.simulation.ragdolls.{n}.behaviours: target '{id}' is no body of the scene", scene.id));
                    }
                }
                out.behaviours.push(BehaviourInput {
                    kind: bh.kind,
                    from: from.unwrap_or(sim.start),
                    to: to.unwrap_or(sim.end),
                    amount: (0..=samples).map(|k| amount.at(sim.start + k as f64 / rate + scene.pre_roll).clamp(0.0, 1.0)).collect(),
                    params: bh.params.clone(),
                    target_body,
                });
            }
            input.ragdolls.push(out);
        }
    }
}
