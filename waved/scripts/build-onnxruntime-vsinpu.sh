#!/usr/bin/env bash
# SP#4 Task 3: cross-compile onnxruntime v1.22.0 for aarch64 with the
# VSI-NPU execution provider. Output: target/ort/libonnxruntime.so*.
#
# Env contract (matches cmake/onnxruntime_providers_vsinpu.cmake):
#   TIM_VX_INSTALL     — dir with include/ and lib/libtim-vx.so
#   VIVANTE_SDK_DIR    — dir with lib/libOpenVX.so etc.
#
# Runs inside the cross-rs aarch64 docker image; same pattern as
# build-tim-vx.sh. Build time: 25–40 min first run.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ORT_SRC="$REPO_ROOT/third_party/onnxruntime"
TIM_INSTALL="$REPO_ROOT/target/tim-vx-install"
VIV_SDK="$REPO_ROOT/third_party/device-sdk"
OUT="$REPO_ROOT/target/ort"
BUILD="$REPO_ROOT/target/ort-build"

for p in "$ORT_SRC/cmake" "$TIM_INSTALL/lib/libtim-vx.so" "$VIV_SDK/lib/libOpenVX.so"; do
    if [[ ! -e "$p" ]]; then
        echo "ERROR: required path missing: $p"
        exit 1
    fi
done

mkdir -p "$OUT" "$BUILD"

echo "==> building onnxruntime v1.22.0 + VSI-NPU EP (aarch64)..."
# Using ubuntu:22.04 (not cross-rs) because ORT 1.22.0 build.py requires
# Python >= 3.10 (match/case syntax) and cross-rs ships 20.04 + 3.8. Host
# Ubuntu version doesn't affect the aarch64 output — the cross-compiler
# has its own target sysroot.
docker run --rm --platform=linux/amd64 \
    -v "$REPO_ROOT":/work \
    -w /work \
    -e TIM_VX_INSTALL=/work/target/tim-vx-install \
    -e VIVANTE_SDK_DIR=/work/third_party/device-sdk \
    ubuntu:22.04 \
    bash -c '
        set -euo pipefail
        export DEBIAN_FRONTEND=noninteractive
        apt-get -qq update
        apt-get -qq install -y --no-install-recommends \
            make ninja-build protobuf-compiler git file ca-certificates curl \
            python3 python3-pip python3-setuptools python3-wheel \
            gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
            gcc g++ pkg-config
        # ubuntu:22.04 ships cmake 3.22; ORT 1.22 requires >= 3.28. Install
        # a newer cmake via pip (manylinux wheel ships a binary).
        python3 -m pip install --quiet "cmake>=3.28" numpy packaging
        export PATH="/usr/local/bin:$PATH"
        cmake --version | head -1

        # Always wipe the build dir before configure — cmake refuses to
        # switch generators mid-tree, and past failed runs have left the
        # dir in inconsistent state.
        rm -rf /work/target/ort-build

        # GitLab regenerates archive tarballs non-deterministically; ORT
        # 1.22s pinned eigen SHA1 no longer matches what gitlab serves.
        # Patch with the actual content hash (verified via shasum) so the
        # FetchContent download passes integrity check. Same pattern may be
        # needed for other gitlab-hosted deps if they surface.
        sed -i "s/5ea4d05e62d7f954a46b3213f9b2535bdd866803/51982be81bbe52572b54180454df11a3ece9a934/" \
            /work/third_party/onnxruntime/cmake/deps.txt

        # ORT 1.22.0 VSI-NPU EP source bug: references "logger" where the
        # in-scope name is "logger_". One-line patch until upstream fixes it.
        # Ref: onnxruntime/core/providers/vsinpu/vsinpu_execution_provider.cc:87
        sed -i "s/QDQ::GetAllNodeUnits(graph_viewer, logger)/QDQ::GetAllNodeUnits(graph_viewer, *GetLogger())/" \
            /work/third_party/onnxruntime/onnxruntime/core/providers/vsinpu/vsinpu_execution_provider.cc

        cd /work/third_party/onnxruntime
        # ORT 1.22 build.py dropped --arm64 and --cross_compile; cross-compile
        # is expressed by passing CMAKE_SYSTEM_* + compiler paths via
        # --cmake_extra_defines. The shipped linux_arm64_crosscompile_toolchain
        # .cmake hardcodes aarch64-none-linux-gnu-* compilers (Arm ARM SDK
        # convention) and Ubuntu 22.04 ships aarch64-linux-gnu-* (Debian
        # convention), so we point at the Ubuntu names directly.
        ./build.sh \
            --config Release \
            --build_shared_lib \
            --parallel \
            --use_vsinpu \
            --skip_tests \
            --skip_submodule_sync \
            --allow_running_as_root \
            --cmake_generator Ninja \
            --build_dir /work/target/ort-build \
            --cmake_extra_defines \
                CMAKE_SYSTEM_NAME=Linux \
                CMAKE_SYSTEM_PROCESSOR=aarch64 \
                CMAKE_C_COMPILER=aarch64-linux-gnu-gcc \
                CMAKE_CXX_COMPILER=aarch64-linux-gnu-g++ \
                CMAKE_ASM_COMPILER=aarch64-linux-gnu-gcc \
                onnxruntime_USE_VSINPU=ON

        cp -v /work/target/ort-build/Release/libonnxruntime.so* /work/target/ort/
    '

echo ""
echo "==> built:"
ls -la "$OUT/"
docker run --rm -v "$REPO_ROOT":/work ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main \
    bash -c 'file /work/target/ort/libonnxruntime.so* 2>/dev/null || true'
