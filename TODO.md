# Wave open firmware — backlog

Rewritten 2026-10-04. This is the single list of everything still missing. It replaces the
old scratch notes in this file. Their open questions are folded in below: Wave Controller
compatibility, how to go back to stock, and the NPU.

Sources:

- [`docs/FIRMWARE_GAP_ANALYSIS.md`](docs/FIRMWARE_GAP_ANALYSIS.md): parity with stock
  firmware 1.2.2.6, the latest release
- [`docs/mockups/`](docs/mockups/): UI redesign, revision 2
- [`waved/TODO.md`](waved/TODO.md): detailed acceptance evidence for audio, recording and
  webhooks. Its open items are carried over in workstreams C and M.

## How to read this

- Every item has an ID, a size and its location in the repo.
  - **S** is a day or less, **M** is two to five days, **L** is more than a week.
- All decisions are settled. Only the visual confirmation of D8 is left, during the first device pass.
- **Phase 0** has to land first. It defines the shared contracts: commands, state, config,
  theme and components. After that, workstreams **A–L** can run in parallel. Items inside a
  workstream are in order.
- Integration hotspots are `waved/crates/waved/src/main.rs`, `waved-ui/src/lib.rs` + `runtime/`,
  `waved-state` and `waved-config`. Workstreams don't edit these directly: changes there go
  through Phase 0 or the integrator.

## Decisions (settled 2026-10-04)

| ID | Decision | Outcome |
|---|---|---|
| D1 | Stop during a recording | Hold Stop for 1 s. A short tap shows "Hold Stop to end the recording". |
| D2 | Audio modes | Six modes: Whole room, Follow talker (`AudioMode::Steered`), Focused / Narrow / Two sides aimed by dragging (persisted `SetDoaOverride`), Stereo. |
| D3 | Pre-recording | On by default. Record reads "includes last 30 s", the resting screen shows a dot, and there's a switch under System. |
| D4 | Nobody answers "Send this recording?" | Keep it on Wave after 60 s. |
| D5 | Scope of this round | **In:** full redesign, Bluetooth audio and calls, Wave Controller, self-hosted OTA, USB storage, Mic height, PIN lock, physical volume buttons. **Later:** more languages. |
| D6 | Device use during implementation | Agents test on the host (cargo tests and the LVGL snapshot harness). One integration pass on the device per milestone. |
| D7 | Branching | `feat/display-redesign` off `feat/wifi-provisioning` (125 commits ahead of `main`, not merged), one worktree per workstream, one PR at the end. |
| D8 | Panel type | **LCD.** The 1.2.2.6 device tree has an `ilitek,ili79400` touch-and-display controller (`ili79400ts@41` is its touch half) and a PWM `lcd-backlight` under `pwm-leds`. No burn-in protection is needed. Confirm visually in the milestone 1 device pass. |
| D9 | Wave Controller source | Build from the decompiled stock handlers first. Verify against the official app once a stock capture is possible. |
| D10 | PIN scope | Locks Files, Wi-Fi, Bluetooth, Uploads and System. Never mute, volume, Record, audio mode or noise reduction. 4–6 digits set on the setup page. Unlocked until the screen rests; five wrong tries means a one-minute wait. |

## Execution plan

| Milestone | Work | How it runs | Device pass at the end |
|---|---|---|---|
| 1 | Phase 0 (P0.1–P0.6), F5 panel test. Plan: `waved/docs/superpowers/plans/2026-10-04-display-redesign-m1-foundation.md` | Two parallel lanes (contracts, UI kit) plus the integrator | Panel test; theme, fonts and components on the real screen |
| 2 | A main screens, B settings screens, C recording and audio, D Files, E system services | Parallel, one worktree each | Home, Recording, menus; pause/resume; brightness; power key |
| 3 | F hardware, G USB storage, H updates, I Bluetooth, J Wave Controller, K security | Parallel, one worktree each | Each feature on the device |
| 4 | M acceptance, L docs, parity matrix | Parallel | Full regression and the one-hour certification |

Rules for the parallel milestones:
- Only the integrator edits the hotspot files.
- A workstream is done when its host tests pass and its snapshot PNGs match the mockups.

## Phase 0 — shared contracts (serial, lands first)

