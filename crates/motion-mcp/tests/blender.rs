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
