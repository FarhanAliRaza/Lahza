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
    let c = Compiled::new(&p);
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    let mut gpu = Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, ..Default::default() });
    if !gpu.is_gpu() {
        eprintln!("skipping gpu_matches_cpu: {}", gpu.backend_label());
        return;
    }
    let mut cpu = cpu();
    for t in [0.0, 0.1, 0.5] {
        let a = gpu.render(&c, t);
        let b = cpu.render(&c, t);
        let mut max = 0u8;
        let mut total = 0u64;
        for (pa, pb) in a.pixels().zip(b.pixels()) {
            for k in 0..4 {
                let d = pa.0[k].abs_diff(pb.0[k]);
                max = max.max(d);
                total += d as u64;
            }
        }
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
            pivots: vec![u32::MAX; s.chars().count()].into(),
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
