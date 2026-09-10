//! Non-destructive preview transport: seek the original media, including its
//! audio, to each retained range. Only export materializes the composition.
use super::{
    clips::RecordingClipTimeline,
    video::{decode_dimensions, present_frame, probe_media, DecodedFrame, VideoError, PLAYBACK_FLAGS},
};
use gstreamer::{self as gst, prelude::*};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct TimelinePlaybackStream {
    path: PathBuf,
    timeline: RecordingClipTimeline,
    position: f64,
    width: u32,
    height: u32,
    muted: bool,
    window_capture: bool,
    pipeline: Option<gst::Element>,
    sink: Option<gst::Element>,
    active: Option<usize>,
    gap_clock: Option<(Instant, f64)>,
    #[cfg(test)]
    audio_samples: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
}

impl TimelinePlaybackStream {
    pub fn open(
        path: &Path,
        mut timeline: RecordingClipTimeline,
        start: f64,
        maximum_width: u32,
        maximum_height: u32,
        muted: bool,
    ) -> Result<Self, VideoError> {
        gst::init().map_err(|e| VideoError::Decode(e.to_string()))?;
        let info = probe_media(path)?;
        let (width, height) =
            decode_dimensions(&info, maximum_width, maximum_height);
        timeline.segments = timeline.playback_ranges();
        Ok(Self {
            path: path.to_owned(),
            position: start.clamp(0.0, timeline.duration()),
            timeline,
            width,
            height,
            muted,
            window_capture: info.window_capture,
            pipeline: None,
            sink: None,
            active: None,
            gap_clock: None,
            #[cfg(test)]
            audio_samples: Default::default(),
        })
    }

