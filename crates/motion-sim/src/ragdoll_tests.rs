use crate::tests::{baked, project};
use lahza_motion_core::sim::{quat_mul, Bake, Pose};
use lahza_motion_core::{Compiled, Project};
use serde_json::{json, Value};

thread_local! {
    /// Transform fields for bones by id (a clip, for the figure built next).
    static KEYS: std::cell::RefCell<Vec<(String, Value)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn bone(id: &str, at: [f64; 2], children: Vec<Value>) -> Value {
    let mut transform = json!({"position": at});
    KEYS.with(|k| {
        for (_, fields) in k.borrow().iter().filter(|(b, _)| b == id) {
            for (name, v) in fields.as_object().unwrap() {
                transform[name] = v.clone();
            }
        }
    });
    json!({"id": id, "kind": "comp", "three_d": true, "transform": transform, "layers": children})
}

/// The figure built next has these transform fields on bone `id`.
fn keyed(id: &str, fields: Value) {
    KEYS.with(|k| k.borrow_mut().push((id.to_string(), fields)));
}

/// A figure of nested comps (no meshes): hips at (960, 560), 225 px of
/// leg under them, 180 px of trunk and head above, arms hanging.
pub(crate) fn figure(root: Value) -> Value {
    let leg = |s: &str, x: f64| bone(&format!("thigh_{s}"), [x, 10.0], vec![bone(&format!("shin_{s}"), [0.0, 110.0], vec![bone(&format!("foot_{s}"), [0.0, 105.0], vec![])])]);
    let arm = |s: &str, x: f64| bone(&format!("upper_{s}"), [x, -65.0], vec![bone(&format!("fore_{s}"), [0.0, 75.0], vec![bone(&format!("hand_{s}"), [0.0, 70.0], vec![])])]);
    let mut hips = bone("hips", [960.0, 560.0], vec![bone("spine", [0.0, -30.0], vec![bone("chest", [0.0, -70.0], vec![bone("head", [0.0, -80.0], vec![]), arm("l", -50.0), arm("r", 50.0)])]), leg("l", -25.0), leg("r", 25.0)]);
    for (k, v) in root.as_object().unwrap() {
        hips[k] = v.clone();
    }
    hips
}

const BONES: [&str; 16] = ["hips", "spine", "chest", "head", "upper_l", "fore_l", "hand_l", "upper_r", "fore_r", "hand_r", "thigh_l", "shin_l", "foot_l", "thigh_r", "shin_r", "foot_r"];
const PARENT: [&str; 16] = ["", "hips", "spine", "chest", "chest", "upper_l", "fore_l", "chest", "upper_r", "fore_r", "hips", "thigh_l", "shin_l", "hips", "thigh_r", "shin_r"];

pub(crate) fn figure_scene(root: Value, ragdoll: Value, sim: Value, seconds: f64) -> Project {
    scene(root, ragdoll, sim, seconds)
}

fn scene(root: Value, ragdoll: Value, sim: Value, seconds: f64) -> Project {
    let mut sim = sim;
    let mut r = ragdoll;
    r["id"] = json!("doll");
    r["root"] = json!("hips");
    sim["ragdolls"] = json!([r]);
    if sim.get("ground").is_none() {
        sim["ground"] = json!({"y": 850});
    }
    let p = project(json!({"id": "s1", "simulation": sim, "layers": [figure(root)]}), seconds);
    KEYS.with(|k| k.borrow_mut().clear());
    p
}

pub(crate) fn track_pose(bake: &Bake, id: &str, sample: usize) -> Pose {
    pose(bake, id, sample)
}

fn pose(bake: &Bake, id: &str, sample: usize) -> Pose {
    let s = bake.tracks[bake.track(id).unwrap_or_else(|| panic!("no track for {id}"))].samples[sample];
    Pose { p: [s[0] as f64, s[1] as f64, s[2] as f64], q: [s[3] as f64, s[4] as f64, s[5] as f64, s[6] as f64] }
}

/// Degrees between two rotations.
fn apart(a: [f64; 4], b: [f64; 4]) -> f64 {
    let d: f64 = (0..4).map(|k| a[k] * b[k]).sum::<f64>().abs().min(1.0);
    2.0 * d.acos().to_degrees()
}

#[test]
fn a_limp_figure_falls_in_one_piece_and_comes_to_rest() {
    let mut p = scene(json!({}), json!({}), json!({}), 6.0);
    let c = Compiled::new(&p);
    let (input, errors) = c.sim_input(0, None);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(input.ragdolls[0].bones.len(), 16);
    assert_eq!(input.bodies.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(), BONES);
    // 70 kg, shared by volume: the trunk's bones are the heavy ones.
    let mass: f64 = input.ragdolls[0].bones.iter().map(|b| b.mass).sum();
    assert!((mass - 70.0).abs() < 1e-6 && input.ragdolls[0].bones[2].mass > input.ragdolls[0].bones[6].mass * 4.0);
    // (And the hips no lighter than three quarters of a thigh.)
    assert!(input.ragdolls[0].bones[0].mass >= input.ragdolls[0].bones[10].mass * 0.75 - 1e-9);
    let bake = baked(&mut p);
    let n = bake.tracks[0].samples.len();
    assert_eq!((bake.tracks.len(), n), (16, 181));
    // It starts as it stood.
    let hips = pose(&bake, "hips", 0);
    assert!((hips.p[0] - 960.0).abs() < 1e-3 && (hips.p[1] - 560.0).abs() < 1e-3);
    let foot = pose(&bake, "foot_l", 0);
    assert!((foot.p[0] - 935.0).abs() < 1e-3 && (foot.p[1] - 785.0).abs() < 1e-3, "{:?}", foot.p);
    // It ends on the ground: the hips have come down from 290 px above it
    // to lying height, nothing is under the floor, nothing still moves.
    let hips = pose(&bake, "hips", n - 1);
    assert!(hips.p[1] > 740.0, "the hips are still up at y {}", hips.p[1]);
    for id in BONES {
        let (end, before) = (pose(&bake, id, n - 1), pose(&bake, id, n - 16));
        assert!(end.p[1] < 851.0, "{id} is under the floor at y {}", end.p[1]);
        let moved = (0..3).map(|k| (end.p[k] - before.p[k]).abs()).fold(0.0, f64::max);
        assert!(moved < 2.0, "{id} still moves {moved} px in the last half second");
    }
    // In one piece: every bone stays at its joint on its parent, all the way.
    let c2 = Compiled::new(&p);
    let (input, _) = c2.sim_input(0, None);
    let bones = &input.ragdolls[0].bones;
    for k in 0..n {
        for (b, id) in BONES.iter().enumerate().skip(1) {
            let joint = pose(&bake, PARENT[b], k).apply(bones[b].anchor_parent);
            let at = pose(&bake, id, k).p;
            let gap = (0..3).map(|a| (joint[a] - at[a]).powi(2)).sum::<f64>().sqrt();
            assert!(gap < 4.0, "{id} is {gap:.1} px off its joint at sample {k}");
        }
    }
}

#[test]
fn joints_bend_no_further_than_their_limits() {
    // Thrown hard and spinning, so every joint is driven to its limits.
    // Knees are hinges that bend one way.
    let limits = json!([{"bone": "shin", "swing": 120, "hinge": [1, 0, 0]}, {"bone": "fore", "swing": 60, "twist": 10}, {"bone": "head", "swing": 25, "twist": 15}]);
    let mut p = scene(json!({}), json!({"limits": limits, "hits": [{"at": 0.1, "bone": "head", "velocity": [900, -500, 300]}, {"at": 0.6, "bone": "foot_l", "velocity": [-1200, -900, -400]}]}), json!({}), 4.0);
    let bake = baked(&mut p);
    let (input, _) = Compiled::new(&p).sim_input(0, None);
    let bones = &input.ragdolls[0].bones;
    let n = bake.tracks[0].samples.len();
    let mut most = vec![0.0f64; 16];
    for k in 0..n {
        for (b, id) in BONES.iter().enumerate().skip(1) {
            let rel = quat_mul(pose(&bake, PARENT[b], k).inverse().q, pose(&bake, id, k).q);
            most[b] = most[b].max(apart(rel, bones[b].bind));
        }
    }
    for (b, id) in BONES.iter().enumerate().skip(1) {
        // Swing and twist together can reach a little past either alone.
        let allowed = (bones[b].swing.powi(2) * if bones[b].hinge.is_some() { 1.0 } else { 2.0 } + if bones[b].hinge.is_some() { 0.0 } else { bones[b].twist.powi(2) }).sqrt() + 6.0;
        assert!(most[b] <= allowed, "{id} bent {:.1}° (its limits allow {allowed:.1}°)", most[b]);
    }
    // And the hits did drive them: the head and a forearm reached theirs.
    assert!(most[3] > 20.0 && most[5].max(most[8]) > 40.0, "head {:.1}°, forearms {:.1}° {:.1}°", most[3], most[5], most[8]);
    // A knee bends about its axle only, and only one way.
    for (b, id) in [(11usize, "shin_l"), (14, "shin_r")] {
        let mut lo = 0.0f64;
        for k in 0..n {
            let rel = quat_mul(pose(&bake, PARENT[b], k).inverse().q, pose(&bake, id, k).q);
            // The bind is the identity here: the turn about x, and what is not about x.
            let about_x = 2.0 * rel[0].atan2(rel[3]).to_degrees();
            lo = lo.min(about_x);
            let off_axis = (rel[1] * rel[1] + rel[2] * rel[2]).sqrt().asin().to_degrees() * 2.0;
            assert!(off_axis < 4.0, "{id} turned {off_axis:.1}° off its axle at sample {k}");
            assert!(about_x < 124.0, "{id} bent {about_x:.1}°");
        }
        assert!(lo > -4.0, "{id} bent {lo:.1}° the wrong way");
    }
}

#[test]
fn it_leaves_its_animation_with_the_pose_and_speed_it_had() {
    // Walking right at 300 px/s with an arm raised by keys; let go at 0.5 s
    // in no gravity: it drifts on at that speed, the arm as it was.
    let root = json!({"transform": {"position": {"keys": [{"at": 0, "v": [400, 560]}, {"at": 1, "v": [700, 560], "ease": "linear"}]}}});
    keyed("upper_l", json!({"rotation": 70}));
    let mut p = scene(root, json!({"release": 0.5, "rest": 0.0}), json!({"gravity": [0, 0, 0], "ground": {"y": 2000}}), 1.5);
    let bake = baked(&mut p);
    assert!(bake.tracks.iter().all(|t| t.from == 15));
    let (at, later) = (pose(&bake, "hips", 15), pose(&bake, "hips", 30));
    assert!((at.p[0] - 550.0).abs() < 0.01, "released at x {}", at.p[0]);
    assert!((later.p[0] - 700.0).abs() < 4.0, "half a second on it is at x {} (300 px/s would be 700)", later.p[0]);
    // The raised arm is still raised right after (70° about z against the chest).
    let rel = quat_mul(pose(&bake, "chest", 16).inverse().q, pose(&bake, "upper_l", 16).q);
    let raised = 2.0 * rel[2].atan2(rel[3]).to_degrees();
    assert!((raised - 70.0).abs() < 3.0, "the arm is at {raised}°");
    // Drawn: the keys until the release, the bake from it, the same place at it.
    let c = Compiled::new(&p);
    let hips = |t: f64| c.keyed_world(0, "hips", t).unwrap().0[3];
    assert!((hips(0.25) - 475.0).abs() < 1e-6);
}

#[test]
fn a_ragdoll_bakes_to_the_same_bytes() {
    let make = || scene(json!({}), json!({"strength": 0.3, "hits": [{"at": 0.3, "bone": "chest", "velocity": [500, 0, 200]}]}), json!({}), 2.0);
    let (a, b) = (baked(&mut make()).to_bytes(), baked(&mut make()).to_bytes());
    assert!(a == b, "two bakes of one ragdoll differ");
    let mut h = lahza_motion_core::sim::Fnv::default();
    h.bytes(&a);
    assert_eq!((a.len(), h.0), (30237, 0xf179add661f3dd20), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

#[test]
fn motors_hold_the_clip_and_strength_lets_go() {
    // No gravity. The clip raises the left forearm 80° over a second.
    let build = |strength: Value| {
        keyed("fore_l", json!({"rotation": {"keys": [{"at": 0.2, "v": 0}, {"at": 1.2, "v": 80, "ease": "glide"}]}}));
        scene(json!({}), json!({"strength": strength, "limits": [{"bone": "fore", "swing": 100}]}), json!({"gravity": [0, 0, 0], "ground": {"y": 3000}}), 2.0)
    };
    let elbow = |bake: &Bake, k: usize| {
        let rel = quat_mul(pose(bake, "upper_l", k).inverse().q, pose(bake, "fore_l", k).q);
        2.0 * rel[2].atan2(rel[3]).to_degrees()
    };
    // At full strength the elbow follows the clip closely, and the rest
    // of the body keeps its pose.
    let mut p = build(json!(1.0));
    let bake = baked(&mut p);
    let c = Compiled::new(&p);
    let (input, _) = c.sim_input(0, None);
    assert!((elbow(&bake, 60) - 80.0).abs() < 3.0, "the elbow is at {:.1}° (the clip: 80°)", elbow(&bake, 60));
    // Through the move (30° in its first two frames) it is within a few
    // degrees of the clip.
    for k in [8, 12, 14, 21, 30, 45] {
        let clip = input.ragdolls[0].bones[5].clip[k];
        let want = 2.0 * clip[2].atan2(clip[3]).to_degrees();
        assert!((elbow(&bake, k) - want).abs() < 5.0, "at sample {k} the elbow is at {:.1}° (the clip: {want:.1}°)", elbow(&bake, k));
    }
    for (b, id) in BONES.iter().enumerate().skip(1) {
        if *id == "fore_l" {
            continue;
        }
        let rel = quat_mul(pose(&bake, PARENT[b], 60).inverse().q, pose(&bake, id, 60).q);
        assert!(apart(rel, input.ragdolls[0].bones[b].bind) < 3.0, "{id} left its pose by {:.1}°", apart(rel, input.ragdolls[0].bones[b].bind));
    }
    // Limp, nothing raises it.
    let limp = baked(&mut build(json!(0.0)));
    assert!(elbow(&limp, 60).abs() < 3.0, "limp, the elbow is at {:.1}°", elbow(&limp, 60));
    // Strength is keyed: let go at 1.3 s, the arm's motor stops holding;
    // a limb's own strength scales the body's.
    let mut p = build(json!(1.0));
    let mut sim = serde_json::to_value(p.scenes[0].simulation.as_ref().unwrap()).unwrap();
    sim["ragdolls"][0]["strengths"] = json!([{"bone": "upper_l", "strength": 0.0}]);
    p.scenes[0].simulation = Some(serde_json::from_value(sim).unwrap());
    let one_arm = baked(&mut p);
    assert!(elbow(&one_arm, 60).abs() < 3.0, "its arm limp, the elbow is at {:.1}°", elbow(&one_arm, 60));
}

#[test]
#[ignore]
fn probe() {
    let mut p = scene(json!({}), json!({}), json!({}), 4.0);
    let bake = baked(&mut p);
    let (input, _) = Compiled::new(&p).sim_input(0, None);
    let bones = &input.ragdolls[0].bones;
    for (b, id) in BONES.iter().enumerate() {
        let bi = &input.bodies[bones[b].body];
        println!("{id}: along {:?} len {:.0} shape {:?} mass {:.1}", bones[b].along.map(|v| (v * 100.0).round() / 100.0), bones[b].length, bi.colliders[0].shape, bones[b].mass);
    }
    for k in (0..121).step_by(10) {
        let mut line = format!("{k}:");
        for (b, id) in BONES.iter().enumerate() {
            let q = pose(&bake, id, k);
            if b == 0 {
                line += &format!(" hips ({:.0},{:.0},{:.0})", q.p[0], q.p[1], q.p[2]);
            } else if [1, 2, 3, 5, 11].contains(&b) {
                let rel = quat_mul(pose(&bake, PARENT[b], k).inverse().q, q.q);
                line += &format!(" {id} {:.0}°", apart(rel, bones[b].bind));
            }
        }
        println!("{line}");
    }
}
