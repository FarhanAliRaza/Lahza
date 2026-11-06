//! Display list: a frame's layers as flat primitives binned into 16×16
//! tiles. The CPU and GPU rasterisers consume exactly this data, so both
//! paths draw the same picture.
//!
//! Every primitive carries the inverse transform from its target's pixels
//! to its own space (shape units, atlas texels or image texels) and the
//! size of one target pixel in that space for analytic anti-aliasing.
//! A blurred layer becomes a *group*: its primitives are drawn into a
//! downsampled offscreen target (large blurs run at 1/2…1/8 resolution, as
//! `blur_plane` does), blurred there, and composited as one image primitive.

use crate::font::{self, Fonts};
use crate::images::Images;
use bytemuck::{Pod, Zeroable};
use lahza_motion_core::path::{self, morph, trim_measured};
use lahza_motion_core::space::Proj;
use lahza_motion_core::compile::{Affine, Content, EffectState, FieldState, FrameState, LayerState, PaintState, PostKind, ShapeState, TextState};
use lahza_motion_core::document::{Blend, Gradient, GradientKind, GradientUnits, LineCap, LineJoin, MatteMode, ShapeKind, StrokeAlign, StrokeStyle};
use lahza_motion_core::value::Color;

pub const TILE: u32 = 16;

pub const PRIM_SHAPE: u32 = 0;
pub const PRIM_GLYPH: u32 = 1;
pub const PRIM_IMAGE: u32 = 2;
/// Image sources (kind.y of image primitives).
pub const SOURCE_POOL: u32 = 0;
pub const SOURCE_GROUP: u32 = 1;
pub const SOURCE_FOOTAGE: u32 = 2;
/// Group A masked by group B (same size): geom2 = [A offset, B offset, mode].
pub const PRIM_MATTE: u32 = 3;
/// Mesh batch (see `mesh3d`): geom = [bins base, first tile x, first tile
/// y, tiles across] (bits), geom2 = [triangles, vertices, materials,
/// lights] row offsets into `mesh` (bits).
pub const PRIM_MESH: u32 = 4;

pub const SHAPE_ELLIPSE: u32 = 0;
pub const SHAPE_RECT: u32 = 1;
pub const SHAPE_CAPSULE: u32 = 2;
pub const SHAPE_ARC: u32 = 3;
pub const SHAPE_LINE: u32 = 4;
pub const SHAPE_STAR: u32 = 5;
/// Flattened Bézier path: geom2 = [first segment, count, open (1) / filled (0)].
pub const SHAPE_PATH: u32 = 6;

/// Paint kinds (paint.y): where a fill's colour comes from.
pub const PAINT_SOLID: u32 = 0;
/// paint2 = [from, to].
pub const PAINT_LINEAR: u32 = 1;
/// paint2 = [centre, edge point].
pub const PAINT_RADIAL: u32 = 2;
/// Axis-aligned ellipse: paint2 = [centre, radii].
pub const PAINT_ELLIPSE: u32 = 3;
/// Along a path: paint2 = [first centreline segment, count] (bits); the
/// count segments after them hold each one's [u0, u1] (fractions of the
/// full path's length).
pub const PAINT_ALONG: u32 = 4;
/// Field: per point a stop pair [x, y, 1/r², 0] + premultiplied colour;
/// paint2 = [warp px, warp scale px, falloff power, 0].
pub const PAINT_FIELD: u32 = 5;

pub const FLAG_FILL: u32 = 1;
pub const FLAG_STROKE: u32 = 2;
/// Image: colour = fill × sample alpha (shadows).
pub const FLAG_TINT_ALPHA: u32 = 4;
/// Image: colour = fill × sample (glows).
pub const FLAG_TINT_MUL: u32 = 8;
/// Glyph: rim light over the fill (colour in `stroke`).
pub const FLAG_EDGE: u32 = 16;
/// Glyph wipe masks (kind.y of glyph primitives): none, linear, |linear|, radial.
pub const WIPE_LINEAR: u32 = 1;
pub const WIPE_CENTER: u32 = 2;
pub const WIPE_RADIAL: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Prim {
    /// x: primitive kind, y: shape kind / image source (0 pool, 1 groups),
    /// z: blend mode, w: flags.
    pub kind: [u32; 4],
    /// local.x = x·px + y·py + z; w = local units per target pixel.
    pub inv_x: [f32; 4],
    /// local.y = x·px + y·py + z; w = opacity.
    pub inv_y: [f32; 4],
    /// Projective row: local = (inv_x·p, inv_y·p) / (inv_w·p); w = det of
    /// the inverse (per-pixel anti-aliasing width √|det / w³|).
    pub inv_w: [f32; 4],
    pub geom: [f32; 4],
    pub geom2: [f32; 4],
    /// Premultiplied fill colour.
    pub fill: [f32; 4],
    /// Premultiplied stroke colour.
    pub stroke: [f32; 4],
    /// x: stroke width, y: paint kind (`PAINT_*`), z: first stop, w: stop
    /// count.
    pub paint: [f32; 4],
    /// Paint geometry in local units (see `PAINT_*`).
    pub paint2: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    /// First tile range (pairs in `ranges`).
    pub range_base: u32,
    /// Premultiplied.
    pub background: [f32; 4],
}

impl Target {
    fn new(width: u32, height: u32, range_base: u32, background: [f32; 4]) -> Self {
        Self { width, height, tiles_x: width.div_ceil(TILE), tiles_y: height.div_ceil(TILE), range_base, background }
    }
    pub fn tiles(&self) -> u32 {
        self.tiles_x * self.tiles_y
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Group {
    pub target: Target,
    /// Offset into the group texel buffer.
    pub offset: u32,
    /// Gaussian sigma in group pixels.
    pub sigma: f32,
    /// Blur clamps to edge pixels instead of transparent.
    pub clamp: bool,
    /// Traced instead of rasterised: index into `DisplayList::traces`.
    pub trace: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayList {
    pub main: Option<Target>,
    pub groups: Vec<Group>,
    pub prims: Vec<Prim>,
    /// Per tile: [first index, count].
    pub ranges: Vec<u32>,
    pub indices: Vec<u32>,
    /// Gradient stops: pairs of [offset, 0, 0, 0], premultiplied colour.
    pub stops: Vec<[f32; 4]>,
    /// Texels (vec4 f32) all groups need.
    pub group_texels: u32,
    /// Path segments [x0, y0, x1, y1] in each path's own space.
    pub segments: Vec<[f32; 4]>,
    /// Post programs of composited groups (see `raster::OP_*`).
    pub ops: Vec<[f32; 4]>,
    /// Mesh batches: triangles, vertices, materials and lights.
    pub mesh: Vec<[f32; 4]>,
    /// Mesh tile bins: per tile [start (relative to the batch's base),
    /// count], then triangle indices.
    pub bins: Vec<u32>,
    /// Layer lists the path tracer renders (export), by group.
    pub traces: Vec<crate::pt::Trace>,
}

impl DisplayList {
    pub fn main(&self) -> Target {
        self.main.expect("display list built")
    }
    /// Bytes that fully determine the picture (frame cache key).
    pub fn signature(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.prims.len() * 144 + 64);
        let m = self.main();
        out.extend_from_slice(bytemuck::cast_slice(&[m.width, m.height]));
        out.extend_from_slice(bytemuck::cast_slice(&m.background));
        for g in &self.groups {
            out.extend_from_slice(bytemuck::cast_slice(&[g.target.width, g.target.height, g.offset, g.sigma.to_bits()]));
        }
        out.extend_from_slice(bytemuck::cast_slice(&self.prims));
        out.extend_from_slice(bytemuck::cast_slice(&self.stops));
        out.extend_from_slice(bytemuck::cast_slice(&self.segments));
        out.extend_from_slice(bytemuck::cast_slice(&self.ops));
        out.extend_from_slice(bytemuck::cast_slice(&self.mesh));
        out.extend_from_slice(bytemuck::cast_slice(&self.bins));
        for t in &self.traces {
            t.signature(&mut out);
        }
        out
    }
}

fn premul(c: Color, opacity: f64) -> [f32; 4] {
    let a = (c.0[3] * opacity) as f32;
    [c.0[0] as f32 * a, c.0[1] as f32 * a, c.0[2] as f32 * a, a]
}

/// Text styling in atlas texels (the distance field's units). Glyph prims
/// use paint = [weight, stroke band inner, outer, edge depth] (paint.y stays
/// ≤ 0: the GPU merge treats paint.y > 0.5 as a gradient), geom2 = [wipe
/// opacity from, to, light direction] and paint2 = wipe parameters.
struct GlyphStyle {
    weight: f32,
    /// Outline band [inner, outer] around the weighted edge, and colour.
    stroke: Option<([f32; 2], [f32; 4])>,
    /// Rim colour, depth and light direction (layer space, towards the light).
    edge: Option<([f32; 4], f32, [f64; 2])>,
}

impl GlyphStyle {
    fn new(text: &TextState) -> Self {
        let texels = font::ATLAS_EM as f64 / text.size;
        // The atlas stores distances up to SPREAD texels from the outline.
        let reach = (font::SPREAD - 1.0) as f64;
        let weight = (text.weight * texels).clamp(-reach, reach);
        let stroke = text.stroke.and_then(|(c, w, align)| {
            let w = w * texels;
            let (lo, hi) = match align {
                StrokeAlign::Outside => (0.0, w),
                StrokeAlign::Center => (-w / 2.0, w / 2.0),
                StrokeAlign::Inside => (-w, 0.0),
            };
            let (lo, hi) = (lo.max(-reach - weight), hi.min(reach - weight));
            (hi > lo && c.0[3] > 0.0).then(|| ([lo as f32, hi as f32], premul(c, 1.0)))
        });
        let edge = text.edge.and_then(|(c, w, angle)| {
            let depth = (w * texels).min(reach + weight);
            let (s, co) = angle.to_radians().sin_cos();
            (depth > 1e-3 && c.0[3] > 0.0).then(|| (premul(c, 1.0), depth as f32, [s, -co]))
        });
        Self { weight: weight as f32, stroke, edge }
    }
}

fn blend_code(b: Blend) -> u32 {
    b.code()
}

fn matte_code(mode: MatteMode) -> f32 {
    match mode {
        MatteMode::Alpha => 0.0,
        MatteMode::AlphaInverted => 1.0,
        MatteMode::Luma => 2.0,
        MatteMode::LumaInverted => 3.0,
    }
}

#[derive(Clone)]
struct Item {
    prim: Prim,
    /// Forward transform (local → canvas) and local bounds, for binning.
    forward: Proj,
    bounds: [f64; 4],
}

fn with_inverse(mut prim: Prim, forward: impl Into<Proj>, opacity: f64) -> Option<(Prim, Proj)> {
    let forward: Proj = forward.into();
    let mut inv = forward.inverse()?;
    // Normalise so |w| ≈ 1 near the layer (keeps f32 well conditioned);
    // by the magnitude only, since w's sign marks points behind the camera.
    let k = inv.0[8].abs();
    if k > 1e-12 && !inv.is_affine() {
        inv = Proj(inv.0.map(|v| v / k));
    }
    let m = inv.0;
    let det = inv.determinant();
    // Nominal pixel size in local units (exact for affine layers; at the
    // local origin for perspective ones, which shade it per pixel).
    let aa = if inv.is_affine() { det.abs().sqrt() } else { 1.0 / forward.local_scale(0.0, 0.0).max(1e-12) }.max(1e-9);
    prim.inv_x = [m[0] as f32, m[1] as f32, m[2] as f32, aa as f32];
    prim.inv_y = [m[3] as f32, m[4] as f32, m[5] as f32, opacity as f32];
    prim.inv_w = [m[6] as f32, m[7] as f32, m[8] as f32, det as f32];
    Some((prim, inv))
}

/// Rebinds an item to a target whose pixel (gx, gy) is canvas
/// `origin + factor·(gx, gy)`.
fn retarget(item: &Item, origin: [f64; 2], factor: [f64; 2]) -> Option<Prim> {
    let to_canvas = Affine::translate(origin[0], origin[1]).then(Affine::scale(factor[0], factor[1]));
    let forward = Proj::from(to_canvas.inverse()?).then(item.forward);
    let (p, _) = with_inverse(item.prim, forward, item.prim.inv_y[3] as f64)?;
    Some(p)
}

struct Pending {
    group: Group,
    items: Vec<Item>,
    origin: [f64; 2],
    factor: [f64; 2],
}

pub struct Builder<'a> {
    pub fonts: &'a mut Fonts,
    pub images: &'a mut Images,
    pub footage: &'a mut crate::footage::Footage,
    pub meshes: &'a mut crate::mesh3d::MeshCache,
    /// Output scale (0.5 for half-resolution previews).
    pub scale: f64,
    /// Path-trace layer lists with meshes instead of rasterising them.
    pub trace: Option<crate::pt::TraceOptions>,
    /// Seeds the tracer's samples (the frame's time), so a frame renders
    /// the same every time.
    pub seed: u32,
}

impl Builder<'_> {
    pub fn build(&mut self, frame: &FrameState) -> DisplayList {
        let w = ((frame.width as f64 * self.scale).round() as u32).max(1);
        let h = ((frame.height as f64 * self.scale).round() as u32).max(1);
        let root = Proj::from(Affine::scale(self.scale, self.scale));
        let mut list = DisplayList::default();
        self.meshes.begin();
        let mut pending: Vec<Pending> = Vec::new();
        let mut main_items: Vec<Item> = Vec::new();
        self.emit_list(&frame.layers, root, (w, h), &mut list, &mut pending, &mut main_items);
        let background = premul(frame.background, 1.0);
        // Groups first (inner before outer), then the main target.
        let mut range_base = 0;
        for Pending { mut group, items, origin, factor } in pending {
            group.target.range_base = range_base;
            let prims: Vec<(Prim, [f64; 4])> = items
                .iter()
                .filter_map(|it| {
                    if it.prim.kind[0] == PRIM_MESH {
                        return self.place_mesh(&mut list, it, &group.target, origin, factor);
                    }
                    let p = retarget(it, origin, factor)?;
                    let b = canvas_bounds(it);
                    Some((p, [(b[0] - origin[0]) / factor[0], (b[1] - origin[1]) / factor[1], (b[2] - origin[0]) / factor[0], (b[3] - origin[1]) / factor[1]]))
                })
                .collect();
            bin(&mut list, &group.target, prims);
            range_base += group.target.tiles();
            list.groups.push(group);
        }
        let main = Target::new(w, h, range_base, background);
        let prims: Vec<(Prim, [f64; 4])> = main_items
            .iter()
            .filter_map(|it| if it.prim.kind[0] == PRIM_MESH { self.place_mesh(&mut list, it, &main, [0.0, 0.0], [1.0, 1.0]) } else { Some((it.prim, canvas_bounds(it))) })
            .collect();
        bin(&mut list, &main, prims);
        list.main = Some(main);
        list
    }

