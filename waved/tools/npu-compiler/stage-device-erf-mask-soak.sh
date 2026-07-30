#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
EXPECTED_KERNEL="Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux"
REMOTE_ROOT="/tmp/wave-npu-poc13f"

LOCAL_EXECUTABLE="$REPOSITORY_ROOT/artifacts/npu/build/poc13f/wave-npu-soak-erf-mask-v1"
LOCAL_TIM_VX="$REPOSITORY_ROOT/waved/target/tim-vx/libtim-vx.so"
LOCAL_NBG="$REPOSITORY_ROOT/artifacts/npu/runs/poc13b-erf-mask-a1-20260729/a1.nbg"
EXECUTABLE_SHA256="ef0d4ece1d44141b1254581aa5b05e5fd371f99f819cfac32b478cbcaaaf4cff"
TIM_VX_SHA256="d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"
NBG_SHA256="96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94"

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
verify_local "$LOCAL_TIM_VX" "$TIM_VX_SHA256"
verify_local "$LOCAL_NBG" "$NBG_SHA256"
if [[ "$(wc -c < "$LOCAL_NBG" | tr -d ' ')" != "5840" ]]; then
    echo "stage: fixed NBG byte count differs" >&2
    exit 11
fi
connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    exit 12
fi
if [[ "$(adb -s "$EXPECTED_SERIAL" shell uname -a | tr -d '\r')" != "$EXPECTED_KERNEL" ]]; then
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
if [[ "$(adb -s "$EXPECTED_SERIAL" shell "if [ -e '$REMOTE_ROOT' ] || [ -L '$REMOTE_ROOT' ]; then printf PRESENT; else printf ABSENT; fi" | tr -d '\r')" != "ABSENT" ]]; then
    echo "stage: refusing pre-existing remote root $REMOTE_ROOT" >&2
    exit 14
fi
for clc_path in /lib/libCLC.so /usr/lib/libCLC.so; do
    if [[ "$(adb -s "$EXPECTED_SERIAL" shell "if [ -e '$clc_path' ] || [ -L '$clc_path' ]; then printf PRESENT; else printf ABSENT; fi" | tr -d '\r')" != "ABSENT" ]]; then
        echo "stage: quarantined CLC is visible: $clc_path" >&2
        exit 15
    fi
done
# shellcheck disable=SC2016  # Expand fd/target only in the remote shell.
holders="$(
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

adb -s "$EXPECTED_SERIAL" shell "
    umask 022
    mkdir -p \
        '$REMOTE_ROOT/bin' \
        '$REMOTE_ROOT/lib' \
        '$REMOTE_ROOT/model'
    chmod 0755 \
        '$REMOTE_ROOT' \
        '$REMOTE_ROOT/bin' \
        '$REMOTE_ROOT/lib' \
        '$REMOTE_ROOT/model'
"
adb -s "$EXPECTED_SERIAL" push \
    "$LOCAL_EXECUTABLE" \
    "$REMOTE_ROOT/bin/wave-npu-soak-erf-mask-v1"
adb -s "$EXPECTED_SERIAL" push \
    "$LOCAL_TIM_VX" \
    "$REMOTE_ROOT/lib/libtim-vx.so"
adb -s "$EXPECTED_SERIAL" push \
    "$LOCAL_NBG" \
    "$REMOTE_ROOT/model/erf-mask.nbg"
adb -s "$EXPECTED_SERIAL" shell "
    chmod 0555 '$REMOTE_ROOT/bin/wave-npu-soak-erf-mask-v1'
    chmod 0444 \
        '$REMOTE_ROOT/lib/libtim-vx.so' \
        '$REMOTE_ROOT/model/erf-mask.nbg'
"
remote_hashes="$(
    adb -s "$EXPECTED_SERIAL" shell "
        sha256sum \
            '$REMOTE_ROOT/bin/wave-npu-soak-erf-mask-v1' \
            '$REMOTE_ROOT/lib/libtim-vx.so' \
            '$REMOTE_ROOT/model/erf-mask.nbg'
    " | tr -d '\r'
)"
for expected in \
    "$EXECUTABLE_SHA256" \
    "$TIM_VX_SHA256" \
    "$NBG_SHA256"; do
    if ! grep -Fq "$expected" <<<"$remote_hashes"; then
        echo "stage: remote hash verification failed" >&2
        exit 17
    fi
done
echo "stage: PoC 13F fixed 10k-frame soak payload is ready"
