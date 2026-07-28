# PoC 3 — Wave NPU Runtime and Driver Trace

PoC 3 passed on 2026-07-28. Two independently captured, bounded traces of
the firmware-matched stock CTC graph produced the same:

- eight probe lifecycle phases;
- loaded runtime-library set and order;
- driver open/probe events;
- 73-call normalized galcore command sequence;
- per-phase command assignment;
- syscall counts;
- 332,800-byte output with SHA-256
  `e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4`;
- graph cleanup status zero;
- responsive post-run ADB and unchanged captured NPU state.

The normalized GAL sequence SHA-256 in both runs is:

```text
33b93229efcf7258253d2124129ac24f107de63b1742d6666db2e674cbb1c226
```

This proves a stable, observable userspace-to-driver transaction for a known
valid Wave graph. It does not unlock compilation, disclose command-buffer
contents, or authorize ioctl replay.

## Evidence

Tracked, pointer-free evidence:

| File | Identity | Purpose |
|---|---|---|
| [`normalized-run-1.json`](normalized-run-1.json) | `b694d88f778ecdc72b06afb24bd4c749824c358d9c5a4bcb9ada67d970b3f8b3` | First successful normalized trace |
| [`normalized-run-2.json`](normalized-run-2.json) | `ee0090152e5492dc6ac3f107a492610801e69500607b92ca8bf089f506f7cf7c` | Second successful normalized trace |
| [`repeatability.json`](repeatability.json) | `207b70cdea1ecb89cbd2ad80e8a3d81c1fde4958d5369d02d378e563f36ea7fc` | Machine comparison |
| [`galcore-interface.md`](galcore-interface.md) | Analysis | Interface and command evidence |
| [`runtime-sequence.md`](runtime-sequence.md) | Analysis | Phase-by-phase interpretation |
| [`repeatability.md`](repeatability.md) | Analysis | Acceptance and variance analysis |

The full experiment record is
[`2026-07-28-poc3-runtime-driver-trace.md`](../lab-notebook/2026-07-28-poc3-runtime-driver-trace.md).
Raw envelopes and streams remain under `artifacts/npu/runs/` and are
gitignored because they contain PIDs, addresses, timestamps, and complete
system-call formatting.

## What was observed

### Exact live interface

Every GAL call used:

```text
ioctl request:          30000 (0x7530 in raw strace)
driver envelope:        32 bytes
HAL interface bytes:    376
core index:             0
successful ioctl:       73 / 73
returned HAL status 0:  73 / 73
```

The 32-byte envelope and first 16-byte interface prefix are supported by the
pinned public eCV5546 GAL `6.4.18.5` source. The live Wave library is
`6.4.19.5.946407`, so field names remain version-adjacent evidence; coherent
live values across 146 completed call observations strongly corroborate the
prefix on this unit. Details and evidence classes are in
[`galcore-interface.md`](galcore-interface.md).

### Driver lifecycle

The explicit probe check in phase 1 opened and closed `/dev/galcore`.
During graph creation in phase 5, the runtime:

1. opened `/dev/galcore`;
2. probed `/dev/galcore1`, which returned `ENOENT`;
3. initialized the generic and VIP contexts;
4. allocated and locked graph memory;
5. kept the primary driver descriptor until process exit.

No explicit `/dev/galcore` close appears after runtime initialization. The
probe reports graph cleanup zero, and normal process exit closes the
descriptor. This describes the observed probe lifetime; it is not a
recommendation for a long-lived production wrapper.

### Graph lifecycle

| Probe phase | GAL calls | Observed boundary |
|---|---:|---|
| 1 — driver reachability | 0 | Explicit open/close only |
| 2 — read NPU status | 0 | Read-only sysfs |
| 3 — read/validate model | 0 | Stock NBG file read |
| 4 — query tensor metadata | 0 | Parser/runtime metadata path |
| 5 — create graph | 46 | Version/chip discovery, VIP attach, signals, base address, 11 allocations and 11 locks |
| 6 — submit zero input | 0 | No driver transaction visible at this boundary |
| 7 — execute three times | 9 | Exactly three `COMMIT → EVENT_COMMIT → USER_SIGNAL` groups |
| 8 — destroy/teardown | 18 | Seven unlock/release pairs, one final flush group, one final unlock |

The absence of an ioctl in phase 6 indicates that the tested
`aml_module_input_set` path updates already prepared userspace/runtime
storage. It does not prove every tensor-binding mode is driver-free.

### Dynamic runtime stack

The traced process successfully opened these relevant shared objects, in
first-observed order:

```text
libnnsdk.so
librt.so.1
libovxlib.so
libOpenVX.so
libVSC_Lite.so
libm.so.6
libVSC.so
libGAL.so
libArchModelSw.so
libNNArchPerf.so
libstdc++.so.6
```

