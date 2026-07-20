#!/usr/bin/env bash
set -euo pipefail

# Configurable P0 recording-resilience acceptance.
#
# Required build artifacts:
#   cross build --release --target aarch64-unknown-linux-gnu \
#     --features "ui,capture-alsa,recording-integrity-test-hooks" -p waved
#   cross build --release --target aarch64-unknown-linux-gnu \
#     --features recording-integrity-test-hooks \
#     -p waved-meeting --bin retention-acceptance
#
# Quick proof:
#   ALLOW_DEVICE_REBOOT=yes ADB_SERIAL=<serial> \
#     DURATION_SECONDS=20 ./scripts/test-recording-resilience.sh
#
# One-hour certification:
#   ALLOW_DEVICE_REBOOT=yes ADB_SERIAL=<serial> \
#     DURATION_SECONDS=3600 SAMPLE_INTERVAL_SECONDS=30 \
#     ./scripts/test-recording-resilience.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET="${TARGET:-aarch64-unknown-linux-gnu}"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target-centos}"
RELEASE_DIR="$TARGET_DIR/$TARGET/release"
HIL_BINARY="${HIL_BINARY:-$RELEASE_DIR/waved.hil.stripped}"
PRODUCTION_BINARY="${PRODUCTION_BINARY:-$RELEASE_DIR/waved.production.stripped}"
RETENTION_BINARY="${RETENTION_BINARY:-$RELEASE_DIR/retention-acceptance.stripped}"
DURATION_SECONDS="${DURATION_SECONDS:-20}"
SAMPLE_INTERVAL_SECONDS="${SAMPLE_INTERVAL_SECONDS:-2}"
RUN_PHASES="${RUN_PHASES:-all}"
DEVICE_BINARY="${DEVICE_BINARY:-/data/waved/waved}"
DEVICE_RETENTION_BINARY="/tmp/waved-retention-acceptance"
DATABASE="/data/waved/meetings.db"
RECORDINGS="/mnt/udisk/recordings"
FAULT_HOOK="/tmp/waved-integrity-fault"
CHECKPOINT_REQUEST="/tmp/waved-finalization-checkpoint"
CHECKPOINT_READY="/tmp/waved-finalization-ready"
EVIDENCE_DIR="${EVIDENCE_DIR:-$REPO_ROOT/recording-resilience-$(date +%Y%m%d-%H%M%S)}"

ADB=(adb)
if [[ -n "${ADB_SERIAL:-}" ]]; then
  ADB+=(-s "$ADB_SERIAL")
fi

DEPLOYED_HIL=0
RESTORE_OK=0

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || fail "required host tool not found: $1"
}

adb_shell() {
  "${ADB[@]}" shell "$@"
}

host_sha256() {
  shasum -a 256 "$1" | awk '{print $1}'
}

device_sha256() {
  adb_shell "sha256sum '$1'" | tr -d '\r' | awk '{print $1}'
}

first_pid() {
  adb_shell "pidof waved 2>/dev/null" | tr -d '\r' | awk '{print $1}'
}

process_count() {
  adb_shell "pidof waved 2>/dev/null" | tr -d '\r' | awk '{print NF}'
}

watchdog_count() {
  adb_shell "ps -o pid,args 2>/dev/null" |
    tr -d '\r' |
    awk '$2 == "/bin/sh" && $3 == "/etc/init.d/S60waved" && $4 == "start" { count++ } END { print count + 0 }'
}

db_query() {
  local sql=$1
  adb_shell "sqlite3 -noheader '$DATABASE' \"$sql\"" | tr -d '\r'
}

recording_count() {
  db_query "SELECT COUNT(*) FROM meetings;"
}

latest_recording() {
  db_query "SELECT id FROM meetings ORDER BY rowid DESC LIMIT 1;"
}

meeting_status() {
  local id=$1
  db_query "SELECT upload_status FROM meetings WHERE id = '$id';"
}

