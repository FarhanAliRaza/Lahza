use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs, io,
    path::Path,
};
use uuid::Uuid;

pub const SESSION_EXTENSION: &str = "lahzarec";

/// Platform-neutral cursor shape, derived from the Xcursor name the captured
/// bitmap matched.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CursorShape {
    #[default]
    Arrow,
    PointingHand,
    OpenHand,
    ClosedHand,
    IBeam,
}

impl CursorShape {
    /// Maps the file names used by Xcursor themes (and their common legacy
    /// aliases) to a shape.
    pub fn from_xcursor_name(name: &str) -> Option<Self> {
        Some(match name {
            "default" | "left_ptr" | "arrow" | "top_left_arrow" => Self::Arrow,
            "pointer" | "hand" | "hand2" | "pointing_hand" => Self::PointingHand,
            "grab" | "openhand" | "hand1" => Self::OpenHand,
            "grabbing" | "closedhand" | "dnd-move" | "dnd-none" => Self::ClosedHand,
            "text" | "xterm" | "ibeam" => Self::IBeam,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureManifest {
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub duration: f64,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub pixel_scale: f64,
    pub source_display_id: Option<String>,
    pub includes_system_audio: bool,
    pub includes_microphone: bool,
    pub includes_camera: bool,
    pub pointer_synthesized: bool,
    pub press_effects_baked: bool,
    pub press_effects_enabled: bool,
    pub recording_backend: RecordingBackend,
}

impl Default for CaptureManifest {
    fn default() -> Self {
        Self {
            version: 1,
            created_at: Utc::now(),
            duration: 0.0,
            pixel_width: 0,
            pixel_height: 0,
            pixel_scale: 1.0,
            source_display_id: None,
            includes_system_audio: false,
            includes_microphone: false,
            includes_camera: false,
            pointer_synthesized: false,
            press_effects_baked: true,
            press_effects_enabled: true,
            recording_backend: RecordingBackend::PipeWire,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingBackend {
    #[default]
    #[serde(alias = "obs")]
    PipeWire,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingProjectMetadata {
    pub version: Option<u32>,
    pub display_name: Option<String>,
    pub saved_at: Option<DateTime<Utc>>,
    pub last_opened_at: Option<DateTime<Utc>>,
}

impl RecordingProjectMetadata {
    pub const CURRENT_VERSION: u32 = 1;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerCaptureFile {
    pub format_version: u32,
    pub travel: Vec<PointerTravelSample>,
    pub presses: Vec<PointerPressEvent>,
    /// Hover-target transitions contain no target identity, label, or text.
    #[serde(default)]
    pub hovers: Vec<PointerHoverEvent>,
    pub keystrokes: Vec<KeystrokeEvent>,
    pub artwork: Vec<PointerArtwork>,
    pub pause_intervals: Vec<PauseInterval>,
    pub is_sanitized: bool,
}

impl PointerCaptureFile {
    pub const CURRENT_FORMAT_VERSION: u32 = 2;
}

impl Default for PointerCaptureFile {
    fn default() -> Self {
        Self {
            format_version: Self::CURRENT_FORMAT_VERSION,
            travel: Vec::new(),
            presses: Vec::new(),
            hovers: Vec::new(),
            keystrokes: Vec::new(),
            artwork: Vec::new(),
            pause_intervals: Vec::new(),
            is_sanitized: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerTravelSample {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub kind: PointerTravelKind,
    pub artwork_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PointerTravelKind {
    #[default]
    Move,
    Drag,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerHoverEvent {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerPressEvent {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub button: u8,
    pub phase: PressPhase,
    pub artwork_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PressPhase {
    Down,
    Up,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeystrokeEvent {
    pub time: f64,
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerArtwork {
    pub artwork_id: String,
    pub image_data_base64: String,
    pub anchor_point: NormalizedPoint,
    pub reference_width: f64,
    pub reference_height: f64,
    /// Shape the bitmap was recognised as, when a cursor theme matched it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<CursorShape>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseInterval {
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NormalizedPoint {
    pub x: f64,
    pub y: f64,
}

impl NormalizedPoint {
    pub fn clamped(self) -> Self {
        Self {
            x: finite_unit(self.x),
            y: finite_unit(self.y),
        }
    }
}

fn finite_unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("sidecar.json");
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4()));
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// Lahza's configuration directory.
pub fn config_root() -> std::path::PathBuf {
    if let Some(root) = std::env::var_os("LAHZA_CONFIG_DIR") {
        return std::path::PathBuf::from(root);
    }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        return std::path::PathBuf::from(root).join("lahza");
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".config").join("lahza")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_hover_metadata_is_additive_and_contains_no_target_identity() {
        let old = serde_json::json!({"formatVersion":1,"travel":[],"presses":[],
            "keystrokes":[],"artwork":[],"pauseIntervals":[],"isSanitized":false});
        let mut capture: PointerCaptureFile = serde_json::from_value(old).unwrap();
        assert!(capture.hovers.is_empty());
        capture.hovers.push(PointerHoverEvent { time: 0.25, x: 0.4, y: 0.6, artwork_id: None });
        let encoded = serde_json::to_value(&capture).unwrap();
        assert_eq!(encoded["hovers"][0], serde_json::json!({"time":0.25,"x":0.4,"y":0.6}));
        assert_eq!(serde_json::from_value::<PointerCaptureFile>(encoded).unwrap(), capture);
    }
}
