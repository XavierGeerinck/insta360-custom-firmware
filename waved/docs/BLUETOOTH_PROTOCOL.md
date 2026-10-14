# Bluetooth Protocol: `/dev/ttyS1`, Telink module

The Wave's Bluetooth is a separate **Telink** chip that runs its own Bluetooth
stack (Classic and LE) plus a private 2.4 GHz link to the Insta360 USB dongle.
The SoC never speaks HCI. It drives the module over **`/dev/ttyS1` at 921600
baud, 8N1, raw**, using a small vendor command protocol, and exchanges audio
with it over **I2S on TDM-A (`hw:0,0`)**.

Sources:

- The stock 1.2.2.6 libraries, decompiled in `../firmwares/fw_v1.2.2.6/decompiled/`.
  Line numbers refer to `libins_entry.so.c`; `ui:` marks `libui.so.c`.
- `../firmwares/fw_v1.2.2.6/extracted/`: rootfs, `dtb.img`, `BT.bin`,
  `DONGLE.bin` and `sw-description`.
- The stock `/usr/bin/bttool`.
- The 3.0.0.1 `libInstaAudio.so.c`, which keeps the field names.

The 3.0.0.1 host code uses the same command bytes as 1.2.2.6. 1.0.13.6 lacks
`0x97`.

Most of this comes from the decompiled code; what was seen on the device is in
"Verified on the device (2026-10-13)" and "Acknowledgements and replies".
Every fact carries a tag:

- **[code]**: read in decompiled code.
- **[file]**: read in an extracted file (binary data, strings, the device
  tree, configuration).
- **[device]**: seen on the device (a capture or a session).
- **[inferred]**: concluded from those, but not seen directly.

The checklist for the first device session is at the end.

## In short

1. The module is a Telink SoC with its own Classic+LE host stack: A2DP sink
   (SBC), HFP hands-free (CVSD and mSBC), AVRCP, PBAP and SPP. It also has a
   2.4 GHz link to the dongle. **[file]**
2. Stock loads no firmware at boot and has no HCI or BlueZ. The module boots
   from its own flash. `BT.bin` is only written by an update or a recovery.
   **[code]**
3. The UART runs at 921600 8N1 raw, without hardware flow control in the
   device tree. **[file]**
4. Each frame is `75 45|46 LEN CMD DATA SUM`. `LEN` = 1 + the data length.
   `SUM` = the low byte of `LEN + CMD + DATA`, **without the prefix bytes**.
   **[code]**
5. The host sends requests with the `75 45` prefix. Stock acknowledges
   *every* received frame with `75 46 <same CMD>` (and `00`), except `0xE0`.
   **[code]** The module repeats its own `75 45` frames every 500 ms until
   acknowledged and answers our requests with one-shot `75 46` frames;
   waved acknowledges only the former. **[device]**
6. `0xB2` carries the connection and audio state as bits. `0xB1` carries the
   caller's number. `0xD1` reports a call going up (0) or down (1). **[code]**
7. To answer or decline, send `0xD0` with event 3 (answer) or 0 (hang up or
   decline), then `time(NULL)` as a little-endian u32, then `00`. **[code]**
8. BT audio runs on TDM-A (`hw:0,0`): 48 kHz, 2 ch, S16_LE, both directions,
   with the SoC as clock master. The same link also carries the dongle's
   audio. **[code] [file]**
9. Power is the `btpower` GPIO LED: `1` holds the module off and `0` runs it.
   Stock resets the module with 1, then 0. **[code] [file]**
10. The protocol has no paired-device list, no "connect to X" and no
    "forget". The module keeps its pairings itself. **[code] [file]**

## Hardware

| What | Fact | Source |
|------|------|--------|
| Module chip | **Telink**. The exact part can't be identified. | **[file]** `BT.bin`: a boot area with the `KNLT` magic at 0x20 (Telink's "TLNK" flag) and RISC-V code. Strings: `telink-semi.com`, `telink hands_free Service`, `<enc: telink low latency source (40ms)>` |
| Module stack | Its own HCI/LMP and host stack: A2DP sink (SBC), AVRCP (absolute volume), HFP hands-free (CVSD and mSBC), PBAP, SPP and LE pairing. A "LL sync" 2.4 GHz link goes to the dongle. | **[file]** `BT.bin` strings `a2dp_snk_*`, `#bt_CVSD_DEC`, `#bt_MSBC_DEC`, `hfp_hf_*`, `avrcp_abs_vol_get`, `PBAP obex…`, `LL_SYNC_*` |
| Module firmware | 1.2.2.6 ships `0.0.0.33`, built `2026-06-12 12:43:40` (see Firmware) | **[file]** |
| UART | `/dev/ttyS1` = `serial@fe078000`, which is UART_A on pins `uart_a_tx_x`/`uart_a_rx_x`. The device tree gives it no RTS/CTS pins. | **[file]** dtb alias `serial1`, pinctrl `a_uart0` |
| Speed | 921600 8N1 raw. Stock runs `stty -F … speed 921600`, then `-echo -icanon -icrnl -ixon -opost -onlcr -isig -iexten …` | **[file]** `bttool` device table: `/dev/ttyS1` with `0x000E1000`, and `/dev/ttyS4` with 115200. **[code]** `uart_init` 138362 |
| Power | `/sys/class/leds/btpower/brightness` (gpio-leds `btpower`, GPIO `bank@4000` line 0x31, active-low) | **[file]** dtb `leds/btpower`. **[code]** `bt_power_set` 77956 |
| Power polarity | `1` holds the module off and `0` runs it. Stock's resets go 1, sleep 1 s, then 0 and continue with commands. Its power off ends with 1. | **[code]** `bt_server_request` case 0, `bt_force_reset` 75060, ui `_device_bt_poweroff` |
| Audio link | TDM-A in I2S format, with the SoC as bit-clock and frame master. Two 16-bit slots at 48 kHz (BCLK 1.536 MHz). Out lane `tdmouta_lane0` on `tdm_d0` (SoC → module); in lane `tdmina_lane0` on `tdm_d1` (module → SoC). Codec: `aml_dummy_codec`. | **[file]** dtb `auge_sound/dai-link@0`, `tdm@0`, pinctrl |
| ALSA | `hw:0,0` playback (`pcmC0D0p`) carries audio **to** the phone or dongle. Capture (`pcmC0D0c`) carries audio **from** them. | **[code]** `libInstaAudio` `Insta360_BtOutDevice` / `Insta360_BtInDevice` ("BLUETOOTH_OUT" / "BLUETOOTH_IN") |
| Not used | The Amlogic W1/W2 Bluetooth parts (`/etc/bluetooth/aml/w*_bt_fw_uart.bin`, `aml_bt_hciattach`, `sdio_bt.ko`, `/etc/alsa_bt.conf`) are SDK leftovers. The `aml_bt` dtb node is `disabled`, the Wi-Fi chip is an ATBM6031 (WIFI.md) and no init script loads them. The rootfs has no BlueZ (`bluetoothd`, `hciattach`, `libbluetooth`). | **[file]** |

The four card-0 devices follow the dtb dai-links: 0 = TDM-A (Bluetooth),
1 = TDM-B (speaker amplifier), 2 = PDM (microphones), 3 = loopback. That
matches `AUDIO_HARDWARE.md`. **[file]**

## How stock runs the module

`pspapp` → `bt_server_init` (79471) → `bt_proto_handler_init("UART_BT")`
(77672). That opens `ttyS1` and starts two threads. **[code]**

- **`bt_msg_recv_thread`** (75187) reads the UART, parses frames, acknowledges
  each one and dispatches on the command byte.