- [ ] **P0.1 Commands and state** (M) — `waved-state/src/lib.rs`
  - Replace the ten `Open*` commands with UI-internal navigation (`kit::Router`, `UiEvent::Navigate` and `Back`).
    The `Open*` commands are deleted at the milestone 2 runtime switch.
  - Add display commands: `SetBrightness(u8)`, `SetDimAfter(u32)`, `WakeScreen`, `SleepScreen`.
  - Add time commands: `SetTimeZone(String)`, `SetClock24h(bool)`.
  - Add recording-policy commands: `SetPreRecording(bool)`, `SetUploadMode(UploadMode)`.
  - Add file commands: `DeleteRecording(Uuid)`, `PlayRecording(Uuid)`, `PausePlayback`,
    `SeekPlayback(u32)`, `StopPlayback`.
  - Add system commands: `FactoryReset`, `StartUsbStorage`, `StopUsbStorage`.
  - Add Bluetooth commands: `BtSetDiscoverable(bool)`, `BtConnect`, `BtDisconnect`, `BtForget`.
  - Add call commands: `AnswerCall`, `DeclineCall`.
  - Add `SetAim(Option<f32>)`, which wraps `SetDoaOverride` and is persisted.
  - Add state: `DisplayState`, `PlaybackState`, `UsbMode`, Bluetooth device list, `OtaState`,
    `first_run_done`, and an `audio_in_use` flag (host capture open, off-hook, or recording).
- [ ] **P0.2 Config schema and migration** (M) — `waved-config/src/*`
  - Bump `config_version` and add:
    - `display.brightness`, `display.dim_after_s`, `display.off_after_s`
    - `time.zone`, `time.clock_24h`
    - `recording.pre_recording`, `recording.ask_timeout_s`
    - `audio.aim_deg`, `tones.enabled`, `device.first_run_done`
  - Add a migration from the current version.
  - Add setup-portal form fields for the user-facing ones.
- [ ] **P0.3 Theme, fonts, icons** (M) — `waved-ui/src/kit/tokens.rs` (was `theme.rs`), `lv_conf.h`, `build.rs`
  - Use the revision-2 tokens and `LV_THEME_DEFAULT_DARK`.
  - Build Barlow and Barlow Semi Condensed at 12/13/15/20/26/48 with `lv_font_conv`. Include
    the glyph range for `…`, `·`, curly quotes, `−` and Latin-1.
  - Add a five-glyph icon font: microphone, microphone off, bookmark, speaker −, speaker +.
  - Replace every PNG dependency with `LV_SYMBOL_*` or the icon font.
- [ ] **P0.4 Components** (L) — new `waved-ui/src/components/`
  - Top-layer pieces:
    - status cluster and tappable status pill
    - message toast: not clickable, severity icon, 2 s / 4 s, replaces the status pill
  - Buttons: Back; side button with hold-repeat; primary with an optional second line; button
    rows of two and three.
  - Dial with four modes: ring, talker arc, gauge and progress.
  - Lists: rotary list (`lv_example_scroll_6` pattern, bottom fade, arc scrollbar), radio list,
    value row, switch.
  - Controls: value arc, confirmation dialog. The radial picker moves to B2, the only screen that uses it.
  - Each component gets a host-side LVGL snapshot test.
- [x] **P0.5 Navigation** (M) — `waved-ui/src/nav.rs`, `lib.rs`
  - Navigation stack with `back()`.
  - Gestures: swipe down from the top edge opens Menu; swipe right goes back.
  - Wake guard: the first touch after dimming only wakes the screen.
  - Gestures and the wake guard land with the milestone 2 runtime switch, when the router drives the screen.
  - Meeting-state routing (Home ↔ Recording ↔ After stop) that works from every screen. Today
    it only works while one of those three is showing.
- [ ] **P0.6 UI test harness** (M) — host build of `waved-ui` that renders any screen and state
  to PNG. Every workstream uses it to check its screens against the mockups without hardware.

## A — Main screens

- [ ] **A1 Home** (M) — `screens/home.rs`
  - Tapping the dial mutes.
  - Quieter / Louder with hold-repeat; the volume gauge shows for 1.5 s.
  - The status pill opens Menu.
  - Record button with a second line "includes last 30 s".
  - Ring states as in the README dial table, including "In a call" (HID off-hook) and
    "Computer is using the mic" (`upstream_active`).
