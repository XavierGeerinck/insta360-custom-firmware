# Milestone 3, I1 / I3 / I4: Bluetooth control — plan

**Goal:** drive the Telink module from the screen. Pair a phone, see and drop the connected
phone, and answer or decline a phone call on Wave. Bluetooth audio routing is I2 (another
agent); this work only reads the `0xB2` audio bits it already decodes.

**Spec:** root `TODO.md` I1, I3, I4, D10 and "Not planned" (no phone book);
`docs/mockups/png/bluetooth.png`, `call.png` and their captions in `docs/mockups/index.html`;
`docs/BLUETOOTH_PROTOCOL.md`, especially "Verified on the device (2026-10-13)".

Already in place: framing, commands, the event decoder, `Tracker` and a passive `BtService`
(acks every frame like stock) in `crates/waved-bt`; `BluetoothState.{pairing, phone_audio,
call_audio}` from `0xB2`; `Route::Bluetooth` and `Route::IncomingCall` (no screens); the router
puts the PIN in front of `Route::Bluetooth` (D10).

## Fixes found on the device (first, TDD)

1. **Phone book.** `0xB0` decodes to `Event::PhoneBookEntry`, which carries nothing and is
   never logged at any level. It is still acknowledged, so the module stops repeating it.
   The `waved-uart` reader's trace line no longer prints frame bytes either (it printed every
   payload, the phone book included).
2. **Logs.** Module log lines (`0xF0`) go to trace only. No info or debug line carries a
   phone's name, MAC or number: `ConnectRequest`, `Heartbeat`, `PhoneMac` and
   `DeviceDisconnected` lose their payload in the log. The log lines come from one pure
   function (`log_line`) so a test can check every event.
3. **`PhoneMac`.** The reply is `[?] [6] mac[6]`: the device sent `06 A1 B2 C3 D4 E5 57` after
   one leading byte, for `app_set_bt_last_addr A1B2C3D4E557`. The MAC starts at byte 2.

## Decisions

1. **No paired list (the mockup's "Paired" rows are left out).** The module keeps its link
   keys and has no command to list, connect to or forget a phone. A host-side list could only
   show names we can neither connect nor forget. The screen shows the connected phone only.
   *Cost if wrong:* add a list later (persist name and MAC when a phone connects); the row
   component is in place.
2. **Pair = `Disconnect` then `EnterPairing`, back to back**, exactly stock's "Connect a new
   device" (`bluetooth_set_connect(0)`, libui 115208; verified on the device). Wave shows a
   2-minute countdown. The window ends at 2:00, when a phone connects (a rising edge, so a
   phone still dropping does not end it), or when the module clears `0xB2` bit 3 after setting
   it. Nothing is sent at the end: no command to leave pairing is known.
   *Cost if wrong:* the module may stay discoverable after the countdown (privacy); the
   device pass measures how long bit 3 stays set.
   **Superseded (second round, R2/R4):** `AutoReconnect(false)` goes first.
3. **Disconnect = `Disconnect` only.** Stock's UI disconnect also enters pairing; ours does not,
   since the user asked to drop the phone, not to pair another.
   *Cost if wrong:* the phone may reconnect at once; the device pass checks.
   **Superseded:** it did reconnect at once (second round, R4).
4. **An incoming call takes over the screen, also during a recording.** A ringing call is the
   one thing on Wave that cannot wait, and the call screen is the only way to answer it here.
   The recording keeps running underneath; the call screen never touches it. A swipe right
   (Back) dismisses it for the rest of that call; it leaves by itself when the call ends. It
   never shows during start-up, first start, an install or power off. A ring wakes a resting
   screen.
   *Cost if wrong:* for the length of a ring, the call covers Recording's Stop and Mark (one
   swipe away), and the caller's number is visible to the room.
