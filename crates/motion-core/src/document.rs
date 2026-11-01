//! The project document (design section 6). JSON on disk; every node has a
//! stable `id`. Unknown fields are rejected so typos surface immediately.

use crate::curve::Ease;
use crate::markers::Markers;
use crate::property::Property;
use crate::time::{Duration, Fps, Lead};
use crate::timeref::{Anchor, TimeRef};
use crate::value::{Color, Vec2};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(default = "version")]
    pub version: u32,
    pub settings: Settings,
    #[serde(default, skip_serializing_if = "Audio::is_empty")]
    pub audio: Audio,
    /// Every file the project uses, with where it came from and its licence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Asset>,
    /// Scenes in timeline order; stored as `scenes/<id>.json` on disk.
    #[serde(default)]
    pub scenes: Vec<Scene>,
    /// Marker tracks; stored as `markers/<track>.json` on disk.
    #[serde(skip)]
    pub markers: Markers,
}

fn version() -> u32 {
    FORMAT_VERSION
}

impl Project {
    pub fn new(width: u32, height: u32, fps: Fps, duration: f64) -> Self {
        Self {
            version: FORMAT_VERSION,
            settings: Settings { width, height, fps, duration, ..Settings::default() },
            audio: Audio::default(),
            assets: Vec::new(),
            scenes: Vec::new(),
            markers: Markers::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
    /// Seconds.
    pub duration: f64,
    #[serde(default = "black")]
    pub background: Color,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Off unless set; scenes may override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_blur: Option<MotionBlur>,
}

fn black() -> Color {
    Color::BLACK
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: Fps::whole(30),
            duration: 10.0,
            background: Color::BLACK,
            style: None,
            motion_blur: None,
        }
    }
}

/// Shutter-angle motion blur: `samples` sub-frames spread over
/// shutter/360 of a frame, centred on the frame time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionBlur {
    /// Degrees; 180 is film-like, 0 disables.
    pub shutter: f64,
    #[serde(default = "eight")]
    pub samples: u32,
}

fn eight() -> u32 {
    8
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<AudioClip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vo: Option<AudioClip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sfx: Vec<AudioClip>,
}

impl Audio {
    pub fn is_empty(&self) -> bool {
        self.music.is_none() && self.vo.is_none() && self.sfx.is_empty()
    }
    pub fn clips(&self) -> impl Iterator<Item = &AudioClip> {
        self.music.iter().chain(self.vo.iter()).chain(self.sfx.iter())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioClip {
    /// Relative to the project folder.
    pub path: String,
    /// Timeline time where the clip starts.
    #[serde(default)]
    pub at: TimeRef,
    /// Seconds skipped at the start of the file.
    #[serde(default)]
    pub trim: f64,
    #[serde(default)]
    pub gain_db: f64,
    /// Music only: lower by this much under narration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duck_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub path: String,
    /// Where it came from: "user", a URL, "generated".
    pub source: String,
    pub licence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub start: TimeRef,
    /// Cut lead; defaults to −1 frame when `start` is a marker reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<Lead>,
    /// End of the scene; defaults to the next scene's start or the project end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<TimeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Duration>,
    /// The shot opens this far into its animation (content time = t + pre_roll).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_roll: Option<Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Color>,
    /// How this scene arrives from the previous one (default: a cut).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
    #[serde(default, skip_serializing_if = "Camera::is_default")]
    pub camera: Camera,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_blur: Option<MotionBlur>,
    /// Stepped clock for every layer of the scene (the camera stays smooth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_fps: Option<f64>,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

impl Scene {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            name: None,
            start: TimeRef::Seconds(0.0),
            lead: None,
            end: None,
            duration: None,
            pre_roll: None,
            background: None,
            transition: None,
            camera: Camera::default(),
            motion_blur: None,
            step_fps: None,
            layers: Vec::new(),
        }
    }
}

/// A 2D camera on its own smooth clock. `position` is the point of the
/// scene shown at the canvas centre (default: the canvas centre).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Property<Vec2>>,
    #[serde(default = "one_prop", skip_serializing_if = "is_one")]
    pub zoom: Property<f64>,
    /// Degrees.
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub rotation: Property<f64>,
}

impl Default for Camera {
    fn default() -> Self {
        Self { position: None, zoom: one_prop(), rotation: zero_prop() }
    }
}

impl Camera {
    pub fn is_default(&self) -> bool {
        *self == Camera::default()
    }
}

