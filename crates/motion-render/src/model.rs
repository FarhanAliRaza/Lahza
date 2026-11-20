//! glTF models (what Blender exports): meshes for the renderer, and a
//! description of the file's nodes, materials, textures, animation, cameras
//! and lights for `model_import`.
//!
//! glTF is in metres with y up and z toward the viewer; a layer's space is
//! px with y down and z away. A point (x, y, z) becomes (x, −y, −z) · unit
//! (px per metre): a half turn about x, so handedness and winding hold.

use lahza_motion_core::mesh::{tag, Mesh, PART_SIDE};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A parsed file and its buffers.
pub struct File {
    pub document: gltf::Document,
    pub buffers: Vec<gltf::buffer::Data>,
}

/// Parsed files by path (a sequence's frames come and go: the cache keeps
/// the last few).
#[derive(Default)]
pub struct Files {
    files: HashMap<PathBuf, (std::time::SystemTime, Arc<File>)>,
}

fn resolve(root: Option<&Path>, src: &str) -> PathBuf {
    let p = Path::new(src);
    match root {
        Some(r) if p.is_relative() => r.join(p),
        _ => p.to_path_buf(),
    }
}

/// Opens a .glb/.gltf with its buffers (not its images).
pub fn open(path: &Path) -> Result<File, String> {
    let gltf = gltf::Gltf::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = path.parent();
    let buffers = gltf::import_buffers(&gltf.document, base, gltf.blob.clone()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(File { document: gltf.document, buffers })
}

impl Files {
    fn get(&mut self, path: &Path) -> Result<Arc<File>, String> {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some((when, f)) = self.files.get(path) {
            if *when == modified {
                return Ok(f.clone());
            }
        }
        if self.files.len() >= 16 {
            self.files.clear();
        }
        let f = Arc::new(open(path)?);
        self.files.insert(path.to_path_buf(), (modified, f.clone()));
        Ok(f)
    }
}

/// glTF (metres, y up, z toward the viewer) → layer px (y down, z away).
pub fn to_layer(p: [f32; 3], unit: f64) -> [f32; 3] {
    let u = unit as f32;
    [p[0] * u, -p[1] * u, -p[2] * u]
}

/// One primitive of `mesh` in `src` (relative to `root`) as an engine mesh.
pub fn load_mesh(files: &mut Files, root: Option<&Path>, src: &str, mesh: u32, primitive: u32, unit: f64) -> Result<Mesh, String> {
    let path = resolve(root, src);
    let file = files.get(&path)?;
    let m = file.document.meshes().nth(mesh as usize).ok_or_else(|| format!("{src}: no mesh {mesh}"))?;
    let p = m.primitives().nth(primitive as usize).ok_or_else(|| format!("{src}: mesh {mesh} has no primitive {primitive}"))?;
    if p.mode() != gltf::mesh::Mode::Triangles {
        return Err(format!("{src}: mesh {mesh} primitive {primitive} is not triangles ({:?})", p.mode()));
    }
    let reader = p.reader(|b| file.buffers.get(b.index()).map(|d| &d.0[..]));
    let positions: Vec<[f32; 3]> = reader.read_positions().ok_or_else(|| format!("{src}: mesh {mesh} has no positions"))?.map(|v| to_layer(v, unit)).collect();
    let n = positions.len();
    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..n as u32).collect(),
    };
    let tris: Vec<[u32; 3]> = indices.chunks_exact(3).filter(|t| t.iter().all(|&i| (i as usize) < n)).map(|t| [t[0], t[1], t[2]]).collect();
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(ns) => ns.map(|v| to_layer(v, 1.0)).collect(),
        None => smooth_normals(&positions, &tris),
    };
    let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
        Some(tc) => tc.into_f32().collect(),
        None => vec![[0.0; 2]; n],
    };
    let count = tris.len();
    Ok(Mesh { positions, normals, tris, tags: vec![tag(PART_SIDE, 0); count], edges: vec![0b111; count], uvs })
}

