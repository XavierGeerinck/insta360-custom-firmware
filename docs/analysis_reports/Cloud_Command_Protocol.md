# Cloud command protocol — Insta360 Wave firmware v1.2.0.6 (consumer package `v3.0.0.1`)

> Source of truth: static analysis of
> `firmwares/fw_v3.0.0.1/extracted/rootfs/usr/lib/libins_entry.so` and
> `usr/lib/libcloud_proto_https.so`. The decompiled C is at
> `firmwares/fw_v3.0.0.1/decompiled/{libins_entry.so.c, libcloud_proto_https.so.c}`.

This document describes the **device-side cloud protocol surface** — the
internal IPC commands pspapp publishes to its "cloud server" subsystem and
the HTTPS endpoints those commands ultimately hit. It exists so that a
custom firmware (`waved/`) can either emulate this surface for Wave
Controller compatibility, or stub it out for a self-hosted setup.

## Internal IPC: `CLOUD_CMD_*`

A new internal command enum (`machine_server_cmd_e` in the decompile,
referenced via `cloud_server_param_s.cloud_server_cmd`) introduced in v3.
**All 11 values are new in v3** — v1.1.6.6 has zero `CLOUD_CMD_*` symbols.

The dispatch is in `cloud_server_request()` in `libins_entry.so` (around
line 117850 of `libins_entry.so.c`). Each command is implemented by a
matching helper — `record_prj_manager_delete_speaker_mapping`,
`record_prj_manager_update_speaker_mapping`,
`upload_record_project`, `cloud_info_sync`, etc.

### Command table

Numeric values recovered from the switch-statement dispatch (Ghidra
applies a wrong enum overlay in places; values listed here are taken from
the printf strings, which are ground-truth).

| Value | Name | Action |
|------:|---|---|
| 1 | `CLOUD_CMD_UPLOAD_RECORD_PATH` | Register a record-path with the cloud uploader. |
| 2 | `CLOUD_CMD_UPLOAD_MODE` | Set upload mode to `AUTO` (constant `CLOUD_UPLOAD_MODE_AUTO`) or `MANUAL` (`CLOUD_UPLOAD_MODE_MANUAL`). Writes `*g_cloud_server_info`. |
| 3 | `CLOUD_CMD_UPLOAD_RECORD_CALL` | Trigger upload of a meeting recording: calls `upload_record_project(meeting_file_info)`. |
| 4 | `CLOUD_CMD_UPLOAD_GET_MEETING_INFO` | Look up a meeting by `meeting_id` in `g_record_upload_meeting_list`; return `meeting_local_name` + `meeting_cloud_url` + `meeting_upload_status`. |
| 5 | `CLOUD_CMD_CLOUD_INFO_SYNC` | Pull the device-token / base-URL / device-ID triple from the local DB and call `cloud_info_sync(device_token, base_url, device_id)`. |
| 6 | `CLOUD_CMD_TRIGGER_SUMMARY` | Request server-side summary for a meeting. |
| 7 | `CLOUD_CMD_GET_SPEAKER_NAME` | Query speaker name by `speaker_id` from local DB. |
| 8 | `CLOUD_CMD_UPDATE_SPEAKER_MAPPING` | Update `speaker_id → name` mapping in local DB. |
| 9 | `CLOUD_CMD_DELETE_SPEAKER_MAPPING` | Delete a single speaker mapping by `speaker_id`. |
| 10 | `CLOUD_CMD_DELETE_ALL_SPEAKER_MAPPINGS` | Truncate the speaker-mapping table. |
| 11 (inferred) | `CLOUD_CMD_UOLOAD_GET_LAST_MEETING` | Return the last entry of `g_record_upload_meeting_list` (note the typo `UOLOAD` in the source). |

The value-11 mapping is inferred: 11 named commands exist, the
"copy last meeting from `g_record_upload_meeting_list`" branch is the
only one with no matching printf label, and the `cloud_server_cmd =
CLOUD_CMD_UOLOAD_GET_LAST_MEETING` assigns at lines 23938 / 192431 are
otherwise unaccounted for. Confirm by reading the enum declaration in
the original header if it ever leaks.

### Parameter layout: `cloud_server_param_s`

The union member `field_1` is overloaded by command. Recovered fields:

```c
struct cloud_server_param_s {
    machine_server_cmd_e cloud_server_cmd;   // 1..11
    union {
        // CLOUD_CMD_UPLOAD_MODE (2)
        cloud_upload_mode_t upload_mode;     // AUTO or MANUAL

        // CLOUD_CMD_UPLOAD_RECORD_CALL (3)
        meeting_file_info_t meeting_file_info;

        // CLOUD_CMD_UPLOAD_GET_MEETING_INFO (4)
        struct {
            uint16_t meeting_id;             // input
            char     meeting_local_name[128]; // output (at offset +2)
            char     meeting_cloud_url[512];  // output (at offset +0x82)
            uint8_t  meeting_upload_status;
        } last_upload_meeting;

        // CLOUD_CMD_GET_SPEAKER_NAME (7),
        // CLOUD_CMD_UPDATE_SPEAKER_MAPPING (8),
        // CLOUD_CMD_DELETE_SPEAKER_MAPPING (9)
        struct {
            int speaker_id;
            // For UPDATE: speaker name follows at field_1+4 (NUL-terminated)
            char speaker_name[127];          // present for UPDATE only
        } speaker_query;
    } field_1;
};
```

(Sizes inferred from `strncpy(..., 0x1ff)` for the URL field and
`strncpy(..., 0x7f)` for the name field; total `field_1` is 0x288 bytes.)

