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
EXPECTED_BINARY="$REPOSITORY_ROOT/artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v2"
EXPECTED_SOURCE="$REPOSITORY_ROOT/waved/tools/npu-compiler/wave_npu_benchmark_ctc_cpu_vs_npu.cc"
EXPECTED_HEADER="$REPOSITORY_ROOT/artifacts/npu/build/poc15a/onnxruntime-v1.15.1/include/onnxruntime_c_api.h"
EXPECTED_HEADER_SHA256="cfa6e9fcf7750d386f94a0b1c3bc3ac062f9ccdff51490154bcd9243a82f1f91"

if [[ ! -f "$BINARY_INPUT" || ! -f "$SOURCE_INPUT" ||
      -L "$BINARY_INPUT" || -L "$SOURCE_INPUT" ]]; then
    echo "policy: fixed PoC 15A binary or source is missing" >&2
    exit 3
fi
BINARY="$(cd "$(dirname "$BINARY_INPUT")" && pwd)/$(basename "$BINARY_INPUT")"
SOURCE="$(cd "$(dirname "$SOURCE_INPUT")" && pwd)/$(basename "$SOURCE_INPUT")"
if [[ "$BINARY" != "$EXPECTED_BINARY" ||
      "$SOURCE" != "$EXPECTED_SOURCE" ]]; then
    echo "policy: inputs differ from the fixed PoC 15A paths" >&2
    exit 3
fi
HEADER_SHA256="$(
    shasum -a 256 "$EXPECTED_HEADER" 2>/dev/null | awk '{print $1}'
)"
if [[ ! -f "$EXPECTED_HEADER" || -L "$EXPECTED_HEADER" ||
      "$HEADER_SHA256" != "$EXPECTED_HEADER_SHA256" ]]; then
    echo "policy: pinned ORT C header differs" >&2
    exit 3
fi

python3 - "$SOURCE" <<'PY'
import pathlib
import re
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")

exact_counts = {
    r"runtime->api->Run\(": 1,
    r"runtime->create\(&config\)": 1,
    r"runtime->input_set\(runtime->context, &npu_input\)": 1,
    r"runtime->output_get\(runtime->context, run_config\)": 1,
    r"runtime->output_get\(runtime->context, output_config\)": 1,
    r"runtime->destroy\(runtime->context\)": 1,
    r"=\s*dlopen\(": 2,
    r"void\* const symbol = dlsym\(": 1,
    r"\bgcoHAL_QueryChipIdentityEx\(": 2,
}
for pattern, expected in exact_counts.items():
    observed = len(re.findall(pattern, source))
    if observed != expected:
        raise SystemExit(
            f"policy: {pattern!r} count {observed}, expected {expected}"
        )

required = [
    "constexpr size_t kColdPairs = 1;",
    "constexpr size_t kWarmupPairs = 20;",
    "constexpr size_t kMeasuredPairs = 200;",
    "constexpr size_t kFixtureCount = 4;",
    "constexpr uint32_t kQualificationSeed = 0x573315a1U;",
    "constexpr uint32_t kBenchmarkSeed = 0x573315b1U;",
    "constexpr uint32_t kExpectedFinalQualificationState = 0x474c5d42U;",
    "constexpr uint32_t kExpectedFinalBenchmarkState = 0x98d70268U;",
    "constexpr double kMeanAbsoluteErrorMax = 0.025;",
    "constexpr double kMaximumAbsoluteErrorMax = 0.25;",
    "constexpr double kCosineSimilarityMin = 0.999;",
    "/tmp/wave-npu-poc15a-r2/model/ctc.onnx",
    "/tmp/wave-npu-poc15a-r2/model/ctc_online.nb",
    "/usr/lib/libonnxruntime.so.1.15.1",
    "/usr/lib/libnnsdk.so",
    "/tmp/wave-npu-poc15a-r2/lib/libCLC.so",
    "/tmp/wave-npu-poc15a-r2/result/ctc-cpu-npu-timings-v2.bin",
    "/tmp/wave-npu-poc15a-r2/result/ctc-cpu-npu-timings-v2.partial",
    "8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219",
    "3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5",
    "457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639",
    "36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe",
    "CPUExecutionProvider",
    "SetSessionGraphOptimizationLevel",
    "ORT_ENABLE_ALL",
    "CreateTensorWithDataAsOrtValue",
    "GetTensorMutableData",
    "CLOCK_MONOTONIC_RAW",
    "CLOCK_THREAD_CPUTIME_ID",
    "CLOCK_PROCESS_CPUTIME_ID",
    "PR_SET_PDEATHSIG",
    "O_EXCL",
    "O_NOFOLLOW",
    "FloatToHalfBits(",
    "HalfBitsToFloat(",
    "static_cast<int32_t>(*state & 0xffU) - 128",
    "input->data()[0] = 0.5F;",
    "!enforce_numerical_gates || qualification_passed",
    "pair-materialization-gate",
    "std::memcpy(",
    "pair & 1U",
    "CTC_CPU_NPU_BENCH status=PASS mode=benchmark",
    "cpu_run_count=225 npu_run_count=229",
    "--describe",
    "--benchmark",
]
for text in required:
    if text not in source:
        raise SystemExit(f"policy: required fixed contract missing: {text}")

