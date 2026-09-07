# Snap Store screenshots

These four real application screenshots are ordered to show Lahza's workflow:

| File | Feature | Original capture (2026-09-07) |
| --- | --- | --- |
| `screenshots/01-screenshot-styling.png` | Wallpaper backgrounds and image appearance | `14-54-03.png` |
| `screenshots/02-motion-and-captions.png` | Motion presets, 3D tilt, and timed captions | `15-17-52.png` |
| `screenshots/03-video-and-camera.png` | Video timeline, camera overlay, pointer controls, and audio | `15-29-38.png` |
| `screenshots/04-export-formats.png` | MP4, WebM, GIF, resolution, and frame rate | `15-30-04.png` |

Sources were selected from `Screenshot From 2026-09-07 <time>.png` in the
maintainer's Pictures/Screenshots directory. Originals are untouched.

Store images are RGB PNGs under 2 MB each, preserving the captures' natural
proportions without upscaling. The styling and motion images are
**1440 × 937** (the transparent outer margin is removed). Neither has added
padding. The Store requires aspect ratios between 1:2 and 2:1, so the
3440 × 1408 video/export captures have 156 pixels of neutral padding above
and below, producing **3440 × 1720** store images. Controls and timelines remain visible.
README copies in `docs/screenshots/` are **1280 pixels wide**, proportionally
downscaled with Lanczos resampling from the unpadded captures and linked to
the larger store files. These
are documentation assets, outside the application's bundled `assets/` directory.

The [Store media API](https://dashboard.snapcraft.io/docs/reference/v1/snap.html)
validates image dimensions, aspect ratios, and file sizes during upload.

To update manually, open the [Lahza listing editor](https://snapcraft.io/lahza/listing),
upload the four files in numeric order in the screenshots section, and save.
Screenshot uploads are separate from releasing a new snap; `snapcraft
upload-metadata` only handles summary, description, and icon.
