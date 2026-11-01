//! `playback` tests that also drive higher layers (export, media),
//! so they live in the crate that depends on all of them.

use crate::playback::*;
use crate::clips::{RecordingClipSegment, RecordingClipTimeline};
use crate::video::probe_media;
use std::{fs, path::Path, process::Command, time::{Duration, Instant}};

#[test]
fn prepared_audio_is_shared_by_preview_and_export_across_cuts_and_speed() {
    use crate::{audio_processing, export};
    let root = std::env::temp_dir().join(format!("lahza-audio-consumers-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("separate.mkv");
    let output = Command::new("ffmpeg").args([
        "-v", "error", "-y", "-f", "lavfi", "-i", "color=red:s=64x36:r=30:d=3",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=3",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000:duration=3",
        "-map", "0:v", "-map", "1:a", "-map", "2:a", "-c:v", "ffv1", "-c:a", "pcm_s16le",
        "-metadata:s:a:0", "title=Lahza-Microphone", "-metadata:s:a:1", "title=Lahza-System-Audio",
    ]).arg(&path).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let bank_path = root.join("takes.media.json");
    let entry = crate::media_bank::MediaEntry {
        file: "separate.mkv".into(), info: probe_media(&path).unwrap(), still: false,
    };
    crate::media_bank::MediaBank { entries: vec![entry.clone(), entry] }
        .save(&bank_path).unwrap();
    let decode = |path: &Path| -> Vec<f32> {
        let output = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path)
            .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "pipe:1"]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout.chunks_exact(4).map(|v| f32::from_le_bytes(v.try_into().unwrap())).collect()
    };
    let amplitude = |samples: &[f32], frequency: f64| -> f64 {
        let (sin, cos) = samples.iter().enumerate().fold((0.0, 0.0), |(s, c), (i, x)| {
            let angle = std::f64::consts::TAU * frequency * i as f64 / 48000.0;
            (s + f64::from(*x) * angle.sin(), c + f64::from(*x) * angle.cos())
        });
        sin.hypot(cos) * 2.0 / samples.len() as f64
    };
    for (index, audio_mode) in crate::audio_mode::AudioMode::ALL.into_iter().enumerate() {
        let noise_reduction = index % 2 != 0; // Exercise both direct media and media-bank source offsets.
        let media = if noise_reduction { &bank_path } else { &path };
        let control = audio_processing::ProcessingControl::default();
        let prepared = audio_processing::ensure_audio(&path, audio_mode, &control).unwrap().unwrap();
        let reference = decode(&prepared);
        assert!(amplitude(&reference[24000..72000], 880.0) > 0.05, "system audio disappeared");
        let simple = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.5, 1.5)]);
        let exported = root.join(format!("simple-{}.wav", audio_mode.cache_key()));
        export::export_audio(export::AudioExportRequest {
            destination: exported.clone(), format: export::AudioFormat::Wav,
            media: media.clone(), clips: simple.clone(), include_recording: true,
            noise_reduction: false, audio_mode: Some(audio_mode), audio_clips: Vec::new(),
        }, &export::ExportProgress::default()).unwrap();
        let samples = decode(&exported);
        assert_eq!(samples.len(), 48000);
        // One clean take feeds export before cutting. A second adaptive
        // denoise pass, or using the embedded first track, fails this.
        let error = samples.iter().zip(&reference[24000..72000])
            .map(|(a, b)| f64::from((a - b).abs())).sum::<f64>() / 48000.0;
        assert!(error < 0.00005, "export did not reuse prepared samples: {error}");

        let mut first = RecordingClipSegment::new(0.4, 1.2);
        first.speed = 2.0;
        // Cleaned playback crosses a virtual media-bank asset boundary;
        // each FLAC still uses local source time rather than bank time.
        let offset = if noise_reduction { 3.0 } else { 0.0 };
        let mut second = RecordingClipSegment::new(1.8 + offset, 2.6 + offset);
        second.gap_before = 0.15;
        let timeline = RecordingClipTimeline::new(vec![first, second]);
        let mut preview = TimelinePlaybackStream::open_with_audio_processing(
            media, timeline.clone(), 0.0, 64, 36, false, audio_mode, control,
        ).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut frames = Vec::new();
        loop {
            match preview.next_frame(|| Instant::now() > deadline) {
                Ok(Some(frame)) => frames.push(frame.time),
                Ok(None) => break,
                Err(error) => panic!("prepared mode={audio_mode:?} failed: {error}; frames={frames:?}"),
            }
        }
        assert!(Instant::now() < deadline, "prepared mode={audio_mode:?} stalled: frames={frames:?}");
        preview.stop();
        let played = preview.audio_samples.lock().unwrap();
        assert!((played.len() as f64 / 48000.0 - 1.2).abs() < 0.16,
            "prepared audio was not cut/time-stretched: {} samples", played.len());
        assert!(amplitude(&played[played.len()/3..played.len()*2/3], 880.0) > 0.025,
            "preview lost system audio or altered its pitch");
        assert!(frames.windows(2).all(|pair| pair[1] >= pair[0]));
        assert!(frames.last().copied().unwrap_or(0.0) > timeline.duration() - 0.1);
        drop(played);
        drop(preview);
    }
    // Other consumers must not silently choose only microphone stream 0.
    let session = crate::project::RecordingSession::create_in(root.join("projects")).unwrap();
    let imported = crate::media::import_audio(&session, &path, 0.0).unwrap();
    let imported = decode(&crate::media::asset_path(&session, &imported.file).unwrap());
    assert!(amplitude(&imported[24000..72000], 440.0) > 0.05);
    assert!(amplitude(&imported[24000..72000], 880.0) > 0.05);
    let clip_export = root.join("clip.mp4");
    crate::export::export_clip_timeline(&path, &clip_export,
        &RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.5, 1.5)])).unwrap();
    let clip_audio = decode(&clip_export);
    assert!(amplitude(&clip_audio[12000..36000], 440.0) > 0.05);
    assert!(amplitude(&clip_audio[12000..36000], 880.0) > 0.05);
    fs::remove_dir_all(root).unwrap();
}
