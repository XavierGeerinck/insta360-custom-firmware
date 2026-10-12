# waved-system

System services for the Wave (TODO workstream E). Everything device-specific
sits behind a path or a trait so the crate tests on the host:
`cargo test -p waved-system --lib`.

| Module | What | Device edge |
|---|---|---|
| `display` | `DisplayPower`: rest after `dim_after_s`, off after `off_after_s`, never off while recording or in a call, first touch only wakes, power-key meaning | none (pure) |
| `backlight` | `Backlight::set_percent` | `/sys/class/leds/lcd-backlight/{brightness,max_brightness}` (0–255) |
| `power_key` | `KEY_POWER` → `DeviceEvent::PowerKey(Short / Long)` (long = held 2 s) | `/sys/class/input/event*/device/{name,capabilities/key}`, `/dev/input/eventN` |
| `tz` | `TimeZone::offset_at` from TZif + POSIX footer | `/usr/share/zoneinfo` (slim files) |
| `rtc` | BCD RTC, system clock | `/dev/i2c-3`, slave `0x68`, `settimeofday` |
| `about` | version (`0.1.0 (sha)`), serial, storage, licences | `/unifykey/usid`, `/proc/device-tree/serial-number`, `statvfs` |
| `SystemService` | publishes `SystemState` (offset each minute, About); RTC restore and save | the above |
| `tones` | recording start and stop chimes | `ToneSink` (waved-audio playback) |
| `reset` | factory reset at the next start-up; the marker records the steps already done, so a retry runs only the rest | `/data/waved/config.toml*`, `meetings.db*`, `/mnt/udisk/recordings`, `/etc/wpa_supplicant.conf` |

## Device facts

- **Power key: the physical key does not reach Linux** (device pass,
  2026-10-12). Short presses moved nothing on `event0`, the eight SARADC
  channels, the exported GPIOs, the interrupt counters or the MCU UART
  (`/dev/ttyS1` carried only the `0x4B` heartbeat). The MCU alone acts on the
  key: holding it about 3 s cuts power. `vad_keypad` (`event0`,
  `fe330000.audiobus:vad`) is the SoC's voice-activity detector; its
  `KEY_POWER` wakes the system on speech, so `power_key::find_power_key` never
  takes it and finds no key on the Wave. Stock firmware gets presses from the
  MCU over `/dev/ttyS4` (MAVLink, `mcu_notify` cmd 0x0E; verified on the
  device the same day, `docs/MCU_PROTOCOL.md`); waved only ever opened
  `/dev/ttyS1`, the Bluetooth module. Reading it is the `waved-mcu` rework
  (TODO F7).
- **RTC.** BCD at `0x68` on `/dev/i2c-3`, registers 0–6 = seconds, minutes,
  hours, weekday (0 = Sunday), day, month, year − 2000, in UTC. This is the stock
  `rtc_gettime` / `rtc_settime` layout. Decoding masks seconds and minutes with `0x7f` (bit 7 is the oscillator
  halt), hours with `0x3f`, day with `0x3f` and month with `0x1f` (bit 7 is the
  century); a set 12-hour flag (bit 6 of hours) is not a time.
- **Time zones.** The device ships slim TZif files (`Europe/Brussels` is
  1 103 bytes). Dates after the last transition come from the POSIX footer,
  for example `CET-1CEST,M3.5.0,M10.5.0/3`. A v2+ file whose footer is not
  exactly `\n<tz>\n` (cut off, damaged or missing its newlines) is rejected as
  malformed rather than read as a zone without daylight saving time. The
  service then shows UTC and logs the zone name and the error.
- **Factory reset.** `request` only writes `factory-reset.pending` next to
  `config.toml`; `reset::run` sends `Command::RestartForConfig` once `request`
  has succeeded. `complete_pending` runs
  first thing at the next start-up and erases four steps in order: `config`,
  `database`, `recordings` and `wifi`.
  - The marker holds `attempts=N` and `done=...` (the finished steps). A retry
    after a power cut or a failed step runs only the steps not yet done, so
    settings the user has made since are not erased again.
  - After `MAX_ATTEMPTS` (3) start-ups the reset gives up and removes the
    marker, so it never erases on every boot.
  - If a start-up cannot save its attempt count (for example `/data` is
    read-only), it erases nothing and reports `Retrying`, since the cap could
    not hold otherwise.
  - The recordings step empties the directory, keeps it, and flushes the
    recordings volume (`syncfs`) before the step counts as done, since the
    marker's own sync covers only `/data`. It refuses a
    recordings path that is empty, contains `..` or, when a mount is set (on the
    device, `/mnt/udisk`), is not absolute and strictly inside that mount. It
    does not wait for the mount: while the mount is not a mount point the step
    fails at once, and that start-up counts as a failed attempt. If the mount
    has not appeared by the third start-up the reset gives up, removes the
    marker and the recordings are never erased.
  - `config.toml*` and `meetings.db*` match by name prefix, regular files only,
    so the `waved` binary beside them stays.
- **Build id.** `build.rs` takes the About version's id from the
  `WAVED_BUILD_ID` environment variable when it is set and not blank, otherwise
  from `git rev-parse --short=7 HEAD`, otherwise there is none and About shows
  just `0.1.0`. The cross build container has no git, so a device build must
  pass `WAVED_BUILD_ID` in (for example `WAVED_BUILD_ID=$(git rev-parse --short=7 HEAD)`
  with a Cross.toml passthrough, which the integrator adds).
