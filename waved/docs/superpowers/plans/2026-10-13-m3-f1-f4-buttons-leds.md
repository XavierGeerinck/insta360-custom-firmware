# Milestone 3, Workstream F: Volume buttons (F1) and status LEDs (F4) — Plan

> **For agentic workers:** host only. Never run `adb`, never open a serial port. TDD: a failing
> test first. Tests: `cargo test -p waved-mcu --lib` and `--features simulate`.

**Goal:** find out from stock 1.2.2.6 which physical controls and LEDs the Wave has and how stock
drives them; wire what exists into waved.

**Spec:** `../TODO.md` F1, F4; `docs/MCU_PROTOCOL.md`; stock `../firmwares/fw_v1.2.2.6/`
(`decompiled/libins_entry.so.c`, `decompiled/libui.so.c`, `extracted/dtb.img`,
`extracted/MCU.bin`, the DWARF of `extracted/rootfs/usr/lib/libins_entry.so`).

## Findings (details and line refs: `docs/MCU_PROTOCOL.md`, "Status LEDs" and "Physical controls")

- **F1: the Wave has no volume buttons.** Its one key is the power key on the MCU. The only
  volume/mute keys in the device tree belong to a disabled reference-board node
  (`aml_pca9557@1f`, "aml, ledring", keys mute/pause/vol+/vol-, `status = "disabled"`). Stock's
  `user_request_volume_up/down` send HID keys *to the computer*; nothing reads a volume key.
- **F4: the LEDs are on the MCU.** Stock's `devstatus_server` turns device states into
  `LED_CTRL` (cmd 1, 8 bytes: pattern, sub-pattern, LED count, colour, target) from a 40-entry
  table (`g_ios_event_status_cfg_array`, identical in 1.2.2.6 and 3.0.0.1): 12 LEDs, 7 colours,
  priorities, timeouts. The MCU firmware has an LED module and a BCT3236 36-channel LED driver
  (12 RGB LEDs). Whether the ring is fitted and visible on this unit is for the device pass.

## Decisions

| ID | Decision | Cost if wrong |
|---|---|---|
| L-1 | F1 stops at the finding: no input path to wire, nothing implemented; TODO F1 closed as "not on this hardware". The legacy `ButtonType::VolumeUp/Down` path (waved-hid bridge, `SimMcuHandle::press_button`) is left alone. | If a unit does have keys, a capture on the device would show it; nothing in waved blocks adding them later. |
| L-2 | LEDs follow stock's scheduler exactly: highest priority wins, the latest activation breaks ties, events with a time limit expire, an unchanged top event is not resent (except the speaker volume and update levels), `KEEP` reuses the last colour, mic unmute while recording is cyan, and nothing else shows for 10 s after the power-off pattern unless the lift is obstructed. Stock's bytes are sent unchanged. | The bytes are stock's; only which state maps to which event is ours (below). |
| L-3 | State → event mapping: mic mute (red) / unmute (green 1 s); speaker volume or speaker mute change (level on the ring, 500 ms); recording or paused (cyan) / its end (1 s); Bluetooth pairing (blue chase) / a phone connects (green 1 s); a ringing phone call (cyan chase) / a call in progress (cyan); lift moving (cyan pulse); `Command::PowerOff` (green fall); an obstructed power-off retract (yellow blink 3 s); update in progress (blue pulse) / failed (yellow blink 3 s); low space (yellow blink 3 s); battery temperature ≥ 75 °C (yellow blink); low battery ≤ 10 % (yellow pulse 8 s) and ≤ 5 % (red pulse 8 s), once each; `Command::FactoryReset` (cyan pulse 10 s); otherwise IDLE (green). | A wrong mapping shows a wrong colour; no command beyond LED_CTRL. |
| L-4 | Not mapped (no waved state, or never raised by stock): standby at half brightness (screen asleep: no bus state; the UI runtime is a hotspot), USB disk mode (blue, the USB storage workstream), reverse charging, board / CPU temperature, camera module (PUC), boot and AI meeting mic mode (no stock caller), mic error (stock never registers it). **Charging has no LED in stock** (`BATTERY_CHARING` is not in the table). | The ring stays on its last stock-like state in those cases. |
| L-5 | The speaker-volume level shows for every change, not only on-device ones (stock: `vol_src == 0` only); waved can't tell a host change from a local one at the MCU. | The ring flashes the level when the computer changes the volume. |
| L-6 | LED_CTRL is resent up to 3 times without a cmd 1 `mcu_notify` (stock: 3 tries, 100 ms); a newer LED state replaces one still waiting. | Two extra frames per change if the MCU never acknowledges LED_CTRL. |
| L-7 | `McuService::with_leds(LedSources)` (watch receivers for audio, meeting, Bluetooth, updates, files) turns it on; `main.rs` wires it for the device and the simulator. `WAVED_LEDS=0` leaves the LEDs alone. | One additive block in `main.rs`. |

