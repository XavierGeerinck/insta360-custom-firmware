# Wave NPU PoC 5C: Embedded EVIS/OpenCL Compile-Only Gate

**Date:** 2026-07-28
**Status:** Closed after Candidate 1; valid OpenVX NBG recovered, intended
EVIS/CL path not selected, Candidate 2 intentionally skipped
**Authorization:** The device owner explicitly requested continued NPU
research in this session
**Depends on:** PoC 0 through PoC 3 passed; PoC 5A and PoC 5B closed as
documented negative results
**Safety class:** One fixed graph constructed and compiled through stock
TIM-VX/OpenVX/ovxlib/GAL userspace; no graph execution
**Program design:**
`docs/superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md`

## 1. Question

Can the Wave's exact `libovxlib.so` instantiate an embedded custom EVIS or
OpenCL kernel and export it as a target-native NBG, even though PoC 5B showed
that the stock built-in OpenVX Add path is rejected by `vxGenerateNBG`?

PoC 5C answers only the compiler-path question. A generated NBG is an
untrusted artifact and remains non-executable until a separate PoC 6 review
records its hash, structure, inputs, outputs, expected behavior, and execution
containment.

## 2. Evidence and Rationale

PoC 5B established the following on the live Wave:

1. The externally built TIM-VX layer creates a context and graph against the
   exact device libraries.
2. `gcoHAL_QueryChipIdentityEx` returns the live Wave identity:

   ```text
   chip model:       0x00009000
   chip revision:    0x00009002
   chip product ID:  0x05090001
   customer/target:  0x1000000e
   ECO ID:           0x04000000
   ```

3. TIM-VX graph setup reaches built-in Add node instantiation.
4. The first size-only `CompileToBinary` call fails inside the
   `vsi_nn_GenerateNBG` / `vxGenerateNBG` boundary.
5. No output, kernel warning, recovery event, retained `/dev/galcore` holder,
   boot change, or NPU parameter change occurred.

Static inspection of the exact Wave `libovxlib.so`, SHA-256
`09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9`,
adds an important distinction:

- the filesystem does not contain a loose custom-kernel source tree;
- `libovxlib.so` itself embeds an EVIS and OpenCL source resource table;
- the table includes `vsi_nn_kernel_header_vx`, `erf_vx`, and `erf_cl`;
- the library imports `vxCreateProgramWithSource`,
  `vxCreateProgramWithBinary`, and `vxBuildProgram`;
- the open TIM-VX source first resolves named embedded resources, then falls
  back to loose files only when a resource is absent;
- the EVIS Erf backend registers two source resources,
  `vsi_nn_kernel_header` and `erf`;
- the CL Erf backend registers `erf`;
- both backends support quantized `UINT8 -> UINT8`;
- the normal backend selector prefers EVIS over CL when EVIS is available.

This makes a custom GPU-kernel-backed operation materially different from the
built-in OpenVX Add control. It is also directly relevant to later audio
model lowering: elementwise activation, normalization, framing, matrix
multiplication, and related DSP kernels appear in the same embedded resource
corpus.

Whether this exact `libovxlib.so` chooses source or internal VXC binary format
at runtime is not assumed. The fixed verbose log is part of the experiment so
the selected backend and any program-build failure can be recorded without
loader interposition.

## 3. Fixed Graph

The native executable contains exactly this graph:

```text
input X:   UINT8 asymmetric, scale 1.0, zero point 0, shape [4]
operation: Erf
output Y:  UINT8 asymmetric, scale 1.0, zero point 0, shape [4]
```

Shape `[4]` is selected because it is the smallest clear vector aligned with
the EVIS Erf initializer's four-element horizontal work scale. The graph is
compile-only, so no input values are uploaded and no output values are read.

The tool accepts no:

- graph or model file;
- source text or binary kernel;
- operation name;
- dimensions;
- data type;
- quantization;
- tensor data;
- backend selector;
- build option;
- output path;
- retry count;
- device path;
- raw command or ioctl payload.

