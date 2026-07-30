# PoC 9D — Compiler Acquisition and Static Qualification

**Date:** 2026-07-28
**Experiment ID:** `wave-poc9d-compiler-acquisition-v1`
**Safety class:** host-only, inert archive and binary inspection
**Disposition:** acquisition succeeded; NBG compiler qualification is blocked

## 1. Question

Can a trusted public vendor package supply all of the following for the
Insta360 Wave:

1. the exact `VIP9000NANOS_PID0X1000000E` hardware profile;
2. a genuine `VIV_GPU_FILE` consumer;
3. a no-kernel SDK at least as new as `6.4.22`;
4. an Acuity/TIM-VX graph path that emits a Wave-compatible NBG; and
5. a sufficiently proven input/output and ownership ABI to authorize one
   isolated fixed-Conv compilation?

This is the acquisition and static-qualification part of PoC 9D. It is not a
compiler-execution experiment.

## 2. Falsifiable hypothesis

> The Allwinner NPU development image linked by Radxa contains a compatible
> x86-64 no-kernel NBG compiler whose graph path directly consumes
> `VIV_GPU_FILE` and is new enough for the current TIM-VX no-kernel contract.

The hypothesis is false for the acquired `ubuntu-npu:v2.0.10.2` image:

- the exact Wave profile is present;
- genuine no-kernel `VIV_GPU_FILE` consumers are present;
- those consumers are standalone GLSL/OpenCL shader compilers, not the
  Acuity graph-to-NBG compiler;
- their SDK build is `6.4.18:967862`, below the required `6.4.22`;
- the Acuity graph wrappers instead select `VSIMULATOR_CONFIG`;
- their embedded simulator SDK is `6.4.6:769531`;
- the complete native `gen_nbg` ABI and ownership contract remains unproven.

The correct outcome is therefore `blocked`, not “try it and see.”

## 3. Safety gate and action ledger

The experiment manifest prohibits:

```text
docker-load
docker-run
compiler-execution
device-access
quarantined-clc-load
```

Observed action counts:

| Action | Count |
|---|---:|
| Compiler calls | 0 |
| Container registrations | 0 |
| Container runs | 0 |
| Device calls | 0 |
| Generated NBG files | 0 |

The OCI image was never loaded into a container runtime. ZIP, TAR, OCI,
JSON, text, and ELF content was treated as untrusted inert data. Selected
members were copied to a static-analysis subset without constructing or
running the root filesystem.

The Wave device being available did not broaden the experiment. No ADB
connection, device node, firmware API, OpenVX graph, or driver operation was
used.

## 4. Evidence artifacts

Tracked inputs and output:

| Artifact | Purpose |
|---|---|
| `waved/scripts/npu/compiler_acquisition_targets.json` | exact source, archive, artifact, parser, and policy contract |
| `waved/scripts/npu/compiler_acquisition.py` | inert validator and evidence generator |
| `waved/scripts/npu/tests/test_compiler_acquisition.py` | archive, literal, ELF, call, and version-gate tests |
| `docs/npu/profile/compiler-acquisition-evidence.json` | canonical generated result |

The generated evidence SHA-256 is:

```text
26ff665a47f5af4afda71061a3e977fbc5dc1403f36cfe15f70bc1b051589b45
```

The target manifest SHA-256 embedded in that evidence is:

```text
bf89032387cd900d24b28feccd0d3876cfb7195359aeab3bfa5473b45ca9ef10
```

Raw, large acquisition artifacts remain under:

```text
artifacts/npu/compiler-acquisition/
```

That directory is intentionally ignored by Git.

## 5. PoC 9D.0 — Trusted acquisition

### 5.1 Source

Radxa's Cubie A7S NPU setup guide links the Allwinner development-image
distribution. The package was retrieved on 2026-07-28:

```text
sharing page:
  https://netstorage.allwinnertech.com:5001/sharing/Mh23BhPHq

download:
  https://netstorage.allwinnertech.com:5001/fsdownload/Mh23BhPHq/docker_images_v2.0.x.zip

HTTP Content-Length:
  2903273080

HTTP Last-Modified:
  Tue, 09 Jun 2026 07:25:12 GMT

HTTP ETag:
  "6a27bfd8-ad0c6e78"
```

The official public references used for the compatibility gate were:

- [Radxa Acuity environment guide](https://docs.radxa.com/en/cubie/a7s/app-dev/npu-dev/cubie-acuity-env);
- [Radxa Acuity usage guide](https://docs.radxa.com/en/cubie/a7s/app-dev/npu-dev/cubie-acuity-usage);
- [VeriSilicon TIM-VX platform sample](https://github.com/VeriSilicon/TIM-VX/blob/main/samples/platform_sample/README);
- [VeriSilicon Acuity repository](https://github.com/VeriSilicon/acuitylite);
- [AcuityLite package index](https://pypi.org/project/acuitylite/).

The Radxa prose still names an older image release, while the linked public
share contained `v2.0.10.2`. The acquired bytes, not the stale prose version,
define this experiment.

### 5.2 Archive chain

| Level | Member | Bytes | SHA-256 |
|---|---|---:|---|
| Outer ZIP | `allwinner-docker_images_v2.0.x.zip` | 2,903,273,080 | `d876a96a74905b92d09ae5681256dd5fc6c430b0ebbd612097142473ec87e287` |
| Nested ZIP | `docker_images_v2.0.x/ubuntu-npu_v2.0.10.2.tar.zip` | 2,898,787,419 | `c804d08e6499883b51b32443c7e470fbf03eb894e08946e7326fcca0c628f5f8` |
| OCI TAR | `ubuntu-npu_v2.0.10.2.tar` | 7,883,452,416 | `7d150a626e588cdae872e700a1616f9ce831c97ce57765e1c3baf0a7591641d6` |

Archive member checks:

```text
outer selected-member CRC-32: 7301124e
inner selected-member CRC-32: 411aa1c1
outer member count:            8
inner member count:            1
```

The decompressed outer selected member hashes to the pinned nested ZIP
SHA-256, and the decompressed nested selected member hashes to the pinned OCI
TAR SHA-256. The evidence chain therefore does not rely on CRC-32 for
identity.

The validator rejects absolute paths, `..`, `.`, repeated separators,
Windows-drive paths, backslashes, NULs, duplicate ZIP/TAR names, encrypted
ZIP members, ZIP symlinks, TAR special devices, and escaping TAR links.

## 6. PoC 9D.1 — OCI image inspection without execution

Recovered image identity:

```text
reference:     docker.io/library/ubuntu-npu:v2.0.10.2
created:       2026-06-09T14:37:11.046404394+08:00
architecture:  amd64
OS:            linux
layers:        15
```

Pinned OCI identities:

```text
manifest:
  sha256:36b2a89e016f2cf10821b2434ebc1abb95269d15a930c53301630406c0b9c5df

config:
  sha256:724759a03c224d102768586c3a051891a2281c56d838c0abf87fc2499637ba99
```

The generator independently rehashed the config, manifest, and every one of
the 15 layer blobs while streaming them from the OCI TAR. It did not trust
the filenames as proof of their digest.

The toolchain material is primarily introduced by uncompressed layer:

```text
sha256:df1922a4414b9ff879444d62aa99da6c6cb17bc441f66c6250b3355875300c00
bytes: 3499751424
```

The preceding common layer is:

```text
sha256:a88ddfd3ff780f6335e22d6aafa5ca2d26e7bed32d48ed48d3a669985be7f170
bytes: 356904448
```

The last two tiny compressed layers contain shell-history or dotfile
updates; they do not change the recovered compiler conclusion.

## 7. PoC 9D.2 — Version and component inventory

The vendor readme names:

```text
Acuity:      6.30.22
Vivante IDE: 5.11.0
```

The IDE records are more specific:

```text
IDE5.11.0_CL CMDSRC@OL_CLbebe73d_6.4.x_6.4.18_24Q1_CL83666f8
Generate NBG version: 0x00010020
```

The standalone compilers contain:

```text
$VERSION$6.4.18:967862$
```

The Acuity wheel embeds:

```text
SDK version: 6.4.6:769531
```

The resulting matrix is:

| Component | Architecture | Role | Profile input | Version | NBG emitter |
|---|---|---|---|---|---|
| `vcompiler` | x86-64 static ELF | GLSL shader compiler | `VIV_GPU_FILE` | `6.4.18:967862` | no |
| `vcCompiler` | x86-64 static ELF | OpenCL shader compiler | `VIV_GPU_FILE` | `6.4.18:967862` | no |
| Acuity ovxlib wrapper | x86-64 extension | generates/builds native NBG helper | `VSIMULATOR_CONFIG` | Acuity `6.30.22` | indirect |
| Acuity TIM-VX wrapper | x86-64 extension | generates/builds native NBG helper | `VSIMULATOR_CONFIG` | Acuity `6.30.22` | indirect |
| embedded `vsi_sdk` | x86-64 shared libraries | simulator backend | `VSIMULATOR_CONFIG` | `6.4.6:769531` | simulator path |
| Wave firmware runtime | AArch64 shared libraries | live runtime | device/driver | `6.4.19.5.946407` | narrow exact-device path only |
| adjacent quarantined CLC | AArch64 shared library | source compiler candidate | none proven | `6.4.18:886827` | incompatible; never call |

These versions are different compatibility domains. A newer Acuity Python
package number does not imply a newer embedded Vivante SDK.

As a separate static cross-check, the public
`acuitylite-6.57.1-cp310-cp310-manylinux2010_x86_64.whl`
was inspected without importing it:

```text
bytes:   246671312
sha256:  bc687bc67aa475e97f174dfa7bbbab05c1872ed116f5954810da858ce72c99b8
```

Its wrapper still generates a native `gen_nbg` program, configures
`VSIMULATOR_CONFIG`/`VIVANTE_SDK_DIR`, and moves `network_binary.nb`. Its
embedded SDK remains in the older simulator family and contains no recovered
`VIV_GPU_FILE` consumer. This corroborates the boundary but is not part of
the canonical Allwinner evidence manifest.

## 8. PoC 9D.3 — Exact Wave vendor profile

The vendor image contains:

```text
Vivante_IDE/VivanteIDE5.11.0/cmdtools/common/cfg/
  VIP9000NANOS_PID0X1000000E.config
```

Pinned artifact:

```text
bytes:   140
sha256:  4baa78fdfb2288e7c87f1acd77e3bb1a3ea18eba5b4ec33a8f16df95063a125d
```

Parsed identity:

| Field | Vendor database profile | Prior live observation |
|---|---:|---:|
| `chipModel` | `0x00009000` | `0x00009000` |
| `chipRevision` | `0x00009001` | `0x00009002` |
| `productID` | `0x05090001` | `0x05090001` |
| `ecoID` | `0x04000000` | `0x04000000` |
| `customerID` | `0x1000000e` | `0x1000000e` |

This independently corroborates PoC 9A. The vendor file uses database
revision `0x9001`, while the live device reports `0x9002`. PoC 9A proved the
architecture lookup accepts both through:

```c
((input_revision ^ database_revision) & 0x0000fff0) == 0
```

The relationship is now stronger than a reconstructed candidate: an
appropriately named exact Wave config exists in a trusted vendor image.
However, the file is only qualified for a compiler after the relevant
compiler consumer, version, and ABI are qualified. A correct-looking config
cannot make an unrelated backend safe.

## 9. PoC 9D.4 — Reverse engineering the real `VIV_GPU_FILE` parser

### 9.1 Consumer identity

`VIV_GPU_FILE` occurs in the two standalone static compilers:

```text
vcompiler
  bytes:   22490520
  sha256:  dfa6a751e472184238915b4c0e20edeec0ca4a98f02ea07b887b0eae2f55e263

vcCompiler
  bytes:   23573768
  sha256:  719b1811cd19dba80397ffb695ef9a78e8f4b74d1a3c39e6119cb9b208e18dd2
```

Both are x86-64 ELF64 files with no `PT_INTERP` and no `PT_DYNAMIC`, so this
specific executable boundary has no dynamic dependency closure. Neither
binary was executed.

Usage strings classify their inputs:

```text
vcompiler: GLSL shader source
vcCompiler: OpenCL program source
```

Their no-kernel diagnostics refer to shader/program memory, feature database
selection, and no-kernel allocation. Neither advertises Acuity graph input or
NBG graph output.

### 9.2 Byte-locked parser

The recovered `vcompiler` parser is pinned at:

```text
virtual address: 0x007a53e0
file offset:     0x003a53e0
byte count:      800
SHA-256:         d8ce3160f273f0719d56d46cff82c384ba81e608251ad19bdba63fa691a486d4
```

Three independently decoded relative calls target it:

```text
0x007a64ee -> 0x007a53e0
0x007a66ee -> 0x007a53e0
0x007a67ed -> 0x007a53e0
```

The parser reads the five formats in this order:

| Field | Format-string file offset | Destination VA |
|---|---:|---:|
| `chipModel` | `0x0132f59a` | `0x01b88c64` |
| `chipRevision` | `0x0132f5b8` | `0x01b88c70` |
| `productID` | `0x0132f5d6` | `0x01b88c6c` |
| `ecoID` | `0x0132f5f4` | `0x01b88c60` |
| `customerID` | `0x0132f612` | `0x01b88c68` |

The feature-database lookup call is:

```text
call:   0x007a55ab
target: 0x00777890
```

Recovered behavior, expressed as pseudocode rather than a claimed source
reconstruction:

```c
path = getenv("VIV_GPU_FILE");
if (path == NULL || open(path) fails)
    report profile-open failure;

scan("chipModel              = %x;\n", &chip_model);
scan("chipRevision           = %x;\n", &chip_revision);
scan("productID              = %x;\n", &product_id);
scan("ecoID                  = %x;\n", &eco_id);
scan("customerID             = %x;\n", &customer_id);

row = lookup_database(
    chip_model,
    chip_revision,
    product_id,
    eco_id,
    customer_id
);

if (row == NULL)
    report all five values and ask the operator to check VIV_GPU_FILE;
```

The five-field order agrees with the PoC 9C simulator parser, but this result
is stronger in one narrow respect: it is recovered from an actual
`VIV_GPU_FILE` no-kernel consumer.

It is also narrower in the important operational respect: the consumer is a
shader compiler below the SDK version gate, not the Acuity graph compiler
needed to produce a Wave NBG.

## 10. PoC 9D.5 — Acuity graph-to-NBG boundary

The acquired Acuity wheel is:

```text
acuity-6.30.22-cp38-cp38-manylinux2010_x86_64.whl
bytes:   251665238
sha256:  70d8b36297e1cae785cb6f4e0022fc73afe01db343a3b880624447d09558393a
```

Static wrapper evidence establishes:

- both ovxlib and TIM-VX case generators contain
  `VSIMULATOR_CONFIG`;
- both contain `VIVANTE_SDK_DIR`;
- both name `network_binary.nb`;
- neither contains `VIV_GPU_FILE`;
- the wrapper does not directly serialize the NBG;
- generated source is built into a native `gen_nbg` helper;
- that helper produces or exposes `network_binary.nb`;
- the complete helper CLI, tensor input, error, memory-allocation, and output
  ownership ABI is not yet proven.

The full wheel scan found no `VIV_GPU_FILE` occurrence. The embedded SDK scan
found:

```text
VIV_GPU_FILE:                    no hits
VSIMULATOR_CONFIG:               libEmulator.so
VIP9000NANOS_PID0X1000000E:      libEmulator.so
6.4.6:769531:                    seven SDK libraries
```

This yields two distinct paths:

```text
shader source
  -> vcompiler / vcCompiler
  -> VIV_GPU_FILE
  -> shader/program object
  -> not an NBG graph

ONNX / Acuity graph
  -> Acuity exporter wrapper
  -> generated native gen_nbg
  -> VSIMULATOR_CONFIG + embedded SDK 6.4.6
  -> network_binary.nb
  -> simulator backend, not qualified for Wave
```

Finding both paths in one distribution must not be interpreted as proof that
the graph wrapper internally invokes the no-kernel shader compiler with the
Wave profile. Static searches and wrapper boundaries show the opposite.

## 11. Qualification gate result

| Gate | Result | Evidence |
|---|---|---|
| Trusted vendor provenance | pass | Radxa-linked Allwinner distribution; HTTP and nested hashes pinned |
| Archive integrity and safe paths | pass | ZIP CRC/geometry, OCI structure, all 17 OCI digests rehashed |
| Exact Wave vendor profile | pass | named `PID0X1000000E` config matches the five-word Wave database identity |
| `VIV_GPU_FILE` parser recovery | pass-limited | byte-locked in genuine no-kernel shader compilers |
| No-kernel SDK version | fail | `6.4.18:967862` is below `6.4.22` |
| No-kernel NBG consumer | fail | Acuity graph path uses the embedded `6.4.6` simulator SDK |
| Compiler I/O ABI and ownership | fail | `gen_nbg` boundary identified but complete contract not proven |
| Fixed Conv compilation | blocked | compiler execution remains prohibited |

Overall:

```text
qualification.status = blocked
```

This is a useful closed result. It rules out three unsafe shortcuts:

1. treating the standalone shader compiler as an NBG graph compiler;
2. treating the Acuity package version as the embedded SDK version;
3. passing the Wave profile into an older simulator backend and trusting the
   target name in any resulting binary.

## 12. What is still required

A candidate package may advance only if it supplies:

1. a dedicated x86-64 Vivante no-kernel SDK `6.4.22` or newer;
2. an OpenVX/ovxlib graph-to-NBG path that directly and demonstrably consumes
   `VIV_GPU_FILE`;
3. ovxlib `1.2.26` or newer;
4. VIPLite `2.0.0` or newer if the VIPLite deployment route is used;
5. the exact target licence or entitlement required by that release;
6. all headers, version records, build metadata, and dynamic libraries;
7. a complete hash-pinned transitive dependency closure;
8. a statically recovered compiler CLI and environment contract;
9. exact input graph/tensor/quantization/layout constraints;
10. output-directory and output-filename ownership;
11. exit-status, error-reporting, timeout, and partial-output behavior;
12. proof that the backend emits NBG rather than a simulator-only artifact.

The exact package request should therefore say:

```text
Vivante no-kernel x86-64 SDK >= 6.4.22
OpenVX/ovxlib NBG generation
ovxlib >= 1.2.26
VIP9000NANOS profile support
customer/product ID 0x1000000e
VIV_GPU_FILE support
complete headers, libraries, tools, licence, and build/version records
```

“Acuity wheel” or “ONNX converter” by itself is not a sufficiently precise
request.

A shareable request, provider questions, deliverable matrix, intake
procedure, and first-compile envelope are maintained in
[`no-kernel-sdk-acquisition-checklist.md`](../profile/no-kernel-sdk-acquisition-checklist.md).

## 13. General ONNX support and CPU fallback

The acquisition result does not invalidate general ONNX support. It fixes
the boundary:

```text
general ONNX loading and correctness:
  available now through ONNX Runtime CPU

Wave NPU placement:
  no approved graph signatures

current hybrid behavior:
  audit model
  classify every node/partition
  construct one CPU-only session
  make zero NPU attempts

strict NPU behavior:
  refuse before session construction
```

Acuity accepting an ONNX file proves only frontend parsing for its supported
model subset. It does not prove:

- every ONNX operator is lowerable;
- dynamic shapes are supported;
- quantization and layout are compatible;
- a partition compiles for the Wave;
- an emitted NBG targets `0x1000000e`;
- the designated noise-cancelling partition actually runs on the NPU.

The correct long-term architecture remains:

```text
ONNX Runtime
  ├─ unsupported/unqualified regions -> CPU ExecutionProvider
  └─ preflight-approved region
       -> hardened VSINPU
       -> qualified TIM-VX/ovxlib compiler
       -> statically audited Wave NBG
       -> separately reviewed device execution
```

For noise cancellation, STFT, windowing, overlap-add, recurrent state, and
unverified operators can remain on CPU. A small fixed-shape Conv mask
estimator is the first intended NPU island. Until the fixed Conv compiler
control passes, the entire model remains CPU-correct and all-CPU.

## 14. Next PoCs

Each step is an independent exit gate.

### PoC 9D-A — Acquire the missing SDK

- obtain the exact package described above;
- record provenance and HTTP/package metadata;
- hash every archive and selected component;
- do not install, import, or execute it.

Exit: trusted provenance and a complete immutable artifact set.

### PoC 9D-B — Static backend qualification

- recover executable format and architecture;
- enumerate all imports and dependencies;
- prove the graph/NBG component consumes `VIV_GPU_FILE`;
- recover CLI, environment, licence, input, output, and ownership contracts;
- verify every component version is in one compatible release family.

Exit: no unresolved critical ABI or dependency field.

### PoC 9D-C — Isolated fixed-Conv compile

Only after 9D-B passes:

- one immutable fixed Conv graph;
- one exact vendor Wave profile;
- network disabled;
- read-only dependency root;
- private empty output directory;
- no device nodes;
- no ADB;
- no container-runtime registration;
- CPU, memory, PID, file-size, file-count, descriptor, and wall limits;
- one compiler attempt;
- capture exit status, stdout/stderr, file set, and hashes.

Exit: exactly the expected output, or a closed negative result.

### PoC 9D-D — Static NBG qualification

- parse the artifact without executing it;
- require VPMN/NBG structure and valid bounds;
- require target `0x1000000e`;
- require expected input/output descriptors;
- validate every stored checksum;
- compare invariant distributions with the stock Wave corpus;
- repeat the compile only if determinism is the explicit question.

Exit: an approved compile artifact manifest. No device authorization follows
automatically.

### PoC 10A — Separate device execution

Only after an independently reviewed execution plan:

- stage by exact hash into an exclusive path;
- collect pre-state;
- execute once with fixed canary-protected tensors;
- compare all output elements with the CPU oracle;
- collect post-state and cleanup;
- stop on any health or integrity change.

Exit: one numerically correct, health-preserving Conv control.

### PoC 10B and PoC 11

- approve one ONNX graph signature and prove a mixed CPU/NPU control;
- then qualify the fixed spectral-mask model;
- retain CPU fallback and honest placement telemetry;
- add latency, audio-quality, thermal, and soak gates last.

## 15. Reproduction

All commands in this section are host-only and inert:

```bash
python3 -m unittest \
  waved/scripts/npu/tests/test_compiler_acquisition.py

python3 waved/scripts/npu/compiler_acquisition.py \
  validate-targets

python3 waved/scripts/npu/compiler_acquisition.py \
  generate-evidence

python3 waved/scripts/npu/compiler_acquisition.py \
  verify-generated
```

Expected top-level result:

```json
{
  "compiler_calls": 0,
  "status": "blocked"
}
```

`verify-generated` rehashes approximately 13.5 GB across the outer ZIP,
nested ZIP, OCI TAR, and selected artifacts; then it streams and rehashes
all OCI blobs and scans the Acuity archives. The repeated work is deliberate:
verification does not trust a prior extraction or cached digest.

The focused tests cover:

- strict JSON duplicate-key refusal;
- parser literal control-character policy;
- ambiguous and escaping archive names;
- ZIP traversal, duplicate, and symlink rejection;
- TAR traversal, special-file, and escaping-link rejection;
- cross-buffer string scanning;
- static x86-64 ELF proof;
- dynamic ELF and wrong-machine refusal;
- x86 relative-call decoding;
- the `6.4.22` version gate;
- canonical evidence serialization.

## 16. Positive, negative, inferred, and unknown findings

### Static verified

- the archive and OCI identities in the generated evidence;
- the exact Wave vendor config and five identity values;
- the `vcompiler` parser bytes, strings, field order, call sites, and lookup
  call;
- `vcompiler` and `vcCompiler` are static x86-64 shader compilers;
- the acquired standalone compiler build is `6.4.18:967862`;
- Acuity 6.30.22's wrappers use `VSIMULATOR_CONFIG`;
- the embedded simulator SDK is `6.4.6:769531`;
- no acquired compiler or library was executed.

### Closed negative

- the acquired package is not a qualifying no-kernel Wave NBG compiler;
- the fixed Conv compile is not authorized;
- the adjacent `6.4.18:886827` AArch64 CLC remains quarantined.

### Inferred

- the generated native `gen_nbg` helper, rather than the Python wrapper,
  owns final NBG generation. Static wrapper strings and file movement support
  this, but the complete native ABI has not yet been reconstructed.

### Unknown

- the exact vendor package name and distribution channel for the required
  `>=6.4.22` no-kernel SDK;
- whether entitlement is needed for target `0x1000000e`;
- the complete supported operator/shape/type/quantization set for Wave;
- the correct compiler/runtime compatibility range;
- the fixed Conv compiler result;
- any NPU latency or power benefit for a noise-cancelling model.

## 17. Non-claims

This PoC does not claim:

- that the vendor config is qualified for an NBG compiler;
- that `vcompiler` or `vcCompiler` emits NBG;
- that Acuity 6.30.22 or 6.57.1 can produce a Wave NBG;
- that changing a profile or product ID makes a foreign NBG compatible;
- that the simulator's acceptance predicts device execution;
- that any general ONNX node is currently NPU-resident;
- that a noise-cancelling model ran on the Wave NPU.

## 18. Decision

PoC 9D's public-package acquisition and static qualification phase is
complete as a closed blocker.

Allowed now:

- use the exact vendor Wave profile as corroborated static evidence;
- use the recovered five-field parser contract to qualify a future,
  version-compatible no-kernel NBG consumer;
- continue ONNX CPU correctness, model selection, partition auditing, and
  noise-cancellation quality work;
- request the precisely scoped missing SDK package.

Not allowed:

- execute either acquired standalone shader compiler;
- load or run the acquired OCI image;
- import the Acuity native extensions;
- compile the fixed Conv with the embedded simulator SDK;
- pass the config candidate to the quarantined CLC;
- send a new generated NBG to the device.

The next technical action is acquisition of a dedicated x86-64 Vivante
no-kernel SDK `6.4.22` or newer whose OpenVX/ovxlib NBG path demonstrably
consumes `VIV_GPU_FILE`.