for forbidden in [
    "OrtSessionOptionsAppendExecutionProvider",
    "SessionOptionsAppendExecutionProvider",
    "CompileToBinary",
    "AsyncRun",
]:
    if forbidden in source:
        raise SystemExit(f"policy: forbidden execution surface: {forbidden}")

call_forbidden = re.compile(
    r"\b(?:ioctl|system|popen|fork|vfork|execv|execve|execl|socket|"
    r"reboot|setpriority|sched_setaffinity)\s*\("
)
match = call_forbidden.search(source)
if match:
    raise SystemExit(f"policy: forbidden call: {match.group(0)}")

path_forbidden = [
    "/dev/galcore",
    "/proc/sys/",
    "/boot/",
    "/lib/firmware/",
]
for text in path_forbidden:
    if text in source:
        raise SystemExit(f"policy: forbidden path: {text}")

main = re.search(r"int main\(int argc, char\*\* argv\) \{(.*)\n\}", source, re.S)
if not main:
    raise SystemExit("policy: main not found")
if len(re.findall(r'argv\[1\], "--(?:describe|benchmark)"', main.group(1))) != 2:
    raise SystemExit("policy: fixed CLI modes differ")
if re.search(r"argv\[[2-9]\]", main.group(1)):
    raise SystemExit("policy: arbitrary CLI argument surface")
PY

docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        binary="artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v2"
        header="$(aarch64-linux-gnu-readelf -h "$binary")"
        dynamic="$(aarch64-linux-gnu-readelf -d "$binary")"
        program="$(aarch64-linux-gnu-readelf -W -l "$binary")"
        imports="$(
            aarch64-linux-gnu-nm -D --undefined-only "$binary" | c++filt
        )"
        strings="$(aarch64-linux-gnu-strings "$binary")"

        grep -Fq "Class:                             ELF64" <<<"$header"
        grep -Fq "Machine:                           AArch64" <<<"$header"
        grep -Fq "Type:                              DYN" <<<"$header"
        grep -Fq "Shared library: [libGAL.so]" <<<"$dynamic"
        grep -Fq \
            "Library runpath: [/tmp/wave-npu-poc15a-r2/lib:/usr/lib]" \
            <<<"$dynamic"
        grep -Fq "BIND_NOW" <<<"$dynamic"
        grep -Fq "GNU_RELRO" <<<"$program"
        grep -Eq "GNU_STACK[[:space:]].*RW[[:space:]]" <<<"$program"
        if grep -Eq "GNU_STACK[[:space:]].*RWE" <<<"$program"; then
            echo "policy: executable stack" >&2
            exit 6
        fi

        grep -Fq "gcoHAL_QueryChipIdentityEx" <<<"$imports"
        grep -Fq "clock_gettime" <<<"$imports"
        grep -Fq "dlopen" <<<"$imports"
        grep -Fq "dlsym" <<<"$imports"
        grep -Fq "CTC_CPU_NPU_BENCH status=PASS mode=benchmark" <<<"$strings"
        grep -Fq "CCN2" <<<"$strings"
        grep -Fq "OrtGetApiBase" <<<"$strings"
        grep -Fq "aml_module_create" <<<"$strings"
        grep -Fq "aml_module_input_set" <<<"$strings"
        grep -Fq "aml_module_output_get" <<<"$strings"
        grep -Fq "aml_module_destroy" <<<"$strings"

        if grep -Eq \
            "[[:space:]](ioctl|system|popen|fork|vfork|execv|execve|socket|reboot)(@|$)" \
            <<<"$imports"; then
            echo "policy: executable imports a forbidden primitive" >&2
            exit 6
        fi
        if grep -Eq \
            "/dev/galcore|/proc/sys/|/boot/|/lib/firmware/|libCLC\\.so" \
            <<<"$strings" &&
            ! grep -Fq "/tmp/wave-npu-poc15a-r2/lib/libCLC.so" <<<"$strings"; then
            echo "policy: executable references a forbidden path" >&2
            exit 6
        fi
    '

echo "device CTC CPU/NPU benchmark policy: PASS"
