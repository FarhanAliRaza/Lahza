//! Phase 1 benchmark: an original 20 s kinetic-type promo synced to a
//! supplied music track, built only through the tool interface the agent
//! uses (the same calls `lahza mcp` serves). `check` must report no sync
//! drift over ±1 frame. The draft render needs ffmpeg.

use lahza_motion_mcp::tools::Toolbox;
use lahza_motion_render::Backend;
use serde_json::{json, Value};
use std::path::Path;

const SR: u32 = 44100;
const BPM: f64 = 124.0;
const START: f64 = 0.35;

/// An original track: 124 BPM four-on-the-floor with hats, a bass line, a
/// stop (half a bar of silence) before bar 7, and a final impact.
fn write_music(path: &Path) -> Vec<f64> {
    let beat = 60.0 / BPM;
    let len = (20.5 * SR as f64) as usize;
    let mut buf = vec![0.0f32; len];
    let mut seed = 7u64;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    };
    let stop_from = START + 23.0 * beat;
    let stop_to = START + 24.0 * beat;
    let add = |buf: &mut Vec<f32>, t: f64, dur: f64, f: &mut dyn FnMut(f32) -> f32| {
        let s = (t * SR as f64) as usize;
        for i in 0..(dur * SR as f64) as usize {
            if let Some(v) = buf.get_mut(s + i) {
                *v += f(i as f32 / SR as f32);
            }
        }
    };
    let mut beats = Vec::new();
    for k in 0..38 {
        let t = START + k as f64 * beat;
        beats.push(t);
        if t >= stop_from - 1e-6 && t < stop_to - 1e-6 {
            continue;
        }
        let mut phase = 0.0f32;
        let amp = if k % 4 == 0 { 0.8 } else { 0.55 };
        add(&mut buf, t, 0.3, &mut |s| {
            phase += std::f32::consts::TAU * (48.0 + 80.0 * (-s * 35.0).exp()) / SR as f32;
            amp * phase.sin() * (-s * 10.0).exp()
        });
        let mut prev = 0.0;
        let mut hat = |s: f32| {
            let n = noise();
            let hp = n - prev;
            prev = n;
            0.18 * hp * (-s * 70.0).exp()
        };
        add(&mut buf, t + beat / 2.0, 0.05, &mut hat);
        let note = [55.0f32, 55.0, 65.4, 49.0][(k / 4) % 4];
        add(&mut buf, t, beat * 0.9, &mut |s| 0.12 * (std::f32::consts::TAU * note * s).sin() * (1.0 - s / (beat as f32 * 0.9)));
    }
    // Silence for the stop, then the big hit on bar 7's downbeat.
    let (a, b) = ((stop_from * SR as f64) as usize, (stop_to * SR as f64) as usize);
    buf[a..b].iter_mut().for_each(|v| *v = 0.0);
    let mut prev = 0.0;
    add(&mut buf, stop_to, 0.6, &mut |s| {
        let n = noise();
        let hp = n - prev;
        prev = n;
        0.9 * hp * (-s * 12.0).exp() + 0.9 * (std::f32::consts::TAU * 50.0 * s).sin() * (-s * 6.0).exp()
    });
    let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let mut bytes = Vec::with_capacity(44 + len * 2);
    let data_len = (len * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&SR.to_le_bytes());
    bytes.extend_from_slice(&(SR * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for v in buf {
        bytes.extend_from_slice(&((v / peak * 0.9 * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
    beats
}

fn call(tb: &mut Toolbox, name: &str, args: Value) -> Value {
    match tb.call(name, args.clone()) {
        Ok(out) => out.json,
        Err(e) => panic!("{name} failed: {e}\nargs: {args:#}"),
    }
}

/// The ops an agent writes from the visual script:
/// | section | words / visual | anchor |
/// | bars 1–2 | MAKE / IT / MOVE, per-glyph entrances | beats 1, 3, 5 |
/// | bars 3–4 | eight dots pop in a row, then a bar wipes | beats 9…16, peak on beat 13 |
/// | bars 5–6 | SYNC THE PEAK, line draws on, camera push | bar 5, peak on beat 21 |
/// | stop → bar 7 | LAHZA slams on the hit, drifts to the end | hit after the stop |
fn promo_ops(largest_hit: &str) -> Vec<Value> {
    let glyphs = |id: &str, text: &str, at: &str, y: f64| {
        json!({"op": "add", "id": id, "kind": "text", "text": text, "size": 190, "color": "#ffffff",
            "transform": {"position": [960, y], "scale": {"keys": [{"at": at, "from": 1.0, "v": 1.0}], "add": [{"type": "drift", "rate": 0.0015, "per": "frame", "mode": "mul", "from": at}]}},
            "time": {"start": at},
            "effects": [{"type": "shadow", "color": "#00000099", "offset": [0, 10], "radius": 10}],
            "animators": [{"at": at, "anchor": "start", "stagger": "1f", "dur": "8f", "ease": "enter", "from": {"y": 120, "opacity": 0, "scale": 0.7}}]})
    };
    let mut ops = vec![
        json!({"op": "add_scene", "id": "open", "start": 0, "end": "bar:3"}),
        glyphs("make", "MAKE", "beat:1", 330.0),
        glyphs("it", "IT", "beat:3", 540.0),
        glyphs("move", "MOVE", "beat:5", 750.0),
        json!({"op": "add_scene", "id": "dots", "start": "bar:3", "end": "bar:5", "background": "#d7ff3a"}),
    ];
    for i in 0..8 {
        ops.push(json!({"op": "add", "scene": "dots", "id": format!("dot{i}"), "kind": "shape", "shape": "circle", "size": 120, "fill": "#0b0b0b",
            "transform": {"position": [330 + i * 180, 540], "scale": {"keys": [{"at": format!("beat:{}", 9 + i), "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]}}}));
    }
    ops.push(json!({"op": "add", "scene": "dots", "id": "wipe", "kind": "shape", "shape": "rect", "size": [1920, 1080], "fill": "#0b0b0b",
        "transform": {"position": {"keys": [{"at": 0, "v": [-960, 540]}, {"at": "beat:16", "v": [960, 540], "ease": "gesture", "dur": "10f", "anchor": "peak", "lead": -1}]}}}));
    ops.extend([
        json!({"op": "add_scene", "id": "peak", "start": "bar:5", "end": "stop:1",
            "transition": {"type": "whip", "dur": "8f", "direction": "left"},
            "camera": {"zoom": {"keys": [{"at": "bar:5", "v": 1.0}, {"at": "stop:1", "v": 1.06, "ease": "push"}]}}}),
        json!({"op": "add", "scene": "peak", "id": "sync", "kind": "text", "text": "SYNC THE PEAK", "size": 150, "color": "#d7ff3a",
            "transform": {"position": [960, 480]},
            "animators": [{"at": "bar:5", "unit": "word", "stagger": "2f", "dur": "10f", "ease": "glide", "from": {"x": -60, "opacity": 0}}]}),
        json!({"op": "add", "scene": "peak", "id": "line", "kind": "shape", "shape": "line", "size": [1100, 0], "thickness": 14, "fill": "#ffffff",
            "transform": {"position": [960, 640]},
            "trim": {"keys": [{"at": 0, "v": [0, 0]}, {"at": "beat:21", "v": [0, 1], "ease": "gesture", "dur": "12f", "anchor": "peak"}]}}),
        // A swoosh drawn on through a soft matte (path + track matte).
        json!({"op": "add", "scene": "peak", "id": "swoosh-matte", "kind": "shape", "shape": "rect", "size": [1920, 300],
            "fill": {"gradient": "linear", "from": [-960, 0], "to": [960, 0], "stops": [[0, "#ffffff00"], [0.3, "#ffffff"], [1, "#ffffff"]]},
            "transform": {"position": [960, 820]}}),
        json!({"op": "add", "scene": "peak", "id": "swoosh", "kind": "shape", "shape": "path", "d": "M -700 40 C -300 -120 300 160 700 -40",
            "stroke": {"color": "#d7ff3a", "width": 10}, "transform": {"position": [960, 820]},
            "matte": {"layer": "swoosh-matte", "mode": "alpha"},
            "trim": {"keys": [{"at": "beat:22", "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": "16f", "anchor": "start"}]}}),
        json!({"op": "add_scene", "id": "logo", "start": largest_hit, "end": 20.0, "background": "#0b0b0b"}),
        json!({"op": "add", "scene": "logo", "id": "mark", "kind": "shape", "shape": "ring", "size": 420, "thickness": 18, "fill": "#d7ff3a",
            "transform": {"position": [960, 540], "scale": {"keys": [{"at": largest_hit, "from": 0.4, "v": 1, "ease": "land", "anchor": "start"}], "add": [{"type": "drift", "rate": 0.003, "per": "frame", "mode": "mul", "from": largest_hit}]}},
            "effects": [{"type": "glow", "color": "#d7ff3a", "radius": 18, "intensity": 0.9}],
            "trim": {"keys": [{"at": largest_hit, "from": [0, 0], "v": [0, 1], "ease": "settle", "dur": "20f", "anchor": "start"}]}}),
        json!({"op": "add", "scene": "logo", "id": "name", "kind": "text", "text": "LAHZA", "size": 120, "color": "#ffffff",
            "transform": {"position": [960, 540], "scale": {"keys": [{"at": largest_hit, "from": 2.2, "v": 1, "ease": "slam", "anchor": "end"}], "add": [{"type": "drift", "rate": 0.002, "per": "frame", "mode": "mul", "from": largest_hit}]},
                "opacity": {"keys": [{"at": largest_hit, "from": 0, "v": 1, "ease": "slam", "anchor": "end"}]}}}),
    ]);
    ops
}

#[test]
fn twenty_second_kinetic_promo_synced_to_music() {
    let dir = std::env::temp_dir().join(format!("lahza-promo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let music = dir.join("track.wav");
    let beats = write_music(&music);
    let ffmpeg = std::process::Command::new("ffmpeg").arg("-version").output().is_ok();
    if !ffmpeg {
        eprintln!("skipping: ffmpeg is needed to decode music");
        return;
    }
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    tb.backend = Backend::Cpu;
    call(&mut tb, "project_create", json!({"path": "promo", "duration": 20, "fps": 30, "style": "promo.kinetic", "music": "track.wav"}));
    let analysis = call(&mut tb, "audio_analyze", json!({}));
    assert!((analysis["bpm"].as_f64().unwrap() - BPM).abs() < 0.5, "{analysis:#}");
    assert!((analysis["first_downbeat"].as_f64().unwrap() - START).abs() < 1.0 / 30.0, "{analysis:#}");
    let markers = call(&mut tb, "markers_query", json!({"track": ["beat", "hit", "stop"]}));
    let beat17 = markers["beat"][16]["t"].as_f64().unwrap();
    assert!((beat17 - beats[16]).abs() < 0.015, "beat 17 at {beat17}, music has it at {}", beats[16]);
    let largest = markers["hit"].as_array().unwrap().iter().find(|h| h["label"] == "largest").unwrap()["ref"].as_str().unwrap().to_string();
    assert_eq!(markers["stop"].as_array().unwrap().len(), 1, "{markers:#}");
    let applied = call(&mut tb, "edit_apply", json!({"ops": promo_ops(&largest)}));
    assert_eq!(applied["errors"], json!([]), "{applied:#}");

    let report = call(&mut tb, "check", json!({"verbose": true}));
    assert_eq!(report["ok"], true, "{report:#}");
    assert!(report["max_drift_frames"].as_f64().unwrap() <= 1.0, "{report:#}");
    assert!(report["sync_count"].as_u64().unwrap() >= 20, "{report:#}");
    let timeline = call(&mut tb, "timeline_get", json!({}));
    assert_eq!(timeline["scenes"].as_array().unwrap().len(), 4);

    let sheet = tb.call("snapshot", json!({"sheet": true, "width": 240})).unwrap();
    assert_eq!(sheet.images.len(), 1);
    let frames = sheet.json["frames"].as_array().unwrap().len();
    assert!(frames >= 8, "{}", sheet.json);
    if let Ok(keep) = std::env::var("LAHZA_PROMO_OUT") {
        std::fs::write(Path::new(&keep).join("promo-sheet.png"), &sheet.images[0]).unwrap();
    }

    // Undo the whole build, then redo it through the history.
    let history = call(&mut tb, "history", json!({"undo": 1}));
    assert_eq!(history["undone"], json!([1]));
    assert_eq!(call(&mut tb, "timeline_get", json!({}))["scenes"], json!([]));
    call(&mut tb, "edit_apply", json!({"ops": promo_ops(&largest)}));

    // Draft render of the first 3 s with audio.
    let out = call(&mut tb, "render", json!({"quality": "draft", "from": 0, "to": 3, "wait": true}));
    assert_eq!(out["state"], "done", "{out:#}");
    let path = out["output"].as_str().unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,width,height", "-of", "json", path])
        .output()
        .unwrap();
    let streams: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let kinds: Vec<&str> = streams["streams"].as_array().unwrap().iter().map(|s| s["codec_type"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"video") && kinds.contains(&"audio"), "{streams}");
    eprintln!("draft render: {}", out["report"]);
    if let Ok(keep) = std::env::var("LAHZA_PROMO_OUT") {
        let full = call(&mut tb, "render", json!({"wait": true, "output": Path::new(&keep).join("promo.mp4")}));
        eprintln!("full render: {:#}", full["report"]);
    } else {
        std::fs::remove_dir_all(&dir).ok();
    }
}
