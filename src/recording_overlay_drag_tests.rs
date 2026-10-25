//! Real server input: GPUI event injection cannot exercise native window moves.
use super::*;
use crate::recording::camera_preview::CameraFrame;
use crate::{asset_directory, open_studio_window, Assets, NativeRecorder, RecordingController};
use gpui::{Application, AsyncApp};
use std::sync::Arc;
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        xinput::{self, ConnectionExt as _},
        xproto::{ConnectionExt as _, *},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

struct NativePointer {
    connection: Arc<RustConnection>,
    root: u32,
    initial: (i16, i16),
    monitor: DesktopRect,
}

impl NativePointer {
    fn trace_devices(connection: &RustConnection, label: &str) {
        if std::env::var_os("LAHZA_DEBUG_OVERLAY_DRAG").is_none() {
            return;
        }
        let Ok(cookie) = connection.xinput_xi_query_device(xinput::Device::ALL) else {
            return;
        };
        let Ok(reply) = cookie.reply() else { return };
        for device in reply.infos {
            for class in device.classes {
                if let xinput::DeviceClassData::Button(buttons) = class.data {
                    eprintln!(
                        "XTEST {label}: device={} name={:?} type={:?} attachment={} enabled={} button_words={:x?}",
                        device.deviceid, String::from_utf8_lossy(&device.name), device.type_,
                        device.attachment, device.enabled, buttons.state,
                    );
                }
            }
        }
    }

    fn trace(&self, label: &str, target: Option<u32>) {
        if std::env::var_os("LAHZA_DEBUG_OVERLAY_DRAG").is_none() {
            return;
        }
        match self.connection.query_pointer(self.root).unwrap().reply() {
            Ok(pointer) => eprintln!(
                "XTEST {label}: root=({}, {}) child={:#x} mask={:?} same_screen={}",
                pointer.root_x, pointer.root_y, pointer.child, pointer.mask, pointer.same_screen,
            ),
            Err(error) => eprintln!("XTEST {label}: root query failed: {error}"),
        }
        if let Some(target) = target {
            if let Ok(pointer) = self.connection.query_pointer(target).unwrap().reply() {
                let attributes = self
                    .connection
                    .get_window_attributes(target)
                    .unwrap()
                    .reply()
                    .ok();
                eprintln!(
                    "XTEST {label}: target={target:#x} origin={:?} local=({}, {}) child={:#x} mask={:?} attrs={:?}",
                    self.origin(target), pointer.win_x, pointer.win_y, pointer.child, pointer.mask,
                    attributes.map(|attributes| (attributes.map_state, attributes.override_redirect)),
                );
            }
        }
    }

    fn new() -> Self {
        let (connection, screen) = x11rb::connect(None).unwrap();
        connection.xtest_get_version(2, 2).unwrap().reply().unwrap();
        connection
            .xinput_xi_query_version(2, 0)
            .unwrap()
            .reply()
            .unwrap();
        let root = connection.setup().roots[screen].root;
        let pointer = connection.query_pointer(root).unwrap().reply().unwrap();
        let monitors = connection
            .randr_get_monitors(root, true)
            .unwrap()
            .reply()
            .unwrap();
        let monitor = monitors
            .monitors
            .iter()
            .find(|monitor| {
                i32::from(pointer.root_x) >= i32::from(monitor.x)
                    && i32::from(pointer.root_x) < i32::from(monitor.x) + i32::from(monitor.width)
                    && i32::from(pointer.root_y) >= i32::from(monitor.y)
                    && i32::from(pointer.root_y) < i32::from(monitor.y) + i32::from(monitor.height)
            })
            .or_else(|| monitors.monitors.iter().find(|monitor| monitor.primary))
            .or_else(|| monitors.monitors.first())
            .map(|monitor| DesktopRect {
                x: i32::from(monitor.x),
                y: i32::from(monitor.y),
                width: u32::from(monitor.width),
                height: u32::from(monitor.height),
            })
            .unwrap_or(DesktopRect {
                x: 0,
                y: 0,
                width: u32::from(connection.setup().roots[screen].width_in_pixels),
                height: u32::from(connection.setup().roots[screen].height_in_pixels),
            });
        if std::env::var_os("LAHZA_DEBUG_OVERLAY_DRAG").is_some() {
            eprintln!(
                "XTEST preflight: root=({}, {}) child={:#x} mask={:?}",
                pointer.root_x, pointer.root_y, pointer.child, pointer.mask
            );
            Self::trace_devices(&connection, "preflight");
        }
        assert!(
            !pointer.mask.contains(KeyButMask::BUTTON1),
            "release the pointer before running this native test"
        );
        Self {
            connection: Arc::new(connection),
            root,
            initial: (pointer.root_x, pointer.root_y),
            monitor,
        }
    }