    /// Reserves an offscreen group over canvas rect `rect` for `items`,
    /// blurred by `sigma` canvas pixels (drawn at reduced resolution for
    /// large blurs). Returns (group offset, origin, factor, size).
    /// The texel grid fits `rect` exactly (per-axis factor ≈ `factor`), so
    /// edge texels are fully covered.
    #[allow(clippy::too_many_arguments)]
    fn reserve(&self, list: &mut DisplayList, pending: &mut Vec<Pending>, items: Vec<Item>, rect: [f64; 4], sigma: f64, factor: f64, clamp: bool) -> (u32, [f64; 2], [f64; 2], (u32, u32)) {
        let (w, h) = ((rect[2] - rect[0]).max(1.0), (rect[3] - rect[1]).max(1.0));
        let gw = (w / factor).ceil().max(1.0) as u32;
        let gh = (h / factor).ceil().max(1.0) as u32;
        let f = [w / gw as f64, h / gh as f64];
        let group = Group { target: Target::new(gw, gh, 0, [0.0; 4]), offset: list.group_texels, sigma: (sigma / ((f[0] + f[1]) / 2.0)) as f32, clamp, trace: None };
        list.group_texels += gw * gh;
        let origin = [rect[0], rect[1]];
        pending.push(Pending { group, items, origin, factor: f });
        (group.offset, origin, f, (gw, gh))
    }

    /// Encodes post effects for a group whose texel is `f` canvas px.
    fn program(&mut self, effects: &[EffectState], f: [f64; 2], list: &mut DisplayList) -> (u32, u32) {
        use crate::raster::*;
        let start = list.ops.len();
        let k = (self.scale / ((f[0] + f[1]) / 2.0)) as f32; // canvas px → texels
        let col = |c: Color| [c.0[0] as f32, c.0[1] as f32, c.0[2] as f32, c.0[3] as f32];
        for e in effects {
            let EffectState::Post(p) = e else { continue };
            let v = p.v.map(|x| x as f32);
            let mut body: Vec<[f32; 4]> = Vec::new();
            let head: [f32; 4] = match p.kind {
                PostKind::Exposure => [OP_EXPOSURE, 0.0, v[0], 0.0],
                PostKind::BrightnessContrast => [OP_BRIGHTNESS_CONTRAST, 0.0, v[0], v[1]],
                PostKind::Levels => {
                    body.push([v[2], v[3], v[4], 0.0]);
                    [OP_LEVELS, 0.0, v[0], v[1]]
                }
                PostKind::HueSaturation => {
                    body.push([v[2], 0.0, 0.0, 0.0]);
                    [OP_HUE_SATURATION, 0.0, v[0], v[1]]
                }
                PostKind::Tint => {
                    body.push(col(p.colors[0]));
                    body.push(col(p.colors[1]));
                    [OP_TINT, 0.0, v[0], 0.0]
                }
                PostKind::GradientMap => {
                    let stops = p.stops.as_deref().map(|s| s.as_slice()).unwrap_or(&[]);
                    for (o, c) in stops {
                        body.push([*o as f32, 0.0, 0.0, 0.0]);
                        body.push(col(*c));
                    }
                    [OP_GRADIENT_MAP, 0.0, v[0], stops.len() as f32]
                }
                PostKind::Invert => [OP_INVERT, 0.0, v[0], 0.0],
                PostKind::Fill => {
                    body.push(col(p.colors[0]));
                    [OP_FILL, 0.0, v[0], 0.0]
                }
                PostKind::Lut => {
                    let Some(lut) = p.path.as_deref().and_then(|path| self.images.lut(path)) else { continue };
                    body.extend_from_slice(&lut.1);
                    [OP_LUT, 0.0, v[0], lut.0 as f32]
                }
                PostKind::Displace => {
                    body.push([v[2], p.seed as f32, 0.0, 0.0]);
                    [OP_DISPLACE, 0.0, v[0] * k, (v[1] * k).max(1e-3)]
                }
                PostKind::DirectionalBlur => [OP_DIRECTIONAL_BLUR, 0.0, v[0] * k, v[1] * k],
                PostKind::RgbSplit => [OP_RGB_SPLIT, 0.0, v[0] * k, v[1] * k],
                PostKind::Vignette => [OP_VIGNETTE, 0.0, v[0], v[1].clamp(0.0, 1.0)],
                PostKind::Grain => {
                    body.push([(p.seed % 16_777_216) as f32, 0.0, 0.0, 0.0]);
                    [OP_GRAIN, 0.0, v[0], (v[1] * k).max(1e-3)]
                }
            };
            let mut head = head;
            head[1] = (1 + body.len()) as f32;
            list.ops.push(head);
            list.ops.extend(body);
        }
        (start as u32, (list.ops.len() - start) as u32)
    }

    fn place_mesh(&mut self, list: &mut DisplayList, item: &Item, target: &Target, origin: [f64; 2], factor: [f64; 2]) -> Option<(Prim, [f64; 4])> {
        let cache = &mut *self.meshes;
        let batch = cache.batches.get(item.prim.kind[1] as usize)?;
        batch.place(&mut list.mesh, &mut list.bins, &mut cache.placed, target, origin, factor, item.prim.kind[2])
    }

    /// A mesh item: a new batch, or `m` added to the batch of the item just
    /// before it (meshes next to each other share one depth buffer).
    /// Loads a mesh's image maps into the pool (once) for its material rows.
    fn mesh_textures(&mut self, m: &lahza_motion_core::compile::MeshState) {
        for mat in &m.materials {
            for path in [&mat.map, &mat.normal_map, &mat.bump_map].into_iter().flatten() {
                if !self.meshes.textures.contains_key(path) {
                    let t = self.images.get(path);
                    self.meshes.textures.insert(path.clone(), t);
                }
            }
        }
    }

    fn mesh_item(&mut self, layer: &LayerState, m: &lahza_motion_core::compile::MeshState, root: Proj, prev: Option<&mut Item>) -> Option<Item> {
        self.mesh_textures(m);
        let blend = blend_code(layer.blend);
        let see_through = crate::mesh3d::see_through(layer);
        let (list_serial, index) = self.meshes.member();
        if let (Some(prev), false) = (prev, see_through) {
            let i = prev.prim.kind[1] as usize;
            if prev.prim.kind[0] == PRIM_MESH && prev.prim.kind[2] == blend && blend == blend_code(Blend::Normal) && self.meshes.batches[i].can_join(m) && self.meshes.batches[i].list == list_serial {
                let mut batch = std::mem::replace(&mut self.meshes.batches[i], crate::mesh3d::Batch::new(m)?);
                batch.add(self.meshes, m, root, layer.opacity);
                batch.members.push(index);
                prev.bounds = batch.bounds;
                self.meshes.batches[i] = batch;
                return None;
            }
        }
        let mut batch = crate::mesh3d::Batch::new(m)?;
        batch.list = list_serial;
        batch.members.push(index);
        batch.see_through = see_through;
        batch.add(self.meshes, m, root, layer.opacity);
        let bounds = batch.bounds;
        if !(bounds[2] >= bounds[0]) {
            return None;
        }
        self.meshes.batches.push(batch);
        let prim = Prim { kind: [PRIM_MESH, (self.meshes.batches.len() - 1) as u32, blend, 0], ..Default::default() };
        Some(Item { prim, forward: Proj::IDENTITY, bounds })
    }

