//! Crowds in the compiled project: each agent's start, goal and, for
//! those that may fall, its bones' places through a cycle of its clip.

use super::ragdoll::tree;
use super::sim::visit;
use super::*;
use crate::sim::{decompose, AgentInput, Bake, CrowdInput, Pose, SimInput};
use std::sync::OnceLock;

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
            if !(c.radius > 0.0 && c.stride > 0.0 && c.cycle > 0.0) {
                errors.push(format!("{what}: radius, stride and cycle must be positive"));
                continue;
            }
            if let Some(character) = &c.character {
                self.herd_input(i, n, c, character, clip.unwrap_or(sim.start), &flat, &worlds_at, input, errors);
                continue;
            }
            if c.ragdoll.as_ref().is_some_and(|r| r.get_up.is_some()) {
                errors.push(format!("{what}.ragdoll.get_up: only a crowd of instances (\"character\") has its fallen get up: an agent that is a layer is its bones' own tracks from its fall on, and they are not given back"));
            }
            if c.guard.is_some() {
                errors.push(format!("{what}.guard: only a crowd of instances (\"character\") has a guard: its agents share the character's poses, which is what a second pose is blended with"));
            }
            if c.agents.is_empty() {
                errors.push(format!("{what}: a crowd needs \"agents\" (comps) or a \"character\" (one comp, with \"starts\" or \"count\" and \"area\")"));
                continue;
            }
            if c.goals.is_empty() {
                errors.push(format!("{what}.goals: a crowd needs somewhere to go ([[x, z]])"));
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
            input.crowds.push(CrowdInput { id: c.id.clone(), agents, instanced: false, pool: Vec::new(), local: Vec::new(), speed: c.speed, vary: c.vary.clamp(0.0, 0.9), radius: c.radius, height: c.height.unwrap_or(c.radius * 6.0), stride: c.stride, cycle: c.cycle, clip_at, facing: c.facing, knock: c.knock, plan: c.plan, dodge: c.dodge.max(0.0), guard: None, guard_pose: Vec::new() });
        }
    }

    /// A crowd of instances of `character`.
    #[allow(clippy::too_many_arguments)]
    fn herd_input(&self, i: usize, n: usize, c: &crate::sim::Crowd, character: &str, clip_at: f64, flat: &[(&CLayer, Option<usize>)], worlds_at: &dyn Fn(f64, &HashSet<&str>) -> HashMap<String, Mat4>, input: &mut SimInput, errors: &mut Vec<String>) {
        let scene = &self.scenes[i];
        let what = format!("{}.simulation.crowds.{n}", scene.id);
        let before = errors.len();
        if !c.agents.is_empty() {
            errors.push(format!("{what}: \"agents\" (a comp each) or \"character\" (instances of one), not both"));
        }
        match scene.layers.iter().find(|l| &*l.id == character) {
            Some(l) if !matches!(l.kind, CKind::Comp(_)) => errors.push(format!("{what}.character: '{character}' is no comp (a character is a comp holding meshes and its skeleton)")),
            Some(l) => {
                if let Some(other) = plain(l.children()) {
                    errors.push(format!("{what}.character: '{other}' is neither a mesh nor a comp; instances are drawn from meshes"));
                }
            }
            None if flat.iter().any(|(l, _)| &*l.id == character) => errors.push(format!("{what}.character: '{character}' is inside another layer; a character stands in the scene itself")),
            None => errors.push(format!("{what}.character: no layer '{character}' in the scene")),
        }
        // Where they start.
        let mut starts = c.starts.clone();
        match (starts.is_empty(), c.count, c.area) {
            (true, Some(count), Some([x0, z0, x1, z1])) if count > 0 => {
                let (w, d) = ((x1 - x0).abs().max(1e-9), (z1 - z0).abs().max(1e-9));
                let cols = ((count as f64 * w / d).sqrt().ceil() as u32).clamp(1, count);
                let rows = count.div_ceil(cols);
                let (cw, cd) = (w / cols as f64, d / rows as f64);
                if cw.min(cd) < 2.0 * c.radius {
                    errors.push(format!("{what}: {count} agents of radius {} do not fit in an area of {w:.0} by {d:.0} px ({} at most)", c.radius, ((w / (2.0 * c.radius)).floor() * (d / (2.0 * c.radius)).floor()) as u64));
                }
                let mut state = input.seed ^ (n as u64).wrapping_mul(0x9e3779b97f4a7c15) ^ 0x6865_7264;
                let mut rand = move || {
                    // splitmix64, to −1..1.
                    state = state.wrapping_add(0x9e3779b97f4a7c15);
                    let mut z = state;
                    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
                    ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 52) as f64 - 1.0
                };
                for k in 0..count {
                    let (col, row) = (k % cols, k / cols);
                    let (jx, jz) = (rand() * (cw / 2.0 - c.radius).max(0.0) * 0.8, rand() * (cd / 2.0 - c.radius).max(0.0) * 0.8);
                    starts.push([x0.min(x1) + (col as f64 + 0.5) * cw + jx, z0.min(z1) + (row as f64 + 0.5) * cd + jz]);
                }
            }
            (true, _, _) => errors.push(format!("{what}: instances need \"starts\" ([[x, z]]) or a \"count\" and an \"area\" ([x0, z0, x1, z1])")),
            (false, None, None) => {}
            (false, _, _) => errors.push(format!("{what}: \"starts\", or \"count\" and \"area\", not both")),
        }
        // (The guard's pose is the character at a time of the scene:
        // outside it the character is not there to be posed.)
        if let Some(at) = scene.sim.crowd_guards.get(n).copied().flatten() {
            if at < scene.start - 1e-9 || at >= scene.end - 1e-9 {
                errors.push(format!("{what}.guard.pose: {at} s is outside the scene ({} to {} s): key the pose at a moment of the scene the walk's cycle does not use", scene.start, scene.end));
            }
        }
        if c.goals.is_empty() == c.march.is_none() {
            errors.push(format!("{what}: instances go to \"goals\" ([[x, z]]) or \"march\" ([dx, dz] from where each starts), one of them"));
        }
        if errors.len() > before {
            return;
        }
        let pool: Vec<usize> = (0..input.ragdolls.len()).filter(|&r| input.ragdolls[r].id.starts_with(&format!("{}#", c.id))).collect();
        // The bones' layers, by the pool's first ragdoll.
        let bones: Vec<String> = pool.first().map(|&r| input.ragdolls[r].bones.iter().map(|b| input.bodies[b.body].id.split_once('/').map_or("", |x| x.1).to_string()).collect()).unwrap_or_default();
        let mut wanted: HashSet<&str> = bones.iter().map(String::as_str).collect();
        wanted.insert(character);
        let (pose, rest) = decompose(worlds_at(scene.sim.start, &wanted)[character]);
        let cycle = (c.cycle * input.rate).round().max(1.0) as usize;
        let mut local: Vec<Vec<Pose>> = vec![Vec::with_capacity(cycle + 1); bones.len()];
        if !bones.is_empty() {
            for j in 0..=cycle {
                let f = worlds_at(clip_at + j as f64 / input.rate, &wanted);
                let root = decompose(f[character]).0.inverse();
                for (b, id) in bones.iter().enumerate() {
                    local[b].push(root.then(&decompose(f[id.as_str()]).0));
                }
            }
        }
        // The guard's pose, bone by bone, for one knocked down in it.
        let mut guard_pose: Vec<(Pose, bool)> = Vec::new();
        if let (Some(at), Some(g), false) = (scene.sim.crowd_guards.get(n).copied().flatten(), &c.guard, bones.is_empty()) {
            let f = worlds_at(at, &wanted);
            let root = decompose(f[character]).0.inverse();
            let parents = &input.ragdolls[pool[0]].bones;
            for (b, id) in bones.iter().enumerate() {
                // (It takes a bone it names, and all that hang from one.)
                let mut cur = Some(b);
                let mut takes = g.bones.is_empty();
                while let (Some(x), false) = (cur, takes) {
                    takes = g.bones.iter().any(|n| n == &bones[x]);
                    cur = parents[x].parent;
                }
                guard_pose.push((root.then(&decompose(f[id.as_str()]).0), takes));
            }
        }
        let agents = starts
            .iter()
            .enumerate()
            .map(|(k, s)| AgentInput {
                id: String::new(),
                start: Pose { p: [s[0], pose.p[1], s[1]], q: pose.q },
                rest,
                goal: match c.march {
                    Some(m) => [s[0] + m[0], s[1] + m[1]],
                    None => c.goals[k % c.goals.len()],
                },
                ragdoll: None,
                local: Vec::new(),
            })
            .collect();
        input.crowds.push(CrowdInput { id: c.id.clone(), agents, instanced: true, pool, local, speed: c.speed, vary: c.vary.clamp(0.0, 0.9), radius: c.radius, height: c.height.unwrap_or(c.radius * 6.0), stride: c.stride, cycle: c.cycle, clip_at, facing: c.facing, knock: c.knock, plan: c.plan, dodge: c.dodge.max(0.0), guard: c.guard.as_ref().map(|g| g.ahead.max(0.0)), guard_pose });
    }
}

