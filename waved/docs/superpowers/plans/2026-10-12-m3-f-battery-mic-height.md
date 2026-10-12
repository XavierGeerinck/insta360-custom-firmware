# Milestone 3, Workstream F: Battery (F2), Motor (F3) and Mic height (B5) — Plan

> **For agentic workers:** host only. Never run `adb`, never open a serial port. TDD: a failing test
> first, goldens for every new screen and pill state.

**Goal:** the status pill and the battery messages read the MCU's heartbeats; Mic height moves the
lift through `waved-mcu` and shows what the lift is doing; the whole motor path runs against a
simulated MCU on the host, so the device pass only has to confirm the frames.

**Spec:** `../TODO.md` F2, F3, B5 and D5; `docs/MCU_PROTOCOL.md`; `../docs/mockups/png/mic-height.png`,
`home.png`, the Messages strip in `../docs/mockups/index.html`; the stock code in
`../firmwares/fw_v3.0.0.1/decompiled/` (`libui.so.c`: `mcu_server_request_motor_ctrl_pos`,
`mcu_server_request_init_pos`, `process_ntf_mcu_height_block`; `libins_entry.so.c`:
`mcu_init_pos_ctrl`, `mcu_motor_block_pos_control`, `mcu_factory_init`, the cmd 4 branch of
`mcu_msg_recv_thread`).

## Settled (not changed here)

- Low battery never stops a recording or moves the lift. waved powers off at 3 % (finalize the take,
  then power off). `LowBattery` events: on battery power only, falling, after 3 heartbeats, re-armed
  at 13 % (`waved-mcu`, done).
- The lift moves only when the user asks (Mic height) and for power off.

## Decisions (this plan)

| ID | Decision | Why |
|---|---|---|
| F-1 | Heights are stock's: **Low 30 mm, Medium 40 mm, High 50 mm**. 0 mm is only for power off. `command_plan` mapped Low to 0 and Medium to 30; fixed. | `mcu_server_request_motor_ctrl_pos`: lvl 0/1/2 → 30/40/50, lvl 3 → 0 |
| F-2 | **Persist the height**: `[device] mic_height` in config.toml (default `medium`) and `SettingsState::mic_height`; on each choice waved-mcu also sends the MCU's start height (MOTOR_CTRL sub 4: 0 = 50, 1 = 40, 2 = 30 mm), as stock does. waved never moves the lift at start-up: the MCU raises it to its start height by itself. | Stock stores both (`mcu_init_pos_ctrl`: `ins_config` + sub 4); factory default `MOTOR_INIT_40` (`mcu_factory_init`) matches the mockup's Medium |
| F-3 | **No automatic retries.** A stalled lift (state BLOCK) is reported; tapping the same height again retries with stock's block-control speeds 80 / 45. Stock retries BLOCK by itself once per 10 s. | "The lift moves only when the user asks" |
| F-4 | `McuState::lift: LiftState` (Idle, Moving, Blocked, Obstructed) is the lift's motion and the outcome of the last move; Blocked and Obstructed stay until the next move. While a move runs, waved-mcu polls MOTOR_CHECK sub 2 every 500 ms; IDLE counts once the motor was seen working or 1 s passed; after 20 s it gives up (Idle, warning in the log). Commands are resent up to 3 times without an acknowledgement (stock: 3 tries). | The UI needs Moving / done / blocked; a lost frame must not leave "Moving" forever |
| F-5 | Messages: the battery messages show the battery glyph the pill shows (mockup "Battery 10%, plug in Wave" has the battery icon); "Mic lift blocked" (warning) once each time a move ends blocked or obstructed. The low-battery pill and both battery messages already exist (M2, workstream A); this plan adds their goldens. | Mockup Messages strip |
| F-6 | The MCU simulator speaks the protocol: `SimMcuService` runs the real `McuService` over a simulated MCU (heartbeats, acks, motor states, positions, scripted block or obstruction), so the `simulate` build and the tests exercise framing, lift and power off. `main.rs` is unchanged. | "Make the motor path fully testable with the MCU simulator" |
| F-7 | The obstruction event (cmd 4 sub 2 = 4, SENSOR_TRIGGER) and BLOCK (= 2) are decoded from stock code; position reports (cmd 4 sub 1, i16 mm in `params[1..2]`) from the 2026-10-12 power-key capture (layout assumed). All three are **unverified on a user move** until the device pass. | `MCU_PROTOCOL.md` |

## File structure

| File | Change |
|---|---|
| `crates/waved-state/src/lib.rs` (hotspot) | `LiftState`; `McuState::lift`; `SettingsState::mic_height`; serde on `MotorPosition` (`Moving` skipped) |
| `crates/waved-config/src/lib.rs`, `config/default.toml` (hotspot) | `DeviceConfig::mic_height` (default medium) |
| `crates/waved-settings/src/{lib,change}.rs` | `Change::MicHeight` from `SetMotorPosition`; seeded into `SettingsState` |
| `crates/waved-mcu/src/protocol.rs` | `height_mm`, `Speeds` and `motor_move` (block speeds), `start_height`, `lift_position` (sub 1) |
| `crates/waved-mcu/src/lift.rs` (new) | Pure lift state machine: request, MCU reports, acks, polling, timeout |
| `crates/waved-mcu/src/handler.rs` | Routes motor notifications and acks to the lift; `tick` |
| `crates/waved-mcu/src/lib.rs` | `McuService<T: Transport>`; publishes `McuState::lift` / `motor_position` |
| `crates/waved-mcu/src/simulate.rs` | `SimMcu` transport + `SimMcuService` on top of `McuService` |
| `crates/waved-ui/src/screens_v2/mic_height.rs` (new) | B5 screen |
| `crates/waved-ui/src/screens_v2/{mod,menu,messages}.rs`, `runtime/registry.rs` (hotspot), `kit/dispatch.rs`, `kit/components/status.rs` | Register the screen, the Menu row, "Mic lift blocked", message icons, pill goldens |
| `docs/MCU_PROTOCOL.md`, `crates/waved-ui/src/kit/README.md`, `../TODO.md` | What waved does now; device-pass checklist |

