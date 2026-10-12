# Wave Controller protocol ("PC Link") — vendor HID report 0x03

The official **Wave Controller** desktop app (Mac/Windows, 1.1.0) talks to the
Wave over the USB HID interface, vendor report **0x03**. Stock calls this
**PC Link**. This document describes it as stock firmware **1.2.2.6** implements
the device side, so `waved` can answer the same app (decision D9).

Every fact carries a tag:

- **[code]** read from the stock binary: decompiled code, DWARF types, or data
  tables read out of the ELF.
- **[inferred]** our reading of intent, not proven by the code alone.
- **[waved]** what waved does where it deviates or where stock gives no answer.

Nothing here has been checked against a capture of the real app yet (TODO J5).

## Sources

| What | Where |
|------|-------|
| Decompiled device code | `firmwares/fw_v1.2.2.6/decompiled/libins_entry.so.c` (Ghidra; not in git, it lives in the main checkout) |
| Types, enums, struct layouts, bitfield positions | DWARF of `firmwares/fw_v1.2.2.6/extracted/rootfs/usr/lib/libins_entry.so` (`xcrun llvm-dwarfdump --debug-info`; the library is not stripped) |
| Command handler table | `g_pc_link_slave_cmd_handle_array` at ELF `0x282148`, 62 × `{u32 cmd, fn*}`; the handler pointers are `R_AARCH64_ABS64` relocations against the symbols listed below |
| MAVLink message table | `mavlink_message_crcs.2` (`0x231098`) and `.10` (`0x1e37f8`), identical, 24 entries |
| Source files (from assert strings) | `src/client/pclink/pc_protocol/{pc_link.c,pc_mavlink.c,pc_link_aes.c}`, `src/client/pclink/pclink_client.c`, `src/server/hid/{hid_protocol.c,hid_server.c}`, `app_root/hid_manager_server.c` |
| Device UI side of the PIN check | `firmwares/fw_v1.2.2.6/decompiled/libui.so.c` (`device_bind_password_check_cb`, `gui_dev_bind_password_check_result`) |

Ghidra's addresses are the ELF addresses + `0x100000`
(`pc_link_slave_recv_pkt` is ELF `0x89638`, "Function at 0x00189638").

## Layers

```
 Wave Controller app
        │  USB HID, interrupt IN/OUT, report ID 0x03, 64 bytes incl. the ID
        ▼
 /dev/hidg0 ── hid_manager_server (stock bridge, ZMQ tcp 5533/5544) ──┐
                                                                       ▼
 1. vendor transport   hid_protocol.c   message type, 64-bit send id, ≤52-byte chunks,
                                          resend / sync / reset, short heartbeat 0xEC/0xED→0xEF
 2. MAVLink v2         pc_mavlink.c     msgid 1000 PC_CMD_PKG (128 B), msgid 0 HEARTBEAT_CONTROL
 3. PC-link packet     pc_link.c        16-byte header + 112-byte payload, session token, AES flag
 4. commands           pc_link.c + pclink_client.c   62 handlers, one ack per command
```

The stock device process (`pspapp`) never opens `/dev/hidg0` for PC Link; the
separate `hid_manager_server` copies 64-byte reports between `/dev/hidg0` and
ZMQ (PUB `tcp://*:5544`, SUB `tcp://127.0.0.1:5533`), each wrapped in a 68-byte
frame `FF FE 00 <len> <64 bytes>` **[code]**. The frame wrapper is internal to
stock; waved reads and writes `/dev/hidg0` directly **[waved]**.

## 1. USB: report 0x03

The 1.2.2.6 gadget (`/etc/init.d/S89usbgadget`, "Vendor+Telphone+Consumer",
`report_length 64`) declares **[code]**:

```
06 00 FF  09 01  A1 01  85 03            Usage Page 0xFF00, collection, Report ID 3
09 01 15 00 26 FF 00 95 3F 75 08 81 02   Input:  63 bytes
09 01 15 00 26 FF 00 95 3F 75 08 91 22   Output: 63 bytes
C0
```

So a vendor report is **64 bytes: `0x03` + 63 data bytes**, in both directions.
A read of `/dev/hidg0` returns the report ID as byte 0 (the existing LED reader
relies on that for reports 5–7). Stock rejects anything whose length is not 64 or
whose byte 0 is not 3 (`change_carray_to_hid_vendor_package`) **[code]**.

## 2. Vendor transport (`hid_protocol.c`)

### Report layout

All transport messages share one layout **[code]** (`change_hid_vendor_package_to_carry`,
`change_carray_to_hid_vendor_package`; struct `_hid_vendor_package`):

| Byte | Field | Notes |
|------|-------|-------|
| 0 | report ID | `0x03` |
| 1 | message type | table below |
| 2–9 | send id | `u64` little-endian |
| 10 | payload length | ≤ 52 |
| 11–62 | payload | 52 bytes; unused bytes are 0 |
| 63 | — | never written, 0 |

The short-heartbeat messages (0xEC/0xED/0xEF) use a different layout, see below.

### Message types (`enum HID_MSG_TYPE`) **[code]**

| Value | Name | Direction | Meaning |
|-------|------|-----------|---------|
| 0x00 | MAVLINK | both | A ≤52-byte chunk of the MAVLink byte stream |
| 0x01 | RESEND | both | "Send me chunk *id* again"; payload length 1, payload 0 |
| 0x02 | RESEND_RESP | both | The re-sent chunk: a copy of the original with type 2 |
| 0x03 | RESET | host → device | Restart the transport; send id 0, length 1, payload `01` |
| 0x04 | RESET_RESP | device → host | Send id **0**, length 1, payload `01` |
| 0x05 | SYNC | both | "My last chunk was *id*"; length 1, payload 0 |
| 0x06 | SYNC_RESP | both | Echo of the SYNC id; length 1, payload 0 |
| 0xA1–0xA5 | CASCADE_* | — | Wave + "VB" (second unit) cascade; ignored by waved |
| 0xEC | SHORT_HEARTBEAT_REQ | host → device | Heartbeat request |
| 0xED | SHORT_HEARTBEAT_NEW_REQ | host → device | Same handling as 0xEC |
| 0xEF | SHORT_HEARTBEAT_RESP | device → host | Short status |
| 0xF0 | SWT_FACTORY | host → device | Factory / USB-mode switch; ignored by waved |