/// Skin `skin` of `src` as primitive `primitive` of `mesh` takes it: each
/// point's joints and weights, and the joints' inverse bind matrices in
/// the layer's axes and px (C · B · C⁻¹ for C = `to_layer`).
pub fn load_skin(files: &mut Files, root: Option<&Path>, src: &str, mesh: u32, primitive: u32, skin: u32, unit: f64) -> Result<lahza_motion_core::skin::SkinData, String> {
    use lahza_motion_core::space::Mat4;
    let path = resolve(root, src);
    let file = files.get(&path)?;
    let m = file.document.meshes().nth(mesh as usize).ok_or_else(|| format!("{src}: no mesh {mesh}"))?;
    let p = m.primitives().nth(primitive as usize).ok_or_else(|| format!("{src}: mesh {mesh} has no primitive {primitive}"))?;
    let reader = p.reader(|b| file.buffers.get(b.index()).map(|d| &d.0[..]));
    let joints: Vec<[u16; 4]> = reader.read_joints(0).ok_or_else(|| format!("{src}: mesh {mesh} has no joints (JOINTS_0): it is not skinned"))?.into_u16().collect();
    let mut weights: Vec<[f32; 4]> = reader.read_weights(0).ok_or_else(|| format!("{src}: mesh {mesh} has no weights (WEIGHTS_0)"))?.into_f32().collect();
    // A second four a point, when the file has them (both or neither).
    let mut more = match (reader.read_joints(1), reader.read_weights(1)) {
        (Some(j), Some(w)) => Some((j.into_u16().collect::<Vec<[u16; 4]>>(), w.into_f32().collect::<Vec<[f32; 4]>>())),
        (None, None) => None,
        _ => return Err(format!("{src}: mesh {mesh} has one of JOINTS_1 and WEIGHTS_1 without the other")),
    };
    if more.as_ref().is_some_and(|(j, w)| j.len() != joints.len() || w.len() != joints.len()) {
        return Err(format!("{src}: mesh {mesh}'s JOINTS_1 are not as many as its JOINTS_0"));
    }
    // (Nothing weighted in it: four a point after all.)
    if more.as_ref().is_some_and(|(_, w)| w.iter().all(|w| w.iter().all(|v| *v == 0.0))) {
        more = None;
    }
    if p.get(&gltf::Semantic::Joints(2)).is_some() {
        return Err(format!("{src}: mesh {mesh} has more than eight joints a point (JOINTS_2): export with eight or fewer"));
    }
    for (i, w) in weights.iter_mut().enumerate() {
        let second = more.as_mut().map(|(_, w)| &mut w[i]);
        let sum: f32 = w.iter().sum::<f32>() + second.as_ref().map_or(0.0, |w| w.iter().sum());
        if sum > 0.0 {
            *w = w.map(|v| v / sum);
            if let Some(w) = second {
                *w = w.map(|v| v / sum);
            }
        }
    }
    let s = file.document.skins().nth(skin as usize).ok_or_else(|| format!("{src}: no skin {skin}"))?;
    let count = s.joints().count();
    let sreader = s.reader(|b| file.buffers.get(b.index()).map(|d| &d.0[..]));
    let sign = [1.0, -1.0, -1.0];
    let inverse_bind: Vec<Mat4> = match sreader.read_inverse_bind_matrices() {
        // glTF's matrices are columns: g[c][r].
        Some(ms) => ms
            .map(|g| {
                let mut out = Mat4::IDENTITY;
                for r in 0..3 {
                    for c in 0..3 {
                        out.0[r * 4 + c] = sign[r] * g[c][r] as f64 * sign[c];
                    }
                    out.0[r * 4 + 3] = sign[r] * g[3][r] as f64 * unit;
                }
                out
            })
            .collect(),
        None => vec![Mat4::IDENTITY; count],
    };
    if inverse_bind.len() != count {
        return Err(format!("{src}: skin {skin} has {count} joints and {} inverse bind matrices", inverse_bind.len()));
    }
    Ok(lahza_motion_core::skin::SkinData { joints, weights, more, inverse_bind })
}

/// Per-vertex normals from the faces around each vertex.
fn smooth_normals(p: &[[f32; 3]], tris: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f32; 3]; p.len()];
    for t in tris {
        let [a, b, c] = t.map(|i| p[i as usize]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &i in t {
            for k in 0..3 {
                acc[i as usize][k] += n[k];
            }
        }
    }
    acc.into_iter()
        .map(|n| {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if l > 0.0 {
                [n[0] / l, n[1] / l, n[2] / l]
            } else {
                [0.0, 0.0, -1.0]
            }
        })
        .collect()
}

// ------------------------------------------------------------- import

