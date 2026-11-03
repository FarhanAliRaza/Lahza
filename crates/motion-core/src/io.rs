//! Project folders (`*.lahza/`): `project.json`, `scenes/<id>.json`,
//! `markers/<track>.json`, `assets/`, `history/<n>.json`. Writes are atomic
//! (temporary file + rename) so an interrupted save never corrupts a project.

use crate::document::Project;
use crate::markers::{MarkerTrack, Markers};
use crate::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn err(path: &Path, e: impl std::fmt::Display) -> crate::Error {
    crate::Error(format!("{}: {e}", path.display()))
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| err(parent, e))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        std::process::id()
    ));
    fs::write(&tmp, bytes).map_err(|e| err(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| err(path, e))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| err(path, e))?;
    text.push('\n');
    write_atomic(path, text.as_bytes())
}

fn read_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).map_err(|e| err(path, e))?;
    serde_json::from_str(&text).map_err(|e| err(path, e))
}

pub fn is_project(dir: &Path) -> bool {
    dir.join("project.json").is_file()
}

pub fn load(dir: &Path) -> Result<Project> {
    let file = dir.join("project.json");
    if !file.is_file() {
        bail!("{} is not a Lahza motion project (no project.json)", dir.display());
    }
    let mut doc = read_json(&file)?;
    if let Some(scenes) = doc.get_mut("scenes").and_then(Value::as_array_mut) {
        for entry in scenes.iter_mut() {
            if let Value::String(id) = entry {
                let path = dir.join("scenes").join(format!("{id}.json"));
                *entry = read_json(&path)?;
            }
        }
    }
    let mut project: Project = serde_json::from_value(doc).map_err(|e| err(&file, e))?;
    project.markers = load_markers(dir)?;
    let narration = dir.join("narration.json");
    if narration.is_file() {
        project.narration = Some(serde_json::from_value(read_json(&narration)?).map_err(|e| err(&narration, e))?);
    }
    Ok(project)
}

pub fn save_narration(dir: &Path, narration: &crate::narration::Narration) -> Result<()> {
    write_json(&dir.join("narration.json"), narration)
}

pub fn load_markers(dir: &Path) -> Result<Markers> {
    let mut markers = Markers::default();
    let mdir = dir.join("markers");
    if let Ok(entries) = fs::read_dir(&mdir) {
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let track: MarkerTrack = serde_json::from_value(read_json(&path)?).map_err(|e| err(&path, e))?;
            markers.insert(track);
        }
    }
    Ok(markers)
}

/// Writes `project.json` and `scenes/*.json`; removes scene files of
/// scenes that no longer exist. Markers are saved separately.
pub fn save(dir: &Path, project: &Project) -> Result<()> {
    fs::create_dir_all(dir.join("scenes")).map_err(|e| err(dir, e))?;
    fs::create_dir_all(dir.join("assets")).map_err(|e| err(dir, e))?;
    let mut doc = serde_json::to_value(project).map_err(|e| err(dir, e))?;
    let ids: Vec<String> = project.scenes.iter().map(|s| s.id.clone()).collect();
    for scene in &project.scenes {
        write_json(&dir.join("scenes").join(format!("{}.json", scene.id)), scene)?;
    }
    doc["scenes"] = Value::from(ids.clone());
    write_json(&dir.join("project.json"), &doc)?;
    if let Ok(entries) = fs::read_dir(dir.join("scenes")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if path.extension().and_then(|e| e.to_str()) == Some("json") && !ids.contains(&stem) {
                let _ = fs::remove_file(path);
            }
        }
    }
    Ok(())
}

pub fn save_track(dir: &Path, track: &MarkerTrack) -> Result<()> {
    if !track.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') || track.name.is_empty() {
        bail!("bad marker track name '{}'", track.name);
    }
    write_json(&dir.join("markers").join(format!("{}.json", track.name)), track)
}

pub fn remove_track(dir: &Path, name: &str) -> Result<()> {
    let path = dir.join("markers").join(format!("{name}.json"));
    if path.exists() {
        fs::remove_file(&path).map_err(|e| err(&path, e))?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub n: u64,
    /// Unix seconds.
    pub time: u64,
    /// "agent", "person", or a tool name.
    pub author: String,
    pub summary: Vec<String>,
    pub ops: Vec<Value>,
    pub inverse: Vec<Value>,
    /// Set when a later undo reverted this entry.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undone: bool,
    /// For undo entries: the entries they reverted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undoes: Vec<u64>,
}

fn history_dir(dir: &Path) -> PathBuf {
    dir.join("history")
}

pub fn read_history(dir: &Path) -> Result<Vec<HistoryEntry>> {
    let mut entries = Vec::new();
    if let Ok(read) = fs::read_dir(history_dir(dir)) {
        for e in read.flatten() {
            let path = e.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let entry: HistoryEntry = serde_json::from_value(read_json(&path)?).map_err(|e| err(&path, e))?;
            entries.push(entry);
        }
    }
    entries.sort_by_key(|e| e.n);
    Ok(entries)
}

pub fn write_history(dir: &Path, entry: &HistoryEntry) -> Result<()> {
    write_json(&history_dir(dir).join(format!("{:06}.json", entry.n)), entry)
}

pub fn next_history_number(dir: &Path) -> Result<u64> {
    Ok(read_history(dir)?.last().map(|e| e.n + 1).unwrap_or(1))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Fps;

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("lahza-motion-io-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut p = Project::new(1280, 720, Fps::whole(25), 4.0);
        p.scenes.push(crate::document::Scene::new("intro"));
        p.scenes.push(crate::document::Scene::new("outro"));
        save(&dir, &p).unwrap();
        save_track(&dir, &MarkerTrack::from_times("beat", [0.5, 1.0])).unwrap();
        let mut loaded = load(&dir).unwrap();
        assert_eq!(loaded.markers.get("beat").unwrap().markers.len(), 2);
        loaded.markers = Markers::default();
        assert_eq!(loaded, p);
        p.scenes.pop();
        save(&dir, &p).unwrap();
        assert!(!dir.join("scenes/outro.json").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
