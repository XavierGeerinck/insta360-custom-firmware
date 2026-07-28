# 2026-07-28 — PoC 0: Reproducible Wave NPU Lab

## Record

| Field | Value |
|---|---|
| PoC | `0 — Reproducible NPU Lab` |
| Experiment ID | `stock-ctc` |
| Device alias | `wave-lab-unit-a` |
| Live window | 2026-07-28 08:35:37–08:35:58 UTC |
| Runner commit | `68fcd874a7e45c972c1328d4237085269c3185f0` |
| Worktree state | Runner committed; documentation and `.gitignore` changes present, therefore envelopes record `+dirty` |
| Definition SHA-256 | `9cf0a286874a1f2062383977b9a26136efba12e768ac14f4e26eef9b47e1df6a` |
| Raw run IDs | `poc0-first-control-20260728`, `poc0-second-control-20260728` |
| Outcome | **Pass** |

## Falsifiable hypothesis

Given the exact firmware runtime, model, and probe hashes in the committed
experiment definition, the host harness can:

1. refuse mismatched state before native execution;
2. execute the known Wave-targeted CTC graph three times;
3. retrieve the same nonzero output for every iteration and invocation;
4. destroy the graph with cleanup status zero;
5. preserve required artifact hashes and captured NPU state;
6. classify two independent runs as semantically equivalent.

Any mismatched hash, native timeout, missing result, differing output digest,
nonzero cleanup, incomplete post-state, or comparator difference would falsify
the passing hypothesis.

## Why this PoC exists

Earlier work proved direct inference once, but later reverse engineering needs
a repeatable laboratory boundary. Without it, a changed firmware library,
different model, stale probe, variable command, or missing cleanup could be
mistaken for a valid result.

PoC 0 does not unlock a custom model. It makes the evidence from PoCs 1–9
auditable and prevents exploratory work from silently expanding into unsafe
device mutation.

## Safety gate

### Actions performed

- host-only schema, parser, refusal, timeout, and comparison tests;
- `adb get-state`;
- bounded read-only device snapshots;
- SHA-256 reads of four required artifacts;
- read-only sysfs reads;
- point-in-time `/dev/galcore` holder scans;
- two executions of the already-present, already-hashed stock CTC probe;
- local, gitignored envelope and log creation.

### Actions not performed

- no file was pushed to or changed on the device;
- no system library or model was replaced;
- no stock service was stopped;
- no kernel module was loaded, unloaded, or reset;
- no sysfs value was written;
- no firmware or boot setting was changed;
- no reboot occurred;
- no raw driver command was replayed;
- no product-ID-patched or otherwise modified NBG was executed;
- no automatic recovery action was attempted.

### Timeout behavior

The Wave rootfs has no `timeout` command. The existing native probe contains a
15-second worker watchdog. The source-controlled host timeout is 20 seconds.
Had it fired, the harness would have terminated only the host `adb` client,
recorded `native-timeout`, attempted a read-only post-snapshot, and stopped
for manual review. Both controls completed normally.

## Host implementation evidence

The Python standard-library-only runner separates:

- strict immutable definition loading;
- production `AdbTransport`;
- bounded snapshot collection;
- exact hash preflight;
- fixed remote command construction;
- unique remote-exit sentinel parsing;
- stock result parsing;
- classification;
- atomic envelope creation;
- semantic comparison.

The production transport uses argv lists and `shell=False` on the host, no
inherited stdin, explicit timeouts, and bounded head/tail capture. The remote
shell string is generated only from validated tracked tokens. The CLI cannot
append a remote argument or environment override.

Twenty-nine fake-transport unit tests passed before the live controls. They
cover all nine classifications and the required refusal and comparison
contracts. No unit test uses ADB.

## Identity and prerequisites

### Device

```text
alias: wave-lab-unit-a
kernel: Linux buildroot 5.4.242-ab575 #2 SMP PREEMPT
        Fri Nov 28 20:02:25 CST 2025 aarch64 GNU/Linux
rootfs: Buildroot 2020.02.1-00822-g022c5748b-dirty
device tree compatible: amlogic, galcore
/dev/galcore: character device, major 199, minor 0
```

The tracked manifest calls the runtime “v1.1.6.6-compatible” because the live
library hashes exactly match that extracted firmware. This is an inference;
the bounded live rootfs identity files did not report a product release
string of `v1.1.6.6`.

### NPU state before execution

```text
nanoqFreq=838860800
enableNN=255
recovery=1
/dev/galcore holders=[]
```

