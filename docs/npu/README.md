# Insta360 Wave NPU Research

This directory is the evidence hub for learning how to run custom neural
networks on the Insta360 Wave's VeriSilicon-backed NPU safely and
reproducibly.

The immediate result is concrete:

- code we control can open `/dev/galcore`;
- code we control can load the Wave's `libnnsdk.so`;
- an unmodified, firmware-targeted NBG can be loaded, executed, read back,
  and destroyed;
- two independently captured PoC 0 controls produced the same 332,800-byte
  output digest;
- the device remained responsive and all captured NPU parameters and
  required artifact hashes were unchanged;
- PoC 1 recovered and test-locked the private runtime ABI, backend dispatch,
  NBG loader lifecycle, and cleanup/error boundaries;
- PoC 2 classified all 139 configured graphs, resolved the public v2 section
  schema, recovered tensor/network/memory metadata, and verified all 690
  stored section CRC-32 records;
- the available host simulator still emits the wrong NBG product target, but
  PoC 1 also found a separate private export-data graph-construction route;
- arbitrary custom models are **not yet unlocked**: neither route has yet
  produced a verified custom graph on the Wave.

The detailed program design is in
[`2026-07-28-wave-npu-reverse-engineering-program-design.md`](../superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md).
PoC 0's implementation plan is in
[`2026-07-28-wave-npu-poc0-reproducible-lab.md`](../superpowers/plans/2026-07-28-wave-npu-poc0-reproducible-lab.md).
PoC 1's ABI evidence starts at [`abi/README.md`](abi/README.md), and its
implementation plan is
[`2026-07-28-wave-npu-poc1-runtime-abi.md`](../superpowers/plans/2026-07-28-wave-npu-poc1-runtime-abi.md).
PoC 2's format evidence starts at
[`format/README.md`](format/README.md), and its implementation plan is
[`2026-07-28-wave-npu-poc2-nbg-corpus-format.md`](../superpowers/plans/2026-07-28-wave-npu-poc2-nbg-corpus-format.md).

## Why this is a staged program

The Wave does not expose a documented custom-model SDK. Its stock software
contains several useful layers, but they solve different parts of the
problem:

```text
audio model or ONNX
        │
        ├── compile to Wave-targeted NBG — compiler/target not solved
        │
        └── reconstruct private export-data graph — format not yet complete
        ▼
compiled NBG or verified export-data graph
        │
        │ private libnnsdk ABI — recovered and layout-tested
        ▼
VeriSilicon userspace runtime
        │
        │ /dev/galcore ioctls
        ▼
galcore kernel driver
        │
        ▼
Wave NPU
```

The compiled-NBG blocker is above the runtime: the host SDK available in this
repository produces target `0x10000020`, while the tested Wave graph targets
`0x1000000e`. The runtime rejects the former before execution. We do not
patch that product field and execute the result because the field selects
compiled hardware behavior; changing an identifier cannot make kernels
correct for different silicon.

PoC 1 found a second branch in the exact stock runtime. `nbgType` 2/3 parses
a private export-data representation, creates individual OpenVX tensors and
nodes, then verifies the graph. That branch may avoid a precompiled NBG, but
its format, operation parameters, ownership rules, and target behavior are
not yet sufficiently recovered to execute a handcrafted graph. It is a
research route, not an unlock claim.

