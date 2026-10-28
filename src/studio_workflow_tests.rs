//! Editor action, persistence, compositor and encoded-output regression coverage.
use super::*;
use crate::recording::{
    camera_layout::{CameraAspect, CameraLayout, CameraSplitDirection},
    export::{export_scene, ExportFormat, ExportProgress, SceneExportRequest, SceneSource},
    model::{
        CaptureManifest, NormalizedPoint, PointerCaptureFile, PointerPressEvent, PointerTravelKind,
        PointerTravelSample, PressPhase,
    },
    pointer_timeline::{PointerMotion, PointerRotation},
    scene::{CameraShape, FrameInput, PointerOverlay, SceneCompositor, SceneStyle},
    video::{decode_frame, probe_media},
    viewport::{CameraMotion, ZoomAnchorMode, ZoomPresentation},
};
use crate::text_field::{EventKind, FieldEvent, Target};
use std::path::Path;

fn artifact_directory() -> PathBuf {
    let path = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("screenstudio-analysis/workflow"));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn fixture(root: &Path) -> RecordingSession {
    let directory = root.join(format!("fixture-{}.lahzarec", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let session = RecordingSession { directory };
    for (path, source) in [
        (
            session.screen_path(),
            "testsrc2=size=320x180:rate=10:duration=8",
        ),
        (
            session.camera_path(),
            "testsrc2=size=160x120:rate=10:duration=8",
        ),
    ] {
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", source, "-c:v", "ffv1", "-y"])
            .arg(path)
            .status()
            .unwrap()
            .success());
    }
    session
        .write_manifest(&CaptureManifest {
            duration: 8.0,
            pixel_width: 320,
            pixel_height: 180,
            includes_camera: true,
            pointer_synthesized: true,
            press_effects_baked: false,
            ..Default::default()
        })
        .unwrap();
    let mut capture = PointerCaptureFile::default();
    capture.travel = (0..=160)
        .map(|i| PointerTravelSample {
            time: i as f64 * 0.05,
            x: (40.0 + i as f64) / 320.0,
            y: (80.0 + 15.0 * (i as f64 * 0.1).sin()) / 180.0,
            kind: PointerTravelKind::Move,
            artwork_id: None,
        })
        .collect();
    capture.presses = vec![
        PointerPressEvent {
            time: 2.0,
            x: 80.0 / 320.0,
            y: 80.0 / 180.0,
            button: 0,
            phase: PressPhase::Down,
            artwork_id: None,
        },
        PointerPressEvent {
            time: 2.1,
            x: 80.0 / 320.0,
            y: 80.0 / 180.0,
            button: 0,
            phase: PressPhase::Up,
            artwork_id: None,
        },
    ];
    session.write_pointer_capture(&capture).unwrap();
    session
}

fn own_x11_window(
    connection: &x11rb::rust_connection::RustConnection,
    root: u32,
    compact: bool,
) -> (u32, u16, u16, u32) {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let pid_atom = connection
        .intern_atom(false, b"_NET_WM_PID")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let mut pending = vec![root];
    let mut target = None;
    while let Some(parent) = pending.pop() {
        let children = connection
            .query_tree(parent)
            .unwrap()
            .reply()
            .unwrap()
            .children;
        for child in children {
            pending.push(child);
            let geometry = connection.get_geometry(child).unwrap().reply().unwrap();
            let attrs = connection
                .get_window_attributes(child)
                .unwrap()
                .reply()
                .unwrap();
            let pid = connection
                .get_property(false, child, pid_atom, AtomEnum::CARDINAL, 0, 1)
                .unwrap()
                .reply()
                .unwrap()
                .value32()
                .and_then(|mut values| values.next());
            if attrs.map_state == x11rb::protocol::xproto::MapState::VIEWABLE
                && geometry.width > 400
                && geometry.height > 300
                && pid == Some(std::process::id())
                && (geometry.width < 600) == compact
            {
                target = Some((child, geometry.width, geometry.height, attrs.visual));
            }
        }
    }
    target.expect("mapped Lahza test window")
}

fn save_display(path: &Path, compact: bool) {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder},
    };
    let (connection, index) = x11rb::connect(None).unwrap();
    let setup = connection.setup();
    let screen = &setup.roots[index];
    // Vulkan presentation can make the root drawable read black in Xvfb.
    // Read this process's window and its visual, never another application's.
    let (drawable, width, height, visual_id) = own_x11_window(&connection, screen.root, compact);
    if compact {
        assert_eq!((width, height), (430, 610));
    }
    let reply = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            drawable,
            0,
            0,
            width,
            height,
            u32::MAX,
        )
        .unwrap()
        .reply()
        .unwrap();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|v| v.depth == reply.depth)
        .unwrap();
    assert_eq!(
        format.bits_per_pixel, 32,
        "screenshot fixture expects a 24/32-bit X11 server"
    );
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == visual_id)
        .unwrap();
    let channel = |pixel: u32, mask: u32| ((pixel & mask) >> mask.trailing_zeros()) as u8;
    let rgba = image::RgbaImage::from_fn(width.into(), height.into(), |x, y| {
        let offset = (y as usize * usize::from(width) + x as usize) * 4;
        let bytes: [u8; 4] = reply.data[offset..offset + 4].try_into().unwrap();
        let pixel = if setup.image_byte_order == ImageOrder::LSB_FIRST {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        };
        image::Rgba([
            channel(pixel, visual.red_mask),
            channel(pixel, visual.green_mask),
            channel(pixel, visual.blue_mask),
            255,
        ])
    });
    rgba.save(path).unwrap();
    assert!(
        rgba.pixels()
            .step_by(97)
            .map(|p| p.0)
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 32,
        "display screenshot must contain rendered UI"
    );
}

async fn uhd_wait_for_settled_preview(
    handle: WindowHandle<Studio>,
    time: f64,
    glass: bool,
    cx: &mut gpui::AsyncApp,
) -> (Duration, Vec<Duration>) {
    let started = std::time::Instant::now();
    let mut wakes = Vec::new();
    loop {
        let scheduled = std::time::Instant::now();
        Timer::after(Duration::from_millis(50)).await;
        wakes.push(scheduled.elapsed());
        let ready = handle
            .update(cx, |studio, _, _| {
                assert!(!studio.video_playing);
                let generation = studio
                    .video_playback_generation
                    .load(std::sync::atomic::Ordering::SeqCst);
                studio.preview_cache.frame.as_ref().is_some_and(|(key, _)| {
                    key.high_quality
                        && key.generation == generation
                        && key.time().to_bits() == time.to_bits()
                        && key.source_size == (3840, 2160)
                        && key.viewport.loupe.is_some() == glass
                        && (glass || key.viewport.magnification > 1.1)
                        && studio.video_frame_rgba.as_ref().is_some_and(|frame| {
                            frame.dimensions() == (3840, 2160)
                                && key.source == Arc::as_ptr(frame) as usize
                        })
                })
            })
            .unwrap();
        if ready {
            return (started.elapsed(), wakes);
        }
        if started.elapsed() >= Duration::from_secs(20) {
            let status = handle.update(cx, |studio, window, _| {
                format!("active={}, position={}, source={:?}, generation={}, presented={:?}, expected_viewport={:?}, toast_present={}",
                    window.is_window_active(), studio.video_position,
                    studio.video_frame_rgba.as_ref().map(|frame| frame.dimensions()),
                    studio.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst),
                    studio.preview_cache.frame.as_ref().map(|(key, _)| (
                        key.time(), key.source_size, key.canvas, key.generation,
                        key.high_quality, key.viewport.magnification, key.viewport.loupe.is_some(),
                        studio.video_frame_rgba.as_ref().is_some_and(|source| key.source == Arc::as_ptr(source) as usize),
                    )),
                    studio.video_viewport_timeline.frame_at(time), studio.toast.is_some())
            }).unwrap();
            panic!("native settled preview at {time}s (glass={glass}) did not complete: {status}");
        }
    }
}

