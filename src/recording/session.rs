use super::{
    model::{CaptureManifest, PointerCaptureFile, RecordingSession},
    native::{RecordStatus, RecorderBackend, RecorderError},
    video::{probe_media, write_poster},
};
use std::{
    fmt, fs, io,
    path::Path,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecordingState {
    #[default]
    Idle,
    Starting,
    Recording,
    Paused,
    Finishing,
}

#[derive(Debug)]
pub enum RecordingError {
    InvalidTransition {
        state: RecordingState,
        action: &'static str,
    },
    Backend(RecorderError),
    Io(io::Error),
    UnusableOutput(String),
}

impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { state, action } => {
                write!(f, "cannot {action} while recording is {state:?}")
            }
            Self::Backend(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
            Self::UnusableOutput(path) => {
                write!(f, "the encoder produced no usable recording at {path}")
            }
        }
    }
}

impl std::error::Error for RecordingError {}

impl From<RecorderError> for RecordingError {
    fn from(value: RecorderError) -> Self {
        Self::Backend(value)
    }
}

impl From<io::Error> for RecordingError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct RecordingController<B: RecorderBackend> {
    backend: B,
    state: RecordingState,
    session: Option<RecordingSession>,
    started_at: Option<Instant>,
    elapsed_before_pause_ms: u64,
    warnings: Vec<String>,
}

