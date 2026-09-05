use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn screenshots_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let contents = fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    pictures_dir(&home, &contents).join("Screenshots")
}

fn pictures_dir(home: &Path, contents: &str) -> PathBuf {
    for line in contents.lines() {
        let Some(value) = line.trim().strip_prefix("XDG_PICTURES_DIR=") else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if let Some(rest) = value.strip_prefix("$HOME/") {
            return home.join(rest);
        }
        if value.starts_with('/') {
            return PathBuf::from(value);
        }
    }
    home.join("Pictures")
}

/// Discover saved items without descending into recording bundles or symlinks.
/// Return every item, newest first; the launcher provides scrolling.
pub fn saved_items(root: &Path, projects: bool) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut items = Vec::new();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if kind.is_dir() {
                if extension == "screendroprec" {
                    if projects {
                        items.push(path);
                    }
                } else if !entry.file_name().to_string_lossy().starts_with('.') {
                    pending.push(path);
                }
            } else if !projects
                && kind.is_file()
                && matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
            {
                items.push(path);
            }
        }
    }
    items.sort_by_cached_key(|path| {
        (
            std::cmp::Reverse(fs::metadata(path).and_then(|m| m.modified()).ok()),
            path.clone(),
        )
    });
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_custom_picture_directories() {
        let home = Path::new("/home/test");
        assert_eq!(
            pictures_dir(home, "XDG_PICTURES_DIR=\"$HOME/My Pictures\""),
            home.join("My Pictures")
        );
        assert_eq!(
            pictures_dir(home, "XDG_PICTURES_DIR=\"/media/photos\""),
            PathBuf::from("/media/photos")
        );
        assert_eq!(pictures_dir(home, ""), home.join("Pictures"));
    }

    #[test]
    fn discovers_nested_items_and_skips_project_contents_without_truncating() {
        let root = std::env::temp_dir().join(format!("lahza-library-test-{}", std::process::id()));
        fs::create_dir_all(root.join("Screenshots/nested")).unwrap();
        fs::create_dir_all(root.join("Videos/test.screendroprec")).unwrap();
        for index in 0..12 {
            fs::write(
                root.join(format!("Screenshots/nested/{index}.PNG")),
                b"image",
            )
            .unwrap();
        }
        fs::write(
            root.join("Videos/test.screendroprec/thumbnail.png"),
            b"preview",
        )
        .unwrap();
        fs::write(root.join("not-a-project.screendroprec"), b"file").unwrap();
        assert_eq!(saved_items(&root, false).len(), 12);
        assert_eq!(
            saved_items(&root, true),
            vec![root.join("Videos/test.screendroprec")]
        );
        assert!(saved_items(&root.join("missing"), false).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
