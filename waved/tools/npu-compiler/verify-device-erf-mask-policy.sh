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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc13b/wave-npu-compile-erf-mask-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/tim_vx_compile_erf_mask.cc"

if [[ ! -f "$BINARY_INPUT" || ! -f "$SOURCE_INPUT" ]]; then
    echo "policy: fixed PoC 13B binary or source is missing" >&2
    exit 3
fi
BINARY="$(cd "$(dirname "$BINARY_INPUT")" && pwd)/$(basename "$BINARY_INPUT")"
SOURCE="$(cd "$(dirname "$SOURCE_INPUT")" && pwd)/$(basename "$SOURCE_INPUT")"
if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 13B paths" >&2
    exit 3
fi

if [[ "$(rg -o -- 'CompileToBinary[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "2" ]]; then
    echo "policy: source must contain exactly two CompileToBinary calls" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CreateOperation<tim::vx::ops::Erf>[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain exactly one fixed Erf operation" >&2
    exit 4
fi
if [[ "$(rg -o -- 'tim::vx::DataType::UINT8' "$SOURCE" | wc -l | tr -d ' ')" != "2" ]]; then
    echo "policy: fixed UINT8 declarations changed" >&2
    exit 4
fi
for required in \
    'const tim::vx::ShapeType shape{481};' \
    'tim::vx::QuantType::ASYMMETRIC, 0.03125F, 128' \
    'tim::vx::QuantType::ASYMMETRIC, 0.0078125F, 128' \
    '/tmp/wave-npu-poc13b/output/erf-mask-a1.nbg' \
    '/tmp/wave-npu-poc13b/output/erf-mask-a2.nbg' \
    '/tmp/wave-npu-poc13b/output/.erf-mask-a1.nbg.partial' \
    '/tmp/wave-npu-poc13b/output/.erf-mask-a2.nbg.partial' \
    '{"--a1", "a1"' \
    '{"--a2", "a2"'; do
    if ! rg -Fq -- "$required" "$SOURCE"; then
        echo "policy: required fixed source contract missing: $required" >&2
        exit 4
    fi
done
if rg -n -- \
    '(^|[^[:alnum:]_])Run[[:space:]]*\(|CopyData(To|From)Tensor[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(|libCLC' \
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
        binary="artifacts/npu/build/poc13b/wave-npu-compile-erf-mask-v1"
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
            "Library runpath: [/tmp/wave-npu-poc13b/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq "tim::vx::ops::Erf::Erf(tim::vx::Graph*)" <<<"$imports"

        if grep -Eq \
            "tim::vx::Graph::Run|CopyData(To|From)Tensor|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an execution primitive" >&2
            exit 6
        fi
        if grep -Fq "libCLC" <<<"$strings"; then
            echo "policy: executable references quarantined CLC" >&2
            exit 6
        fi
    '

echo "device Erf-mask compiler policy: PASS"
