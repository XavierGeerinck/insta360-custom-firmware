#!/usr/bin/env bash
# F2 on-device acceptance: 7 tests covering the 5 buckets from the spec.
# Requires a loudspeaker the operator can reposition; prompts for each.
#
# Usage:  scripts/test-doa-improvements.sh
# Device: must be reachable via adb; waved binary deployed at /data/waved/waved
#         and the new waved-doa-tap binary at /data/waved/waved-doa-tap.

set -euo pipefail

DEVICE="${DEVICE:-PSP_0000000000000000}"
WAVED_BIN="${WAVED_BIN:-/data/waved/waved}"
TAP_BIN="${TAP_BIN:-/data/waved/waved-doa-tap}"

adb_run() { adb -s "$DEVICE" shell "$@"; }
adb_kill_waved() {
    adb_run "killall waved 2>/dev/null; killall S60waved 2>/dev/null; sleep 1" || true
}
adb_start_waved() {
    adb_run "(setsid $WAVED_BIN >/tmp/waved.log 2>&1 </dev/null &); sleep 4"
}

prompt() {
    echo
    echo "==> $1"
    read -r -p "press ENTER when ready: " _
}

read_az() {
    # Reads doa_horizontal from /tmp/audio_stats.json (4s average).
    adb_run "for i in 1 2 3 4 5 6 7 8; do cat /tmp/audio_stats.json 2>/dev/null | \
        grep -o 'doa_horizontal\":[^,]*' | head -1; sleep 0.5; done" \
        | grep -oE '[0-9]+\.[0-9]+' \
        | awk '{s+=$1; n++} END {if (n>0) print s/n}'
}

run_test_270_az() {
    echo "=== Test 1: 270° azimuth fix regression ==="
    prompt "place loudspeaker at az=270° (device's right side), play 1 kHz tone"
    az=$(read_az)
    echo "measured az: ${az}°"
    err=$(echo "$az" | awk '{d=270-$1; if (d<0) d=-d; if (d>180) d=360-d; print d}')
    echo "az error: ${err}°"
    if [ "$(echo "$err < 5.0" | bc)" = "1" ]; then
        echo "PASS: az within ±5°"
    else
        echo "FAIL: az error ${err}° >= 5°"
        return 1
    fi
}

run_test_other_az_regression() {
    echo "=== Test 2: 4-quadrant az regression (no SP#3 regression) ==="
    for target in 0 90 180; do
        prompt "place loudspeaker at az=${target}°, play 1 kHz tone"
        az=$(read_az)
        err=$(echo "$az $target" | awk '{d=$2-$1; if (d<0) d=-d; if (d>180) d=360-d; print d}')
        echo "az=${target}° measured ${az}° → error ${err}°"
        if [ "$(echo "$err < 10.0" | bc)" = "1" ]; then
            echo "  PASS (within ±10° SP#3 spec)"
        else
            echo "  FAIL: ${err}° >= 10°"
            return 1
        fi
    done
}

run_test_srp_backend() {
    echo "=== Test 4: SRP-PHAT backend swap ==="
    adb_kill_waved
    adb_run "echo '{\"version\":4,\"capture\":{\"mic_device\":\"hw:0,2\",\"mic_channels\":8,\"mic_sample_rate\":48000,\"mic_format\":\"S32_LE\"},\"period_frames\":1024,\"buffer_periods\":8,\"doa_backend\":\"srp_phat\"}' > /tmp/waved-audio-config.json"
    # Override config path via env (or copy to /etc/) — depends on existing
    # waved.rs config loading. Easiest: copy to /etc/.
    adb_run "cp /tmp/waved-audio-config.json /etc/waved-audio-config.json"
    adb_start_waved
    # Re-run test 1 with the new backend.
    prompt "(SRP-PHAT active) place loudspeaker at az=270°"
    az=$(read_az)
    err=$(echo "$az" | awk '{d=270-$1; if (d<0) d=-d; if (d>180) d=360-d; print d}')
    echo "SRP-PHAT az=${az}°, err=${err}°"
    if [ "$(echo "$err < 10.0" | bc)" = "1" ]; then
        echo "PASS: SRP-PHAT no worse than GCC-PHAT (±10°)"
    else
        echo "FAIL: SRP-PHAT az error ${err}° >= 10°"
        return 1
    fi
}

run_test_srp_cpu() {
    echo "=== Test 5: SRP-PHAT CPU delta < 5% ==="
    # Sample CPU for 15s under speech load.
    prompt "play speech via loudspeaker for 20s"
    pid=$(adb_run "pidof waved")
    samples=$(adb_run "for i in \$(seq 1 15); do awk '{print \$14+\$15}' /proc/${pid}/stat; sleep 1; done")
    avg=$(echo "$samples" | awk '{a+=$1; n++} END {print a/n}')
    echo "SRP-PHAT 15s CPU jiffies avg: $avg"
    # NOTE: a comparable GCC-PHAT baseline must be captured first; this test
    # is currently informational, not auto-PASS/FAIL — operator records both.
    echo "INFO: compare to GCC-PHAT baseline manually for the < 5% delta gate"
}

run_test_steered_beam_pickup() {
    echo "=== Test 6: Steered beam pickup (subjective) ==="
    adb_run "echo 'audio_mode_set steered' > /tmp/waved-mode" # if your runtime supports it; otherwise SetAudioMode via cmd path
    prompt "play continuous speech from az=0°, then move loudspeaker to az=90° over ~2s"
    echo "LISTEN: does the captured audio follow the source? Y/N"
    read -r answer
    if [ "$answer" = "Y" ] || [ "$answer" = "y" ]; then
        echo "PASS (subjective)"
    else
        echo "FAIL: beam did not follow"
        return 1
    fi
}

run_test_f3_consumer_smoke() {
    echo "=== Test 7: F3 consumer smoke via waved-doa-tap ==="
    adb_kill_waved
    # Run tap directly — produces frames itself by owning the AudioService.
    out=$(adb_run "$TAP_BIN --duration-secs 5 2>&1" | head -20)
    echo "$out"
    n_lines=$(echo "$out" | grep -c '^{' || true)
    if [ "$n_lines" -gt 50 ]; then
        echo "PASS: ${n_lines} DoaFrame JSONs in 5 s (expected ~240 at 21ms cadence)"
    else
        echo "FAIL: only ${n_lines} frames"
        return 1
    fi
}

main() {
    adb_kill_waved
    adb_start_waved
    run_test_270_az
    run_test_other_az_regression
    echo "=== Test 3 (elevation bias measurement) is in test-doa-elevation.sh ==="
    run_test_srp_backend
    run_test_srp_cpu
    run_test_steered_beam_pickup
    run_test_f3_consumer_smoke
    echo
    echo "All F2 acceptance tests complete."
}

main "$@"
