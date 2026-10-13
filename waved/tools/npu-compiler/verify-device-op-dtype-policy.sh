#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 || ( "$1" != fp16-conv && "$1" != u8-fc ) ]]; then
    echo "usage: $0 <fp16-conv|u8-fc>" >&2
    exit 2
fi

KIND="$1"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIRECTORY="$REPOSITORY_ROOT/artifacts/npu/build/poc16/probes"
BINARY="$OUTPUT_DIRECTORY/wave-npu-compile-$KIND-v1"
SOURCE="$SCRIPT_DIR/tim_vx_compile_${KIND//-/_}.cc"
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
    require(len(re.findall(pattern, source)) == expected,
            "unexpected count for " + pattern)

root = "/tmp/wave-npu-compile" + ("16a" if kind == "fp16-conv" else "16b")
for value in (
    root + "/lib:/usr/lib", root + "/sdk", root + "/lib/libCLC.so",
    root + "/output/wave-" + kind + "-candidate-1.nbg",
    root + "/output/.wave-" + kind + "-candidate-1.nbg.partial",
    "constexpr unsigned int kWatchdogSeconds = 15;",
    "constexpr uint32_t kWaveChipModel = 0x00009000U;",
    "constexpr uint32_t kWaveProductId = 0x1000000eU;",
    "constexpr uint32_t kWaveNbgVersion = 0x00020000U;",
    'std::getenv("VIV_VX_DEBUG_LEVEL")',
    'std::getenv("VIV_VX_ENABLE_PRINT_TARGET")',
    'std::strcmp(debug_level, "1") != 0',
    'std::strcmp(print_target, "1") != 0',
    'std::getenv("VSIMULATOR_CONFIG")', 'std::getenv("LD_PRELOAD")',
    'std::getenv("LD_AUDIT")',
    'return PathIsAbsent(kForbiddenClcPath, "forbidden CLC");',
    'argc == 2 && std::strcmp(argv[1], "--describe") == 0',
    'argc != 2 || std::strcmp(argv[1], "--candidate-1") != 0',
    "prctl(PR_SET_PDEATHSIG, SIGKILL)", "getppid() == 1",
    "sigaction(SIGALRM, &watchdog_action, nullptr)",
    "alarm(kWatchdogSeconds);",
    "O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW",
    "fchmod(fd, 0444)", "fsync(fd)", "link(partial_path, path)",
    "identity.chip_model != kWaveChipModel",
    "identity.customer_id != kWaveProductId",
    'std::memcmp(nbg.data(), "VPMN", 4)',
    "version != kWaveNbgVersion || product != kWaveProductId",
    "graph->CompileToBinary(nullptr, &nbg_size)",
    "graph->CompileToBinary(nbg.data(), &written_size)",
):
    require(value in source, "fixed boundary missing: " + value)

require(not re.search(
    r"\b(?:Run|Compile|CopyDataToTensor|CopyDataFromTensor|CopyTensor|"
    r"ioctl|dlopen|vxProcessGraph|vxVerifyGraph|vxImportBinary|"
    r"vxImportKernelFromURL|clCreateProgramWithSource)\s*\(|/dev/galcore",
    source), "execution, import or direct runtime primitive present")
count(r"\bCompileToBinary\s*\(", 2)
count(r"\balarm\s*\(", 1)
count(r"\bCreateOperation\s*<", 1)
count(r"\bCreateTensor\s*\(", 4)
count(r"\bBindInput\s*\(", 3)
count(r"\bBindOutput\s*\(", 1)
count(r"tim::vx::TensorAttribute::CONSTANT", 2)

if kind == "fp16-conv":
    shapes = (("input", "16, 1, 1, 1"), ("weight", "3, 1, 1, 1"),
              ("bias", "1"), ("output", "16, 1, 1, 1"))
    types = (("input", "FLOAT16"), ("weight", "FLOAT16"),
             ("bias", "FLOAT32"), ("output", "FLOAT16"))
    for value in (
        "constexpr std::array<uint16_t, 3> kWeights{0x3400U, 0x3800U, 0x3400U};",
        "constexpr std::array<float, 1> kBias{0.0F};",
        "const std::array<uint32_t, 2> stride{1, 1};",
        "const std::array<uint32_t, 2> dilation{0, 0};",
        "graph->CreateOperation<tim::vx::ops::Conv2d>(\n"
        "        tim::vx::PadType::SAME, stride, dilation);",
    ):
        require(value in source, "fixed Conv value missing: " + value)
    count(r"tim::vx::DataType::FLOAT16", 3)
    count(r"tim::vx::DataType::FLOAT32", 1)
