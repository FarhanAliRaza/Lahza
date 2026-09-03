//! Live webcam preview frames for the Studio canvas.
//!
//! A V4L2 device can only be opened by one pipeline at a time, so the preview
//! has two producers that publish into one [`CameraFrames`] slot: a standalone
//! pipeline while the camera toggle is on and nothing is recording, and a
//! `tee` branch of the recording pipeline while a capture is running.

use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

/// Preview frames are downscaled to this width; the height follows the
/// camera's aspect ratio.
pub const PREVIEW_WIDTH: u32 = 640;

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
        "queue max-size-buffers=2 leaky=downstream ! videoconvert ! videoscale ! \
         video/x-raw,format=RGBA,width={PREVIEW_WIDTH},pixel-aspect-ratio=1/1 ! \
         fakesink name={sink_name} sync=false "
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
        let Some(structure) = caps.structure(0) else {
            return gst::PadProbeReturn::Ok;
        };
        let (Ok(width), Ok(height)) = (
            structure.get::<i32>("width"),
            structure.get::<i32>("height"),
        ) else {
            return gst::PadProbeReturn::Ok;
        };
        let (width, height) = (width as u32, height as u32);
        let Ok(map) = buffer.map_readable() else {
            return gst::PadProbeReturn::Ok;
        };
        let expected = width as usize * height as usize * 4;
        if map.len() >= expected {
            frames.publish(CameraFrame {
                width,
                height,
                rgba: map.as_slice()[..expected].to_vec(),
            });
        }
        gst::PadProbeReturn::Ok
    });
    Ok(())
}

/// A standalone webcam pipeline that only feeds the preview.
pub struct CameraPreview {
    pipeline: gst::Pipeline,
}

impl CameraPreview {
    pub fn start(device: &str, frames: Arc<CameraFrames>) -> Result<Self, String> {
        gst::init().map_err(|error| format!("could not initialize GStreamer: {error}"))?;
        let description = format!(
            "v4l2src device=\"{device}\" ! {}",
            preview_branch("camera_preview")
        );
        let pipeline = gst::parse::launch(&description)
            .map_err(|error| format!("could not build webcam preview: {error}"))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "GStreamer did not create a preview pipeline".to_string())?;
        attach_preview(&pipeline, "camera_preview", frames)?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|error| format!("could not start webcam preview: {error}"))?;
        Ok(Self { pipeline })
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
        assert_eq!((frame.width, frame.height), (PREVIEW_WIDTH, 360));
        assert_eq!(frame.rgba.len(), (PREVIEW_WIDTH * 360 * 4) as usize);
    }
}
