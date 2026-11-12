//! 3D meshes for `mesh` layers: primitives (sphere, box, cylinder,
//! capsule, torus), extruded and bevelled outlines, profiles revolved
//! around an axis, and tubes swept along a path.
//!
//! Space is the layer's (x right, y down, z away from the viewer), so a
//! mesh with no rotation faces the camera the way a shape layer does: a
//! torus lies in the screen plane like a `ring`, an extruded outline shows
//! its front face, and angles run from up, clockwise, like `arc`.
//!
//! Every triangle carries a tag: the part it belongs to (front, back,
//! side, bevel, caps — each can take its own material) and the segment
//! (rings and revolves split into `segments` pieces take per-segment
//! colours). Generation depends only on the parameters, so a mesh is
//! cached by them and identical on every machine.

use crate::compile::PathMorph;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

pub const PART_FRONT: u32 = 0;
pub const PART_BACK: u32 = 1;
pub const PART_SIDE: u32 = 2;
pub const PART_BEVEL: u32 = 3;
pub const PART_CAPS: u32 = 4;
/// Revolved surfaces facing the axis (a ring's inner wall).
pub const PART_INSIDE: u32 = 5;
pub const PARTS: usize = 6;
pub const PART_NAMES: [&str; PARTS] = ["front", "back", "side", "bevel", "caps", "inside"];
/// Tag bit: the surface is open (seen from both sides).
pub const TWO_SIDED: u32 = 1 << 31;

/// A triangle mesh in layer units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
    /// Per triangle: part (bits 0..8), segment (bits 8..24), `TWO_SIDED`.
    pub tags: Vec<u32>,
    /// Per triangle: which edges are real (bit k: the edge opposite vertex
    /// k), for wireframes; quad diagonals and cap fills are not.
    pub edges: Vec<u8>,
    /// Per vertex: texture coordinates (0..1 across a face, around a
    /// revolved surface, over a sphere); set at the end of `generate`.
    pub uvs: Vec<[f32; 2]>,
}

/// A glTF sequence's file for a frame (`{frame}`, `{frame:04}`).
pub use crate::compile::frame_path;

pub fn tag(part: u32, segment: u32) -> u32 {
    part | (segment.min(0xffff) << 8)
}
pub fn tag_part(tag: u32) -> u32 {
    tag & 0xff
}
pub fn tag_segment(tag: u32) -> u32 {
    (tag >> 8) & 0xffff
}

type V3 = [f64; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-12 {
        mul(a, 1.0 / l)
    } else {
        a
    }
}

impl Mesh {
    /// How far its texture coordinates stretch a map over it: the length
    /// on the mesh one unit of UV covers, in units of the mesh's longest
    /// side. 1 when a map lies once across that side, π around a sphere;
    /// more where an atlas gives a part of the mesh few of its texels.
    /// The value 95% of the surface is within (a few stretched triangles
    /// do not decide it). None without UVs.
    pub fn uv_stretch(&self) -> Option<f64> {
        if self.uvs.len() != self.positions.len() || self.tris.is_empty() {
            return None;
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in &self.positions {
            for k in 0..3 {
                (lo[k], hi[k]) = (lo[k].min(p[k]), hi[k].max(p[k]));
            }
        }
        let side = (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(0.0, f64::max);
        // Per triangle: (length per unit of UV, its area).
        let mut ratios: Vec<(f64, f64)> = Vec::with_capacity(self.tris.len());
        for t in &self.tris {
            let [a, b, c] = t.map(|i| self.positions[i as usize].map(|v| v as f64));
            let [ua, ub, uc] = t.map(|i| self.uvs[i as usize].map(|v| v as f64));
            let (e, f) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let n = [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]];
            let area = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0;
            let uv = ((ub[0] - ua[0]) * (uc[1] - ua[1]) - (ub[1] - ua[1]) * (uc[0] - ua[0])).abs() / 2.0;
            if area > 0.0 && uv > 1e-12 && area.is_finite() && uv.is_finite() {
                ratios.push(((area / uv).sqrt(), area));
            }
        }
        if ratios.is_empty() || !(side > 0.0) {
            return None;
        }
        ratios.sort_by(|a, b| a.0.total_cmp(&b.0));
        let total: f64 = ratios.iter().map(|r| r.1).sum();
        let mut seen = 0.0;
        for (ratio, area) in &ratios {
            seen += area;
            if seen >= total * 0.95 {
                return Some(ratio / side);
            }
        }
        ratios.last().map(|r| r.0 / side)
    }

    fn vertex(&mut self, p: V3, n: V3) -> u32 {
        self.positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        let n = norm(n);
        self.normals.push([n[0] as f32, n[1] as f32, n[2] as f32]);
        (self.positions.len() - 1) as u32
    }
    fn tri(&mut self, a: u32, b: u32, c: u32, tag: u32) {
        self.tri_edges(a, b, c, tag, 7);
    }
    fn tri_edges(&mut self, a: u32, b: u32, c: u32, tag: u32, edges: u8) {
        self.tris.push([a, b, c]);
        self.tags.push(tag);
        self.edges.push(edges);
    }
    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32, tag: u32) {
        // The diagonal a–c is not an edge of the quad.
        self.tri_edges(a, b, c, tag, 0b101);
        self.tri_edges(a, c, d, tag, 0b011);
    }
    /// Triangles filling outlines: only edges along an outline (between
    /// neighbouring points of one contour) are real.
    fn fill(&mut self, base: u32, tris: &[[usize; 3]], contours: &[usize], tag: u32) {
        let contour_of = |i: usize| {
            let mut start = 0;
            for &n in contours {
                if i < start + n {
                    return (start, n);
                }
                start += n;
            }
            (start, 1)
        };
        let outline = |i: usize, j: usize| {
            let ((si, ni), (sj, _)) = (contour_of(i), contour_of(j));
            si == sj && ((i - si + 1) % ni == j - si || (j - si + 1) % ni == i - si)
        };
        for t in tris {
            let mut mask = 0u8;
            for k in 0..3 {
                if outline(t[(k + 1) % 3], t[(k + 2) % 3]) {
                    mask |= 1 << k;
                }
            }
            self.tri_edges(base + t[0] as u32, base + t[1] as u32, base + t[2] as u32, tag, mask);
        }
    }
    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        if !other.uvs.is_empty() || !self.uvs.is_empty() {
            self.uvs.resize(self.positions.len(), [0.0; 2]);
            self.uvs.extend_from_slice(&other.uvs);
            self.uvs.resize(self.positions.len() + other.positions.len(), [0.0; 2]);
        }
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.tris.extend(other.tris.iter().map(|t| t.map(|i| i + base)));
        self.tags.extend_from_slice(&other.tags);
        self.edges.extend_from_slice(&other.edges);
    }
    /// Local bounds [min, max].
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }
}

// ------------------------------------------------------------ parameters

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Caps {
    Flat,
    Round,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SweepProfile {
    Circle,
    Square,
    /// A flat band `width` wide, standing out of the layer plane (along z).
    Ribbon,
}

/// A mesh's evaluated parameters (layer units, degrees).
#[derive(Clone, Debug, PartialEq)]
pub enum MeshShape {
    Sphere { radius: f64 },
    /// A flat rectangle in the layer plane, seen from both sides.
    Plane { size: [f64; 2] },
    /// `size` [w, h, d]; `bevel` rounds every edge.
    Box { size: V3, bevel: f64 },
    /// Axis along y; `top` radius makes a cone or frustum.
    Cylinder { radius: f64, top: f64, height: f64, bevel: f64 },
    Capsule { radius: f64, height: f64 },
    /// Facing the camera (axis z); `angles` like an arc (0 = up, clockwise).
    Torus { radius: f64, tube: f64, angles: [f64; 2], caps: Caps },
    /// A closed outline pushed back `depth`, its edges rounded by `bevel`.
    Extrude { path: PathMorph, depth: f64, bevel: f64 },
    /// An outline in (x = distance from the axis, y = along the axis)
    /// turned about the axis (`axis_y`: the layer's y, else z).
    Revolve { profile: PathMorph, angles: [f64; 2], axis_y: bool },
    /// A tube along a path in the layer plane, drawn on by `trim`.
    Sweep { path: PathMorph, radius: f64, width: f64, profile: SweepProfile, trim: [f64; 2], caps: Caps, head: f64 },
    /// A glTF primitive, loaded by the renderer (it knows the project's
    /// folder): `src` with any `{frame}` already filled in.
    Gltf { src: String, mesh: u32, primitive: u32, unit: f64, bounds: Option<[[f64; 3]; 2]> },
    /// Generated by the project's Slang functions.
    Slang(crate::slang_mesh::SlangShape),
}

/// Shape plus the splitting and tessellation settings every kind shares.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshParams {
    pub shape: MeshShape,
    /// Pieces around the angle range (rings, revolves) or along the path
    /// (sweeps); each takes the next of the layer's `colors`.
    pub segments: u32,
    /// Degrees between pieces (rings, revolves) or fraction of the length
    /// between pieces (sweeps).
    pub gap: f64,
    /// Tessellation multiplier (1 = smooth at the size drawn).
    pub detail: f64,
    /// Relative sizes of the pieces, in turn (empty: all equal).
    pub spans: Vec<f64>,
    /// Screen pixels per layer unit where the mesh is drawn (in half-octave
    /// steps): tessellation follows the drawn size, so small profiles
    /// scaled up and far meshes both get the segments they need.
    pub screen: f64,
    /// Displacement along the normals: what pushes, and its reach (px).
    pub displace: Option<(Displacement, f64)>,
    /// A Slang function moving every point, applied last (by the renderer,
    /// which loads glTF meshes and height images).
    pub deform: Option<crate::slang_mesh::Deform>,
}

/// What pushes a displaced surface out and in.
#[derive(Clone, Debug, PartialEq)]
pub enum Displacement {
    /// Noise: out and in by `amount` × (2·noise − 1).
    Noise(crate::noise::Field),
    /// A height image (project path) over the mesh's UVs: white rises by
    /// `amount`, black stays. The renderer loads it (`displace_by_image`).
    Image(String),
}

