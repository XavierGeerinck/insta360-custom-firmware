# Wave NPU Inference PoC Implementation Plan

> **Goal:** Extend `waved-npu-probe` so a process we control executes the
> deterministic add graph on the Insta360 Wave NPU and verifies `[1, 2] +
> [3, 4] = [4, 6]` for ten iterations.

**Design:** `docs/superpowers/specs/2026-07-23-wave-npu-inference-poc-design.md`

**Architecture:** Keep the existing OpenVX proof-of-life path unchanged.
Add a `--run-add` mode backed by an isolated `libnnsdk.so` wrapper. The
wrapper loads the model from memory, submits two raw `UINT8[2]` inputs,
uses the stock runtime's separate run/output modes, validates the output,
and owns cleanup through RAII. Run the device work on a monitored thread
so a stuck native call terminates the probe after a fixed timeout without
changing driver recovery settings.

**Tech stack:** Rust, `libloading`, `sha2`, Amlogic NN SDK C ABI,
`aarch64-unknown-linux-gnu`, ADB.

## ABI Evidence

The declarations below are not inferred from symbol names alone. They
are cross-checked against:

- the public Amlogic `nn_sdk.h`;
- the newer ADLA-compatible `nn_sdk.h`;
- Ghidra/objdump analysis of the exact Wave
  `libnnsdk.so` build ID `859f8be7a853f735793c1ed9e3227e2ef86c0fce`;
- call sites in the exact firmware-matched `libasr_reverb.so`.

The stock caller establishes:

- `aml_module_create(aml_config *)`;
- `aml_module_input_set(void *, nn_input *)`;
- `aml_module_output_get(void *, aml_output_config_t)` where the
  56-byte by-value aggregate is passed indirectly under AAPCS64;
- `aml_module_destroy(void *)`;
- `aml_config` size `2272`, with `typeSize` at `0`, `length` at `4`,
  `path` at `8`, `pdata` at `16`, `modelType` at `24`, and `nbgType`
  at `28`;
- stock NBG-memory loading uses `modelType = ONNX (4)` and
  `nbgType = NN_NBG_MEMORY (1)`;
- `nn_input` uses `input_index` at `4`, `size` at `8`, `input` at
  `16`, and `input_type` at `24`;
- `aml_output_config_t` size `56`, with `perfMode` at `8` and
  `format` at `12`;
- `perfMode = 1` executes the graph and `perfMode = 2` retrieves the
  output;
- `nn_output.num` is at `4`, the first `outBuf_t` starts at `8`, and
  its data pointer is therefore at `nn_output + 0x50`.

The probe will encode these layouts with `#[repr(C)]` structures and
host tests for every relied-upon size and offset.

## Task 1: Add ABI types and layout tests

**Files:**

- Create: `waved/crates/waved-npu-probe/src/nnsdk.rs`
- Modify: `waved/crates/waved-npu-probe/src/main.rs`

- [x] Define only the NN SDK enums and structures the probe needs.
- [x] Represent the unused tail of `aml_config` as reserved zeroed
      bytes so the complete object is exactly 2272 bytes.
- [x] Represent the unused tail of `aml_output_config_t` so it is
      exactly 56 bytes.
- [x] Define the current `outBuf_t` ordering (`size`, `name`, `buf`,
      `param`, `out_format`) confirmed by the stock caller's load from
      return pointer offset `0x50`.
- [x] Add tests using `size_of` and `offset_of` for all ABI facts
      listed above.
- [x] Add a pure output-verification function and tests for the pass,
      wrong-value, null-buffer, wrong-count, and short-buffer cases.

Run:

```sh
cd waved
cargo test -p waved-npu-probe
```

Expected: all host tests pass without requiring an NPU or loading
`libnnsdk.so`.

## Task 2: Implement the stock-runtime wrapper

**Files:**

- Modify: `waved/crates/waved-npu-probe/src/nnsdk.rs`
- Modify: `waved/crates/waved-npu-probe/Cargo.toml`
- Modify: `waved/Cargo.lock`

- [x] Add `sha2` for deterministic model validation.
- [x] Load `/usr/lib/libnnsdk.so` and resolve the four exact direct
      function exports.
- [x] Keep the `Library` alive for the full lifetime of every resolved
      symbol and graph handle.
- [x] Read the model, validate the expected SHA-256
      `9bef0c0f2271ad534cb975b386be8428e973dc7ee4c71c04854c95eeac1fea5c`,
      and validate the `VPMN` header before entering native code.
- [x] Create the graph using the firmware-confirmed NBG-memory config.
      The call correctly rejects the host graph's incompatible product
      target before returning a context.
- [x] Implement submission of `[1, 2]` at input index 0 and `[3, 4]` at input index 1
      using `BINARY_RAW_DATA (3)`.
- [x] Implement execution with output `perfMode = 1`, retrieval with
      `perfMode = 2` and `AML_OUTDATA_RAW (1)`, then verify `[4, 6]`.
- [x] Implement ten execution/retrieval iterations and elapsed-duration
      recording. The add graph cannot enter this phase until it is
      recompiled for the Wave target.
- [x] Read the hardware status through existing read-only sysfs values.
      Do not call the optional `aml_util_getHardwareStatus`: testing
      showed that export is unsafe through this firmware's direct ABI.
- [x] Make explicit destruction report the native status; retain a
      best-effort `Drop` fallback for every error path.

Run:

