# PoC 12B — Controlled Erf Constant Differential

- **Date:** 2026-07-29
- **Run ID:** `poc12b-erf-constant-diff-20260729`
- **Safety class:** one staged, compile-only device experiment
- **Live result:** PASS
- **Locked PoC 12B interpretation:** `multi-region-unresolved`
- **Device cleanup:** PASS; `/tmp/wave-npu-poc12b` absent

## Outcome

The exact Wave userspace stack emitted three valid target-native NBG v2
files from one fixed TIM-VX graph:

```text
A1: CONSTANT U8[4] [0,1,2,3] -> Erf -> OUTPUT U8[4]
A2: CONSTANT U8[4] [0,1,2,3] -> Erf -> OUTPUT U8[4]
B:  CONSTANT U8[4] [0,1,2,4] -> Erf -> OUTPUT U8[4]
```

A1 and A2 were byte-for-byte identical. B had the same file size, section
directory, tensor metadata, network metadata, and memory metadata. Two file
ranges changed:

1. the four-byte header `crc_sum` at file offsets `[12,16)`;
2. one byte at file offset `1511`, inside
   `NBG_SECTION_DYNAIO` at section-relative offset `3`.

PoC 12B deliberately used the closed constant-search registry inherited from
PoC 12A. That registry did not include section type 20, so this run was
recorded honestly as `multi-region-unresolved`; it did not retroactively
expand the search space after seeing the result. PoC 12C subsequently
qualified the DYNAIO table and file checksum in a separate host-only evidence
layer.

No generated NBG was loaded or executed in this PoC.

## Hypothesis and falsification gates

The primary hypothesis was:

> If the exact Wave producer is deterministic and preserves a controlled
> one-byte constant change, A1/A2 will be identical and the A/B difference
> will be confined to one source-owned payload region plus integrity
> collateral.

The experiment would stop before B if:

- A1 failed;
- A2 failed;
- A1 and A2 differed by even one byte;
- strict VPMN parsing failed;
- target or version differed;
- a stored section checksum failed;
- device health changed;
- a fourth native invocation would be required.

## Fixed policy

Allowed device behavior was limited to:

- stage hash-pinned files under `/tmp/wave-npu-poc12b`;
- call the fixed binary once for each of A1, A2, and B;
- compile the fixed Erf graph to NBG;
- hash, pull, and strictly parse each output;
- read bounded health state;
- remove the exact stage root.

Explicitly prohibited counters all remained zero:

```text
graph execution                 0
NBG loading                     0
tensor upload/readback          0 / 0
direct galcore opens            0
raw ioctls                      0
driver resets                   0
device reboots                  0
service stops                   0
simulator calls                 0
quarantined CLC calls           0
compiler retries                0
alternate operation calls       0
device container runs           0
unapproved container runs       0
persistent mutated NBGs         0
```

The experiment definition is
[`erf_constant_diff_manifest.json`](../../../waved/scripts/npu/erf_constant_diff_manifest.json),
SHA-256
`03ceeae068b1f2f91dd6d6e0b8085bad079201736c5bef263ced5eccfa4da738`.

## Build provenance

The fixed source was
[`tim_vx_compile_erf_constant_diff.cc`](../../../waved/tools/npu-compiler/tim_vx_compile_erf_constant_diff.cc),
SHA-256
`e7c4bb1a4fbb2efa437ae3afad4e1e417dd51a0f2fc38e36e7596aecf0a72f50`.

Exactly one network-disabled cross-build container was used:

```text
image:
  ghcr.io/cross-rs/aarch64-unknown-linux-gnu
  @sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c
network: none
container runs: 1
```

The resulting 37,104-byte AArch64 executable had SHA-256:

```text
29a19a7a9b818335a6f31f3b84ca2469073302a2f3b051e0ae6c9c4a56ff6721
```

The first post-build policy check stopped because the ELF interpreter
`ld-linux-aarch64.so.1` was not yet explicitly named in the verifier's
allowlist. This was a verifier-policy omission, not a binary or compile
failure. The verifier was corrected on the host and rerun against the same
ELF. No second build container was started and the binary hash did not
change.

