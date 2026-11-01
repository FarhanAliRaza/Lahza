"""Test shot: a Lahza recording on a lit ultrawide monitor, the camera dollies
in and racks focus onto a real click. Run:
  blender -b --factory-startup -P shot.py -- <shot_dir> [preview]
"""
import bpy, json, math, os, sys
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1:]
DIR = argv[0]
PREVIEW = len(argv) > 1 and argv[1] == "preview"
FPS, FRAMES, SOURCE_START = 30, 150, 1.5
ev = json.load(open(os.path.join(DIR, "events.json")))

# ---------------------------------------------------------------- scene reset
for o in list(bpy.data.objects):
    bpy.data.objects.remove(o)
scene = bpy.context.scene
scene.frame_start, scene.frame_end = 1, FRAMES
scene.render.fps = FPS

def src_frame(t):  # source seconds -> shot frame
    return (t - SOURCE_START) * FPS + 1

def mat(name, **inputs):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes["Principled BSDF"]
    for key, value in inputs.items():
        bsdf.inputs[key].default_value = value
    return m, bsdf

def box(name, size, location, material, bevel=0.0):
    bpy.ops.mesh.primitive_cube_add(location=location)
    o = bpy.context.object
    o.name = name
    o.scale = (size[0] / 2, size[1] / 2, size[2] / 2)
    bpy.ops.object.transform_apply(scale=True)
    if bevel:
        mod = o.modifiers.new("bevel", "BEVEL")
        mod.width, mod.segments = bevel, 6
        bpy.ops.object.shade_smooth()
    o.data.materials.append(material)
    return o

# ---------------------------------------------------------------- dimensions
DESK_Z = 0.75
SCREEN_W = 0.80
SCREEN_H = SCREEN_W * 1440 / 3440
SCREEN_Z = DESK_Z + 0.33           # screen centre height
PANEL_Y = 0.0                      # monitor front plane, facing -Y

def screen_point(nx, ny, lift=0.0006):
    return Vector(((nx - 0.5) * SCREEN_W, PANEL_Y - lift, SCREEN_Z + (0.5 - ny) * SCREEN_H))

# ---------------------------------------------------------------- materials
shell, _ = mat("shell", **{"Base Color": (0.018, 0.018, 0.02, 1), "Roughness": 0.38, "Metallic": 0.6})
alu, _ = mat("aluminium", **{"Base Color": (0.55, 0.56, 0.58, 1), "Roughness": 0.22, "Metallic": 1.0})
keys, _ = mat("keys", **{"Base Color": (0.03, 0.03, 0.035, 1), "Roughness": 0.55})

desk = bpy.data.materials.new("desk"); desk.use_nodes = True
nt = desk.node_tree; b = nt.nodes["Principled BSDF"]
wave = nt.nodes.new("ShaderNodeTexWave"); wave.wave_type = "BANDS"; wave.bands_direction = "X"
wave.inputs["Scale"].default_value = 1.6; wave.inputs["Distortion"].default_value = 6.0
wave.inputs["Detail"].default_value = 3.0
ramp = nt.nodes.new("ShaderNodeValToRGB")
ramp.color_ramp.elements[0].color = (0.035, 0.018, 0.010, 1)
ramp.color_ramp.elements[1].color = (0.11, 0.058, 0.030, 1)
nt.links.new(wave.outputs["Fac"], ramp.inputs["Fac"])
nt.links.new(ramp.outputs["Color"], b.inputs["Base Color"])
b.inputs["Roughness"].default_value = 0.32
b.inputs["Coat Weight"].default_value = 0.35
b.inputs["Coat Roughness"].default_value = 0.08

# Screen: the recording is the emission, under a thin glossy coat.
screen_mat = bpy.data.materials.new("screen"); screen_mat.use_nodes = True
nt = screen_mat.node_tree; b = nt.nodes["Principled BSDF"]
img = bpy.data.images.load(os.path.join(DIR, "frames", "0001.jpg"))
img.source = "SEQUENCE"
tex = nt.nodes.new("ShaderNodeTexImage"); tex.image = img; tex.interpolation = "Cubic"
tex.image_user.frame_duration = FRAMES; tex.image_user.frame_start = 1
tex.image_user.use_auto_refresh = True
nt.links.new(tex.outputs["Color"], b.inputs["Emission Color"])
b.inputs["Emission Strength"].default_value = 4.0
b.inputs["Base Color"].default_value = (0.0, 0.0, 0.0, 1)
b.inputs["Roughness"].default_value = 0.35
b.inputs["Coat Weight"].default_value = 1.0
b.inputs["Coat Roughness"].default_value = 0.04

