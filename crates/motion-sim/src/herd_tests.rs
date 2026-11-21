//! Crowds of instances: one character, a pose reference an agent.

use crate::crowd_tests::{agent, at, crowd_scene, floor, level};
use crate::tests::{baked, project};
use lahza_motion_core::compile::Content;
use lahza_motion_core::sim::Bake;
use lahza_motion_core::space::Mat4;
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
    assert_eq!(crate::behaviour_tests::pin(&bake), (17657, 10946326245003775), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
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
    // None changes from the character to its picture in a frame: within
    // the last tenth of `near` an agent is the character coming in over
    // its picture (both drawn, the character part see-through, more of
    // it the nearer); nearer than that it is all there, and past `near`
    // none of it. (The six are within a twentieth of each other's depth.)
    {
        let far = cards.iter().map(|c| depth(c)).fold(f64::INFINITY, f64::min);
        let drawn = |near: f64| {
            let mut q = p.clone();
            let crowd = &mut q.scenes[0].simulation.as_mut().unwrap().crowds[0];
            (crowd.full, crowd.near) = (6, Some(near));
            let frame = Compiled::new(&q).frame(1.0);
            let (mut marks, mut cards) = (Vec::new(), Vec::new());
            states(&frame.layers, "a0-mark", &mut marks);
            states(&frame.layers, "a0", &mut cards);
            assert!(cards.iter().all(|c| c.opacity == 1.0));
            let mut by_depth: Vec<(f64, f64)> = marks.iter().map(|m| (depth(m), m.opacity)).collect();
            by_depth.sort_by(|a, b| a.0.total_cmp(&b.0));
            (by_depth.iter().map(|m| m.1).collect::<Vec<f64>>(), cards.len())
        };
        let (coming, under) = drawn(far * 1.04);
        assert!(coming.len() == 6 && under == 6 && coming.iter().all(|o| *o > 0.0 && *o < 1.0), "{coming:?} over {under} pictures");
        assert_eq!(drawn(far * 2.0), (vec![1.0; 6], 0));
        assert_eq!(drawn(far * 0.5), (vec![], 6));
    }
    let nearest_card = cards.iter().map(|c| depth(c)).fold(f64::INFINITY, f64::min);
    assert!(marks.iter().all(|m| depth(m) < nearest_card + 250.0));
    // The same frame asked for twice names the same pictures.
    assert_eq!(Compiled::new(&p).frame(1.0), frame);
}

