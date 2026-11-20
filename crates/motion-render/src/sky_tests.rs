//! Sky visibility in the raster (`RenderOptions::sky`): rays against the
//! path tracer's structures tell the raster's lighting how much of the
//! environment each point sees. Pictures go to $LAHZA_PT_OUT when set.

use crate::pt::scene_tests::tracer;
use crate::tests::{cpu, project};
use crate::{Backend, RenderOptions, Renderer};
use image::RgbaImage;
use lahza_motion_core::compile::Compiled;
use lahza_motion_core::document::Project;
use serde_json::json;

fn raster(sky: bool, profile: bool) -> Renderer {
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, sky, profile, ..Default::default() })
}

fn keep(name: &str, img: &RgbaImage) {
    if let Some(dir) = std::env::var_os("LAHZA_PT_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        img.save(std::path::Path::new(&dir).join(format!("sky-{name}.png"))).unwrap();
    }
}

fn psnr(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let mse = a.pixels().zip(b.pixels()).map(|(p, q)| (0..3).map(|k| (p.0[k] as f64 - q.0[k] as f64).powi(2)).sum::<f64>()).sum::<f64>() / (a.width() * a.height() * 3) as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
}

/// Mean brightness of the px within `r` of (x, y).
fn around(img: &RgbaImage, x: i32, y: i32, r: i32) -> f64 {
    let (mut sum, mut n) = (0.0, 0.0);
    for dy in -r..=r {
        for dx in -r..=r {
            let p = img.get_pixel((x + dx) as u32, (y + dy) as u32).0;
            sum += (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
            n += 1.0;
        }
    }
    sum / n
}

/// A ball and a box on a floor, a wall behind them. `lights`: the scene's
/// (None: the default key, rim and fill).
fn room(lights: Option<serde_json::Value>) -> Project {
    let mut p = project(json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"},
         "transform": {"position": [160, 130], "rotation_x": 90}},
        {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 500]}, "material": {"preset": "matte", "color": "#c8d0d8"},
         "transform": {"position": [160, 0], "z": 160}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 30}, "material": {"preset": "matte", "color": "#d03734"},
         "transform": {"position": [110, 100]}},
        {"id": "box", "kind": "mesh", "mesh": {"type": "box", "size": [60, 60], "depth": 60}, "material": {"preset": "matte", "color": "#e8e8e8"},
         "transform": {"position": [205, 100], "rotation_y": 25}},
        {"id": "slab", "kind": "mesh", "mesh": {"type": "box", "size": [70, 8], "depth": 60}, "material": {"preset": "matte", "color": "#e8e8e8"},
         "transform": {"position": [205, 62], "rotation_y": 25}}
    ]));
    p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 20]})).unwrap();
    if let Some(l) = lights {
        p.scenes[0].lights = serde_json::from_value(l).unwrap();
    }
    p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "soft"})).unwrap());
    p
}

