# Wave Setup Portal and Recording Webhook Design

**Date:** 2026-07-17  
**Status:** Approved for implementation  
**Scope:** Configuration over the temporary Wave access point, reliable meeting
recording finalization, generic multipart webhook delivery, upload retry, and
storage retention.

## 1. Context

Wave can already:

- record post-DSP meeting audio;
- store a WAV plus `metadata.json` under `/mnt/udisk/recordings/<meeting-id>/`;
- offer Save, Send, and Discard after recording;
- submit a multipart HTTP request containing audio and metadata;
- retry failed sends when Wi-Fi reconnects; and
- provision Wi-Fi through the WPA2-protected `Wave-XXXX` captive portal.

The current path is not yet suitable for unattended real meetings:

- the live device configuration is invalid and the whole file falls back to
  defaults, including the localhost upload endpoint;
- configuration is loaded once and cloned into services;
- metadata is written only at recording start, leaving duration at zero and
  the device ID empty;
- device wall-clock time remains at the 1970 epoch when the absent `ntpd`
  executable is invoked;
- the uploader reads the complete WAV into RAM;
- database rows become uploadable before the recording decision is complete;
- bookmarks and speaker segments are not included in delivered metadata;
- failed legacy Opus rows are retried as missing WAV files; and
- stale failed rows can make reconnect-triggered flushing spend minutes
  retrying unusable recordings.

This design makes recording-to-webhook delivery dependable and turns the
existing captive portal into the single configuration interface.

## 2. Goals

1. Configure every supported `config.toml` field from a phone connected to the
   temporary `Wave-XXXX` setup network.
2. Keep `config.toml` as the canonical persistent configuration.
3. Define a stable, generic multipart webhook contract containing a finalized
   WAV and complete JSON metadata.
4. Upload automatically by default, with configurable ask and local-only
   modes.
5. Stream large recordings from storage without holding the complete file in
   memory.
6. Make upload state durable, idempotent, observable, and recoverable across
   restarts and Wi-Fi changes.
7. Retain uploaded recordings locally until configurable storage pressure
   requires safe pruning.
8. Migrate the invalid configuration currently present on the development
   device without silently discarding valid settings.

## 3. Non-goals

- An always-on configuration service on the normal home or office LAN.
- A cloud-hosted management application.
- Direct integrations for vendor-specific webhook formats.
- Transcription, summarization, or server-side meeting processing.
- Uploading while `wlan0` is acting as the setup access point.
- Converting legacy Opus recordings to WAV as part of this feature.
- Deleting unsent, failed, awaiting-decision, or local-only recordings
  automatically.
- Supporting arbitrary unknown TOML fields with guessed HTML controls.

## 4. User experience

### 4.1 Entering setup

The user opens Settings → Wi-Fi → Connect/configure on the Wave. The existing
provisioning service:

1. scans nearby station networks;
2. starts the random-password WPA2 `Wave-XXXX` access point;
3. shows its Wi-Fi QR code and manual credentials;
4. runs captive DNS/DHCP/HTTP on `192.168.2.1`; and
5. keeps the access point active until the user submits or cancels setup.

The phone's captive sheet opens the Wave setup application. This application is
available only during this temporary access-point session.

### 4.2 Setup form

The form is split into:

1. **Device** — device name and language.
2. **Audio** — audio mode and speaker/microphone volume.
3. **Webhook** — endpoint, bearer token, upload mode, insecure-HTTP opt-in, and
   retry settings.
4. **Storage** — uploaded-recording pruning threshold and minimum free space.
5. **Advanced** — device password, OTA server, and NTP server list.
6. **Wi-Fi** — nearby network selection, optional other SSID, and password.

The page offers two final actions:

- **Save settings and reconnect** persists settings, tears down the access
  point, and lets the existing saved `wpa_supplicant` profiles reconnect.
- **Save and connect to another network** persists settings and provisions the
  selected station network before reconnecting.

Wi-Fi remains the final step because the device has a single radio. Switching
from access-point mode to station mode necessarily closes the browser session.

### 4.3 Apply and test

After configuration is atomically persisted and station connectivity has been
restored, `waved` performs a controlled restart so every service receives one
consistent configuration snapshot.

When a non-empty endpoint was saved, the restarted process sends a small
multipart `webhook.test` request. Its result appears on the Wave display and is
retained in the on-device Webhook status screen. The setup page explains that
the result will appear on the device because the browser loses the temporary
network before the test can use the internet.

## 5. Configuration model

### 5.1 Canonical TOML

