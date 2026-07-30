#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
EXPECTED_SERIAL="PSP_0000000000000000"
REMOTE_DIRECTORY="/tmp/wave-npu-compile/bin"
REMOTE_PATH="$REMOTE_DIRECTORY/wave-npu-query-caps-v1"
LOCAL_PATH="$REPOSITORY_ROOT/artifacts/npu/build/poc5d/wave-npu-query-caps-v1"
EXPECTED_SHA256="6a1c0ac1af75c31de32e0b8d827bbde898b6a606919d5aef3987967ac459bfe9"
INCOMING="$REMOTE_PATH.incoming-${EXPECTED_SHA256:0:16}"

local_sha256="$(shasum -a 256 "$LOCAL_PATH" | awk '{print $1}')"
if [[ "$local_sha256" != "$EXPECTED_SHA256" ]]; then
    echo "stage: local capability probe hash mismatch" >&2
    exit 10
fi

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "stage: expected only device $EXPECTED_SERIAL" >&2
    exit 11
fi

remote_state="$(
    adb shell "
        if [ -e '$REMOTE_PATH' ]; then
            sha256sum '$REMOTE_PATH' | cut -d ' ' -f 1
        else
            printf 'ABSENT\n'
        fi
    " | tr -d '\r'
)"
if [[ "$remote_state" == "$EXPECTED_SHA256" ]]; then
    permissions="$(
        adb shell "ls -ldn '$REMOTE_PATH' | awk '{print \$1}'" | tr -d '\r'
    )"
    if [[ "$permissions" != "-r-xr-xr-x" ]]; then
        echo "stage: existing capability probe has wrong permissions" >&2
        exit 12
    fi
    echo "stage: capability probe already present and verified"
    exit 0
fi
if [[ "$remote_state" != "ABSENT" ]]; then
    echo "stage: refusing to replace unexpected capability probe" >&2
    exit 13
fi
incoming_state="$(
    adb shell "
        if [ -e '$INCOMING' ]; then
            printf 'PRESENT\n'
        else
            printf 'ABSENT\n'
        fi
    " | tr -d '\r'
)"
if [[ "$incoming_state" != "ABSENT" ]]; then
    echo "stage: refusing to replace stale incoming capability probe" >&2
    exit 14
fi

adb shell "mkdir -p '$REMOTE_DIRECTORY' && chmod 0755 '$REMOTE_DIRECTORY'"
adb push "$LOCAL_PATH" "$INCOMING"
incoming_hash="$(
    adb shell "sha256sum '$INCOMING' | cut -d ' ' -f 1" | tr -d '\r'
)"
if [[ "$incoming_hash" != "$EXPECTED_SHA256" ]]; then
    echo "stage: pushed capability probe hash mismatch" >&2
    exit 15
fi
adb shell "
    chmod 0555 '$INCOMING' &&
    ln '$INCOMING' '$REMOTE_PATH' &&
    rm -f '$INCOMING'
"

final_hash="$(
    adb shell "sha256sum '$REMOTE_PATH' | cut -d ' ' -f 1" | tr -d '\r'
)"
final_permissions="$(
    adb shell "ls -ldn '$REMOTE_PATH' | awk '{print \$1}'" | tr -d '\r'
)"
if [[ "$final_hash" != "$EXPECTED_SHA256" ||
      "$final_permissions" != "-r-xr-xr-x" ]]; then
    echo "stage: final capability probe verification failed" >&2
    exit 16
fi

echo "stage: PoC 5D read-only capability probe is ready"
