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
    assert_eq!(words["word"][0]["ref"], "word:canal", "words are referred to by text, which survives edits");
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
fn ssim_is_one_for_identical_pictures_and_finds_the_differing_cell() {
    let mut a = image::RgbaImage::new(64, 64);
    for (x, y, p) in a.enumerate_pixels_mut() {
        let v = ((x * 7 + y * 13) % 256) as u8;
        p.0 = [v, v / 2, 255 - v, 255];
    }
    let (s, grid) = crate::measure::ssim(&a, &a, None);
    assert!((s - 1.0).abs() < 1e-4 && grid.len() == 16, "{s}");
    // Flatten the bottom-right cell: SSIM drops there and only there.
    let mut b = a.clone();
    for y in 48..64 {
        for x in 48..64 {
            b.put_pixel(x, y, image::Rgba([128, 128, 128, 255]));
        }
    }
    let (s, grid) = crate::measure::ssim(&a, &b, None);
    assert!(s < 0.97 && s > 0.8, "{s}");
    let worst = (0..16).min_by(|&i, &j| grid[i].total_cmp(&grid[j])).unwrap();
    assert_eq!(worst, 15, "{grid:?}");
    assert!(grid[0] > 0.999, "{grid:?}");
    // Inside a box away from the change: identical.
    assert!(crate::measure::ssim(&a, &b, Some([0, 0, 32, 32])).0 > 0.999);
    let summary = crate::measure::ssim_summary(&[(s, grid.clone()), (1.0, vec![1.0; 16])]);
    assert_eq!(summary["worst_cell"]["column"], 3);
    assert_eq!(summary["worst_cell"]["row"], 3);
    assert_eq!(summary["p05"].as_f64().unwrap(), crate::measure::ssim_summary(&[(s, grid)])["mean"].as_f64().unwrap());
}

#[test]
fn compare_scores_ssim_per_scene_and_in_a_drawn_box() {
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping: ffmpeg is not installed");
        return;
    }
    let (mut tb, dir) = toolbox("compare-ssim");
    tb.call("project_create", json!({"path": "score", "duration": 1, "fps": 10, "width": 160, "height": 96, "background": "#202024"})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "scene": "s1", "layer": {"id": "dot", "kind": "shape", "shape": "circle", "size": 20, "fill": "#ffffff",
            "transform": {"position": {"keys": [{"at": 0, "v": [20, 36]}, {"at": 0.9, "v": [140, 36]}]}}}},
        {"op": "add", "scene": "s1", "layer": {"id": "bar", "kind": "shape", "shape": "rect", "size": [120, 12], "fill": "#e07030",
            "transform": {"position": [80, 80]}}}
    ]})).unwrap();
    tb.call("render", json!({"output": "ref.mp4", "wait": true, "audio": false})).unwrap();
    let same = tb.call("compare", json!({"against": "ref.mp4", "width": 160, "drawn": [20, 72, 120, 16]})).unwrap().json;
    let s = same["scenes"][0]["ssim"]["mean"].as_f64().unwrap();
    assert!(s > 0.95, "against itself: {same:#}");
    assert!(same["drawn"]["ssim"]["mean"].as_f64().unwrap() > 0.95, "{same:#}");
    // Hold the dot still: the frame scores lower, the worst cell is on the
    // dot's path (row 1 of 4 at y 24–48), and the bar's box still matches.
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "dot.transform.position", "value": [20, 36]}]})).unwrap();
    let held = tb.call("compare", json!({"against": "ref.mp4", "width": 160, "drawn": [20, 72, 120, 16]})).unwrap().json;
    let sc = &held["scenes"][0];
    let s2 = sc["ssim"]["mean"].as_f64().unwrap();
    assert!(s2 < s - 0.02, "held {s2} vs {s}: {held:#}");
    assert!(sc["ssim"]["p05"].as_f64().unwrap() <= s2, "{held:#}");
    assert_eq!(sc["ssim"]["worst_cell"]["row"], 1, "{held:#}");
    let cell = &sc["ssim"]["worst_cell"]["box"];
    assert!(cell[1] == json!(24.0) && cell[2] == json!(40.0) && cell[3] == json!(24.0), "{held:#}");
    assert!(sc["drawn_ssim"]["mean"].as_f64().unwrap() > 0.95, "the bar is unchanged: {held:#}");
    assert!(held["ssim"]["mean"].is_number() && held["drawn"]["box"] == json!([20.0, 72.0, 120.0, 16.0]), "{held:#}");
    // A box on the dot's path scores lower than the whole frame.
    let path = tb.call("compare", json!({"against": "ref.mp4", "width": 160, "drawn": [0, 24, 160, 24]})).unwrap().json;
    assert!(path["drawn"]["ssim"]["mean"].as_f64().unwrap() < s2, "{path:#}");
    // Moments carry it too.
    let at = tb.call("compare", json!({"against": "ref.mp4", "at": [0.8], "width": 160, "drawn": [20, 72, 120, 16]})).unwrap().json;
    assert!(at["rows"][0]["ssim"].as_f64().unwrap() < 0.99 && at["rows"][0]["ssim_drawn"].as_f64().unwrap() > 0.95, "{at:#}");
    assert!(tb.call("compare", json!({"against": "ref.mp4", "drawn": [0, 0, 0, 5]})).is_err());
    std::fs::remove_dir_all(&dir).ok();
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

