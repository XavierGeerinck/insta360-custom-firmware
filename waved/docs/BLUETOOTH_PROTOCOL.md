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

**Nothing here has been captured on the wire yet.** Every fact carries a tag:

- **[code]**: read in decompiled code.
- **[file]**: read in an extracted file (binary data, strings, the device
  tree, configuration).
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
5. The host sends requests with the `75 45` prefix. It acknowledges *every*
   received frame with `75 46 <same CMD>` (and `00`), except `0xE0`. **[code]**
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
  1. For 10 s it calls `bluetooth_set_volume` once a second. In 1.2.2.6 that
     sends nothing on the wire: `bt_ctrl_cmd_send` has no volume command.
     **[code]**
  2. `bt_info_init` (79403) sends, in order:
     - `SetName`, retried up to 10 × 1 s until `0x8C` arrives.
     - `OpenBt(at_boot=1)` if the saved switch is on, otherwise `CloseBt`.
     - `AutoReconnect(saved)`.
     - `SaveFlash`, `GetVersion`, `GetBuildTime`, `GetStatus`.
     - `GetName`, retried until `0xC6` arrives.
     - `PtaMode`: 1 when a Wi-Fi chip exists (Wi-Fi/BT coexistence;
       **[inferred]** from the name).
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
- **Open:** which prefix the module puts on its replies and events.

## Commands (host → module)

`waved_bt::command::Command` encodes the commands marked "yes" in the last
column, byte for byte.

| Byte | Stock name | Data | Reply | Stock use | waved |
|------|-----------|------|-------|-----------|-------|
| `0x8B` | CTRL_SET_NAME | the name, 23 bytes | `0x8C` | at boot; on each heartbeat while unnamed | yes |
| `0x91` | CTRL_GET_VERSION | — | `0x91` `[?] ascii` | at boot | yes |
| `0x93` | CTRL_GET_BUILDTIME | — | `0x93` `[?] ascii` | at boot | yes |
| `0x97` | CTRL_BT_PHONE_MODE | mode (stock always sends 1) | `0x97` | every 5 s | yes |
| `0x9A` | CTRL_GET_MAC | — | not handled | — | yes |
| `0xA3` | CTRL_BT_ENABLE (used as `bt_disconnect`) | `00` | — | Disconnect | yes |
| `0xA5` | CTRL_SWITCH_AUDIO_MODE | mode (stock sends 0) | `0xA6` `[?] [mode]` | test only | yes |
| `0xA7` | CTRL_REPAIR (`bt_repair`) | `00` | — | pairing mode | yes |
| `0xA9` | CTRL_GET_BT_STATUS | — | `0xAA` | at boot; every 5 s | yes |
| `0xAB` | CTRL_GET_24G_STATUS | — | — | unused | yes |
| `0xAD` | CTRL_GET_PBAP_STATUS | — | `0xAE` | phone book | no (D: number only) |
| `0xAF` | CTRL_GET_PHONE_BOOK | — | one `0xB0` per contact | phone book | no |
| `0xB3` | CTRL_GET_RSSI (heartbeat) | battery step (`level/10`, minus 1 unless 0), charging 0/1, Wi-Fi present 0/1 | `0xB4` | every 5 s | yes |
| `0xC5` | CTRL_GET_NAME | — | `0xC6` | at boot | yes |
| `0xC7` | CTRL_GET_PHONE_MAC | — | `0xC8` | — | yes |
| `0xCB` | CTRL_CLOSE_BT | — | — | switch off | yes |
| `0xCC` | CTRL_OPEN_BT | `is_init` (1 at boot, 0 from the switch) | — | switch on | yes |
| `0xCD` | CTRL_24G_ENTER_PAIRING | mode (`ctrl_bt_pair_mode`: 1 to pair, 2 to unpair the dongle) | — | dongle | no |
| `0xCE` | CTRL_24G_DISCONNECT | `00` | — | dongle | no |
| `0xD0` | CTRL_BUTTON_EVENT | event, `time(NULL)` as a u32 LE, reset level (0) | `0xD0` | call buttons; recording state | yes |
| `0xD7` | CTRL_CLOSE_CONNECT_REPLY | reply | — | answers `0xD6` | yes |
| `0xD8` | CTRL_RECONNECT_SWITCH | 0/1 | — | at boot (the setting); 0 after each connect | yes |
| `0xE0` | CTRL_BT_CONFIG_SAVE_FLASH | `00` | `0xE0` | at boot, after a connect, at power off | yes |
| `0xE2`–`0xE5` | LL/BT data test modes | 2–3 bytes | — | factory | no |
| `0xE6` | GET_DONGLE_VERSION | `00` | `0xE8` | update | yes |
| `0xE7` | GET_DONGLE_BUILDTIME | `00` | `0xE9` | — | yes |
| `0xEA` | SET_PTA_MODE | Wi-Fi present 0/1, then `00` | `0xEB` | at boot | yes |
| `0x80` `0x9D` `0x9F` `0xA1` `0xA2` | module OTA | see Firmware | | update | `ota` module |
| `0xD9` `0xDB` `0xDD` `0xDF` | dongle OTA | see Firmware | | update | `ota` module |

