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
- the available host simulator still emits the wrong NBG product target, so
  arbitrary custom models are **not yet unlocked**.

The detailed program design is in
[`2026-07-28-wave-npu-reverse-engineering-program-design.md`](../superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md).
PoC 0's implementation plan is in
[`2026-07-28-wave-npu-poc0-reproducible-lab.md`](../superpowers/plans/2026-07-28-wave-npu-poc0-reproducible-lab.md).

## Why this is a staged program

The Wave does not expose a documented custom-model SDK. Its stock software
contains several useful layers, but they solve different parts of the
problem:

```text
audio model or ONNX
        │
        │ compiler/target selection — not solved
        ▼
Wave-targeted NBG (product 0x1000000e)
        │
        │ libnnsdk ABI — direct use proven
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

The current blocker is above the runtime: the host SDK available in this
repository produces NBG target `0x10000020`, while the tested Wave graph
targets `0x1000000e`. The runtime rejects the former before execution. We do
not patch that product field and execute the result because the field selects
compiled hardware behavior; changing an identifier cannot make kernels
correct for different silicon.

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
| 1 — Runtime ABI and call graph | What are the exact loader, tensor, execution, and cleanup contracts? | Next | Recovered ABI map is backed by static callers and host tests. |
| 2 — NBG corpus and format | Which sections encode target, tensors, operations, constants, and relocation data? | Pending | Parser describes a representative stock corpus without executing modified graphs. |
| 3 — Runtime and driver trace | What runtime calls and driver interactions occur for a known graph? | Pending | Bounded traces correlate lifecycle phases without raw mutation fuzzing. |
| 4 — Firmware v3 audio loader | Does `libmodel_infer.so` provide a useful CPU, OpenCL, or NPU path for audio models? | Pending | Backend and accepted model format are proven, not inferred from strings. |
| 5 — Wave compiler gate | Can a minimal add/identity graph be compiled for product `0x1000000e`? | Blocked on research | A genuinely Wave-targeted graph passes create, execute, verify, and destroy. |
| 6 — Operation characterization | Which operations, shapes, dtypes, and state patterns work? | Pending | An allowlisted compatibility matrix covers the needs of candidate audio models. |
| 7 — Neural noise suppression | Can a useful denoiser meet quality and real-time constraints? | Pending | Quality, deadline, stability, cleanup, and CPU fallback gates all pass. |
| 8 — Other audio models | Which VAD, echo suppression, dereverb, embedding, or event models are worthwhile? | Pending | Each candidate has an evidence-backed value/cost decision. |
| 9 — Production runtime | Can `waved` schedule the NPU safely with fallback and observability? | Pending | Full audio acceptance passes without privileged recovery behavior. |

PoC 0 does not claim that custom models run. It establishes the controlled
lab required to make every later claim reproducible.

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

waved/scripts/npu/
  README.md                         harness developer guide
  experiments.json                  sole executable experiment source
  npu_lab.py                        host runner
  tests/test_npu_lab.py             fake-device contract tests

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
- “`libmodel_infer.so` uses the NPU for arbitrary ONNX” remains unknown.
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
PoCs 1–6 prove the compiler and operation surface. Candidate models must:

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

1. Read [`reproduction.md`](reproduction.md).
2. Run the host-only unit tests.
3. List the committed experiments.
4. Take a read-only snapshot.
5. Run only when preflight hashes match.
6. Compare repeated envelopes.
7. Record the conclusion and its limits in a notebook.

