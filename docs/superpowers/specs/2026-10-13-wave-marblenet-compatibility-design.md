# Wave MarbleNet compiler isolation and compatible lowering

Approved scope: the owner's `go` accepts the preceding recommendation to
isolate kernel width from dilation, trace the exact rejection with REA,
preserve learned weights in a compatible lowering, qualify remaining layers
and compare the complete neural graph with the measured CPU baseline.
This records that approval; no second confirmation is required.

Reuse the exact checkpoint, FP16 exports, runtime, firmware SDK and offline
toolchain from PoC18. Its negative and partial diagnostic-history result
remain immutable. New artifacts belong to `artifacts/npu/poc19/`; existing
qualified packages and the working CPU fallback remain usable.

## Architecture and staged decisions

First produce two diagnostic Conv/pointwise/Relu variants of frozen18C:
K13/rate2/pad12 and K29/rate1/pad14, same FP16 `[1,64,200]` input and
`[1,128,200]` output. K29 retains all original learned taps; K13 retains
the centered13 tap bits of the same K29 tensor. Pointwise weights/bias and
activation remain identical. These are diagnostic graphs, not complete
trained-model equivalents. Actual ORT and independent oracle references
are bound before source/build review. Each compiles once; a successful
export receives one qualification Run. A clean compiler rejection does not
prevent the other independent isolation control.

REA independently follows the exact `libOpenVX.so` initializer and its
callees, reusing retained evidence and opening at most one owned session.
Distinguish machine-level branch facts, source hints and candidate causes;
correlate them with actual isolated compiler outcomes. No live attachment,
vendor helper execution, firmware patch or driver setting change is needed.

Use the resulting supported signatures to choose an exact rewrite. Prefer
small supported convolutions plus tensor movement/addition if their public
native route and complete host equivalence are established. A zero-expanded
K57 is an alternative only if evidence establishes a useful supported route.
No algebraic identity by itself establishes native admission or preserved
FP16 accumulation. The complete checkpoint's original FP32 probabilities
remain the final reference. Do not retrain, omit a layer, silently change
weights, or call a partial offload a complete NPU graph.

Before launching any compatibility control, add its exact source/graph
contract, hashes and numerical tests to the reviewed plan. Validate trained
remaining blocks and full graph after successful prerequisites. If a native
route cannot be established, finish with the exact measured constraint and
working CPU fallback, rather than improvising a target or claiming success.

## Numerical and lifecycle gates

Preserve PoC18 finite element policy `abs(error)<=0.02+0.02*abs(reference)`;
whole-model speech probabilities differ by at most0.02 and threshold0.5
decisions agree at least99% across the four frozen clips. All taps, tensor
geometry, model/lowering/source/ELF/NBG/input/readback and CPU/independent
references remain separately bound. Qualified loading creates no CPU session
or transport. The PoC17 byte-exact policy is unchanged.

Use native compile15s/host20s and native execution8s/host12s, keeping alarms
armed through teardown. Review sources and inspected ELF before each new
launch. No retry of a native control. Stop all further native work on fault,
timeout, ambiguous exit, failed numerical comparison or changed health.
Clean compiler negatives skip dependent stages while allowing the already
specified independent isolation control.

The new budget is at most8 compiler processes/16 size-export calls and
46 NPU Runs: at most6 control Runs,4 full-clip Runs,4 warm and32 measured
full-graph Runs. The initial isolation consumes at most2 compiles/4 public
export calls/2 Runs. Later controls require explicit recorded adaptive
selection within this ceiling, source review and successful dependencies.
Unused budget is not a reason to launch another graph. No live retries,
recovery, service stops, driver/sysfs writes, stock library replacement or
incompatible CLC are admitted.

Capture bounded diagnostic overlap between stages, not only around native
processes. Preserve original snapshots and record any aged-out history as
inconclusive. Final state checks include stock services, boot, device/driver
parameters, exact system hashes, all derived roots absent and no lab process.
CPU comparison timings exclude feature extraction and orchestration exactly
as the8.630792ms baseline does. Keep the about1.46s lookahead limitation.

## Deliverables

Reviewed/frozen isolation packages and actual outcome matrix; REA evidence
for the rejecting path and any unresolved condition; tested compatible
lowering and full-model qualification/timings when achievable; host tests,
durable notebook/result ledger and updated guide with observed coverage only.
Keep work in the existing feature checkout, commit owned files only, and
do not push, merge or deploy a production service.
