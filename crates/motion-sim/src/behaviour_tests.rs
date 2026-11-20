use crate::ragdoll_tests::{figure_scene, track_pose as pose};
use crate::tests::baked;
use lahza_motion_core::sim::Bake;
use serde_json::{json, Value};

/// The figure standing at full strength on box feet, pushed at the chest
/// at 0.5 s with `push` px/s, with these behaviours.
fn pushed(push: [f64; 3], behaviours: Value, seconds: f64) -> Bake {
    let doll = json!({"strength": 1, "behaviours": behaviours,
        "shapes": [{"bone": "foot", "shape": {"type": "box", "size": [44, 24, 130], "offset": [0, 12, -20]}}],
        "limits": [{"bone": "shin", "swing": 130, "hinge": [-1, 0, 0]}, {"bone": "thigh", "swing": 70}, {"bone": "upper", "swing": 120, "twist": 60}, {"bone": "fore", "swing": 140, "hinge": [1, 0, 0]}],
        "hits": [{"at": 0.5, "bone": "chest", "velocity": push}]});
    let mut p = figure_scene(json!({}), doll, json!({"ground": {"y": 809.5, "friction": 0.9}}), seconds);
    (*baked(&mut p)).clone()
}

fn fell(bake: &Bake) -> Option<f64> {
    bake.events.iter().find(|e| e.kind == "fall").map(|e| e.t)
}

fn hips_drop(bake: &Bake) -> f64 {
    let n = bake.tracks[0].samples.len();
    pose(bake, "hips", n - 1).p[1] - pose(bake, "hips", 0).p[1]
}

fn steps(bake: &Bake) -> usize {
    bake.events.iter().filter(|e| e.kind == "step").count()
}

#[test]
fn leaning_at_the_ankles_holds_a_shove_that_topples_a_statue() {
    // Motors at full strength and nothing else: it stands as a statue
    // does, and a 200 px/s shove at the chest topples it.
    // (Slowly: it is down in six seconds.)
    let statue = pushed([200.0, 0.0, 0.0], json!([]), 8.0);
    assert!(hips_drop(&statue) > 80.0, "the statue's hips came down {:.0} px", hips_drop(&statue));
    assert!(hips_drop(&pushed([120.0, 0.0, 0.0], json!([]), 5.0)) < 5.0, "a lighter shove leaves it standing");
    // Balance with no help (no force on the hips): the ankles alone hold
    // that shove and one half as hard again, and not twice it.
    let lean = json!([{"type": "balance", "params": {"assist": 0}}]);
    let held = pushed([200.0, 0.0, 0.0], lean.clone(), 5.0);
    assert!(hips_drop(&held) < 5.0 && fell(&held).is_none(), "leaning, its hips came down {:.0} px", hips_drop(&held));
    let more = pushed([300.0, 0.0, 0.0], lean.clone(), 6.0);
    assert!(hips_drop(&more) < 5.0 && fell(&more).is_none());
    let lost = pushed([400.0, 0.0, 0.0], lean, 6.0);
    assert!(fell(&lost).is_some() && hips_drop(&lost) > 80.0);
}

