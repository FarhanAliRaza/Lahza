# Bundled fonts

The **Hand** annotation style uses Shantell Sans Informal by the Shantell Sans
Project Authors. Regular, bold, italic, and bold italic faces are embedded for
both GPUI and SVG rendering, including exports without system fonts installed.

Font project: https://github.com/arrowtype/shantell-sans
License: SIL Open Font License 1.1; see `OFL-ShantellSans.txt`.

These are the Informal static faces, converted from the reference assets' WOFF2
files to native TTF. The regular/italic source weights are 611 and bold weights
are 800; metadata is mapped to 400/700 to match their normal/bold CSS roles.
The upright faces' slant metadata is set to zero so native renderers distinguish
them from the italic faces, matching the reference's normal/italic CSS roles.
Glyph outlines, spacing, and shaping tables are preserved.

To regenerate with Python and `fonttools[woff]` installed:

```sh
python crates/annotations/tools/import-hand-fonts.py /path/to/reference/assets/fonts
```
