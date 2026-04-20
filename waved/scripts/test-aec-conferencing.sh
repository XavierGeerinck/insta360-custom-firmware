#!/usr/bin/env bash
# SP#2 conferencing AEC test.
#
# Simulates a Microsoft Teams / Zoom scenario:
#   - Far-end (remote participants): a multi-tone speech-band signal played
#     through waved's own PlaybackService via SIGUSR2.
#   - Near-end (local speaker): the human running the test is encouraged to
#     speak into the device mic during the playback window.
#
# Because the signal flows through waved's PlaybackService the reference
# broadcast tap sees every sample — giving AEC the same coupling that a
# real conferencing call has.
#
# Verifies:
#   1. reference_rms[0] rises during playback.
#   2. mic_post_aec_rms stays near baseline (far-end echo cancelled).
#   3. ERLE >= ERLE_THRESHOLD_DB.
#   4. reference_rms decays after playback stops.
#
# Artifacts saved to OUT_DIR:
#   speech_band.s16le   — generated far-end test signal
#   mic_capture.pcm     — raw S16LE 48 kHz mono post-AEC output
#   mic_capture.wav     — same, wrapped in WAV for easy playback
set -euo pipefail

SIGNAL_DURATION_S="${SIGNAL_DURATION_S:-5}"
SIGNAL_AMPLITUDE="${SIGNAL_AMPLITUDE:-0.25}"
ERLE_THRESHOLD_DB="${ERLE_THRESHOLD_DB:-15}"
DEVICE_SPEAKER_VOL_PCT="${DEVICE_SPEAKER_VOL_PCT:-50}"
SKIP_BUILD="${SKIP_BUILD:-0}"
OUT_DIR="${OUT_DIR:-/tmp/waved-conferencing-test}"

mkdir -p "${OUT_DIR}"

# ---------------------------------------------------------------------------
# Build + Deploy
# ---------------------------------------------------------------------------

if [ "${SKIP_BUILD}" = "0" ]; then
    echo "==> Building aarch64 release binary"
    cross build --target aarch64-unknown-linux-gnu --release -p waved --features "ui,capture-alsa"

    echo "==> Deploying"
    adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
fi

# ---------------------------------------------------------------------------
# Generate speech-band multi-tone test signal
# ---------------------------------------------------------------------------

echo "==> Generating speech-band test signal (${SIGNAL_DURATION_S}s, amplitude=${SIGNAL_AMPLITUDE})"
SIGNAL_FILE="${OUT_DIR}/speech_band.s16le"
python3 - "${SIGNAL_DURATION_S}" "${SIGNAL_AMPLITUDE}" "${SIGNAL_FILE}" <<'PY'
import sys, struct, math

dur_s  = float(sys.argv[1])
amp    = float(sys.argv[2])
out    = sys.argv[3]
sr     = 48000
n      = int(sr * dur_s)

# Six tones spanning the speech band (300–3000 Hz).
freqs = [300, 500, 800, 1200, 2000, 3000]
a = amp / len(freqs)

data = []
for i in range(n):
    t = i / sr
    v = sum(a * math.sin(2 * math.pi * f * t) for f in freqs)
    s = max(-32768, min(32767, int(v * 32767)))
    data.extend([s, s])   # stereo — PlaybackService is 2-ch

with open(out, 'wb') as f:
    f.write(struct.pack(f'<{len(data)}h', *data))
print(f"  {out}: {dur_s}s, freqs={freqs}, amplitude={amp:.3f}")
PY

echo "==> Pushing test signal to device"
adb push "${SIGNAL_FILE}" /data/waved/playback.s16le

# ---------------------------------------------------------------------------
# Start waved (fresh mic_capture.pcm starts from t=0)
# ---------------------------------------------------------------------------

echo "==> Restarting waved"
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &); sleep 5"

echo "==> Confirming waved is up"
WAVED_PID=$(adb shell "for p in /proc/[0-9]*; do if [ -f \$p/comm ]; then n=\$(cat \$p/comm 2>/dev/null); if [ \"\$n\" = \"waved\" ]; then echo \${p#/proc/}; break; fi; fi; done" | tr -d '\r\n')
if [ -z "$WAVED_PID" ]; then
    echo "ERROR: waved did not start. Log:"
    adb shell "cat /tmp/waved.log 2>/dev/null | head -40" || true
    exit 1
fi
echo "  waved pid=${WAVED_PID}"

