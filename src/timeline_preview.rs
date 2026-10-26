//! One persistent decoder and a latest-request mailbox for timeline previews.
//! Pointer events replace pending work; they never create decoder processes.
use crate::{
    recording::{
        media_bank::MediaBank,
        video::{self, DecodedFrame, MediaInfo},
    },
    Studio,
};
use gpui::{Context, Timer};
use gstreamer::{self as gst, prelude::*};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Request {
    epoch: u64,
    generation: u64,
    source_revision: u64,
    path: PathBuf,
    time: f64,
    source_time: Option<f64>,
    size: (u32, u32),
    hover: bool,
}

struct Response {
    request: Request,
    frame: Result<DecodedFrame, String>,
    frame_rate: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeRequestKey {
    generation: u64,
    source_revision: u64,
    time_bits: u64,
}

#[derive(Default)]
struct Worker {
    pending: Mutex<Option<Request>>,
    response: Mutex<Option<Response>>,
    wake: Condvar,
    epoch: AtomicU64,
    active: AtomicBool,
    closed: AtomicBool,
}

impl Worker {
    fn start() -> Arc<Self> {
        let worker = Arc::new(Self::default());
        let shared = worker.clone();
        std::thread::spawn(move || {
            let mut decoder: Option<PreviewDecoder> = None;
            let mut source_revision = 0;
            while !shared.closed.load(Ordering::SeqCst) {
                let mut pending = shared.pending.lock().unwrap();
                while pending.is_none() && !shared.closed.load(Ordering::SeqCst) {
                    pending = shared
                        .wake
                        .wait_timeout(pending, Duration::from_secs(2))
                        .unwrap()
                        .0;
                    if pending.is_none() {
                        // Entering Null can wait for decoder threads. Never
                        // hold the UI's request mailbox while tearing down.
                        drop(pending);
                        decoder = None;
                        pending = shared.pending.lock().unwrap();
                    }
                }
                if shared.closed.load(Ordering::SeqCst) {
                    break;
                }
                // Coalesce rapid mouse moves before seeking the persistent player.
                drop(pending);
                std::thread::sleep(Duration::from_millis(16));
                let request = {
                    let mut pending = shared.pending.lock().unwrap();
                    let Some(request) = pending.take() else {
                        continue;
                    };
                    shared.active.store(true, Ordering::SeqCst);
                    request
                };
                let cancelled = || {
                    shared.closed.load(Ordering::SeqCst)
                        || shared.epoch.load(Ordering::SeqCst) != request.epoch
                };
                let mut frame_rate = None;
                let frame = (|| {
                    if cancelled() {
                        return Err("cancelled".into());
                    }
                    let Some(time) = request.source_time else {
                        return Ok(DecodedFrame {
                            time: request.time,
                            width: 16,
                            height: 9,
                            rgba: [0, 0, 0, 255].repeat(16 * 9),
                        });
                    };
                    if decoder.as_ref().is_none_or(|decoder| {
                        decoder.path != request.path || decoder.size != request.size
                    }) || source_revision != request.source_revision
                    {
                        decoder = Some(PreviewDecoder::open(request.path.clone(), request.size)?);
                        source_revision = request.source_revision;
                    }
                    let decoder = decoder.as_mut().unwrap();
                    frame_rate = Some(decoder.info.frame_rate);
                    decoder.frame(time, &cancelled)
                })();
                if !cancelled() {
                    *shared.response.lock().unwrap() = Some(Response {
                        request,
                        frame,
                        frame_rate,
                    });
                }
                shared.active.store(false, Ordering::SeqCst);
            }
        });
        worker
    }
}

#[derive(Default)]
pub(crate) struct TimelinePreviewState {
    worker: Option<Arc<Worker>>,
    polling: bool,
    epoch: u64,
    interactive_mode: Option<bool>,
    requested_hover: Option<f64>,
    displayed_hover: Option<f64>,
    displayed_scrub: Option<f64>,
    last_request: Option<Instant>,
    deferred: Option<(f64, bool)>,
    defer_scheduled: bool,
    native_request: Option<NativeRequestKey>,
    pub(crate) frame_rate: Option<f64>,
}

impl Drop for TimelinePreviewState {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.closed.store(true, Ordering::SeqCst);
            worker.wake.notify_all();
        }
    }
}