- **`bt_heartbeat_thread`** (75087):
  1. For 10 s it calls `bluetooth_set_volume(1, speaker volume)` once a
     second (75110-75117). That is a button event on the wire (libui
     `bluetooth_set_volume` 115578 → `bt_server_request` 0xf, `BT_CMD_BUTTON_EVENT`
     → `bt_button_event` 78918): `0xD0 01 <time> <?>`. 1.1.6.6 puts the
     volume in the last byte; 1.2.2.6 leaves it out of the request, so the
     byte is whatever the stack holds. See "Button events". **[code]**
  2. `bt_info_init` (79403) sends, in order:
     - `SetName`, retried up to 10 × 1 s until `0x8C` arrives.
     - `OpenBt(at_boot=1)` if the saved switch is on, otherwise `CloseBt`.
     - `AutoReconnect(saved)`.
     - `SaveFlash`, `GetVersion`, `GetBuildTime`, `GetStatus`.
     - `GetName`, retried until `0xC6` arrives.
     - `PtaMode`: 1 when a Wi-Fi chip exists (Wi-Fi/BT coexistence;
       **[inferred]** from the name).

     It runs once per pspapp start (and after `bt_force_reset`), and checks
     nothing first: not the module's name, not a connected phone (`D8
     <saved>` goes out regardless, 79438). waved reads first and writes
     only what differs ("Start-up and the module's flash").
  3. Every 5 s it sends `Heartbeat` (`0xB3`: battery, charging, Wi-Fi),
     `GetStatus` (`0xA9`) and `PhoneMode(1)` (`0x97`). It sends `SetName` and
     `GetName` again while the stored name still contains "XXXXXX".
  4. **Liveness:** frames `0xB3` and `0xB4` refresh a timestamp. A 5 s tick
     with a timestamp 21 s old or more counts as lost.
     - After 36 lost ticks (about 3 min) stock power-cycles the module
       (`bt_force_reset`: `OTA_RESET`, btpower 1 → 0, `bt_info_init`).
     - The second time in a row, it first re-flashes `/usr/lib/BT.bin`.
       **[code]**
- **Name:** `"Insta360 Wave BT "` followed by six hex digits from
  `/proc/cpuinfo`'s `Serial` (`extract_uuid`, 74100). The module's own default
  name is `"Insta360 Wave BT XXXXXX"`. **[code] [file]**
- **Power off** (ui `_device_bt_poweroff`): stock sends `SaveFlash` every 0.5 s,
  up to 20 times, until `0xE0` comes back. It then waits 1 s and sets btpower
  to 1. **[code]**

## Framing

```
P0 P1 LEN CMD DATA[LEN-1] SUM
```

- **`P0 P1`:** `75 45` ("uE") on every request the host sends. `75 46` ("uF")
  on acknowledgements. The parser also accepts `DC CD`, which stock never
  sends. **[code]** 74018-74045, 74350 and onwards, 74902 and onwards
- **`LEN`** = 1 + the data length; it counts `CMD`. A `LEN` of 0 is invalid,
  and the parser drops back to idle. **[code]** 74005
- **`SUM`** is the low byte of `LEN + CMD + every DATA byte`. **The two prefix
  bytes are not summed.** **[code]** 74626 (`for (i = 2; …)`); the parser
  starts its sum at `LEN` (74012)
- There is no escaping and no sequence number. Data can run to 254 bytes; the
  most stock sends is 76 (an OTA packet). **[code]**
- The stock parser drops a frame with a bad sum without a word, and treats
  `75 75 45` as a broken prefix. `waved_bt::frame::Parser` retries the second
  `75` as a new start. **[code]**

## Acknowledgements and replies

- **Stock acknowledges every frame it receives except `0xE0`**, at once, with
  `75 46 <len> <cmd> …` and the same command byte:
  - no data for `0x8B 0xA4 0xAC 0xAE 0xC4 0xD1 0xD4 0xD5`;
  - one `00` byte for everything else.

  **[code]** 75277, 74888. `bttool` does the same: **[file]** its disassembly
  stores `0x4675` with `0xB4`, `0xAA`, `0xAC` and `0xA4`.
- Stock doesn't look at the prefix of what it receives, so it also
  acknowledges acknowledgements. **[code]**
- The receive dispatcher ignores frames that carry the host's own request
  bytes: `0x8B 0x9A 0x9D 0x9F 0xA3 0xA5 0xA7 0xA9 0xAB 0xAD 0xAF 0xC5 0xC7
  0xCB 0xCC 0xCD 0xCE 0xD7 0xD8 0xD9 0xDB 0xDD 0xDF 0xE2-0xE7 0xEA` (75281).
  **[code]** Most likely the module echoes each request as its own
  acknowledgement. **[inferred]**
- Stock waits for a reply only for seven commands, whose replies set an ack
  flag:

  | Command | Reply |
  |---------|-------|
  | `SetName` | `0x8C` |
  | `GetName` | `0xC6` |
  | `Button` | `0xD0` |
  | `PhoneMode` | `0x97` |
  | `GetDongleVersion` | `0xE8` |
  | `GetDongleBuildTime` | `0xE9` |
  | `PtaMode` | `0xEB` |

  It waits 100 ms (`timeout_set`, 77631) and otherwise ignores the result.
  Firmware updates use longer waits. **[code]**

**On the wire** (2026-10-13; captures `pulled-recordings/bt/s1-listen-20261013.bin`
and `s1-pair-20261013.bin`, git-ignored because module log lines name the
phone; taken with a shell that sent requests and never acknowledged
anything) **[device]**:

- **The module repeats each `75 45` frame about every 500 ms until it is
  acknowledged:** 180 identical `0xB2` in 90 s; in the second capture one
  `0xF0` log line 265 times. While it repeats one frame it sends nothing else
  from its queue: no `0xAA` or `0xC6` ever came in the second capture,
  although `GetStatus` and `GetName` were sent. **[inferred]**: one frame in
  flight at a time.
- **It answers each request at once with a `75 46` frame that carries the
  request's own command byte, and sends it only once:** `91 00 "0.0.0.28"`,
  `93 00 "2025-11-28 20:15:49"`, `A9 00`, `C5 00`, `A7 00`. The version and
  build time come in that answer; the status (`0xAA`) and the name (`0xC6`)
  come later as frames of their own (seen by waved).
- **With waved a856f70 running, `0xC6` and `0xAA` came twice a second for
  as long as it ran** ("bt module name …" at info). Two candidate causes,
  both fixed; check S1 in the plan's device-pass checklist says which:
  1. `waved-uart`'s mux shared one port handle behind a mutex between its
     reader and its writer. The reader held it through each 100 ms read and
     took it back at once, so a write (an acknowledgement) could wait far
     past the module's 500 ms repeat: 1.4 s on a quiet pty in
     `mux::tests::a_write_never_waits_for_the_reader`. The reader now has its
     own handle (`try_clone`), as `waved-mcu`'s link does.
  2. waved acknowledged the `75 46` answers too, as stock does. Such an
     acknowledgement hands the module our own request byte (`75 46 02 A9 00 AB`
     carries GetStatus' `0xA9`, `75 46 02 C5 00 C7` GetName's `0xC5`).
     **[inferred]**: the module reads it as the request again and answers
     with another `0xAA` / `0xC6`, which is why only answers to our own
     requests kept coming. Stock does the same and saves its config on every
     `0xC6` (76953), so with the 1.2.2.6 module this loop probably does not
     happen; the 0.0.0.28 module may differ.
- **waved now** acknowledges every `75 45` frame but `0xE0` with stock's
  bytes, and no `75 46` frame (`command::ack_for_frame`). The `75 46` frames
  are never repeated, so nothing waits for that acknowledgement. A log line
  shows at info only when the frame told waved something new; a repeat goes
  to debug.

## Commands (host → module)

`waved_bt::command::Command` encodes the commands marked "yes" in the last
column, byte for byte.

| Byte | Stock name | Data | Reply | Stock use | waved |
|------|-----------|------|-------|-----------|-------|
| `0x8B` | CTRL_SET_NAME | the name, 23 bytes | `0x8C` | at boot; on each heartbeat while unnamed | yes: `device.name`, at start when the module's name differs, and on the 5 s poll while unnamed; at most 3 tries a boot per name |
| `0x91` | CTRL_GET_VERSION | — | `0x91` `[?] ascii` | at boot | yes |
| `0x93` | CTRL_GET_BUILDTIME | — | `0x93` `[?] ascii` | at boot | yes |
| `0x97` | CTRL_BT_PHONE_MODE | mode (stock always sends 1) | `0x97` | every 5 s | yes |
| `0x9A` | CTRL_GET_MAC | — | not handled | — | yes |
| `0xA3` | CTRL_BT_ENABLE (used as `bt_disconnect`) | `00` | — | Connect New Device (then `0xA7`) | yes |
| `0xA5` | CTRL_SWITCH_AUDIO_MODE | mode: 2 = the phone, 3 = the dongle, 0 (server request 9) | `0xA6` `[?] [mode]` | as the audio server opens the phone's or the dongle's path, then waits up to 100 ms for `BT_SWITCH_MODE` and asks once more (`audio_server_select_in_path_policy` 70553, at 70907 and 70965) | encoded, not sent |
| `0xA7` | CTRL_REPAIR (`bt_repair`) | `00` | — | pairing mode | yes |
| `0xA9` | CTRL_GET_BT_STATUS | — | `0xAA` | at boot; every 5 s | yes |
| `0xAB` | CTRL_GET_24G_STATUS | — | — | unused | yes |
| `0xAD` | CTRL_GET_PBAP_STATUS | — | `0xAE` | phone book | no (D: number only) |
| `0xAF` | CTRL_GET_PHONE_BOOK | — | one `0xB0` per contact | phone book | no |
| `0xB3` | CTRL_GET_RSSI (heartbeat) | battery step (`level/10`, minus 1 unless 0), charging 0/1, Wi-Fi present 0/1 | `0xB4` | every 5 s | yes |
| `0xC5` | CTRL_GET_NAME | — | `0xC6` | at boot | yes |
| `0xC7` | CTRL_GET_PHONE_MAC | — | `0xC8` | — | yes |
| `0xCB` | CTRL_CLOSE_BT | — | — | switch off; at boot when off; cloud power off; factory reset | yes: the switch, at start when off |
| `0xCC` | CTRL_OPEN_BT | `00` always (stock's `is_init` only decides whether it saves the switch, 74446-74473) | — | switch on; at boot when on; after a module reset | yes: the switch, at start when on |
| `0xCD` | CTRL_24G_ENTER_PAIRING | mode (`ctrl_bt_pair_mode`: 1 to pair, 2 to unpair the dongle) | — | dongle | no |
| `0xCE` | CTRL_24G_DISCONNECT | `00` | — | dongle | no |
| `0xD0` | CTRL_BUTTON_EVENT | event, `time(NULL)` as a u32 LE, a level byte (the volume for events 1, 2, 9 in 1.1.6.6; 0 or unset otherwise) | `0xD0` | call buttons; recording state; the Wave's volume while the phone plays | yes: call buttons |
| `0xD7` | CTRL_CLOSE_CONNECT_REPLY | reply | — | answers `0xD6` | yes |
| `0xD8` | CTRL_RECONNECT_SWITCH | 0/1; 2 at factory reset | — | at boot (the setting); the Auto Reconnect switch; 0 after each connect; 2 at factory reset | yes: at start (1 when on, 0 when off) once per boot per value, not while a phone is connected; 0 before Disconnect and Pair |
| `0xE0` | CTRL_BT_CONFIG_SAVE_FLASH | `00` | `0xE0` | at boot, after a connect, at power off | yes: at start, only after a `SetName` or `D8` of that start |
| `0xE2`–`0xE5` | LL/BT data test modes | 2–3 bytes | — | factory | no |
| `0xE6` | GET_DONGLE_VERSION | `00` | `0xE8` | update | yes |
| `0xE7` | GET_DONGLE_BUILDTIME | `00` | `0xE9` | — | yes |
| `0xEA` | SET_PTA_MODE | Wi-Fi present 0/1, then `00` | `0xEB` | at boot | yes: at start, 1 |
| `0x80` `0x9D` `0x9F` `0xA1` `0xA2` | module OTA | see Firmware | | update | `ota` module |
| `0xD9` `0xDB` `0xDD` `0xDF` | dongle OTA | see Firmware | | update | `ota` module |

### Button events (`0xD0` byte 0)

| Code | Sent by stock when | Source |
|------|--------------------|--------|
| 0 | hang up or decline: the red button on the incoming-call screen and on the calling screen | **[code]** ui 27933, 28725 |
| 1 | the Wave's volume went up (or is 100), while the phone or the dongle plays; also at boot (see "How stock runs the module") | **[code]** `audio_server_spk_volume_policy` 71893 (`increase_volume`), `bluetooth_set_volume` |
| 2 | the Wave's volume went down, while the phone or the dongle plays | **[code]** same |
| 9 | the Wave was muted (level 0) or unmuted (level = volume) | **[code]** same |
| 3 | answer: the green button on the incoming-call screen. Also "music button 0". | **[code]** ui 27968, 78679 |
| 4 | "music button 1" (meaning unknown) | **[code]** ui 78679 |
| 6 | a recording was set up or started | **[code]** ui 116516, 116565 |
| 7 | a recording paused | **[code]** ui 116614 |
| 8 | a recording finished | **[code]** ui 116663, 116712 |

Events 1, 2 and 9 carry the level in the last byte only in 1.1.6.6
(libui `bluetooth_set_volume` 90972 stores it; 1.2.2.6's, 115578, does not,
so `bt_button_event` 78918 leaves the byte as it was). What the module does
with them is unknown: its strings have `app_bt_vol_up!`, `api_bt_vol_down!`,
`app_bt_vol_restore!`, `avrcp_ct_send_vol_change step-absolute_vol` and
`app_set_avrcp_cur_music_vol -step_cur_vol,vol`, so they probably move the
phone's volume. waved sends none of them (see "Volume").

The module has more key handlers (`key_evt_mfb_short/double/long…`,
`key_evt_music_forward_func`, `key_evt_enter_pairing_func`,
`key_evt_clean_bt_link_info_func`, volume up and down). Their codes are
unknown. **[file]**

## Events and replies (module → host)

Offsets count into DATA, after CMD. `waved_bt::event::decode` turns each frame
into an `Event`.

| Byte | Meaning | DATA layout | Stock reaction | Source |
|------|---------|-------------|----------------|--------|
| `0x91` / `0x93` | version / build time | `[?] ascii…` | stores `SYSTEM_BT_VERSION` / `SYSTEM_BT_BUILD_TIME` | **[code]** 75368, 75504 |
| `0x8C` | name set | — | ack flag | **[code]** |
| `0x97` | phone mode taken | — | ack flag | **[code]** |
| `0xA6` | audio mode switched | `[?] [mode]`: 0 idle, 2 the phone, 3 the dongle | `BT_SWITCH_MODE`, which only the audio server reads (it waits for 2 or 3 after `0xA5`); no call or page uses it | **[code]** 75679, 70907. **[device]** unsolicited, 2 as A2DP started and 0 as it stopped |
| `0xAA` | status | `[connected]`, where 0 = no phone | `BT_CONNECT_STATUS` 1 / 0 | **[code]** 75696 |
| `0xAE` | PBAP state | `[ready]` | if 1, asks for the phone book (all versions; 1.1.6.6 64073). waved decodes it (`Event::PbapStatus`), logs it at debug and never asks | **[code]** 75716. **[device]** 175 in 40 min with an iPhone connected, one data byte, unasked (`0xAD` was never sent): the module's own PBAP retries (`PBAP dev closed, open PBAP`) **[inferred]** |
| `0xB0` | phone-book entry | `[?×4] [n] number[n] [m] name[2m]` (UTF-16LE, seen on the device) | `add_contact`, saved to `/tmp/PHONE_BOOK.bin`. waved drops it unread (`Event::PhoneBookEntry`, carries nothing) and acknowledges it | **[code]** 75722 |
| `0xB1` | call number | ASCII; empty means the number is gone | `BT_PHONE_COMING_NUM` (plus a name from the phone book) → incoming-call screen | **[code]** 75739, ui 102913 |
| `0xB2` | work status | `[bits] [?] [n] name[n] [?] [?] mac[6]`; name and MAC only matter when bit 0 is set | `BT_WORK_STAUS` → audio routing. On a new connection: status 0, name, MAC, `AutoReconnect(0)`, `SaveFlash`. Falls through into the `0xD0` handler. | **[code]** 75767-76013 |
| `0xB3` | heartbeat / connected phone | `[?] [n] name[n] [?] [?] mac[6]` | liveness; stores a changed name or MAC | **[code]** 76014 |
| `0xB4` | link report | `[dongle 0 or FE] [dongle RSSI i8] [2.4 GHz RSSI i8] [phone 0 or FE] [phone RSSI i8] [?] [tx u32 LE] [rx u32 LE]`; needs more than 5 bytes | liveness and RSSI. Phone 0 → connected; FE three reports running → disconnected | **[code]** 76195 |
| `0xBD` | dongle info | `[?] mac[6] sn… uuid… [freq] code[31]`; needs 30 bytes or more | stores DONGLE_MAC, SN, UUID, FREQ and CODE | **[code]** 76351 |
| `0xC4` | module info | `mac[6] uuid[16] sn[32] [freq]`; needs more than 9 bytes | stores BT_MAC, UUID, SN and FREQ (waved decodes the MAC only) | **[code]** 76754 |
| `0xC6` | name | `[?] name[23]` | `BT_NAME`, saved | **[code]** 76953 |
| `0xC8` | phone MAC | `[?] [6] mac[6]` on the device (stock reads `[?] mac[6]`, one byte early) | `BT_DEVICE_PHONE_MAC` | **[code]** 76980, device 2026-10-13 |
| `0xD0` | button taken | — | ack flag | **[code]** 76008 |
| `0xD1` | phone event | `[event]`: 0 = call up, 1 = call ended (both only while a number is stored), 2 = start, 3 = restart, 4 = pause, 5 = stop a recording | calling screen; recording control | **[code]** 76991, ui 85228 |
| `0xD4` | dongle keys | `[?] [?] [speaker mute] [volume] [mic mute]` | mute and volume | **[code]** 77033 |
| `0xD5` | phone volume | `[kind: 0 music / 1 call] [scale: 0 = 15 steps / 1 = 16] [level]` | stored as 0–100 in steps (`map_volume_100`); the music volume becomes the speaker volume while the phone plays (see "Volume") | **[code]** 74219, 73324 |
| `0xD6` | connection request | `[6] mac[6] [n] name[n]` | in factory mode, `ConnectReply(0)`. Otherwise `BT_CONNECT_STATUS = 2` and stores the name and MAC. | **[code]** 77078 |
| `0xE0` | flash saved | — | flag; never acknowledged | **[code]** 77151 |
| `0xE1` | phone dropped | `mac[6]` | disconnected, if it is the current phone | **[code]** 77158 |
| `0xE8` / `0xE9` | dongle version / build time | `[FE = no dongle, or ?] ascii` | stored | **[code]** 77174 and onwards |
| `0xEB` | PTA mode set | — | ack flag | **[code]** |
| `0xF0` | module log | `[1] text`, or `[2] raw bytes` | syslog | **[code]** 77363 |
| `0x80` `0x9E` `0xA0` `0xA1` `0xA2` `0xDA` `0xDC` `0xDE` | OTA replies | see Firmware (`0xDC`'s verdict is byte 4) | | **[code]** |

### `0xB2` bits (`audio_listen_bt_work_status_event`, 73172) **[code]**

| Bit | Meaning |
|-----|---------|
| 0 (`0x01`) | a phone is connected |
| 1 (`0x02`) | phone audio plays on the speaker (A2DP, or a call) |
| 2 (`0x04`) | phone audio runs both ways: the microphone too (a call) |
| 4 (`0x10`) | the dongle is connected |
| 5 (`0x20`) | dongle audio plays on the speaker |
| 6 (`0x40`) | dongle audio runs both ways |
| 3, 7 | unknown |

Stock's `BT_CONNECT_STATUS` is 0 = connected, 1 = not connected, 2 = a
connection request is pending ("Bluetooth connecting" page). **[code]**
ui 102857, 22256

**Unknown:**

- The two bytes between the name and the MAC in `0xB2` and `0xB3`. One is
  probably the MAC length (6), as in `0xD6`. **[inferred]**
- Whether MACs arrive in display order. Stock prints them in the order they
  arrive.

## Calls (I3) **[code]**

1. **The phone rings.** The module sends `0xB1 "<number>"`. Stock stores the
   number, looks up a name in the downloaded phone book and shows the
   incoming-call page with the name (or "call_id_title") and the number
   (ui `ui_call_in_screen_*`).
2. **The user answers or declines on the Wave.** Answer: `0xD0 03 <time> 00`.
   Decline: `0xD0 00 <time> 00`.
3. **The call is up**, answered on the Wave or on the phone. The module sends
   `0xD1 00`, and stock switches to the calling page (ID 0x48) with a timer.
   `0xB2` turns bits 1 and 2 on, and stock opens the Bluetooth audio paths
   both ways.
4. **Hanging up on the Wave** sends `0xD0 00 <time> 00`.
5. **The call ends.** The module sends `0xD1 01`, and stock leaves the calling
   page. An empty `0xB1` clears the number, and `0xB2` drops bits 1 and 2.

Stock only acts on `0xD1 00/01` while a number is stored (77006). The number
arrives only in `0xB1`; names come only from PBAP, which waved leaves out
(D: phone number only). The "HID hook-switch to the computer" (I3) belongs to
the USB HID side, not this link. The call state above is what drives it.

### When stock leaves its call pages **[code]**

`BT_PHONE_COMING_NUM` (0x8c) → ui `bt_call_in_event_cb` 102913,
`BT_DEVICE_PHONE_EVENT` (0x82) → `bt_call_phone_act_event_cb` 102970,
`BT_CONNECT_STATUS` (0x78) → `bluetooth_refresh_cb` 102857. Then:

| Page | Leaves on | Line |
|---|---|---|
| Incoming call (0x47) | `0xD1 01` (only while a number is stored) | ui `process_ntf_bt_callpickup_event` 85228 |
| | Decline: goes back at once and sends event 0 (business value 0x1b) | ui `hangup_btn_click_event_cb` 27920 |
| | Answer: opens the calling page at once and sends event 3 | ui `pickup_btn_click_event_cb` 27958 |
| | the phone gone (`BT_CONNECT_STATUS` 1): a hang-up sent to itself | ui `process_ntf_bt_connect_state` 85054 |
| Calling (0x48) | `0xD1 01`; Hang up (at once, event 0); the phone gone | 85228; ui 28702; 85054 |
| Incoming → calling | `0xD1 00`, answered anywhere | 85228 |

- **An empty `0xB1` changes no page** (`bt_call_in_event_cb` logs
  "strlen==0" and stops); it only clears the stored number, so a later
  `0xD1` is ignored.
- **No timeout**: the calling page's 1 s timer only counts the duration
  (`calling_timer_event_handler` 28350).
- **`0xB2` and `0xA6` never touch the call pages**: `0xB2` routes audio
  (`audio_listen_bt_work_status_event` 73172), `0xA6` is the audio mode
  the audio server waits for.

### Calls in waved (`crates/waved-bt/src/call.rs`)

On the device (build 15f8d8d, module 0.0.0.28) a call answered on the
iPhone sent neither `0xD1` nor an empty `0xB1`, and the Wave kept offering
Answer and Decline after the call. Stock would have stayed too. waved
leaves the ringing or in-call state on every sign it has:

| From | To | Sign |
|---|---|---|
| ringing | in a call | `0xD1 00`; Answer on the Wave (at once, as stock); module log `HFP  Call active`; 60 s without `0xB1` while the call audio (bit 2) is open |
| ringing | no call | `0xD1 01`; an empty `0xB1`; Decline on the Wave (at once); the phone gone or Bluetooth switched off; module log `HFP call end` or `HFP RFCOMM_CLOSED`; the call audio open during the ring, then closed for 3 s; 60 s without `0xB1` |
| in a call | no call | `0xD1 01`; an empty `0xB1`; Hang up on the Wave (at once); the phone gone or switched off; `HFP call end` / `HFP RFCOMM_CLOSED`; the call audio open during the call, then closed for 3 s; the call audio never open since the call went up and 60 s without news of it |

- The module log lines are the module's own fixed messages (0.0.0.28 and
  0.0.0.33): `HFP  Call active` (two spaces), `HFP call end`,
  `HFP RFCOMM_CLOSED`. Whether the module sends them over the UART at its
  log level is unverified (checklist T1).
- A call that went up with its audio closed (answered on the phone, audio
  there; or answered on the Wave and the module never opened the call
  audio) stays "In a call" while the module says so, and ends 60 s after
  its last word (`call::QUIET_CALL_MS`, the ring's own bound): the number
  again (`0xB1`), `0xD1 00` or `HFP  Call active`. Stock would keep its
  calling page until `0xD1 01`; with no bound a missed end kept "In a call"
  and the lit screen for ever. **Cost if wrong:** a call answered on the
  phone with its audio there, which the module stays silent about, shows
  "In a call" for one minute only; if the phone then moves the call audio
  to the Wave, the audio plays but no call screen (and no Hang up) shows,
  as the call audio alone starts no call.
- Each `0xB1` with the same number restarts the 60 s; stock has no such
  timeout. Networks hand an unanswered call to voicemail after 20–30 s.
- After Decline or Hang up on the Wave, the same number does not ring again
  until the module has been quiet about it for 10 s.
- **Cost if wrong:** a ring with in-band audio whose SCO link drops for more
  than 3 s mid-ring would dismiss the screen (the phone still rings); a call
  answered on the phone with audio here shows "Incoming call" for up to
  60 s before "In a call".

## Pairing and connection (I1)

- **Pairing mode:** `EnterPairing` (`0xA7 00`). Stock's "Connect a new device"
  sends `Disconnect` (`0xA3 00`), then `EnterPairing` (ui 115166).
  **[code]** The module logs "BT set pairing mode by user". It refuses while
  connected or reconnecting ("Warning! bt set pairing but bt has been
  connected", "…while bt is reconnecting"). **[file]** No command to leave
  pairing mode exists (see "Stock's Bluetooth controls"); the module ends it
  on its own timeout ("time out ,app_bt_pairing_exit") **[file]**, whose
  length is unknown.
- **Discoverable when idle:** no, with waved. With no phone connected a new
  phone does not find the module; `Disconnect` then `EnterPairing` makes it
  discoverable (device, 2026-10-13). Stock sends the two back to back with no
  wait (`bluetooth_set_connect(0)`, ui 115208; `bluetooth_set_connect(1)`
  sends `EnterPairing` alone). Stock's page says "Device can be discovered as
  …" whenever its switch is on, and stock sends `OpenBt` at every boot,
  which waved never does. **[inferred]**: `OpenBt` may make the idle module
  discoverable; a device test (checklist).
- **Paired devices:** the module keeps the link keys itself
  (`app_write_stored_linkkey`, `flash_clear_last_dev_func`, "linkkey change,
  clean old services"). **[file]** The protocol has no command to list,
  connect to or forget a device. The host only learns about the connected
  phone, from `0xB2`, `0xB3`, `0xC8` and `0xD6`. **[code]** **[inferred]**
  from that:
  - a paired list has to be host-side memory of the phones seen;
  - "connect" means letting the module reconnect (`AutoReconnect(1)`, which
    reaches only the last phone) or connecting from the phone;
  - "forget" can only be stock's "Connect a new device" (`Disconnect` then
    `EnterPairing`), or the factory reset's `0xD8 02`. Whether either erases
    the old link key is unknown.
- **Auto reconnect:** stock sends the saved setting at boot. Each time a phone
  connects it sends `0xD8 00`, then `SaveFlash` (75955, 76006). So `0xD8`
  probably means "try to reconnect now" more than a stored setting:
  **[inferred]** `0xD8 01` starts the module's reconnect to the last phone
  ("connback"), `0xD8 00` stops it. Details below.
- **Connection request:** `0xD6` puts stock in the "connecting" state. The
  answer is `ConnectReply` (`0xD7 [reply]`). In 1.2.2.6 the UI never sends
  one; only the factory/aging path replies, with 0. Stock treats reply 1 as
  "now disconnected" (74485). So **[inferred]**: 0 accepts and 1 refuses.
- **Name:** stock's `SetName` always sends exactly 23 bytes:
  `snprintf(data, 24, "%s%s", "Insta360 Wave BT ", last_six_digits)`
  (74400; the six hex digits of `/proc/cpuinfo`'s `Serial`, `extract_uuid`
  74100), so no NUL reaches the wire. It is sent first at boot, again until
  `0x8C` arrives (10 tries, 1 s apart, 79416), and on each 5 s heartbeat
  while the module's name still contains `XXXXXX` (75122). **[code]**
  waved sends `device.name` (the mockup's "Shows up as Wave Boardroom"),
  cut at 23 bytes on a character boundary and NUL-padded to 23, with the
  same retries, only when the module's own name (`GetName`, read first)
  differs, and at most 3 times a boot for one name (start-up and the 5 s
  poll together; see "Start-up and the module's flash"); an empty
  `device.name` sends stock's name.
  Whether the module accepts a NUL-padded shorter name is unverified
  (checklist T4: the screen's "Shows up as" is the module's own `0xC6`
  answer). **Cost if wrong:** the module keeps or garbles the name; the fix
  is padding with spaces or stock's format, in `startup::module_name`.
- **On/off:** `OpenBt` (`CC 00`) / `CloseBt` (`CB`). The host persists the
  switch (`INSUTIL_CFG2_BT_IS_OPEN`). **[code]** waved: config
  `[bluetooth] enabled` (default on), `Command::SetBluetooth`, the switch in
  the Bluetooth screen's device row.

### Stock's Bluetooth controls (1.2.2.6) **[code]**

Plain numbers are `libins_entry.so.c` lines, `ui` ones `libui.so.c`. The copy
is stock's English (`/etc/pspapp/lvgl/language/language_EN.json`). **[file]**

**The page** (`ui_setting_bluetooth_screen_on_create`, ui 22409; refresh ui
22256):

- a Bluetooth switch, with "Device can be discovered as <name>" under it
  while on;
- "Auto Reconnect" ("Auto-connects to last device when powered on"), shown
  only while on;
- "Connect New Device", shown only while on **and** a phone is connected
  (ui 22320-22369).

There is no pairing page, no countdown, no paired list and no Disconnect or
Forget button.

**The Bluetooth switch** (ui `bt_onoff_switch_event_cb` 22124 → business
message 0x17 → `business_executor` ui 83262 → `bluetooth_set_switch` ui
115118):

| | Frame | Host side |
|---|---|---|
| On | `bt_open_switch(0)` (79149): `75 45 02 CC 00 CE` | `BT_IS_OPEN` = 1, saved to config and flushed (74445-74468). `is_init` only decides this saving; data[0] is then set to 0 (74473), so the wire carries `CC 00` at boot too. |
| Off | `bt_close_switch()` (79124): `75 45 01 CB CC` | `BT_IS_OPEN` = 0 and `BT_CONNECT_STATUS` = 1 ("not connected") at once, saved (74413-74444). |

- Neither touches btpower, `0xD8` or `0xE0`, and nothing waits for a reply.
- **While off, stock ignores the phone:** `0xB2` counts as "not connected"
  whatever bit 0 says (75805), and `0xB4` neither connects (76303) nor counts
  towards a disconnect (76327). `0xAA` is not gated (75696).
- At boot `bt_info_init` (79403) sends `CloseBt` if the saved switch is off,
  else `OpenBt`. The cloud "power off" (`mqttcli_power_off` 198112) closes it
  too, and power on (`mqttcli_power_on` 198043) or a wake (197924) reopens it
  if it was on. After a module reset with the switch on: 10 s, then `OpenBt`
  (`bt_reset` 78286, `bt_crc_check`).

**Auto Reconnect (`0xD8`):**

- At boot: `D8 <saved setting>`, then `SaveFlash` (79438-79439).
- The switch (ui `bt_auto_reconnect_switch_event_cb` 22153 → message 0x19 →
  `bluetooth_set_auto_reconnect_switch` ui 115446): `D8 <0|1>` while
  database value 0x77 is 0 (its name is not in the decompile), then saves
  `BT_IS_RECONNECT`.
- **Each time a phone connects** (`0xB2` bit 0 while on and not yet
  connected, 75805-76006): status "connected", `BT_HAS_EVER_CONNECT` = 1,
  **`D8 00`** (75955), the phone's name and MAC when new, `SaveFlash`
  (76006). So whenever stock has a phone connected, the module's reconnect is
  off.
- Factory reset (`system_factory_reset` 105466): `CloseBt`, then `D8 02`
  (105474-105475). What 2 means is unknown; **[inferred]** it clears the
  module's last device (module strings `app_bt_clean_last_dev`, `clean all
  info`). Destructive: test it only to wipe pairings.
- Module strings: `RECONN_START!`, `RECONN_CANCEL!`, `bt stop connback`,
  `app_bt_start_connback_func`. **[file]**

**Connect New Device** (ui `bt_new_connect_btn_clicked_cb` 22184):

1. A confirmation: "Connecting a new device will disconnect from <phone>.
   Continue?", with "Disconnect" and a cancel.
2. Confirm (`bluetooth_disconnect_done_event_cb` ui 39896) → message 0x1a,
   value 0 → `bluetooth_set_connect(0)` (ui 115166): forget the stored phone
   name, `bt_disconnect` (`75 45 02 A3 00 A5`, 78702), `bt_repair`
   (`75 45 02 A7 00 A9`, 78724), back to back, waiting for no reply.
3. Cancel (ui 39874) returns to the page and sends nothing.

**Leaving pairing:** stock has no pairing page to leave, and no command that
ends pairing mode: `bt_ctrl_cmd_send` (74281-74619) builds every command
there is. The "Bluetooth connecting..." page (ui 22671, for a `0xD6`
request) has a Cancel (ui 22648) that sends message 0x1a, value 0: the same
`Disconnect` + `EnterPairing`.

**Disconnect and forget:** no plain Disconnect and no Forget. `bt_disconnect`
is reached only through Connect New Device, the connecting page's Cancel and
the debug `_control_handler` ("disconnect", 89180 and onwards). The only wipe
is the factory reset's `0xD8 02`.

**When stock counts the phone as gone** (`BT_CONNECT_STATUS` = 1):

| Trigger | Rule | Line |
|---|---|---|
| `0xB2` | bit 0 clear, or the switch is off | 75805 |
| `0xAA` | data byte 0 is 0 | 75696 |
| `0xE1` | its MAC is the stored phone's | 77158 |
| `0xB4` | phone byte `0xFE` in three reports in a row, switch on | 76327 |
| `CloseBt` | at once | 74422 |
| `ConnectReply(1)` | at once | 74486 |

It counts the phone as connected on `0xB2` bit 0 with the switch on (75805),
`0xAA` non-zero (75708) and `0xB4` phone byte 0 with the switch on (76303).

**Power off** (ui `_device_bt_poweroff_pre_action` 114129,
`_device_bt_poweroff` 114173): `SaveFlash`, then up to 20 × (`SaveFlash`,
0.5 s) until `0xE0` arrives, 1 s, btpower 1. btpower (`bt_power_set` 77956)
is used only for resets (75069, 78804, 79533) and power off, never by the
switch.

**Acknowledgements:** `bt_msg_recv_thread` acknowledges every parsed frame
but `0xE0`, whatever its prefix, at once from its receive thread
(75273-75279; bytes from `bt_ctrl_cmd_send_without_ack` 74888).

### Seen on the device with waved a856f70 (2026-10-13) **[device]**

1. `0xC6` and earlier `0xAA` came twice a second for as long as waved ran
   (see "Acknowledgements and replies").
2. Back from the pairing countdown did not end pairing: nothing was sent, and
   the countdown went with the screen.
3. Connected, the screen offered only Disconnect.
4. After `Disconnect` (`75 45 02 A3 00 A5`) the music stopped, but within a
   second `0xB2` said `0x03` again: the iPhone was back, and the screen said
   "connected" throughout.
5. From a shell, `CloseBt` (`75 45 01 CB CC`), then 10 s later `OpenBt`
   (`75 45 02 CC 00 CE`): the iPhone stopped playing, but the module kept
   reporting `0x03` and `0xAA 01`, and the phone did not show a disconnect.
   Stock would have shown "not connected" all along: it forces that on
   `CloseBt` and ignores bit 0 while off. Whether the phone drops after a
   longer wait (a link supervision timeout is often 20 s) is not known yet.

### What waved does with this

- **Disconnect** = `AutoReconnect(false)` then `Disconnect`
  (`75 45 02 D8 00 DA`, `75 45 02 A3 00 A5`). Stock never disconnects with
  the module's reconnect on (it turns it off as each phone connects); waved
  never sent `0xD8`, so the module was free to reconnect at once. The phone
  can still connect from its own Bluetooth settings. If the phone itself
  reconnects, only `CloseBt` can stop it: stock's Bluetooth switch, with the
  host forcing "not connected" while off and `OpenBt` to turn it back on.
- **The Bluetooth switch** (built 2026-10-13): `SetBluetooth(false)` sends
  `CloseBt`, counts the phone as gone at once (the call, the audio bits) and
  ignores `0xB2` bits 0-2, `0xB4`, `0xAA`, `0xB1`, `0xD1` and `0xD5` until
  `SetBluetooth(true)` sends `OpenBt`. Stock ignores only `0xB2` bit 0 and
  `0xB4`; `0xAA` too here because the module kept answering `0xAA 01` after
  `CloseBt` (observation 5). The module's name, MAC and pairing bit still
  count. Nothing else is sent while off.
- **Pair a device** and **Pair another phone** = `AutoReconnect(false)`,
  `Disconnect`, `EnterPairing`: stock's Connect New Device with the reconnect
  off, as stock always has it while a phone is connected (and the module
  refuses pairing while it reconnects). Pair another phone asks first, as
  stock does ("Pair another phone? Wave disconnects from <phone>, then shows
  up for a new phone for two minutes.", `ConfirmKind::PairAnotherPhone`).
- **Leaving the Bluetooth screen** (Back, a swipe, a takeover; not resting)
  sends `BtStopPairing`, which ends Wave's countdown. Nothing goes to the
  module: there is no command for it. The module stays discoverable until
  its own timeout or a phone connects; the device pass measures how long.

## Audio (I2)

- **Route:** TDM-A connects to the module. Stock's routes are `bt2spk`
  (capture `hw:0,0` → speaker) and `mic2bt` (processed microphones → playback
  `hw:0,0`). It has the same pair for the dongle (`dongle2spk`, `mic2dongle`)
  through the same device class, so the module carries the phone and the
  dongle on one I2S link. **[code]** `libInstaAudio` strings; **[inferred]**
  that the link is shared
- **Format:** 48 kHz, 2 ch, S16_LE in both directions. Stock's periods are 480
  frames (10 ms) × 8 for capture and 960 frames (20 ms) × 10 for playback.
  **[code]** 3.0.0.1 `libInstaAudio.so.c` 19425-19504
- **When:** stock opens and closes the paths on the `0xB2` bits.
  - Bit 1 opens the speaker path; bit 2 adds the microphone path. Dongle bits
    5 and 6 work the same way.
  - The phone's microphone path and the dongle's exclude each other.
  - One source drives the speaker at a time (USB, Bluetooth or the dongle);
    the current source keeps it while it stays active.

  **[code]** 73172, 70553
- **Codecs:** SBC, mSBC and CVSD are decoded and encoded in the module. The
  I2S side is always 48 kHz. **[file] [inferred]**
- **Clocks:** TDM-A runs on the SoC's clock (the SoC is the master), unlike the
  USB host. So Bluetooth audio shouldn't drift against the speaker.
  **[inferred]** from the dtb
- **Volume:** see "Volume" below.

**In waved** (`crates/waved-audio/src/bt_audio/`, built 2026-10-13, not yet
run on the device; details in `AUDIO_HARDWARE.md` "Bluetooth audio (I2)"):

- **Bit 1 (`phone_audio`)** opens `hw:0,0` capture and feeds the speaker path
  (`PlaybackService::play_phone`): Wave volume and mute, EQ, limiter, the
  echo canceller's reference, and the far end in recordings. The bit clearing
  closes it.
- **Bit 2 (`call_audio`)** opens `hw:0,0` playback and writes the processed
  uplink (the USB microphone's signal, mono on both slots).
- **USB and Bluetooth mix; neither takes precedence** (unlike stock). macOS
  keeps the UAC2 stream open and sends zeros when nothing plays, so "the
  current source keeps the speaker" would lock the phone out.
- Periods are 1024 frames × 4 (config `bt` block), not stock's 480/960.
- `bt.playback_clock: true` also runs playback (silence) during music, in case
  TDM-A capture needs it; off by default because `arecord` alone worked.
- The phone's volume (`0xD5`) sets the speaker volume ("Volume"); dongle
  bits 5 and 6 are not routed.

### Volume **[code]**

Stock 1.2.2.6, `libins_entry.so.c`:

1. `0xD5 [kind] [scale] [level]` → `sync_phone_volume` (74219): scale 0 is
   15 steps, 1 is 16 (stored as `BT_PHONE_MAX_VOLUME`, 0x8b); any other
   scale leaves the maximum 0, which maps to 0.
2. `map_volume_100(level, max, 100)` (74146): `level × 100 / max` (level
   clamped to `max`), then 0 → 0, 1–10 → 5, 11–19 → 10, then down to the
   ten (20–29 → 20 … 90–99 → 90), 100 → 100. With 15 steps: 0, 5, 10, 20,
   20, 30, 40, 40, 50, 60, 60, 70, 80, 80, 90, 100.
3. Kind 0 is stored as `BT_MUSIC_VOLUME` (0x89), kind 1 as `BT_PHONE_VOLUME`
   (0x8a). The audio server's listener table (`g_audio_client_register_param`,
   `.data` 0x284d08 of `libins_entry.so`, `{status, 0x0b, callback}` with
   relocated callbacks) sends 0x89 to `audio_listen_bt_spk_volume_event`
   (73324) and 0x8a to `audio_listen_bt_mic_volume_event` (73395), which
   only logs "bt mic volume is %d". The status ids come from the DWARF.
4. `audio_listen_bt_spk_volume_event` calls `audio_server_spk_volume_policy`
   (71893) with `vol_src` 2 (Bluetooth), `vol_type` 1 (set): applied only
   when the current input path is `BT_IN` (4) or `DONGLE_IN` (8), "not need
   sync volume" otherwise. 0 mutes, anything else unmutes; the volume is
   saved (`INSUTIL_CFG2_AUDIO_SPK_VOLUME`) and shown, without a tone.
5. The other way: a volume change of the Wave's own while the input path is
   the phone or the dongle calls `bluetooth_set_volume(1 | 2 | 9, volume)`,
   a `0xD0` button event ("Button events"); 1.1.6.6 sends the volume in it,
   1.2.2.6 does not. Also `bluetooth_set_volume(1, volume)` as the phone's
   path opens (70907) and at boot.

No other frame carries the phone's volume: `0xD4` is the dongle's keys
(raw volume into both statuses), `0xAE` is PBAP's link. 1.0.13.6, 1.1.6.6
and the business build handle `0xD5` the same way (1.1.6.6 65441, 62590).

**waved** (`crates/waved-bt/src/volume.rs`):

- Both kinds through `map_volume_100`, while the phone's audio plays
  (`0xB2` bit 1 or 2, waved's `BT_IN`). The call kind applies too, so the
  phone's volume keys work in a call; stock only logs it. **Cost if wrong:**
  if the module also scales the call audio by the phone's volume, quiet
  calls get quieter; drop `VolumeKind::Call` in `speaker_target`.
- `SetSpeakerVolume` on the bus, so it is saved like any volume change.
  Level 0 sets 0 without muting; a level above 0 unmutes, as stock: the
  volume first, then `SetSpeakerMute(false)`, at no less than waved-audio's
  10 % unmute floor (from muted, a 5 % level unmutes at 10 %).
- **The phone's volume never sends the computer a key.** Both changes are
  marked before either command goes: the volume in
  `StateBus::host_pending_speaker_vol`, the unmute in
  `StateBus::pending_speaker_mute`, which the HID mute watch consumes on that
  edge (and drops when no edge can come). Before (review 2026-10-13), the
  unmute reached the computer as a `ConsumerMute` key, and at a 5 % level
  the unmute's 10 % floor sent a stray volume-up key (only the final 5 %
  was marked). **Cost if wrong:** the computer's mute no longer follows a
  speaker the phone unmuted; macOS's F10 toggles the Wave's speaker mute
  (`ToggleSpeakerMute`), so one press may mute it instead of unmuting.
  With a 5 % phone level and a muted Wave, the speaker plays at 10 %.
- An unknown scale is ignored (stock would mute).
- **The Wave's own volume is not sent to the phone.** Stock's `0xD0 1/2/9`
  events are unverified on this module (what they do, whether the phone's
  answer comes back as `0xD5` and fights the Wave's knob). **Cost:** the
  phone's slider does not follow the Wave; its next change sets the Wave
  to the phone's level. Checklist T3 tries one event by hand.
- On the device no `0xD5` came in 40 minutes (whether the phone's volume
  was changed then is unknown; checklist T2).

## Firmware: `BT.bin` and `DONGLE.bin` (TODO F6)

### What the blobs are **[file]**

| Firmware | `BT.bin` | `DONGLE.bin` |
|----------|----------|--------------|
| 1.0.13.6 | 0.0.0.27, built 2025-10-27 21:41:50 | same |
| 1.1.6.6 | 0.0.0.28, built 2025-11-28 20:15:49 | same |
| business 1.0.2.6 | 0.0.0.28, built 2026-03-13 17:24:27 | same |
| 3.0.0.1 | 0.0.0.33, built 2026-04-17 13:45:53 | same |
| **1.2.2.6** | **0.0.0.33, built 2026-06-12 12:43:40**: 622 346 bytes, sha256 `5ded27d2…` | 0.0.0.33, built 2026-06-12 12:43:40: 179 212 bytes, sha256 `acadd3f6…` |

`BT.bin` is the module's firmware. `DONGLE.bin` is the firmware of the USB
dongle, a Telink 2.4 GHz audio receiver for the computer, and reaches the
dongle through the module. The 1.2.2.6 hashes match `sw-description`.
**3.0.0.1 and 1.2.2.6 both report 0.0.0.33, so compare the build time too.**

- **Layout:** bytes 0x0000–0x7FFF hold the Telink boot area (`KNLT` at 0x20)
  and are never sent. At 0x8004 a u32 holds the payload length minus 0x1000;
  that holds for all ten images. **[file] [code]**
- **Size limits:** stock refuses a module image smaller than 0x4B000 and a
  dongle image larger than 0x4B000. **[code]**

### How stock loads them **[code]**

- **Update** (`ota_thread`, around 45440): swupdate drops the blobs in
  `/tmp/insta360/` (`sw-description` `files`). After installing the rootfs,
  stock does this in order:
  1. It **always** flashes the module (`BT_CMD_UPGRADE`), up to 3 tries, with
     `reset_bt` and `bt_uart_restart` between tries.
  2. It flashes the MCU.
  3. If the dongle is connected and its version (`0xE6` → `0xE8`) differs
     (`dongle_compare_versions`, `a.b.c.d`), it flashes the dongle.

  The dongle image is also kept under `/mnt/udisk/misc/data/DONGLE.bin`
  (through `/opt/DONGLE.bin`) for later.
- **Recovery:** `bt_heartbeat_thread` re-flashes `/usr/lib/BT.bin` when the
  module stays silent through two power-cycles.
- **Otherwise never**, in particular not at boot.

### The update sequence **[code]** 78031, 78420, 74668; timeouts from `timeout_set`

| Step | Module | Dongle |
|------|--------|--------|
| request | `0x80 AB` → `0x80` (20 tries, 3 s each) | — |
| size | `0x9D size_u32` → `0x9E` (20 tries, 3 s) | `0xD9 size_u32` → `0xDA` (20 tries, 1 s) |
| packets | `0x9F` → `0xA0`; byte 0 = 3 is an error (up to 21 tries per packet, 3 s) | `0xDB` → `0xDC`; byte **4** non-zero is an error (21 tries, 1 s) |
| CRC check | `0xA1 01` → `0xA1`; byte 0 = 1 passes (20 tries, 5 s) | `0xDD 01` → `0xDE`; non-zero is an error (20 tries, 10 s) |
| reset | `0xA2 01` (1 s for `0xA2`), then 1 s sleep | `0xDF 01`, 1 s, sleep 1 s; up to 3 times while the dongle still shows; it never answers |

Details (subagent read of `bt_ota_cmd_send` 74668 and the two loops, checked
against the binary's DWARF and disassembly) **[code]**:

- Each try clears its reply slot, sends, then polls every 1 ms up to the
  step's count from `timeout_set` (77631); a retry sends the frame again.
  A reply that fails (`0xA0` 3, `0xA1` not 1) ends the try at once.
- **The module's size frame goes out with `0xAB` as its low byte**: the size
  shares a union with the request's `boot_mode_key`, and the request is built
  first (74706, 78114). The 1.2.2.6 image announces `9D AB FF 08 00`, never
  `9D 0A FF 08 00`. Every stock update of the module was made this way.
- Module: if the request or the size goes unanswered 20 times, stock goes
  on anyway; it sends the reset (`A2 01`) on every path, success or not.
  The first packet's tries are what the size step left over (the counter
  is not reset, 78214).
- Dongle: no request. The size step stops counting tries after one failure
  (`ret | …`, 78512); a failing `0xDE` counts as a pass (78624); a packet that
  fails 21 times returns without a reset.
- Stock pauses its heartbeat and its log-line handling while updating
  (`bt_ota_begin`, 75120, 77462). The UART stays at 921600 (`bt_uart_restart`
  78318 reopens it at that speed between tries).
- Around it (`ota_thread` 45510): up to 3 tries, with `reset_bt` (btpower 1,
  1 s, 0) and `bt_uart_restart` between them. After a module flash stock
  goes on to the MCU and reboots; `bt_crc_check` (78791) and `bt_reset`
  (78286) send `OpenBt` 10 s after a reset when the switch is on.
- Recovery (`bt_heartbeat_thread` 75131-75160): no `0xB3`/`0xB4` for 21 s
  on 36 heartbeats in a row power-cycles the module; the second time in a
  row it re-flashes `/usr/lib/BT.bin` first (about 6 minutes of silence).

- **Packet layout:** `addr u32 | crc u32 | len u16 | offset u16 | data[len]`,
  all little-endian, at most 64 bytes of data.
  - `addr` counts from the end of the boot area.
  - `crc` is the reflected CRC-32 table update from `0xFFFFFFFF` **without the
    final XOR**, i.e. the bitwise NOT of the usual CRC-32. **[code]** 74068;
    **[file]** every `CRC32_Table` copy in `libins_entry.so` is the standard
    table.
  - `offset` is the length for the module (0 for a short last packet) and
    always 0 for the dongle.
- **Size announcement:** `size` = the u32 at 0x8004 + 0x1000.

### Adopting them in waved (F6)

The device's module runs **0.0.0.28** (2025-11-28 20:15:49, 1.1.6.6's
`BT.bin`); 1.2.2.6 ships 0.0.0.33 (2026-06-12 12:43:40). Their strings differ
only in the version, the build time and a few timing messages: nothing about
calls, volume or PBAP changed. F6 is low priority unless the module
misbehaves.

**Built (2026-10-13, host-tested, never run on the device):**
`waved_bt::ota::flash` runs stock's sequence over a `Link`, and the
**`waved-bt-flash`** tool is the only caller. Nothing in waved runs it: not
the service, not boot, not an update (`the_service_never_flashes`).

- Same as stock: frames byte for byte (the module's size with `0xAB`), the
  waits per try, 20 tries for request / size / CRC, 21 per packet, the
  module reset on every path once it took the request, 1 s sleep after it,
  up to 3 dongle resets while it still shows (`0xB2` bit 4).
- Stricter than stock: it stops if the module never takes the request
  (nothing written yet); the dongle's size gets its 20 tries; a failing
  dongle CRC is a failure; a failing CRC reply does not cut the 5 s / 10 s
  window short (a pass later in it counts, in case the module answers the
  check at once as it answers other requests); each packet gets 21 tries.
- It acknowledges every `75 45` frame as the service does, so a log line
  mid-update cannot stall the module's queue. It prints no frame contents
  but the versions.

**How to run it** (device session, with the owner's approval for `flash`):

```bash
# Build (the integrator): the tool is a binary of its own.
cross build --target aarch64-unknown-linux-gnu -p waved-bt --bin waved-bt-flash
adb push target/aarch64-unknown-linux-gnu/debug/waved-bt-flash /tmp/
# The images: 1.2.2.6's extracted BT.bin / DONGLE.bin (sha256 5ded27d2… / acadd3f6…).
adb push firmwares/fw_v1.2.2.6/extracted/BT.bin /tmp/BT.bin
adb shell '/tmp/waved-bt-flash check module /tmp/BT.bin'      # opens nothing
# Stop the watchdog and waved (waved/CLAUDE.md), then:
adb shell '/tmp/waved-bt-flash version'                      # read-only
adb shell '/tmp/waved-bt-flash flash module /tmp/BT.bin --owner-approved'
```

`check` prints the version and build time inside the image, the packet
count and the first frames; `version` asks the module (and the dongle) for
theirs. Flash only when they differ. `flash` refuses without
`--owner-approved` and while a `waved` process runs.

**Risks.** A failed or interrupted flash can leave the module without a
working application: no Bluetooth, no phone audio, no dongle, until it is
flashed again. **[inferred]** The 0x8000-byte boot area is never sent, so the
Telink bootloader should still take the image again; power-cycling the
module (`echo 1 > /sys/class/leds/btpower/brightness; sleep 1; echo 0 > …`)
is the recovery to try first, then running `flash` again. Keep the Wave on
USB power, nothing else on `ttyS1`, and do not touch it (about 9 200
packets, a few minutes). Stock re-flashes `/usr/lib/BT.bin` by itself after
6 minutes of silence; waved does not, and going back to stock firmware
would. The dongle is flashed through the module's 2.4 GHz link: flash it
only when it is connected (`0xB2` bit 4) and its `0xE8` version differs.


## Example frames

Computed with the stock rule. They are also the unit-test fixtures in
`crates/waved-bt`.

| Frame | Bytes |
|-------|-------|
| GetStatus | `75 45 01 A9 AA` |
| GetVersion / GetBuildTime / GetName | `75 45 01 91 92` / `75 45 01 93 94` / `75 45 01 C5 C6` |
| OpenBt (boot and switch alike) / CloseBt | `75 45 02 CC 00 CE` / `75 45 01 CB CC` |
| Disconnect / EnterPairing | `75 45 02 A3 00 A5` / `75 45 02 A7 00 A9` |
| AutoReconnect on / off | `75 45 02 D8 01 DB` / `75 45 02 D8 00 DA` |
| SetName "Wave Boardroom" (waved, NUL-padded to 23 bytes) | `75 45 18 8B 57 61 76 65 20 42 6F 61 72 64 72 6F 6F 6D 00 00 00 00 00 00 00 00 00 FB` |
| Module size as stock sends it (1.2.2.6 `BT.bin`) | `75 45 05 9D AB FF 08 00 54` |
| Phone volume: music, 15 steps, level 8 (→ 50 %) | `75 45 04 D5 00 00 08 E1` |
| PBAP link status 0 | `75 45 02 AE 00 B0` |
| waved's Disconnect | `75 45 02 D8 00 DA`, `75 45 02 A3 00 A5` |
| waved's Pair (Pair a device, Pair another phone) | `75 45 02 D8 00 DA`, `75 45 02 A3 00 A5`, `75 45 02 A7 00 A9` |
| Module answers (not acknowledged by waved) | `75 46 02 A9 00 AB`, `75 46 02 C5 00 C7`, `75 46 02 A7 00 A9` |
| Ack for `0xC6` / `0xAA` | `75 46 02 C6 00 C8` / `75 46 02 AA 00 AC` |
| SaveFlash | `75 45 02 E0 00 E2` |
| PhoneMode 1 | `75 45 02 97 01 9A` |
| PtaMode, Wi-Fi present | `75 45 03 EA 01 00 EE` |
| Heartbeat: 85 %, charging, Wi-Fi | `75 45 04 B3 07 01 01 C0` |
| SetName "Insta360 Wave BT 0A1B2C" | `75 45 18 8B 49 6E 73 74 61 33 36 30 20 57 61 76 65 20 42 54 20 30 41 31 42 32 43 1D` |
| Answer, t = 0x67200000 | `75 45 07 D0 03 00 00 20 67 00 61` |
| Hang up / decline, same t | `75 45 07 D0 00 00 00 20 67 00 5E` |
| Ack for `0xB2` / for `0xD4` | `75 46 02 B2 00 B4` / `75 46 01 D4 D5` |
| OTA request / size 589 578 | `75 45 02 80 AB 2D` / `75 45 05 9D 0A FF 08 00 B3` |
| Event: status connected | `75 45 02 AA 01 AD` |
| Event: call number "+3212345678" | `75 45 0C B1 2B 33 32 31 32 33 34 35 36 37 38 F1` |
| Event: call up / call ended | `75 45 02 D1 00 D3` / `75 45 02 D1 01 D4` |

## What waved assumed until 2026-10-12, and what changed

- **Command bytes.** `waved-bt` sent `0x0E`, `0x09`, `0x0A`, `0x0D`, `0x07`,
  `0x03`, `0x02` and `0x04`, and expected replies `0x01`, `0x02`, `0x09` and
  `0x0E`. Those are stock's **server-request numbers** (the
  `bt_server_request` cases, also the "Command Set" table in
  `docs/REPORT.md`), not wire bytes. None of them exists on the wire.
- **Checksum.** `waved-uart` added the two prefix bytes into the checksum, so
  every real frame would have failed. It now sums from `LEN`, as stock does.
- **Speed.** `main.rs` still opens `ttyS1` at 115200. It should be 921600
  (`waved_bt::BT_BAUD`); this is the one-line follow-up for the integrator.

### In waved now (`crates/waved-bt`)

| Module | Role |
|--------|------|
| `frame.rs` | framing and the parser |
| `command.rs` | host commands and acknowledgements |
| `event.rs` | decodes module frames |
| `state.rs` | folds events into `BluetoothState` (the phone, the module's name, the switch) and `BtConnected` / `BtDisconnected` |
| `call.rs` | the call: rings, up, and every sign that it is over (see "Calls in waved") |
| `control.rs` | bus commands → frames (the switch, pairing, disconnect, call buttons); the 2-minute pairing window |
| `startup.rs` | stock's start-up sequence; the module's name |
| `volume.rs` | the phone's volume on the speaker (see "Volume") |
| `ota.rs` | firmware images, OTA packets and the update loop (only `waved-bt-flash` runs it) |

`BtService` keeps its constructor and runs on the `waved-uart` mux at 921600
(the mux's reader and writer each have their own port handle, so an
acknowledgement never waits behind a read). `main.rs` hands it the bus with
`.with_commands(bus.commands.subscribe())`, config with
`.with_settings(Settings::new(config.bluetooth.enabled, &config.device.name))`
and the speaker with `.with_speaker(Speaker::from_bus(&bus))`.

- At start it sends stock's `bt_info_init` (79400) with the reads first and
  the writes only when needed (see "Start-up and the module's flash"
  below): `OpenBt` or `CloseBt`, `GetVersion`, `GetBuildTime`, `GetStatus`,
  `GetName` until `0xC6`; then `SetName(device.name)` until `0x8C` if the
  name differs, `AutoReconnect` (1 when on, 0 when off; waved has no Auto
  Reconnect setting) if this boot has not sent it and no phone is
  connected, `SaveFlash` after either (and `GetName` again after a rename);
  then `PtaMode(1)` (the Wave always has its Wi-Fi chip) and waved's
  `GetPhoneMac`. Stock's 10 s of `0xD0 01` volume
  events before it are left out. It acknowledges every `75 45` frame but `0xE0` with
  stock's bytes, and no `75 46` frame (`command::ack_for_frame`; see
  "Acknowledgements and replies"), and polls `GetStatus` every 5 s once the
  module has answered. The loop runs every 100 ms for the bus and the
  countdown. A log line shows at info only when its frame brought something
  new; repeats go to debug.
- **Pair** (`Command::BtPair`, Bluetooth screen "Pair a device", and "Pair
  another phone" while one is connected): `AutoReconnect(false)`,
  `Disconnect`, `EnterPairing`, back to back as stock's "Connect New Device"
  (which runs with the reconnect off; the screen asks first when a phone is
  connected). `BluetoothState::pairing_left_s`
  counts 120 s down. The window ends at 0, when a phone connects (a new
  connection, not the old phone still dropping), when `0xB2` bit 3 clears
  after it was set, or on `BtStopPairing` (leaving the Bluetooth screen).
  Nothing is sent at the end: no command leaves pairing mode.
- **Disconnect** (`BtDisconnect`): `AutoReconnect(false)`, then `Disconnect`,
  so the module does not bring the phone straight back.
- **Calls** (`BluetoothState::call`, `BtCall`, `call.rs`): `0xB1` with a
  number rings, `0xD1 00` makes the call active while a number is known (as
  stock), and every sign in "Calls in waved" ends it. `0xB2` bit 2 alone
  starts no call (an in-band ring tone opens the audio link while ringing).
  `BtAnswerCall` sends button event 3 only while ringing and shows "In a
  call" at once; `BtDeclineCall` sends event 0 while ringing (decline) or
  active (hang up) and ends the call at once, as stock's buttons do. Both
  carry `time(NULL)`.
- **The switch** (`SetBluetooth`): see "What waved does with this".
- **Volume** (`0xD5`): see "Volume".
- **`0xAE`** decodes to `Event::PbapStatus`, logged at debug; waved never
  asks for the phone book.
- **Disconnects:** whatever says the phone is gone (`0xB2` bit 0 clear,
  `0xAA 00`, `0xE1` with its MAC, three `0xB4` reports without a phone) also
  clears the call, `phone_audio` and `call_audio`, so I2 closes `hw:0,0`.
  `pairing` (bit 3) is the module's own and stays.
- **Name:** `SetName(device.name)` at start when the module's name differs;
  `0xC6` fills `BluetoothState::advertised_name`, which the Bluetooth screen
  shows ("Shows up as …", and large while pairing). While it still contains
  `XXXXXX` the 5 s poll sends `SetName` and `GetName` again, as stock, but
  within the boot's 3 tries for that name.
- **Privacy:** the phone book (`0xB0`) is dropped unread. No log line at any
  level carries a phone's name, MAC or a number (`log_line`, tested on every
  event that carries one). Module log lines (`0xF0`) show only as one of the
  module's fixed messages about pairing, reconnecting, its radio, calls,
  call audio, volume and the phone book's link, at debug
  (`bt module: RECONN_START!`, `bt module: HFP call end`), never as received;
  any other line shows as
  its length at trace. Undecoded frames show their command byte and length
  only, and so does the `waved-uart` reader's trace line (plus the prefix).
  `BtCall`'s `Debug` hides the number.

It saves the module's flash only at start, and only when that start wrote a
name or the reconnect switch (stock also saves after each connect and at
power off: not done), sends no `0xB3` heartbeat or `PhoneMode`, and never
flashes (`waved-bt-flash` does, by hand). `waved-uart`'s reader logs the
first frame parse error, then their count at most once a minute
(`waved.log` is on the small `/data`). The plan and the device-pass
checklist are in `docs/superpowers/plans/2026-10-13-m3-i1-i3-i4-bt-control.md`
(third round: T1-T9; review fixes: U1-U6).

### Start-up and the module's flash (review 2026-10-13)

Stock runs `bt_info_init` once per boot: pspapp starts once. waved starts
far more often: `S60waved` kills the first waved of every boot after 5 s
and restarts any waved that exits within ~3 s, so stock's sequence at
every start (`SetName`, `D8`, `SaveFlash`) would save the module's flash at
every restart, about 25,000 times a day in a crash loop; and while the
name held `XXXXXX`, `SetName` + `GetName` went out every 5 s without end.
Whether `SetName` and `D8` write the flash by themselves is unknown; stock
follows them with `SaveFlash` at boot.

waved (`crates/waved-bt/src/startup.rs`):

1. `OpenBt` / `CloseBt`, as the switch is at that moment (stock's switch
   never saves the module, 74413-74473).
2. The reads: `GetVersion`, `GetBuildTime`, `GetStatus`, `GetName` until
   `0xC6` (10 tries, 1.1 s apart).
3. `SetName` until `0x8C`, only when the module's name differs from
   `device.name`, and at most 3 times a boot for that name (`MAX_RENAMES`,
   the start-up and the 5 s poll together).
4. `AutoReconnect`, as the switch is at that moment, only when this boot
   has not sent that value and no phone is connected (stock turns the
   reconnect off, `D8 00`, as each phone connects, 75955, and its
   `bt_info_init` sends `D8 <saved>` regardless at its once-per-boot start).
5. `SaveFlash` only when 3 or 4 sent something, then `GetName` again after a
   rename. Then `PtaMode(1)` and `GetPhoneMac`.

What a start wrote is noted in `/tmp/waved-bt-module` (`startup::Stamp`:
the name in hex, its tries, the `D8` value) before the frames go, so a
waved killed right after sending them does not send them again. `/tmp` is
a tmpfs, emptied at boot, so the first start of every boot configures the
module as stock does. A new `device.name` or switch state is a new value:
written once more. `waved-bt-flash` removes the note after flashing the
module. Each step reads the switch when it comes, so turning Bluetooth off
while the start-up waits is not undone by a queued `OpenBt` or `D8 01`.

**Cost if wrong:** if the module forgets its name or reconnect switch
without a power cycle (a module reset by itself), waved does not write them
again until the next boot or a change of `device.name` / the switch (the
name is still read and retried within the boot's 3 tries). With a phone
connected at the first start of a boot, the module's reconnect stays as its
flash has it (stock would send `D8 01`, then `D8 00` + `SaveFlash` as it saw
the phone). A module that answers `GetName` with spaces after the name, or
cut shorter, is renamed and saved once a boot (T4).

## Implementation outline

### I1: Pairing (`waved-bt`, plus additive state and config)

- **Transport:** give `ttyS1` its own link at 921600, like `waved-mcu`'s
  `link.rs`, reusing `frame::Parser`. Alternatively, keep `UartMux` and fix
  the speed. The port carries only this protocol; the `DC CD` route is dead.
- **Start-up**, like stock's `bt_info_init`:
  - `SetName(device.name)`, 23 bytes;
  - `OpenBt` or `CloseBt` from config;
  - `AutoReconnect(config)`;
  - `PtaMode { wifi: true }`;
  - `GetVersion`, `GetBuildTime`, `GetStatus` and `GetName`.
- **Every 5 s:** `Heartbeat { battery, charging, wifi }` from `BatteryState`,
  then `GetStatus` and `PhoneMode(1)`. After about 3 min of silence, reset
  through btpower; don't re-flash automatically.
- **Bus commands** (TODO A):

  | Command | Frames |
  |---------|--------|
  | `BtSetDiscoverable(true)` | `EnterPairing`, plus a 2-minute timer |
  | end of the 2 minutes | unknown (see Open); `CloseBt` + `OpenBt` is the candidate to measure |
  | `BtDisconnect` | `AutoReconnect(0)` + `Disconnect` (built) |
  | `SetBluetooth(on)` | `OpenBt` / `CloseBt`, host forcing "not connected" while off (stock's switch; built) |
  | `BtForget` | `Disconnect` + `EnterPairing`, and drop the phone from the host-side list |
  | `BtConnect` | `AutoReconnect(1)` (the last phone only) |

- **On a new connection:** `AutoReconnect(0)` and `SaveFlash`, as stock does.
- **Power off:** `SaveFlash` until `0xE0` arrives (at most 10 s), then
  btpower 1. This hooks into `waved-mcu`'s power-off sequence.
- **State** (additive in `waved-state`): the phone's MAC, RSSI, the work
  status bits, the module's version, a `CallState`, and the paired list
  persisted in config.

### I2: Audio routing (`waved-audio`)

Built as `bt_audio` (plan: `docs/superpowers/plans/2026-10-13-m3-i2-bt-audio.md`,
which ends with the device-pass checklist):

- Downstream: `hw:0,0` capture → `PlaybackService::play_phone` (its own queue,
  not `play_batch`: sharing the host's queue made the two take turns and
  crackle, as the tones did). The far end lands in the post-DSP AEC reference
  and in recordings.
- Uplink: processed mono frames, duplicated to 2 ch, → `hw:0,0` playback.
- Start and stop on the `0xB2` bits: bit 1 downstream, bit 2 uplink.
- Arbitration: **mix**, not stock's one source at a time (see Audio (I2)).
- Still to measure on the device: ERLE with phone audio on the speaker, and
  the latency of `hw:0,0`.

### I3: Calls

- **`CallState`:**
  - `Idle` → `Ringing { number }` on `0xB1`;
  - → `Active` on `0xD1 00`, Answer, `HFP  Call active` (not on `0xB2` bit 2
    alone);
  - → `Idle` on `0xD1 01`, an empty `0xB1`, the phone gone and the other
    signs in "Calls in waved" (built).
- **Answer / Decline** commands send `Button { AnswerCall / EndCall,
  unix_time }`.
- **Screens:** an incoming-call screen with the number only, and an
  active-call screen with mute and hang-up.
- **USB HID hook-switch:** driven from `CallState` (separate work).
- **Optional:** send button events 6, 7 and 8 on recording start, pause and
  stop, as stock does.

### I4: Bluetooth screen (`screens/bluetooth.rs`)

- On/off.
- The advertised name.
- The connected phone, with its signal from `0xB4`.
- "Make discoverable", with a countdown.
- The host-side paired list, with Forget.
- Locked behind the PIN (D10).

## Verified on the device (2026-10-13)

waved at `bae1fff`, then with `UART_BAUD = waved_bt::BT_BAUD`:

- **Framing and speed confirmed.** At 921600 raw, the module's first frame was
  `75 45 03 B2 00 00 B5`: prefix `75 45`, `LEN` = 1 + data, `SUM` over
  `LEN CMD DATA`. No junk bytes in 13 KB. (`stty` reports "Inappropriate ioctl"
  for one of the flow-control flags but sets the speed; busybox here has no
  `timeout`, use `cat & sleep; kill`.)
- **The module repeats a frame every 500 ms until it is acknowledged** (180
  identical `0xB2` in 90 s with no host). Our `75 46` acknowledgements stop
  it; replies to our requests come back as `75 46` frames.
- **Firmware 0.0.0.28, built 2025-11-28 20:15:49**: older than 1.2.2.6's
  `BT.bin` (0.0.0.33, F6). Name: `Insta360 Wave BT <six hex digits of the
  serial>`.
- **Pairing:** idle, the module is not discoverable to a new phone. Stock's
  "Connect a new device" sequence, `Disconnect` (`75 45 02 A3 00 A5`) then
  `EnterPairing` (`75 45 02 A7 00 A9`), made it discoverable; an iPhone paired
  and connected. **`0xB2` bit 3 (`0x08`) is pairing mode**; a connected phone
  is `0x01`. A phone paired under stock reconnects by itself.
- **The phone book arrives by itself.** Right after an iPhone connected, the
  module sent `0xB0` frames: one contact each (index, length, number as ASCII,
  name as UTF-16LE). It is PBAP, which stock uses for caller names. waved never
  logs or stores these: `0xB0` decodes to `Event::PhoneBookEntry`, which
  carries nothing, and is still acknowledged ("Phone book (PBAP) sync" is not
  planned).
- **Module log lines (`0xF0`) carry the phone's Bluetooth name and MAC**
  (`app_set_bt_last_addr`). Keep them out of the log at every level: waved
  shows only the module's known fixed messages (see "In waved now").
- **`PhoneMac` decodes one byte off:** `app_set_bt_last_addr A1B2C3D4E557`
  arrived as `06 A1 B2 C3 D4 E5`: the first byte is a length (6).
  `DeviceDisconnected` decodes the same MAC correctly.
- **Audio arrives with no host command:** with the phone playing, `arecord -D
  hw:0,0 -c 2 -r 48000 -f S16_LE` recorded a full-scale signal (peak ≈ 24 000).
  waved routes none of it yet, so the speaker stays silent (I2).

### Build 15f8d8d (2026-10-13) **[device]**

- **The call screen stayed.** A call rang (`0xB1` with the number, "call
  ringing"); the user answered on the iPhone. The module sent no `0xD1` and
  no empty `0xB1`, so the Wave kept showing Answer and Decline after the
  call (fixed: "Calls in waved").
- **In 40 minutes of other use** the module sent only `0xF0` log lines,
  `0xB2` (`0x01`, `0x03`), `0xAA`, the `75 46 A9` answers, `0xA6`
  (`AudioModeSwitched` 2 as A2DP started, 0 as it stopped) and **`0xAE` with
  one data byte, 175 times**: the phone book link's status, unasked.
- **No `0xD5`** in that window; whether the phone's volume was changed then
  is unknown. The iPhone's volume did not change the Wave's level.

## First device session

Run everything below with **waved stopped**, together with its watchdog
(`docs/DISPLAY_HARDWARE.md#deploy`). waved holds `ttyS1`. Never stop the module
with btpower in this session.

