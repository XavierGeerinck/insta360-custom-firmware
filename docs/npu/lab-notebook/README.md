# Wave NPU Lab Notebook

This directory is the append-only research record for individual NPU PoCs.

Use [`000-template.md`](000-template.md) for each new experiment. A notebook
is not merely a success report: it must preserve the hypothesis, safety gate,
exact identities, negative observations, limits, and decision that follows
from the evidence.

## Naming

Use:

```text
YYYY-MM-DD-pocN-short-purpose.md
```

If one PoC has several independent experiments, add a descriptive suffix
rather than overwriting an earlier record.

## Raw versus tracked evidence

Raw envelopes and native output live under `artifacts/npu/` and are ignored by
Git. A tracked notebook records:

- run IDs;
- experiment-definition digest;
- binary/model/runtime hashes;
- raw log hashes when important;
- semantic output;
- safety postconditions;
- enough command detail to locate and reproduce the raw evidence.

Do not paste complete binary dumps, complete kernel logs, audio buffers,
credentials, or device identifiers into a notebook.

## Evidence graduation

A notebook may contain provisional or inferred observations. Graduate a fact
to one of these only after it is stable:

- `docs/npu/manifests/` for an exact stack identity and control baseline;
- `docs/npu/README.md` for program status and supported conclusions;
- `waved/docs/COMPUTE_HARDWARE.md` for durable device facts;
- a parser or unit test for binary-format and ABI behavior.

Negative results graduate too. A cleanly rejected product target is a useful
compiler constraint, not a failed research session.

## Completed records

- [`2026-07-28-poc0-reproducible-lab.md`](2026-07-28-poc0-reproducible-lab.md)
  — two live, allowlisted stock-CTC controls and unchanged-state evidence.
- [`2026-07-28-poc1-runtime-abi.md`](2026-07-28-poc1-runtime-abi.md)
  — host-only runtime ABI, call graph, backend, and error recovery.
- [`2026-07-28-poc2-nbg-corpus-format.md`](2026-07-28-poc2-nbg-corpus-format.md)
  — host-only NBG v1/v2 parser, 139-file corpus, decoded metadata, and
  690/690 section-checksum validation.
- [`2026-07-28-poc3-runtime-driver-trace.md`](2026-07-28-poc3-runtime-driver-trace.md)
  — two live metadata-only traces, stable 73-call GAL sequence, exact output
  control, unchanged state, and the retained PTY-framing negative pilot.
- [`2026-07-28-poc5-custom-compiler.md`](2026-07-28-poc5-custom-compiler.md)
  — host/device compiler ladder, exact hardware capabilities and selector
  behavior, the first Wave-targeted generated NBG, missing CLC boundary, and
  the quarantined version-adjacent compiler.
- [`2026-07-28-poc6-7-custom-nbg-execution.md`](2026-07-28-poc6-7-custom-nbg-execution.md)
  — zero and nonzero execution of the generated Erf NBG with exact tensor
  readback and unchanged device health.
- [`2026-07-28-poc8-audio-denoiser-boundary.md`](2026-07-28-poc8-audio-denoiser-boundary.md)
  — fixed MatrixMul and Conv2d denoiser representations, four clean negative
  compiler/verification results, model-portability matrix, and the next
  compiler requirement.
- [`2026-07-28-poc9a-architecture-database-recovery.md`](2026-07-28-poc9a-architecture-database-recovery.md)
  — host-only five-key architecture lookup, exact and simulator database-row
  recovery, revision-family mask, ABI-size deltas, strict evidence profile,
  and explicit compiler-profile unknowns.
- [`2026-07-28-poc9c-viv-gpu-file-parser-recovery.md`](2026-07-28-poc9c-viv-gpu-file-parser-recovery.md)
  — static and isolated-host recovery of the x86 Vivante five-field config
  parser, 358-entry target catalog, 13-case behavior matrix, hardened
  parser/renderer, and a Wave candidate with an explicit compiler refusal.
- [`2026-07-28-poc9d-compiler-acquisition-static-qualification.md`](2026-07-28-poc9d-compiler-acquisition-static-qualification.md)
  — trusted Allwinner package acquisition, complete ZIP/OCI provenance,
  exact vendor Wave profile, byte-locked no-kernel parser, Acuity NBG
  boundary, and a closed `>=6.4.22` graph-compiler blocker with zero
  compiler/device calls.
