#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 2 ]]; then
    echo "usage: $0 <device-binary> <source>" >&2
    exit 2
fi

BINARY="$1"
SOURCE="$2"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"
REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc8a/wave-npu-compile-audio-denoiser-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/tim_vx_compile_audio_denoiser.cc"

if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 8A paths" >&2
    exit 3
fi
if [[ ! -f "$BINARY" || ! -f "$SOURCE" ]]; then
    echo "policy: fixed PoC 8A binary or source is missing" >&2
    exit 3
fi

if rg -n -- '(^|[^[:alnum:]_])(Run|Compile)[[:space:]]*\(' "$SOURCE"; then
    echo "policy: compile-only source contains Compile or Run" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CompileToBinary[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "2" ]]; then
    echo "policy: source must contain exactly two CompileToBinary calls" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CreateOperation<tim::vx::ops::Matmul>[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain exactly one Matmul operation" >&2
    exit 4
fi
if rg -n -- 'CreateOperation<tim::vx::ops::(Erf|Add|Conv|Custom|NBG)' "$SOURCE"; then
    echo "policy: source contains an operation outside fixed Matmul" >&2
    exit 4
fi
for fixed_value in \
    '/tmp/wave-npu-compile8/lib:/usr/lib' \
    '/tmp/wave-npu-compile8/sdk' \
    '/tmp/wave-npu-compile8/lib/libCLC.so' \
    '/tmp/wave-npu-compile8/output/wave-audio-denoiser-candidate-1.nbg' \
    '/tmp/wave-npu-compile8/output/.wave-audio-denoiser-candidate-1.nbg.partial' \
    'const tim::vx::ShapeType input_shape{16, 1}' \
    'const tim::vx::ShapeType weight_shape{16, 16}' \
    'const tim::vx::ShapeType output_shape{16, 1}' \
    'tim::vx::TensorAttribute::CONSTANT' \
    'weights[output * kFrameSamples + output] = 0.50F' \
    '= 0.25F'; do
    if ! rg -Fq -- "$fixed_value" "$SOURCE"; then
        echo "policy: fixed model value missing: $fixed_value" >&2
        exit 4
    fi
done
if [[ "$(rg -o -- 'tim::vx::DataType::FLOAT32' "$SOURCE" | wc -l | tr -d ' ')" != "3" ]]; then
    echo "policy: fixed FLOAT32 tensor declarations changed" >&2
    exit 4
fi
if rg -n -- \
    'CopyData(To|From)Tensor|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(' \
    "$SOURCE"; then
    echo "policy: source contains execution or direct-runtime primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc8a/wave-npu-compile-audio-denoiser-v1"
        header="$(aarch64-linux-gnu-readelf -h "$binary")"
        dynamic="$(aarch64-linux-gnu-readelf -d "$binary")"
        imports="$(
            aarch64-linux-gnu-nm -D --undefined-only "$binary" | c++filt
        )"

        grep -Fq "Class:                             ELF64" <<<"$header"
        grep -Fq "Machine:                           AArch64" <<<"$header"
        grep -Fq "Type:                              DYN" <<<"$header"
        grep -Fq "Shared library: [libtim-vx.so]" <<<"$dynamic"
        grep -Fq "Shared library: [libGAL.so]" <<<"$dynamic"
        grep -Fq \
            "Library runpath: [/tmp/wave-npu-compile8/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq \
            "tim::vx::ops::Matmul::Matmul(tim::vx::Graph*, bool, bool, bool, bool)" \
            <<<"$imports"
        if grep -Eq \
            "tim::vx::Graph::(Run|Compile)\\(|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an execution primitive" >&2
            exit 6
        fi
    '

echo "device audio-denoiser compiler policy: PASS"