- [ ] **A2 Recording** (M) — `screens/recording.rs`
  - Same dial and sides as Home.
  - Button row: Mark, Pause, Stop. Stop is hold-to-stop.
  - Muted state, paused state, timer h:mm:ss after 59:59.
  - "Marked at mm:ss" message.
- [ ] **A3 After stop** (S) — `screens/confirmation.rs`
  - "Send this recording?" with Send, Keep and Delete. Delete confirms first.
  - 60 s auto-Keep with a countdown.
  - Without a server, only Keep and Delete are shown.
- [ ] **A4 Resting screen** (M) — `screens/ambient.rs`, needs E1 and E2
  - Dimmed clock, mic state and pre-recording dot.
  - The recording variant shows the ring and timer and never turns fully off.
- [ ] **A5 Start-up and first start** (S) — `screens/boot.rs`, `screens/first_run.rs`
  - Start-up: the ring draws itself while services start.
  - First start: shown once, "Set up Wi-Fi" or "Use without Wi-Fi".
- [ ] **A6 Messages** (S) — `toast.rs`
  - Rewrite on top of the P0.4 message component.
  - Copy for every existing message, plus "Marked at", "Hold Stop to end the recording",
    battery and microphone faults.

## B — Settings screens

- [ ] **B1 Menu** (S) — `screens/settings.rs`
  - Rotary list with live values: Files, Audio mode, Noise reduction, Volume, Mic height,
    Wi-Fi, Bluetooth, Uploads, System.
  - Rows whose feature isn't in scope are hidden.
- [ ] **B2 Audio mode** (M) — `screens/audio_mode.rs`, needs C3
  - Radial picker with six modes and a live preview on `lv_canvas`.
  - Drag to aim the directional modes.
  - Live DoA overlay for Follow talker.
- [ ] **B3 Noise reduction** (S) — `screens/noise_reduction.rs`
  - Full-column hit areas.
  - Plain descriptions: room untouched / softens fans and hum / most background noise /
    voices only.
- [ ] **B4 Volume and Mic level** (S) — `screens/volume.rs`
  - `lv_arc` with −/+.
  - Speaker mute in the centre.
  - Swipe to the microphone level.
- [ ] **B5 Mic height** (S) — `screens/mic_height.rs`, needs F3
- [ ] **B6 System** (M) — `screens/system.rs` + new `display.rs`, `datetime.rs`, `dialog.rs`
  - Rotary list: Display, Date and time, Alert tones, Pre-recording, USB storage, PIN, About,
    Firmware update, Factory reset, Power off.
  - Hide rows that aren't built.
  - Power off and Factory reset confirm first.
- [ ] **B7 Wi-Fi and setup** (S) — `screens/wifi_provisioning.rs`
  - Revision-2 layout.
  - No more `…` boxes.
  - Warn before "Change network" drops the current network.
- [ ] **B8 Uploads and After a recording** (S) — `screens/webhook.rs`, new `upload_mode.rs`
  - Counts, relative times, and Retry shown only when something failed.
  - Radio list for automatic / ask / local only.

## C — Recording and audio backend

- [ ] **C1 Pause and resume** (M) — `waved-meeting/src/lib.rs`, `recorder.rs`
  - Handle `PauseRecording` / `ResumeRecording`; today the commands exist but nothing
    handles them.
  - Store pause spans in the metadata.
  - Keep the timer correct.
- [ ] **C2 Pre-recording setting** (S) — `recorder.rs` (`PREBUFFER_SECONDS`)
  - Config toggle and length.
  - Clear the buffer when the setting is turned off.
  - Expose the state to the UI.
- [ ] **C3 Fixed aim for directional modes** (M) — `waved-audio/src/lib.rs:659-662`, `beam/`
  - Today Cardioid, Supercardioid and Figure-8 chase the raw DoA every frame. Use a persisted
    `SetDoaOverride` instead.
  - Follow talker maps to `AudioMode::Steered`.
  - Stereo keeps the device orientation.
- [ ] **C4 Stereo over USB** (S) — `waved-audio/src/uac/upstream.rs:110`
  - Send the beamformer's L/R pair in Stereo mode. Today it sends mono copied to both channels.
- [ ] **C5 Mic level** (S) — wire `audio.mic_volume` and `SetMicVolume` to the capture gain
  for the on-device control; today only the host's UAC control sets it.
