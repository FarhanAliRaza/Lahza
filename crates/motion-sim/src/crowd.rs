//! Crowds: agents that walk to their goals round each other and round
//! what stands in their way, their clips run by the distance walked. One
//! a body strikes hard goes down: its skeleton is let go as a limp
//! ragdoll where the clip had it, or it stops where it is.
//!
//! Steering is once a sample (a frame): toward the goal at the agent's
//! own speed, pushed off agents within three radii and off the level
//! outline of any body that stands in its height. In the world an agent
//! is a capsule that is moved, not pushed: bodies bounce off it.

use crate::world::{pose, v3, Rng, World};
use lahza_motion_core::sim::{CrowdInput, HerdTrack, Pose, SimEvent, SimInput, Track};
use std::collections::HashMap;
use rapier3d::prelude::*;

struct Agent {
    at: [f64; 2],
    vel: [f64; 2],
    /// The way it faces (unit, level).
    heading: [f64; 2],
    speed: f64,
    /// px walked, and where in the cycle it started (0..1).
    walked: f64,
    phase: f64,
    /// The sample from which it is down.
    down: Option<u32>,
    proxy: RigidBodyHandle,
    /// The corners of its way round what stands between it and its goal
    /// (none: straight there), and the body it last stepped aside from.
    path: Vec<[f64; 2]>,
    dodged: Option<usize>,
    /// How far its guard is up (0..1), and until when it holds it.
    guard: f64,
    wary: f64,
    /// In a try ahead: too far from what is tried to matter; it stands.
    idle: bool,
    /// Its place at this sample and the next.
    now: Pose,
    next: Pose,
    track: Track,
}

pub struct Herd {
    agents: Vec<Vec<Agent>>,
    pub events: Vec<SimEvent>,
    /// Per crowd of instances: its samples so far, each agent's ragdoll
    /// of the pool, and how many of the pool are taken.
    rows: Vec<Vec<[f32; 4]>>,
    /// And, for those with a guard, how far each agent's is up.
    guards: Vec<Vec<f32>>,
    slots: Vec<Vec<u32>>,
    taken: Vec<usize>,
    /// Samples planned so far.
    sample: u32,
    /// Whose capsule a body is.
    proxies: HashMap<RigidBodyHandle, (usize, usize)>,
    /// A copy for a try ahead: it steers, and keeps nothing.
    quiet: bool,
}

/// A body as agents see it: its box (px), how it moves (px/s).
struct Block {
    body: usize,
    lo: [f64; 3],
    hi: [f64; 3],
    vel: [f64; 3],
    fixed: bool,
    /// Its box is not its outline on the level (a wall at an angle, a
    /// hull, a mesh): its colliders are asked where it is.
    shaped: bool,
}

/// How far the level point `p` is from block `b`'s outline, and the
/// nearest point of it: of its box, or (`shaped`) of its colliders, asked
/// at three and at seven tenths of the agents' height. Inside it: 0.
fn outline(world: &World, b: &Block, p: [f64; 2], feet: f64, height: f64) -> (f64, [f64; 2]) {
    if !b.shaped {
        let n = [p[0].clamp(b.lo[0], b.hi[0]), p[1].clamp(b.lo[2], b.hi[2])];
        return (len2([p[0] - n[0], p[1] - n[1]]), n);
    }
    let u = world.unit;
    let mut best = (f64::INFINITY, p);
    for &c in world.w.bodies[world.handles[b.body]].colliders() {
        let col = &world.w.colliders[c];
        for at in [0.3, 0.7] {
            let from = [p[0], feet - at * height, p[1]];
            let proj = col.shape().project_point(col.position(), v3(from, 1.0 / u), true);
            let n = [proj.point.x as f64 * u, proj.point.z as f64 * u];
            let d = if proj.is_inside { 0.0 } else { len2([p[0] - n[0], p[1] - n[1]]).max((proj.point.y as f64 * u - from[1]).abs()) };
            if d < best.0 {
                best = (d, n);
            }
        }
    }
    best
}

