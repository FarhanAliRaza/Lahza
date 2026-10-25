//! Dragging for non-activating recording windows. Mutter deliberately refuses
//! interactive WM drags for notification windows; ordinary placement requests
//! still work and preserve their capture protection and stacking behavior.
use gpui::{App, MouseButton, MouseDownEvent, Pixels, Point, Window};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use x11rb::{connection::Connection, protocol::xproto::*, rust_connection::RustConnection};

const DRAG_INTERVAL: Duration = Duration::from_millis(16);
type DragResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
pub(crate) struct OverlayDrag {
    generation: Arc<AtomicU64>,
}

impl OverlayDrag {
    pub(crate) fn begin(
        &mut self,
        title: &str,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(test)]
        if std::env::var_os("LAHZA_DEBUG_OVERLAY_DRAG").is_some() {
            eprintln!(
                "overlay drag begin: {} {:?} scale {} compositor {}",
                title,
                event.position,
                window.scale_factor(),
                gpui::guess_compositor()
            );
        }
        self.end();
        if event.button != MouseButton::Left {
            return;
        }
        if gpui::guess_compositor() != "X11" {
            window.start_window_move();
            return;
        }
        let anchor = device_anchor(event.position, window.scale_factor());
        let title = title.to_owned();
        let generation = self.generation.clone();
        let token = generation.load(Ordering::Acquire);
        // Connect and inspect native windows off the UI thread. The local
        // mouse-down anchor, rather than a later pointer sample, preserves
        // any movement made while this worker is being scheduled.
        cx.background_executor()
            .spawn(async move {
                let drag = match NativeDrag::open(&title) {
                    Ok(drag) => drag,
                    Err(error) => {
                        eprintln!("Could not start overlay drag: {error}");
                        return;
                    }
                };
                drag.run(anchor, &generation, token);
            })
            .detach();
    }

    pub(crate) fn end(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for OverlayDrag {
    fn drop(&mut self) {
        self.end();
    }
}

fn device_anchor(position: Point<Pixels>, scale: f32) -> (i32, i32) {
    (
        (f32::from(position.x) * scale).round() as i32,
        (f32::from(position.y) * scale).round() as i32,
    )
}

fn drag_origin(pointer: (i16, i16), anchor: (i32, i32)) -> (i32, i32) {
    (
        i32::from(pointer.0) - anchor.0,
        i32::from(pointer.1) - anchor.1,
    )
}

fn find_owned_window(
    connection: &RustConnection,
    root: u32,
    title: &str,
) -> DragResult<Option<u32>> {
    let pid_atom = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
    let title_atom = connection
        .intern_atom(false, b"_NET_WM_NAME")?
        .reply()?
        .atom;
    let clients_atom = connection
        .intern_atom(false, b"_NET_CLIENT_LIST")?
        .reply()?
        .atom;
    let mut candidates: Vec<_> = connection
        .get_property(false, root, clients_atom, AtomEnum::WINDOW, 0, 4096)?
        .reply()?
        .value32()
        .map(|windows| windows.collect())
        .unwrap_or_default();
    // Notifications need not appear in the WM client list; include the
    // native tree, with enough depth for reparenting window managers.
    let mut level = vec![root];
    for _ in 0..3 {
        let mut next = Vec::new();
        for parent in level {
            if let Ok(reply) = connection.query_tree(parent)?.reply() {
                next.extend(reply.children);
            }
        }
        candidates.extend(next.iter().copied());
        level = next;
    }
    candidates.sort_unstable();
    candidates.dedup();
    for candidate in candidates {
        let Ok(pid) = connection
            .get_property(false, candidate, pid_atom, AtomEnum::CARDINAL, 0, 1)?
            .reply()
        else {
            continue;
        };
        if pid.value32().and_then(|mut values| values.next()) != Some(std::process::id()) {
            continue;
        }
        let Ok(name) = connection
            .get_property(false, candidate, title_atom, AtomEnum::ANY, 0, 1024)?
            .reply()
        else {
            continue;
        };
        if name.value == title.as_bytes() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

struct NativeDrag {
    connection: RustConnection,
    root: u32,
    window: u32,
    move_atom: u32,
    managed_move: bool,
}

impl NativeDrag {
    fn open(title: &str) -> DragResult<Self> {
        let (connection, index) = x11rb::connect(None)?;
        let root = connection.setup().roots[index].root;
        let window = find_owned_window(&connection, root, title)?
            .ok_or("recording overlay is no longer available")?;
        let move_atom = connection
            .intern_atom(false, b"_NET_MOVERESIZE_WINDOW")?
            .reply()?
            .atom;
        let supported = connection
            .intern_atom(false, b"_NET_SUPPORTED")?
            .reply()?
            .atom;
        let managed_move = connection
            .get_property(false, root, supported, AtomEnum::ATOM, 0, 4096)?
            .reply()?
            .value32()
            .is_some_and(|mut atoms| atoms.any(|atom| atom == move_atom))
            && !connection
                .get_window_attributes(window)?
                .reply()?
                .override_redirect;
        Ok(Self {
            connection,
            root,
            window,
            move_atom,
            managed_move,
        })
    }

    fn move_to(&self, origin: (i32, i32)) -> DragResult<()> {
        if self.managed_move {
            // NorthWest gravity, x/y present, source=application. This is a
            // placement request, not the interactive _NET_WM_MOVERESIZE
            // operation that Mutter disables for notification windows.
            let event = ClientMessageEvent::new(
                32,
                self.window,
                self.move_atom,
                [
                    1 | (1 << 8) | (1 << 9) | (1 << 12),
                    origin.0 as u32,
                    origin.1 as u32,
                    0,
                    0,
                ],
            );
            self.connection
                .send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    event,
                )?
                .check()?;
        } else {
            self.connection
                .configure_window(
                    self.window,
                    &ConfigureWindowAux::new().x(origin.0).y(origin.1),
                )?
                .check()?;
        }
        self.connection.flush()?;
        Ok(())
    }

    fn run(&self, anchor: (i32, i32), generation: &AtomicU64, token: u64) {
        let mut previous = None;
        while generation.load(Ordering::Acquire) == token {
            let started = Instant::now();
            // Query the target, not just the root: destruction then ends
            // the worker immediately, even if the pointer stays pressed.
            let pointer = match self.connection.query_pointer(self.window) {
                Ok(cookie) => match cookie.reply() {
                    Ok(pointer) => pointer,
                    Err(_) => break,
                },
                Err(_) => break,
            };
            if !pointer.same_screen || !pointer.mask.contains(KeyButMask::BUTTON1) {
                break;
            }
            let origin = drag_origin((pointer.root_x, pointer.root_y), anchor);
            #[cfg(test)]
            if std::env::var_os("LAHZA_DEBUG_OVERLAY_DRAG").is_some() {
                eprintln!(
                    "overlay drag sample: window={} root=({}, {}) mask={:?} target={:?} managed={}",
                    self.window,
                    pointer.root_x,
                    pointer.root_y,
                    pointer.mask,
                    origin,
                    self.managed_move
                );
            }
            // A new press/release may have arrived while QueryPointer was
            // in flight. Never let that superseded worker issue a new move.
            if generation.load(Ordering::Acquire) != token {
                break;
            }
            if previous != Some(origin) {
                if self.move_to(origin).is_err() {
                    break;
                }
                previous = Some(origin);
            }
            std::thread::sleep(DRAG_INTERVAL.saturating_sub(started.elapsed()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_preserves_the_grab_point_and_motion_during_worker_startup() {
        let anchor = device_anchor(gpui::point(gpui::px(24.0), gpui::px(12.0)), 1.5);
        assert_eq!(anchor, (36, 18));
        assert_eq!(drag_origin((1036, 618), anchor), (1000, 600));
        assert_eq!(drag_origin((1116, 658), anchor), (1080, 640));
        assert_eq!(drag_origin((-124, -42), anchor), (-160, -60));
    }

    #[test]
    fn release_and_view_drop_invalidate_workers() {
        let mut drag = OverlayDrag::default();
        let generation = drag.generation.clone();
        let first = generation.load(Ordering::Acquire);
        drag.end();
        assert_ne!(generation.load(Ordering::Acquire), first);
        let next = generation.load(Ordering::Acquire);
        drop(drag);
        assert_ne!(generation.load(Ordering::Acquire), next);
    }

    #[test]
    #[ignore = "requires X11 or XWayland; run under Xvfb"]
    fn native_drag_target_lookup_and_move_only_touch_the_exact_owned_window() {
        use x11rb::wrapper::ConnectionExt as _;
        let (connection, index) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[index].root;
        let atom = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let pid_atom = atom(b"_NET_WM_PID");
        let title_atom = atom(b"_NET_WM_NAME");
        let utf8 = atom(b"UTF8_STRING");
        let window_type = atom(b"_NET_WM_WINDOW_TYPE");
        let notification_type = atom(b"_NET_WM_WINDOW_TYPE_NOTIFICATION");
        let title = format!("Lahza drag target {}", uuid::Uuid::new_v4());
        let create = |name: &str, pid: u32, notification: bool| {
            let window = connection.generate_id().unwrap();
            connection
                .create_window(
                    x11rb::COPY_DEPTH_FROM_PARENT,
                    window,
                    root,
                    20,
                    30,
                    100,
                    80,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    0,
                    &CreateWindowAux::new().override_redirect(u32::from(!notification)),
                )
                .unwrap()
                .check()
                .unwrap();
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    pid_atom,
                    AtomEnum::CARDINAL,
                    &[pid],
                )
                .unwrap();
            connection
                .change_property8(PropMode::REPLACE, window, title_atom, utf8, name.as_bytes())
                .unwrap();
            if notification {
                connection
                    .change_property32(
                        PropMode::REPLACE,
                        window,
                        window_type,
                        AtomEnum::ATOM,
                        &[notification_type],
                    )
                    .unwrap()
                    .check()
                    .unwrap();
            }
            connection.map_window(window).unwrap().check().unwrap();
            window
        };
        let target = create(&title, std::process::id(), false);
        let foreign = create(&title, std::process::id() + 1, false);
        let unrelated = create("Unrelated Lahza window", std::process::id(), false);
        let focus = connection.get_input_focus().unwrap().reply().unwrap().focus;
        let drag = NativeDrag::open(&title).unwrap();
        assert_eq!(drag.window, target);
        drag.move_to((240, 360)).unwrap();
        let origin = |window| {
            let geometry = connection.get_geometry(window).unwrap().reply().unwrap();
            (geometry.x, geometry.y)
        };
        assert_eq!(origin(target), (240, 360));
        assert_eq!(origin(foreign), (20, 30));
        assert_eq!(origin(unrelated), (20, 30));
        // Exercise the exact notification type Mutter refuses to move
        // interactively, as well as unmanaged windows on bare Xvfb.
        let notification_title = format!("Lahza drag notification {}", uuid::Uuid::new_v4());
        let notification = create(&notification_title, std::process::id(), true);
        std::thread::sleep(Duration::from_millis(200));
        let notification_drag = NativeDrag::open(&notification_title).unwrap();
        notification_drag.move_to((420, 280)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let location = connection
                .translate_coordinates(notification, root, 0, 0)
                .unwrap()
                .reply()
                .unwrap();
            if (location.dst_x, location.dst_y) == (420, 280) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "notification move was ignored: ({}, {})",
                location.dst_x,
                location.dst_y
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            connection.get_input_focus().unwrap().reply().unwrap().focus,
            focus
        );
        for window in [target, foreign, unrelated, notification] {
            connection.destroy_window(window).unwrap().check().unwrap();
        }
    }
}
