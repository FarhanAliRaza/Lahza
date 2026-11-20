//! The fast mode (`TraceOptions::fast`), with a stand-in for NVIDIA's Ray
//! Reconstruction (`Tracer::pass_through_ngx`: the picture is the packed
//! colour image, enlarged) and without it (few paths a texel, accumulated
//! and denoised here). Pictures go to $LAHZA_PT_OUT when it is set.

use super::scene_tests::{still_life, tracer};
use super::TraceOptions;
use crate::tests::project;
use crate::{RenderOptions, Renderer};
use image::RgbaImage;
use lahza_motion_core::compile::Compiled;
use lahza_motion_core::document::Project;
use serde_json::json;

/// How the fast mode makes its picture in a test.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Way {
    /// The stand-in for Ray Reconstruction.
    StandIn,
    /// Without Ray Reconstruction.
    Own,
}

fn fast(way: Way, spp: u32, blur: bool) -> Option<Renderer> {
    tracer(1)?;
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let path = Some(TraceOptions { spp, bounces: 4, fast: true, temporal: true, ..Default::default() });
    let mut r = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, motion_blur: blur, path, ..Default::default() });
    let g = r.gpu.as_mut().unwrap();
    match way {
        Way::StandIn => g.pass_through_ngx(),
        Way::Own => g.tracer_for_tests().unwrap().without_ngx(),
    }
    Some(r)
}

fn keep(name: &str, img: &RgbaImage) {
    if let Some(dir) = std::env::var_os("LAHZA_PT_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        img.save(std::path::Path::new(&dir).join(format!("fast-{name}.png"))).unwrap();
    }
}

/// Peak signal to noise of `a` against `b`, in dB (their colours over black).
pub(super) fn psnr(a: &RgbaImage, b: &RgbaImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    let over = |p: &image::Rgba<u8>, k: usize| p.0[k] as f64 * p.0[3] as f64 / 255.0;
    let mse = a.pixels().zip(b.pixels()).map(|(p, q)| (0..3).map(|k| (over(p, k) - over(q, k)).powi(2)).sum::<f64>()).sum::<f64>() / (a.width() * a.height() * 3) as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
}

fn edge_width(img: &RgbaImage, y: u32, xs: std::ops::Range<u32>) -> usize {
    let v: Vec<f64> = xs.map(|x| img.get_pixel(x, y).0[0] as f64 * img.get_pixel(x, y).0[3] as f64 / 255.0).collect();
    let (lo, hi) = (v.iter().cloned().fold(f64::MAX, f64::min), v.iter().cloned().fold(0.0, f64::max));
    v.iter().filter(|c| **c > lo + 0.1 * (hi - lo) && **c < lo + 0.9 * (hi - lo)).count()
}

/// A red ball rolling 12 px a frame over a grey floor, under one light.
fn rolling() -> Project {
    let mut p = project(json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#d0d0d0"}, "transform": {"position": [80, 60]}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 15}, "material": {"preset": "matte", "color": "#ff2020"},
         "transform": {"position": {"keys": [{"at": 0, "v": [20, 60]}, {"at": 1, "v": [140, 60]}]}, "z": -30}}
    ]));
    p.settings.width = 160;
    p.settings.height = 120;
    p.settings.fps = lahza_motion_core::time::Fps::whole(10);
    p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [-0.5, -0.4, -1.0], "shadow": true}])).unwrap();
    p
}

