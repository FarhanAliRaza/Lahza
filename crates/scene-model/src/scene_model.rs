//! The scene model: background, media transform, window frame, camera and
//! pointer styles, and the geometry that places them on the canvas. Preview,
//! export, and the camera timeline share it; `scene` renders it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use super::pointer_timeline::PointerMotion;
pub use super::camera_layout::{CameraAspect, CameraLayout, CameraSplitDirection};
pub use super::rect::Rect;
pub use super::viewport::SceneTransform;

use super::{
    cursor_assets::CursorFamily,
    pointer_timeline::PointerTimelineOptions,
    viewport::ViewportFrame,
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
    /// Longest side as a percent of the canvas short side (10-60).
    pub size: u8,
    pub shape: CameraShape,
    pub aspect: CameraAspect,
    pub layout: CameraLayout,
    /// Optional size during screen zooms; None preserves the normal size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoomed_size: Option<u8>,
    /// Corner radius as a percent of the short side; None uses the shape preset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<u8>,
    /// Center in unmirrored webcam coordinates.
    pub crop_center: (f64, f64),
    pub crop_zoom: u16,
    pub split_direction: CameraSplitDirection,
    pub split_flip: bool,
    /// Screen share of a split layout (25-75 percent).
    pub split_ratio: u8,
    /// Fill the split cells edge to edge rather than using padded camera framing.
    pub split_full: bool,
    /// Person-background removal controls, applied before camera composition.
    pub cutout_softness: u8,
    pub cutout_inset: i8,
    /// Evaluated camera-region geometry. Never persisted as authored style.
    #[serde(skip)]
    pub pose: Option<super::camera_timeline::CameraPose>,
    pub mirror: bool,
    /// Distance from the canvas edge as a percent of its short side.
    pub margin: u8,
    pub shadow: bool,
    /// Free placement: the bubble's center as a fraction of the canvas.
    /// `None` pins it to `position`'s corner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center: Option<(f64, f64)>,
}

impl Default for CameraOverlay {
    fn default() -> Self {
        Self {
            enabled: true,
            position: WatermarkPosition::BottomRight,
            size: 24,
            shape: CameraShape::Circle,
            aspect: CameraAspect::Square,
            layout: CameraLayout::Overlay,
            zoomed_size: None,
            corner_radius: None,
            crop_center: (0.5, 0.5),
            crop_zoom: 100,
            split_direction: CameraSplitDirection::Auto,
            split_flip: false,
            split_ratio: 60,
            split_full: false,
            cutout_softness: 20,
            cutout_inset: 0,
            pose: None,
            mirror: false,
            margin: 4,
            shadow: true,
            center: None,
        }
    }
}

impl CameraOverlay {
    /// Snap distance for dragging, in canvas pixels at 1080p.
    const SNAP_DISTANCE: f64 = 8.0 / 1080.0;

    pub fn aspect_ratio(&self, source_aspect: f64) -> f64 {
        if self.layout == CameraLayout::Cutout { source_aspect }
        else if self.shape == CameraShape::Circle { 1.0 } else { self.aspect.ratio(source_aspect) }
    }

    /// The unzoomed overlay, retained for callers without a source frame.
    pub fn rect(&self, canvas_width: f64, canvas_height: f64) -> Rect {
        self.rect_for_frame(canvas_width, canvas_height, 1.0, 1.0)
    }

    pub fn rect_for_frame(&self, width: f64, height: f64, source_aspect: f64, zoom: f64) -> Rect {
        super::camera_layout::layout(*self, width, height, source_aspect, zoom).camera
    }

    pub fn screen_area(&self, width: f64, height: f64) -> Rect {
        super::camera_layout::layout(*self, width, height, 1.0, 1.0).screen
    }

    pub fn snapped_center(
        &self, proposed: (f64, f64), width: f64, height: f64,
    ) -> ((f64, f64), (Option<f64>, Option<f64>)) {
        self.snapped_center_for_frame(proposed, width, height, 1.0, 1.0)
    }

