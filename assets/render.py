"""Render the boards and piece sprites kess draws, with Blender in the background.

blender --background --factory-startup --python assets/render.py -- --out assets --samples 96
"""

import argparse
import json
import math
import sys
from collections.abc import Callable
from pathlib import Path

import bmesh
import bpy
from bpy_extras.object_utils import world_to_camera_view
from mathutils import Euler, Vector

Color = tuple[float, float, float, float]

WIDTH, HEIGHT = 2560, 1600
TEXTURE = 2048
# the board and its frame, edge to edge, in board units
BOARD_SPAN = 9.5
STYLES = ("amber", "glass", "metal")
PIECES = ("p", "n", "b", "r", "q", "k")
SEGMENTS = 72


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="assets")
    parser.add_argument("--samples", type=int, default=96)
    parser.add_argument("--styles", default=",".join(STYLES))
    parser.add_argument("--kinds", default="".join(PIECES))
    parser.add_argument("--only", choices=("boards", "pieces", "all"), default="all")
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    return parser.parse_args(argv)


def reset_scene(samples: int) -> bpy.types.Scene:
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.samples = samples
    scene.cycles.use_denoising = True
    scene.cycles.max_bounces = 16
    scene.cycles.transmission_bounces = 16
    scene.cycles.transparent_max_bounces = 16
    scene.render.resolution_x = WIDTH
    scene.render.resolution_y = HEIGHT
    scene.render.resolution_percentage = 100
    preferences = bpy.context.preferences.addons["cycles"].preferences
    preferences.compute_device_type = "METAL"
    preferences.get_devices()
    for device in preferences.devices:
        device.use = True
    scene.cycles.device = "GPU"
    return scene


def node_material(name: str) -> tuple[bpy.types.Material, bpy.types.bpy_prop_collection, bpy.types.bpy_prop_collection, bpy.types.Node]:
    material = bpy.data.materials.new(name)
    material.use_nodes = True
    nodes = material.node_tree.nodes
    principled = nodes["Principled BSDF"]
    return material, nodes, material.node_tree.links, principled


def set_inputs(principled: bpy.types.Node, **values: float) -> None:
    for key, value in values.items():
        principled.inputs[key.replace("_", " ")].default_value = value


def noise(nodes: bpy.types.bpy_prop_collection, scale: float, detail: float = 8.0, distortion: float = 2.0) -> bpy.types.Node:
    texture = nodes.new("ShaderNodeTexNoise")
    texture.inputs["Scale"].default_value = scale
    texture.inputs["Detail"].default_value = detail
    texture.inputs["Distortion"].default_value = distortion
    return texture


def ramp(nodes: bpy.types.bpy_prop_collection, links: bpy.types.bpy_prop_collection, source: bpy.types.NodeSocket, stops: list[tuple[float, Color]]) -> bpy.types.Node:
    color_ramp = nodes.new("ShaderNodeValToRGB")
    elements = color_ramp.color_ramp.elements
    elements[0].position, elements[0].color = stops[0]
    elements[1].position, elements[1].color = stops[-1]
    for position, color in stops[1:-1]:
        element = elements.new(position)
        element.color = color
    links.new(source, color_ramp.inputs["Fac"])
    return color_ramp


def amber(name: str, light: Color, dark: Color, depth: float) -> bpy.types.Material:
    material, nodes, links, principled = node_material(name)
    set_inputs(
        principled,
        Roughness=0.04,
        IOR=1.54,
        Transmission_Weight=0.35,
        Subsurface_Weight=1.0,
        Subsurface_Scale=depth,
        Coat_Weight=1.0,
        Coat_Roughness=0.02,
    )
    principled.inputs["Subsurface Radius"].default_value = (1.0, 0.45, 0.12)
    streaks = noise(nodes, 2.2, 12.0, 6.0)
    colors = ramp(nodes, links, streaks.outputs["Fac"], [(0.3, light), (0.55, dark), (0.75, light)])
    links.new(colors.outputs["Color"], principled.inputs["Base Color"])
    return material


def glass(name: str, roughness: float, tint: Color) -> bpy.types.Material:
    material, _, _, principled = node_material(name)
    set_inputs(principled, Base_Color=tint, Roughness=roughness, IOR=1.5, Transmission_Weight=1.0)
    return material


