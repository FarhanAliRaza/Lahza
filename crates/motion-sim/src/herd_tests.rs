//! Crowds of instances: one character, a pose reference an agent.

use crate::crowd_tests::{agent, at, crowd_scene, floor, level};
use crate::tests::{baked, project};
use lahza_motion_core::compile::Content;
use lahza_motion_core::sim::Bake;
use lahza_motion_core::{Compiled, LayerState, Project};
use serde_json::{json, Value};

/// The six agents as instances of one character (`a0`), started where
/// `crowd_scene` has its layers.
fn herd_scene(extra: Vec<Value>, crowd: Value, seed: u64, seconds: f64) -> Project {
    let mut layers = vec![floor(), agent(0, 300.0, 0.0)];
    layers.extend(extra);
    let mut c = json!({"id": "walkers", "character": "a0", "starts": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect::<Vec<_>>(),
        "goals": (0..6).map(|i| json!([1300.0 + i as f64 * 90.0, 260.0])).collect::<Vec<_>>(), "radius": 30, "speed": 160, "stride": 140, "cycle": 1.0});
    for (k, v) in crowd.as_object().unwrap() {
        c[k] = v.clone();
    }
    project(json!({"id": "s1", "simulation": {"seed": seed, "crowds": [c]}, "layers": layers}), seconds)
}

fn states<'a>(layers: &'a [LayerState], id: &str, out: &mut Vec<&'a LayerState>) {
    for l in layers {
        if &*l.id == id {
            out.push(l);
        }
        if let Content::Group(g) = &l.content {
            states(g, id, out);
        }
    }
}