#[test]
fn balance_holds_a_push_within_its_help_and_falls_soft_past_it() {
    let balance = json!([{"type": "balance"}]);
    let held = pushed([1400.0, 0.0, 0.0], balance.clone(), 5.0);
    assert!(hips_drop(&held) < 6.0 && fell(&held).is_none(), "hips down {:.0} px, fell {:?}", hips_drop(&held), fell(&held));
    // It ends about where it stood, all but still.
    let n = held.tracks[0].samples.len();
    let (end, before) = (pose(&held, "hips", n - 1), pose(&held, "hips", n - 31));
    assert!((end.p[0] - 960.0).abs() < 30.0 && (end.p[0] - before.p[0]).abs() < 6.0, "hips at x {} (a second before: {})", end.p[0], before.p[0]);
    // Sideways and from behind too.
    for push in [[0.0, 0.0, 900.0], [-900.0, 0.0, -600.0]] {
        let b = pushed(push, balance.clone(), 5.0);
        assert!(hips_drop(&b) < 8.0 && fell(&b).is_none(), "pushed {push:?}: hips down {:.0} px", hips_drop(&b));
    }
    // Past its help (a quarter of its weight) it goes over, and says when.
    let lost = pushed([1800.0, 0.0, 0.0], balance, 5.0);
    let at = fell(&lost).expect("a fall");
    assert!(at > 0.6 && at < 3.5 && hips_drop(&lost) > 150.0, "fell at {at}, hips down {:.0} px", hips_drop(&lost));
    // Fallen, it goes soft: a statue that fell lies in its pose, this one
    // does not (its knees and hips have given).
    let n = lost.tracks[0].samples.len();
    let bend = |bake: &Bake, a: &str, b: &str| {
        let (p, q) = (pose(bake, a, n - 1).q, pose(bake, b, n - 1).q);
        2.0 * (0..4).map(|k| p[k] * q[k]).sum::<f64>().abs().min(1.0).acos().to_degrees()
    };
    let statue = pushed([1800.0, 0.0, 0.0], json!([]), 5.0);
    let soft: f64 = [("thigh_l", "shin_l"), ("thigh_r", "shin_r"), ("hips", "thigh_l"), ("hips", "thigh_r"), ("chest", "upper_l"), ("chest", "upper_r")].iter().map(|(a, b)| bend(&lost, a, b)).sum();
    let stiff: f64 = [("thigh_l", "shin_l"), ("thigh_r", "shin_r"), ("hips", "thigh_l"), ("hips", "thigh_r"), ("chest", "upper_l"), ("chest", "upper_r")].iter().map(|(a, b)| bend(&statue, a, b)).sum();
    assert!(soft > stiff + 10.0, "joints bent {soft:.0}° in all, fallen soft; {stiff:.0}° as a statue");
}

#[test]
fn a_step_keeps_what_balance_alone_loses() {
    let both = json!([{"type": "balance"}, {"type": "step"}]);
    let kept = pushed([1800.0, 0.0, 0.0], both.clone(), 5.0);
    assert!(fell(&kept).is_none() && hips_drop(&kept) < 8.0, "fell {:?}, hips down {:.0} px", fell(&kept), hips_drop(&kept));
    assert!(steps(&kept) >= 2, "{} steps", steps(&kept));
    // It stepped the way it was pushed: both feet are well along +x.
    let n = kept.tracks[0].samples.len();
    for foot in ["foot_l", "foot_r"] {
        let moved = pose(&kept, foot, n - 1).p[0] - pose(&kept, foot, 0).p[0];
        assert!(moved > 120.0, "{foot} moved {moved:.0} px");
        // And is on the ground again.
        assert!((pose(&kept, foot, n - 1).p[1] - pose(&kept, foot, 0).p[1]).abs() < 6.0);
    }
    // A light shove needs no step.
    assert_eq!(steps(&pushed([300.0, 0.0, 0.0], both.clone(), 4.0)), 0);
    // Steps have their limit as well.
    assert!(fell(&pushed([3200.0, 0.0, 0.0], both, 5.0)).is_some());
}

#[test]
#[ignore]
fn probe_pushes() {
    for (name, behaviours) in [("balance", json!([{"type": "balance"}])), ("balance + step", json!([{"type": "balance"}, {"type": "step"}])), ("no help + step", json!([{"type": "balance", "params": {"assist": 0}}, {"type": "step"}]))] {
        for speed in [700.0, 1000.0, 1400.0, 1800.0, 2400.0, 3200.0] {
            let bake = pushed([speed, 0.0, 0.0], behaviours.clone(), 5.0);
            let n = bake.tracks[0].samples.len();
            let steps = bake.events.iter().filter(|e| e.kind == "step").count();
            println!("{name:18} push {speed:4}: hips drop {:6.1} px, x {:6.1}, fall {:?}, steps {steps}", hips_drop(&bake), pose(&bake, "hips", n - 1).p[0] - 960.0, fell(&bake));
        }
        let bake = pushed([0.0, 0.0, 250.0], behaviours.clone(), 5.0);
        println!("{name:18} push z 250: hips drop {:6.1} px, fall {:?}", hips_drop(&bake), fell(&bake));
    }
}