stop_supervisor() {
  adb_shell "pids=\$(ps -o pid,args 2>/dev/null | awk '\$2 == \"/bin/sh\" && \$3 == \"/etc/init.d/S60waved\" && \$4 == \"start\" { print \$1 }'); \
    test -z \"\$pids\" || kill -9 \$pids 2>/dev/null; \
    killall -9 waved 2>/dev/null; sleep 2; true" >/dev/null
}

start_supervisor() {
  adb_shell "start-stop-daemon -S -b -x /bin/sh -- /etc/init.d/S60waved start"
}

wait_for_device() {
  local deadline=$((SECONDS + 120))
  while (( SECONDS < deadline )); do
    if [[ "$("${ADB[@]}" get-state 2>/dev/null || true)" == device ]]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_for_stable_waved() {
  local deadline=$((SECONDS + 60))
  local stable_pid=""
  local stable_since=0
  while (( SECONDS < deadline )); do
    local count pid
    count=$(process_count 2>/dev/null || echo 0)
    pid=$(first_pid 2>/dev/null || true)
    if [[ "$count" == 1 && -n "$pid" ]]; then
      if [[ "$pid" != "$stable_pid" ]]; then
        stable_pid=$pid
        stable_since=$SECONDS
      elif (( SECONDS - stable_since >= 8 )); then
        echo "$pid"
        return 0
      fi
    else
      stable_pid=""
    fi
    sleep 1
  done
  return 1
}

restart_for_second_reconcile() {
  local old_pid
  old_pid=$(first_pid)
  adb_shell "kill -TERM '$old_pid'" >/dev/null
  local deadline=$((SECONDS + 45))
  while (( SECONDS < deadline )); do
    local new_pid
    new_pid=$(first_pid 2>/dev/null || true)
    if [[ -n "$new_pid" && "$new_pid" != "$old_pid" ]]; then
      sleep 3
      [[ "$(process_count)" == 1 ]] || return 1
      return 0
    fi
    sleep 1
  done
  return 1
}

restore_production() {
  [[ "$DEPLOYED_HIL" == 1 ]] || return 0
  wait_for_device || return 1
  "${ADB[@]}" push "$PRODUCTION_BINARY" "$DEVICE_BINARY.restore" >/dev/null || return 1
  adb_shell "chmod 0755 '$DEVICE_BINARY.restore'" >/dev/null || return 1
  stop_supervisor || return 1
  adb_shell "mv -f '$DEVICE_BINARY.restore' '$DEVICE_BINARY'; \
    rm -f '$FAULT_HOOK' '$CHECKPOINT_REQUEST' '$CHECKPOINT_READY' \
      '$CHECKPOINT_READY.new' '$DEVICE_RETENTION_BINARY'" >/dev/null || return 1
  start_supervisor >/dev/null || return 1
  wait_for_stable_waved >/dev/null || return 1
  [[ "$(device_sha256 "$DEVICE_BINARY")" == "$(host_sha256 "$PRODUCTION_BINARY")" ]] || return 1
  [[ "$(process_count)" == 1 && "$(watchdog_count)" == 1 ]] || return 1
  RESTORE_OK=1
}

cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  set +e
  if ! restore_production; then
    echo "ERROR: production restoration failed; inspect the device immediately" >&2
    rc=1
  fi
  echo "Evidence: $EVIDENCE_DIR"
  if [[ "$RESTORE_OK" == 1 ]]; then
    echo "Production binary restored and watchdog state verified."
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM

inject() {
  local action=$1
  adb_shell "printf '%s\n' '$action' > '$FAULT_HOOK'; kill -HUP \$(pidof waved)"
}

wait_for_new_recording() {
  local before=$1
  local deadline=$((SECONDS + 12))
  while (( SECONDS < deadline )); do
    local count
    count=$(recording_count)
    if (( count > before )); then
      latest_recording
      return 0
    fi
    sleep 0.25
  done
  return 1
}

wait_for_checkpoint() {
  local phase=$1
  local deadline=$((SECONDS + 20))
  while (( SECONDS < deadline )); do
    local ready
    ready=$(adb_shell "cat '$CHECKPOINT_READY' 2>/dev/null" | tr -d '\r' || true)
    if [[ "$ready" == "$phase "* ]]; then
      echo "${ready#* }"
      return 0
    fi
    sleep 0.25
  done
  return 1
}

wait_for_status() {
  local id=$1
  local expected=$2
  local deadline=$((SECONDS + 25))
  while (( SECONDS < deadline )); do
    if [[ "$(meeting_status "$id")" == "$expected" ]]; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

assert_not_uploadable() {
  local id=$1
  local count
  count=$(db_query "SELECT COUNT(*) FROM meetings WHERE id = '$id' AND upload_status IN ('queued','retry_wait','uploading');")
  [[ "$count" == 0 ]] || fail "interrupted artifact $id became uploadable"
}

snapshot_recordings() {
  adb_shell "find '$RECORDINGS' -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort" |
    tr -d '\r'
}

pull_stats() {
  local output=$1
  "${ADB[@]}" pull /tmp/audio_stats.json "$output" >/dev/null
}

json_counter() {
  local file=$1
  local field=$2
  jq -r ".$field // 0" "$file"
}

phase_enabled() {
  local phase=$1
  [[ "$RUN_PHASES" == all || ",$RUN_PHASES," == *",$phase,"* ]]
}

validate_wav_and_metadata() {
  local id=$1
  local local_dir="$EVIDENCE_DIR/endurance/$id"
  mkdir -p "$local_dir"
  "${ADB[@]}" pull "$RECORDINGS/$id/audio.wav" "$local_dir/audio.wav" >/dev/null
  "${ADB[@]}" pull "$RECORDINGS/$id/metadata.json" "$local_dir/metadata.json" >/dev/null
  [[ "$(adb_shell "test ! -e '$RECORDINGS/$id/audio.wav.partial'; echo \$?" | tr -d '\r')" == 0 ]] ||
    fail "finalized recording retained a partial WAV"

  file "$local_dir/audio.wav" | tee "$local_dir/file.txt"
  grep -q "WAVE audio, Microsoft PCM, 16 bit, mono 48000 Hz" "$local_dir/file.txt" ||
    fail "unexpected WAV format"

  local file_bytes data_bytes sample_count duration_ms metadata_bytes metadata_duration
  file_bytes=$(wc -c < "$local_dir/audio.wav" | tr -d ' ')
  data_bytes=$(od -An -tu4 -j40 -N4 "$local_dir/audio.wav" | tr -d ' ')
  [[ "$file_bytes" == $((data_bytes + 44)) ]] || fail "WAV RIFF/data sizes disagree"
  (( data_bytes % 2 == 0 )) || fail "WAV data chunk has a partial 16-bit sample"
  sample_count=$((data_bytes / 2))
  duration_ms=$((sample_count * 1000 / 48000))
  metadata_bytes=$(jq -r '.audio.size_bytes' "$local_dir/metadata.json")
  metadata_duration=$(jq -r '.recording.audio_duration_ms' "$local_dir/metadata.json")
  [[ "$metadata_bytes" == "$file_bytes" ]] || fail "metadata WAV size mismatch"
  [[ "$metadata_duration" == "$duration_ms" ]] || fail "metadata WAV duration mismatch"
  [[ "$(jq -r '.audio.sample_rate_hz' "$local_dir/metadata.json")" == 48000 ]] ||
    fail "metadata sample rate mismatch"
  [[ "$(jq -r '.audio.channels' "$local_dir/metadata.json")" == 1 ]] ||
    fail "metadata channel count mismatch"
  [[ "$(jq -r '.audio.bit_depth' "$local_dir/metadata.json")" == 16 ]] ||
    fail "metadata bit depth mismatch"
  [[ "$(jq -r '.audio.sha256' "$local_dir/metadata.json")" == "$(host_sha256 "$local_dir/audio.wav")" ]] ||
    fail "metadata WAV checksum mismatch"

  jq -n \
    --arg id "$id" \
    --argjson file_bytes "$file_bytes" \
    --argjson data_bytes "$data_bytes" \
    --argjson sample_count "$sample_count" \
    --argjson duration_ms "$duration_ms" \
    '{meeting_id:$id,file_bytes:$file_bytes,data_bytes:$data_bytes,sample_count:$sample_count,audio_duration_ms:$duration_ms}' \
    > "$local_dir/validation.json"
}

run_endurance_phase() {
  echo "1/5 Configurable real recording"
  local phase_dir="$EVIDENCE_DIR/endurance"
  mkdir -p "$phase_dir/stats"
  adb_shell "rm -f '$FAULT_HOOK' '$CHECKPOINT_REQUEST' '$CHECKPOINT_READY' '$CHECKPOINT_READY.new'"
  pull_stats "$phase_dir/stats/baseline.json"
  local before id start_seconds sample_index last_size first_size final_size
  before=$(recording_count)
  inject start
  id=$(wait_for_new_recording "$before") || fail "recording did not start"
  echo "$id" > "$phase_dir/meeting-id.txt"
  echo "elapsed_s,pid,rss_kb,free_kb,partial_bytes,frames_emitted,frames_dropped,xruns,raw_drops,processed_drops,status" \
    > "$phase_dir/samples.csv"
  start_seconds=$SECONDS
  sample_index=0
  last_size=0
  first_size=0

  while true; do
    local elapsed pid remote rss free_kb partial_bytes status stats_file
    elapsed=$((SECONDS - start_seconds))
    pid=$(first_pid)
    remote=$(adb_shell "rss=\$(awk '/VmRSS:/ { print \$2 }' '/proc/$pid/status'); \
      free=\$(df -Pk '$RECORDINGS' | awk 'NR == 2 { print \$4 }'); \
      size=\$(wc -c < '$RECORDINGS/$id/audio.wav.partial'); \
      status=\$(sqlite3 -noheader '$DATABASE' \"SELECT upload_status FROM meetings WHERE id = '$id';\"); \
      printf '%s,%s,%s,%s\n' \"\$rss\" \"\$free\" \"\$size\" \"\$status\"" | tr -d '\r')
    IFS=, read -r rss free_kb partial_bytes status <<< "$remote"
    stats_file="$phase_dir/stats/$(printf '%04d' "$sample_index").json"
    pull_stats "$stats_file"
    local frames frames_dropped xruns raw_drops processed_drops
    frames=$(json_counter "$stats_file" frames_emitted)
    frames_dropped=$(json_counter "$stats_file" frames_dropped)
    xruns=$(json_counter "$stats_file" xrun_count)
    raw_drops=$(json_counter "$stats_file" raw_frames_dropped)
    processed_drops=$(json_counter "$stats_file" processed_frames_dropped)
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
      "$elapsed" "$pid" "$rss" "$free_kb" "$partial_bytes" "$frames" \
      "$frames_dropped" "$xruns" "$raw_drops" "$processed_drops" "$status" \
      >> "$phase_dir/samples.csv"
    (( sample_index == 0 )) && first_size=$partial_bytes
    (( partial_bytes >= last_size )) || fail "partial WAV shrank during recording"
    last_size=$partial_bytes
    final_size=$partial_bytes
    (( elapsed >= DURATION_SECONDS )) && break
    sample_index=$((sample_index + 1))
    sleep "$SAMPLE_INTERVAL_SECONDS"
  done
  (( final_size > first_size )) || fail "partial WAV did not grow during recording"

  inject stop
  local deadline=$((SECONDS + 30))
  while (( SECONDS < deadline )); do
    status=$(meeting_status "$id")
    [[ "$status" != recording ]] && break
    sleep 0.5
  done
  [[ "$status" =~ ^(queued|uploading|uploaded|awaiting_decision|local_only)$ ]] ||
    fail "unexpected final recording status: $status"
  echo "$status" > "$phase_dir/final-status.txt"
  pull_stats "$phase_dir/stats/final.json"

  for field in frames_dropped xrun_count raw_frames_dropped processed_frames_dropped; do
    local baseline final
    baseline=$(json_counter "$phase_dir/stats/baseline.json" "$field")
    final=$(json_counter "$phase_dir/stats/final.json" "$field")
    [[ "$baseline" == "$final" ]] || fail "$field increased during the accepted recording"
  done
  local baseline_frames final_frames
  baseline_frames=$(json_counter "$phase_dir/stats/baseline.json" frames_emitted)
  final_frames=$(json_counter "$phase_dir/stats/final.json" frames_emitted)
  (( final_frames > baseline_frames )) || fail "capture frame counter did not advance"

  local initial_rss peak_rss final_rss
  initial_rss=$(awk -F, 'NR == 2 { print $3 }' "$phase_dir/samples.csv")
  peak_rss=$(awk -F, 'NR > 1 && $3 > max { max=$3 } END { print max+0 }' "$phase_dir/samples.csv")
  final_rss=$(awk -F, 'END { print $3 }' "$phase_dir/samples.csv")
  jq -n \
    --argjson duration_seconds "$DURATION_SECONDS" \
    --argjson initial_rss_kb "$initial_rss" \
    --argjson peak_rss_kb "$peak_rss" \
    --argjson final_rss_kb "$final_rss" \
    '{duration_seconds:$duration_seconds,initial_rss_kb:$initial_rss_kb,peak_rss_kb:$peak_rss_kb,final_rss_kb:$final_rss_kb}' \
    > "$phase_dir/memory-summary.json"
  if (( DURATION_SECONDS >= 3600 )); then
    local allowance
    allowance=$((initial_rss / 5))
    (( allowance < 32768 )) && allowance=32768
    (( final_rss <= initial_rss + allowance )) ||
      fail "one-hour final RSS exceeded the 20%/32 MiB growth allowance"
  fi

  validate_wav_and_metadata "$id"
  echo "  PASS: $id recorded for ${DURATION_SECONDS}s with consistent WAV and metadata"
}

run_retention_phase() {
  echo "2/5 Isolated storage-pressure safety"
  local phase_dir="$EVIDENCE_DIR/retention"
  local fixture
  fixture="/tmp/waved-retention-acceptance-$(date +%s)-$$"
  mkdir -p "$phase_dir"
  snapshot_recordings > "$phase_dir/recordings-before.txt"
  adb_shell "'$DEVICE_RETENTION_BINARY' '$fixture'" |
    tr -d '\r' |
    tee "$phase_dir/result.json"
  jq -e '.result == "PASS" and .percentage_threshold == "PASS" and
    .minimum_free_threshold == "PASS" and .blocked_without_candidate == true' \
    "$phase_dir/result.json" >/dev/null || fail "retention fixture reported failure"
  snapshot_recordings > "$phase_dir/recordings-after.txt"
  diff -u "$phase_dir/recordings-before.txt" "$phase_dir/recordings-after.txt" \
    > "$phase_dir/recordings.diff" ||
    fail "isolated retention fixture changed real recording directories"
  adb_shell "rm -rf '$fixture'"
  echo "  PASS: both thresholds pruned only uploaded fixture artifacts"
}

run_interruption_phase() {
  local ordinal=$1
  local phase=$2
  local expected_status=$3
  local phase_dir="$EVIDENCE_DIR/interruption-$phase"
  mkdir -p "$phase_dir"
  echo "$ordinal/5 Reboot checkpoint: $phase"
  adb_shell "rm -f '$FAULT_HOOK' '$CHECKPOINT_READY' '$CHECKPOINT_READY.new'; \
    printf '%s\n' '$phase' > '$CHECKPOINT_REQUEST'"

  local before id
  before=$(recording_count)
  inject start
  id=$(wait_for_new_recording "$before") || fail "$phase recording did not start"
  if [[ "$phase" != active-partial ]]; then
    sleep 2
    inject stop
  fi
  local checkpoint_id
  checkpoint_id=$(wait_for_checkpoint "$phase") || fail "$phase checkpoint was not reached"
  [[ "$checkpoint_id" == "$id" ]] || fail "$phase checkpoint meeting ID mismatch"
  echo "$id" > "$phase_dir/meeting-id.txt"
  adb_shell "sqlite3 -line '$DATABASE' \"SELECT * FROM meetings WHERE id = '$id';\"; \
    ls -la '$RECORDINGS/$id'" > "$phase_dir/before-reboot.txt"

  "${ADB[@]}" reboot >/dev/null 2>&1 || true
  wait_for_device || fail "device did not return after $phase reboot"
  sleep 2
  adb_shell "rm -f '$FAULT_HOOK' '$CHECKPOINT_REQUEST' '$CHECKPOINT_READY' '$CHECKPOINT_READY.new'"
  if ! wait_for_stable_waved > "$phase_dir/first-reconcile-pid.txt"; then
    start_supervisor >/dev/null
    wait_for_stable_waved > "$phase_dir/first-reconcile-pid.txt" ||
      fail "waved did not stabilize after $phase reboot"
  fi
  wait_for_status "$id" "$expected_status" ||
    fail "$phase reconciled as $(meeting_status "$id"), expected $expected_status"
  assert_not_uploadable "$id"

  case "$phase" in
    active-partial)
      adb_shell "test -f '$RECORDINGS/$id/audio.wav.partial' &&
        test ! -e '$RECORDINGS/$id/audio.wav' &&
        test ! -e '$RECORDINGS/$id/metadata.json'" ||
        fail "active interruption artifact shape is unsafe"
      ;;
    wav-finalized)
      adb_shell "test -f '$RECORDINGS/$id/audio.wav' &&
        test ! -e '$RECORDINGS/$id/audio.wav.partial' &&
        test ! -e '$RECORDINGS/$id/metadata.json'" ||
        fail "WAV-finalized interruption artifact shape is unsafe"
      ;;
    metadata-finalized)
      adb_shell "test -f '$RECORDINGS/$id/audio.wav' &&
        test -f '$RECORDINGS/$id/metadata.json' &&
        test ! -e '$RECORDINGS/$id/audio.wav.partial'" ||
        fail "metadata-finalized interruption artifact shape is unsafe"
      ;;
  esac
  adb_shell "sha256sum '$RECORDINGS/$id'/*; \
    sqlite3 -line '$DATABASE' \"SELECT * FROM meetings WHERE id = '$id';\"" \
    > "$phase_dir/after-first-reconcile.txt"

  restart_for_second_reconcile || fail "second reconciliation restart failed for $phase"
  wait_for_status "$id" "$expected_status" ||
    fail "$phase changed status during the second reconciliation"
  assert_not_uploadable "$id"
  adb_shell "sha256sum '$RECORDINGS/$id'/*; \
    sqlite3 -line '$DATABASE' \"SELECT * FROM meetings WHERE id = '$id';\"" \
    > "$phase_dir/after-second-reconcile.txt"
  diff -u "$phase_dir/after-first-reconcile.txt" "$phase_dir/after-second-reconcile.txt" \
    > "$phase_dir/reconcile.diff" ||
    fail "$phase reconciliation was not idempotent"
  echo "  PASS: $id remained $expected_status and non-uploadable across two reconciliations"
}