### Button events (`0xD0` byte 0)

| Code | Sent by stock when | Source |
|------|--------------------|--------|
| 0 | hang up or decline: the red button on the incoming-call screen and on the calling screen | **[code]** ui 27933, 28725 |
| 3 | answer: the green button on the incoming-call screen. Also "music button 0". | **[code]** ui 27968, 78679 |
| 4 | "music button 1" (meaning unknown) | **[code]** ui 78679 |
| 6 | a recording was set up or started | **[code]** ui 116516, 116565 |
| 7 | a recording paused | **[code]** ui 116614 |
| 8 | a recording finished | **[code]** ui 116663, 116712 |

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
| `0xA6` | audio mode switched | `[?] [mode]` | `BT_SWITCH_MODE` | **[code]** 75679 |
| `0xAA` | status | `[connected]`, where 0 = no phone | `BT_CONNECT_STATUS` 1 / 0 | **[code]** 75696 |
| `0xAE` | PBAP state | `[ready]` | if 1, asks for the phone book | **[code]** 75716 |
| `0xB0` | phone-book entry | `[?×4] [n] number[n] [m] name[2m]` (UTF-16?) | `add_contact`, saved to `/tmp/PHONE_BOOK.bin` | **[code]** 75722 |
| `0xB1` | call number | ASCII; empty means the number is gone | `BT_PHONE_COMING_NUM` (plus a name from the phone book) → incoming-call screen | **[code]** 75739, ui 102913 |
| `0xB2` | work status | `[bits] [?] [n] name[n] [?] [?] mac[6]`; name and MAC only matter when bit 0 is set | `BT_WORK_STAUS` → audio routing. On a new connection: status 0, name, MAC, `AutoReconnect(0)`, `SaveFlash`. Falls through into the `0xD0` handler. | **[code]** 75767-76013 |
| `0xB3` | heartbeat / connected phone | `[?] [n] name[n] [?] [?] mac[6]` | liveness; stores a changed name or MAC | **[code]** 76014 |
| `0xB4` | link report | `[dongle 0 or FE] [dongle RSSI i8] [2.4 GHz RSSI i8] [phone 0 or FE] [phone RSSI i8] [?] [tx u32 LE] [rx u32 LE]`; needs more than 5 bytes | liveness and RSSI. Phone 0 → connected; FE three reports running → disconnected | **[code]** 76195 |
| `0xBD` | dongle info | `[?] mac[6] sn… uuid… [freq] code[31]`; needs 30 bytes or more | stores DONGLE_MAC, SN, UUID, FREQ and CODE | **[code]** 76351 |
| `0xC4` | module info | `mac[6] uuid[16] sn[32] [freq]`; needs more than 9 bytes | stores BT_MAC, UUID, SN and FREQ (waved decodes the MAC only) | **[code]** 76754 |
| `0xC6` | name | `[?] name[23]` | `BT_NAME`, saved | **[code]** 76953 |
| `0xC8` | phone MAC | `[?] mac[6]` | `BT_DEVICE_PHONE_MAC` | **[code]** 76980 |
| `0xD0` | button taken | — | ack flag | **[code]** 76008 |
| `0xD1` | phone event | `[event]`: 0 = call up, 1 = call ended (both only while a number is stored), 2 = start, 3 = restart, 4 = pause, 5 = stop a recording | calling screen; recording control | **[code]** 76991, ui 85228 |
| `0xD4` | dongle keys | `[?] [?] [speaker mute] [volume] [mic mute]` | mute and volume | **[code]** 77033 |
| `0xD5` | phone volume | `[kind: 0 music / 1 call] [scale: 0 = 15 steps / 1 = 16] [level]` | stored as 0–100 in steps (`map_volume_100`) | **[code]** 74219 |
| `0xD6` | connection request | `[6] mac[6] [n] name[n]` | in factory mode, `ConnectReply(0)`. Otherwise `BT_CONNECT_STATUS = 2` and stores the name and MAC. | **[code]** 77078 |
| `0xE0` | flash saved | — | flag; never acknowledged | **[code]** 77151 |
| `0xE1` | phone dropped | `mac[6]` | disconnected, if it is the current phone | **[code]** 77158 |
| `0xE8` / `0xE9` | dongle version / build time | `[FE = no dongle, or ?] ascii` | stored | **[code]** 77174 and onwards |
| `0xEB` | PTA mode set | — | ack flag | **[code]** |
| `0xF0` | module log | `[1] text`, or `[2] raw bytes` | syslog | **[code]** 77363 |
| `0x80` `0x9E` `0xA0` `0xA1` `0xA2` `0xDA` `0xDC` `0xDE` | OTA replies | see Firmware | | **[code]** |

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

