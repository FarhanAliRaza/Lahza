//! A virtual source: stable source-time ranges backed by separate project files.
//! Import only copies the new asset. Decoding and fitting happen when needed.
use super::{
    clips::RecordingClipTimeline,
    video::{self, DecodedFrame, MediaInfo, VideoError},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaEntry {
    pub file: PathBuf,
    pub info: MediaInfo,
    pub still: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaBank {
    pub entries: Vec<MediaEntry>,
}

impl MediaBank {
    pub fn load(path: &Path) -> Result<Option<Self>, VideoError> {
        if !path.to_string_lossy().ends_with(".media.json") {
            return Ok(None);
        }
        let bank: Self = serde_json::from_slice(&fs::read(path)?)
            .map_err(|e| VideoError::InvalidMedia(format!("invalid media index: {e}")))?;
        if bank.entries.is_empty()
            || bank.entries.iter().any(|entry| {
                entry.file.as_os_str().is_empty()
                    || entry
                        .file
                        .components()
                        .any(|c| !matches!(c, Component::Normal(_)))
                    || !entry.info.duration.is_finite()
                    || entry.info.duration <= 0.0
                    || entry.info.width == 0
                    || entry.info.height == 0
                    || !entry.info.frame_rate.is_finite()
                    || entry.info.frame_rate <= 0.0
            })
        {
            return Err(VideoError::InvalidMedia(
                "invalid media index entries".into(),
            ));
        }
        Ok(Some(bank))
    }

    pub fn save(&self, path: &Path) -> Result<(), VideoError> {
        let bytes =
            serde_json::to_vec(self).map_err(|e| VideoError::InvalidMedia(e.to_string()))?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    pub fn info(&self) -> MediaInfo {
        let mut info = self.entries[0].info.clone();
        info.duration = self.entries.iter().map(|e| e.info.duration).sum();
        info.has_audio = self.entries.iter().any(|e| e.info.has_audio);
        info.frame_rate = self
            .entries
            .iter()
            .map(|e| e.info.frame_rate)
            .fold(30.0, f64::max);
        info
    }

    pub fn locate(&self, time: f64) -> (usize, f64) {
        let mut start = 0.0;
        for (i, entry) in self.entries.iter().enumerate() {
            if time < start + entry.info.duration || i + 1 == self.entries.len() {
                return (i, start);
            }
            start += entry.info.duration;
        }
        unreachable!("validated nonempty bank")
    }

    pub fn entry_path(&self, manifest: &Path, index: usize) -> PathBuf {
        manifest.parent().unwrap().join(&self.entries[index].file)
    }

    pub fn decode_frame(
        &self,
        manifest: &Path,
        time: f64,
        max_w: u32,
        max_h: u32,
    ) -> Result<DecodedFrame, VideoError> {
        let info = self.info();
        let (width, height) = video::decode_dimensions(&info, max_w, max_h);
        let (index, start) = self.locate(time.max(0.0));
        let entry = &self.entries[index];
        let path = self.entry_path(manifest, index);
        // Decode the original recording through its own presentation path so
        // transparent window margins retain exactly the same crop.
        if index == 0
            && entry.info.width == info.width
            && entry.info.height == info.height
            && !entry.still
        {
            let mut frame = video::decode_frame(&path, time - start, max_w, max_h)?;
            frame.time = time;
            return Ok(frame);
        }
        let mut command = Command::new("ffmpeg");
        command.args(["-v", "error"]);
        if !entry.still {
            command
                .arg("-ss")
                .arg(format!("{:.9}", (time - start).max(0.0)));
        }
        let output = command.arg("-i").arg(&path).args(["-frames:v", "1", "-an", "-vf"])
            .arg(format!("scale={width}:{height}:force_original_aspect_ratio=decrease,format=rgba,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1"))
            .args(["-f", "rawvideo", "pipe:1"]).output()?;
        if !output.status.success() || output.stdout.len() != width as usize * height as usize * 4 {
            return Err(VideoError::Decode(
                String::from_utf8_lossy(&output.stderr).into(),
            ));
        }
        Ok(video::present_frame(
            DecodedFrame {
                time,
                width,
                height,
                rgba: output.stdout,
            },
            info.window_capture,
        ))
    }

    /// Split virtual ranges at asset boundaries; offsets remain in source time.
    pub fn ranges(&self, timeline: &RecordingClipTimeline) -> RecordingClipTimeline {
        let mut ranges = Vec::new();
        for clip in timeline.playback_ranges() {
            let mut start = 0.0;
            let mut first = true;
            for entry in &self.entries {
                let end = start + entry.info.duration;
                let lo = clip.source_start.max(start);
                let hi = clip.source_end.min(end);
                if hi > lo + 1e-9 {
                    let mut part = clip.clone();
                    part.source_start = lo;
                    part.source_end = hi;
                    part.source_bounds = Some([start, end]);
                    if !first {
                        part.gap_before = 0.0;
                    }
                    first = false;
                    ranges.push(part);
                }
                start = end;
            }
        }
        RecordingClipTimeline::new(ranges)
    }
}

#[derive(Clone)]
pub struct MediaInput {
    pub path: PathBuf,
    pub still_duration: Option<f64>,
    pub seek: f64,
    pub duration: Option<f64>,
}

pub fn add_inputs(command: &mut Command, inputs: &[MediaInput]) {
    for input in inputs {
        if let Some(duration) = input.still_duration {
            command
                .args(["-loop", "1", "-framerate", "30", "-t"])
                .arg(format!("{duration:.9}"));
        }
        if input.seek > 0.0 {
            command.arg("-ss").arg(format!("{:.9}", input.seek));
        }
        if let Some(duration) = input.duration {
            command.arg("-t").arg(format!("{duration:.9}"));
        }
        command.arg("-i").arg(&input.path);
    }
}

/// Build one graph for preview/export, including heterogeneous video sizes and
/// silence for images or videos without audio. Input indices include the caller's offset.
pub fn source_filter(
    path: &Path,
    timeline: &RecordingClipTimeline,
    input: usize,
    video: bool,
    audio: bool,
) -> Result<(Vec<MediaInput>, String), VideoError> {
    let Some(bank) = MediaBank::load(path)? else {
        let info = video::probe_media(path)?;
        return Ok((
            vec![MediaInput {
                path: path.into(),
                still_duration: None,
                seek: 0.0,
                duration: None,
            }],
            video::clip_filter(&info, timeline, input, video, audio),
        ));
    };
    let info = bank.info();
    let mut inputs = Vec::new();
    let mut filter = String::new();
    let mut labels = Vec::new();
    let mut n = 0;
    for clip in bank.ranges(timeline).segments {
        if clip.gap_before > 0.0 {
            if video {
                filter.push_str(&format!(
                    "color=c=black:s={}x{}:r=30:d={:.9},format=rgba,setsar=1[v{n}];",
                    info.width, info.height, clip.gap_before
                ));
            }
            if audio {
                filter.push_str(&format!(
                    "anullsrc=r=48000:cl=stereo,atrim=duration={:.9}[a{n}];",
                    clip.gap_before
                ));
            }
            labels.push(n);
            n += 1;
        }
        let (index, offset) = bank.locate(clip.source_start);
        let entry = &bank.entries[index];
        let duration = clip.editor_duration();
        // Open each retained range independently with an input seek: decoding
        // a late trim must not scan all earlier footage, nor buffer every split.
        let source_input = input + inputs.len();
        if video || (audio && entry.info.has_audio) {
            inputs.push(MediaInput {
                path: bank.entry_path(path, index),
                still_duration: entry.still.then_some(clip.duration()),
                seek: if entry.still {
                    0.0
                } else {
                    clip.source_start - offset
                },
                duration: Some(clip.duration()),
            });
        }
        if video {
            filter.push_str(&format!("[{source_input}:v]trim=start={:.9}:end={:.9},setpts=(PTS-STARTPTS)/{:.9},scale={}:{}:force_original_aspect_ratio=decrease,format=rgba,pad={}:{}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={:.6},tpad=stop_mode=clone:stop_duration={:.9},trim=duration={duration:.9}[v{n}];", 0.0, clip.duration(), clip.speed, info.width, info.height, info.width, info.height, info.frame_rate, 1.0/info.frame_rate));
        }
        if audio {
            if entry.info.has_audio {
                filter.push_str(&format!("[{source_input}:a]atrim=start={:.9}:end={:.9},asetpts=PTS-STARTPTS,atempo={:.9},aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,apad,atrim=duration={duration:.9}[a{n}];", 0.0, clip.duration(), clip.speed));
            } else {
                filter.push_str(&format!(
                    "anullsrc=r=48000:cl=stereo,atrim=duration={duration:.9}[a{n}];"
                ));
            }
        }
        labels.push(n);
        n += 1;
    }
    for n in &labels {
        if video {
            filter.push_str(&format!("[v{n}]"));
        }
        if audio {
            filter.push_str(&format!("[a{n}]"));
        }
    }
    filter.push_str(&format!(
        "concat=n={}:v={}:a={}{}{}",
        labels.len(),
        u8::from(video),
        u8::from(audio),
        if video { "[video]" } else { "" },
        if audio { "[audio]" } else { "" }
    ));
    Ok((inputs, filter))
}