    /// Emits a layer list in order. Meshes that let what is behind them
    /// show (opacity, glass, blend modes) are drawn after the opaque meshes
    /// next to them, and every mesh batch hides where the list's other
    /// opaque meshes are nearer (shared depth across effects, blends and
    /// depth of field).
    fn emit_list(&mut self, layers: &[LayerState], root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, out: &mut Vec<Item>) {
        if let Some(options) = self.trace {
            if layers.iter().any(|l| matches!(l.content, Content::Mesh(_))) {
                match untraceable(layers) {
                    None => {
                        self.trace_list(layers, root, canvas, list, pending, out, options);
                        return;
                    }
                    Some((id, why)) => {
                        // Say so: a silently rasterised list looks traced in the report.
                        let w = format!("path tracing: the layer list with '{id}' was rasterised ({why})");
                        if !self.meshes.warnings.contains(&w) {
                            self.meshes.warnings.push(w);
                        }
                    }
                }
            }
        }
        self.meshes.open_run(layers, root);
        // With meshes in the list, flat 3D layers join their depth buffer;
        // so do tilted 3D layers that could cut through each other.
        let cards = layers.iter().filter(|l| card_candidate(l)).count();
        let has_mesh = layers.iter().any(|l| matches!(l.content, Content::Mesh(_)))
            || cards >= 2 && layers.iter().any(|l| card_candidate(l) && !l.matrix.is_affine());
        // Cards (flat 3D layers among meshes) are drawn after the opaque
        // meshes next to them and blend by their picture's alpha, so their
        // soft and anti-aliased parts show the mesh behind them, not the
        // background; a nearer opaque mesh hides them. See-through meshes
        // come last (glass refracts the cards too).
        let mut cards: Vec<usize> = Vec::new();
        let mut deferred: Vec<usize> = Vec::new();
        for (i, layer) in layers.iter().enumerate() {
            if crate::mesh3d::see_through(layer) {
                deferred.push(i);
                continue;
            }
            if has_mesh && card_candidate(layer) {
                cards.push(i);
                continue;
            }
            if !matches!(layer.content, Content::Mesh(_)) {
                self.flush_deferred(layers, &mut cards, &mut deferred, root, canvas, list, pending, out);
            }
            self.meshes.set_current(i);
            self.emit(layer, root, canvas, list, pending, out);
        }
        self.flush_deferred(layers, &mut cards, &mut deferred, root, canvas, list, pending, out);
        self.meshes.close_run();
    }

    /// Path tracing (export): the list's meshes and flat 3D layers become
    /// one traced picture, composited where the first of them is; its 2D
    /// layers draw around it as usual.
    #[allow(clippy::too_many_arguments)]
    fn trace_list(&mut self, layers: &[LayerState], root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, out: &mut Vec<Item>, options: crate::pt::TraceOptions) {
        let mut at: Option<usize> = None;
        let mut instances = Vec::new();
        let mut cards = Vec::new();
        let mut first: Option<lahza_motion_core::compile::MeshState> = None;
        for layer in layers {
            let card = trace_card_candidate(layer);
            if !matches!(layer.content, Content::Mesh(_)) && !card {
                self.emit(layer, root, canvas, list, pending, out);
                continue;
            }
            at.get_or_insert(out.len());
            if let Content::Mesh(m) = &layer.content {
                self.mesh_textures(m);
                if let Some(i) = self.meshes.trace_instance(layer.id.clone(), m, layer.opacity, root.max_scale()) {
                    instances.push(i);
                }
                first.get_or_insert_with(|| (**m).clone());
                continue;
            }
            match self.card_picture(layer, root, canvas, list, pending, false) {
                Ok(Some((view, _, picture, _))) => cards.push(crate::pt::TraceCard { view: view.map(|v| v.map(|x| x as f32)), picture }),
                Ok(None) => {}
                Err(()) => self.emit(layer, root, canvas, list, pending, out),
            }
        }
        let (Some(at), Some(m)) = (at, first) else { return };
        let mut lights = m.lighting.as_ref().map(|l| crate::mesh3d::light_rows(l, &m)).unwrap_or_default();
        let env = m.lighting.as_ref().and_then(|l| self.meshes.env_map(l));
        if env.is_none() && !lights.is_empty() {
            // No image (or it did not load): the environment is dark.
            lights[0][2] = 0.0;
        }
        if let Some(l) = &m.lighting {
            // Which lights cast shadows (the raster links its maps here).
            for (i, light) in l.lights.iter().enumerate() {
                lights[7 + i * crate::mesh3d::LROWS + 1][3] = if light.shadow { 1.0 } else { 0.0 };
            }
        }
        let inv = root.inverse().unwrap_or(root);
        let rect = [0.0, 0.0, canvas.0 as f64, canvas.1 as f64];
        let (offset, origin, factor, (gw, gh)) = self.reserve(list, pending, Vec::new(), rect, 0.0, 1.0, false);
        if let Some(p) = pending.last_mut() {
            p.group.trace = Some(list.traces.len() as u32);
        }
        let f = |v: [f64; 9]| v.map(|x| x as f32);
        list.traces.push(crate::pt::Trace {
            instances,
            cards,
            lights,
            focal: m.camera.focal as f32,
            centre: [m.camera.centre.0 as f32, m.camera.centre.1 as f32],
            root: f(root.0),
            inv_root: f(inv.0),
            origin: [origin[0] as f32, origin[1] as f32],
            factor: [factor[0] as f32, factor[1] as f32],
            options,
            seed: self.seed,
            env,
            // Thin lens matching the raster's depth of field: its blur
            // σ = per_px · |z − focus| is a circle of confusion of radius
            // ≈ 2σ near the focus plane.
            lens: m.lens.filter(|l| l[0] > 1.0 && l[1] > 0.0).map(|[focus, per_px]| [(2.0 * per_px * focus * focus / m.camera.focal) as f32, focus as f32]).unwrap_or([0.0; 2]),
        });
        let forward = Proj::from(Affine::translate(origin[0], origin[1]).then(Affine::scale(factor[0], factor[1])));
        let prim = Prim { kind: [PRIM_IMAGE, 1, blend_code(Blend::Normal), 0], geom: [gw as f32, gh as f32, 1.0, 0.0], geom2: [f32::from_bits(offset), 0.0, 0.0, 0.0], ..Default::default() };
        if let Some((prim, _)) = with_inverse(prim, forward, 1.0) {
            out.insert(at, Item { prim, forward, bounds: [0.0, 0.0, gw as f64, gh as f64] });
        }
    }

    /// Draws the cards and see-through meshes held back by `emit_list`.
    #[allow(clippy::too_many_arguments)]
    fn flush_deferred(&mut self, layers: &[LayerState], cards: &mut Vec<usize>, deferred: &mut Vec<usize>, root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, out: &mut Vec<Item>) {
        for j in cards.drain(..) {
            self.meshes.set_current(j);
            if !self.card_item(&layers[j], root, canvas, list, pending, out) {
                self.emit(&layers[j], root, canvas, list, pending, out);
            }
        }
        for j in deferred.drain(..) {
            self.meshes.set_current(j);
            self.emit(&layers[j], root, canvas, list, pending, out);
        }
    }

    /// Draws a flat 3D layer as a card in the mesh depth buffer: its
    /// picture into a group, its quad into the current batch. False when it
    /// can't be (it crosses the camera): it is then drawn as usual.
    /// A flat 3D layer's picture (an offscreen group, blurred by its own
    /// blur when `blurred`) and the quad it lies on in camera space.
    /// Err: it can't be a card (it crosses the camera); Ok(None): nothing
    /// to draw.
    #[allow(clippy::type_complexity)]
    fn card_picture(&mut self, layer: &LayerState, root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, blurred: bool) -> Result<Option<([[f64; 3]; 4], [[f64; 2]; 4], crate::mesh3d::Backdrop, [f64; 4])>, ()> {
        let Some(view) = layer.view else { return Err(()) };
        let Some(inverse) = root.then(layer.matrix).inverse() else { return Err(()) };
        let bare = LayerState { effects: Vec::new(), ..layer.clone() };
        let items = self.layer_items(&bare, root, &mut list.stops, &mut list.segments);
        let blur = if blurred { card_blur(layer, self.scale) } else { 0.0 };
        let pad = if blur > 0.3 { 3.0 * blur + 2.0 } else { 0.0 };
        if items.is_empty() {
            return Ok(None);
        }
        let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for it in &items {
            let b = canvas_bounds(it);
            bb = [bb[0].min(b[0]), bb[1].min(b[1]), bb[2].max(b[2]), bb[3].max(b[3])];
        }
        let (cw, ch) = (canvas.0 as f64, canvas.1 as f64);
        // A blurred picture reaches past the canvas by the blur's reach, so
        // content just outside the frame blurs in instead of fading the
        // edges to transparent.
        let rect = [(bb[0] - pad).floor().max(-pad), (bb[1] - pad).floor().max(-pad), (bb[2] + pad).ceil().min(cw + pad), (bb[3] + pad).ceil().min(ch + pad)];
        if !rect.iter().all(|v| v.is_finite()) || rect[2] <= rect[0] || rect[3] <= rect[1] {
            return if bb.iter().all(|v| v.is_finite()) { Ok(None) } else { Err(()) };
        }
        // The quad: the picture's rect, found back on the layer's plane.
        let screen = [[rect[0], rect[1]], [rect[2], rect[1]], [rect[2], rect[3]], [rect[0], rect[3]]];
        let mut corners = [[0.0; 3]; 4];
        for (k, s) in screen.iter().enumerate() {
            let (x, y) = inverse.apply(s[0], s[1]);
            let q = view.apply(x, y, 0.0);
            if !(x.is_finite() && y.is_finite()) || q[2] < 1.0 {
                return Err(());
            }
            corners[k] = [q[0], q[1], q[2]];
        }
        let (sigma, factor) = if blur > 0.3 { (blur, if blur <= 4.0 { 1.0 } else { 2f64.powf((blur / 4.0).log2().ceil()).min(16.0) }) } else { (0.0, 1.0) };
        let (offset, origin, factor, size) = self.reserve(list, pending, items, rect, sigma, factor, false);
        Ok(Some((corners, screen, crate::mesh3d::Backdrop { offset, size, origin, factor }, rect)))
    }

    fn card_item(&mut self, layer: &LayerState, root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, out: &mut Vec<Item>) -> bool {
        let (corners, screen, picture, rect) = match self.card_picture(layer, root, canvas, list, pending, true) {
            Err(()) => return false,
            Ok(None) => return true,
            Ok(Some(c)) => c,
        };
        let (serial, index) = self.meshes.member();
        let card = crate::mesh3d::Card { index, view: corners, screen, picture, bounds: rect };
        self.meshes.add_card(card.clone());

        // Cards share a batch with each other (they cut through each
        // other), never with opaque meshes (drawn before them).
        if let Some(prev) = out.last_mut().filter(|p| p.prim.kind[0] == PRIM_MESH) {
            let i = prev.prim.kind[1] as usize;
            if prev.prim.kind[2] == blend_code(Blend::Normal) && self.meshes.batches[i].list == serial && self.meshes.batches[i].cards_only {
                let batch = &mut self.meshes.batches[i];
                batch.add_card(&card, false);
                batch.members.push(index);
                prev.bounds = batch.bounds;
                return true;
            }
        }
        let mut batch = crate::mesh3d::Batch::cards_only();
        batch.list = serial;
        batch.members.push(index);
        batch.add_card(&card, false);
        let bounds = batch.bounds;
        self.meshes.batches.push(batch);
        let prim = Prim { kind: [PRIM_MESH, (self.meshes.batches.len() - 1) as u32, blend_code(Blend::Normal), 0], ..Default::default() };
        out.push(Item { prim, forward: Proj::IDENTITY, bounds });
        true
    }

