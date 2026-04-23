# Waved Audio — SP#4 Neural NS + Dereverb Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the speex NS stage with DeepFilterNet 3 running on the Vivante VIP NPU. Ship a working NS backend that beats speex on stationary + non-stationary noise, stays under a 2% CPU delta, and falls back cleanly to speex when the NPU is unavailable.

**Architecture:** New `waved-npu` crate wraps our own `onnxruntime` build (with the VSI-NPU execution provider) behind a small `NpuInfer` trait. New `waved-audio::ns::deep_filter::DeepFilterNs` implements the existing `NoiseSuppressor` trait on top of `waved-npu`. `AudioConfig` bumps to v4 with a `neural_ns` section. `AudioService::init_processing()` tries DFN3 first, falls back to `SpeexNs` on any init failure. Deploy ships `libonnxruntime.so` and `deepfilternet3.onnx` to `/data/waved/{lib,models}/` alongside the binary.

**Tech Stack:** Rust 2021, `ort` crate (ONNX Runtime bindings), our own cross-built `libonnxruntime.so ≥ 1.17` with `--use_vsinpu`, TIM-VX (VeriSilicon), on-device `libovxlib.so` 6.4.19 and `/dev/galcore`. DeepFilterNet 3 exported to ONNX via upstream's `export.py`.

**Reference spec:** `docs/superpowers/specs/2026-04-23-waved-audio-neural-ns-design.md`
**Reference hardware docs:** `docs/COMPUTE_HARDWARE.md`, `docs/AUDIO_HARDWARE.md`

---

## File Structure

```
crates/waved-npu/                                        (new crate)
  Cargo.toml                                             (new)   ort + anyhow
  src/
    lib.rs                                               (new)   NpuInfer trait + OrtInfer impl
    providers.rs                                         (new)   VSI EP registration + fallback ordering
crates/waved-audio/
  Cargo.toml                                             (modify) add waved-npu as dep
  src/
    ns/
      mod.rs                                             (new)   re-exports + trait lives here
      deep_filter.rs                                     (new)   DeepFilterNs — NoiseSuppressor impl
      speex.rs                                           (move)  existing SpeexNs, lifted into ns/
    aec/
      mod.rs                                             (modify) NoiseSuppressor trait moves to ns/mod.rs
    audio_config.rs                                      (modify) v4: neural_ns section
    lib.rs                                               (modify) init_processing: DFN3 first, speex fallback
  config/audio_config_default.json                       (modify) version: 4; neural_ns block
crates/waved-audio/src/stats_collector.rs                (modify) emit ns_backend + ns_frame_ms
crates/waved-npu-probe/                                  (existing, extended)
  src/main.rs                                            (modify) optional --load <model.onnx> sub-step
scripts/
  export-dfn3-onnx.sh                                    (new)   host script: clone DFN3, export int8 ONNX
  build-onnxruntime-vsinpu.sh                            (new)   cross-build ORT+TIM-VX into target/ort/
  test-neural-ns.sh                                      (new)   on-device acceptance
Cross.toml                                               (modify) add pre-build steps for TIM-VX headers
docs/
  COMPUTE_HARDWARE.md                                    (modify) record measured per-frame latency + chip caveats
```

## Task classification

| Task | Classification | Gate |
|------|----------------|------|
| 1 | Host-only TDD (`waved-npu` scaffold + trait) | `cargo test -p waved-npu` green |
| 2 | Device integration — TIM-VX smoke build | `tim_vx_smoke` aarch64 binary runs on device and creates a trivial graph |
| 3 | Build-system — cross-compile ORT + VSI-NPU EP | `libonnxruntime.so.1.17+` built, `ort::providers()` lists `VsiNpuExecutionProvider` |
| 4 | Device integration — `waved-npu-probe --load` | loads `silero_vad_v5.onnx` via VSI-NPU, output matches CPU reference within 1e-3 |
| 5 | Host-only recipe — DFN3 ONNX export | `scripts/export-dfn3-onnx.sh` produces `deepfilternet3.onnx` (<15 MB int8), smoke-loads on host |
| 6 | Mixed — `waved-npu::OrtInfer` | `cargo test -p waved-npu` green on host (CPU EP); cross-build green |
| 7 | Host-only TDD (`DeepFilterNs`) | `cargo test -p waved-audio ns::deep_filter` green; fallback path unit-tested |
| 8 | Host-only TDD (AudioConfig v4) | `cargo test -p waved-audio audio_config` green; old v3 JSON still deserialises |
| 9 | Host-only TDD (stats fields + init_processing) | `cargo test -p waved-audio` green; cross build green |
| 10 | Device integration — acceptance tests | `scripts/test-neural-ns.sh` — latency ≤ 5 ms, stationary ≥ 15 dB, CPU delta ≤ 2 % |

