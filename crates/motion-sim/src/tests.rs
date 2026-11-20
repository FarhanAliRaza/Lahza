use super::*;
use lahza_motion_core::compile::Content;
use lahza_motion_core::space::Mat4;
use lahza_motion_core::{Compiled, Fps, LayerState, Project, Scene};
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn project(scene: Value, seconds: f64) -> Project {
    let mut p = Project::new(1920, 1080, Fps::whole(30), seconds);
    let scene: Scene = serde_json::from_value(scene).unwrap();
    p.scenes.push(scene);
    p
}

/// Bakes the first scene and puts the bake in the project.
pub(crate) fn baked(p: &mut Project) -> Arc<lahza_motion_core::sim::Bake> {
    p.bakes.clear();
    let c = Compiled::new(p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (bake, _) = bake_scene(&c, 0, None, &mut core_meshes).unwrap();
    let bake = Arc::new(bake);
    p.bakes.insert(p.scenes[0].id.clone(), bake.clone());
    bake
}

fn boxed(id: &str, x: f64, y: f64, body: Value) -> Value {
    json!({"id": id, "kind": "mesh", "mesh": {"type": "box", "size": [100, 100], "depth": 100}, "transform": {"position": [x, y]}, "body": body})
}

/// A mesh layer's world matrix in a frame.
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

fn at(bake: &lahza_motion_core::sim::Bake, id: &str, sample: usize) -> [f32; 7] {
    bake.tracks[bake.track(id).unwrap()].samples[sample]
}

#[test]
fn a_box_falls_and_rests_on_the_ground() {
    let mut p = project(json!({"id": "s1", "simulation": {"ground": {"y": 900}}, "layers": [boxed("b", 960.0, 200.0, json!({}))]}), 3.0);
    let bake = baked(&mut p);
    assert_eq!(bake.tracks.len(), 1);
    assert_eq!(bake.tracks[0].samples.len(), 91);
    // It starts where the keys put it, falls (y down), and comes to rest
    // with its underside on the ground.
    let first = at(&bake, "b", 0);
    assert!((first[0] - 960.0).abs() < 1e-3 && (first[1] - 200.0).abs() < 1e-3 && first[2] == 0.0);
    // Half a second of free fall: ½ · 9.81 m/s² · 100 px/m · 0.25 s².
    let fall = at(&bake, "b", 15)[1] - 200.0;
    assert!((fall - 122.6).abs() < 3.0, "fell {fall} px in 0.5 s");
    let end = at(&bake, "b", 90);
    assert!((end[1] - 850.0).abs() < 1.5, "rests at y {}", end[1]);
    // (A flat landing at 7 m/s tips the box a degree or two on one edge
    // before it settles: it ends a few px to the side.)
    assert!((end[0] - 960.0).abs() < 10.0 && end[2].abs() < 10.0, "{end:?}");
    let before = at(&bake, "b", 80);
    assert!((0..7).all(|k| (end[k] - before[k]).abs() < 0.05), "still moving: {before:?} → {end:?}");
    // What happened: it touched the ground once, at speed.
    let touch: Vec<_> = bake.events.iter().filter(|e| e.kind == "touch").collect();
    assert_eq!((touch[0].a.as_str(), touch[0].b.as_str()), ("b", "ground"));
    assert!(touch[0].speed > 300.0 && (touch[0].t - 1.15).abs() < 0.1, "{:?}", touch[0]);
}

fn chaos(seed: u64, jitter: f64) -> Project {
    let mut layers = vec![json!({"id": "floor", "kind": "mesh", "mesh": {"type": "box", "size": [1600, 40], "depth": 800}, "transform": {"position": [960, 920]}, "body": {"type": "fixed"}})];
    for i in 0..12 {
        layers.push(boxed(&format!("b{i}"), 760.0 + (i % 4) as f64 * 110.0, 840.0 - (i / 4) as f64 * 101.0, json!({})));
    }
    layers.push(json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "transform": {"position": [100, 700]}, "body": {"velocity": [1400, -200, 0], "density": 4}}));
    layers.push(json!({"id": "cone", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 60, "top": 0, "height": 140}, "transform": {"position": [900, 300], "rotation": 20}, "body": {"spin": [0, 0, 90]}}));
    project(json!({"id": "s1", "simulation": {"seed": seed, "jitter": jitter}, "layers": layers}), 4.0)
}

