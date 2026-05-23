# waved-wakeup-probe — Design

**Goal:** A one-shot CLI benchmark that measures DSP wake-from-silence latency on
the Insta360 Wave: after the audio pipeline has been quiescent for N seconds,
how long until a speech-like burst makes it through AEC/NS, and how much of
the burst's leading edge is suppressed.

**Why this metric:** The chirp-roundtrip number reported by `waved-latency-probe`
measures steady-state propagation. It does *not* surface the bug class the user
actually hits — "the first ~100 ms of speech gets eaten when I start talking
after a quiet stretch" — caused by AEC filter coefficients, DFN3 internal
state, and the beamformer DoA estimator decaying during silence and needing
to re-converge. See [[feedback_latency_benchmark_methodology]].

**Scope:** One-shot CLI tool. Not a service, not exported as a metric, not
wired into `waved`'s runtime. Run on demand; print/JSON one report; exit.

---

## 1. Architecture

Standalone binary, same crate layout as `waved-latency-probe`. Constructs its
own `waved_audio::AudioService` with the same `frame_size` / `sample_rate` /
`MicGeometry` that `waved` uses in production, then drives it synchronously
via `process_frame()`:

1. Generate a deterministic speech-shaped burst (~200 ms).
2. Feed `silence_seconds` of zero-fill, splice in the burst, capture every
   output frame.
3. Cross-correlate the captured output against the known burst to recover
   the burst's arrival offset, then derive latency + leading-edge loss.

What this design **does NOT measure**: mic+ADC physical wake, USB UAC TX
queue latency. Those are fixed sub-period costs we don't control. The probe
isolates the variable cost we *do* control — DSP state-machine wake-up.

---

## 2. File Layout

```
waved/crates/waved-wakeup-probe/
├── Cargo.toml
└── src/
    └── main.rs        # CLI + orchestration + reporting (~200 LoC target)
```

One file. No internal modules. The crate has exactly one job and shouldn't
grow past it.

**Dependencies:**

| Crate | Why |
|---|---|
| `waved-audio` (path) | `AudioService`, `AudioFrame`, `MicGeometry`, `ProcessedAudioFrame` |
| `clap` (workspace) | CLI flags |
| `tokio` (rt + macros + sync) | `AudioService::new` takes `mpsc`/`watch` channels even though `process_frame()` is driven synchronously |
| `tracing` / `tracing-subscriber` (workspace) | Log output |
| `serde` / `serde_json` | `--json <path>` output mode |

---

## 3. CLI

```
waved-wakeup-probe \
  --silence-seconds <u32>    # idle duration before each cold burst (default: 60)
  --burst-ms <u32>           # burst duration in ms (default: 200)
  --warmup-bursts <u32>      # warm bursts to establish baseline (default: 5)
  --cold-runs <u32>          # cold (post-silence) bursts to measure (default: 10)
  --json <path>              # optional: write machine-readable report
```

**Single-run schedule:**

1. Construct `AudioService` once. Heavy work (DFN3 model load, ≈ 50 ms) happens
   here and is **not** counted against any latency number.
2. **Warmup phase** — feed `warmup_bursts` back-to-back, with no silence
   between them. Record per-burst latency. Median across warmup runs =
   `warm_latency_ms`.
3. **Cold phase** — repeat `cold_runs` times: feed `silence_seconds` of zeros,
   then one burst, then a 200 ms tail of zeros. Measure each.
4. Print summary; optionally write JSON.

---

## 4. Measurement Algorithm

### 4.1 Stimulus generation

A deterministic speech-shaped noise burst:

- **Source:** white noise (fixed seed), band-pass filtered to 200 Hz – 4 kHz
  (covers voice fundamentals + first three formants).
- **Envelope:** raised-cosine ramp on the first and last 10 ms of the burst
  to avoid click artifacts at the edges.
- **Length:** `burst_ms` (default 200 ms = 9600 samples at 48 kHz).
- **Level:** −20 dBFS peak (typical speech RMS, well clear of NS noise floor).

**Why speech-shaped noise rather than a sine, chirp, or recorded speech clip:**

- A pure tone gets classified as steady-state interference by NS models and
  may be attenuated more than speech.
- A chirp is already what `waved-latency-probe` uses; reusing it would give
  no new information.
- A recorded speech clip ships an opaque WAV with the binary, complicates
  reproducibility, and licensing-around-test-fixtures is annoying.

The deterministic seed means: same binary → byte-identical burst → comparable
xcorr results across runs and across hosts.

### 4.2 Drive loop

```rust
let total_samples = silence_samples + burst_samples + tail_samples;
let mut input: Vec<f32> = Vec::with_capacity(total_samples);
input.extend(std::iter::repeat(0.0).take(silence_samples));
input.extend(&burst);
input.extend(std::iter::repeat(0.0).take(tail_samples));

let mut output: Vec<f32> = Vec::with_capacity(total_samples);
for chunk in input.chunks(frame_size) {
    let mic_8ch = expand_to_8ch(chunk);          // duplicate mono → 8 channels
    let ref_2ch = vec![0i16; chunk.len() * 2];   // no playback ref signal
    let frame = AudioFrame { mic_8ch, ref_2ch, /* ... */ };
    let processed = audio_service.process_frame(&frame);
    output.extend_from_slice(&processed.mic_mono_f32);
}
```

The 200 ms tail ensures we capture the trailing edge of the burst even if the
DSP introduces a small group delay.

### 4.3 Per-burst metrics

