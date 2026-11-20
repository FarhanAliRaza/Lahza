//! Behaviours: what sets a ragdoll's motors on top of its clip. Each step
//! a `Brain` reads the body (where its weight is over its feet, what hit
//! it, what is coming) and gives the joints' targets and strengths for
//! the step: balance, a step to keep it, a hand toward what is coming, a
//! hand to where it was hit, arms about the head.
//!
//! Balance is helped: besides leaning at the ankles, a force on the hips
//! of at most `assist` × the body's weight (and as much torque as that
//! force would make at the height of its weight) holds it up. A push
//! past that topples it, and it then goes soft (`limp`).
//!
//! Getting up is helped more. The motors shape it (arms pushed straight
//! under the shoulders, hips and knees folded, then all of it back to
//! the clip), but what lifts and rights the body is a force on its hips
//! of at most `help` × its weight (0.6 by default) and the torque that
//! would make: its `rise` event says what share of its weight that force
//! carried over the rise.

use crate::ragdoll::sampled;
use crate::world::{v3, World};
use lahza_motion_core::sim::{cross, quat_mul, quat_rotate, BehaviourInput, BehaviourKind, Pose, RagdollInput, SimEvent, SimInput};

type V = [f64; 3];

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V, s: f64) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: V) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V) -> Option<V> {
    let l = len(a);
    (l > 1e-9).then(|| mul(a, 1.0 / l))
}
fn capped(a: V, most: f64) -> V {
    let l = len(a);
    if l > most && l > 0.0 { mul(a, most / l) } else { a }
}