    /// Glass refracts what is drawn before it: those items, drawn again
    /// into a group around the glass (blurred by its roughness: frosted).
    fn glass_backdrop(&mut self, m: &lahza_motion_core::compile::MeshState, item: &Item, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, out: &[Item]) {
        let glass = m.materials.iter().filter(|x| x.transmission > 0.0);
        let (mut reach, mut rough) = (0.0f64, 0.0f64);
        for x in glass {
            reach = reach.max(x.thickness);
            rough = rough.max(x.roughness);
        }
        let sigma = rough * rough * 60.0 * self.scale;
        let pad = reach * self.scale + 3.0 * sigma + 8.0;
        let b = item.bounds;
        let (cw, ch) = (canvas.0 as f64, canvas.1 as f64);
        let rect = [(b[0] - pad).floor().max(0.0), (b[1] - pad).floor().max(0.0), (b[2] + pad).ceil().min(cw), (b[3] + pad).ceil().min(ch)];
        if rect[2] <= rect[0] || rect[3] <= rect[1] {
            return;
        }
        let factor = if sigma <= 4.0 { 1.0 } else { 2f64.powf((sigma / 4.0).log2().ceil()).min(16.0) };
        let (offset, origin, f, size) = self.reserve(list, pending, out.to_vec(), rect, if sigma > 0.3 { sigma } else { 0.0 }, factor, false);
        let i = item.prim.kind[1] as usize;
        self.meshes.batches[i].backdrop = Some(crate::mesh3d::Backdrop { offset, size, origin, factor: f });
    }

