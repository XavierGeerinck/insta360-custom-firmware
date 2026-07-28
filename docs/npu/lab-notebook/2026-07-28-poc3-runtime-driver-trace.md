# 2026-07-28 — PoC 3: Runtime and Driver Trace

## Record

| Field | Value |
|---|---|
| PoC | `3` |
| Experiment ID | `stock-ctc-trace` |
| Operator | Codex research session |
| Device alias | `wave-lab-unit-a` |
| Pilot UTC | `2026-07-28T11:17:34.637Z` |
| Accepted run UTCs | `2026-07-28T11:22:35.395Z`, `2026-07-28T11:24:34.530Z` |
| Recorded run git identity | `825826c276563f6aa3ef042256fb102e9ceea3c8+dirty` |
| Final tracer implementation | commit `3fcbb28` |
| Worktree during runs | Dirty only with the scoped PoC 3 implementation and definition |
| Final definition SHA-256 | `dcc6efe65182a9a8ded2f1c1f96fbf403098a51d414f2f48da4f9f029ac19992` |
| Raw run IDs | `poc3-trace-framed-1-20260728`, `poc3-trace-framed-2-20260728` |
| Negative pilot | `poc3-trace-1-20260728` |
| Outcome | Pass |

The raw envelopes preserve the dirty git marker. Reproducibility does not
depend on an unrecorded build: every executed/staged file and every runtime
dependency was hash-pinned, and the final implementation was committed
immediately after the accepted traces.

## Falsifiable hypothesis

> Given exact stock runtime, model, probe, tracer, and wrapper hashes, a
> metadata-only trace observes the complete stock CTC graph lifecycle,
> produces the established output digest and cleanup status, leaves captured
> device state unchanged, and yields the same normalized driver-command
> sequence in two independent runs.

The hypothesis would fail if:

- the tracer changed the output or cleanup;
- any ioctl/envelope/status was invalid;
- a run timed out;
- required hashes or NPU parameters changed;
- the lifecycle phases were incomplete;
- the normalized command sequence differed;
- the second run needed reset/recovery;
- useful correlation required capturing pointer or payload data.

## Why this PoC exists

PoC 0 proved direct inference. PoC 1 recovered the NN SDK and OVXLIB ABI
statically. PoC 2 mapped the NBG container and corpus.

The remaining uncertainty below a valid graph was dynamic:

```text
NN SDK lifecycle
    ↓
loaded userspace stack
    ↓
GAL command families
    ↓
/dev/galcore submission and synchronization
```

A safe custom-graph gate needs a known-good transaction reference. Without
it, a future custom graph could return success at one layer while taking a
different backend or failing to release resources.

## Safety gate

### Allowed actions

- Host compilation of a small metadata-only AArch64 interposer.
- Read-only ADB capability and filesystem-space checks.
- Creation of one dedicated ephemeral directory.
- Staging two exact, hash-pinned files in that directory.
- Execution of the unmodified, already-proven stock CTC graph.
- Fixed `strace` observation of an allowlisted syscall set.
- Reading only the 32-byte GAL envelope and 16-byte interface prefix.
- Two bounded accepted runs after a successful preflight.
- Host-only normalization and comparison.

### Explicitly excluded

- New, foreign, mutated, or product-ID-patched graph execution.
- Raw ioctl replay or mutation.
- Blind `/dev/galcore` fuzzing.
- Command-specific union or command-buffer capture.
- Model, coefficient, tensor, output, or audio payload capture.
- System-library, stock-probe, or stock-model replacement.
- Service stop.
- Sysfs/debugfs write.
- Kernel-module operation.
- Driver reset.
- Reboot.
- Boot configuration change.

### Timeout and recovery

```text
probe internal watchdog: 15 seconds
harness timeout:          30 seconds
automatic remote kill:    none
automatic driver reset:   none
automatic reboot:         none
```

A host timeout would terminate only the local ADB client and trigger manual
holder/state review. No timeout occurred.

## Identity and prerequisites

### Device

```text
kernel:
  Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT
  Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux

driver:
  /dev/galcore, character device 199:0
  live GAL version 6.4.19.5.946407

preflight parameters:
  nanoqFreq = 838860800
  enableNN  = 255
  recovery  = 1
```

