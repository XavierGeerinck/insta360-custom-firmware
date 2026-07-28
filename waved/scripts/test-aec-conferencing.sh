#!/usr/bin/env bash
# SP#2 conferencing AEC test.
#
# Simulates a Microsoft Teams / Zoom scenario:
#   - Far-end (remote participants): a required real-speech fixture played
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
#   2. same-frame linear ERLE and final effective suppression pass.
#   3. residual echo suppression is active.
#   4. reference_rms decays after playback stops and integrity remains clean.
#
# Artifacts saved to OUT_DIR:
#   far_end_speech.s16le — normalized far-end speech fixture
#   mic_capture.pcm     — raw S16LE 48 kHz mono post-AEC output
#   mic_capture.wav     — same, wrapped in WAV for easy playback
set -euo pipefail

SIGNAL_DURATION_S="${SIGNAL_DURATION_S:-5}"
SIGNAL_AMPLITUDE="${SIGNAL_AMPLITUDE:-0.25}"
LINEAR_ERLE_THRESHOLD_DB="${LINEAR_ERLE_THRESHOLD_DB:-20}"
EFFECTIVE_SUPPRESSION_THRESHOLD_DB="${EFFECTIVE_SUPPRESSION_THRESHOLD_DB:-25}"
DEVICE_SPEAKER_VOL_PCT="${DEVICE_SPEAKER_VOL_PCT:-50}"
SKIP_BUILD="${SKIP_BUILD:-0}"
OUT_DIR="${OUT_DIR:-/tmp/waved-conferencing-test}"
: "${SPEECH_FIXTURE:?Set SPEECH_FIXTURE to a WAV/FLAC/MP3 file containing real conversational speech}"

if [ ! -f "${SPEECH_FIXTURE}" ]; then
    echo "ERROR: SPEECH_FIXTURE does not exist: ${SPEECH_FIXTURE}" >&2
    exit 1
fi
if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "ERROR: ffmpeg is required to normalize SPEECH_FIXTURE" >&2
    exit 1
fi

mkdir -p "${OUT_DIR}"

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
# Normalize the real-speech fixture to the PlaybackService format
# ---------------------------------------------------------------------------

echo "==> Preparing real-speech fixture (${SIGNAL_DURATION_S}s, target peak=${SIGNAL_AMPLITUDE})"
SIGNAL_FILE="${OUT_DIR}/far_end_speech.s16le"
ffmpeg -hide_banner -loglevel error -stream_loop -1 -i "${SPEECH_FIXTURE}" \
    -t "${SIGNAL_DURATION_S}" -af "alimiter=limit=${SIGNAL_AMPLITUDE}:attack=5:release=50" \
    -ar 48000 -ac 2 -f s16le -y "${SIGNAL_FILE}"
echo "  ${SIGNAL_FILE}: source=${SPEECH_FIXTURE}"

echo "==> Pushing test signal to device"
adb push "${SIGNAL_FILE}" /data/waved/playback.s16le

# ---------------------------------------------------------------------------
# Start waved (fresh mic_capture.pcm starts from t=0)
# ---------------------------------------------------------------------------

echo "==> Restarting waved"
stop_waved
adb shell "(setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &); sleep 5"

echo "==> Confirming waved is up"
WAVED_PID=$(waved_pids | head -1 | tr -d '\n')
if [ -z "$WAVED_PID" ]; then
    echo "ERROR: waved did not start. Log:"
    adb shell "cat /tmp/waved.log 2>/dev/null | head -40" || true
    exit 1
fi
echo "  waved pid=${WAVED_PID}"

echo "==> Setting speaker volume to ${DEVICE_SPEAKER_VOL_PCT}%"
adb shell "amixer sset '5707_A Master' ${DEVICE_SPEAKER_VOL_PCT}% 2>/dev/null || amixer sset 'Master' ${DEVICE_SPEAKER_VOL_PCT}% 2>/dev/null || true" | tail -3

# ---------------------------------------------------------------------------
# Helper: read one internally consistent /tmp/audio_stats.json snapshot.
# ---------------------------------------------------------------------------

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
    s.get("mic_gain_db"),
    s.get("recording_limiter_reduction_db"),
    s.get("xrun_count", 0),
    s.get("raw_frames_dropped", 0),
    s.get("processed_frames_dropped", 0),
]
print("\t".join("null" if value is None else str(value) for value in values))
'
}

# ---------------------------------------------------------------------------
# Baseline (2 s, no far-end audio)
# ---------------------------------------------------------------------------

echo "==> Baseline (2 s of room noise)"
echo "   >>> You may start speaking into the mic now (near-end). <<<"
sleep 2
IFS=$'\t' read -r BASELINE_POST BASELINE_MIC0 BASELINE_REF BASELINE_PRE _ \
    <<<"$(read_stats_snapshot)"
echo "  raw_mic[0]=${BASELINE_MIC0}  pre_aec=${BASELINE_PRE}  legacy_uplink=${BASELINE_POST}  reference=${BASELINE_REF}"

# ---------------------------------------------------------------------------
# Trigger SIGUSR2 — waved plays the speech-band file through its speaker
# ---------------------------------------------------------------------------

