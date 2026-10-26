//! Source-time audio preparation shared by playback and export.
//!
//! New captures retain microphone and desktop streams. Only the microphone is
//! cleaned; their lossless prepared mix is reused before any cuts or speed
//! changes. Legacy recordings retain their original first-audio-stream behavior.
use super::{audio_mode::AudioMode, media_bank::MediaBank, video::VideoError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc, Mutex},
    thread,
    time::{Duration, UNIX_EPOCH},
};

// Bump whenever selection, DSP, sample format, alignment, or model changes.
const PIPELINE_VERSION: &str = "source-audio-v3-neural-flac24-48k";

fn processing_identity(mode: AudioMode) -> String {
    let model = match mode {
        AudioMode::NoiseReduction => super::neural_audio::MODEL_ID,
        AudioMode::Podcast => super::restoration_audio::MODEL_ID,
        _ => "none",
    };
    format!("{}:{}:{}", mode.cache_key(), super::audio_dsp::DSP_VERSION, model)
}

#[derive(Clone, Debug)]
pub struct ProcessingControl {
    cancelled: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    status: Arc<Mutex<String>>,
    done: Arc<AtomicBool>,
}

impl Default for ProcessingControl {
    fn default() -> Self { Self::with_cancel_flag(Arc::new(AtomicBool::new(false))) }
}

