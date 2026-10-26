//! Authored camera layouts in source time, evaluated independently of seeking.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    camera_layout::{self, CameraLayout},
    scene::{CameraOverlay, Rect, SceneStyle},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CameraRegion {
    pub id: Uuid,
    pub start: f64,
    pub end: f64,
    pub overlay: CameraOverlay,
    /// Seconds of source time. Zero produces an exact cut.
    pub transition: f64,
    pub enabled: bool,
}

impl Default for CameraRegion {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            start: 0.0,
            end: 3.0,
            overlay: CameraOverlay::default(),
            transition: 0.35,
            enabled: true,
        }
    }
}

impl CameraRegion {
    pub const MINIMUM_DURATION: f64 = 0.1;

    pub fn valid(&self) -> bool {
        self.enabled
            && self.start.is_finite()
            && self.end.is_finite()
            && self.start >= 0.0
            && self.end - self.start >= Self::MINIMUM_DURATION
    }

    fn transition_duration(&self) -> f64 {
        if self.transition.is_finite() {
            self.transition
                .clamp(0.0, 2.0)
                .min((self.end - self.start) * 0.5)
        } else {
            0.0
        }
    }
}

/// Runtime geometry is normalized to the canvas so HiDPI/export scaling does
/// not change transition shape. Authored values never contain this structure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub screen: Rect,
    pub camera: Rect,
    pub radius_fraction: f64,
    pub opacity: f64,
    pub cutout: f64,
    pub full_bleed: f64,
    pub shadow: f64,
}

fn pose(mut overlay: CameraOverlay, canvas: (u32, u32), aspect: f64, zoom: f64) -> CameraPose {
    if let Some(pose) = overlay.pose {
        return pose;
    }
    overlay.pose = None;
    let (width, height) = (canvas.0.max(1) as f64, canvas.1.max(1) as f64);
    // Hidden layouts retain their overlay framing as the fade destination.
    let opacity = overlay.opacity();
    let shadow = overlay.shadow_strength();
    if overlay.layout == CameraLayout::Hidden {
        overlay.layout = CameraLayout::Overlay;
    }
    let geometry = camera_layout::layout(overlay, width, height, aspect, zoom);
    let normalize = |r: Rect| Rect {
        x: r.x / width,
        y: r.y / height,
        width: r.width / width,
        height: r.height / height,
    };
    CameraPose {
        screen: normalize(geometry.screen),
        camera: normalize(geometry.camera),
        radius_fraction: overlay.radius(geometry.camera)
            / geometry.camera.width.min(geometry.camera.height).max(1e-6),
        opacity,
        cutout: if overlay.layout == CameraLayout::Cutout {
            1.0
        } else {
            0.0
        },
        full_bleed: overlay.full_bleed_strength(),
        shadow,
    }
}

fn blend(
    from: CameraOverlay,
    mut to: CameraOverlay,
    progress: f64,
    canvas: (u32, u32),
    aspect: f64,
    zoom: f64,
) -> CameraOverlay {
    let t = progress.clamp(0.0, 1.0);
    // Quintic smoothstep has zero velocity and acceleration at both ends.
    let t = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let mut a = pose(from, canvas, aspect, zoom);
    let mut b = pose(to, canvas, aspect, zoom);
    // A hidden endpoint inherits the visible frame's shadow. Visibility alone
    // fades that shadow, without inventing one for a cutout/full-bleed layout.
    if a.opacity == 0.0 {
        a.shadow = b.shadow;
    }
    if b.opacity == 0.0 {
        b.shadow = a.shadow;
    }
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    let rect = |a: Rect, b: Rect| Rect {
        x: lerp(a.x, b.x),
        y: lerp(a.y, b.y),
        width: lerp(a.width, b.width),
        height: lerp(a.height, b.height),
    };
    to.pose = Some(CameraPose {
        screen: rect(a.screen, b.screen),
        camera: rect(a.camera, b.camera),
        radius_fraction: lerp(a.radius_fraction, b.radius_fraction),
        opacity: lerp(a.opacity, b.opacity),
        cutout: lerp(a.cutout, b.cutout),
        full_bleed: lerp(a.full_bleed, b.full_bleed),
        shadow: lerp(a.shadow, b.shadow),
    });
    to.enabled = a.opacity > 0.0 || b.opacity > 0.0;
    to.shadow = a.shadow > 0.0 || b.shadow > 0.0;
    to.crop_center = (
        lerp(from.crop_center.0, to.crop_center.0),
        lerp(from.crop_center.1, to.crop_center.1),
    );
    to.crop_zoom = lerp(from.crop_zoom as f64, to.crop_zoom as f64).round() as u16;
    to
}

impl SceneStyle {
    /// Resolve a camera layout at original recording time. Callers map their
    /// editor time through RecordingClipTimeline first, exactly like zoom cues.
    pub fn camera_at(
        &self,
        source_time: f64,
        canvas: (u32, u32),
        camera_aspect: f64,
        zoom: f64,
    ) -> CameraOverlay {
        self.camera_before(source_time, canvas, camera_aspect, zoom, f64::INFINITY)
    }