#[test]
#[ignore = "requires an isolated X11 process, FFmpeg and graphics driver; synthetic UHD media only"]
fn studio_uhd_zoom_playback_and_settled_preview_remain_responsive() {
    let artifacts = artifact_directory().join(format!("uhd-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&artifacts).unwrap();
    // This ignored workflow runs in its own process. Keep settings, recent
    // projects and library discovery away from the user's configuration.
    let environment = [
        ("LAHZA_CONFIG_DIR", artifacts.join("settings")),
        ("LAHZA_RECORDINGS_DIR", artifacts.join("recordings")),
        ("XDG_CONFIG_HOME", artifacts.join("xdg-config")),
    ];
    let previous: Vec<_> = environment
        .iter()
        .map(|(name, _)| (*name, std::env::var_os(name)))
        .collect();
    for (name, directory) in &environment {
        std::fs::create_dir_all(directory).unwrap();
        std::env::set_var(name, directory);
    }
    std::fs::write(
        artifacts.join("xdg-config/user-dirs.dirs"),
        format!(
            "XDG_PICTURES_DIR=\"{}\"\n",
            artifacts.join("pictures").display()
        ),
    )
    .unwrap();

    let session = RecordingSession {
        directory: artifacts.join("native-uhd.lahzarec"),
    };
    std::fs::create_dir_all(&session.directory).unwrap();
    let still = artifacts.join("fine-detail.png");
    image::RgbaImage::from_fn(3840, 2160, |x, y| {
        let stripe = (x / 3 + y / 24) % 2 == 0;
        if (900..2940).contains(&x) && (500..1660).contains(&y) {
            image::Rgba(if stripe {
                [238, 242, 250, 255]
            } else {
                [24, 39, 65, 255]
            })
        } else {
            image::Rgba([32 + (x % 71) as u8, 80 + (y % 81) as u8, 180, 255])
        }
    })
    .save(&still)
    .unwrap();
    let encoded = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-loop", "1", "-framerate", "30", "-i"])
        .arg(&still)
        // RGB FFV1 in a recording bundle marks a native window recording.
        // YUV444 exercises the ordinary screen path and its former decode cap.
        .args([
            "-frames:v",
            "90",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-pix_fmt",
            "yuv444p",
            "-threads",
            "2",
            "-y",
        ])
        .arg(session.screen_path())
        .output()
        .unwrap();
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    assert!(!probe_media(&session.screen_path()).unwrap().window_capture);
    session
        .write_manifest(&CaptureManifest {
            duration: 3.0,
            pixel_width: 3840,
            pixel_height: 2160,
            includes_camera: false,
            pointer_synthesized: false,
            ..Default::default()
        })
        .unwrap();
    session
        .write_pointer_capture(&PointerCaptureFile::default())
        .unwrap();
    let mut classic =
        recording::viewport::ZoomCue::pinned(0.0, 1.35, 2.2, NormalizedPoint { x: 0.5, y: 0.5 });
    classic.skips_easing = true;
    // Start outside the preceding instant cue's 50ms snap window so this
    // fixture exercises a genuinely animated, multi-sample glass entrance.
    let mut glass =
        recording::viewport::ZoomCue::pinned(1.5, 3.0, 2.5, NormalizedPoint { x: 0.5, y: 0.5 });
    glass.presentation = ZoomPresentation::GlassLoupe;
    glass.camera_motion = CameraMotion::Smooth;
    let cues=vec![classic, glass];
    let style=SceneStyle {
        animation: recording::animation::AnimationSettings { motion_blur: 70, ..Default::default() },
        ..Default::default()
    };
    let viewport=recording::viewport::ViewportTimeline::build_static(&cues,3.0);
    assert!((151..180).any(|step| recording::motion_blur::plan(&style,&viewport,None,None,
        step as f64/100.0,3.0,30.0,(1600,900),(3840,2160),1.0,
        recording::motion_blur::Quality::Playing).samples.len()>1),
        "the UHD fixture must contain a real multi-sample entrance");
    session.write_edit_field("zoomCues", &cues).unwrap();
    session.write_edit_field("scene", &style).unwrap();
    let project = session.directory.clone();
    let screenshots = artifacts.clone();
    Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
        let handle = open_studio_window(cx, true, |handle, cx| {
            cx.new(|cx| Studio::new(handle, None, None, cx))
        }).unwrap();
        handle.update(cx, |studio, window, cx| {
            studio.launcher_window = false;
            studio.load_generation = studio.load_generation.wrapping_add(1);
            studio.loading = None;
            studio.open_video_project(project.clone()).unwrap();
            studio.launcher_active = false;
            studio.record_camera = false;
            studio.record_microphone = false;
            studio.record_system_audio = false;
            studio.video_audio_muted = true;
            studio.select_inspector_tab(shell_ui::InspectorTab::Motion);
            window.activate_window();
            window.refresh();
            cx.notify();
        }).unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            // Normal opening must replace its poster without any seek action.
            let (normal_open_hq, _) = uhd_wait_for_settled_preview(handle, 0.0, false, cx).await;
            handle.update(cx, |studio, _, cx| {
                studio.update_timeline_skim(Some(0.4), cx);
                cx.notify();
            }).unwrap();
            let proxy_started = std::time::Instant::now();
            loop {
                Timer::after(Duration::from_millis(50)).await;
                let proxy_ready = handle.update(cx, |studio, _, _| {
                    studio.preview_cache.frame.as_ref().is_some_and(|(key, _)| {
                        key.source_size == (1280, 720) && !key.high_quality
                            && studio.video_frame_rgba.as_ref().is_some_and(|frame| frame.dimensions() == (1280, 720))
                    })
                }).unwrap();
                if proxy_ready { break; }
                assert!(proxy_started.elapsed() < Duration::from_secs(8), "interactive proxy did not display");
            }
            // Pause in the same UI update that starts playback: the decoder
            // cannot publish a native frame in between these two actions.
            handle.update(cx, |studio, _, cx| {
                studio.start_video_playback(cx);
                studio.pause_video_playback();
                cx.notify();
            }).unwrap();
            let (quick_pause_hq, _) = uhd_wait_for_settled_preview(handle, 0.0, false, cx).await;
            // Reopen and seek before the first render can start thumbnails.
            // That setup must not invalidate this native-frame request.
            handle.update(cx, |studio, window, cx| {
                studio.open_video_project(project.clone()).unwrap();
                studio.launcher_active = false;
                studio.video_audio_muted = true;
                studio.select_inspector_tab(shell_ui::InspectorTab::Motion);
                studio.seek_video(0.5, cx);
                window.refresh();
                cx.notify();
            }).unwrap();
            let (initial_hq, _) = uhd_wait_for_settled_preview(handle, 0.5, false, cx).await;
            Timer::after(Duration::from_millis(100)).await;
            save_display(&screenshots.join("classic-native-paused.png"), false);
            handle.update(cx, |studio, _, cx| {
                studio.seek_video(0.0, cx);
                studio.start_video_playback(cx);
                cx.notify();
            }).unwrap();
            let warmup = std::time::Instant::now();
            loop {
                Timer::after(Duration::from_millis(25)).await;
                let ready = handle.update(cx, |studio, _, _| {
                    let generation = studio.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst);
                    studio.preview_cache.frame.as_ref().is_some_and(|(key, _)| {
                        key.generation == generation && !key.high_quality && key.time() > 0.05
                            && key.source_size == (3840, 2160)
                    })
                }).unwrap();
                if ready { break; }
                assert!(warmup.elapsed() < Duration::from_secs(8), "UHD playback preview did not start");
            }
            let warmup_elapsed = warmup.elapsed();
            let sampled = std::time::Instant::now();
            let mut wakes = Vec::new();
            let mut presented = Vec::new();
            let mut max_lag = 0.0_f64;
            let mut saw_classic = false;
            let mut saw_glass = false;
            let mut saw_temporal = false;
            while sampled.elapsed() < Duration::from_millis(2800) {
                let scheduled = std::time::Instant::now();
                Timer::after(Duration::from_millis(50)).await;
                wakes.push(scheduled.elapsed());
                handle.update(cx, |studio, _, _| {
                    assert_eq!(studio.video_frame_rgba.as_ref().unwrap().dimensions(), (3840, 2160));
                    let (key, _) = studio.preview_cache.frame.as_ref().unwrap();
                    assert_eq!(key.source_size, (3840, 2160));
                    assert!(u64::from(key.canvas.0) * u64::from(key.canvas.1) <= 1600 * 900,
                        "moving preview composition must remain bounded");
                    presented.push(key.time());
                    max_lag = max_lag.max(studio.video_position - key.time());
                    saw_glass |= key.viewport.loupe.is_some();
                    saw_classic |= key.viewport.loupe.is_none() && key.viewport.magnification > 1.1;
                    saw_temporal |= key.motion.as_ref().is_some_and(|plan| plan.samples.len() > 1);
                }).unwrap();
            }
            let mut distinct = presented.clone();
            distinct.dedup_by(|a, b| a.to_bits() == b.to_bits());
            wakes.sort();
            let p95 = wakes[(wakes.len() - 1) * 95 / 100];
            let maximum = *wakes.last().unwrap();
            eprintln!("UHD playback diagnostic warmup={warmup_elapsed:?}, times={presented:?}, classic={saw_classic}, glass={saw_glass}, temporal={saw_temporal}, lag={max_lag:.3}s, wake p95={p95:?}, max={maximum:?}");
            assert!(distinct.len() >= 3 && presented.last().unwrap() - presented[0] > 0.5,
                "UHD preview must present advancing frames");
            assert!(saw_classic && saw_glass, "both zoom styles must reach the displayed preview");
            assert!(saw_temporal, "UHD playback must present a native multi-sample shutter");
            assert!(maximum < Duration::from_secs(1) && p95 < Duration::from_millis(500),
                "UHD playback blocked UI timers: p95={p95:?}, maximum={maximum:?}");
            assert!(max_lag < 0.5, "bounded preview accumulated {max_lag:.3}s of latency");
            handle.update(cx, |studio, _, cx| {
                studio.pause_video_playback();
                studio.seek_video(1.8, cx);
                cx.notify();
            }).unwrap();
            let (paused_hq, paused_wakes) = uhd_wait_for_settled_preview(handle, 1.8, true, cx).await;
            let paused_max = *paused_wakes.iter().max().unwrap();
            assert!(paused_max < Duration::from_secs(1), "settled enhancement blocked UI timers");
            Timer::after(Duration::from_millis(100)).await;
            save_display(&screenshots.join("glass-native-paused.png"), false);
            eprintln!("UHD UI: {} advancing frames/{} samples, normal open HQ={normal_open_hq:?}, quick-pause HQ={quick_pause_hq:?}, immediate seek HQ={initial_hq:?}, playback warmup={warmup_elapsed:?}, 50ms wake p95={p95:?}, max={maximum:?}, max lag={max_lag:.3}s, paused HQ={paused_hq:?}, paused max wake={paused_max:?}", distinct.len(), presented.len());
            std::fs::write(screenshots.join("metrics.json"), serde_json::to_vec_pretty(&serde_json::json!({
                "source": [3840, 2160], "motion_blur_percent":70, "saw_temporal":saw_temporal,
                "presented_frames": distinct.len(), "samples": presented.len(),
                "normal_open_hq_ms": normal_open_hq.as_secs_f64() * 1000.0,
                "quick_pause_hq_ms": quick_pause_hq.as_secs_f64() * 1000.0,
                "initial_hq_ms": initial_hq.as_secs_f64() * 1000.0,
                "playback_warmup_ms": warmup_elapsed.as_secs_f64() * 1000.0,
                "timer_p95_ms": p95.as_secs_f64() * 1000.0, "timer_max_ms": maximum.as_secs_f64() * 1000.0,
                "maximum_preview_lag_seconds": max_lag,
                "paused_hq_ms": paused_hq.as_secs_f64() * 1000.0,
                "paused_timer_max_ms": paused_max.as_secs_f64() * 1000.0,
            })).unwrap()).unwrap();
            cx.update(|cx| cx.quit()).unwrap();
        }).detach();
    });
    for (name, value) in previous {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
    eprintln!("UHD UI artifacts: {}", artifacts.display());
}

