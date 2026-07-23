# Full-duplex residual-echo suppression implementation plan

**Design:** `docs/superpowers/specs/2026-07-23-full-duplex-residual-echo-design.md`

## Task 1: Version and validate the AEC tuning block

**Files:**

- Modify `crates/waved-audio/src/audio_config.rs`
- Modify `crates/waved-audio/config/audio_config_default.json`

1. Add failing parser tests for audio-config version 5 and the optional `aec`
   block.
2. Cover the version 3/4 compatibility path, default values 40/15 dB, valid
   boundary values, active attenuation exceeding far-end attenuation, and
   values above 60 dB.
3. Add `AecConfig` with positive attenuation magnitudes and a validated
   default.
4. Advance the embedded JSON to version 5 while continuing to accept versions
   3 and 4.
5. Run the focused audio-config tests, then the complete `waved-audio` config
   test group.

## Task 2: Define same-frame AEC metrics

**Files:**

- Modify `crates/waved-audio/src/processed_frame.rs`
- Modify `crates/waved-audio/src/stats_collector.rs`

1. Add failing tests for an `AecFrameMetrics` snapshot containing reference,
   pre-AEC, post-linear-AEC, post-residual, post-noise-suppression, and final
   uplink RMS values.
2. Require linear ERLE and effective final suppression to be absent below the
   reference-activity floor and finite when present.
3. Attach the snapshot to the exact `ProcessedAudioFrame` that produced it;
   do not derive ERLE from the independently received raw-frame broadcast.
4. Add explicit JSON fields for the metrics, residual-stage state/tuning, and
   active noise-suppression backend.
5. Keep `mic_post_aec_rms` serialization compatible, document it as the legacy
   final-processed value, and add `mic_uplink_rms` with unambiguous semantics.
6. Prove in tests that pre/post-residual metrics exclude user gain and that
   final uplink RMS includes gain and limiting.

## Task 3: Build the bounded Speex residual-echo component

**Files:**

- Modify `crates/waved-audio/src/aec/sys.rs`
- Modify `crates/waved-audio/src/aec/speex.rs`
- Modify `crates/waved-audio/src/aec/mod.rs`

1. Expose the vendored Speex controls for far-end-only and
   near-end-active echo suppression.
2. Add failing synthetic tests for:
   - residual reduction after converged linear AEC;
   - near-end-only transparency with a silent reference;
   - double-talk near-end retention within 3 dB;
   - invalid tuning rejection;
   - preprocessor-before-echo-state destruction.
3. Refactor the coupled native objects behind one `SpeexAecStage` that owns the
   linear canceller and linked preprocessor.
4. Configure residual-only operation with denoise disabled and the validated
   40/15 dB defaults.
5. Return explicit construction errors and expose whether the residual stage
   is active.
6. Process each frame in place with no queue, lookahead, or microphone delay.
7. Run the focused Speex tests under Miri-compatible ownership assumptions
   where possible, then run all `waved-audio` AEC tests.

## Task 4: Integrate residual suppression before noise and gain

**Files:**

- Modify `crates/waved-audio/src/lib.rs`
- Modify `crates/waved-audio/src/aec/deep_filter.rs` only if the backend label
  or fallback interface requires it

1. Add failing pipeline tests for the required order:
   beamforming, linear AEC, residual suppression, optional environmental
   noise suppression, user gain, limiter.
2. Replace the separate echo-canceller/noise-suppressor ownership with the
   composite Speex stage plus an optional DFN stage.
3. Keep residual echo handling active for every `NrLevel`, including Off.
4. On successful DFN initialization, run DFN after residual suppression.
5. On DFN failure, enable denoise on the already-linked Speex preprocessor
   rather than constructing a second echo-linked preprocessor.
6. Populate all `AecFrameMetrics` values at their exact stage boundaries and
   finalize `mic_uplink_rms` only after gain and limiting.
7. On residual-stage construction failure, retain linear AEC, report the
   inactive state, and never produce a silent microphone.
8. Confirm both USB uplink and meeting recording still consume the same final
   `ProcessedAudioFrame`.