/// Heights over UVs, row-major, 0..1 (bright = raised).
#[derive(Clone, Debug, Default)]
pub struct Heightfield {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Heightfield {
    /// Bilinear, clamped at the edges.
    fn at(&self, uv: [f32; 2]) -> f64 {
        let (w, h) = (self.width, self.height);
        let x = (uv[0] as f64 * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
        let y = (uv[1] as f64 * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let v = |x: usize, y: usize| self.data[y * w + x] as f64;
        (v(x0, y0) * (1.0 - fx) + v(x1, y0) * fx) * (1.0 - fy) + (v(x0, y1) * (1.0 - fx) + v(x1, y1) * fx) * fy
    }
}

impl MeshParams {
    /// Bytes identifying the generated mesh (cache key).
    pub fn key(&self) -> Vec<u8> {
        let mut k = Vec::with_capacity(96);
        let mut f = |v: f64| k.extend_from_slice(&v.to_bits().to_le_bytes());
        f(self.segments as f64);
        f(self.gap);
        f(self.detail);
        f(self.screen);
        f(self.spans.len() as f64);
        for s in &self.spans {
            f(*s);
        }
        let mut text = String::new();
        let mut path = |m: &PathMorph, f: &mut dyn FnMut(f64)| {
            f(m.t);
            text.push_str(&m.from);
            text.push('|');
            text.push_str(m.to.as_deref().unwrap_or(""));
            text.push('|');
        };
        match &self.shape {
            MeshShape::Sphere { radius } => {
                f(1.0);
                f(*radius);
            }
            MeshShape::Plane { size } => {
                f(9.0);
                f(size[0]);
                f(size[1]);
            }
            MeshShape::Box { size, bevel } => {
                f(2.0);
                size.iter().for_each(|v| f(*v));
                f(*bevel);
            }
            MeshShape::Cylinder { radius, top, height, bevel } => {
                f(3.0);
                [*radius, *top, *height, *bevel].iter().for_each(|v| f(*v));
            }
            MeshShape::Capsule { radius, height } => {
                f(4.0);
                f(*radius);
                f(*height);
            }
            MeshShape::Torus { radius, tube, angles, caps } => {
                f(5.0);
                [*radius, *tube, angles[0], angles[1], *caps as u8 as f64].iter().for_each(|v| f(*v));
            }
            MeshShape::Extrude { path: p, depth, bevel } => {
                f(6.0);
                f(*depth);
                f(*bevel);
                path(p, &mut f);
            }
            MeshShape::Revolve { profile, angles, axis_y } => {
                f(7.0);
                [angles[0], angles[1], *axis_y as u8 as f64].iter().for_each(|v| f(*v));
                path(profile, &mut f);
            }
            MeshShape::Sweep { path: p, radius, width, profile, trim, caps, head } => {
                f(8.0);
                [*radius, *width, *profile as u8 as f64, trim[0], trim[1], *caps as u8 as f64, *head].iter().for_each(|v| f(*v));
                path(p, &mut f);
            }
            MeshShape::Gltf { src, mesh, primitive, unit, .. } => {
                f(11.0);
                [*mesh as f64, *primitive as f64, *unit].iter().for_each(|v| f(*v));
                text.push_str(src);
                text.push('\n');
            }
            MeshShape::Slang(s) => {
                // The library's build: edited shaders make a new mesh.
                f(13.0);
                f(f64::from_bits(s.call.build().unwrap_or(0)));
                let o = s.outputs;
                [s.vertices as f64, s.triangles as f64, s.time as f64, o.normals as u8 as f64, o.uvs as u8 as f64, o.segments as u8 as f64, s.two_sided as u8 as f64].iter().for_each(|v| f(*v));
                f(s.params.len() as f64);
                s.params.iter().for_each(|v| f(*v as f64));
                text.push_str(&s.function);
                text.push('\n');
            }
        }
        if let Some((Displacement::Image(src), amount)) = &self.displace {
            f(12.0);
            f(*amount);
            text.push_str(src);
            text.push('\n');
        }
        if let Some((Displacement::Noise(field), amount)) = &self.displace {
            [10.0, field.kind.code() as f64, field.scale, field.octaves as f64, field.seed as f64, *amount, field.offset[0], field.offset[1], field.offset[2]].iter().for_each(|v| f(*v));
        }
        if let Some(d) = &self.deform {
            f(14.0);
            f(f64::from_bits(d.call.build().unwrap_or(0)));
            [d.time as f64, d.subdivide as f64, d.params.len() as f64].iter().for_each(|v| f(*v));
            d.params.iter().for_each(|v| f(*v as f64));
            text.push_str(&d.function);
            text.push('\n');
        }
        k.extend_from_slice(text.as_bytes());
        k
    }
}

impl MeshParams {
    /// Whether the mesh is new every frame (Slang functions of time):
    /// caches keep it only while it is drawn.
    pub fn animated(&self) -> bool {
        matches!(&self.shape, MeshShape::Slang(s) if s.animated) || self.deform.as_ref().is_some_and(|d| d.animated)
    }

    /// The deformer and its kernel's WGSL, when the GPU can move this
    /// mesh's points itself, in its mesh pool: the kernel built.
    pub fn gpu_deformer(&self) -> Option<(&crate::slang_mesh::Deform, &std::sync::Arc<str>)> {
        let d = self.deform.as_ref()?;
        let wgsl = d.gpu.as_ref()?.wgsl()?;
        (d.params.len() <= crate::slang_mesh::GPU_PARAMS).then_some((d, wgsl))
    }

    /// This mesh without its deformer, when that is a mesh the GPU
    /// generates itself (`gpu_generator`): the GPU then makes the mesh,
    /// subdivides it as the deformer asks and moves it without the CPU
    /// having its points, and the layer's `bounds`, padded by the
    /// deformer's `reach`, stand for the moved ones.
    pub fn gpu_deformed_generator(&self) -> Option<MeshParams> {
        self.gpu_deformer()?;
        let plain = MeshParams { deform: None, ..self.clone() };
        (plain.gpu_generator().is_some() && self.local_bounds().is_some()).then_some(plain)
    }

    /// The Slang generator and its kernel's WGSL, when the GPU can make
    /// this mesh itself, in its mesh pool: the kernel built, the layer
    /// gives `bounds` (the CPU places and culls the mesh by them without
    /// its points), and nothing moves the points afterwards.
    pub fn gpu_generator(&self) -> Option<(&crate::slang_mesh::SlangShape, &std::sync::Arc<str>)> {
        let MeshShape::Slang(s) = &self.shape else { return None };
        let wgsl = s.gpu.as_ref()?.wgsl()?;
        let fits = s.bounds.is_some() && self.displace.is_none() && self.deform.is_none() && s.vertices > 0 && s.triangles > 0 && s.params.len() <= crate::slang_mesh::GPU_PARAMS;
        fits.then_some((s, wgsl))
    }

    /// What generation will change from the parameters asked for (bevels
    /// and tubes too big for the shape), as messages for `check`.
    pub fn adjustments(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut note = |what: &str, asked: f64, used: f64, why: &str| {
            if asked > used + 1e-6 {
                out.push(format!("{what} {asked:.1} is too big and is drawn as {used:.1} ({why})"));
            }
        };
        match &self.shape {
            MeshShape::Box { size, bevel } => {
                let h = size.map(|v| v.abs() / 2.0);
                note("bevel", *bevel, bevel.max(0.0).min(h[0]).min(h[1]).min(h[2]), "half the smallest side");
            }
            MeshShape::Cylinder { radius, top, height, bevel } => {
                note("bevel", *bevel, bevel.max(0.0).min(height / 2.0).min(radius.max(*top) * 0.999), "half the height or the radius");
            }
            MeshShape::Torus { radius, tube, .. } => note("tube", *tube, tube.max(0.0).min(radius.max(0.0)), "the ring's radius"),
            MeshShape::Extrude { path, depth, bevel } => {
                let d = depth.max(0.0) / 2.0;
                let mut b = bevel.max(0.0).min(d);
                let mut why = "half the depth";
                if let Ok(subpaths) = crate::path::morph(path, 0.5) {
                    for r in regions(subpaths.into_iter().map(|(p, _)| p).collect()) {
                        for c in &r {
                            let (mut lo, mut hi) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
                            for p in c {
                                lo = (lo.0.min(p.0), lo.1.min(p.1));
                                hi = (hi.0.max(p.0), hi.1.max(p.1));
                            }
                            let cap = (hi.0 - lo.0).min(hi.1 - lo.1) * 0.3;
                            if cap < b {
                                b = cap;
                                why = "30% of the outline's narrowest part";
                            }
                        }
                    }
                }
                note("bevel", *bevel, b, why);
            }
            _ => {}
        }
        out
    }

    /// Local bounds [min, max] without generating the mesh (a slightly
    /// loose box: arcs count as full rings).
    pub fn local_bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        // Displacement and deformers reach this far out.
        let pad = self.displace.as_ref().map(|(_, a)| a.abs()).unwrap_or(0.0) + self.deform.as_ref().map(|d| d.reach.abs()).unwrap_or(0.0);
        self.shape_bounds().map(|(lo, hi)| (lo.map(|v| v - pad), hi.map(|v| v + pad)))
    }

    fn shape_bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let sym = |x: f64, y: f64, z: f64| Some(([-x, -y, -z], [x, y, z]));
        let path_box = |m: &PathMorph| -> Option<[f64; 4]> {
            let subpaths = crate::path::morph(m, 1.0).ok()?;
            let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for (pts, _) in &subpaths {
                for p in pts {
                    b = [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)];
                }
            }
            b[0].is_finite().then_some(b)
        };
        match &self.shape {
            MeshShape::Sphere { radius } => sym(*radius, *radius, *radius),
            MeshShape::Plane { size } => sym(size[0].abs() / 2.0, size[1].abs() / 2.0, 0.0),
            MeshShape::Box { size, .. } => sym(size[0].abs() / 2.0, size[1].abs() / 2.0, size[2].abs() / 2.0),
            MeshShape::Cylinder { radius, top, height, .. } => {
                let r = radius.max(*top);
                sym(r, height.abs() / 2.0, r)
            }
            MeshShape::Capsule { radius, height } => sym(*radius, (height.abs() / 2.0).max(*radius), *radius),
            MeshShape::Torus { radius, tube, .. } => sym(radius + tube, radius + tube, *tube),
            MeshShape::Extrude { path, depth, .. } => {
                let b = path_box(path)?;
                Some(([b[0], b[1], -depth / 2.0], [b[2], b[3], depth / 2.0]))
            }
            MeshShape::Revolve { profile, axis_y, .. } => {
                let b = path_box(profile)?;
                let r = b[0].abs().max(b[2].abs());
                if *axis_y {
                    Some(([-r, b[1], -r], [r, b[3], r]))
                } else {
                    Some(([-r, -r, b[1]], [r, r, b[3]]))
                }
            }
            MeshShape::Sweep { path, radius, width, head, .. } => {
                let b = path_box(path)?;
                let pad = radius.max(*head);
                let z = radius.max(width / 2.0).max(*head);
                Some(([b[0] - pad, b[1] - pad, -z], [b[2] + pad, b[3] + pad, z]))
            }
            MeshShape::Gltf { bounds, .. } => bounds.map(|[lo, hi]| (lo, hi)),
            MeshShape::Slang(s) => s.local_bounds(),
        }
    }
}

/// Segments for a circle of radius `r` (one every ~9 px, scaled by detail).
fn circle_steps(r: f64, detail: f64, span: f64) -> usize {
    let n = (r.abs() * span / 9.0 * detail).ceil();
    let min = (24.0 * span / TAU).ceil().max(3.0);
    n.clamp(min, 320.0 * span / TAU + 4.0) as usize
}

/// The pieces of an angle range: `count` spans with `gap` degrees between
/// them (between neighbours only, unless the range is a full turn).
fn pieces(a0: f64, a1: f64, count: u32, gap: f64, spans: &[f64]) -> Vec<(f64, f64)> {
    let count = count.max(1) as usize;
    let span = a1 - a0;
    let full = span.abs() >= 360.0 - 1e-9;
    let gaps = if full { count } else { count - 1 };
    let gap = gap.max(0.0).min(span.abs() / gaps.max(1) as f64 * 0.95) * span.signum();
    let weights = piece_weights(count, spans);
    let total: f64 = weights.iter().sum();
    let room = span - gap * gaps as f64;
    let mut s = a0 + if full { gap / 2.0 } else { 0.0 };
    weights
        .iter()
        .map(|w| {
            let e = s + room * w / total;
            let piece = (s, e);
            s = e + gap;
            piece
        })
        .collect()
}

/// Each piece's relative size: `spans` in turn (all 1 when empty).
fn piece_weights(count: usize, spans: &[f64]) -> Vec<f64> {
    let w: Vec<f64> = (0..count).map(|i| if spans.is_empty() { 1.0 } else { spans[i % spans.len()].max(0.0) }).collect();
    if w.iter().sum::<f64>() > 0.0 {
        w
    } else {
        vec![1.0; count]
    }
}