No `/dev/galcore` holder was reported in the final preflight snapshot.

### Required live artifacts

| Path | Bytes | SHA-256 | Role |
|---|---:|---|---|
| `/bin/sh` | 881,880 | `6350ce539100969ca9f54b7098d70699e20c8be422958f2f3b23d78f7e97b6f9` | Wrapper interpreter |
| `/data/waved/waved-npu-probe.poc` | 721,696 | `fe8da5702032c397023acebd1a4bdf40ce086f6e86277a52c4f059f409552db0` | Stock control probe |
| `/lib/libc.so.6` | 1,430,328 | `cfc6aa042a880a87e4f0adbfb74b55c9ba9607561905b20f58da96fcb2d3bb86` | libc |
| `/lib/libgcc_s.so.1` | 84,144 | `859408230bf4baf943d0c7985870488683b6e61c0cb5725ed8934d8c45ea0ab1` | Interposer runtime |
| `/lib64/ld-2.31.so` | 167,592 | `912002c930f3096d98fc32e0897466049d6e5182cbb606b456b2ebcc99551366` | Dynamic loader |
| `/model/asr_de/de_online/ctc_online.nb` | 2,810,984 | `5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66` | Known-good graph |
| `/tmp/wave-npu-lab/libwave-gal-trace.so` | 67,560 | `dccaedd13812c161949905efecf7aa19fe74712b75de8d80211924e286e68601` | Metadata interposer |
| `/tmp/wave-npu-lab/stock-ctc-trace` | 1,905 | `7799626fcad221088a109740022e93ca5b7652d7589c9e79347ec92a1b1ac866` | Fixed wrapper |
| `/usr/bin/sha256sum` | 881,880 | `6350ce539100969ca9f54b7098d70699e20c8be422958f2f3b23d78f7e97b6f9` | Wrapper self-check |
| `/usr/bin/strace` | 855,264 | `2046787af2bec0edd5c3483b59bcbd870e5a0fb68cc3c618e9e464e61ee62aa2` | Syscall observer |
| `/usr/lib/libGAL.so` | 1,923,672 | `13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6` | Userspace driver HAL |
| `/usr/lib/libnnsdk.so` | 288,448 | `36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe` | NN SDK |
| `/usr/lib/libovxlib.so` | 5,838,160 | `09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9` | NBG/OpenVX runtime |

### Tracer source identity

| Source | SHA-256 |
|---|---|
| `gal_trace.c` | `3bc0cb755124edca5b36ebb6e71891e8c8e6b5270ccd31ac01de219e473cae43` |
| `build.sh` | `d5aec7ed62711d79402eae96b9be5555017451fec6df12b2a8a0a4d92a7d6532` |
| `stage.sh` | `026f372104abea02d75cada055c518e375c61a5f46e8dfb64212b9657ea46a04` |
| `stock-ctc-trace.sh` | `7799626fcad221088a109740022e93ca5b7652d7589c9e79347ec92a1b1ac866` |

The normalizer continued to gain tracked reporting detail after the runs;
the raw logs are the immutable source for each regeneration.

### Adjacent public driver identity

```text
repository: https://github.com/eYs3D/eCV5546_linux
commit:     946d2c841fd82b916bdd2168033906bfec89db61
GAL tree:   v6.4.18.5
```

The source supplies the neutral envelope, prefix, request, and enum
hypotheses. The live device supplies the values and behavioral correlation.

## Device-space deviation

The planned persistent lab directory was `/data/waved/npu-lab/`.

Observed before staging:

```text
/dev/data: 450360 KiB total, 440388 KiB used, 0 KiB available, 100%
mkdir /data/waved/npu-lab: No space left on device
```

Both initial pushes failed because the directory did not exist; no partial
target file was created. Existing `/data` content was inventoried but not
deleted or moved.

The reviewed fallback was:

```text
/tmp/wave-npu-lab/
```

`/tmp` is a 1.9 GiB writable tmpfs without `noexec`. The harness was changed
to accept the exact wrapper path only. The staging script refuses an
unexpected existing target and verifies read-back hashes.