fn one_prop() -> Property<f64> {
    Property::Constant(1.0)
}
fn zero_prop() -> Property<f64> {
    Property::Constant(0.0)
}
fn is_one(p: &Property<f64>) -> bool {
    *p == Property::Constant(1.0)
}
fn is_zero(p: &Property<f64>) -> bool {
    *p == Property::Constant(0.0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    #[serde(default = "zero_vec")]
    pub position: Property<Vec2>,
    /// Pivot in the layer's own coordinates (shapes and text are centred
    /// on their origin, so the default 0,0 is their centre).
    #[serde(default = "zero_vec", skip_serializing_if = "is_zero_vec")]
    pub anchor: Property<Vec2>,
    #[serde(default = "one_vec", skip_serializing_if = "is_one_vec")]
    pub scale: Property<Vec2>,
    /// Degrees, clockwise.
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub rotation: Property<f64>,
    #[serde(default = "one_prop", skip_serializing_if = "is_one")]
    pub opacity: Property<f64>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: zero_vec(),
            anchor: zero_vec(),
            scale: one_vec(),
            rotation: zero_prop(),
            opacity: one_prop(),
        }
    }
}

fn zero_vec() -> Property<Vec2> {
    Property::Constant(Vec2::ZERO)
}
fn one_vec() -> Property<Vec2> {
    Property::Constant(Vec2::ONE)
}
fn is_zero_vec(p: &Property<Vec2>) -> bool {
    *p == zero_vec()
}
fn is_one_vec(p: &Property<Vec2>) -> bool {
    *p == one_vec()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerTime {
    /// Defaults to the parent's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<TimeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<TimeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Duration>,
    /// Stepped clock (8 or 12 for stop-motion); 0 turns off an inherited one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_fps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_phase: Option<f64>,
    /// Comps: children see time `t - offset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Duration>,
}

impl LayerTime {
    pub fn is_default(&self) -> bool {
        *self == LayerTime::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Blend {
    #[default]
    Normal,
    Add,
    Multiply,
    Screen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Effect {
    /// Gaussian blur; radius is the standard deviation in pixels.
    Blur {
        radius: Property<f64>,
        /// Extend edge pixels instead of fading to transparent (AE "repeat edge pixels").
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        repeat_edges: bool,
    },
    /// Drop shadow: a blurred, tinted copy of the layer's alpha beneath it.
    Shadow {
        #[serde(default = "shadow_color")]
        color: Property<Color>,
        #[serde(default = "shadow_offset")]
        offset: Property<Vec2>,
        #[serde(default = "twelve")]
        radius: Property<f64>,
    },
    /// Glow: a blurred, tinted copy added on top.
    Glow {
        #[serde(default = "white")]
        color: Property<Color>,
        #[serde(default = "twelve")]
        radius: Property<f64>,
        #[serde(default = "one_prop")]
        intensity: Property<f64>,
    },
}

fn shadow_color() -> Property<Color> {
    Property::Constant(Color([0.0, 0.0, 0.0, 0.5]))
}
fn shadow_offset() -> Property<Vec2> {
    Property::Constant(Vec2::new(0.0, 12.0))
}
fn twelve() -> Property<f64> {
    Property::Constant(12.0)
}

/// Track matte: the layer shows only where the `layer` (a sibling, which
/// is then not drawn itself) is opaque (`alpha`) or bright (`luma`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matte {
    pub layer: String,
    #[serde(default)]
    pub mode: MatteMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatteMode {
    #[default]
    Alpha,
    AlphaInverted,
    Luma,
    LumaInverted,
}

/// How a scene arrives from the previous one, centred on its cut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    #[serde(rename = "type")]
    pub kind: TransitionKind,
    #[serde(default = "transition_dur")]
    pub dur: Duration,
    #[serde(default = "gesture")]
    pub ease: Ease,
    #[serde(default)]
    pub direction: Direction,
}

fn transition_dur() -> Duration {
    Duration::Frames(10.0)
}
fn gesture() -> Ease {
    Ease::Preset(crate::curve::Preset::Gesture)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransitionKind {
    /// Cross-dissolve.
    Fade,
    /// Both scenes move together.
    Push,
    /// The new scene slides over the old.
    Slide,
    /// The old scene zooms past the camera while the new one settles in.
    Zoom,
    /// A hard edge reveals the new scene.
    Wipe,
    /// A fast push with motion smear (peak speed on the cut).
    Whip,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

/// A layer: common fields plus the fields of its `kind`.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: String,
    pub common: Common,
    pub kind: LayerKind,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Common {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default, skip_serializing_if = "LayerTime::is_default")]
    pub time: LayerTime,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub blend: Blend,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// Transform parent: a layer id in the same layer list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matte: Option<Matte>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

fn is_normal(b: &Blend) -> bool {
    *b == Blend::Normal
}

const COMMON_FIELDS: &[&str] = &["name", "transform", "time", "blend", "effects", "parent", "matte", "hidden"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum LayerKind {
    Shape(ShapeLayer),
    Text(TextLayer),
    Image(ImageLayer),
    Video(VideoLayer),
    Comp(CompLayer),
}

impl LayerKind {
    pub fn name(&self) -> &'static str {
        match self {
            LayerKind::Shape(_) => "shape",
            LayerKind::Text(_) => "text",
            LayerKind::Image(_) => "image",
            LayerKind::Video(_) => "video",
            LayerKind::Comp(_) => "comp",
        }
    }
}

impl Serialize for Layer {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut map = serde_json::Map::new();
        map.insert("id".into(), self.id.clone().into());
        let kind = serde_json::to_value(&self.kind).map_err(S::Error::custom)?;
        let common = serde_json::to_value(&self.common).map_err(S::Error::custom)?;
        if let serde_json::Value::Object(k) = kind {
            map.extend(k);
        }
        if let serde_json::Value::Object(c) = common {
            map.extend(c);
        }
        serde_json::Value::Object(map).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Layer {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(d)?;
        Layer::from_json(value).map_err(D::Error::custom)
    }
}

impl Layer {
    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        let serde_json::Value::Object(mut map) = value else {
            return Err("a layer must be an object".into());
        };
        let id = match map.remove("id") {
            Some(serde_json::Value::String(id)) if valid_id(&id) => id,
            Some(serde_json::Value::String(id)) => {
                return Err(format!("bad layer id '{id}': use letters, digits, '_' and '-'"))
            }
            _ => return Err("layer needs a string \"id\"".into()),
        };
        let mut common = serde_json::Map::new();
        for field in COMMON_FIELDS {
            if let Some(v) = map.remove(*field) {
                common.insert((*field).into(), v);
            }
        }
        let common: Common = serde_json::from_value(serde_json::Value::Object(common))
            .map_err(|e| format!("layer '{id}': {e}"))?;
        if !map.contains_key("kind") {
            return Err(format!("layer '{id}' needs a \"kind\": shape, text, image or comp"));
        }
        let kind: LayerKind =
            serde_json::from_value(serde_json::Value::Object(map)).map_err(|e| format!("layer '{id}': {e}"))?;
        Ok(Layer { id, common, kind })
    }

    pub fn children(&self) -> &[Layer] {
        match &self.kind {
            LayerKind::Comp(c) => &c.layers,
            _ => &[],
        }
    }
    pub fn children_mut(&mut self) -> Option<&mut Vec<Layer>> {
        match &mut self.kind {
            LayerKind::Comp(c) => Some(&mut c.layers),
            _ => None,
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeKind {
    /// `size` is the diameter (an ellipse when width ≠ height).
    Circle,
    /// Rounded rectangle; `radius` rounds the corners.
    Rect,
    /// Pill: fully rounded ends.
    Capsule,
    /// Annulus with outer diameter `size.x` and `thickness`.
    Ring,
    /// Ring segment from `angles.x` to `angles.y` degrees (0 = up, clockwise), round caps.
    Arc,
    /// Regular polygon with `sides`, circumscribed diameter `size.x`, rounded by `radius`.
    Polygon,
    /// Star with `sides` points; inner radius `inner` × outer.
    Star,
    /// Horizontal stroke of length `size.x`, width `thickness`, centred; use `trim`.
    Line,
    /// Bézier path from SVG path data `d` (layer space; fill uses non-zero
    /// winding, `stroke` draws the outline, `trim` draws it on).
    Path,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapeLayer {
    pub shape: ShapeKind,
    #[serde(default = "hundred")]
    pub size: Property<Vec2>,
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub radius: Property<f64>,
    #[serde(default = "ten", skip_serializing_if = "is_ten")]
    pub thickness: Property<f64>,
    #[serde(default = "five", skip_serializing_if = "is_five")]
    pub sides: u32,
    #[serde(default = "half_prop", skip_serializing_if = "is_half")]
    pub inner: Property<f64>,
    /// Arc angles [from, to] in degrees.
    #[serde(default = "arc_angles", skip_serializing_if = "is_arc_angles")]
    pub angles: Property<Vec2>,
    /// Draw-on for line, ring and arc: visible fraction [from, to] in 0..1.
    #[serde(default = "full_trim", skip_serializing_if = "is_full_trim")]
    pub trim: Property<Vec2>,
    /// SVG path data for `shape: "path"`, e.g. "M -50 0 C -50 -60 50 -60 50 0 Z".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
}

fn hundred() -> Property<Vec2> {
    Property::Constant(Vec2::new(100.0, 100.0))
}
fn ten() -> Property<f64> {
    Property::Constant(10.0)
}
fn is_ten(p: &Property<f64>) -> bool {
    *p == ten()
}
fn five() -> u32 {
    5
}
fn is_five(v: &u32) -> bool {
    *v == 5
}
fn half_prop() -> Property<f64> {
    Property::Constant(0.5)
}
fn is_half(p: &Property<f64>) -> bool {
    *p == half_prop()
}
fn arc_angles() -> Property<Vec2> {
    Property::Constant(Vec2::new(0.0, 270.0))
}
fn is_arc_angles(p: &Property<Vec2>) -> bool {
    *p == arc_angles()
}
fn full_trim() -> Property<Vec2> {
    Property::Constant(Vec2::new(0.0, 1.0))
}
fn is_full_trim(p: &Property<Vec2>) -> bool {
    *p == full_trim()
}

/// A fill: a colour (animatable) or a gradient.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Property<Color>),
    Gradient(Gradient),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gradient {
    /// "linear" or "radial".
    #[serde(rename = "gradient")]
    pub kind: GradientKind,
    /// Layer-space points: linear from→to; radial centre→edge.
    pub from: Vec2,
    pub to: Vec2,
    /// [[offset, "#colour"], …], offsets in 0..1.
    pub stops: Vec<(f64, Color)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GradientKind {
    Linear,
    Radial,
}

impl Serialize for Paint {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Paint::Solid(p) => p.serialize(s),
            Paint::Gradient(g) => g.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Paint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(d)?;
        if value.get("gradient").is_some() {
            let g: Gradient = serde_json::from_value(value).map_err(D::Error::custom)?;
            if g.stops.is_empty() {
                return Err(D::Error::custom("gradient needs at least one stop"));
            }
            return Ok(Paint::Gradient(g));
        }
        serde_json::from_value(value).map(Paint::Solid).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub color: Property<Color>,
    #[serde(default = "four")]
    pub width: Property<f64>,
}

fn four() -> Property<f64> {
    Property::Constant(4.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextLayer {
    pub text: String,
    /// Bundled: "Inter", "Inter Medium", "Inter SemiBold"; or a path to a
    /// TTF/OTF file in the project.
    #[serde(default = "default_font")]
    pub font: String,
    /// Font size in pixels.
    #[serde(default = "ninety_six")]
    pub size: Property<f64>,
    #[serde(default = "white")]
    pub color: Property<Color>,
    /// Extra letter spacing in 1/1000 em.
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub tracking: Property<f64>,
    #[serde(default = "line_height", skip_serializing_if = "is_line_height")]
    pub line_height: f64,
    #[serde(default, skip_serializing_if = "is_center")]
    pub align: Align,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animators: Vec<TextAnimator>,
}

fn default_font() -> String {
    "Inter SemiBold".into()
}
fn ninety_six() -> Property<f64> {
    Property::Constant(96.0)
}
pub(crate) fn white() -> Property<Color> {
    Property::Constant(Color::WHITE)
}
fn line_height() -> f64 {
    1.15
}
fn is_line_height(v: &f64) -> bool {
    *v == 1.15
}
fn is_center(a: &Align) -> bool {
    *a == Align::Center
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Glyph,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    #[default]
    Forward,
    Reverse,
    /// Centre out.
    Center,
    /// Edges in.
    Edges,
    Random,
}

/// Per-unit transform offsets. Positions add, scale multiplies, opacity
/// multiplies. Missing fields are the identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
}

/// Animates glyphs, words or lines from `from` to `to`, one unit after
/// another (`stagger`), in `order`. The first unit's `anchor` lands on `at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnimator {
    #[serde(default)]
    pub unit: Unit,
    #[serde(default)]
    pub order: Order,
    #[serde(default)]
    pub seed: u32,
    #[serde(default = "one_frame")]
    pub stagger: Duration,
    pub at: TimeRef,
    #[serde(default = "start_anchor")]
    pub anchor: Anchor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<Lead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<Duration>,
    #[serde(default = "enter_ease")]
    pub ease: Ease,
    #[serde(default)]
    pub from: UnitState,
    #[serde(default)]
    pub to: UnitState,
    /// 1-based inclusive unit range [first, last]; default all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[usize; 2]>,
}

fn one_frame() -> Duration {
    Duration::Frames(1.0)
}
fn start_anchor() -> Anchor {
    Anchor::Start
}
fn enter_ease() -> Ease {
    Ease::Preset(crate::curve::Preset::Enter)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageLayer {
    /// Relative to the project folder.
    pub src: String,
    /// Display size in pixels; default the image's own size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Property<Vec2>>,
}

/// Footage: a video file (mp4, mov, webm, mkv; alpha from ProRes 4444,
/// VP8/VP9 alpha or PNG/QT-RLE) or an image sequence (`"renders/shot_####.png"`,
/// Blender-style, or a folder of frames).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoLayer {
    pub src: String,
    /// Display size in pixels; default the footage's own size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Property<Vec2>>,
    /// Source time (seconds) shown at the layer's start.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub trim: f64,
    /// Playback rate (2 = twice as fast).
    #[serde(default = "one_f64", skip_serializing_if = "is_one_f64")]
    pub speed: f64,
    /// Restart from `trim` when the source ends; otherwise hold the last frame.
    #[serde(default, rename = "loop", skip_serializing_if = "std::ops::Not::not")]
    pub looped: bool,
    /// Time remap: the source time (seconds) to show, animatable. Overrides
    /// trim and speed (freeze frames, speed ramps, reverse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_remap: Option<Property<f64>>,
    /// Image sequences: frames per second (default the project's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<Fps>,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub volume_db: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mute: bool,
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}
fn one_f64() -> f64 {
    1.0
}
fn is_one_f64(v: &f64) -> bool {
    *v == 1.0
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompLayer {
    #[serde(default)]
    pub layers: Vec<Layer>,
}

/// Depth-first walk over a layer tree.
pub fn walk_layers<'a>(layers: &'a [Layer], f: &mut dyn FnMut(&'a Layer)) {
    for layer in layers {
        f(layer);
        walk_layers(layer.children(), f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_round_trip_and_reject_typos() {
        let json = serde_json::json!({
            "id": "title", "kind": "text", "text": "MOVE", "size": 140,
            "transform": {"position": [960, 540], "opacity": {"keys": [{"at": 0, "v": 0}, {"at": 0.5, "v": 1}]}},
            "time": {"start": "beat:1", "step_fps": 12},
            "animators": [{"at": "beat:1", "from": {"y": 40, "opacity": 0}, "stagger": "1f"}]
        });
        let layer = Layer::from_json(json.clone()).unwrap();
        assert!(matches!(layer.kind, LayerKind::Text(_)));
        let back = serde_json::to_value(&layer).unwrap();
        assert_eq!(Layer::from_json(back).unwrap(), layer);
        let mut typo = json.clone();
        typo["sizee"] = 3.into();
        let err = Layer::from_json(typo).unwrap_err();
        assert!(err.contains("sizee"), "{err}");
        let shape = Layer::from_json(serde_json::json!({
            "id": "dot", "kind": "shape", "shape": "circle", "size": 40, "fill": "#ff3355",
            "stroke": {"color": "#000", "width": 3}
        }))
        .unwrap();
        let LayerKind::Shape(s) = &shape.kind else { panic!() };
        assert!(matches!(s.fill, Some(Paint::Solid(_))));
        let grad = Layer::from_json(serde_json::json!({
            "id": "g", "kind": "shape", "shape": "rect",
            "fill": {"gradient": "linear", "from": [-50, 0], "to": [50, 0], "stops": [[0, "#f00"], [1, "#00f"]]}
        }))
        .unwrap();
        let LayerKind::Shape(s) = &grad.kind else { panic!() };
        assert!(matches!(s.fill, Some(Paint::Gradient(_))));
    }
}