pub fn generate(params: &MeshParams) -> Result<Mesh, String> {
    let detail = if params.detail.is_finite() && params.detail > 0.0 { params.detail.clamp(0.1, 8.0) } else { 1.0 };
    let screen = if params.screen.is_finite() && params.screen > 0.0 { params.screen.clamp(1.0 / 16.0, 32.0) } else { 1.0 };
    let detail = detail * screen;
    let mut mesh = Mesh::default();
    match &params.shape {
        MeshShape::Gltf { src, .. } => return Err(format!("{src}: glTF meshes are loaded by the renderer")),
        MeshShape::Slang(s) => mesh = s.mesh()?.clone(),
        MeshShape::Sphere { radius } => {
            let r = radius.max(0.0);
            let profile = Profile::arc((0.0, 0.0), r, -FRAC_PI_2, FRAC_PI_2, circle_steps(r, detail, PI), PART_SIDE);
            revolve_pieces(&mut mesh, &[profile], (0.0, 360.0), 1, 0.0, &[], circle_steps(r, detail, TAU), false, false);
        }
        MeshShape::Box { size, bevel } => rounded_box(&mut mesh, *size, *bevel, detail),
        MeshShape::Plane { size } => {
            // A grid, so lighting interpolates smoothly across big planes.
            let (hw, hh) = (size[0].abs() / 2.0, size[1].abs() / 2.0);
            let n = (((hw.max(hh) * 2.0) / 200.0 * detail).ceil() as usize).clamp(1, 16);
            let base = mesh.positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let (x, y) = (-hw + 2.0 * hw * i as f64 / n as f64, -hh + 2.0 * hh * j as f64 / n as f64);
                    mesh.vertex([x, y, 0.0], [0.0, 0.0, -1.0]);
                }
            }
            let w = (n + 1) as u32;
            for j in 0..n as u32 {
                for i in 0..n as u32 {
                    let a = base + j * w + i;
                    mesh.quad(a, a + 1, a + w + 1, a + w, tag(PART_FRONT, 0) | TWO_SIDED);
                }
            }
        }
        MeshShape::Cylinder { radius, top, height, bevel } => {
            let lines = cylinder_profile(radius.max(0.0), top.max(0.0), height.max(0.0), *bevel, detail);
            let steps = circle_steps(radius.max(*top), detail, TAU);
            revolve_pieces(&mut mesh, &lines, (0.0, 360.0), params.segments, params.gap, &params.spans, steps, true, false);
        }
        MeshShape::Capsule { radius, height } => {
            let r = radius.max(0.0);
            let half = (height / 2.0 - r).max(0.0);
            let n = circle_steps(r, detail, FRAC_PI_2);
            let mut p = Profile::arc((0.0, -half), r, -FRAC_PI_2, 0.0, n, PART_SIDE);
            p.extend(Profile::arc((0.0, half), r, 0.0, FRAC_PI_2, n, PART_SIDE));
            revolve_pieces(&mut mesh, &[p], (0.0, 360.0), params.segments, params.gap, &params.spans, circle_steps(r, detail, TAU), true, false);
        }
        MeshShape::Torus { radius, tube, angles, caps } => {
            let (r, t) = (radius.max(0.0), tube.max(0.0).min(radius.max(0.0)));
            let ring = Profile::arc((r, 0.0), t, -PI, PI, circle_steps(t, detail, TAU), PART_SIDE);
            let steps = circle_steps(r + t, detail, TAU);
            let n = params.segments.max(1);
            for (k, (a0, a1)) in pieces(angles[0], angles[1], n, params.gap, &params.spans).into_iter().enumerate() {
                let piece = revolve(&ring_profile(&ring), a0.to_radians(), a1.to_radians(), steps, k as u32, false, true);
                mesh.append(&piece);
                if (a1 - a0).abs() < 360.0 - 1e-9 {
                    for (a, dir) in [(a0, -1.0), (a1, 1.0)] {
                        let sign = dir * (a1 - a0).signum();
                        match caps {
                            Caps::Flat => disc_cap(&mut mesh, r, t, a.to_radians(), sign, steps_for(t, detail), k as u32),
                            Caps::Round => round_cap(&mut mesh, r, t, a.to_radians(), sign, steps_for(t, detail), k as u32),
                        }
                    }
                }
            }
        }
        MeshShape::Extrude { path, depth, bevel } => extrude(&mut mesh, path, *depth, *bevel, detail)?,
        MeshShape::Revolve { profile, angles, axis_y } => {
            let subpaths = crate::path::morph(profile, 0.3 / detail)?;
            if subpaths.is_empty() {
                return Err("revolve: the profile is empty".into());
            }
            let mut lines = Vec::new();
            let mut closed_all = true;
            let mut reach = 0.0f64;
            for (pts, closed) in &subpaths {
                closed_all &= *closed;
                reach = pts.iter().fold(reach, |m, p| m.max(p.0.abs()));
                lines.extend(Profile::from_polyline(pts, *closed, PART_SIDE));
            }
            let steps = circle_steps(reach, detail, TAU);
            revolve_pieces(&mut mesh, &lines, (angles[0], angles[1]), params.segments, params.gap, &params.spans, steps, *axis_y, true);
            if !closed_all {
                for t in &mut mesh.tags {
                    *t |= TWO_SIDED;
                }
            }
            if closed_all && (angles[1] - angles[0]).abs() < 360.0 - 1e-9 || params.segments > 1 {
                for (k, (a0, a1)) in pieces(angles[0], angles[1], params.segments, params.gap, &params.spans).into_iter().enumerate() {
                    if (a1 - a0).abs() >= 360.0 - 1e-9 {
                        continue;
                    }
                    for (pts, closed) in &subpaths {
                        let pts: Vec<(f64, f64)> = if *closed { pts.clone() } else { close_on_axis(pts) };
                        for (a, s) in [(a0, -1.0), (a1, 1.0)] {
                            profile_cap(&mut mesh, &pts, a.to_radians(), s * (a1 - a0).signum(), *axis_y, k as u32);
                        }
                    }
                }
            }
        }
        MeshShape::Sweep { path, radius, width, profile, trim, caps, head } => {
            sweep(&mut mesh, path, *radius, *width, *profile, *trim, *caps, *head, params.segments, params.gap, &params.spans, detail)?
        }
    }
    if !matches!(&params.shape, MeshShape::Slang(s) if s.outputs.uvs) {
        set_uvs(&mut mesh, &params.shape);
    }
    if let Some((Displacement::Noise(field), amount)) = &params.displace {
        if *amount != 0.0 {
            let amount = *amount;
            displace(&mut mesh, field.scale / 3.0, screen, |q, _| amount * (2.0 * field.at(q) - 1.0));
        }
    }
    Ok(mesh)
}

/// Texture coordinates by the shape's kind: spheres equirectangular,
/// revolved surfaces around their axis (caps planar), rings around and
/// across the tube, flat faces planar by the axis they face.
fn set_uvs(mesh: &mut Mesh, shape: &MeshShape) {
    // A Slang mesh with `bounds` spreads its planar UVs over that box, not
    // the box its points fill at the moment: the picture on an animated
    // mesh stays where it is as the points move (and the GPU, generating
    // the mesh itself, has the same box).
    let (lo, hi) = match shape {
        MeshShape::Slang(crate::slang_mesh::SlangShape { bounds: Some([lo, hi]), .. }) => (lo.map(|v| v as f32), hi.map(|v| v as f32)),
        _ => mesh.bounds(),
    };
    let span = |k: usize| ((hi[k] - lo[k]) as f64).max(1e-6);
    let unit = |v: f64, k: usize| ((v - lo[k] as f64) / span(k)) as f32;
    let around = |a: f64, b: f64| (0.5 + a.atan2(b) / TAU) as f32;
    let planar = |p: [f64; 3], n: [f64; 3]| -> [f32; 2] {
        let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
        if az >= ax && az >= ay {
            [unit(p[0], 0), unit(p[1], 1)]
        } else if ax >= ay {
            [unit(p[2], 2), unit(p[1], 1)]
        } else {
            [unit(p[0], 0), unit(p[2], 2)]
        }
    };
    mesh.uvs = mesh
        .positions
        .iter()
        .zip(&mesh.normals)
        .map(|(p, n)| {
            let p = [p[0] as f64, p[1] as f64, p[2] as f64];
            let n = [n[0] as f64, n[1] as f64, n[2] as f64];
            match shape {
                MeshShape::Sphere { .. } => {
                    let d = norm(p);
                    [around(d[0], -d[2]), ((-d[1]).clamp(-1.0, 1.0).acos() / PI) as f32]
                }
                MeshShape::Cylinder { .. } | MeshShape::Capsule { .. } | MeshShape::Revolve { axis_y: true, .. } => {
                    if n[1].abs() > 0.7 && !matches!(shape, MeshShape::Capsule { .. }) {
                        [unit(p[0], 0), unit(p[2], 2)]
                    } else {
                        [around(p[0], -p[2]), unit(p[1], 1)]
                    }
                }
                MeshShape::Revolve { axis_y: false, .. } => {
                    if n[2].abs() > 0.7 {
                        [unit(p[0], 0), unit(p[1], 1)]
                    } else {
                        [around(p[0], -p[1]), unit(p[2], 2)]
                    }
                }
                MeshShape::Torus { radius, .. } => {
                    let rho = (p[0] * p[0] + p[1] * p[1]).sqrt();
                    [around(p[0], -p[1]), around(p[2], rho - radius)]
                }
                MeshShape::Plane { .. } => [unit(p[0], 0), unit(p[1], 1)],
                _ => planar(p, n),
            }
        })
        .collect();
}

/// Most triangles a displaced mesh is subdivided to.
const DISPLACE_MAX_TRIS: usize = 200_000;

/// Displacement subdivides no finer than this many screen pixels per edge:
/// smaller triangles add no visible silhouette (bump shades finer detail),
/// and 18 close-up displaced rods at the cap filled the path tracer's GPU.
const DISPLACE_MIN_EDGE_PX: f64 = 6.0;

/// Displaces a generated mesh by its height image (`Displacement::Image`),
/// subdividing to about one texel per edge (within the same limits as noise).
pub fn displace_by_image(mesh: &mut Mesh, params: &MeshParams, heights: &Heightfield) {
    let Some((Displacement::Image(_), amount)) = &params.displace else { return };
    if *amount == 0.0 || heights.width == 0 || heights.height == 0 || heights.data.len() < heights.width * heights.height {
        return;
    }
    let screen = if params.screen.is_finite() && params.screen > 0.0 { params.screen.clamp(1.0 / 16.0, 32.0) } else { 1.0 };
    let (lo, hi) = mesh.bounds();
    let extent = (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(0.0, f64::max);
    let texel = extent / heights.width.max(heights.height) as f64;
    let amount = *amount;
    displace(mesh, texel, screen, |_, uv| amount * heights.at(uv));
}

/// Pushes the surface along its normals by `offset` (px, from the point and
/// its UV), subdividing first until edges average `feature` px, or
/// `DISPLACE_MIN_EDGE_PX` on screen (`screen`: screen px per layer unit),
/// whichever is longer. Copies of one point (hard edges, UV seams) move
/// together, by the first copy's offset, so nothing cracks.
fn displace(mesh: &mut Mesh, feature: f64, screen: f64, offset: impl Fn([f64; 3], [f32; 2]) -> f64) {
    let target = feature.max(DISPLACE_MIN_EDGE_PX / screen).max(0.5);
    loop {
        // The mean edge, not the longest: a few long seam or equator edges
        // would force another 4x subdivision of the whole mesh.
        let d = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() as f64;
        let total: f64 = mesh.tris.iter().map(|t| {
            let p = t.map(|i| mesh.positions[i as usize]);
            d(p[0], p[1]) + d(p[1], p[2]) + d(p[2], p[0])
        }).sum();
        let mean = total / (3 * mesh.tris.len().max(1)) as f64;
        if mean <= target || mesh.tris.len() * 4 > DISPLACE_MAX_TRIS {
            break;
        }
        subdivide(mesh);
    }
    // One direction per point (welded), the average of its copies' normals.
    let key = |p: [f32; 3]| [(p[0] * 64.0).round() as i64, (p[1] * 64.0).round() as i64, (p[2] * 64.0).round() as i64];
    let mut dirs: std::collections::HashMap<[i64; 3], ([f64; 3], f64)> = std::collections::HashMap::new();
    for (i, (p, n)) in mesh.positions.iter().zip(&mesh.normals).enumerate() {
        let e = dirs.entry(key(*p)).or_insert_with(|| ([0.0; 3], offset([p[0] as f64, p[1] as f64, p[2] as f64], mesh.uvs.get(i).copied().unwrap_or([0.0; 2]))));
        e.0 = add(e.0, [n[0] as f64, n[1] as f64, n[2] as f64]);
    }
    let moved: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|p| {
            let q = [p[0] as f64, p[1] as f64, p[2] as f64];
            let (d, h) = dirs[&key(*p)];
            let d = norm(d);
            [(q[0] + d[0] * h) as f32, (q[1] + d[1] * h) as f32, (q[2] + d[2] * h) as f32]
        })
        .collect();
    mesh.positions = moved;
    face_normals(mesh);
}

