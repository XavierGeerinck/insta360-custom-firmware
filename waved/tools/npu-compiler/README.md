# Wave NPU Compiler and Execution Probes

This directory contains the native half of PoCs 5 through 8. Every live
binary has one immutable graph or observation, a narrow CLI, a native
watchdog, a source/ELF policy, a pinned cross-build, and a non-overwriting
staging script.

These are research probes, not a general NPU SDK.

## Proven result

One source-controlled graph has completed the full path:

```text
tim_vx_compile_erf.cc
  -> exact Wave CompileToBinary
  -> 5,840-byte target-0x1000000e NBG
  -> wave_npu_execute_erf_{zero,nonzero}.cc
  -> verified NPU readback
```

The NBG SHA-256 is:

```text
6b2ac998909fecfdf8243e0b1e2b283e5abb485d09d579ae818fbc59824de3ee
```

General graph construction is not unlocked. Add, MatrixMul, and Conv2d
built-ins select OpenVX but fail export; the tested MatrixMul and Conv2d
graphs also fail ordinary graph verification before `Graph::Run`.

## File families

| Native source | PoC | Behavior | Live result |
|---|---:|---|---|
| `tim_vx_compile_add.cc` | 5B | Compile-only U8 Add | OpenVX selected; export failed |
| `tim_vx_compile_erf.cc` | 5C | Compile-only U8 Erf | Exact-target NBG emitted |
| `wave_npu_query_caps.cc` | 5D | Context/identity/caps query only | EVIS2, subgroup 4, no SP/VA40 |
| `tim_vx_compile_signal_frame.cc` | 5E | Compile-only FP16 SignalFrame | Shader build failed; exact CLC absent |
| `wave_npu_clc_loader_probe.cc` | 5F-A | Relocation only | Three CLC exports resolved; none called |
| `tim_vx_compile_signal_frame_clc.cc` | 5F-B | Compile-only with 6.4.18 CLC | ABI failure and isolated abort |
| `wave_npu_execute_erf_zero.cc` | 6 | One NBG execution | Pass |
| `wave_npu_execute_erf_nonzero.cc` | 7 | One nonzero NBG execution | Pass |
| `tim_vx_compile_audio_denoiser.cc` | 8A | Compile-only fixed MatrixMul | Export failed |
| `wave_npu_run_audio_denoiser.cc` | 8B | Direct fixed MatrixMul | Verification failed |
| `tim_vx_compile_conv_audio_denoiser.cc` | 8C | Compile-only fixed Conv2d | Export failed |
| `wave_npu_run_conv_audio_denoiser.cc` | 8D | Direct fixed Conv2d | Verification failed |

## Build, verify, and stage split

Each probe has up to three shell scripts:

```text
build-device-*.sh
verify-device-*-policy.sh
stage-device-*.sh
```

### Build

Build scripts use the pinned digest:

```text
ghcr.io/cross-rs/aarch64-unknown-linux-gnu
sha256:7f8308a8734d9fcd2ebbe9a3e4bdea74af293f0799d80c3cc341e340cda49a4c
```

The container runs with `--network=none`. Builds use fixed source date,
locale, compiler options, device headers/libraries, and runpath. Output
binaries live below gitignored `artifacts/npu/build/`.

### Verify

The policy script runs before staging and rejects:

- a source or output path different from the fixed PoC path;
- the wrong operation count or operation family;
- a changed shape, type, quantization, constant, path, or expected value;
- an unexpected graph `Run` in compile-only tools;
- more than one compile/run/input/output action in one-shot runners;
- `CompileToBinary` in a direct runner;
- raw `ioctl`, `/dev/galcore`, `dlopen`, async graph, or direct OpenVX
  schedule/process primitives;
- a non-AArch64/non-PIE binary;
- a changed runpath or required dependency;
- an imported unreviewed execution/export primitive.

These are defense-in-depth static checks, not a proof that a private runtime
cannot have a bug.

### Stage

Staging scripts:

- require exactly the authorized serial;
- verify local binary/runtime/model hashes before ADB;
- use one unique `/tmp/wave-npu-*` root;
- reject symlinked or unexpectedly typed directories/files;
- never replace a different existing artifact;
- upload to a content-addressed incoming name;
- verify the remote hash before hard-link publication;
- set runners to `0555` and libraries/models to `0444`;
- verify `libCLC.so` presence or absence according to the reviewed PoC.

Staging does not execute a graph. It is still a device mutation and belongs
to the PoC plan that names the exact script and hashes.

