# NPU userspace stack inventory — Insta360 Wave firmware v1.2.0.6 (consumer package `v3.0.0.1`)

> **Status: validated by static analysis only.** Every claim below is
> evidenced by either a string literal embedded in the binary, an ELF
> symbol returned by `nm`, or a file present in the extracted rootfs.
> **Anything requiring a live device is flagged as `unconfirmed`.**

This document inventories what's actually in
`firmwares/fw_v3.0.0.1/extracted/rootfs/usr/lib/` for the VeriSilicon
VIP9000 / Acuity NN stack, and explicitly separates **what we can prove
from the rootfs** from **what still requires testing on a real device**.

It exists to keep [[project_sp4_npu_status]] honest after a round of
overclaims got walked back on 2026-05-22.

## TL;DR

- VeriSilicon VIP9000 userspace stack is **present and matches the
  device's exact galcore kernel version (6.4.19)**. That removes the
  version-skew blocker called out in [[project_sp4_npu_status]] —
  *for the runtime*.
- **No host-side compiler is in the rootfs.** Producing an `.nb` (Network
  Binary Graph) from a fresh ONNX file still requires TIM-VX with a
  VIP9000 archmodel, Acuity (NDA), or device-side JIT (hypothetical).
- New `libmodel_infer.so` ships with a **generic `load_from_buf/file`
  API** that might be device-side JIT, but this is hypothesis only — not
  proven from static analysis.
- Strings reference an Amlogic **Adla NPU** (`/dev/adla0`) but **no
  `libadla.so` is in the rootfs and `nm` finds zero Adla* symbols** in
  any installed library. Adla's status on this device is unverified.

## Library inventory (with evidence)

All paths are under `firmwares/fw_v3.0.0.1/extracted/rootfs/usr/lib/`.

| Library | Size | Δ vs v1.1.6.6 | Embedded version | Evidence |
|---|---:|---|---|---|
| `libGAL.so` | 1.9 MB | byte-identical | **`$VERSION$6.4.19:946407$`** | `strings libGAL.so \| grep VERSION` |
| `libVSC.so` | 17 MB | byte-identical | **`$VERSION$6.4.19:946407$`** | `strings libVSC.so \| grep VERSION` |
| `libovxlib.so` | 5.8 MB | byte-identical | `OVXLIB_VERSION==1.2.15` | `strings libovxlib.so \| grep OVXLIB_VERSION` |
| `libArchModelSw.so` | 457 KB | byte-identical | (no version string) | sha256 unchanged across versions |
| `libOpenVX.so` | 6.4 MB | byte-identical | (no version string) | sha256 unchanged across versions |
| `libnnsdk.so` | 332 KB | +44 KB | "init vip lite, driver version=0x%08x" (runtime-printed) | `strings libnnsdk.so` |
| `libmodel_infer.so` | 2.7 MB | **NEW in v3** | `3.3.0` | `strings libmodel_infer.so \| grep -E '^[0-9]+\.'` |
| `libsentencepiece_core.so` | 1 MB | **NEW in v3** | n/a (Google) | file present |
| `libasr_psp.so` | 490 KB | +98 KB | n/a | file present |
| `libasr_reverb.so` | 8.3 MB | +1.8 MB | n/a | file present |

The `946407` field next to `6.4.19` is a build/changeset number — same
value in `libGAL.so` and `libVSC.so` so they're from the same drop.

## Device-node and sysfs surface

| Path | Referenced from | Verified by |
|---|---|---|
| `/dev/galcore` | `libGAL.so`, `libnnsdk.so` | `strings ... \| grep /dev/galcore` |
| `/sys/module/galcore/parameters/nanoqFreq` | `libInstaAudio.so`, `audioserver` | embedded full shell command: `echo 350000000 > /sys/module/galcore/parameters/nanoqFreq` |
| `/dev/adla0` | `libnnsdk.so` (`.rodata`, both v1.1.6.6 and v3) | `strings ... \| grep /dev/adla0` |
| `/sys/class/adla/adla0/device/debug/{clk_core,dpm_period,hw_info}` | `libnnsdk.so` (`.rodata`) | same |

Galcore is the kernel module backing the VIP9000 NPU. The Adla paths
suggest a second NPU exists, but see below — the userspace path isn't
in the rootfs.

## `libmodel_infer.so` — what it actually exports

Run `nm -D --defined-only libmodel_infer.so` for the authoritative list.
Summary by prefix:

| Prefix | Count | Purpose (from naming convention) |
|---|---:|---|
| `model_infer_aes_*` | 12 | Acoustic echo suppression: 1 generic `load_from_buf` + 1 generic `load_from_file` + 6 model-specific `_load_<name>_from_{buf,file}` convenience wrappers + create/destroy/run/version/delay. |
| `model_infer_ans_*` | 32 | Adaptive noise suppression: same shape as `aes` but with more model variants (`vdcrn`, `vaibf`, `vdf2`, `mulch_fusion`). |
| `bf_*` | 15 | Beamforming (classic DSP, not NN). `bf_create`, `bf_process`, `bf_apply`, `bf_update`, `bf_post_ns_strength_set`, etc. |
| `doa_srp_*` | 11 | Direction-of-arrival via Steered Response Power. `doa_srp_create_3d`, `doa_srp_get_angs`, `doa_srp_get_vad`, `doa_srp_set_phat_enable`, etc. |
| `model_infer_thread_*` | 2 | Thread-priority binding setup. |