The configuration becomes:

```toml
config_version = 1

[device]
name = "Wave Open"
language = "en"

[audio]
mode = "omni"
speaker_volume = 150
mic_volume = 200

[api]
endpoint = ""
auth_token = ""
upload_mode = "automatic"
allow_insecure_http = false
retry_max = 5
retry_backoff_ms = 1000
retry_max_backoff_ms = 60000

[storage]
prune_uploaded_at_percent = 85
minimum_free_mb = 2048

[security]
device_password = ""
ota_server = ""

[ntp]
servers = ["pool.ntp.org", "time.google.com"]
```

An empty endpoint disables webhook attempts without making the otherwise
default configuration invalid. In `automatic` mode, completed recordings still
enter `queued` and wait for an endpoint to be configured; the setup form shows
a warning about this state. A non-empty endpoint must be valid. Supported
upload modes are:

- `automatic` — finalize and queue immediately;
- `ask` — retain the current Save / Send / Discard decision screen; and
- `local_only` — finalize and retain without opening the decision screen.

### 5.2 Typed field registry

`WavedConfig` remains the typed serde model. A field registry defines one entry
for each configurable path:

```rust
struct FieldSpec {
    path: &'static str,
    section: Section,
    label: &'static str,
    help: &'static str,
    kind: FieldKind,
    required: bool,
    restart_required: bool,
}

enum FieldKind {
    Text { max_len: usize },
    Secret { max_len: usize },
    Url,
    Boolean,
    Select { options: &'static [OptionSpec] },
    AudioMode {
        options: &'static [OptionSpec],
        steered_azimuth_min: f32,
        steered_azimuth_max: f32,
    },
    Integer { min: i64, max: i64, step: i64 },
    StringList { max_items: usize, item_max_len: usize },
}
```

The TOML parser supplies values. The registry supplies HTML control type,
labels, descriptions, ranges, enum options, and secret behavior. This avoids
guessing security or validation semantics from TOML primitive types.
`AudioMode` renders the normal mode dropdown plus the conditional initial
azimuth control required by the existing structured `steered` variant.

`config_version` is schema metadata rather than a user setting and is the only
canonical key not rendered as an editable control.

A coverage test compares the supported configuration paths against the
registry, so adding a new configurable field without a form definition fails
CI.

### 5.3 Secrets

`api.auth_token` and `security.device_password` are never returned to the
browser or placed in rendered HTML.

- A blank submitted secret means “keep the existing value.”
- A separate explicit clear control removes it.
- Logs may state whether a secret is configured but never include its value.
- Validation errors never echo submitted secret content.

### 5.4 Validation

Validation occurs after form decoding and before any persistent write:

- device names and other strings have bounded UTF-8 lengths and reject NUL,
  CR, and LF;
- enums must match the typed configuration variants;
- volumes and retry/storage values must fall within their declared ranges;
- endpoint and OTA fields must parse as absolute URLs where applicable;
- webhook endpoints require HTTPS unless `allow_insecure_http = true`;
- endpoint URLs may not contain embedded credentials;
- NTP hostnames are bounded and reject whitespace/control characters;
- `prune_uploaded_at_percent` is constrained to a safe range;
- `minimum_free_mb` must fit the known storage volume; and
- an empty endpoint produces a visible warning, while any non-empty endpoint
  is cross-validated against the selected transport-security options.

The request body limit increases from 4 KiB to a bounded 32 KiB to accommodate
the full settings form and NTP list.

### 5.5 Atomic persistence

Saving uses this sequence:

1. load and migrate the existing TOML into `WavedConfig`;
2. apply validated submitted values while preserving unchanged secrets;
3. serialize pretty TOML into `config.toml.new`;
4. flush and sync the temporary file;
5. reparse `config.toml.new` through the production loader;
6. set mode `0600`;
7. copy or hard-link the previous valid file to `config.toml.bak` and sync the
   backup without removing the live file;
8. atomically rename the temporary file over `config.toml`; and
9. sync the parent directory.

Any error leaves the live file unchanged. The page remains available with
field-level errors.

### 5.6 Migration

Configuration loading first parses a generic TOML value, applies ordered
versioned migrations, and then deserializes the typed model. The initial
migration:

- adds `config_version = 1`;
- maps `api.auto_upload = true` to `api.upload_mode = "automatic"`;
- maps `api.auto_upload = false` to `api.upload_mode = "ask"`;
- removes the deprecated `api.auto_upload` key;
- maps the obsolete `audio.mode = "noise_reduction"` to `audio.mode = "omni"`;
  and
