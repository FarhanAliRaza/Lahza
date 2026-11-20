//! `sim_bake`: steps a scene's simulation and writes its bake.

use super::*;
use lahza_motion_core::compile::SimStatus;
use lahza_motion_core::sim::SimEvent;
use lahza_motion_core::mesh::{Mesh, MeshParams, MeshShape};

pub(super) fn def() -> ToolDef {
    ToolDef {
        name: "sim_bake",
        description: "Bake a scene's simulation: its bodies (layers with \"body\"), joints, pushes and blasts (the scene's \"simulation\") are stepped at a fixed rate from the simulation's start, and every body's place is kept per frame in sim/<scene>.bake. Drawing reads the bake, so snapshot, review and render show the result and any frame can be asked for in any order. The same project bakes to the same bytes. Bake again after changing a body, its shape or keys, a joint, a push or the settings: `check` reports a scene whose bake is missing or stale (and until then the old motion is what is drawn). Returns per scene what was stepped, how long it took, and the take's events (a ragdoll's fall and steps; first touches between bodies with their speed in px/s) with a line per ragdoll (fell at, steps). `seed` sets the scene's simulation.seed first: another seed is another take.",
        schema: obj(json!({"scene": {"type": "string", "description": "the scene to bake (default: every scene that has bodies)"},
            "seed": {"type": "integer", "description": "set the scene's simulation.seed, then bake (another take); needs \"scene\" when several scenes have bodies"},
            "force": {"type": "boolean", "description": "bake even when the bake is up to date (default false)"}}), &[]),
        read_only: false,
    }
}

fn scene_ragdolls(project: &Project, scene: &str) -> Vec<String> {
    project.scenes.iter().find(|s| s.id == scene).and_then(|s| s.simulation.as_ref()).map(|s| s.ragdolls.iter().map(|r| r.id.clone()).collect()).unwrap_or_default()
}

impl Toolbox {
    /// What is drawn wrong for want of a bake: a line per scene whose
    /// simulation is not baked, stale, or cannot be.
    pub(super) fn sim_warnings(&mut self, dir: &Path) -> Vec<String> {
        let compiled = self.compiled(dir);
        (0..compiled.scenes.len())
            .filter_map(|i| {
                let id = &compiled.scenes[i].id;
                match compiled.sim_status(i, Some(dir)) {
                    SimStatus::None | SimStatus::Fresh { .. } => None,
                    SimStatus::Unbaked { bodies } => Some(format!("scene '{id}': its simulation ({bodies} bodies) is not baked: they are drawn where their keys put them (sim_bake)")),
                    SimStatus::Stale { .. } => Some(format!("scene '{id}': its simulation's bake is stale: what is drawn is the simulation as it was before the last changes (sim_bake)")),
                    SimStatus::Broken(errors) => Some(format!("scene '{id}': its simulation cannot be baked: {}", errors.join("; "))),
                }
            })
            .collect()
    }

