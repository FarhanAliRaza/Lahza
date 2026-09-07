//! Select a rectangle from the actual portal stream, before encoding starts.

use super::camera_preview::{attach_preview, CameraFrame, CameraFrames};
use gstreamer::{self as gst, prelude::*};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AreaPreset {
    #[default]
    Free,
    Ratio(u32, u32),
    Size(u32, u32),
}

impl AreaPreset {
    pub fn fits(self, source: (u32, u32)) -> bool {
        match self {
            Self::Size(width, height) => width <= source.0 && height <= source.1,
            _ => true,
        }
    }

    pub fn select(
        self,
        current: Option<RecordingArea>,
        source: (u32, u32),
    ) -> Option<RecordingArea> {
        if !self.fits(source) {
            return None;
        }
        let (width, height) = match self {
            Self::Free => return current,
            Self::Size(width, height) => (width, height),
            Self::Ratio(x, y) => {
                // Shape buttons start from the screen, not the previous crop:
                // fitting each shape inside its predecessor compounds shrinkage.
                let units = (source.0 / (2 * x)).min(source.1 / (2 * y));
                (units * 2 * x, units * 2 * y)
            }
        };
        if width < 2 || height < 2 {
            return None;
        }
        let center = current
            .map(|a| {
                (
                    a.left as f32 + a.width as f32 / 2.0,
                    a.top as f32 + a.height as f32 / 2.0,
                )
            })
            .unwrap_or((source.0 as f32 / 2.0, source.1 as f32 / 2.0));
        Some(
            RecordingArea {
                left: 0,
                top: 0,
                width,
                height,
                source_width: source.0,
                source_height: source.1,
            }
            .at(
                center.0 - width as f32 / 2.0,
                center.1 - height as f32 / 2.0,
            ),
        )
    }