/// What Ray Reconstruction is handed, frame after frame: the six images
/// at half the picture's size (colour with coverage, albedo, specular
/// colour, normal with roughness, depth, motion), where in its texel each
/// frame's samples went, and that only the first frame starts afresh.
#[test]
fn ray_reconstruction_is_handed_the_trace_as_it_asks() {
    let Some(mut r) = fast(Way::StandIn, 2, false) else { return };
    let c = Compiled::new(&rolling());
    // Where the ball's texels are (its albedo), middle and count.
    let ball_at = |r: &Renderer| {
        let (_, images, low) = r.gpu.as_ref().unwrap().stand_in_saw(0);
        let n = (low.0 * low.1) as usize;
        let at: Vec<usize> = (0..n).filter(|i| images[(n + i) * 4] > 0.6 && images[(n + i) * 4 + 1] < 0.2).collect();
        (at.iter().map(|i| (i % low.0 as usize) as f32 + 0.5).sum::<f32>() / at.len().max(1) as f32, at.len())
    };
    r.render(&c, 0.0);
    r.render(&c, 0.1);
    let before = ball_at(&r);
    r.render(&c, 0.2);
    let after = ball_at(&r);
    let (seen, images, low) = r.gpu.as_ref().unwrap().stand_in_saw(0);
    assert_eq!(low, (80, 60));
    assert_eq!(seen.iter().map(|s| s.1).collect::<Vec<_>>(), [true, false, false], "only the first frame has none before it");
    assert!(seen.iter().all(|(j, _)| j.iter().all(|v| v.abs() < 0.5)), "{seen:?}");
    assert!(seen[0].0 != seen[1].0 && seen[1].0 != seen[2].0, "the samples move within the texel: {seen:?}");
    let n = (low.0 * low.1) as usize;
    assert_eq!(images.len(), n * 19);
    let four = |image: usize, i: usize| [images[(image * n + i) * 4], images[(image * n + i) * 4 + 1], images[(image * n + i) * 4 + 2], images[(image * n + i) * 4 + 3]];
    let depth = |i: usize| images[16 * n + i];
    let motion = |i: usize| [images[17 * n + i * 2], images[17 * n + i * 2 + 1]];
    let (mut ball, mut floor) = (Vec::new(), Vec::new());
    for i in 0..n {
        let (colour, albedo, normal) = (four(0, i), four(1, i), four(3, i));
        assert!(depth(i) > 0.0 && depth(i) < 1e6, "the floor is behind everything: texel {i} at {}", depth(i));
        assert!((colour[3] - 1.0).abs() < 1e-3, "covered: {colour:?}");
        let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        assert!((len - 1.0).abs() < 0.02 && (0.0..=1.0).contains(&normal[3]), "a unit normal and a roughness: {normal:?}");
        if albedo[0] > 0.6 && albedo[1] < 0.2 {
            ball.push(i);
        } else if (albedo[0] - albedo[1]).abs() < 0.02 && albedo[0] > 0.5 {
            floor.push(i);
        }
    }
    // The ball: π · 7.5² texels of the half-size trace, or more (it is
    // nearer the camera than the layers' plane).
    assert!(ball.len() > 150 && ball.len() < 450 && floor.len() > 4000, "{} ball texels, {} floor", ball.len(), floor.len());
    assert_eq!(ball.len(), after.1);
    let mean = |v: &[usize], f: &dyn Fn(usize) -> f32| v.iter().map(|i| f(*i)).sum::<f32>() / v.len() as f32;
    // Motion: to where the surface was a frame ago, in the trace's
    // texels: as far as the ball's texels moved since that frame (12 px
    // of the layers' plane, six texels, or more as it is nearer).
    let (bx, by) = (mean(&ball, &|i| motion(i)[0]), mean(&ball, &|i| motion(i)[1]));
    let moved = after.0 - before.0;
    assert!(moved > 5.5 && moved < 9.0 && (bx + moved).abs() < 0.6 && by.abs() < 0.3, "the ball came from {moved:.2} texels to the left: ({bx:.2}, {by:.2})");
    let still = floor.iter().map(|i| motion(*i)[0].abs().max(motion(*i)[1].abs())).fold(0.0f32, f32::max);
    assert!(still < 0.05, "the floor stands still: {still}");
    assert!(mean(&ball, &depth) < mean(&floor, &depth), "the ball is nearer than the floor");
    // The specular guide: what a matte dielectric mirrors of an even
    // light (a few hundredths, more at a glancing angle), not nothing.
    let spec = mean(&floor, &|i| four(2, i)[1]);
    assert!(spec > 0.01 && spec < 0.3, "the floor's specular guide: {spec}");
    // The ball is at x 44 of 160 now: left of the middle of the 80 texels.
    assert!(after.0 > 12.0 && after.0 < 24.0, "the ball's texels are around x {:.1}", after.0);
}

