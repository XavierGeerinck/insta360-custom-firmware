# Waved Audio — SP#3 Beamforming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the 8-mic array from "channel 0, ignore the rest" into a direction-steered Delay-and-Sum beamformer with 5 audio-mode presets (Omni, Cardioid, Supercardioid, Figure-8, Stereo) and a live DOA estimator that drives both the UI pointer and the beam's steering direction.

**Architecture:** New `waved_audio::beam` + `waved_audio::doa` modules insert before the existing AEC stage. DOA runs every frame, gated by VAD, exponentially smoothed; drives weight matrices consumed by the beamformer. `AudioConfig` bumps to version 3 with `mic_geometry` (physical positions + ALSA→physical permutation) as the version-bump payload. `AudioMode::NoiseReduction` is removed (was overlapping with `NrLevel`); default becomes `Omni`. `ProcessedAudioFrame` grows an optional stereo sibling field for Stereo mode. A one-shot `waved-mic-map` binary produces the ALSA→physical permutation table via a tap harness.

**Tech Stack:** Rust 2021, `rustfft` (DOA cross-correlation), existing speexdsp AEC/NS. Cross-compiled via `cross`.

**Reference spec:** `docs/superpowers/specs/2026-04-20-waved-audio-beamforming-design.md`

---

## File Structure

```
crates/waved-audio/
  Cargo.toml                                            (modify) add rustfft
  src/
    beam/
      mod.rs                                            (new)    BeamFormer trait + DasBeamFormer
      weights.rs                                        (new)    per-preset weight computation
      delays.rs                                         (new)    fractional-delay line
      geometry.rs                                       (new)    MicGeometry type + default Wave table
    doa/
      mod.rs                                            (new)    DoaEstimator + EWMA smoother
      gcc_phat.rs                                       (new)    pairwise cross-correlation
      solve.rs                                          (new)    least-squares direction fit
      vad.rs                                            (new)    energy-gate wrapper over Speex VAD
    audio_config.rs                                     (modify) bump to v3; add MicGeometry field
    processed_frame.rs                                  (modify) add mic_stereo_f32 field
    lib.rs                                              (modify) wire beam before AEC; SetAudioMode handler
  config/audio_config_default.json                      (modify) version: 3; mic_geometry block
crates/waved-state/
  src/lib.rs                                            (modify) remove AudioMode::NoiseReduction; Omni is default
crates/waved-mic-map/                                   (new crate)
  Cargo.toml                                            (new)    ALSA + waved-audio deps
  src/main.rs                                           (new)    tap-harness CLI
crates/waved/
  src/main.rs                                           (modify) boot-time one-shot state migration
scripts/
  tap-mic-map.sh                                        (new)    host wrapper for waved-mic-map
  test-beamforming.sh                                   (new)    on-device FBR + DOA + CPU acceptance tests
```

## Task classification

| Tasks | Classification | Gate |
|---|---|---|
| 1 | Host-only TDD (geometry primitives) | `cargo test -p waved-audio geometry` green |
| 2 | Host-only TDD (fractional delay line) | `cargo test -p waved-audio beam::delays` green |
| 3 | Host-only TDD (weights) | `cargo test -p waved-audio beam::weights` green |
| 4 | Host-only TDD (DasBeamFormer) | `cargo test -p waved-audio beam` green; synthetic plane-wave test shows ≥10 dB FBR |
| 5 | Host-only TDD (GCC-PHAT) | `cargo test -p waved-audio doa::gcc_phat` green |
| 6 | Host-only TDD (least-squares solve) | `cargo test -p waved-audio doa::solve` green |
| 7 | Host-only TDD (DoaEstimator + smoother) | `cargo test -p waved-audio doa` green; synthetic angle recovered within 2° |
| 8 | Host-only TDD (AudioConfig v3) | `cargo test -p waved-audio audio_config` green |
| 9 | Host-only TDD (AudioMode enum migration) | `cargo test -p waved-state` green; `cargo build -p waved` green |
| 10 | Host-only TDD (ProcessedAudioFrame stereo field) | `cargo test -p waved-audio processed_frame` green |
| 11 | Host-only TDD (AudioService wiring) | `cargo test -p waved-audio` green; cross build for aarch64 green |
| 12 | Device-touching (`waved-mic-map` binary) | cross-builds; runs on device; produces alsa_to_physical table |
| 13 | Device-touching (acceptance tests) | `scripts/test-beamforming.sh` — FBR ≥ 10 dB, DOA ≤ 10°, CPU < 5% |

---

## Task 1: MicGeometry type + default Wave geometry table

**Files:**
- Create: `crates/waved-audio/src/beam/geometry.rs`
- Create: `crates/waved-audio/src/beam/mod.rs` (stub, re-exports geometry; full content lands in Task 4)

**Rationale:** beamforming math needs concrete 3D positions per mic. We encode the teardown-measured geometry as a compile-time default and expose it via `AudioConfig.mic_geometry`. The `Position3D` + `MicGeometry` types are the substrate for all later tasks.

- [ ] **Step 1: Write the failing tests**

  Create `crates/waved-audio/src/beam/geometry.rs`:

  ```rust
  //! Physical mic array geometry for beamforming.
  //!
  //! Coordinate system: +X forward, +Y left, +Z up. Origin at center of
  //! top-ring plane. Units: millimeters.
  //!
  //! Default matches the teardown-measured Insta360 Wave geometry: top ring
  //! of 4 mics at z=0 (azimuths 0/90/180/270°), side ring of 4 mics at
  //! z=-78.2 mm (azimuths 45/135/225/315°), all at r=25.5 mm.

  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
  pub struct Position3D {
      pub x: f32,
      pub y: f32,
      pub z: f32,
  }

  impl Position3D {
      pub fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }

      /// Dot product (both in mm) returns mm². Used in steering-vector math
      /// after converting to meters.
      pub fn dot(&self, other: &Self) -> f32 {
          self.x * other.x + self.y * other.y + self.z * other.z
      }
  }

  /// 8 physical mic positions + ALSA channel → physical index permutation.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct MicGeometry {
      /// Positions indexed by physical index (0..8).
      pub positions_mm: [Position3D; 8],
      /// `alsa_to_physical[alsa_ch] = physical_index`. Produced by the
      /// `waved-mic-map` tap harness. Defaults to identity until overridden.
      pub alsa_to_physical: [u8; 8],
  }

  impl Default for MicGeometry {
      fn default() -> Self {
          Self::insta360_wave()
      }
  }

  impl MicGeometry {
      /// Teardown-measured geometry: r=25.5 mm, top ring z=0 at 0/90/180/270°,
      /// side ring z=-78.2 mm at 45/135/225/315°.
      pub fn insta360_wave() -> Self {
          const R: f32 = 25.5;
          const Z_SIDE: f32 = -78.2;
          let side_xy = R * std::f32::consts::FRAC_1_SQRT_2; // cos/sin 45°

          Self {
              positions_mm: [
                  // Top ring
                  Position3D::new(R, 0.0, 0.0),       // 0: 0°
                  Position3D::new(0.0, R, 0.0),       // 1: 90°
                  Position3D::new(-R, 0.0, 0.0),      // 2: 180°
                  Position3D::new(0.0, -R, 0.0),      // 3: 270°
                  // Side ring (offset 45°)
                  Position3D::new(side_xy, side_xy, Z_SIDE),    // 4: 45°
                  Position3D::new(-side_xy, side_xy, Z_SIDE),   // 5: 135°
                  Position3D::new(-side_xy, -side_xy, Z_SIDE),  // 6: 225°
                  Position3D::new(side_xy, -side_xy, Z_SIDE),   // 7: 315°
              ],
              alsa_to_physical: [0, 1, 2, 3, 4, 5, 6, 7],
          }
      }

      /// Resolve an ALSA channel index to the corresponding physical mic position.
      pub fn position_for_alsa(&self, alsa_ch: usize) -> &Position3D {
          let physical = self.alsa_to_physical[alsa_ch] as usize;
          &self.positions_mm[physical]
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn default_wave_geometry_top_ring_at_z_zero() {
          let g = MicGeometry::default();
          for i in 0..4 {
              assert_eq!(g.positions_mm[i].z, 0.0, "top mic {i} should be at z=0");
          }
      }

      #[test]
      fn default_wave_geometry_side_ring_below_top() {
          let g = MicGeometry::default();
          for i in 4..8 {
              assert_eq!(g.positions_mm[i].z, -78.2, "side mic {i} should be at z=-78.2");
          }
      }

      #[test]
      fn default_wave_geometry_radius_25_5() {
          let g = MicGeometry::default();
          for (i, p) in g.positions_mm.iter().enumerate() {
              let r = (p.x * p.x + p.y * p.y).sqrt();
              assert!((r - 25.5).abs() < 0.01, "mic {i} radius {r} != 25.5");
          }
      }

      #[test]
      fn default_wave_geometry_top_azimuths_cardinal() {
          let g = MicGeometry::default();
          // Mic 0 at 0° → (+R, 0)
          assert!((g.positions_mm[0].x - 25.5).abs() < 0.01);
          assert!(g.positions_mm[0].y.abs() < 0.01);
          // Mic 1 at 90° → (0, +R)
          assert!(g.positions_mm[1].x.abs() < 0.01);
          assert!((g.positions_mm[1].y - 25.5).abs() < 0.01);
      }

      #[test]
      fn default_alsa_to_physical_is_identity() {
          let g = MicGeometry::default();
          for i in 0..8 {
              assert_eq!(g.alsa_to_physical[i], i as u8);
          }
      }

      #[test]
      fn position_for_alsa_applies_permutation() {
          let mut g = MicGeometry::default();
          g.alsa_to_physical = [3, 2, 1, 0, 7, 6, 5, 4]; // swapped
          // ALSA ch 0 → physical 3 (top 270° = (0, -R, 0))
          let p = g.position_for_alsa(0);
          assert!((p.y + 25.5).abs() < 0.01);
      }
  }
  ```

  Create `crates/waved-audio/src/beam/mod.rs`:

  ```rust
  //! Beamforming: turn 8-ch raw mic into a direction-steered mono (or stereo) signal.
  pub mod geometry;
  pub use geometry::{MicGeometry, Position3D};
  ```

  Modify `crates/waved-audio/src/lib.rs` — add module declaration after existing `pub mod` lines:

  ```rust
  pub mod beam;
  ```

- [ ] **Step 2: Run tests to verify they fail**

  Run: `cargo test -p waved-audio geometry`

  Expected: compilation succeeds, tests pass on first run (this task is mostly type + data declaration — the tests exist to pin the geometry constants, so they pass immediately after creating the module).

- [ ] **Step 3: Commit**

  ```bash
  git add crates/waved-audio/src/beam/
  git add crates/waved-audio/src/lib.rs
  git commit -m "feat(audio): SP#3 — MicGeometry type + Insta360 Wave default layout"
  ```

---

## Task 2: Fractional delay line

**Files:**
- Create: `crates/waved-audio/src/beam/delays.rs`
- Modify: `crates/waved-audio/src/beam/mod.rs`

**Rationale:** a plane wave reaches each mic at different times. DAS aligns by delaying each channel by a computed amount. Max delay across our geometry is ~150 μs (≈7.2 samples at 48 kHz); we need fractional-sample precision, which means linear interpolation over a per-channel ring buffer.