impl ProcessingControl {
    pub fn with_cancel_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled, progress: Arc::new(AtomicU64::new(0)),
            status: Arc::new(Mutex::new("Preparing audio".into())),
            done: Arc::new(AtomicBool::new(false)) }
    }
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::Release); }
    pub fn is_cancelled(&self) -> bool { self.cancelled.load(Ordering::Acquire) }
    pub fn fraction(&self) -> f64 { self.progress.load(Ordering::Relaxed) as f64 / 10000.0 }
    pub fn progress(&self) -> u8 { (self.progress.load(Ordering::Relaxed) / 100).min(100) as u8 }
    pub fn status(&self) -> String { self.status.lock().unwrap().clone() }
    pub fn is_done(&self) -> bool { self.done.load(Ordering::Acquire) }
    fn set_progress(&self, fraction: f64) {
        self.progress.store((fraction.clamp(0.0, 1.0) * 10000.0) as u64, Ordering::Relaxed);
    }
    fn set_status(&self, status: &str) { *self.status.lock().unwrap() = status.into(); }
    fn check(&self, cancelled: &dyn Fn() -> bool) -> Result<(), VideoError> {
        if self.is_cancelled() || cancelled() { Err(VideoError::Cancelled) } else { Ok(()) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SourceIdentity {
    path: PathBuf,
    bytes: u64,
    modified_ns: u128,
    // Unix ctime catches replacing/editing a file while restoring its mtime.
    changed_seconds: i64,
    changed_nanos: i64,
    device: u64,
    inode: u64,
}

impl SourceIdentity {
    fn read(path: &Path) -> Result<Self, VideoError> {
        let path = fs::canonicalize(path)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() { return Err(VideoError::InvalidMedia("audio source is not a file".into())); }
        let modified_ns = metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        #[cfg(unix)]
        let (changed_seconds, changed_nanos, device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (metadata.ctime(), metadata.ctime_nsec(), metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let (changed_seconds, changed_nanos, device, inode) = (0, 0, 0, 0);
        Ok(Self { path, bytes: metadata.len(), modified_ns, changed_seconds, changed_nanos, device, inode })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Role { Microphone, System, Legacy }

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AudioTrack { index: u32, role: Role }

#[derive(Serialize, Deserialize)]
struct CacheRecord {
    version: String,
    processor: String,
    source: SourceIdentity,
    source_sha256: String,
    mode: AudioMode,
    // None records a verified source that can use its original embedded audio.
    audio_file: Option<String>,
    audio_bytes: u64,
    audio_sha256: Option<String>,
    audio_identity: Option<SourceIdentity>,
    tracks: Vec<AudioTrack>,
}

fn cache_directory(source: &SourceIdentity) -> PathBuf {
    if let Some(project) = source.path.ancestors().skip(1)
        .find(|path| path.extension().is_some_and(|extension| extension == super::model::SESSION_EXTENSION)) {
        return project.join(".lahza-audio");
    }
    // Imported/external media can be read-only. Never require creating files
    // beside it; a stable user cache also makes repeated imports reusable.
    let root = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("lahza-cache-{}", current_user_id())));
    root.join("lahza").join("audio")
}

fn current_user_id() -> u32 {
    #[cfg(unix)] { unsafe { libc::geteuid() } }
    #[cfg(not(unix))] { std::process::id() }
}

fn index_path(source: &SourceIdentity, mode: impl Into<AudioMode>) -> PathBuf {
    let mode = mode.into();
    let mut hash = Sha256::new();
    hash.update(source.path.as_os_str().as_encoded_bytes());
    let mode = mode.cache_key();
    cache_directory(source).join(format!("{:x}-{mode}.json", hash.finalize()))
}

fn valid_record(source: &SourceIdentity, mode: impl Into<AudioMode>) -> Option<CacheRecord> {
    let mode = mode.into();
    let record: CacheRecord = serde_json::from_slice(&fs::read(index_path(source, mode)).ok()?).ok()?;
    if record.version != PIPELINE_VERSION || record.processor != processing_identity(mode)
        || record.source != *source || record.mode != mode {
        return None;
    }
    if let Some(name) = &record.audio_file {
        // Cache metadata cannot reference outside its own directory.
        if Path::new(name).components().count() != 1 || !name.ends_with(".flac") { return None; }
        let path = cache_directory(source).join(name);
        if fs::metadata(&path).ok()?.len() != record.audio_bytes || record.audio_bytes < 42 { return None; }
        if record.audio_identity.as_ref() != Some(&SourceIdentity::read(&path).ok()?) { return None; }
        let mut magic = [0; 4];
        File::open(path).ok()?.read_exact(&mut magic).ok()?;
        if &magic != b"fLaC" { return None; }
    }
    Some(record)
}

/// Cheap ready lookup: reads only source metadata and a small cache stamp.
/// None means either the original is sufficient or preparation is not ready;
/// callers that need audio now must use `ensure_audio`.
pub fn prepared_audio(path: &Path, mode: impl Into<AudioMode>) -> Result<Option<PathBuf>, VideoError> {
    let mode = mode.into();
    let source = SourceIdentity::read(path)?;
    Ok(valid_record(&source, mode).and_then(|record|
        record.audio_file.map(|name| cache_directory(&source).join(name))))
}

/// Readiness includes a stored decision for legacy/raw and no-audio sources.
pub fn source_is_prepared(path: &Path, mode: impl Into<AudioMode>) -> bool {
    let mode = mode.into();
    let sources = match source_files(path) { Ok(sources) => sources, Err(_) => return false };
    sources.into_iter().all(|path| SourceIdentity::read(&path).ok()
        .is_some_and(|source| valid_record(&source, mode).is_some()))
}

fn source_files(path: &Path) -> Result<Vec<PathBuf>, VideoError> {
    if let Some(bank) = MediaBank::load(path)? {
        Ok(bank.entries.iter().enumerate().filter(|(_, entry)| !entry.still && entry.info.has_audio)
            .map(|(index, _)| bank.entry_path(path, index)).collect())
    } else { Ok(vec![path.to_owned()]) }
}

pub fn prepare_source(path: &Path, mode: impl Into<AudioMode>, control: &ProcessingControl) -> Result<(), VideoError> {
    let mode = mode.into();
    prepare_source_with_cancel(path, mode, control, &|| false)
}

pub fn prepare_source_with_cancel(path: &Path, mode: impl Into<AudioMode>, control: &ProcessingControl,
    cancelled: &dyn Fn() -> bool) -> Result<(), VideoError> {
    let mode = mode.into();
    control.done.store(false, Ordering::Release);
    let result = (|| {
        control.check(cancelled)?;
        let paths = source_files(path)?;
        let count = paths.len().max(1) as f64;
        for (index, path) in paths.iter().enumerate() {
            ensure_inner(path, mode, control, cancelled, index as f64 / count, 1.0 / count)?;
        }
        control.set_progress(1.0);
        control.set_status("Audio ready");
        Ok(())
    })();
    control.done.store(true, Ordering::Release);
    if matches!(result, Err(VideoError::Cancelled)) { control.set_status("Audio preparation cancelled"); }
    else if result.is_err() { control.set_status("Audio preparation failed"); }
    result
}

pub fn ensure_audio(path: &Path, mode: impl Into<AudioMode>, control: &ProcessingControl) -> Result<Option<PathBuf>, VideoError> {
    let mode = mode.into();
    ensure_audio_with_cancel(path, mode, control, &|| false)
}

pub fn ensure_audio_with_cancel(path: &Path, mode: impl Into<AudioMode>, control: &ProcessingControl,
    cancelled: &dyn Fn() -> bool) -> Result<Option<PathBuf>, VideoError> {
    let mode = mode.into();
    control.done.store(false, Ordering::Release);
    let result = ensure_inner(path, mode, control, cancelled, 0.0, 1.0);
    control.done.store(true, Ordering::Release);
    match &result {
        Ok(_) => control.set_status("Audio ready"),
        Err(VideoError::Cancelled) => control.set_status("Audio preparation cancelled"),
        Err(_) => control.set_status("Audio preparation failed"),
    }
    result
}

struct CacheLock(File);
impl CacheLock {
    fn acquire(path: &Path, control: &ProcessingControl, cancelled: &dyn Fn() -> bool) -> Result<Self, VideoError> {
        let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            loop {
                control.check(cancelled)?;
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 { break; }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::WouldBlock { return Err(error.into()); }
                control.set_status("Waiting for audio preparation");
                thread::sleep(Duration::from_millis(25));
            }
        }
        #[cfg(not(unix))]
        return Err(VideoError::InvalidMedia("audio cache locking is unavailable on this platform".into()));
        #[allow(unreachable_code)]
        Ok(Self(file))
    }
}
impl Drop for CacheLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        { use std::os::fd::AsRawFd; unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); } }
    }
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }

