#!/bin/sh
set -eu

if [ "$#" -ne 0 ]; then
    echo "stock-ctc-trace takes no arguments" >&2
    exit 64
fi

verify_sha256() {
    expected_digest="$1"
    artifact_path="$2"
    observed_line="$(sha256sum "$artifact_path")" || exit 70
    observed_digest="${observed_line%% *}"
    if [ "$observed_digest" != "$expected_digest" ]; then
        echo "stock-ctc-trace hash mismatch: $artifact_path" >&2
        exit 70
    fi
}

verify_sha256 \
    "2046787af2bec0edd5c3483b59bcbd870e5a0fb68cc3c618e9e464e61ee62aa2" \
    "/usr/bin/strace"
verify_sha256 \
    "13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6" \
    "/usr/lib/libGAL.so"
verify_sha256 \
    "fe8da5702032c397023acebd1a4bdf40ce086f6e86277a52c4f059f409552db0" \
    "/data/waved/waved-npu-probe.poc"
verify_sha256 \
    "dccaedd13812c161949905efecf7aa19fe74712b75de8d80211924e286e68601" \
    "/tmp/wave-npu-lab/libwave-gal-trace.so"

export LD_LIBRARY_PATH="/data/waved/lib:/usr/lib"
export VIVANTE_SDK_DIR="/usr"
export LD_PRELOAD="/tmp/wave-npu-lab/libwave-gal-trace.so"

STRACE_OUTPUT="/tmp/wave-npu-lab/strace.raw"
GAL_OUTPUT="/tmp/wave-npu-lab/gal.raw"

: >"$STRACE_OUTPUT"
: >"$GAL_OUTPUT"
chmod 0600 "$STRACE_OUTPUT" "$GAL_OUTPUT"
trap 'rm -f "$STRACE_OUTPUT" "$GAL_OUTPUT"' 0 1 2 15

set +e
/usr/bin/strace \
    -f \
    -qq \
    -ttt \
    -T \
    -yy \
    -x \
    -e raw=ioctl \
    -e trace=execve,openat,close,ioctl,mmap,munmap,mprotect,brk,futex,poll,ppoll,select,pselect6,clone,write,exit,exit_group \
    -o "$STRACE_OUTPUT" \
    -- \
    /data/waved/waved-npu-probe.poc \
    --run-stock-ctc \
    2>"$GAL_OUTPUT"
probe_status="$?"
set -e

printf '\nWAVE_NPU_TRACE_GAL_BEGIN\n'
cat "$GAL_OUTPUT"
printf 'WAVE_NPU_TRACE_GAL_END\n'
printf 'WAVE_NPU_TRACE_STRACE_BEGIN\n'
cat "$STRACE_OUTPUT"
printf 'WAVE_NPU_TRACE_STRACE_END\n'

rm -f "$STRACE_OUTPUT" "$GAL_OUTPUT"
trap - 0 1 2 15
exit "$probe_status"
