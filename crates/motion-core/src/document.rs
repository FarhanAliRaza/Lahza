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
    /// Narration lines and their word timings; `narration.json` on disk.
    #[serde(skip)]
    pub narration: Option<crate::narration::Narration>,
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
            narration: None,
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
    /// How exports draw meshes: the preview's rasteriser (default) or the
    /// path tracer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<RendererKind>,
    /// Path tracing: paths per pixel (default 64; draft exports use a
    /// quarter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<u32>,
}

/// The export renderer for meshes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererKind {
    /// Depth-buffered raster, as the preview draws.
    Raster,
    /// Path traced on the GPU (ray queries), rasterised where unsupported.
    Path,
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
            renderer: None,
            samples: None,
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
    /// Narration lines from `narration_import` (one clip per line, owned by
    /// `narration.json`: re-importing replaces them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub narration: Vec<AudioClip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sfx: Vec<AudioClip>,
}

impl Audio {
    pub fn is_empty(&self) -> bool {
        self.music.is_none() && self.vo.is_none() && self.narration.is_empty() && self.sfx.is_empty()
    }
    pub fn clips(&self) -> impl Iterator<Item = &AudioClip> {
        self.music.iter().chain(self.vo.iter()).chain(self.narration.iter()).chain(self.sfx.iter())
    }
    /// The voice: the single `vo` clip and the narration lines.
    pub fn voice(&self) -> impl Iterator<Item = &AudioClip> {
        self.vo.iter().chain(self.narration.iter())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioClip {
    /// Stable id for edits (`whoosh1.at`); music and narration answer to
    /// "music" and "vo" when they have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Shown in editors; defaults to the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// Seconds to play; the rest of the file is cut. None plays it all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    /// Seconds of fade at the start and at the end of what plays.
    #[serde(default, skip_serializing_if = "zero")]
    pub fade_in: f64,
    #[serde(default, skip_serializing_if = "zero")]
    pub fade_out: f64,
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
    /// Animatable (keys fade the backdrop from one colour to another).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Property<Color>>,
    /// How this scene arrives from the previous one (default: a cut).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
    #[serde(default, skip_serializing_if = "Camera::is_default")]
    pub camera: Camera,
    /// Lights for `mesh` layers (default: a key, a rim and a little fill).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    /// What glossy meshes reflect (default: the studio environment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
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
            lights: Vec::new(),
            environment: None,
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
    /// 3D layers: vertical field of view in degrees (39.6 maps z = 0 1:1).
    #[serde(default = "default_fov", skip_serializing_if = "is_default_fov")]
    pub fov: Property<f64>,
    /// 3D layers: [yaw, pitch] degrees around the point of interest.
    #[serde(default = "zero_vec", skip_serializing_if = "is_zero_vec")]
    pub orbit: Property<Vec2>,
    /// 3D layers: move toward the point of interest (px).
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub dolly: Property<f64>,
    /// 3D layers: lens shift in px: moves the picture of the 3D scene
    /// without turning the camera, so verticals stay parallel (frame a tall
    /// object from below without its sides converging).
    #[serde(default = "zero_vec", skip_serializing_if = "is_zero_vec")]
    pub shift: Property<Vec2>,
    /// Depth of field: the sharp plane, as a depth like a layer's `z`
    /// (0 = the point of interest's plane) or a layer id to keep sharp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<Focus>,
    /// Depth of field: blur radius (px) per 100 px of depth away from the
    /// focus plane; 0 turns it off. 3D layers only.
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub aperture: Property<f64>,
}

/// Where the camera focuses: a depth (animatable, for rack focus) or a
/// layer whose depth it follows.
#[derive(Clone, Debug, PartialEq)]
pub enum Focus {
    Depth(Property<f64>),
    Layer(String),
}

impl Serialize for Focus {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Focus::Depth(p) => p.serialize(s),
            Focus::Layer(id) => id.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Focus {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(id) => Ok(Focus::Layer(id)),
            value => serde_json::from_value(value).map(Focus::Depth).map_err(|e| D::Error::custom(format!("focus: a depth or a layer id: {e}"))),
        }
    }
}

fn default_fov() -> Property<f64> {
    Property::Constant(39.6)
}
fn is_default_fov(p: &Property<f64>) -> bool {
    *p == default_fov()
}

impl Default for Camera {
    fn default() -> Self {
        Self { position: None, zoom: one_prop(), rotation: zero_prop(), fov: default_fov(), orbit: zero_vec(), dolly: zero_prop(), shift: zero_vec(), focus: None, aperture: zero_prop() }
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
fn is_zero_prop(p: &Property<f64>) -> bool {
    *p == zero_prop()
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
    /// 3D layers: depth (away from the viewer).
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub z: Property<f64>,
    /// 3D layers: tilt about x and turn about y, degrees.
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub rotation_x: Property<f64>,
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub rotation_y: Property<f64>,
    /// 3D layers: depth scale relative to `scale` (depth scales with the
    /// mean of |scale x| and |scale y|, times this; default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_z: Option<Property<f64>>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: zero_vec(),
            anchor: zero_vec(),
            scale: one_vec(),
            rotation: zero_prop(),
            opacity: one_prop(),
            z: zero_prop(),
            rotation_x: zero_prop(),
            rotation_y: zero_prop(),
            scale_z: None,
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
    Overlay,
    Darken,
    Lighten,
    #[serde(rename = "color_dodge")]
    ColorDodge,
    #[serde(rename = "color_burn")]
    ColorBurn,
    #[serde(rename = "hard_light")]
    HardLight,
    #[serde(rename = "soft_light")]
    SoftLight,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    /// Shader code of the mode.
    pub fn code(self) -> u32 {
        self as u32
    }
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
    /// Exposure in stops.
    Exposure {
        #[serde(default = "zero_prop")]
        stops: Property<f64>,
    },
    /// Brightness −1…1 and contrast −1…1 (0 leaves the picture alone).
    #[serde(rename = "brightness_contrast")]
    BrightnessContrast {
        #[serde(default = "zero_prop")]
        brightness: Property<f64>,
        #[serde(default = "zero_prop")]
        contrast: Property<f64>,
    },
    /// Input black/white points, gamma, output black/white (0…1).
    Levels {
        #[serde(default = "zero_prop")]
        in_black: Property<f64>,
        #[serde(default = "one_prop")]
        in_white: Property<f64>,
        #[serde(default = "one_prop")]
        gamma: Property<f64>,
        #[serde(default = "zero_prop")]
        out_black: Property<f64>,
        #[serde(default = "one_prop")]
        out_white: Property<f64>,
    },
    /// Hue rotation in degrees, saturation and lightness offsets (−1…1).
    #[serde(rename = "hue_saturation")]
    HueSaturation {
        #[serde(default = "zero_prop")]
        hue: Property<f64>,
        #[serde(default = "zero_prop")]
        saturation: Property<f64>,
        #[serde(default = "zero_prop")]
        lightness: Property<f64>,
    },
    /// Map blacks to `black` and whites to `white`.
    Tint {
        #[serde(default = "black_prop")]
        black: Property<Color>,
        #[serde(default = "white")]
        white: Property<Color>,
        #[serde(default = "one_prop")]
        amount: Property<f64>,
    },
    /// Map luminance through a gradient (duotones, heat maps).
    #[serde(rename = "gradient_map")]
    GradientMap {
        stops: Vec<(f64, Color)>,
        #[serde(default = "one_prop")]
        amount: Property<f64>,
    },
    Invert {
        #[serde(default = "one_prop")]
        amount: Property<f64>,
    },
    /// Replace colour, keep alpha.
    Fill {
        color: Property<Color>,
        #[serde(default = "one_prop")]
        amount: Property<f64>,
    },
    /// A 3D LUT from a .cube file (project-relative).
    Lut {
        path: String,
        #[serde(default = "one_prop")]
        amount: Property<f64>,
    },
    /// Turbulent displacement in pixels; `scale` is the feature size in px,
    /// `speed` how fast the field evolves (per second).
    Displace {
        #[serde(default = "ten")]
        amount: Property<f64>,
        #[serde(default = "hundred_f")]
        scale: Property<f64>,
        #[serde(default = "one_prop")]
        speed: Property<f64>,
        #[serde(default)]
        seed: u32,
    },
    /// Motion-style blur along `angle` (degrees) over `length` px.
    #[serde(rename = "directional_blur")]
    DirectionalBlur {
        #[serde(default = "zero_prop")]
        angle: Property<f64>,
        #[serde(default = "twelve")]
        length: Property<f64>,
    },
    /// Chromatic aberration: red and blue shifted apart by `amount` px.
    #[serde(rename = "rgb_split")]
    RgbSplit {
        #[serde(default = "four_f")]
        amount: Property<f64>,
        #[serde(default = "zero_prop")]
        angle: Property<f64>,
    },
    Vignette {
        #[serde(default = "half_f")]
        amount: Property<f64>,
        #[serde(default = "half_f")]
        softness: Property<f64>,
    },
    /// Film grain; a new pattern each frame (held on stepped frames).
    Grain {
        #[serde(default = "grain_amount")]
        amount: Property<f64>,
        #[serde(default = "one_prop")]
        size: Property<f64>,
        #[serde(default)]
        seed: u32,
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
fn black_prop() -> Property<Color> {
    Property::Constant(Color::BLACK)
}
fn hundred_f() -> Property<f64> {
    Property::Constant(100.0)
}
fn four_f() -> Property<f64> {
    Property::Constant(4.0)
}
fn half_f() -> Property<f64> {
    Property::Constant(0.5)
}
fn grain_amount() -> Property<f64> {
    Property::Constant(0.08)
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

/// Rides a sibling path layer: the layer is placed on the path's drawn
/// geometry (its morph, trim and full transform) at every evaluated time,
/// replacing its own `position` (and `parent`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    /// A sibling `shape: "path"` layer.
    pub layer: String,
    #[serde(default)]
    pub at: FollowAt,
    /// Also turn along the path's direction (added to `rotation`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rotate: bool,
    /// Added to the point, in the path's direction frame when `rotate`.
    #[serde(default = "zero_vec", skip_serializing_if = "is_zero_vec")]
    pub offset: Property<Vec2>,
}

/// Where on the path: the drawn head (`trim[1]`), its tail (`trim[0]`),
/// or a fraction of the full length (animatable).
#[derive(Clone, Debug, PartialEq)]
pub enum FollowAt {
    Head,
    Tail,
    Fraction(Property<f64>),
}

impl Default for FollowAt {
    fn default() -> Self {
        FollowAt::Head
    }
}

impl Serialize for FollowAt {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            FollowAt::Head => s.serialize_str("head"),
            FollowAt::Tail => s.serialize_str("tail"),
            FollowAt::Fraction(p) => p.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for FollowAt {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(d)?;
        match value.as_str() {
            Some("head") => Ok(FollowAt::Head),
            Some("tail") => Ok(FollowAt::Tail),
            Some(other) => Err(D::Error::custom(format!("follow.at: \"head\", \"tail\" or a fraction 0..1, not \"{other}\""))),
            None => serde_json::from_value(value).map(FollowAt::Fraction).map_err(|e| D::Error::custom(format!("follow.at: {e}"))),
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow: Option<Follow>,
    /// Place the layer in 3D (z, rotation_x/y, the scene's 3D camera);
    /// 3D layers sort by depth among their neighbours.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub three_d: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// "screen": placed in canvas coordinates, untouched by the scene's
    /// camera (2D moves, orbit, dolly, perspective) — titles and finishing
    /// layers over a moving 3D shot. Default "world".
    #[serde(default, skip_serializing_if = "LayerSpace::is_world")]
    pub space: LayerSpace,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerSpace {
    #[default]
    World,
    Screen,
}

impl LayerSpace {
    pub fn is_world(&self) -> bool {
        *self == LayerSpace::World
    }
}

fn is_normal(b: &Blend) -> bool {
    *b == Blend::Normal
}

const COMMON_FIELDS: &[&str] = &["name", "transform", "time", "blend", "effects", "parent", "matte", "follow", "three_d", "hidden", "space"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum LayerKind {
    Shape(ShapeLayer),
    Text(TextLayer),
    Image(ImageLayer),
    Video(VideoLayer),
    Comp(CompLayer),
    Instancer(InstancerLayer),
    Mesh(MeshLayer),
    Component(ComponentLayer),
}

/// A component (`crate::components`): its generator expands `params` to
/// children at compile time; `overrides` patches children by id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentLayer {
    pub component: String,
    #[serde(default = "empty_object")]
    pub params: serde_json::Value,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub overrides: serde_json::Map<String, serde_json::Value>,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

impl LayerKind {
    pub fn name(&self) -> &'static str {
        match self {
            LayerKind::Shape(_) => "shape",
            LayerKind::Text(_) => "text",
            LayerKind::Image(_) => "image",
            LayerKind::Video(_) => "video",
            LayerKind::Comp(_) => "comp",
            LayerKind::Instancer(_) => "instancer",
            LayerKind::Mesh(_) => "mesh",
            LayerKind::Component(_) => "component",
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
            return Err(format!("layer '{id}' needs a \"kind\": shape, text, image, video, comp, instancer, mesh or component"));
        }
        let kind: LayerKind =
            serde_json::from_value(serde_json::Value::Object(map)).map_err(|e| format!("layer '{id}': {e}"))?;
        Ok(Layer { id, common, kind })
    }

    pub fn children(&self) -> &[Layer] {
        match &self.kind {
            LayerKind::Comp(c) => &c.layers,
            LayerKind::Instancer(i) => &i.layers,
            _ => &[],
        }
    }
    pub fn children_mut(&mut self) -> Option<&mut Vec<Layer>> {
        match &mut self.kind {
            LayerKind::Comp(c) => Some(&mut c.layers),
            LayerKind::Instancer(i) => Some(&mut i.layers),
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
    /// A `size` box filled with a smooth blend of coloured `points`
    /// (mesh/blob gradient backgrounds); `radius` rounds the corners.
    Field,
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
    /// Path for `shape: "path"`: SVG path data ("M -50 0 C …"), or keys
    /// that morph between paths: `{"keys": [{"at": …, "v": "M …"}, {"at":
    /// "beat:5", "v": {"shape": "star", "size": 200}, "ease": "gesture"}]}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<PathData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    /// Field: the coloured points blended across the box.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<FieldPoint>,
    /// Field: how far colours blend into each other (default 1; 0.25 is
    /// crisp, 2 very soft).
    #[serde(default = "one_prop", skip_serializing_if = "is_one")]
    pub softness: Property<f64>,
    /// Field: noise displacement of the blend in px (organic edges).
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub warp: Property<f64>,
    /// Field: the warp noise's feature size in px.
    #[serde(default = "warp_scale", skip_serializing_if = "is_warp_scale")]
    pub warp_scale: Property<f64>,
}

/// One coloured point of a field shape (layer space, like `d`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldPoint {
    pub position: Property<Vec2>,
    pub color: Property<Color>,
    /// Reach of the colour in px: inside it the colour holds, beyond it it
    /// gives way to the other points.
    #[serde(default = "two_hundred")]
    pub radius: Property<f64>,
}

fn two_hundred() -> Property<f64> {
    Property::Constant(200.0)
}
fn warp_scale() -> Property<f64> {
    Property::Constant(300.0)
}
fn is_warp_scale(p: &Property<f64>) -> bool {
    *p == warp_scale()
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

/// Path data: one path, or keys morphing between paths.
#[derive(Clone, Debug, PartialEq)]
pub enum PathData {
    Static(PathValue),
    Keys(Vec<PathKey>),
}

/// SVG path data, or a primitive converted to a path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathValue {
    Svg(String),
    Shape(PathShape),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathShape {
    /// circle, rect, capsule, polygon, star
    pub shape: ShapeKind,
    #[serde(default = "hundred_vec")]
    pub size: Vec2,
    #[serde(default)]
    pub radius: f64,
    #[serde(default = "five")]
    pub sides: u32,
    #[serde(default = "half")]
    pub inner: f64,
}

fn hundred_vec() -> Vec2 {
    Vec2::new(100.0, 100.0)
}
fn half() -> f64 {
    0.5
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathKey {
    #[serde(alias = "t")]
    pub at: TimeRef,
    #[serde(alias = "value", alias = "to")]
    pub v: PathValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Ease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<Lead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<Duration>,
}

impl Serialize for PathData {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            PathData::Static(v) => v.serialize(s),
            PathData::Keys(k) => serde_json::json!({"keys": k}).serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for PathData {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(d)?;
        if let Some(keys) = value.get("keys") {
            let keys: Vec<PathKey> = serde_json::from_value(keys.clone()).map_err(D::Error::custom)?;
            if keys.is_empty() {
                return Err(D::Error::custom("path keys must not be empty"));
            }
            return Ok(PathData::Keys(keys));
        }
        serde_json::from_value(value).map(PathData::Static).map_err(|e| D::Error::custom(format!("path: SVG path data or {{\"shape\": …}}: {e}")))
    }
}

impl PathValue {
    /// SVG path data for this value (primitives become closed outlines,
    /// starting at the top and running clockwise).
    pub fn svg(&self) -> String {
        match self {
            PathValue::Svg(d) => d.clone(),
            PathValue::Shape(p) => primitive_path(p),
        }
    }
}

fn primitive_path(p: &PathShape) -> String {
    let (hw, hh) = (p.size.x() / 2.0, p.size.y() / 2.0);
    let pts = |n: usize, f: &dyn Fn(usize) -> (f64, f64)| -> String {
        let mut d = String::new();
        for i in 0..n {
            let (x, y) = f(i);
            d.push_str(&format!("{} {:.4} {:.4} ", if i == 0 { "M" } else { "L" }, x, y));
        }
        d.push('Z');
        d
    };
    match p.shape {
        ShapeKind::Circle | ShapeKind::Ring | ShapeKind::Arc => {
            // Four cubic arcs (k = 0.5523), clockwise from the top.
            let k = 0.552_284_75;
            format!(
                "M 0 {t} C {a} {t} {r} {b} {r} 0 C {r} {c} {a} {bt} 0 {bt} C {na} {bt} {nr} {c} {nr} 0 C {nr} {b} {na} {t} 0 {t} Z",
                t = -hh, r = hw, bt = hh, nr = -hw, a = k * hw, na = -k * hw, b = -k * hh, c = k * hh
            )
        }
        ShapeKind::Polygon | ShapeKind::Star => {
            let n = p.sides.max(3) as usize;
            let inner = if p.shape == ShapeKind::Star { p.inner } else { 1.0 };
            let count = if p.shape == ShapeKind::Star { n * 2 } else { n };
            pts(count, &|i| {
                let a = std::f64::consts::TAU * i as f64 / count as f64;
                let r = if i % 2 == 1 && p.shape == ShapeKind::Star { hw * inner } else { hw };
                (r * a.sin(), -r * a.cos())
            })
        }
        ShapeKind::Capsule => {
            let r = hw.min(hh);
            format!("M {x0} {t} L {x1} {t} A {r} {r} 0 0 1 {x1} {b} L {x0} {b} A {r} {r} 0 0 1 {x0} {t} Z", x0 = -hw + r, x1 = hw - r, t = -r, b = r, r = r)
        }
        ShapeKind::Line => format!("M {} 0 L {} 0", -hw, hw),
        _ => {
            let r = p.radius.clamp(0.0, hw.min(hh));
            if r <= 0.0 {
                format!("M 0 {t} L {hw} {t} L {hw} {b} L {nw} {b} L {nw} {t} Z", t = -hh, b = hh, hw = hw, nw = -hw)
            } else {
                format!(
                    "M 0 {t} L {x1} {t} A {r} {r} 0 0 1 {hw} {y0} L {hw} {y1} A {r} {r} 0 0 1 {x1} {b} L {x0} {b} A {r} {r} 0 0 1 {nw} {y1} L {nw} {y0} A {r} {r} 0 0 1 {x0} {t} Z",
                    t = -hh, b = hh, hw = hw, nw = -hw, x0 = -hw + r, x1 = hw - r, y0 = -hh + r, y1 = hh - r, r = r
                )
            }
        }
    }
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
    /// Points: linear from→to; radial centre→edge. In `units`.
    pub from: Vec2,
    pub to: Vec2,
    /// [[offset, "#colour"], …], offsets in 0..1.
    pub stops: Vec<(f64, Color)>,
    /// "layer" (default): `from`/`to` in layer px. "box": fractions of the
    /// shape's box, [0, 0] top-left to [1, 1] bottom-right, so the gradient
    /// stretches with an animated `size` (radials become ellipses).
    #[serde(default, skip_serializing_if = "is_default")]
    pub units: GradientUnits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GradientKind {
    Linear,
    Radial,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GradientUnits {
    #[default]
    Layer,
    Box,
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
    /// Ignored when `gradient` is set.
    #[serde(default = "white_prop")]
    pub color: Property<Color>,
    #[serde(default = "four")]
    pub width: Property<f64>,
    /// Ends of open strokes and dashes.
    #[serde(default, skip_serializing_if = "is_default")]
    pub cap: LineCap,
    /// Corners between segments.
    #[serde(default, skip_serializing_if = "is_default")]
    pub join: LineJoin,
    /// Miter joins longer than this many half-widths are bevelled.
    #[serde(default = "four_f64", skip_serializing_if = "is_four")]
    pub miter_limit: f64,
    /// Alternating dash and gap lengths in px.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f64>,
    /// Shift of the dash pattern along the stroke, px (animatable: marching
    /// ants, a dash running along a route).
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero_prop")]
    pub dash_offset: Property<f64>,
    /// Fade over this many px at the stroke's far end (the drawing head
    /// while `trim` draws it on).
    #[serde(default, skip_serializing_if = "is_default")]
    pub feather: f64,
    /// Fade over this many px at the stroke's start (a fading tail).
    #[serde(default, skip_serializing_if = "is_default")]
    pub feather_start: f64,
    /// Path strokes: colour along the stroke instead of `color`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<StrokeGradient>,
}

/// A stroke colour that changes along the path: offset 0 is the path's
/// start, 1 its end, over the full path (trim reveals it; the colours do
/// not slide while it draws on).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrokeGradient {
    /// [[offset, "#colour"], …], offsets in 0..1.
    pub stops: Vec<(f64, Color)>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub along: Along,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Along {
    /// Arc length of the full path.
    #[default]
    Length,
}

fn white_prop() -> Property<Color> {
    Property::Constant(Color::WHITE)
}

/// How a path stroke is outlined, when it is not the default round caps
/// and joins with no dashes or feather.
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeStyle {
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f64,
    pub dash: Vec<f64>,
    pub dash_offset: f64,
    pub feather: f64,
    pub feather_start: f64,
}

impl Stroke {
    pub fn style(&self) -> Option<StrokeStyle> {
        let dash: Vec<f64> = self.dash.iter().map(|d| d.max(0.0)).collect();
        let dashed = dash.iter().sum::<f64>() > 0.0;
        let plain = self.cap == LineCap::Round && self.join == LineJoin::Round && !dashed && self.feather <= 0.0 && self.feather_start <= 0.0;
        (!plain).then(|| StrokeStyle {
            cap: self.cap,
            join: self.join,
            miter_limit: self.miter_limit.max(1.0),
            dash: if dashed { dash } else { Vec::new() },
            // The current value is set per frame (see compile).
            dash_offset: 0.0,
            feather: self.feather.max(0.0),
            feather_start: self.feather_start.max(0.0),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineCap {
    #[default]
    Round,
    Butt,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineJoin {
    #[default]
    Round,
    Miter,
    Bevel,
}

fn four() -> Property<f64> {
    Property::Constant(4.0)
}

fn four_f64() -> f64 {
    4.0
}

fn is_four(v: &f64) -> bool {
    *v == 4.0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
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
    /// Outline drawn from the glyphs' distance field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<TextStroke>,
    /// Faux bold: px the glyph edges move outward (negative thins).
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub weight: Property<f64>,
    /// Directional rim light along the inside of the glyph edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<TextEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animators: Vec<TextAnimator>,
}

/// Text outline; `width` in px, `align` places it outside (default), centred
/// on or inside the glyph edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextStroke {
    pub color: Property<Color>,
    #[serde(default = "four")]
    pub width: Property<f64>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub align: StrokeAlign,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrokeAlign {
    #[default]
    Outside,
    Center,
    Inside,
}

/// Rim light: `color` laid over the fill within `width` px inside the edge,
/// strongest where the edge faces the light. `angle` is where the light
/// comes from, degrees (0 = above, clockwise).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdge {
    #[serde(default = "white")]
    pub color: Property<Color>,
    #[serde(default = "three")]
    pub width: Property<f64>,
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub angle: Property<f64>,
}

fn three() -> Property<f64> {
    Property::Constant(3.0)
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

/// Animator order: by unit index (`"forward"`, …) or by position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UnitOrder {
    Index(Order),
    Spatial(SpatialOrder),
}

impl Default for UnitOrder {
    fn default() -> Self {
        UnitOrder::Index(Order::Forward)
    }
}

/// Position-driven order: each unit's delay is its position across the
/// text (0…1) times the stagger span. Either `axis` (`"x"`, `"y"` or the
/// sweep direction in degrees, 0 = rightward, clockwise) with `from`, or
/// `radial`: a centre in fractions of the text block ([0.5, 0.5] = middle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialOrder {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<Axis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radial: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub from: SweepFrom,
    /// Soft reveal: opacity becomes a gradient wipe this wide (fraction of
    /// the text's extent) instead of whole units fading.
    #[serde(default, skip_serializing_if = "zero")]
    pub feather: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Axis {
    Named(AxisName),
    Degrees(f64),
}

impl Axis {
    /// Sweep direction in degrees (0 = +x, 90 = +y).
    pub fn degrees(self) -> f64 {
        match self {
            Axis::Named(AxisName::X) => 0.0,
            Axis::Named(AxisName::Y) => 90.0,
            Axis::Degrees(d) => d,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AxisName {
    X,
    Y,
}

/// Where a spatial sweep begins: the axis' start, its end, or its middle
/// (outward). For `radial`, `end` runs from the outside in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SweepFrom {
    #[default]
    Start,
    End,
    Center,
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
    /// Fill colour; missing is the text's own. A colour applies once the
    /// unit's move has started (a word lights up when it is spoken, and a
    /// later animator takes it back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

/// Animates glyphs, words or lines from `from` to `to`, one unit after
/// another (`stagger`), in `order`. The first unit's `anchor` lands on `at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnimator {
    #[serde(default)]
    pub unit: Unit,
    #[serde(default)]
    pub order: UnitOrder,
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

/// Repeats its `layers` (drawn like a comp) once per point of `layout`;
/// each copy can start later (`stagger`), vary at random (`random`), take
/// a colour from `palette` and be shaped by `fields`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstancerLayer {
    pub layout: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<Stagger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random: Option<Spread>,
    /// Colours given to the copies in turn (`palette_order`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub palette: Vec<Color>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub palette_order: PaletteOrder,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    /// The template, drawn once per copy centred on its point.
    #[serde(default)]
    pub layers: Vec<Layer>,
}

/// Where the copies sit, in the instancer's own space (centred on its
/// origin, like shapes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Layout {
    /// `columns` × `rows` cells `spacing` apart; `hex` shifts odd rows by
    /// half a cell.
    Grid {
        columns: u32,
        rows: u32,
        #[serde(default = "fifty_vec")]
        spacing: Property<Vec2>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        hex: bool,
    },
    /// `count` copies on a circle from `start` over `sweep` degrees (0 =
    /// up, clockwise). `align` turns each copy to its angle (up points out).
    Radial {
        count: u32,
        #[serde(default = "two_hundred")]
        radius: Property<f64>,
        #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
        start: Property<f64>,
        #[serde(default = "full_turn")]
        sweep: Property<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        align: bool,
    },
    /// `count` copies from `from` to `to`, ends included; `align` turns
    /// their x axis along the line.
    Line {
        count: u32,
        #[serde(default = "line_from")]
        from: Property<Vec2>,
        #[serde(default = "line_to")]
        to: Property<Vec2>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        align: bool,
    },
    /// `count` copies scattered over a `size` box (or the ellipse in it).
    Random {
        count: u32,
        #[serde(default = "thousand_vec")]
        size: Property<Vec2>,
        #[serde(default, skip_serializing_if = "is_default")]
        area: Area,
        #[serde(default)]
        seed: u32,
    },
    /// `count` copies spread evenly along a sibling path layer as drawn
    /// (morph and transform), moved along it by `offset` (a fraction of its
    /// length; closed paths wrap). `align` turns them along the path.
    Path {
        count: u32,
        layer: String,
        #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
        offset: Property<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        align: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Area {
    #[default]
    Box,
    Circle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaletteOrder {
    #[default]
    Cycle,
    Random,
}

fn fifty_vec() -> Property<Vec2> {
    Property::Constant(Vec2::new(50.0, 50.0))
}
fn full_turn() -> Property<f64> {
    Property::Constant(360.0)
}
fn line_from() -> Property<Vec2> {
    Property::Constant(Vec2::new(-500.0, 0.0))
}
fn line_to() -> Property<Vec2> {
    Property::Constant(Vec2::new(500.0, 0.0))
}
fn thousand_vec() -> Property<Vec2> {
    Property::Constant(Vec2::new(1000.0, 1000.0))
}

/// Copies start one after another: copy k sees time `t − delay(k)`, its
/// rank in `order` times `each`, or the ranks spread over `total`.
/// `order` center starts in the layout's middle and moves out in rings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stagger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<Duration>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub order: Order,
    #[serde(default, skip_serializing_if = "is_default")]
    pub seed: u32,
}

/// Seeded per-copy variation: [min, max] ranges (`scale` multiplies,
/// `rotation` degrees, `opacity` multiplies, `time` extra delay in
/// seconds), `position` a ± jitter in px, `color` a mix between two colours.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spread {
    #[serde(default)]
    pub seed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Vec2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[Color; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    /// 1 within `radius` − `falloff` of `center`, fading to 0 at `radius`.
    Radial,
    /// Ramps 0 → 1 over `range` px along `angle` (degrees), centred on `center`.
    Linear,
    /// Smooth noise with features `size` px across, evolving `speed` per second.
    Noise,
    /// A ring leaving `center` at time `at` at `speed` px/s; each of its
    /// `cycles` crests is `wavelength` px wide (0 cycles: endless ripples);
    /// `decay` px fades it with distance.
    Wave,
}

/// A field effector: a 0..1 value per copy (shaped by `ease`, flipped by
/// `invert`, weighted by `amount`) that scales deltas added to the copy's
/// `position`, `scale`, `rotation` and `opacity` and mixes in `color`.
/// Fields apply in list order; their parameters are animatable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    #[serde(rename = "type")]
    pub kind: FieldKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<Property<Vec2>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub falloff: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<TimeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wavelength: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycles: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Ease>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invert: bool,
    #[serde(default = "one_prop", skip_serializing_if = "is_one")]
    pub amount: Property<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Property<Vec2>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Property<Color>>,
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

    #[test]
    fn fields_stroke_gradients_box_units_and_focus_round_trip() {
        let round_trip = |json: serde_json::Value| {
            let layer = Layer::from_json(json).unwrap();
            assert_eq!(Layer::from_json(serde_json::to_value(&layer).unwrap()).unwrap(), layer);
            layer
        };
        let field = round_trip(serde_json::json!({
            "id": "bg", "kind": "shape", "shape": "field", "size": [1920, 1080], "softness": 0.6, "warp": 20,
            "points": [{"position": [0, 0], "color": "#ff3355"}, {"position": {"keys": [{"at": 0, "v": [100, 0]}]}, "color": "#2d5bff", "radius": 400}]
        }));
        let LayerKind::Shape(s) = &field.kind else { panic!() };
        assert_eq!((s.shape, s.points.len(), s.points[0].radius.clone()), (ShapeKind::Field, 2, Property::Constant(200.0)));
        let stroke = round_trip(serde_json::json!({
            "id": "p", "kind": "shape", "shape": "path", "d": "M 0 0 H 100",
            "stroke": {"width": 6, "gradient": {"stops": [[0, "#f00"], [1, "#00f"]], "along": "length"}},
            "fill": {"gradient": "radial", "units": "box", "from": [0.5, 0.5], "to": [1, 0.5], "stops": [[0, "#fff"]]}
        }));
        let LayerKind::Shape(s) = &stroke.kind else { panic!() };
        assert_eq!(s.stroke.as_ref().unwrap().color, Property::Constant(Color::WHITE), "colour may be left out");
        assert!(matches!(&s.fill, Some(Paint::Gradient(g)) if g.units == GradientUnits::Box));
        for (focus, expected) in [(serde_json::json!("hero"), Focus::Layer("hero".into())), (serde_json::json!(250), Focus::Depth(Property::Constant(250.0)))] {
            let cam: Camera = serde_json::from_value(serde_json::json!({"focus": focus, "aperture": 2})).unwrap();
            assert_eq!(cam.focus, Some(expected));
            assert_eq!(serde_json::from_value::<Camera>(serde_json::to_value(&cam).unwrap()).unwrap(), cam);
        }
    }
}

fn zero(v: &f64) -> bool {
    *v == 0.0
}

// ------------------------------------------------------------------ 3D

/// A lit 3D object, rendered in the engine with the scene's camera,
/// `lights` and `environment`. Always 3D: it sorts by depth with other
/// 3D layers, and meshes next to each other share one depth buffer (they
/// intersect correctly).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshLayer {
    pub mesh: MeshDef,
    #[serde(default, skip_serializing_if = "Material::is_empty")]
    pub material: Material,
    /// Materials for parts of the mesh (front, back, side, bevel, caps),
    /// over `material`.
    #[serde(default, skip_serializing_if = "Parts::is_empty")]
    pub parts: Parts,
    /// Split rings, revolves and sweeps into this many pieces.
    #[serde(default = "one_u32", skip_serializing_if = "is_one_u32")]
    pub segments: u32,
    /// Between pieces: degrees (rings, revolves) or a fraction of the
    /// length (sweeps).
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub gap: Property<f64>,
    /// One colour per piece, in turn (parts with their own colour keep it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<Property<Color>>,
    /// Relative sizes of the pieces, in turn (`[1, 2]`: every other piece
    /// twice as long). Default: all equal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<f64>,
    /// Tessellation (1 = smooth at the drawn size; 2 = twice as fine).
    #[serde(default = "one_f64", skip_serializing_if = "is_one_f64")]
    pub detail: f64,
}

fn one_u32() -> u32 {
    1
}
fn is_one_u32(v: &u32) -> bool {
    *v == 1
}

/// Mesh geometry. Space is the layer's: x right, y down, z away from the
/// viewer; the mesh is centred on the layer origin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum MeshDef {
    /// A flat rectangle (both sides): floors, walls, backdrops that take
    /// shadows. Tilt it with rotation_x (90 lays it flat).
    Plane {
        #[serde(default = "hundred")]
        size: Property<Vec2>,
    },
    Sphere {
        #[serde(default = "hundred_f")]
        radius: Property<f64>,
    },
    /// A box `size` [w, h] by `depth`, edges rounded by `bevel`.
    Box {
        #[serde(default = "hundred")]
        size: Property<Vec2>,
        #[serde(default = "hundred_f")]
        depth: Property<f64>,
        #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
        bevel: Property<f64>,
    },
    /// Standing along y; `top` gives a cone or frustum.
    Cylinder {
        #[serde(default = "fifty_f")]
        radius: Property<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        top: Option<Property<f64>>,
        #[serde(default = "hundred_f")]
        height: Property<f64>,
        #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
        bevel: Property<f64>,
    },
    /// Standing along y, `height` end to end.
    Capsule {
        #[serde(default = "fifty_f")]
        radius: Property<f64>,
        #[serde(default = "two_hundred")]
        height: Property<f64>,
    },
    /// A ring facing the camera: `radius` to the tube's centre, `tube` its
    /// radius, `angles` [from, to] like an arc (0 = up, clockwise).
    Torus {
        #[serde(default = "hundred_f")]
        radius: Property<f64>,
        #[serde(default = "twenty_f")]
        tube: Property<f64>,
        #[serde(default = "full_turn_vec", skip_serializing_if = "is_full_turn_vec")]
        angles: Property<Vec2>,
        #[serde(default, skip_serializing_if = "is_default")]
        caps: MeshCaps,
    },
    /// An outline (`d`: SVG path data, a primitive, or morph keys) pushed
    /// back `depth`, its edges rounded by `bevel`. Holes are kept.
    Extrude {
        d: PathData,
        #[serde(default = "forty_f")]
        depth: Property<f64>,
        #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
        bevel: Property<f64>,
    },
    /// `profile` (x = distance from the axis, y = position along it) turned
    /// about the `axis` ("y": standing up, "z": facing the camera).
    Revolve {
        profile: PathData,
        #[serde(default = "full_turn_vec", skip_serializing_if = "is_full_turn_vec")]
        angles: Property<Vec2>,
        #[serde(default)]
        axis: RevolveAxis,
    },
    /// A tube along `d` in the layer plane, drawn on by `trim` [from, to],
    /// with an optional ball (`head` radius) leading it.
    Sweep {
        d: PathData,
        #[serde(default = "ten")]
        radius: Property<f64>,
        #[serde(default, skip_serializing_if = "is_default")]
        profile: SweepShape,
        /// Ribbon: width across the path (standing out of the plane).
        #[serde(default = "forty_f", skip_serializing_if = "is_forty")]
        width: Property<f64>,
        #[serde(default = "full_trim", skip_serializing_if = "is_full_trim")]
        trim: Property<Vec2>,
        #[serde(default = "round_caps", skip_serializing_if = "is_round_caps")]
        caps: MeshCaps,
        #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
        head: Property<f64>,
    },
}

fn fifty_f() -> Property<f64> {
    Property::Constant(50.0)
}
fn forty_f() -> Property<f64> {
    Property::Constant(40.0)
}
fn is_forty(p: &Property<f64>) -> bool {
    *p == forty_f()
}
fn twenty_f() -> Property<f64> {
    Property::Constant(20.0)
}
fn full_turn_vec() -> Property<Vec2> {
    Property::Constant(Vec2::new(0.0, 360.0))
}
fn is_full_turn_vec(p: &Property<Vec2>) -> bool {
    *p == full_turn_vec()
}
fn round_caps() -> MeshCaps {
    MeshCaps::Round
}
fn is_round_caps(c: &MeshCaps) -> bool {
    *c == MeshCaps::Round
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeshCaps {
    #[default]
    Flat,
    Round,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SweepShape {
    #[default]
    Circle,
    Square,
    Ribbon,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RevolveAxis {
    #[default]
    Y,
    Z,
}

/// Surface look. Unset fields come from `preset` (default "glossy").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<MaterialPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Property<Color>>,
    /// 0 dielectric (plastic, paint) … 1 metal (reflections take the colour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metallic: Option<Property<f64>>,
    /// 0 mirror-sharp highlights … 1 fully diffuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roughness: Option<Property<f64>>,
    /// A clear lacquer layer over the base (car paint, coated plastic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearcoat: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearcoat_roughness: Option<Property<f64>>,
    /// Light the surface gives off (not lit, not shaded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive: Option<Property<Color>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive_strength: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Property<f64>>,
    /// Glass: the body lets this much of what is behind through; edges
    /// reflect more (fresnel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transmission: Option<Property<f64>>,
    /// Glass: index of refraction (1 = no bending, 1.5 glass, 2.4 diamond).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ior: Option<Property<f64>>,
    /// Glass: how far light travels inside before leaving, px (how much
    /// what is behind shifts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness: Option<Property<f64>>,
    /// Wireframe: draw the mesh's edges this many px wide instead of its
    /// surface (in `color`, unlit). Without `fill` the wires show through
    /// (the back edges too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wireframe: Option<Property<f64>>,
    /// Wireframe: fill the faces with this colour and hide the edges
    /// behind them (hidden-line viewport look).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Property<Color>>,
    /// One colour per piece, in turn, for this part (over the layer's
    /// `colors`): two-tone segmented rings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<Property<Color>>,
    /// Dielectric reflection strength: 0.5 (default) reflects 4% head-on,
    /// like most plastics and paints; 0 none, 1 twice as much.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular: Option<Property<f64>>,
    /// Tints reflections: a dielectric's (head-on) and a metal's edges
    /// (gold that turns paler, copper that turns redder at grazing angles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular_tint: Option<Property<Color>>,
    /// Brushed or spun metal: highlights stretch across the grain, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anisotropy: Option<Property<f64>>,
    /// Degrees the grain turns from running around the mesh's axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anisotropy_rotation: Option<Property<f64>>,
    /// Soft velvet-like light at grazing angles (fabric, peach skin), 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheen: Option<Property<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheen_color: Option<Property<Color>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheen_roughness: Option<Property<f64>>,
    /// The clear coat's tint (candy paint): what is under it is seen
    /// through this colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearcoat_color: Option<Property<Color>>,
    /// Glass: rainbow fringes, as 1 / Abbe number (0.02 window glass, 0.05
    /// flint; more for a stylised prism). Path tracing only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispersion: Option<Property<f64>>,
    /// Ignore lights: the flat colour (flat-shaded idents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlit: Option<bool>,
    /// Faceted: one normal per triangle (low-poly look).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faceted: Option<bool>,
}

impl Material {
    pub fn is_empty(&self) -> bool {
        *self == Material::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaterialPreset {
    Matte,
    Plastic,
    #[default]
    Glossy,
    /// Base colour under a clear lacquer: crisp reflections over colour.
    Coated,
    Metal,
    Chrome,
    /// Spun metal: grain around the mesh's axis.
    Brushed,
    /// Fabric: matte with a soft sheen at the edges.
    Velvet,
    Glass,
    Emissive,
    /// Unlit flat colour.
    Flat,
    /// Invisible except for the shadows falling on it (`color` tints them,
    /// `opacity` is their strength): drops meshes' shadows onto 2D layers.
    Shadow,
    /// Edges only (1.5 px, light grey, see-through): a 3D viewport look.
    Wireframe,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front: Option<Material>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<Material>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Material>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bevel: Option<Material>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<Material>,
    /// Revolves and rings: the surface facing the axis (inner wall).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<Material>,
}

impl Parts {
    pub fn is_empty(&self) -> bool {
        *self == Parts::default()
    }
    /// In `mesh::PART_*` order.
    pub fn list(&self) -> [Option<&Material>; 6] {
        [self.front.as_ref(), self.back.as_ref(), self.side.as_ref(), self.bevel.as_ref(), self.caps.as_ref(), self.inside.as_ref()]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightKind {
    /// Parallel light arriving from direction `from`.
    #[default]
    Directional,
    /// A light at `position` (world px, like a 3D layer's position and z).
    Point,
    /// Even light from everywhere.
    Ambient,
    /// A glowing panel (rect or disc) at `position` facing `aim`: soft
    /// light whose shadows soften with its `size`.
    Area,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AreaShape {
    #[default]
    Rect,
    Disc,
}

/// A light for mesh layers. Directions are in the scene's space (x right,
/// y down, z away): `from: [-1, -1, -1]` is a key from the upper left,
/// in front of the subject; `[1, -0.5, 1]` a rim from behind on the right.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Light {
    #[serde(rename = "type", default)]
    pub kind: LightKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Property<crate::value::Vec3>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Property<crate::value::Vec3>>,
    #[serde(default = "white_prop")]
    pub color: Property<Color>,
    #[serde(default = "one_prop", skip_serializing_if = "is_one")]
    pub intensity: Property<f64>,
    /// Cast shadows (default: only the first directional or point light).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<bool>,
    /// Shadow edge: 0 sharp … 1 very soft (default 0.35).
    #[serde(default = "shadow_softness", skip_serializing_if = "is_shadow_softness")]
    pub softness: Property<f64>,
    /// Point lights: px at which the light has faded to nothing (smoothly;
    /// default none: it lights everything evenly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Property<f64>>,
    /// Area lights: the world point the panel faces (default the canvas
    /// centre at z 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<Property<crate::value::Vec3>>,
    /// Area lights: [w, h] px (disc: its diameters); default [300, 300].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Property<crate::value::Vec2>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<AreaShape>,
}

fn shadow_softness() -> Property<f64> {
    Property::Constant(0.35)
}
fn is_shadow_softness(p: &Property<f64>) -> bool {
    *p == shadow_softness()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentPreset {
    /// Neutral studio: a soft top light, two strip boxes, dark floor.
    #[default]
    Studio,
    /// Bright and even (product shots on white).
    Soft,
    /// Warm low sun, cool sky.
    Sunset,
    /// Dark with thin bright strips (premium, glossy idents).
    Night,
    /// No reflections or ambient from the environment.
    None,
}

/// What glossy surfaces reflect and how much ambient light comes from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    #[serde(default)]
    pub preset: EnvironmentPreset,
    #[serde(default = "one_prop", skip_serializing_if = "is_one")]
    pub intensity: Property<f64>,
    /// Turns the environment about the vertical axis (degrees).
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero")]
    pub rotation: Property<f64>,
    /// Override the preset's colours (animatable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky: Option<Property<Color>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<Property<Color>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<Property<Color>>,
}

impl Default for Environment {
    fn default() -> Self {
        Self { preset: EnvironmentPreset::Studio, intensity: one_prop(), rotation: zero_prop(), sky: None, horizon: None, ground: None }
    }
}