    /// A plain group drawn straight into its parent, without an offscreen
    /// picture, when that gives exactly the same pixels: no effects, matte
    /// or blend mode on the group or (for its children's blending) on any
    /// child, nothing 3D, and either full opacity or children that do not
    /// overlap (a faded grid of dots: each dot takes the group's opacity).
    /// Rendering a group offscreen costs a raster of its whole area plus a
    /// resampled composite, so this is the common case worth skipping.
    fn flat_group(&mut self, layer: &LayerState, children: &[LayerState], root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>) -> Option<Vec<Item>> {
        fn has_3d(layers: &[LayerState]) -> bool {
            layers.iter().any(|l| {
                l.view.is_some() || !l.matrix.is_affine() || matches!(l.content, Content::Mesh(_)) || matches!(&l.content, Content::Group(c) if has_3d(c))
            })
        }
        if !layer.effects.is_empty() || layer.matte.is_some() || layer.blend != Blend::Normal || !layer.matrix.is_affine() || layer.view.is_some() {
            return None;
        }
        if children.iter().any(|c| c.blend != Blend::Normal) || has_3d(children) {
            return None;
        }
        let inner = root.then(layer.matrix);
        if layer.opacity >= 0.999 {
            let mut items = Vec::new();
            self.emit_list(children, inner, canvas, list, pending, &mut items);
            return Some(items);
        }
        if layer.opacity <= 0.0 {
            return Some(Vec::new());
        }
        if children.iter().any(LayerState::is_isolated) {
            return None;
        }
        let mut items = Vec::new();
        let mut boxes: Vec<[f64; 4]> = Vec::with_capacity(children.len());
        for child in children {
            let faded = LayerState { opacity: child.opacity * layer.opacity, ..child.clone() };
            let its = self.layer_items(&faded, inner, &mut list.stops, &mut list.segments);
            let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for it in &its {
                let c = canvas_bounds(it);
                b = [b[0].min(c[0]), b[1].min(c[1]), b[2].max(c[2]), b[3].max(c[3])];
            }
            // One child drawn as several overlapping items (glyphs, path
            // pieces) would double-fade where they overlap.
            if its.len() > 1 {
                return None;
            }
            if b[0] < b[2] {
                boxes.push(b);
            }
            items.extend(its);
        }
        // Sweep in x: any two boxes overlapping (anti-aliased edges
        // included) need the offscreen group.
        boxes.sort_by(|a, b| a[0].total_cmp(&b[0]));
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                if boxes[j][0] >= boxes[i][2] {
                    break;
                }
                if boxes[j][1] < boxes[i][3] && boxes[i][1] < boxes[j][3] {
                    return None;
                }
            }
        }
        Some(items)
    }

    /// A gradient field is smooth (its features are its point radii and
    /// warp scale, hundreds of px), yet costs a sum over its points and a
    /// noise lookup per pixel. Drawn into a group at up to ¼ resolution
    /// and stretched back (bilinear, edges clamped), it costs ~1/16 as much
    /// and looks the same. Only for plain, axis-aligned rectangle fields.
    fn field_proxy(&mut self, layer: &LayerState, root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>) -> Option<Item> {
        let Content::Shape(s) = &layer.content else { return None };
        let field = s.field.as_ref()?;
        if s.shape != ShapeKind::Field || field.points.is_empty() || s.stroke.is_some() || s.radius > 0.0 || layer.blend != Blend::Normal {
            return None;
        }
        let m = root.then(layer.matrix);
        if !m.is_affine() {
            return None;
        }
        let o = m.apply(0.0, 0.0);
        let (ax, ay) = (m.apply(1.0, 0.0), m.apply(0.0, 1.0));
        let (ux, uy) = ((ax.0 - o.0, ax.1 - o.1), (ay.0 - o.0, ay.1 - o.1));
        if ux.1.abs() > 1e-6 || uy.0.abs() > 1e-6 {
            return None; // rotated or skewed
        }
        let px = ux.0.abs().min(uy.1.abs()) * self.scale;
        let smallest = field.points.iter().map(|p| p.2).fold(f64::INFINITY, f64::min).min(if field.warp > 0.0 { field.warp_scale } else { f64::INFINITY });
        let factor = (smallest * px / 48.0).floor().clamp(1.0, 4.0);
        if factor < 2.0 {
            return None;
        }
        let items = self.layer_items(layer, root, &mut list.stops, &mut list.segments);
        let mut t = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for it in &items {
            let b = tight_bounds(it);
            t = [t[0].min(b[0]), t[1].min(b[1]), t[2].max(b[2]), t[3].max(b[3])];
        }
        let (cw, ch) = (canvas.0 as f64, canvas.1 as f64);
        let rect = [t[0].round().max(0.0), t[1].round().max(0.0), t[2].round().min(cw), t[3].round().min(ch)];
        if rect[2] <= rect[0] || rect[3] <= rect[1] {
            return None;
        }
        let (off, origin, f, (gw, gh)) = self.reserve(list, pending, items, rect, 0.0, factor, false);
        let forward = Proj::from(Affine::translate(origin[0], origin[1]).then(Affine::scale(f[0], f[1])));
        let prim = Prim {
            kind: [PRIM_IMAGE, 1, blend_code(Blend::Normal), 0],
            geom: [gw as f32, gh as f32, 1.0, 0.0],
            geom2: [f32::from_bits(off), 0.0, 0.0, 0.0],
            ..Default::default()
        };
        with_inverse(prim, forward, 1.0).map(|(prim, _)| Item { prim, forward, bounds: [0.0, 0.0, gw as f64, gh as f64] })
    }

    fn emit(&mut self, layer: &LayerState, root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, out: &mut Vec<Item>) {
        if let (false, Content::Mesh(m)) = (layer.is_isolated(), &layer.content) {
            if let Some(item) = self.mesh_item(layer, m, root, out.last_mut()) {
                if crate::mesh3d::refracts(m) && !out.is_empty() {
                    self.glass_backdrop(m, &item, canvas, list, pending, out);
                }
                out.push(item);
            }
            return;
        }
        if !layer.is_isolated() {
            if let Some(item) = self.field_proxy(layer, root, canvas, list, pending) {
                out.push(item);
                return;
            }
            out.extend(self.layer_items(layer, root, &mut list.stops, &mut list.segments));
            return;
        }
        if let Content::Group(children) = &layer.content {
            if let Some(items) = self.flat_group(layer, children, root, canvas, list, pending) {
                out.extend(items);
                return;
            }
        }
        let scale = self.scale;
        // The layer's own picture, in canvas space.
        let mut items = Vec::new();
        let group_opacity = match &layer.content {
            Content::Group(children) => {
                let inner = root.then(layer.matrix);
                self.emit_list(children, inner, canvas, list, pending, &mut items);
                layer.opacity
            }
            _ => {
                let bare = LayerState { effects: Vec::new(), matte: None, ..layer.clone() };
                items = self.layer_items(&bare, root, &mut list.stops, &mut list.segments);
                1.0
            }
        };
        let inverted = matches!(layer.matte.as_deref(), Some(m) if matches!(m.mode, MatteMode::AlphaInverted | MatteMode::LumaInverted));
        if items.is_empty() {
            return;
        }
        if let Some(m) = &layer.matte {
            if m.source.is_none() && !inverted {
                return;
            }
        }
        let mut blur = 0.0f64;
        let mut clamp = false;
        let mut reach = 0.0f64;
        let mut post_pad = 0.0f64;
        for e in &layer.effects {
            match e {
                EffectState::Blur { sigma, repeat_edges, .. } => {
                    blur = (blur * blur + sigma * sigma).sqrt();
                    clamp |= *repeat_edges;
                }
                EffectState::Shadow { offset, sigma, .. } => reach = reach.max(3.0 * sigma + offset.x().abs().max(offset.y().abs())),
                EffectState::Glow { sigma, .. } | EffectState::Bloom { sigma, .. } => reach = reach.max(3.0 * sigma),
                EffectState::Post(p) => match p.kind {
                    PostKind::Displace => post_pad += p.v[0].abs(),
                    PostKind::DirectionalBlur | PostKind::RgbSplit => post_pad += p.v[0].hypot(p.v[1]),
                    _ => {}
                },
            }
        }
        let blur = blur * scale;
        let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for it in &items {
            let b = canvas_bounds(it);
            bb = [bb[0].min(b[0]), bb[1].min(b[1]), bb[2].max(b[2]), bb[3].max(b[3])];
        }
        let (cw, ch) = (canvas.0 as f64, canvas.1 as f64);
        let pad = 3.0 * blur + 2.0 + post_pad * scale;
        let rect = [(bb[0] - pad).floor().max(0.0), (bb[1] - pad).floor().max(0.0), (bb[2] + pad).ceil().min(cw), (bb[3] + pad).ceil().min(ch)];
        let wide = |reach: f64| [(bb[0] - reach).floor().max(0.0), (bb[1] - reach).floor().max(0.0), (bb[2] + reach).ceil().min(cw), (bb[3] + reach).ceil().min(ch)];
        let factor_for = |sigma: f64| if sigma <= 4.0 { 1.0 } else { 2f64.powf((sigma / 4.0).log2().ceil()).min(16.0) };
        let blend = blend_code(layer.blend);
        let matte = layer.matte.as_ref().and_then(|m| m.source.as_ref().map(|src| (m.mode, src)));
        let matte_items = matte.map(|(_, src)| {
            let mut mi = Vec::new();
            self.emit(src, root, canvas, list, pending, &mut mi);
            mi
        });
        // Shadows and glows follow what the layer shows: with a matte they
        // are made from the matted picture, not from the whole layer.
        let has_outer = layer.effects.iter().any(|e| matches!(e, EffectState::Shadow { .. } | EffectState::Glow { .. }));
        let effect_src = match (matte, &matte_items) {
            (Some((mode, _)), Some(mi)) if has_outer && rect[2] > rect[0] && rect[3] > rect[1] => {
                let (off, origin, f, (gw, gh)) = self.reserve(list, pending, items.clone(), rect, 0.0, 1.0, false);
                let (moff, ..) = self.reserve(list, pending, mi.clone(), rect, 0.0, 1.0, false);
                let forward = Proj::from(Affine::translate(origin[0], origin[1]).then(Affine::scale(f[0], f[1])));
                let prim = Prim {
                    kind: [PRIM_MATTE, 1, 0, 0],
                    geom: [gw as f32, gh as f32, 1.0, 0.0],
                    geom2: [f32::from_bits(off), f32::from_bits(moff), matte_code(mode), 0.0],
                    ..Default::default()
                };
                with_inverse(prim, forward, 1.0)
                    .map(|(prim, _)| vec![Item { prim, forward, bounds: [0.0, 0.0, gw as f64, gh as f64] }])
                    .unwrap_or_default()
            }
            _ => items.clone(),
        };
        // Effects below the layer.
        for e in &layer.effects {
            if let EffectState::Shadow { color, offset, sigma } = e {
                let sigma = sigma * scale;
                let r = wide(reach * scale + 2.0);
                if r[2] <= r[0] || r[3] <= r[1] {
                    continue;
                }
                let (off, origin, f, (gw, gh)) = self.reserve(list, pending, effect_src.clone(), r, sigma, factor_for(sigma), false);
                let forward = Proj::from(Affine::translate(offset.x() * scale, offset.y() * scale).then(Affine::translate(origin[0], origin[1])).then(Affine::scale(f[0], f[1])));
                let prim = Prim {
                    kind: [PRIM_IMAGE, 1, 0, FLAG_TINT_ALPHA],
                    geom: [gw as f32, gh as f32, 1.0, 0.0],
                    geom2: [f32::from_bits(off), 0.0, 0.0, 0.0],
                    fill: premul(*color, 1.0),
                    ..Default::default()
                };
                if let Some((prim, _)) = with_inverse(prim, forward, group_opacity) {
                    out.push(Item { prim, forward, bounds: [0.0, 0.0, gw as f64, gh as f64] });
                }
            }
        }
        // Shadows and glows only: the layer itself is drawn as it is, with
        // no offscreen picture (that would cost a raster of its area and a
        // resampled composite for the same pixels).
        let plain = matte.is_none()
            && blur == 0.0
            && layer.effects.iter().all(|e| matches!(e, EffectState::Shadow { .. } | EffectState::Glow { .. }))
            && layer.blend == Blend::Normal
            && match &layer.content {
                Content::Group(children) => group_opacity >= 0.999 && children.iter().all(|c| c.blend == Blend::Normal),
                _ => true,
            };
        if plain {
            out.extend(items);
        } else if rect[2] > rect[0] && rect[3] > rect[1] {
            let factor = factor_for(blur);
            // Repeat-edge blur needs the content's exact rectangle: an
            // anti-aliasing margin would make the repeated edge transparent.
            let rect = if clamp {
                let mut t = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                for it in &items {
                    let b = tight_bounds(it);
                    t = [t[0].min(b[0]), t[1].min(b[1]), t[2].max(b[2]), t[3].max(b[3])];
                }
                [t[0].round().max(0.0), t[1].round().max(0.0), t[2].round().min(cw), t[3].round().min(ch)]
            } else {
                rect
            };
            let (off, origin, f, (gw, gh)) = self.reserve(list, pending, items.clone(), rect, blur, factor, clamp);
            let forward = Proj::from(Affine::translate(origin[0], origin[1]).then(Affine::scale(f[0], f[1])));
            let prim = match (matte, matte_items) {
                (Some((mode, _)), Some(mi)) => {
                    let (moff, ..) = self.reserve(list, pending, mi, rect, 0.0, factor, false);
                    Prim {
                        kind: [PRIM_MATTE, 1, blend, 0],
                        geom: [gw as f32, gh as f32, 1.0, 0.0],
                        geom2: [f32::from_bits(off), f32::from_bits(moff), matte_code(mode), 0.0],
                        ..Default::default()
                    }
                }
                _ => Prim {
                    kind: [PRIM_IMAGE, 1, blend, 0],
                    geom: [gw as f32, gh as f32, 1.0, 0.0],
                    geom2: [f32::from_bits(off), 0.0, 0.0, 0.0],
                    ..Default::default()
                },
            };
            let mut prim = prim;
            let (start, len) = self.program(&layer.effects, f, list);
            if len > 0 {
                prim.paint[2] = f32::from_bits(start);
                prim.paint[3] = f32::from_bits(len);
            }
            if let Some((prim, _)) = with_inverse(prim, forward, group_opacity) {
                out.push(Item { prim, forward, bounds: [0.0, 0.0, gw as f64, gh as f64] });
            }
        }
        // Effects above the layer.
        for e in &layer.effects {
            if let EffectState::Glow { color, sigma, intensity } = e {
                let sigma = sigma * scale;
                let r = wide(3.0 * sigma + 2.0);
                if r[2] <= r[0] || r[3] <= r[1] || *intensity <= 0.0 {
                    continue;
                }
                let (off, origin, f, (gw, gh)) = self.reserve(list, pending, effect_src.clone(), r, sigma, factor_for(sigma), false);
                let forward = Proj::from(Affine::translate(origin[0], origin[1]).then(Affine::scale(f[0], f[1])));
                let k = *intensity as f32;
                let prim = Prim {
                    kind: [PRIM_IMAGE, 1, 1, FLAG_TINT_MUL],
                    geom: [gw as f32, gh as f32, 1.0, 0.0],
                    geom2: [f32::from_bits(off), 0.0, 0.0, 0.0],
                    fill: [color.0[0] as f32 * k, color.0[1] as f32 * k, color.0[2] as f32 * k, color.0[3] as f32 * k],
                    ..Default::default()
                };
                if let Some((prim, _)) = with_inverse(prim, forward, group_opacity) {
                    out.push(Item { prim, forward, bounds: [0.0, 0.0, gw as f64, gh as f64] });
                }
            }
            if let EffectState::Bloom { threshold, sigma, intensity } = e {
                // What is brighter than the threshold, blurred, added on top.
                let sigma = sigma * scale;
                let r = wide(3.0 * sigma + 2.0);
                if r[2] <= r[0] || r[3] <= r[1] || *intensity <= 0.0 {
                    continue;
                }
                let factor = factor_for(sigma).min(4.0);
                let (aoff, aorigin, af, (aw, ah)) = self.reserve(list, pending, effect_src.clone(), r, 0.0, factor, false);
                let start = list.ops.len() as u32;
                list.ops.push([crate::raster::OP_THRESHOLD, 1.0, *threshold as f32, 0.0]);
                let aforward = Proj::from(Affine::translate(aorigin[0], aorigin[1]).then(Affine::scale(af[0], af[1])));
                let mut bright = Prim { kind: [PRIM_IMAGE, 1, 0, 0], geom: [aw as f32, ah as f32, 1.0, 0.0], geom2: [f32::from_bits(aoff), 0.0, 0.0, 0.0], ..Default::default() };
                bright.paint[2] = f32::from_bits(start);
                bright.paint[3] = f32::from_bits(1);
                let Some((bright, _)) = with_inverse(bright, aforward, 1.0) else { continue };
                let (off, origin, f, (gw, gh)) = self.reserve(list, pending, vec![Item { prim: bright, forward: aforward, bounds: [0.0, 0.0, aw as f64, ah as f64] }], r, sigma, factor_for(sigma), false);
                let forward = Proj::from(Affine::translate(origin[0], origin[1]).then(Affine::scale(f[0], f[1])));
                let k = *intensity as f32;
                let prim = Prim { kind: [PRIM_IMAGE, 1, 1, FLAG_TINT_MUL], geom: [gw as f32, gh as f32, 1.0, 0.0], geom2: [f32::from_bits(off), 0.0, 0.0, 0.0], fill: [k, k, k, k], ..Default::default() };
                if let Some((prim, _)) = with_inverse(prim, forward, group_opacity) {
                    out.push(Item { prim, forward, bounds: [0.0, 0.0, gw as f64, gh as f64] });
                }
            }
        }
    }

    fn layer_items(&mut self, layer: &LayerState, root: Proj, stops: &mut Vec<[f32; 4]>, segments: &mut Vec<[f32; 4]>) -> Vec<Item> {
        let matrix = root.then(layer.matrix);
        let blend = blend_code(layer.blend);
        let mut items = Vec::new();
        match &layer.content {
            Content::Group(_) => {}
            Content::Mesh(m) => items.extend(self.mesh_item(layer, m, root, None)),
            Content::Shape(s) if s.shape == ShapeKind::Path => {
                items.extend(path_items(s, matrix, layer.opacity, blend, stops, segments));
            }
            Content::Shape(s) => {
                let mut prim = Prim { kind: [PRIM_SHAPE, 0, blend, 0], ..Default::default() };
                let (hw, hh) = (s.size.x() / 2.0, s.size.y() / 2.0);
                let half_t = s.thickness / 2.0;
                let bounds_half;
                let mut box_half = None;
                match s.shape {
                    ShapeKind::Circle => {
                        prim.kind[1] = SHAPE_ELLIPSE;
                        prim.geom = [hw as f32, hh as f32, 0.0, 0.0];
                        bounds_half = (hw, hh);
                    }
                    ShapeKind::Rect => {
                        prim.kind[1] = SHAPE_RECT;
                        prim.geom = [hw as f32, hh as f32, s.radius.min(hw.min(hh)) as f32, 0.0];
                        bounds_half = (hw, hh);
                    }
                    ShapeKind::Capsule => {
                        prim.kind[1] = SHAPE_CAPSULE;
                        prim.geom = [hw as f32, hh as f32, 0.0, 0.0];
                        bounds_half = (hw, hh);
                    }
                    ShapeKind::Ring | ShapeKind::Arc => {
                        prim.kind[1] = SHAPE_ARC;
                        let (a0, a1) = if s.shape == ShapeKind::Ring { (0.0, 360.0) } else { (s.angles.x(), s.angles.y()) };
                        let span = a1 - a0;
                        let (t0, t1) = (s.trim.x().clamp(0.0, 1.0), s.trim.y().clamp(0.0, 1.0));
                        if t1 <= t0 || span.abs() < 1e-6 {
                            return items;
                        }
                        let (b0, b1) = (a0 + span * t0, a0 + span * t1);
                        let (lo, hi) = if b1 >= b0 { (b0, b1) } else { (b1, b0) };
                        let full = (hi - lo) >= 360.0 - 1e-6;
                        let rc = (hw - half_t).max(0.0);
                        prim.geom = [rc as f32, half_t as f32, ((lo + hi) / 2.0).to_radians() as f32, ((hi - lo) / 2.0).min(180.0).to_radians() as f32];
                        prim.geom2[0] = if full { 1.0 } else { 0.0 };
                        bounds_half = (hw, hw);
                    }
                    ShapeKind::Line => {
                        prim.kind[1] = SHAPE_LINE;
                        let (t0, t1) = (s.trim.x().clamp(0.0, 1.0), s.trim.y().clamp(0.0, 1.0));
                        if t1 <= t0 {
                            return items;
                        }
                        let len = s.size.x();
                        prim.geom = [(-len / 2.0 + len * t0) as f32, (-len / 2.0 + len * t1) as f32, half_t as f32, 0.0];
                        bounds_half = (hw + half_t, half_t);
                        box_half = Some((hw, half_t));
                    }
                    ShapeKind::Path => unreachable!("paths are handled above"),
                    ShapeKind::Field => {
                        prim.kind[1] = SHAPE_RECT;
                        prim.geom = [hw as f32, hh as f32, s.radius.min(hw.min(hh)) as f32, 0.0];
                        bounds_half = (hw, hh);
                    }
                    ShapeKind::Polygon | ShapeKind::Star => {
                        prim.kind[1] = SHAPE_STAR;
                        let n = s.sides.max(3) as f64;
                        let k = if s.shape == ShapeKind::Polygon { (std::f64::consts::PI / n).cos() } else { s.inner };
                        prim.geom = [hw as f32, k as f32, n as f32, s.radius.min(hw) as f32];
                        bounds_half = (hw, hw);
                    }
                }
                let stroke_w = s.stroke.map(|(_, w)| w).unwrap_or(0.0);
                let (bx, by) = box_half.unwrap_or(bounds_half);
                match (&s.field, &s.fill) {
                    (Some(field), _) => {
                        if !field.points.is_empty() {
                            prim.kind[3] |= FLAG_FILL;
                            set_field(&mut prim, field, stops);
                        }
                    }
                    (None, Some(fill)) => {
                        prim.kind[3] |= FLAG_FILL;
                        set_fill(&mut prim, fill, stops, [-bx, -by, bx, by]);
                    }
                    (None, None) => {}
                }
                if let Some((c, w)) = s.stroke {
                    if w > 0.0 && c.0[3] > 0.0 {
                        prim.kind[3] |= FLAG_STROKE;
                        prim.stroke = premul(c, 1.0);
                        prim.paint[0] = w as f32;
                    }
                }
                if prim.kind[3] == 0 {
                    return items;
                }
                let pad = stroke_w / 2.0;
                if let Some((prim, _)) = with_inverse(prim, matrix, layer.opacity) {
                    let pad_px = pad + prim.inv_x[3] as f64 * 1.5;
                    items.push(Item {
                        prim,
                        forward: matrix,
                        bounds: [-bounds_half.0 - pad_px, -bounds_half.1 - pad_px, bounds_half.0 + pad_px, bounds_half.1 + pad_px],
                    });
                }
            }
            Content::Text(text) => {
                if text.size <= 0.0 {
                    return items;
                }
                let layout = self.fonts.layout(text);
                let (glyphs, wipe) = font::sweep(&layout, text);
                let style = GlyphStyle::new(text);
                let color = premul(text.color, 1.0);
                let mut strokes = Vec::new();
                for g in &layout.glyphs {
                    let (glyph_xf, glyph_opacity) = Fonts::glyph_transform(&layout, text, &glyphs, g);
                    let opacity = layer.opacity * glyph_opacity;
                    if opacity <= 0.0 {
                        continue;
                    }
                    let forward = matrix.then(glyph_xf);
                    let r = g.atlas.rect;
                    let bounds = [r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64];
                    let fill = match glyphs.get(g.char_index).and_then(|s| s.color) {
                        Some(c) => premul(c, 1.0),
                        None => color,
                    };
                    let mut prim = Prim {
                        kind: [PRIM_GLYPH, 0, blend, FLAG_FILL],
                        geom: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                        fill,
                        paint: [style.weight, 0.0, 0.0, 0.0],
                        ..Default::default()
                    };
                    if let Some(w) = &wipe {
                        (prim.kind[1], prim.paint2) = w.params(glyph_xf);
                        prim.geom2[0] = w.opacity[0];
                        prim.geom2[1] = w.opacity[1];
                    }
                    if let Some((band, stroke_color)) = style.stroke {
                        let mut s = prim;
                        s.kind[3] = FLAG_STROKE;
                        s.fill = stroke_color;
                        s.paint = [style.weight, band[0], band[1], 0.0];
                        if let Some((s, _)) = with_inverse(s, forward, opacity) {
                            strokes.push(Item { prim: s, forward, bounds });
                        }
                    }
                    if let Some((edge_color, depth, light)) = style.edge {
                        // Light direction into atlas space (normals turn
                        // with the glyph).
                        if let Some(inv) = glyph_xf.inverse() {
                            let [a, b, c, d, _, _] = inv.0;
                            let (lx, ly) = (a * light[0] + c * light[1], b * light[0] + d * light[1]);
                            let n = lx.hypot(ly).max(1e-12);
                            prim.kind[3] |= FLAG_EDGE;
                            prim.stroke = edge_color;
                            prim.paint[3] = depth;
                            prim.geom2[2] = (lx / n) as f32;
                            prim.geom2[3] = (ly / n) as f32;
                        }
                    }
                    if color[3] <= 0.0 && prim.kind[3] & FLAG_EDGE == 0 {
                        continue;
                    }
                    if let Some((prim, _)) = with_inverse(prim, forward, opacity) {
                        items.push(Item { prim, forward, bounds });
                    }
                }
                // Outside outlines sit under every fill (one outline around
                // touching letters); centred and inside ones on top.
                match text.stroke.map(|s| s.2) {
                    Some(StrokeAlign::Outside) => {
                        strokes.extend(items);
                        items = strokes;
                    }
                    _ => items.extend(strokes),
                }
            }
            Content::Video(v) => {
                let Some(tex) = self.footage.frame(&v.src, v.source_time, v.looped, v.trim, v.fps) else { return items };
                let (dw, dh) = match v.size {
                    Some(s) => (s.x(), s.y()),
                    None => (tex.width as f64, tex.height as f64),
                };
                if dw <= 0.0 || dh <= 0.0 {
                    return items;
                }
                let forward = matrix
                    .then(Affine::translate(-dw / 2.0, -dh / 2.0))
                    .then(Affine::scale(dw / tex.width as f64, dh / tex.height as f64));
                let prim = Prim {
                    kind: [PRIM_IMAGE, SOURCE_FOOTAGE, blend, 0],
                    geom: [tex.width as f32, tex.height as f32, tex.levels as f32, 0.0],
                    geom2: [f32::from_bits(tex.offset), 0.0, 0.0, 0.0],
                    ..Default::default()
                };
                if let Some((prim, _)) = with_inverse(prim, forward, layer.opacity) {
                    items.push(Item { prim, forward, bounds: [0.0, 0.0, tex.width as f64, tex.height as f64] });
                }
            }
            Content::Image { src, size } => {
                let Some(tex) = self.images.get(src) else { return items };
                let (dw, dh) = match size {
                    Some(s) => (s.x(), s.y()),
                    None => (tex.width as f64, tex.height as f64),
                };
                if dw <= 0.0 || dh <= 0.0 {
                    return items;
                }
                // Texel space → layer space: centred, scaled to display size.
                let forward = matrix
                    .then(Affine::translate(-dw / 2.0, -dh / 2.0))
                    .then(Affine::scale(dw / tex.width as f64, dh / tex.height as f64));
                let prim = Prim {
                    kind: [PRIM_IMAGE, 0, blend, 0],
                    geom: [tex.width as f32, tex.height as f32, tex.levels as f32, 0.0],
                    geom2: [f32::from_bits(tex.offset), 0.0, 0.0, 0.0],
                    ..Default::default()
                };
                if let Some((prim, _)) = with_inverse(prim, forward, layer.opacity) {
                    items.push(Item { prim, forward, bounds: [0.0, 0.0, tex.width as f64, tex.height as f64] });
                }
            }
        }
        items
    }
}

