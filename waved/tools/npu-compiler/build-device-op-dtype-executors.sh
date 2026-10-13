#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 0 ]]; then
    echo "usage: $0" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIRECTORY="$REPOSITORY_ROOT/artifacts/npu/build/poc16/executors"
RUNTIME="$REPOSITORY_ROOT/artifacts/npu/build/poc16/runtime/libtim-vx.so"
RUNTIME_PROVENANCE="$REPOSITORY_ROOT/artifacts/npu/build/poc16/runtime-provenance.json"
TIM_VX_COMMIT="702715d714277376d7ff466b7b72833ff57564c1"
IMAGE="sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "build: immutable offline cross image missing; refusing pull" >&2
    exit 20
fi
if [[ "$(git -C "$REPOSITORY_ROOT/waved/third_party/TIM-VX" rev-parse HEAD)" != "$TIM_VX_COMMIT" ||
      -n "$(git -C "$REPOSITORY_ROOT/waved/third_party/TIM-VX" status --porcelain)" ]]; then
    echo "build: TIM-VX must remain the exact clean approved source" >&2
    exit 21
fi
if [[ ! -L "$REPOSITORY_ROOT/waved/third_party/device-sdk/lib/libCLC.so" ||
      "$(readlink "$REPOSITORY_ROOT/waved/third_party/device-sdk/lib/libCLC.so")" != libOpenCL.so ]]; then
    echo "build: SDK CLC must remain inert libOpenCL symlink" >&2
    exit 22
fi
if [[ ! -f "$RUNTIME" || -L "$RUNTIME" || ! -f "$RUNTIME_PROVENANCE" ]]; then
    echo "build: inspected fixed runtime and provenance are required" >&2
    exit 23
fi

python3 - "$REPOSITORY_ROOT" "$RUNTIME" "$RUNTIME_PROVENANCE" "$IMAGE" "$TIM_VX_COMMIT" <<'PY'
import hashlib
import json
import os
import pathlib
import sys

root, runtime, provenance, image, commit = sys.argv[1:]
root = pathlib.Path(root)
data = json.loads(pathlib.Path(provenance).read_text())
actual = hashlib.sha256(pathlib.Path(runtime).read_bytes()).hexdigest()
if (data.get("wrapper", {}).get("sha256") != actual or data.get("image_id") != image or
        data.get("source_commit") != commit or data.get("source_date_epoch") != 1785196800):
    raise SystemExit("build: runtime identity differs from fixed provenance")
inputs = json.loads((root / "artifacts/npu/build/poc16/runtime-inputs.json").read_text())
canonical = json.dumps({"source": inputs["source_files"], "sdk": inputs["sdk_files"]},
                       sort_keys=True, separators=(",", ":")).encode()
if hashlib.sha256(canonical).hexdigest() != data.get("input_manifest_sha256"):
    raise SystemExit("build: runtime input manifest differs from provenance")
