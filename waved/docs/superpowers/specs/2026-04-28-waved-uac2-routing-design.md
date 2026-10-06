# Waved Audio — SP#6 UAC2 Routing for Wave Controller Design

**Date:** 2026-04-28
**Depends on:** SP#1 capture foundation v2 + SP#2 AEC + SP#3 beamforming + SP#4 neural NS + SP#5 playback EQ + gain (2026-04-28)
**Status:** spec — implementation plan to be created via `writing-plans`

---

## 1. Context

The Wave already enumerates on the host as a USB Audio Class 2 device. `/etc/init.d/S89usbgadget` configures `f_uac2` with both directions active:

```
c_chmask=0x3 c_srate=48000 c_ssize=2    # capture-from-host (host writes audio)
p_chmask=0x3 p_srate=48000 p_ssize=2    # playback-to-host  (host reads audio)
```

On a working device the host sees a stereo S16 48 kHz speakerphone — selectable in macOS Sound preferences, picked up by Zoom / Teams / Wave Controller as both microphone *and* speaker. Validated 2026-04-28 on PSP_0000000000000000: kernel reports `Audio Streaming OUT Interface (Speaker) assigned to Interface 1`, ALSA card 1 is `UAC2Gadget` with PCM `01-00 playback 1 capture 1`, gadget tree linked into `configs/amlogic.1`.

**The current waved firmware does not bridge UAC2 to the speaker or pull mic audio upstream.** When the host plays audio:

```
host writes UAC2-out endpoint → kernel f_uac2 → ALSA hw:1,0 capture
                                                       │
                                                       ▼
                                             [no userspace reader] ← SP#6 fills this
```

`/proc/asound/card1/pcm0c/sub0/status` shows `closed`. The host's audio reaches the kernel's gadget endpoint and stops there. Stock firmware bridged this with `libInstaAudio.so::Insta360_AudioPath::pushDownLinkFrame` → `Insta360_DownlinkAsrPlayAudioAglo` → `Insta360_Speaker` → `hw:0,1`. Closed source.

Symmetrically, on the upstream side, `/proc/asound/card1/pcm0p/sub0/status` shows `closed`. Nothing in waved feeds audio into the UAC2-in endpoint, so the host sees a microphone that always reports zero samples — which is why Mac call apps with the Wave selected currently send silence.

SP#6 makes the device a real two-way speakerphone. Host audio plays through the SP#5 pipeline (with AEC capture of host-played material as the far-end reference, satisfying "they don't hear themselves echo"). Mic audio reaches the host via the existing post-DSP `ProcessedAudioFrame` stream — beamformed, AEC-cleaned, NS-cleaned mono, duplicated to stereo on the wire.

## 2. Decisions (locked in brainstorming)

1. **Bidirectional. Both directions ship in this SP.** Host → speaker is the immediate user-visible bug; mic → host is what makes the device a speakerphone instead of a fancy speaker. Both endpoints already exist in the kernel; userspace just has to read/write them. No reason to ship half.

2. **Format passthrough — zero conversion.** Kernel UAC2 endpoint, speaker `hw:0,1`, and `ProcessedAudioFrame` are all 48 kHz S16-or-trivially-convertible-to-S16 stereo (or upmixable mono). No resampler, no sample-rate converter, no format conversion. Period sizes match (1024 frames default).

