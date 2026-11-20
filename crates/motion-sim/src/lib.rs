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
    /// Steps a search took in its tries ahead, and the seconds they took.
    pub tried_steps: u64,
    pub search_seconds: f64,
}

/// A take in progress: the world, and each ragdoll's brain and the turns
/// its controller has it make on top of its clip.
pub struct Take {
    pub world: World,
    pub brains: Vec<behaviour::Brain>,
    pub turns: Vec<Option<Vec<Option<[f64; 4]>>>>,
}

impl Take {
    pub fn new(input: &SimInput, meshes: Meshes) -> Result<Take, String> {
        Ok(Take { world: World::new(input, meshes)?, brains: input.ragdolls.iter().map(|r| behaviour::Brain::new(input, r)).collect(), turns: vec![None; input.ragdolls.len()] })
    }

    /// A copy that goes its own way from here (a try ahead).
    pub fn fork(&self) -> Take {
        Take { world: self.world.fork(), brains: self.brains.clone(), turns: self.turns.clone() }
    }

    /// Lets go the bodies whose release is sample `k`, where their keys
    /// have them.
    pub fn release_due(&mut self, input: &SimInput, k: u32) {
        for (i, b) in input.bodies.iter().enumerate() {
            if b.kind == BodyKind::Dynamic && !self.world.released[i] && b.release == k {
                self.world.release(input, i, b);
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
        for (rig, r) in input.ragdolls.iter().enumerate() {
            let drive = self.brains[rig].think(&mut self.world, input, r, x, t, self.turns[rig].as_deref());
            self.world.drive_ragdoll(r, rig, x, Some(&drive.targets), Some(&drive.scale));
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
            Some(Controller::Search { samples, horizon, every, spread }) => Some(Mind::Search(search::Search::new(*samples, *horizon, *every, *spread))),
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
        take.release_due(input, k);
        record(&take.world, &mut tracks);
        herd.plan(input, &take.world);
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
                Some(Mind::Search(s)) => take.turns[rig] = Some(s.decide(&take, input, rig, k, t)),
                None => {}
            }
        }
        for s in 0..n {
            if !herd.is_empty() {
                herd.drive(&mut take.world, (s + 1) as f64 / n as f64);
            }
            take.step(input, k, s);
            if !herd.is_empty() {
                herd.knocks(input, &mut take.world, k, input.start + (k as f64 + (s + 1) as f64 / n as f64) / input.rate);
            }
            steps += 1;
        }
    }
    record(&take.world, &mut tracks);
    herd.plan(input, &take.world);
    let mut events = std::mem::take(&mut take.world.events);
    events.append(&mut herd.events);
    // A crowd's agents, and the bones of those knocked down from when
    // they were (the others' stay their agents').
    let (agents, let_go, herds) = herd.finish(input);
    for (body, at) in let_go {
        if let Some(track) = dynamic.iter().position(|&i| i == body) {
            tracks[track].from = at;
        }
    }
    // (Bones never let go have nothing to say: their agents place them.)
    tracks.retain(|t| t.from != u32::MAX);
    tracks.extend(agents);
    for b in &mut take.brains {
        events.append(&mut b.events);
    }
    events.sort_by(|a, b| a.t.total_cmp(&b.t).then_with(|| a.a.cmp(&b.a)).then_with(|| a.b.cmp(&b.b)));
    let mut stats = Stats { bodies: input.bodies.len(), joints: input.joints.len(), samples: input.samples, steps, seconds: began.elapsed().as_secs_f64(), ..Default::default() };
    for mind in minds.iter().flatten() {
        match mind {
            Mind::Policy(p) => (stats.policy_runs, stats.policy_seconds) = (stats.policy_runs + p.runs, stats.policy_seconds + p.seconds),
            Mind::Search(s) => (stats.tried_steps, stats.search_seconds) = (stats.tried_steps + s.tried_steps, stats.search_seconds + s.seconds),
        }
    }
    Ok((Bake { hash: input.hash(), start: input.start, rate: input.rate, tracks, events, herds }, stats))
}

/// Bakes scene `scene` of a compiled project. `dir` is the project's
/// folder (collider files are hashed from it). Problems in the scene's
/// simulation (a joint to no body, a layer with no shape) are the error.
pub fn bake_scene(compiled: &Compiled, scene: usize, dir: Option<&std::path::Path>, meshes: Meshes) -> Result<(Bake, Stats), String> {
    let (input, errors) = compiled.sim_input(scene, dir);
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    bake_with(&input, meshes, &|src| match dir {
        Some(dir) => std::fs::read(dir.join(src)).map_err(|e| format!("{src}: {e}")),
        None => Err(format!("{src}: no project folder to read it from")),
    })
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
