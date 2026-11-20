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
    let shot = tb.call("snapshot", json!({"at": [0.5], "width": 160})).unwrap().json;
    assert!(shot["warnings"][0].as_str().unwrap().contains("is not baked"), "{shot:#}");
    let sheets = tb.call("review", json!({})).unwrap().json;
    assert!(sheets["warnings"].as_array().unwrap().len() == 1 && sheets["warnings"][0].as_str().unwrap().contains("scene 's1'") && sheets["warnings"][0].as_str().unwrap().contains("is not baked"), "{:#}", sheets["warnings"]);
    assert!(tb.call("render", json!({"quality": "draft", "audio": false})).unwrap_err().contains("not baked"));
    let baked = tb.call("sim_bake", json!({})).unwrap().json;
    let s = &baked["scenes"][0];
    assert_eq!((s["status"].as_str(), s["bodies"].as_u64(), s["tracks"].as_u64(), s["frames"].as_u64()), (Some("baked"), Some(2), Some(2), Some(60)), "{baked:#}");
    assert!(s["events"].as_array().unwrap().iter().any(|e| e["a"] == "crate" && e["b"] == "ground"), "{baked:#}");
    let file = dir.join("drop.lahza/sim/s1.bake");
    let first = std::fs::read(&file).unwrap();
    assert!(sim_issues(&tb.call("check", json!({})).unwrap().json).is_empty());
    // Fresh, a snapshot has nothing to say about the simulation and a render goes ahead.
    assert!(tb.call("snapshot", json!({"at": [0.5], "width": 160})).unwrap().json.get("warnings").is_none());
    assert!(tb.call("review", json!({})).unwrap().json.get("warnings").is_none());
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
    // A snapshot of it says so, and a render is refused unless asked for as it is.
    let shot = tb.call("snapshot", json!({"at": [0.5], "width": 160})).unwrap().json;
    assert!(shot["warnings"][0].as_str().unwrap().contains("bake is stale"), "{shot:#}");
    let sheets = tb.call("review", json!({})).unwrap().json;
    assert!(sheets["warnings"][0].as_str().unwrap().contains("bake is stale") && sheets["warnings"][0].as_str().unwrap().contains("sim_bake"), "{:#}", sheets["warnings"]);
    let refused = tb.call("render", json!({"quality": "draft", "audio": false, "wait": true})).unwrap_err();
    assert!(refused.contains("scene 's1'") && refused.contains("stale") && refused.contains("sim_bake"), "{refused}");
    let out = dir.join("stale.mp4");
    let done = tb.call("render", json!({"quality": "draft", "audio": false, "wait": true, "unbaked": true, "to": 0.2, "output": out.display().to_string()})).unwrap().json;
    assert!(out.is_file(), "{done:#}");
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
fn a_gltf_is_imported_with_the_buffer_and_the_picture_beside_it() {
    let (mut tb, dir) = toolbox("apart");
    // The arm as a .gltf whose buffer is `arm.bin` beside it, with a
    // picture beside it too for its colour.
    let src = dir.join("src");
    let model = lahza_motion_render::model::write_test_arm_as(&src, false, true).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255])).save(src.join("skin colour.png")).unwrap();
    let mut doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&model).unwrap()).unwrap();
    doc["images"] = json!([{"uri": "skin%20colour.png"}]);
    doc["textures"] = json!([{"source": 0}]);
    doc["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": 0});
    std::fs::write(&model, serde_json::to_vec(&doc).unwrap()).unwrap();
    tb.call("project_create", json!({"path": "arm", "duration": 2, "width": 480, "height": 360})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1", "background": "#101014"}]})).unwrap();
    let out = tb.call("model_import", json!({"path": model.display().to_string(), "scene": "s1", "position": [240, 320]})).unwrap().json;
    assert_eq!((out["files"].as_u64(), out["report"]["skinned"].as_u64(), out["report"]["textures"].as_array().map(Vec::len)), (Some(2), Some(1), Some(1)), "{out:#}");
    assert!(out["report"]["warnings"].as_array().unwrap().is_empty(), "{out:#}");
    let project = dir.join("arm.lahza");
    let project = if project.is_dir() { project } else { dir.join("arm") };
    assert!(project.join("assets/models/arm/arm.bin").is_file() && project.join("assets/models/arm/tex0.png").is_file(), "{:?}", std::fs::read_dir(project.join("assets/models/arm")).map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>()));
    // The project stands by itself: with what it was imported from gone,
    // it checks and draws.
    std::fs::remove_dir_all(&src).unwrap();
    let report = tb.call("check", json!({})).unwrap().json;
    assert_eq!(report["errors"], 0, "{report:#}");
    let shot = image::load_from_memory(&tb.call("snapshot", json!({"at": [1.0], "width": 480})).unwrap().images[0]).unwrap().to_rgba8();
    assert!(shot.pixels().filter(|p| p.0[0] > 120).count() > 1500, "the arm is drawn");
    // A buffer that is not there is named.
    let lone = lahza_motion_render::model::write_test_arm_as(&dir.join("lone"), false, true).unwrap();
    std::fs::remove_file(dir.join("lone/arm.bin")).unwrap();
    let e = tb.call("model_import", json!({"path": lone.display().to_string(), "scene": "s1", "id": "lone"})).unwrap_err();
    assert!(e.contains("arm.bin"), "{e}");
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

