# Wave NPU PoC 5E: SignalFrame EVIS Compile

**Date:** 2026-07-28
**Status:** Complete — Candidate 1 stopped at the shader compiler boundary
**Authorization:** The device owner explicitly requested continued, stepwise
NPU research in this session
**Depends on:** PoC 5C valid NBG emission; PoC 5D EVIS2 capability and exact
selector recovery
**Safety class:** Fixed graph construction and compile-to-binary only; no graph
or NBG execution

## 1. Question

Can the Wave compile an operation through its embedded EVIS2 shader backend,
without modifying the firmware, runtime capability records, backend priority,
or product identity?

PoC 5C established exact-device NBG generation, but `Erf` has a recovered
operation-specific VX-first selector. PoC 5D established that the hardware is
eligible for shader selection:

```text
evis1=0
evis2=1
subGroupSize=4
supportStreamProcessor=0
```

The next probe must use the normal selector order and an operation for which
the exact library contains an EVIS implementation.

## 2. Why SignalFrame

`SignalFrame` converts a sample stream into overlapping windows, the first
stage of many spectral noise-suppression front ends. It is more informative
than another generic activation while remaining a one-node graph.

The exact Wave `libovxlib.so` embeds:

```text
com.vivantecorp.extension.evis.signal_frame_F16toF16
com.vivantecorp.extension.evis.signal_frame_I16toI16
com.vivantecorp.extension.evis.signal_frame_U8toU8
com.vivantecorp.extension.evis.signal_frame_I8toI8
com.vivantecorp.extension.evis.signal_frame_BF16toBF16
```

It also embeds the `signal_frame` shader source and registers EVIS and CL
backends. `signal_frame` is absent from the recovered VX-first selector list.
The staged exact-device TIM-VX library exports the `SignalFrame` constructor.

## 3. Fixed Graph

The native compiler contains one immutable graph:

```text
input:         FLOAT16 [32]
operation:     SignalFrame(window_length=8, step=4, pad_end=0, axis=0)
output:        FLOAT16 [8, 7]
graph inputs:  1
graph outputs: 1
```

The seven windows follow:

```text
(32 - 8) / 4 + 1 = 7
```

FLOAT16 avoids adding quantization assumptions to the first shader-backend
gate. Later operator characterization will separately cover the audio-native
I16 DFP route.

The tool accepts only:

```text
wave-npu-compile-signal-frame-v1 --describe
wave-npu-compile-signal-frame-v1 --candidate-1
wave-npu-compile-signal-frame-v1 --candidate-2
```

Both candidates contain the same graph and differ only in their fixed,
exclusive output paths.

## 4. Native Invariants

The probe:

1. installs a 15-second native alarm and parent-death signal;
2. rejects simulator and loader-interposition variables;
3. requires the exact Wave chip model and customer/product ID;
4. constructs only the graph in section 3;
5. calls `CompileToBinary` once for size and once for bytes;
6. requires `VPMN`, NBG version `0x00020000`, and target `0x1000000e`;
7. writes with `O_EXCL|O_NOFOLLOW`, publishes by hard link, and leaves mode
   `0444`;
8. never calls `Compile`, `Run`, `Verify`, `ProcessGraph`, `ScheduleGraph`,
   raw `open("/dev/galcore")`, or `ioctl`.

There is no model path, operation selector, tensor selector, environment
override, arbitrary remote command, retry, reset, or cleanup interface.

## 5. Static Gate

Before staging:

- build an AArch64 PIE against the staged exact-device TIM-VX library;
- require direct TIM-VX and GAL dependencies and the exact `/tmp` runpath;
- require the `SignalFrame` constructor import;
- require exactly two `CompileToBinary` call sites;
- reject execution, raw ioctl, loader-interposition, and arbitrary model-load
  primitives;
- require the fixed FLOAT16 `[32] -> [8,7]` definition in `--describe`;
- record the source, binary, runtime, and manifest SHA-256 values.

## 6. Live Gate

Before each candidate:

- require only device `PSP_0000000000000000`;
- require the allowlisted kernel, boot ID format, `/dev/galcore` `199:0`, NPU
  parameters, and exact artifact hashes;
- require no `/dev/galcore` holder;
- require both the final and partial output paths to be absent;
- capture a bounded kernel-diagnostic baseline.

After each candidate:

- require unchanged device, boot ID, kernel, parameters, dependency hashes,
  and holder state;
