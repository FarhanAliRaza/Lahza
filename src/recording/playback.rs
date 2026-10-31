//! Non-destructive preview transport: seek original video and the shared
//! source-time audio asset to each retained range on one pipeline clock.
//! Only export materializes the edited composition.
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
    bank: Option<super::media_bank::MediaBank>,
    source_offset: f64,
    still_frame: Option<(DecodedFrame, Instant, f64)>,
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
    temporal_frames: Option<super::primary_frames::PrimaryFrames>,
    audio_mode: super::audio_mode::AudioMode,
    audio_control: super::audio_processing::ProcessingControl,
    #[cfg(test)]
    pub(super) audio_samples: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
}

impl TimelinePlaybackStream {
    pub fn open(
        path: &Path,
        timeline: RecordingClipTimeline,
        start: f64,
        maximum_width: u32,
        maximum_height: u32,
        muted: bool,
    ) -> Result<Self, VideoError> {
        Self::open_with_audio_processing(path, timeline, start, maximum_width,
            maximum_height, muted, false, Default::default())
    }

    pub fn open_with_audio_processing(
        path: &Path,
        mut timeline: RecordingClipTimeline,
        start: f64,
        maximum_width: u32,
        maximum_height: u32,
        muted: bool,
        audio_mode: impl Into<super::audio_mode::AudioMode>,
        audio_control: super::audio_processing::ProcessingControl,
    ) -> Result<Self, VideoError> {
        let audio_mode = audio_mode.into();
        gst::init().map_err(|e| VideoError::Decode(e.to_string()))?;
        let info = probe_media(path)?;
        let (width, height) =
            decode_dimensions(&info, maximum_width, maximum_height);
        let bank = super::media_bank::MediaBank::load(path)?;
        timeline.segments = timeline.playback_ranges();
        if let Some(bank) = &bank { timeline = bank.ranges(&timeline); }
        Ok(Self {
            path: path.to_owned(),
            bank,
            source_offset: 0.0,
            still_frame: None,
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
            temporal_frames: None,
            audio_mode,
            audio_control,
            #[cfg(test)]
            audio_samples: Default::default(),
        })
    }

    /// Share encoded source intervals before transport cadence conversion.
    pub fn enable_temporal_frames(&mut self, revision: u64) {
        self.temporal_frames = Some(super::primary_frames::PrimaryFrames::new(
            super::motion_blur::VideoSource { path: self.path.clone(), size: (self.width, self.height), revision },
            (self.width, self.height), self.window_capture));
    }

