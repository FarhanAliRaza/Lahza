//! Sound for motion projects in the editor: the music, narration, effects
//! and footage sound play in sync with the playhead, and a waveform lane
//! shows where they sit on the timeline.
//!
//! Playback reuses the studio's `AudioPlayback` (GStreamer), placed the way
//! the motion exporter mixes the project, so what plays in the editor is
//! what renders. Footage with a speed other than 1× and music ducking are
//! export-only for now.

use crate::recording::media::{AudioClip, AudioPlayback};
use lahza_motion_core::compile::Compiled;
use lahza_motion_core::document::Project;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Waveform buckets per second.
const BUCKETS_PER_SECOND: f64 = 100.0;

/// A sound of the project placed on its timeline.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    /// Sound id for edits ("music", "vo", an effect's id); None for the
    /// sound of a footage layer (edited on the layer).
    pub id: Option<String>,
    pub path: PathBuf,
    pub clip: AudioClip,
}

/// Where each sound of the project plays, with its file probed.
pub(crate) fn placed(project: &Project, compiled: &Compiled, dir: &Path) -> Vec<Placed> {
    let end = project.settings.duration;
    let mut out = Vec::new();
    let mut push = |id: Option<String>, path: PathBuf, at: f64, trim: f64, gain_db: f64, length: Option<f64>, fades: (f64, f64), name: &str| {
        let Ok((source_duration, has_audio)) = lahza_motion_render::footage::probe_sound(&path) else { return };
        if !has_audio {
            return;
        }
        // A clip placed before zero starts part-way through.
        let start = at.max(0.0);
        let source_start = trim + (start - at);
        let mut duration = (source_duration - source_start).max(0.0).min(end - start);
        if let Some(length) = length {
            duration = duration.min(length - (start - at));
        }
        if duration <= 0.01 {
            return;
        }
        let clip = AudioClip {
            id: uuid::Uuid::new_v4(),
            file: path.clone(),
            name: name.to_string(),
            start,
            source_start,
            duration,
            volume: 10f64.powf(gain_db / 20.0).min(2.0),
            // A clip that starts before zero has used up part of its fade in.
            fade_in: (fades.0 - (start - at)).clamp(0.0, duration),
            fade_out: fades.1.clamp(0.0, duration),
            source_duration,
        };
        out.push(Placed { id, path, clip });
    };
    let slots = [("music", project.audio.music.as_ref()), ("vo", project.audio.vo.as_ref())];
    let named = slots.into_iter().filter_map(|(slot, c)| c.map(|c| (c.id.clone().unwrap_or_else(|| slot.into()), c)));
    for (id, clip) in named.chain(project.audio.sfx.iter().map(|c| (c.id.clone().unwrap_or_default(), c))) {
        let at = compiled.resolve(&project.markers, &clip.at).unwrap_or(0.0);
        let file = clip.path.rsplit('/').next().unwrap_or(&clip.path).to_string();
        let name = clip.name.clone().unwrap_or_else(|| if id.is_empty() { file } else { id.clone() });
        push((!id.is_empty()).then_some(id), dir.join(&clip.path), at, clip.trim, clip.gain_db, clip.length, (clip.fade_in, clip.fade_out), &name);
    }
    for footage in compiled.footage_audio() {
        if (footage.speed - 1.0).abs() < 1e-3 {
            push(None, dir.join(&*footage.src), footage.start, footage.trim, footage.gain_db, Some(footage.end - footage.start), (0.0, 0.0), &footage.src);
        }
    }
    out
}

/// Every sound as studio audio clips (path, clip), for playback.
pub(crate) fn clips(project: &Project, compiled: &Compiled, dir: &Path) -> Vec<(PathBuf, AudioClip)> {
    placed(project, compiled, dir).into_iter().map(|p| (p.path, p.clip)).collect()
}

/// Plays the project's sound on its own thread while the editor plays.
/// Dropping it stops the sound.
pub(crate) struct Player {
    stop: Arc<AtomicBool>,
}