## Build validation

The interposer was compiled in the repository's pinned cross image:

```text
ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main-centos
sha256:59a28167cffea49e71e1951bcaf9105d6a31dcf130c95197e2faff8fbf032508
```

Validation:

```text
ELF:        64-bit LSB shared object, ARM aarch64
stripped:   yes
GLIBC floor: 2.17
needed:     libc.so.6, libgcc_s.so.1
export:     ioctl
build 1:    dccaedd1…8601
build 2:    dccaedd1…8601
byte cmp:   equal
```

The full NPU host test suite passed before execution. At final
implementation commit it contained 87 tests.

## Experiment definition

```text
experiment ID:       stock-ctc-trace
definition SHA-256:  dcc6efe65182a9a8ded2f1c1f96fbf403098a51d414f2f48da4f9f029ac19992
remote argv:         /tmp/wave-npu-lab/stock-ctc-trace
host timeout:        30000 ms
expected remote exit: 0
expected iterations: 3
expected output:     332800 bytes
expected SHA-256:    e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
expected cleanup:    0
```

The wrapper hard-codes `LD_LIBRARY_PATH`, `VIVANTE_SDK_DIR`, `LD_PRELOAD`,
the `strace` options, syscall set, probe path, and stock mode. It accepts no
arguments.

## Method

```bash
waved/tools/npu-trace/build.sh
waved/tools/npu-trace/stage.sh

python3 waved/scripts/npu/npu_lab.py snapshot \
  --experiment stock-ctc-trace \
  --output artifacts/npu/poc3-trace-framed-preflight-20260728.json

python3 waved/scripts/npu/npu_lab.py run stock-ctc-trace \
  --artifact-root artifacts/npu/runs \
  --run-id poc3-trace-framed-1-20260728

python3 waved/scripts/npu/trace_normalize.py normalize \
  --run-directory artifacts/npu/runs/poc3-trace-framed-1-20260728 \
  --output docs/npu/trace/normalized-run-1.json

python3 waved/scripts/npu/npu_lab.py run stock-ctc-trace \
  --artifact-root artifacts/npu/runs \
  --run-id poc3-trace-framed-2-20260728

python3 waved/scripts/npu/trace_normalize.py normalize \
  --run-directory artifacts/npu/runs/poc3-trace-framed-2-20260728 \
  --output docs/npu/trace/normalized-run-2.json

python3 waved/scripts/npu/trace_normalize.py compare \
  --left docs/npu/trace/normalized-run-1.json \
  --right docs/npu/trace/normalized-run-2.json \
  --output docs/npu/trace/repeatability.json
```

## Final preflight

| Check | Expected | Observed | Result |
|---|---|---|---|
| ADB | `device` | `device` | pass |
| Kernel | captured identity | exact build above | pass |
| `/dev/galcore` | present | `199:0` | pass |
| Required artifacts | 13 exact hashes | 13/13 match | pass |
| NPU parameters | all observed | `838860800`, `255`, `1` | pass |
| Holders | recorded | none | pass |
| Snapshot errors | none | `[]` | pass |

## Negative pilot

### Observation

Run:

```text
poc3-trace-1-20260728
```

Result:

```text
remote exit:        0
classification:     result-parse-failure
reason:             expected exactly one RESULT line, found 0
post-state health:  all true
raw stdout:         85305 bytes
raw stdout SHA:     3212a12320483801555def238ff67b3cdcdc2d45675d1244655ef7bec2c5f640
```

The raw stream contained the expected PASS result, digest, cleanup, 73
successful ioctls, and all lifecycle markers. However, the device ADB shell
placed stdout and stderr on the same PTY. `strace` printed the traced
`write(1, ...)` prefix, then the actual write bytes appeared inside that
line. The result therefore did not begin a line, and the strict parser
correctly refused it.

### Correction

No result parser was relaxed. The wrapper was changed to:

- send strace output to `/tmp/wave-npu-lab/strace.raw`;
- send probe/GAL stderr to `/tmp/wave-npu-lab/gal.raw`;
- keep probe stdout clean;
- append exact GAL/strace frames after native completion;
- delete both raw tmpfs files;
- return the original probe status.