### A. Read-only (nothing reaches the module)

```bash
adb shell 'cat /proc/device-tree/aliases/serial1; echo; ls -l /dev/ttyS1'
adb shell 'ls /proc/tty/driver/; cat /proc/tty/driver/*uart*'   # tx/rx counters per port (ttyS1 = line 1)
adb shell 'cat /sys/class/leds/btpower/brightness /sys/class/leds/btpower/max_brightness'   # expect 0 = running
adb shell 'grep -i -B1 -A1 btpower /sys/kernel/debug/gpio'   # only if debugfs is already mounted; don't mount it
adb shell 'cat /proc/asound/cards /proc/asound/pcm'
adb shell 'cat /proc/asound/card0/pcm0p/info /proc/asound/card0/pcm0c/info'
adb shell 'cat /proc/asound/card0/pcm0p/sub0/hw_params /proc/asound/card0/pcm0c/sub0/hw_params'   # expect "closed"
adb shell 'amixer -c 0 contents | head -120'
adb shell 'lsmod | grep -i -E "bt|blue|sdio"; ls /etc/bluetooth /etc/bluetooth/aml'
adb shell 'stty -F /dev/ttyS1 -a'                       # opens the port, sends nothing: current speed
```

### B. Listen only (sends nothing; `stty` changes local settings)