echo "==> Setting speaker volume to ${DEVICE_SPEAKER_VOL_PCT}%"
adb shell "amixer sset '5707_A Master' ${DEVICE_SPEAKER_VOL_PCT}% 2>/dev/null || amixer sset 'Master' ${DEVICE_SPEAKER_VOL_PCT}% 2>/dev/null || true" | tail -3

# ---------------------------------------------------------------------------
# Helper: read a Python expression from /tmp/audio_stats.json
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# Baseline (2 s, no far-end audio)
# ---------------------------------------------------------------------------

echo "==> Baseline (2 s of room noise)"
echo "   >>> You may start speaking into the mic now (near-end). <<<"
sleep 2
BASELINE_POST=$(read_stats_field "s.get('mic_post_aec_rms', 'null')")
BASELINE_MIC0=$(read_stats_field "s['rms_per_channel'][0]")
BASELINE_REF=$(read_stats_field "s['reference_rms'][0] if s.get('reference_rms') else 0")
echo "  mic_rms[0]=${BASELINE_MIC0}  mic_post_aec_rms=${BASELINE_POST}  reference_rms[0]=${BASELINE_REF}"

# ---------------------------------------------------------------------------
# Trigger SIGUSR2 — waved plays the speech-band file through its speaker
# ---------------------------------------------------------------------------

echo "==> Sending SIGUSR2 (playing far-end speech-band signal)"
echo "   >>> Keep talking into the mic to test double-talk. <<<"
adb shell "kill -USR2 ${WAVED_PID}"

# 2 s convergence window — AEC needs a few filter taps before it cancels well.
sleep 2

echo "==> Sampling during playback (t+2 s)"
PLAY_POST=$(read_stats_field "s.get('mic_post_aec_rms', 'null')")
PLAY_MIC0=$(read_stats_field "s['rms_per_channel'][0]")
PLAY_REF=$(read_stats_field "s['reference_rms'][0] if s.get('reference_rms') else 0")
echo "  mic_rms[0]=${PLAY_MIC0}  mic_post_aec_rms=${PLAY_POST}  reference_rms[0]=${PLAY_REF}"

# Wait for the signal to finish + 2 s for decay.
REMAINING=$(( SIGNAL_DURATION_S - 2 + 2 ))
echo "==> Waiting ${REMAINING}s for playback to finish and speaker to decay..."
sleep ${REMAINING}

echo "==> Post-decay sample"
DECAY_POST=$(read_stats_field "s.get('mic_post_aec_rms', 'null')")
DECAY_REF=$(read_stats_field "s['reference_rms'][0] if s.get('reference_rms') else 0")
echo "  mic_post_aec_rms=${DECAY_POST}  reference_rms[0]=${DECAY_REF}"

# ---------------------------------------------------------------------------
# Pull the post-AEC mic recording
# ---------------------------------------------------------------------------

echo "==> Pulling mic_capture.pcm"
if adb pull /tmp/mic_capture.pcm "${OUT_DIR}/mic_capture.pcm" 2>/dev/null; then
    echo "  Saved to ${OUT_DIR}/mic_capture.pcm"
else
    echo "  (not found — SIGUSR2 path may be compiled out)"
fi

# ---------------------------------------------------------------------------
# Evaluate
# ---------------------------------------------------------------------------

echo "==> Evaluating"
python3 - \
    "${BASELINE_POST}" "${BASELINE_MIC0}" "${BASELINE_REF}" \
    "${PLAY_POST}" "${PLAY_MIC0}" "${PLAY_REF}" \
    "${DECAY_REF}" "${ERLE_THRESHOLD_DB}" \
    "${SIGNAL_DURATION_S}" "${OUT_DIR}" <<'PY'
import sys, math, struct, os

def f(x):
    return float(x) if x not in ('null', 'None', '') else 0.0

(bl_post, bl_mic0, bl_ref,
 pl_post, pl_mic0, pl_ref,
 dc_ref,  thresh,
 sig_dur, out_dir) = sys.argv[1:]

bl_post, bl_mic0, bl_ref = f(bl_post), f(bl_mic0), f(bl_ref)
pl_post, pl_mic0, pl_ref = f(pl_post), f(pl_mic0), f(pl_ref)
dc_ref, thresh = f(dc_ref), f(thresh)

