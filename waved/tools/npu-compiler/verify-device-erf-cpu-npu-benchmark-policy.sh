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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc13g/wave-npu-benchmark-erf-cpu-vs-npu-v2"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_benchmark_erf_cpu_vs_npu.cc"

if [[ ! -f "$BINARY_INPUT" || ! -f "$SOURCE_INPUT" ||
      -L "$BINARY_INPUT" || -L "$SOURCE_INPUT" ]]; then
    echo "policy: fixed PoC 13G binary or source is missing" >&2
    exit 3
fi
BINARY="$(cd "$(dirname "$BINARY_INPUT")" && pwd)/$(basename "$BINARY_INPUT")"
SOURCE="$(cd "$(dirname "$SOURCE_INPUT")" && pwd)/$(basename "$SOURCE_INPUT")"
if [[ "$BINARY" != "$EXPECTED_BINARY" || "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 13G paths" >&2
    exit 3
fi

if [[ "$(rg -o -- 'context->CreateGraph[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'graph->Run[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'graph->Compile[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CreateOperation<tim::vx::ops::NBG>' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CopyDataToTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ||
      "$(rg -o -- 'CopyDataFromTensor[[:space:]]*\(' "$SOURCE" | wc -l | tr -d ' ')" != "1" ]]; then
    echo "policy: fixed graph/NBG/tensor call sites differ" >&2
    exit 4
fi

for required in \
    'constexpr size_t kColdFrames = 1;' \
    'constexpr size_t kWarmupFrames = 100;' \
    'constexpr size_t kMeasuredFrames = 10000;' \
    'constexpr size_t kFrameWidth = 481;' \
    'constexpr size_t kCanaryBytes = 32;' \
    'constexpr uint32_t kInputSeed = 0x5733130fU;' \
    'constexpr size_t kCalibrationSamples = 100000;' \
    'UINT64_C(0x367c7eaad959d707)' \
    'UINT64_C(0x742e3a62d58acd0d)' \
    'constexpr uint32_t kExpectedFinalState = 0x6ae16fe0U;' \
    '/tmp/wave-npu-poc13g-r2/model/erf-mask.nbg' \
    '/tmp/wave-npu-poc13g-r2/result/erf-cpu-npu-timings-v1.bin' \
    '/tmp/wave-npu-poc13g-r2/result/erf-cpu-npu-timings-v1.partial' \
    '/tmp/wave-npu-poc13g-r2/lib:/usr/lib' \
    '/tmp/wave-npu-poc13g-r2/lib/libCLC.so' \
    '96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94' \
    'c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296' \
    'const tim::vx::ShapeType shape{481};' \
    'tim::vx::QuantType::ASYMMETRIC, 0.03125F, 128' \
    'tim::vx::QuantType::ASYMMETRIC, 0.0078125F, 128' \
    'CLOCK_MONOTONIC_RAW' \
    'CLOCK_THREAD_CPUTIME_ID' \
    'CLOCK_PROCESS_CPUTIME_ID' \
    'constexpr char kExpectedLogLevel[] = "2";' \
    'DurationAllowZero(' \
    'PR_SET_PDEATHSIG' \
    'O_EXCL' \
    'frame_index < kTotalFrames' \
    'sequence & 1U' \
    'cpu_output[index] = kErfLut[input[index]]' \
    'ERF_CPU_NPU_BENCH status=PASS mode=benchmark' \
    'cpu_thread_zero_count=%zu' \
    'cpu_count=10101 npu_run_count=10101 compile_count=1' \
    '--benchmark' \
    '--describe'; do
    if ! rg -Fq -- "$required" "$SOURCE"; then
        echo "policy: required fixed benchmark contract missing: $required" >&2
        exit 4
    fi
done

if rg -n -- \
    '(^|[^[:alnum:]_])(erf|erff|erfl)[[:space:]]*\(|CompileToBinary[[:space:]]*\(|ioctl[[:space:]]*\(|/dev/galcore|dlopen[[:space:]]*\(|vx(Process|Schedule)Graph|AsyncRun|system[[:space:]]*\(|popen[[:space:]]*\(|/sys/|/proc/sys/|/boot/|/lib/firmware/|reboot[[:space:]]*\(|setpriority[[:space:]]*\(|sched_setaffinity[[:space:]]*\(' \
    "$SOURCE"; then
    echo "policy: source contains a forbidden primitive or path" >&2
    exit 5
fi

python3 - "$SOURCE" <<'PY'
import hashlib
import pathlib
import re
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
matches = re.findall(
    r"constexpr std::array<uint8_t, 256> kErfLut = \{(.*?)\n\};",
    source,
    flags=re.DOTALL,
)
if len(matches) != 1:
    raise SystemExit("policy: expected exactly one embedded Erf table")
values = bytes(int(token, 16) for token in re.findall(r"0x([0-9a-f]{2})", matches[0]))
if len(values) != 256:
    raise SystemExit(f"policy: embedded Erf table has {len(values)} bytes")
digest = hashlib.sha256(values).hexdigest()
expected = "c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296"
if digest != expected:
    raise SystemExit(f"policy: embedded Erf table hash differs: {digest}")
PY

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc13g/wave-npu-benchmark-erf-cpu-vs-npu-v2"
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
            "Library runpath: [/tmp/wave-npu-poc13g-r2/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq \
            "tim::vx::ops::NBG::NBG(tim::vx::Graph*, char const*, unsigned long, unsigned long)" \
            <<<"$imports"
        grep -Fq "clock_gettime" <<<"$imports"
        grep -Fq "PR_SET_PDEATHSIG" <<<"$strings" ||
            grep -Fq "parent-death gate failed" <<<"$strings"
        grep -Fq "ERF_CPU_NPU_BENCH status=PASS mode=benchmark" <<<"$strings"
        grep -Fq "ECN1" <<<"$strings"
        if grep -Eq \
            "CompileToBinary|vx(Process|Schedule)Graph|[[:space:]]ioctl(@|$)|[[:space:]]dlopen(@|$)|[[:space:]]system(@|$)|[[:space:]]popen(@|$)|[[:space:]]reboot(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports an unreviewed primitive" >&2
            exit 6
        fi
        if grep -Eq "/dev/galcore|/sys/|/proc/sys/|/boot/|/lib/firmware/" <<<"$strings"; then
            echo "policy: executable references a forbidden path" >&2
            exit 6
        fi
    '

echo "device Erf CPU/NPU benchmark policy: PASS"