#[test]
fn instances_of_one_character_walk_as_its_copies_did() {
    let pillar = json!({"id": "pillar", "kind": "mesh", "mesh": {"type": "box", "size": [90, 300], "depth": 90}, "transform": {"position": [950, 650], "z": 130}, "body": {"type": "fixed"}});
    let mut p = herd_scene(vec![pillar.clone()], json!({}), 1, 6.0);
    let bake = baked(&mut p);
    // No track per agent: one herd of six, four numbers a sample each.
    assert!(bake.tracks.is_empty(), "{:?}", bake.tracks.iter().map(|t| &t.id).collect::<Vec<_>>());
    let h = &bake.herds[0];
    assert_eq!((h.id.as_str(), h.count, h.len(), h.samples.len()), ("walkers", 6, 181, 6 * 181));
    assert!(h.down.iter().chain(&h.slot).all(|v| *v == u32::MAX));
    // Each walks exactly as the agent that was a layer of its own did.
    let layered = baked(&mut crowd_scene(vec![pillar], json!({}), 1, 6.0));
    for i in 0..6 {
        let t = &layered.tracks[layered.track(&format!("a{i}")).unwrap()];
        for k in 0..181 {
            let s = h.samples[k * 6 + i];
            assert_eq!((s[0], s[1], s[3]), (t.samples[k][0], t.samples[k][2], t.clock[k]), "agent {i}, sample {k}");
            let (pose, clock, _) = h.agent(i, k as f64).unwrap();
            let q: Vec<f64> = (3..7).map(|c| t.samples[k][c] as f64).collect();
            assert!((0..4).map(|c| pose.q[c] * q[c]).sum::<f64>().abs() > 1.0 - 1e-6 && (pose.p[1] - 800.0).abs() < 1e-9 && clock == s[3] as f64, "agent {i}, sample {k}: turned {:?}, its layer {q:?}", pose.q);
        }
    }
    // The bake is the same file read back, and the same bytes baked again.
    let bytes = bake.to_bytes();
    assert_eq!(Bake::from_bytes(&bytes).unwrap(), *bake);
    assert!(bytes == baked(&mut p).to_bytes());
    assert_eq!(crate::behaviour_tests::pin(&bake), (17657, 8715268341068219897), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
    // Drawn: the character is there six times, each at its agent's place
    // and its own moment of the clip (the marker is 100 px a second of
    // clip along), and not once more where its keys have it.
    let c = Compiled::new(&p);
    for k in [0usize, 31, 77] {
        let frame = c.frame(k as f64 / 30.0);
        let mut marks = Vec::new();
        states(&frame.layers, "a0-mark", &mut marks);
        assert_eq!(marks.len(), 6);
        assert!(frame.pictures.is_empty());
        for i in 0..6 {
            let (pose, clock, _) = h.agent(i, k as f64).unwrap();
            let want = pose.matrix().then(h.rest).apply(100.0 * clock, -200.0, 0.0);
            let nearest = marks
                .iter()
                .map(|m| {
                    let Content::Mesh(m) = &m.content else { panic!() };
                    (0..3).map(|a| (m.world.0[a * 4 + 3] - want[a]).powi(2)).sum::<f64>().sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            // (One of 60 moments of the clip: within a 120th of a second of it.)
            assert!(nearest < 100.0 / 120.0 + 0.05, "sample {k}: no marker within {nearest:.2} px of agent {i}'s");
        }
    }
    // Only the nearest `full` are the character itself; the rest are
    // pictures of it, which the frame carries the making of.
    p.scenes[0].simulation.as_mut().unwrap().crowds[0].full = 2;
    let frame = Compiled::new(&p).frame(1.0);
    let (mut marks, mut cards) = (Vec::new(), Vec::new());
    states(&frame.layers, "a0-mark", &mut marks);
    states(&frame.layers, "a0", &mut cards);
    assert_eq!((marks.len(), cards.len()), (2, 4));
    assert!(!frame.pictures.is_empty() && frame.pictures.len() <= 4);
    for card in &cards {
        let Content::Image { src, size } = &card.content else { panic!("{:?}", card.content) };
        let picture = frame.pictures.iter().find(|p| p.name == *src).unwrap_or_else(|| panic!("no picture {src}"));
        assert_eq!((picture.width as f64, picture.height as f64), (size.unwrap().x(), size.unwrap().y()));
        assert!(picture.layers.iter().any(|l| &*l.id == "a0-mark" && matches!(&l.content, Content::Mesh(m) if m.lighting.is_some())));
        assert!(card.view.is_some());
    }
    // The cards are the farther ones.
    let depth = |l: &LayerState| l.view.unwrap().apply(0.0, 0.0, 0.0)[2];
    let nearest_card = cards.iter().map(|c| depth(c)).fold(f64::INFINITY, f64::min);
    assert!(marks.iter().all(|m| depth(m) < nearest_card + 250.0));
    // The same frame asked for twice names the same pictures.
    assert_eq!(Compiled::new(&p).frame(1.0), frame);
}

#[test]
fn an_instance_struck_falls_as_a_ragdoll_of_the_pool() {
    let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [545, 700], "z": -150}, "body": {"mass": 30, "velocity": [0, 0, 1500]}});
    let mut p = herd_scene(vec![ball.clone()], json!({"ragdoll": {"mass": 60, "max": 2}}), 1, 12.0);
    let bake = baked(&mut p);
    let knocks: Vec<_> = bake.events.iter().filter(|e| e.kind == "knock").collect();
    assert_eq!(knocks.len(), 1, "{knocks:?}");
    assert_eq!((knocks[0].a.as_str(), knocks[0].b.as_str()), ("walkers#3", "ball"));
    let h = &bake.herds[0];
    let down = h.down[3] as usize;
    assert!(down.abs_diff((knocks[0].t * 30.0).ceil() as usize) <= 1 && h.slot[3] == 0);
    assert!((0..6).filter(|&i| i != 3).all(|i| h.down[i] == u32::MAX && h.slot[i] == u32::MAX));
    // The pool's first ragdoll is its bones from then; the second, never
    // let go, has no tracks.
    let ids: Vec<&str> = bake.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["ball", "walkers#0/a0-hips", "walkers#0/a0-spine", "walkers#0/a0-head", "walkers#0/a0-leg_l", "walkers#0/a0-leg_r"]);
    let n = h.len();
    for id in &ids[1..] {
        let t = &bake.tracks[bake.track(id).unwrap()];
        assert_eq!(t.from as usize, down, "{id}");
        let (end, before) = (at(&bake, id, n - 1), at(&bake, id, n - 16));
        assert!(end[1] > 740.0 && end[1] < 801.0 && level(end, before) < 3.0, "{id} ended at {end:?}");
    }
    // They start where the clip had them in that agent, not in the character.
    let (pose, _, _) = h.agent(3, down as f64).unwrap();
    let head = at(&bake, "walkers#0/a0-head", down);
    assert!(level(head, pose.p) < 40.0 && (pose.p[1] - head[1] - 160.0).abs() < 30.0, "the head at {head:?} over the agent at {:?}", pose.p);
    // The agent stays where it fell; the others arrive.
    let place = |i: usize, k: usize| [h.samples[k * 6 + i][0] as f64, 0.0, h.samples[k * 6 + i][1] as f64];
    assert!(level(place(3, n - 1), place(3, down + 1)) < 1e-6);
    for i in [0, 1, 2, 4, 5] {
        assert!(level(place(i, n - 1), [1300.0 + i as f64 * 90.0, 0.0, 260.0]) < 30.0, "agent {i}");
    }
    assert_eq!(crate::behaviour_tests::pin(&bake), (97272, 11227731455442370143), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
    // Still drawn six times after the fall.
    let frame = Compiled::new(&p).frame(6.0);
    let mut marks = Vec::new();
    states(&frame.layers, "a0-mark", &mut marks);
    assert_eq!(marks.len(), 6);
    // What is wrong is named.
    let errors = |crowd: Value| {
        let p = herd_scene(vec![], crowd, 1, 1.0);
        Compiled::new(&p).sim_input(0, None).1.join("\n")
    };
    assert!(errors(json!({"character": "nobody"})).contains("crowds.0.character: no layer 'nobody'"));
    assert!(errors(json!({"character": "floor"})).contains("'floor' is no comp"));
    assert!(errors(json!({"character": "a0-hips"})).contains("is inside another layer"));
    assert!(errors(json!({"agents": ["a0"]})).contains("not both"));
    assert!(errors(json!({"starts": []})).contains("instances need \"starts\""));
    assert!(errors(json!({"starts": [], "count": 400, "area": [0, 0, 300, 300]})).contains("do not fit"));
    assert!(errors(json!({"goals": []})).contains("\"goals\" ([[x, z]]) or \"march\""));
    // A count over an area: every one inside it, none on another, the
    // same for the same seed.
    let spread = |seed: u64| {
        let p = herd_scene(vec![], json!({"starts": [], "count": 40, "area": [0, -900, 1900, 900], "goals": [], "march": [0, 500]}), seed, 1.0);
        let (input, errors) = Compiled::new(&p).sim_input(0, None);
        assert!(errors.is_empty(), "{errors:?}");
        input.crowds[0].agents.iter().map(|a| ([a.start.p[0], a.start.p[2]], a.goal)).collect::<Vec<_>>()
    };
    let (a, b) = (spread(1), spread(2));
    assert!(a.len() == 40 && a == spread(1) && a != b);
    for (i, (at, goal)) in a.iter().enumerate() {
        assert!((0.0..=1900.0).contains(&at[0]) && (-900.0..=900.0).contains(&at[1]) && *goal == [at[0], at[1] + 500.0]);
        assert!(a[..i].iter().all(|(o, _)| ((o[0] - at[0]).powi(2) + (o[1] - at[1]).powi(2)).sqrt() >= 60.0));
    }
}