```bash
adb shell 'stty -F /dev/ttyS1 921600 raw -echo -ixon -ixoff -crtscts;
  timeout 60 cat /dev/ttyS1 > /tmp/s1-listen.bin; ls -l /tmp/s1-listen.bin'
adb pull /tmp/s1-listen.bin && xxd s1-listen.bin | head -80
```

During the 60 s, connect a paired phone, play music for 10 s and call the
Wave's phone, letting it ring. Look for `75 45`/`75 46` frames: `0xB2`,
`0xB1`, `0xD1` and `0xF0` logs. This shows whether the module talks
unsolicited, and with which prefix.

### C. First queries (four read-only frames)

These send only `GetVersion`, `GetBuildTime`, `GetStatus` and `GetName`:

```bash
adb shell 'stty -F /dev/ttyS1 921600 raw -echo; cat /dev/ttyS1 > /tmp/s1-q.bin & p=$!; sleep 1;
  printf "\165\105\001\221\222" > /dev/ttyS1; sleep 1;
  printf "\165\105\001\223\224" > /dev/ttyS1; sleep 1;
  printf "\165\105\001\251\252" > /dev/ttyS1; sleep 1;
  printf "\165\105\001\305\306" > /dev/ttyS1; sleep 2; kill $p'
adb pull /tmp/s1-q.bin && xxd s1-q.bin
```

