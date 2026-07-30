# Wave NPU PoC 5F: CLC Compiler Recovery

**Date:** 2026-07-28
**Status:** Complete — relocation passed; compiler call quarantined after ABI failure
**Authorization:** The device owner explicitly requested continued, stepwise
NPU research in this session
**Depends on:** PoC 5E reached EVIS and CL source-program construction but both
failed in `vxBuildProgram`
**Safety class:** Fixed-path shared-library relocation probe followed, only on
success, by one fixed graph compile-to-binary probe; no graph or NBG execution

## 1. Question

Can the Wave's source-program path be restored by supplying the missing
Vivante kernel compiler in an isolated `/tmp` runtime directory?

The exact `libOpenVX.so` contains the following dynamic compiler contract:

```text
library: libCLC.so
symbols:
  gcLoadKernelCompiler
  gcCompileKernel
  gcUnloadKernelCompiler
```

The production filesystem does not contain `libCLC.so`. This explains why
stock, precompiled NBGs can run while a newly instantiated EVIS or CL kernel
cannot be built.

## 2. Candidate Compiler and Known Mismatch

The repository contains:

```text
path:
  waved/third_party/vip9000-sdk/6.4.18.5/drivers/libCLC.so
sha256:
  a1e5d11cafb6d23c7c787d24a71a766782ea022346f1580797d892238bd99f14
architecture:
  AArch64 ELF64 shared object
compiler marker:
  6.4.18:886827
dependencies:
  libVSC.so
  libGAL.so
  libm.so.6
  libpthread.so.0
  libdl.so.2
  librt.so.1
  libc.so.6
```

The Wave's exact `libVSC.so` and `libGAL.so` report `6.4.19:946407`. The CLC
candidate is therefore a one-patch vendor mismatch.

Static ABI comparison found:

```text
undefined CLC symbols:       135
exact GAL/VSC exports:       1580
missing vendor-like imports: 0
required compiler exports:   3/3
```

Symbol coverage is necessary but not sufficient. No compatibility claim is
made until the fixed loader test and compile gate pass.

## 3. Isolation

The candidate is staged only as:

```text
/tmp/wave-npu-compile/lib/libCLC.so
```

with mode `0444` and exact hash verification. The stage script refuses to
replace any existing file at that path. It does not write `/usr`, `/lib`,
firmware, a stock model, a service configuration, sysfs, or a device node.

The existing allowlisted runtime path is:

```text
LD_LIBRARY_PATH=/tmp/wave-npu-compile/lib:/usr/lib
```

Thus `libCLC.so` is added while its `libVSC.so` and `libGAL.so` dependencies
continue to resolve to the Wave's stock `6.4.19` libraries.

## 4. PoC 5F-A: Relocation-Only Loader Smoke

The fixed native loader tool:

1. installs an 8-second alarm and parent-death signal;
2. accepts only `--describe` or `--probe`;
3. rejects loader-interposition and simulator variables;
4. calls `dlopen` with the exact absolute CLC path, `RTLD_NOW|RTLD_LOCAL`;
5. resolves exactly the three compiler symbols above;
6. requires each pointer to be non-null and pairwise distinct;
7. closes the handle and prints one machine-readable result.

It does not call any resolved function. It has no graph, tensor, OpenVX
context, model, NBG, device-node open, ioctl, output file, loop, or arbitrary
library path.

The host harness requires the exact device, runtime hashes, CLC hash, no
`/dev/galcore` holder, unchanged boot/kernel/parameters, and clean kernel
diagnostics before and after the probe.

Stop the entire PoC if relocation fails, the process receives a signal, a
holder remains, or health changes. Do not attempt the compiler graph.

## 5. PoC 5F-B: New SignalFrame Compile Namespace

Only after 5F-A passes, a new immutable compiler probe constructs:

```text
input:         FLOAT16 [32]
operation:     SignalFrame(window_length=8, step=4, pad_end=0, axis=0)
output:        FLOAT16 [8,7]
mode:          CompileToBinary only
```

It uses a new executable and output namespace:

```text
/tmp/wave-npu-compile/bin/wave-npu-compile-signal-frame-clc-v1
/tmp/wave-npu-compile/output/wave-signal-frame-clc-candidate-1.nbg
/tmp/wave-npu-compile/output/wave-signal-frame-clc-candidate-2.nbg
```

This is not a retry of PoC 5E Candidate 1 and does not repurpose PoC 5E
Candidate 2.

The binary retains the identity checks, native watchdog, exclusive atomic
output publication, exact NBG target validation, and prohibition on graph
execution.

## 6. Acceptance

Candidate 1 passes only when:

- the CLC artifact and every dependency match the manifest;
- verbose output reports exactly one successful
  `Instance EVIS node with kernel "signal_frame"` selection;