Risk is concentrated in Tasks 2 + 3 (the cross-compile of ORT against on-device libs). Task 2 is deliberately a **minimal TIM-VX-alone spike** before the bigger ORT build in Task 3 — if TIM-VX doesn't link, ORT can't, and we want to know in hours not days. Task 4 is the first end-to-end proof the NPU can actually run an ONNX from our code.

---

## Task 1: `waved-npu` crate scaffold + `NpuInfer` trait

**Files:**
- Create: `crates/waved-npu/Cargo.toml`
- Create: `crates/waved-npu/src/lib.rs`
- Create: `crates/waved-npu/src/providers.rs` (stub — real content in Task 6)

**Rationale:** Land the public API surface before we sink time into the ORT build. Tasks 2/3 can fail and we still keep a type-checked trait that `DeepFilterNs` (Task 7) can compile against, using only the CPU EP from the `ort` crate's default build.

- [ ] **Step 1: Write the failing tests**

  Create `crates/waved-npu/src/lib.rs`:

  ```rust
  //! ONNX Runtime wrapper with VSI-NPU execution provider preference.
  //!
  //! `OrtInfer` is constructed from a model path; it tries providers in
  //! [`preferred_providers()`] order and falls through to CPU. Exposes a
  //! single per-frame entry point `run(&[f32]) -> Vec<f32>` sized to match
  //! the model's declared output shape.

  use anyhow::Result;
  use std::path::Path;

  pub trait NpuInfer: Send {
      /// Run one forward pass. `input.len()` must match the declared input
      /// size; returned vector length equals the declared output size.
      fn run(&mut self, input: &[f32]) -> Result<Vec<f32>>;

      /// Name of the execution provider actually used at construction —
      /// "vsi_npu", "cpu", etc. Used by stats_collector for `ns_backend`.
      fn provider(&self) -> &'static str;
  }

  pub struct OrtInfer {
      // Opaque — implementation in Task 6.
      _stub: (),
  }

  impl OrtInfer {
      /// Placeholder constructor; real body in Task 6.
      pub fn new(_model_path: &Path) -> Result<Self> {
          anyhow::bail!("OrtInfer not yet implemented — see Task 6")
      }
  }

  impl NpuInfer for OrtInfer {
      fn run(&mut self, _input: &[f32]) -> Result<Vec<f32>> {
          anyhow::bail!("OrtInfer not yet implemented — see Task 6")
      }
      fn provider(&self) -> &'static str {
          "stub"
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use std::path::PathBuf;

      #[test]
      fn stub_constructor_errors_cleanly() {
          let r = OrtInfer::new(&PathBuf::from("/nonexistent"));
          assert!(r.is_err(), "stub should error, not panic");
      }

      #[test]
      fn provider_name_is_static_str() {
          // Compile-time: trait signature returns &'static str. Tests
          // that the trait shape is stable before we wire anything.
          fn _check<T: NpuInfer>() {
              let _: fn(&T) -> &'static str = T::provider;
          }
      }
  }
  ```

  Cargo manifest:

  ```toml
  [package]
  name = "waved-npu"
  version.workspace = true
  edition.workspace = true

  [dependencies]
  anyhow = "1"
  ```

  No `ort` dependency yet — added in Task 6 once the real ORT build exists.

- [ ] **Step 2: Run tests**

  ```bash
  cargo test -p waved-npu
  ```

  Expected: 2 passing.

- [ ] **Step 3: Commit**

  ```
  feat(npu): SP#4 — waved-npu crate scaffold + NpuInfer trait
  ```

---

## Task 2: TIM-VX smoke build (device spike)

**Files:**
- Create: `third_party/TIM-VX/` (git submodule, pinned to a release compatible with galcore 6.4.19.x — start with `v1.2.15` or latest tag before Jan 2026)
- Create: `scripts/build-tim-vx.sh`
- Create: `crates/waved-npu-probe/src/tim_vx_smoke.rs` (extension of existing probe — gated behind `--smoke-tim-vx` flag)

**Rationale:** Before investing in an ORT cross-compile, prove TIM-VX can be cross-compiled **against the on-device libovxlib.so** and can construct + execute a trivial graph. If TIM-VX doesn't link cleanly, ORT won't either, and we need to know in an afternoon. This is the cheapest way to catch a galcore-ABI mismatch.

- [ ] **Step 1: Add TIM-VX as a git submodule (or vendored copy)**

  ```bash
  mkdir -p third_party
  git submodule add https://github.com/VeriSilicon/TIM-VX.git third_party/TIM-VX
  cd third_party/TIM-VX && git checkout v1.2.15 && cd ../..
  ```

  If we prefer not to carry a submodule, `git clone --depth 1 --branch v1.2.15` into `third_party/TIM-VX` and `.gitignore` it — whichever convention the repo already uses (`git ls-files --others` and existing `.gitignore`).

