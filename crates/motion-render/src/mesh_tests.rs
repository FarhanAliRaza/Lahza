//! Mesh layers: depth, lighting, materials, parity with the GPU.

use super::*;
use lahza_motion_core::document::{Project, Scene};
use lahza_motion_core::time::Fps;
use serde_json::json;

fn cpu() -> Renderer {
    Renderer::new(None, RenderOptions { backend: Backend::Cpu, ..Default::default() })
}

fn scene(w: u32, h: u32, scene: serde_json::Value) -> Project {
    let mut p = Project::new(w, h, Fps::whole(30), 2.0);
    p.settings.background = lahza_motion_core::value::Color::parse("#101014").unwrap();
    let mut s = scene;
    s["id"] = json!("s1");
    let scene: Scene = serde_json::from_value(s).unwrap();
    p.scenes.push(scene);
    p
}

fn showcase() -> Project {
    scene(960, 540, json!({"layers": [
        {"id": "ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 150, "tube": 34},
         "segments": 4, "gap": 8, "colors": ["#ff3b5c", "#ffb020", "#2ec4ff", "#7b61ff"],
         "material": {"preset": "coated"},
         "transform": {"position": [250, 270], "rotation_x": 55, "rotation": 20}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "chrome"},
         "transform": {"position": [250, 270]}},
        {"id": "box", "kind": "mesh", "mesh": {"type": "box", "size": [160, 160], "depth": 160, "bevel": 24},
         "material": {"color": "#f2f2f2", "preset": "plastic"},
         "transform": {"position": [560, 200], "rotation_x": -25, "rotation_y": 35}},
        {"id": "word", "kind": "mesh", "mesh": {"type": "extrude", "d": {"shape": "star", "size": 200, "sides": 5, "inner": 0.5}, "depth": 50, "bevel": 8},
         "material": {"color": "#ffd23f", "preset": "metal"}, "parts": {"side": {"color": "#b8860b"}},
         "transform": {"position": [790, 330], "rotation_y": -30, "rotation_x": 15}},
        {"id": "tube", "kind": "mesh", "mesh": {"type": "sweep", "d": "M -200 0 C -100 -150 100 150 200 0", "radius": 16, "head": 26, "trim": [0, 0.8]},
         "material": {"color": "#33ddaa", "preset": "glossy"},
         "transform": {"position": [560, 440], "rotation_x": 30}}
    ]}))
}

/// Writes the showcase to $LAHZA_MESH_OUT (manual look).
#[test]
#[ignore]
fn dump_showcase() {
    let p = showcase();
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let mut r = cpu();
    let t = std::time::Instant::now();
    let img = r.render(&c, 0.0);
    eprintln!("cpu frame {:?}", t.elapsed());
    let out = std::env::var("LAHZA_MESH_OUT").unwrap_or_else(|_| "/tmp/mesh.png".into());
    img.save(out).unwrap();
}

#[test]
fn gpu_matches_cpu_meshes() {
    let mut p = showcase();
    // Every kind, part materials, faceted and unlit, glass, emissive, a
    // revolve, motion blur and depth of field (a blurred mesh group).
    let more = json!([
        {"id": "vase", "kind": "mesh", "mesh": {"type": "revolve", "profile": "M 0 60 L 40 60 C 60 20 20 -20 30 -60", "angles": [0, 300]},
         "material": {"color": "#88aaff", "faceted": true}, "transform": {"position": [80, 90], "rotation_x": 20}},
        {"id": "cyl", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 40, "top": 20, "height": 80, "bevel": 6},
         "material": {"preset": "glass", "color": "#ccf0ff"}, "transform": {"position": [880, 100], "rotation_x": 30}},
        {"id": "cap", "kind": "mesh", "mesh": {"type": "capsule", "radius": 20, "height": 120}, "material": {"preset": "emissive", "color": "#ff66cc"},
         "transform": {"position": [420, 90], "rotation": {"keys": [{"at": 0, "v": 0}, {"at": 1, "v": 90}]}}},
        {"id": "flat", "kind": "mesh", "mesh": {"type": "box", "size": [60, 60], "depth": 60}, "material": {"preset": "flat", "color": "#00ff88"},
         "transform": {"position": [700, 480], "rotation_y": 30, "rotation_x": 20, "z": 200}}
    ]);
    for l in more.as_array().unwrap() {
        p.scenes[0].layers.push(serde_json::from_value(l.clone()).unwrap());
    }
    p.scenes[0].lights = serde_json::from_value(json!([
        {"type": "directional", "from": [-1, -1, -1], "intensity": 1.1},
        {"type": "point", "position": [480, 100, -300], "color": "#ffddaa", "intensity": 0.6},
        {"type": "ambient", "intensity": 0.05}
    ])).unwrap();
    p.scenes[0].environment = Some(serde_json::from_value(json!({"preset": "night", "rotation": 30})).unwrap());
    p.scenes[0].camera.aperture = lahza_motion_core::property::Property::Constant(0.6);
    p.settings.motion_blur = Some(lahza_motion_core::document::MotionBlur { shutter: 180.0, samples: 3 });
    crate::tests::assert_gpu_matches_cpu_within(&p, &[0.0, 0.5], 0.0002);
}

fn render(p: &Project, t: f64) -> image::RgbaImage {
    let c = Compiled::new(p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    cpu().render(&c, t)
}

fn px(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.get_pixel(x, y).0
}

fn luma(c: [u8; 4]) -> f64 {
    0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64
}

const BG: [u8; 4] = [16, 16, 20, 255];

fn one(layer: serde_json::Value) -> Project {
    scene(320, 240, json!({"layers": [layer]}))
}

#[test]
fn every_kind_draws_its_silhouette() {
    let at = |mesh: serde_json::Value| one(json!({"id": "m", "kind": "mesh", "mesh": mesh, "material": {"preset": "flat", "color": "#ff0000"}, "transform": {"position": [160, 120]}}));
    // (mesh, filled pixels, empty pixels)
    let cases = [
        (json!({"type": "sphere", "radius": 50}), vec![(160, 120), (205, 120)], vec![(214, 120), (160, 60)]),
        (json!({"type": "box", "size": [100, 60], "depth": 40}), vec![(112, 95), (208, 145)], vec![(160, 85), (215, 120)]),
        (json!({"type": "cylinder", "radius": 40, "height": 100}), vec![(160, 75), (195, 160)], vec![(205, 120), (160, 60)]),
        (json!({"type": "capsule", "radius": 20, "height": 120}), vec![(160, 63), (160, 177)], vec![(160, 55), (185, 120)]),
        (json!({"type": "torus", "radius": 80, "tube": 15}), vec![(80, 120), (160, 45)], vec![(160, 120), (160, 20)]),
        (json!({"type": "torus", "radius": 80, "tube": 15, "angles": [0, 90]}), vec![(160, 45), (236, 112)], vec![(85, 120), (160, 195)]),
        (json!({"type": "extrude", "d": "M -80 -60 L 80 -60 L 80 60 L -80 60 Z M -30 -30 L -30 30 L 30 30 L 30 -30 Z", "depth": 20}), vec![(90, 70), (230, 170)], vec![(160, 120), (245, 120)]),
        (json!({"type": "revolve", "profile": "M 0 -50 L 50 0 L 0 50", "axis": "z"}), vec![(160, 120), (205, 120)], vec![(215, 120)]),
        (json!({"type": "sweep", "d": "M -100 0 L 100 0", "radius": 10, "trim": [0, 0.5]}), vec![(70, 120), (150, 120)], vec![(175, 120), (70, 135)]),
    ];
    for (mesh, filled, empty) in cases {
        let img = render(&at(mesh.clone()), 0.0);
        for (x, y) in filled {
            assert_eq!(px(&img, x, y), [255, 0, 0, 255], "{mesh} filled at {x},{y}");
        }
        for (x, y) in empty {
            assert_eq!(px(&img, x, y), BG, "{mesh} empty at {x},{y}");
        }
    }
}

#[test]
fn meshes_share_a_depth_buffer_and_intersect() {
    // Two slabs crossing like an X seen from above: left of centre the red
    // one is nearer, right of centre the blue one; in either layer order.
    for order in [0, 1] {
        let red = json!({"id": "a", "kind": "mesh", "mesh": {"type": "box", "size": [200, 100], "depth": 4}, "material": {"preset": "flat", "color": "#ff0000"},
            "transform": {"position": [160, 120], "rotation_y": 30}});
        let blue = json!({"id": "b", "kind": "mesh", "mesh": {"type": "box", "size": [200, 100], "depth": 4}, "material": {"preset": "flat", "color": "#0000ff"},
            "transform": {"position": [160, 120], "rotation_y": -30}});
        let layers = if order == 0 { json!([red, blue]) } else { json!([blue, red]) };
        let img = render(&scene(320, 240, json!({"layers": layers})), 0.0);
        assert_eq!(px(&img, 110, 120), [255, 0, 0, 255], "order {order}");
        assert_eq!(px(&img, 210, 120), [0, 0, 255, 255], "order {order}");
    }
}

#[test]
fn lights_shade_the_surface() {
    let lit = |from: [f64; 3]| {
        scene(320, 240, json!({"environment": {"preset": "none"}, "lights": [{"from": from}],
            "layers": [{"id": "s", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "matte", "color": "#cccccc"}, "transform": {"position": [160, 120]}}]}))
    };
    let left = render(&lit([-1.0, 0.0, -0.3]), 0.0);
    let right = render(&lit([1.0, 0.0, -0.3]), 0.0);
    assert!(luma(px(&left, 110, 120)) > luma(px(&left, 210, 120)) + 60.0);
    assert!(luma(px(&right, 210, 120)) > luma(px(&right, 110, 120)) + 60.0);
    // The lit side falls off smoothly toward the terminator: shading, not bands.
    let row: Vec<f64> = (90..160).map(|x| luma(px(&left, x, 120))).collect();
    let distinct: std::collections::BTreeSet<u8> = row.iter().map(|v| *v as u8).collect();
    assert!(distinct.len() > 25, "smooth gradient: {} levels", distinct.len());
    // Unlit stays one flat colour.
    let flat = render(&one(json!({"id": "s", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "flat", "color": "#336699"}, "transform": {"position": [160, 120]}})), 0.0);
    for x in (100..220).step_by(10) {
        assert_eq!(px(&flat, x, 120), [0x33, 0x66, 0x99, 255]);
    }
    // A point light lights the side facing it.
    let point = scene(320, 240, json!({"environment": {"preset": "none"}, "lights": [{"type": "point", "position": [160, 300, -50]}],
        "layers": [{"id": "s", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "matte"}, "transform": {"position": [160, 120]}}]}));
    let img = render(&point, 0.0);
    assert!(luma(px(&img, 160, 185)) > luma(px(&img, 160, 55)) + 60.0);
}

