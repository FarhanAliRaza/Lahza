//! Compilation: every time reference resolved once, so evaluating a frame
//! is pure arithmetic. [`Compiled::frame`] turns a time into the flat,
//! paint-ordered list of layer states a renderer draws.

use crate::curve::Curve;
use crate::document::*;
use crate::markers::Markers;
use crate::property::{compile as compile_prop, Ctx, Prop, Property, SyncPoint};
use crate::time::{Duration, Fps, Lead, Step, TimeBase};
use crate::timeref::{Anchor, Edge, ElementTimes, TimeRef};
use crate::value::{Animatable, Color, Vec2};
use crate::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// 2D affine transform: (x, y) → (a·x + c·y + e, b·x + d·y + f).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    pub fn translate(x: f64, y: f64) -> Self {
        Affine([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub fn scale(x: f64, y: f64) -> Self {
        Affine([x, 0.0, 0.0, y, 0.0, 0.0])
    }
    /// Clockwise degrees in a y-down space.
    pub fn rotate(degrees: f64) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        Affine([c, s, -s, c, 0.0, 0.0])
    }
    /// self ∘ other: apply `other` first.
    pub fn then(self, other: Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = other.0;
        Affine([
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ])
    }
    pub fn apply(self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }
    pub fn determinant(self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }
    pub fn inverse(self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let [a, b, c, d, e, f] = self.0;
        let inv = 1.0 / det;
        Some(Affine([
            d * inv,
            -b * inv,
            -c * inv,
            a * inv,
            (c * f - d * e) * inv,
            (b * e - a * f) * inv,
        ]))
    }
    /// Largest scale factor (for blur radii and anti-aliasing widths).
    pub fn max_scale(self) -> f64 {
        let sx = (self.0[0] * self.0[0] + self.0[1] * self.0[1]).sqrt();
        let sy = (self.0[2] * self.0[2] + self.0[3] * self.0[3]).sqrt();
        sx.max(sy)
    }
}

// ---------------------------------------------------------------- output