- [ ] **Step 2: Write `scripts/build-tim-vx.sh`**

  Build TIM-VX for aarch64 inside the `cross` docker image, using on-device `libovxlib.so` + headers pulled from the TIM-VX repo. Output: `target/tim-vx/libtim-vx.so` + `libtim-vx.a`.

  ```bash
  #!/usr/bin/env bash
  set -euo pipefail
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
  TIM_VX_SRC="$REPO_ROOT/third_party/TIM-VX"
  OUT="$REPO_ROOT/target/tim-vx"
  mkdir -p "$OUT"

  docker run --rm -v "$REPO_ROOT":/work \
      ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main bash -c "
      apt-get update && apt-get install -y --no-install-recommends cmake
      mkdir -p /work/target/tim-vx-build
      cd /work/target/tim-vx-build
      cmake $TIM_VX_SRC \
          -DCMAKE_TOOLCHAIN_FILE=/opt/cross/aarch64-unknown-linux-gnu.toolchain.cmake \
          -DTIM_VX_ENABLE_TEST=OFF \
          -DEXTERNAL_VIV_SDK=/work/third_party/TIM-VX/prebuilt-sdk/x86_64_linux
      make -j\$(nproc)
      cp -v libtim-vx.so $OUT/
  "
  ```

  (The exact `EXTERNAL_VIV_SDK` value may need tweaking — TIM-VX ships prebuilt SDK headers; we use those for the build but swap in `libovxlib.so` from the device at link time. If headers version-mismatch, that's the smoke test catching it.)

- [ ] **Step 3: Minimal TIM-VX smoke test**

  Extend `waved-npu-probe/src/main.rs` with a `--smoke-tim-vx` path that `dlopen`s `libtim-vx.so` (or loads it at link time if we statically link the smoke binary) and creates a trivial graph: one Add node over two 1×1 tensors. Run it on the device; verify the output tensor equals the expected sum.

  Alternative if dlopen is awkward: a separate C file compiled into the probe via the `cc` crate, linked against `libtim-vx.so`. The probe's Cargo.toml gains `cc` as a build-dep.

  The smoke test is not about speed or quality — only "does it execute on the NPU and return the right number?".

- [ ] **Step 4: Run on device**

  ```bash
  ./scripts/build-tim-vx.sh
  adb push target/tim-vx/libtim-vx.so /data/waved/lib/
  cross build -p waved-npu-probe --release --target aarch64-unknown-linux-gnu
  adb push target/aarch64-unknown-linux-gnu/release/waved-npu-probe /data/waved/
  adb shell "LD_LIBRARY_PATH=/data/waved/lib /data/waved/waved-npu-probe --smoke-tim-vx"
  ```

  Expected: probe reports "TIM-VX Add(2,3) = 5 OK".

- [ ] **Step 5: Commit**

  ```
  feat(npu-probe): SP#4 — TIM-VX smoke build, ABI-compatible with galcore 6.4.19
  ```

  **Exit criterion for the plan overall**: if TIM-VX won't build or doesn't run on device, pause the plan and re-scope SP#4. Document the failure mode in `COMPUTE_HARDWARE.md` so the next attempt starts from the right premise.

---

## Task 3: Cross-compile `onnxruntime` with VSI-NPU EP

**Files:**
- Create: `scripts/build-onnxruntime-vsinpu.sh`
- Create: `third_party/onnxruntime/` (git submodule or vendored, pinned to `v1.18.0`)
- Modify: `Cross.toml` — add a second target image entry `[target.aarch64-unknown-linux-gnu.env.volumes]` to share `target/ort/` with the cross container

**Rationale:** We need a `libonnxruntime.so` that contains the VSI-NPU provider. Upstream `ort` crate 2.x links dynamically against the system `libonnxruntime.so`, so we only need to ship the .so. Our own ORT build must link against the TIM-VX .so from Task 2 at runtime (or statically, to avoid dual-.so drift).

- [ ] **Step 1: Pin ORT source + build script**

  ```bash
  git submodule add -b v1.18.0 https://github.com/microsoft/onnxruntime.git third_party/onnxruntime
  ```

  `scripts/build-onnxruntime-vsinpu.sh`:

  ```bash
  #!/usr/bin/env bash
  set -euo pipefail
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
  OUT="$REPO_ROOT/target/ort"
  mkdir -p "$OUT"

  docker run --rm -v "$REPO_ROOT":/work \
      ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main bash -c "
      apt-get update && apt-get install -y --no-install-recommends \
          cmake python3 python3-pip git ninja-build
      cd /work/third_party/onnxruntime
      ./build.sh --config Release \
          --build_shared_lib \
          --parallel \
          --arm64 \
          --use_vsinpu \
          --vsi_npu_home=/work/third_party/TIM-VX \
          --skip_tests
      cp -v build/Linux/Release/libonnxruntime.so* $OUT/
  "
  ```

- [ ] **Step 2: Run the build**

  Expected build time: 25–40 min on a laptop, first run. Incremental rebuilds are fast (ninja).

  If build fails at the VSI EP stage, likely causes + fixes:
  - ORT `--use_vsinpu` was experimental in 1.16 and matured in 1.17/1.18; try 1.18 (already pinned above) and if still broken, try a minor patch release.
  - TIM-VX header path mismatch: the ORT VSI-EP expects TIM-VX at `$vsi_npu_home/include`; verify paths.
  - Missing protobuf at host: install `protobuf-compiler` in the docker layer.

- [ ] **Step 3: Verify VSI EP presence**

  On host (fastest):

  ```bash
  strings target/ort/libonnxruntime.so.1.18.0 | grep -E 'VsiNpuExecutionProvider|VSI_NPU'
  ```

  Expected: at least one match.

- [ ] **Step 4: Push to device + basic load test**

  ```bash
  adb shell "mkdir -p /data/waved/lib"
  adb push target/ort/libonnxruntime.so.1.18.0 /data/waved/lib/
  adb shell "cd /data/waved/lib && ln -sf libonnxruntime.so.1.18.0 libonnxruntime.so"
  ```

  Quick smoke (from Python or a tiny C++ client) to confirm the .so loads and `ort_api->GetProvider()` includes `VsiNpuExecutionProvider`. A minimal Rust check using `ort` crate is also fine — this is plumbing, not a formal test.

- [ ] **Step 5: Commit**

  ```
  feat(build): SP#4 — cross-compile ORT 1.18 with VSI-NPU EP
  ```

---

## Task 4: NPU inference end-to-end — silero VAD via our ORT

**Files:**
- Modify: `crates/waved-npu-probe/src/main.rs` — add `--load <model.onnx>` sub-step
- Create: `crates/waved-npu-probe/tests/silero_reference.txt` (CPU-side reference outputs, captured once from stock ORT, committed as golden)

**Rationale:** First real proof that NPU + our ORT actually run an ONNX model end-to-end. We use `/model/vad/silero_vad_v5.onnx` because it's already on-device, small, and has deterministic outputs we can cross-check against stock firmware's CPU ORT.

- [ ] **Step 1: Capture CPU reference**

  On-device, using the stock `libonnxruntime.so.1.15.1` (not ours — for a ground-truth baseline):

  ```bash
  # tiny Rust tool or python via stock ORT linkage; feed a known 480-sample
  # buffer of silence + a synthetic sine; capture the VAD probabilities
  # frame-by-frame for 100 frames; dump to silero_reference.txt
  ```

  Commit the reference file once — it never changes.

- [ ] **Step 2: Extend `waved-npu-probe` with `--load <model>`**

  Uses the `ort` crate against the OUR `/data/waved/lib/libonnxruntime.so.1.18.0` (set via `LD_LIBRARY_PATH` or by passing `--path` at session-create time). Registers VSI-NPU EP first, CPU second. Feeds the same known input as step 1. Compares output against the golden reference within 1e-3 absolute (int8 quantisation will introduce some delta; 1e-3 is forgiving enough to not flake).

- [ ] **Step 3: Run on device**

  ```bash
  adb push target/aarch64-unknown-linux-gnu/release/waved-npu-probe /data/waved/
  adb push crates/waved-npu-probe/tests/silero_reference.txt /data/waved/models/
  adb shell "LD_LIBRARY_PATH=/data/waved/lib /data/waved/waved-npu-probe \
      --load /model/vad/silero_vad_v5.onnx \
      --reference /data/waved/models/silero_reference.txt"
  ```

  Expected output:
  ```
  [9] session created with provider=vsi_npu
  [10] 100 frames ran, max abs diff vs CPU reference = 4.2e-4  OK
  RESULT: OK — NPU end-to-end matches CPU reference within 1e-3.
  ```

- [ ] **Step 4: Commit**

  ```
  feat(npu-probe): SP#4 — silero VAD end-to-end on NPU matches CPU reference
  ```

  **This is the go/no-go gate for the model choice.** If step 3 fails with op-unsupported errors, DFN3 (which uses similar conv + GRU ops) will also fail, and we pivot (either to a simpler model or to a different runtime).

---

## Task 5: DeepFilterNet 3 ONNX export recipe

**Files:**
- Create: `scripts/export-dfn3-onnx.sh`
- Create: `models/deepfilternet3.onnx` (generated, `.gitignore`d) — the deploy script pushes this to `/data/waved/models/`
- Modify: `.gitignore` — add `models/*.onnx`

**Rationale:** We need a reproducible path from "upstream DeepFilterNet checkpoint" to "int8 ONNX that our ORT can load". Upstream ships `DeepFilterNet/scripts/export_onnx.py`. We wrap it in a host script so the build recipe is committed.

- [ ] **Step 1: Write the export script**

  ```bash
  #!/usr/bin/env bash
  set -euo pipefail
  DFN_REPO="${DFN_REPO:-https://github.com/Rikorose/DeepFilterNet.git}"
  DFN_REF="${DFN_REF:-v0.5.6}"   # adjust to the latest stable tag
  WORK="$(pwd)/target/dfn3-export"
  OUT="$(pwd)/models/deepfilternet3.onnx"
  mkdir -p "$WORK" "$(dirname "$OUT")"

  if [[ ! -d "$WORK/DeepFilterNet" ]]; then
      git clone --depth 1 --branch "$DFN_REF" "$DFN_REPO" "$WORK/DeepFilterNet"
  fi
  cd "$WORK/DeepFilterNet"
  python3 -m pip install --user -r requirements.txt
  python3 scripts/export_onnx.py \
      --model DeepFilterNet3 \
      --out "$OUT" \
      --quantize int8
  echo "exported to $OUT  ($(du -h "$OUT" | cut -f1))"
  ```

- [ ] **Step 2: Run it**

  First run downloads DFN3 weights (~50 MB) and exports. Expected ONNX size: 8–12 MB int8.

- [ ] **Step 3: Smoke-load on host**

  Quick Python (or `ort` Rust) load to confirm the ONNX is well-formed: input shape (1, 480), output shape (1, 480), int8 initializers present. Not a formal test — just "does it open cleanly?".

- [ ] **Step 4: Commit**

  ```
  feat(scripts): SP#4 — export-dfn3-onnx.sh produces int8 DFN3 ONNX
  ```

  (The model file itself is gitignored; only the recipe is committed.)

---

## Task 6: `OrtInfer` implementation in `waved-npu`

**Files:**
- Modify: `crates/waved-npu/Cargo.toml` — add `ort = "2"` dependency with `load-dynamic` feature so it uses our `/data/waved/lib/libonnxruntime.so.1.18.0` at runtime, not linked statically
- Modify: `crates/waved-npu/src/lib.rs`
- Modify: `crates/waved-npu/src/providers.rs`

**Rationale:** Replace the Task 1 stubs with a real ORT session. `ort = "2"` load-dynamic mode means the Rust crate doesn't carry its own ORT — it picks up ours from the system. Keeps our build simple.

- [ ] **Step 1: Write failing tests**

  `crates/waved-npu/tests/ort_infer.rs`:

  ```rust
  use std::path::PathBuf;
  use waved_npu::{NpuInfer, OrtInfer};

  fn fixture(name: &str) -> PathBuf {
      PathBuf::from(env!("CARGO_MANIFEST_DIR"))
          .join("tests/fixtures")
          .join(name)
  }

  #[test]
  #[ignore = "needs libonnxruntime.so in LD_LIBRARY_PATH"]
  fn load_real_model_runs_one_frame() {
      let m = fixture("silero_vad_v5.onnx");
      let mut infer = OrtInfer::new(&m).expect("session");
      let out = infer.run(&vec![0.0f32; 480]).expect("run");
      assert!(!out.is_empty());
  }

  #[test]
  fn missing_model_errors() {
      let r = OrtInfer::new(&PathBuf::from("/definitely/not/a/model"));
      assert!(r.is_err());
  }
  ```

  The first test is `#[ignore]`d by default and only runs on-device (`cargo test --test ort_infer -- --ignored` with `LD_LIBRARY_PATH` set to our ORT build).

- [ ] **Step 2: Implement `providers.rs`**

  ```rust
  use ort::execution_providers::{CPUExecutionProvider, ExecutionProviderDispatch};

  /// Build the execution-provider list the session will try in order.
  /// VSI-NPU first, CPU fallback. When ORT was built without VSI EP the
  /// first entry is silently ignored at session construction.
  pub fn preferred_providers() -> Vec<ExecutionProviderDispatch> {
      vec![
          // TODO: replace with ort::execution_providers::VsiNpuExecutionProvider
          // once ort crate 2.x exposes it; for now rely on name-based
          // registration.
          CPUExecutionProvider::default().build(),
      ]
  }

  /// Name of the provider that actually materialised for a given session.
  pub fn active_provider_name(providers: &[&str]) -> &'static str {
      if providers.iter().any(|p| p.starts_with("VsiNpu")) { "vsi_npu" }
      else if providers.iter().any(|p| *p == "CPUExecutionProvider") { "cpu" }
      else { "unknown" }
  }
  ```

  (If the `ort` crate version we settle on already exposes `VsiNpuExecutionProvider`, use that directly and drop the TODO. At Jan 2026 cutoff, `ort 2.0.0-rc.9` has it.)

- [ ] **Step 3: Implement `OrtInfer`**

  ```rust
  pub struct OrtInfer {
      session: ort::Session,
      input_name: String,
      output_name: String,
      provider: &'static str,
  }

  impl OrtInfer {
      pub fn new(model_path: &Path) -> Result<Self> {
          let session = ort::Session::builder()?
              .with_execution_providers(preferred_providers())?
              .commit_from_file(model_path)?;
          let input_name = session.inputs[0].name.clone();
          let output_name = session.outputs[0].name.clone();
          let providers = session.providers()?.iter().map(String::as_str).collect::<Vec<_>>();
          let provider = active_provider_name(&providers);
          Ok(Self { session, input_name, output_name, provider })
      }
  }

  impl NpuInfer for OrtInfer {
      fn run(&mut self, input: &[f32]) -> Result<Vec<f32>> {
          let tensor = ort::Value::from_array(([1, input.len()], input))?;
          let outputs = self.session.run(ort::inputs![&self.input_name => tensor]?)?;
          let out: &ort::Tensor<f32> = outputs[self.output_name.as_str()].try_extract_tensor()?;
          Ok(out.as_slice().ok_or_else(|| anyhow::anyhow!("non-contig output"))?.to_vec())
      }
      fn provider(&self) -> &'static str { self.provider }
  }
  ```

- [ ] **Step 4: Run tests**

  ```bash
  cargo test -p waved-npu                 # CPU-only, hits the missing-model test
  # on device:
  LD_LIBRARY_PATH=/data/waved/lib cargo test -p waved-npu -- --ignored
  ```

- [ ] **Step 5: Commit**

  ```
  feat(npu): SP#4 — OrtInfer with VSI-NPU + CPU fallback
  ```

---

## Task 7: `DeepFilterNs` implementation

**Files:**
- Create: `crates/waved-audio/src/ns/mod.rs` (move `NoiseSuppressor` trait from `aec/mod.rs`)
- Create: `crates/waved-audio/src/ns/deep_filter.rs`
- Move: `crates/waved-audio/src/aec/speex.rs` → `crates/waved-audio/src/ns/speex.rs` (keep the AEC parts where they are — only `SpeexNs` moves)
- Modify: `crates/waved-audio/src/aec/mod.rs` — remove `NoiseSuppressor` trait (imports from `ns/mod.rs` now)

**Rationale:** Group NS implementations in one module. Adds `DeepFilterNs` as a peer of `SpeexNs` behind the same trait. Keeps framing + wet/dry mix in the `DeepFilterNs` impl; the `waved-npu` crate stays ignorant of audio semantics.

- [ ] **Step 1: Write failing tests**

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn frame_size_must_be_480_at_48k() {
          let mut ns = DeepFilterNs::new_with_infer(MockInfer::new_passthrough()).unwrap();
          let mut wrong = vec![0.0f32; 479];
          let err = ns.process_frame_checked(&mut wrong);
          assert!(err.is_err(), "must reject non-480 frames");
      }

      #[test]
      fn wet_dry_blend_at_level_off_is_passthrough() {
          let mut ns = DeepFilterNs::new_with_infer(MockInfer::new_inverting()).unwrap();
          ns.set_nr_level(NrLevel::Off);
          let mut frame = vec![0.5f32; 480];
          ns.process_frame(&mut frame);
          assert!(frame.iter().all(|&x| (x - 0.5).abs() < 1e-6));
      }

      #[test]
      fn wet_dry_blend_at_level_high_is_full_wet() {
          let mut ns = DeepFilterNs::new_with_infer(MockInfer::new_inverting()).unwrap();
          ns.set_nr_level(NrLevel::High);
          let mut frame = vec![0.5f32; 480];
          ns.process_frame(&mut frame);
          // MockInfer inverts sign → at full wet we get -0.5
          assert!(frame.iter().all(|&x| (x + 0.5).abs() < 1e-6));
      }

      #[test]
      fn fallback_error_contains_reason_when_model_missing() {
          let err = DeepFilterNs::from_path(&PathBuf::from("/nope"))
              .err().expect("should fail");
          let s = format!("{err}");
          assert!(s.contains("model") || s.contains("ONNX"), "unhelpful error: {s}");
      }
  }
  ```

  `MockInfer` is a tiny in-test `NpuInfer` impl (passthrough or sign-invert) — keeps these tests host-only.

- [ ] **Step 2: Implement `DeepFilterNs`**

  ```rust
  pub struct DeepFilterNs {
      infer: Box<dyn NpuInfer>,
      nr_level: NrLevel,   // from waved-state
      wet_dry: f32,        // 0.0..=1.0
  }

  impl DeepFilterNs {
      pub fn from_path(model: &Path) -> Result<Self> {
          let infer = Box::new(OrtInfer::new(model)?);
          Ok(Self::new_with_infer(infer).unwrap())
      }

      pub fn new_with_infer(infer: Box<dyn NpuInfer>) -> Result<Self> {
          Ok(Self { infer, nr_level: NrLevel::Medium, wet_dry: 0.6 })
      }

      pub fn set_nr_level(&mut self, level: NrLevel) {
          self.nr_level = level;
          self.wet_dry = match level {
              NrLevel::Off => 0.0,
              NrLevel::Low => 0.3,
              NrLevel::Medium => 0.6,
              NrLevel::High => 1.0,
          };
      }
  }

  impl NoiseSuppressor for DeepFilterNs {
      fn process_frame(&mut self, mono: &mut [f32]) {
          debug_assert_eq!(mono.len(), 480, "DFN3 expects 10 ms @ 48 kHz frames");
          if self.wet_dry == 0.0 { return; }
          match self.infer.run(mono) {
              Ok(enhanced) => {
                  for (dry, wet) in mono.iter_mut().zip(enhanced.iter()) {
                      *dry = *dry * (1.0 - self.wet_dry) + wet * self.wet_dry;
                  }
              }
              Err(e) => {
                  // Quiet-once logging — a flood of per-frame errors would
                  // spam the log. Counted, not rate-limited, so we know how
                  // many frames we missed.
                  self.log_miss_once(e);
              }
          }
      }
      fn name(&self) -> &'static str { "deep_filter" }
  }
  ```

- [ ] **Step 3: Run tests**

  ```bash
  cargo test -p waved-audio ns::deep_filter
  ```

- [ ] **Step 4: Commit**

  ```
  feat(audio): SP#4 — DeepFilterNs NS impl with NrLevel wet/dry blend
  ```

---

## Task 8: `AudioConfig` v4 with `neural_ns` section

**Files:**
- Modify: `crates/waved-audio/src/audio_config.rs`
- Modify: `crates/waved-audio/config/audio_config_default.json` (or wherever the canonical default lives)

**Rationale:** Version-bump the config format to expose the model path and the fallback toggle. Mirrors the v2→v3 pattern used for `mic_geometry`.

- [ ] **Step 1: Write failing tests**

  ```rust
  #[test]
  fn deserializes_v3_without_neural_ns_block() {
      let v3 = r#"{"version":3,"capture":{...},"aec":{...},"beam":{...},"mic_geometry":{...}}"#;
      let cfg: AudioConfig = serde_json::from_str(v3).unwrap();
      assert_eq!(cfg.neural_ns.enabled, true, "default enabled");
      assert_eq!(cfg.neural_ns.fallback_to_speex, true);
  }

  #[test]
  fn v4_roundtrip_preserves_neural_ns() {
      let cfg = AudioConfig { version: 4, /* ... */ neural_ns: NeuralNsConfig { enabled: false, .. } };
      let json = serde_json::to_string(&cfg).unwrap();
      let back: AudioConfig = serde_json::from_str(&json).unwrap();
      assert_eq!(back.neural_ns.enabled, false);
  }
  ```

- [ ] **Step 2: Implement**

  Add `NeuralNsConfig` struct, add `neural_ns: NeuralNsConfig` to `AudioConfig` with `#[serde(default)]`. Bump `version: u32 = 4` (the existing migration path tolerates ≥ the embedded default).

