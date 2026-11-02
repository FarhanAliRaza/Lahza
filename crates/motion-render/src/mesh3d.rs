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
    frame: u64,
    /// Batches of the display list being built.
    pub(crate) batches: Vec<Batch>,
    pub warnings: Vec<String>,
}

impl MeshCache {
    pub(crate) fn begin(&mut self) {
        self.frame += 1;
        self.batches.clear();
        if self.meshes.len() > 96 {
            let keep = self.frame.saturating_sub(4);
            self.meshes.retain(|_, (_, used)| *used >= keep);
        }
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
}

pub(crate) struct Batch {
    verts: Vec<Vert>,
    /// Vertex indices and material index.
    tris: Vec<([u32; 3], u32)>,
    /// 4 rows per material (see `material_rows`).
    materials: Vec<[f32; 4]>,
    lighting: Arc<Lighting>,
    /// Rows of the light block.
    light_rows: Vec<[f32; 4]>,
    pub(crate) bounds: [f64; 4],
    layers: usize,
}

impl Batch {
    pub(crate) fn can_join(&self, m: &MeshState) -> bool {
        m.lighting.as_ref().is_some_and(|l| Arc::ptr_eq(l, &self.lighting) || **l == *self.lighting)
    }
}

const NEAR: f64 = 1.0;
/// Most triangles one batch may hold (beyond this, meshes are skipped).
const MAX_TRIS: usize = 4_000_000;

use lahza_motion_core::compile::srgb_to_linear;

/// Rows of one material: [rgb linear, opacity], [metallic, roughness,
/// clear coat, its roughness], [emissive rgb, flags (1 unlit, 2 faceted)],
/// [transmission, 0, 0, 0].
fn material_rows(m: &MeshState, part: usize, segment: u32, opacity: f64) -> [[f32; 4]; 4] {
    let mat = &m.materials[part];
    let mut color = mat.color;
    if !m.colors.is_empty() && !m.own_color[part] {
        color = m.colors[segment as usize % m.colors.len()];
    }
    let flags = (mat.unlit as u32) | ((mat.faceted as u32) << 1);
    [
        [srgb_to_linear(color.0[0]) as f32, srgb_to_linear(color.0[1]) as f32, srgb_to_linear(color.0[2]) as f32, (color.0[3] * mat.opacity * opacity) as f32],
        [mat.metallic as f32, mat.roughness as f32, mat.clearcoat as f32, mat.clearcoat_roughness as f32],
        [mat.emissive[0] as f32, mat.emissive[1] as f32, mat.emissive[2] as f32, flags as f32],
        [mat.transmission as f32, 0.0, 0.0, 0.0],
    ]
}

/// The light block: [lights, lobes, 0, 0], camera→world rotation (3
/// rows), sky, horizon, ground, then per light [vector, kind] [colour]
/// and per lobe [direction, size] [colour]. Light vectors are in camera
/// space; lobes in world space.
fn light_rows(l: &Lighting, m: &MeshState) -> Vec<[f32; 4]> {
    let v = m.camera.view.0;
    // World → camera rotation R (rows of the view matrix); camera → world is Rᵀ.
    let r = [[v[0], v[1], v[2]], [v[4], v[5], v[6]], [v[8], v[9], v[10]]];
    let rot = |d: [f64; 3]| [r[0][0] * d[0] + r[0][1] * d[1] + r[0][2] * d[2], r[1][0] * d[0] + r[1][1] * d[1] + r[1][2] * d[2], r[2][0] * d[0] + r[2][1] * d[1] + r[2][2] * d[2]];
    let f = |a: [f64; 3], w: f64| [a[0] as f32, a[1] as f32, a[2] as f32, w as f32];
    let mut rows = vec![[l.lights.len() as f32, l.env.lobes.len() as f32, 0.0, 0.0]];
    for c in 0..3 {
        rows.push([r[0][c] as f32, r[1][c] as f32, r[2][c] as f32, 0.0]);
    }
    rows.push(f(l.env.sky, 0.0));
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
        };
        rows.push(f(vector, kind));
        rows.push(f(light.color, 0.0));
    }
    for lobe in &l.env.lobes {
        rows.push(f(lobe.dir, lobe.size));
        rows.push(f(lobe.color, 0.0));
    }
    rows
}

