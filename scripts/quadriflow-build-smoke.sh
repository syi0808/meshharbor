#!/usr/bin/env bash
set -euo pipefail

readonly QUADRIFLOW_COMMIT="810b7a0967c35b0dc85b4464e3835e26a756c967"
readonly UPSTREAM_DIR="${RUNNER_TEMP}/quadriflow-upstream"
readonly BUILD_DIR="${RUNNER_TEMP}/quadriflow-build"
readonly OUTPUT_DIR="${RUNNER_TEMP}/quadriflow-smoke"
readonly OUTPUT_OBJ="${OUTPUT_DIR}/cube-quad.obj"

mkdir -p "${OUTPUT_DIR}"
git init "${UPSTREAM_DIR}"
git -C "${UPSTREAM_DIR}" remote add origin https://github.com/hjwdzh/QuadriFlow.git
git -C "${UPSTREAM_DIR}" fetch --depth=1 origin "${QUADRIFLOW_COMMIT}"
git -C "${UPSTREAM_DIR}" checkout --detach FETCH_HEAD
test "$(git -C "${UPSTREAM_DIR}" rev-parse HEAD)" = "${QUADRIFLOW_COMMIT}"

cmake -S "${UPSTREAM_DIR}" -B "${BUILD_DIR}" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_FREE_LICENSE=ON \
  -DBUILD_OPENMP=OFF \
  -DBUILD_TBB=OFF \
  -DBUILD_GUROBI=OFF \
  -DBUILD_LOG=OFF \
  -DBUILD_PERFORMANCE_TEST=OFF \
  -DLEMON_ENABLE_GLPK=OFF \
  -DLEMON_ENABLE_ILOG=OFF \
  -DLEMON_ENABLE_COIN=OFF \
  -DLEMON_ENABLE_SOPLEX=OFF
cmake --build "${BUILD_DIR}" --parallel 2

# One approved run on a synthetic cube only. Never enable -sat or pass user flags.
timeout 180 "${BUILD_DIR}/quadriflow" \
  -i "${GITHUB_WORKSPACE}/crates/core/tests/fixtures/quadriflow_cube_triangles.obj" \
  -o "${OUTPUT_OBJ}" \
  -f 12 -seed 0

MESHHARBOR_QUADRIFLOW_OBJ_OUTPUT="${OUTPUT_OBJ}" \
  cargo test -p meshharbor-core --locked --test quadriflow_output -- --ignored --nocapture