/// A turn of `angle` radians about `axis` (unit).
fn turn(axis: V, angle: f64) -> [f64; 4] {
    let (s, c) = (libm::sin(angle / 2.0), libm::cos(angle / 2.0));
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// The shortest turn that takes `from` to `to` (units).
fn arc(from: V, to: V) -> [f64; 4] {
    let c = dot(from, to).clamp(-1.0, 1.0);
    match unit(cross(from, to)) {
        Some(axis) => turn(axis, libm::acos(c)),
        // Along each other; opposite, about any axis across them.
        None if c > 0.0 => [0.0, 0.0, 0.0, 1.0],
        None => {
            let other = if from[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            turn(unit(cross(from, other)).unwrap_or([0.0, 0.0, 1.0]), std::f64::consts::PI)
        }
    }
}

fn conj(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// From `a` to `b` by `f` (0..1), the short way.
fn blend(a: [f64; 4], b: [f64; 4], f: f64) -> [f64; 4] {
    let s = if (0..4).map(|k| a[k] * b[k]).sum::<f64>() < 0.0 { -1.0 } else { 1.0 };
    let q: [f64; 4] = std::array::from_fn(|k| a[k] + (s * b[k] - a[k]) * f);
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    q.map(|v| v / n)
}

fn param(b: &BehaviourInput, name: &str, default: f64) -> f64 {
    b.params.get(name).and_then(|v| v.as_f64()).unwrap_or(default)
}

/// An arm: its bones' indices in the ragdoll.
#[derive(Clone)]
struct Arm {
    hand: usize,
    fore: usize,
    upper: usize,
    left: bool,
}

#[derive(Clone)]
struct Step {
    foot: usize,
    from: V,
    to: V,
    start: f64,
    dur: f64,
    lift: f64,
}

/// How it stood when it was let go: its hips' height over its feet, their
/// turn, and its front (the way its feet point) in the hips' frame.
#[derive(Clone, Copy)]
struct Stood {
    hips: f64,
    q: [f64; 4],
    front: V,
}

/// A rise in progress.
#[derive(Clone)]
struct Rise {
    start: f64,
    /// How its hips were turned and how high when it began.
    q: [f64; 4],
    low: f64,
    /// Upright, facing the way its head lay.
    up: [f64; 4],
    /// Newton-seconds the outside force has pushed up with so far.
    lifted: f64,
    /// It gets up from its back (no roll first).
    on_back: bool,
}

fn arms_aimed(drive: &Drive, aimed: &[Option<[f64; 4]>], bone: usize) -> bool {
    drive.targets[bone].is_some() || aimed[bone].is_some()
}

/// What a step asks of the motors.
pub struct Drive {
    /// Per bone: its turn against its parent to aim for (None: the clip's).
    pub targets: Vec<Option<[f64; 4]>>,
    /// Per bone: its strength, times the document's.
    pub scale: Vec<f64>,
    /// Per bone: placed by a behaviour this step (an arm aimed, a body
    /// getting up): it passes through the rest of its body.
    pub pass: Vec<bool>,
    /// Per bone: the one bone it passes through this step, plus one (0:
    /// none).
    pub slip: Vec<u32>,
}

#[derive(Clone)]
pub struct Brain {
    feet: Vec<usize>,
    arms: Vec<Arm>,
    head: Option<usize>,
    /// The height of its weight over its feet, and the way its trunk
    /// pointed, when it was let go (px; the scene's axes).
    stand: Option<(f64, V)>,
    /// When it fell, if it has.
    pub fallen: Option<f64>,
    down_since: Option<f64>,
    /// How it stood when let go; since when it has lain still; its rise.
    stood: Option<Stood>,
    still_since: Option<f64>,
    rising: Option<Rise>,
    step: Option<Step>,
    step_ready: f64,
    /// The bone last hit, and when.
    hit: Option<(usize, f64)>,
    seen: usize,
    /// Per bone: the target a behaviour last gave its motor. A target
    /// moves toward what is asked over `EASE` seconds: one worked out
    /// anew each step from a body that is itself moving would shake it.
    aimed: Vec<Option<[f64; 4]>>,
    /// Per behaviour: the arm it took, kept while it uses one (the
    /// nearer shoulder changes as the body sways).
    taken: Vec<Option<usize>>,
    /// Per bone: its capsule in its own frame (its ends, its radius), for
    /// the bones that are capsules.
    caps: Vec<Option<(V, V, f64)>>,
    pub events: Vec<SimEvent>,
}

/// The nearest points of segments `p1 q1` and `p2 q2`: how far along
/// each (0..1), and how far apart.
fn nearest(p1: V, q1: V, p2: V, q2: V) -> (f64, f64, f64) {
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (b, c) = (dot(d1, d2), dot(d1, r));
    let denom = a * e - b * b;
    let mut s = if denom > 1e-12 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let mut t = if e > 1e-12 { (b * s + f) / e } else { 0.0 };
    if t < 0.0 {
        (t, s) = (0.0, if a > 1e-12 { (-c / a).clamp(0.0, 1.0) } else { 0.0 });
    } else if t > 1.0 {
        (t, s) = (1.0, if a > 1e-12 { ((b - c) / a).clamp(0.0, 1.0) } else { 0.0 });
    }
    (s, t, len(sub(add(p1, mul(d1, s)), add(p2, mul(d2, t)))))
}

/// Seconds a behaviour's target takes to reach what is asked.
const EASE: f64 = 0.09;

impl Brain {
    pub fn new(input: &SimInput, r: &RagdollInput) -> Brain {
        let name = |b: usize| input.bodies[r.bones[b].body].id.to_lowercase();
        let has = |b: usize, part: &str| name(b).rsplit(['-', '/']).next().is_some_and(|n| n.contains(part));
        let leaf = |b: usize| !r.bones.iter().any(|x| x.parent == Some(b));
        let feet: Vec<usize> = (0..r.bones.len()).filter(|&b| has(b, "foot") || has(b, "toe") && leaf(b)).collect();
        let mut arms = Vec::new();
        for hand in (0..r.bones.len()).filter(|&b| has(b, "hand")) {
            let Some(fore) = r.bones[hand].parent else { continue };
            let Some(upper) = r.bones[fore].parent else { continue };
            let n = name(hand);
            let left = n.ends_with("_l") || n.ends_with(".l") || n.contains("left");
            arms.push(Arm { hand, fore, upper, left });
        }
        let caps = r.bones.iter().map(|b| match input.bodies[b.body].colliders.first() {
            Some(lahza_motion_core::sim::ColliderInput { shape: lahza_motion_core::sim::ShapeInput::Capsule { radius, half }, local }) => {
                let m = local.0;
                let end = |s: f64| [m[3] + m[1] * half * s, m[7] + m[5] * half * s, m[11] + m[9] * half * s];
                Some((end(-1.0), end(1.0), *radius))
            }
            _ => None,
        }).collect();
        Brain { caps, feet, arms, head: (0..r.bones.len()).find(|&b| has(b, "head")), stand: None, fallen: None, down_since: None, stood: None, still_since: None, rising: None, step: None, step_ready: 0.0, hit: None, seen: 0, aimed: vec![None; r.bones.len()], taken: vec![None; r.behaviours.len()], events: Vec::new() }
    }

    /// A bone's joint targets that point arm `arm` at `target` (px): the
    /// upper arm's turn against its parent, the forearm's against it.
    fn aim(&self, world: &World, r: &RagdollInput, arm: &Arm, target: V) -> Option<([f64; 4], [f64; 4])> {
        let (u, f) = (&r.bones[arm.upper], &r.bones[arm.fore]);
        let chest = world.pose(r.bones[u.parent?].body);
        let shoulder = world.pose(u.body).p;
        let (l1, l2) = (u.length.max(1.0), (f.length + r.bones[arm.hand].length * 0.5).max(1.0));
        let to = sub(target, shoulder);
        let d = len(to).clamp((l1 - l2).abs() + 1.0, l1 + l2 - 1.0);
        let dir = unit(to)?;
        // The triangle shoulder, elbow, hand.
        let at_shoulder = libm::acos(((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0));
        let flex = std::f64::consts::PI - libm::acos(((l1 * l1 + l2 * l2 - d * d) / (2.0 * l1 * l2)).clamp(-1.0, 1.0));
        // The upper arm as bound, on the chest as it is now, turned to the target.
        let bound = quat_mul(chest.q, u.bind);
        let straight = quat_mul(arc(quat_rotate(bound, u.along), dir), bound);
        // The elbow's axle (the forearm's hinge, else across the arm).
        let hinge = f.hinge.unwrap_or_else(|| unit(cross(f.along, [0.0, 0.0, 1.0])).unwrap_or([1.0, 0.0, 0.0]));
        let axle = quat_rotate(straight, quat_rotate(f.bind, hinge));
        let upper = quat_mul(turn(axle, -at_shoulder), straight);
        Some((quat_mul(conj(chest.q), upper), quat_mul(f.bind, turn(hinge, flex.min(f.swing.to_radians())))))
    }

    /// Where arm `arm`'s hand should make for now, to get to `target`
    /// round its own trunk and head, not through them. `target` is first
    /// brought out to the surface of what it lies in (a hand goes onto a
    /// chest). Then, while the straight way from the hand to it passes
    /// through the trunk (its bones' capsules, as wide again as the
    /// forearm is thick), the hand makes for a point beside the trunk
    /// where that way is deepest in it, on the side the way already
    /// leans to and toward the body's front; and what it makes for is
    /// moved out until the forearm, from the elbow, clears the trunk too.
    /// Worked out anew each step from where the hand is, so it goes
    /// round as far as it must.
    fn route(&self, world: &World, r: &RagdollInput, arm: &Arm, target: V) -> V {
        // (The trunk: what the arm hangs from, down to the root, and the head.)
        let mut trunk: Vec<usize> = Vec::new();
        let mut cur = r.bones[arm.upper].parent;
        while let Some(b) = cur {
            trunk.push(b);
            cur = r.bones[b].parent;
        }
        trunk.extend(self.head.filter(|h| !trunk.contains(h)));
        let thick = self.caps[arm.fore].map_or(8.0, |c| c.2);
        let shoulder = world.pose(r.bones[arm.upper].body).p;
        // (The body's front: the way it stood facing, as its root is now turned.)
        let front = self.stood.map(|s| quat_rotate(world.pose(r.bones[0].body).q, s.front));
        trunk.retain(|&b| self.caps[b].is_some());
        let caps: Vec<(V, V, f64)> = trunk.iter().filter_map(|&b| {
            let (a, c, radius) = self.caps[b]?;
            let p = world.pose(r.bones[b].body);
            Some((p.apply(a), p.apply(c), radius))
        }).collect();
        // The way out of a capsule from a point by it: away from its line,
        // leaning to the front and to the arm's own side.
        let out = |a: V, b: V, at: V, from: V| -> V {
            let axis = unit(sub(b, a)).unwrap_or([0.0, 1.0, 0.0]);
            let across = |v: V| sub(v, mul(axis, dot(v, axis)));
            let lean = add(mul(front.unwrap_or([0.0; 3]), 0.8), mul(unit(across(sub(shoulder, at))).unwrap_or([0.0; 3]), 0.3));
            unit(across(add(unit(across(sub(from, at))).unwrap_or([0.0; 3]), lean))).or(front).unwrap_or([0.0, 0.0, -1.0])
        };
        let on = |a: V, b: V, t: f64| add(a, mul(sub(b, a), t));
        let hand = world.pose(r.bones[arm.hand].body).apply(mul(r.bones[arm.hand].along, r.bones[arm.hand].length * 0.5));
        // A target inside the trunk is on it.
        let mut target = target;
        let mut resting: Option<(usize, f64)> = None;
        for (k, (a, b, radius)) in caps.iter().enumerate() {
            let (_, t, d) = nearest(target, target, *a, *b);
            // (The hand's middle is aimed: its tip is half its length on.)
            let skin = radius + r.bones[arm.hand].length * 0.3 + self.caps[arm.hand].map_or(thick * 0.6, |c| c.2);
            if d < radius + thick * 0.6 {
                let at = on(*a, *b, t);
                target = add(at, mul(out(*a, *b, at, hand), skin));
                resting = Some((k, skin));
            }
        }
        // The first of the trunk in the straight way.
        let mut aim = target;
        let mut first: Option<(f64, V)> = None;
        for (k, (a, b, radius)) in caps.iter().enumerate() {
            let wide = radius + thick + 4.0;
            let (s, t, d) = nearest(hand, target, *a, *b);
            // (In the way: the way goes through it, deeper than where it
            // starts and where it ends. A hand by a head, making for a
            // place by the head, has nothing to go round.)
            let ends = nearest(hand, hand, *a, *b).2.min(nearest(target, target, *a, *b).2);
            let _ = (k, resting);
            if d < wide && d < ends - 3.0 && first.is_none_or(|(before, _)| s < before) {
                let at = on(*a, *b, t);
                first = Some((s, add(at, mul(out(*a, *b, at, on(hand, target, s)), wide * 1.15))));
            }
        }
        if let Some((_, beside)) = first {
            aim = beside;
        }
        // The forearm clears the trunk too (but what the target lies on,
        // and what the arm itself hangs from: a folded arm lies along it).
        let elbow = world.pose(r.bones[arm.fore].body).p;
        for _ in 0..2 {
            for (k, (a, b, radius)) in caps.iter().enumerate() {
                if resting.is_some_and(|(on_it, _)| on_it == k) && first.is_none() || Some(trunk[k]) == r.bones[arm.upper].parent {
                    continue;
                }
                let wide = radius + thick + 2.0;
                let (s, t, d) = nearest(elbow, aim, *a, *b);
                let ends = nearest(elbow, elbow, *a, *b).2.min(nearest(aim, aim, *a, *b).2);
                if d < wide && d < ends - 3.0 {
                    let at = on(*a, *b, t);
                    aim = add(aim, mul(out(*a, *b, at, on(elbow, aim, s)), (wide - d) / s.max(0.3)));
                }
            }
        }
        aim
    }

    /// The targets and strengths for the step from time `t` that ends at
    /// sample position `x`; pushes the body where a behaviour does.
    ///
    /// `turns`: per bone, a turn to make on top of the clip's target (a
    /// controller's: a policy's action, a search's choice).
    pub fn think(&mut self, world: &mut World, input: &SimInput, r: &RagdollInput, x: f64, t: f64, turns: Option<&[Option<[f64; 4]>]>) -> Drive {
        let n = r.bones.len();
        let mut drive = Drive { targets: vec![None; n], scale: vec![1.0; n], pass: vec![false; n], slip: vec![0; n] };
        if let Some(turns) = turns {
            for (k, turn) in turns.iter().enumerate().take(n) {
                if let Some(turn) = turn {
                    drive.targets[k] = Some(quat_mul(*turn, world.clip(r, k, x).unwrap_or(r.bones[k].bind)));
                }
            }
        }
        if !world.released[r.bones[0].body] {
            return drive;
        }
        let u = world.unit;
        let dt = world.dt;
        let active: Vec<(&BehaviourInput, f64)> = r.behaviours.iter().filter(|b| t >= b.from - 1e-9 && t < b.to).map(|b| (b, sampled(&b.amount, x))).filter(|(_, a)| *a > 0.0).collect();
        let index_of = |b: &BehaviourInput| r.behaviours.iter().position(|x| std::ptr::eq(x, b)).unwrap_or(0);
        let mut using = vec![false; r.behaviours.len()];
        let find = |kind: BehaviourKind| active.iter().find(|(b, _)| b.kind == kind).copied();
        // What hit it: its own `hits`, and other bodies at speed.
        for (at, bone, _, _) in &r.hits {
            if *at >= t - 1e-9 && *at < t + dt - 1e-9 {
                self.hit = Some((*bone, t));
            }
        }
        for e in &world.events[self.seen.min(world.events.len())..] {
            if e.kind == "touch" && e.speed >= 150.0 {
                let bone = (0..n).find(|&b| input.bodies[r.bones[b].body].id == e.a || input.bodies[r.bones[b].body].id == e.b);
                if let (Some(bone), true) = (bone, e.a != "ground" && e.b != "ground") {
                    self.hit = Some((bone, t));
                }
            }
        }
        self.seen = world.events.len();
        // Where its weight is, and how it moves (px, px/s).
        let (mut mass, mut com, mut vel) = (0.0, [0.0; 3], [0.0; 3]);
        for b in &r.bones {
            let rb = &world.w.bodies[world.handles[b.body]];
            let (m, c, v) = (rb.mass() as f64, rb.center_of_mass(), rb.linvel());
            mass += m;
            com = add(com, mul([c.x as f64, c.y as f64, c.z as f64], m * u));
            vel = add(vel, mul([v.x as f64, v.y as f64, v.z as f64], m * u));
        }
        let (com, vel) = (mul(com, 1.0 / mass.max(1e-9)), mul(vel, 1.0 / mass.max(1e-9)));
        let g = len(input.gravity);
        let places: Vec<(usize, V)> = self
            .feet
            .iter()
            .map(|&b| {
                let c = world.w.bodies[world.handles[r.bones[b].body]].center_of_mass();
                (b, [c.x as f64 * u, c.y as f64 * u, c.z as f64 * u])
            })
            .collect();
        let foot_at = |b: usize| places.iter().find(|(f, _)| *f == b).map(|(_, p)| *p).unwrap_or([0.0; 3]);
        let balance = find(BehaviourKind::Balance);
        if let (Some(up), false, Some(_)) = (unit(mul(input.gravity, -1.0)), self.feet.is_empty(), balance.or(find(BehaviourKind::Step))) {
            let flat = |v: V| sub(v, mul(up, dot(v, up)));
            let swinging = self.step.as_ref().map(|s| s.foot);
            let standing: Vec<usize> = self.feet.iter().copied().filter(|f| Some(*f) != swinging || self.feet.len() == 1).collect();
            let support = mul(standing.iter().fold([0.0; 3], |acc, &f| add(acc, foot_at(f))), 1.0 / standing.len() as f64);
            let height = dot(sub(com, support), up);
            let trunk = quat_rotate(world.pose(r.bones[0].body).q, r.bones[0].along);
            let (stand, upright) = *self.stand.get_or_insert((height.max(1.0), trunk));
            let hips = world.pose(r.bones[0].body);
            let stood = *self.stood.get_or_insert_with(|| {
                let toes = self.feet.iter().fold([0.0; 3], |acc, &f| add(acc, quat_rotate(world.pose(r.bones[f].body).q, r.bones[f].along)));
                let front = unit(flat(toes)).unwrap_or_else(|| unit(flat(cross(up, trunk))).unwrap_or([0.0, 0.0, -1.0]));
                Stood { hips: dot(sub(hips.p, support), up).max(1.0), q: hips.q, front: quat_rotate(conj(hips.q), front) }
            });
            // Fallen: its weight low over its feet, or its trunk far over,
            // for a tenth of a second. It does not get up.
            let tilt = libm::acos(dot(trunk, upright).clamp(-1.0, 1.0));
            if self.fallen.is_none() {
                if height < 0.55 * stand || tilt > 1.15 {
                    let since = *self.down_since.get_or_insert(t);
                    if t - since >= 0.1 {
                        self.fallen = Some(t);
                        self.step = None;
                        self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "fall".into(), a: r.id.clone(), b: String::new(), speed: len(vel).round() });
                    }
                } else {
                    self.down_since = None;
                }
            }
            if let Some(at) = self.fallen {
                // Soft from the fall on (over a fifth of a second).
                let limp = balance.map(|(b, _)| param(b, "limp", 0.15)).unwrap_or(0.15);
                let f = ((t - at) / 0.2).clamp(0.0, 1.0);
                drive.scale.iter_mut().for_each(|s| *s = 1.0 + (limp - 1.0) * f);
                let weight = mass * g;
                match find(BehaviourKind::GetUp) {
                    None => (self.rising, self.still_since) = (None, None),
                    // Lying: it starts once it has been still for `wait`.
                    Some((b, _)) if self.rising.is_none() => {
                        if len(vel) < 0.3 * stand {
                            let since = *self.still_since.get_or_insert(t);
                            if t - since >= param(b, "wait", 0.8) {
                                // Which way up: from its front (rolling onto it
                                // first), from its back (sitting up), or
                                // whichever it lies nearer.
                                let on_back = match b.params.get("way").and_then(|w| w.as_str()) {
                                    Some("back") => true,
                                    Some("near") => dot(quat_rotate(hips.q, stood.front), up) > 0.0,
                                    _ => false,
                                };
                                // Up again it faces the way its head lies
                                // (from its back: the way its feet do).
                                let lying = unit(flat(trunk)).map(|v| if on_back { mul(v, -1.0) } else { v }).unwrap_or(quat_rotate(stood.q, stood.front));
                                let faced = unit(flat(quat_rotate(stood.q, stood.front))).unwrap_or(lying);
                                self.rising = Some(Rise { start: t, q: hips.q, low: dot(sub(hips.p, support), up), up: quat_mul(arc(faced, lying), stood.q), lifted: 0.0, on_back });
                                self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "get_up".into(), a: r.id.clone(), b: String::new(), speed: 0.0 });
                            }
                        } else {
                            self.still_since = None;
                        }
                    }
                    Some((b, amount)) => {
                        let mut rise = self.rising.take().unwrap();
                        drive.pass.iter_mut().for_each(|p| *p = true);
                        let time = param(b, "time", 2.4).max(0.5);
                        let help = param(b, "help", 0.6) * amount;
                        let f = (t - rise.start) / time;
                        let smooth = |x: f64| {
                            let x = x.clamp(0.0, 1.0);
                            x * x * (3.0 - 2.0 * x)
                        };
                        // Stiff again (over a fifth of a second).
                        let stiff = (f * time / 0.2).clamp(0.0, 1.0);
                        drive.scale.iter_mut().for_each(|s| *s = limp + (1.0 - limp) * stiff);
                        let down = mul(up, -1.0);
                        let front = quat_rotate(hips.q, stood.front);
                        // Onto its front first (a quarter of the time at most).
                        let (toward, side_up) = if rise.on_back { (up, dot(front, up)) } else { (down, dot(front, down)) };
                        let rolling = f < 0.25 && side_up < 0.6 && dot(trunk, up).abs() < 0.7;
                        // Then: pushed up and folded by 0.6, standing by 1.
                        let (push, stand_up) = if rolling { (0.0, 0.0) } else { (smooth((f - 0.2) / 0.35), smooth((f - 0.55) / 0.45)) };
                        let rb = &mut world.w.bodies[world.handles[r.bones[0].body]];
                        let w = rb.angvel();
                        let spin = [w.x as f64, w.y as f64, w.z as f64];
                        let lever = stood.hips / u;
                        let inertia = mass * lever * lever * 0.2;
                        let torque = if rolling {
                            // About its length, the short way round to face down.
                            let way = dot(cross(front, toward), trunk);
                            let side = if way.abs() < 0.05 && side_up < 0.0 { 1.0 } else { way.signum() };
                            mul(trunk, (side * 14.0 - dot(spin, trunk) * 4.0) * inertia)
                        } else {
                            // Toward upright by the time it stands.
                            let want = blend(rise.q, rise.up, smooth((f - 0.25) / 0.6));
                            let d = quat_mul(want, conj(hips.q));
                            let d = if d[3] < 0.0 { d.map(|v| -v) } else { d };
                            let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                            let turn = if s < 1e-9 { [0.0; 3] } else { mul([d[0] / s, d[1] / s, d[2] / s], 2.0 * libm::atan2(s, d[3])) };
                            mul(sub(mul(turn, 90.0), mul(spin, 14.0)), inertia)
                        };
                        rb.apply_torque_impulse(v3(capped(torque, help * weight * lever), dt), true);
                        if !rolling {
                            // Its hips up to where they were, and over its feet.
                            let high = rise.low + (0.5 * stood.hips - rise.low).max(0.0) * push + (stood.hips - rise.low.max(0.5 * stood.hips)) * stand_up;
                            let v = rb.linvel();
                            let moving = [v.x as f64 * u, v.y as f64 * u, v.z as f64 * u];
                            let off = sub(hips.p, support);
                            let lift = (high - dot(off, up)) * 70.0 - dot(moving, up) * 14.0;
                            let over = sub(mul(flat(off), -18.0 * stand_up), mul(flat(moving), 6.0 * stand_up));
                            let force = capped(mul(add(mul(up, lift), over), mass / u), help * weight);
                            rb.apply_impulse(v3(force, dt), true);
                            rise.lifted += dot(force, up).max(0.0) * dt;
                            // Arms: hands pressed down under the shoulders
                            // while it pushes up, let go as it stands.
                            let press = push * (1.0 - stand_up);
                            if press > 0.0 {
                                for arm in &self.arms {
                                    let reach = r.bones[arm.upper].length + r.bones[arm.fore].length;
                                    let shoulder = world.pose(r.bones[arm.upper].body).p;
                                    let Some((upper, fore)) = self.aim(world, r, arm, add(shoulder, mul(down, reach * (0.45 + 0.5 * push)))) else { continue };
                                    for (bone, want) in [(arm.upper, upper), (arm.fore, fore)] {
                                        let clip = world.clip(r, bone, x).unwrap_or(r.bones[bone].bind);
                                        drive.targets[bone] = Some(blend(clip, want, press));
                                    }
                                }
                            }
                            // Legs: folded at the hips and knees (a knee
                            // bends about its `hinge`), then straightened.
                            let fold = push * (1.0 - stand_up);
                            if fold > 0.0 {
                                for &foot in &self.feet {
                                    let Some(shin) = r.bones[foot].parent else { continue };
                                    let (Some(thigh), Some(hinge)) = (r.bones[shin].parent, r.bones[shin].hinge) else { continue };
                                    let knee = (2.0 * fold).min(r.bones[shin].swing.to_radians());
                                    let across = quat_rotate(r.bones[shin].bind, hinge);
                                    drive.targets[shin] = Some(quat_mul(world.clip(r, shin, x).unwrap_or(r.bones[shin].bind), turn(hinge, knee)));
                                    drive.targets[thigh] = Some(quat_mul(world.clip(r, thigh, x).unwrap_or(r.bones[thigh].bind), turn(across, -(1.5 * fold).min(r.bones[thigh].swing.to_radians()))));
                                }
                            }
                        }
                        let tilt = libm::acos(dot(trunk, upright).clamp(-1.0, 1.0));
                        let share = rise.lifted / (weight * (t - rise.start)).max(1e-9);
                        self.rising = Some(rise);
                        if f >= 1.0 && height > 0.85 * stand && tilt < 0.35 {
                            // Up: what share of its weight the force carried.
                            self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "rise".into(), a: r.id.clone(), b: String::new(), speed: (share * 100.0).round() });
                            (self.fallen, self.down_since, self.rising, self.still_since) = (None, None, None, None);
                        } else if f >= 1.6 {
                            // Not up: it lies again, and tries again.
                            (self.rising, self.still_since) = (None, None);
                            self.fallen = Some(t);
                        }
                    }
                }
            } else {
                // Where it is falling to: its weight's place, carried on
                // by its speed for the time a fall from this height takes.
                let fall_time = (height.max(1.0) / (g * u).max(1e-9)).sqrt();
                let error = flat(sub(add(com, mul(vel, fall_time)), support));
                if let Some((b, amount)) = balance {
                    let (gain, assist) = (param(b, "gain", 1.0), param(b, "assist", 0.25) * amount);
                    // Lean at the ankles: toes pressed down toward the way it tips.
                    if let Some(way) = unit(error) {
                        let angle = (gain * len(error) / height.max(1.0)).min(0.35) * amount;
                        let lean = turn(cross(up, way), angle);
                        for &f in &standing {
                            let Some(shin) = r.bones[f].parent else { continue };
                            let s = world.pose(r.bones[shin].body).q;
                            let want = drive.targets[f].or_else(|| world.clip(r, f, x)).unwrap_or(r.bones[f].bind);
                            drive.targets[f] = Some(quat_mul(quat_mul(conj(s), quat_mul(lean, s)), want));
                        }
                    }
                    // The help: a push on the hips back over the feet and a
                    // turn of them upright, each no more than `assist` of
                    // its weight would make.
                    if assist > 0.0 {
                        let weight = mass * g;
                        let rb = &mut world.w.bodies[world.handles[r.bones[0].body]];
                        let push = capped(mul(add(mul(error, 30.0), mul(flat(vel), 6.0)), -mass / u), assist * weight);
                        rb.apply_impulse(v3(push, dt), true);
                        let w = rb.angvel();
                        let lever = stand / u;
                        let axis = cross(trunk, upright);
                        let torque = capped(mul(sub(mul(unit(axis).unwrap_or([0.0; 3]), 60.0 * tilt), mul([w.x as f64, w.y as f64, w.z as f64], 10.0)), mass * lever * lever * 0.2), assist * weight * lever);
                        rb.apply_torque_impulse(v3(torque, dt), true);
                    }
                }
                if let Some((b, _)) = find(BehaviourKind::Step) {
                    let trigger = param(b, "trigger", 0.12 * stand);
                    if self.step.is_none() && t >= self.step_ready && len(error) > trigger && self.feet.len() >= 2 {
                        // The foot behind, to past where it is falling to.
                        if let Some(way) = unit(error) {
                            let foot = *self.feet.iter().min_by(|a, b| dot(sub(foot_at(**a), support), way).total_cmp(&dot(sub(foot_at(**b), support), way))).unwrap();
                            let from = foot_at(foot);
                            let reach = param(b, "length", 0.5 * stand);
                            let aim = add(support, mul(error, 1.4));
                            let to = add(from, capped(flat(sub(aim, from)), reach));
                            self.step = Some(Step { foot, from, to, start: t, dur: param(b, "time", 0.28).max(0.05), lift: 0.12 * stand });
                            self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "step".into(), a: input.bodies[r.bones[foot].body].id.clone(), b: String::new(), speed: len(vel).round() });
                        }
                    }
                }
            }
        }
        // A step under way (a behaviour's, or one a controller ordered): the
        // foot is carried to its place.
        if let (Some(up), Some(s)) = (unit(mul(input.gravity, -1.0)), self.step.clone()) {
            let amount = find(BehaviourKind::Step).map_or(1.0, |(_, a)| a);
            let f = ((t - s.start) / s.dur).clamp(0.0, 1.0);
            let ease = f * f * (3.0 - 2.0 * f);
            let want = add(add(s.from, mul(sub(s.to, s.from), ease)), mul(up, s.lift * libm::sin(std::f64::consts::PI * f)));
            // The foot is carried there; its leg follows, soft.
            let mut leg = vec![s.foot];
            while let (Some(p), true) = (r.bones[*leg.last().unwrap()].parent, leg.len() < 3) {
                leg.push(p);
            }
            let carried: f64 = leg.iter().map(|&b| r.bones[b].mass).sum();
            let rb = &mut world.w.bodies[world.handles[r.bones[s.foot].body]];
            let v = rb.linvel();
            let pull = capped(mul(sub(mul(sub(want, foot_at(s.foot)), 500.0), mul([v.x as f64 * u, v.y as f64 * u, v.z as f64 * u], 40.0)), carried / u), 5.0 * carried * g * amount);
            rb.apply_impulse(v3(pull, dt), true);
            for &b in &leg {
                drive.scale[b] *= 0.3;
            }
            if f >= 1.0 {
                self.step = None;
                self.step_ready = t + 0.15;
            }
        }
        // Arms: later behaviours take an arm over earlier ones.
        let mut arms: Vec<Option<(V, f64)>> = vec![None; self.arms.len()];
        let chest_of = |a: &Arm| r.bones[a.upper].parent.map(|c| world.pose(r.bones[c].body));
        let pick = |b: &BehaviourInput, target: V, not_on: Option<usize>| -> Option<usize> {
            let free = |k: &usize| not_on.is_none_or(|bone| ![self.arms[*k].hand, self.arms[*k].fore, self.arms[*k].upper].contains(&bone));
            match b.params.get("hand").and_then(|h| h.as_str()) {
                Some("left") => (0..self.arms.len()).find(|k| self.arms[*k].left),
                Some("right") => (0..self.arms.len()).find(|k| !self.arms[*k].left),
                _ => (0..self.arms.len()).filter(free).min_by(|a, c| len(sub(world.pose(r.bones[self.arms[*a].upper].body).p, target)).total_cmp(&len(sub(world.pose(r.bones[self.arms[*c].upper].body).p, target)))),
            }
        };
        for (b, amount) in &active {
            match b.kind {
                BehaviourKind::Guard => {
                    let target = match (b.target_body, b.params.get("target").and_then(|v| v.as_array())) {
                        (Some(body), _) => Some(world.pose(body).p),
                        (None, Some(p)) if p.len() == 3 => Some([p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0), p[2].as_f64().unwrap_or(0.0)]),
                        _ => None,
                    };
                    let Some(target) = target else { continue };
                    let i = index_of(b);
                    let Some(k) = self.taken[i].or_else(|| pick(b, target, None)) else { continue };
                    let arm = &self.arms[k];
                    let reach = r.bones[arm.upper].length + r.bones[arm.fore].length;
                    let away = len(sub(target, world.pose(r.bones[arm.upper].body).p));
                    let within = param(b, "within", 4.0 * reach);
                    // Comes up as the thing comes within reach of `within`.
                    let near = ((within - away) / (0.3 * within).max(1.0)).clamp(0.0, 1.0);
                    if near > 0.0 {
                        arms[k] = Some((target, amount * near));
                        (self.taken[i], using[i]) = (Some(k), true);
                    }
                }
                BehaviourKind::Reach => {
                    let Some((bone, at)) = self.hit else { continue };
                    let hold = param(b, "hold", 1.5);
                    if t - at > hold {
                        continue;
                    }
                    let target = world.pose(r.bones[bone].body).apply(mul(r.bones[bone].along, r.bones[bone].length * 0.5));
                    let i = index_of(b);
                    let Some(k) = self.taken[i].or_else(|| pick(b, target, Some(bone))) else { continue };
                    (self.taken[i], using[i]) = (Some(k), true);
                    // (Up in a fifth of a second, down over the last third.)
                    let f = ((t - at) / 0.2).clamp(0.0, 1.0) * ((hold - (t - at)) / (hold / 3.0).max(1e-9)).clamp(0.0, 1.0);
                    arms[k] = Some((target, amount * f));
                }
                BehaviourKind::Protect => {
                    let Some(head) = self.head else { continue };
                    let h = &r.bones[head];
                    let centre = world.pose(h.body).apply(mul(h.along, h.length * 0.5));
                    for (k, arm) in self.arms.iter().enumerate() {
                        let Some(chest) = chest_of(arm) else { continue };
                        // Each hand to its own side of the head.
                        let side = unit(sub(world.pose(r.bones[arm.upper].body).p, chest.p)).unwrap_or([0.0; 3]);
                        let across = sub(side, mul(quat_rotate(world.pose(h.body).q, h.along), dot(side, quat_rotate(world.pose(h.body).q, h.along))));
                        arms[k] = Some((add(centre, mul(unit(across).unwrap_or(side), h.length * 0.7)), *amount));
                    }
                }
                BehaviourKind::Balance | BehaviourKind::Step | BehaviourKind::GetUp => {}
            }
        }
        for (i, used) in using.iter().enumerate() {
            if !used {
                self.taken[i] = None;
            }
        }
        for (k, wanted) in arms.iter().enumerate() {
            let Some((target, amount)) = wanted else { continue };
            let arm = &self.arms[k];
            let Some((upper, fore)) = self.aim(world, r, arm, self.route(world, r, arm, *target)) else { continue };
            for (bone, want) in [(arm.upper, upper), (arm.fore, fore)] {
                let clip = world.clip(r, bone, x).unwrap_or(r.bones[bone].bind);
                drive.targets[bone] = Some(blend(clip, want, amount.clamp(0.0, 1.0)));
            }
        }
        // An arm that is aimed, or on its way back: its upper arm passes
        // through its body (it is worked out without the body in its
        // way). Its forearm and hand meet the body and are led round it,
        // but for the bone the arm hangs from: a folded arm lies along
        // its own chest, whose capsule holds the shoulders.
        for arm in &self.arms {
            if [arm.upper, arm.fore].iter().any(|&b| arms_aimed(&drive, &self.aimed, b)) {
                drive.pass[arm.upper] = true;
                if let Some(chest) = r.bones[arm.upper].parent {
                    drive.slip[arm.fore] = chest as u32 + 1;
                    drive.slip[arm.hand] = chest as u32 + 1;
                }
            }
        }
        // Targets ease to what is asked, and back to the clip when a
        // behaviour lets go.
        let f = 1.0 - libm::exp(-dt / EASE);
        for bone in 0..n {
            let clip = world.clip(r, bone, x).unwrap_or(r.bones[bone].bind);
            match (drive.targets[bone], self.aimed[bone]) {
                (Some(want), was) => {
                    let now = blend(was.unwrap_or(clip), want, f);
                    self.aimed[bone] = Some(now);
                    drive.targets[bone] = Some(now);
                }
                (None, Some(was)) => {
                    let now = blend(was, clip, f);
                    let back = (0..4).map(|k| now[k] * clip[k]).sum::<f64>().abs() > 0.99999;
                    self.aimed[bone] = (!back).then_some(now);
                    drive.targets[bone] = (!back).then_some(now);
                }
                (None, None) => {}
            }
        }
        drive
    }
}

