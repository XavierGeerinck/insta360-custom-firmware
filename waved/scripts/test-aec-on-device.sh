#!/usr/bin/env bash
# SP#2 on-device AEC smoke test.
#
# Sends SIGUSR1 to the running waved, which plays a 1 kHz tone through its
# own PlaybackService. That path populates the reference tap correctly,
# giving AEC a signal to cancel.
#
# Verifies:
#   1. reference_rms[0] rises while the tone plays (PlaybackService is
#      broadcasting what it writes to the speaker).
#   2. same-frame pre-AEC mic RMS rises (the mic hears the speaker).
#   3. linear ERLE and final effective suppression meet their thresholds.
#   4. residual suppression is active with the configured tuning.
#   5. reference RMS decays after the tone stops.
#
# Legacy mic_post_aec_rms is deliberately not used: it is the final uplink
# after user gain and limiting and cannot be compared with a raw mic channel.
set -euo pipefail

DEVICE_SPEAKER_VOL_PCT="${DEVICE_SPEAKER_VOL_PCT:-50}"
TONE_DURATION_MS="${TONE_DURATION_MS:-4000}"
TONE_FREQ_HZ="${TONE_FREQ_HZ:-1000}"
TONE_AMPLITUDE="${TONE_AMPLITUDE:-0.3}"
LINEAR_ERLE_THRESHOLD_DB="${LINEAR_ERLE_THRESHOLD_DB:-20}"
EFFECTIVE_SUPPRESSION_THRESHOLD_DB="${EFFECTIVE_SUPPRESSION_THRESHOLD_DB:-25}"
SKIP_BUILD="${SKIP_BUILD:-0}"

waved_pids() {
    adb shell 'for p in /proc/[0-9]*; do
        if [ -f "$p/comm" ] && [ "$(cat "$p/comm" 2>/dev/null)" = waved ]; then
            echo "${p#/proc/}"
        fi
    done' | tr -d '\r'
}

stop_waved() {
    adb shell "killall -TERM waved 2>/dev/null || true"
    for _ in $(seq 1 10); do
        if [ -z "$(waved_pids)" ]; then
            return
        fi
        sleep 1
    done
    echo "WARN: waved did not stop within 10s; forcing shutdown" >&2
    adb shell "killall -KILL waved 2>/dev/null || true"
    sleep 1
}

if [ "${SKIP_BUILD}" = "0" ]; then
    echo "==> Building aarch64 release binary"
    cross build --target aarch64-unknown-linux-gnu --release -p waved --features "ui,capture-alsa"

    echo "==> Deploying"
    adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved

    echo "==> Restarting waved"
    # Launch + sleep inside ONE adb shell so setsid has time to detach before
    # the remote shell exits. Trying to split into multiple adb shell calls
    # races with the remote shell teardown and often kills the child.
    stop_waved
    adb shell "(setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &); sleep 5"
fi

echo "==> Confirming waved is up"
WAVED_PID=$(waved_pids | head -1 | tr -d '\n')
if [ -z "$WAVED_PID" ]; then
    echo "ERROR: waved did not start. Log:"
    adb shell "cat /tmp/waved.log 2>/dev/null | head -30" || true
    exit 1
fi
echo "  waved pid=${WAVED_PID}"

echo "==> Setting speaker to ${DEVICE_SPEAKER_VOL_PCT}%"
adb shell "amixer sset '5707_A Master' ${DEVICE_SPEAKER_VOL_PCT}% 2>/dev/null || amixer sset 'Master' ${DEVICE_SPEAKER_VOL_PCT}% 2>/dev/null || true" | tail -3

read_stats_field() {
    local expr="$1"
    adb shell "cat /tmp/audio_stats.json 2>/dev/null" | python3 -c "
import json, sys
try:
    s = json.load(sys.stdin)
except Exception:
    print('null'); sys.exit(0)
try:
    print(${expr})
except Exception:
    print('null')
"
}

read_stats_snapshot() {
    adb shell "cat /tmp/audio_stats.json 2>/dev/null" | python3 -c '
import json, sys
s = json.load(sys.stdin)
a = s.get("aec") or {}
values = [
    s.get("mic_post_aec_rms"),
    (s.get("rms_per_channel") or [0])[0],
    (s.get("reference_rms") or [0])[0],
    a.get("mic_pre_aec_rms"),
    a.get("mic_post_linear_aec_rms"),
    a.get("mic_post_residual_echo_rms"),
    a.get("mic_post_noise_suppression_rms"),
    a.get("mic_uplink_rms"),
    a.get("linear_erle_db"),
    a.get("effective_echo_suppression_db"),
    1 if a.get("residual_echo_suppression_active") else 0,
    a.get("residual_echo_suppress_db"),
    a.get("residual_echo_suppress_active_db"),
    a.get("noise_suppression_backend", "missing"),
    s.get("mic_gain_db"),
    s.get("recording_limiter_reduction_db"),
    s.get("xrun_count", 0),
    s.get("raw_frames_dropped", 0),
    s.get("processed_frames_dropped", 0),
]
print("\t".join("null" if value is None else str(value) for value in values))
'
}