- reject any new warning, fault, timeout, recovery, MMU, or hang diagnostic;
- pull the NBG and verify all section checksums;
- require one FLOAT16 `[32]` input and one FLOAT16 `[8,7]` output;
- preserve the complete stdout/stderr and parsed NBG report.

## 7. Acceptance and Stop Conditions

Candidate 1 passes only if all of the following hold:

- native compile succeeds and emits an exact-target NBG;
- verbose selector evidence contains exactly one successful
  `Instance EVIS node with kernel "signal_frame"` line;
- no successful VX, CL, CPU, or SP selection is reported for the operation;
- strict NBG validation and the device health gate pass.

Stop immediately, without Candidate 2, on timeout, malformed result, non-EVIS
selection, health change, checksum failure, or unexpected tensor metadata.
No recovery action is authorized.

Candidate 2 is allowed only after Candidate 1 satisfies every acceptance
condition. It measures compiler determinism. The runs pass the determinism
gate when their bytes are identical; a normalized metadata match with
different bytes is recorded as nondeterminism and blocks PoC 6 pending review.

## 8. Explicit Non-Goals

PoC 5E does not:

- execute either NBG;
- load live audio;
- force or patch a backend;
- change runtime options, sysfs, firmware, libraries, or models;
- characterize numeric correctness or performance;
- claim that a complete noise-cancellation model is supported.

The first NBG execution remains a separate PoC 6 review gate.

## 9. Result

Candidate 1 was run once under:

```text
run_id: poc5e-signal-frame-compile-1-20260728
manifest_sha256: 33529402eafa459bc369047849805363059f9675f8e757370f7691109a481887
probe_sha256: efb5047921574d67015da527c89df5a4321ad57ddf14ac8ffc8f13a70be0e37f
native duration: 86 ms
native exit: 5
```

Graph setup matched the fixed definition:

```text
input:  f16 [32], quantization NONE
output: f16 [8,7], quantization NONE
op:     SIGNAL_FRAME
```

Backend setup then produced:

```text
EVIS signal_frame_F16toF16:
  Failed to compile vx shader (0xFFFFFFED)
CL signal_frame_F32toF32:
  Failed to compile vx shader (0xFFFFFFED)
CPU signal_frame:
  registration succeeded
CompileToBinary size query:
  failed
```

`0xFFFFFFED` is signed status `-19`, which the exact OpenVX headers name
`VX_ERROR_INVALID_NODE`. The selector ultimately reported CPU, so the EVIS
acceptance criterion was not met. No NBG or partial output was created.
Candidate 2 was not run.

The host `adb` process returned zero even though the unique internal sentinel
reported native exit 5. A separate read-only check confirmed that this device's
legacy ADB shell transport also returns host status zero for `adb shell
"exit 7"`. The run correctly stopped at the then-current
transport-consistency gate; the preserved native log provides the more
specific failure.

Preflight and postflight matched: boot ID, kernel, device node, module
parameters, dependency hashes, and holder state were unchanged. No new kernel
alert, recovery, reboot, or timeout occurred.

## 10. Root Cause Narrowing

Static analysis after the stopped run recovered the source-program path:

1. this `libovxlib.so` build sets custom GPU kernels to
   `VSI_NN_GPU_SOURCE_FMT_CODE`, not the precompiled VXC-binary path;
2. `_gpu_register` creates a source program and calls `vxBuildProgram`;
3. the exact `libOpenVX.so` loads `libCLC.so` dynamically and resolves
   `gcLoadKernelCompiler`, `gcCompileKernel`, and `gcUnloadKernelCompiler`;
4. the Wave filesystem has no `libCLC.so` under `/usr`, `/lib`, `/vendor`,
   `/system`, `/opt`, or `/app`;
5. the failure therefore occurs before an EVIS node can be instantiated.

The repository contains an AArch64 vendor `libCLC.so` from driver
`6.4.18:886827`, while the Wave's GAL/VSC runtime reports
`6.4.19:946407`. Its three loader/compiler exports are present, and all 135
undefined imports—including every vendor-prefixed import—resolve against the
exact Wave `libVSC.so`, `libGAL.so`, or standard C runtime. This is encouraging
but does not prove patch-version ABI compatibility.

Loading that compiler is a separate PoC 5F. It must use a fixed `/tmp` path,
perform a relocation-only smoke test before graph construction, preserve the
version mismatch in evidence, and use new output names. No stock library is
replaced.
