# Wave NPU PoC 6: First Custom NBG Execution

**Date:** 2026-07-28
**Status:** Executed once — PASS
**Authorization:** The device owner explicitly requested that the stepwise NPU
program continue through the POCs in this session
**Depends on:** PoC 5C emitted a strict-parser-valid exact-Wave Erf NBG; PoC
5D recovered hardware capabilities and selector behavior
**Safety class:** One fixed, previously validated NBG execution through the
documented TIM-VX NBG operation; no raw driver access or retry

## 1. Hypothesis

> The exact PoC 5C Erf NBG can be loaded as one TIM-VX NBG node, process one
> fixed all-zero U8 tensor, return four zero bytes, release normally, and
> preserve the captured device health envelope.

Any timeout, signal, setup/compile/run/copy failure, nonzero output,
postflight holder, kernel alert, recovery, reboot, parameter change, or hash
change disproves the hypothesis and stops the PoC.

## 2. Eligible Artifact

Only this artifact may execute:

```text
provenance run:
  artifacts/npu/runs/poc5c-erf-compile-1-20260728
local path:
  artifacts/npu/runs/poc5c-erf-compile-1-20260728/candidate-1.nbg
bytes:
  5840
sha256:
  6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee
magic:
  VPMN
version:
  0x00020000
target:
  0x1000000e
backend at compilation:
  OPENVX
```

The strict parser established:

```text
inputs:               1 × U8 [4], affine-asymmetric scale 1, zero point 0
outputs:              1 × U8 [4], affine-asymmetric scale 1, zero point 0
layer count:          1
absolute op count:    1
NPU core count:       1
VIP SRAM:             43,520 bytes
section checksums:    3/3 valid
```

No PoC 5E or 5F output exists or is eligible. The foreign host Add NBG and
every stock model are explicitly excluded.

## 3. Isolated Runtime

PoC 6 stages non-overwriting, hash-pinned files under:

```text
/tmp/wave-npu-exec/bin/wave-npu-execute-erf-zero-v1
/tmp/wave-npu-exec/lib/libtim-vx.so
/tmp/wave-npu-exec/model/wave-erf-openvx.nbg
```

The only library path is:

```text
/tmp/wave-npu-exec/lib:/usr/lib
```

`/tmp/wave-npu-exec/lib/libCLC.so` must be absent. This isolates execution
from the quarantined PoC 5F compiler without deleting evidence or modifying a
stock library.

## 4. Fixed Native Behavior

The executable accepts only:

```text
wave-npu-execute-erf-zero-v1 --describe
wave-npu-execute-erf-zero-v1 --execute-once
```

`--execute-once`:

1. requires the fixed environment and absence of loader interposition,
   simulator configuration, and CLC in its isolated library directory;
2. installs an 8-second alarm and parent-death signal;
3. opens only the fixed NBG path with
   `O_RDONLY|O_CLOEXEC|O_NOFOLLOW`;
4. requires a mode-`0444`, 5,840-byte regular file and validates
   `VPMN`, version, and product before constructing a context;
5. creates one TIM-VX context and graph;
6. creates one U8 `[4]` affine-asymmetric input and output tensor;
7. constructs one `tim::vx::ops::NBG` node with one input and one output;
8. copies the fixed input `{0,0,0,0}`;
9. calls `Compile` once and `Run` once;
10. copies four output bytes and requires `{0,0,0,0}`;
11. lets graph, tensors, operation, and context release through scoped
    ownership;
12. prints one exact machine-readable PASS line.

There is no input path, model path, operation, shape, dtype, iteration count,
environment override, output file, arbitrary command, or retry argument.

## 5. Static Gate

Before staging:

- build a reproducible AArch64 PIE;
- require the exact `/tmp/wave-npu-exec/lib:/usr/lib` runpath;
- require direct TIM-VX and GAL dependencies;
- require the NBG constructor import plus one source-level `Graph::Compile`,
  one `Graph::Run`, and one input/output tensor-copy call (the graph and
  tensor methods dispatch virtually and therefore need not appear as direct
  ELF imports);
- require one fixed input and one fixed expected output;
- reject `CompileToBinary`, raw ioctl, direct `/dev/galcore` open, `dlopen`,
  loader interposition, multiple-run loops, threads, async graph execution,
  and raw OpenVX processing calls;
- record all source, executable, NBG, and runtime hashes.

## 6. Live Gate

Preflight requires:

- only device `PSP_0000000000000000`;
- exact kernel and `/dev/galcore` `199:0`;
- `nanoqFreq=838860800`, `enableNN=255`, `recovery=1`;
- exact executable, NBG, TIM-VX, GAL, OpenVX, ovxlib, loader, and C runtime
  hashes;
- no `/dev/galcore` holder;
- no competing PoC 6 process;
- bounded kernel diagnostic baseline;
- absent isolated `libCLC.so`.

The host deadline is 12 seconds. A timeout stops the local ADB client and
blocks all further graph execution pending manual state review. It does not
reset the driver, stop a service, unload a module, or reboot.

Postflight requires unchanged:

- device list, kernel, boot ID, device node, NPU parameters, and required
  hashes;
- no holder or competing probe;
- no new warning, error, fault, timeout, hang, recovery, exception, or MMU
  diagnostic.

## 7. Stop Conditions

This PoC has exactly one execution. Do not retry on failure.

Stop without automatic recovery on:

- native or host timeout;
- signal or nonzero exit;
- malformed result;
- graph setup, compile, run, or copy failure;
- output other than four zeros;
- health difference or new kernel alert;
- lingering holder;
- missing/mismatched artifact;
- unexpected CLC visibility.

No Candidate 2 or performance loop is part of PoC 6.

## 8. Non-Goals

PoC 6 does not:

- execute an EVIS custom shader;
- characterize Erf away from zero;
- benchmark latency, power, or throughput;
- stream audio;
- prove a noise-suppression network;
- mutate a graph or product field;
- replace a service, model, runtime, firmware, or driver;
- use raw ioctls or recovery controls.

A pass proves the first generated exact-target NBG execution path. Numeric
range tests and operation coverage belong to PoC 7.

## 9. Result

PoC 6 executed exactly once:

```text
run:
  artifacts/npu/runs/poc6-erf-execution-1-20260728
manifest sha256:
  9de7357b327d624eac302a6bd0c232d587cf07a9b913f26dfdc9d4209ec45377
execution attempts:
  1
host-observed native duration:
  127 ms
remote exit:
  0
status:
  PASS
```

The exact native result was:

```text
IDENTITY chip_model=0x00009000 chip_revision=0x00009002 chip_product_id=0x05090001 chip_eco_id=0x04000000 customer_id=0x1000000e reserved=0x00000000 flags=0x0000000000000000
EXECUTION status=PASS mode=execute-once iterations=1 input=0,0,0,0 output=0,0,0,0 bytes=4
```

TIM-VX reported one `NBG` node with one U8 `[4]` input and one U8 `[4]`
affine-asymmetric output. Preflight and postflight retained the same boot ID,
kernel identity, device node, NPU parameters, required artifact hashes, and
absence of CLC in all three checked loader locations. There were no
`/dev/galcore` holders, health differences, or new kernel alerts.

## 10. Conclusion

The hypothesis passed. This is the first repository-controlled,
device-generated, exact-target NBG that has been loaded and executed on the
Wave NPU. It establishes the complete path:

```text
TIM-VX graph
  -> exact Wave CompileToBinary
  -> strict VPMN validation
  -> TIM-VX NBG import
  -> galcore execution
  -> verified tensor output
```

It does not yet prove useful nonzero arithmetic or a multi-operator audio
model; those remain separate PoCs.