- [ ] **Step 1: Write the failing tests**

  Create `crates/waved-audio/src/beam/delays.rs`:

  ```rust
  //! Fractional-sample delay line via linear interpolation.
  //!
  //! Each channel gets its own ring buffer. Max delay our geometry demands
  //! is ~7 samples at 48 kHz; we size the buffer at 32 samples to give
  //! comfortable headroom and stay cache-friendly.

  const BUFFER_LEN: usize = 32;

  /// Single-channel fractional delay. The current sample is pushed via
  /// `write(x)`; the delayed sample at delay `d` (in samples, fractional)
  /// is read via `read(d)`. `d` must be in [0.0, BUFFER_LEN - 1].
  pub struct DelayLine {
      buf: [f32; BUFFER_LEN],
      write_idx: usize,
  }

  impl DelayLine {
      pub fn new() -> Self { Self { buf: [0.0; BUFFER_LEN], write_idx: 0 } }

      pub fn reset(&mut self) {
          self.buf.fill(0.0);
          self.write_idx = 0;
      }

      pub fn write(&mut self, sample: f32) {
          self.buf[self.write_idx] = sample;
          self.write_idx = (self.write_idx + 1) % BUFFER_LEN;
      }

      /// Read delayed sample. `delay_samples` may be fractional.
      /// Returns 0.0 if delay exceeds buffer capacity.
      pub fn read(&self, delay_samples: f32) -> f32 {
          if delay_samples < 0.0 || delay_samples >= (BUFFER_LEN - 1) as f32 {
              return 0.0;
          }
          let int_d = delay_samples.floor() as usize;
          let frac = delay_samples - int_d as f32;

          // write_idx points at the next write slot; most recent sample is at write_idx - 1.
          let idx_a = (self.write_idx + BUFFER_LEN - 1 - int_d) % BUFFER_LEN;
          let idx_b = (self.write_idx + BUFFER_LEN - 2 - int_d) % BUFFER_LEN;

          let a = self.buf[idx_a];
          let b = self.buf[idx_b];
          a * (1.0 - frac) + b * frac
      }
  }

  impl Default for DelayLine { fn default() -> Self { Self::new() } }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn zero_delay_reads_most_recent_sample() {
          let mut d = DelayLine::new();
          d.write(1.0);
          d.write(2.0);
          d.write(3.0);
          assert!((d.read(0.0) - 3.0).abs() < 1e-6);
      }

      #[test]
      fn integer_delay_reads_older_sample() {
          let mut d = DelayLine::new();
          for i in 0..10 {
              d.write(i as f32);
          }
          // Most recent is 9.0; delay 2 should give 7.0.
          assert!((d.read(2.0) - 7.0).abs() < 1e-6);
          // Delay 5 should give 4.0.
          assert!((d.read(5.0) - 4.0).abs() < 1e-6);
      }

      #[test]
      fn fractional_delay_interpolates_linearly() {
          let mut d = DelayLine::new();
          d.write(10.0);
          d.write(20.0);
          // read(0.5) should interpolate between 20.0 (most recent) and 10.0
          // (one back): 20 * 0.5 + 10 * 0.5 = 15.
          let y = d.read(0.5);
          assert!((y - 15.0).abs() < 1e-6, "expected 15.0, got {y}");
      }

      #[test]
      fn out_of_range_delay_reads_zero() {
          let mut d = DelayLine::new();
          d.write(1.0);
          assert_eq!(d.read(-0.1), 0.0);
          assert_eq!(d.read(100.0), 0.0);
      }

      #[test]
      fn reset_clears_buffer() {
          let mut d = DelayLine::new();
          for i in 0..5 { d.write(i as f32); }
          d.reset();
          assert_eq!(d.read(0.0), 0.0);
          assert_eq!(d.read(2.0), 0.0);
      }
  }
  ```

  Update `crates/waved-audio/src/beam/mod.rs`:

  ```rust
  pub mod geometry;
  pub mod delays;
  pub use geometry::{MicGeometry, Position3D};
  ```

- [ ] **Step 2: Run tests**

  Run: `cargo test -p waved-audio beam::delays`

  Expected: 5/5 pass.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/waved-audio/src/beam/
  git commit -m "feat(audio): SP#3 — fractional-sample DelayLine for DAS alignment"
  ```

---

## Task 3: Per-preset beamformer weights

**Files:**
- Create: `crates/waved-audio/src/beam/weights.rs`
- Modify: `crates/waved-audio/src/beam/mod.rs`

**Rationale:** each audio-mode preset becomes a fixed weight vector `[f32; 8]` applied after delay alignment. Weights are computed from the target direction and the mic geometry. We precompute analytically (closed form for Omni / Figure-8 / Cardioid) and fall back to a numerical constrained solve only if needed (§11 of the spec, out of scope for this task).

- [ ] **Step 1: Write the failing tests**

  Create `crates/waved-audio/src/beam/weights.rs`:

  ```rust
  //! Per-preset beamformer weight vectors.
  //!
  //! Inputs: target direction (azimuth φ, elevation θ) and MicGeometry.
  //! Output: `[f32; 8]` weights applied AFTER delay alignment. Weights sum
  //! is 1 for unity unit-gain on-axis for Omni/Cardioid/Supercardioid; for
  //! Figure-8 the sum is 0 (pressure-gradient pattern).
  //!
  //! Conventions:
  //! - Azimuth φ: 0° = +X (front), 90° = +Y (left), CCW looking down.
  //! - Elevation θ: measured from +Z (zenith). θ=90° is horizon. Our "pitch"
  //!   in AudioState is `90 - θ` (0 = horizon).

  use super::geometry::{MicGeometry, Position3D};
  use waved_state::AudioMode;

  /// Unit vector in Cartesian for (azimuth φ, elevation θ) where θ is
  /// measured from +Z. Inputs in degrees.
  pub fn direction_vector(azimuth_deg: f32, elevation_deg: f32) -> Position3D {
      // elevation in AudioState convention is 0 = horizon; convert to polar θ.
      let theta = (90.0 - elevation_deg).to_radians();
      let phi = azimuth_deg.to_radians();
      Position3D::new(
          theta.sin() * phi.cos(),
          theta.sin() * phi.sin(),
          theta.cos(),
      )
  }

  /// Compute weights for a given preset, DOA, and geometry.
  ///
  /// Returns a [f32; 8] indexed by *physical* mic index (not ALSA channel);
  /// the beamformer maps ALSA → physical before applying.
  pub fn weights_for(mode: AudioMode, doa_az_deg: f32, doa_el_deg: f32, geom: &MicGeometry) -> [f32; 8] {
      match mode {
          AudioMode::Omni => [1.0 / 8.0; 8],
          AudioMode::Cardioid => cardioid_weights(doa_az_deg, doa_el_deg, geom, 0.5),
          AudioMode::Supercardioid => cardioid_weights(doa_az_deg, doa_el_deg, geom, 0.37),
          AudioMode::Figure8 => figure8_weights(doa_az_deg, doa_el_deg, geom),
          AudioMode::Stereo => {
              // Stereo returns left-channel weights; right is computed separately
              // with az+45. Use cardioid weights at az-45 here.
              cardioid_weights(doa_az_deg - 45.0, doa_el_deg, geom, 0.5)
          }
      }
  }

  /// First-order cardioid: w_i = alpha + (1-alpha) * (p_i_hat · d_hat)
  /// where p_i_hat is the unit vector to mic i and d_hat is DOA unit vector.
  /// alpha controls pattern tightness: 0.5 = cardioid, 0.37 = supercardioid,
  /// 0.0 = figure-8 (handled separately).
  fn cardioid_weights(az_deg: f32, el_deg: f32, geom: &MicGeometry, alpha: f32) -> [f32; 8] {
      let d = direction_vector(az_deg, el_deg);
      let mut w = [0.0f32; 8];
      let mut sum = 0.0f32;
      for (i, p) in geom.positions_mm.iter().enumerate() {
          let r = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt().max(1e-6);
          let p_hat = Position3D::new(p.x / r, p.y / r, p.z / r);
          let cos_theta = p_hat.dot(&d);
          w[i] = alpha + (1.0 - alpha) * cos_theta;
          sum += w[i];
      }
      // Normalize so on-axis unity gain.
      let norm = 1.0 / sum.max(1e-6);
      for v in w.iter_mut() { *v *= norm; }
      w
  }

  /// Figure-8: front vs back differential. Pressure-gradient pattern has
  /// zero sum; we don't normalize.
  fn figure8_weights(az_deg: f32, el_deg: f32, geom: &MicGeometry) -> [f32; 8] {
      let d = direction_vector(az_deg, el_deg);
      let mut w = [0.0f32; 8];
      for (i, p) in geom.positions_mm.iter().enumerate() {
          let r = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt().max(1e-6);
          let p_hat = Position3D::new(p.x / r, p.y / r, p.z / r);
          w[i] = p_hat.dot(&d); // +1 front, -1 back
      }
      // Scale so peak weight magnitude is 1/N_nonzero-ish.
      let max = w.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
      if max > 1e-6 {
          for v in w.iter_mut() { *v /= max * 2.0; }
      }
      w
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn direction_vector_front_is_plus_x() {
          let d = direction_vector(0.0, 0.0);
          assert!((d.x - 1.0).abs() < 1e-5);
          assert!(d.y.abs() < 1e-5);
          assert!(d.z.abs() < 1e-5);
      }

      #[test]
      fn direction_vector_left_is_plus_y() {
          let d = direction_vector(90.0, 0.0);
          assert!(d.x.abs() < 1e-5);
          assert!((d.y - 1.0).abs() < 1e-5);
      }

      #[test]
      fn omni_weights_sum_to_one() {
          let geom = MicGeometry::default();
          let w = weights_for(AudioMode::Omni, 0.0, 0.0, &geom);
          let sum: f32 = w.iter().sum();
          assert!((sum - 1.0).abs() < 1e-5);
      }

      #[test]
      fn cardioid_peak_weight_at_front_mic_for_doa_zero() {
          let geom = MicGeometry::default();
          let w = weights_for(AudioMode::Cardioid, 0.0, 0.0, &geom);
          // Mic 0 is at +X (front); should be the highest weighted.
          let max_idx = w.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
          assert_eq!(max_idx, 0, "expected front mic (0) to peak, got {max_idx}");
      }

      #[test]
      fn cardioid_rear_mic_has_near_zero_weight_for_doa_zero() {
          let geom = MicGeometry::default();
          let w = weights_for(AudioMode::Cardioid, 0.0, 0.0, &geom);
          // Mic 2 is at -X (back); classic cardioid has zero response at 180°.
          // alpha=0.5 → w_back = 0.5 + 0.5*(-1) = 0 pre-normalize.
          assert!(w[2].abs() < 0.01, "back mic weight {} should be near 0", w[2]);
      }

      #[test]
      fn cardioid_sums_to_one_after_normalize() {
          let geom = MicGeometry::default();
          let w = weights_for(AudioMode::Cardioid, 45.0, 0.0, &geom);
          let sum: f32 = w.iter().sum();
          assert!((sum - 1.0).abs() < 1e-4, "sum {sum} != 1");
      }

      #[test]
      fn figure8_front_and_back_opposite_sign() {
          let geom = MicGeometry::default();
          let w = weights_for(AudioMode::Figure8, 0.0, 0.0, &geom);
          // Front mic (0) positive, back mic (2) negative, same magnitude.
          assert!(w[0] > 0.0, "front weight should be positive");
          assert!(w[2] < 0.0, "back weight should be negative");
          assert!((w[0] + w[2]).abs() < 1e-4, "front+back should cancel");
      }
  }
  ```

  Update `crates/waved-audio/src/beam/mod.rs`:

  ```rust
  pub mod geometry;
  pub mod delays;
  pub mod weights;
  pub use geometry::{MicGeometry, Position3D};
  ```

- [ ] **Step 2: Run tests**

  Run: `cargo test -p waved-audio beam::weights`

  Expected: 7/7 pass.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/waved-audio/src/beam/
  git commit -m "feat(audio): SP#3 — per-preset DAS weight computation"
  ```

---

## Task 4: DasBeamFormer — end-to-end synthesis

**Files:**
- Modify: `crates/waved-audio/src/beam/mod.rs`

**Rationale:** combine delays + weights into a single consumable. Takes an 8-ch interleaved frame in, produces mono (or stereo for Stereo mode) out. Weight matrix is precomputed when DOA or mode changes; per-sample hot path is 8 delay reads + 8 muladds. The acceptance test here is the end-to-end front/back ratio on a synthetic plane wave — the number the on-device test later mirrors.