def metal(name: str, color: Color, roughness: float) -> bpy.types.Material:
    material, _, _, principled = node_material(name)
    set_inputs(principled, Base_Color=color, Metallic=1.0, Roughness=roughness)
    return material


def lacquer(name: str, base: Color, vein: Color, roughness: float = 0.12) -> bpy.types.Material:
    material, nodes, links, principled = node_material(name)
    set_inputs(principled, Roughness=roughness, Coat_Weight=1.0, Coat_Roughness=0.02)
    veins = noise(nodes, 4.0, 10.0, 3.0)
    colors = ramp(nodes, links, veins.outputs["Fac"], [(0.4, base), (0.5, vein), (0.6, base)])
    links.new(colors.outputs["Color"], principled.inputs["Base Color"])
    return material


def emission(name: str, color: Color, strength: float) -> bpy.types.Material:
    material, nodes, links, principled = node_material(name)
    nodes.remove(principled)
    emit = nodes.new("ShaderNodeEmission")
    emit.inputs["Color"].default_value = color
    emit.inputs["Strength"].default_value = strength
    links.new(emit.outputs["Emission"], nodes["Material Output"].inputs["Surface"])
    return material


def rgba(red: float, green: float, blue: float) -> Color:
    return (red, green, blue, 1.0)


PIECE_LOOKS: dict[str, dict[str, Callable[[], bpy.types.Material]]] = {
    "amber": {
        "w": lambda: amber("amber-honey", rgba(0.8, 0.42, 0.03), rgba(0.6, 0.25, 0.01), 0.05),
        "b": lambda: amber("amber-cherry", rgba(0.12, 0.015, 0.002), rgba(0.05, 0.006, 0.001), 0.04),
    },
    "glass": {
        "w": lambda: glass("glass-clear", 0.0, rgba(1.0, 1.0, 1.0)),
        "b": lambda: glass("glass-smoked", 0.12, rgba(0.08, 0.08, 0.09)),
    },
    "metal": {
        "w": lambda: metal("silver", rgba(0.95, 0.95, 0.97), 0.08),
        "b": lambda: metal("gold", rgba(1.0, 0.68, 0.22), 0.12),
    },
}

BOARD_LOOKS: dict[str, Callable[[], tuple[bpy.types.Material, bpy.types.Material, bpy.types.Material]]] = {
    "amber": lambda: (
        lacquer("honey", rgba(0.7, 0.4, 0.06), rgba(0.95, 0.68, 0.18), 0.06),
        lacquer("cognac", rgba(0.06, 0.015, 0.002), rgba(0.28, 0.08, 0.008), 0.06),
        lacquer("frame", rgba(0.05, 0.02, 0.01), rgba(0.2, 0.08, 0.02), 0.05),
    ),
    "glass": lambda: (
        lacquer("white", rgba(0.85, 0.85, 0.85), rgba(0.95, 0.95, 0.95), 0.03),
        lacquer("black", rgba(0.004, 0.004, 0.004), rgba(0.02, 0.02, 0.02), 0.03),
        lacquer("resin", rgba(0.12, 0.08, 0.05), rgba(0.3, 0.2, 0.12), 0.04),
    ),
    "metal": lambda: (
        lacquer("ivory", rgba(0.75, 0.72, 0.62), rgba(0.85, 0.82, 0.72), 0.1),
        lacquer("burl", rgba(0.18, 0.18, 0.18), rgba(0.35, 0.35, 0.34), 0.1),
        lacquer("steel", rgba(0.03, 0.03, 0.035), rgba(0.12, 0.12, 0.13), 0.06),
    ),
}


def catmull(points: list[tuple[float, float]], steps: int = 6) -> list[tuple[float, float]]:
    out = []
    padded = [points[0], *points, points[-1]]
    for index in range(1, len(padded) - 2):
        p0, p1, p2, p3 = (Vector(padded[index + offset]) for offset in (-1, 0, 1, 2))
        for step in range(steps):
            t = step / steps
            out.append(
                0.5
                * (
                    2 * p1
                    + (p2 - p0) * t
                    + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t * t
                    + (3 * p1 - p0 - 3 * p2 + p3) * t * t * t
                )
            )
    out.append(Vector(points[-1]))
    return [(max(point.x, 0.0), point.y) for point in out]


