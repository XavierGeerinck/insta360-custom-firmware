# MCU Protocol — `/dev/ttyS4`, MAVLink v2

The Wave's microcontroller (battery, charging, the camera-lift motor, the status
LEDs, the power key, power off) talks to the SoC over **`/dev/ttyS4` at 115200 baud,
8N1, raw**, using a private **MAVLink v2** dialect. MAVLink is the open
message protocol from drone autopilots (PX4, ArduPilot); here it is only the
framing and message format on a local serial line inside the device.

Sources: the stock `libins_entry.so` / `libui.so` (decompiled in
`../firmwares/fw_v3.0.0.1/decompiled/`, line numbers below refer to
`libins_entry.so.c` unless marked `ui:`), the stock `/usr/bin/mcutool`, and
captures on the device on 2026-10-12. **Verified on device** marks what a
capture confirmed; the rest is from the stock code.

## Which UART is which

| Device | Speed | Peer | Stock name |
|--------|-------|------|------------|
| `/dev/ttyS4` | 115200 | **MCU** (this document) | `g_uart_mcu_device` "UART_MCU" (`.data` 0x28ac20) |
| `/dev/ttyS1` | 921600 | Bluetooth module (`75 46` / `75 45` / `DC CD` + len + sum8 framing, `bt_cmd_parser`) | `g_uart_bt_device` |
| `/dev/ttyS3` | 115200 | camera module ("PUC") | `g_uart_puc_device` |

Same mapping in every stock version with a library (1.0.13.6, business 1.0.2.6,
1.2.2.6, 3.0.0.1). **waved talked to `ttyS1` at 115200 until 2026-10-12**, i.e.
to the Bluetooth chip at the wrong speed: its motor, shutdown and battery frames
never reached the MCU (power off only restarted waved), and the "`0x4B`
heartbeat" with low-bit "corruption" on `ttyS1` (`project_ghost_volume`) is most
likely the Bluetooth module's 921600 traffic read at 115200.

## Framing (verified on device)

```
FD len incompat compat seq sysid compid msgid[3, LE] payload[len] crc[2, LE]
```

- `incompat = compat = 0`, `sysid = compid = 0` in both directions; `seq`
  increments per sender (wraps at 255).
- **Trailing zero bytes of the payload are trimmed** (at least 1 byte stays), so
  `len` is shorter than the message's full length; the receiver zero-fills.
- **CRC: CRC-16/MCRF4XX** (poly 0x1021 reflected, init 0xFFFF, the MAVLink
  `crc_accumulate`) over every byte after `FD` up to the end of the payload,
  then over the message's `crc_extra` byte. All 17 MCU frames captured on
  2026-10-12 and the stock host frames check out.
- The stock parser also accepts MAVLink v1 (`FE`); the MCU sent only v2.

| msgid | Message | Direction | Full length | crc_extra |
|-------|---------|-----------|-------------|-----------|
| 0 | heartbeat_control | MCU → host, every ~2 s | 32 | 0x45 |
| 1 | heartbeat_notify | host → MCU, reply to each heartbeat | 32 | 0x20 |
| 100 | mcu_control | host → MCU (commands) | 36 | 0x27 |
| 101 | mcu_notify | MCU → host (replies, events) | 36 | 0x3A |
| 102 / 103 | ota_control / ota_notify | firmware update | 140 / 68 | 0xA9 / 0x15 |

`mcu_control` / `mcu_notify` payload: `u32 cmd` (LE), then `params[32]`. Most
commands put a **sub-command in `params[0]`** and data in `params[1..16]`
(LED_CTRL and BUILD_TIME have no sub byte).

### Example frames (seq 0)

| Frame | Bytes |
|-------|-------|
| Heartbeat reply (32 zero bytes, trimmed to 1) | `FD 01 00 00 00 00 00 01 00 00 00 FC 8F` |
| Build time query (cmd 0x0F) | `FD 01 00 00 00 00 00 64 00 00 0F 6C FF` |
| Motor to 50 mm (cmd 3 sub 0, POS) | `FD 11 00 00 00 00 00 64 00 00 03 00 00 00 00 02 00 00 00 32 00 A0 0F B8 0B 32 19 B9 86` |
| Power-off timer START (cmd 5 sub 1 data 1) | `FD 06 00 00 00 00 00 64 00 00 05 00 00 00 01 01 4D C7` |

