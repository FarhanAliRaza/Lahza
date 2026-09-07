//! Desktop-only, click-through border for an active area recording.
//!
//! Uses X11 or XWayland override-redirect windows so GNOME Wayland can show
//! the border without installing a Shell extension. The windows are outside
//! the encoded crop and never take keyboard or pointer focus.

use super::{area::RecordingArea, input::InputMapping};
use std::time::{Duration, Instant};
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        shape::{ConnectionExt as _, SK, SO},
        xproto::*,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
    COPY_DEPTH_FROM_PARENT,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// Match compositor coordinates to the corresponding X11/XWayland monitor.
/// A single monitor is unambiguous even when fractional scaling changes its
/// XWayland dimensions. Multiple monitors must match position and scale.
fn monitor_rect(mapping: InputMapping, monitors: &[Rect]) -> Option<Rect> {
    if monitors.len() == 1 {
        return Some(monitors[0]);
    }
    let (x, y) = mapping.origin?;
    if mapping.size.0 <= 0.0 || mapping.size.1 <= 0.0 {
        return None;
    }
    let matches: Vec<_> = monitors
        .iter()
        .copied()
        .filter(|monitor| {
            let scale_x = monitor.width as f64 / mapping.size.0;
            let scale_y = monitor.height as f64 / mapping.size.1;
            (scale_x - scale_y).abs() < 0.01
                && (monitor.x as f64 - x * scale_x).abs() < 3.0
                && (monitor.y as f64 - y * scale_y).abs() < 3.0
        })
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

fn crop_rect(area: RecordingArea, monitor: Rect) -> Rect {
    // Round the inner boundary outwards, so even fractional scaling cannot
    // put indicator pixels inside the recorded rectangle.
    let x = monitor.x
        + (area.left as f64 * monitor.width as f64 / area.source_width as f64).floor() as i32;
    let y = monitor.y
        + (area.top as f64 * monitor.height as f64 / area.source_height as f64).floor() as i32;
    let right = monitor.x
        + ((area.left + area.width) as f64 * monitor.width as f64 / area.source_width as f64).ceil()
            as i32;
    let bottom = monitor.y
        + ((area.top + area.height) as f64 * monitor.height as f64 / area.source_height as f64)
            .ceil() as i32;
    Rect {
        x,
        y,
        width: (right - x) as u32,
        height: (bottom - y) as u32,
    }
}

fn border_rects(rect: Rect) -> [Rect; 4] {
    const BORDER: u32 = 3;
    [
        Rect {
            x: rect.x - BORDER as i32,
            y: rect.y - BORDER as i32,
            width: rect.width + 2 * BORDER,
            height: BORDER,
        },
        Rect {
            x: rect.x - BORDER as i32,
            y: rect.y + rect.height as i32,
            width: rect.width + 2 * BORDER,
            height: BORDER,
        },
        Rect {
            x: rect.x - BORDER as i32,
            y: rect.y,
            width: BORDER,
            height: rect.height,
        },
        Rect {
            x: rect.x + rect.width as i32,
            y: rect.y,
            width: BORDER,
            height: rect.height,
        },
    ]
}

pub struct AreaIndicator {
    connection: RustConnection,
    windows: Vec<Window>,
    recording_color: u32,
    paused_color: u32,
    last_raise: Instant,
}

impl AreaIndicator {
    pub fn start(area: RecordingArea, mapping: InputMapping) -> Result<Self> {
        let (connection, screen_index) = x11rb::connect(None)?;
        let screen = &connection.setup().roots[screen_index];
        let root = screen.root;
        let monitors = connection
            .randr_get_monitors(root, true)?
            .reply()?
            .monitors
            .into_iter()
            .map(|m| Rect {
                x: m.x as i32,
                y: m.y as i32,
                width: m.width as u32,
                height: m.height as u32,
            })
            .collect::<Vec<_>>();
        let monitor = monitor_rect(mapping, &monitors)
            .ok_or("could not map the recording monitor to the desktop")?;
        let recording_color = connection
            .alloc_color(screen.default_colormap, 0xffff, 0x3333, 0x4444)?
            .reply()?
            .pixel;
        let paused_color = connection
            .alloc_color(screen.default_colormap, 0xffff, 0xbbbb, 0x2222)?
            .reply()?
            .pixel;
        let rect = crop_rect(area, monitor);
        let mut indicator = Self {
            connection,
            windows: Vec::new(),
            recording_color,
            paused_color,
            last_raise: Instant::now(),
        };
        let conn = &indicator.connection;
        let type_atom = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE")?
            .reply()?
            .atom;
        let notification_atom = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE_NOTIFICATION")?
            .reply()?
            .atom;
        let state_atom = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
        let above_atom = conn
            .intern_atom(false, b"_NET_WM_STATE_ABOVE")?
            .reply()?
            .atom;
        let bypass_atom = conn
            .intern_atom(false, b"_NET_WM_BYPASS_COMPOSITOR")?
            .reply()?
            .atom;
        let extents_atom = conn
            .intern_atom(false, b"_GTK_FRAME_EXTENTS")?
            .reply()?
            .atom;
        for border in border_rects(rect) {
            let window = conn.generate_id()?;
            conn.create_window(
                COPY_DEPTH_FROM_PARENT,
                window,
                root,
                border.x.try_into()?,
                border.y.try_into()?,
                border.width.try_into()?,
                border.height.try_into()?,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .background_pixel(recording_color),
            )?
            .check()?;
            indicator.windows.push(window);
            conn.change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"Lahza recording area",
            )?;
            conn.change_property32(
                PropMode::REPLACE,
                window,
                type_atom,
                AtomEnum::ATOM,
                &[notification_atom],
            )?;
            conn.change_property32(
                PropMode::REPLACE,
                window,
                state_atom,
                AtomEnum::ATOM,
                &[above_atom],
            )?;
            conn.change_property32(
                PropMode::REPLACE,
                window,
                bypass_atom,
                AtomEnum::CARDINAL,
                &[2],
            )?;
            // Mutter treats client-provided frame extents as a request not
            // to add its own shadow, which would bleed into the capture.
            conn.change_property32(
                PropMode::REPLACE,
                window,
                extents_atom,
                AtomEnum::CARDINAL,
                &[0, 0, 0, 0],
            )?;
            // An empty input shape makes every border pixel click-through.
            conn.shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &[],
            )?
            .check()?;
            conn.map_window(window)?.check()?;
            conn.configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
        }
        conn.flush()?;
        Ok(indicator)
    }

    pub fn set_paused(&mut self, paused: bool) {
        let color = if paused {
            self.paused_color
        } else {
            self.recording_color
        };
        for &window in &self.windows {
            let _ = self.connection.change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().background_pixel(color),
            );
            let _ = self.connection.clear_area(false, window, 0, 0, 0, 0);
        }
        let _ = self.connection.flush();
    }

    pub fn tick(&mut self) {
        if self.last_raise.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.last_raise = Instant::now();
        for &window in &self.windows {
            let _ = self.connection.configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            );
        }
        let _ = self.connection.flush();
    }
}

