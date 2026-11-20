use crate::ragdoll_tests::{figure_scene, track_pose as pose};
use crate::{bake_with, core_meshes, policy, Stats, Take};
use lahza_motion_core::sim::{quat_mul, Bake};
use lahza_motion_core::Compiled;
use serde_json::{json, Value};

fn fixture(src: &str) -> Result<Vec<u8>, String> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/policy").join(src)).map_err(|e| format!("{src}: {e}"))
}

/// The figure floating (no gravity) at full strength with a controller.
fn floating(controller: Value) -> Result<(Bake, Stats), String> {
    let mut doll = json!({"strength": 1, "limits": [{"bone": "fore", "swing": 120}]});
    if !controller.is_null() {
        doll["controller"] = controller;
    }
    let p = figure_scene(json!({}), doll, json!({"gravity": [0, 0, 0], "ground": {"y": 3000}}), 1.5);
    let c = Compiled::new(&p);
    let (input, errors) = c.sim_input(0, None);
    assert!(errors.is_empty(), "{errors:?}");
    bake_with(&input, &mut core_meshes, &fixture)
}

fn elbow(bake: &Bake, k: usize) -> f64 {
    let rel = quat_mul(pose(bake, "upper_l", k).inverse().q, pose(bake, "fore_l", k).q);
    2.0 * rel[2].atan2(rel[3]).to_degrees()
}

#[test]
fn the_observation_is_laid_out_as_written() {
    let p = figure_scene(json!({}), json!({"strength": 1}), json!({}), 1.0);
    let c = Compiled::new(&p);
    let (input, _) = c.sim_input(0, None);
    let take = Take::new(&input, &mut core_meshes).unwrap();
    let r = &input.ragdolls[0];
    let feet = take.brains[0].feet().to_vec();
    assert_eq!(feet, vec![12, 15]);
    assert_eq!((policy::observation_len(r, feet.len()), policy::action_len(r)), (183, 45));
    let o = policy::observe(&take.world, &input, r, &feet, 0.0);
    assert_eq!(o.len(), 183);
    let near = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
    // Standing still: up is −y in the root's frame, nothing moves, the
    // hips are 2.25 m over the ankles (225 px at 100 px a metre), nothing
    // else is about.
    assert!(near(&o[0..3], &[0.0, -1.0, 0.0]) && near(&o[3..9], &[0.0; 6]) && near(&o[9..10], &[2.25]) && near(&o[10..16], &[0.0; 6]), "{:?}", &o[..16]);
    // Each bone: its turn (none), its turning (none), the clip's turn (none).
    for k in 0..15 {
        assert!(near(&o[16 + k * 11..27 + k * 11], &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]), "bone {}: {:?}", k + 1, &o[16 + k * 11..27 + k * 11]);
    }
    // An action's three numbers per bone are a turn in its parent's frame.
    let mut action = vec![0.0f32; 45];
    action[(5 - 1) * 3 + 2] = 1.0;
    action[0] = 9.0;
    let turns = policy::turns(r, &action);
    assert!(turns[0].is_none());
    let q = turns[5].unwrap();
    assert!((q[2] - 0.5f64.sin()).abs() < 1e-6 && (q[3] - 0.5f64.cos()).abs() < 1e-6 && turns[4] == Some([0.0, 0.0, 0.0, 1.0]));
    // (No part of a turn is more than `REACH`.)
    assert!((turns[1].unwrap()[0] - (policy::REACH / 2.0).sin()).abs() < 1e-6);
}

#[test]
fn a_policy_file_sets_the_motors() {
    // Zeros leave the clip as it is.
    let (plain, _) = floating(Value::Null).unwrap();
    let (zeros, stats) = floating(json!({"type": "policy", "src": "zeros.onnx"})).unwrap();
    assert_eq!(stats.policy_runs, 45, "once a frame");
    for id in ["hips", "fore_l", "hand_r", "foot_l"] {
        let (a, b) = (pose(&plain, id, 45).p, pose(&zeros, id, 45).p);
        assert!((0..3).all(|k| (a[k] - b[k]).abs() < 1.0), "{id}: {a:?} vs {b:?}");
    }
    // A constant action turns its joint by that much: 0.8 rad on the forearm.
    let (bend, _) = floating(json!({"type": "policy", "src": "bend.onnx"})).unwrap();
    assert!((elbow(&bend, 45) - 0.8f64.to_degrees()).abs() < 2.5, "the elbow is at {:.1}° (0.8 rad is 45.8°)", elbow(&bend, 45));
    assert!(elbow(&zeros, 45).abs() < 1.0);
    // What it reads reaches it: half the root's height (2.25 m) as radians.
    let (height, _) = floating(json!({"type": "policy", "src": "height.onnx"})).unwrap();
    assert!((elbow(&height, 45) - 1.125f64.to_degrees()).abs() < 3.5, "the elbow is at {:.1}° (1.125 rad is 64.5°)", elbow(&height, 45));
    // A file for another body is refused, with the counts.
    let e = floating(json!({"type": "policy", "src": "wrong.onnx"})).unwrap_err();
    assert!(e.contains("wrong.onnx") && e.contains("gives 7 numbers") && e.contains("takes 45"), "{e}");
    let e = floating(json!({"type": "policy", "src": "none.onnx"})).unwrap_err();
    assert!(e.contains("none.onnx"), "{e}");
}