The wrapper and experiment definition received new hashes. A fresh snapshot
passed before accepted run 1.

## Accepted observations

### Run A

```text
run ID:           poc3-trace-framed-1-20260728
classification:   success
remote exit:      0
host duration:    347 ms
native mean:      3.562 ms (descriptive)
iterations:       3
output bytes:     332800
output SHA-256:   e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
cleanup:          0
raw stdout bytes: 83657
raw stdout SHA:   72ed5125ae84a142092e7379575a8473b1f551db68370babdcf0954bedc0db79
raw stderr SHA:   e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

### Run B

```text
run ID:           poc3-trace-framed-2-20260728
classification:   success
remote exit:      0
host duration:    337 ms
native mean:      3.474 ms (descriptive)
iterations:       3
output bytes:     332800
output SHA-256:   e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
cleanup:          0
raw stdout bytes: 83657
raw stdout SHA:   03d8f835cd5f0e10a8aeff2bdcfbc859255f0d5ad6c82213c469e3c4dccffeac
raw stderr SHA:   e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

### Normalized result

Both runs:

```text
lifecycle phases:       1,2,3,4,5,6,7,8
driver events:          identical
loaded path set/order:  identical
GAL calls:              73
HAL statuses captured:  73
syscall result zero:    73
interface bytes:        376
core:                   0
sequence SHA-256:       33b93229efcf7258253d2124129ac24f107de63b1742d6666db2e674cbb1c226
syscall counts:         identical
comparison:             equivalent
```

Per-phase GAL calls:

| Phase | Count | Summary |
|---:|---:|---|
| 1 | 0 | Driver open/close reachability |
| 2 | 0 | Read-only parameters |
| 3 | 0 | Model read/hash |
| 4 | 0 | Metadata parse |
| 5 | 46 | Initialization, attach, signals, 11 allocation/lock pairs |
| 6 | 0 | Input submission |
| 7 | 9 | Three commit/event/signal groups |
| 8 | 18 | Unlock/release, final event flush, final unlock |

Total command counts:

```text
CHIP_INFO                      1
VERSION                        1
QUERY_CHIP_IDENTITY            2
QUERY_CHIP_OPTION              3
QUERY_VIDEO_MEMORY             1
ALLOCATE_LINEAR_VIDEO_MEMORY  11
RELEASE_VIDEO_MEMORY           7
LOCK_VIDEO_MEMORY             11
UNLOCK_VIDEO_MEMORY            8
ATTACH                         2
EVENT_COMMIT                   4
COMMIT                         4
USER_SIGNAL                   16
GET_BASE_ADDRESS               2
```

## Timing

Timing is descriptive, not a benchmark.

The two means, 3.562 and 3.474 ms, include tracer overhead around a stock ASR
component with deterministic zero input. There was no warm-up policy,
thermal control, scheduler isolation, long-duration run, or audio deadline
test. PoC 6/7 own those measurements.

## Evidence classification

### Observed live

- The exact stock CTC graph still produces the established digest with the
  tracer loaded.
- Runtime graph creation opens `/dev/galcore` and probes missing
  `/dev/galcore1`.
- Both runs contain 73 request-30000 ioctls.
- Every interface size is 376 bytes.
- Every ioctl result and every captured HAL status is zero.
- Hardware type 4 owns all NPU lifecycle calls.
- Graph creation has 46 calls.
- Input submission has no ioctl.
- Each execute iteration has one commit/event/signal group.
- Graph destroy/teardown has the recorded 18-call sequence.
- Both normalized sequences and selected syscall counts are identical.
- Post-run device state and hashes remain unchanged.

### Static verified

- Exact live `libGAL.so` imports `ioctl`.
- The interposer exports `ioctl` and requires only GLIBC 2.17 plus
  `libgcc_s`.
- The adjacent public driver defines request 30000, a 32-byte envelope, the
  four-field interface prefix, and the observed enum labels.
- The normalizer contains no device-execution function.

### Inferred

- Phase-6 input submission most likely updates preallocated/bound memory.
- The two initialization groups likely correspond to distinct runtime/GAL
  objects.