    fn start_clip(&mut self, index: usize, source_time: f64) -> Result<(), VideoError> {
        self.stop();
        let clip = &self.timeline.segments[index];
        // Screen captures are variable-rate: a static desktop can have only
        // one frame per second. Repeat its last picture at a regular cadence
        // so the playhead and linked webcam keep following the audio clock.
        // Scale before videorate so repeated frames reuse the resized buffer.
        // Dimensions are already fitted. Padding for subpixel aspect-ratio
        // rounding would add opaque black edges to transparent windows.
        let sink = gst::parse::bin_from_description(&format!(
            "videoconvert ! videoscale add-borders=false ! video/x-raw,format=RGBA,width={},height={},pixel-aspect-ratio=1/1 ! videorate ! video/x-raw,framerate=30/1 ! appsink name=frames sync=true max-buffers=2 drop=false wait-on-eos=false", self.width, self.height
        ), true).map_err(|e| VideoError::Decode(e.to_string()))?;
        let frames = sink.by_name("frames").unwrap();
        let uri = gst::glib::filename_to_uri(&self.path, None)
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        // Rate seeks alone resample audio and shift the speaker's pitch.
        // Scaletempo time-stretches samples to the segment rate while keeping
        // their pitch, matching the atempo filter used for export.
        let tempo = gst::ElementFactory::make("scaletempo")
            .build()
            .map_err(|e| VideoError::Decode(format!("could not preserve audio pitch: {e}")))?;
        let pipeline = gst::ElementFactory::make("playbin")
            .property_from_str("flags", PLAYBACK_FLAGS)
            .property("uri", uri.as_str())
            .property("video-sink", &sink)
            .property("audio-filter", &tempo)
            .property("mute", self.muted)
            .build()
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        #[cfg(test)]
        {
            let audio_sink = gst::parse::bin_from_description(
                "audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=48000,channels=1 ! fakesink name=audio sync=true signal-handoffs=true", true).unwrap();
            let samples = self.audio_samples.clone();
            audio_sink
                .by_name("audio")
                .unwrap()
                .connect("handoff", false, move |values| {
                    let buffer = values[1].get::<gst::Buffer>().unwrap();
                    let map = buffer.map_readable().unwrap();
                    samples.lock().unwrap().extend(
                        map.chunks_exact(4)
                            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())),
                    );
                    None
                });
            pipeline.set_property("audio-sink", audio_sink);
        }
        // Store before changing state so every failure is cleaned up by Drop.
        self.pipeline = Some(pipeline.clone());
        self.sink = Some(frames);
        self.active = Some(index);
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        pipeline
            .state(gst::ClockTime::from_seconds(5))
            .0
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        pipeline
            .seek(
                clip.speed,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds((source_time * 1e9) as u64),
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds((clip.source_end * 1e9) as u64),
            )
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        Ok(())
    }

    /// Poll in short intervals so pause/edit cancels even when media stalls.
    pub fn next_frame(
        &mut self,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<DecodedFrame>, VideoError> {
        loop {
            if cancelled() || self.position >= self.timeline.duration() {
                return Ok(None);
            }
            let location = self.timeline.location_at(self.position).unwrap();
            if self.position < location.editor_start {
                self.stop();
                let (clock, start) = *self
                    .gap_clock
                    .get_or_insert((Instant::now(), self.position));
                std::thread::sleep(Duration::from_millis(16));
                self.position = (start + clock.elapsed().as_secs_f64()).min(location.editor_start);
                return Ok(Some(DecodedFrame {
                    time: self.position,
                    width: self.width,
                    height: self.height,
                    rgba: [0, 0, 0, 255].repeat(self.width as usize * self.height as usize),
                }));
            }
            self.gap_clock = None;
            if self.active != Some(location.segment_index) {
                self.start_clip(location.segment_index, location.source_time)?;
            }
            let sink = self.sink.as_ref().unwrap();
            let sample =
                sink.emit_by_name::<Option<gst::Sample>>("try-pull-sample", &[&100_000_000u64]);
            if let Some(sample) = sample {
                let buffer = sample
                    .buffer()
                    .ok_or_else(|| VideoError::Decode("preview sample has no buffer".into()))?;
                let pts = buffer
                    .pts()
                    .map(|t| t.nseconds() as f64 / 1e9)
                    .ok_or_else(|| VideoError::Decode("preview frame has no timestamp".into()))?;
                let clip = &self.timeline.segments[location.segment_index];
                if pts < clip.source_start || pts >= clip.source_end {
                    continue;
                }
                let time = location.editor_start + (pts - clip.source_start) / clip.speed;
                self.position = time.max(self.position);
                let map = buffer
                    .map_readable()
                    .map_err(|e| VideoError::Decode(e.to_string()))?;
                let expected = self.width as usize * self.height as usize * 4;
                if map.len() != expected {
                    return Err(VideoError::Decode("invalid preview frame size".into()));
                }
                return Ok(Some(present_frame(DecodedFrame {
                    time: self.position, width: self.width, height: self.height, rgba: map.to_vec(),
                }, self.window_capture)));
            }
            if let Some(message) = self
                .pipeline
                .as_ref()
                .unwrap()
                .bus()
                .unwrap()
                .pop_filtered(&[gst::MessageType::Error])
            {
                if let gst::MessageView::Error(error) = message.view() {
                    return Err(VideoError::Decode(error.error().to_string()));
                }
            }
            if sink.property::<bool>("eos") {
                self.position = location.editor_start
                    + self.timeline.segments[location.segment_index].editor_duration();
                self.stop();
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        self.sink = None;
        self.active = None;
    }
}
impl Drop for TimelinePlaybackStream {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::super::clips::RecordingClipSegment;
    use super::*;
    use std::{fs, process::Command};

    #[test]
    #[ignore = "set LAHZA_SYNC_RECORDING to a recording package for real-media diagnostics"]
    fn real_recording_webcam_cadence_on_restart() {
        let root = PathBuf::from(std::env::var_os("LAHZA_SYNC_RECORDING").expect("recording package"));
        let edit: serde_json::Value = serde_json::from_slice(&fs::read(root.join("edit.json")).unwrap()).unwrap();
        let timeline = RecordingClipTimeline::new(serde_json::from_value(edit["clips"].clone()).unwrap());
        for restart in 0..2 {
            let mut stream = TimelinePlaybackStream::open(&root.join("screen.mkv"), timeline.clone(), 0.0, 1920, 1080, false).unwrap();
            let mut camera = crate::recording::camera_playback::CameraPlaybackDecoder::new(root.join("camera.mkv"));
            camera.frame_at(timeline.content_source_time_at(0.0), || false).unwrap();
            let started = Instant::now();
            let mut times = Vec::new();
            while let Some(frame) = stream.next_frame(|| started.elapsed() > Duration::from_secs(20)).unwrap() {
                let source_time = timeline.content_source_time_at(frame.time).unwrap();
                let (camera_time, _) = camera.frame_at(Some(source_time), || false).unwrap().unwrap();
                assert!((source_time - camera_time).abs() < 0.035);
                times.push(frame.time);
                if frame.time >= 12.0 { break; }
            }
            stream.stop();
            let max_gap = times.windows(2).map(|t| t[1] - t[0]).fold(0.0_f64, f64::max);
            eprintln!("restart {restart}: {} paired frames over {:.2}s; maximum timestamp gap {:.4}s; wall {:.2}s", times.len(), times.last().unwrap(), max_gap, started.elapsed().as_secs_f64());
            assert!(*times.last().unwrap() >= 12.0);
            assert!(max_gap < 0.05);
        }
    }

    #[test]
    fn sparse_screen_frames_keep_playhead_and_webcam_advancing_on_every_restart() {
        let root = std::env::temp_dir().join(format!("lahza-sparse-playback-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("screen.mkv");
        let output = Command::new("ffmpeg").args([
            "-v", "error", "-y", "-f", "lavfi", "-i", "color=red:s=64x36:r=1:d=3",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
            "-c:v", "libvpx", "-deadline", "realtime", "-c:a", "pcm_s16le",
        ]).arg(&path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let camera_path = root.join("camera.mkv");
        let output = Command::new("ffmpeg").args([
            "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=64x36:r=15:d=3",
            "-c:v", "ffv1",
        ]).arg(&camera_path).output().unwrap();
        assert!(output.status.success());
        for _restart in 0..2 {
            let mut clip = RecordingClipSegment::new(0.0, 2.0);
            clip.speed = 1.25;
            let timeline = RecordingClipTimeline::new(vec![clip]);
            let mut stream = TimelinePlaybackStream::open(&path, timeline.clone(), 0.0, 64, 36, false).unwrap();
            let mut camera = crate::recording::camera_playback::CameraPlaybackDecoder::new(camera_path.clone());
            camera.frame_at(Some(0.0), || false).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut times = Vec::new();
            while let Some(frame) = stream.next_frame(|| Instant::now() > deadline).unwrap() {
                let source_time = timeline.content_source_time_at(frame.time).unwrap();
                let (camera_time, _) = camera.frame_at(Some(source_time), || false).unwrap().unwrap();
                assert!((source_time - camera_time).abs() < 0.035);
                times.push(frame.time);
            }
            assert!(Instant::now() < deadline);
            assert!(times.len() >= 50, "only {} updates from sparse screen frames", times.len());
            assert!(times.windows(2).all(|t| t[1] - t[0] < 0.05), "playhead stalled: {times:?}");
            stream.stop();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn virtual_playback_skips_deleted_media_and_handles_speed_gaps_and_seek() {
        let root = std::env::temp_dir().join(format!("lahza-virtual-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("source.mkv");
        let output = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x36:r=20:d=1",
                "-f",
                "lavfi",
                "-i",
                "color=green:s=64x36:r=20:d=1",
                "-f",
                "lavfi",
                "-i",
                "color=blue:s=64x36:r=20:d=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-filter_complex",
                "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v]",
                "-map",
                "[v]",
                "-map",
                "3:a",
                "-c:v",
                "libvpx",
                "-deadline",
                "realtime",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let full = RecordingClipTimeline::full(3.0);
        let (split, _) = full.split_at(1.0).unwrap();
        let (split, _) = split.split_at(2.0).unwrap();
        let mut clips = split.deleting(split.segments[1].id).unwrap();
        clips.segments[1].speed = 2.0;
        clips.segments[1].gap_before = 0.15;
        let mut stream =
            TimelinePlaybackStream::open(&path, clips.clone(), 0.5, 64, 36, false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut red = 0;
        let mut blue = 0;
        let mut black = 0;
        let mut previous = 0.5;
        while let Some(frame) = stream.next_frame(|| Instant::now() > deadline).unwrap() {
            assert!(frame.time >= previous);
            previous = frame.time;
            let pixel = &frame.rgba[..4];
            assert!(
                pixel[1] < 50,
                "deleted green video leaked into preview: {pixel:?}"
            );
            if pixel[0] > 200 {
                red += 1;
                assert!(frame.time < 1.0);
            } else if pixel[2] > 200 {
                blue += 1;
                assert!(frame.time >= 1.15 && frame.time < 1.65);
            } else {
                black += 1;
            }
        }
        assert!(Instant::now() < deadline, "playback stalled");
        assert!(
            red > 0 && blue > 0 && black > 0,
            "missing content: {red}/{blue}/{black}"
        );
        assert!((stream.position - clips.duration()).abs() < 1e-6);
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "preview must never render temporary media"
        );
        let mut seek = TimelinePlaybackStream::open(&path, clips, 1.3, 64, 36, true).unwrap();
        let frame = seek
            .next_frame(|| Instant::now() > deadline)
            .unwrap()
            .unwrap();
        assert!(frame.rgba[2] > 200);
        assert!(frame.time >= 1.3);
        seek.stop();
        stream.stop();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn speed_changes_preserve_audio_pitch() {
        let root = std::env::temp_dir().join(format!("lahza-pitch-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("tone.mkv");
        let output = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x36:r=20:d=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=2",
                "-c:v",
                "libvpx",
                "-deadline",
                "realtime",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for speed in [0.5, 1.0, 1.5, 2.0, 4.0, 16.0] {
            let mut clip = RecordingClipSegment::new(0.0, 2.0);
            clip.speed = speed;
            let mut stream = TimelinePlaybackStream::open(
                &path,
                RecordingClipTimeline::new(vec![clip]),
                0.0,
                64,
                36,
                false,
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while stream
                .next_frame(|| Instant::now() > deadline)
                .unwrap()
                .is_some()
            {}
            assert!(Instant::now() < deadline, "playback stalled at {speed}x");
            stream.stop();
            let samples = stream.audio_samples.lock().unwrap();
            // Check time stretching as well as sample pitch: without the
            // filter, a rate seek merely plays unchanged samples faster.
            let audio_duration = samples.len() as f64 / 48000.0;
            assert!(
                (audio_duration - 2.0 / speed).abs() < 0.15,
                "{speed}x produced {audio_duration:.3}s of audio instead of {:.3}s",
                2.0 / speed
            );
            // Count rising zero crossings away from startup/end transients.
            let samples = &samples[samples.len() / 5..samples.len() * 4 / 5];
            assert!(samples.len() > 1000, "missing audio at {speed}x");
            let cycles = samples
                .windows(2)
                .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
                .count();
            let pitch = cycles as f64 * 48000.0 / samples.len() as f64;
            assert!(
                (pitch - 440.0).abs() < 20.0,
                "{speed}x changed pitch to {pitch:.1} Hz"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adjacent_splits_share_one_playback_range() {
        let clip = RecordingClipSegment::new(0.0, 7200.0);
        let timeline = RecordingClipTimeline::new(vec![clip]);
        let (split, _) = timeline.split_at(3600.0).unwrap();
        assert_eq!(split.duration(), timeline.duration());
        assert_eq!(split.playback_ranges().len(), 1);
        assert!(split.same_source_mapping(&timeline));
        let deleted = split.deleting(split.segments[0].id).unwrap();
        assert!(!deleted.same_source_mapping(&timeline));
        for t in [0.0, 3599.0, 3600.0, 7199.0] {
            assert_eq!(
                split.content_source_time_at(t),
                timeline.content_source_time_at(t)
            );
        }
    }
}