/// Normals from the faces around each vertex (area weighted), keeping
/// each one's side (zero normals take the faces' winding).
pub(crate) fn face_normals(mesh: &mut Mesh) {
    let mut acc = vec![[0.0f64; 3]; mesh.positions.len()];
    for t in &mesh.tris {
        let p = t.map(|i| {
            let v = mesh.positions[i as usize];
            [v[0] as f64, v[1] as f64, v[2] as f64]
        });
        let a = sub(p[1], p[0]);
        let b = sub(p[2], p[0]);
        let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        for i in t {
            acc[*i as usize] = add(acc[*i as usize], n);
        }
    }
    for (n, a) in mesh.normals.iter_mut().zip(acc) {
        let mut d = norm(a);
        if dot(d, [n[0] as f64, n[1] as f64, n[2] as f64]) < 0.0 {
            d = mul(d, -1.0);
        }
        *n = [d[0] as f32, d[1] as f32, d[2] as f32];
    }
}

/// Splits every triangle into four at its edges' midpoints (shared, so the
/// mesh stays watertight).
pub(crate) fn subdivide(mesh: &mut Mesh) {
    let mut mid: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    let mut point = |mesh: &mut Mesh, a: u32, b: u32| -> u32 {
        let k = (a.min(b), a.max(b));
        if let Some(&i) = mid.get(&k) {
            return i;
        }
        let (pa, pb) = (mesh.positions[a as usize], mesh.positions[b as usize]);
        let (na, nb) = (mesh.normals[a as usize], mesh.normals[b as usize]);
        let (ua, ub) = (mesh.uvs[a as usize], mesh.uvs[b as usize]);
        let n = norm([(na[0] + nb[0]) as f64, (na[1] + nb[1]) as f64, (na[2] + nb[2]) as f64]);
        mesh.positions.push([(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0, (pa[2] + pb[2]) / 2.0]);
        mesh.normals.push([n[0] as f32, n[1] as f32, n[2] as f32]);
        mesh.uvs.push([(ua[0] + ub[0]) / 2.0, (ua[1] + ub[1]) / 2.0]);
        let i = (mesh.positions.len() - 1) as u32;
        mid.insert(k, i);
        i
    };
    let (tris, tags, edges) = (std::mem::take(&mut mesh.tris), std::mem::take(&mut mesh.tags), std::mem::take(&mut mesh.edges));
    for ((t, tag), e) in tris.into_iter().zip(tags).zip(edges) {
        let [a, b, c] = t;
        let (ab, bc, ca) = (point(mesh, a, b), point(mesh, b, c), point(mesh, c, a));
        // Edge k is opposite vertex k: c–a is bit 1, a–b bit 2, b–c bit 0.
        let (e_bc, e_ca, e_ab) = (e & 1, (e >> 1) & 1, (e >> 2) & 1);
        mesh.tri_edges(a, ab, ca, tag, (e_ca << 1) | (e_ab << 2));
        mesh.tri_edges(ab, b, bc, tag, e_bc | (e_ab << 2));
        mesh.tri_edges(ca, bc, c, tag, e_bc | (e_ca << 1));
        mesh.tri_edges(ab, bc, ca, tag, 0);
    }
}

fn steps_for(r: f64, detail: f64) -> usize {
    circle_steps(r, detail, TAU)
}

// ------------------------------------------------------------- profiles

/// A profile point: (distance from the axis, position along it) and the
/// outward normal in that plane.
#[derive(Clone, Copy, Debug)]
struct PV {
    r: f64,
    h: f64,
    nr: f64,
    nh: f64,
}

/// A smooth run of profile points (corners start a new run).
#[derive(Clone, Debug)]
struct Profile {
    pts: Vec<PV>,
    part: u32,
}

impl Profile {
    /// Circular arc about `c` of radius `r` from angle `a0` to `a1`
    /// (radians; 0 points away from the axis, positive turns toward +h).
    fn arc(c: (f64, f64), r: f64, a0: f64, a1: f64, n: usize, part: u32) -> Profile {
        let n = n.max(1);
        let pts = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f64 / n as f64;
                let (s, co) = a.sin_cos();
                PV { r: c.0 + r * co, h: c.1 + r * s, nr: co, nh: s }
            })
            .collect();
        Profile { pts, part }
    }
    fn extend(&mut self, other: Profile) {
        let skip = usize::from(matches!((self.pts.last(), other.pts.first()), (Some(a), Some(b)) if (a.r - b.r).abs() < 1e-9 && (a.h - b.h).abs() < 1e-9));
        self.pts.extend(other.pts.into_iter().skip(skip));
    }
    /// A straight run from `a` to `b` with its normal on the right of the
    /// direction of travel when `right`, else on the left.
    fn line(a: (f64, f64), b: (f64, f64), outward: (f64, f64), part: u32) -> Profile {
        let l = outward.0.hypot(outward.1).max(1e-12);
        let (nr, nh) = (outward.0 / l, outward.1 / l);
        Profile { pts: vec![PV { r: a.0, h: a.1, nr, nh }, PV { r: b.0, h: b.1, nr, nh }], part }
    }
    /// A polyline split into smooth runs at corners sharper than ~35°.
    /// Normals point to the right of the direction of travel (for a closed
    /// outline drawn clockwise on screen, outward).
    fn from_polyline(pts: &[(f64, f64)], closed: bool, part: u32) -> Vec<Profile> {
        let mut p: Vec<(f64, f64)> = Vec::with_capacity(pts.len());
        for &q in pts {
            if p.last().is_none_or(|l: &(f64, f64)| (l.0 - q.0).hypot(l.1 - q.1) > 1e-6) {
                p.push(q);
            }
        }
        if closed && p.len() > 2 && (p[0].0 - p[p.len() - 1].0).hypot(p[0].1 - p[p.len() - 1].1) <= 1e-6 {
            p.pop();
        }
        if p.len() < 2 {
            return Vec::new();
        }
        // Profiles turn about the axis with h along the axis: the right of
        // travel in (r, h) is (dh, −dr) — outward for a clockwise
        // (screen) outline; flip so the outline's outside is outward.
        let area: f64 = (0..p.len()).map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a.0 * b.1 - b.0 * a.1
        }).sum();
        let flip = if closed && area < 0.0 { -1.0 } else { 1.0 };
        let n = p.len();
        let edges = if closed { n } else { n - 1 };
        let edge_normal = |i: usize| {
            let (a, b) = (p[i % n], p[(i + 1) % n]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = dx.hypot(dy).max(1e-12);
            (dy / l * flip, -dx / l * flip)
        };
        let sharp = |i: usize| -> bool {
            // Corner at vertex i between edge i−1 and edge i.
            let (a, b) = (edge_normal((i + edges - 1) % edges), edge_normal(i % edges));
            a.0 * b.0 + a.1 * b.1 < 0.82
        };
        let mut out = Vec::new();
        let mut cur: Vec<PV> = Vec::new();
        let vertex = |i: usize, n0: (f64, f64)| PV { r: p[i % n].0, h: p[i % n].1, nr: n0.0, nh: n0.1 };
        // Start at a corner when there is one, so closed outlines split cleanly.
        let start = if closed { (0..n).find(|&i| sharp(i)).unwrap_or(0) } else { 0 };
        let count = if closed { n + 1 } else { n };
        for k in 0..count {
            let i = start + k;
            let at_end = !closed && (k == 0 || k == n - 1);
            let (ein, eout) = ((i + edges - 1) % edges, i % edges);
            if at_end {
                let e = if k == 0 { edge_normal(eout) } else { edge_normal(ein) };
                cur.push(vertex(i, e));
                continue;
            }
            if sharp(i % n) && closed || (!closed && sharp(i)) {
                if !cur.is_empty() {
                    cur.push(vertex(i, edge_normal(ein)));
                    out.push(Profile { pts: std::mem::take(&mut cur), part });
                }
                if k < count - 1 {
                    cur.push(vertex(i, edge_normal(eout)));
                }
            } else {
                let (a, b) = (edge_normal(ein), edge_normal(eout));
                let m = (a.0 + b.0, a.1 + b.1);
                let l = m.0.hypot(m.1).max(1e-12);
                cur.push(vertex(i, (m.0 / l, m.1 / l)));
            }
        }
        if cur.len() >= 2 {
            out.push(Profile { pts: cur, part });
        }
        out
    }
}

/// The torus tube's profile as a single smooth loop.
fn ring_profile(ring: &Profile) -> Vec<Profile> {
    vec![ring.clone()]
}

fn cylinder_profile(rb: f64, rt: f64, height: f64, bevel: f64, detail: f64) -> Vec<Profile> {
    let hh = height / 2.0;
    let b = bevel.max(0.0).min(hh).min(rb.max(rt) * 0.999);
    let n = circle_steps(b, detail, FRAC_PI_2).min(16);
    // Side slope: from bottom (rb, +hh) to top (rt, −hh); y is down.
    let side_n = (2.0 * hh, rt - rb);
    let mut lines = Vec::new();
    if b <= 1e-9 {
        lines.push(Profile::line((0.0, hh), (rb, hh), (0.0, 1.0), PART_CAPS));
        lines.push(Profile::line((rb, hh), (rt, -hh), side_n, PART_SIDE));
        lines.push(Profile::line((rt, -hh), (0.0, -hh), (0.0, -1.0), PART_CAPS));
    } else {
        lines.push(Profile::line((0.0, hh), ((rb - b).max(0.0), hh), (0.0, 1.0), PART_CAPS));
        lines.push(Profile::arc(((rb - b).max(0.0), hh - b), b, FRAC_PI_2, 0.0, n, PART_BEVEL));
        lines.push(Profile::line((rb, hh - b), (rt, -hh + b), side_n, PART_SIDE));
        lines.push(Profile::arc(((rt - b).max(0.0), -hh + b), b, 0.0, -FRAC_PI_2, n, PART_BEVEL));
        lines.push(Profile::line(((rt - b).max(0.0), -hh), (0.0, -hh), (0.0, -1.0), PART_CAPS));
    }
    lines
}

/// The point at angle `a` (radians, 0 = up, clockwise on screen) and
/// distance `r` from the axis, `h` along it. Axis z faces the camera;
/// axis y stands up (h down the screen), angle 0 toward the viewer.
fn around(axis_y: bool, a: f64, r: f64, h: f64) -> V3 {
    let (s, c) = a.sin_cos();
    if axis_y {
        [r * s, h, -r * c]
    } else {
        [r * s, -r * c, h]
    }
}

fn around_dir(axis_y: bool, a: f64, nr: f64, nh: f64) -> V3 {
    around(axis_y, a, nr, nh)
}

/// d(around)/da direction at angle `a` (unit, for caps).
fn tangent_dir(axis_y: bool, a: f64) -> V3 {
    let (s, c) = a.sin_cos();
    if axis_y {
        [c, 0.0, s]
    } else {
        [c, s, 0.0]
    }
}

#[allow(clippy::too_many_arguments)]
fn revolve_pieces(mesh: &mut Mesh, lines: &[Profile], angles: (f64, f64), segments: u32, gap: f64, spans: &[f64], steps: usize, axis_y: bool, classify: bool) {
    for (k, (a0, a1)) in pieces(angles.0, angles.1, segments, gap, spans).into_iter().enumerate() {
        let piece = revolve(lines, a0.to_radians(), a1.to_radians(), steps, k as u32, axis_y, classify);
        mesh.append(&piece);
    }
}

