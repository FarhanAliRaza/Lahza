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


#[test]
fn accelerando_follows_the_tempo() {
    // 100 → 132 BPM over 16 s: a fixed grid drifts by beats; the tracker follows.
    let mut s = Synth::new(17.0);
    let mut t = 0.5;
    let mut truth = Vec::new();
    let mut k = 0;
    while t < 16.5 {
        truth.push(t);
        if k % 4 == 0 { s.kick(t, 0.9) } else { s.kick(t, 0.45) }
        let bpm = 100.0 + 32.0 * (t / 16.5);
        let beat = 60.0 / bpm;
        s.hat(t + beat / 2.0, 0.25);
        t += beat;
        k += 1;
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert!(a.tempo_varies, "{:?}", a.summary());
    let beats = track(&a, "beat");
    let inner: Vec<f64> = truth[2..truth.len() - 2].to_vec();
    if std::env::var_os("LAHZA_AUDIO_DEBUG").is_some() {
        eprintln!("truth {:?}", truth.iter().map(|t| (t * 1000.0).round() / 1000.0).collect::<Vec<_>>());
        eprintln!("beats {:?}", beats.markers.iter().map(|m| (m.t * 1000.0).round() / 1000.0).collect::<Vec<_>>());
    }
    let err = grid_error(beats, inner.into_iter());
    assert!(err < 0.03, "beat error {err}");
    assert!((a.first_downbeat - 0.5).abs() < 0.04, "{}", a.first_downbeat);
}

#[test]
fn waltz_has_three_beats_per_bar() {
    let beat = 60.0 / 150.0;
    let mut s = Synth::new(14.0);
    for k in 0..32 {
        let t = 0.4 + k as f64 * beat;
        if k % 3 == 0 { s.kick(t, 0.9) } else { s.hat(t, 0.4) }
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert_eq!(a.beats_per_bar, 3, "{:?}", a.summary());
    assert!((a.first_downbeat - 0.4).abs() < 0.04, "{}", a.first_downbeat);
    let bars = track(&a, "bar");
    assert!((bars.markers[1].t - (0.4 + 3.0 * beat)).abs() < 0.03);
}

impl Synth {
    fn snare(&mut self, t: f64, amp: f32) {
        let start = (t * SR as f64) as usize;
        for i in 0..(0.2 * SR as f64) as usize {
            let n = self.noise();
            let s = i as f32 / SR as f32;
            if let Some(v) = self.buf.get_mut(start + i) {
                *v += amp * (0.7 * n * (-s * 25.0).exp() + 0.5 * (std::f32::consts::TAU * 190.0 * s).sin() * (-s * 30.0).exp());
            }
        }
    }
    fn pluck(&mut self, t: f64, hz: f32, amp: f32) {
        let start = (t * SR as f64) as usize;
        for i in 0..(0.12 * SR as f64) as usize {
            let s = i as f32 / SR as f32;
            if let Some(v) = self.buf.get_mut(start + i) {
                *v += amp * (std::f32::consts::TAU * hz * s).sin() * (-s * 40.0).exp();
            }
        }
    }
    /// Decaying noise from `t`: a reverb tail or a ringing hit.
    fn tail(&mut self, t: f64, seconds: f64, amp: f32, db_per_s: f32) {
        let start = (t * SR as f64) as usize;
        let k = db_per_s / 8.686;
        let mut low = 0.0f32;
        for i in 0..(seconds * SR as f64) as usize {
            let n = self.noise();
            low += 0.05 * (n - low);
            let s = i as f32 / SR as f32;
            if let Some(v) = self.buf.get_mut(start + i) {
                *v += amp * (0.5 * n + 2.0 * low) * (-s * k).exp();
            }
        }
    }
    fn cut(&mut self, t: f64) {
        let a = ((t * SR as f64) as usize).min(self.buf.len());
        for v in &mut self.buf[a..] {
            *v = 0.0;
        }
    }
}

/// The reference opener's music: an even stream of triplet eighths
/// (150 ms) with a soft kick every fourth one. Grouped in 3 it is 133.3 BPM
/// in triplets, in 4 it is 100 BPM in sixteenths; neither grouping is
/// accented, so the tempo nearer the preferred one wins, with low
/// confidence and the other reading listed.
#[test]
fn even_triplet_stream_reads_as_133_triplet() {
    let (first, tatum) = (0.25, 0.45006 / 3.0);
    let mut s = Synth::new(16.0);
    let notes = [440.0, 554.0, 659.0, 880.0];
    for k in 0..100 {
        let t = first + k as f64 * tatum;
        s.hat(t, 0.4);
        s.pluck(t, notes[k % 4], 0.25);
        if k % 4 == 0 {
            s.kick(t, 0.3);
        }
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert!((a.bpm - 133.3).abs() < 0.6, "{:?}", a.summary());
    assert_eq!(a.feel, "triplet", "{:?}", a.summary());
    assert!(a.tempo_confidence < 0.5, "{:?}", a.summary());
    assert!(a.alternatives.iter().any(|alt| (alt.bpm - 100.0).abs() < 1.0 && alt.feel == "straight"), "{:?}", a.summary());
    let err = grid_error(track(&a, "beat"), (0..30).map(|k| first + k as f64 * 3.0 * tatum));
    assert!(err < 0.02, "beat grid error {err}");
    let err = grid_error(track(&a, "triplet"), (0..90).map(|k| first + k as f64 * tatum));
    assert!(err < 0.02, "triplet grid error {err}");
}

/// The same 150 ms pulse played as straight sixteenths at 100 BPM: kick on
/// 1 and 3, snare on 2 and 4, hats accented on the eighths. The beats are
/// accented, so it stays 100 BPM straight.
#[test]
fn straight_sixteenths_with_a_backbeat_stay_straight() {
    let (first, beat) = (0.3, 0.6);
    let mut s = Synth::new(16.0);
    for k in 0..25 {
        let t = first + k as f64 * beat;
        if k % 2 == 0 { s.kick(t, 0.8) } else { s.snare(t, 0.5) }
        for q in 0..4 {
            s.hat(t + q as f64 * beat / 4.0, if q % 2 == 0 { 0.35 } else { 0.2 });
        }
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert!((a.bpm - 100.0).abs() < 0.5, "{:?}", a.summary());
    assert_eq!(a.feel, "straight", "{:?}", a.summary());
    assert!(a.tempo_confidence > 0.5, "{:?}", a.summary());
    assert!((a.first_downbeat - first).abs() < 0.04, "{}", a.first_downbeat);
}

/// A shuffle (the Lahza opener's groove): kick on 1 and 3, snare on 2 and
/// 4, hats on the beat and its last third only. One third is enough for a
/// triplet feel.
#[test]
fn shuffle_is_a_triplet_feel() {
    let (first, beat) = (0.217, 0.4503);
    let mut s = Synth::new(16.0);
    for k in 0..32 {
        let t = first + k as f64 * beat;
        if k % 2 == 0 { s.kick(t, 0.9) } else { s.snare(t, 0.5) }
        s.hat(t, 0.35);
        s.hat(t + 2.0 * beat / 3.0, 0.3);
    }
    let a = analyze(&s.buf, SR, 0.0);
    assert!((a.bpm - 133.3).abs() < 0.6, "{:?}", a.summary());
    assert_eq!(a.feel, "triplet", "{:?}", a.summary());
    assert!(a.feel_confidence > 0.3, "{:?}", a.summary());
    assert!((a.first_downbeat - first).abs() < 0.034, "{}", a.first_downbeat);
}

/// Hard stops: the music cuts on a hit and only a tail follows, a quiet
/// reverb fading out or a loud hit ringing on. Either is a `stop` at the
/// cut, though the tail never falls silent.
#[test]
fn hard_stop_with_a_tail() {
    let beat = 0.5;
    for (name, amp, db_per_s) in [("reverb", 0.25, 30.0), ("ring-out", 0.9, 4.0)] {
        let mut s = Synth::new(12.0);
        let cut = 0.4 + 16.0 * beat + beat / 2.0;
        for k in 0..20 {
            let t = 0.4 + k as f64 * beat;
            if k % 2 == 0 { s.kick(t, 0.8) } else { s.snare(t, 0.5) }
            s.hat(t + beat / 2.0, 0.3);
            s.tone(t, t + beat, 110.0, 0.1);
        }
        s.cut(cut);
        s.kick(cut, 0.9);
        s.tail(cut, 3.0, amp, db_per_s);
        let a = analyze(&s.buf, SR, 0.0);
        let stop = track(&a, "stop");
        assert_eq!(stop.markers.len(), 1, "{name}: {stop:?}");
        assert!((stop.markers[0].t - cut).abs() < 0.03, "{name}: {stop:?}");
        assert!(stop.markers[0].label.as_deref().unwrap_or("").contains("cuts"), "{name}: {stop:?}");
    }
}

/// The two opener tracks, when they are on this machine: the reference
/// opener's music and the Lahza opener's own track (133.3 BPM triplet feel,
/// a hard stop with a reverb tail at 17.02 s).
#[test]
#[ignore]
fn reference_openers() {
    let home = std::env::var("HOME").unwrap_or_default();
    let cases = [
        (format!("{home}/Videos/replica/ref-audio.wav"), 0.2546, 0.45006, None),
        (format!("{home}/Videos/lahza-opener/track.wav"), 0.217, 0.4503, Some(7.421)),
    ];
    let mut failures = Vec::new();
    for (path, first, beat, drop) in cases {
        let path = Path::new(&path);
        if !path.exists() {
            continue;
        }
        let a = analyze_file(path, 0.0).unwrap();
        let mut s = a.summary();
        s["tracks"] = serde_json::Value::Null;
        eprintln!("{}: {s}", path.display());
        for name in ["beat", "bar", "stop", "drop", "hit"] {
            eprintln!("  {name}: {:?}", track(&a, name).markers.iter().take(12).map(|m| (m.t * 1000.0).round() / 1000.0).collect::<Vec<_>>());
        }
        let name = path.display();
        let err = grid_error(track(&a, "beat"), (0..30).map(|k| first + k as f64 * beat));
        let checks = [
            ((a.beat - beat).abs() < 0.003, format!("{name}: beat {} vs {beat}", a.beat)),
            (a.feel == "triplet", format!("{name}: feel {}", a.feel)),
            (err < 0.02, format!("{name}: beat grid error {err}")),
            (track(&a, "stop").markers.iter().any(|m| (m.t - 17.02).abs() < 0.04), format!("{name}: no stop at 17.02")),
            (drop.is_none_or(|d| track(&a, "drop").markers.iter().any(|m| (m.t - d).abs() < 0.03)), format!("{name}: no drop at {drop:?}")),
        ];
        failures.extend(checks.into_iter().filter(|c| !c.0).map(|c| c.1));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