The empty holder list is a point-in-time observation. It does not imply stock
services never use the NPU.

### Required artifacts

| Path | Bytes | SHA-256 | Role |
|---|---:|---|---|
| `/data/waved/waved-npu-probe.poc` | 721,696 | `fe8da5702032c397023acebd1a4bdf40ce086f6e86277a52c4f059f409552db0` | Native control |
| `/usr/lib/libnnsdk.so` | 288,448 | `36b87d6ce89f4253b9120cb89b5cc1ce33a2cab84e4acb2ee010742b66cc44fe` | Model lifecycle/tensor ABI |
| `/usr/lib/libovxlib.so` | 5,838,160 | `09e740b4d4b0a759fb7f274ebf91456bdd6b702c96e47ab04a8a1655ffc65ba9` | VeriSilicon NN runtime |
| `/model/asr_de/de_online/ctc_online.nb` | 2,810,984 | `5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66` | Unmodified Wave-targeted graph |

The separately inventoried `/usr/lib/libArchModelSw.so` was 457,080 bytes
with SHA-256
`2b3df0844a2ff582f71da6517113939b54acdc7c97bebcac771e6e58140df63f`.
It is recorded in the manifest but is not a PoC 0 execution gate because the
probe does not load it directly.

### Experiment definition

```text
remote argv:
  /data/waved/waved-npu-probe.poc
  --run-stock-ctc

environment:
  LD_LIBRARY_PATH=/data/waved/lib:/usr/lib
  VIVANTE_SDK_DIR=/usr

host timeout: 20000 ms
expected remote exit: 0
expected status: PASS
expected iterations: 3
expected output bytes: 332800
expected output sha256:
  e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
expected cleanup: 0
```

## Method

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -v
python3 -m py_compile waved/scripts/npu/npu_lab.py
python3 waved/scripts/npu/npu_lab.py list

python3 waved/scripts/npu/npu_lab.py snapshot \
  --experiment stock-ctc \
  --output artifacts/npu/poc0-snapshot.json

python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu \
  --run-id poc0-first-control-20260728

python3 waved/scripts/npu/npu_lab.py run stock-ctc \
  --artifact-root artifacts/npu \
  --run-id poc0-second-control-20260728

python3 waved/scripts/npu/npu_lab.py compare \
  artifacts/npu/poc0-first-control-20260728/envelope.json \
  artifacts/npu/poc0-second-control-20260728/envelope.json
```

The standalone snapshot and both run-integrated preflights observed all
required hashes exactly.

## Graph contract observed in both runs

```text
NBG version: 0x00020000
NBG target:  0x1000000e

input:
  name: input/output[0]
  dtype: FP16
  dimensions: [256, 16, 1]
  bytes: 8192
  value: deterministic zero-filled tensor

output:
  name: uid_20000_sub_uid_1_out_0
  dtype: FP32
  dimensions: [5200, 16, 1]
  bytes: 332800
```

Resolved model API:

```text
aml_module_create
aml_module_input_set
aml_module_output_get
aml_module_destroy
aml_util_getInputTensorInfo
aml_util_getOutputTensorInfo
aml_util_freeTensorInfo
```

## Run observations

### Control 1

```text
run ID: poc0-first-control-20260728
timestamp: 2026-07-28T08:35:37.094Z
classification: success
host ADB duration: 340 ms
remote exit: 0

iteration 1: 8.198 ms
iteration 2: 2.241 ms
iteration 3: 2.290 ms
mean: 4.243 ms

output bytes: 332800
output sha256:
  e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
cleanup: 0

stdout sha256:
  2951562950f7a2b9f96da610345998e094682cc20ab194cefd6ac564188566ea
stderr sha256:
  e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

### Control 2