| Metric | Definition |
|---|---|
| `latency_ms` | `argmax(xcorr(burst, output)) − silence_samples`, in ms. When the burst first appears at the output. |
| `leading_edge_loss_ms` | Portion of the burst (starting from `latency_ms`) where the rolling 10 ms RMS window is below 50% of the input burst's corresponding 10 ms RMS window. How much of the burst's early energy got suppressed. |
| `peak_db` | Peak dBFS of the output burst window. Sanity check that output isn't silenced entirely. |

If the cross-correlation peak score falls below a configurable threshold
(default 0.3 of input burst auto-correlation peak), the burst is marked
`MISSED` for that run and excluded from medians.

### 4.4 Aggregate report

Stdout:

```
waved-wakeup-probe — 5 warm + 10 cold runs (60s silence, 200ms bursts)

warm  latency:            median   4.2 ms  (5 runs,  p95   5.1 ms)
cold  latency:            median  78.4 ms  (10 runs, p95 112.0 ms)
cold  leading-edge loss:  median  38.0 ms  (p95 64.0 ms)
warm  peak:  -8.0 dBFS    cold peak:  -11.0 dBFS
missed: 0 / 15
```

JSON (when `--json` given):

```json
{
  "config": { "silence_seconds": 60, "burst_ms": 200, "warmup_bursts": 5, "cold_runs": 10 },
  "warm":   { "latency_ms": { "median": 4.2, "p95": 5.1 }, "peak_db": -8.0 },
  "cold":   { "latency_ms": { "median": 78.4, "p95": 112.0 },
              "leading_edge_loss_ms": { "median": 38.0, "p95": 64.0 },
              "peak_db": -11.0 },
  "missed": 0, "total_runs": 15
}
```

---

## 5. Error Handling & Edge Cases

**Abort with a clear message:**

- `AudioService::new()` fails or produces a config we can't reproduce
  → exit 2, print `pipeline construction failed: <reason>`.
- All cold runs missed → exit 3, print `DSP swallowed every burst — check NS aggression / mute state`.

**Handled inline:**

- **Mute state.** Probe explicitly drives `AudioState.mute = false` via the
  `watch::Sender` before running. If `AudioService` later observes muted, the
  bursts go to zero and we'd report MISSED across the board; the explicit
  pre-set prevents this.
- **DFN3 model load.** Constructed once before warmup; not counted in any
  latency number.
- **Frame size mismatch.** The `input` vector is right-padded with zeros so
  `input.len() % frame_size == 0` — the final `chunks(frame_size)` chunk is
  exactly `frame_size`. Padding is added to the tail, after the burst, so it
  doesn't affect measurement.

**No retries, no fallbacks.** A missed burst is a real result, not a
transient to mask.

**Not handled (intentional):**

- **Host runs.** The probe needs real `waved-audio` Processor state. Build
  target is `aarch64-unknown-linux-musl`, deployed via the existing flash
  script.
- **Concurrent `waved` running.** Not detected. If `waved` is up at the
  same time, ALSA contention is on the operator to avoid — same constraint
  as `waved-latency-probe`. Documented in `--help`, not enforced.

---

## 6. Testing

### 6.1 Unit tests (default `cargo test`, in `main.rs` under `#[cfg(test)]`)

| Test | Assertion |
|---|---|
| `burst_generation_is_deterministic` | Same seed → byte-identical samples; expected length and sample rate. |
| `burst_envelope_has_raised_cosine_edges` | First/last 10 ms samples ≈ 0; peak in middle. |
| `xcorr_finds_known_offset` | Synthetic input with burst at known offset → `latency_ms` matches within ±1 sample. |
| `leading_edge_loss_zero_when_output_is_input` | Pass-through path → ~0 ms loss. |
| `missed_burst_returns_none` | Output with no detectable burst → `None`, not a bogus number. |
| `aggregate_report_excludes_missed_from_medians` | One MISSED run does not skew the median. |

These cover the math (xcorr, envelope, aggregation). They do NOT need a real
`AudioService`.

### 6.2 Integration test (`#[ignore]`-gated, on-device only)

`cargo test -- --ignored` constructs a real `AudioService`, runs one warmup
+ one cold cycle, and asserts `warm_latency_ms < cold_latency_ms`. Catches
gross regressions in the wakeup logic but is excluded from the default test
gate because real DSP timing depends on hardware.

### 6.3 Manual on-device validation (commit-time)

1. Deploy via the existing flash script.
2. SSH in, run `./waved-wakeup-probe --silence-seconds 10` (short, fast).
3. Verify warm latency < 10 ms, cold latency ≥ warm, no `MISSED`.
4. Run `--silence-seconds 60` for the real measurement; record the baseline
   numbers in the commit message so future regressions are spottable.

---

## 7. Out of Scope

- Continuous monitoring / metric export.
- Host-side UAC capture (would measure USB + host buffer; not the bug class
  we care about).
- Speaker self-playback stimulus (AEC would cancel it).
- IPC into running `waved` (would need a debug RPC surface; standalone
  process reproduces enough of the deployed state for a regression metric).
- Multi-channel reporting. The probe reports the post-DSP mono output. The
  per-channel beamformer behavior is not in scope.

---

## 8. Open Questions / Follow-ups

None blocking. Possible future extensions, NOT part of this spec:

- Add `--reference-burst <path>` to allow a custom WAV stimulus.
- Sweep `silence_seconds ∈ {1, 5, 10, 30, 60, 120}` in one run to chart the
  wake-up cost as a function of idle duration.
- Wire the JSON output into a CI dashboard for cross-build comparison.