#[test]
fn materials_look_different() {
    let ball = |material: serde_json::Value, env: &str| {
        render(&scene(320, 240, json!({"environment": {"preset": env},
            "layers": [{"id": "s", "kind": "mesh", "mesh": {"type": "sphere", "radius": 90}, "material": material, "transform": {"position": [160, 120]}}]})), 0.0)
    };
    let spread = |img: &image::RgbaImage| {
        let v: Vec<f64> = (0..240).flat_map(|y| (0..320).map(move |x| (x, y))).filter(|&(x, y)| ((x as f64 - 160.0).powi(2) + (y as f64 - 120.0).powi(2)).sqrt() < 80.0).map(|(x, y)| luma(px(img, x, y))).collect();
        let max = v.iter().cloned().fold(0.0, f64::max);
        let min = v.iter().cloned().fold(255.0, f64::min);
        (max, min)
    };
    let (matte_max, _) = spread(&ball(json!({"preset": "matte", "color": "#808080"}), "studio"));
    let (chrome_max, chrome_min) = spread(&ball(json!({"preset": "chrome", "color": "#ffffff"}), "studio"));
    let (glossy_max, _) = spread(&ball(json!({"preset": "glossy", "color": "#808080"}), "studio"));
    assert!(chrome_max - chrome_min > 150.0, "chrome reflects the studio: {chrome_min}..{chrome_max}");
    assert!(glossy_max > matte_max + 25.0, "glossy highlights: {glossy_max} vs {matte_max}");
    // Gold metal tints its reflections.
    let gold = ball(json!({"preset": "metal", "color": "#ffc030"}), "studio");
    let c = (0..240).flat_map(|y| (0..320).map(move |x| (x, y))).map(|(x, y)| px(&gold, x, y)).max_by_key(|c| c[0] as u32 + c[1] as u32).unwrap();
    assert!(c[0] > c[2] + 60, "metal highlights take the colour: {c:?}");
    // Emissive shows with no light at all.
    let glow = render(&scene(320, 240, json!({"environment": {"preset": "none"}, "lights": [{"type": "ambient", "intensity": 0}],
        "layers": [{"id": "s", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": {"preset": "emissive", "color": "#ff2080"}, "transform": {"position": [160, 120]}}]})), 0.0);
    assert!(px(&glow, 160, 120)[0] > 240);
    // Half opacity and glass let the background through.
    let mut white = one(json!({"id": "s", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "glass"}, "transform": {"position": [160, 120]}}));
    white.settings.background = lahza_motion_core::value::Color::parse("#ff0000").unwrap();
    let glass = render(&white, 0.0);
    let c = px(&glass, 160, 120);
    assert!(c[0] > 150 && c[1] < 120, "glass shows the red behind it: {c:?}");
    let half = render(&one(json!({"id": "s", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "flat", "color": "#ffffff", "opacity": 0.5}, "transform": {"position": [160, 120]}})), 0.0);
    let c = px(&half, 160, 120);
    assert!((c[0] as i32 - 136).abs() <= 2, "{c:?}");
}

#[test]
fn segments_and_parts_take_their_colours() {
    let img = render(&one(json!({"id": "r", "kind": "mesh", "mesh": {"type": "torus", "radius": 80, "tube": 18}, "segments": 4, "gap": 10,
        "colors": ["#ff0000", "#00ff00", "#0000ff", "#ffff00"], "material": {"preset": "flat"}, "transform": {"position": [160, 120]}})), 0.0);
    // Clockwise from the top: up-right red, down-right green, down-left blue, up-left yellow.
    let d = 80.0 / std::f64::consts::SQRT_2;
    assert_eq!(px(&img, (160.0 + d) as u32, (120.0 - d) as u32), [255, 0, 0, 255]);
    assert_eq!(px(&img, (160.0 + d) as u32, (120.0 + d) as u32), [0, 255, 0, 255]);
    assert_eq!(px(&img, (160.0 - d) as u32, (120.0 + d) as u32), [0, 0, 255, 255]);
    assert_eq!(px(&img, (160.0 - d) as u32, (120.0 - d) as u32), [255, 255, 0, 255]);
    // The gaps at 0°, 90°, … are empty.
    assert_eq!(px(&img, 160, 40), BG);
    assert_eq!(px(&img, 240, 120), BG);
    // Parts: a slab turned sideways shows its side material.
    let slab = render(&one(json!({"id": "x", "kind": "mesh", "mesh": {"type": "extrude", "d": {"shape": "rect", "size": [120, 120]}, "depth": 120},
        "material": {"preset": "flat", "color": "#ff0000"}, "parts": {"side": {"color": "#0000ff"}}, "transform": {"position": [160, 120], "rotation_y": 45}})), 0.0);
    let reds = slab.pixels().filter(|p| p.0 == [255, 0, 0, 255]).count();
    let blues = slab.pixels().filter(|p| p.0 == [0, 0, 255, 255]).count();
    assert!(reds > 2000 && blues > 2000, "front {reds}, side {blues}");
}

#[test]
fn meshes_crossing_the_camera_are_clipped() {
    let mut p = one(json!({"id": "b", "kind": "mesh", "mesh": {"type": "box", "size": [400, 400], "depth": 2000}, "material": {"preset": "matte"}, "transform": {"position": [160, 120]}}));
    p.scenes[0].camera.dolly = lahza_motion_core::property::Property::Constant(0.0);
    let img = render(&p, 0.0);
    // The camera sits inside the box's depth: its walls fill the view
    // without panics or garbage.
    assert!(img.pixels().all(|p| p.0[3] == 255));
}

/// 1080p, 40 glossy meshes of a few thousand triangles each, 3 lights.
/// `cargo test -p lahza-motion-render --release -- --ignored --nocapture benchmark_meshes`
#[test]
#[ignore]
fn benchmark_meshes_1080p() {
    let mut layers = Vec::new();
    for i in 0..40 {
        let (x, y) = (160.0 + (i % 8) as f64 * 230.0, 140.0 + (i / 8) as f64 * 200.0);
        let mesh = match i % 4 {
            0 => json!({"type": "torus", "radius": 70, "tube": 22}),
            1 => json!({"type": "sphere", "radius": 75}),
            2 => json!({"type": "box", "size": [110, 110], "depth": 110, "bevel": 18}),
            _ => json!({"type": "extrude", "d": {"shape": "star", "size": 160, "sides": 5}, "depth": 40, "bevel": 6}),
        };
        layers.push(json!({"id": format!("m{i}"), "kind": "mesh", "mesh": mesh, "segments": if i % 4 == 0 { 4 } else { 1 },
            "colors": ["#ff3b5c", "#ffb020", "#2ec4ff", "#7b61ff"], "material": {"preset": "coated", "color": "#e0e0e0"},
            "transform": {"position": [x, y], "rotation_x": {"keys": [{"at": 0, "v": 0}, {"at": 2, "v": 360}]}, "rotation_y": 25 + i}}));
    }
    let mut p = scene(1920, 1080, json!({"layers": layers, "lights": [
        {"from": [-1, -1, -1]}, {"from": [1, -0.4, 1], "intensity": 0.7}, {"type": "point", "position": [960, 200, -400], "intensity": 0.5}]}));
    p.settings.duration = 2.0;
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let tris: usize = c.frame(0.0).layers.iter().filter_map(|l| match &l.content {
        Content::Mesh(m) => Some(lahza_motion_core::mesh::generate(&m.params).unwrap().tris.len()),
        _ => None,
    }).sum();
    for backend in [Backend::Gpu, Backend::Cpu] {
        let mut r = Renderer::new(None, RenderOptions { backend, allow_software_gpu: true, motion_blur: false, ..Default::default() });
        let label = r.backend_label();
        r.render(&c, 0.0);
        r.stats = Stats::default();
        let n = 10;
        let t = std::time::Instant::now();
        for i in 0..n {
            r.render(&c, 0.05 + i as f64 * 0.1);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        eprintln!("{label}: {tris} triangles, {ms:.1} ms/frame (build {:.1} ms, raster {:.1} ms)", r.stats.build_ms / n as f64, r.stats.raster_ms / n as f64);
    }
}

#[test]
fn meshes_drawn_on_their_own_still_share_depth() {
    // The X of slabs again, with the red slab drawn on its own (an effect,
    // a blend mode, depth of field) and the blue one see-through.
    let slab = |id: &str, color: &str, turn: f64, extra: serde_json::Value| {
        let mut l = json!({"id": id, "kind": "mesh", "mesh": {"type": "box", "size": [200, 100], "depth": 4}, "material": {"preset": "flat", "color": color},
            "transform": {"position": [160, 120], "rotation_y": turn}});
        for (k, v) in extra.as_object().unwrap() {
            l[k] = v.clone();
        }
        l
    };
    let variants = [
        json!({"effects": [{"type": "brightness_contrast", "brightness": 0}]}),
        json!({"effects": [{"type": "blur", "radius": 0.2}]}),
    ];
    for extra in variants {
        for order in [0, 1] {
            let red = slab("a", "#ff0000", 30.0, extra.clone());
            let blue = slab("b", "#0000ff", -30.0, json!({}));
            let layers = if order == 0 { json!([red, blue]) } else { json!([blue, red]) };
            let img = render(&scene(320, 240, json!({"layers": layers})), 0.0);
            let (l, r) = (px(&img, 110, 120), px(&img, 210, 120));
            assert!(l[0] > 240 && l[2] < 15, "{extra} order {order}: left {l:?}");
            assert!(r[2] > 240 && r[0] < 15, "{extra} order {order}: right {r:?}");
        }
    }
    // A blend mode blends a mesh in front over the meshes behind it, and
    // hides nothing in front of it.
    for order in [0, 1] {
        let red = slab("a", "#ff0000", 30.0, json!({"blend": "lighten"}));
        let blue = slab("b", "#0000ff", -30.0, json!({}));
        let layers = if order == 0 { json!([red, blue]) } else { json!([blue, red]) };
        let img = render(&scene(320, 240, json!({"layers": layers})), 0.0);
        assert_eq!(px(&img, 110, 120), [255, 0, 255, 255], "order {order}");
        assert_eq!(px(&img, 210, 120), [0, 0, 255, 255], "order {order}");
    }
    // A half-transparent blue slab: in front (right) it tints the red
    // behind it; behind (left) the opaque red hides it.
    for order in [0, 1] {
        let red = slab("a", "#ff0000", 30.0, json!({}));
        let blue = slab("b", "#0000ff", -30.0, json!({"transform": {"position": [160, 120], "rotation_y": -30, "opacity": 0.5}}));
        let layers = if order == 0 { json!([red, blue]) } else { json!([blue, red]) };
        let img = render(&scene(320, 240, json!({"layers": layers})), 0.0);
        assert_eq!(px(&img, 110, 120), [255, 0, 0, 255], "order {order}");
        let r = px(&img, 210, 120);
        assert!((r[0] as i32 - 128).abs() <= 3 && (r[2] as i32 - 128).abs() <= 3, "order {order}: {r:?}");
    }
}

fn shadow_scene(floor: serde_json::Value, softness: f64) -> Project {
    scene(640, 400, json!({"camera": {"orbit": [0, 18]}, "lights": [{"from": [-0.8, -1.2, -0.5], "softness": softness}],
        "layers": [
            {"id": "bg", "kind": "shape", "shape": "rect", "size": [640, 400], "fill": {"gradient": "linear", "from": [0, -200], "to": [0, 200], "stops": [[0, "#e9e4da"], [1, "#c9c2b4"]]}, "transform": {"position": [320, 200]}},
            {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 700]}, "material": floor, "transform": {"position": [320, 300], "rotation_x": 90}},
            {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 70}, "material": {"preset": "glossy", "color": "#ff3b5c"}, "transform": {"position": [240, 230]}},
            {"id": "ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 70, "tube": 18}, "material": {"preset": "coated", "color": "#2d5bff"}, "transform": {"position": [430, 220], "rotation_y": 30}}
        ]}))
}

#[test]
#[ignore]
fn dump_shadows() {
    let out = std::env::var("LAHZA_MESH_OUT").unwrap_or_else(|_| "/tmp".into());
    for (name, floor, soft) in [("floor", json!({"preset": "matte", "color": "#e8e2d6"}), 0.35), ("catcher", json!({"preset": "shadow"}), 0.6), ("hard", json!({"preset": "matte", "color": "#e8e2d6"}), 0.0)] {
        let t = std::time::Instant::now();
        let img = render(&shadow_scene(floor, soft), 0.0);
        eprintln!("{name}: {:?}", t.elapsed());
        img.save(format!("{out}/shadow-{name}.png")).unwrap();
    }
}

#[test]
fn meshes_cast_soft_shadows() {
    let floor = json!({"preset": "matte", "color": "#e8e2d6"});
    let lit = render(&shadow_scene(floor.clone(), 0.35), 0.0);
    // Where the ball's shadow falls (behind it, away from the key on the
    // upper left) the floor is much darker than open floor.
    let shade = luma(px(&lit, 330, 280));
    let open = luma(px(&lit, 120, 360));
    assert!(open - shade > 50.0, "shadow {shade} vs open floor {open}");
    // Without shadows from that light the floor there is open floor.
    let mut p = shadow_scene(floor.clone(), 0.35);
    p.scenes[0].lights[0].shadow = Some(false);
    let off = render(&p, 0.0);
    assert!((luma(px(&off, 330, 280)) - luma(px(&off, 300, 360))).abs() < 25.0);
    // Softer shadows have a wider penumbra: more in-between pixels across
    // the shadow's edge.
    let edge = |img: &image::RgbaImage| (250..420).filter(|&x| { let l = luma(px(img, x, 285)); l > shade + 15.0 && l < open - 15.0 }).count();
    let (hard, soft) = (edge(&render(&shadow_scene(floor.clone(), 0.0), 0.0)), edge(&render(&shadow_scene(floor, 1.0), 0.0)));
    assert!(soft > hard + 4, "penumbra soft {soft} vs hard {hard}");
    // A shadow catcher is invisible except for the shadows: the 2D
    // background shows through, darkened only in the shadow.
    let catcher = render(&shadow_scene(json!({"preset": "shadow"}), 0.35), 0.0);
    let bg = |y: f64| 0xe9 as f64 + (0xc9 as f64 - 0xe9 as f64) * (y / 400.0);
    let open_c = px(&catcher, 120, 360);
    assert!((open_c[0] as f64 - bg(360.0)).abs() < 4.0, "open catcher shows the background: {open_c:?}");
    assert!(luma(px(&catcher, 330, 280)) < bg(280.0) - 40.0);
}

#[test]
fn gpu_matches_cpu_shadows_and_occluders() {
    let mut p = shadow_scene(json!({"preset": "matte", "color": "#e8e2d6"}), 0.5);
    p.scenes[0].lights.push(serde_json::from_value(json!({"type": "point", "position": [520, 80, -100], "intensity": 0.5, "softness": 0.1})).unwrap());
    p.scenes[0].layers.push(serde_json::from_value(json!({"id": "glow", "kind": "mesh", "mesh": {"type": "box", "size": [60, 60], "depth": 60, "bevel": 10},
        "material": {"preset": "glossy", "color": "#ffd23f"}, "effects": [{"type": "glow", "radius": 6}], "transform": {"position": [320, 250], "rotation_y": 30}})).unwrap());
    p.scenes[0].layers.push(serde_json::from_value(json!({"id": "catch", "kind": "mesh", "mesh": {"type": "plane", "size": [200, 200]}, "material": {"preset": "shadow"},
        "transform": {"position": [540, 300], "rotation_x": 90}})).unwrap());
    crate::tests::assert_gpu_matches_cpu_within(&p, &[0.0], 0.0005);
}

/// Glass over stripes: real refraction bends them (and frosts with roughness).
fn glass_scene(material: serde_json::Value) -> Project {
    let mut layers = vec![json!({"id": "bg", "kind": "shape", "shape": "rect", "size": [640, 400], "fill": "#f4f1ea", "transform": {"position": [320, 200]}})];
    for i in 0..16 {
        let fill = ["#ff3b5c", "#2d5bff", "#111111", "#ffd23f"][i % 4];
        layers.push(json!({"id": format!("stripe{i}"), "kind": "shape", "shape": "rect", "size": [18, 400], "fill": fill, "transform": {"position": [20 + i * 40, 200]}}));
    }
    layers.push(json!({"id": "t", "kind": "text", "text": "GLASS", "size": 90, "color": "#111111", "transform": {"position": [320, 340]}}));
    layers.push(json!({"id": "g", "kind": "mesh", "mesh": {"type": "sphere", "radius": 120}, "material": material, "transform": {"position": [220, 190]}}));
    layers.push(json!({"id": "c", "kind": "mesh", "mesh": {"type": "capsule", "radius": 50, "height": 260}, "material": {"preset": "glass", "roughness": 0.45}, "transform": {"position": [480, 190], "rotation": 30}}));
    scene(640, 400, json!({"layers": layers}))
}

#[test]
#[ignore]
fn dump_glass() {
    let out = std::env::var("LAHZA_MESH_OUT").unwrap_or_else(|_| "/tmp".into());
    let img = render(&glass_scene(json!({"preset": "glass"})), 0.0);
    img.save(format!("{out}/glass.png")).unwrap();
}

#[test]
fn glass_refracts_what_is_behind() {
    let clear = render(&glass_scene(json!({"preset": "glass"})), 0.0);
    let mut none = glass_scene(json!({"preset": "glass"}));
    none.scenes[0].layers.retain(|l| l.id != "g" && l.id != "c");
    let plain = render(&none, 0.0);
    // Through the sphere the stripes are bent: the row across it differs
    // from the plain stripes much more than tint and gloss alone would.
    let row = |img: &image::RgbaImage| (120..320).map(|x| px(img, x, 190)).collect::<Vec<_>>();
    let (a, b) = (row(&clear), row(&plain));
    let moved = a.iter().zip(&b).filter(|(p, q)| (0..3).map(|k| p[k].abs_diff(q[k]) as u32).sum::<u32>() > 120).count();
    assert!(moved > 40, "only {moved} pixels changed: no refraction");
    // Colours stay what is behind (red stays red, not grey).
    assert!(a.iter().any(|p| p[0] > 200 && p[1] < 120 && p[2] < 140), "the red stripes show through");
    // Frosted glass (the capsule) blurs: neighbouring pixels differ less
    // than the crisp stripes do.
    let contrast = |img: &image::RgbaImage, y: u32, x0: u32, x1: u32| (x0..x1).map(|x| luma(px(img, x + 1, y)) - luma(px(img, x, y))).map(f64::abs).sum::<f64>();
    let frosted = render(&glass_scene(json!({"preset": "glass"})), 0.0);
    assert!(contrast(&frosted, 190, 450, 510) < 0.35 * contrast(&plain, 190, 450, 510), "frosted glass blurs");
}

#[test]
fn gpu_matches_cpu_glass() {
    crate::tests::assert_gpu_matches_cpu_within(&glass_scene(json!({"preset": "glass", "color": "#e8f4ff", "ior": 1.6, "thickness": 90})), &[0.0], 0.0005);
}

#[test]
fn flat_3d_layers_share_the_mesh_depth_buffer() {
    // A red 3D rectangle and a blue slab crossing like an X: each is in
    // front on its own side, whatever the layer order.
    for order in [0, 1] {
        let card = json!({"id": "card", "kind": "shape", "shape": "rect", "size": [200, 100], "fill": "#ff0000", "three_d": true,
            "transform": {"position": [160, 120], "rotation_y": 30}});
        let slab = json!({"id": "slab", "kind": "mesh", "mesh": {"type": "box", "size": [200, 100], "depth": 4}, "material": {"preset": "flat", "color": "#0000ff"},
            "transform": {"position": [160, 120], "rotation_y": -30}});
        let layers = if order == 0 { json!([card, slab]) } else { json!([slab, card]) };
        let img = render(&scene(320, 240, json!({"layers": layers})), 0.0);
        assert_eq!(px(&img, 110, 120), [255, 0, 0, 255], "order {order}: left");
        assert_eq!(px(&img, 210, 120), [0, 0, 255, 255], "order {order}: right");
    }
    // A card only stands where its picture is: between two bars of a
    // 3D layer in front, the mesh behind shows.
    let bars = json!({"id": "bars", "kind": "shape", "shape": "path", "d": "M -100 -50 L -40 -50 L -40 50 L -100 50 Z M 40 -50 L 100 -50 L 100 50 L 40 50 Z",
        "fill": "#ff0000", "three_d": true, "transform": {"position": [160, 120], "z": -40}});
    let wall = json!({"id": "wall", "kind": "mesh", "mesh": {"type": "box", "size": [260, 160], "depth": 10}, "material": {"preset": "flat", "color": "#0000ff"},
        "transform": {"position": [160, 120], "z": 40}});
    let img = render(&scene(320, 240, json!({"layers": [wall, bars]})), 0.0);
    assert_eq!(px(&img, 160, 120), [0, 0, 255, 255], "the gap shows the mesh");
    assert_eq!(px(&img, 160 - 75, 120)[0], 255, "the bar in front");
    // A mesh with an effect (drawn on its own) is still hidden by a card.
    let mut blurred = json!({"id": "slab", "kind": "mesh", "mesh": {"type": "box", "size": [200, 100], "depth": 4}, "material": {"preset": "flat", "color": "#0000ff"},
        "effects": [{"type": "brightness_contrast", "brightness": 0}], "transform": {"position": [160, 120], "rotation_y": -30}});
    blurred["id"] = json!("slab2");
    let card = json!({"id": "card", "kind": "shape", "shape": "rect", "size": [200, 100], "fill": "#ff0000", "three_d": true,
        "transform": {"position": [160, 120], "rotation_y": 30}});
    let img = render(&scene(320, 240, json!({"layers": [card, blurred]})), 0.0);
    assert_eq!(px(&img, 110, 120), [255, 0, 0, 255]);
    assert_eq!(px(&img, 210, 120), [0, 0, 255, 255]);
}

#[test]
fn tilted_3d_layers_cut_through_each_other() {
    // Two flat 3D rectangles crossing like an X, no meshes at all.
    for order in [0, 1] {
        let a = json!({"id": "a", "kind": "shape", "shape": "rect", "size": [200, 100], "fill": "#ff0000", "three_d": true, "transform": {"position": [160, 120], "rotation_y": 30}});
        let b = json!({"id": "b", "kind": "shape", "shape": "rect", "size": [200, 100], "fill": "#0000ff", "three_d": true, "transform": {"position": [160, 120], "rotation_y": -30}});
        let layers = if order == 0 { json!([a, b]) } else { json!([b, a]) };
        let img = render(&scene(320, 240, json!({"layers": layers})), 0.0);
        assert_eq!(px(&img, 110, 120), [255, 0, 0, 255], "order {order}");
        assert_eq!(px(&img, 210, 120), [0, 0, 255, 255], "order {order}");
    }
    // Text on a card in the same tilted plane stays on top (paint order
    // breaks the tie).
    let card = json!({"id": "card", "kind": "shape", "shape": "rect", "size": [220, 120], "fill": "#202020", "three_d": true, "transform": {"position": [160, 120], "rotation_y": 35}});
    let text = json!({"id": "label", "kind": "shape", "shape": "rect", "size": [120, 40], "fill": "#ffffff", "three_d": true, "transform": {"position": [160, 120], "rotation_y": 35}});
    let img = render(&scene(320, 240, json!({"layers": [card, text]})), 0.0);
    assert_eq!(px(&img, 160, 120), [255, 255, 255, 255]);
}

#[test]
fn gpu_matches_cpu_cards() {
    let p = scene(320, 240, json!({"layers": [
        {"id": "wall", "kind": "mesh", "mesh": {"type": "box", "size": [260, 160], "depth": 10}, "material": {"preset": "glossy", "color": "#3355ff"}, "transform": {"position": [160, 120], "z": 40, "rotation_y": 20}},
        {"id": "card", "kind": "shape", "shape": "rect", "size": [200, 100], "radius": 12, "fill": "#ff3355", "three_d": true, "transform": {"position": [160, 120], "rotation_y": -25}},
        {"id": "word", "kind": "text", "text": "DEPTH", "size": 40, "color": "#ffffff", "three_d": true, "transform": {"position": [160, 120], "rotation_y": -25}},
        {"id": "glow", "kind": "mesh", "mesh": {"type": "torus", "radius": 70, "tube": 14}, "material": {"preset": "coated", "color": "#ffd23f"},
         "effects": [{"type": "glow", "radius": 4}], "transform": {"position": [160, 120], "rotation_x": 70}}
    ]}));
    crate::tests::assert_gpu_matches_cpu_within(&p, &[0.0], 0.001);
}

#[test]
fn two_tone_rings_colour_parts_per_segment() {
    // Front faces cycle red/green per segment; the sides stay blue.
    let img = render(&one(json!({"id": "r", "kind": "mesh", "mesh": {"type": "torus", "radius": 80, "tube": 20}, "segments": 2, "spans": [1, 3],
        "material": {"preset": "flat", "color": "#0000ff"}, "parts": {"front": {"colors": ["#ff0000", "#00ff00"]}},
        "transform": {"position": [160, 120]}})), 0.0);
    // Segment 0 spans 0–90° (up-right), segment 1 the other three quarters.
    let d = 80.0 / std::f64::consts::SQRT_2;
    assert_eq!(px(&img, (160.0 + d) as u32, (120.0 - d) as u32), [255, 0, 0, 255]);
    assert_eq!(px(&img, (160.0 - d) as u32, (120.0 + d) as u32), [0, 255, 0, 255]);
    assert_eq!(px(&img, (160.0 + d) as u32, (120.0 + d) as u32), [0, 255, 0, 255]);
    // Seen edge-on-ish, the outer side shows blue.
    let tilted = render(&one(json!({"id": "r", "kind": "mesh", "mesh": {"type": "torus", "radius": 80, "tube": 20}, "segments": 2,
        "material": {"preset": "flat", "color": "#0000ff"}, "parts": {"front": {"colors": ["#ff0000", "#00ff00"]}},
        "transform": {"position": [160, 120], "rotation_x": 75}})), 0.0);
    assert!(tilted.pixels().filter(|p| p.0 == [0, 0, 255, 255]).count() > 300);
}

fn wire_scene() -> Project {
    scene(640, 360, json!({"background": "#2a2c31", "camera": {"orbit": [20, 15]}, "layers": [
        {"id": "box", "kind": "mesh", "mesh": {"type": "box", "size": [140, 140], "depth": 140, "bevel": 20}, "material": {"preset": "wireframe"},
         "transform": {"position": [150, 180], "rotation_y": 25}},
        {"id": "torus", "kind": "mesh", "mesh": {"type": "torus", "radius": 90, "tube": 30}, "material": {"preset": "wireframe", "color": "#ffb020", "fill": "#3a3d44"},
         "transform": {"position": [340, 180], "rotation_x": 60}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "glossy", "color": "#5fd4ff"}, "transform": {"position": [530, 180]}},
        {"id": "cage", "kind": "mesh", "mesh": {"type": "sphere", "radius": 95}, "material": {"preset": "wireframe", "wireframe": 1}, "transform": {"position": [530, 180]}}
    ]}))
}

#[test]
#[ignore]
fn dump_wireframe() {
    let out = std::env::var("LAHZA_MESH_OUT").unwrap_or_else(|_| "/tmp".into());
    render(&wire_scene(), 0.0).save(format!("{out}/wire.png")).unwrap();
}

#[test]
fn wireframes_draw_edges() {
    let img = render(&wire_scene(), 0.0);
    // The see-through box: thin light lines with the background between.
    let grey = img.pixels().filter(|p| p.0[0] > 200 && p.0[1] > 200 && p.0[2] > 200).count();
    assert!(grey > 1500, "wires drawn: {grey}");
    assert_eq!(px(&img, 150, 180), [0x2a, 0x2c, 0x31, 255], "inside the x-ray box is background");
    // The hidden-line torus: its fill shows between the orange wires.
    let fill = img.pixels().filter(|p| p.0 == [0x3a, 0x3d, 0x44, 255]).count();
    let orange = img.pixels().filter(|p| p.0[0] > 240 && p.0[1] > 160 && p.0[1] < 190 && p.0[2] < 60).count();
    assert!(fill > 3000 && orange > 1500, "fill {fill}, wires {orange}");
    // Quad diagonals are not drawn: a flat-shaded box face has only its outline.
    let face = render(&one(json!({"id": "b", "kind": "mesh", "mesh": {"type": "box", "size": [160, 160], "depth": 160}, "material": {"preset": "wireframe", "wireframe": 2, "fill": "#000000"}, "transform": {"position": [160, 120]}})), 0.0);
    let lit = |x0: u32, x1: u32, y: u32| (x0..x1).filter(|&x| face.get_pixel(x, y).0[0] > 100).count();
    assert_eq!(lit(100, 220, 120), 0, "no diagonal across the face");
    assert!(lit(30, 100, 120) >= 1, "the left edge");
}

#[test]
fn gpu_matches_cpu_wireframes() {
    crate::tests::assert_gpu_matches_cpu_within(&wire_scene(), &[0.0], 0.001);
}

/// A sphere of `material` under `lights`, in `environment`.
fn sphere_in(material: serde_json::Value, lights: serde_json::Value, environment: serde_json::Value) -> Project {
    scene(320, 240, json!({"layers": [{"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 90}, "material": material,
        "transform": {"position": [160, 120]}}], "lights": lights, "environment": environment}))
}

/// Linear 0.5 everywhere (sRGB 188), and no lights.
fn uniform() -> (serde_json::Value, serde_json::Value) {
    (json!([{"type": "ambient", "intensity": 0}]), json!({"preset": "none", "sky": "#bcbcbc", "horizon": "#bcbcbc", "ground": "#bcbcbc"}))
}

/// Mean of channel `c` over the sphere's disc (radius `r` around the centre).
fn disc_mean(img: &image::RgbaImage, r: f64, c: usize) -> f64 {
    let (mut sum, mut n) = (0.0, 0.0);
    for y in 0..240u32 {
        for x in 0..320u32 {
            if ((x as f64 - 160.0).powi(2) + (y as f64 - 120.0).powi(2)).sqrt() < r {
                sum += px(img, x, y)[c] as f64;
                n += 1.0;
            }
        }
    }
    sum / n
}

/// The furnace: white surfaces lit only by a uniform environment reflect
/// exactly what arrives, rough metals included (their light that single
/// scattering loses is given back), so they vanish against it.
#[test]
fn white_surfaces_vanish_in_a_uniform_environment() {
    let (lights, env) = uniform();
    for material in [
        json!({"preset": "metal", "color": "#ffffff", "roughness": 1}),
        json!({"preset": "metal", "color": "#ffffff", "roughness": 0.5}),
        json!({"preset": "matte", "color": "#ffffff"}),
        json!({"preset": "plastic", "color": "#ffffff", "roughness": 0.2}),
    ] {
        let img = render(&sphere_in(material.clone(), lights.clone(), env.clone()), 0.0);
        let mean = disc_mean(&img, 80.0, 1);
        assert!((mean - 188.0).abs() < 5.0, "{material}: mean {mean:.1}, environment 188");
    }
}

/// Pixels of the highlight (luma above `k` × the brightest) and their
/// spread (standard deviation) across x and y.
fn highlight_spread(img: &image::RgbaImage, k: f64) -> (f64, f64) {
    let max = img.pixels().map(|p| luma(p.0)).fold(0.0, f64::max);
    let pts: Vec<(f64, f64)> = img.enumerate_pixels().filter(|(_, _, p)| luma(p.0) > k * max).map(|(x, y, _)| (x as f64, y as f64)).collect();
    let n = pts.len() as f64;
    let (mx, my) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
    ((pts.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>() / n).sqrt(), (pts.iter().map(|p| (p.1 - my).powi(2)).sum::<f64>() / n).sqrt())
}

/// Brushed metal: the grain runs around the mesh's axis (y), so a light's
/// highlight stretches across it, up and down the sphere; turned 90° it
/// stretches sideways.
#[test]
fn brushed_metal_stretches_highlights_across_the_grain() {
    let lights = json!([{"type": "directional", "from": [0, 0, -1], "intensity": 1.5}]);
    let env = json!({"preset": "none"});
    let spread = |m: serde_json::Value| highlight_spread(&render(&sphere_in(m, lights.clone(), env.clone()), 0.0), 0.5);
    let (px_, py_) = spread(json!({"preset": "metal", "color": "#ffffff", "roughness": 0.3}));
    assert!((px_ / py_ - 1.0).abs() < 0.2, "isotropic: round ({px_:.1} × {py_:.1})");
    let (bx, by) = spread(json!({"preset": "brushed", "color": "#ffffff"}));
    assert!(by > 1.8 * bx, "brushed: tall ({bx:.1} × {by:.1})");
    let (rx, ry) = spread(json!({"preset": "brushed", "color": "#ffffff", "anisotropy_rotation": 90}));
    assert!(rx > 1.8 * ry, "turned: wide ({rx:.1} × {ry:.1})");
}

/// Sheen lights a velvet's edges: brighter rim against the centre than the
/// same matte without it.
#[test]
fn sheen_brightens_the_rim() {
    let lights = json!([{"type": "directional", "from": [0, -0.3, -1]}]);
    let env = json!({"preset": "soft"});
    let rim_over_centre = |m: serde_json::Value| {
        let img = render(&sphere_in(m, lights.clone(), env.clone()), 0.0);
        let rim = (disc_mean(&img, 88.0, 1) * 88.0f64.powi(2) - disc_mean(&img, 80.0, 1) * 80.0f64.powi(2)) / (88.0f64.powi(2) - 80.0f64.powi(2));
        rim / disc_mean(&img, 30.0, 1)
    };
    let plain = rim_over_centre(json!({"preset": "matte", "color": "#802040"}));
    let velvet = rim_over_centre(json!({"preset": "velvet", "color": "#802040", "sheen_color": "#ffffff"}));
    assert!(velvet > plain * 1.3, "rim/centre: velvet {velvet:.2}, matte {plain:.2}");
}

/// `specular` sets how much a dielectric reflects (0: none) and
/// `specular_tint` colours it; `clearcoat_color` tints what is under a coat.
#[test]
fn specular_level_and_tints() {
    let lights = json!([{"type": "directional", "from": [0, 0, -1], "intensity": 0.5}]);
    let env = json!({"preset": "studio"});
    let shot = |m: serde_json::Value| render(&sphere_in(m, lights.clone(), env.clone()), 0.0);
    let off = shot(json!({"preset": "glossy", "color": "#000000", "clearcoat": 0, "specular": 0}));
    let on_ball = (0..240u32).flat_map(|y| (0..320u32).map(move |x| (x, y))).filter(|&(x, y)| ((x as f64 - 160.0).powi(2) + (y as f64 - 120.0).powi(2)).sqrt() < 85.0);
    let brightest = on_ball.map(|(x, y)| luma(px(&off, x, y))).fold(0.0, f64::max);
    assert!(brightest < 3.0, "no reflection at specular 0: {brightest}");
    let tinted = shot(json!({"preset": "glossy", "color": "#000000", "clearcoat": 0, "specular": 1, "specular_tint": "#ff2020", "roughness": 0.2}));
    let c = px(&tinted, 160, 120);
    assert!(c[0] > 60 && c[0] as f64 > 2.0 * c[1] as f64, "red reflection at the highlight: {c:?}");
    let candy = shot(json!({"preset": "coated", "color": "#ffffff", "clearcoat_color": "#ff3030"}));
    let c = px(&candy, 130, 150);
    assert!(c[0] as f64 > 2.0 * c[1] as f64, "red under the coat: {c:?}");
}

#[test]
fn gpu_matches_cpu_principled_materials() {
    let p = scene(640, 240, json!({"layers": [
        {"id": "spun", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 60, "height": 50, "bevel": 6}, "material": {"preset": "brushed", "color": "#d0d4da", "anisotropy_rotation": 30},
         "transform": {"position": [90, 120], "rotation_x": 40}},
        {"id": "velvet", "kind": "mesh", "mesh": {"type": "sphere", "radius": 55}, "material": {"preset": "velvet", "color": "#5a1030", "sheen_color": "#ffc0d0"},
         "transform": {"position": [230, 120]}},
        {"id": "gold", "kind": "mesh", "mesh": {"type": "torus", "radius": 50, "tube": 18}, "material": {"preset": "metal", "color": "#ffc356", "specular_tint": "#ff8040", "roughness": 0.25},
         "transform": {"position": [370, 120], "rotation_x": 60}},
        {"id": "candy", "kind": "mesh", "mesh": {"type": "box", "size": [80, 80], "depth": 80, "bevel": 10}, "material": {"preset": "coated", "color": "#f0f0f0", "clearcoat_color": "#30a0ff", "specular": 0.8},
         "transform": {"position": [510, 120], "rotation_y": 30, "rotation_x": 20}}
    ]}));
    crate::tests::assert_gpu_matches_cpu_within(&p, &[0.0], 0.0002);
}

#[test]
#[ignore]
fn dump_principled() {
    let p = scene(960, 300, json!({"layers": [
        {"id": "spun", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 90, "height": 40, "bevel": 8}, "material": {"preset": "brushed", "color": "#d0d4da"},
         "transform": {"position": [120, 150], "rotation_x": 55}},
        {"id": "velvet", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "velvet", "color": "#5a1030", "sheen_color": "#ffc0d0"},
         "transform": {"position": [330, 150]}},
        {"id": "gold", "kind": "mesh", "mesh": {"type": "torus", "radius": 70, "tube": 24}, "material": {"preset": "metal", "color": "#ffc356", "specular_tint": "#ff8040", "roughness": 0.25},
         "transform": {"position": [530, 150], "rotation_x": 60}},
        {"id": "candy", "kind": "mesh", "mesh": {"type": "box", "size": [110, 110], "depth": 110, "bevel": 14}, "material": {"preset": "coated", "color": "#f0f0f0", "clearcoat_color": "#30a0ff"},
         "transform": {"position": [720, 150], "rotation_y": 30, "rotation_x": 20}},
        {"id": "rough", "kind": "mesh", "mesh": {"type": "sphere", "radius": 70}, "material": {"preset": "metal", "color": "#ffffff", "roughness": 0.8},
         "transform": {"position": [880, 150]}}
    ]}));
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    cpu().render(&c, 0.0).save(std::env::var("LAHZA_MESH_OUT").unwrap_or_else(|_| "/tmp/principled.png".into())).unwrap();
}