else:
    shapes = (("input", "16, 1"), ("weight", "16, 4"),
              ("bias", "4"), ("output", "4, 1"))
    types = (("input", "UINT8"), ("weight", "UINT8"),
             ("bias", "INT32"), ("output", "UINT8"))
    require("constexpr std::array<int32_t, 4> kBias{0, 0, 0, 0};" in source,
            "INT32 zero bias changed")
    require("tim::vx::QuantType::ASYMMETRIC, 1.0F, 0);" in source,
            "fixed asymmetric quantization changed")
    require("graph->CreateOperation<tim::vx::ops::FullyConnected>(0, 4);" in source,
            "FullyConnected axis or units changed")
    match = re.search(r"constexpr std::array<uint8_t, 64> kWeights\{([^}]+)\};", source)
    require(match is not None, "fixed sparse U8 weights missing")
    weights = [int(value.strip()) for value in match.group(1).split(",")]
    expected = [int(index in (0, 21, 42, 63)) for index in range(64)]
    require(weights == expected, "sparse weight columns changed")
    count(r"tim::vx::DataType::UINT8", 3)
    count(r"tim::vx::DataType::INT32", 1)
    count(r"\n        quantization\);", 4)

for name, shape in shapes:
    require("const tim::vx::ShapeType " + name + "_shape{" + shape + "};" in source,
            name + " shape changed")
for name, dtype in types:
    require("const tim::vx::TensorSpec " + name + "_spec(\n"
            "        tim::vx::DataType::" + dtype + "," in source,
            name + " dtype changed")
print("device " + kind + " source policy: PASS")
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
        binary="artifacts/npu/build/poc16/probes/wave-npu-compile-$1-v1"
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
root = "/tmp/wave-npu-compile" + ("16a" if kind == "fp16-conv" else "16b")
runpaths = re.findall(r"\(RUNPATH\)\s+Library runpath: \[([^]]+)\]", report)
require(runpaths == [root + "/lib:/usr/lib"], "fixed RUNPATH changed")
needed = set(re.findall(r"\(NEEDED\)\s+Shared library: \[([^]]+)\]", report))
allowed = {"libtim-vx.so", "libGAL.so", "libstdc++.so.6", "libgcc_s.so.1",
           "libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0",
           "ld-linux-aarch64.so.1"}
require({"libtim-vx.so", "libGAL.so", "libc.so.6"} <= needed <= allowed,
        "unexpected native dependencies: " + repr(needed))
op = "Conv2d" if kind == "fp16-conv" else "FullyConnected"
require("tim::vx::ops::" + op + "::" + op + "(" in report,
        "fixed operation constructor import missing")
# Graph and Tensor methods are pure virtual. Their indirect dispatch has no
# dynamic named import; the exact compile-only calls are checked in source.
require(not re.search(
    r"tim::vx::Graph::(?:Run|Compile)\(|"
    r"tim::vx::Tensor::(?:CopyData|CopyTensor)|"
    r"tim::vx::ops::NBG::|"
    r"\b(?:ioctl|dlopen|vxProcessGraph|vxVerifyGraph|vxImportBinary|"
    r"vxImportKernelFromURL|clCreateProgramWithSource)(?:@|\s|$)", report),
    "execution or import primitive present")

# Floors equal the already-device-tested PoC 8C binary from the same cross image.
limits = {"GLIBC": (2, 17), "GLIBCXX": (3, 4), "CXXABI": (1, 3, 9), "GCC": (3, 0)}
for namespace, version in re.findall(r"\b(GLIBCXX|GLIBC|CXXABI|GCC)_([0-9.]+)", report):
    numeric = tuple(int(part) for part in version.split("."))
    require(numeric <= limits[namespace], "ABI version exceeds baseline: " + namespace + "_" + version)
print("device " + kind + " ELF policy: PASS")
PY

echo "device $KIND compile-only policy: PASS"