#[test]
fn the_same_project_bakes_to_the_same_bytes() {
    let (a, b) = (baked(&mut chaos(1, 0.0)).to_bytes(), baked(&mut chaos(1, 0.0)).to_bytes());
    assert!(a == b, "two bakes of one project differ");
    // A known result: the same on every machine and build.
    let h = {
        let mut h = lahza_motion_core::sim::Fnv::default();
        h.bytes(&a);
        h.0
    };
    assert_eq!((a.len(), h), (52663, 0x9eaeafe84a8029c5), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
    // Another seed is another take (with jitter); the same seed the same.
    let (j1, j1b, j2) = (baked(&mut chaos(1, 30.0)).to_bytes(), baked(&mut chaos(1, 30.0)).to_bytes(), baked(&mut chaos(2, 30.0)).to_bytes());
    assert!(j1 == j1b && j1 != j2 && j1 != a);
}

#[test]
fn a_stack_comes_to_rest() {
    let mut layers = vec![json!({"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [2000, 2000]}, "transform": {"position": [960, 900], "rotation_x": 90}, "body": {"type": "fixed"}})];
    for i in 0..6 {
        // A little apart, so they drop onto each other.
        layers.push(boxed(&format!("b{i}"), 960.0, 845.0 - i as f64 * 104.0, json!({})));
    }
    let mut p = project(json!({"id": "s1", "layers": layers}), 5.0);
    let bake = baked(&mut p);
    let n = bake.tracks[0].samples.len();
    for i in 0..6 {
        let (end, before) = (at(&bake, &format!("b{i}"), n - 1), at(&bake, &format!("b{i}"), n - 31));
        // The plane is a sheet a px thick each way: its top is at 899.
        let want = 899.0 - 50.0 - i as f32 * 100.0;
        assert!((end[1] - want).abs() < 2.0, "b{i} rests at {} (want {want})", end[1]);
        assert!((end[0] - 960.0).abs() < 2.0 && end[2].abs() < 2.0, "b{i} slid to {:?}", &end[..3]);
        assert!((0..7).all(|k| (end[k] - before[k]).abs() < 0.05), "b{i} still moving in the last second");
    }
}

/// A hinge's angle about z, degrees, from a body's rotation.
fn roll(s: [f32; 7]) -> f64 {
    2.0 * (s[5] as f64).atan2(s[6] as f64).to_degrees()
}

#[test]
fn a_hinge_holds_its_limits_and_its_anchor() {
    // A bar hanging from its top end, pushed hard: it may swing 30° each way.
    let bar = json!({"id": "bar", "kind": "mesh", "mesh": {"type": "box", "size": [40, 300], "depth": 40}, "transform": {"position": [960, 400]},
        "body": {"pushes": [{"at": 0.2, "velocity": [900, 0, 0]}, {"at": 1.5, "velocity": [-1500, 0, 0]}]}});
    let mut p = project(json!({"id": "s1", "simulation": {"joints": [{"type": "hinge", "a": "bar", "anchor": [0, -150, 0], "limits": [-30, 30]}]}, "layers": [bar]}), 4.0);
    let bake = baked(&mut p);
    let (mut lo, mut hi) = (0.0f64, 0.0f64);
    for s in &bake.tracks[0].samples {
        let a = roll(*s);
        (lo, hi) = (lo.min(a), hi.max(a));
        assert!(a.abs() < 31.5, "the hinge let the bar reach {a}°");
        // The top end stays where it was pinned: (960, 250).
        let (sin, cos) = a.to_radians().sin_cos();
        let top = [s[0] as f64 + 150.0 * sin, s[1] as f64 - 150.0 * cos];
        assert!((top[0] - 960.0).abs() < 2.0 && (top[1] - 250.0).abs() < 2.0, "the pin moved to {top:?}");
        assert!(s[2].abs() < 0.5 && s[3].abs() < 0.01 && s[4].abs() < 0.01, "it left the hinge's plane: {s:?}");
    }
    assert!(lo < -25.0 && hi > 25.0, "the pushes should swing it to both limits: {lo}..{hi}");
}

#[test]
fn a_released_body_keeps_the_speed_its_keys_gave_it() {
    // Keyed to the right at 300 px/s, turning 90°/s; let go at 1 s, no gravity.
    let layer = json!({"id": "b", "kind": "mesh", "mesh": {"type": "box", "size": [100, 100], "depth": 100},
        "transform": {"position": {"keys": [{"at": 0, "v": [200, 500]}, {"at": 2, "v": [800, 500], "ease": "linear"}]}, "rotation": {"keys": [{"at": 0, "v": 0}, {"at": 2, "v": 180, "ease": "linear"}]}},
        "body": {"release": 1.0}});
    let mut p = project(json!({"id": "s1", "simulation": {"gravity": [0, 0, 0]}, "layers": [layer]}), 3.0);
    let bake = baked(&mut p);
    assert_eq!(bake.tracks[0].from, 30);
    let (a, b) = (at(&bake, "b", 30), at(&bake, "b", 60));
    assert!((a[0] - 500.0).abs() < 0.01 && (b[0] - 800.0).abs() < 1.0, "x {} → {}", a[0], b[0]);
    assert!((roll(a) - 90.0).abs() < 0.01 && (roll(b) - 180.0).abs() < 1.0, "turned {} → {}", roll(a), roll(b));
    // Drawing: the keys until the release, the bake after, and no jump between.
    let c = Compiled::new(&p);
    let x = |t: f64| world(&c.frame(t).layers, "b").unwrap().0[3];
    assert!((x(0.5) - 350.0).abs() < 1e-6 && (x(1.0) - 500.0).abs() < 0.01 && (x(1.5) - 650.0).abs() < 0.5 && (x(2.5) - 950.0).abs() < 1.5, "{} {} {} {}", x(0.5), x(1.0), x(1.5), x(2.5));
    // Between frames the bake is sampled between its samples.
    assert!((x(1.0 + 0.5 / 30.0) - 505.0).abs() < 0.05);
}

#[test]
fn drawing_reads_the_bake_through_comps_parents_and_scale() {
    // A scaled, turned body inside a moved, turned, scaled comp, and a
    // layer parented to it.
    let layers = json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "box", "size": [3000, 40], "depth": 3000}, "transform": {"position": [960, 1000]}, "body": {"type": "fixed"}},
        {"id": "rig", "kind": "comp", "three_d": true, "transform": {"position": [700, 300], "rotation": 30, "scale": [1.5, 1.5], "rotation_y": 20}, "layers": [
            {"id": "b", "kind": "mesh", "mesh": {"type": "box", "size": [80, 60], "depth": 40}, "transform": {"position": [50, 20], "rotation": 10, "scale": [1.2, 0.8], "anchor": [10, 5]}, "body": {}},
            {"id": "rider", "kind": "mesh", "mesh": {"type": "sphere", "radius": 10}, "parent": "b", "transform": {"position": [0, -60]}}
        ]}
    ]);
    let mut p = project(json!({"id": "s1", "layers": layers}), 3.0);
    let keyed = Compiled::new(&p);
    let start = world(&keyed.frame(0.0).layers, "b").unwrap();
    let bake = baked(&mut p);
    let c = Compiled::new(&p);
    // At its release the layer is exactly where the keys had it.
    let now = world(&c.frame(0.0).layers, "b").unwrap();
    for k in 0..16 {
        assert!((now.0[k] - start.0[k]).abs() < 1e-3, "{k}: {} vs {}", now.0[k], start.0[k]);
    }
    // Later it is where the bake has it, in the scene's space, whatever its comp.
    let t = 2.0;
    let want = bake.world(0, t).unwrap();
    let got = world(&c.frame(t).layers, "b").unwrap();
    for k in 0..16 {
        assert!((got.0[k] - want.0[k]).abs() < 1e-6, "{k}: {} vs {}", got.0[k], want.0[k]);
    }
    assert!(got.0[7] > start.0[7] + 300.0, "it fell: y {} → {}", start.0[7], got.0[7]);
    // What is parented to it rides it.
    let rider = world(&c.frame(t).layers, "rider").unwrap();
    let on = got.apply(0.0, -60.0, 0.0);
    assert!((rider.0[3] - on[0]).abs() < 1e-6 && (rider.0[7] - on[1]).abs() < 1e-6 && (rider.0[11] - on[2]).abs() < 1e-6);
    // A frame is its time alone: asked again, and after other frames, the same.
    let again = Compiled::new(&p);
    again.frame(0.3);
    assert_eq!(again.frame(t), c.frame(t));
}

