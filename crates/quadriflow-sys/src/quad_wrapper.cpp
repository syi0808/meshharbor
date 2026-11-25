#include "optimizer.hpp"
#include "parametrizer.hpp"
#include <cstddef>
#include <cstdint>

extern "C" int meshharbor_quadriflow_run(
    const float* positions, std::size_t vertex_count,
    const std::uint32_t* indices, std::size_t triangle_count,
    std::uint32_t target_quads) noexcept {
    try {
        qflow::Parametrizer field;
        field.hierarchy.rng_seed = 0;
        field.V = Eigen::Map<const Eigen::Matrix<float, 3, Eigen::Dynamic, Eigen::ColMajor>>(
            positions, 3, vertex_count).cast<double>();
        field.F = Eigen::Map<const Eigen::Matrix<std::uint32_t, 3, Eigen::Dynamic, Eigen::ColMajor>>(
            indices, 3, triangle_count).cast<int>();
        field.NormalizeMesh();
        field.Initialize(static_cast<int>(target_quads));
        qflow::Optimizer::optimize_orientations(field.hierarchy);
        field.ComputeOrientationSingularities();
        qflow::Optimizer::optimize_scale(field.hierarchy, field.rho, field.flag_adaptive_scale);
        field.flag_adaptive_scale = 1;
        qflow::Optimizer::optimize_positions(field.hierarchy, field.flag_adaptive_scale);
        field.ComputePositionSingularities();
        field.ComputeIndexMap();
        field.OutputMesh("output.obj");
        return 0;
    } catch (...) {
        return 1;
    }
}