    fn start_clip(&mut self, index: usize, source_time: f64, cancelled: &dyn Fn() -> bool) -> Result<(), VideoError> {
        self.stop();
        let clip = &self.timeline.segments[index];
        let (path, offset, borders) = if let Some(bank) = &self.bank {
            let (entry, offset) = bank.locate(source_time);
            if bank.entries[entry].still {
                let frame = bank.decode_frame(&self.path, source_time, self.width, self.height)?;
                self.still_frame = Some((frame, Instant::now(), self.position));
                self.active = Some(index);
                return Ok(());
            }
            (bank.entry_path(&self.path, entry), offset, entry != 0)
        } else { (self.path.clone(), 0.0, false) };
        self.source_offset = offset;
        // Screen captures are variable-rate: a static desktop can have only
        // one frame per second. Repeat its last picture at a regular cadence
        // so the playhead and linked webcam keep following the audio clock.
        // Scale before videorate so repeated frames reuse the resized buffer.
        // Dimensions are already fitted. Padding for subpixel aspect-ratio
        // rounding would add opaque black edges to transparent windows.
        let sink = gst::parse::bin_from_description(&format!(
            "videoconvert ! videoscale add-borders={borders} ! video/x-raw,format=RGBA,width={},height={},pixel-aspect-ratio=1/1 ! identity name=native_temporal ! videorate ! video/x-raw,framerate=30/1 ! appsink name=frames sync=true max-buffers=2 drop=false wait-on-eos=false", self.width, self.height
        ), true).map_err(|e| VideoError::Decode(e.to_string()))?;
        let frames = sink.by_name("frames").unwrap();
        let uri = gst::glib::filename_to_uri(&path, None)
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        // Rate seeks alone resample audio and shift the speaker's pitch.
        // Scaletempo time-stretches samples to the segment rate while keeping
        // their pitch, matching the atempo filter used for export.
        let audio_filter = gst::parse::bin_from_description("scaletempo ! volume name=recording_gain", true)
            .map_err(|e| VideoError::Decode(format!("could not prepare recording audio: {e}")))?;
        audio_filter.by_name("recording_gain").unwrap().set_property("mute", self.muted);
        let prepared_audio = if self.muted { None } else {
            super::audio_processing::ensure_audio_with_cancel(
                &path, self.audio_mode, &self.audio_control, cancelled)?
        };
        let pipeline = if let Some(audio) = prepared_audio {
            // Original video and prepared audio live in one pipeline so rate
            // seeks, start time, pause, and the audio clock apply to both.
            let audio_output: gst::Element = {
                #[cfg(test)]
                { test_audio_sink(self.audio_samples.clone()).upcast() }
                #[cfg(not(test))]
                { gst::ElementFactory::make("autoaudiosink").build()
                    .map_err(|e| VideoError::Decode(e.to_string()))? }
            };
            prepared_audio_pipeline(&path, &audio, &sink, &audio_filter, &audio_output)?
        } else {
            let pipeline = gst::ElementFactory::make("playbin")
            .property_from_str("flags", &format!("{PLAYBACK_FLAGS}+force-filters"))
            .property("uri", uri.as_str())
            .property("video-sink", &sink)
            .property("audio-filter", &audio_filter)
            // Reset the stream gain left by older builds; recording mute lives
            // in the filter, so it cannot mute music through a shared sink.
            .property("volume", 1.0_f64)
            .property("mute", false)
            .build()
            .map_err(|e| VideoError::Decode(e.to_string()))?;
            #[cfg(test)]
            pipeline.set_property("audio-sink", test_audio_sink(self.audio_samples.clone()));
            pipeline
        };
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
        if let Some(cache) = self.temporal_frames.clone() {
            let asset = self.bank.as_ref().map_or(0, |bank| bank.locate(source_time).0);
            let bounds = self.bank.as_ref().map_or((0.0, f64::INFINITY), |bank|
                (offset, offset + bank.entries[asset].info.duration));
            let bounds=(bounds.0.max(source_time),bounds.1.min(clip.source_end));
            sink.by_name("native_temporal").unwrap().static_pad("src").unwrap()
                .add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
                    if let Some(buffer) = info.buffer() {
                        let origin = buffer.pts().and_then(|pts| {
                            let event = pad.sticky_event::<gst::event::Segment>(0)?;
                            let segment = event.segment().downcast_ref::<gst::ClockTime>()?;
                            let stream_time = segment.to_stream_time(pts)?;
                            Some(stream_time.nseconds() as f64 / 1e9 - pts.nseconds() as f64 / 1e9)
                        }).unwrap_or(0.0);
                        cache.push(buffer.clone(), asset, offset + origin, bounds, index);
                    }
                    gst::PadProbeReturn::Ok
                });
        }
        pipeline
            .seek(
                clip.speed,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds(((source_time - offset) * 1e9) as u64),
                gst::SeekType::Set,
                gst::ClockTime::from_nseconds(((clip.source_end - offset) * 1e9) as u64),
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
                self.start_clip(location.segment_index, location.source_time, &cancelled)?;
            }
            if let Some((frame, clock, start)) = &self.still_frame {
                std::thread::sleep(Duration::from_millis(16));
                let end: f64 = self.timeline.segments[..=location.segment_index].iter()
                    .map(|clip| clip.slot_duration()).sum();
                self.position = (start + clock.elapsed().as_secs_f64()).min(end);
                let mut frame = frame.clone();
                frame.time = self.position.min(end - 1e-9);
                return Ok(Some(frame));
            }
            let sink = self.sink.as_ref().unwrap();
            let sample =
                sink.emit_by_name::<Option<gst::Sample>>("try-pull-sample", &[&100_000_000u64]);
            if let Some(sample) = sample {
                let buffer = sample
                    .buffer()
                    .ok_or_else(|| VideoError::Decode("preview sample has no buffer".into()))?;
                // Demuxers may preserve a positive container timestamp origin
                // in buffer PTS while seeks use normalized stream time. The
                // prepared audio uses that same normalized source clock.
                let pts = buffer.pts().and_then(|pts| {
                    sample.segment().and_then(|segment| segment.downcast_ref::<gst::ClockTime>())
                        .map_or(Some(pts), |segment| segment.to_stream_time(pts))
                })
                    .map(|t| t.nseconds() as f64 / 1e9 + self.source_offset)
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
                // Use the same accumulation order as timeline lookup/duration.
                // Adding gap + duration in a different order can leave the
                // playhead one ULP before the boundary and reopen an exhausted
                // asset forever (especially with media-bank source offsets).
                self.position = self.timeline.segments[..=location.segment_index].iter()
                    .map(|clip| clip.slot_duration()).sum();
                self.stop();
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        self.still_frame = None;
        self.sink = None;
        self.active = None;
    }
}
impl Drop for TimelinePlaybackStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn prepared_audio_pipeline(
    video: &Path,
    audio: &Path,
    video_sink: &gst::Bin,
    audio_filter: &gst::Bin,
    audio_sink: &gst::Element,
) -> Result<gst::Element, VideoError> {
    let pipeline = gst::Pipeline::new();
    let build_source = |path: &Path, media_type: &str| -> Result<gst::Element, VideoError> {
        let uri = gst::glib::filename_to_uri(path, None)
            .map_err(|e| VideoError::Decode(e.to_string()))?;
        gst::ElementFactory::make("uridecodebin")
            .property("uri", uri.as_str())
            // Decodebin checks decoder template caps before instantiation.
            // ANY features accept decoders advertising both system/GPU memory;
            // the downstream conversion bin negotiates usable CPU buffers.
            .property("caps", gst::Caps::builder(media_type).any_features().build())
            .property("expose-all-streams", false)
            .build().map_err(|e| VideoError::Decode(e.to_string()))
    };
    let video_source = build_source(video, "video/x-raw")?;
    let audio_source = build_source(audio, "audio/x-raw")?;
    let video_queue = gst::ElementFactory::make("queue").build()
        .map_err(|e| VideoError::Decode(e.to_string()))?;
    let deinterlace = gst::ElementFactory::make("deinterlace").build()
        .map_err(|e| VideoError::Decode(e.to_string()))?;
    let audio_queue = gst::ElementFactory::make("queue").build()
        .map_err(|e| VideoError::Decode(e.to_string()))?;
    let convert = gst::ElementFactory::make("audioconvert").build()
        .map_err(|e| VideoError::Decode(e.to_string()))?;
    let resample = gst::ElementFactory::make("audioresample").build()
        .map_err(|e| VideoError::Decode(e.to_string()))?;
    pipeline.add_many([
        &video_source, &audio_source, &video_queue, &deinterlace, &audio_queue, &convert, &resample,
        video_sink.upcast_ref(), audio_filter.upcast_ref(), audio_sink,
    ]).map_err(|e| VideoError::Decode(e.to_string()))?;
    gst::Element::link_many([&video_queue, &deinterlace, video_sink.upcast_ref()])
        .map_err(|e| VideoError::Decode(e.to_string()))?;
    gst::Element::link_many([&audio_queue, &convert, &resample, audio_filter.upcast_ref(), audio_sink])
        .map_err(|e| VideoError::Decode(e.to_string()))?;
    for (source, queue, media_type) in [
        (video_source, video_queue, "video/"), (audio_source, audio_queue, "audio/"),
    ] {
        source.connect_pad_added(move |_, pad| {
            let matches = pad.current_caps().and_then(|caps|
                caps.structure(0).map(|s| s.name().starts_with(media_type))).unwrap_or(false);
            let sink = queue.static_pad("sink").expect("queue has a sink");
            if matches && !sink.is_linked() { let _ = pad.link(&sink); }
        });
    }
    Ok(pipeline.upcast())
}