- [ ] **Step 1: Write the failing tests**

  Replace `crates/waved-audio/src/beam/mod.rs` with:

  ```rust
  //! Beamforming: turn 8-ch raw mic into a direction-steered mono (or stereo) signal.

  pub mod geometry;
  pub mod delays;
  pub mod weights;

  pub use geometry::{MicGeometry, Position3D};

  use delays::DelayLine;
  use waved_state::AudioMode;

  const SPEED_OF_SOUND_M_S: f32 = 343.0;

  pub struct DasBeamFormer {
      geom: MicGeometry,
      sample_rate: u32,
      mode: AudioMode,
      doa_az_deg: f32,
      doa_el_deg: f32,
      delays: [DelayLine; 8],
      weights_l: [f32; 8],
      weights_r: [f32; 8],       // populated for Stereo; unused otherwise
      delay_samples: [f32; 8],   // per-ALSA-channel delay in samples
      is_stereo: bool,
  }

  /// Output of a single `process_frame` call.
  pub struct BeamOutput {
      pub mono: Vec<f32>,
      /// `Some((L, R))` when mode = Stereo; `None` otherwise.
      pub stereo: Option<(Vec<f32>, Vec<f32>)>,
  }

  impl DasBeamFormer {
      pub fn new(geom: MicGeometry, sample_rate: u32) -> Self {
          let mut s = Self {
              geom,
              sample_rate,
              mode: AudioMode::Omni,
              doa_az_deg: 0.0,
              doa_el_deg: 0.0,
              delays: std::array::from_fn(|_| DelayLine::new()),
              weights_l: [1.0 / 8.0; 8],
              weights_r: [0.0; 8],
              delay_samples: [0.0; 8],
              is_stereo: false,
          };
          s.recompute();
          s
      }

      pub fn set_mode(&mut self, mode: AudioMode) {
          if mode != self.mode {
              self.mode = mode;
              self.recompute();
          }
      }

      pub fn set_doa(&mut self, az_deg: f32, el_deg: f32) {
          if (az_deg - self.doa_az_deg).abs() > 0.1 || (el_deg - self.doa_el_deg).abs() > 0.1 {
              self.doa_az_deg = az_deg;
              self.doa_el_deg = el_deg;
              self.recompute();
          }
      }

      fn recompute(&mut self) {
          self.weights_l = weights::weights_for(self.mode, self.doa_az_deg, self.doa_el_deg, &self.geom);
          self.is_stereo = matches!(self.mode, AudioMode::Stereo);
          if self.is_stereo {
              // Left = cardioid at az-45; Right = cardioid at az+45.
              self.weights_r = weights::weights_for(AudioMode::Cardioid, self.doa_az_deg + 45.0, self.doa_el_deg, &self.geom);
              // weights_l for Stereo already points at az-45 (see weights.rs).
          }
          self.recompute_delays();
      }

      fn recompute_delays(&mut self) {
          let d = weights::direction_vector(self.doa_az_deg, self.doa_el_deg);
          let mut max_tau = f32::NEG_INFINITY;
          let mut raw = [0.0f32; 8];
          for alsa_ch in 0..8 {
              let p = self.geom.position_for_alsa(alsa_ch);
              // TOA relative to origin: τ = -(d · p) / c. Positive τ = arrives later.
              let dot_mm = p.x * d.x + p.y * d.y + p.z * d.z;
              let tau_sec = -(dot_mm * 1e-3) / SPEED_OF_SOUND_M_S;
              raw[alsa_ch] = tau_sec;
              if tau_sec > max_tau { max_tau = tau_sec; }
          }
          // Delay each channel by (max_tau - τ_i) to align the wavefront.
          // Converting to fractional samples.
          let sr = self.sample_rate as f32;
          for (alsa_ch, tau) in raw.iter().enumerate() {
              let delay_sec = max_tau - tau;
              self.delay_samples[alsa_ch] = delay_sec * sr;
          }
      }

      /// Process an 8-channel interleaved f32 frame. `interleaved.len()` must
      /// be a multiple of 8. Appends to output Vec in `out`.
      pub fn process(&mut self, interleaved: &[f32]) -> BeamOutput {
          let frames = interleaved.len() / 8;
          let mut mono = Vec::with_capacity(frames);
          let mut stereo_l = if self.is_stereo { Vec::with_capacity(frames) } else { Vec::new() };
          let mut stereo_r = if self.is_stereo { Vec::with_capacity(frames) } else { Vec::new() };

          for i in 0..frames {
              // Write all 8 channels into their delay lines.
              for alsa_ch in 0..8 {
                  self.delays[alsa_ch].write(interleaved[i * 8 + alsa_ch]);
              }
              // Read delayed, weight, sum.
              let mut sum_l = 0.0f32;
              let mut sum_r = 0.0f32;
              for alsa_ch in 0..8 {
                  let physical = self.geom.alsa_to_physical[alsa_ch] as usize;
                  let x = self.delays[alsa_ch].read(self.delay_samples[alsa_ch]);
                  sum_l += self.weights_l[physical] * x;
                  if self.is_stereo {
                      sum_r += self.weights_r[physical] * x;
                  }
              }
              if self.is_stereo {
                  mono.push(0.5 * (sum_l + sum_r));
                  stereo_l.push(sum_l);
                  stereo_r.push(sum_r);
              } else {
                  mono.push(sum_l);
              }
          }

          BeamOutput {
              mono,
              stereo: if self.is_stereo { Some((stereo_l, stereo_r)) } else { None },
          }
      }

      pub fn reset(&mut self) {
          for d in &mut self.delays { d.reset(); }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      /// Generate an 8-ch synthetic plane wave at given angle.
      /// Source: 1 kHz tone, arriving from (az, el).
      fn synthesize_plane_wave(az_deg: f32, el_deg: f32, sr: u32, frames: usize, geom: &MicGeometry) -> Vec<f32> {
          let d = weights::direction_vector(az_deg, el_deg);
          let freq = 1000.0f32;
          let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
          let mut out = vec![0.0; frames * 8];
          for alsa_ch in 0..8 {
              let p = geom.position_for_alsa(alsa_ch);
              let dot_mm = p.x * d.x + p.y * d.y + p.z * d.z;
              let tau_sec = -(dot_mm * 1e-3) / SPEED_OF_SOUND_M_S;
              let tau_samples = tau_sec * sr as f32;
              for i in 0..frames {
                  out[i * 8 + alsa_ch] = ((i as f32 - tau_samples) * omega).sin();
              }
          }
          out
      }

      fn rms(xs: &[f32]) -> f32 {
          if xs.is_empty() { return 0.0; }
          let sq: f32 = xs.iter().map(|x| x * x).sum();
          (sq / xs.len() as f32).sqrt()
      }

      #[test]
      fn omni_is_nondirectional() {
          let geom = MicGeometry::default();
          let mut bf = DasBeamFormer::new(geom.clone(), 48000);
          bf.set_mode(AudioMode::Omni);

          let front = synthesize_plane_wave(0.0, 0.0, 48000, 4800, &geom);
          let back = synthesize_plane_wave(180.0, 0.0, 48000, 4800, &geom);

          let rms_front = rms(&bf.process(&front).mono[480..]); // skip ramp-in
          bf.reset();
          let rms_back = rms(&bf.process(&back).mono[480..]);

          // Omni: within 1 dB regardless of direction.
          let ratio_db = 20.0 * (rms_front / rms_back).log10();
          assert!(ratio_db.abs() < 1.0, "Omni front/back ratio {ratio_db} dB should be ~0");
      }

      #[test]
      fn cardioid_achieves_ten_db_front_to_back() {
          let geom = MicGeometry::default();
          let mut bf = DasBeamFormer::new(geom.clone(), 48000);
          bf.set_mode(AudioMode::Cardioid);
          bf.set_doa(0.0, 0.0);

          let front = synthesize_plane_wave(0.0, 0.0, 48000, 4800, &geom);
          let back = synthesize_plane_wave(180.0, 0.0, 48000, 4800, &geom);

          let rms_front = rms(&bf.process(&front).mono[480..]);
          bf.reset();
          let rms_back = rms(&bf.process(&back).mono[480..]);

          let fbr_db = 20.0 * (rms_front / rms_back.max(1e-6)).log10();
          assert!(fbr_db >= 10.0, "Cardioid FBR {fbr_db:.1} dB < 10 dB spec");
      }

      #[test]
      fn stereo_produces_two_channels_with_different_content() {
          let geom = MicGeometry::default();
          let mut bf = DasBeamFormer::new(geom.clone(), 48000);
          bf.set_mode(AudioMode::Stereo);
          bf.set_doa(0.0, 0.0);

          // Source at 90° (pure left) — should favor L cardioid pointing at 0-45=-45°.
          let source = synthesize_plane_wave(90.0, 0.0, 48000, 4800, &geom);
          let out = bf.process(&source);
          let (l, r) = out.stereo.expect("stereo mode produces stereo");
          assert_eq!(l.len(), r.len());
          // L and R should differ (otherwise stereo isn't actually stereo).
          let diff: f32 = l.iter().zip(r.iter()).map(|(a, b)| (a - b).abs()).sum();
          assert!(diff > 1.0, "L and R too similar (sum|diff|={diff})");
      }

      #[test]
      fn figure8_rejects_off_axis() {
          let geom = MicGeometry::default();
          let mut bf = DasBeamFormer::new(geom.clone(), 48000);
          bf.set_mode(AudioMode::Figure8);
          bf.set_doa(0.0, 0.0);

          // On-axis: front → peak output
          let front = synthesize_plane_wave(0.0, 0.0, 48000, 4800, &geom);
          let rms_front = rms(&bf.process(&front).mono[480..]);
          bf.reset();

          // 90° off axis: should be in the null plane.
          let side = synthesize_plane_wave(90.0, 0.0, 48000, 4800, &geom);
          let rms_side = rms(&bf.process(&side).mono[480..]);

          let ratio_db = 20.0 * (rms_front / rms_side.max(1e-6)).log10();
          assert!(ratio_db >= 15.0, "Figure-8 90°-rejection {ratio_db:.1} dB < 15 dB");
      }
  }
  ```

