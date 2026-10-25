//! Explicit, device-free diagnostics for the installed capture helper.
use super::*;
use gpui::Application;
use std::{collections::HashSet, path::Path};
use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt, ImageFormat, ImageOrder, MapState},
};

#[derive(Debug)]
struct NativePixels {
    window: u32,
    mapped: bool,
    bounds: (i16, i16, u16, u16),
    colors: usize,
}

fn real_monitors() -> Vec<DesktopRect> {
    use x11rb::protocol::randr::ConnectionExt as _;
    let (connection, index) = x11rb::connect(None).unwrap();
    connection
        .randr_get_monitors(connection.setup().roots[index].root, true)
        .unwrap()
        .reply()
        .unwrap()
        .monitors
        .into_iter()
        .map(|monitor| DesktopRect {
            x: monitor.x.into(),
            y: monitor.y.into(),
            width: monitor.width.into(),
            height: monitor.height.into(),
        })
        .collect()
}

fn snapshot_phase(
    phase: &str,
    exclusions: &[CaptureExclusion],
    monitors: &[DesktopRect],
    artifacts: &Path,
) -> (Vec<bool>, Vec<Option<usize>>) {
    let mut painted = Vec::new();
    let mut locations = Vec::new();
    for (index, name) in ["pill", "camera"].into_iter().enumerate() {
        let evidence = save_owned_window(
            exclusions[index].title(),
            &artifacts.join(format!("{name}-{phase}.png")),
        );
        let mut location = None;
        let mut rendered = false;
        if let Ok(Some(evidence)) = &evidence {
            let (x, y, width, height) = evidence.bounds;
            location = monitors.iter().position(|monitor| {
                i32::from(x) >= monitor.x
                    && i32::from(y) >= monitor.y
                    && i64::from(x) + i64::from(width)
                        <= i64::from(monitor.x) + i64::from(monitor.width)
                    && i64::from(y) + i64::from(height)
                        <= i64::from(monitor.y) + i64::from(monitor.height)
            });
            rendered = evidence.window != 0 && evidence.mapped && evidence.colors > 16;
        }
        eprintln!(
            "overlay probe {phase} {name}: {evidence:?}, monitor={location:?}, protected={}",
            exclusions[index].is_protected()
        );
        painted.push(rendered);
        locations.push(location);
    }
    (painted, locations)
}

