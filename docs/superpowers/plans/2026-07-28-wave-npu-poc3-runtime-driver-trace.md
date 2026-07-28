# Wave NPU PoC 3 — Runtime and Driver Trace Plan

**Date:** 2026-07-28  
**Status:** Approved for execution; implementation in progress  
**Depends on:** PoC 0 pass, PoC 1 pass at commit `f68708d`, PoC 2 pass at commit `9f8c035`  
**Safety class:** Hash-pinned, bounded observation of the already-validated stock CTC graph  
**Program design:** `docs/superpowers/specs/2026-07-28-wave-npu-reverse-engineering-program-design.md`

## 1. Goal

Observe one complete, successful stock CTC transaction from the explicit
`waved-npu-probe` lifecycle markers through the userspace GAL boundary and
the `/dev/galcore` system calls.

PoC 3 answers:

- Which shared objects are loaded for the known-good NN SDK path?
- Which file, mapping, synchronization, and driver calls occur during graph
  creation, input submission, execution, output retrieval, and cleanup?
- Which galcore command identifiers occur, in what order, and with which
  bounded interface metadata?
- Is that normalized sequence stable across two equivalent runs?
- Does observation preserve the established output digest and device state?

PoC 3 does **not**:

- execute a new, modified, foreign, or product-ID-patched graph;
- replay, mutate, or fuzz a galcore request;
- capture NBG bytes, tensor contents, weights, command buffers, or audio;
- overwrite a stock library, model, executable, or system path;
- stop a service, reboot, reset the driver, alter a module parameter, or
  write to debugfs/sysfs.

## 2. Accepted Control

The only graph executed is the existing PoC 0 control:

```text
probe:       /data/waved/waved-npu-probe.poc
probe SHA:   fe8da5702032c397023acebd1a4bdf40ce086f6e86277a52c4f059f409552db0
mode:        --run-stock-ctc
model:       /model/asr_de/de_online/ctc_online.nb
model SHA:   5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66
iterations:  3
output:      332800 bytes
output SHA:  e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
cleanup:     0
```

The probe has its own 15-second watchdog. The allowlisted host experiment
has a shorter fixed operational budget than the global harness maximum and
does not issue a remote kill or reset after a timeout.

## 3. Evidence Baseline

### 3.1 Exact live Wave artifacts

The 2026-07-28 read-only capability survey established:

| Artifact | Live SHA-256 |
|---|---|
| `/usr/bin/strace` | `2046787af2bec0edd5c3483b59bcbd870e5a0fb68cc3c618e9e464e61ee62aa2` |
| `/usr/lib/libGAL.so` | `13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6` |
| `/usr/lib/libnnsdk.so` | `36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe` |
| `/usr/lib/libovxlib.so` | `09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9` |

The live tracer is `strace 5.9`. The live GAL version observed statically in
PoC 1 is `6.4.19.5.946407`.

### 3.2 Version-adjacent public driver source

The public eYs3D eCV5546 Linux tree is pinned at:

```text
repository: https://github.com/eYs3D/eCV5546_linux
commit:     946d2c841fd82b916bdd2168033906bfec89db61
GAL tree:   drivers/media/platform/eys3d/vip9000/v6.4.18.5/
```

Relevant source hashes:

| File | SHA-256 |
|---|---|
| `hal/os/linux/kernel/gc_hal_kernel_driver.c` | `1581ea445e8012111815657f297134791c4dfa933eb82a3f486c010a69b0f977` |
| `hal/os/linux/kernel/gc_hal_kernel_os.h` | `a8b08fa5e1c273d173e848959f8b5ad0ff5daefdbc957154eb0cd1f04d315302` |
| `hal/kernel/inc/shared/gc_hal_driver_shared.h` | `239638f4db4444832d951ba9941e30dac408abc7351d7ffbb3b13bba52f20e7e` |
| `hal/kernel/inc/shared/gc_hal_enum_shared.h` | `961c621c92cbf55b40ae90eaae0c100056c1d0c3b708187238bf1874fdf64557` |