This research follows the same evidence-first spirit as Maderix's
[Apple Neural Engine investigation](https://maderix.substack.com/p/inside-the-m4-apple-neural-engine)
and [ANE tools](https://github.com/maderix/ANE): recover boundaries, preserve
raw observations, build parsers rather than relying on offsets, and graduate
from tiny controlled graphs to useful models. The Wave stack is not Apple's
ANE, so none of the private ABI or binary-format details are assumed to
transfer.

## Program status

| PoC | Question | Status | Exit gate |
|---|---|---|---|
| 0 — Reproducible lab | Can every later experiment be hash-pinned, bounded, and compared? | **Passed 2026-07-28** | Two stock CTC controls are semantically equivalent and leave captured state unchanged. |
| 1 — Runtime ABI and call graph | What are the exact loader, tensor, execution, and cleanup contracts? | **Passed 2026-07-28** | Recovered ABI map is backed by exact function bytes, callers, focused decompilation, public-header deltas, and layout tests. |
| 2 — NBG corpus and format | Which sections encode target, tensors, operations, constants, and relocation data? | **Passed 2026-07-28** | The strict host parser classifies all 139 configured graphs, agrees with live CTC metadata, and validates 690/690 section checksums. |
| 3 — Runtime and driver trace | What runtime calls and driver interactions occur for a known graph? | Pending | Bounded traces correlate lifecycle phases without raw mutation fuzzing. |
| 4 — Firmware v3 audio loader | Does `libmodel_infer.so` provide a useful CPU, OpenCL, or NPU path for audio models? | **Static backend gate passed in PoC 1** | It is classified as a CPU/MNN-style reference; dynamic format/quality characterization remains optional. |
| 5 — Wave custom-graph gate | Can a minimal add/identity graph be represented for this runtime and target? | Two research routes | A genuinely Wave-targeted compiled NBG or fully reconstructed export-data graph passes create, execute, verify, and destroy. |
| 6 — Operation characterization | Which operations, shapes, dtypes, and state patterns work? | Pending | An allowlisted compatibility matrix covers the needs of candidate audio models. |
| 7 — Neural noise suppression | Can a useful denoiser meet quality and real-time constraints? | Pending | Quality, deadline, stability, cleanup, and CPU fallback gates all pass. |
| 8 — Other audio models | Which VAD, echo suppression, dereverb, embedding, or event models are worthwhile? | Pending | Each candidate has an evidence-backed value/cost decision. |
| 9 — Production runtime | Can `waved` schedule the NPU safely with fallback and observability? | Pending | Full audio acceptance passes without privileged recovery behavior. |

PoC 0 does not claim that custom models run. It establishes the controlled
lab required to make every later claim reproducible.

PoC 1 also remains host-only. It proves the ABI and code paths present in
specific hashed binaries; it does not make a new graph safe to execute.

PoC 2 is also host-only. It names sections from pinned public source and
decodes bounded metadata, but it leaves accelerator commands, coefficients,
patches, load states, dynamic I/O, and the connection region opaque. Format
knowledge and valid checksums do not make a mutated or foreign graph safe.

## PoC 0 evidence summary

Both controls used definition SHA-256
`9cf0a286874a1f2062383977b9a26136efba12e768ac14f4e26eef9b47e1df6a`.

| Field | Control 1 | Control 2 |
|---|---:|---:|
| Run ID | `poc0-first-control-20260728` | `poc0-second-control-20260728` |
| Host-side ADB duration | 340 ms | 182 ms |
| Mean of three native iterations | 4.243 ms | 2.618 ms |
| Output bytes | 332,800 | 332,800 |
| Output SHA-256 | `e621b213…52d4` | `e621b213…52d4` |
| Graph cleanup | 0 | 0 |
| Post-run ADB responsive | yes | yes |
| NPU parameters unchanged | yes | yes |
| Required hashes unchanged | yes | yes |

The six individual native observations were 8.198, 2.241, 2.290, 3.539,
2.155, and 2.160 ms. Their descriptive median is 2.266 ms and their range is
2.155–8.198 ms. This is a control observation, not a benchmark: the graph is
an ASR CTC component with zero-filled input, the sample is tiny, and no
thermal or scheduler controls were imposed.

See the full
[`PoC 0 lab notebook`](lab-notebook/2026-07-28-poc0-reproducible-lab.md)
and the tracked
[`live-device manifest`](manifests/wave-live-v1.1.6.6.json).
Raw envelopes and native logs are kept under `artifacts/npu/` and are
intentionally gitignored.

## Repository map

```text
docs/npu/
  README.md                         this status and architecture guide
  reproduction.md                  operator procedure
  manifests/
    README.md                       manifest policy
    wave-live-v1.1.6.6.json         hash-pinned live baseline
  lab-notebook/
    README.md                       notebook policy
    000-template.md                 required experiment record
    2026-07-28-poc0-...md           PoC 0 evidence
    2026-07-28-poc1-...md           PoC 1 evidence
    2026-07-28-poc2-...md           PoC 2 evidence
  abi/
    README.md                       PoC 1 result and reproduction
    prototypes.md                   recovered FFI and layouts
    stock-ctc-call-graph.md          live path lifecycle
    backend-map.md                  runtime and v3 audio backends
    error-taxonomy.md               failure and recovery contract
    runtime-abi-inventory.json       generated exact ELF evidence
    firmware-matrix.md              generated cross-firmware matrix
    ghidra-evidence.json             generated focused call/decompile evidence
  format/
    README.md                       PoC 2 result and reproduction
    field-map.md                    byte layout and confidence ledger
    corpus-summary.md               generated corpus distributions
    stock-nbg-inventory.json        generated per-file evidence

waved/scripts/npu/
  README.md                         harness developer guide
  experiments.json                  sole executable experiment source
  npu_lab.py                        host runner
  abi_corpus.json                   exact static-analysis corpus
  abi_inventory.py                  deterministic ELF inventory
  ghidra_targets.json               bounded decompiler targets
  ghidra_abi.py                     deterministic Ghidra export
  nbg_corpus.json                   exact NBG corpus definition
  nbg_format.py                     strict non-executing NBG parser
  tests/                             host contract tests

artifacts/npu/                       raw, local, gitignored run evidence
```

Stable hardware facts also live in
[`waved/docs/COMPUTE_HARDWARE.md`](../../waved/docs/COMPUTE_HARDWARE.md).
Research notebooks may contain provisional observations; facts move into an
authoritative document only after their evidence and limits are clear.

## Evidence language

Every important statement should be classed as one of:

- **Observed live** — returned by the connected device during a bounded,
  recorded read or experiment.
- **Static verified** — derived from a specific hashed binary, source file,
  or firmware artifact without executing it.
- **Inferred** — the evidence supports the explanation, but an alternative is
  still plausible.
- **Unknown** — no adequate evidence exists yet.

The distinction matters. For example:

- “The tested stock CTC graph executes” is observed live.
- “The graph target field is `0x1000000e`” is both statically parsed and
  observed in probe output.
- “The v3 `libmodel_infer.so` AES/ANS path is CPU/MNN-style and does not
  import the NPU stack” is static verified.
- “The private export-data branch can run an arbitrary handcrafted graph”
  remains unknown.
- “Changing a product ID makes a foreign NBG safe” is unsupported and is not
  tested.

## Safety boundary

Allowed without a new device-mutation review:

- host-side firmware and binary analysis;
- ADB state checks and bounded reads;
- read-only sysfs inspection;
- execution of a source-controlled experiment whose executable, arguments,
  environment, timeout, artifact hashes, and expected result are pinned;
- staging disposable research artifacts only under `/data/waved/npu-lab/`
  after that staging action is explicitly reviewed.

Requires explicit approval before the specific action:

- firmware flashing or upgrade;
- replacing a system library or stock model;
- loading or unloading a kernel module;
- stopping a normal service;
- rebooting;
- writing sysfs;
- replaying raw driver commands;
- executing an unverified target graph;
- changing boot configuration.

Never part of this program:

- blind mutation fuzzing against `/dev/galcore`;
- executing a graph whose product bytes were patched to bypass validation;
- overwriting a stock model;
- automatic NPU-driver reset after a hang.

## Noise-cancellation target

The first useful custom workload is neural noise suppression, but only after
PoCs 2, 3, 5, and 6 prove a custom graph route and the required operation
surface. Candidate models must:

- operate on bounded streaming frames;
- have a CPU quality/reference implementation;
- use operations and dtypes already proven on this NPU;
- preserve recurrent state explicitly if needed;
- meet the Wave audio deadline with margin;
- fail closed to the existing CPU path;
- never require reboot, driver reset, or stock-service replacement for
  recovery.

The existing DeepFilterNet-based CPU backend remains the correctness and
fallback baseline. NPU acceleration is an optimization, not a reason to make
audio capture fragile.

## Start here

1. Read [`reproduction.md`](reproduction.md),
   [`abi/README.md`](abi/README.md), and
   [`format/README.md`](format/README.md).
2. Run the host-only unit and ABI-layout tests.
3. Regenerate or verify static evidence before changing a recovered
   declaration.
4. List the committed device experiments.
5. Take a read-only snapshot.
6. Run only a reviewed definition when preflight hashes match.
7. Compare repeated envelopes and record the conclusion and limits.
