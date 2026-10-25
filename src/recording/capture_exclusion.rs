//! A recording-scoped request for the GNOME helper to omit the prompter
//! window from offscreen capture paints while preserving its desktop paint.

use super::input::monotonic_ns;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_HEARTBEAT_AGE_US: u64 = 2_000_000;
const HELPER_UNAVAILABLE: &str = "Teleprompter protection is not ready. Enable Lahza Capture Helper; after installing or updating it, log out and back in, then retry.";

/// No DMA-BUF caps means no SPA video modifier: Mutter selects MemFd buffers
/// and repaints offscreen, allowing the helper to omit the prompt.
pub(super) const CAPTURE_MEMORY_CAPS: &str =
    "video/x-raw(memory:SystemMemory),format=(string){BGRA,RGBA,BGRx,RGBx}";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    id: &'a str,
    window_title: &'a str,
    pid: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Acknowledgement {
    id: String,
    state: String,
    heartbeat_us: u64,
}

#[derive(Debug)]
struct ExclusionRequest {
    id: String,
    title: String,
    request_path: PathBuf,
    ready_path: PathBuf,
    protected_once: AtomicBool,
}

impl Drop for ExclusionRequest {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.request_path);
        let _ = fs::remove_file(&self.ready_path);
    }
}

/// Clones keep one request alive across the prompter window, the recorder and
/// pause/restart operations. The final owner removes its request and reply.
#[derive(Clone, Debug)]
pub struct CaptureExclusion {
    inner: Arc<ExclusionRequest>,
}

impl PartialEq for CaptureExclusion {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CaptureExclusion {}

impl CaptureExclusion {
    pub fn new() -> Result<Self, String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
        Self::new_in(&runtime.join("lahza-prompter"))
    }

    pub(super) fn new_in(directory: &Path) -> Result<Self, String> {
        let create = || -> std::io::Result<Self> {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
            let id = uuid::Uuid::new_v4().simple().to_string();
            let title = format!("Lahza Teleprompter {id}");
            let request_path = directory.join(format!("{id}.json"));
            let temporary = directory.join(format!("{id}.tmp"));
            let ready_path = directory.join(format!("{id}.ready.json"));
            let request = Request {
                id: &id,
                window_title: &title,
                pid: std::process::id(),
            };
            let bytes = serde_json::to_vec(&request).map_err(std::io::Error::other)?;
            let write = || -> std::io::Result<()> {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temporary)?;
                file.write_all(&bytes)?;
                drop(file);
                fs::rename(&temporary, &request_path)
            };
            if let Err(error) = write() {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
            Ok(Self {
                inner: Arc::new(ExclusionRequest {
                    id,
                    title,
                    request_path,
                    ready_path,
                    protected_once: AtomicBool::new(false),
                }),
            })
        };
        create()
            .map_err(|error| format!("Could not request teleprompter capture exclusion: {error}"))
    }

    pub fn title(&self) -> &str {
        &self.inner.title
    }

    /// An armed helper has not necessarily found the window yet. Only a fresh
    /// protected acknowledgement for this exact request permits capture.
    pub fn is_protected(&self) -> bool {
        let Some(ack) = fs::read(&self.inner.ready_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Acknowledgement>(&bytes).ok())
        else {
            return false;
        };
        let protected = ack.id == self.inner.id
            && ack.state == "protected"
            && (monotonic_ns() / 1_000)
                .checked_sub(ack.heartbeat_us)
                .is_some_and(|age| age <= MAX_HEARTBEAT_AGE_US);
        if protected {
            self.inner.protected_once.store(true, Ordering::Relaxed);
        }
        protected
    }

    /// Shared with the encoder so rebuilding the script view cannot forget
    /// that a missing helper heartbeat means protection has been lost.
    pub fn has_been_protected(&self) -> bool {
        self.inner.protected_once.load(Ordering::Relaxed)
    }

    pub fn wait_until_protected(&self, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if self.is_protected() {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                return Err(HELPER_UNAVAILABLE.into());
            }
            thread::sleep(Duration::from_millis(20).min(timeout.saturating_sub(started.elapsed())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> (PathBuf, CaptureExclusion) {
        let root = std::env::temp_dir().join(format!("lahza-exclusion-{}", uuid::Uuid::new_v4()));
        let request = CaptureExclusion::new_in(&root).unwrap();
        (root, request)
    }

    fn acknowledge(request: &CaptureExclusion, id: &str, state: &str, heartbeat_us: u64) {
        fs::write(
            &request.inner.ready_path,
            serde_json::to_vec(&serde_json::json!({
                "id": id, "state": state, "heartbeatUs": heartbeat_us,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn capture_requires_a_fresh_protected_acknowledgement_for_this_window() {
        let (root, request) = request();
        assert!(!request.is_protected());
        assert!(!request.has_been_protected());
        for (id, state, age) in [
            ("another-request", "protected", 0),
            (request.inner.id.as_str(), "armed", 0),
            (
                request.inner.id.as_str(),
                "protected",
                MAX_HEARTBEAT_AGE_US + 1,
            ),
        ] {
            acknowledge(
                &request,
                id,
                state,
                (monotonic_ns() / 1_000).saturating_sub(age),
            );
            assert!(!request.is_protected());
        }
        acknowledge(&request, &request.inner.id, "protected", u64::MAX);
        assert!(
            !request.is_protected(),
            "future heartbeats must not be accepted"
        );
        assert!(request.wait_until_protected(Duration::ZERO).is_err());
        acknowledge(
            &request,
            &request.inner.id,
            "protected",
            monotonic_ns() / 1_000,
        );
        assert!(request.wait_until_protected(Duration::ZERO).is_ok());
        fs::write(&request.inner.ready_path, b"partial JSON").unwrap();
        assert!(!request.is_protected());
        assert!(request.clone().has_been_protected());
        drop(request);
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn requests_are_private_and_live_until_the_final_owner_drops() {
        let (root, request) = request();
        let path = request.inner.request_path.clone();
        let ready_path = request.inner.ready_path.clone();
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["windowTitle"], request.title());
        assert_eq!(json["pid"], std::process::id());
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        acknowledge(
            &request,
            &request.inner.id,
            "protected",
            monotonic_ns() / 1_000,
        );
        let retained = request.clone();
        assert_eq!(retained, request);
        drop(request);
        assert!(path.exists());
        assert!(retained.is_protected());
        drop(retained);
        assert!(!path.exists());
        assert!(!ready_path.exists());
        fs::remove_dir(root).unwrap();
    }
}