/// Directing: intent written on the timeline through the tools, a bake,
/// a look at the take, and another seed for another.
#[test]
fn a_take_is_directed_baked_read_and_taken_again() {
    let (mut tb, dir) = toolbox("direct");
    tb.call("project_create", json!({"path": "shot", "duration": 4, "width": 640, "height": 360})).unwrap();
    // A figure of bare bones (comps), standing on a floor.
    let bone = |id: &str, at: [f64; 2], kids: Vec<Value>| json!({"id": id, "kind": "comp", "three_d": true, "transform": {"position": at}, "layers": kids});
    let leg = |s: &str, x: f64| bone(&format!("thigh_{s}"), [x, 5.0], vec![bone(&format!("shin_{s}"), [0.0, 50.0], vec![bone(&format!("foot_{s}"), [0.0, 48.0], vec![])])]);
    let arm = |s: &str, x: f64| bone(&format!("upper_{s}"), [x, -30.0], vec![bone(&format!("fore_{s}"), [0.0, 34.0], vec![bone(&format!("hand_{s}"), [0.0, 32.0], vec![])])]);
    let figure = bone("hips", [320.0, 200.0], vec![bone("spine", [0.0, -14.0], vec![bone("chest", [0.0, -32.0], vec![bone("head", [0.0, -36.0], vec![]), arm("l", -23.0), arm("r", 23.0)])]), leg("l", -11.0), leg("r", 11.0)]);
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}, {"op": "add", "scene": "s1", "layer": figure}]})).unwrap();
    // "Pushed from the left at 1 s; raises its arms; stumbles; recovers."
    let doll = json!({"id": "doll", "root": "hips", "strength": 1,
        "shapes": [{"bone": "foot", "shape": {"type": "box", "size": [20, 11, 60], "offset": [0, 5.5, -9]}}],
        "limits": [{"bone": "shin", "swing": 130, "hinge": [-1, 0, 0]}, {"bone": "thigh", "swing": 70}, {"bone": "upper", "swing": 170, "twist": 170}, {"bone": "fore", "swing": 150, "hinge": [1, 0, 0]}],
        "hits": [{"at": 1.0, "bone": "chest", "velocity": [700, 0, 0]}],
        "behaviours": [{"type": "balance"}, {"type": "step"}, {"type": "protect", "from": 0.7, "to": 1.6}]});
    let sim = |doll: &Value, extra: Value| {
        let mut s = json!({"unit": 46, "ground": {"y": 314.2, "friction": 0.9}, "ragdolls": [doll]});
        for (k, v) in extra.as_object().unwrap() {
            s[k] = v.clone();
        }
        json!({"ops": [{"op": "set", "target": "s1.simulation", "value": s}]})
    };
    tb.call("edit_apply", sim(&doll, json!({}))).unwrap();
    let take = tb.call("sim_bake", json!({})).unwrap().json["scenes"][0].clone();
    // The take, read: it stepped and did not fall.
    assert_eq!(take["ragdolls"][0]["id"], "doll");
    assert!(take["ragdolls"][0]["fell"].is_null() && take["ragdolls"][0]["steps"].as_u64().unwrap() >= 1, "{take:#}");
    assert!(take["events"].as_array().unwrap().iter().any(|e| e["kind"] == "step"), "{take:#}");
    // Looked at: a sheet across the action.
    let sheet = tb.call("snapshot", json!({"at": [0.5, 1.0, 1.3, 2.0, 3.5], "sheet": true})).unwrap();
    assert_eq!(sheet.images.len(), 1);
    // Each frame says what happened by it since the frame before: nothing
    // by 0.5 s, the steps after the push at 1 s; every event once.
    let frames = sheet.json["frames"].as_array().unwrap();
    assert!(frames[0].get("events").is_none(), "{:#}", sheet.json);
    let kinds = |k: usize| frames[k]["events"].as_array().map(|e| e.iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    let stepped: usize = (0..5).map(|k| kinds(k).iter().filter(|x| *x == "step").count()).sum();
    assert_eq!(stepped as u64, take["ragdolls"][0]["steps"].as_u64().unwrap(), "{:#}", sheet.json);
    assert!(kinds(2).contains(&"step".to_string()) || kinds(3).contains(&"step".to_string()));
    let listed: usize = (0..5).map(|k| kinds(k).len()).sum();
    assert_eq!(listed, take["events"].as_array().unwrap().iter().filter(|e| e["t"].as_f64().unwrap() <= 3.5).count());
    // And the sheet marks them on those frames (amber, over the frame's foot).
    let amber = |png: &[u8]| image::load_from_memory(png).unwrap().to_rgba8().pixels().filter(|p| p.0[0] > 200 && p.0[1] > 160 && p.0[2] < 150 && p.0[0] > p.0[2] + 80).count();
    assert!(amber(&sheet.images[0]) > 40, "{} amber pixels", amber(&sheet.images[0]));
    let quiet = tb.call("snapshot", json!({"at": [0.2, 0.5], "sheet": true})).unwrap();
    assert_eq!(amber(&quiet.images[0]), 0);
    if let Ok(out) = std::env::var("LAHZA_SIM_OUT") {
        std::fs::write(format!("{out}/marked.png"), &sheet.images[0]).unwrap();
    }
    // Redirected: a harder push, no steps. It falls, and the take says when.
    let mut harder = doll.clone();
    harder["hits"][0]["velocity"] = json!([1500, 0, 0]);
    harder["behaviours"] = json!([{"type": "balance"}]);
    tb.call("edit_apply", sim(&harder, json!({}))).unwrap();
    assert!(sim_issues(&tb.call("check", json!({})).unwrap().json)[0].contains("stale"));
    let take = tb.call("sim_bake", json!({})).unwrap().json["scenes"][0].clone();
    let fell = take["ragdolls"][0]["fell"].as_f64().expect("a fall");
    assert!(fell > 1.0 && fell < 3.5, "{take:#}");
    assert_eq!(take["events"][0]["kind"], "fall", "what it did comes before what touched what");
    // Another seed is another take: with jitter the blow lands differently.
    tb.call("edit_apply", sim(&harder, json!({"jitter": 120}))).unwrap();
    let file = dir.join("shot.lahza/sim/s1.bake");
    tb.call("sim_bake", json!({"seed": 1})).unwrap();
    let one = std::fs::read(&file).unwrap();
    let two = tb.call("sim_bake", json!({"seed": 2})).unwrap().json["scenes"][0].clone();
    assert_eq!(two["seed"], 2);
    assert!(std::fs::read(&file).unwrap() != one);
    // And the first take comes back with its seed.
    tb.call("sim_bake", json!({"seed": 1})).unwrap();
    assert!(std::fs::read(&file).unwrap() == one);
    std::fs::remove_dir_all(&dir).ok();
}