    pub(super) fn sim_bake(&mut self, args: &Value) -> ToolResult {
        let dir = self.dir(args)?;
        let only = get_str(args, "scene").map(str::to_string);
        if let Some(seed) = args.get("seed").and_then(Value::as_u64) {
            let compiled = self.compiled(&dir);
            let with: Vec<String> = (0..compiled.scenes.len()).filter(|&i| compiled.has_simulation(i)).map(|i| compiled.scenes[i].id.to_string()).collect();
            let scene = match (&only, with.as_slice()) {
                (Some(s), _) => s.clone(),
                (None, [one]) => one.clone(),
                (None, _) => return Err(format!("\"seed\" needs \"scene\": {} scenes have bodies", with.len())),
            };
            let project = &self.state(&dir).project;
            let current = project.scenes.iter().find(|s| s.id == scene).ok_or_else(|| format!("no scene '{scene}'"))?.simulation.clone().unwrap_or_default();
            if current.seed != seed {
                let mut sim = serde_json::to_value(&current).map_err(|e| e.to_string())?;
                sim["seed"] = json!(seed);
                self.call("edit_apply", json!({"project": dir.display().to_string(), "ops": [{"op": "set", "target": format!("{scene}.simulation"), "value": sim}]}))?;
            }
        }
        let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
        let compiled = self.compiled(&dir);
        if let Some(s) = &only {
            if !compiled.scenes.iter().any(|c| &*c.id == s.as_str()) {
                return Err(format!("no scene '{s}'"));
            }
        }
        let mut files = lahza_motion_render::model::Files::default();
        let mut meshes = |params: &MeshParams| -> Result<Mesh, String> {
            match &params.shape {
                MeshShape::Gltf { src, mesh, primitive, unit, .. } => lahza_motion_render::model::load_mesh(&mut files, Some(&dir), src, *mesh, *primitive, *unit),
                _ => lahza_motion_sim::core_meshes(params),
            }
        };
        let mut out = Vec::new();
        let mut baked = Vec::new();
        for (i, scene) in compiled.scenes.iter().enumerate() {
            let id = scene.id.to_string();
            if only.as_ref().is_some_and(|s| *s != id) {
                continue;
            }
            match compiled.sim_status(i, Some(&dir)) {
                SimStatus::None => {
                    if only.is_some() {
                        out.push(json!({"scene": id, "status": "nothing to simulate: no layer of the scene has a \"body\" that the simulation moves"}));
                    }
                }
                SimStatus::Broken(errors) => return Err(format!("scene '{id}': {}", errors.join("; "))),
                SimStatus::Fresh { bodies } if !force => out.push(json!({"scene": id, "status": "up to date", "bodies": bodies})),
                _ => {
                    let watch = self.bake_watch.clone();
                    let watched = |share: f64| watch.as_ref().is_none_or(|w| w(&id, share));
                    let (bake, stats) = lahza_motion_sim::bake_scene_watched(&compiled, i, Some(&dir), &mut meshes, &watched).map_err(|e| format!("scene '{id}': {e}"))?;
                    let bytes = bake.to_bytes().len();
                    lahza_motion_core::sim::save_bake(&dir, &id, &bake).map_err(|e| e.to_string())?;
                    // Falls and steps first: they are what a take is judged by.
                    let acted = |e: &&lahza_motion_core::sim::SimEvent| e.kind != "touch";
                    let shown: Vec<&lahza_motion_core::sim::SimEvent> = bake.events.iter().filter(acted).chain(bake.events.iter().filter(|e| !acted(e))).take(60).collect();
                    let ragdolls: Vec<Value> = scene_ragdolls(&self.state(&dir).project, &id)
                        .into_iter()
                        .map(|doll| {
                            let fell = bake.events.iter().find(|e| e.kind == "fall" && e.a == doll).map(|e| e.t);
                            // (A rise's `speed` is the percent of its weight the outside force carried.)
                            let rose = bake.events.iter().find(|e| e.kind == "rise" && e.a == doll);
                            json!({"id": doll, "fell": fell, "rose": rose.map(|e| e.t), "lifted_percent": rose.map(|e| e.speed), "steps": bake.events.iter().filter(|e| e.kind == "step").count()})
                        })
                        .collect();
                    out.push(json!({"scene": id, "status": "baked", "file": format!("sim/{id}.bake"), "bytes": bytes, "tracks": bake.tracks.len(), "agents": bake.herds.iter().map(|h| h.count as u64).sum::<u64>(),
                        "bodies": stats.bodies, "joints": stats.joints, "frames": stats.samples, "steps": stats.steps, "seconds": (stats.seconds * 1000.0).round() / 1000.0,
                        "policy_runs": stats.policy_runs, "tried_steps": stats.tried_steps, "search_seconds": (stats.search_seconds * 1000.0).round() / 1000.0, "search_threads": stats.search_threads,
                        "from": bake.start, "seed": compiled.scenes[i].sim.settings.seed, "ragdolls": ragdolls, "events": shown, "events_total": bake.events.len()}));
                    baked.push((id, Arc::new(bake)));
                }
            }
        }
        // Scenes that no longer simulate anything keep no bake.
        let open = self.state(&dir);
        let gone: Vec<String> = open.project.bakes.keys().filter(|id| !compiled.scenes.iter().enumerate().any(|(i, s)| &*s.id == id.as_str() && compiled.has_simulation(i))).cloned().collect();
        for id in gone {
            let _ = std::fs::remove_file(lahza_motion_core::sim::bake_path(&dir, &id));
            open.project.bakes.remove(&id);
            open.compiled = None;
        }
        if !baked.is_empty() {
            for (id, bake) in baked {
                open.project.bakes.insert(id, bake);
            }
            open.compiled = None;
            // What was drawn from the old bake is not this project's any more.
            open.renderer = None;
            open.stamp = stamp(&dir);
        }
        Ok(Output::json(json!({"scenes": out})))
    }
}

