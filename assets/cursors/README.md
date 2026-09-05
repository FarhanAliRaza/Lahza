# Cursor artwork

SVG reproductions of the system cursors, used when the editor re-renders the
recorded pointer in a chosen cursor style (Cap does the same).

- `macos/` and `tahoe/`: macOS cursors, taken from
  https://github.com/daviddarnes/mac-cursors via
  https://github.com/CapSoftware/Cap (`crates/cursor-info/assets/mac`).
  Released under the Apple User Agreement; the artwork remains Apple's.
- `windows/`: Windows cursors, taken from
  https://github.com/CapSoftware/Cap (`crates/cursor-info/assets/windows`).
  The artwork remains Microsoft's.

Hotspots (as fractions of the SVG box) live in `src/recording/cursor_assets.rs`.