## Pairing and connection (I1)

- **Pairing mode:** `EnterPairing` (`0xA7 00`). Stock's "Connect a new device"
  sends `Disconnect` (`0xA3 00`), then `EnterPairing` (ui 115166).
  **[code]** The module logs "BT set pairing mode by user". It refuses while
  connected or reconnecting ("Warning! bt set pairing but bt has been
  connected", "…while bt is reconnecting"). **[file]** No command to leave
  pairing mode is known, and the module's own timeout is unknown.
- **Discoverable when idle:** whether the module is discoverable with no phone
  connected is unknown. Stock offers no pairing button outside "Connect a new
  device", so first pairing presumably works out of the box. **[inferred]**
- **Paired devices:** the module keeps the link keys itself
  (`app_write_stored_linkkey`, `flash_clear_last_dev_func`, "linkkey change,
  clean old services"). **[file]** The protocol has no command to list,
  connect to or forget a device. The host only learns about the connected
  phone, from `0xB2`, `0xB3`, `0xC8` and `0xD6`. **[code]** **[inferred]**
  from that:
  - a paired list has to be host-side memory of the phones seen;
  - "connect" means letting the module reconnect (`AutoReconnect(1)` at
    boot, which reaches only the last phone);
  - "forget" can only be stock's "Connect a new device" (`Disconnect` then
    `EnterPairing`). Whether that erases the old link key is unknown.
- **Auto reconnect:** stock sends the saved setting at boot. Each time a phone
  connects it sends `0xD8 00`, then `SaveFlash` (75955, 76006). So `0xD8`
  probably means "try to reconnect now" more than a stored setting.
  **[code] [inferred]**
- **Connection request:** `0xD6` puts stock in the "connecting" state. The
  answer is `ConnectReply` (`0xD7 [reply]`). In 1.2.2.6 the UI never sends
  one; only the factory/aging path replies, with 0. Stock treats reply 1 as
  "now disconnected" (74485). So **[inferred]**: 0 accepts and 1 refuses.
- **Name:** `SetName` always sends exactly 23 bytes. waved NUL-pads a shorter
  name and cuts a longer one at a character boundary. Whether the module
  accepts other lengths is unknown.
- **On/off:** `OpenBt` / `CloseBt`. The host persists the switch
  (`INSUTIL_CFG2_BT_IS_OPEN`). **[code]**

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
- **Volume:** the phone reports its volume in `0xD5`. Stock stores it as
  `BT_MUSIC_VOLUME` / `BT_PHONE_VOLUME` and turns it into the speaker volume
  (`audio_listen_bt_spk_volume_event` → `audio_server_spk_volume_policy`).
  **[code] [inferred]** for the link between the two. Nothing sends the
  Wave's volume to the phone in 1.2.2.6. **[code]**

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
| packets | `0x9F` → `0xA0`; byte 0 = 3 is an error (up to 21 retries per packet, 3 s) | `0xDB` → `0xDC`; non-zero is an error (1 s) |
| CRC check | `0xA1 01` → `0xA1`; byte 0 = 1 passes (20 tries, 5 s) | `0xDD 01` → `0xDE`; non-zero is an error (10 s) |
| reset | `0xA2 01`, then 1 s sleep | `0xDF 01` |

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

1. **Find out what the module runs.** The new `BtService` logs
   `bt module firmware …` and `bt module built …` once it talks at 921600.
2. **Ship the 1.2.2.6 blobs** with waved (for example `/usr/lib/BT.bin` as
   stock does, or under `/data/waved/fw/`). Flash only when the module's
   version **and build time** differ from the blob; stock instead flashes on
   every update.
   - `waved_bt::ota::Image` checks a blob the way stock does and splits it into
     packets.
   - The send loop with stock's waits and retries is still to write.
3. **Flash with audio idle and on USB power**, in a device session of its
   own. The 0x8000-byte boot area is never sent, so a failed flash should
   leave the Telink bootloader able to take the image again. **[inferred]**
   The btpower reset is the recovery.
4. **Dongle:** flash only when it is connected (`0xB2` bit 4) and its `0xE8`
   version differs.
5. Like the MCU, BT.bin did not change between 3.0.0.1 and 1.2.2.6 except in
   its build. Without a changelog, F6 is low priority unless the module
   misbehaves.

## Example frames

Computed with the stock rule. They are also the unit-test fixtures in
`crates/waved-bt`.

| Frame | Bytes |
|-------|-------|
| GetStatus | `75 45 01 A9 AA` |
| GetVersion / GetBuildTime / GetName | `75 45 01 91 92` / `75 45 01 93 94` / `75 45 01 C5 C6` |
| OpenBt at boot / CloseBt | `75 45 02 CC 01 CF` / `75 45 01 CB CC` |
| Disconnect / EnterPairing | `75 45 02 A3 00 A5` / `75 45 02 A7 00 A9` |
| AutoReconnect on | `75 45 02 D8 01 DB` |
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
| `state.rs` | folds events into `BluetoothState` and `BtConnected` / `BtDisconnected` |
| `ota.rs` | firmware images and OTA packets |

`BtService` keeps its constructor and still runs on the `waved-uart` mux. It
is passive:

- at start it sends only `GetVersion`, `GetBuildTime`, `GetStatus`, `GetName`
  and `GetPhoneMac`;
- it acknowledges every frame as stock does;
- it polls `GetStatus` every 5 s, but only after the module has answered once;
- it logs the version, build time, name and call events, but never the
  caller's number.

It doesn't switch Bluetooth on or off, rename the module, pair or save the
module's flash.

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
  | `BtDisconnect` | `Disconnect` |
  | `BtForget` | `Disconnect` + `EnterPairing`, and drop the phone from the host-side list |
  | `BtConnect` | `AutoReconnect(1)` (the last phone only) |

- **On a new connection:** `AutoReconnect(0)` and `SaveFlash`, as stock does.
- **Power off:** `SaveFlash` until `0xE0` arrives (at most 10 s), then
  btpower 1. This hooks into `waved-mcu`'s power-off sequence.
- **State** (additive in `waved-state`): the phone's MAC, RSSI, the work
  status bits, the module's version, a `CallState`, and the paired list
  persisted in config.

### I2: Audio routing (`waved-audio`)

- **`BtBridge`**, modelled on `UacBridge`:
  - Downstream: read `hw:0,0` capture (2 ch S16 at 48 kHz, 480-frame
    periods) and feed `PlaybackService::play_batch`. The far end then lands in
    the post-DSP AEC reference tap automatically, as UAC audio already does.
  - Upstream: take processed mono frames, duplicate them to 2 ch and write
    them to `hw:0,0` playback (960-frame periods).
- **Start and stop** on the `0xB2` bits: bit 1 starts downstream, bit 2
  upstream.
- **Arbitration:** follow stock's one-source-at-a-time rule against UAC. A
  call should win over USB playback; that is a product decision.
- **Measure:** ERLE with phone audio on the speaker, plus the latency of
  `hw:0,0`.

### I3: Calls

- **`CallState`:**
  - `Idle` → `Ringing { number }` on `0xB1`;
  - → `Active` on `0xD1 00` (or `0xB2` bit 2);
  - → `Idle` on `0xD1 01`, an empty `0xB1` or `0xB2` bit 0 cleared.
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

- The module's prefix and echo behaviour (B, C above).
- How to leave pairing mode, and whether the module is discoverable when idle.
- What `ConnectReply` 0 and 1 mean, and when `0xD6` is sent.
- What `0xD8` means (stock clears it on every connect).
- The unknown bytes in `0xB2`, `0xB3`, `0xB4`, `0x91`/`0x93` and `0xC6`, and
  `0xB2` bits 3 and 7.
- Button codes other than 0, 3, 4, 6, 7 and 8, including a "clean link info"
  code (forget).
- Whether a shorter `SetName` is accepted.
- MAC byte order.
- The module's exact Telink part.
