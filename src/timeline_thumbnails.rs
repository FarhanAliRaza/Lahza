//! Source-time anchored thumbnail tiles, sampled only for the visible viewport.
use crate::{cached_render_image, timeline_preview::PreviewDecoder, Studio};
use gpui::{Context, RenderImage, Timer};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

const BASE_TILES: f64 = 32.0;
const TARGET_WIDTH: f64 = 78.0;
const CACHE_LIMIT: usize = 192;
const QUEUE_LIMIT: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Key {
    level: u8,
    index: u32,
}

#[derive(Clone, PartialEq)]
struct Viewport {
    path: PathBuf,
    duration: f64,
    seconds_per_pixel: f64,
    scroll: f64,
    width: f64,
    clips: crate::recording::clips::RecordingClipTimeline,
}

#[derive(Default)]
pub(crate) struct TimelineThumbnailState {
    viewport: Option<Viewport>,
    generation: u64,
    cancel_generation: Arc<AtomicU64>,
    settled: bool,
    render_seconds_per_pixel: Option<f64>,
    busy: bool,
    cache: HashMap<Key, Arc<RenderImage>>,
    order: VecDeque<Key>,
    queue: VecDeque<Key>,
    failed: HashSet<Key>,
}

fn spacing(duration: f64, level: u8) -> f64 {
    duration / BASE_TILES / 2f64.powi(level as i32)
}

fn level_for(duration: f64, target: f64) -> u8 {
    let base = duration / BASE_TILES;
    if base <= 0.0 || target <= 0.0 {
        return 0;
    }
    (base / target.max(0.1)).log2().floor().max(0.0).min(20.0) as u8
}

fn visible_keys(viewport: &Viewport) -> Vec<Key> {
    let start = (viewport.scroll - TARGET_WIDTH).max(0.0) * viewport.seconds_per_pixel;
    let end = (viewport.scroll + viewport.width + TARGET_WIDTH) * viewport.seconds_per_pixel;
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    let starts = viewport.clips.clip_starts();
    for (index, clip) in viewport.clips.segments.iter().enumerate() {
        let left = start.max(starts[index]);
        let right = end.min(starts[index] + clip.editor_duration());
        if right <= left {
            continue;
        }
        let source_start = clip.source_start + (left - starts[index]) * clip.speed;
        let source_end = clip.source_start + (right - starts[index]) * clip.speed;
        let level = level_for(
            viewport.duration,
            TARGET_WIDTH * viewport.seconds_per_pixel * clip.speed,
        );
        if level == 0 {
            continue;
        }
        let step = spacing(viewport.duration, level);
        for index in (source_start / step).floor() as u32..=(source_end / step).floor() as u32 {
            let key = Key { level, index };
            if seen.insert(key) {
                keys.push(key);
            }
            if keys.len() == QUEUE_LIMIT {
                return keys;
            }
        }
    }
    keys
}

impl Studio {
    pub(crate) fn reset_timeline_thumbnails(&mut self) {
        self.timeline_thumbnails.generation = self.timeline_thumbnails.generation.wrapping_add(1);
        self.timeline_thumbnails
            .cancel_generation
            .store(self.timeline_thumbnails.generation, Ordering::SeqCst);
        self.timeline_thumbnails.viewport = None;
        self.timeline_thumbnails.render_seconds_per_pixel = None;
        self.timeline_thumbnails.settled = false;
        self.timeline_thumbnails.queue.clear();
        self.timeline_thumbnails.order.clear();
        self.timeline_thumbnails.failed.clear();
        self.retired_images.extend(
            self.timeline_thumbnails
                .cache
                .drain()
                .map(|(_, image)| image),
        );
    }