fn digest_file(path: &Path, control: &ProcessingControl, cancelled: &dyn Fn() -> bool) -> Result<String, VideoError> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        control.check(cancelled)?;
        let len = file.read(&mut buffer)?;
        if len == 0 { break; }
        hash.update(&buffer[..len]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)] streams: Vec<ProbeStream>,
    #[serde(default)] format: ProbeFormat,
}
#[derive(Deserialize)]
struct ProbeStream {
    index: u32,
    #[serde(default)] codec_type: String,
    #[serde(default)] tags: std::collections::BTreeMap<String, String>,
}
#[derive(Default, Deserialize)]
struct ProbeFormat { duration: Option<String>, start_time: Option<String> }

fn inspect(path: &Path, control: &ProcessingControl, cancelled: &dyn Fn() -> bool) -> Result<(Vec<AudioTrack>, f64, f64), VideoError> {
    let mut command = Command::new("ffprobe");
    command.args(["-v", "error", "-show_entries", "stream=index,codec_type:stream_tags=title:format=duration,start_time", "-of", "json"]).arg(path);
    let bytes = run_command(command, control, cancelled, None)?;
    let probe: Probe = serde_json::from_slice(&bytes).map_err(|error| VideoError::Probe(error.to_string()))?;
    let mut tracks: Vec<_> = probe.streams.into_iter().filter(|stream| stream.codec_type == "audio").map(|stream| {
        let title = stream.tags.iter().find(|(key, _)| key.eq_ignore_ascii_case("title"))
            .map(|(_, value)| value.replace('-', " ").to_lowercase()).unwrap_or_default();
        let role = match title.as_str() {
            "lahza microphone" => Role::Microphone,
            "lahza system audio" => Role::System,
            _ => Role::Legacy,
        };
        AudioTrack { index: stream.index, role }
    }).collect();
    if tracks.iter().any(|track| track.role != Role::Legacy) {
        tracks.retain(|track| track.role != Role::Legacy);
    } else {
        // Imports may contain alternative language tracks. Preserve the old
        // first-track policy rather than accidentally mixing every language.
        tracks.truncate(1);
    }
    let duration = probe.format.duration.and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0).unwrap_or(0.0);
    // Negative format starts commonly describe codec pre-roll (e.g. Opus's
    // -7 ms packet), not a negative recording origin. FFmpeg decodes that
    // packet with its skip-sample metadata; shifting it again delays speech.
    let origin = probe.format.start_time.and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite()).unwrap_or(0.0).max(0.0);
    Ok((tracks, duration, origin))
}

