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
3. **Disconnect = `Disconnect` only.** Stock's UI disconnect also enters pairing; ours does not,
   since the user asked to drop the phone, not to pair another.
   *Cost if wrong:* the phone may reconnect at once; the device pass checks.
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
`/tmp/waved.log` running with `RUST_LOG=waved_bt=debug`.

1. **Start-up.** `grep "bt module" /tmp/waved.log`: firmware, build time and name lines appear.
   Menu → Bluetooth (PIN first if one is set) shows "Shows up as Insta360 Wave BT <6 hex>".
2. **Privacy in the log.** Connect a phone with contacts and place a call. Then
   `grep -i -E "<phone's Bluetooth name>|<phone MAC, any case, with and without colons>|<caller number>" /tmp/waved.log`
   finds nothing at debug. With `RUST_LOG=waved_bt=trace,waved_uart=trace` it may show module
   log lines (`bt module log:`) but never a contact: grep for a contact's name and number.
3. **Pair.** With no phone connected tap Pair a device. Expect the countdown from 2:00, the
   module name large, and in the log `bt work status 0x08`. On a phone that never paired, the
   name appears in its Bluetooth list; pair. Expect the screen to switch to the phone's name
   with "Connected" and the countdown to stop.
4. **How pairing ends.** Tap Pair a device and pair nothing. Note when `0xB2` bit 3 clears
   (`bt work status` without 0x08), if ever; whether the phone still lists Wave after the
   countdown ends at 0:00; record both in `BLUETOOTH_PROTOCOL.md`.
5. **Disconnect.** Connected, tap Disconnect. Expect "No phone connected" within 5 s and the
   phone to show Wave as not connected. Note whether the phone reconnects by itself, and after
   how long.
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