#[test]
fn a_stale_bake_is_found_and_a_camera_move_is_not_one() {
    let scene = |mass: f64, orbit: f64| json!({"id": "s1", "camera": {"orbit": [orbit, 10]}, "simulation": {"ground": {"y": 900}}, "layers": [boxed("b", 960.0, 200.0, json!({"mass": mass}))]});
    let hash = |p: &Project| Compiled::new(p).sim_input(0, None).0.hash();
    let mut p = project(scene(2.0, 0.0), 2.0);
    let bake = baked(&mut p);
    assert_eq!(bake.hash, hash(&p), "a fresh bake is not stale");
    // With the bake in the project the input is the same: bodies are
    // gathered from their keys, not from the bake.
    assert_eq!(hash(&project(scene(2.0, 0.0), 2.0)), bake.hash);
    assert_ne!(hash(&project(scene(3.0, 0.0), 2.0)), bake.hash, "another mass");
    assert_ne!(hash(&project(scene(2.0, 0.0), 2.5)), bake.hash, "a longer scene");
    assert_eq!(hash(&project(scene(2.0, 40.0), 2.0)), bake.hash, "the camera is not part of it");
    let mut moved = project(scene(2.0, 0.0), 2.0);
    moved.scenes[0].layers[0].common.transform.position = lahza_motion_core::Property::Constant(lahza_motion_core::Vec2::new(900.0, 200.0));
    assert_ne!(hash(&moved), bake.hash, "the body starts elsewhere");
}

