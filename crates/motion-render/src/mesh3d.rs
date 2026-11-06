//! Mesh layers in the display list, and their CPU shading.
//!
//! A run of mesh layers becomes one *batch*: its triangles are moved into
//! camera space, clipped at the near plane, back faces dropped, and
//! projected to canvas pixels. When the batch is placed on a target its
//! triangles are binned into that target's 16×16 tiles, and one
//! `PRIM_MESH` primitive covers them. Per pixel the rasteriser finds the
//! nearest triangle at eight sample points (a depth test, so meshes in a
//! batch intersect correctly), then shades each distinct triangle once at
//! the centroid of the samples it won: physically based (GGX specular,
//! clear coat, fresnel) under the scene's lights and a procedural
//! environment, in linear light, encoded to sRGB.
//!
//! `shade_mesh` mirrors `mesh_shade` in `motion.wgsl` line for line.

use crate::list::{Prim, Target, PRIM_MESH, TILE};
use lahza_motion_core::compile::{Lighting, MeshState};
use lahza_motion_core::document::LightKind;
use lahza_motion_core::mesh::{self, Mesh, PARTS, TWO_SIDED};
use lahza_motion_core::space::Proj;
use std::collections::HashMap;
use std::sync::Arc;

/// Generated meshes by parameters; entries unused for a while are dropped.
#[derive(Default)]
pub struct MeshCache {
    meshes: HashMap<Vec<u8>, (Arc<Mesh>, u64)>,
    /// Per generated mesh: each triangle's outward winding (+1/−1, 0 open).
    sides: HashMap<usize, Arc<Vec<f32>>>,
    frame: u64,
    /// Batches of the display list being built.
    pub(crate) batches: Vec<Batch>,
    /// The layer lists being emitted (innermost last).
    runs: Vec<Run>,
    /// Shadow maps already written to the display list being built
    /// (by address), and where.
    pub(crate) placed: HashMap<usize, u32>,
    serial: u64,
    pub warnings: Vec<String>,
    /// The project folder (environment images resolve against it).
    pub root: Option<std::path::PathBuf>,
    /// Environment images, loaded and prefiltered once.
    envs: HashMap<Arc<str>, Option<Arc<crate::envmap::EnvMap>>>,
    /// Environment images that could not be loaded (not drawn as asked).
    pub dropped: Vec<String>,
    /// Image maps of the meshes being added (resolved by the list builder).
    pub(crate) textures: HashMap<Arc<str>, Option<crate::images::Texture>>,
}

/// A mesh layer of a layer list, for the other batches of that list.
struct RunMesh {
    index: usize,
    state: MeshState,
    root: Proj,
    /// Opaque, normal blend: it hides what is behind it.
    occludes: bool,
    bounds: [f64; 4],
}

/// The mesh layers of one layer list: batches made from the list hide
/// what the list's other meshes cover.
struct Run {
    serial: u64,
    meshes: Vec<RunMesh>,
    /// Flat 3D layers drawn as cards in the list's batches.
    cards: Vec<Card>,
    current: usize,
}

/// A flat 3D layer in the mesh depth buffer: its picture (a group, in
/// output pixels) on a quad at its place in 3D. Samples where the picture
/// is under half opaque let what is behind show.
#[derive(Clone, Debug)]
pub(crate) struct Card {
    pub index: usize,
    /// Quad corners in camera space and in output pixels.
    pub view: [[f64; 3]; 4],
    pub screen: [[f64; 2]; 4],
    pub picture: Backdrop,
    pub bounds: [f64; 4],
}

/// A mesh layer that lets what is behind it show (opacity, glass, a
/// blend mode): drawn after the opaque meshes, never hiding them.
pub(crate) fn see_through(layer: &lahza_motion_core::compile::LayerState) -> bool {
    use lahza_motion_core::compile::Content;
    let Content::Mesh(m) = &layer.content else { return false };
    layer.opacity < 1.0
        || layer.blend != lahza_motion_core::document::Blend::Normal
        || m.materials.iter().any(|x| x.opacity < 1.0 || x.transmission > 0.0 || x.color.0[3] < 1.0)
        || m.colors.iter().any(|c| c.0[3] < 1.0)
        || m.materials.iter().any(|x| x.wireframe > 0.0 && x.wire_fill.is_none())
}

impl MeshCache {
    /// `root`: the project folder.
    pub fn new(root: Option<&std::path::Path>) -> Self {
        Self { root: root.map(std::path::Path::to_path_buf), ..Default::default() }
    }

    pub(crate) fn begin(&mut self) {
        self.frame += 1;
        self.batches.clear();
        self.runs.clear();
        self.placed.clear();
        self.textures.clear();
        if self.meshes.len() > 96 {
            let keep = self.frame.saturating_sub(4);
            self.meshes.retain(|_, (_, used)| *used >= keep);
            let live: std::collections::HashSet<usize> = self.meshes.values().map(|(m, _)| Arc::as_ptr(m) as usize).collect();
            self.sides.retain(|k, _| live.contains(k));
        }
    }

    /// Starts emitting a layer list drawn through `root`.
    pub(crate) fn open_run(&mut self, layers: &[lahza_motion_core::compile::LayerState], root: Proj) {
        use lahza_motion_core::compile::Content;
        self.serial += 1;
        let meshes = layers
            .iter()
            .enumerate()
            .filter_map(|(index, l)| {
                let Content::Mesh(m) = &l.content else { return None };
                let b = m.screen_bounds()?;
                let pts = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| root.apply(x, y));
                let bounds = [
                    pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min),
                    pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
                    pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max),
                    pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
                ];
                Some(RunMesh { index, state: (**m).clone(), root, occludes: !see_through(l), bounds })
            })
            .collect();
        self.runs.push(Run { serial: self.serial, meshes, cards: Vec::new(), current: 0 });
    }

    /// The layer of the current list about to be emitted.
    pub(crate) fn set_current(&mut self, index: usize) {
        if let Some(r) = self.runs.last_mut() {
            r.current = index;
        }
    }

    /// A mesh of the current list (its lighting and camera start batches
    /// that open with a card).
    pub(crate) fn add_card(&mut self, card: Card) {
        if let Some(r) = self.runs.last_mut() {
            r.cards.push(card);
        }
    }

    /// The current list and layer (for a new or joined batch).
    pub(crate) fn member(&self) -> (u64, usize) {
        self.runs.last().map(|r| (r.serial, r.current)).unwrap_or((0, 0))
    }

    /// Ends the current list: each of its batches gets the list's other
    /// opaque meshes over it as occluders.
    pub(crate) fn close_run(&mut self) {
        let Some(run) = self.runs.pop() else { return };
        if run.meshes.is_empty() && run.cards.len() < 2 {
            return;
        }
        // Shadows: every mesh of the list casts onto every other.
        let shadows = self.shadow_maps(&run);
        let mut batches = std::mem::take(&mut self.batches);
        // Cards are unlit pictures: no light rows to hang shadow maps on.
        for b in batches.iter_mut().filter(|b| b.list == run.serial && !b.cards_only) {
            b.shadows = shadows.clone();
        }
        if run.meshes.len() + run.cards.len() < 2 {
            self.batches = batches;
            return;
        }
        for b in batches.iter_mut().filter(|b| b.list == run.serial) {
            for rm in &run.meshes {
                let overlaps = rm.bounds[2] >= b.bounds[0] && rm.bounds[0] <= b.bounds[2] && rm.bounds[3] >= b.bounds[1] && rm.bounds[1] <= b.bounds[3];
                if rm.occludes && overlaps && !b.members.contains(&rm.index) {
                    b.add_occluder(self, &rm.state, rm.root);
                }
            }
            // Cards are drawn after the opaque meshes and blend over them,
            // so they hide only other cards (in another cards' batch).
            if !b.cards_only {
                continue;
            }
            for card in &run.cards {
                let overlaps = card.bounds[2] >= b.bounds[0] && card.bounds[0] <= b.bounds[2] && card.bounds[3] >= b.bounds[1] && card.bounds[1] <= b.bounds[3];
                if overlaps && !b.members.contains(&card.index) {
                    b.add_card(card, true);
                }
            }
        }
        self.batches = batches;
    }

    /// One light-space depth map per shadow-casting light of the list's
    /// lighting, from all of its meshes (back faces too).
    fn shadow_maps(&mut self, run: &Run) -> Arc<Vec<Option<Arc<ShadowMap>>>> {
        let Some(first) = run.meshes.first() else { return Arc::default() };
        let Some(lighting) = first.state.lighting.clone() else { return Arc::default() };
        if !lighting.lights.iter().any(|l| l.shadow && l.color.iter().any(|c| *c > 0.0)) {
            return Arc::default();
        }
        // Camera-space geometry of every mesh, indexed; each closed
        // triangle's outward side (+1/−1 on its winding), 0 when open.
        let mut caster = Caster::default();
        for rm in &run.meshes {
            if rm.state.materials.iter().all(|x| x.wireframe > 0.0 && x.wire_fill.is_none()) {
                continue;
            }
            let Some(mesh) = self.get(&rm.state) else { continue };
            let mv = rm.state.camera.view.then(rm.state.world);
            let m3 = mv.0;
            let det = m3[0] * (m3[5] * m3[10] - m3[6] * m3[9]) - m3[1] * (m3[4] * m3[10] - m3[6] * m3[8]) + m3[2] * (m3[4] * m3[9] - m3[5] * m3[8]);
            let base = caster.verts.len() as u32;
            caster.verts.extend(mesh.positions.iter().map(|p| {
                let q = mv.apply(p[0] as f64, p[1] as f64, p[2] as f64);
                [q[0], q[1], q[2]]
            }));
            let sides = self
                .sides
                .entry(Arc::as_ptr(&mesh) as usize)
                .or_insert_with(|| {
                    Arc::new(
                        mesh.tris
                            .iter()
                            .zip(&mesh.tags)
                            .map(|(t, &tag)| {
                                if tag & TWO_SIDED != 0 {
                                    return 0.0;
                                }
                                let p = t.map(|i| mesh.positions[i as usize].map(|v| v as f64));
                                let n = t.iter().fold([0.0; 3], |acc, &i| add(acc, mesh.normals[i as usize].map(|v| v as f64)));
                                dot(cross(sub(p[1], p[0]), sub(p[2], p[0])), n).signum() as f32
                            })
                            .collect(),
                    )
                })
                .clone();
            let flip = det.signum() as f32;
            caster.tris.extend(mesh.tris.iter().map(|t| t.map(|i| i + base)));
            caster.sides.extend(sides.iter().map(|s| s * flip));
        }
        let tris = &caster;
        let view = first.state.camera.view;
        let scale = first.root.max_scale().clamp(0.25, 2.0);
        let size = ((1024.0 * scale / 4.0).round() as usize * 4).clamp(256, 2048);
        let maps = lighting
            .lights
            .iter()
            .map(|l| {
                if !l.shadow || tris.tris.is_empty() {
                    return None;
                }
                let v = view.0;
                let r = [[v[0], v[1], v[2]], [v[4], v[5], v[6]], [v[8], v[9], v[10]]];
                let rot = |d: [f64; 3]| [0, 1, 2].map(|k| r[k][0] * d[0] + r[k][1] * d[1] + r[k][2] * d[2]);
                match l.kind {
                    LightKind::Directional => ShadowMap::directional(&tris, rot(l.vector), size, l.softness),
                    LightKind::Point => {
                        let p = view.apply(l.vector[0], l.vector[1], l.vector[2]);
                        ShadowMap::point(&tris, [p[0], p[1], p[2]], size, l.softness)
                    }
                    LightKind::Area => {
                        // From its centre, softened by its size.
                        let p = view.apply(l.vector[0], l.vector[1], l.vector[2]);
                        let soft = ((len3(l.u) + len3(l.v)) / 150.0).clamp(0.1, 1.0).max(l.softness);
                        ShadowMap::point(&tris, [p[0], p[1], p[2]], size, soft)
                    }
                    LightKind::Ambient => None,
                }
                .map(Arc::new)
            })
            .collect();
        Arc::new(maps)
    }

    /// A mesh layer as an instance for the path tracer: its geometry, its
    /// object → camera transform and its materials by part and segment.
    pub(crate) fn trace_instance(&mut self, id: Arc<str>, m: &MeshState, opacity: f64, scale: f64) -> Option<crate::pt::TraceInstance> {
        let mesh = self.get(m)?;
        let mv = m.camera.view.then(m.world).0;
        let transform = [0, 1, 2].map(|r| [mv[r * 4] as f32, mv[r * 4 + 1] as f32, mv[r * 4 + 2] as f32, mv[r * 4 + 3] as f32]);
        let w = m.world.0;
        let world = [0, 1, 2].map(|r| [w[r * 4] as f32, w[r * 4 + 1] as f32, w[r * 4 + 2] as f32, w[r * 4 + 3] as f32]);
        let mut materials = Vec::new();
        let mut parts = [(0u32, 1u32); PARTS];
        for (part, slot) in parts.iter_mut().enumerate() {
            let colors = piece_colors(m, part);
            *slot = ((materials.len() / MAT_ROWS) as u32, colors as u32);
            for segment in 0..colors as u32 {
                materials.extend_from_slice(&material_rows(m, part, segment, opacity, scale, &self.textures));
            }
        }
        Some(crate::pt::TraceInstance { id, mesh, key: m.params.key(), transform, world, materials, parts })
    }

    /// The environment image of these lights, loaded once.
    pub(crate) fn env_map(&mut self, l: &Lighting) -> Option<Arc<crate::envmap::EnvMap>> {
        let path = l.env.image.clone()?;
        if let Some(e) = self.envs.get(&path) {
            return e.clone();
        }
        let full = match &self.root {
            Some(root) => root.join(&*path),
            None => std::path::PathBuf::from(&*path),
        };
        let env = match crate::envmap::EnvMap::load(&full) {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                self.dropped.push(format!("environment image '{path}': {e}"));
                None
            }
        };
        self.envs.insert(path, env.clone());
        env
    }

    fn get(&mut self, m: &MeshState) -> Option<Arc<Mesh>> {
        let key = m.params.key();
        if let Some((mesh, used)) = self.meshes.get_mut(&key) {
            *used = self.frame;
            return Some(mesh.clone());
        }
        match mesh::generate(&m.params) {
            Ok(mesh) => {
                let mesh = Arc::new(mesh);
                self.meshes.insert(key, (mesh.clone(), self.frame));
                Some(mesh)
            }
            Err(e) => {
                self.warnings.push(e);
                None
            }
        }
    }
}