/// The fast mode's picture is the traced one: against the usual trace at
/// many samples, the lit colour (the stand-in's is the half-size trace
/// enlarged; without Ray Reconstruction it is this tracer's own) is close,
/// what is flat (an unlit ball) keeps its exact colour, and edges, which
/// come from the visibility pass at full size, are as sharp.
#[test]
fn the_fast_picture_is_the_traced_one() {
    let Some(mut usual) = tracer(64) else { return };
    let mut p = still_life();
    let layer: lahza_motion_core::document::Layer = serde_json::from_value(json!({"id": "flat", "kind": "mesh", "mesh": {"type": "sphere", "radius": 18}, "material": {"color": "#00ff00", "unlit": true}, "transform": {"position": [250, 40]}})).unwrap();
    p.scenes[0].layers.push(layer);
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let reference = usual.render(&c, 0.0);
    keep("reference", &reference);
    for (way, spp, least) in [(Way::StandIn, 32, 27.0), (Way::Own, 4, 27.0)] {
        let Some(mut r) = fast(way, spp, false) else { return };
        // (Frames of a sequence: the picture builds on those before it.)
        let mut img = r.render(&c, 0.0);
        for k in 1..6 {
            img = r.render(&c, k as f64 / 30.0);
        }
        keep(&format!("picture-{way:?}"), &img);
        let db = psnr(&img, &reference);
        assert!(db > least, "{way:?}: {db:.1} dB against the usual trace");
        assert_eq!(img.get_pixel(250, 40).0, [0, 255, 0, 255], "{way:?}: what is unlit keeps its colour");
        assert_eq!(img.get_pixel(250, 40).0, reference.get_pixel(250, 40).0);
        let (ours, theirs) = (edge_width(&img, 40, 215..250), edge_width(&reference, 40, 215..250));
        assert!(ours <= theirs + 1 && ours <= 3, "{way:?}: an edge {ours} px wide (the usual trace: {theirs})");
        let warned = r.warnings().iter().any(|w| w.contains("without NVIDIA Ray Reconstruction"));
        assert_eq!(warned, way == Way::Own, "{way:?}: {:?}", r.warnings());
    }
}

