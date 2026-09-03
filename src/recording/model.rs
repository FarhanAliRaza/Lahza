use super::cursor_assets::CursorShape;
use chrono::{DateTime, Local, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use uuid::Uuid;

use super::clips::{RecordingClipSegment, RecordingClipTimeline};
use super::viewport::ZoomCue;

pub const SESSION_EXTENSION: &str = "screendroprec";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordingSession {
    pub directory: PathBuf,
}

impl RecordingSession {
    pub const SCREEN_FILE: &'static str = "screen.mkv";
    pub const CAMERA_FILE: &'static str = "camera.mkv";
    pub const POINTER_FILE: &'static str = "input.json";
    pub const CAPTURE_FILE: &'static str = "capture.json";
    pub const EDIT_FILE: &'static str = "edit.json";
    pub const DRAFT_FILE: &'static str = "edit.draft.json";
    pub const PROJECT_FILE: &'static str = "project.json";
    pub const RENDER_STAMP_FILE: &'static str = "render.json";
    pub const POSTER_FILE: &'static str = "poster.jpg";
    pub const REPLACEMENT_AUDIO_STEM: &'static str = "audio-replacement";
    /// Derived copy of the screen recording with noise-reduced audio.
    pub const DENOISED_FILE: &'static str = ".audio-denoised.mkv";

    pub fn create() -> io::Result<Self> {
        Self::create_in(recordings_root())
    }

    pub fn create_in(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(&root)?;
        let stamp: DateTime<Local> = Local::now();
        let name = format!(
            "Lahza_{}_{}.{}",
            stamp.format("%Y-%m-%d-%H-%M-%S"),
            &Uuid::new_v4().simple().to_string()[..6],
            SESSION_EXTENSION
        );
        let directory = root.join(name);
        fs::create_dir(&directory)?;
        Ok(Self { directory })
    }

    pub fn denoised_path(&self) -> PathBuf {
        self.directory.join(Self::DENOISED_FILE)
    }

    pub fn screen_path(&self) -> PathBuf {
        self.directory.join(Self::SCREEN_FILE)
    }

    pub fn pointer_path(&self) -> PathBuf {
        self.directory.join(Self::POINTER_FILE)
    }

    pub fn camera_path(&self) -> PathBuf {
        self.directory.join(Self::CAMERA_FILE)
    }

    pub fn edit_path(&self) -> PathBuf {
        self.directory.join(Self::EDIT_FILE)
    }

    pub fn draft_path(&self) -> PathBuf {
        self.directory.join(Self::DRAFT_FILE)
    }

    pub fn project_path(&self) -> PathBuf {
        self.directory.join(Self::PROJECT_FILE)
    }

    pub fn render_stamp_path(&self) -> PathBuf {
        self.directory.join(Self::RENDER_STAMP_FILE)
    }

    pub fn poster_path(&self) -> PathBuf {
        self.directory.join(Self::POSTER_FILE)
    }

    pub fn capture_path(&self) -> PathBuf {
        self.directory.join(Self::CAPTURE_FILE)
    }

    pub fn write_manifest(&self, manifest: &CaptureManifest) -> io::Result<()> {
        write_json_atomic(&self.capture_path(), manifest)
    }

    pub fn write_pointer_capture(&self, capture: &PointerCaptureFile) -> io::Result<()> {
        write_json_atomic(&self.pointer_path(), capture)
    }

    pub fn read_manifest(&self) -> io::Result<CaptureManifest> {
        read_json(&self.capture_path())
    }

    pub fn read_pointer_capture(&self) -> io::Result<PointerCaptureFile> {
        read_json(&self.pointer_path())
    }

    pub fn load_edit_document(&self) -> io::Result<Option<serde_json::Value>> {
        read_optional_json(&self.edit_path())
    }

    pub fn write_edit_document(&self, document: &serde_json::Value) -> io::Result<()> {
        write_json_atomic(&self.edit_path(), document)
    }

    pub fn load_draft_document(&self) -> io::Result<Option<serde_json::Value>> {
        read_optional_json(&self.draft_path())
    }

    pub fn write_draft_document(&self, document: &serde_json::Value) -> io::Result<()> {
        write_json_atomic(&self.draft_path(), document)
    }

    pub fn remove_draft_document(&self) -> io::Result<()> {
        match fs::remove_file(self.draft_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The editor always reopens the crash-safe working copy first. Consumers
    /// outside the editor use the same rule so an export cannot silently omit
    /// the edits visible before a crash.
    pub fn effective_edit_document(&self) -> io::Result<Option<serde_json::Value>> {
        match self.load_draft_document()? {
            Some(draft) => Ok(Some(draft)),
            None => self.load_edit_document(),
        }
    }

    /// Reads Swift's v5 clip schema, falling back to its legacy trim envelope
    /// and finally the full source. Unknown edit fields remain untouched.
    pub fn effective_clip_timeline(
        &self,
        source_duration: f64,
    ) -> io::Result<RecordingClipTimeline> {
        let Some(document) = self.effective_edit_document()? else {
            return Ok(RecordingClipTimeline::full(source_duration));
        };
        if let Some(clips) = document.get("clips").filter(|value| !value.is_null()) {
            let segments: Vec<RecordingClipSegment> = serde_json::from_value(clips.clone())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            return Ok(RecordingClipTimeline::new(segments).normalized(source_duration));
        }
        Ok(RecordingClipTimeline::legacy_trim(
            document
                .get("trimStart")
                .and_then(serde_json::Value::as_f64),
            document.get("trimEnd").and_then(serde_json::Value::as_f64),
            source_duration,
        ))
    }

    /// Autosaves only the clip fields into the working copy. This deliberately
    /// mutates the existing JSON object instead of re-encoding the whole edit
    /// document, preserving Swift/newer-build fields byte-for-value.
    pub fn write_clip_timeline_draft(&self, timeline: &RecordingClipTimeline) -> io::Result<()> {
        let mut document = self
            .effective_edit_document()?
            .unwrap_or_else(|| serde_json::json!({}));
        let object = document.as_object_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "edit document must be an object",
            )
        })?;
        let clips = serde_json::to_value(&timeline.segments)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        object.insert("clips".into(), clips);
        if let Some(only) = timeline
            .segments
            .first()
            .filter(|_| timeline.segments.len() == 1)
        {
            object.insert("trimStart".into(), serde_json::json!(only.source_start));
            object.insert("trimEnd".into(), serde_json::json!(only.source_end));
        } else {
            object.insert("trimStart".into(), serde_json::Value::Null);
            object.insert("trimEnd".into(), serde_json::Value::Null);
        }
        let version = object
            .get("formatVersion")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .max(5);
        object.insert("formatVersion".into(), serde_json::json!(version));
        self.write_draft_document(&document)
    }

    /// Reads the editable zoom lane written by both the Swift and Rust
    /// editors. `None` means the project has never materialized its generated
    /// click zooms, so the caller may seed it from the pointer capture.
    pub fn effective_zoom_cues(&self) -> io::Result<Option<Vec<ZoomCue>>> {
        let Some(document) = self.effective_edit_document()? else {
            return Ok(None);
        };
        let Some(cues) = document.get("zoomCues").filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        serde_json::from_value(cues.clone())
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Autosaves the zoom lane without re-encoding unrelated edit settings.
    pub fn write_zoom_cues_draft(&self, cues: &[ZoomCue]) -> io::Result<()> {
        let mut document = self
            .effective_edit_document()?
            .unwrap_or_else(|| serde_json::json!({}));
        let object = document.as_object_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "edit document must be an object",
            )
        })?;
        object.insert(
            "zoomCues".into(),
            serde_json::to_value(cues)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        let version = object
            .get("formatVersion")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .max(5);
        object.insert("formatVersion".into(), serde_json::json!(version));
        self.write_draft_document(&document)
    }

    /// Reads one top-level field of the effective edit document.
    pub fn read_edit_field<T: DeserializeOwned>(&self, key: &str) -> io::Result<Option<T>> {
        let Some(document) = self.effective_edit_document()? else {
            return Ok(None);
        };
        let Some(value) = document.get(key).filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Autosaves one top-level field without re-encoding unrelated settings.
    pub fn write_edit_field<T: Serialize>(&self, key: &str, value: &T) -> io::Result<()> {
        let mut document = self
            .effective_edit_document()?
            .unwrap_or_else(|| serde_json::json!({}));
        let object = document.as_object_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "edit document must be an object",
            )
        })?;
        object.insert(
            key.to_string(),
            serde_json::to_value(value)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        let version = object
            .get("formatVersion")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .max(5);
        object.insert("formatVersion".into(), serde_json::json!(version));
        self.write_draft_document(&document)
    }

    pub fn has_unsaved_draft(&self) -> io::Result<bool> {
        let Some(draft) = self.load_draft_document()? else {
            return Ok(false);
        };
        Ok(self.load_edit_document()?.as_ref() != Some(&draft))
    }

    /// Explicit save commits the exact draft JSON, preserving fields written
    /// by newer Swift or Rust builds, then removes only the working copy.
    pub fn commit_draft(&self) -> io::Result<bool> {
        let Some(draft) = self.load_draft_document()? else {
            return Ok(false);
        };
        self.write_edit_document(&draft)?;
        self.remove_draft_document()?;
        self.update_project_metadata(|metadata| metadata.saved_at = Some(Utc::now()))?;
        Ok(true)
    }

    pub fn load_project_metadata(&self) -> io::Result<Option<RecordingProjectMetadata>> {
        read_optional_json(&self.project_path())
    }

    pub fn write_project_metadata(&self, metadata: &RecordingProjectMetadata) -> io::Result<()> {
        write_json_atomic(&self.project_path(), metadata)
    }

    pub fn update_project_metadata(
        &self,
        mutate: impl FnOnce(&mut RecordingProjectMetadata),
    ) -> io::Result<()> {
        let mut metadata = self.load_project_metadata()?.unwrap_or_default();
        metadata.version = Some(RecordingProjectMetadata::CURRENT_VERSION);
        mutate(&mut metadata);
        self.write_project_metadata(&metadata)
    }

    pub fn load_render_stamp(&self) -> io::Result<Option<serde_json::Value>> {
        read_optional_json(&self.render_stamp_path())
    }

    pub fn write_render_stamp(&self, document: Option<&serde_json::Value>) -> io::Result<()> {
        match document {
            Some(document) => write_json_atomic(&self.render_stamp_path(), document),
            None => match fs::remove_file(self.render_stamp_path()) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
    }

    pub fn display_name(&self) -> io::Result<String> {
        if let Some(name) = self
            .load_project_metadata()?
            .and_then(|metadata| metadata.display_name)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
        {
            return Ok(name);
        }
        Ok(self
            .directory
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Screendrop recording")
            .to_string())
    }
}