/// Turns profile runs from `a0` to `a1` radians in `steps` per full turn.
/// The part of a revolved band by which way it faces: along the axis
/// (front toward −axis, back toward +axis), away from it (side) or toward
/// it (inside).
fn facing_part(nr: f64, nh: f64) -> u32 {
    if nh.abs() > nr.abs() * 1.2 {
        if nh < 0.0 {
            PART_FRONT
        } else {
            PART_BACK
        }
    } else if nr >= 0.0 {
        PART_SIDE
    } else {
        PART_INSIDE
    }
}

/// `classify`: side surfaces take their part from the way they face.
fn revolve(lines: &[Profile], a0: f64, a1: f64, steps: usize, segment: u32, axis_y: bool, classify: bool) -> Mesh {
    let mut m = Mesh::default();
    let n = ((steps as f64 * (a1 - a0).abs() / TAU).ceil() as usize).max(1);
    for line in lines {
        let part_of = |j: usize| {
            if classify && line.part == PART_SIDE {
                let (p, q) = (line.pts[j], line.pts[j + 1]);
                tag(facing_part(p.nr + q.nr, p.nh + q.nh), segment)
            } else {
                tag(line.part, segment)
            }
        };
        let mut prev: Option<Vec<u32>> = None;
        for i in 0..=n {
            let a = a0 + (a1 - a0) * i as f64 / n as f64;
            let ring: Vec<u32> = line.pts.iter().map(|p| m.vertex(around(axis_y, a, p.r, p.h), around_dir(axis_y, a, p.nr, p.nh))).collect();
            if let Some(prev) = &prev {
                for j in 0..ring.len() - 1 {
                    // Skip quads collapsed on the axis.
                    let (p, q) = (line.pts[j], line.pts[j + 1]);
                    if p.r.abs() < 1e-9 && q.r.abs() < 1e-9 {
                        continue;
                    }
                    let t = part_of(j);
                    if p.r.abs() < 1e-9 {
                        m.tri(prev[j], ring[j + 1], prev[j + 1], t);
                    } else if q.r.abs() < 1e-9 {
                        m.tri(prev[j], ring[j], prev[j + 1], t);
                    } else {
                        m.quad(prev[j], ring[j], ring[j + 1], prev[j + 1], t);
                    }
                }
            }
            prev = Some(ring);
        }
    }
    m
}

/// Flat cap closing a torus tube at angle `a`; `dir` −1 faces back along
/// the turn (start), +1 forward (end).
fn disc_cap(mesh: &mut Mesh, r: f64, t: f64, a: f64, dir: f64, steps: usize, segment: u32) {
    let normal = mul(tangent_dir(false, a), dir);
    let c = mesh.vertex(around(false, a, r, 0.0), normal);
    let ring: Vec<u32> = (0..=steps)
        .map(|i| {
            let b = TAU * i as f64 / steps as f64;
            mesh.vertex(around(false, a, r + t * b.cos(), t * b.sin()), normal)
        })
        .collect();
    for w in ring.windows(2) {
        mesh.tri_edges(c, w[0], w[1], tag(PART_CAPS, segment), 0b001);
    }
}

/// A half sphere closing a torus tube at angle `a`.
fn round_cap(mesh: &mut Mesh, r: f64, t: f64, a: f64, dir: f64, steps: usize, segment: u32) {
    let centre = around(false, a, r, 0.0);
    let out = mul(tangent_dir(false, a), dir);
    let radial = around(false, a, 1.0, 0.0);
    let axis = [0.0, 0.0, 1.0];
    let lat = (steps / 4).max(3);
    let mut prev: Option<Vec<u32>> = None;
    for j in 0..=lat {
        let phi = FRAC_PI_2 * j as f64 / lat as f64;
        let ring: Vec<u32> = (0..=steps)
            .map(|i| {
                let b = TAU * i as f64 / steps as f64;
                let rim = add(mul(radial, b.cos()), mul(axis, b.sin()));
                let n = add(mul(rim, phi.cos()), mul(out, phi.sin()));
                mesh.vertex(add(centre, mul(n, t)), n)
            })
            .collect();
        if let Some(prev) = &prev {
            for i in 0..steps {
                if j == lat {
                    mesh.tri(prev[i], prev[i + 1], ring[i], tag(PART_CAPS, segment));
                } else {
                    mesh.quad(prev[i], prev[i + 1], ring[i + 1], ring[i], tag(PART_CAPS, segment));
                }
            }
        }
        prev = Some(ring);
    }
}

/// An open profile closed along the axis (its ends dropped onto r = 0).
fn close_on_axis(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = pts.to_vec();
    if let (Some(&first), Some(&last)) = (pts.first(), pts.last()) {
        if last.0.abs() > 1e-9 {
            out.push((0.0, last.1));
        }
        if first.0.abs() > 1e-9 {
            out.push((0.0, first.1));
        }
    }
    out
}

/// A flat cap: the profile polygon at angle `a`.
fn profile_cap(mesh: &mut Mesh, pts: &[(f64, f64)], a: f64, dir: f64, axis_y: bool, segment: u32) {
    let tris = triangulate(&[pts.to_vec()]);
    let normal = mul(tangent_dir(axis_y, a), dir);
    let base = mesh.positions.len() as u32;
    for p in pts {
        mesh.vertex(around(axis_y, a, p.0, p.1), normal);
    }
    mesh.fill(base, &tris, &[pts.len()], tag(PART_CAPS, segment));
}

// ------------------------------------------------------------------ box

fn rounded_box(mesh: &mut Mesh, size: V3, bevel: f64, detail: f64) {
    let h = size.map(|v| v.abs() / 2.0);
    let r = bevel.max(0.0).min(h[0]).min(h[1]).min(h[2]);
    let n = if r > 1e-9 { circle_steps(r, detail, FRAC_PI_2).clamp(2, 16) } else { 0 };
    // Coordinates along an axis: the bevel band spread by angle.
    let coords = |half: f64| -> Vec<(f64, bool)> {
        let mut c = Vec::new();
        if n == 0 {
            return vec![(-half, false), (half, false)];
        }
        for k in 0..=n {
            let t = (FRAC_PI_4 * (n - k) as f64 / n as f64).tan();
            c.push((-(half - r) - r * t, k < n));
        }
        for k in 0..=n {
            let t = (FRAC_PI_4 * k as f64 / n as f64).tan();
            c.push(((half - r) + r * t, k > 0));
        }
        c
    };
    let inner = h.map(|v| v - r);
    // (normal axis, sign, u axis, v axis, part)
    let faces = [(2, -1.0, 0, 1, PART_FRONT), (2, 1.0, 0, 1, PART_BACK), (0, -1.0, 1, 2, PART_SIDE), (0, 1.0, 1, 2, PART_SIDE), (1, -1.0, 0, 2, PART_SIDE), (1, 1.0, 0, 2, PART_SIDE)];
    for (axis, sign, u, v, part) in faces {
        let (cu, cv) = (coords(h[u]), coords(h[v]));
        let base = mesh.positions.len() as u32;
        for &(y, _) in &cv {
            for &(x, _) in &cu {
                let mut p = [0.0; 3];
                p[axis] = sign * h[axis];
                p[u] = x;
                p[v] = y;
                let mut face_n = [0.0; 3];
                face_n[axis] = sign;
                if r <= 1e-9 {
                    mesh.vertex(p, face_n);
                } else {
                    let q = [p[0].clamp(-inner[0], inner[0]), p[1].clamp(-inner[1], inner[1]), p[2].clamp(-inner[2], inner[2])];
                    let d = norm(sub(p, q));
                    mesh.vertex(add(q, mul(d, r)), d);
                }
            }
        }
        let w = cu.len() as u32;
        for j in 0..cv.len() - 1 {
            for i in 0..cu.len() - 1 {
                // Every cell but the middle one lies on the rounded edges.
                let band = n > 0 && (i != n || j != n);
                let flat_cell = (cu[i + 1].0 - cu[i].0).abs() < 1e-12 || (cv[j + 1].0 - cv[j].0).abs() < 1e-12;
                if flat_cell {
                    continue;
                }
                let t = tag(if band { PART_BEVEL } else { part }, 0);
                let a = base + j as u32 * w + i as u32;
                mesh.quad(a, a + 1, a + w + 1, a + w, t);
            }
        }
    }
}

// --------------------------------------------------------------- extrude

fn signed_area(p: &[(f64, f64)]) -> f64 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        / 2.0
}

