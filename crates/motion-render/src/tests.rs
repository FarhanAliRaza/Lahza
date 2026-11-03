use super::*;
use lahza_motion_core::document::{Project, Scene};
use lahza_motion_core::markers::MarkerTrack;
use lahza_motion_core::time::Fps;
use serde_json::json;

fn cpu() -> Renderer {
    Renderer::new(None, RenderOptions { backend: Backend::Cpu, ..Default::default() })
}

fn project(layers: serde_json::Value) -> Project {
    let mut p = Project::new(320, 180, Fps::whole(30), 2.0);
    p.settings.background = lahza_motion_core::value::Color::parse("#000000").unwrap();
    let scene: Scene = serde_json::from_value(json!({"id": "s1", "layers": layers})).unwrap();
    p.scenes.push(scene);
    p
}

fn px(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.get_pixel(x, y).0
}

#[test]
fn shapes_draw_where_and_how_they_should() {
    let p = project(json!([
        {"id": "r", "kind": "shape", "shape": "rect", "size": [100, 60], "radius": 10, "fill": "#ff0000", "transform": {"position": [80, 90]}},
        {"id": "c", "kind": "shape", "shape": "circle", "size": 60, "fill": "#00ff00", "transform": {"position": [220, 90]}},
        {"id": "ring", "kind": "shape", "shape": "ring", "size": 40, "thickness": 6, "stroke": {"color": "#0000ff", "width": 2}, "transform": {"position": [290, 30]}},
        {"id": "l", "kind": "shape", "shape": "line", "size": [100, 0], "thickness": 8, "trim": [0, 0.5], "fill": "#ffffff", "transform": {"position": [160, 160]}}
    ]));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let img = cpu().render(&c, 0.0);
    assert_eq!(px(&img, 80, 90), [255, 0, 0, 255]);
    assert_eq!(px(&img, 220, 90), [0, 255, 0, 255]);
    // Rounded corner is not filled.
    assert_eq!(px(&img, 31, 61), [0, 0, 0, 255]);
    // Outside the circle.
    assert_eq!(px(&img, 220, 55), [0, 0, 0, 255]);
    // Line trimmed to its left half.
    assert_eq!(px(&img, 120, 160), [255, 255, 255, 255]);
    assert_eq!(px(&img, 190, 160), [0, 0, 0, 255]);
    // Ring has a stroke but no fill: its centre stays empty.
    assert_eq!(px(&img, 290, 30), [0, 0, 0, 255]);
    let ring_edge = px(&img, 290 + 20 - 1, 30);
    assert!(ring_edge[2] > 100, "{ring_edge:?}");
}

#[test]
fn antialiased_edges_are_partial() {
    let p = project(json!([{"id": "c", "kind": "shape", "shape": "circle", "size": 101, "fill": "#ffffff", "transform": {"position": [160, 90]}}]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    let row: Vec<u8> = (100..160).map(|x| px(&img, x, 90)[0]).collect();
    assert!(row.iter().any(|v| *v > 10 && *v < 245), "{row:?}");
}

#[test]
fn text_renders_and_animates_per_glyph() {
    let p = project(json!([{"id": "t", "kind": "text", "text": "HELLO", "size": 60, "color": "#ffffff",
        "transform": {"position": [160, 90]},
        "animators": [{"at": 0.5, "from": {"y": 200}, "stagger": "2f", "dur": "6f"}]}]));
    let c = Compiled::new(&p);
    let mut r = cpu();
    let before = r.render(&c, 0.0);
    let lit = |img: &image::RgbaImage| img.pixels().filter(|p| p.0[0] > 128).count();
    assert_eq!(lit(&before), 0, "glyphs start below the frame");
    let after = r.render(&c, 1.5);
    assert!(lit(&after) > 500, "{}", lit(&after));
    // Text is centred on its position.
    let xs: Vec<u32> = after.enumerate_pixels().filter(|(_, _, p)| p.0[0] > 128).map(|(x, _, _)| x).collect();
    let mid = (xs.iter().min().unwrap() + xs.iter().max().unwrap()) / 2;
    assert!((mid as i32 - 160).abs() <= 3, "centre {mid}");
}

#[test]
fn rendering_is_deterministic_and_held_frames_are_cached() {
    let p = project(json!([
        {"id": "a", "kind": "shape", "shape": "star", "size": 80, "sides": 5, "inner": 0.45, "fill": "#ffcc00",
         "transform": {"position": {"keys": [{"at": 0, "v": [40, 90]}, {"at": 2, "v": [280, 90]}]}, "rotation": {"keys": [{"at": 0, "v": 0}, {"at": 2, "v": 180}]}},
         "time": {"step_fps": 12}},
        {"id": "b", "kind": "text", "text": "Lahza", "size": 40, "effects": [{"type": "blur", "radius": 3}], "transform": {"position": [160, 140]}}
    ]));
    let c = Compiled::new(&p);
    let mut r1 = cpu();
    let mut r2 = cpu();
    for n in 0..8 {
        let t = n as f64 / 30.0;
        assert_eq!(r1.render(&c, t), r2.render(&c, t), "frame {n}");
    }
    // 12 fps steps at 30 fps: of 8 frames at least 4 repeat their predecessor.
    assert!(r1.stats.cached >= 4, "{:?}", r1.stats);
}

#[test]
fn blur_spreads_and_keeps_energy() {
    let layers = |blur: f64| json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [60, 60], "fill": "#ffffff",
        "effects": [{"type": "blur", "radius": blur}], "transform": {"position": [160, 90]}}]);
    let sharp = cpu().render(&Compiled::new(&project(layers(0.0))), 0.0);
    for sigma in [2.0, 12.0] {
        let soft = cpu().render(&Compiled::new(&project(layers(sigma))), 0.0);
        let sum = |img: &image::RgbaImage| img.pixels().map(|p| p.0[0] as f64).sum::<f64>();
        let ratio = sum(&soft) / sum(&sharp);
        assert!((ratio - 1.0).abs() < 0.04, "sigma {sigma}: energy ratio {ratio}");
        assert!(px(&soft, 160 - 30 - (sigma as u32), 90)[0] > 5, "blur reaches outside the rect");
        assert!(px(&soft, 160, 90)[0] > 200);
    }
}

#[test]
fn motion_blur_smears_fast_moves() {
    let mut p = project(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [20, 40], "fill": "#ffffff",
        "transform": {"position": {"keys": [{"at": 0, "v": [0, 90]}, {"at": 1, "v": [600, 90]}]}}}]));
    p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: 8 });
    let c = Compiled::new(&p);
    let img = cpu().render(&c, 0.25);
    let row: Vec<u8> = (100..200).map(|x| px(&img, x, 90)[0]).collect();
    let partial = row.iter().filter(|v| **v > 20 && **v < 235).count();
    assert!(partial > 6, "{row:?}");
}

#[test]
fn images_and_gradients() {
    let dir = std::env::temp_dir().join(format!("lahza-motion-render-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut img = image::RgbaImage::new(64, 32);
    for (x, _, p) in img.enumerate_pixels_mut() {
        *p = if x < 32 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) };
    }
    img.save(dir.join("pic.png")).unwrap();
    let p = project(json!([
        {"id": "i", "kind": "image", "src": "pic.png", "size": [128, 64], "transform": {"position": [100, 90]}},
        {"id": "g", "kind": "shape", "shape": "rect", "size": [100, 40], "transform": {"position": [250, 90]},
         "fill": {"gradient": "linear", "from": [-50, 0], "to": [50, 0], "stops": [[0, "#000000"], [1, "#ffffff"]]}}
    ]));
    let mut r = Renderer::new(Some(&dir), RenderOptions { backend: Backend::Cpu, ..Default::default() });
    let out = r.render(&Compiled::new(&p), 0.0);
    assert_eq!(px(&out, 60, 90), [255, 0, 0, 255]);
    assert_eq!(px(&out, 140, 90), [0, 0, 255, 255]);
    let g0 = px(&out, 210, 90)[0];
    let g1 = px(&out, 290, 90)[0];
    assert!(g0 < 70 && g1 > 185, "{g0} {g1}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn shader_validates_and_layouts_match() {
    let module = naga::front::wgsl::parse_str(gpu::SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(gpu::SHADER)));
    let flags = naga::valid::ValidationFlags::all() ^ naga::valid::ValidationFlags::BINDINGS;
    naga::valid::Validator::new(flags, naga::valid::Capabilities::empty()).validate(&module).unwrap();
    let size = |name: &str| {
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some(name)).unwrap();
        match ty.inner {
            naga::TypeInner::Struct { span, .. } => span as usize,
            _ => panic!(),
        }
    };
    assert_eq!(size("Prim"), std::mem::size_of::<list::Prim>());
    assert_eq!(size("Pass"), 64);
    assert_eq!(size("BlurPass"), 32);
    assert_eq!(module.entry_points.len(), 3);
}

/// GPU and CPU draw the same picture. Runs when a Vulkan device exists
/// (set LAHZA_MOTION_SOFTWARE_GPU=1 to accept llvmpipe); skips otherwise.
#[test]
fn gpu_matches_cpu() {
    let mut p = project(json!([
        {"id": "bg", "kind": "shape", "shape": "rect", "size": [320, 180], "fill": {"gradient": "radial", "from": [0, 0], "to": [180, 0], "stops": [[0, "#203040"], [1, "#000000"]]}, "transform": {"position": [160, 90]}},
        {"id": "a", "kind": "shape", "shape": "arc", "size": 120, "thickness": 14, "angles": [-90, 200], "fill": "#ff3355", "blend": "screen", "transform": {"position": [90, 90], "rotation": 15}},
        {"id": "s", "kind": "shape", "shape": "polygon", "sides": 6, "size": 70, "radius": 6, "fill": "#33ddaa", "stroke": {"color": "#ffffff", "width": 3}, "transform": {"position": [230, 70], "scale": [1.3, 0.8]}},
        {"id": "t", "kind": "text", "text": "Sync the peak", "size": 28, "color": "#ffffff", "transform": {"position": [160, 150], "rotation": -4},
         "animators": [{"at": 0, "from": {"y": 30, "opacity": 0, "scale": 0.5}, "dur": "10f"}]},
        {"id": "rim", "kind": "text", "text": "RIM", "size": 44, "color": "#2a3a8a", "weight": 1.5,
         "stroke": {"color": "#ffffff", "width": 2.5}, "edge": {"color": "#ffe9a0", "width": 4, "angle": -30},
         "transform": {"position": [70, 140], "rotation": 8},
         "animators": [{"at": 0, "order": {"radial": [0.3, 0.5], "feather": 0.4}, "from": {"opacity": 0, "y": 12, "rotation": 20}, "stagger": "2f", "dur": "6f"}]},
        {"id": "wipe", "kind": "text", "text": "WIPE\nIN", "size": 26, "color": "#ffd23f",
         "stroke": {"color": "#ff3355", "width": 3, "align": "center"}, "transform": {"position": [270, 150]},
         "animators": [{"at": 0, "order": {"axis": 30, "from": "center", "feather": 0.3}, "from": {"opacity": 0}, "stagger": "1f", "dur": "6f"},
                       {"at": 0, "unit": "word", "order": {"axis": "y", "from": "end"}, "from": {"scale": 0.6}, "stagger": "3f", "dur": "8f"}]},
        {"id": "b", "kind": "shape", "shape": "capsule", "size": [80, 24], "fill": "#ffd23f", "effects": [{"type": "blur", "radius": 9}], "transform": {"position": [240, 130]}},
        {"id": "fx", "kind": "shape", "shape": "rect", "size": [120, 60], "fill": {"gradient": "linear", "from": [-60, 0], "to": [60, 0], "stops": [[0, "#ff0000"], [1, "#00ffff"]]},
         "blend": "soft_light", "transform": {"position": [230, 40]},
         "effects": [{"type": "hue_saturation", "hue": 40, "saturation": 0.3}, {"type": "levels", "gamma": 1.4}, {"type": "gradient_map", "stops": [[0, "#101030"], [1, "#ffe0a0"]], "amount": 0.5},
                     {"type": "displace", "amount": 3, "scale": 12}, {"type": "rgb_split", "amount": 2}, {"type": "directional_blur", "angle": 30, "length": 6},
                     {"type": "vignette", "amount": 0.5}, {"type": "grain", "amount": 0.1}]},
        {"id": "c3", "kind": "comp", "three_d": true, "transform": {"position": [160, 120], "rotation_y": 35, "rotation_x": -20, "z": 60},
         "layers": [{"id": "c3a", "kind": "shape", "shape": "rect", "size": [90, 50], "radius": 8, "fill": "#7b61ff", "stroke": {"color": "#ffffff", "width": 2}},
                    {"id": "c3t", "kind": "text", "text": "3D", "size": 30, "color": "#ffffff"}]},
        {"id": "m", "kind": "shape", "shape": "path", "d": "M -40 -30 C 0 -60 40 -30 40 0 L 0 40 Z", "fill": "#ffffff", "transform": {"position": [60, 40]}},
        {"id": "g", "kind": "comp", "transform": {"opacity": 0.8}, "matte": {"layer": "m", "mode": "luma"},
         "effects": [{"type": "shadow", "offset": [4, 6], "radius": 3}, {"type": "glow", "color": "#88ccff", "radius": 6, "intensity": 0.6}],
         "layers": [
            {"id": "g1", "kind": "shape", "shape": "rect", "size": [70, 50], "fill": "#3366ff", "blend": "overlay", "transform": {"position": [55, 40]}},
            {"id": "g2", "kind": "comp", "effects": [{"type": "blur", "radius": 2}], "layers": [
                {"id": "g3", "kind": "shape", "shape": "path", "d": "M -30 0 Q 0 -40 30 0", "stroke": {"color": "#ff8800", "width": 5}, "trim": [0.1, 0.8], "transform": {"position": [60, 45]}}]}]}
    ]));
    p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: 4 });
    assert_gpu_matches_cpu(&p, &[0.0, 0.1, 0.5]);
}

