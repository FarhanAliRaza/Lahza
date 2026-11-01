//! `video` tests that also drive higher layers (playback, export, native, project, audio_processing),
//! so they live in the crate that depends on all of them.

use crate::video::*;
use crate::clips::RecordingClipTimeline;
use std::{fs, path::PathBuf, process::{Command, Stdio}};

fn shadowed_window(width: u32, height: u32, body: &image::RgbaImage) -> image::RgbaImage {
    let (left, top) = ((width - body.width()) / 2, (height - body.height()) / 2);
    let mut frame = image::RgbaImage::new(width, height);
    for (x, y, pixel) in frame.enumerate_pixels_mut() {
        let dx = left
            .saturating_sub(x)
            .max((x + 1).saturating_sub(left + body.width()));
        let dy = top
            .saturating_sub(y)
            .max((y + 1).saturating_sub(top + body.height()));
        let distance = dx.max(dy);
        if distance == 0 {
            *pixel = *body.get_pixel(x - left, y - top);
        } else {
            *pixel = image::Rgba([
                0,
                0,
                0,
                [55, 39, 28, 16, 8, 4, 2, 1]
                    .get(distance as usize - 1)
                    .copied()
                    .unwrap_or(0),
            ]);
        }
    }
    frame
}

fn test_video() -> Option<(PathBuf, PathBuf)> {
    let root = std::env::temp_dir().join(format!("lahza-video-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).ok()?;
    let path = root.join("test.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=12:duration=1",
            "-c:v",
            "ffv1",
            "-y",
        ])
        .arg(&path)
        .status()
        .ok()?;
    status.success().then_some((root, path))
}

fn test_video_with_audio() -> Option<(PathBuf, PathBuf)> {
    let root =
        std::env::temp_dir().join(format!("lahza-video-audio-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).ok()?;
    let path = root.join("test.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=12:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-shortest",
            "-y",
        ])
        .arg(&path)
        .status()
        .ok()?;
    status.success().then_some((root, path))
}

#[test]
fn window_alpha_survives_recording_posters_playback_and_edited_export() {
    use crate::{clips::RecordingClipSegment, playback::TimelinePlaybackStream};
    use gst::prelude::*;
    use gstreamer as gst;

    gst::init().unwrap();
    let root = std::env::temp_dir().join(format!("lahza-alpha-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("screen.mkv");
    let pipeline = gst::parse::launch(&format!(
        "videotestsrc num-buffers=20 pattern=ball foreground-color=4278190080 background-color=0 ! \
         video/x-raw,format=BGRA,width=64,height=48,framerate=10/1 ! identity ! {} ! \
         matroskamux ! filesink location=\"{}\"",
        crate::native::WINDOW_ENCODER, path.display()
    )).unwrap().downcast::<gst::Pipeline>().unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let message = pipeline.bus().unwrap().timed_pop_filtered(
        gst::ClockTime::from_seconds(5),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    pipeline.set_state(gst::State::Null).unwrap();
    assert!(matches!(message.unwrap().view(), gst::MessageView::Eos(_)));
    let assert_alpha = |rgba: &[u8]| {
        assert!(
            rgba.chunks_exact(4).any(|p| p[3] == 0),
            "transparent margins became opaque"
        );
        assert!(
            rgba.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255),
            "shadow alpha was lost"
        );
        assert!(
            rgba.chunks_exact(4).any(|p| p == [0, 0, 0, 255]),
            "opaque black content must remain opaque"
        );
    };
    assert_alpha(&decode_frame(&path, 0.2, 64, 48).unwrap().rgba);
    let poster = root.join("poster.png");
    write_poster(&path, &poster, 64, 48).unwrap();
    assert_alpha(image::open(&poster).unwrap().to_rgba8().as_raw());
    let timeline = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.2, 1.2)]);
    let mut playback =
        TimelinePlaybackStream::open(&path, timeline.clone(), 0.0, 64, 48, true).unwrap();
    assert_alpha(&playback.next_frame(|| false).unwrap().unwrap().rgba);
    playback.stop();
    let mut export = VideoFrameStream::open_timeline(&path, &timeline, 64, 48, 10.0).unwrap();
    assert_alpha(&export.next_frame().unwrap().unwrap().rgba);
    export.stop();
    let edited = root.join("edited.mkv");
    crate::export::render_clip_preview(&path, &edited, &timeline).unwrap();
    assert_alpha(&decode_frame(&edited, 0.2, 64, 48).unwrap().rgba);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_window_presentation_and_export_follow_each_frames_shape() {
    use crate::{
        clips::RecordingClipSegment,
        export::{export_scene, ExportFormat, ExportProgress, SceneExportRequest, SceneSource},
        playback::TimelinePlaybackStream,
        scene_model::{SceneBackground, SceneStyle},
        viewport::ViewportTimeline,
    };
    use std::io::Write;
    let root =
        std::env::temp_dir().join(format!("lahza-native-window-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("screen.mkv");
    let mut encoder = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            "64x48",
            "-framerate",
            "10",
            "-i",
            "pipe:0",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "bgra",
        ])
        .arg(&path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = encoder.stdin.take().unwrap();
    for index in 0..20 {
        let (width, height) = if index < 10 { (20, 32) } else { (48, 27) };
        let mut body = image::RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                body.put_pixel(
                    x,
                    y,
                    image::Rgba([0, if (x + y) % 2 == 0 { 255 } else { 180 }, 0, 255]),
                );
            }
        }
        let frame = shadowed_window(64, 48, &body);
        stdin.write_all(frame.as_raw()).unwrap();
    }
    drop(stdin);
    assert!(encoder.wait().unwrap().success());
    fs::write(path.with_extension("window.json"), r#"{"version":1}"#).unwrap();
    let first = decode_frame(&path, 0.2, 8, 8).unwrap();
    assert_eq!((first.width, first.height), (20, 32));
    let wide = decode_frame(&path, 1.2, 8, 8).unwrap();
    assert_eq!((wide.width, wide.height), (48, 27));
    for (i, pixel) in wide.rgba.chunks_exact(4).enumerate() {
        assert_eq!(
            pixel,
            [
                0,
                if (i % 48 + i / 48) % 2 == 0 { 255 } else { 180 },
                0,
                255
            ]
        );
    }
    let clips = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.0, 2.0)]);
    let mut playback =
        TimelinePlaybackStream::open(&path, clips.clone(), 1.2, 8, 8, true).unwrap();
    let frame = playback.next_frame(|| false).unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (48, 27));
    playback.stop();
    let reordered = RecordingClipTimeline::new(vec![
        RecordingClipSegment::new(1.0, 2.0),
        RecordingClipSegment::new(0.0, 1.0),
    ]);
    let mut stream = VideoFrameStream::open_timeline(&path, &reordered, 8, 8, 10.0).unwrap();
    assert_eq!(stream.next_frame().unwrap().unwrap().width, 48);
    for _ in 0..10 {
        stream.next_frame().unwrap().unwrap();
    }
    assert_eq!(stream.next_frame().unwrap().unwrap().width, 20);
    stream.stop();
    let destination = root.join("export.mp4");
    let style = SceneStyle {
        background: SceneBackground::Solid(0x202080),
        padding: 0,
        corners: 0,
        shadow: 0,
        border: false,
        aspect: Some(16.0 / 9.0),
        ..Default::default()
    };
    let mut request = SceneExportRequest::new(
        destination.clone(),
        ExportFormat::Mp4,
        180,
        style,
        ViewportTimeline::default(),
        2.0,
    );
    request.frame_rate = 10.0;
    export_scene(
        SceneSource::Video {
            media: path,
            clips,
            pointer: None,
            camera: None,
        },
        &mut request,
        &ExportProgress::default(),
    )
    .unwrap();
    let extent = |time| {
        let frame = decode_frame(&destination, time, 320, 180).unwrap();
        let mut left = 320;
        let mut right = 0;
        for (i, p) in frame.rgba.chunks_exact(4).enumerate() {
            if p[1] > p[0].saturating_add(50) && p[1] > p[2].saturating_add(50) {
                left = left.min(i as u32 % frame.width);
                right = right.max(i as u32 % frame.width);
            }
        }
        right - left + 1
    };
    assert!(extent(0.2) < 130);
    assert!(
        extent(1.2) > 300,
        "wide frames must fill the 16:9 canvas at export, not the original portrait box"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn odd_window_dimensions_do_not_add_opaque_playback_edges() {
    use crate::{clips::RecordingClipSegment, playback::TimelinePlaybackStream};
    let root = std::env::temp_dir().join(format!("lahza-odd-window-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    for (width, height) in [(65, 64), (64, 65)] {
        let mut source = image::RgbaImage::new(width, height);
        for y in 16..48 {
            for x in 16..48 {
                source.put_pixel(x, y, image::Rgba([20, 60, 100, 255]));
            }
        }
        let still = root.join("window.png");
        source.save(&still).unwrap();
        let path = root.join("screen.mkv");
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-loop", "1", "-i"])
            .arg(&still)
            .args(["-t", "1", "-r", "10", "-c:v", "ffv1", "-pix_fmt", "bgra"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let assert_edges = |frame: DecodedFrame, expected: (u32, u32)| {
            assert_eq!((frame.width, frame.height), expected);
            for (i, pixel) in frame.rgba.chunks_exact(4).enumerate() {
                let (x, y) = (i % frame.width as usize, i / frame.width as usize);
                if x == 0 || x + 1 == frame.width as usize || y == 0 || y + 1 == frame.height as usize {
                    assert_eq!(
                        pixel[3], 0,
                        "opaque edge at ({x}, {y}) for {width}x{height} window"
                    );
                }
            }
            let center = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
            assert_eq!(
                &frame.rgba[center..center + 4],
                &[20, 60, 100, 255]
            );
            if expected == (width, height) {
                assert_eq!(frame.rgba, *source.as_raw(),
                    "native decoding must preserve every pixel, including odd edges");
            }
        };
        // Native requests retain the odd edge instead of shrinking it.
        // Actual proxy downscales still must not introduce opaque borders.
        let proxy = if width > height { (64, 62) } else { (62, 64) };
        for (limit, expected) in [(128, (width, height)), (64, proxy)] {
            let timeline = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.0, 0.8)]);
            let mut playback =
                TimelinePlaybackStream::open(&path, timeline, 0.0, limit, limit, true).unwrap();
            assert_edges(playback.next_frame(|| false).unwrap().unwrap(), expected);
            playback.stop();
            let mut transport = SynchronizedPlaybackStream::open(&path, 0.0, limit, limit).unwrap();
            assert_edges(transport.next_frame().unwrap().unwrap(), expected);
            transport.stop();
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn placeholder_container_dimensions_recover_frames_and_cached_posters() {
    use crate::{clips::RecordingClipSegment, playback::TimelinePlaybackStream};
    let root = std::env::temp_dir().join(format!("lahza-placeholder-size-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let path = root.join("screen.mkv");
    let output = Command::new("ffmpeg")
        .args([
            "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=64x48:rate=10:duration=1",
            "-c:v", "libvpx", "-deadline", "realtime",
        ])
        .arg(&path)
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    // Reproduce an old recording: Matroska PixelWidth/PixelHeight say
    // 1x1, but the VP8 keyframes contain the complete 64x48 picture.
    let mut bytes = fs::read(&path).unwrap();
    let dimensions = [0xb0, 0x81, 64, 0xba, 0x81, 48];
    let offsets: Vec<_> = bytes.windows(dimensions.len()).enumerate()
        .filter_map(|(i, window)| (window == dimensions).then_some(i)).collect();
    assert_eq!(offsets.len(), 1);
    bytes[offsets[0] + 2] = 1;
    bytes[offsets[0] + 5] = 1;
    fs::write(&path, bytes).unwrap();

    let info = probe_media(&path).unwrap();
    assert_eq!((info.width, info.height), (64, 48));
    let bank_path = root.join("source.media.json");
    let mut cached_info = info.clone();
    cached_info.width = 1;
    cached_info.height = 1;
    crate::media_bank::MediaBank {
        entries: vec![crate::media_bank::MediaEntry {
            file: "screen.mkv".into(), info: cached_info, still: false,
        }],
    }.save(&bank_path).unwrap();
    let bank_info = probe_media(&bank_path).unwrap();
    assert_eq!((bank_info.width, bank_info.height), (64, 48));
    let frame = decode_frame(&path, 0.2, 32, 24).unwrap();
    assert_eq!((frame.width, frame.height), (32, 24));
    assert!(frame.rgba.chunks_exact(4).any(|pixel| pixel != &frame.rgba[..4]));
    let bank_frame = decode_frame(&bank_path, 0.2, 32, 24).unwrap();
    assert_eq!(bank_frame, frame);
    let timeline = RecordingClipTimeline::new(vec![RecordingClipSegment::new(0.0, 0.8)]);
    let mut playback = TimelinePlaybackStream::open(&path, timeline, 0.0, 32, 24, true).unwrap();
    let frame = playback.next_frame(|| false).unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (32, 24));
    playback.stop();

    let poster = root.join("poster.png");
    // The old writer could also upscale its broken square thumbnail.
    for (width, height) in [(2, 2), (24, 24), (32, 24)] {
        image::RgbaImage::from_pixel(width, height, image::Rgba([9, 9, 9, 255]))
            .save(&poster).unwrap();
        let recovered = load_or_rebuild_poster(&path, &poster, 32, 24).unwrap();
        assert_eq!(recovered.dimensions(), (32, 24));
        assert!(recovered.pixels().any(|pixel| pixel.0 != [9, 9, 9, 255]));
        assert_eq!(image::image_dimensions(&poster).unwrap(), (32, 24));
    }
    // A valid small source should not be enlarged just to fill the budget.
    write_poster(&path, &poster, 1280, 720).unwrap();
    assert_eq!(image::image_dimensions(&poster).unwrap(), (64, 48));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn edited_preview_applies_cuts_and_speed_to_video_and_audio() {
    let Some((root, path)) = test_video_with_audio() else {
        eprintln!("FFmpeg unavailable; skipping clip preview integration test");
        return;
    };
    let mut fast = crate::clips::RecordingClipSegment::new(2.0, 3.0);
    fast.speed = 2.0;
    let timeline = RecordingClipTimeline::new(vec![
        crate::clips::RecordingClipSegment::new(0.0, 1.0),
        fast,
    ]);
    let preview = root.join("edited.mkv");
    crate::export::render_clip_preview(&path, &preview, &timeline).unwrap();
    let info = probe_media(&preview).unwrap();
    assert!(info.has_audio);
    // Container/audio packet padding may add up to two source frames; the
    // editor continues to use the exact mathematical clip duration.
    assert!((info.duration - 1.5).abs() < 0.2, "{}", info.duration);
    let final_frame = decode_frame(&preview, 1.4, 160, 90).unwrap();
    assert!(final_frame.rgba.iter().any(|value| *value != 0));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn edited_preview_renders_reordered_clips() {
    let Some((root, path)) = test_video_with_audio() else {
        eprintln!("FFmpeg unavailable; skipping reorder preview integration test");
        return;
    };
    // Play the tail of the recording before its head.
    let timeline = RecordingClipTimeline::new(vec![
        crate::clips::RecordingClipSegment::new(2.0, 3.0),
        crate::clips::RecordingClipSegment::new(0.0, 1.5),
    ]);
    assert_eq!(timeline.normalized(3.0), timeline);
    let preview = root.join("reordered.mkv");
    crate::export::render_clip_preview(&path, &preview, &timeline).unwrap();
    let info = probe_media(&preview).unwrap();
    assert!((info.duration - 2.5).abs() < 0.2, "{}", info.duration);
    let final_frame = decode_frame(&preview, 2.3, 160, 90).unwrap();
    assert!(final_frame.rgba.iter().any(|value| *value != 0));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn edited_preview_renders_gaps_as_black_filler() {
    let Some((root, path)) = test_video_with_audio() else {
        eprintln!("FFmpeg unavailable; skipping gap preview integration test");
        return;
    };
    let mut spaced = crate::clips::RecordingClipSegment::new(2.0, 3.0);
    spaced.gap_before = 1.0;
    let timeline = RecordingClipTimeline::new(vec![
        crate::clips::RecordingClipSegment::new(0.0, 1.0),
        spaced,
    ]);
    assert!((timeline.duration() - 3.0).abs() < 1e-9);
    let preview = root.join("gapped.mkv");
    crate::export::render_clip_preview(&path, &preview, &timeline).unwrap();
    let info = probe_media(&preview).unwrap();
    assert!(info.has_audio);
    assert!((info.duration - 3.0).abs() < 0.2, "{}", info.duration);
    // The gap between one and two seconds decodes as pure black.
    let gap_frame = decode_frame(&preview, 1.5, 160, 90).unwrap();
    let max_luma = gap_frame
        .rgba
        .chunks(4)
        .flat_map(|pixel| pixel[..3].iter())
        .copied()
        .max()
        .unwrap_or(0);
    assert!(max_luma < 32, "gap frame not black (max {max_luma})");
    // Content after the gap still decodes.
    let content_frame = decode_frame(&preview, 2.5, 160, 90).unwrap();
    assert!(content_frame.rgba.iter().any(|value| *value > 64));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn audio_levels_and_thumbnails_describe_the_media() {
    let Some((root, path)) = test_video_with_audio() else {
        eprintln!("FFmpeg unavailable; skipping audio level test");
        return;
    };
    let levels = crate::audio_processing::audio_levels(&path, 12, crate::audio_mode::AudioMode::Off).unwrap();
    assert_eq!(levels.len(), 12);
    assert!(levels.iter().any(|level| *level > 0.5));
    assert!(levels.iter().all(|level| (0.0..=1.0).contains(level)));
    let thumbnails = decode_thumbnails(&path, 4, 36).unwrap();
    assert!(!thumbnails.is_empty() && thumbnails.len() <= 4);
    assert_eq!(thumbnails[0].height(), 36);
    assert!(thumbnails[0].pixels().any(|pixel| pixel[0] > 0));
    let silent = test_video().unwrap();
    assert!(crate::audio_processing::audio_levels(&silent.1, 8, crate::audio_mode::AudioMode::Off).unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(silent.0).unwrap();
}

#[test]
#[ignore = "plays frames from the newest persistent recording"]
fn live_gstreamer_transport_decodes_recording_package() {
    let session = crate::project::recoverable_sessions()
        .unwrap()
        .into_iter()
        .next()
        .expect("a persistent recording package");
    let mut stream = SynchronizedPlaybackStream::open(&session.screen_path(), 0.25, 640, 360)
        .expect("open synchronized playback");
    let first = stream.next_frame().unwrap().expect("first frame");
    let second = stream.next_frame().unwrap().expect("second frame");
    assert_eq!(first.rgba.len(), (first.width * first.height * 4) as usize);
    assert!(first.rgba.iter().any(|value| *value != 0));
    assert!(second.time > first.time);
    stream.stop();
    println!("playback_session={}", session.directory.display());
}