#[test]
fn a_kinematic_body_pushes_and_a_blast_throws() {
    // A paddle keyed across the floor shoves a box; later a blast throws it up.
    let layers = json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "box", "size": [3000, 40], "depth": 800}, "transform": {"position": [960, 920]}, "body": {"type": "fixed", "friction": 0.9}},
        {"id": "paddle", "kind": "mesh", "mesh": {"type": "box", "size": [40, 200], "depth": 300},
         "transform": {"position": {"keys": [{"at": 0, "v": [300, 800]}, {"at": 1, "v": [800, 800], "ease": "linear"}]}}, "body": {"type": "kinematic"}},
        boxed("b", 600.0, 850.0, json!({"friction": 0.9}))
    ]);
    let mut p = project(json!({"id": "s1", "simulation": {"blasts": [{"at": 2.0, "origin": [900, 950, 0], "speed": 900, "radius": 400}]}, "layers": layers}), 3.0);
    let bake = baked(&mut p);
    assert_eq!(bake.tracks.len(), 1, "only what the simulation moves is kept");
    let (pushed, thrown) = (at(&bake, "b", 45), at(&bake, "b", 70));
    // The paddle's face reaches x 820; the box's centre is 50 px ahead of it.
    assert!(pushed[0] > 868.0 && pushed[0] < 1100.0, "pushed to x {}", pushed[0]);
    assert!((at(&bake, "b", 59)[1] - 850.0).abs() < 1.5 && thrown[1] < 800.0, "thrown to y {}", thrown[1]);
    assert!(bake.events.iter().any(|e| e.a == "paddle" && e.b == "b" || e.a == "b" && e.b == "paddle"));
}

#[test]
fn flat_bodies_stay_in_the_plane_and_shapes_collide() {
    // Flat layers as bodies: a rect drops onto a tilted fixed rect and slides off.
    let layers = json!([
        {"id": "ramp", "kind": "shape", "shape": "rect", "size": [600, 30], "fill": "#888", "transform": {"position": [960, 700], "rotation": 20}, "body": {"type": "fixed", "friction": 0.05}},
        {"id": "tile", "kind": "shape", "shape": "rect", "size": [120, 80], "fill": "#f00", "transform": {"position": [900, 300], "rotation": 5}, "body": {"friction": 0.05}},
        {"id": "dot", "kind": "shape", "shape": "circle", "size": 60, "fill": "#0f0", "transform": {"position": [1000, 100]}, "body": {"bounce": 0.6}}
    ]);
    let mut p = project(json!({"id": "s1", "simulation": {"flat": true}, "layers": layers}), 3.0);
    let bake = baked(&mut p);
    for t in &bake.tracks {
        for s in &t.samples {
            assert!(s[2].abs() < 1e-4 && s[3].abs() < 1e-5 && s[4].abs() < 1e-5, "{} left the plane: {s:?}", t.id);
        }
    }
    let tile = at(&bake, "tile", 90);
    assert!(tile[0] > 1000.0 && tile[1] > 700.0, "the tile slides down the ramp: {:?}", &tile[..2]);
    // A flat layer is drawn from the bake too (its matrix is 2D).
    let c = Compiled::new(&p);
    let frame = c.frame(1.0);
    let l = frame.layers.iter().find(|l| &*l.id == "tile").unwrap();
    let (x, y) = l.matrix.apply(0.0, 0.0);
    let want = at(&bake, "tile", 30);
    assert!((x - want[0] as f64).abs() < 1e-3 && (y - want[1] as f64).abs() < 1e-3);
}