/// Depth of field is a pass on the fast mode's picture: the ball in focus
/// keeps its edge and the far one blurs about as the lens blurs it in the
/// usual trace, lit or unlit (what is flat has its own depth).
#[test]
fn the_fast_mode_defocuses_after_the_picture() {
    let Some(mut usual) = tracer(64) else { return };
    for unlit in [true, false] {
        let material = if unlit { json!({"color": "#ff0000", "unlit": true}) } else { json!({"preset": "matte", "color": "#ff0000"}) };
        let mut p = project(json!([
            {"id": "far", "kind": "mesh", "mesh": {"type": "sphere", "radius": 50}, "material": material, "transform": {"position": [230, 120], "z": 600}},
            {"id": "near", "kind": "mesh", "mesh": {"type": "sphere", "radius": 50}, "material": material, "transform": {"position": [80, 120]}}
        ]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.scenes[0].camera = serde_json::from_value(json!({"aperture": 1.5, "focus": "near"})).unwrap();
        // (Lit evenly from everywhere, so an edge is an edge of coverage.)
        p.scenes[0].lights = serde_json::from_value(json!([{"type": "ambient", "intensity": 0.5}])).unwrap();
        p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none"})).unwrap());
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let reference = usual.render(&c, 0.0);
        let lens = edge_width(&reference, 120, 186..260);
        for way in [Way::StandIn, Way::Own] {
            let Some(mut r) = fast(way, 8, false) else { return };
            let img = r.render(&c, 0.0);
            keep(&format!("lens-{way:?}-{}", if unlit { "unlit" } else { "lit" }), &img);
            let (near, far) = (edge_width(&img, 120, 110..160), edge_width(&img, 120, 186..260));
            assert!(near <= 4, "{way:?}, unlit {unlit}: in focus {near} px");
            assert!(far >= 5 && (far as f64) > 0.5 * lens as f64 && (far as f64) < 2.0 * lens as f64, "{way:?}, unlit {unlit}: out of focus {far} px (the lens: {lens} px)");
        }
    }
}

/// Motion blur is a pass on the fast mode's picture, along the motion
/// over the shutter: a ball crossing a floor is a streak that smears over
/// the floor beside it, and what stands still in front keeps its edge.
#[test]
fn the_fast_mode_blurs_along_motion() {
    for way in [Way::StandIn, Way::Own] {
        let (Some(mut blurred), Some(mut sharp)) = (fast(way, 8, true), fast(way, 8, false)) else { return };
        let mut p = project(json!([
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"color": "#202020", "unlit": true}, "transform": {"position": [160, 120], "z": 200}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"color": "#ff0000", "unlit": true},
             "transform": {"position": {"keys": [{"at": 0, "v": [60, 120]}, {"at": 1, "v": [260, 120]}]}}},
            {"id": "post", "kind": "mesh", "mesh": {"type": "box", "size": [20, 60], "depth": 20}, "material": {"color": "#00ff00", "unlit": true}, "transform": {"position": [160, 40]}}
        ]));
        p.settings.width = 320;
        p.settings.height = 240;
        p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 360.0, samples: 8 });
        p.settings.fps = lahza_motion_core::time::Fps::whole(4);
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        // (The frame before gives the motion.)
        let frames = |r: &mut Renderer| {
            r.render(&c, 0.25);
            r.render(&c, 0.5)
        };
        let (b, s) = (frames(&mut blurred), frames(&mut sharp));
        keep(&format!("blur-{way:?}"), &b);
        keep(&format!("blur-{way:?}-sharp"), &s);
        let red = |img: &RgbaImage, least: u8| (0..320u32).filter(|&x| img.get_pixel(x, 120).0[0] > least).count();
        // The ball moves 50 px a frame: its edges along the row are ramps,
        // 60 px and those 50 from end to end.
        assert!(red(&s, 250) > 50 && red(&b, 250) < red(&s, 250) - 20, "{way:?}: {} px of full red blurred, {} sharp", red(&b, 250), red(&s, 250));
        assert!(edge_width(&b, 120, 80..160) >= 25, "{way:?}: the ball's edge is {} px wide", edge_width(&b, 120, 80..160));
        assert!(red(&b, 60) > red(&s, 60) + 25 && red(&b, 60) < 125, "{way:?}: the streak is {} px long (still: {})", red(&b, 60), red(&s, 60));
        // What stands still keeps its edge.
        let post = |img: &RgbaImage| (120..200u32).filter(|&x| { let p = img.get_pixel(x, 40).0; p[1] > 30 && p[1] < 225 }).count();
        assert!(post(&b) <= post(&s) + 1, "{way:?}: the post's edges: {} px blurred, {} sharp", post(&b), post(&s));
    }
}

/// A moving thing smears over nothing too: a ball alone is a streak as
/// long as in the usual trace's sub-frames.
#[test]
fn the_fast_mode_streaks_over_nothing() {
    let Some(mut usual) = tracer(32) else { return };
    let mut p = project(json!([{"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"color": "#ff0000", "unlit": true},
        "transform": {"position": {"keys": [{"at": 0, "v": [60, 120]}, {"at": 1, "v": [260, 120]}]}}}]));
    p.settings.width = 320;
    p.settings.height = 240;
    p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 360.0, samples: 8 });
    p.settings.fps = lahza_motion_core::time::Fps::whole(4);
    let c = Compiled::new(&p);
    let covered = |img: &RgbaImage| (0..320u32).filter(|&x| img.get_pixel(x, 120).0[0] as u32 * img.get_pixel(x, 120).0[3] as u32 / 255 > 20).count();
    let reference = covered(&usual.render(&c, 0.5));
    for way in [Way::StandIn, Way::Own] {
        let Some(mut r) = fast(way, 8, true) else { return };
        r.render(&c, 0.25);
        let img = r.render(&c, 0.5);
        keep(&format!("streak-{way:?}"), &img);
        let ours = covered(&img);
        assert!(ours > 90 && (ours as i32 - reference as i32).abs() < 20, "{way:?}: the streak covers {ours} px (the usual trace: {reference}; still: 60)");
    }
}

