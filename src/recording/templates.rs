//! Curated scene templates: a background look, layout, 3D pose, motion, and
//! timed captions bundled into one click. Templates are the "make it look
//! like a launch video" starting points; everything they set stays editable
//! afterwards through the normal inspector, lanes, and tools.

use super::{
    scene::{SceneBackground, SceneStyle, SceneTransform},
    viewport::{MotionEasing, MotionPreset, ZoomCue},
};
use crate::{
    timed::{AnnotationTiming, EntranceEffect, ExitEffect},
    AnnotationMark, NormPoint, Tool,
};

/// Camera motion a template starts with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TemplateMotion {
    /// No motion regions: the scene holds still.
    Still,
    /// One of the built-in motion presets, optionally with its own easing.
    Preset {
        preset: MotionPreset,
        easing: Option<MotionEasing>,
    },
}

impl TemplateMotion {
    /// Motion regions covering `duration` seconds.
    pub fn cues(self, duration: f64) -> Vec<ZoomCue> {
        match self {
            TemplateMotion::Still => Vec::new(),
            TemplateMotion::Preset { preset, easing } => {
                let mut cues = preset.cues(duration);
                if let Some(easing) = easing {
                    for cue in &mut cues {
                        cue.easing = easing;
                    }
                }
                cues
            }
        }
    }

    pub fn preset(self) -> Option<MotionPreset> {
        match self {
            TemplateMotion::Still => None,
            TemplateMotion::Preset { preset, .. } => Some(preset),
        }
    }
}

/// A caption or callout a template places on the media, in normalized
/// media coordinates, with its entrance and exit already timed.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateMark {
    pub tool: Tool,
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub text: &'static str,
    pub color: u32,
    pub font_size: f32,
    pub bold: bool,
    /// 0 left, 1 center, 2 right.
    pub alignment: u8,
    pub stroke_width: f32,
    pub number: usize,
    pub timing: AnnotationTiming,
}

impl TemplateMark {
    fn caption(
        text: &'static str,
        x: f32,
        y: f32,
        width: f32,
        font_size: f32,
        color: u32,
        alignment: u8,
        start: f64,
        end: f64,
        entrance: EntranceEffect,
        exit: ExitEffect,
    ) -> Self {
        // Preview text is laid out in canvas pixels; a typical media height
        // of ~480px keeps the box tight around the glyphs.
        let height = (font_size * 1.35 / 480.0).min(0.5);
        Self {
            tool: Tool::Text,
            start: (x, y),
            end: ((x + width).min(1.0), (y + height).min(1.0)),
            text,
            color,
            font_size,
            bold: true,
            alignment,
            stroke_width: 4.0,
            number: 1,
            timing: AnnotationTiming {
                start,
                end,
                entrance,
                exit,
                transition: 0.4,
            },
        }
    }

    fn number(number: usize, x: f32, y: f32, color: u32, start: f64, end: f64) -> Self {
        let size = 0.075;
        Self {
            tool: Tool::Number,
            start: (x, y),
            end: (x + size * 9.0 / 16.0, y + size),
            text: "",
            color,
            font_size: 24.0,
            bold: false,
            alignment: 0,
            stroke_width: 4.0,
            number,
            timing: AnnotationTiming {
                start,
                end,
                entrance: EntranceEffect::Pop,
                exit: ExitEffect::Pop,
                transition: 0.3,
            },
        }
    }

