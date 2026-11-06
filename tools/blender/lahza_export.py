"""Blender → Lahza: export models, baked rigid bodies and simulation
sequences as glTF, which `lahza motion model-import` (MCP `model_import`)
brings into a scene as ordinary, editable layers. Lahza renders them
(rasteriser preview, path tracer for export); Blender only models, lights
the look and simulates.

Use from a bpy script (Blender's own Python):

    import sys; sys.path.insert(0, "/path/to/screendrop-rs/tools/blender")
    import lahza_export as lx
    lx.export_model("rock.glb", [rock])                  # static or keyframed objects
    pieces = lx.fracture(rock, 16, focus=hit_point)      # Voronoi pieces, smaller near the hit
    lx.bake_textures(pieces)                             # procedural materials → one shared atlas (glTF carries images only)
    lx.blast(hit_point, 400, frame=1)                    # a short push that breaks them apart
    lx.bake_rigid_bodies(1, 240); lx.export_model("smash.glb", pieces)  # physics → keys
    lx.export_sequence(liquid, "splash_{frame:04}.glb", 1, 48)          # fluid, cloth: one file per frame

Then:

    lahza motion model-import proj.lahza rock.glb --scene s02 --unit 100
    lahza motion model-import proj.lahza "splash_{frame:04}.glb" --scene s02 --frames 48 --fps 24 --at 8.0

Units: Blender metres; `--unit` is px per metre in Lahza (default 100).
Shade objects smooth in Blender if they should look smooth: glTF carries the
normals as authored.
"""
import os
import random

import bmesh
import bpy
import mathutils
from mathutils import Vector
from mathutils.bvhtree import BVHTree


def _select_only(objects):
    bpy.ops.object.select_all(action="DESELECT")
    for o in objects:
        o.select_set(True)
    if objects:
        bpy.context.view_layer.objects.active = objects[0]


def _principled_extras(objects):
    """What glTF has no field for goes into each material's extras under
    "lahza", which `model-import` reads: the Principled BSDF's subsurface
    scattering (its scale in metres)."""
    for o in objects:
        for slot in getattr(o, "material_slots", []):
            m = slot.material
            p = _principled(m)
            if p is None or "Subsurface Weight" not in p.inputs:
                continue
            weight = p.inputs["Subsurface Weight"].default_value
            if weight <= 0 or p.inputs["Subsurface Weight"].is_linked:
                m.pop("lahza", None)
                continue
            m["lahza"] = {"subsurface": float(weight), "subsurface_radius": [float(v) for v in p.inputs["Subsurface Radius"].default_value],
                          "subsurface_scale_m": float(p.inputs["Subsurface Scale"].default_value),
                          "subsurface_anisotropy": float(p.inputs["Subsurface Anisotropy"].default_value)}


def export_model(path, objects=None, animations=True):
    """Exports `objects` (default: every visible mesh) to a .glb: modifiers
    applied, textures embedded, keyframed transforms as glTF animation,
    subsurface scattering in the materials' extras."""
    objects = objects if objects is not None else [o for o in bpy.context.scene.objects if o.type == "MESH" and o.visible_get()]
    _principled_extras(objects)
    _select_only(objects)
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    bpy.ops.export_scene.gltf(filepath=os.path.abspath(path), export_format="GLB", use_selection=True, export_apply=True,
                              export_animations=animations, export_yup=True, export_cameras=False, export_lights=False, export_extras=True)
    return path


def bake_rigid_bodies(frame_start, frame_end, objects=None):
    """Runs the rigid-body simulation and turns it into keyframes on each
    object (so the motion travels as glTF animation, editable in Lahza).
    Does what Blender's Bake To Keyframes does without its UI-only keying
    operator, so it works in background mode (blender -b)."""
    scene = bpy.context.scene
    scene.frame_start, scene.frame_end = frame_start, frame_end
    objects = objects if objects is not None else [o for o in scene.objects if o.rigid_body is not None]
    objects = [o for o in objects if o.rigid_body is not None and o.rigid_body.type == "ACTIVE"]
    frames = range(frame_start, frame_end + 1)
    poses = []
    for f in frames:
        scene.frame_set(f)
        poses.append([o.matrix_world.copy() for o in objects])
    _select_only(objects)
    bpy.ops.rigidbody.objects_remove()
    prefs = bpy.context.preferences.edit
    old = prefs.keyframe_new_interpolation_type
    prefs.keyframe_new_interpolation_type = "LINEAR"
    try:
        for o in objects:
            o.rotation_mode = "QUATERNION"
        for f, pose in zip(frames, poses):
            for o, m in zip(objects, pose):
                if o.parent:
                    m = o.matrix_parent_inverse.inverted() @ o.parent.matrix_world.inverted() @ m
                q = m.to_quaternion()
                if o.rotation_quaternion.dot(q) < 0:
                    q.negate()
                o.location, o.rotation_quaternion = m.to_translation(), q
                o.keyframe_insert("location", frame=f)
                o.keyframe_insert("rotation_quaternion", frame=f)
    finally:
        prefs.keyframe_new_interpolation_type = old
    scene.frame_set(frame_start)
    return objects


