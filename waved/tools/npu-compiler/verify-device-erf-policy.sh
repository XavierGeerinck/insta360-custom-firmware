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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc5c/wave-npu-compile-erf-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/tim_vx_compile_erf.cc"

if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 5C paths" >&2
    exit 3
fi
if [[ ! -f "$BINARY" || ! -f "$SOURCE" ]]; then
    echo "policy: fixed PoC 5C binary or source is missing" >&2
    exit 3
fi

if rg -n -- '(^|[^[:alnum:]_])Run[[:space:]]*\(' "$SOURCE"; then
    echo "policy: source contains a Run call" >&2
    exit 4
fi
COMPILE_CALLS="$(
    rg -o -- 'CompileToBinary[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' '
)"
if [[ "$COMPILE_CALLS" != "2" ]]; then
    echo "policy: source must contain exactly two CompileToBinary calls" >&2
    exit 4
fi
ERF_CALLS="$(
    rg -o -- 'CreateOperation<tim::vx::ops::Erf>[[:space:]]*\(' "$SOURCE" |
        wc -l |
        tr -d ' '
)"
if [[ "$ERF_CALLS" != "1" ]]; then
    echo "policy: source must contain exactly one fixed Erf operation" >&2
    exit 4
fi
if rg -n -- 'CreateOperation<tim::vx::ops::(Add|Conv|Custom)' "$SOURCE"; then
    echo "policy: source contains an operation outside fixed Erf" >&2
    exit 4
fi
if [[ "$(rg -o -- 'BindInput[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'BindOutput[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must bind exactly one input and one output" >&2
    exit 4
fi
if ! rg -q -- 'const tim::vx::ShapeType shape\{4\};' "$SOURCE"; then
    echo "policy: fixed Erf shape [4] is missing" >&2
    exit 4
fi
if [[ "$(rg -o -- 'tim::vx::DataType::UINT8' "$SOURCE" | wc -l | tr -d ' ')" != "2" ]]; then
    echo "policy: fixed UINT8 input/output declaration changed" >&2
    exit 4
fi
for fixed_path in \
    /tmp/wave-npu-compile/output/wave-erf-candidate-1.nbg \
    /tmp/wave-npu-compile/output/wave-erf-candidate-2.nbg \
    /tmp/wave-npu-compile/output/.wave-erf-candidate-1.nbg.partial \
    /tmp/wave-npu-compile/output/.wave-erf-candidate-2.nbg.partial; do
    if ! rg -Fq -- "$fixed_path" "$SOURCE"; then
        echo "policy: fixed output path missing: $fixed_path" >&2
        exit 4
    fi
done
if rg -n -- 'ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(' "$SOURCE"; then
    echo "policy: source contains a forbidden direct-runtime primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc5c/wave-npu-compile-erf-v1"
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
            "Library runpath: [/tmp/wave-npu-compile/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq "tim::vx::ops::Erf::Erf(tim::vx::Graph*)" <<<"$imports"

        if grep -Eq \
            "tim::vx::Graph::Run|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an execution primitive" >&2
            exit 6
        fi
    '

echo "device Erf compiler policy: PASS"
