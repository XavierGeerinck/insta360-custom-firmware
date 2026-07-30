# PoC 13D — Six-Vector NPU Numerical Qualification

## Question

Does the sealed Wave NBG return the correct quantized Erf values for boundary,
structured, random, and real learned inputs?

## Fixed vectors

1. zero point (`128`);
2. minimum (`0`);
3. maximum (`255`);
4. repeated `0..255` ramp;
5. xorshift32 seed `0x57335645`;
6. learned fixture frame 341.

The pack is 2,902 bytes, SHA-256
`3374b928722c7d504e9ae8d8e1e99c1dc7b3542ee08cbede60b2f8bb8f93a232`,
FNV-1a64 `0x8f90178b58ca8afb`.

## Safety envelope

- one context and graph;
- one NBG node;
- one compile;
- six sequential runs;
- fixed input pack and NBG paths;
- 32-byte prefix/suffix output canaries;
- 20-second native watchdog and 30-second host deadline;
- no retry, recovery, output file, arbitrary argument, CLC, or direct ioctl.

## Result

```text
status: PASS
execution attempts: 1
vectors: 6 / 6
compared bytes: 2,886
mismatched bytes: 0
maximum U8 error: 0
learned output unique values: 63
canaries: PASS
health differences: none
kernel alerts: none
```

Latency:

```text
per vector: 1,571, 167, 164, 132, 116, 116 us
warm range: 116..167 us
```

## Evidence

| Artifact | SHA-256 |
|---|---|
| Runner binary | `99d51397b8c6b21c360755b6a66cc62eba3897287fb12a215468bd8f7e2f62d3` |
| Execution manifest | `10565c58589afdf7e9514220863fd03a7d52b20235ea38be669b4f773d5f873c` |
| Run result | `f8ffbca89fb96275e1c404ea780c1ccb62de3deda07820ec7308ccf6aaab7fac` |

## Oracle

The host independently creates a 256-entry table with:

```text
clamp(round(erf((u8 - 128) / 32) * 128) + 128, 0, 255)
```

The table SHA-256 is
`c9525963e022d4a4aea0814845119b37488f3727ec8d96684f52e1e2ad776296`.
The native runner does not contain that output table and cannot declare
numerical success by comparing against its own oracle.

## Interpretation

This proves exact numerical NPU execution for the sealed partition. It does
not yet prove a full enhanced waveform.

## Next gate

Run every partition frame from the frozen audio fixture, pull the ordered
output pack, and reconstruct audio from those NPU bytes.
