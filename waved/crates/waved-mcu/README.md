# waved-mcu

UART link to the Wave's MCU (motor, and in theory battery, buttons and USB).

## What the device actually sends (device pass, 2026-10-06)

With `RUST_LOG=info,waved_mcu=debug`, 25 s of normal running logged **no MCU
frames at all**: no battery (0x10–0x1F), no USB (0x50/0x51), no unknown ids.
The battery and USB frame ids in `lib.rs` are guesses that never matched.

Consequences, handled elsewhere:
- `BatteryState` stays at its default (level 0, voltage 0). The UI treats
  voltage 0 as "no reading" and hides the battery
  (`waved-ui` `status::battery_reading`, `screens_v2/messages.rs`).
- `McuState.usb_connected` comes from the USB device controller instead
  (`waved-system/src/usb.rs`, `/sys/class/udc/*/state == "configured"`).

The stock firmware's MCU event names (`mcu_evt_handle`, `BTN_CLICK`, …) are in
`crates/waved-system/README.md` (power key). See also the 0x4B heartbeat note
in the project memory: corrupted heartbeats were once misread as buttons.
