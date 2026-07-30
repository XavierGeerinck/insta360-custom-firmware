# Wave NPU Architecture and Compiler-Profile Evidence

This directory contains PoC 9A's host-only architecture profile, PoC 9C's
recovery of the available x86 simulator configuration parser, PoC 9D's
trusted compiler-package acquisition and static qualification, and PoC
14B's exact elementwise type-checker correlation.

PoC 9A:

- [`wave-profile-evidence.json`](wave-profile-evidence.json) is the strict,
  generated fact ledger;
- [`wave-profile.schema.json`](wave-profile.schema.json) is its independent
  JSON Schema;
- [`arch-profile-ghidra-evidence.json`](arch-profile-ghidra-evidence.json)
  contains the bounded decompilation and call-site evidence;
- the
  [PoC 9A notebook](../lab-notebook/2026-07-28-poc9a-architecture-database-recovery.md)
  explains the recovery and its limits.

PoC 9C:

- [`compiler-profile-ghidra-evidence.json`](compiler-profile-ghidra-evidence.json)
  contains focused, bounded x86 parser decompilation;
- [`vsim-parser-probe-results.json`](vsim-parser-probe-results.json) records
  13 isolated parser cases;
- [`viv-gpu-file-grammar.json`](viv-gpu-file-grammar.json) separates the
  proven simulator grammar from the missing no-kernel consumer;
- [`wave-viv-gpu-file.candidate.config`](wave-viv-gpu-file.candidate.config)
  is a deterministic parser-shaped candidate;
- [`wave-viv-gpu-file.candidate.manifest.json`](wave-viv-gpu-file.candidate.manifest.json)
  explicitly refuses compiler, graph, and device use;
- the
  [PoC 9C notebook](../lab-notebook/2026-07-28-poc9c-viv-gpu-file-parser-recovery.md)
  documents the grammar, catalog, isolation envelope, and remaining blocker.

PoC 9D:

- [`compiler-acquisition-evidence.json`](compiler-acquisition-evidence.json)
  is the deterministic archive, OCI, version, profile, parser, and
  qualification ledger;
- [`compiler_acquisition_targets.json`](../../../waved/scripts/npu/compiler_acquisition_targets.json)
  pins the complete acquisition and refusal contract;
- [`no-kernel-sdk-acquisition-checklist.md`](no-kernel-sdk-acquisition-checklist.md)
  is the shareable vendor/package request and intake gate;
- the
  [PoC 9D notebook](../lab-notebook/2026-07-28-poc9d-compiler-acquisition-static-qualification.md)
  documents each acquisition and static-recovery gate.

PoC 14B:

- [`affine-elementwise-compiler-profile.json`](affine-elementwise-compiler-profile.json)
  records all 59 ADD/Multiply input-type pairs from the pinned source,
  correlates the live `ASYM UINT8, SYMM PC INT8` rejection, and keeps
  affine semantic emission disabled;
- [`affine_compile_profile.py`](../../../waved/scripts/npu/affine_compile_profile.py)
  deterministically regenerates the profile from the exact source/runtime
  hashes and retained raw A1 log;
- the
  [PoC 14B notebook](../lab-notebook/2026-07-29-poc14b-affine-compiler-negative.md)
  documents the fixed graph, call ledger, clean negative, cleanup, and
  implications for ONNX placement.

The `.candidate.config` file is not a compiler-qualified Acuity profile.
PoC 9D found a vendor-authored exact Wave config and genuine
`VIV_GPU_FILE` consumers, but those consumers are standalone
`6.4.18:967862` shader compilers. The Acuity graph-to-NBG route still uses
`VSIMULATOR_CONFIG` with an embedded `6.4.6:769531` simulator SDK. No
qualifying no-kernel NBG compiler is present.

## Result

The exact Wave performance-model library has a 457-entry architecture table.
Its one customer-`0x1000000e` row is:

```text
key                 table value   live value
chip ID             0x00009000    0x00009000
chip version        0x00009001    0x00009002
product ID          0x05090001    0x05090001
eco ID              0x04000000    0x04000000
customer ID         0x1000000e    0x1000000e
```