```text
run ID: poc0-second-control-20260728
timestamp: 2026-07-28T08:35:57.813Z
classification: success
host ADB duration: 182 ms
remote exit: 0

iteration 1: 3.539 ms
iteration 2: 2.155 ms
iteration 3: 2.160 ms
mean: 2.618 ms

output bytes: 332800
output sha256:
  e621b2134934ab94ab83049b4b56e5bb410831de41d9727aaf95384d413b52d4
cleanup: 0

stdout sha256:
  6a96dc765c6aa87c506a931ca3bf4fd7498fe382d4b6afcac922be3daab20fb8
stderr sha256:
  e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

The empty stderr digest is the SHA-256 of an empty file.

### Semantic comparison

```text
result: equivalent
exit status: 0
differences: none
```

The comparator included experiment and definition identity, required and
observed hashes, kernel, pre/post NPU parameters, classification, remote exit,
semantic output, cleanup, and post-health. It ignored timestamps, run IDs,
uptime, PIDs, pointer values, durations, and average latency.

## Descriptive timing

Across the six native iterations:

```text
minimum: 2.155 ms
median:  2.266 ms
maximum: 8.198 ms
```

This is not a benchmark. The sample is small, includes cold-start effects,
uses zero-filled input, and does not control thermal or scheduler state. It
only demonstrates that both graph lifecycles completed far inside the probe's
15-second watchdog.

## Safety postconditions

| Invariant | Before | After both controls | Pass |
|---|---|---|---|
| ADB responsive | yes | yes | yes |
| `nanoqFreq` | `838860800` | `838860800` | yes |
| `enableNN` | `255` | `255` | yes |
| `recovery` | `1` | `1` | yes |
| `/dev/galcore` present | yes | yes | yes |
| Required hashes | exact | exact | yes |
| `/dev/galcore` holders | none observed | none observed | yes, point-in-time |
| Graph cleanup | n/a | `0` on each run | yes |
| Post-snapshot complete | n/a | yes on each run | yes |

No device-side state mutation was requested or observed.

## Evidence classification

### Observed live

- `/dev/galcore` was reachable from the controlled process.
- The exact stock NBG was read, identified, created, executed six times,
  retrieved, and destroyed across two process lifecycles.
- Every iteration returned one 332,800-byte output with the same digest.
- Cleanup returned zero twice.
- Captured NPU parameters and artifact hashes remained unchanged.
- ADB remained responsive.

### Static verified

- The local v1.1.6.6 runtime files match the live `libnnsdk.so`,
  `libovxlib.so`, and `libArchModelSw.so` hashes.
- The local stock German CTC NBG matches the live model hash.
- The probe source uses deterministic input and validates tensor counts,
  lengths, nonzero output, stable per-iteration digest, and cleanup.
- The host test suite covers schema refusal, bounded capture, every
  classification, and semantic comparison.

### Inferred

- The live rootfs is described as v1.1.6.6-compatible because exact runtime
  hashes match that extraction.
- The larger first iteration in each invocation is consistent with
  initialization or warm-up cost, but PoC 0 does not isolate the cause.

### Unknown

- The exact compiler configuration needed to produce product `0x1000000e`.
- The complete NBG section schema.
- The precise driver ioctl sequence and command-buffer format.
- Whether firmware v3 `libmodel_infer.so` uses CPU, OpenCL, NNAPI, this NPU,
  or different backends per model.
- Which operations and recurrent-state patterns needed by a denoiser are
  supported.
- Production noise-suppression quality, latency, power, and contention.

### Negative findings retained from the preceding direct-inference work

- The repository's current host-generated add NBG has version `0x0001001e`
  and product `0x10000020`.
- The Wave rejects it with “hardware version is not in product list.”
- Therefore the current host simulator output is not a custom-model path for
  this Wave.
- Product-field patching is not a valid or safe compiler substitute and was
  not attempted.

## Raw evidence

```text
artifacts/npu/poc0-snapshot.json
artifacts/npu/poc0-first-control-20260728/envelope.json
artifacts/npu/poc0-first-control-20260728/stdout.txt
artifacts/npu/poc0-first-control-20260728/stderr.txt
artifacts/npu/poc0-second-control-20260728/envelope.json
artifacts/npu/poc0-second-control-20260728/stdout.txt
artifacts/npu/poc0-second-control-20260728/stderr.txt
```

These files remain local and gitignored. Stable identities, output digests,
raw-log hashes, results, and limits are preserved in this notebook and the
device manifest.

## Conclusion

The hypothesis passed.

PoC 0 proves a reproducible, hash-pinned, bounded path from a host tool to
actual NPU inference through the Wave's stock runtime. It also proves the
harness can compare repeated results while ignoring legitimate volatile
fields. The evidence is sufficient to use this harness as the execution gate
for later NPU research.

It does not prove custom-model compilation, generic ONNX ingestion, operation
support, or noise-cancellation readiness.

## Gate decision

```text
PoC 0 gate: PASS

Next allowed work:
  PoC 1 host-side runtime ABI and call-graph mapping.
  New dynamic probes only after they are separately planned, hashed,
  allowlisted, internally bounded, and staged under /data/waved/npu-lab/.

Still prohibited:
  unverified graph execution
  NBG product-ID patching
  blind /dev/galcore mutation fuzzing
  service stops
  sysfs writes
  driver reset
  firmware or boot changes
  stock model/library replacement
```

