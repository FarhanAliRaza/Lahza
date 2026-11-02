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
use lahza_motion_core::path::{self, morph, trim};
use lahza_motion_core::space::Proj;
use lahza_motion_core::compile::{Affine, Content, EffectState, FrameState, LayerState, PaintState, PostKind, ShapeState, TextState};
use lahza_motion_core::document::{Blend, Gradient, GradientKind, LineCap, LineJoin, MatteMode, ShapeKind, StrokeAlign, StrokeStyle};
use lahza_motion_core::value::{Color, Vec2};

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

pub const SHAPE_ELLIPSE: u32 = 0;
pub const SHAPE_RECT: u32 = 1;
pub const SHAPE_CAPSULE: u32 = 2;
pub const SHAPE_ARC: u32 = 3;
pub const SHAPE_LINE: u32 = 4;
pub const SHAPE_STAR: u32 = 5;
/// Flattened Bézier path: geom2 = [first segment, count, open (1) / filled (0)].
pub const SHAPE_PATH: u32 = 6;

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
    /// x: stroke width, y: gradient kind (0 none, 1 linear, 2 radial),
    /// z: first stop, w: stop count.
    pub paint: [f32; 4],
    /// Gradient from.xy, to.xy in local units.
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
    /// Output scale (0.5 for half-resolution previews).
    pub scale: f64,
}