#[test]
#[ignore = "requires an X11 display, FFmpeg and graphics driver; Xvfb supported"]
fn studio_camera_cursor_glass_save_reopen_and_export() {
    let artifacts = artifact_directory();
    let session = fixture(&artifacts);
    let project = session.directory.clone();
    let screenshots = artifacts.clone();
    Application::new()
        .with_assets(Assets {
            base: asset_directory(),
        })
        .run(move |cx| {
            let handle = open_studio_window(cx, true, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            })
            .unwrap();
            handle
                .update(cx, |studio, _, cx| {
                    studio.launcher_window = false;
                    studio.load_generation = studio.load_generation.wrapping_add(1);
                    studio.loading = None;
                    studio.open_video_project(project.clone()).unwrap();
                    studio.launcher_active = false;
                    studio.video_position = 2.0;
                    studio.select_inspector_tab(shell_ui::InspectorTab::Design);
                    studio.aspect_ratio = 10;
                    // The actual TextField subscription handles these commit events.
                    studio.text_fields.aspect_width.update(cx, |_, cx| {
                        cx.emit(FieldEvent {
                            target: Target::CanvasAspect(true),
                            kind: EventKind::Commit,
                            text: "1400".into(),
                        })
                    });
                })
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                Timer::after(Duration::from_millis(200)).await;
                handle
                    .update(cx, |studio, _, cx| {
                        assert_eq!(studio.custom_canvas_dimensions, (16, 9));
                        assert_eq!(studio.custom_canvas_draft, (1400, 9));
                        studio.text_fields.aspect_height.update(cx, |_, cx| {
                            cx.emit(FieldEvent {
                                target: Target::CanvasAspect(false),
                                kind: EventKind::Commit,
                                text: "1000".into(),
                            })
                        });
                    })
                    .unwrap();
                Timer::after(Duration::from_millis(100)).await;
                handle
                    .update(cx, |studio, window, cx| {
                        studio.video_zoom_cues.clear();
                        studio.video_undo_stack.clear();
                        studio.video_redo_stack.clear();
                        studio.select_inspector_tab(shell_ui::InspectorTab::Motion);
                        studio.inspector_visible = true;
                        studio.rebuild_video_motion_timelines();
                        studio.seek_video(2.0, cx);
                        window.activate_window();
                        window.refresh();
                        cx.notify();
                    })
                    .unwrap();
                Timer::after(Duration::from_millis(300)).await;
                let editor: AnyWindowHandle = handle.into();
                editor
                    .update(cx, |_, window, cx| window.draw(cx).clear())
                    .unwrap();
                Timer::after(Duration::from_millis(100)).await;
                save_display(&screenshots.join("motion-overview-add-glass.png"), false);
                // Hit the visible `motion-add-glass-at-playhead` button in
                // the 1440×900 editor. This exercises its actual UI listener.
                workflow_click(editor, 1280.0, 235.0, false, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert_eq!(
                            studio.video_zoom_cues.len(),
                            1,
                            "Glass loupe action must create a region"
                        );
                        assert_eq!(
                            studio.video_undo_stack.len(),
                            1,
                            "creation and presentation must be one undo operation"
                        );
                        let cue = &studio.video_zoom_cues[0];
                        assert_eq!(cue.presentation, ZoomPresentation::GlassLoupe);
                        assert_eq!(studio.video_selected_zoom_cue, Some(cue.id));
                        assert!(cue.start <= 2.0 && cue.end > 2.0);
                    })
                    .unwrap();
                Timer::after(Duration::from_millis(150)).await;
                save_display(
                    &screenshots.join("motion-created-glass-inspector.png"),
                    false,
                );
                handle
                    .update(cx, |studio, _, cx| {
                        let created = studio.video_zoom_cues[0].clone();
                        studio.undo_video_edit(cx);
                        assert!(studio.video_zoom_cues.is_empty());
                        assert!(studio.video_undo_stack.is_empty());
                        assert_eq!(studio.video_redo_stack.len(), 1);
                        studio.redo_video_edit(cx);
                        assert_eq!(studio.video_zoom_cues, vec![created]);
                        assert_eq!(studio.video_undo_stack.len(), 1);
                        assert!(studio.video_redo_stack.is_empty());
                    })
                    .unwrap();
                exercise_motion_controls(handle, editor, &screenshots, cx).await;
                exercise_project_animation_controls(handle, editor, &screenshots, cx).await;
                handle
                    .update(cx, |studio, _, cx| {
                        assert_eq!(studio.custom_canvas_dimensions, (1400, 1000));
                        assert!((studio.scene_style().aspect.unwrap() - 1.4).abs() < 1e-6);
                        studio.camera_overlay.shape = CameraShape::Squircle;
                        studio.camera_overlay.aspect = CameraAspect::Source;
                        studio.camera_overlay.zoomed_size = Some(18);
                        studio.camera_overlay.crop_center = (0.64, 0.43);
                        studio.camera_overlay.crop_zoom = 150;
                        studio.camera_overlay.mirror = true;
                        studio.camera_overlay.split_direction = CameraSplitDirection::Vertical;
                        studio.camera_overlay.split_flip = true;
                        studio.set_scene_slider(scene_ui::SceneSlider::CameraSize.id(), 60);
                        studio.set_scene_slider(scene_ui::SceneSlider::CameraZoomedSize.id(), 20);
                        studio.set_scene_slider(scene_ui::SceneSlider::CameraSplitRatio.id(), 30);
                        let camera = studio.camera_overlay;
                        let full = camera.rect_for_frame(700.0, 500.0, 4.0 / 3.0, 1.0);
                        let zoomed = camera.rect_for_frame(700.0, 500.0, 4.0 / 3.0, 2.0);
                        assert!(zoomed.width < full.width);
                        assert!((full.width / full.height - 4.0 / 3.0).abs() < 1e-9);
                        studio.video_zoom_cues = vec![ZoomCue::pinned(
                            0.5,
                            3.5,
                            2.0,
                            NormalizedPoint { x: 0.45, y: 0.45 },
                        )];
                        studio.video_selected_zoom_cue = Some(studio.video_zoom_cues[0].id);
                        studio.video_undo_stack.clear();
                        studio.video_redo_stack.clear();
                        assert!(studio.edit_selected_region(|cue| {
                            cue.presentation = ZoomPresentation::GlassLoupe;
                            cue.camera_motion = CameraMotion::Smooth;
                            cue.loupe.radius = 0.32;
                            cue.loupe.bevel = 0.3;
                            cue.loupe.chromatic_aberration = 0.4;
                            cue.start_animation_early = true;
                        }));
                        studio.undo_video_edit(cx);
                        assert_eq!(
                            studio.video_zoom_cues[0].presentation,
                            ZoomPresentation::Classic
                        );
                        studio.redo_video_edit(cx);
                        studio.video_selected_zoom_cue = Some(studio.video_zoom_cues[0].id);
                        studio.duplicate_selected_motion();
                        assert_eq!(studio.video_zoom_cues.len(), 2);
                        assert_ne!(studio.video_zoom_cues[0].id, studio.video_zoom_cues[1].id);
                        studio.edit_selected_region(|cue| {
                            cue.presentation = ZoomPresentation::Classic;
                        });
                        studio.apply_selected_zoom_to_all();
                        assert!(studio
                            .video_zoom_cues
                            .iter()
                            .all(|cue| cue.presentation == ZoomPresentation::Classic));
                        studio.undo_video_edit(cx);
                        assert_eq!(
                            studio.video_zoom_cues[0].presentation,
                            ZoomPresentation::GlassLoupe
                        );
                        studio.video_selected_zoom_cue = Some(studio.video_zoom_cues[0].id);
                        studio.apply_selected_zoom_to_all();
                        for motion in PointerMotion::ALL {
                            studio.pointer_style.motion = motion;
                            studio.rebuild_video_motion_timelines();
                            let frame = studio.video_pointer_timeline.frame_at(2.0).unwrap();
                            assert!((frame.location.x * 320.0 - 80.0).abs() < 2.0);
                        }
                        studio.pointer_style.rotation = PointerRotation::Directional;
                        studio.pointer_style.hide_when_idle = true;
                        studio.pointer_style.hide_when_typing = true;
                        studio.pointer_style.loop_to_start = true;
                        studio.set_scene_slider(scene_ui::SceneSlider::CursorRotation.id(), 65);
                        studio.set_scene_slider(scene_ui::SceneSlider::CursorIdleDelay.id(), 40);
                        studio.set_scene_slider(scene_ui::SceneSlider::CursorLoopDuration.id(), 50);
                        studio.rebuild_video_motion_timelines();
                        let style = studio.scene_style();
                        let cues = studio.video_zoom_cues.clone();
                        studio.autosave_scene_style();
                        studio.open_video_project(project.clone()).unwrap();
                        assert_eq!(studio.camera_overlay, style.camera);
                        let mut expected_pointer = style.pointer;
                        for (expected, restored) in [
                            (
                                &mut expected_pointer.rotation_strength,
                                studio.pointer_style.rotation_strength,
                            ),
                            (
                                &mut expected_pointer.idle_hide_delay,
                                studio.pointer_style.idle_hide_delay,
                            ),
                            (
                                &mut expected_pointer.loop_return_duration,
                                studio.pointer_style.loop_return_duration,
                            ),
                        ] {
                            assert!((*expected - restored).abs() < 1e-12);
                            *expected = restored;
                        }
                        assert_eq!(studio.pointer_style, expected_pointer);
                        assert_eq!(studio.video_zoom_cues, cues);
                        assert_eq!(studio.custom_canvas_dimensions, (1400, 1000));
                        assert!((studio.scene_style().aspect.unwrap() - 1.4).abs() < 0.0001);
                        studio.video_position = 2.0;
                        assert!(studio.video_viewport_timeline.frame_at(2.0).loupe.is_some());
                        studio.video_selected_zoom_cue = Some(studio.video_zoom_cues[0].id);
                        studio.scene_preview_image_ready(px(700.0), px(500.0)).unwrap();
                        cx.notify();
                    })
                    .unwrap();
                for (tab, name) in [
                    (shell_ui::InspectorTab::Design, "general-controls.png"),
                    (shell_ui::InspectorTab::Camera, "camera-controls.png"),
                    (shell_ui::InspectorTab::Cursor, "cursor-controls.png"),
                    (shell_ui::InspectorTab::Motion, "glass-controls.png"),
                ] {
                    handle
                        .update(cx, |studio, window, cx| {
                            studio.select_inspector_tab(tab);
                            let time = if tab == shell_ui::InspectorTab::Motion {
                                2.0
                            } else {
                                7.8
                            };
                            studio.seek_video(time, cx);
                            if tab == shell_ui::InspectorTab::Motion {
                                studio.video_selected_zoom_cue = Some(studio.video_zoom_cues[0].id);
                            }
                            studio.inspector_visible = true;
                            window.activate_window();
                            window.refresh();
                            cx.notify();
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(600)).await;
                    save_display(&screenshots.join(name), false);
                }
                // Run the real file transport and preview worker in this same
                // application. The presented image must follow playback while
                // the UI executor remains available for timer/input work.
                handle
                    .update(cx, |studio, _, cx| {
                        studio.seek_video(0.5, cx);
                        studio.start_video_playback(cx);
                        assert!(studio.video_playing);
                        cx.notify();
                    })
                    .unwrap();
                let warmup = std::time::Instant::now();
                let first_presented = loop {
                    Timer::after(Duration::from_millis(50)).await;
                    let ready = handle
                        .update(cx, |studio, _, _| {
                            assert!(studio.video_playing, "transport stopped before glass preview was ready");
                            let generation = studio
                                .video_playback_generation
                                .load(std::sync::atomic::Ordering::SeqCst);
                            studio.preview_cache.frame.as_ref().and_then(|(key, _)| {
                                (key.generation == generation
                                    && key.time() > 0.6
                                    && key.viewport.loupe.is_some())
                                .then_some(key.time())
                            })
                        })
                        .unwrap();
                    if let Some(time) = ready {
                        break time;
                    }
                    assert!(warmup.elapsed() < Duration::from_secs(6), "glass preview did not start");
                };
                let warmup_elapsed = warmup.elapsed();
                let mut presented = Vec::new();
                let mut wake_times = Vec::new();
                let mut lags = Vec::new();
                for _ in 0..20 {
                    let scheduled = std::time::Instant::now();
                    Timer::after(Duration::from_millis(75)).await;
                    wake_times.push(scheduled.elapsed());
                    let (time, lag) = handle
                        .update(cx, |studio, _, _| {
                            assert!(studio.video_playing, "glass playback unexpectedly stopped");
                            let key = &studio.preview_cache.frame.as_ref().unwrap().0;
                            assert!(key.viewport.loupe.is_some(), "sample must show the glass effect");
                            (key.time(), studio.video_position - key.time())
                        })
                        .unwrap();
                    presented.push(time);
                    lags.push(lag);
                }
                let mut distinct = presented.clone();
                distinct.dedup_by(|a, b| a.to_bits() == b.to_bits());
                wake_times.sort();
                let p95 = wake_times[18];
                let max_wake = *wake_times.last().unwrap();
                let max_lag = lags.iter().copied().fold(0.0_f64, f64::max);
                eprintln!(
                    "Glass playback: {} presented timestamps in 20 UI samples, warmup={warmup_elapsed:?}, wake p95={p95:?}, max={max_wake:?}, max preview lag={max_lag:.3}s",
                    distinct.len()
                );
                assert!(distinct.len() >= 5, "preview must present advancing frames");
                assert!(presented.windows(2).all(|pair| pair[1] >= pair[0]));
                assert!(presented.last().unwrap() - first_presented > 0.75);
                assert!(max_wake < Duration::from_secs(1), "UI was blocked during playback");
                assert!(p95 < Duration::from_millis(500), "UI repeatedly stalled during playback");
                assert!(max_lag < 0.75, "preview accumulated latency behind playback");
                assert!(lags.last().unwrap() < &(lags[0] + 0.4), "preview backlog grew");
                handle
                    .update(cx, |studio, _, cx| {
                        studio.pause_video_playback();
                        studio.seek_video(2.0, cx);
                        cx.notify();
                    })
                    .unwrap();
                Timer::after(Duration::from_millis(300)).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(!studio.video_playing);
                        assert_eq!(studio.video_position, 2.0);
                        let key = &studio.preview_cache.frame.as_ref().unwrap().0;
                        assert_eq!(key.time(), 2.0, "paused preview must show the exact seek time");
                        assert!(key.viewport.loupe.is_some());
                    })
                    .unwrap();
                save_display(&screenshots.join("glass-playback-paused.png"), false);
                let recorder = handle
                    .update(cx, |studio, _, cx| {
                        studio.record_camera = false;
                        studio.recording_camera_enabled = false;
                        studio.camera_device = None;
                        studio.open_recorder_window(cx);
                        assert!(studio.video_project.is_some());
                        studio.recorder_window.unwrap()
                    })
                    .unwrap();
                recorder
                    .update(cx, |studio, window, cx| {
                        studio.load_generation = studio.load_generation.wrapping_add(1);
                        studio.loading = None;
                        assert!(studio.launcher_active);
                        assert_eq!(studio.custom_canvas_dimensions, (1400, 1000));
                        assert!(!studio.record_camera);
                        window.activate_window();
                        window.refresh();
                        cx.notify();
                    })
                    .unwrap();
                Timer::after(Duration::from_millis(600)).await;
                save_display(&screenshots.join("recorder-launcher.png"), true);
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
    verify_encoded_workflow(&session, &artifacts);
}