/// The first layer under `layers` that is neither a mesh nor a comp.
fn plain(layers: &[CLayer]) -> Option<&str> {
    layers.iter().find_map(|l| match &l.kind {
        CKind::Mesh(_) => None,
        CKind::Comp(children) => plain(children),
        _ => Some(&*l.id),
    })
}

// ------------------------------------------------------------- drawing

/// A crowd's character whose instances the bake places: drawn once per
/// agent, each in the pose its clip has at the agent's own time.
pub(super) struct CHerd {
    bake: Arc<Bake>,
    herd: usize,
    /// Content time less this is the bake's.
    shift: f64,
    clip_at: f64,
    cycle: f64,
    full: u32,
    near: Option<f64>,
    /// Per ragdoll of the pool: the bake's track of each bone's layer.
    slots: Vec<HashMap<String, usize>>,
    palette: OnceLock<Palette>,
    /// The guard: the content time of its pose, and the bones it takes
    /// (None: all).
    guard: Option<(f64, Option<Vec<String>>)>,
    /// A walk's moment with the guard's pose over it, by how far (in
    /// quarters): made when first drawn, shared by every agent in it.
    guarded: std::sync::Mutex<HashMap<(usize, u8), Arc<Vec<LayerState>>>>,
}

/// The character through one cycle of its clip: its meshes (placed in the
/// agent's own space, skins posed) at each of `phases` moments, and a
/// sphere that holds it (its centre in that space, its radius).
struct Palette {
    phases: Vec<Vec<LayerState>>,
    /// The character in its guard's pose, and per skinned mesh of it
    /// which joints the pose takes.
    guard: Option<(Vec<LayerState>, Vec<Vec<bool>>)>,
    centre: [f64; 3],
    /// Half its box, and half the box's diagonal.
    half: [f64; 3],
    radius: f64,
    /// A hash of what it is made of.
    key: u64,
}