An alternative is to deploy this branch with `UART_BAUD = waved_bt::BT_BAUD`
and read `/tmp/waved.log`, filtering for `bt module`.

### D. Audio, read-only on the module side

Play music on the phone and record from TDM-A (this opens the ALSA capture
only):

```bash
adb shell 'arecord -D hw:0,0 -c 2 -r 48000 -f S16_LE -d 5 /tmp/bt-in.wav; ls -l /tmp/bt-in.wav'
```

### What to record

- The prefix the module uses, and whether it echoes our requests.
- The module's version and build time, which decide F6.
- Any unsolicited frames after start (`0xC4`? `0xB2`?) and the cadence of
  `0xF0`.
- `0xB2` bits for idle, music and a call.
- Whether audio shows up on `hw:0,0` without any host command.
- Then, as the next active steps: whether the Wave is discoverable when idle,
  what `EnterPairing` does and how long it lasts, the answer (3) and decline
  (0) codes on a real call, and the meaning of `ConnectReply` 0 and 1.

## Open

- ~~The module's prefix and echo behaviour~~: it repeats its `75 45` frames
  until acknowledged and answers requests once with `75 46` frames
  ("Acknowledgements and replies"). Still open: whether acknowledging a
  `75 46` answer makes the module repeat the request (the 2 Hz `0xC6`).