- supplies defaults for new API and storage fields.

Migration is logged without secret values. A migrated configuration is written
only during an explicit setup save, not merely because the application read
it.

The loader must no longer replace the complete file silently for one bad
field. If the primary file cannot be migrated and validated, startup tries the
last-known-valid backup and publishes a visible configuration fault. Built-in
defaults are used only when neither primary nor backup can be loaded, and that
fallback remains visible in device state and logs.

## 6. Recording artifact lifecycle

### 6.1 Durable finalization

Each recording directory contains:

```text
<meeting-id>/
  audio.wav.partial       # while recording
  audio.wav               # after successful finalization
  metadata.json           # after successful finalization
```

The recorder writes 48 kHz, mono, 16-bit PCM WAV. On stop:

1. finalize the WAV header;
2. sync `audio.wav.partial`;
3. rename it atomically to `audio.wav`;
4. query finalized sample count and file size;
5. read bookmarks and speaker segments from SQLite;
6. calculate SHA-256 by streaming the finalized file;
7. atomically write complete `metadata.json`; and
8. transition the meeting database row from `recording` to its post-finalize
   state.

An artifact is never queued until both final files exist.

### 6.2 Duration and time

Durations come from sample counts:

- `audio_duration_ms` covers all samples in the WAV;
- `pre_roll_duration_ms` is the actual prefix spliced from the prebuffer; and
- `recording_duration_ms = audio_duration_ms - pre_roll_duration_ms`.

This remains correct even if the wall clock is invalid.

`started_at` represents the button-triggered meeting start in UTC. Metadata
also contains `clock_synchronized`. If trustworthy UTC time is unavailable,
`started_at` is `null` and `clock_synchronized` is `false`; the device never
labels an epoch-derived value as a valid 1970 meeting time.

The existing missing-`ntpd` path is replaced with an in-process or otherwise
verified time-sync implementation before timestamps are marked synchronized.

### 6.3 Database states

The durable upload state becomes:

```text
recording
finalized
awaiting_decision
local_only
queued
uploading
retry_wait
uploaded
configuration_error
missing_artifact
legacy_unsupported
```

State transitions:

```text
recording → finalized
finalized → queued               automatic
finalized → awaiting_decision    ask
finalized → local_only           local_only
awaiting_decision → queued       Send
awaiting_decision → local_only   Save
awaiting_decision → deleted      Discard
queued → uploading
uploading → uploaded
uploading → retry_wait           transient failure
uploading → configuration_error  permanent request/config failure
retry_wait → queued              retry deadline or connectivity return
configuration_error → queued     relevant configuration changed/manual retry
```

Upload attempt counters increment only for actual HTTP attempts, not for local
state changes.

## 7. Multipart webhook contract

### 7.1 Request

Wave sends:

```http
POST <api.endpoint>
Authorization: Bearer <api.auth_token>   # omitted when empty
Idempotency-Key: <meeting UUID>
X-Wave-Device-ID: <stable device ID>
User-Agent: waved/<software version>
Content-Type: multipart/form-data; boundary=...
```

Parts:

| Name | Content type | Filename | Content |
|---|---|---|---|
| `meeting_id` | `text/plain; charset=utf-8` | none | Meeting UUID |
| `metadata` | `application/json` | `metadata.json` | Schema below |
| `audio` | `audio/wav` | `<meeting UUID>.wav` | Finalized WAV |

The audio part streams from disk with bounded buffers. The request builder must
not call `tokio::fs::read` for the WAV.

Cross-host redirects are not followed. This prevents accidental bearer-token
disclosure and keeps webhook success semantics explicit.

### 7.2 Metadata schema

```json
{
  "schema_version": 1,
  "event": "meeting.recording.completed",
  "meeting_id": "45e1c59c-9c9d-48e0-8232-9976063e0903",
  "device": {
    "id": "PSP_0000000000000000",
    "name": "Wave Open",
    "software_version": "0.1.0"
  },
  "recording": {
    "started_at": "2026-07-17T14:30:00Z",
    "clock_synchronized": true,
    "recording_duration_ms": 3600000,
    "audio_duration_ms": 3630000,
    "pre_roll_duration_ms": 30000,
    "audio_mode": "omni"
  },
  "audio": {
    "filename": "45e1c59c-9c9d-48e0-8232-9976063e0903.wav",
    "content_type": "audio/wav",
    "codec": "pcm_s16le",
    "sample_rate_hz": 48000,
    "channels": 1,
    "bit_depth": 16,
    "size_bytes": 348480044,
    "sha256": "hex-encoded-sha256"
  },
  "bookmarks": [
    { "offset_ms": 42000 }
  ],
  "speaker_segments": [
    {
      "start_ms": 1000,
      "end_ms": 3500,
      "speaker_id": 2,
      "azimuth_deg": 137.5
    }
  ]
}
```