impl Brain {
    /// The feet: bones' indices.
    pub fn feet(&self) -> &[usize] {
        &self.feet
    }

    /// The upper arms: bones' indices.
    pub fn upper_arms(&self) -> Vec<usize> {
        self.arms.iter().map(|a| a.upper).collect()
    }

    /// Whether a step may begin at `t` (none is under way, and the last
    /// has landed).
    pub fn can_step(&self, t: f64) -> bool {
        self.step.is_none() && t >= self.step_ready && self.fallen.is_none() && self.feet.len() >= 2
    }

    /// Has `foot` (a bone) stepped from `from` to `to` (px), from `t`, as
    /// the `step` behaviour steps: carried there in 0.28 s, its leg soft.
    pub fn order_step(&mut self, input: &SimInput, r: &RagdollInput, foot: usize, from: [f64; 3], to: [f64; 3], lift: f64, t: f64) {
        self.step = Some(Step { foot, from, to, start: t, dur: 0.28, lift });
        self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "step".into(), a: input.bodies[r.bones[foot].body].id.clone(), b: String::new(), speed: 0.0 });
    }
}

/// How a ragdoll stands at one moment.
#[derive(Clone, Copy, Debug)]
pub struct Stance {
    /// The height of its weight over its feet (px).
    pub height: f64,
    /// How far its weight is headed past the middle of its feet (px,
    /// level): where it is, carried on by its speed for the time a fall
    /// from this height takes.
    pub heading: f64,
    /// The way its trunk points (the scene's axes).
    pub trunk: [f64; 3],
    /// The middle of its feet, and from there to where its weight is
    /// headed, on the level (px).
    pub support: [f64; 3],
    pub toward: [f64; 3],
}

