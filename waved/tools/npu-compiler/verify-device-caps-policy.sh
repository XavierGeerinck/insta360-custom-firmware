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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc5d/wave-npu-query-caps-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_query_caps.cc"

if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 5D paths" >&2
    exit 3
fi
if [[ ! -f "$BINARY" || ! -f "$SOURCE" ]]; then
    echo "policy: fixed PoC 5D binary or source is missing" >&2
    exit 3
fi

CAPS_CALLS="$(
    rg -o -- 'vxQueryHardwareCaps[[:space:]]*\(' "$SOURCE" |
        wc -l |
        tr -d ' '
)"
if [[ "$CAPS_CALLS" != "2" ]]; then
    echo "policy: source must contain exactly two capability queries" >&2
    exit 4
fi
if [[ "$(rg -o -- 'vxCreateContext[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must create exactly one OpenVX context" >&2
    exit 4
fi
if rg -n -- \
    'vx(CreateGraph|VerifyGraph|ProcessGraph|ScheduleGraph|CreateProgram|BuildProgram)[[:space:]]*\(|CompileToBinary[[:space:]]*\(|(^|[^[:alnum:]_])Run[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(' \
    "$SOURCE"; then
    echo "policy: source contains a graph, execution, or direct-runtime primitive" >&2
    exit 5
fi

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc5d/wave-npu-query-caps-v1"
        header="$(aarch64-linux-gnu-readelf -h "$binary")"
        dynamic="$(aarch64-linux-gnu-readelf -d "$binary")"
        imports="$(
            aarch64-linux-gnu-nm -D --undefined-only "$binary" | c++filt
        )"

        grep -Fq "Class:                             ELF64" <<<"$header"
        grep -Fq "Machine:                           AArch64" <<<"$header"
        grep -Fq "Type:                              DYN" <<<"$header"
        grep -Fq "Shared library: [libOpenVX.so]" <<<"$dynamic"
        grep -Fq "Shared library: [libGAL.so]" <<<"$dynamic"
        grep -Fq "Library runpath: [/usr/lib]" <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq "vxCreateContext" <<<"$imports"
        grep -Fq "vxGetStatus" <<<"$imports"
        grep -Fq "vxQueryHardwareCaps" <<<"$imports"
        grep -Fq "vxReleaseContext" <<<"$imports"

        if grep -Eq \
            "vx(CreateGraph|VerifyGraph|ProcessGraph|ScheduleGraph|CreateProgram|BuildProgram)|tim::vx::Graph::Run|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports a graph or execution primitive" >&2
            exit 6
        fi
    '

echo "device capability-query policy: PASS"