/// A small thing far out of focus in front of a sharp wall blurs over the
/// wall around it, as wide as the lens blurs it in the usual trace: it is
/// found from every texel its circle of confusion reaches.
#[test]
fn a_small_defocused_thing_blurs_over_what_is_sharp() {
    let Some(mut usual) = tracer(64) else { return };
    let mut p = project(json!([
        {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [600, 400]}, "material": {"color": "#202020", "unlit": true}, "transform": {"position": [160, 120]}},
        {"id": "bead", "kind": "mesh", "mesh": {"type": "sphere", "radius": 5}, "material": {"color": "#ff0000", "unlit": true}, "transform": {"position": [167, 131], "z": -120}}
    ]));
    p.settings.width = 320;
    p.settings.height = 240;
    p.scenes[0].camera = serde_json::from_value(json!({"aperture": 3.0, "focus": "wall"})).unwrap();
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    // How many px of the picture have some of the bead's red.
    let spread = |img: &RgbaImage| img.pixels().filter(|p| p.0[0] > 44).count();
    let reference = usual.render(&c, 0.0);
    keep("bead-reference", &reference);
    let lens = spread(&reference);
    for way in [Way::StandIn, Way::Own] {
        let Some(mut r) = fast(way, 8, false) else { return };
        let img = r.render(&c, 0.0);
        keep(&format!("bead-{way:?}"), &img);
        let ours = spread(&img);
        assert!(lens > 150 && (ours as f64) > 0.6 * lens as f64 && (ours as f64) < 1.5 * lens as f64, "{way:?}: the bead's blur covers {ours} px (the lens: {lens})");
    }
}

/// A shadow catcher over a card: the card keeps its colour, the soft
/// shadow on it is there, and it is smooth (four paths a texel, smoothed
/// across the texels around), not grain.
#[test]
fn a_shadow_catchers_soft_shadow_is_smooth() {
    if tracer(1).is_none() {
        return;
    }
    // (Not denoised: the usual trace's filter widens a catcher's shadow.)
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let mut usual = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, path: Some(TraceOptions { spp: 256, denoise: false, ..Default::default() }), ..Default::default() });
    let mut p = project(json!([
        {"id": "card", "kind": "shape", "shape": "rect", "size": [300, 170], "fill": "#d02020", "three_d": true, "transform": {"position": [160, 90]}},
        {"id": "catch", "kind": "mesh", "mesh": {"type": "plane", "size": [300, 170]}, "material": {"preset": "shadow", "opacity": 0.8},
         "transform": {"position": [160, 90], "z": -2}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"preset": "matte", "color": "#ffffff"},
         "transform": {"position": [90, 90], "z": -120}}
    ]));
    p.scenes[0].lights = serde_json::from_value(json!([{"type": "directional", "from": [0, 0, -1], "softness": 0.6, "shadow": true}])).unwrap();
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    // Grain: what is left of the red after a 3 × 3 mean, over the shadow
    // and its soft edge (right of the ball, which hides its middle).
    let grain = |img: &RgbaImage| {
        let l = |x: u32, y: u32| img.get_pixel(x, y).0[0] as f64;
        let (mut sum, mut n) = (0.0, 0.0);
        for y in 50..130u32 {
            for x in 75..150u32 {
                let mean = (0..9u32).map(|k| l(x + k % 3 - 1, y + k / 3 - 1)).sum::<f64>() / 9.0;
                sum += (l(x, y) - mean).abs();
                n += 1.0;
            }
        }
        sum / n
    };
    let red = |img: &RgbaImage, x: u32| (80..100u32).map(|y| img.get_pixel(x, y).0[0] as f64).sum::<f64>() / 20.0;
    let reference = usual.render(&c, 0.0);
    keep("catcher-reference", &reference);
    for way in [Way::StandIn, Way::Own] {
        let Some(mut r) = fast(way, 4, false) else { return };
        let img = r.render(&c, 0.0);
        keep(&format!("catcher-{way:?}"), &img);
        assert!((red(&img, 250) - red(&reference, 250)).abs() < 3.0, "{way:?}: the card's colour: {:.0} for {:.0}", red(&img, 250), red(&reference, 250));
        assert!(red(&img, 100) < red(&img, 250) * 0.8 && (red(&img, 100) - red(&reference, 100)).abs() < 12.0, "{way:?}: the shadow: {:.0} for {:.0}", red(&img, 100), red(&reference, 100));
        eprintln!("{way:?}: grain {:.2} (the usual trace at 256 spp, not denoised: {:.2})", grain(&img), grain(&reference));
        assert!(grain(&img) < grain(&reference) + 1.0, "{way:?}: grain {:.2} (the usual trace: {:.2})", grain(&img), grain(&reference));
    }
}

