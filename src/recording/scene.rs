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

use super::{
    model::NormalizedPoint,
    overlays::pointer_press_effect_geometry,
    pointer_timeline::PointerFrame,
    viewport::{visible_rect, Tilt, ViewportFrame},
};

/// The preview canvas height every scene dimension is expressed against.
/// Padding, border thickness, corner radius, and shadow spread scale linearly
/// with the actual canvas height so the export matches the preview.
pub const REFERENCE_CANVAS_HEIGHT: f64 = 600.0;

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
        }
    }
}

/// Everything that styles a scene except the media itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SceneStyle {
    pub background: SceneBackground,
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
}

impl Default for SceneStyle {
    fn default() -> Self {
        Self {
            background: SceneBackground::default(),
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
        !self.transform.is_identity()
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
        let available_width = (canvas_width - inset * 2.0).max(1.0);
        let available_height = (canvas_height - inset * 2.0).max(1.0);
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
            y: inset + (available_height - height) * 0.5,
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
            shadow,
            ui_scale,
        }
    }

    /// The card rect at scale 1: media plus border.
    pub fn card(&self) -> Rect {
        self.media.inset(-self.border_width)
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
        let mut background = render_background(&style.background, canvas_width, canvas_height)?;
        if style.background_blur > 0 {
            let sigma = style.background_blur as f64 / 100.0 * 40.0 * geometry.ui_scale;
            blur_image(&mut background, sigma);
        }
        if style.background_noise > 0 {
            apply_noise(&mut background, style.background_noise as f64 / 100.0);
        }
        let vignette = (style.vignette > 0)
            .then(|| vignette_map(canvas_width, canvas_height, style.vignette as f64 / 100.0));
        let watermark = style
            .watermark
            .as_ref()
            .filter(|watermark| !watermark.text.trim().is_empty())
            .and_then(|watermark| render_watermark(watermark, canvas_width, canvas_height).ok());
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
        if self.geometry.border_width > 0.0 && self.style.border_opacity > 0 {
            self.paint_border(&mut pixels, &projection);
        }
        *cache = Some(CardLayer {
            transform,
            pixels: pixels.clone(),
        });
        pixels
    }

    /// Signed distance (media pixels) from `(u, v)` to the rounded media
    /// rectangle, negative inside.
    fn media_distance(&self, u: f64, v: f64) -> f64 {
        rounded_rect_distance(
            u,
            v,
            self.geometry.media.width,
            self.geometry.media.height,
            self.geometry.radius,
        )
    }

    fn pixel_range(&self, projection: &MediaProjection, margin: f64) -> (u32, u32, u32, u32) {
        let x0 = (projection.bounds.x - margin).floor().max(0.0) as u32;
        let y0 = (projection.bounds.y - margin).floor().max(0.0) as u32;
        let x1 = ((projection.bounds.right() + margin).ceil().max(0.0) as u32).min(self.width);
        let y1 = ((projection.bounds.bottom() + margin).ceil().max(0.0) as u32).min(self.height);
        (x0, y0, x1, y1)
    }

