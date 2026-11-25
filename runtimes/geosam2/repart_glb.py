"""Read one source primitive and export parts without rebuilding vertex streams."""

import json
from pathlib import Path
import struct

import numpy as np


MAX_BYTES = 64 * 1024 * 1024
MAX_TRIANGLES = 500_000
JSON_CHUNK = b"JSON"
BIN_CHUNK = b"BIN\0"


def pack_glb(document, binary):
    payload = json.dumps(document, separators=(",", ":"), allow_nan=False).encode()
    payload += b" " * (-len(payload) % 4)
    binary = bytes(binary) + b"\0" * (-len(binary) % 4)
    size = 28 + len(payload) + len(binary)
    if size > MAX_BYTES:
        raise ValueError("The split GLB exceeds the 64 MiB worker limit.")
    return (
        struct.pack("<4sII", b"glTF", 2, size)
        + struct.pack("<I4s", len(payload), JSON_CHUNK) + payload
        + struct.pack("<I4s", len(binary), BIN_CHUNK) + binary
    )


class Source:
    def __init__(self, path):
        with Path(path).open("rb") as stream:
            data = stream.read(MAX_BYTES + 1)
        if not 28 <= len(data) <= MAX_BYTES:
            raise ValueError("Source size is outside the GLB worker limit.")
        if struct.unpack_from("<4sII", data) != (b"glTF", 2, len(data)):
            raise ValueError("The source is not a valid GLB 2 container.")
        json_size, json_type = struct.unpack_from("<I4s", data, 12)
        bin_header = 20 + json_size
        if json_type != JSON_CHUNK or bin_header + 8 > len(data):
            raise ValueError("The source is missing the embedded JSON or binary chunk.")
        bin_size, bin_type = struct.unpack_from("<I4s", data, bin_header)
        if bin_type != BIN_CHUNK or bin_header + 8 + bin_size != len(data):
            raise ValueError("Expected exactly one embedded binary chunk.")
        self.document = json.loads(data[20:bin_header])
        self.binary = memoryview(data)[bin_header + 8:]
        doc = self.document
        if any(doc.get(key) for key in ("animations", "skins", "extensionsRequired")):
            raise ValueError("Guided repart requires a static GLB without required extensions.")
        if len(doc["buffers"]) != 1 or "uri" in doc["buffers"][0]:
            raise ValueError("Guided repart requires one embedded buffer.")
        if any("uri" in image for image in doc.get("images", [])):
            raise ValueError("Guided repart requires embedded images.")
        declared_size = doc["buffers"][0]["byteLength"]
        if not 0 < declared_size <= len(self.binary) <= declared_size + 3:
            raise ValueError("Invalid embedded buffer length.")
        self.binary = self.binary[:declared_size]
        if len(doc["meshes"]) != 1 or len(doc["meshes"][0]["primitives"]) != 1:
            raise ValueError("Guided repart currently requires one mesh with one triangle primitive.")
        active_mesh_nodes = [
            idx for idx, node in enumerate(doc["nodes"]) if node.get("mesh") == 0
        ]
        if len(doc["scenes"]) != 1 or len(active_mesh_nodes) != 1 or any(
            {"translation", "rotation", "scale", "matrix", "skin", "weights", "extensions"} & node.keys()
            for node in doc["nodes"]
        ):
            raise ValueError("Apply transforms and use one untransformed mesh node in one scene.")
        self.node = active_mesh_nodes[0]
        self.validate_scene()
        primitive = doc["meshes"][0]["primitives"][0]
        if primitive.get("mode", 4) != 4 or primitive.get("targets") or primitive.get("extensions"):
            raise ValueError("Guided repart requires static, uncompressed triangles.")
        self.positions = self.read_accessor(primitive["attributes"]["POSITION"], {5126: "<f4"}, "VEC3", 3)
        indices = self.read_accessor(primitive["indices"], {5121: "u1", 5123: "<u2", 5125: "<u4"}, "SCALAR", 1)
        if indices.size % 3 or not 1 <= indices.size // 3 <= MAX_TRIANGLES:
            raise ValueError("Source triangle count is outside the worker limit.")
        if not np.isfinite(self.positions).all() or int(indices.max()) >= len(self.positions):
            raise ValueError("Source triangle positions or indices are invalid.")
        self.faces = indices.reshape(-1, 3)

    def validate_scene(self):
        doc = self.document
        if doc.get("scene", 0) != 0:
            raise ValueError("The GLB has no usable default scene.")
        pending = list(doc["scenes"][0]["nodes"])
        seen = set()
        while pending:
            node_index = pending.pop()
            if not 0 <= node_index < len(doc["nodes"]) or node_index in seen:
                raise ValueError("Scene nodes must form a tree without instancing.")
            seen.add(node_index)
            node = doc["nodes"][node_index]
            if "mesh" in node and node["mesh"] != 0:
                raise ValueError("The scene references an invalid mesh.")
            pending.extend(node.get("children", []))
        if self.node not in seen:
            raise ValueError("The source mesh is not in the default scene.")

    def read_accessor(self, idx, types, shape, components):
        if not 0 <= idx < len(self.document["accessors"]):
            raise ValueError("A primitive references an invalid accessor.")
        accessor = self.document["accessors"][idx]
        view_index = accessor["bufferView"]
        if not 0 <= view_index < len(self.document["bufferViews"]):
            raise ValueError("An accessor references an invalid buffer view.")
        view = self.document["bufferViews"][view_index]
        if (accessor["type"] != shape or accessor["componentType"] not in types
                or accessor.get("sparse") or accessor.get("normalized") or view.get("extensions")
                or view["buffer"] != 0):
            raise ValueError(f"Unsupported {shape} accessor in source GLB.")
        dtype = np.dtype(types[accessor["componentType"]])
        count = accessor["count"]
        element = dtype.itemsize * components
        stride = view.get("byteStride", element)
        start = view.get("byteOffset", 0)
        inner = accessor.get("byteOffset", 0)
        if (not isinstance(count, int) or count < 1 or stride < element or start < 0 or inner < 0
                or start + view["byteLength"] > len(self.binary)
                or inner + (count - 1) * stride + element > view["byteLength"]):
            raise ValueError("Accessor extends beyond its buffer view.")
        return np.ndarray((count, components), dtype=dtype, buffer=self.binary,
                          offset=start + inner, strides=(stride, dtype.itemsize))

    def export(self, labels):
        if labels.shape != (len(self.faces),) or labels.dtype.kind not in "iu" or np.any(labels < 0):
            raise ValueError("GeoSAM2 labels do not match the source triangles.")
        parts, counts = np.unique(labels, return_counts=True)
        if not 2 <= len(parts) <= 64 or int(parts[-1]) > 65535:
            raise ValueError("Expected a meaningful split into 2 to 64 parts.")
        document = json.loads(json.dumps(self.document))
        original_mesh = document["meshes"][0]
        original_primitive = original_mesh["primitives"][0]
        document["meshes"] = []
        parent = document["nodes"][self.node]
        parent.pop("mesh")
        children = parent.setdefault("children", [])
        binary = bytearray(self.binary)
        for label in parts:
            indices = self.faces[labels == label].astype("<u4", copy=False).reshape(-1)
            binary += b"\0" * (-len(binary) % 4)
            views = document["bufferViews"]
            accessors = document["accessors"]
            views.append({"buffer": 0, "byteOffset": len(binary), "byteLength": indices.nbytes, "target": 34963})
            binary += indices.tobytes()
            accessors.append({
                "bufferView": len(views) - 1, "componentType": 5125, "type": "SCALAR",
                "count": indices.size, "min": [int(indices.min())], "max": [int(indices.max())],
            })
            name = "Remainder" if label == 0 else f"Part {int(label)}"
            mesh_index = len(document["meshes"])
            document["meshes"].append({
                "name": name, "primitives": [{**original_primitive, "indices": len(accessors) - 1}],
            })
            children.append(len(document["nodes"]))
            document["nodes"].append({"name": name, "mesh": mesh_index})
        document["buffers"][0]["byteLength"] = len(binary)
        return pack_glb(document, binary), {
            "triangles": len(self.faces), "parts": int(len(parts)),
            "triangles_per_part": dict(zip(map(int, parts), map(int, counts), strict=True)),
        }
