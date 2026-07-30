# PoC 5 — Recovering a Wave-Targeted Custom-Graph Compiler Path

**Date:** 2026-07-28
**Status:** Complete through PoC 5F; one exact-target NBG recovered, general
custom-op compilation still blocked
**Evidence class:** Host static analysis plus bounded compile-only and
capability probes on the authorized Wave
**Device:** `PSP_0000000000000000`
**Baseline boot ID:** `0b8091ee-557d-4857-8019-af1aaab9d665`

## Executive result

PoC 5 found a legitimate route from repository-controlled TIM-VX code to an
untouched NBG for the Wave target. The successful graph is one U8 Erf
operation:

```text
TIM-VX Erf graph
  -> exact Wave libovxlib/libOpenVX/libGAL
  -> vxGenerateNBG
  -> VPMN v2, target 0x1000000e
  -> 5,840-byte NBG
```

The artifact was not made valid by changing a product field. The Wave stack
itself emitted target `0x1000000e`, and the repository parser validated its
directory structure, tensor metadata, memory metadata, and all stored
checksums.

This is a narrow compiler success, not a general compiler unlock:

- Erf selected a firmware-provided OpenVX LUT/user-kernel path and exported.
- Add, MatrixMul, and Conv2d built-in OpenVX nodes did not export.
- SignalFrame reached the normal shader path but the firmware lacks
  `libCLC.so`.
- A version-adjacent `6.4.18:886827` CLC library relocated against the Wave,
  but its compiler/program-object ABI was incompatible with the exact
  `6.4.19:946407` runtime and caused an isolated user-process abort.

The version-adjacent CLC is quarantined. Its compiler entry points must not be
called again.

## Why this was split into small PoCs

The software-visible path has several independent gates:

```text
graph construction
        |
        v
kernel/backend selection
        |
        v
OpenVX graph verification
        |
        v
NBG serialization
        |
        v
NBG import and execution
```

Success at one gate does not imply success at the next. This became observable
in PoC 5B: the Add node could be created and selected, while NBG
serialization still failed. PoCs 8B and 8D later showed the same distinction
between node selection and graph verification.

The experiment style follows the useful parts of the ANE reverse-engineering
methodology: map the public-to-private stack, recover exact identities and
formats, use a tiny graph to find each boundary, retain negative results, and
only then graduate an artifact to execution. No Apple ANE ABI, format, or
hardware assumption was transferred to the Wave.

## Fixed device and runtime identity

Every live PoC pinned:

```text
kernel:
  Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT
  Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux

/dev/galcore:
  character device 199:0

galcore:
  6.4.19.5.946407

chip identity:
  chip_model       0x00009000
  chip_revision    0x00009002
  chip_product_id  0x05090001
  chip_eco_id      0x04000000
  customer_id      0x1000000e
  flags            0

read-only module parameters:
  nanoqFreq  838860800
  enableNN   255
  recovery   1
```

The serialized NBG target matches `customer_id`, not `chip_product_id`.
An early identity probe placed customer and ECO in the wrong order; that
probe rejected itself before graph construction. The corrected 32-byte ABI
is now compile-time asserted and result-parsed.

## Results at a glance

| PoC | Fixed question | Last useful stage | Result |
|---|---|---|---|
| 5A | Can the bundled x86 simulator authentically select the Wave target? | Simulator identity selection | Negative |
| 5B | Can exact Wave userspace export a U8 Add graph? | OpenVX `add` selected | Export failed |
| 5C | Can a firmware-provided Erf path export? | Exact-target NBG emitted | Pass |
| 5D | What shader capabilities and selector order are active? | Read-only caps and static tables | Pass |
| 5E | Can SignalFrame use the normal EVIS-first shader path? | `vxBuildProgram` | Missing CLC |
| 5F-A | Does a 6.4.18 CLC relocate against Wave 6.4.19? | Three symbols resolved; none called | Pass |
| 5F-B | Is that CLC compiler ABI compatible? | Kernel-object initialization | Fail; quarantined |

