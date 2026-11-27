import sys
import types

import numpy as np
from PIL import Image
import torch


def mesh_voxels(vertices, faces, grid_size, aabb):
    if not isinstance(grid_size, int) or not 1 <= grid_size <= 128:
        raise ValueError("Unexpected LATO.2 voxel grid size")
    positions = (np.asarray(vertices, dtype=np.float64) - np.asarray(aabb[0])) * grid_size
    occupied = np.zeros((grid_size, grid_size, grid_size), dtype=bool)
    eye = np.eye(3)
    candidates_total = 0
    for triangle in positions[np.asarray(faces)]:
        low = np.floor(triangle.min(axis=0) - 1e-6).astype(int).clip(0, grid_size - 1)
        high = np.floor(triangle.max(axis=0) + 1e-6).astype(int).clip(0, grid_size - 1)
        candidates_total += np.prod(high - low + 1)
        if candidates_total > 15_000_000:
            raise ValueError("Mesh conditioning exceeds the CPU voxel traversal budget")
        cells = np.mgrid[low[0]:high[0] + 1, low[1]:high[1] + 1, low[2]:high[2] + 1].reshape(3, -1).T
        edges = np.roll(triangle, -1, axis=0) - triangle
        axes = np.concatenate((eye, np.cross(edges[0], edges[1])[None], np.cross(edges[:, None], eye[None]).reshape(-1, 3)))
        interval = triangle @ axes.T
        midpoints = (cells + 0.5) @ axes.T
        radius = abs(axes).sum(axis=1) * 0.5 + 1e-6
        keep = np.all((midpoints + radius >= interval.min(axis=0)) & (midpoints - radius <= interval.max(axis=0)), axis=1)
        occupied[tuple(cells[keep].T)] = True
    return torch.from_numpy(np.argwhere(occupied).astype(np.int32)), None, None


class WhiteModelRenderer:
    def __init__(self, img_res, crop_padding, **options):
        self.resolution = img_res
        self.padding = crop_padding

    def render(self, vertices, faces, num_views, azimuths, elevations):
        if num_views != 1:
            raise ValueError("LATO.2 Mesh to Mesh uses exactly one conditioning view")
        angles = np.deg2rad([azimuths[0], elevations[0]])
        azimuth, elevation = angles
        direction = np.array([np.cos(azimuth) * np.cos(elevation), np.sin(elevation), np.sin(azimuth) * np.cos(elevation)])
        right = np.cross([0, 1, 0], direction)
        right /= np.linalg.norm(right)
        up = np.cross(direction, right)
        center = (vertices.max(axis=0) + vertices.min(axis=0)) * 0.5
        centered = vertices - center
        height = width = self.resolution
        depth = 1.8 - centered @ direction
        inverse_z = 1 / depth
        focal = width / (2 * np.tan(np.deg2rad(25)))
        screen = np.column_stack((width / 2 + centered @ right * focal * inverse_z,
                                  height / 2 - centered @ up * focal * inverse_z))
        reverse_depth = np.full((height, width), 0.0)
        pixels = np.zeros((height, width, 3), dtype=np.uint8)
        light = np.array([-0.35, 1, -0.35])
        light /= np.linalg.norm(light)
        for face in faces:
            corners = screen[face]
            minimum = np.floor(corners.min(axis=0)).astype(int).clip(0, width - 1)
            maximum = np.ceil(corners.max(axis=0)).astype(int).clip(0, width - 1)
            px, py = np.meshgrid(np.arange(minimum[0], maximum[0] + 1), np.arange(minimum[1], maximum[1] + 1))
            first, second, third = corners
            across, along = second - first, third - first
            signed_area = across[0] * along[1] - across[1] * along[0]
            if abs(signed_area) < 1e-9:
                continue
            column, row = px + 0.5 - first[0], py + 0.5 - first[1]
            weight_b = (column * along[1] - row * along[0]) / signed_area
            weight_c = (across[0] * row - across[1] * column) / signed_area
            weight_a = 1 - weight_b - weight_c
            closest = weight_a * inverse_z[face[0]] + weight_b * inverse_z[face[1]] + weight_c * inverse_z[face[2]]
            keep = (weight_a >= 0) & (weight_b >= 0) & (weight_c >= 0) & (closest > reverse_depth[py, px])
            if not np.any(keep):
                continue
            triangle = vertices[face]
            normal = np.cross(triangle[1] - triangle[0], triangle[2] - triangle[0])
            norm = np.linalg.norm(normal)
            if norm <= 1e-12:
                continue
            normal /= norm
            if normal @ direction < 0:
                normal *= -1
            shade = 0.50 + 0.33 * max(normal @ light, 0) + 0.14 * max(normal @ direction, 0)
            pixels[py[keep], px[keep]] = np.rint(np.array([200, 200, 210]) * shade)
            reverse_depth[py[keep], px[keep]] = closest[keep]
        ys, xs = np.where(reverse_depth != 0)
        if not len(xs):
            raise ValueError("LATO.2 conditioning produced an empty view")
        size = max(1, int(max(np.ptp(xs), np.ptp(ys)) * self.padding))
        left = int((xs.min() + xs.max() - size) // 2)
        top = int((ys.min() + ys.max() - size) // 2)
        image = Image.fromarray(pixels).crop((left, top, left + size, top + size))
        return [np.asarray(image.resize((width, height), Image.Resampling.LANCZOS))], []


def install():
    voxels = types.ModuleType("o_voxel")
    convert = types.ModuleType("o_voxel.convert")
    voxels.convert = convert
    convert.mesh_to_flexible_dual_grid = mesh_voxels
    renderer = types.ModuleType("dataset.mesh_render")
    renderer.WhiteModelRenderer = WhiteModelRenderer
    sys.modules.update({"o_voxel": voxels, "o_voxel.convert": convert, "dataset.mesh_render": renderer})
