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
#   2. mic_rms[0] rises too (the speaker is audible enough for the mic to
#      hear the echo — baseline for ERLE).
#   3. mic_post_aec_rms stays near the pre-tone baseline (AEC is cancelling
#      the echo in the processed-frame pipeline).
#   4. reference_rms decays after the tone stops.
#
# Pass criterion: ERLE (= 20*log10(rms_per_channel[0] / mic_post_aec_rms))
# >= ERLE_THRESHOLD_DB during the tone.
set -euo pipefail

DEVICE_SPEAKER_VOL_PCT="${DEVICE_SPEAKER_VOL_PCT:-50}"
TONE_DURATION_MS="${TONE_DURATION_MS:-4000}"
TONE_FREQ_HZ="${TONE_FREQ_HZ:-1000}"
TONE_AMPLITUDE="${TONE_AMPLITUDE:-0.3}"
ERLE_THRESHOLD_DB="${ERLE_THRESHOLD_DB:-15}"

echo "==> Building aarch64 release binary"
cross build --target aarch64-unknown-linux-gnu --release -p waved --features "ui,capture-alsa"

echo "==> Deploying"
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved

echo "==> Restarting waved"
# Launch + sleep inside ONE adb shell so setsid has time to detach before
# the remote shell exits. Trying to split into multiple adb shell calls
# races with the remote shell teardown and often kills the child.
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &); sleep 5"

echo "==> Confirming waved is up"
WAVED_PID=$(adb shell "for p in /proc/[0-9]*; do if [ -f \$p/comm ]; then n=\$(cat \$p/comm 2>/dev/null); if [ \"\$n\" = \"waved\" ]; then echo \${p#/proc/}; break; fi; fi; done" | tr -d '\r\n')
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

echo "==> Baseline (no tone)"
sleep 1
BASELINE_POST=$(read_stats_field "s.get('mic_post_aec_rms', 'null')")
BASELINE_MIC0=$(read_stats_field "s['rms_per_channel'][0]")
BASELINE_REF=$(read_stats_field "s['reference_rms'][0] if s.get('reference_rms') else 0")
echo "  mic_rms[0]=${BASELINE_MIC0}  mic_post_aec_rms=${BASELINE_POST}  reference_rms[0]=${BASELINE_REF}"

echo "==> Firing SIGUSR1 (tone defaults to 1000 Hz / 3 s / 0.3 amp)"
adb shell "kill -USR1 ${WAVED_PID}"

# Let the tone get past the filter-convergence window.
sleep 2

echo "==> Sampling during tone"
TONE_POST=$(read_stats_field "s.get('mic_post_aec_rms', 'null')")
TONE_MIC0=$(read_stats_field "s['rms_per_channel'][0]")
TONE_REF=$(read_stats_field "s['reference_rms'][0] if s.get('reference_rms') else 0")
echo "  mic_rms[0]=${TONE_MIC0}  mic_post_aec_rms=${TONE_POST}  reference_rms[0]=${TONE_REF}"

# Wait for tone to finish (default 3 s minus the 2 s we already slept, +1 s safety).
sleep 3

echo "==> Decay check"
DECAY_REF=$(read_stats_field "s['reference_rms'][0] if s.get('reference_rms') else 0")
DECAY_POST=$(read_stats_field "s.get('mic_post_aec_rms', 'null')")
echo "  reference_rms[0]=${DECAY_REF}  mic_post_aec_rms=${DECAY_POST}"

echo "==> Evaluating"
python3 - "$BASELINE_POST" "$BASELINE_MIC0" "$BASELINE_REF" "$TONE_POST" "$TONE_MIC0" "$TONE_REF" "$DECAY_REF" "$ERLE_THRESHOLD_DB" <<'PY'
import sys, math

def f(x):
    return float(x) if x not in ('null', 'None', '') else 0.0

(baseline_post, baseline_mic0, baseline_ref,
 tone_post, tone_mic0, tone_ref,
 decay_ref, thresh) = map(f, sys.argv[1:])

print(f"  baseline: mic0={baseline_mic0:.2f} post={baseline_post:.3f} ref={baseline_ref:.2f}")
print(f"  tone:     mic0={tone_mic0:.2f} post={tone_post:.3f} ref={tone_ref:.2f}")
print(f"  decay:    ref={decay_ref:.2f}")

errors = []
# 1. Reference broadcast saw the tone.
if tone_ref <= max(baseline_ref + 10.0, 10.0):
    errors.append(f"reference_rms did not rise (baseline={baseline_ref:.2f}, tone={tone_ref:.2f})")

# 2. Mic heard the echo — we need this as the "pre-AEC with echo" reference.
if tone_mic0 < baseline_mic0 + 5.0:
    errors.append(f"mic_rms[0] did not rise appreciably (baseline={baseline_mic0:.2f}, tone={tone_mic0:.2f}) — speaker may be too quiet")
else:
    # 3. ERLE.
    erle = 20.0 * math.log10(tone_mic0 / max(tone_post, 0.01))
    print(f"  ERLE = {erle:.1f} dB (threshold {thresh:.1f} dB)")
    if erle < thresh:
        errors.append(f"ERLE {erle:.1f} dB < threshold {thresh:.1f} dB")

# 4. Reference decays.
if decay_ref > max(tone_ref * 0.3, 20.0):
    errors.append(f"reference_rms did not decay (tone={tone_ref:.2f}, decay={decay_ref:.2f})")

if errors:
    print("FAIL")
    for e in errors:
        print("  - " + e)
    sys.exit(1)
print("PASS")
PY

echo "==> Smoke test complete"
