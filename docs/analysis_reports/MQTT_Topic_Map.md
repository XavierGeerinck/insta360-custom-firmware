# MQTT topic map — Insta360 Wave firmware v1.2.0.6 (consumer package `v3.0.0.1`)

> Source of truth: static analysis of
> `firmwares/fw_v3.0.0.1/extracted/rootfs/` —
> `usr/bin/webshell_cli`, `usr/lib/libins_entry.so`, `usr/lib/libui.so`,
> `etc/mosquitto/mosquitto.conf`. **Not yet confirmed by live capture.**

## Component layout

The on-device MQTT topology has three parts:

```
┌──────────────────┐   loopback?    ┌──────────────────┐  TCP 1883/8883  ┌──────────────────┐
│  pspapp + libui  │ ───events────▶ │   webshell_cli   │ ──publish/sub──▶│  remote broker   │
│  (event source)  │ ◀──commands─── │  (MQTT bridge)   │ ◀───────────────│  (cloud-hosted)  │
└──────────────────┘                └──────────────────┘                 └──────────────────┘
                                            │
                                            ▼
                                    /usr/sbin/mosquitto
                                    (local broker —
                                     stock default cfg,
                                     possibly unused
                                     in production)
```

- `pspapp` and `libui.so` reference `MQTTCLI_*` symbolic event names but
  **do not link `libmosquitto.so`** — they hand events to some other process
  (most likely `webshell_cli`) via a non-MQTT IPC mechanism (signals, UNIX
  socket, or shared memory).
- `webshell_cli` is the **only** rootfs binary outside the broker / CLI tools
  that links `libmosquitto.so`. It is the bridge to the cloud.
- The locally-shipped `/usr/sbin/mosquitto` broker has the **stock upstream
  default config** (`/etc/mosquitto/mosquitto.conf`) — no listener overrides,
  no auth, no persistence. It may not even be started in production; needs
  confirmation by checking `S60pspapp_daemon` and process list on a live
  device.

## Cloud broker connection

Format strings in `webshell_cli`:

```
mqtt broker %s, port: %d, clientId: %s
try connect to mqtt broker %s ....
mqtt client wait connect to mqtt broker %s:%d
port error, must 1883 ; or 8883
```

- **Host:** dynamic, from runtime config (not in binary). Likely resolved
  via the cloud endpoint-discovery handshake — see
  [Cloud_Command_Protocol.md](Cloud_Command_Protocol.md).
- **Port:** restricted to `1883` (plain MQTT) or `8883` (MQTT over TLS) —
  hard validation in `webshell_cli`.
- **Client ID:** dynamic, from device identity.
- **Keep-alive / will:** `mosquitto_will_set` is called; will-topic content
  not in static strings.

## Publish topics — confirmed in `webshell_cli`

| Topic | When | Payload (inferred) |
|---|---|---|
| `webshell/mqtt/device/online/<device_id>` | On MQTT connect | Device-presence beacon |
| `webshell/mqtt/device/heartbeat/<device_id>` | Periodic | Heartbeat (interval not in strings) |
| `webshell/mqtt/device/offline/<device_id>` | Last-will / on disconnect | Device-offline notification |

Surrounding format strings in `webshell_cli`:

```
client publish online msg error, %d
client publish online msg success, %s
client publish online heartbeat msg error, %d
client publish online heartbeat msg success, %s
```

## Subscribe topics — **NOT yet resolved**

`webshell_cli` calls `mosquitto_subscribe_callback_set` and prints
`subscribed (mid: %d): %d` and `mqtt client msg topic: %s, payloadlen: %d`,
but the **subscribed topic strings are not present as static literals** —
they're likely built at runtime from device-ID format strings or pulled
from the cloud endpoint-discovery response.

A reasonable guess (by symmetry with the publish topics) is:

- `webshell/mqtt/device/cmd/<device_id>` — inbound commands
- `webshell/mqtt/device/event/<device_id>` — inbound events

**To confirm:** decompile `webshell_cli` (currently not in our 6-binary
decompile set) and look at the `mosquitto_subscribe` call site, or run
`mosquitto_sub -t '#' -v` against the local broker on a live device while
the daemon runs.

## `MQTTCLI_*` event-name catalogue

These symbolic names appear in `libins_entry.so` and `libui.so`. They are
**not raw MQTT topics** — they are internal event keys passed through
pspapp / libui's event bus, and translated to MQTT topics + payloads by
`webshell_cli`. The mapping from `MQTTCLI_*` → on-the-wire topic name is
not yet recovered.