    /// The editable annotation this template mark becomes in a scene that
    /// lasts `scene_duration` seconds.
    pub fn to_mark(&self, scene_duration: f64) -> AnnotationMark {
        AnnotationMark {
            tool: self.tool,
            start: NormPoint {
                x: self.start.0,
                y: self.start.1,
            },
            end: NormPoint {
                x: self.end.0,
                y: self.end.1,
            },
            points: Vec::new(),
            number: self.number,
            color: self.color,
            stroke_width: self.stroke_width,
            text: self.text.to_string(),
            font_size: self.font_size,
            bold: self.bold,
            text_alignment: self.alignment,
            timing: Some(self.timing.clamped(scene_duration)),
            from_template: true,
            // Captions overlay the frame; the camera moves beneath them.
            pinned: true,
            ..AnnotationMark::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    pub style: SceneStyle,
    /// Aspect preset index in the inspector (0 = auto).
    pub aspect_index: usize,
    /// Scene length for an animated screenshot; the intro window that gets
    /// motion and captions when applied to a longer recording.
    pub duration: f64,
    pub motion: TemplateMotion,
    pub marks: Vec<TemplateMark>,
}

impl SceneTemplate {
    /// Motion regions for a scene of `scene_duration` seconds: the whole
    /// scene for a screenshot, the template's intro window for a recording.
    pub fn cues(&self, scene_duration: f64) -> Vec<ZoomCue> {
        let window = self.duration.min(scene_duration);
        self.motion.cues(window)
    }

    pub fn marks(&self, scene_duration: f64) -> Vec<AnnotationMark> {
        self.marks
            .iter()
            .map(|mark| mark.to_mark(scene_duration))
            .collect()
    }

    /// The colours a gallery card paints for this template.
    pub fn swatch(&self) -> [u32; 3] {
        match &self.style.background {
            SceneBackground::Solid(color) => [*color; 3],
            SceneBackground::Gradient { colors, .. } => *colors,
            SceneBackground::Wallpaper(_) => [0x8a94a6, 0x5b6474, 0x2f3542],
        }
    }
}

fn gradient(title: &str) -> SceneBackground {
    let preset = crate::GRADIENT_BACKGROUNDS
        .iter()
        .find(|preset| preset.title == title)
        .unwrap_or(&crate::GRADIENT_BACKGROUNDS[0]);
    SceneBackground::Gradient {
        colors: preset.colors,
        angle_degrees: preset.angle as f64,
    }
}

fn solid(name: &str) -> SceneBackground {
    let color = crate::SOLID_BACKGROUNDS
        .iter()
        .find(|(label, _)| *label == name)
        .map(|(_, color)| *color)
        .unwrap_or(crate::SOLID_BACKGROUNDS[0].1);
    SceneBackground::Solid(color)
}

const WHITE: u32 = 0xf5f5f5;
const INK: u32 = 0x050506;
const YELLOW: u32 = 0xffd12e;
const BLUE: u32 = 0x2e7aff;
const PINK: u32 = 0xff2e6e;

/// The built-in gallery, in display order.
pub fn all() -> Vec<SceneTemplate> {
    vec![
        SceneTemplate {
            id: "product-launch",
            name: "Product launch",
            tagline: "Floating card, deep gradient, typed headline",
            style: SceneStyle {
                background: gradient("Plasma"),
                padding: 30,
                corners: 16,
                shadow: 70,
                shadow_style: 1,
                background_noise: 8,
                vignette: 28,
                transform: SceneTransform {
                    scale: 0.94,
                    rotation_x: 6.0,
                    rotation_y: -12.0,
                    perspective: 0.42,
                    ..SceneTransform::IDENTITY
                },
                ..SceneStyle::default()
            },
            aspect_index: 4,
            duration: 7.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::FloatingCard,
                easing: Some(MotionEasing::Cinematic),
            },
            marks: vec![
                TemplateMark::caption(
                    "Introducing Screendrop",
                    0.08,
                    0.07,
                    0.84,
                    44.0,
                    WHITE,
                    1,
                    0.4,
                    4.2,
                    EntranceEffect::Type,
                    ExitEffect::Fade,
                ),
                TemplateMark::caption(
                    "The motion editor for Linux",
                    0.08,
                    0.2,
                    0.84,
                    24.0,
                    YELLOW,
                    1,
                    1.2,
                    4.2,
                    EntranceEffect::SlideUp,
                    ExitEffect::Fade,
                ),
            ],
        },
        SceneTemplate {
            id: "feature-spotlight",
            name: "Feature spotlight",
            tagline: "Push in on the centre with a bold callout",
            style: SceneStyle {
                background: gradient("Cobalt"),
                padding: 22,
                corners: 12,
                shadow: 55,
                shadow_style: 0,
                vignette: 15,
                ..SceneStyle::default()
            },
            aspect_index: 4,
            duration: 5.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::FocusCenter,
                easing: Some(MotionEasing::Smooth),
            },
            marks: vec![TemplateMark::caption(
                "NEW  One-click motion presets",
                0.05,
                0.82,
                0.9,
                30.0,
                YELLOW,
                0,
                0.5,
                4.6,
                EntranceEffect::Pop,
                ExitEffect::Pop,
            )],
        },
        SceneTemplate {
            id: "tutorial",
            name: "Tutorial steps",
            tagline: "Light look, numbered steps that slide in",
            style: SceneStyle {
                background: gradient("Mist"),
                padding: 14,
                corners: 10,
                shadow: 35,
                shadow_style: 0,
                ..SceneStyle::default()
            },
            aspect_index: 0,
            duration: 8.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::Sweep,
                easing: Some(MotionEasing::Smooth),
            },
            marks: vec![
                TemplateMark::number(1, 0.04, 0.06, BLUE, 0.3, 3.9),
                TemplateMark::caption(
                    "Open the editor",
                    0.11,
                    0.065,
                    0.6,
                    28.0,
                    INK,
                    0,
                    0.5,
                    3.9,
                    EntranceEffect::SlideLeft,
                    ExitEffect::SlideDown,
                ),
                TemplateMark::number(2, 0.04, 0.06, BLUE, 4.0, 7.8),
                TemplateMark::caption(
                    "Pick a template",
                    0.11,
                    0.065,
                    0.6,
                    28.0,
                    INK,
                    0,
                    4.2,
                    7.8,
                    EntranceEffect::SlideLeft,
                    ExitEffect::Fade,
                ),
            ],
        },
        SceneTemplate {
            id: "social-square",
            name: "Social square",
            tagline: "1:1 with glow and a punchy hook",
            style: SceneStyle {
                background: gradient("Aurora"),
                padding: 30,
                corners: 20,
                shadow: 60,
                shadow_style: 2,
                ..SceneStyle::default()
            },
            aspect_index: 1,
            duration: 5.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::PanRight,
                easing: Some(MotionEasing::Smooth),
            },
            marks: vec![
                TemplateMark::caption(
                    "Wait for it",
                    0.1,
                    0.4,
                    0.8,
                    40.0,
                    WHITE,
                    1,
                    0.3,
                    2.3,
                    EntranceEffect::Pop,
                    ExitEffect::Pop,
                ),
                TemplateMark::caption(
                    "Made with Screendrop",
                    0.1,
                    0.86,
                    0.8,
                    20.0,
                    WHITE,
                    1,
                    2.6,
                    5.0,
                    EntranceEffect::Fade,
                    ExitEffect::None,
                ),
            ],
        },
        SceneTemplate {
            id: "changelog",
            name: "Changelog",
            tagline: "Dark, bordered, a slow pan and a version line",
            style: SceneStyle {
                background: solid("Graphite"),
                padding: 18,
                corners: 12,
                shadow: 30,
                shadow_style: 3,
                border: true,
                border_thickness: 12,
                border_color: 0x3678ef,
                border_opacity: 100,
                ..SceneStyle::default()
            },
            aspect_index: 4,
            duration: 5.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::PanRight,
                easing: Some(MotionEasing::Linear),
            },
            marks: vec![TemplateMark::caption(
                "What's new in 2.0",
                0.05,
                0.06,
                0.7,
                34.0,
                WHITE,
                0,
                0.3,
                4.6,
                EntranceEffect::SlideLeft,
                ExitEffect::Fade,
            )],
        },
        SceneTemplate {
            id: "cinematic",
            name: "Cinematic",
            tagline: "Blurred ember backdrop, grain, slow push",
            style: SceneStyle {
                background: gradient("Ember"),
                padding: 24,
                corners: 14,
                shadow: 75,
                shadow_style: 0,
                background_blur: 45,
                background_noise: 18,
                vignette: 60,
                ..SceneStyle::default()
            },
            aspect_index: 4,
            duration: 8.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::SlowZoomIn,
                easing: Some(MotionEasing::Cinematic),
            },
            marks: vec![TemplateMark::caption(
                "Built for makers",
                0.1,
                0.42,
                0.8,
                46.0,
                WHITE,
                1,
                1.0,
                4.8,
                EntranceEffect::Fade,
                ExitEffect::Fade,
            )],
        },
        SceneTemplate {
            id: "minimal-dark",
            name: "Minimal dark",
            tagline: "Black, tight padding, gentle zoom out",
            style: SceneStyle {
                background: solid("Black"),
                padding: 8,
                corners: 6,
                shadow: 0,
                shadow_style: 0,
                ..SceneStyle::default()
            },
            aspect_index: 0,
            duration: 5.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::SlowZoomOut,
                easing: Some(MotionEasing::Smooth),
            },
            marks: Vec::new(),
        },
        SceneTemplate {
            id: "store-listing",
            name: "Store listing",
            tagline: "4:3 tilted card with an availability line",
            style: SceneStyle {
                background: gradient("Mineral"),
                padding: 26,
                corners: 18,
                shadow: 45,
                shadow_style: 1,
                transform: SceneTransform {
                    rotation_y: 16.0,
                    perspective: 0.4,
                    ..SceneTransform::IDENTITY
                },
                ..SceneStyle::default()
            },
            aspect_index: 2,
            duration: 6.0,
            motion: TemplateMotion::Preset {
                preset: MotionPreset::Tilt3D,
                easing: Some(MotionEasing::Smooth),
            },
            marks: vec![TemplateMark::caption(
                "Available now on Flathub",
                0.1,
                0.84,
                0.8,
                30.0,
                PINK,
                1,
                0.6,
                5.6,
                EntranceEffect::Pop,
                ExitEffect::Fade,
            )],
        },
    ]
}

