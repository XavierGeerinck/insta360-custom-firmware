#!/usr/bin/env bash
# Build only the source-controlled TIM-VX wrapper for the approved PoC 16.
# Vendor libraries are link inputs; no vendor or AArch64 executable is run.
set -euo pipefail

if [[ "$#" -ne 0 ]]; then
    echo "usage: $0" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
BUILD_ROOT="$REPOSITORY_ROOT/artifacts/npu/build/poc16"
SOURCE_ROOT="$REPOSITORY_ROOT/waved/third_party/TIM-VX"
SDK_ROOT="$REPOSITORY_ROOT/waved/third_party/device-sdk"
SOURCE_COMMIT="702715d714277376d7ff466b7b72833ff57564c1"
IMAGE="sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a"
IMAGE_REFERENCE="ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c"
SOURCE_DATE="1785196800"

for name in runtime runtime-cmake runtime-build.log runtime-image.json \
    runtime-inputs.json runtime-provenance.json runtime-build-script.sh; do
    if [[ -e "$BUILD_ROOT/$name" || -L "$BUILD_ROOT/$name" ]]; then
        echo "runtime build: refusing existing evidence: $BUILD_ROOT/$name" >&2
        exit 3
    fi
done
if [[ "$(git -C "$SOURCE_ROOT" rev-parse HEAD)" != "$SOURCE_COMMIT" ||
      -n "$(git -C "$SOURCE_ROOT" status --porcelain)" ]]; then
    echo "runtime build: TIM-VX must be the exact clean reviewed commit" >&2
    exit 4
fi
if [[ ! -L "$SDK_ROOT/lib/libCLC.so" ||
      "$(readlink "$SDK_ROOT/lib/libCLC.so")" != "libOpenCL.so" ]]; then
    echo "runtime build: CLC must remain the existing inert OpenCL symlink" >&2
    exit 5
fi
if [[ "$(docker image inspect "$IMAGE" --format '{{.Id}}')" != "$IMAGE" ||
      "$(docker image inspect "$IMAGE" --format '{{.Architecture}}')" != "amd64" ]]; then
    echo "runtime build: immutable amd64 build image is unavailable" >&2
    echo "acquire the pinned image with: docker pull --platform=linux/amd64 $IMAGE_REFERENCE" >&2
    exit 6
fi

mkdir -p "$BUILD_ROOT"
mkdir "$BUILD_ROOT/runtime" "$BUILD_ROOT/runtime-cmake"
cp "$SCRIPT_DIR/build-device-op-dtype-runtime.sh" "$BUILD_ROOT/runtime-build-script.sh"
docker image inspect "$IMAGE" > "$BUILD_ROOT/runtime-image.json"

# Record and check every source/SDK input before granting the container a
# read-only source mount. Symlink targets are recorded rather than copied.
PYTHONDONTWRITEBYTECODE=1 python3 - "$REPOSITORY_ROOT" "$SOURCE_COMMIT" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1])
source = root / "waved/third_party/TIM-VX"
sdk = root / "waved/third_party/device-sdk"
expected = {
    "libArchModelSw.so": "2b3df0844a2ff582f71da6517113939b54acdc7c97bebcac771e6e58140df63f",
    "libGAL.so": "13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6",
    "libNNArchPerf.so": "253167677e9839d09815de93a86423824f26f56f097c100d59c03170adecffa9",
    "libOpenCL.so": "40ea26cfc80a43b79af09c0798ced760bb5339f9eef9450e6d08931c3b320ded",
    "libOpenVX.so": "f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb",
    "libOpenVXU.so": "370083254b7de8478f95fe90914245ddb4a52b2b3520749fe01f3ab45fba791c",
    "libVSC.so": "e9a39fe3b522b3926ff34f3fb380c73983ad8d48c5f40ffd6e6d8024adc2ed18",
    "libVSC_Lite.so": "e7c2c38fa813c669b7370795fad5d7b1207881103c481a6ac90e4785d058591f",
    "libovxlib.so": "09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9",
}