The CRC covers `seq`, so every frame must be re-checksummed when `seq` changes.

## Heartbeat (msgid 0, MCU → host, verified on device)

| Bytes | Type | Field | Stock use (82912–83260) |
|-------|------|-------|-------------------------|
| 0–3 | i32 | battery current, mA (sign unknown) | stored |
| 4–5 | u16 | battery level, % | ignored if ≥ 111; a jump of ±6 or more from the last accepted value is dropped |
| 6–7 | u16 | battery voltage, mV | stored |
| 8–9 | i16 | battery temperature, °C | alarm after 5 heartbeats outside −20…70 |
| 10–13 | u32 | charge time (unit unknown) | unused |
| 14–15 | u16 | USB input current, mA | low-battery check |
| 16–31 | — | reserved | — |

Captured on 2026-10-12 on USB power: 0 mA, 100 %, 4176–4178 mV, 35–36 °C, USB
256–544 mA (varies frame to frame). The host answers **every** heartbeat with
heartbeat_notify (32 zero bytes, `seq` incrementing); stock reports the link
lost after 5 s without a heartbeat (81205).

## The power key (verified on device)

The key is not a Linux input: pressing it moves no input event, ADC, GPIO or
interrupt (`vad_keypad` on `event0` is the SoC's voice-activity wake). The MCU
reports it as `mcu_notify` **cmd 0x0E (MISC_CHECK), `params[0]` = 0**,
`params[1]` = the event:

| `params[1]` | Event | Captured 2026-10-12 |
|-------------|-------|---------------------|
| 0 | PRESSED (key down) | before every gesture |
| 1 | CLICKED | short press |
| 2 | DOUBLE_CLICKED | double press |
| 3 | (ignored by stock) | — |
| 4 | LONG_PRESSED_1s_DONE (still held at 1 s) | hold ~1.5 s |
| 5 | LONG_PRESSED_1s_2s_UP (released between 1 and 2 s) | hold ~1.5 s |
| 6 | LONG_PRESSED_2s_DONE (still held at 2 s) | — |
| 7 | LONG_PRESSED_2s_5s_UP (released between 2 and 5 s) | — |

A click is `PRESSED` then `CLICKED`; a 1.5 s hold is `PRESSED`,
`1s_DONE`, `1s_2s_UP`. Holding about 3 s cuts power in the MCU itself.
The events arrived while the host answered heartbeats; the MCU did not resend
them without an acknowledgement (stock sends cmd 0x0E sub 0 three times after
each event, not required). A click or double click does nothing on its own
(the motor did not move). Stock's reactions (`ui:`): PRESSED wakes the screen,
CLICK → motor to 0 mm, DOUBLE_CLICK → 50 mm, 2 s hold → power off.

**A hold of 2 s or more powers the unit off in the MCU itself** (captured
2026-10-12, a 2.5 s hold): after `2s_5s_UP` the MCU reports motor state WORK
(cmd 4 sub 2 = 1), then `POWER_CHECK` (cmd 6, `params` `00 02`), then the
lift's position as it retracts (cmd 4 sub 1: 40, 30, 0 mm), and cuts power
(the next boot is a `cold_boot`), without any command from the host. waved
therefore treats `2s_DONE` as "powering off" (no confirmation) and uses the
seconds before the cut to stop the services.

## Commands (host → MCU, `mcu_control`)

Every msgid 101 from the MCU with the same `cmd` counts as the reply; the host
keeps its `params[0..15]`. Stock (`mcu_ctrl_cmd_send`, 80975) waits
`mcu_timeout[cmd]` (1000 ms for cmd 4, 500 ms for cmd 5, 100 ms otherwise)
and sends at most 3 times.

| cmd | Name | `params[0]` sub → data |
|-----|------|------------------------|
| 1 | LED_CTRL | no sub: `[0]` pattern, `[1]` sub-pattern, `[2]` LED count, `[3]` colour, `[4]` target, `[5..7]` 0 (stock's status LEDs, "Status LEDs" below) |
| 3 | MOTOR_CTRL | **0 move** (below); 1 sensor on/off; 2 sensor reset enable; 3 sensor-up enable; 4 start height (0 = 50, 1 = 40, 2 = 30 mm); 5 clear data; 7 calibration config; 0x0B / 0x0C calibration min / max (u16); 0x0D position thresholds (4 × i16); 0x0E rise time (i16) |
| 4 | MOTOR_CHECK | no params = motor info; 2 motor state; 3 sensor self-test; 6 hall calibration; 7 / 8 calibration data; 0x0C / 0x0D min / max; 0x0E thresholds; 0x0F rise time |
| 5 | POWER_CTRL | **0 power**, data: 0 SoC shutdown, 1 SoC reset, 2 camera-module shutdown, 3 camera-module reset, **4 all shutdown**, 5 all reset; **1 power-off timer**, data: 0 stop, 1 start, 2 start (motor) |
| 6 | POWER_CHECK | 1 |
| 7 | CHARGE_CTRL | 0 OTG charge type; 2 charging off / on (0 / 1); 5 quick charge (data 1) |
| 8 | CHARGE_CHECK | 1 → reply `params[1]` = charge type |
| 9 | USB_CTRL | 0 VBUS off / on; 1 USB reconnect; 2 "a PC is connected" |
| 0x0A | USB_CHECK | 0 USB state (reply `params[1]` 0 = removed); 1 CC; 5 OTG end |
| 0x0C | PUC_CHECK | 0 camera-module state (0 removed, 1 handshake, 4 inserted, 5 old version); 1 position level |
| 0x0D | MISC_CTRL | 2 factory flag; 4 hall reverse. **Never send type 0** (heartbeat-loss detection: stock never calls it, and `MCU.bin` contains "SOC heartbeat lost reboot soc") |
| 0x0E | MISC_CHECK | 0 button acknowledgement; 1 battery status (reply `params[4]`); 2 state dump (reply `params[1..12]`: LED mode, motor state, motor position, position state, motor output i16, PM state, charge state, charge type, camera state, USB state, PD type) |
| 0x0F | BUILD_TIME | no params; reply is ASCII. `mcutool vers` (verified): `app_ver: APP_0.1.3.3`, active slot 2 |

After POWER_CTRL sub 0 with data other than 2 or 3, stock suspends the SoC
(`echo mem > /sys/power/state`, 81098).

### Motor move (cmd 3 sub 0)

| `params` bytes | Field | Stock values |
|----------------|-------|--------------|
| 1–4 | mode, u32 | 0 PWM, 2 POS, 3 DATA, 4 SENSOR, 5 SELF_DEMARCATE |
| 5–6 | target, i16 | POS: height in mm (0 / 30 / 40 / 50); PWM: speed, negative = up |
| 7–8 | up current limit, i16 | 4000 |
| 9–10 | down current limit, i16 | 3000 |
| 11 | high speed, i8 | 50 (40 when target is 0) |
| 12 | low speed, i8 | 25 (20 when target is 0) |

`mcutool pos` uses a different, older layout; don't copy it.

Stock's Mic height (`ui:` `mcu_server_request_motor_ctrl_pos`) has three stops,
**Low 30 mm, Medium 40 mm, High 50 mm**; 0 mm is the retracted lift (power
off, and stock's power-key click). Choosing one sends the move and then
**MOTOR_CTRL sub 4** with the start height (`params[1]`: 0 = 50, 1 = 40,
2 = 30 mm, `mcu_init_pos_ctrl`), which the MCU keeps and raises the lift to at
power-on; stock also saves it in its config. The factory default is 40 mm
(`mcu_factory_init`). After a stall (BLOCK) stock re-sends the move by itself
at speeds 80 / 45 (`mcu_motor_block_pos_control`, not for 0 mm, at most once per
10 s); a stop is a PWM move with speed 0 (`mcu_server_request_height_stop`).

### Motor notifications (`mcu_notify` cmd 4)

- sub 2: `params[1]` motor state: 0 IDLE, 1 WORK, 2 BLOCK, 3 WAIT_DATA,
  4 SENSOR_TRIGGER (obstructed: aborts a power off). On BLOCK stock re-sends
  the move (speeds 80 / 45, at most once per 10 s, 80911). On SENSOR_TRIGGER
  stock's UI shows its "anti-hand" (pinch) reminder (`ui:`
  `process_ntf_mcu_height_block`).
- sub 1: the lift's position in mm, sent while it moves (captured 2026-10-12
  during a power-key retract: 40, 30, 0). waved reads `params[1..2]` as an
  i16; that it is two bytes is assumed.
- No params (motor info): i16 `[1..2]` current, `[3..4]` force3,
  `[5..6]` position, `[7..12]` hall A/B/C, `[13..16]` force1/force2.

## Physical controls (stock 1.2.2.6, researched 2026-10-13)

Line numbers in this section and the next are **1.2.2.6** (`../firmwares/fw_v1.2.2.6/decompiled/`,
`libins_entry.so.c` unless marked `ui:`); **[code]** is read from stock code or data, **[file]**
from a stock binary (device tree, `MCU.bin`, `.data` of `libins_entry.so` decoded with its DWARF),
**[inferred]** is a conclusion the device pass should confirm.

**The power key is the Wave's only physical key; it has no volume buttons.**

- [code] The MCU reports one key: `mcu_notify` cmd 0x0E sub 0 (`mcu_msg_recv_thread` 80633–80800
  maps `params[1]` 0–7 to `MCU_BUTTON_*`; any other value logs "mcu_misc_notify recv an unknow
  state", 80810). No other cmd or sub carries a key.
- [code] The factory key test (`_cmd_handler` "button", 88063–88080) waits for
  `MCU_BUTTON_PRESSED` only; stock's key callbacks (`ui:` `button_clicked_cb` 102030 and the
  double-click one after it) only move the lift.
- [file] Device tree (`dtb.img`): no `gpio-keys` / `adc-keys` node. The only key-bearing node is
  `i2c@6a000/aml_pca9557@1f` (compatible `"aml, ledring"`, `key_name = "mute", "pause", "vol+",
  "vol-"`), which is **`status = "disabled"`**: the Amlogic AV400 reference board's key and
  LED-ring daughterboard, not fitted. The IR receiver (`ir@8000`, the reference remotes' keymaps)
  and `state_led` are disabled too.
- [code] The SAR ADC is read only for the hardware version (channel 0, `info_init` 138987) and
  the board temperature (channel 3, `temperature_read_board` 139311), not for keys.
- [code] Stock's `user_request_volume_up/down` (194746, 194797) send HID consumer keys **to the
  computer** (`uac_speaker_volume_control_server_func` 72953 steps the PC's volume towards the
  one set on the screen); they are not inputs.
- [file] Stock's own help text: "Mic and volume can be controlled on the screen"
  (`language_EN.json` `guide_2b_help_vb_usage`).

`docs/AUDIO_HARDWARE.md` once mentioned "pressing the physical Volume Up button"; nothing in stock
backs a volume key, so that test pressed something else (TODO F1 is closed as "not on this
hardware").

## Status LEDs (stock `devstatus_server`, researched 2026-10-13)

[code] Stock drives a ring of status LEDs through the MCU with **LED_CTRL (cmd 1)**: 8 bytes
`[pattern, sub-pattern, LED count, colour, target, 0, 0, 0]` (`mcu_led_ctrl` 85254 →
`mcu_ctrl_cmd_send(1, …, 8)` 85283; layout from the DWARF `__mcu_led_ctrl_t`). Stock waits 100 ms
for a cmd 1 `mcu_notify` and sends at most 3 times (`mcu_timeout[1]` = 100, [file] 0x1fbb08).

[file] `MCU.bin` (APP_0.1.3.3) has an LED module (`[I/LED]`, `led_fac set:%d`, `hsv param err.`)
and a **BCT3236** driver (`BCT3236 ok.` / `BCT3236 doesn't exist.`): a 36-channel I2C LED driver,
i.e. 12 RGB LEDs, matching stock's LED count of 12. [inferred] The ring is the MCU's; whether this
unit has it fitted and where it shows is for the device pass (the MCU firmware checks for a
missing BCT3236).

### Values (DWARF enums of `devstatus_server.c`)

| Byte | Values |
|------|--------|
| `[0]` pattern | 0 LIGHT (steady), 1 BLINK, 2 GRADIENT (pulse), 3 RACE (chase), 4 PROGRESS, 5 HUE_CHANGE, 6 RISE, 7 DOWN |
| `[1]` sub-pattern | LIGHT: brightness 0 = 100 %, 1 = 75 %, 2 = 50 %, 3 = 25 %, 4 = 0 %; BLINK and GRADIENT: 0 = 1 Hz, 1 = 3 Hz; RACE: 0 come-back, 1 down, 2 up; PROGRESS: 0 = 0→100, 1 = 100→0; HUE_CHANGE: 0 normal, 1 reverse |
| `[2]` LED count | 12 = all; 1 for a chase; speaker volume `12 × volume / 100` |
| `[3]` colour | 0 red, 1 green, 2 blue, 3 cyan, 4 yellow, 5 white, 6 orange (7 KEEP is resolved by the host to the last colour sent) |
| `[4]` target | 15 for most; 3 for the error blinks, 4 for mic error, 1 for meeting exit (meaning unknown, maybe a repeat count) |

### The event table

[file] `g_ios_event_status_cfg_array` (40 × 0xb8 bytes; 1.2.2.6 `.data` 0x288658, 3.0.0.1
0x285658, **identical**). Priority SS > P0 > P1 > P2 > IDLE; "hold" is the time limit after which
the event switches itself off (0 = until cleared). Stock also plays the tone named in the table.

| Event (id) | Priority | Hold | Colour | Pattern | Sub | LEDs | Target | Stock raises it on (1.2.2.6) |
|-----------|----------|------|--------|---------|-----|------|--------|------------------------------|
| IDLE (0) | IDLE | — | green | LIGHT | 100 % | 12 | 15 | always: `devstatus_timer_thread` 111180 re-activates it every second (unless factory mode) |
| SHUTDOWN (2) | SS | 30 s | green | DOWN | 1 | 12 | 15 | power off (`ui:` `_poweroff_long_running_task` 114702) |
| BT_BROADCAST (3) | P1 | 60 s | blue | RACE | come-back | 1 | 15 | Bluetooth switched on / re-pair (`BT_IS_OPEN`, `ui:` 110096, 115214) |
| BT_PAIR (4) | IDLE | 1 s | green | LIGHT | 100 % | 12 | 15 | a phone connected (`BT_CONNECT_STATUS` CONNECTED = 0) |
| RECOVERY (5) | P2 | 10 s | cyan | GRADIENT | 1 Hz | 12 | 15 | factory reset (106678) |
| STANDBY (6) | SS | — | KEEP | LIGHT | 50 % | 12 | 15 | screen asleep (`GUI_SLEEP` 1) |
| UPDOWN (7) | P2 | 3 s | cyan | GRADIENT | 1 Hz | 12 | 15 | motor WORK (`MCU_MOTOR_STATE` 1); off on IDLE |
| OTA (8) | P2 | — | blue | GRADIENT | 1 Hz | 12 | 15 | update in progress (45431); the scheduler is then frozen (`ios_devstatus_switch(0)`) until it ends |
| MIC_MUTE (9) | SS | — | red | LIGHT | 100 % | 12 | 15 | mic muted (`AUDIO_NOTIFY_MIC_VOL_TONE`) |
| AGING_FAIL (10) | SS | — | red | LIGHT | 100 % | 12 | 15 | factory only |
| MIC_UNMUTE (11) | SS | 1 s | green | LIGHT | 100 % | 12 | 15 | mic unmuted; **cyan while a meeting records** (`devstatu_led_ctrl` 110469) |
| SPEAKER_VOLUME (12) | SS | 0.5 s | KEEP | LIGHT | 100 % | 12 × v / 100 | 15 | a volume change made on the device (`vol_src` 0, `audio_server_spk_volume_policy` 72400); muted shows 0 |
| MEETING_WAIT (13) | SS | 60 s | cyan | RACE | come-back | 1 | 15 | a phone call rings (`BT_PHONE_COMING_NUM`) |
| MEETING_TALK (14) | SS | — | cyan | LIGHT | 100 % | 12 | 15 | the call is answered (`BT_DEVICE_PHONE_EVENT` 0) |
| HW_ERR (15) | SS | 1 s | yellow | BLINK | 1 Hz | 12 | 3 | test code only |
| TMP_ERR (16) | SS | — | yellow | BLINK | 1 Hz | 12 | 15 | board or CPU ≥ 85 °C, or battery ≥ 75 °C |
| BATTERY_20_PERCENT (18) | P0 | 8 s | yellow | GRADIENT | 1 Hz | 12 | 15 | low-battery tip at 20 % and 10 % (`ui:` `ui_battery_low_tip_show` 23191) |
| BATTERY_5_PERCENT (17) | P0 | 8 s | red | GRADIENT | 1 Hz | 12 | 15 | low-battery tip at 5 % |
| BATTERY_REVERSE_CHARING (20) | P1 | — | cyan | RACE | down | 1 | 15 | powering another device (`MCU_BATTERY_IN_CHARGING` 2) |
| MIC_ERR (21) | P1 | — | green | HUE_CHANGE | normal | 12 | 4 | never: its status is not registered |
| EXCEPTION_MOTOR (22) | SS | 3 s | yellow | BLINK | 3 Hz | 12 | 3 | lift obstructed (SENSOR_TRIGGER) during power off |
| EXCEPTION_STORAGE (23) | SS | 3 s | yellow | BLINK | 3 Hz | 12 | 3 | storage warning or full |
| EXCEPTION_BTPAIR_FAIL (24) | SS | 3 s | yellow | BLINK | 3 Hz | 12 | 3 | never raised |
| EXCEPTION_OTA_FAIL (25) | SS | 3 s | yellow | BLINK | 3 Hz | 12 | 3 | update failed (45821) |
| USB_MODE (26) | P2 | — | blue | LIGHT | 100 % | 12 | 15 | USB disk mode (`ui:` `gui_set_usb_work_mode` 106948 from the U-disk button) |
| AI_MEETING (27) | P0 | — | cyan | LIGHT | 100 % | 12 | 15 | a meeting records or is paused |
| PC_DETECT (28), PUC_DETECTING (29) | SS | — | green | GRADIENT | 1 Hz | 12 | 15 | camera module (PUC) |
| PUC_DETECTED (30) | SS | 1 s | green | LIGHT | 100 % | 12 | 15 | camera module (PUC) |
| AI_MEETING_EXIT (37) | P0 | 1 s | cyan | HUE_CHANGE | reverse | 12 | 1 | the meeting finished |
| AI_MEETING_MIC_MODE (38) | P0 | — | cyan | GRADIENT | 3 Hz | 12 | 15 | never raised in 1.2.2.6 |
| LED_OFF (39) | SS | — | KEEP | LIGHT | 0 % | 12 | 15 | remote "power off" (MQTT, 198201) |

Events without an entry (BOOT 1 aside, which has one but no caller) are never shown:
**`BATTERY_CHARING` (19) has no entry, so stock shows nothing for charging.**

### Stock's scheduler

- [code] `devstatus_event_set` (110862) marks an event active (with the time) or inactive and
  runs `devstatus_handle_schedule` (110611): time-limited events past their hold switch off; the
  active event with the highest priority wins, the latest activation breaks a tie (110688–110697).
- [code] An unchanged winner is not resent, except BOOT, SPEAKER_VOLUME, AI_MEETING_MIC_MODE and
  OTA, whose LED count can change (110699–110706).
- [code] `devstatu_led_ctrl` (110469): colour KEEP takes the last colour sent; while a meeting
  records or is paused, MIC_UNMUTE (and AI_MEETING_MIC_MODE) are cyan and don't update the last
  colour.
- [code] For 10 s after SHUTDOWN was shown, only SHUTDOWN and EXCEPTION_MOTOR are taken
  (110894).
- [code] The 17 statuses it watches (`devstatus_server_register_func` 111879, [file] 0x28a668):
  speaker / mic tone notifications, Bluetooth on, connect status, incoming number, phone event,
  motor state, USB disk mode, charging type, meeting record state, board / CPU / battery
  temperature, screen sleep, storage warning, alert-tone switch. The mapping is
  `devstatus_server_event_handle_cb` (111330).
- [code] `devstatus_server` is in the server list (`g_server_object_list[2]`, [file] 0x282730) and
  its schedule switch starts on (`scheduleAuto` = 1, [file] 0x288620): stock sends LED_CTRL in
  normal use, so the frames are safe to send.

## Stock power off (`ui:` 114531, 114218)

1. End any meeting; POWER_CTRL sub 1 START.
2. Motor to 0 mm; START again; MOTOR_CHECK sub 2.
3. Wait for IDLE (≤ 20 s) plus 1 s; poll motor info every 100 ms until
   position 0 and IDLE.
4. Bluetooth off; CHARGE_CTRL sub 2 data 1; POWER_CTRL sub 0 data 4 (all
   shutdown); suspend.

If the motor is obstructed: timer STOP, back to the configured height; a later
retry uses START_MOTOR.

## Battery low (stock)

Shutdown after 5 heartbeats in a row with level 0 and USB current < 900 mA;
libui first asks MISC_CHECK sub 1 and does not power off if the status is 0
(`ui:` 101927).

## Tools

`mcutool` (stock, on the device) uses `ttyS4`: `vers`, `watch 1` (polls power
status), `pwr`, `usb`, `puc`, `led`, `motor`, `pos`, `motor-burn`, `reboot`,
`reboot_all`, `upgrade`. It has no raw send. waved owns `ttyS4`: stop it first
(`docs/DISPLAY_HARDWARE.md#deploy`) before using `mcutool` or watching the
link by hand:

```bash
adb shell 'stty -F /dev/ttyS4 115200 raw -echo; cat /dev/ttyS4 > /tmp/s4.bin & p=$!;
  i=0; while [ $i -lt 20 ]; do printf "\375\001\000\000\000\000\000\001\000\000\000\374\217" > /dev/ttyS4;
  sleep 1; i=$((i+1)); done; kill $p'
```

## In waved

`crates/waved-mcu` (since 2026-10-12) runs this link: `mavlink.rs` (framing, on top of
the `waved-mavlink` codec it shares with the Wave Controller link),
`protocol.rs` (decoding and the commands it sends), `handler.rs` (heartbeat
replies, the battery with stock's glitch filter, the key as
`DeviceEvent::PowerKey`: click = sleep / wake, `2s_DONE` = power off),
`power_off.rs` (stock's sequence: timer start, lift to 0 mm, wait idle,
charging on, all shutdown; an obstructed lift aborts) and `link.rs` (the
serial port). `Command::PowerOff` (System → Power off, a 2 s hold, an empty
battery) runs the sequence; `Command::Shutdown`, sent on every exit, never
reaches the MCU. Verified on the device 2026-10-12: heartbeats answered,
click sleeps and wakes, a 2 s hold and System → Power off both power the
unit off, `killall waved` does not.

### The lift in waved (Mic height)

`crates/waved-mcu/src/lift.rs` (2026-10-12): `SetMotorPosition` sends the move
(sub 0, stock's limits and speeds), then the start height (sub 4), each resent
up to 3 times without a cmd 3 `mcu_notify` (a motor report also counts as the
move's acknowledgement, so a stalled move is never resent). While it moves,
waved polls MOTOR_CHECK sub 2 every 500 ms; WORK or a position report means
motion, IDLE ends the move (an IDLE in the first second before any motion is
ignored), BLOCK ends it as Blocked, SENSOR_TRIGGER as Obstructed with
`DeviceEvent::MotorObstruction`; after 20 s without IDLE it gives up.
`McuState::lift` carries the result; WORK without a move of waved's (the
power key's hold) shows as moving too. Nothing retries by itself: the user's
next tap on a stalled height uses the block speeds 80 / 45. waved never moves
the lift at start-up (the MCU raised it to the start height).
Verified on the device 2026-10-13: High (50 mm) arrives in 1.8 s; Low goes
50 → 40 → 30 with cmd 4 sub 1 position reports on the way; IDLE ends each move;
the choice is saved (`mic_height` in config.toml). A gentle finger on the lift
produced no BLOCK or SENSOR_TRIGGER: the MCU kept driving and the move took
2.6 s instead of 1.8. Not yet checked: the retract at power off and the
start height after a power cycle.
`crates/waved-mcu/src/simulate.rs` (`SimMcu`) answers like this description;
the device pass checks it (`docs/superpowers/plans/2026-10-12-m3-f-battery-mic-height.md`,
"Device-pass checklist").

### The status LEDs in waved

`crates/waved-mcu/src/leds.rs` (2026-10-13, not yet seen on the device): stock's table and
scheduler ("Status LEDs" above) and stock's LED_CTRL bytes, sent from the MCU service loop
(`McuService::with_leds`, on in `main.rs` unless `WAVED_LEDS=0`). Each LED change logs
`mcu: led <EVENT> (pattern … sub … leds … colour … target …)`; the first answer (or the first
3 unanswered tries) logs whether the MCU acknowledges LED_CTRL. LED_CTRL is resent up to 3
times, 200 ms apart, without a cmd 1 `mcu_notify`; a newer state replaces one still waiting.

| waved state | Event |
|-------------|-------|
| Always (the floor) | IDLE |
| `AudioState::mic_muted` on / off | MIC_MUTE / MIC_UNMUTE |
| `speaker_volume` or `speaker_muted` changes (any source, not only on-device as stock) | SPEAKER_VOLUME (0 LEDs when muted) |
| `MeetingState::recording` or `paused` on / off | AI_MEETING / AI_MEETING_EXIT |
| `BluetoothState::pairing` on / off | BT_BROADCAST |
| `connected` turns on | BT_PAIR |
| `call` ringing / active / idle | MEETING_WAIT / MEETING_TALK / both off |
| `McuState::lift` moving | UPDOWN |
| `Command::PowerOff`, `PowerOffNow`, or `power_off` InProgress / Sent | SHUTDOWN |
| `power_off` turns Aborted (obstructed retract) | SHUTDOWN off, EXCEPTION_MOTOR |
| `OtaPhase::Installing` / `Restarting` (the scheduler then freezes, as stock) | OTA |
| `Failed` right after installing | EXCEPTION_OTA_FAIL |
| `FilesState::low_space` turns on | EXCEPTION_STORAGE |
| `BatteryState::temperature` ≥ 75 °C | TMP_ERR |
| `DeviceEvent::LowBattery` (below 10 %) / at 5 % or less, once each until charging or 13 % | BATTERY_20_PERCENT / BATTERY_5_PERCENT |
| `Command::FactoryReset` | RECOVERY |

Not followed: STANDBY (the screen's sleep is not on the bus), USB_MODE (USB disk mode,
the USB storage work), reverse charging, board / CPU temperature, the camera module. One
deviation from stock: during the 10 s power-off lock other requests are recorded but not
shown (stock drops them), so an aborted power off returns to the current state.

## Open

- Where the status LEDs are and whether this unit has them (`MCU.bin` checks for a missing
  BCT3236); what each pattern looks like and what `target` (3, 4, 15) means; whether the MCU
  acknowledges LED_CTRL with a cmd 1 `mcu_notify`; whether the MCU drives the LEDs by itself
  (charging with the SoC off, its own power off).
- The unit of charge time; the sign of the battery current.
- Whether the MCU reports keys when nobody answers heartbeats (all captures
  with key presses had replies running).
- What the MCU does on its own after the power-off timer START.
- Whether the MCU acknowledges MOTOR_CTRL moves and the start height with a
  cmd 3 `mcu_notify`, reports WORK and IDLE on its own during a move, and
  whether sub 4 moves the lift (all assumed by `lift.rs`).
- BLOCK versus SENSOR_TRIGGER when a hand holds the lift, and whether the MCU
  stops the motor by itself on either.
- That cmd 4 sub 1 positions are i16 mm.
- Whether a BLOCK during the power-off retract means the bottom stop or a
  real stall. waved's power off (`power_off.rs`) keeps polling on BLOCK and
  cuts power on IDLE, or after 20 s without one; it never checks that the
  position reached 0 mm. Stock doesn't re-drive a 0 mm move at block speeds
  (`mcu_motor_block_pos_control` sets them only for `pos != 0`), so waved
  doesn't either. The device pass captures the retract to settle this.
