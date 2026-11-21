//! Simulation for the motion engine: a scene's bodies, joints and pushes
//! (`lahza_motion_core::sim::SimInput`) stepped with rapier at a fixed
//! rate and kept per frame as a `Bake`, which drawing reads.
//!
//! The same input bakes to the same bytes: rapier is built with
//! `enhanced-determinism` (no SIMD, no threads, libm's maths), the step is
//! fixed, and bodies, colliders and joints are made in the input's order.

pub mod behaviour;
pub mod crowd;
pub mod policy;
pub mod ragdoll;
pub mod search;
mod world;

pub use world::{core_meshes, Meshes, World};

use lahza_motion_core::sim::{Bake, BodyKind, SimInput, Track};
use lahza_motion_core::Compiled;

/// What a bake took.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub bodies: usize,
    pub joints: usize,
    pub samples: u32,
    pub steps: u64,
    /// Wall-clock seconds of stepping.
    pub seconds: f64,
    /// A policy's runs and the seconds they took.
    pub policy_runs: u64,
    pub policy_seconds: f64,
    /// Steps a search took in its tries ahead, the seconds they took, and
    /// how many tries ran at once.
    pub tried_steps: u64,
    pub search_seconds: f64,
    pub search_threads: usize,
}

/// A take in progress: the world, and each ragdoll's brain and the turns
/// its controller has it make on top of its clip.
pub struct Take {
    pub world: World,
    pub brains: Vec<behaviour::Brain>,
    pub turns: Vec<Option<Vec<Option<[f64; 4]>>>>,
    /// What is moved, not pushed, and by something outside the take (a
    /// crowd's agents): each with its speed (px/s). In a try ahead they
    /// go on as they were going.
    pub movers: Vec<(rapier3d::prelude::RigidBodyHandle, [f64; 3])>,
    /// And, for a take a search looks ahead in, the crowds as they are:
    /// in a try the agents near what is tried steer on as in the take.
    pub crowd: Option<crowd::Herd>,
    trial: bool,
}

impl Take {
    pub fn new(input: &SimInput, meshes: Meshes) -> Result<Take, String> {
        Ok(Take { world: World::new(input, meshes)?, brains: input.ragdolls.iter().map(|r| behaviour::Brain::new(input, r)).collect(), turns: vec![None; input.ragdolls.len()], movers: Vec::new(), crowd: None, trial: false })
    }

    /// A copy that goes its own way from here (a try ahead).
    pub fn fork(&self, input: &SimInput) -> Take {
        Take { world: self.world.fork(), brains: self.brains.clone(), turns: self.turns.clone(), movers: self.movers.clone(), crowd: self.crowd.as_ref().map(|h| h.fork(input, None, 0.0)), trial: true }
    }

    /// In a try, at a sample's start: its crowds' agents steer.
    pub fn plan_crowd(&mut self, input: &SimInput) {
        if let Some(mut herd) = self.crowd.take() {
            herd.plan(input, &self.world);
            self.crowd = Some(herd);
        }
    }

    /// Lets go the bodies whose release is sample `k`, where their keys
    /// have them.
    pub fn release_due(&mut self, input: &SimInput, k: u32) {
        for (i, b) in input.bodies.iter().enumerate() {
            if b.kind == BodyKind::Dynamic && !self.world.released[i] && b.release == k {
                self.world.release(input, i, b);
            }
        }
        // An emitter's particles: born into the world, and taken out.
        for (i, b) in input.bodies.iter().enumerate() {
            for life in &b.lives {
                if life.to == k && life.from < k {
                    self.world.gone(i);
                }
                if life.from == k {
                    self.world.born(input, i, b, life);
                }
            }
        }
    }

    /// Step `s` of the steps between samples `k` and `k + 1`.
    pub fn step(&mut self, input: &SimInput, k: u32, s: u64) {
        let n = input.substeps as u64;
        let t = input.start + (k as f64 + s as f64 / n as f64) / input.rate;
        let x = k as f64 + (s + 1) as f64 / n as f64;
        self.world.drive(input, x);
        self.world.push(input, t);
        if let (true, Some(herd)) = (self.trial, &self.crowd) {
            herd.drive(&mut self.world, (s + 1) as f64 / n as f64);
        } else if self.trial {
            let step = self.world.dt / self.world.unit;
            for (h, v) in &self.movers {
                let body = &mut self.world.w.bodies[*h];
                let mut at = *body.position();
                at.translation += rapier3d::prelude::Vector::new((v[0] * step) as f32, (v[1] * step) as f32, (v[2] * step) as f32);
                body.set_next_kinematic_position(at);
            }
        }
        for (rig, r) in input.ragdolls.iter().enumerate() {
            let drive = self.brains[rig].think(&mut self.world, input, r, x, t, self.turns[rig].as_deref());
            self.world.drive_ragdoll(r, rig, x, Some(&drive.targets), Some(&drive.scale));
            self.world.free[rig] = drive.pass;
            self.world.slip[rig] = drive.slip;
            self.world.hit_ragdoll(input, r, t);
        }
        self.world.step(input, t);
    }
}

