use chrono::{DateTime, Local, Utc};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

use super::clips::{RecordingClipSegment, RecordingClipTimeline};
use super::model::{
    read_json, write_json_atomic, CaptureManifest, PointerCaptureFile, RecordingProjectMetadata,
    SESSION_EXTENSION,
};
use super::video::probe_media;
use super::viewport::ZoomCue;

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
    pub const POSTER_FILE: &'static str = "poster.png";
    pub const REPLACEMENT_AUDIO_STEM: &'static str = "audio-replacement";

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

    pub fn screen_path(&self) -> PathBuf {
        self.directory.join(Self::SCREEN_FILE)
    }

    /// Restore a take interrupted before normal finalization. Native screen
    /// segments are recovered in order, with all audio streams; the webcam is
    /// recovered independently and can never replace the screen recording.
    pub fn recover_interrupted_capture(&self) -> io::Result<bool> {
        let mut recovered = recover_interrupted_segments(&self.screen_path())
            .map_err(io::Error::other)?;
        if !self.screen_path().metadata().is_ok_and(|meta| meta.len() > 0) {
            if let Some(raw) = newest_legacy_recording(&self.directory)? {
                fs::rename(raw, self.screen_path())?;
                recovered = true;
            }
        }
        if !recovered && self.capture_path().exists() {
            return Ok(false);
        }
        if !self.screen_path().metadata().is_ok_and(|meta| meta.len() > 0) {
            return Ok(false);
        }
        let info = probe_media(&self.screen_path()).map_err(io::Error::other)?;
        let mut manifest = self.read_manifest().unwrap_or_default();
        manifest.duration = info.duration;
        manifest.pixel_width = info.width;
        manifest.pixel_height = info.height;
        manifest.includes_camera = self.camera_path().metadata().is_ok_and(|meta| meta.len() > 0);
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream_tags=title", "-of", "json"])
            .arg(self.screen_path()).output()?;
        if probe.status.success() {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&probe.stdout) {
                for stream in value["streams"].as_array().into_iter().flatten() {
                    let title = stream["tags"].as_object().and_then(|tags| tags.iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case("title")))
                        .and_then(|(_, value)| value.as_str()).unwrap_or("");
                    match title.replace('-', " ").to_ascii_lowercase().as_str() {
                        "lahza microphone" => manifest.includes_microphone = true,
                        "lahza system audio" => manifest.includes_system_audio = true,
                        _ => {}
                    }
                }
            }
        }
        self.write_manifest(&manifest)?;
        Ok(true)
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
            .unwrap_or("Lahza recording")
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
        if directory.extension().and_then(|value| value.to_str()) != Some(SESSION_EXTENSION)
            || !entry.file_type()?.is_dir() {
            continue;
        }
        let session = RecordingSession { directory };
        if let Err(error) = session.recover_interrupted_capture() {
            eprintln!("Could not recover {}: {error}", session.directory.display());
            continue;
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

fn newest_legacy_recording(directory: &Path) -> io::Result<Option<PathBuf>> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == RecordingSession::CAMERA_FILE || name == RecordingSession::SCREEN_FILE
            || name.starts_with("camera-") || name.starts_with("screen-segment-")
            || name.starts_with("audio-") || name.starts_with('.') {
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

/// Where new recording projects are created. `LAHZA_RECORDINGS_DIR`
/// overrides the default of `<XDG videos dir>/Lahza` (usually
/// `~/Videos/Lahza`).
pub fn recordings_root() -> PathBuf {
    if let Some(root) = std::env::var_os("LAHZA_RECORDINGS_DIR") {
        return PathBuf::from(root);
    }
    videos_dir().join("Lahza")
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

fn read_optional_json<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match read_json(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn recover_segment(path: &Path) -> Result<(), String> {
    let recovered = path.with_extension("recovered.mkv");
    remux_recovered_segment(path, &recovered)?;
    fs::rename(recovered, path).map_err(|error| format!("could not install recovered recording: {error}"))
}

pub fn remux_recovered_segment(path: &Path, recovered: &Path) -> Result<(), String> {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-copyts", "-copytb", "1", "-i"])
        .arg(path)
        .args(["-map", "0", "-c", "copy"])
        .arg(recovered)
        .output()
        .map_err(|error| format!("could not recover recording: {error}"))?;
    if !result.status.success()
        || !probe_media(recovered).is_ok_and(|info| info.duration.is_finite() && info.duration > 0.0)
    {
        let _ = fs::remove_file(recovered);
        return Err(format!("could not finalize recording; captured segments remain at {}: {}",
            path.display(), String::from_utf8_lossy(&result.stderr).trim()));
    }
    Ok(())
}

pub fn interrupted_segments(directory: &Path, role: &str) -> Result<Vec<PathBuf>, String> {
    let prefix = format!("{role}-segment-");
    let mut segments = Vec::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue; };
        let Some(number) = name.strip_prefix(&prefix).and_then(|name| name.strip_suffix(".mkv")) else { continue; };
        let Ok(index) = number.parse::<usize>() else { continue; };
        if name != format!("{prefix}{index:04}.mkv") || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        segments.push((index, entry.path()));
    }
    segments.sort_by_key(|(index, _)| *index);
    if segments.iter().enumerate().any(|(expected, (actual, _))| expected != *actual) {
        return Err(format!("Some {role} recording segments are missing; the remaining files have been kept in {}", directory.display()));
    }
    Ok(segments.into_iter().map(|(_, path)| path).collect())
}

/// Reconstruct every ordered native segment, never a webcam or newest-only
/// substitute for the screen. Work on remuxed copies so a failed repair leaves
/// every original available for another recovery attempt.
pub fn recover_interrupted_segments(output: &Path) -> Result<bool, String> {
    let directory = output.parent().ok_or("recording has no project directory")?;
    let screen_exists = output.metadata().is_ok_and(|meta| meta.len() > 0);
    // Older builds concatenated directly into screen.mkv. A crash could leave
    // a nonempty partial final file next to every original segment. Prefer the
    // complete contiguous source set even when that destination already exists.
    let screen = match interrupted_segments(directory, "screen") {
        Ok(segments) => segments,
        Err(error) if screen_exists && valid_finalized_media(output) => {
            // A crash after atomic installation can interrupt source cleanup.
            // An incomplete remainder cannot reconstruct the complete take.
            eprintln!("Keeping finalized screen recording: {error}");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let camera_output = camera_path(output);
    let camera = interrupted_segments(directory, "camera").unwrap_or_else(|error| {
        eprintln!("Webcam recovery: {error}");
        Vec::new()
    });
    if screen.is_empty() && (!screen_exists || camera.is_empty()) {
        return Ok(false);
    }
    let work = directory.join(format!(".recovery-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&work).map_err(|e| format!("could not create recovery workspace: {e}"))?;
    let repair = |sources: &[PathBuf], destination: &Path, role: &str| -> Result<(), String> {
        let copies: Vec<_> = sources.iter().enumerate()
            .map(|(index, _)| work.join(format!("{role}-{index:04}.mkv"))).collect();
        for (source, copy) in sources.iter().zip(&copies) {
            remux_recovered_segment(source, copy)?;
        }
        let completed = work.join(format!("{role}.mkv"));
        finalize_segments(&copies, &completed)?;
        let completed_duration = probe_media(&completed).map_err(|e| e.to_string())?.duration;
        if let Ok(existing) = probe_media(destination) {
            if existing.duration > completed_duration + 0.15 {
                return Err(format!("The remaining {role} segments are shorter than the existing recording; all originals were kept"));
            }
        }
        if role == "camera" {
            let screen_duration = probe_media(output).map_err(|e| e.to_string())?.duration;
            if (screen_duration - completed_duration).abs() > 0.15 {
                return Err("Recovered webcam duration does not match the screen; the original camera segments were kept".into());
            }
        }
        fs::rename(&completed, destination).map_err(|e| format!("could not install recovered {role}: {e}"))?;
        for source in sources {
            let _ = fs::remove_file(source);
        }
        Ok(())
    };
    if !screen.is_empty() {
        if let Err(error) = repair(&screen, output, "screen") {
            let _ = fs::remove_dir_all(&work);
            return Err(error);
        }
    }
    if !camera.is_empty() {
        if !screen.is_empty() && camera.len() != screen.len() {
            eprintln!("Webcam recovery: screen and camera segment counts differ; camera files remain in {}", directory.display());
        } else if let Err(error) = repair(&camera, &camera_output, "camera") {
            eprintln!("Webcam recovery: {error}");
        }
    }
    let _ = fs::remove_dir_all(&work);
    Ok(true)
}

pub fn camera_segment_path(output: &Path, index: usize) -> PathBuf {
    output.with_file_name(format!("camera-segment-{index:04}.mkv"))
}

pub fn camera_path(output: &Path) -> PathBuf {
    output.with_file_name(RecordingSession::CAMERA_FILE)
}

pub fn finalize_segments(segments: &[PathBuf], output: &Path) -> Result<(), String> {
    let usable: Vec<_> = segments
        .iter()
        .filter(|path| path.metadata().map(|meta| meta.len() > 0).unwrap_or(false))
        .collect();
    if usable.is_empty() {
        return Err("the encoder produced no video".into());
    }
    // FFmpeg's concat demuxer can finish successfully after a bad later input,
    // leaving a playable but shortened file. Validate every source before
    // allowing installation or removal of any captured segment.
    if usable.len() != segments.len() || usable.iter().any(|path| !valid_finalized_media(path)) {
        return Err("Some captured segments are missing or incomplete; all original files were kept".into());
    }
    if usable.len() == 1 {
        fs::rename(usable[0], output)
            .or_else(|_| {
                let temporary = output.with_file_name(format!(".finalizing-{}.mkv", uuid::Uuid::new_v4()));
                if let Err(error) = fs::copy(usable[0], &temporary).and_then(|_| fs::rename(&temporary, output)) {
                    let _ = fs::remove_file(temporary);
                    return Err(error);
                }
                fs::remove_file(usable[0])
            })
            .map_err(|error| format!("could not install recording: {error}"))?;
        return Ok(());
    }

    let list = output.with_file_name(format!(".segments-{}.txt", uuid::Uuid::new_v4()));
    let contents = usable
        .iter()
        .map(|path| format!("file '{}'\n", path.to_string_lossy().replace('\'', "'\\''")))
        .collect::<String>();
    fs::write(&list, contents).map_err(|error| format!("could not list segments: {error}"))?;
    let temporary = output.with_file_name(format!(".finalizing-{}.mkv", uuid::Uuid::new_v4()));
    let result = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list)
        .args(["-map", "0", "-c", "copy"])
        .arg(&temporary)
        .output()
        .map_err(|error| format!("could not start FFmpeg: {error}"))?;
    let _ = fs::remove_file(&list);
    if !result.status.success() || !valid_finalized_media(&temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "could not join paused recording segments: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    fs::rename(&temporary, output).map_err(|error| format!("could not install recording: {error}"))?;
    for path in usable {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub fn valid_finalized_media(path: &Path) -> bool {
    probe_media(path).is_ok_and(|info| {
        info.duration.is_finite() && info.duration > 0.0 && info.width > 0 && info.height > 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NormalizedPoint;

    fn recovery_fixture(path: &Path, frequency: u32) {
        let result = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=64x48:r=30:d=0.5",
                "-f", "lavfi", "-i", &format!("sine=frequency={frequency}:sample_rate=48000:duration=0.5"),
                "-f", "lavfi", "-i", "sine=frequency=1200:sample_rate=48000:duration=0.5",
                "-map", "0:v", "-map", "1:a", "-map", "2:a", "-c:v", "ffv1", "-c:a", "pcm_s16le",
                "-metadata:s:a:0", "title=Lahza-Microphone", "-metadata:s:a:1", "title=Lahza-System-Audio"])
            .arg(path).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }

    use crate::viewport::{ZoomAnchorMode, ZoomCue};

    #[test]
    fn recovery_adopts_a_finalized_encoder_file_left_by_a_crash() {
        let root =
            std::env::temp_dir().join(format!("lahza-recovery-test-{}", uuid::Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        let raw = session.directory.join("2026-08-24 00-01-02.mkv");
        recovery_fixture(&raw, 440);
        let original = fs::read(&raw).unwrap();

        let recovered = recoverable_sessions_in(&root).unwrap();

        assert_eq!(recovered, vec![session.clone()]);
        assert_eq!(
            fs::read(session.screen_path()).unwrap(),
            original
        );
        assert!(!raw.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_joins_every_paused_segment_and_recovers_camera_separately() {
        let root = std::env::temp_dir().join(format!("lahza-recovery-paused-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        // Write reverse order so recovery cannot rely on mtime/newest-file.
        for index in [1, 0] {
            recovery_fixture(&session.directory.join(format!("screen-segment-{index:04}.mkv")), 440 + index * 100);
            recovery_fixture(&session.directory.join(format!("camera-segment-{index:04}.mkv")), 220);
        }
        // Older finalizers wrote directly to the final name. Even a nonempty
        // destination must not hide the complete original segment sequence.
        fs::write(session.screen_path(), b"partial final file left by an older build").unwrap();
        fs::write(session.camera_path(), b"partial webcam final file").unwrap();
        assert_eq!(recoverable_sessions_in(&root).unwrap(), vec![session.clone()]);
        let manifest = session.read_manifest().unwrap();
        assert!((manifest.duration - 1.0).abs() < 0.05, "{}", manifest.duration);
        assert!(manifest.includes_microphone && manifest.includes_system_audio && manifest.includes_camera);
        assert_eq!((manifest.pixel_width, manifest.pixel_height), (64, 48));
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream_tags=title", "-of", "json"])
            .arg(session.screen_path()).output().unwrap();
        let metadata: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(metadata["streams"].as_array().unwrap().len(), 2);
        assert!(session.camera_path().is_file());
        assert!(!session.directory.join("screen-segment-0000.mkv").exists());
        assert!(!session.directory.join("camera-segment-0001.mkv").exists());
        let screen = fs::read(session.screen_path()).unwrap();
        let camera = fs::read(session.camera_path()).unwrap();
        assert!(!session.recover_interrupted_capture().unwrap());
        assert_eq!(screen, fs::read(session.screen_path()).unwrap());
        assert_eq!(camera, fs::read(session.camera_path()).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_recovery_keeps_all_originals_and_does_not_adopt_webcam() {
        let root = std::env::temp_dir().join(format!("lahza-recovery-failed-{}", Uuid::new_v4()));
        let broken = RecordingSession::create_in(&root).unwrap();
        let first = broken.directory.join("screen-segment-0000.mkv");
        recovery_fixture(&first, 440);
        let original = fs::read(&first).unwrap();
        let last = broken.directory.join("screen-segment-0001.mkv");
        fs::write(&last, b"unrecoverable interrupted header").unwrap();
        recovery_fixture(&broken.directory.join("camera-segment-0000.mkv"), 220);
        assert!(broken.recover_interrupted_capture().is_err());
        assert!(!broken.screen_path().exists());
        assert_eq!(fs::read(first).unwrap(), original);
        assert_eq!(fs::read(last).unwrap(), b"unrecoverable interrupted header");

        let camera_only = RecordingSession::create_in(&root).unwrap();
        let camera = camera_only.directory.join("camera-segment-0000.mkv");
        recovery_fixture(&camera, 220);
        let good = RecordingSession::create_in(&root).unwrap();
        recovery_fixture(&good.screen_path(), 880);
        // A broken sibling must not hide the usable project from discovery.
        assert_eq!(recoverable_sessions_in(&root).unwrap(), vec![good]);
        assert!(camera.exists());
        assert!(!camera_only.screen_path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn draft_is_separate_until_explicit_commit_and_preserves_unknown_fields() {
        let root = std::env::temp_dir().join(format!("lahza-draft-test-{}", Uuid::new_v4()));
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
        let root = std::env::temp_dir().join(format!("lahza-clips-test-{}", Uuid::new_v4()));
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
        let root = std::env::temp_dir().join(format!("lahza-zoom-test-{}", Uuid::new_v4()));
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
            presentation: Default::default(),
            loupe: Default::default(),
            start_animation_early: false,
            motion: Default::default(),
            pan_to: None,
            easing: Default::default(),
            camera_motion: super::super::viewport::CameraMotion::Focused,
            tilt: None,
            transform: Some(super::super::viewport::TransformMotion {
                end: super::super::scene_model::SceneTransform {
                    scale: 1.3,
                    position_x: 0.2,
                    rotation_z: 15.0,
                    ..super::super::scene_model::SceneTransform::IDENTITY
                },
                ..Default::default()
            }),
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
        let root = std::env::temp_dir().join(format!("lahza-name-test-{}", Uuid::new_v4()));
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
