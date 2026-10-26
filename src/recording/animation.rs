//! Project-wide animation preferences. Missing overrides retain legacy regions.
use super::viewport::{CameraMotion, ZoomCue};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnimationSettings {
    /// None preserves authored responses in older projects.
    pub screen_motion: Option<CameraMotion>,
    /// None preserves older per-region early-start flags until explicitly set.
    pub start_early: Option<bool>,
    /// Percentage of one frame interval covered by the shutter.
    pub motion_blur: u8,
}

impl AnimationSettings {
    pub fn cues(self, authored: &[ZoomCue]) -> Vec<ZoomCue> {
        authored
            .iter()
            .cloned()
            .map(|mut cue| {
                if let Some(motion) = self.screen_motion {
                    cue.camera_motion = motion;
                }
                if let Some(early) = self.start_early {
                    cue.start_animation_early = early;
                }
                cue
            })
            .collect()
    }

    pub fn blur_amount(self) -> f64 {
        f64::from(self.motion_blur.min(100)) / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_regions_survive_and_project_settings_override_every_region() {
        let mut a = ZoomCue::pinned(
            1.0,
            3.0,
            2.0,
            super::super::model::NormalizedPoint { x: 0.5, y: 0.5 },
        );
        a.start_animation_early = true;
        a.camera_motion = CameraMotion::Smooth;
        let mut b = a.clone();
        b.start_animation_early = false;
        b.camera_motion = CameraMotion::Focused;
        let authored = vec![a, b];
        let legacy: AnimationSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.cues(&authored), authored);
        let settings = AnimationSettings {
            start_early: Some(false),
            screen_motion: Some(CameraMotion::Focused),
            motion_blur: 70,
        };
        let restored: AnimationSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        let resolved = restored.cues(&authored);
        assert!(resolved
            .iter()
            .all(|cue| !cue.start_animation_early && cue.camera_motion == CameraMotion::Focused));
        assert_eq!(resolved[0].id, authored[0].id);
        assert_eq!(restored.blur_amount(), 0.7);
        assert!(
            authored[0].start_animation_early,
            "resolving a project does not overwrite its legacy source data"
        );
    }
}