That source shows:

- ioctl request `IOCTL_GCHAL_INTERFACE = 30000`;
- a four-`uint64_t`, 32-byte `DRIVER_ARGS` envelope;
- equal input and output `gcsHAL_INTERFACE` sizes;
- a `gcsHAL_INTERFACE` prefix containing command, hardware type, core index,
  and status;
- kernel copy-in, dispatch, and copy-out around the interface;
- the adjacent command enumeration used to label observed command numbers.

Because public source is `6.4.18.5` and the live library is `6.4.19.5`, all
names and layouts remain explicitly **version-adjacent** until corroborated
by the exact library or live behavior. The tracer records raw numeric values
alongside any provisional label.

## 4. Instrumentation Design

### 4.1 Existing explicit lifecycle markers

The hash-pinned stock probe already logs:

1. prerequisites and model read;
2. read-only NPU status;
3. NN SDK load and symbol resolution;
4. input/output tensor metadata;
5. graph creation from stock NBG memory;
6. deterministic zero-filled input submission;
7. three execute/retrieve/verify iterations;
8. graph destruction and resource release.

No probe rebuild is required for this PoC.

### 4.2 Fixed `strace` wrapper

A source-controlled shell wrapper is cross-independent. The originally
planned persistent path was
`/data/waved/npu-lab/stock-ctc-trace`. Live pre-staging found `/data` at
100% with zero available blocks, and directory creation failed before either
artifact could be written. No existing data was deleted or moved. The safer
execution path is therefore the exact ephemeral path:

```text
/tmp/wave-npu-lab/stock-ctc-trace
```

It has no arguments and hard-codes:

- exact probe and interposer paths;
- `LD_LIBRARY_PATH=/data/waved/lib:/usr/lib`;
- `VIVANTE_SDK_DIR=/usr`;
- `LD_PRELOAD=/tmp/wave-npu-lab/libwave-gal-trace.so`;
- `strace -f -qq -ttt -T -yy`;
- a fixed syscall allowlist;
- `/data/waved/waved-npu-probe.poc --run-stock-ctc`.

The syscall allowlist covers process/thread creation, dynamic-loader file
access, `/dev/galcore` open/close, mapping, synchronization, polling, ioctl,
and process exit. A first instrumented pilot proved that this firmware's ADB
shell uses a PTY that merges trace stderr into probe stdout. Although native
inference passed, that byte-level interleaving split the `RESULT` line and
caused the host parser to reject the run. The corrected wrapper writes
`strace` and GAL diagnostics to two exact mode-0600 files under the same
ephemeral directory, emits them in named frames only after the clean probe
result, then deletes both files. It does not use `-s` to enlarge string
capture and does not request memory dumps.

`ioctl` is requested in raw numeric form. The wrapper never asks `strace` to
dereference the request envelope or print pointed-to memory.

### 4.3 Metadata-only ioctl interposer

`libwave-gal-trace.so` interposes only `ioctl(2)`. Other syscall boundaries
come from `strace`, avoiding a broad and fragile preload surface. The fixed
syscall list includes `write(2)` solely so the probe's existing `[N/8]`
diagnostic markers retain ordering in the raw trace; no write buffer is
decoded into the tracked report.

For request `30000` only, the interposer:

1. reads the 32-byte public `DRIVER_ARGS` envelope;
2. validates that input/output pointers are non-null;
3. validates that interface sizes are equal and within a conservative
   bounded range;
4. reads only the first 16 bytes of each interface;
5. records command, hardware type, core index, interface size, return code,
   `errno`, and returned status;
6. forwards the original request unchanged to libc;
7. preserves the forwarded call's `errno`.

It never logs an address and never reads the command-specific union after
the prefix. It does not intercept, replace, delay, retry, or synthesize a
request.