impl Studio {
    pub(crate) fn timeline_display_time(&self) -> f64 {
        if self.video_playing {
            self.video_position
        } else {
            self.timeline_preview
                .displayed_hover
                .or(self.timeline_preview.displayed_scrub)
                .unwrap_or(self.video_position)
        }
    }

    pub(crate) fn timeline_skim_time(&self) -> Option<f64> {
        self.timeline_preview.requested_hover
    }

    pub(crate) fn clear_timeline_preview(&mut self) {
        self.timeline_preview.requested_hover = None;
        self.timeline_preview.displayed_hover = None;
        self.timeline_preview.displayed_scrub = None;
        self.timeline_preview.deferred = None;
        self.timeline_preview.interactive_mode = None;
        self.timeline_preview.native_request = None;
        self.timeline_preview.epoch = self.timeline_preview.epoch.wrapping_add(1);
        if let Some(worker) = &self.timeline_preview.worker {
            worker
                .epoch
                .store(self.timeline_preview.epoch, Ordering::SeqCst);
            *worker.pending.lock().unwrap() = None;
        }
    }

    pub(crate) fn update_timeline_skim(&mut self, time: Option<f64>, cx: &mut Context<Self>) {
        let time = time
            .filter(|_| {
                !self.video_playing
                    && self.video_seek_drag.is_none()
                    && self.video_trim_drag.is_none()
                    && self.video_move_drag.is_none()
                    && self.video_zoom_drag.is_none()
                    && self.camera_region_drag.is_none()
                    && self.annotation_drag.is_none()
                    && self.video_audio_drag.is_none()
                    && !self.crop_active
                    && !self.video_edit_busy
            })
            .map(|time| time.clamp(0.0, self.video_duration));
        if self.timeline_preview.requested_hover == time {
            return;
        }
        self.timeline_preview.requested_hover = time;
        if self.video_project.is_none() {
            self.timeline_preview.displayed_hover = time;
            cx.notify();
            return;
        }
        self.queue_timeline_frame(time.unwrap_or(self.video_position), time.is_some(), cx);
    }

    pub(crate) fn scrub_video_to(&mut self, time: f64, cx: &mut Context<Self>) {
        self.timeline_preview.requested_hover = None;
        self.timeline_preview.displayed_hover = None;
        self.video_position = time.clamp(0.0, self.video_timeline_duration());
        if self.video_project.is_some() {
            self.queue_timeline_frame(self.video_position, false, cx);
        }
        cx.notify();
    }

    pub(crate) fn finish_timeline_scrub(&mut self, cx: &mut Context<Self>) {
        self.video_seek_drag = None;
        self.seek_video_frame(self.video_position, cx);
    }

    fn native_request_key(&self, time: f64) -> NativeRequestKey {
        NativeRequestKey {
            generation: self.video_playback_generation.load(Ordering::SeqCst),
            source_revision: self.video_extras_token,
            time_bits: time.to_bits(),
        }
    }

    /// A quick play/pause can stop before the first native playback frame
    /// replaces a scrub proxy. Promote once per source/time/generation. Keep
    /// the attempted key after errors or gaps, so rendering cannot retry-loop.
    pub(crate) fn promote_paused_video_frame(&mut self, cx: &mut Context<Self>) {
        if self.video_project.is_none() || self.video_playing || self.video_window_capture
            || self.video_seek_drag.is_some() || self.timeline_preview.requested_hover.is_some()
            || self.video_frame_rgba.as_ref().is_none_or(|frame| frame.dimensions() == self.video_source_size)
        {
            return;
        }
        let key = self.native_request_key(self.video_position);
        if self.timeline_preview.native_request != Some(key) {
            self.queue_timeline_frame(self.video_position, false, cx);
        }
    }