- [ ] **C6 Microphone health check** (S) — `waved-audio/src/mic_check.rs` (declared, never called)
  - Run it at start-up and expose it in stats.
  - Show "Microphones not responding" when it fails.
- [ ] **C7 Recording decisions** (S) — `waved-meeting`
  - `ask_timeout_s` auto-Keep.
  - `DeleteRecording` for kept files, with audit-safe removal.
  - Resend for failed uploads.
- [ ] **C8 Playback** (M) — new playback source into the speaker path
  - WAV reader, play/pause/seek, jump to marks.
  - Pauses while a call is active.
- [ ] **C9 Audio acceptance (carried over)** (L)
  - Live full-duplex AEC acceptance, including a double-talk gate and an explicit CPU-headroom
    decision. The residual stage costs +29.5 points of one core.
  - Stock-vs-waved audio fixture.
  - Upstream mic EQ check.
  - SRP-PHAT validation before changing the DoA default.
  - Build DFN3 once at start-up instead of on every NR switch, which drops ~50 frames.
  - Profile noise suppression under sustained speech.
  - `ns_frame_ms` stat; remove stale `/tmp/audio_stats.json.tmp`.
- [ ] **C10 Apply the persisted beam aim** (S) — `waved-audio/src/lib.rs`, needs C3
  - `SetBeamAim` is clamped, persisted to `audio.beam_aim_deg` and seeded into
    `AudioState.beam_aim_deg` at boot, but no service consumes the command (D2).
  - AudioService maps it to the DoA override for Focused / Narrow / Two sides and updates
    `AudioState.beam_aim_deg`.
- [ ] **C11 Live recording settings in MeetingService** (S) — `waved-meeting/src/lib.rs:152`, `:170`, `:458`
  - MeetingService reads `api.upload_mode` and the audio mode for metadata from the boot-time
    config, so `SetUploadMode` and audio-mode changes only apply after a restart.
  - Subscribe to `bus.settings` and the audio watch and use the live values.
- [ ] **C12 Publish the pre-recording state** (S) — `waved-meeting/src/lib.rs`, needs C2
  - `MeetingState.pre_recording_active` has no producer yet, so the kit's pre-recording dot in
    the status pill (Home) never shows on the device.
  - Set it while the pre-roll buffer is filling: setting on, not recording, not muted.

- [ ] **C13 Neural noise reduction on the NPU** (L) — deferred until milestone 2 is done (user, 2026-10-06). DeepFilterNet on the CPU takes ≈ 84 % of a Cortex-A55 core during speech; in series with the DSP chain the UAC2 uplink underran (183 gaps / 20 s), and a dedicated thread didn't help (it alone is ≈ 8.5 ms per 10 ms hop). Noise reduction stays Off on the device until it runs on the NPU (see `memory/project_sp4_npu_status`, `docs/AUDIO_HARDWARE.md` "Uplink Level and CPU").
- [ ] **C14 Re-measure ERLE after the 48 kHz echo canceller fix** (S) — SP#5/SP#6 figures were taken while Speex assumed 8 kHz. Quick check on 2026-10-06: linear 15.6 dB, effective 23.5 dB with speech from the Wave speaker; residual echo sits ≈ 10 dB under near-end speech in the uplink. Rerun the acceptance script.
- [x] **C15 Stereo alignment** (host-verified; confirm on device) (S) — in Stereo mode the side signal comes from the raw beam while the mid carries the AEC/NS delay (DeepFilterNet adds ≈ 40 ms when on); delay the side to match.
- [x] **C16 Talker wedge follows room noise** (S) — the DoA VAD gate reads open in almost every sample of a quiet room. Fixed: the wedge and the Follow talker preview gate on `doa_confidence >= 0.30` with 150 ms on / 100 ms off hysteresis (`kit::talker`). Confirm on the device.

- [ ] **C17 Dereverberation** (L) — after today's fixes (2026-10-06) callers still hear the voice "subtly further away" in Whole room: mostly room reverb at desk distance. Stock firmware uses neural dereverb; do it with C13 on the NPU.
- [x] **C18 Bass for the directional modes** (host-verified; confirm on device) (S) — Focused, Narrow, Two sides, Follow talker and Stereo are first-order differential beams: −16 dB at 125 Hz, −10 dB at 250 Hz (simulated, front, el 0). Add a limited low-shelf compensation so they don't sound thin.
  - Cardioid aimed at a mic diagonal has an on-axis null near 3.4 kHz (Stereo sides always); needs a different beam above ~2 kHz
