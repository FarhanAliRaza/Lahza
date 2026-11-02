# Lahza motion project format (version 1)

A project is a folder, conventionally named `*.lahza/`:

```
project.json          settings, audio, assets, scene order
scenes/<id>.json      one scene (its layers, camera, timing)
markers/<track>.json  marker tracks: beat, bar, word, hit, …
assets/               files the project uses
history/<n>.json      edit history (ops and their inverses)
```

All times are **timeline seconds** unless written as a reference string
(see `lahza://catalog` → `time_references`). Indices in references are
1-based. Unknown fields are rejected.

## project.json

```json
{
  "version": 1,
  "settings": {"width": 1920, "height": 1080, "fps": 30, "duration": 20,
               "background": "#0b0b0b", "style": "promo.kinetic",
               "motion_blur": {"shutter": 180, "samples": 8}},
  "audio": {"music": {"path": "assets/music.wav", "at": 0, "trim": 0, "gain_db": 0}},
  "assets": [{"path": "assets/music.wav", "source": "user", "licence": "user-supplied"}],
  "scenes": ["intro", "drop"]
}
```

`fps` is a number or a ratio string (`"30000/1001"`).

## scenes/<id>.json

```json
{
  "id": "intro",
  "start": "bar:1",
  "end": "bar:3",
  "camera": {"zoom": {"keys": [{"at": "bar:1", "v": 1}, {"at": "bar:3", "v": 1.08, "ease": "push"}]}},
  "layers": [ … bottom first … ]
}
```

A scene ends at its `end`, else `start + duration`, else the next scene's
start, else the project end. A cut on a marker leads it by one frame unless
`lead` says otherwise.

## Layers

Common fields: `id`, `kind`, `name`, `transform` (`position`, `anchor`,
`scale`, `rotation`, `opacity`), `time` (`start`, `end`, `duration`,
`step_fps`, `step_phase`, `offset`), `blend`, `effects` (`blur`, `shadow`,
`glow`), `matte` (`{"layer": "<sibling>", "mode": "alpha"}`), `parent`, `follow`, `hidden`.

Kinds: `shape` (`circle`, `rect`, `capsule`, `ring`, `arc`, `polygon`,
`star`, `line`, `path` with SVG `d`), `text`, `image`, `comp`.

```json
{"id": "route", "kind": "shape", "shape": "path", "d": "M -300 0 C -100 -200 100 200 300 0",
 "stroke": {"color": "#ffd23f", "width": 8},
 "trim": {"keys": [{"at": "word:route", "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": "20f", "anchor": "start"}]}}
```

Path strokes take `cap` (`round`, `butt`, `square`), `join` (`round`,
`miter`, `bevel`), `miter_limit`, `dash` (`[dash, gap, …]` px),
`dash_offset`, `feather` (px faded at the far end, so a stroke drawn on
with `trim` has a soft head) and `feather_start` (px faded at the start, a
fading tail):

```json
{"id": "guide", "kind": "shape", "shape": "path", "d": "M 0 450 H 1080",
 "stroke": {"color": "#3b35b0", "width": 3, "cap": "butt", "dash": [14, 12]}}
```

`follow` places a layer on a sibling path layer, evaluated at every
moment (motion-blur samples and stepped time included), from the path as
drawn: its morph, trim and full transform (parents and comps too). `at`
is `"head"` (the trim end, the default: a handle riding a stroke as it
draws on), `"tail"` (the trim start) or a fraction of the full length
(animatable, `{"keys": …}`). `rotate: true` also turns the layer along the
path's direction (added to its `rotation`); `offset` (px) moves it off the
point, along the path's direction frame when rotating. The follower's
`position` and `z` are not used, and it can't also have a `parent`:

```json
{"id": "pen", "kind": "shape", "shape": "ring", "size": 36, "thickness": 8, "fill": "#ffffff",
 "follow": {"layer": "route", "at": "head", "rotate": false, "offset": [0, 0]}}
```

Scenes may arrive with a transition centred on their cut:
`"transition": {"type": "whip", "dur": "8f", "direction": "left"}`
(`fade`, `push`, `slide`, `zoom`, `wipe`, `whip`).

Shapes and text are centred on their origin: `position` places their centre.

```json
{"id": "title", "kind": "text", "text": "MOVE FAST", "size": 160,
 "transform": {"position": [960, 540]},
 "time": {"start": "beat:1"},
 "animators": [{"at": "beat:1", "stagger": "1f", "dur": "8f", "ease": "enter",
                "from": {"y": 80, "opacity": 0}}]}
```