/// With sky visibility the raster darkens where things touch and under
/// what overhangs, as the traced picture does, and leaves the open floor
/// as it was; the same frame is the same picture whatever was drawn
/// before; and the CPU, which has no rays, draws as without it.
#[test]
fn the_sky_is_seen_less_where_things_touch() {
    if tracer(1).is_none() {
        return;
    }
    // (Lit by the environment only: what the visibility is about.)
    let c = Compiled::new(&room(Some(json!([{"type": "ambient", "intensity": 0.2}]))));
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (mut with, mut without, mut traced) = (raster(true, false), raster(false, false), tracer(64).unwrap());
    let (a, b, t) = (with.render(&c, 0.0), without.render(&c, 0.0), traced.render(&c, 0.0));
    keep("env-with", &a);
    keep("env-without", &b);
    keep("env-traced", &t);
    assert!(with.warnings().is_empty(), "{:?}", with.warnings());
    assert_ne!(a, b);
    // Find the floor right of the ball's foot by where the traced picture
    // is darkest under it, and an open place at the front of the floor.
    let open = (40, 165);
    let (dark, _) = (60..100i32).flat_map(|x| (118..140i32).map(move |y| (x, y))).map(|(x, y)| ((x, y), around(&t, x, y, 2) / around(&b, x, y, 2))).fold(((0, 0), f64::MAX), |m, v| if v.1 < m.1 { v } else { m });
    let ratio = |img: &RgbaImage, at: (i32, i32)| around(img, at.0, at.1, 2) / around(&b, at.0, at.1, 2);
    eprintln!("the contact at {dark:?}: sky visibility {:.2} of the raster, traced {:.2}; the open floor {:.2}, traced {:.2}", ratio(&a, dark), ratio(&t, dark), ratio(&a, open), ratio(&t, open));
    assert!(ratio(&a, dark) < 0.8, "where the ball stands is darker: {:.2}", ratio(&a, dark));
    // (The wall behind takes a tenth of the open floor's sky; traced, its
    // own light comes back from there.)
    assert!(ratio(&a, open) > 0.85 && ratio(&a, open) <= 1.0, "the open floor is nearly as it was: {:.2}", ratio(&a, open));
    assert!((ratio(&a, dark) - ratio(&t, dark)).abs() < 0.12, "the contact is as dark as traced: {:.2} for {:.2}", ratio(&a, dark), ratio(&t, dark));
    // The whole picture: with the light that comes back from what is in
    // the way it is about as bright as traced (one bounce: a little
    // darker). Visibility alone was 16 dB from the traced picture here,
    // every face that looks at another having lost that light.
    let mean = |img: &RgbaImage| img.pixels().map(|p| p.0[0] as f64 + p.0[1] as f64 + p.0[2] as f64).sum::<f64>() / (img.width() * img.height() * 3) as f64;
    eprintln!("against the traced picture: {:.1} dB with sky visibility, {:.1} dB without; mean brightness {:.1}, {:.1} without, {:.1} traced", psnr(&a, &t), psnr(&b, &t), mean(&a), mean(&b), mean(&t));
    assert!(psnr(&a, &t) > 23.0 && (mean(&a) / mean(&t) - 1.0).abs() < 0.08, "{:.1} dB, {:.1} for {:.1}", psnr(&a, &t), mean(&a), mean(&t));
    // Large flat things stay even: the wall and the open floor are not
    // blotchy (the spread of their brightness, in levels; some of it is
    // the light's own slope, as in the traced picture).
    let spread = |img: &RgbaImage, xs: std::ops::Range<u32>, ys: std::ops::Range<u32>| {
        let v: Vec<f64> = ys.flat_map(|y| xs.clone().map(move |x| (x, y))).map(|(x, y)| { let p = img.get_pixel(x, y).0; (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0 }).collect();
        let m = v.iter().sum::<f64>() / v.len() as f64;
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
    };
    let (wall, floor) = (|img: &RgbaImage| spread(img, 15..95, 8..50), |img: &RgbaImage| spread(img, 15..300, 150..175));
    eprintln!("the wall's spread {:.2} (plain {:.2}, traced {:.2}), the open floor's {:.2} (plain {:.2}, traced {:.2})", wall(&a), wall(&b), wall(&t), floor(&a), floor(&b), floor(&t));
    assert!(wall(&a) < wall(&t) + 0.5 && floor(&a) < floor(&t) + 0.5, "blotches: wall {:.2}, floor {:.2}", wall(&a), floor(&a));
    // Another frame, then this one again.
    with.render(&c, 0.5);
    with.last_invalidate();
    assert_eq!(with.render(&c, 0.0), a, "the same frame is the same picture");
    // The CPU casts the same rays itself: its frame against the GPU's,
    // as the other GPU-against-CPU tests measure it (px more than three
    // levels apart), beside the same two frames without sky visibility.
    let mut on_cpu = Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() });
    let started = std::time::Instant::now();
    let ours = on_cpu.render(&c, 0.0);
    let took = started.elapsed();
    keep("env-cpu", &ours);
    let plain = cpu().render(&c, 0.0);
    assert_ne!(ours, plain);
    let apart = |x: &RgbaImage, y: &RgbaImage| {
        let d: Vec<u8> = x.pixels().zip(y.pixels()).map(|(p, q)| (0..4).map(|k| p.0[k].abs_diff(q.0[k])).max().unwrap()).collect();
        (d.iter().filter(|v| **v > 3).count() as f64 / d.len() as f64, *d.iter().max().unwrap())
    };
    let (with_sky, without) = (apart(&a, &ours), apart(&b, &plain));
    eprintln!("GPU against CPU: {:.2}% of px more than 3 levels apart with sky visibility (most {}), {:.2}% without (most {}); the CPU's frame took {:.0} ms", with_sky.0 * 100.0, with_sky.1, without.0 * 100.0, without.1, took.as_secs_f64() * 1000.0);
    assert!(with_sky.0 < without.0 + 0.01, "{with_sky:?} against {without:?}");
}