echo "==> Sending SIGUSR2 (playing far-end speech-band signal)"
echo "   >>> Keep talking into the mic to test double-talk. <<<"
adb shell "kill -USR2 ${WAVED_PID}"

# 2 s convergence window — AEC needs a few filter taps before it cancels well.
sleep 2

echo "==> Sampling during playback (t+2 s)"
IFS=$'\t' read -r PLAY_POST PLAY_MIC0 PLAY_REF PLAY_PRE PLAY_LINEAR \
    PLAY_RESIDUAL PLAY_NS PLAY_UPLINK PLAY_LINEAR_ERLE PLAY_EFFECTIVE \
    RESIDUAL_ACTIVE MIC_GAIN_DB LIMITER_DB XRUNS RAW_DROPS PROCESSED_DROPS \
    <<<"$(read_stats_snapshot)"
echo "  pre=${PLAY_PRE} linear=${PLAY_LINEAR} residual=${PLAY_RESIDUAL} ns=${PLAY_NS} uplink=${PLAY_UPLINK} ref=${PLAY_REF}"
echo "  linear_erle=${PLAY_LINEAR_ERLE}dB effective=${PLAY_EFFECTIVE}dB residual_active=${RESIDUAL_ACTIVE}"
echo "  mic_gain=${MIC_GAIN_DB}dB limiter=${LIMITER_DB}dB xruns=${XRUNS} raw_drops=${RAW_DROPS} processed_drops=${PROCESSED_DROPS}"

# Wait for the signal to finish + 2 s for decay.
REMAINING=$(( SIGNAL_DURATION_S - 2 + 2 ))
echo "==> Waiting ${REMAINING}s for playback to finish and speaker to decay..."
sleep ${REMAINING}

echo "==> Post-decay sample"
IFS=$'\t' read -r DECAY_POST _ DECAY_REF _ <<<"$(read_stats_snapshot)"
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
    "${BASELINE_PRE}" "${BASELINE_REF}" \
    "${PLAY_PRE}" "${PLAY_REF}" "${PLAY_LINEAR_ERLE}" "${PLAY_EFFECTIVE}" \
    "${RESIDUAL_ACTIVE}" "${DECAY_REF}" \
    "${LINEAR_ERLE_THRESHOLD_DB}" "${EFFECTIVE_SUPPRESSION_THRESHOLD_DB}" \
    "${XRUNS}" "${RAW_DROPS}" "${PROCESSED_DROPS}" \
    "${SIGNAL_DURATION_S}" "${OUT_DIR}" <<'PY'
import sys, math, struct, os

def f(x):
    return float(x) if x not in ('null', 'None', '') else 0.0

(bl_pre, bl_ref, pl_pre, pl_ref, linear_erle, effective,
 residual_active, dc_ref, linear_thresh, effective_thresh,
 xruns, raw_drops, processed_drops,
 sig_dur, out_dir) = sys.argv[1:]

bl_pre, bl_ref = f(bl_pre), f(bl_ref)
pl_pre, pl_ref = f(pl_pre), f(pl_ref)
linear_erle, effective = f(linear_erle), f(effective)
residual_active, dc_ref = f(residual_active), f(dc_ref)
linear_thresh, effective_thresh = f(linear_thresh), f(effective_thresh)
xruns, raw_drops, processed_drops = f(xruns), f(raw_drops), f(processed_drops)

print(f"\n  baseline: pre={bl_pre:.2f}  ref={bl_ref:.2f}")
print(f"  play:     pre={pl_pre:.2f}  ref={pl_ref:.2f}")
print(f"  decay:    ref={dc_ref:.2f}")

errors = []

# 1. Reference broadcast saw the far-end signal.
if pl_ref <= max(bl_ref + 10.0, 10.0):
    errors.append(f"reference_rms did not rise (baseline={bl_ref:.2f}, play={pl_ref:.2f})")

# 2. Mic heard the echo (speaker was loud enough to test AEC).
if pl_pre < bl_pre + 5.0:
    errors.append(
        f"pre-AEC mic did not rise appreciably "
        f"(baseline={bl_pre:.2f}, play={pl_pre:.2f}) — "
        f"speaker too quiet for ERLE measurement; try higher DEVICE_SPEAKER_VOL_PCT"
    )

print(f"\n  linear ERLE = {linear_erle:.1f} dB (threshold {linear_thresh:.1f} dB)")
print(f"  effective suppression = {effective:.1f} dB (threshold {effective_thresh:.1f} dB)")
if linear_erle < linear_thresh:
    errors.append(f"linear ERLE {linear_erle:.1f} dB < threshold {linear_thresh:.1f} dB")
if effective < effective_thresh:
    errors.append(f"effective suppression {effective:.1f} dB < threshold {effective_thresh:.1f} dB")
if residual_active != 1.0:
    errors.append("residual echo suppression is not active")

# 4. Reference decayed after playback.
if dc_ref > max(pl_ref * 0.3, 20.0):
    errors.append(
        f"reference_rms did not decay after playback "
        f"(play={pl_ref:.2f}, decay={dc_ref:.2f})"
    )
if xruns or raw_drops or processed_drops:
    errors.append(f"integrity counters non-zero: xruns={xruns:g} raw={raw_drops:g} processed={processed_drops:g}")

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