The stable device ID comes from the production device identifier when
available, with a documented hardware-derived fallback. It is never left
empty.

The test request uses the same multipart structure with:

- `event = "webhook.test"`;
- a new UUID/idempotency key;
- a small valid silent WAV fixture; and
- empty bookmarks and speaker segments.

### 7.3 Response classification

- Any `2xx`: success.
- Connection, DNS, TLS, timeout, `408`, `425`, `429`, and `5xx`: transient.
- Redirects and all other `4xx`: configuration/permanent failure.

Response bodies are read only up to a small diagnostic limit. Logs and device
status include the status code and a sanitized/truncated message, never
credentials.

## 8. Upload worker and retry

A dedicated upload worker owns delivery. The meeting command loop only writes
durable state and wakes the worker; it never performs the upload inline.

The worker:

1. waits for station connectivity and a configured endpoint;
2. takes one eligible row at a time in deterministic database order;
3. atomically claims it as `uploading`;
4. streams its multipart request;
5. persists success or classified failure; and
6. wakes again for queued work, retry deadlines, Wi-Fi reconnection, endpoint
   changes, or manual retry.

Transient retries use exponential backoff from `retry_backoff_ms`, capped at
`retry_max_backoff_ms`, with bounded jitter. `retry_max` is the maximum number
of automatic attempts in one failure cycle. After it is reached, the recording
remains failed and visible; Wi-Fi reconnection, a relevant configuration
change, or a manual Retry Failed action begins another cycle.

The meeting UUID is the idempotency key on every attempt. Endpoint receivers
should return the same successful result for a duplicate key.

## 9. Storage retention

Wave keeps uploaded recordings locally. The pruner runs:

- after an upload succeeds;
- after recording finalization; and
- at boot after reconciliation.

Pruning begins when either:

- filesystem usage is at or above `prune_uploaded_at_percent`; or
- available space is below `minimum_free_mb`.

It deletes the oldest `uploaded` recording by SQLite insertion order, including
its audio, metadata, bookmarks, speaker segments, and meeting row, then
rechecks both conditions.

It never deletes `recording`, `awaiting_decision`, `local_only`, `queued`,
`uploading`, `retry_wait`, `configuration_error`, or unknown artifacts.

If thresholds are still violated and no uploaded recording can be removed,
Wave blocks the next recording and shows a storage-full message. Existing
recordings remain intact.

At 48 kHz mono S16, audio consumes approximately 5.76 MB/minute or 346 MB/hour,
before filesystem overhead.

## 10. Boot reconciliation and legacy data

At boot, before the upload worker starts:

- finalized WAV + metadata with a matching row retain their state;
- finalized WAV with a missing row is imported conservatively as `local_only`;
- rows with a missing finalized artifact become `missing_artifact`;
- `.partial` recordings remain preserved and are reported as incomplete;
- legacy directories containing only `audio.opus` become
  `legacy_unsupported`; and
- no legacy or missing artifact is retried as `audio.wav`.

Reconciliation is idempotent and never deletes user data.

The current development database's stale `upload_failed` rows are migrated by
artifact inspection rather than blindly requeued.

## 11. Setup service and security

The setup HTTP service continues to bind only to `192.168.2.1:80` while
`wlan0` is the random-password WPA2 access point. Plain HTTP is acceptable
within this encrypted, physically initiated, short-lived network; no service
is exposed on the normal LAN.

Additional controls:

- each setup session gets a random CSRF token embedded in forms;
- POSTs require the session token;
- request and field sizes are bounded;
- output is HTML-escaped;
- secrets are write-only;
- saved files are `0600`;
- bearer tokens and Wi-Fi passphrases are masked from logs;
- no endpoint request occurs until station mode is restored; and
- access-point teardown and station restoration remain mandatory on every exit
  path.

## 12. Controlled restart

The setup service records that an apply/test action is pending, completes
access-point teardown, restores or provisions station Wi-Fi, and then requests
a controlled `waved` process restart. The existing watchdog starts the new
process with the persisted configuration.

The restart marker is durable enough to survive the restart but is cleared
after the new process consumes it. A failed webhook test does not roll back an
otherwise valid local configuration; its result is shown as an actionable
status.