impl Builder<'_> {
    pub fn build(&mut self, frame: &FrameState) -> DisplayList {
        let w = ((frame.width as f64 * self.scale).round() as u32).max(1);
        let h = ((frame.height as f64 * self.scale).round() as u32).max(1);
        let root = Proj::from(Affine::scale(self.scale, self.scale));
        let mut list = DisplayList::default();
        let mut pending: Vec<Pending> = Vec::new();
        let mut main_items: Vec<Item> = Vec::new();
        for layer in &frame.layers {
            self.emit(layer, root, (w, h), &mut list, &mut pending, &mut main_items);
        }
        let background = premul(frame.background, 1.0);
        // Groups first (inner before outer), then the main target.
        let mut range_base = 0;
        for Pending { mut group, items, origin, factor } in pending {
            group.target.range_base = range_base;
            let prims: Vec<(Prim, [f64; 4])> = items
                .iter()
                .filter_map(|it| {
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
        let prims: Vec<(Prim, [f64; 4])> = main_items.iter().map(|it| (it.prim, canvas_bounds(it))).collect();
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
        let group = Group { target: Target::new(gw, gh, 0, [0.0; 4]), offset: list.group_texels, sigma: (sigma / ((f[0] + f[1]) / 2.0)) as f32, clamp };
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

    fn emit(&mut self, layer: &LayerState, root: Proj, canvas: (u32, u32), list: &mut DisplayList, pending: &mut Vec<Pending>, out: &mut Vec<Item>) {
        if !layer.is_isolated() {
            out.extend(self.layer_items(layer, root, &mut list.stops, &mut list.segments));
            return;
        }
        let scale = self.scale;
        // The layer's own picture, in canvas space.
        let mut items = Vec::new();
        let group_opacity = match &layer.content {
            Content::Group(children) => {
                let inner = root.then(layer.matrix);
                for c in children {
                    self.emit(c, inner, canvas, list, pending, &mut items);
                }
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
                EffectState::Blur { sigma, repeat_edges } => {
                    blur = (blur * blur + sigma * sigma).sqrt();
                    clamp |= *repeat_edges;
                }
                EffectState::Shadow { offset, sigma, .. } => reach = reach.max(3.0 * sigma + offset.x().abs().max(offset.y().abs())),
                EffectState::Glow { sigma, .. } => reach = reach.max(3.0 * sigma),
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
        if rect[2] > rect[0] && rect[3] > rect[1] {
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
        }
    }

    fn layer_items(&mut self, layer: &LayerState, root: Proj, stops: &mut Vec<[f32; 4]>, segments: &mut Vec<[f32; 4]>) -> Vec<Item> {
        let matrix = root.then(layer.matrix);
        let blend = blend_code(layer.blend);
        let mut items = Vec::new();
        match &layer.content {
            Content::Group(_) => {}
            Content::Shape(s) if s.shape == ShapeKind::Path => {
                items.extend(path_items(s, matrix, layer.opacity, blend, stops, segments));
            }
            Content::Shape(s) => {
                let mut prim = Prim { kind: [PRIM_SHAPE, 0, blend, 0], ..Default::default() };
                let (hw, hh) = (s.size.x() / 2.0, s.size.y() / 2.0);
                let half_t = s.thickness / 2.0;
                let bounds_half;
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
                    }
                    ShapeKind::Path => unreachable!("paths are handled above"),
                    ShapeKind::Polygon | ShapeKind::Star => {
                        prim.kind[1] = SHAPE_STAR;
                        let n = s.sides.max(3) as f64;
                        let k = if s.shape == ShapeKind::Polygon { (std::f64::consts::PI / n).cos() } else { s.inner };
                        prim.geom = [hw as f32, k as f32, n as f32, s.radius.min(hw) as f32];
                        bounds_half = (hw, hw);
                    }
                }
                let stroke_w = s.stroke.map(|(_, w)| w).unwrap_or(0.0);
                match &s.fill {
                    Some(PaintState::Solid(c)) => {
                        prim.kind[3] |= FLAG_FILL;
                        prim.fill = premul(*c, 1.0);
                    }
                    Some(PaintState::Gradient(g)) => {
                        prim.kind[3] |= FLAG_FILL;
                        prim.fill = [1.0; 4];
                        prim.paint[1] = match g.kind {
                            GradientKind::Linear => 1.0,
                            GradientKind::Radial => 2.0,
                        };
                        prim.paint[2] = (stops.len() / 2) as f32;
                        prim.paint[3] = g.stops.len() as f32;
                        let mut sorted = g.stops.clone();
                        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
                        for (offset, color) in sorted {
                            stops.push([offset as f32, 0.0, 0.0, 0.0]);
                            stops.push(premul(color, 1.0));
                        }
                        prim.paint2 = [g.from.x() as f32, g.from.y() as f32, g.to.x() as f32, g.to.y() as f32];
                    }
                    None => {}
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
                    let mut prim = Prim {
                        kind: [PRIM_GLYPH, 0, blend, FLAG_FILL],
                        geom: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                        fill: color,
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
        set_fill(&mut prim, fill, stops);
        add(prim, b, 0.0);
    }
    if let Some((color, width)) = s.stroke {
        let (t0, t1) = (s.trim.x().clamp(0.0, 1.0), s.trim.y().clamp(0.0, 1.0));
        if width > 0.0 && color.0[3] > 0.0 && t1 > t0 {
            let lines = trim(&subpaths, t0, t1);
            if let Some(style) = &s.stroke_style {
                // Caps, joins, dashes and feather: outline the stroke and fill it.
                let outline = stroke_outline(&lines, width, style, tolerance);
                let (first, count, b) = push_segments(&outline, true, segments);
                if count > 0 {
                    let mut prim = Prim { kind: [PRIM_SHAPE, SHAPE_PATH, blend, FLAG_FILL], geom2: [f32::from_bits(first), f32::from_bits(count), 0.0, 0.0], ..Default::default() };
                    let paint = match feather_gradient(&lines, style, color) {
                        Some(g) => PaintState::Gradient(std::sync::Arc::new(g)),
                        None => PaintState::Solid(color),
                    };
                    set_fill(&mut prim, &paint, stops);
                    add(prim, b, 0.0);
                }
                return items;
            }
            let (first, count, b) = push_segments(&lines, false, segments);
            if count > 0 {
                let prim = Prim {
                    kind: [PRIM_SHAPE, SHAPE_PATH, blend, FLAG_STROKE],
                    geom2: [f32::from_bits(first), f32::from_bits(count), 1.0, 0.0],
                    stroke: premul(color, 1.0),
                    paint: [width as f32, 0.0, 0.0, 0.0],
                    ..Default::default()
                };
                add(prim, b, width / 2.0);
            }
        }
    }
    items
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

/// The point `distance` px back from the end of `pts` (or its start when
/// the line is shorter).
fn back_along(pts: &[(f64, f64)], distance: f64) -> (f64, f64) {
    let mut left = distance;
    for w in pts.windows(2).rev() {
        let l = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        if l >= left && l > 0.0 {
            let k = left / l;
            return (w[1].0 + (w[0].0 - w[1].0) * k, w[1].1 + (w[0].1 - w[1].1) * k);
        }
        left -= l;
    }
    pts[0]
}

/// Opacity ramps for `feather` (far end) and `feather_start` (start) as a
/// linear gradient. One ramp follows the stroke's direction where it fades;
/// both together ramp along the line from start to end.
fn feather_gradient(lines: &[Vec<(f64, f64)>], style: &StrokeStyle, color: Color) -> Option<Gradient> {
    let clear = Color([color.0[0], color.0[1], color.0[2], 0.0]);
    let first = lines.first()?;
    let last = lines.last()?;
    let (start, end) = (*first.first()?, *last.last()?);
    let linear = |from: (f64, f64), to: (f64, f64), stops| Gradient { kind: GradientKind::Linear, from: Vec2::new(from.0, from.1), to: Vec2::new(to.0, to.1), stops };
    match (style.feather_start > 0.0, style.feather > 0.0) {
        (false, false) => None,
        (false, true) => {
            let from = back_along(last, style.feather);
            (from != end).then(|| linear(from, end, vec![(0.0, color), (1.0, clear)]))
        }
        (true, false) => {
            let reversed: Vec<(f64, f64)> = first.iter().rev().copied().collect();
            let to = back_along(&reversed, style.feather_start);
            (to != start).then(|| linear(start, to, vec![(0.0, clear), (1.0, color)]))
        }
        (true, true) => {
            let length = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
            (length > 0.0).then(|| {
                let a = (style.feather_start / length).min(0.5);
                let b = (1.0 - style.feather / length).max(a);
                linear(start, end, vec![(0.0, clear), (a, color), (b, color), (1.0, clear)])
            })
        }
    }
}

fn set_fill(prim: &mut Prim, fill: &PaintState, stops: &mut Vec<[f32; 4]>) {
    match fill {
        PaintState::Solid(c) => prim.fill = premul(*c, 1.0),
        PaintState::Gradient(g) => {
            prim.fill = [1.0; 4];
            prim.paint[1] = match g.kind {
                GradientKind::Linear => 1.0,
                GradientKind::Radial => 2.0,
            };
            prim.paint[2] = (stops.len() / 2) as f32;
            prim.paint[3] = g.stops.len() as f32;
            let mut sorted = g.stops.clone();
            sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (offset, color) in sorted {
                stops.push([offset as f32, 0.0, 0.0, 0.0]);
                stops.push(premul(color, 1.0));
            }
            prim.paint2 = [g.from.x() as f32, g.from.y() as f32, g.to.x() as f32, g.to.y() as f32];
        }
    }
}