def arc(center_z: float, radius: float, start: float, end: float, steps: int = 10) -> list[tuple[float, float]]:
    return [
        (radius * math.sin(angle), center_z - radius * math.cos(angle))
        for angle in (start + (end - start) * step / steps for step in range(steps + 1))
    ]


def base(height: float = 0.2) -> list[tuple[float, float]]:
    return [
        (0.36, 0.0),
        (0.37, 0.04),
        (0.355, 0.08),
        (0.31, 0.1),
        (0.3, 0.13),
        (0.24, height * 0.85),
        (0.2, height),
    ]


PROFILES: dict[str, list[tuple[float, float]]] = {
    "p": base() + [(0.13, 0.42), (0.21, 0.47), (0.2, 0.52), (0.1, 0.56)] + arc(0.71, 0.17, 0.5, math.pi),
    "b": base()
    + [(0.12, 0.6), (0.22, 0.66), (0.21, 0.71), (0.12, 0.74), (0.19, 0.86), (0.21, 0.98), (0.16, 1.12)]
    + [(0.07, 1.2), (0.05, 1.23)]
    + arc(1.3, 0.07, 0.3, math.pi, 6),
    "r": base(0.22) + [(0.2, 0.65), (0.26, 0.72), (0.27, 0.95), (0.27, 1.05), (0.0, 1.05)],
    "q": base(0.24)
    + [(0.13, 0.85), (0.24, 0.93), (0.22, 0.98), (0.15, 1.02), (0.27, 1.34), (0.24, 1.38), (0.12, 1.4)]
    + arc(1.5, 0.09, 0.4, math.pi, 8),
    "k": base(0.24)
    + [(0.14, 0.95), (0.26, 1.03), (0.24, 1.08), (0.16, 1.12), (0.27, 1.45), (0.24, 1.5), (0.1, 1.55)]
    + [(0.08, 1.6), (0.0, 1.6)],
    "n": base(0.22) + [(0.24, 0.32), (0.27, 0.38), (0.0, 0.38)],
}

KNIGHT_HEAD: list[tuple[float, float]] = [
    (-0.24, 0.38),
    (0.26, 0.38),
    (0.22, 0.6),
    (0.12, 0.8),
    (0.1, 0.95),
    (0.3, 0.85),
    (0.4, 0.92),
    (0.36, 1.08),
    (0.14, 1.28),
    (0.06, 1.38),
    (-0.02, 1.3),
    (-0.14, 1.24),
    (-0.24, 1.0),
    (-0.28, 0.7),
]


def lathe(name: str, profile: list[tuple[float, float]]) -> bpy.types.Object:
    points = catmull(profile)
    mesh = bpy.data.meshes.new(name)
    built = bmesh.new()
    rings = []
    for radius, z in points:
        ring = [
            built.verts.new((radius * math.cos(angle), radius * math.sin(angle), z))
            for angle in (2 * math.pi * step / SEGMENTS for step in range(SEGMENTS))
        ]
        rings.append(ring)
    for lower, upper in zip(rings, rings[1:]):
        for step in range(SEGMENTS):
            following = (step + 1) % SEGMENTS
            built.faces.new((lower[step], lower[following], upper[following], upper[step]))
    built.faces.new(list(reversed(rings[0])))
    built.faces.new(rings[-1])
    bmesh.ops.remove_doubles(built, verts=built.verts, dist=1e-5)
    built.to_mesh(mesh)
    built.free()
    for polygon in mesh.polygons:
        polygon.use_smooth = True
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    return obj


def cutter(location: tuple[float, float, float], scale: tuple[float, float, float]) -> bpy.types.Object:
    bpy.ops.mesh.primitive_cube_add(location=location, scale=scale)
    return bpy.context.active_object


def carve(target: bpy.types.Object, tool: bpy.types.Object) -> None:
    modifier = target.modifiers.new("carve", "BOOLEAN")
    modifier.operation = "DIFFERENCE"
    modifier.object = tool
    bpy.context.view_layer.objects.active = target
    bpy.ops.object.modifier_apply(modifier=modifier.name)
    bpy.data.objects.remove(tool)


def join(target: bpy.types.Object, *parts: bpy.types.Object) -> bpy.types.Object:
    for obj in bpy.context.selected_objects:
        obj.select_set(False)
    for part in (target, *parts):
        part.select_set(True)
    bpy.context.view_layer.objects.active = target
    bpy.ops.object.join()
    return target


