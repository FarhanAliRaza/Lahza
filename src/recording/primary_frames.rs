//! Native buffers shared by the main transport and temporal preview. The tap
//! precedes videorate, so intervals describe encoded pictures, not UI clocks.
use super::{motion_blur::VideoSource, temporal_decode::NativeFrame, video};
use gstreamer as gst;
use image::RgbaImage;
use std::{collections::VecDeque, sync::{Arc, Mutex, Weak, atomic::{AtomicU64, Ordering}}};

const MAX_BYTES: usize = 192 * 1024 * 1024;
const MAX_FRAMES: usize = 6;
static SOURCES: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());

enum Pixels { Buffer(gst::Buffer), Image(Arc<RgbaImage>) }
struct Entry {
    start: f64,
    end: Arc<AtomicU64>,
    asset: usize,
    continuity: usize,
    bytes: usize,
    pixels: Mutex<Pixels>,
}
impl Entry {
    fn end(&self) -> f64 { f64::from_bits(self.end.load(Ordering::Acquire)) }
}
struct Shared {
    source: VideoSource,
    dimensions: (u32, u32),
    window_capture: bool,
    frames: Mutex<VecDeque<Arc<Entry>>>,
}

#[derive(Clone)]
pub(super) struct PrimaryFrames(Arc<Shared>);

impl PrimaryFrames {
    pub fn new(source: VideoSource, dimensions: (u32, u32), window_capture: bool) -> Self {
        let shared = Arc::new(Shared { source, dimensions, window_capture, frames: Mutex::new(VecDeque::new()) });
        let mut sources = SOURCES.lock().unwrap();
        sources.retain(|source| source.strong_count() > 0);
        sources.push(Arc::downgrade(&shared));
        Self(shared)
    }

    /// The native buffer's PTS is local to its asset. Never infer missing
    /// duration from a nominal frame rate, and never extend across an asset.
    pub fn push(&self, buffer: gst::Buffer, asset: usize, offset: f64, bounds: (f64, f64), continuity: usize) {
        let Some(pts) = buffer.pts() else { return; };
        let duration = buffer.duration().map_or(0.0, |duration| duration.nseconds() as f64 / 1e9);
        let bounds=(super::temporal_decode::source_clock(bounds.0),super::temporal_decode::source_clock(bounds.1));
        let raw_start = super::temporal_decode::source_clock(offset + pts.nseconds() as f64 / 1e9);
        let raw_end = super::temporal_decode::source_clock(raw_start + duration);
        if raw_start < bounds.0 && (duration <= 0.0 || raw_end <= bounds.0) { return; }
        let start = raw_start.max(bounds.0);
        let end = raw_end.min(bounds.1);
        let bytes = buffer.size();
        if start >= bounds.1 || bytes == 0 || bytes > MAX_BYTES { return; }
        let mut frames = self.0.frames.lock().unwrap();
        if frames.iter().any(|frame| frame.asset == asset && frame.continuity == continuity && frame.start == start) { return; }
        if let Some(previous) = frames.back().filter(|frame| frame.asset == asset
            && frame.continuity == continuity && frame.start < start) {
            // A VFR picture lasts until the next actual source picture, even
            // when its buffer duration reports a nominal cadence. A seek or
            // asset transition starts a new continuity and cannot fill a gap.
            previous.end.store(start.min(bounds.1).to_bits(), Ordering::Release);
        }
        frames.push_back(Arc::new(Entry { start, end: Arc::new(AtomicU64::new(end.to_bits())), asset, continuity, bytes, pixels: Mutex::new(Pixels::Buffer(buffer)) }));
        while frames.len() > MAX_FRAMES || frames.iter().map(|frame| frame.bytes).sum::<usize>() > MAX_BYTES {
            frames.pop_front();
        }
    }
}

