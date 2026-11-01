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
`glow`), `matte` (`{"layer": "<sibling>", "mode": "alpha"}`), `parent`, `hidden`.

Kinds: `shape` (`circle`, `rect`, `capsule`, `ring`, `arc`, `polygon`,
`star`, `line`, `path` with SVG `d`), `text`, `image`, `comp`.

```json
{"id": "route", "kind": "shape", "shape": "path", "d": "M -300 0 C -100 -200 100 200 300 0",
 "stroke": {"color": "#ffd23f", "width": 8},
 "trim": {"keys": [{"at": "word:route", "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": "20f", "anchor": "start"}]}}
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

## Markers

`markers/<track>.json`:

```json
{"name": "beat", "source": "audio_analyze assets/music.wav",
 "markers": [{"t": 1.093}, {"t": 1.760, "value": 0.8}]}
```

Marker fields: `t`, optional `end`, `label` (words, OCR text), `value`
(strength), `bbox` ([x, y, w, h]).
