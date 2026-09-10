use gst::prelude::*;
use gstreamer as gst;
use gstreamer_video::VideoCropMeta;
use std::sync::{Arc, Mutex, OnceLock};

pub type RecordingSize = Arc<OnceLock<(u32, u32)>>;

pub const FILTER: &str = "videocrop name=screen_crop ! videoconvert ! \
    videoscale add-borders=false ! capsfilter name=screen_scaled ! \
    videobox autocrop=true border-alpha=0 ! capsfilter name=screen_size";

pub const NATIVE_WINDOW_FILTER: &str = "video/x-raw,format=(string){BGRA,RGBA} ! \
    identity name=screen_crop ! capssetter name=screen_native_caps ! \
    videoconvert ! capsfilter name=screen_size";

pub fn attach(
    pipeline: &gst::Pipeline,
    recording_size: RecordingSize,
    native_window: bool,
) -> Result<(), String> {
    let pad = pipeline
        .by_name("screen_source")
        .and_then(|source| source.static_pad("src"))
        .ok_or_else(|| "screen source has no output pad".to_string())?;
    let crop = pipeline
        .by_name("screen_crop")
        .ok_or_else(|| "screen crop filter is missing".to_string())?;
    let size = pipeline
        .by_name("screen_size")
        .ok_or_else(|| "screen size filter is missing".to_string())?;
    let scaled = pipeline.by_name("screen_scaled");
    let native_caps = pipeline.by_name("screen_native_caps");
    let weak_pipeline = pipeline.downgrade();
    let previous = Mutex::new(None);
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
        let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let Some(caps) = pad.current_caps() else {
            return gst::PadProbeReturn::Drop;
        };
        let Some(caps) = caps.structure(0) else {
            return gst::PadProbeReturn::Drop;
        };
        let (Ok(width), Ok(height)) = (caps.get::<i32>("width"), caps.get::<i32>("height")) else {
            return gst::PadProbeReturn::Drop;
        };
        if width <= 0 || height <= 0 {
            return gst::PadProbeReturn::Drop;
        }
        let (width, height) = (width as u32, height as u32);
        let rect = buffer
            .meta::<VideoCropMeta>()
            .map(|meta| meta.rect())
            .unwrap_or((0, 0, width, height));
        let (x, y, content_width, content_height) = rect;
        if content_width == 0
            || content_height == 0
            || x >= width
            || y >= height
            || content_width > width - x
            || content_height > height - y
        {
            return gst::PadProbeReturn::Drop;
        }

        if native_window {
            let &(output_width, output_height) = recording_size.get_or_init(|| (width, height));
            if content_width > output_width || content_height > output_height {
                if let Some(pipeline) = weak_pipeline.upgrade() {
                    gst::element_error!(pipeline, gst::CoreError::Negotiation,
                        ("Window exceeded the capture surface ({}x{}); recording stopped to preserve its pixels", output_width, output_height));
                }
                return gst::PadProbeReturn::Drop;
            }
            if !matches!(caps.get::<&str>("format"), Ok("BGRA" | "RGBA")) {
                return gst::PadProbeReturn::Drop;
            }
            let (offset, stride) = buffer.meta::<gstreamer_video::VideoMeta>()
                .map(|meta| (meta.offset()[0], meta.stride()[0]))
                .unwrap_or((0, width as i32 * 4));
            if stride < width as i32 * 4 { return gst::PadProbeReturn::Drop; }
            let Ok(map) = buffer.map_readable() else { return gst::PadProbeReturn::Drop; };
            let mut pixels = vec![0u8; output_width as usize * output_height as usize * 4];
            let left = (output_width - content_width) / 2;
            let top = (output_height - content_height) / 2;
            for row in 0..content_height {
                let source = offset + (y + row) as usize * stride as usize + x as usize * 4;
                let target = ((top + row) as usize * output_width as usize + left as usize) * 4;
                let Some(bytes) = map.get(source..source + content_width as usize * 4) else {
                    return gst::PadProbeReturn::Drop;
                };
                pixels[target..target + bytes.len()].copy_from_slice(bytes);
            }
            drop(map);
            // Copy native pixels exactly. Reinitialize the unused surface on
            // every frame so a smaller window cannot leave stale pixels. The
            // replacement buffer is tightly packed and carries no old crop or
            // stride metadata; capssetter describes its actual storage size.
            let mut replacement = gst::Buffer::from_mut_slice(pixels);
            if buffer.copy_into(replacement.get_mut().unwrap(), gst::BufferCopyFlags::TIMESTAMPS | gst::BufferCopyFlags::FLAGS, ..).is_err() {
                return gst::PadProbeReturn::Drop;
            }
            *buffer = replacement;
            let mut previous = previous.lock().expect("screen geometry poisoned");
            if previous.as_ref().is_none_or(|(w, h, _)| (*w, *h) != (width, height)) {
                let caps = gst::Caps::builder("video/x-raw")
                    .field("width", output_width as i32).field("height", output_height as i32)
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1)).build();
                native_caps.as_ref().unwrap().set_property("caps", &caps);
                size.set_property("caps", &caps);
            }
            #[cfg(test)]
            if *previous != Some((width, height, rect)) {
                eprintln!("native capture: surface={width}x{height}, window={rect:?}, stride={stride}");
            }
            *previous = Some((width, height, rect));
            return gst::PadProbeReturn::Ok;
        }

        // Mutter can allocate a monitor-sized buffer for a small window.
        // Apply its content rectangle before encoding; caps describe storage,
        // not necessarily the visible window. Consume the metadata so a later
        // element cannot apply the same offsets a second time.
        if let Some(meta) = buffer.make_mut().meta_mut::<VideoCropMeta>() {
            if meta.remove().is_err() {
                return gst::PadProbeReturn::Drop;
            }
        }
        let mut previous = previous.lock().expect("screen geometry poisoned");
        if *previous != Some((width, height, rect)) {
            crop.set_properties(&[
                ("left", &(x as i32)),
                ("top", &(y as i32)),
                ("right", &((width - x - content_width) as i32)),
                ("bottom", &((height - y - content_height) as i32)),
            ]);
            let &(output_width, output_height) = recording_size.get_or_init(|| (content_width, content_height));
            // Letterbox resized windows with transparency. videoscale's
            // built-in borders are opaque black even for alpha formats.
            let scale = (f64::from(output_width) / f64::from(content_width))
                .min(f64::from(output_height) / f64::from(content_height));
            if let Some(scaled) = scaled.as_ref() { scaled.set_property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field(
                        "width",
                        (f64::from(content_width) * scale).round().max(1.0) as i32,
                    )
                    .field(
                        "height",
                        (f64::from(content_height) * scale).round().max(1.0) as i32,
                    )
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .build(),
            );
            }
            size.set_property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", output_width as i32)
                    .field("height", output_height as i32)
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .build(),
            );
            *previous = Some((width, height, rect));
        }
        gst::PadProbeReturn::Ok
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Capture {
        pipeline: gst::Pipeline,
        source: gst::Element,
        sink: gst::Element,
        frame: u64,
    }

    impl Capture {
        fn new(size: RecordingSize) -> Self {
            Self::with_mode(size, false)
        }

        fn with_mode(size: RecordingSize, native_window: bool) -> Self {
            gst::init().unwrap();
            let filter = if native_window {
                NATIVE_WINDOW_FILTER
            } else {
                FILTER
            };
            let pipeline = gst::parse::launch(&format!(
                "appsrc name=screen_source is-live=true format=time \
                 caps=video/x-raw,format=RGBA,width=64,height=48,framerate=30/1 ! \
                 {filter} ! videoconvert ! video/x-raw,format=RGBA ! \
                 appsink name=frames sync=false"
            ))
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
            attach(&pipeline, size, native_window).unwrap();
            let source = pipeline.by_name("screen_source").unwrap();
            let sink = pipeline.by_name("frames").unwrap();
            pipeline.set_state(gst::State::Playing).unwrap();
            Self {
                pipeline,
                source,
                sink,
                frame: 0,
            }
        }

        fn push(&mut self, rect: Option<(u32, u32, u32, u32)>, color: [u8; 4]) {
            let mut pixels = [255, 0, 255, 255].repeat(64 * 48);
            let (x, y, width, height) = rect.unwrap_or((0, 0, 64, 48));
            for row in y..y.saturating_add(height).min(48) {
                for col in x..x.saturating_add(width).min(64) {
                    let offset = (row * 64 + col) as usize * 4;
                    pixels[offset..offset + 4].copy_from_slice(&color);
                }
            }
            self.push_pixels(rect, pixels);
        }

        fn push_pixels(&mut self, rect: Option<(u32, u32, u32, u32)>, pixels: Vec<u8>) {
            let mut buffer = gst::Buffer::from_mut_slice(pixels);
            let buffer_ref = buffer.get_mut().unwrap();
            buffer_ref.set_pts(gst::ClockTime::from_nseconds(self.frame * 33_333_333));
            buffer_ref.set_duration(gst::ClockTime::from_nseconds(33_333_333));
            if let Some(rect) = rect {
                VideoCropMeta::add(buffer_ref, rect);
            }
            self.frame += 1;
            assert_eq!(
                self.source
                    .emit_by_name::<gst::FlowReturn>("push-buffer", &[&buffer]),
                gst::FlowReturn::Ok
            );
        }

        fn assert_frame(&self, width: i32, height: i32, color: [u8; 4]) {
            let sample = self
                .sink
                .emit_by_name::<Option<gst::Sample>>(
                    "try-pull-sample",
                    &[&gst::ClockTime::from_seconds(2)],
                )
                .expect("cropped frame must arrive");
            let caps = sample.caps().unwrap().structure(0).unwrap();
            assert_eq!(caps.get::<i32>("width").unwrap(), width);
            assert_eq!(caps.get::<i32>("height").unwrap(), height);
            let pixels = sample.buffer().unwrap().map_readable().unwrap();
            assert_eq!(pixels.len(), (width * height * 4) as usize);
            for pixel in pixels.chunks_exact(4) {
                assert_eq!(
                    pixel, color,
                    "storage padding or an incorrect crop reached output"
                );
            }
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            let _ = self.pipeline.set_state(gst::State::Null);
        }
    }

    #[test]
    fn native_windows_keep_every_pixel_when_growing_changing_aspect_and_resuming() {
        use crate::recording::video::{present_frame, DecodedFrame};
        let size = RecordingSize::default();
        let mut capture = Capture::with_mode(size.clone(), true);
        for (index, (width, height)) in [(9, 33), (48, 27), (51, 45)].into_iter().enumerate() {
            if index == 2 {
                drop(capture);
                capture = Capture::with_mode(size.clone(), true);
            }
            let (x, y) = (3, 2);
            let mut pixels = [255, 0, 255, 255].repeat(64 * 48);
            let mut expected = Vec::new();
            for row in 0..height {
                for col in 0..width {
                    let color = if (row + col) % 2 == 0 {
                        [0, 0, 0, 255]
                    } else {
                        [255, 255, 255, 255]
                    };
                    expected.extend_from_slice(&color);
                    let offset = ((row + y) * 64 + col + x) as usize * 4;
                    pixels[offset..offset + 4].copy_from_slice(&color);
                }
            }
            capture.push_pixels(Some((x, y, width, height)), pixels);
            let sample = capture
                .sink
                .emit_by_name::<Option<gst::Sample>>(
                    "try-pull-sample",
                    &[&gst::ClockTime::from_seconds(2)],
                )
                .unwrap();
            let pixels = sample.buffer().unwrap().map_readable().unwrap();
            assert_eq!(pixels.len(), 64 * 48 * 4);
            let frame = present_frame(
                DecodedFrame {
                    time: 0.0,
                    width: 64,
                    height: 48,
                    rgba: pixels.to_vec(),
                },
                true,
            );
            assert_eq!((frame.width, frame.height), (width, height));
            assert_eq!(
                frame.rgba, expected,
                "native single-pixel detail must survive every resize"
            );
        }
    }

    #[test]
    fn window_content_is_cropped_from_padded_buffers() {
        let mut capture = Capture::new(RecordingSize::default());
        capture.push(Some((7, 9, 31, 21)), [20, 60, 100, 255]);
        capture.assert_frame(31, 21, [20, 60, 100, 255]);
    }

    #[test]
    fn monitor_without_crop_metadata_keeps_its_dimensions() {
        let mut capture = Capture::new(RecordingSize::default());
        capture.push(None, [20, 60, 100, 255]);
        capture.assert_frame(64, 48, [20, 60, 100, 255]);
    }

    #[test]
    fn resizing_and_resuming_keep_the_initial_window_size() {
        let size = RecordingSize::default();
        let mut capture = Capture::new(size.clone());
        capture.push(Some((4, 6, 32, 20)), [20, 60, 100, 255]);
        capture.assert_frame(32, 20, [20, 60, 100, 255]);
        capture.push(Some((13, 7, 16, 10)), [80, 40, 20, 255]);
        capture.assert_frame(32, 20, [80, 40, 20, 255]);
        drop(capture);
        let mut resumed = Capture::new(size);
        resumed.push(Some((6, 8, 48, 30)), [30, 60, 90, 255]);
        resumed.assert_frame(32, 20, [30, 60, 90, 255]);
    }

    #[test]
    fn resized_windows_get_transparent_letterboxing() {
        let size = RecordingSize::default();
        size.set((32, 20)).unwrap();
        let mut capture = Capture::new(size);
        capture.push(Some((7, 9, 16, 20)), [20, 60, 100, 128]);
        let sample = capture
            .sink
            .emit_by_name::<Option<gst::Sample>>(
                "try-pull-sample",
                &[&gst::ClockTime::from_seconds(2)],
            )
            .unwrap();
        let pixels = sample.buffer().unwrap().map_readable().unwrap();
        assert_eq!(pixels.len(), 32 * 20 * 4);
        for row in pixels.chunks_exact(32 * 4) {
            for (x, pixel) in row.chunks_exact(4).enumerate() {
                if (8..24).contains(&x) {
                    assert_eq!(&pixel[..3], &[20, 60, 100]);
                    assert!(pixel[3].abs_diff(128) <= 1);
                } else {
                    assert_eq!(pixel[3], 0, "resizing must not introduce black borders");
                }
            }
        }
    }

    #[test]
    fn invalid_crop_does_not_set_the_recording_dimensions() {
        let size = RecordingSize::default();
        let mut capture = Capture::new(size.clone());
        capture.push(Some((0, 0, 0, 0)), [0, 0, 0, 255]);
        capture.push(Some((63, 47, u32::MAX, u32::MAX)), [0, 0, 0, 255]);
        capture.push(Some((7, 9, 31, 21)), [20, 60, 100, 255]);
        capture.assert_frame(31, 21, [20, 60, 100, 255]);
        assert_eq!(size.get(), Some(&(31, 21)));
    }
}