Text styling comes from the glyphs' distance field, so it stays crisp at
any size and per-glyph animators still apply. `stroke` (`color`, `width`
px, `align`: `outside` (default, drawn under every fill so touching
letters share one outline), `center` or `inside`) outlines the glyphs;
a transparent `color` with a stroke gives outline-only type. `weight` (px)
moves the edges outward for a faux bold (negative thins). `edge` (`color`,
`width` px, `angle`: where the light comes from, 0 = above, clockwise)
lays a rim light inside the edges that face the light. All of these are
animatable; weight plus an outside stroke reach at most ~0.11 em past the
outline.

```json
{"id": "logo", "kind": "text", "text": "LAHZA", "size": 180, "color": "#1d2a6b", "weight": 2,
 "stroke": {"color": "#ffffff", "width": 4},
 "edge": {"color": "#ffe9a0", "width": 6, "angle": -40}}
```

An animator's `order` can also come from position: `{"axis": "x"}`
(`"y"`, or the sweep direction in degrees, 0 = rightward, clockwise; with
`"from": "start" | "end" | "center"`) or `{"radial": [0.5, 0.5]}` (a centre
in fractions of the text block). Each unit's delay is its position across
the text (0 for the first, 1 for the last) times `stagger × (units − 1)`.
`"feather": 0.3` turns the opacity change into a soft wipe 30% of the
text wide that travels through the glyphs over the whole animation;
position, scale and rotation still move per unit. One feathered animator
per text layer.

```json
{"id": "reveal", "kind": "text", "text": "SWEEP", "size": 140,
 "animators": [{"at": "beat:1", "order": {"axis": "x", "feather": 0.25},
                "stagger": "2f", "dur": "10f", "from": {"opacity": 0, "y": 20}}]}
```

```json
{"id": "dot", "kind": "shape", "shape": "circle", "size": 120, "fill": "#d7ff3a",
 "transform": {"scale": {"keys": [{"at": "beat:5", "from": 0, "v": 1, "ease": "pop", "anchor": "start"}],
                         "add": [{"type": "drift", "rate": 0.01, "per": "frame", "mode": "mul"}]}}}
```

## Properties

Any animatable field is a plain value or

```json
{"keys": [ {"at": <time>, "v": <value>, "from"?, "ease"?, "anchor"?, "lead"?, "dur"?, "space"?} ],
 "add":  [ {"type": "drift" | "wiggle" | "sine", …} ],
 "step_fps"?: 12}
```

A key is a move that *arrives* at `v`. Without `dur` it runs from the
previous key's end to `at` (a classic keyframe). With `dur` or an `anchor`
other than `end`, it lasts `dur` (default: the ease's natural duration) and
is placed so its anchor moment (`start`, `peak` speed, `land` at 98%, or
`end`) falls on `at + lead`. A move that begins before the previous one
ends takes over from the current value, so motion stays continuous.

Values: numbers; `[x, y]` vectors (scale also takes one number); colours
`"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"`.

## Sounds

`audio` holds the `music`, the narration (`vo`) and sound effects (`sfx`):

```json
"audio": {
  "music": {"path": "assets/track.wav", "at": 0, "gain_db": -2},
  "sfx": [
    {"id": "whoosh1", "path": "assets/sfx/whoosh.wav", "at": "#card.start - 3f", "gain_db": -6},
    {"id": "pop1", "path": "assets/sfx/pop.wav", "at": "beat:9", "length": 0.1}
  ]
}
```

Fields: `path`, `at` (any time reference: an effect anchored to a layer,
`#card.start`, moves when the layer moves), `trim` (seconds skipped in the
file), `gain_db`, `length` (seconds to play), `fade_in` and `fade_out`
(seconds, at the ends of what plays), `name`, `id`. Add one with
`{"op": "add_sound", "path": …, "at": …}` (`"kind": "music"` or `"vo"` for
those); then `set` and `remove` work on its id (`whoosh1.at`), and music and
narration answer to `music` and `vo`. Built-in effects (`sfx_add`):
whoosh, whoosh-out, swipe, pop, click, tick, type, thud, hit, boom, riser,
ding, sparkle, glitch.

## Markers

`markers/<track>.json`:

```json
{"name": "beat", "source": "audio_analyze assets/music.wav",
 "markers": [{"t": 1.093}, {"t": 1.760, "value": 0.8}]}
```

Marker fields: `t`, optional `end`, `label` (words, OCR text), `value`
(strength), `bbox` ([x, y, w, h]).