#[test]
fn problems_are_named() {
    let c = |layers: Value, sim: Value| {
        let p = project(json!({"id": "s1", "simulation": sim, "layers": layers}), 1.0);
        Compiled::new(&p).sim_input(0, None).1.join("\n")
    };
    let e = c(json!([{"id": "t", "kind": "text", "text": "hi", "body": {}}]), json!({}));
    assert!(e.contains("t.body: nothing to collide with"), "{e}");
    let e = c(json!([boxed("b", 0.0, 0.0, json!({}))]), json!({"joints": [{"type": "hinge", "a": "nope"}, {"type": "ball", "a": "b", "b": "b"}, {"type": "ball", "a": "b", "motor": {"target": 3}}]}));
    assert!(e.contains("joints.0.a: no body 'nope'") && e.contains("joints.1: a and b are the same body") && e.contains("joints.2.motor: only hinges and sliders"), "{e}");
    let e = c(json!([boxed("b", 0.0, 0.0, json!({"shape": {"type": "mesh"}}))]), json!({}));
    assert!(e.contains("b.body.shape: \"mesh\""), "{e}");
    // A text layer is a body once it has a shape.
    let e = c(json!([{"id": "t", "kind": "text", "text": "hi", "body": {"shape": {"type": "box", "size": [200, 80, 20]}}}]), json!({}));
    assert!(e.is_empty(), "{e}");
}

#[test]
fn a_motor_drives_a_hinge_to_its_keyed_target() {
    // A door on a hinge about y; its motor's target is keyed 0 → 80°.
    let door = json!({"id": "door", "kind": "mesh", "mesh": {"type": "box", "size": [200, 300], "depth": 20}, "transform": {"position": [960, 500]}, "body": {}});
    let sim = json!({"gravity": [0, 0, 0], "joints": [{"type": "hinge", "a": "door", "anchor": [-100, 0, 0], "axis": [0, 1, 0],
        "motor": {"target": {"keys": [{"at": 0.5, "v": 0}, {"at": 1.5, "v": 80, "ease": "glide"}]}, "stiffness": 400, "damping": 40}}]});
    let mut p = project(json!({"id": "s1", "simulation": sim, "layers": [door]}), 3.0);
    let bake = baked(&mut p);
    let yaw = |s: [f32; 7]| 2.0 * (s[4] as f64).atan2(s[6] as f64).to_degrees();
    assert!(yaw(at(&bake, "door", 10)).abs() < 0.5);
    let end = yaw(at(&bake, "door", 90));
    assert!((end.abs() - 80.0).abs() < 2.0, "the door is at {end}°");
}

