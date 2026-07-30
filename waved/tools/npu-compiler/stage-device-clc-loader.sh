#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
REMOTE_ROOT="/tmp/wave-npu-compile"

LOCAL_PROBE="$REPOSITORY_ROOT/artifacts/npu/build/poc5f/wave-npu-clc-loader-probe-v1"
LOCAL_CLC="$REPOSITORY_ROOT/waved/third_party/vip9000-sdk/6.4.18.5/drivers/libCLC.so"

PROBE_SHA256="33efe6b0e4cf081e9b871fb12dd50654b52912cd566252caeba51883b72f8e79"
CLC_SHA256="a1e5d11cafb6d23c7c787d24a71a766782ea022346f1580797d892238bd99f14"

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

verify_local "$LOCAL_PROBE" "$PROBE_SHA256"
verify_local "$LOCAL_CLC" "$CLC_SHA256"

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
    mkdir -p '$REMOTE_ROOT/bin' '$REMOTE_ROOT/lib'
    chmod 0755 '$REMOTE_ROOT' '$REMOTE_ROOT/bin' '$REMOTE_ROOT/lib'
"

stage_one \
    "$LOCAL_CLC" \
    "$REMOTE_ROOT/lib/libCLC.so" \
    "$CLC_SHA256" \
    "444" \
    "-r--r--r--"
stage_one \
    "$LOCAL_PROBE" \
    "$REMOTE_ROOT/bin/wave-npu-clc-loader-probe-v1" \
    "$PROBE_SHA256" \
    "555" \
    "-r-xr-xr-x"

echo "stage: PoC 5F-A relocation-only CLC payload is ready"
