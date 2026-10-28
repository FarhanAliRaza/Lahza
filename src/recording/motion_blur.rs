//! Shared, cut-aware shutter sampling for preview and export.
//! Planning only evaluates immutable timelines; decoding and drawing stay off UI.
use super::{
    clips::RecordingClipTimeline,
    pointer_timeline::PointerTimeline,
    scene::{CameraOverlay, PointerOverlay, SceneGeometry, SceneStyle},
    viewport::{ViewportFrame, ViewportTimeline},
};
use image::RgbaImage;
use std::{collections::VecDeque, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Interactive,
    Playing,
    High,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MotionSample {
    pub time: f64,
    pub source_time: Option<f64>,
    pub viewport: ViewportFrame,
    pub pointer: Option<PointerOverlay>,
    pub camera: CameraOverlay,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MotionPlan {
    pub samples: Vec<MotionSample>,
    pub motion_pixels: f32,
    pub trail_pixels: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoSource {
    pub path: PathBuf,
    pub size: (u32, u32),
    pub revision: u64,
}

/// A shutter never reaches into another clip or across an authored hard cut.
fn shutter_bounds(
    time: f64,
    half: f64,
    duration: f64,
    clips: Option<&RecordingClipTimeline>,
    style: &SceneStyle,
    viewport: &ViewportTimeline,
) -> (f64, f64) {
    let mut start = (time - half).max(0.0);
    let mut end = (time + half).min((duration - 1e-7).max(time));
    if let Some(clips) = clips {
        if clips.content_source_time_at(time).is_none() {
            return (time, time);
        }
        let Some(location) = clips.location_at(time) else {
            return (time, time);
        };
        let clip = &clips.segments[location.segment_index];
        // A purely editorial split keeps one continuous shutter. Removed
        // source ranges, speed changes, gaps, and asset switches remain cuts.
        let continuous = |a: &super::clips::RecordingClipSegment,
                          b: &super::clips::RecordingClipSegment| {
            b.gap_before == 0.0
                && a.source_bounds == b.source_bounds
                && (a.source_end - b.source_start).abs() < 1e-6
                && (a.speed - b.speed).abs() < 1e-6
        };
        let mut lo = location.segment_index;
        let mut hi = lo;
        let mut editor_start = location.editor_start;
        let mut editor_end = location.editor_start + clip.editor_duration();
        while lo > 0 && continuous(&clips.segments[lo - 1], &clips.segments[lo]) {
            lo -= 1;
            editor_start -= clips.segments[lo].editor_duration();
        }
        while hi + 1 < clips.segments.len()
            && continuous(&clips.segments[hi], &clips.segments[hi + 1])
        {
            hi += 1;
            editor_end += clips.segments[hi].editor_duration();
        }
        start = start.max(editor_start);
        end = end.min(editor_end - 1e-7);
        for region in style
            .camera_regions
            .iter()
            .filter(|r| r.enabled && r.transition <= 0.0)
        {
            for boundary in [region.start, region.end] {
                let boundary = location.editor_start + (boundary - clip.source_start) / clip.speed;
                if boundary <= time {
                    start = start.max(boundary);
                } else {
                    end = end.min(boundary - 1e-7);
                }
            }
        }
    }
    viewport.constrain_shutter(time, start, end)
}

fn sample(
    style: &SceneStyle,
    viewport: &ViewportTimeline,
    pointer: Option<&PointerTimeline>,
    clips: Option<&RecordingClipTimeline>,
    time: f64,
    canvas: (u32, u32),
    camera_aspect: f64,
) -> MotionSample {
    let source_time = clips.map_or(Some(time), |clips| clips.content_source_time_at(time));
    let viewport = viewport.frame_at(time);
    let mut camera = style.camera_at(
        source_time.unwrap_or(time),
        canvas,
        camera_aspect,
        viewport.magnification,
    );
    if source_time.is_none() {
        camera.enabled = false;
        camera.pose = None;
    }
    MotionSample {
        time,
        source_time,
        viewport,
        camera,
        pointer: pointer
            .and_then(|timeline| timeline.frame_at(time))
            .map(|frame| PointerOverlay { frame }),
    }
}

fn projected_points(
    style: &SceneStyle,
    state: &MotionSample,
    canvas: (u32, u32),
    source: (u32, u32),
    camera_aspect: f64,
) -> Vec<(f64, f64)> {
    let mut style = style.clone();
    style.camera = state.camera;
    let geometry = SceneGeometry::layout(
        canvas.0 as f64,
        canvas.1 as f64,
        source.0 as f64,
        source.1 as f64,
        &style,
    );
    let viewport = geometry.camera_viewport(state.viewport, &style, source);
    let projection = geometry.projection(style.transform.with_motion(viewport));
    let (left, top, width, height) = style.source_crop.visible_rect(viewport);
    let project = |x: f64, y: f64| projection.project((x - left) / width, (y - top) / height);
    let mut points = vec![
        project(0.0, 0.0),
        project(1.0, 0.0),
        project(0.0, 1.0),
        project(1.0, 1.0),
    ];
    if let Some(pointer) = &state.pointer {
        let frame = &pointer.frame;
        let center = project(frame.location.x, frame.location.y);
        points.push(center);
        // Rotation and click compression move artwork even at a stationary
        // hotspot. Estimate its two axes so those changes also get a shutter.
        let unit = geometry.media.height
            * projection.screen_scale_at(
                (frame.location.x - left) / width,
                (frame.location.y - top) / height,
            )
            / height;
        let radius = unit
            * frame
                .bitmap
                .as_ref()
                .map_or(0.032, |b| b.reference_height * 1.5)
            * frame.magnification
            * f64::from(style.pointer.scale)
            / 100.0;
        let angle = frame.tilt_degrees.to_radians();
        points.extend([
            (
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            ),
            (
                center.0 - radius * angle.sin(),
                center.1 + radius * angle.cos(),
            ),
        ]);
    } else {
        points.extend([(0.0, 0.0); 3]);
    }
    let rect = state.camera.rect_for_frame(
        canvas.0 as f64,
        canvas.1 as f64,
        camera_aspect,
        state.viewport.magnification,
    );
    points.extend([
        (rect.x, rect.y),
        (rect.x + rect.width, rect.y + rect.height),
    ]);
    let crop = super::camera_layout::source_crop(state.camera, camera_aspect, rect);
    for (u, v) in [(0.0, 0.0), (1.0, 1.0)] {
        points.push((
            rect.x + (u - crop.x) / crop.width * rect.width,
            rect.y + (v - crop.y) / crop.height * rect.height,
        ));
    }
    points.push((state.camera.opacity() * rect.width.min(rect.height), 0.0));
    if let Some(lens) = state.viewport.loupe {
        let center = project(lens.center.x, lens.center.y);
        let radius = lens.radius * geometry.media.width.max(geometry.media.height);
        points.extend([
            center,
            (center.0 + radius, center.1 + radius),
            (
                center.0 + radius * lens.magnification,
                center.1 + radius * lens.presence,
            ),
        ]);
    } else {
        points.extend([(0.0, 0.0); 3]);
    }
    points
}

/// Adaptive sample counts are bounded to six while playing and sixteen for a
/// settled/export frame. The same shutter and timelines drive both paths.
pub fn plan(
    style: &SceneStyle,
    viewport: &ViewportTimeline,
    pointer: Option<&PointerTimeline>,
    clips: Option<&RecordingClipTimeline>,
    time: f64,
    duration: f64,
    frame_rate: f64,
    canvas: (u32, u32),
    source: (u32, u32),
    camera_aspect: f64,
    quality: Quality,
) -> MotionPlan {
    if quality == Quality::Interactive {
        return MotionPlan {
            samples: vec![sample(style, viewport, pointer, clips, time, canvas, camera_aspect)],
            motion_pixels: 0.0,
            trail_pixels: 0.0,
        };
    }
    let fps = if frame_rate.is_finite() {
        frame_rate.clamp(1.0, 240.0)
    } else {
        30.0
    };
    let amount = style.animation.blur_amount();
    let half = amount / fps * 0.5;
    let (start, end) = shutter_bounds(time, half, duration, clips, style, viewport);
    let center = sample(style, viewport, pointer, clips, time, canvas, camera_aspect);
    let a = sample(
        style,
        viewport,
        pointer,
        clips,
        start,
        canvas,
        camera_aspect,
    );
    let b = sample(style, viewport, pointer, clips, end, canvas, camera_aspect);
    let extent = projected_points(style, &a, canvas, source, camera_aspect)
        .into_iter()
        .zip(projected_points(style, &b, canvas, source, camera_aspect))
        .map(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1))
        .filter(|value| value.is_finite())
        .fold(0.0_f64, f64::max);
    // Enhancement still needs the full-frame motion estimate when blur is off.
    let motion = if amount > 0.0 {
        extent / amount
    } else {
        let (lo, hi) = shutter_bounds(time, 0.5 / fps, duration, clips, style, viewport);
        projected_points(
            style,
            &sample(style, viewport, pointer, clips, lo, canvas, camera_aspect),
            canvas,
            source,
            camera_aspect,
        )
        .into_iter()
        .zip(projected_points(
            style,
            &sample(style, viewport, pointer, clips, hi, canvas, camera_aspect),
            canvas,
            source,
            camera_aspect,
        ))
        .map(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1))
        .filter(|v| v.is_finite())
        .fold(0.0_f64, f64::max)
    };
    let (limit, spacing) = match quality {
        Quality::Interactive => (1, f64::INFINITY),
        Quality::Playing => (6, 16.0),
        Quality::High => (16, 4.0),
    };
    let count = if amount <= 0.0 || extent < 0.5 || end - start < 1e-7 {
        1
    } else {
        ((extent / spacing).ceil() as usize + 1).clamp(2, limit)
    };
    let samples = if count == 1 {
        vec![center]
    } else {
        (0..count)
            .map(|i| {
                let t = start + (end - start) * (i as f64 + 0.5) / count as f64;
                sample(style, viewport, pointer, clips, t, canvas, camera_aspect)
            })
            .collect()
    };
    MotionPlan {
        samples,
        motion_pixels: motion.min(1e5) as f32,
        trail_pixels: extent.min(1e5) as f32,
    }
}

/// Native frames near a shutter are decoded once and shared by its samples.
/// This cache is worker-owned; neither seeking nor decoding blocks the UI.
struct TemporalAsset {
    path: PathBuf,
    start: f64,
    duration: f64,
    still: bool,
}

struct CachedFrame {
    asset: usize,
    start: f64,
    end: f64,
    primary_end: Option<(Arc<std::sync::atomic::AtomicU64>, f64)>,
    image: Arc<RgbaImage>,
}

impl CachedFrame {
    fn end(&self)->f64 {
        self.primary_end.as_ref().map_or(self.end,|(end,offset)|
            super::temporal_decode::source_clock(f64::from_bits(end.load(std::sync::atomic::Ordering::Acquire))-*offset))
    }
}

#[derive(Default)]
pub struct FrameCache {
    source: Option<VideoSource>,
    bank: Option<super::media_bank::MediaBank>,
    assets: Vec<TemporalAsset>,
    decoder: Option<(usize, super::temporal_decode::NativeDecoder)>,
    dimensions: (u32, u32),
    window_capture: bool,
    duration: f64,
    frames: VecDeque<CachedFrame>,
    #[cfg(test)]
    seeks: usize,
}

impl FrameCache {
    fn prepare(&mut self, source: &VideoSource) -> Result<(), String> {
        if self.source.as_ref() == Some(source) { return Ok(()); }
        let bank = super::media_bank::MediaBank::load(&source.path).map_err(|e|e.to_string())?;
        let info = match &bank {
            Some(bank) => bank.info(),
            None => super::video::probe_media(&source.path).map_err(|e|e.to_string())?,
        };
        let assets = if let Some(bank)=&bank {
            let mut start=0.0;
            bank.entries.iter().enumerate().map(|(index,entry)| {
                let asset=TemporalAsset { path:bank.entry_path(&source.path,index), start,
                    duration:entry.info.duration, still:entry.still };
                start+=entry.info.duration;
                asset
            }).collect()
        } else { vec![TemporalAsset { path:source.path.clone(),start:0.0,duration:info.duration,still:false }] };
        self.dimensions=super::video::decode_dimensions(&info,source.size.0,source.size.1);
        self.duration=info.duration;
        self.window_capture=info.window_capture;
        self.assets=assets;
        self.bank=bank;
        self.decoder=None;
        self.source=Some(source.clone());
        self.frames.clear();
        #[cfg(test)] { self.seeks=0; }
        Ok(())
    }

    fn trim(&mut self) {
        while self.frames.len() > 6 || (self.frames.len() > 2
            && self.frames.iter().map(|frame|frame.image.as_raw().len()).sum::<usize>() > 192*1024*1024) {
            self.frames.pop_front();
        }
    }

    /// Camera tracks can end before the screen. Do not resurrect their final
    /// frame merely because a decoder can hold at EOF.
    pub fn contains_time(&mut self, source: &VideoSource, time: f64) -> Result<bool, String> {
        self.prepare(source)?;
        Ok(time.is_finite() && time >= 0.0 && time < self.duration)
    }

    /// Start bounded native preroll ahead of the first moving shutter. This
    /// never labels a supplied image with an estimated timestamp, and it does
    /// not wait for a decoded sample. Actual sampling still uses encoded PTS.
    pub fn warm(&mut self, source: &VideoSource, time: f64) -> Result<(), String> {
        self.prepare(source)?;
        let time = time.clamp(0.0, (self.duration - 1e-7).max(0.0));
        let index = self.assets.partition_point(|asset| super::temporal_decode::source_clock(asset.start + asset.duration) <= time)
            .min(self.assets.len() - 1);
        let asset = &self.assets[index];
        if !asset.still && self.decoder.as_ref().is_none_or(|(active, _)| *active != index) {
            self.decoder = Some((index, super::temporal_decode::NativeDecoder::open(
                &asset.path, self.dimensions, self.window_capture, index != 0, asset.duration)?));
        }
        Ok(())
    }

    pub fn frame(&mut self, source:&VideoSource, time:f64, cancelled:&impl Fn()->bool) -> Result<Arc<RgbaImage>,String> {
        self.frame_internal(source,time,false,cancelled)?.ok_or_else(||"missing exact temporal frame".into())
    }

    pub fn playing_frame(&mut self, source:&VideoSource, time:f64, cancelled:&impl Fn()->bool) -> Result<Option<Arc<RgbaImage>>,String> {
        self.frame_internal(source,time,true,cancelled)
    }

    fn frame_internal(&mut self, source:&VideoSource, time:f64, drop_expired:bool, cancelled:&impl Fn()->bool) -> Result<Option<Arc<RgbaImage>>,String> {
        if cancelled() { return Err("Temporal frame decoding cancelled".into()); }
        if !time.is_finite() { return Err("invalid temporal timestamp".into()); }
        let time=super::temporal_decode::source_clock(time);
        #[cfg(test)] let trace_started=std::time::Instant::now();
        self.prepare(source)?;
        #[cfg(test)] let trace_prepared=trace_started.elapsed();
        #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some(){eprintln!("temporal prepared {time:.6} {trace_prepared:?}");}
        let time=time.clamp(0.0,(self.duration-1e-7).max(0.0));
        let asset_index=self.assets.partition_point(|asset|super::temporal_decode::source_clock(asset.start+asset.duration) <= time)
            .min(self.assets.len()-1);
        let asset=&self.assets[asset_index];
        let local=super::temporal_decode::source_clock(time-asset.start);
        if let Some(frame) = super::primary_frames::imminent_frame(source, self.dimensions, asset_index, time, cancelled) {
            #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some() {
                eprintln!("temporal primary hit t={time:.6} pts={:.6}..{:.6}",frame.start,frame.end);
            }
            // Retain nearby native images if the transport's ring advances
            // during composition. Asset-local identity matches decoder frames.
            if let Some(cached) = self.frames.iter_mut().find(|cached|cached.asset==asset_index && cached.start==super::temporal_decode::source_clock(frame.start-asset.start)) {
                cached.end=super::temporal_decode::source_clock(frame.end-asset.start).min(asset.duration);
                cached.primary_end=frame.end_signal.clone().map(|end|(end,asset.start));
            } else {
                self.frames.push_back(CachedFrame {asset:asset_index,start:super::temporal_decode::source_clock(frame.start-asset.start),
                    end:super::temporal_decode::source_clock(frame.end-asset.start).min(asset.duration),primary_end:frame.end_signal.clone().map(|end|(end,asset.start)),image:frame.image.clone()});
                self.trim();
            }
            return Ok(Some(frame.image));
        }
        // Intervals are actual encoded PTS in the asset's own clock. Two
        // assets can never alias, even if their boundary falls within one
        // aggregate frame-rate bucket or their native frame rates differ.
        if let Some(frame)=self.frames.iter().find(|frame| frame.asset==asset_index
            && local >= frame.start && local < frame.end()) { return Ok(Some(frame.image.clone())); }
        if drop_expired && super::primary_frames::expired(source,self.dimensions,asset_index,time) {
            return Ok(None);
        }
        let (start,end,image)=if asset.still {
            let frame=self.bank.as_ref().ok_or("still without media bank")?
                .decode_frame(&source.path,asset.start,source.size.0,source.size.1).map_err(|e|e.to_string())?;
            let image=RgbaImage::from_raw(frame.width,frame.height,frame.rgba).ok_or("invalid temporal still")?;
            (0.0,asset.duration,Arc::new(image))
        } else {
            if self.decoder.as_ref().is_none_or(|(index,_)|*index!=asset_index) {
                self.decoder=Some((asset_index,super::temporal_decode::NativeDecoder::open(
                    &asset.path,self.dimensions,self.window_capture,asset_index!=0,asset.duration)?));
            }
            let decoder=&mut self.decoder.as_mut().unwrap().1;
            #[cfg(test)] let old_seeks=decoder.seeks;
            let frame=decoder.frame(local,cancelled)?;
            #[cfg(test)] { self.seeks+=decoder.seeks-old_seeks; }
            (frame.start,frame.end.min(asset.duration),frame.image)
        };
        if cancelled() { return Err("Temporal frame decoding cancelled".into()); }
        #[cfg(test)] if std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some() {
            eprintln!("temporal decode t={time:.6} pts={start:.6}..{end:.6} prepare={trace_prepared:?} total={:?}",trace_started.elapsed());
        }
        self.frames.push_back(CachedFrame {asset:asset_index,start,end,primary_end:None,image:image.clone()});
        self.trim();
        Ok(Some(image))
    }
}

