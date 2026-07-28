# YYYY-MM-DD — PoC N: Experiment title

## Record

| Field | Value |
|---|---|
| PoC | `N` |
| Experiment ID | `source-controlled-id` |
| Operator | human/agent alias, no personal identifiers needed |
| Device alias | `wave-lab-unit-a` |
| Start UTC | |
| End UTC | |
| Git commit | |
| Worktree state | clean / dirty with scoped explanation |
| Definition SHA-256 | |
| Raw run IDs | |
| Outcome | pass / fail / blocked / inconclusive |

## Falsifiable hypothesis

State one proposition that the experiment can disprove.

Example:

> Given exact runtime and model hashes, the allowlisted probe executes three
> graph iterations, retrieves an identical output each time, destroys the
> graph with status zero, and leaves captured NPU state unchanged.

## Why this PoC exists

Explain which program uncertainty this resolves and which later gate depends
on it.

## Safety gate

### Allowed actions

- List the exact read-only or disposable actions.

### Explicitly excluded actions

- List service stops, reboot, sysfs writes, stock replacement, raw driver
  replay, unverified graph execution, or other excluded operations.

### Timeout and recovery behavior

State:

- native watchdog;
- host timeout;
- what the harness terminates;
- what it deliberately does not recover;
- manual stop condition.

## Identity and prerequisites

### Device

```text
kernel:
device node:
NPU parameters:
```

### Artifacts

| Path | Bytes | SHA-256 | Role | Evidence |
|---|---:|---|---|---|
| | | | | |

### Experiment definition

```text
experiment ID:
definition SHA-256:
remote argv:
allowlisted environment:
timeout:
expected remote exit:
```

## Method

Record the exact high-level procedure. Prefer invoking `npu_lab.py` over
copying a generated remote wrapper.

```bash
python3 waved/scripts/npu/npu_lab.py snapshot ...
python3 waved/scripts/npu/npu_lab.py run ...
python3 waved/scripts/npu/npu_lab.py compare ...
```

If a step mutates the disposable lab directory, record its separate approval
and exact target.

## Preflight

| Check | Expected | Observed | Result |
|---|---|---|---|
| ADB state | `device` | | |
| Kernel identity | manifest | | |
| `/dev/galcore` | present | | |
| Required hashes | exact | | |
| NPU parameters | observed | | |
| Holder state | recorded | | |

Do not continue to native work when a required check fails.

## Observations

### Run A

```text
run ID:
classification:
remote exit:
semantic output:
cleanup:
post-state:
raw stdout SHA-256:
raw stderr SHA-256:
```

### Run B

```text
run ID:
classification:
remote exit:
semantic output:
cleanup:
post-state:
raw stdout SHA-256:
raw stderr SHA-256:
```

### Semantic comparison

```text
command:
exit:
result:
differences:
```

### Timing

Label this section `descriptive` unless a benchmark design with warm-up,
sample count, thermal control, scheduling, and statistics was approved.

## Evidence classification

### Observed live

- 

### Static verified

- 

### Inferred

- 

### Unknown

- 

### Negative findings

- 

## Safety postconditions

| Invariant | Before | After | Pass |
|---|---|---|---|
| ADB responsive | | | |
| `nanoqFreq` | | | |
| `enableNN` | | | |
| `recovery` | | | |
| `/dev/galcore` present | | | |
| Required hashes | | | |
| Holders understood | | | |
| Graph cleanup | n/a | | |

Record any service, firmware, model, system-library, sysfs, module, or boot
change. Expected value for a read-only experiment is “none.”

## Raw evidence

```text
artifacts/npu/RUN_ID/envelope.json
artifacts/npu/RUN_ID/stdout.txt
artifacts/npu/RUN_ID/stderr.txt
```

State the retention decision and any tracked summary hashes.

## Conclusion

Answer the hypothesis directly. Separate:

- what is proven;
- what is disproven;
- what remains unknown.

## Gate decision

```text
Current PoC gate: pass / fail / blocked
Next allowed work:
Still prohibited:
New plan required:
```