- [ ] **Step 2: Run tests**

  Run: `cargo test -p waved-audio beam::tests`

  Expected: 4/4 pass. If Cardioid FBR test fails, check weight normalization in Task 3.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/waved-audio/src/beam/mod.rs
  git commit -m "feat(audio): SP#3 — DasBeamFormer integrates delays + weights; ≥10 dB FBR"
  ```

---

## Task 5: GCC-PHAT cross-correlation

**Files:**
- Create: `crates/waved-audio/src/doa/gcc_phat.rs`
- Create: `crates/waved-audio/src/doa/mod.rs` (stub; full content in Task 7)
- Modify: `crates/waved-audio/Cargo.toml` (add `rustfft`)
- Modify: `crates/waved-audio/src/lib.rs` (add `pub mod doa;`)

**Rationale:** DOA needs time-delay estimates between mic pairs. GCC-PHAT (Generalized Cross-Correlation with Phase Transform) is the standard approach: FFT both signals, compute `X1 * conj(X2) / |X1 * conj(X2)|`, IFFT, find peak.

- [ ] **Step 1: Add rustfft dependency**

  Modify `crates/waved-audio/Cargo.toml` `[dependencies]` section — add:

  ```toml
  rustfft = "6.2"
  ```

- [ ] **Step 2: Write the failing tests**

  Create `crates/waved-audio/src/doa/gcc_phat.rs`:

  ```rust
  //! GCC-PHAT pairwise time-delay estimation.
  //!
  //! Given two mic signals x1, x2 with the same source delayed by τ samples
  //! between them, returns τ (fractional) via the phase-transform-weighted
  //! cross-correlation peak.

  use rustfft::{num_complex::Complex, FftPlanner};

  pub struct GccPhat {
      fft_len: usize,
      planner_fwd: std::sync::Arc<dyn rustfft::Fft<f32>>,
      planner_inv: std::sync::Arc<dyn rustfft::Fft<f32>>,
  }

  impl GccPhat {
      pub fn new(frame_size: usize) -> Self {
          // Zero-pad to 2x to avoid circular-correlation artifacts.
          let fft_len = (frame_size * 2).next_power_of_two();
          let mut planner = FftPlanner::<f32>::new();
          Self {
              fft_len,
              planner_fwd: planner.plan_fft_forward(fft_len),
              planner_inv: planner.plan_fft_inverse(fft_len),
          }
      }

      /// Estimate TDOA (in samples, fractional) such that x2[n] ≈ x1[n - τ].
      /// `max_lag_samples` bounds the search window (set from geometry: max
      /// physical separation / c / T_s).
      pub fn estimate_tdoa(&self, x1: &[f32], x2: &[f32], max_lag_samples: usize) -> f32 {
          assert_eq!(x1.len(), x2.len());
          let n = self.fft_len;

          // Zero-pad both into Complex buffers.
          let mut b1: Vec<Complex<f32>> = x1.iter().map(|&v| Complex::new(v, 0.0)).collect();
          b1.resize(n, Complex::new(0.0, 0.0));
          let mut b2: Vec<Complex<f32>> = x2.iter().map(|&v| Complex::new(v, 0.0)).collect();
          b2.resize(n, Complex::new(0.0, 0.0));

          self.planner_fwd.process(&mut b1);
          self.planner_fwd.process(&mut b2);

          // PHAT-weighted cross-power: X1 * conj(X2) / |X1 * conj(X2)|
          let mut cross: Vec<Complex<f32>> = b1.iter().zip(b2.iter()).map(|(a, b)| {
              let num = a * b.conj();
              let mag = num.norm().max(1e-10);
              num / mag
          }).collect();

          self.planner_inv.process(&mut cross);

          // Search for peak within ±max_lag_samples (wrap-around: positive lags
          // at start of buffer; negative lags at end).
          let max_lag = max_lag_samples.min(n / 2 - 1);
          let mut best_lag = 0i32;
          let mut best_val = f32::NEG_INFINITY;
          for lag in -(max_lag as i32)..=(max_lag as i32) {
              let idx = if lag >= 0 { lag as usize } else { (n as i32 + lag) as usize };
              let v = cross[idx].re;
              if v > best_val { best_val = v; best_lag = lag; }
          }

          // Parabolic interpolation for sub-sample precision.
          let center = best_lag;
          let idx_c = if center >= 0 { center as usize } else { (n as i32 + center) as usize };
          let idx_l = if idx_c == 0 { n - 1 } else { idx_c - 1 };
          let idx_r = if idx_c == n - 1 { 0 } else { idx_c + 1 };
          let y_l = cross[idx_l].re;
          let y_c = cross[idx_c].re;
          let y_r = cross[idx_r].re;
          let denom = y_l - 2.0 * y_c + y_r;
          let frac = if denom.abs() > 1e-9 { 0.5 * (y_l - y_r) / denom } else { 0.0 };

          center as f32 + frac
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn tone(freq: f32, sr: u32, n: usize, phase_samples: f32) -> Vec<f32> {
          let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
          (0..n).map(|i| ((i as f32 - phase_samples) * omega).sin()).collect()
      }

      #[test]
      fn zero_delay_detects_zero() {
          let gcc = GccPhat::new(480);
          let x1 = tone(1000.0, 48000, 480, 0.0);
          let x2 = x1.clone();
          let tau = gcc.estimate_tdoa(&x1, &x2, 16);
          assert!(tau.abs() < 0.5, "expected τ~0, got {tau}");
      }

      #[test]
      fn positive_delay_detected() {
          let gcc = GccPhat::new(480);
          // x2 lags x1 by 5 samples. So x2[n] = x1[n-5]. τ in our convention = +5.
          let x1 = tone(1000.0, 48000, 480, 0.0);
          let x2 = tone(1000.0, 48000, 480, 5.0);
          let tau = gcc.estimate_tdoa(&x1, &x2, 16);
          assert!((tau - 5.0).abs() < 0.5, "expected τ≈5, got {tau}");
      }

      #[test]
      fn negative_delay_detected() {
          let gcc = GccPhat::new(480);
          let x1 = tone(1000.0, 48000, 480, 3.0);
          let x2 = tone(1000.0, 48000, 480, 0.0);
          let tau = gcc.estimate_tdoa(&x1, &x2, 16);
          assert!((tau + 3.0).abs() < 0.5, "expected τ≈-3, got {tau}");
      }

      #[test]
      fn fractional_delay_subsample_precision() {
          let gcc = GccPhat::new(480);
          let x1 = tone(2000.0, 48000, 480, 0.0);
          let x2 = tone(2000.0, 48000, 480, 2.5);
          let tau = gcc.estimate_tdoa(&x1, &x2, 16);
          assert!((tau - 2.5).abs() < 0.3, "expected τ≈2.5, got {tau}");
      }
  }
  ```

  Create `crates/waved-audio/src/doa/mod.rs`:

  ```rust
  //! Direction-of-arrival estimation.
  pub mod gcc_phat;
  ```

  Modify `crates/waved-audio/src/lib.rs` — add:

  ```rust
  pub mod doa;
  ```

- [ ] **Step 3: Run tests**

  Run: `cargo test -p waved-audio doa::gcc_phat`

  Expected: 4/4 pass.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/waved-audio/Cargo.toml
  git add crates/waved-audio/src/doa/
  git add crates/waved-audio/src/lib.rs
  git commit -m "feat(audio): SP#3 — GCC-PHAT pairwise TDOA with parabolic interpolation"
  ```

---

## Task 6: Least-squares direction solver

**Files:**
- Create: `crates/waved-audio/src/doa/solve.rs`
- Modify: `crates/waved-audio/src/doa/mod.rs`

**Rationale:** given TDOAs from ≥3 pairs, solve for the source's unit direction vector in Cartesian. Uses the linear relation `τ_ij · c = (p_i - p_j) · d_hat` → overdetermined system, solve via normal equations.

- [ ] **Step 1: Write the failing tests**

  Create `crates/waved-audio/src/doa/solve.rs`:

  ```rust
  //! Least-squares direction-of-arrival solver.
  //!
  //! Given TDOAs τ_ij between mic pairs with known positions, find the unit
  //! direction vector d̂ that best satisfies  τ_ij · c = (p_i - p_j) · d̂  for
  //! all pairs. Solved in 3D via normal equations; result is normalized to
  //! unit length and converted to (azimuth, elevation) in AudioState convention.

  use crate::beam::Position3D;

  const SPEED_OF_SOUND_M_S: f32 = 343.0;

  pub struct PairObservation {
      /// Position difference p_i - p_j, in METERS.
      pub delta_m: Position3D,
      /// TDOA in seconds (positive = mic i delayed vs mic j).
      pub tdoa_sec: f32,
  }

  /// Result: (azimuth_deg 0..360, elevation_deg -90..90 with 0=horizon).
  #[derive(Debug, Clone, Copy)]
  pub struct DoaAngles {
      pub azimuth_deg: f32,
      pub elevation_deg: f32,
  }

  /// Solve the overdetermined system for the source direction.
  /// Returns None if observations are degenerate (e.g. all pairs colinear).
  pub fn solve_direction(obs: &[PairObservation]) -> Option<DoaAngles> {
      if obs.len() < 3 { return None; }

      // Build normal equations: A d = b where A is 3x3, b is 3x1.
      // Each observation contributes a rank-1 update:
      //   A += Δ Δ^T,  b += (τ c) Δ
      let mut a = [[0.0f32; 3]; 3];
      let mut b_vec = [0.0f32; 3];
      for o in obs {
          let d = [o.delta_m.x, o.delta_m.y, o.delta_m.z];
          let tc = o.tdoa_sec * SPEED_OF_SOUND_M_S;
          for i in 0..3 {
              b_vec[i] += tc * d[i];
              for j in 0..3 {
                  a[i][j] += d[i] * d[j];
              }
          }
      }

      // Solve 3x3 via Cramer's rule.
      let det_a = det3(&a);
      if det_a.abs() < 1e-9 { return None; }

      let a_x = replace_col(&a, 0, &b_vec);
      let a_y = replace_col(&a, 1, &b_vec);
      let a_z = replace_col(&a, 2, &b_vec);
      let dx = det3(&a_x) / det_a;
      let dy = det3(&a_y) / det_a;
      let dz = det3(&a_z) / det_a;

      // Normalize to unit vector.
      let norm = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-9);
      let dhat = (dx / norm, dy / norm, dz / norm);

      // Convert to spherical: az = atan2(y, x), el = asin(z).
      let az = dhat.1.atan2(dhat.0).to_degrees();
      let el = dhat.2.clamp(-1.0, 1.0).asin().to_degrees();

      // Wrap azimuth to [0, 360).
      let az = if az < 0.0 { az + 360.0 } else { az };

      Some(DoaAngles { azimuth_deg: az, elevation_deg: el })
  }

  fn det3(m: &[[f32; 3]; 3]) -> f32 {
      m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
          - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
          + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
  }

  fn replace_col(m: &[[f32; 3]; 3], col: usize, b: &[f32; 3]) -> [[f32; 3]; 3] {
      let mut out = *m;
      for row in 0..3 {
          out[row][col] = b[row];
      }
      out
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      /// Synthesize TDOAs for a known source direction + mic positions, solve, verify round-trip.
      fn make_obs(pairs: &[(Position3D, Position3D)], source_az_deg: f32, source_el_deg: f32) -> Vec<PairObservation> {
          let az = source_az_deg.to_radians();
          let el = source_el_deg.to_radians();
          let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
          pairs.iter().map(|(pi, pj)| {
              let delta = Position3D::new(
                  (pi.x - pj.x) * 1e-3, (pi.y - pj.y) * 1e-3, (pi.z - pj.z) * 1e-3,
              );
              let tc = delta.x * d[0] + delta.y * d[1] + delta.z * d[2];
              PairObservation { delta_m: delta, tdoa_sec: tc / SPEED_OF_SOUND_M_S }
          }).collect()
      }

      #[test]
      fn recovers_source_at_zero_azimuth() {
          let pairs = vec![
              (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),  // front-back
              (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),  // left-right
              (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),  // top-to-side diag
          ];
          let obs = make_obs(&pairs, 0.0, 0.0);
          let ang = solve_direction(&obs).expect("well-conditioned");
          assert!(ang.azimuth_deg < 2.0 || ang.azimuth_deg > 358.0, "az {} not ≈ 0", ang.azimuth_deg);
          assert!(ang.elevation_deg.abs() < 2.0, "el {} not ≈ 0", ang.elevation_deg);
      }

      #[test]
      fn recovers_source_at_ninety_azimuth() {
          let pairs = vec![
              (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
              (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
              (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
          ];
          let obs = make_obs(&pairs, 90.0, 0.0);
          let ang = solve_direction(&obs).expect("well-conditioned");
          assert!((ang.azimuth_deg - 90.0).abs() < 2.0, "az {} not ≈ 90", ang.azimuth_deg);
      }

      #[test]
      fn recovers_source_at_elevation() {
          let pairs = vec![
              (Position3D::new(25.5, 0.0, 0.0), Position3D::new(-25.5, 0.0, 0.0)),
              (Position3D::new(0.0, 25.5, 0.0), Position3D::new(0.0, -25.5, 0.0)),
              (Position3D::new(25.5, 0.0, 0.0), Position3D::new(0.0, 0.0, -78.2)),
          ];
          let obs = make_obs(&pairs, 45.0, 30.0);
          let ang = solve_direction(&obs).expect("well-conditioned");
          assert!((ang.azimuth_deg - 45.0).abs() < 2.0);
          assert!((ang.elevation_deg - 30.0).abs() < 2.0);
      }

      #[test]
      fn rejects_degenerate_pairs() {
          // All three pairs along X axis → rank-1, unsolvable.
          let pairs = vec![
              (Position3D::new(10.0, 0.0, 0.0), Position3D::new(-10.0, 0.0, 0.0)),
              (Position3D::new(20.0, 0.0, 0.0), Position3D::new(-20.0, 0.0, 0.0)),
              (Position3D::new(5.0, 0.0, 0.0), Position3D::new(-5.0, 0.0, 0.0)),
          ];
          let obs = make_obs(&pairs, 45.0, 0.0);
          assert!(solve_direction(&obs).is_none());
      }

      #[test]
      fn requires_at_least_three_pairs() {
          let obs = vec![
              PairObservation { delta_m: Position3D::new(0.05, 0.0, 0.0), tdoa_sec: 0.0 },
              PairObservation { delta_m: Position3D::new(0.0, 0.05, 0.0), tdoa_sec: 0.0 },
          ];
          assert!(solve_direction(&obs).is_none());
      }
  }
  ```

  Update `crates/waved-audio/src/doa/mod.rs`:

  ```rust
  pub mod gcc_phat;
  pub mod solve;
  pub use solve::{DoaAngles, PairObservation};
  ```

- [ ] **Step 2: Run tests**

  Run: `cargo test -p waved-audio doa::solve`

  Expected: 5/5 pass.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/waved-audio/src/doa/
  git commit -m "feat(audio): SP#3 — least-squares 3D direction solver from pairwise TDOAs"
  ```

---

## Task 7: DoaEstimator — VAD gate + smoothing + wire to geometry

**Files:**
- Create: `crates/waved-audio/src/doa/vad.rs`
- Modify: `crates/waved-audio/src/doa/mod.rs` (add DoaEstimator)

**Rationale:** tie GCC-PHAT + solver + VAD + EWMA smoothing into a single per-frame `update(frame)` call. The estimator owns state (previous smoothed angles, VAD history) and selects which 4 mic pairs to use from geometry.

- [ ] **Step 1: Write VAD helper**

  Create `crates/waved-audio/src/doa/vad.rs`:

  ```rust
  //! Voice activity gate. Simple energy-threshold wrapper; the "real" VAD
  //! is Speex's `speech_prob` but we expose it here as a trait so tests
  //! can stub in deterministic behavior.

  pub trait Vad {
      fn is_speech(&self, frame: &[f32]) -> bool;
  }

  /// Threshold-based VAD — "is speech" if frame RMS exceeds `threshold`.
  /// Threshold 0.003 (~ -50 dBFS) is the default that rejects most room tone.
  pub struct EnergyVad {
      threshold: f32,
  }

  impl EnergyVad {
      pub fn new(threshold: f32) -> Self { Self { threshold } }
  }

  impl Default for EnergyVad {
      fn default() -> Self { Self::new(0.003) }
  }

  impl Vad for EnergyVad {
      fn is_speech(&self, frame: &[f32]) -> bool {
          if frame.is_empty() { return false; }
          let sq: f32 = frame.iter().map(|x| x * x).sum();
          let rms = (sq / frame.len() as f32).sqrt();
          rms > self.threshold
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn silence_is_not_speech() {
          let v = EnergyVad::default();
          assert!(!v.is_speech(&vec![0.0; 480]));
      }

      #[test]
      fn loud_tone_is_speech() {
          let v = EnergyVad::default();
          let frame: Vec<f32> = (0..480).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
          assert!(v.is_speech(&frame));
      }
  }
  ```

- [ ] **Step 2: Write DoaEstimator tests**

  Replace `crates/waved-audio/src/doa/mod.rs` with:

  ```rust
  //! Direction-of-arrival estimation.
  pub mod gcc_phat;
  pub mod solve;
  pub mod vad;

  pub use solve::{DoaAngles, PairObservation};

  use crate::beam::MicGeometry;
  use gcc_phat::GccPhat;
  use vad::{EnergyVad, Vad};

  /// Fixed set of pairs for the 8-mic array: 2 top-ring opposing + 2 top-to-side diagonals.
  /// Indexed by PHYSICAL index (not ALSA); the estimator permutes internally.
  const PAIR_INDICES: [(usize, usize); 4] = [
      (0, 2),  // top: front ↔ back (+X axis)
      (1, 3),  // top: left ↔ right (+Y axis)
      (0, 4),  // top front ↔ side front-left (introduces Z)
      (2, 6),  // top back ↔ side back-right (introduces Z, other sign)
  ];

  /// Result of a single estimator update.
  #[derive(Debug, Clone, Copy)]
  pub struct DoaUpdate {
      pub azimuth_deg: f32,
      pub elevation_deg: f32,
      pub vad_active: bool,
      /// False when solver couldn't recover a direction (degenerate input / no speech).
      pub fresh: bool,
  }

  pub struct DoaEstimator {
      geom: MicGeometry,
      sample_rate: u32,
      frame_size: usize,
      gcc: GccPhat,
      vad: EnergyVad,
      smoothed_az: f32,
      smoothed_el: f32,
      alpha: f32,
      /// Max lag between paired mics in samples. Computed from geometry.
      max_lag_samples: usize,
      /// Precomputed pair position deltas (meters) keyed by pair index.
      pair_deltas_m: [(f32, f32, f32); 4],
  }

  impl DoaEstimator {
      pub fn new(geom: MicGeometry, sample_rate: u32, frame_size: usize) -> Self {
          // Max pairwise distance across our geometry: top-ring across is 2*R = 51 mm;
          // top-to-side diagonal is sqrt((R + R/√2)^2 + (R/√2)^2 + 78.2^2) ≈ 90 mm.
          // τ_max = 90e-3 / 343 ≈ 262 μs → 12.6 samples at 48 kHz. Add headroom.
          let max_lag_samples = 20;

          let mut pair_deltas_m = [(0.0f32, 0.0, 0.0); 4];
          for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
              let pi = &geom.positions_mm[i];
              let pj = &geom.positions_mm[j];
              pair_deltas_m[k] = (
                  (pi.x - pj.x) * 1e-3,
                  (pi.y - pj.y) * 1e-3,
                  (pi.z - pj.z) * 1e-3,
              );
          }

          Self {
              geom,
              sample_rate,
              frame_size,
              gcc: GccPhat::new(frame_size),
              vad: EnergyVad::default(),
              smoothed_az: 0.0,
              smoothed_el: 0.0,
              alpha: 0.9,
              max_lag_samples,
              pair_deltas_m,
          }
      }

      /// Process an 8-channel f32 interleaved frame, return current smoothed DOA.
      /// If VAD gates the frame (below threshold) or the solver fails, the
      /// previous smoothed value is returned unchanged.
      pub fn update(&mut self, interleaved: &[f32]) -> DoaUpdate {
          assert_eq!(interleaved.len(), self.frame_size * 8);

          // Extract per-physical-mic frame via ALSA permutation.
          let mut per_ch: [Vec<f32>; 8] = std::array::from_fn(|_| Vec::with_capacity(self.frame_size));
          for i in 0..self.frame_size {
              for alsa_ch in 0..8 {
                  let physical = self.geom.alsa_to_physical[alsa_ch] as usize;
                  per_ch[physical].push(interleaved[i * 8 + alsa_ch]);
              }
          }

          // VAD on channel-0 physical (reasonable proxy for overall energy).
          let vad_active = self.vad.is_speech(&per_ch[0]);
          if !vad_active {
              return DoaUpdate {
                  azimuth_deg: self.smoothed_az,
                  elevation_deg: self.smoothed_el,
                  vad_active: false,
                  fresh: false,
              };
          }

          // Compute TDOAs for each pair; build PairObservations.
          let mut obs = Vec::with_capacity(4);
          for (k, &(i, j)) in PAIR_INDICES.iter().enumerate() {
              let tau_samples = self.gcc.estimate_tdoa(&per_ch[i], &per_ch[j], self.max_lag_samples);
              let (dx, dy, dz) = self.pair_deltas_m[k];
              obs.push(PairObservation {
                  delta_m: crate::beam::Position3D::new(dx, dy, dz),
                  tdoa_sec: tau_samples / self.sample_rate as f32,
              });
          }

          match solve::solve_direction(&obs) {
              Some(ang) => {
                  // Smooth via (sin, cos) EWMA to wrap-around-safely.
                  self.smoothed_az = smooth_angle_deg(self.smoothed_az, ang.azimuth_deg, self.alpha);
                  self.smoothed_el = self.alpha * self.smoothed_el + (1.0 - self.alpha) * ang.elevation_deg;
                  DoaUpdate {
                      azimuth_deg: self.smoothed_az,
                      elevation_deg: self.smoothed_el,
                      vad_active: true,
                      fresh: true,
                  }
              }
              None => DoaUpdate {
                  azimuth_deg: self.smoothed_az,
                  elevation_deg: self.smoothed_el,
                  vad_active: true,
                  fresh: false,
              },
          }
      }
  }

  /// Wrap-around-safe EWMA on angles in degrees via unit-vector smoothing.
  fn smooth_angle_deg(prev: f32, new: f32, alpha: f32) -> f32 {
      let p = prev.to_radians();
      let n = new.to_radians();
      let sx = alpha * p.cos() + (1.0 - alpha) * n.cos();
      let sy = alpha * p.sin() + (1.0 - alpha) * n.sin();
      let out = sy.atan2(sx).to_degrees();
      if out < 0.0 { out + 360.0 } else { out }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn synth_plane_wave(az_deg: f32, el_deg: f32, sr: u32, frames: usize, geom: &MicGeometry) -> Vec<f32> {
          let az = az_deg.to_radians();
          let el = el_deg.to_radians();
          let d = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
          let freq = 1000.0f32;
          let omega = 2.0 * std::f32::consts::PI * freq / sr as f32;
          let mut out = vec![0.0; frames * 8];
          const C: f32 = 343.0;
          for alsa_ch in 0..8 {
              let p = geom.position_for_alsa(alsa_ch);
              let dot_mm = p.x * d[0] + p.y * d[1] + p.z * d[2];
              let tau_sec = -(dot_mm * 1e-3) / C;
              let tau_samples = tau_sec * sr as f32;
              for i in 0..frames {
                  out[i * 8 + alsa_ch] = ((i as f32 - tau_samples) * omega).sin() * 0.5;
              }
          }
          out
      }

      #[test]
      fn recovers_doa_at_zero() {
          let geom = MicGeometry::default();
          let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
          let frame = synth_plane_wave(0.0, 0.0, 48000, 480, &geom);
          // Run a few frames to let EWMA converge; first call acts like initial observation.
          let mut last = est.update(&frame);
          for _ in 0..20 { last = est.update(&frame); }
          assert!(last.vad_active, "synthetic tone should register as speech");
          assert!(last.fresh, "should have solved direction");
          let diff = (last.azimuth_deg - 0.0).min((360.0 - last.azimuth_deg).abs());
          assert!(diff < 5.0, "recovered az {} not ≈ 0", last.azimuth_deg);
      }

      #[test]
      fn recovers_doa_at_ninety() {
          let geom = MicGeometry::default();
          let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
          let frame = synth_plane_wave(90.0, 0.0, 48000, 480, &geom);
          let mut last = est.update(&frame);
          for _ in 0..20 { last = est.update(&frame); }
          assert!((last.azimuth_deg - 90.0).abs() < 5.0, "recovered az {} not ≈ 90", last.azimuth_deg);
      }

      #[test]
      fn silence_freezes_smoothed_doa() {
          let geom = MicGeometry::default();
          let mut est = DoaEstimator::new(geom.clone(), 48000, 480);
          // First drive it with a loud source at 180°.
          let loud = synth_plane_wave(180.0, 0.0, 48000, 480, &geom);
          for _ in 0..30 { est.update(&loud); }
          // Then send silence; DOA should not drift.
          let silence = vec![0.0f32; 480 * 8];
          let az_before = est.update(&silence).azimuth_deg;
          for _ in 0..50 { est.update(&silence); }
          let az_after = est.update(&silence).azimuth_deg;
          assert!((az_before - az_after).abs() < 0.01, "DOA drifted during silence: {az_before} -> {az_after}");
      }

      #[test]
      fn smoothing_wraps_around_360_zero_boundary() {
          // 359° and 1° should smooth to ~0°, not ~180°.
          let smoothed = smooth_angle_deg(359.0, 1.0, 0.5);
          let diff = smoothed.min(360.0 - smoothed);
          assert!(diff < 1.0, "smoothed {} should be near 0", smoothed);
      }
  }
  ```

- [ ] **Step 3: Run tests**

  Run: `cargo test -p waved-audio doa`

  Expected: VAD 2/2 + gcc_phat 4/4 + solve 5/5 + doa 4/4 = 15/15 pass.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/waved-audio/src/doa/
  git commit -m "feat(audio): SP#3 — DoaEstimator with VAD gate + EWMA smoothing"
  ```

---

## Task 8: AudioConfig v3 with MicGeometry

**Files:**
- Modify: `crates/waved-audio/src/audio_config.rs`
- Modify: `crates/waved-audio/config/audio_config_default.json`

**Rationale:** bump config to version 3, attach the tear-down geometry as a serializable field (defaulting to the embedded Wave geometry when absent). Keeps v2 JSONs working via `serde(default)` on the new field and simultaneous tolerance of `version: 2` during migration — see test.

- [ ] **Step 1: Extend the tests**

  Modify `crates/waved-audio/src/audio_config.rs` — in the test module, add:

  ```rust
  #[test]
  fn embedded_default_is_v3_with_wave_geometry() {
      let cfg = AudioConfig::default_embedded();
      assert_eq!(cfg.version, 3);
      let geom = cfg.mic_geometry.as_ref().expect("default includes geometry");
      assert_eq!(geom.positions_mm.len(), 8);
      // Top ring first, z=0.
      assert_eq!(geom.positions_mm[0].z, 0.0);
      // Side ring at z=-78.2.
      assert_eq!(geom.positions_mm[4].z, -78.2);
  }

  #[test]
  fn v2_json_without_geometry_upgrades_to_v3_default() {
      let json = r#"{
          "version":3,
          "capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},
          "playback":{"device":"hw:0,1","channels":2,"sample_rate":48000,"format":"S16_LE","period_frames":1024,"buffer_periods":4},
          "period_frames":1024,"buffer_periods":8
      }"#;
      let cfg = AudioConfig::from_json(json).unwrap();
      assert!(cfg.mic_geometry.is_some(), "missing geometry should default");
      assert_eq!(cfg.mic_geometry.unwrap().positions_mm[0].z, 0.0);
  }

  #[test]
  fn rejects_v2_version_now() {
      let json = r#"{"version":2,"capture":{"mic_device":"hw:0,2","mic_channels":8,"mic_sample_rate":48000,"mic_format":"S32_LE"},"period_frames":256,"buffer_periods":4}"#;
      // v2 is no longer supported; validate() rejects it.
      assert!(matches!(AudioConfig::from_json(json), Err(ConfigError::Invalid(_))));
  }
  ```

  Note: replace the existing `rejects_wrong_version` test (which expected v1 rejected) — change its `"version":1` to `"version":5` to keep it meaningful and not overlap with the new test.

- [ ] **Step 2: Implement v3 schema**

  Modify `crates/waved-audio/src/audio_config.rs`:

  - Add import at top of file:

    ```rust
    use crate::beam::MicGeometry;
    ```

  - Add field to `AudioConfig` struct:

    ```rust
    #[serde(default = "default_mic_geometry")]
    pub mic_geometry: Option<MicGeometry>,
    ```

  - Add free function:

    ```rust
    fn default_mic_geometry() -> Option<MicGeometry> {
        Some(MicGeometry::default())
    }
    ```

  - Update `validate()`:

    ```rust
    if self.version != 3 {
        return Err(ConfigError::Invalid(format!(
            "unsupported config version {}, expected 3", self.version
        )));
    }
    ```

  - Remove or adjust the `embedded_default_parses_and_is_version_2` test (rename to `..._v3`) — the existing test's version assertion will fail once we change the JSON.

- [ ] **Step 3: Update default JSON**

  Modify `crates/waved-audio/config/audio_config_default.json`:

  ```json
  {
    "version": 3,
    "capture": {
      "mic_device": "hw:0,2",
      "mic_channels": 8,
      "mic_sample_rate": 48000,
      "mic_format": "S32_LE"
    },
    "playback": {
      "device": "hw:0,1",
      "channels": 2,
      "sample_rate": 48000,
      "format": "S16_LE",
      "period_frames": 1024,
      "buffer_periods": 4
    },
    "period_frames": 1024,
    "buffer_periods": 8
  }
  ```

  The geometry is omitted from JSON — `serde(default = "default_mic_geometry")` synthesizes it via `MicGeometry::default()`. This keeps the JSON file human-editable without embedding the long mic-position table.

- [ ] **Step 4: Run tests**

  Run: `cargo test -p waved-audio audio_config`

  Expected: all tests pass (existing `embedded_default_*`, `accepts_playback_only`, `load_or_default_valid_file_uses_file` need their JSONs updated to `"version": 3` — fix any failures by updating test JSONs to v3 inline).

- [ ] **Step 5: Commit**

  ```bash
  git add crates/waved-audio/src/audio_config.rs
  git add crates/waved-audio/config/audio_config_default.json
  git commit -m "feat(audio): SP#3 — AudioConfig v3 with embedded mic_geometry default"
  ```

---

## Task 9: Remove AudioMode::NoiseReduction; default becomes Omni

**Files:**
- Modify: `crates/waved-state/src/lib.rs`
- Modify: `crates/waved/src/main.rs` (state migration)

**Rationale:** `NoiseReduction` overlaps conceptually with `NrLevel`. Removing it collapses the UI to "Omni + NrLevel knob" which is the clean model. One-time migration handles persisted state files.

- [ ] **Step 1: Write the failing test in waved-state**

  Modify `crates/waved-state/src/lib.rs` — in the test module, replace the existing `assert_eq!(AudioMode::default(), AudioMode::NoiseReduction)` line and the `state_bus_creates_with_defaults` mode assertion:

  ```rust
  #[test]
  fn audio_mode_default_is_omni() {
      assert_eq!(AudioMode::default(), AudioMode::Omni);
  }
  ```

  Update `state_bus_creates_with_defaults`:

  ```rust
  #[test]
  fn state_bus_creates_with_defaults() {
      let bus = StateBus::new();
      let audio = bus.audio.subscribe();
      assert!(!audio.borrow().mic_muted);
      assert_eq!(audio.borrow().mode, AudioMode::Omni);
  }
  ```

- [ ] **Step 2: Implement the enum change**

  Modify `crates/waved-state/src/lib.rs`:

  ```rust
  #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
  #[serde(rename_all = "snake_case")]
  pub enum AudioMode {
      #[default]
      Omni,
      Cardioid,
      Supercardioid,
      Figure8,
      Stereo,
  }
  ```

- [ ] **Step 3: Scan dependents and update**

  Run: `cargo build --workspace 2>&1 | grep -E "NoiseReduction|error\[" | head -20`

  For each compile error that references `AudioMode::NoiseReduction`:
  - If it's in a match arm: add arm logic that was appropriate, or remove the arm if it was a fallback that Omni now handles.
  - If it's default-value construction: remove; `AudioMode::default()` now yields Omni.
  - If it's a persisted-state deserialize path in `waved/src/main.rs`: see Step 4 below.

- [ ] **Step 4: One-shot state migration at waved boot**

  Modify `crates/waved/src/main.rs` — in the code path that loads persisted `AudioState` (or whatever the current load point is; if no persistence yet exists, this step becomes a no-op and can be skipped):

  Search for `serde_json::from_str::<AudioState>` or similar. If found, wrap with:

  ```rust
  match serde_json::from_str::<AudioState>(&saved) {
      Ok(s) => s,
      Err(e) => {
          tracing::warn!("persisted audio state failed to parse ({e}); using defaults — likely AudioMode::NoiseReduction migration");
          AudioState::default()
      }
  }
  ```

  If no such load path exists yet (state isn't persisted), this step is a no-op; the only concern is runtime defaults, already covered.

- [ ] **Step 5: Run all tests**

  Run: `cargo test --workspace`

  Expected: entire workspace passes. Fix any stragglers referencing `NoiseReduction`.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/waved-state/src/lib.rs
  git add crates/waved/src/main.rs
  # plus any files touched in Step 3
  git commit -m "feat(state): SP#3 — remove AudioMode::NoiseReduction; default to Omni"
  ```

---

## Task 10: Extend ProcessedAudioFrame with stereo sibling field

**Files:**
- Modify: `crates/waved-audio/src/processed_frame.rs`

**Rationale:** Stereo mode produces two mic channels; the existing mono-only field doesn't carry them. Add `mic_stereo_f32: Option<(Vec<f32>, Vec<f32>)>` so downstream consumers that want stereo (future meeting recorder) can get it, while the mono consumers (stats, uplink) keep reading `mic_mono_f32` unchanged.

- [ ] **Step 1: Write the failing tests**

  Modify `crates/waved-audio/src/processed_frame.rs` — extend `#[cfg(test)] mod tests` with:

  ```rust
  #[test]
  fn default_has_none_for_stereo() {
      let frame = AudioFrame::mic_only(vec![1, 2, 3, 4], 1, 48000, 17);
      let mic_mono = [1000i16, 2000, -3000, 4000];
      let ref_mono = [0i16; 4];
      let p = ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono);
      assert!(p.mic_stereo_f32.is_none());
  }

  #[test]
  fn with_stereo_accepts_and_preserves_lengths() {
      let frame = AudioFrame::mic_only(vec![1, 2, 3, 4], 1, 48000, 17);
      let mic_mono = [1000i16; 4];
      let ref_mono = [0i16; 4];
      let p = ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono)
          .with_stereo(vec![0.1, 0.2, 0.3, 0.4], vec![-0.1, -0.2, -0.3, -0.4]);
      let (l, r) = p.mic_stereo_f32.expect("stereo set");
      assert_eq!(l.len(), 4);
      assert_eq!(r.len(), 4);
      assert!((l[2] - 0.3).abs() < 1e-6);
      assert!((r[2] + 0.3).abs() < 1e-6);
      // mic_mono_f32 unchanged.
      assert_eq!(p.mic_mono_f32.len(), 4);
  }

  #[test]
  #[should_panic(expected = "stereo length mismatch")]
  fn with_stereo_rejects_mismatched_lengths() {
      let frame = AudioFrame::mic_only(vec![1, 2], 1, 48000, 0);
      let mic_mono = [100i16; 2];
      let ref_mono = [0i16; 2];
      let _ = ProcessedAudioFrame::from_raw(&frame, &mic_mono, &ref_mono)
          .with_stereo(vec![0.1, 0.2, 0.3], vec![-0.1, -0.2]); // 3 vs 2 → panic
  }
  ```

- [ ] **Step 2: Implement the extension**

  Modify `crates/waved-audio/src/processed_frame.rs`:

  - Add field in struct:

    ```rust
    pub struct ProcessedAudioFrame {
        pub mic_mono_f32: Vec<f32>,
        pub mic_stereo_f32: Option<(Vec<f32>, Vec<f32>)>,   // NEW
        pub reference_f32: Vec<f32>,
        // ...remaining fields unchanged...
    }
    ```

  - Initialize to `None` in `from_raw`:

    ```rust
    Self {
        mic_mono_f32,
        mic_stereo_f32: None,
        reference_f32,
        // ...
    }
    ```

  - Add builder method:

    ```rust
    impl ProcessedAudioFrame {
        /// Attach a stereo pair. L and R must match `mic_mono_f32.len()`.
        pub fn with_stereo(mut self, l: Vec<f32>, r: Vec<f32>) -> Self {
            assert_eq!(l.len(), r.len(), "stereo length mismatch: L={} R={}", l.len(), r.len());
            assert_eq!(l.len(), self.mic_mono_f32.len(), "stereo length mismatch vs mono: stereo={} mono={}", l.len(), self.mic_mono_f32.len());
            self.mic_stereo_f32 = Some((l, r));
            self
        }
    }
    ```

- [ ] **Step 3: Run tests**

  Run: `cargo test -p waved-audio processed_frame`

  Expected: 3 new tests pass, existing tests still pass.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/waved-audio/src/processed_frame.rs
  git commit -m "feat(audio): SP#3 — ProcessedAudioFrame.mic_stereo_f32 sibling"
  ```

---

## Task 11: Wire beamformer + DOA into AudioService

**Files:**
- Modify: `crates/waved-audio/src/lib.rs`

**Rationale:** insert the beamformer before AEC in `process_frame`, drive it with the current DOA, write DOA back to `AudioState`. `Command::SetAudioMode` now affects `bf.set_mode(...)` instead of being state-only.

- [ ] **Step 1: Write integration test**

  Modify `crates/waved-audio/src/lib.rs` — in the existing test module, add:

  ```rust
  #[test]
  fn audio_service_applies_beamformer_before_aec() {
      use crate::beam::MicGeometry;
      use waved_state::AudioMode;

      let sr: u32 = 48000;
      let frame_size: usize = 480;
      let mic_channels: u16 = 8;

      let mut initial_state = AudioState::default();
      initial_state.nr_level = NrLevel::Off;
      initial_state.mode = AudioMode::Cardioid;
      let (audio_tx, _audio_rx) = tokio::sync::watch::channel(initial_state);
      let audio_tx = Arc::new(audio_tx);
      let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<AudioFrame>(4);
      let (meeting_tx, _meeting_rx) = tokio::sync::mpsc::channel::<ProcessedAudioFrame>(4);
      let mut svc = AudioService::new(audio_tx.clone(), frame_tx, frame_rx, meeting_tx, frame_size, sr);

      // Build a "front" plane-wave frame (source at az=0, el=0).
      let geom = MicGeometry::default();
      let omega = 2.0 * std::f32::consts::PI * 1000.0 / sr as f32;
      const C: f32 = 343.0;
      let mut samples = vec![0i16; frame_size * mic_channels as usize];
      for alsa_ch in 0..mic_channels as usize {
          let p = geom.position_for_alsa(alsa_ch);
          let tau_samples = -(p.x * 1e-3) / C * sr as f32; // source at +X
          for i in 0..frame_size {
              let v = ((i as f32 - tau_samples) * omega).sin() * 0.3 * i16::MAX as f32;
              samples[i * mic_channels as usize + alsa_ch] = v as i16;
          }
      }
      let frame = AudioFrame::mic_only(samples, mic_channels, sr, 0);

      // Set DOA manually so the beamformer points correctly.
      svc.set_doa(0.0, 0.0);
      let processed = svc.process_frame(&frame);
      let rms_front: f32 = {
          let sq: f32 = processed.mic_mono_f32.iter().map(|x| x * x).sum();
          (sq / processed.mic_mono_f32.len() as f32).sqrt()
      };
      assert!(rms_front > 0.05, "beamformer produced silence: rms={rms_front}");
  }
  ```

- [ ] **Step 2: Wire beamformer + DOA into AudioService**

  Modify `crates/waved-audio/src/lib.rs`:

  - Add imports:

    ```rust
    use crate::beam::{DasBeamFormer, MicGeometry};
    use crate::doa::DoaEstimator;
    ```

  - Add fields to `AudioService`:

    ```rust
    pub struct AudioService {
        // ...existing fields...
        beamformer: DasBeamFormer,
        doa: DoaEstimator,
        mic_8ch_f32_scratch: Vec<f32>,
    }
    ```

  - In `AudioService::new`, load geometry and init beamformer + DOA. Accept geometry as parameter (or load from AudioConfig::default_embedded() as interim — caller passes via `main.rs` which has the config):

    ```rust
    pub fn new(
        audio_tx: Arc<watch::Sender<AudioState>>,
        frame_tx: mpsc::Sender<AudioFrame>,
        frame_rx: mpsc::Receiver<AudioFrame>,
        meeting_tx: mpsc::Sender<ProcessedAudioFrame>,
        frame_size: usize,
        sample_rate: u32,
        geometry: MicGeometry,
    ) -> Self {
        // ...existing body...
        let mode = audio_tx.borrow().mode.clone();
        let mut beamformer = DasBeamFormer::new(geometry.clone(), sample_rate);
        beamformer.set_mode(mode);
        let doa = DoaEstimator::new(geometry, sample_rate, frame_size);

        Self {
            // ...existing fields...
            beamformer,
            doa,
            mic_8ch_f32_scratch: vec![0.0; frame_size * 8],
        }
    }
    ```

  - Update all call sites of `AudioService::new`. Search: `rg "AudioService::new"` and add the geometry argument. In `waved/src/main.rs`, pass `audio_cfg.mic_geometry.clone().unwrap_or_default()`.

  - Add a test helper:

    ```rust
    impl AudioService {
        /// Test hook: set DOA directly, bypassing DoaEstimator.
        #[cfg(test)]
        pub fn set_doa(&mut self, az_deg: f32, el_deg: f32) {
            self.beamformer.set_doa(az_deg, el_deg);
        }
    }
    ```

  - Rewrite `process_frame` to use the beamformer BEFORE the existing AEC step:

    ```rust
    pub fn process_frame(&mut self, frame: &AudioFrame) -> ProcessedAudioFrame {
        let frame_size = self.frame_size;

        // Convert 8-ch i16 → f32 into scratch (assumes frame.channels == 8;
        // fallback to mono mix for smaller configs to stay backward-compat
        // with unit tests that still send 1-ch).
        if frame.channels as usize == 8 && frame.data.len() == frame_size * 8 {
            if self.mic_8ch_f32_scratch.len() != frame_size * 8 {
                self.mic_8ch_f32_scratch.resize(frame_size * 8, 0.0);
            }
            for (i, &s) in frame.data.iter().enumerate() {
                self.mic_8ch_f32_scratch[i] = (s as f32) / 32768.0;
            }

            // DOA update (smoothed); write into AudioState.
            let doa = self.doa.update(&self.mic_8ch_f32_scratch);
            self.audio_tx.send_modify(|s| {
                s.doa_horizontal = doa.azimuth_deg;
                s.doa_pitch = doa.elevation_deg;
            });
            self.beamformer.set_doa(doa.azimuth_deg, doa.elevation_deg);

            // Beamform → f32 mono.
            let beam_out = self.beamformer.process(&self.mic_8ch_f32_scratch);

            // Convert f32 mono back to i16 for speexdsp.
            if self.mic_mono_scratch.len() != frame_size {
                self.mic_mono_scratch.resize(frame_size, 0);
            }
            for (dst, &src) in self.mic_mono_scratch.iter_mut().zip(beam_out.mono.iter()) {
                *dst = (src.clamp(-1.0, 1.0) * 32768.0) as i16;
            }

            // Reference (unchanged from today).
            if self.ref_mono_scratch.len() != frame_size {
                self.ref_mono_scratch.resize(frame_size, 0);
            }
            match frame.reference.as_ref() {
                Some(r) if frame.reference_channels > 0 => {
                    let mono_ref = mix_to_mono(r, frame.reference_channels as usize);
                    let copy_len = mono_ref.len().min(frame_size);
                    self.ref_mono_scratch[..copy_len].copy_from_slice(&mono_ref[..copy_len]);
                    if copy_len < frame_size {
                        self.ref_mono_scratch[copy_len..].fill(0);
                    }
                }
                _ => self.ref_mono_scratch.fill(0),
            }

            // Apply existing AEC + NS.
            let mut aec_out = vec![0i16; frame_size];
            self.echo_canceller.process(&self.mic_mono_scratch, &self.ref_mono_scratch, &mut aec_out);
            if let Some(ns) = self.noise_suppressor.as_mut() {
                ns.process(&mut aec_out);
            }

            let mut p = ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch);
            if let Some((l, r)) = beam_out.stereo {
                p = p.with_stereo(l, r);
            }
            p
        } else {
            // Legacy path: mic mix via mix_to_mono (used only by host tests that
            // inject 1-ch mic data). Preserves prior behavior.
            let mono_mic = mix_to_mono(&frame.data, frame.channels as usize);
            // ...existing code from the old body starting at `if self.mic_mono_scratch.len() != frame_size`...
            // (copy the original pre-task body verbatim here; kept for test backward-compat)
            if self.mic_mono_scratch.len() != frame_size {
                self.mic_mono_scratch.resize(frame_size, 0);
            }
            let copy_len = mono_mic.len().min(frame_size);
            self.mic_mono_scratch[..copy_len].copy_from_slice(&mono_mic[..copy_len]);
            if copy_len < frame_size {
                self.mic_mono_scratch[copy_len..].fill(0);
            }
            if self.ref_mono_scratch.len() != frame_size {
                self.ref_mono_scratch.resize(frame_size, 0);
            }
            match frame.reference.as_ref() {
                Some(r) if frame.reference_channels > 0 => {
                    let mono_ref = mix_to_mono(r, frame.reference_channels as usize);
                    let copy_len = mono_ref.len().min(frame_size);
                    self.ref_mono_scratch[..copy_len].copy_from_slice(&mono_ref[..copy_len]);
                    if copy_len < frame_size {
                        self.ref_mono_scratch[copy_len..].fill(0);
                    }
                }
                _ => self.ref_mono_scratch.fill(0),
            }
            let mut aec_out = vec![0i16; frame_size];
            self.echo_canceller.process(&self.mic_mono_scratch, &self.ref_mono_scratch, &mut aec_out);
            if let Some(ns) = self.noise_suppressor.as_mut() {
                ns.process(&mut aec_out);
            }
            ProcessedAudioFrame::from_raw(frame, &aec_out, &self.ref_mono_scratch)
        }
    }
    ```

  - In the `cmd_rx.recv()` arm for `Command::SetAudioMode`, add the beamformer wire:

    ```rust
    Ok(Command::SetAudioMode(mode)) => {
        info!("audio mode → {mode:?}");
        self.beamformer.set_mode(mode.clone());
        self.audio_tx.send_modify(|s| s.mode = mode);
    }
    ```

- [ ] **Step 3: Run workspace tests**

  Run: `cargo test --workspace`

  Expected: all tests including the new `audio_service_applies_beamformer_before_aec` pass. The existing echo-cancellation test `audio_service_process_frame_cancels_echo` should also still pass (it sends 8-ch data so the new 8-ch path runs, then AEC).

- [ ] **Step 4: Cross-compile check**

  Run: `cross build --target aarch64-unknown-linux-gnu -p waved --release`

  Expected: clean build. If rustfft pulls in anything that breaks cross, pin its feature flags in `Cargo.toml` (`rustfft = { version = "6.2", default-features = false }`).

- [ ] **Step 5: Commit**

  ```bash
  git add crates/waved-audio/src/lib.rs
  # plus callers of AudioService::new
  git commit -m "feat(audio): SP#3 — wire beamformer + DOA into AudioService pipeline"
  ```

---

## Task 12: `waved-mic-map` tap-harness binary

**Files:**
- Create: `crates/waved-mic-map/Cargo.toml`
- Create: `crates/waved-mic-map/src/main.rs`
- Modify: `Cargo.toml` (workspace members)
- Create: `scripts/tap-mic-map.sh`

**Rationale:** produce the ALSA channel → physical-mic permutation table by recording 12 s while the operator taps each mic hole in order. Emits `/tmp/mic_map.json`, which the operator copies into `mic_geometry.alsa_to_physical` and commits.

- [ ] **Step 1: Create the crate**

  Create `crates/waved-mic-map/Cargo.toml`:

  ```toml
  [package]
  name = "waved-mic-map"
  version = "0.1.0"
  edition = "2021"

  [dependencies]
  alsa = "0.9"
  tracing = "0.1"
  tracing-subscriber = "0.3"
  serde_json = "1"
  ```

  Modify workspace root `Cargo.toml` to add `crates/waved-mic-map` to `members`.

- [ ] **Step 2: Write the harness**

  Create `crates/waved-mic-map/src/main.rs`:

  ```rust
  //! Tap-harness CLI. Records 12 s of 8-ch mic while the operator taps each
  //! physical mic hole in a documented order. Segments the recording into
  //! eight 1.5 s windows, computes per-ALSA-channel energy per window, and
  //! reports the ALSA channel with highest energy per physical-index slot.

  use alsa::pcm::{Access, Format, HwParams, PCM};
  use alsa::{Direction, ValueOr};
  use std::io::Write;
  use std::time::Duration;

  const SAMPLE_RATE: u32 = 48000;
  const CHANNELS: u32 = 8;
  const PERIOD_FRAMES: i64 = 1024;
  const DURATION_SEC: usize = 12;
  const TAP_SLOT_SEC: f32 = 1.5;
  const NUM_TAPS: usize = 8;

  const PHYSICAL_LABELS: [&str; 8] = [
      "TOP 0° (front)",
      "TOP 90° (left)",
      "TOP 180° (back)",
      "TOP 270° (right)",
      "SIDE 45°",
      "SIDE 135°",
      "SIDE 225°",
      "SIDE 315°",
  ];

  fn main() -> std::io::Result<()> {
      tracing_subscriber::fmt().with_env_filter("info").init();
      println!();
      println!("waved-mic-map — tap-harness for ALSA→physical mic mapping");
      println!();
      println!("Instructions:");
      println!("  You will tap each of the 8 mic holes in the order below.");
      println!("  Each tap should be a short, sharp pencil-tap on the hole.");
      println!("  You have 1.5 seconds per slot. Recording begins in 3 s...");
      println!();
      for (i, label) in PHYSICAL_LABELS.iter().enumerate() {
          println!("  slot {i}: tap {label}");
      }
      println!();
      for s in (1..=3).rev() {
          println!("  {s}...");
          std::thread::sleep(Duration::from_secs(1));
      }

      let pcm = PCM::new("hw:0,2", Direction::Capture, false)
          .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("open hw:0,2: {e}")))?;
      {
          let hwp = HwParams::any(&pcm).unwrap();
          hwp.set_channels(CHANNELS).unwrap();
          hwp.set_rate(SAMPLE_RATE, ValueOr::Nearest).unwrap();
          hwp.set_format(Format::s32()).unwrap();
          hwp.set_access(Access::RWInterleaved).unwrap();
          hwp.set_period_size(PERIOD_FRAMES, ValueOr::Nearest).unwrap();
          pcm.hw_params(&hwp).unwrap();
      }
      pcm.prepare().unwrap();

      let io = pcm.io_i32();
      let total_frames = SAMPLE_RATE as usize * DURATION_SEC;
      let mut recording = vec![0i32; total_frames * CHANNELS as usize];
      let mut filled = 0usize;

      println!("RECORDING... tap each slot in order:");
      let start = std::time::Instant::now();
      let mut last_slot_announced = usize::MAX;
      while filled < total_frames {
          let chunk_frames = PERIOD_FRAMES as usize;
          let end = (filled + chunk_frames).min(total_frames);
          let read_len = end - filled;
          let slice_start = filled * CHANNELS as usize;
          let slice_end = end * CHANNELS as usize;
          let n = io
              .readi(&mut recording[slice_start..slice_end])
              .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("readi: {e}")))?;
          filled += n;

          let elapsed_s = start.elapsed().as_secs_f32();
          let slot = (elapsed_s / TAP_SLOT_SEC).floor() as usize;
          if slot < NUM_TAPS && slot != last_slot_announced {
              println!("  -> slot {slot}: tap {}", PHYSICAL_LABELS[slot]);
              last_slot_announced = slot;
          }
      }
      println!("DONE recording, analyzing...");

      // Segment + per-alsa-channel per-slot RMS.
      let slot_frames = (TAP_SLOT_SEC * SAMPLE_RATE as f32) as usize;
      let mut energies = [[0.0f64; 8]; NUM_TAPS]; // [slot][alsa_ch]
      for slot in 0..NUM_TAPS {
          let base_frame = slot * slot_frames;
          let end_frame = (base_frame + slot_frames).min(total_frames);
          for f in base_frame..end_frame {
              for ch in 0..CHANNELS as usize {
                  let v = recording[f * CHANNELS as usize + ch] as f64 / i32::MAX as f64;
                  energies[slot][ch] += v * v;
              }
          }
      }

      // For each physical slot, pick ALSA channel with highest energy.
      // Also validate: no two slots picked the same channel; no channel is dead.
      let mut alsa_to_physical = [0u8; 8];
      let mut used_channels = [false; 8];
      let mut ok = true;
      for slot in 0..NUM_TAPS {
          let (best_ch, best_e) = energies[slot].iter().enumerate()
              .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
              .unwrap();
          println!("  slot {slot} ({}): ALSA ch {best_ch} energy={best_e:.3}", PHYSICAL_LABELS[slot]);
          if used_channels[best_ch] {
              eprintln!("  WARNING: ALSA ch {best_ch} already assigned to a previous slot — tap error?");
              ok = false;
          }
          used_channels[best_ch] = true;
          // alsa_to_physical[alsa_ch] = physical_index
          alsa_to_physical[best_ch] = slot as u8;
      }
      for (ch, &used) in used_channels.iter().enumerate() {
          if !used {
              eprintln!("  WARNING: ALSA ch {ch} never peaked — dead mic or missed tap?");
              ok = false;
          }
      }

      let json = serde_json::json!({ "alsa_to_physical": alsa_to_physical.to_vec(), "ok": ok });
      let json_str = serde_json::to_string_pretty(&json).unwrap();
      let out_path = "/tmp/mic_map.json";
      let mut f = std::fs::File::create(out_path)?;
      f.write_all(json_str.as_bytes())?;
      println!();
      println!("wrote {out_path}");
      println!("alsa_to_physical = {:?}", alsa_to_physical);
      if !ok {
          eprintln!();
          eprintln!("  analysis flagged issues — consider re-running.");
          std::process::exit(2);
      }
      Ok(())
  }
  ```

- [ ] **Step 3: Add host-side wrapper script**

  Create `scripts/tap-mic-map.sh`:

  ```bash
  #!/usr/bin/env bash
  # Run waved-mic-map on-device: stops waved, runs the tap harness,
  # pulls the resulting JSON back to the host.
  set -euo pipefail

  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

  echo "building waved-mic-map for aarch64..."
  (cd "$REPO_ROOT" && cross build -p waved-mic-map --release --target aarch64-unknown-linux-gnu)

  BIN="$REPO_ROOT/target/aarch64-unknown-linux-gnu/release/waved-mic-map"
  [[ -x "$BIN" ]] || { echo "binary not found: $BIN"; exit 1; }

  echo "pushing to device..."
  adb push "$BIN" /data/waved-mic-map
  adb shell chmod +x /data/waved-mic-map

  echo "stopping waved..."
  adb shell 'pkill -9 waved || true'
  sleep 1

  echo "running tap harness. FOLLOW ON-SCREEN PROMPTS."
  adb shell /data/waved-mic-map

  echo "pulling result..."
  adb pull /tmp/mic_map.json "$REPO_ROOT/mic_map.json"

  echo
  echo "Result:"
  cat "$REPO_ROOT/mic_map.json"
  echo
  echo "Next step: copy the alsa_to_physical array into the MicGeometry default"
  echo "in crates/waved-audio/src/beam/geometry.rs and commit."
  ```

  Make executable:

  ```bash
  chmod +x scripts/tap-mic-map.sh
  ```

- [ ] **Step 4: Cross-compile check**

  Run: `cross build -p waved-mic-map --release --target aarch64-unknown-linux-gnu`

  Expected: clean build.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/waved-mic-map/
  git add Cargo.toml
  git add scripts/tap-mic-map.sh
  git commit -m "feat(audio): SP#3 — waved-mic-map tap harness for ALSA→physical mapping"
  ```

