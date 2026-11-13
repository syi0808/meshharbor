#include "xatlas.h"
#define STBI_FAILURE_USERMSG
#define STB_IMAGE_IMPLEMENTATION
#include "stb_image.h"
#define STB_IMAGE_WRITE_IMPLEMENTATION
#include "stb_image_write.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <exception>
#include <limits>
#include <memory>
#include <string>
#include <stdexcept>
#include <vector>

struct MhAtlasResult {
    uint32_t vertex_count, index_count, width, height, chart_count;
    float *uv;
    uint32_t *xref, *indices;
    uint8_t *base_png;
    size_t base_png_len;
    uint8_t *mr_png;
    size_t mr_png_len;
    uint64_t covered_texels, gutter_texels;
};

namespace {
struct Image { int w = 0, h = 0; std::vector<uint8_t> rgba; };
struct OutputBytes { std::vector<uint8_t> bytes; };

void fail(char *dst, size_t cap, const std::string &message) {
    if (!dst || cap == 0) return;
    const size_t n = std::min(cap - 1, message.size());
    std::memcpy(dst, message.data(), n);
    dst[n] = '\0';
}

Image decode_png(const uint8_t *bytes, size_t length) {
    if (!bytes || length == 0 || length > static_cast<size_t>(std::numeric_limits<int>::max()))
        throw std::runtime_error("embedded PNG is empty or too large");
    Image image;
    int channels = 0;
    int width = 0, height = 0;
    if (!stbi_info_from_memory(bytes, static_cast<int>(length), &width, &height, &channels)
        || width <= 0 || height <= 0 || uint64_t(width) * uint64_t(height) > 16777216ull)
        throw std::runtime_error("PNG dimensions are invalid or exceed the 16M-pixel safety limit");
    stbi_uc *decoded = stbi_load_from_memory(bytes, static_cast<int>(length), &image.w, &image.h, &channels, 4);
    if (!decoded) throw std::runtime_error(std::string("PNG decode failed: ") + stbi_failure_reason());
    if (image.w != width || image.h != height || image.w <= 0 || image.h <= 0 || uint64_t(image.w) * uint64_t(image.h) > 16777216ull) {
        stbi_image_free(decoded);
        throw std::runtime_error("PNG dimensions are invalid or exceed the atlas safety limit");
    }
    image.rgba.assign(decoded, decoded + size_t(image.w) * size_t(image.h) * 4);
    stbi_image_free(decoded);
    return image;
}

void png_write(void *context, void *data, int size) {
    if (size <= 0) return;
    auto *out = static_cast<OutputBytes *>(context);
    auto *bytes = static_cast<uint8_t *>(data);
    out->bytes.insert(out->bytes.end(), bytes, bytes + size);
}

std::vector<uint8_t> encode_png(const Image &image) {
    OutputBytes out;
    if (!stbi_write_png_to_func(png_write, &out, image.w, image.h, 4, image.rgba.data(), image.w * 4))
        throw std::runtime_error("PNG encode failed");
    return std::move(out.bytes);
}

float wrap(float value) {
    value -= std::floor(value);
    return value;
}
float clamp01(float value) { return std::max(0.0f, std::min(1.0f, value)); }
float srgb_to_linear(float value) {
    return value <= 0.04045f ? value / 12.92f : std::pow((value + 0.055f) / 1.055f, 2.4f);
}
float linear_to_srgb(float value) {
    value = clamp01(value);
    return value <= 0.0031308f ? 12.92f * value : 1.055f * std::pow(value, 1.0f / 2.4f) - 0.055f;
}

std::array<float, 4> sample(const Image &image, float u, float v, bool srgb) {
    const float x = wrap(u) * image.w - 0.5f;
    const float y = wrap(v) * image.h - 0.5f;
    const int x0 = static_cast<int>(std::floor(x));
    const int y0 = static_cast<int>(std::floor(y));
    const float fx = x - x0, fy = y - y0;
    auto at = [&](int ix, int iy, int c) {
        ix = (ix % image.w + image.w) % image.w;
        iy = (iy % image.h + image.h) % image.h;
        float value = image.rgba[(size_t(iy) * image.w + ix) * 4 + c] / 255.0f;
        return srgb && c < 3 ? srgb_to_linear(value) : value;
    };
    std::array<float, 4> result{};
    for (int c = 0; c < 4; ++c) {
        const float a = at(x0, y0, c) * (1 - fx) + at(x0 + 1, y0, c) * fx;
        const float b = at(x0, y0 + 1, c) * (1 - fx) + at(x0 + 1, y0 + 1, c) * fx;
        result[c] = a * (1 - fy) + b * fy;
        if (srgb && c < 3) result[c] = linear_to_srgb(result[c]);
    }
    return result;
}

void write_pixel(Image &image, size_t index, const std::array<float, 4> &color) {
    for (int c = 0; c < 4; ++c)
        image.rgba[index * 4 + c] = static_cast<uint8_t>(std::lround(clamp01(color[c]) * 255.0f));
}
std::array<float, 4> read_pixel(const Image &image, size_t index) {
    return {image.rgba[index * 4] / 255.0f, image.rgba[index * 4 + 1] / 255.0f,
            image.rgba[index * 4 + 2] / 255.0f, image.rgba[index * 4 + 3] / 255.0f};
}

struct BakedTextures {
    std::vector<uint8_t> base, mr;
    uint64_t covered = 0, gutter = 0;
};

BakedTextures bake_textures(const Image &source_base, const Image &source_mr,
                            const std::vector<std::array<float, 2>> &source_uv,
                            const std::vector<std::array<float, 2>> &atlas_uv,
                            const std::vector<uint32_t> &xref,
                            const std::vector<uint32_t> &chart_ids,
                            const std::vector<uint32_t> &indices,
                            const uint32_t *atlas_image, int width, int height) {
    Image output_base{width, height, std::vector<uint8_t>(size_t(width) * height * 4, 0)};
    Image output_mr{width, height, std::vector<uint8_t>(size_t(width) * height * 4, 0)};
    std::vector<uint8_t> covered(size_t(width) * height, 0);
    std::vector<uint32_t> pixel_chart(size_t(width) * height, UINT32_MAX);

    for (size_t face = 0; face < indices.size() / 3; ++face) {
        uint32_t ids[3] = {indices[face * 3], indices[face * 3 + 1], indices[face * 3 + 2]};
        if (ids[0] >= xref.size() || ids[1] >= xref.size() || ids[2] >= xref.size())
            throw std::runtime_error("xatlas returned an out-of-range index");
        uint32_t source_ids[3] = {xref[ids[0]], xref[ids[1]], xref[ids[2]]};
        if (source_ids[0] >= source_uv.size() || source_ids[1] >= source_uv.size() || source_ids[2] >= source_uv.size())
            throw std::runtime_error("xatlas returned an out-of-range vertex reference");
        float x[3], y[3];
        for (int i = 0; i < 3; ++i) {
            x[i] = atlas_uv[ids[i]][0] * width;
            y[i] = atlas_uv[ids[i]][1] * height;
        }
        const float determinant = (x[1] - x[0]) * (y[2] - y[0]) - (y[1] - y[0]) * (x[2] - x[0]);
        if (std::abs(determinant) < 1e-8f) continue;
        const int xmin = std::max(0, static_cast<int>(std::floor(std::min({x[0], x[1], x[2]}))));
        const int xmax = std::min(width - 1, static_cast<int>(std::ceil(std::max({x[0], x[1], x[2]}))));
        const int ymin = std::max(0, static_cast<int>(std::floor(std::min({y[0], y[1], y[2]}))));
        const int ymax = std::min(height - 1, static_cast<int>(std::ceil(std::max({y[0], y[1], y[2]}))));
        const uint32_t chart = chart_ids[ids[0]];
        if (chart_ids[ids[1]] != chart || chart_ids[ids[2]] != chart)
            throw std::runtime_error("xatlas output triangle spans multiple charts");
        for (int py = ymin; py <= ymax; ++py) for (int px = xmin; px <= xmax; ++px) {
            const float qx = px + 0.5f, qy = py + 0.5f;
            const float a = ((x[1] - qx) * (y[2] - qy) - (y[1] - qy) * (x[2] - qx)) / determinant;
            const float b = ((x[2] - qx) * (y[0] - qy) - (y[2] - qy) * (x[0] - qx)) / determinant;
            const float c = 1.0f - a - b;
            if (a < -1e-5f || b < -1e-5f || c < -1e-5f) continue;
            const size_t pixel = size_t(py) * width + px;
            if (covered[pixel]) continue;
            const float u = a * source_uv[source_ids[0]][0] + b * source_uv[source_ids[1]][0] + c * source_uv[source_ids[2]][0];
            const float v = a * source_uv[source_ids[0]][1] + b * source_uv[source_ids[1]][1] + c * source_uv[source_ids[2]][1];
            write_pixel(output_base, pixel, sample(source_base, u, v, true));
            write_pixel(output_mr, pixel, sample(source_mr, u, v, false));
            covered[pixel] = 1;
            pixel_chart[pixel] = chart;
        }
    }
    // Atlas image pixels carry a chart id in their low bits. Seed only rasterized
    // pixels inside the matching chart; dilate into same-chart padding only.
    std::vector<int32_t> owner(size_t(width) * height, -1);
    for (size_t i = 0; i < owner.size(); ++i)
        if (atlas_image[i] & xatlas::kImageHasChartIndexBit)
            owner[i] = static_cast<int32_t>(atlas_image[i] & xatlas::kImageChartIndexMask);
    // xatlas mesh vertices keep chartIndex. Assign raster pixels to their label
    // using the triangle's first vertex chart. The caller stores chart ids below.
    // If a chart has sub-pixel rasterization gaps, nearest same-chart dilation fills them.
    std::vector<int32_t> source(owner.size(), -1);
    std::deque<int32_t> queue;
    for (size_t i = 0; i < owner.size(); ++i) {
        if (covered[i] && owner[i] >= 0 && pixel_chart[i] == static_cast<uint32_t>(owner[i])) {
            source[i] = static_cast<int32_t>(i);
            queue.push_back(static_cast<int32_t>(i));
        }
    }
    const int dx[4] = {1, -1, 0, 0}, dy[4] = {0, 0, 1, -1};
    while (!queue.empty()) {
        const int32_t current = queue.front(); queue.pop_front();
        const int px = current % width, py = current / width;
        for (int k = 0; k < 4; ++k) {
            const int nx = px + dx[k], ny = py + dy[k];
            if (nx < 0 || nx >= width || ny < 0 || ny >= height) continue;
            const int32_t neighbor = ny * width + nx;
            if (source[neighbor] >= 0 || owner[neighbor] < 0 || owner[neighbor] != owner[current]) continue;
            source[neighbor] = source[current]; queue.push_back(neighbor);
        }
    }
    BakedTextures result;
    for (size_t i = 0; i < covered.size(); ++i) {
        if (covered[i]) ++result.covered;
        else if (source[i] >= 0) {
            const size_t seed = static_cast<size_t>(source[i]);
            write_pixel(output_base, i, read_pixel(output_base, seed));
            write_pixel(output_mr, i, read_pixel(output_mr, seed));
            ++result.gutter;
        }
    }
    result.base = encode_png(output_base);
    result.mr = encode_png(output_mr);
    return result;
}

template <class T> T *copy_array(const T *data, size_t count) {
    if (count == 0) return nullptr;
    auto *output = static_cast<T *>(std::malloc(sizeof(T) * count));
    if (!output) throw std::bad_alloc();
    std::memcpy(output, data, sizeof(T) * count);
    return output;
}
} // namespace

