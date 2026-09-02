//! Saved scene presets: a coordinated background, layout, border, shadow,
//! pointer look, and default zoom strength that applies to screenshots and
//! recordings alike. Stored as JSON in the user's config directory.

use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

use super::scene::SceneStyle;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScenePreset {
    pub name: String,
    pub style: SceneStyle,
    /// Magnification used for new motion regions.
    pub default_zoom: f64,
    /// Aspect preset index in the inspector (0 = auto).
    pub aspect_index: usize,
}

impl Default for ScenePreset {
    fn default() -> Self {
        Self {
            name: "Preset".into(),
            style: SceneStyle::default(),
            default_zoom: 2.0,
            aspect_index: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PresetLibrary {
    pub presets: Vec<ScenePreset>,
}

impl PresetLibrary {
    pub const FILE_NAME: &'static str = "presets.json";

    pub fn path() -> PathBuf {
        config_root().join(Self::FILE_NAME)
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }

    /// Adds a preset with a unique name derived from `base`.
    pub fn add(&mut self, mut preset: ScenePreset) -> usize {
        let base = if preset.name.trim().is_empty() {
            "My preset".to_string()
        } else {
            preset.name.trim().to_string()
        };
        let mut name = base.clone();
        let mut counter = 2;
        while self.presets.iter().any(|existing| existing.name == name) {
            name = format!("{base} {counter}");
            counter += 1;
        }
        preset.name = name;
        self.presets.push(preset);
        self.presets.len() - 1
    }

    pub fn remove(&mut self, index: usize) -> Option<ScenePreset> {
        (index < self.presets.len()).then(|| self.presets.remove(index))
    }
}

fn config_root() -> PathBuf {
    if let Some(root) = std::env::var_os("SCREENDROP_CONFIG_DIR") {
        return PathBuf::from(root);
    }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(root).join("screendrop");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".config").join("screendrop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_round_trips_and_names_presets_uniquely() {
        let root =
            std::env::temp_dir().join(format!("screendrop-presets-{}", uuid::Uuid::new_v4()));
        let path = root.join("nested").join(PresetLibrary::FILE_NAME);
        let mut library = PresetLibrary::default();
        let first = library.add(ScenePreset {
            name: "Studio".into(),
            ..ScenePreset::default()
        });
        let second = library.add(ScenePreset {
            name: "Studio".into(),
            default_zoom: 1.5,
            ..ScenePreset::default()
        });
        assert_eq!(library.presets[first].name, "Studio");
        assert_eq!(library.presets[second].name, "Studio 2");
        library.save_to(&path).unwrap();
        let loaded = PresetLibrary::load_from(&path);
        assert_eq!(loaded, library);
        assert!(PresetLibrary::load_from(&root.join("missing.json"))
            .presets
            .is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