async fn exercise_motion_controls(
    handle: WindowHandle<Studio>,
    editor: AnyWindowHandle,
    screenshots: &Path,
    cx: &mut gpui::AsyncApp,
) {
    // Redo returns to the overview. Reopen through the visible region list,
    // then exercise the primary controls and their conditional content.
    Timer::after(Duration::from_millis(150)).await;
    save_display(&screenshots.join("motion-region-list.png"), false);
    let cue_id = handle
        .update(cx, |studio, _, _| studio.video_zoom_cues[0].id)
        .unwrap();
    workflow_click(editor, 1260.0, 340.0, false, cx).await; // motion-region-select/0
    handle
        .update(cx, |studio, _, _| {
            assert_eq!(studio.video_selected_zoom_cue, Some(cue_id));
            assert_eq!(studio.video_zoom_cues.len(), 1);
            assert!(!studio.section_open("motion-advanced"));
            assert!(!studio.section_open("motion-transform"));
        })
        .unwrap();

    workflow_click(editor, 1345.0, 420.0, false, cx).await; // motion-position/Manual
    let initial_point = handle
        .update(cx, |studio, _, _| {
            let cue = studio.selected_region().unwrap();
            assert_eq!(cue.anchor_mode, ZoomAnchorMode::PinnedAnchor);
            cue.pinned_point
        })
        .unwrap();
    save_display(&screenshots.join("motion-manual-position.png"), false);
    workflow_click(editor, 1130.0, 480.0, false, cx).await; // thumbnail letterbox
    handle
        .update(cx, |studio, _, _| {
            assert_eq!(
                studio.selected_region().unwrap().pinned_point,
                initial_point,
                "letterboxing must not change the chosen source point"
            );
        })
        .unwrap();
    workflow_click(editor, 1325.0, 480.0, false, cx).await; // motion-position-preview
    handle
        .update(cx, |studio, _, _| {
            let cue = studio.selected_region().unwrap();
            assert_eq!(cue.anchor_mode, ZoomAnchorMode::PinnedAnchor);
            assert!((0.7..0.9).contains(&cue.pinned_point.x));
            assert!((0.2..0.5).contains(&cue.pinned_point.y));
            assert_ne!(cue.pinned_point, initial_point);
        })
        .unwrap();
    save_display(
        &screenshots.join("motion-manual-position-picked.png"),
        false,
    );
    workflow_click(editor, 1195.0, 420.0, false, cx).await; // motion-position/Auto
    handle
        .update(cx, |studio, _, _| {
            let cue = studio.selected_region().unwrap();
            assert_eq!(cue.anchor_mode, ZoomAnchorMode::SmartAnchor);
            assert!(cue.pan_to.is_none());
        })
        .unwrap();
    let undo_count = handle
        .update(cx, |studio, _, _| studio.video_undo_stack.len())
        .unwrap();
    workflow_click(editor, 1195.0, 420.0, false, cx).await;
    handle
        .update(cx, |studio, _, _| {
            assert_eq!(studio.video_undo_stack.len(), undo_count)
        })
        .unwrap();

    workflow_click(editor, 1190.0, 275.0, false, cx).await; // motion-presentation/Classic
    let instant = handle
        .update(cx, |studio, _, _| {
            let cue = studio.selected_region().unwrap();
            assert_eq!(cue.presentation, ZoomPresentation::Classic);
            cue.skips_easing
        })
        .unwrap();
    workflow_click(editor, 1405.0, 348.0, false, cx).await; // conditional motion-instant
    handle
        .update(cx, |studio, _, _| {
            assert_eq!(studio.selected_region().unwrap().skips_easing, !instant);
        })
        .unwrap();
    save_display(&screenshots.join("motion-classic-inspector.png"), false);
    workflow_click(editor, 1405.0, 348.0, false, cx).await;
    workflow_click(editor, 1345.0, 275.0, false, cx).await; // motion-presentation/Glass

    workflow_click(editor, 1250.0, 596.0, false, cx).await; // section-header-motion-advanced
    let before = handle
        .update(cx, |studio, _, _| {
            assert!(studio.section_open("motion-advanced"));
            let cue = studio.selected_region().unwrap();
            assert_eq!(cue.presentation, ZoomPresentation::GlassLoupe);
            assert!(cue.loupe.glass);
            cue
        })
        .unwrap();
    save_display(&screenshots.join("motion-advanced-expanded.png"), false);
    workflow_click(editor, 1405.0, 639.0, false, cx).await; // motion-early
    handle
        .update(cx, |studio, _, cx| {
            assert_eq!(
                studio.animation_settings.start_early,
                Some(!before.start_animation_early)
            );
            assert_eq!(studio.selected_region().unwrap().start_animation_early, before.start_animation_early,
                "project overrides must preserve the region's authored legacy setting");
            studio.undo_video_edit(cx);
            assert_eq!(studio.animation_settings.start_early, None);
        })
        .unwrap();
    workflow_click(editor, 1405.0, 763.0, false, cx).await; // loupe-glass
    handle
        .update(cx, |studio, _, _| {
            assert!(!studio.selected_region().unwrap().loupe.glass)
        })
        .unwrap();
    workflow_click(editor, 1405.0, 763.0, false, cx).await;
    workflow_click(editor, 1410.0, 802.0, false, cx).await; // loupe-parameter/Bevel+
    handle
        .update(cx, |studio, _, _| {
            let cue = studio.selected_region().unwrap();
            assert!(cue.loupe.glass);
            assert!((cue.loupe.bevel - before.loupe.bevel - 0.05).abs() < 1e-9);
        })
        .unwrap();
    save_display(
        &screenshots.join("motion-advanced-optics-edited.png"),
        false,
    );
    workflow_click(editor, 1250.0, 596.0, false, cx).await;
    handle
        .update(cx, |studio, _, _| {
            assert!(!studio.section_open("motion-advanced"))
        })
        .unwrap();
    workflow_click(editor, 1250.0, 652.0, false, cx).await; // section-header-motion-transform
    handle
        .update(cx, |studio, _, _| {
            assert!(studio.section_open("motion-transform"))
        })
        .unwrap();
    save_display(
        &screenshots.join("motion-pan-transform-expanded.png"),
        false,
    );
    workflow_click(editor, 1250.0, 652.0, false, cx).await;
    handle
        .update(cx, |studio, _, _| {
            assert!(!studio.section_open("motion-transform"))
        })
        .unwrap();
    save_display(&screenshots.join("motion-primary-controls.png"), false);

    workflow_click(editor, 1122.0, 86.0, false, cx).await; // inspector-back
    handle
        .update(cx, |studio, _, _| {
            assert!(studio.video_selected_zoom_cue.is_none());
            assert_eq!(studio.video_zoom_cues.len(), 1);
        })
        .unwrap();
    workflow_click(editor, 1260.0, 340.0, false, cx).await;
    handle
        .update(cx, |studio, _, _| {
            assert_eq!(studio.video_selected_zoom_cue, Some(cue_id));
            assert_eq!(studio.video_zoom_cues.len(), 1);
        })
        .unwrap();

    workflow_click(editor, 1190.0, 275.0, false, cx).await; // Classic at the 1× boundary
    let (before_zoom, undo_count) = handle
        .update(cx, |studio, _, _| {
            (
                studio.video_zoom_cues.clone(),
                studio.video_undo_stack.len(),
            )
        })
        .unwrap();
    workflow_click(editor, 1125.0, 530.0, false, cx).await; // track start / 1×
    handle
        .update(cx, |studio, _, _| {
            let cue = studio.selected_region().unwrap();
            assert_eq!(cue.presentation, ZoomPresentation::Classic);
            assert!(cue.transform.is_none());
            assert_eq!(cue.zoom, 1.0);
            assert_eq!(studio.video_undo_stack.len(), undo_count + 1);
        })
        .unwrap();
    save_display(&screenshots.join("motion-classic-one-times.png"), false);
    // The primary Zoom controls must remain interactive at 1×. This second
    // native click fails if the inspector mistakenly changes to Transform.
    workflow_click(editor, 1275.0, 530.0, false, cx).await;
    handle
        .update(cx, |studio, _, cx| {
            assert!(
                studio.selected_region().unwrap().zoom > 1.0,
                "the zoom track must remain interactive after choosing 1×"
            );
            assert_eq!(studio.video_undo_stack.len(), undo_count + 2);
            studio.undo_video_edit(cx);
            assert_eq!(
                studio.video_zoom_cues[0].zoom, 1.0,
                "one undo must restore the zoom before the last native track click"
            );
            studio.undo_video_edit(cx);
            assert_eq!(
                studio.video_zoom_cues, before_zoom,
                "two clicks must undo as two independent edits"
            );
        })
        .unwrap();
}