- The `USER_SIGNAL` after each event commit is the visible wait/synchronizing
  family.
- Some allocations are context- or event-owned rather than directly
  released by the seven visible release calls.

### Unknown

- Command-specific union layouts on exact GAL `6.4.19.5`.
- Exact `USER_SIGNAL` subcommands.
- Handles, physical addresses, page tables, and command packets.
- NPU ISA and microarchitecture.
- Whether `libVSC*` can compile a valid product-`0x1000000e` graph.
- Custom denoiser operation coverage.
- Long-lived context reuse and explicit final close/detach behavior.

### Negative findings

- `/data` could not accept the planned lab directory.
- `/dev/galcore1` is absent.
- The ADB PTY does not preserve remote stdout/stderr separation.
- No fd-annotated `/dev/galcore` `mmap` was observed.
- No HAL ioctl occurs during this graph's phase-6 input submission.
- No explicit runtime `/dev/galcore` close or `DETACH` appears before process
  exit.
- Merely loading compiler-named DSOs does not establish a custom compiler.

## Safety postconditions

| Invariant | Before | After run A | After run B | Pass |
|---|---|---|---|---|
| ADB responsive | yes | yes | yes | yes |
| `nanoqFreq` | `838860800` | unchanged | unchanged | yes |
| `enableNN` | `255` | unchanged | unchanged | yes |
| `recovery` | `1` | unchanged | unchanged | yes |
| `/dev/galcore` | present | present | present | yes |
| Required hashes | 13 match | unchanged | unchanged | yes |
| Graph cleanup | n/a | 0 | 0 | yes |
| Wrapper raw tmpfs files | absent | absent | absent | yes |

Changes made to service, firmware, model, stock library, sysfs, module, boot
configuration, or driver recovery:

```text
none
```

Disposable files intentionally left until reboot or manual reviewed cleanup:

```text
/tmp/wave-npu-lab/libwave-gal-trace.so
/tmp/wave-npu-lab/stock-ctc-trace
```

## Raw and tracked evidence

Raw, gitignored:

```text
artifacts/npu/runs/poc3-trace-1-20260728/
artifacts/npu/runs/poc3-trace-framed-1-20260728/
artifacts/npu/runs/poc3-trace-framed-2-20260728/
artifacts/npu/poc3-trace-framed-preflight-20260728.json
```

Tracked:

```text
docs/npu/trace/normalized-run-1.json
docs/npu/trace/normalized-run-2.json
docs/npu/trace/repeatability.json
docs/npu/trace/README.md
docs/npu/trace/galcore-interface.md
docs/npu/trace/runtime-sequence.md
docs/npu/trace/repeatability.md
```

Tracked JSON hashes:

```text
b694d88f778ecdc72b06afb24bd4c749824c358d9c5a4bcb9ada67d970b3f8b3  normalized-run-1.json
ee0090152e5492dc6ac3f107a492610801e69500607b92ca8bf089f506f7cf7c  normalized-run-2.json
207b70cdea1ecb89cbd2ad80e8a3d81c1fde4958d5369d02d378e563f36ea7fc  repeatability.json
```

## Conclusion

The hypothesis passed.

The Wave stock NBG path has a stable software-visible driver transaction:

- creation initializes and allocates through the VIP HAL;
- input submission reuses prepared state;
- each inference produces a commit/event/signal triple;
- graph teardown unlocks/releases memory and flushes an event group;
- the observer does not change output, cleanup, or captured device state.

This is the reference signature for later graph and operation tests. It is
not a direct-driver programming API.

## Gate decision

```text
Current PoC gate: pass

Next allowed work:
  - host/static recovery of a legitimate product-0x1000000e compiler route;
  - reconstruct and validate the private export-data representation;
  - design a separately reviewed tiny add/identity graph experiment;
  - compare future legitimate graph execution with this normalized control.

Still prohibited:
  - raw ioctl replay;
  - command-buffer mutation or fuzzing;
  - product-ID patching;
  - unverified custom graph execution;
  - stock-file replacement;
  - automatic driver reset or reboot.

New plan required:
  PoC 5 Wave-targeted custom-graph gate.
```
