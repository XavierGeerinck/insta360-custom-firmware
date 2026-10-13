# Recording webhooks

Wave can deliver each finalized meeting to a generic HTTP endpoint. Delivery is
durable: the WAV and metadata are committed locally first, the queue survives a
restart, and the meeting UUID is reused as the idempotency key for every retry.

## Configure on the phone

1. On Wave, open the Menu (swipe down from the top of Home, or tap the status
   at the top), then **Wi-Fi**.
2. Tap **Set up Wi-Fi** (it reads **Change network** when Wave is already
   connected, and asks first) and scan the temporary `Wave-XXXX` Wi-Fi QR.
3. Open the captive setup page on the phone.
4. In **Webhook**, enter:
   - **Webhook URL** — the full receiver URL;
   - **Bearer token** — optional;
   - **After recording** — Automatic, Ask after recording, or Keep locally; and
   - retry timings.
5. For a plain `http://` development endpoint, explicitly enable **Allow
   insecure HTTP**. Use HTTPS in production.
6. Tap **Save settings and reconnect**, or save and select another Wi-Fi
   network.

The access point closes while Wave reconnects. `waved` then restarts with the
new settings and sends a small `webhook.test` request. Wave's screens don't
show its result; it is saved in `/data/waved/webhook-test.result`.
**Menu → Uploads** shows the counts of sent, waiting and failed recordings. The
bearer token is write-only: the setup page never returns the saved value.

An empty endpoint disables sending. Automatic recordings remain safely queued
until an endpoint is configured.

## Upload modes

- `automatic` (default): finalize and queue immediately; no confirmation
  screen.
- `ask`: show **Send this recording?** with Keep / Send / Delete after each
  meeting (without an endpoint: **Keep this recording?**, Keep / Delete).
- `local_only`: finalize and retain locally without queuing.

## HTTP contract

Wave sends one `POST` with `multipart/form-data`. A receiver must accept these
three parts:

| Part | Content type | Filename | Meaning |
|---|---|---|---|
| `meeting_id` | text | — | Stable meeting UUID |
| `metadata` | `application/json` | `metadata.json` | Versioned metadata below |
| `audio` | `audio/wav` | `<meeting-id>.wav` | Mono PCM S16LE, 48 kHz |

Headers:

```text
Authorization: Bearer <token>        # omitted when no token is configured
Idempotency-Key: <meeting UUID>
X-Wave-Device-ID: <stable device ID>
User-Agent: waved/<version>
```

Any 2xx response completes delivery. Network failures, 408, 429, and 5xx
responses retry with capped exponential backoff and bounded jitter. Other
responses, including redirects and authentication errors, remain visible as
configuration failures until settings change or **Retry … failed** is tapped
in **Menu → Uploads**.
Response diagnostics are bounded and control characters are removed.

Receivers should persist the request before returning 2xx and deduplicate on
`Idempotency-Key`. Wave deliberately does not follow redirects.

## Metadata version 1

Example (values abbreviated):

```json
{
  "schema_version": 1,
  "event": "meeting.recording.completed",
  "meeting_id": "21b17c4c-7390-4d52-8cbf-477fab102bc0",
  "device": {
    "id": "wave-device-id",
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
    "filename": "21b17c4c-7390-4d52-8cbf-477fab102bc0.wav",
    "content_type": "audio/wav",
    "codec": "pcm_s16le",
    "sample_rate_hz": 48000,
    "channels": 1,
    "bit_depth": 16,
    "size_bytes": 348480044,
    "sha256": "..."
  },
  "bookmarks": [
    { "offset_ms": 42000 }
  ],
  "speaker_segments": [
    {
      "start_ms": 1000,
      "end_ms": 8500,
      "speaker_id": 1,
      "azimuth_deg": 45.0
    }
  ]
}
```

Until network time is trustworthy, `started_at` is `null` and
`clock_synchronized` is `false`. Durations are sample-derived and remain
accurate.

The `webhook.test` event uses the same multipart shape with a short valid
silent WAV. Consumers should accept the event without treating it as a real
meeting.

## Local test receiver

Run this on a computer reachable from Wave:

```bash
python3 scripts/webhook_receiver.py --port 8787 --output ./webhook-inbox
```

Configure Wave with `http://<computer-lan-ip>:8787/meetings` and enable
insecure HTTP for this local test only. Each request is durably saved as a raw
`.multipart` file, which is useful for inspecting the exact contract.

To require the configured bearer token:

```bash
WAVE_WEBHOOK_TOKEN='replace-me' python3 scripts/webhook_receiver.py
```

## Local retention

Delivered recordings stay on Wave until either configured storage threshold is
crossed. Pruning selects only the oldest recording already marked uploaded.
Queued, failed, local-only, incomplete, and awaiting-decision recordings are
never pruned automatically. When storage remains constrained and no delivered
recording is safe to remove, Wave blocks new recording starts and shows
**Storage full** without deleting unsent data.
