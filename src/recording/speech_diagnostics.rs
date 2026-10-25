//! Durable text diagnostics for each voice-follow session, including desktop
//! launches whose stderr may not be connected to the development launcher.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const RETAINED_LOGS: usize = 20;

pub struct VoiceLog {
    path: Option<PathBuf>,
    file: Mutex<Option<File>>,
    started: Instant,
}

impl VoiceLog {
    /// Test fixtures can exercise the follower without creating user logs.
    pub fn disabled() -> Arc<Self> {
        Arc::new(Self {
            path: None,
            file: Mutex::new(None),
            started: Instant::now(),
        })
    }

    /// Logging is best effort: an unwritable state directory must not prevent
    /// recording or speech recognition from starting.
    pub fn new() -> Arc<Self> {
        let directory = state_directory();
        Arc::new(Self::create(&directory).unwrap_or_else(|error| {
            eprintln!(
                "[voice-follow] could not save diagnostics in {}: {error}",
                directory.display()
            );
            Self {
                path: None,
                file: Mutex::new(None),
                started: Instant::now(),
            }
        }))
    }

    fn create(directory: &Path) -> io::Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let now = chrono::Utc::now();
        let path = directory.join(format!(
            "voice-{}-{}.log",
            now.format("%Y%m%dT%H%M%S%.3fZ"),
            uuid::Uuid::new_v4(),
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let log = Self {
            path: Some(path),
            file: Mutex::new(Some(file)),
            started: Instant::now(),
        };
        log.write(&format!(
            "Voice-follow session started at {}",
            now.to_rfc3339()
        ));
        prune_logs(directory, RETAINED_LOGS);
        Ok(log)
    }

    #[cfg(test)]
    pub(super) fn in_directory(directory: &Path) -> io::Result<Arc<Self>> {
        Self::create(directory).map(Arc::new)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Each call is written immediately, so logs survive a stopped recognizer,
    /// app crash, or restart. Audio is never written here.
    pub fn write(&self, message: &str) {
        let mut guard = self.file.lock().unwrap_or_else(|error| error.into_inner());
        let Some(file) = guard.as_mut() else { return };
        let elapsed = self.started.elapsed().as_secs_f64();
        let result = (|| -> io::Result<()> {
            for line in message.lines() {
                writeln!(file, "[+{elapsed:010.3}s] {line}")?;
            }
            file.flush()
        })();
        if let Err(error) = result {
            eprintln!("[voice-follow] could not write saved diagnostics: {error}");
            // Report once; a full disk must not flood stderr or stop following.
            *guard = None;
        }
    }
}

fn state_directory() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir)
        .join("lahza/voice-follow")
}

fn prune_logs(directory: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut logs: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            (name.starts_with("voice-")
                && name.ends_with(".log")
                && entry.file_type().is_ok_and(|kind| kind.is_file()))
            .then(|| entry.path())
        })
        .collect();
    // Names start with UTC timestamps, so active writes do not change which
    // sessions count as the latest ones.
    logs.sort();
    let remove = logs.len().saturating_sub(keep);
    for path in logs.into_iter().take(remove) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("lahza-voice-logs-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sessions_keep_separate_private_logs_and_flush_before_drop() {
        let root = Scratch::new();
        let directory = root.0.join("voice-follow");
        let first = VoiceLog::create(&directory).unwrap();
        first.write("hypothesis=hello matched=1\nbackend=Vulkan0");
        let second = VoiceLog::create(&directory).unwrap();
        second.write("hypothesis=world matched=2");
        let first_path = first.path().unwrap();
        let second_path = second.path().unwrap();
        assert_ne!(first_path, second_path);
        assert_eq!(
            fs::metadata(first_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let contents = fs::read_to_string(first_path).unwrap();
        assert!(contents.contains("session started at"));
        assert!(contents.contains("hypothesis=hello matched=1"));
        assert!(contents.contains("backend=Vulkan0"));
        assert!(contents
            .lines()
            .all(|line| line.starts_with("[+") && line.contains("s] ")));
        assert!(!contents.contains("hypothesis=world"));
    }

    #[test]
    fn retention_keeps_latest_twenty_sessions_and_leaves_other_files_alone() {
        let root = Scratch::new();
        for index in 0..25 {
            fs::write(
                root.0.join(format!("voice-20200101T{index:06}Z-test.log")),
                "old session",
            )
            .unwrap();
        }
        let notes = root.0.join("notes.log");
        fs::write(&notes, "keep").unwrap();
        let log = VoiceLog::create(&root.0).unwrap();
        let count = fs::read_dir(&root.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("voice-"))
            .count();
        assert_eq!(count, RETAINED_LOGS);
        assert!(!root.0.join("voice-20200101T000005Z-test.log").exists());
        assert!(root.0.join("voice-20200101T000006Z-test.log").exists());
        assert!(log.path().unwrap().is_file());
        assert!(notes.is_file());
    }
}
