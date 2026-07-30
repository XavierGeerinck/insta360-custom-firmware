# PoC 9C — Vivante Configuration Parser Recovery

**Date:** 2026-07-28
**Status:** Passed the host parser-recovery gate
**Device contact:** None
**Wave API, graph, or driver calls:** None
**Compiler calls:** None
**Dynamic scope:** One old x86 simulator parser in isolated containers
**Candidate status:** Parser-shaped, deliberately not compiler-qualified

## 1. Outcome

PoC 9C recovered a complete five-field configuration-file parser from the
local x86 Vivante simulator, extracted its 358-entry built-in target catalog,
confirmed the Wave-family entry, implemented a stricter host parser and
renderer, and exercised 13 fixed vendor-parser cases in isolated x86
containers.

The recovered assignment grammar is:

```text
chipModel              = 0x00009000;
chipRevision           = 0x00009002;
productID              = 0x05090001;
ecoID                  = 0x04000000;
customerID             = 0x1000000e;
```

The exact parser function is:

```text
StateParser::chooseTargetByConfigFile(char const*)
```

The five values occupy offsets `0x50..0x60` of a 100-byte simulator target
record. The first 80 bytes are the built-in target name.

The built-in simulator catalog contains exactly one
customer-`0x1000000e` record:

```text
index:         157
name:          VIP9000NANOS_PID0X1000000E
chip model:    0x00009000
revision:      0x00009001
product ID:    0x05090001
eco ID:        0x04000000
customer ID:   0x1000000e
record SHA-256:
  4d533d3c898a3eb1e317148826bad0d16172cecf7add61f10ede6d45ef407e69
```

The isolated parser accepted both the catalog revision `0x9001` and the live
Wave revision `0x9002`. That independently exercises the revision-family
behavior recovered in PoC 9A.

This is not yet the Acuity/no-kernel compiler contract. The public TIM-VX
sample names `VIV_GPU_FILE` and requires a dedicated no-kernel Vivante SDK
6.4.22 or newer. The local simulator binary is from the older 6.4.6 SDK and
owns `VSIMULATOR_CONFIG`, not the missing `VIV_GPU_FILE` consumer.

The generated candidate therefore carries this decision:

```text
parser-shaped-candidate-not-compiler-qualified
```

It must not be passed to a compiler until the actual no-kernel consumer is
acquired, hash-pinned, reversed, and tested.

PoC 9D subsequently acquired a real `VIV_GPU_FILE` consumer and corroborated
the five-field grammar, but only in a `6.4.18:967862` shader compiler. The
Acuity graph-to-NBG route remained an older simulator backend, so this
candidate's refusal is still active. See the
[`PoC 9D notebook`](2026-07-28-poc9d-compiler-acquisition-static-qualification.md).

## 2. Question and hypothesis

Question:

> What configuration-file grammar is implemented by the available Vivante
> stack, how does its target selection work, and can a deterministic Wave
> candidate be rendered without guessing any live identity value?

Hypothesis:

> The x86 simulator contains a bounded configuration parser whose keys map to
> the same five-word identity recovered in PoC 9A. Static analysis can recover
> its grammar and built-in catalog, and a narrowly isolated host call can
> characterize its edge cases without invoking a compiler, graph, driver, or
> device.

The hypothesis would have failed if:

- the parser body was absent or truncated;
- its fields or record layout depended on guessed private C++ types;
- the catalog contained zero or multiple customer-`0x1000000e` rows;
- the isolated loader required OpenVX, VSC, CLC, GAL, or a device node;
- the live five-field identity did not pass the parser's feature query;
- any candidate field was unknown, inferred, or below high confidence;
- dynamic testing required a full `StateParser` constructor;
- the available binary actually consumed `VIV_GPU_FILE` but its behavior
  could not be separated from `VSIMULATOR_CONFIG`.

None of those failures occurred. One dependency omission was caught before a
parser method ran and corrected by adding a single hash-pinned loader
dependency.

## 3. Authorization and safety boundary

The owner approved compiler/profile recovery and requested stepwise PoCs.
PoC 9C was constrained to host-only static work plus a reviewed x86
parser-only call.

Allowed:

- read and hash local source, documentation, and ELF files;
- inspect symbols, strings, relocations, load segments, and `DT_NEEDED`;
- run focused Ghidra analysis in temporary projects;
- read the file-backed `vsimConfigs` table as inert bytes;
- compile a small x86 harness in a local, pinned container image;
- call only three reviewed x86 `StateParser` methods;
- render and validate a candidate file on the host;
- write deterministic evidence and documentation.