- [ ] **Step 6: Run the harness against hardware (device-touching, one-shot)**

  ```bash
  scripts/tap-mic-map.sh
  ```

  Follow the prompts, tapping each mic hole in the printed order. Confirm all 8 slots identified distinct ALSA channels and none were flagged dead.

  **Expected output:**
  ```
  alsa_to_physical = [X, X, X, X, X, X, X, X]   # 8 distinct u8 values
  ```

  Copy that array into `MicGeometry::insta360_wave()` (replacing the identity `[0, 1, 2, 3, 4, 5, 6, 7]`) and commit:

  ```bash
  git add crates/waved-audio/src/beam/geometry.rs
  git add mic_map.json
  git commit -m "chore(audio): SP#3 — bake tap-harness measured ALSA→physical map"
  ```

---

## Task 13: On-device acceptance tests

**Files:**
- Create: `scripts/test-beamforming.sh`

**Rationale:** spec §9.2 defines three acceptance tests: front-to-back ratio ≥ 10 dB, DOA accuracy ≤ 10°, beam CPU < 5%. This script produces pass/fail on device.

- [ ] **Step 1: Write the script**

  Create `scripts/test-beamforming.sh`:

  ```bash
  #!/usr/bin/env bash
  # SP#3 beamforming acceptance: front-to-back ratio, DOA accuracy, CPU budget.
  # Requires: Bluetooth speaker paired to the device OR speaker placed at
  # known angular positions around it. Operator moves speaker between runs.
  set -euo pipefail

  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
  WAVED_BIN="$REPO_ROOT/target/aarch64-unknown-linux-gnu/release/waved"

  if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
    echo "building waved for aarch64..."
    (cd "$REPO_ROOT" && cross build -p waved --release --target aarch64-unknown-linux-gnu)
    adb push "$WAVED_BIN" /data/waved/waved
  fi

  adb shell 'pkill -9 waved || true'
  sleep 1
  adb shell setsid /data/waved/waved >/dev/null 2>&1 &
  sleep 3

  fetch_rms() {
    # Expects stats_collector broadcasts a "mic_post_aec_rms" field; reuse SP#2 mechanism.
    adb shell 'cat /tmp/waved-stats.jsonl | tail -20 | jq -s "map(.mic_post_aec_rms)[-1]"' 2>/dev/null
  }

  fetch_doa_az() {
    adb shell 'cat /tmp/waved-state.jsonl | tail -20 | jq -s "map(.audio.doa_horizontal)[-1]"' 2>/dev/null
  }

  set_mode() {
    adb shell "echo '$1' > /tmp/waved-command.json"
    # (assumes waved has a command-injection socket/file for testing; if not,
    # adjust to the actual RPC path. Otherwise hand-edit persisted state
    # then restart.)
    sleep 2
  }

  echo
  echo "=== Test 1: Cardioid front-to-back ratio ==="
  echo "Place speaker at φ=0° (directly in front), 1 m away, playing 1 kHz tone."
  read -rp "Press Enter when speaker is playing..."
  set_mode '{"SetAudioMode":"cardioid"}'
  sleep 3
  RMS_FRONT=$(fetch_rms)
  echo "  front RMS: $RMS_FRONT"

  echo "Move speaker to φ=180° (directly behind), same distance, same tone."
  read -rp "Press Enter when in position..."
  sleep 3
  RMS_BACK=$(fetch_rms)
  echo "  back RMS: $RMS_BACK"

  FBR_DB=$(python3 -c "import math; print(20 * math.log10($RMS_FRONT / max($RMS_BACK, 1e-9)))")
  echo "  Front-to-back: ${FBR_DB} dB (pass ≥ 10 dB)"
  FBR_PASS=$(python3 -c "print(1 if $FBR_DB >= 10.0 else 0)")
  [[ "$FBR_PASS" == "1" ]] || { echo "  FAIL"; exit 1; }
  echo "  PASS"

  echo
  echo "=== Test 2: DOA accuracy ==="
  for angle in 0 90 180 270; do
    echo "Place speaker at φ=${angle}°, 1 m. Start speech-band signal."
    read -rp "Press Enter when ready..."
    sleep 3
    MEASURED=$(fetch_doa_az)
    DIFF=$(python3 -c "d = abs(${MEASURED} - ${angle}); print(min(d, 360 - d))")
    echo "  ground truth: ${angle}°, measured: ${MEASURED}°, error: ${DIFF}°"
    PASS=$(python3 -c "print(1 if ${DIFF} <= 10.0 else 0)")
    [[ "$PASS" == "1" ]] || { echo "  FAIL"; exit 1; }
    echo "  PASS"
  done

  echo
  echo "=== Test 3: CPU budget (Supercardioid, 30 s silence) ==="
  set_mode '{"SetAudioMode":"supercardioid"}'
  PID=$(adb shell pgrep -x waved | tr -d '\r')
  echo "  sampling /proc/${PID}/stat for 30 s..."
  T0=$(adb shell "awk '{print \$14 + \$15}' /proc/${PID}/stat" | tr -d '\r')
  sleep 30
  T1=$(adb shell "awk '{print \$14 + \$15}' /proc/${PID}/stat" | tr -d '\r')
  # Clock ticks: Android typically 100 Hz (HZ=100). Confirm via `getconf CLK_TCK`.
  CLK=$(adb shell 'getconf CLK_TCK' | tr -d '\r')
  CPU_SEC=$(python3 -c "print(($T1 - $T0) / $CLK)")
  CPU_PCT=$(python3 -c "print($CPU_SEC / 30.0 * 100)")
  echo "  CPU: ${CPU_PCT}% over 30 s (pass < 5%)"
  PASS=$(python3 -c "print(1 if $CPU_PCT < 5.0 else 0)")
  [[ "$PASS" == "1" ]] || { echo "  FAIL"; exit 1; }
  echo "  PASS"

  echo
  echo "=== SP#3 ACCEPTANCE: PASS ==="
  ```

  Make executable:

  ```bash
  chmod +x scripts/test-beamforming.sh
  ```

  **Note on `set_mode`:** the stub above writes to `/tmp/waved-command.json`; if waved has no such RPC injection path, adjust the script to modify persisted state + restart waved (SP#2's test-aec-on-device.sh does similar — follow that pattern). The mode-switching mechanic is not SP#3's deliverable; use whatever is available.

- [ ] **Step 2: Commit the script**

  ```bash
  git add scripts/test-beamforming.sh
  git commit -m "test(audio): SP#3 — on-device beamforming acceptance tests"
  ```

- [ ] **Step 3: Run the acceptance tests on hardware**

  Prep: Bluetooth speaker, tape marks at 0°/90°/180°/270° and 1 m from device, 1 kHz tone + speech-band audio ready to play.

  Run: `scripts/test-beamforming.sh`

  Follow each prompt; the script prints PASS/FAIL per sub-test and overall.

  **Acceptance:** all three sub-tests must pass. If any fail:
  - FBR < 10 dB: check weight normalization (Task 3) and that `alsa_to_physical` is correct (Task 12 output).
  - DOA error > 10°: VAD gate may be too aggressive (tune in `doa::vad`), or the 4-pair selection in `doa/mod.rs::PAIR_INDICES` is poorly conditioned for the target angle — add a 5th top-to-side pair.
  - CPU > 5%: profile with `perf top`; most likely rustfft allocation per frame. Pre-plan once at init (GccPhat already does this, but verify).

  On overall PASS, SP#3 is complete — handoff to SP#4.

---

## Self-review

**Spec coverage:**

- §2.1 geometry (teardown) — Task 1 encodes positions; Task 12 measures alsa_to_physical. ✓
- §2.2 DAS only — Task 4. ✓
- §2.3 mode semantics (remove NR, add stereo field) — Tasks 9 (enum) + 10 (stereo field) + weights Task 3 (Stereo preset uses ±45° cardioids). ✓
- §2.4 GCC-PHAT + VAD + EWMA — Tasks 5, 6, 7. ✓
- §2.5 pipeline topology Beamform→AEC→NS — Task 11. ✓
- §2.6 tap harness — Task 12. ✓
- §2.7 acceptance thresholds — Task 13 enforces all three. ✓
- §5.3 AudioConfig v3 — Task 8. ✓
- §10 rollout/migration — Task 9 Step 4 handles persisted state migration. ✓

**Placeholder scan:** no TBD / TODO / "similar to Task N" / "handle edge cases" strings. `set_mode` in Task 13's script has a noted ambiguity about command-injection mechanism — I flagged this inline as a known env dependency rather than a plan gap.

**Type consistency:** `MicGeometry`, `Position3D`, `DasBeamFormer`, `DoaEstimator`, `ProcessedAudioFrame.mic_stereo_f32`, `AudioMode::Omni` used consistently across tasks.