/// The newer paints and depth of field: box gradients, a warped field,
/// gradient strokes (plain, dashed, feathered and trimmed) and rack focus.
#[test]
fn gpu_matches_cpu_fields_strokes_and_focus() {
    let mut p = project(json!([
        {"id": "f", "kind": "shape", "shape": "field", "size": [320, 180], "warp": 12, "warp_scale": 40, "softness": 0.8,
         "points": [{"position": {"keys": [{"at": 0, "v": [-100, -40]}, {"at": 1, "v": [60, 30]}]}, "color": "#ff3355", "radius": 50},
                    {"position": [90, 30], "color": "#3355ff", "radius": 70}, {"position": [0, 60], "color": "#ffd23f80", "radius": 30}],
         "transform": {"position": [160, 90]}},
        {"id": "bx", "kind": "shape", "shape": "rect", "size": {"keys": [{"at": 0, "v": [60, 40]}, {"at": 1, "v": [120, 40]}]},
         "fill": {"gradient": "linear", "units": "box", "from": [0, 0], "to": [1, 1], "stops": [[0, "#000000"], [1, "#00ff88"]]}, "transform": {"position": [80, 40]}},
        {"id": "br", "kind": "shape", "shape": "circle", "size": [90, 50],
         "fill": {"gradient": "radial", "units": "box", "from": [0.5, 0.5], "to": [1, 0.5], "stops": [[0, "#ffffff"], [1, "#ff000000"]]}, "transform": {"position": [240, 40]}},
        {"id": "st1", "kind": "shape", "shape": "path", "d": "M 20 120 C 80 60 140 180 200 120 S 280 100 300 150",
         "stroke": {"width": 9, "gradient": {"stops": [[0, "#ff0000"], [0.5, "#00ff00"], [1, "#0000ff"]]}}, "trim": [0.1, 0.9]},
        {"id": "s2", "kind": "shape", "shape": "path", "d": {"shape": "star", "size": 90, "sides": 5},
         "stroke": {"width": 6, "cap": "butt", "join": "miter", "dash": [14, 6], "feather": 40, "gradient": {"stops": [[0, "#ffff00"], [1, "#ff00ff"]]}},
         "trim": {"keys": [{"at": 0, "v": [0, 0.3]}, {"at": 1, "v": [0, 1]}]}, "transform": {"position": [250, 120]}},
        {"id": "near", "kind": "shape", "shape": "rect", "size": [60, 60], "fill": "#ffffff", "three_d": true, "transform": {"position": [100, 120], "z": -50}},
        {"id": "far", "kind": "text", "text": "DOF", "size": 40, "color": "#00ffff", "three_d": true, "transform": {"position": [150, 150], "z": 300}}
    ]));
    p.scenes[0].camera.aperture = lahza_motion_core::property::Property::Constant(2.0);
    p.scenes[0].camera.focus = Some(serde_json::from_value(json!({"keys": [{"at": 0, "v": -50}, {"at": 1, "v": 300}]})).unwrap());
    assert_gpu_matches_cpu(&p, &[0.0, 0.4, 1.0]);
    // Motion-blur samples merge several lists (segments and stops rebased).
    p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: 3 });
    assert_gpu_matches_cpu(&p, &[0.5]);
}

pub(crate) fn assert_gpu_matches_cpu(p: &Project, times: &[f64]) {
    assert_gpu_matches_cpu_within(p, times, 0.0);
}

/// `outliers`: fraction of pixels allowed to differ by more than 3 levels
/// (mesh edges: a sample on a triangle edge can fall either way when the
/// GPU fuses multiply-adds).
pub(crate) fn assert_gpu_matches_cpu_within(p: &Project, times: &[f64], outliers: f64) {
    let c = Compiled::new(p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let mut gpu = Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, ..Default::default() });
    if !gpu.is_gpu() {
        eprintln!("skipping gpu_matches_cpu: {}", gpu.backend_label());
        return;
    }
    let mut cpu = cpu();
    for &t in times {
        let a = gpu.render(&c, t);
        let b = cpu.render(&c, t);
        let mut max = 0u8;
        let mut total = 0u64;
        let mut over = 0usize;
        for (pa, pb) in a.pixels().zip(b.pixels()) {
            let mut m = 0u8;
            for k in 0..4 {
                let d = pa.0[k].abs_diff(pb.0[k]);
                m = m.max(d);
                total += d as u64;
            }
            if m > 3 {
                over += 1;
            } else {
                max = max.max(m);
            }
        }
        let allowed = (outliers * (a.width() * a.height()) as f64) as usize;
        if let Ok(dir) = std::env::var("LAHZA_MOTION_DUMP") {
            a.save(format!("{dir}/gpu-{t}.png")).unwrap();
            b.save(format!("{dir}/cpu-{t}.png")).unwrap();
        }
        assert!(over <= allowed, "t={t}: {over} pixels differ by more than 3 levels (allowed {allowed})");
        let mean = total as f64 / (a.len() as f64);
        if let Ok(dir) = std::env::var("LAHZA_MOTION_DUMP") {
            a.save(format!("{dir}/gpu-{t}.png")).unwrap();
            b.save(format!("{dir}/cpu-{t}.png")).unwrap();
        }
        assert!(max <= 3 && mean < 0.05, "t={t}: max diff {max}, mean {mean}");
        // And the GPU is deterministic too.
        let mut again = Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, ..Default::default() });
        assert_eq!(again.render(&c, t), a);
    }
}