`CreateAPModel` first searches for an exact five-word match. If that fails,
it searches again using:

```c
input_chip_id == row_chip_id
&& ((input_chip_version ^ row_chip_version) & 0x0000fff0) == 0
&& input_product_id == row_product_id
&& input_eco_id == row_eco_id
&& input_customer_id == row_customer_id
```

The live `0x9002` revision therefore belongs to the static row's `0x9001`
revision family. The profile preserves both values separately. It does not
rewrite the live revision to match the table.

The x86 simulator database independently contains the same five-word row and
the same masked-revision lookup. Its table is not ABI-compatible with the
Wave:

| Property | Exact Wave AArch64 | x86 simulator |
|---|---:|---:|
| Table records | 457 | 387 |
| Record bytes | 576 | 552 |
| Matching row | 320 | 290 |
| Common prefix | words 0..113 except build-specific word 6 | same |
| First layout divergence | word 114 | word 114 |

The complete matching rows are not expanded in prose. Their hashes and exact
file locations are stored in the profile:

```text
Wave row SHA-256:
  85b860a40fbbc3663ea2c2b13dd4f81ed84ed0643d3e09fa3cee52ea7177f831

x86 row SHA-256:
  c8483d5250b4c6fb0ab55a4d4620cdd19862121cdf6d89b93ecd626a47e68882
```

## PoC 9C parser result

The x86 simulator has a separate 358-entry, 100-byte configuration catalog.
Its unique customer-`0x1000000e` record is:

```text
VIP9000NANOS_PID0X1000000E
model=0x00009000 revision=0x00009001
product=0x05090001 eco=0x04000000 customer=0x1000000e
```

The complete file parser performs five sequential `%x` scans:

```text
chipModel
chipRevision
productID
ecoID
customerID
```

An isolated x86 probe confirmed that the parser accepts both the database
revision `0x9001` and live revision `0x9002`. It also confirmed unsafe vendor
edge behavior: EOF can preserve partial values, long lines are unbounded,
trailing content is ignored, and an identity lookup failure is logged but
can remain selected.

The host reimplementation rejects all of those ambiguous inputs. Its
canonical Wave rendering is:

```text
chipModel              = 0x00009000;
chipRevision           = 0x00009002;
productID              = 0x05090001;
ecoID                  = 0x04000000;
customerID             = 0x1000000e;
```

This rendering advances profile recovery, but it does not remove the need to
acquire and reverse the dedicated no-kernel SDK 6.4.22-or-newer consumer.

## PoC 9D acquisition result

The Radxa-linked Allwinner `ubuntu-npu:v2.0.10.2` distribution supplies the
vendor-authored file:

```text
VIP9000NANOS_PID0X1000000E.config
SHA-256:
  4baa78fdfb2288e7c87f1acd77e3bb1a3ea18eba5b4ec33a8f16df95063a125d
```

It independently matches the five-word Wave database identity recovered in
PoC 9A. Its database revision is `0x9001`, and the observed live revision
`0x9002` remains a distinct member of the same masked revision family.

The package also supplies two static x86-64 `VIV_GPU_FILE` consumers:

| Binary | Role | Version | Emits graph NBG |
|---|---|---|---|
| `vcompiler` | GLSL shader compiler | `6.4.18:967862` | no |
| `vcCompiler` | OpenCL shader compiler | `6.4.18:967862` | no |

The `vcompiler` parser is byte-locked at virtual address `0x007a53e0`. It
performs the same five ordered `%x` reads as PoC 9C and calls the feature
database at `0x00777890`. This validates the grammar for a real no-kernel
consumer, but not for the missing graph-to-NBG backend.

The acquired Acuity `6.30.22` wheel:

- contains no `VIV_GPU_FILE` occurrence;
- configures `VSIMULATOR_CONFIG` and `VIVANTE_SDK_DIR`;
- generates a native `gen_nbg` helper and expects `network_binary.nb`;
- embeds simulator libraries versioned `6.4.6:769531`;
- does not expose a proven, compatible no-kernel Wave NBG route.

PoC 9D therefore closes with:

