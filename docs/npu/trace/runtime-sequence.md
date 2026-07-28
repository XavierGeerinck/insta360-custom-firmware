# Stock CTC Runtime Sequence

This is the normalized dynamic sequence for
`/data/waved/waved-npu-probe.poc --run-stock-ctc`.

It combines:

- explicit `[npu:N]` markers emitted by the probe;
- loaded paths and syscalls observed by `strace 5.9`;
- metadata-only request records from the `ioctl` interposer;
- PoC 1's static NN SDK call graph;
- PoC 2's parsed NBG metadata.

No pointer, timestamp, file descriptor, tensor content, model byte, or
command-union payload appears here.

## End-to-end view

```text
waved-npu-probe
  │
  ├─ validate stock CTC NBG
  │    v2, target 0x1000000e
  │    input  FP16 [256, 16, 1]
  │    output FP32 [5200, 16, 1]
  │
  ├─ libnnsdk.so
  │    aml_module_create / input_set / output_get / destroy
  │
  ├─ libovxlib.so + libOpenVX.so
  │    NBG graph setup and execution
  │
  ├─ libGAL.so
  │    73 × ioctl(request 30000, 376-byte interface)
  │
  ├─ /dev/galcore
  │    allocation, lock, commit, event, signal, unlock, release
  │
  └─ VIP core 0
       three deterministic executions
```

## Phase map

### Phase 1 — reachability

Probe marker:

```text
[npu:1] open /dev/galcore O_RDWR
```

Observed:

```text
openat("/dev/galcore", O_RDWR|O_CLOEXEC) = success
close("/dev/galcore") = success
```

This proves only that the process can reach the driver. No HAL ioctl occurs
because the Rust `OpenOptions` handle is immediately dropped.

### Phase 2 — read-only status

Probe marker:

```text
[npu:2] read NPU status (read-only)
```

Observed reads:

```text
/sys/module/galcore/parameters/nanoqFreq = 838860800
/sys/module/galcore/parameters/enableNN  = 255
```

No HAL ioctl occurs.

### Phase 3 — model validation

Probe marker:

```text
[npu:3] read and validate /model/asr_de/de_online/ctc_online.nb
```

The exact 2,810,984-byte model is opened and hashed. PoC 2 parses it as:

```text
NBG version: 0x00020000
product:     0x1000000e
input:       FP16 [256, 16, 1]
output:      FP32 [5200, 16, 1]
```

No HAL ioctl occurs.

### Phase 4 — NN SDK and metadata

Probe marker:

```text
[npu:4] load libnnsdk.so and query NBG tensor metadata
```

The dynamic path reaches `libnnsdk.so`; tensor metadata agrees with PoC 2.
No HAL ioctl occurs. This supports PoC 1's static finding that the metadata
helpers parse graph metadata without creating a live device graph.

### Phase 5 — graph creation

Probe marker:

```text
[npu:5] create graph from firmware NBG memory
```

Driver events:

```text
open /dev/galcore  → success
open /dev/galcore1 → ENOENT
```

The exact 46-command sequence is:

```text
01 VERSION
02 CHIP_INFO
03 QUERY_VIDEO_MEMORY                 hardware 3D
04 QUERY_CHIP_IDENTITY                hardware VIP
05 QUERY_CHIP_OPTION                  hardware VIP
06 ATTACH                             hardware VIP
07 USER_SIGNAL                        hardware VIP
08 GET_BASE_ADDRESS                   hardware VIP
09 USER_SIGNAL
10 USER_SIGNAL
11 ALLOCATE_LINEAR_VIDEO_MEMORY
12 LOCK_VIDEO_MEMORY
13 USER_SIGNAL
14 USER_SIGNAL
15 ALLOCATE_LINEAR_VIDEO_MEMORY
16 LOCK_VIDEO_MEMORY
17 USER_SIGNAL

18 QUERY_CHIP_IDENTITY
19 QUERY_CHIP_OPTION
20 ATTACH
21 USER_SIGNAL
22 GET_BASE_ADDRESS
23 USER_SIGNAL
24 USER_SIGNAL
25 ALLOCATE_LINEAR_VIDEO_MEMORY
26 LOCK_VIDEO_MEMORY
27 USER_SIGNAL
28 USER_SIGNAL
29 ALLOCATE_LINEAR_VIDEO_MEMORY
30 LOCK_VIDEO_MEMORY
31 USER_SIGNAL

32 QUERY_CHIP_OPTION
33 ALLOCATE_LINEAR_VIDEO_MEMORY
34 LOCK_VIDEO_MEMORY
35 ALLOCATE_LINEAR_VIDEO_MEMORY
36 LOCK_VIDEO_MEMORY
37 ALLOCATE_LINEAR_VIDEO_MEMORY
38 LOCK_VIDEO_MEMORY
39 ALLOCATE_LINEAR_VIDEO_MEMORY
40 LOCK_VIDEO_MEMORY
41 ALLOCATE_LINEAR_VIDEO_MEMORY
42 LOCK_VIDEO_MEMORY
43 ALLOCATE_LINEAR_VIDEO_MEMORY
44 LOCK_VIDEO_MEMORY
45 ALLOCATE_LINEAR_VIDEO_MEMORY
46 LOCK_VIDEO_MEMORY
```

The two similar initialization groups may represent separate OpenVX/GAL
objects or devices within the userspace stack. The trace proves the repeated
shape, not the owning object identity, because handles and union payloads are
not captured.

By the end of phase 5:

