#!/usr/bin/env bash
# F3 on-device acceptance: drive a real meeting on the Wave with the
# operator standing at different azimuths, then pull the SQLite db and
# verify diarization_segments rows.
#
# Prereqs:
# - device reachable via adb
# - waved binary at /data/waved/waved (per scripts/test-doa-improvements.sh)
# - operator able to physically reposition themselves

set -euo pipefail

DEVICE="${DEVICE:-PSP_0000000000000000}"
WAVED_BIN="${WAVED_BIN:-/data/waved/waved}"
DB_PATH="${DB_PATH:-/data/waved/meetings.db}"

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

main() {
    adb_kill_waved
    adb_start_waved

    prompt "stand at az=0° (front of device); confirm waved is running"

    # Start a meeting via the SIGHUP / command bus is hardware-dependent.
    # Easier: rely on the touch UI. Operator triggers Start themselves.
    prompt "start recording via touch UI; talk for ~5 seconds"
    prompt "WALK to az=180° (behind device); talk for ~5 seconds"
    prompt "WALK back to az=0°; talk for ~5 seconds"
    prompt "stop recording via touch UI"

    # Pull the db and inspect.
    LOCAL_DB="/tmp/meetings-$(date +%Y%m%d-%H%M%S).db"
    adb pull "$DB_PATH" "$LOCAL_DB"

    echo
    # "Latest meeting" is selected by rowid (insertion order), NOT created_at:
    # the device clock sits at the 1970 epoch, so meetings.created_at
    # (DEFAULT datetime('now')) is non-monotonic and "ORDER BY created_at" can
    # return a stale meeting. rowid is monotonic with INSERT order.
    echo "=== Diarization rows from latest meeting ==="
    sqlite3 "$LOCAL_DB" \
        "SELECT meeting_id, speaker_id, start_ms, end_ms, ROUND(azimuth_deg,1) \
         FROM diarization_segments \
         WHERE meeting_id = (SELECT id FROM meetings ORDER BY rowid DESC LIMIT 1) \
         ORDER BY start_ms"

    # Sanity check: at least 2 distinct speaker_ids in this meeting.
    DISTINCT=$(sqlite3 "$LOCAL_DB" \
        "SELECT COUNT(DISTINCT speaker_id) FROM diarization_segments \
         WHERE meeting_id = (SELECT id FROM meetings ORDER BY rowid DESC LIMIT 1)")
    if [ "$DISTINCT" -ge 2 ]; then
        echo "PASS: $DISTINCT distinct speakers detected (expected ≥ 2)"
    else
        echo "FAIL: only $DISTINCT distinct speakers"
        exit 1
    fi

    # Inspect the speaker_id sequence — expect [1, 2, 1] or similar pattern.
    SEQ=$(sqlite3 "$LOCAL_DB" \
        "SELECT speaker_id FROM diarization_segments \
         WHERE meeting_id = (SELECT id FROM meetings ORDER BY rowid DESC LIMIT 1) \
         ORDER BY start_ms")
    echo "Speaker sequence: $(echo $SEQ | tr '\n' ' ')"
    echo "(Expected: pattern like 1 2 1 for the az=0→180→0 walk.)"
}

main "$@"
