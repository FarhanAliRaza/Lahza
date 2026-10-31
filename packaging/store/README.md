# Snap Store screenshots

These four real application screenshots show what is new in Lahza 2.0:

| File | Feature |
| --- | --- |
| `screenshots/01-glass-loupe.png` | Glass loupe zoom with loupe size and zoom level |
| `screenshots/02-camera-split-9x16.png` | 9:16 split screen: the recording fills its half |
| `screenshots/03-enhance-audio.png` | Enhance audio (RE-USE) with the cleaned waveform |
| `screenshots/04-upscale-zoom.png` | Upscale zoomed footage on a 3.3× zoom |

They were captured on 2026-10-31 from a private headless GNOME Shell session
with one virtual 1920 × 1080 monitor, driven and captured through Mutter's
RemoteDesktop and ScreenCast APIs. The Lahza window is cropped to its
1440 × 900 bounds. The screen recording is GNOME Settings, recorded in the same
virtual session. The camera bubble is covered with a cartoon placeholder
(pasted over the captured frame, matching its rounded corners).

Store images are RGB PNGs under 2 MB each at **1440 × 900** (aspect 1.6,
inside the Store's 1:2–2:1 range), with no padding or upscaling. README images
in `docs/screenshots/` are 1280 px wide (Lanczos), plus the camera layout
animation GIF and two before/after strips (upscaler, Enhance audio). These are
documentation assets, outside the application's bundled `assets/` directory.

The [Store media API](https://dashboard.snapcraft.io/docs/reference/v1/snap.html)
validates image dimensions, aspect ratios, and file sizes during upload.

To update manually, open the [Lahza listing editor](https://snapcraft.io/lahza/listing),
upload the four files in numeric order in the screenshots section, and save.
Screenshot uploads are separate from releasing a new snap; `snapcraft
upload-metadata` only handles summary, description, and icon.
