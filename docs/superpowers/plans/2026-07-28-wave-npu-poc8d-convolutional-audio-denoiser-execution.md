# Wave NPU PoC 8D: Convolutional Audio Denoiser Execution

**Date:** 2026-07-28
**Status:** Executed once — clean negative
**Authorization:** The device owner requested continuous, stepwise NPU and
noise-model PoCs in this session
**Depends on:** PoC 8C proved exact Conv2d graph construction and OpenVX
selection, while separately disproving general NBG export
**Safety class:** One fixed Conv2d graph execution through public TIM-VX; no
NBG import/export, CLC compiler, raw driver access, retry, or benchmark

## 1. Question

Can the Wave verify and execute the fixed convolutional audio denoiser through
the ordinary TIM-VX graph path?

This is not a retry of PoC 8C. It is a separate process, runtime root, native
interface, health envelope, and question. It makes no `CompileToBinary` call
and consumes no output from PoC 8C.

## 2. Fixed Graph and Signal

```text
input:       FLOAT32 [16,1,1,1]
weights:     FLOAT32 [3,1,1,1] = [0.25,0.50,0.25], constant
bias:        FLOAT32 [1] = [0], constant
operation:   Conv2d
padding:     SAME
stride:      [1,1]
dilation:    [0,0]
output:      FLOAT32 [16,1,1,1]
```

Fixed alternating input:

```text
[1,-1,1,-1,1,-1,1,-1,1,-1,1,-1,1,-1,1,-1]
```

Expected zero-padded convolution output:

```text
[0.25,0,0,0,0,0,0,0,0,0,0,0,0,0,0,-0.25]
```

Acceptance tolerance is `1e-6`. Input energy must be `16.0`, output energy
must be `0.125`, and the ratio must be `0.0078125` (approximately `21.07 dB`
attenuation of this high-frequency component).

## 3. Isolated Runtime

```text
/tmp/wave-npu-audio8d/bin/wave-npu-run-conv-audio-denoiser-v1
/tmp/wave-npu-audio8d/lib/libtim-vx.so
```

Environment:

```text
LD_LIBRARY_PATH=/tmp/wave-npu-audio8d/lib:/usr/lib
VIVANTE_SDK_DIR=/usr
VSI_NN_LOG_LEVEL=5
```

`libCLC.so` must be absent from the isolated root, `/usr/lib`, and `/lib`.
The quarantined library and every compile root are outside the loader path.

## 4. Native and Host Bounds

The executable accepts only `--describe` and `--execute-once`. It creates
exactly one Conv2d, uploads one fixed input, calls `Graph::Compile` once,
calls `Graph::Run` once, reads one output, validates every element and both
energies, releases scoped objects, and exits.

Bounds:

- one native process and one execution attempt;
- 8-second native watchdog;
- 12-second host deadline;
- exact OpenVX `conv2d` selector required once;
- no arbitrary tensor/model/operation/output argument;
- no `CompileToBinary`, NBG, raw ioctl, async graph, iteration loop, retry,
  reset, service action, module action, reboot, or recovery.

Preflight and postflight pin the exact device, kernel, boot ID, device node,
NPU parameters, runner, TIM-VX and stock DSO hashes, file modes, CLC absence,
no holders, no competing runner, and bounded kernel diagnostics.

## 5. Acceptance

The one attempt must print:

```text
AUDIO status=PASS mode=conv-direct iterations=1 input_energy=16.000000 output_energy=0.125000 max_error=0.000000
```

It must also select OpenVX `conv2d`, exit zero, leave no holder or process,
and preserve every health field without a new kernel alert.

A pass establishes a custom, constant-weight, audio-relevant convolutional
model executing on the Wave accelerator stack. It does not establish NBG
portability, streaming integration, real-time latency, or learned speech
quality.

## 6. Result

The single authorized attempt ran under:

```text
run:
  artifacts/npu/runs/poc8d-conv-audio-denoiser-direct-1-20260728
manifest sha256:
  e598463b990074e5c94b7fef7f760e48078201f710ebd9c7da928b24e99cd897
runner sha256:
  bf3e4716588055ed7c774135ed15a77ad068fddab52b362c0c0c03dfb05f2c9f
execution attempts:
  1
duration:
  158 ms
remote exit:
  6
status:
  FAIL (clean graph-verification negative)
```

The graph constructed the intended tensors and selected exactly:

```text
Instance OPENVX node with kernel "conv2d"
```

`Graph::Compile()` then returned false. The executable therefore did not
call `Graph::Run()` and did not read an output tensor. There was no timeout,
retry, recovery action, or second graph.

Preflight and postflight retained the same boot ID, kernel, device-node
identity, NPU parameters, required hashes, staged modes, CLC-absence state,
and empty holder set. The only raw snapshot difference was normal uptime
advance. There was no new kernel alert.

## 7. Conclusion

The direct-execution hypothesis failed at graph verification:

```text
TIM-VX tensor and Conv2d construction: PASS
OpenVX conv2d selection:              PASS
OpenVX graph verification:            FAIL
Graph::Run:                            NOT REACHED
output readback:                       NOT REACHED
device health:                         PASS
```

Together with PoC 8C, this establishes that this fixed FP32 Conv2d graph is
rejected both by the ordinary verification path and by `vxGenerateNBG`.
It does not show that convolution is absent from the hardware: the stock
Wave NBG corpus contains convolutional models, and compiled stock graphs run.
It shows that the exposed Wave 6.4.19 userspace stack does not accept this
new built-in graph through the tested public TIM-VX path.

PoC 8D is closed without retry. A learned noise-cancellation port now depends
on a genuine compatible compiler route—preferably the exact
`6.4.19:946407` CLC/program compiler or an offline Wave-targeted NBG
toolchain—not on another shape or datatype variation inside this PoC.
