//! Live webcam preview frames for the Studio canvas.
//!
//! A V4L2 device can only be opened by one pipeline at a time, so the preview
//! has two producers that publish into one [`CameraFrames`] slot: a standalone
//! pipeline while the camera toggle is on and nothing is recording, and a
//! `tee` branch of the recording pipeline while a capture is running.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video::prelude::*;
use std::time::{Duration, Instant};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

/// Preview frames are downscaled to this width; the height follows the
/// camera's aspect ratio.
pub const PREVIEW_WIDTH: u32 = 320;

#[derive(Clone, Debug)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The most recent preview frame plus a counter so a poller can tell whether
/// it has already shown it.
#[derive(Default)]
pub struct CameraFrames {
    frame: Mutex<Option<CameraFrame>>,
    generation: AtomicU64,
}

impl CameraFrames {
    pub fn clear(&self) {
        *self.frame.lock().expect("camera frame slot poisoned") = None;
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn publish(&self, frame: CameraFrame) {
        *self.frame.lock().expect("camera frame slot poisoned") = Some(frame);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// The latest frame if it is newer than `seen`, with its generation.
    pub fn newer_than(&self, seen: u64) -> Option<(u64, CameraFrame)> {
        let generation = self.generation.load(Ordering::Acquire);
        if generation == seen {
            return None;
        }
        let frame = self
            .frame
            .lock()
            .expect("camera frame slot poisoned")
            .clone()?;
        Some((generation, frame))
    }
}

/// Pipeline fragment that scales frames for the preview and drops them into a
/// `fakesink` named `sink_name`, for use after a `tee` or a live source.
pub fn preview_branch(sink_name: &str) -> String {
    format!(
        "queue max-size-buffers=2 leaky=downstream ! videorate drop-only=true max-rate=15 ! videoconvert ! videoscale ! \
         video/x-raw,format=RGBA,width={PREVIEW_WIDTH},pixel-aspect-ratio=1/1 ! \
         fakesink name={sink_name} sync=false async=false "
    )
}

/// Publishes every buffer reaching the preview sink into `frames`.
pub fn attach_preview(
    pipeline: &gst::Pipeline,
    sink_name: &str,
    frames: Arc<CameraFrames>,
) -> Result<(), String> {
    let pad = pipeline
        .by_name(sink_name)
        .and_then(|sink| sink.static_pad("sink"))
        .ok_or_else(|| "GStreamer pipeline has no camera preview sink".to_string())?;
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_ref() else {
            return gst::PadProbeReturn::Ok;
        };
        let Some(caps) = pad.current_caps() else {
            return gst::PadProbeReturn::Ok;
        };
        let Ok(video_info) = gstreamer_video::VideoInfo::from_caps(&caps) else {
            return gst::PadProbeReturn::Ok;
        };
        let Ok(frame) = gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &video_info) else {
            return gst::PadProbeReturn::Ok;
        };
        let (width, height) = (frame.width(), frame.height());
        let stride = frame.plane_stride()[0];
        let Ok(pixels) = frame.plane_data(0) else { return gst::PadProbeReturn::Ok; };
        let row_bytes = width as usize * 4;
        if stride > 0 && stride as usize >= row_bytes && pixels.len() >= stride as usize * height as usize {
            let mut rgba = Vec::with_capacity(row_bytes * height as usize);
            for row in pixels.chunks_exact(stride as usize).take(height as usize) {
                rgba.extend_from_slice(&row[..row_bytes]);
            }
            frames.publish(CameraFrame { width, height, rgba });
        }
        gst::PadProbeReturn::Ok
    });
    Ok(())
}

/// A standalone webcam pipeline that only feeds the preview.
pub struct CameraPreview {
    pipeline: gst::Pipeline,
    frames: Arc<CameraFrames>,
    initial_generation: u64,
    started_at: Instant,
}

impl CameraPreview {
    pub fn start(device: &str, frames: Arc<CameraFrames>) -> Result<Self, String> {
        gst::init().map_err(|error| format!("could not initialize GStreamer: {error}"))?;
        frames.clear();
        let initial_generation = frames.generation.load(Ordering::Acquire);
        // decodebin accepts raw and MJPEG-only webcam modes alike. Device
        // properties are set directly rather than parsed as pipeline syntax.
        let description = format!(
            "v4l2src name=camera_source do-timestamp=true ! decodebin ! {}",
            preview_branch("camera_preview")
        );
        let pipeline = gst::parse::launch(&description)
            .map_err(|error| format!("could not build webcam preview: {error}"))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "GStreamer did not create a preview pipeline".to_string())?;
        pipeline.by_name("camera_source").ok_or_else(|| "webcam source is unavailable".to_string())?
            .set_property("device", device);
        attach_preview(&pipeline, "camera_preview", frames.clone())?;
        let preview = Self { pipeline, frames, initial_generation, started_at: Instant::now() };
        preview.pipeline.set_state(gst::State::Playing)
            .map_err(|error| format!("could not start webcam preview: {error}"))?;
        Ok(preview)
    }

    /// Poll with the launcher frames so unplugged/busy cameras cannot leave
    /// a frozen picture that appears to be live.
    pub fn take_error(&self) -> Option<String> {
        if let Some(bus) = self.pipeline.bus() {
            while let Some(message) = bus.pop() {
                match message.view() {
                    gst::MessageView::Error(error) => return Some(format!("{}", error.error())),
                    gst::MessageView::Eos(..) => return Some("The webcam stopped sending video.".into()),
                    _ => {}
                }
            }
        }
        if self.started_at.elapsed() > Duration::from_secs(5)
            && self.frames.generation.load(Ordering::Acquire) == self.initial_generation
        {
            return Some("No webcam frames arrived. Check that the camera is connected and not in use by another application.".into());
        }
        None
    }
}