Prohibited:

- ADB or any other Wave transport;
- `/dev/galcore`;
- Wave filesystem or process access;
- OpenVX/TIM-VX graph construction;
- graph verification, compilation, or execution;
- `libCLC.so`, `libVSC.so`, `libOpenVX.so`, or `libGAL.so` calls;
- a full `StateParser` constructor;
- any AArch64 function call;
- any claim that simulator grammar automatically equals no-kernel compiler
  grammar.

The quarantined adjacent 6.4.18 `libCLC.so` remained static-only and was
never called.

## 4. Inputs

The strict corpus contains 17 artifacts:

| Artifact ID | Bytes | SHA-256 | Policy |
|---|---:|---|---|
| `adjacent-clc-6.4.18` | 2,682,016 | `a1e5d11cafb6d23c7c787d24a71a766782ea022346f1580797d892238bd99f14` | `quarantined-never-call` |
| `adjacent-gal-6.4.18` | 2,109,480 | `fd544a476e0105b51d710aeb0fc8e173c85aabd77766d3469348a6b7c99512fb` | `static-only` |
| `adjacent-openvx-6.4.18` | 6,808,008 | `59ccad8a74c0e8e29487b0427c9c8898340315cc598f9dade646db70b9bcc75e` | `static-only` |
| `adjacent-vsc-6.4.18` | 17,994,360 | `7d20e84d6314d9c75a5288de418aa660fd365f1b187dca1812faa03c0ed3c079` | `static-only` |
| `timvx-multi-device-readme` | 1,074 | `53f7b6cfe4bbce04d43b3889b22806dba366457a26525e9c1ce57b6979ca7481` | `not-executable` |
| `timvx-platform-readme` | 915 | `9bf64795fd7e77def7877cfd4dd856557d16f4ab506ca2f3f9a20ed77bbf40e1` | `not-executable` |
| `wave-gal-exact` | 1,923,672 | `13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6` | `static-only` |
| `wave-openvx-exact` | 6,422,744 | `f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb` | `static-only` |
| `wave-profile-poc9a` | 26,734 | `0e465f66f80c19747d4f0a73ab7db0fc9ba4a3b873fb9253a544b6dc4c3235e8` | `not-executable` |
| `wave-vsc-exact` | 17,181,872 | `e9a39fe3b522b3926ff34f3fb380c73983ad8d48c5f40ffd6e6d8024adc2ed18` | `static-only` |
| `wave-vsc-lite-exact` | 150,384 | `e7c2c38fa813c669b7370795fad5d7b1207881103c481a6ac90e4785d058591f` | `static-only` |
| `x86-clc-6.4.6` | 2,930,624 | `102d040c1a4bd83595bf99705494603a35d447f5efdaa0a4672ef0793406ecba` | `static-only` |
| `x86-emulator-6.4.6` | 3,158,688 | `56e34c51ca5476e0ca54155d4e78679d96281552ddad7674f5ce4fe2fe5f7de6` | `isolated-parser-only` |
| `x86-vdtproxy-6.4.6` | 13,424 | `64934faba48ffb143bb7b52519f99ced40d69a2ecc6f56d0fe5bb23a866edf9f` | `isolated-parser-only` |
| `x86-gal-6.4.6` | 2,623,584 | `013718031b134ad0f8af859179e32615b9de939a7873ccf539d30ff172a61e9c` | `static-only` |
| `x86-openvx-6.4.6` | 11,091,336 | `423514a8f50c8d3c15d7b257b2027a1cd9434051a74ed0b2936b2df38badeb38` | `static-only` |
| `x86-vsc-6.4.6` | 18,090,152 | `2081c87900446cb3561046d5652b695280eff7760c5de204cf45d658634aed33` | `static-only` |

The manifest itself is:

```text
waved/scripts/npu/compiler_profile_corpus.json
SHA-256:
  75f6619a74985c06a309d3a9ca1ee603da8d46b95558919b1999e5d24144d9c9
```

The current TIM-VX checkout is:

```text
commit: 702715d714277376d7ff466b7b72833ff57564c1
commit date: 2026-03-30T17:35:49+08:00
```

That recent source checkout is independent of the old prebuilt simulator
bundle. The simulator embeds:

```text
$VERSION$6.4.6:690884$
```

## 5. `VIV_GPU_FILE` versus `VSIMULATOR_CONFIG`

This distinction is the most important negative result.

### 5.1 Source-documented no-kernel route

