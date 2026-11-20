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
use lahza_motion_core::sim::{CrowdInput, Pose, SimEvent, SimInput, Track};
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
    /// Its place at this sample and the next.
    now: Pose,
    next: Pose,
    track: Track,
}

pub struct Herd {
    agents: Vec<Vec<Agent>>,
    pub events: Vec<SimEvent>,
}

fn len2(v: [f64; 2]) -> f64 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

/// The agent's place: on the level at `at`, turned from how it started
/// to face `heading`.
fn place(c: &CrowdInput, k: usize, at: [f64; 2], heading: [f64; 2]) -> Pose {
    let a = &c.agents[k];
    let f = lahza_motion_core::sim::quat_rotate(a.start.q, [c.facing[0], 0.0, c.facing[1]]);
    // A turn about y takes the level angle down by its own.
    let turn = libm::atan2(f[2], f[0]) - libm::atan2(heading[1], heading[0]);
    let q = lahza_motion_core::sim::quat_mul([0.0, libm::sin(turn / 2.0), 0.0, libm::cos(turn / 2.0)], a.start.q);
    Pose { p: [at[0], a.start.p[1], at[1]], q }
}

fn sample(p: &Pose) -> [f32; 7] {
    [p.p[0] as f32, p.p[1] as f32, p.p[2] as f32, p.q[0] as f32, p.q[1] as f32, p.q[2] as f32, p.q[3] as f32]
}