/// Two scenes, the second from half a second: other layers, another look.
fn two_scenes() -> Project {
    let mut p = rolling();
    p.settings.width = 160;
    p.settings.height = 120;
    p.settings.duration = 1.0;
    let scene: lahza_motion_core::document::Scene = serde_json::from_value(json!({"id": "s2", "start": 0.5, "layers": [
        {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#2040c0"}, "transform": {"position": [80, 60]}},
        {"id": "cube", "kind": "mesh", "mesh": {"type": "box", "size": [40, 40], "depth": 40}, "material": {"preset": "matte", "color": "#e0e040"}, "transform": {"position": [80, 60], "z": -40, "rotation_y": 30}}
    ]})).unwrap();
    p.scenes.push(scene);
    p
}

/// After a cut the history is another scene's: Ray Reconstruction is told
/// to start again on the cut's first frame (and only there), and without
/// it the first frame after the cut is the frame a renderer that never saw
/// the scene before draws, with nothing of the last scene in it.
#[test]
fn a_cut_starts_the_history_again() {
    let c = Compiled::new(&two_scenes());
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let Some(mut r) = fast(Way::StandIn, 2, false) else { return };
    for k in 3..8 {
        r.render(&c, k as f64 / 10.0);
    }
    let (seen, _, _) = r.gpu.as_ref().unwrap().stand_in_saw(0);
    assert_eq!(seen.iter().map(|s| s.1).collect::<Vec<_>>(), [true, false, true, false, false], "frames 3 to 7; the cut is at frame 5");
    let (Some(mut through), Some(mut fresh)) = (fast(Way::Own, 4, false), fast(Way::Own, 4, false)) else { return };
    let mut after = Vec::new();
    for k in 0..8 {
        let img = through.render(&c, k as f64 / 10.0);
        if k >= 5 {
            after.push(img);
        }
    }
    let alone = fresh.render(&c, 0.5);
    keep("cut-first", &after[0]);
    keep("cut-alone", &alone);
    let db = psnr(&after[0], &alone);
    assert!(db > 45.0, "the first frame after the cut against the same frame drawn alone: {db:.1} dB");
    // And how soon it is clean: against the scene at many samples, not
    // denoised (the first frame has four times the paths, `FAST_FIRST`).
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let mut usual = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, path: Some(TraceOptions { spp: 256, denoise: false, ..Default::default() }), ..Default::default() });
    let reference: Vec<RgbaImage> = (5..8).map(|k| usual.render(&c, k as f64 / 10.0)).collect();
    let clean: Vec<f64> = after.iter().zip(&reference).map(|(a, b)| psnr(a, b)).collect();
    eprintln!("after a cut, without Ray Reconstruction, frames 1 to 3 against 256 spp: {clean:.1?} dB");
    assert!(clean.iter().all(|db| *db > 28.0), "{clean:?}");
}