impl<B: RecorderBackend> RecordingController<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            state: RecordingState::Idle,
            session: None,
            started_at: None,
            elapsed_before_pause_ms: 0,
            warnings: Vec::new(),
        }
    }

    pub fn state(&self) -> RecordingState {
        self.state
    }

    pub fn session(&self) -> Option<&RecordingSession> {
        self.session.as_ref()
    }

    pub fn elapsed_ms(&self) -> u64 {
        match (self.state, self.started_at) {
            (RecordingState::Recording, Some(started_at)) => self
                .elapsed_before_pause_ms
                .saturating_add(started_at.elapsed().as_millis() as u64),
            _ => self.elapsed_before_pause_ms,
        }
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    pub fn start(&mut self) -> Result<&RecordingSession, RecordingError> {
        self.require(RecordingState::Idle, "start")?;
        let session = match RecordingSession::create() {
            Ok(session) => session,
            Err(error) => return Err(error.into()),
        };
        self.start_with_session(session)
    }

    fn start_with_session(
        &mut self,
        session: RecordingSession,
    ) -> Result<&RecordingSession, RecordingError> {
        self.require(RecordingState::Idle, "start")?;
        self.state = RecordingState::Starting;
        self.session = Some(session);
        let session_directory = self
            .session
            .as_ref()
            .expect("session was just created")
            .directory
            .clone();
        let mut backend_started = false;
        let result = (|| {
            self.backend
                .start_recording(&self.session.as_ref().expect("session exists").screen_path())?;
            backend_started = true;
            if !self.wait_until_active(Duration::from_secs(5))? {
                return Err(RecordingError::UnusableOutput(
                    session_directory.display().to_string(),
                ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            if backend_started {
                // Finalize before removing the package. The encoder may have
                // accepted the stream even if its active status never settled.
                let _ = self.backend.stop_recording();
            }
            if let Some(session) = self.session.take() {
                let _ = fs::remove_dir_all(&session.directory);
            }
            self.state = RecordingState::Idle;
            return Err(error);
        }

        self.started_at = Some(Instant::now());
        self.elapsed_before_pause_ms = 0;
        self.state = RecordingState::Recording;
        Ok(self.session.as_ref().expect("active session exists"))
    }

    pub fn pause(&mut self) -> Result<(), RecordingError> {
        self.require(RecordingState::Recording, "pause")?;
        self.backend.pause_recording()?;
        if !self.wait_for_status(Duration::from_secs(5), |status| {
            status.active && status.paused
        })? {
            return Err(RecordingError::Backend(RecorderError::State(
                "the recorder did not enter the paused state within 5 seconds".into(),
            )));
        }
        self.elapsed_before_pause_ms = self
            .backend
            .record_status()
            .ok()
            .filter(|status| status.active)
            .map(|status| status.duration_ms)
            .unwrap_or_else(|| self.elapsed_ms());
        self.started_at = None;
        self.state = RecordingState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), RecordingError> {
        self.require(RecordingState::Paused, "resume")?;
        self.backend.resume_recording()?;
        if !self.wait_for_status(Duration::from_secs(5), |status| {
            status.active && !status.paused
        })? {
            return Err(RecordingError::Backend(RecorderError::State(
                "the recorder did not resume within 5 seconds".into(),
            )));
        }
        self.started_at = Some(Instant::now());
        self.state = RecordingState::Recording;
        Ok(())
    }

    pub fn status(&mut self) -> Result<RecordStatus, RecordingError> {
        Ok(self.backend.record_status()?)
    }

    /// Stops the encoder and preserves the exact output it reports. If finalization or
    /// import fails, the session remains discoverable for recovery.
    pub fn stop_and_save(&mut self) -> Result<RecordingSession, RecordingError> {
        if !matches!(
            self.state,
            RecordingState::Recording | RecordingState::Paused
        ) {
            return Err(RecordingError::InvalidTransition {
                state: self.state,
                action: "stop",
            });
        }
        let previous_state = self.state;
        self.elapsed_before_pause_ms = self
            .backend
            .record_status()
            .ok()
            .filter(|status| status.active)
            .map(|status| status.duration_ms)
            .unwrap_or_else(|| self.elapsed_ms());
        self.started_at = None;
        self.state = RecordingState::Finishing;

        let output = match self.backend.stop_recording() {
            Ok(output) => output,
            Err(error) => {
                self.state = previous_state;
                if previous_state == RecordingState::Recording {
                    self.started_at = Some(Instant::now());
                }
                return Err(error.into());
            }
        };
        if !self.wait_for_status(Duration::from_secs(5), |status| !status.active)? {
            self.warnings.push(
                "The encoder returned the recording path but did not report a fully stopped output within 5 seconds."
                    .into(),
            );
        }
        let source = output.as_path();
        let output_is_usable = wait_for_usable_output(source, Duration::from_secs(5));
        if !output_is_usable {
            return Err(RecordingError::UnusableOutput(output.display().to_string()));
        }

        let session = self
            .session
            .as_ref()
            .expect("active recording has a session");
        install_output(source, &session.screen_path())?;
        let mut manifest = CaptureManifest::default();
        manifest.recording_backend = super::model::RecordingBackend::PipeWire;
        manifest.press_effects_baked = false;
        manifest.includes_system_audio = self.backend.includes_system_audio();
        manifest.includes_microphone = self.backend.includes_microphone();
        manifest.pointer_synthesized = self.backend.pointer_synthesized();
        if let Some(probe) = probe_media_with_retry(&session.screen_path(), Duration::from_secs(5))
        {
            manifest.duration = probe.duration;
            manifest.pixel_width = probe.width;
            manifest.pixel_height = probe.height;
            if let Err(error) =
                write_poster(&session.screen_path(), &session.poster_path(), 1280, 720)
            {
                self.warnings.push(format!(
                    "The recording was saved, but its preview poster could not be created: {error}"
                ));
            }
        } else {
            manifest.duration = self.elapsed_before_pause_ms as f64 / 1000.0;
            self.warnings.push(
                "Could not inspect the finalized recording; duration and dimensions may be approximate."
                    .into(),
            );
        }
        session.write_manifest(&manifest)?;
        if !session.pointer_path().exists() {
            session.write_pointer_capture(&PointerCaptureFile::default())?;
        }

        let session = self.session.take().expect("active session exists");
        self.state = RecordingState::Idle;
        self.elapsed_before_pause_ms = 0;
        Ok(session)
    }

    /// Matches Swift's discard invariant: the encoder must first finalize a usable
    /// recording. Only then is the project package removed.
    pub fn discard(&mut self) -> Result<(), RecordingError> {
        let session = self.stop_and_save()?;
        fs::remove_dir_all(session.directory)?;
        Ok(())
    }

    /// Finalizes the current clip before deleting it, then starts a fresh
    /// session through the full setup path. A failed finalization preserves
    /// the original project instead of silently losing it.
    pub fn restart(&mut self) -> Result<&RecordingSession, RecordingError> {
        let session = self.stop_and_save()?;
        fs::remove_dir_all(session.directory)?;
        self.start()
    }

    fn require(
        &self,
        expected: RecordingState,
        action: &'static str,
    ) -> Result<(), RecordingError> {
        if self.state == expected {
            Ok(())
        } else {
            Err(RecordingError::InvalidTransition {
                state: self.state,
                action,
            })
        }
    }

    fn wait_until_active(&mut self, timeout: Duration) -> Result<bool, RecordingError> {
        self.wait_for_status(timeout, |status| status.active)
    }

    fn wait_for_status(
        &mut self,
        timeout: Duration,
        predicate: impl Fn(&RecordStatus) -> bool,
    ) -> Result<bool, RecordingError> {
        let deadline = Instant::now() + timeout;
        loop {
            if predicate(&self.backend.record_status()?) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(40));
        }
    }
}

fn install_output(source: &Path, destination: &Path) -> io::Result<()> {
    if source == destination {
        return Ok(());
    }
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(18) => {
            fs::copy(source, destination)?;
            fs::remove_file(source)
        }
        Err(error) => Err(error),
    }
}

fn probe_media_with_retry(path: &Path, timeout: Duration) -> Option<super::video::MediaInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(info) = probe_media(path) {
            return Some(info);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(80));
    }
}