Example frames (all 64 bytes, trailing zeros omitted):

| Frame | Bytes |
|-------|-------|
| RESET_RESP | `03 04 00 00 00 00 00 00 00 00 01 01` |
| RESEND of id 7 | `03 01 07 00 00 00 00 00 00 00 01 00` |
| SYNC of id 300 | `03 05 2C 01 00 00 00 00 00 00 01 00` |

### Sending (device → host) **[code]**

- Each MAVLink frame is cut into chunks of at most **52 bytes**; each chunk is
  one MAVLINK report with the next **send id** (a `u64` counter, incremented
  before use, so the first id is **1**). The counter is not reset by RESET
  (`send_mavlink_message_to_hid`, `g_final_send_mavlink_id`).
- Every sent MAVLINK chunk is kept in a pool of the last **1200** for
  retransmission (`save_hid_vendor_package_to_already_send_pool`).
- On **RESEND(id)** from the host: if the pool holds *id*, a RESEND_RESP copy
  goes to the **front** of the send queue; the same id is re-sent at most once
  per **30 ms**. Unknown ids are logged and ignored.
- **SYNC**: when the send queue is empty, there was traffic before, and nothing
  was sent or received for **≥ 300 ms**, and the last sent id differs from the
  last id the host confirmed with SYNC_RESP, the device puts SYNC(last sent id)
  at the front of the queue. It repeats every ≥ 300 ms of silence until the
  matching SYNC_RESP arrives (`get_next_send_hid_message`).
- RESET_RESP, RESEND_RESP and SYNC_RESP jump the queue; MAVLINK chunks keep
  their order; the queue holds 400 entries and drops the oldest when full.

### Receiving (host → device) **[code]**

- Chunks are delivered to the MAVLink parser **in send-id order**
  (`get_recv_not_used_package_with_lose_pack`, `handle_recv_mavlink_package`).
  Delivery stops at the first missing id until it arrives or is given up.
- A chunk whose id is already held, or lower than the last one held, is dropped.
- The first chunk after an empty state with **1 < id < 20** marks ids
  `1..id-1` as missing; any later jump marks the ids in between as missing
  (`append_hid_vendor_package_to_recv_list`). A jump of more than **400** ids
  drops everything and restarts from the new chunk.
- Missing ids are requested with RESEND, only when the send queue is empty, at
  most every **100 ms**, at most **10** ids per round: the first request at
  once, later ones **≥ 61 ms** apart, **21** requests per id at most, then the id
  is given up and skipped (`resend_lose_package_request_from_lose_list`).
- A RESEND_RESP fills its id exactly like a MAVLINK chunk.
- **SYNC(id)**: if *id* was not received, it (and so every id up to it) is
  marked missing; a SYNC_RESP(id) always goes to the front of the queue.
- **SYNC_RESP(id)** records *id* as the host's confirmation of our sends.
- **RESET**: clears the receive state, the send queue and the missing list,
  queues RESET_RESP, resets the MAVLink parser **and** the MAVLink send
  sequence (`pc_mavlink_snd_clear` zeroes channel 1), and forgets the last
  confirmed SYNC. It does **not** reset the send-id counter, the sent pool, or
  the PC-link session token.

### Short heartbeat **[code]**

The host polls with a 64-byte report `03 EC 00 … 00` or `03 ED 00 … 00`
(only bytes 0–1 are looked at, in `hid_transfer_message_handler`). The device
answers at once with

```
03 EF 3A <58 bytes pclink_heartbeat_short_status> 00 00 00
```

(`transfor_short_heartbeat_to_carray`; 0x3A = 58). Heartbeat requests are
handled outside the transport: no send id, no RESET needed first.

After the first heartbeat request, the device rebuilds the short status every
**100 ms** and pushes an unsolicited 0xEF when any compared field changed, or
at least every **1000 ms** otherwise (`thread_of_pclink_client_hearbeat_check_task`,
`is_difference_of_heartbeat_status_info`). Stock stops pushing only when the USB
cable is removed. **[waved]** also stops when no heartbeat request came for
30 s, so a closed app does not leave a 1 Hz writer behind.

The stock MAVLink heartbeat (msgid 0, see below) is never sent:
`pc_link_slave_heartbeat_task_init` has no caller, and its sender would reject
the 318-byte status anyway (`pc_mavlink_snd_heartbeat` only accepts ≤ 32
bytes) **[code]**. The 0xEC/0xED → 0xEF exchange is the real liveness check
**[inferred]**.

#### `pclink_heartbeat_short_status` (58 bytes, packed) **[code]**