Trace lines are short ASCII records written directly to stderr with one
`write(2)` call. A thread-local reentrancy guard prevents recursive tracing.
An atomic sequence counter gives an order independent of timestamps.

The pilot also observed that the probe's detached worker can begin one final
successful unlock ioctl while the main thread calls `exit_group`. The entry
record and successful `strace` result are present, but process exit can
preempt the interposer's post-call record. The normalizer permits this only
for exactly the final contiguous call and only when the strace ioctl count
matches and every syscall result is zero. It remains a documented teardown
race, not silently synthesized status.

The C implementation has compile-time assertions for the 32-byte envelope
and 16-byte prefix. A host unit test exercises the parser/normalizer against
valid and malformed synthetic lines. The target binary is checked with
`file`, `readelf`, and `objdump` before staging.

### 4.4 Dynamic-loader observation

The fixed file syscall trace records library paths opened by the dynamic
loader. The report treats paths actually seen under the traced process as
dynamic evidence and compares them with PoC 1's static dependency map.

No `LD_DEBUG` payload is enabled because it adds substantial noise and is
not needed to establish loaded paths.

## 5. Trace Data Policy

Raw run output remains under gitignored `artifacts/npu/runs/`.

Tracked reports contain only:

- source and staged artifact hashes;
- normalized lifecycle markers;
- normalized syscall names and path categories;
- numeric ioctl command IDs and provisional names;
- hardware/core/status/size metadata;
- counts, ordering, and phase correlations;
- output digest and health comparison;
- unresolved or conflicting evidence.

Normalization removes:

- PIDs and TIDs;
- file descriptor numbers;
- absolute and relative timestamps;
- syscall durations;
- pointer-shaped values and mapping addresses;
- host run-directory names;
- incidental timing text in probe output.

The normalizer rejects unknown trace record syntax rather than silently
guessing. It never emits raw pointers or raw ioctl argument text.

## 6. Device Execution Sequence

### Phase A — host build and validation

1. Compile the C interposer for `aarch64-unknown-linux-gnu` in the repository's
   pinned cross container.
2. Confirm ELF class, machine, dynamic type, imported symbols, and glibc
   compatibility.
3. Run host parser/normalizer tests.
4. Hash the interposer and wrapper.

### Phase B — reversible staging

1. Verify ADB state and the live exact artifact hashes.
2. Attempt the preferred `/data/waved/npu-lab/` path without deleting or
   moving existing data.
3. If `/data` has no free blocks, create only `/tmp/wave-npu-lab/` in the
   device's executable tmpfs and allowlist the exact wrapper path rather than
   the directory.
4. Push the two new files under the selected research directory.
5. Set executable/read permissions only on those two paths.
6. Read back both hashes from the Wave.
7. Add the exact staged hashes to the source-controlled experiment registry.

Nothing is placed under `/usr`, `/lib`, `/model`, `/etc`, or a boot path.
The `/tmp` fallback is cleared by reboot and must be explicitly restaged.

### Phase C — first trace

1. Capture the harness pre-snapshot.
2. Verify every required artifact hash.
3. Run the fixed wrapper once.
4. Let the probe's internal watchdog and harness timeout bound execution.
5. Capture the post-snapshot.
6. Stop immediately if the run times out, the result digest changes, cleanup
   is nonzero, ADB is unresponsive, parameters change, or hashes drift.
7. Normalize the trace and inspect command/order coverage before any retry.

No automated recovery occurs.

### Phase D — repeatability trace

Only after Phase C passes:

1. run the identical allowlisted experiment a second time;
2. capture an independent run directory;
3. normalize it separately;
4. compare semantic result, device health, loaded-library set, command
   sequence, phase sequence, and aggregate counts;
5. document stable and variable fields.

## 7. Implementation Files

Planned source:

```text
waved/tools/npu-trace/
  gal_trace.c                 bounded ioctl metadata interposer
  stock-ctc-trace.sh          fixed no-argument device wrapper
  build.sh                    pinned cross-container build
  stage.sh                    hash-checking, non-overwriting tmpfs staging

waved/scripts/npu/
  trace_normalize.py          strict raw-to-normalized report generator
  experiments.json            exact allowlisted trace experiment
  tests/test_trace_normalize.py

docs/npu/trace/
  README.md                    result, limits, and reproduction
  galcore-interface.md        evidenced envelope and command ledger
  normalized-run-1.json       generated compact trace evidence
  normalized-run-2.json       generated compact trace evidence
  repeatability.md            comparison and lifecycle interpretation

docs/npu/lab-notebook/
  2026-07-28-poc3-runtime-driver-trace.md
```

The built `.so`, raw trace streams, and complete harness envelopes remain
gitignored experiment artifacts.

## 8. Tests

### Interposer checks

- target is an AArch64 ELF shared object;
- there is one exported `ioctl` symbol;
- no model or stock library is linked into the tracer;
- no trace line contains input/output pointers;
- unsupported requests pass through and emit no GAL record;
- invalid envelope sizes emit only a bounded rejection record;
- forwarded return code and `errno` are preserved.

The last three behaviors are host-tested with a mock forwarding seam where
practical and then confirmed indirectly on the live successful control.

### Normalizer checks

- accepts known GAL enter/exit records;
- accepts selected `strace 5.9` line forms;
- redacts PIDs, file descriptors, timestamps, durations, and addresses;
- recognizes lifecycle boundaries without copying timing values;
- maps known adjacent command numbers but preserves raw IDs;
- rejects malformed, oversized, duplicate, or impossible GAL records;
- never emits a pointer-shaped hexadecimal value;
- produces deterministic JSON for equivalent synthetic traces;
- reports unmatched enter/exit records and sequence discontinuities.

### Harness checks

- schema accepts only the exact fixed `/tmp/wave-npu-lab/stock-ctc-trace`
  fallback in addition to the existing `/data/waved/npu-lab/` policy;
- all staged and stock dependencies are required artifacts;
- missing or changed tracer/wrapper/strace/libGAL hashes reject preflight;
- only the stock result shape and output digest satisfy success.

## 9. Acceptance Gate

PoC 3 passes only if:

- one complete stock CTC create/run/retrieve/destroy trace succeeds;
- graph output remains exactly 332,800 bytes with SHA-256
  `e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4`;
- cleanup remains zero;
- the report identifies observable driver open, memory/map, submission or
  commit, synchronization/wait, and release boundaries—or explicitly marks
  any boundary not visible at this layer;
- a second run has the same normalized GAL command sequence and lifecycle
  phase sequence;
- device parameters, required hashes, galcore presence, and ADB health remain
  unchanged;
- raw payload capture, ioctl mutation/replay, recovery actions, and stock-file
  writes are all zero.

If syscall count variation is caused by scheduling, stable ordering and
phase relationships may pass while the varying count is documented. A
missing required semantic boundary does not pass merely because inference
did.

## 10. Stop Conditions

Stop without retry or recovery on:

- host or probe timeout;
- ADB transport loss;
- kernel warning/oops attributable to the trace;
- changed NPU parameter or required hash;
- nonzero graph cleanup;
- output digest mismatch;
- malformed or unexpectedly large trace output;
- an interposer record showing an invalid envelope;
- evidence that the adjacent 6.4.18.5 prefix layout does not match the live
  6.4.19.5 interaction.

The resulting failure evidence is preserved and documented. A new run needs
human review of holders and device health; it is never automatically reset.

## 11. Exit and Next Gate

Passing PoC 3 establishes an observational runtime/driver transaction map.
It does not authorize replaying captured ioctls or executing handcrafted
graphs.

The next compiler/custom-graph PoC may use the trace only to:

- distinguish compiler work from runtime submission;
- identify which command families surround legitimate graph creation;
- validate that a future tiny graph follows the stock lifecycle;
- define additional allowlisted telemetry.

Any first custom graph remains a separate plan and approval gate.