/// Standing on box feet, pushed at the chest at 0.5 s.
fn pushed(speed: f64, controller: Value, behaviours: Value, seconds: f64) -> (Bake, Stats) {
    let mut doll = json!({"strength": 1, "behaviours": behaviours,
        "shapes": [{"bone": "foot", "shape": {"type": "box", "size": [44, 24, 130], "offset": [0, 12, -20]}}],
        "limits": [{"bone": "shin", "swing": 130, "hinge": [-1, 0, 0]}, {"bone": "thigh", "swing": 70}],
        "hits": [{"at": 0.5, "bone": "chest", "velocity": [speed, 0, 0]}]});
    if !controller.is_null() {
        doll["controller"] = controller;
    }
    let p = figure_scene(json!({}), doll, json!({"ground": {"y": 809.5, "friction": 0.9}}), seconds);
    let (input, errors) = Compiled::new(&p).sim_input(0, None);
    assert!(errors.is_empty(), "{errors:?}");
    bake_with(&input, &mut core_meshes, &fixture).unwrap()
}

fn drop(bake: &Bake) -> f64 {
    let n = bake.tracks[0].samples.len();
    pose(bake, "hips", n - 1).p[1] - pose(bake, "hips", 0).p[1]
}

/// `cargo test --release -p lahza-motion-sim probe_search -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_search() {
    for threads in ["1", "2", "4"] {
        std::env::set_var("LAHZA_SIM_THREADS", threads);
        let (bake, stats) = pushed(700.0, json!({"type": "search", "horizon": 0.5}), json!([]), 4.0);
        println!("{threads} threads: hips drop {:.1} px; bake {:.2} s ({:.2} s in tries on {} threads), {:x}", drop(&bake), stats.seconds, stats.search_seconds, stats.search_threads, crate::behaviour_tests::pin(&bake).1);
    }
    std::env::remove_var("LAHZA_SIM_THREADS");
    for (name, controller) in [("search 0.25s", json!({"type": "search", "horizon": 0.25})), ("search", json!({"type": "search"})), ("search 32", json!({"type": "search", "samples": 32})), ("search 1s", json!({"type": "search", "horizon": 1.0}))] {
        for speed in [300.0, 450.0, 700.0, 1000.0, 1400.0, 1800.0] {
            let (bake, stats) = pushed(speed, controller.clone(), json!([]), 4.0);
            let strides = bake.events.iter().filter(|e| e.kind == "step").count();
            println!("{name:12} push {speed:4}: hips drop {:6.1} px, {strides} steps; bake {:.2} s ({} steps, {} tried, {:.2} s in tries on {} threads)", drop(&bake), stats.seconds, stats.steps, stats.tried_steps, stats.search_seconds, stats.search_threads);
        }
    }
}

