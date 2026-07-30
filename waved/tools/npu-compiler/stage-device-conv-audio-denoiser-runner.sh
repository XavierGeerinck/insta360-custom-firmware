#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
REMOTE_ROOT="/tmp/wave-npu-audio8d"

LOCAL_EXECUTABLE="$REPOSITORY_ROOT/artifacts/npu/build/poc8d/wave-npu-run-conv-audio-denoiser-v1"
LOCAL_TIM_VX="$REPOSITORY_ROOT/waved/target/tim-vx/libtim-vx.so"

EXECUTABLE_SHA256="bf3e4716588055ed7c774135ed15a77ad068fddab52b362c0c0c03dfb05f2c9f"
TIM_VX_SHA256="d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"

local_sha256() {
    shasum -a 256 "$1" | awk '{print $1}'
}

verify_local() {
    local path="$1"
    local expected="$2"
    if [[ ! -f "$path" || -L "$path" ]]; then
        echo "stage: missing, non-regular, or symlinked local artifact: $path" >&2
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
    adb -s "$EXPECTED_SERIAL" shell "
        if [ -e '$path' ] || [ -L '$path' ]; then
            if [ -L '$path' ] || [ ! -f '$path' ]; then
                printf 'NOT_REGULAR\n'
            else
                sha256sum '$path' | cut -d ' ' -f 1
            fi
        else
            printf 'ABSENT\n'
        fi
    " | tr -d '\r'
}

require_directory_or_create() {
    local path="$1"
    local state
    state="$(
        adb -s "$EXPECTED_SERIAL" shell "
            if [ -L '$path' ]; then
                printf 'SYMLINK\n'
            elif [ -e '$path' ]; then
                ls -ldn '$path' | awk '{print \$1}'
            else
                printf 'ABSENT\n'
            fi
        " | tr -d '\r'
    )"
    if [[ "$state" == "ABSENT" ]]; then
        adb -s "$EXPECTED_SERIAL" shell "
            umask 022
            mkdir '$path'
            chmod 0755 '$path'
        "
    elif [[ "$state" != "drwxr-xr-x" ]]; then
        echo "stage: refusing unexpected directory state: $path ($state)" >&2
        exit 12
    fi
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
            adb -s "$EXPECTED_SERIAL" shell \
                "ls -ldn '$remote_path' | awk '{print \$1}'" |
                tr -d '\r'
        )"
        if [[ "$existing_permissions" != "$expected_permissions" ]]; then
            echo "stage: existing artifact has wrong mode: $remote_path" >&2
            exit 13
        fi
        echo "stage: already present and verified: $remote_path"
        return
    fi
    if [[ "$existing" != "ABSENT" ]]; then
        echo "stage: refusing to replace unexpected artifact: $remote_path" >&2
        exit 14
    fi
    if [[ "$(remote_sha256_or_absent "$incoming")" != "ABSENT" ]]; then
        echo "stage: refusing stale incoming artifact: $incoming" >&2
        exit 15
    fi

    adb -s "$EXPECTED_SERIAL" push "$local_path" "$incoming"
    if [[ "$(remote_sha256_or_absent "$incoming")" != "$expected" ]]; then
        echo "stage: pushed artifact hash mismatch: $incoming" >&2
        exit 16
    fi
    adb -s "$EXPECTED_SERIAL" shell "
        chmod '$expected_mode' '$incoming' &&
        ln '$incoming' '$remote_path' &&
        rm -f '$incoming'
    "
    local final_permissions
    final_permissions="$(
        adb -s "$EXPECTED_SERIAL" shell \
            "ls -ldn '$remote_path' | awk '{print \$1}'" |
            tr -d '\r'
    )"
    if [[ "$(remote_sha256_or_absent "$remote_path")" != "$expected" ||
          "$final_permissions" != "$expected_permissions" ]]; then
        echo "stage: final verification failed: $remote_path" >&2
        exit 17
    fi
    echo "stage: installed and verified: $remote_path"
}

verify_local "$LOCAL_EXECUTABLE" "$EXECUTABLE_SHA256"
verify_local "$LOCAL_TIM_VX" "$TIM_VX_SHA256"

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    printf 'stage: observed device(s): %s\n' "${connected_serials:-none}" >&2
    exit 18
fi

require_directory_or_create "$REMOTE_ROOT"
require_directory_or_create "$REMOTE_ROOT/bin"
require_directory_or_create "$REMOTE_ROOT/lib"

if [[ "$(remote_sha256_or_absent "$REMOTE_ROOT/lib/libCLC.so")" != "ABSENT" ]]; then
    echo "stage: isolated runtime unexpectedly contains libCLC.so" >&2
    exit 19
fi

stage_one \
    "$LOCAL_EXECUTABLE" \
    "$REMOTE_ROOT/bin/wave-npu-run-conv-audio-denoiser-v1" \
    "$EXECUTABLE_SHA256" \
    "555" \
    "-r-xr-xr-x"
stage_one \
    "$LOCAL_TIM_VX" \
    "$REMOTE_ROOT/lib/libtim-vx.so" \
    "$TIM_VX_SHA256" \
    "444" \
    "-r--r--r--"

if [[ "$(remote_sha256_or_absent "$REMOTE_ROOT/lib/libCLC.so")" != "ABSENT" ]]; then
    echo "stage: libCLC.so appeared during staging" >&2
    exit 20
fi

echo "stage: PoC 8D one-shot convolutional audio-denoiser payload is ready"