The local TIM-VX platform sample documents:

```text
VIV_GPU_FILE=/path/to/VIP9000NANOQ_PLUS_PID0X100000XX.config
```

It also states:

```text
Vivante SDK version >= 6.4.22
dedicated no-kernel SDK
```

`0X100000XX` is a placeholder. It is not a Wave customer ID, and
`NANOQ_PLUS` is not proof of the Wave target name.

The required no-kernel SDK is absent from this repository. Searches across
the exact Wave, adjacent 6.4.18, and x86 prebuilt binaries found no
`VIV_GPU_FILE` consumer.

### 5.2 Proven simulator route

The x86 `libEmulator.so` owns `VSIMULATOR_CONFIG`.

The recovered selector behavior is:

```text
if value begins with decimal digit:
    parse decimal built-in target ID
else if value begins with VIP, GC, or CC:
    select exact built-in target name
else:
    treat value as a configuration-file path
```

An absolute path begins with `/`, so it takes the file branch.

These two variables must not be conflated:

| Property | `VIV_GPU_FILE` | `VSIMULATOR_CONFIG` |
|---|---|---|
| Evidence | TIM-VX sample README | x86 simulator code |
| Consumer present locally | No | Yes |
| SDK named | dedicated no-kernel >=6.4.22 | simulator 6.4.6 |
| File grammar dynamically tested | No | Yes |
| Compiler qualification | No | Not applicable |

## 6. Focused Ghidra recovery

Target:

```text
waved/third_party/TIM-VX/prebuilt-sdk/x86_64_linux/lib/libEmulator.so
SHA-256:
  56e34c51ca5476e0ca54155d4e78679d96281552ddad7674f5ce4fe2fe5f7de6
```

Focused symbols:

```text
StateParser::QueryFeatureDatabase
StateParser::StateParser
StateParser::chooseTargetByConfigFile
StateParser::chooseTargetByFeature
StateParser::chooseTargetByID
StateParser::chooseTargetByName
StateParser::getConfig
StateParser::~StateParser
gcQueryFeatureDB
```

All nine were found. Every claim-bearing primary body is successfully
decompiled and untruncated. The evidence has zero Ghidra warnings. One broad
`_vmiTargetSelect` caller is truncated, but it is not used for a grammar or
ABI claim; the selector, constructor, `DebugServer` caller, query, and copy
bodies are complete.

Key entries in the ELF's native virtual-address space:

| Function | Entry |
|---|---:|
| `StateParser::getConfig()` | `0x000b1018` |
| `StateParser::getConfig(out)` | `0x000b181c` |
| `StateParser::QueryFeatureDatabase()` | `0x000b1b06` |
| `StateParser::chooseTargetByID(...)` | `0x000b1efa` |
| `StateParser::StateParser(...)` | `0x000b2128` |
| `StateParser::chooseTargetByName(...)` | `0x000b2662` |
| `StateParser::chooseTargetByConfigFile(...)` | `0x000b2aa8` |

The generated evidence is:

```text
docs/npu/profile/compiler-profile-ghidra-evidence.json
bytes: 422,285
SHA-256:
  c38cd4a2965a3c84ce479ede89017b0e7329736f2f2327bbad970669cf830772
```

## 7. Exact vendor parser

The selector reduces to:

```c
file = fopen(path, "rb");
if (file == NULL) {
    warn_and_keep_current_config();
    return;
}

copy_current_config_to_locals();

if (fscanf(file, "chipModel              = %x;\n", &model) != 0)
if (fscanf(file, "chipRevision           = %x;\n", &revision) != 0)
if (fscanf(file, "productID              = %x;\n", &product) != 0)
if (fscanf(file, "ecoID                  = %x;\n", &eco) != 0)
if (fscanf(file, "customerID             = %x;\n", &customer) != 0) {
    fclose(file);
    copy_locals_to_global_config();
    QueryFeatureDatabase();
    copy_config_and_features_to_user_snapshots();
    return;
}

fclose(file);
```

### 7.1 Layout

| Field | Offset | Type | Query order |
|---|---:|---|---:|
| target name | `0x00` | `char[80]` | — |
| `chipModel` | `0x50` | `uint32` | 0 |
| `chipRevision` | `0x54` | `uint32` | 1 |
| `productID` | `0x58` | `uint32` | 2 |
| `ecoID` | `0x5c` | `uint32` | 3 |
| `customerID` | `0x60` | `uint32` | 4 |