/// Phase 1 target: a 1080p scene with 200 animated shapes and text in real
/// time. `cargo test -p lahza-motion-render --release -- --ignored --nocapture benchmark`
#[test]
#[ignore]
fn benchmark_200_shapes_and_text_1080p() {
    let mut layers = Vec::new();
    for i in 0..200 {
        let (x, y) = (60.0 + (i % 20) as f64 * 90.0, 80.0 + (i / 20) as f64 * 95.0);
        let shape = ["circle", "rect", "capsule", "ring", "star"][i % 5];
        let fill = ["#ff3355", "#33ddaa", "#ffd23f", "#7b61ff", "#ffffff"][i % 5];
        layers.push(json!({"id": format!("s{i}"), "kind": "shape", "shape": shape, "size": [60, 40], "radius": 8, "sides": 5,
            "fill": fill,
            "transform": {
                "position": {"keys": [{"at": 0, "from": [x, y + 300.0], "v": [x, y], "ease": "glide", "dur": 0.7, "anchor": "start"}],
                             "add": [{"type": "wiggle", "hz": 1.5, "amp": [6, 6], "seed": i}]},
                "rotation": {"keys": [{"at": 0, "v": 0}, {"at": 4, "v": 360}]},
                "scale": {"keys": [{"at": (i % 10) as f64 * 0.033, "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]}}}));
    }
    for (i, word) in ["MOVE", "FAST", "SYNC", "THE PEAK"].iter().enumerate() {
        layers.push(json!({"id": format!("t{i}"), "kind": "text", "text": word, "size": 120, "transform": {"position": [960, 200 + i * 220]},
            "animators": [{"at": 0.2 * i as f64, "from": {"y": 80, "opacity": 0}, "stagger": "1f", "dur": "8f"}]}));
    }
    let mut p = Project::new(1920, 1080, Fps::whole(60), 4.0);
    p.markers.insert(MarkerTrack::from_times("beat", [0.5]));
    p.scenes.push(serde_json::from_value(json!({"id": "s", "layers": layers})).unwrap());
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    for backend in [Backend::Gpu, Backend::Cpu] {
        let mut r = Renderer::new(None, RenderOptions { backend, motion_blur: false, ..Default::default() });
        if backend == Backend::Gpu && !r.is_gpu() {
            eprintln!("no GPU: {}", r.backend_label());
            continue;
        }
        r.render(&c, 0.0);
        let frames = 60;
        let started = Instant::now();
        let mut eval_ms = 0.0;
        for n in 0..frames {
            let t = 0.5 + n as f64 / 60.0;
            let e = Instant::now();
            let _ = c.frame(t);
            eval_ms += e.elapsed().as_secs_f64() * 1000.0;
            r.render(&c, t);
        }
        let per = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
        eprintln!(
            "{}: {per:.2} ms/frame ({:.1} fps); evaluate {:.2} ms, build {:.2} ms, raster+readback {:.2} ms",
            r.backend_label(),
            1000.0 / per,
            eval_ms / frames as f64,
            r.stats.build_ms / r.stats.frames as f64,
            r.stats.raster_ms / r.stats.frames as f64
        );
    }
}

fn avg(img: &image::RgbaImage, x: u32, y: u32) -> [f64; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0] as f64, p[1] as f64, p[2] as f64]
}

#[test]
fn text_is_shaped_with_kerning() {
    let mut fonts = font::Fonts::new(None);
    let width = |fonts: &mut font::Fonts, s: &str| {
        let t = lahza_motion_core::compile::TextState {
            text: s.into(), font: "Inter SemiBold".into(), size: 100.0, color: lahza_motion_core::value::Color::WHITE,
            tracking: 0.0, line_height: 1.2, align: lahza_motion_core::document::Align::Left, glyphs: Vec::new(),
            pivots: vec![u32::MAX; s.chars().count()].into(), weight: 0.0, stroke: None, edge: None, sweeps: Vec::new(), units: Vec::new().into(),
        };
        let l = fonts.layout(&t);
        l.bounds[2] - l.bounds[0]
    };
    let av = width(&mut fonts, "AV");
    let apart = width(&mut fonts, "A") + width(&mut fonts, "V");
    assert!(av < apart - 2.0, "kerned AV {av} vs {apart}");
}

#[test]
fn precomp_opacity_applies_to_the_flattened_group() {
    let p = project(json!([{"id": "g", "kind": "comp", "transform": {"opacity": 0.5}, "layers": [
        {"id": "a", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ffffff", "transform": {"position": [130, 90]}},
        {"id": "b", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ffffff", "transform": {"position": [190, 90]}}
    ]}]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    // Overlap (160) looks the same as a single rect (100): one picture at 50%.
    assert_eq!(px(&img, 160, 90), px(&img, 100, 90));
    assert!((px(&img, 100, 90)[0] as i32 - 128).abs() <= 1);
}

#[test]
fn track_mattes() {
    let layers = |mode: &str| json!([
        {"id": "m", "kind": "shape", "shape": "circle", "size": 80, "fill": "#ffffff", "transform": {"position": [160, 90]}},
        {"id": "r", "kind": "shape", "shape": "rect", "size": [200, 120], "fill": "#ff0000", "transform": {"position": [160, 90]}, "matte": {"layer": "m", "mode": mode}}
    ]);
    let alpha = cpu().render(&Compiled::new(&project(layers("alpha"))), 0.0);
    assert_eq!(px(&alpha, 160, 90), [255, 0, 0, 255], "inside the matte");
    assert_eq!(px(&alpha, 80, 90), [0, 0, 0, 255], "outside the matte; the matte layer itself is not drawn");
    let inv = cpu().render(&Compiled::new(&project(layers("alpha_inverted"))), 0.0);
    assert_eq!(px(&inv, 160, 90), [0, 0, 0, 255]);
    assert_eq!(px(&inv, 80, 90), [255, 0, 0, 255]);
    let luma = cpu().render(&Compiled::new(&project(layers("luma"))), 0.0);
    assert_eq!(px(&luma, 160, 90), [255, 0, 0, 255]);    // A hidden matte source (the After Effects habit) still mattes.
    let mut hidden = layers("alpha_inverted");
    hidden[0]["hidden"] = json!(true);
    let inv_hidden = cpu().render(&Compiled::new(&project(hidden)), 0.0);
    assert_eq!(px(&inv_hidden, 160, 90), [0, 0, 0, 255]);
    assert_eq!(px(&inv_hidden, 80, 90), [255, 0, 0, 255]);
}

#[test]
fn stroke_caps_joins_dashes_and_feather() {
    let path = |d: &str, extra: serde_json::Value| {
        let mut stroke = json!({"color": "#ffffff", "width": 20});
        stroke.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let p = project(json!([{"id": "p", "kind": "shape", "shape": "path", "d": d, "stroke": stroke}]));
        cpu().render(&Compiled::new(&p), 0.0)
    };
    // Caps: 5 px past the end of a horizontal stroke.
    assert_eq!(px(&path("M 60 40 H 260", json!({})), 55, 40)[0], 255, "round cap (default) covers past the end");
    assert_eq!(px(&path("M 60 40 H 260", json!({"cap": "butt"})), 55, 40)[0], 0, "butt cap stops at the end");
    assert_eq!(px(&path("M 60 40 H 260", json!({"cap": "square"})), 55, 40)[0], 255, "square cap extends half a width");
    // Joins: just outside the round corner of a right angle, inside a miter.
    let corner = "M 100 60 V 140 H 200";
    assert_eq!(px(&path(corner, json!({})), 92, 148)[0], 0, "round join is rounded");
    assert_eq!(px(&path(corner, json!({"join": "miter"})), 92, 148)[0], 255, "miter join is square");
    // Dashes: 20 on, 20 off from x = 20.
    let dashed = path("M 20 90 H 300", json!({"cap": "butt", "dash": [20, 20]}));
    assert_eq!(px(&dashed, 30, 90)[0], 255, "inside a dash");
    assert_eq!(px(&dashed, 50, 90)[0], 0, "inside a gap");
    // Feather: the last 100 px fade out toward the end.
    let feathered = path("M 20 90 H 300", json!({"cap": "butt", "feather": 100}));
    assert_eq!(px(&feathered, 100, 90)[0], 255, "before the feather");
    let (mid, end) = (px(&feathered, 250, 90)[0], px(&feathered, 296, 90)[0]);
    assert!(mid > 60 && mid < 200 && end < 30, "fades toward the end: {mid} then {end}");
    // A fading tail: the first 100 px fade in from the start.
    let tail = path("M 20 90 H 300", json!({"cap": "butt", "feather_start": 100}));
    assert!(px(&tail, 24, 90)[0] < 30 && px(&tail, 200, 90)[0] == 255, "tail {:?} body {:?}", px(&tail, 24, 90), px(&tail, 200, 90));
}

#[test]
fn glow_and_shadow_follow_the_matte() {
    // A wide rect matted to a small circle, with a glow and a drop shadow:
    // both must come from the circle the viewer sees, not from the rect.
    let p = project(json!([
        {"id": "m", "kind": "shape", "shape": "circle", "size": 40, "fill": "#ffffff", "transform": {"position": [160, 90]}},
        {"id": "r", "kind": "shape", "shape": "rect", "size": [240, 120], "fill": "#ff0000", "transform": {"position": [160, 90]},
         "matte": {"layer": "m", "mode": "alpha"},
         "effects": [{"type": "glow", "color": "#ffffff", "radius": 6, "intensity": 1},
                     {"type": "shadow", "color": "#ffffff", "offset": [0, 0], "radius": 3}]}
    ]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    // Inside the rect but far outside the circle and the glow's reach.
    assert_eq!(px(&img, 60, 50), [0, 0, 0, 255], "the glow drew the unmatted rect");
    assert_eq!(px(&img, 260, 130), [0, 0, 0, 255], "the shadow drew the unmatted rect");
    // Just outside the circle the glow still shows.
    assert!(px(&img, 160, 66)[0] > 10, "no glow around the matted circle: {:?}", px(&img, 160, 66));
}

#[test]
fn shadow_and_glow() {
    let p = project(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [80, 40], "fill": "#808080", "transform": {"position": [160, 90]},
        "effects": [{"type": "shadow", "color": "#000000", "offset": [0, 30], "radius": 4}, {"type": "glow", "color": "#ffffff", "radius": 8, "intensity": 1}]}]));
    let mut p2 = p.clone();
    p2.settings.background = lahza_motion_core::value::Color::parse("#ffffff").unwrap();
    let on_white = cpu().render(&Compiled::new(&p2), 0.0);
    // Below the rect the shadow darkens the white background.
    assert!(px(&on_white, 160, 90 + 20 + 15)[0] < 150, "{:?}", px(&on_white, 160, 125));
    let on_black = cpu().render(&Compiled::new(&p), 0.0);
    // Just left of the rect the glow brightens the black background.
    assert!(px(&on_black, 160 - 40 - 4, 80)[0] > 15, "{:?}", px(&on_black, 116, 80));
    // The rect itself got brighter from the additive glow.
    assert!(px(&on_black, 160, 90)[0] > 128);
}

#[test]
fn bezier_paths_fill_and_trim() {
    let p = project(json!([
        {"id": "tri", "kind": "shape", "shape": "path", "d": "M 0 -50 L 50 40 L -50 40 Z", "fill": "#00ff00", "transform": {"position": [90, 90]}},
        {"id": "arc", "kind": "shape", "shape": "path", "d": "M -60 0 C -60 -60 60 -60 60 0", "stroke": {"color": "#ffffff", "width": 8}, "trim": [0, 0.5], "transform": {"position": [240, 110]}}
    ]));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let img = cpu().render(&c, 0.0);
    assert_eq!(px(&img, 90, 100), [0, 255, 0, 255]);
    assert_eq!(px(&img, 50, 60), [0, 0, 0, 255], "outside the triangle");
    // Left half of the curve is drawn, the right half is trimmed away.
    assert_eq!(px(&img, 180, 110)[0], 255);
    assert_eq!(px(&img, 300, 110)[0], 0);
}

#[test]
fn follower_sits_on_the_drawn_head() {
    // A dot riding the head of a curve drawn on with trim keys.
    let p = project(json!([
        {"id": "curve", "kind": "shape", "shape": "path", "d": "M -120 0 C -60 -90 60 90 120 0", "stroke": {"color": "#ffffff", "width": 10, "cap": "butt"},
         "transform": {"position": [160, 90]}, "trim": {"keys": [{"at": 0, "v": [0, 0]}, {"at": 1, "v": [0, 1]}]}},
        {"id": "dot", "kind": "shape", "shape": "circle", "size": 6, "fill": "#ff0000", "follow": {"layer": "curve"}}
    ]));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let path = lahza_motion_core::path::flatten("M -120 0 C -60 -90 60 90 120 0", lahza_motion_core::path::tolerance(1.0)).unwrap();
    let mut r = cpu();
    for t in [0.3, 0.5, 0.8] {
        let img = r.render(&c, t);
        let ((x, y), (dx, dy)) = lahza_motion_core::path::point_at(&path, t).unwrap();
        let (x, y) = (x + 160.0, y + 90.0);
        assert_eq!(px(&img, x as u32, y as u32), [255, 0, 0, 255], "dot on the head at {t}");
        // Just behind the dot: the stroke; just ahead: nothing drawn yet.
        assert_eq!(px(&img, (x - dx * 6.0) as u32, (y - dy * 6.0) as u32), [255, 255, 255, 255], "stroke behind at {t}");
        assert_eq!(px(&img, (x + dx * 6.0) as u32, (y + dy * 6.0) as u32), [0, 0, 0, 255], "nothing ahead at {t}");
    }
}

#[test]
fn scene_transitions() {
    let scenes = |kind: &str| {
        let mut p = Project::new(320, 180, Fps::whole(30), 4.0);
        p.scenes.push(serde_json::from_value(json!({"id": "a", "background": "#ff0000"})).unwrap());
        p.scenes.push(serde_json::from_value(json!({"id": "b", "start": 2, "background": "#0000ff",
            "transition": {"type": kind, "dur": 1, "ease": "linear", "direction": "left"}})).unwrap());
        Compiled::new(&p)
    };
    let fade = cpu().render(&scenes("fade"), 2.0);
    let c = avg(&fade, 160, 90);
    assert!((c[0] - 128.0).abs() < 3.0 && (c[2] - 128.0).abs() < 3.0, "{c:?}");
    let wipe = cpu().render(&scenes("wipe"), 2.0);
    assert_eq!(px(&wipe, 40, 90), [255, 0, 0, 255]);
    assert_eq!(px(&wipe, 280, 90), [0, 0, 255, 255]);
    let push = cpu().render(&scenes("push"), 2.0);
    assert_eq!(px(&push, 40, 90), [255, 0, 0, 255]);
    assert_eq!(px(&push, 280, 90), [0, 0, 255, 255]);
    // Whip blurs with repeated edges: the frame border keeps full colour.
    let whip = cpu().render(&scenes("whip"), 1.8);
    assert_eq!(px(&whip, 0, 0), [255, 0, 0, 255], "no dark fade at the frame edge");
    assert_eq!(px(&whip, 0, 179), [255, 0, 0, 255]);
    // Before and after the transition window: plain cuts.
    assert_eq!(px(&cpu().render(&scenes("zoom"), 1.4), 160, 90), [255, 0, 0, 255]);
    assert_eq!(px(&cpu().render(&scenes("whip"), 2.6), 160, 90), [0, 0, 255, 255]);
}



#[test]
fn covered_background_does_not_leak_through_a_whip() {
    // Scene a: yellow background fully covered by a black rect.
    // 1366 wide: the whip blur runs at reduced resolution on a grid that
    // does not divide the width evenly.
    let mut p = Project::new(1366, 180, Fps::whole(30), 4.0);
    p.scenes.push(serde_json::from_value(json!({"id": "a", "background": "#d7ff3a", "layers": [
        {"id": "k", "kind": "shape", "shape": "rect", "size": [1366, 180], "fill": "#000000", "transform": {"position": [683, 90]}}]})).unwrap());
    p.scenes.push(serde_json::from_value(json!({"id": "b", "start": 2, "background": "#000000",
        "transition": {"type": "whip", "dur": 1, "ease": "linear", "direction": "left"}})).unwrap());
    let c = Compiled::new(&p);
    for t in [1.6, 1.75, 1.9, 2.0, 2.2] {
        let img = cpu().render(&c, t);
        let (x, y, worst) = img.enumerate_pixels().map(|(x, y, p)| (x, y, p.0[1])).max_by_key(|v| v.2).unwrap();
        assert!(worst <= 2, "t={t}: green {worst} at ({x}, {y}) leaks from the covered background; row {:?}", (x.saturating_sub(8)..x + 8).map(|x| img.get_pixel(x.min(1365), 90).0[1]).collect::<Vec<_>>());
    }
}

fn ffmpeg(args: &[&str]) -> bool {
    std::process::Command::new("ffmpeg").args(["-v", "error", "-y"]).args(args).status().is_ok_and(|s| s.success())
}

/// red 0–1 s, green 1–2 s, blue 2–3 s at 30 fps, with a 440 Hz tone.
fn rgb_video(dir: &std::path::Path) -> bool {
    std::fs::create_dir_all(dir).unwrap();
    ffmpeg(&[
        "-f", "lavfi", "-i", "color=c=red:s=64x36:r=30:d=1",
        "-f", "lavfi", "-i", "color=c=lime:s=64x36:r=30:d=1",
        "-f", "lavfi", "-i", "color=c=blue:s=64x36:r=30:d=1",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
        "-filter_complex", "[0][1][2]concat=n=3:v=1:a=0[v]", "-map", "[v]", "-map", "3:a",
        "-c:v", "libx264", "-g", "15", "-pix_fmt", "yuv444p", "-crf", "0", "-c:a", "aac",
        dir.join("rgb.mp4").to_str().unwrap(),
    ])
}

fn footage_project(dir: &std::path::Path, layer: serde_json::Value) -> (Compiled, Project) {
    let _ = dir;
    let mut p = project(json!([layer]));
    p.settings.duration = 4.0;
    (Compiled::new(&p), p)
}

fn dominant(img: &image::RgbaImage) -> char {
    let p = img.get_pixel(160, 90).0;
    if p[0] > 200 && p[1] < 60 { 'r' } else if p[1] > 200 && p[0] < 60 { 'g' } else if p[2] > 200 && p[0] < 60 { 'b' } else { '?' }
}

#[test]
fn video_footage_frames_trim_speed_loop_remap() {
    let dir = std::env::temp_dir().join(format!("lahza-footage-{}", std::process::id()));
    if !rgb_video(&dir) {
        eprintln!("skipping: ffmpeg cannot make the test video");
        return;
    }
    let mut r = Renderer::new(Some(&dir), RenderOptions { backend: Backend::Cpu, ..Default::default() });
    let mut colours = |layer: serde_json::Value, times: &[f64]| -> String {
        let (c, _) = footage_project(&dir, layer);
        times.iter().map(|t| dominant(&r.render(&c, *t))).collect()
    };
    let base = json!({"id": "v", "kind": "video", "src": "rgb.mp4", "size": [320, 180], "transform": {"position": [160, 90]}});
    assert_eq!(colours(base.clone(), &[0.0, 0.5, 0.99, 1.0, 1.5, 2.2, 2.97]), "rrrggbb");
    let mut trimmed = base.clone();
    trimmed["trim"] = json!(1.0);
    assert_eq!(colours(trimmed, &[0.0, 0.9, 1.1]), "ggb");
    let mut fast = base.clone();
    fast["speed"] = json!(2.0);
    assert_eq!(colours(fast.clone(), &[0.25, 0.75, 1.25, 1.9]), "rgbb", "speed 2, holding the last frame");
    fast["loop"] = json!(true);
    // 2× speed: 1.6 s → source 3.2 s → 0.2 s after the wrap; 2.1 s → 1.2 s.
    assert_eq!(colours(fast, &[1.6, 2.1]), "rg", "looping restarts");
    let mut remap = base.clone();
    remap["time_remap"] = json!({"keys": [{"at": 0, "v": 2.5}, {"at": 2, "v": 0.2}]});
    assert_eq!(colours(remap, &[0.0, 1.0, 1.99]), "bgr", "reverse through time remap");
    // Playing forward reads the stream in order: few seeks for many frames.
    let (c, _) = footage_project(&dir, base);
    let seeks = r.footage.seeks;
    for n in 0..60 {
        r.render(&c, n as f64 / 30.0);
    }
    assert!(r.footage.seeks - seeks <= 2, "{} seeks", r.footage.seeks - seeks);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn image_sequences_and_alpha_video() {
    let dir = std::env::temp_dir().join(format!("lahza-seq-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("shot")).unwrap();
    for (i, c) in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 128]].iter().enumerate() {
        image::RgbaImage::from_pixel(16, 9, image::Rgba(*c)).save(dir.join(format!("shot/f_{:04}.png", i + 1))).unwrap();
    }
    let mut r = Renderer::new(Some(&dir), RenderOptions { backend: Backend::Cpu, ..Default::default() });
    let layer = json!({"id": "s", "kind": "video", "src": "shot/f_####.png", "fps": 10, "size": [320, 180], "transform": {"position": [160, 90]}});
    let (c, _) = footage_project(&dir, layer);
    assert_eq!(dominant(&r.render(&c, 0.05)), 'r');
    assert_eq!(dominant(&r.render(&c, 0.15)), 'g');
    let half = r.render(&c, 0.25).get_pixel(160, 90).0;
    assert!((half[2] as i32 - 128).abs() <= 2 && half[0] == 0, "half-transparent blue over black: {half:?}");
    // A folder works the same.
    let (c, _) = footage_project(&dir, json!({"id": "s", "kind": "video", "src": "shot", "fps": 10, "size": [320, 180], "transform": {"position": [160, 90]}}));
    assert_eq!(dominant(&r.render(&c, 0.15)), 'g');
    // VP9 with alpha (how Blender or AE hand over transparent footage).
    if ffmpeg(&["-f", "lavfi", "-i", "color=c=white@0.5:s=64x36:r=30:d=1,format=rgba", "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-auto-alt-ref", "0", dir.join("alpha.webm").to_str().unwrap()]) {
        let info = crate::footage::probe(&dir.join("alpha.webm")).unwrap();
        assert!(info.has_alpha, "{info:?}");
        let (c, _) = footage_project(&dir, json!({"id": "a", "kind": "video", "src": "alpha.webm", "size": [320, 180], "transform": {"position": [160, 90]}}));
        let p = r.render(&c, 0.5).get_pixel(160, 90).0;
        assert!((p[0] as i32 - 128).abs() <= 6, "white at 50% over black: {p:?}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_mixes_footage_audio() {
    let dir = std::env::temp_dir().join(format!("lahza-footage-audio-{}", std::process::id()));
    if !rgb_video(&dir) {
        return;
    }
    let mut p = project(json!([{"id": "v", "kind": "video", "src": "rgb.mp4", "trim": 0.5, "speed": 1.5, "transform": {"position": [160, 90]}, "time": {"start": 0.5}}]));
    p.settings.duration = 2.0;
    let c = Compiled::new(&p);
    let clips = c.footage_audio();
    assert_eq!(clips.len(), 1);
    assert_eq!((clips[0].start, clips[0].trim, clips[0].speed), (0.5, 0.5, 1.5));
    let out = dir.join("out.mp4");
    let options = export::ExportOptions { output: out.clone(), quality: export::Quality::Draft, range: None, audio: true, render: RenderOptions { backend: Backend::Cpu, ..Default::default() } };
    let report = export::export(&p, &c, &dir, &options, &export::Progress::default()).unwrap();
    assert_eq!(report.frames, 60);
    let info = crate::footage::probe(&out).unwrap();
    assert!(info.has_audio, "footage sound is in the export");
    std::fs::remove_dir_all(&dir).ok();
}


fn one_pixel(layers: serde_json::Value, bg: &str) -> [u8; 4] {
    let mut p = project(layers);
    p.settings.background = lahza_motion_core::value::Color::parse(bg).unwrap();
    cpu().render(&Compiled::new(&p), 0.0).get_pixel(160, 90).0
}

#[test]
fn blend_modes_follow_the_compositing_formulas() {
    // Backdrop (64, 128, 192) under source (128, 64, 255), both opaque.
    let b = [64.0 / 255.0, 128.0 / 255.0, 192.0 / 255.0];
    let s = [128.0 / 255.0, 64.0 / 255.0, 1.0];
    let modes: &[(&str, fn(f64, f64) -> f64)] = &[
        ("multiply", |b, s| b * s),
        ("screen", |b, s| b + s - b * s),
        ("overlay", |b, s| if b <= 0.5 { 2.0 * b * s } else { 1.0 - 2.0 * (1.0 - b) * (1.0 - s) }),
        ("darken", f64::min),
        ("lighten", f64::max),
        ("difference", |b, s| (b - s).abs()),
        ("exclusion", |b, s| b + s - 2.0 * b * s),
        ("subtract", |b, s| (b - s).max(0.0)),
        ("hard_light", |b, s| if s <= 0.5 { 2.0 * b * s } else { 1.0 - 2.0 * (1.0 - b) * (1.0 - s) }),
        ("color_dodge", |b, s| if s >= 1.0 { 1.0 } else { (b / (1.0 - s)).min(1.0) }),
        ("color_burn", |b, s| if s <= 0.0 { 0.0 } else { 1.0 - ((1.0 - b) / s).min(1.0) }),
    ];
    for (mode, f) in modes {
        let got = one_pixel(json!([
            {"id": "s", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#8040ff", "blend": mode, "transform": {"position": [160, 90]}}
        ]), "#4080c0");
        for i in 0..3 {
            let want = (f(b[i], s[i]) * 255.0).round();
            assert!((got[i] as f64 - want).abs() <= 1.0, "{mode} channel {i}: got {} want {want}", got[i]);
        }
    }
    // Non-separable: luminosity keeps the backdrop's hue and takes the source's luminance.
    let lumi = one_pixel(json!([{"id": "s", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ffffff", "blend": "luminosity", "transform": {"position": [160, 90]}}]), "#ff0000");
    assert_eq!(lumi, [255, 255, 255, 255]);
    let colour = one_pixel(json!([{"id": "s", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ff0000", "blend": "color", "transform": {"position": [160, 90]}}]), "#808080");
    assert!(colour[0] > colour[1] && colour[1] == colour[2], "{colour:?}");
}

fn with_effect(fill: &str, effect: serde_json::Value) -> [u8; 4] {
    one_pixel(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [300, 160], "fill": fill, "transform": {"position": [160, 90]}, "effects": [effect]}]), "#000000")
}

#[test]
fn colour_effects() {
    assert_eq!(with_effect("#404040", json!({"type": "exposure", "stops": 1}))[0], 128);
    assert_eq!(with_effect("#ff8000", json!({"type": "invert"})), [0, 127, 255, 255]);
    let levels = with_effect("#808080", json!({"type": "levels", "in_black": 0.25, "in_white": 0.75}));
    assert!((levels[0] as i32 - 128).abs() <= 2, "{levels:?}");
    let hue = with_effect("#ff0000", json!({"type": "hue_saturation", "hue": 180}));
    assert!(hue[0] < 60 && hue[1] > 100 && hue[2] > 100, "red turns cyan: {hue:?}");
    let grey = with_effect("#ff0000", json!({"type": "hue_saturation", "saturation": -1}));
    assert!(grey[0] == grey[1] && grey[1] == grey[2], "{grey:?}");
    let tint = with_effect("#ffffff", json!({"type": "tint", "black": "#000000", "white": "#00ff00"}));
    assert_eq!(tint, [0, 255, 0, 255]);
    let map = with_effect("#000000", json!({"type": "gradient_map", "stops": [[0, "#ff00ff"], [1, "#ffff00"]]}));
    assert_eq!(map, [255, 0, 255, 255]);
    assert_eq!(with_effect("#123456", json!({"type": "fill", "color": "#ffffff"})), [255, 255, 255, 255]);
    // A .cube LUT that swaps red and blue.
    let dir = std::env::temp_dir().join(format!("lahza-lut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut cube = String::from("LUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                cube.push_str(&format!("{b} {g} {r}\n"));
            }
        }
    }
    std::fs::write(dir.join("swap.cube"), cube).unwrap();
    let mut p = project(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [300, 160], "fill": "#ff8000", "transform": {"position": [160, 90]},
        "effects": [{"type": "lut", "path": "swap.cube"}]}]));
    p.settings.background = lahza_motion_core::value::Color::BLACK;
    let mut r = Renderer::new(Some(&dir), RenderOptions { backend: Backend::Cpu, ..Default::default() });
    let swapped = r.render(&Compiled::new(&p), 0.0).get_pixel(160, 90).0;
    assert_eq!(swapped, [0, 128, 255, 255]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn stylize_effects() {
    let row = |effect: serde_json::Value, t: f64| -> Vec<u8> {
        let p = project(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ffffff", "transform": {"position": [160, 90]}, "effects": [effect]}]));
        let img = cpu().render(&Compiled::new(&p), t);
        (90..130).map(|x| img.get_pixel(x, 90).0[0]).collect()
    };
    // Directional blur smears the left edge (x = 110) horizontally.
    let blur = row(json!({"type": "directional_blur", "angle": 0, "length": 30}), 0.0);
    assert!(blur.iter().filter(|v| **v > 10 && **v < 245).count() > 10, "{blur:?}");
    // RGB split shows coloured fringes at the edge.
    let p = project(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ffffff", "transform": {"position": [160, 90]}, "effects": [{"type": "rgb_split", "amount": 6}]}]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    assert!((100..120).any(|x| { let c = img.get_pixel(x, 90).0; c[0] != c[2] }), "fringe");
    // Displacement moves the straight edge.
    let d = row(json!({"type": "displace", "amount": 8, "scale": 20}), 0.0);
    let straight = row(json!({"type": "exposure", "stops": 0}), 0.0);
    assert_ne!(d, straight);
    // Grain: deterministic, different on the next frame.
    let g0 = row(json!({"type": "grain", "amount": 0.3}), 0.0);
    assert_eq!(g0, row(json!({"type": "grain", "amount": 0.3}), 0.0));
    assert_ne!(g0, row(json!({"type": "grain", "amount": 0.3}), 1.0 / 30.0));
    // Vignette darkens corners, not the centre.
    let p = project(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [320, 180], "fill": "#ffffff", "transform": {"position": [160, 90]}, "effects": [{"type": "vignette", "amount": 0.8}]}]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    assert_eq!(img.get_pixel(160, 90).0[0], 255);
    assert!(img.get_pixel(2, 2).0[0] < 120);
}

#[test]
fn primitives_as_paths_match_the_shapes() {
    for spec in [json!({"shape": "circle", "size": 120}), json!({"shape": "rect", "size": [160, 100], "radius": 20}), json!({"shape": "star", "size": 140, "sides": 5, "inner": 0.45}), json!({"shape": "capsule", "size": [180, 60]})] {
        let mut native = spec.clone();
        native["id"] = json!("a");
        native["kind"] = json!("shape");
        native["fill"] = json!("#ffffff");
        native["transform"] = json!({"position": [160, 90]});
        let path = json!({"id": "a", "kind": "shape", "shape": "path", "d": spec.clone(), "fill": "#ffffff", "transform": {"position": [160, 90]}});
        let a = cpu().render(&Compiled::new(&project(json!([native]))), 0.0);
        let b = cpu().render(&Compiled::new(&project(json!([path]))), 0.0);
        let differ = a.pixels().zip(b.pixels()).filter(|(p, q)| p.0[0].abs_diff(q.0[0]) > 96).count();
        assert!(differ < 60, "{spec}: {differ} pixels differ");
    }
}

#[test]
fn path_morph_follows_eases_and_anchors() {
    let mut p = project(json!([{"id": "m", "kind": "shape", "shape": "path", "fill": "#ffffff", "transform": {"position": [160, 90]},
        "d": {"keys": [
            {"at": 0, "v": {"shape": "circle", "size": 100}},
            {"at": "beat:2", "v": {"shape": "star", "size": 160, "sides": 5, "inner": 0.4}, "ease": "gesture", "dur": "12f", "anchor": "peak"}
        ]}}]));
    p.markers.insert(MarkerTrack::from_times("beat", [0.5, 1.0, 1.5]));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let mut r = cpu();
    let lit = |img: &image::RgbaImage| img.pixels().filter(|p| p.0[0] > 128).count();
    let circle = lit(&r.render(&c, 0.0));
    let star = lit(&r.render(&c, 1.9));
    let mid = lit(&r.render(&c, 1.0));
    assert!((circle as f64 - std::f64::consts::PI * 2500.0).abs() < 200.0, "{circle}");
    assert!(mid != circle && mid != star, "{circle} {mid} {star}");
    // The star's tip (top) is far above the circle's edge.
    assert_eq!(r.render(&c, 1.9).get_pixel(160, 90 - 70).0[0], 255);
    assert_eq!(r.render(&c, 0.0).get_pixel(160, 90 - 70).0[0], 0);
    // check sees the morph's anchor and measures it on the frame grid.
    let report = lahza_motion_core::check::check(&p, &c, &Default::default());
    let sync = report.syncs.iter().find(|s| s.element == "m" && s.what == "d").expect("morph sync point");
    assert!(sync.drift_frames.abs() <= 0.5, "{sync:?}");
}

fn lit_span(img: &image::RgbaImage, y: u32, channel: usize) -> (u32, u32) {
    let xs: Vec<u32> = (0..img.width()).filter(|x| img.get_pixel(*x, y).0[channel] > 128).collect();
    (xs.first().copied().unwrap_or(0), xs.last().copied().unwrap_or(0))
}

#[test]
fn three_d_layers_perspective_and_depth() {
    // Turned 60° about y: about half as wide.
    let turned = cpu().render(&Compiled::new(&project(json!([{"id": "a", "kind": "shape", "shape": "rect", "size": [160, 100], "fill": "#ffffff", "three_d": true,
        "transform": {"position": [160, 90], "rotation_y": 60}}]))), 0.0);
    let (x0, x1) = lit_span(&turned, 90, 0);
    assert!(((x1 - x0) as i32 - 80).abs() <= 6, "width {}", x1 - x0);
    // Tilted back about x: a trapezoid, narrower at the top (farther away).
    let tilted = cpu().render(&Compiled::new(&project(json!([{"id": "a", "kind": "shape", "shape": "rect", "size": [200, 120], "fill": "#ffffff", "three_d": true,
        "transform": {"position": [160, 90], "rotation_x": 50}}]))), 0.0);
    let ys: Vec<u32> = (0..180).filter(|y| tilted.get_pixel(160, *y).0[0] > 128).collect();
    let (top, bottom) = (lit_span(&tilted, ys[2], 0), lit_span(&tilted, ys[ys.len() - 3], 0));
    assert!(top.1 - top.0 + 4 < bottom.1 - bottom.0, "top {top:?} bottom {bottom:?}");
    // Depth beats list order: the nearer red plane covers the farther blue one.
    let sorted = cpu().render(&Compiled::new(&project(json!([
        {"id": "near", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ff0000", "three_d": true, "transform": {"position": [160, 90], "z": -100}},
        {"id": "far", "kind": "shape", "shape": "rect", "size": [200, 200], "fill": "#0000ff", "three_d": true, "transform": {"position": [160, 90], "z": 300}}
    ]))), 0.0);
    assert_eq!(px(&sorted, 160, 90), [255, 0, 0, 255]);
    // Camera orbit moves 3D layers; 2D layers stay put as overlays.
    let mut p = project(json!([
        {"id": "card", "kind": "shape", "shape": "rect", "size": [60, 60], "fill": "#00ff00", "three_d": true, "transform": {"position": [160, 90], "z": 200}},
        {"id": "hud", "kind": "shape", "shape": "rect", "size": [20, 20], "fill": "#ffffff", "transform": {"position": [20, 20]}}
    ]));
    p.scenes[0].camera.orbit = lahza_motion_core::property::Property::Constant(lahza_motion_core::value::Vec2::new(25.0, 0.0));
    let orbit = cpu().render(&Compiled::new(&p), 0.0);
    let (g0, g1) = lit_span(&orbit, 90, 1);
    assert!((g0 + g1) / 2 != 160, "the 3D card moved: {g0}..{g1}");
    assert_eq!(px(&orbit, 20, 20), [255, 255, 255, 255], "the 2D overlay did not move");
    // Dollied past the plane: it is behind the camera and not drawn.
    let mut p = project(json!([{"id": "a", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": "#ffffff", "three_d": true, "transform": {"position": [160, 90]}}]));
    p.scenes[0].camera.dolly = lahza_motion_core::property::Property::Constant(lahza_motion_core::space::focal(180.0, 39.6) + 50.0);
    let behind = cpu().render(&Compiled::new(&p), 0.0);
    assert!(behind.pixels().all(|q| q.0[0] < 10), "nothing behind the camera is drawn");
}

#[test]
fn instancer_halftone_ripple() {
    // 16 × 9 dots 20 px apart (centres at 10 + 20k), radius 3, growing to
    // radius 9 at the crest of a ring that leaves the centre at 0.5 s.
    let p = project(json!([{"id": "dots", "kind": "instancer", "transform": {"position": [160, 90]},
        "layout": {"type": "grid", "columns": 16, "rows": 9, "spacing": 20},
        "fields": [{"type": "wave", "center": [0, 0], "at": 0.5, "speed": 200, "wavelength": 60, "scale": 2}],
        "palette": ["#ffffff", "#ff0000"],
        "layers": [{"id": "dot", "kind": "shape", "shape": "circle", "size": 6, "fill": "#000000"}]}]));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let mut r = cpu();
    // Before the wave every dot is small: 6 px right of a centre is dark.
    let before = r.render(&c, 0.4);
    // Copy 69 (row 4, column 5) takes the palette's second colour.
    assert_eq!(px(&before, 110, 90), [255, 0, 0, 255], "a dot centre in its palette colour");
    assert_eq!(px(&before, 116, 90), [0, 0, 0, 255]);
    assert_eq!(px(&before, 130, 90), [255, 255, 255, 255], "the next dot takes the next palette colour");
    // At 0.9 s the crest is 50 px out: the dot there is three times larger,
    // while a far corner dot is not reached yet.
    let crest = r.render(&c, 0.9);
    assert_eq!(px(&crest, 116, 90), [255, 0, 0, 255]);
    assert_eq!(px(&crest, 16, 10), [0, 0, 0, 255]);
    assert_eq!(px(&crest, 10, 10), [255, 255, 255, 255]);
}

/// 2,000 copies (a 50 × 40 dot grid with a wave, noise twinkle, staggered
/// entrances and a palette) at 1080p: evaluating and building a frame stays
/// well inside a 60 fps frame. Prints the full frame time on the GPU when
/// one is available (`--nocapture`).
#[test]
fn instancer_2000_copies_at_1080p() {
    let p = {
        let mut p = Project::new(1920, 1080, Fps::whole(60), 6.0);
        p.scenes.push(serde_json::from_value(json!({"id": "s", "layers": [{"id": "field", "kind": "instancer", "transform": {"position": [960, 540]},
            "layout": {"type": "grid", "columns": 50, "rows": 40, "spacing": [38, 26], "hex": true},
            "stagger": {"total": 1.0, "order": "center"},
            "random": {"seed": 4, "scale": [0.7, 1.0]},
            "palette": ["#ffffff", "#ff3d7f", "#d7ff3a"], "palette_order": "random",
            "fields": [
                {"type": "wave", "at": 1.0, "speed": 900, "wavelength": 260, "scale": 1.4, "ease": "gesture"},
                {"type": "noise", "size": 180, "speed": 0.8, "scale": 0.25, "opacity": -0.3}
            ],
            "layers": [{"id": "dot", "kind": "shape", "shape": "circle", "size": 14, "fill": "#ffffff",
                "transform": {"scale": {"keys": [{"at": 0, "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]}}}]}]}))
        .unwrap());
        p
    };
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(c.frame(2.0).layers.len(), 2000);
    let frames = 30;
    let times = |n: usize| 1.5 + n as f64 / 60.0;
    let mut eval = 0.0;
    let mut build = 0.0;
    let (mut fonts, mut images, mut footage) = (font::Fonts::new(None), images::Images::new(None), footage::Footage::new(None));
    for n in 0..frames {
        let started = Instant::now();
        let f = c.frame(times(n));
        let evaluated = Instant::now();
        let list = Builder { fonts: &mut fonts, images: &mut images, footage: &mut footage, meshes: &mut Default::default(), scale: 1.0 }.build(&f);
        build += evaluated.elapsed().as_secs_f64() * 1000.0;
        eval += (evaluated - started).as_secs_f64() * 1000.0;
        assert_eq!(list.prims.len(), 2000);
    }
    let (eval, build) = (eval / frames as f64, build / frames as f64);
    eprintln!("2000 copies at 1080p: evaluate {eval:.2} ms, display list {build:.2} ms per frame");
    // Generous for slow CI machines; locally both are a small fraction of this.
    assert!(eval + build < 16.0, "evaluate {eval:.2} ms + build {build:.2} ms");

    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let mut gpu = Renderer::new(None, RenderOptions { backend: Backend::Gpu, motion_blur: false, allow_software_gpu: allow, ..Default::default() });
    if !gpu.is_gpu() {
        eprintln!("no GPU for the frame time: {}", gpu.backend_label());
        return;
    }
    for r in [&mut gpu, &mut cpu()] {
        r.render(&c, 1.0);
        r.stats = Stats::default();
        let started = Instant::now();
        for n in 0..frames {
            r.render(&c, times(n));
        }
        let per = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
        eprintln!("{}: {per:.2} ms per frame ({:.0} fps), raster+readback {:.2} ms", r.backend_label(), 1000.0 / per, r.stats.raster_ms / frames as f64);
    }
    // The GPU draws the same picture as the CPU.
    let (a, b) = (gpu.render(&c, 2.2), cpu().render(&c, 2.2));
    let max = a.pixels().zip(b.pixels()).flat_map(|(x, y)| (0..4).map(move |k| x.0[k].abs_diff(y.0[k]))).max().unwrap_or(0);
    assert!(max <= 3, "GPU and CPU differ by {max}");
}

/// One text layer at the canvas centre, with `extra` fields merged in.
fn text_layer(text: &str, size: f64, color: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut layer = json!({"id": "t", "kind": "text", "text": text, "size": size, "color": color, "transform": {"position": [160, 90]}});
    for (k, v) in extra.as_object().unwrap() {
        layer[k] = v.clone();
    }
    layer
}

fn render_text(layer: serde_json::Value, t: f64) -> image::RgbaImage {
    let c = Compiled::new(&project(json!([layer])));
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    cpu().render(&c, t)
}

#[test]
fn text_stroke_and_weight_move_the_outline() {
    // "I" is a plain stem: its row 90 span measures the outline exactly.
    let i = |extra: serde_json::Value| render_text(text_layer("I", 120.0, "#ffffff", extra), 0.0);
    let plain = i(json!({}));
    let (x0, x1) = lit_span(&plain, 90, 1);
    assert!(x1 - x0 > 8, "{x0}..{x1}");
    let near = |a: u32, b: i64| (a as i64 - b).abs() <= 1;
    // Outside (default): the fill is untouched, the outline adds 6 px a side.
    let out = i(json!({"stroke": {"color": "#ff0000", "width": 6}}));
    let (g0, g1) = lit_span(&out, 90, 1);
    let (r0, r1) = lit_span(&out, 90, 0);
    assert!(near(g0, x0 as i64) && near(g1, x1 as i64), "fill {g0}..{g1} vs {x0}..{x1}");
    assert!(near(r0, x0 as i64 - 6) && near(r1, x1 as i64 + 6), "outline {r0}..{r1} vs {x0}..{x1}");
    assert_eq!(px(&out, x0 - 3, 90), [255, 0, 0, 255]);
    assert_eq!(px(&out, (x0 + x1) / 2, 90), [255, 255, 255, 255]);
    // Centred: half outside, half over the fill.
    let mid = i(json!({"stroke": {"color": "#ff0000", "width": 6, "align": "center"}}));
    let (r0, r1) = lit_span(&mid, 90, 0);
    let (g0, g1) = lit_span(&mid, 90, 1);
    assert!(near(r0, x0 as i64 - 3) && near(r1, x1 as i64 + 3), "{r0}..{r1}");
    assert!(near(g0, x0 as i64 + 3) && near(g1, x1 as i64 - 3), "{g0}..{g1}");
    // Inside: the silhouette keeps its size.
    let inside = i(json!({"stroke": {"color": "#ff0000", "width": 4, "align": "inside"}}));
    let (r0, r1) = lit_span(&inside, 90, 0);
    assert!(near(r0, x0 as i64) && near(r1, x1 as i64), "{r0}..{r1}");
    assert_eq!(px(&inside, x0 + 2, 90), [255, 0, 0, 255]);
    // Weight moves the edge itself; negative thins.
    let bold = i(json!({"weight": 3}));
    let (b0, b1) = lit_span(&bold, 90, 1);
    assert!(near(b0, x0 as i64 - 3) && near(b1, x1 as i64 + 3), "bold {b0}..{b1} vs {x0}..{x1}");
    let thin = i(json!({"weight": -2}));
    let (t0, t1) = lit_span(&thin, 90, 1);
    assert!(near(t0, x0 as i64 + 2) && near(t1, x1 as i64 - 2), "thin {t0}..{t1} vs {x0}..{x1}");
    // A stroke follows the weighted edge.
    let both = i(json!({"weight": 3, "stroke": {"color": "#ff0000", "width": 4}}));
    let (r0, _) = lit_span(&both, 90, 0);
    assert!(near(r0, x0 as i64 - 7), "{r0} vs {x0}");
    // Animatable: the outline grows over time.
    let grow = Compiled::new(&project(json!([text_layer("I", 120.0, "#ffffff",
        json!({"stroke": {"color": "#ff0000", "width": {"keys": [{"at": 0, "v": 0}, {"at": 1, "v": 8}]}}}))])));
    let mut r = cpu();
    let w0 = lit_span(&r.render(&grow, 0.0), 90, 0);
    let w1 = lit_span(&r.render(&grow, 1.0), 90, 0);
    assert!(near(w0.0, x0 as i64) && near(w1.0, x0 as i64 - 8), "{w0:?} {w1:?}");
}

#[test]
fn edge_light_lights_the_side_facing_the_light() {
    let dark = "#202020";
    let lit = |angle: f64| render_text(text_layer("I", 120.0, dark, json!({"edge": {"color": "#ffffff", "width": 6, "angle": angle}})), 0.0);
    let plain = render_text(text_layer("I", 120.0, dark, json!({})), 0.0);
    let ink = |img: &image::RgbaImage, x: u32, y: u32| px(img, x, y)[0] >= 30;
    let col: Vec<u32> = (0..180).filter(|y| ink(&plain, 160, *y)).collect();
    let (top, bottom) = (*col.first().unwrap(), *col.last().unwrap());
    let left = (0..320).find(|x| ink(&plain, *x, 90)).unwrap();
    let from_above = lit(0.0);
    assert!(px(&from_above, 160, top + 1)[0] > 150, "{:?}", px(&from_above, 160, top + 1));
    assert!(px(&from_above, 160, bottom - 1)[0] < 50, "{:?}", px(&from_above, 160, bottom - 1));
    // The middle of the stem and its sides (parallel to the light) stay dark.
    assert!(px(&from_above, 160, 90)[0] < 40);
    assert!(px(&from_above, left + 1, 90)[0] < 50, "{:?}", px(&from_above, left + 1, 90));
    // Light from below lights the bottom; from the left, the left side.
    let from_below = lit(180.0);
    assert!(px(&from_below, 160, bottom - 1)[0] > 150 && px(&from_below, 160, top + 1)[0] < 50);
    let from_left = lit(-90.0);
    assert!(px(&from_left, left + 1, 90)[0] > 150, "{:?}", px(&from_left, left + 1, 90));
    // The silhouette is unchanged: the rim lies inside the edge.
    let width = |img: &image::RgbaImage| (0..320).filter(|x| px(img, *x, 90)[0] > 0).count();
    assert_eq!(width(&from_left), width(&plain));
}

/// Per-char (opacity, y) of a text layer at `t`, with the renderer's
/// position-ordered animators applied.
fn swept(text: &str, animator: serde_json::Value, t: f64) -> Vec<(f64, f64)> {
    let c = Compiled::new(&project(json!([text_layer(text, 40.0, "#ffffff", json!({"animators": [animator]}))])));
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let f = c.frame(t);
    let lahza_motion_core::compile::Content::Text(tx) = &f.layers[0].content else { panic!() };
    let mut fonts = font::Fonts::new(None);
    let layout = fonts.layout(tx);
    let (glyphs, _) = font::sweep(&layout, tx);
    glyphs.iter().map(|g| (g.opacity, g.y)).collect()
}

#[test]
fn spatial_order_delays_units_by_position() {
    // Linear 4-frame fades, 2 f stagger: 4 glyphs span 6 f.
    let anim = |order: serde_json::Value| json!({"at": 0, "order": order, "stagger": "2f", "dur": "4f", "ease": "linear", "from": {"opacity": 0}});
    let f = 1.0 / 30.0;
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    // Left to right: the first glyph is 2 of 4 frames in, the last hasn't
    // started. Equal advances put the glyphs at 0, 1/3, 2/3, 1: the last
    // starts at exactly 6 f, as in index order.
    let x = swept("HHHH", anim(json!({"axis": "x"})), 2.0 * f);
    assert!(near(x[0].0, 0.5) && x[1].0 == 0.0 && x[3].0 == 0.0, "{x:?}");
    let mid = swept("HHHH", anim(json!({"axis": "x"})), 4.0 * f);
    assert!(near(mid[0].0, 1.0) && near(mid[1].0, 0.5) && near(mid[2].0, 0.0), "{mid:?}");
    let late = swept("HHHH", anim(json!({"axis": "x"})), 8.0 * f);
    assert!(near(late[3].0, 0.5) && late[0].0 == 1.0, "{late:?}");
    // From the end: mirrored; an angle of 180° sweeps right to left too.
    for order in [json!({"axis": "x", "from": "end"}), json!({"axis": 180})] {
        let end = swept("HHHH", anim(order), 2.0 * f);
        assert!(near(end[3].0, 0.5) && end[0].0 == 0.0, "{end:?}");
    }
    // From the centre outward: the middle pair together, the ends last.
    let centre = swept("HHHH", anim(json!({"axis": "x", "from": "center"})), 2.0 * f);
    assert!(near(centre[1].0, 0.5) && near(centre[2].0, 0.5) && centre[0].0 == 0.0 && centre[3].0 == 0.0, "{centre:?}");
    // Vertical: lines top to bottom.
    let lines = swept("AB\nC", json!({"at": 0, "unit": "line", "order": {"axis": "y"}, "stagger": "4f", "dur": "4f", "ease": "linear", "from": {"opacity": 0}}), 2.0 * f);
    assert!(near(lines[0].0, 0.5) && near(lines[1].0, 0.5) && lines[3].0 == 0.0, "{lines:?}");
    // Radial from the middle: the centre glyph first, then symmetric pairs.
    let radial = swept("HHHHH", anim(json!({"radial": [0.5, 0.5]})), 5.0 * f);
    // Span 8 f: the neighbours sit at 0.5 (start at 4 f), the ends at 1.
    assert!(near(radial[2].0, 1.0), "{radial:?}");
    assert!(near(radial[1].0, radial[3].0) && near(radial[0].0, radial[4].0), "{radial:?}");
    // (Close to 0.25: the centre is the block's, a little off the cap middle.)
    assert!((radial[1].0 - 0.25).abs() < 0.01 && radial[0].0 == 0.0, "{radial:?}");
    // Other channels follow the same delays.
    let y = swept("HHHH", json!({"at": 0, "order": {"axis": "x"}, "stagger": "2f", "dur": "4f", "ease": "linear", "from": {"y": 40}}), 2.0 * f);
    assert!(near(y[0].1, 20.0) && y[3].1 == 40.0, "{y:?}");
}

#[test]
fn feathered_reveal_is_a_soft_wipe_across_glyphs() {
    let layer = text_layer("HHHH", 60.0, "#ffffff", json!({"animators": [
        {"at": 0, "order": {"axis": "x", "feather": 0.3}, "stagger": "4f", "dur": "6f", "ease": "linear", "from": {"opacity": 0}}]}));
    let c = Compiled::new(&project(json!([layer])));
    let mut r = cpu();
    let column = |img: &image::RgbaImage, x: u32| (0..180).map(|y| px(img, x, y)[0]).max().unwrap();
    let full = r.render(&c, 1.5);
    let (x0, x1) = lit_span(&full, 90, 0);
    assert!(r.render(&c, 0.0).pixels().all(|p| p.0[0] == 0), "nothing shows at the start");
    // Halfway (span 12 f + dur 6 f = 18 f): left lit, right dark, and a
    // ramp between that runs through glyphs (not whole-glyph steps).
    let half = r.render(&c, 9.0 / 30.0);
    let cols: Vec<u8> = (x0..=x1).map(|x| column(&half, x)).collect();
    let ink: Vec<u8> = (x0..=x1).map(|x| column(&full, x)).collect();
    assert!(cols[1] > 240 && *cols.last().unwrap() < 10, "{cols:?}");
    let partial = cols.iter().zip(&ink).filter(|(c, i)| **i > 250 && **c > 20 && **c < 235).count();
    assert!(partial > 10, "{partial}: {cols:?}");
    // Monotone where there is ink: the wipe never brightens to the right.
    let mut last = 255u8;
    for (c, i) in cols.iter().zip(&ink) {
        if *i > 250 {
            assert!(*c <= last.saturating_add(2), "{cols:?}");
            last = *c;
        }
    }
    // A radial wipe from the middle: centre lit before the ends.
    let radial = Compiled::new(&project(json!([text_layer("HHHH", 60.0, "#ffffff", json!({"animators": [
        {"at": 0, "order": {"radial": [0.5, 0.5], "feather": 0.3}, "stagger": "4f", "dur": "6f", "ease": "linear", "from": {"opacity": 0}}]}))])));
    let img = r.render(&radial, 9.0 / 30.0);
    // The inked column nearest the middle (the middle itself is a gap).
    let m = (x0..=x1).filter(|x| ink[(x - x0) as usize] > 250).min_by_key(|x| (*x as i64 - (x0 + x1) as i64 / 2).abs()).unwrap();
    let cols = [column(&img, m), column(&img, x0 + 1), column(&img, x1 - 1)];
    assert!(cols[0] > 200 && cols[1] < 30 && cols[2] < 30, "{cols:?}");
}

#[test]
fn box_gradients_follow_an_animated_size() {
    let p = project(json!([{"id": "g", "kind": "shape", "shape": "rect", "transform": {"position": [160, 90]},
        "size": {"keys": [{"at": 0, "v": [100, 40]}, {"at": 1, "v": [240, 40]}]},
        "fill": {"gradient": "linear", "units": "box", "from": [0, 0.5], "to": [1, 0.5], "stops": [[0, "#000000"], [1, "#ffffff"]]}}]));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let mut r = cpu();
    for (t, hw) in [(0.0, 50u32), (1.0, 120u32)] {
        let img = r.render(&c, t);
        let (left, mid, right) = (px(&img, 160 - hw + 2, 90)[0], px(&img, 160, 90)[0], px(&img, 160 + hw - 2, 90)[0]);
        assert!(left < 12 && right > 243 && (mid as i32 - 128).abs() < 6, "t={t}: {left} {mid} {right}");
    }
    // A radial gradient in box units is an ellipse filling the box.
    let p = project(json!([{"id": "e", "kind": "shape", "shape": "rect", "size": [200, 60], "transform": {"position": [160, 90]},
        "fill": {"gradient": "radial", "units": "box", "from": [0.5, 0.5], "to": [1, 0.5], "stops": [[0, "#ffffff"], [1, "#000000"]]}}]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    let (x, y) = (px(&img, 160 + 70, 90)[0], px(&img, 160, 90 + 21)[0]);
    assert!((x as i32 - y as i32).abs() <= 4 && (x as i32 - 77).abs() < 8, "70% out along x {x}, along y {y}");
}

#[test]
fn stroke_gradient_runs_along_the_path() {
    let stroke = |extra: serde_json::Value| {
        let mut s = json!({"width": 20, "gradient": {"stops": [[0, "#ff0000"], [1, "#0000ff"]], "along": "length"}});
        s.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        s
    };
    let render = |d: &str, s: serde_json::Value, trim: serde_json::Value| {
        let p = project(json!([{"id": "p", "kind": "shape", "shape": "path", "d": d, "stroke": s, "trim": trim}]));
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        cpu().render(&c, 0.0)
    };
    // A U turn: a linear gradient could not make both arms' starts differ.
    let u = "M 40 40 H 280 V 140 H 40";
    let img = render(u, stroke(json!({})), json!([0, 1]));
    let (a, b) = (px(&img, 50, 40), px(&img, 50, 140));
    assert!(a[0] > 230 && a[2] < 25 && b[2] > 230 && b[0] < 25, "start {a:?} end {b:?}");
    let corner = px(&img, 280, 90);
    assert!((corner[0] as i32 - corner[2] as i32).abs() < 12, "halfway is half and half: {corner:?}");
    // Trim reveals the colours without sliding them.
    let trimmed = render(u, stroke(json!({})), json!([0, 0.6]));
    assert_eq!(px(&trimmed, 200, 40), px(&img, 200, 40));
    assert_eq!(px(&trimmed, 280, 90), corner);
    assert_eq!(px(&trimmed, 100, 140), [0, 0, 0, 255], "background past the trim");
    // Dashes and butt caps keep the same colours; feathers fade the head.
    let dashed = render(u, stroke(json!({"cap": "butt", "dash": [30, 10]})), json!([0, 1]));
    assert_eq!(px(&dashed, 55, 40), px(&img, 55, 40));
    assert_eq!(px(&dashed, 75, 40), [0, 0, 0, 255], "a gap");
    let feathered = render(u, stroke(json!({"feather": 120})), json!([0, 0.6]));
    let (body, head) = (px(&feathered, 150, 40), px(&feathered, 280, 130));
    assert_eq!(body, px(&img, 150, 40), "before the feather");
    assert!((head[0] as u32 + head[2] as u32) < 80, "18 px from the end of a 120 px feather: {head:?}");
    // No seams around a closed curve: neighbouring samples change gently.
    let ring = render("M 160 30 A 60 60 0 1 1 159.99 30", json!({"width": 12, "gradient": {"stops": [[0, "#ff0000"], [0.5, "#00ff00"], [1, "#0000ff"]]}}), json!([0, 1]));
    let mut prev: Option<[u8; 4]> = None;
    for i in 10..350 {
        let a = (i as f64).to_radians();
        let q = px(&ring, (160.0 + 60.0 * a.sin()).round() as u32, (90.0 - 60.0 * a.cos()).round() as u32);
        if let Some(p) = prev {
            let d = (0..3).map(|k| p[k].abs_diff(q[k])).max().unwrap();
            assert!(d <= 12, "jump at {i}°: {p:?} → {q:?}");
        }
        prev = Some(q);
    }
}

/// Pixels between the dark background and a white edge along row `y`.
fn soft_edge(img: &image::RgbaImage, y: u32, from: u32, to: u32) -> usize {
    (from..to).filter(|x| (20..235).contains(&px(img, *x, y)[0])).count()
}

#[test]
fn depth_of_field_blurs_by_distance_from_focus() {
    let layers = json!([
        {"id": "near", "kind": "shape", "shape": "rect", "size": [60, 60], "fill": "#ffffff", "three_d": true, "transform": {"position": [80, 90]}},
        {"id": "far", "kind": "shape", "shape": "rect", "size": [60, 60], "fill": "#ffffff", "three_d": true, "transform": {"position": [240, 90], "z": 400}},
        {"id": "hud", "kind": "shape", "shape": "rect", "size": [20, 20], "fill": "#ffffff", "transform": {"position": [160, 20]}}
    ]);
    let render = |focus: serde_json::Value, t: f64| {
        let mut p = project(layers.clone());
        p.scenes[0].camera.aperture = lahza_motion_core::property::Property::Constant(2.0);
        p.scenes[0].camera.focus = Some(serde_json::from_value(focus).unwrap());
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        cpu().render(&c, t)
    };
    // Focused on the near plane: it stays crisp, the far one blurs.
    let a = render(json!(0), 0.0);
    let (near, far) = (soft_edge(&a, 90, 30, 80), soft_edge(&a, 90, 180, 240));
    assert!(near <= 2 && far >= 8, "near {near} far {far}");
    // Rack focus to the far plane over a second, or focus on its layer.
    let rack = json!({"keys": [{"at": 0, "v": 0}, {"at": 1, "v": 400}]});
    for b in [render(rack, 1.0), render(json!("far"), 0.0)] {
        let (near, far) = (soft_edge(&b, 90, 30, 80), soft_edge(&b, 90, 180, 240));
        assert!(near >= 8 && far <= 2, "near {near} far {far}");
        // 2D layers are never blurred.
        assert_eq!(soft_edge(&b, 20, 140, 160), 0);
    }
    // An unknown focus layer is an error.
    let mut p = project(layers.clone());
    p.scenes[0].camera.focus = Some(serde_json::from_value(json!("nope")).unwrap());
    assert!(Compiled::new(&p).errors.iter().any(|e| e.contains("camera.focus")));
}

#[test]
fn field_blends_points_smoothly() {
    let layers = |warp: f64| json!([{"id": "f", "kind": "shape", "shape": "field", "size": [320, 180], "transform": {"position": [160, 90]}, "warp": warp,
        "points": [{"position": {"keys": [{"at": 0, "v": [-110, 0]}, {"at": 1, "v": [-60, 0]}]}, "color": "#ff0000", "radius": 50},
                   {"position": [110, 0], "color": "#0000ff", "radius": 50}]}]);
    let c = Compiled::new(&project(layers(0.0)));
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let mut r = cpu();
    let img = r.render(&c, 0.0);
    let (red, blue, mid) = (px(&img, 50, 90), px(&img, 270, 90), px(&img, 160, 90));
    assert!(red[0] > 235 && red[2] < 20 && blue[2] > 235 && blue[0] < 20, "{red:?} {blue:?}");
    assert!((mid[0] as i32 - mid[2] as i32).abs() < 6 && mid[0] > 100, "an even blend between them: {mid:?}");
    // Smooth everywhere, and it fills the whole box (no gaps far away).
    for y in [0, 90, 179] {
        for x in 1..320 {
            let (a, b) = (px(&img, x - 1, y), px(&img, x, y));
            assert!((0..3).all(|k| a[k].abs_diff(b[k]) <= 4) && b[3] == 255, "({x}, {y}): {a:?} {b:?}");
        }
    }
    // The red point moves right with its keys: the blend moves with it.
    let later = r.render(&c, 1.0);
    assert!(px(&later, 160, 90)[0] > mid[0] + 30, "{:?}", px(&later, 160, 90));
    // Warp makes the boundary organic.
    let warped = cpu().render(&Compiled::new(&project(layers(30.0))), 0.0);
    assert!((0..180).any(|y| px(&warped, 160, y) != px(&img, 160, y)));
}

/// A 1080p frame with a 6-point warped field and depth of field on five 3D
/// cards (rack focus), against the same cards on a flat background in
/// focus. `cargo test -p lahza-motion-render --release -- --ignored --nocapture benchmark_field`
#[test]
#[ignore]
fn benchmark_field_and_depth_of_field_1080p() {
    let colours = ["#ff3355", "#7b61ff", "#00d4a6", "#ffd23f", "#2d5bff", "#ff8800"];
    let points: Vec<serde_json::Value> = (0..6)
        .map(|i| json!({"position": {"keys": [{"at": 0, "v": [-700 + i * 280, -300 + (i % 3) * 300]}, {"at": 4, "v": [-500 + i * 200, 300 - (i % 3) * 300]}]},
            "color": colours[i as usize], "radius": 300}))
        .collect();
    let build = |field: bool, dof: bool| {
        let mut layers = vec![if field {
            json!({"id": "bg", "kind": "shape", "shape": "field", "size": [1920, 1080], "warp": 40, "transform": {"position": [960, 540]}, "points": points})
        } else {
            json!({"id": "bg", "kind": "shape", "shape": "rect", "size": [1920, 1080], "fill": "#203040", "transform": {"position": [960, 540]}})
        }];
        for i in 0..5 {
            layers.push(json!({"id": format!("card{i}"), "kind": "shape", "shape": "rect", "size": [360, 220], "radius": 24, "fill": "#ffffff", "three_d": true,
                "transform": {"position": [300 + i * 330, 540], "z": i * 250 - 250, "rotation_y": {"keys": [{"at": 0, "v": 10}, {"at": 4, "v": 30}]}}}));
        }
        let mut p = Project::new(1920, 1080, Fps::whole(60), 4.0);
        let mut scene: Scene = serde_json::from_value(json!({"id": "s", "layers": layers})).unwrap();
        if dof {
            scene.camera.aperture = lahza_motion_core::property::Property::Constant(3.0);
            scene.camera.focus = Some(serde_json::from_value(json!({"keys": [{"at": 0, "v": -250}, {"at": 4, "v": 750}]})).unwrap());
        }
        p.scenes.push(scene);
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        c
    };
    for backend in [Backend::Gpu, Backend::Cpu] {
        for (label, field, dof) in [("flat, in focus", false, false), ("field", true, false), ("DOF", false, true), ("field + DOF", true, true)] {
            let c = build(field, dof);
            let mut r = Renderer::new(None, RenderOptions { backend, motion_blur: false, ..Default::default() });
            if backend == Backend::Gpu && !r.is_gpu() {
                eprintln!("no GPU: {}", r.backend_label());
                break;
            }
            r.render(&c, 0.0);
            let frames = if backend == Backend::Gpu { 120 } else { 6 };
            let started = Instant::now();
            for n in 0..frames {
                r.render(&c, 0.5 + n as f64 / 60.0);
            }
            let per = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            eprintln!(
                "{} {label}: {per:.2} ms/frame; build {:.2} ms, raster+readback {:.2} ms",
                r.backend_label(),
                r.stats.build_ms / r.stats.frames as f64,
                r.stats.raster_ms / r.stats.frames as f64
            );
        }
    }
}


#[test]
fn whip_smears_the_edge_between_scenes() {
    let mut p = Project::new(320, 180, Fps::whole(30), 4.0);
    p.scenes.push(serde_json::from_value(json!({"id": "a", "background": "#ff0000"})).unwrap());
    p.scenes.push(serde_json::from_value(json!({"id": "b", "start": 2, "background": "#0000ff",
        "transition": {"type": "whip", "dur": 1, "ease": "linear", "direction": "up"}})).unwrap());
    let img = cpu().render(&Compiled::new(&p), 2.0);
    // Halfway: the scenes meet at y = 90. Each row may only change a little
    // from the next (a smear), not jump from red to blue (a seam).
    let worst = (60..120).map(|y| (px(&img, 160, y)[0] as i32 - px(&img, 160, y + 1)[0] as i32).abs()).max().unwrap();
    assert!(worst < 40, "largest red step between rows near the seam: {worst}");
    assert!(px(&img, 160, 40)[0] > 200 && px(&img, 160, 140)[2] > 200, "far from the seam: the scenes' own colours");
}

#[test]
fn zoom_never_shows_a_box_or_border() {
    let mut p = Project::new(320, 180, Fps::whole(30), 4.0);
    p.scenes.push(serde_json::from_value(json!({"id": "a", "background": "#ff0000"})).unwrap());
    p.scenes.push(serde_json::from_value(json!({"id": "b", "start": 2, "background": "#0000ff",
        "transition": {"type": "zoom", "dur": 1, "ease": "linear"}})).unwrap());
    let c = Compiled::new(&p);
    for t in [1.6, 2.0, 2.4] {
        let img = cpu().render(&c, t);
        // Both backgrounds fill the frame: corners match the centre.
        let mid = px(&img, 160, 90);
        for (x, y) in [(0, 0), (319, 0), (0, 179), (319, 179), (40, 90)] {
            let c = px(&img, x, y);
            assert!((0..3).all(|i| (c[i] as i32 - mid[i] as i32).abs() <= 2), "t={t}: ({x},{y}) {c:?} vs centre {mid:?}");
        }
    }
}

#[test]
fn faded_groups_fade_as_one_picture() {
    // Two overlapping white squares in a 50 % group: where they overlap
    // the group is still 50 % (one picture), not 75 % (two layers).
    let p = project(json!([
        {"id": "g", "kind": "comp", "transform": {"opacity": 0.5}, "layers": [
            {"id": "a", "kind": "shape", "shape": "rect", "size": [80, 80], "fill": "#ffffff", "transform": {"position": [120, 90]}},
            {"id": "b", "kind": "shape", "shape": "rect", "size": [80, 80], "fill": "#ffffff", "transform": {"position": [160, 90]}}]},
        // Apart: each square takes the group's opacity (drawn directly).
        {"id": "h", "kind": "comp", "transform": {"opacity": 0.5}, "layers": [
            {"id": "c", "kind": "shape", "shape": "rect", "size": [20, 20], "fill": "#ffffff", "transform": {"position": [40, 20]}},
            {"id": "d", "kind": "shape", "shape": "rect", "size": [20, 20], "fill": "#ffffff", "transform": {"position": [280, 20]}}]}
    ]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    let grey = |c: [u8; 4]| (c[0] as i32 - 128).abs() <= 2;
    assert!(grey(px(&img, 140, 90)), "overlap: {:?}", px(&img, 140, 90));
    assert!(grey(px(&img, 100, 90)) && grey(px(&img, 180, 90)));
    assert!(grey(px(&img, 40, 20)) && grey(px(&img, 280, 20)), "{:?} {:?}", px(&img, 40, 20), px(&img, 280, 20));
}

#[test]
fn big_paths_split_per_tile_draw_the_same() {
    // A 600-segment circle against the analytic circle: same pixels.
    let n = 600;
    let d: String = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            format!("{} {:.4} {:.4}", if i == 0 { "M" } else { "L" }, 70.0 * a.cos(), 70.0 * a.sin())
        })
        .collect::<Vec<_>>()
        .join(" ")
        + " Z";
    let p = project(json!([
        {"id": "poly", "kind": "shape", "shape": "path", "d": d, "fill": "#ffffff", "stroke": {"color": "#ff0000", "width": 4}, "transform": {"position": [85, 90]}},
        {"id": "disc", "kind": "shape", "shape": "circle", "size": 140, "fill": "#ffffff", "stroke": {"color": "#ff0000", "width": 4}, "transform": {"position": [235, 90]}}
    ]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    let mut worst = 0;
    for y in 0..180 {
        for x in 8..160 {
            let a = px(&img, x, y);
            let b = px(&img, x + 150, y);
            worst = worst.max((0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).max().unwrap());
        }
    }
    if worst > 40 {
        img.save(std::env::temp_dir().join("split.png")).ok();
        let mut where_ = Vec::new();
        for y in 0..180 {
            for x in 8..160 {
                let a = px(&img, x, y);
                let b = px(&img, x + 150, y);
                if (0..3).any(|i| (a[i] as i32 - b[i] as i32).abs() > 40) {
                    where_.push((x, y, a, b));
                }
            }
        }
        panic!("split path differs from the circle at {} px, e.g. {:?}", where_.len(), &where_[..where_.len().min(6)]);
    }
    assert_eq!(px(&img, 85, 90), [255, 255, 255, 255], "inside");
    assert_eq!(px(&img, 85, 10), [0, 0, 0, 255], "outside");
}

#[test]
#[ignore]
fn benchmark_big_path_1080x1920() {
    // A jagged 4000-segment coastline filling most of a vertical frame.
    let n = 4000;
    let d: String = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let r = 480.0 + 60.0 * (a * 37.0).sin() + 25.0 * (a * 211.0).sin();
            format!("{} {:.2} {:.2}", if i == 0 { "M" } else { "L" }, r * a.cos(), 1.8 * r * a.sin())
        })
        .collect::<Vec<_>>()
        .join(" ")
        + " Z";
    let mut p = Project::new(1080, 1920, Fps::whole(30), 1.0);
    p.scenes.push(serde_json::from_value(json!({"id": "s", "layers": [
        {"id": "land", "kind": "shape", "shape": "path", "d": d, "fill": "#d8cfb8", "stroke": {"color": "#222222", "width": 3}, "transform": {"position": [540, 960]}}]})).unwrap());
    let c = Compiled::new(&p);
    let mut r = cpu();
    r.render(&c, 0.0);
    let t = std::time::Instant::now();
    for _ in 0..5 {
        r.last_invalidate();
        r.render(&c, 0.0);
    }
    eprintln!("big path: {:.1} ms/frame", t.elapsed().as_secs_f64() * 200.0);
}

/// Split paths (per-tile copies with a row backdrop) on the GPU.
#[test]
fn gpu_matches_cpu_big_paths() {
    let n = 900;
    let d: String = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let r = 70.0 + 12.0 * (a * 23.0).sin();
            format!("{} {:.3} {:.3}", if i == 0 { "M" } else { "L" }, r * a.cos(), 0.9 * r * a.sin())
        })
        .collect::<Vec<_>>()
        .join(" ")
        + " Z M -20 -10 L 20 -10 L 20 10 L -20 10 Z";
    let p = project(json!([
        {"id": "land", "kind": "shape", "shape": "path", "d": d, "fill": "#d8cfb8", "stroke": {"color": "#222222", "width": 2},
         "transform": {"position": {"keys": [{"at": 0, "v": [160, 90]}, {"at": 1, "v": [150, 95]}]}, "scale": {"keys": [{"at": 0, "v": [1, 1]}, {"at": 1, "v": [1.6, 1.6]}]}}}
    ]));
    assert_gpu_matches_cpu(&p, &[0.0, 0.5, 1.0]);
}

#[test]
fn motion_blurred_cpu_frames_are_not_black() {
    // A group with an effect: its offscreen picture comes before the main
    // target in each sample's list.
    let mut p = project(json!([{"id": "r", "kind": "shape", "shape": "rect", "size": [100, 60], "fill": "#ff0000",
        "transform": {"position": {"keys": [{"at": 0, "v": [150, 90]}, {"at": 1, "v": [170, 90]}]}}},
        {"id": "g", "kind": "comp", "effects": [{"type": "blur", "radius": {"keys": [{"at": 0, "v": 1}, {"at": 1, "v": 6}]}}], "layers": [
            {"id": "d", "kind": "shape", "shape": "circle", "size": 30, "fill": "#00ff00", "transform": {"position": [40, 40]}}]}]));
    p.settings.background = lahza_motion_core::value::Color::parse("#203040").unwrap();
    for samples in [1u32, 4] {
        p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples });
        let img = cpu().render(&Compiled::new(&p), 0.5);
        assert_eq!(px(&img, 160, 90), [255, 0, 0, 255], "samples {samples}");
        assert_eq!(px(&img, 5, 5), [32, 48, 64, 255], "samples {samples}");
    }
}

#[test]
fn dash_offset_animates() {
    let p = project(json!([{"id": "ants", "kind": "shape", "shape": "path", "d": "M 20 90 L 300 90",
        "stroke": {"color": "#ffffff", "width": 8, "cap": "butt", "dash": [20, 20], "dash_offset": {"keys": [{"at": 0, "v": 0}, {"at": 1, "v": 20, "ease": "linear"}]}}}]));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let a = cpu().render(&c, 0.0);
    let b = cpu().render(&c, 1.0);
    // Shifted by one dash: lit and dark swap along the line.
    let lit = |img: &image::RgbaImage, x| px(img, x, 90)[0] > 128;
    assert!((30..270).step_by(10).all(|x| lit(&a, x) != lit(&b, x)), "the dashes moved");
}

#[test]
fn a_gradient_rect_works_as_a_matte() {
    for (mode, stops) in [("alpha", json!([[0, "#ffffffff"], [1, "#ffffff00"]])), ("luma", json!([[0, "#ffffff"], [1, "#000000"]]))] {
        let p = project(json!([
            {"id": "m", "kind": "shape", "shape": "rect", "size": [300, 160],
             "fill": {"gradient": "linear", "from": [-150, 0], "to": [150, 0], "stops": stops}, "transform": {"position": [160, 90]}},
            {"id": "red", "kind": "shape", "shape": "rect", "size": [300, 160], "fill": "#ff0000", "transform": {"position": [160, 90]},
             "matte": {"layer": "m", "mode": mode}}
        ]));
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let img = cpu().render(&c, 0.0);
        let (l, m, r) = (px(&img, 30, 90)[0], px(&img, 160, 90)[0], px(&img, 290, 90)[0]);
        assert!(l > 220 && (100..160).contains(&m) && r < 40, "{mode}: red should fade left to right, got {l} {m} {r}");
    }
}


/// Gradient-filled mattes (alpha, luma, box units, on a comp) on the GPU.
#[test]
fn gpu_matches_cpu_gradient_mattes() {
    let p = project(json!([
        {"id": "m1", "kind": "shape", "shape": "rect", "size": [140, 160], "fill": {"gradient": "linear", "from": [-70, 0], "to": [70, 0], "stops": [[0, "#ffffffff"], [1, "#ffffff00"]]}, "transform": {"position": [80, 90]}},
        {"id": "a", "kind": "shape", "shape": "rect", "size": [140, 160], "fill": "#ff0000", "transform": {"position": [80, 90]}, "matte": {"layer": "m1", "mode": "alpha"}},
        {"id": "m2", "kind": "shape", "shape": "rect", "size": [140, 160], "fill": {"gradient": "radial", "units": "box", "from": [0.5, 0.5], "to": [1, 0.5], "stops": [[0, "#ffffff"], [1, "#000000"]]}, "transform": {"position": [240, 90]}},
        {"id": "g", "kind": "comp", "matte": {"layer": "m2", "mode": "luma"}, "layers": [
            {"id": "b", "kind": "shape", "shape": "rect", "size": [140, 160], "fill": "#00ff00", "transform": {"position": [240, 90]}}]}
    ]));
    assert_gpu_matches_cpu(&p, &[0.0]);
}

#[test]
#[ignore]
fn benchmark_feathered_trimmed_path() {
    // A long wavy route drawn on with trim and a soft head.
    let n = 3000;
    let d: String = (0..n)
        .map(|i| {
            let u = i as f64 / n as f64;
            format!("{} {:.2} {:.2}", if i == 0 { "M" } else { "L" }, -450.0 + 900.0 * u, 600.0 * (u * 9.0).sin() + 40.0 * (u * 97.0).sin())
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut p = Project::new(1080, 1920, Fps::whole(30), 1.0);
    p.scenes.push(serde_json::from_value(json!({"id": "s", "layers": [
        {"id": "route", "kind": "shape", "shape": "path", "d": d, "stroke": {"color": "#ff4040", "width": 8, "feather": 120, "feather_start": 40},
         "trim": [0, 0.85], "transform": {"position": [540, 960]}}]})).unwrap());
    let c = Compiled::new(&p);
    let mut r = cpu();
    r.render(&c, 0.0);
    let t = std::time::Instant::now();
    for _ in 0..5 {
        r.last_invalidate();
        r.render(&c, 0.0);
    }
    eprintln!("feathered path: {:.1} ms/frame", t.elapsed().as_secs_f64() * 200.0);
}

/// Colour along a long feathered line: split per tile (each tile scans
/// only its nearby centreline) against the same line rotated a quarter
/// turn, which is never split.
#[test]
fn along_paint_split_per_tile_draws_the_same() {
    let n = 400;
    let pts: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let u = i as f64 / (n - 1) as f64;
            (-65.0 + 130.0 * u, 55.0 * (u * 11.0).sin())
        })
        .collect();
    let path = |f: &dyn Fn((f64, f64)) -> (f64, f64)| {
        pts.iter().enumerate().map(|(i, p)| { let q = f(*p); format!("{} {:.4} {:.4}", if i == 0 { "M" } else { "L" }, q.0, q.1) }).collect::<Vec<_>>().join(" ")
    };
    let stroke = json!({"width": 9, "feather": 40, "feather_start": 15, "gradient": {"stops": [[0, "#ff2020"], [0.5, "#20ff20"], [1, "#4040ff"]], "along": "length"}});
    let p = project(json!([
        {"id": "split", "kind": "shape", "shape": "path", "d": path(&|p| p), "stroke": stroke, "trim": [0.05, 0.9], "transform": {"position": [85, 90]}},
        {"id": "whole", "kind": "shape", "shape": "path", "d": path(&|(x, y)| (y, -x)), "stroke": stroke, "trim": [0.05, 0.9], "transform": {"position": [235, 90], "rotation": 90}}
    ]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    let mut worst = 0;
    for y in 0..180 {
        for x in 8..160 {
            let (a, b) = (px(&img, x, y), px(&img, x + 150, y));
            worst = worst.max((0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).max().unwrap());
        }
    }
    let lit = (0..180).flat_map(|y| (8..160).map(move |x| (x, y))).filter(|&(x, y)| px(&img, x, y)[..3].iter().any(|v| *v > 40)).count();
    assert!(lit > 800, "the line drew ({lit} px)");
    assert!(worst <= 24, "split along paint differs by {worst}");
    assert_gpu_matches_cpu(&p, &[0.0]);
}

/// Depth of field blurs a 3D layer; blurred, it must still stand in the
/// mesh depth buffer as a card, not be painted over by a floor behind it.
#[test]
fn defocused_cards_keep_their_depth() {
    let build = |aperture: f64| {
        let mut p = project(json!([
            {"id": "word", "kind": "text", "text": "HI", "size": 70, "color": "#ffffff", "three_d": true, "transform": {"position": [160, 80], "z": 300}},
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [3000, 3000]}, "material": {"preset": "matte", "color": "#404040"},
             "transform": {"position": [160, 150], "rotation_x": 90}}
        ]));
        p.scenes[0].camera = serde_json::from_value(json!({"orbit": [0, 12], "aperture": aperture, "focus": 0})).unwrap();
        p
    };
    let sharp = cpu().render(&Compiled::new(&build(0.0)), 0.0);
    let p = build(0.5);
    let soft = cpu().render(&Compiled::new(&p), 0.0);
    // The word's lower half: as bright (summed) blurred as sharp, not covered by the floor.
    let sum = |img: &image::RgbaImage| (60..110).flat_map(|y| (110..210).map(move |x| (x, y))).map(|(x, y)| px(img, x, y)[0] as u64).sum::<u64>();
    let (a, b) = (sum(&sharp), sum(&soft));
    assert!(b as f64 > 0.8 * a as f64, "defocused word lost to the floor: {b} vs {a}");
    assert_gpu_matches_cpu(&p, &[0.0]);
}

/// A feather fades along the path's length: on a path of two lines only the
/// end of the second fades, not whatever lies past a straight ramp.
#[test]
fn feathers_fade_along_every_subpath() {
    let p = project(json!([{"id": "p", "kind": "shape", "shape": "path", "d": "M 20 60 H 300 M 300 120 H 20",
                            "stroke": {"color": "#ffffff", "width": 16, "cap": "butt", "feather": 60}}]));
    let img = cpu().render(&Compiled::new(&p), 0.0);
    assert_eq!(px(&img, 40, 60)[0], 255, "the first line is whole");
    assert_eq!(px(&img, 200, 120)[0], 255, "the second line before its feather");
    assert!(px(&img, 24, 120)[0] < 40, "the end of the second line fades: {:?}", px(&img, 24, 120));
    assert_gpu_matches_cpu(&p, &[0.0]);
}
