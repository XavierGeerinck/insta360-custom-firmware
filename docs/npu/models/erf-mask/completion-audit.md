# Wave NPU Erf-Mask Completion Audit

**Audit date:** 2026-07-29

**Scope:** design criteria 1–13 from
`docs/superpowers/specs/2026-07-29-wave-npu-learned-erf-noise-suppression-design.md`

## Verdict

The research objective is proven: the Insta360 Wave NPU numerically executed
the learned Erf partition, its returned tensors were used in a causal
noise-suppression pipeline, and the resulting complete waveform passed the
quality and CPU-agreement gates.

The production milestone is incomplete. The exact model has not been ported
behind the Rust `NpuInfer` service, and the fixed-runner soak is not a
substitute for end-to-end service percentile, memory, reset, CPU-load, and
thermal qualification.

```text
research NPU noise-cancellation PoC: PROVEN
production Rust NPU backend:         INCOMPLETE
arbitrary ONNX or Acuity support:    NOT CLAIMED
```

## Requirement-to-Evidence Audit

| # | Requirement | Authoritative evidence and SHA-256 | Observed result | Classification |
|---:|---|---|---|---|
| 1 | Deterministic learned 962-parameter package | `artifacts/npu/runs/poc13a-erf-mask-host-20260729-model-v4-selector/model-manifest.json` — `bd7ffc58d253855e3ddc546def711f91812da8cb9ecd88dc658b56aada40ce5e` | 481 slopes plus 481 biases; deterministic weights and ONNX are hash-bound | **proven** |
| 2 | Held-out ablation and quality gates | `artifacts/npu/runs/poc13a-erf-mask-host-20260729-evaluation-v5-selector-final/evaluation-summary.json` — `32d7b224253e2a05f230555db7cfdf4bb6094c81d832d5793851a582a376ee2d`; host qualification — `ad286a54ca1c3311cb1a44818c777de2dd62dca1b8d155b715e076f03710693d` | 1,230 valid items; synthetic SI-SDR +3.2598 dB, natural +5.2338 dB, STOI +0.01355, noise attenuation 10.5838 dB, held-out clean RMS delta 0.04638 dB | **proven** |
| 3 | Exact U8 `[481]` Erf NBG compiles twice identically | A1 run — `aa9a6c860c273f0232fdabe57238b5c1911bb7cf3044fea5bad600e532725647`; A2 run — `de57fcf8a8f88211255f979267c7802808913562b1e04234f9b3e6a4b7d8ea66`; both NBGs — `96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94` | Two independent compile-only processes produced byte-identical 5,840-byte VPMN v2 files for `0x1000000e` | **proven** |
| 4 | Strict parser, CRC, metadata, signature, and corruption checks | `docs/npu/format/erf-mask-nbg-profile.json` — `9e5300363c5eca8bd9dae6db820128d4e9534f2aa6007a69fe10bf7b9bc9cab7` | CRC `0xf14f321a`, exact U8 tensor metadata, lossless ownership, zero conflicts, structural signature `236bd56d…`; negative tests pass | **proven** |
| 5 | Six bounded NPU vectors match CPU oracle | `artifacts/npu/runs/poc13d-erf-mask-six-vector-20260729/run.json` — `f8ffbca89fb96275e1c404ea780c1ccb62de3deda07820ec7308ccf6aaab7fac` | 2,886/2,886 bytes exact; max error 0; learned output has 63 unique values; canaries pass | **proven** |
| 6 | Strict numerical calls reached NPU without CPU fallback | Same PoC 13D run and fixed-runner policy; runner binary SHA-256 `99d51397b8c6b21c360755b6a66cc62eba3897287fb12a215468bd8f7e2f62d3` | One TIM-VX graph compiled, six NN SDK executions ran on the identified Wave target, and the runner contains no CPU Erf implementation or fallback path | **proven** |
| 7 | Complete waveform reconstructed from NPU-returned masks | Stream run — `6079c34599386f2024ac7787deacf01413231b5bf312f75b030292f2cb87bda2`; waveform evidence — `6589c517483fbf120e70176c46267c867f96dc2d0286d57521920dab2d93d09b` | 1,201/1,201 NPU frames, 577,681/577,681 bytes exact, 576,000 samples reconstructed, zero fallback | **proven** |
| 8 | Waveform passes quality and CPU/NPU agreement gates | Same waveform evidence; rendered WAV SHA-256 `30e015ed85faacd5198c70a3021fd59557103945bd19c24b496a58f4ee21286c` | Noise attenuation 9.8283 dB, SI-SDR +4.8443 dB, STOI +0.01083, clean transition 1.1822 dB within its 1.5 dB fixture gate, silence −85.6481 dBFS, CPU/NPU samples exact | **proven** |
| 9 | Production implementation runs behind Rust `NpuInfer` | No qualifying artifact | Python/lab integration and fixed native device runners exist; no Rust service backend was added | **incomplete** |
| 10 | Production strict, hybrid, CPU, and audit modes | Python fallback source SHA-256 `001295b27aea1b5d7a4547b1f78d7fcf49257aa370e7ba741a5da2d6f05240e4`; exact partition policy SHA-256 `f5c7d9a76e44a20e10d0af0f7e7cfba76c0de99a8cac616eb22b471f079de44d` | Strict fixed-device execution and Python CPU/hybrid behavior are proven; the four production service modes are not wired | **incomplete** |
| 11 | Same-frame fallback and no automatic retry | `waved/tools/onnx-hybrid/tests/test_erf_mask_device_stream.py` — `3b0d9dd9dfb3d60096c26e53bc3e365a1783ab8de5b56420c08548b3c9e080ff`; waveform evidence above | Injected first NPU failure recomputes that frame exactly on CPU, latches fallback, and makes only one NPU attempt | **proven at lab boundary** |
| 12 | 10,000 frames meet all latency, ordering, memory, health, and cleanup gates | `artifacts/npu/runs/poc13f-erf-mask-soak-10000-20260729/run.json` — `e4f3fc3d0de3c833dadf912563585c3737d7dc30c7338f2e26d9936077ee6f8d` | 10,000 ordered executions and 4,810,000 bytes covered; 10/10 oracle checkpoints; mean 84.8758 µs, max 1,649 µs; canaries and health pass. No p50/p95/p99, service RSS/CPU, reset, or thermal evidence | **incomplete as written; numerical/health subset proven** |
| 13 | Manifests, notebooks, hashes, and reproduction commands agree | This audit, `README.md`, four tracked JSON manifests, six PoC 13 notebooks, and the hashes rechecked below | Required research evidence is documented; explicit production non-claims are retained | **proven** |

