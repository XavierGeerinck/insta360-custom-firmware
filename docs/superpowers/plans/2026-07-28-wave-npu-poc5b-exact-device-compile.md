# Wave NPU PoC 5B: Exact-Device Compile-Only Gate

**Date:** 2026-07-28
**Status:** Complete — OpenVX Add selected; NBG export cleanly failed
**Authorization:** The device owner explicitly requested that the NPU work
continue in this session.
**Depends on:** PoC 0 through PoC 3 passed; PoC 5A closed as a safe negative
**Safety class:** Bounded graph construction and compilation through the
stock userspace ABI; no graph execution
**Program design:**
`docs/superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md`

## 1. Question

Can the pinned, externally-built TIM-VX C++ layer construct one fixed Add
graph and ask the Wave's exact OpenVX/ovxlib/GAL stack to export a native NBG
for the live Wave target?

This PoC answers only the compiler question. It does not test whether the
result can be instantiated or executed.

## 2. Why This Is a Separate PoC

PoC 5A was intentionally host-only. It proved that the x86 simulator contains
a Wave identity profile, but its selector is applied after the simulator HAL
has materialized the default `0x10000020` identity. The host compiler
therefore cannot authentically emit a Wave graph through that path.

The exact Wave `libOpenVX.so` exports `vxGenerateNBG`, and the pinned
AArch64 `libtim-vx.so` delegates to the device's exact `libovxlib.so`. That
makes a compile-only device experiment technically distinct from simulator
target selection and from graph execution. It receives its own harness,
allowlist, watchdog, evidence directory, and stop conditions here.

## 3. Fixed Graph

The executable contains this graph in source. It accepts no model file,
operator name, dimensions, data type, quantization, input data, or output
path from the caller.

```text
input A:  UINT8 asymmetric, scale 1.0, zero point 0, shape [2]
input B:  UINT8 asymmetric, scale 1.0, zero point 0, shape [2]
operation: Add
output C: UINT8 asymmetric, scale 1.0, zero point 0, shape [2]
```

There is deliberately no tensor upload, graph `Run`, tensor readback, loop,
raw ioctl, device-node open, or command-buffer construction in the tool.

## 4. Identity Gate

Binary analysis of the exact Wave `libGAL.so` establishes:

- `gcoHAL_QueryChipIdentityEx` is a wrapper around an internal query;
- argument 2 must be the record size `0x20`;
- the returned 32-byte record begins with five 32-bit fields;
- those fields are model, revision, product ID, customer ID, and ECO ID;
- the customer ID is the target ID serialized into an NBG.

The relevant Wave values are:

```text
chip model:       0x00009000
database revision: expected 0x00009001
register revision: observed 0x00009002
product ID:       0x05090001
customer/target:  0x1000000e
ECO ID:           0x04000000
```

The database/register revision mismatch is already present in the stock
driver and is not itself a failure. Before graph construction, the probe must
require:

```text
chip model == 0x00009000
customer ID == 0x1000000e
```

After in-memory export and before filesystem creation, it must independently
require:

```text
NBG magic   == "VPMN"
NBG version == 0x00020000
NBG target  == 0x1000000e
12 <= NBG bytes <= 64 MiB
```

No target, version, checksum, table, or command-stream byte may be patched.

## 5. Exact Runtime Boundary

The staged probe uses:

- the pinned AArch64 `libtim-vx.so` built from TIM-VX commit
  `702715d714277376d7ff466b7b72833ff57564c1`;
- `/usr/lib/libovxlib.so` from the Wave;
- `/usr/lib/libOpenVX.so` from the Wave;
- `/usr/lib/libGAL.so` from the Wave;
- the exact transitive Wave libraries resolved by the stock dynamic loader;
- only `include/CL/cl_viv_vx_ext.h` in a disposable SDK directory if the
  compiler asks for the Vivante OpenCL extension header.

The experiment must verify every staged file and every required system DSO
against its allowlisted SHA-256 before launch. A missing or changed artifact
is a preflight failure, not an invitation to substitute a nearby SDK.

All new device files live below:

```text
/tmp/wave-npu-compile/
```

The experiment does not write `/data`, `/usr`, a stock model directory, a
service directory, sysfs, debugfs, a module path, or a raw device node.

## 6. Fixed Interface

The executable accepts exactly:

```text
wave-npu-compile-add --describe
wave-npu-compile-add --candidate-1
wave-npu-compile-add --candidate-2
```

The two compile modes map internally to:

```text
/tmp/wave-npu-compile/output/wave-add-candidate-1.nbg
/tmp/wave-npu-compile/output/wave-add-candidate-2.nbg
```

Unknown arguments, extra arguments, pre-existing outputs, symlink outputs,
and an unexpected `VIVANTE_SDK_DIR` are rejected before context creation.
Output files are created with `O_CREAT | O_EXCL | O_NOFOLLOW`, flushed, and
made read-only. The binary never deletes or replaces an output.

## 7. Time and Process Bounds

The device executable installs:

- a 15-second `SIGALRM` watchdog before context creation;
- an async-signal-safe timeout path using only `write` and `_exit(124)`;
- a parent-death signal so an abandoned ADB shell cannot leave the compiler
  running;
- one graph, three tensors, one operation, and two export calls;
- no retry and no loop.