- [x] **C19 Show the Whole room tracker in the stats** (host-verified; confirm on device) (S) — Whole room delay-and-sums on the tracked talker and falls back to the plain average (−12 to −25 dB at 4 kHz) while no direction is confident. Publish `omni_mix` and the steered direction in `/tmp/audio_stats.json` to check it engages in real rooms.

## D — Files

- [ ] **D1 Files list** (M) — new `screens/files.rs`
  - Attention first, then newest.
  - Upload-state icons.
  - Free-space line, amber below the recording block threshold.
  - Empty state.
- [ ] **D2 Recording detail** (M) — new `screens/file_detail.rs`, needs C8
  - Scrubber dial with mark ticks.
  - Delete (confirms first) and Send / Send again.
  - Failure reason in plain words.
- [ ] **D3 Storage visibility** (S) — free space and retention thresholds in state; low-space message

## E — System services

- [ ] **E1 Display** (S) — `display.rs`
  - Brightness through `/sys/class/leds/lcd-backlight`; today it's fixed at 255.
  - Dim after 2 min, off after 10 min.
  - Never fully off while recording or in a call.
- [ ] **E2 Power key** (S) — read `KEY_POWER` from `/dev/input/event0`, which is never read today
  - Short press: sleep or wake.
  - Hold 2 s: power off, no confirmation (the MCU powers off by itself, device pass 2026-10-12).
- [ ] **E3 Time** (M)
  - Time zone and 24-hour clock.
  - Check the RTC on i2c-3.
  - Refresh the status clock every minute; today it's UTC and only refreshes when an icon
    changes.
- [ ] **E4 Languages** (M), deferred to a later round (D5) — `i18n.rs` is loaded and then dropped (`lib.rs:71`)
  - Route every string through it.
  - English first, then the stock language list.
  - Language screen as a radio list.
- [ ] **E5 Alert tones** (S) — `audio_fx.rs`
  - On/off setting.
  - Recording start and stop tones, so everyone in the room hears the consent cue.
- [ ] **E6 Factory reset** (S) — config, recordings, Wi-Fi and Bluetooth pairings, behind a
  confirmation.
- [ ] **E7 About** (S) — device name, waved version, serial, IP, free storage, licences.

## F — Hardware

- [ ] **F1 Physical volume buttons** (M)
  - Find the real input path. It isn't `ttyS1`, `event0` or `event1`, see
    `waved/docs/AUDIO_HARDWARE.md:648`.
  - Wire the buttons to the speaker volume.
- [ ] **F2 Battery** (S)
  - Level, voltage, temperature and charging now come from the MCU's heartbeats on
    `/dev/ttyS4` (`waved-mcu`, 2026-10-12); check the status pill against them on battery.
  - Status pill variants for charging and low battery.
  - Messages at 10 % and 5 %.
  - At 3 %, finalize any recording, then shut down.
- [ ] **F3 Motor** (S) — verify the `MotorPosition` frames and the obstruction event on
  the device before Mic height ships.
- [ ] **F4 LEDs** (S) — find out whether the device has status LEDs and drive them from state.
- [ ] **F5 Panel type** (S) — the device tree says LCD (D8). Confirm visually by setting the backlight to 0, then
  fix "circular OLED" in `DISPLAY_HARDWARE.md` and `waved/CLAUDE.md`.
- [x] **F7 Power key through the MCU** (M) — done 2026-10-12: the key arrives on the
  MCU link (`/dev/ttyS4`, MAVLink, `waved/docs/MCU_PROTOCOL.md`); click = sleep / wake,
  2 s hold = power off (the MCU powers off by itself on a hold). Follow-up: the Bluetooth
  UART (`/dev/ttyS1`) runs at 115200 in waved but is 921600 in stock.
- [x] **F8 Boot screen** (S) — the Insta360 image is drawn by the signed, encrypted
  bootloader (the `logo` partition is empty, the kernel has no logo), so it can't be
  replaced safely. Cover it from userspace instead: an early init script writes our
  splash to `/dev/fb0` (~2 s after power-on instead of ~10–15 s until waved draws).
