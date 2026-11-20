import itertools

import mlx.core as mx
import mlx.nn as nn
import numpy as np
from mlx_diffuser.layers.sparse import SparseConv3D, sparse_subdivide
from mlx_diffuser.layers.trellis import absolute_position_embedding
from mlx_diffuser.layers.trellis_sparse import SparseLinear, SparseTransformerBlock


class SparseGroupNorm(nn.Module):
    def __init__(self, channels):
        super().__init__()
        self.weight = mx.ones((channels,))
        self.bias = mx.zeros((channels,))

    def __call__(self, sparse):
        features = sparse.features
        groups = features.astype(mx.float32).reshape(features.shape[0], 32, -1)
        centered = groups - mx.mean(groups, axis=(0, 2), keepdims=True)
        variance = mx.mean(mx.square(centered), axis=(0, 2), keepdims=True)
        normalized = (centered * mx.rsqrt(variance + 1e-5)).reshape(features.shape)
        return sparse.replace((normalized * self.weight + self.bias).astype(features.dtype))


def mesh_neighbor_map(sparse):
    shape = tuple(size + 2 for size in sparse.spatial_shape)
    grid = np.full(shape, -1, dtype=np.int32)
    first, second, third = np.asarray(sparse.coords[:, 1:]).T + 1
    grid[first, second, third] = np.arange(sparse.num_points, dtype=np.int32)
    neighbors = np.empty((sparse.num_points, 27), dtype=np.int32)
    for index, (dx, dy, dz) in enumerate(itertools.product((-1, 0, 1), repeat=3)):
        neighbors[:, index] = grid[first + dx, second + dy, third + dz]
    return mx.array(neighbors)


class SparseSubdivideBlock(nn.Module):
    def __init__(self, channels, out_channels):
        super().__init__()
        self.act_layers = [SparseGroupNorm(channels), nn.SiLU()]
        self.out_layers = [
            SparseConv3D(channels, out_channels),
            SparseGroupNorm(out_channels),
            nn.SiLU(),
            SparseConv3D(out_channels, out_channels),
        ]
        self.skip_connection = SparseConv3D(channels, out_channels, kernel_size=1)

    def __call__(self, source):
        x = self.act_layers[0](source)
        x = sparse_subdivide(x.replace(nn.silu(x.features)))
        key = f"neighbors_3_1_{x.scale}"
        x.cache[key] = mesh_neighbor_map(x)
        x = self.out_layers[0](x)
        mx.eval(x.features)
        x = self.out_layers[1](x)
        x = self.out_layers[3](x.replace(nn.silu(x.features)))
        skip_weights = self.skip_connection.conv
        skip = source.features @ skip_weights.weight[:, 0, 0, 0, :].T + skip_weights.bias
        x = x + mx.repeat(skip, 8, axis=0)
        mx.eval(x.features)
        x.cache.pop(key)
        return x


class MeshDecoder(nn.Module):
    def __init__(self, *, resolution, model_channels, latent_channels, num_blocks, num_heads,
                 mlp_ratio, attn_mode, window_size, use_fp16, representation_config):
        super().__init__()
        if attn_mode != "swin" or not representation_config["use_color"]:
            raise ValueError("Expected the TRELLIS Large shifted-window FlexiCubes mesh decoder.")
        self.resolution = resolution
        self.model_channels = model_channels
        self.use_fp16 = use_fp16
        self.input_layer = SparseLinear(latent_channels, model_channels)
        self.blocks = [
            SparseTransformerBlock(
                model_channels, num_heads, mlp_ratio, window_size=window_size,
                shift_window=(window_size // 2 * (index % 2),) * 3,
            )
            for index in range(num_blocks)
        ]
        self.upsample = [
            SparseSubdivideBlock(model_channels, model_channels // 4),
            SparseSubdivideBlock(model_channels // 4, model_channels // 8),
        ]
        self.out_layer = SparseLinear(model_channels // 8, 101)

    def __call__(self, source):
        if source.batch_size != 1 or source.spatial_shape != (self.resolution,) * 3:
            raise ValueError("Mesh decoder requires a single structured latent on its native grid.")
        x = self.input_layer(source)
        x = x + absolute_position_embedding(x.coords[:, 1:].astype(mx.float32), self.model_channels)
        if self.use_fp16:
            x = x.astype(mx.float16)
        for block in self.blocks:
            x = block(x)
            mx.eval(x.features)
        for block in self.upsample:
            x = block(x)
        full = self.out_layer(x.astype(source.dtype))
        geometry = full.replace(full.features[:, :53])
        mx.eval(geometry.features)
        return geometry


def build_mesh_decoder(config, weights):
    model = MeshDecoder(**config["args"])
    model.load_weights(list(weights.items()), strict=True)
    return model.eval()
