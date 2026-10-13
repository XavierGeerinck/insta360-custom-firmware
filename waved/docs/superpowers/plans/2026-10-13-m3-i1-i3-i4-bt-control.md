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
    mockup's still); naming the module from `device.name` (I1; `SetName`'s accepted lengths
    are unverified and the module already names itself `Insta360 Wave BT <serial>`); call
    waiting and outgoing calls (see the checklist).

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
  drops the phone; a confirm step (`ConfirmKind`) can be added.
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