async fn exercise_project_animation_controls(
    handle: WindowHandle<Studio>,
    editor: AnyWindowHandle,
    screenshots: &Path,
    cx: &mut gpui::AsyncApp,
) {
    let (authored, previous_pointer) = handle.update(cx, |studio, window, cx| {
        studio.select_inspector_tab(shell_ui::InspectorTab::Motion);
        studio.open_sections.insert("project-animation");
        studio.animation_settings = Default::default();
        studio.rebuild_video_motion_timelines();
        window.refresh();
        cx.notify();
        (studio.video_zoom_cues.clone(), studio.pointer_style.motion)
    }).unwrap();
    Timer::after(Duration::from_millis(200)).await;
    editor.update(cx, |_, window, cx| window.draw(cx).clear()).unwrap();
    Timer::after(Duration::from_millis(100)).await;
    save_display(&screenshots.join("project-animation-region-settings.png"), false);

    // Choose the first visible screen preset through the native input path.
    workflow_hover(1170.0, 515.0, cx).await;
    handle.update(cx, |studio, _, _| assert!(studio.animation_hover.is_some())).unwrap();
    save_display(&screenshots.join("project-animation-screen-hover.png"), false);
    workflow_click(editor, 1170.0, 515.0, false, cx).await;
    handle.update(cx, |studio, _, cx| {
        assert_eq!(studio.animation_settings.screen_motion, Some(CameraMotion::Focused));
        assert_eq!(studio.video_zoom_cues, authored);
        studio.undo_video_edit(cx);
        assert_eq!(studio.animation_settings.screen_motion, None);
        studio.redo_video_edit(cx);
        assert_eq!(studio.animation_settings.screen_motion, Some(CameraMotion::Focused));

    }).unwrap();
    let undo_count = handle.update(cx, |studio, _, _| studio.video_undo_stack.len()).unwrap();
    workflow_hover(1385.0, 660.0, cx).await;
    handle.update(cx, |studio, _, _| assert!(studio.animation_hover.is_some())).unwrap();
    save_display(&screenshots.join("project-animation-cursor-hover.png"), false);
    workflow_click(editor, 1385.0, 660.0, false, cx).await;
    handle.update(cx, |studio, _, cx| {
        assert_eq!(studio.pointer_style.motion, PointerMotion::Rapid);
        assert_eq!(studio.video_undo_stack.len(), undo_count + 1);
        studio.undo_video_edit(cx);
        assert_eq!(studio.pointer_style.motion, previous_pointer);
        studio.redo_video_edit(cx);
        assert_eq!(studio.pointer_style.motion, PointerMotion::Rapid);
    }).unwrap();
    workflow_click(editor, 1380.0, 753.0, false, cx).await;
    let reopened_poster = handle.update(cx, |studio, _, cx| {
        assert_eq!(studio.animation_settings.motion_blur, 2);
        studio.undo_video_edit(cx);
        assert_eq!(studio.animation_settings.motion_blur, 0);
        studio.redo_video_edit(cx);
        assert_eq!(studio.animation_settings.motion_blur, 2);
        studio.set_project_early(Some(true));
        studio.record_animation_edit();
        studio.set_scene_slider(scene_ui::SceneSlider::MotionBlurAmount.id(), 65);
        studio.undo_video_edit(cx);
        assert_eq!(studio.animation_settings.motion_blur, 2);
        studio.redo_video_edit(cx);
        assert_eq!(studio.animation_settings.motion_blur, 65);
        assert!(!studio.annotations_paint_flat(),"temporal media marks must have one compositor owner");
        assert!(studio.animation_settings.cues(&authored).iter().all(|cue|
            cue.camera_motion == CameraMotion::Focused && cue.start_animation_early));
        let expected = studio.scene_style();
        studio.autosave_scene_style();
        let session = studio.video_project.clone().unwrap();
        let saved: SceneStyle = session.read_edit_field("scene").unwrap().unwrap();
        assert_eq!(saved.animation, expected.animation);
        assert_eq!(saved.pointer.motion, PointerMotion::Rapid);
        studio.open_video_project(session.directory).unwrap();
        assert_eq!(studio.animation_settings, expected.animation);
        assert_eq!(studio.pointer_style.motion, PointerMotion::Rapid);
        assert_eq!(studio.video_zoom_cues, authored);
        studio.select_inspector_tab(shell_ui::InspectorTab::Motion);
        studio.open_sections.insert("project-animation");
        studio.seek_video(0.0, cx);
        cx.notify();
        studio.video_frame_rgba.clone()
    }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        Timer::after(Duration::from_millis(100)).await;
        let ready = handle.update(cx, |studio, _, cx| {
            let ready = studio.video_frame_rgba.as_ref().is_some_and(|frame|
                reopened_poster.as_ref().is_none_or(|poster| !Arc::ptr_eq(frame, poster)))
                && studio.scene_preview_image_ready(px(700.0), px(500.0)).is_some();
            cx.notify();
            ready
        }).unwrap();
        if ready { break; }
        assert!(Instant::now() < deadline, "reopened animation settings must render a decoded frame");
    }
    editor.update(cx, |_, window, cx| window.draw(cx).clear()).unwrap();
    Timer::after(Duration::from_millis(100)).await;
    save_display(&screenshots.join("project-animation-saved.png"), false);
    handle.update(cx, |studio, _, cx| {
        studio.set_screen_animation(None);
        studio.set_project_early(None);
        studio.set_cursor_animation(previous_pointer);
        studio.set_scene_slider(scene_ui::SceneSlider::MotionBlurAmount.id(), 0);
        assert_eq!(studio.animation_settings.cues(&authored), authored);
        studio.autosave_scene_style();
        cx.notify();
    }).unwrap();
}