/// A projected vertex: canvas position, camera-space position and normal.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vert {
    screen: [f64; 2],
    pos: [f32; 3],
    normal: [f32; 3],
    /// Noise-space position (object or world) and UV.
    aux: [f32; 5],
}

/// Rows per vertex: [camera position, 1/z], [normal, u], [noise-space
/// position, v].
pub(crate) const VROWS: usize = 3;

pub(crate) struct Batch {
    verts: Vec<Vert>,
    /// Vertex indices and material index.
    tris: Vec<([u32; 3], u32, u8)>,
    /// MAT_ROWS rows per material (see `material_rows`).
    materials: Vec<[f32; 4]>,
    lighting: Arc<Lighting>,
    /// Rows of the light block.
    light_rows: Vec<[f32; 4]>,
    pub(crate) bounds: [f64; 4],
    layers: usize,
    /// The material standing for other meshes' surfaces: they win the
    /// depth test but draw nothing (their own batch draws them).
    occluder: Option<u32>,
    /// The layer list this batch was made in and its layers' places in it.
    pub(crate) list: u64,
    pub(crate) members: Vec<usize>,
    pub(crate) see_through: bool,
    /// Per light of the lighting: its shadow map, if it casts shadows.
    shadows: Arc<Vec<Option<Arc<ShadowMap>>>>,
    /// The environment image (written once per display list).
    pub(crate) env: Option<Arc<crate::envmap::EnvMap>>,
    /// Glass: a group holding everything behind, to refract.
    pub(crate) backdrop: Option<Backdrop>,
    /// Camera focal length in output pixels (refraction offsets).
    focal: f64,
    /// Holds only cards (no meshes, no lighting).
    pub(crate) cards_only: bool,
}

/// A group over output-pixel rect `origin + factor·(x, y)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backdrop {
    pub offset: u32,
    pub size: (u32, u32),
    pub origin: [f64; 2],
    pub factor: [f64; 2],
}

/// Whether a mesh layer refracts (some part is glass).
pub(crate) fn refracts(m: &MeshState) -> bool {
    m.materials.iter().any(|x| x.transmission > 0.0 && !x.unlit && !x.shadow_catcher)
}

/// A light-space depth map: how close to the light the nearest surface is
/// at each texel. Rows: [kind (0 directional, 1 point), size, u0, v0],
/// [texels per unit u, v, bias, filter radius in texels], [U, normal
/// offset], [V, 0], [toward the light (directional) or axis (point), 0],
/// [light position, 0], then the texels, 4 per row.
pub(crate) struct ShadowMap {
    rows: Vec<[f32; 4]>,
}

const NO_SURFACE: f32 = -1.0e30;

fn len3(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn unit3(v: [f64; 3]) -> [f64; 3] {
    let l = dot(v, v).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// Two unit vectors completing `d` to an orthonormal basis.
fn basis(d: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let helper = if d[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let u = unit3(cross(helper, d));
    (u, cross(d, u))
}

/// The meshes of a layer list as shadow casters (camera space).
#[derive(Default)]
struct Caster {
    verts: Vec<[f64; 3]>,
    tris: Vec<[u32; 3]>,
    /// Which winding faces out (+1/−1); 0 for open, two-sided surfaces.
    sides: Vec<f32>,
}

impl ShadowMap {
    fn directional(c: &Caster, toward: [f64; 3], size: usize, softness: f64) -> Option<ShadowMap> {
        let l = unit3(toward);
        let (u, v) = basis(l);
        let proj: Vec<(f64, f64, f64)> = c.verts.iter().map(|&p| (dot(p, u), dot(p, v), dot(p, l))).collect();
        Self::build(c, &proj, &|_| l, size, softness, 0, l, [0.0; 3], u, v)
    }

    fn point(c: &Caster, pos: [f64; 3], size: usize, softness: f64) -> Option<ShadowMap> {
        // Aim at the middle of the geometry.
        let mut m = [0.0; 3];
        for p in &c.verts {
            m = add(m, *p);
        }
        let n = c.verts.len().max(1) as f64;
        let axis = unit3(sub([m[0] / n, m[1] / n, m[2] / n], pos));
        let (u, v) = basis(axis);
        let proj: Vec<(f64, f64, f64)> = c
            .verts
            .iter()
            .map(|&p| {
                let q = sub(p, pos);
                let z = dot(q, axis);
                if z < 1.0 {
                    return (f64::NAN, f64::NAN, f64::NAN);
                }
                // Closeness 1/z interpolates linearly across the map.
                ((dot(q, u) / z).clamp(-3.0, 3.0), (dot(q, v) / z).clamp(-3.0, 3.0), 1.0 / z)
            })
            .collect();
        Self::build(c, &proj, &|p| sub(pos, p), size, softness, 1, axis, pos, u, v)
    }

    /// Rasterises the casters' light-facing triangles (projected per
    /// vertex as (u, v, closeness)) keeping the nearest surface per texel.
    #[allow(clippy::too_many_arguments)]
    fn build(c: &Caster, proj: &[(f64, f64, f64)], toward: &(dyn Fn([f64; 3]) -> [f64; 3] + Sync), size: usize, softness: f64, kind: u32, axis: [f64; 3], pos: [f64; 3], u: [f64; 3], v: [f64; 3]) -> Option<ShadowMap> {
        // Closed surfaces only need their sides facing the light.
        let faces = |i: usize| {
            let t = c.tris[i];
            if t.iter().any(|&k| !proj[k as usize].0.is_finite()) {
                return false;
            }
            if c.sides[i] == 0.0 {
                return true;
            }
            let p = t.map(|k| c.verts[k as usize]);
            let g = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            dot(g, toward(p[0])) * c.sides[i] as f64 >= 0.0
        };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
        let chunk = c.tris.len().div_ceil(threads).max(1);
        let keep: Vec<usize> = std::thread::scope(|scope| {
            let faces = &faces;
            let jobs: Vec<_> = (0..c.tris.len()).step_by(chunk).map(|a| scope.spawn(move || (a..(a + chunk).min(c.tris.len())).filter(|&i| faces(i)).collect::<Vec<usize>>())).collect();
            jobs.into_iter().flat_map(|j| j.join().expect("shadow casters")).collect()
        });
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for &i in &keep {
            for &k in &c.tris[i] {
                let p = proj[k as usize];
                b = [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)];
            }
        }
        if !(b[2] > b[0] && b[3] > b[1]) {
            return None;
        }
        let n = size as f64;
        // A two-texel margin all round.
        let (pu, pv) = ((b[2] - b[0]) * 2.0 / n, (b[3] - b[1]) * 2.0 / n);
        let (u0, v0) = (b[0] - pu, b[1] - pv);
        let (su, sv) = (n / (b[2] - b[0] + 2.0 * pu), n / (b[3] - b[1] + 2.0 * pv));
        let mut texels = vec![NO_SURFACE; size * size];
        // Triangles in texel space, sorted into the threads' bands of rows.
        let tex: Vec<[(f64, f64, f64); 3]> = keep.iter().map(|&i| c.tris[i].map(|k| {
            let q = proj[k as usize];
            ((q.0 - u0) * su, (q.1 - v0) * sv, q.2)
        })).collect();
        // Many thin bands dealt out in turn, so busy regions spread over
        // every thread.
        let nbands = threads * 8;
        let band = size.div_ceil(nbands);
        let nbands = size.div_ceil(band);
        let mut bands: Vec<Vec<u32>> = vec![Vec::new(); nbands];
        for (i, p) in tex.iter().enumerate() {
            let lo = (p[0].1.min(p[1].1).min(p[2].1) - 0.5).ceil().max(0.0) as usize;
            let hi = (p[0].1.max(p[1].1).max(p[2].1) - 0.5).floor().clamp(-1.0, n - 1.0);
            if hi < lo as f64 {
                continue;
            }
            for bi in lo / band..=(hi as usize / band).min(nbands - 1) {
                bands[bi].push(i as u32);
            }
        }
        let mut work: Vec<Vec<(usize, &mut [f32], &Vec<u32>)>> = (0..threads).map(|_| Vec::new()).collect();
        for ((k, rows), list) in texels.chunks_mut(band * size).enumerate().zip(&bands) {
            work[k % threads].push((k, rows, list));
        }
        std::thread::scope(|scope| {
            for jobs in work {
                let tex = &tex;
                scope.spawn(move || {
                  for (k, rows, list) in jobs {
                    let (y_lo, y_hi) = (k * band, (k * band + rows.len() / size).min(size));
                    for &ti in list {
                        let p = tex[ti as usize];
                        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
                        if area.abs() < 1e-12 {
                            continue;
                        }
                        let ys = (p[0].1.min(p[1].1).min(p[2].1) - 0.5).ceil().max(y_lo as f64) as usize;
                        let ye = ((p[0].1.max(p[1].1).max(p[2].1) - 0.5).floor() + 1.0).min(y_hi as f64);
                        let xs = (p[0].0.min(p[1].0).min(p[2].0) - 0.5).ceil().max(0.0) as usize;
                        let xe = ((p[0].0.max(p[1].0).max(p[2].0) - 0.5).floor() + 1.0).min(n);
                        if ye <= ys as f64 || xe <= xs as f64 {
                            continue;
                        }
                        // Edge functions (weights of the opposite vertex) as
                        // planes: w = a·x + b·y + c; closeness likewise.
                        let plane = |a: (f64, f64, f64), b: (f64, f64, f64)| {
                            let (ea, eb) = (-(b.1 - a.1) / area, (b.0 - a.0) / area);
                            (ea, eb, -(ea * a.0 + eb * a.1))
                        };
                        let e = [plane(p[1], p[2]), plane(p[2], p[0]), plane(p[0], p[1])];
                        let cz = (
                            e[0].0 * p[0].2 + e[1].0 * p[1].2 + e[2].0 * p[2].2,
                            e[0].1 * p[0].2 + e[1].1 * p[1].2 + e[2].1 * p[2].2,
                            e[0].2 * p[0].2 + e[1].2 * p[1].2 + e[2].2 * p[2].2,
                        );
                        let x0 = xs as f64 + 0.5;
                        for y in ys..ye as usize {
                            let cy = y as f64 + 0.5;
                            let row = &mut rows[(y - y_lo) * size..(y - y_lo + 1) * size];
                            let mut w = [0, 1, 2].map(|k| e[k].0 * x0 + e[k].1 * cy + e[k].2);
                            let mut c = cz.0 * x0 + cz.1 * cy + cz.2;
                            for t in &mut row[xs..xe as usize] {
                                if w[0] >= 0.0 && w[1] >= 0.0 && w[2] >= 0.0 && c as f32 > *t {
                                    *t = c as f32;
                                }
                                w[0] += e[0].0;
                                w[1] += e[1].0;
                                w[2] += e[2].0;
                                c += cz.0;
                            }
                        }
                    }
                  }
                });
            }
        });
        // Bias: directional in px along the light; point as a fraction of
        // the distance (both about a texel and a half).
        let texel = (1.0 / su).max(1.0 / sv);
        let bias = if kind == 0 { 1.5 * texel + 0.5 } else { 1.5 * texel };
        let radius = 1.0 + softness * 10.0;
        let f = |a: [f64; 3], w: f64| [a[0] as f32, a[1] as f32, a[2] as f32, w as f32];
        let mut rows = vec![
            [kind as f32, size as f32, u0 as f32, v0 as f32],
            [su as f32, sv as f32, bias as f32, radius as f32],
            f(u, texel),
            f(v, 0.0),
            f(axis, 0.0),
            f(pos, 0.0),
        ];
        rows.extend(texels.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]));
        Some(ShadowMap { rows })
    }
}

impl Batch {
    pub(crate) fn can_join(&self, m: &MeshState) -> bool {
        !self.see_through && m.lighting.as_ref().is_some_and(|l| Arc::ptr_eq(l, &self.lighting) || **l == *self.lighting)
    }
}

const NEAR: f64 = 1.0;
/// Material flags (row 2, w): unlit, faceted, depth-only occluder.
pub const FLAG_UNLIT: u32 = 1;
pub const FLAG_FACETED: u32 = 2;
pub const FLAG_OCCLUDER: u32 = 4;
pub const FLAG_SHADOW_CATCHER: u32 = 8;
/// A card: rows 0–1 hold its picture (canvas → texel map, texel offset
/// relative to the primitive's texel base, size).
pub const FLAG_CARD: u32 = 16;
/// A card stands (wins its sample) wherever its picture shows at all; its
/// colour then blends over what was drawn before it (cards come after the
/// opaque meshes), so soft and anti-aliased edges keep what is behind.
pub const CARD_STANDS: f32 = 1.0 / 512.0;
/// A wireframe: edges `row 3.w` px wide in row 0's colour; faces in row 1's
/// colour, or see-through when its alpha is 0.
pub const FLAG_WIRE: u32 = 32;

/// Shadow filter taps (a Poisson disk on the unit circle).
pub const SHADOW_TAPS: [[f32; 2]; 8] = [[-0.613, 0.617], [0.170, -0.040], [-0.299, -0.792], [0.645, 0.493], [-0.651, -0.075], [0.421, -0.718], [-0.084, 0.290], [0.874, -0.130]];
/// Most triangles one batch may hold (beyond this, meshes are skipped).
const MAX_TRIS: usize = 4_000_000;

