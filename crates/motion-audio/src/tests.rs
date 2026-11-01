//! Synthetic tracks with known structure. The study numbers (Motion Array
//! 90 BPM with its first downbeat at 1.093 s, IMEX 110 BPM, BeyondTheBlack
//! 133.3 BPM triplet feel, BOUNC impact onsets) are rebuilt as test signals;
//! `reference_videos` checks the real files when they are on this machine.

use super::*;

const SR: u32 = SAMPLE_RATE;

struct Synth {
    buf: Vec<f32>,
    seed: u64,
}

impl Synth {
    fn new(seconds: f64) -> Self {
        Self { buf: vec![0.0; (seconds * SR as f64) as usize], seed: 12345 }
    }
    fn noise(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }
    fn kick(&mut self, t: f64, amp: f32) {
        let start = (t * SR as f64) as usize;
        let mut phase = 0.0f32;
        for i in 0..(0.35 * SR as f64) as usize {
            let s = i as f32 / SR as f32;
            let f = 45.0 + 70.0 * (-s * 30.0).exp();
            phase += std::f32::consts::TAU * f / SR as f32;
            if let Some(v) = self.buf.get_mut(start + i) {
                *v += amp * phase.sin() * (-s * 9.0).exp();
            }
        }
    }
    fn hat(&mut self, t: f64, amp: f32) {
        let start = (t * SR as f64) as usize;
        let mut prev = 0.0;
        for i in 0..(0.06 * SR as f64) as usize {
            let n = self.noise();
            let hp = n - prev;
            prev = n;
            let s = i as f32 / SR as f32;
            if let Some(v) = self.buf.get_mut(start + i) {
                *v += amp * hp * (-s * 60.0).exp();
            }
        }
    }
    fn tone(&mut self, from: f64, to: f64, hz: f32, amp: f32) {
        let (a, b) = ((from * SR as f64) as usize, ((to * SR as f64) as usize).min(self.buf.len()));
        for i in a..b {
            self.buf[i] += amp * (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin();
        }
    }
}

fn track<'a>(a: &'a Analysis, name: &str) -> &'a MarkerTrack {
    a.tracks.iter().find(|t| t.name == name).unwrap()
}

/// Largest distance from each expected time to the nearest marker.
fn grid_error(markers: &MarkerTrack, expected: impl Iterator<Item = f64>) -> f64 {
    expected
        .map(|e| markers.markers.iter().map(|m| (m.t - e).abs()).fold(f64::INFINITY, f64::min))
        .fold(0.0, f64::max)
}