impl Player {
    /// `clock` holds the playhead in seconds (`f64::to_bits`); `error`
    /// receives a message if the sound cannot play.
    pub(crate) fn start(project: Project, compiled: Arc<Compiled>, dir: PathBuf, clock: Arc<AtomicU64>, error: Arc<Mutex<Option<String>>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("motion-sound".into())
            .spawn(move || {
                let clips = clips(&project, &compiled, &dir);
                if clips.is_empty() || stopped.load(Ordering::SeqCst) {
                    return;
                }
                let mut playback = match AudioPlayback::open(clips) {
                    Ok(playback) => playback,
                    Err(e) => {
                        *error.lock().unwrap() = Some(format!("Sound unavailable: {e}"));
                        return;
                    }
                };
                while !stopped.load(Ordering::SeqCst) {
                    if let Err(e) = playback.sync(f64::from_bits(clock.load(Ordering::SeqCst))) {
                        *error.lock().unwrap() = Some(format!("Sound stopped: {e}"));
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
            })
            .ok();
        Self { stop }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// One sound on the timeline, with the peaks of its whole file so a trim
/// can be widened again while dragging.
#[derive(Clone, Debug)]
pub(crate) struct Wave {
    pub id: Option<String>,
    pub name: String,
    /// Timeline start, file offset and seconds played.
    pub start: f64,
    pub source_start: f64,
    pub duration: f64,
    pub source_duration: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    /// Display gain: the clip's volume over the project's loudest moment.
    pub gain: f32,
    /// Whole-file peaks, `BUCKETS_PER_SECOND` per second.
    pub file: Arc<Vec<f32>>,
}

impl Wave {
    /// Fade envelope (0..1) at timeline time `t`.
    pub(crate) fn envelope(&self, t: f64) -> f32 {
        let local = t - self.start;
        if local < 0.0 || local > self.duration {
            return 0.0;
        }
        let fin = if self.fade_in > 0.0 { (local / self.fade_in).min(1.0) } else { 1.0 };
        let fout = if self.fade_out > 0.0 { ((self.duration - local) / self.fade_out).min(1.0) } else { 1.0 };
        (fin * fout) as f32
    }

    /// The loudest displayed peak between `t0` and `t1` (timeline seconds),
    /// with gain and fades applied.
    pub(crate) fn peak(&self, t0: f64, t1: f64) -> f32 {
        let (t0, t1) = (t0.max(self.start), t1.min(self.start + self.duration));
        if t1 <= t0 {
            return 0.0;
        }
        let to_bucket = |t: f64| (self.source_start + t - self.start) * BUCKETS_PER_SECOND;
        let a = to_bucket(t0).floor().max(0.0) as usize;
        let b = (to_bucket(t1).ceil().max(0.0) as usize).min(self.file.len());
        let raw = self.file.get(a..b).map(|s| s.iter().copied().fold(0.0, f32::max)).unwrap_or(0.0);
        raw * self.gain * self.envelope((t0 + t1) / 2.0)
    }
}

/// Peaks of whole files, decoded once: moving or cutting a sound only
/// re-slices them.
pub(crate) type PeakCache = Arc<Mutex<std::collections::HashMap<PathBuf, Arc<Vec<f32>>>>>;

fn file_peaks(cache: &PeakCache, path: &Path) -> Option<Arc<Vec<f32>>> {
    if let Some(p) = cache.lock().unwrap().get(path) {
        return Some(p.clone());
    }
    let samples = lahza_motion_audio::decode(path).ok()?;
    let rate = lahza_motion_audio::SAMPLE_RATE as f64;
    // Bucket edges from exact times, so long sounds don't drift.
    let per = rate / BUCKETS_PER_SECOND;
    let count = (samples.len() as f64 / per).ceil() as usize;
    let peaks: Vec<f32> = (0..count)
        .map(|i| {
            let (a, b) = ((i as f64 * per) as usize, (((i + 1) as f64 * per) as usize).min(samples.len()));
            samples.get(a..b).map(|r| r.iter().fold(0.0f32, |m, s| m.max(s.abs()))).unwrap_or(0.0)
        })
        .collect();
    let peaks = Arc::new(peaks);
    cache.lock().unwrap().insert(path.to_path_buf(), peaks.clone());
    Some(peaks)
}

/// Every sound's peaks, normalised so the loudest moment of the project
/// reaches 1.
pub(crate) fn waves(project: &Project, compiled: &Compiled, dir: &Path, cache: &PeakCache) -> Vec<Wave> {
    let mut waves = Vec::new();
    for p in placed(project, compiled, dir) {
        let Some(file) = file_peaks(cache, &p.path) else { continue };
        let c = &p.clip;
        waves.push(Wave {
            id: p.id,
            name: c.name.clone(),
            start: c.start,
            source_start: c.source_start,
            duration: c.duration,
            source_duration: c.source_duration,
            fade_in: c.fade_in,
            fade_out: c.fade_out,
            gain: c.volume as f32,
            file,
        });
    }
    // Normalise: the loudest played moment of the project reaches 1.
    let loudest = waves
        .iter()
        .map(|w| {
            let a = (w.source_start * BUCKETS_PER_SECOND) as usize;
            let b = (((w.source_start + w.duration) * BUCKETS_PER_SECOND).ceil() as usize).min(w.file.len());
            w.file.get(a..b).map(|s| s.iter().copied().fold(0.0, f32::max)).unwrap_or(0.0) * w.gain
        })
        .fold(0.0f32, f32::max);
    if loudest > 0.0 {
        for w in &mut waves {
            w.gain /= loudest;
        }
    }
    waves
}

/// A signature of what the sound depends on, to recompute waveforms only
/// when the audio placement changes.
pub(crate) fn signature(project: &Project, compiled: &Compiled) -> String {
    let mut s = serde_json::to_string(&project.audio).unwrap_or_default();
    // Anchored sounds move with their layers.
    for clip in project.audio.clips() {
        s.push_str(&format!("|{:.4}", compiled.resolve(&project.markers, &clip.at).unwrap_or(0.0)));
    }
    for f in compiled.footage_audio() {
        s.push_str(&format!("|{}@{:.4}+{:.4}x{:.3}", f.src, f.start, f.trim, f.speed));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use lahza_motion_mcp::tools::Toolbox;
    use serde_json::json;

    #[test]
    fn music_is_placed_like_the_export_and_drawn() {
        let dir = std::env::temp_dir().join(format!("lahza-motion-sound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tone = dir.join("tone.wav");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=3", "-y"])
            .arg(&tone)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut tb = Toolbox::new();
        tb.cwd = dir.clone();
        tb.call("project_create", json!({"path": "p", "duration": 2, "music": "tone.wav"})).unwrap();
        let project_dir = dir.join("p.lahza");
        // Start the music half a second in, one second into the file.
        let file = project_dir.join("project.json");
        let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        doc["audio"]["music"]["at"] = json!(0.5);
        doc["audio"]["music"]["trim"] = json!(1.0);
        doc["audio"]["music"]["gain_db"] = json!(-6.0);
        std::fs::write(&file, doc.to_string()).unwrap();
        let project = lahza_motion_core::io::load(&project_dir).unwrap();
        let compiled = Compiled::new(&project);
        let placed = clips(&project, &compiled, &project_dir);
        assert_eq!(placed.len(), 1);
        let clip = &placed[0].1;
        assert!((clip.start - 0.5).abs() < 1e-9);
        assert!((clip.source_start - 1.0).abs() < 1e-9);
        // Cut at the project end: 2 s − 0.5 s.
        assert!((clip.duration - 1.5).abs() < 0.01, "{}", clip.duration);
        assert!((clip.volume - 0.501).abs() < 0.01, "{}", clip.volume);
        let waves = waves(&project, &compiled, &project_dir, &PeakCache::default());
        assert_eq!(waves.len(), 1);
        assert!(waves[0].peak(0.0, 0.4) == 0.0, "silent before the music starts");
        assert!(waves[0].peak(1.0, 1.2) > 0.9, "full level mid-tone");
        assert!((waves[0].duration - 1.5).abs() < 0.01);
        std::fs::remove_dir_all(&dir).ok();
    }
}