The dynamic loader also opened the normal glibc support libraries
`libdl.so.2`, `libgcc_s.so.1`, `libpthread.so.0`, and `libc.so.6`.

Loading `libVSC.so`, `libVSC_Lite.so`, or `libArchModelSw.so` is not proof
that a stock NBG is compiled on-device. PoC 3 proves only that those DSOs are
mapped during initialization. PoC 5 must find and exercise a legitimate
Wave-targeted graph-production route separately.

## Repeatability result

| Invariant | Run 1 | Run 2 |
|---|---:|---:|
| Harness classification | success | success |
| Remote exit | 0 | 0 |
| Probe mean, descriptive | 3.562 ms | 3.474 ms |
| Output bytes | 332,800 | 332,800 |
| Output digest | `e621b213…52d4` | `e621b213…52d4` |
| Cleanup | 0 | 0 |
| GAL calls | 73 | 73 |
| Complete metadata/status pairs | 73 | 73 |
| Normalized sequence digest | `33b93229…1c226` | `33b93229…1c226` |
| Syscall counts identical | yes | yes |
| Loaded path observations identical | yes | yes |
| Driver events identical | yes | yes |
| Required hashes unchanged | yes | yes |
| NPU parameters unchanged | yes | yes |

The raw stdout hashes differ because PIDs, addresses, timestamps, and
durations are intentionally retained only in raw evidence. The normalizer
removes those values before comparison.

## Instrumentation

Source:

```text
waved/tools/npu-trace/gal_trace.c
waved/tools/npu-trace/build.sh
waved/tools/npu-trace/stage.sh
waved/tools/npu-trace/stock-ctc-trace.sh
waved/scripts/npu/trace_normalize.py
```

Important artifact hashes:

| Artifact | SHA-256 |
|---|---|
| AArch64 interposer | `dccaedd13812c161949905efecf7aa19fe74712b75de8d80211924e286e68601` |
| Fixed device wrapper | `7799626fcad221088a109740022e93ca5b7652d7589c9e79347ec92a1b1ac866` |
| Live `strace` 5.9 | `2046787af2bec0edd5c3483b59bcbd870e5a0fb68cc3c618e9e464e61ee62aa2` |
| Live `libGAL.so` | `13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6` |
| Stock probe | `fe8da5702032c397023acebd1a4bdf40ce086f6e86277a52c4f059f409552db0` |
| Stock CTC NBG | `5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66` |

The final experiment definition SHA-256 is:

```text
dcc6efe65182a9a8ded2f1c1f96fbf403098a51d414f2f48da4f9f029ac19992
```

## Reproduce

Run from the repository root with the authorized Wave connected:

```bash
waved/tools/npu-trace/build.sh
waved/tools/npu-trace/stage.sh

python3 waved/scripts/npu/npu_lab.py snapshot \
  --experiment stock-ctc-trace \
  --output artifacts/npu/poc3-snapshot.json

python3 waved/scripts/npu/npu_lab.py run stock-ctc-trace \
  --artifact-root artifacts/npu/runs \
  --run-id poc3-local-trace-1

python3 waved/scripts/npu/trace_normalize.py normalize \
  --run-directory artifacts/npu/runs/poc3-local-trace-1 \
  --output artifacts/npu/poc3-local-normalized-1.json
```

Use new paths and run IDs; the tools refuse to overwrite existing results.
Repeat once, then compare:

```bash
python3 waved/scripts/npu/trace_normalize.py compare \
  --left artifacts/npu/poc3-local-normalized-1.json \
  --right artifacts/npu/poc3-local-normalized-2.json
```

The Wave's `/data` filesystem was full during this PoC. The two disposable
trace artifacts are therefore staged under the executable tmpfs directory
`/tmp/wave-npu-lab/`. The harness allows exactly
`/tmp/wave-npu-lab/stock-ctc-trace`, not arbitrary `/tmp` executables.
Reboot clears the staged files. `stage.sh` verifies local hashes, refuses to
overwrite an unexpected remote file, and reads both hashes back.

## Safety and limits

The interposer:

- forwards the original ioctl once through the kernel syscall;
- records no pointer value;
- reads only the 32-byte envelope and 16-byte interface prefix;
- rejects interface sizes outside 16–65,536 bytes;
- never reads the command-specific union;
- records no model, coefficient, tensor, audio, or command-buffer payload;
- performs no mutation, replay, retry, or recovery.

PoC 3 does not prove:

- the meaning or layout of any command-specific union member;
- the NPU instruction set;
- how NBG command, coefficient, patch, or load-state sections map to hardware
  packets;
- that VSC can compile a custom graph for product `0x1000000e`;
- that a noise-suppression topology is supported;
- that this ASR graph's descriptive latency predicts audio-model latency.

Those are later gates. Raw ioctl replay and unverified graph execution remain
prohibited.
