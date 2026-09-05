//! Shared scene composition for screenshots and recordings.
//!
//! A scene is a canvas (background + effects) with one media surface (the
//! screenshot or a decoded video frame) placed on it through a 2D/3D
//! transform. The same geometry and the same CPU compositor are used for the
//! editor preview, animated-screenshot export, and recording export, so what
//! the user sees is what the file contains.

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use super::pointer_timeline::PointerMotion;

use super::{
    cursor_assets::{self, CursorFamily},
    model::NormalizedPoint,
    overlays::pointer_press_effect_geometry,
    pointer_timeline::{PointerBitmap, PointerFrame, PointerTimelineOptions},
    viewport::{visible_rect, Tilt, ViewportFrame},
};

/// The preview canvas height every scene dimension is expressed against.
/// Padding, border thickness, corner radius, and shadow spread scale linearly
/// with the actual canvas height so the export matches the preview.
pub const REFERENCE_CANVAS_HEIGHT: f64 = 600.0;
/// Window title bar height at the reference canvas height.
const TITLE_BAR_HEIGHT: f64 = 30.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SceneBackground {
    Solid(u32),
    Gradient {
        colors: [u32; 3],
        angle_degrees: f64,
    },
    Wallpaper(PathBuf),
}

impl Default for SceneBackground {
    fn default() -> Self {
        Self::Solid(0x111214)
    }
}

/// Placement, scale, and 3D orientation of the media surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SceneTransform {
    /// 1.0 = fitted inside the padding.
    pub scale: f64,
    /// Offset as a fraction of half the canvas width (-1..1).
    pub position_x: f64,
    /// Offset as a fraction of half the canvas height (-1..1).
    pub position_y: f64,
    pub rotation_x: f64,
    pub rotation_y: f64,
    pub rotation_z: f64,
    /// 0..1 perspective strength (camera distance).
    pub perspective: f64,
    /// Rotation/scale pivot inside the media (0..1).
    pub anchor_x: f64,
    pub anchor_y: f64,
}

impl Default for SceneTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl SceneTransform {
    pub const IDENTITY: SceneTransform = SceneTransform {
        scale: 1.0,
        position_x: 0.0,
        position_y: 0.0,
        rotation_x: 0.0,
        rotation_y: 0.0,
        rotation_z: 0.0,
        perspective: 0.35,
        anchor_x: 0.5,
        anchor_y: 0.5,
    };
    pub const MIN_SCALE: f64 = 0.2;
    pub const MAX_SCALE: f64 = 4.0;

    pub fn is_identity(&self) -> bool {
        !self.has_rotation()
            && (self.scale - 1.0).abs() < 1e-6
            && self.position_x.abs() < 1e-6
            && self.position_y.abs() < 1e-6
    }

    pub fn has_rotation(&self) -> bool {
        self.rotation_x.abs() > 1e-6 || self.rotation_y.abs() > 1e-6 || self.rotation_z.abs() > 1e-6
    }

    pub fn clamped(mut self) -> Self {
        let finite = |value: f64, fallback: f64| {
            if value.is_finite() {
                value
            } else {
                fallback
            }
        };
        self.scale = finite(self.scale, 1.0).clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        self.position_x = finite(self.position_x, 0.0).clamp(-1.5, 1.5);
        self.position_y = finite(self.position_y, 0.0).clamp(-1.5, 1.5);
        self.rotation_x = finite(self.rotation_x, 0.0).clamp(-80.0, 80.0);
        self.rotation_y = finite(self.rotation_y, 0.0).clamp(-80.0, 80.0);
        self.rotation_z = finite(self.rotation_z, 0.0).clamp(-180.0, 180.0);
        self.perspective = finite(self.perspective, 0.35).clamp(0.0, 1.0);
        self.anchor_x = finite(self.anchor_x, 0.5).clamp(0.0, 1.0);
        self.anchor_y = finite(self.anchor_y, 0.5).clamp(0.0, 1.0);
        self
    }

    /// Adds an animated tilt on top of the authored rotation.
    pub fn with_tilt(mut self, tilt: Tilt) -> Self {
        self.rotation_x += tilt.x;
        self.rotation_y += tilt.y;
        self.rotation_z += tilt.z;
        self.clamped()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatermarkPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

impl WatermarkPosition {
    pub const ALL: [WatermarkPosition; 4] = [
        WatermarkPosition::TopLeft,
        WatermarkPosition::TopRight,
        WatermarkPosition::BottomLeft,
        WatermarkPosition::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WatermarkPosition::TopLeft => "Top left",
            WatermarkPosition::TopRight => "Top right",
            WatermarkPosition::BottomLeft => "Bottom left",
            WatermarkPosition::BottomRight => "Bottom right",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Watermark {
    pub text: String,
    pub position: WatermarkPosition,
    /// 0-100.
    pub opacity: u8,
    /// 0-100 relative size.
    pub size: u8,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            text: String::new(),
            position: WatermarkPosition::BottomRight,
            opacity: 70,
            size: 30,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraShape {
    #[default]
    Circle,
    Squircle,
    Square,
}

impl CameraShape {
    pub const ALL: [CameraShape; 3] = [
        CameraShape::Circle,
        CameraShape::Squircle,
        CameraShape::Square,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CameraShape::Circle => "Circle",
            CameraShape::Squircle => "Squircle",
            CameraShape::Square => "Square",
        }
    }
}

/// Picture-in-picture placement of the camera (webcam) clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CameraOverlay {
    pub enabled: bool,
    pub position: WatermarkPosition,
    /// Diameter as a percent of the canvas height (10-60).
    pub size: u8,
    pub shape: CameraShape,
    pub mirror: bool,
    /// Distance from the canvas edge as a percent of the canvas height.
    pub margin: u8,
    pub shadow: bool,
}

impl Default for CameraOverlay {
    fn default() -> Self {
        Self {
            enabled: true,
            position: WatermarkPosition::BottomRight,
            size: 24,
            shape: CameraShape::Circle,
            mirror: false,
            margin: 4,
            shadow: true,
        }
    }
}

impl CameraOverlay {
    /// The overlay rect on a canvas of this size.
    pub fn rect(&self, canvas_width: f64, canvas_height: f64) -> Rect {
        let side = canvas_height * (self.size.clamp(10, 60) as f64 / 100.0);
        let margin = canvas_height * (self.margin.min(20) as f64 / 100.0);
        let x = match self.position {
            WatermarkPosition::TopLeft | WatermarkPosition::BottomLeft => margin,
            WatermarkPosition::TopRight | WatermarkPosition::BottomRight => {
                canvas_width - margin - side
            }
        };
        let y = match self.position {
            WatermarkPosition::TopLeft | WatermarkPosition::TopRight => margin,
            WatermarkPosition::BottomLeft | WatermarkPosition::BottomRight => {
                canvas_height - margin - side
            }
        };
        Rect {
            x,
            y,
            width: side,
            height: side,
        }
    }

    pub fn radius(&self, rect: Rect) -> f64 {
        match self.shape {
            CameraShape::Circle => rect.width * 0.5,
            CameraShape::Squircle => rect.width * 0.18,
            CameraShape::Square => rect.width * 0.03,
        }
    }
}

/// How the reconstructed cursor is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PointerStyle {
    pub visible: bool,
    /// Percent, 50-250.
    pub scale: u8,
    pub click_effects: bool,
    pub click_color: u32,
    pub hide_when_idle: bool,
    pub shadow: bool,
    /// How tightly the smoothed cursor follows the recorded pointer.
    pub motion: PointerMotion,
    /// Artwork the pointer is drawn with: the captured bitmap or one of the
    /// shipped cursor styles.
    pub family: CursorFamily,
    /// Glide back to the starting position before the end so the video
    /// loops cleanly.
    pub loop_to_start: bool,
}

impl PointerStyle {
    /// Seconds of stillness before an idle cursor fades out.
    pub const IDLE_HIDE_DELAY: f64 = 1.5;

    /// Cursor-track build options implied by this style.
    pub fn timeline_options(self) -> PointerTimelineOptions {
        PointerTimelineOptions {
            fallback_artwork: None,
            hide_after_inactivity: Some(Self::IDLE_HIDE_DELAY),
            motion: self.motion,
            loop_to_start: self.loop_to_start,
        }
    }
}

impl Default for PointerStyle {
    fn default() -> Self {
        Self {
            visible: true,
            scale: 100,
            click_effects: true,
            click_color: 0x007aff,
            hide_when_idle: true,
            shadow: true,
            motion: PointerMotion::Default,
            family: CursorFamily::Recorded,
            loop_to_start: false,
        }
    }
}

/// Everything that styles a scene except the media itself.
/// Fake application window chrome drawn around the media: a title bar with
/// traffic lights so zooms and pans read as happening inside a window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowFrame {
    #[default]
    Off,
    Light,
    Dark,
}

impl WindowFrame {
    pub const ALL: [WindowFrame; 3] = [WindowFrame::Off, WindowFrame::Light, WindowFrame::Dark];

    pub fn label(self) -> &'static str {
        match self {
            WindowFrame::Off => "None",
            WindowFrame::Light => "Light",
            WindowFrame::Dark => "Dark",
        }
    }