- [ ] **Step 3: Run tests + update the JSON default**

  ```bash
  cargo test -p waved-audio audio_config
  ```

  Update `config/audio_config_default.json` to set `"version": 4` and add the block.

- [ ] **Step 4: Commit**

  ```
  feat(audio): SP#4 — AudioConfig v4 adds neural_ns section
  ```

---

## Task 9: Wire into `init_processing` + stats fields

**Files:**
- Modify: `crates/waved-audio/src/lib.rs` — `init_processing` / `AudioService::new` (whichever constructs the NS)
- Modify: `crates/waved-audio/src/stats_collector.rs` — emit `ns_backend` + `ns_frame_ms`

**Rationale:** Make the pipeline actually use DFN3 when the model is available, fall back to speex otherwise. Expose enough telemetry for the acceptance script to assert the right backend activated.

- [ ] **Step 1: Write failing tests**

  ```rust
  #[test]
  fn init_processing_prefers_deep_filter() {
      // Mock a config where neural_ns.enabled=true and a model file exists.
      let tmp = tempfile::NamedTempFile::new().unwrap();
      std::fs::copy(test_fixture("tiny.onnx"), tmp.path()).unwrap();
      let cfg = AudioConfig { neural_ns: NeuralNsConfig { model_path: tmp.path().into(), .. }, .. };
      let svc = AudioService::new_for_tests(cfg).unwrap();
      assert_eq!(svc.ns_name(), "deep_filter");
  }

  #[test]
  fn init_processing_falls_back_to_speex_on_missing_model() {
      let cfg = AudioConfig { neural_ns: NeuralNsConfig { model_path: "/nope".into(), fallback_to_speex: true, .. }, .. };
      let svc = AudioService::new_for_tests(cfg).unwrap();
      assert_eq!(svc.ns_name(), "speex");
  }

  #[test]
  fn init_processing_errors_without_fallback() {
      let cfg = AudioConfig { neural_ns: NeuralNsConfig { model_path: "/nope".into(), fallback_to_speex: false, .. }, .. };
      let r = AudioService::new_for_tests(cfg);
      assert!(r.is_err());
  }
  ```