### Speaker-mapping local data store

The speaker-mapping CRUD is fully **local** — backed by
`record_prj_manager_t * g_db_manager` and methods
`record_prj_manager_{delete_all_speaker_mappings,
delete_speaker_mapping, update_speaker_mapping}`. The cloud is never
required for these operations to succeed.

This is the easiest piece for `waved/` to adopt verbatim — it's just a
key-value table (`int → string`) and three method calls. Suggested
location: a new table in the existing `meetings.db` schema.

## On-the-wire: HTTPS endpoints

Endpoints visible in `libcloud_proto_https.so` (new in v3 or v3-significant):

### Device lifecycle

| Endpoint | Inferred purpose |
|---|---|
| `/device/secret/register` | First-boot device registration → returns device token. |
| `/device/secret/token/refresh` | Periodic / on-expiry token refresh. |

The response schema includes `userEndPoint` (the per-tenant endpoint URL),
`deviceToken`, and `confirmInfo` — visible from log strings:

```
[CLOUD_PROTO] [INFO] Endpoint: %s
[CLOUD_PROTO] [INFO] User endpoint found: %s
[CLOUD_PROTO] [INFO] Endpoint buffer too small: need %zu, have %zu
[CLOUD_PROTO] [INFO] Invalid response format: missing endpoint
[CLOUD_PROTO] [INFO] Invalid response format: missing deviceToken
[CLOUD_PROTO] [INFO] Device Token: %s
[CLOUD_PROTO] [INFO] Device token: %s
[CLOUD_PROTO] [INFO] Device token buffer too small: need %zu, have %zu
[CLOUD_PROTO] [INFO] Warning: confirmInfo is present but userEndPoint is missing
```

### Wave Controller binding flow

Used for the new QR-code / mobile-app binding:

| Endpoint | Inferred purpose |
|---|---|
| `/wave/binding/task/create` | Device starts a binding task. |
| `/wave/binding/task/confirm` | Mobile app confirms binding. |
| `/wave/binding/task/cancel` | Either side cancels. |
| `/wave/binding/task/status` | Poll binding state. |

### Meeting / recording lifecycle

| Endpoint | Inferred purpose |
|---|---|
| `/device/abnormalMeeting/list` | Device reports list of meetings that ended abnormally. |
| `/device/abnormalMeeting/reportComplete` | Device acknowledges abnormal-meeting report. |
| `/device/summary/asr/task` | Submit / poll an ASR transcription task. |
| `/device/summary/asr/startSummaryWithTranscription` | Trigger server-side summary that requires a transcript. |
| `/device/summary/qrcode` | Get a QR code to share / view the summary. |

### Bypass / opt-out

- File `/tmp/send_no_transcription.txt` — if present, the current recording
  is uploaded *without* cloud transcription. Used by the
  `MQTTCLI_CMD_REPORT_NO_TRANSCRIPTION` MQTT command (see
  [MQTT_Topic_Map.md](MQTT_Topic_Map.md)).

## What `waved/` needs to do for Wave Controller compatibility

If we want a Wave Controller (mobile-app) build talking to an open-firmware
device to "just work", `waved/` needs to expose **at minimum**:

1. **MQTT bridge** that registers the device on a known broker and
   subscribes to `webshell/mqtt/device/cmd/<device_id>` (subscribe topic
   currently inferred; see [MQTT_Topic_Map.md](MQTT_Topic_Map.md)).
   Translate inbound `MQTTCLI_CONTROL_CMD` payloads into local actions.
2. **HTTPS endpoint** at the URL the Wave Controller is configured to
   reach, implementing at least:
   - `/device/secret/register` → return `{deviceToken, userEndPoint}`.
   - `/device/secret/token/refresh` → return new token.
   - `/wave/binding/task/{create,confirm,cancel,status}` for the
     QR-binding flow.
   - `/device/abnormalMeeting/list` → return `[]` (we don't need this
     surveillance feature).
3. **Mirror** the publish topics `webshell/mqtt/device/{online,offline,heartbeat}/<id>`
   so the controller's "is the device online?" check passes.

If we want a Wave Controller talking to *waved* with **no cloud at all**:

- Run a local Mosquitto on the device, listening on the LAN.
- Run a self-hosted HTTPS endpoint that satisfies the binding/registration
  flow, ideally inside `waved/`.
- Replace cloud-required features (summary, transcription) with either
  local equivalents or "feature unavailable" responses.

## What to skip (anti-feature surface)

- `/device/summary/asr/*` — only meaningful with a cloud transcription
  service. Open firmware doesn't need to emulate these; just return 4xx
  and the UI degrades to "transcription unavailable".
- `/device/summary/qrcode` — same.
- `/device/abnormalMeeting/list` / `reportComplete` — telemetry. Return
  an empty list.
- `ins_lic_tool` / license enforcement — the whole point of the open
  firmware is no licensing.

## Next steps to close gaps

1. Decompile `webshell_cli` (not currently in the 6-binary set) and read
   `mosquitto_subscribe` call sites to learn the inbound topic format —
   that closes the only open question on the IPC layout.
2. Capture one live HTTPS round-trip per endpoint with `mitmproxy` against
   a v3 device (TLS pinning may complicate this; check
   `CURLOPT_SSL_VERIFYPEER` call sites in `libcloud_proto_https.so` first).
3. Cross-reference each `CLOUD_CMD_*` with the HTTPS endpoint it
   ultimately calls — should be visible by following the helper
   functions (`upload_record_project`, `cloud_info_sync`, etc.) into
   `libcloud_proto_https.so`.