There is exactly one graph, one input tensor, one output tensor, one Erf
operation, and at most two `CompileToBinary` calls: one size query and one
in-memory export. There is no `Graph::Run`, tensor upload, tensor readback,
direct device-node open, direct ioctl, `dlopen`, loader interposition, loop,
or command-buffer construction.

## 4. Why Erf

Erf is chosen as a compiler-path discriminator, not as a target noise
suppression model:

- Add selected the stock OpenVX backend in PoC 5B.
- Erf is implemented by TIM-VX's custom-kernel selector.
- The exact firmware embeds both `erf_vx` and `erf_cl`.
- Its U8 mapping is explicitly present in the EVIS and CL kernel maps.
- It has no operation parameters, reducing ABI ambiguity.
- One input and one output keep NBG metadata review small.

A successful Erf export would prove that a named custom kernel from the exact
firmware resource corpus can pass setup, program creation/build, graph
verification, and NBG capture. It would not prove that arbitrary source can
be supplied, that every embedded kernel works, or that the resulting graph
executes correctly.

## 5. Exact Runtime Boundary

The probe must use only:

- the pinned AArch64 `libtim-vx.so`, SHA-256
  `d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b`;
- the exact Wave `/usr/lib/libovxlib.so`;
- the exact Wave `/usr/lib/libOpenVX.so`;
- the exact Wave `/usr/lib/libGAL.so`;
- the exact allowlisted transitive Wave libraries;
- the staged Vivante extension header, SHA-256
  `908d36ab616e27cabf5022bf16185825ce1c49f91fbaa37b98e67b4b6bb9af57`.

The only allowed environment is:

```text
PATH=/bin:/usr/bin
LC_ALL=C
LD_LIBRARY_PATH=/tmp/wave-npu-compile/lib:/usr/lib
VIVANTE_SDK_DIR=/tmp/wave-npu-compile/sdk
VSI_NN_LOG_LEVEL=5
```

`VSI_NN_LOG_LEVEL=5` is fixed from the first PoC 5C run. It is not a later
diagnostic retry. `VSIMULATOR_CONFIG`, `LD_PRELOAD`, and `LD_AUDIT` must be
unset and are rejected by the executable before context creation.

All new device files remain below `/tmp/wave-npu-compile/`. No stock library,
model, service file, firmware partition, `/data` file, sysfs value, debugfs
value, module, or device node is replaced or modified.

## 6. Fixed Interface and Versioned Files

The new executable accepts exactly:

```text
wave-npu-compile-erf-v1 --describe
wave-npu-compile-erf-v1 --candidate-1
wave-npu-compile-erf-v1 --candidate-2
```

The compile modes map internally to:

```text
/tmp/wave-npu-compile/output/wave-erf-candidate-1.nbg
/tmp/wave-npu-compile/output/wave-erf-candidate-2.nbg
```

Partial paths are fixed as:

```text
/tmp/wave-npu-compile/output/.wave-erf-candidate-1.nbg.partial
/tmp/wave-npu-compile/output/.wave-erf-candidate-2.nbg.partial
```

The PoC 5B binaries and outputs are separate evidence and must not be
overwritten, renamed, or used as input. Staging is hash-pinned and refuses to
replace any unexpected existing path.

Output publication uses a new partial file with
`O_CREAT | O_EXCL | O_NOFOLLOW`, complete write checks, `fchmod(0444)`,
`fsync`, `close`, and a hard-link into the final absent path. The tool never
deletes or replaces a final output. Its watchdog may unlink only the fixed
partial/final path selected for its own still-running invocation.

## 7. Identity and NBG Gates

Before graph construction, the probe must call the exact recovered ABI:

```c
gcoHAL_QueryChipIdentityEx(NULL, 0x20, &identity)
```

It must require:

```text
chip model  == 0x00009000
customer ID == 0x1000000e
```

The observed revision, product ID, ECO ID, reserved field, and flags are
printed as evidence but are not used to patch or synthesize a target.

After in-memory compilation and before filesystem publication, it must
require:

```text
NBG magic   == "VPMN"
NBG version == 0x00020000
NBG target  == 0x1000000e
12 <= NBG bytes <= 64 MiB
```