    pub(crate) fn queue_timeline_frame(&mut self, time: f64, hover: bool, cx: &mut Context<Self>) {
        // Leave an in-flight seek enough time to display a frame while the
        // pointer keeps moving, and retain only the latest intermediate target.
        let interactive = hover || self.video_seek_drag.is_some();
        let elapsed = self
            .timeline_preview
            .last_request
            .map(|last| last.elapsed());
        if interactive && elapsed.is_some_and(|elapsed| elapsed < Duration::from_millis(60)) {
            self.timeline_preview.deferred = Some((time, hover));
            if !self.timeline_preview.defer_scheduled {
                self.timeline_preview.defer_scheduled = true;
                let delay = Duration::from_millis(60) - elapsed.unwrap();
                cx.spawn(async move |weak, cx| {
                    Timer::after(delay).await;
                    let _ = weak.update(cx, |this, cx| {
                        this.timeline_preview.defer_scheduled = false;
                        if let Some((time, hover)) = this.timeline_preview.deferred.take() {
                            this.queue_timeline_frame(time, hover, cx);
                        }
                    });
                })
                .detach();
            }
            return;
        }
        self.timeline_preview.deferred = None;
        self.timeline_preview.last_request = Some(Instant::now());
        let Some(path) = self.video_playback_path() else {
            return;
        };
        // Opening a project or importing media defers thumbnail setup until
        // render. That setup advances the shared source revision, so start it
        // before capturing this seek's revision; otherwise the first native
        // frame can be discarded and the low-resolution poster stays visible.
        if self.video_extras_pending {
            self.video_extras_pending = false;
            self.spawn_video_extras(cx);
        }
        // Pointer samples within one gesture may finish and display while a
        // newer target waits. A release, hover exit, or explicit seek starts
        // a new epoch, so none of the previous gesture's frames can land.
        if !interactive || self.timeline_preview.interactive_mode != Some(hover) {
            self.timeline_preview.epoch = self.timeline_preview.epoch.wrapping_add(1);
        }
        self.timeline_preview.interactive_mode = interactive.then_some(hover);
        let worker = self
            .timeline_preview
            .worker
            .get_or_insert_with(Worker::start)
            .clone();
        worker
            .epoch
            .store(self.timeline_preview.epoch, Ordering::SeqCst);
        let source_time = if time > self.video_duration {
            None
        } else {
            self.video_clip_timeline
                .content_source_time_at(time.min((self.video_duration - 1e-6).max(0.0)))
        };
        let size = if self.crop_active || self.video_crop != crate::CropRect::UNIT {
            self.video_source_size
        } else if hover || self.video_seek_drag.is_some() {
            (1280, 720)
        } else {
            // Explicit seeks and scrub release replace the interactive proxy
            // with original pixels before evaluating any zoom or glass lens.
            self.video_source_size
        };
        self.timeline_preview.native_request = (!interactive).then(|| self.native_request_key(time));
        *worker.pending.lock().unwrap() = Some(Request {
            epoch: self.timeline_preview.epoch,
            generation: self.video_playback_generation.load(Ordering::SeqCst),
            source_revision: self.video_extras_token,
            path,
            time,
            source_time,
            size,
            hover,
        });
        worker.wake.notify_one();
        if self.timeline_preview.polling {
            return;
        }
        self.timeline_preview.polling = true;
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(16)).await;
            let response = worker.response.lock().unwrap().take();
            let keep_polling = weak.update(cx, |this, cx| {
                if let Some(response) = response {
                    let request = response.request;
                    if request.epoch == this.timeline_preview.epoch
                        && request.source_revision == this.video_extras_token
                        && request.generation
                            == this.video_playback_generation.load(Ordering::SeqCst)
                        && !this.video_playing
                        && this.video_playback_path().as_ref() == Some(&request.path)
                    {
                        if let Some(frame_rate) = response.frame_rate {
                            this.timeline_preview.frame_rate = Some(frame_rate);
                        }
                        match response.frame {
                            Ok(frame) => {
                                if let Some(image) = image::RgbaImage::from_raw(
                                    frame.width,
                                    frame.height,
                                    frame.rgba,
                                ) {
                                    this.timeline_preview.displayed_hover =
                                        request.hover.then_some(request.time);
                                    this.timeline_preview.displayed_scrub = (!request.hover
                                        && this.video_seek_drag.is_some())
                                    .then_some(request.time);
                                    this.set_video_frame(image);
                                    this.camera_frame_rgba = None;
                                    this.camera_decoded_time = -1.0;
                                }
                            }
                            Err(error) if !request.hover => {
                                this.toast = Some(format!("Could not seek: {error}").into())
                            }
                            Err(_) => {}
                        }
                        cx.notify();
                    }
                }
                let busy = worker.active.load(Ordering::SeqCst)
                    || worker.pending.lock().unwrap().is_some()
                    || worker.response.lock().unwrap().is_some();
                if !busy {
                    this.timeline_preview.polling = false;
                }
                busy
            });
            if !matches!(keep_polling, Ok(true)) {
                break;
            }
        })
        .detach();
    }
}

