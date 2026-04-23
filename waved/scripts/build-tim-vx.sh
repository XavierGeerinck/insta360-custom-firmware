#!/usr/bin/env bash
# SP#4 Task 2: cross-compile TIM-VX for aarch64 against the device-pulled
# Vivante SDK. Output: target/tim-vx/libtim-vx.so (aarch64).
#
# Context: the A113X2 chip isn't in TIM-VX's prebuilt target list (A311D,
# S905D3). We build with EXTERNAL_VIV_SDK pointing at third_party/device-sdk/
# which combines generic headers from TIM-VX's x86_64 prebuilt SDK with
# aarch64 .so files pulled from the device. Build runs inside the cross-rs
# aarch64 docker image so we don't need an aarch64 toolchain on the host.
#
# libCLC.so is aliased to libOpenCL.so on this stack — the CLC JIT compiler
# is folded into the OpenCL runtime on this chip revision.
#
# Prereqs:
#   third_party/TIM-VX/           — TIM-VX source (git clone)
#   third_party/device-sdk/       — headers from TIM-VX prebuilt + aarch64
#                                   libs pulled via `adb pull /usr/lib/...`
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TIM_VX_SRC="$REPO_ROOT/third_party/TIM-VX"
SDK="$REPO_ROOT/third_party/device-sdk"
OUT="$REPO_ROOT/target/tim-vx"
BUILD="$REPO_ROOT/target/tim-vx-build"

if [[ ! -d "$TIM_VX_SRC" ]]; then
    echo "ERROR: $TIM_VX_SRC missing — run:"
    echo "  git clone --depth 50 https://github.com/VeriSilicon/TIM-VX.git $TIM_VX_SRC"
    exit 1
fi
if [[ ! -f "$SDK/lib/libOpenVX.so" ]]; then
    echo "ERROR: $SDK/lib/libOpenVX.so missing — pull via adb:"
    echo "  for lib in libArchModelSw libGAL libNNArchPerf libOpenCL libOpenVX libOpenVXU libVSC libovxlib; do"
    echo "      adb pull /usr/lib/\$lib.so $SDK/lib/"
    echo "  done"
    exit 1
fi

mkdir -p "$OUT" "$BUILD"

echo "==> building TIM-VX (aarch64) via cross-rs docker..."
docker run --rm \
    -v "$REPO_ROOT":/work \
    -w /work \
    ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main \
    bash -c '
        set -euo pipefail
        export DEBIAN_FRONTEND=noninteractive
        apt-get -qq update
        apt-get -qq install -y --no-install-recommends cmake ninja-build
        cd /work/target/tim-vx-build
        rm -rf CMakeCache.txt CMakeFiles
        cmake /work/third_party/TIM-VX \
            -G Ninja \
            -DCMAKE_SYSTEM_NAME=Linux \
            -DCMAKE_SYSTEM_PROCESSOR=aarch64 \
            -DCMAKE_C_COMPILER=aarch64-linux-gnu-gcc \
            -DCMAKE_CXX_COMPILER=aarch64-linux-gnu-g++ \
            -DBUILD_SHARED_LIBS=ON \
            -DTIM_VX_ENABLE_TEST=OFF \
            -DTIM_VX_BUILD_EXAMPLES=OFF \
            -DEXTERNAL_VIV_SDK=/work/third_party/device-sdk \
            -DCMAKE_BUILD_TYPE=Release
        ninja -v 2>&1 | tail -80
        cp -v /work/target/tim-vx-build/src/tim/libtim-vx.so /work/target/tim-vx/
    '

echo ""
echo "==> built:"
ls -la "$OUT/"
file "$OUT/libtim-vx.so" 2>/dev/null || echo "(file command not available, skipping)"