pub(super) fn frame(source: &VideoSource, dimensions: (u32, u32), asset: usize, time: f64) -> Option<NativeFrame> {
    let time=super::temporal_decode::source_clock(time);
    let sources: Vec<_> = SOURCES.lock().ok()?.iter().rev().filter_map(Weak::upgrade).collect();
    for shared in sources {
        if shared.source.path != source.path || shared.source.revision != source.revision || shared.dimensions != dimensions { continue; }
        let entry = shared.frames.lock().ok()?.iter().rev()
            .find(|frame| frame.asset == asset && time >= frame.start && time < frame.end()).cloned();
        let Some(entry) = entry else { continue; };
        // No image copy in the transport probe. Convert once, off the UI
        // thread, and replace the retained buffer with the shared pixels.
        let mut pixels = entry.pixels.lock().ok()?;
        if let Pixels::Buffer(buffer) = &*pixels {
            let bytes = buffer.map_readable().ok()?;
            if bytes.len() != dimensions.0 as usize * dimensions.1 as usize * 4 { continue; }
            let decoded = video::present_frame(video::DecodedFrame { time: entry.start,
                width: dimensions.0, height: dimensions.1, rgba: bytes.to_vec() }, shared.window_capture);
            let image = Arc::new(RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)?);
            drop(bytes);
            *pixels = Pixels::Image(image);
        }
        if let Pixels::Image(image) = &*pixels {
            return Some(NativeFrame { start: entry.start, end: entry.end(), end_signal: Some(entry.end.clone()), image: image.clone() });
        }
    }
    None
}

/// True only when an active native transport has advanced beyond a required
/// picture. A live preview may discard that obsolete job instead of seeking;
/// paused and exported frames always retain the accurate fallback.
pub(super) fn expired(source:&VideoSource, dimensions:(u32,u32), asset:usize, time:f64)->bool {
    let time=super::temporal_decode::source_clock(time);
    let sources:Vec<_>=SOURCES.lock().unwrap().iter().rev().filter_map(Weak::upgrade).collect();
    let mut oldest=f64::INFINITY;
    let mut newest=f64::NEG_INFINITY;
    for shared in sources {
        if shared.source.path!=source.path || shared.source.revision!=source.revision || shared.dimensions!=dimensions {continue;}
        for entry in shared.frames.lock().unwrap().iter().filter(|entry|entry.asset==asset) {
            if time>=entry.start && time<entry.end() {return false;}
            oldest=oldest.min(entry.start);newest=newest.max(entry.end());
        }
    }
    let expired=oldest.is_finite() && time<oldest;
    #[cfg(test)] if expired && std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some() {
        eprintln!("temporal obsolete t={time:.6} ring={oldest:.6}..{newest:.6}");
    }
    expired
}