#[test]
fn an_instance_struck_falls_as_a_ragdoll_of_the_pool() {
    let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [545, 700], "z": -150}, "body": {"mass": 30, "velocity": [0, 0, 1500]}});
    let mut p = herd_scene(vec![ball.clone()], json!({"ragdoll": {"mass": 60, "max": 2, "limits": [{"bone": ""}]}}), 1, 12.0);
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
    assert_eq!(crate::behaviour_tests::pin(&bake), (97272, 10319859945345561035), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
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
    // (As pinned: every joint 40° and 20°, not a human's by its name.)
    let mut ragdoll = ragdoll;
    ragdoll["limits"] = json!([{"bone": ""}]);
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
    assert_eq!((crate::behaviour_tests::pin(&limp), crate::behaviour_tests::pin(&held)), ((49695, 11188245037107290127), (49616, 10013887437959853070)), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
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
    let mut trial = take.fork(&input);
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

#[test]
fn in_a_try_the_agents_near_what_is_tried_steer_as_the_take_does() {
    // The six on their way round a pillar that stands across it: over
    // the next second each turns as it goes.
    let pillar = json!({"id": "pillar", "kind": "mesh", "mesh": {"type": "box", "size": [160, 300], "depth": 160}, "transform": {"position": [900, 650], "z": 150}, "body": {"type": "fixed"}});
    let p = herd_scene(vec![pillar], json!({}), 1, 4.0);
    let (input, errors) = Compiled::new(&p).sim_input(0, None);
    assert!(errors.is_empty(), "{errors:?}");
    let mut take = crate::Take::new(&input, &mut crate::core_meshes).unwrap();
    let mut herd = crate::crowd::Herd::new(&input, &mut take.world);
    let n = input.substeps as u64;
    let walk = |take: &mut crate::Take, herd: &mut crate::crowd::Herd, from: u32, to: u32| {
        for k in from..to {
            herd.plan(&input, &take.world);
            take.movers = herd.movers(input.rate);
            for s in 0..n {
                herd.drive(&mut take.world, (s + 1) as f64 / n as f64);
                take.step(&input, k, s);
            }
        }
    };
    walk(&mut take, &mut herd, 0, 45);
    herd.plan(&input, &take.world);
    take.movers = herd.movers(input.rate);
    let handles: Vec<_> = take.movers.iter().map(|m| m.0).collect();
    let place = |t: &crate::Take, i: usize| {
        let p = t.world.w.bodies[handles[i]].position().translation;
        [p.x as f64 * t.world.unit, p.y as f64 * t.world.unit, p.z as f64 * t.world.unit]
    };
    let before: Vec<[f64; 3]> = (0..6).map(|i| place(&take, i)).collect();
    let try_from = |take: &crate::Take| {
        let mut trial = take.fork(&input);
        for k in 45..75 {
            if k > 45 {
                trial.plan_crowd(&input);
            }
            for s in 0..n {
                trial.step(&input, k, s);
            }
        }
        trial
    };
    // A try that only carries them on, one whose agents steer (those
    // within reach of a place among them), and the take itself.
    let carried = try_from(&take);
    take.crowd = Some(herd.fork(&input, Some(&[[before[2][0], before[2][2]]]), 1.0));
    assert_eq!(take.crowd.as_ref().unwrap().steering(), 6);
    let steered = try_from(&take);
    // (One a long way from what is tried stands: it costs the try nothing.)
    let far = herd.fork(&input, Some(&[[before[2][0] + 4000.0, before[2][2]]]), 1.0);
    assert_eq!(far.steering(), 0);
    let mut on = take.fork(&input);
    on.crowd = Some(far);
    let stood = try_from(&on);
    // The take, a second on. (`herd` planned sample 45 already.)
    for s in 0..n {
        herd.drive(&mut take.world, (s + 1) as f64 / n as f64);
        take.step(&input, 45, s);
    }
    walk(&mut take, &mut herd, 46, 75);
    let mut off = 0.0f64;
    for i in 0..6 {
        let (real, tried, straight) = (place(&take, i), place(&steered, i), place(&carried, i));
        assert!(level(real, tried) < 0.5, "agent {i}: the try has it at {tried:?}, the take at {real:?}");
        assert!(level(place(&stood, i), before[i]) < 1e-6);
        off = off.max(level(real, straight));
    }
    // Carried on at the speed it had, one of them is well off where it went.
    assert!(off > 25.0, "carried straight on they are {off:.0} px from the take at most");
}

#[test]
fn an_agent_raises_its_guard_at_what_is_coming_and_is_drawn_in_it() {
    // The character with an arm on its spine; at 3.5 s (after its walk's
    // cycle) its spine is keyed a quarter round: that is its guard's pose. A
    // ball is thrown along the row's line at instance 3 from 900 px off.
    let scene = |crowd: Value| {
        let mut a = agent(0, 300.0, 0.0);
        let spine = &mut a["layers"][0]["layers"][0];
        spine["transform"]["rotation"] = json!({"keys": [{"at": 2, "v": 0}, {"at": 3.5, "v": 90, "ease": "linear"}]});
        spine["layers"].as_array_mut().unwrap().push(json!({"id": "a0-arm", "kind": "mesh", "mesh": {"type": "box", "size": [10, 60], "depth": 10}, "transform": {"position": [0, -30]}}));
        let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [540, 720], "z": -900}, "body": {"mass": 30, "velocity": [0, -120, 1500]}});
        let mut c = json!({"id": "walkers", "character": "a0", "starts": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect::<Vec<_>>(),
            "goals": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect::<Vec<_>>(), "radius": 30, "speed": 160, "stride": 140, "cycle": 1.0});
        for (k, v) in crowd.as_object().unwrap() {
            c[k] = v.clone();
        }
        project(json!({"id": "s1", "simulation": {"crowds": [c]}, "layers": [floor(), a, ball]}), 4.0)
    };
    let guard = json!({"guard": {"pose": 3.5, "ahead": 1.0, "bones": ["a0-spine"]}});
    let mut p = scene(guard.clone());
    let bake = baked(&mut p);
    let h = &bake.herds[0];
    assert_eq!(h.guard.len(), h.samples.len());
    let up = |k: usize, sample: usize| h.guard[sample * 6 + k];
    // The one it is thrown at sees it from the start (0.6 s off, within
    // the second its guard looks ahead): its guard is up in a sixth of a
    // second, held while it comes and half a second after, then down.
    assert!(up(3, 0) == 0.0 && up(3, 6) > 0.99 && up(3, 30) > 0.99, "{:?}", (0..40).map(|s| up(3, s)).collect::<Vec<_>>());
    let last = h.len() - 1;
    assert!(up(3, last) == 0.0 && (0..=last).any(|s| up(3, s) > 0.2 && up(3, s) < 0.8 && s > 30));
    // Those the ball is not coming at keep theirs down.
    assert!((0..=last).all(|s| up(0, s) == 0.0 && up(5, s) == 0.0));
    // It walks as it does without a guard (its steps aside are the same),
    // and the bake comes back from its bytes.
    let plain = baked(&mut scene(json!({})));
    assert!(plain.herds[0].samples == h.samples && plain.herds[0].guard.is_empty() && plain.hash != bake.hash);
    assert_eq!(Bake::from_bytes(&bake.to_bytes()).unwrap(), *bake);
    // Drawn: its arm stands up as it walks, lies a quarter round with
    // the guard up, and is between the two on the way; the others' and
    // what is not under the spine (the marker) are as they walk.
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let arms = |t: f64| -> Vec<Mat4> {
        let frame = c.frame(t);
        let mut found = Vec::new();
        states(&frame.layers, "a0-arm", &mut found);
        found.iter().map(|l| match &l.content {
            Content::Mesh(m) => m.world,
            _ => panic!(),
        }).collect()
    };
    // (The arm's own up, y, in the scene, a column of its world: how
    // much of it is level, whichever way the agent has turned, and how
    // much upright.)
    let lean = |m: &Mat4| (m.0[1].hypot(m.0[9]), m.0[5]);
    let (before, during) = (arms(0.0), arms(1.0));
    assert_eq!((before.len(), during.len()), (6, 6));
    assert!(before.iter().all(|m| lean(m).0.abs() < 1e-9 && (lean(m).1 - 1.0).abs() < 1e-9), "{:?}", before.iter().map(lean).collect::<Vec<_>>());
    let turned: Vec<&Mat4> = during.iter().filter(|m| lean(m).1.abs() < 1e-6 && (lean(m).0.abs() - 1.0).abs() < 1e-6).collect();
    assert_eq!(turned.len(), 1, "one arm a quarter round at 1 s: {:?}", during.iter().map(lean).collect::<Vec<_>>());
    let rising = arms(2.0 / 30.0);
    assert!(rising.iter().any(|m| lean(m).0.abs() > 0.2 && lean(m).0.abs() < 0.9), "{:?}", rising.iter().map(lean).collect::<Vec<_>>());
    let mut marks = Vec::new();
    let (walk, kept) = (Compiled::new(&{ let mut q = scene(json!({})); baked(&mut q); q }).frame(1.0), c.frame(1.0));
    states(&walk.layers, "a0-mark", &mut marks);
    let mut guarded_marks = Vec::new();
    states(&kept.layers, "a0-mark", &mut guarded_marks);
    assert!(marks.len() == 6 && marks.iter().zip(&guarded_marks).all(|(a, b)| a.matrix == b.matrix));
    // The pose is a moment of the scene.
    let e = Compiled::new(&scene(json!({"guard": {"pose": 50.0}}))).sim_input(0, None).1.join("\n");
    assert!(e.contains("crowds.0.guard.pose: 50 s is outside the scene"), "{e}");
    // A crowd of layers has no guard, and says why.
    let mut layered = crowd_scene(vec![], guard, 1, 1.0);
    let e = Compiled::new(&layered).sim_input(0, None).1.join("\n");
    assert!(e.contains("crowds.0.guard: only a crowd of instances"), "{e}");
    layered.bakes.clear();
    assert_eq!(crate::behaviour_tests::pin(&bake), (18476, 4764109921858966059), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

#[test]
fn a_light_that_moves_makes_new_pictures_a_step_not_a_frame() {
    // Six instances standing where they are, all as pictures, under a
    // sun that drifts some eight degrees and brightens by a tenth in a
    // second (and one that swings a quarter round and half as bright
    // again in that second).
    let scene = |lights: Value| {
        let mut c = json!({"id": "walkers", "character": "a0", "starts": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect::<Vec<_>>(),
            "goals": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect::<Vec<_>>(), "radius": 30, "speed": 160, "stride": 140, "cycle": 1.0, "full": 0});
        c["dodge"] = json!(0);
        project(json!({"id": "s1", "lights": lights, "simulation": {"crowds": [c]}, "layers": [floor(), agent(0, 300.0, 0.0)]}), 1.0)
    };
    // How many makings the frames of the second ask for: a picture is
    // made anew when its key is not the one it was last made with.
    let makings = |p: &mut Project| {
        baked(p);
        let c = Compiled::new(p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let mut held: std::collections::HashMap<std::sync::Arc<str>, u64> = Default::default();
        let (mut made, mut shown) = (0, 0);
        for k in 0..30 {
            for picture in &c.frame(k as f64 / 30.0).pictures {
                shown += 1;
                if held.insert(picture.name.clone(), picture.key) != Some(picture.key) {
                    made += 1;
                }
            }
        }
        (made, shown)
    };
    let still = json!([{"from": [-0.5, -1.0, -0.8], "intensity": 1.2}, {"type": "ambient", "intensity": 0.3}]);
    let sun = |to: [f64; 3], brighter: f64| json!([{"from": {"keys": [{"at": 0, "v": [-0.5, -1.0, -0.8]}, {"at": 1, "v": to, "ease": "linear"}]}, "intensity": {"keys": [{"at": 0, "v": 1.0}, {"at": 1, "v": brighter, "ease": "linear"}]}}, {"type": "ambient", "intensity": 0.3}]);
    let moving = sun([-0.3, -1.0, -0.8], 1.1);
    let (once, shown) = makings(&mut scene(still));
    let (stepped, shown_moving) = makings(&mut scene(moving.clone()));
    let (fast, _) = makings(&mut scene(sun([0.8, -1.0, -0.5], 1.5)));
    eprintln!("{once} makings of {shown} showings under a still light; {stepped} of {shown_moving} under one that drifts; {fast} under one that swings");
    // Still: each picture made once (they stand: one moment each).
    assert!(once <= 6 && shown >= 30);
    // Drifting: before, every showing was a making (its lights were part
    // of its key to the last bit). Now one a step of the light: its
    // eight degrees are two or three steps, its tenth brighter two.
    assert!(stepped <= shown_moving / 4 && stepped > once, "{stepped} makings of {shown_moving} showings");
    // (A light that swings a quarter round in a second takes a step
    // nearly every frame: that one is still a making a frame.)
    assert!(fast > shown_moving / 2);
    // A picture is what its key says: the same frame asked for first or
    // last carries the same pictures.
    let mut p = scene(moving);
    baked(&mut p);
    let (a, b) = (Compiled::new(&p), Compiled::new(&p));
    b.frame(0.2);
    assert_eq!(a.frame(0.7).pictures, b.frame(0.7).pictures);
}

/// A doorway `door` px wide in a wall across z = 400, twelve agents on
/// each side with their goals on the other.
fn doorway(door: f64, crowd: Value, seconds: f64) -> Project {
    let half = 1000.0;
    let wall = |id: &str, x: f64| json!({"id": id, "kind": "mesh", "mesh": {"type": "box", "size": [half, 300], "depth": 40}, "transform": {"position": [x, 650], "z": 400}, "body": {"type": "fixed"}});
    let (mut starts, mut goals) = (Vec::new(), Vec::new());
    for side in [0.0, 1.0] {
        for j in 0..2 {
            for i in 0..6 {
                let (x, z) = (300.0 + i as f64 * 80.0, 150.0 + j as f64 * 80.0);
                let (from, to) = ([x, z + side * 500.0 - j as f64 * side * 160.0], [x, z + 500.0 - side * 500.0 - j as f64 * (1.0 - side) * 160.0]);
                starts.push(json!(from));
                goals.push(json!(to));
            }
        }
    }
    let mut c = json!({"starts": starts, "goals": goals});
    for (k, v) in crowd.as_object().unwrap() {
        c[k] = v.clone();
    }
    herd_scene(vec![wall("wall-l", 500.0 - door / 2.0 - half / 2.0), wall("wall-r", 500.0 + door / 2.0 + half / 2.0)], c, 1, seconds)
}

/// How many of the 24 are through the wall by sample `k`, how many
/// within 100 px of their goals, and how near two of them ever came.
fn through_a_doorway(door: f64, seconds: f64) -> (Vec<usize>, usize, f64, std::sync::Arc<Bake>) {
    let mut p = doorway(door, json!({}), seconds);
    let goals: Vec<[f64; 2]> = serde_json::from_value(serde_json::to_value(&p.scenes[0]).unwrap()["simulation"]["crowds"][0]["goals"].clone()).unwrap();
    let bake = baked(&mut p);
    let h = &bake.herds[0];
    let at = |k: usize, i: usize| h.samples[k * 24 + i];
    let through = (0..h.len()).map(|k| (0..24).filter(|&i| (at(k, i)[1] > 400.0) == (goals[i][1] > 400.0)).count()).collect();
    let last = h.len() - 1;
    let arrived = (0..24).filter(|&i| ((at(last, i)[0] as f64 - goals[i][0]).powi(2) + (at(last, i)[1] as f64 - goals[i][1]).powi(2)).sqrt() < 100.0).count();
    let mut closest = f64::INFINITY;
    for k in 0..h.len() {
        for i in 0..24 {
            for j in 0..i {
                closest = closest.min(((at(k, i)[0] - at(k, j)[0]).powi(2) + (at(k, i)[1] - at(k, j)[1]).powi(2)).sqrt() as f64);
            }
        }
    }
    (through, arrived, closest, bake)
}

#[test]
fn two_dozen_cross_in_a_doorway_and_none_stands_in_another() {
    // Twelve on each side of a wall, their goals on the other, one door
    // 160 px wide (an agent is 30 px round, and a way is planned 46 px
    // off a wall: room for one at a time). They keep to the right of
    // those who come, wait behind those who go their way, and are moved
    // apart when nearer than 1.7 radii. (Before: all 24 were through in
    // 5 s, two of them 2 px from each other: they went through each other.)
    for door in [160.0, 280.0] {
        let (through, arrived, closest, bake) = through_a_doorway(door, 25.0);
        eprintln!("door {door}: through by 5, 10, 15, 20 s: {:?}; {arrived} within 100 px of their goals at 25 s; the closest two came {closest:.0} px", [150, 300, 450, 600].map(|k| through[k]));
        assert!(through[450] == 24 && arrived >= 22, "door {door}: {} through at 15 s, {arrived} arrived", through[450]);
        assert!(closest > 30.0, "door {door}: two came {closest:.0} px from each other");
        // (A door is a queue: not all at once.)
        assert!(through[150] < 24);
        if door == 160.0 {
            assert_eq!(crate::behaviour_tests::pin(&bake), (288809, 18174244763527505987), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
        }
    }
}

#[test]
fn those_who_walk_go_round_one_who_lies() {
    // Instance 3 is knocked down by a ball as they set off, and lies
    // where the three behind it in the row are to walk: their goals are
    // set past it, in line.
    let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [545, 700], "z": -150}, "body": {"mass": 30, "velocity": [0, 0, 1500]}});
    let lain = |goals: Value| {
        let mut p = herd_scene(vec![ball.clone()], json!({"ragdoll": {"mass": 60, "max": 2, "limits": [{"bone": ""}]}, "dodge": 0, "goals": goals}), 1, 14.0);
        baked(&mut p)
    };
    let hips = |bake: &Bake, k: usize| at(bake, "walkers#0/a0-hips", k);
    // The three behind it are sent 600 px along the row's line, on which
    // it lies; the two before it walk off.
    let from = |i: usize| [300.0 + i as f64 * 80.0, 0.0];
    let goals: Vec<[f64; 2]> = (0..6).map(|i| if i < 3 { [from(i)[0] + 600.0, 0.0] } else { [from(i)[0], -300.0] }).collect();
    let bake = lain(json!(goals));
    let lies = hips(&bake, 420);
    let h = &bake.herds[0];
    assert!(h.down[3] < 10 && level(hips(&bake, 150), lies) < 5.0 && lies[0] > 420.0 && lies[0] < 800.0 && lies[2].abs() < 60.0, "it lies at {lies:?}");
    for i in 0..3 {
        let way: Vec<[f64; 3]> = (0..h.len()).map(|k| { let s = h.samples[k * 6 + i]; [s[0] as f64, 0.0, s[1] as f64] }).collect();
        let nearest = (30..h.len()).map(|k| level(way[k], hips(&bake, k))).fold(f64::INFINITY, f64::min);
        let end = way[h.len() - 1];
        eprintln!("instance {i}: {nearest:.0} px from the hips of the one that lies at its nearest; ended {:.0} px from its goal", level(end, [goals[i][0], 0.0, goals[i][1]]));
        // Past it, and never over it (its bones' box is shut ground once
        // it lies: a way is planned round, an agent's radius off).
        assert!(level(end, [goals[i][0], 0.0, goals[i][1]]) < 40.0 && nearest > 40.0, "instance {i} came {nearest:.0} px from its hips and ended at {end:?}");
    }
    assert_eq!(crate::behaviour_tests::pin(&bake), (113054, 11342613009363423641), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

/// Instances of the sixteen-bone figure (`ragdoll_tests`), standing on
/// the floor: three in a row 220 px apart, walking along x.
fn figures(extra: Vec<Value>, crowd: Value, ragdoll: Value, seconds: f64) -> Project {
    let hero = json!({"id": "hero", "kind": "comp", "three_d": true, "transform": {"position": [300, 800]}, "layers": [crate::ragdoll_tests::figure(json!({"transform": {"position": [0, -250.5]}}))]});
    let mut layers = vec![floor(), hero];
    layers.extend(extra);
    let mut r = json!({"mass": 70, "max": 2,
        "shapes": [{"bone": "foot", "shape": {"type": "box", "size": [44, 24, 130], "offset": [0, 12, -20]}}],
        "limits": [{"bone": "shin", "swing": 130, "hinge": [-1, 0, 0]}, {"bone": "thigh", "swing": 70}, {"bone": "upper", "swing": 120, "twist": 60}, {"bone": "fore", "swing": 140, "hinge": [1, 0, 0]}, {"bone": ""}]});
    for (k, v) in ragdoll.as_object().unwrap() {
        r[k] = v.clone();
    }
    let mut c = json!({"id": "walkers", "character": "hero", "starts": [[300, 0], [520, 0], [740, 0]], "march": [900, 0], "radius": 40, "height": 420, "speed": 120, "vary": 0, "stride": 140, "cycle": 1.0, "dodge": 0, "ragdoll": r});
    for (k, v) in crowd.as_object().unwrap() {
        c[k] = v.clone();
    }
    project(json!({"id": "s1", "simulation": {"seed": 1, "crowds": [c]}, "layers": layers}), seconds)
}

/// A ball thrown across the row at the instance that starts at `x`.
fn thrown(id: &str, x: f64, at: f64) -> Value {
    json!({"id": id, "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "transform": {"position": [x, 430], "z": -260}, "body": {"mass": 30, "release": at, "velocity": [0, 0, 1600]}})
}

#[test]
fn one_knocked_down_gets_up_and_walks_on() {
    // One of the sixteen-bone figure, walking along x, struck from the
    // side at 0.1 s by a ball at 1600 px/s; its crowd's fallen get up.
    let take = || {
        let mut p = figures(vec![thrown("ball", 530.0, 0.0)], json!({"starts": [[520, 0], [1500, 0], [1500, 300]], "knock": 800}), json!({"get_up": {}}), 14.0);
        baked(&mut p)
    };
    let bake = take();
    let events: Vec<(&str, &str, f64)> = bake.events.iter().filter(|e| ["knock", "fall", "get_up", "rise", "up"].contains(&e.kind.as_str())).map(|e| (e.kind.as_str(), e.a.as_str(), e.t)).collect();
    // Knocked, down as the pool's first ragdoll, still for 0.8 s, up as
    // `get_up` does it (helped, by 2.4 s), and then an agent again.
    let kinds: Vec<(&str, &str)> = events.iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(kinds, [("knock", "walkers#0"), ("fall", "walkers#0"), ("get_up", "walkers#0"), ("rise", "walkers#0"), ("up", "walkers#0")], "{events:?}");
    let (down, up) = (events[0].2, events[4].2);
    assert!(down < 0.2 && up > 3.5 && up < 6.5 && (events[3].2 - up).abs() < 0.05, "{events:?}");
    let h = &bake.herds[0];
    assert_eq!(h.falls.len(), 1);
    let [who, from, to, fall] = h.falls[0];
    assert!(who == 0 && fall == 0 && from == h.down[0] && (to as f64 / 30.0 - up).abs() < 0.04, "{:?}", h.falls);
    // It is its ragdoll's from the one to the other, and the agent's
    // before and after.
    assert!(h.fallen(0, from as f64 - 0.5).is_none() && h.fallen(0, from as f64) == Some(0) && h.fallen(0, to as f64 - 0.01) == Some(0) && h.fallen(0, to as f64).is_none());
    // Down, its hips were on the floor; it walks on from where it stood
    // up, and is at its goal at the end. Its bones stay as they stood.
    let hips = |k: usize| at(&bake, "walkers#0/hips", k);
    let lowest = (from as usize..to as usize).map(|k| hips(k)[1]).fold(f64::MIN, f64::max);
    let agent = |k: usize| { let s = h.samples[k * 3]; [s[0] as f64, 0.0, s[1] as f64] };
    assert!(lowest > 740.0 && (hips(to as usize)[1] - 550.0).abs() < 25.0, "its hips were at y {lowest:.0} at their lowest and are at {:.0} as it stands", hips(to as usize)[1]);
    assert!(level(agent(to as usize), hips(to as usize)) < 30.0, "it walks on from {:?}; its hips were at {:?}", agent(to as usize), hips(to as usize));
    assert!(level(agent(to as usize + 60), agent(to as usize)) > 100.0 && level(agent(h.len() - 1), [1420.0, 0.0, 0.0]) < 40.0, "it ended at {:?}", agent(h.len() - 1));
    assert!(hips(h.len() - 1) == hips(to as usize - 1));
    eprintln!("down at {down:.2} s, up at {up:.2} s, {:.0} px from where it was struck", level(agent(to as usize), [520.0, 0.0, 0.0]));
    // The same bytes again; and the bake survives its file.
    assert!(take().to_bytes() == bake.to_bytes());
    assert_eq!(Bake::from_bytes(&bake.to_bytes()).unwrap(), *bake);
    assert_eq!(crate::behaviour_tests::pin(&bake), (225744, 18350922712031349476), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

#[test]
fn a_ragdoll_of_the_pool_falls_again_for_the_next() {
    // Six standing, a pool of one ragdoll, four balls: at instance 3 as
    // the take starts, at 1 at 5 s, at 5 at 5.4 s, at 0 at 10 s.
    let ball = |id: &str, x: f64, at: f64| json!({"id": id, "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [x + 5.0, 700], "z": -150}, "body": {"mass": 30, "release": at, "velocity": [0, 0, 1500]}});
    let starts: Vec<Value> = (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect();
    let mut p = herd_scene(vec![ball("b3", 540.0, 0.0), ball("b1", 380.0, 5.0), ball("b5", 700.0, 5.4), ball("b0", 300.0, 10.0)], json!({"goals": starts, "dodge": 0, "knock": 900, "ragdoll": {"mass": 60, "max": 1, "limits": [{"bone": ""}]}}), 1, 14.0);
    let bake = baked(&mut p);
    let knocks: Vec<(&str, f64)> = bake.events.iter().filter(|e| e.kind == "knock").map(|e| (e.a.as_str(), e.t)).collect();
    assert_eq!(knocks.iter().map(|k| k.0).collect::<Vec<_>>(), ["walkers#3", "walkers#1", "walkers#5", "walkers#0"], "{knocks:?}");
    let h = &bake.herds[0];
    // The first lay still a second and more: the second falls as its
    // ragdoll, and the fourth as it again. The third was struck while it
    // was still falling: with none to take, it stops where it stands.
    let falls: Vec<[u32; 4]> = h.falls.clone();
    assert!(falls.len() == 3 && falls.iter().map(|f| (f[0], f[3])).collect::<Vec<_>>() == [(3, 0), (1, 1), (0, 2)] && falls.iter().all(|f| f[2] == u32::MAX), "{falls:?}");
    assert_eq!(h.slot, [2, 1, u32::MAX, 0, u32::MAX, u32::MAX]);
    assert!(h.down[5] != u32::MAX && h.fallen(5, 400.0).is_none() && h.fallen(3, 400.0) == Some(0) && h.fallen(1, 400.0) == Some(1) && h.fallen(0, 400.0) == Some(2));
    // Each fall has its own bones' tracks; a fall's stay as they lay from
    // when its ragdoll was given on.
    let ids: Vec<&str> = bake.tracks.iter().map(|t| t.id.as_str()).filter(|id| id.ends_with("hips")).collect();
    assert_eq!(ids, ["walkers#0/a0-hips", "walkers#1/a0-hips", "walkers#2/a0-hips"]);
    let hips = |fall: usize, k: usize| at(&bake, &format!("walkers#{fall}/a0-hips"), k);
    let (second, third, last) = (falls[1][1] as usize, falls[2][1] as usize, h.len() - 1);
    assert!(hips(0, second - 1) == hips(0, last) && level(hips(0, last), hips(0, second - 30)) < 2.0, "the first lay at {:?}, and at the end {:?}", hips(0, second), hips(0, last));
    assert!(hips(1, third - 1) == hips(1, last) && level(hips(1, last), hips(0, last)) > 60.0 && hips(1, last)[1] > 770.0);
    assert!(level(hips(2, last), hips(1, last)) > 30.0 && hips(2, last)[1] > 770.0 && bake.tracks[bake.track("walkers#2/a0-hips").unwrap()].from as usize == third);
    assert_eq!(Bake::from_bytes(&bake.to_bytes()).unwrap(), *bake);
    assert_eq!(crate::behaviour_tests::pin(&bake), (269649, 10792876780960502325), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

#[test]
fn one_knocked_down_with_its_guard_up_falls_from_that_pose() {
    // The guard test's character (its spine a quarter round in the
    // guard's pose) and its ball, which now strikes: instance 3 has its
    // guard up when it does.
    let scene = |crowd: Value| {
        let mut a = agent(0, 300.0, 0.0);
        let spine = &mut a["layers"][0]["layers"][0];
        spine["transform"]["rotation"] = json!({"keys": [{"at": 2, "v": 0}, {"at": 3.5, "v": 90, "ease": "linear"}]});
        let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [540, 720], "z": -900}, "body": {"mass": 30, "velocity": [0, -120, 1500]}});
        let starts: Vec<Value> = (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect();
        let mut c = json!({"id": "walkers", "character": "a0", "starts": starts, "goals": starts, "radius": 30, "speed": 160, "stride": 140, "cycle": 1.0, "dodge": 0, "ragdoll": {"mass": 60, "max": 1, "limits": [{"bone": ""}]}});
        for (k, v) in crowd.as_object().unwrap() {
            c[k] = v.clone();
        }
        let mut p = project(json!({"id": "s1", "simulation": {"crowds": [c]}, "layers": [floor(), a, ball]}), 4.0);
        let input = Compiled::new(&p).sim_input(0, None).0;
        (baked(&mut p), input)
    };
    // How far its spine is turned against its hips as it is let go.
    let bent = |bake: &Bake| {
        let h = &bake.herds[0];
        let k = h.down[3] as usize;
        assert!(k < 30 && h.slot[3] == 0, "down {:?}", h.down);
        let q = |id: &str| { let s = bake.tracks[bake.track(id).unwrap()].samples[k]; [s[3] as f64, s[4] as f64, s[5] as f64, s[6] as f64] };
        let (a, b) = (q("walkers#0/a0-hips"), q("walkers#0/a0-spine"));
        ((2.0 * (0..4).map(|i| a[i] * b[i]).sum::<f64>().abs().min(1.0).acos()).to_degrees(), h.guard.get(k * 6 + 3).copied())
    };
    let (plain, input) = scene(json!({}));
    assert!(input.crowds[0].guard_pose.is_empty() && bent(&plain).0 < 3.0, "walking, its spine is {:.0}° from its hips", bent(&plain).0);
    let (guarded, input) = scene(json!({"guard": {"pose": 3.5, "ahead": 1.0, "bones": ["a0-spine"]}}));
    // (The pose takes the spine and the head on it; not the hips or legs.)
    assert_eq!(input.crowds[0].guard_pose.iter().map(|g| g.1).collect::<Vec<_>>(), [false, true, true, false, false]);
    let (angle, up) = bent(&guarded);
    assert!(up.is_some_and(|g| g > 0.99) && (angle - 90.0).abs() < 4.0, "its guard {up:?} up, its spine {angle:.0}° from its hips as it is let go");
    assert_eq!(crate::behaviour_tests::pin(&guarded), (36989, 3641593593800269882), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}
