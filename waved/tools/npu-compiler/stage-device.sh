#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
REMOTE_ROOT="/tmp/wave-npu-compile"

LOCAL_EXECUTABLE="$REPOSITORY_ROOT/artifacts/npu/build/poc5b/wave-npu-compile-add"
LOCAL_TIM_VX="$REPOSITORY_ROOT/waved/target/tim-vx/libtim-vx.so"
LOCAL_HEADER="$REPOSITORY_ROOT/waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h"

EXECUTABLE_SHA256="76638c6bcbbab15cd2f466519abc6817521cddda6854b9c6da9a1840f350f10a"
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
    local observed
    observed="$(local_sha256 "$path")"
    if [[ "$observed" != "$expected" ]]; then
        echo "stage: local hash mismatch: $path" >&2
        echo "expected=$expected observed=$observed" >&2
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

stage_one() {
    local local_path="$1"
    local remote_path="$2"
    local expected="$3"
    local expected_mode="$4"
    local expected_permissions="$5"
    local incoming="${remote_path}.incoming-${expected:0:16}"
    local existing
    existing="$(remote_sha256_or_absent "$remote_path")"

    if [[ "$existing" == "$expected" ]]; then
        local existing_permissions
        existing_permissions="$(
            adb shell "ls -ldn '$remote_path' | awk '{print \$1}'" |
                tr -d '\r'
        )"
        if [[ "$existing_permissions" != "$expected_permissions" ]]; then
            echo "stage: existing artifact has wrong mode: $remote_path" >&2
            exit 12
        fi
        echo "stage: already present and verified: $remote_path"
        return
    fi
    if [[ "$existing" != "ABSENT" ]]; then
        echo "stage: refusing to replace unexpected artifact: $remote_path" >&2
        exit 13
    fi
    if [[ "$(remote_sha256_or_absent "$incoming")" != "ABSENT" ]]; then
        echo "stage: refusing to replace stale incoming artifact: $incoming" >&2
        exit 14
    fi

    adb push "$local_path" "$incoming"
    local incoming_hash
    incoming_hash="$(remote_sha256_or_absent "$incoming")"
    if [[ "$incoming_hash" != "$expected" ]]; then
        echo "stage: pushed artifact hash mismatch: $incoming" >&2
        exit 15
    fi
    adb shell "
        chmod '$expected_mode' '$incoming' &&
        ln '$incoming' '$remote_path' &&
        rm -f '$incoming'
    "
    local final_hash
    local final_permissions
    final_hash="$(remote_sha256_or_absent "$remote_path")"
    final_permissions="$(
        adb shell "ls -ldn '$remote_path' | awk '{print \$1}'" |
            tr -d '\r'
    )"
    if [[ "$final_hash" != "$expected" ||
          "$final_permissions" != "$expected_permissions" ]]; then
        echo "stage: final verification failed: $remote_path" >&2
        exit 16
    fi
    echo "stage: installed and verified: $remote_path"
}

verify_local "$LOCAL_EXECUTABLE" "$EXECUTABLE_SHA256"
verify_local "$LOCAL_TIM_VX" "$TIM_VX_SHA256"
verify_local "$LOCAL_HEADER" "$HEADER_SHA256"

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    printf 'stage: observed device(s): %s\n' "${connected_serials:-none}" >&2
    exit 17
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

for output in \
    "$REMOTE_ROOT/output/wave-add-candidate-1.nbg" \
    "$REMOTE_ROOT/output/.wave-add-candidate-1.nbg.partial" \
    "$REMOTE_ROOT/output/wave-add-candidate-2.nbg" \
    "$REMOTE_ROOT/output/.wave-add-candidate-2.nbg.partial"; do
    if [[ "$(remote_sha256_or_absent "$output")" != "ABSENT" ]]; then
        echo "stage: candidate output already exists; refusing: $output" >&2
        exit 18
    fi
done

stage_one \
    "$LOCAL_EXECUTABLE" \
    "$REMOTE_ROOT/bin/wave-npu-compile-add-v2" \
    "$EXECUTABLE_SHA256" \
    "555" \
    "-r-xr-xr-x"
stage_one \
    "$LOCAL_TIM_VX" \
    "$REMOTE_ROOT/lib/libtim-vx.so" \
    "$TIM_VX_SHA256" \
    "444" \
    "-r--r--r--"
stage_one \
    "$LOCAL_HEADER" \
    "$REMOTE_ROOT/sdk/include/CL/cl_viv_vx_ext.h" \
    "$HEADER_SHA256" \
    "444" \
    "-r--r--r--"

echo "stage: PoC 5B compile-only payload is ready"