#[test]
fn motion_array_grid_90_bpm_first_downbeat() {
    let beat = 60.0 / 90.0;
    let first = 1.093;
    let mut s = Synth::new(20.0);
    s.tone(0.2, 20.0, 330.0, 0.03);
    for k in 0..27 {
        let t = first + k as f64 * beat;
        match k % 4 {
            0 => s.kick(t, 0.9),
            2 => s.kick(t, 0.5),
            _ => s.hat(t, 0.5),
        }
        s.hat(t + beat / 2.0, 0.25);
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert!((a.bpm - 90.0).abs() < 0.5, "{a:?}");
    // ±1 frame at 25 fps.
    assert!((a.first_downbeat - first).abs() < 0.04, "first downbeat {} vs {first}", a.first_downbeat);
    let beats = track(&a, "beat");
    let err = grid_error(beats, (0..27).map(|k| first + k as f64 * beat));
    assert!(err < 0.02, "beat grid error {err}");
    assert_eq!(a.feel, "straight");
    assert_eq!(track(&a, "bar").markers[0].t, a.first_downbeat);
}

#[test]
fn imex_110_bpm() {
    let beat = 60.0 / 110.0;
    let mut s = Synth::new(16.0);
    for k in 0..28 {
        let t = 0.6 + k as f64 * beat;
        if k % 4 == 0 { s.kick(t, 0.9) } else { s.hat(t, 0.4) }
        s.hat(t + beat / 2.0, 0.2);
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert!((a.bpm - 110.0).abs() < 0.5, "{}", a.bpm);
    assert!((a.first_downbeat - 0.6).abs() < 0.04, "{}", a.first_downbeat);
}

#[test]
fn beyond_the_black_triplet_feel() {
    let beat = 0.45006; // 133.3 BPM
    let first = 0.2546;
    let mut s = Synth::new(17.0);
    for k in 0..36 {
        let t = first + k as f64 * beat;
        if k % 4 == 0 {
            s.kick(t, 0.8);
        }
        s.hat(t, 0.45);
        s.hat(t + beat / 3.0, 0.4);
        s.hat(t + 2.0 * beat / 3.0, 0.4);
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert!((a.bpm - 133.3).abs() < 0.6, "{}", a.bpm);
    assert_eq!(a.feel, "triplet", "{:?}", a.phase_strength);
    assert!((a.first_downbeat - first).abs() < 0.034, "{}", a.first_downbeat);
    let triplets = track(&a, "triplet");
    let err = grid_error(triplets, (0..30).map(|k| first + k as f64 * beat / 3.0));
    assert!(err < 0.02, "triplet grid error {err}");
}

#[test]
fn stops_drops_and_hits() {
    let beat = 0.5;
    let mut s = Synth::new(12.0);
    // A bass bed that drops out for the beat before bar 5's downbeat.
    let bar5 = 0.5 + 16.0 * beat;
    s.tone(0.5, bar5 - beat, 55.0, 0.25);
    s.tone(bar5, 11.0, 55.0, 0.25);
    for k in 0..20 {
        let t = 0.5 + k as f64 * beat;
        if t > 6.0 && t < 6.6 {
            continue;
        }
        if k % 4 == 0 { s.kick(t, 0.6) } else { s.hat(t, 0.3) }
    }
    // Silence then an impact (BOUNC-style stop → hit).
    let hit = 11.2;
    for v in &mut s.buf[(10.6 * SR as f64) as usize..(hit * SR as f64) as usize] {
        *v = 0.0;
    }
    s.kick(hit, 1.0);
    s.hat(hit, 1.0);
    let a = analyze(&s.buf, SR, 0.0);
    let stop = track(&a, "stop");
    assert!(stop.markers.iter().any(|m| (m.t - hit).abs() < 0.02), "{stop:?}");
    let hits = track(&a, "hit");
    let largest = hits.markers.iter().find(|m| m.label.as_deref() == Some("largest")).unwrap();
    assert!((largest.t - hit).abs() < 0.02, "{hits:?}");
    let drop = track(&a, "drop");
    assert!(drop.markers.iter().any(|m| (m.t - bar5).abs() < 0.03), "{drop:?} {:?}", track(&a, "bar").markers.iter().map(|m| m.t).collect::<Vec<_>>());
    // Onset times are sharp: every kick has an onset within 10 ms.
    let onsets = track(&a, "onset");
    let err = grid_error(onsets, (0..12).map(|k| 0.5 + k as f64 * beat));
    assert!(err < 0.012, "onset error {err}");
}

#[test]
fn decode_through_ffmpeg() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping: no ffmpeg");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lahza-motion-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clicks.wav");
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=1000:beep_factor=8:duration=6", "-ar", "44100"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    let a = analyze_file(&path, 2.0).unwrap();
    // The beeps repeat every second: 60 BPM, or its double.
    assert!((a.bpm - 60.0).abs() < 1.0 || (a.bpm - 120.0).abs() < 2.0, "{}", a.bpm);
    assert!(a.music_start >= 2.0, "offset applies");
    std::fs::remove_dir_all(dir).ok();
}

/// The study reference videos, when present in ~/Downloads.
#[test]
#[ignore]
fn reference_videos() {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = Path::new(&home).join("Downloads");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        let expect = if name.contains("motion") && name.contains("array") {
            Some((90.0, Some(1.093)))
        } else if name.contains("imex") {
            Some((110.0, None))
        } else if name.contains("black") {
            Some((133.3, Some(0.255)))
        } else {
            None
        };
        let Some((bpm, downbeat)) = expect else { continue };
        let a = analyze_file(&e.path(), 0.0).unwrap();
        eprintln!("{name}: {} BPM (expected {bpm}), first downbeat {} ({downbeat:?}), feel {}", a.bpm, a.first_downbeat, a.feel);
        assert!((a.bpm - bpm).abs() < 1.0);
    }
}