fn ensure_inner(path: &Path, mode: AudioMode, control: &ProcessingControl, cancelled: &dyn Fn() -> bool,
    base: f64, share: f64) -> Result<Option<PathBuf>, VideoError> {
    control.check(cancelled)?;
    let source = SourceIdentity::read(path)?;
    let directory = cache_directory(&source);
    fs::create_dir_all(&directory)?;
    let index = index_path(&source, mode);
    let _lock = CacheLock::acquire(&index.with_extension("lock"), control, cancelled)?;
    if let Some(record) = valid_record(&source, mode) {
        // Content was hashed when this artifact was committed. Identity includes
        // ctime/inode, so edited or replaced audio invalidates it without reading
        // a whole recording at every clip boundary.
        let ready = record.audio_file.as_ref().map(|name| directory.join(name));
        control.check(cancelled)?;
        control.set_progress(base + share);
        return Ok(ready);
    }
    control.set_status("Inspecting audio");
    let (tracks, duration, origin) = inspect(path, control, cancelled)?;
    let needs_asset = !tracks.is_empty() && (mode != AudioMode::Off || tracks.iter().any(|track| track.role != Role::Legacy));
    control.set_status("Checking audio source");
    let fingerprint = if needs_asset { digest_file(path, control, cancelled)? } else { String::new() };
    let mut record = CacheRecord { version: PIPELINE_VERSION.into(), processor: processing_identity(mode),
        source: source.clone(), source_sha256: fingerprint,
        mode, audio_file: None, audio_bytes: 0, audio_sha256: None, audio_identity: None, tracks };
    let ready = if needs_asset {
        if duration <= 0.0 { return Err(VideoError::InvalidMedia("audio source has no valid duration".into())); }
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(&record).map_err(|error| VideoError::InvalidMedia(error.to_string()))?);
        hash.update(processing_identity(mode).as_bytes());
        let filename = format!("{:x}.flac", hash.finalize());
        let destination = directory.join(&filename);
        let temporary = TemporaryFile(directory.join(format!("{}.part.flac", uuid::Uuid::new_v4())));
        let work = TemporaryDirectory(directory.join(format!(".prepare-{}", uuid::Uuid::new_v4())));
        fs::create_dir(&work.0)?;
        let eligible = record.tracks.iter().filter(|track| track.role != Role::System && (mode != AudioMode::Off || track.role == Role::Microphone)).count();
        let mut processed = Vec::new();
        for track in &record.tracks {
            if track.role == Role::System || (mode == AudioMode::Off && track.role == Role::Legacy) { continue; }
            let stage_base = base + share * 0.9 * processed.len() as f64 / eligible.max(1) as f64;
            let stage_share = share * 0.9 / eligible.max(1) as f64;
            let pcm = prepare_microphone(path, track.index, duration, origin, mode,
                &work.0, control, cancelled, stage_base, stage_share)?;
            processed.push((track.index, pcm));
        }
        let mut graph = String::new();
        for (index, track) in record.tracks.iter().enumerate() {
            if let Some(position) = processed.iter().position(|(stream, _)| *stream == track.index) {
                graph.push_str(&format!("[{}:a:0]aformat=sample_fmts=fltp:channel_layouts=stereo[a{index}];", position + 1));
            } else {
                graph.push_str(&format!("[0:{}]aresample=48000:async=1:first_pts=0,aformat=sample_fmts=fltp:channel_layouts=stereo[a{index}];", track.index));
            }
        }
        for index in 0..record.tracks.len() { graph.push_str(&format!("[a{index}]")); }
        if record.tracks.len() > 1 {
            graph.push_str(&format!("amix=inputs={}:normalize=0:duration=longest,alimiter=limit=0.95:level=0:latency=1,", record.tracks.len()));
        }
        graph.push_str(&format!("apad,atrim=duration={duration:.9},aformat=sample_fmts=s32[out]"));
        let mut command = Command::new("ffmpeg");
        command.args(["-v", "error", "-nostdin", "-y", "-copyts", "-itsoffset"]).arg(format!("{:.9}", -origin))
            .arg("-i").arg(path);
        for (_, pcm) in &processed {
            command.args(["-f", "f32le", "-ar", "48000", "-ac", "2", "-i"]).arg(pcm);
        }
        command.args(["-filter_complex"]).arg(graph).args(["-map", "[out]", "-vn", "-sn", "-dn", "-map_metadata", "-1",
                "-c:a", "flac", "-sample_fmt", "s32", "-ar", "48000", "-ac", "2", "-progress", "pipe:1", "-nostats", "-f", "flac"])
            .arg(&temporary.0);
        control.set_status("Saving prepared audio");
        let completed = if eligible > 0 { 0.9 } else { 0.0 };
        run_command(command, control, cancelled, Some((duration, base + share * completed, share * (1.0 - completed))))?;
        control.check(cancelled)?;
        // A completed process can still produce an invalid file (e.g. a broken
        // encoder wrapper). Validate its container before publishing the stamp.
        validate_flac(&temporary.0, duration)?;
        record.audio_bytes = fs::metadata(&temporary.0)?.len();
        record.audio_sha256 = Some(digest_file(&temporary.0, control, cancelled)?);
        record.audio_file = Some(filename);
        if SourceIdentity::read(path)? != source { return Err(VideoError::InvalidMedia("audio source changed during preparation; retry".into())); }
        File::open(&temporary.0)?.sync_all()?;
        fs::rename(&temporary.0, &destination)?;
        record.audio_identity = Some(SourceIdentity::read(&destination)?);
        Some(destination)
    } else { None };
    control.check(cancelled)?;
    if SourceIdentity::read(path)? != source { return Err(VideoError::InvalidMedia("audio source changed during preparation; retry".into())); }
    let stamp = TemporaryFile(index.with_extension(format!("{}.tmp", uuid::Uuid::new_v4())));
    let bytes = serde_json::to_vec(&record).map_err(|error| VideoError::InvalidMedia(error.to_string()))?;
    let mut file = File::create(&stamp.0)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&stamp.0, &index)?;
    control.set_progress(base + share);
    Ok(ready)
}