The host harness applies the stricter corpus-derived lower bound of 513 bytes
and strict-parses the NBG. For Erf, it must recover exactly:

```text
inputs:  1 × uint8, dimensions [4], byte size 4
outputs: 1 × uint8, dimensions [4], byte size 4
```

No target ID, NBG version, header, table, checksum, or command byte may be
patched.

## 8. Time and Process Bounds

The native executable installs before context creation:

- a 15-second `SIGALRM`;
- an async-signal-safe timeout handler using only `unlink`, `write`, and
  `_exit(124)`;
- `PR_SET_PDEATHSIG` with `SIGKILL`;
- a parent-liveness check;
- no retry and no loop.

The host harness adds a 20-second ADB deadline. Dynamic-loader constructors
run before `main`, so the host deadline covers the portion not covered by the
native alarm.

If either deadline fires, the experiment stops. The harness records
postflight evidence but does not issue a kill command, reset galcore, write
sysfs, stop a service, unload a module, reboot, or launch another compile.

## 9. Static Review Before Staging

The source and AArch64 PIE must pass all of these checks:

1. Exactly two textual `CompileToBinary` call sites.
2. No textual graph `Run` call.
3. No direct `ioctl`, `/dev/galcore`, or `dlopen`.
4. Exactly one `CreateOperation<tim::vx::ops::Erf>` graph operation.
5. Exactly one input and one output tensor binding.
6. Fixed shape `[4]`, U8 type, and fixed quantization.
7. Fixed output and partial paths.
8. AArch64 ELF64 PIE with RELRO, immediate binding, non-executable stack, and
   the exact runpath `/tmp/wave-npu-compile/lib:/usr/lib`.
9. Direct `NEEDED` entries for `libtim-vx.so` and `libGAL.so`.
10. Import of `gcoHAL_QueryChipIdentityEx`.
11. No imported `tim::vx::Graph::Run`, raw `ioctl`, or `dlopen`.
12. Reproducible build command using the pinned offline cross image.

The new binary is staged only after its final SHA-256 is placed into both the
staging script and the PoC 5C manifest.

## 10. Live Preflight

Before Candidate 1:

1. Confirm exactly one authorized ADB device:
   `PSP_0000000000000000`.
2. Confirm the exact kernel string.
3. Record and validate the boot ID.
4. Confirm `/dev/galcore` is character device `199:0`.
5. Confirm:

   ```text
   nanoqFreq = 838860800
   enableNN  = 255
   recovery  = 1
   ```

6. Confirm no process holds `/dev/galcore`.
7. Hash every required staged and system artifact.
8. Confirm all four fixed Erf output/partial paths are absent.
9. Capture a bounded recent dmesg baseline without clearing it.

Any mismatch refuses launch.

## 11. Candidate 1

Candidate 1 is one fresh process:

1. Create a new exclusive host evidence directory.
2. Save the strict preflight snapshot.
3. Invoke only `wave-npu-compile-erf-v1 --candidate-1`.
4. Save bounded stdout, stderr, transport status, remote status, and duration.
5. Save a postflight snapshot regardless of native success.
6. Search the new verbose output for the selected backend and kernel name.
7. If and only if the native result is a strict pass and health is unchanged,
   hash and pull the read-only candidate.
8. Strict-parse the candidate and validate its fixed I/O metadata.

Expected useful log evidence includes a selector result such as an EVIS or CL
node with an Erf kernel. The exact spelling is recorded, not predeclared as a
pass requirement, because it is an observation this PoC is intended to make.

## 12. Candidate 2

Candidate 2 is not a retry. It is a predeclared determinism control and may
run only if Candidate 1:

- exits normally before both deadlines;
- leaves the boot ID, libraries, NPU parameters, and device node unchanged;
- leaves no `/dev/galcore` holder;
- introduces no kernel alert or recovery event;
- reports the expected live identity;
- emits a valid, strict-parsed target-`0x1000000e` version-2 NBG;
- exposes a coherent custom-kernel backend in its verbose log.