impl Drop for CameraPreview {
    fn drop(&mut self) {
        // Releasing the device synchronously lets the recorder open it next.
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn missing_device_reports_an_error_and_discards_stale_frames() {
        let frames = Arc::new(CameraFrames::default());
        frames.publish(CameraFrame { width: 1, height: 1, rgba: vec![255; 4] });
        let result = CameraPreview::start("/dev/lahza-missing-camera-test", frames.clone());
        match result {
            Err(error) => assert!(!error.is_empty()),
            Ok(preview) => {
                let deadline = Instant::now() + Duration::from_secs(6);
                loop {
                    if let Some(error) = preview.take_error() {
                        assert!(!error.is_empty());
                        break;
                    }
                    assert!(Instant::now() < deadline, "missing device appeared live");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        assert!(frames.newer_than(0).is_none());
    }

    #[test]
    fn mjpeg_camera_frames_decode_into_the_same_preview_branch() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch(&format!(
            "videotestsrc num-buffers=4 ! video/x-raw,width=640,height=480 ! jpegenc ! decodebin ! {}",
            preview_branch("preview")
        )).unwrap().downcast::<gst::Pipeline>().unwrap();
        let frames = Arc::new(CameraFrames::default());
        attach_preview(&pipeline, "preview", frames.clone()).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let message = pipeline.bus().unwrap().timed_pop_filtered(
            gst::ClockTime::from_seconds(5), &[gst::MessageType::Eos, gst::MessageType::Error]);
        pipeline.set_state(gst::State::Null).unwrap();
        assert!(matches!(message.unwrap().view(), gst::MessageView::Eos(_)));
        let (_, frame) = frames.newer_than(0).unwrap();
        assert_eq!((frame.width, frame.height), (320, 240));
        assert_eq!(frame.rgba.len(), 320 * 240 * 4);
    }

    #[test]
    fn recording_tee_encodes_while_publishing_bounded_preview_frames() {
        gst::init().unwrap();
        let description = format!(
            "videotestsrc is-live=true num-buffers=30 ! video/x-raw,width=640,height=360,framerate=30/1 ! tee name=camera \
             camera. ! queue ! videoconvert ! vp8enc deadline=1 ! fakesink name=recorded sync=false \
             camera. ! {}", preview_branch("preview")
        );
        let pipeline = gst::parse::launch(&description)
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
        let frames = Arc::new(CameraFrames::default());
        attach_preview(&pipeline, "preview", frames.clone()).unwrap();
        let encoded = Arc::new(AtomicU64::new(0));
        let count = encoded.clone();
        pipeline
            .by_name("recorded")
            .unwrap()
            .static_pad("sink")
            .unwrap()
            .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                count.fetch_add(1, Ordering::Relaxed);
                gst::PadProbeReturn::Ok
            });
        pipeline.set_state(gst::State::Playing).unwrap();
        let message = pipeline.bus().unwrap().timed_pop_filtered(
            gst::ClockTime::from_seconds(5),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        pipeline.set_state(gst::State::Null).unwrap();
        assert!(matches!(message.unwrap().view(), gst::MessageView::Eos(_)));
        assert_eq!(encoded.load(Ordering::Relaxed), 30);
        let (generation, frame) = frames.newer_than(0).unwrap();
        assert!(
            generation > 0 && generation <= 16,
            "preview should run at most 15 fps"
        );
        assert_eq!((frame.width, frame.height), (320, 180));
        assert!(frames.newer_than(generation).is_none());
    }

    #[test]
    fn preview_branch_publishes_scaled_rgba_frames() {
        gst::init().expect("gstreamer");
        let description = format!(
            "videotestsrc is-live=true ! video/x-raw,width=1280,height=720 ! {}",
            preview_branch("camera_preview")
        );
        let pipeline = gst::parse::launch(&description)
            .expect("pipeline")
            .downcast::<gst::Pipeline>()
            .expect("pipeline type");
        let frames = Arc::new(CameraFrames::default());
        attach_preview(&pipeline, "camera_preview", frames.clone()).expect("probe");
        pipeline.set_state(gst::State::Playing).expect("play");
        let deadline = Instant::now() + Duration::from_secs(5);
        let frame = loop {
            if let Some((_, frame)) = frames.newer_than(0) {
                break frame;
            }
            assert!(Instant::now() < deadline, "no preview frame arrived");
            std::thread::sleep(Duration::from_millis(20));
        };
        let _ = pipeline.set_state(gst::State::Null);
        assert_eq!((frame.width, frame.height), (PREVIEW_WIDTH, 180));
        assert_eq!(frame.rgba.len(), (PREVIEW_WIDTH * 180 * 4) as usize);
    }
}