```sh
cd waved
cargo test -p waved-npu-probe
cargo clippy -p waved-npu-probe --all-targets -- -D warnings
```

Expected: tests pass and the wrapper introduces no warnings.

## Task 3: Add the CLI mode, diagnostics, and timeout

**Files:**

- Modify: `waved/crates/waved-npu-probe/src/main.rs`
- Modify: `waved/crates/waved-npu-probe/src/nnsdk.rs`

- [x] Preserve the no-argument OpenVX context probe in
      behavior and exit status.
- [x] Recognize `--run-add` and the firmware-graph control
      `--run-stock-ctc`; reject unknown or combined arguments
      before opening the driver.
- [x] Use `/data/waved/lib/add_graph.nb` as the fixed accepted custom model.
- [x] Print a concise boundary line for driver access, checksum,
      runtime load, symbol resolution, graph creation, both inputs,
      every execution/output, and cleanup.
- [x] Read and print `nanoqFreq` and `enableNN` without writing them.
- [x] Run the native PoC on a monitored thread and terminate the probe
      with a distinct error after 15 seconds. Do not reset the driver
      or alter sysfs on timeout.
- [x] Use distinct exit codes for argument, driver, checksum, runtime,
      symbol, create, input, execution, output, verification, cleanup,
      and timeout failures.
- [x] On a driver/runtime conflict, scan `/proc/*/fd` best-effort and
      report processes holding `/dev/galcore`; never kill them.

Run:

```sh
cd waved
cargo test -p waved-npu-probe
cargo run -p waved-npu-probe -- --unexpected
```

Expected: tests pass and the unknown argument exits before attempting
device access.

## Task 4: Cross-build and stage the probe safely

**Files:**

- No source changes expected.

- [x] Build the release binary for the Wave.
- [x] Confirm it is AArch64 and dynamically linked as expected.
- [x] Record the current device's NPU parameters and running
      `waved` PID.
- [x] Confirm no process currently holds `/dev/galcore`.
- [x] Push to `/data/waved/waved-npu-probe.poc-new`, mark executable,
      then rename it to `/data/waved/waved-npu-probe.poc`.
- [x] Do not replace the currently deployed default probe until the
      staged binary passes its original context check.

Run:

```sh
cd waved
cross build --release --target aarch64-unknown-linux-gnu \
  -p waved-npu-probe
file target/aarch64-unknown-linux-gnu/release/waved-npu-probe
adb push target/aarch64-unknown-linux-gnu/release/waved-npu-probe \
  /data/waved/waved-npu-probe.poc-new
adb shell chmod 0755 /data/waved/waved-npu-probe.poc-new
adb shell mv /data/waved/waved-npu-probe.poc-new \
  /data/waved/waved-npu-probe.poc
```

Expected: the staged binary exists under `/data/waved`; no service,
firmware, model, sysfs value, boot setting, or clock is changed.

## Task 5: Run device acceptance checks

**Files:**

- No source changes expected unless diagnostics reveal a bounded bug.

- [x] Run the original context proof with the staged binary.
- [x] Run `--run-add` with
      `LD_LIBRARY_PATH=/data/waved/lib:/usr/lib`.
- [ ] Require ten exact `[4, 6]` results and successful destruction.
      Blocked: the host graph targets product `0x10000020`; the Wave
      requires `0x1000000e`, so the runtime rejects it at graph creation.
- [x] Add and run `--run-stock-ctc` twice as a firmware-matched control;
      require correct tensor sizes, nonzero output, identical output
      digests, and successful destruction.
- [x] Confirm `waved` is still running with the same PID.
- [x] Confirm `nanoqFreq` and `enableNN` are unchanged.
- [x] Confirm ADB and the device remain responsive.

Run:

```sh
adb shell 'LD_LIBRARY_PATH=/data/waved/lib:/usr/lib \
  /data/waved/waved-npu-probe.poc'
adb shell 'LD_LIBRARY_PATH=/data/waved/lib:/usr/lib \
  /data/waved/waved-npu-probe.poc --run-add'
adb shell 'LD_LIBRARY_PATH=/data/waved/lib:/usr/lib \
  /data/waved/waved-npu-probe.poc --run-add'
```

Observed: the custom add invocation is rejected for an NBG product
target mismatch. Both firmware-graph control invocations pass and the
post-run device state matches the pre-run snapshot.

## Task 6: Record the result

**Files:**

- Modify: `waved/docs/COMPUTE_HARDWARE.md`
- Modify: `docs/REPORT_NPU.md` only if its existing conclusions need
  correction.

- [x] Record the exact runtime route, graph hash, output, iteration
      count, latency summary, and cleanup/repeatability result.
- [x] Separate what the PoC proves (custom process can execute a
      compiled NBG) from what remains blocked (reproducible arbitrary
      ONNX-to-NBG compilation for this chip).
- [x] Run formatting/tests once more and inspect the final diff,
      excluding all unrelated pre-existing audio changes.

## Final Outcome

The implementation proves direct NPU inference from a process we
control using an unmodified Wave-targeted firmware graph. It also
pinpoints why the custom add acceptance criterion cannot pass with the
current artifacts: the available host SDK emits a different
hardware-product target. No runtime wrapper change can make that NBG
safe or compatible; a compiler/archmodel for product `0x1000000e` is
required.

Final verification:

```sh
cd waved
cargo fmt --check -p waved-npu-probe
cargo test -p waved-npu-probe
cargo clippy -p waved-npu-probe --all-targets -- -D warnings
git diff --check
```
