# Lahza annotations

`lahza-annotations` is the shared annotation engine used by the screenshot and
video editors. It is a GPUI-based library with no dependency on the Lahza binary,
recording pipeline, filesystem project format, or playback controller.

The crate owns:

- `AnnotationMark`, `AnnotationTiming`, `Tool`, normalized points, and their
  existing serialized representation.
- Arrow curves, pressure-shaped ink, drawn outlines, and hit geometry.
- Bounds snapping, alignment guides, selection handles, and gesture math.
- Text layout, bundled font faces, and a shared font database.
- GPUI painting, SVG fragments, raster export, and blur/pixelation.
- Entrance/exit animation and mapping pinned marks into a visible source region.

The host owns selection state, input dispatch, text input/IME, undo transactions,
file IO, crop/viewport calculation, and video playhead changes. In Lahza these
adapters live in `src/annotations.rs`, `src/timed.rs`, and the editor UI modules.
The host registers `fonts::HANDWRITTEN_FONTS` with GPUI at startup; SVG rendering
and text metrics use the crate's single shared font database. Tool icon paths
are resolved by the host's asset source.

```rust
use lahza_annotations::{AnnotationMark, AnnotationTiming, NormPoint, Tool, svg, timing};

let mark = AnnotationMark {
    tool: Tool::Arrow,
    start: NormPoint { x: 0.1, y: 0.2 },
    end: NormPoint { x: 0.8, y: 0.6 },
    bend: 0.25,
    timing: Some(AnnotationTiming::for_tool(Tool::Arrow, 1.0, 5.0)),
    ..Default::default()
};
let time = mark.timing.unwrap().editing_time(1.0);
let visible = timing::animated_mark(&mark, time).unwrap();
let layer = svg::render_annotations(&[visible], 800, 600).unwrap();
assert_eq!(layer.dimensions(), (800, 600));
```

Run the library tests independently, or check both the library and app:

```sh
cargo test --locked -p lahza-annotations
cargo test --locked --workspace
```

The crate uses the same GPUI/Linux build dependencies as the app. Its tests do
not open a window or require a recording. Geometry fixtures and their generator
live in `tests/fixtures` and `tools`; see the [fixture notes](tests/fixtures/README.md).
Bundled fonts retain their [license and attribution](assets/fonts/README.md).
