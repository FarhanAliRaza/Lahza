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
