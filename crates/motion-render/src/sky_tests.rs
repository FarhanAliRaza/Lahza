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
    // blotchy. What is left of their brightness, in levels, once its
    // slope is taken away (each px against the mean of the 33 x 33 around
    // it: the light's own slope is in the traced picture too).
    let spread = |img: &RgbaImage, xs: std::ops::Range<i32>, ys: std::ops::Range<i32>| {
        let l = |x: i32, y: i32| {
            let p = img.get_pixel(x.clamp(0, img.width() as i32 - 1) as u32, y.clamp(0, img.height() as i32 - 1) as u32).0;
            (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0
        };
        let v: Vec<f64> = ys.flat_map(|y| xs.clone().map(move |x| (x, y))).map(|(x, y)| l(x, y) - (-16..=16).flat_map(|dy| (-16..=16).map(move |dx| (dx, dy))).map(|(dx, dy)| l(x + dx, y + dy)).sum::<f64>() / 1089.0).collect();
        (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
    };
    let (wall, floor) = (|img: &RgbaImage| spread(img, 30..80, 20..40), |img: &RgbaImage| spread(img, 30..290, 158..168));
    eprintln!("blotches: the wall {:.2} (plain {:.2}, traced {:.2}), the open floor {:.2} (plain {:.2}, traced {:.2})", wall(&a), wall(&b), wall(&t), floor(&a), floor(&b), floor(&t));
    assert!(wall(&a) < wall(&t) + 0.3 && floor(&a) < floor(&t) + 0.3, "blotches: wall {:.2}, floor {:.2}", wall(&a), floor(&a));
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
            // (A still scene's frames are the same lists: drawn each time.)
            r.last_invalidate();
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
            r.last_invalidate();
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

/// A table (a top on four legs) over the floor before the wall, under the
/// default lights.
fn table() -> Project {
    let leg = |id: &str, x: i32, z: i32| json!({"id": id, "kind": "mesh", "mesh": {"type": "box", "size": [8, 50], "depth": 8}, "material": {"preset": "matte", "color": "#b08050"}, "transform": {"position": [x, 105], "z": z}});
    let mut p = project(json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"}, "transform": {"position": [160, 130], "rotation_x": 90}},
        {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 500]}, "material": {"preset": "matte", "color": "#c8d0d8"}, "transform": {"position": [160, 0], "z": 160}},
        {"id": "top", "kind": "mesh", "mesh": {"type": "box", "size": [150, 8], "depth": 90}, "material": {"preset": "matte", "color": "#b08050"}, "transform": {"position": [160, 76]}},
        leg("a", 92, -38), leg("b", 228, -38), leg("c", 92, 38), leg("d", 228, 38)
    ]));
    p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 12]})).unwrap();
    p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "soft"})).unwrap());
    p
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
    let p = table();
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

/// Not a test (`--ignored --nocapture`, LAHZA_SKY_TIMES=1 for its
/// stages): the CPU's frame with and without its sky rays at
/// $LAHZA_MEASURE (default the preview's 1080 x 608).
#[test]
#[ignore]
fn measure_the_cpus_rays() {
    let v: Vec<u32> = std::env::var("LAHZA_MEASURE").unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let mut p = room(None);
    p.settings.width = *v.first().unwrap_or(&1080);
    p.settings.height = *v.get(1).unwrap_or(&608);
    let c = Compiled::new(&p);
    for sky in [true, false] {
        let mut r = Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky, ..Default::default() });
        r.render(&c, 0.0);
        let started = std::time::Instant::now();
        for k in 1..=4 {
            r.last_invalidate();
            r.render(&c, k as f64 / 30.0);
        }
        eprintln!("the CPU, sky visibility {sky}: {:.0} ms a frame", started.elapsed().as_secs_f64() * 250.0);
    }
}

/// With an environment image the light that comes back is the image's
/// too (its diffuse level, looked up at what a ray hits), on the GPU and
/// on the CPU alike.
#[test]
fn the_light_back_is_an_environment_images_too() {
    if tracer(1).is_none() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("lahza-sky-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("sky.hdr");
    crate::envmap::tests::write_hdr(&image, 0.0);
    let mut p = room(Some(json!([{"type": "ambient", "intensity": 0}])));
    p.scenes[0].environment = Some(serde_json::from_value(json!({"image": image.to_str().unwrap()})).unwrap());
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (a, b) = (raster(true, false).render(&c, 0.0), raster(false, false).render(&c, 0.0));
    let ours = Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() }).render(&c, 0.0);
    keep("image-with", &a);
    keep("image-without", &b);
    let _ = std::fs::remove_dir_all(&dir);
    let apart = a.pixels().zip(ours.pixels()).filter(|(p, q)| (0..4).any(|k| p.0[k].abs_diff(q.0[k]) > 3)).count() as f64 / (a.width() * a.height()) as f64;
    // The wall behind sees the floor for half its sky: without the
    // image's light back from there it would have half its light.
    let wall = around(&a, 40, 25, 4) / around(&b, 40, 25, 4);
    eprintln!("an environment image: the wall {wall:.2} of the plain raster's; GPU against CPU {:.2}% of px more than 3 levels apart", apart * 100.0);
    assert!(wall > 0.7 && wall < 1.3, "{wall:.2}");
    assert!(apart < 0.01, "{apart}");
}