`StateParser::getConfig(out)` copies through offset `0x60`, establishing the
100-byte record size without relying on a reconstructed C++ header.

`QueryFeatureDatabase` calls:

```text
gcQueryFeatureDB(model, revision, product, eco, customer)
```

It logs a five-value diagnostic if the lookup returns null. It does not
return a status to the file selector.

### 7.2 Vendor weaknesses

The parser tests `fscanf(...) != 0`, not `== 1`.

`fscanf` returns:

- `1` for one successful conversion;
- `0` for a matching failure;
- `EOF`, commonly `-1`, when input ends before a conversion.

Because `-1 != 0`, empty or truncated files can pass subsequent nested
conditions and retain prior/local values. This is a vendor parser bug, not a
feature to reproduce.

The parser also:

- never checks for trailing content after `customerID`;
- has no explicit line or file bound;
- lets `%x` define numeric behavior without an eight-digit width;
- returns `void`;
- logs an unknown identity but can retain it in the selected config;
- leaves the 80-byte name unchanged for file-based selection.

## 8. Built-in target catalog

`vsimConfigs` is a data symbol:

```text
virtual address: 0x0047c760
ELF file offset: 0x0027c760
symbol bytes:    35,800
record bytes:    100
record count:    358
table SHA-256:
  5f6bf1cf829c1b13878a8162950a0edc2e3b750aafb4dba0beeace5a6992b1ec
```

The extractor maps the virtual range through ELF64 little-endian `PT_LOAD`
segments and reads only file-backed bytes. It does not load the library.

The constructor searches for this default:

```text
index 187
VIP9000NANODI_PID0X10000020
```

That default is not the Wave.

The unique customer-`0x1000000e` row is the `NANOS` name at index 157. This
exact name proves a simulator catalog entry. It does not prove a no-kernel
config filename.

## 9. Isolated parser probe

### 9.1 Static ABI basis

The complete bodies prove:

- `chooseTargetByConfigFile` only reads `this + 0xf0` in its default branch;
- the full constructor allocates `0x118` bytes for `StateParser`;
- the selector's output is held in static/global state;
- `getConfig(out)` writes exactly 100 bytes;
- selector and copy methods have exported x86_64 symbols.

The harness therefore uses:

```text
fake receiver bytes:        0x200
initialized receiver field: offset 0x00f0 only
initialized value:          built-in target ID 187
output buffer bytes:        100
```

It does not call the constructor. That avoids simulator, mutex, VX-state, and
other setup paths unrelated to parsing.

Called symbols:

```text
_ZN11StateParser18chooseTargetByNameEPKc
_ZN11StateParser24chooseTargetByConfigFileEPKc
_ZN11StateParser9getConfigEP17_VSimulatorConfig
```

### 9.2 Dependency stop and correction

The first isolated attempt completed no cases because `dlopen` reported:

```text
libvdtproxy.so: cannot open shared object file
```

No parser method ran in that attempt.

Static `DT_NEEDED` inspection then established:

```text
libEmulator.so:
  libvdtproxy.so
  libm.so.6
  librt.so.1
  libstdc++.so.6
  libgcc_s.so.1
  libpthread.so.0
  libc.so.6

libvdtproxy.so:
  libc.so.6
```

`libvdtproxy.so` was added to the corpus with SHA-256:

```text
64934faba48ffb143bb7b52519f99ced40d69a2ecc6f56d0fe5bb23a866edf9f
```

For the successful run it was copied into the temporary output directory,
rehash-checked, and exposed as `LD_LIBRARY_PATH=/out`. No general SDK
library directory was on the loader path.

### 9.3 Isolation envelope

Container image:

```text
wave-npu-compiler-toolchain:ubuntu22.04-amd64
image ID:
  sha256:083ca628e0feb7ca682aec5180e5331d0c758bb4d73fcd878b9900cdd3d777ce
platform: linux/amd64
```

Every case received a fresh container:

```text
network:               none
root filesystem:       read-only
repository mount:      read-only
capabilities:          all dropped
no-new-privileges:     enabled
user:                  65534:65534
CPU limit:             1
memory limit:          256 MiB
PID limit:             64
file-descriptor limit: 64
core dumps:            disabled
wall limit:            15 seconds
environment:           fixed PATH and LD_LIBRARY_PATH=/out only
```

The harness source is:

```text
waved/tools/npu-profile/vsim_config_parser_probe.cc
bytes: 9,474
SHA-256:
  6b2187cde33a94fd620335592c91409655c24a5845116af385b32dbbef5562af
```