/// Paused, silent GStreamer player reused for arbitrary forward/backward seeks.
/// Imported stills use the existing decoder and are cached until the asset changes.
pub(crate) struct PreviewDecoder {
    path: PathBuf,
    size: (u32, u32),
    info: MediaInfo,
    bank: Option<MediaBank>,
    active_path: Option<PathBuf>,
    pipeline: Option<gst::Element>,
    sink: Option<gst::Element>,
    still: Option<(usize, DecodedFrame)>,
    dimensions: (u32, u32),
}

impl PreviewDecoder {
    pub(crate) fn open(path: PathBuf, size: (u32, u32)) -> Result<Self, String> {
        gst::init().map_err(|error| error.to_string())?;
        let info = video::probe_media(&path).map_err(|error| error.to_string())?;
        let bank = MediaBank::load(&path).map_err(|error| error.to_string())?;
        let dimensions = video::decode_dimensions(&info, size.0, size.1);
        Ok(Self {
            path,
            size,
            info,
            bank,
            active_path: None,
            pipeline: None,
            sink: None,
            still: None,
            dimensions,
        })
    }

    pub(crate) fn frame(
        &mut self,
        time: f64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<DecodedFrame, String> {
        let time = time.clamp(0.0, (self.info.duration - 0.001).max(0.0));
        let (path, local_time, borders) = if let Some(bank) = &self.bank {
            let (index, offset) = bank.locate(time);
            if bank.entries[index].still {
                if self
                    .still
                    .as_ref()
                    .is_none_or(|(cached, _)| *cached != index)
                {
                    self.still = Some((
                        index,
                        bank.decode_frame(&self.path, time, self.size.0, self.size.1)
                            .map_err(|error| error.to_string())?,
                    ));
                }
                return Ok(self.still.as_ref().unwrap().1.clone());
            }
            (
                bank.entry_path(&self.path, index),
                time - offset,
                index != 0,
            )
        } else {
            (self.path.clone(), time, false)
        };
        if self.active_path.as_ref() != Some(&path) {
            self.stop();
            let (width, height) = self.dimensions;
            let sink = gst::parse::bin_from_description(&format!(
                "videoconvert ! videoscale add-borders={borders} ! video/x-raw,format=RGBA,width={width},height={height},pixel-aspect-ratio=1/1 ! appsink name=frames sync=false max-buffers=1 drop=true wait-on-eos=false"), true)
                .map_err(|error| error.to_string())?;
            let frames = sink.by_name("frames").unwrap();
            let uri = gst::glib::filename_to_uri(&path, None).map_err(|error| error.to_string())?;
            let player = gst::ElementFactory::make("playbin")
                .property_from_str("flags", "video")
                .property("uri", uri.as_str())
                .property("video-sink", &sink)
                .build()
                .map_err(|error| error.to_string())?;
            self.pipeline = Some(player.clone());
            self.sink = Some(frames);
            self.active_path = Some(path);
            player
                .set_state(gst::State::Paused)
                .map_err(|error| error.to_string())?;
        }
        if cancelled() {
            return Err("cancelled".into());
        }
        let pipeline = self.pipeline.as_ref().unwrap();
        let sink = self.sink.as_ref().unwrap();
        // A preceding cancelled request may have left startup in progress.
        let started = Instant::now();
        loop {
            if cancelled() {
                return Err("cancelled".into());
            }
            let (result, current, _) = pipeline.state(gst::ClockTime::from_mseconds(20));
            result.map_err(|error| error.to_string())?;
            if current == gst::State::Paused {
                break;
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err("preview decoder timed out".into());
            }
        }
        pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds((local_time * 1e9) as u64),
            )
            .map_err(|error| error.to_string())?;
        let started = Instant::now();
        loop {
            if cancelled() {
                return Err("cancelled".into());
            }
            if let Some(sample) =
                sink.emit_by_name::<Option<gst::Sample>>("try-pull-preroll", &[&20_000_000u64])
            {
                let buffer = sample.buffer().ok_or("preview sample has no buffer")?;
                let bytes = buffer.map_readable().map_err(|error| error.to_string())?;
                let (width, height) = self.dimensions;
                if bytes.len() != width as usize * height as usize * 4 {
                    return Err("invalid preview frame size".into());
                }
                return Ok(video::present_frame(
                    DecodedFrame {
                        time,
                        width,
                        height,
                        rgba: bytes.to_vec(),
                    },
                    self.info.window_capture,
                ));
            }
            if let Some(message) = pipeline
                .bus()
                .unwrap()
                .pop_filtered(&[gst::MessageType::Error])
            {
                if let gst::MessageView::Error(error) = message.view() {
                    return Err(error.error().to_string());
                }
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err("preview seek timed out".into());
            }
        }
    }