/// Steps in the sky's part of a picture: neighbours (across and down) on
/// one flat face, which the plain raster draws alike (four px in a row
/// within two levels), that sky visibility draws more than `over` levels
/// apart. How many, and the widest. On one face what a point sees of the
/// sky changes slowly; a jump there is another surface's texel taken.
fn steps(sky: &RgbaImage, plain: &RgbaImage, over: f64) -> (usize, f64) {
    let l = |img: &RgbaImage, x: i32, y: i32| {
        let p = img.get_pixel(x as u32, y as u32).0;
        (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0
    };
    let (w, h) = (sky.width() as i32, sky.height() as i32);
    let (mut n, mut worst) = (0, 0.0f64);
    for (dx, dy) in [(1, 0), (0, 1)] {
        for y in 1..h - 2 {
            for x in 1..w - 2 {
                let run: Vec<f64> = (-1..=2).map(|k| l(plain, x + dx * k, y + dy * k)).collect();
                let (lo, hi) = run.iter().fold((f64::MAX, f64::MIN), |m, v| (m.0.min(*v), m.1.max(*v)));
                if hi - lo > 2.0 {
                    continue;
                }
                let jump = (l(sky, x, y) - l(sky, x + dx, y + dy)).abs();
                if jump > over {
                    n += 1;
                    worst = worst.max(jump);
                }
            }
        }
    }
    (n, worst)
}

/// A box with a slab over it, a gap between them, and a block standing
/// against a wall: edges where two surfaces meet at one depth (the box's
/// top and its front, the slab's front over the dark gap).
fn edges() -> Project {
    let mut p = project(json!([
        {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"},
         "transform": {"position": [160, 130], "rotation_x": 90}},
        {"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 500]}, "material": {"preset": "matte", "color": "#c8d0d8"},
         "transform": {"position": [160, 0], "z": 160}},
        {"id": "box", "kind": "mesh", "mesh": {"type": "box", "size": [90, 50], "depth": 70}, "material": {"preset": "matte", "color": "#e8e8e8"},
         "transform": {"position": [105, 105], "rotation_y": 25}},
        {"id": "slab", "kind": "mesh", "mesh": {"type": "box", "size": [100, 8], "depth": 70}, "material": {"preset": "matte", "color": "#e8e8e8"},
         "transform": {"position": [105, 70], "rotation_y": 25}},
        {"id": "block", "kind": "mesh", "mesh": {"type": "box", "size": [80, 70], "depth": 50}, "material": {"preset": "matte", "color": "#e8e8e8"},
         "transform": {"position": [235, 95], "rotation_y": -35}}
    ]));
    p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 20]})).unwrap();
    p.scenes[0].lights = serde_json::from_value(json!([{"type": "ambient", "intensity": 0.2}])).unwrap();
    p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "soft"})).unwrap());
    p
}

/// At an edge between two surfaces at nearly one depth (a crease, a
/// slab's front over the gap under it) each side keeps its own sky: the
/// lookup tells surfaces apart by their planes, not by depth, so neither
/// side takes the other's texels in steps of two px (by depth this scene
/// had 53 such steps, up to 15 levels; one is left, at the end of the gap
/// where three faces meet within a texel). On the GPU and on the CPU,
/// which agree as before.
#[test]
fn an_edge_at_one_depth_has_no_steps() {
    if tracer(1).is_none() {
        return;
    }
    let c = Compiled::new(&edges());
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (a, b) = (raster(true, false).render(&c, 0.0), raster(false, false).render(&c, 0.0));
    let ours = Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() }).render(&c, 0.0);
    let plain = cpu().render(&c, 0.0);
    keep("edges-with", &a);
    keep("edges-without", &b);
    keep("edges-cpu", &ours);
    assert_ne!(a, b);
    let (gpu, on_cpu) = (steps(&a, &b, 6.0), steps(&ours, &plain, 6.0));
    let apart = a.pixels().zip(ours.pixels()).filter(|(p, q)| (0..4).any(|k| p.0[k].abs_diff(q.0[k]) > 3)).count() as f64 / (a.width() * a.height()) as f64;
    eprintln!("steps over 6 levels on a flat face: the GPU {} (widest {:.0}), the CPU {} (widest {:.0}); GPU against CPU {:.2}% of px more than 3 levels apart", gpu.0, gpu.1, on_cpu.0, on_cpu.1, apart * 100.0);
    assert!(gpu.0 <= 2 && on_cpu.0 <= 2, "steps: {gpu:?} on the GPU, {on_cpu:?} on the CPU");
    assert!(apart < 0.01, "{apart}");
}

