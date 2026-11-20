use crate::tests::{baked, project};
use lahza_motion_core::compile::Content;
use lahza_motion_core::sim::Bake;
use lahza_motion_core::space::Mat4;
use lahza_motion_core::{Compiled, LayerState, Project};
use serde_json::{json, Value};

/// An agent: a comp at (x, 800) on the floor holding a five-bone figure
/// (hips, spine, head, two legs) and a marker that its clip moves 100 px
/// a second from the clip's start.
pub(crate) fn agent(i: usize, x: f64, z: f64) -> Value {
    let bone = |id: &str, at: [f64; 2], kids: Vec<Value>| json!({"id": format!("a{i}-{id}"), "kind": "comp", "three_d": true, "transform": {"position": at}, "layers": kids});
    let skeleton = bone("hips", [0.0, -90.0], vec![bone("spine", [0.0, -40.0], vec![bone("head", [0.0, -30.0], vec![])]), bone("leg_l", [-12.0, 5.0], vec![]), bone("leg_r", [12.0, 5.0], vec![])]);
    let marker = json!({"id": format!("a{i}-mark"), "kind": "mesh", "mesh": {"type": "sphere", "radius": 5},
        "transform": {"position": {"keys": [{"at": 0, "v": [0, -200]}, {"at": 1, "v": [100, -200], "ease": "linear"}]}}});
    json!({"id": format!("a{i}"), "kind": "comp", "three_d": true, "transform": {"position": [x, 800], "z": z}, "layers": [skeleton, marker]})
}

pub(crate) fn floor() -> Value {
    json!({"id": "floor", "kind": "mesh", "mesh": {"type": "box", "size": [6000, 40], "depth": 6000}, "transform": {"position": [960, 820]}, "body": {"type": "fixed"}})
}

pub(crate) fn crowd_scene(extra: Vec<Value>, crowd: Value, seed: u64, seconds: f64) -> Project {
    let mut layers = vec![floor()];
    layers.extend((0..6).map(|i| agent(i, 300.0 + i as f64 * 80.0, 0.0)));
    layers.extend(extra);
    let mut c = json!({"id": "walkers", "agents": ["a0", "a1", "a2", "a3", "a4", "a5"], "goals": (0..6).map(|i| json!([1300.0 + i as f64 * 90.0, 260.0])).collect::<Vec<_>>(), "radius": 30, "speed": 160, "stride": 140, "cycle": 1.0});
    for (k, v) in crowd.as_object().unwrap() {
        c[k] = v.clone();
    }
    project(json!({"id": "s1", "simulation": {"seed": seed, "crowds": [c]}, "layers": layers}), seconds)
}

pub(crate) fn at(bake: &Bake, id: &str, k: usize) -> [f64; 3] {
    let s = bake.tracks[bake.track(id).unwrap_or_else(|| panic!("no track for {id}"))].samples[k];
    [s[0] as f64, s[1] as f64, s[2] as f64]
}