/// Inspect only the exact-title window owned by this test process. Reading
/// the root framebuffer would conflate Vulkan/X11 presentation with painting.
fn save_owned_window(
    title: &str,
    path: &Path,
) -> Result<Option<NativePixels>, Box<dyn std::error::Error>> {
    let (connection, screen_index) = x11rb::connect(None)?;
    let screen = &connection.setup().roots[screen_index];
    let pid_atom = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
    let title_atom = connection
        .intern_atom(false, b"_NET_WM_NAME")?
        .reply()?
        .atom;
    let utf8 = connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    let mut pending = vec![screen.root];
    while let Some(parent) = pending.pop() {
        let Ok(tree) = connection.query_tree(parent)?.reply() else {
            continue;
        };
        for child in tree.children {
            pending.push(child);
            let Ok(pid) = connection
                .get_property(false, child, pid_atom, AtomEnum::CARDINAL, 0, 1)?
                .reply()
            else {
                continue;
            };
            if pid.value32().and_then(|mut values| values.next()) != Some(std::process::id()) {
                continue;
            }
            let name = connection
                .get_property(false, child, title_atom, utf8, 0, 1024)?
                .reply()?;
            if name.value != title.as_bytes() {
                continue;
            }
            let geometry = connection.get_geometry(child)?.reply()?;
            let attrs = connection.get_window_attributes(child)?.reply()?;
            let origin = connection
                .translate_coordinates(child, screen.root, 0, 0)?
                .reply()?;
            let mut evidence = NativePixels {
                window: child,
                mapped: attrs.map_state == MapState::VIEWABLE,
                bounds: (origin.dst_x, origin.dst_y, geometry.width, geometry.height),
                colors: 0,
            };
            if !evidence.mapped {
                return Ok(Some(evidence));
            }
            let pixels = connection
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    child,
                    0,
                    0,
                    geometry.width,
                    geometry.height,
                    u32::MAX,
                )?
                .reply()?;
            let format = connection
                .setup()
                .pixmap_formats
                .iter()
                .find(|format| format.depth == pixels.depth)
                .ok_or("missing pixmap format")?;
            if format.bits_per_pixel != 32 {
                return Err("probe expects 32-bit X11 pixels".into());
            }
            let visual = screen
                .allowed_depths
                .iter()
                .flat_map(|depth| &depth.visuals)
                .find(|visual| visual.visual_id == attrs.visual)
                .ok_or("missing native visual")?;
            let channel = |pixel: u32, mask: u32| ((pixel & mask) >> mask.trailing_zeros()) as u8;
            let image =
                image::RgbaImage::from_fn(geometry.width.into(), geometry.height.into(), |x, y| {
                    let offset = (y as usize * usize::from(geometry.width) + x as usize) * 4;
                    let bytes: [u8; 4] = pixels.data[offset..offset + 4].try_into().unwrap();
                    let pixel = if connection.setup().image_byte_order == ImageOrder::LSB_FIRST {
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
            evidence.colors = image
                .pixels()
                .map(|pixel| pixel.0)
                .collect::<HashSet<_>>()
                .len();
            image.save(path)?;
            return Ok(Some(evidence));
        }
    }
    Ok(None)
}

#[test]
#[ignore = "opt-in real GNOME helper diagnostic; synthetic frames only, no capture or device access"]
fn native_recording_overlay_real_helper_visibility_probe() {
    assert_eq!(
        std::env::var("LAHZA_REAL_OVERLAY_PROBE").as_deref(),
        Ok("1"),
        "set LAHZA_REAL_OVERLAY_PROBE=1 explicitly; use isolated XDG_CONFIG_HOME/XDG_DATA_HOME"
    );
    let lock_state = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.ScreenSaver",
            "--object-path",
            "/org/gnome/ScreenSaver",
            "--method",
            "org.gnome.ScreenSaver.GetActive",
        ])
        .output()
        .expect("could not query GNOME lock state");
    assert!(
        lock_state.status.success(),
        "could not verify GNOME lock state; no probe windows opened"
    );
    assert_eq!(String::from_utf8_lossy(&lock_state.stdout).trim(), "(false,)",
        "GNOME is locked; unlock the desktop before rerunning this opt-in probe. No windows or capture requests were created.");
    eprintln!("overlay probe PID={}", std::process::id());
    let artifacts = std::env::var_os("LAHZA_OVERLAY_PROBE_OUTPUT")
        .map(std::path::PathBuf::from)
        .expect("set LAHZA_OVERLAY_PROBE_OUTPUT to a diagnostic directory");
    std::fs::create_dir_all(&artifacts).unwrap();
    let result = Arc::new(std::sync::Mutex::new(None));
    let completed = result.clone();
    Application::new()
        .with_assets(crate::Assets {
            base: crate::asset_directory(),
        })
        .run(move |cx| {
            // A hidden owner avoids replacing or modifying the user's running
            // editor. Only the two clearly titled, synthetic overlays are shown.
            let launcher = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(0.0), px(0.0)),
                            size(px(430.0), px(610.0)),
                        ))),
                        show: false,
                        focus: false,
                        app_id: Some("com.lahza.OverlayProbe".into()),
                        ..Default::default()
                    },
                    |window, cx| cx.new(|cx| Studio::new(window.window_handle(), None, None, cx)),
                )
                .unwrap();
            let owner = launcher.entity(cx).unwrap();
            let (_pill, camera, frames, exclusions) = owner.update(cx, |studio, cx| {
                studio.load_generation = studio.load_generation.wrapping_add(1);
                studio.loading = None;
                studio.launcher_active = false;
                studio.record_camera = true;
                studio.recording_camera_enabled = true;
                studio.record_microphone = false;
                studio.record_system_audio = false;
                studio.recording_state = RecordingState::Starting;
                studio.recording_busy = true;
                studio.recording_controller =
                    Some(crate::RecordingController::new(crate::NativeRecorder::new()));
                let exclusions = studio.open_recording_overlays(cx).unwrap();
                (
                    studio.recording_pill.unwrap(),
                    studio.recording_camera_window.unwrap(),
                    studio.camera_frames.clone(),
                    exclusions,
                )
            });
            let synthetic = image::RgbaImage::from_fn(320, 180, |x, y| {
                image::Rgba([
                    (30 + x * 180 / 320) as u8,
                    (50 + y * 150 / 180) as u8,
                    180,
                    255,
                ])
            });
            frames.publish(CameraFrame {
                width: 320,
                height: 180,
                rgba: synthetic.clone().into_raw(),
            });
            // Starting intentionally does not consume a live stream. Seed this
            // diagnostic monitor with the same generated frame, retaining its
            // Starting snapshot and disabled transport throughout the probe.
            camera
                .update(cx, |camera, _, cx| {
                    camera.image = Some(cached_render_image(synthetic));
                    camera.image_aspect = 16.0 / 9.0;
                    camera.last_frame = Some(Instant::now());
                    camera.waiting = false;
                    cx.notify();
                })
                .unwrap();
            cx.spawn(async move |cx| {
                Timer::after(Duration::from_millis(600)).await;
                let started = Instant::now();
                loop {
                    Timer::after(Duration::from_millis(100)).await;
                    if exclusions.iter().all(CaptureExclusion::is_protected)
                        || started.elapsed() >= Duration::from_secs(5)
                    {
                        break;
                    }
                }
                let monitors = real_monitors();
                eprintln!("overlay probe real monitors: {monitors:?}");
                let protected: Vec<_> = exclusions
                    .iter()
                    .map(CaptureExclusion::is_protected)
                    .collect();
                let (mut painted, initial) =
                    snapshot_phase("initial", &exclusions, &monitors, &artifacts);
                let mut contained = vec![initial[0].is_some() && initial[0] == initial[1]];
                for (phase, source) in [
                    ("source-unknown", None),
                    (
                        "source-window",
                        Some(CaptureSourceGeometry {
                            position: (120, 100),
                            size: (800, 600),
                            is_monitor: false,
                        }),
                    ),
                ] {
                    owner
                        .update(cx, |studio, cx| {
                            studio.position_recording_overlays(source, cx)
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(150)).await;
                    let (images, locations) =
                        snapshot_phase(phase, &exclusions, &monitors, &artifacts);
                    painted.extend(images);
                    contained.push(locations == initial);
                }
                let minimized = launcher
                    .update(cx, |studio, window, _| {
                        studio.recording_window_minimized = true;
                        window.minimize_window();
                    })
                    .is_ok();
                eprintln!("overlay probe owner minimize requested={minimized}");
                Timer::after(Duration::from_millis(150)).await;
                let (images, locations) =
                    snapshot_phase("owner-minimized", &exclusions, &monitors, &artifacts);
                painted.extend(images);
                contained.push(locations == initial);
                // Exercise the real window-manager movement path using synthetic
                // portal geometry. No portal session or screen capture is opened.
                for (index, monitor) in monitors.iter().enumerate() {
                    owner
                        .update(cx, |studio, cx| {
                            studio.position_recording_overlays(
                                Some(CaptureSourceGeometry {
                                    position: (monitor.x, monitor.y),
                                    size: (monitor.width as i32, monitor.height as i32),
                                    is_monitor: true,
                                }),
                                cx,
                            )
                        })
                        .unwrap();
                    Timer::after(Duration::from_millis(500)).await;
                    let (images, locations) = snapshot_phase(
                        &format!("source-monitor-{index}"),
                        &exclusions,
                        &monitors,
                        &artifacts,
                    );
                    painted.extend(images);
                    contained.push(locations.iter().all(|location| *location == Some(index)));
                }
                // Close only this process's windows and retire only its requests.
                owner
                    .update(cx, |studio, cx| {
                        assert_eq!(studio.recording_state, RecordingState::Starting);
                        assert!(studio.recording_busy);
                        assert!(studio.camera_preview.is_none());
                        studio.close_recording_overlays(cx);
                        studio.recording_controller = None;
                    })
                    .unwrap();
                // GPUI test application globals can retain retired views past
                // run(). Delete only this PID's exact closed-window requests.
                let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into());
                for exclusion in &exclusions {
                    let id = exclusion.title().rsplit(' ').next().unwrap();
                    let directory = runtime.join("lahza-prompter");
                    let request = directory.join(format!("{id}.json"));
                    let data = std::fs::read(&request)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
                    if data.as_ref().is_some_and(|data| {
                        data["pid"].as_u64() == Some(u64::from(std::process::id()))
                            && data["windowTitle"].as_str() == Some(exclusion.title())
                    }) {
                        let _ = std::fs::remove_file(request);
                        let _ = std::fs::remove_file(directory.join(format!("{id}.ready.json")));
                    }
                }
                // Some platforms already remove a never-shown native owner.
                let _ = launcher.update(cx, |_, window, _| window.remove_window());
                *completed.lock().unwrap() = Some((protected, painted, contained));
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
    let (protected, painted, contained) =
        result.lock().unwrap().take().expect("probe did not finish");
    assert!(
        protected.iter().all(|protected| *protected),
        "real helper did not acknowledge both probe windows: {protected:?}"
    );
    assert!(
        painted.iter().all(|painted| *painted),
        "both mapped windows must contain rendered pixels: {painted:?}"
    );
    assert!(contained.iter().all(|contained| *contained),
        "both overlays must fit one real monitor initially and each selected source afterward: {contained:?}");
}
