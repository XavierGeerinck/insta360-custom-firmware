#!/usr/bin/env bash
# test-latency-benchmark.sh — DSP wake-from-silence latency benchmark.
#
# Builds `waved-wakeup-probe`, deploys it to the device, stops the waved
# watchdog so the probe owns the DSP/DFN3 model, runs the benchmark, pulls the
# JSON report back to the host, then REMOVES the probe binary and restarts
# waved — leaving the device as it was and reclaiming the ~57 MB the probe
# occupies. Cleanup runs even on failure or Ctrl-C (trap).
#
# The probe injects a deterministic speech burst after N seconds of silence and
# reports per-burst latency + leading-edge loss vs a warm baseline. It needs the
# real Processor (DFN3), so waved must not be running concurrently — hence the
# watchdog stop/restart dance. See docs/AUDIO_HARDWARE.md and
# docs/superpowers/specs/2026-05-23-waved-wakeup-probe-design.md.
#
# Usage:
#   scripts/test-latency-benchmark.sh                 # quick default run
#   scripts/test-latency-benchmark.sh --silence-seconds 60 --cold-runs 10
#   SKIP_BUILD=1 scripts/test-latency-benchmark.sh    # reuse host build artifact
#   KEEP_PROBE=1 scripts/test-latency-benchmark.sh    # leave probe on device
#
# All args are forwarded to the probe. Defaults if none given:
#   --silence-seconds 10 --warmup-bursts 3 --cold-runs 3
# Probe flags: --silence-seconds N  --burst-ms N  --warmup-bursts N
#              --cold-runs N         (--json is managed by this script)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET="aarch64-unknown-linux-gnu"
PROBE_BIN="$REPO_ROOT/target/$TARGET/release/waved-wakeup-probe"
DEVICE_PROBE="/data/waved/waved-wakeup-probe"
DEVICE_JSON="/tmp/wakeup-latency.json"
HOST_JSON="${HOST_JSON:-$REPO_ROOT/wakeup-latency-$(date +%Y%m%d-%H%M%S).json}"

# Probe args: caller's, or a sensible quick default.
PROBE_ARGS=("$@")
if [[ ${#PROBE_ARGS[@]} -eq 0 ]]; then
    PROBE_ARGS=(--silence-seconds 10 --warmup-bursts 3 --cold-runs 3)
fi

command -v adb >/dev/null || { echo "ERROR: adb not found on PATH"; exit 1; }
[[ "$(adb get-state 2>/dev/null)" == "device" ]] || { echo "ERROR: no adb device (adb get-state != 'device')"; exit 1; }

restart_waved() {
    # Re-launch the init.d watchdog (respawns waved). Two subtleties, both
    # learned the hard way:
    #   * NO trailing '&'. `S60waved start` already self-backgrounds its
    #     `while true` respawn loop; we just need `setsid` to run it to
    #     completion in a NEW session. A trailing '&' instead backgrounds
    #     setsid inside adb's own process group, which adb kills on shell
    #     exit before the loop detaches — leaving the device with no waved.
    #   * The in-shell `sleep 2` keeps the adb session alive long enough for
    #     the new session to take hold before teardown.
    adb shell "setsid /bin/sh /etc/init.d/S60waved start >/dev/null 2>&1 </dev/null; sleep 2" >/dev/null 2>&1 || true
}

cleanup() {
    local rc=$?
    echo
    echo "==> cleanup"
    if [[ "${KEEP_PROBE:-0}" == "1" ]]; then
        echo "  keeping $DEVICE_PROBE (KEEP_PROBE=1)"
    elif adb shell "rm -f $DEVICE_PROBE" >/dev/null 2>&1; then
        echo "  removed $DEVICE_PROBE (reclaimed ~57 MB)"
    fi
    adb shell "rm -f $DEVICE_JSON" >/dev/null 2>&1 || true
    echo "  restarting waved (watchdog)..."
    restart_waved
    local p=""
    for _ in $(seq 1 12); do
        p="$(adb shell 'pidof waved' 2>/dev/null | tr -d '\r')"
        [[ -n "$p" ]] && break
        sleep 1
    done
    if [[ -n "$p" ]]; then
        echo "  waved running (pid $p)"
    else
        echo "  WARNING: waved not back — check device (adb shell /etc/init.d/S60waved start)"
    fi
    exit "$rc"
}
trap cleanup EXIT INT TERM

# 1. Build + deploy the probe.
if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
    echo "==> building waved-wakeup-probe ($TARGET)..."
    (cd "$REPO_ROOT" && cross build --release --target "$TARGET" -p waved-wakeup-probe)
fi
[[ -f "$PROBE_BIN" ]] || { echo "ERROR: $PROBE_BIN not found — run without SKIP_BUILD=1"; exit 1; }
echo "==> deploying probe..."
adb push "$PROBE_BIN" "$DEVICE_PROBE" >/dev/null
adb shell "chmod +x $DEVICE_PROBE"

# 2. Stop waved + watchdog so the probe owns the DSP (no respawn mid-run).
echo "==> stopping waved + watchdog (frees the DSP for the probe)..."
adb shell "killall S60waved 2>/dev/null; killall -9 waved 2>/dev/null; sleep 2; true"

# 3. Run the benchmark (blocks until the probe finishes).
echo "==> running: waved-wakeup-probe ${PROBE_ARGS[*]} --json $DEVICE_JSON"
echo "------------------------------------------------------------------"
adb shell "$DEVICE_PROBE ${PROBE_ARGS[*]} --json $DEVICE_JSON"
echo "------------------------------------------------------------------"

# 4. Pull the JSON report to the host.
if adb pull "$DEVICE_JSON" "$HOST_JSON" >/dev/null 2>&1; then
    echo "==> report saved: $HOST_JSON"
else
    echo "==> note: no JSON report was produced"
fi

# cleanup() (trap EXIT) removes the probe + JSON and restarts waved.