3. **Architecture: one std::thread per direction.** Mirrors the existing `audio-capture` and `audio-playback` thread pattern. SCHED_FIFO priority 18 (one less than playback's 20, so speaker-clock wins on contention). Tokio is not in the path — ALSA's `readi`/`writei` blocks; doing this in tokio without careful blocking-thread shimming is fragile, and we already have a working blocking-ALSA pattern.

4. **Host audio routes through SP#5 `play_batch`. AEC reference comes for free.** Once UAC2-downstream calls `playback.play_batch(samples)`, the rest of the pipeline is unchanged: SP#5 DSP applies, the post-DSP broadcast tap publishes to AEC, the existing `MicWithSoftwareReferenceSource` already subscribes. No AEC changes in this SP.

5. **Mic upstream uses `ProcessedAudioFrame.mic_mono_f32` (post-AEC, post-beamform, post-NS).** This is the cleanest signal we produce. Raw 8-channel upstream (option C in brainstorming) is deferred to a future SP if a host-side app needs it; would require re-configfs of `c_chmask=0xff` and a separate UAC2 device exposed via a vendor-specific interface descriptor. Not worth the kernel-side work for a hypothetical use case.

6. **Mono → stereo duplication on upstream.** `mic_mono_f32` is one channel; UAC2-in is configured for two. Duplicate L = R = mono. The Mac mixer will collapse to mono if the receiving app prefers mono. No spatial information to preserve (post-beamform output is single-direction anyway).

7. **Volume policy: host UAC2 volume drives device.** Mac volume slider → kernel `f_uac2` SET_CUR → ALSA mixer event on card 1 → `uac/control.rs` polling task picks it up → dispatches `Command::SetSpeakerVolume(v)`. On-device touch volume changes write back to the same kernel control via `f_uac2`'s feedback path, so the Mac slider mirrors. Mute is symmetric: muting either side mutes both.

8. **Drift handling: drop/duplicate at period boundaries.** Host and device crystals drift ~50 ppm = ~2.4 samples/sec at 48 kHz. Over a 1-hour call, ~180 ms accumulated drift if uncorrected. Mitigation: track `play_batch` queue depth in the downstream bridge; if queue grows beyond 4 periods, drop the oldest period (audible as a 21 ms gap, but in practice happens once every several minutes — barely perceptible during music, inaudible during speech). If queue would underflow, the existing `run_playback_loop` silence-fill covers it (no audible artefact: silence in a gap of host audio is what should happen anyway). No ASRC. Same scheme on upstream, mirrored: if host can't drain fast enough, drop the oldest period from the upstream FIFO.

9. **Host endpoint not selected? Sleep 50 ms and retry.** When `hw:1,0c` is "closed" (host hasn't picked the device), `readi` returns -EAGAIN. Bridge sleeps 50 ms and retries. Same on upstream when host has the endpoint closed (returns -EBADFD on `writei`). No CPU wasted polling, sub-second response when the host attaches.

10. **Acceptance criteria (`scripts/test-uac2-routing.sh`):**
    - **Host → speaker round-trip.** Connect to a Mac, play `afplay 1k_tone.wav`. Device speaker emits audible 1 kHz within ≤ 100 ms of `afplay` start. Measured via mic loopback (mic captures the speaker output, latency = mic-frame timestamp − afplay start time).
    - **Mic → host round-trip.** `arecord -D 'hw:UAC2Gadget,0' -f S16_LE -r 48000 -c 2` on the Mac while speaking into the Wave. Recording matches the on-device `ProcessedAudioFrame.mic_mono_f32` to within DSP latency, RMS within ±1 dB, no glitches over 30 s.
    - **AEC of host audio (the "they don't hear themselves" gate).** Mac plays far-end speech (the remote party in a real call), local user speaks simultaneously. The recording captured on the Mac (= what the remote would hear) has ERLE ≥ 20 dB on the far-end content. Same bar as SP#2.
    - **Volume mirror.** Mac volume slider movement → device `speaker_volume` state changes (verify via `cat /tmp/audio_stats.json`). On-device touch volume change → Mac volume slider updates within 100 ms (verify via `osascript -e 'output volume of (get volume settings)'`).
    - **Connect/disconnect/reconnect.** Mac unplugs and replugs the Wave. Both bridges resume cleanly (no zombie threads, no stuck ALSA opens). Verified via `ps -T` showing single instances of `uac-downstream` and `uac-upstream` after reconnect.
    - **Sustained call test.** 30-minute Mac call (loopback to a second machine). No audible artefacts; UAC2 stats show drift compensation engaging at most twice per 10 minutes.

## 3. Scope

**In scope:**
- New module `crates/waved-audio/src/uac/` with `mod.rs`, `bridge.rs` (downstream), `upstream.rs` (mic → host), `control.rs` (volume + mute mirror).
- `crates/waved/src/main.rs`: instantiate `UacBridge` and `UacUpstreamSink` inside the `capture-alsa` block, after `spawn_playback`. Spawn one std::thread each. Subscribe `UacUpstreamSink` to `audio_svc.subscribe_processed()`.
- `UacBridge` opens `hw:1,0` capture, reads in 1024-frame periods, calls `playback.play_batch(samples)`.
- `UacUpstreamSink` opens `hw:1,0` playback, drains `ProcessedAudioFrame` from the broadcast subscriber, mono → stereo duplicate, writes to ALSA. Mic mute is already applied at the source (`apply_mic_mute` in the capture loop) — upstream sees zeros, host gets silence.
- `UacControl` polls `card 1` mixer events for `c_volume_*` (host-set capture volume) and `p_volume_*` (host-set playback volume). Translates to `Command::SetSpeakerVolume(v)` and `Command::SetMicVolume(v)`. Reverse direction: when those state values change (touch UI, /tmp file), `UacControl` writes the new value back via `amixer -c 1 sset`. Polling cadence: 50 ms.
- Optional `UacConfig` block in `audio_config.json` (no version bump — fields default and SP#6 doesn't require schema change). Fields: `enabled: bool` (default true), `downstream_device: String` (default `hw:1,0`), `upstream_device: String` (default `hw:1,0`).
- Acceptance script `scripts/test-uac2-routing.sh` covering the six criteria in §2.10. The Mac-side parts (afplay, arecord, osascript) ship as a host-side helper script `scripts/test-uac2-routing-host.sh` to be invoked manually since CI doesn't have a Mac with the Wave attached.
- `docs/AUDIO_HARDWARE.md` extended with the UAC2 block diagram and the host-volume-mirror sequence.
- Cleanup: replace the existing `speaker_mixer::amixer` shell-out (in `AudioService::run`'s `Command::ToggleSpeakerMute` handler) with the new `UacControl`-routed mute. SP#5 explicitly deferred this; SP#6 owns it because volume control is now end-to-end through the UAC2 mixer events.

**Out of scope:**
- Multichannel UAC2 (8-ch raw mic upstream). Future SP#6.5 if a host app needs it.
- Higher sample rates (88.2 / 96 / 192 kHz). Kernel UAC2 endpoint is fixed at 48 k via `c_srate` / `p_srate`. Re-configfs would require kernel re-init, separate sub-project.
- USB Audio Class 1 (UAC1) compatibility for legacy hosts. UAC2 is universal post-2015.
- ASIO compatibility for Windows pro-audio hosts. UAC2 is what matters.
- Wave Controller-specific extensions (telephony class units, vendor commands). Stock firmware may negotiate these; we'll mirror what stock does *if* a packet capture reveals incompatibility. Default assumption: standard UAC2 is sufficient.
- Wake-on-USB-active behaviour. Kernel-side concern, separate from waved.

## 4. Non-goals

- **We do not change the kernel f_uac2 configuration.** `S89usbgadget` already configures both directions correctly. We don't re-write the configfs layout. If a future SP needs different parameters (channel count, rate), it owns its own configfs change.
- **We do not implement a full software mixer.** UAC2-downstream + SIGUSR1 test-tone + SIGUSR2 file-play all share the same `play_batch` mpsc queue. In practice only one is active at a time; if two collide, last-writer-wins is acceptable for these dev paths.
- **We do not buffer the upstream side beyond one period.** Upstream FIFO holds at most 4 frames (~85 ms at 1024-sample periods). If the host can't drain that fast, the device has bigger problems than playback latency. Drop oldest, log, continue.
- **We do not expose an opt-out for the AEC reference tap.** Once SP#5 ships, the post-DSP tap is the canonical reference signal; the existing `subscribe_reference()` callers (the soft-ref capture source) are unaffected. SP#6 just adds another batch source upstream of the tap.

## 5. Data model changes

### 5.1 Optional `UacConfig` (no version bump)

```rust
// crates/waved-audio/src/audio_config.rs (additive, optional)
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UacConfig {
    #[serde(default = "default_uac_enabled")]
    pub enabled: bool,                    // default true
    #[serde(default = "default_uac_dev")]
    pub downstream_device: String,        // default "hw:1,0"
    #[serde(default = "default_uac_dev")]
    pub upstream_device: String,          // default "hw:1,0"
    #[serde(default = "default_uac_period")]
    pub period_frames: usize,             // default 1024
    #[serde(default = "default_uac_periods")]
    pub buffer_periods: u32,              // default 4
}

impl Default for UacConfig { /* sensible defaults */ }
```

Added as `pub uac: Option<UacConfig>` on `AudioConfig`. When absent, default values are used. No version bump required — purely additive optional field.

### 5.2 New `Command` variant (already covered by SP#5)

`Command::SetSpeakerVolume(u16)` and `Command::SetMicVolume(u16)` already exist (`crates/waved-state/src/lib.rs`). UAC2 control task uses them as-is.

### 5.3 `/tmp/audio_stats.json` extensions

```json
{
  // ... existing fields
  "uac": {
    "downstream_active": true,        // host has hw:1,0c open and writing
    "upstream_active": true,          // host has hw:1,0p open and reading
    "downstream_drops": 0,            // periods dropped due to drift over-fill
    "upstream_drops": 0,              // periods dropped due to host slow-drain
    "downstream_underruns": 0,        // ALSA xruns on read side
    "upstream_underruns": 0,          // ALSA xruns on write side
    "host_set_volume_events": 0,      // counter: Mac → device volume changes
    "device_set_volume_events": 0     // counter: touch UI → host updates
  }
}
```

Diagnostic only. The meeting service ignores them.

## 6. Algorithms

### 6.1 Downstream bridge (`uac/bridge.rs`)

```
loop {
    match pcm.io_i16().readi(&mut buf) {
        Ok(0)                    => continue,                  // device idle, retry
        Ok(frames)               => {
            // buf now contains `frames * channels` interleaved samples.
            // Trim to period and submit.
            let _ = playback.play_batch(buf[..frames * 2].to_vec());
        }
        Err(e) if e == EAGAIN    => sleep(50ms),               // host not selected
        Err(e) if e == EPIPE     => pcm.prepare()?,            // overrun
        Err(e)                   => return Err(e.into()),
    }
}
```

ALSA capture is opened with `Direction::Capture`, `Access::RWInterleaved`, `Format::s16`, 2 channels, 48 kHz, period 1024 frames, buffer 4 periods (= 85 ms). Block on `readi`; period boundary scheduling is the kernel's job.

Drift handling: sample `playback.queue_depth()` after every successful read. If queue depth > 4 periods, skip the next `play_batch` (drop the period). If queue is at 0 and `readi` returns 0 (no host audio), the existing silence-fill in `run_playback_loop` covers it — bridge does nothing, drains naturally.

### 6.2 Upstream sink (`uac/upstream.rs`)

```
let mut rx = audio_svc.subscribe_processed();
loop {
    match rx.blocking_recv() {  // tokio broadcast supports blocking_recv() outside async
        Ok(frame)            => {
            // mic_mono_f32: Vec<f32> length = period_frames
            // duplicate to stereo: L = R = clamp(mono * 32767, i16)
            let stereo: Vec<i16> = frame.mic_mono_f32.iter()
                .flat_map(|&v| {
                    let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                    [s, s]
                })
                .collect();
            match pcm.io_i16().writei(&stereo) {
                Ok(_)                 => {}
                Err(e) if e == EBADFD => sleep(50ms),    // host not draining
                Err(e) if e == EPIPE  => pcm.prepare()?, // underrun
                Err(e)                => return Err(e.into()),
            }
        }
        Err(broadcast::Lagged(n))   => warn!("uac-upstream lagged {n} frames"),
        Err(broadcast::Closed)      => return Ok(()),
    }
}
```

ALSA playback opened with `Direction::Playback`, same format/rate/period/buffer as downstream. Period frames match `ProcessedAudioFrame` size (1024 by default). If they ever differ (e.g., capture period changes), the upstream sink reshape-pads on the fly — same defensive logic as `PlaybackService::run_playback_loop`.

> **Updated 2026-10-06 (milestone 2).** The sink now writes
> `ProcessedAudioFrame::uplink_interleaved_i16`: in Stereo mode the rebuilt pair
> (L/R = processed mid ± the beam's side, ducked by the mid's suppression ratio,
> `processed_frame::rebuild_stereo`), in every other mode the mono signal on both
> channels. After each (re)start (`prepare`) it writes `cushion::PREFILL_PERIODS`
> (2) periods of silence ahead of the first frame, so one late frame doesn't
> underrun the host. Device measurements: `docs/AUDIO_HARDWARE.md` "Uplink Level
> and CPU".

Mic mute is already applied in the capture loop's `apply_mic_mute(&mut frame.data, muted)`. By the time `subscribe_processed` delivers the frame, mute has zeroed the data; upstream emits zeros; host hears silence. No mute-aware code in the upstream sink.

### 6.3 Volume + mute mirror (`uac/control.rs`)

ALSA mixer on card 1 exposes the f_uac2 controls. We use `alsa::mixer::Mixer::open("hw:1")` and register an event callback. The kernel emits a `SND_CTL_EVENT_MASK_VALUE` event when the host issues SET_CUR.

```
loop {
    mixer.handle_events()?;
    for elem in mixer.iter() {
        let name = elem.get_id().get_name().unwrap_or("");
        match name {
            "PCM Capture Volume"   => /* host set our mic volume — update mic_volume state */,
            "PCM Capture Switch"   => /* host muted our mic — toggle mic_muted */,
            "PCM Playback Volume"  => /* host set our speaker volume → SetSpeakerVolume */,
            "PCM Playback Switch"  => /* host muted speaker → ToggleSpeakerMute */,
            _ => {}
        }
    }
    sleep(50ms);  // batch events, smooth Mac slider drag
}
```

Reverse direction: when `AudioState.speaker_volume` changes via the device-side path (touch UI / `/tmp/waved-volume`), a watcher task calls `mixer.find_selem("PCM Playback Volume").set_playback_volume_all(v)`. The kernel f_uac2 driver then reports the new value to the host on the next GET_CUR (asynchronous, but Mac polls roughly every second for the menu-bar slider).

Volume range: kernel f_uac2 in `S89usbgadget` is configured `c_volume_min=-100, c_volume_max=0, c_volume_res=1` (and same for `p_volume_*`) — in 0.5 dB units per UAC2 spec. Maps linearly to `AudioState.speaker_volume: u16` (which is in 0–100 percent). Conversion is a single multiply.

### 6.4 Lifecycle

Both bridges are spawned at the same point in `main.rs` as the existing capture and playback threads (inside the `capture-alsa` block, after `spawn_playback`). The threads exit when:
- `play_batch` returns `Stopped` (downstream — playback service died)
- The `subscribe_processed` channel closes (upstream — audio service died)
- An unrecoverable ALSA error occurs

Re-spawn is the watchdog's job (`/etc/init.d/S60waved`'s while-true loop). Within a single waved process, a thread crash takes down waved cleanly via the existing FATAL → exit path.

## 7. Risks + mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Clock skew accumulates over a long call | Drift, eventual under/overrun | Drop/duplicate at period boundaries (§6.1); 21 ms gap is inaudible during speech, rare during music |
| Host changes UAC2 endpoint format mid-call (rate, channels) | ALSA renegotiation, dropped frames | Kernel f_uac2 is fixed at 48 k stereo S16 in `S89usbgadget` — host can't re-negotiate. If a future kernel build adds rate negotiation, bridge will return `Fatal` and the watchdog respawns waved (acceptable: rare event, fast recovery). |
| Wave Controller incompatibility (proprietary UAC2 extensions) | Wave Controller doesn't recognise the device | Default assumption is standard UAC2 works. If broken, USB packet capture vs stock identifies the missing class request; mitigation is in-kernel (configfs) or vendor-specific descriptor — separate task. |
| Mac volume slider lags on rapid drag | Bad UX | 50 ms polling debounces multi-event drags; kernel mixer events coalesce naturally. Sub-100 ms perceived latency is well within Mac UI norms. |
| Concurrent SIGUSR1 test-tone collides with host audio | Test tone interrupts call | Out of scope for production: SIGUSR1/SIGUSR2 are dev-only. Document in test scripts. |
| AEC reference tap goes stale during host disconnect | AEC processes silence vs nothing | Already handled: when no `play_batch` happens, `run_playback_loop` writes silence and broadcasts it as the reference. AEC sees silence in, silence out — correct behaviour. |
| Mic upstream during host disconnect bottlenecks | broadcast `subscribe_processed` lags | `subscribe_processed` channel has `lagged` semantics; bridge logs and continues. Capacity is set to 16 frames (~340 ms) in the existing code; ample for Mac-attach latency. |
| Volume mirror loop (host → device → host → …) | Infinite event ping-pong | Both directions write only when the value changes; mirror writes are gated by an "ignore-self" flag (set when we initiate a write, cleared in the next event from the kernel). |

## 8. Open questions

- **Wave Controller protocol.** Stock firmware almost certainly handles some Wave Controller-specific UAC2 control surface (microphone gain table, sidetone, etc.). Default-config UAC2 may not give Wave Controller all the knobs it expects. Diagnose with `usbmon` + Wireshark on the Mac side once SP#6 lands; mitigation is additive (vendor descriptors), doesn't gate ship.
- **Echo from device-internal sounds.** UI sounds (touch beeps, transition tones) currently don't exist in waved but might in future SPs. When they do, they'll feed `play_batch` like everything else, get DSP'd, captured by mics, AEC-cancelled — same pipeline as host audio. No special-case needed.
- **Upstream EQ / gain.** Stock firmware had `Insta360_UplinkAsr*` paths suggesting they shaped the upstream signal. Possibly to compensate for mic frequency response. Defer until a host-side listening test reveals a need; add a single biquad in the upstream sink if so.

## 9. Self-review

- [x] No `TBD` / `TODO` placeholders left in the spec.
- [x] No internal contradictions: AEC tap behaviour consistent with SP#5 spec; volume policy single-sourced (host drives, device mirrors).
- [x] Scope is single-implementation-plan-sized: one new module, two threads, one polling task, one optional config block. Cleanup of `speaker_mixer::amixer` is small enough to stay in this SP rather than spawning a sub-task.
- [x] Ambiguity check: "bidirectional" defined as both kernel UAC2 endpoints actively bridged; "AEC works for host audio" defined as ERLE ≥ 20 dB acceptance test.
- [x] Acceptance criteria are scripted, host-side parts split out where they need a Mac to run.
- [x] Dependency on SP#5 is explicit: SP#5's `play_batch` → DSP → tap pipeline must be in place before SP#6 can satisfy the AEC requirement.
