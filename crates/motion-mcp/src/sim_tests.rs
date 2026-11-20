use crate::tools::Toolbox;
use lahza_motion_render::Backend;
use serde_json::{json, Value};

fn toolbox(name: &str) -> (Toolbox, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lahza-motion-sim-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    tb.backend = Backend::Cpu;
    (tb, dir)
}

fn sim_issues(report: &Value) -> Vec<String> {
    report["issues"].as_array().unwrap().iter().filter(|i| i["kind"] == "simulation").map(|i| i["message"].as_str().unwrap().to_string()).collect()
}

#[test]
fn check_names_a_missing_and_a_stale_bake_and_bake_clears_them() {
    let (mut tb, dir) = toolbox("bake");
    tb.call("project_create", json!({"path": "drop", "duration": 2, "width": 640, "height": 360})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1", "simulation": {"ground": {"y": 330}}},
        {"op": "add", "id": "crate", "kind": "mesh", "mesh": {"type": "box", "size": [60, 60], "depth": 60}, "transform": {"position": [320, 60], "rotation": 12}, "body": {"bounce": 0.3}},
        {"op": "add", "id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "transform": {"position": [330, -40]}, "body": {}}
    ]}))
    .unwrap();
    // Not baked: check says so, and the bodies stand where their keys put them.
    let report = tb.call("check", json!({})).unwrap().json;
    let issues = sim_issues(&report);
    assert!(report["ok"] == false && issues.len() == 1 && issues[0].contains("2 bodies and no bake"), "{report:#}");
    let baked = tb.call("sim_bake", json!({})).unwrap().json;
    let s = &baked["scenes"][0];
    assert_eq!((s["status"].as_str(), s["bodies"].as_u64(), s["tracks"].as_u64(), s["frames"].as_u64()), (Some("baked"), Some(2), Some(2), Some(60)), "{baked:#}");
    assert!(s["events"].as_array().unwrap().iter().any(|e| e["a"] == "crate" && e["b"] == "ground"), "{baked:#}");
    let file = dir.join("drop.lahza/sim/s1.bake");
    let first = std::fs::read(&file).unwrap();
    assert!(sim_issues(&tb.call("check", json!({})).unwrap().json).is_empty());
    // Asked again, nothing is stepped; forced, the same bytes come out.
    assert_eq!(tb.call("sim_bake", json!({})).unwrap().json["scenes"][0]["status"], "up to date");
    tb.call("sim_bake", json!({"force": true})).unwrap();
    assert!(std::fs::read(&file).unwrap() == first, "a second bake of the same project differs");
    // An edit to what the simulation depends on makes the bake stale; one
    // to what it does not (a colour) leaves it.
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "crate.material", "value": {"color": "#ff0000"}}]})).unwrap();
    assert!(sim_issues(&tb.call("check", json!({})).unwrap().json).is_empty(), "a colour is not part of the simulation");
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "crate.body.bounce", "value": 0.8}]})).unwrap();
    let issues = sim_issues(&tb.call("check", json!({})).unwrap().json);
    assert!(issues.len() == 1 && issues[0].contains("its bake is stale"), "{issues:?}");
    assert_eq!(tb.call("sim_bake", json!({})).unwrap().json["scenes"][0]["status"], "baked");
    assert!(sim_issues(&tb.call("check", json!({})).unwrap().json).is_empty());
    assert!(std::fs::read(&file).unwrap() != first);
    // Another seed is another take (with jitter), kept in the document.
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "s1.simulation.jitter", "value": 40}]})).unwrap();
    tb.call("sim_bake", json!({})).unwrap();
    let take1 = std::fs::read(&file).unwrap();
    tb.call("sim_bake", json!({"seed": 7})).unwrap();
    assert!(std::fs::read(&file).unwrap() != take1);
    let scene: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("drop.lahza/scenes/s1.json")).unwrap()).unwrap();
    assert_eq!(scene["simulation"]["seed"], 7, "{scene:#}");
    // Undo brings the old seed back, and with it a stale bake.
    tb.call("history", json!({"undo": 1})).unwrap();
    assert!(sim_issues(&tb.call("check", json!({})).unwrap().json)[0].contains("stale"));
    // A project opened fresh reads the bake from its folder.
    tb.call("sim_bake", json!({})).unwrap();
    let mut other = Toolbox::new();
    other.cwd = dir.clone();
    other.backend = Backend::Cpu;
    other.call("project_open", json!({"path": "drop.lahza"})).unwrap();
    assert!(sim_issues(&other.call("check", json!({})).unwrap().json).is_empty());
    // With its last body gone a scene keeps no bake.
    tb.call("edit_apply", json!({"ops": [{"op": "remove", "id": "crate"}, {"op": "remove", "id": "ball"}]})).unwrap();
    tb.call("sim_bake", json!({})).unwrap();
    assert!(!file.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_snapshot_shows_the_baked_frame_whatever_was_asked_before() {
    let (mut tb, dir) = toolbox("scrub");
    tb.call("project_create", json!({"path": "drop", "duration": 2, "width": 320, "height": 180})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1", "background": "#101010", "simulation": {"ground": {"y": 170}}},
        {"op": "add", "id": "crate", "kind": "mesh", "mesh": {"type": "box", "size": [40, 40], "depth": 40}, "material": {"preset": "flat", "color": "#ffffff"}, "transform": {"position": [160, 20]}, "body": {}}
    ]}))
    .unwrap();
    let shot = |tb: &mut Toolbox, t: f64| {
        let out = tb.call("snapshot", json!({"at": [t], "width": 320})).unwrap();
        image::load_from_memory(&out.images[0]).unwrap().to_rgba8()
    };
    // Where the white box is: the mean row of bright pixels.
    let row = |img: &image::RgbaImage| {
        let (mut sum, mut n) = (0.0, 0.0);
        for (_, y, p) in img.enumerate_pixels() {
            if p[0] > 200 {
                sum += y as f64;
                n += 1.0;
            }
        }
        sum / n
    };
    let unbaked = row(&shot(&mut tb, 1.5));
    tb.call("sim_bake", json!({})).unwrap();
    let late = shot(&mut tb, 1.5);
    let early = shot(&mut tb, 0.2);
    // (Rows are in the snapshot's pixels; the camera's perspective is the same for both.)
    assert!(row(&late) > unbaked + 60.0, "baked, the box has fallen: row {} vs {unbaked}", row(&late));
    assert!(row(&early) < row(&late) - 50.0);
    // Scrubbing back and forth gives the same pictures.
    assert!(shot(&mut tb, 1.5) == late && shot(&mut tb, 0.2) == early);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_skinned_model_is_imported_with_its_joints_and_bends_with_them() {
    let (mut tb, dir) = toolbox("skin");
    let model = lahza_motion_render::model::write_test_arm(&dir.join("src")).unwrap();
    tb.call("project_create", json!({"path": "arm", "duration": 2, "width": 480, "height": 360})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1", "background": "#101014"}]})).unwrap();
    let out = tb.call("model_import", json!({"path": model.display().to_string(), "scene": "s1", "position": [240, 320]})).unwrap().json;
    assert_eq!((out["report"]["skinned"].as_u64(), out["report"]["animated_nodes"].as_u64()), (Some(1), Some(1)), "{out:#}");
    let report = tb.call("check", json!({})).unwrap().json;
    assert_eq!(report["errors"], 0, "{report:#}");
    let shot = |tb: &mut Toolbox, t: f64| image::load_from_memory(&tb.call("snapshot", json!({"at": [t], "width": 480})).unwrap().images[0]).unwrap().to_rgba8();
    let left = |img: &image::RgbaImage| img.enumerate_pixels().filter(|(_, _, p)| p.0[0] > 120).map(|(x, _, _)| x).min().unwrap();
    // The file's animation turns the middle joint: the arm's tip swings left.
    let (rest, bent) = (shot(&mut tb, 0.0), shot(&mut tb, 1.0));
    assert!(left(&bent) + 50 < left(&rest), "left edge {} → {}", left(&rest), left(&bent));
    // A joint is a layer: keyed by an edit, the mesh follows.
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "arm-mid.transform.rotation", "value": 0}]})).unwrap();
    assert_eq!(left(&shot(&mut tb, 1.0)), left(&rest));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_skinned_model_falls_as_a_ragdoll_and_its_mesh_goes_with_it() {
    let (mut tb, dir) = toolbox("ragdoll");
    let model = lahza_motion_render::model::write_test_arm(&dir.join("src")).unwrap();
    tb.call("project_create", json!({"path": "arm", "duration": 3, "width": 480, "height": 360})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1", "background": "#101014"}]})).unwrap();
    tb.call("model_import", json!({"path": model.display().to_string(), "scene": "s1", "position": [240, 200]})).unwrap();
    // The arm's three joints as a ragdoll, let go at 1 s (the end of its
    // bend), limp, over a floor.
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "s1.simulation", "value": {"ground": {"y": 330}, "ragdolls": [{"id": "doll", "root": "arm-root", "release": 1.0, "rest": 0.0, "limits": [{"bone": "mid", "swing": 120}]}]}}]})).unwrap();
    let report = tb.call("check", json!({})).unwrap().json;
    assert!(sim_issues(&report)[0].contains("no bake"), "{report:#}");
    let baked = tb.call("sim_bake", json!({})).unwrap().json;
    assert_eq!((baked["scenes"][0]["bodies"].as_u64(), baked["scenes"][0]["tracks"].as_u64()), (Some(3), Some(3)), "{baked:#}");
    assert_eq!(tb.call("check", json!({})).unwrap().json["errors"], 0);
    let shot = |tb: &mut Toolbox, t: f64| image::load_from_memory(&tb.call("snapshot", json!({"at": [t], "width": 480})).unwrap().images[0]).unwrap().to_rgba8();
    let top = |img: &image::RgbaImage| img.enumerate_pixels().filter(|(_, _, p)| p.0[0] > 120).map(|(_, y, _)| y).min().unwrap();
    // Until its release it is the file's animation; two seconds on, the
    // skinned mesh lies on the floor with its bones.
    let (standing, fallen) = (shot(&mut tb, 0.9), shot(&mut tb, 2.9));
    assert!(top(&fallen) > top(&standing) + 100, "the mesh's top went from row {} to {}", top(&standing), top(&fallen));
    std::fs::remove_dir_all(&dir).ok();
}