fn self_places(places: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in places {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Where a crowd's agents may walk this sample: the level in squares,
/// those within an agent's radius of something that stands still shut.
struct Ground {
    origin: [f64; 2],
    cell: f64,
    w: usize,
    h: usize,
    shut: Vec<bool>,
    /// What shuts them, each as [x0, z0, x1, z1] with the room round it.
    walls: Vec<[f64; 4]>,
}

impl Ground {
    /// Covers the agents, their goals and what stands, with room round them.
    /// `far`: how far a level point is from a block's outline.
    fn new(c: &CrowdInput, places: &([f64; 2], [f64; 2]), standing: &[&Block], far: &dyn Fn(&Block, [f64; 2]) -> f64) -> Ground {
        let (mut lo, mut hi) = *places;
        let cover = |lo: &mut [f64; 2], hi: &mut [f64; 2], p: [f64; 2]| {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        };
        c.agents.iter().for_each(|a| cover(&mut lo, &mut hi, a.goal));
        // What stands still is a wall. What moves slower than an agent
        // walks is one too, over where it will be in the second a way is
        // kept for: its box drawn out along its way. (What moves faster
        // is no wall: it is stepped aside from, or pushed off.)
        let speed = |b: &Block| (b.vel[0] * b.vel[0] + b.vel[1] * b.vel[1] + b.vel[2] * b.vel[2]).sqrt();
        let swept: Vec<Block> = standing.iter().filter(|b| !b.fixed && speed(b) >= 20.0 && speed(b) < c.speed).map(|b| Block { body: b.body, lo: [b.lo[0] + b.vel[0].min(0.0), b.lo[1], b.lo[2] + b.vel[2].min(0.0)], hi: [b.hi[0] + b.vel[0].max(0.0), b.hi[1], b.hi[2] + b.vel[2].max(0.0)], vel: b.vel, fixed: false, shaped: false }).collect();
        let still: Vec<&Block> = standing.iter().copied().filter(|b| b.fixed || speed(b) < 20.0).chain(&swept).collect();
        // (A floor-sized thing does not make the ground floor-sized.)
        let (walked_lo, walked_hi) = (lo, hi);
        for b in &still {
            cover(&mut lo, &mut hi, [b.lo[0].max(walked_lo[0] - 40.0 * c.radius), b.lo[2].max(walked_lo[1] - 40.0 * c.radius)]);
            cover(&mut lo, &mut hi, [b.hi[0].min(walked_hi[0] + 40.0 * c.radius), b.hi[2].min(walked_hi[1] + 40.0 * c.radius)]);
        }
        let margin = 6.0 * c.radius;
        let (lo, hi) = ([lo[0] - margin, lo[1] - margin], [hi[0] + margin, hi[1] + margin]);
        let cell = (1.5 * c.radius).max((hi[0] - lo[0]).max(hi[1] - lo[1]) / 192.0);
        let (w, h) = ((((hi[0] - lo[0]) / cell).ceil() as usize).max(1), (((hi[1] - lo[1]) / cell).ceil() as usize).max(1));
        let mut shut = vec![false; w * h];
        let grow = c.radius + 0.35 * cell;
        for b in &still {
            let x = (((b.lo[0] - grow - lo[0]) / cell).floor().max(0.0) as usize, (((b.hi[0] + grow - lo[0]) / cell).floor().max(-1.0) as isize).min(w as isize - 1));
            let z = (((b.lo[2] - grow - lo[1]) / cell).floor().max(0.0) as usize, (((b.hi[2] + grow - lo[1]) / cell).floor().max(-1.0) as isize).min(h as isize - 1));
            for j in z.0 as isize..=z.1 {
                for i in x.0 as isize..=x.1 {
                    // (By the square's middle.)
                    let (cx, cz) = (lo[0] + (i as f64 + 0.5) * cell, lo[1] + (j as f64 + 0.5) * cell);
                    if cx > b.lo[0] - grow && cx < b.hi[0] + grow && cz > b.lo[2] - grow && cz < b.hi[2] + grow && (!b.shaped || far(b, [cx, cz]) < grow) {
                        shut[j as usize * w + i as usize] = true;
                    }
                }
            }
        }
        let walls = still.iter().map(|b| [b.lo[0] - grow - cell, b.lo[2] - grow - cell, b.hi[0] + grow + cell, b.hi[2] + grow + cell]).collect();
        Ground { origin: lo, cell, w, h, shut, walls }
    }

    fn square(&self, p: [f64; 2]) -> Option<usize> {
        let (i, j) = (((p[0] - self.origin[0]) / self.cell).floor(), ((p[1] - self.origin[1]) / self.cell).floor());
        (i >= 0.0 && j >= 0.0 && (i as usize) < self.w && (j as usize) < self.h).then(|| j as usize * self.w + i as usize)
    }

    fn middle(&self, i: usize) -> [f64; 2] {
        [self.origin[0] + ((i % self.w) as f64 + 0.5) * self.cell, self.origin[1] + ((i / self.w) as f64 + 0.5) * self.cell]
    }

    /// Whether the straight line from `a` to `b` is open (past where `a`
    /// itself stands: an agent pushed up against a wall still sees).
    fn sight(&self, a: [f64; 2], b: [f64; 2]) -> bool {
        // (Most lines pass nothing at all.)
        let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        if !self.walls.iter().any(|w| lo[0] < w[2] && hi[0] > w[0] && lo[1] < w[3] && hi[1] > w[1]) {
            return true;
        }
        let l = len2([b[0] - a[0], b[1] - a[1]]);
        let steps = (l / (0.5 * self.cell)).ceil().max(1.0) as usize;
        (0..=steps).all(|s| {
            let f = s as f64 / steps as f64;
            f * l < 1.5 * self.cell || self.square([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]).is_none_or(|i| !self.shut[i])
        })
    }

    /// The open square nearest `p`.
    fn open(&self, p: [f64; 2]) -> Option<usize> {
        let at = self.square([p[0].clamp(self.origin[0], self.origin[0] + self.w as f64 * self.cell - 1e-6), p[1].clamp(self.origin[1], self.origin[1] + self.h as f64 * self.cell - 1e-6)])?;
        if !self.shut[at] {
            return Some(at);
        }
        let (ci, cj) = ((at % self.w) as isize, (at / self.w) as isize);
        (1..(self.w.max(self.h) as isize)).find_map(|ring| {
            let mut best: Option<(f64, usize)> = None;
            for j in cj - ring..=cj + ring {
                for i in ci - ring..=ci + ring {
                    if (i - ci).abs().max((j - cj).abs()) != ring || i < 0 || j < 0 || i >= self.w as isize || j >= self.h as isize {
                        continue;
                    }
                    let k = j as usize * self.w + i as usize;
                    let m = self.middle(k);
                    let d = len2([m[0] - p[0], m[1] - p[1]]);
                    if !self.shut[k] && best.is_none_or(|b| d < b.0) {
                        best = Some((d, k));
                    }
                }
            }
            best.map(|b| b.1)
        })
    }

    /// The corners of the shortest open way from `from` to `to` (the
    /// squares' middles, then `to` itself); none when there is no way.
    fn route(&self, from: [f64; 2], to: [f64; 2]) -> Vec<[f64; 2]> {
        let (Some(start), Some(end)) = (self.open(from), self.open(to)) else { return Vec::new() };
        let (w, h) = (self.w as isize, self.h as isize);
        // A*, costs in tenths of a square (10 straight, 14 across).
        let far = |i: usize| {
            let (dx, dy) = (((i % self.w) as isize - (end % self.w) as isize).unsigned_abs() as u32, ((i / self.w) as isize - (end / self.w) as isize).unsigned_abs() as u32);
            10 * dx.max(dy) + 4 * dx.min(dy)
        };
        let mut cost = vec![u32::MAX; self.shut.len()];
        let mut came = vec![u32::MAX; self.shut.len()];
        let mut heap = std::collections::BinaryHeap::new();
        cost[start] = 0;
        heap.push(std::cmp::Reverse((far(start), start as u32)));
        while let Some(std::cmp::Reverse((f, at))) = heap.pop() {
            let at = at as usize;
            if at == end {
                break;
            }
            if f > cost[at] + far(at) {
                continue;
            }
            let (i, j) = ((at % self.w) as isize, (at / self.w) as isize);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let (x, y) = (i + dx, j + dy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let next = y as usize * self.w + x as usize;
                // (No way between two shut squares that touch at a corner.)
                if self.shut[next] || (dx != 0 && dy != 0 && (self.shut[j as usize * self.w + x as usize] || self.shut[y as usize * self.w + i as usize])) {
                    continue;
                }
                let g = cost[at] + if dx != 0 && dy != 0 { 14 } else { 10 };
                if g < cost[next] {
                    cost[next] = g;
                    came[next] = at as u32;
                    heap.push(std::cmp::Reverse((g + far(next), next as u32)));
                }
            }
        }
        if cost[end] == u32::MAX {
            return Vec::new();
        }
        let mut way = vec![if self.square(to) == Some(end) { to } else { self.middle(end) }];
        let mut at = end;
        while came[at] != u32::MAX {
            at = came[at] as usize;
            way.push(self.middle(at));
        }
        way.reverse();
        way
    }
}

fn len2(v: [f64; 2]) -> f64 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

/// The agent's place: on the level at `at`, turned from how it started
/// to face `heading`.
fn place(c: &CrowdInput, k: usize, at: [f64; 2], heading: [f64; 2]) -> Pose {
    let a = &c.agents[k];
    let turn = turn(c, k, heading);
    let q = lahza_motion_core::sim::quat_mul([0.0, libm::sin(turn / 2.0), 0.0, libm::cos(turn / 2.0)], a.start.q);
    Pose { p: [at[0], a.start.p[1], at[1]], q }
}

/// The turn about y that has agent `k` face `heading`.
fn turn(c: &CrowdInput, k: usize, heading: [f64; 2]) -> f64 {
    let f = lahza_motion_core::sim::quat_rotate(c.agents[k].start.q, [c.facing[0], 0.0, c.facing[1]]);
    // A turn about y takes the level angle down by its own.
    libm::atan2(f[2], f[0]) - libm::atan2(heading[1], heading[0])
}

fn sample_of(p: &Pose) -> [f32; 7] {
    [p.p[0] as f32, p.p[1] as f32, p.p[2] as f32, p.q[0] as f32, p.q[1] as f32, p.q[2] as f32, p.q[3] as f32]
}

impl Herd {
    pub fn new(input: &SimInput, world: &mut World) -> Herd {
        let m = 1.0 / world.unit;
        let mut agents = Vec::new();
        let mut proxies = HashMap::new();
        for (n, c) in input.crowds.iter().enumerate() {
            let mut herd = Vec::with_capacity(c.agents.len());
            for (k, a) in c.agents.iter().enumerate() {
                let mut rng = Rng(input.seed ^ ((n as u64) << 32 | k as u64).wrapping_mul(0xc2b2ae3d27d4eb4f) ^ 0x51ed);
                let f = lahza_motion_core::sim::quat_rotate(a.start.q, [c.facing[0], 0.0, c.facing[1]]);
                let l = len2([f[0], f[2]]).max(1e-9);
                let heading = [f[0] / l, f[2] / l];
                let at = [a.start.p[0], a.start.p[2]];
                let now = place(c, k, at, heading);
                // A capsule standing on the agent's origin.
                let half = (c.height / 2.0 - c.radius).max(0.0);
                let proxy = world.w.insert_body(RigidBodyBuilder::kinematic_position_based().pose(pose(&now, world.unit)));
                world.w.insert_collider(ColliderBuilder::capsule_y((half * m) as f32, (c.radius * m) as f32).translation(v3([0.0, -c.height / 2.0, 0.0], m)).friction(0.6), Some(proxy));
                proxies.insert(proxy, (n, k));
                let room = if c.instanced { 0 } else { input.samples as usize + 1 };
                herd.push(Agent {
                    at,
                    vel: [0.0; 2],
                    heading,
                    speed: c.speed * (1.0 + c.vary * rng.signed()),
                    walked: 0.0,
                    phase: (rng.signed() + 1.0) / 2.0,
                    down: None,
                    proxy,
                    path: Vec::new(),
                    dodged: None,
                    guard: 0.0,
                    wary: f64::NEG_INFINITY,
                    idle: false,
                    now,
                    next: now,
                    track: Track { id: a.id.clone(), from: 0, rest: a.rest, samples: Vec::with_capacity(room), clock: Vec::with_capacity(room) },
                });
            }
            agents.push(herd);
        }
        Herd {
            agents,
            events: Vec::new(),
            guards: vec![Vec::new(); input.crowds.len()],
            rows: input.crowds.iter().map(|c| Vec::with_capacity(if c.instanced { c.agents.len() * (input.samples as usize + 1) } else { 0 })).collect(),
            slots: input.crowds.iter().map(|c| vec![u32::MAX; if c.instanced { c.agents.len() } else { 0 }]).collect(),
            taken: vec![0; input.crowds.len()],
            sample: 0,
            proxies,
            quiet: false,
        }
    }

    /// A copy for tries ahead: the agents as they are, steering on from
    /// here, with nothing kept of where they go. Those farther from every
    /// one of `near` (level places, px) than an agent could come in twice
    /// `horizon` seconds stand where they are: they cannot matter to what
    /// is tried there, and a try's cost is by the agents that steer.
    /// (`near` None: a copy of a copy, the same agents steering.)
    pub fn fork(&self, input: &SimInput, near: Option<&[[f64; 2]]>, horizon: f64) -> Herd {
        let agents = self.agents.iter().zip(&input.crowds).map(|(herd, c)| {
            let range = 2.0 * (2.2 * c.speed * horizon + 8.0 * c.radius);
            herd.iter().map(|a| Agent {
                at: a.at, vel: a.vel, heading: a.heading, speed: a.speed, walked: a.walked, phase: a.phase, down: a.down, proxy: a.proxy, path: a.path.clone(), dodged: a.dodged, guard: a.guard, wary: a.wary,
                idle: a.idle || near.is_some_and(|near| !near.iter().any(|p| len2([p[0] - a.at[0], p[1] - a.at[1]]) < range)),
                now: a.now, next: a.next,
                track: Track { id: String::new(), from: 0, rest: a.track.rest, samples: Vec::new(), clock: Vec::new() },
            }).collect()
        }).collect();
        Herd { agents, events: Vec::new(), rows: Vec::new(), guards: Vec::new(), slots: Vec::new(), taken: Vec::new(), sample: self.sample, proxies: HashMap::new(), quiet: true }
    }

    /// How many of its agents steer (in a try's copy: those near enough).
    pub fn steering(&self) -> usize {
        self.agents.iter().flatten().filter(|a| a.down.is_none() && !a.idle).count()
    }

    /// The capsules of the agents that walk, each with its speed to the
    /// next sample (px/s).
    pub fn movers(&self, rate: f64) -> Vec<(RigidBodyHandle, [f64; 3])> {
        self.agents.iter().flatten().filter(|a| a.down.is_none()).map(|a| (a.proxy, std::array::from_fn(|k| (a.next.p[k] - a.now.p[k]) * rate))).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.iter().all(|a| a.is_empty())
    }

    fn clock(c: &CrowdInput, a: &Agent) -> f64 {
        let turns = a.walked / c.stride + a.phase;
        c.clip_at + (turns - turns.floor()) * c.cycle
    }

    /// Notes every agent's place at this sample, and works out the next.
    pub fn plan(&mut self, input: &SimInput, world: &World) {
        let u = world.unit;
        let dt = 1.0 / input.rate;
        let sample = self.sample;
        self.sample += 1;
        let t = input.start + sample as f64 * dt;
        // What stands in the way or comes flying: every body that is no
        // agent's own, as its box.
        let mut blocks: Vec<Block> = Vec::new();
        for (b, body) in input.bodies.iter().enumerate() {
            if body.release == u32::MAX {
                continue;
            }
            let rb = &world.w.bodies[world.handles[b]];
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for &c in rb.colliders() {
                let aabb = world.w.colliders[c].compute_aabb();
                for k in 0..3 {
                    lo[k] = lo[k].min(aabb.mins[k] as f64 * u);
                    hi[k] = hi[k].max(aabb.maxs[k] as f64 * u);
                }
            }
            // (A box on the axes, or a round thing standing, is its box;
            // a box turned, a hull, a mesh is not.)
            let shaped = rb.colliders().iter().any(|&c| {
                let col = &world.w.colliders[c];
                match col.shape().as_cuboid() {
                    Some(cuboid) => {
                        let e = col.compute_aabb().half_extents();
                        (e.x * e.y * e.z) as f64 > 1.02 * (cuboid.half_extents.x * cuboid.half_extents.y * cuboid.half_extents.z) as f64
                    }
                    None => !matches!(col.shape().shape_type(), ShapeType::Ball | ShapeType::Capsule | ShapeType::Cylinder | ShapeType::Cone),
                }
            });
            if lo[0] <= hi[0] {
                let v = rb.linvel();
                blocks.push(Block { body: b, lo, hi, vel: [v.x as f64 * u, v.y as f64 * u, v.z as f64 * u], fixed: body.kind == lahza_motion_core::sim::BodyKind::Fixed, shaped });
            }
        }
        // And who lies or is falling: each one down as the box of its
        // bones, whatever its height (a wall to plan round once it lies).
        // (Every crowd's fallen are in every crowd's way.)
        let mut lying: Vec<Block> = Vec::new();
        for (n, c) in input.crowds.iter().enumerate() {
            for (k, a) in self.agents[n].iter().enumerate() {
                let ragdoll = if c.instanced { self.slots.get(n).and_then(|s| c.pool.get(*s.get(k)? as usize)).copied() } else { c.agents[k].ragdoll };
                let (Some(_), Some(r)) = (a.down, ragdoll) else { continue };
                let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                for bone in &input.ragdolls[r].bones {
                    for &col in world.w.bodies[world.handles[bone.body]].colliders() {
                        let aabb = world.w.colliders[col].compute_aabb();
                        for x in 0..3 {
                            lo[x] = lo[x].min(aabb.mins[x] as f64 * u);
                            hi[x] = hi[x].max(aabb.maxs[x] as f64 * u);
                        }
                    }
                }
                let root = input.ragdolls[r].bones[0].body;
                let v = world.w.bodies[world.handles[root]].linvel();
                if lo[0] <= hi[0] {
                    lying.push(Block { body: root, lo, hi, vel: [v.x as f64 * u, v.y as f64 * u, v.z as f64 * u], fixed: false, shaped: false });
                }
            }
        }

        for (n, c) in input.crowds.iter().enumerate() {
            let places: Vec<[f64; 2]> = self.agents[n].iter().map(|a| a.at).collect();
            let headings: Vec<[f64; 2]> = self.agents[n].iter().map(|a| a.heading).collect();
            let vels: Vec<[f64; 2]> = self.agents[n].iter().map(|a| a.vel).collect();
            // (Who is where it was going, or down: it will not make room.)
            let settled: Vec<bool> = self.agents[n].iter().enumerate().map(|(k, a)| a.down.is_some() || len2([c.agents[k].goal[0] - a.at[0], c.agents[k].goal[1] - a.at[1]]) <= c.radius * 0.25).collect();
            // Who is near whom: the agents by the square they stand in
            // (three radii a side, as far as one pushes another).
            let reach = 3.0 * c.radius;
            let square = |at: [f64; 2]| ((at[0] / reach).floor() as i64, (at[1] / reach).floor() as i64);
            let mut squares: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
            for (j, at) in places.iter().enumerate() {
                squares.entry(square(*at)).or_default().push(j as u32);
            }
            let mut near: Vec<u32> = Vec::new();
            // What stands in these agents' height, and where they may
            // walk (worked out when the first of them needs it).
            let Some(feet) = c.agents.first().map(|a| a.start.p[1]) else { continue };
            // (y is down: a head is at feet − height.)
            let standing: Vec<&Block> = blocks.iter().filter(|b| b.hi[1] > feet - c.height && b.lo[1] < feet - 0.2 * c.height).chain(&lying).collect();
            let off_of = |b: &Block, p: [f64; 2]| outline(world, b, p, feet, c.height).0;
            let mut ground: Option<Ground> = None;
            let gravity = input.gravity[1] * u;
            for (k, a) in self.agents[n].iter_mut().enumerate() {
                a.now = a.next;
                if self.quiet {
                } else if c.instanced {
                    self.rows[n].push([a.now.p[0] as f32, a.now.p[2] as f32, turn(c, k, a.heading) as f32, Self::clock(c, a) as f32]);
                    if c.guard.is_some() {
                        self.guards[n].push(a.guard as f32);
                    }
                } else {
                    a.track.samples.push(sample_of(&a.now));
                    a.track.clock.push(Self::clock(c, a) as f32);
                }
                if a.down.is_some() || a.idle {
                    continue;
                }
                let goal = c.agents[k].goal;
                let far = len2([goal[0] - a.at[0], goal[1] - a.at[1]]);
                let arrived = far <= c.radius * 0.25;
                // Where to head: the goal, or round what stands across the way.
                let mut toward = goal;
                if c.plan && !arrived && !standing.is_empty() {
                    let ground = ground.get_or_insert_with(|| Ground::new(c, &self_places(&places), &standing, &off_of));
                    if ground.sight(a.at, goal) {
                        a.path.clear();
                    } else {
                        // A way is kept while its next corner is in sight, and looked for again every second.
                        let stale = (sample as usize + k) % (input.rate.round().max(1.0) as usize) == 0;
                        if a.path.is_empty() || stale || !a.path.iter().any(|p| ground.sight(a.at, *p)) {
                            a.path = ground.route(a.at, goal);
                        }
                        // (The farthest of its corners in sight.)
                        if let Some(p) = a.path.iter().rev().find(|p| ground.sight(a.at, **p)).or(a.path.first()) {
                            toward = *p;
                        }
                    }
                }
                let to = [toward[0] - a.at[0], toward[1] - a.at[1]];
                let way = len2(to);
                // Slowing over its last two radii.
                let slow = (far / (2.0 * c.radius)).min(1.0);
                // Who is near it.
                near.clear();
                if !arrived {
                    // (In the agents' order, whatever the squares': the sum is the same sum.)
                    let (sx, sz) = square(a.at);
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            near.extend(squares.get(&(sx + dx, sz + dz)).into_iter().flatten());
                        }
                    }
                    near.sort_unstable();
                }
                // It waits for room: no faster than would close, in two
                // fifths of a second, on the nearest agent before it that
                // goes its way or waits itself (as fast as that one goes
                // on, and what is between them besides), and never under a fifth
                // of its pace (a ring of them each waiting for the next
                // would stand for good). One that comes the other way, or
                // has arrived, is passed, not waited for.
                let mut pace = a.speed * slow;
                if way > 1e-9 {
                    let dir = [to[0] / way, to[1] / way];
                    for &j in near.iter().filter(|&&j| j as usize != k) {
                        let (d, h) = ([places[j as usize][0] - a.at[0], places[j as usize][1] - a.at[1]], headings[j as usize]);
                        let (along, across) = (d[0] * dir[0] + d[1] * dir[1], (d[0] * dir[1] - d[1] * dir[0]).abs());
                        if along > 0.0 && across < 1.6 * c.radius && h[0] * dir[0] + h[1] * dir[1] > -0.3 && !settled[j as usize] {
                            let on = (vels[j as usize][0] * dir[0] + vels[j as usize][1] * dir[1]).max(0.0);
                            pace = pace.min((on + (along - 2.0 * c.radius) / 0.4).max(0.2 * a.speed * slow));
                        }
                    }
                }
                let mut want = if !arrived && way > 1e-9 { [to[0] / way * pace, to[1] / way * pace] } else { [0.0; 2] };
                // (An agent's right, as it faces.)
                let right = [-a.heading[1], a.heading[0]];
                let mut push = |d: [f64; 2], reach: f64, hard: f64, oncoming: bool| {
                    let l = len2(d);
                    if l >= reach {
                        return;
                    }
                    // Straight off it, and round it on the goal's side; one
                    // that comes the other way is passed on the right (it
                    // keeps to its own right too: they do not dance).
                    let away = if l > 1e-6 { [d[0] / l, d[1] / l] } else { [a.heading[1], -a.heading[0]] };
                    let side = [-away[1], away[0]];
                    let toward = if oncoming { right } else { to };
                    let s = if side[0] * toward[0] + side[1] * toward[1] >= 0.0 { 1.0 } else { -1.0 };
                    let f = hard * a.speed * (reach - l) / reach;
                    want[0] += away[0] * f + side[0] * s * f * 0.6;
                    want[1] += away[1] * f + side[1] * s * f * 0.6;
                };
                if !arrived {
                    for &j in &near {
                        if j as usize != k {
                            let h = headings[j as usize];
                            push([a.at[0] - places[j as usize][0], a.at[1] - places[j as usize][1]], reach, 1.6, h[0] * a.heading[0] + h[1] * a.heading[1] < -0.3);
                        }
                    }
                    // Off the nearest point of each thing's outline (its
                    // middle, from inside it).
                    for b in &standing {
                        let (off, nearest) = outline(world, b, a.at, feet, c.height);
                        let mut d = [a.at[0] - nearest[0], a.at[1] - nearest[1]];
                        // (Off a turned thing's side, not its box's.)
                        if b.shaped && off >= 3.0 * c.radius {
                            continue;
                        }
                        if len2(d) < 1e-6 {
                            d = [a.at[0] - (b.lo[0] + b.hi[0]) / 2.0, a.at[1] - (b.lo[2] + b.hi[2]) / 2.0];
                            let l = len2(d).max(1e-6);
                            d = [d[0] / l * 1e-3, d[1] / l * 1e-3];
                        }
                        push(d, 3.0 * c.radius, 4.0, false);
                    }
                }
                // What is coming at it: a body that, as it flies, will be
                // on it within `dodge` seconds at a speed that would
                // knock it down. It steps out of its line.
                let mut dodging = false;
                // (And, with a guard, what would be on it within the
                // guard's time: its guard goes up.)
                let ahead = c.guard.unwrap_or(0.0);
                let mut threatened = false;
                if c.dodge > 0.0 || ahead > 0.0 {
                    for b in blocks.iter().chain(&lying).filter(|b| !b.fixed) {
                        let rel = [b.vel[0] - a.vel[0], b.vel[1], b.vel[2] - a.vel[1]];
                        if (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt() < c.knock {
                            continue;
                        }
                        let (p, v) = ([(b.lo[0] + b.hi[0]) / 2.0 - a.at[0], (b.lo[2] + b.hi[2]) / 2.0 - a.at[1]], [rel[0], rel[2]]);
                        let (vv, closing) = (v[0] * v[0] + v[1] * v[1], -(p[0] * v[0] + p[1] * v[1]));
                        if vv < 1.0 || closing <= 0.0 {
                            continue;
                        }
                        let when = closing / vv;
                        if when > c.dodge.max(ahead) {
                            continue;
                        }
                        let miss = [p[0] + v[0] * when, p[1] + v[1] * when];
                        let size = ((b.hi[0] - b.lo[0]).max(b.hi[2] - b.lo[2]) / 2.0).min((b.hi[1] - b.lo[1]) / 2.0 * 4.0);
                        let y = (b.lo[1] + b.hi[1]) / 2.0 + b.vel[1] * when + 0.5 * gravity * when * when;
                        let tall = (b.hi[1] - b.lo[1]) / 2.0;
                        if len2(miss) >= 1.5 * c.radius + size || y + tall < feet - c.height || y - tall > feet {
                            continue;
                        }
                        threatened |= when <= ahead;
                        if when > c.dodge {
                            continue;
                        }
                        // Away from its line, to the side it already is.
                        let l = len2(miss);
                        let out = if l > 1e-6 { [-miss[0] / l, -miss[1] / l] } else { [-v[1] / vv.sqrt(), v[0] / vv.sqrt()] };
                        want[0] += out[0] * a.speed * 3.0;
                        want[1] += out[1] * a.speed * 3.0;
                        dodging = true;
                        if a.dodged != Some(b.body) && !self.quiet {
                            a.dodged = Some(b.body);
                            let who = if c.instanced { format!("{}#{k}", c.id) } else { a.track.id.clone() };
                            self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "dodge".into(), a: who, b: input.bodies[b.body].id.clone(), speed: (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt().round() });
                        }
                    }
                }
                // Its guard: up in a sixth of a second, held half a
                // second after the last threat, down over half a second.
                if c.guard.is_some() {
                    if threatened {
                        a.wary = t + 0.5;
                    }
                    a.guard = if t < a.wary { (a.guard + dt / 0.16).min(1.0) } else { (a.guard - dt / 0.5).max(0.0) };
                }
                // (A dodge is a lunge: twice its pace, taken up at once.)
                let (most, take) = if dodging { (2.2 * a.speed, 0.6) } else { (a.speed, 0.25) };
                let l = len2(want);
                if l > most {
                    want = [want[0] / l * most, want[1] / l * most];
                }
                a.vel = [a.vel[0] + (want[0] - a.vel[0]) * take, a.vel[1] + (want[1] - a.vel[1]) * take];
                let moved = len2(a.vel) * dt;
                a.at = [a.at[0] + a.vel[0] * dt, a.at[1] + a.vel[1] * dt];
                a.walked += moved;
                // It turns to the way it walks, half a turn a second at most.
                if len2(a.vel) > 0.05 * a.speed {
                    let (h, v) = (a.heading, [a.vel[0] / len2(a.vel), a.vel[1] / len2(a.vel)]);
                    let off = libm::atan2(h[0] * v[1] - h[1] * v[0], h[0] * v[0] + h[1] * v[1]);
                    let step = off.clamp(-std::f64::consts::PI * dt, std::f64::consts::PI * dt);
                    let (s, co) = (libm::sin(step), libm::cos(step));
                    a.heading = [h[0] * co - h[1] * s, h[0] * s + h[1] * co];
                }
            }
            // None stands in another: two nearer than 1.7 radii are moved
            // apart, each by half (one that is down or stands, not at
            // all), twice over; and none is moved into what is shut. (At
            // two radii, tried: the six of the tests, started 80 px
            // apart, hold each other in a ball's line.)
            let apart = 1.7 * c.radius;
            let mut at: Vec<[f64; 2]> = self.agents[n].iter().map(|a| a.at).collect();
            let fixed: Vec<bool> = self.agents[n].iter().map(|a| a.down.is_some() || a.idle).collect();
            for _ in 0..2 {
                let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
                let cell = |p: [f64; 2]| ((p[0] / apart).floor() as i64, (p[1] / apart).floor() as i64);
                for (j, p) in at.iter().enumerate() {
                    cells.entry(cell(*p)).or_default().push(j as u32);
                }
                for i in 0..at.len() {
                    let (cx, cz) = cell(at[i]);
                    near.clear();
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            near.extend(cells.get(&(cx + dx, cz + dz)).into_iter().flatten().filter(|&&j| (j as usize) < i));
                        }
                    }
                    near.sort_unstable();
                    for &j in &near {
                        let j = j as usize;
                        let d = [at[i][0] - at[j][0], at[i][1] - at[j][1]];
                        let l = len2(d);
                        // (By one that has arrived there is less room to
                        // ask: goals may be set nearer than agents pass.)
                        let apart = if settled[i] || settled[j] { 1.2 * c.radius } else { apart };
                        if l >= apart || fixed[i] && fixed[j] {
                            continue;
                        }
                        let away = if l > 1e-6 { [d[0] / l, d[1] / l] } else { [1.0, 0.0] };
                        let (mine, its) = match (fixed[i], fixed[j]) {
                            (false, false) => (0.5, 0.5),
                            (false, true) => (1.0, 0.0),
                            _ => (0.0, 1.0),
                        };
                        let open = |p: [f64; 2]| ground.as_ref().is_none_or(|g| g.square(p).is_none_or(|q| !g.shut[q]));
                        let (pi, pj) = ([at[i][0] + away[0] * (apart - l) * mine, at[i][1] + away[1] * (apart - l) * mine], [at[j][0] - away[0] * (apart - l) * its, at[j][1] - away[1] * (apart - l) * its]);
                        if open(pi) || !open(at[i]) {
                            at[i] = pi;
                        }
                        if open(pj) || !open(at[j]) {
                            at[j] = pj;
                        }
                    }
                }
            }
            for (k, a) in self.agents[n].iter_mut().enumerate() {
                if a.down.is_some() || a.idle {
                    continue;
                }
                a.at = at[k];
                a.next = place(c, k, a.at, a.heading);
            }
        }
    }

    /// Moves the agents' capsules toward `f` (0..1) of the way to the
    /// next sample, for the step about to be taken.
    pub fn drive(&self, world: &mut World, f: f64) {
        for a in self.agents.iter().flatten().filter(|a| a.down.is_none() && !a.idle) {
            let p = Pose { p: std::array::from_fn(|k| a.now.p[k] + (a.next.p[k] - a.now.p[k]) * f), q: a.next.q };
            world.w.bodies[a.proxy].set_next_kinematic_position(pose(&p, world.unit));
        }
    }

    /// After a step: agents a body struck at speed go down. `k` is the
    /// sample the step is in, `t` its time.
    /// Returns, for each ragdoll let go, the turns that have its motors
    /// hold the pose it was let go in.
    pub fn knocks(&mut self, input: &SimInput, world: &mut World, k: u32, t: f64) -> Vec<(usize, Vec<Option<[f64; 4]>>)> {
        let mut held = Vec::new();
        let u = world.unit;
        let mut struck: Vec<(usize, usize, usize, f64)> = Vec::new();
        for pair in world.w.contact_pairs() {
            if !pair.has_any_active_contact() {
                continue;
            }
            let (b1, b2) = (world.w.colliders[pair.collider1].parent(), world.w.colliders[pair.collider2].parent());
            for (mine, other) in [(b1, b2), (b2, b1)] {
                let Some((n, a)) = mine.and_then(|h| self.proxies.get(&h).copied()).filter(|&(n, a)| self.agents[n][a].down.is_none()) else { continue };
                let Some(body) = other.and_then(|h| world.handles.iter().position(|x| *x == h)) else { continue };
                if !world.released[body] {
                    continue;
                }
                let v = world.before.get(body).copied().unwrap_or(Vector::ZERO);
                let agent = &self.agents[n][a];
                let rel = [v.x as f64 * u - agent.vel[0], v.y as f64 * u, v.z as f64 * u - agent.vel[1]];
                let speed = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
                if speed >= input.crowds[n].knock {
                    struck.push((n, a, body, speed));
                }
            }
        }
        struck.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        struck.dedup_by_key(|s| (s.0, s.1));
        for (n, a, body, speed) in struck {
            let c = &input.crowds[n];
            let agent = &mut self.agents[n][a];
            agent.down = Some(k + 1);
            agent.next = agent.now;
            let who = if c.instanced { format!("{}#{a}", c.id) } else { agent.track.id.clone() };
            self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "knock".into(), a: who, b: input.bodies[body].id.clone(), speed: speed.round() });
            // Its capsule is gone; its bones take over where the clip has them.
            let colliders: Vec<ColliderHandle> = world.w.bodies[agent.proxy].colliders().to_vec();
            for h in colliders {
                world.w.colliders[h].set_enabled(false);
            }
            // Its own ragdoll, or the next of the pool while there is one.
            let (ragdoll, local) = if c.instanced {
                let slot = self.taken[n];
                if slot < c.pool.len() {
                    self.taken[n] += 1;
                    self.slots[n][a] = slot as u32;
                }
                (c.pool.get(slot).copied(), &c.local)
            } else {
                (c.agents[a].ragdoll, &c.agents[a].local)
            };
            let Some(r) = ragdoll else { continue };
            let cycle = local.first().map_or(1, |l| l.len().max(2) - 1) as f64;
            let turns = agent.walked / c.stride + agent.phase;
            let at = (turns - turns.floor()) * cycle;
            let root = world.w.bodies[agent.proxy].position();
            let root = Pose { p: [root.translation.x as f64 * u, root.translation.y as f64 * u, root.translation.z as f64 * u], q: [root.rotation.x as f64, root.rotation.y as f64, root.rotation.z as f64, root.rotation.w as f64] };
            // Samples of the clip a second, as it was walking.
            let pace = len2(agent.vel) / c.stride * cycle;
            let mut placed: Vec<Pose> = Vec::with_capacity(local.len());
            for (bone, local) in input.ragdolls[r].bones.iter().zip(local) {
                let i = (at.floor() as usize).min(local.len() - 1);
                let j = (i + 1).min(local.len() - 1);
                let f = at - i as f64;
                let (p, q) = (&local[i], &local[j]);
                let s = if (0..4).map(|x| p.q[x] * q.q[x]).sum::<f64>() < 0.0 { -1.0 } else { 1.0 };
                let mut turn: [f64; 4] = std::array::from_fn(|x| p.q[x] + (s * q.q[x] - p.q[x]) * f);
                let l = turn.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
                turn = turn.map(|v| v / l);
                let here = root.then(&Pose { p: std::array::from_fn(|x| p.p[x] + (q.p[x] - p.p[x]) * f), q: turn });
                placed.push(here);
                // Its own swing in the clip: how it moves and turns from
                // this sample of the cycle to the next, at the agent's pace.
                let swing = lahza_motion_core::sim::quat_rotate(root.q, std::array::from_fn(|x| (q.p[x] - p.p[x]) * pace));
                let d = lahza_motion_core::sim::quat_mul(q.q.map(|v| v * s), [-p.q[0], -p.q[1], -p.q[2], p.q[3]]);
                let d = if d[3] < 0.0 { d.map(|v| -v) } else { d };
                let sine = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let spin = if sine < 1e-12 { [0.0; 3] } else { lahza_motion_core::sim::quat_rotate(root.q, std::array::from_fn(|x| d[x] / sine * 2.0 * libm::atan2(sine, d[3]) * pace)) };
                let h = world.handles[bone.body];
                let rb = &mut world.w.bodies[h];
                rb.set_enabled(true);
                rb.set_position(pose(&here, u), true);
                rb.set_body_type(RigidBodyType::Dynamic, true);
                // It was walking: every bone has the agent's speed and its own in the clip.
                rb.set_linvel(v3([agent.vel[0] + swing[0], swing[1], agent.vel[1] + swing[2]], 1.0 / u), true);
                rb.set_angvel(v3(spin, 1.0), true);
                let colliders: Vec<ColliderHandle> = rb.colliders().to_vec();
                for ch in colliders {
                    world.w.colliders[ch].set_enabled(true);
                }
                world.released[bone.body] = true;
            }
            // Each bone's turn against its parent as it fell, on top of
            // how it is bound (what its motors aim for from now).
            let bones = &input.ragdolls[r].bones;
            held.push((r, bones.iter().enumerate().map(|(b, bone)| {
                let parent = placed.get(bone.parent?)?;
                let against = lahza_motion_core::sim::quat_mul([-parent.q[0], -parent.q[1], -parent.q[2], parent.q[3]], placed.get(b)?.q);
                Some(lahza_motion_core::sim::quat_mul(against, [-bone.bind[0], -bone.bind[1], -bone.bind[2], bone.bind[3]]))
            }).collect()));
        }
        held
    }

    /// The agents' tracks (after the last `plan`), and for each bone body
    /// let go the sample it was let go at.
    pub fn finish(mut self, input: &SimInput) -> (Vec<Track>, Vec<(usize, u32)>, Vec<HerdTrack>) {
        let mut tracks = Vec::new();
        let mut released = Vec::new();
        let mut herds = Vec::new();
        for (n, herd) in self.agents.into_iter().enumerate() {
            let c = &input.crowds[n];
            if c.instanced {
                for (k, a) in herd.iter().enumerate() {
                    if let (Some(at), Some(&r)) = (a.down, c.pool.get(self.slots[n][k] as usize)) {
                        released.extend(input.ragdolls[r].bones.iter().map(|b| (b.body, at)));
                    }
                }
                let Some(first) = c.agents.first() else { continue };
                herds.push(HerdTrack {
                    id: c.id.clone(),
                    count: c.agents.len() as u32,
                    rest: first.rest,
                    y: first.start.p[1],
                    q: first.start.q,
                    samples: std::mem::take(&mut self.rows[n]),
                    down: herd.iter().map(|a| a.down.unwrap_or(u32::MAX)).collect(),
                    slot: std::mem::take(&mut self.slots[n]),
                    guard: std::mem::take(&mut self.guards[n]),
                });
                continue;
            }
            for (k, a) in herd.into_iter().enumerate() {
                if let (Some(at), Some(r)) = (a.down, c.agents[k].ragdoll) {
                    released.extend(input.ragdolls[r].bones.iter().map(|b| (b.body, at)));
                }
                tracks.push(a.track);
            }
        }
        (tracks, released, herds)
    }
}