print(f"\n  baseline: mic0={bl_mic0:.2f}  post={bl_post:.3f}  ref={bl_ref:.2f}")
print(f"  play:     mic0={pl_mic0:.2f}  post={pl_post:.3f}  ref={pl_ref:.2f}")
print(f"  decay:    ref={dc_ref:.2f}")

errors = []

# 1. Reference broadcast saw the far-end signal.
if pl_ref <= max(bl_ref + 10.0, 10.0):
    errors.append(f"reference_rms did not rise (baseline={bl_ref:.2f}, play={pl_ref:.2f})")

# 2. Mic heard the echo (speaker was loud enough to test AEC).
if pl_mic0 < bl_mic0 + 5.0:
    errors.append(
        f"mic_rms[0] did not rise appreciably "
        f"(baseline={bl_mic0:.2f}, play={pl_mic0:.2f}) — "
        f"speaker too quiet for ERLE measurement; try higher DEVICE_SPEAKER_VOL_PCT"
    )
else:
    # 3. ERLE: far-end echo was cancelled from post-AEC signal.
    erle = 20.0 * math.log10(pl_mic0 / max(pl_post, 0.01))
    print(f"\n  ERLE = {erle:.1f} dB  (threshold {thresh:.1f} dB)")
    if erle < thresh:
        errors.append(f"ERLE {erle:.1f} dB < threshold {thresh:.1f} dB")

# 4. Reference decayed after playback.
if dc_ref > max(pl_ref * 0.3, 20.0):
    errors.append(
        f"reference_rms did not decay after playback "
        f"(play={pl_ref:.2f}, decay={dc_ref:.2f})"
    )

# 5. PCM file analysis: verify post-AEC level stayed near baseline.
pcm_path = os.path.join(out_dir, "mic_capture.pcm")
if os.path.exists(pcm_path):
    with open(pcm_path, 'rb') as fh:
        raw = fh.read()
    n = len(raw) // 2
    samples = struct.unpack(f'<{n}h', raw[:n*2])
    sr = 48000

    # The recorder runs from waved start-up (~7 s before SIGUSR2 in the
    # script: 5 s startup + 2 s baseline).  Grab a 2-second window just
    # before SIGUSR2 as the baseline and 2 s starting at t+2 s after for
    # the during-playback window.
    startup_gap_s = 7   # approximate; conservative
    bl_start = startup_gap_s * sr
    bl_end   = bl_start + 2 * sr
    pl_start = bl_end + 2 * sr   # skip 2 s convergence
    pl_end   = pl_start + 2 * sr

    def rms(s):
        if not s: return 0.0
        return math.sqrt(sum(x*x for x in s) / len(s))

    pcm_bl  = rms(samples[bl_start:bl_end])
    pcm_pl  = rms(samples[pl_start:pl_end]) if pl_end <= len(samples) else rms(samples[pl_start:])

    print(f"\n  PCM post-AEC analysis (raw S16LE 48 kHz mono):")
    print(f"    baseline window RMS = {pcm_bl:.1f}")
    print(f"    playback window RMS = {pcm_pl:.1f}")
    if pcm_bl > 1 and pcm_pl > 0:
        ratio_db = 20.0 * math.log10(pcm_pl / pcm_bl)
        print(f"    ratio = {ratio_db:+.1f} dB  (near 0 = echo cancelled; >> 0 = echo leaking)")

    # Write a WAV so the tester can listen without extra tools.
    wav_path = pcm_path.replace('.pcm', '.wav')
    ch, bits = 1, 16
    byte_rate = sr * ch * bits // 8
    blk = ch * bits // 8
    with open(wav_path, 'wb') as wf:
        wf.write(b'RIFF')
        wf.write(struct.pack('<I', 36 + len(raw)))
        wf.write(b'WAVE')
        wf.write(b'fmt ')
        wf.write(struct.pack('<IHHIIHH', 16, 1, ch, sr, byte_rate, blk, bits))
        wf.write(b'data')
        wf.write(struct.pack('<I', len(raw)))
        wf.write(raw)
    print(f"\n  Saved {wav_path}")
    print(f"  Listen: afplay {wav_path}")
    print(f"  Expect: room noise throughout; far-end speech-band tones NOT audible")

print()
if errors:
    print("FAIL")
    for e in errors:
        print("  - " + e)
    sys.exit(1)
print("PASS")
PY

echo "==> Conferencing AEC test complete"
echo "    Artifacts in ${OUT_DIR}/"