- [ ] **Step 2: Implement fallback chain**

  ```rust
  fn build_noise_suppressor(cfg: &NeuralNsConfig) -> Result<Box<dyn NoiseSuppressor>> {
      if cfg.enabled {
          match DeepFilterNs::from_path(&cfg.model_path) {
              Ok(ns) => {
                  tracing::info!(model = %cfg.model_path.display(), "neural NS active");
                  return Ok(Box::new(ns));
              }
              Err(e) if cfg.fallback_to_speex => {
                  tracing::warn!("neural NS unavailable: {e}; falling back to speex");
              }
              Err(e) => return Err(e),
          }
      }
      Ok(Box::new(SpeexNs::new()?))
  }
  ```

- [ ] **Step 3: Stats**

  In `stats_collector.rs`, extend the JSON written to `/tmp/audio_stats.json`:

  ```rust
  pub struct AudioStats {
      /* existing */
      pub ns_backend: &'static str,     // set from AudioService::ns_name()
      pub ns_frame_ms: Option<f32>,     // EWMA over last 100 frames, None if ns=="off"
  }
  ```

  The EWMA is maintained inside `DeepFilterNs` (timing each `infer.run()`) and read at the stats tick.

- [ ] **Step 4: Run tests + cross-build**

  ```bash
  cargo test -p waved-audio
  cross build -p waved --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa"
  ```