def describe(path):
    if path.is_symlink():
        return {"kind": "symlink", "target": os.readlink(path)}
    if not path.is_file():
        raise SystemExit(f"runtime build: non-regular input: {path}")
    return {"kind": "file", "bytes": path.stat().st_size,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

for name, digest in expected.items():
    observed = describe(sdk / "lib" / name)
    if observed.get("sha256") != digest:
        raise SystemExit(f"runtime build: SDK hash mismatch: {name}: {observed}")
tracked = subprocess.check_output(
    ["git", "-C", str(source), "ls-files", "-z"]
).decode().split("\0")
source_files = {name: describe(source / name) for name in tracked if name}
sdk_files = {str(path.relative_to(sdk)): describe(path)
             for path in sorted(sdk.rglob("*")) if not path.is_dir()}
canonical = json.dumps({"source": source_files, "sdk": sdk_files},
                       sort_keys=True, separators=(",", ":")).encode()
payload = {"schema_version": 1, "source_commit": sys.argv[2],
           "source_files": source_files, "sdk_files": sdk_files,
           "device_matched_library_hashes": expected,
           "input_manifest_sha256": hashlib.sha256(canonical).hexdigest()}
with (root / "artifacts/npu/build/poc16/runtime-inputs.json").open("x") as stream:
    json.dump(payload, stream, indent=2, sort_keys=True)
    stream.write("\n")
PY

# Unix Makefiles uses the already-present make, so no package installation,
# compiler substitution, Ninja download or network access is needed.
docker run --rm \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --volume "$BUILD_ROOT":/output \
    --workdir /work \
    --env LC_ALL=C \
    --env TZ=UTC \
    --env SOURCE_DATE_EPOCH="$SOURCE_DATE" \
    "$IMAGE" \
    bash -euo pipefail -c '
        cmake --version > /output/runtime/cmake-version.txt
        make --version > /output/runtime/make-version.txt
        aarch64-linux-gnu-gcc --version > /output/runtime/gcc-version.txt
        aarch64-linux-gnu-g++ --version > /output/runtime/gxx-version.txt
        aarch64-linux-gnu-ld --version > /output/runtime/ld-version.txt
        aarch64-linux-gnu-readelf --version > /output/runtime/readelf-version.txt
        for tool in cmake ninja make aarch64-linux-gnu-gcc aarch64-linux-gnu-g++ \
            aarch64-linux-gnu-readelf aarch64-linux-gnu-nm; do
            command -v "$tool" || true
        done
        cat /output/runtime/gxx-version.txt
        cmake -S /work/waved/third_party/TIM-VX \
            -B /output/runtime-cmake \
            -G "Unix Makefiles" \
            -DCMAKE_SYSTEM_NAME=Linux \
            -DCMAKE_SYSTEM_PROCESSOR=aarch64 \
            -DCMAKE_C_COMPILER=aarch64-linux-gnu-gcc \
            -DCMAKE_CXX_COMPILER=aarch64-linux-gnu-g++ \
            -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
            -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_SKIP_RPATH=ON \
            -DCMAKE_SHARED_LINKER_FLAGS="-Wl,--as-needed -Wl,-z,now -Wl,-z,relro -Wl,-z,noexecstack" \
            -DBUILD_SHARED_LIBS=ON \
            -DTIM_VX_ENABLE_TEST=OFF \
            -DTIM_VX_BUILD_EXAMPLES=OFF \
            -DTIM_VX_ENABLE_NBG_PARSER=ON \
            -DTIM_VX_USE_EXTERNAL_OVXLIB=ON \
            -DOVXLIB_LIB=/work/waved/third_party/device-sdk/lib/libovxlib.so \
            -DOVXLIB_INC=/work/waved/third_party/TIM-VX/src/tim/vx/internal/include \
            -DEXTERNAL_VIV_SDK=/work/waved/third_party/device-sdk
        cmake --build /output/runtime-cmake --target tim-vx -- -j2 VERBOSE=1
        cp /output/runtime-cmake/src/tim/libtim-vx.so /output/runtime/libtim-vx.so
        aarch64-linux-gnu-readelf -h /output/runtime/libtim-vx.so \
            > /output/runtime/elf-header.txt
        aarch64-linux-gnu-readelf -d /output/runtime/libtim-vx.so \
            > /output/runtime/elf-dynamic.txt
        aarch64-linux-gnu-readelf -W --version-info /output/runtime/libtim-vx.so \
            > /output/runtime/elf-versions.txt
        aarch64-linux-gnu-readelf -W -l /output/runtime/libtim-vx.so \
            > /output/runtime/elf-program-headers.txt
        aarch64-linux-gnu-nm -D --undefined-only /output/runtime/libtim-vx.so \
            > /output/runtime/undefined-symbols.txt
        aarch64-linux-gnu-readelf -W --version-info \
            /work/firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libstdc++.so.6.0.28 \
            > /output/runtime/device-libstdcxx-versions.txt
        sha256sum /output/runtime/libtim-vx.so
        cat /output/runtime/elf-header.txt /output/runtime/elf-dynamic.txt \
            /output/runtime/elf-versions.txt
    ' 2>&1 | tee "$BUILD_ROOT/runtime-build.log"

PYTHONDONTWRITEBYTECODE=1 python3 - "$REPOSITORY_ROOT" "$IMAGE" \
    "$IMAGE_REFERENCE" "$SOURCE_DATE" <<'PY'
import hashlib
import json
from pathlib import Path
import re
import sys

root = Path(sys.argv[1])
build = root / "artifacts/npu/build/poc16"
runtime = build / "runtime"
header = (runtime / "elf-header.txt").read_text()
dynamic = (runtime / "elf-dynamic.txt").read_text()
versions = (runtime / "elf-versions.txt").read_text()
imports = (runtime / "undefined-symbols.txt").read_text()
if not all(token in header for token in ("ELF64", "AArch64", "DYN")):
    raise SystemExit("runtime build: wrapper is not AArch64 ELF64 shared object")
needed = re.findall(r"Shared library: \[([^\]]+)\]", dynamic)
allowed = {"libArchModelSw.so", "libGAL.so", "libNNArchPerf.so", "libOpenCL.so",
           "libOpenVX.so", "libOpenVXU.so", "libVSC.so", "libVSC_Lite.so",
           "libovxlib.so", "libstdc++.so.6", "libm.so.6", "libgcc_s.so.1",
           "libc.so.6", "libpthread.so.0", "libdl.so.2", "librt.so.1",
           "ld-linux-aarch64.so.1"}
if "libovxlib.so" not in needed or set(needed) - allowed:
    raise SystemExit(f"runtime build: unreviewed DT_NEEDED list: {needed}")
if "libCLC" in dynamic or "RPATH" in dynamic or "RUNPATH" in dynamic:
    raise SystemExit("runtime build: unexpected CLC dependency or embedded library path")
if re.search(r"clCompiler\w*|clLinkKernel|clCompileKernel", imports):
    raise SystemExit("runtime build: wrapper imports a CLC compiler entry point")
required = sorted(set(re.findall(r"Name: ((?:GLIBC|GLIBCXX|CXXABI)_[0-9.]+)", versions)))
glibc = [tuple(map(int, v.removeprefix("GLIBC_").split(".")))
         for v in required if v.startswith("GLIBC_")]
if glibc and max(glibc) > (2, 31):
    raise SystemExit("runtime build: wrapper requires a newer libc than the Wave")
device_loader_path = root / "firmwares/fw_v1.1.6.6/extracted/rootfs/lib64/ld-2.31.so"
device_loader_sha = hashlib.sha256(device_loader_path.read_bytes()).hexdigest()
if device_loader_sha != "912002c930f3096d98fc32e0897466049d6e5182cbb606b456b2ebcc99551366":
    raise SystemExit("runtime build: local device-matched AArch64 loader hash changed")
device_cxx_path = root / "firmwares/fw_v1.1.6.6/extracted/rootfs/usr/lib/libstdc++.so.6.0.28"
device_cxx_sha = hashlib.sha256(device_cxx_path.read_bytes()).hexdigest()
if device_cxx_sha != "d2189eb10122876a7fd35909eaecbea22da38462ce345d9bf46a53f500435009":
    raise SystemExit("runtime build: local device-matched libstdc++ hash changed")
available = set(re.findall(r"Name: ((?:GLIBCXX|CXXABI)_[0-9.]+)",
                          (runtime / "device-libstdcxx-versions.txt").read_text()))
missing = set(v for v in required if v.startswith(("GLIBCXX_", "CXXABI_"))) - available
if missing:
    raise SystemExit(f"runtime build: unavailable device C++ ABI versions: {sorted(missing)}")
inputs = json.loads((build / "runtime-inputs.json").read_text())
wrapper_sha = hashlib.sha256((runtime / "libtim-vx.so").read_bytes()).hexdigest()
historical_sha = "d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"
payload = {
    "schema_version": 1,
    "mode": "host-only-wrapper-build",
    "image_id": sys.argv[2],
    "image_reference": sys.argv[3],
    "source_date_epoch": int(sys.argv[4]),
    "source_commit": inputs["source_commit"],
    "input_manifest_sha256": inputs["input_manifest_sha256"],
    "input_manifest_path": str((build / "runtime-inputs.json").relative_to(root)),
    "build_log_path": str((build / "runtime-build.log").relative_to(root)),
    "build_log_sha256": hashlib.sha256((build / "runtime-build.log").read_bytes()).hexdigest(),
    "build_script_snapshot_path": str((build / "runtime-build-script.sh").relative_to(root)),
    "build_script_sha256": hashlib.sha256((build / "runtime-build-script.sh").read_bytes()).hexdigest(),
    "qualification_script_sha256": hashlib.sha256((root / "waved/tools/npu-compiler/build-device-op-dtype-runtime.sh").read_bytes()).hexdigest(),
    "wrapper": {"path": str((runtime / "libtim-vx.so").relative_to(root)),
                "bytes": (runtime / "libtim-vx.so").stat().st_size,
                "sha256": wrapper_sha, "elf_class": "ELF64", "machine": "AArch64",
                "type": "DYN", "needed": needed, "required_versions": required,
                "historical_sha256": historical_sha,
                "matches_historical_digest": wrapper_sha == historical_sha},
    "device_cpp_abi": {"local_path": str(device_cxx_path.relative_to(root)),
                       "sha256": device_cxx_sha, "missing_required_versions": sorted(missing)},
    "device_loader": {"local_path": str(device_loader_path.relative_to(root)),
                      "sha256": device_loader_sha,
                      "loader_symlink": "lib/ld-linux-aarch64.so.1 -> ../lib64/ld-2.31.so"},
    "compiler_versions": {name.removesuffix("-version.txt"): path.read_text()
                          for path in sorted(runtime.glob("*-version.txt"))
                          for name in [path.name]},
    "cmake_options": {"generator": "Unix Makefiles", "build_type": "Release",
                      "external_ovxlib": True, "nbg_parser": True,
                      "tests": False, "examples": False, "try_compile": "STATIC_LIBRARY",
                      "skip_rpath": True, "parallel_jobs": 2},
    "containment": {"network": "none", "source_sdk_mount": "read-only",
                    "device_calls": 0, "vendor_code_executions": 0,
                    "graph_compile_or_execution_calls": 0,
                    "clc_symlink_target": "libOpenCL.so", "clc_dt_needed": False,
                    "clc_compiler_imports": False},
    "qualification": "wrapper-identified; fixed graph compilation and execution remain separate gates",
}
with (build / "runtime-provenance.json").open("x") as stream:
    json.dump(payload, stream, indent=2, sort_keys=True)
    stream.write("\n")
print(json.dumps(payload["wrapper"], indent=2))
PY