/// A crowd of 24 instances of a two-mesh character walking away over a
/// field, `full` of them drawn as meshes.
fn herd_shot(name: &str, full: u32) -> (image::RgbaImage, Value) {
    let (mut tb, _) = toolbox(name);
    tb.call("project_create", json!({"path": "herd", "duration": 3, "width": 640, "height": 360})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1", "background": "#101014", "camera": {"orbit": [20, 25], "dolly": -500},
         "lights": [{"from": [-0.5, -1, -0.6], "intensity": 1.2}, {"type": "ambient", "intensity": 0.4}],
         "simulation": {"crowds": [{"id": "walkers", "character": "fig", "count": 24, "area": [-200, -300, 840, 900], "march": [60, 400], "full": full, "radius": 28, "speed": 120, "stride": 120}]}},
        {"op": "add", "id": "fig", "kind": "comp", "three_d": true, "transform": {"position": [320, 300]}, "layers": [
            {"id": "fig-body", "kind": "mesh", "mesh": {"type": "box", "size": [44, 110], "depth": 26}, "material": {"color": "#3b8cff"}, "transform": {"position": [0, -55]}},
            {"id": "fig-head", "kind": "mesh", "mesh": {"type": "sphere", "radius": 20}, "material": {"color": "#ffd9b0"}, "transform": {"position": [0, -132]}}
        ]}
    ]}))
    .unwrap();
    let baked = tb.call("sim_bake", json!({})).unwrap().json;
    assert_eq!(baked["scenes"][0]["status"], "baked", "{baked:#}");
    let report = tb.call("check", json!({})).unwrap().json;
    assert!(sim_issues(&report).is_empty(), "{report:#}");
    let shot = tb.call("snapshot", json!({"at": [1.5], "width": 640})).unwrap();
    assert!(shot.json.get("warnings").is_none(), "{:#}", shot.json);
    (image::load_from_memory(&shot.images[0]).unwrap().to_rgba8(), baked)
}