**Key finding:** the per-model `_load_vdcrn_16k_0_07m_from_buf` -style
functions are *convenience wrappers*, not the only entry point. The
generic `model_infer_{aes,ans}_load_from_{buf,file}` symbols also exist.
Whether the generic loaders accept arbitrary model topologies (and what
format they expect — ONNX? raw NBG?) is **not determinable from the
binary alone** and needs a runtime test.

Backends referenced in strings: `NNAPI`, `OPENCL`, `OPENGLES`. The
"Navie RNN only support onnx model" string applies to one specific
backend, not the whole library.

## NBG (Network Binary Graph) loader in `libnnsdk.so`

`libnnsdk.so` contains NBG-loading log strings:

```
P read_binary_file! nbg filepath = %s
I nbg file version = %x
I nbg version = 0x%x
I nbg_size = %d
E open nbg file fail, nbg filepath = %s
E read nbg from memory fail, nbg.data is null
E nbg file qnt_type not right.
E Network creating failed. Please validate the content of nbg .
D config nbg_path: %s, model_path: %s
```

It supports versioned NBG (`< 0x00020000` vs `>= 0x00020000` branches)
and distinguishes `nbg_path` from `model_path` — suggesting at least two
input formats. The exact compile format used by Insta360 isn't
recoverable from these strings; cross-check against the 3 GiB
`model_package.bin` for actual NBG samples.

## The Adla situation (unverified)

Strings present in `libnnsdk.so` `.rodata` section:

- Function names: `AdlaCompile`, `AdlaInvoke`, `AdlaCreateContext`,
  `AdlaSetInput`, `AdlaGetCompiledData`, `AdlaWriteCompiledData`, ~30
  more. The naming pattern suggests a complete runtime-compile pipeline
  (Compile → GetCompiledData → WriteCompiledData → SetInput → Invoke).
- Device paths: `/dev/adla0`, `/sys/class/adla/adla0/device/debug/*`.
- Diagnostic format strings: ` adla Gops : %d`, ` adla hw version : %s`,
  ` adla cur clk : %d`, etc.

**But:**

- `nm` reports **0 defined Adla\* symbols and 0 undefined Adla\* symbols**
  across every binary in `usr/lib/` and `usr/bin/`.
- **No `libadla.so` / `libamlnn*.so` / `libaml_npu*.so`** is in the
  rootfs.
- The `AdlaCompile` string is in `libnnsdk.so`'s `.rodata` section
  (confirmed by mapping the string offset to ELF section headers), not
  `.dynsym` or `.debug_info` — most likely either a dlsym-name lookup
  table or hardcoded log content from a feature that didn't ship in
  this build.

**Therefore:** the Amlogic Adla NPU may exist on the SoC, but a usable
userspace path on this firmware is **not confirmed by static analysis**.
To verify, on a live device:

```bash
ls -l /dev/adla0
ls /usr/lib/ | grep -i adla
find / -name 'libadla*' 2>/dev/null
```

## What's missing (compared to a normal SDK install)

- **No headers.** `find / -name '*.h'` in the rootfs returns nothing
  under `/usr/include` (which doesn't exist).
- **No pkg-config files.** No `.pc` anywhere.
- **No static libraries.** No `.a` anywhere.
- **No CMake configs / find modules.** No `*.cmake` related to NPU.
- **No host-side compiler.** No `acuity*` / `aml_compile*` / `nbg_gen*`
  in `usr/bin` or `usr/sbin`.

This means reusing these `.so` files in `waved/` requires either:
1. Pulling API headers from the public eYs3D SDK (potentially
   version-mismatched but the API tends to be stable across point
   releases), or
2. Reverse-engineering the entry points from the binaries.

## What this means for SP#4

See [[project_sp4_npu_status]] for the full blocker map. Net contribution
of the v3 firmware extraction:

| Blocker | Status |
|---|---|
| On-device runtime version skew (galcore kernel vs userspace) | **Likely closed** — `libGAL.so` and `libVSC.so` literally embed `6.4.19:946407`, the device kernel version. Needs a confirmation test (`vsi_nn_AddTensor` against `waved-npu-probe`). |
| Host-side compile (ONNX → NPU NBG) | **Still open.** No compiler in the rootfs. TIM-VX + VIP9000 archmodel remains the cleanest known path. |
| Device-side JIT | **Hypothesis upgraded.** `libmodel_infer.so`'s generic `load_from_buf/file` API is intriguing but not proven; needs a runtime test feeding it a known model. |
| Adla NPU as alternative target | **Status unknown.** Strings hint at it; no userspace ships in this rootfs. |

## How to reproduce this report

Every claim above is reproducible from `firmwares/fw_v3.0.0.1/extracted/`:

```bash
B=firmwares/fw_v3.0.0.1/extracted/rootfs

# Library versions:
strings -a $B/usr/lib/libGAL.so | grep VERSION
strings -a $B/usr/lib/libVSC.so | grep VERSION
strings -a $B/usr/lib/libovxlib.so | grep OVXLIB_VERSION

# Device-node references:
strings -a $B/usr/lib/libnnsdk.so | grep -E '/dev/(galcore|adla)|/sys/class/adla'

# Exported symbols of libmodel_infer.so:
nm -D --defined-only $B/usr/lib/libmodel_infer.so | awk '{print $3}' | sort

# Symbol-table check that Adla* are NOT exported anywhere:
for so in $B/usr/lib/*.so $B/usr/bin/* $B/usr/sbin/*; do
  file -L "$so" 2>/dev/null | grep -q ELF || continue
  nm "$so" 2>/dev/null | grep -E " Adla[A-Z]" && echo "  ^^ in $so"
done

# Header presence:
find $B -name '*.h' -o -name '*.hpp' -o -name '*.pc' -o -name '*.a'
ls $B/usr/include 2>&1
```
