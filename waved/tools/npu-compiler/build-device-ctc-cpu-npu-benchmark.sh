#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIRECTORY="$REPOSITORY_ROOT/artifacts/npu/build/poc15a"
OUTPUT_BINARY="$OUTPUT_DIRECTORY/wave-npu-benchmark-ctc-cpu-vs-npu-v1"
SECOND_BINARY="$OUTPUT_DIRECTORY/wave-npu-benchmark-ctc-cpu-vs-npu-v1.second"
SOURCE="$SCRIPT_DIR/wave_npu_benchmark_ctc_cpu_vs_npu.cc"
ORT_INCLUDE="$OUTPUT_DIRECTORY/onnxruntime-v1.15.1/include"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "build: approved immutable offline image is absent" >&2
    exit 10
fi
if [[ ! -f "$ORT_INCLUDE/onnxruntime_c_api.h" ||
      -L "$ORT_INCLUDE/onnxruntime_c_api.h" ]]; then
    echo "build: pinned ORT 1.15.1 header is absent" >&2
    exit 11
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
        export SOURCE_DATE_EPOCH=1785369600

        build_one() {
            local output="$1"
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
                -I artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include \
                waved/tools/npu-compiler/wave_npu_benchmark_ctc_cpu_vs_npu.cc \
                -L waved/third_party/device-sdk/lib \
                -Wl,-rpath-link,/work/waved/third_party/device-sdk/lib \
                -Wl,--allow-shlib-undefined \
                -Wl,--as-needed \
                -Wl,--build-id=sha1 \
                -Wl,--enable-new-dtags \
                -Wl,-rpath,/tmp/wave-npu-poc15a/lib:/usr/lib \
                -Wl,-z,noexecstack \
                -Wl,-z,now \
                -Wl,-z,relro \
                -pie \
                -lGAL \
                -ldl \
                -lpthread \
                -o "$output"
        }

        build_one \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1
        build_one \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1.second
        cmp \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1 \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1.second

        aarch64-linux-gnu-readelf -h \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1
        aarch64-linux-gnu-readelf -d \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1
        aarch64-linux-gnu-nm -D --undefined-only \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1 \
            | c++filt
        sha256sum \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1 \
            artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v1.second \
            waved/tools/npu-compiler/wave_npu_benchmark_ctc_cpu_vs_npu.cc \
            artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include/onnxruntime_c_api.h
    '

if ! cmp -s "$OUTPUT_BINARY" "$SECOND_BINARY"; then
    echo "build: deterministic rebuild differs" >&2
    exit 12
fi
rm -f -- "$SECOND_BINARY"

if [[ -x "$SCRIPT_DIR/verify-device-ctc-cpu-npu-benchmark-policy.sh" ]]; then
    "$SCRIPT_DIR/verify-device-ctc-cpu-npu-benchmark-policy.sh" \
        "$OUTPUT_BINARY" \
        "$SOURCE"
fi