The host harness adds a 20-second subprocess deadline. This is a second
containment layer, not permission to retry after a device-side timeout.

Dynamic-loader constructors run before `main`, so the internal alarm cannot
cover a constructor hang. The host deadline covers that gap. If the host
deadline fires, the harness attempts only to terminate the exact child
process; it does not reset the driver, reboot, unload a module, stop a
service, or launch another compile.

## 8. Preflight

Before each compile:

1. Confirm exactly one authorized ADB device is present.
2. Record the boot ID.
3. Confirm `/dev/galcore` exists with major/minor `199:0`.
4. Record `nanoqFreq`, `enableNN`, and `recovery`.
5. Confirm no unexpected process holds `/dev/galcore`.
6. Hash the probe, pinned `libtim-vx.so`, staged header, and exact Wave DSOs.
7. Confirm the selected fixed output does not exist.
8. Confirm the other candidate is not used as input.
9. Capture bounded recent kernel diagnostics without clearing them.

Any mismatch stops before process launch.

## 9. Run Sequence

### Candidate 1

1. Create a new, exclusive host run directory.
2. Save the preflight snapshot.
3. Launch the fixed `--candidate-1` command once.
4. Save bounded stdout, stderr, exit status, and duration.
5. Save a postflight snapshot even on failure.
6. If and only if the result line is a strict pass, hash and pull the
   read-only NBG.
7. Strict-parse the pulled file on the host.

### Candidate 2

Candidate 2 runs only if candidate 1:

- exits normally before both deadlines;
- leaves device health unchanged;
- emits the expected identity;
- writes a valid target-`0x1000000e`, version-2 NBG;
- produces no new kernel error or recovery event.

Candidate 2 uses a fresh process and the other fixed output path. It is not a
retry. It is the predeclared determinism control.

## 10. Immediate Stop Conditions

Stop the PoC without retry if any of these occurs:

- internal exit `124` or host timeout;
- ADB disconnect;
- changed boot ID;
- missing or changed allowlisted artifact;
- unexpected `/dev/galcore` holder;
- context, graph, tensor, operation, or export failure;
- HAL identity mismatch;
- NBG magic, version, target, or size mismatch;
- output pre-existence or filesystem error;
- new NPU recovery event;
- changed `enableNN` or recovery state;
- kernel warning, fault, timeout, MMU error, command exception, or hang;
- incomplete postflight snapshot.

After a stop, permitted follow-up is read-only evidence collection and
documentation. Driver reset, module action, reboot, service action, raw ioctl
replay, or another compile requires a new reviewed decision.

One diagnostic reproduction is separately allowed after a clean, fast
userspace failure when all pre/post health fields are unchanged. It must use
the identical executable, graph, output path, libraries, and timeouts, adding
only `VSI_NN_LOG_LEVEL=5` to expose the failing TIM-VX/ovxlib stage. It is not
a determinism candidate, may run only once, and cannot be followed by
Candidate 2 unless the original Candidate 1 acceptance gate later passes.

## 11. Acceptance

PoC 5B passes only when:

- source review confirms there is no graph execution path;
- ELF review confirms the executable is AArch64 and imports the expected
  compile APIs but no `tim::vx::Graph::Run`;
- the build container, compiler version, flags, source hash, executable hash,
  and DSO hashes are recorded;
- both fresh processes report the live Wave identity;
- both NBGs are emitted without post-processing;
- strict parsing accepts both as `VPMN`, version `0x00020000`, target
  `0x1000000e`;
- graph I/O metadata is consistent with the fixed graph;
- normalized structures match;
- byte differences, if any, are bounded and explained;
- pre/post snapshots show no device-health change.

Passing this PoC proves only that a legitimate Wave-targeted compile path
exists. The NBG remains non-executable until PoC 6 assigns its SHA-256 to a
separate, no-argument execution probe with known inputs, expected outputs,
timeouts, and health gates.

## 12. Deliverables

- device-mode compiler source;
- reproducible AArch64 build script;
- source and ELF policy checks;
- fixed staging script;
- dedicated compile-only harness and tests;
- two run envelopes and health snapshots;
- pulled NBG candidates;
- strict parser reports and deterministic comparison;
- a lab notebook with every observation, failure, and SHA-256;
- updated program status and compute-hardware documentation.

## 13. Result

Three bounded processes closed the ABI and Add-export questions:

```text
runner sha256:
  76638c6bcbbab15cd2f466519abc6817521cddda6854b9c6da9a1840f350f10a

initial identity self-check:
  run:      poc5b-device-compile-1-20260728
  duration: 81 ms
  exit:     15
  result:   original customer/ECO field order rejected before graph work

corrected compile:
  run:      poc5b-device-compile-1b-20260728
  duration: 82 ms
  exit:     5
  result:   size-only CompileToBinary returned false

verbose diagnostic:
  run:      poc5b-device-compile-diagnostic-20260728
  duration: 75 ms
  exit:     5
  selector: Instance OPENVX node with kernel "add"
```

No final or partial NBG was created. Every postflight retained the same boot
ID, device node, module parameters, required hashes, holder state, and
bounded kernel diagnostics.

The hypothesis failed at NBG serialization, not graph construction or backend
selection. The result justified PoC 5C's different firmware-provided Erf
operation; it did not authorize another Add shape or datatype.
