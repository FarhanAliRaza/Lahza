//! `media` tests that also drive higher layers (export),
//! so they live in the crate that depends on all of them.

use crate::media::*;
use crate::{
    clips::{RecordingClipSegment, RecordingClipTimeline},
    export::{export_scene, ExportFormat, ExportProgress, SceneExportRequest, SceneSource},
    project::RecordingSession,
    scene_model::SceneStyle,
    video::probe_media,
    viewport::ViewportTimeline,
};
use std::{fs, path::PathBuf, process::Command};
use uuid::Uuid;

struct Project(RecordingSession);
impl Project {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("lahza-media-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        Self(RecordingSession { directory })
    }
    fn tone(&self, name: &str, frequency: u32) -> PathBuf {
        let path = self.0.directory.join(name);
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={frequency}:duration=0.6:sample_rate=48000"
            ))
            .args(["-c:a", "pcm_s16le", "-y"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    }
    fn video(&self, name: &str, color: &str) -> PathBuf {
        let path = self.0.directory.join(name);
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("color=c={color}:s=64x36:r=30:d=0.6"))
            .args(["-c:v", "ffv1", "-y"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0.directory);
    }
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn separate_assets_support_seek_speed_gaps_playback_and_export_audio() {
    use crate::{playback::TimelinePlaybackStream, video::VideoFrameStream};
    let project = Project::new();
    project.video("screen.mkv", "red");
    let photo = project.0.directory.join("wide.png");
    image::RgbaImage::from_pixel(100, 20, image::Rgba([0, 255, 0, 255]))
        .save(&photo)
        .unwrap();
    let (first, image_start, _) = append_media(&project.0, &photo).unwrap();
    project.0.write_edit_field("mediaSource", &first).unwrap();
    let added = project.0.directory.join("square.mkv");
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=blue:s=48x48:r=30:d=0.8",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:duration=0.8:sample_rate=48000",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-y",
        ])
        .arg(&added)
        .output()
        .unwrap();
    assert!(output.status.success());
    let (file, video_start, _) = append_media(&project.0, &added).unwrap();
    let bank = asset_path(&project.0, &file).unwrap();
    let mut video = RecordingClipSegment::new(video_start + 0.2, video_start + 0.6);
    video.speed = 2.0;
    video.gap_before = 0.1;
    let still = RecordingClipSegment::new(image_start + 0.1, image_start + 0.5);
    let mut original = RecordingClipSegment::new(0.1, 0.3);
    original.speed = 0.5;
    let timeline = RecordingClipTimeline::new(vec![video, still, original]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut playback =
        TimelinePlaybackStream::open(&bank, timeline.clone(), 0.0, 64, 36, false).unwrap();
    let mut seen = [false; 4];
    while let Some(frame) = playback
        .next_frame(|| std::time::Instant::now() > deadline)
        .unwrap()
    {
        let center = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
        let p = &frame.rgba[center..center + 3];
        if p[2] > 200 {
            seen[1] = true;
            assert!(frame.time >= 0.1 && frame.time < 0.31);
        } else if p[1] > 200 {
            seen[2] = true;
            assert!(frame.time >= 0.3 && frame.time < 0.71);
        } else if p[0] > 200 {
            seen[3] = true;
            assert!(frame.time >= 0.7 - 1e-8);
        } else {
            seen[0] = true;
        }
    }
    assert!(
        std::time::Instant::now() < deadline,
        "playback stalled at an asset boundary"
    );
    assert_eq!(seen, [true; 4]);
    playback.stop();
    let mut seek =
        TimelinePlaybackStream::open(&bank, timeline.clone(), 0.45, 64, 36, true).unwrap();
    let frame = seek.next_frame(|| false).unwrap().unwrap();
    assert!(frame.rgba[(18 * 64 + 32) * 4 + 1] > 200);
    seek.stop();
    let mut frames = VideoFrameStream::open_timeline(&bank, &timeline, 64, 36, 30.0).unwrap();
    let mut count = 0;
    let mut seen = [false; 3];
    while let Some(frame) = frames.next_frame().unwrap() {
        let p = &frame.rgba[(18 * 64 + 32) * 4..][..3];
        if p[2] > 200 {
            seen[0] = true;
        }
        if p[1] > 200 {
            seen[1] = true;
        }
        if p[0] > 200 {
            seen[2] = true;
        }
        count += 1;
    }
    assert_eq!(seen, [true; 3]);
    assert!(
        (count as f64 - timeline.duration() * 30.0).abs() <= 1.0,
        "{count} frames"
    );
    frames.stop();
    let music = project.tone("music.wav", 440);
    let clip = import_audio(&project.0, &music, 0.0).unwrap();
    let mut request = SceneExportRequest::new(
        project.0.directory.join("export.mp4"),
        ExportFormat::Mp4,
        36,
        SceneStyle::default(),
        ViewportTimeline::default(),
        timeline.duration(),
    );
    request.frame_rate = 30.0;
    request.audio_clips = vec![(asset_path(&project.0, &clip.file).unwrap(), clip)];
    export_scene(
        SceneSource::Video {
            media: bank,
            clips: timeline,
            pointer: None,
            camera: None,
        },
        &mut request,
        &ExportProgress::default(),
    )
    .unwrap();
    let info = probe_media(&request.destination).unwrap();
    assert!(info.has_audio);
    assert!((info.duration - 1.1).abs() < 0.1);
    let audio = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&request.destination)
        .args(["-vn", "-ar", "48000", "-ac", "1", "-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(audio.status.success());
    let samples: Vec<f32> = audio
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let spectral = |frequency: f64| {
        let slice = &samples[7200..12000];
        let (mut re, mut im) = (0.0, 0.0);
        for (i, sample) in slice.iter().enumerate() {
            let angle = std::f64::consts::TAU * frequency * i as f64 / 48000.0;
            re += *sample as f64 * angle.cos();
            im += *sample as f64 * angle.sin();
        }
        (re * re + im * im).sqrt() / slice.len() as f64
    };
    assert!(spectral(440.0) > 0.01, "missing background audio");
    assert!(
        spectral(880.0) > 0.02,
        "missing imported audio or changed pitch"
    );
}

#[test]
fn background_audio_is_mixed_with_the_recording_and_can_outlive_it() {
    let project = Project::new();
    let recorded = project.0.screen_path();
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=64x36:r=30:d=1.2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.8:sample_rate=48000",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-y",
        ])
        .arg(&recorded)
        .output()
        .unwrap();
    assert!(output.status.success());
    let music = project.tone("music.wav", 880);
    let clip = import_audio(&project.0, &music, 0.6).unwrap();
    let mut request = SceneExportRequest::new(
        project.0.directory.join("with-music.mp4"),
        ExportFormat::Mp4,
        36,
        SceneStyle::default(),
        ViewportTimeline::default(),
        1.2,
    );
    request.frame_rate = 10.0;
    request.audio_clips = vec![(asset_path(&project.0, &clip.file).unwrap(), clip)];
    export_scene(
        SceneSource::Video {
            media: recorded,
            clips: RecordingClipTimeline::full(1.2),
            pointer: None,
            camera: None,
        },
        &mut request,
        &ExportProgress::default(),
    )
    .unwrap();
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&request.destination)
        .args(["-vn", "-ar", "48000", "-ac", "1", "-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let samples: Vec<f32> = output
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let amplitude = |start: f64, frequency: f64| {
        let slice = &samples[(start * 48000.0) as usize..((start + 0.1) * 48000.0) as usize];
        let component = |cos: bool| {
            slice
                .iter()
                .enumerate()
                .map(|(i, sample)| {
                    let phase = std::f64::consts::TAU * frequency * i as f64 / 48000.0;
                    *sample as f64 * if cos { phase.cos() } else { phase.sin() }
                })
                .sum::<f64>()
        };
        component(true).hypot(component(false)) * 2.0 / slice.len() as f64
    };
    assert!(amplitude(0.2, 440.0) > 0.08);
    assert!(amplitude(0.2, 880.0) < 0.005);
    assert!(amplitude(0.65, 440.0) > 0.08);
    assert!(amplitude(0.65, 880.0) > 0.03);
    assert!(
        amplitude(1.0, 880.0) > 0.03,
        "music should continue after recorded audio ends"
    );
    assert!((probe_media(&request.destination).unwrap().duration - 1.2).abs() < 0.1);
}

#[test]
fn background_audio_export_mixes_layers_and_preserves_full_video_duration() {
    let project = Project::new();
    let a = project.tone("first.wav", 440);
    let b = project.tone("second.wav", 880);
    let first = import_audio(&project.0, &a, 0.3).unwrap();
    let mut second = import_audio(&project.0, &b, 0.5).unwrap();
    second.source_start = 0.1;
    second.duration = 0.4;
    let mut request = SceneExportRequest::new(
        project.0.directory.join("mixed.mp4"),
        ExportFormat::Mp4,
        36,
        SceneStyle::default(),
        ViewportTimeline::default(),
        1.3,
    );
    request.frame_rate = 10.0;
    request.audio_clips = vec![
        (asset_path(&project.0, &first.file).unwrap(), first.clone()),
        (
            asset_path(&project.0, &second.file).unwrap(),
            second.clone(),
        ),
    ];
    let source = SceneSource::Image {
        image: image::RgbaImage::from_pixel(64, 36, image::Rgba([0, 0, 0, 255])),
        pointer: None,
    };
    export_scene(source, &mut request, &ExportProgress::default()).unwrap();
    let info = probe_media(&request.destination).unwrap();
    assert!(info.has_audio);
    assert!((info.duration - 1.3).abs() < 0.1, "{}", info.duration);
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&request.destination)
        .args(["-vn", "-ar", "48000", "-ac", "1", "-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let samples: Vec<f32> = output
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let at = |start: f64, end: f64| {
        rms(&samples[(start * 48000.0) as usize..(end * 48000.0) as usize])
    };
    assert!(at(0.05, 0.2) < 0.001, "audio leaked before start");
    let single = at(0.35, 0.45);
    let mixed = at(0.55, 0.75);
    assert!(
        single > 0.015 && mixed > single * 1.25,
        "single {single}, mixed {mixed}"
    );
    assert!(at(1.05, 1.2) < 0.001, "audio leaked after end");
    let mut playback = AudioPlayback::open(request.audio_clips.clone()).unwrap();
    playback.sync(0.4).unwrap();
    playback.sync(0.7).unwrap();
    playback.sync(1.0).unwrap();
    project
        .0
        .write_edit_field("audioClips", &vec![first.clone(), second.clone()])
        .unwrap();
    project.0.commit_draft().unwrap();
    assert_eq!(
        project
            .0
            .read_edit_field::<Vec<AudioClip>>("audioClips")
            .unwrap()
            .unwrap(),
        vec![first, second]
    );
}
