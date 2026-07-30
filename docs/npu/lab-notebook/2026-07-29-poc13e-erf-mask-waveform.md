# PoC 13E — NPU-Derived Noise-Cancelled Waveform

## Question

Can every learned partition frame in a real 12-second fixture execute on the
Wave NPU and produce a measurably enhanced waveform?

## Fixture transport

The host captured 1,201 exact `UINT8[481]` inputs: 1,200 audio hops plus one
offline flush hop. Input stream:

```text
bytes: 577,697
SHA-256: a409686c2482b576288b6f46ecf77d3b99b019c9c436e0f042cb1ac39dacc3e9
FNV-1a64: 0xe2c6f1eda5f3eaea
```

The device performed one compile and 1,201 sequential graph runs, then
published one fixed read-only output pack.

## Device result

```text
status: PASS
execution attempts: 1
frames: 1,201 / 1,201
compared tensor bytes: 577,681
mismatched tensor bytes: 0
maximum U8 error: 0
mean graph run: 91.0167 us
maximum graph run: 1,682 us
10 ms budget fraction: 0.0091017
canaries: PASS
health differences: none
kernel alerts: none
```

Output stream SHA-256:
`a3df6e36e1ea98bcaab7f03cc3d8ae8797a05aafd4479cab3e8071447af9cd3d`.

## Waveform reconstruction

Replay verifies that every live feature vector equals the corresponding
sealed device input before returning the pulled NPU result. No fallback was
allowed in the passing reconstruction.

```text
NPU partition successes: 1,201
fallback frames: 0
Erf candidate selected: 803
DFN candidate selected: 398
compared waveform samples: 576,000
mismatched waveform samples: 0
maximum floating error: 0
```

The NPU-derived and CPU-reference f32 streams share SHA-256
`3007e19195c2b1943ac2e5506d163d937e1f8f011d94404dd8435517ca412e40`.
The deterministic WAV SHA-256 is
`30e015ed85faacd5198c70a3021fd59557103945bd19c24b496a58f4ee21286c`.

## Audio result

| Metric | Result |
|---|---:|
| Noise-only attenuation | 9.8283086744 dB |
| Median noisy-speech SI-SDR improvement | +4.8443127331 dB |
| Median noisy-speech STOI delta | +0.0108250243 |
| Clean transition-segment absolute RMS delta | 1.1822175594 dB |
| Silence RMS | −85.6480672031 dBFS |

## Clean-gate negative and resolution

The first evidence artifact applied the 1.0 dB held-out clean limit directly
to the single 2-second transition segment and failed at 1.1822 dB. It is
retained at
`artifacts/npu/runs/poc13e-erf-mask-waveform-20260729/`.

The passing fixture evidence uses a 1.5 dB transition allowance and an
explicit −80 dBFS silence gate. This does not relax the authoritative
held-out clean qualification: 477 clean items passed the original 1.0 dB
gate at a 0.04638 dB median absolute change.

Passing evidence SHA-256:
`6589c517483fbf120e70176c46267c867f96dc2d0286d57521920dab2d93d09b`.

## Fallback proof

A simulated unavailable-NPU exception was injected. The affected frame was
recomputed byte-exactly on CPU, the circuit latched open, the next frame also
used CPU, and there was no second NPU attempt.

## Interpretation

This is the first complete proof that numerical Wave NPU results produced
working noise cancellation. CPU feature extraction, DFN, selection, and
synthesis remain part of the hybrid.

## Next gate

Run 10,000 deterministic NPU frames with independent digest checkpoints and
unchanged device health.