### 9.4 Observed matrix

All 13 cases completed. None crashed or timed out.

| Case | Result classification | Returned identity/name |
|---|---|---|
| built-in Wave name | selected Wave database record | named row, revision `0x9001` |
| valid database file | accepted database identity file | unnamed, revision `0x9001` |
| valid live file | accepted live identity file | unnamed, revision `0x9002` |
| missing file | retained zero initial state | warning |
| empty file | retained zero initial state | zero-tuple query error |
| truncated after model | retained partial identity | model `0x9000`, remaining words zero |
| wrong order | retained zero initial state | no explicit diagnostic |
| duplicate key | retained zero initial state | no explicit diagnostic |
| unknown key | retained zero initial state | no explicit diagnostic |
| malformed number | retained zero initial state | no explicit diagnostic |
| overlong first line | accepted database identity | no bound enforced |
| wrong customer | retained unknown identity | five-word query error |
| trailing content | accepted database identity | trailing field ignored |

The most useful positive observation is:

```text
input revision:  0x00009002
output revision: 0x00009002
query error:     none
```

The old simulator feature database accepts the live Wave revision-family
tuple.

The most useful negative observation is that lookup failure does not make the
selector fail. A wrong customer is logged but retained. A wrapper must
validate the identity independently before and after any vendor parse.

Evidence:

```text
docs/npu/profile/vsim-parser-probe-results.json
bytes: 9,258
SHA-256:
  f620d368ebddc0a09850e222032795ba8c63bac32b65349858003703a62c5a75
```

## 10. Hardened host grammar

`compiler_profile.py` deliberately implements a safe subset:

```text
encoding:                 ASCII
line ending:              LF only
final LF:                 required
file limit:               512 bytes
line limit:               128 bytes
field count:              exactly 5
field order:              exact
integer syntax:           (?:0[xX])?[0-9a-fA-F]{1,8}
integer semantics:        unsigned hexadecimal uint32
duplicate field:          reject
unknown field:            reject
comment:                  reject
blank line:               reject
trailing content:         reject
sign:                     reject
overflow:                 reject
```

It accepts the harmless `%x` lexical variants needed for differential tests,
then renders one canonical form:

```text
<key padded to 23 columns>= 0x<eight lowercase hex digits>;
```

It does not emulate:

- EOF-as-success;
- unbounded whitespace;
- unbounded `%x`;
- silent retention of a previous identity;
- missing-file fallback;
- ignored trailing content.

## 11. Candidate generation

Each emitted field must be:

- present in the recovered parser;
- typed as `uint32`;
- sourced from a PoC 9A identity fact;
- `observed-live`;
- high confidence;
- `single-source` or `corroborated`;
- backed by at least one source reference.

Mapping:

| Config key | PoC 9A fact | Value |
|---|---|---:|
| `chipModel` | `identity.chip_model` | `0x00009000` |
| `chipRevision` | `identity.chip_revision_observed` | `0x00009002` |
| `productID` | `identity.product_id` | `0x05090001` |
| `ecoID` | `identity.eco_id` | `0x04000000` |
| `customerID` | `identity.customer_id` | `0x1000000e` |

Candidate:

```text
docs/npu/profile/wave-viv-gpu-file.candidate.config
bytes: 185
SHA-256:
  8d1d184a4546731a7b014fab1a2f01ff997d50f9a25212b194a1605261558d89
```

Grammar evidence:

```text
docs/npu/profile/viv-gpu-file-grammar.json
bytes: 10,553
SHA-256:
  141258477d751e4b30b4c5408817c5c16852ff34b43c454624a67ef2fa665a4e
```

Manifest:

```text
docs/npu/profile/wave-viv-gpu-file.candidate.manifest.json
bytes: 6,396
SHA-256:
  45b3c44488e4b8ceb8840111ffed67c18762bc8214a7e1a629dc6fc0f8721ddc
```

The filename contains `.candidate`. The manifest explicitly sets:

```json
{
  "compiler_execution": false,
  "device_execution": false,
  "graph_construction": false
}
```

## 12. Tests and reproduction

Host-only unit and integration tests:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_compiler_profile
```

Static input validation:

```bash
python3 waved/scripts/npu/compiler_profile.py validate-targets
```

Focused Ghidra regeneration:

```bash
python3 waved/scripts/npu/compiler_profile.py generate-ghidra
```

Isolated parser matrix:

```bash
python3 waved/scripts/npu/compiler_profile.py probe-vendor-parser
```

Grammar and candidate:

```bash
python3 waved/scripts/npu/compiler_profile.py recover-grammar
python3 waved/scripts/npu/compiler_profile.py generate-candidate
python3 waved/scripts/npu/compiler_profile.py verify-generated
```

Full NPU regression:

```bash
python3 -m unittest discover \
  -s waved/scripts/npu/tests \
  -p "test_*.py"
