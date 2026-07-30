#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
REMOTE_ROOT="/tmp/wave-npu-compile"

LOCAL_EXECUTABLE="$REPOSITORY_ROOT/artifacts/npu/build/poc5f/wave-npu-compile-signal-frame-clc-v1"
LOCAL_TIM_VX="$REPOSITORY_ROOT/waved/target/tim-vx/libtim-vx.so"
LOCAL_CLC="$REPOSITORY_ROOT/waved/third_party/vip9000-sdk/6.4.18.5/drivers/libCLC.so"
LOCAL_HEADER="$REPOSITORY_ROOT/waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h"

EXECUTABLE_SHA256="883bb4aa3593af8d226053b0a3760f6974ab46c4516ed02f5e6953a194a4381a"
TIM_VX_SHA256="d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"
CLC_SHA256="a1e5d11cafb6d23c7c787d24a71a766782ea022346f1580797d892238bd99f14"
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
    local observed
    observed="$(local_sha256 "$path")"
    if [[ "$observed" != "$expected" ]]; then
        echo "stage: local hash mismatch: $path" >&2
        exit 11
    fi
}

remote_sha256_or_absent() {
    local path="$1"
    adb shell "
        if [ -e '$path' ]; then
            sha256sum '$path' | cut -d ' ' -f 1
        else
            printf 'ABSENT\n'
        fi
    " | tr -d '\r'
}

require_remote() {
    local path="$1"
    local expected="$2"
    local expected_permissions="$3"
    local observed
    local permissions
    observed="$(remote_sha256_or_absent "$path")"
    permissions="$(
        adb shell "ls -ldn '$path' 2>/dev/null | awk '{print \$1}'" |
            tr -d '\r'
    )"
    if [[ "$observed" != "$expected" ||
          "$permissions" != "$expected_permissions" ]]; then
        echo "stage: required artifact mismatch: $path" >&2
        exit 12
    fi
}

stage_executable() {
    local remote_path="$REMOTE_ROOT/bin/wave-npu-compile-signal-frame-clc-v1"
    local incoming="${remote_path}.incoming-${EXECUTABLE_SHA256:0:16}"
    local existing
    existing="$(remote_sha256_or_absent "$remote_path")"
    if [[ "$existing" == "$EXECUTABLE_SHA256" ]]; then
        require_remote "$remote_path" "$EXECUTABLE_SHA256" "-r-xr-xr-x"
        echo "stage: already present and verified: $remote_path"
        return
    fi
    if [[ "$existing" != "ABSENT" ||
          "$(remote_sha256_or_absent "$incoming")" != "ABSENT" ]]; then
        echo "stage: refusing to replace compiler artifact" >&2
        exit 13
    fi
    adb push "$LOCAL_EXECUTABLE" "$incoming"
    if [[ "$(remote_sha256_or_absent "$incoming")" != "$EXECUTABLE_SHA256" ]]; then
        echo "stage: pushed compiler hash mismatch" >&2
        exit 14
    fi
    adb shell "
        chmod 0555 '$incoming' &&
        ln '$incoming' '$remote_path' &&
        rm -f '$incoming'
    "
    require_remote "$remote_path" "$EXECUTABLE_SHA256" "-r-xr-xr-x"
    echo "stage: installed and verified: $remote_path"
}

verify_local "$LOCAL_EXECUTABLE" "$EXECUTABLE_SHA256"
verify_local "$LOCAL_TIM_VX" "$TIM_VX_SHA256"
verify_local "$LOCAL_CLC" "$CLC_SHA256"
verify_local "$LOCAL_HEADER" "$HEADER_SHA256"

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    exit 17
fi

adb shell "
    umask 022
    mkdir -p '$REMOTE_ROOT/bin' '$REMOTE_ROOT/output'
    chmod 0755 '$REMOTE_ROOT' '$REMOTE_ROOT/bin' '$REMOTE_ROOT/output'
"

require_remote \
    "$REMOTE_ROOT/lib/libtim-vx.so" \
    "$TIM_VX_SHA256" \
    "-r--r--r--"
require_remote \
    "$REMOTE_ROOT/lib/libCLC.so" \
    "$CLC_SHA256" \
    "-r--r--r--"
require_remote \
    "$REMOTE_ROOT/sdk/include/CL/cl_viv_vx_ext.h" \
    "$HEADER_SHA256" \
    "-r--r--r--"

for output in \
    "$REMOTE_ROOT/output/wave-signal-frame-clc-candidate-1.nbg" \
    "$REMOTE_ROOT/output/.wave-signal-frame-clc-candidate-1.nbg.partial" \
    "$REMOTE_ROOT/output/wave-signal-frame-clc-candidate-2.nbg" \
    "$REMOTE_ROOT/output/.wave-signal-frame-clc-candidate-2.nbg.partial"; do
    if [[ "$(remote_sha256_or_absent "$output")" != "ABSENT" ]]; then
        echo "stage: candidate output already exists; refusing: $output" >&2
        exit 18
    fi
done

stage_executable

echo "stage: PoC 5F-B compile-only SignalFrame CLC payload is ready"
