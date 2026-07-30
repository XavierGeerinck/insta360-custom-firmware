# PoC 13F — 10,000-Frame Erf NPU Soak

## Question

Can one sealed graph execute 10,000 consecutive diverse frames without
numerical drift, buffer corruption, timeout, or device-health change?

## Input and oracle

The native runner generates one continuous xorshift32 byte stream from seed
`0x5733130f`. Each of 10,000 frames contains 481 bytes. It reports cumulative
input and output FNV-1a64 values every 1,000 frames.

The host independently generates the same inputs and maps them through its
`math.erf` U8 table. The native runner contains no expected output table.

Final expected digests:

```text
input:  0x1417bf4ceca8d26e
output: 0xcbd4ae7bafd1fccd
```

## Safety envelope

- one context, graph, NBG node, and compile;
- exactly 10,000 sequential `Run` calls;
- fixed seed, shape, quantization, NBG, paths, and argument;
- ten bounded checkpoint lines;
- 32-byte output canaries on every frame;
- 30-second native watchdog and 40-second host deadline;
- no output file, retry, recovery, CLC, arbitrary input, or direct ioctl.

## Result

```text
status: PASS
execution attempts: 1
frames: 10,000 / 10,000
oracle-covered bytes: 4,810,000
checkpoint matches: 10 / 10
mean graph run: 84.8758 us
minimum graph run: 74 us
maximum graph run: 1,649 us
10 ms budget fraction: 0.00848758
canaries: PASS
health differences: none
kernel alerts: none
```

## Evidence

| Artifact | SHA-256 |
|---|---|
| Runner binary | `ef0d4ece1d44141b1254581aa5b05e5fd371f99f819cfac32b478cbcaaaf4cff` |
| Soak manifest | `ed64868e2c28b057066513c4b1f390d3686f2fd16c47f47fa9a3b469bb6acd74` |
| Run result | `e4f3fc3d0de3c833dadf912563585c3737d7dc30c7338f2e26d9936077ee6f8d` |

## Regression

After the soak:

```text
wave-onnx-hybrid: 187 tests passed
NPU harnesses:    338 tests and 302 subtests passed
Ruff:             passed
shell syntax:     passed
```

## Interpretation

The exact partition has strong sequential stability and a large graph-run
timing margin. This is not the originally planned full production Rust soak:
it does not measure p50/p95/p99, RSS growth, CPU load, allocations, reset
reproducibility, thermal behavior, or end-to-end audio-service scheduling.

## Next gate

Port the exact model and latched fallback behind production Rust `NpuInfer`,
then collect the omitted service-level statistics before enabling it outside
the lab.