## Native containment

All live tools use:

- `PR_SET_PDEATHSIG` so an abandoned parent cannot leave the process
  indefinitely;
- a native `SIGALRM` watchdog;
- an async-signal-safe timeout handler;
- a fixed identity check before graph work;
- a minimal environment and exact isolated runpath;
- no loop and no retry.

The host harness adds a longer deadline and always attempts a bounded
postflight snapshot. A timeout is a stop condition. It does not authorize a
driver reset, service kill, module action, reboot, or retry.

## Exact artifact identities

| PoC | Binary SHA-256 |
|---:|---|
| 5B | `76638c6bcbbab15cd2f466519abc6817521cddda6854b9c6da9a1840f350f10a` |
| 5C | `7f7edd31db1f0e80bcc25abfe1ae9a92cf10ce70545dc76d9c4fdd177cf35772` |
| 5D | `6a1c0ac1af75c31de32e0b8d827bbde898b6a606919d5aef3987967ac459bfe9` |
| 5E | `efb5047921574d67015da527c89df5a4321ad57ddf14ac8ffc8f13a70be0e37f` |
| 5F-A | `33efe6b0e4cf081e9b871fb12dd50654b52912cd566252caeba51883b72f8e79` |
| 5F-B | `883bb4aa3593af8d226053b0a3760f6974ab46c4516ed02f5e6953a194a4381a` |
| 6 | `ed6fa998e20e51cb785b928d1a2bb398f39770a7c95190a857627bee5d16e630` |
| 7 | `df354003e0bb89458d8d425718ca91db0ed344e1c7c792e1c7ea11fa2c779e58` |
| 8A | `e8704e8cf05afbda70c7cad0c89046cb197966aa106989dc98e733eed8f642ae` |
| 8B | `7b0919b4576c5c4feb34bac71347a65765a111ad3d52ca5c2755bc266030adfc` |
| 8C | `7ee79cb52426e9fe8658ea2636d652f0df44d2c9f9c89f9a59a28fb99643dca9` |
| 8D | `bf3e4716588055ed7c774135ed15a77ad068fddab52b362c0c0c03dfb05f2c9f` |

`libtim-vx.so` used by these device roots:

```text
d2c8fc7aaf0ad0b86f02fcd54bb1cdca9f04967b5cd9efa61a211565b453375b
```

## CLC quarantine

The version-adjacent CLC:

```text
version: 6.4.18:886827
sha256:  a1e5d11cafb6d23c7c787d24a71a766782ea022346f1580797d892238bd99f14
```

is loader-compatible but compiler-ABI-incompatible with the Wave's
`6.4.19:946407` userspace. The one authorized compiler call produced invalid
kernel references followed by allocator corruption in the short-lived
process.

Do not:

- call its three compiler exports again;
- use it for another graph;
- preload it;
- copy it into a stock library directory;
- link it into a stock service;
- treat its relocation-only pass as compatibility.

Any future compiler candidate needs a new plan and a relocation-only gate
before it may receive a compile-only graph.

## Host harnesses and evidence

The adjacent `waved/scripts/npu/` directory owns manifests, preflight,
execution, postflight, result parsing, and host tests. Raw runs live below:

```text
artifacts/npu/runs/
```

Long-form results:

- [`PoC 5 compiler notebook`](../../../docs/npu/lab-notebook/2026-07-28-poc5-custom-compiler.md)
- [`PoC 6/7 execution notebook`](../../../docs/npu/lab-notebook/2026-07-28-poc6-7-custom-nbg-execution.md)
- [`PoC 8 audio notebook`](../../../docs/npu/lab-notebook/2026-07-28-poc8-audio-denoiser-boundary.md)

## Safe development loop

For host-only changes:

```bash
python3 -m unittest discover -s waved/scripts/npu/tests -p 'test_*.py'
bash -n waved/tools/npu-compiler/*.sh
git diff --check
```

Building is also host-only. Staging or running is not part of a routine test
loop. Before any new live graph:

1. write a separate fixed native source;
2. write and review the exact graph, input, output, attempt count, watchdog,
   health envelope, and stop conditions;
3. add source and ELF policy checks;
4. build deterministically and pin the final hash;
5. add a strict manifest and fake-transport tests;
6. stage without replacement;
7. perform preflight;
8. execute at most the reviewed count;
9. stop and document on any outcome before changing the graph.

Never turn a native tool or harness into an arbitrary model/shape/op command.