# ---------------------------------------------------------------- geometry
bpy.ops.mesh.primitive_plane_add(size=12, location=(0, 0, DESK_Z))
bpy.context.object.data.materials.append(desk)

box("panel", (SCREEN_W + 0.012, 0.022, SCREEN_H + 0.014), (0, PANEL_Y + 0.011, SCREEN_Z), shell, bevel=0.004)
bpy.ops.mesh.primitive_plane_add(size=1, location=(0, PANEL_Y - 0.0002, SCREEN_Z), rotation=(math.pi / 2, 0, 0))
scr = bpy.context.object; scr.name = "screen"
scr.scale = (SCREEN_W, SCREEN_H, 1); bpy.ops.object.transform_apply(scale=True)
scr.data.materials.append(screen_mat)
box("neck", (0.07, 0.025, 0.30), (0, 0.10, DESK_Z + 0.16), alu, bevel=0.008)
box("base", (0.26, 0.20, 0.008), (0, 0.10, DESK_Z + 0.004), alu, bevel=0.003)
box("keyboard", (0.44, 0.135, 0.016), (-0.03, -0.33, DESK_Z + 0.008), keys, bevel=0.005)
bpy.ops.mesh.primitive_uv_sphere_add(radius=1, location=(0.29, -0.33, DESK_Z + 0.014))
mouse = bpy.context.object; mouse.scale = (0.032, 0.058, 0.016)
bpy.ops.object.shade_smooth(); mouse.data.materials.append(keys)

# ---------------------------------------------------------------- cursor from input.json
cur_mat = bpy.data.materials.new("cursor"); cur_mat.use_nodes = True
nt = cur_mat.node_tree; b = nt.nodes["Principled BSDF"]
ct = nt.nodes.new("ShaderNodeTexImage"); ct.image = bpy.data.images.load(os.path.join(DIR, "cursor.png"))
nt.links.new(ct.outputs["Color"], b.inputs["Emission Color"])
nt.links.new(ct.outputs["Alpha"], b.inputs["Alpha"])
b.inputs["Emission Strength"].default_value = 4.0
b.inputs["Base Color"].default_value = (0, 0, 0, 1)
cw = ev["ref"][0] * SCREEN_W                     # cursor size on the panel
ax, ay = ev["anchor"]["x"], ev["anchor"]["y"]
bpy.ops.mesh.primitive_plane_add(size=1)
cursor = bpy.context.object; cursor.name = "cursor"
for v in cursor.data.vertices:                   # hotspot at the object origin
    v.co.x = (v.co.x + 0.5 - ax) * cw
    v.co.y = (v.co.y - 0.5 + ay) * cw
    v.co = Vector((v.co.x, 0.0, v.co.y))
cursor.data.materials.append(cur_mat)
travel = ev["travel"]
def cursor_at(t):
    if t <= travel[0]["time"]: return travel[0]["x"], travel[0]["y"]
    for a, c in zip(travel, travel[1:]):
        if a["time"] <= t <= c["time"]:
            k = (t - a["time"]) / max(c["time"] - a["time"], 1e-6)
            return a["x"] + (c["x"] - a["x"]) * k, a["y"] + (c["y"] - a["y"]) * k
    return travel[-1]["x"], travel[-1]["y"]
for f in range(1, FRAMES + 1):
    cursor.location = screen_point(*cursor_at(SOURCE_START + (f - 1) / FPS), lift=0.0008)
    cursor.keyframe_insert("location", frame=f)
for fc in cursor.animation_data.action.fcurves if hasattr(cursor.animation_data.action, "fcurves") else []:
    for k in fc.keyframe_points: k.interpolation = "LINEAR"

