# MCU Protocol — `/dev/ttyS4`, MAVLink v2

The Wave's microcontroller (battery, charging, the camera-lift motor, the LED,
the power key, power off) talks to the SoC over **`/dev/ttyS4` at 115200 baud,
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
| 1 | LED_CTRL | no sub: `[0]` first-level state, `[1]` second-level state, `[2]` LED number, `[3]` colour, `[4]` target; value meanings unknown (factory use only) |
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

### Motor notifications (`mcu_notify` cmd 4)

- sub 2: `params[1]` motor state: 0 IDLE, 1 WORK, 2 BLOCK, 3 WAIT_DATA,
  4 SENSOR_TRIGGER (obstructed: aborts a power off). On BLOCK stock re-sends
  the move (speeds 80 / 45, at most once per 10 s, 80911).
- No params (motor info): i16 `[1..2]` current, `[3..4]` force3,
  `[5..6]` position, `[7..12]` hall A/B/C, `[13..16]` force1/force2.

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

`crates/waved-mcu` (since 2026-10-12) runs this link: `mavlink.rs` (framing),
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

## Open

- LED field meanings; the unit of charge time; the sign of the battery current.
- Whether the MCU reports keys when nobody answers heartbeats (all captures
  with key presses had replies running).
- What the MCU does on its own after the power-off timer START.
