#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
EXPECTED_KERNEL="Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux"
REMOTE_ROOT="/tmp/wave-npu-poc14b"

LOCAL_EXECUTABLE="$REPOSITORY_ROOT/artifacts/npu/build/poc14b/wave-npu-compile-erf-affine-v1"
LOCAL_TIM_VX="$REPOSITORY_ROOT/waved/target/tim-vx/libtim-vx.so"
LOCAL_HEADER="$REPOSITORY_ROOT/waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h"

EXECUTABLE_SHA256="eab129f936cd3fc22142acd7abe55d9ecd276d1a2d7e76d0c647acf04af75fe9"
TIM_VX_SHA256="d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"
HEADER_SHA256="908d36ab616e27cabf5022bf16185825ce1c49f91fbaa37b98e67b4b6bb9af57"

local_sha256() {
    shasum -a 256 "$1" | awk '{print $1}'
}

verify_local() {
    local path="$1"
    local expected="$2"
    if [[ ! -f "$path" ]]; then
        echo "stage: missing local artifact: $path" >&2
        exit 10
    fi
    if [[ "$(local_sha256 "$path")" != "$expected" ]]; then
        echo "stage: local hash mismatch: $path" >&2
        exit 11
    fi
}

verify_local "$LOCAL_EXECUTABLE" "$EXECUTABLE_SHA256"
verify_local "$LOCAL_TIM_VX" "$TIM_VX_SHA256"
verify_local "$LOCAL_HEADER" "$HEADER_SHA256"

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    exit 12
fi
if [[ "$(adb shell uname -a | tr -d '\r')" != "$EXPECTED_KERNEL" ]]; then
    echo "stage: kernel identity differs" >&2
    exit 13
fi
galcore_metadata="$(adb shell "ls -ln /dev/galcore" | tr -d '\r')"
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
        adb shell "cat '/sys/module/galcore/parameters/$name'" |
            tr -d '\r'
    )"
    if [[ "$observed" != "$expected" ]]; then
        echo "stage: NPU parameter differs: $name" >&2
        exit 13
    fi
done
if [[ "$(adb shell "if [ -e '$REMOTE_ROOT' ]; then printf PRESENT; else printf ABSENT; fi" | tr -d '\r')" != "ABSENT" ]]; then
    echo "stage: refusing pre-existing remote root $REMOTE_ROOT" >&2
    exit 14
fi
for clc_path in /lib/libCLC.so /usr/lib/libCLC.so; do
    if [[ "$(adb shell "if [ -e '$clc_path' ]; then printf PRESENT; else printf ABSENT; fi" | tr -d '\r')" != "ABSENT" ]]; then
        echo "stage: quarantined CLC is visible: $clc_path" >&2
        exit 15
    fi
done
# shellcheck disable=SC2016  # Expand fd/target only in the remote shell.
holders="$(adb shell '
for fd in /proc/[0-9]*/fd/*; do
    target=$(readlink "$fd" 2>/dev/null) || continue
    [ "$target" = "/dev/galcore" ] || continue
    printf "%s\n" "$fd"
done
' | tr -d '\r')"
if [[ -n "$holders" ]]; then
    echo "stage: galcore already has a holder" >&2
    exit 16
fi
tmp_available="$(
    adb shell "df -Pk /tmp | awk 'NR == 2 {print \$4 * 1024}'" |
        tr -d '\r'
)"
if [[ ! "$tmp_available" =~ ^[0-9]+$ ]] || (( tmp_available < 33554432 )); then
    echo "stage: insufficient or malformed /tmp capacity" >&2
    exit 16
fi

adb shell "
    umask 022
    mkdir -p \
        '$REMOTE_ROOT/bin' \
        '$REMOTE_ROOT/lib' \
        '$REMOTE_ROOT/sdk/include/CL' \
        '$REMOTE_ROOT/output'
    chmod 0755 \
        '$REMOTE_ROOT' \
        '$REMOTE_ROOT/bin' \
        '$REMOTE_ROOT/lib' \
        '$REMOTE_ROOT/sdk' \
        '$REMOTE_ROOT/sdk/include' \
        '$REMOTE_ROOT/sdk/include/CL' \
        '$REMOTE_ROOT/output'
"
adb push "$LOCAL_EXECUTABLE" "$REMOTE_ROOT/bin/wave-npu-compile-erf-affine-v1"
adb push "$LOCAL_TIM_VX" "$REMOTE_ROOT/lib/libtim-vx.so"
adb push "$LOCAL_HEADER" "$REMOTE_ROOT/sdk/include/CL/cl_viv_vx_ext.h"
adb shell "
    chmod 0555 '$REMOTE_ROOT/bin/wave-npu-compile-erf-affine-v1'
    chmod 0444 \
        '$REMOTE_ROOT/lib/libtim-vx.so' \
        '$REMOTE_ROOT/sdk/include/CL/cl_viv_vx_ext.h'
"

remote_hashes="$(adb shell "
    sha256sum \
        '$REMOTE_ROOT/bin/wave-npu-compile-erf-affine-v1' \
        '$REMOTE_ROOT/lib/libtim-vx.so' \
        '$REMOTE_ROOT/sdk/include/CL/cl_viv_vx_ext.h'
" | tr -d '\r')"
for expected in "$EXECUTABLE_SHA256" "$TIM_VX_SHA256" "$HEADER_SHA256"; do
    if ! grep -Fq "$expected" <<<"$remote_hashes"; then
        echo "stage: remote hash verification failed" >&2
        exit 17
    fi
done
echo "stage: PoC 14B fixed compile-only payload is ready"
