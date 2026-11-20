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
