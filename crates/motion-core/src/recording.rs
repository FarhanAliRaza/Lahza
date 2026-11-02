//! Motion pieces inside a Lahza recording project.
//!
//! A recording folder (`*.lahzarec`) can hold motion projects under
//! `motion/<name>.lahza/`. The recording's edit document lists where each
//! one plays on the video timeline (`motionPieces`): over the video as an
//! overlay, or covering it as a full-frame segment. Motion projects share
//! the recording's files: a layer can use `../../screen.mkv` (the raw
//! recording) or `../../media/<file>` (imported media), and media imported
//! into a piece lands in the shared `media/` folder.
//!
//! The edit document is written the way the studio writes it: the draft
//! (`edit.draft.json`) is updated, other keys are left untouched, and the
//! studio commits the draft on export.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const EDIT_FILE: &str = "edit.json";
pub const DRAFT_FILE: &str = "edit.draft.json";
pub const PIECES_KEY: &str = "motionPieces";
/// Folder of motion projects inside a recording.
pub const MOTION_DIR: &str = "motion";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Drawn over the video; transparent parts show the recording.
    #[default]
    Overlay,
    /// Covers the frame for its duration, like an inserted clip.
    Full,
}

impl Placement {
    pub fn label(self) -> &'static str {
        match self {
            Placement::Overlay => "Over the video",
            Placement::Full => "Full screen",
        }
    }
}

/// One motion project placed on a recording's timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionPiece {
    pub id: String,
    pub name: String,
    /// Relative to the recording folder: `motion/<name>.lahza`.
    pub folder: String,
    /// Editor time (seconds on the edited video) where it starts.
    pub start: f64,
    #[serde(default)]
    pub placement: Placement,
    #[serde(default = "one")]
    pub opacity: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

fn one() -> f64 {
    1.0
}

impl MotionPiece {
    pub fn dir(&self, recording: &Path) -> PathBuf {
        recording.join(&self.folder)
    }
}

pub fn is_recording(dir: &Path) -> bool {
    dir.extension().and_then(|e| e.to_str()) == Some("lahzarec") && dir.is_dir()
}

/// The recording a motion project lives in, if it is `<rec>/motion/<name>`.
pub fn recording_of(project_dir: &Path) -> Option<PathBuf> {
    let parent = project_dir.parent()?;
    if parent.file_name()? != MOTION_DIR {
        return None;
    }
    let rec = parent.parent()?;
    is_recording(rec).then(|| rec.to_path_buf())
}

/// The edit document the studio would open: the draft when there is one.
pub fn read_edit(recording: &Path) -> Value {
    for name in [DRAFT_FILE, EDIT_FILE] {
        if let Ok(text) = fs::read_to_string(recording.join(name)) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v.is_object() {
                    return v;
                }
            }
        }
    }
    json!({})
}

pub fn read_pieces(recording: &Path) -> Vec<MotionPiece> {
    read_edit(recording)
        .get(PIECES_KEY)
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Replaces `motionPieces` in the draft, keeping every other key.
pub fn write_pieces(recording: &Path, pieces: &[MotionPiece]) -> Result<(), String> {
    let mut doc = read_edit(recording);
    let object = doc.as_object_mut().ok_or("edit document is not an object")?;
    let version = object.get("formatVersion").and_then(Value::as_u64).unwrap_or(0).max(5);
    object.insert("formatVersion".into(), json!(version));
    object.insert(PIECES_KEY.into(), serde_json::to_value(pieces).map_err(|e| e.to_string())?);
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let tmp = recording.join(format!("{DRAFT_FILE}.tmp"));
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, recording.join(DRAFT_FILE)).map_err(|e| e.to_string())
}

/// `path` as seen from a motion project inside `recording`
/// (`../../media/x.png`), when the file is inside the recording.
pub fn shared_path(project_dir: &Path, recording: &Path, file: &Path) -> Option<String> {
    let inside = file.strip_prefix(recording).ok()?;
    let depth = project_dir.strip_prefix(recording).ok()?.components().count();
    let mut rel = PathBuf::new();
    for _ in 0..depth {
        rel.push("..");
    }
    rel.push(inside);
    Some(rel.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_round_trip_through_the_draft_and_keep_other_keys() {
        let rec = std::env::temp_dir().join(format!("lahza-rec-{}.lahzarec", std::process::id()));
        let _ = fs::remove_dir_all(&rec);
        fs::create_dir_all(rec.join("motion/intro.lahza")).unwrap();
        fs::write(rec.join(EDIT_FILE), r#"{"formatVersion": 6, "clips": [{"id": "a"}]}"#).unwrap();
        assert!(read_pieces(&rec).is_empty());
        let piece = MotionPiece {
            id: "p1".into(),
            name: "Intro".into(),
            folder: "motion/intro.lahza".into(),
            start: 2.5,
            placement: Placement::Full,
            opacity: 1.0,
            hidden: false,
        };
        write_pieces(&rec, &[piece.clone()]).unwrap();
        assert_eq!(read_pieces(&rec), vec![piece]);
        let draft: Value = serde_json::from_str(&fs::read_to_string(rec.join(DRAFT_FILE)).unwrap()).unwrap();
        assert_eq!(draft["clips"][0]["id"], "a");
        assert_eq!(draft["formatVersion"], 6);
        // edit.json itself is untouched until the studio commits.
        assert!(fs::read_to_string(rec.join(EDIT_FILE)).unwrap().contains("\"clips\""));
        assert!(!fs::read_to_string(rec.join(EDIT_FILE)).unwrap().contains(PIECES_KEY));
        let project = rec.join("motion/intro.lahza");
        assert_eq!(recording_of(&project), Some(rec.clone()));
        assert_eq!(shared_path(&project, &rec, &rec.join("media/shot.png")).as_deref(), Some("../../media/shot.png"));
        assert_eq!(shared_path(&project, &rec, Path::new("/elsewhere/x.png")), None);
        fs::remove_dir_all(&rec).ok();
    }
}