/// A floor and a wall with, as asked: a card lying flat over the floor (a
/// flat 3D layer, a canopy), and a box of the given opacity standing on
/// the floor.
fn canopy(card: bool, pane: Option<f64>) -> Project {
    let mut layers = vec![
        json!({"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"},
         "transform": {"position": [160, 130], "rotation_x": 90}}),
        json!({"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 500]}, "material": {"preset": "matte", "color": "#c8d0d8"},
         "transform": {"position": [160, 0], "z": 160}}),
    ];
    if card {
        layers.push(json!({"id": "card", "kind": "shape", "shape": "rect", "size": [110, 90], "fill": "#3050c0", "three_d": true,
         "transform": {"position": [95, 105], "rotation_x": 90}}));
    }
    if let Some(opacity) = pane {
        layers.push(json!({"id": "pane", "kind": "mesh", "mesh": {"type": "box", "size": [70, 60], "depth": 50}, "material": {"preset": "matte", "color": "#e8e8e8", "opacity": opacity},
         "transform": {"position": [235, 100], "rotation_y": -30}}));
    }
    let mut p = project(serde_json::Value::Array(layers));
    p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 20]})).unwrap();
    p.scenes[0].lights = serde_json::from_value(json!([{"type": "ambient", "intensity": 0.2}])).unwrap();
    p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "soft"})).unwrap());
    p
}