/// A frame drawn alone has no history; `try_render_warm` draws the frames
/// before it first, so it is the frame a sequence reaches (closer to the
/// usual trace than the frame alone), and it stops at the scene's cut.
#[test]
fn a_frame_alone_is_warmed_up() {
    let c = Compiled::new(&two_scenes());
    let (Some(mut usual), Some(mut alone), Some(mut warm), Some(mut sequence)) = (tracer(64), fast(Way::Own, 1, false), fast(Way::Own, 1, false), fast(Way::Own, 1, false)) else { return };
    let reference = usual.render(&c, 0.9);
    let a = alone.render(&c, 0.9);
    let w = warm.try_render_warm(&c, 0.9, 6).unwrap();
    let mut s = sequence.render(&c, 0.5);
    for k in 6..10 {
        s = sequence.render(&c, k as f64 / 10.0);
    }
    keep("warm", &w);
    keep("warm-alone", &a);
    assert!(psnr(&w, &s) > 45.0, "warmed up is the sequence's frame: {:.1} dB", psnr(&w, &s));
    // (Four frames of the scene were there to draw: frames 5 to 8.)
    assert_eq!(warm.stats.frames, 5);
    // (Alone it has four times the paths, `FAST_FIRST`, and no history.)
    eprintln!("1 spp without Ray Reconstruction against 64 spp: alone {:.1} dB, warmed up {:.1} dB", psnr(&a, &reference), psnr(&w, &reference));
    assert!(psnr(&w, &a) < 45.0, "the frames before it are in it");
    // Drawn again somewhere else, then here: the same picture.
    warm.render(&c, 0.2);
    let again = warm.try_render_warm(&c, 0.9, 6).unwrap();
    assert!(psnr(&again, &w) > 45.0, "{:.1} dB", psnr(&again, &w));
}

/// Not a test (`--ignored --nocapture`): the fast mode without Ray
/// Reconstruction against the usual trace on a sequence that moves (a
/// ball rolls and the camera turns): the time a frame takes, and its last
/// frame against the same frame at many samples. $LAHZA_MEASURE:
/// "width,height,frames,reference spp" (default 320,180,8,512).
#[test]
#[ignore]
fn measure_the_fast_mode_against_the_usual_trace() {
    let v: Vec<u32> = std::env::var("LAHZA_MEASURE").unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let (w, h, frames, many) = (*v.first().unwrap_or(&320), *v.get(1).unwrap_or(&180), *v.get(2).unwrap_or(&8), *v.get(3).unwrap_or(&512));
    let mut p = project(json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"},
         "transform": {"position": [160, 130], "rotation_x": 90}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"preset": "plastic", "color": "#d03734"},
         "transform": {"position": {"keys": [{"at": 0, "v": [90, 100]}, {"at": 1, "v": [150, 100]}]}}},
        {"id": "cube", "kind": "mesh", "mesh": {"type": "box", "size": [50, 50], "depth": 50}, "material": {"preset": "metal", "color": "#c0c0c0"},
         "transform": {"position": [205, 105], "rotation_y": 30}},
        {"id": "glass", "kind": "mesh", "mesh": {"type": "sphere", "radius": 22}, "material": {"preset": "glass"}, "transform": {"position": [255, 108], "z": -40}}
    ]));
    p.scenes[0].camera = serde_json::from_value(json!({"orbit": {"keys": [{"at": 0, "v": [-6, 20]}, {"at": 1, "v": [6, 20]}]}})).unwrap();
    p.settings.width = w;
    p.settings.height = h;
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let make = |path: TraceOptions, own: bool| {
        let mut r = Renderer::new(None, RenderOptions { backend: crate::Backend::Gpu, allow_software_gpu: allow, motion_blur: false, path: Some(path), ..Default::default() });
        if own {
            r.gpu.as_mut().unwrap().tracer_for_tests().unwrap().without_ngx();
        }
        r
    };
    let at = |k: u32| k as f64 / 30.0;
    let last = frames - 1;
    let reference = make(TraceOptions { spp: many, denoise: false, ..Default::default() }, false).render(&c, at(last));
    keep("measure-reference", &reference);
    eprintln!("{w} x {h}, {frames} frames; the last against {many} spp, not denoised ({})", make(TraceOptions::default(), false).backend_label());
    let usual = |spp| TraceOptions { spp, temporal: true, ..Default::default() };
    let own = |spp| TraceOptions { spp, fast: true, temporal: true, ..Default::default() };
    for (name, path, is_own) in [("usual, 64 spp", usual(64), false), ("usual, 16 spp", usual(16), false), ("usual, 4 spp", usual(4), false), ("fast, 8 spp", own(8), true), ("fast, 4 spp", own(4), true), ("fast, 2 spp", own(2), true), ("fast, 1 spp", own(1), true)] {
        let mut r = make(path, is_own);
        let mut times = Vec::new();
        let mut img = None;
        for k in 0..frames {
            let started = std::time::Instant::now();
            img = Some(r.render(&c, at(k)));
            times.push(started.elapsed().as_secs_f64());
        }
        let img = img.unwrap();
        keep(&format!("measure-{}", name.replace(", ", "-").replace(' ', "")), &img);
        // (The first frame builds the pipelines; with the fast mode it
        // also traces four times the paths.)
        let rest = times[1..].iter().sum::<f64>() / (times.len() - 1).max(1) as f64;
        eprintln!("{name:>14}: first frame {:6.2} s, then {:6.2} s a frame; the last frame {:.1} dB   {:?}", times[0], rest, psnr(&img, &reference), r.warnings());
    }
}