#[test]
fn trying_ahead_keeps_it_up_where_holding_the_clip_does_not() {
    // Motors holding the clip and nothing else: a 300 px/s shove at the
    // chest topples it. Trying sixteen plans half a second ahead, ten
    // times a second, it stays up, with no force on its body.
    let (statue, plain) = pushed(300.0, Value::Null, json!([]), 4.0);
    assert!(drop(&statue) > 150.0, "holding the clip, its hips came down {:.0} px", drop(&statue));
    let (kept, stats) = pushed(300.0, json!({"type": "search"}), json!([]), 4.0);
    assert!(drop(&kept) < 10.0, "trying ahead, its hips came down {:.0} px", drop(&kept));
    // What it costs: the steps of every try, on top of the take's own.
    assert_eq!(stats.steps, plain.steps);
    assert!(stats.tried_steps > 40 * stats.steps, "{} steps tried for {} taken", stats.tried_steps, stats.steps);
    // Three times as hard a shove: it steps (a plan may have the foot
    // behind carried to where it is falling to), swings its arms, and
    // stands at the end, 60 px and more from where it stood.
    let (stepped, _) = pushed(1000.0, json!({"type": "search"}), json!([]), 4.0);
    let strides: Vec<&str> = stepped.events.iter().filter(|e| e.kind == "step").map(|e| e.a.as_str()).collect();
    assert!(drop(&stepped) < 10.0 && !strides.is_empty() && strides.iter().all(|f| f.starts_with("foot")), "its hips came down {:.0} px, steps {strides:?}", drop(&stepped));
    let n = stepped.tracks[0].samples.len();
    let moved = pose(&stepped, "hips", n - 1).p[0] - pose(&stepped, "hips", 0).p[0];
    assert!(moved > 60.0, "it ended {moved:.0} px along");
    let swing = |bake: &Bake, k: usize| {
        let (chest, arm) = (pose(bake, "chest", k).q, pose(bake, "upper_l", k).q);
        let d = lahza_motion_core::sim::quat_mul([-chest[0], -chest[1], -chest[2], chest[3]], arm);
        let rest = {
            let (c, a) = (pose(bake, "chest", 0).q, pose(bake, "upper_l", 0).q);
            lahza_motion_core::sim::quat_mul([-c[0], -c[1], -c[2], c[3]], a)
        };
        (2.0 * (0..4).map(|i| d[i] * rest[i]).sum::<f64>().abs().min(1.0).acos()).to_degrees()
    };
    let most = (0..n).map(|k| swing(&stepped, k)).fold(0.0, f64::max);
    assert!(most > 25.0, "its upper arm swung {most:.0}° from where it hung at most");
    // The same take again (its tries are seeded), and the same on one
    // core as on all of them.
    let (again, on_all) = pushed(1000.0, json!({"type": "search"}), json!([]), 4.0);
    assert!(again.to_bytes() == stepped.to_bytes());
    std::env::set_var("LAHZA_SIM_THREADS", "1");
    let (alone, on_one) = pushed(1000.0, json!({"type": "search"}), json!([]), 4.0);
    std::env::remove_var("LAHZA_SIM_THREADS");
    assert!(alone.to_bytes() == stepped.to_bytes() && on_one.search_threads == 1 && on_one.tried_steps == on_all.tried_steps);
    assert_eq!(crate::behaviour_tests::pin(&stepped), (56831, 7627719729510559245), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
    // It has its limit too.
    let (lost, _) = pushed(1800.0, json!({"type": "search"}), json!([]), 4.0);
    assert!(drop(&lost) > 150.0);
}

#[test]
fn knocked_down_under_the_search_it_gets_up_and_is_kept_up_again() {
    // Shoved past what trying ahead can hold, with `get_up` and nothing
    // else: it falls, lies still, rises (as `get_up` does it: helped),
    // and from then the search has it again.
    let (bake, stats) = pushed(1800.0, json!({"type": "search"}), json!([{"type": "get_up"}]), 9.0);
    let events: Vec<(&str, f64)> = bake.events.iter().filter(|e| ["fall", "get_up", "rise"].contains(&e.kind.as_str())).map(|e| (e.kind.as_str(), e.t)).collect();
    assert!(events.len() == 3 && events[0].0 == "fall" && events[1].0 == "get_up" && events[2].0 == "rise", "{events:?}");
    let (fell, up) = (events[0].1, events[2].1);
    assert!(fell < 2.5 && up > fell + 2.0 && up < 7.5, "{events:?}");
    let hips = |k: usize| pose(&bake, "hips", k).p[1];
    let lowest = (0..bake.tracks[0].samples.len()).map(hips).fold(f64::MIN, f64::max);
    assert!(lowest > hips(0) + 150.0, "it was down: its hips at y {lowest:.0}");
    // Up again, and kept up to the end with no behaviour to balance it.
    let n = bake.tracks[0].samples.len();
    for k in [((up + 0.5) * 30.0) as usize, n - 1] {
        assert!((hips(k) - hips(0)).abs() < 20.0, "at {:.1} s its hips are {:.0} px from where they stood", k as f64 / 30.0, hips(k) - hips(0));
    }
    // (While it is down nothing is tried: tries are for the time it stands.)
    let standing = fell - 0.0 + (9.0 - up);
    assert!((stats.tried_steps as f64) < 16.0 * 0.5 * 240.0 * 10.0 * (standing + 0.3), "{} steps tried", stats.tried_steps);
    assert_eq!(crate::behaviour_tests::pin(&bake), (126115, 13679216537685136859), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}
