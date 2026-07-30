#!/usr/bin/env bash
set -euo pipefail

EXPECTED_SERIAL="PSP_0000000000000000"
REMOTE_ROOT="/tmp/wave-npu-poc15a"

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "cleanup: expected only device $EXPECTED_SERIAL" >&2
    exit 10
fi
root_state="$(
    adb -s "$EXPECTED_SERIAL" shell "
        if [ -L '$REMOTE_ROOT' ]; then
            printf SYMLINK
        elif [ -d '$REMOTE_ROOT' ]; then
            printf DIRECTORY
        elif [ -e '$REMOTE_ROOT' ]; then
            printf OTHER
        else
            printf ABSENT
        fi
    " | tr -d '\r'
)"
if [[ "$root_state" == "ABSENT" ]]; then
    echo "cleanup: PoC 15A root is already absent"
    exit 0
fi
if [[ "$root_state" != "DIRECTORY" ]]; then
    echo "cleanup: refusing non-directory or symlink root" >&2
    exit 11
fi
processes="$(
    adb -s "$EXPECTED_SERIAL" shell \
        "ps 2>/dev/null | grep '[w]ave-npu-benchmark-ctc-cpu-vs-npu' || true" |
        tr -d '\r'
)"
if [[ -n "$processes" ]]; then
    echo "cleanup: benchmark process is still running" >&2
    exit 12
fi
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
    echo "cleanup: galcore still has a holder" >&2
    exit 12
fi

unexpected="$(
    adb -s "$EXPECTED_SERIAL" shell "
        find '$REMOTE_ROOT' -mindepth 1 -print | while IFS= read -r path; do
            case \"\$path\" in
                '$REMOTE_ROOT/bin'|\
                '$REMOTE_ROOT/lib'|\
                '$REMOTE_ROOT/meta'|\
                '$REMOTE_ROOT/model'|\
                '$REMOTE_ROOT/result'|\
                '$REMOTE_ROOT/bin/wave-npu-benchmark-ctc-cpu-vs-npu-v1'|\
                '$REMOTE_ROOT/meta/manifest.json'|\
                '$REMOTE_ROOT/model/ctc.onnx'|\
                '$REMOTE_ROOT/model/ctc_online.nb'|\
                '$REMOTE_ROOT/result/ctc-cpu-npu-timings-v1.bin')
                    ;;
                *)
                    printf '%s\n' \"\$path\"
                    ;;
            esac
        done
    " | tr -d '\r'
)"
if [[ -n "$unexpected" ]]; then
    echo "cleanup: refusing unexpected path below fixed root:" >&2
    echo "$unexpected" >&2
    exit 13
fi

adb -s "$EXPECTED_SERIAL" shell \
    "rm -rf -- '$REMOTE_ROOT'"
final_state="$(
    adb -s "$EXPECTED_SERIAL" shell \
        "if [ -e '$REMOTE_ROOT' ] || [ -L '$REMOTE_ROOT' ]; then printf PRESENT; else printf ABSENT; fi" |
        tr -d '\r'
)"
if [[ "$final_state" != "ABSENT" ]]; then
    echo "cleanup: fixed root remains" >&2
    exit 14
fi
echo "cleanup: removed only $REMOTE_ROOT; device evidence remains in the host run directory"