```text
trusted provenance:            pass
archive/OCI integrity:         pass
exact Wave vendor profile:     pass
VIV_GPU_FILE parser:           pass-limited
SDK >= 6.4.22:                 fail
no-kernel NBG consumer:        fail
compiler I/O ownership ABI:    fail
fixed Conv compile:            blocked
compiler/device call count:    0
```

## Recovered contracts

### Lookup tuple

The APM input and table use five consecutive 32-bit words:

| Word | Byte offset | Meaning |
|---:|---:|---|
| 0 | `0x00` | chip ID |
| 1 | `0x04` | chip version |
| 2 | `0x08` | product ID |
| 3 | `0x0c` | eco ID |
| 4 | `0x10` | customer ID |

An unknown tuple is not accepted permissively. Both library variants print
all five values and enter `__assert_fail`.

### `archHAL_CHIPIDENTITY`

`archShowConfigurations` reads a different display structure:

| Byte offset | Printed name |
|---:|---|
| `0x00` | `chipModel` |
| `0x04` | `chipRevision` |
| `0x08` | `productID` |
| `0x0c` | `customerID` |
| `0x10` | `ecoID` |

The eco/customer order differs from the APM lookup tuple. Consumers must not
cast one layout to the other.

### Exact-build structure bounds

`archSwLibInit` and `CreateAPModel` expose the exact copy/allocation sizes:

| Structure | Wave AArch64 | x86 simulator |
|---|---:|---:|
| `arch_nn_config` | `0x1a0` / 416 | `0x198` / 408 |
| `arch_drv_option` | `0x2088` / 8,328 | `0x268` / 616 |
| `archHAL_CHIPIDENTITY` | `0x20` / 32 | `0x20` / 32 |
| `archNN_DATABASE_FEATURE` | `0x160` / 352 | `0x148` / 328 |
| `APM_IN_PARAM_T` | `0x128` / 296 | `0x0f0` / 240 |
| internal APM hardware info | `0x0e8` / 232 | `0x0b0` / 176 |

These sizes are evidence of private exact-build ABIs. They are not promises
from VeriSilicon, and the large driver-option difference is a direct warning
against copying the x86 layout onto the Wave.

### Live capability baseline

Previously reviewed, read-only PoCs established:

| Capability | Value |
|---|---:|
| EVIS1 | false |
| EVIS2 | true |
| subgroup size | 4 |
| VA40 | false |
| stream processor | false |
| eco ID | `0x04000000` |
| customer/target | `0x1000000e` |

PoC 9A imports those immutable notebook facts. It does not repeat the device
query.

## Explicit unknowns

The profile refuses to infer these from plausible-looking integers in a
private row:

- complete named bug/errata feature vector;
- physical NPU core count;
- usable core count per precision;
- VIP SRAM bank/core/reserved partitioning;
- AXI SRAM budget available to a generated graph;
- compiler tensor/cache-line alignment constraints;
- architecture-model, software-tiling, and VIP7 version values.

These are not optional details for a compiler. They are inputs to later ABI
and configuration-parser recovery.

## Evidence model

Every fact has:

```text
value
representation
evidence_class
confidence
sources
validation_state
notes
```

Evidence classes are:

- `observed-live`;
- `static-verified`;
- `differentially-verified`;
- `inferred`;
- `unknown`.

Validation states are:

- `corroborated`;
- `single-source`;
- `candidate`;
- `unresolved`.

Unknown facts must have a null value, `confidence: none`, no sources, and
`validation_state: unresolved`. The root `unknowns` array must exactly equal
the set of unknown facts. Unknown keys, duplicate JSON keys, path escapes,
unknown artifacts, duplicate source references, malformed hashes, and a
target other than `0x1000000e` are rejected.

## Reproduction

All commands below are host-only:

```bash
python3 -m unittest waved.scripts.npu.tests.test_arch_profile

python3 waved/scripts/npu/arch_profile.py validate-targets

python3 waved/scripts/npu/arch_profile.py generate-ghidra

python3 waved/scripts/npu/arch_profile.py generate-profile

python3 waved/scripts/npu/arch_profile.py validate-profile

python3 waved/scripts/npu/arch_profile.py verify-generated
```