    /// Drag handles use the same dimensions and padding as the composed frame.
    pub fn snapped_center_for_frame(
        &self, proposed: (f64, f64), width: f64, height: f64, source_aspect: f64, zoom: f64,
    ) -> ((f64, f64), (Option<f64>, Option<f64>)) {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return ((0.5, 0.5), (None, None));
        }
        let rect = self.rect_for_frame(width, height, source_aspect, zoom);
        let mut corner = *self;
        corner.center = None;
        corner.position = WatermarkPosition::TopLeft;
        let inset = corner.rect_for_frame(width, height, source_aspect, zoom);
        let snap = Self::SNAP_DISTANCE * width.min(height);
        let axis = |center: f64, length: f64, size: f64, margin: f64| {
            let radius = size * 0.5;
            let low = margin + radius;
            let high = (length - margin - radius).max(low);
            let targets = [(low, margin), (length * 0.5, length * 0.5), (high, length - margin)];
            let center = if center.is_finite() { center } else { length * 0.5 };
            let nearest = targets.into_iter()
                .min_by(|a, b| (a.0 - center).abs().total_cmp(&(b.0 - center).abs()))
                .filter(|(target, _)| (target - center).abs() <= snap);
            match nearest {
                Some((target, guide)) => (target, Some(guide)),
                None => (center.clamp(low, high), None),
            }
        };
        let (x, guide_x) = axis(proposed.0, width, rect.width, inset.x);
        let (y, guide_y) = axis(proposed.1, height, rect.height, inset.y);
        (((x / width).clamp(0.0, 1.0), (y / height).clamp(0.0, 1.0)), (guide_x, guide_y))
    }

    pub fn radius(&self, rect: Rect) -> f64 {
        if let Some(pose) = self.pose { return pose.radius_fraction * rect.width.min(rect.height); }
        if self.layout == CameraLayout::Fullscreen || self.layout == CameraLayout::Cutout || (self.layout == CameraLayout::Split && self.split_full) { return 0.0; }
        let side = rect.width.min(rect.height);
        if self.shape == CameraShape::Circle { return side * 0.5; }
        if self.shape == CameraShape::Squircle {
            if let Some(radius) = self.corner_radius { return side * f64::from(radius.min(50)) / 100.0; }
        }
        side * match self.shape {
            CameraShape::Circle => 0.5,
            CameraShape::Squircle => 0.18,
            CameraShape::Square => 0.03,
        }
    }

    /// The editor no longer offers the "Screen only" layout; a saved one
    /// becomes a hidden overlay, which renders the same.
    pub fn without_hidden_layout(mut self) -> Self {
        if self.layout == CameraLayout::Hidden {
            self.layout = CameraLayout::Overlay;
            self.enabled = false;
        }
        self
    }

    pub fn opacity(&self) -> f64 {
        self.pose.map_or(if self.enabled && self.layout != CameraLayout::Hidden { 1.0 } else { 0.0 }, |pose| pose.opacity)
    }

    pub fn cutout_strength(&self) -> f64 {
        self.pose.map_or(if self.layout == CameraLayout::Cutout {1.0}else{0.0}, |pose|pose.cutout)
    }

    pub fn full_bleed_strength(&self) -> f64 {
        self.pose.map_or(if self.enabled && self.layout == CameraLayout::Split && self.split_full {1.0}else{0.0}, |pose|pose.full_bleed)
    }

    pub fn split_strength(&self) -> f64 {
        self.pose.map_or(if self.enabled && self.layout == CameraLayout::Split {1.0}else{0.0}, |pose|pose.split)
    }

    pub fn shadow_strength(&self) -> f64 {
        if !self.shadow {return 0.0;}
        self.pose.map_or_else(|| {
            if self.enabled && !matches!(self.layout,CameraLayout::Fullscreen | CameraLayout::Cutout | CameraLayout::Hidden)
                && !(self.layout==CameraLayout::Split && self.split_full) {1.0}else{0.0}
        }, |pose|pose.shadow)
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
    pub rotation: super::pointer_timeline::PointerRotation,
    pub rotation_strength: f64,
    pub hide_when_typing: bool,
    pub idle_hide_delay: f64,
    pub loop_return_duration: f64,
    /// Runtime shutter override. None retains the legacy cursor smear.
    #[serde(skip)]
    pub motion_blur_amount: Option<f64>,
}

impl PointerStyle {
    /// Seconds of stillness before an idle cursor fades out.
    pub const IDLE_HIDE_DELAY: f64 = 1.5;