Active recording and setup save are mutually exclusive. The device UI does not
allow setup to begin while recording, and the service rejects a save if that
invariant is violated.

## 13. Error handling

- Invalid configuration: field errors; no write.
- Persistent write failure: keep old config and AP session; show retryable
  error.
- AP/session cancellation: discard unsaved form values, restore station mode.
- Wi-Fi connection failure: preserve valid settings, show Wi-Fi failure, allow
  provisioning retry.
- Controlled restart failure: watchdog retries; marker remains pending.
- Test webhook failure: retain settings and expose sanitized reason.
- Recording finalization failure: keep `.partial`, mark incomplete, never
  upload.
- Metadata/hash failure: retain finalized audio, mark artifact error, never
  upload incomplete metadata.
- Upload process crash: rows left in `uploading` are returned to `queued` by
  boot reconciliation.
- Storage prune failure: stop pruning, log/show warning, do not touch unsent
  data.

## 14. Observability

The on-device Webhook status view shows:

- endpoint host only, not full query strings or credentials;
- whether authentication is configured;
- selected upload mode;
- last test result;
- queued, uploading, and failed counts;
- last successful delivery time when the clock is trustworthy; and
- Retry Failed action.

Logs use meeting UUIDs and state transitions. They never log audio contents,
Wi-Fi passphrases, bearer tokens, or secret form submissions.

## 15. Acceptance criteria

### Configuration

- Every supported `WavedConfig` path is rendered with the correct control.
- Existing secrets are never present in page source or responses.
- Blank secret submissions preserve the old value; explicit clear removes it.
- Invalid values show field-level errors and do not modify `config.toml`.
- Valid values survive restart and the final file is mode `0600`.
- The current invalid live configuration migrates to schema version 1 without
  losing device, volume, endpoint, token, retry, security, or NTP values.
- Saving without new Wi-Fi credentials restores a saved station profile.

### Recording

- A one-hour meeting records without unbounded memory growth.
- The finalized WAV is readable, mono, 48 kHz, PCM S16, and has duration
  matching its sample count.
- Metadata has non-zero durations, stable device ID, correct audio properties,
  bookmarks, speaker segments, and an honest clock-synchronization flag.
- No `.partial` or awaiting-decision artifact is uploaded.

### Webhook

- A test receiver observes exactly three multipart fields with the documented
  names/types.
- Upload memory remains bounded while sending a large WAV.
- Bearer and idempotency headers are correct.
- Any `2xx` marks uploaded.
- Transient failures retry with bounded backoff.
- Permanent `4xx` failures become visible configuration errors.
- Duplicate attempts reuse the same idempotency key.
- Endpoint redirects are rejected.

### Recovery and storage

- Reboot during recording preserves `.partial` and never uploads it.
- Reboot during upload safely requeues the meeting.
- Wi-Fi loss queues work and reconnect resumes it.
- Legacy Opus and missing-WAV rows stop generating retry storms.
- At configured pressure, only the oldest successfully uploaded recordings are
  pruned.
- When only unsent/local recordings remain, storage pressure blocks new
  recording without deleting them.

### Device smoke test

1. Open setup and join `Wave-XXXX` from a phone.
2. Configure device/audio/webhook/storage fields.
3. Save and reconnect using the existing Wi-Fi profile.
4. Observe controlled restart and successful `webhook.test` on the device.
5. Record a short meeting and let automatic mode deliver it.
6. Validate the receiver's WAV, metadata, headers, and idempotency key.
7. Force a transient failure and verify retry.
8. Force a permanent `400` and verify configuration-error status.
9. Re-enter setup, change the endpoint, reconnect, and verify failed work is
   eligible for retry.

## 16. Likely implementation areas

- `waved-config`: schema version, migrations, validation, atomic save, and
  field registry.
- `waved-provision`: multi-section form, session/CSRF handling, settings
  routes, final actions, apply marker, and larger bounded request bodies.
- `waved-meeting`: partial/final artifact lifecycle, complete metadata,
  database migration/state machine, reconciliation, upload worker, streaming
  multipart, retries, and pruning.
- `waved-state`: configuration/apply/test/upload status exposed to services and
  UI.
- `waved-ui`: setup/apply/test progress, Webhook status screen, upload counts,
  retry action, and storage-full errors.
- `waved-wifi`: verified time synchronization and reconnect/apply coordination.
- device scripts/tests: configuration migration, captive-form acceptance,
  mock webhook receiver, bounded-memory upload, restart recovery, and retention
  smoke tests.
