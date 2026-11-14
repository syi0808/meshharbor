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
    uint64_t projection_attempted, projection_projected, projection_ambiguous;
    float projection_max_distance, projection_distance_limit;
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
    uint64_t projection_attempted = 0, projection_projected = 0, projection_ambiguous = 0;
    float projection_max_distance = 0.0f, projection_distance_limit = 0.0f;
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

using Vec3 = std::array<float, 3>;

Vec3 sub3(const Vec3 &a, const Vec3 &b) { return {a[0] - b[0], a[1] - b[1], a[2] - b[2]}; }
Vec3 add3(const Vec3 &a, const Vec3 &b) { return {a[0] + b[0], a[1] + b[1], a[2] + b[2]}; }
Vec3 scale3(const Vec3 &a, float t) { return {a[0] * t, a[1] * t, a[2] * t}; }
float dot3(const Vec3 &a, const Vec3 &b) { return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]; }
Vec3 cross3(const Vec3 &a, const Vec3 &b) {
    return {a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]};
}
Vec3 normalize3(const Vec3 &a) {
    const float length = std::sqrt(dot3(a, a));
    if (!std::isfinite(length) || length <= 1.0e-12f) return {0, 0, 0};
    return scale3(a, 1.0f / length);
}

struct ProjectionTriangle {
    Vec3 lo, hi, center, normal;
    uint32_t a, b, c;
};
struct ProjectionNode {
    Vec3 lo, hi;
    uint32_t begin = 0, end = 0, left = 0, right = 0;
    bool leaf = false;
};
struct ProjectionHit {
    uint32_t triangle = 0;
    std::array<float, 3> bary{};
    Vec3 position{};
    float distance2 = std::numeric_limits<float>::infinity();
    float normal_dot = -1.0f;
};

float point_box_distance2(const Vec3 &p, const Vec3 &lo, const Vec3 &hi) {
    float sum = 0.0f;
    for (int axis = 0; axis < 3; ++axis) {
        const float d = p[axis] < lo[axis] ? lo[axis] - p[axis] : p[axis] > hi[axis] ? p[axis] - hi[axis] : 0.0f;
        sum += d * d;
    }
    return sum;
}

std::pair<Vec3, std::array<float, 3>> closest_point_triangle(
    const Vec3 &p, const Vec3 &a, const Vec3 &b, const Vec3 &c) {
    const Vec3 ab = sub3(b, a), ac = sub3(c, a), ap = sub3(p, a);
    const float d1 = dot3(ab, ap), d2 = dot3(ac, ap);
    if (d1 <= 0 && d2 <= 0) return {a, {1, 0, 0}};
    const Vec3 bp = sub3(p, b);
    const float d3 = dot3(ab, bp), d4 = dot3(ac, bp);
    if (d3 >= 0 && d4 <= d3) return {b, {0, 1, 0}};
    const float vc = d1 * d4 - d3 * d2;
    if (vc <= 0 && d1 >= 0 && d3 <= 0) {
        const float v = d1 / (d1 - d3);
        return {add3(a, scale3(ab, v)), {1 - v, v, 0}};
    }
    const Vec3 cp = sub3(p, c);
    const float d5 = dot3(ab, cp), d6 = dot3(ac, cp);
    if (d6 >= 0 && d5 <= d6) return {c, {0, 0, 1}};
    const float vb = d5 * d2 - d1 * d6;
    if (vb <= 0 && d2 >= 0 && d6 <= 0) {
        const float w = d2 / (d2 - d6);
        return {add3(a, scale3(ac, w)), {1 - w, 0, w}};
    }
    const float va = d3 * d6 - d5 * d4;
    if (va <= 0 && d4 - d3 >= 0 && d5 - d6 >= 0) {
        const float w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return {add3(b, scale3(sub3(c, b), w)), {0, 1 - w, w}};
    }
    const float denominator = 1.0f / (va + vb + vc);
    const float v = vb * denominator, w = vc * denominator;
    return {add3(a, add3(scale3(ab, v), scale3(ac, w))), {1 - v - w, v, w}};
}