- [ ] **Step 5: Commit**

  ```
  feat(audio): SP#4 — init_processing prefers DeepFilterNs, falls back to speex
  ```

---

## Task 10: On-device acceptance tests

**Files:**
- Create: `scripts/test-neural-ns.sh`

**Rationale:** Validate the four gates from §2.8 of the spec on real hardware. Same shape as `test-beamforming.sh` — adb deploy, adjust runtime mode via `/tmp/waved-mode` + SIGHUP, read `/tmp/audio_stats.json`.

- [ ] **Step 1: Write the script**

  Outline:

  1. Build + push `waved`, `libonnxruntime.so.1.18.0`, `deepfilternet3.onnx` to device.
  2. Restart waved (kill watchdog + ALSA-release 8 s from SP#3 experience, same pattern).
  3. Assert `ns_backend == "deep_filter"` in stats. If not, print /tmp/waved.log and FAIL.
  4. **Test 1 — inference latency**: sample `ns_frame_ms` 20× over 10 s; compute 95th percentile; pass if ≤ 5 ms.
  5. **Test 2 — stationary noise attenuation**: play pink noise from a BT speaker at calibrated –20 dB SNR; measure `mic_post_aec_rms` with NrLevel=High vs NrLevel=Off; pass if delta ≥ 15 dB.
  6. **Test 3 — CPU delta vs speex**: sample CPU% at NrLevel=High (DFN3) and NrLevel=Off (speex path via fallback override); pass if delta ≤ 2 %. Same CLK_TCK / awk pattern as `test-beamforming.sh`.
  7. Non-stationary test is manual (listen to keyboard clicks) — script prompts, operator confirms.

- [ ] **Step 2: Run on device**

  ```bash
  SKIP_BUILD=0 ./scripts/test-neural-ns.sh
  ```

  Expected: all three automated gates PASS.

- [ ] **Step 3: Document measured numbers**

  Append the baseline measurements (per-frame latency, noise-attenuation deltas, CPU-delta) to `docs/COMPUTE_HARDWARE.md` — like the SP#3 "CPU baseline" table in `AUDIO_HARDWARE.md`. These are the numbers future regressions are judged against.

- [ ] **Step 4: Commit**

  ```
  test(neural-ns): SP#4 — on-device acceptance (latency/atten/CPU)
  docs(compute): SP#4 — measured DFN3 numbers on device
  ```

---

## Self-review

- §2 decisions all flow from evidence gathered in the 2026-04-23 probing session (recorded in `COMPUTE_HARDWARE.md`).
- Tasks 2 + 3 are the real risk; Task 2 exists specifically to fail-fast before Task 3's bigger investment.
- Speex NS stays as a fallback — the worst credible outcome of SP#4 is "NS quality unchanged if NPU path breaks", not a regression.
- `NoiseSuppressor` trait is preserved; the public surface of `waved-audio` changes only by gaining the v4 config block and the two new stats fields. No dependent-crate churn.
- Acceptance gates mirror SP#3's pattern and are measurable from `/tmp/audio_stats.json` without new instrumentation.