for tool in adb jq shasum od awk sed grep diff file; do
  require_tool "$tool"
done
[[ "$ALLOW_DEVICE_REBOOT" == yes ]] ||
  fail "set ALLOW_DEVICE_REBOOT=yes to acknowledge the three deliberate reboots"
[[ "$DURATION_SECONDS" =~ ^[1-9][0-9]*$ ]] || fail "DURATION_SECONDS must be a positive integer"
[[ "$SAMPLE_INTERVAL_SECONDS" =~ ^[1-9][0-9]*$ ]] ||
  fail "SAMPLE_INTERVAL_SECONDS must be a positive integer"
[[ "$RUN_PHASES" =~ ^(all|[a-z-]+(,[a-z-]+)*)$ ]] ||
  fail "RUN_PHASES must be all or a comma-separated phase list"
for artifact in "$HIL_BINARY" "$PRODUCTION_BINARY" "$RETENTION_BINARY"; do
  [[ -x "$artifact" || -f "$artifact" ]] || fail "build artifact not found: $artifact"
done
mkdir -p "$EVIDENCE_DIR"

wait_for_device || fail "ADB device is not reachable"
[[ "$(process_count)" == 1 ]] || fail "expected exactly one running waved process"
[[ "$(watchdog_count)" == 1 ]] || fail "expected exactly one S60waved watchdog"
production_sha=$(host_sha256 "$PRODUCTION_BINARY")
hil_sha=$(host_sha256 "$HIL_BINARY")
installed_before_sha=$(device_sha256 "$DEVICE_BINARY")
if [[ -n "${EXPECTED_INSTALLED_SHA256:-}" ]]; then
  [[ "$installed_before_sha" == "$EXPECTED_INSTALLED_SHA256" ]] ||
    fail "installed binary does not match EXPECTED_INSTALLED_SHA256"