/// Looks a template up by its stable id.
pub fn find(id: &str) -> Option<SceneTemplate> {
    all().into_iter().find(|template| template.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_have_unique_ids_and_valid_scenes() {
        let templates = all();
        let mut ids: Vec<_> = templates.iter().map(|template| template.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), templates.len(), "template ids collide");
        for template in &templates {
            assert!(template.duration >= 3.0, "{} is too short", template.id);
            assert_eq!(
                template.style.transform,
                template.style.transform.clamped(),
                "{} transform is out of range",
                template.id
            );
            assert!(
                matches!(
                    template.style.background,
                    SceneBackground::Solid(_) | SceneBackground::Gradient { .. }
                ),
                "{} must use a bundled background",
                template.id
            );
            for mark in template.marks(template.duration) {
                let timing = mark.timing.expect("template marks are timed");
                assert!(timing.start >= 0.0 && timing.end <= template.duration + 1e-9);
                assert!(timing.duration() >= AnnotationTiming::MINIMUM_DURATION);
                assert!(mark.from_template);
                assert!(mark.pinned);
                assert!(mark.start.x >= 0.0 && mark.end.x <= 1.0);
                assert!(mark.start.y >= 0.0 && mark.end.y <= 1.0);
                if mark.tool == Tool::Text {
                    assert!(!mark.text.is_empty());
                }
            }
            for cue in template.cues(template.duration) {
                assert!(cue.start >= 0.0 && cue.end <= template.duration + 1e-9);
            }
        }
    }

    #[test]
    fn recording_gets_the_intro_window_only() {
        let template = find("product-launch").unwrap();
        let cues = template.cues(30.0);
        assert!(!cues.is_empty());
        assert!(cues.iter().all(|cue| cue.end <= template.duration + 1e-9));
        assert!(cues.iter().all(|cue| cue.easing == MotionEasing::Cinematic));
        // A scene shorter than the template clamps captions to fit.
        let marks = template.marks(2.0);
        assert!(marks
            .iter()
            .all(|mark| mark.timing.unwrap().end <= 2.0 + 1e-9));
    }

    #[test]
    fn still_template_has_no_motion() {
        assert!(TemplateMotion::Still.cues(5.0).is_empty());
        assert_eq!(TemplateMotion::Still.preset(), None);
    }
}