def sphere(location: tuple[float, float, float], radius: float) -> bpy.types.Object:
    bpy.ops.mesh.primitive_uv_sphere_add(location=location, radius=radius, segments=32, ring_count=16)
    obj = bpy.context.active_object
    bpy.ops.object.shade_smooth()
    return obj


def knight_head() -> bpy.types.Object:
    mesh = bpy.data.meshes.new("head")
    built = bmesh.new()
    face = built.faces.new([built.verts.new((0.0, y, z)) for y, z in KNIGHT_HEAD])
    extruded = bmesh.ops.extrude_face_region(built, geom=[face])
    moved = [element for element in extruded["geom"] if isinstance(element, bmesh.types.BMVert)]
    bmesh.ops.translate(built, verts=moved, vec=(0.24, 0.0, 0.0))
    bmesh.ops.translate(built, verts=built.verts, vec=(-0.12, 0.0, 0.0))
    built.to_mesh(mesh)
    built.free()
    head = bpy.data.objects.new("head", mesh)
    bpy.context.collection.objects.link(head)
    bevel = head.modifiers.new("bevel", "BEVEL")
    bevel.width, bevel.segments = 0.07, 4
    smooth = head.modifiers.new("smooth", "SUBSURF")
    smooth.levels = smooth.render_levels = 2
    bpy.context.view_layer.objects.active = head
    for modifier in list(head.modifiers):
        bpy.ops.object.modifier_apply(modifier=modifier.name)
    bpy.ops.object.shade_smooth()
    head.rotation_euler = Euler((0.0, 0.0, math.radians(-50)))
    return head


def build_piece(kind: str) -> bpy.types.Object:
    body = lathe(kind, PROFILES[kind])
    if kind == "r":
        # six notches around the rim, each cut only through the wall
        for step in range(6):
            angle = step * math.pi / 3
            tool = cutter((0.24 * math.cos(angle), 0.24 * math.sin(angle), 1.05), (0.07, 0.045, 0.11))
            tool.rotation_euler = Euler((0.0, 0.0, angle))
            carve(body, tool)
        carve(body, lathe("hollow", [(0.19, 0.97), (0.19, 1.2), (0.0, 1.2)]))
    if kind == "b":
        tool = cutter((0.08, 0.0, 1.02), (0.25, 0.02, 0.09))
        tool.rotation_euler = Euler((0.0, math.radians(35), math.radians(40)))
        carve(body, tool)
    if kind == "q":
        beads = [
            sphere((0.25 * math.cos(angle), 0.25 * math.sin(angle), 1.38), 0.045)
            for angle in (2 * math.pi * step / 9 for step in range(9))
        ]
        join(body, *beads)
    if kind == "k":
        bpy.ops.mesh.primitive_cube_add(location=(0.0, 0.0, 1.76), scale=(0.035, 0.035, 0.17))
        upright = bpy.context.active_object
        bpy.ops.mesh.primitive_cube_add(location=(0.0, 0.0, 1.8), scale=(0.12, 0.035, 0.035))
        join(body, upright, bpy.context.active_object)
    if kind == "n":
        join(body, knight_head())
    return body


def place_camera(scene: bpy.types.Scene) -> bpy.types.Object:
    camera_data = bpy.data.cameras.get("camera") or bpy.data.cameras.new("camera")
    camera_data.lens = 40
    camera = bpy.data.objects.get("camera") or bpy.data.objects.new("camera", camera_data)
    if camera.name not in scene.collection.objects:
        scene.collection.objects.link(camera)
    camera.location = (0.0, -11.6, 9.6)
    direction = Vector((0.0, -0.5, 0.0)) - camera.location
    camera.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()
    scene.camera = camera
    bpy.context.view_layer.update()
    return camera