/// Headings and view directions a picture of an agent stands for, to a
/// turn; and px across the character's box, corner to corner.
const HEADINGS: f64 = 16.0;
const CELL: u32 = 160;
/// The pictures' camera stands this many of the character's radii away.
const FAR: f64 = 40.0;

thread_local! {
    /// Pictures the layers emitted so far name, not yet lit.
    static PENDING: std::cell::RefCell<Vec<Picture>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The pictures named by what was emitted since the last call, lit by
/// `lighting`.
pub(super) fn take_pictures(lighting: &dyn Fn() -> Arc<meshes::Lighting>) -> Vec<Arc<Picture>> {
    let mut pictures = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if pictures.is_empty() {
        return Vec::new();
    }
    // Its lights are part of what a picture is, to a step: a picture is
    // lit by the scene's lights as they are to the nearest step of
    // `stepped`, so a light that moves or dims a little is the same
    // picture, and one that is animated makes a new one a step, not a
    // frame.
    let lighting = Arc::new(stepped(&lighting()));
    let mut lit = crate::sim::Fnv::default();
    lit.str(&format!("{lighting:?}"));
    let lit = lit.0;
    pictures
        .drain(..)
        .map(|mut p| {
            meshes::light_meshes(&mut p.layers, &lighting);
            p.key ^= lit;
            Arc::new(p)
        })
        .collect()
}

/// `lighting` to the nearest step: a direction to a sixteenth in each
/// part (under four degrees), a light's colour, the sky's and the
/// exposure to a sixteenth of a stop (four in a hundred), a light's
/// place and size to a sixteenth of how far off it is.
fn stepped(lighting: &meshes::Lighting) -> meshes::Lighting {
    let level = |v: f64| if v > 1e-9 { crate::dm::exp2((crate::dm::log2(v) * 16.0).round() / 16.0) } else { 0.0 };
    let colour = |c: [f64; 3]| c.map(level);
    let way = |d: [f64; 3]| {
        let q = d.map(|v| (v * 16.0).round() / 16.0);
        let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
        if l > 1e-9 { q.map(|v| v / l) } else { d }
    };
    let place = |p: [f64; 3], by: f64| p.map(|v| (v / by).round() * by);
    let mut out = lighting.clone();
    for l in &mut out.lights {
        l.color = colour(l.color);
        match l.kind {
            crate::document::LightKind::Directional => l.vector = way(l.vector),
            _ => {
                let by = ((l.vector[0] * l.vector[0] + l.vector[1] * l.vector[1] + l.vector[2] * l.vector[2]).sqrt() / 16.0).max(1.0);
                let by = crate::dm::exp2(crate::dm::log2(by).ceil());
                (l.vector, l.u, l.v, l.range) = (place(l.vector, by), place(l.u, by / 4.0), place(l.v, by / 4.0), (l.range / by).round() * by);
            }
        }
        l.softness = (l.softness * 16.0).round() / 16.0;
    }
    (out.env.sky, out.env.horizon, out.env.ground) = (colour(out.env.sky), colour(out.env.horizon), colour(out.env.ground));
    for lobe in &mut out.env.lobes {
        (lobe.dir, lobe.color, lobe.size) = (way(lobe.dir), colour(lobe.color), (lobe.size * 64.0).round() / 64.0);
    }
    out.env.image_intensity = level(out.env.image_intensity);
    out.env.image_rotation = (out.env.image_rotation * 32.0).round() / 32.0;
    out.exposure = level(out.exposure);
    out
}

/// Binds each crowd of instances the bake has to its character's layer.
pub(super) fn bind_herds(layers: &mut [CLayer], sim: &super::sim::CSim, shift: f64) {
    let Some(bake) = &sim.bake else { return };
    for ((c, clip), guard) in sim.settings.crowds.iter().zip(&sim.crowd_clips).zip(&sim.crowd_guards) {
        let (Some(character), Some(herd)) = (&c.character, bake.herds.iter().position(|h| h.id == c.id)) else { continue };
        let Some(l) = layers.iter_mut().find(|l| &*l.id == character.as_str()) else { continue };
        let prefix = format!("{}#", c.id);
        let mut slots: Vec<HashMap<String, usize>> = Vec::new();
        for (k, t) in bake.tracks.iter().enumerate() {
            let Some((slot, bone)) = t.id.strip_prefix(&prefix).and_then(|r| r.split_once('/')) else { continue };
            let Ok(slot) = slot.parse::<usize>() else { continue };
            if slots.len() <= slot {
                slots.resize_with(slot + 1, HashMap::new);
            }
            slots[slot].insert(bone.to_string(), k);
        }
        l.herd = Some(Box::new(CHerd { bake: bake.clone(), herd, shift, clip_at: clip.unwrap_or(sim.start), cycle: c.cycle.max(1e-6), full: c.full, near: c.near, slots, palette: OnceLock::new(), guard: guard.map(|t| (t + shift, c.guard.as_ref().filter(|g| !g.bones.is_empty()).map(|g| g.bones.clone()))), guarded: Default::default() }));
    }
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-300);
    v.map(|x| x / l)
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// World → a camera that looks along `d` (unit) with the world's down
/// down, as a turn: its rows are the camera's right, down and forward.
fn look(d: [f64; 3]) -> Mat4 {
    let side = cross([0.0, 1.0, 0.0], d);
    let x = if side.iter().map(|v| v * v).sum::<f64>() < 1e-12 { [1.0, 0.0, 0.0] } else { unit(side) };
    let y = cross(d, x);
    Mat4([x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, d[0], d[1], d[2], 0.0, 0.0, 0.0, 0.0, 1.0])
}

fn transpose3(m: Mat4) -> Mat4 {
    let a = m.0;
    Mat4([a[0], a[4], a[8], 0.0, a[1], a[5], a[9], 0.0, a[2], a[6], a[10], 0.0, 0.0, 0.0, 0.0, 1.0])
}

impl CHerd {
    fn build(&self, l: &CLayer, step: Option<Step>, space: &Space) -> Palette {
        let count = ((self.cycle * self.bake.rate * 2.0).round() as usize).clamp(16, 128);
        let inner = Space { world: Mat4::IDENTITY, three_d: space.three_d || l.three_d, dof: None, keyed: true, ..*space };
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        let mut skinned = false;
        let phases: Vec<Vec<LayerState>> = (0..count)
            .map(|j| {
                let tc = self.clip_at + self.shift + j as f64 / count as f64 * self.cycle - l.offset;
                let mut out = Vec::new();
                emit(l.children(), tc, step, &inner, 1.0, &mut out);
                out.retain(|s| matches!(s.content, Content::Mesh(_)));
                super::sim::pose_skins_in(l.children(), tc, step, &inner, &mut out);
                for s in &out {
                    let Content::Mesh(m) = &s.content else { continue };
                    let mut point = |x: f64, y: f64, z: f64| {
                        let p = m.world.apply(x, y, z);
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    };
                    // A skinned mesh is about its joints (padded below); another, its box.
                    match m.skin.as_ref().filter(|s| !s.joints.is_empty()) {
                        Some(skin) => {
                            skinned = true;
                            skin.joints.iter().for_each(|j| point(j.0[3], j.0[7], j.0[11]));
                        }
                        None => {
                            let Some((a, b)) = m.params.local_bounds() else { continue };
                            (0..8).for_each(|c| point(if c & 1 == 0 { a[0] } else { b[0] }, if c & 2 == 0 { a[1] } else { b[1] }, if c & 4 == 0 { a[2] } else { b[2] }));
                        }
                    }
                }
                out
            })
            .collect();
        if lo[0] > hi[0] {
            (lo, hi) = ([-1.0; 3], [1.0; 3]);
        }
        let centre = std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0);
        // (Flesh about the outermost joints: a tenth of its height more each way.)
        let pad = if skinned { 0.1 * (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max) } else { 0.0 };
        let half: [f64; 3] = std::array::from_fn(|k| (hi[k] - lo[k]) / 2.0 + pad);
        let radius = half.iter().map(|v| v * v).sum::<f64>().sqrt();
        // The guard's pose, and the joints it takes: the bones named and
        // all under them.
        let guard = self.guard.as_ref().map(|(at, bones)| {
            let tc = at - l.offset;
            let mut out = Vec::new();
            emit(l.children(), tc, step, &inner, 1.0, &mut out);
            out.retain(|s| matches!(s.content, Content::Mesh(_)));
            super::sim::pose_skins_in(l.children(), tc, step, &inner, &mut out);
            let mut under: HashSet<&str> = HashSet::new();
            fn gather<'l>(layers: &'l [CLayer], inside: bool, named: &[String], out: &mut HashSet<&'l str>) {
                for l in layers {
                    let inside = inside || named.iter().any(|n| n.as_str() == &*l.id);
                    if inside {
                        out.insert(&l.id);
                    }
                    gather(l.children(), inside, named, out);
                }
            }
            if let Some(bones) = bones {
                gather(l.children(), false, bones, &mut under);
            }
            let takes = out.iter().map(|s| match &s.content {
                // (A mesh with no skin: whether it is under a bone the pose takes.)
                Content::Mesh(m) => m.skin.as_ref().map_or(vec![bones.is_none() || under.contains(&*s.id)], |skin| skin.ids.iter().map(|id| bones.is_none() || under.contains(&**id)).collect()),
                _ => Vec::new(),
            }).collect();
            (out, takes)
        });
        let mut key = crate::sim::Fnv::default();
        key.str(&format!("{phases:?}{guard:?}"));
        Palette { phases, guard, centre, half, radius: radius.max(1e-6), key: key.0 }
    }

    /// The walk's moment `phase` with the guard's pose over it by
    /// `quarters` quarters.
    fn guarded(&self, palette: &Palette, phase: usize, quarters: u8) -> Option<Arc<Vec<LayerState>>> {
        let (pose, takes) = palette.guard.as_ref()?;
        let mut made = self.guarded.lock().unwrap_or_else(|e| e.into_inner());
        Some(made.entry((phase, quarters)).or_insert_with(|| {
            let f = quarters as f64 / 4.0;
            let between = |a: Mat4, b: Mat4| -> Mat4 {
                let ((pa, ra), (pb, rb)) = (crate::sim::decompose(a), crate::sim::decompose(b));
                let s = if (0..4).map(|k| pa.q[k] * pb.q[k]).sum::<f64>() < 0.0 { -1.0 } else { 1.0 };
                let q: [f64; 4] = std::array::from_fn(|k| pa.q[k] + (s * pb.q[k] - pa.q[k]) * f);
                let n = q.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
                let rigid = crate::sim::Pose { p: std::array::from_fn(|k| pa.p[k] + (pb.p[k] - pa.p[k]) * f), q: q.map(|v| v / n) };
                rigid.matrix().then(Mat4(std::array::from_fn(|k| ra.0[k] + (rb.0[k] - ra.0[k]) * f)))
            };
            Arc::new(palette.phases[phase].iter().map(|piece| {
                let mut piece = piece.clone();
                let Some(k) = pose.iter().position(|g| g.id == piece.id) else { return piece };
                let (Content::Mesh(m), Content::Mesh(g)) = (&mut piece.content, &pose[k].content) else { return piece };
                match (&m.skin, &g.skin) {
                    (Some(skin), Some(other)) if skin.joints.len() == other.joints.len() => {
                        let joints = skin.joints.iter().zip(&other.joints).enumerate().map(|(j, (a, b))| if takes[k].get(j).copied().unwrap_or(true) { between(*a, *b) } else { *a }).collect();
                        m.skin = Some(Arc::new(SkinPose { index: skin.index, blend: skin.blend, ids: skin.ids.clone(), joints }));
                    }
                    // (What is not skinned moves as a whole.)
                    (None, None) if takes[k].first().copied().unwrap_or(false) => m.world = between(m.world, g.world),
                    _ => {}
                }
                piece
            }).collect())
        }).clone())
    }
}