Candidate 2 uses a fresh process and the second fixed output. The two NBGs are
then compared bytewise and by normalized parsed structure.

If Candidate 1 fails, Candidate 2 is not run in this PoC.

## 13. Immediate Stop Conditions

Stop without retry on:

- native timeout or host timeout;
- ADB disconnect;
- boot-ID change;
- changed allowlisted artifact;
- unexpected `/dev/galcore` holder;
- identity mismatch;
- context, graph, tensor, Erf, program build, or export failure;
- no coherent custom-kernel selector evidence;
- NBG magic, version, target, size, or I/O mismatch;
- pre-existing or malformed output;
- new galcore warning, fault, timeout, MMU error, command exception, hang, or
  recovery event;
- changed `nanoqFreq`, `enableNN`, or `recovery`;
- incomplete postflight evidence.

Permitted follow-up after a stop is read-only inspection and documentation.
No second diagnostic reproduction is pre-authorized because verbose logging
is already enabled from Candidate 1.

## 14. Acceptance and Interpretation

PoC 5C passes only when:

- all static policy checks pass;
- Candidate 1 and Candidate 2 both pass every live and host gate;
- logs establish a custom EVIS or CL Erf kernel path rather than the built-in
  OpenVX Add path;
- both untouched NBGs strict-parse as the fixed one-input/one-output graph;
- their normalized structures match;
- any byte differences are bounded and explained;
- device health remains unchanged.

A pass proves a legitimate route from one firmware-embedded custom kernel to
a Wave-targeted NBG. It does not yet prove arbitrary custom-source ingestion,
general operator coverage, numerical correctness, real-time suitability, or
safe execution.

A clean failure after custom kernel instantiation is also useful: it narrows
the blocked boundary to program source/binary loading, program build, graph
verification, or NBG capture. The exact last log stage must be documented
without claiming more precision than the evidence supports.

## 15. Deliverables

- this reviewed PoC 5C plan;
- separate fixed Erf compiler source;
- reproducible AArch64 build script;
- source and ELF policy checks;
- versioned hash-pinned staging script;
- immutable PoC 5C manifest;
- fixed harness profile and tests;
- Candidate 1 evidence;
- Candidate 2 evidence only if Candidate 1 passes;
- pulled NBGs only on strict native pass;
- strict parser reports and determinism comparison on two passes;
- a lab notebook recording positive and negative results;
- corrected compute-hardware documentation distinguishing absent loose
  sources from embedded ovxlib source resources;
- an updated program status and compiler/operator evidence matrix.

## 16. Live Result

Candidate 1 ran once under run ID
`poc5c-erf-compile-1-20260728`.

The native process:

- exited `0` in 141 ms;
- reported the expected live Wave identity;
- emitted an untouched 5,840-byte NBG;
- wrote it read-only at the fixed Candidate 1 path;
- produced SHA-256
  `6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee`;
- left the boot ID, NPU parameters, runtime hashes, `/dev/galcore` holders,
  and bounded kernel diagnostics unchanged.

Strict parsing recovered:

```text
magic:              VPMN
format:             v2-directory
target:             0x1000000e
size:               5840
core count:         1
layer count:        1
absolute op count:  1
input:              uint8 [4], affine-asymmetric scale 1.0, zero point 0
output:             uint8 [4], affine-asymmetric scale 1.0, zero point 0
VIP SRAM:           43520 bytes
section checksums:  3/3 valid
```

The decisive verbose line was:

```text
Instance OPENVX node with kernel "erf"
```

The exact selector's type-to-string function maps this spelling to
`VSI_NN_KERNEL_TYPE_VX`. It does not mean EVIS and must not be reported as an
embedded shader result. Candidate 1 therefore proves a second compiler route
that can emit a Wave NBG, but it does not meet this PoC's EVIS/CL acceptance
criterion.

Per sections 12 and 13, Candidate 2 was not run. The next bounded experiment
queries `vxQueryHardwareCaps` directly to determine `evis1`, `evis2`,
`subGroupSize`, virtual-address width support, stream-processor support, and
core count before deciding whether an EVIS/CL compile attempt is technically
valid.
