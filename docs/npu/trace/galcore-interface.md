# GAL Interface Evidence

This document describes only the software-visible ioctl envelope and prefix
observed in Wave NPU PoC 3. It deliberately stops before the
command-specific union and accelerator command buffers.

## Evidence sources

### Exact live artifacts

| Item | Identity |
|---|---|
| Kernel | `Linux buildroot 5.4.242-ab575`, build 2025-11-28 |
| Live GAL version | `6.4.19.5.946407` |
| `/usr/lib/libGAL.so` | SHA-256 `13b7d87eabb8924758d49fbd8f1fac41bb81f34069f6ceed949af764893061e6` |
| `/dev/galcore` | character device major 199, minor 0 |
| Runtime ioctl request | decimal `30000`, raw display `0x7530` |
| Live interface size | 376 bytes |
| Live calls | 73 in each of two runs |

### Version-adjacent public source

Repository:
[eYs3D/eCV5546_linux](https://github.com/eYs3D/eCV5546_linux/tree/946d2c841fd82b916bdd2168033906bfec89db61)

Pinned commit:

```text
946d2c841fd82b916bdd2168033906bfec89db61
```

Relevant GAL `6.4.18.5` files:

| File | SHA-256 |
|---|---|
| `hal/os/linux/kernel/gc_hal_kernel_driver.c` | `1581ea445e8012111815657f297134791c4dfa933eb82a3f486c010a69b0f977` |
| `hal/os/linux/kernel/gc_hal_kernel_os.h` | `a8b08fa5e1c273d173e848959f8b5ad0ff5daefdbc957154eb0cd1f04d315302` |
| `hal/kernel/inc/shared/gc_hal_driver_shared.h` | `239638f4db4444832d951ba9941e30dac408abc7351d7ffbb3b13bba52f20e7e` |
| `hal/kernel/inc/shared/gc_hal_enum_shared.h` | `961c621c92cbf55b40ae90eaae0c100056c1d0c3b708187238bf1874fdf64557` |

The version gap is explicit: public `6.4.18.5` is not asserted to be the
exact source of live `6.4.19.5`. Raw numeric values remain authoritative.

## Request envelope

The public driver defines:

```c
typedef struct {
    uint64_t input_buffer;
    uint64_t input_buffer_size;
    uint64_t output_buffer;
    uint64_t output_buffer_size;
} wave_driver_args;
```

| Offset | Bytes | Neutral field | Trace handling |
|---:|---:|---|---|
| 0 | 8 | Input-interface address | Validated nonzero; never logged |
| 8 | 8 | Input-interface byte size | Logged as bounded decimal |
| 16 | 8 | Output-interface address | Validated nonzero; never logged |
| 24 | 8 | Output-interface byte size | Logged as bounded decimal |
| total | 32 | Envelope | Compile-time asserted |

The adjacent kernel `drv_ioctl`:

1. copies this 32-byte envelope from userspace;
2. requires input and output sizes to match `sizeof(gcsHAL_INTERFACE)`;
3. copies the input interface;
4. dispatches its command;
5. copies the resulting interface to the output address.

PoC 3 does not log either address. It uses `process_vm_readv` against its own
process to copy the two small fixed prefixes safely, avoiding a direct
dereference of a malformed pointer.

## HAL prefix

The public interface begins:

```c
typedef struct {
    uint32_t command;
    uint32_t hardware_type;
    uint32_t core_index;
    int32_t status;
} wave_hal_prefix;
```

| Offset | Bytes | Adjacent name | Live evidence |
|---:|---:|---|---|
| 0 | 4 | `command` | Coherent command enum values in 73/73 calls per run |
| 4 | 4 | `hardwareType` | Generic `0`, 3D `2`, and VIP `4` appear in appropriate initialization paths |
| 8 | 4 | `coreIndex` | Always 0 |
| 12 | 4 | `status` | Zero after all 146 completed calls |
| 16 | 360 | Opaque remainder | Never read by the tracer |
| total | 376 | Live interface size | Identical in every call |

The tracer reads command/hardware/core before the syscall and status after
it. It validates:

```text
input size == output size
16 <= size <= 65536
request == 30000
```

Every accepted live record used 376 bytes. No unreadable or invalid envelope
record occurred.

## Hardware types

The adjacent unconditional enumeration maps:

| Raw | Adjacent label | Live count | Interpretation |
|---:|---|---:|---|
| 0 | `INVALID` | 2 | Generic version and chip-info requests before a hardware target is selected |
| 2 | `3D` | 1 | Video-memory query |
| 4 | `VIP` | 70 | NPU/VIP initialization, allocation, execution, and teardown |

The label `INVALID` is the public enum name, not an error result. Both generic
calls returned status zero.

## Observed command ledger

The public command enumeration is linear and unconditional through raw value
32 in the pinned source. These are the only values observed:

| Raw | Adjacent command | Create | Execute | Destroy | Total |
|---:|---|---:|---:|---:|---:|
| 0 | `CHIP_INFO` | 1 | 0 | 0 | 1 |
| 1 | `VERSION` | 1 | 0 | 0 | 1 |
| 2 | `QUERY_CHIP_IDENTITY` | 2 | 0 | 0 | 2 |
| 3 | `QUERY_CHIP_OPTION` | 3 | 0 | 0 | 3 |
| 5 | `QUERY_VIDEO_MEMORY` | 1 | 0 | 0 | 1 |
| 6 | `ALLOCATE_LINEAR_VIDEO_MEMORY` | 11 | 0 | 0 | 11 |
| 8 | `RELEASE_VIDEO_MEMORY` | 0 | 0 | 7 | 7 |
| 9 | `LOCK_VIDEO_MEMORY` | 11 | 0 | 0 | 11 |
| 10 | `UNLOCK_VIDEO_MEMORY` | 0 | 0 | 8 | 8 |
| 15 | `ATTACH` | 2 | 0 | 0 | 2 |
| 17 | `EVENT_COMMIT` | 0 | 3 | 1 | 4 |
| 18 | `COMMIT` | 0 | 3 | 1 | 4 |
| 20 | `USER_SIGNAL` | 12 | 3 | 1 | 16 |
| 32 | `GET_BASE_ADDRESS` | 2 | 0 | 0 | 2 |
| | **Total** | **46** | **9** | **18** | **73** |

“Create,” “Execute,” and “Destroy” are correlations with the probe's
explicit phases 5, 7, and 8, not vendor command categories.

## Submission and wait boundary

Each of the three deterministic inference iterations emits exactly:

```text
COMMIT
EVENT_COMMIT
USER_SIGNAL
```

That is the strongest dynamic boundary recovered in PoC 3:

- `COMMIT` submits the userspace command transaction;
- `EVENT_COMMIT` associates deferred/event work;
- `USER_SIGNAL` is the visible synchronization family after each submit.

The tracer intentionally does not read the `USER_SIGNAL` union. Therefore it
does not claim whether the exact subcommand is create, signal, wait, map, or
unmap. The order and success are observed; the sub-operation is unknown.

## Memory lifecycle

Phase 5 contains 11 alternating allocation/lock pairs after initialization.
Phase 8 contains:

```text
7 × (UNLOCK_VIDEO_MEMORY → RELEASE_VIDEO_MEMORY)
COMMIT → EVENT_COMMIT → USER_SIGNAL
1 × UNLOCK_VIDEO_MEMORY
```

Allocation and release counts do not balance one-for-one within the visible
graph-destroy call. Plausible explanations include distinct ownership
classes, deferred event release, context-owned allocations, and process-exit
cleanup. The trace does not distinguish them because the command-specific
node/handle fields are deliberately opaque.

What is known:

- all calls returned syscall result zero;
- all captured HAL statuses were zero;
- `aml_module_destroy` returned zero;
- required artifacts and NPU parameters were unchanged;
- ADB and `/dev/galcore` remained available;
- the process then exited normally.

It would be incorrect to infer either a leak or perfect long-lived ownership
from the numeric mismatch alone.

## Driver file events

Both runs normalized to:

| Phase | Operation | Path | Result |
|---:|---|---|---|
| 1 | open | `/dev/galcore` | success |
| 1 | close | `/dev/galcore` | success |
| 5 | open | `/dev/galcore` | success |
| 5 | open | `/dev/galcore1` | missing |

No explicit close of the phase-5 descriptor appears. `exit_group(0)` closes
it as part of process teardown.

No `mmap` line is annotated with a `/dev/galcore` descriptor. There are 69
process mappings overall, mostly loader/anonymous mappings. The visible NPU
memory boundary is therefore the allocation/lock ioctl family, not an
fd-backed mapping claim.

## Confidence ledger

| Claim | Class | Reason |
|---|---|---|
| Request 30000 is used | Observed live | 73 calls in each trace |
| Interface size is 376 | Observed live | All input/output envelope sizes |
| Envelope has four 64-bit fields | Adjacent source, live-correlated | Public kernel copy contract plus coherent safe reads |
| Prefix offsets 0/4/8/12 | Adjacent source, live-correlated | Coherent commands, hardware types, core, and status |
| Raw hardware 4 is VIP | Adjacent source, behavior-correlated | Public enum and all NPU lifecycle commands |
| Raw command 18 is commit | Adjacent source, phase-correlated | Public enum and exact per-iteration position |
| `USER_SIGNAL` exact subcommand | Unknown | Union payload not captured |
| Accelerator packet/ISA | Unknown | Command buffers not captured or decoded |
| Physical addresses | Deliberately unobserved | Pointer/address logging prohibited |

## Why no payload capture

Command union and buffer capture would expose addresses and opaque device
programming data and would create pressure to replay it. Neither is necessary
to answer PoC 3's lifecycle question. The next compiler PoC should operate
through legitimate graph APIs and compare its normalized sequence with this
control; it should not synthesize or replay request 30000 directly.
