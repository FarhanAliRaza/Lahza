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

#[test]
fn sound_effects_follow_their_animation_into_the_render() {
    let (mut tb, dir) = toolbox("sfx");
    tb.call("project_create", json!({"path": "p", "duration": 2, "width": 320, "height": 180})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "id": "card", "kind": "shape", "shape": "rect", "time": {"start": 1.0}}
    ]})).unwrap();
    let out = tb.call("sfx_add", json!({"sound": "pop", "at": ["#card.start", 0.25], "gain_db": -3})).unwrap().json;
    assert_eq!(out["created"], json!(["pop1", "pop2"]));
    assert_eq!(out["path"], "assets/sfx/pop.wav");
    assert!(dir.join("p.lahza/assets/sfx/pop.wav").is_file());
    let report = tb.call("check", json!({})).unwrap().json;
    assert!(!report.to_string().contains("has no asset entry"), "built-in sounds are registered assets: {report:#}");
    // Moving the animation moves its sound; the sound can be retimed by id.
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "card.time.start", "value": 1.2}, {"op": "remove", "id": "pop2"}]})).unwrap();
    let info = tb.call("project_info", json!({})).unwrap().json;
    assert_eq!(info["audio"]["sfx"].as_array().unwrap().len(), 1);
    assert!(tb.call("sfx_add", json!({"sound": "kazoo", "at": 1})).is_err());

    // The render's sound: silent, then the pop at 1.2 s.
    let mp4 = dir.join("out.mp4");
    tb.call("render", json!({"output": mp4, "quality": "draft", "wait": true})).unwrap();
    let pcm = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"]).arg(&mp4)
        .args(["-vn", "-ac", "1", "-ar", "8000", "-f", "f32le", "-"])
        .output().unwrap().stdout;
    let samples: Vec<f32> = pcm.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let at = |t: f64| ((t * 8000.0) as usize).min(samples.len());
    let loud = |a: f64, b: f64| samples[at(a)..at(b)].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(loud(0.0, 1.1) < 0.01, "silent before: {}", loud(0.0, 1.1));
    assert!(loud(1.18, 1.3) > 0.3, "the pop: {}", loud(1.18, 1.3));
    assert!(loud(1.4, 2.0) < 0.01, "and it ends");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sounds_on_a_recordings_own_timeline() {
    let (mut tb, dir) = toolbox("recsound");
    let rec = dir.join("demo.lahzarec");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(rec.join("edit.json"), json!({"formatVersion": 6, "clips": [
        {"id": "a", "sourceStart": 0.0, "sourceEnd": 2.0, "speed": 1.0},
        {"id": "b", "sourceStart": 4.0, "sourceEnd": 6.0, "speed": 1.0}
    ], "zoomCues": [{"id": "z", "start": 4.5, "end": 5.5}]}).to_string()).unwrap();
    std::fs::write(rec.join("input.json"), json!({"presses": [
        {"time": 1.0, "phase": "down"}, {"time": 1.1, "phase": "up"}, {"time": 3.0, "phase": "down"}, {"time": 5.0, "phase": "down"}
    ], "keystrokes": [{"time": 4.2}]}).to_string()).unwrap();
    let info = tb.call("recording_info", json!({"recording": "demo.lahzarec"})).unwrap().json;
    // The click at 3 s was cut; 5 s plays at 3 s on the edited video.
    assert_eq!(info["clicks"], json!([1.0, 3.0]));
    assert_eq!(info["keystrokes"], json!([2.2]));
    assert_eq!(info["zooms"][0]["start"], 2.5);
    let out = tb.call("recording_sound_add", json!({"recording": "demo.lahzarec", "sound": "click", "at": [1.0, 3.0], "gain_db": -6})).unwrap().json;
    assert_eq!(out["added"], 2);
    assert_eq!(out["sounds"].as_array().unwrap().len(), 2);
    let draft: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(rec.join("edit.draft.json")).unwrap()).unwrap();
    let clip = &draft["audioClips"][1];
    assert_eq!(clip["file"], "media/sfx-click.wav");
    assert_eq!(clip["start"], 3.0);
    assert!((clip["volume"].as_f64().unwrap() - 0.501).abs() < 0.01);
    assert!(rec.join("media/sfx-click.wav").is_file());
    assert_eq!(draft["clips"].as_array().unwrap().len(), 2, "other keys survive");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sound_trim_and_fades_shape_the_render() {
    let (mut tb, dir) = toolbox("fades");
    tb.call("project_create", json!({"path": "p", "duration": 3, "width": 320, "height": 180})).unwrap();
    assert!(std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=4", "-y"])
        .arg(dir.join("p.lahza/assets/tone.wav"))
        .status().unwrap().success());
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add_sound", "id": "tone", "path": "assets/tone.wav", "at": 0.5, "trim": 1.0, "length": 2.0, "fade_in": 0.5, "fade_out": 0.5}
    ]})).unwrap();
    let mp4 = dir.join("out.mp4");
    tb.call("render", json!({"output": mp4, "quality": "draft", "wait": true})).unwrap();
    let pcm = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&mp4)
        .args(["-vn", "-ac", "1", "-ar", "8000", "-f", "f32le", "-"]).output().unwrap().stdout;
    let s: Vec<f32> = pcm.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let level = |a: f64, b: f64| s[((a * 8000.0) as usize).min(s.len())..((b * 8000.0) as usize).min(s.len())].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let full = level(1.3, 1.7);
    assert!(full > 0.1, "full level mid-clip (the sine source is 1/8): {full}");
    assert!(level(0.0, 0.45) < 0.01, "silent before it starts");
    assert!(level(0.5, 0.6) < full * 0.35, "fading in: {}", level(0.5, 0.6));
    assert!(level(2.4, 2.5) < full * 0.35, "fading out: {}", level(2.4, 2.5));
    assert!(level(2.55, 3.0) < 0.01, "cut after its length: {}", level(2.55, 3.0));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn compare_lines_frames_up_against_a_reference() {
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping: ffmpeg is not installed");
        return;
    }
    let (mut tb, dir) = toolbox("compare");
    tb.call("project_create", json!({"path": "look", "duration": 2, "width": 320, "height": 180, "background": "#000000"})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "scene": "s1", "layer": {"id": "dot", "kind": "shape", "shape": "circle", "size": 80, "fill": "#ff8800", "transform": {"position": [160, 90]}}}
    ]})).unwrap();
    // Our own frame as the reference: almost no difference.
    tb.call("snapshot", json!({"at": [0.5], "width": 320, "save": "same.png"})).unwrap();
    let same = tb.call("compare", json!({"against": "same.png", "at": [0.5], "width": 320})).unwrap();
    assert_eq!(same.images.len(), 1);
    let d = same.json["rows"][0]["mean_difference"].as_f64().unwrap();
    assert!(d < 2.0, "identical frames differ by {d}");
    // A reference with the dot elsewhere: a clear difference.
    let mut moved = image::RgbaImage::from_pixel(320, 180, image::Rgba([0, 0, 0, 255]));
    for y in 50..130 {
        for x in 20..100 {
            moved.put_pixel(x, y, image::Rgba([255, 136, 0, 255]));
        }
    }
    moved.save(dir.join("moved.png")).unwrap();
    let other = tb.call("compare", json!({"against": "moved.png", "at": [0.5], "width": 320})).unwrap().json;
    assert!(other["rows"][0]["mean_difference"].as_f64().unwrap() > 10.0, "{other:#}");
    assert!(tb.call("compare", json!({"against": "missing.mp4", "at": [0.5]})).is_err());
}