use lahza_motion_core::compile::srgb_to_linear;

/// A piece's colour for a part: the part's own `colors`, else the layer's
/// `colors` (unless the part sets its own colour), else the material's.
fn piece_color(m: &MeshState, part: usize, segment: u32) -> lahza_motion_core::value::Color {
    let own = &m.part_colors[part];
    if !own.is_empty() {
        own[segment as usize % own.len()]
    } else if !m.colors.is_empty() && !m.own_color[part] {
        m.colors[segment as usize % m.colors.len()]
    } else {
        m.materials[part].color
    }
}

/// How many distinct colours a part cycles through.
pub(crate) fn piece_colors(m: &MeshState, part: usize) -> usize {
    if !m.part_colors[part].is_empty() {
        m.part_colors[part].len()
    } else if !m.colors.is_empty() && !m.own_color[part] {
        m.colors.len()
    } else {
        1
    }
}

/// Rows per material (GPU raster, CPU raster and path tracer agree).
pub(crate) const MAT_ROWS: usize = 17;

/// Rows of one material:
/// 0 [base rgb linear, opacity], 1 [metallic, roughness, clear coat, its
/// roughness], 2 [emissive rgb, flags], 3 [transmission, ior, thickness,
/// dielectric F0], 4 [specular tint rgb, anisotropy], 5 [sheen rgb (colour ×
/// amount), sheen roughness], 6 [clear coat colour rgb, dispersion], 7 [grain
/// axis (camera space), grain rotation], 8 [mesh centre (camera space), 0],
/// surface detail: 9 [noise kind + 1 (0: none), 1 / scale, octaves, seed
/// bits], 10 [noise colour rgb, amount], 11 [noise roughness ±, noise bump
/// px, 0, 0], 12 [colour map offset, w, h (bits), 1 if set], 13 [normal map
/// the same], 14 [bump map offset, w, h, height px], 15 [uv scale x, y,
/// offset x, y], 16 [uv rotation, 0, 0, 0].
pub(crate) fn material_rows(m: &MeshState, part: usize, segment: u32, opacity: f64, scale: f64, textures: &HashMap<Arc<str>, Option<crate::images::Texture>>) -> [[f32; 4]; MAT_ROWS] {
    let mat = &m.materials[part];
    let color = piece_color(m, part, segment);
    let lin = |c: lahza_motion_core::value::Color| [srgb_to_linear(c.0[0]) as f32, srgb_to_linear(c.0[1]) as f32, srgb_to_linear(c.0[2]) as f32];
    let mut rows = [[0.0f32; 4]; MAT_ROWS];
    if mat.wireframe > 0.0 {
        // Wires: [colour, opacity], [fill colour, fill opacity (0: see-through)],
        // [_, flags], [_, _, _, width in output px].
        let (w, f) = (lin(color), mat.wire_fill.map(|c| (lin(c), c.0[3] * mat.opacity * opacity)).unwrap_or(([0.0; 3], 0.0)));
        rows[0] = [w[0], w[1], w[2], (color.0[3] * mat.opacity * opacity) as f32];
        rows[1] = [f.0[0], f.0[1], f.0[2], f.1 as f32];
        rows[2] = [0.0, 0.0, 0.0, (FLAG_WIRE | FLAG_UNLIT) as f32];
        rows[3] = [0.0, 0.0, 0.0, (mat.wireframe * scale) as f32];
        return rows;
    }
    let flags = (mat.unlit as u32) * FLAG_UNLIT | (mat.faceted as u32) * FLAG_FACETED | (mat.shadow_catcher as u32) * FLAG_SHADOW_CATCHER;
    let base = lin(color);
    rows[0] = [base[0], base[1], base[2], (color.0[3] * mat.opacity * opacity) as f32];
    rows[1] = [mat.metallic as f32, mat.roughness as f32, mat.clearcoat as f32, mat.clearcoat_roughness as f32];
    rows[2] = [mat.emissive[0] as f32, mat.emissive[1] as f32, mat.emissive[2] as f32, flags as f32];
    rows[3] = [mat.transmission as f32, mat.ior as f32, mat.thickness as f32, (0.08 * mat.specular) as f32];
    let tint = lin(mat.specular_tint);
    rows[4] = [tint[0], tint[1], tint[2], mat.anisotropy as f32];
    let sheen = lin(mat.sheen_color).map(|c| c * mat.sheen as f32);
    rows[5] = [sheen[0], sheen[1], sheen[2], mat.sheen_roughness as f32];
    let coat = lin(mat.clearcoat_color);
    rows[6] = [coat[0], coat[1], coat[2], mat.dispersion as f32];
    // The grain runs around the mesh's axis (y; z for rings and revolves
    // about z): spun metal.
    let mv = m.camera.view.then(m.world).0;
    let axis_obj = match &m.params.shape {
        mesh::MeshShape::Torus { .. } | mesh::MeshShape::Revolve { axis_y: false, .. } => [0.0, 0.0, 1.0],
        _ => [0.0, 1.0, 0.0],
    };
    let a = [0, 1, 2].map(|r| mv[r * 4] * axis_obj[0] + mv[r * 4 + 1] * axis_obj[1] + mv[r * 4 + 2] * axis_obj[2]);
    let a = unit3(a);
    rows[7] = [a[0] as f32, a[1] as f32, a[2] as f32, mat.anisotropy_rotation as f32];
    rows[8] = [mv[3] as f32, mv[7] as f32, mv[11] as f32, 0.0];
    let b = |v: u32| f32::from_bits(v);
    if let Some(n) = &mat.noise {
        rows[9] = [(n.field.kind.code() + 1) as f32, (1.0 / n.field.scale) as f32, n.field.octaves as f32, b(n.field.seed)];
        let (c, amount) = n.color.map(|c| (lin(c), n.amount)).unwrap_or(([0.0; 3], 0.0));
        rows[10] = [c[0], c[1], c[2], amount as f32];
        rows[11] = [n.roughness as f32, n.bump as f32, if n.world { 1.0 } else { 0.0 }, 0.0];
    }
    let texture = |path: &Option<Arc<str>>| path.as_ref().and_then(|p| textures.get(p).copied().flatten());
    if let Some(t) = texture(&mat.map) {
        rows[12] = [b(t.offset), b(t.width), b(t.height), 1.0];
    }
    if let Some(t) = texture(&mat.normal_map) {
        rows[13] = [b(t.offset), b(t.width), b(t.height), 1.0];
    }
    if let Some(t) = texture(&mat.bump_map) {
        rows[14] = [b(t.offset), b(t.width), b(t.height), mat.bump as f32];
    }
    rows[15] = [mat.uv[0] as f32, mat.uv[1] as f32, mat.uv[2] as f32, mat.uv[3] as f32];
    // Which UVs wrap around (1: u, 2: v), so a triangle across the seam
    // joins its corners.
    let wraps = match &m.params.shape {
        mesh::MeshShape::Sphere { .. } | mesh::MeshShape::Cylinder { .. } | mesh::MeshShape::Capsule { .. } | mesh::MeshShape::Revolve { .. } => 1.0,
        mesh::MeshShape::Torus { .. } => 3.0,
        _ => 0.0,
    };
    rows[16] = [mat.uv[4] as f32, wraps, 0.0, 0.0];
    rows
}

/// Rows per light in the light block.
pub(crate) const LROWS: usize = 5;

/// The light block: [lights, lobes, 0, 0], camera→world rotation (3
/// rows), sky, horizon, ground, then per light (LROWS) [vector, kind]
/// [colour, shadow] [range (0: none), softness, 0, 0] and two rows for
/// area lights, and per lobe [direction, size] [colour]. Light vectors are
/// in camera space; lobes in world space.
pub(crate) fn light_rows(l: &Lighting, m: &MeshState) -> Vec<[f32; 4]> {
    let v = m.camera.view.0;
    // World → camera rotation R (rows of the view matrix); camera → world is Rᵀ.
    let r = [[v[0], v[1], v[2]], [v[4], v[5], v[6]], [v[8], v[9], v[10]]];
    let rot = |d: [f64; 3]| [r[0][0] * d[0] + r[0][1] * d[1] + r[0][2] * d[2], r[1][0] * d[0] + r[1][1] * d[1] + r[1][2] * d[2], r[2][0] * d[0] + r[2][1] * d[1] + r[2][2] * d[2]];
    let f = |a: [f64; 3], w: f64| [a[0] as f32, a[1] as f32, a[2] as f32, w as f32];
    // Row 0: [lights, lobes, environment image (1), its intensity]; the
    // sky row's w is the image's rotation and the horizon row's w its
    // offset from this block (set where the image is placed).
    let image = l.env.image.is_some();
    let mut rows = vec![[l.lights.len() as f32, l.env.lobes.len() as f32, if image { 1.0 } else { 0.0 }, l.env.image_intensity as f32]];
    for c in 0..3 {
        rows.push([r[0][c] as f32, r[1][c] as f32, r[2][c] as f32, 0.0]);
    }
    rows.push(f(l.env.sky, l.env.image_rotation));
    rows.push(f(l.env.horizon, 0.0));
    rows.push(f(l.env.ground, 0.0));
    for light in &l.lights {
        let (vector, kind) = match light.kind {
            LightKind::Directional => (rot(light.vector), 0.0),
            LightKind::Point => {
                let p = m.camera.view.apply(light.vector[0], light.vector[1], light.vector[2]);
                ([p[0], p[1], p[2]], 1.0)
            }
            LightKind::Ambient => ([0.0; 3], 2.0),
            LightKind::Area => {
                let p = m.camera.view.apply(light.vector[0], light.vector[1], light.vector[2]);
                ([p[0], p[1], p[2]], 3.0)
            }
        };
        rows.push(f(vector, kind));
        rows.push(f(light.color, 0.0));
        rows.push([light.range as f32, light.softness as f32, 0.0, 0.0]);
        // Area lights: half extents (camera space), shape, area.
        let (hu, hv) = (len3(light.u), len3(light.v));
        let area = if light.disc { std::f64::consts::PI * hu * hv } else { 4.0 * hu * hv };
        rows.push(f(rot(light.u), if light.disc { 1.0 } else { 0.0 }));
        rows.push(f(rot(light.v), area));
    }
    for lobe in &l.env.lobes {
        rows.push(f(lobe.dir, lobe.size));
        rows.push(f(lobe.color, 0.0));
    }
    rows
}