/// glTF quaternion [x, y, z, w] → rotation matrix (rows), in glTF's axes.
fn quat_matrix(q: [f32; 4]) -> [[f64; 3]; 3] {
    let [x, y, z, w] = q.map(|v| v as f64);
    let n = (x * x + y * y + z * z + w * w).sqrt().max(1e-12);
    let (x, y, z, w) = (x / n, y / n, z / n, w / n);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// A glTF rotation as a layer's `rotation_x`, `rotation_y` and `rotation`
/// (degrees). In the layer's axes (glTF's turned half about x: M R M with
/// M = diag(1, −1, −1)) the layer applies Ry(rotation_y)·Rx(rotation_x)·
/// Rz(rotation) with the engine's matrices, which are, in textbook terms,
/// Ry(−b)·Rx(−a)·Rz(c).
pub fn euler(q: [f32; 4]) -> [f64; 3] {
    let g = quat_matrix(q);
    let m = [1.0, -1.0, -1.0];
    let r: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| m[i] * g[i][j] * m[j]));
    // Textbook YXZ: R = Ry(β)·Rx(α)·Rz(γ).
    let alpha = (-r[1][2]).clamp(-1.0, 1.0).asin();
    let (beta, gamma) = if alpha.cos().abs() > 1e-6 { (r[0][2].atan2(r[2][2]), r[1][0].atan2(r[1][1])) } else { ((-r[2][0]).atan2(r[0][0]), 0.0) };
    [-alpha.to_degrees(), -beta.to_degrees(), gamma.to_degrees()]
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Linear RGB → "#rrggbb" (sRGB).
fn hex(c: [f32; 3]) -> String {
    let enc = |v: f32| {
        let v = v.clamp(0.0, 1.0) as f64;
        let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    format!("#{:02x}{:02x}{:02x}", enc(c[0]), enc(c[1]), enc(c[2]))
}

/// What an import made, besides the layers.
#[derive(Default, Debug, serde::Serialize)]
pub struct ImportReport {
    pub nodes: usize,
    pub meshes: usize,
    pub textures: Vec<String>,
    pub animated_nodes: usize,
    /// Skinned meshes: their points follow the joints' layers.
    pub skinned: usize,
    /// Cameras and lights in the file (not imported yet: set the scene's
    /// camera and lights to match).
    pub cameras: Vec<String>,
    pub lights: Vec<String>,
    pub warnings: Vec<String>,
}

/// Options for `to_layers`.
pub struct Import<'a> {
    /// The layer id of the model's root.
    pub id: &'a str,
    /// The model's path as layers reference it (relative to the project),
    /// `{frame}` included for a sequence.
    pub src: &'a str,
    /// px per metre.
    pub unit: f64,
    /// Seconds the file's animation starts at.
    pub at: f64,
    /// Where textures are written, and their path as layers reference it.
    pub texture_dir: &'a Path,
    pub texture_src: &'a str,
    /// A sequence: (first frame, frames, fps): meshes show frame
    /// first + (t − at)·fps.
    pub sequence: Option<(u32, u32, f64)>,
}