impl Herd {
    pub fn new(input: &SimInput, world: &mut World) -> Herd {
        let m = 1.0 / world.unit;
        let mut agents = Vec::new();
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
                herd.push(Agent {
                    at,
                    vel: [0.0; 2],
                    heading,
                    speed: c.speed * (1.0 + c.vary * rng.signed()),
                    walked: 0.0,
                    phase: (rng.signed() + 1.0) / 2.0,
                    down: None,
                    proxy,
                    now,
                    next: now,
                    track: Track { id: a.id.clone(), from: 0, rest: a.rest, samples: Vec::with_capacity(input.samples as usize + 1), clock: Vec::with_capacity(input.samples as usize + 1) },
                });
            }
            agents.push(herd);
        }
        Herd { agents, events: Vec::new() }
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
        // What stands in the way: the level outline of every body that is
        // no agent's own, with the heights it spans.
        let mut blocks: Vec<([f64; 2], f64, f64, f64)> = Vec::new();
        for (b, body) in input.bodies.iter().enumerate() {
            if body.release == u32::MAX {
                continue;
            }
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for &c in world.w.bodies[world.handles[b]].colliders() {
                let aabb = world.w.colliders[c].compute_aabb();
                for k in 0..3 {
                    lo[k] = lo[k].min(aabb.mins[k] as f64 * u);
                    hi[k] = hi[k].max(aabb.maxs[k] as f64 * u);
                }
            }
            if lo[0] <= hi[0] {
                blocks.push(([(lo[0] + hi[0]) / 2.0, (lo[2] + hi[2]) / 2.0], (hi[0] - lo[0]).max(hi[2] - lo[2]) / 2.0, lo[1], hi[1]));
            }
        }
        for (n, c) in input.crowds.iter().enumerate() {
            let places: Vec<([f64; 2], bool)> = self.agents[n].iter().map(|a| (a.at, a.down.is_some())).collect();
            for (k, a) in self.agents[n].iter_mut().enumerate() {
                a.now = a.next;
                a.track.samples.push(sample(&a.now));
                a.track.clock.push(Self::clock(c, a) as f32);
                if a.down.is_some() {
                    continue;
                }
                let goal = c.agents[k].goal;
                let to = [goal[0] - a.at[0], goal[1] - a.at[1]];
                let far = len2(to);
                // Slowing over its last two radii.
                let mut want = if far > c.radius * 0.25 { [to[0] / far * a.speed * (far / (2.0 * c.radius)).min(1.0), to[1] / far * a.speed * (far / (2.0 * c.radius)).min(1.0)] } else { [0.0; 2] };
                let arrived = far <= c.radius * 0.25;
                let mut push = |from: [f64; 2], reach: f64, hard: f64| {
                    let d = [a.at[0] - from[0], a.at[1] - from[1]];
                    let l = len2(d);
                    if l >= reach {
                        return;
                    }
                    // Straight off it, and round it on the goal's side.
                    let away = if l > 1e-6 { [d[0] / l, d[1] / l] } else { [a.heading[1], -a.heading[0]] };
                    let side = [-away[1], away[0]];
                    let s = if side[0] * to[0] + side[1] * to[1] >= 0.0 { 1.0 } else { -1.0 };
                    let f = hard * a.speed * (reach - l) / reach;
                    want[0] += away[0] * f + side[0] * s * f * 0.6;
                    want[1] += away[1] * f + side[1] * s * f * 0.6;
                };
                if !arrived {
                    for (j, (other, _)) in places.iter().enumerate() {
                        if j != k {
                            push(*other, 3.0 * c.radius, 1.6);
                        }
                    }
                    let feet = c.agents[k].start.p[1];
                    for (centre, half, top, bottom) in &blocks {
                        // (y is down: its head is at feet − height.)
                        if *bottom > feet - c.height && *top < feet - 0.2 * c.height {
                            push(*centre, half + 3.0 * c.radius, 4.0);
                        }
                    }
                }
                let l = len2(want);
                if l > a.speed {
                    want = [want[0] / l * a.speed, want[1] / l * a.speed];
                }
                a.vel = [a.vel[0] + (want[0] - a.vel[0]) * 0.25, a.vel[1] + (want[1] - a.vel[1]) * 0.25];
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
                a.next = place(c, k, a.at, a.heading);
            }
        }
    }

    /// Moves the agents' capsules toward `f` (0..1) of the way to the
    /// next sample, for the step about to be taken.
    pub fn drive(&self, world: &mut World, f: f64) {
        for a in self.agents.iter().flatten().filter(|a| a.down.is_none()) {
            let p = Pose { p: std::array::from_fn(|k| a.now.p[k] + (a.next.p[k] - a.now.p[k]) * f), q: a.next.q };
            world.w.bodies[a.proxy].set_next_kinematic_position(pose(&p, world.unit));
        }
    }

    /// After a step: agents a body struck at speed go down. `k` is the
    /// sample the step is in, `t` its time.
    pub fn knocks(&mut self, input: &SimInput, world: &mut World, k: u32, t: f64) {
        let u = world.unit;
        let mut struck: Vec<(usize, usize, usize, f64)> = Vec::new();
        for pair in world.w.contact_pairs() {
            if !pair.has_any_active_contact() {
                continue;
            }
            let (b1, b2) = (world.w.colliders[pair.collider1].parent(), world.w.colliders[pair.collider2].parent());
            for (mine, other) in [(b1, b2), (b2, b1)] {
                let Some((n, a)) = self.agents.iter().enumerate().find_map(|(n, herd)| herd.iter().position(|a| Some(a.proxy) == mine && a.down.is_none()).map(|a| (n, a))) else { continue };
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
            self.events.push(SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "knock".into(), a: agent.track.id.clone(), b: input.bodies[body].id.clone(), speed: speed.round() });
            // Its capsule is gone; its bones take over where the clip has them.
            let colliders: Vec<ColliderHandle> = world.w.bodies[agent.proxy].colliders().to_vec();
            for h in colliders {
                world.w.colliders[h].set_enabled(false);
            }
            let Some(r) = c.agents[a].ragdoll else { continue };
            let cycle = c.agents[a].local.first().map_or(1, |l| l.len().max(2) - 1) as f64;
            let turns = agent.walked / c.stride + agent.phase;
            let at = (turns - turns.floor()) * cycle;
            let root = world.w.bodies[agent.proxy].position();
            let root = Pose { p: [root.translation.x as f64 * u, root.translation.y as f64 * u, root.translation.z as f64 * u], q: [root.rotation.x as f64, root.rotation.y as f64, root.rotation.z as f64, root.rotation.w as f64] };
            for (bone, local) in input.ragdolls[r].bones.iter().zip(&c.agents[a].local) {
                let i = (at.floor() as usize).min(local.len() - 1);
                let j = (i + 1).min(local.len() - 1);
                let f = at - i as f64;
                let (p, q) = (&local[i], &local[j]);
                let s = if (0..4).map(|x| p.q[x] * q.q[x]).sum::<f64>() < 0.0 { -1.0 } else { 1.0 };
                let mut turn: [f64; 4] = std::array::from_fn(|x| p.q[x] + (s * q.q[x] - p.q[x]) * f);
                let l = turn.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
                turn = turn.map(|v| v / l);
                let here = root.then(&Pose { p: std::array::from_fn(|x| p.p[x] + (q.p[x] - p.p[x]) * f), q: turn });
                let h = world.handles[bone.body];
                let rb = &mut world.w.bodies[h];
                rb.set_position(pose(&here, u), true);
                rb.set_body_type(RigidBodyType::Dynamic, true);
                // It was walking: every bone has the agent's speed.
                rb.set_linvel(v3([agent.vel[0], 0.0, agent.vel[1]], 1.0 / u), true);
                rb.set_angvel(Vector::ZERO, true);
                let colliders: Vec<ColliderHandle> = rb.colliders().to_vec();
                for ch in colliders {
                    world.w.colliders[ch].set_enabled(true);
                }
                world.released[bone.body] = true;
            }
        }
    }

    /// The agents' tracks (after the last `plan`), and for each bone body
    /// let go the sample it was let go at.
    pub fn finish(self, input: &SimInput) -> (Vec<Track>, Vec<(usize, u32)>) {
        let mut tracks = Vec::new();
        let mut released = Vec::new();
        for (n, herd) in self.agents.into_iter().enumerate() {
            for (k, a) in herd.into_iter().enumerate() {
                if let (Some(at), Some(r)) = (a.down, input.crowds[n].agents[k].ragdoll) {
                    released.extend(input.ragdolls[r].bones.iter().map(|b| (b.body, at)));
                }
                tracks.push(a.track);
            }
        }
        (tracks, released)
    }
}