    fn bar_color(self) -> Option<[u8; 3]> {
        match self {
            WindowFrame::Off => None,
            WindowFrame::Light => Some([0xe9, 0xe9, 0xeb]),
            WindowFrame::Dark => Some([0x2c, 0x2c, 0x30]),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SceneStyle {
    pub background: SceneBackground,
    /// Window chrome (title bar) above the media.
    pub window_frame: WindowFrame,
    /// 0-100, matches the inspector slider.
    pub padding: u8,
    /// 0-100, matches the inspector slider.
    pub corners: u8,
    /// 0-100 shadow strength.
    pub shadow: u8,
    /// 0 soft, 1 long, 2 glow, 3 crisp.
    pub shadow_style: usize,
    pub border: bool,
    pub border_thickness: u8,
    pub border_color: u32,
    pub border_opacity: u8,
    /// Canvas aspect ratio (width / height). `None` follows the media.
    pub aspect: Option<f64>,
    /// 0-100 blur of the background layer.
    pub background_blur: u8,
    /// 0-100 film grain on the background layer.
    pub background_noise: u8,
    /// 0-100 darkening toward the canvas edges.
    pub vignette: u8,
    pub transform: SceneTransform,
    pub watermark: Option<Watermark>,
    pub pointer: PointerStyle,
    /// How a camera clip is placed; the clip itself lives in the project.
    pub camera: CameraOverlay,
}

impl Default for SceneStyle {
    fn default() -> Self {
        Self {
            background: SceneBackground::default(),
            window_frame: WindowFrame::Off,
            padding: 20,
            corners: 12,
            shadow: 40,
            shadow_style: 0,
            border: false,
            border_thickness: 20,
            border_color: 0x3678ef,
            border_opacity: 100,
            aspect: None,
            background_blur: 0,
            background_noise: 0,
            vignette: 0,
            transform: SceneTransform::IDENTITY,
            watermark: None,
            pointer: PointerStyle::default(),
            camera: CameraOverlay::default(),
        }
    }
}

impl SceneStyle {
    /// Canvas size for an export at the requested height. Width follows the
    /// scene aspect ratio (or the media's own ratio) and both are even so
    /// every video encoder accepts them.
    pub fn export_canvas_size(
        &self,
        source_width: u32,
        source_height: u32,
        canvas_height: u32,
    ) -> (u32, u32) {
        let ratio = self
            .aspect
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
            .unwrap_or_else(|| {
                if source_height == 0 {
                    16.0 / 9.0
                } else {
                    source_width as f64 / source_height as f64
                }
            });
        let height = (canvas_height.max(2) / 2) * 2;
        let width = ((height as f64 * ratio).round() as u32).max(2);
        (width / 2 * 2, height)
    }

    /// True when the GPUI preview cannot reproduce the style with plain
    /// elements and must show the composited frame instead.
    pub fn needs_composited_preview(&self) -> bool {
        self.window_frame != WindowFrame::Off
            || !self.transform.is_identity()
            || self.background_blur > 0
            || self.background_noise > 0
            || self.vignette > 0
            || self
                .watermark
                .as_ref()
                .is_some_and(|watermark| !watermark.text.trim().is_empty())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x <= self.right() && y <= self.bottom()
    }

    fn inset(&self, amount: f64) -> Rect {
        Rect {
            x: self.x + amount,
            y: self.y + amount,
            width: (self.width - amount * 2.0).max(0.0),
            height: (self.height - amount * 2.0).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    pub blur_radius: f64,
    pub offset_y: f64,
    pub opacity: f64,
}

/// Resolved layout of one scene at one canvas size (before the transform).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneGeometry {
    pub canvas_width: f64,
    pub canvas_height: f64,
    /// Rect the media occupies at scale 1 (inside the border).
    pub media: Rect,
    /// Corner radius of the media surface.
    pub radius: f64,
    /// Border thickness in canvas pixels (0 when the border is off).
    pub border_width: f64,
    /// Height of the window title bar above the media (0 when off).
    pub title_height: f64,
    pub shadow: Option<ShadowSpec>,
    /// Multiplier applied to every reference-size dimension.
    pub ui_scale: f64,
}

impl SceneGeometry {
    pub fn layout(
        canvas_width: f64,
        canvas_height: f64,
        source_width: f64,
        source_height: f64,
        style: &SceneStyle,
    ) -> Self {
        let ui_scale = (canvas_height / REFERENCE_CANVAS_HEIGHT).max(0.05);
        let border_width = if style.border {
            style.border_thickness as f64 * 0.48 * ui_scale
        } else {
            0.0
        };
        // Zero padding means the media reaches the canvas edge; only the
        // border adds space beyond the user's padding setting.
        let inset = style.padding as f64 * 2.0 * ui_scale + border_width;
        let title_height = if style.window_frame == WindowFrame::Off {
            0.0
        } else {
            TITLE_BAR_HEIGHT * ui_scale
        };
        let available_width = (canvas_width - inset * 2.0).max(1.0);
        let available_height = (canvas_height - inset * 2.0 - title_height).max(1.0);
        let source_width = if source_width > 0.0 {
            source_width
        } else {
            1200.0
        };
        let source_height = if source_height > 0.0 {
            source_height
        } else {
            720.0
        };
        let scale = (available_width / source_width).min(available_height / source_height);
        let width = source_width * scale;
        let height = source_height * scale;
        let media = Rect {
            x: inset + (available_width - width) * 0.5,
            y: inset + title_height + (available_height - height) * 0.5,
            width,
            height,
        };
        let strength = style.shadow as f64 / 100.0;
        let (radius_scale, offset_scale, opacity_scale) = match style.shadow_style {
            0 => (1.0, 0.3, 1.0),
            1 => (1.2, 0.9, 0.85),
            2 => (1.6, 0.0, 0.7),
            _ => (0.8, 0.2, 1.1),
        };
        let shadow_radius = 85.0 * strength * radius_scale * ui_scale;
        let shadow = (style.shadow > 0).then(|| ShadowSpec {
            blur_radius: shadow_radius,
            offset_y: shadow_radius * offset_scale,
            opacity: ((0.08 + strength * 1.35).min(0.35) * opacity_scale).min(0.5),
        });
        Self {
            canvas_width,
            canvas_height,
            media,
            radius: style.corners as f64 * 0.64 * ui_scale,
            border_width,
            title_height,
            shadow,
            ui_scale,
        }
    }

    /// The card rect at scale 1: media, title bar, and border.
    pub fn card(&self) -> Rect {
        let mut card = self.media.inset(-self.border_width);
        card.y -= self.title_height;
        card.height += self.title_height;
        card
    }

    /// Signed distance (media pixels) from media-normalized `(u, v)` to the
    /// rounded card surface (media plus title bar), negative inside.
    pub fn surface_distance(&self, u: f64, v: f64) -> f64 {
        let height = self.media.height + self.title_height;
        let y = (v * self.media.height + self.title_height) / height;
        rounded_rect_distance(u, y, self.media.width, height, self.radius)
    }

    /// Normalized `v` of the title bar's top edge (0 without a frame).
    fn title_top(&self) -> f64 {
        if self.media.height <= 0.0 {
            return 0.0;
        }
        -self.title_height / self.media.height
    }

    /// Canvas bounds of the projected card surface including the title bar.
    fn surface_bounds(&self, projection: &MediaProjection) -> Rect {
        let mut bounds = projection.bounds;
        if self.title_height <= 0.0 {
            return bounds;
        }
        let top = self.title_top();
        for (x, y) in [projection.project(0.0, top), projection.project(1.0, top)] {
            let right = bounds.right().max(x);
            let bottom = bounds.bottom().max(y);
            bounds.x = bounds.x.min(x);
            bounds.y = bounds.y.min(y);
            bounds.width = right - bounds.x;
            bounds.height = bottom - bounds.y;
        }
        bounds
    }

    pub fn card_radius(&self) -> f64 {
        self.radius + self.border_width
    }

    /// Scale at which the media covers the whole canvas.
    pub fn fill_scale(&self) -> f64 {
        if self.media.width <= 0.0 || self.media.height <= 0.0 {
            return 1.0;
        }
        (self.canvas_width / self.media.width).max(self.canvas_height / self.media.height)
    }

    /// Scale at which one media pixel maps to one pixel of an export whose
    /// canvas is `export_canvas_width` wide.
    pub fn actual_size_scale(&self, source_width: f64, export_canvas_width: f64) -> f64 {
        if self.media.width <= 0.0 || self.canvas_width <= 0.0 || export_canvas_width <= 0.0 {
            return 1.0;
        }
        let export_media_width = self.media.width / self.canvas_width * export_canvas_width;
        (source_width / export_media_width)
            .clamp(SceneTransform::MIN_SCALE, SceneTransform::MAX_SCALE)
    }

    /// Projects the media through `transform`.
    pub fn projection(&self, transform: SceneTransform) -> MediaProjection {
        MediaProjection::new(self, transform)
    }
}

/// Planar homography between media space (0..1 × 0..1) and canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaProjection {
    /// Media (u, v) → canvas.
    forward: [[f64; 3]; 3],
    /// Canvas → media (u, v).
    inverse: [[f64; 3]; 3],
    /// Projected corners: top-left, top-right, bottom-right, bottom-left.
    pub quad: [(f64, f64); 4],
    /// Canvas-space bounding box of the quad.
    pub bounds: Rect,
    /// Media size at scale 1 (canvas pixels); radii are measured in it.
    media_width: f64,
    media_height: f64,
    /// Constant media-pixels-per-canvas-pixel when the transform is affine.
    affine_pixel_size: Option<f64>,
}

impl MediaProjection {
    fn new(geometry: &SceneGeometry, transform: SceneTransform) -> Self {
        let transform = transform.clamped();
        let media = geometry.media;
        let anchor = (
            media.x + transform.anchor_x * media.width,
            media.y + transform.anchor_y * media.height,
        );
        let translate = (
            transform.position_x * geometry.canvas_width * 0.5,
            transform.position_y * geometry.canvas_height * 0.5,
        );
        let diagonal = (geometry.canvas_width.powi(2) + geometry.canvas_height.powi(2)).sqrt();
        let camera = diagonal * (3.2 - 2.4 * transform.perspective);
        let (sx, cx) = transform.rotation_x.to_radians().sin_cos();
        let (sy, cy) = transform.rotation_y.to_radians().sin_cos();
        let (sz, cz) = transform.rotation_z.to_radians().sin_cos();
        let corners = [
            (media.x, media.y),
            (media.right(), media.y),
            (media.right(), media.bottom()),
            (media.x, media.bottom()),
        ];
        let mut quad = [(0.0, 0.0); 4];
        for (index, (px, py)) in corners.into_iter().enumerate() {
            let x = (px - anchor.0) * transform.scale;
            let y = (py - anchor.1) * transform.scale;
            // Rotate around Z, then X, then Y (media plane starts at z = 0).
            let (x, y) = (x * cz - y * sz, x * sz + y * cz);
            let (y, z) = (y * cx, y * sx);
            let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
            let depth = (camera - z).max(camera * 0.05);
            let factor = camera / depth;
            quad[index] = (
                anchor.0 + translate.0 + x * factor,
                anchor.1 + translate.1 + y * factor,
            );
        }
        let forward = square_to_quad(quad);
        let inverse =
            invert(forward).unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let min_x = quad.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let max_x = quad.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let min_y = quad.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let max_y = quad.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        let affine_pixel_size = (!transform.has_rotation()).then(|| 1.0 / transform.scale);
        Self {
            forward,
            inverse,
            quad,
            bounds: Rect {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            },
            media_width: media.width,
            media_height: media.height,
            affine_pixel_size,
        }
    }

    /// Media (u, v) in 0..1 → canvas pixel.
    pub fn project(&self, u: f64, v: f64) -> (f64, f64) {
        apply(self.forward, u, v)
    }

    /// Canvas pixel → media (u, v); may fall outside 0..1.
    pub fn unproject(&self, x: f64, y: f64) -> (f64, f64) {
        apply(self.inverse, x, y)
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (u, v) = self.unproject(x, y);
        (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)
    }

    /// `(a, b, c, d)` with `u = a * x + b` and `v = c * y + d` when the
    /// projection is a pure scale and translation.
    fn axis_aligned(&self) -> Option<(f64, f64, f64, f64)> {
        let m = self.inverse;
        let pure = m[0][1].abs() < 1e-12
            && m[1][0].abs() < 1e-12
            && m[2][0].abs() < 1e-12
            && m[2][1].abs() < 1e-12
            && (m[2][2] - 1.0).abs() < 1e-9;
        (pure && self.affine_pixel_size.is_some()).then_some((m[0][0], m[0][2], m[1][1], m[1][2]))
    }

    /// Media pixels (at scale 1) covered by one canvas pixel at `(x, y)`.
    fn pixel_size_at(&self, x: f64, y: f64) -> f64 {
        if let Some(size) = self.affine_pixel_size {
            return size;
        }
        let (u0, v0) = self.unproject(x, y);
        let (u1, v1) = self.unproject(x + 1.0, y);
        let (u2, v2) = self.unproject(x, y + 1.0);
        let dudx = (u1 - u0) * self.media_width;
        let dvdx = (v1 - v0) * self.media_height;
        let dudy = (u2 - u0) * self.media_width;
        let dvdy = (v2 - v0) * self.media_height;
        (dudx * dvdy - dvdx * dudy).abs().sqrt().max(1e-6)
    }

    /// Canvas pixels per media pixel around `(u, v)`.
    fn screen_scale_at(&self, u: f64, v: f64) -> f64 {
        let (x, y) = self.project(u, v);
        1.0 / self.pixel_size_at(x, y)
    }
}

fn apply(matrix: [[f64; 3]; 3], x: f64, y: f64) -> (f64, f64) {
    let w = matrix[2][0] * x + matrix[2][1] * y + matrix[2][2];
    let w = if w.abs() < 1e-12 { 1e-12 } else { w };
    (
        (matrix[0][0] * x + matrix[0][1] * y + matrix[0][2]) / w,
        (matrix[1][0] * x + matrix[1][1] * y + matrix[1][2]) / w,
    )
}

/// Heckbert's unit-square-to-quadrilateral mapping.
fn square_to_quad(quad: [(f64, f64); 4]) -> [[f64; 3]; 3] {
    let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = quad;
    let sx = x0 - x1 + x2 - x3;
    let sy = y0 - y1 + y2 - y3;
    if sx.abs() < 1e-9 && sy.abs() < 1e-9 {
        return [
            [x1 - x0, x3 - x0, x0],
            [y1 - y0, y3 - y0, y0],
            [0.0, 0.0, 1.0],
        ];
    }
    let dx1 = x1 - x2;
    let dx2 = x3 - x2;
    let dy1 = y1 - y2;
    let dy2 = y3 - y2;
    let denominator = dx1 * dy2 - dx2 * dy1;
    let denominator = if denominator.abs() < 1e-12 {
        1e-12
    } else {
        denominator
    };
    let g = (sx * dy2 - dx2 * sy) / denominator;
    let h = (dx1 * sy - sx * dy1) / denominator;
    [
        [x1 - x0 + g * x1, x3 - x0 + h * x3, x0],
        [y1 - y0 + g * y1, y3 - y0 + h * y3, y0],
        [g, h, 1.0],
    ]
}

fn invert(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-18 {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
        ],
    ])
}

/// Pointer state drawn on top of the media surface.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerOverlay {
    pub frame: PointerFrame,
}