5. **After Answer the call screen stays, as "In a call" with Hang up** (button event 0, as on
   stock's calling page), so a call answered on Wave can be ended on Wave. No timer and no
   mute there: Back shows Home, which says "In a call" for a Bluetooth call too and mutes on a
   tap. There is no mockup for this state; it reuses `call.png`'s ring and text.
6. **Guards.** Answer is sent only while a call rings and Decline / Hang up only while there is
   a call: event 3 doubles as a music key on the module.
7. **Phone number only.** The number lives in `BluetoothState::call` (`BtCall`), whose `Debug`
   never prints it. It is never logged and never written to disk. The phone's own Bluetooth
   name shows on screen ("via iPhone") but never in an info or debug log line.
8. **No QR code for pairing.** Phones cannot pair classic Bluetooth from a QR code (iOS not at
   all, Android only through Google's Fast Pair). The pairing screen shows the module's name
   large with "Pick this name in your phone's Bluetooth list".
9. **Call state** (`docs/BLUETOOTH_PROTOCOL.md` outline): `Idle` → `Ringing { number }` on
   `0xB1`; → `Active` on `0xD1 00` (only while a number is known, as stock); → `Idle` on
   `0xD1 01`, an empty `0xB1` or the phone dropping. `0xB2` bit 2 does not start a call:
   phones with an in-band ring tone open the audio link while ringing.
10. **Left out:** the HID hook-switch to the computer (I3; it belongs to the USB HID side and
    needs a telephony report, not this link); the ripple animation (the rings are drawn as the
    mockup's still); naming the module from `device.name` (done in the third round, R13);
    call waiting and outgoing calls (see the checklist).

## Second round: the device pass of a856f70 (2026-10-13)

What the device showed, and the research behind each fix, is in `docs/BLUETOOTH_PROTOCOL.md`
("Acknowledgements and replies", "Stock's Bluetooth controls", "Seen on the device with waved
a856f70", "What waved does with this").

- **R1. Frames repeating twice a second (`0xC6`, `0xAA`).** waved already acknowledged every
  frame with stock's bytes; two things went wrong. (a) `waved-uart`'s mux shared one port
  handle behind a mutex, and the reader starved the writer: an acknowledgement waited 1.4 s
  on a quiet pty, past the module's 500 ms repeat. The reader now has its own handle
  (`try_clone`), as `waved-mcu` does. (b) waved also acknowledged the module's one-shot `75 46`
  answers, which hands it our own request byte back. Now: every `75 45` frame but `0xE0`, no
  `75 46` frame (a deviation from stock, which never looks at the prefix). Log lines are info
  only when the frame brought something new.
  *Cost if wrong:* if the module wanted `75 46` answers acknowledged it would repeat them; the
  capture shows it never does. If neither cause was the one, `0xC6` still repeats, now at debug.
- **R2. Leaving the pairing screen ends pairing.** The Bluetooth route leaves `BtStopPairing`
  (runtime leave commands): Back, a swipe, ToBase, a takeover. Resting does not end it (the
  route stays parked; a phone may take longer than the dim time to pair). Nothing goes to the
  module: stock has no command that leaves pairing mode, and the least disruptive choice is to
  send nothing. *Cost if wrong:* the module stays discoverable until its own timeout (privacy);
  the checklist measures it. A stronger option, if it is long: `CloseBt` + `OpenBt`.
- **R3. Pair another phone while connected.** The connected view has Disconnect (second row)
  and Pair another phone (where Pair a device sits): `BtPair`, stock's Connect New Device. No
  confirmation (stock asks "Connecting a new device will disconnect from <phone>. Continue?"):
  the countdown screen follows at once and Back ends it. *Cost if wrong:* an accidental tap
  drops the phone; a confirm step (`ConfirmKind`) can be added. **Superseded (R14):** it asks.
- **R4. Disconnect that sticks.** Stock sends `0xD8 00` (reconnect off) as each phone connects,
  so its disconnects never race the module's reconnect; waved never sent `0xD8`. Now
  Disconnect = `D8 00`, `A3 00`, and Pair = `D8 00`, `A3 00`, `A7 00`. The copy stays
  "Disconnect" (it is not a Bluetooth switch); the idle hint now says "Reconnect a phone from
  its Bluetooth settings". *Cost if wrong:* if the iPhone reconnects by itself, only stock's
  Bluetooth switch (`CloseBt`) stops it; that is not built: `CloseBt` alone left the module
  reporting "connected" for at least 10 s (observation 5), so it needs a device test first
  (checklist S6) and host-side "not connected while off" as stock does.
- **R5. `OpenBt` is `CC 00`.** Stock sends 00 at boot and from the switch alike; waved's
  `OpenBt { at_boot: true }` encoded `CC 01`. Now the unit `Command::OpenBt`. Unused so far.
- **R6. No phone data at any level.** The first round logged module log lines (`0xF0`, which
  carry the phone's name and MAC) and undecoded frames' bytes at trace. Now a module line
  shows only as one of the module's fixed messages about pairing, reconnecting and its radio
  (`MODULE_MESSAGES`, at debug, so S5 can tell who reconnected), else as its length; an
  undecoded frame as its command byte and length. *Cost:* other module diagnostics are gone
  from the log; add a fixed message to the list to see it.

Files: `waved-uart/src/mux.rs` (reader handle, prefix in the trace line);
`waved-bt/src/{command,control,lib}.rs` (`ack_for_frame`, `OpenBt`, `D8 00` first,
`BtStopPairing`, info-once logging, `Response::acted`, `MODULE_MESSAGES`); `waved-ui/src/screens_v2/bluetooth.rs`
and goldens `screen_bluetooth`, `screen_bluetooth_idle`; `waved-ui/src/kit/README.md`; this plan
and `docs/BLUETOOTH_PROTOCOL.md`.

Hotspot edits (additive): `waved-state` `Command::BtStopPairing` (and its test list);
`waved-ui/src/runtime/gesture.rs` `leave_commands` (Bluetooth) and a test;
`waved-ui/src/runtime/mod.rs` a test with its registry helper. `crates/waved/src/main.rs` is
untouched.

## Third round: the device pass of 15f8d8d (2026-10-13)

What the device showed: a call answered on the iPhone left the Wave on Answer / Decline (no
`0xD1`, no empty `0xB1`); the iPhone's volume did nothing; 175 `0xAE` frames in 40 minutes.
Research and line numbers are in `docs/BLUETOOTH_PROTOCOL.md` ("When stock leaves its call
pages", "Calls in waved", "Volume", "Firmware").

| ID | Decision | Cost if wrong |
|---|---|---|
| R7 | **Calls end on every sign** (`waved-bt/src/call.rs`). Stock leaves its call pages only on `0xD1 01`, the phone dropping and its own buttons; waved also takes the module's log lines `HFP  Call active` / `HFP call end` / `HFP RFCOMM_CLOSED`, the call audio (`0xB2` bit 2) closed for 3 s after it was open, and 60 s without an `0xB1` while ringing (then "In a call" if the call audio is open, else over). `0xA6` and the other `0xB2` bits do not count (stock uses them for audio only). | A ring whose in-band SCO link drops for over 3 s is dismissed while the phone still rings; a call answered on the phone with audio here shows "Incoming call" for up to 60 s. |
| R8 | **Answer, Decline and Hang up act at once**, as stock's buttons (libui 27958, 27920, 28702): Answer shows "In a call" without waiting for `0xD1 00`; Decline / Hang up end the call, and the same number repeated within 10 s does not ring again. | If the module did not take the button, the screen is wrong until the next sign; the user can tap again from Home ("In a call"). |
| R9 | **The phone's volume sets the speaker** while the phone's audio plays, through stock's `map_volume_100`, music and call kinds alike (stock ignores the call kind). Level 0 sets 0 without muting; above 0 unmutes. Marked as not the Wave's own (no HID volume steps to the computer). | Double attenuation in calls if the module already applies the phone's call volume: drop the call kind. |
| R10 | **Nothing goes back to the phone** when the Wave's volume changes. Stock does send `0xD0 01 / 02 / 09` (1.1.6.6 with the level, 1.2.2.6 without), not "nothing" as the doc said; what the module does with them is unknown, and an echo through `0xD5` could fight the knob. | The phone's slider does not follow the Wave. T3 tries one event by hand. |
| R11 | **Stock's Bluetooth switch**: config `[bluetooth] enabled` (default on, waved-settings persists it), `SetBluetooth`, a switch at the end of the device row (the only target there). Off: `CloseBt`, the phone counts as gone at once, and `0xB2` bits 0-2, `0xB4`, `0xAA`, `0xB1`, `0xD1` and `0xD5` are ignored until on (stock ignores only `0xB2` bit 0 and `0xB4`; `0xAA` because the module kept answering `0xAA 01` after `CloseBt`). | If the module never drops the phone after `CloseBt`, the phone may still play through the module's own path; T6 checks. |
| R12 | **Stock's start-up sequence** at every waved start: `SetName` until `0x8C`, `OpenBt` / `CloseBt`, `D8 01` (on) / `D8 00` (off), `SaveFlash`, versions, status, `GetName` until `0xC6`, `PtaMode(1)`, then `GetPhoneMac`. No Auto Reconnect setting: on means reconnect to the last phone, as stock's default copy reads. Stock's 10 s of `0xD0 01` volume events before it are left out (R10). | `OpenBt` / `D8 01` on a module that is already running could disturb a connected phone at every waved restart (T5); reconnect at start may bring back a phone the user disconnected before a restart. |
| R13 | **Module name = `device.name`**, cut to 23 bytes, NUL-padded to stock's 23; empty → stock's `Insta360 Wave BT <serial digits>`. The mockup shows "Shows up as Wave Boardroom". Two Waves left at the default "Wave Open" show the same name. | If the module refuses or garbles a shorter name, use stock's format (`startup::module_name`). |
| R14 | **Pair another phone asks first** (`ConfirmKind::PairAnotherPhone`), as stock's Connect New Device (supersedes R3). | One more tap. |
| R15 | **F6 by hand only**: `ota::flash` with stock's waits, tries, CRC check and byte-exact frames (the module's size with stock's `0xAB` low byte), run only by `waved-bt-flash` (`check`, `version`, `flash … --owner-approved`, refuses while waved runs). Stricter than stock on its failure paths. | A bad flash leaves the module without Bluetooth until flashed again; the boot area is never sent and btpower is the first recovery. |

Files: `waved-bt/src/{call,startup,volume}.rs` (new), `{lib,control,state,event,ota}.rs`,
`src/bin/waved-bt-flash.rs` (new); `waved-ui/src/screens_v2/{bluetooth,confirm,menu}.rs`,
`kit/route.rs`, `kit/components/controls.rs` (`Switch`), goldens `screen_bluetooth`,
`screen_bluetooth_idle`, `screen_bluetooth_off`, `screen_confirm_pair_another_phone`;
`waved-settings/src/change.rs`; docs.

Hotspot edits (additive): `waved-state` `Command::SetBluetooth` (+ tests, a doc line on
`bt_on`); `waved-config` `BluetoothConfig` / `[bluetooth] enabled` (+ `config/default.toml`);
`crates/waved/src/main.rs` `.with_settings(..)` and `.with_speaker(..)` on `BtService`;
`waved-ui/src/runtime/registry.rs` one route in a test list.

## Files

| File | Change |
|---|---|
| `crates/waved-bt/src/event.rs` | `Event::PhoneBookEntry`; `PhoneMac` at byte 2 |
| `crates/waved-bt/src/lib.rs` | `log_line`; `BtService::with_commands`; 100 ms loop with the 5 s poll and the countdown |
| `crates/waved-bt/src/control.rs` | new: bus commands → frames, the pairing window |
| `crates/waved-bt/src/state.rs` | the call state, the module's name |
| `crates/waved-uart/src/mux.rs` | the trace line prints no payload |
| `crates/waved-state/src/lib.rs` (hotspot) | `Command::{BtPair, BtDisconnect, BtAnswerCall, BtDeclineCall}`; `BluetoothState.{advertised_name, pairing_left_s, call}`; `BtCall` |
| `crates/waved/src/main.rs` (hotspot) | `.with_commands(bus.commands.subscribe())` |
| `crates/waved-ui/src/screens_v2/bluetooth.rs` | new: `BluetoothScreen` |
| `crates/waved-ui/src/screens_v2/call.rs` | new: `CallScreen` |
| `crates/waved-ui/src/screens_v2/menu.rs` | the Bluetooth row |
| `crates/waved-ui/src/screens_v2/mod.rs` | two modules |
| `crates/waved-ui/src/kit/router.rs` | the call takeover in `sync`, dismissal on Back |
| `crates/waved-ui/src/kit/snapshot.rs` | `in_call` counts a Bluetooth call; `bt_ringing` |
| `crates/waved-ui/src/runtime/{mod,registry}.rs` (hotspots) | a Bluetooth ring wakes the screen; register both screens |
| `docs/BLUETOOTH_PROTOCOL.md`, `crates/waved-ui/src/kit/README.md`, root `TODO.md` | docs |

## Tasks (TDD: failing test first)

1. **Fixes.** Tests: a `0xB0` wire frame decodes to `PhoneBookEntry` whatever its length and
   is acknowledged `75 46 02 B0 00 B2`; `log_line` returns nothing for it; module logs are
   trace; no info/debug line for a name-, MAC- or number-carrying event contains the name, the
   number or the MAC in hex or decimal; the `PhoneMac` wire fixture
   `75 46 09 C8 00 06 A1 B2 C3 D4 E5 57 FD` decodes to `A1:B2:C3:D4:E5:57`; the mux describes
   a frame without its payload.
2. **State contract.** Tests: the new commands exist; `BtCall` defaults to `Idle` and its
   `Debug` hides the number.
3. **Tracker.** Tests: the call state machine above; the module's name from `0xC6`.
4. **Control.** Tests (byte-exact): `BtPair` sends `75 45 02 A3 00 A5` then `75 45 02 A7 00 A9`
   and opens a 120 s window; the countdown; the window ends at 2:00, on a phone connecting,
   on bit 3 clearing after it was set, but not on a phone that was already connected;
   `BtDisconnect` sends `75 45 02 A3 00 A5`; Answer sends `D0 03 <time> 00` only while
   ringing; Decline sends `D0 00 <time> 00` while ringing or active; other commands send
   nothing.
5. **Service.** `with_commands`; one loop of 100 ms: frames, commands, the countdown, the
   status poll every 5 s once the module answered.
6. **Bluetooth screen.** Goldens: not connected, pairing (1:42), connected. Tests: Pair a
   device sends `BtPair`; Disconnect sends `BtDisconnect`; the state follows the snapshot; a
   long phone name and a missing module name.
7. **Call screen.** Goldens: ringing, in a call. Tests: Answer and Decline send their commands;
   Hang up sends `BtDeclineCall`; a long number fits; no "via" line without a phone name.
8. **Routing.** Tests: Menu's Bluetooth row opens `Route::Bluetooth` and carries the lock and
   the value; a ring pushes the call screen (over Recording too) and wakes the screen; Back
   dismisses it for that call; it leaves when the call ends; never during first start; the
   registry builds both routes.
9. **Docs.** `BLUETOOTH_PROTOCOL.md` "In waved now"; `kit/README.md` registry rows; root
   `TODO.md` I1, I3, I4.

## Device-pass checklist

Deploy the branch (`waved/CLAUDE.md`, Build & Deploy), set no PIN or know it, and keep
`/tmp/waved.log` running with `RUST_LOG=waved_bt=debug,waved_uart=trace` (the `waved_uart`
trace line names each received frame's prefix, command byte and length, never its bytes:
`parsed frame: 75 45 cmd 0xc6, 24 data bytes, target Bluetooth`). Stop the watchdog and start
waved with `setsid` as in `waved/CLAUDE.md`.

### Third round (R7-R15)

Log with `RUST_LOG=waved_bt=debug,waved_uart=trace` as above. Note every answer in
`docs/BLUETOOTH_PROTOCOL.md` ("Open" and the sections named).

- **T0. Start-up.** Fresh start of waved: `grep -E "bt_srv started|bt:|bt module" /tmp/waved.log`.
  Expect `bt_srv started, Bluetooth on`, then `bt: sends 0x..` (debug) in this order: `0x8b`
  (once if `0x8C` comes, up to 10 × 1.1 s if not), `0xcc`, `0xd8`, `0xe0`, `0x91`, `0x93`,
  `0xc5` (`GetStatus` is not logged), `0xea`, `0xc7`. Record whether
  `0x8C`, `0xE0` and `0xEB` come back, and with which prefix. The phone that was connected
  before the restart: still connected after 30 s? (T5).
- **T1. Call answered on the phone** (the bug). Phone connected, Wave idle. Call it; the Wave
  shows Incoming call. Answer **on the phone** with the audio on the phone. Expect within 3 s
  of the ring ending: the screen leaves (`bt: the call audio closed: call over here` if the
  ring had in-band audio, or `bt: the module reports the call up` then "In a call" if the
  module logs `HFP  Call active`), at the latest 60 s after the last `bt: call ringing`
  (`bt: no news of the ring for 60 s: ring over`). Note the order of `bt work status`,
  `bt module: HFP …`, `bt module: SCO …` and `bt: phone event` lines during the ring, the
  answer and the hang-up. Repeat answering on the phone **with the audio sent to the Wave**:
  expect "In a call" (log line or the 60 s rule) and the screen leaving within 3 s of hanging
  up. Repeat answering **on the Wave**: "In a call" at once on Answer; hang up on the phone:
  it leaves. Decline on the Wave: it leaves at once and does not come back for that call.
- **T2. Volume from the phone.** Music playing from the iPhone through the Wave. Press the
  phone's volume keys: `grep -E "PhoneVolume|phone's volume sets" /tmp/waved.log` shows
  `bt: PhoneVolume { kind: Music, level: n, max: 15|16 }` (debug) and
  `bt: the phone's volume sets the speaker to N %` (info); Home's volume follows the steps
  0, 5, 10, 20 … 100; the computer's volume does not move. Pause the music and change the
  phone's volume: no change on the Wave. During a call, the phone's keys change the Wave's
  level (`kind: Call`). If no `PhoneVolume` line ever appears, look for
  `bt module: app_bt_avrcp_ct_vol_change` / `AVRCP_CTYPE_CONTROL @remote vol change` /
  `app_bt_hfp_hf_vol_change` and `bt frame 0x.., N bytes` lines while pressing the keys and
  record them (the module may report volume another way on 0.0.0.28).
- **T3. The Wave's volume to the phone (research, by hand).** Waved stopped, music playing:
  send stock's volume-up event with a level of 50:
  `printf '\165\105\007\320\001\000\000\000\000\062\012' > /dev/ttyS1`
  (`75 45 07 D0 01 00 00 00 00 32 0A`). Does the phone's slider move, by one step or to
  50 %? Does a `0xD5` come back? Record it; R10 changes only with this answer.
- **T4. The module's name.** Set `device.name` to "Wave Boardroom" in the setup page, let waved
  restart. Menu → Bluetooth shows "Shows up as Wave Boardroom" (the module's own `0xC6`
  answer); a phone's Bluetooth list shows "Wave Boardroom" (no trailing garbage). If the
  screen keeps "Insta360 Wave BT …", note whether `0x8C` came (T0) and try a 23-character
  name. Then empty `device.name`: stock's name comes back.
- **T5. `OpenBt` at start with a phone connected.** Phone connected and playing; restart waved
  (`killall waved`, watchdog on). Does the music stop, the phone drop, `RECONN_START!` show?
  Record how long until the screen shows the phone again.
- **T6. The switch.** Phone connected and playing. Menu → Bluetooth: tap the switch off. Expect
  at once: "Bluetooth off", no Disconnect or pairing; log `bt: SetBluetooth(false)`;
  `waved_uart` sends `CB`; the music stops; Menu shows "Bluetooth Off". Within 60 s does the
  phone show the Wave disconnected? Call the phone: the Wave shows nothing. Restart waved: it
  starts off (`bt_srv started, Bluetooth off`, `CB` and `D8 00` at start;
  `grep -A1 "\[bluetooth\]" /data/waved/config.toml` says `enabled = false`). Switch on:
  `CC 00`; the phone reconnects (from the phone if not by itself) and shows "Connected".
- **T7. Pair another phone asks.** Phone A connected: tap Pair another phone. The
  confirmation names A; Cancel returns with A still connected and nothing in the log; Pair
  sends `bt: BtPair`, returns to Bluetooth with the countdown (no `BtStopPairing` in the
  log).
- **T8. `0xAE`.** With an iPhone connected for 10 minutes: count
  `grep -c "phone book link status" /tmp/waved.log` and note the values; it must never be
  followed by a phone-book request (`AF` on the wire).
- **T9. F6, dry run first** (only with the owner's approval for the last step; procedure in
  `docs/BLUETOOTH_PROTOCOL.md`, "Adopting them in waved"). `waved-bt-flash check module
  /tmp/BT.bin` prints `version 0.0.0.33, built 2026-06-12 12:43:40`, 9213 packets and the size
  frame `75 45 05 9D AB FF 08 00 54`; `check dongle /tmp/DONGLE.bin` prints 2289 packets.
  With waved stopped: `waved-bt-flash version` prints `module firmware 0.0.0.28`,
  `module built 2025-11-28 20:15:49` and the dongle's (or "no dongle connected"). Only then,
  on USB power and with the owner present: `flash module /tmp/BT.bin --owner-approved`;
  expect `Requested`, `SizeTaken`, packets to 100 %, `CrcPassed`, `Reset`; restart waved:
  `bt module firmware 0.0.0.33` / `built 2026-06-12 12:43:40`. Record the time it took and
  any retry. If it fails: power-cycle the module through btpower and run `flash` again.

### Second round (R1-R6)

- **S1. Nothing repeats (R1).** Start waved and wait 60 s with no phone connected, then
  `grep -o "parsed frame: 75 4[56] cmd 0x[0-9a-f]*" /tmp/waved.log | sort | uniq -c`.
  Expect `0xc6` once (one `GetName`), `0xaa` about 12 (one per 5 s poll), `0x91`/`0x93` once,
  `0xb2` only when the status changes; the `75 46` answers (`0xa9`, `0xc5`, and `0xc7` or
  `0xc8`) once per request. Record in `BLUETOOTH_PROTOCOL.md` which prefix `0xaa`, `0xc6` and
  `0xc8` carry. `grep -c "INFO waved_bt: bt module name" /tmp/waved.log` = 1. With a phone
  connected and music playing for 60 s, `grep -c "INFO waved_bt" /tmp/waved.log` grows by at
  most a handful. If `0xc6` or `0xaa` still come every 500 ms, note their timestamps against
  the `GetStatus` polls and record it ("Acknowledgements and replies").
- **S2. Back ends pairing (R2).** No phone connected: Menu → Bluetooth → Pair a device; the
  countdown starts (`INFO waved_bt: bt: BtPair`, `bt work status 0x08`). Tap Back: the log shows
  `INFO waved_bt: bt: BtStopPairing`; Menu → Bluetooth again shows "No phone connected" and
  Pair a device, no countdown. Repeat with a swipe right instead of Back: same. Then measure,
  for `BLUETOOTH_PROTOCOL.md`: the time from Back to the first `bt work status` without `0x08`
  (if any within 5 min), and whether a never-paired phone still lists "Insta360 Wave BT <hex>"
  1 min and 3 min after Back.
- **S3. Resting keeps pairing (R2).** Pair a device, then leave the screen untouched past the
  dim time. Wake it: the countdown is still running, and no `BtStopPairing` is in the log.
- **S4. Pair another phone (R3).** With phone A connected and playing music, Bluetooth shows
  A "Connected", Disconnect and Pair another phone. Tap Pair another phone: log
  `bt: BtPair`; A stops playing and drops; the countdown starts from 2:00 with the name large.
  A must not come back during the countdown (`bt work status` never shows `0x01`/`0x03` until
  B connects). Pair phone B: the screen shows B "Connected" and the countdown stops.
- **S5. Disconnect sticks (R4).** Phone connected and playing: tap Disconnect. Log
  `bt: BtDisconnect`; "No phone connected" within 5 s; for the next 2 min every
  `bt work status` has bit 0 clear (`0x00`, `0x08`, `0x10`, never `0x01`/`0x03`), the screen
  stays "No phone connected", the phone shows Wave not connected. Then connect from the
  phone's Bluetooth settings: it connects and the screen shows it. If the phone came back by
  itself, note after how long and whether the debug lines around it include
  `bt module: RECONN_START!` or `bt module: app_bt_start_connback_func` (the module
  reconnected) or not (the phone did; then run S6). `bt module: bt stop connback` right after
  `bt: BtDisconnect` would confirm what `0xD8 00` does.
- **S6. Does `CloseBt` drop the phone? (research for a Bluetooth switch).** waved running,
  phone connected and playing. From `adb shell`: `printf '\165\105\001\313\314' > /dev/ttyS1`
  (`75 45 01 CB CC`). Watch for 60 s: when the phone shows Wave disconnected (if ever), and
  whether any `bt work status` loses bit 0. Then `printf '\165\105\002\314\000\316' > /dev/ttyS1`
  (`75 45 02 CC 00 CE`) and note whether the phone reconnects. Record both in
  `BLUETOOTH_PROTOCOL.md` ("Open").
- **S7. Is the idle module discoverable after `OpenBt`?** No phone connected, not pairing,
  after a fresh boot (or after S6): `printf '\165\105\002\314\000\316' > /dev/ttyS1`; on a
  never-paired phone, look for "Insta360 Wave BT <hex>" for 30 s. If it shows, waved should send
  `OpenBt` at start as stock does (follow-up); record it.
- **S8. Pairing from idle still works with `D8 00` first (R4).** Covered by item 3 below: the
  phone must find the name and pair.
- Do **not** send `75 45 02 D8 02 DC` (stock's factory reset value) unless wiping the module's
  pairings is the goal.

### First round

1. **Start-up.** `grep "bt module" /tmp/waved.log`: firmware, build time and name lines appear,
   each once at info.
   Menu → Bluetooth (PIN first if one is set) shows "Shows up as Insta360 Wave BT <6 hex>".
2. **Privacy in the log.** Connect a phone with contacts and place a call. Then
   `grep -i -E "<phone's Bluetooth name>|<phone MAC, any case, with and without colons>|<caller number>" /tmp/waved.log`
   finds nothing, at trace too (`RUST_LOG=waved_bt=trace,waved_uart=trace`): module log lines
   show only as `bt module: <fixed message>` or `bt module log line, N bytes`, undecoded
   frames as `bt frame 0x.., N bytes`. Grep for a contact's name and number as well.
3. **Pair.** With no phone connected tap Pair a device. Expect the countdown from 2:00, the
   module name large, and in the log `bt work status 0x08`. On a phone that never paired, the
   name appears in its Bluetooth list; pair. Expect the screen to switch to the phone's name
   with "Connected" and the countdown to stop.
4. **How pairing ends.** Tap Pair a device and pair nothing, staying on the screen. Note when
   `0xB2` bit 3 clears (`bt work status` without 0x08), if ever; whether the phone still lists
   Wave after the countdown ends at 0:00; record both in `BLUETOOTH_PROTOCOL.md`.
5. **Disconnect.** See S5.
6. **Reconnect.** Toggle Bluetooth off and on on the phone. Expect it to reconnect and the
   screen to show it.
7. **PhoneMac.** After a connect, debug shows `bt: phone MAC received` (no MAC); disconnecting
   the phone from its own settings flips the screen to "No phone connected" (`0xE1` matched the
   MAC).
8. **Incoming call, answer.** Call the paired phone. Expect the Wave screen to wake and show
   "Incoming call", the number (never a contact name) and "via <phone>". Tap Answer: the phone
   answers (event 3), the screen shows "In a call" with Hang up, Home (Back) says "In a call".
   Tap Hang up: the call ends (event 0) and the screen returns to where it was.
9. **Incoming call, decline.** Call again, tap Decline: the call is rejected on the phone and the
   screen leaves.
10. **Answer on the phone.** Call, answer on the phone: the Wave screen switches to "In a call"
    (`0xD1 00`); hang up on the phone: it leaves (`0xD1 01` or the empty `0xB1`). Log the frame
    order (`bt: phone event`, `bt: call ...`) into the doc.
11. **Dismiss.** Call, swipe right on the call screen: it goes away and does not come back for
    that call; the next call shows it again.
12. **During a recording.** Start a recording, call the phone: the call screen covers Recording,
    the recording timer keeps running (Back shows it), Decline returns to Recording, and the
    saved recording has no gap.
13. **Resting screen.** Let the screen rest, call: it wakes onto the call screen.
14. **Outgoing call.** Dial out from the phone: note whether `0xB1` arrives (the Wave would show
    "Incoming call" until the call is up) and record it.
15. **In-band ring.** During a ring note the `0xB2` bits (bit 2 while ringing?).
16. **PIN.** With a PIN set, Menu → Bluetooth asks for it; the call screen never does.