/// Emits a crowd's instances at the character's content time `t`: the
/// nearest as the character's meshes, the rest as pictures of it.
pub(super) fn emit_agents(l: &CLayer, h: &CHerd, t: f64, step: Option<Step>, space: &Space, opacity: f64, out: &mut Vec<LayerState>) {
    let herd = &h.bake.herds[h.herd];
    let x = h.bake.position(t - h.shift);
    let palette = h.palette.get_or_init(|| h.build(l, step, space));
    let phases = palette.phases.len();
    let mut agents: Vec<(f64, usize, Mat4, usize, f64)> = (0..herd.count as usize)
        .filter_map(|k| {
            let (pose, clock, turn) = herd.agent(k, x)?;
            let world = pose.matrix().then(herd.rest);
            let turns = (clock - h.clip_at) / h.cycle;
            Some((view_depth(space, world), k, world, ((turns - turns.floor()) * phases as f64).round() as usize % phases, turn))
        })
        .collect();
    // How far each has its guard up, in quarters.
    let quarters = |k: usize| if palette.guard.is_some() { (herd.guard_at(k, x) * 4.0).round().clamp(0.0, 4.0) as u8 } else { 0 };
    // Far to near.
    agents.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let lens = space.dof.map(|d| [d.plane, d.per_px]);
    let blur = |world: Mat4| -> Vec<EffectState> {
        let Some(dof) = space.dof else { return Vec::new() };
        let z = view_depth(space, world);
        let sigma = if z > 1.0 { (dof.per_px * dof.plane * dof.plane * (1.0 / dof.plane - 1.0 / z).abs()).min(dof.max) } else { dof.max };
        if sigma > 0.05 { vec![EffectState::Blur { sigma, repeat_edges: false, lens: true }] } else { Vec::new() }
    };
    let eye = crate::sim::inverse(space.cam.view).apply(0.0, 0.0, 0.0);
    let scale = (0..3).map(|k| (0..3).map(|r| herd.rest.0[r * 4 + k].powi(2)).sum::<f64>().sqrt()).fold(0.0, f64::max);
    // Pictures: px a px of the scene (the character's box is CELL px
    // corner to corner), and the ones this frame shows so far.
    let per_px = CELL as f64 / (2.0 * palette.radius * scale);
    let mut cards: HashMap<(usize, i64, i64, i64), (Arc<str>, f64, f64)> = HashMap::new();
    let first_full = agents.len().saturating_sub(h.full as usize);
    // The farthest of those drawn as the character come in over its
    // picture: the first of them a little of the character on all of the
    // picture, and so on (an eighth of them, twelve at most; and the last
    // tenth of `near`), so none changes from one to the other in a frame.
    let band = if first_full > 0 { (h.full as usize / 8).min(12) } else { 0 };
    let shown = |rank: usize, depth: f64| -> f64 {
        let by_rank = if rank < first_full { 0.0 } else if rank < first_full + band { (rank - first_full + 1) as f64 / (band + 1) as f64 } else { 1.0 };
        let by_depth = h.near.map_or(1.0, |d| ((d - depth) / (0.1 * d).max(1e-9)).clamp(0.0, 1.0));
        by_rank.min(by_depth)
    };
    for (rank, &(depth, k, world, phase, turned)) in agents.iter().enumerate() {
        let fallen = herd.fallen(k, x).and_then(|fall| h.slots.get(fall as usize));
        let solid = if fallen.is_some() { 1.0 } else { shown(rank, depth) };
        if solid < 1.0 {
            // A picture of it: the character seen from where the camera
            // is, as it is turned, at one of eight moments of its clip.
            if depth <= 1.0 {
                continue;
            }
            let c = world.apply(palette.centre[0], palette.centre[1], palette.centre[2]);
            let d = unit([c[0] - eye[0], c[1] - eye[1], c[2] - eye[2]]);
            let step_angle = std::f64::consts::TAU / HEADINGS;
            let round = |a: f64| (a / step_angle).round().rem_euclid(HEADINGS) as i64;
            // (Its guard half up or more: a picture of it with its guard up.)
            let up = quarters(k) >= 2;
            let key = (phase * 8 / phases + if up { 8 } else { 0 }, round(turned), round(crate::dm::atan2(d[0], d[2])), ((crate::dm::asin(d[1].clamp(-1.0, 1.0)) / step_angle).round() as i64).clamp(-3, 3));
            let (name, w, hgt) = cards.entry(key).or_insert_with(|| {
                let (moment, turn, az, el) = key;
                let (moment, up) = (moment % 8, moment >= 8);
                // The character at the origin, turned so; a camera far
                // off along the way it is seen, and a picture as large
                // as the character's box is from there.
                let (ta, aa, ea) = (turn as f64 * step_angle, az as f64 * step_angle, el as f64 * step_angle);
                let q = crate::sim::quat_mul([0.0, crate::dm::sin(ta / 2.0), 0.0, crate::dm::cos(ta / 2.0)], herd.q);
                let stand = Pose { p: [0.0; 3], q }.matrix().then(herd.rest);
                let at = stand.apply(palette.centre[0], palette.centre[1], palette.centre[2]);
                let dir = [crate::dm::sin(aa) * crate::dm::cos(ea), crate::dm::sin(ea), crate::dm::cos(aa) * crate::dm::cos(ea)];
                let turned = look(dir).then(stand);
                let mut reach = [0.0f64; 2];
                for corner in 0..8 {
                    let half: [f64; 3] = std::array::from_fn(|a| if corner >> a & 1 == 0 { -palette.half[a] } else { palette.half[a] });
                    for (a, r) in reach.iter_mut().enumerate() {
                        *r = r.max((0..3).map(|b| turned.0[a * 4 + b] * half[b]).sum::<f64>().abs());
                    }
                }
                let (w, hgt) = ((2.0 * reach[0] * per_px).ceil().max(2.0), (2.0 * reach[1] * per_px).ceil().max(2.0));
                let name: Arc<str> = format!("\0crowd/{}/{}/{moment}{}/{turn}/{az}/{el}", l.id, herd.id, if up { "g" } else { "" }).into();
                let far = FAR * palette.radius * scale;
                let view = Mat4::translate(0.0, 0.0, far).then(look(dir)).then(Mat4::translate(-at[0], -at[1], -at[2]));
                let focal = far * per_px;
                let cam = crate::space::Camera3 { view, project: Mat4([focal, 0.0, w / 2.0, 0.0, 0.0, focal, hgt / 2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]).then(view), focal, centre: (w / 2.0, hgt / 2.0) };
                let posed = up.then(|| h.guarded(palette, moment * phases / 8, 4)).flatten();
                let layers = posed.as_deref().unwrap_or(&palette.phases[moment * phases / 8])
                    .iter()
                    .filter_map(|piece| {
                        let Content::Mesh(m) = &piece.content else { return None };
                        let mut m = m.clone();
                        m.world = stand.then(m.world);
                        m.camera = cam;
                        m.lens = None;
                        Some(LayerState { id: piece.id.clone(), matrix: cam.project.then(m.world).plane(), opacity: piece.opacity, blend: Blend::Normal, effects: Vec::new(), matte: None, view: Some(cam.view.then(m.world)), content: Content::Mesh(m) })
                    })
                    .collect();
                PENDING.with(|p| p.borrow_mut().push(Picture { name: name.clone(), key: palette.key, width: w as u32, height: hgt as u32, layers }));
                (name, w, hgt)
            });
            // A card across the way it is seen from, about its middle.
            let card = Mat4::translate(c[0], c[1], c[2]).then(transpose3(look(d))).then(Mat4::scale(1.0 / per_px, 1.0 / per_px, 1.0));
            out.push(LayerState { id: l.id.clone(), matrix: space.screen(card, true), opacity, blend: Blend::Normal, effects: blur(card), matte: None, content: Content::Image { src: name.clone(), size: Some(Vec2::new(*w, *hgt)) }, view: Some(space.cam.view.then(card)) });
            if solid <= 0.0 {
                continue;
            }
        }
        let posed = match (quarters(k), fallen) {
            (q @ 1.., None) => h.guarded(palette, phase, q),
            _ => None,
        };
        for piece in posed.as_deref().unwrap_or(&palette.phases[phase]) {
            let Content::Mesh(m) = &piece.content else { continue };
            let mut m = m.clone();
            let w = world.then(m.world);
            if let (Some(tracks), Some(skin)) = (fallen, &m.skin) {
                // Down: its joints are where the bake has its bones.
                let to_mesh = crate::sim::inverse(w);
                let joints = skin.ids.iter().zip(&skin.joints).map(|(id, j)| tracks.get(&**id).and_then(|&tr| h.bake.world(tr, t - h.shift)).map_or(*j, |b| to_mesh.then(b))).collect();
                m.skin = Some(Arc::new(SkinPose { index: skin.index, blend: skin.blend, ids: skin.ids.clone(), joints }));
            }
            m.world = w;
            m.camera = space.cam;
            m.lens = lens;
            out.push(LayerState { id: piece.id.clone(), matrix: space.screen(w, true), opacity: opacity * piece.opacity * solid, blend: piece.blend, effects: blur(w), matte: None, view: Some(space.cam.view.then(w)), content: Content::Mesh(m) });
        }
    }
}