pub(crate) fn level(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn world(layers: &[LayerState], id: &str) -> Option<Mat4> {
    for l in layers {
        match &l.content {
            Content::Mesh(m) if &*l.id == id => return Some(m.world),
            Content::Group(g) => {
                if let Some(m) = world(g, id) {
                    return Some(m);
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn agents_walk_to_their_goals_round_each_other_and_a_pillar() {
    // A pillar stands on the straight line from the row to the goals.
    let pillar = json!({"id": "pillar", "kind": "mesh", "mesh": {"type": "box", "size": [90, 300], "depth": 90}, "transform": {"position": [950, 650], "z": 130}, "body": {"type": "fixed"}});
    let mut p = crowd_scene(vec![pillar], json!({}), 1, 12.0);
    let bake = baked(&mut p);
    let n = bake.tracks[bake.track("a0").unwrap()].samples.len();
    assert_eq!((bake.tracks.len(), n), (6, 361), "an agent a track, and no bone's (none fell)");
    for i in 0..6 {
        let id = format!("a{i}");
        // It starts where its keys have it and ends at its goal, on the floor all the way.
        assert!(level(at(&bake, &id, 0), [300.0 + i as f64 * 80.0, 800.0, 0.0]) < 1e-3);
        let end = at(&bake, &id, n - 1);
        assert!(level(end, [1300.0 + i as f64 * 90.0, 0.0, 260.0]) < 30.0, "{id} ended at {end:?}");
        assert!((0..n).all(|k| (at(&bake, &id, k)[1] - 800.0).abs() < 1e-3));
        // Never into the pillar (45 px half across, the agent 30).
        let nearest = (0..n).map(|k| level(at(&bake, &id, k), [950.0, 0.0, 130.0])).fold(f64::INFINITY, f64::min);
        assert!(nearest > 70.0, "{id} came within {nearest:.0} px of the pillar's middle");
        // Nor into each other.
        for j in 0..i {
            let apart = (0..n).map(|k| level(at(&bake, &id, k), at(&bake, &format!("a{j}"), k))).fold(f64::INFINITY, f64::min);
            assert!(apart > 40.0, "a{j} and {id} came within {apart:.0} px");
        }
        // No faster than its own speed (160 px/s, a fifth either way).
        let fastest = (1..n).map(|k| level(at(&bake, &id, k), at(&bake, &id, k - 1)) * 30.0).fold(0.0, f64::max);
        assert!(fastest < 160.0 * 1.2 + 1.0, "{id} reached {fastest:.0} px/s");
    }
    // Some had to go round: a2's path is longer than the straight line.
    let walked: f64 = (1..n).map(|k| level(at(&bake, "a2", k), at(&bake, "a2", k - 1))).sum();
    assert!(walked > level(at(&bake, "a2", 0), at(&bake, "a2", n - 1)) + 15.0);
}

#[test]
fn the_clip_runs_with_the_distance_walked() {
    let mut p = crowd_scene(vec![], json!({}), 1, 6.0);
    let bake = baked(&mut p);
    let t = &bake.tracks[bake.track("a3").unwrap()];
    // A cycle of the clip (1 s) for every stride (140 px), wherever in
    // the cycle the agent started.
    let (mut walked, mut turned) = (0.0, 0.0);
    for k in 1..120 {
        walked += level(at(&bake, "a3", k), at(&bake, "a3", k - 1));
        let d = t.clock[k] as f64 - t.clock[k - 1] as f64;
        turned += if d < -0.5 { d + 1.0 } else { d };
    }
    assert!(walked > 400.0 && (turned - walked / 140.0).abs() < 0.02, "{walked:.0} px walked, the clip {turned:.2} cycles on");
    assert!(t.clock.iter().all(|c| (0.0..=1.0).contains(c)));
    // Drawn: the agent's children are shown at that time of the clip. Its
    // marker (100 px a second of clip) is that far along in the agent.
    let c = Compiled::new(&p);
    for k in [0usize, 31, 77] {
        let frame = c.frame(k as f64 / 30.0);
        let mark = world(&frame.layers, "a3-mark").unwrap();
        let agent = bake.world(bake.track("a3").unwrap(), k as f64 / 30.0).unwrap();
        let want = agent.apply(100.0 * t.clock[k] as f64, -200.0, 0.0);
        assert!((mark.0[3] - want[0]).abs() < 0.05 && (mark.0[7] - want[1]).abs() < 0.05 && (mark.0[11] - want[2]).abs() < 0.05, "sample {k}: the marker is at {:?}, the clip's time puts it at {want:?}", [mark.0[3], mark.0[7], mark.0[11]]);
    }
    // Agents do not walk in step: they start at different places in the cycle.
    let other = &bake.tracks[bake.track("a4").unwrap()];
    assert!((other.clock[0] - t.clock[0]).abs() > 1e-3);
}

#[test]
fn one_struck_hard_goes_down_as_a_ragdoll_and_the_rest_walk_on() {
    // A heavy ball thrown across the row at a3, arriving as they set off.
    let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [545, 700], "z": -150}, "body": {"mass": 30, "velocity": [0, 0, 1500]}});
    // (Not looking: one that sees it coming lunges, and is clipped.)
    let ragdoll = json!({"ragdoll": {"mass": 60, "limits": [{"bone": ""}]}, "dodge": 0});
    let mut p = crowd_scene(vec![ball.clone()], ragdoll, 1, 12.0);
    let bake = baked(&mut p);
    let knocks: Vec<_> = bake.events.iter().filter(|e| e.kind == "knock").collect();
    assert_eq!(knocks.len(), 1, "{:?}", knocks);
    assert_eq!((knocks[0].a.as_str(), knocks[0].b.as_str()), ("a3", "ball"));
    assert!(knocks[0].speed > 1000.0 && knocks[0].t < 0.3, "{:?}", knocks[0]);
    let down = (knocks[0].t * 30.0).ceil() as usize;
    // Its five bones are the simulation's from then, and only its.
    let bones: Vec<&str> = bake.tracks.iter().map(|t| t.id.as_str()).filter(|id| id.contains('-')).collect();
    assert_eq!(bones, ["a3-hips", "a3-spine", "a3-head", "a3-leg_l", "a3-leg_r"]);
    let n = bake.tracks[0].samples.len();
    for id in &bones {
        let t = &bake.tracks[bake.track(id).unwrap()];
        assert!((t.from as usize).abs_diff(down) <= 1, "{id} is baked from sample {} (knocked at {down})", t.from);
        // It ends lying on the floor (the floor's top is at 800), still.
        let (end, before) = (at(&bake, id, n - 1), at(&bake, id, n - 16));
        assert!(end[1] > 740.0 && end[1] < 801.0, "{id} ended at y {}", end[1]);
        assert!(level(end, before) < 3.0);
    }
    // Where the bake takes the bones over they are where the clip had them
    // in the agent: no jump (within what one frame of falling moves them).
    let from = bake.tracks[bake.track("a3-head").unwrap()].from as f64;
    let head = at(&bake, "a3-head", from as usize);
    let agent = at(&bake, "a3", from as usize);
    assert!(level(head, agent) < 40.0 && (agent[1] - head[1] - 160.0).abs() < 30.0, "the head at {head:?} over the agent at {agent:?}");
    // The agent itself stays where it fell; the others reach their goals.
    assert!(level(at(&bake, "a3", n - 1), at(&bake, "a3", down + 1)) < 1e-6);
    for i in [0, 1, 2, 4, 5] {
        assert!(level(at(&bake, &format!("a{i}"), n - 1), [1300.0 + i as f64 * 90.0, 0.0, 260.0]) < 30.0, "a{i}");
    }
    // The ball was turned by the body it hit.
    let ball_end = at(&bake, "ball", n - 1);
    assert!(ball_end[2] < 2500.0, "the ball ended at {ball_end:?}");
    // With no ragdoll for the crowd the one struck stops where it stands.
    let mut plain = crowd_scene(vec![ball], json!({"dodge": 0}), 1, 6.0);
    let b = baked(&mut plain);
    assert_eq!(b.tracks.len(), 7, "six agents and the ball");
    assert!(b.events.iter().any(|e| e.kind == "knock" && e.a == "a3"));
    let m = b.tracks[b.track("a3").unwrap()].samples.len();
    assert!(level(at(&b, "a3", m - 1), at(&b, "a3", 0)) < 120.0);
}

#[test]
fn a_crowd_bakes_to_the_same_bytes_and_a_seed_changes_its_pace() {
    let (a, b, c) = (baked(&mut crowd_scene(vec![], json!({}), 1, 4.0)).to_bytes(), baked(&mut crowd_scene(vec![], json!({}), 1, 4.0)).to_bytes(), baked(&mut crowd_scene(vec![], json!({}), 2, 4.0)).to_bytes());
    assert!(a == b && a != c);
    // What is wrong is named.
    let errors = |crowd: Value| {
        let p = crowd_scene(vec![], crowd, 1, 1.0);
        Compiled::new(&p).sim_input(0, None).1.join("\n")
    };
    assert!(errors(json!({"agents": ["a0", "nobody"]})).contains("crowds.0.agents: no layer 'nobody'"));
    assert!(errors(json!({"agents": ["floor"]})).contains("'floor' is no comp"));
    assert!(errors(json!({"goals": []})).contains("crowds.0.goals"));
}

/// Everything of the agents' paths: the least distance any came to a
/// point on the level, and where each ended.
fn path(bake: &Bake, id: &str) -> Vec<[f64; 3]> {
    (0..bake.tracks[bake.track(id).unwrap()].samples.len()).map(|k| at(bake, id, k)).collect()
}

#[test]
fn agents_find_their_way_round_a_wall_across_it() {
    // A wall 700 px long across the straight line from the row to the
    // goals: the row is before its middle, the goals behind it.
    let wall = json!({"id": "wall", "kind": "mesh", "mesh": {"type": "box", "size": [700, 300], "depth": 40}, "transform": {"position": [500, 650], "z": 400}, "body": {"type": "fixed"}});
    let goals: Vec<Value> = (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 800.0])).collect();
    let mut p = crowd_scene(vec![wall.clone()], json!({"goals": goals}), 1, 16.0);
    let bake = baked(&mut p);
    for i in 0..6 {
        let id = format!("a{i}");
        let way = path(&bake, &id);
        let end = way[way.len() - 1];
        assert!(level(end, [300.0 + i as f64 * 80.0, 0.0, 800.0]) < 30.0, "{id} ended at {end:?}");
        // Never in the wall (it is 150..850 by 380..420; an agent 30 px round).
        for (k, p) in way.iter().enumerate() {
            let inside = p[0] > 150.0 - 22.0 && p[0] < 850.0 + 22.0 && p[2] > 380.0 - 22.0 && p[2] < 420.0 + 22.0;
            assert!(!inside, "{id} is in the wall at sample {k}: {p:?}");
        }
        // Round one of its ends: farther than the 800 px straight through.
        let walked: f64 = way.windows(2).map(|w| level(w[0], w[1])).sum();
        assert!(walked > 870.0 && walked < 2200.0, "{id} walked {walked:.0} px");
        // It kept its pace round the corner (no minute spent against the wall).
        let slowest = way.windows(60).map(|w| level(w[0], w[59])).fold(f64::INFINITY, f64::min);
        let arrived = way.iter().position(|p| level(*p, end) < 60.0).unwrap();
        assert!(arrived < 15 * 30 && (way[..arrived.saturating_sub(60)].windows(60).map(|w| level(w[0], w[59])).fold(f64::INFINITY, f64::min) > 60.0 || slowest > 60.0), "{id} arrived at sample {arrived}");
    }
    assert_eq!(crate::behaviour_tests::pin(&bake), (93270, 6398300622357464177), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
    // They split: some round the left end, some round the right.
    let sides: Vec<bool> = (0..6).map(|i| path(&bake, &format!("a{i}")).iter().any(|p| p[0] > 850.0 && (p[2] - 400.0).abs() < 60.0)).collect();
    assert!(sides.iter().any(|s| *s) && sides.iter().any(|s| !*s), "{sides:?}");
    // Not planning, as before: the wall holds them.
    let mut held = crowd_scene(vec![wall], json!({"goals": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 800.0])).collect::<Vec<_>>(), "plan": false}), 1, 16.0);
    let held = baked(&mut held);
    let through = (0..6).filter(|i| at(&held, &format!("a{i}"), 480)[2] > 420.0).count();
    assert!(through <= 2, "{through} of six got past the wall without a plan");
    // An enclosure with no way out: they go as far as they can and stand.
    let pen: Vec<Value> = [("n", 500.0, -200.0, 900.0, 40.0), ("s", 500.0, 300.0, 900.0, 40.0), ("w", 70.0, 50.0, 40.0, 540.0), ("e", 930.0, 50.0, 40.0, 540.0)]
        .iter()
        .map(|(id, x, z, w, d)| json!({"id": format!("pen-{id}"), "kind": "mesh", "mesh": {"type": "box", "size": [w, 300], "depth": d}, "transform": {"position": [x, 650], "z": z}, "body": {"type": "fixed"}}))
        .collect();
    let mut shut = crowd_scene(pen, json!({"goals": [[500, 900]]}), 1, 8.0);
    let shut = baked(&mut shut);
    for i in 0..6 {
        let end = at(&shut, &format!("a{i}"), 240);
        assert!(end[0] > 90.0 && end[0] < 910.0 && end[2] > -180.0 && end[2] < 280.0, "a{i} left the pen: {end:?}");
    }
}