impl Drop for AreaIndicator {
    fn drop(&mut self) {
        for &window in &self.windows {
            let _ = self.connection.destroy_window(window);
        }
        let _ = self.connection.flush();
        // Closing this dedicated connection also removes every window after
        // process exit, without a timer or stale desktop state.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_is_outside_capture_and_maps_scaled_monitors() {
        let area = RecordingArea::from_drag((0.25, 0.25), (0.75, 0.75), (3840, 2160)).unwrap();
        let monitor = Rect {
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
        };
        let rect = crop_rect(area, monitor);
        assert_eq!(
            rect,
            Rect {
                x: 2560,
                y: 360,
                width: 1280,
                height: 720
            }
        );
        for border in border_rects(rect) {
            assert!(
                border.x + border.width as i32 <= rect.x
                    || border.x >= rect.x + rect.width as i32
                    || border.y + border.height as i32 <= rect.y
                    || border.y >= rect.y + rect.height as i32
            );
        }
        let mapping = InputMapping {
            origin: Some((1920.0, 0.0)),
            size: (2560.0, 1440.0),
        };
        let other = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(monitor_rect(mapping, &[other, monitor]), Some(monitor));
        let scaled = Rect {
            x: 3840,
            y: 0,
            width: 5120,
            height: 2880,
        };
        assert_eq!(monitor_rect(mapping, &[other, scaled]), Some(scaled));
        assert_eq!(
            monitor_rect(
                InputMapping {
                    origin: None,
                    ..mapping
                },
                &[other, monitor]
            ),
            None
        );
    }

    #[test]
    #[ignore = "requires X11 or XWayland; run under xvfb-run"]
    fn indicator_is_click_through_changes_color_and_cleans_up() {
        let (observer, index) = x11rb::connect(None).unwrap();
        let screen = &observer.setup().roots[index];
        let monitors = observer
            .randr_get_monitors(screen.root, true)
            .unwrap()
            .reply()
            .unwrap()
            .monitors;
        let monitor = &monitors[0];
        let source = (monitor.width as u32, monitor.height as u32);
        let area = RecordingArea::from_drag((0.25, 0.25), (0.75, 0.75), source).unwrap();
        let mapping = InputMapping {
            origin: Some((monitor.x as f64, monitor.y as f64)),
            size: (source.0 as f64, source.1 as f64),
        };
        let mut indicator = AreaIndicator::start(area, mapping).unwrap();
        let windows = indicator.windows.clone();
        for &window in &windows {
            let shape = observer
                .shape_get_rectangles(window, SK::INPUT)
                .unwrap()
                .reply()
                .unwrap();
            assert!(shape.rectangles.is_empty());
            assert!(
                observer
                    .get_window_attributes(window)
                    .unwrap()
                    .reply()
                    .unwrap()
                    .override_redirect
            );
        }
        let pixel = |connection: &RustConnection, window| {
            connection
                .get_image(ImageFormat::Z_PIXMAP, window, 0, 0, 1, 1, u32::MAX)
                .unwrap()
                .reply()
                .unwrap()
                .data
        };
        let red = pixel(&indicator.connection, windows[0]);
        indicator.set_paused(true);
        let amber = pixel(&indicator.connection, windows[0]);
        assert_ne!(red, amber);
        indicator.set_paused(false);
        assert_eq!(red, pixel(&indicator.connection, windows[0]));
        drop(indicator);
        for window in windows {
            assert!(observer
                .get_window_attributes(window)
                .unwrap()
                .reply()
                .is_err());
        }
    }
}