/// PCM intermediates stay private to the job and disappear on failure or
/// cancellation. Models and DSP stream bounded blocks rather than whole takes.
struct TemporaryDirectory(PathBuf);
impl Drop for TemporaryDirectory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn prepare_microphone(source: &Path, stream: u32, duration: f64, origin: f64,
    mode: AudioMode, directory: &Path, control: &ProcessingControl,
    cancelled: &dyn Fn() -> bool, base: f64, share: f64) -> Result<PathBuf, VideoError> {
    let decoded = directory.join(format!("{stream}-decoded.f32"));
    let normalized = directory.join(format!("{stream}-normalized.f32"));
    let enhanced = directory.join(format!("{stream}-enhanced.f32"));
    let gated = directory.join(format!("{stream}-speech.f32"));
    let finished = directory.join(format!("{stream}-finished.f32"));
    control.set_status("Preparing microphone");
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin", "-y", "-copyts", "-itsoffset"])
        .arg(format!("{:.9}", -origin)).arg("-i").arg(source)
        .arg("-map").arg(format!("0:{stream}"))
        .arg("-af").arg(format!("aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration:.9}"))
        .args(["-vn", "-sn", "-dn", "-ac", "2", "-ar", "48000", "-c:a", "pcm_f32le",
            "-f", "f32le", "-progress", "pipe:1", "-nostats"]).arg(&decoded);
    run_command(command, control, cancelled, Some((duration, base, share * 0.1)))?;
    let stopped = || control.is_cancelled() || cancelled();
    let failure = |error: String| if stopped() { VideoError::Cancelled } else { VideoError::Decode(error) };
    control.set_status("Balancing microphone channels");
    super::audio_dsp::normalize_file(&decoded, &normalized, 2, mode, &stopped,
        &mut |fraction| control.set_progress(base + share * (0.1 + 0.1 * fraction)))
        .map_err(&failure)?;
    fs::remove_file(&decoded)?;
    let neural_input = match mode {
        AudioMode::NoiseReduction => {
            control.set_status("Removing background noise");
            super::neural_audio::enhance_file(&normalized, &enhanced, 2, &stopped,
                &mut |fraction| control.set_progress(base + share * (0.2 + 0.6 * fraction)))
                .map_err(&failure)?;
            &enhanced
        }
        AudioMode::Podcast => {
            control.set_status("Softening pauses before restoration");
            super::audio_dsp::prepare_restoration_file(&normalized, &gated, 2, &stopped,
                &mut |fraction| control.set_progress(base + share * (0.2 + 0.05 * fraction)))
                .map_err(&failure)?;
            control.set_status("Enhancing microphone speech");
            super::restoration_audio::enhance_file(&gated, &enhanced, 2, &stopped,
                &mut |fraction| control.set_progress(base + share * (0.25 + 0.55 * fraction)))
                .map_err(&failure)?;
            &enhanced
        }
        _ => &normalized,
    };
    control.set_status("Finishing microphone sound");
    super::audio_dsp::finish_file(neural_input, Some(&normalized), &finished,
        48000, 2, mode, &stopped,
        &mut |fraction| control.set_progress(base + share * (0.8 + 0.2 * fraction)))
        .map_err(&failure)?;
    let _ = fs::remove_file(&normalized);
    let _ = fs::remove_file(&enhanced);
    let _ = fs::remove_file(&gated);
    control.check(cancelled)?;
    Ok(finished)
}

fn validate_flac(path: &Path, duration: f64) -> Result<(), VideoError> {
    let mut header = [0; 42];
    File::open(path)?.read_exact(&mut header)?;
    let valid_header = &header[..4] == b"fLaC" && header[4] & 0x7f == 0 && header[5..8] == [0, 0, 34];
    let format = u64::from_be_bytes(header[18..26].try_into().unwrap());
    let rate = format >> 44;
    let channels = ((format >> 41) & 7) + 1;
    let bits = ((format >> 36) & 31) + 1;
    let samples = format & ((1 << 36) - 1);
    let expected = (duration * 48000.0).round() as u64;
    if !valid_header || rate != 48000 || channels != 2 || bits != 24 || samples == 0 || samples.abs_diff(expected) > 1 {
        return Err(VideoError::Decode("audio preparation produced an incomplete or invalid FLAC file".into()));
    }
    Ok(())
}

