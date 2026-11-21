//! Blender bridge: an agent-written bpy script renders a transparent
//! sequence that becomes a footage layer. Skips when Blender is missing.

use lahza_motion_mcp::tools::Toolbox;
use lahza_motion_render::Backend;
use serde_json::json;

const CUBE: &str = r#"
import bpy, math
cube = bpy.data.objects["Cube"]
mat = bpy.data.materials.new("lime")
mat.use_nodes = True
mat.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (0.84, 1.0, 0.23, 1.0)
if cube.data.materials:
    cube.data.materials[0] = mat
else:
    cube.data.materials.append(mat)
cube.rotation_euler = (0.4, 0.0, 0.0)
cube.keyframe_insert("rotation_euler", frame=LAHZA["frame_start"])
cube.rotation_euler = (0.4, 0.0, math.radians(90))
cube.keyframe_insert("rotation_euler", frame=LAHZA["frame_end"])
"#;

/// Keys the cube on the project's second beat and records what the script saw.
const TIMING: &str = r#"
import bpy, json, os
beats = LAHZA["markers"]["beat"]
assert beats["frames"] == [frame_of(t) for t in beats["seconds"]], beats
seen = {"beat_frames": beats["frames"], "title": LAHZA["elements"]["title"], "quality": LAHZA["quality"]}
json.dump(seen, open(os.path.join(LAHZA["out"], "..", "timing.json"), "w"))
cube = bpy.data.objects["Cube"]
cube.location.x = -2
cube.keyframe_insert("location", frame=LAHZA["frame_start"])
cube.location.x = 2
cube.keyframe_insert("location", frame=beats["frames"][1])
"#;

#[test]
fn blender_preview_sees_the_timeline() {
    if std::process::Command::new("blender").arg("--version").output().is_err() {
        eprintln!("skipping: Blender is not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lahza-blender-timing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    tb.backend = Backend::Cpu;
    tb.call("project_create", json!({"path": "timing", "width": 320, "height": 180, "fps": 10, "duration": 1.0})).unwrap();
    tb.call("markers_import", json!({"kind": "user", "name": "beat", "markers": [0.2, 0.6]})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"},
        {"op": "add", "scene": "s1", "id": "title", "kind": "shape", "shape": "circle", "size": 40, "fill": "#ffffff"}]})).unwrap();
    let out = tb.call("blender_run", json!({"name": "beat", "script": TIMING, "quality": "preview", "preview_frames": 3})).unwrap();
    let status = &out.json;
    assert_eq!(status["state"], "done", "{status:#}");
    let report = &status["report"];
    assert_eq!(report["preview"], true, "{report:#}");
    assert_eq!(report["frames"], 3, "{report:#}");
    assert_eq!(report["frame_times"].as_array().unwrap().len(), 3, "{report:#}");
    assert!(report["seconds_per_frame"].as_f64().unwrap() > 0.0, "{report:#}");
    assert_eq!(report["blender"]["width"], 160, "half size: {report:#}");
    assert_eq!(report["blender"]["motion_blur"], false, "{report:#}");
    assert!(report["blender"]["device"].is_string(), "{report:#}");
    assert_eq!(out.images.len(), 1, "a sheet of the preview frames");
    let seen: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("timing.lahza/assets/blender/timing.json")).unwrap()).unwrap();
    // Frame 1 shows 0 s at 10 fps: beats at 0.2 s and 0.6 s are frames 3 and 7.
    assert_eq!(seen["beat_frames"], json!([3, 7]), "{seen:#}");
    assert_eq!(seen["title"]["start_frame"], 1, "{seen:#}");
    assert_eq!(seen["title"]["end_frame"], 11, "{seen:#}");
    assert_eq!(seen["quality"], "preview");
    // A preview is not footage: no asset, and the final folder is untouched.
    let info = tb.call("project_info", json!({})).unwrap().json;
    assert!(info["assets"].as_array().unwrap().iter().all(|a| !a["path"].as_str().unwrap().contains("blender")), "{info:#}");
    assert!(!dir.join("timing.lahza/assets/blender/beat").exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn blender_script_becomes_footage() {
    if std::process::Command::new("blender").arg("--version").output().is_err() {
        eprintln!("skipping: Blender is not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lahza-blender-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    tb.backend = Backend::Cpu;
    tb.call("project_create", json!({"path": "three", "width": 320, "height": 180, "fps": 10, "duration": 0.5, "background": "#202040"})).unwrap();
    let out = tb.call("blender_run", json!({"name": "cube", "script": CUBE, "samples": 4})).unwrap().json;
    assert_eq!(out["state"], "done", "{out:#}");
    let report = &out["report"];
    eprintln!("blender: {} on {}, {} s per frame", report["blender"]["engine"], report["blender"]["device"], report["seconds_per_frame"]);
    assert_eq!(report["frames"], 5, "{report:#}");
    let src = report["src"].as_str().unwrap().to_string();
    // The frames have transparency around the cube.
    let first = image::open(dir.join("three.lahza/assets/blender/cube/cube_0001.png")).unwrap().to_rgba8();
    assert_eq!(first.get_pixel(0, 0).0[3], 0, "transparent film");
    assert!(first.pixels().any(|p| p.0[3] == 255), "the cube is opaque");
    // Composite it over the background as footage.
    let mut layer = report["layer"].clone();
    layer["id"] = json!("cube");
    layer["transform"] = json!({"position": [160, 90]});
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}, {"op": "add", "scene": "s1", "layer": layer}]})).unwrap();
    let report = tb.call("check", json!({})).unwrap().json;
    assert_eq!(report["ok"], true, "{report:#}");
    let shot = tb.call("snapshot", json!({"at": [0.0, 0.4], "sheet": false, "width": 320})).unwrap();
    let a = image::load_from_memory(&shot.images[0]).unwrap().to_rgba8();
    let b = image::load_from_memory(&shot.images[1]).unwrap().to_rgba8();
    assert_eq!(a.get_pixel(2, 2).0, [0x20, 0x20, 0x40, 255], "background shows through");
    assert_ne!(a, b, "the cube turns");
    let info = tb.call("project_info", json!({})).unwrap().json;
    assert!(info["assets"].as_array().unwrap().iter().any(|a| a["path"] == src && a["licence"] == "generated"));
    if let Ok(keep) = std::env::var("LAHZA_BLENDER_OUT") {
        std::fs::write(std::path::Path::new(&keep).join("blender-cube.png"), &shot.images[1]).unwrap();
    } else {
        std::fs::remove_dir_all(&dir).ok();
    }
}