#[test]
fn compare_says_when_ours_is_flatter_than_a_lit_reference() {
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping: ffmpeg is not installed");
        return;
    }
    let (mut tb, dir) = toolbox("compare-flat");
    tb.call("project_create", json!({"path": "flat", "duration": 1, "fps": 10, "width": 320, "height": 180, "background": "#000000"})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "scene": "s1", "layer": {"id": "ball", "kind": "shape", "shape": "circle", "size": 130, "fill": "#eb8c46", "transform": {"position": [160, 90]}}}
    ]})).unwrap();
    // The reference: the same ball, lit from the top left.
    let mut lit = image::RgbaImage::from_pixel(320, 180, image::Rgba([0, 0, 0, 255]));
    let l = [-0.5f64, -0.6, 0.62];
    for y in 0..180 {
        for x in 0..320 {
            let (nx, ny) = ((x as f64 + 0.5 - 160.0) / 65.0, (y as f64 + 0.5 - 90.0) / 65.0);
            if nx * nx + ny * ny <= 1.0 {
                let nz = (1.0 - nx * nx - ny * ny).sqrt();
                let d = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
                let c = [235.0, 140.0, 70.0].map(|c: f64| (c * (0.15 + 0.85 * d)).min(255.0) as u8);
                lit.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    }
    lit.save(dir.join("lit.png")).unwrap();
    let at = tb.call("compare", json!({"against": "lit.png", "at": [0.5]})).unwrap().json;
    let row = &at["rows"][0];
    assert!(row["shading_ratio"].as_f64().unwrap() < 0.2, "{at:#}");
    assert!(row["shading_verdict"].as_str().unwrap().starts_with("ours is flatter"), "{at:#}");
    // Every frame, against a compressed video of it.
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-loop", "1", "-i", "lit.png", "-t", "1", "-r", "10", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", "lit.mp4"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    let run = tb.call("compare", json!({"against": "lit.mp4"})).unwrap().json;
    let scene = &run["scenes"][0];
    assert!(scene["verdict"].as_str().unwrap().contains("ours is flatter than the reference (shading"), "{run:#}");
    assert!(run["worst"].as_array().unwrap().iter().any(|w| w["verdict"] == "ours is flatter"), "{run:#}");
    // Against our own (flat) render nothing is flagged, and a flat
    // reference has no ratio.
    tb.call("render", json!({"output": "ours.mp4", "wait": true, "audio": false})).unwrap();
    let same = tb.call("compare", json!({"against": "ours.mp4"})).unwrap().json;
    assert!(!same["scenes"][0]["verdict"].as_str().unwrap().contains("flatter"), "{same:#}");
    assert!(same["scenes"][0]["shading_ratio"].is_null(), "{same:#}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn audio_files_import_as_sounds_and_paths_resolve_inside_the_project() {
    let (mut tb, dir) = toolbox("audio-import");
    tb.call("project_create", json!({"path": "p", "duration": 2, "width": 320, "height": 180})).unwrap();
    // A sound that lives inside the project, named relative to it (the
    // server's working folder is elsewhere).
    std::fs::create_dir_all(dir.join("p.lahza/input")).unwrap();
    let wav = dir.join("p.lahza/input/whoosh.wav");
    assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=0.5"]).arg(&wav).status().unwrap().success());
    let imported = tb.call("media_import", json!({"path": "input/whoosh.wav"})).unwrap().json;
    assert_eq!(imported["kind"], "audio");
    assert_eq!(imported["src"], "input/whoosh.wav");
    assert!((imported["info"]["duration"].as_f64().unwrap() - 0.5).abs() < 0.02, "{imported}");
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}]})).unwrap();
    let out = tb.call("sfx_add", json!({"sound": "input/whoosh.wav", "at": 0.5})).unwrap().json;
    assert_eq!(out["path"], "input/whoosh.wav");
    let err = tb.call("sfx_add", json!({"sound": "input/missing.wav", "at": 0.5})).unwrap_err();
    assert!(err.contains("not a built-in sound") && err.contains("project"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn narration_manifest_times_everything_and_survives_a_new_hook() {
    let (mut tb, dir) = toolbox("narration");
    tb.call("project_create", json!({"path": "p", "duration": 10, "width": 320, "height": 180})).unwrap();
    // "Speech": a tone where each word is, silence between words.
    let tone = |name: &str, spans: &[(f64, f64)], total: f64| {
        let expr = spans.iter().map(|(a, b)| format!("between(t,{a},{b})")).collect::<Vec<_>>().join("+");
        let path = dir.join(name);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("aevalsrc='0.5*sin(2*PI*220*t)*({expr})':s=22050:d={total}")])
            .arg(&path)
            .status()
            .unwrap()
            .success();
        assert!(ok);
    };
    tone("a.wav", &[(0.1, 0.4), (0.5, 0.9), (1.2, 1.5), (1.6, 2.1)], 2.3);
    tone("b.wav", &[(0.1, 0.5), (0.6, 1.4), (1.5, 1.9)], 2.0);
    let words_a = json!([{"text": "One", "start": 0.1, "end": 0.4}, {"text": "bomb.", "start": 0.5, "end": 0.9},
        {"text": "Over", "start": 1.2, "end": 1.5}, {"text": "Iran.", "start": 1.6, "end": 2.1}]);
    let words_b = json!([{"text": "It", "start": 0.1, "end": 0.5}, {"text": "eighty-one", "start": 0.6, "end": 1.4}, {"text": "cities", "start": 1.5, "end": 1.9}]);
    let manifest = |clips: serde_json::Value| {
        std::fs::write(dir.join("narration.json"), json!({"lead": 0.5, "pause": 0.4, "clips": clips}).to_string()).unwrap();
    };
    let a = json!({"id": "s01", "audio": "a.wav", "text": "One bomb. Over [strike]Iran.", "words": words_a});
    let b = json!({"id": "s02", "audio": "b.wav", "text": "It hits 81 cities.", "words": words_b});
    manifest(json!([a, b]));
    let out = tb.call("narration_import", json!({"manifest": "narration.json"})).unwrap().json;
    assert_eq!(out["lines"], 2, "{out:#}");
    // "hits" was never spoken: an error, with the line and the word.
    assert_eq!(out["issues"].as_array().unwrap().len(), 1, "{out:#}");
    assert!(out["issues"][0]["word"].as_str().unwrap().contains("hits"), "{out:#}");
    let markers = |tb: &mut crate::tools::Toolbox, track: &str| tb.call("markers_query", json!({"track": track})).unwrap().json;
    let cue = markers(&mut tb, "cue");
    assert_eq!(cue["cue"][0]["t"], 2.1, "line s01 starts at 0.5; Iran at 1.6: {cue:#}");
    let words = markers(&mut tb, "word");
    let w81 = words["word"].as_array().unwrap().iter().find(|m| m["label"] == "81").unwrap().clone();
    assert_eq!(w81["alt"], "eighty-one");
    // s02 follows s01 (ends 0.5 + 2.3) after the 0.4 s pause.
    assert_eq!(w81["t"], 3.8, "{w81:#}");
    // A word in silence is caught without listening.
    let mut late = words_a.clone();
    late[1]["start"] = json!(0.95);
    late[1]["end"] = json!(1.15);
    manifest(json!([{"id": "s01", "audio": "a.wav", "text": "One bomb. Over [strike]Iran.", "words": late}, b]));
    let out = tb.call("narration_import", json!({"manifest": "narration.json"})).unwrap().json;
    assert_eq!(out["reused"], json!(["s02"]), "unchanged lines are reused: {out:#}");
    assert!(out["issues"].as_array().unwrap().iter().any(|i| i["message"].as_str().unwrap().contains("silence")), "{out:#}");
    let report = tb.call("check", json!({})).unwrap().json;
    assert!(report["errors"].as_u64().unwrap() >= 2, "narration problems are errors in check: {report:#}");
    // A layer timed to the cue; then a hook line goes in front.
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}, {"op": "add", "id": "flash", "kind": "shape", "shape": "rect", "time": {"start": "cue:strike", "end": "cue:strike.end + 0.5"}}]})).unwrap();
    tone("h.wav", &[(0.1, 0.5), (0.6, 1.0)], 1.2);
    let hook = json!({"id": "hook", "audio": "h.wav", "text": "What if?", "words": [{"text": "What", "start": 0.1, "end": 0.5}, {"text": "if", "start": 0.6, "end": 1.0}]});
    manifest(json!([hook, a, b]));
    let out = tb.call("narration_import", json!({"manifest": "narration.json"})).unwrap().json;
    // The hook is new and s01 went back to its first timings; s02 is reused.
    assert_eq!(out["imported"], json!(["hook", "s01"]), "{out:#}");
    assert_eq!(out["reused"], json!(["s02"]), "{out:#}");
    let cue = markers(&mut tb, "cue");
    // hook 0.5–1.7, pause 0.4: s01 at 2.1, Iran at 2.1 + 1.6.
    assert_eq!(cue["cue"][0]["t"], 3.7, "{cue:#}");
    assert_eq!(markers(&mut tb, "line")["line"][0]["id"], "hook");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn imports_are_content_addressed_and_idempotent() {
    let (mut tb, dir) = toolbox("assets-cas");
    tb.call("project_create", json!({"path": "p", "duration": 2, "width": 320, "height": 180})).unwrap();
    let ff = |name: &str, color: &str| {
        assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("color=c={color}:s=16x16"), "-frames:v", "1"]).arg(dir.join(name)).status().unwrap().success());
    };
    std::fs::create_dir_all(dir.join("g1")).unwrap();
    std::fs::create_dir_all(dir.join("g2")).unwrap();
    ff("g1/map.png", "red");
    ff("g2/map.png", "blue");
    let a = tb.call("media_import", json!({"path": "g1/map.png"})).unwrap().json;
    let again = tb.call("media_import", json!({"path": "g1/map.png"})).unwrap().json;
    let b = tb.call("media_import", json!({"path": "g2/map.png"})).unwrap().json;
    assert_eq!(a["src"], "assets/map.png");
    assert_eq!(again["src"], "assets/map.png", "the same file twice is a no-op");
    let b = b["src"].as_str().unwrap();
    assert!(b.starts_with("assets/map-") && b.ends_with(".png"), "same name, other bytes: a distinct path, not an error ({b})");
    // Both files are intact.
    let red = image::open(dir.join("p.lahza").join("assets/map.png")).unwrap().to_rgba8();
    let blue = image::open(dir.join("p.lahza").join(b)).unwrap().to_rgba8();
    assert!(red.get_pixel(0, 0)[0] > 200 && blue.get_pixel(0, 0)[2] > 200);
    let third = tb.call("media_import", json!({"path": "g2/map.png"})).unwrap().json;
    assert_eq!(third["src"].as_str().unwrap(), b);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dropped_assets_are_errors_and_check_summarises_causes() {
    let (mut tb, dir) = toolbox("dropped");
    tb.call("project_create", json!({"path": "p", "duration": 1, "width": 320, "height": 180})).unwrap();
    assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=32x32", "-frames:v", "1"]).arg(dir.join("p.lahza/assets/pic.png")).status().unwrap().success());
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "id": "pic", "kind": "image", "src": "assets/pic.png"},
        {"op": "add", "id": "t1", "kind": "text", "text": "Hi", "font": "fonts/Missing.ttf", "transform": {"position": [160, 90]}},
        {"op": "add", "id": "a", "kind": "text", "text": "A", "color": "#16181c", "transform": {"position": [60, 90]}},
        {"op": "add", "id": "b", "kind": "text", "text": "B", "color": "#16181c", "transform": {"position": [260, 90]}},
        // White on white, but outlined: readable.
        {"op": "add", "id": "cap", "kind": "text", "text": "Caption", "color": "#ffffff", "size": 40, "stroke": {"color": "#000000", "width": 6}, "transform": {"position": [160, 150]}}
    ]})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "set", "target": "s1.background", "value": "#000000"}]})).unwrap();
    let r = tb.call("check", json!({})).unwrap().json;
    let msgs: Vec<String> = r["issues"].as_array().unwrap().iter().map(|i| format!("{} {}", i["severity"], i["message"])).collect();
    assert!(msgs.iter().any(|m| m.contains("error") && m.contains("Missing.ttf")), "a missing font file is an error: {msgs:?}");
    assert!(!msgs.iter().any(|m| m.contains("contrast") && m.contains("cap")), "{msgs:?}");
    // Two texts with the same problem: one cause, counted twice.
    let low = r["causes"].as_array().unwrap().iter().find(|c| c["what"].as_str().unwrap().contains("contrast")).expect("contrast cause").clone();
    assert_eq!(low["count"], 2, "{low}");
    // A deleted image: an error in check and in the snapshot.
    std::fs::remove_file(dir.join("p.lahza/assets/pic.png")).unwrap();
    let r = tb.call("check", json!({})).unwrap().json;
    assert!(r["issues"].as_array().unwrap().iter().any(|i| i["severity"] == "error" && i["message"].as_str().unwrap().contains("pic.png")), "{r:#}");
    let snap = tb.call("snapshot", json!({"at": [0.5]})).unwrap().json;
    assert_eq!(snap["ok"], false, "{snap:#}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sync_check_flags_hits_with_nothing_to_hit() {
    let (mut tb, dir) = toolbox("synccheck");
    let rec = dir.join("talk.lahzarec");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(
        rec.join("edit.json"),
        json!({"formatVersion": 5, "transcript": {"words": [
            {"text": " The", "start": 0.4, "end": 0.6},
            {"text": " canal", "start": 0.7, "end": 1.1},
            {"text": " closed.", "start": 1.15, "end": 1.6}
        ], "cues": [], "language": "en"}})
        .to_string(),
    )
    .unwrap();
    tb.call("project_create", json!({"path": "sync", "duration": 4, "width": 640, "height": 360})).unwrap();
    tb.call("markers_import", json!({"kind": "transcript", "path": "talk.lahzarec", "offset": 1.0})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "id": "label", "kind": "text", "text": "Canal", "size": 48, "color": "#ffffff",
         "transform": {"position": [320, 180], "opacity": {"keys": [{"at": "word:canal", "from": 0, "v": 1, "dur": "6f", "anchor": "start"}]}}},
        {"op": "add", "id": "pop", "kind": "shape", "shape": "circle", "size": 40, "fill": "#ff0000",
         "transform": {"position": [100, 100], "scale": {"keys": [{"at": 2.4, "v": [0, 0]}, {"at": 2.6, "v": [1, 1]}]}}}
    ]}))
    .unwrap();
    let r = tb.call("sync_check", json!({"flagged": true})).unwrap().json;
    let flagged: Vec<&str> = r["events"].as_array().unwrap().iter().map(|e| e["element"].as_str().unwrap()).collect();
    assert_eq!(flagged, ["pop"], "{r:#}");
    assert_eq!(r["events"][0]["flag"], "unsynced");
    assert_eq!(r["events"][0]["nearest"]["marker"], "word:3", "{r:#}");
    let all = tb.call("sync_check", json!({"element": "label"})).unwrap().json;
    let label = &all["events"].as_array().unwrap().iter().find(|e| e["reference"] == "word:canal").unwrap();
    assert_eq!(label["drift_frames"], 0.0, "{all:#}");
    std::fs::remove_dir_all(&dir).ok();
}