| Off | Size | Field | Stock source / waved value |
|-----|------|-------|----------------------------|
| 0x00 | 1 | audio_voice_db | mic pickup level (`AUDIO_MIC_PICKUP_LEVEL`) / waved: 0 |
| 0x01 | 1 | dev_bind_status | bind flag / waved: 0 |
| 0x02 | 1 | puc_status | camera module state / waved: 0 |
| 0x03 | 2 | upload_meeting_id | cloud / waved: 0 |
| 0x05 | 1 | upload_meeting_status | cloud / waved: 0 |
| 0x06 | 1 | meet_status | `MEETING_SERVER_RECORD_STATE`, see below |
| 0x07 | 4 | meet_long | recording duration, seconds |
| 0x0B | 1 | meet_mode | record type (0 record, 1 transcript only) |
| 0x0C | 1 | meet_type | always 1 |
| 0x0D | 1 | battery | charging state (`MCU_BATTERY_IN_CHARGING`) |
| 0x0E | 1 | battery_level | percent |
| 0x0F | 1 | ota_upgrade_status | 0 idle, 1 running, 2 done, 3 error / waved: 0 |
| 0x10 | 1 | ota_error_code | / waved: 0 |
| 0x11 | 1 | ota_upgrade_percent | / waved: 0 |
| 0x12 | 1 | wifi_dev_rssi | see Wi-Fi status |
| 0x13 | 1 | wifi_dev_encry_mode | 0 or 2 |
| 0x14 | 1 | wifi_dev_status | see Wi-Fi status codes |
| 0x15 | 1 | wifi_dev_switch | Wi-Fi on |
| 0x16 | 1 | audio_speaker_volume | 0 when the speaker is muted |
| 0x17 | 1 | audio_mic_volume | 0 when the mic is muted |
| 0x18 | 1 | audio_eq_mode | never filled by stock (0) |
| 0x19 | 1 | audio_mic_mode | mic mode 1–6, see J4 |
| 0x1A | 1 | audio_source_mic_mute | |
| 0x1B | 1 | audio_source_speaker_mute | |
| 0x1C | 1 | audio_uac_input | `AUDIO_MIC_IN_WORK` |
| 0x1D | 1 | time_NTP_sync_flag | |
| 0x1E | 1 | time_show_mode_flag | 1 = 24 h **[inferred]** |
| 0x1F | 1 | user_improv_plan | / waved: 0 |
| 0x20 | 1 | puc_mode | / waved: 0 |
| 0x21 | 1 | puc_privacy_enable | / waved: 0 |
| 0x22 | 1 | puc_exy_privacy_enable | / waved: 0 |
| 0x23 | 2 | puc_zoom_val | / waved: 0 |
| 0x25 | 7 | puc_dev_uuid | / waved: 0 |
| 0x2C | 1 | dev_passwd_status | 1 when a PIN is set and enabled |
| 0x2D | 1 | bits 0–5 audio_reserved (bit 0 = `GUI_DISABLE_AUDIO_MODE`), bits 6–7 audio_denoise | waved: 0 |
| 0x2E | 1 | now_wifi_set_ui | always 0 |
| 0x2F | 4 | storage_total_space | |
| 0x33 | 4 | storage_used_space | |
| 0x37 | 1 | system_setting_crc | CRC-8 of `language_code[4] ‖ country[64] ‖ timezone[64]` |
| 0x38 | 1 | wifi_setting_crc | CRC-8 of `ssid[64] ‖ mac[32] ‖ ipv4[32]` |
| 0x39 | 1 | bit 0 meet_switch | `GUI_AIRECORD_FEATURE_EXIST` |

Multi-byte fields are little-endian. The two CRCs match the first 132 bytes of
the GET_SYSTEM_BASE_INFO ack and the first 128 bytes of the GET_WIFI_BASE_INFO
ack, so the app can tell when to re-read them **[inferred]**. Storage units are
not visible in the code **[inferred: MiB]**.

**CRC-8** (`calc_crc8`, `crc8_calc`): polynomial 0x07, init 0, no reflection,
no final XOR (CRC-8/SMBUS; `"123456789"` → `0xF4`) **[code]**.

## 3. MAVLink v2 (`pc_mavlink.c`)

Same framing and CRC as the MCU link (`docs/MCU_PROTOCOL.md`): `FD len incompat
compat seq sysid compid msgid[3] payload crc[2]`, CRC-16/MCRF4XX over everything
after `FD` plus `crc_extra`, trailing payload zeros trimmed (at least one byte
kept) **[code]**. The stock library carries one 24-entry message table for both
links **[code]**:

| msgid | len | crc_extra | Use |
|-------|-----|-----------|-----|
| 0 | 32 | 0x45 | HEARTBEAT_CONTROL (MCU heartbeat; PC: see above) |
| 1 | 32 | 0x20 | MCU heartbeat reply |
| 100–103 | 36/36/140/68 | 0x27/0x3A/0xA9/0x15 | MCU control / notify / OTA |
| **1000** | **128** | **0xF8** | **PC_CMD_PKG** — every PC-link packet |
| 2000–2016 | 4…234 | (see `waved-mavlink`) | camera module ("PUC") |

PC Link uses only msgid 1000 (and 0, which the device ignores) **[code]**:

- **sysid 0** both ways.
- **compid 0**: command channel (msgid 1000) or heartbeat (msgid 0).
  **compid 1**: "out-of-band" channel (`pc_link_slave_recv_ob_pkt_handle`),
  any msgid, only used by the file-transfer resume commands 0x70/0x71.
- `seq` is the per-channel transmit counter (channel 1 for every PC-link send),
  starting at 0 and reset by a transport RESET.
- incompat/compat flags 0; the stock parser would accept signed frames, but
  nothing signs.

## 4. PC-link packet (`pc_link.c`)

Every msgid-1000 payload is one 128-byte packet **[code]** (`pc_link_pkt_t`):

| Byte | Field | Notes |
|------|-------|-------|
| 0–3 | sessionToken | `i32` LE; see Session |
| 4 | protocolVer | always 1 |
| 5 | flags | bits 0–1 pktType (0 command, 1 ack); bits 2–3 ackFlag (1 in everything stock sends); bit 4 encrypted; bits 5–6 endFlag; bit 7 0 |
| 6–7 | pktCmdType | `i16` LE, the command id |
| 8–11 | pktSeq | `i32` LE; commands: per-sender counter; **acks: `FF FF FF FF`** (never set) |
| 12–15 | pktPayloadLen | `i32` LE, bytes used in *this* packet (≤ 112) |
| 16–127 | payload | 112 bytes; **unused bytes are `0xFF`** (`memset(pkt_buf, 0xff, 0x80)`) |