```

The corpus loader rejects:

- duplicate or unknown JSON keys;
- path and symlink escape;
- malformed or uppercase hashes;
- hash drift;
- non-ELF ELF artifacts;
- invalid UTF-8 text artifacts;
- duplicate roles;
- unknown execution policies;
- a quarantined artifact without `quarantined-never-call`;
- isolated execution policy on anything except the reviewed parser or its
  dependency.

The config parser tests cover:

- canonical round trip;
- optional `0x` and hex case;
- wrong order;
- missing, duplicate, and unknown fields;
- comments, blank lines, and trailing content;
- sign and overflow;
- missing terminator and final LF;
- non-ASCII, NUL, file limit, and line limit.

## 13. What is now proven

High-confidence:

- the simulator file parser has exactly five ordered `%x` assignments;
- the record layout is name[80] plus five uint32 words;
- the feature query order is model, revision, product, eco, customer;
- the catalog has 358 records of 100 bytes;
- the unique Wave-family built-in entry is
  `VIP9000NANOS_PID0X1000000E`;
- the x86 parser accepts both revision `0x9001` and live revision `0x9002`;
- overlong lines and trailing content are accepted by the vendor parser;
- EOF is treated as success in nested scans;
- an invalid identity can be logged yet retained;
- the strict host parser removes those ambiguities;
- all candidate values are sourced from high-confidence PoC 9A live facts.

Still unknown:

- the actual no-kernel `VIV_GPU_FILE` discovery and file grammar;
- whether the compiler uses this five-field file directly or expands it into
  a larger Acuity architecture profile;
- exact compiler-required feature, SRAM, alignment, and errata inputs;
- whether the compiler accepts live revision `0x9002` or expects a catalog
  family row;
- which compiler build is ABI-compatible with Wave runtime 6.4.19.5;
- general Conv/MatrixMul lowering and Wave-targeted code generation;
- legal ONNX partition coverage.

## 14. Implications for ONNX and audio denoising

General ONNX support should not wait for full NPU coverage.

The recommended architecture remains:

```text
ONNX model
    |
    v
strict shape/type inference
    |
    v
per-node capability check
    |
    +---- unsupported or uncertain ----> ONNX Runtime CPU
    |
    +---- proven NPU island -----------> Wave compiler adapter
                                             |
                                             v
                                      NBG structural auditor
                                             |
                    audit failure -----------+----> CPU fallback
                                             |
                                             v
                                      reviewed NPU execution
```

For noise cancellation, begin with models whose CPU path is already useful:

1. STFT and iSTFT on CPU;
2. fixed tensor shapes;
3. a small Conv or dense spectral-mask network;
4. CPU inference as the correctness oracle;
5. NPU partitioning only for independently proven operators;
6. per-frame timeout and immediate CPU fallback;
7. numerical comparison against CPU before real-time enablement.

PoC 9C supplies target identity and parser provenance. It does not yet supply
Conv lowering or compiler trust.

## 15. Exit gate and next work

PoC 9C passes because:

- discovery ownership is separated correctly;
- the simulator grammar and edge cases are statically and dynamically
  recovered;
- the built-in Wave-family target is unique;
- the strict parser and renderer are tested;
- a deterministic candidate uses only high-confidence live facts;
- the candidate is explicitly refused for compiler use;
- no compiler or device call occurred.

Before PoC 9D can call any compiler:

1. acquire the dedicated no-kernel Vivante SDK 6.4.22 or newer, or recover
   the same consumer from a trusted vendor distribution;
2. hash-pin the full transitive dependency set;
3. locate and reverse the actual `VIV_GPU_FILE` consumer;
4. prove compiler input/output ABI and ownership;
5. choose one fixed Conv graph and one fixed output directory;
6. run in a no-network, read-only, resource-limited container;
7. structurally audit any output before considering device work;
8. keep the 6.4.18 CLC quarantine intact.

If the no-kernel SDK cannot be obtained, the next safe path is a compiler
adapter that refuses all NPU partitions and runs the full ONNX model on CPU.
That is still useful: it establishes the API, model validation, telemetry,
and fallback contract without making an unsupported accelerator claim.