impl Batch {
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
        })
    }

    /// Adds one mesh layer drawn through `root` (canvas → output pixels).
    pub(crate) fn add(&mut self, cache: &mut MeshCache, m: &MeshState, root: Proj, opacity: f64) {
        let Some(mesh) = cache.get(m) else { return };
        self.layers += 1;
        if self.tris.len() + mesh.tris.len() > MAX_TRIS {
            cache.warnings.push("too many mesh triangles in one frame; some meshes were skipped".into());
            return;
        }
        let mv = m.camera.view.then(m.world);
        let Some(nm) = mv.normal_matrix() else { return };
        let (focal, (cx, cy)) = (m.camera.focal, m.camera.centre);
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
        // Vertex slots: mesh vertices in front of the near plane, placed on demand.
        let mut slot: Vec<u32> = vec![u32::MAX; cam.len()];
        let push = |verts: &mut Vec<Vert>, bounds: &mut [f64; 4], p: [f64; 3], n: [f64; 3]| -> u32 {
            let s = project(p);
            *bounds = [bounds[0].min(s[0]), bounds[1].min(s[1]), bounds[2].max(s[0]), bounds[3].max(s[1])];
            verts.push(Vert { screen: s, pos: [p[0] as f32, p[1] as f32, p[2] as f32], normal: [n[0] as f32, n[1] as f32, n[2] as f32] });
            (verts.len() - 1) as u32
        };
        // Material slots by (part, colour), filled on first use.
        let colors_n = m.colors.len().max(1);
        let mut slots = vec![u32::MAX; PARTS * colors_n];
        for (t, &tag) in mesh.tris.iter().zip(&mesh.tags) {
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
            if tag & TWO_SIDED == 0 && dot(g, v[0].0) >= 0.0 {
                continue;
            }
            let colors = if m.colors.is_empty() || m.own_color[part] { 1 } else { m.colors.len() as u32 };
            let mslot = &mut slots[part * colors_n + (segment % colors) as usize];
            if *mslot == u32::MAX {
                *mslot = (self.materials.len() / 4) as u32;
                self.materials.extend_from_slice(&material_rows(m, part, segment, opacity));
            }
            let material = *mslot;
            let behind = v.iter().filter(|q| q.0[2] < NEAR).count();
            if behind == 3 {
                continue;
            }
            if behind == 0 {
                let ids = t.map(|i| {
                    let s = &mut slot[i as usize];
                    if *s == u32::MAX {
                        *s = push(&mut self.verts, &mut self.bounds, cam[i as usize].0, cam[i as usize].1);
                    }
                    *s
                });
                self.tris.push((ids, material));
                continue;
            }
            // Clip against the near plane (Sutherland–Hodgman).
            let mut poly: Vec<([f64; 3], [f64; 3])> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (v[k], v[(k + 1) % 3]);
                if a.0[2] >= NEAR {
                    poly.push(a);
                }
                if (a.0[2] >= NEAR) != (b.0[2] >= NEAR) {
                    let s = (NEAR - a.0[2]) / (b.0[2] - a.0[2]);
                    let lerp = |x: [f64; 3], y: [f64; 3]| [x[0] + (y[0] - x[0]) * s, x[1] + (y[1] - x[1]) * s, x[2] + (y[2] - x[2]) * s];
                    poly.push((lerp(a.0, b.0), lerp(a.1, b.1)));
                }
            }
            let ids: Vec<u32> = poly.iter().map(|(p, n)| push(&mut self.verts, &mut self.bounds, *p, *n)).collect();
            for k in 1..ids.len().saturating_sub(1) {
                self.tris.push(([ids[0], ids[k], ids[k + 1]], material));
            }
        }
    }

    /// Writes the batch for `target` (whose pixel (x, y) is canvas
    /// `origin + factor·(x, y)`) into the list's mesh buffers and returns
    /// its primitive, or None when nothing lands on the target.
    pub(crate) fn place(&self, mesh_rows: &mut Vec<[f32; 4]>, bins: &mut Vec<u32>, target: &Target, origin: [f64; 2], factor: [f64; 2], blend: u32) -> Option<(Prim, [f64; 4])> {
        let (tw_px, th_px) = (target.width as f64, target.height as f64);
        let to_target = |s: [f64; 2]| [(s[0] - origin[0]) / factor[0], (s[1] - origin[1]) / factor[1]];
        // Triangles on the target with their edge and depth planes.
        struct Tri {
            rows: [[f32; 4]; 4],
            bbox: [f32; 4],
        }
        let setup = |(ids, material): &([u32; 3], u32)| -> Option<Tri> {
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
                [f32::from_bits(ids[0]), f32::from_bits(ids[1]), f32::from_bits(ids[2]), 0.0],
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
        let mut used = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for t in &tris {
            let bbox = t.bbox.map(|v| v as f64);
            used = [used[0].min(bbox[0]), used[1].min(bbox[1]), used[2].max(bbox[2]), used[3].max(bbox[3])];
        }
        if tris.is_empty() {
            return None;
        }
        let clamp_x = |v: f64| (v.max(0.0).min(tw_px - 1.0) as u32 / TILE).min(target.tiles_x - 1);
        let clamp_y = |v: f64| (v.max(0.0).min(th_px - 1.0) as u32 / TILE).min(target.tiles_y - 1);
        let (tx0, ty0, tx1, ty1) = (clamp_x(used[0]), clamp_y(used[1]), clamp_x(used[2]), clamp_y(used[3]));
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
            mesh_rows.push([v.normal[0], v.normal[1], v.normal[2], 0.0]);
        }
        let mat_base = mesh_rows.len() as u32;
        mesh_rows.extend_from_slice(&self.materials);
        let light_base = mesh_rows.len() as u32;
        mesh_rows.extend_from_slice(&self.light_rows);
        let b = |v: u32| f32::from_bits(v);
        let prim = Prim {
            kind: [PRIM_MESH, 0, blend, 0],
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

/// The environment's radiance toward world direction `d`, blurred for
/// roughness `r`.
fn env(m: &[[f32; 4]], lb: usize, d: F3, r: f32) -> F3 {
    let up = -d[1];
    let sky = f3(m[lb + 4]);
    let horizon = f3(m[lb + 5]);
    let ground = f3(m[lb + 6]);
    let mut c = if up >= 0.0 { fmix(horizon, sky, smoothstep(0.0, 0.6 + r, up)) } else { fmix(horizon, ground, smoothstep(0.0, 0.2 + r, -up)) };
    let lights = m[lb][0] as usize;
    let lobes = m[lb][1] as usize;
    let first = lb + 7 + lights * 2;
    for i in 0..lobes {
        let l = m[first + i * 2];
        let s2 = l[3] * l[3];
        let w2 = s2 + r * r * 1.2;
        let k = ((fdot(d, f3(l)) - 1.0) / w2).exp() * s2 / w2;
        c = fadd(c, fmul(f3(m[first + i * 2 + 1]), k));
    }
    c
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
fn shade_tri(prim: &Prim, m: &[[f32; 4]], tri: u32, q: [f32; 2]) -> [f32; 4] {
    let tb = prim.geom2[0].to_bits() as usize + tri as usize * 4;
    let vb = prim.geom2[1].to_bits() as usize;
    let mb = prim.geom2[2].to_bits() as usize;
    let lb = prim.geom2[3].to_bits() as usize;
    let (e, _) = cover(m[tb], m[tb + 1], q);
    let ids = m[tb + 2];
    let mut w = [0.0f32; 3];
    let mut pos = [[0.0f32; 3]; 3];
    let mut nrm = [[0.0f32; 3]; 3];
    for k in 0..3 {
        let v = ids[k].to_bits() as usize;
        let a = m[vb + v * 2];
        pos[k] = f3(a);
        nrm[k] = f3(m[vb + v * 2 + 1]);
        w[k] = e[k].max(0.0) * a[3];
    }
    let s = (w[0] + w[1] + w[2]).max(1e-30);
    let b = w.map(|x| x / s);
    let p = fadd(fadd(fmul(pos[0], b[0]), fmul(pos[1], b[1])), fmul(pos[2], b[2]));
    let navg = fadd(fadd(fmul(nrm[0], b[0]), fmul(nrm[1], b[1])), fmul(nrm[2], b[2]));
    let mat = m[tb + 1][3].to_bits() as usize;
    let m0 = m[mb + mat * 4];
    let m1 = m[mb + mat * 4 + 1];
    let m2 = m[mb + mat * 4 + 2];
    let m3 = m[mb + mat * 4 + 3];
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
    let base = f3(m0);
    let opacity = m0[3];
    let emissive = f3(m2);
    if flags & 1 != 0 {
        let c = fadd(base, emissive).map(|x| encode(x));
        return [c[0] * opacity, c[1] * opacity, c[2] * opacity, opacity];
    }
    let (metal, rough, cc, ccr) = (m1[0], m1[1].max(0.03), m1[2], m1[3].max(0.03));
    let trans = m3[0];
    let f0 = fmix([0.04; 3], base, metal);
    let diffuse = fmul(base, (1.0 - metal) * (1.0 - trans));
    let n_v = fdot(n, v).max(1e-4);
    let fc_v = 0.04 + 0.96 * (1.0 - n_v).powi(5);
    let coat = 1.0 - cc * fc_v;
    let mut d_light = [0.0f32; 3];
    let mut s_light = [0.0f32; 3];
    let lights = m[lb][0] as usize;
    for i in 0..lights {
        let row = m[lb + 7 + i * 2];
        let col = f3(m[lb + 7 + i * 2 + 1]);
        let kind = row[3] as u32;
        if kind == 2 {
            d_light = fadd(d_light, fmul3(diffuse, col));
            continue;
        }
        let l = if kind == 1 { fnorm(fsub(f3(row), p)) } else { f3(row) };
        let n_l = fdot(n, l);
        if n_l <= 0.0 {
            continue;
        }
        let h = fnorm(fadd(l, v));
        let n_h = fdot(n, h).max(0.0);
        let v_h = fdot(v, h).max(0.0);
        let fw = (1.0 - v_h).powi(5);
        let f = fadd(f0, fmul(fsub([1.0; 3], f0), fw));
        let spec = ggx(n_h, rough * rough) * smith(n_v, n_l, rough) / (4.0 * n_v * n_l).max(1e-4) * std::f32::consts::PI;
        let kd = fsub([1.0; 3], f);
        d_light = fadd(d_light, fmul3(fmul3(kd, diffuse), fmul(col, n_l * coat)));
        s_light = fadd(s_light, fmul3(fmul(f, spec * n_l * coat), col));
        if cc > 0.0 {
            let fc = 0.04 + 0.96 * fw;
            let sc = ggx(n_h, ccr * ccr) * smith(n_v, n_l, ccr) / (4.0 * n_v * n_l).max(1e-4) * std::f32::consts::PI;
            s_light = fadd(s_light, fmul(col, sc * fc * cc * n_l));
        }
    }
    let nw = to_world(m, lb, n);
    let r_view = fsub(fmul(n, 2.0 * fdot(n, v)), v);
    let rw = to_world(m, lb, r_view);
    d_light = fadd(d_light, fmul(fmul3(diffuse, env(m, lb, nw, 1.0)), coat));
    let ab = env_brdf(rough, n_v);
    let spec_env = fmul3(env(m, lb, rw, rough), fadd(fmul(f0, ab[0]), [ab[1]; 3]));
    s_light = fadd(s_light, fmul(spec_env, coat));
    if cc > 0.0 {
        s_light = fadd(s_light, fmul(env(m, lb, rw, ccr), fc_v * cc));
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
pub fn shade_mesh(prim: &Prim, p: [f32; 2], m: &[[f32; 4]], bins: &[u32]) -> [f32; 4] {
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
                if z > depth[s] {
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
        let c = shade_tri(prim, m, tri, centroid);
        for i in 0..4 {
            out[i] += c[i] * n / 8.0;
        }
    }
    let opacity = prim.inv_y[3];
    out.map(|v| v * opacity)
}