#[test]
fn a_rope_is_drawn_from_end_to_end_and_hangs_by_its_slack() {
    use lahza_motion_core::compile::Content;
    // A ball let go beside the point its rope is tied to: 300 px of rope,
    // 100 px apart. It falls till the rope is taut, and swings.
    let scene = |draw: serde_json::Value, kind: &str| {
        json!({"id": "s1", "simulation": {"joints": [{"type": kind, "a": "ball", "anchor_b": [600, 200, 0], "length": 300, "draw": draw}]}, "layers": [
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "transform": {"position": [700, 200]}, "body": {"mass": 5}},
            {"id": "cord", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 3, "height": 50}, "transform": {"position": [100, 900]}},
            {"id": "flat", "kind": "shape", "shape": "rect", "size": [10, 10], "fill": "#fff"}
        ]})
    };
    let mut p = project(scene(json!("cord"), "rope"), 3.0);
    let bake = baked(&mut p);
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    // The rope's pieces at a time: each a (middle, half its length along its y).
    let pieces = |t: f64| -> Vec<([f64; 3], [f64; 3])> {
        c.frame(t)
            .layers
            .iter()
            .filter(|l| &*l.id == "cord" || l.id.starts_with("cord#"))
            .map(|l| {
                let Content::Mesh(m) = &l.content else { panic!() };
                let w = m.world.0;
                // (Its mesh is 50 long: its y column is the piece's length over 50.)
                ([w[3], w[7], w[11]], [w[1] * 25.0, w[5] * 25.0, w[9] * 25.0])
            })
            .collect()
    };
    let ends = |ps: &[([f64; 3], [f64; 3])]| -> ([f64; 3], [f64; 3], f64) {
        let (first, last) = (ps[0], ps[ps.len() - 1]);
        let total = ps.iter().map(|p| 2.0 * (p.1[0] * p.1[0] + p.1[1] * p.1[1] + p.1[2] * p.1[2]).sqrt()).sum();
        (std::array::from_fn(|k| first.0[k] - first.1[k]), std::array::from_fn(|k| last.0[k] + last.1[k]), total)
    };
    let near = |a: [f64; 3], b: [f64; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt();
    let ball = |t: f64| {
        let m = bake.world(bake.track("ball").unwrap(), t).unwrap().0;
        [m[3], m[7], m[11]]
    };
    // At the start, slack: twelve pieces from the ball to the tie, as long
    // together as the rope (the hang is a parabola: within a tenth), their
    // lowest well below both ends. Its layer is not drawn where it lies.
    let slack = pieces(0.0);
    assert_eq!(slack.len(), 12);
    let (from, to, long) = ends(&slack);
    assert!(near(from, ball(0.0)) < 1e-6 && near(to, [600.0, 200.0, 0.0]) < 1e-6, "from {from:?} to {to:?}");
    assert!((long - 300.0).abs() < 30.0, "its pieces are {long:.0} px together");
    let lowest = slack.iter().map(|p| p.0[1]).fold(0.0, f64::max);
    assert!(lowest > 200.0 + 90.0, "it hangs to y {lowest:.0}");
    assert!(slack.iter().all(|p| p.0[1] < 600.0));
    // Each piece begins where the last ended.
    for w in slack.windows(2) {
        let (end, begin): ([f64; 3], [f64; 3]) = (std::array::from_fn(|k| w[0].0[k] + w[0].1[k]), std::array::from_fn(|k| w[1].0[k] - w[1].1[k]));
        assert!(near(end, begin) < 1e-6);
    }
    // Taut (the ball has fallen to the rope's end): one straight piece,
    // the rope's length, from the ball to the tie.
    let taut = (10..90).map(|k| k as f64 / 30.0).find(|t| near(ball(*t), [600.0, 200.0, 0.0]) > 299.0).expect("the rope never came taut");
    let straight = pieces(taut);
    let (from, to, long) = ends(&straight);
    assert!(straight.len() == 1 && near(from, ball(taut)) < 1e-6 && near(to, [600.0, 200.0, 0.0]) < 1e-6 && (long - near(ball(taut), [600.0, 200.0, 0.0])).abs() < 1e-6, "{straight:?}");
    // A spring is drawn straight whatever its length.
    let mut spring = project(scene(json!("cord"), "spring"), 1.0);
    baked(&mut spring);
    assert_eq!(Compiled::new(&spring).frame(0.0).layers.iter().filter(|l| l.id.starts_with("cord")).count(), 1);
    // What cannot be drawn is said.
    let wrong = |draw: &str, kind: &str| Compiled::new(&project(scene(json!(draw), kind), 1.0)).sim_input(0, None).1.join("\n");
    assert!(wrong("nothing", "rope").contains("joints.0.draw: no layer 'nothing'"));
    assert!(wrong("flat", "rope").contains("'flat' is no cylinder, capsule or box mesh"));
    assert!(wrong("cord", "hinge").contains("only ropes and springs are drawn"));
}