/// Not a test (`--ignored --nocapture`): the default lights' scene with
/// and without sky visibility and traced, to look at, and what the rays'
/// structures and passes cost a raster frame. $LAHZA_MEASURE: "width,height".
#[test]
#[ignore]
fn measure_sky_visibility() {
    if tracer(1).is_none() {
        return;
    }
    let v: Vec<u32> = std::env::var("LAHZA_MEASURE").unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let mut p = room(None);
    p.settings.width = *v.first().unwrap_or(&480);
    p.settings.height = *v.get(1).unwrap_or(&270);
    let c = Compiled::new(&p);
    let (mut with, mut without, mut traced) = (raster(true, true), raster(false, true), tracer(64).unwrap());
    let time = |r: &mut Renderer, name: &str| {
        let mut img = r.render(&c, 0.0);
        let started = std::time::Instant::now();
        for k in 1..=4 {
            img = r.render(&c, k as f64 / 30.0);
        }
        eprintln!("{name}: {:.0} ms a frame ({})", started.elapsed().as_secs_f64() * 250.0, r.backend_label());
        r.flush_profile();
        if let Some(report) = r.profile.as_ref().map(|p| p.report()) {
            let text = serde_json::to_string_pretty(&report).unwrap();
            for line in text.lines().filter(|l| l.contains("trace") || l.contains("sky") || l.contains("\"gpu frame\"")) {
                eprintln!("    {}", line.trim());
            }
        }
        img
    };
    let (a, b) = (time(&mut with, "with sky visibility"), time(&mut without, "without"));
    // The CPU's own rays (a BVH a frame, every core), with and without.
    for sky in [true, false] {
        let mut r = Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky, ..Default::default() });
        r.render(&c, 0.0);
        let started = std::time::Instant::now();
        for k in 1..=4 {
            r.render(&c, k as f64 / 30.0);
        }
        eprintln!("the CPU, sky visibility {sky}: {:.0} ms a frame", started.elapsed().as_secs_f64() * 250.0);
    }
    let t = traced.render(&c, 4.0 / 30.0);
    keep("lit-with", &a);
    keep("lit-without", &b);
    keep("lit-traced", &t);
    eprintln!("against the traced picture: {:.1} dB with sky visibility, {:.1} dB without", psnr(&a, &t), psnr(&b, &t));
}

/// Not a test (`--ignored --nocapture`): under a table (a top on four
/// legs over the floor, the default lights), where the traced picture has
/// the light that comes back off the floor and the legs more than once:
/// how dark the floor under it is with sky visibility, plain and traced.
#[test]
#[ignore]
fn measure_under_a_table() {
    if tracer(1).is_none() {
        return;
    }
    let leg = |id: &str, x: i32, z: i32| json!({"id": id, "kind": "mesh", "mesh": {"type": "box", "size": [8, 50], "depth": 8}, "material": {"preset": "matte", "color": "#b08050"}, "transform": {"position": [x, 105], "z": z}});
    let mut p = project(json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"}, "transform": {"position": [160, 130], "rotation_x": 90}},
        {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 500]}, "material": {"preset": "matte", "color": "#c8d0d8"}, "transform": {"position": [160, 0], "z": 160}},
        {"id": "top", "kind": "mesh", "mesh": {"type": "box", "size": [150, 8], "depth": 90}, "material": {"preset": "matte", "color": "#b08050"}, "transform": {"position": [160, 76]}},
        leg("a", 92, -38), leg("b", 228, -38), leg("c", 92, 38), leg("d", 228, 38)
    ]));
    p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 12]})).unwrap();
    p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "soft"})).unwrap());
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (a, b, t) = (raster(true, false).render(&c, 0.0), raster(false, false).render(&c, 0.0), tracer(64).unwrap().render(&c, 0.0));
    keep("table-with", &a);
    keep("table-without", &b);
    keep("table-traced", &t);
    // The darkest 9 x 9 px of the traced floor between the legs, and the same place in the others.
    let (at, _) = (110..210i32).flat_map(|x| (100..150i32).map(move |y| (x, y))).map(|at| (at, around(&t, at.0, at.1, 4))).fold(((0, 0), f64::MAX), |m, v| if v.1 < m.1 { v } else { m });
    eprintln!("under the table at {at:?}: {:.0} with sky visibility, {:.0} plain, {:.0} traced; the open floor {:.0}, {:.0}, {:.0}", around(&a, at.0, at.1, 4), around(&b, at.0, at.1, 4), around(&t, at.0, at.1, 4), around(&a, 40, 165, 4), around(&b, 40, 165, 4), around(&t, 40, 165, 4));
    eprintln!("against the traced picture: {:.1} dB with sky visibility, {:.1} dB without", psnr(&a, &t), psnr(&b, &t));
}