fn wait_for_usable_output(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut previous_size = 0;
    let mut stable_observations = 0;
    loop {
        let size = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        if size > 0 && size == previous_size {
            stable_observations += 1;
            if stable_observations >= 2 {
                return true;
            }
        } else {
            stable_observations = 0;
            previous_size = size;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct FakeBackend {
        active: bool,
        paused: bool,
        output: PathBuf,
    }

    impl RecorderBackend for FakeBackend {
        fn record_status(&mut self) -> Result<RecordStatus, RecorderError> {
            Ok(RecordStatus {
                active: self.active,
                paused: self.paused,
                timecode: "00:00:01.250".into(),
                duration_ms: 1_250,
                output_bytes: 4,
            })
        }

        fn start_recording(&mut self, _output: &Path) -> Result<(), RecorderError> {
            self.active = true;
            Ok(())
        }

        fn pause_recording(&mut self) -> Result<(), RecorderError> {
            self.paused = true;
            Ok(())
        }

        fn resume_recording(&mut self) -> Result<(), RecorderError> {
            self.paused = false;
            Ok(())
        }

        fn stop_recording(&mut self) -> Result<PathBuf, RecorderError> {
            self.active = false;
            Ok(self.output.clone())
        }
    }

    #[test]
    fn state_machine_imports_the_exact_backend_output() {
        let output =
            std::env::temp_dir().join(format!("screendrop-test-{}.mkv", uuid::Uuid::new_v4()));
        fs::write(&output, b"video").unwrap();
        let backend = FakeBackend {
            active: false,
            paused: false,
            output,
        };
        let mut controller = RecordingController::new(backend);
        let root = std::env::temp_dir().join(format!(
            "screendrop-recording-test-{}",
            uuid::Uuid::new_v4()
        ));
        let pending_session = RecordingSession::create_in(&root).unwrap();

        controller.start_with_session(pending_session).unwrap();
        assert_eq!(controller.state(), RecordingState::Recording);
        controller.pause().unwrap();
        assert_eq!(controller.state(), RecordingState::Paused);
        controller.resume().unwrap();
        let session = controller.stop_and_save().unwrap();

        assert_eq!(controller.state(), RecordingState::Idle);
        assert_eq!(fs::read(session.screen_path()).unwrap(), b"video");
        assert_eq!(session.read_manifest().unwrap().duration, 1.25);
        assert_eq!(
            session.read_pointer_capture().unwrap(),
            PointerCaptureFile::default()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_transitions_do_not_call_backend() {
        let backend = FakeBackend {
            active: false,
            paused: false,
            output: PathBuf::new(),
        };
        let mut controller = RecordingController::new(backend);
        assert!(matches!(
            controller.pause(),
            Err(RecordingError::InvalidTransition { .. })
        ));
        assert_eq!(controller.state(), RecordingState::Idle);
    }

    #[test]
    fn discard_finalizes_before_removing_the_project() {
        let output =
            std::env::temp_dir().join(format!("screendrop-test-{}.mkv", uuid::Uuid::new_v4()));
        fs::write(&output, b"video").unwrap();
        let backend = FakeBackend {
            active: false,
            paused: false,
            output,
        };
        let mut controller = RecordingController::new(backend);
        let root =
            std::env::temp_dir().join(format!("screendrop-discard-test-{}", uuid::Uuid::new_v4()));
        let pending_session = RecordingSession::create_in(&root).unwrap();
        let project = pending_session.directory.clone();

        controller.start_with_session(pending_session).unwrap();
        controller.discard().unwrap();

        assert_eq!(controller.state(), RecordingState::Idle);
        assert!(!project.exists());
        fs::remove_dir(root).unwrap();
    }
}
