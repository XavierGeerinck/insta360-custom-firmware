# Speech enhancement candidates for the Wave

Research date: 2026-10-14. These are trained-model candidates with public
weights, not Wave/NPU-qualified models. VAD integration remains deferred until
the NPU compatibility work is complete ([backlog C20](../../../TODO.md#c--recording-and-audio-backend)).
The immediate compatibility experiment is SqueezeNet; this guide selects
subsequent audio workloads without changing production processing.

## Recommended candidates

| Role | Candidate | Available trained artifact | Why test it |
|---|---|---|---|
| Small recurrent compatibility baseline | [GTCRN](https://github.com/Xiaobin-Rong/gtcrn/tree/502ebfab64da7c4a9af78dcb9c6ceef1ebb01c73), 16 kHz | Author's PyTorch checkpoints and streaming ONNX, MIT | Author reports 48.2K parameters and 33 MMAC/s. Adds GRU, state, ConvTranspose and normalization beyond the CNN profiles. |
| Lightweight quality contender | [UL-UNAS](https://github.com/Xiaobin-Rong/ul-unas/tree/00f7c700da43d38347f30a6ccebd86fcbc798e07), 16 kHz | Author's trained checkpoint and streaming ONNX, MIT | [Paper](https://arxiv.org/html/2503.00340v2) reports 171K parameters / 35 MMAC/s and stronger results than GTCRN on its evaluation set. More graph nodes/recurrent units can still increase runtime. |
| Stronger full-band denoising | [DPDFNet2_48khz_hr](https://huggingface.co/Ceva-IP/DPDFNet/tree/dd6818d00f50c836fed43a6243ebe49116de5964), 48 kHz | Official checkpoint/ONNX/TFLite, Apache-2.0 model card | Matches the Wave's capture sample rate; substantial quality candidate with 2.58M parameters and explicit streaming state. Native compatibility and complete latency remain unmeasured. |
| Joint echo cancellation, noise reduction and dereverberation | [LocalVQE v1.2](https://huggingface.co/LocalAI-io/LocalVQE/tree/29ca38495cba9d6393a92a4dd890f28dd81f758d), 16 kHz | Project-author PyTorch/GGUF weights, Apache-2.0 code/card | Actual downloadable joint model, 1.3M parameters. No ONNX artifact in the inspected release; export and equivalence proof are new work. |
| Established neural AEC control | [DTLN-AEC128](https://github.com/breizhn/DTLN-aec/tree/9d24e128b4f409db18227b8babb343016625921f), 16 kHz | Original-author two-part TFLite model, MIT repository | Provides a reproducible microphone/reference AEC baseline; 1.8M parameters. Original tree contains TFLite, so ONNX conversion needs its own proof. |

Published quality/timing figures are upstream evidence, not measurements on
the Wave. DPDFNet's [paper](https://arxiv.org/html/2512.16420v1) reports improved
quality against DFN3 on its particular 16 kHz evaluation sets; those results
do not measure the selected 48 kHz checkpoint. LocalVQE's reported Ryzen timing does not establish performance on the Wave's
Cortex-A55. Its Pi5 timing refers to the separate 49K-parameter compact
GTCRN-AEC variant, rather than the 1.3M-parameter v1.2 model.

Keep the existing DeepFilterNet3 as a quality and runtime reference. The Wave
has already measured roughly 84% of one worker core during speech and about
8.5 ms per 10 ms hop with tract FP32, with uplink underruns in the serial DSP
workload. That is evidence about the current runtime/pipeline, not proof that
another runtime or NPU conversion will solve it. See
[compute measurements](../../../waved/docs/COMPUTE_HARDWARE.md).

## Microphone input and playback output

For microphone/uplink speech, evaluate noise suppression and dereverberation
on Wave recordings. AEC additionally needs a synchronized reference of what
the local speakers actually play, including reference delay and clock drift.
The [DTLN-AEC runner](https://github.com/breizhn/DTLN-aec/blob/9d24e128b4f409db18227b8babb343016625921f/run_aec.py)
uses paired microphone/loopback signals. Test echo-only, near-end-only,
double-talk, moving echo paths and speaker distortion; suppressing near-end
speech during double-talk must not count as successful echo removal.

For received speech before playback, test a separate denoising instance and
state. A single-stream playback denoiser does not cancel the acoustic echo
in the microphones. Speech-enhancement results also do not establish
preservation of music, alerts or mixed media. These are two evaluation paths,
not a claim that one model should process every input/output signal.

The commonly linked [public DeepVQE implementation](https://github.com/Xiaobin-Rong/deepvqe/blob/615ca55a5c093bf6f6f4a13f6c6a6afd2bba33eb/README.md)
omits the alignment block and supplies a noise-suppression checkpoint. It is
not the joint Microsoft AEC/NS/dereverb model from the paper. No original
Microsoft joint checkpoint was verified in this bounded search. LocalVQE and
DTLN-AEC are more concrete downloadable AEC candidates for this investigation.

## Actual host artifact inspection

Three revision-pinned ONNX files were downloaded and checked statically.
No audio model inference or Wave execution was performed.

| Artifact | Exact identity | Observed graph constraints |
|---|---|---|
| GTCRN `gtcrn_simple.onnx` | 535,190 B; SHA `b4718df6228e7bdf1a8a435cf98f838636eb2fd331acabf86ba87c5192ebcb87`|IR6/opset11, 415 nodes, 14 GRU / 11 ConvTranspose; spectrum plus three FP32 state inputs,18,048 state values|
| UL-UNAS `ulunas_stream_simple.onnx` | 788,967 B; SHA `f2e804d54d6a88f4f82f44d86c9f1cf646db2509bfca935cfbfc5fcd8cbfac3b`|IR6/opset11, 697 nodes, 28 GRU / 5 ConvTranspose; spectrum and three state inputs|
| DPDFNet2_48khz_hr | 10,493,337 B; SHA `7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b`|IR8/opset17, 493 nodes, 4 GRU / 8 LayerNormalization / 22 Gemm / 14 MatMul; FP32 state 56,436 values|

Their multiple inputs/outputs, state, recurrent operations and original FP32
precision exceed the qualified MarbleNet profile and the SqueezeNet candidate contract.
Bidirectional recurrence over frequency within a frame does not itself imply
future temporal context. For DPDFNet, a 225,744 B state copied in/out every 10 ms
would move about 45.1 MB/s before spectrum and other overhead; persistent state
matters as much as MAC counts.

Do not equate hop duration with total algorithmic delay. DPDFNet's wrapper
describes 20 ms initial buffering / 10 ms hops, while its trained design and
[streaming mask](https://github.com/ceva-ip/DPDFNet/blob/9bd9844a227bb6aa57e55588d8d0e961fcff1c46/onnx_model/dpdfnet_48khz_hr.py)
include two delayed frames. Complete waveform latency/alignment remains
unverified. GTCRN/UL-UNAS use 32 ms analysis windows / 16 ms hops; DTLN-AEC uses
32 ms windows / 8 ms hops. Measure input-to-output alignment rather than quoting
hop size as latency.

## Test order after spatial compatibility

1. Establish GTCRN CPU/stateful sequence references, then small native GRU,
   ConvTranspose and normalization controls before considering full offload.
2. Compare UL-UNAS and DPDFNet against the same clean/noisy recordings and DFN3
   reference, measuring speech preservation, quality, hop deadlines and memory.
3. Evaluate DTLN-AEC128 and LocalVQE separately with paired mic/reference audio
   and double-talk cases. Prove any export or precision conversion first.

The detailed search report and exact static inventories remain under
`artifacts/npu/poc20/audio-model-research.md` and `audio-model-sources/`.
Wave compatibility, sustained timing, quality on Wave recordings and complete
latency are open questions. These recommendations authorize no integration or
unbounded native experiment beyond the owner's current SqueezeNet test.
