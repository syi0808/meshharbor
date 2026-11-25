import base64
import json
from pathlib import Path
import struct
import tempfile
import unittest

import numpy as np

from repart_glb import Source, pack_glb


def fixture():
    positions = np.asarray([(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)], dtype="<f4").tobytes()
    texcoords = np.asarray([(0, 0), (1, 0), (0, 1), (1, 1)], dtype="<f4").tobytes()
    indices = np.asarray([0, 1, 2, 1, 3, 2], dtype="<u2").tobytes()
    png = base64.b64decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8"
        "/x8AAwMCAO+X2yQAAAAASUVORK5CYII="
    )
    segments = [positions, texcoords, indices, png]
    views = []
    payload = bytearray()
    for data in segments:
        views.append({"buffer": 0, "byteOffset": len(payload), "byteLength": len(data)})
        payload += data
    return {
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "Whole", "mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "indices": 2, "material": 0}]}],
        "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
        "textures": [{"source": 0}],
        "images": [{"bufferView": 3, "mimeType": "image/png"}],
        "buffers": [{"byteLength": len(payload)}],
        "bufferViews": views,
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"},
            {"bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR"},
        ],
    }, payload


def read_output(data):
    json_size = struct.unpack_from("<I", data, 12)[0]
    return json.loads(data[20:20 + json_size]), data[28 + json_size:]


class RepartGlbTest(unittest.TestCase):
    def write_source(self, root, document, payload):
        path = Path(root) / "source.glb"
        path.write_bytes(pack_glb(document, payload))
        return Source(path)

    def test_splits_connected_faces_into_nodes_while_retaining_appearance(self):
        doc, payload = fixture()
        with tempfile.TemporaryDirectory() as root:
            source = self.write_source(root, doc, payload)
            result, report = source.export(np.array([1, 0]))
        output, binary = read_output(result)
        self.assertEqual(report["triangles_per_part"], {0: 1, 1: 1})
        self.assertEqual(binary[:len(payload)], payload)
        self.assertEqual(output["materials"], doc["materials"])
        self.assertEqual(output["images"], doc["images"])
        self.assertEqual(output["nodes"][0], {"name": "Whole", "children": [1, 2]})
        self.assertEqual([node["mesh"] for node in output["nodes"][1:]], [0, 1])
        for mesh, indices in zip(output["meshes"], ((1, 3, 2), (0, 1, 2)), strict=True):
            primitive = mesh["primitives"][0]
            self.assertEqual(primitive["attributes"], {"POSITION": 0, "TEXCOORD_0": 1})
            self.assertEqual(primitive["material"], 0)
            view = output["bufferViews"][output["accessors"][primitive["indices"]]["bufferView"]]
            self.assertEqual(struct.unpack_from("<III", binary, view["byteOffset"]), indices)

    def test_rejects_invalid_and_non_split_labels(self):
        doc, payload = fixture()
        with tempfile.TemporaryDirectory() as root:
            source = self.write_source(root, doc, payload)
            for invalid in ([0], [1, 1], [0, -1], [0.0, 1.0]):
                with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                    source.export(np.array(invalid))

    def test_rejects_instancing_or_transformed_source(self):
        with tempfile.TemporaryDirectory() as root:
            doc, payload = fixture()
            doc["scenes"][0]["nodes"].append(0)
            with self.assertRaisesRegex(ValueError, "without instancing"):
                self.write_source(root, doc, payload)
            doc["scenes"][0]["nodes"].pop()
            doc["nodes"][0]["scale"] = [2, 2, 2]
            with self.assertRaisesRegex(ValueError, "Apply transforms"):
                self.write_source(root, doc, payload)


if __name__ == "__main__":
    unittest.main()
