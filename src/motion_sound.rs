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

/// Where each sound of the project plays, with its file probed.
pub(crate) fn clips(project: &Project, compiled: &Compiled, dir: &Path) -> Vec<(PathBuf, AudioClip)> {
    let end = project.settings.duration;
    let mut out = Vec::new();
    let mut push = |path: PathBuf, at: f64, trim: f64, gain_db: f64, length: Option<f64>, name: &str| {
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
        out.push((
            path.clone(),
            AudioClip {
                id: uuid::Uuid::new_v4(),
                file: path,
                name: name.to_string(),
                start,
                source_start,
                duration,
                volume: 10f64.powf(gain_db / 20.0).min(2.0),
                fade_in: 0.0,
                fade_out: 0.0,
                source_duration,
            },
        ));
    };
    for clip in project.audio.clips() {
        let at = compiled.resolve(&project.markers, &clip.at).unwrap_or(0.0);
        let name = clip.name.clone().or(clip.id.clone()).unwrap_or_else(|| clip.path.clone());
        push(dir.join(&clip.path), at, clip.trim, clip.gain_db, clip.length, &name);
    }
    for footage in compiled.footage_audio() {
        if (footage.speed - 1.0).abs() < 1e-3 {
            push(dir.join(&*footage.src), footage.start, footage.trim, footage.gain_db, Some(footage.end - footage.start), &footage.src);
        }
    }
    out
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

/// Peak levels of one sound, placed on the timeline.
#[derive(Clone, Debug)]
pub(crate) struct Wave {
    pub start: f64,
    /// 0..1 peaks, `BUCKETS_PER_SECOND` per second.
    pub peaks: Vec<f32>,
}

impl Wave {
    /// The loudest peak between `t0` and `t1` (timeline seconds).
    pub(crate) fn peak(&self, t0: f64, t1: f64) -> f32 {
        let a = ((t0 - self.start) * BUCKETS_PER_SECOND).floor().max(0.0) as usize;
        let b = (((t1 - self.start) * BUCKETS_PER_SECOND).ceil().max(0.0) as usize).min(self.peaks.len());
        self.peaks.get(a..b).map(|s| s.iter().copied().fold(0.0, f32::max)).unwrap_or(0.0)
    }
}

/// Decodes every sound and measures its peaks, normalised so the loudest
/// moment of the project reaches 1.
pub(crate) fn waves(project: &Project, compiled: &Compiled, dir: &Path) -> Vec<Wave> {
    let rate = lahza_motion_audio::SAMPLE_RATE as f64;
    let mut waves = Vec::new();
    for (path, clip) in clips(project, compiled, dir) {
        let Ok(samples) = lahza_motion_audio::decode(&path) else { continue };
        let first = (clip.source_start * rate) as usize;
        let last = (((clip.source_start + clip.duration) * rate) as usize).min(samples.len());
        let Some(range) = samples.get(first..last) else { continue };
        // Bucket edges from exact times, so long sounds don't drift.
        let per = rate / BUCKETS_PER_SECOND;
        let count = (range.len() as f64 / per).round() as usize;
        let gain = clip.volume as f32;
        let peaks = (0..count)
            .map(|i| {
                let (a, b) = ((i as f64 * per) as usize, (((i + 1) as f64 * per) as usize).min(range.len()));
                range[a..b].iter().fold(0.0f32, |m, s| m.max(s.abs())) * gain
            })
            .collect();
        waves.push(Wave { start: clip.start, peaks });
    }
    let loudest = waves.iter().flat_map(|w| w.peaks.iter().copied()).fold(0.0f32, f32::max);
    if loudest > 0.0 {
        for w in &mut waves {
            for p in &mut w.peaks {
                *p /= loudest;
            }
        }
    }
    waves
}

/// A signature of what the sound depends on, to recompute waveforms only
/// when the audio placement changes.
pub(crate) fn signature(project: &Project, compiled: &Compiled) -> String {
    let mut s = serde_json::to_string(&project.audio).unwrap_or_default();
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
        let waves = waves(&project, &compiled, &project_dir);
        assert_eq!(waves.len(), 1);
        assert!(waves[0].peak(0.0, 0.4) == 0.0, "silent before the music starts");
        assert!(waves[0].peak(1.0, 1.2) > 0.9, "full level mid-tone");
        assert_eq!(waves[0].peaks.len(), 150);
        std::fs::remove_dir_all(&dir).ok();
    }
}
