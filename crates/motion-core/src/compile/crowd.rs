//! Crowds in the compiled project: each agent's start, goal and, for
//! those that may fall, its bones' places through a cycle of its clip.

use super::ragdoll::tree;
use super::sim::visit;
use super::*;
use crate::sim::{decompose, AgentInput, CrowdInput, Pose, SimInput};

impl Compiled {
    pub(super) fn crowd_input(&self, i: usize, input: &mut SimInput, errors: &mut Vec<String>) {
        let scene = &self.scenes[i];
        let sim = &scene.sim;
        if sim.settings.crowds.is_empty() {
            return;
        }
        let rate = input.rate;
        let mut flat = Vec::new();
        tree(&scene.layers, None, &mut flat);
        let worlds_at = |t: f64, ids: &HashSet<&str>| -> HashMap<String, Mat4> {
            let tc = t + scene.pre_roll;
            let space = Space { keyed: true, ..self.camera(scene, tc) };
            let mut out = HashMap::new();
            visit(&scene.layers, tc, None, &space, None, &mut |l, world, _, _| {
                if ids.contains(&*l.id) {
                    out.insert(l.id.to_string(), world);
                }
                None
            });
            out
        };
        for (n, (c, clip)) in sim.settings.crowds.iter().zip(&sim.crowd_clips).enumerate() {
            let what = format!("{}.simulation.crowds.{n}", scene.id);
            if c.goals.is_empty() {
                errors.push(format!("{what}.goals: a crowd needs somewhere to go ([[x, z]])"));
                continue;
            }
            if !(c.radius > 0.0 && c.stride > 0.0 && c.cycle > 0.0) {
                errors.push(format!("{what}: radius, stride and cycle must be positive"));
                continue;
            }
            let mut wanted: HashSet<&str> = HashSet::new();
            for a in &c.agents {
                match flat.iter().find(|(l, _)| &*l.id == a.as_str()) {
                    Some((l, _)) if matches!(l.kind, CKind::Comp(_)) => {
                        wanted.insert(a.as_str());
                    }
                    Some(_) => errors.push(format!("{what}.agents: '{a}' is no comp (an agent is a comp holding a character)")),
                    None => errors.push(format!("{what}.agents: no layer '{a}' in the scene")),
                }
            }
            if wanted.len() != c.agents.len() {
                continue;
            }
            let clip_at = clip.unwrap_or(sim.start);
            let start = worlds_at(sim.start, &wanted);
            // The bones of the agents that may fall, through one cycle.
            let cycle = (c.cycle * rate).round().max(1.0) as usize;
            let bones_of = |a: &str| input.ragdolls.iter().position(|r| r.id == format!("{}/{a}", c.id));
            let mut with_bones: HashSet<&str> = wanted.clone();
            for a in &c.agents {
                if let Some(r) = bones_of(a) {
                    with_bones.extend(input.ragdolls[r].bones.iter().map(|b| input.bodies[b.body].id.as_str()));
                }
            }
            let frames: Vec<HashMap<String, Mat4>> = if c.ragdoll.is_some() { (0..=cycle).map(|j| worlds_at(clip_at + j as f64 / rate, &with_bones)).collect() } else { Vec::new() };
            let mut agents = Vec::with_capacity(c.agents.len());
            for (k, a) in c.agents.iter().enumerate() {
                let (pose, rest) = decompose(start[a.as_str()]);
                let ragdoll = bones_of(a);
                let local = match ragdoll {
                    Some(r) => input.ragdolls[r]
                        .bones
                        .iter()
                        .map(|b| {
                            let id = &input.bodies[b.body].id;
                            frames.iter().map(|f| decompose(f[a.as_str()]).0.inverse().then(&decompose(f[id.as_str()]).0)).collect::<Vec<Pose>>()
                        })
                        .collect(),
                    None => Vec::new(),
                };
                agents.push(AgentInput { id: a.clone(), start: pose, rest, goal: c.goals[k % c.goals.len()], ragdoll, local });
            }
            input.crowds.push(CrowdInput { id: c.id.clone(), agents, speed: c.speed, vary: c.vary.clamp(0.0, 0.9), radius: c.radius, height: c.height.unwrap_or(c.radius * 6.0), stride: c.stride, cycle: c.cycle, clip_at, facing: c.facing, knock: c.knock });
        }
    }
}