## File structure

| File | Change |
|---|---|
| `crates/waved-mcu/src/leds.rs` (new) | Stock's table, scheduler (pure), state follower, LED_CTRL outbox |
| `crates/waved-mcu/src/protocol.rs` | `cmd::LED_CTRL`, `led_ctrl()` |
| `crates/waved-mcu/src/lib.rs` | `LedSources`, `McuService::with_leds`, feeds the follower each loop |
| `crates/waved-mcu/src/simulate.rs` | `SimMcu::leds()`, `SimMcuService::with_leds` |
| `crates/waved/src/main.rs` (hotspot, additive) | Wire `with_leds` unless `WAVED_LEDS=0` |
| `docs/MCU_PROTOCOL.md`, `../TODO.md`, `docs/AUDIO_HARDWARE.md` | Findings, what waved does, F1/F4 status |

## Tasks

1. **Research** into `MCU_PROTOCOL.md` (done first, with line refs).
2. **Protocol** — test: `led_ctrl` frame is `cmd 1` + the 5 bytes + 3 zero bytes.
3. **Scheduler** — tests: IDLE green at start; priority beats recency; recency breaks ties;
   time limits expire; an unchanged top is not resent; speaker volume resends with 12 × v / 100
   LEDs in the last colour; mic unmute while recording is cyan; the shutdown lock.
4. **Follower** — tests: the first snapshot sets levels without pulses; each mapping in L-3.
5. **Service** — tests through `SimMcu`: `with_leds` sends IDLE at start, mute turns the ring red,
   unmute green then back; a cmd 1 notify stops resends.
6. **Docs and TODO.**

## Device-pass checklist (F1, F4)

Run waved with `RUST_LOG=waved_mcu=info` (the LED lines are `mcu: led …`). For a raw capture stop
waved first (`docs/MCU_PROTOCOL.md`, "Tools").

**F1 Volume buttons**
1. Look over the unit: confirm the power key is the only physical key (no volume rocker, no touch
   keys outside the screen).
2. Optional: under stock 1.2.2.6, `cat /proc/bus/input/devices` lists only `vad_keypad` and
   `ILITEK_TDDI`, and `/sys/firmware/devicetree/base/soc/apb4@fe000000/i2c@6a000/aml_pca9557@1f/status`
   reads `disabled`.

**F4 LEDs**
1. Start waved: the log shows `mcu: led IDLE` and one frame `FD .. 64 00 00 01 00 00 00 00 00 00 0C 01 0F`
   (cmd 1: LIGHT, 100 %, 12 LEDs, GREEN, target 15). Look for a light: where it is (lift, base,
   around the screen), and that it is green. If nothing lights anywhere, note it: the BCT3236 may
   be unfitted on this unit (then set `WAVED_LEDS=0` and record it in `MCU_PROTOCOL.md`).
2. Record whether the MCU answers LED_CTRL with a cmd 1 `mcu_notify` (`mcu: led` lines show
   `no reply` after 3 tries otherwise) and what its params carry.
3. Tap the dial (mic mute): `mcu: led MIC_MUTE`, the ring turns red; tap again: green for 1 s
   (`MIC_UNMUTE`), then back to IDLE.
4. Louder / Quieter: `mcu: led SPEAKER_VOLUME` with `leds n` = 12 × volume / 100; the ring shows
   that many LEDs for 0.5 s in the last colour; at 0 % or muted none.
5. Start a recording: cyan (`AI_MEETING`); mute during it: red; unmute: cyan; stop: 1 s cyan
   colour change (`AI_MEETING_EXIT`), then green.
6. Bluetooth → pair a phone: blue chase (`BT_BROADCAST`, up to 60 s); when the phone connects:
   `BT_PAIR` (green 1 s). Ring the Wave from the phone: cyan chase (`MEETING_WAIT`); answer:
   cyan (`MEETING_TALK`); hang up: back to green.
7. Mic height: cyan pulse while the lift moves (`UPDOWN`, at most 3 s).
8. System → Power off: green falling pattern (`SHUTDOWN`) until the power is cut. With a hand on
   the lift during the retract: yellow fast blink (`EXCEPTION_MOTOR`) for 3 s.
9. Note for each pattern what the MCU actually shows (pattern, speed, `target` 3 = three blinks?)
   and fill in the pattern meanings in `MCU_PROTOCOL.md` "Open".