## Reproduction and Verification

Host model and hybrid tests:

```bash
cd waved/tools/onnx-hybrid
uv sync --frozen
uv run pytest -q
uv run ruff check .
```

NBG parsers, policies, and device-harness tests:

```bash
uv --project waved/tools/onnx-hybrid run pytest -q waved/scripts/npu/tests
```

Static policy checks:

```bash
waved/tools/npu-compiler/verify-device-erf-mask-policy.sh \
  artifacts/npu/build/poc13b/wave-npu-compile-erf-mask-v1 \
  waved/tools/npu-compiler/tim_vx_compile_erf_mask.cc

waved/tools/npu-compiler/verify-device-erf-mask-qualification-policy.sh \
  artifacts/npu/build/poc13d/wave-npu-run-erf-mask-qualification-v1 \
  waved/tools/npu-compiler/wave_npu_run_erf_mask_qualification.cc

waved/tools/npu-compiler/verify-device-erf-mask-stream-policy.sh \
  artifacts/npu/build/poc13e/wave-npu-run-erf-mask-stream-v1 \
  waved/tools/npu-compiler/wave_npu_run_erf_mask_stream.cc

waved/tools/npu-compiler/verify-device-erf-mask-soak-policy.sh \
  artifacts/npu/build/poc13f/wave-npu-soak-erf-mask-v1 \
  waved/tools/npu-compiler/wave_npu_soak_erf_mask.cc
```

Evidence-hash verification:

```bash
shasum -a 256 \
  artifacts/npu/runs/poc13a-erf-mask-host-20260729-model-v4-selector/model-manifest.json \
  artifacts/npu/runs/poc13a-erf-mask-host-20260729-evaluation-v5-selector-final/evaluation-summary.json \
  artifacts/npu/runs/poc13a-erf-mask-host-20260729-qualified-v4-selector/host-qualification.json \
  docs/npu/format/erf-mask-nbg-profile.json \
  artifacts/npu/runs/poc13d-erf-mask-six-vector-20260729/run.json \
  artifacts/npu/runs/poc13e-erf-mask-fixture-stream-20260729/run.json \
  artifacts/npu/runs/poc13e-erf-mask-waveform-v2-20260729/waveform-evidence.json \
  artifacts/npu/runs/poc13f-erf-mask-soak-10000-20260729/run.json
```

The device stage scripts refuse pre-existing remote roots and intentionally
have no retry or recovery mode. Existing `/tmp/wave-npu-poc13*` evidence
must not be deleted merely to replay a run.

## Exact Claim Boundary

Supported:

- custom fixed TIM-VX graph generation for this quantized Erf contract;
- VPMN v2 parsing, hash binding, loading, and numerical execution;
- a real enhanced waveform materially influenced by Wave NPU output;
- partition-first hybrid execution with deterministic CPU fallback;
- sequential 10,000-frame numerical stability for the fixed partition.

Not supported:

- arbitrary ONNX import or full-model NPU residency;
- a complete Acuity compiler/profile;
- general Conv, MatMul, FFT, recurrent, or attention execution;
- a production-ready Rust/audio-service integration;
- a production real-time or thermal qualification claim.