/// Cards and see-through meshes hide the sky by a fixed rule (nothing
/// drawn by lot): each takes of a ray what it covers (a card its
/// picture's coverage, a mesh its opacity) and the rest goes on, so a box
/// 0.3 opaque darkens the floor beside it less than one 0.7 opaque and
/// about as much as traced; and a see-through mesh that is at least half
/// opaque takes the sky itself. On the GPU and on the CPU alike.
#[test]
fn cards_and_see_through_meshes_hide_the_sky() {
    if tracer(1).is_none() {
        return;
    }
    let render = |p: &Project, sky: bool| {
        let c = Compiled::new(p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let mut r = raster(sky, false);
        let img = r.render(&c, 0.0);
        assert!(r.warnings().is_empty(), "{:?}", r.warnings());
        img
    };
    let on_cpu = |p: &Project| Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() }).render(&Compiled::new(p), 0.0);
    let apart = |a: &RgbaImage, b: &RgbaImage| a.pixels().zip(b.pixels()).filter(|(p, q)| (0..4).any(|k| p.0[k].abs_diff(q.0[k]) > 3)).count() as f64 / (a.width() * a.height()) as f64;
    let (bare, with_card, solid, ghost) = (canopy(false, None), canopy(true, None), canopy(false, Some(0.7)), canopy(false, Some(0.3)));
    let (a0, a1, a2, a3) = (render(&bare, true), render(&with_card, true), render(&solid, true), render(&ghost, true));
    let p2 = render(&solid, false);
    keep("canopy-bare", &a0);
    keep("canopy-card", &a1);
    keep("canopy-pane", &a2);
    keep("canopy-pane-plain", &p2);
    keep("canopy-ghost", &a3);
    let mut t = tracer(64).unwrap();
    let (t0, t2, t3) = (t.render(&Compiled::new(&bare), 0.0), t.render(&Compiled::new(&solid), 0.0), t.render(&Compiled::new(&ghost), 0.0));
    keep("canopy-card-traced", &t.render(&Compiled::new(&with_card), 0.0));
    keep("canopy-pane-traced", &t2);
    keep("canopy-ghost-traced", &t3);
    // The floor under the card's front edge, and beside the box's foot
    // (left of it, where the 0.7 opaque box darkens the floor most).
    let l = |img: &RgbaImage, at: (i32, i32)| around(img, at.0, at.1, 2);
    let (beside, _) = (168..190i32).flat_map(|x| (108..136i32).map(move |y| (x, y))).map(|at| (at, l(&a2, at) / l(&a0, at))).fold(((0, 0), f64::MAX), |m, v| if v.1 < m.1 { v } else { m });
    let (under, open) = ((95, 150), (40, 170));
    eprintln!("the floor under the card: {:.0} with it, {:.0} without; beside the box: {:.0} at 0.7 opaque, {:.0} at 0.3, {:.0} without; the open floor {:.0}, {:.0}, {:.0}, {:.0}", l(&a1, under), l(&a0, under), l(&a2, beside), l(&a3, beside), l(&a0, beside), l(&a0, open), l(&a1, open), l(&a2, open), l(&a3, open));
    assert!(l(&a1, under) < l(&a0, under) * 0.9, "a card hides the sky from the floor under it");
    assert!(l(&a2, beside) < l(&a0, beside) * 0.95, "a mesh that is 0.7 opaque hides the sky");
    assert!(l(&a3, beside) < l(&a0, beside) - 4.0 && l(&a3, beside) > l(&a2, beside) + 4.0, "a mesh that is 0.3 opaque hides some of the sky, less than one 0.7 opaque");
    // As the traced picture has it: the floor beside the box as a share
    // of the floor without the box (with a step at one half the 0.3
    // opaque box hid nothing, 1.00 here).
    let share = |with: &RgbaImage, without: &RgbaImage| l(with, beside) / l(without, beside);
    eprintln!("beside the box, of the floor without it: 0.7 opaque {:.2} (traced {:.2}), 0.3 opaque {:.2} (traced {:.2})", share(&a2, &a0), share(&t2, &t0), share(&a3, &a0), share(&t3, &t0));
    assert!((share(&a3, &a0) - share(&t3, &t0)).abs() < 0.05 && (share(&a2, &a0) - share(&t2, &t0)).abs() < 0.05, "as traced");
    // The box itself takes the sky where it is at least half opaque: one
    // that is 0.99 opaque (see-through, to the builder) is lit as a solid
    // one is, where sky visibility darkens that most.
    let (whole, nearly) = (canopy(false, Some(1.0)), canopy(false, Some(0.99)));
    let (w1, w0, n1) = (render(&whole, true), render(&whole, false), render(&nearly, true));
    keep("canopy-nearly", &n1);
    let (on_box, _) = (200..270i32).flat_map(|x| (95..135i32).map(move |y| (x, y))).map(|at| (at, l(&w1, at) / l(&w0, at))).fold(((0, 0), f64::MAX), |m, v| if v.1 < m.1 { v } else { m });
    eprintln!("the box at {on_box:?}: {:.0} solid, {:.0} at 0.99 opaque, {:.0} without sky visibility", l(&w1, on_box), l(&n1, on_box), l(&w0, on_box));
    assert!(l(&w1, on_box) < l(&w0, on_box) - 4.0, "sky visibility darkens the box somewhere");
    assert!((l(&n1, on_box) - l(&w1, on_box)).abs() < 3.0, "what is see-through takes the sky as what is solid does");
    // The CPU's rays meet the same things.
    let (c1, c2, c3) = (on_cpu(&with_card), on_cpu(&solid), on_cpu(&ghost));
    keep("canopy-card-cpu", &c1);
    eprintln!("GPU against CPU, px more than 3 levels apart: the card {:.2}%, 0.7 opaque {:.2}%, 0.3 opaque {:.2}%", apart(&a1, &c1) * 100.0, apart(&a2, &c2) * 100.0, apart(&a3, &c3) * 100.0);
    assert!(apart(&a1, &c1) < 0.01 && apart(&a2, &c2) < 0.01 && apart(&a3, &c3) < 0.01);
    // And the same frame is the same picture.
    assert_eq!(render(&with_card, true), a1);
}

/// Not a test (`--ignored --nocapture`): what a third return of the
/// light would add. Sky visibility has the environment's light, what
/// comes back from the surfaces a point sees and what those take back
/// in turn (a guess beyond: pt.wgsl `SKY_REST`); the traced picture at 2,
/// 3, 4 and 6 bounces has one return more each time. The room lit by the environment and
/// the table under the default lights: the whole picture's brightness
/// and the darkest floor (under the ball; under the table), in levels.
#[test]
#[ignore]
fn measure_a_third_return() {
    if tracer(1).is_none() {
        return;
    }
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let mean = |img: &RgbaImage| img.pixels().map(|p| p.0[0] as f64 + p.0[1] as f64 + p.0[2] as f64).sum::<f64>() / (img.width() * img.height() * 3) as f64;
    for (name, p, spot) in [("the room", room(Some(json!([{"type": "ambient", "intensity": 0.2}]))), (99, 129)), ("the table", table(), (160, 125))] {
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let sky = raster(true, false).render(&c, 0.0);
        eprintln!("{name}: sky visibility {:.1} over the picture, {:.1} at the dark floor", mean(&sky), around(&sky, spot.0, spot.1, 4));
        let mut before: Option<RgbaImage> = None;
        // (LAHZA_SKY_ONLY: not the traced frames, which take minutes.)
        for bounces in [2u32, 3, 4, 5, 6].into_iter().filter(|_| std::env::var_os("LAHZA_SKY_ONLY").is_none()) {
            let mut r = Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, path: Some(crate::pt::TraceOptions { spp: 256, bounces, ..Default::default() }), ..Default::default() });
            let img = r.render(&c, 0.0);
            let more = before.as_ref().map(|b| (mean(&img) - mean(b), around(&img, spot.0, spot.1, 4) - around(b, spot.0, spot.1, 4)));
            eprintln!("    traced, {bounces} bounces: {:.1} over the picture, {:.1} at the dark floor{}", mean(&img), around(&img, spot.0, spot.1, 4), more.map_or(String::new(), |m| format!(" ({:+.1} and {:+.1} on the one before)", m.0, m.1)));
            before = Some(img);
        }
    }
}