Other staged build identities:

| Artifact | SHA-256 |
|---|---|
| exact `libtim-vx.so` | `d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b` |
| `cl_viv_vx_ext.h` | `908d36ab616e27cabf5022bf16185825ce1c49f91fbaa37b98e67b4b6bb9af57` |
| exact Wave `libOpenVX.so` | `f9cd917d4232316ab53a443ec93cd1217893f4de96fc113811b800b9ae8ca0bb` |

## Preflight

Read-only preflight passed before the first native call:

| Gate | Observation |
|---|---|
| ADB devices | exactly one authorized Wave in `device` state |
| kernel | `Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux` |
| `/dev/galcore` | major/minor `199:0` |
| galcore holders | none |
| `enableNN` | `255` |
| `nanoqFreq` | `838860800` |
| `recovery` | `1` |
| stage root | exclusive staged directory |
| output files | all absent |
| free `/tmp` | `1,960,767,488` bytes |
| required artifacts | all hashes matched |

Symlinked system libraries were admitted only after resolving and hashing
their exact targets. No unexpected process held `/dev/galcore`.

## Live sequence

The harness enforced this order:

```text
preflight
  -> A1 compile-only
  -> pull, hash, parse, postflight
  -> A2 compile-only
  -> pull, hash, parse, postflight
  -> exact A1/A2 equality gate
  -> B compile-only
  -> pull, hash, parse, postflight
  -> final postflight
  -> exact stage cleanup
```

Observed native calls:

| Candidate | Constant | Duration | Bytes | SHA-256 |
|---|---|---:|---:|---|
| A1 | `[0,1,2,3]` | 135 ms | 5,904 | `4a84faf4da4604429522dfa6f0a8eb9887287eecb0f97d697843e0beb78c8a8b` |
| A2 | `[0,1,2,3]` | 138 ms | 5,904 | `4a84faf4da4604429522dfa6f0a8eb9887287eecb0f97d697843e0beb78c8a8b` |
| B | `[0,1,2,4]` | 141 ms | 5,904 | `c9e3e18414f5bc674d4204921966ba53b08918ddcef73112003b065b9599f227` |

Each call reported:

```text
remote exit       0
selector backend  OPENVX
selector kernel   erf
chip model        0x00009000
chip revision     0x00009002
chip ECO          0x04000000
customer target   0x1000000e
```

The three stderr captures were empty. The run envelope records exactly three
native invocations and status `PASS`.

The durations above are descriptive orchestration timings, not NPU inference
benchmarks. This PoC did not execute a graph.

## Determinism gate

A1 and A2 matched on:

- byte count;
- full SHA-256;
- every serialized byte;
- structural signature;
- section descriptors;
- section fingerprints;
- tensor, network, and memory metadata.

Result:

```text
A1/A2 exact match  true
first difference  none
B admitted         yes
```

This proves determinism only for the exact producer, exact runtime, and exact
fixed graph used here.

## A/B structure

Both A and B are:

```text
magic       VPMN
version     0x00020000
target      0x1000000e
bytes       5904
inputs      0 external inputs
outputs     1
sections    14
```

All section descriptors were identical. The relevant sections were:

| Index | Type | Offset | Size | Role at PoC 12B time |
|---:|---:|---:|---:|---|
| 2 | 40 | 900 | 24 | decoded section-checksum records |
| 8 | 20 | 1508 | 64 | named DYNAIO, still opaque |
| 11 | 26 | 1596 | 8 | named DYNAIO table, still opaque |
| 12 | 24 | 1664 | 4160 | coefficient data |
| 13 | 21 | 5824 | 80 | load state |

The A/B tensor, network, memory, and directory documents were equal.

## Locked PoC 12B byte-run ledger

| File range | A | B | Owner | PoC 12B classification |
|---|---|---|---|---|
| `[12,16)` | `7e2046d3` | `6f8f7667` | header `crc_sum` | producer integrity collateral; algorithm unresolved at capture time |
| `[1511,1512)` | `03` | `04` | section 8/type 20, relative `3` | unresolved |