def light_rig(scene: bpy.types.Scene) -> None:
    world = bpy.data.worlds.new("world")
    world.use_nodes = True
    background = world.node_tree.nodes["Background"]
    background.inputs["Color"].default_value = rgba(0.008, 0.007, 0.01)
    background.inputs["Strength"].default_value = 1.0
    scene.world = world
    rig = [
        ((-6.0, -5.0, 9.0), (6.0, 4.0), 320.0, (1.0, 0.9, 0.78)),
        ((7.0, 3.0, 7.0), (3.0, 8.0), 180.0, (0.8, 0.88, 1.0)),
        ((0.0, 11.0, 9.0), (10.0, 1.0), 220.0, (1.0, 0.65, 0.35)),
        ((0.0, -9.0, 6.0), (10.0, 1.0), 90.0, (1.0, 1.0, 1.0)),
    ]
    for index, (location, (size_x, size_y), power, color) in enumerate(rig):
        data = bpy.data.lights.new(f"panel{index}", "AREA")
        data.shape, data.size, data.size_y = "RECTANGLE", size_x, size_y
        data.energy, data.color = power, color
        lamp = bpy.data.objects.new(f"panel{index}", data)
        lamp.visible_camera = False
        scene.collection.objects.link(lamp)
        lamp.location = location
        lamp.rotation_euler = (-Vector(location)).to_track_quat("-Z", "Y").to_euler()
    strips = [((-9.0, 0.0, 7.0), (0.2, 14.0, 3.0)), ((9.0, 0.0, 7.0), (0.2, 14.0, 3.0))]
    glow = emission("softbox", rgba(1.0, 0.95, 0.88), 3.0)
    for location, scale in strips:
        bpy.ops.mesh.primitive_cube_add(location=location, scale=scale)
        strip = bpy.context.active_object
        strip.data.materials.append(glow)
        strip.visible_camera = False


def studio_world(scene: bpy.types.Scene) -> None:
    tree = scene.world.node_tree
    nodes, links = tree.nodes, tree.links
    coordinates = nodes.new("ShaderNodeTexCoord")
    axes = nodes.new("ShaderNodeSeparateXYZ")
    links.new(coordinates.outputs["Generated"], axes.inputs["Vector"])
    sky = ramp(
        nodes,
        links,
        axes.outputs["Z"],
        [(0.35, rgba(0.02, 0.012, 0.006)), (0.5, rgba(0.9, 0.55, 0.25)), (0.8, rgba(1.4, 1.3, 1.2))],
    )
    path = nodes.new("ShaderNodeLightPath")
    mix = nodes.new("ShaderNodeMixRGB")
    mix.inputs["Color2"].default_value = rgba(0.0, 0.0, 0.0)
    links.new(path.outputs["Is Camera Ray"], mix.inputs["Fac"])
    links.new(sky.outputs["Color"], mix.inputs["Color1"])
    links.new(mix.outputs["Color"], nodes["Background"].inputs["Color"])


def board(style: str) -> list[bpy.types.Object]:
    light, dark, frame = BOARD_LOOKS[style]()
    parts = []
    for file in range(8):
        for rank in range(8):
            bpy.ops.mesh.primitive_cube_add(location=(file - 3.5, rank - 3.5, -0.05), scale=(0.5, 0.5, 0.05))
            square = bpy.context.active_object
            square.data.materials.append(light if (file + rank) % 2 else dark)
            parts.append(square)
    bpy.ops.mesh.primitive_cube_add(location=(0.0, 0.0, -0.13), scale=(4.75, 4.75, 0.12))
    rim = bpy.context.active_object
    bevel = rim.modifiers.new("bevel", "BEVEL")
    bevel.width, bevel.segments = 0.06, 4
    rim.data.materials.append(frame)
    parts.append(rim)
    bpy.ops.mesh.primitive_plane_add(location=(0.0, 0.0, -0.26), size=200)
    floor = bpy.context.active_object
    floor_material, _, _, floor_shader = node_material("floor")
    set_inputs(floor_shader, Base_Color=rgba(0.004, 0.0035, 0.003), Roughness=1.0, Specular_IOR_Level=0.0)
    floor.data.materials.append(floor_material)
    parts.append(floor)
    return parts



def seen(scene: bpy.types.Scene, camera: bpy.types.Object, point: tuple[float, float, float]) -> tuple[float, float]:
    """Where `point` lands in the full render, in normalized coordinates from the top left."""
    view = world_to_camera_view(scene, camera, Vector(point))
    return view.x, 1.0 - view.y


def render(scene: bpy.types.Scene, path: Path) -> None:
    scene.render.filepath = str(path)
    bpy.ops.render.render(write_still=True)


