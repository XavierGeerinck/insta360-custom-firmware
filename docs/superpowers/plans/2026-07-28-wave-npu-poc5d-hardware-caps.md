# Wave NPU PoC 5D: Exact OpenVX Hardware-Capability Query

**Date:** 2026-07-28
**Status:** Complete — observational query passed
**Authorization:** The device owner explicitly requested continued NPU
research in this session
**Depends on:** PoC 5C Candidate 1 emitted a valid OpenVX Erf NBG but selected
neither EVIS nor CL
**Safety class:** Read-only context creation, identity query, and documented
OpenVX capability queries; no graph creation, compilation, or execution

## 1. Question

Why did TIM-VX select the OpenVX Erf backend even though its normal priority
places EVIS and CL ahead of OpenVX?

The exact selector skips EVIS and CL when any of these is true:

- runtime shader enable is below one;
- extended hardware capability `subGroupSize` is zero;
- the tensor types are not shader-compatible;
- EVIS is selected but the queried EVIS version is unsupported.

The fixed U8 Erf pair is shader-compatible, and the runtime option defaults to
enabled. PoC 5D therefore queries the exact remaining hardware fields without
creating a graph.

## 2. Exact ABI

The Wave's exact `libovxlib.so` calls `vxQueryHardwareCaps` with sizes `0x20`
and `0x28`. The corresponding ABI, including the graph-batch `coreCount`
field compiled into this stack, is:

```text
base:
  uint32 ecoID
  uint32 customerID
  uint32 evis1
  uint32 evis2
  uint32 coreCount

extended (0x20 bytes):
  base
  uint32 subGroupSize
  uint32 supportVA40
  uint32 supportStreamProcessor

extended 2 (0x28 bytes):
  extended
  uint32 streamProcessorExecCount
  uint32 streamProcessorVectorSize
```

The probe uses explicit fixed-width records with compile-time size assertions.
It makes both exact calls, requires success, and requires the common 32-byte
prefixes to match.

## 3. Fixed Native Behavior

The native tool accepts only:

```text
wave-npu-query-caps-v1 --describe
wave-npu-query-caps-v1 --query
```

`--query` performs:

1. 8-second alarm and parent-death setup.
2. `vxCreateContext`.
3. `gcoHAL_QueryChipIdentityEx(NULL, 0x20, ...)`.
4. `vxQueryHardwareCaps(..., 0x20)`.
5. `vxQueryHardwareCaps(..., 0x28)`.
6. Cross-check the two records and the identity.
7. `vxReleaseContext`.
8. Print exactly one machine-readable identity line and one capability line.

It contains no graph, tensor, operation, NBG, program build, model load,
device-node open, ioctl, loop, output file, or execution call.

## 4. Static and Live Gates

Before staging:

- build reproducibly as an AArch64 PIE;
- require direct `libOpenVX.so` and `libGAL.so`;
- require imports for only the reviewed OpenVX context/capability functions
  and identity query;
- reject source or imports containing graph, processing, scheduling, program
  build, raw ioctl, or loader-interposition primitives;
- record the binary SHA-256.

Before running:

- require exactly device `PSP_0000000000000000`;
- require the exact kernel, `/dev/galcore` `199:0`, and NPU parameters;
- require no `/dev/galcore` holder;
- hash the probe and every exact runtime dependency;
- capture bounded kernel diagnostics.

After running:

- require the same boot ID, device, parameters, hashes, and no holder;
- require no new kernel alert or recovery event;
- record both raw capability records and their semantic interpretation.

There is one observational run. No retry is needed because this query has no
determinism question and is already called internally during normal context
creation.

## 5. Interpretation

- `subGroupSize == 0` means the exact TIM-VX selector will skip EVIS and CL,
  even if an EVIS version bit is reported and the firmware embeds sources.
- `evis1 == 0 && evis2 == 0` independently disables the EVIS backend.
- a nonzero subgroup with exactly one EVIS version would justify a separate
  compile-only probe for an operation having no OpenVX fallback.
- stream-processor fields determine whether the higher-priority SP route is
  relevant to audio graph lowering.

No capability value is overridden. Setting a runtime variable, forcing a
backend, patching a capability record, or attempting a graph belongs to a
separate reviewed PoC.

## 6. Result

The single allowlisted run completed in 72 ms:

```text
run_id: poc5d-hardware-caps-20260728
evis1: 0
evis2: 1
subGroupSize: 4
coreCount capability field: 0
supportVA40: 0
supportStreamProcessor: 0
streamProcessorExecCount: 0
streamProcessorVectorSize: 0
ecoID: 0x04000000
customerID: 0x1000000e
```

The preflight and postflight snapshots matched. The boot ID remained
`0b8091ee-557d-4857-8019-af1aaab9d665`; the kernel, `/dev/galcore` identity,
module parameters, probe/runtime hashes, and holder state were unchanged.
There were no new kernel alerts, recoveries, or reboots.

The exact stack is shader-selector eligible: it reports EVIS2 and a nonzero
subgroup size. Stream-processor and VA40 routes are unavailable.

The queried `coreCount` ABI field is zero, while the NBG emitted by PoC 5C
records one NPU core. These fields come from different interfaces and must not
be treated as equivalent; the result does **not** mean that the Wave has zero
NPU cores.

## 7. Selector Root Cause Recovered After the Query

Static recovery of the exact `libovxlib.so` established two priority tables:

```text
default:       SP(5), EVIS(4), CL(3), VX(2), CPU(1)
VX-first ops:  SP(4), VX(3), EVIS(2), CL(1), CPU(0)
```

The default tuple is stored at virtual address `0x004ee508`; the VX-first tuple
is stored at `0x004eef20`. The selector at `0x000a9520` calls an
operation-specific selector when one is registered, then iterates the sorted
backend list.

The matching TIM-VX selector source explicitly registers `erf` as a VX-first
operation. Therefore the PoC 5C OpenVX result did not demonstrate EVIS or CL
setup failure: SP was excluded by the queried hardware capabilities, then VX
succeeded before either shader backend was attempted.

This closes the original PoC 5D question. A separate PoC 5E may now test the
normal EVIS-first path with a fixed operation that has no VX-first override.
