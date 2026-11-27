import unittest

import mlx.core as mx
import numpy as np
import torch
from torch import nn
from torch.nn import functional as F

import mlx_kernels as kernels
from mesh_conditioning import WhiteModelRenderer, mesh_voxels


@unittest.skipUnless(mx.metal.is_available(), "requires Apple Silicon and Metal")
class MlxKernels(unittest.TestCase):
    def setUp(self):
        torch.manual_seed(42)
        mx.set_default_device(mx.gpu)

    def test_linear_and_volume_convolutions(self):
        pairs = [
            (kernels.linear, nn.Linear(8, 5), torch.randn(3, 7, 8)),
            (kernels.conv3d, nn.Conv3d(4, 8, 3, padding=1, stride=2), torch.randn(1, 4, 8, 6, 7)),
            (kernels.conv_transpose3d, nn.ConvTranspose3d(4, 8, 3, padding=1, stride=2, output_padding=1),
             torch.randn(1, 4, 3, 4, 5)),
        ]
        with torch.no_grad():
            for implementation, layer, source in pairs:
                torch.testing.assert_close(implementation(layer, source), layer(source), rtol=0.01, atol=0.002)

    def test_attention_masks_and_packed_batch_boundaries(self):
        tensor = torch.randn(2, 3, 7, 16)
        mask = torch.ones((2, 1, 1, 7), dtype=torch.bool)
        mask[..., -2:] = False
        torch.testing.assert_close(
            kernels.attention(tensor, tensor, tensor, mask),
            F.scaled_dot_product_attention(tensor, tensor, tensor, mask),
            atol=0.002, rtol=0.01,
        )
        packed = tensor.transpose(1, 2).reshape(14, 3, 16)
        offsets = torch.tensor([0, 7, 14])
        output = kernels.flash_varlen(packed, packed, packed, offsets, offsets, 7, 7)
        expected = F.scaled_dot_product_attention(tensor, tensor, tensor).transpose(1, 2).reshape(14, 3, 16)
        torch.testing.assert_close(output, expected, atol=0.002, rtol=0.01)

    def test_sparse_krsc_weights_match_dense_cross_correlation(self):
        coords = torch.tensor([[0, 0, 0, 0], [1, 0, 0, 1], [0, 1, 0, 0], [1, 0, 0, 0], [0, 1, 1, 0]])
        features = torch.randn(5, 4)
        sparse = kernels.SparseConvTensor(features, coords, (2, 2, 2), 2)
        batch, depth, row, col = coords.T
        volume = torch.zeros((2, 4, 2, 2, 2))
        volume[batch, :, depth, row, col] = features
        for size in (1, 3):
            layer = kernels.SubMConv3d(4, 8, size)
            with torch.no_grad():
                layer.weight.normal_(std=0.1)
                layer.bias.zero_()
                result = layer(sparse)
                dense = F.conv3d(volume, layer.weight.permute(0, 4, 1, 2, 3), padding=size // 2)
                self.assertIs(result.indices, coords)
                torch.testing.assert_close(result.features, dense[batch, :, depth, row, col], atol=0.002, rtol=0.01)

    def test_solver_conditioning_produces_bounded_voxels_and_an_image(self):
        points = torch.tensor([[-0.2, -0.2, 0.05], [0.2, -0.2, 0.05], [0.0, 0.2, 0.05]])
        faces = torch.tensor([[0, 1, 2]])
        voxels, _, _ = mesh_voxels(points, faces, 16, [[-0.5] * 3, [0.5] * 3])
        self.assertGreater(len(voxels), 2)
        self.assertTrue((voxels[:, 2] == 8).all())
        renderer = WhiteModelRenderer(128, 1.2)
        images, _ = renderer.render(points.numpy(), faces.numpy(), 1, [45], [30])
        self.assertEqual(images[0].shape, (128, 128, 3))
        self.assertGreater(np.count_nonzero(images[0]), 100)


if __name__ == "__main__":
    unittest.main()