The expensive deterministic check reruns all four bounded Ghidra analyses:

```bash
python3 waved/scripts/npu/arch_profile.py verify-generated --ghidra-only
```

PoC 9C's static and isolated-host workflow is:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_compiler_profile

python3 waved/scripts/npu/compiler_profile.py validate-targets
python3 waved/scripts/npu/compiler_profile.py generate-ghidra
python3 waved/scripts/npu/compiler_profile.py probe-vendor-parser
python3 waved/scripts/npu/compiler_profile.py recover-grammar
python3 waved/scripts/npu/compiler_profile.py generate-candidate
python3 waved/scripts/npu/compiler_profile.py verify-generated
```

PoC 9D's fully inert acquisition check is:

```bash
python3 -m unittest \
  waved/scripts/npu/tests/test_compiler_acquisition.py

python3 waved/scripts/npu/compiler_acquisition.py validate-targets
python3 waved/scripts/npu/compiler_acquisition.py generate-evidence
python3 waved/scripts/npu/compiler_acquisition.py verify-generated
```

This rehashes the outer and inner ZIPs, the complete OCI TAR, all 15 OCI
layers, every selected binary, and the Acuity archives. It never loads or
runs the OCI image, imports a native extension, executes a compiler, or
contacts the Wave.

`probe-vendor-parser` loads only the pinned x86 emulator and its pinned
`libvdtproxy.so` dependency. Every fixed case gets a fresh, networkless,
read-only, resource-limited amd64 container. It does not load any compiler
library.

The generator:

1. rejects target manifests with anything other than the exact four roles;
2. rehashes every ELF before analysis;
3. verifies ELF64 little-endian program headers;
4. maps the statically demonstrated table virtual address to file bytes;
5. requires exactly one `0x1000000e` row in each performance database;
6. verifies the full five-word identity tuple and generic-row marker;
7. checks the 114-word differential prefix;
8. hashes each complete matched row;
9. imports only hash-pinned prior live evidence;
10. validates and atomically writes canonical JSON.

It never imports `ctypes`, calls `dlopen`, invokes an AArch64 function,
connects to ADB, opens `/dev/galcore`, constructs an OpenVX/TIM-VX graph, or
calls a compiler.

## How later code should consume this

The evidence profile is not a free-form configuration object. A later
compiler adapter should:

1. require the exact `profile_id`, artifact hashes, runtime version, and
   target;
2. refuse if any compiler-required fact remains unknown;
3. render a candidate configuration only through a separately recovered and
   tested grammar;
4. run the compiler in an isolated host process with fixed inputs and a
   deadline;
5. validate any output NBG structurally before a device plan is considered;
6. record the profile and compiler hashes in the output manifest.

The ONNX layer should remain independent. ONNX Runtime can provide broad
model parsing and CPU execution while an audited VSINPU partition is
eligible for the NPU. General ONNX support does not require pretending that
every ONNX node is NPU-capable.

## Next gates

PoC 9B is a separate ABI exercise. It may only call an exported architecture
API after the private input/output layouts are statically proven and a
single reviewed call plan is accepted. It must compare the rendered result
against this profile and stop on any disagreement.

PoC 9C recovered and tested the available simulator parser. PoC 9D then
corroborated the grammar in a real `VIV_GPU_FILE` shader consumer and found
the vendor's exact Wave config, while proving that the graph-to-NBG consumer
is still absent from the acquired package.

A fixed-Conv compiler control may be planned only after acquiring a
dedicated x86-64 no-kernel SDK `6.4.22` or newer whose OpenVX/ovxlib NBG path
directly consumes `VIV_GPU_FILE`, plus its complete dependency, licence,
version, input/output, and allocation-ownership contracts. The quarantined
adjacent CLC and the embedded `6.4.6` simulator are not alternative routes.

The CPU side can proceed independently: ONNX validation, CPU execution,
capability reporting, partition refusal, telemetry, and fallback do not
require a compiler call. A useful noise-cancelling model should remain
CPU-correct first, then gain NPU islands one operator family at a time.
