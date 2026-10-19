//! Project-owned imports and editor-time audio layers. Source recordings are immutable.
use super::{
    model::RecordingSession,
    video::{probe_media, VideoError},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioClip {
    pub id: Uuid,
    pub file: PathBuf,
    pub name: String,
    pub start: f64,
    pub source_start: f64,
    pub duration: f64,
    pub volume: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    /// Length of the imported asset; 0 when unknown (older projects).
    #[serde(default)]
    pub source_duration: f64,
}

impl AudioClip {
    pub fn validate(&self) -> Result<(), VideoError> {
        if ![
            self.start,
            self.source_start,
            self.duration,
            self.volume,
            self.fade_in,
            self.fade_out,
        ]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0)
            || self.duration <= 0.0
            || self.volume > 2.0
        {
            return Err(VideoError::InvalidMedia(
                "invalid audio clip timing or volume".into(),
            ));
        }
        Ok(())
    }

    pub fn gain_at(&self, time: f64) -> f64 {
        let local = time - self.start;
        if local < 0.0 || local >= self.duration {
            return 0.0;
        }
        let fade_in = if self.fade_in > 0.0 {
            (local / self.fade_in.min(self.duration)).min(1.0)
        } else {
            1.0
        };
        let fade_out = if self.fade_out > 0.0 {
            ((self.duration - local) / self.fade_out.min(self.duration)).min(1.0)
        } else {
            1.0
        };
        self.volume * fade_in * fade_out
    }

    pub fn filter(&self, input: usize, label: &str) -> String {
        let mut chain = format!("[{input}:a]atrim=start={:.9}:duration={:.9},asetpts=PTS-STARTPTS,aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume={:.6}", self.source_start, self.duration, self.volume);
        if self.fade_in > 0.0 {
            chain.push_str(&format!(
                ",afade=t=in:d={:.9}",
                self.fade_in.min(self.duration)
            ));
        }
        if self.fade_out > 0.0 {
            let fade = self.fade_out.min(self.duration);
            chain.push_str(&format!(
                ",afade=t=out:st={:.9}:d={fade:.9}",
                self.duration - fade
            ));
        }
        chain.push_str(&format!(
            ",adelay={}S:all=1[{label}]",
            (self.start * 48000.0).round() as u64
        ));
        chain
    }
}

/// Resolve only relative project assets; moving the project keeps imports usable.
pub fn asset_path(session: &RecordingSession, file: &Path) -> Result<PathBuf, VideoError> {
    if file
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
        || file.as_os_str().is_empty()
    {
        return Err(VideoError::InvalidMedia(
            "invalid project media path".into(),
        ));
    }
    Ok(session.directory.join(file))
}

pub fn source_path(session: &RecordingSession) -> Result<PathBuf, VideoError> {
    match session.read_edit_field::<PathBuf>("mediaSource")? {
        Some(file) => asset_path(session, &file),
        None => Ok(session.screen_path()),
    }
}

fn run(mut command: Command, destination: &Path) -> Result<(), VideoError> {
    let result = command.args(["-y"]).arg(destination).output()?;
    if !result.status.success() {
        let _ = fs::remove_file(destination);
        return Err(VideoError::Decode(
            String::from_utf8_lossy(&result.stderr).trim().into(),
        ));
    }
    Ok(())
}

pub fn import_audio(
    session: &RecordingSession,
    source: &Path,
    start: f64,
) -> Result<AudioClip, VideoError> {
    fs::create_dir_all(session.directory.join("media"))?;
    let id = Uuid::new_v4();
    let file = PathBuf::from(format!("media/{id}.wav"));
    let destination = asset_path(session, &file)?;
    let mut command = Command::new("ffmpeg");
    command
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(source)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-c:a",
            "pcm_s16le",
        ]);
    run(command, &destination)?;
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(&destination)
        .output()?;
    let duration = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .map_err(|_| VideoError::InvalidMedia("audio has no valid duration".into()))?;
    let clip = AudioClip {
        id,
        file,
        name: source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        start,
        source_start: 0.0,
        duration,
        volume: 0.5,
        fade_in: 0.0,
        fade_out: 0.0,
        source_duration: duration,
    };
    clip.validate()?;
    Ok(clip)
}