## PoC 5A — host simulator target selection

### Hypothesis

The bundled x86 simulator or a documented selector might emit a Wave-native
NBG without using the device.

### Evidence

The host stack could emit the existing Add control:

```text
magic:    VPMN
version:  0x0001001e
target:   0x10000020
```

Static inspection found a Wave identity profile, but the simulator selector
is applied after the simulator HAL has materialized its default identity.
No authentic external hardware configuration matching target
`0x1000000e` was available.

### Conclusion

The host simulator is a useful negative control, not a Wave compiler.
Changing its emitted product bytes would not recompile target-dependent
commands and was never attempted.

## PoC 5B — exact-device Add compile-only gate

### Fixed graph

```text
input A: UINT8 affine-asymmetric, scale 1, zero point 0, shape [2]
input B: UINT8 affine-asymmetric, scale 1, zero point 0, shape [2]
op:      Add
output:  UINT8 affine-asymmetric, scale 1, zero point 0, shape [2]
```

The native tool contained no tensor upload, `Graph::Run`, tensor readback,
raw device-node open, raw ioctl, loop, retry, or output replacement path.

### Runs

| Run | Duration | Exit | Observation |
|---|---:|---:|---|
| `poc5b-device-compile-1-20260728` | 81 ms | 15 | Identity self-check caught the original customer/ECO field-order error before graph construction. |
| `poc5b-device-compile-1b-20260728` | 82 ms | 5 | Correct identity; size-only `CompileToBinary` returned false. |
| `poc5b-device-compile-diagnostic-20260728` | 75 ms | 5 | Verbose reproduction selected OpenVX `add`; export still returned false. |

Compiler SHA-256:

```text
76638c6bcbbab15cd2f466519abc6817521cddda6854b9c6da9a1840f350f10a
```

The decisive diagnostic line was:

```text
Instance OPENVX node with kernel "add"
```

No final or partial NBG was created. Health remained unchanged after all
three short-lived processes.

### Conclusion

The externally built TIM-VX layer is ABI-compatible enough to create a graph
through the exact Wave stack. The exposed `vxGenerateNBG` route is not,
however, a general serializer for this built-in Add node.

## PoC 5C — exact-target Erf NBG

### Fixed graph

```text
input:   UINT8 affine-asymmetric, scale 1, zero point 0, shape [4]
op:      Erf
output:  UINT8 affine-asymmetric, scale 1, zero point 0, shape [4]
```

Compiler SHA-256:

```text
7f7edd31db1f0e80bcc25abfe1ae9a92cf10ce70545dc76d9c4fdd177cf35772
```

Run:

```text
run ID:          poc5c-erf-compile-1-20260728
manifest SHA:    b024f12facd378dfecbafb43c4b776cb99d65c435e575f234dd70bacecd0b241
duration:        141 ms
remote exit:     0
health changes:  none
kernel alerts:   none
```

The selector reported:

```text
Instance OPENVX node with kernel "erf"
```

The untouched output is:

```text
local evidence:
  artifacts/npu/runs/poc5c-erf-compile-1-20260728/candidate-1.nbg

bytes:
  5840

sha256:
  6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee
```

Strict parser result:

```text
magic:              VPMN
format:             v2 directory
version:            0x00020000
target/customer:    0x1000000e
inputs:             1 x U8 [4], scale 1, zero point 0
outputs:            1 x U8 [4], scale 1, zero point 0
operations:         1
layers:             1
cores:              1
VIP SRAM:           43,520 bytes
stored checksums:   3 / 3 valid
```

Candidate 2 was intentionally not run. PoC 5C had been designed to prove an
EVIS/CL compiler route; the observed OpenVX result answered a different but
valuable question and produced a candidate suitable for a separate
execution review. No byte-level compiler-determinism claim is made from one
artifact.

