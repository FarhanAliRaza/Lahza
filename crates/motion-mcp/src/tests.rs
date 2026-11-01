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