/// Per-frame inputs to [`SceneCompositor::compose`].
pub struct FrameInput<'a> {
    /// Full media (screenshot or decoded video frame).
    pub source: &'a RgbaImage,
    /// Optional straight-alpha layer in media space (same aspect as
    /// `source`), e.g. flattened annotations for the current time.
    pub overlay: Option<&'a RgbaImage>,
    pub viewport: ViewportFrame,
    pub pointer: Option<&'a PointerOverlay>,
    /// Camera (webcam) frame for this time, drawn as picture-in-picture.
    pub camera: Option<&'a RgbaImage>,
}

struct CardLayer {
    transform: SceneTransform,
    pixels: RgbaImage,
}

/// CPU compositor with the static layers (background, effects, watermark)
/// precomputed once; the shadow/border card layer is cached per transform.
pub struct SceneCompositor {
    style: SceneStyle,
    geometry: SceneGeometry,
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    background: RgbaImage,
    card: std::cell::RefCell<Option<CardLayer>>,
    vignette: Option<Vec<f32>>,
    watermark: Option<RgbaImage>,
}

impl SceneCompositor {
    pub fn new(
        style: &SceneStyle,
        canvas_width: u32,
        canvas_height: u32,
        source_width: u32,
        source_height: u32,
    ) -> Result<Self, String> {
        Self::build(
            style,
            canvas_width,
            canvas_height,
            source_width,
            source_height,
            None,
        )
    }

    /// Like [`new`](Self::new), but reuses the rendered background, vignette
    /// and watermark layers of `previous` when their inputs did not change,
    /// so interactive edits of one setting only redo that setting's work.
    pub fn rebuild(
        &self,
        style: &SceneStyle,
        canvas_width: u32,
        canvas_height: u32,
        source_width: u32,
        source_height: u32,
    ) -> Result<Self, String> {
        Self::build(
            style,
            canvas_width,
            canvas_height,
            source_width,
            source_height,
            Some(self),
        )
    }

    fn build(
        style: &SceneStyle,
        canvas_width: u32,
        canvas_height: u32,
        source_width: u32,
        source_height: u32,
        previous: Option<&Self>,
    ) -> Result<Self, String> {
        if canvas_width == 0 || canvas_height == 0 {
            return Err("scene canvas must be at least one pixel".into());
        }
        let geometry = SceneGeometry::layout(
            canvas_width as f64,
            canvas_height as f64,
            source_width as f64,
            source_height as f64,
            style,
        );
        let previous = previous
            .filter(|previous| previous.width == canvas_width && previous.height == canvas_height);
        let background = match previous.filter(|previous| {
            previous.style.background == style.background
                && previous.style.background_blur == style.background_blur
                && previous.style.background_noise == style.background_noise
                && previous.geometry.ui_scale == geometry.ui_scale
        }) {
            Some(previous) => previous.background.clone(),
            None => {
                let mut background =
                    render_background(&style.background, canvas_width, canvas_height)?;
                if style.background_blur > 0 {
                    let sigma = style.background_blur as f64 / 100.0 * 40.0 * geometry.ui_scale;
                    blur_image(&mut background, sigma);
                }
                if style.background_noise > 0 {
                    apply_noise(&mut background, style.background_noise as f64 / 100.0);
                }
                background
            }
        };
        let vignette = match previous.filter(|previous| previous.style.vignette == style.vignette) {
            Some(previous) => previous.vignette.clone(),
            None => (style.vignette > 0)
                .then(|| vignette_map(canvas_width, canvas_height, style.vignette as f64 / 100.0)),
        };
        let watermark =
            match previous.filter(|previous| previous.style.watermark == style.watermark) {
                Some(previous) => previous.watermark.clone(),
                None => style
                    .watermark
                    .as_ref()
                    .filter(|watermark| !watermark.text.trim().is_empty())
                    .and_then(|watermark| {
                        render_watermark(watermark, canvas_width, canvas_height).ok()
                    }),
            };
        Ok(Self {
            style: style.clone(),
            geometry,
            width: canvas_width,
            height: canvas_height,
            source_width,
            source_height,
            background,
            card: std::cell::RefCell::new(None),
            vignette,
            watermark,
        })
    }

    pub fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    pub fn style(&self) -> &SceneStyle {
        &self.style
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn source_size(&self) -> (u32, u32) {
        (self.source_width, self.source_height)
    }

    /// Projection for the authored transform plus a frame's tilt.
    pub fn projection(&self, viewport: ViewportFrame) -> MediaProjection {
        self.geometry
            .projection(self.style.transform.with_tilt(viewport.tilt))
    }

    /// Composes one output frame.
    pub fn compose(&self, input: FrameInput<'_>) -> RgbaImage {
        let transform = self.style.transform.with_tilt(input.viewport.tilt);
        let mut output = self.card_layer(transform);
        let projection = self.geometry.projection(transform);
        self.paint_media(
            &mut output,
            &projection,
            input.source,
            input.overlay,
            input.viewport,
        );
        if let Some(pointer) = input.pointer {
            self.paint_pointer(&mut output, &projection, pointer, input.viewport);
        }
        if let Some(camera) = input.camera {
            if self.style.camera.enabled {
                self.paint_camera(&mut output, camera);
            }
        }
        if let Some(watermark) = self.watermark.as_ref() {
            blend_layer(&mut output, watermark);
        }
        if let Some(vignette) = self.vignette.as_ref() {
            for (index, pixel) in output.pixels_mut().enumerate() {
                let factor = vignette[index];
                pixel[0] = (pixel[0] as f32 * factor).round() as u8;
                pixel[1] = (pixel[1] as f32 * factor).round() as u8;
                pixel[2] = (pixel[2] as f32 * factor).round() as u8;
            }
        }
        output
    }

    /// Background plus shadow and border for `transform`, cached while the
    /// transform stays the same (animated tilts re-render it per frame).
    fn card_layer(&self, transform: SceneTransform) -> RgbaImage {
        let mut cache = self.card.borrow_mut();
        if let Some(layer) = cache.as_ref() {
            if layer.transform == transform {
                return layer.pixels.clone();
            }
        }
        let mut pixels = self.background.clone();
        let projection = self.geometry.projection(transform);
        if let Some(shadow) = self.geometry.shadow {
            paint_shadow(&mut pixels, &projection, self.geometry, shadow);
        }
        if let Some(bar) = self.style.window_frame.bar_color() {
            if self.geometry.title_height > 0.0 {
                self.paint_title_bar(&mut pixels, &projection, bar);
            }
        }
        if self.geometry.border_width > 0.0 && self.style.border_opacity > 0 {
            self.paint_border(&mut pixels, &projection);
        }
        *cache = Some(CardLayer {
            transform,
            pixels: pixels.clone(),
        });
        pixels
    }

    /// Signed distance (media pixels) from `(u, v)` to the media area: the
    /// card surface cut off at the title bar, negative inside.
    fn media_distance(&self, u: f64, v: f64) -> f64 {
        self.geometry
            .surface_distance(u, v)
            .max(-v * self.geometry.media.height)
    }

    fn pixel_range(&self, projection: &MediaProjection, margin: f64) -> (u32, u32, u32, u32) {
        let bounds = self.geometry.surface_bounds(projection);
        let x0 = (bounds.x - margin).floor().max(0.0) as u32;
        let y0 = (bounds.y - margin).floor().max(0.0) as u32;
        let x1 = ((bounds.right() + margin).ceil().max(0.0) as u32).min(self.width);
        let y1 = ((bounds.bottom() + margin).ceil().max(0.0) as u32).min(self.height);
        (x0, y0, x1, y1)
    }

    /// Title bar with traffic lights above the media, inside the card outline.
    fn paint_title_bar(&self, output: &mut RgbaImage, projection: &MediaProjection, bar: [u8; 3]) {
        const LIGHTS: [[u8; 3]; 3] = [[0xff, 0x5f, 0x57], [0xfe, 0xbc, 0x2e], [0x28, 0xc8, 0x40]];
        let geometry = self.geometry;
        let title = geometry.title_height;
        let light_radius = title * 0.2;
        let light_spacing = title * 0.67;
        let light_start = title * 0.7;
        let top = geometry.title_top();
        let (x0, y0, x1, y1) = self.pixel_range(projection, 2.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let (u, v) = projection.unproject(px, py);
                if !(-0.05..=1.05).contains(&u) || v < top - 0.05 || v > 0.05 {
                    continue;
                }
                let pixel_size = projection.pixel_size_at(px, py);
                let distance = geometry
                    .surface_distance(u, v)
                    .max(v * geometry.media.height);
                let coverage = (0.5 - distance / pixel_size).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                blend_pixel(output, x, y, bar, coverage);
                let local_x = u * geometry.media.width;
                let local_y = v * geometry.media.height + title * 0.5;
                for (index, color) in LIGHTS.into_iter().enumerate() {
                    let dx = local_x - (light_start + light_spacing * index as f64);
                    let light = (dx * dx + local_y * local_y).sqrt() - light_radius;
                    let light_coverage = (0.5 - light / pixel_size).clamp(0.0, 1.0);
                    if light_coverage > 0.0 {
                        blend_pixel(output, x, y, color, light_coverage * coverage);
                    }
                }
            }
        }
    }

