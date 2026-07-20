# Waved — whole-device roadmap

Last reviewed: 2026-07-20

Waved replaces the stock Insta360 Wave firmware with an open device stack for
audio capture and processing, local meeting recording, USB audio, Wi-Fi
provisioning, and durable webhook delivery.

The current focus is:

1. never lose or silently damage a recording;
2. fix user-visible audio gaps and measure them against stock firmware; and
3. make recording delivery dependable enough for unattended use.

## Status and priority

Stock-parity status:

- **Matched** — the relevant observable stock behavior is reproduced.
- **Better** — measured behavior is better, or Waved adds a useful capability
  without regressing the stock path.
- **Partial** — the main behavior works, but a known user-visible part is
  missing.
- **Unknown** — a concrete stock-device comparison is still required.
- **Not planned** — the difference is understood and intentionally excluded.

Priority:

- **P0** — recording-loss, corruption, or silent-failure risk.
- **P1** — important audio quality, delivery, or stock-parity work.
- **P2** — useful hardening or observability.
- **Deferred** — starts only when its stated trigger occurs.

## Current priorities

### P0 — Audio capture and recording integrity

- [x] **Detect and recover from audio-capture failure.** Capture EOF/read
  failure and raw/processed channel closure are typed terminal exits. Active
  takes fail closed, the meeting actor acknowledges durable fault persistence,
  and the process exits through the normal watchdog path. On-device idle and
  active injection verified restart without deleting the retained partial WAV.
- [x] **Make every recording-path drop or write failure explicit.** Xruns,
  raw/processed drops, channel closure, and WAV writes update a shared integrity
  epoch. Stop drains queued faults before finalization; damaged takes retain
  only `audio.wav.partial`, persist `artifact_error`, never become uploadable,
  and show fixed on-device fault copy. Deterministic host tests and on-device
  frame-drop/writer-failure injection pass.
- [ ] **Run long-session and interrupted-finalization acceptance.** Record for
  at least one hour while tracking RSS, dropped frames, WAV duration, sample
  count, and free space. Repeat with process termination and power loss during
  recording/finalization. Boot reconciliation must retain or classify every
  artifact without presenting a corrupt file as uploaded.
- [ ] **Exercise storage-pressure safety on-device.** Force both configured
  retention thresholds in an isolated fixture area. Only recordings already
  marked `uploaded` may be pruned; queued, failed, local-only, active, and
  incomplete recordings must survive. New recording starts must be blocked
  when no safe pruning candidate exists.

### P1 — Recording and webhook production hardening

- [ ] **Add the missing end-to-end webhook acceptance script.** Implement
  `scripts/test-webhook-setup.sh` around the bounded local receiver. Cover the
  setup form, automatic/ask/local-only modes, authentication redaction,
  multipart metadata, WAV properties, and idempotency.
- [ ] **Complete the failure and recovery matrix.** Test offline recording,
  Wi-Fi loss during upload, transient retry, permanent HTTP 400, redirect
  rejection, retry exhaustion, manual retry, endpoint changes, and reboot
  during upload. Verify the `Saving recording...`, queued, attention, and
  uploaded notifications against durable SQLite state.
- [ ] **Qualify a production HTTPS webhook.** Configure a real authenticated
  endpoint through the setup network, run its webhook test, cold-boot the
  device, record, and confirm one accepted request with the same meeting UUID
  used as the idempotency key. The current local HTTP receiver remains a
  development tool only.

### P1 — Audio quality and stock comparison

- [ ] **Build a repeatable stock-vs-Waved audio fixture.** Capture the same
  microphone, speaker-loopback, USB playback, and USB microphone material on
  stock and open firmware at fixed hardware gain. Compare level, frequency
  response, noise floor, clipping, latency, and AEC residual. This fixture is
  the gate for changing stock-parity rows currently marked **Unknown**.
- [ ] **Measure and, if needed, match upstream microphone EQ/gain.** Stock
  contains `Insta360_UplinkAsr*` paths suggesting microphone frequency-response
  compensation. First resolve this with an impulse/listening comparison; add
  upstream biquads only if a repeatable audible or measured gap exists.
- [ ] **Validate SRP-PHAT on-device before changing the DOA default.** Run
  `scripts/test-doa-improvements.sh` with real meeting positions, including
  270 degrees and simultaneous-room-noise cases. Promote SRP-PHAT only if it
  materially improves speaker separation and remains below the 5% CPU-delta
  limit. Elevation remains a coarse below/level/above band because the
  microphone geometry cannot support a calibrated steep-angle estimate.

### P2 — Audio hardening and observability

- [ ] **Prebuild DeepFilterNet at startup.** A live switch from NR Off to an
  enabled level currently rebuilds the tract session and can drop about 50
  frames. Construct DFN3 once, switch the active backend without rebuilding,
  and then re-baseline `scripts/cpu_delta.py`.
- [ ] **Profile neural suppression under sustained speech and playback.**
  Existing acceptance used a mostly quiet room where DFN3 gating skipped work.
  Repeat with `scripts/test-aec-conferencing.sh` playback and publish sustained
  CPU, memory, underrun, and frame-drop results.
- [ ] **Expose noise-suppression observability.** Add `ns_backend` and
  `ns_frame_ms` to `/tmp/audio_stats.json` so device tests can distinguish
  DFN3 from the Speex fallback.
- [ ] **Reap stale stats temporary files.** Remove stale
  `/tmp/audio_stats.json.tmp` after a failed atomic rename. The next successful
  write already self-corrects, so this remains low priority.

### P2 — Wi-Fi and setup hardening

- [ ] **Automate a cold-boot provisioning regression.** Verify saved-network
  fallback, automatic reconnection, setup-AP start/stop, circular-screen
  reachability, configuration migration, and webhook test status without
  relying on manual inspection.
