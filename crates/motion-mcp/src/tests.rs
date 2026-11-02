use crate::tools::Toolbox;
use lahza_motion_render::Backend;
use serde_json::json;

fn toolbox(name: &str) -> (Toolbox, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lahza-motion-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    tb.backend = Backend::Cpu;
    (tb, dir)
}

#[test]
fn words_from_a_recording_anchor_layers() {
    let (mut tb, dir) = toolbox("words");
    // A recording folder with a transcript in its edit document.
    let rec = dir.join("talk.lahzarec");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(
        rec.join("edit.json"),
        json!({"formatVersion": 5, "transcript": {"words": [
            {"text": " The", "start": 0.4, "end": 0.6},
            {"text": " canal", "start": 0.7, "end": 1.1},
            {"text": " closed.", "start": 1.15, "end": 1.6},
            {"text": " Ships", "start": 2.8, "end": 3.1}
        ], "cues": [], "language": "en"}})
        .to_string(),
    )
    .unwrap();
    tb.call("project_create", json!({"path": "explainer", "duration": 5, "style": "explainer.evidence", "width": 1280, "height": 720})).unwrap();
    let out = tb.call("markers_import", json!({"kind": "transcript", "path": "talk.lahzarec", "offset": 1.0})).unwrap();
    assert_eq!(out.json["imported"][0]["count"], 4);
    let words = tb.call("markers_query", json!({"track": "word", "text": "canal"})).unwrap().json;
    assert_eq!(words["word"][0]["ref"], "word:2");
    assert_eq!(words["word"][0]["t"], 1.7);
    let sentences = tb.call("markers_query", json!({"track": "sentence"})).unwrap().json;
    assert_eq!(sentences["sentence"].as_array().unwrap().len(), 2);
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1", "step_fps": 0},
        {"op": "add", "id": "label", "kind": "text", "text": "Suez Canal", "size": 64, "color": "#111111",
         "transform": {"position": [640, 360], "opacity": {"keys": [{"at": "word:canal", "from": 0, "v": 1, "ease": "enter", "dur": "6f", "anchor": "start"}]}}},
        {"op": "add", "id": "mark", "kind": "shape", "shape": "line", "size": [360, 0], "thickness": 10, "fill": "#ffd23f",
         "transform": {"position": [640, 410]},
         "trim": {"keys": [{"at": 0, "v": [0, 0]}, {"at": "word:closed.end", "v": [0, 1], "ease": "gesture", "dur": "8f", "anchor": "end"}]}}
    ]}))
    .unwrap();
    let report = tb.call("check", json!({})).unwrap().json;
    assert_eq!(report["ok"], true, "{report:#}");
    let syncs = report["syncs"].as_array().unwrap();
    assert!(syncs.iter().any(|s| s["reference"] == "word:canal"), "{report:#}");
    let info = tb.call("project_info", json!({})).unwrap().json;
    assert_eq!(info["settings"]["style"], "explainer.evidence");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn edits_made_on_disk_are_picked_up() {
    let (mut tb, dir) = toolbox("disk");
    tb.call("project_create", json!({"path": "p", "duration": 2})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}]})).unwrap();
    // A person edits the scene file directly.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let scene = dir.join("p.lahza/scenes/s1.json");
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&scene).unwrap()).unwrap();
    v["layers"] = json!([{"id": "box", "kind": "shape", "shape": "rect", "fill": "#ff0000"}]);
    std::fs::write(&scene, v.to_string()).unwrap();
    let tl = tb.call("timeline_get", json!({})).unwrap().json;
    assert_eq!(tl["scenes"][0]["layers"][0]["id"], "box");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_and_redo_through_history() {
    let (mut tb, dir) = toolbox("redo");
    tb.call("project_create", json!({"path": "p", "duration": 2})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}]})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add", "scene": "s1", "id": "a", "kind": "shape", "shape": "rect"}]})).unwrap();
    let layers = |tb: &mut Toolbox| tb.call("timeline_get", json!({})).unwrap().json["scenes"][0]["layers"].as_array().unwrap().len();
    assert_eq!(layers(&mut tb), 1);
    tb.call("history", json!({"undo": 1})).unwrap();
    assert_eq!(layers(&mut tb), 0);
    tb.call("history", json!({"redo": true})).unwrap();
    assert_eq!(layers(&mut tb), 1);
    // Redo once only; then undo works again.
    assert!(tb.call("history", json!({"redo": true})).is_err());
    tb.call("history", json!({"undo": 1})).unwrap();
    assert_eq!(layers(&mut tb), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_motion_piece_lives_inside_a_recording_and_shares_its_files() {
    let (mut tb, dir) = toolbox("recording");
    let rec = dir.join("demo.lahzarec");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(rec.join("edit.json"), json!({"formatVersion": 6, "clips": [
        {"id": "a", "sourceStart": 0.0, "sourceEnd": 1.5, "speed": 1.0},
        {"id": "b", "sourceStart": 1.5, "sourceEnd": 2.0, "speed": 0.5, "gapBefore": 0.25}
    ]}).to_string()).unwrap();
    let ff = |args: &[&str]| assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y"]).args(args).status().unwrap().success());
    ff(&["-f", "lavfi", "-i", "testsrc=s=640x400:d=2:r=30", "-c:v", "libx264", "-pix_fmt", "yuv420p", rec.join("screen.mkv").to_str().unwrap()]);
    ff(&["-f", "lavfi", "-i", "color=c=red:s=64x64", "-frames:v", "1", dir.join("logo.png").to_str().unwrap()]);

    let info = tb.call("recording_info", json!({"recording": "demo.lahzarec"})).unwrap().json;
    assert_eq!(info["size"], json!([640, 400]));
    assert_eq!(info["duration"], 2.75);
    let created = tb.call("project_create", json!({"path": "intro", "recording": "demo.lahzarec", "duration": 3, "background": "#00000000"})).unwrap().json;
    assert_eq!(created["settings"]["width"], 1920);
    assert_eq!(created["settings"]["height"], 1200);
    assert!(rec.join("motion/intro.lahza/project.json").is_file());

    // The recording is used in place; other files go to the shared media folder.
    let screen = tb.call("media_import", json!({"path": "demo.lahzarec/screen.mkv"})).unwrap().json;
    assert_eq!(screen["src"], "../../screen.mkv");
    let logo = tb.call("media_import", json!({"path": "logo.png"})).unwrap().json;
    assert_eq!(logo["src"], "../../media/logo.png");
    assert!(rec.join("media/logo.png").is_file());
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "id": "rec", "kind": "video", "src": "../../screen.mkv", "size": [960, 600], "transform": {"position": [960, 600]}},
        {"op": "add", "id": "logo", "kind": "image", "src": "../../media/logo.png", "transform": {"position": [200, 200]}}
    ]})).unwrap();
    let check = tb.call("check", json!({})).unwrap().json;
    assert_eq!(check["errors"], 0, "{check:#}");
    let shot = tb.call("snapshot", json!({"at": [1.0]})).unwrap();
    assert!(!shot.images.is_empty());

    // Place it on the video, move it, remove it.
    let placed = tb.call("recording_place", json!({"start": 1.25, "placement": "full", "name": "Intro"})).unwrap().json;
    assert_eq!(placed["pieces"][0]["piece"]["folder"], "motion/intro.lahza");
    assert_eq!(placed["pieces"][0]["piece"]["placement"], "full");
    assert_eq!(placed["pieces"][0]["duration"], 3.0);
    let draft: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(rec.join("edit.draft.json")).unwrap()).unwrap();
    assert_eq!(draft["clips"].as_array().unwrap().len(), 2, "other keys survive");
    let moved = tb.call("recording_place", json!({"start": 0.5})).unwrap().json;
    assert_eq!(moved["pieces"][0]["piece"]["start"], 0.5);
    assert_eq!(moved["pieces"][0]["piece"]["placement"], "full", "unchanged fields are kept");
    assert_eq!(tb.call("project_info", json!({})).unwrap().json["recording"]["placed"]["name"], "Intro");
    let removed = tb.call("recording_place", json!({"remove": true})).unwrap().json;
    assert!(removed["pieces"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}
