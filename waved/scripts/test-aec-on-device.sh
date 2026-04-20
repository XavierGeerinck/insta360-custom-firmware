#!/usr/bin/env bash
# SP#2 on-device AEC smoke test.
#
# Plays a 1 kHz tone through PlaybackService via waved-aec-probe, and verifies:
#   1. reference_rms rises when the tone plays (reference tap is alive).
#   2. mic_post_aec_rms stays near the pre-tone baseline (AEC is cancelling
#      the echo rather than passing it through).
#   3. reference_rms decays back near zero after the tone stops.
#
# Pass criterion: ERLE (= 20*log10(rms_with_tone / rms_post_aec)) >= 20 dB.
set -euo pipefail

DEVICE_SPEAKER_VOL="${DEVICE_SPEAKER_VOL:-50}"
TONE_DURATION="${TONE_DURATION:-4}"
TONE_FREQ="${TONE_FREQ:-1000}"
TONE_AMPLITUDE="${TONE_AMPLITUDE:-0.3}"
ERLE_THRESHOLD_DB="${ERLE_THRESHOLD_DB:-20}"

echo "==> Building aarch64 binaries"
cross build --target aarch64-unknown-linux-gnu --release -p waved -p waved-aec-probe --features "ui,capture-alsa"

echo "==> Deploying"
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb push target/aarch64-unknown-linux-gnu/release/waved-aec-probe /data/waved/

echo "==> Restarting waved"
adb shell "killall waved 2>/dev/null; sleep 2"
adb shell "setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &"
sleep 5

echo "==> Setting speaker to ${DEVICE_SPEAKER_VOL}%"
# Best-effort; the exact control name depends on the hardware variant.
adb shell "amixer sset '5707_A Master' ${DEVICE_SPEAKER_VOL}% 2>/dev/null || amixer sset 'Master' ${DEVICE_SPEAKER_VOL}% 2>/dev/null || true"

read_stats_field() {
    local field="$1"
    adb shell "cat /tmp/audio_stats.json 2>/dev/null" | python3 -c "
import json, sys
try:
    s = json.load(sys.stdin)
except Exception:
    sys.exit(2)
v = s.get('${field}')
if v is None:
    sys.exit(3)
if isinstance(v, list):
    print(v[0] if v else 0.0)
else:
    print(v)
"
}

echo "==> Baseline (no tone): sampling mic_post_aec_rms"
sleep 1
BASELINE_POST_AEC=$(read_stats_field mic_post_aec_rms || echo "0")
BASELINE_REF=$(read_stats_field reference_rms || echo "0")
echo "  baseline mic_post_aec_rms=${BASELINE_POST_AEC} reference_rms[0]=${BASELINE_REF}"

echo "==> Playing ${TONE_FREQ} Hz tone for ${TONE_DURATION} s (amplitude ${TONE_AMPLITUDE})"
adb shell "/data/waved/waved-aec-probe --freq ${TONE_FREQ} --duration ${TONE_DURATION} --amplitude ${TONE_AMPLITUDE}" &
PROBE_PID=$!
# Let the adaptive filter converge before sampling.
sleep $(( TONE_DURATION / 2 > 1 ? TONE_DURATION / 2 : 1 ))

echo "==> Sampling during tone"
TONE_POST_AEC=$(read_stats_field mic_post_aec_rms || echo "0")
TONE_MIC0=$(read_stats_field rms_per_channel || echo "0")
TONE_REF=$(read_stats_field reference_rms || echo "0")
echo "  tone mic_post_aec_rms=${TONE_POST_AEC} mic_rms[0]=${TONE_MIC0} reference_rms[0]=${TONE_REF}"

wait "$PROBE_PID" || true
sleep 1

echo "==> Decay check"
DECAY_REF=$(read_stats_field reference_rms || echo "0")
echo "  post-tone reference_rms[0]=${DECAY_REF}"

python3 - "$BASELINE_POST_AEC" "$TONE_POST_AEC" "$TONE_MIC0" "$TONE_REF" "$DECAY_REF" "$ERLE_THRESHOLD_DB" <<'PY'
import sys, math
baseline_post, tone_post, tone_mic0, tone_ref, decay_ref, thresh = map(float, sys.argv[1:])
print(f"  values: baseline_post={baseline_post:.1f} tone_post={tone_post:.1f} "
      f"tone_mic0={tone_mic0:.1f} tone_ref={tone_ref:.1f} decay_ref={decay_ref:.1f}")

errors = []
# 1. Reference tap saw the tone.
if tone_ref <= max(decay_ref, 10.0):
    errors.append(f"reference_rms did not rise during tone (tone={tone_ref:.1f}, decay={decay_ref:.1f})")

# 2. ERLE: how much did AEC suppress relative to the raw mic seeing the tone?
# Use mic channel 0 as the "pre-AEC with echo" reference; if it's ~0 the tone
# never reached the mic (speaker off?) and we can't measure ERLE.
if tone_mic0 < 50.0:
    errors.append(f"mic_rms[0] too low ({tone_mic0:.1f}) — is the speaker playing audibly?")
else:
    erle = 20.0 * math.log10(tone_mic0 / max(tone_post, 1.0))
    print(f"  ERLE = {erle:.1f} dB (threshold {thresh:.1f} dB)")
    if erle < thresh:
        errors.append(f"ERLE {erle:.1f} dB below threshold {thresh:.1f} dB")

# 3. Reference decays after tone.
if decay_ref > max(tone_ref * 0.2, 50.0):
    errors.append(f"reference_rms did not decay (tone={tone_ref:.1f}, decay={decay_ref:.1f})")

if errors:
    print("FAIL")
    for e in errors:
        print("  - " + e)
    sys.exit(1)
print("PASS")
PY

echo "==> Smoke test passed"