The type-40 records did not change because they cover TP, coefficient, and
load-state sections, none of which changed.

The finite PoC 12A search registry was:

```text
{4,5,6,7,8,9,10,21,23,24,27}
```

Section type 20 was outside that predeclared set. The analyzer therefore
reported:

```text
classification  multi-region-unresolved
PoC 12C gate     not-ready
reconstruction  not attempted
```

This locked result is preserved in
[`erf-constant-differential.json`](../format/erf-constant-differential.json).
Its SHA-256 at capture-analysis completion was
`559c1621fe91a787ae4750c5b9421340a61afef41e05bcc46fc9b61e9c90fa7c`.

## Postflight and cleanup

After every candidate and after the complete sequence:

- boot ID was unchanged;
- kernel was unchanged;
- galcore major/minor was unchanged;
- `enableNN`, `nanoqFreq`, and `recovery` were unchanged;
- required runtime hashes were unchanged;
- no probe process remained;
- no new kernel alert was found;
- every partial output remained absent.

Final cleanup removed only:

```text
/tmp/wave-npu-poc12b
```

The cleanup snapshot proved that the stage executable, staged TIM-VX
library, staged header, three outputs, and three partial-output paths were
absent. System runtime libraries remained present with their pinned hashes.

Raw envelope hashes:

| File | SHA-256 |
|---|---|
| `definition.json` | `8fc97f217e33daf0ee8e96a87bd8ba3b9d1b96b98055db0f0207849f3c3176b5` |
| `run-envelope.json` | `4bcc5651dd173409ee1e5ad30fda954724c851474aa5776f18a929f35ca1094d` |
| `preflight.json` | `f0c7f498eba1caddb40818710f109be5e92b010896cbcd1392d3deaadab31751` |
| `postflight.json` | `c7160bdae518e583c172810f84700d92f46c88a9b3ac3de988fa1c2af3361350` |
| `cleanup.json` | `dbbb71aee14cfc93d63d9f55c5ffba2c625ab0a77e0d793efa4887ce1aeaf13d` |

Raw evidence remains under:

```text
artifacts/npu/runs/poc12b-erf-constant-diff-20260729/
```

## Findings

### Positive

- The exact Wave producer is byte-deterministic for this fixed graph.
- A one-byte constant change survives into a target-native NBG.
- Zero-external-input NBG v2 graphs are legitimate for this constant-input
  graph.
- A/B structure remains stable.
- All device safety gates and cleanup passed.

### Negative

- The locked PoC 12B registry could not name the section-20 ownership.
- No type-40 checksum record covers section 20.
- PoC 12B alone was insufficient to authorize a mutable serializer.

### Inferred

- The source byte likely occupied a dedicated serialized data region because
  it changed at the same byte lane as the controlled source. This inference
  was not promoted until PoC 12C decoded the adjacent table and reconstructed
  B exactly.

### Unknown after PoC 12B

- the meaning and geometry of sections 20 and 26;
- the exact header `crc_sum` algorithm and serialization order;
- whether arbitrary dynamic entries are constants, runtime scratch, or
  external-input staging;
- whether mutating such an entry would remain safe at load or execution.

## Reproduction

Host contracts:

```bash
python3 -m unittest \
  waved.scripts.npu.tests.test_npu_erf_constant_diff_lab \
  waved.scripts.npu.tests.test_nbg_constant_diff
```

Offline differential verification:

```bash
python3 waved/scripts/npu/nbg_constant_diff.py verify-generated
```

The live `run` command must only be used with the exact connected device,
manifest, staged hashes, and clean preflight described above. It is not
needed to reproduce the host-only PoC 12C interpretation.

## Non-claims

This PoC does not establish:

- general ONNX compilation;
- arbitrary constant serialization;
- command-stream encoding;
- coefficient packing;
- graph execution;
- noise-cancellation model support;
- runtime safety of a mutated NBG;
- production performance or reliability.
