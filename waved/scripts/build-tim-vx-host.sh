#!/usr/bin/env bash
# SP#4: build TIM-VX for x86_64 Linux host using its prebuilt x86_64 SDK.
# Output: target/tim-vx-host/libtim-vx.so + samples.
#
# The x86_64 SDK ships with runtime shaders + the OpenCL compile path that
# the device firmware strips, so graph->Compile() and graph->CompileToBinary()
# work here. We use this to produce .nb files ahead of time that the device
# can load via ops::NBG.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TIM_VX_SRC="$REPO_ROOT/third_party/TIM-VX"
OUT="$REPO_ROOT/target/tim-vx-host"
BUILD="$REPO_ROOT/target/tim-vx-host-build"

mkdir -p "$OUT" "$BUILD"

echo "==> building TIM-VX for x86_64 host..."
docker run --rm --platform=linux/amd64 \
    -v "$REPO_ROOT":/work \
    -w /work \
    ubuntu:22.04 \
    bash -c '
        set -euo pipefail
        export DEBIAN_FRONTEND=noninteractive
        apt-get -qq update
        apt-get -qq install -y --no-install-recommends \
            cmake ninja-build g++ libc6-dev
        rm -rf /work/target/tim-vx-host-build
        cmake /work/third_party/TIM-VX \
            -B /work/target/tim-vx-host-build \
            -G Ninja \
            -DBUILD_SHARED_LIBS=ON \
            -DTIM_VX_ENABLE_TEST=OFF \
            -DTIM_VX_BUILD_EXAMPLES=ON \
            -DTIM_VX_ENABLE_NBG_PARSER=ON \
            -DCMAKE_BUILD_TYPE=Release
        ninja -C /work/target/tim-vx-host-build -v 2>&1 | tail -8
        cp -v /work/target/tim-vx-host-build/src/tim/libtim-vx.so /work/target/tim-vx-host/
    '
echo "==> built:"
ls -la "$OUT/"