/// `snapshot pt=true` without ray queries draws the raster and says so.
#[test]
fn a_path_traced_snapshot_without_ray_queries_warns() {
    let (mut tb, dir) = toolbox("pt");
    tb.call("project_create", json!({"path": "still", "duration": 1, "width": 320, "height": 180})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "scene": "s1", "layer": {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 40}, "transform": {"position": [160, 90]}}}
    ]})).unwrap();
    let raster = tb.call("snapshot", json!({"at": [0.5], "width": 320})).unwrap();
    assert!(raster.json.get("warnings").is_none(), "{}", raster.json);
    let traced = tb.call("snapshot", json!({"at": [0.5], "width": 320, "pt": true, "samples": 4})).unwrap();
    assert!(traced.json["warnings"].to_string().contains("path tracing needs a GPU with ray queries"), "{}", traced.json);
    assert_eq!(traced.images, raster.images);
    let again = tb.call("snapshot", json!({"at": [0.5], "width": 320})).unwrap();
    assert!(again.json.get("warnings").is_none(), "the warning was about the traced snapshot: {}", again.json);
    assert!(tb.call("snapshot", json!({"at": [0.5], "pt": "yes"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// fit_camera: marks seen through a known camera give that camera back,
/// as `set` ops that, applied, put the scene's camera there.
#[test]
fn fit_camera_solves_and_sets_the_scene_camera() {
    use lahza_motion_core::camfit::{project, CameraParams};
    let (mut tb, dir) = toolbox("fitcam");
    tb.call("project_create", json!({"path": "shot", "duration": 2, "width": 1920, "height": 1080})).unwrap();
    tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}]})).unwrap();
    let truth = CameraParams { fov: 45.0, orbit: [12.0, -8.0], dolly: 200.0, position: Some([1000.0, 500.0]), shift: [0.0, 0.0] };
    let pts = [[300.0, 200.0, 0.0], [1500.0, 260.0, 120.0], [900.0, 800.0, -200.0], [400.0, 900.0, 300.0], [1700.0, 950.0, 0.0], [960.0, 500.0, 600.0]];
    let marks: Vec<_> = pts.iter().map(|&p| json!({"world": p, "image": project(&truth, 1920.0, 1080.0, p).unwrap()})).collect();
    let out = tb.call("fit_camera", json!({"frames": [{"at": 0.5, "marks": marks}], "scene": "s1", "apply": true})).unwrap().json;
    assert!(out["frames"][0]["rms_px"].as_f64().unwrap() < 0.5, "{out}");
    assert_eq!(out["applied"], true);
    let scene: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("shot.lahza/scenes/s1.json")).unwrap()).unwrap();
    let fov = scene["camera"]["fov"].as_f64().unwrap();
    let orbit = scene["camera"]["orbit"].clone();
    assert!((fov - 45.0).abs() < 0.5 && (orbit[0].as_f64().unwrap() - 12.0).abs() < 0.5, "the scene's camera: {}", scene["camera"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A synthetic reference (lossless, 30 fps, 320 × 180): a checker A, a hard
/// cut to stripes B at 1 s, a one-frame white flash at 1.5 s, a dissolve
/// B → A over 2.0–2.5 s, a blurred whip A → B at 3.0–3.2 s; a box moving
/// 3.6–4.0 s; captions (bars in the bottom band) 4.2–4.8 s and 4.8–5.4 s.
fn reference_video(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping: ffmpeg is not installed");
        return None;
    }
    let (w, h) = (320u32, 180u32);
    let a = |x: u32, y: u32| if (x / 20 + y / 20) % 2 == 0 { 230.0 } else { 30.0 };
    let b = |x: u32, _y: u32| if (x / 10) % 2 == 0 { 200.0 } else { 60.0 };
    let frames_dir = dir.join("frames");
    std::fs::create_dir_all(&frames_dir).unwrap();
    for f in 0..170u32 {
        let t = f as f64 / 30.0;
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            let v: f64 = if t < 1.0 {
                a(x, y)
            } else if f == 45 {
                255.0
            } else if t < 2.0 {
                b(x, y)
            } else if t < 2.5 {
                let k = (t - 2.0) / 0.5;
                b(x, y) * (1.0 - k) + a(x, y) * k
            } else if t < 3.0 || (3.6..).contains(&t) {
                // A (and the still background later), with a box that moves.
                let mut v = a(x, y);
                let bx = if t < 3.6 { 40.0 } else if t < 4.0 { 40.0 + (t - 3.6) / 0.4 * 120.0 } else { 160.0 };
                if (bx..bx + 30.0).contains(&(x as f64)) && (20..50).contains(&y) {
                    v = 128.0;
                }
                // Captions: bars on a plain band at the bottom.
                if t >= 3.6 && y >= 140 {
                    v = 90.0;
                }
                if (4.2..4.8).contains(&t) && (150..165).contains(&y) && (60..260).contains(&x) && (x / 8) % 3 != 0 {
                    v = 255.0;
                }
                if (4.8..5.4).contains(&t) && (150..165).contains(&y) && (90..230).contains(&x) && (x / 6) % 2 == 0 {
                    v = 255.0;
                }
                v
            } else if t < 3.2 {
                // Whip: a horizontally smeared slide from A to B.
                let k = (t - 3.0) / 0.2;
                let shift = (k * w as f64) as i64;
                let mut s = 0.0;
                for d in -12..=12i64 {
                    let xx = x as i64 + shift + d;
                    s += if xx < w as i64 { a(xx.max(0) as u32 % w, y) } else { b((xx - w as i64) as u32 % w, y) };
                }
                s / 25.0
            } else {
                b(x, y)
            };
            let v = v as u8;
            image::Rgb([v, v, v])
        });
        img.save(frames_dir.join(format!("f{f:04}.png"))).unwrap();
    }
    let out = dir.join("ref.mkv");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-framerate", "30", "-i"])
        .arg(frames_dir.join("f%04d.png"))
        .args(["-c:v", "ffv1"])
        .arg(&out)
        .status()
        .unwrap()
        .success();
    ok.then_some(out)
}