def bake_fluid(domain):
    """Bakes a Mantaflow domain (liquid or smoke) for the scene's frame range."""
    _select_only([domain])
    with bpy.context.temp_override(active_object=domain, object=domain, selected_objects=[domain]):
        bpy.ops.fluid.bake_all()


def export_sequence(obj, pattern, frame_start, frame_end):
    """One .glb per frame of `obj` as evaluated (a baked liquid surface,
    cloth, any deforming mesh): `pattern` holds {frame} or {frame:04}."""
    # The glTF exporter does not apply simulation results (a fluid domain
    # exports as its box), so each frame's evaluated mesh is copied into a
    # plain temporary object and that is exported.
    scene = bpy.context.scene
    written = []
    for f in range(frame_start, frame_end + 1):
        scene.frame_set(f)
        depsgraph = bpy.context.evaluated_depsgraph_get()
        mesh = bpy.data.meshes.new_from_object(obj.evaluated_get(depsgraph), preserve_all_data_layers=True, depsgraph=depsgraph)
        frame_obj = bpy.data.objects.new(f"{obj.name}_frame", mesh)
        frame_obj.matrix_world = obj.matrix_world.copy()
        scene.collection.objects.link(frame_obj)
        path = pattern.replace("{frame:04}", f"{f:04d}").replace("{frame}", str(f))
        _principled_extras([frame_obj])
        _select_only([frame_obj])
        os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
        bpy.ops.export_scene.gltf(filepath=os.path.abspath(path), export_format="GLB", use_selection=True, export_apply=False,
                                  export_animations=False, export_yup=True, export_cameras=False, export_lights=False, export_extras=True)
        bpy.data.objects.remove(frame_obj)
        bpy.data.meshes.remove(mesh)
        written.append(path)
    return written


def _inside(bvh, p):
    loc, normal, _, _ = bvh.find_nearest(p)
    return loc is not None and (p - loc).dot(normal) < 0


