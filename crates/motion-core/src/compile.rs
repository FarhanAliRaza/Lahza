//! Compilation: every time reference resolved once, so evaluating a frame
//! is pure arithmetic. [`Compiled::frame`] turns a time into the flat,
//! paint-ordered list of layer states a renderer draws.

use crate::curve::Curve;
use crate::document::*;
use crate::markers::Markers;
use crate::property::{compile as compile_prop, Ctx, Prop, Property, SyncPoint};
use crate::time::{Duration, Fps, Lead, Step, TimeBase};
use crate::timeref::{Anchor, Edge, ElementTimes, TimeRef};
use crate::space::{Camera3, Mat4, Proj};
use crate::value::{Animatable, Color, Vec2};
use crate::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod instancer;
use instancer::CInstancer;

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
    /// Layer space → canvas pixels, camera included (a homography for
    /// 3D layers, affine otherwise).
    pub matrix: Proj,
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
    /// Per-pixel post effects applied when the layer is composited.
    Post(PostState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostKind {
    Exposure,
    BrightnessContrast,
    Levels,
    HueSaturation,
    Tint,
    GradientMap,
    Invert,
    Fill,
    Lut,
    Displace,
    DirectionalBlur,
    RgbSplit,
    Vignette,
    Grain,
}

/// Evaluated post effect: numbers in `v` by kind (lengths in canvas px):
/// exposure [stops]; brightness_contrast [b, c]; levels [in_b, in_w, γ,
/// out_b, out_w]; hue_saturation [hue°, sat, light]; tint [amount] with
/// colours [black, white]; gradient_map [amount] + stops; invert [amount];
/// fill [amount] + colours[0]; lut [amount] + path; displace [amount,
/// scale, time]; directional_blur [dx, dy]; rgb_split [dx, dy]; vignette
/// [amount, softness]; grain [amount, size] + seed.
#[derive(Clone, Debug, PartialEq)]
pub struct PostState {
    pub kind: PostKind,
    pub v: [f64; 5],
    pub colors: [Color; 2],
    pub stops: Option<Arc<Vec<(f64, Color)>>>,
    pub path: Option<Arc<str>>,
    pub seed: u32,
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
    /// A footage frame: the renderer decodes `source_time` of `src`.
    Video(VideoState),
    /// A comp drawn as one picture (AE precomp): children carry absolute
    /// matrices; the group's opacity, blend, effects and matte apply once.
    Group(Vec<LayerState>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoState {
    pub src: Arc<str>,
    pub size: Option<Vec2>,
    /// Seconds into the source (before looping).
    pub source_time: f64,
    pub looped: bool,
    /// For looping: where the loop restarts.
    pub trim: f64,
    /// Image sequences: frames per second.
    pub fps: f64,
}

/// Where a footage layer's sound plays, for the audio mix.
#[derive(Clone, Debug, PartialEq)]
pub struct FootageAudio {
    pub src: Arc<str>,
    /// Timeline seconds where the layer starts and ends.
    pub start: f64,
    pub end: f64,
    pub trim: f64,
    pub speed: f64,
    pub gain_db: f64,
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
    /// Caps, joins, dashes and feather for path strokes; `None` is the
    /// default round caps and joins.
    pub stroke_style: Option<Arc<crate::document::StrokeStyle>>,
    /// Path strokes: colour stops along the full path's length (0 = start).
    pub stroke_gradient: Option<Arc<Vec<(f64, Color)>>>,
    /// Path for `ShapeKind::Path`, possibly mid-morph.
    pub path: Option<PathMorph>,
    /// `ShapeKind::Field`: its points this frame.
    pub field: Option<FieldState>,
}

/// A field shape's evaluated points (layer space) and blend.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldState {
    /// (position, colour, radius px).
    pub points: Vec<(Vec2, Color, f64)>,
    pub softness: f64,
    /// Noise displacement amplitude and feature size, px.
    pub warp: f64,
    pub warp_scale: f64,
}

/// `from` morphing toward `to` by `t` (0…1; springs may overshoot).
#[derive(Clone, Debug, PartialEq)]
pub struct PathMorph {
    pub from: Arc<str>,
    pub to: Option<Arc<str>>,
    pub t: f64,
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
    /// Position-ordered animators are not in it: see `sweeps`.
    pub glyphs: Vec<GlyphState>,
    /// Which unit each character's pivot belongs to, per character.
    pub pivots: Arc<[u32]>,
    /// Faux bold, px the edges move outward.
    pub weight: f64,
    /// Outline colour, width (px) and placement.
    pub stroke: Option<(Color, f64, StrokeAlign)>,
    /// Rim light colour, width (px) and light angle (degrees, 0 = above).
    pub edge: Option<(Color, f64, f64)>,
    /// Position-ordered animators; the renderer applies them from the
    /// laid-out unit positions.
    pub sweeps: Vec<Sweep>,
    /// Per character: glyph, word and line unit (u32::MAX = none).
    pub units: Arc<[[u32; 3]]>,
}

/// A text animator ordered by position, evaluated at one time.
#[derive(Clone, Debug, PartialEq)]
pub struct Sweep {
    /// 0 glyph, 1 word, 2 line (index into `TextState::units`).
    pub unit: usize,
    /// Animated units [lo, hi) (0-based).
    pub range: (usize, usize),
    pub order: SpatialOrder,
    /// Seconds since the first unit started.
    pub elapsed: f64,
    /// Delay of the last unit (position 1): stagger × (units − 1).
    pub span: f64,
    pub dur: f64,
    pub curve: Curve,
    pub from: [f64; 5],
    pub to: [f64; 5],
}

impl Sweep {
    /// The unit at normalised position `s` (0…1 along the sweep).
    pub fn values(&self, s: f64) -> [f64; 5] {
        let p = self.curve.progress(self.elapsed - s * self.span, self.dur);
        std::array::from_fn(|k| self.from[k] + (self.to[k] - self.from[k]) * p)
    }
    /// Feathered sweeps: where the opacity wipe's middle is (0…1 along
    /// the text extent); it travels from −feather/2 to 1 + feather/2 over
    /// the whole animation (span + dur), eased.
    pub fn front(&self) -> f64 {
        let f = self.order.feather;
        -f / 2.0 + (1.0 + f) * self.curve.progress(self.elapsed, self.span + self.dur)
    }
}

/// Adds one animator's values to a character's state; `opacity: false`
/// leaves opacity to a feathered wipe.
pub fn apply_unit(g: &mut GlyphState, v: [f64; 5], opacity: bool) {
    g.x += v[0];
    g.y += v[1];
    g.scale *= v[2];
    g.rotation += v[3];
    if opacity {
        g.opacity *= v[4].clamp(0.0, 1.0);
    }
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
    camera_fov: Prop<f64>,
    camera_orbit: Prop<Vec2>,
    camera_dolly: Prop<f64>,
    camera_focus: Option<CFocus>,
    camera_aperture: Prop<f64>,
    transition: Option<CTransition>,
    pub layers: Vec<CLayer>,
}

enum CFocus {
    Depth(Prop<f64>),
    /// A top-level layer of the scene.
    Layer(usize),
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

struct CPost {
    kind: PostKind,
    v: Vec<Prop<f64>>,
    colors: Vec<Prop<Color>>,
    stops: Option<Arc<Vec<(f64, Color)>>>,
    path: Option<Arc<str>>,
    seed: u32,
}

enum CEffect {
    Post(CPost),
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
    z: Prop<f64>,
    rotation_x: Prop<f64>,
    rotation_y: Prop<f64>,
    three_d: bool,
    blend: Blend,
    effects: Vec<CEffect>,
    matte: Option<(usize, MatteMode)>,
    /// Used as another layer's matte: not drawn by itself.
    is_matte: bool,
    parent: Option<usize>,
    follow: Option<CFollow>,
    hidden: bool,
    kind: CKind,
}

/// `follow`: placed on sibling `target`'s path.
struct CFollow {
    target: usize,
    at: CFollowAt,
    rotate: bool,
    offset: Prop<Vec2>,
}

enum CFollowAt {
    Head,
    Tail,
    Fraction(Prop<f64>),
}

impl CLayer {
    pub fn children(&self) -> &[CLayer] {
        match &self.kind {
            CKind::Comp(children) => children,
            CKind::Instancer(n) => &n.layers,
            _ => &[],
        }
    }
}

/// Element references against compiled windows (for times given to tools).
impl Compiled {
    /// Resolves a time reference against this project's markers and
    /// element times (`#card.start`), e.g. where a sound plays.
    pub fn resolve(&self, markers: &crate::markers::Markers, at: &crate::timeref::TimeRef) -> Result<f64> {
        let mut me = self;
        at.resolve(markers, &self.base, &mut me)
    }
}

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
    Video(CVideo),
    Comp(Vec<CLayer>),
    Instancer(Box<CInstancer>),
}

struct CVideo {
    src: Arc<str>,
    size: Option<Prop<Vec2>>,
    trim: f64,
    speed: f64,
    looped: bool,
    remap: Option<Prop<f64>>,
    fps: f64,
    gain_db: f64,
    mute: bool,
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
    stroke_style: Option<Arc<crate::document::StrokeStyle>>,
    stroke_gradient: Option<Arc<Vec<(f64, Color)>>>,
    path: Option<(Vec<Arc<str>>, Prop<f64>)>,
    field: Option<CField>,
}

struct CField {
    points: Vec<(Prop<Vec2>, Prop<Color>, Prop<f64>)>,
    softness: Prop<f64>,
    warp: Prop<f64>,
    warp_scale: Prop<f64>,
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
    weight: Prop<f64>,
    stroke: Option<(Prop<Color>, Prop<f64>, StrokeAlign)>,
    edge: Option<(Prop<Color>, Prop<f64>, Prop<f64>)>,
    animators: Vec<CAnim>,
    /// Per character: unit index for glyph, word and line (u32::MAX = none).
    units: Arc<[[u32; 3]]>,
    pivots: Arc<[u32]>,
}

struct CAnim {
    unit: usize,
    /// Position-driven order (applied by the renderer).
    spatial: Option<SpatialOrder>,
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

    #[allow(clippy::too_many_arguments)]
    fn post(&mut self, kind: PostKind, v: &[&Property<f64>], colors: &[&Property<Color>], id: &str, what: &dyn Fn(&str) -> String, start: f64, step: Option<Step>, shift: f64) -> CEffect {
        let v = v.iter().enumerate().map(|(i, p)| self.prop(p, id, &what(&format!("v{i}")), start, step, shift)).collect();
        let colors = colors.iter().enumerate().map(|(i, p)| self.prop(p, id, &what(&format!("color{i}")), start, step, shift)).collect();
        CEffect::Post(CPost { kind, v, colors, stops: None, path: None, seed: 0 })
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
        // A follower needs a path target whose placement does not depend
        // on the follower itself (through parents and follows).
        for i in 0..out.len() {
            let Some(target) = out[i].follow.as_ref().map(|f| f.target) else { continue };
            let is_path = matches!(&out[target].kind, CKind::Shape(s) if s.path.is_some());
            let error = if !is_path {
                Some(format!("{}.follow: '{}' is not a path layer (shape \"path\" with \"d\")", out[i].id, out[target].id))
            } else if depends_on(&out, target, i) {
                Some(format!("{}.follow: '{}' depends on '{}' through parent or follow (a cycle)", out[i].id, out[target].id, out[i].id))
            } else {
                None
            };
            if let Some(e) = error {
                self.errors.push(e);
                out[i].follow = None;
            }
        }
        // Instancers laid out along a path need a path layer.
        for i in 0..out.len() {
            let CKind::Instancer(n) = &out[i].kind else { continue };
            let Some(target) = n.path_target() else { continue };
            if !matches!(&out[target].kind, CKind::Shape(s) if s.path.is_some()) {
                self.errors.push(format!("{}.layout.layer: '{}' is not a path layer (shape \"path\" with \"d\")", out[i].id, out[target].id));
                if let CKind::Instancer(n) = &mut out[i].kind {
                    n.clear_path_target();
                }
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
        let z = self.prop(&t.z, id, "transform.z", start, step, shift);
        let rotation_x = self.prop(&t.rotation_x, id, "transform.rotation_x", start, step, shift);
        let rotation_y = self.prop(&t.rotation_y, id, "transform.rotation_y", start, step, shift);
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
                Effect::Exposure { stops } => self.post(PostKind::Exposure, &[stops], &[], id, &what, start, step, shift),
                Effect::BrightnessContrast { brightness, contrast } => self.post(PostKind::BrightnessContrast, &[brightness, contrast], &[], id, &what, start, step, shift),
                Effect::Levels { in_black, in_white, gamma, out_black, out_white } => {
                    self.post(PostKind::Levels, &[in_black, in_white, gamma, out_black, out_white], &[], id, &what, start, step, shift)
                }
                Effect::HueSaturation { hue, saturation, lightness } => self.post(PostKind::HueSaturation, &[hue, saturation, lightness], &[], id, &what, start, step, shift),
                Effect::Tint { black, white, amount } => self.post(PostKind::Tint, &[amount], &[black, white], id, &what, start, step, shift),
                Effect::GradientMap { stops, amount } => {
                    let mut e = self.post(PostKind::GradientMap, &[amount], &[], id, &what, start, step, shift);
                    if let CEffect::Post(p) = &mut e {
                        let mut s = stops.clone();
                        s.sort_by(|a, b| a.0.total_cmp(&b.0));
                        if s.is_empty() {
                            self.errors.push(format!("{id}.{}: gradient_map needs stops", what("stops")));
                            s.push((0.0, Color::BLACK));
                        }
                        p.stops = Some(Arc::new(s));
                    }
                    e
                }
                Effect::Invert { amount } => self.post(PostKind::Invert, &[amount], &[], id, &what, start, step, shift),
                Effect::Fill { color, amount } => self.post(PostKind::Fill, &[amount], &[color], id, &what, start, step, shift),
                Effect::Lut { path, amount } => {
                    let mut e = self.post(PostKind::Lut, &[amount], &[], id, &what, start, step, shift);
                    if let CEffect::Post(p) = &mut e {
                        p.path = Some(path.as_str().into());
                    }
                    e
                }
                Effect::Displace { amount, scale, speed, seed } => {
                    let mut e = self.post(PostKind::Displace, &[amount, scale, speed], &[], id, &what, start, step, shift);
                    if let CEffect::Post(p) = &mut e {
                        p.seed = *seed;
                    }
                    e
                }
                Effect::DirectionalBlur { angle, length } => self.post(PostKind::DirectionalBlur, &[angle, length], &[], id, &what, start, step, shift),
                Effect::RgbSplit { amount, angle } => self.post(PostKind::RgbSplit, &[amount, angle], &[], id, &what, start, step, shift),
                Effect::Vignette { amount, softness } => self.post(PostKind::Vignette, &[amount, softness], &[], id, &what, start, step, shift),
                Effect::Grain { amount, size, seed } => {
                    let mut e = self.post(PostKind::Grain, &[amount, size], &[], id, &what, start, step, shift);
                    if let CEffect::Post(p) = &mut e {
                        p.seed = *seed;
                    }
                    e
                }
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
        let mut parent = c.parent.as_ref().and_then(|p| match siblings.get(p.as_str()) {
            Some(i) => Some(*i),
            None => {
                self.errors.push(format!("{id}.parent: no sibling layer '{p}'"));
                None
            }
        });
        let follow = c.follow.as_ref().and_then(|f| match siblings.get(f.layer.as_str()) {
            Some(&target) if f.layer != id => {
                if parent.take().is_some() {
                    self.errors.push(format!("{id}: follow places the layer, so it can't also have a parent"));
                }
                Some(CFollow {
                    target,
                    at: match &f.at {
                        FollowAt::Head => CFollowAt::Head,
                        FollowAt::Tail => CFollowAt::Tail,
                        FollowAt::Fraction(p) => CFollowAt::Fraction(self.prop(p, id, "follow.at", start, step, shift)),
                    },
                    rotate: f.rotate,
                    offset: self.prop(&f.offset, id, "follow.offset", start, step, shift),
                })
            }
            _ => {
                self.errors.push(format!("{id}.follow: no sibling layer '{}'", f.layer));
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
                    stroke_style: s.stroke.as_ref().and_then(|st| st.style()).map(Arc::new),
                    stroke_gradient: s.stroke.as_ref().and_then(|st| st.gradient.as_ref()).and_then(|g| {
                        if g.stops.is_empty() {
                            self.errors.push(format!("{id}.stroke.gradient: needs at least one stop"));
                            return None;
                        }
                        let mut stops = g.stops.clone();
                        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                        Some(Arc::new(stops))
                    }),
                    field: (s.shape == ShapeKind::Field).then(|| {
                        if s.points.is_empty() {
                            self.errors.push(format!("{id}: shape \"field\" needs \"points\""));
                        }
                        CField {
                            points: s
                                .points
                                .iter()
                                .enumerate()
                                .map(|(i, p)| {
                                    let what = |f: &str| format!("points[{i}].{f}");
                                    (
                                        self.prop(&p.position, id, &what("position"), start, step, shift),
                                        self.prop(&p.color, id, &what("color"), start, step, shift),
                                        self.prop(&p.radius, id, &what("radius"), start, step, shift),
                                    )
                                })
                                .collect(),
                            softness: self.prop(&s.softness, id, "softness", start, step, shift),
                            warp: self.prop(&s.warp, id, "warp", start, step, shift),
                            warp_scale: self.prop(&s.warp_scale, id, "warp_scale", start, step, shift),
                        }
                    }),
                    path: match (s.shape, &s.d) {
                        (ShapeKind::Path, Some(PathData::Static(v))) => Some((vec![v.svg().into()], Prop::constant(0.0))),
                        (ShapeKind::Path, Some(PathData::Keys(keys))) => {
                            // Key i's value is path i; the morph index animates
                            // with the usual eases, anchors and leads.
                            let paths: Vec<Arc<str>> = keys.iter().map(|k| k.v.svg().into()).collect();
                            let index = Property::Animated(crate::property::Animated {
                                keys: keys
                                    .iter()
                                    .enumerate()
                                    .map(|(i, k)| crate::property::Key {
                                        at: k.at.clone(),
                                        v: i as f64,
                                        from: None,
                                        ease: k.ease.clone(),
                                        anchor: k.anchor,
                                        lead: k.lead,
                                        dur: k.dur,
                                        space: None,
                                    })
                                    .collect(),
                                add: Vec::new(),
                                step_fps: None,
                                base: None,
                            });
                            Some((paths, self.prop(&index, id, "d", start, step, shift)))
                        }
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
            LayerKind::Video(v) => CKind::Video(CVideo {
                src: v.src.as_str().into(),
                size: v.size.as_ref().map(|s| self.prop(s, id, "size", start, step, shift)),
                trim: v.trim,
                speed: if v.speed.is_finite() && v.speed != 0.0 { v.speed } else { 1.0 },
                looped: v.looped,
                remap: v.time_remap.as_ref().map(|r| self.prop(r, id, "time_remap", start, step, shift)),
                fps: v.fps.unwrap_or(self.windows.base.fps).as_f64(),
                gain_db: v.volume_db,
                mute: v.mute,
            }),
            LayerKind::Comp(comp) => CKind::Comp(self.layers(&comp.layers, step, shift - offset)),
            LayerKind::Instancer(n) => CKind::Instancer(Box::new(self.instancer(n, id, start, step, shift, siblings))),
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
            z,
            rotation_x,
            rotation_y,
            three_d: c.three_d,
            blend: c.blend,
            effects,
            matte,
            is_matte: false,
            parent,
            follow,
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
            let (index_order, spatial) = match a.order {
                UnitOrder::Index(o) => (o, None),
                UnitOrder::Spatial(s) => {
                    if s.axis.is_some() && s.radial.is_some() {
                        self.errors.push(format!("{id}.{what}.order: give either axis or radial, not both"));
                    }
                    if !(s.feather >= 0.0) {
                        self.errors.push(format!("{id}.{what}.order.feather: must be ≥ 0"));
                    }
                    (Order::Forward, Some(SpatialOrder { feather: s.feather.max(0.0), ..s }))
                }
            };
            let order: Vec<f64> = (0..n)
                .map(|u| {
                    if u < lo || u >= hi {
                        return 0.0;
                    }
                    let k = (u - lo) as f64;
                    let last = span.saturating_sub(1) as f64;
                    match index_order {
                        Order::Forward => k,
                        Order::Reverse => last - k,
                        Order::Center => (k - last / 2.0).abs().floor(),
                        Order::Edges => (last / 2.0).floor() - (k - last / 2.0).abs().floor(),
                        Order::Random => 0.0,
                    }
                })
                .collect();
            let ranks = if index_order == Order::Random {
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
                spatial,
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
            weight: self.prop(&tx.weight, id, "weight", start, step, shift),
            stroke: tx.stroke.as_ref().map(|s| {
                (
                    self.prop(&s.color, id, "stroke.color", start, step, shift),
                    self.prop(&s.width, id, "stroke.width", start, step, shift),
                    s.align,
                )
            }),
            edge: tx.edge.as_ref().map(|e| {
                (
                    self.prop(&e.color, id, "edge.color", start, step, shift),
                    self.prop(&e.width, id, "edge.width", start, step, shift),
                    self.prop(&e.angle, id, "edge.angle", start, step, shift),
                )
            }),
            animators,
            units: units.into(),
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
            let camera_fov = c.prop(&scene.camera.fov, id, "camera.fov", start, None, pre_roll);
            let camera_orbit = c.prop(&scene.camera.orbit, id, "camera.orbit", start, None, pre_roll);
            let camera_dolly = c.prop(&scene.camera.dolly, id, "camera.dolly", start, None, pre_roll);
            let camera_aperture = c.prop(&scene.camera.aperture, id, "camera.aperture", start, None, pre_roll);
            let camera_focus = match &scene.camera.focus {
                None => None,
                Some(Focus::Depth(p)) => Some(CFocus::Depth(c.prop(p, id, "camera.focus", start, None, pre_roll))),
                Some(Focus::Layer(layer)) => match scene.layers.iter().position(|l| l.id == *layer) {
                    Some(i) => Some(CFocus::Layer(i)),
                    None => {
                        c.errors.push(format!("{id}.camera.focus: no top-level layer '{layer}' in the scene"));
                        None
                    }
                },
            };
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
                camera_fov,
                camera_orbit,
                camera_dolly,
                camera_focus,
                camera_aperture,
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

    /// Sound of footage layers (constant speed, not muted, no time remap),
    /// clipped to the scenes that show them.
    pub fn footage_audio(&self) -> Vec<FootageAudio> {
        fn walk(layers: &[CLayer], scene: (f64, f64), shift: f64, out: &mut Vec<FootageAudio>) {
            for l in layers {
                match &l.kind {
                    CKind::Video(v) if !v.mute && v.remap.is_none() && !l.hidden => {
                        // Layer times are content times: timeline = content − shift.
                        let start = (l.start - shift).max(scene.0);
                        let end = (l.end - shift).min(scene.1);
                        if end > start {
                            out.push(FootageAudio {
                                src: v.src.clone(),
                                start,
                                end,
                                trim: v.trim + (start - (l.start - shift)) * v.speed,
                                speed: v.speed,
                                gain_db: v.gain_db,
                            });
                        }
                    }
                    CKind::Comp(children) => walk(children, scene, shift + l.offset, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        for s in &self.scenes {
            walk(&s.layers, (s.start, s.end), s.pre_roll, &mut out);
        }
        out
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

    fn camera(&self, scene: &CScene, tc: f64) -> Space {
        let (w, h) = (self.settings.width as f64, self.settings.height as f64);
        let zoom = scene.camera_zoom.at(tc);
        let roll = scene.camera_rotation.at(tc);
        let center = scene.camera_position.as_ref().map(|p| p.at(tc)).unwrap_or(Vec2::new(w / 2.0, h / 2.0));
        let flat = Affine::translate(w / 2.0, h / 2.0)
            .then(Affine::scale(zoom, zoom))
            .then(Affine::rotate(-roll))
            .then(Affine::translate(-center.x(), -center.y()));
        let orbit = scene.camera_orbit.at(tc);
        let (fov, dolly) = (scene.camera_fov.at(tc), scene.camera_dolly.at(tc));
        let cam = Camera3::new(w, h, (center.x(), center.y()), zoom, roll, fov, (orbit.x(), orbit.y()), dolly);
        let mut space = Space { world: Mat4::IDENTITY, flat, cam, three_d: false, dof: None };
        let aperture = scene.camera_aperture.at(tc);
        if aperture > 0.0 {
            // Depths are view-space z; the point of interest sits at the
            // camera's distance, so a focus depth reads like a layer's z.
            let plane = match &scene.camera_focus {
                None => Some(crate::space::focal(h, fov) - dolly),
                Some(CFocus::Depth(p)) => Some(crate::space::focal(h, fov) - dolly + p.at(tc)),
                Some(CFocus::Layer(i)) => {
                    let mut cache = vec![None; scene.layers.len()];
                    let world = layer_matrix(&scene.layers, *i, tc, None, &space, &mut cache);
                    Some(view_depth(&space, world))
                }
            };
            space.dof = plane.map(|plane| Dof { plane, per_px: aperture / 100.0 });
        }
        space
    }

    fn scene_layers(&self, scene: &CScene, t: f64, out: &mut Vec<LayerState>) {
        let tc = t + scene.pre_roll;
        emit(&scene.layers, tc, None, &self.camera(scene, tc), 1.0, out);
    }

    /// A whole scene as one group with its background, for transitions.
    fn scene_group(&self, scene: &CScene, t: f64, matrix: Affine, opacity: f64) -> LayerState {
        let matrix = Proj::from(matrix);
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
                mask.matrix = Affine::translate(cx, cy).into();
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
        matrix: Affine::translate(w / 2.0, h / 2.0).into(),
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
            stroke_style: None,
            stroke_gradient: None,
            path: None,
            field: None,
        }),
    }
}

/// Where layers are being placed: the comp chain's world transform and the
/// scene's cameras (flat for 2D layers, perspective for 3D ones).
#[derive(Clone, Copy)]
struct Space {
    world: Mat4,
    flat: Affine,
    cam: Camera3,
    /// Inside a 3D comp: everything is on its plane, projected.
    three_d: bool,
    /// Depth of field for 3D layers.
    dof: Option<Dof>,
}

/// Depth of field: blur sigma = `per_px` × |view depth − `plane`| canvas px.
#[derive(Clone, Copy)]
struct Dof {
    plane: f64,
    per_px: f64,
}

/// View-space depth of a layer's origin.
fn view_depth(space: &Space, world: Mat4) -> f64 {
    space.cam.view.then(world).apply(0.0, 0.0, 0.0)[2]
}

impl Space {
    fn screen(&self, world: Mat4, three_d: bool) -> Proj {
        if three_d {
            self.cam.project.then(world).plane()
        } else {
            Proj::from(self.flat.then(world.to_affine()))
        }
    }
}

fn layer_matrix(layers: &[CLayer], i: usize, tc: f64, inherited: Option<Step>, space: &Space, cache: &mut [Option<Mat4>]) -> Mat4 {
    if let Some(m) = cache[i] {
        return m;
    }
    let l = &layers[i];
    let t = clock(l.step.or(inherited), tc);
    let s = l.scale.at(t);
    let a = l.anchor.at(t);
    let local = |origin: Mat4| {
        origin
            .then(Mat4::rotate_y(l.rotation_y.at(t)))
            .then(Mat4::rotate_x(l.rotation_x.at(t)))
            .then(Mat4::rotate_z(l.rotation.at(t)))
            .then(Mat4::scale(s.x(), s.y(), 1.0))
            .then(Mat4::translate(-a.x(), -a.y(), 0.0))
    };
    let m = match &l.follow {
        Some(f) => local(follow_origin(layers, f, t, tc, inherited, space, cache)),
        None => {
            let p = l.position.at(t);
            let local = local(Mat4::translate(p.x(), p.y(), l.z.at(t)));
            match l.parent {
                Some(pi) => layer_matrix(layers, pi, tc, inherited, space, cache).then(local),
                None => local,
            }
        }
    };
    cache[i] = Some(m);
    m
}

/// Where a follower sits: the point on its target's path (as drawn at
/// this moment: morph, trim and full transform), turned along the path
/// when `rotate`, then moved by `offset`. `t` is the follower's clock.
#[allow(clippy::too_many_arguments)]
fn follow_origin(layers: &[CLayer], f: &CFollow, t: f64, tc: f64, inherited: Option<Step>, space: &Space, cache: &mut [Option<Mat4>]) -> Mat4 {
    let target = &layers[f.target];
    let tm = layer_matrix(layers, f.target, tc, inherited, space, cache);
    let tt = clock(target.step.or(inherited), tc);
    let mut point = ((0.0, 0.0), (1.0, 0.0));
    if let CKind::Shape(CShape { path: Some((paths, index)), trim, .. }) = &target.kind {
        let trim = trim.at(tt);
        let fraction = match &f.at {
            CFollowAt::Head => trim.y(),
            CFollowAt::Tail => trim.x(),
            CFollowAt::Fraction(p) => p.at(t),
        };
        // Flattened exactly as the renderer flattens the target, so the
        // point lies on the drawn stroke.
        let screen = space.screen(space.world.then(tm), space.three_d || target.three_d);
        let tolerance = crate::path::tolerance(screen.max_scale());
        if let Ok(subpaths) = crate::path::morph(&path_morph(paths, index.at(tt)), tolerance) {
            point = crate::path::point_at(&subpaths, fraction).unwrap_or(point);
        }
    }
    let ((x, y), (dx, dy)) = point;
    let p = tm.apply(x, y, 0.0);
    let mut m = Mat4::translate(p[0], p[1], p[2]);
    if f.rotate {
        let q = tm.apply(x + dx, y + dy, 0.0);
        m = m.then(Mat4::rotate_z((q[1] - p[1]).atan2(q[0] - p[0]).to_degrees()));
    }
    let o = f.offset.at(t);
    m.then(Mat4::translate(o.x(), o.y(), 0.0))
}

/// Whether layer `from`'s placement depends on layer `on` (through parents
/// and follow targets), or is it.
fn depends_on(layers: &[CLayer], from: usize, on: usize) -> bool {
    let mut stack = vec![from];
    let mut seen = HashSet::new();
    while let Some(c) = stack.pop() {
        if c == on {
            return true;
        }
        if seen.insert(c) {
            stack.extend(layers[c].parent);
            stack.extend(layers[c].follow.as_ref().map(|f| f.target));
        }
    }
    false
}

/// Path keys `paths` at morph index `x` (springs may overshoot the last).
fn path_morph(paths: &[Arc<str>], x: f64) -> PathMorph {
    let last = paths.len() - 1;
    let i = (x.floor().max(0.0) as usize).min(last.saturating_sub(1));
    if last == 0 {
        PathMorph { from: paths[0].clone(), to: None, t: 0.0 }
    } else {
        // Overshooting springs extrapolate past the last path.
        let t = if x >= last as f64 { x - (last - 1) as f64 } else { x - i as f64 };
        let i = if x >= last as f64 { last - 1 } else { i };
        if t.abs() < 1e-9 {
            PathMorph { from: paths[i].clone(), to: None, t: 0.0 }
        } else if (t - 1.0).abs() < 1e-9 {
            PathMorph { from: paths[i + 1].clone(), to: None, t: 0.0 }
        } else {
            PathMorph { from: paths[i].clone(), to: Some(paths[i + 1].clone()), t }
        }
    }
}

fn clock(step: Option<Step>, t: f64) -> f64 {
    match step {
        Some(s) => s.quantize(t),
        None => t,
    }
}

fn emit(layers: &[CLayer], tc: f64, inherited: Option<Step>, space: &Space, parent_opacity: f64, out: &mut Vec<LayerState>) {
    let mut cache = vec![None; layers.len()];
    // Paint order, with runs of 3D layers sorted back to front by depth.
    let mut order: Vec<usize> = (0..layers.len()).collect();
    let mut i = 0;
    while i < order.len() {
        if !(layers[order[i]].three_d || space.three_d) {
            i += 1;
            continue;
        }
        let start = i;
        while i < order.len() && (layers[order[i]].three_d || space.three_d) {
            i += 1;
        }
        let mut depth: Vec<(f64, usize)> = order[start..i]
            .iter()
            .map(|&k| {
                let world = space.world.then(layer_matrix(layers, k, tc, inherited, space, &mut cache));
                (view_depth(space, world), k)
            })
            .collect();
        // Stable: equal depths keep list order.
        depth.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (slot, (_, k)) in order[start..i].iter_mut().zip(depth) {
            *slot = k;
        }
    }
    for i in order {
        if layers[i].is_matte {
            continue;
        }
        layer(layers, i, tc, inherited, space, parent_opacity, &mut cache, false, out);
    }
}

/// Emits layer `i`: flattened comps push their children into `out`;
/// everything else pushes one state. `isolate` forces comps into a group
/// (matte sources).
#[allow(clippy::too_many_arguments)]
fn layer(layers: &[CLayer], i: usize, tc: f64, inherited: Option<Step>, space: &Space, parent_opacity: f64, cache: &mut [Option<Mat4>], isolate: bool, out: &mut Vec<LayerState>) {
    let l = &layers[i];
    // A matte source still shapes its layer when hidden (hiding the
    // source is how After Effects users mark it); it never draws itself.
    if (l.hidden && !isolate) || tc < l.start - 1e-9 || tc >= l.end - 1e-9 {
        return;
    }
    let step = l.step.or(inherited);
    let t = clock(step, tc);
    let own_opacity = l.opacity.at(t).clamp(0.0, 1.0);
    if own_opacity * parent_opacity <= 0.0 {
        return;
    }
    let world = space.world.then(layer_matrix(layers, i, tc, inherited, space, cache));
    let three_d = space.three_d || l.three_d;
    let matrix = space.screen(world, three_d);
    let scale = matrix.max_scale();
    let inner = Space { world, three_d, ..*space };
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
            CEffect::Post(p) => {
                let mut v = [0.0; 5];
                for (slot, prop) in v.iter_mut().zip(&p.v) {
                    *slot = prop.at(t);
                }
                let mut colors = [Color::BLACK, Color::WHITE];
                for (slot, prop) in colors.iter_mut().zip(&p.colors) {
                    *slot = clamp_color(prop.at(t));
                }
                match p.kind {
                    // Lengths follow the layer's scale.
                    PostKind::Displace => {
                        v[0] *= scale;
                        v[1] *= scale;
                        v[2] *= t;
                    }
                    PostKind::DirectionalBlur => {
                        let (s, c) = v[0].to_radians().sin_cos();
                        let len = v[1] * scale;
                        v = [c * len, s * len, 0.0, 0.0, 0.0];
                    }
                    PostKind::RgbSplit => {
                        let (s, c) = v[1].to_radians().sin_cos();
                        let len = v[0] * scale;
                        v = [c * len, s * len, 0.0, 0.0, 0.0];
                    }
                    PostKind::Grain => v[1] *= scale,
                    _ => {}
                }
                // Grain changes every frame (and holds on stepped frames).
                let seed = if p.kind == PostKind::Grain { p.seed.wrapping_add((t * 240.0).floor() as i64 as u32) } else { p.seed };
                Some(EffectState::Post(PostState { kind: p.kind, v, colors, stops: p.stops.clone(), path: p.path.clone(), seed }))
            }
            CEffect::Glow { color, radius, intensity } => {
                Some(EffectState::Glow { color: clamp_color(color.at(t)), sigma: radius.at(t).max(0.0) * scale, intensity: intensity.at(t).max(0.0) })
            }
        })
        .collect();
    let mut effects = effects;
    // Depth of field blurs each 3D layer by its distance from the focus
    // plane (comps pass it on to their children instead).
    if let (true, Some(dof), false) = (three_d, space.dof, matches!(l.kind, CKind::Comp(_))) {
        let sigma = dof.per_px * (view_depth(space, world) - dof.plane).abs();
        if sigma > 0.05 {
            effects.insert(0, EffectState::Blur { sigma, repeat_edges: false });
        }
    }
    let matte = l.matte.map(|(mi, mode)| {
        let mut src = Vec::new();
        layer(layers, mi, tc, inherited, space, 1.0, cache, true, &mut src);
        Box::new(MatteState { mode, source: src.pop() })
    });
    let content = match &l.kind {
        CKind::Comp(_) | CKind::Instancer(_) => {
            // Children (or copies) paint straight into `out` unless the
            // layer must be drawn as one picture.
            let pieces = match &l.kind {
                CKind::Instancer(n) => n.count() * n.layers.len(),
                _ => l.children().len(),
            };
            let group = isolate || !effects.is_empty() || matte.is_some() || l.blend != Blend::Normal || own_opacity < 1.0 && pieces > 1;
            let mut kids = Vec::new();
            let (target, opacity) = if group { (&mut kids, 1.0) } else { (&mut *out, parent_opacity * own_opacity) };
            match &l.kind {
                CKind::Instancer(n) => instancer::emit_copies(n, layers, i, tc, t, step, inherited, space, &inner, opacity, cache, target),
                _ => emit(l.children(), tc - l.offset, step, &inner, opacity, target),
            }
            if !group {
                return;
            }
            let inner = kids;
            out.push(LayerState {
                id: l.id.clone(),
                matrix: Proj::IDENTITY,
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
            stroke_style: s.stroke_style.clone(),
            stroke_gradient: s.stroke_gradient.clone(),
            path: s.path.as_ref().map(|(paths, index)| path_morph(paths, index.at(t))),
            field: s.field.as_ref().map(|f| FieldState {
                points: f.points.iter().map(|(p, c, r)| (p.at(t), clamp_color(c.at(t)), r.at(t).max(1e-3))).collect(),
                softness: f.softness.at(t).clamp(0.05, 10.0),
                warp: f.warp.at(t),
                warp_scale: f.warp_scale.at(t).max(1e-3),
            }),
        }),
        CKind::Text(tx) => Content::Text(text_state(tx, t)),
        CKind::Image { src, size } => Content::Image { src: src.clone(), size: size.as_ref().map(|s| s.at(t)) },
        CKind::Video(v) => Content::Video(VideoState {
            src: v.src.clone(),
            size: v.size.as_ref().map(|s| s.at(t)),
            source_time: match &v.remap {
                Some(r) => r.at(t),
                None => v.trim + (t - l.start) * v.speed,
            },
            looped: v.looped,
            trim: v.trim,
            fps: v.fps,
        }),
    };
    out.push(LayerState { id: l.id.clone(), matrix, opacity: parent_opacity * own_opacity, blend: l.blend, effects, matte, content });
}

fn clamp_color(c: Color) -> Color {
    Color::from_fn(|i| c.0[i].clamp(0.0, 1.0))
}

fn text_state(tx: &CText, t: f64) -> TextState {
    let mut glyphs = Vec::new();
    let mut sweeps = Vec::new();
    if !tx.animators.is_empty() {
        glyphs = vec![GlyphState::IDENTITY; tx.units.len()];
        for a in &tx.animators {
            if let Some(order) = a.spatial {
                sweeps.push(Sweep {
                    unit: a.unit,
                    range: a.range,
                    order,
                    elapsed: t - a.start,
                    span: a.stagger * a.range.1.saturating_sub(a.range.0).saturating_sub(1) as f64,
                    dur: a.dur,
                    curve: a.curve.clone(),
                    from: a.from,
                    to: a.to,
                });
                continue;
            }
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
                apply_unit(&mut glyphs[ci], v, true);
            }
        }
    }
    TextState {
        weight: tx.weight.at(t),
        stroke: tx.stroke.as_ref().map(|(c, w, align)| (clamp_color(c.at(t)), w.at(t).max(0.0), *align)),
        edge: tx.edge.as_ref().map(|(c, w, a)| (clamp_color(c.at(t)), w.at(t).max(0.0), a.at(t))),
        sweeps,
        units: tx.units.clone(),
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
    fn spatial_order_becomes_a_sweep() {
        let mut p = Project::new(320, 180, Fps::whole(30), 2.0);
        let scene: Scene = serde_json::from_value(serde_json::json!({"id": "s", "layers": [
            {"id": "t", "kind": "text", "text": "AB CD", "weight": 2, "stroke": {"color": "#f00", "width": 3, "align": "center"},
             "edge": {"angle": 45},
             "animators": [
                {"at": 0.5, "order": {"axis": "y", "from": "end", "feather": 0.25}, "stagger": "2f", "dur": "4f", "from": {"opacity": 0}},
                {"at": 0, "order": "reverse", "from": {"y": 10}, "dur": "4f"}]},
            {"id": "bad", "kind": "text", "text": "X", "animators": [{"at": 0, "order": {"axis": "x", "radial": [0.5, 0.5]}}]}
        ]}))
        .unwrap();
        p.scenes.push(scene);
        let c = Compiled::new(&p);
        assert_eq!(c.errors.len(), 1, "{:?}", c.errors);
        assert!(c.errors[0].contains("bad.animators.0.order"), "{:?}", c.errors);
        let f = c.frame(0.6);
        let Content::Text(tx) = &f.layers[0].content else { panic!() };
        assert_eq!(tx.weight, 2.0);
        assert_eq!(tx.stroke, Some((Color::parse("#f00").unwrap(), 3.0, StrokeAlign::Center)));
        assert_eq!(tx.edge, Some((Color::WHITE, 3.0, 45.0)));
        // The spatial animator is handed to the renderer, not applied here.
        assert_eq!(tx.sweeps.len(), 1);
        let sw = &tx.sweeps[0];
        assert_eq!(sw.order.axis, Some(Axis::Named(AxisName::Y)));
        assert_eq!(sw.order.from, SweepFrom::End);
        assert!((sw.elapsed - 0.1).abs() < 1e-9);
        // Four glyphs: the last starts 3 × 2 f after the first.
        assert!((sw.span - 0.2).abs() < 1e-9, "{}", sw.span);
        assert!(tx.glyphs.iter().all(|g| g.opacity == 1.0));
        assert_eq!(tx.units.len(), 5);
        // Linear timing per position: position 0 is 3 of 4 frames in at
        // 0.6 s; position 1 hasn't started.
        let lin = Sweep { curve: Curve::Linear, ..sw.clone() };
        assert!((lin.values(0.0)[4] - 0.75).abs() < 1e-9);
        assert_eq!(lin.values(1.0)[4], 0.0);
        assert!((lin.values(0.25)[4] - 0.375).abs() < 1e-9);
        // The wipe front travels from −f/2 to 1 + f/2 over span + dur.
        let at = |elapsed: f64| Sweep { elapsed, ..lin.clone() }.front();
        assert!((at(0.0) + 0.125).abs() < 1e-9);
        assert!((at(0.2 + 4.0 / 30.0) - 1.125).abs() < 1e-9);
        // The index-ordered animator still applies per glyph.
        let f = c.frame(0.0);
        let Content::Text(tx) = &f.layers[0].content else { panic!() };
        assert_eq!(tx.glyphs[4].y, 10.0);
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

    fn follow_project(layers: serde_json::Value) -> Project {
        let mut p = Project::new(1000, 1000, Fps::whole(30), 4.0);
        p.scenes.push(serde_json::from_value(serde_json::json!({"id": "s", "start": 0, "layers": layers})).unwrap());
        p
    }

    /// Where layer `id`'s origin lands on the canvas, and its x axis there.
    fn placed(c: &Compiled, t: f64, id: &str) -> ((f64, f64), (f64, f64)) {
        let f = c.frame(t);
        let m = f.layers.iter().find(|l| &*l.id == id).unwrap().matrix;
        let (o, x) = (m.apply(0.0, 0.0), m.apply(1.0, 0.0));
        (o, (x.0 - o.0, x.1 - o.1))
    }

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn follow_rides_head_tail_and_fractions() {
        let c = Compiled::new(&follow_project(serde_json::json!([
            {"id": "line", "kind": "shape", "shape": "path", "d": "M 0 0 L 400 0", "stroke": {"color": "#fff", "width": 4},
             "transform": {"position": [100, 500]}, "trim": {"keys": [{"at": 0, "v": [0, 0]}, {"at": 2, "v": [0, 1]}]}},
            {"id": "band", "kind": "shape", "shape": "path", "d": "M 0 0 L 400 0", "stroke": {"color": "#fff", "width": 4},
             "transform": {"position": [100, 700]}, "trim": [0.25, 0.75]},
            {"id": "head", "kind": "shape", "shape": "ring", "size": 20, "fill": "#fff", "transform": {"position": [999, 999]}, "follow": {"layer": "line"}},
            {"id": "tail", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff", "follow": {"layer": "band", "at": "tail"}},
            {"id": "head2", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff", "follow": {"layer": "band", "at": "head", "offset": [0, -30]}},
            {"id": "frac", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff",
             "follow": {"layer": "line", "at": {"keys": [{"at": 0, "v": 0}, {"at": 2, "v": 1}]}}}
        ])));
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        for (t, x) in [(0.0, 100.0), (0.5, 200.0), (1.0, 300.0), (1.5, 400.0), (2.0, 500.0), (3.0, 500.0)] {
            assert!(near(placed(&c, t, "head").0, (x, 500.0)), "head at {t}: {:?}", placed(&c, t, "head"));
            assert!(near(placed(&c, t, "frac").0, (x, 500.0)), "fraction at {t}");
        }
        assert!(near(placed(&c, 1.0, "tail").0, (200.0, 700.0)));
        assert!(near(placed(&c, 1.0, "head2").0, (400.0, 670.0)), "offset in canvas axes without rotate");
    }

    #[test]
    fn follow_uses_the_targets_full_transform_and_turns_with_it() {
        // Parent turned 90° clockwise; the path scaled 2× about its anchor.
        let c = Compiled::new(&follow_project(serde_json::json!([
            {"id": "p", "kind": "shape", "shape": "circle", "size": 1, "fill": "#00000000", "transform": {"position": [500, 500], "rotation": 90}},
            {"id": "path", "kind": "shape", "shape": "path", "d": "M 0 0 L 100 0", "stroke": {"color": "#fff", "width": 4}, "parent": "p",
             "transform": {"position": [10, 0], "scale": 2, "anchor": [50, 0]}, "trim": [0, 0.75]},
            {"id": "arrow", "kind": "shape", "shape": "polygon", "sides": 3, "size": 20, "fill": "#fff",
             "transform": {"rotation": 10}, "follow": {"layer": "path", "rotate": true, "offset": [0, -20]}}
        ])));
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        // Head (75, 0) → ((75 − 50)·2 + 10, 0) = (60, 0) → turned: (0, 60) → (500, 560);
        // the offset turns with the path: (0, −20) → (20, 0).
        let (o, x) = placed(&c, 0.0, "arrow");
        assert!(near(o, (520.0, 560.0)), "{o:?}");
        // Path direction (down) plus its own 10°.
        let (s, k) = 100f64.to_radians().sin_cos();
        assert!(near(x, (k, s)), "{x:?}");
    }

    #[test]
    fn follow_tracks_a_morphing_path_and_stepped_time() {
        let c = Compiled::new(&follow_project(serde_json::json!([
            {"id": "m", "kind": "shape", "shape": "path", "stroke": {"color": "#fff", "width": 4},
             "d": {"keys": [{"at": 0, "v": "M 0 0 L 100 0"}, {"at": 2, "v": "M 0 100 L 100 100"}]}},
            {"id": "a", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff", "follow": {"layer": "m", "at": 0.5}},
            {"id": "stepped", "kind": "shape", "shape": "path", "d": "M 0 0 L 300 0", "stroke": {"color": "#fff", "width": 4},
             "time": {"step_fps": 10}, "trim": {"keys": [{"at": 0, "v": [0, 0]}, {"at": 3, "v": [0, 1]}]}},
            {"id": "b", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff", "follow": {"layer": "stepped"}}
        ])));
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(near(placed(&c, 1.0, "a").0, (50.0, 50.0)), "{:?}", placed(&c, 1.0, "a"));
        // The stepped stroke holds 0.3 s → its head (and the follower) hold too.
        assert!(near(placed(&c, 0.31, "b").0, (30.0, 0.0)));
        assert!(near(placed(&c, 0.39, "b").0, (30.0, 0.0)));
        assert!(near(placed(&c, 0.41, "b").0, (40.0, 0.0)));
    }

    #[test]
    fn follow_errors_name_the_problem() {
        let errors = |layers| Compiled::new(&follow_project(layers)).errors;
        let e = errors(serde_json::json!([{"id": "a", "kind": "shape", "shape": "circle", "follow": {"layer": "nope"}}]));
        assert!(e.iter().any(|e| e.contains("a.follow: no sibling layer 'nope'")), "{e:?}");
        let e = errors(serde_json::json!([
            {"id": "dot", "kind": "shape", "shape": "circle"},
            {"id": "a", "kind": "shape", "shape": "circle", "follow": {"layer": "dot"}}
        ]));
        assert!(e.iter().any(|e| e.contains("a.follow: 'dot' is not a path layer")), "{e:?}");
        let e = errors(serde_json::json!([
            {"id": "path", "kind": "shape", "shape": "path", "d": "M 0 0 L 10 0", "parent": "a"},
            {"id": "a", "kind": "shape", "shape": "circle", "follow": {"layer": "path"}}
        ]));
        assert!(e.iter().any(|e| e.contains("a.follow") && e.contains("cycle")), "{e:?}");
        let bad: std::result::Result<Layer, _> = Layer::from_json(serde_json::json!({"id": "a", "kind": "shape", "shape": "circle", "follow": {"layer": "p", "at": "middle"}}));
        assert!(bad.unwrap_err().contains("\"head\", \"tail\" or a fraction"));
    }

    #[test]
    fn scene_cut_on_marker_leads_one_frame() {
        let mut p = project();
        p.scenes[0].start = "beat:2".into();
        let c = Compiled::new(&p);
        assert!((c.scenes[0].start - (1.0 - 1.0 / 30.0)).abs() < 1e-12);
    }
}
