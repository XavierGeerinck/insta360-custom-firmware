#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 3 ]]; then
    echo "usage: $0 <device-binary> <source> <constants>" >&2
    exit 2
fi

BINARY_INPUT="$1"
SOURCE_INPUT="$2"
CONSTANTS_INPUT="$3"
IMAGE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"
REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc14b/wave-npu-compile-erf-affine-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/tim_vx_compile_erf_affine.cc"
EXPECTED_CONSTANTS="$REPOSITORY_ROOT/artifacts/npu/build/poc14b/affine-constants.bin"
EXPECTED_CONSTANTS_SHA256="a2b588be1a4e68a3179a18b51be016115782f923344521c667ca4310c9eeecb4"

for input in "$BINARY_INPUT" "$SOURCE_INPUT" "$CONSTANTS_INPUT"; do
    if [[ ! -f "$input" ]]; then
        echo "policy: fixed PoC 14B input is missing: $input" >&2
        exit 3
    fi
done
BINARY="$(cd "$(dirname "$BINARY_INPUT")" && pwd)/$(basename "$BINARY_INPUT")"
SOURCE="$(cd "$(dirname "$SOURCE_INPUT")" && pwd)/$(basename "$SOURCE_INPUT")"
CONSTANTS="$(cd "$(dirname "$CONSTANTS_INPUT")" && pwd)/$(basename "$CONSTANTS_INPUT")"
if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" || "$CONSTANTS" != "$EXPECTED_CONSTANTS" ]]; then
    echo "policy: inputs differ from the fixed PoC 14B paths" >&2
    exit 3
fi
if [[ "$(shasum -a 256 "$CONSTANTS" | awk '{print $1}')" != "$EXPECTED_CONSTANTS_SHA256" ]]; then
    echo "policy: frozen affine constants hash differs" >&2
    exit 4
fi

for operation in Multiply Add Erf; do
    if [[ "$(rg -o -- "CreateOperation<tim::vx::ops::$operation>[[:space:]]*\\(" "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
        echo "policy: source must contain exactly one fixed $operation operation" >&2
        exit 4
    fi
done
if [[ "$(rg -o -- 'CompileToBinary[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "2" ]]; then
    echo "policy: source must contain exactly two CompileToBinary calls" >&2
    exit 4
fi
for required in \
    'const tim::vx::ShapeType shape{481};' \
    'tim::vx::QuantType::SYMMETRIC_PER_CHANNEL' \
    'tim::vx::QuantType::ASYMMETRIC' \
    'kFeatureScale = 0.0625F' \
    'kErfInputScale = 0.03125F' \
    'kErfOutputScale = 0.0078125F' \
    'kDeltaIndex = 17' \
    'kBaselineDeltaWeight = 127' \
    'kBaselineDeltaBias = -4773' \
    'constants.weights[kDeltaIndex] = 126' \
    'constants.biases[kDeltaIndex] = -4772' \
    'a2b588be1a4e68a3179a18b51be016115782f923344521c667ca4310c9eeecb4' \
    'b52ddaf3baed2a7001ff10cbf9ba0600945749a2332f02d71d2ae0be25ac98b8' \
    '/tmp/wave-npu-poc14b/output/erf-affine-a1.nbg' \
    '/tmp/wave-npu-poc14b/output/erf-affine-a2.nbg' \
    '/tmp/wave-npu-poc14b/output/erf-affine-b1.nbg' \
    '/tmp/wave-npu-poc14b/output/erf-affine-b2.nbg' \
    '/tmp/wave-npu-poc14b/output/erf-affine-c1.nbg' \
    '/tmp/wave-npu-poc14b/output/erf-affine-c2.nbg'; do
    if ! rg -Fq -- "$required" "$SOURCE"; then
        echo "policy: required fixed source contract missing: $required" >&2
        exit 4
    fi
done
if rg -n -- \
    '(^|[^[:alnum:]_])Run[[:space:]]*\(|CopyData(To|From)Tensor[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(' \
    "$SOURCE"; then
    echo "policy: source contains a forbidden execution/runtime primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc14b/wave-npu-compile-erf-affine-v1"
        header="$(aarch64-linux-gnu-readelf -h "$binary")"
        dynamic="$(aarch64-linux-gnu-readelf -d "$binary")"
        imports="$(
            aarch64-linux-gnu-nm -D --undefined-only "$binary" | c++filt
        )"
        symbols="$(aarch64-linux-gnu-nm "$binary")"
        strings="$(aarch64-linux-gnu-strings "$binary")"

        grep -Fq "Class:                             ELF64" <<<"$header"
        grep -Fq "Machine:                           AArch64" <<<"$header"
        grep -Fq "Type:                              DYN" <<<"$header"
        grep -Fq "Shared library: [libtim-vx.so]" <<<"$dynamic"
        grep -Fq "Shared library: [libGAL.so]" <<<"$dynamic"
        grep -Fq \
            "Library runpath: [/tmp/wave-npu-poc14b/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq "tim::vx::ops::Multiply::Multiply" <<<"$imports"
        grep -Fq "tim::vx::ops::Add::Add" <<<"$imports"
        grep -Fq "tim::vx::ops::Erf::Erf" <<<"$imports"
        grep -Fq "_binary_affine_constants_bin_start" <<<"$symbols"
        grep -Fq "_binary_affine_constants_bin_end" <<<"$symbols"
        grep -Fq \
            "b52ddaf3baed2a7001ff10cbf9ba0600945749a2332f02d71d2ae0be25ac98b8" \
            <<<"$strings"

        if grep -Eq \
            "tim::vx::Graph::Run|CopyData(To|From)Tensor|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an execution primitive" >&2
            exit 6
        fi
    '

echo "device affine-plus-Erf compiler policy: PASS"
