//! track and fit_curve on synthetic videos made with ffmpeg's lavfi
//! sources (lossless, so colours stay exact). Skips without ffmpeg.

use lahza_motion_mcp::tools::Toolbox;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

fn make(dir: &Path, name: &str, graph: &str) -> String {
    let path = dir.join(name);
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-filter_complex", graph, "-c:v", "ffv1", "-pix_fmt", "bgr0"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg {graph}");
    path.display().to_string()
}

fn setup(name: &str) -> Option<(Toolbox, std::path::PathBuf)> {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping: no ffmpeg");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("lahza-measure-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    Some((tb, dir))
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

/// A red 60×16 bar sliding right at 100 px/s while turning clockwise at
/// 0.5 rad/s: colour tracking measures its centre, size and angle, and
/// returns keys; fit_curve names the slide linear.
#[test]
fn colour_track_measures_a_turning_bar_and_fits_its_move() {
    let Some((mut tb, dir)) = setup("colour") else { return };
    // Pixel (X, Y) is red when it lies in the bar centred at (80 + 100 T, 120)
    // turned by 0.5 T rad: |along| ≤ 30, |across| ≤ 8.
    let bar = "255*lte(abs((X+0.5-80-100*T)*cos(0.5*T)+(Y+0.5-120)*sin(0.5*T)),30)*lte(abs((Y+0.5-120)*cos(0.5*T)-(X+0.5-80-100*T)*sin(0.5*T)),8)";
    let video = make(&dir, "bar.mkv", &format!("color=c=black:s=320x240:r=25:d=2,format=rgb24,geq=r='{bar}':g=0:b=0"));
    let out = tb.call("track", json!({"video": video, "color": "#ff0000", "tolerance": 60, "keys": {"scale": 2.0, "at": 1.0, "layer": "bar"}})).unwrap().json;
    assert_eq!(out["frames"], 50, "{}", out["frames"]);
    assert_eq!(out["found"], 50);
    let rows = out["track"].as_array().unwrap();
    for r in rows {
        let t = f(&r["t"]);
        // Pixel-sampled edges: within 2 px of 80 + 100 t.
        assert!((f(&r["x"]) - (80.0 + 100.0 * t)).abs() <= 2.0, "{r}");
        assert!((f(&r["y"]) - 120.0).abs() <= 2.0, "{r}");
        assert!((f(&r["angle"]) - (0.5 * t).to_degrees()).abs() <= 2.0, "{r}");
        assert!((f(&r["area"]) - 960.0).abs() < 60.0, "{r}");
        assert!(r["color"] == "#ff0000", "{r}");
    }
    // Keys in canvas pixels (×2), shifted to start at 1 s on the timeline.
    let position = out["keys"]["transform.position"]["keys"].as_array().unwrap();
    assert_eq!(position.len(), 50);
    assert_eq!(f(&position[0]["at"]), 1.0);
    assert!((f(&position[0]["v"][0]) - 160.0).abs() <= 2.0, "{}", position[0]);
    let rotation = out["keys"]["transform.rotation"]["keys"].as_array().unwrap();
    assert!((f(&rotation[49]["v"]) - (0.5f64 * 49.0 / 25.0).to_degrees()).abs() <= 2.0, "{}", rotation[49]);
    let ops = out["ops"].as_array().unwrap();
    assert!(ops.iter().any(|o| o["target"] == "bar.transform.position" && o["op"] == "set"), "{ops:?}");

    // A move of the measured centre fits as a linear ease (or its preset).
    let samples: Vec<Value> = rows.iter().map(|r| json!([r["t"], [r["x"], r["y"]]])).collect();
    let fit = tb.call("fit_curve", json!({"samples": samples, "fps": 25})).unwrap().json;
    let best = &fit["best"];
    assert!(["linear", "push"].contains(&best["family"].as_str().unwrap()), "{fit:#}");
    assert!(f(&best["relative_error"]) < 0.01, "{fit:#}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A colour-bars patch drifting diagonally over grey, followed from a box;
/// and a point on a green disc, followed as its colour blob.
#[test]
fn box_and_point_targets_follow_their_pictures() {
    let Some((mut tb, dir)) = setup("box") else { return };
    let video = make(&dir, "card.mkv", "color=c=gray:s=320x240:r=25:d=2[bg];smptehdbars=s=64x64:r=25:d=2[fg];[bg][fg]overlay=x=20+t*60:y=50+t*30:shortest=1");
    let out = tb.call("track", json!({"video": video, "box": [20, 50, 64, 64]})).unwrap().json;
    let rows = out["track"].as_array().unwrap();
    assert_eq!(rows.len(), 50);
    for r in rows {
        let t = f(&r["t"]);
        assert!((f(&r["x"]) - (52.0 + (60.0 * t).floor())).abs() <= 2.0, "{r}");
        assert!((f(&r["y"]) - (82.0 + (30.0 * t).floor())).abs() <= 2.0, "{r}");
    }
    let video = make(&dir, "disc.mkv", "color=c=black:s=320x240:r=25:d=1[bg];color=c=lime:s=30x30:r=25:d=1,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(lte(hypot(X-15,Y-15),15),255,0)'[fg];[bg][fg]overlay=x=200-t*80:y=100:shortest=1");
    let out = tb.call("track", json!({"video": video, "point": [215, 115]})).unwrap().json;
    let rows = out["track"].as_array().unwrap();
    assert_eq!(out["found"], rows.len());
    let last = rows.last().unwrap();
    let t = f(&last["t"]);
    assert!((f(&last["x"]) - (215.0 - (80.0 * t).ceil())).abs() <= 2.0, "{last}");
    std::fs::remove_dir_all(&dir).ok();
}