- [`2026-07-28-poc9e-9f-onnx-cpu-fallback.md`](2026-07-28-poc9e-9f-onnx-cpu-fallback.md)
  — bounded general ONNX admission, recursive subgraph inventory,
  deterministic graph identity, CPU numerical oracle, fail-closed hybrid
  semantics, and a non-executing 689-node Silero VAD audit.
- [`2026-07-28-poc12a-open-nbg-serializer.md`](2026-07-28-poc12a-open-nbg-serializer.md)
  — exact 21-target serializer/load evidence, total byte ownership and
  byte-identical re-emission for five NBGs, bounded ONNX coefficient
  correlation, and an evidence-backed `not-ready` decision for coefficient
  packing.
- [`2026-07-29-poc12b-erf-constant-differential.md`](2026-07-29-poc12b-erf-constant-differential.md)
  — three compile-only stock-serializer captures, deterministic A1/A2
  output, a one-byte source control, sealed finite-registry differential
  evidence, and exact device cleanup.
- [`2026-07-29-poc12c-dyna-io-crc-profile.md`](2026-07-29-poc12c-dyna-io-crc-profile.md)
  — host-only recovery of the v2 header CRC order and section-20/26 table
  geometry, 138/138 corpus validation, 8,939 bounded entries, and an exact
  in-memory reconstruction of the controlled B graph.
- [`2026-07-29-poc13a-learned-erf-mask-host.md`](2026-07-29-poc13a-learned-erf-mask-host.md)
  — deterministic 962-parameter training, 1,230-item held-out evaluation,
  retained negative candidates, and the final causal DFN-or-Erf selector.
- [`2026-07-29-poc13b-erf-mask-compile.md`](2026-07-29-poc13b-erf-mask-compile.md)
  — two independent compile-only processes producing the same exact
  Wave-targeted U8 `[481]` Erf NBG.
- [`2026-07-29-poc13c-erf-mask-package.md`](2026-07-29-poc13c-erf-mask-package.md)
  — strict VPMN v2 profile, CRC and tensor contract, lossless ownership, and
  the correction that this external-input graph has no DYNAIO record.
- [`2026-07-29-poc13d-erf-mask-numerical.md`](2026-07-29-poc13d-erf-mask-numerical.md)
  — six fixed NPU vectors, 2,886 exact oracle bytes, nonconstant learned
  output, canaries, and unchanged health.
- [`2026-07-29-poc13e-erf-mask-waveform.md`](2026-07-29-poc13e-erf-mask-waveform.md)
  — 1,201 NPU frames used to reconstruct a complete quality-qualified
  12-second enhanced waveform, plus tested same-frame CPU fallback.
- [`2026-07-29-poc13f-erf-mask-soak.md`](2026-07-29-poc13f-erf-mask-soak.md)
  — 10,000 consecutive NPU executions with ten exact oracle checkpoints,
  bounded timing, canaries, and unchanged health.
- [`2026-07-29-poc13g-wave-cpu-vs-npu.md`](2026-07-29-poc13g-wave-cpu-vs-npu.md)
  — same-device Cortex-A55/NPU timing for 10,000 paired Erf frames,
  including the retained R1 negative, raw nearest-rank statistics, exact
  numerical equality, and the `cpu-faster` result.
- [`2026-07-29-poc14a-affine-host.md`](2026-07-29-poc14a-affine-host.md)
  — deterministic integer affine-plus-Erf contract, finite quantizer
  registry, six vectors, byte-identical package regeneration, and complete
  held-out noise-suppression quality gates.
- [`2026-07-29-poc14b-affine-compiler-negative.md`](2026-07-29-poc14b-affine-compiler-negative.md)
  — one bounded compile-only clean negative, exact elementwise type-table
  correlation, skipped prohibited retries, unchanged health, exact cleanup,
  and the resulting serializer refusal.

The consolidated result, reproduction guide, and exact production-gap audit
are in [`../models/erf-mask/README.md`](../models/erf-mask/README.md) and
[`../models/erf-mask/completion-audit.md`](../models/erf-mask/completion-audit.md).

## Review checklist

Before marking a notebook complete:

- [ ] Hypothesis is falsifiable.
- [ ] PoC and experiment ID are explicit.
- [ ] Safety class and prohibited actions are explicit.
- [ ] Definition, probe, runtime, and model hashes are recorded.
- [ ] Preflight passed before native work.
- [ ] Native result and remote exit are recorded.
- [ ] Post-state and cleanup are recorded.
- [ ] Timing is labeled benchmark or descriptive.
- [ ] Positive, negative, inferred, and unknown findings are separated.
- [ ] Raw evidence locations are recorded.
- [ ] Conclusion says what becomes allowed next—and what does not.