    /// Cursor-track build options implied by this style.
    pub fn timeline_options(self) -> PointerTimelineOptions {
        PointerTimelineOptions {
            fallback_artwork: None,
            hide_after_inactivity: self.hide_when_idle.then_some(self.idle_hide_delay.clamp(0.5, 5.0)),
            motion: self.motion,
            loop_to_start: self.loop_to_start,
            rotation: self.rotation,
            rotation_strength: self.rotation_strength,
            hide_when_typing: self.hide_when_typing,
            loop_return_duration: self.loop_return_duration,
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
            rotation: super::pointer_timeline::PointerRotation::Velocity,
            rotation_strength: 0.5,
            hide_when_typing: true,
            idle_hide_delay: Self::IDLE_HIDE_DELAY,
            loop_return_duration: 1.0,
            motion_blur_amount: None,
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

    pub fn bar_color(self) -> Option<[u8; 3]> {
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
    pub animation: super::animation::AnimationSettings,
    #[serde(default)]
    pub source_crop: crate::viewport::CropRect,
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
    /// RGB shadow tint; old projects default to black.
    pub shadow_color: u32,
    pub border: bool,
    pub border_thickness: u8,
    pub border_color: u32,
    pub border_opacity: u8,
    /// Canvas aspect ratio (width / height). `None` follows the media.
    pub aspect: Option<f64>,
    /// Preserve the proportions typed into the custom canvas fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_dimensions: Option<(u32, u32)>,
    /// With a fixed aspect: crop the media to fill the card (a follow camera
    /// keeps the action in frame) instead of fitting it whole.
    pub fill: bool,
    /// Neural upscaling of zoomed-in screen pixels.
    pub enhance_screen: bool,
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
    /// Source-time camera layouts; empty preserves the legacy global layout.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub camera_regions: Vec<super::camera_timeline::CameraRegion>,
    /// Editor-preview easing after a camera edit; see `CameraHandoff`.
    #[serde(skip)]
    pub camera_handoff: Option<super::camera_timeline::CameraHandoff>,
    /// Keystroke captions and the speed badge.
    pub input_overlays: super::studio_overlays::InputOverlayStyle,
    /// Narration subtitle bar.
    pub subtitles: super::studio_overlays::SubtitleStyle,
}

impl Default for SceneStyle {
    fn default() -> Self {
        Self {
            animation: Default::default(),
            background: SceneBackground::default(),
            source_crop: crate::viewport::CropRect::UNIT,
            window_frame: WindowFrame::Off,
            padding: 20,
            corners: 12,
            shadow: 40,
            shadow_style: 0,
            shadow_color: 0x000000,
            border: false,
            border_thickness: 20,
            border_color: 0x3678ef,
            border_opacity: 100,
            aspect: None,
            aspect_dimensions: None,
            fill: false,
            enhance_screen: true,
            background_blur: 0,
            background_noise: 0,
            vignette: 0,
            transform: SceneTransform::IDENTITY,
            watermark: None,
            pointer: PointerStyle::default(),
            camera: CameraOverlay::default(),
            camera_regions: Vec::new(),
            camera_handoff: None,
            input_overlays: Default::default(),
            subtitles: Default::default(),
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
                    let crop = self.source_crop.validated();
                    source_width as f64 * crop.width as f64 / (source_height as f64 * crop.height as f64)
                }
            });
        let height = (canvas_height.max(2) / 2) * 2;
        let width = ((height as f64 * ratio).round() as u32).max(2);
        (width / 2 * 2, height)
    }

    /// Fill crops the media to the card; it needs a fixed canvas aspect.
    pub fn fills_card(&self) -> bool {
        self.fill && self.aspect.is_some_and(|ratio| ratio.is_finite() && ratio > 0.0)
    }

    /// Visible fraction of the cropped media per axis when Fill crops it to
    /// the card. Proportional layout makes it independent of resolution.
    pub fn fill_fraction(&self, source_width: u32, source_height: u32) -> Option<(f64, f64)> {
        if !self.fills_card() || source_width == 0 || source_height == 0 {
            return None;
        }
        let height = REFERENCE_CANVAS_HEIGHT;
        let width = height * self.aspect?;
        let card = SceneGeometry::layout(width, height, source_width as f64, source_height as f64, self).media;
        let crop = self.source_crop.validated();
        let media_aspect = source_width as f64 * crop.width as f64 / (source_height as f64 * crop.height as f64);
        let card_aspect = card.width / card.height.max(1e-6);
        Some(if media_aspect > card_aspect {
            ((card_aspect / media_aspect).clamp(0.01, 1.0), 1.0)
        } else {
            (1.0, (media_aspect / card_aspect).clamp(0.01, 1.0))
        })
    }

    /// Source-space viewport available to focus clustering before magnification.
    pub fn focus_bounds(&self, source_width: u32, source_height: u32) -> (f64, f64) {
        let crop = self.source_crop.validated();
        let fill = self.fill_fraction(source_width, source_height).unwrap_or((1.0, 1.0));
        ((f64::from(crop.width) * fill.0).clamp(0.001, 1.0),
            (f64::from(crop.height) * fill.1).clamp(0.001, 1.0))
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

/// Split rule: the screen covers its cell, cropping the
/// overflow. When the crop runs along the split axis and there is padding,
/// it may shrink by up to 10% instead, leaving a small gap over a deep crop.
fn split_cover(cell: Rect, source_width: f64, source_height: f64, side_by_side: bool, relax: bool) -> Rect {
    let fit = (cell.width / source_width).min(cell.height / source_height);
    let cover = (cell.width / source_width).max(cell.height / source_height);
    let along_split = if side_by_side {
        source_width * cover > cell.width + 0.5
    } else {
        source_height * cover > cell.height + 0.5
    };
    let scale = if relax && along_split { fit.max(cover * 0.9) } else { cover };
    let width = (source_width * scale).min(cell.width);
    let height = (source_height * scale).min(cell.height);
    Rect { x: cell.x + (cell.width - width) * 0.5, y: cell.y + (cell.height - height) * 0.5, width, height }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    pub color: [u8; 3],
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
        let crop = style.source_crop.validated();
        let source_width = source_width * crop.width as f64;
        let source_height = source_height * crop.height as f64;
        let full_bleed = style.camera.full_bleed_strength().clamp(0.0,1.0);
        let decoration = 1.0-full_bleed;
        let border_width = if style.border {
            style.border_thickness as f64 * 0.48 * ui_scale * decoration
        } else {
            0.0
        };
        // Zero padding means the media reaches the canvas edge; only the
        // border adds space beyond the user's padding setting.
        let inset = style.padding as f64 * 2.0 * ui_scale * decoration + border_width;
        let title_height = if style.window_frame == WindowFrame::Off {
            0.0
        } else {
            TITLE_BAR_HEIGHT * ui_scale * decoration
        };
        let screen_area = style.camera.screen_area(canvas_width, canvas_height);
        let available_width = (screen_area.width - inset * 2.0).max(1.0);
        let available_height = (screen_area.height - inset * 2.0 - title_height).max(1.0);
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
        let mut media = if style.fills_card() {
            Rect { x: screen_area.x + inset, y: screen_area.y + inset + title_height, width: available_width, height: available_height }
        } else {
            let scale = (available_width / source_width).min(available_height / source_height);
            let width = source_width * scale;
            let height = source_height * scale;
            Rect {
                x: screen_area.x + inset + (available_width - width) * 0.5,
                y: screen_area.y + inset + title_height + (available_height - height) * 0.5,
                width,
                height,
            }
        };
        // Split screen covers its cell rather than leaving a letterboxed
        // screen; Fill already covers the card.
        let split = style.camera.split_strength().clamp(0.0, 1.0);
        if split > 0.0 && !style.fills_card() {
            let cell = Rect { x: screen_area.x + inset, y: screen_area.y + inset + title_height,
                width: available_width, height: available_height };
            let side_by_side = screen_area.height / canvas_height.max(1.0) > screen_area.width / canvas_width.max(1.0);
            let cover = split_cover(cell, source_width, source_height, side_by_side, style.padding > 0);
            media.x += (cover.x-media.x)*split;
            media.y += (cover.y-media.y)*split;
            media.width += (cover.width-media.width)*split;
            media.height += (cover.height-media.height)*split;
        }
        if full_bleed > 0.0 {
            media.x += (screen_area.x-media.x)*full_bleed;
            media.y += (screen_area.y-media.y)*full_bleed;
            media.width += (screen_area.width-media.width)*full_bleed;
            media.height += (screen_area.height-media.height)*full_bleed;
        }
        let strength = style.shadow as f64 / 100.0;
        let (radius_scale, offset_scale, opacity_scale) = match style.shadow_style {
            0 => (1.0, 0.3, 1.0),
            1 => (1.2, 0.9, 0.85),
            2 => (1.6, 0.0, 0.7),
            _ => (0.8, 0.2, 1.1),
        };
        let shadow_radius = 85.0 * strength * radius_scale * ui_scale;
        let shadow = (style.shadow > 0 && decoration > 0.0).then(|| ShadowSpec {
            color: unpack(style.shadow_color),
            blur_radius: shadow_radius,
            offset_y: shadow_radius * offset_scale,
            opacity: ((0.08 + strength * 1.35).min(0.35) * opacity_scale).min(0.5) * decoration,
        });
        Self {
            canvas_width,
            canvas_height,
            media,
            radius: style.corners as f64 * 0.64 * ui_scale * decoration,
            border_width,
            title_height,
            shadow,
            ui_scale,
        }
    }

    /// Cheap, allocation-free full-split crop shared by motion estimates,
    /// handles, preview, and both render backends.
    pub fn camera_viewport(&self, mut viewport: ViewportFrame, style: &SceneStyle, source: (u32,u32)) -> ViewportFrame {
        let split = style.camera.split_strength() > 0.0 && !style.fills_card();
        if style.camera.full_bleed_strength() <= 0.0 && !split { return viewport; }
        let crop=style.source_crop.validated();
        let source_aspect=source.0.max(1) as f64 * crop.width as f64 /
            (source.1.max(1) as f64 * crop.height as f64);
        let aspect=self.media.width / self.media.height.max(1e-6);
        viewport.fill=if source_aspect>aspect {((aspect/source_aspect).clamp(0.01,1.0),1.0)}
            else {(1.0,(source_aspect/aspect).clamp(0.01,1.0))};
        viewport
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
    pub fn title_top(&self) -> f64 {
        if self.media.height <= 0.0 {
            return 0.0;
        }
        -self.title_height / self.media.height
    }

    /// Canvas bounds of the projected card surface including the title bar.
    pub fn surface_bounds(&self, projection: &MediaProjection) -> Rect {
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
    pub forward: [[f64; 3]; 3],
    /// Canvas → media (u, v).
    pub inverse: [[f64; 3]; 3],
    /// Projected corners: top-left, top-right, bottom-right, bottom-left.
    pub quad: [(f64, f64); 4],
    /// Canvas-space bounding box of the quad.
    pub bounds: Rect,
    /// Media size at scale 1 (canvas pixels); radii are measured in it.
    media_width: f64,
    media_height: f64,
    /// Constant media-pixels-per-canvas-pixel when the transform is affine.
    pub affine_pixel_size: Option<f64>,
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
    pub fn axis_aligned(&self) -> Option<(f64, f64, f64, f64)> {
        let m = self.inverse;
        let pure = m[0][1].abs() < 1e-12
            && m[1][0].abs() < 1e-12
            && m[2][0].abs() < 1e-12
            && m[2][1].abs() < 1e-12
            && (m[2][2] - 1.0).abs() < 1e-9;
        (pure && self.affine_pixel_size.is_some()).then_some((m[0][0], m[0][2], m[1][1], m[1][2]))
    }

    /// Media pixels (at scale 1) covered by one canvas pixel at `(x, y)`.
    pub fn pixel_size_at(&self, x: f64, y: f64) -> f64 {
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
    pub fn screen_scale_at(&self, u: f64, v: f64) -> f64 {
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

pub fn rounded_rect_distance(u: f64, v: f64, width: f64, height: f64, radius: f64) -> f64 {
    let radius = radius.clamp(0.0, width.min(height) * 0.5);
    let x = u * width;
    let y = v * height;
    let dx = (x - width * 0.5).abs() - (width * 0.5 - radius);
    let dy = (y - height * 0.5).abs() - (height * 0.5 - radius);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let inside = dx.max(dy).min(0.0);
    outside + inside - radius
}

pub fn unpack(color: u32) -> [u8; 3] {
    [
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    ]
}

/// Pointer state drawn on top of the media surface.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerOverlay {
    pub frame: super::pointer_timeline::PointerFrame,
}