#[test]
fn every_copy_of_an_instancer_is_a_body_of_its_own() {
    use lahza_motion_core::compile::Content;
    // Three by two bricks from one layer, the grid tipped 10° and hung over
    // the ground; a ball beside them that is no copy.
    let scene = |layout: serde_json::Value| {
        json!({"id": "s1", "simulation": {"ground": {"y": 800}}, "layers": [
            {"id": "wall", "kind": "instancer", "transform": {"position": [600, 300], "rotation": 10}, "layout": layout,
             "layers": [{"id": "brick", "kind": "mesh", "mesh": {"type": "box", "size": [80, 40], "depth": 40}, "body": {"bounce": 0.1}}]},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 20}, "transform": {"position": [200, 300]}, "body": {}}
        ]})
    };
    let mut p = project(scene(json!({"type": "grid", "columns": 3, "rows": 2, "spacing": [100, 60]})), 4.0);
    let bake = baked(&mut p);
    let ids: Vec<&str> = bake.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["brick#0", "brick#1", "brick#2", "brick#3", "brick#4", "brick#5", "ball"]);
    let place = |id: &str, k: usize| {
        let s = bake.tracks[bake.track(id).unwrap()].samples[k];
        [s[0] as f64, s[1] as f64, s[2] as f64]
    };
    // They start where the grid has its copies (100 px apart along the
    // tipped rows), each turned as the grid is, and fall as six bodies:
    // all on the ground at the end, lying flat, none inside another.
    let apart = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt();
    assert!((apart(place("brick#0", 0), place("brick#1", 0)) - 100.0).abs() < 1e-3 && (apart(place("brick#0", 0), place("brick#3", 0)) - 60.0).abs() < 1e-3);
    assert!((place("brick#1", 0)[1] - place("brick#0", 0)[1] - 100.0 * 10f64.to_radians().sin()).abs() < 1e-3);
    let last = bake.tracks[0].samples.len() - 1;
    for k in 0..6 {
        let end = place(&format!("brick#{k}"), last);
        assert!(end[1] > 740.0 && end[1] < 781.0, "brick#{k} ended at {end:?}");
        assert!(apart(end, place(&format!("brick#{k}"), last - 15)) < 1.0);
        for j in 0..k {
            assert!(apart(end, place(&format!("brick#{j}"), last)) > 39.0, "brick#{j} and brick#{k} ended {:.0} px apart", apart(end, place(&format!("brick#{j}"), last)));
        }
    }
    // Drawn: each copy where the bake has its body, at every time.
    let c = Compiled::new(&p);
    for t in [0.0, 0.7, 3.5] {
        let frame = c.frame(t);
        let drawn: Vec<[f64; 3]> = frame.layers.iter().filter(|l| &*l.id == "brick").map(|l| match &l.content {
            Content::Mesh(m) => [m.world.0[3], m.world.0[7], m.world.0[11]],
            _ => panic!(),
        }).collect();
        assert_eq!(drawn.len(), 6);
        for k in 0..6 {
            let w = bake.world(bake.track(&format!("brick#{k}")).unwrap(), t).unwrap().0;
            assert!(drawn.iter().any(|d| apart(*d, [w[3], w[7], w[11]]) < 1e-6), "at {t}: brick#{k} is not drawn at {:?}", [w[3], w[7], w[11]]);
        }
    }
    // A joint and a push can name a copy.
    let mut tied = scene(json!({"type": "grid", "columns": 3, "rows": 2, "spacing": [100, 60]}));
    tied["simulation"]["joints"] = json!([{"type": "fixed", "a": "brick#4"}]);
    let mut tied = project(tied, 2.0);
    let held = baked(&mut tied);
    let s = &held.tracks[held.track("brick#4").unwrap()].samples;
    assert!((s[s.len() - 1][1] - s[0][1]).abs() < 1.0 && held.tracks[held.track("brick#3").unwrap()].samples[s.len() - 1][1] > 700.0);
    // An emitter's particles are no bodies.
    let e = Compiled::new(&project(scene(json!({"type": "emitter", "rate": 10, "lifetime": 1})), 1.0)).sim_input(0, None).1.join("\n");
    assert!(e.contains("brick.body: a layer inside an emitter"), "{e}");
    assert_eq!(crate::behaviour_tests::pin(&bake), (25353, 2539571057427851888), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}

/// A ball on 400 px of rope tied at (600, 200), let go level with the tie;
/// a bar across its swing when `bar`.
fn roped(links: u32, bar: bool, mass: f64) -> Project {
    let mut layers = vec![
        json!({"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "transform": {"position": [1000, 200]}, "body": {"mass": mass}}),
        json!({"id": "cord", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 4, "height": 50}, "transform": {"position": [100, 900]}}),
    ];
    if bar {
        layers.push(json!({"id": "bar", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 20, "height": 300}, "transform": {"position": [600, 400], "rotation_x": 90}, "body": {"type": "fixed"}}));
    }
    project(json!({"id": "s1", "simulation": {"joints": [{"id": "rope", "type": "rope", "a": "ball", "anchor_b": [600, 200, 0], "length": 400, "draw": "cord", "links": links}]}, "layers": layers}), 4.0)
}