#[test]
fn measure_finds_cuts_events_ink_and_captions() {
    let (mut tb, dir) = toolbox("measure");
    let Some(video) = reference_video(&dir) else { return };
    let v = video.to_str().unwrap();
    let cuts = tb.call("measure", json!({"video": v, "kind": "cuts"})).unwrap().json;
    let ev = cuts["events"].as_array().unwrap();
    let find = |kind: &str, t: f64| ev.iter().any(|e| e["kind"] == kind && (e["t"].as_f64().unwrap() - t).abs() < 0.1);
    assert!(find("cut", 1.0), "a cut at 1 s: {cuts}");
    assert!(find("flash", 1.5), "a flash at 1.5 s: {cuts}");
    assert!(find("dissolve", 2.0), "a dissolve from 2 s: {cuts}");
    assert!(find("whip", 3.0), "a whip at 3 s: {cuts}");
    let events = tb.call("measure", json!({"video": v, "kind": "events", "from": 3.4, "to": 4.1})).unwrap().json;
    let e = &events["events"][0];
    assert!((e["start"].as_f64().unwrap() - 3.6).abs() < 0.1 && (e["end"].as_f64().unwrap() - 4.0).abs() < 0.1, "{events}");
    let caps = tb.call("measure", json!({"video": v, "kind": "captions", "from": 4.0, "to": 5.6})).unwrap().json;
    let c = caps["captions"].as_array().unwrap();
    assert_eq!(c.len(), 2, "{caps}");
    assert!((c[0]["start"].as_f64().unwrap() - 4.2).abs() < 0.1 && (c[1]["start"].as_f64().unwrap() - 4.8).abs() < 0.1, "{caps}");
    let ink = tb.call("measure", json!({"video": v, "kind": "ink", "at": 4.5, "region": [40, 140, 240, 35]})).unwrap().json;
    assert_eq!(ink["lines"][0]["height_px"], 15, "{ink}");
    // curve: the box's move, tracked and fitted.
    let curve = tb.call("measure", json!({"video": v, "kind": "curve", "box": [40, 20, 30, 30], "search": 40, "from": 3.4, "to": 4.2})).unwrap().json;
    assert!(curve.is_object() && !curve.as_object().unwrap().is_empty(), "{curve}");
    let _ = std::fs::remove_dir_all(&dir);
}