endFlag: **1** = last (or only) packet, **2** = first of several, **0** = middle.
A message longer than 112 bytes is split into 112-byte packets; the last one
carries the remainder (`pc_link_snd_msg_pkt`). So the flags byte of a device
ack is **0x25** (single/last), **0x45** (first), **0x05** (middle) **[code]**.

Receiving **[code]** (`pc_link_recv_msg_pkt`): payloads are appended to one
reassembly buffer (5 MiB; a first packet resets it) until a packet with
endFlag 1, then the command runs with the whole buffer. Command packets with a
`pktSeq` that is not last+1 only bump an error counter. **[waved]** caps
reassembly at 64 KiB and answers an oversized command with its "unsupported"
ack instead of storing it.

**Session token check** **[code]**: for every command id **> 0x13 except 0x80**,
the header token must equal the device's current token, otherwise the packet is
**dropped without an ack**.

**Encryption** **[code]**: when the link's `encry` flag is on, payloads of command
ids ≥ 0x11, except 0x47 and 0x80, are AES-128-**ECB** encrypted (7 blocks, no
padding, `aesEncrypt`/`aesDecrypt`) with the first 16 bytes of a key, both ways.
`encry` turns on (a) at boot when the device is bound (key = stored bind key),
(b) after a successful GET_BIND_START (key = the 16 random bytes in its ack),
(c) after BIND_DEV succeeds (key = the new bind key), (d) **as soon as an
AUTH_DEV command packet arrives** (key = the stored bind key, all zeros on an
unbound unit). The receiver ignores the header's encrypted bit and uses its own
state. **[waved]** never binds, never turns encryption on, and answers a command
whose header has the encrypted bit set with its "unsupported" ack.

Every command gets exactly one ack: same command id, pktType 1, the current
session token, and a fixed-size payload per command (table below) **[code]**.

## 5. Session, authentication and the PIN

**Token** **[code]**: starts at **`0x4748494A`** (bytes `4A 49 48 47`, "JIHG")
at link init, and goes back to it on UNAUTH_DEV (`pc_link_reset_session_token`,
which also clears `encry` and the password-verified flag).

**AUTH_DEV (0x13)** **[code]**: payload `userid[16] ‖ key[64]` (≥ 0x50 bytes).
Only on a **bound** unit whose stored user id and key match (`strcmp`) is the
result 0; then the token becomes `rand_r(time(NULL))` and goes back in the ack.
An unbound unit answers result **1** and leaves the token alone (the ack's token
field is uninitialised stack in stock). **[waved]** is never bound: result 1,
the current token in the token field, zeros elsewhere. The token check and the
reset on UNAUTH_DEV are implemented, so a token handed out by a future local
auth would work unchanged.

**VERIFY_DEV_PASSWD (0x11)** **[code]**: payload `op u8 ‖ password[32]` (≥ 33
bytes); the ack is one result byte. Stock never reads the password field:

- `op == 0` asks the **device's own screen** to check its PIN
  (`SYSTEM_PASWD_VERITY_REQ`; libui `device_bind_password_check_cb`): without a
  PIN the result becomes "verified" at once, with one the PIN page opens. Ack 0.
- `op != 0` polls: **0** while the PIN page is still open, **1** verified,
  **2** anything else (wrong PIN, cancelled, never asked).

It belongs to the binding flow ("bind password check") **[inferred]**.
**[waved]** keeps the same wire behaviour and the same three results, with one
extension for the PIN lock (D10, K2): if `op == 0` carries a non-empty
password, it is checked at once with K2's PIN check (`PinVerifier`); otherwise,
with a PIN set, waved asks for the on-device PIN prompt and reports "pending"
until it answers (and "failed" after 60 s, or at once when no prompt is wired).

**Binding** (GET_BIND_START, BIND_DEV, UNBIND_DEV, AUTH_UNBIND_DEV,
UNBIND_OFFLINE) ties the unit to an Insta360 cloud account and carries cloud
tokens **[code]**; **[waved]** answers each with its "unsupported" ack and stays
unbound.

## 6. Commands