    fn paint_border(&self, output: &mut RgbaImage, projection: &MediaProjection) {
        let tint = unpack(self.style.border_color);
        let alpha = self.style.border_opacity as f64 / 100.0;
        let border = self.geometry.border_width;
        let (x0, y0, x1, y1) = self.pixel_range(projection, border * 2.0 + 2.0);
        let top = self.geometry.title_top() - 0.3;
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let (u, v) = projection.unproject(px, py);
                if !(-0.3..=1.3).contains(&u) || !(top..=1.3).contains(&v) {
                    continue;
                }
                let pixel_size = projection.pixel_size_at(px, py);
                let distance = self.geometry.surface_distance(u, v);
                let outer = (0.5 - (distance - border) / pixel_size).clamp(0.0, 1.0);
                let inner = (0.5 - distance / pixel_size).clamp(0.0, 1.0);
                let coverage = (outer - inner).max(0.0);
                if coverage > 0.0 {
                    blend_pixel(output, x, y, tint, coverage * alpha);
                }
            }
        }
    }

    fn paint_media(
        &self,
        output: &mut RgbaImage,
        projection: &MediaProjection,
        source: &RgbaImage,
        overlay: Option<&RgbaImage>,
        viewport: ViewportFrame,
    ) {
        if source.width() == 0 || source.height() == 0 {
            return;
        }
        let overlay = overlay.filter(|layer| layer.width() > 0 && layer.height() > 0);
        let (left, top, visible) = visible_rect(viewport);
        let (x0, y0, x1, y1) = self.pixel_range(projection, 2.0);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let painter = MediaPainter {
            geometry: self.geometry,
            projection,
            source,
            overlay,
            left,
            top,
            visible,
            x0,
            x1,
        };
        // Every row is independent, so the rows are painted in parallel.
        let stride = output.width() as usize * 4;
        let buffer: &mut [u8] = output.as_mut();
        let region = &mut buffer[y0 as usize * stride..y1 as usize * stride];
        let threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .clamp(1, 8);
        let rows_per_chunk = ((y1 - y0) as usize).div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (index, chunk) in region.chunks_mut(rows_per_chunk * stride).enumerate() {
                let first_row = y0 + (index * rows_per_chunk) as u32;
                let painter = &painter;
                scope.spawn(move || painter.paint_rows(chunk, stride, first_row));
            }
        });
    }

    fn paint_camera(&self, output: &mut RgbaImage, camera: &RgbaImage) {
        let overlay = self.style.camera;
        paint_camera_overlay(output, camera, overlay, overlay.rect(self.width as f64, self.height as f64));
    }

    fn paint_pointer(
        &self,
        output: &mut RgbaImage,
        projection: &MediaProjection,
        pointer: &PointerOverlay,
        viewport: ViewportFrame,
    ) {
        let style = self.style.pointer;
        if !style.visible {
            return;
        }
        let frame = &pointer.frame;
        let (left, top, visible) = visible_rect(viewport);
        let to_canvas = |point: NormalizedPoint| {
            let u = (point.x - left) / visible;
            let v = (point.y - top) / visible;
            (projection.project(u, v), u, v)
        };
        let clip = Rect {
            x: 0.0,
            y: 0.0,
            width: self.width as f64,
            height: self.height as f64,
        };
        let pointer_scale = style.scale as f64 / 100.0;
        if style.click_effects {
            if let Some(press) = frame.press.as_ref() {
                let ((x, y), u, v) = to_canvas(press.location);
                if (-0.02..=1.02).contains(&u) && (-0.02..=1.02).contains(&v) {
                    let local_scale = projection.screen_scale_at(u, v);
                    let geometry = pointer_press_effect_geometry(
                        press.progress,
                        self.geometry.media.height * local_scale,
                        viewport.magnification.max(1.0) * pointer_scale,
                    );
                    let color = unpack(style.click_color);
                    paint_disc(
                        output,
                        clip,
                        x,
                        y,
                        geometry.impact_radius,
                        color,
                        geometry.impact_opacity,
                    );
                    paint_ring(
                        output,
                        clip,
                        x,
                        y,
                        geometry.ripple_radius,
                        geometry.ripple_line_width,
                        color,
                        geometry.ripple_opacity,
                    );
                }
            }
        }
        let opacity = if style.hide_when_idle {
            frame.opacity
        } else {
            1.0
        };
        if opacity <= 0.0 {
            return;
        }
        let ((x, y), u, v) = to_canvas(frame.location);
        if !(-0.02..=1.02).contains(&u) || !(-0.02..=1.02).contains(&v) {
            return;
        }
        let local_scale = projection.screen_scale_at(u, v);
        // Canvas pixels per normalized media unit at this point.
        let unit_scale = (
            self.geometry.media.width * local_scale / visible,
            self.geometry.media.height * local_scale / visible,
        );
        let bitmap = frame
            .bitmap
            .as_deref()
            .filter(|_| style.family == CursorFamily::Recorded);
        if let Some(bitmap) = bitmap {
            // Captured cursors keep their on-screen proportion to the
            // recording, boosted so 100% reads like the vector arrow.
            let media_scale =
                local_scale * pointer_scale * frame.magnification / visible * CAPTURED_CURSOR_BOOST;
            let width = bitmap.reference_width * self.geometry.media.width * media_scale;
            let height = bitmap.reference_height * self.geometry.media.height * media_scale;
            if width < 1.0 || height < 1.0 {
                return;
            }
            let placement = BitmapPlacement {
                hotspot_x: x,
                hotspot_y: y,
                width,
                height,
                tilt_degrees: frame.tilt_degrees,
                smear: cursor_smear(frame.velocity, unit_scale, width),
            };
            if style.shadow {
                let shadow = BitmapPlacement {
                    hotspot_x: x + height * 0.05,
                    hotspot_y: y + height * 0.07,
                    ..placement
                };
                paint_cursor_bitmap(output, clip, bitmap, shadow, opacity * 0.35, true);
            }
            paint_cursor_bitmap(output, clip, bitmap, placement, opacity, false);
            return;
        }
        // Styled cursors follow Cap: a fixed height relative to the media,
        // in the chosen family's rendition of the recorded shape.  The
        // artwork carries its own soft shadow.
        let family = match style.family {
            CursorFamily::Recorded => CursorFamily::MacOs,
            family => family,
        };
        let shape = frame
            .bitmap
            .as_deref()
            .and_then(|bitmap| bitmap.shape)
            .unwrap_or_default();
        let height = ASSET_CURSOR_HEIGHT * self.geometry.media.height * local_scale / visible
            * pointer_scale
            * frame.magnification;
        if height < 1.0 {
            return;
        }
        let Some(asset) = cursor_assets::rasterize(family, shape, height.round() as u32) else {
            return;
        };
        let width = f64::from(asset.image.width());
        let placement = BitmapPlacement {
            hotspot_x: x,
            hotspot_y: y,
            width,
            height: f64::from(asset.image.height()),
            tilt_degrees: frame.tilt_degrees,
            smear: cursor_smear(frame.velocity, unit_scale, width),
        };
        paint_cursor_bitmap(output, clip, &asset, placement, opacity, false);
    }
}

/// Paints the media surface into a band of output rows.  Untransformed
/// media takes an integer bilinear fast path away from the surface edge;
/// the edge band and every projected pixel use the exact per-pixel path.
struct MediaPainter<'a> {
    geometry: SceneGeometry,
    projection: &'a MediaProjection,
    source: &'a RgbaImage,
    overlay: Option<&'a RgbaImage>,
    left: f64,
    top: f64,
    visible: f64,
    x0: u32,
    x1: u32,
}

/// One axis of a bilinear tap: the lower texel and the 8-bit weight of the
/// next one.
#[derive(Clone, Copy)]
struct Tap {
    index: usize,
    weight: u32,
}

fn tap(position: f64, size: u32) -> Tap {
    let max = f64::from(size - 1);
    let position = position.clamp(0.0, max);
    let index = position.floor();
    Tap {
        index: index as usize,
        weight: ((position - index) * 256.0).round() as u32,
    }
}

/// Bilinear sample in 8-bit fixed point, matching `sample_bilinear`.
fn sample_fixed(image: &RgbaImage, x: Tap, y: Tap) -> [u8; 4] {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let x1 = (x.index + 1).min(width - 1);
    let y1 = (y.index + 1).min(height - 1);
    let raw = image.as_raw();
    let row0 = y.index * width * 4;
    let row1 = y1 * width * 4;
    let p00 = &raw[row0 + x.index * 4..row0 + x.index * 4 + 4];
    let p10 = &raw[row0 + x1 * 4..row0 + x1 * 4 + 4];
    let p01 = &raw[row1 + x.index * 4..row1 + x.index * 4 + 4];
    let p11 = &raw[row1 + x1 * 4..row1 + x1 * 4 + 4];
    let (fx, fy) = (x.weight, y.weight);
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let top = u32::from(p00[channel]) * (256 - fx) + u32::from(p10[channel]) * fx;
        let bottom = u32::from(p01[channel]) * (256 - fx) + u32::from(p11[channel]) * fx;
        out[channel] = ((top * (256 - fy) + bottom * fy + 32_768) >> 16) as u8;
    }
    out
}