fi
printf 'installed_before_sha256=%s\nproduction_sha256=%s\nhil_sha256=%s\n' \
  "$installed_before_sha" "$production_sha" "$hil_sha" \
  > "$EVIDENCE_DIR/binaries.txt"

upload_mode=$(adb_shell "sed -n 's/^[[:space:]]*upload_mode[[:space:]]*=[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p' /data/waved/config.toml | head -n 1" |
  tr -d '\r')
[[ "$upload_mode" != ask ]] ||
  fail "upload_mode=ask leaves a pending UI decision; use automatic or local_only for unattended acceptance"

echo "Deploying feature-gated HIL binary..."
"${ADB[@]}" push "$HIL_BINARY" "$DEVICE_BINARY.acceptance" >/dev/null
"${ADB[@]}" push "$RETENTION_BINARY" "$DEVICE_RETENTION_BINARY" >/dev/null
adb_shell "chmod 0755 '$DEVICE_BINARY.acceptance' '$DEVICE_RETENTION_BINARY'"
stop_supervisor
adb_shell "mv -f '$DEVICE_BINARY.acceptance' '$DEVICE_BINARY'; \
  rm -f '$FAULT_HOOK' '$CHECKPOINT_REQUEST' '$CHECKPOINT_READY' '$CHECKPOINT_READY.new'"
DEPLOYED_HIL=1
start_supervisor >/dev/null
wait_for_stable_waved > "$EVIDENCE_DIR/hil-pid.txt" || fail "HIL waved process did not stabilize"
[[ "$(device_sha256 "$DEVICE_BINARY")" == "$hil_sha" ]] || fail "HIL checksum mismatch after deploy"
[[ "$(process_count)" == 1 && "$(watchdog_count)" == 1 ]] ||
  fail "HIL supervisor state is not singular"

phase_enabled endurance && run_endurance_phase
phase_enabled retention && run_retention_phase
phase_enabled active-partial &&
  run_interruption_phase 3 active-partial artifact_error
phase_enabled wav-finalized &&
  run_interruption_phase 4 wav-finalized missing_artifact
phase_enabled metadata-finalized &&
  run_interruption_phase 5 metadata-finalized local_only

echo "PASS: shortened recording resilience and storage-pressure acceptance completed"
echo "One-hour certification:"
echo "  ALLOW_DEVICE_REBOOT=yes ADB_SERIAL=${ADB_SERIAL:-<serial>} DURATION_SECONDS=3600 SAMPLE_INTERVAL_SECONDS=30 ./scripts/test-recording-resilience.sh"