def fracture(obj, pieces=12, seed=0, focus=None, focus_share=0.5, focus_radius=None, inner_material=None):
    """Breaks a closed mesh into Voronoi pieces (as Cell Fracture does):
    each cell of random points inside the mesh, intersected with it, becomes
    an object with its origin at its centre of volume; `obj` is hidden.
    With `focus` (a world point, such as where a hammer lands), a
    `focus_share` of the points crowd within `focus_radius` of it, so the
    pieces there come out smaller. Cut faces get `inner_material` (default
    the object's own material)."""
    rnd = random.Random(seed)
    depsgraph = bpy.context.evaluated_depsgraph_get()
    base = bpy.data.meshes.new_from_object(obj.evaluated_get(depsgraph))
    base.transform(obj.matrix_world)
    bvh = BVHTree.FromPolygons([v.co for v in base.vertices], [p.vertices for p in base.polygons])
    lo = Vector([min(v.co[k] for v in base.vertices) for k in range(3)])
    hi = Vector([max(v.co[k] for v in base.vertices) for k in range(3)])
    size = hi - lo
    radius = focus_radius or 0.35 * min(size)
    points, tries = [], 0
    while len(points) < pieces and tries < pieces * 1000:
        tries += 1
        if focus is not None and len(points) < round(pieces * focus_share):
            d = Vector([rnd.gauss(0, 1) for _ in range(3)]).normalized()
            p = Vector(focus) + d * radius * rnd.random() ** (1 / 3)
        else:
            p = Vector([rnd.uniform(lo[k], hi[k]) for k in range(3)])
        if _inside(bvh, p) and all((p - q).length > 1e-4 for q in points):
            points.append(p)
    inner = inner_material or obj.active_material
    pad = size.length * 0.05 + 1e-3
    collection = obj.users_collection[0] if obj.users_collection else bpy.context.scene.collection
    out = []
    for i, p in enumerate(points):
        # Planes (a, b, c, d), inside where a·x + b·y + c·z + d <= 0, in
        # coordinates relative to the cell's point: the padded bounds, then
        # half way to every other point.
        planes = [Vector((1, 0, 0, -(hi.x - p.x + pad))), Vector((-1, 0, 0, lo.x - p.x - pad)),
                  Vector((0, 1, 0, -(hi.y - p.y + pad))), Vector((0, -1, 0, lo.y - p.y - pad)),
                  Vector((0, 0, 1, -(hi.z - p.z + pad))), Vector((0, 0, -1, lo.z - p.z - pad))]
        for j, q in enumerate(points):
            if j != i:
                n = q - p
                length = n.length
                n.normalize()
                planes.append(Vector((n.x, n.y, n.z, -length / 2)))
        corners, _ = mathutils.geometry.points_in_planes(planes)
        if len(corners) < 4:
            continue
        cb = bmesh.new()
        for c in corners:
            cb.verts.new(p + c)
        bmesh.ops.remove_doubles(cb, verts=cb.verts[:], dist=1e-6)
        hull = bmesh.ops.convex_hull(cb, input=cb.verts[:])
        loose = list({g for g in hull["geom_interior"] + hull["geom_unused"] if isinstance(g, bmesh.types.BMVert)})
        bmesh.ops.delete(cb, geom=loose, context="VERTS")
        cell_mesh = bpy.data.meshes.new("cell")
        cb.to_mesh(cell_mesh)
        cb.free()
        if inner is not None:
            cell_mesh.materials.append(inner)
        cell = bpy.data.objects.new("cell", cell_mesh)
        collection.objects.link(cell)
        piece = bpy.data.objects.new(f"{obj.name}_piece{i:02d}", base.copy())
        collection.objects.link(piece)
        cut = piece.modifiers.new("cut", "BOOLEAN")
        cut.operation, cut.solver, cut.object, cut.material_mode = "INTERSECT", "EXACT", cell, "TRANSFER"
        depsgraph = bpy.context.evaluated_depsgraph_get()
        mesh = bpy.data.meshes.new_from_object(piece.evaluated_get(depsgraph))
        old = piece.data
        piece.modifiers.clear()
        piece.data = mesh
        bpy.data.meshes.remove(old)
        bpy.data.objects.remove(cell)
        bpy.data.meshes.remove(cell_mesh)
        if not mesh.polygons:
            bpy.data.objects.remove(piece)
            bpy.data.meshes.remove(mesh)
            continue
        out.append(piece)
    bpy.data.meshes.remove(base)
    _select_only(out)
    bpy.ops.object.origin_set(type="ORIGIN_CENTER_OF_VOLUME")
    obj.hide_set(True)
    obj.hide_render = True
    return out


def blast(origin, strength, frame, frames=2, radius=1.0, falloff=0.0):
    """A short push away from `origin` (a hammer blow, an explosion): a
    force field of `strength` on rigid bodies within `radius` metres, on
    for `frames` frames from `frame`, off before and after; with `falloff`
    p it weakens as 1 / distance^p (from 2 cm). A field pushes every body
    with the same force, so light pieces fly much faster than heavy ones:
    give the pieces similar masses for an even burst (Blender rigid bodies
    take no starting velocity, not even from released keyframes)."""
    bpy.ops.object.effector_add(type="FORCE", location=origin)
    e = bpy.context.object
    e.name = "blast"
    f = e.field
    f.shape, f.falloff_type, f.falloff_power = "POINT", "SPHERE", falloff
    f.use_max_distance, f.distance_max = True, radius
    f.use_min_distance, f.distance_min = falloff > 0, 0.02
    prefs = bpy.context.preferences.edit
    old = prefs.keyframe_new_interpolation_type
    prefs.keyframe_new_interpolation_type = "CONSTANT"
    try:
        # The step into frame f reads the field at f: on for the steps
        # into frame + 1 … frame + frames.
        for at, s in ((frame, 0.0), (frame + 1, strength), (frame + frames + 1, 0.0)):
            f.strength = s
            e.keyframe_insert("field.strength", frame=at)
    finally:
        prefs.keyframe_new_interpolation_type = old
    return e


def _use_gpu():
    prefs = bpy.context.preferences.addons["cycles"].preferences
    for kind in ("OPTIX", "CUDA", "HIP", "ONEAPI", "METAL"):
        try:
            prefs.compute_device_type = kind
        except TypeError:
            continue
        prefs.get_devices()
        devices = [d for d in prefs.devices if d.type == kind]
        if devices:
            for d in devices:
                d.use = True
            bpy.context.scene.cycles.device = "GPU"
            return
    bpy.context.scene.cycles.device = "CPU"


