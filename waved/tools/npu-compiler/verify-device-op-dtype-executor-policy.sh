#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 || ( "$1" != fp16-conv && "$1" != u8-fc ) ]]; then
    echo "usage: $0 <fp16-conv|u8-fc>" >&2
    exit 2
fi

KIND="$1"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIRECTORY="$REPOSITORY_ROOT/artifacts/npu/build/poc16/executors"
BINARY="$OUTPUT_DIRECTORY/wave-npu-execute-$KIND-v1"
SOURCE="$SCRIPT_DIR/wave_npu_execute_${KIND//-/_}.cc"
IMAGE="sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a"

if [[ ! -f "$BINARY" || -L "$BINARY" || ! -f "$SOURCE" || -L "$SOURCE" ]]; then
    echo "policy: fixed regular binary or source is missing" >&2
    exit 3
fi

python3 - "$KIND" "$SOURCE" <<'PY'
import pathlib
import re
import sys
kind, source_path = sys.argv[1:]
source = pathlib.Path(source_path).read_text()
def require(condition, reason):
    if not condition:
        raise SystemExit("policy: " + reason)
def count(pattern, expected):
    require(len(re.findall(pattern, source)) == expected, "unexpected count for " + pattern)
root = "/tmp/wave-npu-exec" + ("16a" if kind == "fp16-conv" else "16b")
nbg_size = 1800 if kind == "fp16-conv" else 2056
nbg_sha = ("4e2b1df127c7f723faea050bbaff40db618eacd21e9efa1d18bff52151c7af4d"
           if kind == "fp16-conv" else
           "00521883a0e69d27f3ce2092f2cfff1932a208fad7bf2665648537d1f52a4a1e")
for value in (
    root + "/lib:/usr/lib", root + "/model/wave-" + kind + ".nbg",
    root + "/lib/libCLC.so", 'constexpr char kExpectedSdkDirectory[] = "/usr";',
    "constexpr size_t kExpectedNbgBytes = " + str(nbg_size) + ";", nbg_sha,
    "constexpr unsigned int kWatchdogSeconds = 8;",
    "constexpr uint32_t kWaveChipModel = 0x00009000U;",
    "constexpr uint32_t kWaveProductId = 0x1000000eU;",
    "constexpr uint32_t kWaveNbgVersion = 0x00020000U;",
    "constexpr size_t kGuardBytes = 16;",
    "constexpr uint8_t kCanaryByte = 0xa5U;",
    "constexpr uint8_t kGuardByte = 0x5aU;",
    'std::strcmp(library_path, kExpectedLibraryPath) != 0',
    'std::strcmp(sdk_directory, kExpectedSdkDirectory) != 0',
    'std::getenv("VSIMULATOR_CONFIG")', 'std::getenv("LD_PRELOAD")',
    'std::getenv("LD_AUDIT")',
    'argc == 2 && std::strcmp(argv[1], "--describe") == 0',
    'argc != 2 || std::strcmp(argv[1], "--execute-once") != 0',
    "prctl(PR_SET_PDEATHSIG, SIGKILL)", "getppid() == 1",
    "sigaction(SIGALRM, &watchdog_action, nullptr)", "alarm(kWatchdogSeconds);",
    "O_RDONLY | O_CLOEXEC | O_NOFOLLOW", "!S_ISREG(model_stat.st_mode)",
    "(model_stat.st_mode & 0777) != 0444",
    "model_stat.st_size != static_cast<off_t>(kExpectedNbgBytes)",
    "identity.chip_model != kWaveChipModel", "identity.customer_id != kWaveProductId",
    'std::memcmp(bytes, "VPMN", 4)',
    "ReadLe32(bytes + 4) != kWaveNbgVersion", "ReadLe32(bytes + 8) != kWaveProductId",
    "graph->CreateOperation<tim::vx::ops::NBG>(nbg.data(), 1, 1);",
    "input->CopyDataToTensor(kInput.data(), kInputBytes)",
    "std::array<uint8_t, kOutputBytes> device_canary{};",
    "device_canary.fill(kCanaryByte);",
    "output->CopyDataToTensor(device_canary.data(), kOutputBytes)",
    "guarded_output.fill(kGuardByte);",
    "std::memset(guarded_output.data() + kGuardBytes, kCanaryByte, kOutputBytes);",
    "output->CopyDataFromTensor(guarded_output.data() + kGuardBytes)",
    "guarded_output[index] != kGuardByte",
    "guarded_output[kGuardBytes + kOutputBytes + index] != kGuardByte",
    "std::memcmp(observed.data(), kExpected.data(), kOutputBytes) != 0",
    "mode=execute-once target=wave", "operation=" + kind + " iterations=1",
    'std::puts(" cleanup=0");',
):
    require(value in source, "fixed execution boundary missing: " + value)