/// The interpolated UI clock can request the next picture a few milliseconds
/// before its native buffer arrives. Wait briefly on this worker rather than
/// initiating a duplicate accurate seek. Old, distant and unregistered source
/// requests do not wait; their independent decoder remains the exact fallback.
pub(super) fn imminent_frame(source: &VideoSource, dimensions: (u32,u32), asset: usize,
    time: f64, cancelled: &impl Fn()->bool) -> Option<NativeFrame> {
    if let Some(frame) = frame(source, dimensions, asset, time) { return Some(frame); }
    let imminent = SOURCES.lock().ok()?.iter().filter_map(Weak::upgrade).any(|shared| {
        shared.source.path == source.path && shared.source.revision == source.revision
            && shared.dimensions == dimensions && shared.frames.lock().is_ok_and(|frames|
                frames.back().is_some_and(|last| last.asset == asset && time >= last.start && time - last.end() < 0.1))
    });
    if !imminent { return None; }
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(80) && !cancelled() {
        std::thread::sleep(std::time::Duration::from_millis(2));
        if let Some(frame) = frame(source, dimensions, asset, time) { return Some(frame); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buffer(time: f64, duration: Option<f64>, red: u8) -> gst::Buffer {
        let mut buffer = gst::Buffer::from_mut_slice(vec![red, 0, 0, 255]);
        let buffer_mut = buffer.get_mut().unwrap();
        buffer_mut.set_pts(gst::ClockTime::from_nseconds((time * 1e9).round() as u64));
        buffer_mut.set_duration(duration.map(|duration| gst::ClockTime::from_nseconds((duration * 1e9).round() as u64)));
        buffer
    }
    #[test]
    fn nanosecond_boundaries_and_later_pts_corrections_remain_authoritative() {
        gst::init().unwrap();
        let source=VideoSource {path:std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()),size:(1,1),revision:1};
        let cache=PrimaryFrames::new(source.clone(),(1,1),false);
        // Nonzero bank offsets must use the same clock domain as local PTS.
        cache.push(buffer(1.0,Some(1.0),10),2,0.1,(0.1,3.0),1);
        let retained=frame(&source,(1,1),2,1.15).unwrap();
        cache.push(buffer(1.1,Some(0.1),20),2,0.1,(0.1,3.0),1);
        assert_eq!(f64::from_bits(retained.end_signal.unwrap().load(Ordering::Acquire)),1.2);
        for (time,red) in [(1.2-1e-9,10),(1.2,20),(0.1+1.1,20),(1.2+1e-9,20)] {
            assert_eq!(frame(&source,(1,1),2,time).unwrap().image.get_pixel(0,0)[0],red,"{time:.12}");
        }
    }

    #[test]
    fn native_intervals_preserve_high_fps_variable_duration_and_asset_identity() {
        gst::init().unwrap();
        let source = VideoSource { path: std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()), size:(1,1), revision:7 };
        let cache = PrimaryFrames::new(source.clone(), (1,1), false);
        cache.push(buffer(0.0, Some(1.0/60.0), 10), 0, 0.0, (0.0, 0.125), 0);
        cache.push(buffer(1.0/60.0, Some(0.108333333), 20), 0, 0.0, (0.0, 0.125), 0);
        cache.push(buffer(0.0, Some(0.2), 30), 1, 0.125, (0.125, 0.325), 1);
        assert_eq!(frame(&source,(1,1),0,0.01).unwrap().image.get_pixel(0,0)[0],10);
        let slow=frame(&source,(1,1),0,0.12).unwrap();
        assert_eq!(slow.image.get_pixel(0,0)[0],20);
        assert!(Arc::ptr_eq(&slow.image,&frame(&source,(1,1),0,0.10).unwrap().image));
        assert!(frame(&source,(1,1),0,0.13).is_none());
        assert_eq!(frame(&source,(1,1),1,0.13).unwrap().image.get_pixel(0,0)[0],30);
        let mut changed=source.clone();changed.revision+=1;
        assert!(frame(&changed,(1,1),1,0.13).is_none());
        assert!(frame(&source,(2,1),1,0.13).is_none());
        cache.push(buffer(0.0,Some(1.0/30.0),35),3,0.0,(0.0,2.0),4);
        cache.push(buffer(1.0,Some(1.0/30.0),36),3,0.0,(0.0,2.0),4);
        assert_eq!(frame(&source,(1,1),3,0.8).unwrap().image.get_pixel(0,0)[0],35,"VFR hold ends at next native PTS");
        assert!(frame(&source,(1,1),3,1.5).is_none(),"unknown final interval must not guess a hold");
        cache.push(buffer(0.0,Some(1.0/30.0),50),4,0.0,(1.5,2.0),5);
        cache.push(buffer(1.5,Some(1.0/30.0),60),4,0.0,(1.5,2.0),5);
        assert!(frame(&source,(1,1),4,0.8).is_none(),"pre-seek preroll must never become a long source hold");
        assert_eq!(frame(&source,(1,1),4,1.51).unwrap().image.get_pixel(0,0)[0],60);
        cache.push(buffer(0.4,None,40),1,0.0,(0.4,0.5),2);
        assert!(frame(&source,(1,1),1,0.41).is_none(),"missing duration cannot create a guessed interval");
        for i in 0..20 {cache.push(buffer(1.0+i as f64*0.1,Some(0.1),i),2,0.0,(1.0,4.0),3);}
        assert_eq!(cache.0.frames.lock().unwrap().len(),MAX_FRAMES);
        assert!(frame(&source,(1,1),2,1.0).is_none());
    }
}