/// The file's default scene as a root `three_d` comp: one comp per node
/// (its transform, keyed when the file animates it) holding one mesh layer
/// per primitive (with its material), and the node's children.
pub fn to_layers(file: &File, o: &Import) -> Result<(serde_json::Value, ImportReport), String> {
    use serde_json::json;
    let doc = &file.document;
    let mut report = ImportReport::default();
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or("the file has no scene")?;
    // Animation per node and path: (times, values, interpolation).
    let mut channels: HashMap<(usize, &'static str), (Vec<f32>, Vec<Vec<f32>>, bool)> = HashMap::new();
    for anim in doc.animations() {
        for ch in anim.channels() {
            let reader = ch.reader(|b| file.buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(times) = reader.read_inputs() else { continue };
            let times: Vec<f32> = times.collect();
            let step = ch.sampler().interpolation() == gltf::animation::Interpolation::Step;
            let cubic = ch.sampler().interpolation() == gltf::animation::Interpolation::CubicSpline;
            use gltf::animation::util::ReadOutputs;
            let values: Vec<Vec<f32>> = match reader.read_outputs() {
                Some(ReadOutputs::Translations(v)) => v.map(|x| x.to_vec()).collect(),
                Some(ReadOutputs::Scales(v)) => v.map(|x| x.to_vec()).collect(),
                Some(ReadOutputs::Rotations(r)) => r.into_f32().map(|x| x.to_vec()).collect(),
                _ => continue,
            };
            // Cubic splines hold [in tangent, value, out tangent] per key.
            let values = if cubic { values.chunks(3).filter_map(|c| c.get(1).cloned()).collect() } else { values };
            let path = match ch.target().property() {
                gltf::animation::Property::Translation => "translation",
                gltf::animation::Property::Rotation => "rotation",
                gltf::animation::Property::Scale => "scale",
                gltf::animation::Property::MorphTargetWeights => {
                    report.warnings.push("morph targets are not imported: export deforming meshes as a sequence".into());
                    continue;
                }
            };
            channels.insert((ch.target().node().index(), path), (times, values, step));
        }
    }
    let mut textures: HashMap<usize, String> = HashMap::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut unique = |base: String| -> String {
        let clean: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let n = ids.entry(clean.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { clean } else { format!("{clean}-{n}") }
    };
    struct Ctx<'c> {
        file: &'c File,
        o: &'c Import<'c>,
        channels: &'c HashMap<(usize, &'static str), (Vec<f32>, Vec<Vec<f32>>, bool)>,
        /// Layer ids by node, as the nodes are reached.
        ids: std::cell::RefCell<HashMap<usize, String>>,
    }
    let ctx = Ctx { file, o, channels: &channels, ids: Default::default() };
    fn texture(ctx: &Ctx, tex: gltf::texture::Texture, textures: &mut HashMap<usize, String>, report: &mut ImportReport) -> Option<String> {
        let image = tex.source();
        if let Some(p) = textures.get(&image.index()) {
            return Some(p.clone());
        }
        let (bytes, ext): (Vec<u8>, &str) = match image.source() {
            gltf::image::Source::View { view, mime_type } => {
                let data = ctx.file.buffers.get(view.buffer().index())?;
                let bytes = data.0.get(view.offset()..view.offset() + view.length())?.to_vec();
                (bytes, if mime_type.contains("jpeg") { "jpg" } else { "png" })
            }
            gltf::image::Source::Uri { uri, .. } => {
                report.warnings.push(format!("texture {uri}: external textures are not copied; embed them (export GLB)"));
                return None;
            }
        };
        std::fs::create_dir_all(ctx.o.texture_dir).ok()?;
        let name = format!("tex{}.{ext}", image.index());
        std::fs::write(ctx.o.texture_dir.join(&name), bytes).ok()?;
        let src = format!("{}/{name}", ctx.o.texture_src.trim_end_matches('/'));
        report.textures.push(src.clone());
        textures.insert(image.index(), src.clone());
        Some(src)
    }
    fn material(ctx: &Ctx, m: gltf::Material, textures: &mut HashMap<usize, String>, report: &mut ImportReport) -> serde_json::Value {
        let pbr = m.pbr_metallic_roughness();
        let base = pbr.base_color_factor();
        // (No clear coat: a material here starts from the glossy preset,
        // whose lacquer over a scanned rock mirrors the sun at a few points
        // of it, white specks that jump from frame to frame.)
        let mut out = json!({"color": hex([base[0], base[1], base[2]]), "metallic": round(pbr.metallic_factor() as f64), "roughness": round(pbr.roughness_factor() as f64), "clearcoat": 0.0});
        if base[3] < 0.999 && m.alpha_mode() == gltf::material::AlphaMode::Blend {
            out["opacity"] = json!(round(base[3] as f64));
        }
        if let Some(t) = pbr.base_color_texture().and_then(|i| texture(ctx, i.texture(), textures, report)) {
            out["map"] = json!(t);
        }
        if let Some(t) = m.normal_texture().and_then(|n| texture(ctx, n.texture(), textures, report)) {
            out["normal_map"] = json!(t);
        }
        // One image: roughness in green, metallic in blue (scaling the factors).
        if let Some(t) = pbr.metallic_roughness_texture().and_then(|i| texture(ctx, i.texture(), textures, report)) {
            out["roughness_map"] = json!(t);
            out["metallic_map"] = json!(t);
        }
        // glTF's emission is the factor times the texture (sRGB), as
        // emissive × emissive_map.
        let e = m.emissive_factor();
        if e.iter().any(|v| *v > 0.0) {
            out["emissive"] = json!(hex(e));
            out["emissive_strength"] = json!(round(m.emissive_strength().unwrap_or(1.0) as f64));
            if let Some(t) = m.emissive_texture().and_then(|i| texture(ctx, i.texture(), textures, report)) {
                out["emissive_map"] = json!(t);
            }
        }
        // What glTF has no field for travels in the material's extras, under
        // "lahza" (tools/blender/lahza_export.py writes it): subsurface
        // scattering, its scale in metres.
        if let Some(l) = m.extras().as_ref().and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok()).and_then(|v| v.get("lahza").cloned()) {
            for k in ["subsurface", "subsurface_radius", "subsurface_anisotropy"] {
                if let Some(x) = l.get(k) {
                    out[k] = x.clone();
                }
            }
            if let Some(sc) = l.get("subsurface_scale_m").and_then(serde_json::Value::as_f64) {
                out["subsurface_scale"] = json!(round(sc * ctx.o.unit));
            }
        }
        if let Some(t) = m.transmission() {
            if t.transmission_factor() > 0.0 {
                out["transmission"] = json!(round(t.transmission_factor() as f64));
                out["ior"] = json!(round(m.ior().unwrap_or(1.5) as f64));
            }
        }
        out
    }
    fn keys<T: serde::Serialize>(at: f64, times: &[f32], step: bool, value: impl Fn(usize) -> T) -> serde_json::Value {
        let ease = if step { "hold" } else { "linear" };
        json!({"keys": times.iter().enumerate().map(|(k, t)| json!({"at": round(at + *t as f64), "v": value(k), "ease": ease})).collect::<Vec<_>>()})
    }
    fn node(ctx: &Ctx, n: gltf::Node, unique: &mut dyn FnMut(String) -> String, textures: &mut HashMap<usize, String>, report: &mut ImportReport) -> serde_json::Value {
        let o = ctx.o;
        report.nodes += 1;
        let id = unique(format!("{}-{}", o.id, n.name().map(str::to_string).unwrap_or_else(|| format!("node{}", n.index()))));
        ctx.ids.borrow_mut().insert(n.index(), id.clone());
        let (t, r, s) = n.transform().decomposed();
        let u = o.unit;
        let mut tr = serde_json::Map::new();
        let pos = |t: &[f32]| json!([round(t[0] as f64 * u), round(-t[1] as f64 * u)]);
        let depth = |t: &[f32]| round(-t[2] as f64 * u);
        let rot = |q: &[f32]| euler([q[0], q[1], q[2], q[3]]);
        let scale_xy = |s: &[f32]| json!([round(s[0] as f64), round(s[1] as f64)]);
        let scale_z = |s: &[f32]| round(s[2] as f64 / ((s[0].abs() + s[1].abs()) as f64 / 2.0).max(1e-6));
        let anim = |path| ctx.channels.get(&(n.index(), path));
        if let Some((times, values, step)) = anim("translation") {
            tr.insert("position".into(), keys(o.at, times, *step, |k| pos(&values[k])));
            tr.insert("z".into(), keys(o.at, times, *step, |k| depth(&values[k])));
        } else {
            tr.insert("position".into(), pos(&t));
            tr.insert("z".into(), json!(depth(&t)));
        }
        if let Some((times, values, step)) = anim("rotation") {
            // Unwrap so a turn keeps going instead of jumping back 360°.
            let mut eul: Vec<[f64; 3]> = values.iter().map(|q| rot(q)).collect();
            for k in 1..eul.len() {
                for a in 0..3 {
                    while eul[k][a] - eul[k - 1][a] > 180.0 {
                        eul[k][a] -= 360.0;
                    }
                    while eul[k][a] - eul[k - 1][a] < -180.0 {
                        eul[k][a] += 360.0;
                    }
                }
            }
            for (a, name) in ["rotation_x", "rotation_y", "rotation"].iter().enumerate() {
                tr.insert((*name).into(), keys(o.at, times, *step, |k| round(eul[k][a])));
            }
        } else {
            let e = rot(&r);
            for (a, name) in ["rotation_x", "rotation_y", "rotation"].iter().enumerate() {
                if e[a].abs() > 1e-6 {
                    tr.insert((*name).into(), json!(round(e[a])));
                }
            }
        }
        if let Some((times, values, step)) = anim("scale") {
            tr.insert("scale".into(), keys(o.at, times, *step, |k| scale_xy(&values[k])));
            tr.insert("scale_z".into(), keys(o.at, times, *step, |k| scale_z(&values[k])));
        } else if s != [1.0, 1.0, 1.0] {
            tr.insert("scale".into(), scale_xy(&s));
            tr.insert("scale_z".into(), json!(scale_z(&s)));
        }
        if ctx.channels.keys().any(|(i, _)| *i == n.index()) {
            report.animated_nodes += 1;
        }
        let mut children = Vec::new();
        if let Some(m) = n.mesh() {
            for p in m.primitives() {
                report.meshes += 1;
                let bb = p.bounding_box();
                let (lo, hi) = (to_layer(bb.min, u), to_layer(bb.max, u));
                let bounds = [[0, 1, 2].map(|k| round(lo[k].min(hi[k]) as f64)), [0, 1, 2].map(|k| round(lo[k].max(hi[k]) as f64))];
                let mut mesh = json!({"type": "gltf", "src": o.src, "mesh": m.index(), "primitive": p.index(), "unit": u, "bounds": bounds});
                if let Some((first, frames, fps)) = o.sequence {
                    let last = first + frames.saturating_sub(1);
                    mesh["frame"] = json!({"keys": [{"at": round(o.at), "v": first}, {"at": round(o.at + frames.saturating_sub(1) as f64 / fps), "v": last as f64 + 0.999, "ease": "linear"}]});
                }
                let mut layer = json!({"id": unique(format!("{id}-p{}", p.index())), "kind": "mesh", "mesh": mesh,
                                       "material": material(ctx, p.material(), textures, report)});
                // A skin: its joints' nodes for now, their layers once
                // every node has its id.
                if let (Some(skin), true) = (n.skin(), p.get(&gltf::Semantic::Joints(0)).is_some()) {
                    report.skinned += 1;
                    layer["skin"] = json!({"index": skin.index(), "joints": skin.joints().map(|j| j.index()).collect::<Vec<_>>()});
                }
                children.push(layer);
            }
        }
        if let Some(c) = n.camera() {
            report.cameras.push(n.name().unwrap_or(c.name().unwrap_or("camera")).to_string());
        }
        if let Some(l) = n.light() {
            let kind = match l.kind() {
                gltf::khr_lights_punctual::Kind::Directional => "sun",
                gltf::khr_lights_punctual::Kind::Point => "point",
                gltf::khr_lights_punctual::Kind::Spot { .. } => "spot",
            };
            report.lights.push(format!("{} ({kind})", n.name().unwrap_or("light")));
        }
        for c in n.children() {
            children.push(node(ctx, c, unique, textures, report));
        }
        json!({"id": id, "kind": "comp", "three_d": true, "transform": tr, "layers": children})
    }
    let mut roots = Vec::new();
    for n in scene.nodes() {
        roots.push(node(&ctx, n, &mut unique, &mut textures, &mut report));
    }
    // Skins name their joints' layers.
    fn name_joints(v: &mut serde_json::Value, ids: &HashMap<usize, String>, missing: &mut bool) {
        if let Some(joints) = v.get_mut("skin").and_then(|s| s.get_mut("joints")).and_then(serde_json::Value::as_array_mut) {
            for j in joints {
                match j.as_u64().and_then(|i| ids.get(&(i as usize))) {
                    Some(id) => *j = json!(id),
                    None => *missing = true,
                }
            }
        }
        if let Some(layers) = v.get_mut("layers").and_then(serde_json::Value::as_array_mut) {
            for l in layers {
                name_joints(l, ids, missing);
            }
        }
    }
    let mut missing = false;
    let ids = ctx.ids.into_inner();
    for r in &mut roots {
        name_joints(r, &ids, &mut missing);
    }
    if missing {
        return Err("a skin's joints are outside the file's scene: export the armature with the mesh".into());
    }
    Ok((json!({"id": o.id, "kind": "comp", "three_d": true, "layers": roots}), report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lahza_motion_core::space::Mat4;

    /// The layer's rotation from `euler` turns points as the glTF rotation
    /// does, once both are in the layer's axes.
    #[test]
    fn rotations_carry_over_exactly() {
        for q in [[0.0, 0.2474, 0.0, 0.9689], [0.3, -0.2, 0.5, 0.78], [0.7071, 0.0, 0.0, 0.7071], [0.1, 0.9, -0.2, 0.3]] {
            let n = (q.iter().map(|v: &f32| v * v).sum::<f32>()).sqrt();
            let q = q.map(|v| v / n);
            let [a, b, c] = euler(q);
            let m = Mat4::rotate_y(b).then(Mat4::rotate_x(a)).then(Mat4::rotate_z(c));
            let g = quat_matrix(q);
            for p in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.3, -0.5, 0.8]] {
                // glTF: rotate, then into layer axes; layer: into axes, then rotate.
                let rg: [f64; 3] = std::array::from_fn(|i| (0..3).map(|j| g[i][j] * p[j]).sum());
                let want = [rg[0], -rg[1], -rg[2]];
                let pl = [p[0], -p[1], -p[2]];
                let mm = m.0;
                let got: [f64; 3] = std::array::from_fn(|i| (0..3).map(|j| mm[i * 4 + j] * pl[j]).sum());
                for k in 0..3 {
                    assert!((got[k] - want[k]).abs() < 1e-4, "q {q:?}: {got:?} vs {want:?}");
                }
            }
        }
    }
}

