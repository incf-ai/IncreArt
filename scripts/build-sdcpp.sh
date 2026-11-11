#!/usr/bin/env bash
# Build stable-diffusion.cpp's sd-server inside the devcontainer.
#
#   scripts/build-sdcpp.sh            # HIP (gfx1201) only
#   scripts/build-sdcpp.sh hip vulkan # both, for benchmarking
#
# Source and build trees live on /mnt/storage/sdcpp so they survive container
# rebuilds. Binaries end up in $SDCPP_ROOT/build-<backend>/bin/sd-server.
set -euo pipefail

SDCPP_ROOT=${SDCPP_ROOT:-/mnt/storage/sdcpp}
SDCPP_REPO=${SDCPP_REPO:-https://github.com/leejet/stable-diffusion.cpp}
# Known-good commit; bump deliberately after checking docs/qwen_image_2.1.md
# and examples/server/api.md for API changes.
SDCPP_REV=${SDCPP_REV:-32d516c0197ed5ca31305b8575e14eb1451fad33}
GPU_TARGETS=${GPU_TARGETS:-gfx1201}

backends=("$@")
[ ${#backends[@]} -eq 0 ] && backends=(hip)

src="$SDCPP_ROOT/src"
if [ ! -d "$src/.git" ]; then
    git clone "$SDCPP_REPO" "$src"
fi
git -C "$src" fetch --quiet origin
git -C "$src" checkout --quiet "$SDCPP_REV"
git -C "$src" submodule update --init --recursive

common=(-G Ninja -DCMAKE_BUILD_TYPE=Release
        -DCMAKE_C_COMPILER_LAUNCHER=ccache -DCMAKE_CXX_COMPILER_LAUNCHER=ccache)
# The embedded web UI is skipped automatically when pnpm is missing.
command -v pnpm >/dev/null || common+=(-DSD_SERVER_BUILD_FRONTEND=OFF)

for backend in "${backends[@]}"; do
    build="$SDCPP_ROOT/build-$backend"
    case "$backend" in
        hip)
            cmake -S "$src" -B "$build" "${common[@]}" \
                -DCMAKE_C_COMPILER=/opt/rocm/llvm/bin/clang \
                -DCMAKE_CXX_COMPILER=/opt/rocm/llvm/bin/clang++ \
                -DSD_HIPBLAS=ON \
                -DGPU_TARGETS="$GPU_TARGETS" -DAMDGPU_TARGETS="$GPU_TARGETS"
            ;;
        vulkan)
            cmake -S "$src" -B "$build" "${common[@]}" -DSD_VULKAN=ON
            ;;
        *)
            echo "unknown backend: $backend (expected hip or vulkan)" >&2
            exit 2
            ;;
    esac
    cmake --build "$build" --target sd-server sd-cli -j "${CARGO_BUILD_JOBS:-16}"
    echo "built $build/bin/sd-server"
done
