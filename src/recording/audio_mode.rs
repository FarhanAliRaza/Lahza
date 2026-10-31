//! Microphone processing is distinct from recording/system-audio mixing.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioMode {
    /// Earlier Standard and Loud presets (DSP only) open as Off.
    #[default]
    #[serde(alias = "channel-fix", alias = "standard", alias = "voice", alias = "normalized", alias = "loud")]
    Off,
    /// "Enhance audio" (RE-USE); the earlier Podcast preset opens as this.
    #[serde(alias = "noiseReduction", alias = "podcast")]
    NoiseReduction,
}

impl AudioMode {
    pub const ALL: [Self; 2] = [Self::Off, Self::NoiseReduction];
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
            Self::NoiseReduction => "Enhance audio",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Off => "Keep the microphone level and repair a quiet stereo channel.",
            Self::NoiseReduction => {
                "Remove background noise and room echo locally, then balance the voice level."
            }
        }
    }
    pub fn cache_key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::NoiseReduction => "noise-reduction",
        }
    }
    pub fn requires_neural(self) -> bool {
        self == Self::NoiseReduction
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
        assert_eq!(serde_json::from_str::<AudioMode>("\"podcast\"").unwrap(), AudioMode::NoiseReduction);
        for removed in ["standard", "voice", "normalized", "loud"] {
            assert_eq!(serde_json::from_str::<AudioMode>(&format!("\"{removed}\"")).unwrap(), AudioMode::Off);
        }
        assert_eq!(
            serde_json::from_str::<AudioMode>("\"channel-fix\"").unwrap(),
            AudioMode::Off
        );
    }
}
