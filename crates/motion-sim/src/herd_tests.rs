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
    assert_eq!(crate::behaviour_tests::pin(&bake), (17657, 11143674950103157318), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
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
    assert_eq!(crate::behaviour_tests::pin(&bake), (97293, 15291254770378900992), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
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
