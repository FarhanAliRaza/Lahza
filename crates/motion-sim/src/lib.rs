//! Simulation for the motion engine: a scene's bodies, joints and pushes
//! (`lahza_motion_core::sim::SimInput`) stepped with rapier at a fixed
//! rate and kept per frame as a `Bake`, which drawing reads.
//!
//! The same input bakes to the same bytes: rapier is built with
//! `enhanced-determinism` (no SIMD, no threads, libm's maths), the step is
//! fixed, and bodies, colliders and joints are made in the input's order.

pub mod behaviour;
pub mod ragdoll;
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
}

/// Steps `input` from its start and returns every dynamic body's pose per
/// sample.
pub fn bake(input: &SimInput, meshes: Meshes) -> Result<(Bake, Stats), String> {
    let began = std::time::Instant::now();
    let mut world = World::new(input, meshes)?;
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
    let mut brains: Vec<behaviour::Brain> = input.ragdolls.iter().map(|r| behaviour::Brain::new(input, r)).collect();
    let n = input.substeps as u64;
    let mut steps = 0u64;
    for k in 0..input.samples {
        // Bodies whose release is this sample are let go where their keys
        // have them.
        for &i in &dynamic {
            let b = &input.bodies[i];
            if !world.released[i] && b.release == k {
                world.release(input, i, b);
            }
        }
        record(&world, &mut tracks);
        for s in 0..n {
            let t = input.start + (k as f64 + s as f64 / n as f64) / input.rate;
            let x = k as f64 + (s + 1) as f64 / n as f64;
            world.drive(input, x);
            world.push(input, t);
            for (rig, r) in input.ragdolls.iter().enumerate() {
                let drive = brains[rig].think(&mut world, input, r, x, t);
                world.drive_ragdoll(r, rig, x, Some(&drive.targets), Some(&drive.scale));
                world.hit_ragdoll(r, t);
            }
            world.step(input, t);
            steps += 1;
        }
    }
    record(&world, &mut tracks);
    let mut events = std::mem::take(&mut world.events);
    for b in &mut brains {
        events.append(&mut b.events);
    }
    events.sort_by(|a, b| a.t.total_cmp(&b.t).then_with(|| a.a.cmp(&b.a)).then_with(|| a.b.cmp(&b.b)));
    let stats = Stats { bodies: input.bodies.len(), joints: input.joints.len(), samples: input.samples, steps, seconds: began.elapsed().as_secs_f64() };
    Ok((Bake { hash: input.hash(), start: input.start, rate: input.rate, tracks, events }, stats))
}

/// Bakes scene `scene` of a compiled project. `dir` is the project's
/// folder (collider files are hashed from it). Problems in the scene's
/// simulation (a joint to no body, a layer with no shape) are the error.
pub fn bake_scene(compiled: &Compiled, scene: usize, dir: Option<&std::path::Path>, meshes: Meshes) -> Result<(Bake, Stats), String> {
    let (input, errors) = compiled.sim_input(scene, dir);
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    bake(&input, meshes)
}

#[cfg(test)]
mod behaviour_tests;
#[cfg(test)]
mod ragdoll_tests;
#[cfg(test)]
mod tests;
