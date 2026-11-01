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
        {"id": "m", "kind": "shape", "shape": "path", "d": "M -40 -30 C 0 -60 40 -30 40 0 L 0 40 Z", "fill": "#ffffff", "transform": {"position": [60, 40]}},
        {"id": "g", "kind": "comp", "transform": {"opacity": 0.8}, "matte": {"layer": "m", "mode": "luma"},
         "effects": [{"type": "shadow", "offset": [4, 6], "radius": 3}, {"type": "glow", "color": "#88ccff", "radius": 6, "intensity": 0.6}],
         "layers": [
            {"id": "g1", "kind": "shape", "shape": "rect", "size": [70, 50], "fill": "#3366ff", "transform": {"position": [55, 40]}},
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
    assert_eq!(px(&luma, 160, 90), [255, 0, 0, 255]);
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
