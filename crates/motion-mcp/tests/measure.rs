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

// ------------------------------------------------------------- shading

use lahza_motion_mcp::measure::{flatness_verdict, shading, shading_ratio};

const BG: [f64; 3] = [16.0, 16.0, 20.0];

/// A frame `w` wide (16:9) with discs (centre x, y as fractions of the
/// frame, radius as a fraction of its height) painted by `paint(k, nx, ny)`,
/// (nx, ny) being the point on disc k's unit disc.
fn discs(w: u32, centres: &[(f64, f64, f64)], paint: &dyn Fn(usize, f64, f64) -> [f64; 3]) -> image::RgbaImage {
    let h = w * 9 / 16;
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([BG[0] as u8, BG[1] as u8, BG[2] as u8, 255]));
    for (k, &(cx, cy, r)) in centres.iter().enumerate() {
        let (cx, cy, r) = (cx * w as f64, cy * h as f64, r * h as f64);
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = ((x as f64 + 0.5 - cx) / r, (y as f64 + 0.5 - cy) / r);
                if nx * nx + ny * ny <= 1.0 {
                    let c = paint(k, nx, ny).map(|v| v.round().clamp(0.0, 255.0) as u8);
                    img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
        }
    }
    img
}

const BASE: [f64; 3] = [235.0, 140.0, 70.0];
const ONE: &[(f64, f64, f64)] = &[(0.5, 0.5, 0.36)];
const KEY: [f64; 3] = [-0.5, -0.6, 0.65];

fn flat(_: usize, _: f64, _: f64) -> [f64; 3] {
    BASE
}

/// A sphere lit by a key from `light` (Lambert, ambient and a soft specular).
fn lit(light: [f64; 3]) -> impl Fn(usize, f64, f64) -> [f64; 3] {
    let n = (light[0] * light[0] + light[1] * light[1] + light[2] * light[2]).sqrt();
    let l = [light[0] / n, light[1] / n, light[2] / n];
    move |_, x, y| {
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        let d = (x * l[0] + y * l[1] + z * l[2]).max(0.0);
        let spec = (2.0 * d * z - l[2]).max(0.0).powf(12.0) * 80.0;
        BASE.map(|c| c * (0.18 + 0.82 * d) + spec)
    }
}

/// Lit, then quantised to 5 flat bands: posterised polygons traced from a
/// lit render.
fn posterised(light: [f64; 3]) -> impl Fn(usize, f64, f64) -> [f64; 3] {
    let f = lit(light);
    move |k, x, y| f(k, x, y).map(|c| ((c / 255.0 * 4.0).round() / 4.0 * 255.0).min(255.0))
}

fn verdict(ours: &image::RgbaImage, reference: &image::RgbaImage) -> (Option<f64>, Option<f64>, Option<String>) {
    let (o, r) = (shading(ours), shading(reference));
    (o, r, flatness_verdict(shading_ratio(o, r)))
}

/// A flat disc against a lit sphere, or against a directionally shaded
/// disc, is flagged at the widths compare works at.
#[test]
fn shading_flags_a_flat_disc_against_a_lit_one() {
    for w in [270, 360, 720] {
        let ours = discs(w, ONE, &flat);
        let reference = discs(w, ONE, &lit(KEY));
        let (o, r, v) = verdict(&ours, &reference);
        assert!(r.unwrap() > 2.0 && o.unwrap() < 0.1, "w {w}: ours {o:?}, reference {r:?}");
        let v = v.unwrap_or_else(|| panic!("w {w}: not flagged ({o:?} vs {r:?})"));
        assert!(v.starts_with("ours is flatter than the reference (shading 0%"), "{v}");
        // Light from the top left falling off to the bottom right.
        let ramp = discs(w, ONE, &|_, x, y| BASE.map(|c| c * (0.95 - 0.3 * (x + y))));
        assert!(verdict(&ours, &ramp).2.is_some(), "w {w}: directional shading not flagged: {:?}", verdict(&ours, &ramp));
    }
}

/// Posterised bands read flat: their steps are edges, their insides flat.
#[test]
fn shading_reads_posterised_bands_as_flat() {
    let reference = discs(360, ONE, &lit(KEY));
    let ours = discs(360, ONE, &posterised(KEY));
    let (o, r, v) = verdict(&ours, &reference);
    assert!(v.is_some(), "posterised {o:?} vs lit {r:?}");
}

/// Both lit, from another side and at another size: not flagged.
#[test]
fn shading_does_not_flag_lit_against_lit() {
    let reference = discs(360, ONE, &lit(KEY));
    let ours = discs(360, &[(0.45, 0.55, 0.3)], &lit([0.6, -0.3, 0.7]));
    let (o, r, v) = verdict(&ours, &reference);
    let ratio = shading_ratio(o, r).unwrap();
    assert!(v.is_none() && ratio > 0.6 && ratio < 1.6, "ours {o:?}, reference {r:?}");
}

/// Flat 2D graphics on both sides, and line art on both sides: the
/// reference has no meaningful shading, so nothing is flagged.
#[test]
fn shading_ignores_flat_graphics_and_line_art() {
    let palette = [[235.0, 140.0, 70.0], [60.0, 160.0, 240.0], [240.0, 230.0, 220.0]];
    let reference = discs(360, &[(0.3, 0.4, 0.25), (0.7, 0.6, 0.3), (0.5, 0.75, 0.15)], &|k, _, _| palette[k]);
    let ours = discs(360, &[(0.32, 0.42, 0.25), (0.68, 0.58, 0.28)], &|k, _, _| palette[(k + 1) % 3]);
    let (o, r, v) = verdict(&ours, &reference);
    assert!(v.is_none() && r.unwrap() < 0.2, "flat vs flat: {o:?} {r:?} {v:?}");
    // Line art: a grid and a circle outline, 1–2 px wide.
    let lines = |w: u32, step: u32, shift: u32| {
        let h = w * 9 / 16;
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([16, 16, 20, 255]));
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f64 - w as f64 / 2.0, y as f64 - h as f64 / 2.0);
                let ring = ((dx * dx + dy * dy).sqrt() - h as f64 * 0.35).abs() < 1.0;
                if (x + shift) % step == 0 || (y + shift) % step == 0 || ring {
                    img.put_pixel(x, y, image::Rgba([230, 230, 240, 255]));
                }
            }
        }
        img
    };
    let (o, r, v) = verdict(&lines(360, 24, 0), &lines(360, 30, 7));
    assert!(v.is_none(), "line art vs line art: {o:?} {r:?} {v:?}");
    // Against line art, even a filled replica is not "flatter".
    assert!(verdict(&discs(360, ONE, &flat), &lines(360, 30, 7)).2.is_none());
}