impl Stance {
    pub fn of(world: &World, input: &SimInput, r: &RagdollInput, feet: &[usize]) -> Option<Stance> {
        let u = world.unit;
        let up = unit(mul(input.gravity, -1.0))?;
        if feet.is_empty() {
            return None;
        }
        let (mut mass, mut com, mut vel) = (0.0, [0.0; 3], [0.0; 3]);
        let centre = |b: usize| {
            let c = world.w.bodies[world.handles[b]].center_of_mass();
            [c.x as f64 * u, c.y as f64 * u, c.z as f64 * u]
        };
        for b in &r.bones {
            let rb = &world.w.bodies[world.handles[b.body]];
            let (m, v) = (rb.mass() as f64, rb.linvel());
            mass += m;
            com = add(com, mul(centre(b.body), m));
            vel = add(vel, mul([v.x as f64, v.y as f64, v.z as f64], m * u));
        }
        let (com, vel) = (mul(com, 1.0 / mass.max(1e-9)), mul(vel, 1.0 / mass.max(1e-9)));
        let support = mul(feet.iter().fold([0.0; 3], |acc, &f| add(acc, centre(r.bones[f].body))), 1.0 / feet.len() as f64);
        let height = dot(sub(com, support), up);
        let fall_time = (height.max(1.0) / (len(input.gravity) * u).max(1e-9)).sqrt();
        let off = sub(add(com, mul(vel, fall_time)), support);
        let toward = sub(off, mul(up, dot(off, up)));
        Some(Stance { height, heading: len(toward), trunk: quat_rotate(world.pose(r.bones[0].body).q, r.bones[0].along), support, toward })
    }

    /// Radians its trunk is over from how it pointed in `then`.
    pub fn tilt(&self, then: &Stance) -> f64 {
        libm::acos(dot(self.trunk, then.trunk).clamp(-1.0, 1.0))
    }
}

/// A bone's centre of mass (px).
pub fn bone_origin(world: &World, r: &RagdollInput, bone: usize) -> [f64; 3] {
    let c = world.w.bodies[world.handles[r.bones[bone].body]].center_of_mass();
    [c.x as f64 * world.unit, c.y as f64 * world.unit, c.z as f64 * world.unit]
}

/// A bone's centre (px): halfway along it.
pub fn bone_centre(world: &World, r: &RagdollInput, bone: usize) -> [f64; 3] {
    let b = &r.bones[bone];
    let p: Pose = world.pose(b.body);
    p.apply(mul(b.along, b.length * 0.5))
}
