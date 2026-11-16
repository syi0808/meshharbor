#!/usr/bin/env bash
# Unsigned experiment. Build without executing QuadriFlow or uploading artifacts.
set -euo pipefail

if [[ $# != 1 || ! $1 =~ ^(primary|independent)$ ]]; then
  echo 'usage: bash scripts/quadriflow-linux-reproduction.sh {primary|independent}' >&2
  exit 2
fi
root=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/quad-reproduction-$1.XXXXXXXX")
trap 'rm -rf -- "$root"' EXIT
root=$(realpath "$root")
export LC_ALL=C TZ=UTC SOURCE_DATE_EPOCH=0 ZERO_AR_DATE=1
unset CFLAGS CXXFLAGS CPPFLAGS LDFLAGS CMAKE_PREFIX_PATH CPATH CPLUS_INCLUDE_PATH C_INCLUDE_PATH LIBRARY_PATH

# Package bytes independently pinned against Canonical's signed Noble snapshot
# (20261115T135800Z). Extract only, without package scripts or root privileges.
snapshot=https://snapshot.ubuntu.com/ubuntu/20261115T135800Z
fetch_deb() {
  local name=$1 path=$2 size=$3 sha=$4
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 15 --max-time 180 --retry 2 --max-filesize "$size" \
    --output "$root/$name.deb" "$snapshot/$path"
  test "$(stat -c %s "$root/$name.deb")" = "$size"
  printf '%s  %s\n' "$sha" "$root/$name.deb" | sha256sum --check --status
  dpkg-deb --extract "$root/$name.deb" "$root/$name"
}
fetch_deb boost pool/main/b/boost1.83/libboost1.83-dev_1.83.0-2.1ubuntu3.2_amd64.deb \
  10745362 519ecf2c64308527e15b6582955681d192a832250baa4bc424967aaf7d02d68f
fetch_deb eigen pool/universe/e/eigen3/libeigen3-dev_3.4.0-4build0.1_all.deb \
  1056510 56c05756bd5bf1e7d1c30ea545786a51684b3c796f8289b20cc803b2f1e6f754

# Reuse the reviewed source pin and CMake option list, not the signing/build path.
python3 - "$root/recipe" <<'PY'
import runpy
import sys
from pathlib import Path

recipe = runpy.run_path("scripts/quadriflow-platform-smoke.py")
Path(sys.argv[1]).write_text("\n".join([
    recipe["QUADRIFLOW_REPOSITORY"], recipe["QUADRIFLOW_COMMIT"], *recipe["BUILD_OPTIONS"]
]) + "\n", encoding="utf-8")
PY
mapfile -t recipe < "$root/recipe"
source="$root/upstream"
build="$root/build"
git init --quiet "$source"
git -C "$source" fetch --quiet --depth=1 "${recipe[0]}" "${recipe[1]}"
git -C "$source" checkout --quiet --detach FETCH_HEAD
test "$(git -C "$source" rev-parse HEAD)" = "${recipe[1]}"

cc=$(command -v gcc)
cxx=$(command -v g++)
cmake -S "$source" -B "$build" -G 'Unix Makefiles' "${recipe[@]:2}" \
  -DCMAKE_C_COMPILER="$cc" -DCMAKE_CXX_COMPILER="$cxx" \
  -DCMAKE_C_FLAGS="-ffile-prefix-map=$root=/meshharbor/quadbuild" \
  -DCMAKE_CXX_FLAGS="-ffile-prefix-map=$root=/meshharbor/quadbuild" \
  -DBoost_NO_BOOST_CMAKE=ON -DBoost_NO_SYSTEM_PATHS=ON \
  -DBOOST_INCLUDEDIR="$root/boost/usr/include" \
  -DEIGEN_INCLUDE_DIR="$root/eigen/usr/include/eigen3"
cmake --build "$build" --target quadriflow --parallel 2
if LC_ALL=C grep -aqF -- "$root" "$build/quadriflow"; then
  echo 'Executable still contains the replica-specific build root.' >&2
  exit 1
fi

compiler_root=$(dirname "$(realpath "$("$cxx" -print-file-name=include)")")
headers_root=$(realpath "/usr/include/$("$cxx" -dumpmachine)")
roots=(--root "upstream=$source" --root "gcc=$compiler_root" \
  --root "system=$(realpath /usr/include)" --root "uapi=$headers_root" \
  --root "boost=$root/boost/usr/include" --root "eigen=$root/eigen/usr/include/eigen3")
# Some hosts split kernel UAPI headers outside the multiarch include directory.
if [[ -d /usr/lib/linux/uapi ]]; then
  roots+=(--root "uapi-split=$(realpath /usr/lib/linux/uapi)")
fi
observe() {
  local output=$1 target_dir=$2 dependency_base=$3 units=$4 depfile
  local -a depfiles=()
  while IFS= read -r -d '' depfile; do depfiles+=(--depfile "$depfile"); done \
    < <(find "$target_dir" -type f -name '*.o.d' -print0)
  python3 scripts/quadriflow_compile_inputs.py --build-root "$build" \
    --dependency-base "$dependency_base" "${roots[@]}" --expected-units "$units" \
    "${depfiles[@]}" > "$root/$output"
}
lemon="$build/3rd/lemon-1.3.1/lemon"
observe quadriflow.json "$build/CMakeFiles/quadriflow.dir" "$build" 15
observe lemon.json "$lemon/CMakeFiles/lemon.dir" "$lemon" 7
# Require actual use of both extracted packages rather than only checking the .debs.
grep -q '"boost/' "$root/quadriflow.json"
grep -q '"eigen/' "$root/quadriflow.json"

# The replica output carries only four hashes, never raw reports or native bytes.
# This samples toolchain executables, not a complete build or link closure.
sha() { sha256sum -- "$1" | cut -d ' ' -f 1; }
{
  for tool in "$cc" "$cxx" "$(command -v cmake)" "$(command -v make)" \
    "$("$cc" -print-prog-name=cc1)" "$("$cxx" -print-prog-name=cc1plus)" \
    "$(command -v "$("$cxx" -print-prog-name=as)")" \
    "$(command -v "$("$cxx" -print-prog-name=ld)")" \
    "$(command -v "$("$cxx" -print-prog-name=ar)")"; do
    sha "$tool"
  done
} > "$root/tool-inputs.sha256"
echo "Host image (observation only): ${ImageOS:-unknown} ${ImageVersion:-unknown}"
evidence="elf=$(sha "$build/quadriflow"):quadriflow=$(sha "$root/quadriflow.json"):lemon=$(sha "$root/lemon.json"):tools=$(sha "$root/tool-inputs.sha256")"
printf '%s\n' "$evidence"
if [[ -n ${GITHUB_OUTPUT:-} ]]; then
  printf 'digests=%s\n' "$evidence" >> "$GITHUB_OUTPUT"
fi
