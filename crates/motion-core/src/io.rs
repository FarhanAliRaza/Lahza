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
    project.shaders = crate::slang::load(dir).map_err(|e| err(&dir.join("shaders"), e))?;
    project.bakes = crate::sim::load_bakes(dir);
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
    /// The request this edit belongs to (one prompt to an agent makes
    /// many edits): undone and put back together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}

/// A project folder held for one read-change-write. The editor and an
/// agent's server each keep the project in memory and number history
/// entries from what is on disk: whoever writes takes this first, loads
/// what the other wrote, then writes. Released when dropped, or when the
/// process ends (`flock` on `<project>/.lock`).
pub struct Lock(#[allow(dead_code)] fs::File);

/// Waits for the folder's lock.
pub fn lock(dir: &Path) -> Result<Lock> {
    let path = dir.join(".lock");
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(|e| err(&path, e))?;
    file.lock().map_err(|e| err(&path, e))?;
    Ok(Lock(file))
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

/// One past the highest entry: from the file names (`000042.json`), so no
/// entry is read.
pub fn next_history_number(dir: &Path) -> Result<u64> {
    let mut last = 0;
    if let Ok(read) = fs::read_dir(history_dir(dir)) {
        for e in read.flatten() {
            let name = e.file_name();
            let n = name.to_str().and_then(|n| n.strip_suffix(".json")).and_then(|n| n.parse::<u64>().ok());
            last = last.max(n.unwrap_or(0));
        }
    }
    Ok(last + 1)
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

    fn entry(n: u64) -> HistoryEntry {
        HistoryEntry { n, time: 0, author: "agent".into(), summary: vec!["set a.b".into()], ops: vec![serde_json::json!({"op": "set", "target": "a.b", "value": 1})], inverse: Vec::new(), undone: false, undoes: Vec::new(), request: None }
    }

    /// The next number comes from the file names; prints what reading every
    /// entry costs beside it (`-- --nocapture`).
    #[test]
    fn history_numbers_come_from_file_names() {
        let dir = std::env::temp_dir().join(format!("lahza-motion-history-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(next_history_number(&dir).unwrap(), 1);
        for n in 1..=2000 {
            write_history(&dir, &entry(n)).unwrap();
        }
        fs::write(dir.join("history/notes.json"), "not an entry's name").unwrap();
        let t = std::time::Instant::now();
        assert_eq!(next_history_number(&dir).unwrap(), 2001);
        let names = t.elapsed();
        fs::remove_file(dir.join("history/notes.json")).unwrap();
        let t = std::time::Instant::now();
        assert_eq!(read_history(&dir).unwrap().len(), 2000);
        println!("2000 entries: next number {:.2} ms from names, {:.2} ms reading them all", names.as_secs_f64() * 1000.0, t.elapsed().as_secs_f64() * 1000.0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_folder_lock_is_held_by_one_at_a_time() {
        let dir = std::env::temp_dir().join(format!("lahza-motion-lock-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let held = lock(&dir).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let other = dir.clone();
        let waiter = std::thread::spawn(move || {
            let _lock = lock(&other).unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(std::time::Duration::from_millis(150)).is_err(), "the second waits");
        drop(held);
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("and gets it when the first lets go");
        waiter.join().unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

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