    fn paint_border(&self, output: &mut RgbaImage, projection: &MediaProjection) {
        let tint = unpack(self.style.border_color);
        let alpha = self.style.border_opacity as f64 / 100.0;
        let border = self.geometry.border_width;
        let (x0, y0, x1, y1) = self.pixel_range(projection, border * 2.0 + 2.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let (u, v) = projection.unproject(px, py);
                if !(-0.3..=1.3).contains(&u) || !(-0.3..=1.3).contains(&v) {
                    continue;
                }
                let pixel_size = projection.pixel_size_at(px, py);
                let distance = self.media_distance(u, v);
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
        let source_width = source.width() as f64;
        let source_height = source.height() as f64;
        if source_width <= 0.0 || source_height <= 0.0 {
            return;
        }
        let overlay = overlay.filter(|layer| layer.width() > 0 && layer.height() > 0);
        let (left, top, visible) = visible_rect(viewport);
        let (x0, y0, x1, y1) = self.pixel_range(projection, 2.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let (u, v) = projection.unproject(px, py);
                if !(-0.05..=1.05).contains(&u) || !(-0.05..=1.05).contains(&v) {
                    continue;
                }
                let pixel_size = projection.pixel_size_at(px, py);
                let coverage = (0.5 - self.media_distance(u, v) / pixel_size).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let media_u = left + u.clamp(0.0, 1.0) * visible;
                let media_v = top + v.clamp(0.0, 1.0) * visible;
                let mut sample = sample_bilinear(
                    source,
                    media_u * source_width - 0.5,
                    media_v * source_height - 0.5,
                );
                if let Some(layer) = overlay {
                    let over = sample_bilinear(
                        layer,
                        media_u * layer.width() as f64 - 0.5,
                        media_v * layer.height() as f64 - 0.5,
                    );
                    let alpha = over[3] as f64 / 255.0;
                    for channel in 0..3 {
                        sample[channel] = (sample[channel] as f64
                            + (over[channel] as f64 - sample[channel] as f64) * alpha)
                            .round() as u8;
                    }
                    sample[3] =
                        (sample[3] as f64 + (255.0 - sample[3] as f64) * alpha).round() as u8;
                }
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
        // Preview draws the cursor icon at 25px on a reference-height canvas.
        let size = (25.0 * frame.magnification).clamp(15.0, 34.0)
            * self.geometry.ui_scale
            * pointer_scale
            * local_scale;
        if style.shadow {
            paint_cursor(
                output,
                clip,
                x + size * 0.06,
                y + size * 0.08,
                size,
                frame.tilt_degrees,
                opacity * 0.35,
                true,
            );
        }
        paint_cursor(output, clip, x, y, size, frame.tilt_degrees, opacity, false);
    }
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

fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 3], alpha: f64) {
    if x >= image.width() || y >= image.height() {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let pixel = image.get_pixel_mut(x, y);
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
    let box_radius = box_radius_for(sigma);
    let mut channels: Vec<Vec<f32>> = (0..3)
        .map(|channel| {
            image
                .pixels()
                .map(|pixel| pixel[channel] as f32)
                .collect::<Vec<f32>>()
        })
        .collect();
    let mut scratch = vec![0.0f32; width * height];
    for channel in channels.iter_mut() {
        for _ in 0..3 {
            box_blur_horizontal(channel, &mut scratch, width, height, box_radius);
            box_blur_vertical(&scratch, channel, width, height, box_radius);
        }
    }
    for (index, pixel) in image.pixels_mut().enumerate() {
        pixel[0] = channels[0][index].round().clamp(0.0, 255.0) as u8;
        pixel[1] = channels[1][index].round().clamp(0.0, 255.0) as u8;
        pixel[2] = channels[2][index].round().clamp(0.0, 255.0) as u8;
    }
}

fn box_radius_for(sigma: f64) -> usize {
    // Three box passes of width w approximate sigma² = 3 (w² − 1) / 12.
    let box_width = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    (((box_width - 1.0) / 2.0).round() as usize).max(1)
}

/// Deterministic film grain.
fn apply_noise(image: &mut RgbaImage, amount: f64) {
    let strength = amount.clamp(0.0, 1.0) * 48.0;
    let width = image.width();
    for (index, pixel) in image.pixels_mut().enumerate() {
        let x = (index as u32 % width) as u64;
        let y = (index as u32 / width) as u64;
        let noise = (hash_noise(x, y) * 2.0 - 1.0) * strength;
        for channel in 0..3 {
            pixel[channel] = (pixel[channel] as f64 + noise).round().clamp(0.0, 255.0) as u8;
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
    let x0 = (projection.bounds.x - margin).floor().max(0.0) as usize;
    let y0 = (projection.bounds.y - margin + shadow.offset_y)
        .floor()
        .max(0.0) as usize;
    let x1 = ((projection.bounds.right() + margin).ceil().max(0.0) as usize).min(width);
    let y1 = ((projection.bounds.bottom() + margin + shadow.offset_y)
        .ceil()
        .max(0.0) as usize)
        .min(height);
    for y in y0..y1 {
        for x in x0..x1 {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5 - shadow.offset_y;
            let (u, v) = projection.unproject(px, py);
            if !(-0.3..=1.3).contains(&u) || !(-0.3..=1.3).contains(&v) {
                continue;
            }
            let pixel_size = projection.pixel_size_at(px, py);
            let distance = rounded_rect_distance(
                u,
                v,
                geometry.media.width,
                geometry.media.height,
                geometry.radius,
            );
            mask[y * width + x] = (0.5 - (distance - border) / pixel_size).clamp(0.0, 1.0) as f32;
        }
    }
    // A CSS blur radius corresponds to a Gaussian with sigma = radius / 2.
    let sigma = shadow.blur_radius * 0.5;
    if sigma > 0.5 {
        let box_radius = box_radius_for(sigma);
        let mut scratch = vec![0.0f32; width * height];
        for _ in 0..3 {
            box_blur_horizontal(&mask, &mut scratch, width, height, box_radius);
            box_blur_vertical(&scratch, &mut mask, width, height, box_radius);
        }
    }
    for y in 0..height {
        for x in 0..width {
            let alpha = mask[y * width + x] as f64 * shadow.opacity;
            if alpha > 0.001 {
                blend_pixel(image, x as u32, y as u32, [0, 0, 0], alpha);
            }
        }
    }
}

fn box_blur_horizontal(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let mut sum: f32 = row.iter().take(radius + 1).sum();
        for x in 0..width {
            dst[y * width + x] = sum / window;
            if x + radius + 1 < width {
                sum += row[x + radius + 1];
            }
            if x >= radius {
                sum -= row[x - radius];
            }
        }
    }
}

fn box_blur_vertical(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = (radius * 2 + 1) as f32;
    for x in 0..width {
        let mut sum: f32 = (0..(radius + 1).min(height))
            .map(|y| src[y * width + x])
            .sum();
        for y in 0..height {
            dst[y * width + x] = sum / window;
            if y + radius + 1 < height {
                sum += src[(y + radius + 1) * width + x];
            }
            if y >= radius {
                sum -= src[(y - radius) * width + x];
            }
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

/// Arrow cursor matching `icons/mouse-pointer.svg` (24-unit art box). The
/// tip of the arrow lands exactly on the pointer location.
const CURSOR_OUTLINE: [(f64, f64); 8] = [
    (4.2, 2.7),
    (4.2, 17.8),
    (8.3, 13.7),
    (11.55, 20.8),
    (14.6, 19.4),
    (11.45, 12.5),
    (17.3, 12.5),
    (4.2, 2.7),
];

#[allow(clippy::too_many_arguments)]
fn paint_cursor(
    image: &mut RgbaImage,
    clip: Rect,
    tip_x: f64,
    tip_y: f64,
    size: f64,
    tilt_degrees: f64,
    opacity: f64,
    shadow_only: bool,
) {
    const SUPERSAMPLE: usize = 3;
    let scale = size / 24.0;
    let stroke = 1.7 * scale * 0.5;
    let (sin, cos) = tilt_degrees.to_radians().sin_cos();
    let points: Vec<(f64, f64)> = CURSOR_OUTLINE
        .iter()
        .map(|(x, y)| {
            let lx = (x - CURSOR_OUTLINE[0].0) * scale;
            let ly = (y - CURSOR_OUTLINE[0].1) * scale;
            (tip_x + lx * cos - ly * sin, tip_y + lx * sin + ly * cos)
        })
        .collect();
    let min_x = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - stroke - 1.0;
    let max_x = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + stroke + 1.0;
    let min_y = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min) - stroke - 1.0;
    let max_y = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) + stroke + 1.0;
    let x0 = min_x.floor().max(clip.x) as i64;
    let y0 = min_y.floor().max(clip.y) as i64;
    let x1 = max_x.ceil().min(clip.right()) as i64;
    let y1 = max_y.ceil().min(clip.bottom()) as i64;
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f64;
    for y in y0..y1 {
        for x in x0..x1 {
            let mut fill = 0.0;
            let mut outline = 0.0;
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let px = x as f64 + (sx as f64 + 0.5) / SUPERSAMPLE as f64;
                    let py = y as f64 + (sy as f64 + 0.5) / SUPERSAMPLE as f64;
                    let distance = polygon_edge_distance(&points, px, py);
                    let inside = point_in_polygon(&points, px, py);
                    if distance <= stroke {
                        outline += 1.0;
                    } else if inside {
                        fill += 1.0;
                    }
                }
            }
            if shadow_only {
                let coverage = (fill + outline) / samples;
                if coverage > 0.0 {
                    blend_pixel(image, x as u32, y as u32, [0, 0, 0], coverage * opacity);
                }
                continue;
            }
            if fill > 0.0 {
                blend_pixel(
                    image,
                    x as u32,
                    y as u32,
                    [255, 255, 255],
                    fill / samples * opacity,
                );
            }
            if outline > 0.0 {
                blend_pixel(
                    image,
                    x as u32,
                    y as u32,
                    [17, 17, 17],
                    outline / samples * opacity,
                );
            }
        }
    }
}

fn point_in_polygon(points: &[(f64, f64)], px: f64, py: f64) -> bool {
    let mut inside = false;
    let count = points.len();
    let mut j = count - 1;
    for i in 0..count {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn polygon_edge_distance(points: &[(f64, f64)], px: f64, py: f64) -> f64 {
    let mut best = f64::INFINITY;
    for pair in points.windows(2) {
        let (ax, ay) = pair[0];
        let (bx, by) = pair[1];
        let dx = bx - ax;
        let dy = by - ay;
        let length_squared = dx * dx + dy * dy;
        let t = if length_squared > 0.0 {
            (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cx = ax + dx * t;
        let cy = ay + dy * t;
        best = best.min(((px - cx).powi(2) + (py - cy).powi(2)).sqrt());
    }
    best
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
        })
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
        });
        assert_eq!(output.get_pixel(50, 50).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(10, 10).0, [255, 255, 255, 255]);
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
                magnification: 1.0,
                tilt_degrees: 0.0,
                opacity: 1.0,
                blur_radius: 0.0,
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
        });
        assert_eq!(
            output.get_pixel(center.0 + 3, center.1 + 3).0,
            [255, 255, 255, 255]
        );
    }
}
