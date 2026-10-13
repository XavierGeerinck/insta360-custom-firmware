# 2026-10-13 — PoC 16: Native FP16 Conv and quantized U8 FC

## Record

| Field | Value |
|---|---|
| PoC | `16A` / `16B` compile; `16C` / `16D` execute |
| Device alias | `wave-lab-unit-a` |
| Compile UTC | 2026-10-13, 06:43:58–06:48:48 |
| Execution UTC | 2026-10-13, 07:07:43–07:12:08 |
| Source | Approved [design](../../superpowers/specs/2026-10-13-wave-npu-fp16-u8-probes-design.md) and [completed plan](../../superpowers/plans/2026-10-13-wave-npu-fp16-u8-probes.md) |
| Implementation commit | `e541da012a266f3f3670f31931749793e08bacb0` |
| Worktree at compile | Uncommitted probe implementation; exact source and binary identities retained in build provenance. The envelope's HEAD alone does not identify these sources. |
| Outcome | **PASS: both custom graphs exported native NBGs for the NN target and executed once with exact oracle output.** |

## Hypothesis and reason

The exact Wave runtime can compile a small FP16 convolution and a dedicated
asymmetric-U8 fully connected graph, export target-native NBGs, and execute
each against an independent CPU oracle without changing captured device
health.

[REA recovery](2026-10-13-rea-op-dtype-recovery.md) proved that the NN format
checker rejects FP32 weights with an output present. The earlier FP32 Conv
failures therefore did not establish that native Conv was unavailable. REA
also recovered FP16/FP16 and asymmetric U8/U8 admission pairs in both
firmware dtype tables. This experiment tests their remaining live gates.

## Fixed models

| Contract | FP16 FIR Conv | U8 FC |
|---|---|---|
| Input | FP16 `[16,1,1,1]`, 32 bytes | U8 `[16,1]`, 16 bytes |
| Weights | FP16 `[3,1,1,1]`: `[0.25,0.5,0.25]` | U8 `[16,4]`: each output selects input index `0`, `5`, `10`, or `15` |
| Bias | FP32 `[1]`, zero | INT32 `[4]`, zero, scale `1` |
| Operation | Conv2d, SAME padding, stride 1 | FullyConnected, axis 0, 4 units |
| Output | FP16 `[16,1,1,1]`, 32 bytes | U8 `[4,1]`, 4 bytes |
| Quantization | None | Input/weights/output: asymmetric scale `1`, zero point `0` |
| Frozen input | Alternating `+1,-1` | `[0,1,…,15]` |
| CPU oracle | `[0.25,0,…,0,-0.25]`, fourteen interior zeros | `[0,5,10,15]` |

The Python oracles compute the FIR and sparse matrix product independently
of the native expected-byte constants. Both controls avoid rounding and
saturation ambiguity.

## Stack and build identity

The exact device stack remains OpenVX
`f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb`
and ovxlib
`09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9`.
TIM-VX source is commit `702715d714277376d7ff466b7b72833ff57564c1`.

The historical TIM-VX wrapper was absent. A new AArch64 wrapper was built
against the exact device SDK in an immutable Docker toolchain, with source
and SDK mounted read-only and build networking disabled. All 1,895 source
and 43 SDK input hashes remained unchanged. The wrapper's direct imports
are ovxlib and compatible C/C++ runtime libraries; no CLC dependency or
native host model execution was introduced.

| Artifact under `artifacts/npu/build/poc16/` | Bytes | SHA-256 |
|---|---:|---|
| `runtime/libtim-vx.so` | 2,190,640 | `c984e063e9ab4fe4d7e7ac85e783d88a330f1560268924ba4f449f5b258478dc` |
| `probes/wave-npu-compile-fp16-conv-v1` | 36,728 | `db5c8e927af53098f64690b1c63ede17a9ebda16f14f5642945a464a028ef667` |
| `probes/wave-npu-compile-u8-fc-v1` | 37,312 | `bd755c476780b2fa1ce4d2baf8535192c41b47072043967832cd4301ce8c9a98` |
| `executors/wave-npu-execute-fp16-conv-v1` | 32,096 | `81900c201236e06ba5c6b1232d90d236de569f494a6a5ab6a7daf77bfbe37d02` |
| `executors/wave-npu-execute-u8-fc-v1` | 36,776 | `771871c8e5aacccc3bc279d6947ac69268c045e755baee0d0022ce9cb8df4fa0` |

Runtime provenance: `runtime-provenance.json`, SHA-256
`58601a21a3ac1ff0435833d5d30409a43332a892c77047428cb9a54d658efade`.
The local build image ID is
`sha256:a45f1308adfe3fc3b90e329e72d5efe9d743e6cc77017254d28bbba1c689586a`;
the pinned upstream image is
`ghcr.io/cross-rs/aarch64-unknown-linux-gnu@sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c`.

