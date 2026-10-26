//! Native temporal frames: one sequential decode for forward shutters, with
//! accurate seeks for discontinuities. Cache intervals come from encoded PTS.
use super::video::{self, DecodedFrame};
use gstreamer::{self as gst, prelude::*};
use image::RgbaImage;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

/// GStreamer timestamps are integral nanoseconds. Normalize arithmetic
/// equivalents once instead of mixing truncated seeks with fuzzy comparisons.
pub(super) fn source_clock(time:f64)->f64 { (time*1e9).round()/1e9 }

#[derive(Clone)]
pub(super) struct NativeFrame {
    pub start: f64,
    pub end: f64,
    pub end_signal: Option<Arc<std::sync::atomic::AtomicU64>>,
    pub image: Arc<RgbaImage>,
}

struct Sample {
    time: f64,
    duration: Option<f64>,
    image: Arc<RgbaImage>,
}

pub(super) struct NativeDecoder {
    pipeline: gst::Element,
    sink: gst::Element,
    size: (u32, u32),
    window_capture: bool,
    duration: f64,
    current: Option<Sample>,
    next: Option<Sample>,
    last_request: Option<f64>,
    eos: bool,
    #[cfg(test)]
    pub seeks: usize,
}

impl NativeDecoder {
    pub fn open(
        path: &Path,
        size: (u32, u32),
        window_capture: bool,
        borders: bool,
        duration: f64,
    ) -> Result<Self, String> {
        gst::init().map_err(|e| e.to_string())?;
        let bin = gst::parse::bin_from_description(&format!(
            "videoconvert ! videoscale add-borders={borders} ! video/x-raw,format=RGBA,width={},height={},pixel-aspect-ratio=1/1 ! appsink name=frames sync=false max-buffers=2 drop=false wait-on-eos=false", size.0, size.1), true)
            .map_err(|e| e.to_string())?;
        let sink = bin
            .by_name("frames")
            .ok_or("temporal decoder has no sink")?;
        let uri = gst::glib::filename_to_uri(path, None).map_err(|e| e.to_string())?;
        let pipeline = gst::ElementFactory::make("playbin")
            .property_from_str("flags", "video")
            .property("uri", uri.as_str())
            .property("video-sink", &bin)
            .build()
            .map_err(|e| e.to_string())?;
        // Preroll runs asynchronously while ordinary preview frames render.
        // Keeping one paused native frame avoids cold pipeline startup at the
        // first shutter; frame() still performs an accurate timestamped seek.
        pipeline.set_state(gst::State::Paused).map_err(|e| e.to_string())?;
        Ok(Self {
            pipeline,
            sink,
            size,
            window_capture,
            duration,
            current: None,
            next: None,
            last_request: None,
            eos: false,
            #[cfg(test)]
            seeks: 0,
        })
    }

    fn check(&self, cancelled: &impl Fn() -> bool, started: Instant) -> Result<(), String> {
        if cancelled() {
            return Err("Temporal frame decoding cancelled".into());
        }
        if let Some(message) = self
            .pipeline
            .bus()
            .and_then(|bus| bus.pop_filtered(&[gst::MessageType::Error]))
        {
            if let gst::MessageView::Error(error) = message.view() {
                return Err(error.error().to_string());
            }
        }
        if started.elapsed() > Duration::from_secs(5) {
            return Err("temporal decoder timed out".into());
        }
        Ok(())
    }

    fn paused(&self, cancelled: &impl Fn() -> bool) -> Result<(), String> {
        let started = Instant::now();
        loop {
            self.check(cancelled, started)?;
            let (result, current, _) = self.pipeline.state(gst::ClockTime::from_mseconds(20));
            result.map_err(|e| e.to_string())?;
            if current == gst::State::Paused {
                return Ok(());
            }
        }
    }