/// Not a test (`--ignored --nocapture`): what cards in the sky's rays
/// cost. The floor and wall with and without a card over the floor, with
/// sky visibility and without (the card's own drawing is in both): ms a
/// frame on the GPU and on the CPU. $LAHZA_MEASURE: "width,height".
#[test]
#[ignore]
fn measure_cards_in_the_rays() {
    if tracer(1).is_none() {
        return;
    }
    let v: Vec<u32> = std::env::var("LAHZA_MEASURE").unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
    for card in [false, true] {
        let mut p = canopy(card, Some(1.0));
        p.settings.width = *v.first().unwrap_or(&480);
        p.settings.height = *v.get(1).unwrap_or(&270);
        let c = Compiled::new(&p);
        for sky in [false, true] {
            let time = |r: &mut Renderer| {
                r.render(&c, 0.0);
                r.render(&c, 0.0);
                let started = std::time::Instant::now();
                for _ in 0..6 {
                    r.last_invalidate();
                    r.render(&c, 0.0);
                }
                started.elapsed().as_secs_f64() * 1000.0 / 6.0
            };
            let (gpu, on_cpu) = (time(&mut raster(sky, false)), time(&mut Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky, ..Default::default() })));
            eprintln!("a card {card}, sky visibility {sky}: the GPU {gpu:.0} ms a frame, the CPU {on_cpu:.0} ms");
        }
    }
}

/// A volume hides the sky by its density along each ray, marched in even
/// steps (nothing drawn by lot): the floor before a slab of haze hanging
/// over it is darker the denser the haze (a little: the haze sends light
/// back too, and the traced picture has as little), on the GPU and on the
/// CPU alike; and a filled wireframe hides it as the solid thing it
/// looks.
#[test]
fn a_volume_hides_the_sky_by_its_density() {
    if tracer(1).is_none() {
        return;
    }
    let build = |over: Option<serde_json::Value>| {
        let mut p = canopy(false, None);
        if let Some(material) = over {
            p.scenes[0].layers.push(serde_json::from_value(json!({"id": "over", "kind": "mesh", "mesh": {"type": "box", "size": [120, 16], "depth": 100}, "material": material,
                "transform": {"position": [95, 98]}})).unwrap());
        }
        Compiled::new(&p)
    };
    let scenes = [
        ("nothing", build(None)),
        ("thin haze", build(Some(json!({"volume": {"density": 0.3}})))),
        ("thick haze", build(Some(json!({"volume": {"density": 5}})))),
        ("a cloud", build(Some(json!({"volume": {"density": 5}, "noise": {"type": "fbm", "scale": 30}})))),
        ("a filled wireframe", build(Some(json!({"preset": "wireframe", "color": "#ffb020", "fill": "#404040"})))),
    ];
    let spot = (95, 158);
    let mut seen = Vec::new();
    for (name, c) in &scenes {
        assert!(c.errors.is_empty(), "{name}: {:?}", c.errors);
        let mut r = raster(true, false);
        let a = r.render(c, 0.0);
        assert!(r.warnings().is_empty(), "{name}: {:?}", r.warnings());
        let ours = Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() }).render(c, 0.0);
        keep(&format!("over-{}", name.replace(' ', "-")), &a);
        let apart = a.pixels().zip(ours.pixels()).filter(|(p, q)| (0..4).any(|k| p.0[k].abs_diff(q.0[k]) > 3)).count() as f64 / (a.width() * a.height()) as f64;
        eprintln!("{name}: the floor in front of it {:.0}; GPU against CPU {:.2}% of px more than 3 levels apart", around(&a, spot.0, spot.1, 2), apart * 100.0);
        assert!(apart < 0.01, "{name}: {apart}");
        seen.push(around(&a, spot.0, spot.1, 2));
    }
    assert!(seen[1] < seen[0] && seen[2] < seen[1] - 2.0, "the denser, the darker: {seen:?}");
    assert!(seen[3] < seen[0] && seen[3] > seen[2], "a cloud's holes let more through: {seen:?}");
    // As traced: the floor before the slab of thick haze.
    let t = tracer(64).unwrap().render(&scenes[2].1, 0.0);
    keep("over-thick-haze-traced", &t);
    eprintln!("thick haze, traced: {:.0}", around(&t, spot.0, spot.1, 2));
    assert!((seen[2] - around(&t, spot.0, spot.1, 2)).abs() < 7.0, "{:.0} for the traced {:.0}", seen[2], around(&t, spot.0, spot.1, 2));
    assert!(seen[4] < seen[0] - 8.0, "a filled wireframe hides the sky: {seen:?}");
}

