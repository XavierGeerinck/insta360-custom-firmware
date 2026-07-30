#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIRECTORY="$REPOSITORY_ROOT/artifacts/npu/build/poc14b"
OUTPUT_BINARY="$OUTPUT_DIRECTORY/wave-npu-compile-erf-affine-v1"
CONSTANTS="$OUTPUT_DIRECTORY/affine-constants.bin"
SOURCE="$SCRIPT_DIR/tim_vx_compile_erf_affine.cc"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"
EXPECTED_CONSTANTS_SHA256="a2b588be1a4e68a3179a18b51be016115782f923344521c667ca4310c9eeecb4"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "build: approved immutable offline image is absent" >&2
    exit 10
fi
if [[ ! -f "$CONSTANTS" ]]; then
    echo "build: frozen affine constants are absent" >&2
    exit 11
fi
if [[ "$(shasum -a 256 "$CONSTANTS" | awk '{print $1}')" != "$EXPECTED_CONSTANTS_SHA256" ]]; then
    echo "build: frozen affine constants hash differs" >&2
    exit 12
fi
mkdir -p "$OUTPUT_DIRECTORY"

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        export LC_ALL=C
        export SOURCE_DATE_EPOCH=1785196800
        output="artifacts/npu/build/poc14b"

        cd "$output"
        aarch64-linux-gnu-objcopy \
            -I binary \
            -O elf64-littleaarch64 \
            -B aarch64 \
            --rename-section \
            .data=.rodata,alloc,load,readonly,data,contents \
            affine-constants.bin \
            affine-constants.o
        cd /work

        aarch64-linux-gnu-g++ \
            -std=c++17 \
            -O2 \
            -g0 \
            -Wall \
            -Wextra \
            -Werror \
            -Wformat=2 \
            -Wshadow \
            -D_FORTIFY_SOURCE=2 \
            -fPIE \
            -fstack-protector-strong \
            -I waved/third_party/TIM-VX/include \
            waved/tools/npu-compiler/tim_vx_compile_erf_affine.cc \
            artifacts/npu/build/poc14b/affine-constants.o \
            -L waved/target/tim-vx \
            -L waved/third_party/device-sdk/lib \
            -Wl,-rpath-link,/work/waved/third_party/device-sdk/lib \
            -Wl,--allow-shlib-undefined \
            -Wl,--as-needed \
            -Wl,--build-id=sha1 \
            -Wl,--enable-new-dtags \
            -Wl,-rpath,/tmp/wave-npu-poc14b/lib:/usr/lib \
            -Wl,-z,noexecstack \
            -Wl,-z,now \
            -Wl,-z,relro \
            -pie \
            -ltim-vx \
            -lGAL \
            -ldl \
            -lpthread \
            -o "$output/wave-npu-compile-erf-affine-v1"

        aarch64-linux-gnu-readelf -h \
            "$output/wave-npu-compile-erf-affine-v1"
        aarch64-linux-gnu-readelf -d \
            "$output/wave-npu-compile-erf-affine-v1"
        aarch64-linux-gnu-nm -D --undefined-only \
            "$output/wave-npu-compile-erf-affine-v1" \
            | c++filt
        sha256sum \
            "$output/wave-npu-compile-erf-affine-v1" \
            "$output/affine-constants.bin" \
            waved/tools/npu-compiler/tim_vx_compile_erf_affine.cc \
            waved/target/tim-vx/libtim-vx.so \
            waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h
    '

"$SCRIPT_DIR/verify-device-erf-affine-policy.sh" \
    "$OUTPUT_BINARY" \
    "$SOURCE" \
    "$CONSTANTS"
