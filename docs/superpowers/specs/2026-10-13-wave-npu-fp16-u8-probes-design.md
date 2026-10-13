# Wave NPU FP16 Conv and U8 FC probes

Approved by the owner with `go` on 2026-10-13 after the design in the
[REA notebook](../../npu/lab-notebook/2026-10-13-rea-op-dtype-recovery.md).

## Goal and evidence

Test whether two signatures admitted by the firmware can compile to native
Wave NBGs and, if they do, execute with correct numerical outputs. The NN
format checker rejects FP32 weights. Its FP16 and quantized branches have
additional gates, so this experiment records clean negatives as well as
successful outputs. It does not promise broad ONNX compilation.

## Fixed models

1. **16A, FP16 Conv:** input/output `[16,1,1,1]`, FP16 weights `[3,1,1,1]`
   with exact half encodings `0x3400,0x3800,0x3400`, FP32 zero bias `[1]`,
   SAME padding, stride `[1,1]`, dilation `[0,0]`. Alternating `+1,-1`
   input produces half encodings `0x3400,0,...,0xb400`.
2. **16B, U8 FC:** input `[16,1]`, weights `[16,4]`, output `[4,1]`, axis
   `0`, units `4`. Asymmetric scale `1`, zero point `0` on input, weights
   and output; INT32 zero bias `[4]` with scale `1`. The four columns
   select indices `0,5,10,15`. Input `0..15` produces `[0,5,10,15]`.

## Runtime and process boundaries

Build TIM-VX commit `702715d714277376d7ff466b7b72833ff57564c1` against
the existing device-matched SDK and record the wrapper's actual digest.
Use an immutable local Docker image ID, with no compiler/simulator library
replacement. Reject any SDK libCLC copy that is not the existing inert
symlink to libOpenCL; no CLC compiler call is permitted. Stage the wrapper,
one fixed compiler and the header under separate new `/tmp` roots
`/tmp/wave-npu-compile16a` and `/tmp/wave-npu-compile16b`.

Each compiler accepts only `--describe` or `--candidate-1`, pins its paths
and tensor constants, checks the Wave identity, uses a 15-second native
alarm and parent-death signal, and writes its NBG atomically without
overwriting files. One size-query and, on success, one export call are
permitted per model: four public export calls maximum. No graph runs occur
inside these binaries. Enable `VIV_VX_DEBUG_LEVEL=1` and
`VIV_VX_ENABLE_PRINT_TARGET=1` for their compile transactions.

Reuse the existing strict compile harness for exact device/kernel/runtime
hashes, empty galcore holders, boot identity, diagnostics, output metadata,
20-second host deadlines, and unchanged health. Preserve each raw run.
Verify VPMN v2, target `0x1000000e`, tensor contract and stored checksums
using the existing host parser before any execution.

Each successful artifact is eligible for one separate bounded NBG-import
execution against its fixed oracle with canary-initialized output. Bind the
execution manifest to the new NBG and runner hashes. Any fault or health
change ends all remaining live work. Clean compiler rejection ends that
candidate without retry; the other independent candidate may continue.

## Deliverables and acceptance

Keep native probe sources, reproducible build instructions, fixed host
policies and manifests, meaningful refusal/oracle tests, and a notebook
recording positive and negative results. Preserve firmware and service
state and remove only this experiment's staged files after capture.
General custom-model support remains unclaimed unless native export and
oracle-backed execution actually succeed.