/// A skinned test model written to `dir/arm.gltf` (its buffer inside): a square
/// column 2 m tall on three joints in a chain (at y = 0, 1 and 2 above a
/// root at (0.5, 0.2, 0)), its points weighted smoothly from joint to
/// joint, with an animation that turns the middle joint a quarter about z
/// over one second. The mesh's own node is moved 0.3 m aside (glTF says a
/// skinned mesh's node does not place it). For tests and examples: there
/// is nothing to download.
#[doc(hidden)]
pub fn write_test_arm(dir: &Path) -> Result<PathBuf, String> {
    write_test_arm_as(dir, false, false)
}

/// `write_test_arm`, and: `eight`: a point's second joint and weight are
/// in JOINTS_1 and WEIGHTS_1 (the same skin, said with eight joints a
/// point); `apart`: its buffer is a file beside it, `arm.bin`.
#[doc(hidden)]
pub fn write_test_arm_as(dir: &Path, eight: bool, apart: bool) -> Result<PathBuf, String> {
    use serde_json::json;
    let (root, half, rings) = ([0.5f32, 0.2, 0.0], 0.15f32, 17usize);
    let mut pos: Vec<[f32; 3]> = Vec::new();
    let mut nor: Vec<[f32; 3]> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    // Four faces, each a strip of its own (flat shading).
    let corners = [[-1.0f32, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
    for f in 0..4 {
        let (a, b) = (corners[f], corners[(f + 1) % 4]);
        let n = [(a[0] + b[0]) / 2.0, 0.0, (a[1] + b[1]) / 2.0];
        let first = pos.len() as u32;
        for r in 0..rings {
            let y = 2.0 * r as f32 / (rings - 1) as f32;
            for c in [a, b] {
                pos.push([root[0] + c[0] * half, root[1] + y, root[2] + c[1] * half]);
                nor.push(n);
            }
        }
        for r in 0..rings as u32 - 1 {
            let k = first + r * 2;
            // Outward as glTF winds (counter-clockwise seen from outside).
            idx.extend_from_slice(&[k, k + 2, k + 1, k + 1, k + 2, k + 3]);
        }
    }
    for (y, n, flip) in [(0.0f32, [0.0f32, -1.0, 0.0], false), (2.0, [0.0, 1.0, 0.0], true)] {
        let first = pos.len() as u32;
        for c in corners {
            pos.push([root[0] + c[0] * half, root[1] + y, root[2] + c[1] * half]);
            nor.push(n);
        }
        idx.extend_from_slice(&if flip { [first, first + 2, first + 1, first, first + 3, first + 2] } else { [first, first + 1, first + 2, first, first + 2, first + 3] });
    }
    // Joints at heights 0, 1, 2: a point's weight slides from one to the
    // next over the half metre around the joint between them.
    let mut joints: Vec<[u16; 4]> = Vec::new();
    let mut weights: Vec<[f32; 4]> = Vec::new();
    let (mut joints1, mut weights1): (Vec<[u16; 4]>, Vec<[f32; 4]>) = (Vec::new(), Vec::new());
    for p in &pos {
        let y = p[1] - root[1];
        let (j, t) = if y < 1.5 { (0u16, ((y - 0.75) / 0.5).clamp(0.0, 1.0)) } else { (1u16, ((y - 1.75) / 0.25).clamp(0.0, 1.0)) };
        joints.push([j, if eight { 0 } else { j + 1 }, 0, 0]);
        weights.push([1.0 - t, if eight { 0.0 } else { t }, 0.0, 0.0]);
        joints1.push([j + 1, 0, 0, 0]);
        weights1.push([t, 0.0, 0.0, 0.0]);
    }
    // Inverse binds (columns): each joint's world is a translation.
    let ibm: Vec<[f32; 16]> = (0..3).map(|j| [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -root[0], -(root[1] + j as f32), -root[2], 1.0]).collect();
    let times = [0.0f32, 0.5, 1.0];
    let turns: Vec<[f32; 4]> = [0.0f32, 45.0, 90.0].iter().map(|d| [0.0, 0.0, (d.to_radians() / 2.0).sin(), (d.to_radians() / 2.0).cos()]).collect();
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut view = |bytes: &[u8], bin: &mut Vec<u8>| {
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len()}));
        bin.extend_from_slice(bytes);
        views.len() - 1
    };
    let f32s = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let v_pos = view(&f32s(&pos.concat()), &mut bin);
    let v_nor = view(&f32s(&nor.concat()), &mut bin);
    let v_joints = view(&joints.concat().iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(), &mut bin);
    let v_weights = view(&f32s(&weights.concat()), &mut bin);
    let v_idx = view(&idx.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(), &mut bin);
    let v_ibm = view(&f32s(&ibm.concat()), &mut bin);
    let v_times = view(&f32s(&times), &mut bin);
    let v_turns = view(&f32s(&turns.concat()), &mut bin);
    let v_joints1 = view(&joints1.concat().iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(), &mut bin);
    let v_weights1 = view(&f32s(&weights1.concat()), &mut bin);
    let n = pos.len();
    let lo: Vec<f32> = (0..3).map(|k| pos.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min)).collect();
    let hi: Vec<f32> = (0..3).map(|k| pos.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max)).collect();
    let mut attributes = json!({"POSITION": 0, "NORMAL": 1, "JOINTS_0": 2, "WEIGHTS_0": 3});
    if eight {
        attributes["JOINTS_1"] = json!(8);
        attributes["WEIGHTS_1"] = json!(9);
    }
    let buffer = if apart {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("arm.bin"), &bin).map_err(|e| e.to_string())?;
        json!({"uri": "arm.bin", "byteLength": bin.len()})
    } else {
        json!({"uri": format!("data:application/octet-stream;base64,{}", base64(&bin)), "byteLength": bin.len()})
    };
    let doc = json!({
        "asset": {"version": "2.0", "generator": "lahza test arm"},
        "scene": 0,
        "scenes": [{"nodes": [0, 3]}],
        "nodes": [
            {"name": "root", "translation": root, "children": [1]},
            {"name": "mid", "translation": [0, 1, 0], "children": [2]},
            {"name": "tip", "translation": [0, 1, 0]},
            {"name": "arm", "mesh": 0, "skin": 0, "translation": [0.3, 0, 0]}
        ],
        "skins": [{"joints": [0, 1, 2], "inverseBindMatrices": 5, "skeleton": 0}],
        "meshes": [{"primitives": [{"attributes": attributes, "indices": 4, "material": 0}]}],
        "materials": [{"name": "skin", "pbrMetallicRoughness": {"baseColorFactor": [0.9, 0.55, 0.35, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.6}}],
        "animations": [{"name": "bend", "samplers": [{"input": 6, "output": 7, "interpolation": "LINEAR"}], "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}]}],
        "buffers": [buffer],
        "bufferViews": views,
        "accessors": [
            {"bufferView": v_pos, "componentType": 5126, "count": n, "type": "VEC3", "min": lo, "max": hi},
            {"bufferView": v_nor, "componentType": 5126, "count": n, "type": "VEC3"},
            {"bufferView": v_joints, "componentType": 5123, "count": n, "type": "VEC4"},
            {"bufferView": v_weights, "componentType": 5126, "count": n, "type": "VEC4"},
            {"bufferView": v_idx, "componentType": 5125, "count": idx.len(), "type": "SCALAR"},
            {"bufferView": v_ibm, "componentType": 5126, "count": 3, "type": "MAT4"},
            {"bufferView": v_times, "componentType": 5126, "count": 3, "type": "SCALAR", "min": [0.0], "max": [1.0]},
            {"bufferView": v_turns, "componentType": 5126, "count": 3, "type": "VEC4"},
            {"bufferView": v_joints1, "componentType": 5123, "count": n, "type": "VEC4"},
            {"bufferView": v_weights1, "componentType": 5126, "count": n, "type": "VEC4"}
        ]
    });
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join("arm.gltf");
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(path)
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            out.push(if k <= c.len() { T[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}