    fn motion(&self, x: i16, y: i16) {
        self.connection
            .xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, self.root, x, y, 0)
            .unwrap()
            .check()
            .unwrap();
        self.connection.flush().unwrap();
        if std::env::var_os("LAHZA_DEBUG_OVERLAY_DRAG").is_some() {
            eprintln!("XTEST requested motion=({x}, {y})");
        }
        self.trace("motion immediate", None);
    }

    fn button(&self, pressed: bool) {
        self.connection
            .xtest_fake_input(
                if pressed {
                    BUTTON_PRESS_EVENT
                } else {
                    BUTTON_RELEASE_EVENT
                },
                1,
                0,
                self.root,
                0,
                0,
                0,
            )
            .unwrap()
            .check()
            .unwrap();
        self.connection.flush().unwrap();
        self.trace(
            if pressed {
                "press immediate"
            } else {
                "release immediate"
            },
            None,
        );
        Self::trace_devices(&self.connection, if pressed { "press" } else { "release" });
    }

    async fn motion_wait(&self, x: i16, y: i16) {
        // XWayland may forward XTEST through GNOME's asynchronous libei
        // connection. An X roundtrip alone does not mean input was delivered.
        for attempt in 0..100 {
            if attempt % 5 == 0 {
                self.motion(x, y);
            }
            Timer::after(Duration::from_millis(20)).await;
            let pointer = self
                .connection
                .query_pointer(self.root)
                .unwrap()
                .reply()
                .unwrap();
            if (pointer.root_x - x).abs() <= 1 && (pointer.root_y - y).abs() <= 1 {
                return;
            }
        }
        panic!("GNOME did not deliver synthetic pointer movement to ({x}, {y})");
    }

    async fn button_wait(&self, pressed: bool) {
        self.button(pressed);
        for _ in 0..100 {
            Timer::after(Duration::from_millis(20)).await;
            let pointer = self
                .connection
                .query_pointer(self.root)
                .unwrap()
                .reply()
                .unwrap();
            if pointer.mask.contains(KeyButMask::BUTTON1) == pressed {
                settle().await;
                return;
            }
        }
        panic!("GNOME did not deliver synthetic pointer button state {pressed}");
    }

    fn origin(&self, window: u32) -> (i16, i16) {
        let point = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .unwrap()
            .reply()
            .unwrap();
        (point.dst_x, point.dst_y)
    }

    fn point(&self, window: u32, fraction: (f32, f32)) -> (i16, i16) {
        let origin = self.origin(window);
        let geometry = self
            .connection
            .get_geometry(window)
            .unwrap()
            .reply()
            .unwrap();
        (
            origin.0 + (f32::from(geometry.width) * fraction.0).round() as i16,
            origin.1 + (f32::from(geometry.height) * fraction.1).round() as i16,
        )
    }

    fn inward_delta(&self, window: u32) -> (i16, i16) {
        let center = self.point(window, (0.5, 0.5));
        (
            if i32::from(center.0) > self.monitor.x + self.monitor.width as i32 / 2 {
                -64
            } else {
                64
            },
            if i32::from(center.1) > self.monitor.y + self.monitor.height as i32 / 2 {
                -44
            } else {
                44
            },
        )
    }

    fn find_window(&self, title: &str) -> Option<u32> {
        let atom = |name: &[u8]| {
            self.connection
                .intern_atom(false, name)
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let pid_atom = atom(b"_NET_WM_PID");
        let title_atom = atom(b"_NET_WM_NAME");
        let client_list = atom(b"_NET_CLIENT_LIST");
        let mut windows: Vec<_> = self
            .connection
            .get_property(false, self.root, client_list, AtomEnum::WINDOW, 0, 4096)
            .unwrap()
            .reply()
            .unwrap()
            .value32()
            .map(|values| values.collect())
            .unwrap_or_default();
        let mut level = vec![self.root];
        for _ in 0..4 {
            let mut next = Vec::new();
            for parent in level {
                if let Ok(reply) = self.connection.query_tree(parent).unwrap().reply() {
                    next.extend(reply.children);
                }
            }
            windows.extend(next.iter().copied());
            level = next;
        }
        windows.into_iter().find(|window| {
            let Ok(pid) = self
                .connection
                .get_property(false, *window, pid_atom, AtomEnum::CARDINAL, 0, 1)
                .unwrap()
                .reply()
            else {
                return false;
            };
            if pid.value32().and_then(|mut values| values.next()) != Some(std::process::id()) {
                return false;
            }
            self.connection
                .get_property(false, *window, title_atom, AtomEnum::ANY, 0, 1024)
                .unwrap()
                .reply()
                .is_ok_and(|value| value.value == title.as_bytes())
        })
    }

    fn force_configure(&self, window: u32) {
        let geometry = self
            .connection
            .get_geometry(window)
            .unwrap()
            .reply()
            .unwrap();
        let origin = self.origin(window);
        // Bare Xvfb has no WM to send the initial ConfigureNotify that GPUI
        // needs to initialize its drawable. This reports the real geometry.
        let event = ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: window,
            window,
            above_sibling: x11rb::NONE,
            x: origin.0,
            y: origin.1,
            width: geometry.width,
            height: geometry.height,
            border_width: geometry.border_width,
            override_redirect: false,
        };
        self.connection
            .send_event(false, window, EventMask::STRUCTURE_NOTIFY, event)
            .unwrap()
            .check()
            .unwrap();
        self.connection.flush().unwrap();
    }
}

