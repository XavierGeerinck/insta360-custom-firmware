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
