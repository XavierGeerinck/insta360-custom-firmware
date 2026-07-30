#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIRECTORY="$REPOSITORY_ROOT/artifacts/npu/build/poc5f"
OUTPUT_BINARY="$OUTPUT_DIRECTORY/wave-npu-compile-signal-frame-clc-v1"
SOURCE="$SCRIPT_DIR/tim_vx_compile_signal_frame_clc.cc"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"

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
            waved/tools/npu-compiler/tim_vx_compile_signal_frame_clc.cc \
            -L waved/target/tim-vx \
            -L waved/third_party/device-sdk/lib \
            -Wl,-rpath-link,/work/waved/third_party/device-sdk/lib \
            -Wl,--allow-shlib-undefined \
            -Wl,--as-needed \
            -Wl,--build-id=sha1 \
            -Wl,--enable-new-dtags \
            -Wl,-rpath,/tmp/wave-npu-compile/lib:/usr/lib \
            -Wl,-z,noexecstack \
            -Wl,-z,now \
            -Wl,-z,relro \
            -pie \
            -ltim-vx \
            -lGAL \
            -ldl \
            -lpthread \
            -o artifacts/npu/build/poc5f/wave-npu-compile-signal-frame-clc-v1

        aarch64-linux-gnu-readelf -h \
            artifacts/npu/build/poc5f/wave-npu-compile-signal-frame-clc-v1
        aarch64-linux-gnu-readelf -d \
            artifacts/npu/build/poc5f/wave-npu-compile-signal-frame-clc-v1
        aarch64-linux-gnu-nm -D --undefined-only \
            artifacts/npu/build/poc5f/wave-npu-compile-signal-frame-clc-v1 \
            | c++filt
        sha256sum \
            artifacts/npu/build/poc5f/wave-npu-compile-signal-frame-clc-v1 \
            waved/target/tim-vx/libtim-vx.so \
            waved/third_party/vip9000-sdk/6.4.18.5/drivers/libCLC.so \
            waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h
    '

"$SCRIPT_DIR/verify-device-signal-frame-clc-policy.sh" \
    "$OUTPUT_BINARY" \
    "$SOURCE"