- [ ] **F6 Peripheral blobs** (S) — adopt the 1.2.2.6 `BT.bin` and `DONGLE.bin` (MCU is unchanged).
  Note that 1.2.2.6 also changed the root password hash.

## G — USB storage

- [ ] **G1 Mass-storage mode** (M)
  - Switch the gadget to mass storage over the recordings volume and pause audio.
  - Takeover screen with "Stop USB storage".
  - Recover cleanly on cable pull.

## H — Updates (self-hosted, signed)

- [ ] **H1 Package and signature** (M) — waved image format, ed25519 signature, version
  metadata. Fall back to the previous image when boot health checks fail.
- [ ] **H2 Update service** (M) — check `security.ota_server` (configured but unused today),
  download, verify, install, reboot. Expose progress in `OtaState`.
- [ ] **H3 Firmware update screen** (S) — progress ring, plus failure state with Retry.
- [ ] **H4 Going back to stock** (S) — document and test restoring a stock `.swu`. This was an
  open question in the old notes.

## I — Bluetooth

- [ ] **I1 Pairing** (L) — `waved-bt`
  - Discoverable for 2 min.
  - Paired list; connect, disconnect, forget.
  - Name from `device.name`.
- [ ] **I2 Audio routing** (L) — A2DP/HFP into the pipeline, with the AEC reference tap.
- [ ] **I3 Calls** (M)
  - Incoming-call screen with Answer and Decline.
  - HID hook-switch to the computer.
  - Phone number only, no phone book.
- [ ] **I4 Bluetooth screen** (S) — `screens/bluetooth.rs`.

## J — Wave Controller

- [x] **J1 Protocol from the decompiled stock code** (M) (D9) — `g_pc_link_slave_cmd_handle_array`
  in `firmwares/fw_v1.2.2.6/decompiled/libins_entry.so.c`, plus the MAVLink message definitions.
  Write it down as a protocol doc before coding. Done: `waved/docs/WAVE_CONTROLLER_PROTOCOL.md`.
- [x] **J2 PC Link transport** (L) — `waved-hid/src/reader.rs` drops report 0x03 today.
  - MAVLink over HID, heartbeat, retransmit.
  - The `auth_dev` session token.
  - Done: `waved-hid/src/pclink/` on the shared `waved-mavlink` codec.
- [x] **J3 Commands** (L) — done in `pclink/handler.rs`; K2's PIN check and a clock setter
  still have to be plugged into `pclink::service::spawn` (it starts with `DenyAllPins`, `NoClock`).
  - Audio: volumes, mute, mode.
  - Meeting: start, stop, mark, status.
  - System: info, time, language.
  - Wi-Fi status.
  - Cloud and binding commands get a clean "unsupported" reply.
- [x] **J4 Mode mapping** (S) — stock modes 1–6 ↔ the six waved modes from D2 (`pclink/mode.rs`).
- [ ] **J5 Verify against the official app** (M) — once a capture of Wave Controller 1.1.0
  against a stock 1.2.2.6 unit is possible (second unit, or a temporary restore via H4), diff it
  against our implementation and fix the gaps.

## K — Security and privacy

- [ ] **K1 Release lockdown** (M)
  - adb off or authenticated in release builds.
  - No telnet.
  - A root password policy.
- [ ] **K2 PIN lock** (M) (D10) — `security.device_password` is stored but not enforced.
  - New `screens/pin.rs`: 3 × 4 keypad.
  - Guards Files, Wi-Fi, Bluetooth, Uploads and System.
  - Unlocked until the screen rests; five wrong tries means a one-minute wait.
  - Setup page validates 4–6 digits.
  - The Wave Controller's `verify_dev_passwd` uses the same check (J3).
- [ ] **K3 Setup network review** (S) — WPA2 setup AP timeout and credential rotation.
- [ ] **K4 Privacy indicators audit** (S) — recording ring on the resting screen, pre-recording
  indicator, start and stop tones, and mute that also silences the recording (the current
  behaviour, keep it).

## L — Docs and cleanup

- [ ] **L1 Display docs** (S) — panel type, and the stale home layout in `DISPLAY_HARDWARE.md:82`.
- [ ] **L2 UI docs** (S) — `docs/ui/HOME_SCREEN.md`, plus the stale `bg_main.png` warning in
  `ASSETS.md:13`.