impl Batch {
    /// A batch of cards only (flat 3D layers, no meshes): no lights.
    pub(crate) fn cards_only() -> Batch {
        let mut rows = vec![[0.0f32; 4]; 7];
        rows[1][0] = 1.0;
        rows[2][1] = 1.0;
        rows[3][2] = 1.0;
        Batch {
            verts: Vec::new(),
            tris: Vec::new(),
            materials: Vec::new(),
            lighting: Arc::new(Lighting { lights: Vec::new(), env: lahza_motion_core::compile::EnvState { sky: [0.0; 3], horizon: [0.0; 3], ground: [0.0; 3], lobes: Vec::new(), image: None, image_intensity: 0.0, image_rotation: 0.0 } }),
            light_rows: rows,
            bounds: [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
            layers: 0,
            occluder: None,
            list: 0,
            members: Vec::new(),
            see_through: false,
            shadows: Arc::default(),
            env: None,
            backdrop: None,
            focal: 1.0,
            cards_only: true,
        }
    }

    pub(crate) fn new(m: &MeshState) -> Option<Batch> {
        let lighting = m.lighting.clone()?;
        let light_rows = light_rows(&lighting, m);
        Some(Batch {
            verts: Vec::new(),
            tris: Vec::new(),
            materials: Vec::new(),
            lighting,
            light_rows,
            bounds: [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
            layers: 0,
            occluder: None,
            list: 0,
            members: Vec::new(),
            see_through: false,
            shadows: Arc::default(),
            env: None,
            backdrop: None,
            focal: m.camera.focal,
            cards_only: false,
        })
    }

    /// Adds one mesh layer drawn through `root` (canvas → output pixels).
    pub(crate) fn add(&mut self, cache: &mut MeshCache, m: &MeshState, root: Proj, opacity: f64) {
        self.add_mesh(cache, m, root, opacity, false);
    }

    /// Adds another layer's mesh as depth only: where it is nearer, this
    /// batch shows nothing.
    pub(crate) fn add_occluder(&mut self, cache: &mut MeshCache, m: &MeshState, root: Proj) {
        if self.occluder.is_none() {
            self.occluder = Some((self.materials.len() / MAT_ROWS) as u32);
            let mut rows = [[0.0; 4]; MAT_ROWS];
            rows[2][3] = FLAG_OCCLUDER as f32;
            self.materials.extend_from_slice(&rows);
        }
        self.add_mesh(cache, m, root, 1.0, true);
    }

    /// Adds a card (a flat 3D layer's picture on its quad); as an occluder
    /// it hides this batch where its picture is, drawing nothing.
    pub(crate) fn add_card(&mut self, card: &Card, occluder: bool) {
        let b = |v: u32| f32::from_bits(v);
        let p = card.picture;
        let flags = FLAG_CARD | if occluder { FLAG_OCCLUDER } else { 0 };
        let material = (self.materials.len() / MAT_ROWS) as u32;
        let mut rows = [[0.0; 4]; MAT_ROWS];
        rows[0] = [p.origin[0] as f32, p.origin[1] as f32, p.factor[0] as f32, p.factor[1] as f32];
        rows[1] = [b(p.offset), b(p.size.0), b(p.size.1), 0.0];
        rows[2] = [0.0, 0.0, 0.0, flags as f32];
        self.materials.extend_from_slice(&rows);
        let n = cross(sub(card.view[1], card.view[0]), sub(card.view[3], card.view[0]));
        let n = unit3(n);
        let base = self.verts.len() as u32;
        for k in 0..4 {
            let (s, q) = (card.screen[k], card.view[k]);
            self.verts.push(Vert { screen: s, pos: q.map(|v| v as f32), normal: n.map(|v| v as f32), aux: [0.0; 5] });
            if !occluder {
                self.bounds = [self.bounds[0].min(s[0]), self.bounds[1].min(s[1]), self.bounds[2].max(s[0]), self.bounds[3].max(s[1])];
            }
        }
        self.tris.push(([base, base + 1, base + 2], material, 7));
        self.tris.push(([base, base + 2, base + 3], material, 7));
    }

    fn add_mesh(&mut self, cache: &mut MeshCache, m: &MeshState, root: Proj, opacity: f64, occluder: bool) {
        if self.env.is_none() {
            self.env = cache.env_map(&self.lighting);
        }
        let Some(mesh) = cache.get(m) else { return };
        self.layers += 1;
        if self.tris.len() + mesh.tris.len() > MAX_TRIS {
            cache.warnings.push("too many mesh triangles in one frame; some meshes were skipped".into());
            return;
        }
        let mv = m.camera.view.then(m.world);
        let Some(nm) = mv.normal_matrix() else { return };
        let (focal, (cx, cy)) = (m.camera.focal, m.camera.centre);
        if !occluder {
            self.focal = focal * root.max_scale();
        }
        let project = |p: [f64; 3]| -> [f64; 2] {
            let sx = focal * p[0] / p[2] + cx;
            let sy = focal * p[1] / p[2] + cy;
            let (x, y) = root.apply(sx, sy);
            [x, y]
        };
        let cam: Vec<([f64; 3], [f64; 3])> = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .map(|(p, n)| {
                let q = mv.apply(p[0] as f64, p[1] as f64, p[2] as f64);
                let (n0, n1, n2) = (n[0] as f64, n[1] as f64, n[2] as f64);
                let nn = [nm[0][0] * n0 + nm[0][1] * n1 + nm[0][2] * n2, nm[1][0] * n0 + nm[1][1] * n1 + nm[1][2] * n2, nm[2][0] * n0 + nm[2][1] * n1 + nm[2][2] * n2];
                let l = (nn[0] * nn[0] + nn[1] * nn[1] + nn[2] * nn[2]).sqrt().max(1e-12);
                ([q[0], q[1], q[2]], [nn[0] / l, nn[1] / l, nn[2] / l])
            })
            .collect();
        // Surface detail inputs: noise-space position (object, or world when
        // a part's noise asks) and UV.
        let world_noise = m.materials.iter().any(|x| x.noise.is_some_and(|n| n.world));
        let aux: Vec<[f64; 5]> = mesh
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let q = if world_noise { let w = m.world.apply(p[0] as f64, p[1] as f64, p[2] as f64); [w[0], w[1], w[2]] } else { [p[0] as f64, p[1] as f64, p[2] as f64] };
                let uv = mesh.uvs.get(i).copied().unwrap_or([0.0; 2]);
                [q[0], q[1], q[2], uv[0] as f64, uv[1] as f64]
            })
            .collect();
        // Vertex slots: mesh vertices in front of the near plane, placed on demand.
        let mut slot: Vec<u32> = vec![u32::MAX; cam.len()];
        let push = |verts: &mut Vec<Vert>, bounds: &mut [f64; 4], p: [f64; 3], n: [f64; 3], a: [f64; 5]| -> u32 {
            let s = project(p);
            if !occluder {
                *bounds = [bounds[0].min(s[0]), bounds[1].min(s[1]), bounds[2].max(s[0]), bounds[3].max(s[1])];
            }
            verts.push(Vert { screen: s, pos: [p[0] as f32, p[1] as f32, p[2] as f32], normal: [n[0] as f32, n[1] as f32, n[2] as f32], aux: a.map(|v| v as f32) });
            (verts.len() - 1) as u32
        };
        // Material slots by (part, colour), filled on first use.
        let colors_n = (0..PARTS).map(|p| piece_colors(m, p)).max().unwrap_or(1);
        let mut slots = vec![u32::MAX; PARTS * colors_n];
        let wire_scale = root.max_scale();
        for ((t, &tag), &edges) in mesh.tris.iter().zip(&mesh.tags).zip(&mesh.edges) {
            let part = (mesh::tag_part(tag) as usize).min(PARTS - 1);
            let segment = mesh::tag_segment(tag);
            let v = t.map(|i| cam[i as usize]);
            // Back faces of closed surfaces never show.
            let e1 = sub(v[1].0, v[0].0);
            let e2 = sub(v[2].0, v[0].0);
            let mut g = cross(e1, e2);
            let navg = add(add(v[0].1, v[1].1), v[2].1);
            if dot(g, navg) < 0.0 {
                g = [-g[0], -g[1], -g[2]];
            }
            // See-through wireframes show their back edges too.
            let xray = m.materials[part].wireframe > 0.0 && m.materials[part].wire_fill.is_none();
            if tag & TWO_SIDED == 0 && !xray && dot(g, v[0].0) >= 0.0 {
                continue;
            }
            let material = match (occluder, self.occluder) {
                (true, Some(o)) => o,
                _ => {
                    let colors = piece_colors(m, part) as u32;
                    let mslot = &mut slots[part * colors_n + (segment % colors) as usize];
                    if *mslot == u32::MAX {
                        *mslot = (self.materials.len() / MAT_ROWS) as u32;
                        self.materials.extend_from_slice(&material_rows(m, part, segment, opacity, wire_scale, &cache.textures));
                    }
                    *mslot
                }
            };
            let behind = v.iter().filter(|q| q.0[2] < NEAR).count();
            if behind == 3 {
                continue;
            }
            if behind == 0 {
                let ids = t.map(|i| {
                    let s = &mut slot[i as usize];
                    if *s == u32::MAX {
                        *s = push(&mut self.verts, &mut self.bounds, cam[i as usize].0, cam[i as usize].1, aux[i as usize]);
                    }
                    *s
                });
                self.tris.push((ids, material, edges));
                continue;
            }
            // Clip against the near plane (Sutherland–Hodgman).
            let va = t.map(|i| aux[i as usize]);
            let mut poly: Vec<([f64; 3], [f64; 3], [f64; 5])> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (v[k], v[(k + 1) % 3]);
                let (aa, ab) = (va[k], va[(k + 1) % 3]);
                if a.0[2] >= NEAR {
                    poly.push((a.0, a.1, aa));
                }
                if (a.0[2] >= NEAR) != (b.0[2] >= NEAR) {
                    let s = (NEAR - a.0[2]) / (b.0[2] - a.0[2]);
                    let lerp = |x: [f64; 3], y: [f64; 3]| [x[0] + (y[0] - x[0]) * s, x[1] + (y[1] - x[1]) * s, x[2] + (y[2] - x[2]) * s];
                    let lerp5: [f64; 5] = std::array::from_fn(|c| aa[c] + (ab[c] - aa[c]) * s);
                    poly.push((lerp(a.0, b.0), lerp(a.1, b.1), lerp5));
                }
            }
            let ids: Vec<u32> = poly.iter().map(|(p, n, a)| push(&mut self.verts, &mut self.bounds, *p, *n, *a)).collect();
            for k in 1..ids.len().saturating_sub(1) {
                self.tris.push(([ids[0], ids[k], ids[k + 1]], material, 7));
            }
        }
    }

    /// Writes the batch for `target` (whose pixel (x, y) is canvas
    /// `origin + factor·(x, y)`) into the list's mesh buffers and returns
    /// its primitive, or None when nothing lands on the target.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn place(&self, mesh_rows: &mut Vec<[f32; 4]>, bins: &mut Vec<u32>, placed: &mut HashMap<usize, u32>, target: &Target, origin: [f64; 2], factor: [f64; 2], blend: u32) -> Option<(Prim, [f64; 4])> {
        let (tw_px, th_px) = (target.width as f64, target.height as f64);
        let target_origin = origin;
        let to_target = |s: [f64; 2]| [(s[0] - origin[0]) / factor[0], (s[1] - origin[1]) / factor[1]];
        // Triangles on the target with their edge and depth planes.
        struct Tri {
            rows: [[f32; 4]; 4],
            bbox: [f32; 4],
        }
        let setup = |(ids, material, edges): &([u32; 3], u32, u8)| -> Option<Tri> {
            // Positions as the rasteriser sees them (f32), so the edge
            // tests and these planes agree.
            let p = ids.map(|i| to_target(self.verts[i as usize].screen).map(|v| v as f32 as f64));
            let bbox = [
                p[0][0].min(p[1][0]).min(p[2][0]) - 0.5,
                p[0][1].min(p[1][1]).min(p[2][1]) - 0.5,
                p[0][0].max(p[1][0]).max(p[2][0]) + 0.5,
                p[0][1].max(p[1][1]).max(p[2][1]) + 0.5,
            ];
            if bbox[2] < 0.0 || bbox[3] < 0.0 || bbox[0] >= tw_px || bbox[1] >= th_px || !bbox.iter().all(|v| v.is_finite()) {
                return None;
            }
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            if area.abs() < 1e-9 {
                return None;
            }
            // Barycentric weight of vertex k: edge function of the opposite
            // edge over twice the area (sign-free).
            let edge = |a: [f64; 2], b: [f64; 2]| {
                let (ea, eb) = (-(b[1] - a[1]) / area, (b[0] - a[0]) / area);
                [ea, eb, -(ea * a[0] + eb * a[1])]
            };
            let e = [edge(p[1], p[2]), edge(p[2], p[0]), edge(p[0], p[1])];
            let iz = ids.map(|i| 1.0 / self.verts[i as usize].pos[2] as f64);
            let depth = [0, 1, 2].map(|c| e[0][c] * iz[0] + e[1][c] * iz[1] + e[2][c] * iz[2]);
            let f = |v: f64| v as f32;
            let sign = if area > 0.0 { 1.0 } else { -1.0 };
            let rows = [
                [f(p[0][0]), f(p[0][1]), f(p[1][0]), f(p[1][1])],
                [f(p[2][0]), f(p[2][1]), sign, f32::from_bits(*material)],
                [f32::from_bits(ids[0]), f32::from_bits(ids[1]), f32::from_bits(ids[2]), {
                    // Bits: card, wireframe, then the real-edge mask.
                    let flags = self.materials[*material as usize * MAT_ROWS + 2][3] as u32;
                    ((flags & FLAG_CARD != 0) as u32 | ((flags & FLAG_WIRE != 0) as u32) << 1 | (*edges as u32) << 2) as f32
                }],
                [f(depth[0]), f(depth[1]), f(depth[2]), 0.0],
            ];
            Some(Tri { rows, bbox: bbox.map(|v| v as f32) })
        };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
        let tris: Vec<Tri> = if self.tris.len() < 8192 || threads < 2 {
            self.tris.iter().filter_map(setup).collect()
        } else {
            let chunk = self.tris.len().div_ceil(threads);
            std::thread::scope(|scope| {
                let jobs: Vec<_> = self.tris.chunks(chunk).map(|c| scope.spawn(move || c.iter().filter_map(setup).collect::<Vec<Tri>>())).collect();
                let parts: Vec<Vec<Tri>> = jobs.into_iter().map(|j| j.join().expect("triangle setup")).collect();
                let mut all = Vec::with_capacity(parts.iter().map(Vec::len).sum());
                for p in parts {
                    all.extend(p);
                }
                all
            })
        };
        // The drawn triangles set the covered area; occluders only matter
        // inside it.
        let is_occluder = |t: &Tri| self.materials[t.rows[1][3].to_bits() as usize * MAT_ROWS + 2][3] as u32 & FLAG_OCCLUDER != 0;
        let mut used = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for t in tris.iter().filter(|t| !is_occluder(t)) {
            let bbox = t.bbox.map(|v| v as f64);
            used = [used[0].min(bbox[0]), used[1].min(bbox[1]), used[2].max(bbox[2]), used[3].max(bbox[3])];
        }
        if !(used[2] >= used[0]) {
            return None;
        }
        let mut tris = tris;
        if self.occluder.is_some() {
            tris.retain(|t| !is_occluder(t) || (t.bbox[2] as f64 >= used[0] && t.bbox[0] as f64 <= used[2] && t.bbox[3] as f64 >= used[1] && t.bbox[1] as f64 <= used[3]));
        }
        let clamp_x = |v: f64| (v.max(0.0).min(tw_px - 1.0) as u32 / TILE).min(target.tiles_x - 1);
        let clamp_y = |v: f64| (v.max(0.0).min(th_px - 1.0) as u32 / TILE).min(target.tiles_y - 1);
        let (tx0, ty0, tx1, ty1) = (clamp_x(used[0]), clamp_y(used[1]), clamp_x(used[2]), clamp_y(used[3]));
        let clamp_x = move |v: f64| clamp_x(v).clamp(tx0, tx1);
        let clamp_y = move |v: f64| clamp_y(v).clamp(ty0, ty1);
        let (tw, th) = (tx1 - tx0 + 1, ty1 - ty0 + 1);
        // Two passes (count, then fill) into one flat list.
        let tiles = (tw * th) as usize;
        fn visit(t: &Tri, clamp_x: &dyn Fn(f64) -> u32, clamp_y: &dyn Fn(f64) -> u32, origin: (u32, u32), tw: u32, mut f: impl FnMut(usize)) {
            let b = t.bbox.map(|v| v as f64);
            let (ax, ay, bx, by) = (clamp_x(b[0]), clamp_y(b[1]), clamp_x(b[2]), clamp_y(b[3]));
            if ax == bx && ay == by {
                f(((ay - origin.1) * tw + (ax - origin.0)) as usize);
                return;
            }
            for ty in ay..=by {
                for tx in ax..=bx {
                    // Skip tiles the triangle misses (big thin triangles).
                    if touches(&t.rows, tx, ty) {
                        f(((ty - origin.1) * tw + (tx - origin.0)) as usize);
                    }
                }
            }
        }
        let origin = (tx0, ty0);
        // Each thread counts its chunk's triangles per tile, then fills its
        // own slots: tiles list triangles in batch order, as serially.
        let chunk = if tris.len() < 8192 || threads < 2 { tris.len().max(1) } else { tris.len().div_ceil(threads) };
        let chunks: Vec<&[Tri]> = tris.chunks(chunk).collect();
        let counts: Vec<Vec<u32>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = chunks
                .iter()
                .map(|c| {
                    let (clamp_x, clamp_y) = (&clamp_x, &clamp_y);
                    scope.spawn(move || {
                        let mut counts = vec![0u32; tiles];
                        for t in c.iter() {
                            visit(t, clamp_x, clamp_y, origin, tw, |k| counts[k] += 1);
                        }
                        counts
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().expect("binning")).collect()
        });
        let bins_base = bins.len();
        let header = tiles * 2;
        let total: usize = counts.iter().flatten().map(|c| *c as usize).sum();
        bins.resize(bins_base + header + total, 0);
        // Start of each (chunk, tile) run.
        let mut starts: Vec<Vec<u32>> = vec![vec![0; tiles]; chunks.len()];
        let mut rel = header as u32;
        for k in 0..tiles {
            bins[bins_base + k * 2] = rel;
            let mut n = 0;
            for (c, cs) in counts.iter().enumerate() {
                starts[c][k] = rel + n;
                n += cs[k];
            }
            bins[bins_base + k * 2 + 1] = n;
            rel += n;
        }
        {
            let out = &mut bins[bins_base..];
            let addr = out.as_mut_ptr() as usize;
            std::thread::scope(|scope| {
                for (c, (ts, mut next)) in chunks.iter().zip(starts).enumerate() {
                    let (clamp_x, clamp_y) = (&clamp_x, &clamp_y);
                    let first = c * chunk;
                    scope.spawn(move || {
                        let out = addr as *mut u32;
                        for (i, t) in ts.iter().enumerate() {
                            visit(t, clamp_x, clamp_y, origin, tw, |k| {
                                // SAFETY: (chunk, tile) runs are disjoint and inside `out`.
                                unsafe { *out.add(next[k] as usize) = (first + i) as u32 };
                                next[k] += 1;
                            });
                        }
                    });
                }
            });
        }
        let bins_base = bins_base as u32;
        let tri_base = mesh_rows.len() as u32;
        for t in &tris {
            mesh_rows.extend_from_slice(&t.rows);
        }
        let vert_base = mesh_rows.len() as u32;
        for v in &self.verts {
            mesh_rows.push([v.pos[0], v.pos[1], v.pos[2], 1.0 / v.pos[2]]);
            mesh_rows.push([v.normal[0], v.normal[1], v.normal[2], v.aux[3]]);
            mesh_rows.push([v.aux[0], v.aux[1], v.aux[2], v.aux[4]]);
        }
        let mat_base = mesh_rows.len() as u32;
        mesh_rows.extend_from_slice(&self.materials);
        let light_base = mesh_rows.len() as u32;
        mesh_rows.extend_from_slice(&self.light_rows);
        match &self.env {
            Some(env) => {
                let key = Arc::as_ptr(env) as usize;
                let at = *placed.entry(key).or_insert_with(|| {
                    let at = mesh_rows.len() as u32;
                    mesh_rows.extend_from_slice(&env.raster);
                    at
                });
                mesh_rows[light_base as usize + 5][3] = f32::from_bits((at as i64 - light_base as i64) as i32 as u32);
            }
            // Its image did not load: the environment is dark.
            None => mesh_rows[light_base as usize][2] = 0.0,
        }
        // Shadow maps (shared by the list's batches: written once), linked
        // from each light's colour row as an offset from the light block.
        for (i, map) in self.shadows.iter().enumerate() {
            let Some(map) = map else { continue };
            let key = Arc::as_ptr(map) as usize;
            let at = *placed.entry(key).or_insert_with(|| {
                let at = mesh_rows.len() as u32;
                mesh_rows.extend_from_slice(&map.rows);
                at
            });
            let row = light_base as usize + 7 + i * LROWS + 1;
            mesh_rows[row][3] = f32::from_bits((at as i64 - light_base as i64) as i32 as u32);
        }
        let b = |v: u32| f32::from_bits(v);
        // Glass: target pixel p → backdrop texel a + b·p, and output px per
        // unit of x/z for refraction offsets.
        let (paint, paint2, stroke) = match self.backdrop {
            Some(bd) => (
                [b(bd.offset), b(bd.size.0), b(bd.size.1), 1.0],
                [
                    ((target_origin[0] - bd.origin[0]) / bd.factor[0]) as f32,
                    ((target_origin[1] - bd.origin[1]) / bd.factor[1]) as f32,
                    (factor[0] / bd.factor[0]) as f32,
                    (factor[1] / bd.factor[1]) as f32,
                ],
                [(self.focal / factor[0]) as f32, (self.focal / factor[1]) as f32, 0.0, 0.0],
            ),
            None => ([0.0; 4], [0.0; 4], [0.0; 4]),
        };
        let mut stroke = stroke;
        stroke[2] = b(0);
        let prim = Prim {
            kind: [PRIM_MESH, 0, blend, 0],
            paint,
            paint2,
            stroke,
            // Target pixel → output pixel (cards look their pictures up there).
            fill: [target_origin[0] as f32, target_origin[1] as f32, factor[0] as f32, factor[1] as f32],
            inv_x: [1.0, 0.0, 0.0, 1.0],
            inv_y: [0.0, 1.0, 0.0, 1.0],
            inv_w: [0.0, 0.0, 1.0, 1.0],
            geom: [b(bins_base), b(tx0), b(ty0), b(tw)],
            geom2: [b(tri_base), b(vert_base), b(mat_base), b(light_base)],
            ..Default::default()
        };
        let bounds = [(tx0 * TILE) as f64, (ty0 * TILE) as f64, ((tx1 + 1) * TILE) as f64 - 1.0, ((ty1 + 1) * TILE) as f64 - 1.0];
        Some((prim, bounds))
    }
}

