#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 ]]; then
    echo "usage: $0 <device-binary> <source>" >&2
    exit 2
fi

BINARY_INPUT="$1"
SOURCE_INPUT="$2"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"
REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc13e/wave-npu-run-erf-mask-stream-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_run_erf_mask_stream.cc"

if [[ ! -f "$BINARY_INPUT" || ! -f "$SOURCE_INPUT" ]]; then
    echo "policy: fixed PoC 13E binary or source is missing" >&2
    exit 3
fi
BINARY="$(cd "$(dirname "$BINARY_INPUT")" && pwd)/$(basename "$BINARY_INPUT")"
SOURCE="$(cd "$(dirname "$SOURCE_INPUT")" && pwd)/$(basename "$SOURCE_INPUT")"
if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 13E paths" >&2
    exit 3
fi

if [[ "$(rg -o -- 'graph->Run[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'graph->Compile[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain one Compile and one bounded Run site" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CreateOperation<tim::vx::ops::NBG>' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CopyDataToTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CopyDataFromTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: fixed NBG and tensor-copy sites differ" >&2
    exit 4
fi
for required in \
    'constexpr size_t kFrameCount = 1201;' \
    'constexpr size_t kFrameWidth = 481;' \
    'constexpr size_t kCanaryBytes = 32;' \
    'UINT64_C(0xe2c6f1eda5f3eaea)' \
    '/tmp/wave-npu-poc13e/model/erf-mask.nbg' \
    '/tmp/wave-npu-poc13e/input/erf-stream-input.bin' \
    '/tmp/wave-npu-poc13e/output/erf-stream-output.bin' \
    '/tmp/wave-npu-poc13e/output/.erf-stream-output.bin.partial' \
    '/tmp/wave-npu-poc13e/lib:/usr/lib' \
    '/tmp/wave-npu-poc13e/lib/libCLC.so' \
    '96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94' \
    'a409686c2482b576288b6f46ecf77d3b99b019c9c436e0f042cb1ac39dacc3e9' \
    'const tim::vx::ShapeType shape{481};' \
    'tim::vx::QuantType::ASYMMETRIC, 0.03125F, 128' \
    'tim::vx::QuantType::ASYMMETRIC, 0.0078125F, 128' \
    'frame_index < kFrameCount' \
    'ERF_STREAM status=PASS mode=fixture-stream frames=1201' \
    'compile_count=1 run_count=1201' \
    '--fixture-stream'; do
    if ! rg -Fq -- "$required" "$SOURCE"; then
        echo "policy: required fixed stream contract missing: $required" >&2
        exit 4
    fi
done
if rg -n -- \
    'CompileToBinary[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(|vx(Process|Schedule)Graph|AsyncRun|system[[:space:]]*\(|popen[[:space:]]*\(' \
    "$SOURCE"; then
    echo "policy: source contains a forbidden compile/direct/async primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc13e/wave-npu-run-erf-mask-stream-v1"
        header="$(aarch64-linux-gnu-readelf -h "$binary")"
        dynamic="$(aarch64-linux-gnu-readelf -d "$binary")"
        imports="$(
            aarch64-linux-gnu-nm -D --undefined-only "$binary" | c++filt
        )"
        strings="$(aarch64-linux-gnu-strings "$binary")"

        grep -Fq "Class:                             ELF64" <<<"$header"
        grep -Fq "Machine:                           AArch64" <<<"$header"
        grep -Fq "Type:                              DYN" <<<"$header"
        grep -Fq "Shared library: [libtim-vx.so]" <<<"$dynamic"
        grep -Fq "Shared library: [libGAL.so]" <<<"$dynamic"
        grep -Fq \
            "Library runpath: [/tmp/wave-npu-poc13e/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq \
            "tim::vx::ops::NBG::NBG(tim::vx::Graph*, char const*, unsigned long, unsigned long)" \
            <<<"$imports"
        if grep -Eq \
            "CompileToBinary|vx(Process|Schedule)Graph|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an unreviewed primitive" >&2
            exit 6
        fi
        if grep -Fq "/dev/galcore" <<<"$strings"; then
            echo "policy: executable references a direct device path" >&2
            exit 6
        fi
    '

echo "device Erf-mask full-stream execution policy: PASS"