def _principled(material):
    return next((n for n in material.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None) if material and material.node_tree else None


def bake_textures(objects, size=2048, normal_size=None, samples=4, margin=8):
    """Turns the objects' materials into image maps glTF can carry:
    procedural node trees (noise, veins, bump) are baked by Cycles into one
    shared atlas, a base-colour map (`size` px), a tangent-space normal map
    (`normal_size`, default `size`) and, when a material's roughness is
    driven by nodes, a roughness map, on fresh UVs packed across all the
    objects; call once per group that should share an atlas (the pieces of
    one fractured object, say). Each material becomes one baked material
    using the maps, keeping its roughness (if constant), metallic and
    emission values.
    Bake with the objects where their patterns should line up: world-space
    patterns stay continuous across the pieces."""
    scene = bpy.context.scene
    engine = scene.render.engine
    scene.render.engine = "CYCLES"
    _use_gpu()
    scene.cycles.samples = samples
    for obj in objects:
        if obj.data.users > 1:
            obj.data = obj.data.copy()
        if not obj.data.uv_layers:
            obj.data.uv_layers.new(name="UVMap")
    _select_only(objects)
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.uv.smart_project(angle_limit=1.15, island_margin=0.003)
    bpy.ops.object.mode_set(mode="OBJECT")
    name = objects[0].name
    color = bpy.data.images.new(f"{name}_color", size, size)
    normal = bpy.data.images.new(f"{name}_normal", normal_size or size, normal_size or size)
    normal.colorspace_settings.name = "Non-Color"
    # One bake copy of each material, its image node active (Cycles bakes
    # into the active image node of each object's materials).
    copies = {}
    for obj in objects:
        for slot in obj.material_slots:
            m = slot.material
            if m is None:
                continue
            if m.name not in copies:
                c = m.copy()
                pc = _principled(c)
                if pc is not None and "Subsurface Weight" in pc.inputs:
                    # Surface data only: what scatters inside is the renderer's.
                    pc.inputs["Subsurface Weight"].default_value = 0.0
                node = c.node_tree.nodes.new("ShaderNodeTexImage")
                node.image = color
                c.node_tree.nodes.active = node
                copies[m.name] = (m, c, node)
            slot.material = copies[m.name][1]
    _select_only(objects)
    bpy.ops.object.bake(type="DIFFUSE", pass_filter={"COLOR"}, margin=margin, use_clear=True)
    for _, _, node in copies.values():
        node.image = normal
    bpy.ops.object.bake(type="NORMAL", normal_space="TANGENT", margin=margin, use_clear=True)
    rough = None
    if any(_principled(c) is not None and _principled(c).inputs["Roughness"].is_linked for _, c, _ in copies.values()):
        rough = bpy.data.images.new(f"{name}_roughness", size, size)
        rough.colorspace_settings.name = "Non-Color"
        for _, _, node in copies.values():
            node.image = rough
        bpy.ops.object.bake(type="ROUGHNESS", margin=margin, use_clear=True)
        rough.pack()
    color.pack()
    normal.pack()
    baked_of = {}
    for name, (orig, c, _) in copies.items():
        src = _principled(orig)
        baked = bpy.data.materials.new(name + "_baked")
        if baked.node_tree is None:
            baked.use_nodes = True
        nt = baked.node_tree
        p = _principled(baked)
        for key in ("Roughness", "Metallic", "Emission Color", "Emission Strength", "IOR",
                    "Subsurface Weight", "Subsurface Radius", "Subsurface Scale", "Subsurface Anisotropy"):
            if src is not None and key in src.inputs and key in p.inputs:
                p.inputs[key].default_value = src.inputs[key].default_value
        tc = nt.nodes.new("ShaderNodeTexImage")
        tc.image = color
        nt.links.new(tc.outputs["Color"], p.inputs["Base Color"])
        tn = nt.nodes.new("ShaderNodeTexImage")
        tn.image = normal
        nm = nt.nodes.new("ShaderNodeNormalMap")
        nt.links.new(tn.outputs["Color"], nm.inputs["Color"])
        nt.links.new(nm.outputs["Normal"], p.inputs["Normal"])
        if rough is not None and src is not None and src.inputs["Roughness"].is_linked:
            tr = nt.nodes.new("ShaderNodeTexImage")
            tr.image = rough
            nt.links.new(tr.outputs["Color"], p.inputs["Roughness"])
        baked_of[c.name] = baked
    for obj in objects:
        for slot in obj.material_slots:
            if slot.material is not None:
                slot.material = baked_of[slot.material.name]
    for _, c, _ in copies.values():
        bpy.data.materials.remove(c)
    scene.render.engine = engine
    return objects