/// Scene `background` and environment colours take keys: the backdrop
/// fades, and what the meshes reflect changes with it.
#[test]
fn background_and_environment_colours_animate() {
    let keys = |a: &str, b: &str| json!({"keys": [{"at": 0, "v": a}, {"at": 1, "v": b}]});
    let mut p = sphere_in(json!({"preset": "chrome", "color": "#ffffff"}), json!([{"type": "ambient", "intensity": 0}]),
        json!({"preset": "none", "sky": keys("#ff0000", "#0000ff"), "horizon": keys("#ff0000", "#0000ff"), "ground": keys("#ff0000", "#0000ff")}));
    p.scenes[0].background = Some(serde_json::from_value(keys("#000000", "#ffffff")).unwrap());
    let (a, b) = (render(&p, 0.0), render(&p, 1.0));
    assert_eq!(px(&a, 5, 5), [0, 0, 0, 255], "the backdrop starts black");
    assert_eq!(px(&b, 5, 5), [255, 255, 255, 255], "and ends white");
    let (ca, cb) = (px(&a, 160, 120), px(&b, 160, 120));
    assert!(ca[0] > 150 && ca[2] < 40 && cb[2] > 150 && cb[0] < 40, "the chrome reflects red, then blue: {ca:?} → {cb:?}");
}