#[test]
fn a_rope_of_links_swings_and_lies_on_what_it_meets() {
    use lahza_motion_core::compile::Content;
    let tie = [600.0, 200.0, 0.0];
    let far = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt();
    // What a take was: the rope's length joint to joint at its longest,
    // how deep a link's middle got into the bar, and the ball's path.
    let take = |links: u32, bar: bool, mass: f64| {
        let mut p = roped(links, bar, mass);
        let bake = baked(&mut p);
        let n = bake.tracks[0].samples.len();
        let pt = |id: &str, k: usize| {
            let s = at(&bake, id, k);
            [s[0] as f64, s[1] as f64, s[2] as f64]
        };
        let (mut longest, mut inside) = (0.0f64, f64::MIN);
        for k in 0..n {
            // (Link 0 is at the ball; a link's middle to the next's is a
            // link's length when the joint between them holds.)
            let mut long = 25.0;
            let mut prev = pt("ball", k);
            for l in 0..links {
                let c = pt(&format!("rope/{l}"), k);
                if l > 0 {
                    long += far(prev, c);
                }
                prev = c;
                inside = inside.max(24.0 - (c[0] - 600.0).hypot(c[1] - 400.0));
            }
            longest = longest.max(long);
            assert!(far(pt("ball", k), tie) < 401.0, "the rope let the ball {:.1} px from its tie", far(pt("ball", k), tie));
        }
        let ball: Vec<[f64; 3]> = (0..n).map(|k| pt("ball", k)).collect();
        (p, bake, longest, inside, ball)
    };
    // Nothing in its way: it swings through to the other side, and the
    // links stay end to end (the rope is 400 px; 16 links of 25).
    let (p, bake, longest, _, ball) = take(16, false, 5.0);
    assert!(ball.iter().any(|b| b[0] < 300.0), "it swings to the left of its tie");
    assert!(longest < 404.0, "its links came apart: {longest:.1} px joint to joint");
    // It is drawn link by link, from the ball to the tie, each piece
    // beginning where the last ended.
    let c = Compiled::new(&p);
    for t in [0.0, 0.5, 1.3] {
        let frame = c.frame(t);
        let pieces: Vec<([f64; 3], [f64; 3])> = frame.layers.iter().filter(|l| l.id.starts_with("cord")).map(|l| {
            let Content::Mesh(m) = &l.content else { panic!() };
            let w = m.world.0;
            ([w[3], w[7], w[11]], [w[1] * 25.0, w[5] * 25.0, w[9] * 25.0])
        }).collect();
        assert_eq!(pieces.len(), 16);
        let m = bake.world(bake.track("ball").unwrap(), t).unwrap().0;
        let (begin, end): ([f64; 3], [f64; 3]) = (std::array::from_fn(|k| pieces[0].0[k] - pieces[0].1[k]), std::array::from_fn(|k| pieces[15].0[k] + pieces[15].1[k]));
        assert!(far(begin, [m[3], m[7], m[11]]) < 1e-6 && far(end, tie) < 1e-6, "at {t}: from {begin:?} to {end:?}");
        for w in pieces.windows(2) {
            let (end, begin): ([f64; 3], [f64; 3]) = (std::array::from_fn(|k| w[0].0[k] + w[0].1[k]), std::array::from_fn(|k| w[1].0[k] - w[1].1[k]));
            assert!(far(end, begin) < 1e-6);
        }
        // A link's piece lies on its link (the middle link, mid-swing).
        let link = bake.world(bake.track("rope/8").unwrap(), t).unwrap().0;
        assert!(far(pieces[8].0, [link[3], link[7], link[11]]) < 2.0, "at {t}");
    }
    // A bar across its swing, 200 px under the tie: the rope lies on it
    // and the ball is wound round it, where the drawn curve (no links)
    // swings on through.
    let (_, wound, longest, inside, ball) = take(16, true, 5.0);
    assert!(inside < 1.0, "a link is {inside:.1} px inside the bar");
    assert!(longest < 404.0, "{longest:.1}");
    let end = ball[ball.len() - 1];
    assert!(far(end, [600.0, 400.0, 0.0]) < 120.0 && ball[45..].iter().all(|b| b[0] > 450.0), "the ball ends at {end:?}");
    let through = baked(&mut roped(0, true, 5.0));
    assert!(through.tracks.len() == 1 && through.tracks[0].samples.iter().any(|s| s[0] < 300.0));
    // A heavier ball presses the rope into the bar: 50 kg by 2 px, 500 kg
    // by 12 (a link is light beside it, and the solver's passes are few).
    let (_, _, _, inside, _) = take(16, true, 50.0);
    assert!(inside < 4.0, "{inside:.1}");
    // Only ropes have links.
    let mut spring = serde_json::to_value(&roped(4, false, 5.0).scenes[0]).unwrap();
    spring["simulation"]["joints"][0]["type"] = json!("spring");
    assert!(Compiled::new(&project(spring, 1.0)).sim_input(0, None).1.join("\n").contains("joints.0.links: only a rope is made of links"));
    assert_eq!(crate::behaviour_tests::pin(&wound), (61580, 11416658259355396517), "the bake changed: if the simulator or the format did, bakes are stale and this number is the new one");
}