for pattern, expected in (
    (r"\bCreateOperation\s*<", 1), (r"\bCreateTensor\s*\(", 2),
    (r"\bRun\s*\(", 1), (r"\bCompile\s*\(", 1),
    (r"\bCopyDataToTensor\s*\(", 2), (r"\bCopyDataFromTensor\s*\(", 1),
    (r"\bBindInput\s*\(", 1), (r"\bBindOutput\s*\(", 1),
    (r"\balarm\s*\(", 1), (r"\bclock_gettime\s*\(", 2),
    (r"tim::vx::TensorAttribute::INPUT", 1), (r"tim::vx::TensorAttribute::OUTPUT", 1),
):
    count(pattern, expected)
require(source.index("graph->Compile()") <
        source.index("output->CopyDataToTensor(device_canary.data(), kOutputBytes)") <
        source.index("graph->Run()"), "device canary must be initialized between Compile and Run")
require(not re.search(
    r"\b(?:CompileToBinary|ioctl|dlopen|vxProcessGraph|vxScheduleGraph|"
    r"AsyncRun|clCreateProgramWithSource)\s*\(|/dev/galcore", source),
    "export, direct or asynchronous execution primitive present")
for value in ("nbg_operation.reset();", "output.reset();", "input.reset();",
              "graph.reset();", "context.reset();"):
    require(value in source and source.index(value) < source.index('"EXECUTION status=PASS'),
            "teardown must precede PASS: " + value)
if kind == "fp16-conv":
    shapes = (("input", "16, 1, 1, 1"), ("output", "16, 1, 1, 1"))
    dtype = "FLOAT16"
    sizes = (("kInputBytes", 32), ("kOutputBytes", 32))
    expected_input = [0x3c00, 0xbc00] * 8
    expected_output = [0x3400] + [0] * 14 + [0xb400]
    element_type, elements = "uint16_t", 16
else:
    shapes = (("input", "16, 1"), ("output", "4, 1"))
    dtype = "UINT8"
    sizes = (("kInputBytes", 16), ("kOutputBytes", 4))
    expected_input = list(range(16))
    expected_output = [0, 5, 10, 15]
    element_type, elements = "uint8_t", None
    require("tim::vx::QuantType::ASYMMETRIC, 1.0F, 0);" in source,
            "fixed U8 quantization changed")
    count(r"\n        quantization\);", 2)
for name, size in sizes:
    require("constexpr size_t " + name + " = " + str(size) + ";" in source, name + " changed")
for name, values in (("kInput", expected_input), ("kExpected", expected_output)):
    length = elements or len(values)
    match = re.search(r"constexpr std::array<" + element_type + ", " + str(length) +
                      r"> " + name + r"\{([^}]+)\};", source)
    require(match is not None, name + " is missing")
    observed = [int(value.strip().removesuffix("U"), 0) for value in match.group(1).split(",")]
    require(observed == values, name + " changed")
for name, shape in shapes:
    require("const tim::vx::ShapeType " + name + "_shape{" + shape + "};" in source,
            name + " shape changed")
    require("const tim::vx::TensorSpec " + name + "_spec(\n"
            "        tim::vx::DataType::" + dtype + "," in source, name + " dtype changed")
count(r"tim::vx::DataType::" + dtype, 2)
print("device " + kind + " executor source policy: PASS")
PY

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "policy: fixed offline inspection image absent" >&2
    exit 4
