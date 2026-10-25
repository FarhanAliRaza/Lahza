//! Shared webcam layout and source cropping for preview, export, and handles.
use serde::{Deserialize, Serialize};

use super::scene::{CameraOverlay, Rect, WatermarkPosition};
#[cfg(test)]
use super::scene::CameraShape;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraAspect {
    #[default]
    Square,
    Source,
    Wide,
    Portrait,
}

impl CameraAspect {
    pub const ALL: [Self; 4] = [Self::Square, Self::Source, Self::Wide, Self::Portrait];

    pub fn label(self) -> &'static str {
        match self {
            Self::Square => "Square",
            Self::Source => "Original",
            Self::Wide => "Wide",
            Self::Portrait => "Portrait",
        }
    }

    pub fn ratio(self, source_aspect: f64) -> f64 {
        match self {
            Self::Square => 1.0,
            Self::Source => valid_aspect(source_aspect),
            Self::Wide => 16.0 / 9.0,
            Self::Portrait => 9.0 / 16.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraLayout {
    #[default]
    Overlay,
    Split,
    Fullscreen,
}

impl CameraLayout {
    pub const ALL: [Self; 3] = [Self::Overlay, Self::Split, Self::Fullscreen];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overlay => "Overlay",
            Self::Split => "Split screen",
            Self::Fullscreen => "Camera only",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraSplitDirection {
    #[default]
    Auto,
    Horizontal,
    Vertical,
}

impl CameraSplitDirection {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Horizontal, Self::Vertical];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Horizontal => "Horizontal",
            Self::Vertical => "Vertical",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraGeometry {
    pub screen: Rect,
    pub camera: Rect,
}

pub fn layout(overlay: CameraOverlay, width: f64, height: f64, source_aspect: f64, zoom: f64) -> CameraGeometry {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return CameraGeometry { screen: Rect::default(), camera: Rect::default() };
    }
    let canvas = Rect { x: 0.0, y: 0.0, width, height };
    let aspect = overlay.aspect_ratio(source_aspect);
    let shortest = width.min(height);
    let padding = shortest * f64::from(overlay.margin.min(20)) / 100.0;
    if overlay.enabled && overlay.layout == CameraLayout::Fullscreen {
        return CameraGeometry { screen: canvas, camera: canvas };
    }
    if overlay.enabled && overlay.layout == CameraLayout::Split {
        let horizontal = match overlay.split_direction {
            CameraSplitDirection::Auto => width >= height,
            CameraSplitDirection::Horizontal => true,
            CameraSplitDirection::Vertical => false,
        };
        let share = f64::from(overlay.split_ratio.clamp(25, 75)) / 100.0;
        let length = if horizontal { width } else { height };
        let gap = padding.min(length * 0.1);
        let screen_length = (length - gap) * share;
        let camera_length = length - gap - screen_length;
        let (screen_start, camera_start) = if overlay.split_flip {
            (camera_length + gap, 0.0)
        } else {
            (0.0, screen_length + gap)
        };
        let rect = |start, length| if horizontal {
            Rect { x: start, width: length, ..canvas }
        } else {
            Rect { y: start, height: length, ..canvas }
        };
        return CameraGeometry {
            screen: rect(screen_start, screen_length),
            camera: fit_rect(inset(rect(camera_start, camera_length), padding), aspect),
        };
    }
    let progress = if zoom.is_finite() { ((zoom - 1.0) / 0.5).clamp(0.0, 1.0) } else { 0.0 };
    let progress = progress * progress * (3.0 - 2.0 * progress);
    let normal = f64::from(overlay.size.clamp(10, 60));
    let zoomed = f64::from(overlay.zoomed_size.unwrap_or(overlay.size).clamp(10, 60));
    let side = shortest * (normal + (zoomed - normal) * progress) / 100.0;
    let padding = if overlay.zoomed_size.is_some() { padding * (1.0 - progress * 0.5) } else { padding };
    let area = inset(canvas, padding);
    let size = fit_rect(Rect { x: 0.0, y: 0.0, width: side.min(area.width), height: side.min(area.height) }, aspect);
    let align = match overlay.position {
        WatermarkPosition::TopLeft => (0.0, 0.0),
        WatermarkPosition::TopRight => (1.0, 0.0),
        WatermarkPosition::BottomLeft => (0.0, 1.0),
        WatermarkPosition::BottomRight => (1.0, 1.0),
    };
    let camera = if let Some((x, y)) = overlay.center.filter(|(x, y)| x.is_finite() && y.is_finite()) {
        Rect {
            x: (x * width - size.width * 0.5).clamp(area.x, area.right() - size.width),
            y: (y * height - size.height * 0.5).clamp(area.y, area.bottom() - size.height),
            ..size
        }
    } else {
        Rect {
            x: area.x + (area.width - size.width) * align.0,
            y: area.y + (area.height - size.height) * align.1,
            ..size
        }
    };
    CameraGeometry { screen: canvas, camera }
}

pub(crate) fn fit_rect(area: Rect, aspect: f64) -> Rect {
    let aspect = valid_aspect(aspect);
    let width = area.width.min(area.height * aspect).max(0.0);
    let height = width / aspect;
    Rect { x: area.x + (area.width - width) * 0.5, y: area.y + (area.height - height) * 0.5, width, height }
}

fn inset(rect: Rect, padding: f64) -> Rect {
    let padding = padding.max(0.0).min(rect.width.min(rect.height) * 0.49);
    Rect { x: rect.x + padding, y: rect.y + padding, width: rect.width - 2.0 * padding, height: rect.height - 2.0 * padding }
}

fn valid_aspect(aspect: f64) -> f64 {
    if aspect.is_finite() && aspect > 0.0 { aspect.clamp(0.1, 10.0) } else { 1.0 }
}

/// Normalized crop in the unmirrored source. Mirroring happens within this
/// crop, so a chosen face remains framed when the mirror switch changes.
pub fn source_crop(overlay: CameraOverlay, source_aspect: f64, rect: Rect) -> Rect {
    let source_aspect = valid_aspect(source_aspect);
    let output_aspect = valid_aspect(rect.width / rect.height);
    let zoom = f64::from(overlay.crop_zoom.clamp(100, 300)) / 100.0;
    let width = (output_aspect / source_aspect).min(1.0) / zoom;
    let height = (source_aspect / output_aspect).min(1.0) / zoom;
    let coordinate = |value: f64| if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.5 };
    Rect {
        x: (coordinate(overlay.crop_center.0) - width * 0.5).clamp(0.0, 1.0 - width),
        y: (coordinate(overlay.crop_center.1) - height * 0.5).clamp(0.0, 1.0 - height),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_aspect_and_corner_stays_inside_landscape_portrait_and_square() {
        for (width, height) in [(1920.0, 1080.0), (1080.0, 1920.0), (1080.0, 1080.0), (80.0, 240.0)] {
            for aspect in CameraAspect::ALL {
                for position in [WatermarkPosition::TopLeft, WatermarkPosition::TopRight, WatermarkPosition::BottomLeft, WatermarkPosition::BottomRight] {
                    let overlay = CameraOverlay { aspect, position, shape: CameraShape::Squircle, size: 60, margin: 20, zoomed_size: Some(10), ..Default::default() };
                    for zoom in [1.0, 1.1, 1.25, 1.5, 4.0] {
                        let rect = layout(overlay, width, height, 4.0 / 3.0, zoom).camera;
                        assert!(rect.x >= 0.0 && rect.y >= 0.0 && rect.right() <= width + 1e-9 && rect.bottom() <= height + 1e-9);
                        assert!((rect.width / rect.height - aspect.ratio(4.0 / 3.0)).abs() < 1e-9);
                    }
                }
            }
        }
    }

    #[test]
    fn camera_size_tracks_short_side_and_zoom_preserves_the_corner() {
        let overlay = CameraOverlay { zoomed_size: Some(16), ..Default::default() };
        let landscape = layout(overlay, 1920.0, 1080.0, 1.0, 1.0).camera;
        let portrait = layout(overlay, 1080.0, 1920.0, 1.0, 1.0).camera;
        assert_eq!(landscape.width, portrait.width);
        let zoomed = layout(overlay, 1920.0, 1080.0, 1.0, 1.5).camera;
        assert!(zoomed.width < landscape.width);
        assert!((1920.0 - zoomed.right() - (1920.0 - landscape.right()) * 0.5).abs() < 1e-9);
        let middle = layout(overlay, 1920.0, 1080.0, 1.0, 1.25).camera;
        assert!((middle.width - (landscape.width + zoomed.width) * 0.5).abs() < 1e-9);
    }

    #[test]
    fn split_layout_reflows_and_flip_swaps_the_screen_side() {
        let mut overlay = CameraOverlay { layout: CameraLayout::Split, ..Default::default() };
        let wide = layout(overlay, 1920.0, 1080.0, 1.0, 1.0);
        assert!(wide.screen.right() <= wide.camera.x);
        let tall = layout(overlay, 1080.0, 1920.0, 1.0, 1.0);
        assert!(tall.screen.bottom() <= tall.camera.y);
        overlay.split_flip = true;
        let flipped = layout(overlay, 1920.0, 1080.0, 1.0, 1.0);
        assert!(flipped.camera.right() <= flipped.screen.x);
        overlay.layout = CameraLayout::Fullscreen;
        assert_eq!(layout(overlay, 1080.0, 1920.0, 1.0, 1.0).camera, Rect { x: 0.0, y: 0.0, width: 1080.0, height: 1920.0 });
    }

    #[test]
    fn crop_center_zoom_and_mirror_keep_the_same_selected_content() {
        let mut overlay = CameraOverlay { crop_center: (1.0, 0.0), crop_zoom: 200, ..Default::default() };
        let rect = Rect { x: 0.0, y: 0.0, width: 200.0, height: 200.0 };
        let crop = source_crop(overlay, 16.0 / 9.0, rect);
        assert_eq!(crop.right(), 1.0);
        assert_eq!(crop.y, 0.0);
        assert_eq!(crop.height, 0.5);
        overlay.mirror = true;
        assert_eq!(source_crop(overlay, 16.0 / 9.0, rect), crop);
    }

    #[test]
    fn changing_shape_uses_its_own_mask_without_losing_custom_roundness() {
        let rect = Rect { x: 0.0, y: 0.0, width: 200.0, height: 100.0 };
        let mut overlay = CameraOverlay { shape: CameraShape::Squircle, corner_radius: Some(35), ..Default::default() };
        assert_eq!(overlay.radius(rect), 35.0);
        overlay.shape = CameraShape::Square;
        assert_eq!(overlay.radius(rect), 3.0);
        overlay.shape = CameraShape::Circle;
        assert_eq!(overlay.radius(rect), 50.0);
        overlay.shape = CameraShape::Squircle;
        assert_eq!(overlay.radius(rect), 35.0);
    }

    #[test]
    fn legacy_camera_settings_keep_static_square_defaults_and_new_fields_round_trip() {
        let old: CameraOverlay = serde_json::from_str(r#"{"size":32,"mirror":true}"#).unwrap();
        assert_eq!(old.layout, CameraLayout::Overlay);
        assert_eq!(old.aspect, CameraAspect::Square);
        assert_eq!(old.zoomed_size, None);
        assert_eq!(old.rect(1920.0, 1080.0), old.rect_for_frame(1920.0, 1080.0, 16.0 / 9.0, 3.0));
        let edited = CameraOverlay { aspect: CameraAspect::Portrait, layout: CameraLayout::Split, zoomed_size: Some(16), crop_center: (0.3, 0.7), crop_zoom: 150, ..old };
        assert_eq!(serde_json::from_str::<CameraOverlay>(&serde_json::to_string(&edited).unwrap()).unwrap(), edited);
    }
}
