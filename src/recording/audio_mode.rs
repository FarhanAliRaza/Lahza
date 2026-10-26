//! Microphone processing is distinct from recording/system-audio mixing.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioMode {
    #[default]
    #[serde(alias = "channel-fix")]
    Off,
    #[serde(alias = "voice", alias = "normalized")]
    Standard,
    Loud,
    #[serde(alias = "noiseReduction")]
    NoiseReduction,
    Podcast,
}

impl AudioMode {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Standard,
        Self::Loud,
        Self::NoiseReduction,
        Self::Podcast,
    ];
    pub fn from_legacy(reduce_noise: bool) -> Self {
        if reduce_noise {
            Self::NoiseReduction
        } else {
            Self::Off
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Standard => "Standard",
            Self::Loud => "Loud",
            Self::NoiseReduction => "Noise reduction",
            Self::Podcast => "Podcast",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Off => "Keep the microphone level and repair a quiet stereo channel.",
            Self::Standard => "Balance the recording level and repair a quiet stereo channel.",
            Self::Loud => "Keep speech at a consistent level and soften long pauses.",
            Self::NoiseReduction => {
                "Remove background noise locally, then balance the voice level."
            }
            Self::Podcast => {
                "Enhance the voice locally, shape its tone, and soften harsh S sounds."
            }
        }
    }
    pub fn cache_key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Standard => "standard",
            Self::Loud => "loud",
            Self::NoiseReduction => "noise-reduction",
            Self::Podcast => "podcast",
        }
    }
    pub fn requires_neural(self) -> bool {
        matches!(self, Self::NoiseReduction | Self::Podcast)
    }
}
impl From<bool> for AudioMode {
    fn from(value: bool) -> Self {
        Self::from_legacy(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_restore_old_names_and_do_not_enable_cleanup_for_old_off_projects() {
        assert_eq!(AudioMode::from_legacy(false), AudioMode::Off);
        assert_eq!(AudioMode::from_legacy(true), AudioMode::NoiseReduction);
        for mode in AudioMode::ALL {
            assert_eq!(
                serde_json::from_str::<AudioMode>(&serde_json::to_string(&mode).unwrap()).unwrap(),
                mode
            );
        }
        assert_eq!(
            serde_json::from_str::<AudioMode>("\"voice\"").unwrap(),
            AudioMode::Standard
        );
        assert_eq!(
            serde_json::from_str::<AudioMode>("\"channel-fix\"").unwrap(),
            AudioMode::Off
        );
    }
}