- no EVIS or CL shader-build error appears;
- `CompileToBinary` emits a valid Wave NBG;
- the strict parser observes one FP16 `[32]` input, one FP16 `[8,7]` output,
  valid section checksums, and target `0x1000000e`;
- the device health gate passes and no holder remains.

Stop without Candidate 2 on any discrepancy. No reset or cleanup action is
authorized.

Candidate 2 is allowed only after Candidate 1 passes and is used solely for
byte-level determinism. A non-identical result blocks PoC 6 pending review.

## 7. Transport Rule

This device's legacy ADB shell transport returns host process status zero even
for `adb shell "exit 7"`. Future harnesses therefore treat:

- nonzero host ADB status as a transport failure;
- the unique, unpredictable, exactly-once sentinel as the native exit status.

The remote sentinel is still cross-checked for syntax and range. This rule is
device-specific and evidence-backed; it does not weaken result parsing.

## 8. Non-Goals

PoC 5F does not:

- invoke a resolved CLC function in the loader smoke;
- execute a graph or NBG;
- replace a stock runtime library;
- preload or audit a process;
- patch product, chip, or capability values;
- change firmware, services, sysfs, or kernel state;
- claim numeric correctness or full-model support.

The first NBG execution remains PoC 6.

## 9. PoC 5F-A Result

The fixed relocation-only observation passed:

```text
run_id: poc5f-a-clc-loader-20260728
manifest_sha256: 5b96a409b4be6cbd1775e8b97d706a2d92028daf79415a6b8bd753eb4d33a8df
probe_sha256: 33efe6b0e4cf081e9b871fb12dd50654b52912cd566252caeba51883b72f8e79
CLC_sha256: a1e5d11cafb6d23c7c787d24a71a766782ea022346f1580797d892238bd99f14
duration: 45 ms
exports_resolved: 3
symbols_called: 0
```

The loader resolved every relocation and all three compiler entry points,
closed the library, and exited normally. The boot ID, kernel, device node,
module parameters, hashes, holder state, and diagnostics were unchanged.

This established loader compatibility only. It did not establish compiler or
program-binary ABI compatibility.

## 10. PoC 5F-B Result

The separately namespaced compile observation ran once:

```text
run_id: poc5f-b-signal-frame-clc-compile-1-20260728
manifest_sha256: 6a445c75d1da84c4c4fcf8205f634a2ad86502d0c6942801e61ba585e89ec8fe
probe_sha256: 883bb4aa3593af8d226053b0a3760f6974ab46c4516ed02f5e6953a194a4381a
duration: 209 ms
native_exit: 134 (SIGABRT)
```

Unlike PoC 5E, `vxBuildProgram` did not report shader compilation failure.
Both EVIS and CL advanced to kernel-object initialization. They then failed:

```text
EVIS Add parameter 0:  -12 / VX_ERROR_INVALID_REFERENCE
EVIS Finalize kernel:  -12 / VX_ERROR_INVALID_REFERENCE
EVIS Remove kernel:    -10 / VX_ERROR_INVALID_PARAMETERS

CL Add parameter 0:    -12 / VX_ERROR_INVALID_REFERENCE
CL Finalize kernel:    -12 / VX_ERROR_INVALID_REFERENCE
CL Remove kernel:      -10 / VX_ERROR_INVALID_PARAMETERS
```

CPU registration succeeded, `CompileToBinary` failed, and process teardown
reported:

```text
double free or corruption (out)
Aborted
```

No NBG or partial file was created. Candidate 2 was not run.

The device health gate remained clean: no holder, kernel alert, recovery,
reboot, changed boot ID, parameter change, or artifact change was observed.
The corruption was contained to the short-lived user-space compiler process.

## 11. Conclusion and Quarantine

The 6.4.18 CLC candidate is loader-compatible with the Wave's 6.4.19
GAL/VSC pair and can cross the missing-compiler boundary, but it is not
compiler/program-object ABI compatible. The invalid kernel reference followed
by allocator corruption is decisive.

The staged file is retained under its hash-pinned `/tmp` evidence path, but it
is **quarantined**:

- do not call its compiler entry points again;
- do not use it for Candidate 2 or another graph;
- do not preload it into a stock service;
- do not copy it into `/usr/lib`;
- do not treat source-build success as a valid kernel.

Further custom-shader work requires either:

1. the exact `6.4.19:946407` AArch64 `libCLC.so`; or
2. a host/offline VXC-binary compiler path proven to target customer/product
   `0x1000000e` without identity patching.

Until one of those paths is available, PoC 6 may use only an already valid,
strictly reviewed OpenVX-backed NBG such as the PoC 5C Erf artifact. It must
not execute any PoC 5E/5F output because none exists.