pub fn recoverable_sessions() -> io::Result<Vec<RecordingSession>> {
    recoverable_sessions_in(recordings_root())
}

fn recoverable_sessions_in(root: impl AsRef<Path>) -> io::Result<Vec<RecordingSession>> {
    let mut sessions = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(sessions),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let directory = entry.path();
        if directory.extension().and_then(|value| value.to_str()) != Some(SESSION_EXTENSION) {
            continue;
        }
        let session = RecordingSession { directory };
        if !session.screen_path().exists() {
            if let Some(raw_output) = newest_raw_recording(&session.directory)? {
                fs::rename(raw_output, session.screen_path())?;
            }
        }
        if session
            .screen_path()
            .metadata()
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
        {
            sessions.push(session);
        }
    }
    sessions.sort_by(|left, right| right.directory.cmp(&left.directory));
    Ok(sessions)
}

fn newest_raw_recording(directory: &Path) -> io::Result<Option<PathBuf>> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(RecordingSession::CAMERA_FILE) {
            continue;
        }
        let supported = matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("mkv" | "mov" | "mp4")
        );
        let metadata = match path.metadata() {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 && supported => metadata,
            _ => continue,
        };
        candidates.push((metadata.modified().ok(), metadata.len(), path));
    }
    candidates.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    Ok(candidates.pop().map(|(_, _, path)| path))
}

