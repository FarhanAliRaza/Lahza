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
    for (name, controller) in [("search 16 0.5s", json!({"type": "search", "horizon": 0.5})), ("search 8 0.5s", json!({"type": "search", "horizon": 0.5, "samples": 8}))] {
        for speed in [1000.0, 1400.0, 1800.0] {
            let (bake, stats) = pushed(speed, controller.clone(), json!([]), 4.0);
            println!("{name:16} push {speed:4}: hips drop {:6.1} px; bake {:.2} s ({} steps, {} tried, {:.2} s in tries)", drop(&bake), stats.seconds, stats.steps, stats.tried_steps, stats.search_seconds);
        }
    }
}

#[test]
fn trying_ahead_keeps_it_up_where_holding_the_clip_does_not() {
    // Motors holding the clip and nothing else: a 300 px/s shove at the
    // chest topples it. Trying sixteen sets of leans a quarter second
    // ahead, ten times a second, it stays up, with no force helping.
    let (statue, plain) = pushed(300.0, Value::Null, json!([]), 4.0);
    assert!(drop(&statue) > 150.0, "holding the clip, its hips came down {:.0} px", drop(&statue));
    let (kept, stats) = pushed(300.0, json!({"type": "search"}), json!([]), 4.0);
    assert!(drop(&kept) < 10.0, "trying ahead, its hips came down {:.0} px", drop(&kept));
    // What it costs: the steps of every try, on top of the take's own.
    assert_eq!(stats.steps, plain.steps);
    assert!(stats.tried_steps > 20 * stats.steps, "{} steps tried for {} taken", stats.tried_steps, stats.steps);
    // The same take again (its tries are seeded).
    let (again, _) = pushed(300.0, json!({"type": "search"}), json!([]), 4.0);
    assert!(again.to_bytes() == kept.to_bytes());
    // It has its limit too.
    let (lost, _) = pushed(700.0, json!({"type": "search"}), json!([]), 4.0);
    assert!(drop(&lost) > 150.0);
}