`enum pc_link_cmd_e` **[code]**; "In table" means the id has an entry in
`g_pc_link_slave_cmd_handle_array`. Ids without an entry get **no ack** in stock
(`Invalid cmd type`), and waved does the same. "Unsupported" in the waved column
means: the stock ack size, every byte 0 except a **result byte of 1** at the
offset given (the meeting-op code 7 is stock's own "unsupported request").

Request payload sizes are the minimum stock checks; "—" means none is read.

| Id | Name | In table | Request | Ack (size, layout) | Stock behaviour | waved |
|----|------|----------|---------|--------------------|-----------------|-------|
| 0x00 | GET_DEV_INFO | ✓ | — | 933, `pc_link_get_dev_info_ack_t` | device info, see below | info (serials replaced, see §7) |
| 0x01 | AUTH_UNBIND_DEV | ✓ | exactly 512 (cloud token) | 1, result | unbind if the token matches; no ack if len ≠ 512 | unsupported (result 1), any length |
| 0x02 | UNBIND_OFFLINE | ✓ | — | 1, result (i8) | offline unbind | unsupported |
| 0x10 | GET_BIND_START | ✓ | — | 17, `bindCheckResult, randomNum[16]` | 0 + random key (turns AES on), 4 while user data is being erased | unsupported (bindCheckResult 1) |
| 0x11 | VERITY_DEV_PASSWD | ✓ | 33, `op, password[32]` | 1, result | see §5 | §5 |
| 0x12 | BIND_DEV | ✓ | 1216, `pc_link_bind_dev_cmd_t` | 1, result | bind to a cloud user | unsupported |
| 0x13 | AUTH_DEV | ✓ | 80, `userid[16], key[64]` | 85, `result, token u32, userid[16], key[64]` | §5 | result 1, current token |
| 0x14 | UNAUTH_DEV | ✓ | — | 1, result 0 | resets the token | same |
| 0x15 | UNBIND_DEV | ✓ | 1, `rm_usrdat` | 1, result | unbind (asks for the PIN) | unsupported |
| 0x16 | GET_BIND_INFO | ✓ | — | 722, `result, bindFlag, userid[16], key[64], area[128], cs_token[512]` | stored bind data (result left uninitialised) | result 0, bindFlag 0, rest 0 |
| 0x17 | WIFI_SCAN | ✓ | — | 1, result | start a scan | unsupported |
| 0x18 | WIFI_LIST | ✓ | — | 4290, `result, num, info[64]` (67-byte `ssid[64], rssi, encry, pwd_saved`) | scan results | unsupported (result 1, num 0) |
| 0x19 | WIFI_CONNECT | ✓ | 129, `ssid[64], pwd[64], encry` | 1, result | join a network | unsupported |
| 0x1A | WIFI_DISCONNECT | ✓ | 65, `ssid[64], forget` | 65, `ssid[64], result` | | unsupported (result at 64) |
| 0x1B | WIFI_GETSTATUS | ✓ | — | 133, `pc_link_wifi_status_ack_t` | station status | status (§7) |
| 0x1C | WIFI_FORGETSSID | ✓ | 64, `ssid[64]` | 1, result | | unsupported |
| 0x1D | WIFI_SWITCH | ✓ | 1 | 1, result | Wi-Fi on/off | unsupported |
| 0x1E | WIFI_AUTO_CONNECT_SWITCH | ✓ | 1 | 1, result | | unsupported |
| 0x1F | MEETING_OP | ✓ | 1, `opt` | 1, result | 0 start, 1 finish, 2 pause, 3 resume; other → 7 | §7 |
| 0x20 | MEETING_GETSTATUS | ✓ | — | 517, `result, cs_url[512], duration u32` | result 0, url `http://www.download.com/xxyyzz`, duration uninitialised | result 0, empty url, duration |
| 0x21 | MEETING_MARK | ✓ | exactly 1, `mark_type` | 1, result | 0 point, 1 task; result 0 | AddBookmark for 0 and 1, result 0 |
| 0x22 | MEETING_MODE | ✓ | 1, `mode` | 1, result | stores the record type (0 record, 1 transcript only); result 0 | 0 → result 0; other → unsupported |
| 0x23 | MEETING_GET_INFO | ✓ | 2, `meeting_id u16` | 644, `id u16, url[512], name[128], upload_status, result` | cloud upload info | unsupported (id echoed, result at 643) |
| 0x24 | MEETING_GET_CUR_ALL_INFO | ✓ | — | 801, `pc_link_get_cur_meeting_all_info_ack_t` | SN, "PSP", start time, language, meeting id, timezone, cloud device token | all zero (cloud transcription data; no result byte) |
| 0x25 | MEETING_SCREENSHOT | ✓ | exactly 260, `meet_long u32, name[256]` | 1, result | tags a screenshot to the meeting | unsupported |
| 0x26 | SET_AUDIO_SPEAKER_VOLUME | ✓ | 1 | 1, result | set volume; 0 also mutes, >0 unmutes | same |
| 0x27 | SET_AUDIO_SPEAKER_MODE | ✓ | 1 | 1, result | EQ mode | unsupported |
| 0x28 | SET_AUDIO_SPEAKER_MUTE | ✓ | 1 | 1, result | | same |
| 0x29 | SET_AUDIO_MIC_VOLUME | ✓ | 1 | 1, result | set volume; 0 also mutes, >0 unmutes | same |
| 0x2A | SET_AUDIO_MIC_MODE | ✓ | 1 | 1, result | §8 | §8 |
| 0x2B | SET_AUDIO_MIC_MUTE | ✓ | 1 | 1, result | | same |
| 0x2C | GET_AUDIO_SPEAKER_VOLUME | — | | | no ack | no ack |
| 0x2D | GET_SYS_LANGUAGE_LIST | ✓ | — | 1281, `num, name[20][64]` | language names | `1, "English"` |
| 0x2E | GET_SYS_LANGUAGE_COUNTRY_LIST | ✓ | 64, `language[64]` | 16642, `num u16, country[260][64]` | | num 0 |
| 0x2F | GET_SYS_COUNTRY_TIMEZONE_LIST | ✓ | 128, `language[64], country[64]` | 7682, `num u16, tz[120][64]` | | `1, <current time zone>` |
| 0x30 | GET_SYS_COUNTRY | ✓ | — | 65, `country[64], result` | | `"", 0` |
| 0x31 | GET_SYS_LANGUAGE | ✓ | — | 65, `language[64], result` | | `"English", 0` |
| 0x32 | GET_SYS_CITY | ✓ | — | 65, `result, timezone[64]` | timezone name | `0, <time zone>` |
| 0x33 | GET_SYS_ALL_SETTING_LIST | ✓ | — | 25606, `pc_link_get_sys_setting_list_ack_t` | the three lists | languages + time zone, result 0 |
| 0x34 | GET_SYS_DEFAULT_SETTING | ✓ | — | 193, `language[64], country[64], timezone[64], result` | current settings | `"English", "", <tz>, 0` |
| 0x35 | SET_SYS_ENABLE_UDISK_MODE | ✓ | 1 | 1, result | USB mass storage | unsupported |
| 0x36 | SET_SYS_TIME_SHOW_MODE | ✓ | 1 | 1, result | 12/24 h | SetClock24h(v == 1) **[inferred 1 = 24 h]** |
| 0x37 | SET_SYS_TIME_AUTO_SYNC | ✓ | 1 | 1, result | NTP on/off | 1 → 0 (waved always syncs); 0 → unsupported |
| 0x38 | SET_SYS_ALL_DATE_TIME_SETTING | ✓ | 9, `show, auto, year u16, mon, day, h, m, s` | 1, result | all of the above | show mode applied; auto 0 → set time (§7) |
| 0x39 | GET_SYS_TIMEZONE | — | | | no ack | no ack |
| 0x3A | SET_SYS_LANGUAGE | ✓ | 64, `language[64]` | 65, `language[64], result` | | "English" → 0; other → 1 |
| 0x3B | SET_SYS_COUNTRY | ✓ | 64 | 1, result | | unsupported |
| 0x3C | SET_SYS_TIME | ✓ | 24, `year…second` as `u32` | 1, result | set the clock | via `PcLinkHost::set_time` (§7) |
| 0x3D | SET_SYS_TIMEZONE | ✓ | 64, `timezone[64]` | 1, result | 0, or **0xFF** when the name is rejected | IANA name → SetTimeZone, 0; else 0xFF |
| 0x3E | SET_SYS_RECOVERY | ✓ | — | 1, result | **factory reset** | unsupported |
| 0x3F | SET_SYS_REBOOT | ✓ | — | 1, result | only logs and acks 0 | ack 0, nothing else |
| 0x40 | GET_DEV_STATUS | ✓ | — | 318, `pc_link_dev_status_ack_t` | §7 | §7 |
| 0x41 | ASR_START | — | | | no ack | no ack |
| 0x42 | ASR_STOP | — | | | no ack | no ack |
| 0x43 | OTA_ENABLE_UDISK | ✓ | — | 1, result | switch USB to storage for OTA | unsupported |
| 0x44 | OTA_OP | ✓ | 129, `op, package[128]` | 1, result | start an update | unsupported |
| 0x45 | OTA_GETSTATUS | ✓ | — | 4, `result, status, percent, err` | | unsupported |
| 0x46 | OTA_FINISH_SURE | ✓ | 1 | 1, result | | unsupported |
| 0x47 | FILE_TRANS | ✓ | `len u32, crc u32, index u32, data…` (never encrypted) | 1, result | streams up to 5 files into `/tmp/recvfile` | unsupported, data discarded |
| 0x48 | MEETING_PC_STATUS | ✓ | 5, `platform, status, sw_run, input_psp, output_psp` | 1, result | stores what the meeting app does | stored, result 0 |
| 0x49 | PUC_ZOOM_CONTROL | ✓ | 1 | 1, result | camera module | unsupported |
| 0x4A | PUC_POS_RESET | ✓ | 1 | 1, result | camera module | unsupported |
| 0x4B | PUC_POS_CONTROL | ✓ | 1 | 1, result | camera module | unsupported |
| 0x4C | PUC_SET_MODE | ✓ | 1 | 1, result | camera module | unsupported |
| 0x70 | OB_LAST_PKT_SEQ | compid 1 | — | 4, `last_seq i32` | file-transfer resume | `0` |
| 0x71 | OB_BAKCUP_OFFSET | compid 1 | 8, `seq u32, offset u32` | 1, result | file-transfer resume | result 1 |
| 0x80 | DEBUG | — | 2, `cmd, param` | (2, `result, param`) | a handler exists (`'G'` switches factory/user mode) but is **not in the table**, so no ack | no ack |
| 0x90 | GET_SYSTEM_BASE_INFO | ✓ | — | 133, `code[4], country[64], tz[64], result` | | `"EN", "", <tz>, 0` |
| 0x91 | GET_WIFI_BASE_INFO | ✓ | — | 129, `ssid[64], mac[32], ipv4[32], result` | | from Wi-Fi state |

Stock acks are filled into an **uninitialised stack buffer**, so stock
leaves garbage in fields its callback skips **[code]**; waved zeroes them.

The out-of-band acks (0x70/0x71) are sent on compid 1 with an **all-zero
16-byte header** and the payload at byte 16 (`pc_link_slave_recv_ob_pkt_handle`)
**[code]**.

## 7. Behaviour of the supported commands

### GET_DEV_INFO (933 bytes) **[code]**

| Off | Size | Field | Stock | waved |
|-----|------|-------|-------|-------|
| 0x000 | 16 | devSN | serial, fallback `"0000000000000000"` (no NUL) | `"0000000000000000"` |
| 0x010 | 1 | bindFlag | | 0 |
| 0x011 | 1 | rmUsrDat | "erase user data on unbind" | 0 |
| 0x012 | 16 | bindUserid | `"00000000000"` when unbound | `"00000000000"` |
| 0x022 | 512 | devToken | cloud device token | "" |
| 0x222 | 1 | sysKeyIsSetting | 1 when a PIN is set and enabled | `pin_enabled` |
| 0x223 | 1 | init_over | first-run setup done (`GUI_SYSTEM_INIT_DONE`) | `first_run_done` |
| 0x224 | 1 | wifiIsConnected | | Wi-Fi connected |
| 0x225 | 32 | fw_ver1 | SoC version | waved version |
| 0x245 | 32 | fw_ver2 | MCU version, fallback `"0.0.0.1"` | `"0.0.0.1"` |
| 0x265 | 32 | fw_ver3 | third component, fallback `"0.0.0.1"` | `"0.0.0.1"` |
| 0x285 | 32 | fw_ver4 | constant `"v1.0.0_build60"` | same |
| 0x2A5 | 31 | fw_ver5 | unused | "" |
| 0x2C4 | 1 | audio_model_type | 0 TEA, 1 SK algorithm | 1 |
| 0x2C5 | 32 | model_ver | AI model version, fallback `"0.0.0.2"` | `"0.0.0.2"` |
| 0x2E5 | 32 | dev_model_str | model name, fallback `"insta360"` | `"insta360"` |
| 0x305 | 128 | bindArea | cloud region | "" |
| 0x385 | 32 | usbId | `"PSP_"` + last 16 hex of the CPU serial | `"PSP_0000000000000000"` |

**[waved]** sends no real serial number in any field (hard rule: device serials
never leave the unit). Whether the app needs a unique id is a J5 question.

### MEETING_OP **[code]** → **[waved]**

| opt | Stock | waved |
|-----|-------|-------|
| 0 | start: 3 if OTA is running, 5 if storage is full, 2 if USB is in a storage mode, 4 if the privacy agreement was not accepted (shows it), else 0 and start | 5 if storage is blocked, 0 and `StartRecording` if idle, 0 if already recording |
| 1 | finish, 0 | `StopRecording`, 0 |
| 2 | pause, 0 | `PauseRecording`, 0 |
| 3 | resume, 0 | `ResumeRecording`, 0 |
| other | 7 | 7 |

`meet_status` (status and heartbeat) follows `MEETING_SERVER_RECORD_STATE`
**[code]**: 0 idle, 1 recording, 2 paused, 3 finished, 5 start failed,
6 cancelled, 8 pre-recording. **[waved]** sends 0, 1, 2 and 8 (pre-recording
active while idle).

### Audio **[code]** → **[waved]**

- Volumes are one byte; stock passes it straight on (0–100 **[inferred]**).
  Speaker: `SetSpeakerVolume(v)`, then mute if `v == 0` else unmute. Mic: waved
  maps the byte 1:1 onto its mic gain percent (100 = unity) and reports
  `min(mic_volume, 100)` **[inferred]**; mute follows the same 0 rule.
- Mute bytes: non-zero = muted. waved only has toggle commands, so it toggles
  when the requested state differs from the current one.
- Every audio ack is result 0 in stock (it never validates); waved answers 1
  only for a mic mode outside 1–6.

### Wi-Fi status (133 bytes) **[code]**

`ssid[64], mac[32], ipv4[32], rssi u8, encry u8, status u8, switch u8, result u8`.
`status`: the station state (`ENUM_CONNECT_STATUS`: 0 none, 1 connecting,
2 LAN ok, 3 internet ok, 4 failed, 5 scanning, 7 captive login), overridden by
the last error: 1 still connecting, 4 wrong password, 5 no AP, 6 timeout,
7 unsupported security, 8 other. `encry`: 0 open, 2 secured.
**[waved]**: status 3 when connected, else 0; encry 2 when connected; switch 1;
rssi = the dBm value as a signed byte **[inferred, J5]**; mac "".

### Device status (318 bytes, `pc_link_dev_status_ack_t`) **[code]**

Packed, little-endian. Bit numbers are LSB-first (DWARF `bit_offset` is
MSB-first: bit = 8 − offset − size).

| Off | Bits | Field | Stock source | waved |
|-----|------|-------|--------------|-------|
| 0x00 | 0–3 status, 4–7 status_err_code | | 1 on a self-check error | 0 |
| 0x01 | 0 bind, 1 init, 2 passwd, 3 storage, 4 puc, 5–6 motor level, 7 reserved | | bind flag; bit 2 = PIN set | bit 2 = `pin_enabled` |
| 0x02 | | crc_check | CRC-8 of `language_code[4] ‖ country[64] ‖ timezone[64] ‖ battery_level` | same |
| 0x03 | 0–1 puc_status, 2–7 errcode | | camera module | 0 |
| 0x04 | | puc_mode | | 0 |
| 0x05 | u16 | puc_zoom_val | | 0 |
| 0x07 | u16 | puc_privacy_enable | | 0 |
| 0x09 | u16 | puc_exy_privacy_enable | | 0 |
| 0x0B | 7 B | puc_dev_uuid | | 0 |
| 0x12 | 64 B | wifi_dev_ssid | | |
| 0x52 | 32 B | wifi_dev_mac_addr | | "" |
| 0x72 | 32 B | wifi_dev_ipv4_addr | | |
| 0x92–0x95 | | rssi, encry, status, switch | as Wi-Fi status | |
| 0x96 | 0 uac_exit, 1 uac_mix, 2 bt_exit, 3 bt_mix, 4 dongle_exit, 5 dongle_mix, **6 speaker_mute, 7 mic_mute** | | only bits 6–7 set | bits 6–7 |
| 0x97 | | audio_speaker_volume | 0 when muted | |
| 0x98 | | audio_mic_volume | 0 when muted; mic_mute set when it is 0 | |
| 0x99 | | audio_voice_db | pickup level | 0 |
| 0x9A | 0–3 eq_mode, **4–7 mic_mode** | | `AUDIO_WORK_MODE` | mic mode 1–6 |
| 0x9B | 0 uac_input, 1 bt_input, 2 dongle_input, 3–6 reserved (bit 3 = audio-mode switch disabled), **7 meet_status bit 0** | | | |
| 0x9C | **0–2 meet_status bits 1–3**, 3 asr_local, 4 asr_cloud, 5–6 meet_mode | | | |
| 0x9D | u32 | meet_long | duration, s | |
| 0xA1 | 0 meet_switch | | AI-record feature present | 0 |
| 0xA2 | | meet_type | always 1 | 1 |
| 0xA3 | 0–1 battery (charging), 2–7 errcode | | | |
| 0xA4 | | battery_level | % | |
| 0xA5–0xA7 | | bt / dongle / uac status | uninitialised | 0 |
| 0xA8 | u16 | upload_meeting_id | cloud | 0 |
| 0xAA | | upload_meeting_status | cloud | 0 |
| 0xAB | u32 | storage_total_space | | |
| 0xAF | u32 | storage_used_space | | |
| 0xB3 | | time_NTP_sync_flag | | 1 |
| 0xB4 | | time_show_mode_flag | | `clock_24h` |
| 0xB5 | 4 B | system_language_code | e.g. `EN` | `EN` |
| 0xB9 | 64 B | system_country | (stock wipes it, see below) | "" |
| 0xF9 | 64 B | system_city_timezone | | time zone |
| 0x139–0x13B | | ota status, error, percent | | 0 |
| 0x13C | | now_wifi_set_ui | 0 | 0 |
| 0x13D | | user_improv_plan | | 0 |

Stock quirk **[code]**: `user_get_current_language_code` clears 64 bytes from
offset 0xB5, so the country it wrote just before is wiped (all but the code's
own bytes), and the CRC is computed over that. waved computes the CRC over what
it sends.

### Language, region, time **[code]** → **[waved]**

Stock language names are the `language_descript` strings of
`/etc/pspapp/language_tab.json` ("English", "中文", …) with codes `EN`, `CN`,
… (`LANGUAGE_CODE_LIST`, 20 entries). waved has one language (D5): "English",
code `EN`. Time zones in stock are names from the device's own list
**[inferred: not IANA]**; waved accepts any IANA name (`SetTimeZone`) and
answers 0xFF for anything else. Setting the wall clock (0x3C, 0x38 with auto
sync off) goes through `PcLinkHost::set_time` (local time in the device's
zone); its default answers "unsupported" until waved-system can set the clock.

## 8. Mic modes (J4)

Stock `SET_AUDIO_MIC_MODE` byte **[code]** (`slave_set_mic_mode`; the SK
algorithm, `audio_model_type` 1, is what 1.2.2.6 reports):

| Byte | SK algorithm (stock) | TEA algorithm (stock) | waved mode (D2) |
|------|----------------------|-----------------------|-----------------|
| 0 | reduce mode off | `AUDIO_MODE_DEFAULT` | — (unsupported) |
| 1 | reduce mode off ("default interface") | `AUDIO_MODE_MEETTING` | **Follow talker** (`Steered`) |
| 2 | beam mode 0 omnidirectional | `LIVE_BF_OMNIDIRECTIONAL` | **Whole room** (`Omni`) |
| 3 | beam mode 1 cardioid | `LIVE_BF_CARDIOID` | **Focused** (`Cardioid`) |
| 4 | beam mode 2 super-cardioid | `LIVE_BF_SUPER_CARDIOID` | **Narrow** (`Supercardioid`) |
| 5 | beam mode 3 figure-eight | `LIVE_BF_FIGURE_EIGHT` | **Two sides** (`Figure8`) |
| 6 | beam mode 4 stereo | `LIVE_BF_STEREO` | **Stereo** |
| 7 | reduce mode on (noise reduction) | `MEETING_BF_CARDIOID` | — (unsupported) |

Reported back (`audio_mic_mode`), stock SK sends `beam mode + 2` **[code]**, so
2–6 round-trip. Mode 1 ↔ Follow talker is **[inferred]**: stock's mode 1 is its
default meeting mode, the one that follows whoever speaks. Focused, Narrow and
Two sides keep their current aim (`beam_aim_deg`); Follow talker starts from it.

## In waved

| Layer | Code |
|-------|------|
| Report routing | `crates/waved-hid/src/reader.rs`: `HidReader::spawn_with_vendor` hands report 0x03 to the PC link (LED reports 5–7 as before) |
| Vendor report, short heartbeat | `crates/waved-hid/src/pclink/vendor.rs` |
| Transport (ids, resend, sync, reset) | `pclink/transport.rs` |
| MAVLink v2 | `crates/waved-mavlink` (shared with `waved-mcu`; the stock 24-message table) |
| PC-link packet | `pclink/packet.rs` |
| Status structs, CRC-8 | `pclink/status.rs`, read from `pclink/snapshot.rs` |
| Commands, ack sizes | `pclink/cmd.rs`, `pclink/handler.rs` |
| Mic modes (J4) | `pclink/mode.rs` |
| Engine | `pclink/link.rs` (pure; time passed in) |
| Threads | `pclink/service.rs`: engine on a 20 ms tick, own writer handle on `/dev/hidg0` |

Integration points left open on purpose:

- **PIN** — `pclink::handler::PinVerifier::check(candidate, now_ms)`; wrap K2's
  `waved_config::PinAttempts::check(security, candidate, now_ms)` (Accepted →
  `Accepted`, Wrong → `Wrong`, Locked → `Locked`). `main.rs` passes
  `DenyAllPins` until then, so a PIN typed in the app is always refused.
- **On-device PIN prompt** — `pclink::service::PinPrompt`: a `()` on
  `requests` means "show K2's PIN screen", the screen answers `true`/`false`
  on `results`. `main.rs` passes `None`, so with a PIN set and no PIN in the
  request the app reads "not verified".
- **Clock** — `pclink::handler::PcLinkHost::set_time`; `NoClock` answers 1.

Tests: every layer has byte-exact tests; `pclink/link.rs` checks a stock
SET_AUDIO_MIC_MUTE request against the stock ack byte for byte. The fixtures
come from a small reference model of the stock layers written from the
decompiled code, independent of the Rust code; a real capture (J5) replaces
them.

## Open questions (for J5)

1. **Does the app work at all with an unbound unit?** GET_DEV_INFO reports
   `bindFlag 0`; the app may insist on binding (cloud account) before it shows
   any control. If so, waved needs a local, cloud-free "bind" that the app
   accepts, including AES.
2. Which commands the app sends, in what order, and whether it ever sends
   AUTH_DEV with an unbound unit (stock would switch on AES with a zero key).
3. Whether the app fills `dev_password` in VERIFY_DEV_PASSWD or always relies
   on the on-device prompt.
4. The app's heartbeat cadence and which request byte it uses (0xEC or 0xED);
   whether it sends a transport RESET when it opens the device.
5. Units: volume range (0–100?), Wi-Fi RSSI (dBm or percent), storage (MiB?),
   `time_show_mode` (1 = 24 h?).
6. What the app does with `devSN`/`usbId` now that waved sends placeholders.
7. Time zone and country names: format of the strings the app sends.
8. Whether `bindCheckResult 1`, `AUTH_DEV result 1` and the other
   "unsupported" codes give a clean message in the app.
9. The sync/resend timings were read from code paths that stock runs on a busy
   loop; whether the app's side matches them.