fn point_in(poly: &[(f64, f64)], q: (f64, f64)) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.1 > q.1) != (b.1 > q.1) && q.0 < (b.0 - a.0) * (q.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

fn clean(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut p: Vec<(f64, f64)> = Vec::with_capacity(pts.len());
    for &q in pts {
        if p.last().is_none_or(|l: &(f64, f64)| (l.0 - q.0).hypot(l.1 - q.1) > 1e-6) {
            p.push(q);
        }
    }
    while p.len() > 2 && (p[0].0 - p[p.len() - 1].0).hypot(p[0].1 - p[p.len() - 1].1) <= 1e-6 {
        p.pop();
    }
    // Drop collinear points.
    let mut i = 0;
    while p.len() > 3 && i < p.len() {
        let n = p.len();
        let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
        let cr = (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0);
        let l = (b.0 - a.0).hypot(b.1 - a.1) * (c.0 - b.0).hypot(c.1 - b.1);
        if cr.abs() <= 1e-9 * l.max(1e-12) && (b.0 - a.0) * (c.0 - b.0) + (b.1 - a.1) * (c.1 - b.1) > 0.0 {
            p.remove(i);
        } else {
            i += 1;
        }
    }
    p
}

/// Outer outlines with their holes: outers have positive area, holes
/// negative (nesting by even/odd depth).
fn regions(contours: Vec<Vec<(f64, f64)>>) -> Vec<Vec<Vec<(f64, f64)>>> {
    let contours: Vec<Vec<(f64, f64)>> = contours.into_iter().map(|c| clean(&c)).filter(|c| c.len() >= 3 && signed_area(c).abs() > 1e-6).collect();
    let depth: Vec<usize> = contours
        .iter()
        .enumerate()
        .map(|(i, c)| contours.iter().enumerate().filter(|(j, o)| *j != i && signed_area(o).abs() > signed_area(c).abs() && point_in(o, c[0])).count())
        .collect();
    let mut out: Vec<(usize, Vec<Vec<(f64, f64)>>)> = Vec::new();
    for (i, c) in contours.iter().enumerate() {
        if depth[i] % 2 == 0 {
            let mut c = c.clone();
            if signed_area(&c) < 0.0 {
                c.reverse();
            }
            out.push((i, vec![c]));
        }
    }
    for (i, c) in contours.iter().enumerate() {
        if depth[i] % 2 == 1 {
            // The smallest enclosing outer.
            let owner = out
                .iter_mut()
                .filter(|(_, r)| point_in(&r[0], c[0]))
                .min_by(|a, b| signed_area(&a.1[0]).total_cmp(&signed_area(&b.1[0])));
            if let Some((_, r)) = owner {
                let mut c = c.clone();
                if signed_area(&c) > 0.0 {
                    c.reverse();
                }
                r.push(c);
            }
        }
    }
    out.into_iter().map(|(_, r)| r).collect()
}

fn cross2(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

fn segments_cross(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    let (d1, d2, d3, d4) = (cross2(c, d, a), cross2(c, d, b), cross2(a, b, c), cross2(a, b, d));
    ((d1 > 1e-12 && d2 < -1e-12) || (d1 < -1e-12 && d2 > 1e-12)) && ((d3 > 1e-12 && d4 < -1e-12) || (d3 < -1e-12 && d4 > 1e-12))
}

/// Triangulates an outer outline (positive area) with holes (negative)
/// by ear clipping after bridging each hole to the outline. Indices run
/// over the contours' points concatenated in order.
pub fn triangulate(contours: &[Vec<(f64, f64)>]) -> Vec<[usize; 3]> {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut rings: Vec<Vec<usize>> = Vec::new();
    for c in contours {
        let base = pts.len();
        pts.extend_from_slice(c);
        rings.push((base..base + c.len()).collect());
    }
    if rings.is_empty() || rings[0].len() < 3 {
        return Vec::new();
    }
    let mut outer = rings[0].clone();
    if signed_area(&outer.iter().map(|&i| pts[i]).collect::<Vec<_>>()) < 0.0 {
        outer.reverse();
    }
    let mut holes: Vec<Vec<usize>> = rings[1..].to_vec();
    for h in &mut holes {
        if signed_area(&h.iter().map(|&i| pts[i]).collect::<Vec<_>>()) > 0.0 {
            h.reverse();
        }
    }
    // Bridge holes rightmost first.
    holes.sort_by(|a, b| {
        let mx = |h: &Vec<usize>| h.iter().map(|&i| pts[i].0).fold(f64::NEG_INFINITY, f64::max);
        mx(b).total_cmp(&mx(a))
    });
    for (hi, hole) in holes.iter().enumerate() {
        let (mi, _) = hole.iter().enumerate().max_by(|a, b| pts[*a.1].0.total_cmp(&pts[*b.1].0)).unwrap();
        let m = pts[hole[mi]];
        // The nearest outline vertex the bridge can reach without crossing
        // any edge.
        let edges: Vec<(usize, usize)> = std::iter::once(&outer)
            .chain(holes[hi..].iter())
            .flat_map(|r| (0..r.len()).map(move |k| (r[k], r[(k + 1) % r.len()])))
            .collect();
        let mut best: Option<(f64, usize)> = None;
        for (k, &oi) in outer.iter().enumerate() {
            let p = pts[oi];
            let d = (p.0 - m.0).powi(2) + (p.1 - m.1).powi(2);
            if best.is_some_and(|(bd, _)| d >= bd) {
                continue;
            }
            if edges.iter().any(|&(a, b)| a != oi && b != oi && a != hole[mi] && b != hole[mi] && segments_cross(m, p, pts[a], pts[b])) {
                continue;
            }
            best = Some((d, k));
        }
        let Some((_, k)) = best else { continue };
        let mut spliced = Vec::with_capacity(outer.len() + hole.len() + 2);
        spliced.extend_from_slice(&outer[..=k]);
        for j in 0..=hole.len() {
            spliced.push(hole[(mi + j) % hole.len()]);
        }
        spliced.push(outer[k]);
        spliced.extend_from_slice(&outer[k + 1..]);
        outer = spliced;
    }
    ear_clip(&pts, outer)
}

fn ear_clip(pts: &[(f64, f64)], mut poly: Vec<usize>) -> Vec<[usize; 3]> {
    let mut out = Vec::with_capacity(poly.len());
    let mut guard = 0;
    let mut i = 0;
    while poly.len() > 3 && guard < poly.len() * 3 + 16 {
        let n = poly.len();
        let (ia, ib, ic) = (poly[(i + n - 1) % n], poly[i % n], poly[(i + 1) % n]);
        let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
        let convex = cross2(a, b, c) > 1e-12;
        let ear = convex
            && !poly.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = pts[j];
                if p == a || p == b || p == c {
                    return false;
                }
                cross2(a, b, p) >= -1e-12 && cross2(b, c, p) >= -1e-12 && cross2(c, a, p) >= -1e-12
            });
        if ear {
            out.push([ia, ib, ic]);
            poly.remove(i % n);
            guard = 0;
            if i > 0 {
                i -= 1;
            }
        } else {
            i = (i + 1) % n;
            guard += 1;
        }
    }
    if poly.len() == 3 && cross2(pts[poly[0]], pts[poly[1]], pts[poly[2]]).abs() > 1e-12 {
        out.push([poly[0], poly[1], poly[2]]);
    } else if poly.len() > 3 {
        // Degenerate leftovers (self-touching outlines): fan them.
        for k in 1..poly.len() - 1 {
            out.push([poly[0], poly[k], poly[k + 1]]);
        }
    }
    out
}

/// Per-vertex wall data of a closed contour: the miter offset (outward,
/// unit edge distance) and the shading normals (one, or two at corners).
struct Wall {
    p: (f64, f64),
    miter: (f64, f64),
    normals: Vec<(f64, f64)>,
}