fi

docker run --rm --pull=never \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        export LC_ALL=C
        binary="artifacts/npu/build/poc16/executors/wave-npu-execute-$1-v1"
        aarch64-linux-gnu-readelf -W -h -l -d -V "$binary"
        aarch64-linux-gnu-nm -D --undefined-only "$binary" | c++filt
    ' bash "$KIND" > "$OUTPUT_DIRECTORY/$KIND-elf-inspection.txt"

python3 - "$KIND" "$OUTPUT_DIRECTORY/$KIND-elf-inspection.txt" <<'PY'
import pathlib
import re
import sys

kind, inspection_path = sys.argv[1:]
report = pathlib.Path(inspection_path).read_text()

def require(condition, reason):
    if not condition:
        raise SystemExit("policy: " + reason)

for pattern, reason in (
    (r"Class:\s+ELF64", "ELF64 missing"),
    (r"Data:\s+2's complement, little endian", "little endian missing"),
    (r"Machine:\s+AArch64", "AArch64 missing"),
    (r"Type:\s+DYN", "PIE missing"),
    (r"Requesting program interpreter: /lib/ld-linux-aarch64.so.1", "interpreter changed"),
    (r"GNU_RELRO\s", "RELRO missing"),
    (r"BIND_NOW", "BIND_NOW missing"),
    (r"\bFLAGS_1\b[^\n]*\bPIE\b", "dynamic PIE flag missing"),
    (r"\bgcoHAL_QueryChipIdentityEx\b", "identity import missing"),
):
    require(re.search(pattern, report) is not None, reason)

stack = next((line for line in report.splitlines() if "GNU_STACK" in line), "")
require(bool(stack) and re.search(r"\sRW\s", stack) is not None and "RWE" not in stack,
        "nonexecutable stack missing")
require("(TEXTREL)" not in report and "(RPATH)" not in report,
        "TEXTREL or old RPATH present")
root = "/tmp/wave-npu-exec" + ("16a" if kind == "fp16-conv" else "16b")
runpaths = re.findall(r"\(RUNPATH\)\s+Library runpath: \[([^]]+)\]", report)
require(runpaths == [root + "/lib:/usr/lib"], "fixed RUNPATH changed")
needed = set(re.findall(r"\(NEEDED\)\s+Shared library: \[([^]]+)\]", report))
allowed = {"libtim-vx.so", "libGAL.so", "libstdc++.so.6", "libgcc_s.so.1",
           "libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0",
           "ld-linux-aarch64.so.1"}
require({"libtim-vx.so", "libGAL.so", "libc.so.6"} <= needed <= allowed,
        "unexpected native dependencies: " + repr(needed))
op = "NBG"
require("tim::vx::ops::" + op + "::" + op + "(" in report,
        "fixed operation constructor import missing")
# Graph and Tensor methods are pure virtual. Their indirect dispatch has no
# dynamic named import; the exact one-shot execution calls are checked in source.
require(not re.search(
    r"tim::vx::Graph::CompileToBinary\(|"
    r"tim::vx::ops::(?:Conv2d|FullyConnected)::|"
    r"\b(?:ioctl|dlopen|vxProcessGraph|vxVerifyGraph|vxImportBinary|"
    r"vxImportKernelFromURL|clCreateProgramWithSource)(?:@|\s|$)", report),
    "export or direct execution primitive present")

# Floors equal the already-device-tested PoC 8C binary from the same cross image.
limits = {"GLIBC": (2, 17), "GLIBCXX": (3, 4), "CXXABI": (1, 3, 9), "GCC": (3, 0)}
for namespace, version in re.findall(r"\b(GLIBCXX|GLIBC|CXXABI|GCC)_([0-9.]+)", report):
    numeric = tuple(int(part) for part in version.split("."))
    require(numeric <= limits[namespace], "ABI version exceeds baseline: " + namespace + "_" + version)
print("device " + kind + " ELF policy: PASS")
PY

echo "device $KIND one-shot execution policy: PASS"