/// What a sky's ray ends on is lit by what it sees itself, not as if it
/// had the whole environment: one ray more goes on from it, and a last
/// one says whether that one's end is open. Under a table the floor is
/// lit by the top's underside, which the floor around lights and the
/// picture does not show; the open floor before a wall has the sky's own
/// light along its open rays. Both as the traced picture has them, on
/// the GPU and on the CPU alike.
#[test]
fn a_rays_end_is_lit_by_what_it_sees() {
    if tracer(1).is_none() {
        return;
    }
    let on_cpu = |c: &Compiled| Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() }).render(c, 0.0);
    let apart = |a: &RgbaImage, b: &RgbaImage| a.pixels().zip(b.pixels()).filter(|(p, q)| (0..4).any(|k| p.0[k].abs_diff(q.0[k]) > 3)).count() as f64 / (a.width() * a.height()) as f64;
    // Under the table, the default lights.
    let c = Compiled::new(&table());
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (a, b, t) = (raster(true, false).render(&c, 0.0), raster(false, false).render(&c, 0.0), tracer(64).unwrap().render(&c, 0.0));
    let (at, _) = (110..210i32).flat_map(|x| (100..150i32).map(move |y| (x, y))).map(|at| (at, around(&t, at.0, at.1, 4))).fold(((0, 0), f64::MAX), |m, v| if v.1 < m.1 { v } else { m });
    let (ours, plain, traced) = (around(&a, at.0, at.1, 4), around(&b, at.0, at.1, 4), around(&t, at.0, at.1, 4));
    let ours_cpu = on_cpu(&c);
    eprintln!("under the table at {at:?}: {ours:.0} with sky visibility, {plain:.0} plain, {traced:.0} traced; {:.1} dB from traced, {:.1} dB plain; GPU against CPU {:.2}% of px more than 3 levels apart", psnr(&a, &t), psnr(&b, &t), apart(&a, &ours_cpu) * 100.0);
    // (Lit as if it had the whole environment and with what the picture
    // shows for the second return, it was 173 for the traced 184.)
    assert!((ours - traced).abs() < 9.0 && plain > traced + 25.0, "{ours:.0} for {traced:.0} (plain {plain:.0})");
    assert!(psnr(&a, &t) > 33.0, "{:.1} dB", psnr(&a, &t));
    assert!(apart(&a, &ours_cpu) < 0.01);
    // The room under an environment without lobes: the open floor and the
    // wall, and the whole picture's brightness.
    let mut p = room(Some(json!([{"type": "ambient", "intensity": 0.2}])));
    p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#d9d9d9", "horizon": "#bfbfbf", "ground": "#808080"})).unwrap());
    let c = Compiled::new(&p);
    let (a, t) = (raster(true, false).render(&c, 0.0), tracer(64).unwrap().render(&c, 0.0));
    let mean = |img: &RgbaImage| img.pixels().map(|p| p.0[0] as f64 + p.0[1] as f64 + p.0[2] as f64).sum::<f64>() / (img.width() * img.height() * 3) as f64;
    let (floor, wall) = ((around(&a, 40, 165, 4), around(&t, 40, 165, 4)), (around(&a, 60, 30, 4), around(&t, 60, 30, 4)));
    let ours_cpu = on_cpu(&c);
    eprintln!("no lobes: the open floor {:.0} for the traced {:.0}, the wall {:.0} for {:.0}, the picture {:.1} for {:.1}, {:.1} dB; GPU against CPU {:.2}%", floor.0, floor.1, wall.0, wall.1, mean(&a), mean(&t), psnr(&a, &t), apart(&a, &ours_cpu) * 100.0);
    // (It was 180 for 184, 169 for 172, and 34.3 dB with the rays' ends
    // lit whole and nothing along the open rays.)
    assert!((floor.0 - floor.1).abs() < 4.0 && (wall.0 - wall.1).abs() < 5.0, "{floor:?}, {wall:?}");
    assert!((mean(&a) / mean(&t) - 1.0).abs() < 0.02 && psnr(&a, &t) > 34.5, "{:.1} for {:.1}, {:.1} dB", mean(&a), mean(&t), psnr(&a, &t));
    assert!(apart(&a, &ours_cpu) < 0.01);
}