/// A flat 3D layer the path tracer can place in its scene as a card.
fn trace_card_candidate(layer: &LayerState) -> bool {
    layer.view.is_some() && !matches!(layer.content, Content::Mesh(_) | Content::Group(_)) && plain(layer)
}

/// No effects (but depth of field's blur, which the tracer's lens makes),
/// matte or blend mode.
fn plain(layer: &LayerState) -> bool {
    not_plain(layer).is_none()
}

/// What keeps a layer from being traced: effects, a matte or a blend mode.
fn not_plain(layer: &LayerState) -> Option<&'static str> {
    if layer.matte.is_some() {
        Some("a matte")
    } else if !layer.effects.iter().all(|e| matches!(e, EffectState::Blur { lens: true, .. })) {
        Some("effects; use the camera's aperture for depth of field")
    } else if layer.blend != Blend::Normal {
        Some("a blend mode")
    } else {
        None
    }
}

/// The first layer that keeps the path tracer from drawing this list as the
/// raster would, and why; `None` when it can: plain meshes without
/// wireframes and plain flat 3D layers. Other lists are rasterised (the
/// tracer has no effects, mattes, blend modes or wires yet, and a 3D layer
/// drawn flat beside the traced picture would lose its depth). Depth of
/// field is traced through a lens.
fn untraceable(layers: &[LayerState]) -> Option<(std::sync::Arc<str>, &'static str)> {
    layers.iter().find_map(|l| {
        let why = match &l.content {
            Content::Mesh(m) => not_plain(l).or_else(|| m.materials.iter().any(|x| x.wireframe > 0.0).then_some("a wireframe")),
            Content::Group(_) => None,
            _ if l.view.is_none() || trace_card_candidate(l) => None,
            _ => Some(not_plain(l).unwrap_or("a flat 3D layer the tracer can't place")),
        };
        why.map(|w| (l.id.clone(), w))
    })
}

/// A flat 3D layer that can stand in the mesh depth buffer as a card. A
/// blur (depth of field adds one) is drawn into the card's picture.
fn card_candidate(layer: &LayerState) -> bool {
    layer.view.is_some()
        && !matches!(layer.content, Content::Mesh(_) | Content::Group(_))
        && layer.matte.is_none()
        && layer.effects.iter().all(|e| matches!(e, EffectState::Blur { repeat_edges: false, .. }))
        && layer.blend == Blend::Normal
        && layer.opacity >= 0.999
}

/// The combined blur of a card's effects, in canvas px.
fn card_blur(layer: &LayerState, scale: f64) -> f64 {
    let sum: f64 = layer.effects.iter().map(|e| if let EffectState::Blur { sigma, .. } = e { sigma * sigma } else { 0.0 }).sum();
    sum.sqrt() * scale
}

/// Canvas bounds without the anti-aliasing margins.
fn bounds_of(pts: [(f64, f64); 4], pad: f64) -> [f64; 4] {
    // A plane crossing the camera has no finite bounds: cover everything.
    if pts.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return [-1e9, -1e9, 1e9, 1e9];
    }
    [
        pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - pad,
        pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - pad,
        pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + pad,
        pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) + pad,
    ]
}

fn tight_bounds(item: &Item) -> [f64; 4] {
    let m = item.prim.inv_x[3] as f64 * 1.5;
    let [x0, y0, x1, y1] = item.bounds;
    let inner = if item.prim.kind[0] == PRIM_SHAPE { [x0 + m, y0 + m, x1 - m, y1 - m] } else { item.bounds };
    let pts = [(inner[0], inner[1]), (inner[2], inner[1]), (inner[2], inner[3]), (inner[0], inner[3])].map(|(x, y)| item.forward.apply(x, y));
    bounds_of(pts, 0.0)
}

fn canvas_bounds(item: &Item) -> [f64; 4] {
    let [x0, y0, x1, y1] = item.bounds;
    let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| item.forward.apply(x, y));
    bounds_of(pts, 1.0)
}

/// Appends prims and their per-tile index lists for one target.
fn bin(list: &mut DisplayList, target: &Target, prims: Vec<(Prim, [f64; 4])>) {
    let n = target.tiles() as usize;
    let mut per_tile: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (prim, b) in prims {
        if !(b[2] > 0.0 && b[3] > 0.0 && b[0] < target.width as f64 && b[1] < target.height as f64) {
            continue;
        }
        if let Some(parts) = split_path(list, target, &prim, b) {
            for (tile, part) in parts {
                per_tile[tile].push(list.prims.len() as u32);
                list.prims.push(part);
            }
            continue;
        }
        let index = list.prims.len() as u32;
        list.prims.push(prim);
        let tx0 = (b[0].max(0.0) as u32) / TILE;
        let ty0 = (b[1].max(0.0) as u32) / TILE;
        let tx1 = ((b[2].min(target.width as f64 - 1.0)).max(0.0) as u32) / TILE;
        let ty1 = ((b[3].min(target.height as f64 - 1.0)).max(0.0) as u32) / TILE;
        for ty in ty0..=ty1.min(target.tiles_y - 1) {
            for tx in tx0..=tx1.min(target.tiles_x - 1) {
                per_tile[(ty * target.tiles_x + tx) as usize].push(index);
            }
        }
    }
    for tile in per_tile {
        list.ranges.push(list.indices.len() as u32);
        list.ranges.push(tile.len() as u32);
        list.indices.extend(tile);
    }
}