### Command-channel (device receives)

| Event key | Inferred purpose |
|---|---|
| `MQTTCLI_CONTROL_CMD` | Generic control envelope. |
| `MQTTCLI_CMD_INVALID` | Sentinel / unknown command. |
| `MQTTCLI_CMD_REPORT_NO_TRANSCRIPTION` | Mark a recording as "do not transcribe" (companion to the `/tmp/send_no_transcription.txt` flag). |
| `MQTTCLI_CMD_SYNC_PENGCHENG_SERVER_TIME` | Server-side time sync push (Pengcheng = Tencent's South China cluster — gives a strong hint at the broker location). |
| `MQTTCLI_CMD_UPLOAD_RECORD_FILE` | Trigger upload of a specific recording. |

### Status-channel (device publishes)

| Event key | Inferred purpose |
|---|---|
| `MQTTCLI_DEV_ACTIVE_STATUS` | Device activation state. |
| `MQTTCLI_DEV_ACTIVE_ADDRESS` | URL or QR-code payload for activation. |
| `MQTTCLI_ISFIRST_ACTIVE` | First-time-activation flag. |
| `MQTTCLI_UNBAND_RESULT` | Result of unbinding (typo: "UNBAND" for "UNBIND"). |
| `MQTTCLI_DAILY_REBOOT_STATUS` | Whether daily-reboot is enabled. |
| `MQTTCLI_DAILY_REBOOT_TIME` | Scheduled daily-reboot time. |
| `MQTTCLI_MEETING_COUNTDOWN_TIME` | Meeting end-countdown timer. |
| `MQTTCLI_RECORD_STATUS` | Recording state machine. |
| `MQTTCLI_RECORD_SHARE_URL` | Share URL for a completed recording. |
| `MQTTCLI_VIEWER` | "Viewer" presence / viewer count. |
| `MQTTCLI_ERROR_EN_MSG` / `MQTTCLI_ERROR_ZH_MSG` | Error messages, English / Chinese. |

### Server-discovery envelope

| Event key | Inferred purpose |
|---|---|
| `MQTTCLI_SERVER_BASE` | Base URL of the cloud HTTPS API. |
| `MQTTCLI_SERVER_ADDRESS` | MQTT broker address (host:port). |
| `MQTTCLI_SERVER_STATE` | Server-reachability state. |

## `libui.so` event-handler entry points

In `libui.so` decompiled output, these function names indicate the UI
subscribes to / reacts to the corresponding MQTT events:

- `mqttcli_airecord_notes_err_cn_event_cb`
- `mqttcli_airecord_notes_err_en_event_cb`
- `mqttcli_airecord_status_event_cb`
- `mqttcli_device_bound_state_event_cb`
- `mqttcli_device_unbound_result_event_cb`

(Visible via `grep mqttcli_ libui.so.c`; full list available there.)

## Open questions

1. **Are the local broker and the cloud-bridge broker the same connection?**
   The naming `webshell/mqtt/device/...` looks like an internal management
   namespace, but the topology only makes sense if `webshell_cli` is
   actually talking to a *cloud* broker — the local mosquitto would not
   need device-presence beacons.
2. **What's the actual `MQTTCLI_*` → topic-string mapping?** Currently
   only inferable by reading `webshell_cli`'s decompile (not yet done) or
   by sniffing live traffic.
3. **Does the local mosquitto broker even start?** Need to inspect
   `S60pspapp_daemon` and `ps` output on a live device.
4. **What's the auth scheme on the cloud broker?** TLS pinning? Per-device
   client certs? Username/password derived from the device token?
   Inspecting the `mosquitto_tls_*` and `mosquitto_username_pw_set` call
   sites in the `webshell_cli` decompile would settle it.

## Next steps to close gaps

1. Run `scripts/decompile_version.sh` after adding `usr/bin/webshell_cli`
   to the `BINARIES=( … )` array in the script, then grep the resulting
   `webshell_cli.c` for `mosquitto_subscribe`, `mosquitto_publish`, and
   `mosquitto_tls_set` call sites.
2. On a live v3 device:
   - `ps | grep -E 'mosquitto|webshell'` — confirm what's running.
   - `mosquitto_sub -t '#' -v` against `localhost:1883` — see if the
     local broker has any live traffic.
   - `tcpdump -i wlan0 -w cap.pcap port 1883 or port 8883` — capture the
     cloud-broker connection.
3. Document the result back into this file.