- [ ] **Test multi-AP roaming with a local webhook receiver.** The current mesh
  can isolate clients on different radios. Confirm that Waved retries rather
  than losing recordings and uploads automatically when the route becomes
  reachable again. Do not persist a test-only BSSID lock.

## Stock-parity matrix

| Capability | Status | Evidence or remaining gap |
|---|---|---|
| Two-stream ALSA capture and aligned frames | **Partial** | SP#1 capture harness and fail-closed liveness/drop recovery pass on-device; long-session and power-interruption acceptance remain P0. |
| AEC and processed USB microphone | **Unknown** | Waved measures 22.7–26.6 dB ERLE and passes its acceptance bar, but needs the same-fixture stock comparison. |
| Beamforming modes and horizontal DOA | **Partial** | Directional modes and GCC-PHAT work; exact stock beam patterns and close/simultaneous-speaker behavior are not yet compared. |
| Neural noise suppression | **Unknown** | DFN3 works on CPU; stock-vs-Waved speech quality and sustained-load behavior remain unmeasured. |
| Playback EQ, gain staging, and limiter | **Matched** | Voice EQ matches the recovered stock configuration; media bypass, soft knee, preset ramp, and peak-hold stats are implemented. |
| Standard UAC2 playback/capture | **Matched** | Generic macOS applications play and record through the device; host-played AEC acceptance passes. |
| HID consumer and telephony controls | **Matched** | Stock report IDs and UI/MCU bridges are implemented and covered by `scripts/test-hid-control.sh`. |
| Official Wave Controller vendor controls | **Unknown** | Run the official application against Waved, then capture stock USB control traffic only if a missing control is observed. |
| macOS UAC2 master-volume control | **Partial** | Consumer keys and mute work; the kernel playback Feature Unit does not expose the stock-equivalent master-volume control. |
| Local WAV meeting recording | **Partial** | Finalization, metadata, bookmarks, diarization, reconciliation, and real uploads work; long-session and interruption acceptance remain P0. |
| Generic multipart recording webhook | **Better** | Durable streaming upload, retry, retention, idempotency, setup configuration, status UI, and toasts are open-firmware capabilities; the full failure matrix remains P1. |
| Wi-Fi provisioning and saved-network reconnect | **Better** | Waved provides a WPA2 setup AP, QR/manual credentials, a captive configuration form, saved-network fallback, and live connection status; stock setup AP was open. |
| Device configuration through setup portal | **Better** | Versioned `config.toml` fields can be edited and validated without exposing stored secrets. |
| Raw eight-channel microphone over USB | **Not planned** | Internal eight-channel capture exists; expose it to a host only when a real application requests it. |
| VIP9000 NPU inference | **Not planned** | DFN3 on Cortex-A55 meets the current CPU budget; revisit only if a supported toolchain or measured CPU blocker appears. |

## Deferred, trigger-based work

- **Wave Controller protocol probe.** Trigger: the official application fails
  to configure an observable feature. Capture `usbmon`/Wireshark traffic on
  stock and Waved, then implement only the missing controls.
- **Raw eight-channel UAC2 upstream.** Trigger: a host-side beamforming or
  research application requests the raw array.
- **`f_uac2` playback master-volume patch.** Trigger: stock-equivalent host
  volume-slider behavior becomes a release requirement and matching vendor
  kernel source is available.
- **Vendor `libmodel_infer.so` binding.** Trigger: real meetings show
  close-speaker merging that the Rust DOA implementations cannot solve.
- **Multi-source DOA.** Trigger: simultaneous speakers become a required
  diarization case.
- **Runtime DOA backend switching and configurable SRP grid resolution.**
  Trigger: same-run A/B tuning demonstrates a need.
- **NPU DFN3 backend.** Trigger: CPU profiling exceeds the budget or a usable
  VIP9000nano-di+ compiler/runtime becomes available.

## Completed foundation

- [x] **SP#1 — Capture foundation and measurement harness.**
- [x] **SP#2 — AEC and playback reference path.**
- [x] **SP#3 — Beamforming and DOA.**
- [x] **SP#4 — DeepFilterNet 3 CPU noise suppression with Speex fallback.**
- [x] **SP#5 — Playback EQ, multiband gain control, and limiter.**
- [x] **SP#6 — Bidirectional UAC2 routing and mixer state.**
- [x] **SP#7d — HID consumer/telephony control surface.**
- [x] **Wi-Fi — WPA2 QR/captive provisioning, saved-network fallback,
  automatic reconnect, and live connection status.**
- [x] **Device setup — versioned `config.toml` form, validation, secret
  preservation, apply/reconnect/restart status, and webhook test.**
- [x] **Meetings — WAV recording, metadata, bookmarks, diarization,
  reconciliation, upload modes, durable retry, safe retention, and status UI.**
- [x] **Webhook delivery — generic multipart contract, streamed WAV, JSON
  metadata, bearer authentication, idempotency, receiver tooling, and two-stage
  Stop/upload notifications.**

Authoritative detail:

- `docs/AUDIO_HARDWARE.md`
- `docs/COMPUTE_HARDWARE.md`
- `docs/WIFI.md`
- `docs/WEBHOOKS.md`
- `docs/superpowers/specs/2026-07-17-webhook-setup-and-upload-design.md`
- `docs/superpowers/specs/2026-07-20-recording-upload-toasts-design.md`
- `docs/superpowers/specs/2026-07-20-recording-pipeline-integrity-design.md`
- `docs/superpowers/plans/2026-07-20-recording-pipeline-integrity.md`

ASR models and transcription remain out of scope. The device produces
recordings and metadata for downstream systems to transcribe or analyze.