class ProjectionBvh {
public:
    ProjectionBvh(const float *positions, const uint32_t *indices, uint32_t index_count)
        : positions_(positions), indices_(indices) {
        const uint32_t face_count = index_count / 3;
        triangles_.reserve(face_count);
        order_.reserve(face_count);
        for (uint32_t face = 0; face < face_count; ++face) {
            const uint32_t a = indices[face * 3], b = indices[face * 3 + 1], c = indices[face * 3 + 2];
            const Vec3 pa = position(a), pb = position(b), pc = position(c);
            const Vec3 raw_normal = cross3(sub3(pb, pa), sub3(pc, pa));
            const Vec3 normal = normalize3(raw_normal);
            if (dot3(normal, normal) < 0.5f) continue;
            ProjectionTriangle triangle;
            triangle.a = a; triangle.b = b; triangle.c = c; triangle.normal = normal;
            for (int axis = 0; axis < 3; ++axis) {
                triangle.lo[axis] = std::min({pa[axis], pb[axis], pc[axis]});
                triangle.hi[axis] = std::max({pa[axis], pb[axis], pc[axis]});
                triangle.center[axis] = (pa[axis] + pb[axis] + pc[axis]) / 3.0f;
            }
            order_.push_back(static_cast<uint32_t>(triangles_.size()));
            triangles_.push_back(triangle);
        }
        if (triangles_.empty()) throw std::runtime_error("source mesh contains no non-degenerate projection triangles");
        nodes_.reserve(triangles_.size() * 2);
        build(0, static_cast<uint32_t>(order_.size()));
    }

    struct Result { ProjectionHit nearest; float opposing_distance = std::numeric_limits<float>::infinity(); };
    Result query(const Vec3 &point, const Vec3 &target_normal, float max_distance, float ambiguity_tolerance) const {
        Result result;
        query_node(0, point, target_normal, max_distance * max_distance, ambiguity_tolerance, result);
        return result;
    }
    const ProjectionTriangle &triangle(uint32_t index) const { return triangles_[index]; }
    Vec3 position(uint32_t index) const {
        return {positions_[size_t(index) * 3], positions_[size_t(index) * 3 + 1], positions_[size_t(index) * 3 + 2]};
    }
private:
    uint32_t build(uint32_t begin, uint32_t end) {
        const uint32_t node_index = static_cast<uint32_t>(nodes_.size());
        nodes_.push_back({});
        ProjectionNode node;
        node.begin = begin; node.end = end;
        node.lo = {std::numeric_limits<float>::infinity(), std::numeric_limits<float>::infinity(), std::numeric_limits<float>::infinity()};
        node.hi = {-std::numeric_limits<float>::infinity(), -std::numeric_limits<float>::infinity(), -std::numeric_limits<float>::infinity()};
        Vec3 center_lo = node.lo, center_hi = node.hi;
        for (uint32_t i = begin; i < end; ++i) {
            const auto &tri = triangles_[order_[i]];
            for (int axis = 0; axis < 3; ++axis) {
                node.lo[axis] = std::min(node.lo[axis], tri.lo[axis]);
                node.hi[axis] = std::max(node.hi[axis], tri.hi[axis]);
                center_lo[axis] = std::min(center_lo[axis], tri.center[axis]);
                center_hi[axis] = std::max(center_hi[axis], tri.center[axis]);
            }
        }
        if (end - begin <= 8) {
            node.leaf = true;
        } else {
            int axis = 0;
            if (center_hi[1] - center_lo[1] > center_hi[axis] - center_lo[axis]) axis = 1;
            if (center_hi[2] - center_lo[2] > center_hi[axis] - center_lo[axis]) axis = 2;
            const uint32_t middle = begin + (end - begin) / 2;
            std::nth_element(order_.begin() + begin, order_.begin() + middle, order_.begin() + end,
                [&](uint32_t left, uint32_t right) { return triangles_[left].center[axis] < triangles_[right].center[axis]; });
            node.left = build(begin, middle);
            node.right = build(middle, end);
        }
        nodes_[node_index] = node;
        return node_index;
    }
    void query_node(uint32_t index, const Vec3 &point, const Vec3 &target_normal, float limit2,
                    float ambiguity_tolerance, Result &result) const {
        const ProjectionNode &node = nodes_[index];
        const float bound = point_box_distance2(point, node.lo, node.hi);
        const float best_distance = std::isfinite(result.nearest.distance2)
            ? std::sqrt(result.nearest.distance2) : std::sqrt(limit2);
        const float search_distance = std::min(std::sqrt(limit2), best_distance + ambiguity_tolerance);
        if (bound > search_distance * search_distance) return;
        if (node.leaf) {
            for (uint32_t i = node.begin; i < node.end; ++i) {
                const uint32_t tri_index = order_[i];
                const auto &tri = triangles_[tri_index];
                const auto closest = closest_point_triangle(point, position(tri.a), position(tri.b), position(tri.c));
                const float distance2 = dot3(sub3(closest.first, point), sub3(closest.first, point));
                if (distance2 > limit2) continue;
                const float normal_dot = dot3(target_normal, tri.normal);
                if (normal_dot < -0.25f)
                    result.opposing_distance = std::min(result.opposing_distance, std::sqrt(distance2));
                if (distance2 < result.nearest.distance2) {
                    result.nearest = {tri_index, closest.second, closest.first, distance2, normal_dot};
                }
            }
            return;
        }
        const float left_distance = point_box_distance2(point, nodes_[node.left].lo, nodes_[node.left].hi);
        const float right_distance = point_box_distance2(point, nodes_[node.right].lo, nodes_[node.right].hi);
        if (left_distance <= right_distance) {
            query_node(node.left, point, target_normal, limit2, ambiguity_tolerance, result);
            query_node(node.right, point, target_normal, limit2, ambiguity_tolerance, result);
        } else {
            query_node(node.right, point, target_normal, limit2, ambiguity_tolerance, result);
            query_node(node.left, point, target_normal, limit2, ambiguity_tolerance, result);
        }
    }
    const float *positions_;
    const uint32_t *indices_;
    std::vector<ProjectionTriangle> triangles_;
    std::vector<uint32_t> order_;
    std::vector<ProjectionNode> nodes_;
};