    pub(crate) fn refresh_timeline_thumbnails(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.video_playback_path() else {
            return;
        };
        if self.video_source_duration <= 0.0 {
            return;
        }
        let seconds_per_pixel = self.video_timeline_seconds_per_pixel();
        let width = self.video_timeline_viewport_width();
        if self
            .timeline_thumbnails
            .viewport
            .as_ref()
            .is_some_and(|old| {
                old.path == path
                    && old.duration == self.video_source_duration
                    && old.seconds_per_pixel == seconds_per_pixel
                    && old.width == width
                    && old.scroll == self.video_timeline_scroll
                    && old.clips == self.video_clip_timeline
            })
        {
            return;
        }
        let viewport = Viewport {
            path,
            duration: self.video_source_duration,
            seconds_per_pixel,
            scroll: self.video_timeline_scroll,
            width,
            clips: self.video_clip_timeline.clone(),
        };
        let changed_source = self
            .timeline_thumbnails
            .viewport
            .as_ref()
            .is_some_and(|old| old.path != viewport.path || old.duration != viewport.duration);
        if changed_source {
            self.retired_images.extend(
                self.timeline_thumbnails
                    .cache
                    .drain()
                    .map(|(_, image)| image),
            );
            self.timeline_thumbnails.order.clear();
            self.timeline_thumbnails.failed.clear();
        }
        self.timeline_thumbnails.viewport = Some(viewport);
        let seconds_per_pixel = self.video_timeline_seconds_per_pixel();
        self.timeline_thumbnails
            .render_seconds_per_pixel
            .get_or_insert(seconds_per_pixel);
        self.timeline_thumbnails.generation = self.timeline_thumbnails.generation.wrapping_add(1);
        self.timeline_thumbnails
            .cancel_generation
            .store(self.timeline_thumbnails.generation, Ordering::SeqCst);
        self.timeline_thumbnails.settled = false;
        self.timeline_thumbnails.queue.clear();
        let generation = self.timeline_thumbnails.generation;
        cx.spawn(async move |weak, cx| {
            Timer::after(Duration::from_millis(180)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.timeline_thumbnails.generation != generation {
                    return;
                }
                this.timeline_thumbnails.settled = true;
                let Some(viewport) = this.timeline_thumbnails.viewport.as_ref() else {
                    return;
                };
                this.timeline_thumbnails.render_seconds_per_pixel =
                    Some(viewport.seconds_per_pixel);
                this.timeline_thumbnails.queue = visible_keys(viewport)
                    .into_iter()
                    .filter(|key| {
                        !this.timeline_thumbnails.cache.contains_key(key)
                            && !this.timeline_thumbnails.failed.contains(key)
                    })
                    .collect();
                this.pump_timeline_thumbnails(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn pump_timeline_thumbnails(&mut self, cx: &mut Context<Self>) {
        if self.timeline_thumbnails.busy || !self.timeline_thumbnails.settled {
            return;
        }
        let Some(viewport) = self.timeline_thumbnails.viewport.clone() else {
            return;
        };
        let batch: Vec<_> = (0..8)
            .filter_map(|_| self.timeline_thumbnails.queue.pop_front())
            .collect();
        if batch.is_empty() {
            return;
        }
        self.timeline_thumbnails.busy = true;
        let generation = self.timeline_thumbnails.generation;
        let path = viewport.path.clone();
        let cancel = self.timeline_thumbnails.cancel_generation.clone();
        let task = cx.background_executor().spawn(async move {
            let mut decoder = PreviewDecoder::open(path, (220, 124));
            batch
                .into_iter()
                .take_while(|_| cancel.load(Ordering::SeqCst) == generation)
                .map(|key| {
                    let time = ((key.index as f64 + 0.5) * spacing(viewport.duration, key.level))
                        .min((viewport.duration - 0.001).max(0.0));
                    let image = decoder
                        .as_mut()
                        .ok()
                        .and_then(|decoder| {
                            decoder
                                .frame(time, &|| cancel.load(Ordering::SeqCst) != generation)
                                .ok()
                        })
                        .and_then(|frame| {
                            image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                        })
                        .map(|image| {
                            image::DynamicImage::ImageRgba8(image)
                                .resize(220, 124, image::imageops::FilterType::Triangle)
                                .to_rgba8()
                        });
                    (key, image)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |weak, cx| {
            let images = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.timeline_thumbnails.busy = false;
                if generation == this.timeline_thumbnails.generation {
                    for (key, image) in images {
                        if let Some(image) = image {
                            if let Some(old) = this
                                .timeline_thumbnails
                                .cache
                                .insert(key, cached_render_image(image))
                            {
                                this.retired_images.push(old);
                            }
                            this.timeline_thumbnails.order.push_back(key);
                        } else {
                            this.timeline_thumbnails.failed.insert(key);
                        }
                    }
                    while this.timeline_thumbnails.cache.len() > CACHE_LIMIT {
                        let Some(key) = this.timeline_thumbnails.order.pop_front() else {
                            break;
                        };
                        if let Some(image) = this.timeline_thumbnails.cache.remove(&key) {
                            this.retired_images.push(image);
                        }
                    }
                    cx.notify();
                }
                this.pump_timeline_thumbnails(cx);
            });
        })
        .detach();
    }

    pub(crate) fn timeline_thumbnail_tiles(
        &self,
        editor_start: f64,
        source_start: f64,
        source_end: f64,
        speed: f64,
        width: f32,
    ) -> Vec<(f32, f32, Arc<RenderImage>)> {
        if self.video_source_duration <= 0.0 || source_end <= source_start || width <= 0.0 {
            return Vec::new();
        }
        let seconds_per_pixel = self.video_timeline_seconds_per_pixel();
        let viewport_start = self.video_timeline_scroll * seconds_per_pixel;
        let viewport_end =
            viewport_start + self.video_timeline_viewport_width() * seconds_per_pixel;
        let source_visible_start =
            (source_start + (viewport_start - editor_start) * speed).max(source_start);
        let source_visible_end =
            (source_start + (viewport_end - editor_start) * speed).min(source_end);
        if source_visible_end <= source_visible_start {
            return Vec::new();
        }
        let target = TARGET_WIDTH
            * self
                .timeline_thumbnails
                .render_seconds_per_pixel
                .unwrap_or(seconds_per_pixel)
            * speed;
        let level = level_for(self.video_source_duration, target);
        let mut step = spacing(self.video_source_duration, level);
        while step < target {
            step *= 2.0;
        }
        let mut result = Vec::new();
        for index in (source_visible_start / step).floor() as u32
            ..=(source_visible_end / step).floor() as u32
        {
            let begin = (index as f64 * step).max(source_start);
            let end = ((index as f64 + 1.0) * step).min(source_end);
            if end <= begin {
                continue;
            }
            let center = (index as f64 + 0.5) * step;
            let mut found = None;
            for fallback in (1..=level).rev() {
                let key = Key {
                    level: fallback,
                    index: (center / spacing(self.video_source_duration, fallback)).floor() as u32,
                };
                if let Some(image) = self.timeline_thumbnails.cache.get(&key) {
                    found = Some(image.clone());
                    break;
                }
            }
            if found.is_none() && !self.video_thumbnails.is_empty() {
                let coarse_index = (center / self.video_source_duration
                    * self.video_thumbnails.len() as f64)
                    .floor() as usize;
                found = self
                    .video_thumbnails
                    .get(coarse_index.min(self.video_thumbnails.len() - 1))
                    .cloned();
            }
            if let Some(image) = found {
                let left = (begin - source_start) / (source_end - source_start) * width as f64;
                let tile_width = (end - begin) / (source_end - source_start) * width as f64;
                result.push((left as f32, tile_width as f32, image));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::clips::{RecordingClipSegment, RecordingClipTimeline};

    #[test]
    fn visible_thumbnail_requests_follow_source_time_after_cuts_and_speed() {
        let mut clip = RecordingClipSegment::new(40.0, 80.0);
        clip.speed = 2.0;
        clip.gap_before = 5.0;
        let viewport = Viewport {
            path: PathBuf::from("test"),
            duration: 120.0,
            seconds_per_pixel: 0.01,
            scroll: 1000.0,
            width: 500.0,
            clips: RecordingClipTimeline::new(vec![clip]),
        };
        let keys = visible_keys(&viewport);
        assert!(!keys.is_empty());
        assert!(keys.len() < 20);
        for key in keys {
            let source = (key.index as f64 + 0.5) * spacing(viewport.duration, key.level);
            assert!(
                (47.0..64.0).contains(&source),
                "unexpected source time {source}"
            );
        }
    }

    #[test]
    fn finer_grids_keep_parent_tile_alignment_and_bound_density() {
        let coarse = spacing(120.0, 2);
        let fine = spacing(120.0, 3);
        assert_eq!(coarse, fine * 2.0);
        assert!(spacing(120.0, level_for(120.0, 0.001)) >= 0.1);
        assert_eq!(level_for(10.0, 2.0), 0);
    }
}