### Conclusion

This is the first repository-controlled graph compiled by the exact Wave
stack into a structurally valid Wave-targeted NBG. It proves a narrow
firmware-provided Erf export path.

## PoC 5D — hardware capabilities and selector recovery

The read-only probe SHA-256 was:

```text
6a1c0ac1af75c31de32e0b8d827bbde898b6a606919d5aef3987967ac459bfe9
```

The single query:

```text
run ID:          poc5d-hardware-caps-20260728
manifest SHA:    5ba0c96980b6eb66530e9f6cbd205c836debf399ec3ba4e0313754b0b126a6bb
duration:        72 ms
remote exit:     0
health changes:  none
```

Returned:

```text
evis1:                         0
evis2:                         1
subGroupSize:                  4
coreCount capability field:   0
supportVA40:                   0
supportStreamProcessor:        0
streamProcessorExecCount:      0
streamProcessorVectorSize:     0
ecoID:                         0x04000000
customerID:                    0x1000000e
```

The `coreCount` field in this capability record is not the NBG core count.
The generated Erf NBG records one core, and stock execution proves a live
accelerator. A zero in the separate graph-batch capability field must not be
reported as “zero NPU cores.”

Static recovery from the exact `libovxlib.so` found:

```text
default priority:
  SP(5), EVIS(4), CL(3), VX(2), CPU(1)

VX-first operation priority:
  SP(4), VX(3), EVIS(2), CL(1), CPU(0)
```

Erf has an operation-specific VX-first selector. SP is unavailable, so the
OpenVX implementation wins before EVIS or CL is attempted. The PoC 5C
selector result therefore did not imply that EVIS2 was absent.

## PoC 5E — SignalFrame and the missing compiler

### Fixed graph

```text
input:   FLOAT16 [32]
op:      SignalFrame(window_length=8, step=4, pad_end=0, axis=0)
output:  FLOAT16 [8,7]
```

SignalFrame is audio-relevant, has firmware-embedded EVIS and CL source
resources, and does not have the Erf VX-first override.

Compiler SHA-256:

```text
efb5047921574d67015da527c89df5a4321ad57ddf14ac8ffc8f13a70be0e37f
```

Run:

```text
run ID:          poc5e-signal-frame-compile-1-20260728
manifest SHA:    33529402eafa459bc369047849805363059f9675f8e757370f7691109a481887
duration:        86 ms
native exit:     5
health changes:  none
```

The exact path reported two shader-compile failures:

```text
Failed to compile vx shader. (err: FFFFFFED)
```

It then registered the CPU fallback and failed size-only NBG export. No NBG
or partial file was created.

Static call-path recovery showed:

1. `libovxlib.so` stores these kernels as source-format programs.
2. `_gpu_register` creates a source program and calls `vxBuildProgram`.
3. `libOpenVX.so` dynamically loads `libCLC.so`.
4. It resolves `gcLoadKernelCompiler`, `gcCompileKernel`, and
   `gcUnloadKernelCompiler`.
5. The Wave firmware has no `libCLC.so` in its normal system roots.

The firmware therefore contains EVIS/CL kernel source resources but omits
the matching runtime compiler required to build them.

## PoC 5F — version-adjacent CLC compatibility

Candidate:

```text
version:  6.4.18:886827
sha256:   a1e5d11cafb6d23c7c787d24a71a766782ea022346f1580797d892238bd99f14
Wave:     6.4.19:946407
```

Static linkage showed:

```text
undefined CLC symbols:        135
matching GAL/VSC exports:     1,580
missing vendor-like imports:  0
compiler exports present:     3 / 3
```

### PoC 5F-A — relocation only

```text
run ID:          poc5f-a-clc-loader-20260728
manifest SHA:    5b96a409b4be6cbd1775e8b97d706a2d92028daf79415a6b8bd753eb4d33a8df
probe SHA:       33efe6b0e4cf081e9b871fb12dd50654b52912cd566252caeba51883b72f8e79
duration:        45 ms
exports found:   3
exports called:  0
status:          PASS
```