extern "C" int meshharbor_atlas_bake(
    const float *positions, const float *normals, const float *original_uv,
    uint32_t vertex_count, const uint32_t *indices, uint32_t index_count,
    const uint8_t *base_png, size_t base_png_len, const uint8_t *mr_png, size_t mr_png_len,
    uint32_t resolution, uint32_t padding, MhAtlasResult *output, char *error, size_t error_capacity) {
    if (output) std::memset(output, 0, sizeof(*output));
    try {
        if (!output || !positions || !original_uv || !indices || vertex_count == 0 || index_count == 0 || index_count % 3 != 0)
            throw std::runtime_error("invalid atlas input arrays");
        if (resolution < 64 || resolution > 4096 || padding > 64)
            throw std::runtime_error("resolution must be 64..4096 and padding 0..64");
        if (uint64_t(resolution) * resolution > 16777216ull)
            throw std::runtime_error("requested atlas exceeds the 16M-texel safety limit");
        for (uint32_t i = 0; i < index_count; ++i)
            if (indices[i] >= vertex_count) throw std::runtime_error("source index is out of range");
        Image source_base = decode_png(base_png, base_png_len);
        Image source_mr = decode_png(mr_png, mr_png_len);
        std::unique_ptr<xatlas::Atlas, void (*)(xatlas::Atlas *)> atlas(xatlas::Create(), xatlas::Destroy);
        if (!atlas) throw std::runtime_error("could not allocate xatlas context");
        xatlas::MeshDecl declaration;
        declaration.vertexPositionData = positions;
        declaration.vertexPositionStride = sizeof(float) * 3;
        declaration.vertexNormalData = normals;
        declaration.vertexNormalStride = normals ? sizeof(float) * 3 : 0;
        declaration.vertexUvData = original_uv;
        declaration.vertexUvStride = sizeof(float) * 2;
        declaration.indexData = indices;
        declaration.indexCount = index_count;
        declaration.vertexCount = vertex_count;
        declaration.indexFormat = xatlas::IndexFormat::UInt32;
        declaration.faceCount = index_count / 3;
        const auto add_error = xatlas::AddMesh(atlas.get(), declaration);
        if (add_error != xatlas::AddMeshError::Success) {
            throw std::runtime_error("xatlas rejected the triangle mesh");
        }
        xatlas::PackOptions pack;
        pack.resolution = resolution;
        pack.padding = padding;
        pack.bilinear = true;
        pack.blockAlign = true;
        pack.createImage = true;
        pack.rotateCharts = true;
        pack.rotateChartsToAxis = true;
        xatlas::Generate(atlas.get(), {}, pack);
        if (atlas->atlasCount != 1 || atlas->meshCount != 1 || !atlas->image || !atlas->meshes) {
            throw std::runtime_error("xatlas could not produce one packed image atlas");
        }
        const xatlas::Mesh &mesh = atlas->meshes[0];
        if (!mesh.vertexArray || !mesh.indexArray || mesh.indexCount != index_count || mesh.indexCount % 3 != 0) {
            throw std::runtime_error("xatlas output did not preserve triangle count");
        }
        std::vector<std::array<float, 2>> atlas_uv(mesh.vertexCount);
        std::vector<uint32_t> xref(mesh.vertexCount), chart_ids(mesh.vertexCount);
        std::vector<uint32_t> atlas_indices(mesh.indexArray, mesh.indexArray + mesh.indexCount);
        for (uint32_t i = 0; i < mesh.vertexCount; ++i) {
            xref[i] = mesh.vertexArray[i].xref;
            if (mesh.vertexArray[i].chartIndex < 0)
                throw std::runtime_error("xatlas returned an invalid chart id");
            chart_ids[i] = static_cast<uint32_t>(mesh.vertexArray[i].chartIndex);
            atlas_uv[i] = {mesh.vertexArray[i].uv[0] / atlas->width, mesh.vertexArray[i].uv[1] / atlas->height};
        }
        std::vector<std::array<float, 2>> source_uv(vertex_count);
        for (uint32_t i = 0; i < vertex_count; ++i) source_uv[i] = {original_uv[2 * i], original_uv[2 * i + 1]};
        const int width = static_cast<int>(atlas->width), height = static_cast<int>(atlas->height);
        if (width <= 0 || height <= 0 || uint64_t(width) * uint64_t(height) > 16777216ull)
            throw std::runtime_error("xatlas output exceeds the 16M-texel processing limit");
        BakedTextures baked = bake_textures(source_base, source_mr, source_uv, atlas_uv, xref,
                                            chart_ids, atlas_indices, atlas->image, width, height);
        output->vertex_count = mesh.vertexCount;
        output->index_count = mesh.indexCount;
        output->width = atlas->width;
        output->height = atlas->height;
        output->chart_count = atlas->chartCount;
        std::vector<float> flat_uv(mesh.vertexCount * 2);
        for (uint32_t i = 0; i < mesh.vertexCount; ++i) {
            flat_uv[2 * i] = atlas_uv[i][0];
            flat_uv[2 * i + 1] = atlas_uv[i][1];
        }
        output->uv = copy_array(flat_uv.data(), flat_uv.size());
        output->xref = copy_array(xref.data(), xref.size());
        output->indices = copy_array(atlas_indices.data(), atlas_indices.size());
        output->base_png = copy_array(baked.base.data(), baked.base.size());
        output->base_png_len = baked.base.size();
        output->mr_png = copy_array(baked.mr.data(), baked.mr.size());
        output->mr_png_len = baked.mr.size();
        output->covered_texels = baked.covered;
        output->gutter_texels = baked.gutter;
        return 0;
    } catch (const std::exception &exception) {
        if (output) {
            std::free(output->uv); std::free(output->xref); std::free(output->indices);
            std::free(output->base_png); std::free(output->mr_png);
            std::memset(output, 0, sizeof(*output));
        }
        fail(error, error_capacity, exception.what());
        return 1;
    } catch (...) {
        if (output) std::memset(output, 0, sizeof(*output));
        fail(error, error_capacity, "unknown native atlas error");
        return 2;
    }
}

extern "C" void meshharbor_atlas_free(MhAtlasResult *output) {
    if (!output) return;
    std::free(output->uv); std::free(output->xref); std::free(output->indices);
    std::free(output->base_png); std::free(output->mr_png);
    std::memset(output, 0, sizeof(*output));
}