- How long the module stays in pairing mode (no command leaves it; waved ends
  its own window after 2 minutes or when the screen is left). The module is
  not discoverable when idle with waved (verified); whether `OpenBt` changes
  that (stock: "Device can be discovered as …").
- What `ConnectReply` 0 and 1 mean, and when `0xD6` is sent.
- What `0xD8` means exactly: **[inferred]** 1 starts and 0 stops the
  module's reconnect to the last phone; 2 (factory reset) unknown.
- Whether `CloseBt` drops the phone at all (it stopped the music, the phone
  stayed "connected" for at least 10 s) and whether the module's `0xB2`
  bit 0 ever clears while closed.
- The unknown bytes in `0xB2`, `0xB3`, `0xB4`, `0x91`/`0x93` and `0xC6`, and
  `0xB2` bits 3 and 7.
- Button codes other than 0, 3, 4, 6, 7 and 8, including a "clean link info"
  code (forget).
- Whether a shorter, NUL-padded `SetName` is accepted, and whether `0x8C`
  answers it (T4).
- Whether the module sends its HFP log lines (`HFP  Call active`,
  `HFP call end`) over the UART, and the `0xB2` bits during a ring and a
  call (T1).
- Whether `0xD5` arrives from an iPhone with module 0.0.0.28 (T2), and what
  stock's `0xD0 01 / 02 / 09` volume events do (T3).
- What `OpenBt` at every start does to a phone already connected (T5; `D8`
  is no longer sent while one is). Whether `SetName` or `D8` write the
  module's flash without `SaveFlash`.
- MAC byte order.
- The module's exact Telink part.