#[derive(Clone, Debug, PartialEq)]
pub struct FrameState {
    pub t: f64,
    pub scene: Option<Arc<str>>,
    pub background: Color,
    pub width: u32,
    pub height: u32,
    pub layers: Vec<LayerState>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayerState {
    pub id: Arc<str>,
    /// Layer space → canvas pixels, camera included.
    pub matrix: Affine,
    pub opacity: f64,
    pub blend: Blend,
    /// Applied to the layer as one picture, in order (canvas pixels).
    pub effects: Vec<EffectState>,
    pub matte: Option<Box<MatteState>>,
    pub content: Content,
}

impl LayerState {
    /// Needs its own offscreen pass (effects, matte or a grouped comp).
    pub fn is_isolated(&self) -> bool {
        !self.effects.is_empty() || self.matte.is_some() || matches!(self.content, Content::Group(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectState {
    Blur { sigma: f64, repeat_edges: bool },
    Shadow { color: Color, offset: Vec2, sigma: f64 },
    Glow { color: Color, sigma: f64, intensity: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatteState {
    pub mode: MatteMode,
    /// The matte layer; `None` when it is not visible at this time.
    pub source: Option<LayerState>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Shape(ShapeState),
    Text(TextState),
    Image { src: Arc<str>, size: Option<Vec2> },
    /// A comp drawn as one picture (AE precomp): children carry absolute
    /// matrices; the group's opacity, blend, effects and matte apply once.
    Group(Vec<LayerState>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeState {
    pub shape: ShapeKind,
    pub size: Vec2,
    pub radius: f64,
    pub thickness: f64,
    pub sides: u32,
    pub inner: f64,
    pub angles: Vec2,
    pub trim: Vec2,
    pub fill: Option<PaintState>,
    pub stroke: Option<(Color, f64)>,
    /// SVG path data for `ShapeKind::Path`.
    pub path: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaintState {
    Solid(Color),
    Gradient(Arc<Gradient>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextState {
    pub text: Arc<str>,
    pub font: Arc<str>,
    pub size: f64,
    pub color: Color,
    pub tracking: f64,
    pub line_height: f64,
    pub align: Align,
    /// Per character (`text.chars()`), empty when no animator is active.
    pub glyphs: Vec<GlyphState>,
    /// Which unit each character's pivot belongs to, per character.
    pub pivots: Arc<[u32]>,
}

/// Offsets applied to one character, about its unit's centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphState {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub rotation: f64,
    pub opacity: f64,
}

impl GlyphState {
    pub const IDENTITY: GlyphState = GlyphState { x: 0.0, y: 0.0, scale: 1.0, rotation: 0.0, opacity: 1.0 };
}

// --------------------------------------------------------- compiled form

pub struct Compiled {
    pub settings: Settings,
    pub base: TimeBase,
    pub scenes: Vec<CScene>,
    /// Every requested sync point, for `check`.
    pub syncs: Vec<SyncRecord>,
    /// Problems found while compiling; a project with errors still
    /// evaluates (bad properties keep their initial value).
    pub errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SyncRecord {
    pub element: String,
    pub what: String,
    pub reference: String,
    pub anchor: Anchor,
    /// Requested timeline time (reference + lead).
    pub requested: f64,
    pub kind: SyncKind,
}

#[derive(Clone, Debug)]
pub enum SyncKind {
    /// A move: content time = timeline time + shift, sampled through `step`.
    Move { curve: Curve, start: f64, dur: f64, step: Option<Step>, shift: f64 },
    /// A cut or layer edge: lands on the first frame at or after it.
    Edge,
}

pub struct CScene {
    pub id: Arc<str>,
    pub start: f64,
    pub end: f64,
    pub pre_roll: f64,
    pub background: Color,
    pub motion_blur: Option<MotionBlur>,
    camera_position: Option<Prop<Vec2>>,
    camera_zoom: Prop<f64>,
    camera_rotation: Prop<f64>,
    transition: Option<CTransition>,
    pub layers: Vec<CLayer>,
}

struct CTransition {
    kind: TransitionKind,
    dur: f64,
    curve: Curve,
    direction: Direction,
}

impl CScene {
    /// Half the incoming transition (the scene shows this much early).
    pub fn lead_in(&self) -> f64 {
        self.transition.as_ref().map(|t| t.dur / 2.0).unwrap_or(0.0)
    }
}

enum CEffect {
    Blur(Prop<f64>, bool),
    Shadow { color: Prop<Color>, offset: Prop<Vec2>, radius: Prop<f64> },
    Glow { color: Prop<Color>, radius: Prop<f64>, intensity: Prop<f64> },
}

pub struct CLayer {
    pub id: Arc<str>,
    pub start: f64,
    pub end: f64,
    /// `None` inherits; `Some(fps 0)` turns stepping off.
    step: Option<Step>,
    offset: f64,
    position: Prop<Vec2>,
    anchor: Prop<Vec2>,
    scale: Prop<Vec2>,
    rotation: Prop<f64>,
    opacity: Prop<f64>,
    blend: Blend,
    effects: Vec<CEffect>,
    matte: Option<(usize, MatteMode)>,
    /// Used as another layer's matte: not drawn by itself.
    is_matte: bool,
    parent: Option<usize>,
    hidden: bool,
    kind: CKind,
}

impl CLayer {
    pub fn children(&self) -> &[CLayer] {
        match &self.kind {
            CKind::Comp(children) => children,
            _ => &[],
        }
    }
}

/// Element references against compiled windows (for times given to tools).
impl ElementTimes for &Compiled {
    fn element_time(&mut self, id: &str, edge: Edge) -> Result<f64> {
        let (s, e) = self.element_window(id).ok_or_else(|| crate::Error(format!("no layer or scene with id '{id}'")))?;
        Ok(if edge == Edge::End { e } else { s })
    }
}

enum CKind {
    Shape(CShape),
    Text(CText),
    Image { src: Arc<str>, size: Option<Prop<Vec2>> },
    Comp(Vec<CLayer>),
}

struct CShape {
    shape: ShapeKind,
    size: Prop<Vec2>,
    radius: Prop<f64>,
    thickness: Prop<f64>,
    sides: u32,
    inner: Prop<f64>,
    angles: Prop<Vec2>,
    trim: Prop<Vec2>,
    fill: Option<CPaint>,
    stroke: Option<(Prop<Color>, Prop<f64>)>,
    path: Option<Arc<str>>,
}

enum CPaint {
    Solid(Prop<Color>),
    Gradient(Arc<Gradient>),
}

struct CText {
    text: Arc<str>,
    font: Arc<str>,
    size: Prop<f64>,
    color: Prop<Color>,
    tracking: Prop<f64>,
    line_height: f64,
    align: Align,
    animators: Vec<CAnim>,
    /// Per character: unit index for glyph, word and line (u32::MAX = none).
    units: Vec<[u32; 3]>,
    pivots: Arc<[u32]>,
}

struct CAnim {
    unit: usize,
    /// Per unit: its rank in the animation order.
    ranks: Vec<f64>,
    start: f64,
    stagger: f64,
    dur: f64,
    curve: Curve,
    from: [f64; 5],
    to: [f64; 5],
    range: (usize, usize),
}

// -------------------------------------------------- element time windows

enum Elem<'a> {
    Scene(usize),
    Layer { layer: &'a Layer, parent: Parent<'a> },
}

#[derive(Clone, Copy)]
enum Parent<'a> {
    Scene(usize),
    Layer(&'a Layer),
}

struct Windows<'a> {
    project: &'a Project,
    base: TimeBase,
    elems: HashMap<&'a str, Elem<'a>>,
    memo: HashMap<(String, bool), f64>,
    visiting: HashSet<(String, bool)>,
}

impl<'a> Windows<'a> {
    fn new(project: &'a Project, base: TimeBase, errors: &mut Vec<String>) -> Self {
        let mut elems = HashMap::new();
        for (i, scene) in project.scenes.iter().enumerate() {
            if elems.insert(scene.id.as_str(), Elem::Scene(i)).is_some() {
                errors.push(format!("duplicate id '{}'", scene.id));
            }
            fn add<'a>(layers: &'a [Layer], parent: Parent<'a>, elems: &mut HashMap<&'a str, Elem<'a>>, errors: &mut Vec<String>) {
                for layer in layers {
                    if elems.insert(layer.id.as_str(), Elem::Layer { layer, parent }).is_some() {
                        errors.push(format!("duplicate id '{}'", layer.id));
                    }
                    add(layer.children(), Parent::Layer(layer), elems, errors);
                }
            }
            add(&scene.layers, Parent::Scene(i), &mut elems, errors);
        }
        Self { project, base, elems, memo: HashMap::new(), visiting: HashSet::new() }
    }

    fn resolve(&mut self, r: &TimeRef) -> Result<f64> {
        let markers: &Markers = &self.project.markers;
        let base = self.base;
        r.resolve(markers, &base, self)
    }

    fn scene_time(&mut self, i: usize, end: bool) -> Result<f64> {
        let scene = &self.project.scenes[i];
        if !end {
            let mut t = self.resolve(&scene.start)?;
            let lead = match scene.lead {
                Some(Lead(d)) => d.seconds(&self.base)?,
                None if scene.start.is_marker() => -self.base.fps.frame(),
                None => 0.0,
            };
            t += lead;
            return Ok(t);
        }
        if let Some(end) = &scene.end {
            return self.resolve(end);
        }
        if let Some(d) = scene.duration {
            return Ok(self.scene_time(i, false)? + d.seconds(&self.base)?);
        }
        if i + 1 < self.project.scenes.len() {
            return self.scene_time(i + 1, false);
        }
        Ok(self.project.settings.duration)
    }

    fn parent_time(&mut self, parent: Parent<'a>, end: bool) -> Result<f64> {
        match parent {
            Parent::Scene(i) => self.window(&self.project.scenes[i].id.clone(), end),
            Parent::Layer(l) => self.window(&l.id.clone(), end),
        }
    }

    fn window(&mut self, id: &str, end: bool) -> Result<f64> {
        let key = (id.to_string(), end);
        if let Some(t) = self.memo.get(&key) {
            return Ok(*t);
        }
        if !self.visiting.insert(key.clone()) {
            crate::bail!("circular time reference through #{id}");
        }
        let result = match self.elems.get(id) {
            None => Err(crate::Error(format!("no layer or scene with id '{id}'"))),
            Some(Elem::Scene(i)) => {
                let i = *i;
                self.scene_time(i, end)
            }
            Some(Elem::Layer { layer, parent }) => {
                let (layer, parent) = (*layer, *parent);
                let time = &layer.common.time;
                if !end {
                    match &time.start {
                        Some(r) => self.resolve(r),
                        None => self.parent_time(parent, false),
                    }
                } else if let Some(r) = &time.end {
                    self.resolve(r)
                } else if let Some(d) = time.duration {
                    let start = self.window(id, false)?;
                    Ok(start + d.seconds(&self.base)?)
                } else {
                    self.parent_time(parent, true)
                }
            }
        };
        self.visiting.remove(&key);
        let t = result?;
        self.memo.insert(key, t);
        Ok(t)
    }
}

impl ElementTimes for Windows<'_> {
    fn element_time(&mut self, id: &str, edge: Edge) -> Result<f64> {
        self.window(id, edge == Edge::End)
    }
}

// --------------------------------------------------------------- compile

struct Compiler<'a> {
    windows: Windows<'a>,
    errors: Vec<String>,
    syncs: Vec<SyncRecord>,
}

impl<'a> Compiler<'a> {
    fn prop<T: Animatable>(&mut self, p: &Property<T>, element: &str, what: &str, origin: f64, step: Option<Step>, shift: f64) -> Prop<T> {
        let markers = &self.windows.project.markers;
        let base = self.windows.base;
        let mut ctx = Ctx { markers, base, elements: &mut self.windows, origin };
        match compile_prop(p, &mut ctx) {
            Ok(prop) => {
                for seg in &prop.segments {
                    if let Some(SyncPoint { reference, anchor, requested, start, dur }) = &seg.sync {
                        self.syncs.push(SyncRecord {
                            element: element.into(),
                            what: what.into(),
                            reference: reference.clone(),
                            anchor: *anchor,
                            requested: requested - shift,
                            kind: SyncKind::Move { curve: seg.curve.clone(), start: *start, dur: *dur, step: prop.step.or(step), shift },
                        });
                    }
                }
                prop
            }
            Err(e) => {
                self.errors.push(format!("{element}.{what}: {e}"));
                Prop::constant(p.initial())
            }
        }
    }

    fn time(&mut self, r: &TimeRef, element: &str, what: &str) -> Option<f64> {
        match self.windows.resolve(r) {
            Ok(t) => Some(t),
            Err(e) => {
                self.errors.push(format!("{element}.{what}: {e}"));
                None
            }
        }
    }

    fn seconds(&mut self, d: Duration, element: &str, what: &str) -> f64 {
        match d.seconds(&self.windows.base) {
            Ok(s) => s,
            Err(e) => {
                self.errors.push(format!("{element}.{what}: {e}"));
                0.0
            }
        }
    }

    fn layers(&mut self, layers: &[Layer], inherited: Option<Step>, shift: f64) -> Vec<CLayer> {
        let ids: HashMap<&str, usize> = layers.iter().enumerate().map(|(i, l)| (l.id.as_str(), i)).collect();
        let mut out = Vec::with_capacity(layers.len());
        for layer in layers {
            out.push(self.layer(layer, inherited, shift, &ids));
        }
        let sources: Vec<usize> = out.iter().filter_map(|l| l.matte.map(|m| m.0)).collect();
        for i in sources {
            out[i].is_matte = true;
        }
        // Reject parent cycles.
        for i in 0..out.len() {
            let mut seen = HashSet::new();
            let mut cur = Some(i);
            while let Some(c) = cur {
                if !seen.insert(c) {
                    self.errors.push(format!("{}: parent chain forms a cycle", out[i].id));
                    out[i].parent = None;
                    break;
                }
                cur = out[c].parent;
            }
        }
        out
    }

    fn layer(&mut self, layer: &Layer, inherited: Option<Step>, shift: f64, siblings: &HashMap<&str, usize>) -> CLayer {
        let id = layer.id.as_str();
        let c = &layer.common;
        let start = self.windows.window(id, false).unwrap_or_else(|e| {
            self.errors.push(format!("{id}.time.start: {e}"));
            0.0
        });
        let end = self.windows.window(id, true).unwrap_or_else(|e| {
            self.errors.push(format!("{id}.time.end: {e}"));
            f64::INFINITY
        });
        if let Some(r) = &c.time.start {
            if r.is_marker() {
                self.syncs.push(SyncRecord {
                    element: id.into(),
                    what: "time.start".into(),
                    reference: r.text(),
                    anchor: Anchor::Start,
                    requested: start - shift,
                    kind: SyncKind::Edge,
                });
            }
        }
        let step = match c.time.step_fps {
            Some(fps) => Some(Step { fps, phase: c.time.step_phase.unwrap_or(0.0) }),
            None => inherited,
        };
        let offset = c.time.offset.map(|d| self.seconds(d, id, "time.offset")).unwrap_or(0.0);
        let t = &c.transform;
        let position = self.prop(&t.position, id, "transform.position", start, step, shift);
        let anchor = self.prop(&t.anchor, id, "transform.anchor", start, step, shift);
        let scale = self.prop(&t.scale, id, "transform.scale", start, step, shift);
        let rotation = self.prop(&t.rotation, id, "transform.rotation", start, step, shift);
        let opacity = self.prop(&t.opacity, id, "transform.opacity", start, step, shift);
        let mut effects = Vec::new();
        for (i, e) in c.effects.iter().enumerate() {
            let what = |f: &str| format!("effects.{i}.{f}");
            effects.push(match e {
                Effect::Blur { radius, repeat_edges } => CEffect::Blur(self.prop(radius, id, &what("radius"), start, step, shift), *repeat_edges),
                Effect::Shadow { color, offset, radius } => CEffect::Shadow {
                    color: self.prop(color, id, &what("color"), start, step, shift),
                    offset: self.prop(offset, id, &what("offset"), start, step, shift),
                    radius: self.prop(radius, id, &what("radius"), start, step, shift),
                },
                Effect::Glow { color, radius, intensity } => CEffect::Glow {
                    color: self.prop(color, id, &what("color"), start, step, shift),
                    radius: self.prop(radius, id, &what("radius"), start, step, shift),
                    intensity: self.prop(intensity, id, &what("intensity"), start, step, shift),
                },
            });
        }
        let matte = c.matte.as_ref().and_then(|m| match siblings.get(m.layer.as_str()) {
            Some(i) if m.layer != id => Some((*i, m.mode)),
            _ => {
                self.errors.push(format!("{id}.matte: no sibling layer '{}'", m.layer));
                None
            }
        });
        let parent = c.parent.as_ref().and_then(|p| match siblings.get(p.as_str()) {
            Some(i) => Some(*i),
            None => {
                self.errors.push(format!("{id}.parent: no sibling layer '{p}'"));
                None
            }
        });
        let kind = match &layer.kind {
            LayerKind::Shape(s) => {
                let fill = s.fill.as_ref().map(|f| match f {
                    Paint::Solid(p) => CPaint::Solid(self.prop(p, id, "fill", start, step, shift)),
                    Paint::Gradient(g) => CPaint::Gradient(Arc::new(g.clone())),
                });
                let stroke = s.stroke.as_ref().map(|st| {
                    (
                        self.prop(&st.color, id, "stroke.color", start, step, shift),
                        self.prop(&st.width, id, "stroke.width", start, step, shift),
                    )
                });
                CKind::Shape(CShape {
                    shape: s.shape,
                    size: self.prop(&s.size, id, "size", start, step, shift),
                    radius: self.prop(&s.radius, id, "radius", start, step, shift),
                    thickness: self.prop(&s.thickness, id, "thickness", start, step, shift),
                    sides: s.sides.clamp(3, 64),
                    inner: self.prop(&s.inner, id, "inner", start, step, shift),
                    angles: self.prop(&s.angles, id, "angles", start, step, shift),
                    trim: self.prop(&s.trim, id, "trim", start, step, shift),
                    fill,
                    stroke,
                    path: match (s.shape, &s.d) {
                        (ShapeKind::Path, Some(d)) => Some(d.as_str().into()),
                        (ShapeKind::Path, None) => {
                            self.errors.push(format!("{id}: shape \"path\" needs \"d\" (SVG path data)"));
                            None
                        }
                        _ => None,
                    },
                })
            }
            LayerKind::Text(tx) => CKind::Text(self.text(tx, id, start, step, shift)),
            LayerKind::Image(img) => CKind::Image {
                src: img.src.as_str().into(),
                size: img.size.as_ref().map(|s| self.prop(s, id, "size", start, step, shift)),
            },
            LayerKind::Comp(comp) => CKind::Comp(self.layers(&comp.layers, step, shift - offset)),
        };
        CLayer {
            id: id.into(),
            start,
            end,
            step,
            offset,
            position,
            anchor,
            scale,
            rotation,
            opacity,
            blend: c.blend,
            effects,
            matte,
            is_matte: false,
            parent,
            hidden: c.hidden,
            kind,
        }
    }

    fn text(&mut self, tx: &TextLayer, id: &str, start: f64, step: Option<Step>, shift: f64) -> CText {
        // Units per character: glyphs skip whitespace; words split on
        // whitespace; lines on newlines.
        let mut units = Vec::new();
        let (mut g, mut w, mut l) = (0u32, 0u32, 0u32);
        let mut in_word = false;
        for ch in tx.text.chars() {
            if ch == '\n' {
                if in_word {
                    w += 1;
                }
                in_word = false;
                units.push([u32::MAX, u32::MAX, l]);
                l += 1;
                continue;
            }
            if ch.is_whitespace() {
                if in_word {
                    w += 1;
                }
                in_word = false;
                units.push([u32::MAX, u32::MAX, l]);
                continue;
            }
            in_word = true;
            units.push([g, w, l]);
            g += 1;
        }
        let counts = [g as usize, (w + in_word as u32) as usize, (l + 1) as usize];
        let mut animators = Vec::new();
        let mut pivot_unit = 0usize;
        for (i, a) in tx.animators.iter().enumerate() {
            let what = format!("animators.{i}");
            let unit = a.unit as usize;
            pivot_unit = pivot_unit.max(unit);
            let n = counts[unit];
            let ease_curve = match Curve::compile(&a.ease, &self.windows.base) {
                Ok(c) => c,
                Err(e) => {
                    self.errors.push(format!("{id}.{what}.ease: {e}"));
                    Curve::Linear
                }
            };
            let dur = match a.dur {
                Some(d) => self.seconds(d, id, &what),
                None => a.ease.natural_duration(self.windows.base.fps),
            };
            let stagger = self.seconds(a.stagger, id, &format!("{what}.stagger"));
            let lead = a.lead.map(|Lead(d)| self.seconds(d, id, &format!("{what}.lead"))).unwrap_or(0.0);
            let Some(at) = self.time(&a.at, id, &format!("{what}.at")) else { continue };
            let requested = at + lead;
            let first = requested - ease_curve.anchor_time(a.anchor, dur);
            let (lo, hi) = match a.range {
                Some([lo, hi]) => (lo.max(1) - 1, hi.min(n)),
                None => (0, n),
            };
            let span = hi.saturating_sub(lo);
            let order: Vec<f64> = (0..n)
                .map(|u| {
                    if u < lo || u >= hi {
                        return 0.0;
                    }
                    let k = (u - lo) as f64;
                    let last = span.saturating_sub(1) as f64;
                    match a.order {
                        Order::Forward => k,
                        Order::Reverse => last - k,
                        Order::Center => (k - last / 2.0).abs().floor(),
                        Order::Edges => (last / 2.0).floor() - (k - last / 2.0).abs().floor(),
                        Order::Random => 0.0,
                    }
                })
                .collect();
            let ranks = if a.order == Order::Random {
                let mut idx: Vec<usize> = (lo..hi).collect();
                let mut state = (a.seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
                for i in (1..idx.len()).rev() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    idx.swap(i, (state % (i as u64 + 1)) as usize);
                }
                let mut ranks = vec![0.0; n];
                for (rank, u) in idx.into_iter().enumerate() {
                    ranks[u] = rank as f64;
                }
                ranks
            } else {
                order
            };
            let state = |s: &UnitState| {
                [s.x.unwrap_or(0.0), s.y.unwrap_or(0.0), s.scale.unwrap_or(1.0), s.rotation.unwrap_or(0.0), s.opacity.unwrap_or(1.0)]
            };
            if a.at.is_marker() || matches!(a.at, TimeRef::Expr(_)) {
                self.syncs.push(SyncRecord {
                    element: id.into(),
                    what,
                    reference: a.at.text(),
                    anchor: a.anchor,
                    requested: requested - shift,
                    kind: SyncKind::Move { curve: ease_curve.clone(), start: first, dur, step, shift },
                });
            }
            animators.push(CAnim {
                unit,
                ranks,
                start: first,
                stagger,
                dur,
                curve: ease_curve,
                from: state(&a.from),
                to: state(&a.to),
                range: (lo, hi),
            });
        }
        let pivots: Arc<[u32]> = units.iter().map(|u| u[pivot_unit]).collect();
        CText {
            text: tx.text.as_str().into(),
            font: tx.font.as_str().into(),
            size: self.prop(&tx.size, id, "size", start, step, shift),
            color: self.prop(&tx.color, id, "color", start, step, shift),
            tracking: self.prop(&tx.tracking, id, "tracking", start, step, shift),
            line_height: tx.line_height,
            align: tx.align,
            animators,
            units,
            pivots,
        }
    }
}

impl Compiled {
    pub fn new(project: &Project) -> Compiled {
        let mut base = TimeBase::new(project.settings.fps);
        base.beat = project.markers.get("beat").and_then(|t| t.median_interval());
        base.bar = project.markers.get("bar").and_then(|t| t.median_interval());
        let mut errors = Vec::new();
        if project.settings.width == 0 || project.settings.height == 0 || project.settings.width > 8192 || project.settings.height > 8192 {
            errors.push("settings: width and height must be 1..8192".into());
        }
        if !(project.settings.duration > 0.0) {
            errors.push("settings.duration must be positive".into());
        }
        let windows = Windows::new(project, base, &mut errors);
        let mut c = Compiler { windows, errors, syncs: Vec::new() };
        let mut scenes = Vec::new();
        for scene in &project.scenes {
            let id = scene.id.as_str();
            if !valid_id(id) {
                c.errors.push(format!("bad scene id '{id}'"));
            }
            let start = c.windows.window(id, false).unwrap_or_else(|e| {
                c.errors.push(format!("{id}.start: {e}"));
                0.0
            });
            let end = c.windows.window(id, true).unwrap_or_else(|e| {
                c.errors.push(format!("{id}.end: {e}"));
                start
            });
            if end <= start {
                c.errors.push(format!("{id}: scene ends ({end:.3}s) before it starts ({start:.3}s)"));
            }
            if scene.start.is_marker() {
                c.syncs.push(SyncRecord {
                    element: id.into(),
                    what: "cut".into(),
                    reference: scene.start.text(),
                    anchor: Anchor::Start,
                    requested: start,
                    kind: SyncKind::Edge,
                });
            }
            let pre_roll = scene.pre_roll.map(|d| c.seconds(d, id, "pre_roll")).unwrap_or(0.0);
            let step = scene.step_fps.map(|fps| Step { fps, phase: 0.0 });
            // Content time = timeline time + pre_roll.
            let camera_position = scene.camera.position.as_ref().map(|p| c.prop(p, id, "camera.position", start, None, pre_roll));
            let camera_zoom = c.prop(&scene.camera.zoom, id, "camera.zoom", start, None, pre_roll);
            let camera_rotation = c.prop(&scene.camera.rotation, id, "camera.rotation", start, None, pre_roll);
            let layers = c.layers(&scene.layers, step, pre_roll);
            let transition = scene.transition.as_ref().map(|t| CTransition {
                kind: t.kind,
                dur: t.dur.seconds(&c.windows.base).unwrap_or(0.0).max(0.0),
                curve: Curve::compile(&t.ease, &c.windows.base).unwrap_or(Curve::Linear),
                direction: t.direction,
            });
            scenes.push(CScene {
                id: id.into(),
                start,
                end,
                pre_roll,
                background: scene.background.unwrap_or(project.settings.background),
                motion_blur: scene.motion_blur.or(project.settings.motion_blur),
                camera_position,
                camera_zoom,
                camera_rotation,
                transition,
                layers,
            });
        }
        let Compiler { errors, syncs, .. } = c;
        Compiled { settings: project.settings.clone(), base, scenes, syncs, errors }
    }

    pub fn fps(&self) -> Fps {
        self.settings.fps
    }

    /// Resolved [start, end) of a scene or layer by id.
    pub fn element_window(&self, id: &str) -> Option<(f64, f64)> {
        fn find(layers: &[CLayer], id: &str) -> Option<(f64, f64)> {
            layers.iter().find_map(|l| if &*l.id == id { Some((l.start, l.end)) } else { find(l.children(), id) })
        }
        self.scenes
            .iter()
            .find_map(|s| if &*s.id == id { Some((s.start, s.end)) } else { find(&s.layers, id) })
    }

    pub fn frame_count(&self) -> u64 {
        self.settings.fps.frame_count(self.settings.duration)
    }

    /// The scene shown at timeline time `t` (the last one that covers it).
    pub fn scene_at(&self, t: f64) -> Option<&CScene> {
        self.scenes.iter().rev().find(|s| s.start <= t + 1e-9 && t < s.end - 1e-9)
    }

    /// Motion-blur settings in effect at `t`.
    pub fn motion_blur_at(&self, t: f64) -> Option<MotionBlur> {
        self.scene_at(t)
            .and_then(|s| s.motion_blur)
            .filter(|m| m.shutter > 0.0 && m.samples > 1)
    }

    /// Everything visible at timeline time `t`, in paint order.
    pub fn frame(&self, t: f64) -> FrameState {
        let mut frame = FrameState {
            t,
            scene: None,
            background: self.settings.background,
            width: self.settings.width,
            height: self.settings.height,
            layers: Vec::new(),
        };
        // A transition centred on scene j's cut shows j−1 and j together.
        for j in (1..self.scenes.len()).rev() {
            let b = &self.scenes[j];
            let Some(tr) = b.transition.as_ref().filter(|tr| tr.dur > 0.0) else { continue };
            let from = b.start - tr.dur / 2.0;
            if t < from || t >= b.start + tr.dur / 2.0 {
                continue;
            }
            let a = &self.scenes[j - 1];
            let p = tr.curve.progress(t - from, tr.dur);
            // The outgoing scene holds its last moment, the incoming its first.
            let ta = t.min(a.end - 1e-6).max(a.start);
            let tb = t.max(b.start);
            frame.scene = Some(b.id.clone());
            frame.background = a.background;
            frame.layers = self.transition(a, b, tr, p, ta, tb);
            return frame;
        }
        let Some(scene) = self.scene_at(t) else { return frame };
        frame.scene = Some(scene.id.clone());
        frame.background = scene.background;
        self.scene_layers(scene, t, &mut frame.layers);
        frame
    }

    fn camera(&self, scene: &CScene, tc: f64) -> Affine {
        let (w, h) = (self.settings.width as f64, self.settings.height as f64);
        let zoom = scene.camera_zoom.at(tc);
        let center = scene.camera_position.as_ref().map(|p| p.at(tc)).unwrap_or(Vec2::new(w / 2.0, h / 2.0));
        Affine::translate(w / 2.0, h / 2.0)
            .then(Affine::scale(zoom, zoom))
            .then(Affine::rotate(-scene.camera_rotation.at(tc)))
            .then(Affine::translate(-center.x(), -center.y()))
    }

    fn scene_layers(&self, scene: &CScene, t: f64, out: &mut Vec<LayerState>) {
        let tc = t + scene.pre_roll;
        emit(&scene.layers, tc, None, self.camera(scene, tc), 1.0, out);
    }

    /// A whole scene as one group with its background, for transitions.
    fn scene_group(&self, scene: &CScene, t: f64, matrix: Affine, opacity: f64) -> LayerState {
        let (w, h) = (self.settings.width as f64, self.settings.height as f64);
        let mut layers = vec![canvas_rect(&format!("{}-background", scene.id), w, h, scene.background)];
        self.scene_layers(scene, t, &mut layers);
        LayerState { id: scene.id.clone(), matrix, opacity, blend: Blend::Normal, effects: Vec::new(), matte: None, content: Content::Group(layers) }
    }

    fn transition(&self, a: &CScene, b: &CScene, tr: &CTransition, p: f64, ta: f64, tb: f64) -> Vec<LayerState> {
        let (w, h) = (self.settings.width as f64, self.settings.height as f64);
        let (dx, dy) = match tr.direction {
            Direction::Left => (-w, 0.0),
            Direction::Right => (w, 0.0),
            Direction::Up => (0.0, -h),
            Direction::Down => (0.0, h),
        };
        // Whole-pixel offsets: the two scenes' edges meet exactly, with no
        // partially covered seam for the background to show through.
        let shift = |k: f64| Affine::translate((dx * k).round(), (dy * k).round());
        // The incoming scene sits exactly one canvas behind the outgoing
        // one (rounding each separately could open a one-pixel gap).
        let behind = |k: f64| Affine::translate((dx * k).round() - dx, (dy * k).round() - dy);
        let about_centre = |k: f64| Affine::translate(w / 2.0, h / 2.0).then(Affine::scale(k, k)).then(Affine::translate(-w / 2.0, -h / 2.0));
        let id = Affine::IDENTITY;
        match tr.kind {
            TransitionKind::Fade => vec![self.scene_group(a, ta, id, 1.0), self.scene_group(b, tb, id, p)],
            TransitionKind::Push => vec![
                self.scene_group(a, ta, shift(p), 1.0),
                self.scene_group(b, tb, behind(p), 1.0),
            ],
            TransitionKind::Slide => vec![
                self.scene_group(a, ta, id, 1.0),
                self.scene_group(b, tb, shift(p - 1.0), 1.0),
            ],
            TransitionKind::Zoom => vec![
                self.scene_group(a, ta, about_centre(4f64.powf(p)), (1.0 - p).clamp(0.0, 1.0)),
                self.scene_group(b, tb, about_centre(0.5f64.powf(1.0 - p)), p.clamp(0.0, 1.0)),
            ],
            TransitionKind::Whip => {
                let sigma = 120.0 * p * (1.0 - p) * (w / 1920.0);
                let mut ga = self.scene_group(a, ta, shift(p), 1.0);
                let mut gb = self.scene_group(b, tb, behind(p), 1.0);
                if sigma > 0.3 {
                    ga.effects.push(EffectState::Blur { sigma, repeat_edges: true });
                    gb.effects.push(EffectState::Blur { sigma, repeat_edges: true });
                }
                vec![ga, gb]
            }
            TransitionKind::Wipe => {
                // The revealed part grows from the edge opposite `direction`.
                let (rw, rh, cx, cy) = match tr.direction {
                    Direction::Left => (w * p, h, w * (1.0 - p / 2.0), h / 2.0),
                    Direction::Right => (w * p, h, w * p / 2.0, h / 2.0),
                    Direction::Up => (w, h * p, w / 2.0, h * (1.0 - p / 2.0)),
                    Direction::Down => (w, h * p, w / 2.0, h * p / 2.0),
                };
                let mut gb = self.scene_group(b, tb, id, 1.0);
                let mut mask = canvas_rect(&format!("{}-wipe", b.id), rw.max(0.0), rh.max(0.0), Color::WHITE);
                mask.matrix = Affine::translate(cx, cy);
                gb.matte = Some(Box::new(MatteState { mode: MatteMode::Alpha, source: Some(mask) }));
                vec![self.scene_group(a, ta, id, 1.0), gb]
            }
        }
    }
}

/// A solid rectangle covering w × h, centred on the canvas.
fn canvas_rect(id: &str, w: f64, h: f64, color: Color) -> LayerState {
    LayerState {
        id: id.into(),
        matrix: Affine::translate(w / 2.0, h / 2.0),
        opacity: 1.0,
        blend: Blend::Normal,
        effects: Vec::new(),
        matte: None,
        content: Content::Shape(ShapeState {
            shape: ShapeKind::Rect,
            size: Vec2::new(w, h),
            radius: 0.0,
            thickness: 0.0,
            sides: 4,
            inner: 0.5,
            angles: Vec2::ZERO,
            trim: Vec2::new(0.0, 1.0),
            fill: Some(PaintState::Solid(color)),
            stroke: None,
            path: None,
        }),
    }
}

fn layer_matrix(layers: &[CLayer], i: usize, tc: f64, inherited: Option<Step>, cache: &mut [Option<Affine>]) -> Affine {
    if let Some(m) = cache[i] {
        return m;
    }
    let l = &layers[i];
    let t = clock(l.step.or(inherited), tc);
    let p = l.position.at(t);
    let s = l.scale.at(t);
    let a = l.anchor.at(t);
    let local = Affine::translate(p.x(), p.y())
        .then(Affine::rotate(l.rotation.at(t)))
        .then(Affine::scale(s.x(), s.y()))
        .then(Affine::translate(-a.x(), -a.y()));
    let m = match l.parent {
        Some(pi) => layer_matrix(layers, pi, tc, inherited, cache).then(local),
        None => local,
    };
    cache[i] = Some(m);
    m
}

fn clock(step: Option<Step>, t: f64) -> f64 {
    match step {
        Some(s) => s.quantize(t),
        None => t,
    }
}

fn emit(layers: &[CLayer], tc: f64, inherited: Option<Step>, parent: Affine, parent_opacity: f64, out: &mut Vec<LayerState>) {
    let mut cache = vec![None; layers.len()];
    for i in 0..layers.len() {
        if layers[i].is_matte {
            continue;
        }
        layer(layers, i, tc, inherited, parent, parent_opacity, &mut cache, false, out);
    }
}

/// Emits layer `i`: flattened comps push their children into `out`;
/// everything else pushes one state. `isolate` forces comps into a group
/// (matte sources).
#[allow(clippy::too_many_arguments)]
fn layer(layers: &[CLayer], i: usize, tc: f64, inherited: Option<Step>, parent: Affine, parent_opacity: f64, cache: &mut [Option<Affine>], isolate: bool, out: &mut Vec<LayerState>) {
    let l = &layers[i];
    if l.hidden || tc < l.start - 1e-9 || tc >= l.end - 1e-9 {
        return;
    }
    let step = l.step.or(inherited);
    let t = clock(step, tc);
    let own_opacity = l.opacity.at(t).clamp(0.0, 1.0);
    if own_opacity * parent_opacity <= 0.0 {
        return;
    }
    let matrix = parent.then(layer_matrix(layers, i, tc, inherited, cache));
    let scale = matrix.max_scale();
    let effects: Vec<EffectState> = l
        .effects
        .iter()
        .filter_map(|e| match e {
            CEffect::Blur(r, repeat_edges) => {
                let sigma = r.at(t).max(0.0) * scale;
                (sigma > 0.05).then_some(EffectState::Blur { sigma, repeat_edges: *repeat_edges })
            }
            CEffect::Shadow { color, offset, radius } => {
                let o = offset.at(t);
                // Offsets follow the layer's scale, not its rotation (AE).
                Some(EffectState::Shadow { color: clamp_color(color.at(t)), offset: Vec2::new(o.x() * scale, o.y() * scale), sigma: radius.at(t).max(0.0) * scale })
            }
            CEffect::Glow { color, radius, intensity } => {
                Some(EffectState::Glow { color: clamp_color(color.at(t)), sigma: radius.at(t).max(0.0) * scale, intensity: intensity.at(t).max(0.0) })
            }
        })
        .collect();
    let matte = l.matte.map(|(mi, mode)| {
        let mut src = Vec::new();
        layer(layers, mi, tc, inherited, parent, 1.0, cache, true, &mut src);
        Box::new(MatteState { mode, source: src.pop() })
    });
    let content = match &l.kind {
        CKind::Comp(children) => {
            let ct = tc - l.offset;
            let group = isolate || !effects.is_empty() || matte.is_some() || l.blend != Blend::Normal || own_opacity < 1.0 && children.len() > 1;
            if !group {
                emit(children, ct, step, matrix, parent_opacity * own_opacity, out);
                return;
            }
            let mut inner = Vec::new();
            emit(children, ct, step, matrix, 1.0, &mut inner);
            out.push(LayerState {
                id: l.id.clone(),
                matrix: Affine::IDENTITY,
                opacity: parent_opacity * own_opacity,
                blend: l.blend,
                effects,
                matte,
                content: Content::Group(inner),
            });
            return;
        }
        CKind::Shape(s) => Content::Shape(ShapeState {
            shape: s.shape,
            size: s.size.at(t),
            radius: s.radius.at(t).max(0.0),
            thickness: s.thickness.at(t).max(0.0),
            sides: s.sides,
            inner: s.inner.at(t).clamp(0.0, 1.0),
            angles: s.angles.at(t),
            trim: s.trim.at(t),
            fill: s.fill.as_ref().map(|f| match f {
                CPaint::Solid(p) => PaintState::Solid(clamp_color(p.at(t))),
                CPaint::Gradient(g) => PaintState::Gradient(g.clone()),
            }),
            stroke: s.stroke.as_ref().map(|(c, w)| (clamp_color(c.at(t)), w.at(t).max(0.0))),
            path: s.path.clone(),
        }),
        CKind::Text(tx) => Content::Text(text_state(tx, t)),
        CKind::Image { src, size } => Content::Image { src: src.clone(), size: size.as_ref().map(|s| s.at(t)) },
    };
    out.push(LayerState { id: l.id.clone(), matrix, opacity: parent_opacity * own_opacity, blend: l.blend, effects, matte, content });
}

fn clamp_color(c: Color) -> Color {
    Color::from_fn(|i| c.0[i].clamp(0.0, 1.0))
}

fn text_state(tx: &CText, t: f64) -> TextState {
    let mut glyphs = Vec::new();
    if !tx.animators.is_empty() {
        glyphs = vec![GlyphState::IDENTITY; tx.units.len()];
        for a in &tx.animators {
            for (ci, units) in tx.units.iter().enumerate() {
                let u = units[a.unit];
                if u == u32::MAX {
                    continue;
                }
                let u = u as usize;
                if u < a.range.0 || u >= a.range.1 {
                    continue;
                }
                let start = a.start + a.ranks[u] * a.stagger;
                let p = a.curve.progress(t - start, a.dur);
                let v: [f64; 5] = std::array::from_fn(|k| a.from[k] + (a.to[k] - a.from[k]) * p);
                let g = &mut glyphs[ci];
                g.x += v[0];
                g.y += v[1];
                g.scale *= v[2];
                g.rotation += v[3];
                g.opacity *= v[4].clamp(0.0, 1.0);
            }
        }
    }
    TextState {
        text: tx.text.clone(),
        font: tx.font.clone(),
        size: tx.size.at(t).max(0.0),
        color: clamp_color(tx.color.at(t)),
        tracking: tx.tracking.at(t),
        line_height: tx.line_height,
        align: tx.align,
        glyphs,
        pivots: tx.pivots.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::MarkerTrack;

    fn project() -> Project {
        let mut p = Project::new(1920, 1080, Fps::whole(30), 8.0);
        p.markers.insert(MarkerTrack::from_times("beat", (0..16).map(|i| 0.5 + i as f64 * 0.5)));
        let scene: Scene = serde_json::from_value(serde_json::json!({
            "id": "s1", "start": 0,
            "layers": [
                {"id": "box", "kind": "shape", "shape": "rect", "size": [200, 100], "fill": "#ff0000",
                 "transform": {"position": {"keys": [{"at": 0, "v": [0, 540]}, {"at": "beat:3", "v": [960, 540], "ease": "glide", "dur": "10f", "anchor": "start"}]}},
                 "time": {"start": "beat:1"}},
                {"id": "title", "kind": "text", "text": "HI THERE",
                 "transform": {"position": [960, 300]}, "time": {"start": "#box.start + 0.5", "step_fps": 12},
                 "animators": [{"at": "#title.start", "from": {"y": 50, "opacity": 0}, "stagger": "2f", "dur": "6f"}]},
                {"id": "group", "kind": "comp", "transform": {"position": [100, 100], "opacity": 0.5},
                 "layers": [{"id": "dot", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff",
                             "transform": {"position": [10, 0]}}]}
            ]
        }))
        .unwrap();
        p.scenes.push(scene);
        p
    }

    #[test]
    fn frames_evaluate_in_paint_order() {
        let c = Compiled::new(&project());
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let f = c.frame(0.25);
        // box starts at beat 1 (0.5 s); the comp is visible.
        assert_eq!(f.layers.iter().map(|l| &*l.id).collect::<Vec<_>>(), vec!["dot"]);
        let dot = &f.layers[0];
        assert_eq!(dot.matrix.apply(0.0, 0.0), (110.0, 100.0));
        assert_eq!(dot.opacity, 0.5);
        let f = c.frame(3.0);
        assert_eq!(f.layers.len(), 3);
        assert_eq!(f.layers[0].matrix.apply(0.0, 0.0), (960.0, 540.0));
    }

    #[test]
    fn element_references_and_text_animators() {
        let c = Compiled::new(&project());
        let title = |t: f64| {
            c.frame(t).layers.into_iter().find(|l| &*l.id == "title").map(|l| match l.content {
                Content::Text(tx) => tx,
                _ => panic!(),
            })
        };
        assert!(title(0.9).is_none());
        let tx = title(1.0).unwrap();
        // H at its from state, second glyph not started yet.
        assert_eq!(tx.glyphs[0].opacity, 0.0);
        let tx = title(3.0).unwrap();
        assert!(tx.glyphs.iter().all(|g| g.opacity == 1.0 && g.y == 0.0));
        // Space has no glyph unit and stays identity.
        assert_eq!(tx.pivots[2], u32::MAX);
    }

    #[test]
    fn stepped_layers_are_bit_identical_between_steps() {
        let c = Compiled::new(&project());
        let a = c.frame(1.0 + 1.0 / 30.0);
        let b = c.frame(1.0 + 2.0 / 30.0);
        let title = |f: &FrameState| f.layers.iter().find(|l| &*l.id == "title").cloned().unwrap();
        // 12 fps steps at 30 fps: frames 31 and 32 share a step.
        assert_eq!(Step { fps: 12.0, phase: 0.0 }.quantize(1.0 + 1.0 / 30.0), Step { fps: 12.0, phase: 0.0 }.quantize(1.0 + 2.0 / 30.0));
        assert_eq!(title(&a), title(&b));
    }

    #[test]
    fn errors_name_the_element() {
        let mut p = project();
        p.scenes[0].layers[0].common.time.start = Some("hit:2".into());
        let c = Compiled::new(&p);
        assert!(c.errors.iter().any(|e| e.contains("box") && e.contains("hit")), "{:?}", c.errors);
    }

    #[test]
    fn scene_cut_on_marker_leads_one_frame() {
        let mut p = project();
        p.scenes[0].start = "beat:2".into();
        let c = Compiled::new(&p);
        assert!((c.scenes[0].start - (1.0 - 1.0 / 30.0)).abs() < 1e-12);
    }
}
