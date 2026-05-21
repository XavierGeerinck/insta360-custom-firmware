# SP#7(d') — HID Consumer / Telephony control interface

Date: 2026-05-21 (v2: revised 2026-05-21 after `waved-hid` discovery)
Status: spec
Parent roadmap: `waved/TODO.md` — SP#7 (Wave Controller native compatibility), item (d) reframed.

## 1. Context

The original SP#7 item (d) proposed a kernel patch to `f_uac2.c` to advertise a Master Volume control on the playback Feature Unit, with the goal of making macOS dispatch UAC2 SET_CUR(VOLUME) on F11 / F12 / Output slider. That patch is days–weeks of work (cross-compile, vendor kernel tree, CRC-matched module deploy, no `/lib/modules/$(uname -r)/build` on the device) for a single host-side ergonomic.

Static analysis of `fw_v1.1.6.6/rootfs/etc/init.d/S89usbgadget` shows that **stock firmware never solved the Mac F11 / F12 problem either.** It uses the opposite direction: a `functions/hid.0` interface advertising a combined **Vendor + Telephony_V4 + Consumer** report descriptor. The Consumer page (Report 0x01) lets the device emit `VolumeUp / VolumeDown / Mute / Play-Pause / Next / Prev` events to the host. The host then attenuates its own output (Mac slider tracks); F11 / F12 still don’t reach the device, but the local Wave touch UI does — symmetrical to stock behaviour, and significantly cheaper to ship than a kernel patch.

SP#7(d') replaces (d) with the stock-equivalent design. The kernel patch stays available as a future option but is no longer a roadmap blocker.