## Task 5: Correct acceptance scripts and documentation

**Files:**

- Modify `scripts/test-aec-on-device.sh`
- Modify `scripts/test-aec-conferencing.sh`
- Modify `docs/AUDIO_HARDWARE.md`
- Modify `TODO.md` only for evidence or roadmap status directly resolved by
  this work

1. Replace raw-channel-versus-post-gain ERLE calculations with the same-frame
   metrics.
2. Make the speech fixture mandatory; retain the tone only as a quick linear
   convergence check.
3. Report linear ERLE, residual suppression, final effective suppression,
   gain, limiter reduction, latency, and all integrity/UAC drop counters.
4. Add controlled near-end-only and double-talk assertions.
5. Document that residual echo control remains active when environmental
   noise suppression is Off.
6. Document the corrected meaning of legacy `mic_post_aec_rms`.
7. Add regression coverage so future microphone-gain changes cannot
   accidentally corrupt ERLE reporting.

## Task 6: Run host verification and review the implementation

1. Run the focused red/green tests after each task and retain the initial
   failure evidence.
2. Run:
   - `cargo fmt --all --check`;
   - `cargo test -p waved-audio`;
   - `cargo test -p waved-state`;
   - `cargo test -p waved`;
   - affected meeting tests;
   - UAC2 upstream/downstream tests;
   - strict clippy for changed crates;
   - `git diff --check`.
3. Inspect native allocation/drop ordering, fallback behavior, frame-stage
   ordering, JSON compatibility, and the absence of new audio buffering.
4. Commit implementation changes in reviewable logical commits, separate from
   the already committed design and implementation-plan documents.

## Task 7: Cross-build and deploy with rollback

1. Cross-build the ARM64 production binary with the established
   `ui,capture-alsa` feature set.
2. Strip and inspect architecture, GLIBC requirements, and checksum.
3. Save the currently deployed binary and checksum as a temporary rollback
   artifact.
4. Deploy the candidate atomically and restart only `waved`.
5. Verify one production process, PDM gain 24, saved Wi-Fi association, UAC2
   input/output enumeration, `mic_volume = 400`, and zero startup integrity
   faults.
6. Roll back immediately if the service crash-loops, USB audio disappears, or
   the microphone becomes silent.

## Task 8: Run controlled device acceptance

1. At speaker volume 100, run silence, tone, and repeated voice-like
   far-end-only fixtures.
2. Require same-frame linear ERLE at least 20 dB and final effective
   suppression at least 25 dB.
3. Run a fixed-position near-end fixture alone and during distinct far-end
   speech; require near-end double-talk loss no more than 3 dB and silent-
   reference transparency within 0.5 dB.
4. Repeat after mute/unmute and a speaker-volume transition from 50 to 100.
5. Benchmark at least 20 capture-to-USB latency trials against the saved
   production binary; require no more than 5 ms regression and total device
   microphone latency below 50 ms.
6. Run 30 minutes of sustained bidirectional speech/playback; require no
   XRUNs, raw drops, processed drops, active UAC2 drops, or more than 5
   percentage points of added CPU.

## Task 9: Calibrate gain and complete the live-call gate

1. After echo acceptance passes, test `mic_volume` 400, 600, and 800 at
   30–50 cm.
2. Select the lowest value producing -24 to -18 dBFS speech RMS, peaks below
   -3 dBFS, and no more than 3 dB limiter reduction on normal speech.
3. Re-run final-gain echo and latency acceptance at the selected value.
4. Save the selected value only to the test device's `config.toml`; do not
   change the project-wide default without the separate stock-vs-Waved
   fixture.
5. Run a live call with both USB directions on the Wave and cover
   far-end-only, near-end-only, simultaneous speech, and rapid interruptions.
6. Require the far-end listener to report no self-echo, natural close
   near-end speech, intelligible double-talk, and no perceptible delay or
   clipped first syllables.
7. If any full-duplex or latency gate fails, restore the prior binary and the
   clean +12 dB/PDM-24 device configuration. Do not substitute gain ducking,
   microphone buffering, or a half-duplex gate.
