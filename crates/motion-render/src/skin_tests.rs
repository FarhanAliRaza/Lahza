//! Skinned meshes: the import, the pose, the CPU's skinning against a
//! pose worked by hand, and the GPU's against the CPU's.

use super::*;
use lahza_motion_core::compile::{Content, LayerState, MeshState};
use lahza_motion_core::document::{Project, Scene};
use lahza_motion_core::time::Fps;
use serde_json::json;

/// The test arm imported into a 640×480 scene; returns the project and
/// its folder (the model's path is absolute: renderers need no root).
fn arm(name: &str, at: f64) -> (Project, std::path::PathBuf, model::ImportReport) {
    let dir = std::env::temp_dir().join(format!("lahza-skin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = model::write_test_arm(&dir).unwrap();
    let file = model::open(&path).unwrap();
    let src = path.to_string_lossy().into_owned();
    let (mut layer, report) = model::to_layers(&file, &model::Import { id: "arm", src: &src, unit: 100.0, at, texture_dir: &dir, texture_src: "tex", sequence: None }).unwrap();
    layer["transform"] = json!({"position": [300, 420]});
    let mut p = Project::new(640, 480, Fps::whole(30), 2.0);
    p.settings.background = lahza_motion_core::value::Color::parse("#101014").unwrap();
    let scene: Scene = serde_json::from_value(json!({"id": "s1", "camera": {"orbit": [20, 8]}, "layers": [layer]})).unwrap();
    p.scenes.push(scene);
    (p, dir, report)
}

fn mesh_state(layers: &[LayerState]) -> Option<MeshState> {
    for l in layers {
        match &l.content {
            Content::Mesh(m) if m.skin.is_some() => return Some((**m).clone()),
            Content::Group(g) => {
                if let Some(m) = mesh_state(g) {
                    return Some(m);
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn an_import_makes_joint_layers_and_names_them_in_the_skin() {
    let (p, dir, report) = arm("import", 0.0);
    assert_eq!((report.skinned, report.meshes, report.animated_nodes), (1, 1, 1), "{report:?}");
    let doc = serde_json::to_value(&p.scenes[0]).unwrap().to_string();
    assert!(doc.contains(r#""skin":{"joints":["arm-root","arm-mid","arm-tip"]}"#), "{doc}");
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    // A joint that is no layer is an error that names it.
    let mut broken = p.clone();
    let text = serde_json::to_string(&broken.scenes[0]).unwrap().replace(r#""arm-mid","arm-tip"]"#, r#""arm-mid","elbow"]"#);
    broken.scenes[0] = serde_json::from_str(&text).unwrap();
    let errors = Compiled::new(&broken).errors;
    assert!(errors.iter().any(|e| e.contains("skin.joints: 'elbow' is no layer of the scene")), "{errors:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_pose_is_the_bind_at_rest_and_bends_as_worked_by_hand() {
    let (p, dir, _) = arm("pose", 0.0);
    let c = Compiled::new(&p);
    let mut files = model::Files::default();
    let src = dir.join("arm.gltf").to_string_lossy().into_owned();
    let data = model::load_skin(&mut files, None, &src, 0, 0, 0, 100.0).unwrap();
    let bind = model::load_mesh(&mut files, None, &src, 0, 0, 100.0).unwrap();
    assert_eq!((data.inverse_bind.len(), data.joints.len()), (3, bind.positions.len()));
    // At rest every J · B only undoes the mesh node's own 30 px aside (a
    // skinned mesh is placed by its joints, not its node): the layers'
    // transforms (the import's) and the inverse binds (the loader's)
    // agree on axes and units.
    let rest = mesh_state(&c.frame(0.0).layers).unwrap();
    let pose = rest.skin.as_ref().unwrap();
    assert_eq!(pose.joints.len(), 3);
    for m in lahza_motion_core::skin::matrices(&data, &pose.joints) {
        for r in 0..3 {
            for k in 0..4 {
                let want = if r == k { 1.0 } else if (r, k) == (0, 3) { -30.0 } else { 0.0 };
                assert!((m[r][k] - want).abs() < 1e-4, "{m:?}");
            }
        }
    }
    // At 1 s the middle joint has turned a quarter about glTF's z. By
    // hand, in glTF: the tip's corner (0.65, 2.2, 0.15) is (0.15, 1, 0.15)
    // from the joint at (0.5, 1.2, 0); turned, (−1, 0.15, 0.15); so it is
    // at (−0.5, 1.35, 0.15), which is (−0.8, 1.35, 0.15) from the mesh's
    // node: (−80, −135, −15) px in the mesh layer.
    let bent = mesh_state(&c.frame(1.0).layers).unwrap();
    let ms = lahza_motion_core::skin::matrices(&data, &bent.skin.as_ref().unwrap().joints);
    let skinned = lahza_motion_core::skin::skin(&bind, &data, &ms);
    let corner = bind.positions.iter().position(|v| (v[0] - 65.0).abs() < 1e-3 && (v[1] + 220.0).abs() < 1e-3 && (v[2] + 15.0).abs() < 1e-3).expect("the tip's corner");
    let got = skinned.positions[corner];
    assert!((got[0] + 80.0).abs() < 0.05 && (got[1] + 135.0).abs() < 0.05 && (got[2] + 15.0).abs() < 0.05, "{got:?}");
    // The foot of the column has not moved (but for the node's 30 px).
    let foot = bind.positions.iter().position(|v| (v[1] + 20.0).abs() < 1e-3).unwrap();
    assert!((0..3).all(|k| (skinned.positions[foot][k] - bind.positions[foot][k] + if k == 0 { 30.0 } else { 0.0 }).abs() < 1e-3));
    // Halfway through the blend a point is between its two joints' places.
    let mid = bind.positions.iter().position(|v| (v[1] + 120.0).abs() < 1e-3 && data.weights[0][0] >= 0.0).unwrap();
    assert!((data.weights[mid][0] - 0.5).abs() < 1e-3, "{:?}", data.weights[mid]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_cpu_draws_the_bind_pose_as_the_unskinned_mesh_and_the_bend_differently() {
    let (p, dir, _) = arm("cpu", 0.0);
    let c = Compiled::new(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let mut r = Renderer::new(None, RenderOptions { backend: Backend::Cpu, ..Default::default() });
    let rest = r.render(&c, 0.0);
    let bent = r.render(&c, 1.0);
    // The same project without the skin: the mesh where its node puts it,
    // 30 px aside of where its joints do.
    let text = serde_json::to_string(&p.scenes[0]).unwrap().replace(r#""skin":{"joints":["arm-root","arm-mid","arm-tip"]},"#, "");
    let mut plain = p.clone();
    plain.scenes[0] = serde_json::from_str(&text).unwrap();
    // (Move the node back so both stand in one place.)
    let text = serde_json::to_string(&plain.scenes[0]).unwrap().replace(r#""position":[30.0,-0.0]"#, r#""position":[0.0,0.0]"#);
    plain.scenes[0] = serde_json::from_str(&text).unwrap();
    let unskinned = Renderer::new(None, RenderOptions { backend: Backend::Cpu, ..Default::default() }).render(&Compiled::new(&plain), 0.0);
    let differ = |a: &image::RgbaImage, b: &image::RgbaImage| a.pixels().zip(b.pixels()).filter(|(x, y)| (0..4).any(|k| x.0[k].abs_diff(y.0[k]) > 3)).count();
    assert!(differ(&rest, &unskinned) < 20, "{} pixels differ between the bind pose and the unskinned mesh", differ(&rest, &unskinned));
    assert!(differ(&rest, &bent) > 3000, "the bend moves the picture: {} pixels", differ(&rest, &bent));
    // The bent arm's tip is left of the column's foot and no higher than
    // the joint: bright pixels reach farther left than at rest.
    let left = |img: &image::RgbaImage| img.enumerate_pixels().filter(|(_, _, p)| p.0[0] > 120).map(|(x, _, _)| x).min().unwrap();
    assert!(left(&bent) + 60 < left(&rest), "left edge {} → {}", left(&rest), left(&bent));
    if let Ok(out) = std::env::var("LAHZA_SKIN_OUT") {
        rest.save(format!("{out}/arm-rest.png")).unwrap();
        bent.save(format!("{out}/arm-bent.png")).unwrap();
        r.render(&c, 0.5).save(format!("{out}/arm-half.png")).unwrap();
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Run with a Vulkan device (LAHZA_MOTION_SOFTWARE_GPU=1 accepts
/// llvmpipe); skips otherwise.
#[test]
fn gpu_skinning_matches_cpu_skinning() {
    let (p, dir, _) = arm("gpu", 0.0);
    // Rest, mid-bend (a new pose every frame), the full bend, and rest
    // again after it (the pool's bind mesh is reused).
    tests::assert_gpu_matches_cpu_within(&p, &[0.0, 0.3, 0.7, 1.0, 1.5], 0.002);
    std::fs::remove_dir_all(&dir).ok();
}

/// A GPU renderer whose meshes go through the hardware visibility pass
/// and the mesh pool (as `mesh_tests::vis_gpu`).
fn pool_gpu() -> Option<Renderer> {
    let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
    crate::gpu::TEST_VIS_AT_4X.with(|t| t.set(true));
    let r = Renderer::new(None, RenderOptions { backend: Backend::Gpu, allow_software_gpu: allow, ..Default::default() });
    crate::gpu::TEST_VIS_AT_4X.with(|t| t.set(false));
    (r.is_gpu() && r.gpu.as_ref().is_some_and(|g| g.has_vis())).then_some(r)
}

fn difference(a: &image::RgbaImage, b: &image::RgbaImage) -> (f64, usize) {
    let (mut sum, mut over) = (0u64, 0usize);
    for (x, y) in a.pixels().zip(b.pixels()) {
        let mut worst = 0;
        for k in 0..4 {
            let d = x.0[k].abs_diff(y.0[k]);
            sum += d as u64;
            worst = worst.max(d);
        }
        over += (worst > 8) as usize;
    }
    (sum as f64 / (a.width() * a.height() * 4) as f64, over)
}

/// With a Vulkan device: the GPU skins the mesh in its pool
/// (`meshskin.wgsl`: the bind mesh uploaded once, a pose a frame), and
/// what it draws is what it draws from the CPU's skinned mesh. The path
/// tracer takes the skinned mesh from the pool.
#[test]
fn the_gpu_skins_in_its_pool_as_the_cpu_does_and_the_tracer_takes_it_from_there() {
    let (Some(mut gpu), Some(mut cpu)) = (pool_gpu(), pool_gpu()) else { return };
    // The second renderer skins on the CPU and uploads every pose's mesh.
    cpu.meshes.gpu_geometry = false;
    let (p, dir, _) = arm("pool", 0.0);
    let c = Compiled::new(&p);
    for (k, t) in [0.0, 0.3, 0.7, 1.0, 1.5, 0.3].into_iter().enumerate() {
        let (a, b) = (gpu.render(&c, t), cpu.render(&c, t));
        if let Ok(out) = std::env::var("LAHZA_SKIN_OUT") {
            a.save(format!("{out}/pool-gpu-{k}.png")).unwrap();
            b.save(format!("{out}/pool-cpu-{k}.png")).unwrap();
        }
        let (mean, over) = difference(&a, &b);
        // (Edges: a point's last bits differ where the GPU fuses a
        // multiply and an add.)
        assert!(mean < 0.02 && over < 40, "at {t}: mean difference {mean:.4}, {over} pixels apart");
        let shown = a.pixels().filter(|p| p.0[0] > 120).count();
        assert!(shown > 3000, "at {t}: {shown} pixels of the arm");
    }
    // The GPU made a mesh per pose it had not kept (1.5 s is 1 s's pose,
    // still there; the last 0.3 s again, its mesh let go after its frame)
    // and the bind mesh went up once; the other renderer uploaded a mesh
    // per pose.
    let ((made, uploads), (none, cpu_uploads)) = (gpu.gpu.as_ref().unwrap().pool_counts(), cpu.gpu.as_ref().unwrap().pool_counts());
    assert_eq!((made, uploads, none), (5, 1, 0), "meshes the GPU skinned, meshes uploaded");
    assert_eq!(cpu_uploads, 5);
    assert!(gpu.dropped().is_empty() && gpu.warnings().is_empty(), "{:?} {:?}", gpu.dropped(), gpu.warnings());
    if gpu.can_trace() {
        for r in [&mut gpu, &mut cpu] {
            r.set_path(Some(crate::pt::TraceOptions { spp: 16, bounces: 3, ..Default::default() }));
        }
        let (a, b) = (gpu.render(&c, 0.7), cpu.render(&c, 0.7));
        if let Ok(out) = std::env::var("LAHZA_SKIN_OUT") {
            a.save(format!("{out}/traced-gpu.png")).unwrap();
            b.save(format!("{out}/traced-cpu.png")).unwrap();
        }
        let (mean, _) = difference(&a, &b);
        assert!(mean < 1.0, "traced: mean difference {mean:.4}");
        let shown = a.pixels().filter(|p| p.0[0] > 100).count();
        assert!(shown > 3000, "traced: {shown} pixels of the arm");
        let (made, uploads) = gpu.gpu.as_ref().unwrap().pool_counts();
        assert_eq!((made, uploads), (6, 1), "the traced frame's mesh was skinned in the pool too");
        assert!(gpu.dropped().is_empty(), "{:?}", gpu.dropped());
    } else {
        eprintln!("no ray queries here: the tracer's part is not run");
    }
    std::fs::remove_dir_all(&dir).ok();
}