Vendor channel (Report 0x03) is **out of scope** for (d'); it is the deliverable of SP#7(a) (Wave Controller protocol probe).

### 1.1 Pre-existing state discovered after v1 spec (revision note)

`waved/crates/waved-hid/` already exists and is wired live at `crates/waved/src/main.rs:453`. Today its `HidService` consumes `DeviceEvent::ButtonPress(ButtonType)` from `waved-mcu` (physical hardware buttons on the device) and writes 2-byte HID reports to `/dev/hidg0`. Three correctness gaps in the current implementation:

1. **Report IDs are swapped vs stock.** Current code declares `REPORT_ID_TELEPHONE = 0x01, REPORT_ID_CONSUMER = 0x02`. The stock descriptor (and the one Task 11 installs) is the opposite: `0x01 = Consumer, 0x02 = Telephony`. With the stock descriptor present, an MCU `VolumeUp` press today writes `[0x02, 0x01]`, which the host reads as Telephony `HookSwitch` (answer call) — not a volume key. SP#7(d') fixes this.
2. **No host → device path.** `waved-hid` has no reader; LED Set_Report writes from the host (Off-hook / Mute / Ring) are silently discarded and never reach `AudioState`.
3. **No state-driven event source.** Only MCU physical button presses produce HID reports. Wave touch UI volume / mute changes (which mutate `speaker_volume` / `speaker_muted` in `AudioState`) do not currently emit anything to the host.

`waved-mcu` decodes button codes `0x40/0x41/...` into `ButtonType::{MuteToggle, MuteLongPress, VolumeUp, VolumeDown}` (`crates/waved-mcu/src/buttons.rs:8`). The `ButtonType` enum has nothing for Play/Pause/Next/Prev or telephony hook-switch (matches the spec's goal scope — we keep MCU-driven inputs minimal and add state-driven sources for richer keys).

SP#7(d') therefore **extends `waved-hid` in place** rather than introducing a parallel module. The new files all land under `crates/waved-hid/src/`.

## 2. Goals

| # | Goal |
|---|---|
| G1 | Wave touch UI volume tick → host system volume changes by one step (Mac / Win / Linux). |
| G2 | Wave touch UI mute toggle → host mute key fires (Mac mute icon flips on / off). Existing Mac F10 → device-speaker mute path (capture-FU redirect) keeps working. |
| G3 | Telephony app (Zoom / Teams) call state visible in `AudioState` — `call_off_hook`, `call_ringing`, `call_host_muted` reflect host LED writes. |
| G4 | During a telephony call, Wave touch mute also emits Phone Mute so the telephony app sees mic-mute. |
| G5 | No regression in existing SP#6 UAC2 routing (host audio still flows through DSP, AEC ref still post-DSP, F10 capture-FU mute still redirects to speaker). |
| G6 | When `/dev/hidg0` is absent or the host is detached, waved degrades cleanly to no-op — no crashes, no busy-loops. |

## 3. Non-goals

- **Mac F11 / F12 → device volume.** Out of reach without the kernel patch; stock fw never solved this. Hammerspoon shim (`scripts/wave-volume-mac.lua`) remains the per-Mac workaround for users who want F11 / F12 specifically.
- **Hook switch IN (answer / hang up from Wave touch).** No touch UI affordance designed yet; deferred to a small future UX SP.
- **Vendor channel (Report 0x03).** Owned by SP#7(a).
- **OLED indicator rendering for `call_off_hook` / `call_ringing`.** Plumbing only in (d'); rendering is a separate UI SP.
- **HID descriptor generation in code.** We copy the stock descriptor verbatim from `fw_v1.1.6.6/rootfs/etc/init.d/S89usbgadget:199` — no in-code descriptor builder.
- **Multichannel UAC2 upstream** (SP#7(b)), **upstream EQ** (SP#7(c)), **f_uac2 Master Volume kernel patch** (original SP#7(d)) — all stay deferred.

## 4. Architecture

```
crates/waved-hid/src/                       (existing crate; extended)
├── lib.rs           — HidService entry (existing); refactored so all
│                      sources push HidEvent into a single internal
│                      mpsc, drained by one writer task. Existing
│                      DeviceEvent::ButtonPress(ButtonType) bridge stays
│                      intact and now feeds the same mpsc.
├── events.rs        — NEW: HidEvent enum (single output type).
├── report.rs        — NEW: pure event_to_press_release + Report-ID/bit
│                      constants matching the STOCK descriptor
│                      (0x01 = Consumer, 0x02 = Telephony).
├── reader.rs        — NEW: blocking thread owning /dev/hidg0 read side;
│                      parses Reports 0x05 / 0x06 / 0x07 → AudioState.call_*.
├── watch.rs         — NEW: tokio tasks subscribed to audio_tx —
│                      volume_watch (speaker_volume delta → VolumeStep)
│                      and mute_watch (speaker_muted edge → ConsumerMute
│                      [+ TelephonyPhoneMute when call_off_hook=true]).
├── stats.rs         — NEW: HidStats atomics + HidStatsSnapshot.
└── simulate.rs      — existing; updated for the new event enum +
                       report IDs so log lines match real wire format.

crates/waved-audio/src/stats_collector.rs   (existing; extended)
   adds `hid: Option<waved_hid::HidStatsSnapshot>` on AudioStats; threads
   `hid_stats: Option<Arc<waved_hid::HidStats>>` through `run()` and
   `compute_stats_with_processed()`.

crates/waved-audio/src/audio_config.rs      (existing; extended)
   adds `HidConfig` block + `pub hid: Option<HidConfig>` on AudioConfig.
   Lives here (not in waved-hid) to follow the UacConfig precedent —
   audio_config.rs is the single config aggregation point.

crates/waved/src/main.rs                    (existing; extended)
   The existing HidService spawn (line 453) becomes a richer
   HidService::spawn taking HidConfig, the audio_tx (Arc<watch::Sender<
   AudioState>>) for the watchers + reader, and an Arc<HidStats>. The
   MCU-driven path (DeviceEvent::ButtonPress) is preserved inside
   HidService.

scripts/patch-stock-usbgadget.sh            — rename + extend of
   patch-uac2-volume-range.sh; adds functions/hid.0 setup with the
   stock-extracted Vendor + Telephony_V4 + Consumer report descriptor
   and links it into amlogic.1.
```

Modules `bridge.rs`, `upstream.rs`, `control.rs` under `crates/waved-audio/src/uac/` (SP#6) provide the precedent for module-internal layout, error handling, and stats integration patterns.

### 4.1 Single ingress (HidEvent), multiple sources

`HidEvent` is the unified output type. Three sources push to a single `mpsc::Sender<HidEvent>` inside `HidService`:

1. **MCU-button bridge** — existing path. `HidService` receives `DeviceEvent::ButtonPress(ButtonType)` from `bus.events` and maps each to one `HidEvent` (e.g. `ButtonType::VolumeUp → HidEvent::ConsumerVolumeStep { up: true }`). Today this code constructs the wire bytes inline; the v2 refactor pushes a `HidEvent` to the channel instead, and the writer drains and serializes.
2. **`volume_watch`** task — subscribes to `audio_tx`, computes `speaker_volume` deltas, pushes `ConsumerVolumeStep` per integer step (rate-capped at `volume_step_rate_hz`).
3. **`mute_watch`** task — subscribes to `audio_tx`, on `speaker_muted` edge pushes `ConsumerMute`, and additionally `TelephonyPhoneMute` if `call_off_hook == true` at the moment of the edge.

Single writer task drains the mpsc and writes 2-byte press-release pairs to `/dev/hidg0`. `HidReader` runs in a blocking `std::thread` (the gadget read syscall does not yield until the host writes a Set_Report).

### 4.2 Behavioural change vs current `waved-hid` (non-backwards-compat fix)

This is a one-time correction, not a feature regression:

- **Report IDs**: current `REPORT_ID_TELEPHONE = 0x01, REPORT_ID_CONSUMER = 0x02` → **new** `0x01 = Consumer, 0x02 = Telephony`. After this change, MCU `VolumeUp` / `VolumeDown` produce a real Consumer-page volume key event on the host (was previously misread as Telephony HookSwitch / PhoneMute depending on the descriptor in use).
- **Bit positions**: current `VolumeUp = bit0 of byte`, `VolumeDown = bit1 of byte` on the (now-renumbered) Consumer report → **kept**, since stock descriptor's Consumer page also has `bit0 = VolumeUp, bit1 = VolumeDown`. Stock additionally defines `bit2 = Mute, bit3 = PlayPause, bit4 = Next, bit5 = Prev`.
- **Telephony page**: `ButtonType::MuteToggle / MuteLongPress` currently emit a Telephony Report at byte=`0x01` (bit0 — which on stock's Telephony_V4 is `HookSwitch`, not mute). **After**: `MuteToggle` produces `HidEvent::ConsumerMute` (the right semantic on a music-listener device). `MuteLongPress` reserved for future PhoneMute-during-call usage (currently mapped to `ConsumerMute` for parity until UX decides).

The existing unit tests at `crates/waved-hid/src/lib.rs:86-110` will be updated to assert against the corrected bytes.

## 5. Components

### 5.1 `HidEvent` (mpsc payload, writer-bound)

```rust
pub enum HidEvent {
    // Report 0x01 — Consumer page (writer emits press then release)
    ConsumerVolumeStep { up: bool },
    ConsumerMute,
    ConsumerPlayPause,
    ConsumerNext,
    ConsumerPrev,

    // Report 0x02 — Telephony page
    TelephonyPhoneMute,
}
```

`ConsumerPlay`, `ConsumerPause` (separate from Play/Pause toggle) are present in the descriptor but not emitted in v1 — no UI source for them yet.

### 5.2 `HidConfig` (new block in `AudioConfig`)

```rust
pub struct HidConfig {
    pub enabled: bool,                 // default true
    pub device: String,                // default "/dev/hidg0"
    pub press_release_gap_ms: u64,     // default 10
    pub volume_step_rate_hz: u32,      // default 40 — throttle volume bursts
}
```

Defaults pattern matches `UacConfig` — `#[serde(default = "default_hid_enabled")]` etc.

### 5.3 `AudioState` additions

`AudioState` (in `crates/waved-state/src/lib.rs`) is a plain-field struct held by a single `Arc<watch::Sender<AudioState>>`; all mutators already go through `send_modify` so subscribers see every change. (d') adds three plain `bool` fields written by `HidReader`:

```rust
pub call_off_hook:    bool,   // host wrote Report 0x05 bit0 = 1
pub call_ringing:     bool,   // host wrote Report 0x07 bit0 = 1
pub call_host_muted:  bool,   // host wrote Report 0x06 bit0 = 1
```

`call_host_muted` is informational (Mac / Zoom telling us "the call is muted host-side"). We do not act on it in (d') beyond stats.

### 5.4 Watch glue (volume & mute)

`AudioState` is already wrapped in `Arc<watch::Sender<AudioState>>`, with `speaker_volume: u16` and `speaker_muted: bool` mutated via `audio_tx.send_modify(|s| s.<field> = ...)`. No new senders are introduced.

The glue tasks each take a `watch::Receiver<AudioState>` (from `audio_tx.subscribe()`):

- **`volume_watch`** — caches `last_volume: u16` (initialised from the borrowed snapshot). On `changed().await`, reads `current.speaker_volume`, computes `delta = current - last` (as `i32` to avoid underflow), emits `|delta|` × `HidEvent::ConsumerVolumeStep { up: delta > 0 }`, capped to `volume_step_rate_hz` events / s (excess queued in the mpsc, drained at the cap rate so a 0→100 drag is paced but not lost).
- **`mute_watch`** — caches `last_muted: bool`. On `changed().await`, if `current.speaker_muted != last_muted`, emits `HidEvent::ConsumerMute`; if `current.call_off_hook == true`, also emits `HidEvent::TelephonyPhoneMute`.

Both tasks ignore changes that don't touch their field — a `mic_volume` mutation, for instance, fires `changed()` but produces no HID event.

## 6. Wire format & data flow

### 6.1 `/dev/hidg0` byte layout

| Direction | Report | Bytes on wire | Bit map |
|---|---|---|---|
| device → host (write) | `0x01` Consumer | `[0x01, B]` | bit0 VolumeUp, bit1 VolumeDown, bit2 Mute, bit3 PlayPause, bit4 Next, bit5 Prev, bit6 Play, bit7 Pause |
| device → host (write) | `0x02` Telephony | `[0x02, B]` | bit0 HookSwitch, bit1 PhoneMute, bit2 Flash, bit3 Redial, bit4 Button1, bits 5-7 padding |
| host → device (read)  | `0x05` Off-hook LED | `[0x05, B]` | bit0 Off-hook |
| host → device (read)  | `0x06` Mute LED | `[0x06, B]` | bit0 Muted |
| host → device (read)  | `0x07` Ring LED | `[0x07, B]` | bit0 Ringing |

Press-release pair, e.g. one VolumeUp tick:
1. `write(/dev/hidg0, [0x01, 0x01])`
2. `tokio::time::sleep(press_release_gap_ms)`
3. `write(/dev/hidg0, [0x01, 0x00])`

macOS counts the rising edge as one volume tick. Linux PulseAudio / PipeWire and Windows behave equivalently.

### 6.2 Data flow (writer path)

```
Touch UI volume drag ─┐
mode_switch SIGHUP ──┼──▶ state.speaker_volume.store(N)
UacControl host vol ─┘                     │
                                           │  watch::Sender.send_replace(N)
                                           ▼
                                ┌────────────────────────┐
                                │ volume_watch task      │
                                │   on watch changed():  │
                                │     delta = N - last   │
                                │     emit |Δ| × HidEvent│
                                └─────────┬──────────────┘
                                          │ mpsc<HidEvent> (cap 64)
                                          ▼
                                ┌────────────────────────┐
                                │ HidWriter task         │
                                │   serialize to bytes   │
                                │   press → gap → release│
                                └─────────┬──────────────┘
                                          ▼
                                     /dev/hidg0
```

### 6.3 Data flow (reader path)

```
                                     /dev/hidg0
                                          │ host Set_Report
                                          ▼
                                ┌────────────────────────┐
                                │ HidReader task         │
                                │   read N bytes         │
                                │   match report_id      │
                                └─────────┬──────────────┘
                                          ▼
                          audio_tx.send_modify(|s| s.call_off_hook   = bit0)
                          audio_tx.send_modify(|s| s.call_host_muted = bit0)
                          audio_tx.send_modify(|s| s.call_ringing    = bit0)
```

### 6.4 Throttling & debouncing

- **Volume burst cap**: `volume_step_rate_hz` (default 40 Hz). A 0 → 100 slider drag yields 100 events; the writer drains them at 40 / s so the host receives a smooth ramp. Excess sits in the mpsc queue.
- **Mute edge filter**: `mute_watch` records last-known value and only emits on transitions (false ↔ true). Repeated `store(true)` calls do not re-emit.

### 6.5 Stats (additions to `/tmp/audio_stats.json`)

```json
"hid": {
  "enabled": true,
  "device_present": true,
  "events_emitted":   { "volume_step": 142, "mute": 3, "phone_mute": 1, "play_pause": 0, "next": 0, "prev": 0 },
  "leds_received":    { "off_hook": 5, "ringing": 0, "mute": 2 },
  "call_off_hook":    false,
  "call_ringing":     false,
  "call_host_muted":  false,
  "write_errors":     0,
  "glue_drops":       0
}
```

## 7. Error handling

| Condition | Response |
|---|---|
| `/dev/hidg0` missing at startup (configfs not patched) | `HidWriter::spawn` logs `WARN hid: /dev/hidg0 not present, HID control disabled` and returns. Stats emit `hid.device_present = false`. waved keeps running. |
| `write()` returns `EAGAIN` | Yield briefly + retry once. Persistent `EAGAIN` → drop the event, increment `hid.write_errors`. |
| `write()` returns `ESHUTDOWN` / other (host detached mid-stream) | Log once at WARN, set `device_present = false`, suspend writes until reader observes a successful read (host reattach). |
| `read()` returns `EOF` / `ENODEV` | Reader exits cleanly. Supervisor respawns once after 1 s. Second immediate death → disable HID for the rest of the session. |
| `HidConfig.enabled = false` | Tasks not spawned; stats omits the `hid` block. |
| mpsc full | `try_send` drops with WARN log, increments `hid.glue_drops`. Channel sized 64 — far above the 40 Hz cap. |

## 8. configfs / init wiring

`scripts/patch-stock-usbgadget.sh` is a rename + extension of the existing `scripts/patch-uac2-volume-range.sh`. After the existing `p_volume_min` / `p_volume_res` edits, it inserts in `uac_init()` (before `usb_flash`):

```sh
# HID control surface (Vendor + Telephony_V4 + Consumer)
mkdir -p functions/hid.0
echo 1  > functions/hid.0/protocol
echo 1  > functions/hid.0/subclass
echo 64 > functions/hid.0/report_length
echo -ne "\x06\x00\xff\x09\x01\xa1\x01\x85\x03\x09\x01\x15\x00\x26\xff\x00\x95\x3f\x75\x08\x81\x02\x09\x01\x15\x00\x26\xff\x00\x95\x3f\x75\x08\x91\x22\xc0\x05\x0b\x09\x05\xa1\x01\x85\x02\x15\x00\x25\x01\x75\x01\x95\x01\x09\x20\x81\x22\x09\x2f\x81\x06\x09\x21\x81\x02\x09\x24\x81\x06\x09\x07\x05\x09\x09\x01\x81\x06\x75\x03\x81\x01\x85\x05\x05\x08\x09\x17\x75\x01\x91\x02\x75\x07\x91\x03\x85\x06\x09\x09\x75\x01\x91\x02\x75\x07\x91\x03\x85\x07\x09\x18\x75\x01\x91\x02\x75\x07\x91\x03\xc0\x05\x0c\x09\x01\xa1\x01\x85\x01\x05\x0c\x15\x00\x25\x01\x09\xe9\x09\xea\x09\xe2\x09\xcd\x09\xb5\x09\xb6\x09\xb0\x09\xb1\x75\x01\x95\x08\x81\x02\xc0" > functions/hid.0/report_desc
ln -sf functions/hid.0 configs/amlogic.1/hid.0
```

These bytes are the **active** stock report descriptor copied verbatim from `fw_v1.1.6.6/rootfs/etc/init.d/S89usbgadget:199` (the "Vendor+Telephone_V4+Consumer" line; the only echo that is uncommented in stock fw v1.1.6.6).

Idempotent: re-running checks for `functions/hid.0` existence and bails.

After patch + reboot: `ls /dev/hidg0` returns a character device. Stock and our firmware both come up with this device available.

## 9. Acceptance plan

On-device script `scripts/test-hid-control.sh` automates as many steps as possible; the call-state tests require a Zoom or Teams account and are documented manual steps.

| # | Test | Pass criteria |
|---|---|---|
| 1 | configfs setup | `/dev/hidg0` exists after `/etc/init.d/S89usbgadget restart` |
| 2 | Wave touch volume tick → Mac | `osascript -e 'output volume of (get volume settings)'` value changes by ≥ 1 step per touch tick |
| 3 | Wave touch mute → Mac mute icon flips on | macOS Sound icon shows mute glyph; `osascript` reports `output muted: true` |
| 4 | Mac F10 → Wave speaker mute (regression on SP#6) | `state.speaker_muted = true` within 200 ms of F10 |
| 5 | Zoom call connects on Mac → Off-hook LED | `state.call_off_hook = true` within 1 s of connect (verified via `/tmp/audio_stats.json`) |
| 6 | Wave touch mute during Zoom call → Zoom mic mute | Zoom mic-mute UI within 500 ms |
| 7 | Linux host volume tracking | `pactl get-sink-volume @DEFAULT_SINK@` tracks Wave touch ticks (PulseAudio or PipeWire) |
| 8 | Hostless boot | No /dev/hidg0, no host: waved logs WARN once, no crashes, no busy-loop, normal heartbeat |
| 9 | MCU button regression (post-swap) | Press physical Volume Up button on Wave → Mac volume rises by 1 step. Validates that the Report-ID fix in §4.2 produces correct host-side behaviour for the existing MCU-driven path. |
| 10 | MCU MuteToggle button (post-swap) | Press physical mute button on Wave → Mac mute icon flips on; was previously sending HookSwitch and would have hung up calls. |

## 10. Open questions to settle during implementation

- **Does macOS treat "Phone Mute" (Report 2 bit1) differently from "Consumer Mute" (Report 1 bit2)?** First device-side test will reveal whether emitting both simultaneously double-toggles host mute. If yes, suppress Consumer Mute while `call_off_hook=true`.
- **Telephony Hook Switch bit semantics.** Stock descriptor declares it Input/Var/Rel (relative key event). Empirically with the host stack, this should behave as press/release like Phone Mute, but we don't emit Hook Switch in v1 — flagged for SP#7 follow-up.

## 11. Out-of-scope items recorded against future SPs

- **Vendor protocol (Report 0x03)** → SP#7(a). Static analysis of `decompiled/libInstaAudio.so.c` + `fw_v1.1.6.6/rootfs/usr/bin/*` to map the 63-byte command grammar that the Wave Controller app uses.
- **Hook Switch IN (Report 0x02 bit0)** → small UX SP. Needs a touch UI affordance (long-press mute? swipe? dedicated icon?) to trigger answer / hang up.
- **OLED rendering of `call_off_hook` / `call_ringing`** → small UI SP. Probably a phone icon top-center on the home screen.
- **Mac F11 / F12 → Wave volume** → original SP#7(d) kernel patch. Deferred indefinitely; Hammerspoon shim remains the documented per-Mac workaround.