fn walls(c: &[(f64, f64)]) -> Vec<Wall> {
    let n = c.len();
    let en = |i: usize| {
        let (a, b) = (c[i % n], c[(i + 1) % n]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx.hypot(dy).max(1e-12);
        (dy / l, -dx / l)
    };
    (0..n)
        .map(|i| {
            let (a, b) = (en(i + n - 1), en(i));
            let m = (a.0 + b.0, a.1 + b.1);
            let ml = m.0.hypot(m.1);
            let cos = a.0 * b.0 + a.1 * b.1;
            let mu = if ml > 1e-9 { (m.0 / ml, m.1 / ml) } else { b };
            // Miter length 1/cos(half angle), limited.
            let half_cos = (mu.0 * b.0 + mu.1 * b.1).max(0.4);
            let miter = (mu.0 / half_cos, mu.1 / half_cos);
            let normals = if cos < 0.82 { vec![a, b] } else { vec![mu] };
            Wall { p: c[i], miter, normals }
        })
        .collect()
}

fn extrude(mesh: &mut Mesh, path: &PathMorph, depth: f64, bevel: f64, detail: f64) -> Result<(), String> {
    let subpaths = crate::path::morph(path, 0.3 / detail)?;
    let regions = regions(subpaths.into_iter().map(|(p, _)| p).collect());
    if regions.is_empty() {
        return Err("extrude: the outline has no closed area".into());
    }
    let d = depth.max(0.0) / 2.0;
    // The bevel can't exceed the half depth or eat the outline.
    let mut b = bevel.max(0.0).min(d);
    for r in &regions {
        for c in r {
            let (mut lo, mut hi) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
            for p in c {
                lo = (lo.0.min(p.0), lo.1.min(p.1));
                hi = (hi.0.max(p.0), hi.1.max(p.1));
            }
            b = b.min((hi.0 - lo.0).min(hi.1 - lo.1) * 0.3);
        }
    }
    let segs = if b > 1e-9 { circle_steps(b, detail, FRAC_PI_2).clamp(2, 12) } else { 0 };
    for region in &regions {
        let tris = triangulate(region);
        let all_walls: Vec<Vec<Wall>> = region.iter().map(|c| walls(c)).collect();
        // Caps (inset by the bevel): front at −d (toward the viewer), back at +d.
        for (z, nz, part) in [(-d, -1.0, PART_FRONT), (d, 1.0, PART_BACK)] {
            let base = mesh.positions.len() as u32;
            for w in all_walls.iter().flatten() {
                let p = (w.p.0 - w.miter.0 * b, w.p.1 - w.miter.1 * b);
                mesh.vertex([p.0, p.1, z], [0.0, 0.0, nz]);
            }
            let sizes: Vec<usize> = all_walls.iter().map(Vec::len).collect();
            mesh.fill(base, &tris, &sizes, tag(part, 0));
        }
        // Walls: rings from the front face, round the bevel, down the side
        // and round the back bevel. Each ring: (inset, z, normal xy scale,
        // normal z, part).
        let mut rings: Vec<(f64, f64, f64, f64, u32)> = Vec::new();
        for k in 0..=segs {
            let phi = FRAC_PI_2 * k as f64 / segs.max(1) as f64;
            if segs == 0 {
                break;
            }
            rings.push((b * (1.0 - phi.sin()), -d + b - b * phi.cos(), phi.sin(), -phi.cos(), PART_BEVEL));
        }
        rings.push((0.0, -d + b, 1.0, 0.0, PART_SIDE));
        rings.push((0.0, d - b, 1.0, 0.0, PART_SIDE));
        for k in 0..=segs {
            if segs == 0 {
                break;
            }
            let phi = FRAC_PI_2 * k as f64 / segs as f64;
            rings.push((b * (1.0 - phi.cos()), d - b + b * phi.sin(), phi.cos(), phi.sin(), PART_BEVEL));
        }
        for wl in &all_walls {
            // Columns: one per shading normal; corners have two at one point.
            let cols: Vec<(usize, (f64, f64))> = wl.iter().enumerate().flat_map(|(i, w)| w.normals.iter().map(move |n| (i, *n))).collect();
            let nc = cols.len();
            let mut grid: Vec<Vec<u32>> = Vec::new();
            for &(inset, z, nxy, nz, _) in &rings {
                let row = cols
                    .iter()
                    .map(|&(i, n)| {
                        let w = &wl[i];
                        mesh.vertex([w.p.0 - w.miter.0 * inset, w.p.1 - w.miter.1 * inset, z], [n.0 * nxy, n.1 * nxy, nz])
                    })
                    .collect();
                grid.push(row);
            }
            for r in 0..rings.len() - 1 {
                let part = if rings[r].4 == PART_SIDE && rings[r + 1].4 == PART_SIDE { PART_SIDE } else { PART_BEVEL };
                if (rings[r].1 - rings[r + 1].1).abs() < 1e-12 && (rings[r].0 - rings[r + 1].0).abs() < 1e-12 {
                    continue;
                }
                for k in 0..nc {
                    let k2 = (k + 1) % nc;
                    // The two columns of one corner share a point: no quad.
                    if cols[k].0 == cols[k2].0 {
                        continue;
                    }
                    mesh.quad(grid[r][k], grid[r][k2], grid[r + 1][k2], grid[r + 1][k], tag(part, 0));
                }
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- sweep

#[allow(clippy::too_many_arguments)]
fn sweep(mesh: &mut Mesh, path: &PathMorph, radius: f64, width: f64, profile: SweepProfile, trim: [f64; 2], caps: Caps, head: f64, segments: u32, gap: f64, spans: &[f64], detail: f64) -> Result<(), String> {
    let subpaths = crate::path::morph(path, 0.25 / detail)?;
    let (t0, t1) = (trim[0].clamp(0.0, 1.0), trim[1].clamp(0.0, 1.0));
    if t1 <= t0 {
        return Ok(());
    }
    let r = radius.max(0.0);
    let count = segments.max(1);
    let span = t1 - t0;
    let gap = gap.max(0.0).min(span / count as f64 * 0.9);
    let weights = piece_weights(count as usize, spans);
    let total: f64 = weights.iter().sum();
    let room = span - gap * (count - 1) as f64;
    let mut a = t0;
    for k in 0..count {
        let each = room * weights[k as usize] / total;
        let lines = crate::path::trim(&subpaths, a, a + each);
        a += each + gap;
        for line in lines {
            tube(mesh, &line, r, width, profile, caps, k, detail);
        }
    }
    if head > 0.0 {
        if let Some((p, _)) = crate::path::point_at(&subpaths, t1) {
            let n = circle_steps(head, detail, TAU);
            let profile = Profile::arc((0.0, 0.0), head, -FRAC_PI_2, FRAC_PI_2, n / 2, PART_CAPS);
            let mut ball = revolve(&[profile], 0.0, TAU, n, count - 1, true, false);
            for q in &mut ball.positions {
                q[0] += p.0 as f32;
                q[1] += p.1 as f32;
            }
            mesh.append(&ball);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn tube(mesh: &mut Mesh, line: &[(f64, f64)], r: f64, width: f64, profile: SweepProfile, caps: Caps, segment: u32, detail: f64) {
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(line.len());
    for &q in line {
        if pts.last().is_none_or(|l: &(f64, f64)| (l.0 - q.0).hypot(l.1 - q.1) > 1e-6) {
            pts.push(q);
        }
    }
    if pts.len() < 2 {
        return;
    }
    let n = pts.len();
    let dir = |i: usize, j: usize| {
        let (dx, dy) = (pts[j].0 - pts[i].0, pts[j].1 - pts[i].1);
        let l = dx.hypot(dy).max(1e-12);
        (dx / l, dy / l)
    };
    let tangents: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let a = if i > 0 { dir(i - 1, i) } else { dir(0, 1) };
            let b = if i + 1 < n { dir(i, i + 1) } else { dir(n - 2, n - 1) };
            let m = (a.0 + b.0, a.1 + b.1);
            let l = m.0.hypot(m.1);
            if l > 1e-9 {
                (m.0 / l, m.1 / l)
            } else {
                b
            }
        })
        .collect();
    // Cross-section in (N, B) = (in-plane normal, z), with normals.
    let section: Vec<((f64, f64), (f64, f64))> = match profile {
        SweepProfile::Circle => {
            let s = circle_steps(r, detail, TAU).min(64);
            (0..=s)
                .map(|i| {
                    let a = TAU * i as f64 / s as f64;
                    let (sn, c) = a.sin_cos();
                    ((r * c, r * sn), (c, sn))
                })
                .collect()
        }
        SweepProfile::Square => {
            let mut v = Vec::new();
            let corners = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
            let normals = [(0.0, 1.0), (-1.0, 0.0), (0.0, -1.0), (1.0, 0.0)];
            for k in 0..4 {
                let (a, b) = (corners[k], corners[(k + 1) % 4]);
                v.push(((a.0 * r, a.1 * r), normals[k]));
                v.push(((b.0 * r, b.1 * r), normals[k]));
            }
            v
        }
        SweepProfile::Ribbon => {
            let hw = width.max(0.0) / 2.0;
            vec![((0.0, -hw), (1.0, 0.0)), ((0.0, hw), (1.0, 0.0))]
        }
    };
    let two = if profile == SweepProfile::Ribbon { TWO_SIDED } else { 0 };
    let t = tag(PART_SIDE, segment) | two;
    let mut prev: Option<Vec<u32>> = None;
    for i in 0..n {
        let (tx, ty) = tangents[i];
        let nn = [-ty, tx, 0.0];
        let bb = [0.0, 0.0, 1.0];
        let c = [pts[i].0, pts[i].1, 0.0];
        let ring: Vec<u32> = section.iter().map(|&((u, v), (nu, nv))| mesh.vertex(add(c, add(mul(nn, u), mul(bb, v))), add(mul(nn, nu), mul(bb, nv)))).collect();
        if let Some(prev) = &prev {
            let step = if profile == SweepProfile::Square { 2 } else { 1 };
            let mut j = 0;
            while j + 1 < ring.len() {
                mesh.quad(prev[j], ring[j], ring[j + 1], prev[j + 1], t);
                j += step;
            }
        }
        prev = Some(ring);
    }
    if profile == SweepProfile::Ribbon {
        return;
    }
    for (end, sign) in [(0usize, -1.0), (n - 1, 1.0)] {
        let (tx, ty) = tangents[end];
        let out = [tx * sign, ty * sign, 0.0];
        let nn = [-ty, tx, 0.0];
        let c = [pts[end].0, pts[end].1, 0.0];
        match (caps, profile) {
            (Caps::Round, SweepProfile::Circle) => {
                let s = section.len() - 1;
                let lat = (s / 4).max(3);
                let mut prev: Option<Vec<u32>> = None;
                for j in 0..=lat {
                    let phi = FRAC_PI_2 * j as f64 / lat as f64;
                    let ring: Vec<u32> = section
                        .iter()
                        .map(|&(_, (nu, nv))| {
                            let rim = add(mul(nn, nu), mul([0.0, 0.0, 1.0], nv));
                            let d = add(mul(rim, phi.cos()), mul(out, phi.sin()));
                            mesh.vertex(add(c, mul(d, r)), d)
                        })
                        .collect();
                    if let Some(prev) = &prev {
                        for i in 0..s {
                            mesh.quad(prev[i], prev[i + 1], ring[i + 1], ring[i], tag(PART_CAPS, segment));
                        }
                    }
                    prev = Some(ring);
                }
            }
            _ => {
                let centre = mesh.vertex(c, out);
                let ring: Vec<u32> = section.iter().map(|&((u, v), _)| mesh.vertex(add(c, add(mul(nn, u), mul([0.0, 0.0, 1.0], v))), out)).collect();
                for w in ring.windows(2) {
                    mesh.tri_edges(centre, w[0], w[1], tag(PART_CAPS, segment), 0b001);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(shape: MeshShape) -> MeshParams {
        MeshParams { shape, segments: 1, gap: 0.0, detail: 1.0, spans: Vec::new(), screen: 1.0, displace: None, deform: None }
    }

    fn path(d: &str) -> PathMorph {
        PathMorph { from: d.into(), to: None, t: 0.0 }
    }

    /// Normals point away from the centroid for convex closed meshes and
    /// every vertex lies where the shape says.
    fn check_outward(m: &Mesh, centre: V3) {
        assert!(!m.tris.is_empty());
        let mut agree = 0usize;
        for (t, _) in m.tris.iter().zip(&m.tags) {
            let p = t.map(|i| m.positions[i as usize].map(|v| v as f64));
            let c = mul(add(add(p[0], p[1]), p[2]), 1.0 / 3.0);
            let n = t.iter().fold([0.0; 3], |acc, &i| add(acc, m.normals[i as usize].map(|v| v as f64)));
            if dot(n, sub(c, centre)) >= -1e-6 {
                agree += 1;
            }
        }
        assert_eq!(agree, m.tris.len(), "every normal points outward");
    }

    /// Every primitive gets UVs in 0..1, one per vertex.
    #[test]
    fn primitives_have_uvs() {
        let shapes = [
            MeshShape::Sphere { radius: 100.0 },
            MeshShape::Plane { size: [300.0, 200.0] },
            MeshShape::Box { size: [200.0, 100.0, 60.0], bevel: 12.0 },
            MeshShape::Cylinder { radius: 80.0, top: 80.0, height: 120.0, bevel: 8.0 },
            MeshShape::Capsule { radius: 40.0, height: 200.0 },
            MeshShape::Torus { radius: 100.0, tube: 30.0, angles: [0.0, 360.0], caps: Caps::Flat },
            MeshShape::Extrude { path: path("M -50 -50 L 50 -50 L 50 50 L -50 50 Z"), depth: 40.0, bevel: 6.0 },
        ];
        for shape in shapes {
            let m = generate(&params(shape.clone())).unwrap();
            assert_eq!(m.uvs.len(), m.positions.len(), "{shape:?}");
            assert!(m.uvs.iter().all(|uv| (-1e-3..=1.001).contains(&uv[0]) && (-1e-3..=1.001).contains(&uv[1])), "{shape:?}");
            let spread = m.uvs.iter().fold((1.0f32, 0.0f32), |(lo, hi), uv| (lo.min(uv[0]), hi.max(uv[0])));
            assert!(spread.1 - spread.0 > 0.5, "{shape:?}: UVs span the surface");
        }
        // A box's front face: x and y across it.
        let b = generate(&params(MeshShape::Box { size: [200.0, 100.0, 60.0], bevel: 0.0 })).unwrap();
        for ((p, n), uv) in b.positions.iter().zip(&b.normals).zip(&b.uvs) {
            if n[2] < -0.99 {
                assert!((uv[0] - (p[0] + 100.0) / 200.0).abs() < 1e-3 && (uv[1] - (p[1] + 50.0) / 100.0).abs() < 1e-3);
            }
        }
    }

    /// Displacement: a sphere becomes a rock (radii vary by about the
    /// amount), subdivided where the noise is finer than its triangles,
    /// still closed (no cracks at its seams) and facing out; the same
    /// parameters give the same rock.
    #[test]
    fn displacement_roughens_without_cracks() {
        let field = crate::noise::Field { kind: crate::noise::NoiseKind::Fbm, scale: 12.0, octaves: 4, seed: 7, offset: [0.0; 3] };
        let mut p = params(MeshShape::Sphere { radius: 100.0 });
        p.displace = Some((Displacement::Noise(field), 12.0));
        let m = generate(&p).unwrap();
        let plain = generate(&params(MeshShape::Sphere { radius: 100.0 })).unwrap();
        assert!(m.tris.len() > plain.tris.len() * 3, "subdivided: {} vs {}", m.tris.len(), plain.tris.len());
        let radii: Vec<f32> = m.positions.iter().map(|q| (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt()).collect();
        let (lo, hi) = radii.iter().fold((f32::MAX, 0.0f32), |(a, b), r| (a.min(*r), b.max(*r)));
        assert!(hi - lo > 8.0 && lo > 85.0 && hi < 115.0, "radii {lo}..{hi}");
        check_outward(&m, [0.0; 3]);
        // Watertight: every edge (by position) is shared by two triangles.
        let key = |v: [f32; 3]| [(v[0] * 64.0).round() as i64, (v[1] * 64.0).round() as i64, (v[2] * 64.0).round() as i64];
        let mut count: std::collections::HashMap<([i64; 3], [i64; 3]), u32> = std::collections::HashMap::new();
        for t in &m.tris {
            for k in 0..3 {
                let (a, b) = (key(m.positions[t[k] as usize]), key(m.positions[t[(k + 1) % 3] as usize]));
                if a != b {
                    *count.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
        }
        assert!(count.values().all(|c| *c == 2), "{} open edges", count.values().filter(|c| **c != 2).count());
        assert_eq!(generate(&p).unwrap(), m);
        assert_ne!(p.key(), params(MeshShape::Sphere { radius: 100.0 }).key());
    }

    /// Displacement subdivides for the drawn size: fine noise on a 300 px
    /// rock stops once edges are about DISPLACE_MIN_EDGE_PX on screen (one
    /// subdivision; it ran on to 16x the plain mesh, near the 200 000 cap),
    /// and the same rock far away, a quarter the pixels, gets far fewer.
    #[test]
    fn displacement_follows_the_drawn_size() {
        let field = crate::noise::Field { kind: crate::noise::NoiseKind::Fbm, scale: 3.0, octaves: 3, seed: 3, offset: [0.0; 3] };
        let rock = |screen: f64, displace: bool| {
            let mut p = params(MeshShape::Sphere { radius: 150.0 });
            p.displace = displace.then_some((Displacement::Noise(field), 4.0));
            p.screen = screen;
            generate(&p).unwrap()
        };
        let (plain, near, far) = (rock(1.0, false), rock(1.0, true), rock(0.25, true));
        assert!(near.tris.len() <= plain.tris.len() * 4, "{} triangles for a {}-triangle rock: subdivided below {DISPLACE_MIN_EDGE_PX} px edges", near.tris.len(), plain.tris.len());
        assert!(far.tris.len() * 4 <= near.tris.len(), "far {} vs near {}", far.tris.len(), near.tris.len());
    }

    #[test]
    fn primitives_are_closed_and_outward() {
        let s = generate(&params(MeshShape::Sphere { radius: 100.0 })).unwrap();
        check_outward(&s, [0.0; 3]);
        for p in &s.positions {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 100.0).abs() < 1e-3);
        }
        let b = generate(&params(MeshShape::Box { size: [200.0, 100.0, 60.0], bevel: 12.0 })).unwrap();
        check_outward(&b, [0.0; 3]);
        let (lo, hi) = b.bounds();
        assert!((lo[0] + 100.0).abs() < 1e-3 && (hi[2] - 30.0).abs() < 1e-3);
        assert!(b.tags.iter().any(|t| tag_part(*t) == PART_BEVEL) && b.tags.iter().any(|t| tag_part(*t) == PART_FRONT));
        let c = generate(&params(MeshShape::Cylinder { radius: 80.0, top: 80.0, height: 120.0, bevel: 8.0 })).unwrap();
        check_outward(&c, [0.0; 3]);
        let cap = generate(&params(MeshShape::Capsule { radius: 40.0, height: 200.0 })).unwrap();
        check_outward(&cap, [0.0; 3]);
        let (lo, hi) = cap.bounds();
        assert!((lo[1] + 100.0).abs() < 1e-3 && (hi[1] - 100.0).abs() < 1e-3);
    }

    #[test]
    fn torus_faces_the_camera_and_splits_into_coloured_segments() {
        let t = generate(&MeshParams { shape: MeshShape::Torus { radius: 200.0, tube: 30.0, angles: [0.0, 360.0], caps: Caps::Flat }, segments: 4, gap: 10.0, detail: 1.0, spans: Vec::new(), screen: 1.0, displace: None, deform: None }).unwrap();
        let (lo, hi) = t.bounds();
        assert!(hi[0] > 225.0 && hi[0] < 230.01 && (lo[2] + 30.0).abs() < 0.5 && (hi[2] - 30.0).abs() < 0.5, "{lo:?} {hi:?}");
        let segs: std::collections::BTreeSet<u32> = t.tags.iter().map(|x| tag_segment(*x)).collect();
        assert_eq!(segs.into_iter().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        // Gaps: no vertex sits at the very top (0°), which falls in a gap.
        assert!(t.positions.iter().all(|p| !(p[0].abs() < 1.0 && p[1] < -150.0)));
        assert!(t.tags.iter().any(|x| tag_part(*x) == PART_CAPS));
        // Each tube cross-section's normals point away from the tube centre.
        for (p, n) in t.positions.iter().zip(&t.normals) {
            if t.tags.is_empty() {
                break;
            }
            let rr = (p[0] * p[0] + p[1] * p[1]).sqrt();
            let c = [p[0] / rr * 200.0, p[1] / rr * 200.0, 0.0];
            let d = [p[0] - c[0], p[1] - c[1], p[2]];
            let dd = d[0] * n[0] + d[1] * n[1] + d[2] * n[2];
            assert!(dd > -1e-3 || rr < 1.0);
        }
        // An arc from up to the right, clockwise: x ≥ 0, y ≤ 0 (plus the tube).
        let arc = generate(&params(MeshShape::Torus { radius: 200.0, tube: 20.0, angles: [0.0, 90.0], caps: Caps::Round })).unwrap();
        let (lo, hi) = arc.bounds();
        assert!(lo[0] > -21.0 && hi[1] < 21.0, "{lo:?} {hi:?}");
    }

    #[test]
    fn extrude_handles_holes_and_bevels() {
        // A square with a square hole.
        let m = generate(&params(MeshShape::Extrude { path: path("M -100 -100 L 100 -100 L 100 100 L -100 100 Z M -40 -40 L -40 40 L 40 40 L 40 -40 Z"), depth: 40.0, bevel: 6.0 })).unwrap();
        let front: Vec<_> = m.tris.iter().zip(&m.tags).filter(|(_, t)| tag_part(**t) == PART_FRONT).collect();
        // Front area = 200² − 80² minus the bevel inset.
        let area: f64 = front
            .iter()
            .map(|(t, _)| {
                let p = t.map(|i| m.positions[i as usize]);
                (((p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0])) as f64).abs() / 2.0
            })
            .sum();
        let expect = 188.0f64.powi(2) - 92.0f64.powi(2);
        assert!((area - expect).abs() < 1.0, "front area {area} vs {expect}");
        assert!(m.positions.iter().all(|p| (p[2].abs() - 20.0) < 1e-3));
        assert!(m.tags.iter().any(|t| tag_part(*t) == PART_BEVEL) && m.tags.iter().any(|t| tag_part(*t) == PART_SIDE));
        // Side normals point out of the material: away from the hole at the hole wall.
        for (p, n) in m.positions.iter().zip(&m.normals) {
            if n[2].abs() < 1e-3 && p[0].abs() < 41.0 && p[1].abs() < 41.0 {
                assert!(p[0] * n[0] + p[1] * n[1] < 0.0, "hole wall normal points into the hole");
            }
        }
        // A circle extrudes into a disc.
        let disc = generate(&params(MeshShape::Extrude { path: path("M 0 -50 A 50 50 0 1 1 0 50 A 50 50 0 1 1 0 -50 Z"), depth: 10.0, bevel: 0.0 })).unwrap();
        check_outward(&disc, [0.0; 3]);
    }

    #[test]
    fn revolve_and_sweep() {
        // A vase-like open profile turned about y: two-sided.
        let v = generate(&params(MeshShape::Revolve { profile: path("M 0 100 L 60 100 L 40 -100"), angles: [0.0, 360.0], axis_y: true })).unwrap();
        assert!(v.tags.iter().all(|t| t & TWO_SIDED != 0));
        let (lo, hi) = v.bounds();
        assert!((hi[0] - 60.0).abs() < 0.5 && (lo[1] + 100.0).abs() < 1e-3);
        // A closed profile (a circle at x = 150) about z, half a turn: capped.
        let r = generate(&params(MeshShape::Revolve { profile: path("M 150 -20 A 20 20 0 1 1 150 20 A 20 20 0 1 1 150 -20 Z"), angles: [0.0, 180.0], axis_y: false })).unwrap();
        assert!(r.tags.iter().any(|t| tag_part(*t) == PART_CAPS));
        // A tube drawn on halfway.
        let full = generate(&params(MeshShape::Sweep { path: path("M -200 0 L 200 0"), radius: 10.0, width: 0.0, profile: SweepProfile::Circle, trim: [0.0, 1.0], caps: Caps::Round, head: 0.0 })).unwrap();
        let half = generate(&params(MeshShape::Sweep { path: path("M -200 0 L 200 0"), radius: 10.0, width: 0.0, profile: SweepProfile::Circle, trim: [0.0, 0.5], caps: Caps::Round, head: 18.0 })).unwrap();
        let (lo, hi) = full.bounds();
        assert!((lo[0] + 210.0).abs() < 0.5 && (hi[0] - 210.0).abs() < 0.5);
        let (_, hi) = half.bounds();
        assert!((hi[0] - 18.0).abs() < 0.5, "head ball at the trim end: {hi:?}");
        check_outward(&generate(&params(MeshShape::Sweep { path: path("M -200 0 L 200 0"), radius: 10.0, width: 0.0, profile: SweepProfile::Circle, trim: [0.0, 1.0], caps: Caps::Flat, head: 0.0 })).unwrap(), [0.0; 3]);
    }

    #[test]
    fn triangulation_covers_the_area() {
        let outer: Vec<(f64, f64)> = (0..40).map(|i| {
            let a = TAU * i as f64 / 40.0;
            let r = if i % 2 == 0 { 100.0 } else { 60.0 };
            (r * a.cos(), r * a.sin())
        }).collect();
        let tris = triangulate(&[outer.clone()]);
        let area: f64 = tris.iter().map(|t| cross2(outer[t[0]], outer[t[1]], outer[t[2]]).abs() / 2.0).sum();
        assert!((area - signed_area(&outer).abs()).abs() < 1e-6);
        assert_eq!(tris.len(), 38);
    }

    #[test]
    fn local_bounds_contain_the_mesh() {
        let shapes = [
            MeshShape::Sphere { radius: 50.0 },
            MeshShape::Box { size: [100.0, 60.0, 40.0], bevel: 8.0 },
            MeshShape::Cylinder { radius: 40.0, top: 20.0, height: 90.0, bevel: 5.0 },
            MeshShape::Capsule { radius: 20.0, height: 120.0 },
            MeshShape::Torus { radius: 100.0, tube: 20.0, angles: [0.0, 200.0], caps: Caps::Round },
            MeshShape::Extrude { path: path("M -50 -40 L 60 -40 L 0 70 Z"), depth: 30.0, bevel: 4.0 },
            MeshShape::Revolve { profile: path("M 0 -50 L 40 0 L 0 50"), angles: [0.0, 360.0], axis_y: true },
            MeshShape::Sweep { path: path("M -100 0 C 0 -80 50 80 100 0"), radius: 12.0, width: 40.0, profile: SweepProfile::Circle, trim: [0.0, 1.0], caps: Caps::Round, head: 20.0 },
        ];
        for shape in shapes {
            let p = params(shape);
            let (lo, hi) = p.local_bounds().unwrap();
            let (mlo, mhi) = generate(&p).unwrap().bounds();
            for k in 0..3 {
                assert!(lo[k] <= mlo[k] as f64 + 1e-3 && hi[k] >= mhi[k] as f64 - 1e-3, "{:?}: {lo:?} {hi:?} vs {mlo:?} {mhi:?}", p.shape);
            }
        }
    }

    #[test]
    fn rings_tag_faces_and_size_pieces() {
        let t = generate(&MeshParams { shape: MeshShape::Torus { radius: 200.0, tube: 30.0, angles: [0.0, 360.0], caps: Caps::Flat }, segments: 2, gap: 0.0, detail: 1.0, spans: vec![1.0, 3.0], screen: 1.0, displace: None, deform: None }).unwrap();
        let parts: std::collections::BTreeSet<u32> = t.tags.iter().map(|x| tag_part(*x)).collect();
        assert!([PART_FRONT, PART_BACK, PART_SIDE, PART_INSIDE].iter().all(|p| parts.contains(p)), "{parts:?}");
        // Front faces face the camera (−z), inside faces the axis.
        for (tri, tag) in t.tris.iter().zip(&t.tags) {
            let n = tri.iter().fold([0.0f32; 3], |a, &i| { let v = t.normals[i as usize]; [a[0] + v[0], a[1] + v[1], a[2] + v[2]] });
            let p = t.positions[tri[0] as usize];
            match tag_part(*tag) {
                PART_FRONT => assert!(n[2] < 0.0),
                PART_BACK => assert!(n[2] > 0.0),
                PART_INSIDE => assert!(n[0] * p[0] + n[1] * p[1] < 0.0),
                _ => {}
            }
        }
        // Spans [1, 3]: piece 0 is a quarter turn, piece 1 three quarters.
        let angle = |seg: u32| {
            let pts: Vec<f64> = t.tris.iter().zip(&t.tags).filter(|(_, g)| tag_segment(**g) == seg).flat_map(|(tri, _)| tri.iter().map(|&i| {
                let p = t.positions[i as usize];
                (p[0] as f64).atan2(-(p[1] as f64)).to_degrees().rem_euclid(360.0)
            })).collect();
            pts.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - pts.iter().cloned().fold(f64::INFINITY, f64::min)
        };
        assert!((angle(0) - 90.0).abs() < 1.0, "{}", angle(0));
        assert!(angle(1) > 260.0);
    }

    #[test]
    fn wire_edges_skip_diagonals_and_fills() {
        for shape in [
            MeshShape::Box { size: [100.0, 100.0, 100.0], bevel: 0.0 },
            MeshShape::Extrude { path: path("M -50 -50 L 50 -50 L 50 50 L -50 50 Z"), depth: 20.0, bevel: 0.0 },
            MeshShape::Torus { radius: 100.0, tube: 20.0, angles: [0.0, 180.0], caps: Caps::Flat },
        ] {
            let m = generate(&params(shape.clone())).unwrap();
            assert_eq!(m.edges.len(), m.tris.len());
            // Real edges are never interior to a flat face: count edges on
            // the box/extrude front (z = −half): its 4 outline edges only.
            if !matches!(shape, MeshShape::Torus { .. }) {
                let mut front = 0;
                for ((t, tag), mask) in m.tris.iter().zip(&m.tags).zip(&m.edges) {
                    if tag_part(*tag) == PART_FRONT {
                        front += mask.count_ones();
                        let _ = t;
                    }
                }
                assert_eq!(front, 4, "{shape:?}");
            }
        }
    }

    #[test]
    fn keys_differ_by_parameters() {
        let a = params(MeshShape::Sphere { radius: 10.0 }).key();
        let b = params(MeshShape::Sphere { radius: 11.0 }).key();
        assert_ne!(a, b);
        assert_eq!(a, params(MeshShape::Sphere { radius: 10.0 }).key());
    }
}