/// Where new recording projects are created. `SCREENDROP_RECORDINGS_DIR`
/// overrides the default of `<XDG videos dir>/Screendrop` (usually
/// `~/Videos/Screendrop`).
pub fn recordings_root() -> PathBuf {
    if let Some(root) = std::env::var_os("SCREENDROP_RECORDINGS_DIR") {
        return PathBuf::from(root);
    }
    videos_dir().join("Screendrop")
}

fn videos_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if let Ok(contents) = fs::read_to_string(home.join(".config/user-dirs.dirs")) {
        for line in contents.lines() {
            let Some(value) = line.trim().strip_prefix("XDG_VIDEOS_DIR=") else {
                continue;
            };
            let value = value.trim_matches('"');
            if let Some(rest) = value.strip_prefix("$HOME/") {
                return home.join(rest);
            }
            if value == "$HOME" {
                break;
            }
            if value.starts_with('/') {
                return PathBuf::from(value);
            }
        }
    }
    home.join("Videos")
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
    pub keystrokes: Vec<KeystrokeEvent>,
    pub artwork: Vec<PointerArtwork>,
    pub pause_intervals: Vec<PauseInterval>,
    pub is_sanitized: bool,
}

impl PointerCaptureFile {
    pub const CURRENT_FORMAT_VERSION: u32 = 1;
}