async fn workflow_hover(x: f32, y: f32, _cx: &mut gpui::AsyncApp) {
    use x11rb::{connection::Connection, protocol::{xproto::ConnectionExt as _, xtest::ConnectionExt as _}};
    let (connection, index) = x11rb::connect(None).unwrap();
    let root = connection.setup().roots[index].root;
    let (drawable, _, _, _) = own_x11_window(&connection, root, false);
    let origin = connection.translate_coordinates(drawable, root, 0, 0).unwrap().reply().unwrap();
    connection.xtest_fake_input(x11rb::protocol::xproto::MOTION_NOTIFY_EVENT, 0, 0, root,
        origin.dst_x + x as i16, origin.dst_y + y as i16, 0).unwrap().check().unwrap();
    connection.flush().unwrap();
    Timer::after(Duration::from_millis(350)).await;
}

fn verify_encoded_workflow(session: &RecordingSession, artifacts: &Path) {
    let style: SceneStyle = session.read_edit_field("scene").unwrap().unwrap();
    let cues: Vec<ZoomCue> = session.read_edit_field("zoomCues").unwrap().unwrap();
    let capture = session.read_pointer_capture().unwrap();
    let clips = session.effective_clip_timeline(8.0).unwrap();
    let pointer = PointerTimeline::build_with_clip_timeline(
        capture.clone(),
        8.0,
        320.0,
        180.0,
        style.pointer.timeline_options(),
        Some(&clips),
    );
    let mut viewport = ViewportTimeline::build_for_viewport(
        &cues,
        &pointer,
        &clips,
        &capture,
        style.focus_bounds(320, 180),
    );
    if let Some(fill) = style.fill_fraction(320, 180) {
        viewport.set_reframe(fill, style.source_crop, |time| pointer.location_at(time));
    }
    let destination = artifacts.join("studio-workflow.mp4");
    let mut request = SceneExportRequest::new(
        destination.clone(),
        ExportFormat::Mp4,
        180,
        style.clone(),
        viewport.clone(),
        4.0,
    );
    request.frame_rate = 10.0;
    request.include_audio = false;
    export_scene(
        SceneSource::Video {
            media: session.screen_path(),
            clips,
            pointer: Some(pointer.clone()),
            camera: Some(session.camera_path()),
        },
        &mut request,
        &ExportProgress::default(),
    )
    .unwrap();
    let output = probe_media(&destination).unwrap();
    let (width, height) = style.export_canvas_size(320, 180, 180);
    assert_eq!((output.width, output.height), (width, height));
    assert!((output.duration - 4.0).abs() < 0.15);
    let decoded = decode_frame(&destination, 2.0, width, height).unwrap();
    let source = decode_frame(&session.screen_path(), 2.0, 320, 180).unwrap();
    let camera = decode_frame(&session.camera_path(), 2.0, 160, 120).unwrap();
    let source = image::RgbaImage::from_raw(320, 180, source.rgba).unwrap();
    let camera = image::RgbaImage::from_raw(160, 120, camera.rgba).unwrap();
    let cursor = PointerOverlay {
        frame: pointer.frame_at(2.0).unwrap(),
    };
    let view = viewport.frame_at(2.0);
    assert!(view.loupe.is_some());
    let compositor = SceneCompositor::new(&style, width, height, 320, 180).unwrap();
    let preview = compositor.compose(FrameInput {
        source: &source,
        overlay: None,
        viewport: view,
        pointer: Some(&cursor),
        camera: Some(&camera),
    });
    preview.save(artifacts.join("hold-preview.png")).unwrap();
    let encoded = image::RgbaImage::from_raw(width, height, decoded.rgba).unwrap();
    encoded.save(artifacts.join("hold-export.png")).unwrap();
    let error: f64 = preview
        .pixels()
        .zip(encoded.pixels())
        .map(|(a, b)| {
            (0..3)
                .map(|i| (f64::from(a[i]) - f64::from(b[i])).abs())
                .sum::<f64>()
        })
        .sum::<f64>()
        / (f64::from(width) * f64::from(height) * 3.0);
    assert!(error < 12.0, "preview/export mean channel error {error}");
    for layout in CameraLayout::ALL {
        let mut variant = style.clone();
        variant.camera.layout = layout;
        let result = SceneCompositor::new(&variant, width, height, 320, 180)
            .unwrap()
            .compose(FrameInput {
                source: &source,
                overlay: None,
                viewport: viewport.frame_at(7.8),
                pointer: Some(&cursor),
                camera: Some(&camera),
            });
        result
            .save(artifacts.join(format!("camera-layout-{layout:?}.png")))
            .unwrap();
        assert_ne!(result.get_pixel(width / 2, height / 2).0, [0, 0, 0, 0]);
    }
    eprintln!(
        "Workflow screenshots, preview comparison and MP4: {} (mean channel error {error:.3})",
        artifacts.display()
    );
}