/// The figure standing balanced, with these behaviours and hits, and a
/// ball keyed past (or at) it.
fn acting(behaviours: Value, hits: Value, ball: Option<Value>, seconds: f64) -> Bake {
    let mut all = vec![json!({"type": "balance"})];
    all.extend(behaviours.as_array().unwrap().iter().cloned());
    let doll = json!({"strength": 1, "behaviours": all, "hits": hits,
        "shapes": [{"bone": "foot", "shape": {"type": "box", "size": [44, 24, 130], "offset": [0, 12, -20]}}],
        "limits": [{"bone": "shin", "swing": 130, "hinge": [-1, 0, 0]}, {"bone": "thigh", "swing": 70}, {"bone": "upper", "swing": 179, "twist": 179}, {"bone": "fore", "swing": 150, "hinge": [1, 0, 0]}]});
    let mut p = figure_scene(json!({}), doll, json!({"ground": {"y": 809.5, "friction": 0.9}}), seconds);
    if let Some(ball) = ball {
        p.scenes[0].layers.push(serde_json::from_value(ball).unwrap());
    }
    (*baked(&mut p)).clone()
}

fn apart(bake: &Bake, a: &str, b: [f64; 3], k: usize) -> f64 {
    let p = pose(bake, a, k).p;
    (0..3).map(|i| (p[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn wrist(bake: &Bake, side: &str, k: usize) -> [f64; 3] {
    pose(bake, &format!("hand_{side}"), k).apply([0.0, 21.0, 0.0])
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

#[test]
fn arms_go_about_the_head_and_come_down_again() {
    let bake = acting(json!([{"type": "protect", "from": 0.5, "to": 1.5}]), json!([]), None, 3.0);
    let head = |k: usize| pose(&bake, "head", k).apply([0.0, -24.0, 0.0]);
    // Hanging, the hands are 211 px from the head's middle; half a second
    // after the behaviour starts both are at its sides, and stay.
    assert!(dist(wrist(&bake, "l", 10), head(10)) > 200.0);
    for k in [30, 36, 44] {
        for side in ["l", "r"] {
            let d = dist(wrist(&bake, side, k), head(k));
            assert!(d < 48.0, "hand_{side} is {d:.0} px from the head at sample {k}");
        }
        // Each on its own side.
        assert!(wrist(&bake, "l", k)[0] < head(k)[0] && wrist(&bake, "r", k)[0] > head(k)[0]);
    }
    // After it ends they hang again, and it is still standing.
    for side in ["l", "r"] {
        assert!(dist(wrist(&bake, side, 85), head(85)) > 190.0);
    }
    assert!(fell(&bake).is_none() && hips_drop(&bake) < 6.0);
}

#[test]
fn a_hand_goes_to_where_it_was_hit_and_lets_go() {
    let hit = json!([{"at": 0.5, "bone": "chest", "velocity": [-150, 0, 0]}]);
    let bake = acting(json!([{"type": "reach", "params": {"hold": 1.5}}]), hit.clone(), None, 3.0);
    let chest = |b: &Bake, k: usize| pose(b, "chest", k).apply([0.0, -40.0, 0.0]);
    let nearest = |b: &Bake, k: usize| dist(wrist(b, "l", k), chest(b, k)).min(dist(wrist(b, "r", k), chest(b, k)));
    assert!(nearest(&bake, 14) > 140.0, "before the hit the hands hang");
    // Within half a second a hand is on the chest, and holds there.
    for k in [30, 36, 45] {
        assert!(nearest(&bake, k) < 25.0, "a hand is {:.0} px from the hit at sample {k}", nearest(&bake, k));
    }
    // One hand: the other still hangs.
    let other = dist(wrist(&bake, "l", 36), chest(&bake, 36)).max(dist(wrist(&bake, "r", 36), chest(&bake, 36)));
    assert!(other > 130.0);
    // 1.5 s after the hit it has let go.
    assert!(nearest(&bake, 75) > 130.0, "{}", nearest(&bake, 75));
    // Without the behaviour nothing reaches.
    let plain = acting(json!([]), hit, None, 3.0);
    assert!(nearest(&plain, 36) > 140.0);
    // A hit on an arm is reached for with the other one.
    let bake = acting(json!([{"type": "reach"}]), json!([{"at": 0.5, "bone": "fore_r", "velocity": [0, 0, 150]}]), None, 2.0);
    let fore = pose(&bake, "fore_r", 36).apply([0.0, 35.0, 0.0]);
    assert!(dist(wrist(&bake, "l", 36), fore) < 45.0, "the left hand is {:.0} px from the right forearm", dist(wrist(&bake, "l", 36), fore));
}

#[test]
fn a_hand_meets_what_is_coming() {
    // A ball keyed in from the right at chest height, stopping in reach.
    let ball = json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 20}, "body": {"type": "kinematic"},
        "transform": {"position": {"keys": [{"at": 0.3, "v": [1700, 430]}, {"at": 1.5, "v": [1120, 430], "ease": "linear"}]}}});
    let guard = acting(json!([{"type": "guard", "params": {"target": "ball"}}]), json!([]), Some(ball.clone()), 3.0);
    let plain = acting(json!([]), json!([]), Some(ball), 3.0);
    let at = [1120.0, 430.0, 0.0];
    // Far off, nothing; as it comes within reach of `within` the nearer
    // (right) hand comes up to it, and is at it when it stops.
    assert!(dist(wrist(&guard, "r", 12), wrist(&plain, "r", 12)) < 6.0, "at 0.4 s the ball is too far to raise a hand");
    let (with, without) = (dist(wrist(&guard, "r", 60), at), dist(wrist(&plain, "r", 60), at));
    // (The ball is solid, 20 px across its half, and the hand stops on it.)
    assert!(with < 100.0 && without > 150.0, "the wrist is {with:.0} px from the ball's middle (unguarded: {without:.0})");
    assert!(wrist(&guard, "r", 60)[1] < 480.0 && wrist(&plain, "r", 60)[1] > 540.0, "the hand is up at the ball's height");
    assert!(wrist(&guard, "l", 60)[1] > 540.0, "the other hand stays down");
    assert!(fell(&guard).is_none());
}

#[test]
#[ignore]
fn probe_arms() {
    for (name, b, bone) in [("protect", json!([{"type": "protect", "from": 0.5}]), "thigh_r"), ("reach", json!([{"type": "reach"}]), "chest"), ("nothing", json!([]), "chest")] {
        let bake = acting(b, json!([{"at": 0.5, "bone": bone, "velocity": [-150, 0, 0]}]), None, 2.5);
        for k in [0, 16, 18, 20, 22, 24, 27, 30, 36, 45, 60, 74] {
            let head = pose(&bake, "head", k).apply([0.0, -24.0, 0.0]);
            let chest = pose(&bake, "chest", k).apply([0.0, -40.0, 0.0]);
            let wrist = |s: &str| pose(&bake, &format!("hand_{s}"), k).apply([0.0, 21.0, 0.0]);
            let d = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt();
            println!("{name:8} {k:2}: hands to head {:5.0} {:5.0}; to chest {:5.0} {:5.0}; hand_l {:?} hand_r {:?} head {:?}", d(wrist("l"), head), d(wrist("r"), head), d(wrist("l"), chest), d(wrist("r"), chest), wrist("l").map(|v| v.round()), wrist("r").map(|v| v.round()), head.map(|v| v.round()));
        }
    }
}

/// The figure knocked over at 0.5 s, with balance and get_up.
fn risen(push: [f64; 3], params: Value, seconds: f64) -> Bake {
    pushed(push, json!([{"type": "balance"}, {"type": "get_up", "params": params}]), seconds)
}

/// (len, FNV) of a bake's bytes.
pub(crate) fn pin(bake: &Bake) -> (usize, u64) {
    let bytes = bake.to_bytes();
    let mut h = lahza_motion_core::sim::Fnv::default();
    h.bytes(&bytes);
    (bytes.len(), h.0)
}

fn happened(bake: &Bake) -> Vec<(&str, f64, f64)> {
    bake.events.iter().filter(|e| ["fall", "get_up", "rise"].contains(&e.kind.as_str())).map(|e| (e.kind.as_str(), e.t, e.speed)).collect()
}

#[test]
fn it_gets_up_after_a_fall_whichever_way_it_fell() {
    // Knocked flat from the left, the right, the front and the back.
    for push in [[3200.0, 0.0, 0.0], [-3200.0, 0.0, 0.0], [0.0, 0.0, 3200.0], [0.0, 0.0, -3200.0]] {
        let bake = risen(push, json!({}), 10.0);
        let events = happened(&bake);
        let kinds: Vec<&str> = events.iter().map(|e| e.0).collect();
        assert_eq!(kinds, ["fall", "get_up", "rise"], "push {push:?}: {events:?}");
        let (fall, start, rise) = (events[0], events[1], events[2]);
        // It lies there first (0.8 s still), and is up 2.4 s or so later.
        assert!(start.1 - fall.1 > 0.8 && rise.1 - start.1 >= 2.39 && rise.1 - start.1 < 3.9, "push {push:?}: {events:?}");
        // On the floor in between: its hips 200 px lower than they stood.
        let low = (0..bake.tracks[0].samples.len()).map(|k| pose(&bake, "hips", k).p[1] - pose(&bake, "hips", 0).p[1]).fold(0.0, f64::max);
        assert!(low > 190.0, "push {push:?}: its hips dropped {low:.0} px");
        // Up means up: its hips where they stood (within 16 px), its chest
        // over them (leaning less as it settles), from the rise to the end.
        let n = bake.tracks[0].samples.len();
        for k in ((rise.1 * 30.0) as usize + 30..n).step_by(10) {
            let (hips, chest) = (pose(&bake, "hips", k).p, pose(&bake, "chest", k).p);
            assert!((hips[1] - pose(&bake, "hips", 0).p[1]).abs() < 16.0, "push {push:?}, sample {k}: hips at y {}", hips[1]);
            let lean = ((chest[0] - hips[0]).powi(2) + (chest[2] - hips[2]).powi(2)).sqrt();
            assert!(lean < 45.0 && chest[1] < hips[1] - 40.0, "push {push:?}, sample {k}: chest {chest:?} over hips {hips:?}");
        }
        let sway = dist(pose(&bake, "hips", n - 1).p, pose(&bake, "hips", n - 31).p);
        assert!(sway < 12.0, "push {push:?}: its hips moved {sway:.1} px in the last second");
        // What lifted it: the outside force carried a fifth to two fifths
        // of its weight over the rise (never more than `help`, 0.6).
        assert!(rise.2 >= 10.0 && rise.2 <= 45.0, "push {push:?}: the force carried {} % of its weight", rise.2);
    }
    // Without the force it does not get up (it tries, and lies again).
    let alone = risen([3200.0, 0.0, 0.0], json!({"help": 0}), 10.0);
    let kinds: Vec<&str> = happened(&alone).iter().map(|e| e.0).collect();
    assert!(kinds.len() >= 3 && kinds[..2] == ["fall", "get_up"] && !kinds.contains(&"rise"), "{kinds:?}");
    assert!(pose(&alone, "hips", alone.tracks[0].samples.len() - 1).p[1] - pose(&alone, "hips", 0).p[1] > 60.0);
    // With a fifth of its weight it still does, later.
    let little = risen([3200.0, 0.0, 0.0], json!({"help": 0.2}), 10.0);
    let usual = risen([3200.0, 0.0, 0.0], json!({}), 10.0);
    let (little, usual) = (happened(&little), happened(&usual));
    assert!(little.last().unwrap().0 == "rise" && little.last().unwrap().1 > usual.last().unwrap().1 && little.last().unwrap().2 < usual.last().unwrap().2, "{little:?} {usual:?}");
    // Without the behaviour it stays down.
    assert_eq!(happened(&pushed([3200.0, 0.0, 0.0], json!([{"type": "balance"}]), 6.0)).iter().map(|e| e.0).collect::<Vec<_>>(), ["fall"]);
    // `wait` and `time` are its own.
    let slow = risen([3200.0, 0.0, 0.0], json!({"wait": 2.0, "time": 3.5}), 12.0);
    let slow = happened(&slow);
    assert!(slow.len() == 3 && slow[1].1 - slow[0].1 > 2.0 && slow[2].1 - slow[1].1 >= 3.49, "{slow:?}");
    // The same take twice, to the byte.
    let (a, b) = (risen([3200.0, 0.0, 0.0], json!({}), 10.0), risen([3200.0, 0.0, 0.0], json!({}), 10.0));
    assert_eq!(a.to_bytes(), b.to_bytes());
    assert_eq!(pin(&a), (139438, 3354145111831044034), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

#[test]
#[ignore]
fn explore_getting_up() {
    for push in [[3200.0, 0.0, 0.0], [-3200.0, 0.0, 0.0], [0.0, 0.0, 3200.0], [0.0, 0.0, -3200.0]] {
        for help in [1.0, 0.6, 0.4, 0.2, 0.0] {
            let bake = risen(push, json!({"help": help}), 12.0);
            let n = bake.tracks[0].samples.len();
            let heights: Vec<String> = (0..n).step_by(30).map(|k| format!("{:.0}", pose(&bake, "hips", 0).p[1] - pose(&bake, "hips", k).p[1])).collect();
            println!("push {push:?} help {help}: {:?}\n   hips rise by second: {}", happened(&bake), heights.join(" "));
        }
    }
}