/// What sets a ragdoll's motors besides its clip and behaviours.
enum Mind {
    Policy(policy::Policy),
    Search(search::Search),
}

/// Reads a project file a simulation names (a policy).
pub type Files<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>, String>;

/// Steps `input` from its start and returns every dynamic body's pose per
/// sample.
pub fn bake(input: &SimInput, meshes: Meshes) -> Result<(Bake, Stats), String> {
    bake_with(input, meshes, &|src| Err(format!("{src}: no project folder to read it from")))
}

/// `bake`, with the files controllers name read by `files`.
pub fn bake_with(input: &SimInput, meshes: Meshes, files: Files) -> Result<(Bake, Stats), String> {
    bake_watched(input, meshes, files, &|_| true)
}

/// Told the share of a bake done (0 to 1) before each sample; `false`
/// stops the bake there.
pub type Watch<'a> = &'a dyn Fn(f64) -> bool;

/// The error of a bake its `Watch` stopped.
pub const STOPPED: &str = "the bake was stopped";

/// `bake_with`, watched: `watch` hears how far it is and can stop it.
pub fn bake_watched(input: &SimInput, meshes: Meshes, files: Files, watch: Watch) -> Result<(Bake, Stats), String> {
    use lahza_motion_core::sim::Controller;
    let began = std::time::Instant::now();
    let mut take = Take::new(input, meshes)?;
    let mut minds: Vec<Option<Mind>> = Vec::new();
    for (rig, r) in input.ragdolls.iter().enumerate() {
        minds.push(match &r.controller {
            None => None,
            Some(Controller::Policy { src }) => {
                let feet = take.brains[rig].feet().len();
                Some(Mind::Policy(policy::Policy::load(&files(src)?, policy::observation_len(r, feet), policy::action_len(r)).map_err(|e| format!("{src}: {e}"))?))
            }
            Some(Controller::Search { samples, horizon, every, spread, swing, reach }) => Some(Mind::Search(search::Search::new(*samples, *horizon, *every, *spread).with(*swing, *reach))),
        });
    }
    let dynamic: Vec<usize> = (0..input.bodies.len()).filter(|&i| input.bodies[i].kind == BodyKind::Dynamic).collect();
    let mut tracks: Vec<Track> = dynamic
        .iter()
        .map(|&i| {
            let b = &input.bodies[i];
            Track { id: b.id.clone(), from: b.release, rest: b.rest, samples: Vec::with_capacity(input.samples as usize + 1), clock: Vec::new() }
        })
        .collect();
    let record = |world: &World, tracks: &mut Vec<Track>| {
        for (track, &i) in tracks.iter_mut().zip(&dynamic) {
            let p = world.pose(i);
            track.samples.push([p.p[0] as f32, p.p[1] as f32, p.p[2] as f32, p.q[0] as f32, p.q[1] as f32, p.q[2] as f32, p.q[3] as f32]);
        }
    };
    let mut herd = crowd::Herd::new(input, &mut take.world);
    let n = input.substeps as u64;
    let mut steps = 0u64;
    for k in 0..input.samples {
        if !watch(k as f64 / input.samples.max(1) as f64) {
            return Err(STOPPED.into());
        }
        take.release_due(input, k);
        record(&take.world, &mut tracks);
        herd.plan(input, &take.world);
        take.movers = herd.movers(input.rate);
        // (What a search tries ahead in has the agents near it steer.)
        let searching: Vec<(usize, f64)> = minds.iter().enumerate().filter_map(|(rig, m)| match m {
            Some(Mind::Search(s)) if take.world.released[input.ragdolls[rig].bones[0].body] => Some((rig, s.horizon)),
            _ => None,
        }).collect();
        // (`LAHZA_SIM_TRY_STRAIGHT`: they are carried straight on instead,
        // to measure what the steering costs.)
        take.crowd = (!herd.is_empty() && !searching.is_empty() && std::env::var_os("LAHZA_SIM_TRY_STRAIGHT").is_none()).then(|| {
            let near: Vec<[f64; 2]> = searching.iter().map(|(rig, _)| take.world.pose(input.ragdolls[*rig].bones[0].body).p).map(|p| [p[0], p[2]]).collect();
            herd.fork(input, Some(&near), searching.iter().map(|s| s.1).fold(0.0, f64::max))
        });
        // Once a frame, what sets the motors is asked.
        let t = input.start + k as f64 / input.rate;
        for (rig, mind) in minds.iter_mut().enumerate() {
            let r = &input.ragdolls[rig];
            if !take.world.released[r.bones[0].body] {
                continue;
            }
            match mind {
                Some(Mind::Policy(p)) => {
                    let feet = take.brains[rig].feet().to_vec();
                    let action = p.act(&policy::observe(&take.world, input, r, &feet, k as f64)).map_err(|e| format!("ragdoll '{}': {e}", r.id))?;
                    take.turns[rig] = Some(policy::turns(r, &action));
                }
                // (Down, it is its behaviours': it lies, and gets up if
                // it has `get_up`; the search takes it again once it stands.)
                Some(Mind::Search(_)) if take.brains[rig].fallen.is_some() => take.turns[rig] = None,
                Some(Mind::Search(s)) => {
                    let held = s.held;
                    let (turns, stride) = s.decide(&take, input, rig, k, t);
                    take.turns[rig] = Some(turns);
                    if let Some(st) = stride {
                        take.brains[rig].order_step(input, r, st.foot, st.from, st.to, st.lift, t, st.swung);
                    }
                    // (A hand sent to what stands by it, while the plans kept have it there.)
                    if let Some(h) = s.held {
                        take.brains[rig].order_reach(h.arm, h.to, t + s.every + 1e-6);
                        if held.is_none_or(|was| was.arm != h.arm || was.body != h.body) {
                            take.world.events.push(lahza_motion_core::sim::SimEvent { t: (t * 1000.0).round() / 1000.0, kind: "reach".into(), a: input.bodies[r.bones[take.brains[rig].hand(h.arm)].body].id.clone(), b: input.bodies[h.body].id.clone(), speed: 0.0 });
                        }
                    }
                }
                None => {}
            }
        }
        for s in 0..n {
            if !herd.is_empty() {
                herd.drive(&mut take.world, (s + 1) as f64 / n as f64);
            }
            take.step(input, k, s);
            if !herd.is_empty() {
                // (One knocked down holds, as far as its crowd's ragdolls
                // do, the pose it fell in.)
                let brains = &take.brains;
                let knocked = herd.knocks(input, &mut take.world, k, input.start + (k as f64 + (s + 1) as f64 / n as f64) / input.rate, &|rig| brains[rig].rising());
                for (rig, held) in knocked {
                    // (A fall is a ragdoll's first, whoever fell as it before.)
                    let mut before = std::mem::replace(&mut take.brains[rig], behaviour::Brain::new(input, &input.ragdolls[rig]));
                    take.world.events.append(&mut before.events);
                    take.turns[rig] = Some(held);
                }
            }
            steps += 1;
        }
        if !herd.is_empty() {
            // One that gets up is its motors' and the clip's, not the
            // pose it fell in; once up it is an agent again.
            for (rig, r) in input.ragdolls.iter().enumerate() {
                if r.hold.is_some() && take.brains[rig].rising() {
                    take.turns[rig] = None;
                }
            }
            let brains = &take.brains;
            let back = herd.rises(input, &mut take.world, &|rig| brains[rig].risen, k, input.start + (k + 1) as f64 / input.rate);
            for rig in back {
                let mut before = std::mem::replace(&mut take.brains[rig], behaviour::Brain::new(input, &input.ragdolls[rig]));
                take.world.events.append(&mut before.events);
                take.turns[rig] = None;
            }
        }
    }
    record(&take.world, &mut tracks);
    herd.plan(input, &take.world);
    let mut events = std::mem::take(&mut take.world.events);
    events.append(&mut herd.events);
    // A crowd's agents, and the bones of those knocked down from when
    // they were (the others' stay their agents').
    let (agents, let_go, herds, fell) = herd.finish(input);
    for (body, at) in let_go {
        if let Some(track) = dynamic.iter().position(|&i| i == body) {
            tracks[track].from = at;
        }
    }
    // A fall on a ragdoll that fell before has tracks of its own (the
    // ragdoll's, from that fall); and after a fall's end (it rose, or
    // its ragdoll was given on) its bones stay as they were then.
    let mut later: Vec<Track> = Vec::new();
    let stay = |samples: &mut [[f32; 7]], end: u32| {
        // (Sample `end` is already the next fall's, where there is one.)
        if let (Some(&at), true) = (samples.get((end as usize).wrapping_sub(1)), (end as usize) < samples.len()) {
            samples[end as usize..].iter_mut().for_each(|s| *s = at);
        }
    };
    for f in fell.iter().filter(|f| f.fall >= input.crowds[f.crowd].pool.len()) {
        for bone in &input.ragdolls[f.ragdoll].bones {
            let Some(track) = dynamic.iter().position(|&i| i == bone.body) else { continue };
            let name = tracks[track].id.split_once('/').map_or("", |x| x.1);
            let mut own = Track { id: format!("{}#{}/{name}", input.crowds[f.crowd].id, f.fall), from: f.from, rest: tracks[track].rest, samples: tracks[track].samples.clone(), clock: Vec::new() };
            stay(&mut own.samples, f.end);
            later.push(own);
        }
    }
    for f in fell.iter().filter(|f| f.fall < input.crowds[f.crowd].pool.len()) {
        for bone in &input.ragdolls[f.ragdoll].bones {
            if let Some(track) = dynamic.iter().position(|&i| i == bone.body) {
                stay(&mut tracks[track].samples, f.end);
            }
        }
    }
    // A particle's track begins with its first life. The sample before a
    // later one is where it came from (drawing between the two is along
    // its way in, not from where the last one died).
    for (track, &i) in tracks.iter_mut().zip(&dynamic) {
        let lives = &input.bodies[i].lives;
        if let Some(first) = lives.first() {
            track.from = first.from;
        }
        for (n, life) in lives.iter().enumerate() {
            if life.from == 0 || n > 0 && lives[n - 1].to >= life.from {
                continue;
            }
            let (p, q) = (life.pose, life.next);
            let back = lahza_motion_core::sim::quat_mul(lahza_motion_core::sim::quat_mul(p.q, [-q.q[0], -q.q[1], -q.q[2], q.q[3]]), p.q);
            track.samples[life.from as usize - 1] = [(2.0 * p.p[0] - q.p[0]) as f32, (2.0 * p.p[1] - q.p[1]) as f32, (2.0 * p.p[2] - q.p[2]) as f32, back[0] as f32, back[1] as f32, back[2] as f32, back[3] as f32];
        }
    }
    // (Bones never let go have nothing to say: their agents place them.)
    tracks.retain(|t| t.from != u32::MAX);
    tracks.extend(agents);
    tracks.extend(later);
    for b in &mut take.brains {
        events.append(&mut b.events);
    }
    events.sort_by(|a, b| a.t.total_cmp(&b.t).then_with(|| a.a.cmp(&b.a)).then_with(|| a.b.cmp(&b.b)));
    let mut stats = Stats { bodies: input.bodies.len(), joints: input.joints.len(), samples: input.samples, steps, seconds: began.elapsed().as_secs_f64(), ..Default::default() };
    for mind in minds.iter().flatten() {
        match mind {
            Mind::Policy(p) => (stats.policy_runs, stats.policy_seconds) = (stats.policy_runs + p.runs, stats.policy_seconds + p.seconds),
            Mind::Search(s) => (stats.tried_steps, stats.search_seconds, stats.search_threads) = (stats.tried_steps + s.tried_steps, stats.search_seconds + s.seconds, s.threads),
        }
    }
    Ok((Bake { hash: input.hash(), start: input.start, rate: input.rate, tracks, events, herds }, stats))
}