/// A point light's `range`: it fades smoothly to nothing there; without
/// one it lights near and far alike (today's look).
#[test]
fn point_lights_fade_out_at_their_range() {
    let floor = |range: Option<f64>| {
        let mut light = json!({"type": "point", "position": [40, 120, -60], "intensity": 1.0, "shadow": false});
        if let Some(r) = range {
            light["range"] = json!(r);
        }
        scene(320, 240, json!({"layers": [{"id": "wall", "kind": "mesh", "mesh": {"type": "plane", "size": [2000, 2000]}, "material": {"preset": "matte", "color": "#ffffff"},
            "transform": {"position": [160, 120]}}], "lights": [light], "environment": {"preset": "none"}}))
    };
    let (open, ranged) = (render(&floor(None), 0.0), render(&floor(Some(220.0)), 0.0));
    let (near, far) = ((60, 120), (300, 120));
    assert!(luma(px(&open, far.0, far.1)) > 0.5 * luma(px(&open, near.0, near.1)), "no range: far still lit");
    assert!(luma(px(&ranged, near.0, near.1)) > 0.8 * luma(px(&open, near.0, near.1)), "in range: as before");
    assert!(luma(px(&ranged, far.0, far.1)) < 3.0, "beyond range: dark ({:?})", px(&ranged, far.0, far.1));
}

