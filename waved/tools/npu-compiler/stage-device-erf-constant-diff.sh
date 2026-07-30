#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 0 ]]; then
    echo "usage: $0" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
REMOTE_ROOT="/tmp/wave-npu-poc12b"

LOCAL_SOURCE="$SCRIPT_DIR/tim_vx_compile_erf_constant_diff.cc"
LOCAL_EXECUTABLE="$REPOSITORY_ROOT/artifacts/npu/build/poc12b-erf-constant-diff/wave-npu-compile-erf-constant-diff-v1"
LOCAL_TIM_VX="$REPOSITORY_ROOT/waved/target/tim-vx/libtim-vx.so"
LOCAL_HEADER="$REPOSITORY_ROOT/waved/third_party/device-sdk/include/CL/cl_viv_vx_ext.h"

SOURCE_SHA256="e7c4bb1a4fbb2efa437ae3afad4e1e417dd51a0f2fc38e36e7596aecf0a72f50"
EXECUTABLE_SHA256="29a19a7a9b818335a6f31f3b84ca2469073302a2f3b051e0ae6c9c4a56ff6721"
TIM_VX_SHA256="d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"
HEADER_SHA256="908d36ab616e27cabf5022bf16185825ce1c49f91fbaa37b98e67b4b6bb9af57"

local_sha256() {
    shasum -a 256 "$1" | awk '{print $1}'
}

verify_local() {
    local path="$1"
    local expected="$2"
    if [[ -L "$path" || ! -f "$path" ]]; then
        echo "stage: missing or unsafe local artifact: $path" >&2
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

verify_local "$LOCAL_SOURCE" "$SOURCE_SHA256"
verify_local "$LOCAL_EXECUTABLE" "$EXECUTABLE_SHA256"
verify_local "$LOCAL_TIM_VX" "$TIM_VX_SHA256"
verify_local "$LOCAL_HEADER" "$HEADER_SHA256"

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    printf 'stage: observed device(s): %s\n' \
        "${connected_serials:-none}" >&2
    exit 12
fi
ADB=(adb -s "$EXPECTED_SERIAL")

root_state="$(
    "${ADB[@]}" shell "
        if [ -L '$REMOTE_ROOT' ]; then printf 'SYMLINK\n';
        elif [ -e '$REMOTE_ROOT' ]; then printf 'PRESENT\n';
        else printf 'ABSENT\n'; fi
    " | tr -d '\r'
)"
if [[ "$root_state" != "ABSENT" ]]; then
    echo "stage: refusing pre-existing remote root: $root_state" >&2
    exit 13
fi

"${ADB[@]}" shell "
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

remote_sha256_or_absent() {
    local path="$1"
    "${ADB[@]}" shell "
        if [ -L '$path' ]; then
            printf 'SYMLINK\n'
        elif [ -f '$path' ]; then
            sha256sum '$path' | cut -d ' ' -f 1
        elif [ -e '$path' ]; then
            printf 'OTHER\n'
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

    if [[ "$(remote_sha256_or_absent "$remote_path")" != "ABSENT" ||
          "$(remote_sha256_or_absent "$incoming")" != "ABSENT" ]]; then
        echo "stage: refusing an existing final/incoming path: $remote_path" >&2
        exit 14
    fi
    "${ADB[@]}" push "$local_path" "$incoming"
    if [[ "$(remote_sha256_or_absent "$incoming")" != "$expected" ]]; then
        echo "stage: incoming hash mismatch: $incoming" >&2
        exit 15
    fi
    "${ADB[@]}" shell "
        chmod '$expected_mode' '$incoming' &&
        ln '$incoming' '$remote_path' &&
        rm -f '$incoming'
    "
    local final_hash
    local final_permissions
    final_hash="$(remote_sha256_or_absent "$remote_path")"
    final_permissions="$(
        "${ADB[@]}" shell \
            "ls -ldn '$remote_path' | awk '{print \$1}'" |
            tr -d '\r'
    )"
    if [[ "$final_hash" != "$expected" ||
          "$final_permissions" != "$expected_permissions" ]]; then
        echo "stage: final verification failed: $remote_path" >&2
        exit 16
    fi
}

for output in \
    "$REMOTE_ROOT/output/erf-constant-a1.nbg" \
    "$REMOTE_ROOT/output/.erf-constant-a1.nbg.partial" \
    "$REMOTE_ROOT/output/erf-constant-a2.nbg" \
    "$REMOTE_ROOT/output/.erf-constant-a2.nbg.partial" \
    "$REMOTE_ROOT/output/erf-constant-b.nbg" \
    "$REMOTE_ROOT/output/.erf-constant-b.nbg.partial"; do
    if [[ "$(remote_sha256_or_absent "$output")" != "ABSENT" ]]; then
        echo "stage: fixed output already exists: $output" >&2
        exit 17
    fi
done

stage_one \
    "$LOCAL_EXECUTABLE" \
    "$REMOTE_ROOT/bin/wave-npu-compile-erf-constant-diff-v1" \
    "$EXECUTABLE_SHA256" \
    0555 \
    -r-xr-xr-x
stage_one \
    "$LOCAL_TIM_VX" \
    "$REMOTE_ROOT/lib/libtim-vx.so" \
    "$TIM_VX_SHA256" \
    0444 \
    -r--r--r--
stage_one \
    "$LOCAL_HEADER" \
    "$REMOTE_ROOT/sdk/include/CL/cl_viv_vx_ext.h" \
    "$HEADER_SHA256" \
    0444 \
    -r--r--r--

echo "stage: PoC 12B fixed compile-only payload is ready"