/// A camera that is somewhere else altogether from one frame to the next
/// has jumped: the history starts again there (Ray Reconstruction is told
/// so), and a camera that only turns a little keeps it.
#[test]
fn a_camera_jump_starts_the_history_again() {
    let Some(mut r) = fast(Way::StandIn, 2, false) else { return };
    let mut p = still_life();
    p.settings.fps = lahza_motion_core::time::Fps::whole(10);
    p.settings.duration = 1.0;
    // Four degrees a frame, then eighty in one.
    p.scenes[0].camera = serde_json::from_value(json!({"orbit": {"keys": [{"at": 0, "v": [0, 20]}, {"at": 0.4, "v": [16, 20], "ease": "linear"}, {"at": 0.5, "v": [96, 20], "ease": "linear"}, {"at": 1, "v": [116, 20], "ease": "linear"}]}})).unwrap();
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    for k in 2..8 {
        let img = r.render(&c, k as f64 / 10.0);
        if k == 4 || k == 5 {
            keep(&format!("jump-{k}"), &img);
        }
    }
    let (seen, _, _) = r.gpu.as_ref().unwrap().stand_in_saw(0);
    assert_eq!(seen.iter().map(|s| s.1).collect::<Vec<_>>(), [true, false, false, true, false, false], "frames 2 to 7; the camera jumps at frame 5");
}

/// Through a transition two scenes are traced at once, and after it the
/// one that came in is the frame's first traced list where it was the
/// second: it keeps its own history (Ray Reconstruction is told to start
/// again once for each scene, not again when the transition ends).
#[test]
fn a_transition_keeps_each_scenes_history() {
    let Some(mut r) = fast(Way::StandIn, 2, false) else { return };
    let mut p = two_scenes();
    p.scenes[1].transition = Some(serde_json::from_value(json!({"type": "fade", "dur": 0.4, "ease": "linear"})).unwrap());
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    // The cut is at 0.5 s: the fade runs from 0.3 to 0.7.
    for k in 1..10 {
        let img = r.render(&c, k as f64 / 10.0);
        if k == 5 {
            keep("transition", &img);
        }
    }
    let g = r.gpu.as_ref().unwrap();
    let (seen, _, _) = g.stand_in_saw(0);
    let slots = g.stand_in_slots();
    let resets: Vec<(u32, bool)> = slots.iter().copied().zip(seen.iter().map(|s| s.1)).collect();
    eprintln!("the traced lists and their resets: {resets:?}");
    assert!(slots.contains(&1), "two scenes are traced at once in the fade: {resets:?}");
    assert_eq!(resets.iter().filter(|r| r.1).count(), 2, "one start for each scene: {resets:?}");
    assert!(!resets.last().unwrap().1 && resets.last().unwrap().0 == 0);
}
