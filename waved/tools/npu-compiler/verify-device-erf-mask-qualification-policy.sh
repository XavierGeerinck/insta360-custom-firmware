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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc13d/wave-npu-run-erf-mask-qualification-v1"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_run_erf_mask_qualification.cc"

if [[ ! -f "$BINARY_INPUT" || ! -f "$SOURCE_INPUT" ]]; then
    echo "policy: fixed PoC 13D binary or source is missing" >&2
    exit 3
fi
BINARY="$(cd "$(dirname "$BINARY_INPUT")" && pwd)/$(basename "$BINARY_INPUT")"
SOURCE="$(cd "$(dirname "$SOURCE_INPUT")" && pwd)/$(basename "$SOURCE_INPUT")"
if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 13D paths" >&2
    exit 3
fi

if [[ "$(rg -o -- 'graph->Run[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'graph->Compile[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain one Compile and one bounded Run call" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CreateOperation<tim::vx::ops::NBG>' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain exactly one NBG operation" >&2
    exit 4
fi
if [[ "$(rg -o -- 'CopyDataToTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CopyDataFromTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: source must contain one bounded input/output copy site" >&2
    exit 4
fi
for required in \
    'constexpr size_t kVectorCount = 6;' \
    'constexpr size_t kVectorWidth = 481;' \
    'constexpr size_t kCanaryBytes = 32;' \
    'UINT64_C(0x8f90178b58ca8afb)' \
    '/tmp/wave-npu-poc13d/model/erf-mask.nbg' \
    '/tmp/wave-npu-poc13d/input/qualification-vectors.bin' \
    '/tmp/wave-npu-poc13d/lib:/usr/lib' \
    '/tmp/wave-npu-poc13d/lib/libCLC.so' \
    '96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94' \
    '3374b928722c7d504e9ae8d8e1e99c1dc7b3542ee08cbede60b2f8bb8f93a232' \
    'const tim::vx::ShapeType shape{481};' \
    'tim::vx::QuantType::ASYMMETRIC, 0.03125F, 128' \
    'tim::vx::QuantType::ASYMMETRIC, 0.0078125F, 128' \
    'vector_index < kVectorCount' \
    'ERF_MASK status=PASS mode=qualify-six vectors=6' \
    'compile_count=1 run_count=6' \
    '--qualify-six'; do
    if ! rg -Fq -- "$required" "$SOURCE"; then
        echo "policy: required fixed execution contract missing: $required" >&2
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
        binary="artifacts/npu/build/poc13d/wave-npu-run-erf-mask-qualification-v1"
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
            "Library runpath: [/tmp/wave-npu-poc13d/lib:/usr/lib]" \
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

echo "device Erf-mask six-vector execution policy: PASS"