    pub fn draw(
        self,
        start: (f32, f32),
        end: (f32, f32),
        source: (u32, u32),
    ) -> Option<RecordingArea> {
        let Self::Ratio(x, y) = self else {
            return RecordingArea::from_drag(start, end, source);
        };
        if ![start.0, start.1, end.0, end.1]
            .into_iter()
            .all(f32::is_finite)
        {
            return None;
        }
        let left = ((start.0.clamp(0.0, 1.0) * source.0 as f32) as u32 & !1).min(source.0);
        let top = ((start.1.clamp(0.0, 1.0) * source.1 as f32) as u32 & !1).min(source.1);
        let dx = (end.0 - start.0) * source.0 as f32;
        let dy = (end.1 - start.1) * source.1 as f32;
        let available_x = if dx < 0.0 { left } else { source.0 - left };
        let available_y = if dy < 0.0 { top } else { source.1 - top };
        let units = (dx.abs() / (2 * x) as f32).max(dy.abs() / (2 * y) as f32) as u32;
        let units = units.min(available_x / (2 * x)).min(available_y / (2 * y));
        if units == 0 {
            return None;
        }
        let (width, height) = (units * 2 * x, units * 2 * y);
        Some(RecordingArea {
            left: if dx < 0.0 { left - width } else { left },
            top: if dy < 0.0 { top - height } else { top },
            width,
            height,
            source_width: source.0,
            source_height: source.1,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingArea {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
}

impl RecordingArea {
    /// Move without changing the selected dimensions; keep chroma alignment.
    pub fn at(self, left: f32, top: f32) -> Self {
        Self {
            left: (left.clamp(0.0, (self.source_width - self.width) as f32) as u32) & !1,
            top: (top.clamp(0.0, (self.source_height - self.height) as f32) as u32) & !1,
            ..self
        }
    }

    pub fn contains(self, point: (f32, f32)) -> bool {
        let (x, y) = (
            point.0 * self.source_width as f32,
            point.1 * self.source_height as f32,
        );
        x >= self.left as f32
            && x <= (self.left + self.width) as f32
            && y >= self.top as f32
            && y <= (self.top + self.height) as f32
    }

    /// Accept drags in either direction; align to chroma pixels for VP8.
    pub fn from_drag(start: (f32, f32), end: (f32, f32), size: (u32, u32)) -> Option<Self> {
        if ![start.0, start.1, end.0, end.1]
            .into_iter()
            .all(f32::is_finite)
        {
            return None;
        }
        let edge = |value: f32, dimension: u32| {
            ((value.clamp(0.0, 1.0) * dimension as f32).floor() as u32).min(dimension) & !1
        };
        let left = edge(start.0.min(end.0), size.0);
        let top = edge(start.1.min(end.1), size.1);
        let right = edge(start.0.max(end.0), size.0);
        let bottom = edge(start.1.max(end.1), size.1);
        (right >= left + 2 && bottom >= top + 2).then_some(Self {
            left,
            top,
            width: right.saturating_sub(left),
            height: bottom.saturating_sub(top),
            source_width: size.0,
            source_height: size.1,
        })
    }

    pub fn pipeline_filter(self) -> String {
        format!(
            "video/x-raw,width={},height={} ! videocrop left={} top={} right={} bottom={} ! ",
            self.source_width,
            self.source_height,
            self.left,
            self.top,
            self.source_width - self.left - self.width,
            self.source_height - self.top - self.height,
        )
    }
}

pub struct AreaRequest {
    pub frame: CameraFrame,
    pub reply: mpsc::Sender<Option<RecordingArea>>,
}

struct PreviewPipeline(gst::Pipeline);

impl Drop for PreviewPipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

pub fn preview_frame(fd: i32, node: u32) -> Result<CameraFrame, String> {
    gst::init().map_err(|error| error.to_string())?;
    // Preserve the stream's pixel dimensions: portal sizes can be logical
    // coordinates on scaled displays. The RGBA rows have no alignment padding.
    let pipeline = gst::parse::launch(&format!(
        "pipewiresrc fd={fd} path={node} always-copy=true keepalive-time=1000 ! \
         videoconvert ! video/x-raw,format=RGBA ! \
         fakesink name=area_preview sync=false async=false"
    ))
    .map_err(|error| format!("could not prepare area preview: {error}"))?
    .downcast::<gst::Pipeline>()
    .map_err(|_| "could not create area preview pipeline".to_string())?;
    let pipeline = PreviewPipeline(pipeline);
    let frames = Arc::new(CameraFrames::default());
    attach_preview(&pipeline.0, "area_preview", frames.clone())?;
    pipeline
        .0
        .set_state(gst::State::Playing)
        .map_err(|error| format!("could not start area preview: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((_, frame)) = frames.newer_than(0) {
            return Ok(frame);
        }
        if let Some(message) = pipeline
            .0
            .bus()
            .and_then(|bus| bus.pop_filtered(&[gst::MessageType::Error]))
        {
            if let gst::MessageView::Error(error) = message.view() {
                return Err(format!("area preview failed: {}", error.error()));
            }
        }
        if Instant::now() >= deadline {
            return Err("no screen preview arrived; try selecting the screen again".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_uses_stream_pixels_and_accepts_reverse_and_outside_edges() {
        let area = RecordingArea::from_drag((0.75, 0.8), (0.25, 0.2), (3840, 2160)).unwrap();
        assert_eq!(
            (area.left, area.top, area.width, area.height),
            (960, 432, 1920, 1296)
        );
        let full = RecordingArea::from_drag((-0.1, -0.2), (1.2, 1.1), (1921, 1081)).unwrap();
        assert_eq!(
            (full.left, full.top, full.width, full.height),
            (0, 0, 1920, 1080)
        );
        assert!(RecordingArea::from_drag((0.5, 0.5), (0.5, 0.5), (1920, 1080)).is_none());
        assert!(RecordingArea::from_drag((f32::NAN, 0.0), (1.0, 1.0), (1920, 1080)).is_none());
    }

    #[test]
    fn size_presets_keep_exact_pixels_and_reject_oversized_captures() {
        let area = AreaPreset::Size(1920, 1080)
            .select(None, (3840, 2160))
            .unwrap();
        assert_eq!(
            (area.left, area.top, area.width, area.height),
            (960, 540, 1920, 1080)
        );
        let moved = area.at(9000.0, -100.0);
        assert_eq!(
            (moved.left, moved.top, moved.width, moved.height),
            (1920, 0, 1920, 1080)
        );
        assert!(!AreaPreset::Size(1920, 1080).fits((1366, 768)));
        assert!(AreaPreset::Size(1920, 1080)
            .select(None, (1366, 768))
            .is_none());
        let hd = AreaPreset::Size(1280, 720)
            .select(None, (1366, 768))
            .unwrap();
        assert_eq!((hd.left, hd.top, hd.width, hd.height), (42, 24, 1280, 720));
    }

    #[test]
    fn switching_shapes_repeatedly_does_not_shrink_the_selection() {
        let source = (3440, 1440);
        let mut selection = RecordingArea::from_drag((0.4, 0.4), (0.42, 0.42), source);
        let shapes = [
            (AreaPreset::Ratio(16, 9), (2560, 1440)),
            (AreaPreset::Ratio(9, 16), (810, 1440)),
            (AreaPreset::Ratio(1, 1), (1440, 1440)),
            (AreaPreset::Ratio(4, 3), (1920, 1440)),
        ];
        for _ in 0..20 {
            for (preset, dimensions) in shapes {
                let area = preset.select(selection, source).unwrap();
                assert_eq!((area.width, area.height), dimensions);
                assert!(area.left + area.width <= source.0);
                assert!(area.top + area.height <= source.1);
                assert_eq!(preset.select(Some(area), source), Some(area));
                assert_eq!(AreaPreset::Free.select(Some(area), source), Some(area));
                selection = Some(area);
            }
        }
    }

    #[test]
    fn aspect_ratio_drags_remain_exact_and_inside_source_in_every_direction() {
        for (x, y) in [(16, 9), (9, 16), (1, 1), (4, 3)] {
            let preset = AreaPreset::Ratio(x, y);
            let centered = preset.select(None, (1920, 1080)).unwrap();
            assert_eq!(centered.width * y, centered.height * x);
            for end in [(0.9, 0.6), (0.1, 0.9), (-1.0, -1.0), (2.0, 2.0), (0.8, 0.1)] {
                let area = preset.draw((0.5, 0.5), end, (1920, 1080)).unwrap();
                assert_eq!(area.width * y, area.height * x);
                assert!(area.left + area.width <= 1920);
                assert!(area.top + area.height <= 1080);
                assert_eq!(
                    (area.width % 2, area.height % 2, area.left % 2, area.top % 2),
                    (0, 0, 0, 0)
                );
            }
        }
        assert!(AreaPreset::Ratio(16, 9)
            .draw((0.5, 0.5), (0.5, 0.5), (1920, 1080))
            .is_none());
    }

    #[test]
    fn crop_encodes_only_selected_pixels() {
        gst::init().unwrap();
        let area = RecordingArea::from_drag((0.25, 0.25), (0.75, 0.75), (640, 480)).unwrap();
        let root = std::env::temp_dir().join(format!("lahza-area-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("area.webm");
        // Verify dimensions and crop position after a real encode/decode roundtrip.
        let pipeline = gst::parse::launch(&format!(
            "videotestsrc num-buffers=6 ! video/x-raw,width=640,height=480 ! \
             videoconvert ! {} vp8enc deadline=1 ! webmmux ! filesink location=\"{}\"",
            area.pipeline_filter(),
            output.display()
        ))
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let pipeline = PreviewPipeline(pipeline);
        pipeline.0.set_state(gst::State::Playing).unwrap();
        let message = pipeline
            .0
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .unwrap();
        assert!(
            matches!(message.view(), gst::MessageView::Eos(_)),
            "{message:?}"
        );
        pipeline.0.set_state(gst::State::Null).unwrap();
        let media = super::super::video::probe_media(&output).unwrap();
        assert_eq!((media.width, media.height), (320, 240));
        let frame = super::super::video::decode_frame(&output, 0.0, 320, 240).unwrap();
        let yellow = &frame.rgba[(10 * 320 + 10) * 4..][..3];
        assert!(
            yellow[0] > 200 && yellow[1] > 200 && yellow[2] < 40,
            "{yellow:?}"
        );
        let green = &frame.rgba[(10 * 320 + 160) * 4..][..3];
        assert!(
            green[0] < 40 && green[1] > 200 && green[2] < 40,
            "{green:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