/// Segments a path needs before it is split per tile.
const SPLIT_SEGMENTS: u32 = 48;

/// A big path (a country, a coastline) costs every one of its segments at
/// every pixel. Per tile only some matter. With a flat transform (no
/// rotation or perspective) each tile gets its own copy of the primitive
/// holding just the segments that reach it (within the shading margin).
/// For fills, the crossings of segments entirely to its right (the winding
/// ray runs to local +x) are the same for every pixel of a pixel row, so
/// they are summed once per row into a header the shader starts from
/// (`geom2.w` = 1): a header entry [local y of row 0, local y step, 0, 0],
/// then 16 row windings in 4 entries, then the segments.
fn split_path(list: &mut DisplayList, target: &Target, prim: &Prim, b: [f64; 4]) -> Option<Vec<(usize, Prim)>> {
    let count = prim.geom2[1].to_bits();
    if prim.kind[0] != PRIM_SHAPE || prim.kind[1] != SHAPE_PATH || count < SPLIT_SEGMENTS {
        return None;
    }
    let (a, c, d, f, w) = (prim.inv_x[0], prim.inv_x[2], prim.inv_y[1], prim.inv_y[2], prim.inv_w[2]);
    if prim.inv_x[1] != 0.0 || prim.inv_y[0] != 0.0 || prim.inv_w[0] != 0.0 || prim.inv_w[1] != 0.0 || a == 0.0 || d == 0.0 || w <= 0.0 {
        return None;
    }
    let fill = prim.geom2[2] < 0.5;
    let first = prim.geom2[0].to_bits() as usize;
    let segs: Vec<[f32; 4]> = list.segments[first..first + count as usize].to_vec();
    let centreline = (prim.paint[1] as u32 == PAINT_ALONG).then(|| {
        let (cf, cn) = (prim.paint2[0].to_bits() as usize, prim.paint2[1].to_bits() as usize);
        Centreline { segs: list.segments[cf..cf + cn].to_vec(), params: list.segments[cf + cn..cf + 2 * cn].to_vec() }
    });
    // Local x and y of a pixel coordinate, and back.
    let lx = |px: f32| (a * px + c) / w;
    let ly = |py: f32| (d * py + f) / w;
    let py_of = |y: f32| (y * w - f) / d;
    // Shading reach: anti-aliasing, plus half a stroke, in local units.
    let aa = prim.inv_x[3].abs();
    let stroke = if prim.kind[3] & FLAG_STROKE != 0 { prim.paint[0] * 0.5 } else { 0.0 };
    let m = 2.0 * aa.max((a / w).abs()) + stroke;
    let (tw, th) = (target.tiles_x, target.tiles_y);
    let ty0 = ((b[1].max(0.0) as u32) / TILE).min(th - 1);
    let ty1 = (((b[3].min(target.height as f64 - 1.0)).max(0.0) as u32) / TILE).min(th - 1);
    let tx0 = ((b[0].max(0.0) as u32) / TILE).min(tw - 1);
    let tx1 = (((b[2].min(target.width as f64 - 1.0)).max(0.0) as u32) / TILE).min(tw - 1);
    // Segment bounds in local units, and the pixel rows they span.
    let lb: Vec<[f32; 4]> = segs.iter().map(|s| [s[0].min(s[2]), s[1].min(s[3]), s[0].max(s[2]), s[1].max(s[3])]).collect();
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); (ty1 - ty0 + 1) as usize];
    for (i, s) in lb.iter().enumerate() {
        let (p0, p1) = (py_of(s[1] - m), py_of(s[3] + m));
        let r0 = ((p0.min(p1).max(0.0) as u32) / TILE).max(ty0);
        let r1 = ((p0.max(p1).max(0.0) as u32) / TILE).min(ty1);
        for r in r0..=r1 {
            rows[(r - ty0) as usize].push(i as u32);
        }
    }
    let mut out = Vec::new();
    for ty in ty0..=ty1 {
        let band = &rows[(ty - ty0) as usize];
        if band.is_empty() {
            continue;
        }
        // Row centres in local y, and each band segment's crossing there.
        let y_rows: [f32; 16] = std::array::from_fn(|r| ly((ty * TILE + r as u32) as f32 + 0.5));
        let mut order: Vec<u32> = band.clone();
        order.sort_by(|&i, &j| lb[i as usize][0].total_cmp(&lb[j as usize][0]));
        // suffix[k][r]: winding at row r from order[k..].
        let mut suffix = vec![[0i32; 16]; order.len() + 1];
        if fill {
            for k in (0..order.len()).rev() {
                let s = &segs[order[k] as usize];
                let mut acc = suffix[k + 1];
                for (r, y) in y_rows.iter().enumerate() {
                    if (s[1] <= *y) != (s[3] <= *y) {
                        acc[r] += if s[3] > s[1] { 1 } else { -1 };
                    }
                }
                suffix[k] = acc;
            }
        }
        let xmins: Vec<f32> = order.iter().map(|&i| lb[i as usize][0]).collect();
        let (ya, yb) = (ly((ty * TILE) as f32), ly(((ty + 1) * TILE) as f32));
        for tx in tx0..=tx1 {
            let (xa, xb) = (lx((tx * TILE) as f32), lx(((tx + 1) * TILE) as f32));
            let (x0, x1) = (xa.min(xb) - m, xa.max(xb) + m);
            // Wholly right of the tile in local x (the ray's way): summed.
            let far = if a / w > 0.0 { xmins.partition_point(|&x| x <= x1) } else { order.len() };
            let near: Vec<u32> = order[..far].iter().copied().filter(|&i| lb[i as usize][2] >= x0).collect();
            let backdrop = if fill && a / w > 0.0 { suffix[far] } else { [0; 16] };
            if near.is_empty() && backdrop.iter().all(|v| *v == 0) {
                continue;
            }
            let start = list.segments.len() as u32;
            let mut part = *prim;
            if fill && a / w > 0.0 {
                list.segments.push([y_rows[0], d / w, 0.0, 0.0]);
                for q in 0..4 {
                    list.segments.push(std::array::from_fn(|k| backdrop[q * 4 + k] as f32));
                }
                part.geom2[3] = 1.0;
            } else if fill {
                // Mirrored: no backdrop, every segment right of the tile.
                list.segments.extend(order[..].iter().filter(|&&i| lb[i as usize][2] >= x0).map(|&i| segs[i as usize]));
                part.geom2[0] = f32::from_bits(start);
                part.geom2[1] = f32::from_bits(list.segments.len() as u32 - start);
                if let Some(c) = &centreline {
                    cull_along(list, &mut part, c, [xa.min(xb), ya.min(yb), xa.max(xb), ya.max(yb)]);
                }
                out.push(((ty * tw + tx) as usize, part));
                continue;
            }
            list.segments.extend(near.iter().map(|&i| segs[i as usize]));
            part.geom2[0] = f32::from_bits(start);
            part.geom2[1] = f32::from_bits(near.len() as u32);
            if let Some(c) = &centreline {
                cull_along(list, &mut part, c, [xa.min(xb), ya.min(yb), xa.max(xb), ya.max(yb)]);
            }
            out.push(((ty * tw + tx) as usize, part));
        }
    }
    Some(out)
}

/// A colour-along-length prim's centreline: segments and their [u0, u1]
/// params.
struct Centreline {
    segs: Vec<[f32; 4]>,
    params: Vec<[f32; 4]>,
}

/// Points `part` at the centreline segments that can be nearest to some
/// pixel of the tile `rect` (local units), so the per-pixel lookup scans
/// those instead of the whole line. A segment further from the tile
/// centre than the nearest one plus the tile's diagonal is never the
/// nearest to any pixel in it, so shading is unchanged.
fn cull_along(list: &mut DisplayList, part: &mut Prim, c: &Centreline, rect: [f32; 4]) {
    let centre = [(rect[0] + rect[2]) * 0.5, (rect[1] + rect[3]) * 0.5];
    // Half the diagonal, plus a pixel's worth of slack for sample offsets.
    let half = 0.5 * (rect[2] - rect[0]).hypot(rect[3] - rect[1]) * 1.1;
    let dist = |s: &[f32; 4]| {
        let e = [s[2] - s[0], s[3] - s[1]];
        let w = [centre[0] - s[0], centre[1] - s[1]];
        let t = clamp01((w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]).max(1e-12));
        (w[0] - e[0] * t).hypot(w[1] - e[1] * t)
    };
    let d: Vec<f32> = c.segs.iter().map(dist).collect();
    let nearest = d.iter().copied().fold(f32::MAX, f32::min);
    let keep: Vec<usize> = (0..d.len()).filter(|&i| d[i] <= nearest + 2.0 * half).collect();
    if keep.len() == d.len() {
        return;
    }
    let start = list.segments.len() as u32;
    list.segments.extend(keep.iter().map(|&i| c.segs[i]));
    list.segments.extend(keep.iter().map(|&i| c.params[i]));
    part.paint2[0] = f32::from_bits(start);
    part.paint2[1] = f32::from_bits(keep.len() as u32);
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn push_segments(lines: &[Vec<(f64, f64)>], close: bool, segments: &mut Vec<[f32; 4]>) -> (u32, u32, [f64; 4]) {
    let first = segments.len() as u32;
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for line in lines {
        let mut pts = line.clone();
        if close && pts.first() != pts.last() {
            pts.push(pts[0]);
        }
        for w in pts.windows(2) {
            segments.push([w[0].0 as f32, w[0].1 as f32, w[1].0 as f32, w[1].1 as f32]);
        }
        for p in &pts {
            b = [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)];
        }
    }
    (first, segments.len() as u32 - first, b)
}