def render_texture(args: argparse.Namespace, style: str, out: Path) -> None:
    """The board seen straight from above, which kess draws in perspective itself."""
    scene = reset_scene(args.samples)
    scene.render.resolution_x = scene.render.resolution_y = TEXTURE
    scene.render.image_settings.file_format = "JPEG"
    scene.render.image_settings.quality = 90
    world = bpy.data.worlds.new("world")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Color"].default_value = rgba(0.3, 0.3, 0.3)
    scene.world = world
    sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
    sun.data.energy, sun.data.angle = 2.5, math.radians(30)
    scene.collection.objects.link(sun)
    parts = board(style)
    bpy.data.objects.remove(parts[-1])
    # kess adds a sheen that follows the camera, so the texture stays free of reflections
    for material in bpy.data.materials:
        principled = material.node_tree and material.node_tree.nodes.get("Principled BSDF")
        if principled:
            principled.inputs["Coat Weight"].default_value = 0.0
            principled.inputs["Roughness"].default_value = 0.7
    camera = bpy.data.objects.new("camera", bpy.data.cameras.new("camera"))
    camera.data.type, camera.data.ortho_scale = "ORTHO", BOARD_SPAN
    camera.location = (0.0, 0.0, 20.0)
    scene.collection.objects.link(camera)
    scene.camera = camera
    render(scene, out / "boards" / f"{style}.jpg")


def crop_to_piece(scene: bpy.types.Scene, camera: bpy.types.Object) -> dict[str, float | list[float]]:
    """Crop the render to the piece and its shadow, and return where its foot lands in the crop."""
    corners = [
        seen(scene, camera, (x, y, z)) for x in (-0.55, 0.55) for y in (-0.55, 0.55) for z in (-0.02, 2.0)
    ]
    left, right = min(c[0] for c in corners), max(c[0] for c in corners)
    top, bottom = min(c[1] for c in corners), max(c[1] for c in corners)
    render_settings = scene.render
    render_settings.use_border, render_settings.use_crop_to_border = True, True
    render_settings.border_min_x, render_settings.border_max_x = left, right
    render_settings.border_min_y, render_settings.border_max_y = 1.0 - bottom, 1.0 - top
    foot = seen(scene, camera, (0.0, 0.0, 0.0))
    head = seen(scene, camera, (0.0, 0.0, 1.0))
    width, height = render_settings.resolution_x, render_settings.resolution_y
    # blender rounds the border to whole pixels from the bottom left
    crop_left = math.floor(left * width)
    crop_top = height - math.ceil((1.0 - top) * height)
    return {
        "foot": [round(foot[0] * width - crop_left, 2), round(foot[1] * height - crop_top, 2)],
        "unit": round((foot[1] - head[1]) * height, 2),
    }


def render_pieces(args: argparse.Namespace, styles: list[str], out: Path) -> dict[str, dict[str, float | list[float]]]:
    frames = {}
    for style in styles:
        for color in ("w", "b"):
            for kind in args.kinds:
                scene = reset_scene(args.samples)
                scene.render.film_transparent = True
                scene.render.image_settings.file_format = "PNG"
                scene.render.image_settings.color_mode = "RGBA"
                light_rig(scene)
                studio_world(scene)
                bpy.ops.mesh.primitive_plane_add(size=2)
                bpy.context.active_object.is_shadow_catcher = True
                piece = build_piece(kind)
                piece.data.materials.clear()
                piece.data.materials.append(PIECE_LOOKS[style][color]())
                camera = place_camera(scene)
                frames[kind] = crop_to_piece(scene, camera)
                render(scene, out / "pieces" / style / f"{color}{kind}.png")
    return frames


def camera_manifest() -> dict[str, float | list[float]]:
    """The sprite camera, so kess can place the board under the pieces the same way."""
    return {
        "position": [0.0, -11.6, 9.6],
        "target": [0.0, -0.5, 0.0],
        "fov_x": round(2.0 * math.atan(18.0 / 40.0), 6),
        "aspect": WIDTH / HEIGHT,
        "board_span": BOARD_SPAN,
    }


def main() -> None:
    args = arguments()
    out = Path(args.out).resolve()
    styles = [style for style in args.styles.split(",") if style]
    manifest_path = out / "layout.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    manifest = {key: manifest[key] for key in ("pieces",) if key in manifest}
    manifest["camera"] = camera_manifest()
    if args.only in ("boards", "all"):
        for style in styles:
            render_texture(args, style, out)
    if args.only in ("pieces", "all"):
        manifest["pieces"] = {**manifest.get("pieces", {}), **render_pieces(args, styles, out)}
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")


main()