#[test]
fn an_agent_steps_out_of_the_way_of_what_is_thrown() {
    // A ball thrown along the row's line from 900 px off at a3: half a
    // second and more to see it come.
    let ball = |speed: f64| json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [540, 720], "z": -900}, "body": {"mass": 30, "velocity": [0, -120, speed]}});
    let stay = json!({"goals": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect::<Vec<_>>(), "ragdoll": {"mass": 60, "limits": [{"bone": ""}]}});
    let mut c = stay.clone();
    c["dodge"] = json!(0.0);
    let struck = baked(&mut crowd_scene(vec![ball(1500.0)], c, 1, 4.0));
    assert!(struck.events.iter().any(|e| e.kind == "knock" && e.a == "a3"), "{:?}", struck.events);
    // Looking (0.6 s ahead by default), it is out of the ball's line when
    // the ball passes, and is not knocked down; its neighbours make room.
    let bake = baked(&mut crowd_scene(vec![ball(1500.0)], stay.clone(), 1, 4.0));
    let dodges: Vec<_> = bake.events.iter().filter(|e| e.kind == "dodge").collect();
    assert!(dodges.iter().any(|e| e.a == "a3" && e.b == "ball" && e.speed > 1400.0), "{:?}", bake.events);
    assert!(!bake.events.iter().any(|e| e.kind == "knock"), "{:?}", bake.events.iter().filter(|e| e.kind == "knock").collect::<Vec<_>>());
    assert_eq!(crate::behaviour_tests::pin(&bake), (27810, 5747100466678269218), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
    let passing = (0..120).min_by(|a, b| at(&bake, "ball", *a)[2].abs().total_cmp(&at(&bake, "ball", *b)[2].abs())).unwrap();
    let aside = (at(&bake, "a3", passing)[0] - at(&bake, "ball", passing)[0]).abs();
    assert!(aside > 56.0, "a3 is {aside:.0} px from the ball's line as it passes (sample {passing})");
    // It saw it no sooner than 0.6 s before, and then moved faster than it walks.
    let seen = dodges.iter().find(|e| e.a == "a3").unwrap().t;
    assert!(passing as f64 / 30.0 - seen <= 0.6 + 1.0 / 30.0 && passing as f64 / 30.0 - seen > 0.3, "seen at {seen}, passed at sample {passing}");
    let fastest = (1..=passing).map(|k| level(at(&bake, "a3", k), at(&bake, "a3", k - 1)) * 30.0).fold(0.0, f64::max);
    assert!(fastest > 200.0 && fastest < 160.0 * 1.2 * 2.2 + 1.0, "a3 moved at {fastest:.0} px/s at most");
    // Then it walks back to where it was going.
    assert!(level(at(&bake, "a3", 119), [540.0, 0.0, 0.0]) < 30.0, "a3 ended at {:?}", at(&bake, "a3", 119));
    // A ball too fast to get clear of still knocks it down.
    let fast = baked(&mut crowd_scene(vec![ball(9000.0)], stay, 1, 4.0));
    assert!(fast.events.iter().any(|e| e.kind == "knock" && e.a == "a3"), "{:?}", fast.events);
    // One that will pass overhead, or wide, is not stepped away from.
    let high = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [540, 300], "z": -900}, "body": {"mass": 30, "velocity": [0, -400, 1500]}});
    let wide = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 26}, "transform": {"position": [900, 720], "z": -900}, "body": {"mass": 30, "velocity": [0, -120, 1500]}});
    for other in [high, wide] {
        let b = baked(&mut crowd_scene(vec![other], json!({"goals": (0..6).map(|i| json!([300.0 + i as f64 * 80.0, 0.0])).collect::<Vec<_>>()}), 1, 2.0));
        assert!(!b.events.iter().any(|e| e.kind == "dodge" || e.kind == "knock"), "{:?}", b.events);
    }
}