#[test]
fn far_agents_of_a_crowd_are_pictures_that_look_like_the_character() {
    let (meshes, baked) = herd_shot("herd-meshes", 24);
    let (cards, _) = herd_shot("herd-cards", 4);
    // The bake has no track per agent.
    assert_eq!(baked["scenes"][0]["tracks"], 0, "{baked:#}");
    assert_eq!(baked["scenes"][0]["agents"], 24, "{baked:#}");
    // Both show the crowd: about as much of the frame is not background…
    let drawn = |img: &image::RgbaImage| img.pixels().filter(|p| p[0] as i32 + p[1] as i32 + p[2] as i32 > 3 * 0x14 + 40).count() as f64;
    let (a, b) = (drawn(&meshes), drawn(&cards));
    assert!(a > 6000.0, "{a} px of crowd");
    assert!((b / a - 1.0).abs() < 0.12, "{a} px drawn as meshes, {b} as 4 meshes and 20 pictures");
    // …in the same places: what one shows and the other does not is a
    // small part of it (the pictures' turns and moments are rounded, and
    // they cast no shadows on each other).
    let differ = meshes.pixels().zip(cards.pixels()).filter(|(p, q)| (0..3).map(|c| (p[c] as i32 - q[c] as i32).abs()).sum::<i32>() > 90).count() as f64;
    assert!(differ < 0.3 * a, "{differ} px differ of {a} drawn");
    // And not the same picture: twenty of them are cards.
    assert!(differ > 0.0);
    if let Ok(d) = std::env::var("LAHZA_TEST_OUT") { meshes.save(format!("{d}/herd-meshes.png")).unwrap(); cards.save(format!("{d}/herd-cards.png")).unwrap(); eprintln!("drawn {a} {b} differ {differ}"); }
}