/// Native pointer events exercise the rendered targets and outside listeners.
/// The pointer is restored after each click and no capture device is opened.
async fn recorder_click(handle: AnyWindowHandle, x: f32, y: f32, cx: &mut gpui::AsyncApp) {
    workflow_click(handle, x, y, true, cx).await;
}

async fn workflow_click(
    handle: AnyWindowHandle,
    x: f32,
    y: f32,
    compact: bool,
    cx: &mut gpui::AsyncApp,
) {
    use x11rb::{
        connection::Connection,
        protocol::{
            xproto::{ConnectionExt as _, *},
            xtest::ConnectionExt as _,
        },
    };
    let (connection, index) = x11rb::connect(None).unwrap();
    let root = connection.setup().roots[index].root;
    let (drawable, _, _, _) = own_x11_window(&connection, root, compact);
    let origin = connection
        .translate_coordinates(drawable, root, 0, 0)
        .unwrap()
        .reply()
        .unwrap();
    let before = connection.query_pointer(root).unwrap().reply().unwrap();
    assert!(
        !before.mask.contains(KeyButMask::BUTTON1),
        "release pointer before running native test"
    );
    connection
        .set_input_focus(InputFocus::PARENT, drawable, x11rb::CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    connection
        .xtest_fake_input(
            MOTION_NOTIFY_EVENT,
            0,
            0,
            root,
            origin.dst_x + x as i16,
            origin.dst_y + y as i16,
            0,
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    Timer::after(Duration::from_millis(40)).await;
    connection
        .xtest_fake_input(BUTTON_PRESS_EVENT, 1, 0, root, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    Timer::after(Duration::from_millis(40)).await;
    connection
        .xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, root, 0, 0, 0)
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    Timer::after(Duration::from_millis(100)).await;
    connection
        .xtest_fake_input(
            MOTION_NOTIFY_EVENT,
            0,
            0,
            root,
            before.root_x,
            before.root_y,
            0,
        )
        .unwrap()
        .check()
        .unwrap();
    connection.flush().unwrap();
    handle
        .update(cx, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    Timer::after(Duration::from_millis(60)).await;
}

async fn recorder_escape(handle: AnyWindowHandle, cx: &mut gpui::AsyncApp) {
    handle
        .update(cx, |_, window, cx| {
            window.dispatch_keystroke(gpui::Keystroke::parse("escape").unwrap(), cx);
        })
        .unwrap();
    Timer::after(Duration::from_millis(100)).await;
    handle
        .update(cx, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    Timer::after(Duration::from_millis(60)).await;
}

fn recorder_synthetic_devices(studio: &mut Studio) {
    studio.camera_devices = vec![
        (
            "/dev/lahza-fixture-c922".into(),
            "C922 Pro Stream Webcam".into(),
        ),
        (
            "/dev/lahza-fixture-c920".into(),
            "HD Pro Webcam C920".into(),
        ),
    ];
    studio.microphone_devices = vec![
        ("lahza-fixture-earpods".into(), "EarPods".into()),
        (
            "lahza-fixture-internal".into(),
            "Built-in Microphone".into(),
        ),
    ];
}

async fn recorder_wait_for_access(handle: WindowHandle<Studio>, cx: &mut gpui::AsyncApp) {
    for _ in 0..100 {
        Timer::after(Duration::from_millis(50)).await;
        if handle
            .update(cx, |studio, _, _| {
                assert!(
                    studio.capture_access_prompt.is_none(),
                    "device enumeration should succeed outside confinement"
                );
                !studio.capture_access_busy
            })
            .unwrap()
        {
            return;
        }
    }
    panic!("device permission action did not finish");
}

#[test]
#[ignore = "requires an X11 display and graphics driver; uses synthetic devices without opening hardware"]
fn recorder_device_selection_enables_capture_and_switches_disable() {
    let artifacts = artifact_directory();
    Application::new()
        .with_assets(Assets {
            base: asset_directory(),
        })
        .run(move |cx| {
            let handle = open_studio_window(cx, false, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            })
            .unwrap();
            handle
                .update(cx, |studio, window, cx| {
                    studio.load_generation = studio.load_generation.wrapping_add(1);
                    studio.loading = None;
                    studio.record_camera = false;
                    studio.record_microphone = false;
                    studio.record_system_audio = false;
                    studio.camera_device = Some("/dev/lahza-fixture-c922".into());
                    studio.microphone_device = Some("lahza-fixture-earpods".into());
                    recorder_synthetic_devices(studio);
                    studio.camera_access_checked = true;
                    studio.focus_handle.focus(window);
                    window.activate_window();
                    cx.notify();
                })
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                Timer::after(Duration::from_millis(300)).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert_eq!(
                            studio.camera_devices.len(),
                            2,
                            "cancelled startup discovery replaced fixture devices"
                        );
                        assert_eq!(studio.camera_devices[0].1, "C922 Pro Stream Webcam");
                        assert_eq!(studio.microphone_devices.len(), 2);
                        assert_eq!(studio.microphone_devices[0].1, "EarPods");
                    })
                    .unwrap();
                let window: AnyWindowHandle = handle.into();
                window
                    .update(cx, |_, window, cx| window.draw(cx).clear())
                    .unwrap();
                Timer::after(Duration::from_millis(120)).await;
                save_display(&artifacts.join("recorder-devices-closed.png"), true);

                // Device fields are distinct hit targets below the header switches.
                recorder_click(window, 210.0, 392.0, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(
                            studio.launcher_camera_menu_open,
                            "camera field must open its menu"
                        );
                        assert!(!studio.launcher_mic_menu_open);
                        assert!(
                            !studio.record_camera,
                            "opening the camera field must keep it Off"
                        );
                        assert!(studio.camera_preview.is_none());
                    })
                    .unwrap();
                save_display(&artifacts.join("recorder-camera-menu.png"), true);
                recorder_click(window, 210.0, 392.0, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(
                            !studio.launcher_camera_menu_open,
                            "clicking the same field must close its menu"
                        );
                        assert_eq!(
                            studio.camera_device.as_deref(),
                            Some("/dev/lahza-fixture-c922")
                        );
                    })
                    .unwrap();
                recorder_click(window, 210.0, 392.0, cx).await;
                // The real window is already active. Suppress only the preview
                // pipeline while exercising permission completion and On state.
                handle
                    .update(cx, |studio, _, _| studio.recording_window_minimized = true)
                    .unwrap();
                recorder_click(window, 210.0, 305.0, cx).await;
                recorder_wait_for_access(handle, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert_eq!(
                            studio.camera_device.as_deref(),
                            Some("/dev/lahza-fixture-c920"),
                            "clicking another camera must select it"
                        );
                        assert!(!studio.launcher_camera_menu_open);
                        assert!(studio.record_camera, "choosing a camera must turn it On");
                        assert!(studio.recording_window_minimized);
                        recorder_synthetic_devices(studio);
                        assert!(studio.camera_preview.is_none());
                    })
                    .unwrap();
                recorder_click(window, 382.0, 358.0, cx).await;
                handle
                    .update(cx, |studio, _, _| assert!(!studio.record_camera))
                    .unwrap();
                // Re-selecting the already chosen camera also enables it.
                recorder_click(window, 210.0, 392.0, cx).await;
                recorder_click(window, 210.0, 305.0, cx).await;
                recorder_wait_for_access(handle, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(studio.record_camera);
                        assert!(studio.camera_preview.is_none());
                        recorder_synthetic_devices(studio);
                    })
                    .unwrap();
                recorder_click(window, 382.0, 358.0, cx).await;
                recorder_click(window, 210.0, 392.0, cx).await;
                recorder_escape(window, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(
                            !studio.launcher_camera_menu_open,
                            "Escape must dismiss the menu"
                        );
                        assert_eq!(
                            studio.camera_device.as_deref(),
                            Some("/dev/lahza-fixture-c920")
                        );
                    })
                    .unwrap();

                recorder_click(window, 210.0, 500.0, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(
                            studio.launcher_mic_menu_open,
                            "microphone field must open its menu"
                        );
                        assert!(!studio.launcher_camera_menu_open);
                        assert!(!studio.record_microphone);
                    })
                    .unwrap();
                save_display(&artifacts.join("recorder-microphone-menu.png"), true);
                recorder_click(window, 210.0, 413.0, cx).await;
                recorder_wait_for_access(handle, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert_eq!(
                            studio.microphone_device.as_deref(),
                            Some("lahza-fixture-internal"),
                            "clicking another microphone must select it"
                        );
                        assert!(
                            studio.record_microphone,
                            "choosing a microphone must turn it On"
                        );
                        assert!(!studio.launcher_mic_menu_open);
                        recorder_synthetic_devices(studio);
                    })
                    .unwrap();
                recorder_click(window, 210.0, 500.0, cx).await;
                recorder_click(window, 215.0, 25.0, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(
                            !studio.launcher_mic_menu_open,
                            "outside click must dismiss the menu"
                        );
                        assert!(
                            studio.record_microphone,
                            "dismissal must not change enabled state"
                        );
                    })
                    .unwrap();

                // The label is not a switch. The explicit switch disables
                // capture while retaining the selected device.
                recorder_click(window, 110.0, 458.0, cx).await;
                handle
                    .update(cx, |studio, _, _| assert!(studio.record_microphone))
                    .unwrap();
                recorder_click(window, 382.0, 458.0, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(
                            !studio.record_microphone,
                            "explicit switch must turn microphone Off"
                        );
                        assert_eq!(
                            studio.microphone_device.as_deref(),
                            Some("lahza-fixture-internal")
                        );
                        assert!(!studio.record_camera);
                        assert!(studio.camera_preview.is_none());
                    })
                    .unwrap();
                // Re-selecting the current microphone turns it back On too.
                recorder_click(window, 210.0, 500.0, cx).await;
                recorder_click(window, 210.0, 413.0, cx).await;
                recorder_wait_for_access(handle, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(studio.record_microphone);
                        assert_eq!(
                            studio.microphone_device.as_deref(),
                            Some("lahza-fixture-internal")
                        );
                        recorder_synthetic_devices(studio);
                    })
                    .unwrap();
                recorder_click(window, 382.0, 458.0, cx).await;
                handle
                    .update(cx, |studio, _, _| {
                        assert!(!studio.record_microphone);
                        assert!(!studio.record_camera);
                        assert!(studio.camera_preview.is_none());
                    })
                    .unwrap();
                save_display(&artifacts.join("recorder-devices-off.png"), true);
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
}