- **Wi-Fi reset.** The `wpa_supplicant.conf` header is taken from the stock image.
  `wpa_cli reconfigure` follows, because waved-wifi's `save_config` would
  otherwise write the running networks back.

## Wiring

The integrator wires the crate in at the milestone 2 runtime switch. See
`docs/superpowers/plans/2026-10-05-m2-e-system-services.md`, Integration requests.
In short:

- `reset::complete_pending(&ResetPaths::new(config, database, recordings).on_device())` first at start-up,
  before the config loads; `reset::run` handles `Command::FactoryReset`.
- `SystemService::device(system_tx, storage_path).run(settings, wifi)` publishes `SystemState`.
- `power_key::start(events)` (the `DeviceEvent` sender) feeds `DeviceEvent::PowerKey`,
  once device step 4 (including its negative check) has passed.
- `tones::run(meeting, settings, sink)` with a `ToneSink` over the playback service.
- The UI loop owns one `DisplayPower` and one `Backlight`.

## Device verification

Run these after the runtime switch has wired the integration requests (IR1–IR3).
Before deploying, run `adb shell df -h /data` and leave at least 70 MB free.
Push to `/data/waved/waved.new`, compare `md5sum`, then `mv` it over `waved`.

1. **Backlight range.**
   - `adb shell cat /sys/class/leds/lcd-backlight/max_brightness` should read 255.
   - On Display, step brightness from 5 % to 100 %. Each step should be visible.
   - Let the screen rest at 80 % (resting level 24 %) and at 5 % (resting 5 %). If 5 % looks dark, raise `MIN_LIT_PCT`.
2. **Timers.**
   - Set Dim after to 15 s; set `off_after_s = 60` on the setup page.
   - Resting should show at 15 s and the screen should turn off at 60 s.
   - Start a recording and wait past 60 s. The screen stays resting with the red ring visible.
   - Repeat with a Teams/Zoom call (HID off-hook).
   - With the screen off, start a recording from the computer. The resting screen should light.
3. **Wake guard.** Rest the screen, then tap the dial. The screen wakes and the mic does **not** mute.
4. **Power key source.**
   - Run `adb shell cat /proc/bus/input/devices` and note the `vad_keypad` handler.
   - Run `adb shell hexdump -C /dev/input/event0`. Press the power key briefly, then hold it for 3 s.
   - Expect records with type `01 00`, code `74 00` and value `01 00 00 00` / `00 00 00 00`.
   - Check `waved.log` for `power key: Short press` / `Long press`.
   - **Negative check.** `vad_keypad` may be the Amlogic VAD speech-wake input
     rather than the physical key. With the screen awake, talk or clap near the
     unit for 30 s without touching the key. Expect no records on the
     `vad_keypad` node and no `power key:` lines in `waved.log`.
   - The integrator wires `power_key::start` only after this check passes.
   - **If nothing arrives:** the physical key reaches the MCU, not Linux. The stock firmware handles it in `mcu_evt_handle` (`BTN_CLICK`, `BTN_LONG_PRESS_2s_*`). Record this in this README and hand an MCU source for `DeviceEvent::PowerKey` to workstream F (milestone 3). `DisplayPower` and the event contract stay the same.
   - Also note whether holding the key for 5 s or more cuts power in hardware.
5. **Power key behaviour.**
   - A short press puts the screen off (resting while recording).
   - A second short press wakes it.
   - Holding for 2 s opens "Power off?" while the key is still held.
   - Confirm. Does the unit actually power off, or does waved restart?
6. **RTC.**
   - Run `adb shell i2cdetect -y 3` if busybox has it; expect `68`, not `UU`.
   - Connect Wi-Fi. The log shows `RTC set from network time`.
   - Power-cycle with Wi-Fi off. The log shows `clock restored from the RTC`, and the status clock is right.
7. **Time zone and format.**
   - Set `Europe/Brussels`. The status clock should be UTC+2 in October; compare with `adb shell TZ=Europe/Brussels date`.
   - Toggle 24-hour.
   - Check that `--:--` shows before any time source.
8. **Minute refresh.** Watch the status clock roll over to the next minute without any other change on screen.
9. **Tones.**
   - Start a recording: a rising chime plays. Stop: a falling chime.
   - Turn Alert tones off: silence.
   - While the computer streams audio over USB, start a recording and listen for glitches beyond the tone.
10. **Factory reset.**
    - Save a Wi-Fi network and make a recording, then reset.
    - Waved restarts into first start.
    - `cat /data/waved/config.toml` shows defaults or is absent.
    - `ls /mnt/udisk/recordings` is empty.
    - `wpa_cli -i wlan0 list_networks` shows none.
    - `ls /data/waved` still has `waved`, and no `factory-reset.pending` is left.
11. **About.**
    - The version shows the git id. If it shows just `0.1.0`, the build did not
      pass `WAVED_BUILD_ID` (see Build id above).
    - Compare the serial with the label on the unit. If it is empty or wrong, find the source (`ls /unifykey`, `cat /proc/device-tree/serial-number`) and update `SERIAL_SOURCES`.
    - Storage matches `df /mnt/udisk`.
12. **Binary size.** Compare the stripped release binary with the previous deploy. `waved-system` should add well under 1 MB.