    fn camera_before(
        &self,
        source_time: f64,
        canvas: (u32, u32),
        camera_aspect: f64,
        zoom: f64,
        start_limit: f64,
    ) -> CameraOverlay {
        let mut base = self.camera;
        base.pose = None;
        if !source_time.is_finite() {
            return base;
        }
        let active = self
            .camera_regions
            .iter()
            .filter(|r| {
                r.valid() && r.start < start_limit && source_time >= r.start && source_time < r.end
            })
            .max_by(|a, b| a.start.total_cmp(&b.start).then(a.id.cmp(&b.id)));
        if let Some(region) = active {
            let mut target = region.overlay;
            target.pose = None;
            let duration = region.transition_duration();
            if duration > 0.0 && source_time < region.start + duration {
                // Start at the actual outgoing pose, including an unfinished
                // transition. Strictly earlier start times prevent cycles.
                let previous =
                    self.camera_before(region.start, canvas, camera_aspect, zoom, region.start);
                target = blend(
                    previous,
                    target,
                    (source_time - region.start) / duration,
                    canvas,
                    camera_aspect,
                    zoom,
                );
            }
            // Imported ranges may overlap (the editor prevents authoring
            // overlaps). When an inner range ends, fade back to the outer
            // range's current pose instead of jumping to its final layout.
            if let Some(previous) = self
                .camera_regions
                .iter()
                .filter(|r| {
                    r.valid()
                        && r.start > region.start
                        && r.start < start_limit
                        && r.end <= source_time
                        && source_time < r.end + r.transition_duration()
                })
                .max_by(|a, b| a.end.total_cmp(&b.end))
            {
                let duration = previous.transition_duration();
                if duration > 0.0 {
                    return blend(
                        previous.overlay,
                        target,
                        (source_time - previous.end) / duration,
                        canvas,
                        camera_aspect,
                        zoom,
                    );
                }
            }
            return target;
        }
        // An authored region returns smoothly to the project layout. Choose
        // the latest ended range so older ranges cannot leave a held pose.
        if let Some(previous) = self
            .camera_regions
            .iter()
            .filter(|r| r.valid() && r.start < start_limit && r.end <= source_time)
            .max_by(|a, b| a.end.total_cmp(&b.end))
        {
            let duration = previous.transition_duration();
            if duration > 0.0 && source_time < previous.end + duration {
                return blend(
                    previous.overlay,
                    base,
                    (source_time - previous.end) / duration,
                    canvas,
                    camera_aspect,
                    zoom,
                );
            }
        }
        base
    }