    fn decode(&self, sample: gst::Sample) -> Result<Sample, String> {
        let buffer = sample.buffer().ok_or("temporal sample has no buffer")?;
        let time = buffer
            .pts()
            .ok_or("temporal sample has no presentation timestamp")?
            .nseconds() as f64
            / 1e9;
        let duration = buffer
            .duration()
            .map(|value| value.nseconds() as f64 / 1e9)
            .filter(|value| *value > 0.0);
        let bytes = buffer.map_readable().map_err(|e| e.to_string())?;
        if bytes.len() != self.size.0 as usize * self.size.1 as usize * 4 {
            return Err("invalid temporal frame size".into());
        }
        let frame = video::present_frame(
            DecodedFrame {
                time,
                width: self.size.0,
                height: self.size.1,
                rgba: bytes.to_vec(),
            },
            self.window_capture,
        );
        let image = RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
            .ok_or("invalid temporal RGBA frame")?;
        Ok(Sample {
            time,
            duration,
            image: Arc::new(image),
        })
    }

    fn seek(&mut self, time: f64, cancelled: &impl Fn() -> bool) -> Result<(), String> {
        #[cfg(test)] let trace_started=Instant::now();
        #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some(){eprintln!("native seek begin {time:.6}");}
        self.current = None;
        self.next = None;
        self.eos = false;
        self.last_request = None;
        self.pipeline
            .set_state(gst::State::Paused)
            .map_err(|e| e.to_string())?;
        self.paused(cancelled)?;
        #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some(){eprintln!("native paused {time:.6} {:?}",trace_started.elapsed());}
        self.pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds((time * 1e9).round() as u64),
            )
            .map_err(|e| e.to_string())?;
        #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some(){eprintln!("native seek issued {time:.6} {:?}",trace_started.elapsed());}
        self.paused(cancelled)?;
        #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some(){eprintln!("native seek paused {time:.6} {:?}",trace_started.elapsed());}
        let started = Instant::now();
        loop {
            self.check(cancelled, started)?;
            if let Some(sample) = self
                .sink
                .emit_by_name::<Option<gst::Sample>>("try-pull-preroll", &[&20_000_000u64])
            {
                self.current = Some(self.decode(sample)?);
                break;
            }
        }
        #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some(){eprintln!("native preroll {time:.6} {:?}",trace_started.elapsed());}
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| e.to_string())?;
        #[cfg(test)]
        {
            self.seeks += 1;
        }
        Ok(())
    }

    fn next_sample(&mut self, cancelled: &impl Fn() -> bool) -> Result<(), String> {
        if self.next.is_some() || self.eos {
            return Ok(());
        }
        let started = Instant::now();
        loop {
            self.check(cancelled, started)?;
            if let Some(sample) = self
                .sink
                .emit_by_name::<Option<gst::Sample>>("try-pull-sample", &[&20_000_000u64])
            {
                let sample = self.decode(sample)?;
                // Entering Playing also queues the preroll frame. It is not
                // a new interval and must not replace the look-ahead frame.
                if self
                    .current
                    .as_ref()
                    .is_none_or(|current| sample.time > current.time)
                {
                    self.next = Some(sample);
                    return Ok(());
                }
            } else if self.sink.property::<bool>("eos") {
                self.eos = true;
                return Ok(());
            }
        }
    }

    pub fn frame(
        &mut self,
        time: f64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<NativeFrame, String> {
        if cancelled() {
            return Err("Temporal frame decoding cancelled".into());
        }
        let time = source_clock(time).clamp(0.0, (self.duration - 1e-7).max(0.0));
        if self.current.is_none()
            || self
                .last_request
                .is_some_and(|previous| time < previous || time - previous > 0.25)
        {
            self.seek(time, cancelled)?;
        }
        loop {
            self.next_sample(cancelled)?;
            if self
                .next
                .as_ref()
                .is_some_and(|next| next.time <= time)
            {
                self.current = self.next.take();
            } else {
                break;
            }
        }
        self.last_request = Some(time);
        let current = self
            .current
            .as_ref()
            .ok_or("temporal decoder produced no frame")?;
        let end = self.next.as_ref().map_or_else(
            || {
                current.duration.map_or(self.duration, |duration| {
                    (current.time + duration)
                        .max(time + 1e-9)
                        .min(self.duration)
                })
            },
            |next| next.time,
        );
        Ok(NativeFrame {
            start: current.time.min(time),
            end,
            end_signal: None,
            image: current.image.clone(),
        })
    }
}

impl Drop for NativeDecoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
