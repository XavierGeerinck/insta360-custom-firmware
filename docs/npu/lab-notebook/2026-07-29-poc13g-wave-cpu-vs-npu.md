# PoC 13G — Wave Cortex-A55 versus Wave NPU

**Date:** 2026-07-29
**Final result:** PASS in the separately sealed R2 experiment
**Performance classification:** `cpu-faster`

## Question

The earlier PoC 13F soak measured `graph->Run()` on the Wave NPU, but it did
not measure the same operation on the Wave CPU. Host-Mac timings are not a
valid CPU/NPU comparison. PoC 13G therefore asks one narrow question:

> For the exact quantized Erf transform used by the learned mask partition,
> how fast is one `UINT8[481]` frame on the Wave Cortex-A55 compared with the
> same frame on the Wave NPU?

This is not a peak-throughput benchmark and not a claim about arbitrary
models. It measures one tiny operation whose CPU implementation is a
256-byte lookup table.

## Fixed workload

Both paths consumed the same deterministic xorshift32 frame before the next
frame was generated:

```text
input:             UINT8[481]
seed:              0x5733130f
CPU operation:     embedded 256-byte quantized-Erf LUT
NPU operation:     sealed Erf NBG
cold frames:       1
warm-up frames:    100
measured frames:   10,000
total per path:    10,101
measured ordering: even CPU->NPU, odd NPU->CPU
```

The CPU table SHA-256 is
`c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296`.
The NBG SHA-256 is
`96447d6afd63a7c349bbd80f0af99150c89ef24ed7f1ef66ab285075362e6c94`.

One graph was created and compiled before the cold run. The NPU total
interval surrounds input copy, `graph->Run()`, and output copy. CPU wall time
covers exactly 481 indexed output writes plus the two thread-clock calls
inside the wall-clock bracket.

## Timing and validation

Wall durations use `CLOCK_MONOTONIC_RAW`. Thread and complete-process CPU
durations use `CLOCK_THREAD_CPUTIME_ID` and
`CLOCK_PROCESS_CPUTIME_ID`. Before graph execution the runner sampled
100,000 back-to-back wall-clock pairs:

```text
mean pair:     85.15458 ns
minimum pair:  41 ns
```

The 10,000 records are serialized little-endian into one fixed 640,036-byte
file. The host validates every sequence number, alternating-order tag,
mandatory positive duration, byte count, remote/local SHA-256, native/host
FNV-1a64, and nearest-rank percentile.

Each output buffer has 32-byte leading and trailing canaries. Every CPU byte
was compared with the NPU byte before advancing.

## R1 clean negative

The first sealed implementation at commit `7b08f61` was run exactly once
under `/tmp/wave-npu-poc13g`. It produced a valid cold result and then
failed closed at paired frame 4,833 with exit 7.

```text
failure frame:       4,833
normal debug markers: 4,833 after the cold run
timing file:         absent
partial timing file: absent
health differences:  none
kernel alerts:       none
holder leak:         none
retry:               none
```

Frame 4,833 was NPU-first and the runtime emitted its normal run marker
without an error. The supported diagnosis was a resolution-limited zero
tick around the sub-microsecond CPU thread-time interval. R1 combined its
clock/copy/run checks, so this remains an inference rather than a directly
identified failing call.

R1 also used `VSI_NN_LOG_LEVEL=5`, which printed one debug line per
inference and therefore was unsuitable for a timing result. Its evidence is
retained at
`artifacts/npu/runs/poc13g-erf-wave-cpu-vs-npu-20260729`; its remote root
was not deleted or replayed.

## R2 correction

R2 is a separately built binary, manifest, run directory, and remote root:

```text
remote root:       /tmp/wave-npu-poc13g-r2
runtime log level: 2 (warnings/errors)
binary SHA-256:    cc9154961833124c138553e15e572442033bd3c40b26ca302cd32aed40715799
source SHA-256:    07feee816c2c5958b38f4965a076d061ae7bca9fb8d381f5d1010fe109c8be91
git commit:        ef5af98582c9db8d851d397d1e6b1af851c257f5
```

R2 records equal CPU thread timestamps as resolution-limited zero samples.
CPU wall time and every NPU duration must remain positive. The native and
host zero counts must agree. R2 also identifies every clock, copy, run, and
duration stage separately on failure.

The full host gate before R2 was:

```text
ONNX-hybrid: 187 passed
NPU suite:   346 passed, 302 subtests passed
Ruff:        check and format PASS
build:       two byte-identical offline AArch64 binaries
policy:      source and ELF PASS
```

## Numerical result

R2 passed on its only execution:

```text
CPU frames:             10,101
NPU graph runs:         10,101
bytes compared/path:    4,858,581
mismatched bytes:       0
canaries:               PASS
input FNV-1a64:         0x367c7eaad959d707
CPU output FNV-1a64:    0x742e3a62d58acd0d
NPU output FNV-1a64:    0x742e3a62d58acd0d
final PRNG state:       0x6ae16fe0
health differences:     none
kernel alerts:          none
```

The raw timing file SHA-256 is
`7998b0df5000c0140b5c3229b244dbed8d158bbfc42a025fb741ff478dc0eb32`.
Its FNV-1a64, independently reproduced by the host, is
`0x3966c5381aecace1`.

## Performance result

All values below are uncorrected nanoseconds over 10,000 steady-state
frames. Percentiles use nearest rank.

| Boundary | Mean | p50 | p95 | p99 | p99.9 | Max |
|---|---:|---:|---:|---:|---:|---:|
| CPU wall | 3,303 | 3,333 | 3,458 | 3,500 | 3,958 | 18,417 |
| CPU thread | 2,372 | 2,416 | 2,500 | 2,542 | 2,667 | 5,500 |
| NPU copy in | 7,653 | 7,583 | 7,875 | 8,584 | 23,083 | 31,375 |
| NPU `Run()` | 68,330 | 65,750 | 74,458 | 86,083 | 293,542 | 344,667 |
| NPU copy out | 7,188 | 7,084 | 7,625 | 9,084 | 23,375 | 27,416 |
| NPU total | 86,338 | 83,625 | 93,000 | 106,959 | 314,292 | 366,083 |
| NPU thread | 76,281 | 74,083 | 83,208 | 87,125 | 300,792 | 313,541 |

One of 10,000 CPU thread measurements was zero at the available clock
resolution. Its CPU wall duration was valid, and the native/host zero count
matched.

Ratio of means:

```text
NPU Run / CPU wall:    20.6858x
NPU total / CPU wall:  26.1375x
```

Here, a ratio above one means the CPU is faster. The paired-frame ratio
distribution confirms that the conclusion is not an artifact of comparing
separate percentile positions:

| Paired ratio | Mean | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| NPU `Run()` / CPU wall | 20.7713× | 20.4255× | 23.3003× | 26.4313× |
| NPU total / CPU wall | 26.2450× | 25.9507× | 29.1020× | 32.6010× |

Alternation also preserved the conclusion:

```text
CPU-first NPU-total/CPU ratio: 27.1138x
NPU-first NPU-total/CPU ratio: 25.2261x
```

Cold time was 5.042 us on CPU, 1.505 ms for NPU `Run()`, and 1.582 ms for
the NPU total. Graph construction and compilation are excluded.

Mean isolated throughput and 10 ms hop consumption:

| Path | Frames/s | Fraction of 10 ms hop |
|---|---:|---:|
| CPU wall | 302,734 | 0.0330% |
| NPU `Run()` | 14,635 | 0.6833% |
| NPU total | 11,582 | 0.8634% |

The complete paired loop consumed 923.272 ms of process CPU time. The host
command duration was 1.130 seconds.

## System context

All four Cortex-A55 CPUs (`CPU part 0xd05`) were online. The sole cpufreq
policy covered CPUs 0–3 and reported:

```text
governor:         schedutil
observed current: 1,500,000 kHz before and after
minimum:          100,000 kHz
maximum:          2,016,000 kHz
SoC temperature:  38.5 C before, 38.8 C after
```

No affinity, priority, governor, frequency, NPU clock, or thermal setting
was changed.

## Interpretation for noise cancellation

The Wave Cortex-A55 is decisively faster for this tiny lookup. Offloading
Erf alone is therefore an acceleration loss even though both paths fit
comfortably inside a 10 ms audio hop.

This does not imply that the Wave NPU is generally slower. The fixed graph
submission and tensor copies dominate a 481-byte operation. A useful affine
or convolutional partition has more arithmetic per submission and may
amortize that overhead. PoC 14 must measure each larger sealed partition
rather than extrapolate from this result.

For the current learned mask, NPU execution remains a functional proof and
a valid hybrid boundary, but the CPU LUT is the correct performance choice
if Erf is the only offloaded operation.

## Limits

- Natural scheduler behavior was measured; the benchmark did not isolate a
  core or pin frequency.
- CPU wall time includes timing calls, making the raw LUT itself faster than
  the reported boundary.
- The NPU numbers include TIM-VX/OpenVX submission behavior for this exact
  NBG and runtime only.
- Cold time is first execution after compile, not process startup or graph
  construction.
- These values cannot be used for Conv, MatMul, recurrent, or arbitrary
  ONNX performance claims.

The machine-readable result is
[`erf-wave-cpu-vs-npu.json`](../performance/erf-wave-cpu-vs-npu.json).
