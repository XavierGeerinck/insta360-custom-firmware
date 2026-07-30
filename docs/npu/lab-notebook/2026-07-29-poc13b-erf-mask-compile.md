# PoC 13B — Erf-Mask Compile-Only Capture

## Question

Can the exact Wave TIM-VX stack compile the external-input quantized
`Erf[481]` graph deterministically?

## Safety envelope

- exactly two native compiler processes: A1 and A2;
- no graph `Run`;
- no tensor input/output copy;
- no arbitrary model, shape, operation, output, or retry argument;
- fixed `/tmp/wave-npu-poc13b` root;
- no CLC, direct ioctl, firmware, sysfs, module, service, reset, or persistent
  device write.

The fixed compiler accepts only `--describe`, `--a1`, and `--a2`.

## Build note

The offline container build was invoked twice during development. The first
static-policy revision rejected a source comment containing a forbidden
device-path literal. Both builds produced the same binary. This did not add
a device compiler attempt: the only device compiler processes were A1 and A2.

## Result

```text
A1: PASS, 5,840 bytes, process duration 128 ms
A2: PASS, 5,840 bytes, process duration 131 ms
A1 SHA-256: 96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94
A2 SHA-256: 96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94
byte comparison: identical
health differences: none
kernel alerts: none
```

## Exact tensor contract

```text
input:  UINT8[481], asymmetric scale 0.03125, zero point 128
op:     Erf
output: UINT8[481], asymmetric scale 0.0078125, zero point 128
target: VPMN v2 product 0x1000000e
```

## Evidence

| Artifact | SHA-256 |
|---|---|
| Compiler binary | `f088cc4d1dc15b713be386ceee5af9ffe56a0a66fff56a1baab937492e9115d8` |
| Compile manifest | `2bd7cb989f3f166a5e24d55d9ba0b7e496fc2268cd68381ad094ad1e5d21a6f3` |
| A1 run | `aa9a6c860c273f0232fdabe57238b5c1911bb7cf3044fea5bad600e532725647` |
| A2 run | `de57fcf8a8f88211255f979267c7802808913562b1e04234f9b3e6a4b7d8ea66` |

## Reproduction boundary

```bash
waved/tools/npu-compiler/build-device-erf-mask.sh
waved/tools/npu-compiler/stage-device-erf-mask.sh
python3 waved/scripts/npu/npu_erf_mask_compile_lab.py run \
  --candidate a1 --run-id NEW-UNUSED-RUN-ID
```

The stage script refuses a pre-existing remote root. Do not delete prior
evidence merely to replay the command.

## Interpretation

The exact Erf graph is compile-qualified. The general compiler remains
unqualified. Conv, MatMul, FFT, and arbitrary ONNX remain unsupported.

## Next gate

Parse, reconstruct, and seal the two byte-identical NBGs without device
execution.