This proved loader and relocation compatibility only.

### PoC 5F-B — compiler call

```text
run ID:          poc5f-b-signal-frame-clc-compile-1-20260728
manifest SHA:    6a445c75d1da84c4c4fcf8205f634a2ad86502d0c6942801e61ba585e89ec8fe
probe SHA:       883bb4aa3593af8d226053b0a3760f6974ab46c4516ed02f5e6953a194a4381a
duration:        209 ms
native exit:     134 / SIGABRT
status:          FAIL
```

Unlike PoC 5E, shader build advanced into EVIS and CL kernel-object setup.
Both paths then returned invalid references while adding/finalizing/removing
kernel parameters. CPU registration succeeded, `CompileToBinary` failed, and
process teardown reported:

```text
double free or corruption (out)
Aborted
```

The native alarm and host deadline did not fire. No NBG existed, the
short-lived process ended, no holder remained, and the device health
envelope was unchanged.

### Quarantine decision

The library remains evidence under its isolated `/tmp` path but is not a
usable compiler:

- do not call its compiler entry points again;
- do not preload it;
- do not place it in `/usr/lib` or `/lib`;
- do not use it for another graph or candidate;
- do not interpret relocation success as ABI compatibility.

The next shader-compiler route needs the exact `6.4.19:946407` AArch64 CLC
or a compatible offline VXC/NBG compiler for customer `0x1000000e`.

As of 2026-07-28, targeted searches of the local firmware/SDK corpus and
public source repositories found no exact `6.4.19:946407` CLC or matching
offline compiler package. This records the search result, not proof that no
such package exists.

## What PoC 5 proves

### Observed live

- The exact Wave identity can be queried from a repository-controlled
  process.
- TIM-VX can construct nodes through the stock stack.
- The Wave reports EVIS2, subgroup size 4, no stream processor, and no VA40.
- The exact stack emitted one untouched, target-native Erf NBG.
- All compile and query probes left the captured device state unchanged.

### Static verified

- The exact selector has default and VX-first priority tables.
- Erf uses the VX-first table.
- SignalFrame has embedded EVIS/CL source resources.
- The exact OpenVX shader path loads CLC compiler symbols dynamically.
- The firmware does not contain a system CLC library.

### Not proven

- Arbitrary ONNX/TFLite import.
- General built-in-op NBG export.
- A usable custom-source shader compiler.
- Compiler determinism for the one Erf candidate.
- Numeric execution of the generated graph; that is PoC 6/7 evidence.
- Learned noise suppression, streaming deadlines, or production integration.

## Evidence locations

Raw evidence is intentionally gitignored:

```text
artifacts/npu/runs/poc5b-device-compile-1-20260728/
artifacts/npu/runs/poc5b-device-compile-1b-20260728/
artifacts/npu/runs/poc5b-device-compile-diagnostic-20260728/
artifacts/npu/runs/poc5c-erf-compile-1-20260728/
artifacts/npu/runs/poc5d-hardware-caps-20260728/
artifacts/npu/runs/poc5e-signal-frame-compile-1-20260728/
artifacts/npu/runs/poc5f-a-clc-loader-20260728/
artifacts/npu/runs/poc5f-b-signal-frame-clc-compile-1-20260728/
```

Reviewed step plans are under `docs/superpowers/plans/`. Native sources,
pinned builds, source/ELF policies, and non-overwriting staging scripts are
under `waved/tools/npu-compiler/`. Host manifests, harnesses, and parser
tests are under `waved/scripts/npu/`.

## Decision

The PoC 5C NBG graduated to a separate, hash-pinned execution review. No
other PoC 5 output was eligible for execution. General model work remains
compiler-limited above the driver: no raw galcore replay, target-byte patch,
stock-file replacement, service action, or reset is justified by these
results.
