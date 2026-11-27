import json
import math
from pathlib import Path
import sys

import bpy
import numpy as np
from mathutils import Matrix, Vector


def select(active, source=None):
    bpy.ops.object.select_all(action="DESELECT")
    if source is not None:
        source.select_set(True)
    active.select_set(True)
    bpy.context.view_layer.objects.active = active


def blender_frame(position):
    return Vector((position[0], -position[2], position[1]))


def import_source(path, center, extent):
    bpy.ops.import_scene.gltf(filepath=str(path))
    source, = (obj for obj in bpy.context.scene.objects if obj.type == "MESH")
    world = source.matrix_world.copy()
    source.parent = None
    source.matrix_world = Matrix.Identity(4)
    for vertex in source.data.vertices:
        vertex.co = (world @ vertex.co - center) / extent
    source.data.update()
    return source


def unwrap_target(geometry):
    indices = geometry["indices"]
    data = bpy.data.meshes.new("LATO.2 triangles")
    data.from_pydata(
        [blender_frame(position) for position in geometry["positions"]],
        [],
        [indices[start:start + 3] for start in range(0, len(indices), 3)],
    )
    target = bpy.data.objects.new("LATO.2 retopology", data)
    bpy.context.collection.objects.link(target)
    select(target)
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.uv.smart_project(angle_limit=math.radians(66), island_margin=0.008)
    bpy.ops.object.mode_set(mode="OBJECT")
    return target


def active_image(tree, label, resolution, colorspace):
    image = bpy.data.images.new(label, width=resolution, height=resolution, alpha=False)
    image.colorspace_settings.name = colorspace
    node = tree.nodes.new("ShaderNodeTexImage")
    node.image = image
    tree.nodes.active = node
    return node


def bake(target, source=None, margin=0):
    select(target, source)
    status = bpy.ops.object.bake(
        type="EMIT",
        use_selected_to_active=source is not None,
        use_clear=True,
        cage_extrusion=0.012,
        max_ray_distance=0.030,
        margin=margin,
        margin_type="EXTEND",
    )
    if "FINISHED" not in status:
        raise RuntimeError("Cycles emission bake did not finish")


def count_mask(image):
    pixels = np.empty(len(image.pixels), dtype=np.float32)
    image.pixels.foreach_get(pixels)
    return int(np.count_nonzero(pixels[::4] > 0.5))


def measure_coverage(target, source, target_tree, resolution):
    mask = active_image(target_tree, "Projection mask", resolution, "Non-Color")
    bake(target, source)
    projected = count_mask(mask.image)
    white = target_tree.nodes.new("ShaderNodeEmission")
    white.inputs["Color"].default_value = (1, 1, 1, 1)
    target_tree.links.new(white.outputs[0], target_tree.nodes.get("Material Output").inputs["Surface"])
    bake(target)
    occupied = count_mask(mask.image)
    image = mask.image
    target_tree.nodes.remove(white)
    target_tree.nodes.remove(mask)
    bpy.data.images.remove(image)
    return projected, occupied


def connect_input(tree, source, destination):
    if source.is_linked:
        tree.links.new(source.links[0].from_socket, destination)
    else:
        destination.default_value = source.default_value


def pack(image, path):
    image.filepath_raw = str(path)
    image.file_format = "PNG"
    image.save()
    image.pack()


def main(work):
    settings = json.loads((work / "bake-input.json").read_text())
    center = blender_frame(settings["frame"]["center"])
    extent = settings["frame"]["extent"]
    resolution = settings["atlas_resolution"]

    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 16
    scene.render.threads_mode = "FIXED"
    scene.render.threads = 4
    source = import_source(work / "source.glb", center, extent)
    target = unwrap_target(settings["geometry"])

    source_material = source.data.materials[0]
    source_tree = source_material.node_tree
    source_shader = next(node for node in source_tree.nodes if node.type == "BSDF_PRINCIPLED")
    if not source_shader.inputs["Base Color"].is_linked:
        raise RuntimeError("Blender did not import the supported source base-color map")
    emission = source_tree.nodes.new("ShaderNodeEmission")
    source_tree.links.new(
        emission.outputs[0], source_tree.nodes.get("Material Output").inputs["Surface"]
    )
    emission.inputs["Color"].default_value = (1, 1, 1, 1)

    material = bpy.data.materials.new("LATO.2 rebaked PBR")
    material.use_nodes = True
    material.use_backface_culling = source_material.use_backface_culling
    target.data.materials.append(material)
    target_tree = material.node_tree
    target_shader = target_tree.nodes.get("Principled BSDF")
    surface = target_tree.nodes.get("Material Output").inputs["Surface"]
    projected, occupied = measure_coverage(target, source, target_tree, resolution)
    if not occupied or projected > occupied or projected / occupied < settings["min_coverage"]:
        raise RuntimeError(f"Projection hit coverage failed: {projected}/{occupied} UV texels")
    print(f"Projection hit coverage: {projected}/{occupied} UV texels", flush=True)
    target_tree.links.new(target_shader.outputs[0], surface)

    margin = max(6, 12 * resolution // 1024)
    color = active_image(target_tree, "Rebaked base color", resolution, "sRGB")
    connect_input(source_tree, source_shader.inputs["Base Color"], emission.inputs["Color"])
    bake(target, source, margin)
    pack(color.image, work / "base-color.png")
    target_tree.links.new(color.outputs["Color"], target_shader.inputs["Base Color"])

    if settings["has_mr"]:
        if not any(source_shader.inputs[name].is_linked for name in ("Roughness", "Metallic")):
            raise RuntimeError("Blender did not import the supported metallic/roughness map")
        packed = source_tree.nodes.new("ShaderNodeCombineColor")
        packed.inputs["Red"].default_value = 1.0
        connect_input(source_tree, source_shader.inputs["Roughness"], packed.inputs["Green"])
        connect_input(source_tree, source_shader.inputs["Metallic"], packed.inputs["Blue"])
        source_tree.links.new(packed.outputs["Color"], emission.inputs["Color"])
        mr = active_image(target_tree, "Rebaked metallic roughness", resolution, "Non-Color")
        bake(target, source, margin)
        pack(mr.image, work / "metallic-roughness.png")
        channels = target_tree.nodes.new("ShaderNodeSeparateColor")
        target_tree.links.new(mr.outputs["Color"], channels.inputs["Color"])
        target_tree.links.new(channels.outputs["Green"], target_shader.inputs["Roughness"])
        target_tree.links.new(channels.outputs["Blue"], target_shader.inputs["Metallic"])
    else:
        for name in ("Roughness", "Metallic"):
            target_shader.inputs[name].default_value = source_shader.inputs[name].default_value

    for vertex in target.data.vertices:
        vertex.co = vertex.co * extent + center
    for face in target.data.polygons:
        face.use_smooth = True
    target.data.update()
    select(target)
    status = bpy.ops.export_scene.gltf(
        filepath=str(work / "rebaked.glb"),
        export_format="GLB",
        export_animations=False,
        use_selection=True,
    )
    if "FINISHED" not in status:
        raise RuntimeError("GLB export did not finish")
    (work / "bake-stats.json").write_text(json.dumps({
        "projected_texels": projected,
        "occupied_texels": occupied,
    }))


if __name__ == "__main__":
    main(Path(sys.argv[sys.argv.index("--") + 1]))