/// Drain both pipes while polling the child. Cancellation also works during
/// probing and encoder stalls, and never waits for a full recording to finish.
fn run_command(mut command: Command, control: &ProcessingControl, cancelled: &dyn Fn() -> bool,
    progress: Option<(f64, f64, f64)>) -> Result<Vec<u8>, VideoError> {
    control.check(cancelled)?;
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let progress_control = control.clone();
    let reader = thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut kept = Vec::new();
        for line in BufReader::new(stdout).split(b'\n') {
            let line = line?;
            if let Some((duration, base, share)) = progress {
                if let Some(value) = line.strip_prefix(b"out_time_us=") {
                    if let Ok(value) = String::from_utf8_lossy(value).parse::<f64>() {
                        progress_control.set_progress(base + share * (value / 1_000_000.0 / duration).clamp(0.0, 0.99));
                    }
                }
            } else if kept.len() < 4 * 1024 * 1024 {
                kept.extend_from_slice(&line[..line.len().min(4 * 1024 * 1024 - kept.len())]);
                kept.push(b'\n');
            }
        }
        Ok(kept)
    });
    let errors = thread::spawn(move || {
        let mut source = stderr;
        let mut bytes = Vec::new();
        let mut block = [0; 4096];
        loop {
            let count = match source.read(&mut block) { Ok(0) | Err(_) => break, Ok(count) => count };
            let remaining = (64 * 1024usize).saturating_sub(bytes.len());
            bytes.extend_from_slice(&block[..count.min(remaining)]);
        }
        bytes
    });
    let result = loop {
        if let Err(error) = control.check(cancelled) { let _ = child.kill(); let _ = child.wait(); break Err(error); }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => { let _ = child.kill(); let _ = child.wait(); break Err(VideoError::Io(error)); }
        }
    };
    let output = reader.join().map_err(|_| VideoError::Decode("audio progress reader failed".into()))?;
    let errors = errors.join().unwrap_or_default();
    let status = result?;
    let output = output?;
    if !status.success() { return Err(VideoError::Decode(format!("audio preparation failed: {}", String::from_utf8_lossy(&errors).trim()))); }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("lahza-audio-assets-{}.lahzarec", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf { self.0.join(name) }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

    fn ffmpeg(args: &[&str], destination: &Path) {
        let result = Command::new("ffmpeg").args(["-v", "error", "-nostdin", "-y"])
            .args(args).arg(destination).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }

    fn make_tracks(path: &Path) {
        ffmpeg(&["-f", "lavfi", "-i", "anoisesrc=color=white:amplitude=0.15:sample_rate=48000:duration=1:seed=73",
            "-f", "lavfi", "-i", "sine=frequency=997:sample_rate=48000:duration=1",
            "-map", "0:a", "-map", "1:a", "-c:a", "pcm_f32le", "-ac", "2",
            "-metadata:s:a:0", "title=Lahza-Microphone", "-metadata:s:a:1", "title=Lahza-System-Audio"], path);
    }

    fn samples(path: &Path, stream: &str, filter: Option<&str>) -> Vec<f32> {
        let mut command = Command::new("ffmpeg");
        command.args(["-v", "error", "-i"]).arg(path).args(["-map", stream]);
        if let Some(filter) = filter { command.args(["-af", filter]); }
        let output = command.args(["-ar", "48000", "-ac", "2", "-f", "f32le", "pipe:1"]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout.chunks_exact(4).map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())).collect()
    }

    #[test]
    #[ignore = "public speech integration/benchmark; explicit fixture paths required"]
    fn public_speech_presets_probe() {
        let source = PathBuf::from(std::env::var_os("LAHZA_AUDIO_FIXTURE").expect("explicit speech fixture"));
        let directory = PathBuf::from(std::env::var_os("LAHZA_AUDIO_OUTPUT").expect("explicit output directory"));
        fs::create_dir_all(&directory).unwrap();
        let selection = std::env::var("LAHZA_AUDIO_MODES").unwrap_or_else(|_| "standard,loud,noise-reduction,podcast".into());
        let mut report = Vec::new();
        for mode in AudioMode::ALL.into_iter().filter(|mode| selection.split(',').any(|key| key == mode.cache_key())) {
            let began = std::time::Instant::now();
            let prepared = ensure_audio(&source, mode, &ProcessingControl::default()).unwrap().expect("prepared preset asset");
            let elapsed = began.elapsed().as_secs_f64();
            let destination = directory.join(format!("{}.flac", mode.cache_key()));
            fs::copy(&prepared, &destination).unwrap();
            let values = samples(&destination, "0:a:0", None);
            assert!(values.iter().all(|value| value.is_finite()));
            let peak = values.iter().copied().map(f32::abs).fold(0.0_f32, f32::max);
            assert!(peak <= 1.0);
            report.push(serde_json::json!({ "mode": mode.cache_key(), "elapsed_seconds": elapsed,
                "output": destination, "frames": values.len() / 2, "peak": peak }));
        }
        fs::write(directory.join("preparation.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    }

    #[test]
    fn separated_tracks_clean_only_microphone_and_cache_lossless_source_time_mix() {
        let fixture = Fixture::new();
        let source = fixture.path("screen.mkv");
        make_tracks(&source);
        let control = ProcessingControl::default();
        let prepared = ensure_audio(&source, true, &control).unwrap().unwrap();
        assert_eq!(prepared.extension().unwrap(), "flac");
        validate_flac(&prepared, 1.0).unwrap();
        assert!(source_is_prepared(&source, true));
        assert_eq!(prepared_audio(&source, true).unwrap(), Some(prepared.clone()));
        let output = samples(&prepared, "0:a:0", None);
        // Process the isolated microphone independently; adding a desktop
        // stream must not alter its neural/DSP result or clean desktop sound.
        // A stream-copy Matroska remux rounds this fixture to 999 ms.
        // WAV retains the exact PCM frame count, so both model passes see
        // identical context (including the last millisecond).
        let mic_source = fixture.path("microphone.wav");
        let output_status = Command::new("ffmpeg").args(["-v", "error", "-y", "-i"])
            .arg(&source).args(["-map", "0:a:0", "-c:a", "pcm_f32le"]).arg(&mic_source).status().unwrap();
        assert!(output_status.success());
        let mic_asset = ensure_audio(&mic_source, true, &ProcessingControl::default()).unwrap().unwrap();
        let mic = samples(&mic_asset, "0:a:0", None);
        let system = samples(&source, "0:a:1", None);
        assert_eq!(output.len(), 96000);
        let max_error = output.iter().zip(mic.iter().zip(&system)).map(|(actual, (mic, system))|
            (actual - (mic + system)).abs()).fold(0.0_f32, f32::max);
        assert!(max_error < 0.000002, "system track changed or denoiser order changed: {max_error}");
        let before = fs::metadata(&prepared).unwrap().modified().unwrap();
        assert_eq!(ensure_audio(&source, true, &ProcessingControl::default()).unwrap(), Some(prepared.clone()));
        assert_eq!(fs::metadata(&prepared).unwrap().modified().unwrap(), before);
        let raw = ensure_audio(&source, false, &ProcessingControl::default()).unwrap().unwrap();
        assert_ne!(raw, prepared);
        assert_ne!(samples(&raw, "0:a:0", None), output);
    }

    #[test]
    fn off_repairs_tagged_microphone_without_changing_gain_or_legacy_audio() {
        let fixture = Fixture::new();
        let source = fixture.path("one-sided.mkv");
        ffmpeg(&["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=0.1",
            "-af", "pan=stereo|c0=c0|c1=0*c0", "-c:a", "pcm_f32le",
            "-metadata:s:a:0", "title=Lahza-Microphone"], &source);
        let original = samples(&source, "0:a:0", None);
        let prepared = ensure_audio(&source, AudioMode::Off, &ProcessingControl::default()).unwrap().unwrap();
        let repaired = samples(&prepared, "0:a:0", None);
        assert_eq!(original.len(), repaired.len());
        for (before, after) in original.chunks_exact(2).zip(repaired.chunks_exact(2)) {
            assert!((before[0] - after[0]).abs() < 0.000002);
            assert_eq!(after[0], after[1]);
        }
    }

    #[test]
    fn system_only_is_unchanged_by_cleaning_and_legacy_raw_uses_original() {
        let fixture = Fixture::new();
        let system = fixture.path("system.mkv");
        ffmpeg(&["-f", "lavfi", "-i", "anoisesrc=amplitude=0.1:sample_rate=48000:duration=0.4:seed=7",
            "-c:a", "pcm_f32le", "-ac", "2", "-metadata:s:a:0", "title=Lahza-System-Audio"], &system);
        let raw = ensure_audio(&system, false, &ProcessingControl::default()).unwrap().unwrap();
        let cleaned = ensure_audio(&system, true, &ProcessingControl::default()).unwrap().unwrap();
        assert_eq!(samples(&raw, "0:a:0", None), samples(&cleaned, "0:a:0", None));
        let legacy = fixture.path("legacy.wav");
        ffmpeg(&["-f", "lavfi", "-i", "sine=sample_rate=48000:duration=0.1", "-c:a", "pcm_s16le"], &legacy);
        assert!(!source_is_prepared(&legacy, false));
        assert!(ensure_audio(&legacy, false, &ProcessingControl::default()).unwrap().is_none());
        assert!(source_is_prepared(&legacy, false));
        assert!(ensure_audio(&legacy, true, &ProcessingControl::default()).unwrap().is_some());
    }

    #[test]
    fn cache_rejects_changed_source_corrupt_output_and_changed_chain() {
        let fixture = Fixture::new();
        let source = fixture.path("screen.mkv");
        make_tracks(&source);
        let path = ensure_audio(&source, true, &ProcessingControl::default()).unwrap().unwrap();
        let old_source = SourceIdentity::read(&source).unwrap();
        let stamp = index_path(&old_source, true);
        let mut record: CacheRecord = serde_json::from_slice(&fs::read(&stamp).unwrap()).unwrap();
        record.version = "old-algorithm".into();
        fs::write(&stamp, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(!source_is_prepared(&source, true));
        ensure_audio(&source, true, &ProcessingControl::default()).unwrap();
        // Keep the file's size and magic while corrupting its data: output
        // identity must catch this, not just an existence/header check.
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 7;
        fs::write(&path, bytes).unwrap();
        assert!(!source_is_prepared(&source, true));
        ensure_audio(&source, true, &ProcessingControl::default()).unwrap();
        assert!(source_is_prepared(&source, true));
        let old_content_hash = valid_record(&old_source, true).unwrap().source_sha256;
        ffmpeg(&["-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000:duration=1", "-c:a", "pcm_f32le", "-ac", "2",
            "-metadata:s:a:0", "title=Lahza-Microphone"], &source);
        assert!(!source_is_prepared(&source, true));
        let replacement = ensure_audio(&source, true, &ProcessingControl::default()).unwrap().unwrap();
        assert_ne!(replacement, path);
        let new_source = SourceIdentity::read(&source).unwrap();
        assert_ne!(valid_record(&new_source, true).unwrap().source_sha256, old_content_hash);
    }

    #[test]
    fn simultaneous_requests_share_one_complete_asset_and_cancel_waiting_jobs() {
        let fixture = Fixture::new();
        let source = fixture.path("screen.mkv");
        make_tracks(&source);
        let one_source = source.clone();
        let one = thread::spawn(move || ensure_audio(&one_source, true, &ProcessingControl::default()).unwrap());
        let two_source = source.clone();
        let two = thread::spawn(move || ensure_audio(&two_source, true, &ProcessingControl::default()).unwrap());
        assert_eq!(one.join().unwrap(), two.join().unwrap());
        let identity = SourceIdentity::read(&source).unwrap();
        let lock = CacheLock::acquire(&index_path(&identity, false).with_extension("lock"), &ProcessingControl::default(), &|| false).unwrap();
        let cancelled = ProcessingControl::default();
        let job_control = cancelled.clone();
        let waiting = thread::spawn(move || ensure_audio(&source, false, &job_control));
        thread::sleep(Duration::from_millis(60));
        cancelled.cancel();
        assert!(matches!(waiting.join().unwrap(), Err(VideoError::Cancelled)));
        drop(lock);
        assert!(fs::read_dir(cache_directory(&identity)).unwrap().all(|entry|
            !entry.unwrap().file_name().to_string_lossy().contains(".part")));
    }

    #[test]
    fn cancellation_interrupts_running_child_and_reaps_it() {
        // Purely synthetic process: no device or user media is opened.
        let control = ProcessingControl::default();
        let job_control = control.clone();
        let began = std::time::Instant::now();
        let job = thread::spawn(move || {
            let mut command = Command::new("ffmpeg");
            command.args(["-v", "error", "-re", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo",
                "-t", "30", "-f", "null", "-"]);
            run_command(command, &job_control, &|| false, Some((30.0, 0.0, 1.0)))
        });
        thread::sleep(Duration::from_millis(100));
        control.cancel();
        assert!(matches!(job.join().unwrap(), Err(VideoError::Cancelled)));
        assert!(began.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn delayed_microphone_keeps_its_offset_from_video_start() {
        let fixture = Fixture::new();
        let source = fixture.path("offset.mkv");
        ffmpeg(&["-f", "lavfi", "-i", "color=black:size=16x16:rate=10:duration=1",
            "-itsoffset", "0.3", "-f", "lavfi", "-i", "sine=frequency=997:sample_rate=48000:duration=0.7",
            "-map", "0:v", "-map", "1:a", "-c:v", "ffv1", "-c:a", "pcm_s16le", "-ac", "2",
            "-metadata:s:a:0", "title=Lahza-Microphone"], &source);
        let output = ensure_audio(&source, false, &ProcessingControl::default()).unwrap().unwrap();
        let audio = samples(&output, "0:a:0", None);
        assert!(audio[..24000].iter().all(|sample| sample.abs() < 0.000001));
        assert!(audio[38400..48000].iter().any(|sample| sample.abs() > 0.03));
        assert_eq!(audio.len(), 96000);
    }

    #[test]
    fn opus_preroll_does_not_add_an_extra_audio_delay() {
        let fixture = Fixture::new();
        let source = fixture.path("opus.mkv");
        ffmpeg(&["-f", "lavfi", "-i", "color=black:size=16x16:rate=10:duration=0.4",
            "-f", "lavfi", "-i", "sine=frequency=997:sample_rate=48000:duration=0.4",
            "-map", "0:v", "-map", "1:a", "-c:v", "ffv1", "-c:a", "libopus", "-ac", "2",
            "-metadata:s:a:0", "title=Lahza-Microphone"], &source);
        let prepared = ensure_audio(&source, false, &ProcessingControl::default()).unwrap().unwrap();
        let original = samples(&source, "0:a:0", None);
        let output = samples(&prepared, "0:a:0", None);
        let max_error = original.iter().zip(&output).map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
        assert!(max_error < 0.000002, "Opus samples shifted relative to video: {max_error}");
    }

    #[test]
    fn external_readonly_media_uses_user_cache_not_its_source_directory() {
        let root = std::env::temp_dir().join(format!("lahza-external-audio-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let fixture = Fixture(root);
        let source = fixture.path("read-only.mkv");
        make_tracks(&source);
        let mut permissions = fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&source, permissions).unwrap();
        let prepared = ensure_audio(&source, true, &ProcessingControl::default()).unwrap().unwrap();
        assert!(!prepared.starts_with(&fixture.0));
        assert!(!fixture.path(".lahza-audio").exists());
        assert!(source_is_prepared(&source, true));
        let identity = SourceIdentity::read(&source).unwrap();
        let index = index_path(&identity, true);
        fs::remove_file(prepared).unwrap();
        fs::remove_file(&index).unwrap();
        fs::remove_file(index.with_extension("lock")).unwrap();
    }
}