echo "==> Baseline (no tone)"
sleep 1
IFS=$'\t' read -r BASELINE_POST BASELINE_MIC0 BASELINE_REF BASELINE_PRE _ \
    <<<"$(read_stats_snapshot)"
echo "  raw_mic[0]=${BASELINE_MIC0}  pre_aec=${BASELINE_PRE}  legacy_uplink=${BASELINE_POST}  reference=${BASELINE_REF}"

echo "==> Firing SIGUSR1 (tone defaults to 1000 Hz / 3 s / 0.3 amp)"
adb shell "kill -USR1 ${WAVED_PID}"

# Let the tone get past the filter-convergence window.
sleep 2

echo "==> Sampling during tone"
IFS=$'\t' read -r TONE_POST TONE_MIC0 TONE_REF TONE_PRE TONE_LINEAR \
    TONE_RESIDUAL TONE_NS TONE_UPLINK TONE_LINEAR_ERLE TONE_EFFECTIVE \
    RESIDUAL_ACTIVE RESIDUAL_FAR_DB RESIDUAL_ACTIVE_DB NS_BACKEND MIC_GAIN_DB \
    LIMITER_DB XRUNS RAW_DROPS PROCESSED_DROPS <<<"$(read_stats_snapshot)"
echo "  pre=${TONE_PRE} linear=${TONE_LINEAR} residual=${TONE_RESIDUAL} ns=${TONE_NS} uplink=${TONE_UPLINK} ref=${TONE_REF}"
echo "  linear_erle=${TONE_LINEAR_ERLE}dB effective=${TONE_EFFECTIVE}dB residual_active=${RESIDUAL_ACTIVE} tuning=${RESIDUAL_FAR_DB}/${RESIDUAL_ACTIVE_DB}dB ns=${NS_BACKEND}"
echo "  mic_gain=${MIC_GAIN_DB}dB limiter=${LIMITER_DB}dB xruns=${XRUNS} raw_drops=${RAW_DROPS} processed_drops=${PROCESSED_DROPS}"

# Wait for tone to finish (default 3 s minus the 2 s we already slept, +1 s safety).
sleep 3

echo "==> Decay check"
DECAY_REF=$(read_stats_field "s['reference_rms'][0] if s.get('reference_rms') else 0")
DECAY_POST=$(read_stats_field "s.get('mic_post_aec_rms', 'null')")
echo "  reference_rms[0]=${DECAY_REF}  mic_post_aec_rms=${DECAY_POST}"

echo "==> Evaluating"
python3 - \
    "$BASELINE_PRE" "$BASELINE_REF" \
    "$TONE_PRE" "$TONE_REF" "$TONE_LINEAR_ERLE" "$TONE_EFFECTIVE" \
    "$RESIDUAL_ACTIVE" "$DECAY_REF" \
    "$LINEAR_ERLE_THRESHOLD_DB" "$EFFECTIVE_SUPPRESSION_THRESHOLD_DB" \
    "$XRUNS" "$RAW_DROPS" "$PROCESSED_DROPS" <<'PY'
import sys, math

def f(x):
    return float(x) if x not in ('null', 'None', '') else 0.0

(baseline_pre, baseline_ref, tone_pre, tone_ref, linear_erle, effective,
 residual_active, decay_ref, linear_thresh, effective_thresh,
 xruns, raw_drops, processed_drops) = map(f, sys.argv[1:])

print(f"  baseline: pre={baseline_pre:.2f} ref={baseline_ref:.2f}")
print(f"  tone:     pre={tone_pre:.2f} ref={tone_ref:.2f}")
print(f"  decay:    ref={decay_ref:.2f}")

errors = []
# 1. Reference broadcast saw the tone.
if tone_ref <= max(baseline_ref + 10.0, 10.0):
    errors.append(f"reference_rms did not rise (baseline={baseline_ref:.2f}, tone={tone_ref:.2f})")

# 2. Beamformed mic heard the echo.
if tone_pre < baseline_pre + 5.0:
    errors.append(f"pre-AEC mic did not rise appreciably (baseline={baseline_pre:.2f}, tone={tone_pre:.2f})")

# 3. Same-frame suppression — unaffected by receiver timing or field naming.
print(f"  linear ERLE = {linear_erle:.1f} dB (threshold {linear_thresh:.1f} dB)")
print(f"  effective suppression = {effective:.1f} dB (threshold {effective_thresh:.1f} dB)")
if linear_erle < linear_thresh:
    errors.append(f"linear ERLE {linear_erle:.1f} dB < threshold {linear_thresh:.1f} dB")
if effective < effective_thresh:
    errors.append(f"effective suppression {effective:.1f} dB < threshold {effective_thresh:.1f} dB")
if residual_active != 1.0:
    errors.append("residual echo suppression is not active")

# 4. Reference decays.
if decay_ref > max(tone_ref * 0.3, 20.0):
    errors.append(f"reference_rms did not decay (tone={tone_ref:.2f}, decay={decay_ref:.2f})")
if xruns or raw_drops or processed_drops:
    errors.append(f"integrity counters non-zero: xruns={xruns:g} raw={raw_drops:g} processed={processed_drops:g}")

if errors:
    print("FAIL")
    for e in errors:
        print("  - " + e)
    sys.exit(1)
print("PASS")
PY

echo "==> Smoke test complete"