BakedTextures bake_projected_textures(
    const Image &source_base, const Image &source_mr,
    const float *target_positions, const float *target_normals, uint32_t target_vertex_count,
    const float *source_positions, const float *source_uv, uint32_t source_vertex_count,
    const uint32_t *source_indices, uint32_t source_index_count,
    const std::vector<std::array<float, 2>> &atlas_uv,
    const std::vector<uint32_t> &xref, const std::vector<uint32_t> &chart_ids,
    const std::vector<uint32_t> &indices, const uint32_t *atlas_image,
    int width, int height, float max_distance_ratio, float min_coverage) {
    if (source_index_count / 3 > 500000) throw std::runtime_error("source projection is limited to 500,000 triangles");
    if (!target_normals || !source_positions || !source_uv || !source_indices)
        throw std::runtime_error("projected rebaking requires target normals and source mesh streams");
    ProjectionBvh bvh(source_positions, source_indices, source_index_count);
    Vec3 source_lo = {std::numeric_limits<float>::infinity(), std::numeric_limits<float>::infinity(), std::numeric_limits<float>::infinity()};
    Vec3 source_hi = {-source_lo[0], -source_lo[1], -source_lo[2]};
    for (uint32_t i = 0; i < source_vertex_count; ++i) for (int axis = 0; axis < 3; ++axis) {
        const float value = source_positions[size_t(i) * 3 + axis];
        source_lo[axis] = std::min(source_lo[axis], value);
        source_hi[axis] = std::max(source_hi[axis], value);
    }
    const float diagonal = std::sqrt(dot3(sub3(source_hi, source_lo), sub3(source_hi, source_lo)));
    if (!std::isfinite(diagonal) || diagonal <= 1.0e-12f) throw std::runtime_error("source projection bounds are degenerate");
    const float distance_limit = diagonal * max_distance_ratio;
    const float ambiguity_tolerance = std::max(distance_limit * 0.1f, diagonal * 1.0e-6f);
    Image output_base{width, height, std::vector<uint8_t>(size_t(width) * height * 4, 0)};
    Image output_mr{width, height, std::vector<uint8_t>(size_t(width) * height * 4, 0)};
    std::vector<uint8_t> covered(size_t(width) * height, 0);
    std::vector<uint32_t> pixel_chart(size_t(width) * height, UINT32_MAX);
    BakedTextures result;
    result.projection_distance_limit = distance_limit;
    uint64_t no_normal = 0, no_hit = 0, backface = 0;
    for (size_t face = 0; face < indices.size() / 3; ++face) {
        const uint32_t ids[3] = {indices[face * 3], indices[face * 3 + 1], indices[face * 3 + 2]};
        if (ids[0] >= xref.size() || ids[1] >= xref.size() || ids[2] >= xref.size())
            throw std::runtime_error("xatlas returned an out-of-range target index");
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
            ++result.projection_attempted;
            const uint32_t target_ids[3] = {xref[ids[0]], xref[ids[1]], xref[ids[2]]};
            if (target_ids[0] >= target_vertex_count || target_ids[1] >= target_vertex_count || target_ids[2] >= target_vertex_count)
                throw std::runtime_error("xatlas returned an out-of-range target vertex reference");
            auto target_position = [&](int corner) {
                const uint32_t id = target_ids[corner];
                return Vec3{target_positions[size_t(id) * 3], target_positions[size_t(id) * 3 + 1], target_positions[size_t(id) * 3 + 2]};
            };
            auto target_normal = [&](int corner) {
                const uint32_t id = target_ids[corner];
                return Vec3{target_normals[size_t(id) * 3], target_normals[size_t(id) * 3 + 1], target_normals[size_t(id) * 3 + 2]};
            };
            const Vec3 target_point = add3(scale3(target_position(0), a), add3(scale3(target_position(1), b), scale3(target_position(2), c)));
            const Vec3 normal = normalize3(add3(scale3(target_normal(0), a), add3(scale3(target_normal(1), b), scale3(target_normal(2), c))));
            if (dot3(normal, normal) < 0.5f) { ++no_normal; continue; }
            const auto hit = bvh.query(target_point, normal, distance_limit, ambiguity_tolerance);
            if (!std::isfinite(hit.nearest.distance2)) {
                ++no_hit;
                continue;
            }
            const float hit_distance = std::sqrt(hit.nearest.distance2);
            if (hit.nearest.normal_dot < 0.25f) { ++backface; continue; }
            if (std::isfinite(hit.opposing_distance) && std::abs(hit.opposing_distance - hit_distance) <= ambiguity_tolerance) {
                ++result.projection_ambiguous;
                continue;
            }
            const auto &source_triangle = bvh.triangle(hit.nearest.triangle);
            const uint32_t tri_ids[3] = {source_triangle.a, source_triangle.b, source_triangle.c};
            const float u = hit.nearest.bary[0] * source_uv[size_t(tri_ids[0]) * 2] + hit.nearest.bary[1] * source_uv[size_t(tri_ids[1]) * 2] + hit.nearest.bary[2] * source_uv[size_t(tri_ids[2]) * 2];
            const float v = hit.nearest.bary[0] * source_uv[size_t(tri_ids[0]) * 2 + 1] + hit.nearest.bary[1] * source_uv[size_t(tri_ids[1]) * 2 + 1] + hit.nearest.bary[2] * source_uv[size_t(tri_ids[2]) * 2 + 1];
            write_pixel(output_base, pixel, sample(source_base, u, v, true));
            write_pixel(output_mr, pixel, sample(source_mr, u, v, false));
            covered[pixel] = 1;
            pixel_chart[pixel] = chart;
            ++result.projection_projected;
            result.projection_max_distance = std::max(result.projection_max_distance, hit_distance);
        }
    }
    if (result.projection_attempted == 0) throw std::runtime_error("target atlas contains no rasterized texels");
    if (result.projection_ambiguous > 0) throw std::runtime_error("texture projection encountered ambiguous opposing surfaces; refusing to bake thin shells");
    const double coverage = static_cast<double>(result.projection_projected) / static_cast<double>(result.projection_attempted);
    if (coverage < min_coverage) throw std::runtime_error(
        "texture projection coverage " + std::to_string(coverage) + " (" +
        std::to_string(result.projection_projected) + "/" + std::to_string(result.projection_attempted) +
        " texels) is below the configured safety threshold [no-normal=" + std::to_string(no_normal) +
        ", no-hit=" + std::to_string(no_hit) + ", backface=" + std::to_string(backface) +
        "]");
    std::vector<int32_t> owner(size_t(width) * height, -1);
    for (size_t i = 0; i < owner.size(); ++i)
        if (atlas_image[i] & xatlas::kImageHasChartIndexBit)
            owner[i] = static_cast<int32_t>(atlas_image[i] & xatlas::kImageChartIndexMask);
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
    uint32_t resolution, uint32_t padding,
    const float *source_positions, const float *source_uv, uint32_t source_vertex_count,
    const uint32_t *source_indices, uint32_t source_index_count,
    float max_distance_ratio, float min_coverage,
    MhAtlasResult *output, char *error, size_t error_capacity) {
    if (output) std::memset(output, 0, sizeof(*output));
    try {
        const bool projected = source_positions || source_uv || source_indices || source_vertex_count || source_index_count;
        if (!output || !positions || !indices || vertex_count == 0 || index_count == 0 || index_count % 3 != 0
            || (!projected && !original_uv) || (projected && (!normals || !source_positions || !source_uv || !source_indices
                || source_vertex_count == 0 || source_index_count == 0 || source_index_count % 3 != 0)))
            throw std::runtime_error("invalid atlas input arrays");
        if (resolution < 64 || resolution > 4096 || padding > 64)
            throw std::runtime_error("resolution must be 64..4096 and padding 0..64");
        if (uint64_t(resolution) * resolution > 16777216ull)
            throw std::runtime_error("requested atlas exceeds the 16M-texel safety limit");
        if (projected && (!std::isfinite(max_distance_ratio) || max_distance_ratio < 0.0001f || max_distance_ratio > 0.05f
            || !std::isfinite(min_coverage) || min_coverage < 0.95f || min_coverage > 1.0f
            || source_index_count / 3 > 500000))
            throw std::runtime_error("projection limits exceed the safe distance, coverage, or triangle bounds");
        for (uint32_t i = 0; i < index_count; ++i)
            if (indices[i] >= vertex_count) throw std::runtime_error("source index is out of range");
        for (uint32_t i = 0; i < vertex_count * 3ull; ++i)
            if (!std::isfinite(positions[i]) || (normals && !std::isfinite(normals[i])))
                throw std::runtime_error("target positions and normals must be finite");
        if (original_uv) for (uint32_t i = 0; i < vertex_count * 2ull; ++i)
            if (!std::isfinite(original_uv[i])) throw std::runtime_error("source texture coordinates must be finite");
        if (projected) {
            for (uint32_t i = 0; i < source_index_count; ++i)
                if (source_indices[i] >= source_vertex_count) throw std::runtime_error("projection source index is out of range");
            for (uint32_t i = 0; i < source_vertex_count * 3ull; ++i)
                if (!std::isfinite(source_positions[i])) throw std::runtime_error("projection source positions must be finite");
            for (uint32_t i = 0; i < source_vertex_count * 2ull; ++i)
                if (!std::isfinite(source_uv[i])) throw std::runtime_error("projection source UVs must be finite");
        }
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
        if (!mesh.vertexArray || !mesh.indexArray || mesh.vertexCount > index_count
            || mesh.indexCount != index_count || mesh.indexCount % 3 != 0) {
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
        const int width = static_cast<int>(atlas->width), height = static_cast<int>(atlas->height);
        if (width <= 0 || height <= 0 || uint64_t(width) * uint64_t(height) > 16777216ull)
            throw std::runtime_error("xatlas output exceeds the 16M-texel processing limit");
        std::vector<std::array<float, 2>> direct_uv;
        if (!projected) {
            direct_uv.resize(vertex_count);
            for (uint32_t i = 0; i < vertex_count; ++i) direct_uv[i] = {original_uv[2 * i], original_uv[2 * i + 1]};
        }
        BakedTextures baked = projected
            ? bake_projected_textures(source_base, source_mr, positions, normals, vertex_count,
                source_positions, source_uv, source_vertex_count, source_indices, source_index_count,
                atlas_uv, xref, chart_ids, atlas_indices, atlas->image, width, height,
                max_distance_ratio, min_coverage)
            : bake_textures(source_base, source_mr, direct_uv, atlas_uv, xref,
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
        output->projection_attempted = baked.projection_attempted;
        output->projection_projected = baked.projection_projected;
        output->projection_ambiguous = baked.projection_ambiguous;
        output->projection_max_distance = baked.projection_max_distance;
        output->projection_distance_limit = baked.projection_distance_limit;
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