for name, expected in inputs["sdk_files"].items():
    path = root / "waved/third_party/device-sdk" / name
    if path.is_symlink():
        observed = {"kind": "symlink", "target": os.readlink(path)}
    elif path.is_file():
        observed = {"kind": "file", "bytes": path.stat().st_size,
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
    else:
        raise SystemExit("build: SDK input disappeared: " + name)
    if observed != expected:
        raise SystemExit("build: SDK input changed since runtime build: " + name)
print("build: inspected runtime sha256=" + actual)
PY


python3 - "$REPOSITORY_ROOT" <<'PY'
import hashlib
import pathlib
import sys
root = pathlib.Path(sys.argv[1])
for name, size, digest in (
    ("rea-poc16a-fp16-conv-20261013-a1", 1800,
     "4e2b1df127c7f723faea050bbaff40db618eacd21e9efa1d18bff52151c7af4d"),
    ("rea-poc16b-u8-fc-20261013-a1", 2056,
     "00521883a0e69d27f3ce2092f2cfff1932a208fad7bf2665648537d1f52a4a1e"),
):
    path = root / "artifacts/npu/runs" / name / "candidate-1.nbg"
    if path.is_symlink() or not path.is_file():
        raise SystemExit("executor build: fixed qualified model missing: " + str(path))
    model = path.read_bytes()
    if len(model) != size or hashlib.sha256(model).hexdigest() != digest:
        raise SystemExit("executor build: qualified model digest/size changed: " + name)
    if model[:12] != bytes.fromhex("56504d4e000002000e000010"):
        raise SystemExit("executor build: model version/target changed: " + name)
    print("executor build: qualified model sha256=" + digest)
PY

for output in \
    wave-npu-execute-fp16-conv-v1 \
    wave-npu-execute-u8-fc-v1 \
    build.log \
    executor-build-provenance.json; do
    if [[ -e "$OUTPUT_DIRECTORY/$output" || -L "$OUTPUT_DIRECTORY/$output" ]]; then
        echo "build: output already present; refusing overwrite: $output" >&2
        exit 24
    fi
done
mkdir -p "$OUTPUT_DIRECTORY"

docker run --rm --pull=never \
    --network=none \
    --platform=linux/amd64 \
    --volume "$REPOSITORY_ROOT":/work:ro \
    --volume "$OUTPUT_DIRECTORY":/output \
    --workdir /work \
    "$IMAGE" \
    bash -euo pipefail -c '
        export LC_ALL=C
        export SOURCE_DATE_EPOCH=1785196800
        aarch64-linux-gnu-g++ --version

        for kind in fp16-conv u8-fc; do
            root="/tmp/wave-npu-exec16a"
            if [[ "$kind" == u8-fc ]]; then
                root="/tmp/wave-npu-exec16b"
            fi
            aarch64-linux-gnu-g++ \
                -std=c++17 \
                -O2 \
                -g0 \
                -Wall \
                -Wextra \
                -Werror \
                -Wformat=2 \
                -Wshadow \
                -D_FORTIFY_SOURCE=2 \
                -fPIE \
                -fstack-protector-strong \
                -I waved/third_party/TIM-VX/include \
                "waved/tools/npu-compiler/wave_npu_execute_${kind//-/_}.cc" \
                -L artifacts/npu/build/poc16/runtime \
                -L waved/third_party/device-sdk/lib \
                -Wl,-rpath-link,/work/waved/third_party/device-sdk/lib \
                -Wl,--allow-shlib-undefined \
                -Wl,--as-needed \
                -Wl,--build-id=sha1 \
                -Wl,--enable-new-dtags \
                "-Wl,-rpath,$root/lib:/usr/lib" \
                -Wl,-z,noexecstack \
                -Wl,-z,now \
                -Wl,-z,relro \
                -pie \
                -ltim-vx \
                -lGAL \
                -ldl \
                -lpthread \
                -o "/output/wave-npu-execute-$kind-v1"
            sha256sum \
                "/output/wave-npu-execute-$kind-v1" \
                "waved/tools/npu-compiler/wave_npu_execute_${kind//-/_}.cc"
        done
        sha256sum \
            artifacts/npu/build/poc16/runtime/libtim-vx.so \
            waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h
    ' 2>&1 | tee "$OUTPUT_DIRECTORY/build.log"

for kind in fp16-conv u8-fc; do
    "$SCRIPT_DIR/verify-device-op-dtype-executor-policy.sh" "$kind" \
        2>&1 | tee "$OUTPUT_DIRECTORY/$kind-policy.log"
done

python3 - "$REPOSITORY_ROOT" "$IMAGE" "$TIM_VX_COMMIT" <<'PY'
import hashlib
import json
import pathlib
import sys

root, image, commit = sys.argv[1:]
root = pathlib.Path(root)
output = root / "artifacts/npu/build/poc16/executors"
paths = [
    "artifacts/npu/build/poc16/runtime/libtim-vx.so",
    "artifacts/npu/build/poc16/runtime-provenance.json",
    "artifacts/npu/build/poc16/runtime-inputs.json",
    "artifacts/npu/runs/rea-poc16a-fp16-conv-20261013-a1/candidate-1.nbg",
    "artifacts/npu/runs/rea-poc16a-fp16-conv-20261013-a1/nbg-report.json",
    "artifacts/npu/runs/rea-poc16b-u8-fc-20261013-a1/candidate-1.nbg",
    "artifacts/npu/runs/rea-poc16b-u8-fc-20261013-a1/nbg-report.json",
    "waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h",
    "waved/tools/npu-compiler/build-device-op-dtype-executors.sh",
    "waved/tools/npu-compiler/verify-device-op-dtype-executor-policy.sh",
]
for kind in ("fp16-conv", "u8-fc"):
    paths += [
        "waved/tools/npu-compiler/wave_npu_execute_" + kind.replace("-", "_") + ".cc",
        "artifacts/npu/build/poc16/executors/wave-npu-execute-" + kind + "-v1",
        "artifacts/npu/build/poc16/executors/" + kind + "-elf-inspection.txt",
        "artifacts/npu/build/poc16/executors/" + kind + "-policy.log",
    ]
paths += ["artifacts/npu/build/poc16/executors/build.log"]
data = {
    "schema_version": 1,
    "mode": "offline-build-and-inspection-only",
    "image_id": image,
    "source_commit": commit,
    "source_date_epoch": 1785196800,
    "compiler": "aarch64-linux-gnu-g++ 5.4.0",
    "standard": "c++17",
    "source_sdk_mount": "read-only",
    "network": "none",
    "sha256": {path: hashlib.sha256((root / path).read_bytes()).hexdigest()
               for path in paths},
}
with (output / "executor-build-provenance.json").open("x") as handle:
    json.dump(data, handle, indent=2)
    handle.write("\n")
PY

echo "build: fixed FP16 Conv and U8 FC executors produced and inspected offline"
