//! Recording-scoped requests for the GNOME helper to omit floating windows
//! from offscreen capture paints while preserving their desktop paint.

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
const HELPER_UNAVAILABLE: &str = "Recording overlay protection is not ready. Enable Lahza Capture Helper; after installing or updating it, log out and back in, then retry.";

/// No DMA-BUF caps means no SPA video modifier: Mutter selects MemFd buffers
/// and repaints offscreen, allowing the helper to omit protected windows.
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
    window_closed: AtomicBool,
}

impl Drop for ExclusionRequest {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.request_path);
        let _ = fs::remove_file(&self.ready_path);
    }
}

/// Clones keep one request alive across its floating window, the recorder and
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
        Self::for_window("Teleprompter")
    }

    /// Each native window needs its own request and exact, unique title.
    pub fn for_window(label: &str) -> Result<Self, String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
        Self::new_in_for_window(&runtime.join("lahza-prompter"), label)
    }

    #[cfg(test)]
    pub(super) fn new_in(directory: &Path) -> Result<Self, String> {
        Self::new_in_for_window(directory, "Teleprompter")
    }

    fn new_in_for_window(directory: &Path, label: &str) -> Result<Self, String> {
        let create = || -> std::io::Result<Self> {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
            let id = uuid::Uuid::new_v4().simple().to_string();
            let title = format!("Lahza {label} {id}");
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
                    window_closed: AtomicBool::new(false),
                }),
            })
        };
        create().map_err(|error| format!("Could not request recording overlay protection: {error}"))
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

    /// Retire an optional overlay only AFTER its native window is removed.
    /// Never call this for a visible, minimized, or merely hidden window: it
    /// exempts this request from future start/resume protection checks.
    /// Clones share retirement, while the request and SystemMemory requirement
    /// remain alive until the recording releases them. The helper retains its
    /// exclusion effect until the compositor destroys the window actor.
    pub fn retire_closed_window(&self) {
        self.inner.window_closed.store(true, Ordering::Release);
    }

    /// Recheck the entire set together so an earlier window cannot become
    /// stale while a later window is still waiting for its acknowledgement.
    pub fn wait_until_all_protected(exclusions: &[Self], timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if exclusions.iter().all(|exclusion| {
                exclusion.inner.window_closed.load(Ordering::Acquire) || exclusion.is_protected()
            }) {
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
        assert!(CaptureExclusion::wait_until_all_protected(
            std::slice::from_ref(&request),
            Duration::ZERO,
        )
        .is_err());
        acknowledge(
            &request,
            &request.inner.id,
            "protected",
            monotonic_ns() / 1_000,
        );
        assert!(CaptureExclusion::wait_until_all_protected(
            std::slice::from_ref(&request),
            Duration::ZERO,
        )
        .is_ok());
        fs::write(&request.inner.ready_path, b"partial JSON").unwrap();
        assert!(!request.is_protected());
        assert!(request.clone().has_been_protected());
        drop(request);
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn capture_requires_every_window_to_remain_freshly_protected() {
        let root =
            std::env::temp_dir().join(format!("lahza-exclusion-set-{}", uuid::Uuid::new_v4()));
        let requests: Vec<_> = ["Teleprompter", "Recording controls", "Camera preview"]
            .into_iter()
            .map(|label| CaptureExclusion::new_in_for_window(&root, label).unwrap())
            .collect();
        assert!(CaptureExclusion::wait_until_all_protected(&[], Duration::ZERO).is_ok());
        for (index, request) in requests.iter().enumerate() {
            acknowledge(
                request,
                &request.inner.id,
                "protected",
                monotonic_ns() / 1_000,
            );
            assert_eq!(
                CaptureExclusion::wait_until_all_protected(&requests, Duration::ZERO).is_ok(),
                index == requests.len() - 1,
                "every window must have its own protected acknowledgement",
            );
        }
        for request in &requests {
            acknowledge(
                request,
                &request.inner.id,
                "protected",
                (monotonic_ns() / 1_000).saturating_sub(MAX_HEARTBEAT_AGE_US + 1),
            );
            assert!(request.has_been_protected());
            assert!(CaptureExclusion::wait_until_all_protected(&requests, Duration::ZERO).is_err());
            acknowledge(
                request,
                &request.inner.id,
                "protected",
                monotonic_ns() / 1_000,
            );
        }
        assert!(CaptureExclusion::wait_until_all_protected(&requests, Duration::ZERO).is_ok());
        drop(requests);
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn floating_windows_have_distinct_titles_and_independent_request_lifetimes() {
        let root =
            std::env::temp_dir().join(format!("lahza-exclusion-windows-{}", uuid::Uuid::new_v4()));
        let prompter = CaptureExclusion::new_in(&root).unwrap();
        let controls = CaptureExclusion::new_in_for_window(&root, "Recording controls").unwrap();
        let camera = CaptureExclusion::new_in_for_window(&root, "Camera preview").unwrap();
        let other_camera = CaptureExclusion::new_in_for_window(&root, "Camera preview").unwrap();
        assert!(prompter.title().starts_with("Lahza Teleprompter "));
        assert!(controls.title().starts_with("Lahza Recording controls "));
        assert!(camera.title().starts_with("Lahza Camera preview "));
        assert_ne!(camera.title(), other_camera.title());
        let paths: Vec<_> = [&prompter, &controls, &camera]
            .iter()
            .map(|request| request.inner.request_path.clone())
            .collect();
        let retained = vec![prompter.clone(), controls.clone(), camera.clone()];
        drop((prompter, controls, camera, other_camera));
        assert!(paths.iter().all(|path| path.exists()));
        drop(retained);
        assert!(paths.iter().all(|path| !path.exists()));
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn closed_optional_windows_do_not_block_resume_or_exempt_active_windows() {
        let root =
            std::env::temp_dir().join(format!("lahza-exclusion-retired-{}", uuid::Uuid::new_v4()));
        let active = CaptureExclusion::new_in(&root).unwrap();
        let closed = CaptureExclusion::new_in_for_window(&root, "Recording controls").unwrap();
        let closed_request_path = closed.inner.request_path.clone();
        let recording = vec![active.clone(), closed.clone()];
        closed.retire_closed_window();
        drop(closed);
        assert!(
            closed_request_path.exists(),
            "retirement must retain the helper request"
        );
        assert!(CaptureExclusion::wait_until_all_protected(&recording, Duration::ZERO).is_err());
        acknowledge(
            &active,
            &active.inner.id,
            "protected",
            monotonic_ns() / 1_000,
        );
        assert!(CaptureExclusion::wait_until_all_protected(&recording, Duration::ZERO).is_ok());
        fs::remove_file(&active.inner.ready_path).unwrap();
        assert!(CaptureExclusion::wait_until_all_protected(&recording, Duration::ZERO).is_err());
        assert!(
            !recording[1].is_protected(),
            "retirement is not a protected acknowledgement"
        );
        drop((active, recording));
        assert!(!closed_request_path.exists());
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