pub struct Accumulator {
    size: (u32, u32),
    sum: Vec<u32>,
    count: u32,
}
impl Accumulator {
    pub fn new(image: RgbaImage) -> Self {
        Self {
            size: image.dimensions(),
            sum: image.into_raw().into_iter().map(u32::from).collect(),
            count: 1,
        }
    }
    pub fn add(&mut self, image: &RgbaImage) {
        assert_eq!(image.dimensions(), self.size);
        for (sum, value) in self.sum.iter_mut().zip(image.as_raw()) {
            *sum += u32::from(*value);
        }
        self.count += 1;
    }
    pub fn finish(self) -> RgbaImage {
        RgbaImage::from_raw(
            self.size.0,
            self.size.1,
            self.sum
                .into_iter()
                .map(|sum| ((sum + self.count / 2) / self.count) as u8)
                .collect(),
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        camera_layout::CameraLayout,
        camera_timeline::CameraRegion,
        clips::RecordingClipSegment,
        model::{NormalizedPoint, PointerCaptureFile, PointerTravelKind, PointerTravelSample},
        pointer_timeline::{PointerMotion, PointerTimelineOptions},
        viewport::ZoomCue,
    };
    use super::*;

    #[test]
    fn temporal_cache_advances_by_encoded_pts_and_seeks_only_for_discontinuities() {
        use std::process::Command;
        let root=std::env::temp_dir().join(format!("lahza-shutter-sequential-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path=root.join("numbered.mkv");
        let output=Command::new("ffmpeg").args(["-v","error","-y","-f","lavfi","-i",
            "nullsrc=s=96x64:r=30:d=2,format=gbrp,geq=r='N*3':g=0:b=0,format=bgra",
            "-c:v","ffv1"]).arg(&path).output().unwrap();
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        let source=VideoSource {path,size:(96,64),revision:0};
        let mut cache=FrameCache::default();
        cache.warm(&source, 0.0).unwrap();
        assert!(cache.frames.is_empty(), "warming must not invent a timestamped sample");
        for index in 0..40 {
            let time=(index as f64+0.25)/30.0;
            let frame=cache.frame(&source,time,&||false).unwrap();
            let red=frame.get_pixel(48,32)[0];
            assert!((i32::from(red)-index*3).abs()<=1,"PTS frame {index}: red={red}");
            assert!(Arc::ptr_eq(&frame,&cache.frame(&source,time+0.001,&||false).unwrap()));
        }
        assert_eq!(cache.seeks,1,"forward playback must not seek per encoded frame");
        assert!(cache.frames.len()<=6);
        assert!((i32::from(cache.frame(&source,2.25/30.0,&||false).unwrap().get_pixel(48,32)[0])-6).abs()<=1);
        assert_eq!(cache.seeks,2,"reverse outside the retained frames needs one exact seek");
        assert!((i32::from(cache.frame(&source,51.25/30.0,&||false).unwrap().get_pixel(48,32)[0])-153).abs()<=1);
        assert_eq!(cache.seeks,3,"a random jump needs one exact seek");
        assert!(cache.frame(&source,1.8,&||true).is_err());
        assert!(cache.frame(&source,1.8,&||false).is_ok(),"cancelled requests must not poison the decoder");
        assert!((i32::from(cache.frame(&source,1.999,&||false).unwrap().get_pixel(48,32)[0])-177).abs()<=1,
            "the last encoded frame remains available through its actual EOF interval");
        for (time,expected) in [(1.1-1e-9,96),(1.1,99),(0.3+0.8,99),(1.1+1e-9,99)] {
            assert!((i32::from(cache.frame(&source,time,&||false).unwrap().get_pixel(48,32)[0])-expected).abs()<=1,
                "native seek/cache must agree on the nanosecond boundary {time:.12}");
        }
        drop(cache);
        // The cache retains the primary interval's authoritative end, even
        // after the source ring evicts its pixels and a later PTS shortens it.
        let primary=super::super::primary_frames::PrimaryFrames::new(source.clone(),(96,64),false);
        let push=|time:f64,duration:f64,red:u8| {
            let mut buffer=gstreamer::Buffer::from_mut_slice([red,0,0,255].repeat(96*64));
            let writable=buffer.get_mut().unwrap();
            writable.set_pts(gstreamer::ClockTime::from_nseconds((time*1e9).round() as u64));
            writable.set_duration(gstreamer::ClockTime::from_nseconds((duration*1e9).round() as u64));
            primary.push(buffer,0,0.0,(0.0,2.0),0);
        };
        push(0.0,1.0,0);
        let mut retained=FrameCache::default();
        assert_eq!(retained.frame(&source,0.01,&||false).unwrap().get_pixel(48,32)[0],0);
        push(1.0/30.0,1.0/30.0,3);
        assert_eq!(retained.frame(&source,0.04,&||false).unwrap().get_pixel(48,32)[0],3);
        for index in 2..12 {push(index as f64/30.0,1.0/30.0,index*3);}
        assert_eq!(retained.frame(&source,0.04,&||false).unwrap().get_pixel(48,32)[0],3,
            "evicted primary A must not retain its old overreported end across B");
        drop(retained);drop(primary);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn temporal_asset_boundaries_never_share_an_encoded_interval_or_shutter() {
        use super::super::{media_bank::{MediaBank,MediaEntry},video::MediaInfo};
        let root=std::env::temp_dir().join(format!("lahza-shutter-assets-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for (name,pixel) in [("red.png",image::Rgba([255,0,0,255])),("blue.png",image::Rgba([0,0,255,255]))] {
            RgbaImage::from_pixel(64,48,pixel).save(root.join(name)).unwrap();
        }
        let info=MediaInfo {duration:0.125,width:64,height:48,frame_rate:30.0,has_audio:false,window_capture:false};
        let bank=MediaBank {entries:vec![
            MediaEntry {file:"red.png".into(),info:info.clone(),still:true},
            MediaEntry {file:"blue.png".into(),info,still:true},
        ]};
        let path=root.join("mixed.media.json");bank.save(&path).unwrap();
        let source=VideoSource {path,size:(64,48),revision:0};
        let mut cache=FrameCache::default();
        // These are in the same nominal30fps global bucket, opposite sides
        // of an asset boundary. They must remain separate decoded images.
        let red=cache.frame(&source,0.12,&||false).unwrap();
        let blue=cache.frame(&source,0.13,&||false).unwrap();
        assert!(red.get_pixel(32,24)[0]>240 && blue.get_pixel(32,24)[2]>240);
        assert!(!Arc::ptr_eq(&red,&blue));
        assert!(Arc::ptr_eq(&red,&cache.frame(&source,0.05,&||false).unwrap()));
        assert!(Arc::ptr_eq(&blue,&cache.frame(&source,0.2,&||false).unwrap()));
        let clips=bank.ranges(&RecordingClipTimeline::full(0.25));
        assert_ne!(clips.segments[0].source_bounds,clips.segments[1].source_bounds);
        let viewport=ViewportTimeline::build_static(&[],0.25);
        let (_,before)=shutter_bounds(0.12,0.03,0.25,Some(&clips),&SceneStyle::default(),&viewport);
        let (after,_)=shutter_bounds(0.13,0.03,0.25,Some(&clips),&SceneStyle::default(),&viewport);
        assert!(before<0.125 && after>=0.125);
        drop(cache);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn evaluate(
        style: &SceneStyle,
        viewport: &ViewportTimeline,
        pointer: Option<&PointerTimeline>,
        clips: Option<&RecordingClipTimeline>,
        time: f64,
        quality: Quality,
    ) -> MotionPlan {
        plan(
            style,
            viewport,
            pointer,
            clips,
            time,
            4.0,
            30.0,
            (1920, 1080),
            (1920, 1080),
            16.0 / 9.0,
            quality,
        )
    }
    fn moving_pointer() -> PointerTimeline {
        let capture = PointerCaptureFile {
            travel: (0..=120)
                .map(|i| PointerTravelSample {
                    time: i as f64 / 30.0,
                    x: ((i as f64 / 30.0 * 800.0) % 1920.0) / 1920.0,
                    y: 400.0 / 1080.0,
                    kind: PointerTravelKind::Move,
                    artwork_id: None,
                })
                .collect(),
            ..Default::default()
        };
        PointerTimeline::build(
            capture,
            4.0,
            1920.0,
            1080.0,
            PointerTimelineOptions {
                motion: PointerMotion::None,
                ..Default::default()
            },
        )
    }
    #[test]
    fn stationary_frames_are_exact_and_motion_without_blur_still_attenuates_enhancement() {
        let viewport = ViewportTimeline::build_static(&[], 4.0);
        let mut style = SceneStyle::default();
        style.animation.motion_blur = 100;
        assert_eq!(
            evaluate(&style, &viewport, None, None, 1.0, Quality::High)
                .samples
                .len(),
            1
        );
        let pointer = moving_pointer();
        style.animation.motion_blur = 0;
        let plan = evaluate(&style, &viewport, Some(&pointer), None, 1.0, Quality::High);
        assert_eq!(plan.samples.len(), 1);
        assert_eq!(plan.samples[0].time, 1.0);
        assert!(plan.motion_pixels > 10.0);
        assert_eq!(plan.trail_pixels, 0.0);
    }
    #[test]
    fn pointer_and_camera_motion_each_receive_independent_temporal_samples() {
        let viewport = ViewportTimeline::build_static(&[], 4.0);
        let mut style = SceneStyle::default();
        style.animation.motion_blur = 100;
        let pointer = moving_pointer();
        let p = evaluate(&style, &viewport, Some(&pointer), None, 1.0, Quality::High);
        assert!(p.samples.len() > 1);
        assert_ne!(
            p.samples.first().unwrap().pointer,
            p.samples.last().unwrap().pointer
        );
        assert_eq!(
            p.samples.first().unwrap().viewport,
            p.samples.last().unwrap().viewport
        );
        style.camera_regions.push(CameraRegion {
            start: 1.0,
            end: 3.0,
            transition: 0.35,
            overlay: CameraOverlay {
                enabled: true,
                layout: CameraLayout::Fullscreen,
                ..Default::default()
            },
            ..Default::default()
        });
        let p = evaluate(&style, &viewport, None, None, 1.175, Quality::High);
        assert!(p.samples.len() > 1);
        assert_ne!(
            p.samples.first().unwrap().camera.pose,
            p.samples.last().unwrap().camera.pose
        );
        assert!(p.samples.iter().all(|s| s.pointer.is_none()));
    }
    #[test]
    fn camera_crop_motion_is_sampled_without_moving_the_camera_frame() {
        let viewport = ViewportTimeline::build_static(&[], 4.0);
        let mut style = SceneStyle::default();
        style.animation.motion_blur = 100;
        style.camera.crop_zoom = 200;
        style.camera.crop_center = (0.25, 0.5);
        let mut overlay = style.camera;
        overlay.crop_center = (0.75, 0.5);
        style.camera_regions.push(CameraRegion {
            start: 1.0,
            end: 3.0,
            overlay,
            transition: 0.35,
            ..Default::default()
        });
        let p = evaluate(&style, &viewport, None, None, 1.175, Quality::High);
        assert!(p.samples.len() > 1);
        let first = &p.samples.first().unwrap().camera;
        let last = &p.samples.last().unwrap().camera;
        assert_eq!(first.pose.unwrap().camera, last.pose.unwrap().camera);
        assert_ne!(first.crop_center, last.crop_center);
    }

    #[test]
    fn shutter_never_crosses_source_edits_gaps_or_instant_layout_cuts() {
        let mut style = SceneStyle::default();
        style.animation.motion_blur = 100;
        let mut second = RecordingClipSegment::new(8.0, 10.0);
        second.speed = 2.0;
        second.gap_before = 0.5;
        let clips = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.0, 1.0), second]);
        let viewport = ViewportTimeline::build_static(&[], 4.0);
        assert_eq!(
            shutter_bounds(1.2, 0.2, 4.0, Some(&clips), &style, &viewport),
            (1.2, 1.2)
        );
        let (a, b) = shutter_bounds(1.501, 0.2, 4.0, Some(&clips), &style, &viewport);
        assert_eq!(a, 1.5);
        assert!(b < 2.5);
        for time in [a, 1.501, b] {
            let s = sample(
                &style,
                &viewport,
                None,
                Some(&clips),
                time,
                (1920, 1080),
                1.0,
            );
            assert!((s.source_time.unwrap() - (8.0 + (time - 1.5) * 2.0)).abs() < 1e-8);
        }
        style.camera_regions.push(CameraRegion {
            start: 8.25,
            end: 9.0,
            transition: 0.0,
            ..Default::default()
        });
        let (_, before) = shutter_bounds(1.62, 0.2, 4.0, Some(&clips), &style, &viewport);
        let (after, _) = shutter_bounds(1.63, 0.2, 4.0, Some(&clips), &style, &viewport);
        assert!(before < 1.625);
        assert_eq!(after, 1.625);
        let gap = sample(
            &style,
            &viewport,
            None,
            Some(&clips),
            1.2,
            (1920, 1080),
            1.0,
        );
        assert!(gap.source_time.is_none());
        assert_eq!(gap.camera.opacity(), 0.0);
    }
    #[test]
    fn splitting_a_continuous_clip_does_not_change_its_shutter() {
        let style = SceneStyle::default();
        let clips = RecordingClipTimeline::new(vec![
            RecordingClipSegment::new(0.0, 1.0),
            RecordingClipSegment::new(1.0, 3.0),
        ]);
        let mut cue = ZoomCue::pinned(0.5, 2.0, 3.0, NormalizedPoint { x: 0.5, y: 0.5 });
        cue.skips_easing = true;
        let viewport = ViewportTimeline::build(
            &[cue],
            &PointerTimeline::default(),
            &clips,
            &PointerCaptureFile::default(),
        );
        let (a, b) = shutter_bounds(1.0, 0.05, 3.0, Some(&clips), &style, &viewport);
        assert!(
            (a - 0.95).abs() < 1e-10 && (b - 1.05).abs() < 1e-10,
            "{a}..{b}"
        );
    }

    #[test]
    fn authored_hard_zoom_cuts_never_mix_both_sides() {
        let style = SceneStyle::default();
        let mut cue = ZoomCue::pinned(1.0, 2.0, 3.0, NormalizedPoint { x: 0.5, y: 0.5 });
        cue.skips_easing = true;
        let viewport = ViewportTimeline::build_static(&[cue], 4.0);
        let (_, end) = shutter_bounds(0.999, 0.05, 4.0, None, &style, &viewport);
        let (start, _) = shutter_bounds(1.0, 0.05, 4.0, None, &style, &viewport);
        assert!(end < 1.0);
        assert_eq!(start, 1.0);
        let (start, _) = shutter_bounds(2.0, 0.05, 4.0, None, &style, &viewport);
        assert_eq!(start, 2.0);
    }
    #[test]
    fn amount_changes_shutter_width_and_quality_only_bounds_sampling_density() {
        let mut style = SceneStyle::default();
        let viewport = ViewportTimeline::build_static(&[], 4.0);
        let pointer = moving_pointer();
        style.animation.motion_blur = 25;
        let short = evaluate(&style, &viewport, Some(&pointer), None, 1.0, Quality::High);
        style.animation.motion_blur = 100;
        let high = evaluate(&style, &viewport, Some(&pointer), None, 1.0, Quality::High);
        let playing = evaluate(
            &style,
            &viewport,
            Some(&pointer),
            None,
            1.0,
            Quality::Playing,
        );
        assert!(high.trail_pixels > short.trail_pixels * 3.9);
        assert_eq!(high.trail_pixels, playing.trail_pixels);
        assert!(playing.samples.len() <= 6 && high.samples.len() <= 16);
        assert!(playing.samples.len() <= high.samples.len());
        assert!(
            (high.samples.iter().map(|s| s.time).sum::<f64>() / high.samples.len() as f64 - 1.0)
                .abs()
                < 1e-12
        );
    }
    #[test]
    fn native_temporal_cache_reads_distinct_encoded_frames_and_reuses_only_same_frame() {
        use std::process::Command;
        let root = std::env::temp_dir().join(format!("lahza-shutter-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("temporal.mkv");
        let result = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x48:r=10:d=0.2",
                "-f",
                "lavfi",
                "-i",
                "color=blue:s=64x48:r=10:d=0.2",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0[v]",
                "-map",
                "[v]",
                "-c:v",
                "ffv1",
            ])
            .arg(&path)
            .output()
            .expect("FFmpeg required for media tests");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let source = VideoSource {
            path,
            size: (64, 48),
            revision: 1,
        };
        let mut cache = FrameCache::default();
        assert!(cache.contains_time(&source, 0.35).unwrap());
        assert!(!cache.contains_time(&source, 0.4).unwrap());
        assert!(cache.frame(&source, 0.1, &|| true).is_err());
        let red = cache.frame(&source, 0.015, &|| false).unwrap();
        let blue = cache.frame(&source, 0.215, &|| false).unwrap();
        assert!(red.get_pixel(32, 24)[0] > 240 && red.get_pixel(32, 24)[2] < 10);
        assert!(blue.get_pixel(32, 24)[2] > 240 && blue.get_pixel(32, 24)[0] < 10);
        assert!(Arc::ptr_eq(
            &blue,
            &cache.frame(&source, 0.28, &|| false).unwrap()
        ));
        let mut sum = Accumulator::new((*red).clone());
        sum.add(&blue);
        let mixed = sum.finish();
        assert!((120..=130).contains(&mixed.get_pixel(32, 24)[0]));
        assert!((120..=130).contains(&mixed.get_pixel(32, 24)[2]));
        drop(cache);
        std::fs::remove_dir_all(root).unwrap();
    }
}