fn path_items(s: &ShapeState, matrix: Proj, opacity: f64, blend: u32, stops: &mut Vec<[f32; 4]>, segments: &mut Vec<[f32; 4]>) -> Vec<Item> {
    let mut items = Vec::new();
    let Some(m) = &s.path else { return items };
    let tolerance = path::tolerance(matrix.max_scale());
    let Ok(subpaths) = morph(m, tolerance) else { return items };
    let mut add = |prim: Prim, bounds: [f64; 4], pad: f64| {
        if let Some((prim, _)) = with_inverse(prim, matrix, opacity) {
            let pad = pad + prim.inv_x[3] as f64 * 1.5;
            items.push(Item { prim, forward: matrix, bounds: [bounds[0] - pad, bounds[1] - pad, bounds[2] + pad, bounds[3] + pad] });
        }
    };
    if let Some(fill) = &s.fill {
        let lines: Vec<Vec<(f64, f64)>> = subpaths.iter().map(|(p, _)| p.clone()).collect();
        let (first, count, b) = push_segments(&lines, true, segments);
        let mut prim = Prim { kind: [PRIM_SHAPE, SHAPE_PATH, blend, FLAG_FILL], geom2: [f32::from_bits(first), f32::from_bits(count), 0.0, 0.0], ..Default::default() };
        set_fill(&mut prim, fill, stops, b);
        add(prim, b, 0.0);
    }
    if let Some((color, width)) = s.stroke {
        let (t0, t1) = (s.trim.x().clamp(0.0, 1.0), s.trim.y().clamp(0.0, 1.0));
        // Feathers fade along the path's length (every subpath, any curve):
        // a solid stroke with feathers is a one-colour gradient along it.
        let feathered = s.stroke_style.as_deref().is_some_and(|st| st.feather > 0.0 || st.feather_start > 0.0);
        let solid = [(0.0, color), (1.0, color)];
        let along = s.stroke_gradient.as_deref().map(|g| g.as_slice()).or(feathered.then_some(&solid[..]));
        if width > 0.0 && (color.0[3] > 0.0 || along.is_some()) && t1 > t0 {
            let measured = trim_measured(&subpaths, t0, t1);
            let lines: Vec<Vec<(f64, f64)>> = measured.iter().map(|l| l.iter().map(|(p, _)| *p).collect()).collect();
            // Colour along the length: stops (feathers folded in) looked
            // up by the nearest point of the visible centreline.
            let along_stops = along.map(|g| along_stops(g, s.stroke_style.as_deref(), (t0, t1), path::length(&subpaths)));
            if let Some(style) = &s.stroke_style {
                // Caps, joins, dashes and feather: outline the stroke and fill it.
                let centreline = along_stops.as_ref().map(|_| {
                    let (first, count, _) = push_segments(&lines, false, segments);
                    push_params(&measured, segments);
                    (first, count)
                });
                let outline = stroke_outline(&lines, width, style, tolerance);
                let (first, count, b) = push_segments(&outline, true, segments);
                if count > 0 {
                    let mut prim = Prim { kind: [PRIM_SHAPE, SHAPE_PATH, blend, FLAG_FILL], geom2: [f32::from_bits(first), f32::from_bits(count), 0.0, 0.0], ..Default::default() };
                    match (centreline, &along_stops) {
                        (Some(c), Some(a)) => set_along(&mut prim, c, a, stops),
                        _ => set_fill(&mut prim, &PaintState::Solid(color), stops, b),
                    }
                    add(prim, b, 0.0);
                }
                return items;
            }
            let (first, count, b) = push_segments(&lines, false, segments);
            if count > 0 {
                let mut prim = Prim {
                    kind: [PRIM_SHAPE, SHAPE_PATH, blend, FLAG_STROKE],
                    geom2: [f32::from_bits(first), f32::from_bits(count), 1.0, 0.0],
                    stroke: premul(color, 1.0),
                    paint: [width as f32, 0.0, 0.0, 0.0],
                    ..Default::default()
                };
                if let Some(a) = &along_stops {
                    // The stroke's own segments are its centreline.
                    push_params(&measured, segments);
                    set_along(&mut prim, (first, count), a, stops);
                }
                add(prim, b, width / 2.0);
            }
        }
    }
    items
}

/// Appends [u0, u1, 0, 0] for each segment of `measured`, in the order
/// `push_segments(…, false, …)` pushed them.
fn push_params(measured: &[Vec<((f64, f64), f64)>], segments: &mut Vec<[f32; 4]>) {
    for line in measured {
        for w in line.windows(2) {
            segments.push([w[0].1 as f32, w[1].1 as f32, 0.0, 0.0]);
        }
    }
}

fn set_along(prim: &mut Prim, (first, count): (u32, u32), along: &[(f32, [f32; 4])], stops: &mut Vec<[f32; 4]>) {
    push_stops(prim, along, stops);
    prim.paint[1] = PAINT_ALONG as f32;
    prim.paint2 = [f32::from_bits(first), f32::from_bits(count), 0.0, 0.0];
}

/// A stroke gradient's stops (premultiplied) over the full path's length,
/// times the feathers' opacity ramps at the ends of the visible part
/// `trim`. Ramps are sampled finely, so the product stays smooth.
fn along_stops(gradient: &[(f64, Color)], style: Option<&StrokeStyle>, trim: (f64, f64), length: f64) -> Vec<(f32, [f32; 4])> {
    let colours: Vec<(f64, [f32; 4])> = gradient.iter().map(|(o, c)| (*o, premul(*c, 1.0))).collect();
    let colour = |u: f64| -> [f32; 4] {
        let (first, last) = (colours[0], colours[colours.len() - 1]);
        if u <= first.0 {
            return first.1;
        }
        for w in colours.windows(2) {
            if u <= w[1].0 {
                let k = ((u - w[0].0) / (w[1].0 - w[0].0).max(1e-6)).clamp(0.0, 1.0) as f32;
                return std::array::from_fn(|i| w[0].1[i] + (w[1].1[i] - w[0].1[i]) * k);
            }
        }
        last.1
    };
    let (tail, head) = match (style, length > 0.0) {
        (Some(st), true) => (st.feather_start / length, st.feather / length),
        _ => (0.0, 0.0),
    };
    let alpha = |u: f64| {
        let a = if tail > 0.0 { ((u - trim.0) / tail).clamp(0.0, 1.0) } else { 1.0 };
        let b = if head > 0.0 { ((trim.1 - u) / head).clamp(0.0, 1.0) } else { 1.0 };
        a * b
    };
    let mut at: Vec<f64> = colours.iter().map(|(o, _)| *o).collect();
    const STEPS: usize = 8;
    for (from, to) in [(trim.0, trim.0 + tail), (trim.1 - head, trim.1)] {
        if to > from {
            at.extend((0..=STEPS).map(|i| from + (to - from) * i as f64 / STEPS as f64));
        }
    }
    at.sort_by(|a, b| a.total_cmp(b));
    at.dedup_by(|a, b| (*a - *b).abs() < 1e-7);
    at.into_iter()
        .map(|u| {
            let (c, a) = (colour(u), alpha(u) as f32);
            (u as f32, c.map(|v| v * a))
        })
        .collect()
}

/// The filled outline of a stroke along `lines` (layer space), flattened.
fn stroke_outline(lines: &[Vec<(f64, f64)>], width: f64, style: &StrokeStyle, tolerance: f64) -> Vec<Vec<(f64, f64)>> {
    let mut path = kurbo::BezPath::new();
    for line in lines {
        for (i, p) in line.iter().enumerate() {
            if i == 0 {
                path.move_to(*p);
            } else {
                path.line_to(*p);
            }
        }
    }
    let cap = match style.cap {
        LineCap::Round => kurbo::Cap::Round,
        LineCap::Butt => kurbo::Cap::Butt,
        LineCap::Square => kurbo::Cap::Square,
    };
    let join = match style.join {
        LineJoin::Round => kurbo::Join::Round,
        LineJoin::Miter => kurbo::Join::Miter,
        LineJoin::Bevel => kurbo::Join::Bevel,
    };
    let mut k = kurbo::Stroke::new(width).with_caps(cap).with_join(join).with_miter_limit(style.miter_limit);
    if !style.dash.is_empty() {
        k = k.with_dashes(style.dash_offset, style.dash.iter().copied());
    }
    let outline = kurbo::stroke(path, &k, &kurbo::StrokeOpts::default(), tolerance);
    let mut polys: Vec<Vec<(f64, f64)>> = Vec::new();
    kurbo::flatten(&outline, tolerance, |el| match el {
        kurbo::PathEl::MoveTo(p) => polys.push(vec![(p.x, p.y)]),
        kurbo::PathEl::LineTo(p) => {
            if let Some(poly) = polys.last_mut() {
                poly.push((p.x, p.y));
            }
        }
        kurbo::PathEl::ClosePath => {}
        _ => {}
    });
    polys.retain(|p| p.len() >= 3);
    polys
}

/// Sets a solid or gradient fill; `bbox` (local [x0, y0, x1, y1]) is the
/// shape's box for gradients in box units.
fn set_fill(prim: &mut Prim, fill: &PaintState, stops: &mut Vec<[f32; 4]>, bbox: [f64; 4]) {
    match fill {
        PaintState::Solid(c) => prim.fill = premul(*c, 1.0),
        PaintState::Gradient(g) => {
            prim.fill = [1.0; 4];
            let mut sorted = g.stops.clone();
            sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
            let sorted: Vec<(f32, [f32; 4])> = sorted.into_iter().map(|(o, c)| (o as f32, premul(c, 1.0))).collect();
            push_stops(prim, &sorted, stops);
            let (kind, geometry) = gradient_geometry(g, bbox);
            prim.paint[1] = kind as f32;
            prim.paint2 = geometry.map(|v| v as f32);
        }
    }
}

fn push_stops(prim: &mut Prim, sorted: &[(f32, [f32; 4])], stops: &mut Vec<[f32; 4]>) {
    prim.paint[2] = (stops.len() / 2) as f32;
    prim.paint[3] = sorted.len() as f32;
    for (offset, color) in sorted {
        stops.push([*offset, 0.0, 0.0, 0.0]);
        stops.push(*color);
    }
}

/// Paint kind and geometry of a gradient in local units. Box units map
/// [0, 0]..[1, 1] onto `bbox`: a linear gradient stays linear (its
/// equivalent local from/to), a radial one becomes an ellipse.
fn gradient_geometry(g: &Gradient, bbox: [f64; 4]) -> (u32, [f64; 4]) {
    let (f, t) = ([g.from.x(), g.from.y()], [g.to.x(), g.to.y()]);
    let (m, s) = ([bbox[0], bbox[1]], [bbox[2] - bbox[0], bbox[3] - bbox[1]]);
    match (g.units, g.kind) {
        (GradientUnits::Layer, GradientKind::Linear) => (PAINT_LINEAR, [f[0], f[1], t[0], t[1]]),
        (GradientUnits::Layer, GradientKind::Radial) => (PAINT_RADIAL, [f[0], f[1], t[0], t[1]]),
        (GradientUnits::Box, GradientKind::Radial) => {
            let r = (t[0] - f[0]).hypot(t[1] - f[1]);
            (PAINT_ELLIPSE, [m[0] + f[0] * s[0], m[1] + f[1] * s[1], r * s[0].abs(), r * s[1].abs()])
        }
        (GradientUnits::Box, GradientKind::Linear) => {
            // t(l) = (((l − m) / s) − f)·d / |d|² = a·l + b.
            let d = [t[0] - f[0], t[1] - f[1]];
            let dd = d[0] * d[0] + d[1] * d[1];
            if dd < 1e-18 || s[0].abs() < 1e-9 || s[1].abs() < 1e-9 {
                return (PAINT_LINEAR, [0.0; 4]);
            }
            let a = [d[0] / (s[0] * dd), d[1] / (s[1] * dd)];
            let b = -(m[0] * a[0] + m[1] * a[1]) - (f[0] * d[0] + f[1] * d[1]) / dd;
            let aa = a[0] * a[0] + a[1] * a[1];
            let from = [-b * a[0] / aa, -b * a[1] / aa];
            (PAINT_LINEAR, [from[0], from[1], from[0] + a[0] / aa, from[1] + a[1] / aa])
        }
    }
}

/// A field's points as stop pairs (see `PAINT_FIELD`).
fn set_field(prim: &mut Prim, field: &FieldState, stops: &mut Vec<[f32; 4]>) {
    prim.fill = [1.0; 4];
    prim.paint[1] = PAINT_FIELD as f32;
    prim.paint[2] = (stops.len() / 2) as f32;
    prim.paint[3] = field.points.len() as f32;
    for (p, c, r) in &field.points {
        stops.push([p.x() as f32, p.y() as f32, (1.0 / (r * r)) as f32, 0.0]);
        stops.push(premul(*c, 1.0));
    }
    prim.paint2 = [field.warp as f32, field.warp_scale as f32, (2.0 / field.softness) as f32, 0.0];
}