impl Drop for NativePointer {
    fn drop(&mut self) {
        // Restore real desktop input even if an assertion unwinds this task.
        let _ = self
            .connection
            .xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, self.root, 0, 0, 0);
        let _ = self.connection.xtest_fake_input(
            MOTION_NOTIFY_EVENT,
            0,
            0,
            self.root,
            self.initial.0,
            self.initial.1,
            0,
        );
        let _ = self.connection.flush();
    }
}

async fn settle() {
    Timer::after(Duration::from_millis(100)).await;
}

async fn drag(pointer: &NativePointer, window: u32, fraction: (f32, f32), label: &str) {
    let before = pointer.origin(window);
    let start = pointer.point(window, fraction);
    let delta = pointer.inward_delta(window);
    pointer.trace(
        &format!("{label} before drag; requested start={start:?}, delta={delta:?}"),
        Some(window),
    );
    pointer.motion_wait(start.0, start.1).await;
    settle().await;
    pointer.trace("motion settled before press", Some(window));
    pointer.button_wait(true).await;
    settle().await;
    pointer.trace("press settled", Some(window));
    for step in 1..=4 {
        pointer
            .motion_wait(start.0 + delta.0 * step / 4, start.1 + delta.1 * step / 4)
            .await;
        Timer::after(Duration::from_millis(50)).await;
        pointer.trace("drag step settled", Some(window));
    }
    pointer.button_wait(false).await;
    settle().await;
    pointer.trace("release settled", Some(window));
    let after = pointer.origin(window);
    assert!(
        (after.0 - before.0 - delta.0).abs() <= 5,
        "{label} did not move horizontally: {before:?} -> {after:?}, expected {delta:?}"
    );
    assert!(
        (after.1 - before.1 - delta.1).abs() <= 5,
        "{label} did not move vertically: {before:?} -> {after:?}, expected {delta:?}"
    );
    pointer.motion(start.0 + delta.0 + 25, start.1 + delta.1 + 25);
    settle().await;
    assert_eq!(
        pointer.origin(window),
        after,
        "{label} continued moving after release"
    );
}

