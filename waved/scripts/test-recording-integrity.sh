#!/usr/bin/env bash
set -euo pipefail

# Hardware acceptance for a binary built with:
#   --features ui,capture-alsa,recording-integrity-test-hooks
#
# The script never removes recording data or changes config/database files.
# Interrupted takes remain on-device as audio.<opus|flac|wav>.partial for
# inspection.

ADB=(adb)
if [[ -n "${ADB_SERIAL:-}" ]]; then
  ADB+=(-s "$ADB_SERIAL")
fi

HOOK=/tmp/waved-integrity-fault
RECORDINGS=/mnt/udisk/recordings
BACKUP=$(mktemp)
HAD_HOOK=0

adb_shell() {
  "${ADB[@]}" shell "$@"
}

cleanup() {
  adb_shell "rm -f '$HOOK'" >/dev/null 2>&1 || true
  if [[ "$HAD_HOOK" == 1 ]]; then
    "${ADB[@]}" push "$BACKUP" "$HOOK" >/dev/null 2>&1 || true
  fi
  rm -f "$BACKUP"
}
trap cleanup EXIT

if [[ "$(adb_shell "if test -f '$HOOK'; then echo yes; fi" | tr -d '\r')" == yes ]]; then
  HAD_HOOK=1
  "${ADB[@]}" pull "$HOOK" "$BACKUP" >/dev/null
fi

inject() {
  local action=$1
  adb_shell "printf '%s\n' '$action' > '$HOOK'; kill -HUP \$(pidof waved)"
}

first_pid() {
  adb_shell "pidof waved" | tr -d '\r' | awk '{print $1}'
}

recording_count() {
  adb_shell "find '$RECORDINGS' -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l" |
    tr -d '\r '
}

newest_recording() {
  adb_shell "ls -1dt '$RECORDINGS'/* 2>/dev/null | head -n 1" | tr -d '\r'
}

wait_for_new_recording() {
  local before=$1
  local deadline=$((SECONDS + 8))
  while (( SECONDS < deadline )); do
    local now
    now=$(recording_count)
    if (( now > before )); then
      newest_recording
      return 0
    fi
    sleep 0.25
  done
  echo "Timed out waiting for a new recording directory" >&2
  return 1
}

wait_for_restart() {
  local old_pid=$1
  local deadline=$((SECONDS + 45))
  while (( SECONDS < deadline )); do
    local new_pid
    new_pid=$(first_pid || true)
    if [[ -n "$new_pid" && "$new_pid" != "$old_pid" ]]; then
      printf '%s\n' "$new_pid"
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for watchdog restart (old PID $old_pid)" >&2
  return 1
}

assert_partial_only() {
  local dir=$1
  if [[ "$(
    adb_shell "for name in audio.opus.partial audio.flac.partial audio.wav.partial; do \
      if test -f '$dir'/\$name; then echo yes; fi; done" | tr -d '\r'
  )" != yes ]]; then
    echo "Faulted take did not preserve its partial audio file: $dir" >&2
    return 1
  fi
  if [[ "$(
    adb_shell "if test -e '$dir/audio.opus' -o -e '$dir/audio.flac' -o -e '$dir/audio.wav' \
      -o -e '$dir/metadata.json'; then echo yes; fi" |
      tr -d '\r'
  )" == yes ]]; then
    echo "Faulted take was exposed as a complete artifact: $dir" >&2
    return 1
  fi
}

echo "Checking non-production hook..."
inject capture-xrun
sleep 1
if [[ "$(adb_shell "if test -e '$HOOK'; then echo yes; fi" | tr -d '\r')" == yes ]]; then
  echo "Hook was not consumed. Deploy the test-hook build before running this script." >&2
  exit 1
fi

echo "1/3 Active processed-frame drop must preserve only a partial WAV"
pid_before=$(first_pid)
count_before=$(recording_count)
inject start
drop_dir=$(wait_for_new_recording "$count_before")
sleep 1
inject processed-frame-dropped
sleep 1
assert_partial_only "$drop_dir"
[[ "$(first_pid)" == "$pid_before" ]] || {
  echo "Non-fatal frame drop unexpectedly restarted waved" >&2
  exit 1
}

echo "2/3 Active capture failure must abort, acknowledge, and restart"
count_before=$(recording_count)
inject start
fatal_dir=$(wait_for_new_recording "$count_before")
sleep 1
fatal_pid=$(first_pid)
inject capture-read-failed
wait_for_restart "$fatal_pid" >/dev/null
assert_partial_only "$fatal_dir"

echo "3/3 WAV write failure must abort and block another start without looping"
count_before=$(recording_count)
inject start
write_dir=$(wait_for_new_recording "$count_before")
sleep 1
write_pid=$(first_pid)
inject wav-write-failed
sleep 1
assert_partial_only "$write_dir"
blocked_count=$(recording_count)
inject start
sleep 2
[[ "$(recording_count)" == "$blocked_count" ]] || {
  echo "Storage fault did not block a new recording" >&2
  exit 1
}
[[ "$(first_pid)" == "$write_pid" ]] || {
  echo "Storage fault caused a restart loop" >&2
  exit 1
}

echo "Clearing the process-local block through one controlled watchdog restart"
inject capture-read-failed
wait_for_restart "$write_pid" >/dev/null

echo "PASS: faulted takes stayed partial, uploads stayed ineligible, and restart policy held"
