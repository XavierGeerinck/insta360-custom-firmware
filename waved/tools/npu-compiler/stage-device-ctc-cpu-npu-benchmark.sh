#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
EXPECTED_KERNEL="Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux"
REMOTE_ROOT="/tmp/wave-npu-poc15a-r2"

LOCAL_EXECUTABLE="$REPOSITORY_ROOT/artifacts/npu/build/poc15a/wave-npu-benchmark-ctc-cpu-vs-npu-v2"
LOCAL_SOURCE="$SCRIPT_DIR/wave_npu_benchmark_ctc_cpu_vs_npu.cc"
LOCAL_ONNX="$REPOSITORY_ROOT/models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc.onnx"
LOCAL_NBG="$REPOSITORY_ROOT/models/ai-models/asr-models/model/asr_en/reverb_asr_online/ctc_online.nb"
LOCAL_MANIFEST="$REPOSITORY_ROOT/waved/scripts/npu/ctc_cpu_npu_benchmark_manifest.json"
LOCAL_CONTROLLER="$REPOSITORY_ROOT/waved/scripts/npu/npu_ctc_cpu_npu_benchmark_lab.py"

EXECUTABLE_SHA256="d5316b2b45d49edbbc83daf368f32776388580a4e7db3667d0fe4ac88ebccf52"
SOURCE_SHA256="6d924c2779f2636e50aab49656d015165dab487a058c73691b857c14692b7d27"
ONNX_SHA256="8d6d3b6ce9da51f852981bb5bdd33e2818d47d3e545e9efe5d3700a37b367219"
NBG_SHA256="3a9294ea8595789cc23ddc9298ebceb4474738fed07a9a8b10e2442b67b0d8d5"
MANIFEST_SHA256="ca4470dd272ea44e8461bae10b1cdf232f84c435ec471f0dffdf27d4c388026b"

local_sha256() {
    shasum -a 256 "$1" | awk '{print $1}'
}

verify_local() {
    local path="$1"
    local expected="$2"
    if [[ ! -f "$path" || -L "$path" ||
          "$(local_sha256 "$path")" != "$expected" ]]; then
        echo "stage: local artifact differs: $path" >&2
        exit 10
    fi
}

verify_local "$LOCAL_EXECUTABLE" "$EXECUTABLE_SHA256"
verify_local "$LOCAL_SOURCE" "$SOURCE_SHA256"
verify_local "$LOCAL_ONNX" "$ONNX_SHA256"
verify_local "$LOCAL_NBG" "$NBG_SHA256"
verify_local "$LOCAL_MANIFEST" "$MANIFEST_SHA256"
if [[ ! -f "$LOCAL_CONTROLLER" || -L "$LOCAL_CONTROLLER" ]]; then
    echo "stage: fixed host controller is missing" >&2
    exit 10
fi
if [[ "$(wc -c < "$LOCAL_EXECUTABLE" | tr -d ' ')" != "49848" ||
      "$(wc -c < "$LOCAL_ONNX" | tr -d ' ')" != "10258790" ||
      "$(wc -c < "$LOCAL_NBG" | tr -d ' ')" != "5263360" ]]; then
    echo "stage: fixed payload byte count differs" >&2
    exit 11
fi

"$SCRIPT_DIR/verify-device-ctc-cpu-npu-benchmark-policy.sh" \
    "$LOCAL_EXECUTABLE" \
    "$LOCAL_SOURCE"
python3 "$LOCAL_CONTROLLER" manifest >/dev/null

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    exit 12
fi
observed_kernel="$(
    adb -s "$EXPECTED_SERIAL" shell uname -a | tr -d '\r'
)"
if [[ "$observed_kernel" != "$EXPECTED_KERNEL" ]]; then
    echo "stage: kernel identity differs" >&2
    exit 13
fi
galcore_metadata="$(
    adb -s "$EXPECTED_SERIAL" shell "ls -ln /dev/galcore" |
        tr -d '\r'
)"
if ! grep -Eq '199,[[:space:]]*0' <<<"$galcore_metadata"; then
    echo "stage: /dev/galcore is not character device 199:0" >&2
    exit 13
fi
for parameter in \
    "enableNN:255" \
    "nanoqFreq:838860800" \
    "recovery:1"; do
    name="${parameter%%:*}"
    expected="${parameter#*:}"
    observed="$(
        adb -s "$EXPECTED_SERIAL" shell \
            "cat '/sys/module/galcore/parameters/$name'" |
            tr -d '\r'
    )"
    if [[ "$observed" != "$expected" ]]; then
        echo "stage: NPU parameter differs: $name" >&2
        exit 13
    fi
done
root_state="$(
    adb -s "$EXPECTED_SERIAL" shell \
        "if [ -e '$REMOTE_ROOT' ] || [ -L '$REMOTE_ROOT' ]; then printf PRESENT; else printf ABSENT; fi" |
        tr -d '\r'
)"
if [[ "$root_state" != "ABSENT" ]]; then
    echo "stage: refusing pre-existing remote root $REMOTE_ROOT" >&2
    exit 14