# ---------------------------------------------------------------- click ripple
click = ev["click"]; click_f = src_frame(click["time"])
cp = screen_point(click["x"], click["y"], lift=0.001)
ring_mat = bpy.data.materials.new("ripple"); ring_mat.use_nodes = True
rb = ring_mat.node_tree.nodes["Principled BSDF"]
rb.inputs["Base Color"].default_value = (0, 0, 0, 1)
rb.inputs["Emission Color"].default_value = (0.55, 0.78, 1.0, 1)
bpy.ops.mesh.primitive_torus_add(major_radius=1, minor_radius=0.035, location=cp, rotation=(math.pi / 2, 0, 0))
ring = bpy.context.object; ring.name = "ripple"; ring.data.materials.append(ring_mat)
bpy.ops.object.shade_smooth()
strength = rb.inputs["Emission Strength"]
for f, s, e in ((click_f - 0.5, 0.0005, 0.0), (click_f, 0.002, 14.0), (click_f + 12, 0.028, 0.0)):
    ring.scale = (s, s, s); ring.keyframe_insert("scale", frame=f)
    strength.default_value = e; strength.keyframe_insert("default_value", frame=f)
for f, hidden in ((1, True), (click_f - 1, False), (click_f + 12, True)):
    ring.hide_render = hidden; ring.keyframe_insert("hide_render", frame=f)

# ---------------------------------------------------------------- lights and world
world = bpy.data.worlds.new("night"); scene.world = world; world.use_nodes = True
world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.004, 0.005, 0.009, 1)
def area(name, loc, rot, energy, color, size):
    bpy.ops.object.light_add(type="AREA", location=loc, rotation=rot)
    l = bpy.context.object; l.name = name
    l.data.energy, l.data.color, l.data.size = energy, color, size
    return l
area("key", (-1.6, -1.2, 2.2), (math.radians(55), 0, math.radians(-50)), 55, (1.0, 0.82, 0.62), 1.2)
area("rim", (0.9, 0.9, 1.6), (math.radians(-60), 0, math.radians(150)), 70, (0.55, 0.7, 1.0), 0.8)

# ---------------------------------------------------------------- camera
bpy.ops.object.empty_add(location=(0, PANEL_Y, SCREEN_Z)); aim = bpy.context.object; aim.name = "aim"
bpy.ops.object.empty_add(location=(-0.03, -0.27, DESK_Z + 0.02)); focus = bpy.context.object; focus.name = "focus"
bpy.ops.object.camera_add(location=(-0.55, -1.45, 1.18)); cam = bpy.context.object
scene.camera = cam
track = cam.constraints.new("TRACK_TO"); track.target = aim
track.track_axis, track.up_axis = "TRACK_NEGATIVE_Z", "UP_Y"
cam.data.lens = 38
cam.data.dof.use_dof = True
cam.data.dof.focus_object = focus
cam.data.dof.aperture_fstop = 1.8
cam.data.dof.aperture_blades = 7

end_cam = cp + Vector((-0.07, -0.36, 0.035))
for f, loc in ((1, cam.location.copy()), (FRAMES, end_cam)):
    cam.location = loc; cam.keyframe_insert("location", frame=f)
for f, loc in ((1, Vector((0, PANEL_Y, SCREEN_Z))), (click_f + 10, cp), (FRAMES, cp)):
    aim.location = loc; aim.keyframe_insert("location", frame=f)
for f, lens in ((1, 38), (FRAMES, 52)):
    cam.data.lens = lens; cam.data.keyframe_insert("lens", frame=f)
# Focus rests on the keyboard, then racks to the click just before it lands.
for f, loc in ((1, focus.location.copy()), (click_f - 22, focus.location.copy()), (click_f - 2, cp)):
    focus.location = loc; focus.keyframe_insert("location", frame=f)

# ---------------------------------------------------------------- render settings
prefs = bpy.context.preferences.addons["cycles"].preferences
prefs.compute_device_type = "OPTIX"; prefs.get_devices()
for d in prefs.devices: d.use = d.type == "OPTIX"
scene.render.engine = "CYCLES"
scene.cycles.device = "GPU"
scene.cycles.samples = 24 if PREVIEW else 64
scene.cycles.use_denoising = True
scene.render.resolution_x, scene.render.resolution_y = 1920, 1080
scene.render.resolution_percentage = 50 if PREVIEW else 100
scene.render.use_motion_blur = not PREVIEW
scene.render.motion_blur_shutter = 0.5
scene.view_settings.view_transform = "AgX"
for look in ("AgX - Medium High Contrast", "Medium High Contrast"):
    try: scene.view_settings.look = look; break
    except TypeError: pass
scene.render.image_settings.file_format = "PNG"
scene.render.filepath = os.path.join(DIR, "preview" if PREVIEW else "render", "f_####")