impl Default for PointerCaptureFile {
    fn default() -> Self {
        Self {
            format_version: Self::CURRENT_FORMAT_VERSION,
            travel: Vec::new(),
            presses: Vec::new(),
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

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
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

fn read_optional_json<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match read_json(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::viewport::{ZoomAnchorMode, ZoomCue};

    #[test]
    fn recovery_adopts_a_finalized_encoder_file_left_by_a_crash() {
        let root =
            std::env::temp_dir().join(format!("screendrop-recovery-test-{}", uuid::Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        let raw = session.directory.join("2026-08-24 00-01-02.mkv");
        fs::write(&raw, b"recoverable video").unwrap();

        let recovered = recoverable_sessions_in(&root).unwrap();

        assert_eq!(recovered, vec![session.clone()]);
        assert_eq!(
            fs::read(session.screen_path()).unwrap(),
            b"recoverable video"
        );
        assert!(!raw.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn draft_is_separate_until_explicit_commit_and_preserves_unknown_fields() {
        let root = std::env::temp_dir().join(format!("screendrop-draft-test-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        let saved = serde_json::json!({
            "formatVersion": 5,
            "zoomEnabled": true,
            "futureSwiftField": { "kept": true }
        });
        let draft = serde_json::json!({
            "formatVersion": 6,
            "zoomEnabled": false,
            "futureSwiftField": { "kept": true, "also": 42 }
        });
        session.write_edit_document(&saved).unwrap();
        session.write_draft_document(&draft).unwrap();

        assert_eq!(session.load_edit_document().unwrap(), Some(saved));
        assert_eq!(
            session.effective_edit_document().unwrap(),
            Some(draft.clone())
        );
        assert!(session.has_unsaved_draft().unwrap());
        assert!(session.commit_draft().unwrap());
        assert_eq!(session.load_edit_document().unwrap(), Some(draft));
        assert!(session.load_draft_document().unwrap().is_none());
        assert!(!session.has_unsaved_draft().unwrap());
        assert!(session
            .load_project_metadata()
            .unwrap()
            .unwrap()
            .saved_at
            .is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clip_draft_matches_swift_v5_and_preserves_other_edit_fields() {
        let root = std::env::temp_dir().join(format!("screendrop-clips-test-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        session
            .write_edit_document(&serde_json::json!({
                "formatVersion": 4,
                "zoomEnabled": true,
                "futureField": [1, 2, 3],
                "trimStart": 1.0,
                "trimEnd": 8.0
            }))
            .unwrap();
        let legacy = session.effective_clip_timeline(10.0).unwrap();
        assert_eq!(legacy.segments[0].source_start, 1.0);
        assert_eq!(legacy.segments[0].source_end, 8.0);

        let (timeline, _) = legacy.split_at(2.0).unwrap();
        session.write_clip_timeline_draft(&timeline).unwrap();
        let draft = session.load_draft_document().unwrap().unwrap();
        assert_eq!(draft["formatVersion"], 5);
        assert_eq!(draft["futureField"], serde_json::json!([1, 2, 3]));
        assert!(draft["trimStart"].is_null());
        assert_eq!(draft["clips"].as_array().unwrap().len(), 2);
        assert_eq!(session.effective_clip_timeline(10.0).unwrap(), timeline);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zoom_draft_matches_swift_v5_and_preserves_clip_edits() {
        let root = std::env::temp_dir().join(format!("screendrop-zoom-test-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        let clip = RecordingClipTimeline::full(8.0);
        session.write_clip_timeline_draft(&clip).unwrap();
        let cue = ZoomCue {
            id: Uuid::new_v4(),
            start: 1.25,
            end: 3.75,
            zoom: 1.5,
            anchor_mode: ZoomAnchorMode::PointerAnchor,
            pinned_point: NormalizedPoint { x: 0.4, y: 0.6 },
            bounds_bias: 0.25,
            is_enabled: true,
            is_implicit: false,
            skips_easing: false,
            motion: Default::default(),
            pan_to: None,
            easing: Default::default(),
            tilt: None,
        };

        session
            .write_zoom_cues_draft(std::slice::from_ref(&cue))
            .unwrap();
        let draft = session.load_draft_document().unwrap().unwrap();
        assert_eq!(draft["formatVersion"], 5);
        assert_eq!(draft["clips"].as_array().unwrap().len(), 1);
        assert_eq!(session.effective_zoom_cues().unwrap(), Some(vec![cue]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn display_name_uses_trimmed_metadata_then_package_name() {
        let root = std::env::temp_dir().join(format!("screendrop-name-test-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        let package_name = session.directory.file_stem().unwrap().to_str().unwrap();
        assert_eq!(session.display_name().unwrap(), package_name);
        session
            .update_project_metadata(|metadata| metadata.display_name = Some("  Demo  ".into()))
            .unwrap();
        assert_eq!(session.display_name().unwrap(), "Demo");
        fs::remove_dir_all(root).unwrap();
    }
}