- [x] **L3 Dead code** (S) — delete `screens/control_center.rs`, which isn't compiled, and the
  `cc.*` strings.
- [ ] **L4 Single roadmap** (S) — reduce `waved/TODO.md` to acceptance evidence and point it here.
- [ ] **L5 Parity matrix** (S) — update `docs/FIRMWARE_GAP_ANALYSIS.md` as workstreams land.

## M — Recording and delivery acceptance (carried over, P0/P1)

- [ ] **M1 One-hour and power-loss certification** (M, P0) — `scripts/test-recording-resilience.sh`
  - Run it with `DURATION_SECONDS=3600`.
  - Repeat the active and finalization cases with physical power removal.
- [ ] **M2 Webhook acceptance script** (M) — `scripts/test-webhook-setup.sh`
  - Cover the setup form, the three upload modes, auth redaction, multipart metadata, WAV
    properties and idempotency.
- [ ] **M3 Failure and recovery matrix** (M)
  - Offline recording, Wi-Fi loss during upload, retries, permanent 400.
  - Redirect rejection, retry exhaustion, endpoint change, reboot during upload.
- [ ] **M4 Production HTTPS webhook** (S) — real endpoint, cold boot, one accepted request per
  meeting UUID.
- [ ] **M5 Wi-Fi hardening** (M) — automated cold-boot provisioning regression; multi-AP
  roaming with a local receiver.

## Known bugs

The redesign fixes these unless an item ID is given.

| ID | Bug | Where |
|---|---|---|
| BUG-1 | Back button outside the visible circle on System and Webhook | `system.rs:123`, `webhook.rs:179` |
| BUG-2 | Titles hidden under the status bar | `system.rs:115`, `webhook.rs:173` |
| BUG-3 | Toast and status-bar roots are clickable and swallow taps; the toast covers Save for 1.5 s after every stop | `toast.rs:16`, `status_bar.rs:61` — fixed by the M2 runtime switch |
| BUG-4 | `…` and `·` render as boxes (no glyph) | fonts, P0.3 |
| BUG-5 | Save/Send/Discard icons never deployed, buttons render blank | `confirmation.rs:244` — fixed by the M2 runtime switch |
| BUG-6 | Done buttons are white discs (light default theme) | `audio_mode.rs:488`, `noise_reduction.rs:282` |
| BUG-7 | Power off has no confirmation | `system.rs:52` |
| BUG-8 | Six System rows and Files show "Bookmark added" | `system.rs:37`, `settings.rs:70` |
| BUG-9 | Status clock shows UTC and goes stale | `status_bar.rs:143`, E3 |
| BUG-10 | Webhook labels overlap when text wraps | `webhook.rs:147` |
| BUG-11 | Audio mode always returns to More, even when opened from System | `lib.rs:474`, P0.5 — fixed by the M2 runtime switch |
| BUG-12 | No way to reach settings or volume while recording | A2 |
| BUG-13 | Meeting-state screen switching only works on the three main screens | `lib.rs:253`, P0.5 — fixed by the M2 runtime switch |
| BUG-14 | Language table loaded and thrown away | `lib.rs:71`, E4 |
| BUG-15 | Stereo mode sends mono | `uac/upstream.rs:110`, C4 |
| BUG-16 | Pause and resume commands are never handled | `waved-meeting/src/lib.rs`, C1 |
| BUG-17 | Directional modes chase the raw DoA each frame, so the pattern jumps between talkers | `waved-audio/src/lib.rs:659-662`, C3 |

## Not planned

Excluded by design, each with the reason it stays out:

- **On-device transcription, subtitles, AI summary, QR activation.** Downstream services
  handle these from the webhook.
- **Insta360 cloud, binding, accounts, telemetry, log upload.** These are the reason the
  project exists.
- **Phone book (PBAP) sync.** It's a privacy cost for a name on a call screen.
- **HD MP3 recording mode.** Lossless WAV is already better.
- **Daisy-chaining several Wave units (Business SKU).** Revisit if someone needs it.
- **Speaker names in diarization.** Speakers get numbers per meeting only.
- **NPU inference.** DFN3 on the CPU fits the budget, and no open toolchain supports the
  VIP9000 variant. Revisit if one appears or CPU becomes the bottleneck.
- **Raw eight-channel microphone over USB.** Revisit when an application asks for it.