/// Register an immutable, project-owned asset without re-encoding any footage.
pub fn append_media(
    session: &RecordingSession,
    source: &Path,
) -> Result<(PathBuf, f64, f64), VideoError> {
    use super::media_bank::{MediaBank, MediaEntry};
    let original = source_path(session)?;
    let mut bank = match MediaBank::load(&original)? {
        Some(bank) => bank,
        None => MediaBank {
            entries: vec![MediaEntry {
                file: original
                    .strip_prefix(&session.directory)
                    .map_err(|_| VideoError::InvalidMedia("source is outside project".into()))?
                    .into(),
                info: probe_media(&original)?,
                still: false,
            }],
        },
    };
    let start = bank.info().duration;
    let dimensions = image::image_dimensions(source).ok();
    let info = if let Some((width, height)) = dimensions {
        super::video::MediaInfo {
            width,
            height,
            duration: 5.0,
            frame_rate: 30.0,
            has_audio: false,
            window_capture: false,
        }
    } else {
        probe_media(source)?
    };
    if !info.duration.is_finite() || info.duration <= 0.0 {
        return Err(VideoError::InvalidMedia("media has no duration".into()));
    }
    let end = start + info.duration;
    fs::create_dir_all(session.directory.join("media"))?;
    let id = Uuid::new_v4();
    let extension = source.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    // Some image formats (notably GIF/AVIF) use video demuxers in FFmpeg,
    // which cannot loop a still. Normalize those once to a single PNG frame.
    let normalize_still = dimensions.is_some()
        && !matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff"
        );
    let extension = if normalize_still { "png" } else { extension };
    let asset = PathBuf::from(format!("media/{id}.{extension}"));
    let destination = asset_path(session, &asset)?;
    // Keep a project-owned copy so moving/deleting the original cannot break edits.
    let copied = if normalize_still {
        image::open(source)
            .and_then(|image| image.save_with_format(&destination, image::ImageFormat::Png))
            .map_err(|e| VideoError::Decode(e.to_string()))
    } else {
        fs::copy(source, &destination)
            .map(|_| ())
            .map_err(VideoError::from)
    };
    if let Err(error) = copied {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    bank.entries.push(MediaEntry {
        file: asset,
        info,
        still: dimensions.is_some(),
    });
    let file = PathBuf::from(format!(".source-{id}.media.json"));
    if let Err(error) = bank.save(&asset_path(session, &file)?) {
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    // The new index supersedes the previous one (and its denoised twin);
    // the assets they list are all still referenced by the new index.
    if MediaBank::load(&original)?.is_some() {
        let _ = fs::remove_file(&original);
        let _ = fs::remove_file(original.with_extension("denoised.media.json"));
    }
    Ok((file, start, end))
}

/// Independent editor-time layers keep their pitch and placement when video
/// clips are cut, reordered or sped up. The video playhead remains the clock.
pub struct AudioPlayback {
    layers: Vec<AudioLayer>,
}

struct AudioLayer {
    clip: AudioClip,
    pipeline: gstreamer::Element,
    gain: gstreamer::Element,
    playing: bool,
    #[cfg(test)]
    samples: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
}

impl AudioPlayback {
    pub fn open(clips: Vec<(PathBuf, AudioClip)>) -> Result<Self, VideoError> {
        use gstreamer::{self as gst, prelude::*};
        gst::init().map_err(|e| VideoError::Decode(e.to_string()))?;
        let mut playback = Self { layers: Vec::new() };
        for (path, clip) in clips {
            clip.validate()?;
            let uri = gst::glib::filename_to_uri(path, None)
                .map_err(|e| VideoError::Decode(e.to_string()))?;
            // Playbin delegates its volume property to sinks such as PulseAudio,
            // even with soft-volume enabled. Keep fades in an explicit filter so
            // one layer cannot alter the desktop volume used by another stream.
            let gain = gst::ElementFactory::make("volume")
                .property("volume", 0.0_f64)
                .build()
                .map_err(|e| VideoError::Decode(e.to_string()))?;
            let pipeline = gst::ElementFactory::make("playbin")
                .property("uri", uri.as_str())
                .property_from_str("flags", "audio+soft-volume+force-filters")
                .property("audio-filter", &gain)
                .property("volume", 1.0_f64)
                .property("mute", false)
                .build()
                .map_err(|e| VideoError::Decode(e.to_string()))?;
            #[cfg(test)]
            let samples = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            #[cfg(test)]
            pipeline.set_property(
                "audio-sink",
                super::playback::test_audio_sink(samples.clone()),
            );
            playback.layers.push(AudioLayer {
                clip,
                pipeline: pipeline.clone(),
                gain,
                playing: false,
                #[cfg(test)]
                samples,
            });
            pipeline
                .set_state(gst::State::Paused)
                .map_err(|e| VideoError::Decode(e.to_string()))?;
            pipeline
                .state(gst::ClockTime::from_seconds(5))
                .0
                .map_err(|e| VideoError::Decode(e.to_string()))?;
        }
        Ok(playback)
    }

    pub fn sync(&mut self, time: f64) -> Result<(), VideoError> {
        use gstreamer::{self as gst, prelude::*};
        for AudioLayer {
            clip,
            pipeline,
            gain,
            playing,
            ..
        } in &mut self.layers
        {
            let active = time >= clip.start && time < clip.start + clip.duration;
            if active {
                let target = clip.source_start + time - clip.start;
                let actual = pipeline
                    .query_position::<gst::ClockTime>()
                    .map(|p| p.nseconds() as f64 / 1e9)
                    .unwrap_or(0.0);
                if !*playing || (actual - target).abs() > 0.15 {
                    pipeline
                        .seek_simple(
                            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                            gst::ClockTime::from_nseconds((target * 1e9) as u64),
                        )
                        .map_err(|e| VideoError::Decode(e.to_string()))?;
                }
                gain.set_property("volume", clip.gain_at(time));
                if !*playing {
                    pipeline
                        .set_state(gst::State::Playing)
                        .map_err(|e| VideoError::Decode(e.to_string()))?;
                }
            } else if *playing {
                pipeline
                    .set_state(gst::State::Paused)
                    .map_err(|e| VideoError::Decode(e.to_string()))?;
            }
            *playing = active;
        }
        Ok(())
    }
}

impl Drop for AudioPlayback {
    fn drop(&mut self) {
        use gstreamer::prelude::*;
        for layer in &self.layers {
            let _ = layer.pipeline.set_state(gstreamer::State::Null);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{
        clips::{RecordingClipSegment, RecordingClipTimeline},
        export::{export_scene, ExportFormat, ExportProgress, SceneExportRequest, SceneSource},
        scene::SceneStyle,
        video::decode_frame,
        viewport::ViewportTimeline,
    };

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

    #[test]
    fn imported_media_extends_timeline_survives_reopen_and_keeps_original() {
        let project = Project::new();
        let screen = project.video("screen.mkv", "red");
        let original = fs::read(&screen).unwrap();
        let photo = project.0.directory.join("photo.png");
        image::RgbaImage::from_pixel(32, 18, image::Rgba([0, 255, 0, 255]))
            .save(&photo)
            .unwrap();
        let (file, start, end) = append_media(&project.0, &photo).unwrap();
        assert!((start - 0.6).abs() < 0.04);
        assert!((end - start - 5.0).abs() < 0.04);
        project.0.write_edit_field("mediaSource", &file).unwrap();
        let clips = RecordingClipTimeline::new(vec![
            RecordingClipSegment::new(0.0, 0.6),
            RecordingClipSegment::new(start, end),
        ]);
        project.0.write_clip_timeline_draft(&clips).unwrap();
        project.0.commit_draft().unwrap();
        fs::remove_file(photo).unwrap();
        let bank = source_path(&project.0).unwrap();
        let red = decode_frame(&bank, 0.2, 64, 36).unwrap();
        assert!(red.rgba[0] > 200 && red.rgba[1] < 30);
        let green = decode_frame(&bank, start + 0.2, 64, 36).unwrap();
        assert!(green.rgba[1] > 200 && green.rgba[0] < 30);
        assert_eq!(project.0.effective_clip_timeline(end).unwrap(), clips);
        let blue = project.video("blue.mkv", "blue");
        let (next, next_start, next_end) = append_media(&project.0, &blue).unwrap();
        assert!((next_start - end).abs() < 0.04);
        assert!(next_end > end);
        let frame = decode_frame(
            &asset_path(&project.0, &next).unwrap(),
            next_start + 0.2,
            64,
            36,
        )
        .unwrap();
        assert!(frame.rgba[2] > 200 && frame.rgba[0] < 30);
        assert_eq!(fs::read(screen).unwrap(), original);
    }

    #[test]
    fn imports_copy_only_new_assets_and_do_not_decode_the_existing_timeline() {
        use super::super::media_bank::MediaBank;
        let project = Project::new();
        let screen = project.video("screen.mkv", "red");
        let added = project.video("added.mkv", "blue");
        let bytes = fs::read(&added).unwrap();
        let began = std::time::Instant::now();
        let (file, _, _) = append_media(&project.0, &added).unwrap();
        eprintln!("video import: {:?}", began.elapsed());
        let bank = MediaBank::load(&asset_path(&project.0, &file).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::read(asset_path(&project.0, &bank.entries[1].file).unwrap()).unwrap(),
            bytes
        );
        assert_eq!(
            fs::read_dir(project.0.directory.join("media"))
                .unwrap()
                .count(),
            1
        );
        project.0.write_edit_field("mediaSource", &file).unwrap();
        // Cached metadata is enough: no earlier video is opened or rewritten
        // by later imports, even if it is currently unavailable.
        fs::remove_file(screen).unwrap();
        fs::remove_file(asset_path(&project.0, &bank.entries[1].file).unwrap()).unwrap();
        let photo = project.0.directory.join("photo.png");
        image::RgbaImage::from_pixel(160, 90, image::Rgba([0, 255, 0, 255]))
            .save(&photo)
            .unwrap();
        let began = std::time::Instant::now();
        let (next, start, end) = append_media(&project.0, &photo).unwrap();
        eprintln!("image import after existing assets: {:?}", began.elapsed());
        assert!((start - 1.2).abs() < 0.04);
        assert!((end - start - 5.0).abs() < 1e-6);
        let next = MediaBank::load(&asset_path(&project.0, &next).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::read(asset_path(&project.0, &next.entries[2].file).unwrap()).unwrap(),
            fs::read(photo).unwrap()
        );
        assert_eq!(
            fs::read_dir(project.0.directory.join("media"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn gif_image_import_remains_a_still_and_exports() {
        use super::super::{media_bank::MediaBank, video::VideoFrameStream};
        let project = Project::new();
        project.video("screen.mkv", "red");
        let gif = project.0.directory.join("still.gif");
        image::RgbaImage::from_pixel(40, 40, image::Rgba([0, 255, 0, 255]))
            .save(&gif)
            .unwrap();
        let (file, start, _) = append_media(&project.0, &gif).unwrap();
        let path = asset_path(&project.0, &file).unwrap();
        let bank = MediaBank::load(&path).unwrap().unwrap();
        assert_eq!(bank.entries[1].file.extension().unwrap(), "png");
        let timeline =
            RecordingClipTimeline::new(vec![RecordingClipSegment::new(start, start + 0.2)]);
        let mut stream = VideoFrameStream::open_timeline(&path, &timeline, 64, 36, 30.0).unwrap();
        let mut count = 0;
        while let Some(frame) = stream.next_frame().unwrap() {
            assert!(frame.rgba[(18 * 64 + 32) * 4 + 1] > 200);
            count += 1;
        }
        assert_eq!(count, 6);
    }

    #[test]
    fn legacy_media_sources_and_noise_cache_remain_usable_after_import() {
        use super::super::{media_bank::MediaBank, video::render_denoised_copy};
        let project = Project::new();
        project.video("screen.mkv", "red");
        let legacy = project.0.directory.join("legacy.mkv");
        let output = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=green:s=64x36:r=30:d=0.6",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.6",
                "-c:v",
                "ffv1",
                "-c:a",
                "pcm_s16le",
                "-y",
            ])
            .arg(&legacy)
            .output()
            .unwrap();
        assert!(output.status.success());
        project
            .0
            .write_edit_field("mediaSource", &PathBuf::from("legacy.mkv"))
            .unwrap();
        let cached = project.0.denoised_path();
        render_denoised_copy(&legacy, &cached).unwrap();
        let modified = fs::metadata(&cached).unwrap().modified().unwrap();
        let blue = project.video("blue.mkv", "blue");
        let (file, start, end) = append_media(&project.0, &blue).unwrap();
        let path = asset_path(&project.0, &file).unwrap();
        let green = decode_frame(&path, 0.2, 64, 36).unwrap();
        assert!(green.rgba[1] > 100 && green.rgba[0] < 30);
        project.0.write_edit_field("mediaSource", &file).unwrap();
        let destination = project.0.denoised_path();
        assert_ne!(destination, cached);
        // Reuse the previous recording's noise-reduced audio instead of
        // processing that entire recording again after each import.
        fs::remove_file(&legacy).unwrap();
        render_denoised_copy(&path, &destination).unwrap();
        assert_eq!(fs::metadata(cached).unwrap().modified().unwrap(), modified);
        let bank = MediaBank::load(&destination).unwrap().unwrap();
        assert_eq!(
            bank.entries[0].file,
            PathBuf::from(RecordingSession::DENOISED_FILE)
        );
        assert!((bank.info().duration - end).abs() < 1e-9);
        let blue = decode_frame(&destination, start + 0.2, 64, 36).unwrap();
        assert!(blue.rgba[2] > 200);
    }

    #[test]
    fn separate_assets_support_seek_speed_gaps_playback_and_export_audio() {
        use super::super::{playback::TimelinePlaybackStream, video::VideoFrameStream};
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
    fn appending_to_a_transparent_window_keeps_its_presentation() {
        let project = Project::new();
        let mut window = image::RgbaImage::new(64, 36);
        for y in 8..28 {
            for x in 8..56 {
                window.put_pixel(x, y, image::Rgba([230, 30, 40, 255]));
            }
        }
        let png = project.0.directory.join("window.png");
        window.save(&png).unwrap();
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-loop", "1", "-i"])
            .arg(&png)
            .args([
                "-t", "0.6", "-r", "30", "-c:v", "ffv1", "-pix_fmt", "bgra", "-y",
            ])
            .arg(project.0.screen_path())
            .output()
            .unwrap();
        assert!(output.status.success());
        fs::write(project.0.screen_path().with_extension("window.json"), b"{}").unwrap();
        let original = decode_frame(&project.0.screen_path(), 0.2, 64, 36).unwrap();
        let added = project.video("added.mkv", "blue");
        let (file, start, _) = append_media(&project.0, &added).unwrap();
        let bank = asset_path(&project.0, &file).unwrap();
        let preserved = decode_frame(&bank, 0.2, 64, 36).unwrap();
        assert_eq!(
            (preserved.width, preserved.height),
            (original.width, original.height)
        );
        assert_eq!(preserved.rgba, original.rgba);
        let imported = decode_frame(&bank, start + 0.2, 64, 36).unwrap();
        assert_eq!((imported.width, imported.height), (64, 36));
        assert!(imported.rgba[2] > 200);
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

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
    }

    #[test]
    fn background_playback_gain_and_recording_mute_are_independent() {
        use super::super::playback::TimelinePlaybackStream;
        use gstreamer::prelude::*;
        let project = Project::new();
        let recorded = project.0.screen_path();
        let output = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x36:r=30:d=0.6",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.6:sample_rate=48000",
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
        let imported = import_audio(&project.0, &music, 0.0).unwrap();
        let mut results = Vec::new();
        for (mute_recording, volume) in [(false, 1.0), (false, 0.25), (true, 0.25), (false, 0.0)] {
            let mut clip = imported.clone();
            clip.volume = volume;
            let mut future = imported.clone();
            future.id = Uuid::new_v4();
            future.start = 2.0;
            let mut music = AudioPlayback::open(vec![
                (asset_path(&project.0, &clip.file).unwrap(), clip),
                (asset_path(&project.0, &future.file).unwrap(), future),
            ])
            .unwrap();
            let mut video = TimelinePlaybackStream::open(
                &recorded,
                RecordingClipTimeline::full(0.6),
                0.0,
                64,
                36,
                mute_recording,
            )
            .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while let Some(frame) = video
                .next_frame(|| std::time::Instant::now() > deadline)
                .unwrap()
            {
                music.sync(frame.time).unwrap();
                for layer in &music.layers {
                    // The sink may share desktop volume state with the video:
                    // fades and inactive clips must never set that gain to zero.
                    assert_eq!(layer.pipeline.property::<f64>("volume"), 1.0);
                    assert!(!layer.pipeline.property::<bool>("mute"));
                }
            }
            assert!(std::time::Instant::now() < deadline);
            video.stop();
            music.sync(0.7).unwrap();
            let middle_rms = |samples: &[f32]| {
                assert!(samples.len() > 12000, "missing audio buffers");
                let slice = &samples[samples.len() / 2..samples.len() * 4 / 5];
                (slice.iter().map(|v| v * v).sum::<f32>() / slice.len() as f32).sqrt()
            };
            let recording_level = middle_rms(&video.audio_samples.lock().unwrap());
            let music_level = middle_rms(&music.layers[0].samples.lock().unwrap());
            assert!(
                music.layers[1].samples.lock().unwrap().is_empty(),
                "future audio started early"
            );
            results.push((recording_level, music_level));
        }
        assert!(
            results[0].0 > 0.04 && results[0].1 > 0.04,
            "both tracks must be audible: {results:?}"
        );
        assert!(
            (results[1].0 / results[0].0 - 1.0).abs() < 0.02,
            "music volume changed recording level: {results:?}"
        );
        assert!(
            (results[1].1 / results[0].1 - 0.25).abs() < 0.04,
            "music volume was not applied to samples: {results:?}"
        );
        assert!(
            results[2].0 < 0.00001 && results[2].1 > 0.01,
            "recording mute silenced music: {results:?}"
        );
        assert!(
            results[3].0 > 0.04 && results[3].1 < 0.00001,
            "silent music silenced recording: {results:?}"
        );
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

    #[test]
    fn audio_fades_and_invalid_assets_are_bounded() {
        let project = Project::new();
        assert!(asset_path(&project.0, Path::new("../outside.wav")).is_err());
        assert!(asset_path(&project.0, Path::new("/tmp/outside.wav")).is_err());
        let tone = project.tone("tone.wav", 440);
        let mut clip = import_audio(&project.0, &tone, 1.0).unwrap();
        clip.duration = 4.0;
        clip.fade_in = 1.0;
        clip.fade_out = 1.0;
        assert_eq!(clip.gain_at(0.9), 0.0);
        assert_eq!(clip.gain_at(1.5), 0.25);
        assert_eq!(clip.gain_at(3.0), 0.5);
        assert_eq!(clip.gain_at(4.5), 0.25);
        assert_eq!(clip.gain_at(5.0), 0.0);
        clip.start = f64::NAN;
        assert!(clip.validate().is_err());
    }
}