/// `herd_scene` with a ball thrown at instance 3, not looked for, and the
/// character's left leg swung in its clip: 30° forward and back a cycle.
fn struck(ragdoll: Value) -> std::sync::Arc<Bake> {
    let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [545, 700], "z": -150}, "body": {"mass": 30, "velocity": [0, 0, 1500]}});
    let mut p = herd_scene(vec![ball], json!({"ragdoll": ragdoll, "dodge": 0}), 1, 6.0);
    let mut scene = serde_json::to_value(&p.scenes[0]).unwrap();
    let leg = scene["layers"][1]["layers"][0]["layers"].as_array_mut().unwrap().iter_mut().find(|l| l["id"] == "a0-leg_l").unwrap();
    leg["transform"]["rotation_x"] = json!({"keys": [{"at": 0, "v": -30}, {"at": 0.5, "v": 30, "ease": "linear"}, {"at": 1.0, "v": -30, "ease": "linear"}]});
    p.scenes[0] = serde_json::from_value(scene).unwrap();
    baked(&mut p)
}

#[test]
fn one_knocked_down_keeps_its_limbs_swing_and_may_hold_its_pose() {
    let limp = struck(json!({"mass": 60}));
    let h = &limp.herds[0];
    let down = h.down[3] as usize;
    assert!(h.slot[3] == 0 && down < 10);
    // The leg leaves the clip turning as it turned in it: 120° a second
    // in the agent, times the 160/140 its walk runs the clip at, against
    // its hips. (Over the first frame of the fall.)
    let turned = |id: &str| {
        let q = |id: &str, k: usize| crate::ragdoll_tests::track_pose(&limp, &format!("walkers#0/a0-{id}"), k).q;
        let against = |k: usize| lahza_motion_core::sim::quat_mul({ let h = q("hips", k); [-h[0], -h[1], -h[2], h[3]] }, q(id, k));
        let (a, b) = (against(down), against(down + 1));
        (2.0 * (0..4).map(|i| a[i] * b[i]).sum::<f64>().abs().min(1.0).acos()).to_degrees() * 30.0
    };
    assert!(turned("leg_l") > 90.0 && turned("leg_l") < 180.0, "the swung leg turns at {:.0}°/s against its hips", turned("leg_l"));
    assert!(turned("leg_r") < 25.0, "the other leg, still in the clip, at {:.0}°/s", turned("leg_r"));
    // Limp, it crumples: a second on, its head is far from over its hips.
    // Holding the pose it fell in (strength 0.9), it goes down as it stood.
    let held = struck(json!({"mass": 60, "strength": 0.9}));
    assert_eq!(held.herds[0].down[3] as usize, down);
    let bend = |bake: &Bake, k: usize| {
        // The angle at the spine: hips → spine against spine → head.
        let (a, b, c) = (at(bake, "walkers#0/a0-hips", k), at(bake, "walkers#0/a0-spine", k), at(bake, "walkers#0/a0-head", k));
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - b[0], c[1] - b[1], c[2] - b[2]]);
        let dot: f64 = (0..3).map(|i| u[i] * v[i]).sum();
        (dot / ((0..3).map(|i| u[i] * u[i]).sum::<f64>().sqrt() * (0..3).map(|i| v[i] * v[i]).sum::<f64>().sqrt())).clamp(-1.0, 1.0).acos().to_degrees()
    };
    let (soft, stiff) = ((down..down + 45).map(|k| bend(&limp, k)).fold(0.0, f64::max), (down..down + 45).map(|k| bend(&held, k)).fold(0.0, f64::max));
    assert!(soft > 25.0 && stiff < 0.5 * soft, "its spine bends {soft:.0}° limp, {stiff:.0}° holding its pose");
    assert_eq!((crate::behaviour_tests::pin(&limp), crate::behaviour_tests::pin(&held)), ((49850, 4585677346052108598), (51323, 784940492317639703)), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

#[test]
fn a_try_ahead_has_the_agents_walk_on() {
    let p = herd_scene(vec![], json!({}), 1, 2.0);
    let (input, errors) = Compiled::new(&p).sim_input(0, None);
    assert!(errors.is_empty(), "{errors:?}");
    let mut take = crate::Take::new(&input, &mut crate::core_meshes).unwrap();
    let mut herd = crate::crowd::Herd::new(&input, &mut take.world);
    // A second in, as the bake goes.
    let n = input.substeps as u64;
    for k in 0..30 {
        herd.plan(&input, &take.world);
        take.movers = herd.movers(input.rate);
        for s in 0..n {
            herd.drive(&mut take.world, (s + 1) as f64 / n as f64);
            take.step(&input, k, s);
        }
    }
    herd.plan(&input, &take.world);
    take.movers = herd.movers(input.rate);
    assert_eq!(take.movers.len(), 6);
    let place = |t: &crate::Take, i: usize| {
        let p = t.world.w.bodies[t.movers[i].0].position().translation;
        [p.x as f64 * t.world.unit, p.y as f64 * t.world.unit, p.z as f64 * t.world.unit]
    };
    let before: Vec<[f64; 3]> = (0..6).map(|i| place(&take, i)).collect();
    // Half a second of a try: each agent's capsule is where its speed
    // takes it (they walk at 160 px/s, a fifth either way).
    let mut trial = take.fork();
    for k in 30..45 {
        for s in 0..n {
            trial.step(&input, k, s);
        }
    }
    for i in 0..6 {
        let v = take.movers[i].1;
        let (now, want) = (place(&trial, i), [before[i][0] + v[0] * 0.5, before[i][1] + v[1] * 0.5, before[i][2] + v[2] * 0.5]);
        let speed = (v[0] * v[0] + v[2] * v[2]).sqrt();
        assert!(speed > 100.0 && speed < 200.0 && level(now, want) < 1.0 && level(now, before[i]) > 50.0, "agent {i} at {now:?}, was {:?}, going {v:?}", before[i]);
        // The take itself is not moved by that: its agents are the crowd's to move.
        assert!(level(place(&take, i), before[i]) < 1e-9);
    }
}