/// Bakes scene `scene` of a compiled project. `dir` is the project's
/// folder (collider files are hashed from it). Problems in the scene's
/// simulation (a joint to no body, a layer with no shape) are the error.
pub fn bake_scene(compiled: &Compiled, scene: usize, dir: Option<&std::path::Path>, meshes: Meshes) -> Result<(Bake, Stats), String> {
    bake_scene_watched(compiled, scene, dir, meshes, &|_| true)
}

/// `bake_scene`, watched (see `Watch`).
pub fn bake_scene_watched(compiled: &Compiled, scene: usize, dir: Option<&std::path::Path>, meshes: Meshes, watch: Watch) -> Result<(Bake, Stats), String> {
    let (input, errors) = compiled.sim_input(scene, dir);
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let files = |src: &str| match dir {
        Some(dir) => std::fs::read(dir.join(src)).map_err(|e| format!("{src}: {e}")),
        None => Err(format!("{src}: no project folder to read it from")),
    };
    bake_watched(&input, meshes, &files, watch)
}

#[cfg(test)]
mod behaviour_tests;
#[cfg(test)]
mod controller_tests;
#[cfg(test)]
mod crowd_tests;
#[cfg(test)]
mod herd_tests;
#[cfg(test)]
mod ragdoll_tests;
#[cfg(test)]
mod tests;