#[test]
fn compare_runs_every_frame_and_names_motion_gaps() {
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping: ffmpeg is not installed");
        return;
    }
    let (mut tb, dir) = toolbox("compare-run");
    tb.call("project_create", json!({"path": "run", "duration": 1, "fps": 10, "width": 160, "height": 90, "background": "#000000"})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "scene": "s1", "layer": {"id": "dot", "kind": "shape", "shape": "circle", "size": 30, "fill": "#ffffff",
            "transform": {"position": {"keys": [{"at": 0, "v": [20, 45]}, {"at": 0.9, "v": [140, 45]}]}}}}
    ]})).unwrap();
    let job = tb.call("render", json!({"output": "ref.mp4", "wait": true, "audio": false})).unwrap().json;
    assert!(dir.join("ref.mp4").is_file(), "{job:#}");
    // Against its own render: every frame compared, close.
    let same = tb.call("compare", json!({"against": "ref.mp4", "width": 160})).unwrap();
    assert_eq!(same.json["frames"], 10);
    assert!(same.json["mean_difference"].as_f64().unwrap() < 5.0, "{:#}", same.json);
    assert!(same.json["scenes"][0]["verdict"].as_str().unwrap().starts_with("motion similar"), "{:#}", same.json);
    assert!(same.json["video"].as_str().is_some_and(|v| std::path::Path::new(v).is_file()));
    // Hold the dot still: the reference now moves more.
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "dot.transform.position", "value": [20, 45]}]})).unwrap();
    let still = tb.call("compare", json!({"against": "ref.mp4", "width": 160})).unwrap().json;
    assert!(still["scenes"][0]["verdict"].as_str().unwrap().starts_with("the reference moves more"), "{still:#}");
}

#[test]
fn detail_score_notices_missing_thin_lines() {
    // A reference with a few thin lines on grey; ours is the same grey without them.
    let grey = image::Rgba([70, 70, 74, 255]);
    let mut reference = image::RgbaImage::from_pixel(200, 200, grey);
    for i in 20..180 {
        reference.put_pixel(i, 60, image::Rgba([220, 220, 225, 255]));
        reference.put_pixel(100, i, image::Rgba([220, 220, 225, 255]));
    }
    let blank = image::RgbaImage::from_pixel(200, 200, grey);
    let (missing, extra) = crate::tools::detail(&blank, &reference);
    assert!(missing > 0.95 && extra == 0.0, "missing {missing}, extra {extra}");
    let (missing, extra) = crate::tools::detail(&reference, &reference);
    assert!(missing < 0.01 && extra < 0.01, "identical: {missing} {extra}");
    // A one-pixel shift still counts as matched.
    let mut shifted = image::RgbaImage::from_pixel(200, 200, grey);
    for i in 20..180 {
        shifted.put_pixel(i, 61, image::Rgba([220, 220, 225, 255]));
        shifted.put_pixel(101, i, image::Rgba([220, 220, 225, 255]));
    }
    assert!(crate::tools::detail(&shifted, &reference).0 < 0.1);
}