    fn stop(&mut self) {
        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }
        self.sink = None;
        self.active_path = None;
    }
}
impl Drop for PreviewDecoder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_preview_preserves_transparent_window_pixels() {
        gst::init().unwrap();
        let root = std::env::temp_dir().join(format!("lahza-scrub-alpha-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("screen.mkv");
        let pipeline = gst::parse::launch(&format!(
            "videotestsrc num-buffers=12 pattern=ball foreground-color=4278190080 background-color=0 ! video/x-raw,format=BGRA,width=64,height=48,framerate=10/1 ! avenc_ffv1 ! matroskamux ! filesink location=\"{}\"",
            path.display()
        )).unwrap().downcast::<gst::Pipeline>().unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let message = pipeline.bus().unwrap().timed_pop_filtered(
            gst::ClockTime::from_seconds(5),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        pipeline.set_state(gst::State::Null).unwrap();
        assert!(matches!(message.unwrap().view(), gst::MessageView::Eos(_)));
        std::fs::write(path.with_extension("window.json"), b"{}").unwrap();
        let mut decoder = PreviewDecoder::open(path, (32, 24)).unwrap();
        for time in [0.2, 0.7, 0.1] {
            let frame = decoder.frame(time, &|| false).unwrap();
            assert!(frame.rgba.chunks_exact(4).any(|pixel| pixel[3] == 0));
            assert!(frame
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0 && pixel[3] < 255));
            assert!(frame
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel == [0, 0, 0, 255]));
        }
        drop(decoder);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_preview_seeks_backwards_without_reopening_the_player() {
        let root = std::env::temp_dir().join(format!("lahza-scrub-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("clip.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x48:r=30:d=1",
                "-f",
                "lavfi",
                "-i",
                "color=blue:s=64x48:r=30:d=1",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0",
                "-c:v",
                "ffv1",
                "-y",
            ])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let mut decoder = PreviewDecoder::open(path.clone(), (64, 48)).unwrap();
        let first = decoder.frame(0.2, &|| false).unwrap();
        let player = decoder.pipeline.clone().unwrap();
        assert!(first.rgba[0] > 200);
        let second = decoder.frame(1.4, &|| false).unwrap();
        assert!(second.rgba[2] > 200);
        let third = decoder.frame(0.4, &|| false).unwrap();
        assert!(third.rgba[0] > 200);
        assert_eq!(decoder.pipeline.as_ref(), Some(&player));
        assert!(decoder.frame(0.5, &|| true).is_err());
        drop(decoder);

        // Imported stills share virtual source time with the recording, and
        // returning from one must restore the correct recorded source frame.
        image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 255, 0, 255]))
            .save(root.join("still.png"))
            .unwrap();
        let bank_path = root.join("sources.media.json");
        let bank = MediaBank {
            entries: vec![
                crate::recording::media_bank::MediaEntry {
                    file: PathBuf::from("clip.mkv"),
                    info: video::probe_media(&path).unwrap(),
                    still: false,
                },
                crate::recording::media_bank::MediaEntry {
                    file: PathBuf::from("still.png"),
                    info: MediaInfo {
                        duration: 1.0,
                        width: 20,
                        height: 20,
                        frame_rate: 30.0,
                        has_audio: false,
                        window_capture: false,
                    },
                    still: true,
                },
            ],
        };
        bank.save(&bank_path).unwrap();
        let mut decoder = PreviewDecoder::open(bank_path, (32, 24)).unwrap();
        let clip = decoder.frame(1.4, &|| false).unwrap();
        assert_eq!((clip.width, clip.height), (32, 24));
        assert!(clip.rgba[2] > 200);
        let still = decoder.frame(2.4, &|| false).unwrap();
        assert!(still.rgba[(12 * 32 + 16) * 4 + 1] > 200);
        assert!(decoder.frame(0.4, &|| false).unwrap().rgba[0] > 200);
        drop(decoder);
        std::fs::remove_dir_all(root).unwrap();
    }
}