/// Capture the samples after production audio filters without a sound device.
#[cfg(test)]
pub(super) fn test_audio_sink(samples: std::sync::Arc<std::sync::Mutex<Vec<f32>>>) -> gst::Bin {
    let sink = gst::parse::bin_from_description(
        "audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=48000,channels=1 ! fakesink name=audio sync=true signal-handoffs=true", true).unwrap();
    sink.by_name("audio").unwrap().connect("handoff", false, move |values| {
        let buffer = values[1].get::<gst::Buffer>().unwrap();
        let map = buffer.map_readable().unwrap();
        samples.lock().unwrap().extend(map.chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())));
        None
    });
    sink
}

#[cfg(test)]
mod tests {
    use super::super::clips::RecordingClipSegment;
    use super::*;
    use std::{fs, process::Command};

    #[test]
    fn prepared_audio_is_shared_by_preview_and_export_across_cuts_and_speed() {
        use super::super::{audio_processing, export};
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
        let entry = super::super::media_bank::MediaEntry {
            file: "separate.mkv".into(), info: probe_media(&path).unwrap(), still: false,
        };
        super::super::media_bank::MediaBank { entries: vec![entry.clone(), entry] }
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
        for (index, audio_mode) in super::super::audio_mode::AudioMode::ALL.into_iter().enumerate() {
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
                    Err(error) => panic!("prepared mode={audio_mode:?} failed: {error}; position={}, active={:?}, offset={}, frames={frames:?}", preview.position, preview.active, preview.source_offset),
                }
            }
            assert!(Instant::now() < deadline, "prepared mode={audio_mode:?} stalled: position={}, active={:?}, offset={}, frames={frames:?}", preview.position, preview.active, preview.source_offset);
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
        let session = super::super::model::RecordingSession::create_in(root.join("projects")).unwrap();
        let imported = super::super::media::import_audio(&session, &path, 0.0).unwrap();
        let imported = decode(&super::super::media::asset_path(&session, &imported.file).unwrap());
        assert!(amplitude(&imported[24000..72000], 440.0) > 0.05);
        assert!(amplitude(&imported[24000..72000], 880.0) > 0.05);
        let clip_export = root.join("clip.mp4");
        super::super::video::export_clip_timeline(&path, &clip_export,
            &RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.5, 1.5)])).unwrap();
        let clip_audio = decode(&clip_export);
        assert!(amplitude(&clip_audio[12000..36000], 440.0) > 0.05);
        assert!(amplitude(&clip_audio[12000..36000], 880.0) > 0.05);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_playback_keeps_delayed_microphone_aligned_to_video() {
        let root = std::env::temp_dir().join(format!("lahza-audio-offset-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for origin in [0, 2] {
            let path = root.join(format!("delayed-{origin}.mkv"));
            let output = Command::new("ffmpeg").args([
                "-v", "error", "-y", "-f", "lavfi", "-i", "color=blue:s=64x36:r=30:d=1.5",
                "-itsoffset", "0.35", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=1",
                "-map", "0:v", "-map", "1:a", "-c:v", "ffv1", "-c:a", "pcm_s16le",
                "-metadata:s:a:0", "title=Lahza-Microphone",
            ]).args(["-output_ts_offset", &origin.to_string()]).arg(&path).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let mut preview = TimelinePlaybackStream::open(&path, RecordingClipTimeline::full(1.5),
                0.0, 64, 36, false).unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut frames = Vec::new();
            while let Some(frame) = preview.next_frame(|| Instant::now() > deadline).unwrap() { frames.push(frame.time); }
            assert!(frames.len() >= 40 && frames[0] < 0.05, "container origin leaked into source clock: {origin}, {frames:?}");
            assert!(Instant::now() < deadline, "delayed audio prevented preroll");
            preview.stop();
            let played = preview.audio_samples.lock().unwrap();
            assert!(played.len() > 60000);
            let rms = |samples: &[f32]| (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
            assert!(rms(&played[0..12000]) < 0.0001, "microphone moved earlier than capture time");
            assert!(rms(&played[24000..48000]) > 0.01, "delayed microphone disappeared");
            drop(played);
            drop(preview);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn primary_temporal_tap_keeps_native_60fps_across_transport_speed_and_cuts() {
        let root=std::env::temp_dir().join(format!("lahza-primary-pts-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path=root.join("numbered.mkv");
        let encoded=Command::new("ffmpeg").args(["-v","error","-y","-f","lavfi","-i",
            "nullsrc=s=64x48:r=60:d=2,format=gbrp,geq=r='N*2':g=0:b=0,format=bgra","-c:v","ffv1"])
            .arg(&path).output().unwrap();
        assert!(encoded.status.success(),"{}",String::from_utf8_lossy(&encoded.stderr));
        let timestamps=Command::new("ffprobe").args(["-v","error","-select_streams","v:0","-show_entries","frame=best_effort_timestamp_time","-of","csv=p=0"])
            .arg(&path).output().unwrap();
        assert!(timestamps.status.success());
        let encoded_pts:Vec<f64>=String::from_utf8(timestamps.stdout).unwrap().lines()
            .filter_map(|line|line.trim().parse().ok()).collect();
        assert_eq!(encoded_pts.len(),120);
        for speed in [1.0,4.0] {
            let first=RecordingClipSegment::new(0.2,0.7);
            let mut second=RecordingClipSegment::new(1.0,1.5);second.speed=speed;
            let timeline=RecordingClipTimeline::new(vec![first,second]);
            let mut stream=TimelinePlaybackStream::open(&path,timeline.clone(),0.0,64,48,true).unwrap();
            stream.enable_temporal_frames(42);
            let source=super::super::motion_blur::VideoSource {path:path.clone(),size:(64,48),revision:42};
            let mut checked=0;
            while let Some(frame)=stream.next_frame(||false).unwrap() {
                let location=timeline.location_at(frame.time).unwrap();
                let clip=&timeline.segments[location.segment_index];
                let center=location.source_time;
                // The native tap is ahead of the 30fps transport sink. Its
                // six-frame ring must still contain the prior/current 1x
                // shutter intervals, including native pictures videorate drops.
                if center-clip.source_start<0.05 || clip.source_end-center<0.05 {continue;}
                let targets=if clip.speed==1.0 {vec![center-0.01,center+0.005]}else{vec![center]};
                for target in targets {
                    let cached=super::super::primary_frames::frame(&source,(64,48),0,target)
                        .unwrap_or_else(||panic!("missing native ring interval speed={speed}, source={center}, target={target}"));
                    let red=cached.image.get_pixel(32,24)[0];
                    // Matroska stores these PTS in milliseconds, so 1.066666
                    // is still the preceding picture until encoded PTS1.067.
                    let normalized=super::super::temporal_decode::source_clock(target);
                    let expected=encoded_pts.partition_point(|pts|*pts<=normalized).saturating_sub(1) as i32*2;
                    assert!((i32::from(red)-expected).abs()<=1,"resampled timestamp leaked into native cache: {target}, {red}, expected {expected}");
                    assert!(cached.end-cached.start<0.018,"native60fps must not become30fps intervals");
                    checked+=1;
                }
            }
            assert!(checked>10);
            drop(stream);
        }
        // Keep irregular source pictures. The primary tap must retain their
        // holds across the regular transport cadence without nominal-FPS gaps.
        let vfr=root.join("variable.mkv");
        let selected=[0usize,7,30,70,95];
        let encoded=Command::new("ffmpeg").args(["-v","error","-y","-i"]).arg(&path)
            .args(["-vf","select='eq(n,0)+eq(n,7)+eq(n,30)+eq(n,70)+eq(n,95)'","-fps_mode","vfr","-c:v","ffv1"])
            .arg(&vfr).output().unwrap();
        assert!(encoded.status.success());
        let info=probe_media(&vfr).unwrap();
        let timeline=RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.0,info.duration)]);
        let mut stream=TimelinePlaybackStream::open(&vfr,timeline,0.0,64,48,true).unwrap();
        stream.enable_temporal_frames(43);
        let source=super::super::motion_blur::VideoSource {path:vfr,size:(64,48),revision:43};
        let mut held=0;
        while let Some(frame)=stream.next_frame(||false).unwrap() {
            if frame.time<0.05 || frame.time>=encoded_pts[95] {continue;}
            // The tap stores frames on its own thread; on a loaded CI runner the
            // transport can hand out a frame a moment before its hold is cached.
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);
            let cached=loop {
                if let Some(cached)=super::super::primary_frames::frame(&source,(64,48),0,frame.time) {break cached;}
                assert!(std::time::Instant::now()<deadline,"native VFR hold missing at {}",frame.time);
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            let native=selected.iter().copied().take_while(|index|encoded_pts[*index]<=super::super::temporal_decode::source_clock(frame.time)).last().unwrap();
            assert!((i32::from(cached.image.get_pixel(32,24)[0])-native as i32*2).abs()<=1);
            held+=1;
        }
        assert!(held>25);
        drop(stream);
        fs::remove_dir_all(root).unwrap();
    }

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