/// Not a test (`--ignored`): the room under the default lights as a
/// project in $LAHZA_PT_OUT/room, for the CLI (`lahza motion snapshot
/// $LAHZA_PT_OUT/room --sky`, `--pt`, `--fast`: what each takes).
#[test]
#[ignore]
fn keep_the_room_as_a_project() {
    let Some(dir) = std::env::var_os("LAHZA_PT_OUT") else { return };
    let dir = std::path::Path::new(&dir).join("room");
    std::fs::create_dir_all(&dir).unwrap();
    let mut p = room(None);
    p.settings.width = 480;
    p.settings.height = 270;
    std::fs::write(dir.join("project.json"), serde_json::to_string_pretty(&p).unwrap()).unwrap();
}

/// Not a test (`--ignored --nocapture`): sky visibility on the GPU and on
/// the CPU against the traced frame (64 paths), in four scenes: the room
/// lit by the soft environment (a lobe overhead), by one without lobes,
/// and under the default lights; and the table. dB from the traced
/// picture, the mean brightness, and the darkest floor (under the ball;
/// under the table) and an open floor, in levels of 255.
#[test]
#[ignore]
fn measure_against_traced() {
    if tracer(1).is_none() {
        return;
    }
    let ambient = || Some(json!([{"type": "ambient", "intensity": 0.2}]));
    let mut even = room(ambient());
    even.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "none", "sky": "#d9d9d9", "horizon": "#bfbfbf", "ground": "#808080"})).unwrap());
    let mean = |img: &RgbaImage| img.pixels().map(|p| p.0[0] as f64 + p.0[1] as f64 + p.0[2] as f64).sum::<f64>() / (img.width() * img.height() * 3) as f64;
    for (name, p, dark) in [("the room", room(ambient()), (99, 129)), ("the room, no lobes", even, (99, 129)), ("the room, default lights", room(None), (99, 129)), ("the table", table(), (169, 123))] {
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let t = tracer(64).unwrap().render(&c, 0.0);
        let plain = raster(false, false).render(&c, 0.0);
        let on_gpu = raster(true, false).render(&c, 0.0);
        let on_cpu = Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() }).render(&c, 0.0);
        let tag = name.replace([' ', ','], "-");
        keep(&format!("against-{tag}-traced"), &t);
        keep(&format!("against-{tag}-sky"), &on_gpu);
        eprintln!("{name}: traced {:.1} over the picture, {:.0} at the dark floor, {:.0} at the open floor", mean(&t), around(&t, dark.0, dark.1, 4), around(&t, 40, 165, 4));
        for (which, img) in [("plain", &plain), ("sky visibility, GPU", &on_gpu), ("sky visibility, CPU", &on_cpu)] {
            eprintln!("    {which}: {:.1} dB, {:.1} over the picture, {:.0} at the dark floor, {:.0} at the open floor", psnr(img, &t), mean(img), around(img, dark.0, dark.1, 4), around(img, 40, 165, 4));
        }
    }
}