/// For each of `times` (timeline seconds, as a snapshot's frames): the
/// events of its scene's bake since the frame before it in that scene
/// (the earliest of a scene: since the scene began).
pub(super) fn events_by_frame(compiled: &Compiled, times: &[f64]) -> Vec<Vec<SimEvent>> {
    let scene_of = |t: f64| compiled.scenes.iter().position(|s| t >= s.start - 1e-9 && t < s.end + 1e-9);
    times
        .iter()
        .map(|&t| {
            let Some(scene) = scene_of(t) else { return Vec::new() };
            let Some(bake) = &compiled.scenes[scene].sim.bake else { return Vec::new() };
            // The latest other frame of the scene before this one.
            let since = times.iter().copied().filter(|&o| o < t - 1e-9 && scene_of(o) == Some(scene)).fold(f64::NEG_INFINITY, f64::max);
            bake.events.iter().filter(|e| e.t > since + 1e-9 && e.t <= t + 1e-9).cloned().collect()
        })
        .collect()
}

/// A frame's events in a line of at most `most` characters: what a figure
/// did (falls, steps, rises, dodges, knocks) by name, then the hardest
/// touches, then how many more.
pub(super) fn mark(events: &[SimEvent], most: usize) -> String {
    let short = |id: &str| id.rsplit(['-', '/']).next().unwrap_or(id).to_string();
    let mut parts: Vec<String> = Vec::new();
    let mut kinds: Vec<&str> = Vec::new();
    for e in events.iter().filter(|e| e.kind != "touch") {
        if !kinds.contains(&e.kind.as_str()) {
            kinds.push(&e.kind);
        }
    }
    for kind in kinds {
        let of: Vec<&SimEvent> = events.iter().filter(|e| e.kind == kind).collect();
        parts.push(if of.len() == 1 { format!("{kind} {:.2}", of[0].t) } else { format!("{kind} ×{}", of.len()) });
    }
    let mut touches: Vec<&SimEvent> = events.iter().filter(|e| e.kind == "touch").collect();
    touches.sort_by(|a, b| b.speed.total_cmp(&a.speed).then(a.t.total_cmp(&b.t)));
    let mut line = parts.join(" · ");
    let mut shown = 0;
    for e in &touches {
        let next = format!("{}{}×{} {:.2}", if line.is_empty() { "" } else { " · " }, short(&e.a), short(&e.b), e.t);
        if shown >= 2 || line.chars().count() + next.chars().count() + 12 > most {
            break;
        }
        line += &next;
        shown += 1;
    }
    if touches.len() > shown {
        line += &format!("{}+{} touch{}", if line.is_empty() { "" } else { " · " }, touches.len() - shown, if touches.len() - shown == 1 { "" } else { "es" });
    }
    if line.chars().count() > most {
        line = line.chars().take(most.saturating_sub(1)).collect::<String>() + "…";
    }
    line
}