/// Whether a triangle (edge rows) can cover any sample of tile (tx, ty).
fn touches(rows: &[[f32; 4]; 4], tx: u32, ty: u32) -> bool {
    let (x0, y0) = ((tx * TILE) as f32, (ty * TILE) as f32);
    let (x1, y1) = (x0 + TILE as f32, y0 + TILE as f32);
    let p = [[rows[0][0], rows[0][1]], [rows[0][2], rows[0][3]], [rows[1][0], rows[1][1]]];
    let sign = rows[1][2];
    (0..3).all(|k| {
        // Inside is where sign·cross(b − a, q − a) ≥ 0; its largest value
        // over the tile's corners.
        let (a, b) = (p[(k + 1) % 3], p[(k + 2) % 3]);
        let (ex, ey) = ((b[0] - a[0]) * sign, (b[1] - a[1]) * sign);
        let at = |x: f32, y: f32| ex * (y - a[1]) - ey * (x - a[0]);
        at(x0, y0).max(at(x1, y0)).max(at(x0, y1)).max(at(x1, y1)) >= -1e-2
    })
}

/// Edge function of a→b at q, computed the same way whichever triangle
/// asks (endpoints in a fixed order), so neighbours share edges exactly:
/// no sample falls through the crack between two triangles.
fn edge(a: [f32; 2], b: [f32; 2], q: [f32; 2]) -> f32 {
    if a[0] < b[0] || (a[0] == b[0] && a[1] < b[1]) {
        (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])
    } else {
        -((a[0] - b[0]) * (q[1] - b[1]) - (a[1] - b[1]) * (q[0] - b[0]))
    }
}

/// Whether a sample on an edge belongs to the triangle (one side of a
/// shared edge only): `d` is the edge's direction in positive winding.
fn owns(d: [f32; 2]) -> bool {
    d[1] > 0.0 || (d[1] == 0.0 && d[0] < 0.0)
}

/// Positive-inside edge values of triangle rows at q and whether q is
/// covered.
fn cover(r0: [f32; 4], r1: [f32; 4], q: [f32; 2]) -> ([f32; 3], bool) {
    let p = [[r0[0], r0[1]], [r0[2], r0[3]], [r1[0], r1[1]]];
    let sign = r1[2];
    let mut e = [0.0f32; 3];
    let mut inside = true;
    for k in 0..3 {
        let (a, b) = (p[(k + 1) % 3], p[(k + 2) % 3]);
        e[k] = edge(a, b, q) * sign;
        if e[k] < 0.0 || (e[k] == 0.0 && !owns([(b[0] - a[0]) * sign, (b[1] - a[1]) * sign])) {
            inside = false;
        }
    }
    (e, inside)
}

type V3 = [f64; 3];
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

// ------------------------------------------------------------ CPU shading

/// Relative depth within which the later surface wins (coplanar layers).
pub const TIE: f32 = 2e-5;

/// Sample offsets within a pixel (the standard 8× pattern, in 1/16 px).
pub const SAMPLES: [[f32; 2]; 8] = [[1.0, -3.0], [-1.0, 3.0], [5.0, 1.0], [-3.0, -5.0], [-5.0, 5.0], [-7.0, -1.0], [3.0, 7.0], [7.0, -7.0]];

