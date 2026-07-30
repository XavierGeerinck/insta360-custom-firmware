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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc8d/wave-npu-run-conv-audio-denoiser-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_run_conv_audio_denoiser.cc"

if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 8D paths" >&2
    exit 3
fi
if [[ ! -f "$BINARY" || ! -f "$SOURCE" ]]; then
    echo "policy: fixed PoC 8D binary or source is missing" >&2
    exit 3
fi

if [[ "$(rg -o -- 'graph->Run[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'graph->Compile[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain one Compile and one Run call" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CreateOperation<tim::vx::ops::Conv2d>[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain exactly one Conv2d operation" >&2
    exit 4
fi
if rg -n -- 'CompileToBinary|CreateOperation<tim::vx::ops::(NBG|Erf|Add|Matmul|Custom)' "$SOURCE"; then
    echo "policy: source contains export or an operation outside Conv2d" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CopyDataToTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CopyDataFromTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain one input and one output copy" >&2
    exit 4
fi
if [[ "$(rg -o -- 'BindInput[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "3" ||
      "$(rg -o -- 'BindOutput[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain the fixed Conv2d bindings" >&2
    exit 4
fi
for fixed_value in \
    '/tmp/wave-npu-audio8d/lib:/usr/lib' \
    '/tmp/wave-npu-audio8d/lib/libCLC.so' \
    'constexpr size_t kFrameSamples = 16' \
    'constexpr std::array<float, 3> kWeights{0.25F, 0.50F, 0.25F}' \
    'constexpr std::array<float, 1> kBias{0.0F}' \
    'const tim::vx::ShapeType input_shape{16, 1, 1, 1}' \
    'const tim::vx::ShapeType weight_shape{3, 1, 1, 1}' \
    'const tim::vx::ShapeType bias_shape{1}' \
    'const tim::vx::ShapeType output_shape{16, 1, 1, 1}' \
    'tim::vx::PadType::SAME' \
    'const std::array<uint32_t, 2> stride{1, 1}' \
    'const std::array<uint32_t, 2> dilation{0, 0}' \
    'input_energy - 16.0' \
    'output_energy - 0.125' \
    'AUDIO status=PASS mode=conv-direct iterations=1'; do
    if ! rg -Fq -- "$fixed_value" "$SOURCE"; then
        echo "policy: fixed model value missing: $fixed_value" >&2
        exit 4
    fi
done
if [[ "$(rg -o -- 'tim::vx::DataType::FLOAT32' "$SOURCE" | wc -l | tr -d ' ')" != "4" ||
      "$(rg -o -- 'tim::vx::TensorAttribute::CONSTANT' "$SOURCE" | wc -l | tr -d ' ')" != "2" ]]; then
    echo "policy: fixed FLOAT32 tensor declarations changed" >&2
    exit 4
fi
if rg -n -- \
    'ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(|vx(Process|Schedule)Graph|AsyncRun' \
    "$SOURCE"; then
    echo "policy: source contains a forbidden direct/async primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc8d/wave-npu-run-conv-audio-denoiser-v1"
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
            "Library runpath: [/tmp/wave-npu-audio8d/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq "tim::vx::ops::Conv2d::Conv2d" <<<"$imports"
        if grep -Eq \
            "CompileToBinary|tim::vx::ops::NBG|vx(Process|Schedule)Graph|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an unreviewed primitive" >&2
            exit 6
        fi
    '

echo "device convolutional audio-denoiser runner policy: PASS"