    pub fn has_camera_content(&self) -> bool {
        (self.camera.enabled && self.camera.layout != CameraLayout::Hidden)
            || self
                .camera_regions
                .iter()
                .any(|r| r.valid() && r.overlay.enabled && r.overlay.layout != CameraLayout::Hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        camera_layout::CameraAspect,
        clips::{RecordingClipSegment, RecordingClipTimeline},
    };
    use super::*;

    fn style() -> SceneStyle {
        SceneStyle {
            camera_regions: vec![CameraRegion {
                start: 1.0,
                end: 3.0,
                overlay: CameraOverlay {
                    layout: CameraLayout::Fullscreen,
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn transitions_are_continuous_seekable_and_resolution_independent() {
        let style = style();
        let start = style.camera_at(1.0, (1920, 1080), 16.0 / 9.0, 1.0);
        assert_eq!(
            start.pose.unwrap().camera,
            pose(style.camera, (1920, 1080), 16.0 / 9.0, 1.0).camera
        );
        let a = style.camera_at(1.175, (1920, 1080), 16.0 / 9.0, 1.0);
        let b = style.camera_at(1.175, (3840, 2160), 16.0 / 9.0, 1.0);
        assert_eq!(a, b);
        assert!(a.pose.unwrap().camera.width > start.pose.unwrap().camera.width);
        assert_eq!(
            style.camera_at(2.0, (1920, 1080), 16.0 / 9.0, 1.0).layout,
            CameraLayout::Fullscreen
        );
        assert_eq!(
            style.camera_at(4.0, (1920, 1080), 16.0 / 9.0, 1.0),
            style.camera
        );
        assert_eq!(style.camera_at(1.175, (1920, 1080), 16.0 / 9.0, 1.0), a);
    }

    #[test]
    fn hidden_and_adjacent_layouts_fade_without_a_global_camera_jump() {
        let mut style = style();
        style.camera_regions.push(CameraRegion {
            start: 3.0,
            end: 4.0,
            overlay: CameraOverlay {
                layout: CameraLayout::Hidden,
                ..Default::default()
            },
            ..Default::default()
        });
        let boundary = style.camera_at(3.0, (1920, 1080), 1.0, 1.0);
        assert_eq!(boundary.opacity(), 1.0);
        assert_eq!(boundary.pose.unwrap().camera.width, 1.0);
        let middle = style.camera_at(3.175, (1920, 1080), 1.0, 1.0);
        assert!((middle.opacity() - 0.5).abs() < 1e-9);
        assert_eq!(style.camera_at(3.5, (1920, 1080), 1.0, 1.0).opacity(), 0.0);
    }

    #[test]
    fn short_gaps_resume_from_the_outgoing_pose_without_a_jump() {
        let mut style = style();
        style.camera_regions.push(CameraRegion {
            start: 3.1,
            end: 4.0,
            overlay: CameraOverlay {
                layout: CameraLayout::Split,
                ..Default::default()
            },
            ..Default::default()
        });
        let before = style
            .camera_at(3.1 - 1e-7, (1920, 1080), 1.0, 1.0)
            .pose
            .unwrap();
        let after = style.camera_at(3.1, (1920, 1080), 1.0, 1.0).pose.unwrap();
        assert!((before.camera.width - after.camera.width).abs() < 1e-5);
        assert!((before.screen.width - after.screen.width).abs() < 1e-5);
    }

    #[test]
    fn imported_overlap_enters_from_partial_transition_and_returns_to_outer() {
        let mut style = style();
        style.camera_regions[0].end = 5.0;
        style.camera_regions.push(CameraRegion {
            start: 1.1,
            end: 2.0,
            overlay: CameraOverlay {
                layout: CameraLayout::Split,
                ..Default::default()
            },
            ..Default::default()
        });
        for time in [1.1, 2.0] {
            let before = pose(
                style.camera_at(time - 1e-7, (1920, 1080), 1.0, 1.0),
                (1920, 1080),
                1.0,
                1.0,
            );
            let after = pose(
                style.camera_at(time, (1920, 1080), 1.0, 1.0),
                (1920, 1080),
                1.0,
                1.0,
            );
            assert!((before.camera.width - after.camera.width).abs() < 1e-5);
            assert!((before.screen.width - after.screen.width).abs() < 1e-5);
        }
        assert_eq!(
            style.camera_at(2.4, (1920, 1080), 1.0, 1.0).layout,
            CameraLayout::Fullscreen
        );
    }

    #[test]
    fn shadow_fades_only_when_an_endpoint_has_a_floating_camera() {
        let cutout = CameraOverlay {
            layout: CameraLayout::Cutout,
            ..Default::default()
        };
        let split = CameraOverlay {
            layout: CameraLayout::Split,
            split_full: true,
            ..Default::default()
        };
        let floating = CameraOverlay::default();
        let hidden = CameraOverlay {
            layout: CameraLayout::Hidden,
            ..Default::default()
        };
        for progress in [0.0, 0.1, 0.5, 0.9, 1.0] {
            assert_eq!(
                blend(cutout, split, progress, (1600, 900), 1.0, 1.0).shadow_strength(),
                0.0
            );
            assert_eq!(
                blend(split, cutout, progress, (1600, 900), 1.0, 1.0).shadow_strength(),
                0.0
            );
            assert_eq!(
                blend(cutout, hidden, progress, (1600, 900), 1.0, 1.0).shadow_strength(),
                0.0
            );
        }
        assert!(
            (blend(floating, split, 0.5, (1600, 900), 1.0, 1.0).shadow_strength() - 0.5).abs()
                < 1e-9
        );
        assert_eq!(
            blend(floating, split, 1.0, (1600, 900), 1.0, 1.0).shadow_strength(),
            0.0
        );
        assert_eq!(
            blend(split, floating, 1.0, (1600, 900), 1.0, 1.0).shadow_strength(),
            1.0
        );
        let fading = blend(floating, hidden, 0.5, (1600, 900), 1.0, 1.0);
        assert_eq!(fading.shadow_strength(), 1.0);
        assert_eq!(fading.opacity(), 0.5);
    }

    #[test]
    fn source_regions_survive_cuts_speeds_repetition_and_serialization() {
        let style = style();
        let mut clip = RecordingClipSegment::new(1.5, 2.5);
        clip.speed = 2.0;
        let clips = RecordingClipTimeline::new(vec![clip.clone(), clip]);
        for time in [0.1, 0.6] {
            let source = clips.content_source_time_at(time).unwrap();
            assert_eq!(
                style.camera_at(source, (1000, 1000), 1.0, 1.0).layout,
                CameraLayout::Fullscreen
            );
        }
        let decoded: SceneStyle =
            serde_json::from_str(&serde_json::to_string(&style).unwrap()).unwrap();
        assert_eq!(decoded, style);
        let old: CameraOverlay = serde_json::from_str("{}").unwrap();
        assert_eq!(old, CameraOverlay::default());
        assert_eq!(CameraAspect::Classic.ratio(1.0), 4.0 / 3.0);
        assert_eq!(CameraAspect::Tall.ratio(1.0), 3.0 / 4.0);
    }
}
