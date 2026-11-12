# Reflex 0.11 release reel

A 47-second film for the Reflex 0.11.0 release, built from code and rendered
by the motion engine: score, type, 3D and path-traced lighting are one project.

```bash
REPO=$PWD   # the repository root
mkdir -p /tmp/reflex-reel && cd /tmp/reflex-reel
python3 "$REPO/examples/reflex-0.11/music.py" track.wav   # the score + exact beat/bar/hit/tick markers
python3 "$REPO/examples/reflex-0.11/build.py"             # writes reflex-0.11.lahza/ (add --slang for the Slang shaders)
python3 "$REPO/examples/reflex-0.11/sfx.py" "$(which lahza-motion)" "$PWD/reflex-0.11.lahza"   # sound effects, over MCP
lahza motion check reflex-0.11.lahza
lahza motion render reflex-0.11.lahza -o reflex-0.11.mp4   # path traced (settings.renderer); --raster for a quick pass
```

Needs Python 3 with numpy, scipy, Pillow and fontTools, and the font packages
`fonts-inter` and `fonts-jetbrains-mono` (SIL OFL; copied into the project
with their licences). Path tracing needs a Vulkan device with ray queries; on
Mesa's software Vulkan set `LAHZA_MOTION_SOFTWARE_GPU=1` (about 10 s a frame
at 1080p on four cores).

## Design

- **One subject: the block.** The Reflex mark is five blocks on a 48 px
  grid. One violet block opens the film and swallows the frame, five blocks
  slam into the mark, and every feature shot is built from blocks, balls and
  rings of the same material.
- **Palette**: ink `#0c0c10`, paper `#fcfcfd`, Reflex violet `#6e56cf` and its
  lighter steps. Dark and light stages alternate.
- **Type**: Inter Display for headlines, JetBrains Mono for code. Every line
  of code on screen is real Reflex 0.11 code.
- **Score**: `music.py` synthesises 120 BPM in D minor with impacts at 4, 16
  and 40 s and a stab on each feature cut, so the markers are exact.

| s | scene | what it shows | engine features |
|---|---|---|---|
| 0–4 | open | `import reflex as rx`, one block | mesh, area light, zoom through the block |
| 4–8 | mark | the mark assembles: "Web apps in pure Python." | traced shadow catcher, 3D type cards, motion blur |
| 8–16 | idea | a state class types on; the UI card counts with it | 2.5D comps, follow on a path, text animators |
| 16–20 | 0.11 | the version on a glossy stage | extruded glyphs, chrome parts, depth of field, bloom |
| 20–24 | await | `await State.add(amount)`: the call goes out, the value returns | sweeps with trim and head, emissive |
| 24–28 | client state | `rx.client_state`: a switch flips, the server stays dark | keyed material colour, rack focus depth |
| 28–32 | SetVar | `\|`, `&`, `-`, `^` lift exactly their members | 12 keyed spheres, tori |
| 32–36 | Alembic | revisions drop onto the database | bounce anchored to the beat |
| 36–40 | montage | seven more changes, one per beat | slam, hard cuts |
| 40–47 | end | the mark, `pip install --upgrade reflex` | instancer wave field |

`shaders/reel.slang` holds two Slang functions for `--slang`: `aurora`, the
block-grid backdrop of the 0.11 shot, and `circuit`, a material that cuts the
numerals into the mark's grid with seams that light up.