## Admission, bounds and method

The owner approved this compile → qualification → execution chain with
`go`. Each compiler made one size-query and one export call, four public
export calls total. Compile processes had a 15-second native alarm and
20-second host timeout; they contained no inference or NBG import call.
Logging included `VIV_VX_DEBUG_LEVEL=1` and
`VIV_VX_ENABLE_PRINT_TARGET=1`.

Host qualification required native VPMN v2, target `0x1000000e`, exact
dimensions/dtypes/quantization, all section checksums and the file CRC.
Only the two exact qualified model hashes entered execution policies.
Execution staging pinned 17 system artifacts plus the executor, wrapper
and NBG, and verified regular-file modes. The two runners each contained
one NBG import, one `Compile()` and one `Run()`, with an 8-second native
alarm, parent-death signal and 12-second host timeout. The alarm remained
armed through explicit graph/context teardown.

After compilation, each runner seeded the **device output tensor** with
`0xa5` before running. Host readback was independently seeded with `0xa5`
and enclosed by 16-byte `0x5a` guards. Every output byte and guard was
checked before PASS; actual readback bytes were recorded.

The live commands were invoked exactly once per compile/run:

```bash
python3 waved/scripts/npu/npu_op_dtype_lab.py fp16-conv run \
  --run-id rea-poc16a-fp16-conv-20261013-a1
python3 waved/scripts/npu/npu_op_dtype_lab.py u8-fc run \
  --run-id rea-poc16b-u8-fc-20261013-a1
python3 waved/scripts/npu/npu_op_dtype_execute_lab.py fp16-conv run \
  --run-id rea-poc16c-fp16-conv-20261013-a1
python3 waved/scripts/npu/npu_op_dtype_execute_lab.py u8-fc run \
  --run-id rea-poc16d-u8-fc-20261013-a1
```

Each had a separate `stage` and bounded `cleanup` command. Execution roots
were `/tmp/wave-npu-exec16a` and `/tmp/wave-npu-exec16b`; compile roots were
`/tmp/wave-npu-compile16a` and `/tmp/wave-npu-compile16b`. There were no
native retries, recovery actions, service stops, sysfs writes, reboots,
firmware replacements or system-library replacements on the Wave. The
adjacent, incompatible CLC remained quarantined.

## Compile and structural results

Both compile envelopes returned `PASS`, native exit `0`, no timeout and
empty errors, health differences and kernel alerts. Both raw target logs
explicitly name `VXNNE_OPERATION_TARGET_NN`. The dedicated FC frontend was
optimized into an NN convolution by the vendor runtime.

| Qualified `candidate-1.nbg` | Bytes | SHA-256 |
|---|---:|---|
| FP16 Conv, run `rea-poc16a-fp16-conv-20261013-a1` | 1,800 | `4e2b1df127c7f723faea050bbaff40db618eacd21e9efa1d18bff52151c7af4d` |
| U8 FC, run `rea-poc16b-u8-fc-20261013-a1` | 2,056 | `00521883a0e69d27f3ce2092f2cfff1932a208fad7bf2665648537d1f52a4a1e` |

Both NBG reports have the exact I/O contracts above, three of three stored
section CRCs matching, a matching file CRC, no parser warnings and 45,312
bytes of declared VIP SRAM. Both are VPMN v2, target `0x1000000e`.
The frontend graph contains one operation; directory operation records and
network metadata counts describe different internal units and are not a
count of user-visible layers.

## Execution results and descriptive timing

| Run | Status / remote exit | Iterations | Readback | `Run()` time |
|---|---|---:|---|---:|
| `rea-poc16c-fp16-conv-20261013-a1` | PASS / 0 | 1 | All 32 bytes match the FP16 FIR oracle | 1,645 µs |
| `rea-poc16d-u8-fc-20261013-a1` | PASS / 0 | 1 | `00 05 0a 0f` | 1,673 µs |

FP16 output, little-endian bytes:

```text
00340000000000000000000000000000000000000000000000000000000000b4
```

Both runners completed teardown before printing `cleanup=0`, and the
host observed native exit `0` with empty errors, health differences and
kernel alerts. One first execution per model is a correctness control,
not a latency benchmark or evidence of an audio performance gain.

## Negative observations and checks

- The cached older cross-toolchain could not build this source. The pinned
  historical upstream toolchain provided the required compiler without
  installing build dependencies.
- Initial host ELF policy omitted the loader, and the compiler policy
  incorrectly expected a dynamic import for a virtual C++ export call.
  Both host checks were corrected before staging; rejected reports remain
  in the build evidence. These were host qualification failures.