- the generic GAL version and chip queries passed;
- VIP hardware type 4, core 0 is selected;
- two VIP attach/base-address initialization groups passed;
- 11 linear video-memory allocations are locked;
- the graph is ready without a commit yet.

### Phase 6 — input submission

Probe marker:

```text
[npu:6] submit deterministic zero-filled inputs
```

Observed HAL calls:

```text
none
```

For this graph and `INPUT_TYPE_BINARY_RAW`, `aml_module_input_set` does not
create a separately visible driver transaction. The most conservative
interpretation is that it copies or binds input into storage prepared in
phase 5. Other input modes remain uncharacterized.

This matters for streaming audio: a production wrapper may be able to update
an already-bound frame buffer without a graph-recreation ioctl sequence, but
that must be verified with a custom graph and real repeated frames.

### Phase 7 — three executions

Probe marker:

```text
[npu:7] execute and hash 3 deterministic iterations
```

Each iteration emits the identical three-call group:

```text
COMMIT → EVENT_COMMIT → USER_SIGNAL
```

Expanded:

```text
47 COMMIT
48 EVENT_COMMIT
49 USER_SIGNAL

50 COMMIT
51 EVENT_COMMIT
52 USER_SIGNAL

53 COMMIT
54 EVENT_COMMIT
55 USER_SIGNAL
```

All are hardware type VIP, core 0, syscall result zero, HAL status zero.
Each iteration returns one 332,800-byte output with the same digest.

This is the runtime signature a future verified tiny custom graph should
match at the command-family level. It is not a packet template and must not
be replayed.

### Phase 8 — graph destroy and process teardown

Probe marker:

```text
[npu:8] destroy graph and release runtime resources
```

Observed:

```text
56 UNLOCK_VIDEO_MEMORY
57 RELEASE_VIDEO_MEMORY
58 UNLOCK_VIDEO_MEMORY
59 RELEASE_VIDEO_MEMORY
60 UNLOCK_VIDEO_MEMORY
61 RELEASE_VIDEO_MEMORY
62 UNLOCK_VIDEO_MEMORY
63 RELEASE_VIDEO_MEMORY
64 UNLOCK_VIDEO_MEMORY
65 RELEASE_VIDEO_MEMORY
66 UNLOCK_VIDEO_MEMORY
67 RELEASE_VIDEO_MEMORY
68 UNLOCK_VIDEO_MEMORY
69 RELEASE_VIDEO_MEMORY

70 COMMIT
71 EVENT_COMMIT
72 USER_SIGNAL
73 UNLOCK_VIDEO_MEMORY
```

The probe then reports:

```text
aml_module_destroy status=0
RESULT: PASS
exit_group(0)
```

There is no observed `DETACH` command or explicit close of the runtime's
`/dev/galcore` descriptor before process exit. In this disposable probe,
process teardown owns the final descriptor/context cleanup. A long-lived
`waved` integration must test context reuse and explicit shutdown rather than
assuming process exit.

## Loaded library path

Relevant first-successful opens:

```text
probe
├─ libnnsdk.so
│  ├─ librt.so.1
│  └─ libovxlib.so
│     ├─ libOpenVX.so
│     ├─ libVSC_Lite.so
│     ├─ libm.so.6
│     ├─ libVSC.so
│     ├─ libGAL.so
│     ├─ libArchModelSw.so
│     ├─ libNNArchPerf.so
│     └─ libstdc++.so.6
└─ normal glibc support DSOs
```

This is a load-order observation, not a complete ownership tree. A DSO may be
opened because another DSO declares it, because code calls `dlopen`, or
because the loader resolves a transitive dependency.

The coexistence of `libVSC*`, `libArchModelSw`, and a precompiled NBG is a
useful PoC 5 lead. It is not enough to claim that a compiler API or correct
product profile is available.

## System-call envelope

Both acceptance runs had identical selected syscall counts:

| Syscall | Count | Role |
|---|---:|---|
| `execve` | 1 | Start stock probe |
| `openat` | 66 | Loader, model, sysfs, and driver opens |
| `close` | 22 | Explicit file closes |
| `mmap` | 69 | Loader and anonymous process mappings |
| `mprotect` | 181 | Loader protections and memory guards |
| `munmap` | 10 | Userspace mapping release |
| `brk` | 3 | Heap boundary |
| `clone` | 1 | Probe worker thread |
| `futex` | 4 | Rust/thread synchronization |
| `ppoll` | 1 | Runtime startup check |
| `ioctl` | 73 | GAL interface transactions |
| `write` | 178 | Probe diagnostics plus metadata-only tracer records |
| `exit_group` | 1 | Normal process exit |

The selection intentionally excludes `read` and model/tensor payloads.

## Implications for a noise-suppression runtime

PoC 3 removes uncertainty below the graph API:

- the exact stock path reaches VIP hardware type 4 through `/dev/galcore`;
- graph setup, frame execution, and teardown have stable visible boundaries;
- repeated execution reuses the graph and memory setup;
- input submission did not recreate or submit the graph;
- one execute costs one commit/event/signal group at the observed interface;
- cleanup can be checked independently of output quality.

Still required before an NPU denoiser:

1. produce a legitimate product-`0x1000000e` graph;
2. verify its operations, dtypes, layouts, and recurrent-state behavior;
3. bind real streaming frames and prove that input/output updates remain
   bounded;
4. measure deadline, quality, memory, thermals, and sustained stability;
5. keep the existing CPU denoiser as fallback;
6. test explicit long-lived context shutdown without relying on process exit.

The phase-7 signature is a validation reference, not a shortcut around those
gates.
