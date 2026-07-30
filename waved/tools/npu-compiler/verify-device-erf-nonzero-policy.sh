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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc7/wave-npu-execute-erf-nonzero-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_execute_erf_nonzero.cc"

if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 7 paths" >&2
    exit 3
fi
if [[ ! -f "$BINARY" || ! -f "$SOURCE" ]]; then
    echo "policy: fixed PoC 7 binary or source is missing" >&2
    exit 3
fi

if [[ "$(rg -o -- 'graph->Run[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'graph->Compile[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain one Compile and one Run call" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CreateOperation<tim::vx::ops::NBG>' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain exactly one NBG operation" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CopyDataToTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CopyDataFromTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain one input and one output copy" >&2
    exit 4
fi
for fixed_value in \
    /tmp/wave-npu-exec7/model/wave-erf-openvx.nbg \
    /tmp/wave-npu-exec7/lib:/usr/lib \
    /tmp/wave-npu-exec7/lib/libCLC.so \
    6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee \
    'constexpr std::array<uint8_t, 4> kInput{0, 1, 2, 255}' \
    'constexpr std::array<uint8_t, 4> kExpected{0, 1, 1, 1}' \
    'input=0,1,2,255 output=0,1,1,1 bytes=4'; do
    if ! rg -Fq -- "$fixed_value" "$SOURCE"; then
        echo "policy: fixed execution value missing: $fixed_value" >&2
        exit 4
    fi
done
if rg -n -- \
    'CompileToBinary[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(|vx(Process|Schedule)Graph|AsyncRun' \
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
        binary="artifacts/npu/build/poc7/wave-npu-execute-erf-nonzero-v1"
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
            "Library runpath: [/tmp/wave-npu-exec7/lib:/usr/lib]" \
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
    '

echo "device nonzero Erf execution policy: PASS"