type F3 = [f32; 3];
fn f3(v: [f32; 4]) -> F3 {
    [v[0], v[1], v[2]]
}
fn fadd(a: F3, b: F3) -> F3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn fsub(a: F3, b: F3) -> F3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn fmul(a: F3, k: f32) -> F3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn fmul3(a: F3, b: F3) -> F3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
fn fdot(a: F3, b: F3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn fcross(a: F3, b: F3) -> F3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn fnorm(a: F3) -> F3 {
    fmul(a, 1.0 / fdot(a, a).sqrt().max(1e-20))
}
fn fmix(a: F3, b: F3, t: f32) -> F3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Environment image lookups (mirror motion.wgsl's env_image*).
fn env_uv(m: &[[f32; 4]], lb: usize, d: F3) -> [f32; 2] {
    let phi = d[0].atan2(d[2]) + m[lb + 4][3];
    let u = 0.5 + phi / (2.0 * std::f32::consts::PI);
    [u - u.floor(), (-d[1]).clamp(-1.0, 1.0).acos() / std::f32::consts::PI]
}

fn env_tex(m: &[[f32; 4]], at: usize, w: usize, h: usize, uv: [f32; 2]) -> F3 {
    let x = uv[0] * w as f32 - 0.5;
    let y = (uv[1] * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let xf = x.floor();
    let fx = x - xf;
    let y0 = y.floor() as usize;
    let y1 = (y0 + 1).min(h - 1);
    let fy = y - y.floor();
    let x0 = ((xf as i64 + w as i64) as usize) % w;
    let x1 = (x0 + 1) % w;
    let a = fmix(f3(m[at + y0 * w + x0]), f3(m[at + y0 * w + x1]), fx);
    let b = fmix(f3(m[at + y1 * w + x0]), f3(m[at + y1 * w + x1]), fx);
    fmix(a, b, fy)
}

fn env_image(m: &[[f32; 4]], lb: usize, d: F3, r: f32) -> F3 {
    let e = (lb as i64 + m[lb + 5][3].to_bits() as i32 as i64) as usize;
    let uv = env_uv(m, lb, d);
    let n = m[e][0] as usize;
    let mut k = 0;
    for i in 1..n.saturating_sub(1) {
        if m[e + 1 + i][3] <= r {
            k = i;
        }
    }
    let (la, lb2) = (m[e + 1 + k], m[e + 1 + (k + 1).min(n - 1)]);
    let a = env_tex(m, e + la[0] as usize, la[1] as usize, la[2] as usize, uv);
    let b = env_tex(m, e + lb2[0] as usize, lb2[1] as usize, lb2[2] as usize, uv);
    let t = ((r - la[3]) / (lb2[3] - la[3]).max(1e-4)).clamp(0.0, 1.0);
    fmul(fmix(a, b, t), m[lb][3])
}

fn env_image_diffuse(m: &[[f32; 4]], lb: usize, n: F3) -> F3 {
    let e = (lb as i64 + m[lb + 5][3].to_bits() as i32 as i64) as usize;
    let d = m[e];
    fmul(env_tex(m, e + d[1] as usize, d[2] as usize, d[3] as usize, env_uv(m, lb, n)), m[lb][3])
}

/// The environment's radiance toward world direction `d`, blurred for
/// roughness `r`.
fn env(m: &[[f32; 4]], lb: usize, d: F3, r: f32) -> F3 {
    if m[lb][2] > 0.5 {
        return env_image(m, lb, d, r);
    }
    let up = -d[1];
    let sky = f3(m[lb + 4]);
    let horizon = f3(m[lb + 5]);
    let ground = f3(m[lb + 6]);
    let mut c = if up >= 0.0 { fmix(horizon, sky, smoothstep(0.0, 0.6 + r, up)) } else { fmix(horizon, ground, smoothstep(0.0, 0.2 + r, -up)) };
    let lights = m[lb][0] as usize;
    let lobes = m[lb][1] as usize;
    let first = lb + 7 + lights * LROWS;
    for i in 0..lobes {
        let l = m[first + i * 2];
        let s2 = l[3] * l[3];
        let w2 = s2 + r * r * 1.2;
        let k = ((fdot(d, f3(l)) - 1.0) / w2).exp() * s2 / w2;
        c = fadd(c, fmul(f3(m[first + i * 2 + 1]), k));
    }
    c
}

/// Irradiance / π from the environment on a matte surface facing world
/// direction n (mirrors `env_diffuse` in motion.wgsl).
fn env_diffuse(m: &[[f32; 4]], lb: usize, n: F3) -> F3 {
    if m[lb][2] > 0.5 {
        return env_image_diffuse(m, lb, n);
    }
    let up = -n[1];
    let (sky, horizon, ground) = (f3(m[lb + 4]), f3(m[lb + 5]), f3(m[lb + 6]));
    let mut c = if up >= 0.0 { fmix(horizon, sky, smoothstep(0.0, 1.6, up)) } else { fmix(horizon, ground, smoothstep(0.0, 1.2, -up)) };
    let lights = m[lb][0] as usize;
    let lobes = m[lb][1] as usize;
    let first = lb + 7 + lights * LROWS;
    for i in 0..lobes {
        let l = m[first + i * 2];
        let lam = 1.0 / (l[3] * l[3]).max(1e-6);
        let mu = fdot(n, f3(l));
        let eml = (-lam).exp();
        let em2l = eml * eml;
        let rl = 1.0 / lam;
        let scale = 1.0 + 2.0 * em2l - rl;
        let bias = (eml - em2l) * rl - em2l;
        let x = (1.0 - scale).max(0.0).sqrt();
        let x0 = 0.36 * mu;
        let x1 = x / (4.0 * 0.36);
        let nn = x0 + x1;
        let y = if x0.abs() <= x1 { nn * nn / x.max(1e-6) } else { mu.clamp(0.0, 1.0) };
        let e = (scale * y + bias).max(0.0);
        c = fadd(c, fmul(f3(m[first + i * 2 + 1]), e * 2.0 * rl * (1.0 - em2l)));
    }
    c
}

/// Whether a texel's surface leaves a point at closeness `c` lit.
fn shadow_lit(s: f32, c: f32, bias: f32, point: bool) -> f32 {
    if s <= NO_SURFACE * 0.5 {
        return 1.0;
    }
    let lit = if point { s <= 0.0 || 1.0 / s >= c * (1.0 - bias) } else { s <= c + bias };
    if lit {
        1.0
    } else {
        0.0
    }
}

/// How much of a light reaches camera-space point `p` (normal `n`) past
/// the shadow map at row `base`: 1 lit … 0 in shadow, soft at the edges.
fn shadow(m: &[[f32; 4]], base: usize, p: F3, n: F3) -> f32 {
    let (h0, h1) = (m[base], m[base + 1]);
    let (u, v, a, pos) = (m[base + 2], m[base + 3], m[base + 4], m[base + 5]);
    let point = h0[0] > 0.5;
    let size = h0[1] as i32;
    let mut offset = u[3];
    if point {
        offset *= fdot(fsub(p, f3(pos)), f3(a)).max(1.0);
    }
    // Step off the surface toward the light (whichever side faces it).
    let toward = fnorm(if point { fsub(f3(pos), p) } else { f3(a) });
    let n = if fdot(n, toward) < 0.0 { fmul(n, -1.0) } else { n };
    let po = fadd(p, fmul(n, offset));
    // The filter reaches `radius` texels across a surface sloping away
    // from the light: bias by the depth it climbs there.
    let cos = fdot(n, toward).max(0.05);
    let slope = ((1.0 - cos * cos).max(0.0).sqrt() / cos).min(4.0);
    // (Point maps compare distances as fractions: texels are angles.)
    let bias = h1[2] + u[3] * h1[3] * slope;
    let (tu, tv, c) = if point {
        let q = fsub(po, f3(pos));
        let z = fdot(q, f3(a));
        if z < 1.0 {
            return 1.0;
        }
        (fdot(q, f3(u)) / z, fdot(q, f3(v)) / z, z)
    } else {
        (fdot(po, f3(u)), fdot(po, f3(v)), fdot(po, f3(a)))
    };
    let x = (tu - h0[2]) * h1[0] - 0.5;
    let y = (tv - h0[3]) * h1[1] - 0.5;
    let texel = |i: i32, j: i32| -> f32 {
        if i < 0 || j < 0 || i >= size || j >= size {
            return 1.0;
        }
        let k = (j * size + i) as usize;
        shadow_lit(m[base + 6 + k / 4][k % 4], c, bias, point)
    };
    let mut sum = 0.0;
    for t in SHADOW_TAPS {
        let (sx, sy) = (x + t[0] * h1[3], y + t[1] * h1[3]);
        let (fx, fy) = (sx.floor(), sy.floor());
        let (ax, ay) = (sx - fx, sy - fy);
        let (i, j) = (fx as i32, fy as i32);
        let top = texel(i, j) + (texel(i + 1, j) - texel(i, j)) * ax;
        let bottom = texel(i, j + 1) + (texel(i + 1, j + 1) - texel(i, j + 1)) * ax;
        sum += top + (bottom - top) * ay;
    }
    sum / SHADOW_TAPS.len() as f32
}

/// The shadow map linked from light `i`'s colour row, if any.
fn shadow_of(m: &[[f32; 4]], lb: usize, i: usize) -> Option<usize> {
    let off = m[lb + 7 + i * LROWS + 1][3].to_bits() as i32;
    (off != 0).then(|| (lb as i64 + off as i64) as usize)
}

fn to_world(m: &[[f32; 4]], lb: usize, v: F3) -> F3 {
    [fdot(f3(m[lb + 1]), v), fdot(f3(m[lb + 2]), v), fdot(f3(m[lb + 3]), v)]
}

fn ggx(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * d * d).max(1e-12)
}

fn smith(n_v: f32, n_l: f32, r: f32) -> f32 {
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    n_v / (n_v * (1.0 - k) + k) * (n_l / (n_l * (1.0 - k) + k))
}

/// Split-sum environment BRDF (Karis' analytic fit): (scale, bias) on F0.
fn env_brdf(r: f32, n_v: f32) -> [f32; 2] {
    let c0 = [-1.0, -0.0275, -0.572, 0.022];
    let c1 = [1.0, 0.0425, 1.04, -0.04];
    let rr: [f32; 4] = std::array::from_fn(|i| r * c0[i] + c1[i]);
    let a004 = (rr[0] * rr[0]).min((-9.28 * n_v).exp2()) * rr[0] + rr[1];
    [-1.04 * a004 + rr[2], 1.04 * a004 + rr[3]]
}

/// An area light seen from p (mirrors `area_light` in motion.wgsl).
struct AreaSample {
    l: F3,
    ls: F3,
    sa: f32,
    e: f32,
}

fn area_light(m: &[[f32; 4]], row: usize, p: F3, n: F3, v: F3, alpha: f32) -> Option<AreaSample> {
    let c = f3(m[row]);
    let (u, w) = (m[row + 3], m[row + 4]);
    let nl = fnorm(fcross(f3(u), f3(w)));
    let area = w[3].max(1.0);
    let dl = fsub(c, p);
    let d2 = fdot(dl, dl).max(1e-6);
    let dist = d2.sqrt();
    let l = fmul(dl, 1.0 / dist);
    let cos_l = -fdot(l, nl);
    if cos_l <= 0.0 && fdot(fsub(p, c), nl) <= 0.0 {
        return None;
    }
    let pi = std::f32::consts::PI;
    let e = 1e6 * cos_l.max(0.0) / (d2 + area / pi) * range_window(dist, m[row + 2][0]);
    let r = fsub(fmul(n, 2.0 * fdot(n, v)), v);
    let den = fdot(r, nl);
    let mut q = c;
    if den < -1e-4 {
        let t = fdot(fsub(c, p), nl) / den;
        if t > 0.0 {
            q = fadd(p, fmul(r, t));
        }
    }
    let rel = fsub(q, c);
    let mut x = fdot(rel, f3(u)) / fdot(f3(u), f3(u)).max(1e-6);
    let mut y = fdot(rel, f3(w)) / fdot(f3(w), f3(w)).max(1e-6);
    if u[3] > 0.5 {
        let rr = (x * x + y * y).sqrt();
        if rr > 1.0 {
            x /= rr;
            y /= rr;
        }
    } else {
        x = x.clamp(-1.0, 1.0);
        y = y.clamp(-1.0, 1.0);
    }
    let qs = fadd(fadd(c, fmul(f3(u), x)), fmul(f3(w), y));
    Some(AreaSample { l, ls: fnorm(fsub(qs, p)), sa: (alpha + (area / pi).sqrt() / (3.0 * dist)).min(1.0), e })
}

/// A mesh map texel (wrapping), raw 0..1 (mirrors `map_texel`).
fn map_texel(images: &[u32], r: [f32; 4], x: i32, y: i32) -> [f32; 4] {
    let (w, h) = (r[1].to_bits() as i32, r[2].to_bits() as i32);
    let (xx, yy) = (((x % w) + w) % w, ((y % h) + h) % h);
    let v = images.get(r[0].to_bits() as usize + (yy * w + xx) as usize).copied().unwrap_or(0);
    [(v & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, ((v >> 16) & 255) as f32 / 255.0, ((v >> 24) & 255) as f32 / 255.0]
}

fn map_sample(images: &[u32], r: [f32; 4], t: [f32; 2]) -> [f32; 4] {
    let s = [t[0] * r[1].to_bits() as f32 - 0.5, t[1] * r[2].to_bits() as f32 - 0.5];
    let (f0, f1) = (s[0].floor(), s[1].floor());
    let (fx, fy) = (s[0] - f0, s[1] - f1);
    let (x, y) = (f0 as i32, f1 as i32);
    let m = |a: [f32; 4], b: [f32; 4], t: f32| -> [f32; 4] { std::array::from_fn(|c| a[c] * (1.0 - t) + b[c] * t) };
    m(m(map_texel(images, r, x, y), map_texel(images, r, x + 1, y), fx), m(map_texel(images, r, x, y + 1), map_texel(images, r, x + 1, y + 1), fx), fy)
}

fn map_height(images: &[u32], r: [f32; 4], t: [f32; 2]) -> f32 {
    let c = map_sample(images, r, t);
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// A material's surface detail (mirrors `surface_detail` in motion.wgsl):
/// colour, roughness change, bent normal.
#[allow(clippy::too_many_arguments)]
fn surface_detail(m: &[[f32; 4]], images: &[u32], row: usize, pos: [F3; 3], nps: [F3; 3], uvs_in: [[f32; 2]; 3], b: F3, n_in: F3, base_in: F3) -> (F3, f32, F3) {
    let (r9, r12, r13, r14) = (m[row + 9], m[row + 12], m[row + 13], m[row + 14]);
    let mut base = base_in;
    let mut rough = 0.0f32;
    if r9[0] < 0.5 && r12[3] < 0.5 && r13[3] < 0.5 && r14[3] == 0.0 {
        return (base, rough, n_in);
    }
    let e1 = fsub(pos[1], pos[0]);
    let e2 = fsub(pos[2], pos[0]);
    let mut n = n_in;
    if r12[3] > 0.5 || r13[3] > 0.5 || r14[3] != 0.0 {
        let mut uvs = uvs_in;
        let wraps = m[row + 16][1] as u32;
        for c in 0..2 {
            if wraps & (1 << c) == 0 {
                continue;
            }
            for k in 1..3 {
                if uvs[k][c] - uvs[0][c] > 0.5 {
                    uvs[k][c] -= 1.0;
                }
                if uvs[k][c] - uvs[0][c] < -0.5 {
                    uvs[k][c] += 1.0;
                }
            }
        }
        let r15 = m[row + 15];
        let ang = m[row + 16][0];
        let (cs, sn) = (ang.cos(), ang.sin());
        for uv in &mut uvs {
            let c = [uv[0] - 0.5, uv[1] - 0.5];
            *uv = [(c[0] * cs - c[1] * sn) * r15[0] + 0.5 + r15[2], (c[0] * sn + c[1] * cs) * r15[1] + 0.5 + r15[3]];
        }
        let t = [uvs[0][0] * b[0] + uvs[1][0] * b[1] + uvs[2][0] * b[2], uvs[0][1] * b[0] + uvs[1][1] * b[1] + uvs[2][1] * b[2]];
        let d1 = [uvs[1][0] - uvs[0][0], uvs[1][1] - uvs[0][1]];
        let d2 = [uvs[2][0] - uvs[0][0], uvs[2][1] - uvs[0][1]];
        let det = d1[0] * d2[1] - d2[0] * d1[1];
        let (mut dpdu, mut dpdv) = ([0.0f32; 3], [0.0f32; 3]);
        if det.abs() > 1e-12 {
            dpdu = fmul(fsub(fmul(e1, d2[1]), fmul(e2, d1[1])), 1.0 / det);
            dpdv = fmul(fsub(fmul(e2, d1[0]), fmul(e1, d2[0])), 1.0 / det);
        }
        if r12[3] > 0.5 {
            let c = map_sample(images, r12, t);
            let straight = if c[3] > 0.0 { [c[0] / c[3], c[1] / c[3], c[2] / c[3]] } else { [0.0; 3] };
            base = fmul3(base, straight.map(srgb_decode));
        }
        let (lu, lv) = (fdot(dpdu, dpdu).sqrt(), fdot(dpdv, dpdv).sqrt());
        if lu > 1e-9 && lv > 1e-9 {
            let tu = fnorm(fsub(dpdu, fmul(n, fdot(n, dpdu))));
            let tv = fnorm(fsub(fsub(dpdv, fmul(n, fdot(n, dpdv))), fmul(tu, fdot(tu, dpdv))));
            if r13[3] > 0.5 {
                let c = map_sample(images, r13, t);
                let mm = [c[0] * 2.0 - 1.0, c[1] * 2.0 - 1.0, c[2] * 2.0 - 1.0];
                n = fnorm(fadd(fsub(fmul(tu, mm[0]), fmul(tv, mm[1])), fmul(n, mm[2].max(0.05))));
            }
            if r14[3] != 0.0 {
                let (su, sv) = (1.0 / r14[1].to_bits() as f32, 1.0 / r14[2].to_bits() as f32);
                let dhdu = (map_height(images, r14, [t[0] + su, t[1]]) - map_height(images, r14, [t[0] - su, t[1]])) / (2.0 * su);
                let dhdv = (map_height(images, r14, [t[0], t[1] + sv]) - map_height(images, r14, [t[0], t[1] - sv])) / (2.0 * sv);
                n = fnorm(fsub(n, fmul(fadd(fmul(tu, dhdu / lu), fmul(tv, dhdv / lv)), r14[3])));
            }
        }
    }
    if r9[0] > 0.5 {
        use lahza_motion_core::noise::sample;
        let kind = (r9[0] + 0.5) as u32;
        // Core codes: 0 value, 1 fbm, 2 cellular; the rows store code + 1.
        let code = kind - 1;
        let (inv, octaves, seed) = (r9[1], r9[2] as u32, r9[3].to_bits());
        let q = fmul(fadd(fadd(fmul(nps[0], b[0]), fmul(nps[1], b[1])), fmul(nps[2], b[2])), inv);
        let v = sample(code, q, octaves, seed);
        let (r10, r11) = (m[row + 10], m[row + 11]);
        base = std::array::from_fn(|c| base[c] * (1.0 - r10[3] * v) + r10[c] * (r10[3] * v));
        rough = r11[0] * (2.0 * v - 1.0);
        if r11[1] != 0.0 {
            let f1 = fsub(nps[1], nps[0]);
            let f2 = fsub(nps[2], nps[0]);
            let (g11, g12, g22) = (fdot(e1, e1), fdot(e1, e2), fdot(e2, e2));
            let gd = g11 * g22 - g12 * g12;
            if gd.abs() > 1e-12 {
                let t1 = fnorm(fsub(e1, fmul(n_in, fdot(n_in, e1))));
                let t2 = fcross(n_in, t1);
                let h = 0.15 / inv.max(1e-6);
                let mut grad = [0.0f32; 3];
                for tk in [t1, t2] {
                    let (c1, c2) = (fdot(e1, tk), fdot(e2, tk));
                    let a = (c1 * g22 - c2 * g12) / gd;
                    let bb = (c2 * g11 - c1 * g12) / gd;
                    let step = fmul(fadd(fmul(f1, a), fmul(f2, bb)), h * inv);
                    let dv = (sample(code, fadd(q, step), octaves, seed) - v) / h;
                    grad = fadd(grad, fmul(tk, dv));
                }
                n = fnorm(fsub(n, fmul(grad, r11[1])));
            }
        }
    }
    (base, rough, n)
}

/// A point light's fade to its range (0: none). Mirrors motion.wgsl.
pub(crate) fn range_window(dist: f32, range: f32) -> f32 {
    if range <= 0.0 {
        return 1.0;
    }
    let x = dist / range;
    let w = (1.0 - x * x * x * x).clamp(0.0, 1.0);
    w * w
}

/// A tangent perpendicular to n (any). Mirrors motion.wgsl.
fn basis_t(n: F3) -> F3 {
    let a = if n[0].abs() > 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    fnorm(fcross(a, n))
}

/// Anisotropic GGX (h in the t, b, n frame).
fn ggx_aniso(h_t: f32, h_b: f32, n_h: f32, at: f32, ab: f32) -> f32 {
    let (x, y) = (h_t / at, h_b / ab);
    let d = x * x + y * y + n_h * n_h;
    1.0 / (std::f32::consts::PI * at * ab * d * d).max(1e-12)
}

/// Schlick with the F82 edge tint (Hoffman; Blender's metallic Fresnel).
fn fresnel_f82(f0: F3, f82: F3, f90: f32, mu: f32) -> F3 {
    let m = mu.clamp(0.0, 1.0);
    let x = 1.0 - m;
    let w = x * x * x * x * x;
    let k = m * x * x * x * x * x * x;
    std::array::from_fn(|c| {
        let s = f0[c] + (f90 - f0[c]) * w;
        let sb = f0[c] + (f90 - f0[c]) * 0.462_664_37;
        let a = sb * (1.0 - f82[c]) / 0.056_652_7;
        (s - a * k).max(0.0)
    })
}

/// Sheen: Charlie distribution (alpha = roughness²).
fn charlie(n_h: f32, a: f32) -> f32 {
    let inv = 1.0 / a;
    let s2 = (1.0 - n_h * n_h).max(0.007_812_5);
    (2.0 + inv) * s2.powf(inv * 0.5) / (2.0 * std::f32::consts::PI)
}

fn neubelt(n_v: f32, n_l: f32) -> f32 {
    1.0 / (4.0 * (n_l + n_v - n_l * n_v)).max(1e-4)
}

/// Directional albedo of the sheen lobe: roughness 0.05..1 × cos 0..1.
pub(crate) const SHEEN_E: [f32; 64] = [
    1.0000, 0.1364, 0.0009, 0.0000, 0.0000, 0.0000, 0.0000, 0.0000, 1.0000, 0.6862, 0.2519, 0.0837, 0.0229, 0.0044, 0.0004, 0.0000, //
    1.0000, 0.6499, 0.3698, 0.2068, 0.1081, 0.0498, 0.0177, 0.0026, 0.8886, 0.5849, 0.3937, 0.2658, 0.1739, 0.1065, 0.0570, 0.0217, //
    0.7538, 0.5358, 0.3933, 0.2921, 0.2137, 0.1504, 0.0981, 0.0545, 0.6738, 0.5014, 0.3874, 0.3044, 0.2377, 0.1814, 0.1323, 0.0885, //
    0.6227, 0.4772, 0.3811, 0.3105, 0.2530, 0.2033, 0.1589, 0.1180, 0.5882, 0.4599, 0.3757, 0.3138, 0.2631, 0.2190, 0.1790, 0.1418,
];

fn sheen_albedo(n_v: f32, r: f32) -> f32 {
    let x = n_v.clamp(0.0, 1.0) * 7.0;
    let y = ((r - 0.05) / 0.95).clamp(0.0, 1.0) * 7.0;
    let (x0, y0) = ((x as usize).min(6), (y as usize).min(6));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let e = &SHEEN_E;
    let a = e[y0 * 8 + x0] + (e[y0 * 8 + x0 + 1] - e[y0 * 8 + x0]) * fx;
    let b = e[(y0 + 1) * 8 + x0] + (e[(y0 + 1) * 8 + x0 + 1] - e[(y0 + 1) * 8 + x0]) * fx;
    a + (b - a) * fy
}

fn shoulder(x: f32) -> f32 {
    if x < 0.8 {
        x
    } else {
        0.8 + 0.2 * (1.0 - (-(x - 0.8) / 0.2).exp())
    }
}

fn encode(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// Premultiplied sRGB colour of triangle `tri` at target point `q`.
fn srgb_decode(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// Bilinear sample of the glass backdrop group at texel position `g`.
fn sample_group(groups: &[[f32; 4]], prim: &Prim, g: [f32; 2]) -> [f32; 4] {
    let off = prim.paint[0].to_bits() as i64;
    let (w, h) = (prim.paint[1].to_bits() as i64, prim.paint[2].to_bits() as i64);
    let s = [g[0] - 0.5, g[1] - 0.5];
    let (x0, y0) = (s[0].floor(), s[1].floor());
    let (fx, fy) = (s[0] - x0, s[1] - y0);
    let at = |x: i64, y: i64| groups[(off + y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        top + (bottom - top) * fy
    })
}

/// Distance in px from `q` to the triangle's nearest real edge (`mask`
/// bits: edges opposite each vertex), given its positive-inside edge values.
fn wire_distance(r0: [f32; 4], r1: [f32; 4], e: [f32; 3], mask: u32) -> f32 {
    let p = [[r0[0], r0[1]], [r0[2], r0[3]], [r1[0], r1[1]]];
    let mut d = f32::MAX;
    for k in 0..3 {
        if mask & (1 << k) != 0 {
            let (a, b) = (p[(k + 1) % 3], p[(k + 2) % 3]);
            let len = ((b[0] - a[0]) * (b[0] - a[0]) + (b[1] - a[1]) * (b[1] - a[1])).sqrt().max(1e-6);
            d = d.min(e[k] / len);
        }
    }
    d
}

/// Whether a sample stands on a wireframe triangle: on a wire, or on its
/// face when the wireframe is filled.
fn wire_stands(m: &[[f32; 4]], material: usize, r0: [f32; 4], r1: [f32; 4], tri_flags: u32, q: [f32; 2]) -> bool {
    if m[material + 1][3] > 0.0 {
        return true;
    }
    let (e, _) = cover(r0, r1, q);
    wire_distance(r0, r1, e, tri_flags >> 2) <= m[material + 3][3] * 0.5
}

/// A card's picture at target point `q` (transparent outside it).
fn card_sample(prim: &Prim, m: &[[f32; 4]], groups: &[[f32; 4]], material: usize, q: [f32; 2]) -> [f32; 4] {
    let (r0, r1) = (m[material], m[material + 1]);
    let c = [prim.fill[0] + prim.fill[2] * q[0], prim.fill[1] + prim.fill[3] * q[1]];
    let g = [(c[0] - r0[0]) / r0[2] - 0.5, (c[1] - r0[1]) / r0[3] - 0.5];
    let off = prim.stroke[2].to_bits() as i64 + r1[0].to_bits() as i64;
    let (w, h) = (r1[1].to_bits() as i64, r1[2].to_bits() as i64);
    let (x0, y0) = (g[0].floor(), g[1].floor());
    let (fx, fy) = (g[0] - x0, g[1] - y0);
    let at = |x: i64, y: i64| if x < 0 || y < 0 || x >= w || y >= h { [0.0; 4] } else { groups[(off + y * w + x) as usize] };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, cc, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = cc[i] + (d[i] - cc[i]) * fx;
        top + (bottom - top) * fy
    })
}

fn shade_tri(prim: &Prim, m: &[[f32; 4]], groups: &[[f32; 4]], images: &[u32], tri: u32, q: [f32; 2]) -> [f32; 4] {
    let tb = prim.geom2[0].to_bits() as usize + tri as usize * 4;
    let vb = prim.geom2[1].to_bits() as usize;
    let mb = prim.geom2[2].to_bits() as usize;
    let lb = prim.geom2[3].to_bits() as usize;
    let mat_flags = m[mb + m[tb + 1][3].to_bits() as usize * MAT_ROWS + 2][3] as u32;
    if mat_flags & FLAG_OCCLUDER != 0 {
        // Another batch draws this surface.
        return [0.0; 4];
    }
    if mat_flags & FLAG_CARD != 0 {
        return card_sample(prim, m, groups, mb + m[tb + 1][3].to_bits() as usize * MAT_ROWS, q);
    }
    if mat_flags & FLAG_WIRE != 0 {
        let row = mb + m[tb + 1][3].to_bits() as usize * MAT_ROWS;
        let (e, _) = cover(m[tb], m[tb + 1], q);
        let on_wire = wire_distance(m[tb], m[tb + 1], e, m[tb + 2][3] as u32 >> 2) <= m[row + 3][3] * 0.5;
        let c = if on_wire { m[row] } else { m[row + 1] };
        let a = c[3];
        return [encode(c[0]) * a, encode(c[1]) * a, encode(c[2]) * a, a];
    }
    let (e, _) = cover(m[tb], m[tb + 1], q);
    let ids = m[tb + 2];
    let mut w = [0.0f32; 3];
    let mut pos = [[0.0f32; 3]; 3];
    let mut nrm = [[0.0f32; 3]; 3];
    for k in 0..3 {
        let v = ids[k].to_bits() as usize;
        let a = m[vb + v * VROWS];
        pos[k] = f3(a);
        nrm[k] = f3(m[vb + v * VROWS + 1]);
        w[k] = e[k].max(0.0) * a[3];
    }
    let s = (w[0] + w[1] + w[2]).max(1e-30);
    let b = w.map(|x| x / s);
    let p = fadd(fadd(fmul(pos[0], b[0]), fmul(pos[1], b[1])), fmul(pos[2], b[2]));
    let navg = fadd(fadd(fmul(nrm[0], b[0]), fmul(nrm[1], b[1])), fmul(nrm[2], b[2]));
    let mat = m[tb + 1][3].to_bits() as usize;
    let m0 = m[mb + mat * MAT_ROWS];
    let m1 = m[mb + mat * MAT_ROWS + 1];
    let m2 = m[mb + mat * MAT_ROWS + 2];
    let m3 = m[mb + mat * MAT_ROWS + 3];
    let flags = m2[3] as u32;
    let mut n = fnorm(navg);
    if flags & 2 != 0 {
        let g = fnorm(fcross(fsub(pos[1], pos[0]), fsub(pos[2], pos[0])));
        n = if fdot(g, navg) < 0.0 { fmul(g, -1.0) } else { g };
    }
    let v = fnorm(fmul(p, -1.0));
    if fdot(n, v) < 0.0 {
        n = fmul(n, -1.0);
    }
    let vid = |k: usize| ids[k].to_bits() as usize;
    let uvs = [0, 1, 2].map(|k| [m[vb + vid(k) * VROWS + 1][3], m[vb + vid(k) * VROWS + 2][3]]);
    let nps = [0, 1, 2].map(|k| f3(m[vb + vid(k) * VROWS + 2]));
    let (base, rough_d, nd) = surface_detail(m, images, mb + mat * MAT_ROWS, pos, nps, uvs, b, n, f3(m0));
    n = nd;
    let opacity = m0[3];
    let emissive = f3(m2);
    if flags & FLAG_SHADOW_CATCHER != 0 {
        // Only the shadows: how much of the shadowing light is blocked.
        let (mut blocked, mut total) = (0.0f32, 0.0f32);
        for i in 0..m[lb][0] as usize {
            if let Some(sm) = shadow_of(m, lb, i) {
                let w = fdot(f3(m[lb + 7 + i * LROWS + 1]), [0.2126, 0.7152, 0.0722]);
                blocked += w * (1.0 - shadow(m, sm, p, n));
                total += w;
            }
        }
        let a = if total > 0.0 { blocked / total * opacity } else { 0.0 };
        let c = base.map(encode);
        return [c[0] * a, c[1] * a, c[2] * a, a];
    }
    if flags & FLAG_UNLIT != 0 {
        let c = fadd(base, emissive).map(|x| encode(x));
        return [c[0] * opacity, c[1] * opacity, c[2] * opacity, opacity];
    }
    let (metal, rough, cc, ccr) = (m1[0], (m1[1] + rough_d).clamp(0.03, 1.0), m1[2], m1[3].max(0.03));
    let trans = m3[0];
    let row = |k: usize| m[mb + mat * MAT_ROWS + k];
    let (m4, m5, m6, m7, m8) = (row(4), row(5), row(6), row(7), row(8));
    // Mirrors motion.wgsl: F0 and the metal's edge tint (F82).
    let f0 = fmix(fmul(f3(m4), m3[3]), base, metal);
    let f82 = fmix([1.0; 3], f3(m4), metal);
    let f90 = fdot(f0, [16.5; 3]).clamp(0.0, 1.0);
    let n_v = fdot(n, v).max(1e-4);
    let ab = env_brdf(rough, n_v);
    let comp = fadd([1.0; 3], fmul(f0, 1.0 / (ab[0] + ab[1]).max(1e-3) - 1.0));
    let e_spec = fmul3(fadd(fmul(f0, ab[0]), [f90 * ab[1]; 3]), comp).map(|x| x.min(1.0));
    let sheen = f3(m5);
    let sheen_r = m5[3];
    let e_sheen = sheen_albedo(n_v, sheen_r) * sheen[0].max(sheen[1]).max(sheen[2]);
    let diffuse = fmul3(fmul(base, (1.0 - metal) * (1.0 - trans) * (1.0 - e_sheen)), fsub([1.0; 3], e_spec));
    let fc_v = 0.04 + 0.96 * (1.0 - n_v).powi(5);
    let coat = 1.0 - cc * fc_v;
    let coat_tint = fmix([1.0; 3], f3(m6), cc);
    let aniso = m4[3];
    let alpha = rough * rough;
    let (mut at, mut ab_) = (alpha, alpha);
    let (mut tg, mut bt) = ([0.0f32; 3], [0.0f32; 3]);
    if aniso > 0.0 {
        let mut t = fcross(f3(m7), fsub(p, f3(m8)));
        t = fsub(t, fmul(n, fdot(n, t)));
        if fdot(t, t) < 1e-12 {
            t = basis_t(n);
        }
        t = fnorm(t);
        t = fadd(fmul(t, m7[3].cos()), fmul(fcross(n, t), m7[3].sin()));
        tg = t;
        bt = fcross(n, t);
        let aspect = (1.0 - 0.9 * aniso).sqrt();
        at = alpha * aspect;
        ab_ = alpha / aspect;
    }
    let mut d_light = [0.0f32; 3];
    let mut s_light = [0.0f32; 3];
    let mut c_light = [0.0f32; 3];
    let lights = m[lb][0] as usize;
    for i in 0..lights {
        let row = m[lb + 7 + i * LROWS];
        let col = f3(m[lb + 7 + i * LROWS + 1]);
        let extra = m[lb + 7 + i * LROWS + 2];
        let kind = row[3] as u32;
        if kind == 2 {
            d_light = fadd(d_light, fmul3(diffuse, col));
            continue;
        }
        // Specular: toward ls with GGX alpha sa (× spec_norm keeps energy).
        let (l, ls, sa, col) = if kind == 1 {
            let dl = fsub(f3(row), p);
            let dist = fdot(dl, dl).sqrt();
            let l = fmul(dl, 1.0 / dist.max(1e-20));
            (l, l, alpha, fmul(col, range_window(dist, extra[0])))
        } else if kind == 3 {
            let Some(ar) = area_light(m, lb + 7 + i * LROWS, p, n, v, alpha) else { continue };
            (ar.l, ar.ls, ar.sa, fmul(col, ar.e))
        } else {
            (f3(row), f3(row), alpha, col)
        };
        let spec_norm = (alpha / sa) * (alpha / sa);
        let n_l = fdot(n, l);
        let n_ls = fdot(n, ls);
        if n_l <= 0.0 && n_ls <= 0.0 {
            continue;
        }
        let col = match shadow_of(m, lb, i) {
            Some(sm) => fmul(col, shadow(m, sm, p, n)),
            None => col,
        };
        if n_l > 0.0 {
            d_light = fadd(d_light, fmul3(diffuse, fmul(col, n_l * coat)));
            if e_sheen > 0.0 {
                let hd = fnorm(fadd(l, v));
                s_light = fadd(s_light, fmul(fmul3(sheen, col), charlie(fdot(n, hd).max(0.0), sheen_r * sheen_r) * neubelt(n_v, n_l) * n_l * std::f32::consts::PI * coat));
            }
        }
        if n_ls <= 0.0 {
            continue;
        }
        let h = fnorm(fadd(ls, v));
        let n_h = fdot(n, h).max(0.0);
        let v_h = fdot(v, h).max(0.0);
        let fw = (1.0 - v_h).powi(5);
        let f = fresnel_f82(f0, f82, f90, v_h);
        let d = if aniso > 0.0 { ggx_aniso(fdot(h, tg), fdot(h, bt), n_h, at * sa / alpha, ab_ * sa / alpha) * spec_norm } else { ggx(n_h, sa) * spec_norm };
        let spec = d * smith(n_v, n_ls, rough) / (4.0 * n_v * n_ls).max(1e-4) * std::f32::consts::PI;
        s_light = fadd(s_light, fmul3(fmul(fmul3(f, comp), spec * n_ls * coat), col));
        if cc > 0.0 {
            let fc = 0.04 + 0.96 * fw;
            let ca = (ccr * ccr).max(sa - alpha + ccr * ccr);
            let sc = ggx(n_h, ca) * ((ccr * ccr) / ca) * ((ccr * ccr) / ca) * smith(n_v, n_ls, ccr) / (4.0 * n_v * n_ls).max(1e-4) * std::f32::consts::PI;
            c_light = fadd(c_light, fmul(col, sc * fc * cc * n_ls));
        }
    }
    let nw = to_world(m, lb, n);
    let rn = if aniso > 0.0 {
        let at_dir = fcross(bt, v);
        fnorm(fmix(n, fcross(at_dir, bt), aniso))
    } else {
        n
    };
    let r_view = fsub(fmul(rn, 2.0 * fdot(rn, v)), v);
    let rw = to_world(m, lb, r_view);
    let env_d = env_diffuse(m, lb, nw);
    d_light = fadd(d_light, fmul(fmul3(diffuse, env_d), coat));
    let spec_env = fmul3(fmul3(env(m, lb, rw, rough), fadd(fmul(f0, ab[0]), fmul(f82, f90 * ab[1]))), comp);
    s_light = fadd(s_light, fmul(spec_env, coat));
    if e_sheen > 0.0 {
        s_light = fadd(s_light, fmul(fmul3(sheen, env_d), sheen_albedo(n_v, sheen_r) * coat));
    }
    if cc > 0.0 {
        let cr = fsub(fmul(n, 2.0 * fdot(n, v)), v);
        c_light = fadd(c_light, fmul(env(m, lb, to_world(m, lb, cr), ccr), fc_v * cc));
    }
    d_light = fmul3(d_light, coat_tint);
    s_light = fadd(fmul3(s_light, coat_tint), c_light);
    if trans > 0.0 && prim.paint[3] > 0.5 {
        // Refraction: what is behind, seen through the glass where the
        // bent ray leaves it, tinted by the colour.
        let ior = m3[1].max(1.0);
        let i = fmul(v, -1.0);
        let eta = 1.0 / ior;
        let cosi = fdot(n, v).clamp(0.0, 1.0);
        let k = 1.0 - eta * eta * (1.0 - cosi * cosi);
        let t = if k < 0.0 { fsub(i, fmul(n, 2.0 * fdot(n, i))) } else { fadd(fmul(i, eta), fmul(n, eta * cosi - k.sqrt())) };
        let exit = fadd(p, fmul(t, m3[2]));
        let z = exit[2].max(1.0);
        let d = [prim.stroke[0] * (exit[0] / z - p[0] / p[2]), prim.stroke[1] * (exit[1] / z - p[1] / p[2])];
        let g = [prim.paint2[0] + prim.paint2[2] * (q[0] + d[0]), prim.paint2[1] + prim.paint2[3] * (q[1] + d[1])];
        let behind = sample_group(groups, prim, g);
        let f0 = ((ior - 1.0) / (ior + 1.0)).powi(2);
        let fr = f0 + (1.0 - f0) * (1.0 - n_v).powi(5);
        let seen = if behind[3] > 0.0 { [0, 1, 2].map(|c| srgb_decode(behind[c] / behind[3]) * base[c]) } else { [0.0; 3] };
        let through = trans * (1.0 - fr) * behind[3];
        let lit = fadd(fadd(fmul(seen, through), fadd(d_light, emissive)), s_light);
        let peak = s_light[0].max(s_light[1]).max(s_light[2]).min(1.0);
        let a = (through + (1.0 - trans) + (1.0 - through - (1.0 - trans)).max(0.0) * peak).clamp(0.0, 1.0);
        if a <= 0.0 {
            return [0.0; 4];
        }
        let c = lit.map(|x| encode(shoulder(x / a)));
        let a = a * opacity;
        return [c[0] * a, c[1] * a, c[2] * a, a];
    }
    // Glass: the body lets the background through except where it reflects.
    let body = opacity * (1.0 - trans * (1.0 - fc_v));
    let s_light = fmul(s_light, opacity);
    let lit = fadd(fmul(fadd(d_light, emissive), body), s_light);
    let peak = s_light[0].max(s_light[1]).max(s_light[2]).min(1.0);
    let a = (body + (1.0 - body) * peak).clamp(0.0, 1.0);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let c = lit.map(|x| encode(shoulder(x / a)));
    [c[0] * a, c[1] * a, c[2] * a, a]
}

/// Premultiplied colour of a mesh primitive at target pixel centre `p`.
pub fn shade_mesh(prim: &Prim, p: [f32; 2], m: &[[f32; 4]], bins: &[u32], groups: &[[f32; 4]], images: &[u32]) -> [f32; 4] {
    let tx = (p[0] as u32) / TILE;
    let ty = (p[1] as u32) / TILE;
    let (tx0, ty0, tw) = (prim.geom[1].to_bits(), prim.geom[2].to_bits(), prim.geom[3].to_bits());
    if tx < tx0 || ty < ty0 || tx - tx0 >= tw {
        return [0.0; 4];
    }
    let base = prim.geom[0].to_bits() as usize;
    let k = ((ty - ty0) * tw + (tx - tx0)) as usize;
    let start = base + bins[base + k * 2] as usize;
    let count = bins[base + k * 2 + 1] as usize;
    let tb = prim.geom2[0].to_bits() as usize;
    let mut best = [u32::MAX; 8];
    let mut depth = [0.0f32; 8];
    for &tri in &bins[start..start + count] {
        let r = tb + tri as usize * 4;
        let (r0, r1, d) = (m[r], m[r + 1], m[r + 3]);
        // Quick reject: no sample of this pixel can be inside.
        let (c, _) = cover(r0, r1, p);
        let pts = [[r0[0], r0[1]], [r0[2], r0[3]], [r1[0], r1[1]]];
        let mut reject = false;
        for k in 0..3 {
            let (a, b) = (pts[(k + 1) % 3], pts[(k + 2) % 3]);
            if c[k] < -((b[0] - a[0]).abs() + (b[1] - a[1]).abs()) * 0.4375 - 1e-3 {
                reject = true;
            }
        }
        if reject {
            continue;
        }
        for s in 0..8 {
            let q = [p[0] + SAMPLES[s][0] / 16.0, p[1] + SAMPLES[s][1] / 16.0];
            if cover(r0, r1, q).1 {
                let z = d[0] * q[0] + d[1] * q[1] + d[2];
                // Cards only stand where their picture is (alpha test).
                // Later surfaces win ties: coplanar layers keep their order.
                let tf = m[r + 2][3] as u32;
                let material = prim.geom2[2].to_bits() as usize + r1[3].to_bits() as usize * MAT_ROWS;
                let stands = if tf & 1 != 0 {
                    card_sample(prim, m, groups, material, q)[3] >= CARD_STANDS
                } else if tf & 2 != 0 {
                    wire_stands(m, material, r0, r1, tf, q)
                } else {
                    true
                };
                if z >= depth[s] * (1.0 - TIE) && stands {
                    depth[s] = z;
                    best[s] = tri;
                }
            }
        }
    }
    let mut out = [0.0f32; 4];
    for s in 0..8 {
        let tri = best[s];
        if tri == u32::MAX || (0..s).any(|j| best[j] == tri) {
            continue;
        }
        let mut n = 0.0f32;
        let mut q = [0.0f32; 2];
        for j in s..8 {
            if best[j] == tri {
                n += 1.0;
                q = [q[0] + SAMPLES[j][0], q[1] + SAMPLES[j][1]];
            }
        }
        let centroid = [p[0] + q[0] / (16.0 * n), p[1] + q[1] / (16.0 * n)];
        let c = shade_tri(prim, m, groups, images, tri, centroid);
        for i in 0..4 {
            out[i] += c[i] * n / 8.0;
        }
    }
    let opacity = prim.inv_y[3];
    out.map(|v| v * opacity)
}

