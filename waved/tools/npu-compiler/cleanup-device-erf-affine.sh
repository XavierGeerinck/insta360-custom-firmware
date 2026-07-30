#!/usr/bin/env bash
set -euo pipefail

EXPECTED_SERIAL="PSP_0000000000000000"
EXPECTED_KERNEL="Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux"
REMOTE_ROOT="/tmp/wave-npu-poc14b"
EXECUTABLE_SHA256="eab129f936cd3fc22142acd7abe55d9ecd276d1a2d7e76d0c647acf04af75fe9"
TIM_VX_SHA256="d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b"
HEADER_SHA256="908d36ab616e27cabf5022bf16185825ce1c49f91fbaa37b98e67b4b6bb9af57"

connected_serials="$(
    adb devices | awk 'NR > 1 && $2 == "device" {print $1}'
)"
if [[ "$connected_serials" != "$EXPECTED_SERIAL" ]]; then
    echo "cleanup: expected only device $EXPECTED_SERIAL" >&2
    exit 10
fi
if [[ "$(adb shell uname -a | tr -d '\r')" != "$EXPECTED_KERNEL" ]]; then
    echo "cleanup: kernel identity differs" >&2
    exit 11
fi
root_state="$(
    adb shell "
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
if [[ "$root_state" != "DIRECTORY" ]]; then
    echo "cleanup: exact stage root is not the expected directory" >&2
    exit 12
fi
processes="$(
    adb shell "ps 2>/dev/null | grep '[w]ave-npu-compile-erf-affine' || true" |
        tr -d '\r'
)"
if [[ -n "$processes" ]]; then
    echo "cleanup: affine compiler process is still visible" >&2
    exit 13
fi
# shellcheck disable=SC2016  # Expand fd/target only in the remote shell.
holders="$(adb shell '
for fd in /proc/[0-9]*/fd/*; do
    target=$(readlink "$fd" 2>/dev/null) || continue
    [ "$target" = "/dev/galcore" ] || continue
    printf "%s\n" "$fd"
done
' | tr -d '\r')"
if [[ -n "$holders" ]]; then
    echo "cleanup: galcore still has a holder" >&2
    exit 13
fi

remote_hashes="$(adb shell "
    sha256sum \
        '$REMOTE_ROOT/bin/wave-npu-compile-erf-affine-v1' \
        '$REMOTE_ROOT/lib/libtim-vx.so' \
        '$REMOTE_ROOT/sdk/include/CL/cl_viv_vx_ext.h'
" | tr -d '\r')"
for expected in "$EXECUTABLE_SHA256" "$TIM_VX_SHA256" "$HEADER_SHA256"; do
    if ! grep -Fq "$expected" <<<"$remote_hashes"; then
        echo "cleanup: staged artifact hash differs" >&2
        exit 14
    fi
done
file_count="$(
    adb shell "find '$REMOTE_ROOT' -type f | wc -l" | tr -d '\r '
)"
link_count="$(
    adb shell "find '$REMOTE_ROOT' -type l | wc -l" | tr -d '\r '
)"
output_count="$(
    adb shell "find '$REMOTE_ROOT/output' -mindepth 1 | wc -l" |
        tr -d '\r '
)"
if [[ "$file_count" != "3" || "$link_count" != "0" || "$output_count" != "0" ]]; then
    echo "cleanup: stage contents differ from the clean-negative layout" >&2
    exit 15
fi

adb shell "rm -rf '$REMOTE_ROOT'"
if [[ "$(adb shell "if [ -e '$REMOTE_ROOT' ]; then printf PRESENT; else printf ABSENT; fi" | tr -d '\r')" != "ABSENT" ]]; then
    echo "cleanup: exact stage root removal failed" >&2
    exit 16
fi
echo "cleanup: removed only /tmp/wave-npu-poc14b"