## Tasks

1. **State and config** — tests: `MotorPosition` round-trips as `"low"/"medium"/"high"` and refuses `"moving"`; default config has `mic_height = medium` and `default.toml` matches; `SetMotorPosition(Low)` persists, `SetMotorPosition(Moving)` is ignored; seeding copies the height.
2. **Protocol** — tests: heights 30/40/50 mm; block speeds 80/45; start height bytes `03 00 00 00 04 <0|1|2>`; position report decode; command plan maps Low/Medium/High to 30/40/50.
3. **Lift state machine** — tests: request sends move + start height and publishes Moving; resend without ack (3 tries); IDLE before WORK within 1 s is ignored; WORK then IDLE ends at the height; BLOCK → Blocked (no resend); retry after BLOCK uses block speeds; SENSOR_TRIGGER → Obstructed + `MotorObstruction`; polling every 500 ms; 20 s give-up; MCU-initiated motion (power-key hold) shows Moving then Idle.
4. **Service + simulator** — `McuService` generic over `Transport`; tests run it on a thread against `SimMcu` through the bus: Mic height reaches Idle at the height; a scripted obstruction ends Obstructed with the event; a scripted block ends Blocked; power off still ends `Sent` / `Aborted`; the battery reaches `BatteryState` from heartbeats.
5. **Battery UI** — goldens `status_pill_charging`, `status_pill_low_battery`, `status_message_battery_low`; the pill reads `bus.battery` through `UiSnapshot::capture`; battery messages carry the pill's glyph.
6. **Mic height screen** — goldens `screen_mic_height` (mockup), `screen_mic_height_moving`, `screen_mic_height_blocked`; tap sends `SetMotorPosition`; rows disabled while moving; the chosen row reads "Moving"; the blocked hint; `assert_targets_ok`. Register the route; Menu row "Mic height" with the value; "Mic lift blocked" message.
7. **Docs** — `MCU_PROTOCOL.md` "In waved" and Open; kit README registry table; TODO F2/F3/B5 status and the device-pass checklist below.

## Device-pass checklist (F2, F3, B5)

Run with waved stopped where `mcutool` or a raw capture is needed (`docs/MCU_PROTOCOL.md`, "Tools").

**F2 Battery**
1. On battery, the pill's level matches the heartbeat level (`mcutool watch 1` or the waved log) within one heartbeat; no level shows before the second heartbeat.
2. Plug USB in: the pill turns teal with the charging glyph within ~2 heartbeats; unplug: back to grey. Note the USB current on a laptop port and a charger (the 100 mA threshold).
3. At 100 % on USB the pill shows the plain full battery (not charging). Decide whether "plugged in" should show the bolt (ota.png draws it).
4. Drain on battery (or bench supply): one "Battery 10%, plug in Wave" at 10 %, one "Battery 5%, plug in Wave now" at 5 %, pill amber ≤ 20 %, red ≤ 10 %; no repeats while it flaps.
5. At 3 % during a recording: the take is finalized and listed in Files after the next boot, then the unit powers off; the lift retracts before power is cut.
6. Check the sign of the battery current while charging and discharging (Open item in `MCU_PROTOCOL.md`).

**F3 Motor (capture `/dev/ttyS4` during each step)**
1. Mic height → High: waved sends `FD 11 .. 64 00 00 03 00 00 00 00 02 00 00 00 32 00 A0 0F B8 0B 32 19 ..`; the MCU acknowledges with a msgid 101 cmd 3 frame (record its params).
2. Then the start-height frame (cmd 3 sub 4, data 0 for 50 mm) is acknowledged; it does **not** move the lift.
3. Record what the MCU sends during the move: cmd 4 sub 2 WORK → IDLE, cmd 4 sub 1 positions (confirm the i16 at `params[1..2]` is mm), and how long a 30 → 50 mm move takes.
4. Hold the lift down by hand during a move: record whether the MCU reports BLOCK (2) or SENSOR_TRIGGER (4), and whether it stops the motor by itself. The screen shows "Blocked, clear it and tap again" and the "Mic lift blocked" message; tapping the height again moves it (block speeds 80 / 45).
5. Power-cycle after choosing Low: the MCU raises the lift to 30 mm by itself at power-on and Mic height shows Low.
6. `killall waved` mid-move: the lift finishes its move; restarted waved does not move it.
7. Power off (System → Power off and a 2 s key hold) still retracts to 0 mm and cuts power; an obstruction during the retract leaves the unit on.

**B5 Mic height**
1. The screen matches mic-height.png on the panel; every row and Back can be tapped.
2. While moving, the chosen row reads "Moving" and the others ignore taps; it returns to the check when the MCU reports IDLE.
3. Menu shows "Mic height" with the current value.
