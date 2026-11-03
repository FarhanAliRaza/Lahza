# Lahza showreel

A 51-second reel for the motion engine, rendered entirely by it: score,
type, 3D, map, chart and transitions are one project built from code.

```bash
REPO=$PWD   # the repository root
mkdir -p /tmp/reel && cd /tmp/reel
python3 "$REPO/examples/showreel/music.py" track.wav   # the score + exact beat/bar/hit/tick markers
python3 "$REPO/examples/showreel/build.py"             # writes reel.lahza/
lahza motion check reel.lahza
lahza motion sync-check reel.lahza --tracks beat,hit,tick --frames 1
lahza motion render reel.lahza -o lahza-showreel.mp4
```

Needs Python 3 with numpy, scipy and Pillow, and the Debian/Ubuntu font
packages `fonts-inter` and `fonts-jetbrains-mono` (both SIL OFL; the build
copies the faces it uses, with their licences, into the project).
`review.py VIDEO OUT.png T1,T2,…` grabs a labelled contact sheet from the
rendered file, to check cuts and transitions as a viewer sees them.

## Design

- **One protagonist.** The red dot from the wordmark (the record light,
  "the moment") is a clock's pivot, swallows the frame for the title, sits
  inside a glass lens, falls as a ball, pins Karachi on the map, pulses
  alone in the silent beat, and lands back in the wordmark at the end.
- **Palette** from the brand files: ink `#0e0e0d`, paper `#f2eee6`, red
  `#d03734`. One accent.
- **Type**: Inter Display Black for headlines, Light for copy, JetBrains
  Mono for labels and readouts, tracked wide.
- **Grid**: 96 px margins, 12 columns of 122 px with 24 px gutters. A
  screen-space HUD (`"space": "screen"`) is the same in every scene:
  section index, frame counter and a timeline with the dot as playhead.
- **Motion**: exp-lag `glide`/`snap` moves, `slam` on hits, springs only on
  scale and landings, everything anchored to the score's markers.

## Score

`music.py` synthesises the track (120 BPM, A minor, 25 bars) so every
event sits on the grid the edit uses: ticks in the intro, impacts at 4, 16,
34 and 44 s, a drop-out beat before the drop and before the end. It is
mastered to about −14 LUFS with peaks under −1 dBFS.

| s | section | engine features |
|---|---|---|
| 0–3.7 | Moment | radial instancers, a clock hand stepping on the ticks, trim, zoom ease |
| 3.7–8 | Title | per-glyph slam, motion blur, viewfinder brackets |
| 8–14 | Type | glyph/word/feathered/radial/random animators, weight, outline |
| 14–16 | Shape | path morphs, staggered instancer, gradient + feather strokes, dash offset |
| 16–21 | Light | glass refraction, chrome, shadows, depth of field, orbit camera |
| 21–26 | Form | box, revolve, torus spans, extrude, sweep, bounce on the beat |
| 26–34 | Data | native map with arcs, counter, a chart of this soundtrack |
| 34–40 | Sync | 1,100-dot instancer with a wave field per beat |
| 40–44 | Motion | one cut per beat; each word uses the transition it names |
| 44–51 | End | the Urdu wordmark (from `assets/brand`) assembles and draws on |
