#!/usr/bin/env bash
# Tier 3 on-device smoke test for waved-audio capture foundation.
# Run from host with device connected via adb.
set -euo pipefail

echo "==> Building"
cross build --target aarch64-unknown-linux-gnu -p waved -p waved-latency-probe -p waved-capture-record --features "ui,capture-alsa"

echo "==> Deploying"
adb push target/aarch64-unknown-linux-gnu/debug/waved /data/waved/waved
adb push target/aarch64-unknown-linux-gnu/debug/waved-latency-probe /data/waved/
adb push target/aarch64-unknown-linux-gnu/debug/waved-capture-record /data/waved/

echo "==> Restarting waved"
adb shell "killall waved 2>/dev/null; sleep 2"
adb shell "nohup /data/waved/waved >/tmp/waved.log 2>&1 &"
sleep 5

echo "==> Checking /tmp/audio_stats.json exists + valid JSON"
adb shell 'test -f /tmp/audio_stats.json && cat /tmp/audio_stats.json' | python3 -m json.tool > /dev/null
echo "OK"

echo "==> Checking all 8 mic RMS values present"
adb shell 'cat /tmp/audio_stats.json' | python3 -c '
import json, sys
s = json.load(sys.stdin)
assert len(s["rms_per_channel"]) == 8, f"expected 8 rms, got {len(s[\"rms_per_channel\"])}"
assert isinstance(s["xrun_count"], int), "xrun_count must be int"
assert isinstance(s["frames_emitted"], int), "frames_emitted must be int"
assert s["frames_emitted"] > 0, f"no frames emitted after 5s"
print(f"  frames_emitted={s[\"frames_emitted\"]} xruns={s[\"xrun_count\"]}")
print(f"  rms: {[round(v,1) for v in s[\"rms_per_channel\"]]}")
'

echo "==> 10-second capture with waved-capture-record"
adb shell "killall waved 2>/dev/null; sleep 1"
adb shell "/data/waved/waved-capture-record --duration 10 --out /tmp/smoke.pcm --description 'smoke test'"
adb shell 'ls -la /tmp/smoke.pcm /tmp/smoke.meta.json'

echo "==> Latency probe × 5"
adb shell "for i in 1 2 3 4 5; do /data/waved/waved-latency-probe; sleep 1; done" | tee /tmp/latency-probe-log.txt
python3 -c "
import json, re, sys
vals = []
for line in open('/tmp/latency-probe-log.txt'):
    m = re.search(r'\"delay_ms\": ([-\d.]+)', line)
    if m: vals.append(float(m.group(1)))
assert len(vals) >= 5, f'got {len(vals)} latency samples'
import statistics
mean = statistics.mean(vals)
stdev = statistics.stdev(vals) if len(vals) > 1 else 0.0
print(f'  mean={mean:.2f}ms stdev={stdev:.2f}ms')
assert stdev < 5.0, f'latency stdev too high ({stdev:.2f}ms)'
print('OK')
"

echo "==> Restarting waved"
adb shell "nohup /data/waved/waved >/tmp/waved.log 2>&1 &"
echo "==> Smoke test passed"