impl MediaPainter<'_> {
    fn media_distance(&self, u: f64, v: f64) -> f64 {
        self.geometry
            .surface_distance(u, v)
            .max(-v * self.geometry.media.height)
    }

    fn paint_rows(&self, rows: &mut [u8], stride: usize, first_row: u32) {
        let axis_aligned = self.projection.axis_aligned();
        let source_width = f64::from(self.source.width());
        // Column tables for the axis-aligned path: media `u` and the
        // source tap are the same for every row.
        let columns: Vec<(f64, Tap, Option<Tap>)> = axis_aligned
            .map(|(a, b, _, _)| {
                (self.x0..self.x1)
                    .map(|x| {
                        let u = a * (f64::from(x) + 0.5) + b;
                        let media_u = self.left + u.clamp(0.0, 1.0) * self.visible;
                        (
                            u,
                            tap(media_u * source_width - 0.5, self.source.width()),
                            self.overlay.map(|layer| {
                                tap(media_u * f64::from(layer.width()) - 0.5, layer.width())
                            }),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let media = self.geometry.media;
        let pixel_size = self.projection.affine_pixel_size.unwrap_or(1.0);
        // Pixels this close (in media pixels) to the surface outline take
        // the exact path so corners and edge coverage are unchanged.
        let band = self.geometry.radius + 2.0 * pixel_size + 1.0;
        let surface_height = media.height + self.geometry.title_height;

        for (row_index, row) in rows.chunks_mut(stride).enumerate() {
            let y = first_row + row_index as u32;
            let py = f64::from(y) + 0.5;
            let row_taps = axis_aligned.map(|(_, _, c, d)| {
                let v = c * py + d;
                let media_v = self.top + v.clamp(0.0, 1.0) * self.visible;
                let surface_y = v * media.height + self.geometry.title_height;
                let interior_row =
                    v * media.height > pixel_size + 1.0 && surface_y < surface_height - band;
                (
                    v,
                    interior_row,
                    tap(
                        media_v * f64::from(self.source.height()) - 0.5,
                        self.source.height(),
                    ),
                    self.overlay.map(|layer| {
                        tap(media_v * f64::from(layer.height()) - 0.5, layer.height())
                    }),
                )
            });
            for x in self.x0..self.x1 {
                let pixel = &mut row[x as usize * 4..x as usize * 4 + 4];
                if let Some((v, interior_row, source_y, overlay_y)) = row_taps {
                    let (u, source_x, overlay_x) = columns[(x - self.x0) as usize];
                    if !(-0.05..=1.05).contains(&u) || !(-0.05..=1.05).contains(&v) {
                        continue;
                    }
                    let px = u * media.width;
                    if interior_row && px > band && px < media.width - band {
                        let mut sample = sample_fixed(self.source, source_x, source_y);
                        if let (Some(layer), Some(ox), Some(oy)) =
                            (self.overlay, overlay_x, overlay_y)
                        {
                            composite_over(&mut sample, sample_fixed(layer, ox, oy));
                        }
                        blend_fixed(pixel, sample, 256);
                        continue;
                    }
                }
                self.paint_exact(pixel, f64::from(x) + 0.5, py);
            }
        }
    }

    /// The original per-pixel path: exact projection, edge coverage and
    /// floating-point sampling.
    fn paint_exact(&self, pixel: &mut [u8], px: f64, py: f64) {
        let (u, v) = self.projection.unproject(px, py);
        if !(-0.05..=1.05).contains(&u) || !(-0.05..=1.05).contains(&v) {
            return;
        }
        let pixel_size = self.projection.pixel_size_at(px, py);
        let coverage = (0.5 - self.media_distance(u, v) / pixel_size).clamp(0.0, 1.0);
        if coverage <= 0.0 {
            return;
        }
        let media_u = self.left + u.clamp(0.0, 1.0) * self.visible;
        let media_v = self.top + v.clamp(0.0, 1.0) * self.visible;
        let mut sample = sample_bilinear(
            self.source,
            media_u * f64::from(self.source.width()) - 0.5,
            media_v * f64::from(self.source.height()) - 0.5,
        );
        if let Some(layer) = self.overlay {
            let over = sample_bilinear(
                layer,
                media_u * f64::from(layer.width()) - 0.5,
                media_v * f64::from(layer.height()) - 0.5,
            );
            let alpha = f64::from(over[3]) / 255.0;
            for channel in 0..3 {
                sample[channel] = (f64::from(sample[channel])
                    + (f64::from(over[channel]) - f64::from(sample[channel])) * alpha)
                    .round() as u8;
            }
            sample[3] =
                (f64::from(sample[3]) + (255.0 - f64::from(sample[3])) * alpha).round() as u8;
        }
        blend_slice(
            pixel,
            [sample[0], sample[1], sample[2]],
            coverage * f64::from(sample[3]) / 255.0,
        );
    }
}

/// Straight-alpha "over" of `over` onto `under`, in place.
fn composite_over(under: &mut [u8; 4], over: [u8; 4]) {
    let alpha = u32::from(over[3]);
    for channel in 0..3 {
        under[channel] =
            ((u32::from(under[channel]) * (255 - alpha) + u32::from(over[channel]) * alpha + 127)
                / 255) as u8;
    }
    under[3] = ((u32::from(under[3]) * (255 - alpha) + 255 * alpha + 127) / 255) as u8;
}

/// Blends a straight-alpha sample onto a pixel with an 8.8 fixed-point
/// coverage (256 = full).
fn blend_fixed(pixel: &mut [u8], sample: [u8; 4], coverage: u32) {
    let alpha = u32::from(sample[3]) * coverage;
    if alpha >= 255 * 256 {
        pixel[..4].copy_from_slice(&sample);
        return;
    }
    if alpha == 0 {
        return;
    }
    let scale = 255 * 256;
    for channel in 0..3 {
        pixel[channel] = ((u32::from(pixel[channel]) * (scale - alpha)
            + u32::from(sample[channel]) * alpha
            + scale / 2)
            / scale) as u8;
    }
    pixel[3] = ((u32::from(pixel[3]) * (scale - alpha) + 255 * alpha + scale / 2) / scale) as u8;
}

fn rounded_rect_distance(u: f64, v: f64, width: f64, height: f64, radius: f64) -> f64 {
    let radius = radius.clamp(0.0, width.min(height) * 0.5);
    let x = u * width;
    let y = v * height;
    let dx = (x - width * 0.5).abs() - (width * 0.5 - radius);
    let dy = (y - height * 0.5).abs() - (height * 0.5 - radius);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    outside + inside - radius
}

fn unpack(color: u32) -> [u8; 3] {
    [
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    ]
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Enlarged framing preview using the exact crop, mask, mirror, and shadow
/// painter used for the recording. Placement and size remain editor settings.
pub fn camera_framing_preview(camera: &RgbaImage, overlay: CameraOverlay) -> RgbaImage {
    let mut output = RgbaImage::from_pixel(200, 200, Rgba([251, 251, 251, 255]));
    paint_camera_overlay(&mut output, camera, overlay, Rect {
        x: 12.0, y: 12.0, width: 176.0, height: 176.0,
    });
    output
}

/// Shared cover-fit camera renderer for the launcher, editor, and export.
fn paint_camera_overlay(output: &mut RgbaImage, camera: &RgbaImage, overlay: CameraOverlay, rect: Rect) {
    if camera.width() == 0 || camera.height() == 0 {
        return;
    }
    let radius = overlay.radius(rect);
    if overlay.shadow {
        let width = output.width() as usize;
        let height = output.height() as usize;
        let mut mask = vec![0.0f32; width * height];
        let offset = rect.height * 0.06;
        let x0 = (rect.x - 2.0).floor().max(0.0) as usize;
        let y0 = (rect.y + offset - 2.0).floor().max(0.0) as usize;
        let x1 = ((rect.right() + 2.0).ceil().max(0.0) as usize).min(width);
        let y1 = ((rect.bottom() + offset + 2.0).ceil().max(0.0) as usize).min(height);
        for y in y0..y1 {
            for x in x0..x1 {
                let u = (x as f64 + 0.5 - rect.x) / rect.width;
                let v = (y as f64 + 0.5 - offset - rect.y) / rect.height;
                let distance = rounded_rect_distance(u, v, rect.width, rect.height, radius);
                mask[y * width + x] = (0.5 - distance).clamp(0.0, 1.0) as f32;
            }
        }
        blur_plane(&mut mask, width, height, rect.height * 0.05 * 0.5 + 1.0);
        for y in 0..height {
            for x in 0..width {
                let alpha = mask[y * width + x] as f64 * 0.35;
                if alpha > 0.001 {
                    blend_pixel(output, x as u32, y as u32, [0, 0, 0], alpha);
                }
            }
        }
    }
    // Cover-fit: scale the frame so the square is filled, crop the rest.
    let frame_width = camera.width() as f64;
    let frame_height = camera.height() as f64;
    let scale = (rect.width / frame_width).max(rect.height / frame_height);
    let scaled_width = frame_width * scale;
    let scaled_height = frame_height * scale;
    let offset_x = (rect.width - scaled_width) * 0.5;
    let offset_y = (rect.height - scaled_height) * 0.5;
    let x0 = rect.x.floor().max(0.0) as u32;
    let y0 = rect.y.floor().max(0.0) as u32;
    let x1 = (rect.right().ceil().max(0.0) as u32).min(output.width());
    let y1 = (rect.bottom().ceil().max(0.0) as u32).min(output.height());
    for y in y0..y1 {
        for x in x0..x1 {
            let local_x = x as f64 + 0.5 - rect.x;
            let local_y = y as f64 + 0.5 - rect.y;
            let u = local_x / rect.width;
            let v = local_y / rect.height;
            let coverage = (0.5 - rounded_rect_distance(u, v, rect.width, rect.height, radius))
                .clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let mut sx = (local_x - offset_x) / scale - 0.5;
            if overlay.mirror {
                sx = frame_width - 1.0 - sx;
            }
            let sy = (local_y - offset_y) / scale - 0.5;
            let sample = sample_bilinear(camera, sx, sy);
            blend_pixel(
                output,
                x,
                y,
                [sample[0], sample[1], sample[2]],
                coverage * sample[3] as f64 / 255.0,
            );
        }
    }
}


fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 3], alpha: f64) {
    if x >= image.width() || y >= image.height() {
        return;
    }
    blend_slice(&mut image.get_pixel_mut(x, y).0, color, alpha);
}

fn blend_slice(pixel: &mut [u8], color: [u8; 3], alpha: f64) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    for channel in 0..3 {
        pixel[channel] = (pixel[channel] as f64
            + (color[channel] as f64 - pixel[channel] as f64) * alpha)
            .round() as u8;
    }
    pixel[3] = (pixel[3] as f64 + (255.0 - pixel[3] as f64) * alpha).round() as u8;
}

/// Straight-alpha `layer` over `image` (same size).
fn blend_layer(image: &mut RgbaImage, layer: &RgbaImage) {
    if layer.dimensions() != image.dimensions() {
        return;
    }
    for (target, source) in image.pixels_mut().zip(layer.pixels()) {
        let alpha = source[3] as f64 / 255.0;
        if alpha <= 0.0 {
            continue;
        }
        for channel in 0..3 {
            target[channel] = (target[channel] as f64
                + (source[channel] as f64 - target[channel] as f64) * alpha)
                .round() as u8;
        }
        target[3] = (target[3] as f64 + (255.0 - target[3] as f64) * alpha).round() as u8;
    }
}

fn sample_bilinear(image: &RgbaImage, x: f64, y: f64) -> [u8; 4] {
    let max_x = image.width() as f64 - 1.0;
    let max_y = image.height() as f64 - 1.0;
    let x = x.clamp(0.0, max_x);
    let y = y.clamp(0.0, max_y);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let p00 = image.get_pixel(x0, y0).0;
    let p10 = image.get_pixel(x1, y0).0;
    let p01 = image.get_pixel(x0, y1).0;
    let p11 = image.get_pixel(x1, y1).0;
    let mut out = [0u8; 4];
    for channel in 0..4 {
        let top = lerp(p00[channel] as f64, p10[channel] as f64, fx);
        let bottom = lerp(p01[channel] as f64, p11[channel] as f64, fx);
        out[channel] = lerp(top, bottom, fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn render_background(
    background: &SceneBackground,
    width: u32,
    height: u32,
) -> Result<RgbaImage, String> {
    match background {
        SceneBackground::Solid(color) => {
            let [r, g, b] = unpack(*color);
            Ok(RgbaImage::from_pixel(width, height, Rgba([r, g, b, 255])))
        }
        SceneBackground::Gradient {
            colors,
            angle_degrees,
        } => {
            let mut image = RgbaImage::new(width, height);
            let first = unpack(colors[0]);
            let middle = unpack(colors[1]);
            let last = unpack(colors[2]);
            // CSS angles: 0deg points up, 90deg points right.
            let radians = angle_degrees.to_radians();
            let dir = (radians.sin(), -radians.cos());
            let w = width as f64;
            let h = height as f64;
            let half_length = (w * dir.0.abs() + h * dir.1.abs()) * 0.5;
            for y in 0..height {
                for x in 0..width {
                    let px = x as f64 + 0.5 - w * 0.5;
                    let py = y as f64 + 0.5 - h * 0.5;
                    let t = if half_length > 0.0 {
                        ((px * dir.0 + py * dir.1) / half_length * 0.5 + 0.5).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let mut pixel = [0u8; 3];
                    for channel in 0..3 {
                        pixel[channel] =
                            lerp(first[channel] as f64, middle[channel] as f64, t).round() as u8;
                    }
                    // Second layer: transparent middle colour fading into
                    // the final stop from 35% onward, matching the preview.
                    let overlay = ((t - 0.35) / 0.65).clamp(0.0, 1.0);
                    for channel in 0..3 {
                        pixel[channel] = lerp(pixel[channel] as f64, last[channel] as f64, overlay)
                            .round() as u8;
                    }
                    image.put_pixel(x, y, Rgba([pixel[0], pixel[1], pixel[2], 255]));
                }
            }
            Ok(image)
        }
        SceneBackground::Wallpaper(path) => {
            let wallpaper = image::open(path)
                .map_err(|error| format!("could not open wallpaper {}: {error}", path.display()))?
                .to_rgba8();
            let mut image = RgbaImage::new(width, height);
            let ww = wallpaper.width() as f64;
            let wh = wallpaper.height() as f64;
            if ww <= 0.0 || wh <= 0.0 {
                return Err("wallpaper image is empty".into());
            }
            let scale = (width as f64 / ww).max(height as f64 / wh);
            let scaled_w = ww * scale;
            let scaled_h = wh * scale;
            let offset_x = (width as f64 - scaled_w) * 0.5;
            let offset_y = (height as f64 - scaled_h) * 0.5;
            for y in 0..height {
                for x in 0..width {
                    let sx = (x as f64 + 0.5 - offset_x) / scale - 0.5;
                    let sy = (y as f64 + 0.5 - offset_y) / scale - 0.5;
                    let sample = sample_bilinear(&wallpaper, sx, sy);
                    image.put_pixel(x, y, Rgba([sample[0], sample[1], sample[2], 255]));
                }
            }
            Ok(image)
        }
    }
}

/// Gaussian-like blur (three box passes) of an opaque RGBA image.
fn blur_image(image: &mut RgbaImage, sigma: f64) {
    if sigma <= 0.5 {
        return;
    }
    let width = image.width() as usize;
    let height = image.height() as usize;
    let factor = blur_downsample_factor(sigma);
    if factor == 1 {
        let mut channels: Vec<Vec<f32>> = (0..3)
            .map(|channel| {
                image
                    .pixels()
                    .map(|pixel| pixel[channel] as f32)
                    .collect::<Vec<f32>>()
            })
            .collect();
        for channel in channels.iter_mut() {
            blur_plane(channel, width, height, sigma);
        }
        for (index, pixel) in image.pixels_mut().enumerate() {
            pixel[0] = channels[0][index].round().clamp(0.0, 255.0) as u8;
            pixel[1] = channels[1][index].round().clamp(0.0, 255.0) as u8;
            pixel[2] = channels[2][index].round().clamp(0.0, 255.0) as u8;
        }
        return;
    }
    // Large sigma: average blocks straight out of the RGBA buffer, blur the
    // small planes, and write the upsampled result straight back.
    let small_width = width.div_ceil(factor);
    let small_height = height.div_ceil(factor);
    let mut small = vec![vec![0.0f32; small_width * small_height]; 3];
    let raw = image.as_raw();
    for sy in 0..small_height {
        for sx in 0..small_width {
            let x_end = ((sx + 1) * factor).min(width);
            let y_end = ((sy + 1) * factor).min(height);
            let mut sum = [0.0f32; 3];
            for y in sy * factor..y_end {
                for x in sx * factor..x_end {
                    let offset = (y * width + x) * 4;
                    sum[0] += raw[offset] as f32;
                    sum[1] += raw[offset + 1] as f32;
                    sum[2] += raw[offset + 2] as f32;
                }
            }
            let count = ((x_end - sx * factor) * (y_end - sy * factor)) as f32;
            for channel in 0..3 {
                small[channel][sy * small_width + sx] = sum[channel] / count;
            }
        }
    }
    for plane in small.iter_mut() {
        blur_plane(plane, small_width, small_height, sigma / factor as f64);
    }
    let raw: &mut [u8] = image.as_mut();
    for channel in 0..3 {
        upsample_bilinear(
            &small[channel],
            small_width,
            small_height,
            factor,
            width,
            height,
            |index, value| raw[index * 4 + channel] = value.round().clamp(0.0, 255.0) as u8,
        );
    }
}

fn box_radius_for(sigma: f64) -> usize {
    // Three box passes of width w approximate sigma² = 3 (w² − 1) / 12.
    let box_width = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    (((box_width - 1.0) / 2.0).round() as usize).max(1)
}

/// Deterministic film grain.
fn apply_noise(image: &mut RgbaImage, amount: f64) {
    let strength = (amount.clamp(0.0, 1.0) * 48.0) as f32;
    let width = image.width() as usize;
    let height = image.height() as usize;
    let raw: &mut [u8] = image.as_mut();
    for y in 0..height {
        let row = &mut raw[y * width * 4..(y + 1) * width * 4];
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let noise = (hash_noise(x as u64, y as u64) as f32 * 2.0 - 1.0) * strength;
            for channel in pixel.iter_mut().take(3) {
                *channel = (*channel as f32 + noise).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn hash_noise(x: u64, y: u64) -> f64 {
    let mut h = x.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ y.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 29;
    (h & 0xFFFF) as f64 / 65535.0
}

fn vignette_map(width: u32, height: u32, amount: f64) -> Vec<f32> {
    let mut map = vec![1.0f32; (width * height) as usize];
    let cx = width as f64 * 0.5;
    let cy = height as f64 * 0.5;
    let corner = (cx * cx + cy * cy).sqrt().max(1.0);
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let r = (dx * dx + dy * dy).sqrt() / corner;
            let t = ((r - 0.3) / 0.7).clamp(0.0, 1.0);
            let falloff = t * t * (3.0 - 2.0 * t);
            map[(y * width + x) as usize] = (1.0 - amount * 0.85 * falloff) as f32;
        }
    }
    map
}

fn render_watermark(watermark: &Watermark, width: u32, height: u32) -> Result<RgbaImage, String> {
    let font_size = height as f64 * (0.02 + watermark.size as f64 / 100.0 * 0.06);
    let margin = height as f64 * 0.035;
    let (x, anchor) = match watermark.position {
        WatermarkPosition::TopLeft | WatermarkPosition::BottomLeft => (margin, "start"),
        WatermarkPosition::TopRight | WatermarkPosition::BottomRight => {
            (width as f64 - margin, "end")
        }
    };
    let y = match watermark.position {
        WatermarkPosition::TopLeft | WatermarkPosition::TopRight => margin + font_size,
        WatermarkPosition::BottomLeft | WatermarkPosition::BottomRight => height as f64 - margin,
    };
    let opacity = watermark.opacity as f64 / 100.0;
    let text = xml_escape(watermark.text.trim());
    let stroke_opacity = opacity * 0.35;
    let stroke_width = font_size * 0.06;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><text x="{x:.2}" y="{y:.2}" text-anchor="{anchor}" font-family="sans-serif" font-weight="600" font-size="{font_size:.2}" fill="#ffffff" fill-opacity="{opacity:.3}" stroke="#000000" stroke-opacity="{stroke_opacity:.3}" stroke-width="{stroke_width:.2}" paint-order="stroke">{text}</text></svg>"##
    );
    render_svg_layer(&svg, width, height)
}

/// System fonts loaded once and shared by every SVG render (loading them
/// per frame would dominate watermark and annotation rendering).
pub fn shared_fontdb() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = resvg::usvg::fontdb::Database::new();
            database.load_system_fonts();
            std::sync::Arc::new(database)
        })
        .clone()
}

/// Renders SVG markup to a straight-alpha RGBA layer.
pub fn render_svg_layer(svg: &str, width: u32, height: u32) -> Result<RgbaImage, String> {
    let mut options = resvg::usvg::Options::default();
    options.fontdb = shared_fontdb();
    let tree = resvg::usvg::Tree::from_str(svg, &options)
        .map_err(|error| format!("could not parse overlay: {error}"))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "overlay dimensions are too large".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let mut data = pixmap.take();
    // tiny-skia stores premultiplied alpha; the compositor expects straight.
    for pixel in data.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for channel in pixel.iter_mut().take(3) {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width, height, data)
        .ok_or_else(|| "overlay had an invalid byte count".to_string())
}

pub fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn paint_shadow(
    image: &mut RgbaImage,
    projection: &MediaProjection,
    geometry: SceneGeometry,
    shadow: ShadowSpec,
) {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let mut mask = vec![0.0f32; width * height];
    let border = geometry.border_width;
    let margin = border * 2.0 + 2.0;
    let bounds = geometry.surface_bounds(projection);
    let x0 = (bounds.x - margin).floor().max(0.0) as usize;
    let y0 = (bounds.y - margin + shadow.offset_y).floor().max(0.0) as usize;
    let x1 = ((bounds.right() + margin).ceil().max(0.0) as usize).min(width);
    let y1 = ((bounds.bottom() + margin + shadow.offset_y).ceil().max(0.0) as usize).min(height);
    let top = geometry.title_top() - 0.3;
    for y in y0..y1 {
        for x in x0..x1 {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5 - shadow.offset_y;
            let (u, v) = projection.unproject(px, py);
            if !(-0.3..=1.3).contains(&u) || !(top..=1.3).contains(&v) {
                continue;
            }
            let pixel_size = projection.pixel_size_at(px, py);
            let distance = geometry.surface_distance(u, v);
            mask[y * width + x] = (0.5 - (distance - border) / pixel_size).clamp(0.0, 1.0) as f32;
        }
    }
    // A CSS blur radius corresponds to a Gaussian with sigma = radius / 2.
    blur_plane(&mut mask, width, height, shadow.blur_radius * 0.5);
    for y in 0..height {
        for x in 0..width {
            let alpha = mask[y * width + x] as f64 * shadow.opacity;
            if alpha > 0.001 {
                blend_pixel(image, x as u32, y as u32, [0, 0, 0], alpha);
            }
        }
    }
}

/// Gaussian-like blur (three box passes) of one f32 plane. Large sigmas are
/// blurred on a downsampled copy and bilinearly upsampled back: the result is
/// visually identical (the blur removes everything finer than the
/// downsample) and the cost stops growing with sigma.
fn blur_plane(plane: &mut [f32], width: usize, height: usize, sigma: f64) {
    if sigma <= 0.5 || width == 0 || height == 0 {
        return;
    }
    let factor = blur_downsample_factor(sigma);
    if factor == 1 {
        let box_radius = box_radius_for(sigma);
        let mut scratch = vec![0.0f32; width * height];
        for _ in 0..3 {
            box_blur_horizontal(plane, &mut scratch, width, height, box_radius);
            box_blur_vertical(&scratch, plane, width, height, box_radius);
        }
        return;
    }
    let small_width = width.div_ceil(factor);
    let small_height = height.div_ceil(factor);
    let mut small = vec![0.0f32; small_width * small_height];
    for sy in 0..small_height {
        for sx in 0..small_width {
            let x_end = ((sx + 1) * factor).min(width);
            let y_end = ((sy + 1) * factor).min(height);
            let mut sum = 0.0f32;
            for y in sy * factor..y_end {
                sum += plane[y * width + sx * factor..y * width + x_end]
                    .iter()
                    .sum::<f32>();
            }
            let count = ((x_end - sx * factor) * (y_end - sy * factor)) as f32;
            small[sy * small_width + sx] = sum / count;
        }
    }
    blur_plane(&mut small, small_width, small_height, sigma / factor as f64);
    upsample_bilinear(
        &small,
        small_width,
        small_height,
        factor,
        width,
        height,
        |index, value| plane[index] = value,
    );
}

/// Downsampling applied before blurring with `sigma`, keeping the effective
/// sigma on the small image around 2.5 px or more.
fn blur_downsample_factor(sigma: f64) -> usize {
    ((sigma / 2.5).floor() as usize).clamp(1, 16)
}

/// Bilinearly stretches `small` (a `factor`-times downsampled plane) back to
/// `width` × `height`, handing each output pixel's index and value to `write`.
fn upsample_bilinear(
    small: &[f32],
    small_width: usize,
    small_height: usize,
    factor: usize,
    width: usize,
    height: usize,
    mut write: impl FnMut(usize, f32),
) {
    let scale = 1.0 / factor as f32;
    let max_x = (small_width - 1) as f32;
    let max_y = (small_height - 1) as f32;
    // Horizontal weights repeat per column, so compute them once.
    let columns: Vec<(usize, usize, f32)> = (0..width)
        .map(|x| {
            let fx = ((x as f32 + 0.5) * scale - 0.5).clamp(0.0, max_x);
            let x0 = fx.floor() as usize;
            (x0, (x0 + 1).min(small_width - 1), fx - x0 as f32)
        })
        .collect();
    for y in 0..height {
        let fy = ((y as f32 + 0.5) * scale - 0.5).clamp(0.0, max_y);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(small_height - 1);
        let ty = fy - y0 as f32;
        let row0 = &small[y0 * small_width..(y0 + 1) * small_width];
        let row1 = &small[y1 * small_width..(y1 + 1) * small_width];
        for (x, &(x0, x1, tx)) in columns.iter().enumerate() {
            let top = row0[x0] + (row0[x1] - row0[x0]) * tx;
            let bottom = row1[x0] + (row1[x1] - row1[x0]) * tx;
            write(y * width + x, top + (bottom - top) * ty);
        }
    }
}

/// Box blur along a row/column with clamp-to-edge sampling, so borders keep
/// their brightness instead of fading toward black.
fn box_blur_horizontal(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let last = width - 1;
        let mut sum: f32 =
            (0..=radius).map(|dx| row[dx.min(last)]).sum::<f32>() + row[0] * radius as f32;
        for x in 0..width {
            dst[y * width + x] = sum / window;
            sum += row[(x + radius + 1).min(last)];
            sum -= row[x.saturating_sub(radius)];
        }
    }
}

fn box_blur_vertical(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    let last = height - 1;
    for x in 0..width {
        let mut sum: f32 = (0..=radius)
            .map(|dy| src[dy.min(last) * width + x])
            .sum::<f32>()
            + src[x] * radius as f32;
        for y in 0..height {
            dst[y * width + x] = sum / window;
            sum += src[(y + radius + 1).min(last) * width + x];
            sum -= src[y.saturating_sub(radius) * width + x];
        }
    }
}

fn paint_disc(
    image: &mut RgbaImage,
    clip: Rect,
    cx: f64,
    cy: f64,
    radius: f64,
    color: [u8; 3],
    opacity: f64,
) {
    if opacity <= 0.0 || radius <= 0.0 {
        return;
    }
    let x0 = (cx - radius - 1.0).floor().max(clip.x) as i64;
    let y0 = (cy - radius - 1.0).floor().max(clip.y) as i64;
    let x1 = (cx + radius + 1.0).ceil().min(clip.right()) as i64;
    let y1 = (cy + radius + 1.0).ceil().min(clip.bottom()) as i64;
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            blend_pixel(image, x as u32, y as u32, color, coverage * opacity);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_ring(
    image: &mut RgbaImage,
    clip: Rect,
    cx: f64,
    cy: f64,
    radius: f64,
    line_width: f64,
    color: [u8; 3],
    opacity: f64,
) {
    if opacity <= 0.0 || radius <= 0.0 {
        return;
    }
    let outer = radius;
    let inner = (radius - line_width).max(0.0);
    let x0 = (cx - outer - 1.0).floor().max(clip.x) as i64;
    let y0 = (cy - outer - 1.0).floor().max(clip.y) as i64;
    let x1 = (cx + outer + 1.0).ceil().min(clip.right()) as i64;
    let y1 = (cy + outer + 1.0).ceil().min(clip.bottom()) as i64;
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let coverage =
                (outer - distance + 0.5).clamp(0.0, 1.0) * (distance - inner + 0.5).clamp(0.0, 1.0);
            blend_pixel(image, x as u32, y as u32, color, coverage * opacity);
        }
    }
}

/// Captured cursors are drawn larger than life so 100% reads like an
/// editor cursor rather than a 24 px system pointer.
const CAPTURED_CURSOR_BOOST: f64 = 2.5;
/// Height of a styled cursor as a fraction of the media height: Cap's 60 px
/// on a 1080 px tall screen.
const ASSET_CURSOR_HEIGHT: f64 = 60.0 / 1080.0;
/// Motion smear length as a fraction of the distance the cursor travels in
/// one 60 Hz frame (Cap's default amount, Screen Studio semantics).
const CURSOR_SMEAR_AMOUNT: f64 = 1.0;
/// Longest smear, in cursor widths; only pathological jumps hit it.
const CURSOR_SMEAR_MAX_WIDTHS: f64 = 4.0;
const CURSOR_SMEAR_TAPS: usize = 21;

/// Canvas-space trail for a cursor moving at `velocity` (normalized media
/// units per second) drawn `width` pixels wide.
fn cursor_smear(velocity: (f64, f64), scale: (f64, f64), width: f64) -> (f64, f64) {
    let smear = (
        velocity.0 * scale.0 / 60.0 * CURSOR_SMEAR_AMOUNT,
        velocity.1 * scale.1 / 60.0 * CURSOR_SMEAR_AMOUNT,
    );
    let length = smear.0.hypot(smear.1);
    let limit = CURSOR_SMEAR_MAX_WIDTHS * width;
    if !length.is_finite() || length < 0.5 {
        (0.0, 0.0)
    } else if length > limit {
        (smear.0 * limit / length, smear.1 * limit / length)
    } else {
        smear
    }
}

/// Where a cursor bitmap lands on the canvas: hotspot position, drawn size
/// and rotation about the hotspot.
#[derive(Clone, Copy)]
struct BitmapPlacement {
    hotspot_x: f64,
    hotspot_y: f64,
    width: f64,
    height: f64,
    tilt_degrees: f64,
    /// Motion trail in canvas pixels: the cursor is smeared this far behind
    /// its position.
    smear: (f64, f64),
}

/// Draws a captured cursor image with its hotspot at the pointer location,
/// resampling bilinearly in premultiplied space so scaled edges stay clean.
fn paint_cursor_bitmap(
    image: &mut RgbaImage,
    clip: Rect,
    bitmap: &PointerBitmap,
    placement: BitmapPlacement,
    opacity: f64,
    shadow_only: bool,
) {
    let source = &bitmap.image;
    let (source_width, source_height) = (source.width() as f64, source.height() as f64);
    let anchor_x = bitmap.anchor.x * placement.width;
    let anchor_y = bitmap.anchor.y * placement.height;
    let (sin, cos) = placement.tilt_degrees.to_radians().sin_cos();
    let corners = [
        (0.0, 0.0),
        (placement.width, 0.0),
        (0.0, placement.height),
        (placement.width, placement.height),
    ]
    .map(|(lx, ly)| {
        let (lx, ly) = (lx - anchor_x, ly - anchor_y);
        (
            placement.hotspot_x + lx * cos - ly * sin,
            placement.hotspot_y + lx * sin + ly * cos,
        )
    });
    // A pixel shows the sprite averaged along the trail behind it, so the
    // painted box extends opposite the motion.
    let (smear_x, smear_y) = placement.smear;
    let smear_length = smear_x.hypot(smear_y);
    let taps = if smear_length < 0.5 {
        1
    } else {
        CURSOR_SMEAR_TAPS
    };
    // The trail in the sprite's local (unrotated) frame.
    let local_smear = (
        smear_x * cos + smear_y * sin,
        -smear_x * sin + smear_y * cos,
    );
    let min_x = corners.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - 1.0 - smear_x.max(0.0);
    let max_x = corners
        .iter()
        .map(|p| p.0)
        .fold(f64::NEG_INFINITY, f64::max)
        + 1.0
        - smear_x.min(0.0);
    let min_y = corners.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - 1.0 - smear_y.max(0.0);
    let max_y = corners
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max)
        + 1.0
        - smear_y.min(0.0);
    let x0 = min_x.floor().max(clip.x) as i64;
    let y0 = min_y.floor().max(clip.y) as i64;
    let x1 = max_x.ceil().min(clip.right()) as i64;
    let y1 = max_y.ceil().min(clip.bottom()) as i64;
    let scale_x = source_width / placement.width;
    let scale_y = source_height / placement.height;
    let sample = |sx: f64, sy: f64| -> [f64; 4] {
        // Bilinear tap in premultiplied space; outside the image is
        // transparent.
        let fx = sx - 0.5;
        let fy = sy - 0.5;
        let ix = fx.floor();
        let iy = fy.floor();
        let tx = fx - ix;
        let ty = fy - iy;
        let mut out = [0.0; 4];
        for (dx, dy, weight) in [
            (0.0, 0.0, (1.0 - tx) * (1.0 - ty)),
            (1.0, 0.0, tx * (1.0 - ty)),
            (0.0, 1.0, (1.0 - tx) * ty),
            (1.0, 1.0, tx * ty),
        ] {
            let px = ix + dx;
            let py = iy + dy;
            if weight <= 0.0 || px < 0.0 || py < 0.0 || px >= source_width || py >= source_height {
                continue;
            }
            let pixel = source.get_pixel(px as u32, py as u32);
            for channel in 0..4 {
                out[channel] += f64::from(pixel[channel]) * weight;
            }
        }
        out
    };
    for y in y0..y1 {
        for x in x0..x1 {
            // Map the pixel centre back into the unrotated artwork box.
            let dx = x as f64 + 0.5 - placement.hotspot_x;
            let dy = y as f64 + 0.5 - placement.hotspot_y;
            let lx = dx * cos + dy * sin + anchor_x;
            let ly = -dx * sin + dy * cos + anchor_y;
            let mut accumulated = [0.0; 4];
            for tap_index in 0..taps {
                let t = if taps == 1 {
                    0.0
                } else {
                    tap_index as f64 / (taps - 1) as f64
                };
                let tx = lx + local_smear.0 * t;
                let ty = ly + local_smear.1 * t;
                if tx < -1.0
                    || ty < -1.0
                    || tx > placement.width + 1.0
                    || ty > placement.height + 1.0
                {
                    continue;
                }
                let tapped = sample(tx * scale_x, ty * scale_y);
                for channel in 0..4 {
                    accumulated[channel] += tapped[channel];
                }
            }
            let [r, g, b, a] = accumulated.map(|value| value / taps as f64);
            let coverage = a / 255.0;
            if coverage <= 0.002 {
                continue;
            }
            if shadow_only {
                blend_pixel(image, x as u32, y as u32, [0, 0, 0], coverage * opacity);
                continue;
            }
            let color = [
                (r / coverage).round().clamp(0.0, 255.0) as u8,
                (g / coverage).round().clamp(0.0, 255.0) as u8,
                (b / coverage).round().clamp(0.0, 255.0) as u8,
            ];
            blend_pixel(image, x as u32, y as u32, color, coverage * opacity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::pointer_timeline::PointerPressFrame;

    fn checker(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let left = x < width / 2;
                let top = y < height / 2;
                let color = match (left, top) {
                    (true, true) => [255, 0, 0, 255],
                    (false, true) => [0, 255, 0, 255],
                    (true, false) => [0, 0, 255, 255],
                    (false, false) => [255, 255, 0, 255],
                };
                image.put_pixel(x, y, Rgba(color));
            }
        }
        image
    }

    fn flat_style(background: u32) -> SceneStyle {
        SceneStyle {
            background: SceneBackground::Solid(background),
            padding: 0,
            corners: 0,
            shadow: 0,
            border: false,
            aspect: Some(1.0),
            ..SceneStyle::default()
        }
    }

    fn compose(compositor: &SceneCompositor, source: &RgbaImage) -> RgbaImage {
        compositor.compose(FrameInput {
            source,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: None,
        })
    }

    #[test]
    fn window_frame_draws_a_title_bar_above_the_media() {
        let source = RgbaImage::from_pixel(80, 80, Rgba([10, 200, 30, 255]));
        let style = SceneStyle {
            window_frame: WindowFrame::Light,
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 80, 80).unwrap();
        let geometry = compositor.geometry();
        assert!(geometry.title_height > 0.0);
        assert!((geometry.card().y - (geometry.media.y - geometry.title_height)).abs() < 1e-9);
        let output = compose(&compositor, &source);
        let bar_y = (geometry.media.y - geometry.title_height * 0.5) as u32;
        let media_y = (geometry.media.y + 2.0) as u32;
        let x = (geometry.media.x + geometry.media.width * 0.8) as u32;
        assert_eq!(output.get_pixel(x, bar_y).0[..3], [0xe9, 0xe9, 0xeb]);
        assert_eq!(output.get_pixel(x, media_y).0[..3], [10, 200, 30]);
        let light_x = (geometry.media.x + geometry.title_height * 0.7) as u32;
        assert_eq!(output.get_pixel(light_x, bar_y).0[..3], [0xff, 0x5f, 0x57]);
    }

    #[test]
    fn layout_scales_with_canvas_height() {
        let style = SceneStyle {
            padding: 10,
            border: true,
            border_thickness: 50,
            ..SceneStyle::default()
        };
        let small = SceneGeometry::layout(600.0, 600.0, 1600.0, 900.0, &style);
        let large = SceneGeometry::layout(1200.0, 1200.0, 1600.0, 900.0, &style);
        assert!((large.border_width - small.border_width * 2.0).abs() < 1e-9);
        assert!((large.media.x - small.media.x * 2.0).abs() < 1e-6);
        assert!((large.media.width - small.media.width * 2.0).abs() < 1e-6);
        assert!((large.radius - small.radius * 2.0).abs() < 1e-9);
        assert!(large.media.right() <= 1200.0 && large.media.bottom() <= 1200.0);
    }

    #[test]
    fn export_canvas_follows_aspect_and_stays_even() {
        let style = SceneStyle {
            aspect: Some(16.0 / 9.0),
            ..SceneStyle::default()
        };
        assert_eq!(style.export_canvas_size(1000, 1000, 1080), (1920, 1080));
        let auto = SceneStyle {
            aspect: None,
            ..SceneStyle::default()
        };
        assert_eq!(auto.export_canvas_size(1001, 1000, 719), (718, 718));
    }

    #[test]
    fn style_round_trips_through_json_with_defaults() {
        let style = SceneStyle {
            background: SceneBackground::Gradient {
                colors: [1, 2, 3],
                angle_degrees: 45.0,
            },
            transform: SceneTransform {
                rotation_y: 12.0,
                ..SceneTransform::IDENTITY
            },
            watermark: Some(Watermark {
                text: "demo".into(),
                ..Watermark::default()
            }),
            ..SceneStyle::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        let parsed: SceneStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, style);
        // Older documents without the new fields still load.
        let minimal: SceneStyle = serde_json::from_str(r#"{"padding":5}"#).unwrap();
        assert_eq!(minimal.padding, 5);
        assert_eq!(minimal.transform, SceneTransform::IDENTITY);
    }

    #[test]
    fn compositor_paints_background_outside_and_media_inside() {
        let style = SceneStyle {
            padding: 20,
            ..flat_style(0x102030)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 100, 100).unwrap();
        let output = compose(&compositor, &checker(100, 100));
        assert_eq!(output.get_pixel(2, 2).0, [0x10, 0x20, 0x30, 255]);
        let media = compositor.geometry().media;
        let inside_x = (media.x + 5.0) as u32;
        let inside_y = (media.y + 5.0) as u32;
        assert_eq!(output.get_pixel(inside_x, inside_y).0, [255, 0, 0, 255]);
        let far_x = (media.right() - 5.0) as u32;
        let far_y = (media.bottom() - 5.0) as u32;
        assert_eq!(output.get_pixel(far_x, far_y).0, [255, 255, 0, 255]);
    }

    #[test]
    fn viewport_zoom_crops_the_media() {
        let compositor = SceneCompositor::new(&flat_style(0), 100, 100, 100, 100).unwrap();
        let viewport = ViewportFrame {
            magnification: 2.0,
            anchor: NormalizedPoint { x: 0.25, y: 0.25 },
            tilt: Tilt::default(),
        };
        let output = compositor.compose(FrameInput {
            source: &checker(100, 100),
            overlay: None,
            viewport,
            pointer: None,
            camera: None,
        });
        assert_eq!(output.get_pixel(5, 5).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(94, 94).0, [255, 0, 0, 255]);
    }

    #[test]
    fn rounded_corners_reveal_the_background() {
        let style = SceneStyle {
            corners: 100,
            ..flat_style(0xffffff)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let output = compose(&compositor, &checker(200, 200));
        assert_eq!(output.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(output.get_pixel(100, 5).0, [0, 255, 0, 255]);
    }

    #[test]
    fn scale_and_position_move_the_media() {
        let style = SceneStyle {
            transform: SceneTransform {
                scale: 0.5,
                position_x: 0.5,
                ..SceneTransform::IDENTITY
            },
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let output = compose(&compositor, &checker(200, 200));
        // Half-size media centred at x = 150: canvas 100..200 horizontally.
        assert_eq!(output.get_pixel(20, 100).0, [0, 0, 0, 255]);
        assert_eq!(output.get_pixel(110, 60).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(190, 140).0, [255, 255, 0, 255]);
        let projection = compositor.projection(ViewportFrame::default());
        assert!(projection.contains(150.0, 100.0));
        assert!(!projection.contains(50.0, 100.0));
    }

    #[test]
    fn rotation_projects_a_perspective_quad_and_round_trips() {
        let style = SceneStyle {
            transform: SceneTransform {
                rotation_y: 35.0,
                rotation_x: 10.0,
                perspective: 0.8,
                ..SceneTransform::IDENTITY
            },
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 300, 200, 300, 200).unwrap();
        let projection = compositor.projection(ViewportFrame::default());
        // The rotated side recedes: the right edge is shorter than the left.
        let left_height = (projection.quad[3].1 - projection.quad[0].1).abs();
        let right_height = (projection.quad[2].1 - projection.quad[1].1).abs();
        assert!(right_height < left_height, "{right_height} {left_height}");
        for (u, v) in [(0.1, 0.2), (0.5, 0.5), (0.9, 0.8)] {
            let (x, y) = projection.project(u, v);
            let (back_u, back_v) = projection.unproject(x, y);
            assert!((back_u - u).abs() < 1e-6 && (back_v - v).abs() < 1e-6);
        }
        let output = compose(&compositor, &checker(300, 200));
        let (x, y) = projection.project(0.25, 0.25);
        assert_eq!(output.get_pixel(x as u32, y as u32).0, [255, 0, 0, 255]);
        let (x, y) = projection.project(0.75, 0.75);
        assert_eq!(output.get_pixel(x as u32, y as u32).0, [255, 255, 0, 255]);
        // Outside the quad (right of the receding edge) the background shows.
        assert!(projection.bounds.right() < 296.0, "{:?}", projection.bounds);
        assert_eq!(output.get_pixel(296, 100).0, [0, 0, 0, 255]);
    }

    #[test]
    fn tilt_is_applied_per_frame_without_disturbing_the_cache() {
        let compositor = SceneCompositor::new(&flat_style(0), 120, 120, 120, 120).unwrap();
        let flat = compose(&compositor, &checker(120, 120));
        let tilted = compositor.compose(FrameInput {
            source: &checker(120, 120),
            overlay: None,
            viewport: ViewportFrame {
                tilt: Tilt {
                    x: 0.0,
                    y: 40.0,
                    z: 0.0,
                },
                ..ViewportFrame::default()
            },
            pointer: None,
            camera: None,
        });
        assert_ne!(flat.as_raw(), tilted.as_raw());
        let flat_again = compose(&compositor, &checker(120, 120));
        assert_eq!(flat.as_raw(), flat_again.as_raw());
    }

    #[test]
    fn gradient_shadow_border_effects_render() {
        let style = SceneStyle {
            background: SceneBackground::Gradient {
                colors: [0xfa4f94, 0x6652f2, 0x4ad6cc],
                angle_degrees: 135.0,
            },
            padding: 30,
            corners: 40,
            shadow: 80,
            shadow_style: 1,
            border: true,
            border_thickness: 40,
            border_color: 0xffc928,
            border_opacity: 100,
            aspect: Some(16.0 / 9.0),
            background_blur: 40,
            background_noise: 30,
            vignette: 50,
            watermark: Some(Watermark {
                text: "Screendrop".into(),
                ..Watermark::default()
            }),
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 320, 180, 64, 36).unwrap();
        let output = compose(&compositor, &checker(64, 36));
        let geometry = compositor.geometry();
        let border_x = (geometry.media.x - geometry.border_width * 0.5) as u32;
        let border_y = (geometry.media.y + geometry.media.height * 0.5) as u32;
        let border = output.get_pixel(border_x, border_y).0;
        assert!(
            border[0] > 180 && border[1] > 130 && border[2] < 120,
            "{border:?}"
        );
        // Vignette darkens the corner more than the edge midpoint.
        let corner = output.get_pixel(0, 0).0;
        let edge = output.get_pixel(160, 0).0;
        let sum = |pixel: [u8; 4]| pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32;
        assert!(sum(corner) < sum(edge));
    }

    #[test]
    fn captured_cursor_bitmap_paints_with_hotspot_on_the_pointer() {
        use crate::recording::model::PointerArtwork;
        use crate::recording::pointer_timeline::PointerBitmap;
        use base64::Engine;
        // 8x8 red square, hotspot at the centre.
        let square = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        square.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let artwork = PointerArtwork {
            artwork_id: "square".into(),
            image_data_base64: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
            anchor_point: NormalizedPoint { x: 0.5, y: 0.5 },
            reference_width: 0.1,
            reference_height: 0.1,
            shape: None,
        };
        let bitmap = std::sync::Arc::new(PointerBitmap::decode(&artwork).unwrap());
        let style = SceneStyle {
            pointer: PointerStyle {
                shadow: false,
                click_effects: false,
                ..PointerStyle::default()
            },
            ..flat_style(0)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let white = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: Some("square".into()),
                bitmap: Some(bitmap),
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
                velocity: (0.0, 0.0),
                press: None,
            },
        };
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        let geometry = compositor.geometry();
        let centre_x = (geometry.media.x + geometry.media.width * 0.5) as u32;
        let centre_y = (geometry.media.y + geometry.media.height * 0.5) as u32;
        // The square is centred on the pointer: red at the centre, and the
        // half-size is 0.05 * media * boost, so 2.5x that is white again.
        // Bilinear upscaling feathers each edge over one source pixel.
        assert_eq!(output.get_pixel(centre_x, centre_y).0, [255, 0, 0, 255]);
        let half = (0.05 * geometry.media.width * CAPTURED_CURSOR_BOOST) as u32;
        assert_eq!(
            output.get_pixel(centre_x + half - 4, centre_y).0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            output.get_pixel(centre_x - half + 4, centre_y).0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            output.get_pixel(centre_x + half + 3, centre_y).0,
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn overlay_blends_in_media_space() {
        let compositor = SceneCompositor::new(&flat_style(0), 100, 100, 100, 100).unwrap();
        let white = RgbaImage::from_pixel(100, 100, Rgba([255, 255, 255, 255]));
        let mut overlay = RgbaImage::new(100, 100);
        for y in 40..60 {
            for x in 40..60 {
                overlay.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: Some(&overlay),
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: None,
        });
        assert_eq!(output.get_pixel(50, 50).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(10, 10).0, [255, 255, 255, 255]);
    }

    #[test]
    fn launcher_camera_crop_matches_recording_for_every_shape_and_mirror_setting() {
        let camera = RgbaImage::from_fn(320, 180, |x, y| {
            Rgba([(x % 256) as u8, y as u8, 80, 255])
        });
        for shape in CameraShape::ALL {
            for mirror in [false, true] {
                let overlay = CameraOverlay {
                    position: WatermarkPosition::TopLeft,
                    size: 44,
                    margin: 3,
                    shape,
                    mirror,
                    shadow: false,
                    ..CameraOverlay::default()
                };
                let style = SceneStyle { camera: overlay, ..flat_style(0xfbfbfb) };
                let compositor = SceneCompositor::new(&style, 400, 400, 400, 400).unwrap();
                let source = RgbaImage::from_pixel(400, 400, Rgba([251, 251, 251, 255]));
                let recording = compositor.compose(FrameInput {
                    source: &source,
                    overlay: None,
                    viewport: ViewportFrame::default(),
                    pointer: None,
                    camera: Some(&camera),
                });
                let preview = camera_framing_preview(&camera, overlay);
                for (x, y, pixel) in preview.enumerate_pixels() {
                    assert_eq!(pixel, recording.get_pixel(x, y), "{shape:?}, mirror={mirror}, ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn camera_overlay_is_cover_fitted_mirrored_and_masked() {
        let style = SceneStyle {
            camera: CameraOverlay {
                enabled: true,
                position: WatermarkPosition::BottomRight,
                size: 40,
                shape: CameraShape::Circle,
                mirror: true,
                margin: 5,
                shadow: true,
            },
            ..flat_style(0x000000)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let white = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        // Camera frame: left half red, right half green (wider than tall).
        let camera = RgbaImage::from_fn(160, 90, |x, _| {
            if x < 80 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 255, 0, 255])
            }
        });
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&PointerOverlay {
                frame: PointerFrame {
                    location: NormalizedPoint { x: 0.1, y: 0.1 },
                    artwork_id: None,
                    bitmap: None,
                    magnification: 1.0,
                    tilt_degrees: 0.0,
                    opacity: 0.0,
                    blur_radius: 0.0,
                    velocity: (0.0, 0.0),
                    press: None,
                },
            }),
            camera: Some(&camera),
        });
        let rect = style.camera.rect(200.0, 200.0);
        let cx = rect.x + rect.width * 0.5;
        let cy = rect.y + rect.height * 0.5;
        // Mirrored: the camera's left half (red) appears on the right.
        assert_eq!(
            output
                .get_pixel((cx + rect.width * 0.3) as u32, cy as u32)
                .0,
            [255, 0, 0, 255]
        );
        assert_eq!(
            output
                .get_pixel((cx - rect.width * 0.3) as u32, cy as u32)
                .0,
            [0, 255, 0, 255]
        );
        // Circle mask: the square's corner is not camera colour.
        let corner = output
            .get_pixel((rect.x + 2.0) as u32, (rect.y + 2.0) as u32)
            .0;
        assert!(
            corner != [255, 0, 0, 255] && corner != [0, 255, 0, 255],
            "{corner:?}"
        );
        // Disabled camera paints nothing.
        let disabled = SceneStyle {
            camera: CameraOverlay {
                enabled: false,
                ..style.camera
            },
            ..style.clone()
        };
        let compositor = SceneCompositor::new(&disabled, 200, 200, 200, 200).unwrap();
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: Some(&camera),
        });
        assert_eq!(
            output.get_pixel(cx as u32, cy as u32).0,
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn pointer_and_click_ring_paint_inside_the_media() {
        let style = SceneStyle {
            padding: 10,
            ..flat_style(0)
        };
        let compositor = SceneCompositor::new(&style, 200, 200, 200, 200).unwrap();
        let white = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: None,
                bitmap: None,
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
                velocity: (0.0, 0.0),
                press: Some(PointerPressFrame {
                    location: NormalizedPoint { x: 0.5, y: 0.5 },
                    progress: 0.5,
                }),
            },
        };
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        let media = compositor.geometry().media;
        let center = (
            (media.x + media.width * 0.5) as u32,
            (media.y + media.height * 0.5) as u32,
        );
        let mut painted = false;
        for dy in 0..12 {
            for dx in 0..12 {
                if output.get_pixel(center.0 + dx, center.1 + dy).0 != [255, 255, 255, 255] {
                    painted = true;
                }
            }
        }
        assert!(painted);
        // Hidden pointer paints nothing.
        let hidden = SceneStyle {
            pointer: PointerStyle {
                visible: false,
                click_effects: false,
                ..PointerStyle::default()
            },
            ..style
        };
        let compositor = SceneCompositor::new(&hidden, 200, 200, 200, 200).unwrap();
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        assert_eq!(
            output.get_pixel(center.0 + 3, center.1 + 3).0,
            [255, 255, 255, 255]
        );
    }
}

#[cfg(test)]
mod cursor_smear_tests {
    use super::*;

    fn compose_with_velocity(velocity: (f64, f64)) -> (RgbaImage, u32, u32) {
        let style = SceneStyle {
            pointer: PointerStyle {
                click_effects: false,
                hide_when_idle: false,
                ..PointerStyle::default()
            },
            ..SceneStyle::default()
        };
        let compositor = SceneCompositor::new(&style, 400, 300, 400, 300).unwrap();
        let white = RgbaImage::from_pixel(400, 300, Rgba([255, 255, 255, 255]));
        let pointer = PointerOverlay {
            frame: PointerFrame {
                location: NormalizedPoint { x: 0.5, y: 0.5 },
                artwork_id: None,
                bitmap: None,
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
                velocity,
                press: None,
            },
        };
        let output = compositor.compose(FrameInput {
            source: &white,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: Some(&pointer),
            camera: None,
        });
        let media = compositor.geometry().media;
        (
            output,
            (media.x + media.width * 0.5) as u32,
            (media.y + media.height * 0.5) as u32,
        )
    }

    #[test]
    fn fast_cursor_leaves_a_trail_behind_its_motion() {
        let (still, x, y) = compose_with_velocity((0.0, 0.0));
        // Ten media widths per second: a long streak to the left.
        let (moving, _, _) = compose_with_velocity((10.0, 0.0));
        let at = |image: &RgbaImage, dx: i64| image.get_pixel((x as i64 + dx) as u32, y + 4).0;
        // Left of the arrow is plain media when still, streaked when moving.
        assert_eq!(at(&still, -30), at(&still, -120));
        assert_ne!(at(&moving, -30), at(&still, -30));
    }
}

#[cfg(test)]
mod box_blur_edge_tests {
    use super::*;

    #[test]
    fn box_blur_keeps_flat_image_flat_at_edges() {
        let (w, h) = (40usize, 30usize);
        let src = vec![200.0f32; w * h];
        let mut tmp = vec![0.0f32; w * h];
        let mut out = vec![0.0f32; w * h];
        box_blur_horizontal(&src, &mut tmp, w, h, 7);
        box_blur_vertical(&tmp, &mut out, w, h, 7);
        for v in out {
            assert!((v - 200.0).abs() < 0.01, "edge darkened: {v}");
        }
    }
}