/// A volume scatters back what it takes of the sky's rays (the floor
/// beside a slab of red haze is redder than beside a black one, as
/// traced), and takes the sky itself: haze under a roof has less of the
/// environment's light than the same haze in the open. On the GPU and on
/// the CPU alike.
#[test]
fn a_volume_scatters_light_back_and_takes_the_sky() {
    if tracer(1).is_none() {
        return;
    }
    let on_cpu = |c: &Compiled| Renderer::new(None, RenderOptions { backend: Backend::Cpu, sky: true, ..Default::default() }).render(c, 0.0);
    let apart = |a: &RgbaImage, b: &RgbaImage| a.pixels().zip(b.pixels()).filter(|(p, q)| (0..4).any(|k| p.0[k].abs_diff(q.0[k]) > 3)).count() as f64 / (a.width() * a.height()) as f64;
    let rgb = |img: &RgbaImage, at: (i32, i32)| -> [f64; 3] {
        let mut sum = [0.0; 3];
        for (dx, dy) in (-2..=2).flat_map(|dy| (-2..=2).map(move |dx| (dx, dy))) {
            let p = img.get_pixel((at.0 + dx) as u32, (at.1 + dy) as u32).0;
            for k in 0..3 {
                sum[k] += p[k] as f64 / 25.0;
            }
        }
        sum
    };
    // The light back: a slab of thick haze hanging over the floor, red or black.
    let haze = |color: &str| {
        let mut p = canopy(false, None);
        p.scenes[0].layers.push(serde_json::from_value(json!({"id": "over", "kind": "mesh", "mesh": {"type": "box", "size": [120, 16], "depth": 100}, "material": {"volume": {"density": 5, "color": color}},
            "transform": {"position": [95, 98]}})).unwrap());
        Compiled::new(&p)
    };
    let (red, black) = (haze("#ff2010"), haze("#000000"));
    assert!(red.errors.is_empty(), "{:?}", red.errors);
    let spot = (95, 158);
    let (a, b) = (raster(true, false).render(&red, 0.0), raster(true, false).render(&black, 0.0));
    let (ta, tb) = (tracer(64).unwrap().render(&red, 0.0), tracer(64).unwrap().render(&black, 0.0));
    keep("haze-red", &a);
    keep("haze-black", &b);
    keep("haze-red-traced", &ta);
    keep("haze-black-traced", &tb);
    let (ours, traced) = ((rgb(&a, spot), rgb(&b, spot)), (rgb(&ta, spot), rgb(&tb, spot)));
    eprintln!("the floor before a slab of haze: red haze {:.0?}, black {:.0?}; traced {:.0?}, {:.0?}; GPU against CPU {:.2}% and {:.2}%", ours.0, ours.1, traced.0, traced.1, apart(&a, &on_cpu(&red)) * 100.0, apart(&b, &on_cpu(&black)) * 100.0);
    // Red comes back from the red haze, and as much more of it as traced.
    let more = |v: ([f64; 3], [f64; 3])| v.0[0] - v.1[0];
    assert!(more(ours) > 3.0 && ours.0[0] - ours.0[2] > ours.1[0] - ours.1[2] + 2.0, "{ours:?}");
    assert!((more(ours) - more(traced)).abs() < 0.6 * more(traced).max(4.0), "{:.1} more red for the traced {:.1}", more(ours), more(traced));
    assert!(apart(&a, &on_cpu(&red)) < 0.01 && apart(&b, &on_cpu(&black)) < 0.01);
    // Its own sky: a long box of haze, the right half of it under a roof.
    let build = |roof: bool| {
        let mut p = canopy(false, None);
        p.scenes[0].layers.push(serde_json::from_value(json!({"id": "haze", "kind": "mesh", "mesh": {"type": "box", "size": [260, 30], "depth": 80}, "material": {"volume": {"density": 3, "color": "#ffffff"}},
            "transform": {"position": [160, 112]}})).unwrap());
        if roof {
            p.scenes[0].layers.push(serde_json::from_value(json!({"id": "roof", "kind": "mesh", "mesh": {"type": "box", "size": [130, 5], "depth": 150}, "material": {"preset": "matte", "color": "#404040"},
                "transform": {"position": [228, 88]}})).unwrap());
        }
        Compiled::new(&p)
    };
    let (with, without) = (build(true), build(false));
    assert!(with.errors.is_empty(), "{:?}", with.errors);
    let (a, plain, bare, t) = (raster(true, false).render(&with, 0.0), raster(false, false).render(&with, 0.0), raster(true, false).render(&without, 0.0), tracer(64).unwrap().render(&with, 0.0));
    keep("roof-with", &a);
    keep("roof-plain", &plain);
    keep("roof-bare", &bare);
    keep("roof-traced", &t);
    let ours = on_cpu(&with);
    let (open, under) = ((60, 110), (230, 130));
    let l = |img: &RgbaImage, at: (i32, i32)| around(img, at.0, at.1, 2);
    eprintln!("haze in the open and under the roof: {:.0} and {:.0} with sky visibility, {:.0} and {:.0} without, {:.0} and {:.0} traced; without the roof {:.0} and {:.0}; GPU against CPU {:.2}%", l(&a, open), l(&a, under), l(&plain, open), l(&plain, under), l(&t, open), l(&t, under), l(&bare, open), l(&bare, under), apart(&a, &ours) * 100.0);
    assert!(l(&a, under) < l(&a, open) * 0.8 && l(&a, under) > l(&t, under) - 10.0, "the haze under the roof is darker, and no darker than traced");
    assert!((l(&plain, under) - l(&plain, open)).abs() < 6.0, "and was not, without sky visibility");
    assert!(l(&bare, under) > l(&bare, open) * 0.9, "nor is it without the roof");
    assert!(apart(&a, &ours) < 0.01);
}
