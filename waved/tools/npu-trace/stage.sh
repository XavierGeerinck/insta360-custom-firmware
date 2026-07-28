#!/bin/sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPOSITORY_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)"
REMOTE_DIRECTORY="/tmp/wave-npu-lab"

INTERPOSER_LOCAL="$REPOSITORY_ROOT/artifacts/npu/build/poc3/libwave-gal-trace.so"
INTERPOSER_REMOTE="$REMOTE_DIRECTORY/libwave-gal-trace.so"
INTERPOSER_SHA256="dccaedd13812c161949905efecf7aa19fe74712b75de8d80211924e286e68601"

WRAPPER_LOCAL="$SCRIPT_DIR/stock-ctc-trace.sh"
WRAPPER_REMOTE="$REMOTE_DIRECTORY/stock-ctc-trace"
WRAPPER_SHA256="7799626fcad221088a109740022e93ca5b7652d7589c9e79347ec92a1b1ac866"

require_local_hash() {
    local_path="$1"
    expected_digest="$2"
    observed_line="$(sha256sum "$local_path")" || exit 1
    observed_digest="${observed_line%% *}"
    if [ "$observed_digest" != "$expected_digest" ]; then
        echo "local artifact hash mismatch: $local_path" >&2
        echo "expected $expected_digest" >&2
        echo "observed $observed_digest" >&2
        exit 1
    fi
}

remote_hash_or_absent() {
    remote_path="$1"
    adb shell "
        if [ -f '$remote_path' ]; then
            sha256sum '$remote_path'
        else
            echo absent
        fi
    " | {
        read -r first _rest
        printf '%s\n' "$first"
    }
}

stage_one() {
    local_path="$1"
    remote_path="$2"
    expected_digest="$3"
    mode="$4"

    require_local_hash "$local_path" "$expected_digest"
    observed_digest="$(remote_hash_or_absent "$remote_path")"
    case "$observed_digest" in
        absent)
            adb push "$local_path" "$remote_path"
            ;;
        "$expected_digest")
            echo "already staged: $remote_path"
            ;;
        *)
            echo "refusing to overwrite unexpected ephemeral artifact: $remote_path" >&2
            echo "expected $expected_digest" >&2
            echo "observed $observed_digest" >&2
            exit 1
            ;;
    esac

    adb shell "chmod '$mode' '$remote_path'"
    observed_digest="$(remote_hash_or_absent "$remote_path")"
    if [ "$observed_digest" != "$expected_digest" ]; then
        echo "remote read-back hash mismatch: $remote_path" >&2
        exit 1
    fi
    echo "$observed_digest  $remote_path"
}

if [ "$(adb get-state)" != "device" ]; then
    echo "ADB device is unavailable" >&2
    exit 1
fi

adb shell "mkdir -p '$REMOTE_DIRECTORY' && chmod 0755 '$REMOTE_DIRECTORY'"
stage_one "$INTERPOSER_LOCAL" "$INTERPOSER_REMOTE" "$INTERPOSER_SHA256" 0444
stage_one "$WRAPPER_LOCAL" "$WRAPPER_REMOTE" "$WRAPPER_SHA256" 0555