#[test]
fn gpu_matches_cpu_area_lights() {
    let p = scene(480, 240, json!({"layers": [
        {"id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [2000, 2000]}, "material": {"preset": "matte", "color": "#d8d0c0"}, "transform": {"position": [240, 200], "rotation_x": 90}},
        {"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "material": {"preset": "plastic", "color": "#c03030", "roughness": 0.3}, "transform": {"position": [150, 140]}},
        {"id": "cube", "kind": "mesh", "mesh": {"type": "box", "size": [80, 80], "depth": 80}, "material": {"preset": "brushed", "color": "#d0d4da"}, "transform": {"position": [330, 160], "rotation_y": 30}}
    ], "lights": [
        {"type": "area", "position": [0, -200, -300], "size": [400, 150], "intensity": 1.2},
        {"type": "area", "position": [500, 0, -200], "aim": [300, 160, 0], "size": [120, 120], "shape": "disc", "color": "#a0c8ff", "range": 900}
    ], "environment": {"preset": "night"}}));
    crate::tests::assert_gpu_matches_cpu_within(&p, &[0.0], 0.0002);
}

/// An environment image lights and is reflected: a chrome ball shows its
/// red sky above and blue ground below; GPU and CPU agree; a missing image
/// is reported, not silently dark.
#[test]
fn environment_images_light_and_reflect() {
    let dir = std::env::temp_dir().join(format!("lahza-envimg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hdr = dir.join("sky.hdr");
    crate::envmap::tests::write_hdr(&hdr, 0.0);
    let env = json!({"image": hdr.to_str().unwrap()});
    let p = sphere_in(json!({"preset": "metal", "color": "#ffffff", "roughness": 0}), json!([{"type": "ambient", "intensity": 0}]), env.clone());
    let img = render(&p, 0.0);
    let (top, bottom) = (px(&img, 160, 70), px(&img, 160, 170));
    assert!(top[0] > 150 && top[2] < 80, "sky above: {top:?}");
    assert!(bottom[2] > 150 && bottom[0] < 80, "ground below: {bottom:?}");
    let matte = sphere_in(json!({"preset": "matte", "color": "#ffffff"}), json!([{"type": "ambient", "intensity": 0}]), env);
    let m = render(&matte, 0.0);
    let (mt, mb) = (px(&m, 160, 50), px(&m, 160, 190));
    assert!(mt[0] > mt[2] && mb[2] > mb[0], "a matte ball takes the light from each side: {mt:?} {mb:?}");
    crate::tests::assert_gpu_matches_cpu_within(&p, &[0.0], 0.0002);
    crate::tests::assert_gpu_matches_cpu_within(&matte, &[0.0], 0.0002);
    let missing = sphere_in(json!({"preset": "matte"}), json!([]), json!({"image": dir.join("nope.hdr").to_str().unwrap()}));
    let c = Compiled::new(&missing);
    let mut r = cpu();
    r.render(&c, 0.0);
    assert!(r.dropped().iter().any(|d| d.contains("environment image")), "{:?}", r.dropped());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes a w × h checker (cells px, colours a and b) for map tests.
fn checker_png(path: &std::path::Path, w: u32, h: u32, cell: u32, a: [u8; 4], b: [u8; 4]) {
    image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(if (x / cell + y / cell) % 2 == 0 { a } else { b })).save(path).unwrap();
}

fn detail_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lahza-detail-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Noise varies a surface's colour (seeded: the same seed, the same
/// pattern; another seed, another), and a colour map tiles across UVs.
#[test]
fn noise_and_maps_colour_surfaces() {
    let lights = json!([{"type": "ambient", "intensity": 1}]);
    let env = json!({"preset": "none"});
    let noisy = |seed: u32| render(&sphere_in(json!({"preset": "flat", "color": "#ffffff", "noise": {"type": "fbm", "scale": 30, "seed": seed, "color": "#ff0000"}}), lights.clone(), env.clone()), 0.0);
    let (a, a2, b) = (noisy(1), noisy(1), noisy(2));
    assert_eq!(a, a2, "seeded");
    assert_ne!(a, b, "another seed, another pattern");
    let greens: Vec<u8> = (60..180u32).map(|x| px(&a, x, 120)[1]).collect();
    let (lo, hi) = (*greens.iter().min().unwrap(), *greens.iter().max().unwrap());
    assert!(hi - lo > 60, "the noise moves the colour toward red: green {lo}..{hi}");
    // A checker map on a plane facing the camera: 4 cells across at uv
    // scale 1, 8 at scale 2.
    let dir = detail_dir("map");
    let tex = dir.join("check.png");
    checker_png(&tex, 64, 64, 16, [255, 255, 255, 255], [0, 0, 0, 255]);
    let plane = |scale: f64| {
        render(&scene(320, 240, json!({"layers": [{"id": "p", "kind": "mesh", "mesh": {"type": "plane", "size": [200, 200]},
            "material": {"preset": "flat", "color": "#ffffff", "map": tex.to_str().unwrap(), "uv": {"scale": [scale, scale]}},
            "transform": {"position": [160, 120]}}]})), 0.0)
    };
    let changes = |img: &image::RgbaImage| (62..258u32).filter(|&x| (px(img, x, 121)[0] > 128) != (px(img, x + 1, 121)[0] > 128)).count();
    let (one, two) = (changes(&plane(1.0)), changes(&plane(2.0)));
    assert_eq!(one, 3, "four cells across");
    assert_eq!(two, 7, "eight at twice the tiling");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bump (a striped height map, or noise) bends the light on a flat wall
/// lit from the side; without it the wall is even.
#[test]
fn bumps_bend_the_light() {
    let dir = detail_dir("bump");
    let stripes = dir.join("stripes.png");
    image::RgbaImage::from_fn(64, 64, |x, _| {
        let v = (128.0 + 127.0 * (x as f64 / 64.0 * std::f64::consts::TAU * 4.0).sin()) as u8;
        image::Rgba([v, v, v, 255])
    })
    .save(&stripes)
    .unwrap();
    let wall = |material: serde_json::Value| {
        render(&scene(320, 240, json!({"layers": [{"id": "w", "kind": "mesh", "mesh": {"type": "plane", "size": [300, 220]}, "material": material, "transform": {"position": [160, 120]}}],
            "lights": [{"type": "directional", "from": [-1, 0, -0.4], "shadow": false}], "environment": {"preset": "none"}})), 0.0)
    };
    let spread = |img: &image::RgbaImage| {
        let v: Vec<f64> = (40..280u32).map(|x| luma(px(img, x, 120))).collect();
        v.iter().cloned().fold(0.0, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min)
    };
    let flat = spread(&wall(json!({"preset": "matte", "color": "#c0c0c0"})));
    let bumped = spread(&wall(json!({"preset": "matte", "color": "#c0c0c0", "bump_map": stripes.to_str().unwrap(), "bump": 20})));
    let noisy = spread(&wall(json!({"preset": "matte", "color": "#c0c0c0", "noise": {"scale": 20, "bump": 6}})));
    assert!(flat < 6.0, "even: {flat}");
    assert!(bumped > 40.0, "striped light: {bumped}");
    assert!(noisy > 20.0, "noisy light: {noisy}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Displacement reaches the silhouette: a displaced ball's outline is no
/// longer round.
#[test]
fn displacement_changes_the_silhouette() {
    let ball = |displace: serde_json::Value| {
        let mut l = json!({"id": "b", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "flat", "color": "#ffffff"}, "transform": {"position": [160, 120]}});
        if !displace.is_null() {
            l["displace"] = displace;
        }
        render(&scene(320, 240, json!({"layers": [l]})), 0.0)
    };
    let radii = |img: &image::RgbaImage| -> Vec<f64> {
        (0..36).map(|k| {
            let a = k as f64 * std::f64::consts::TAU / 36.0;
            (0..120).map(|r| r as f64).take_while(|r| {
                let (x, y) = ((160.0 + r * a.cos()) as u32, (120.0 + r * a.sin()) as u32);
                px(img, x, y)[0] > 128
            }).count() as f64
        }).collect()
    };
    let spread = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
    let (round, rock) = (radii(&ball(serde_json::Value::Null)), radii(&ball(json!({"type": "fbm", "scale": 40, "amount": 12, "seed": 3}))));
    assert!(spread(&round) <= 3.0, "round: {round:?}");
    assert!(spread(&rock) >= 8.0, "rocky: {rock:?}");
}

#[test]
fn gpu_matches_cpu_surface_detail() {
    let dir = detail_dir("parity");
    let (tex, nrm) = (dir.join("check.png"), dir.join("normal.png"));
    checker_png(&tex, 32, 32, 8, [255, 200, 120, 255], [40, 60, 200, 255]);
    image::RgbaImage::from_fn(32, 32, |x, y| image::Rgba([(128.0 + 90.0 * ((x as f64) * 0.6).sin()) as u8, (128.0 + 90.0 * ((y as f64) * 0.6).cos()) as u8, 220, 255])).save(&nrm).unwrap();
    let p = scene(640, 240, json!({"layers": [
        {"id": "rock", "kind": "mesh", "mesh": {"type": "sphere", "radius": 60}, "displace": {"type": "cellular", "scale": 30, "amount": 8},
         "material": {"preset": "matte", "color": "#a08060", "noise": {"type": "fbm", "scale": 15, "color": "#403020", "roughness": 0.3, "bump": 3}}, "transform": {"position": [90, 120]}},
        {"id": "tile", "kind": "mesh", "mesh": {"type": "box", "size": [110, 110], "depth": 110}, "material": {"preset": "plastic", "map": tex.to_str().unwrap(), "normal_map": nrm.to_str().unwrap(), "uv": {"scale": [2, 2], "rotation": 15}},
         "transform": {"position": [260, 120], "rotation_y": 30, "rotation_x": 20}},
        {"id": "can", "kind": "mesh", "mesh": {"type": "cylinder", "radius": 50, "height": 120}, "material": {"preset": "metal", "color": "#d0d0d0", "bump_map": tex.to_str().unwrap(), "bump": 4, "noise": {"type": "value", "scale": 25, "space": "world", "roughness": 0.2}},
         "transform": {"position": [440, 120], "rotation_x": 20}},
        {"id": "ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 50, "tube": 16}, "material": {"preset": "glossy", "color": "#ffffff", "map": tex.to_str().unwrap()}, "transform": {"position": [580, 120], "rotation_x": 50}}
    ]}));
    crate::tests::assert_gpu_matches_cpu_within(&p, &[0.0], 0.0005);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tone mapping and exposure: AgX and filmic roll a hot light off into
/// a smooth falloff instead of the standard clip, exposure brightens; GPU
/// and CPU agree. Bloom spreads what is bright beyond its edge.
#[test]
fn looks_tone_map_and_bloom() {
    let shot = |look: serde_json::Value| {
        let mut s = json!({"layers": [{"id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 80}, "material": {"preset": "matte", "color": "#ff8040"}, "transform": {"position": [160, 120]}}],
            "lights": [{"type": "directional", "from": [0, 0, -1], "intensity": 3, "shadow": false}], "environment": {"preset": "none"}});
        if !look.is_null() {
            s["look"] = look;
        }
        scene(320, 240, s)
    };
    let (standard, agx, filmic) = (render(&shot(serde_json::Value::Null), 0.0), render(&shot(json!({"tone": "agx"})), 0.0), render(&shot(json!({"tone": "filmic"})), 0.0));
    let c = |img: &image::RgbaImage| px(img, 160, 120);
    assert_ne!(c(&standard), c(&agx));
    assert_ne!(c(&standard), c(&filmic));
    // AgX desaturates the hot centre toward white more than standard.
    let sat = |p: [u8; 4]| p[0] as f64 - p[2] as f64;
    assert!(sat(c(&agx)) < sat(c(&standard)), "AgX {:?} vs standard {:?}", c(&agx), c(&standard));
    // Exposure: −2 stops is darker.
    let dark = render(&shot(json!({"exposure": -2})), 0.0);
    assert!(luma(c(&dark)) < luma(c(&standard)) - 30.0);
    // Bloom: light beyond the ball's edge.
    let bloom = render(&shot(json!({"bloom": {"threshold": 0.5, "radius": 30, "intensity": 1}})), 0.0);
    let outside = |img: &image::RgbaImage| luma(px(img, 160 + 90, 120));
    assert!(outside(&bloom) > outside(&standard) + 10.0, "bloom spreads: {} vs {}", outside(&bloom), outside(&standard));
    for look in [json!({"tone": "agx", "exposure": 0.5}), json!({"tone": "filmic"}), json!({"bloom": {"threshold": 0.6}})] {
        crate::tests::assert_gpu_matches_cpu_within(&shot(look), &[0.0], 0.0005);
    }
}