fi
for clc_path in /lib/libCLC.so /usr/lib/libCLC.so; do
    clc_state="$(
        adb -s "$EXPECTED_SERIAL" shell \
            "if [ -e '$clc_path' ] || [ -L '$clc_path' ]; then printf PRESENT; else printf ABSENT; fi" |
            tr -d '\r'
    )"
    if [[ "$clc_state" != "ABSENT" ]]; then
        echo "stage: quarantined CLC is visible: $clc_path" >&2
        exit 15
    fi
done
holders="$(
    # shellcheck disable=SC2016  # Expand fd/target only in the remote shell.
    adb -s "$EXPECTED_SERIAL" shell '
for fd in /proc/[0-9]*/fd/*; do
    target=$(readlink "$fd" 2>/dev/null) || continue
    [ "$target" = "/dev/galcore" ] || continue
    printf "%s\n" "$fd"
done
' | tr -d '\r'
)"
if [[ -n "$holders" ]]; then
    echo "stage: galcore already has a holder" >&2
    exit 16
fi
processes="$(
    adb -s "$EXPECTED_SERIAL" shell \
        "ps 2>/dev/null | grep '[w]ave-npu-benchmark-ctc-cpu-vs-npu' || true" |
        tr -d '\r'
)"
if [[ -n "$processes" ]]; then
    echo "stage: PoC 15A process already exists" >&2
    exit 16
fi

declare -a system_artifacts=(
    "/usr/lib/libArchModelSw.so:2b3df0844a2ff582f71da6517113939b54acdc7c97bebcac771e6e58140df63f"
    "/usr/lib/libGAL.so:13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6"
    "/usr/lib/libNNArchPerf.so:253167677e9839d09815de93a86423824f26f56f097c100d59c03170adecffa9"
    "/usr/lib/libOpenVX.so:f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb"
    "/usr/lib/libVSC.so:e9a39fe3b522b3926ff34f3fb380c73983ad8d48c5f40ffd6e6d8024adc2ed18"
    "/usr/lib/libnnsdk.so:36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe"
    "/usr/lib/libonnxruntime.so.1.15.1:457784da0d1c525171b15438cfbaf869f57e9317817e30691136b8ebe10d4639"
    "/usr/lib/libovxlib.so:09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9"
)
for artifact in "${system_artifacts[@]}"; do
    path="${artifact%%:*}"
    expected="${artifact#*:}"
    observed="$(
        adb -s "$EXPECTED_SERIAL" shell "sha256sum '$path'" |
            tr -d '\r' |
            awk '{print $1}'
    )"
    if [[ "$observed" != "$expected" ]]; then
        echo "stage: stock runtime artifact differs: $path" >&2
        exit 17
    fi
done

adb -s "$EXPECTED_SERIAL" shell "
    umask 022
    mkdir -p \
        '$REMOTE_ROOT/bin' \
        '$REMOTE_ROOT/lib' \
        '$REMOTE_ROOT/meta' \
        '$REMOTE_ROOT/model' \
        '$REMOTE_ROOT/result'
    chmod 0755 \
        '$REMOTE_ROOT' \
        '$REMOTE_ROOT/bin' \
        '$REMOTE_ROOT/lib' \
        '$REMOTE_ROOT/meta' \
        '$REMOTE_ROOT/model' \
        '$REMOTE_ROOT/result'
"
adb -s "$EXPECTED_SERIAL" push \
    "$LOCAL_EXECUTABLE" \
    "$REMOTE_ROOT/bin/wave-npu-benchmark-ctc-cpu-vs-npu-v2"
adb -s "$EXPECTED_SERIAL" push \
    "$LOCAL_ONNX" \
    "$REMOTE_ROOT/model/ctc.onnx"
adb -s "$EXPECTED_SERIAL" push \
    "$LOCAL_NBG" \
    "$REMOTE_ROOT/model/ctc_online.nb"
adb -s "$EXPECTED_SERIAL" push \
    "$LOCAL_MANIFEST" \
    "$REMOTE_ROOT/meta/manifest.json"
adb -s "$EXPECTED_SERIAL" shell "
    chmod 0555 \
        '$REMOTE_ROOT/bin/wave-npu-benchmark-ctc-cpu-vs-npu-v2'
    chmod 0444 \
        '$REMOTE_ROOT/model/ctc.onnx' \
        '$REMOTE_ROOT/model/ctc_online.nb' \
        '$REMOTE_ROOT/meta/manifest.json'
"
remote_hashes="$(
    adb -s "$EXPECTED_SERIAL" shell "
        sha256sum \
            '$REMOTE_ROOT/bin/wave-npu-benchmark-ctc-cpu-vs-npu-v2' \
            '$REMOTE_ROOT/model/ctc.onnx' \
            '$REMOTE_ROOT/model/ctc_online.nb' \
            '$REMOTE_ROOT/meta/manifest.json'
    " | tr -d '\r'
)"
for expected in \
    "$EXECUTABLE_SHA256" \
    "$ONNX_SHA256" \
    "$NBG_SHA256" \
    "$MANIFEST_SHA256"; do
    if ! grep -Fq "$expected" <<<"$remote_hashes"; then
        echo "stage: remote hash verification failed" >&2
        exit 18
    fi
done

echo "stage: PoC 15A-R2 fixed CTC CPU/NPU benchmark payload is ready"