- Independent review found that the first offline runner guarded only
  host readback and cancelled the alarm before teardown. Both were fixed
  and rebuilt before any executor was staged or run; those old binaries
  are retained separately as `executors/host-canary-only-a1/`.
- Delayed compile cleanup initially refused because noisy Wi-Fi logs had
  rotated the short diagnostic window. No deletion or NPU call occurred
  on refusal. A tested, bounded 16,000-line fallback retained the baseline
  marker and still checks new faults. Cleanup then passed without rerunning
  either model.
- Compile logs contain vendor force-release messages at process teardown.
  They are retained in raw evidence; normal exits, holder scans and health
  checks passed. Execution stderr was empty for both models.
- The 94 relevant host tests passed, covering shared compile/execution
  controls, NBG parsing, fixed contracts, independent oracles, hash checks,
  canaries, service changes, diagnostic-window rotation and strict cleanup.
  Both native source/build policy verifiers also passed before live work.

## Safety postconditions

| Invariant | Observed result |
|---|---|
| ADB and boot | Responsive; boot identity unchanged |
| Kernel / device | Exact kernel retained; `/dev/galcore` remains major 199, minor 0 |
| Parameters | `enableNN=255`, `nanoqFreq=838860800`, `recovery=1`, unchanged |
| System hashes | All 17 execution baseline hashes unchanged |
| Holders | Empty at captured preflight/postflight/cleanup instants |
| Services | `waved` and watchdog process identities unchanged |
| Diagnostics | No new captured NPU/kernel alert |
| Disposable files | All four PoC 16 staging roots removed |

Empty holder snapshots do not establish continuous exclusivity. No Wave
service, stock model, library, sysfs, module or firmware was modified.
The authorized system installation update was on the Mac: Ghidra 12.1.4
and OpenJDK 21.0.12.1, as recorded in the REA notebook.

## Raw evidence and definition hashes

All four raw directories are under `artifacts/npu/runs/`. Each retains
`run.json`, `preflight.json`, `postflight.json`, `stdout.txt`, `stderr.txt`
and `cleanup.json`; compile directories also retain `candidate-1.nbg` and
`nbg-report.json`. Build, stage and final-state evidence is under
`artifacts/npu/build/poc16/`. Raw artifacts are intentionally Git-ignored;
the source, fixed manifests and this evidence summary are tracked.

| PoC | Canonical manifest SHA-256 | Raw `run.json` SHA-256 |
|---|---|---|
| 16A | `c638be02a1368edf931b1d88ebc5680c811a0eccfa7f027fb5319228aa5690e3` | `064e358bfaeab5498445ead5da966a8ca54b9a182d24106c6e5c8af145c635af` |
| 16B | `fdfeb1e05598a68954e799c1cae91f16488c11c75eaa4f27badac8d010829799` | `8ec36c57540010dfbb658db086258587c5335d9f41acace201a1b4dc2c7dbfc4` |
| 16C | `e53cd8741f4c80d60239c045b975d622143e065ae4a8a4853da6e2d2be59fa65` | `5e69f9a86ac75435adadd7cb0aa76088a5a9a56d325d6674d9d88e9269037768` |
| 16D | `cdb6f138a0e5b753c677a6cb3e11b07707a4889c2d52078c4c90dd9435bf9477` | `72e22f79d93bbb88aa5463aa7eda18a15942519cf040231476589939120ddcd8` |

NN-selection stdout SHA-256: 16A
`89ea292c337ae1d53c88e2fa644f7d5b1b2ad7542567bf4e7ad0a38c11568e51`;
16B `661052bdebafd6163f3f836d39d61753f7ce691a4cc26e532b31fee43a6dfa86`.

Final read-only state record: `artifacts/npu/build/poc16/final-state.json`,
553,785 bytes, SHA-256
`f311492cf9cf93f6bae668877c6064f7cb8766c3347fc9b817a4aee31a7eaba1`.
It confirms all four roots absent and empty health differences/kernel alerts
against the last successful cleanup, with the 17 system hashes intact.

## Conclusion and next boundary

The hypothesis passed for both fixed graphs. Repository-controlled TIM-VX
source plus the exact Wave runtime can compile and run FP16 Conv and
quantized U8 FC on the NN engine. FP32-weight rejection was a material
boundary in the previous native Conv experiment.

This proves two useful native primitives. General ONNX import, additional
shapes and operations, multi-layer graph coverage, trained-model accuracy,
an open coefficient emitter and sustained performance remain unqualified.
Vendor coefficient serialization succeeded for these two fixed graphs.
The CPU-first production noise-reduction decision remains based on the
existing end-to-end benchmarks. The completed PoC 16 chain supplies the
working native route for a future model-specific design.