async fn draw_overlay(window: gpui::AnyWindowHandle, cx: &mut AsyncApp) {
    window
        .update(cx, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    settle().await;
}

#[test]
#[ignore = "requires X11/XWayland and XTEST; run with WAYLAND_DISPLAY unset (Xvfb supported)"]
fn native_pointer_drag_moves_recording_overlays_and_preserves_recording_state() {
    let pointer = NativePointer::new();
    Application::new()
        .with_assets(Assets {
            base: asset_directory(),
        })
        .run(move |cx| {
            let launcher = open_studio_window(cx, false, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            })
            .unwrap();
            let owner = launcher.entity(cx).unwrap();
            let (pill, camera, frames, exclusions, saved_camera) =
                owner.update(cx, |studio, cx| {
                    studio.load_generation = studio.load_generation.wrapping_add(1);
                    studio.loading = None;
                    studio.record_camera = true;
                    studio.recording_camera_enabled = true;
                    studio.recording_state = RecordingState::Starting;
                    studio.recording_busy = true;
                    studio.recording_controller =
                        Some(RecordingController::new(NativeRecorder::new()));
                    let exclusions = studio.open_recording_overlays(cx).unwrap();
                    (
                        studio.recording_pill.unwrap(),
                        studio.recording_camera_window.unwrap(),
                        studio.camera_frames.clone(),
                        exclusions,
                        studio.camera_overlay,
                    )
                });
            cx.spawn(async move |cx| {
                settle().await;
                let pill_title = exclusions[0].title().to_owned();
                let camera_title = exclusions[1].title().to_owned();
                let pill_id = pointer
                    .find_window(&pill_title)
                    .expect("native pill window with exact request title+PID");
                let camera_id = pointer
                    .find_window(&camera_title)
                    .expect("native camera window with exact request title+PID");
                // GPUI's X11 display bounds can describe the entire virtual
                // desktop. Put fixtures inside one real RandR monitor so
                // mixed portrait/landscape arrangements have no dead space.
                move_native_overlays(
                    &pointer.connection,
                    pointer.root,
                    pointer.monitor,
                    &[
                        OverlayPlacementTarget {
                            title: pill_title.clone(),
                            camera: false,
                        },
                        OverlayPlacementTarget {
                            title: camera_title.clone(),
                            camera: true,
                        },
                    ],
                )
                .unwrap();
                settle().await;
                pointer.force_configure(pill_id);
                pointer.force_configure(camera_id);
                settle().await;
                owner
                    .update(cx, |studio, cx| {
                        studio.recording_state = RecordingState::Recording;
                        studio.recording_busy = false;
                        studio.recording_elapsed = Duration::from_secs(65);
                        cx.notify();
                    })
                    .unwrap();
                frames.publish(CameraFrame {
                    width: 320,
                    height: 180,
                    rgba: image::RgbaImage::from_pixel(320, 180, image::Rgba([55, 145, 205, 255]))
                        .into_raw(),
                });
                settle().await;
                draw_overlay(pill.into(), cx).await;
                draw_overlay(camera.into(), cx).await;
                camera
                    .update(cx, |camera, _, _| {
                        assert!(camera.image.is_some());
                        assert_eq!(camera.badge(), None);
                    })
                    .unwrap();

                drag(&pointer, camera_id, (0.5, 0.6), "camera image").await;
                drag(&pointer, pill_id, (0.08, 0.5), "pill grip").await;
                drag(&pointer, pill_id, (0.31, 0.5), "pill timecode").await;

                owner
                    .update(cx, |studio, cx| {
                        studio.recording_state = RecordingState::Paused;
                        cx.notify();
                    })
                    .unwrap();
                settle().await;
                draw_overlay(camera.into(), cx).await;
                camera
                    .update(cx, |camera, _, _| {
                        assert_eq!(camera.badge(), Some("Camera paused"))
                    })
                    .unwrap();
                drag(&pointer, camera_id, (0.5, 0.5), "camera paused badge").await;

                // Release away from the old window before its next drag tick.
                let start = pointer.point(pill_id, (0.3, 0.5));
                pointer.motion(start.0, start.1);
                settle().await;
                pointer.button(true);
                settle().await;
                let away = (
                    (pointer.monitor.x + pointer.monitor.width as i32 / 2) as i16,
                    (pointer.monitor.y + 8) as i16,
                );
                pointer.motion(away.0, away.1);
                pointer.button(false);
                settle().await;
                let released = pointer.origin(pill_id);
                pointer.motion(away.0 + 35, away.1 + 35);
                settle().await;
                assert_eq!(
                    pointer.origin(pill_id),
                    released,
                    "release outside the old pill left dragging active"
                );

                owner
                    .update(cx, |studio, cx| {
                        studio.recording_busy = true; // no recorder action during button hit tests
                        cx.notify();
                    })
                    .unwrap();
                settle().await;
                draw_overlay(pill.into(), cx).await;
                for (fraction, label) in [(0.74, "pause"), (0.895, "stop")] {
                    let before = pointer.origin(pill_id);
                    let start = pointer.point(pill_id, (fraction, 0.5));
                    pointer.motion(start.0, start.1);
                    settle().await;
                    pointer.button(true);
                    settle().await;
                    pointer.motion(start.0 - 30, start.1 - 20);
                    settle().await;
                    pointer.button(false);
                    settle().await;
                    assert_eq!(
                        pointer.origin(pill_id),
                        before,
                        "{label} button incorrectly started a window drag"
                    );
                }

                assert_eq!(pointer.find_window(&pill_title), Some(pill_id));
                assert_eq!(pointer.find_window(&camera_title), Some(camera_id));
                pill.update(cx, |pill, _, _| {
                    assert_eq!(pill.exclusion.title(), pill_title)
                })
                .unwrap();
                camera
                    .update(cx, |camera, _, _| {
                        assert_eq!(camera.exclusion.title(), camera_title)
                    })
                    .unwrap();
                owner
                    .update(cx, |studio, cx| {
                        assert_eq!(
                            studio.camera_overlay, saved_camera,
                            "moving a monitor changed the saved camera composition"
                        );
                        assert_eq!(studio.recording_state, RecordingState::Paused);
                        assert!(studio.recording_controller.is_some());
                        studio.recording_busy = false;
                        studio.recording_state = RecordingState::Idle;
                        studio.record_camera = false;
                        studio.close_recording_overlays(cx);
                    })
                    .unwrap();
                drop(pointer);
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
}
