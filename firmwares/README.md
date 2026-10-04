# Insta360 Wave firmware — extract / decompile / diff

How to take an Insta360 Wave firmware blob, turn it into a browsable
rootfs + decompiled C, and compare it to other versions.

All paths in this README are relative to the repo root
(`/Users/xaviergeerinck/Projects/insta360/`).

## Layout

```
firmwares/
  fw_v1.0.13.6/                      # consumer release
    software_model_1_0_13_6.swu      # raw firmware blob
    extracted/                       # produced by extract step
      rootfs/                          # unsquashed rootfs
      BT.bin DONGLE.bin MCU.bin ...    # peripheral firmware + boot/dtb
    decompiled/                      # produced by decompile step
      *.c                              # one per target binary
      diff_*.diff SUMMARY.md           # (only in newest version's dir)
  fw_v1.1.6.6/                       # consumer release
  fw_business_v1.0.2.6/              # business/“tob_verify” SKU
  fw_v3.0.0.1/                       # consumer pkg 3.0.0.1 → on-device 1.2.0.6
  fw_v1.2.2.6/                       # consumer pkg 3.0.0.1 re-spin → on-device 1.2.2.6 (latest, SWU only)
    extracted/
      model_package.bin                # 3 GiB NPU model package (opaque)
      tail_blob.bin                    # 583 MB trailing region (opaque)
```

When a new firmware lands, drop it into a new `firmwares/fw_<version>/`
directory and run the three steps below.

## 1. Container format

All Insta360 Wave SWU files (both `.swu` and `.bin`) share the same layout:

```
offset 0x000000   4 B       magic   "!C#\x01"  (consumer)  OR  "1BOT"  (business)
offset 0x000004   1 MiB     JSON header (zero-padded; readable changelog + sizes)
offset 0x100004   variable  CPIO archive ("070702" newc CRC, swupdate-style)
```

The `v3.0.0.1` consumer build is a **packed** package — the CPIO archive is
followed by a 3 GiB NPU model image and a 583 MB tail blob:

```
offset 0x100004        CPIO archive (rootfs.squashfs + boot.img + MCU.bin + ...)
offset bin_size        NPU model package (opaque, 3 GiB exactly)
offset bin_size+3GiB   Tail blob (opaque, currently un-analysed)
```

`bin_size` is read from the JSON header. To enumerate it without parsing JSON,
the SWU portion always ends at the start of the model region; for the older
unpacked SWUs it equals the file length.

## 2. Extract

```bash
# Replace <ver> with the directory name, e.g. fw_v3.0.0.1
cd firmwares/<ver>
mkdir -p extracted && cd extracted

# Slice out the CPIO archive from offset 0x100004 and unpack it.
# Feeding the whole file is safe — cpio stops at TRAILER.
python3 -c "
import sys
with open('../<source.bin>', 'rb') as f:
    f.seek(0x100004)
    while True:
        b = f.read(4*1024*1024)
        if not b: break
        sys.stdout.buffer.write(b)
" | cpio -i

# Verify SHAs against sw-description:
for f in BT.bin MCU.bin DONGLE.bin boot.img dtb.img rootfs.squashfs udisk_s.exfat; do
  a=$(shasum -a 256 $f | cut -d' ' -f1)
  c=$(grep -A1 "filename = \"$f\"" sw-description | grep sha256 | cut -d'"' -f2)
  [ "$a" = "$c" ] && echo "OK  $f" || echo "BAD $f  got=$a want=$c"
done

# Unsquash the rootfs:
unsquashfs -d rootfs rootfs.squashfs

# For packed v3+ files, save the trailing regions for later analysis:
python3 -c "
import os
src='../<source.bin>'
bin_size = 480_591_872     # read from JSON header
model_size = 3_221_225_472
with open(src,'rb') as f, \
     open('model_package.bin','wb') as mp, \
     open('tail_blob.bin','wb') as tb:
    f.seek(bin_size)
    for _ in range(model_size // (8*1024*1024)):
        mp.write(f.read(8*1024*1024))
    mp.write(f.read(model_size % (8*1024*1024)))
    while True:
        b = f.read(8*1024*1024)
        if not b: break
        tb.write(b)
"
```

Tools required: `python3`, `cpio` (BSD or GNU), `unsquashfs` (`brew install
squashfs`). **BSD `dd` does not support `iflag=skip_bytes` — use the Python
shim above instead.**

### Why the JSON header inspection matters

The directory name (`fw_v3.0.0.1`) ≠ the on-device firmware version. Read the
JSON header for the real version string:

```bash
python3 -c "
with open('firmwares/fw_v3.0.0.1/Insta360WaveFW_model.bin','rb') as f:
    f.seek(4); print(f.read(0x100000).rstrip(b'\\x00').decode())
" | jq '.data.appVersion.version, .data.appVersion.bin_size, .data.appVersion.model_size'
```

For `fw_v3.0.0.1` the JSON reports `appVersion: 1.2.0.6`. `3.0.0.1` is the
firmware *package* version (not Wave Controller — that line is at 1.1.0), and
Insta360 re-published it as `1.2.2.6` under the same label. Check the vendor
catalog for new releases:
`curl -s 'https://openapi.insta360.com/app/appDownload/getGroupApp?group=insta360-wave&X-Language=en-us' | jq`

## 3. Decompile

Driven by `scripts/decompile_version.sh`, which calls Ghidra headless with
`scripts/decompile_one.java` as the post-script. Both are in `scripts/`.

```bash
# Decompile one version (sequential, ~2–4 minutes per version):
scripts/decompile_version.sh fw_v3.0.0.1

# Or run all versions in parallel (each in its own process, ~4 min total):
for v in fw_v1.0.13.6 fw_v1.1.6.6 fw_business_v1.0.2.6 fw_v3.0.0.1; do
  scripts/decompile_version.sh "$v" &
done; wait
```

The script:

- Targets the six binaries listed at the top of `decompile_version.sh`
  (`libInstaAudio.so`, `libcloud_proto_https.so`, `libdatabase.so`,
  `libins_entry.so`, `libui.so`, `pspapp`). Edit the `BINARIES=( … )` array
  to add more.
- Writes one `<binary>.c` per binary into `firmwares/<ver>/decompiled/`.
- Logs to `firmwares/<ver>/decompiled/_log.txt`.
- Skips binaries whose output already exists (re-run-safe — delete the `.c`
  to force redo).
- Uses transient Ghidra projects under `/tmp/ghidra_projects/<ver>/` and
  deletes them after each binary.

### Requirements

- Ghidra 12.x at `/opt/homebrew/Cellar/ghidra/12.0.4/libexec/` (Homebrew).
- JDK 21 at `/opt/homebrew/opt/openjdk@21/` (Homebrew). The script exports
  `JAVA_HOME` for you.
- **Use a Java post-script, not Python.** Ghidra 12 dropped bundled Jython;
  Python scripts now require launching with `pyghidra`. The existing
  `ghidra_decompile.py` at the repo root is kept for reference only.

## 4. Diff across versions

```bash
scripts/diff_versions.sh
```

Writes everything into `firmwares/fw_v3.0.0.1/decompiled/`:

- `SUMMARY.md` — per-binary stats table (functions / lines / KB per version)
  and a diff index with `+added / -removed` line counts.
- `diff_<binary>_<oldver>_to_<newver>.diff` — one unified diff per
  consecutive pair across the chronological order
  `v1.0.13.6 → v1.0.2.6(business) → v1.1.6.6 → v3.0.0.1`.

The diffs are computed after stripping `// Function at 0x…` header lines so
that trivial address shifts don't drown out real changes.

Edit the `VERS=( … )` array in `diff_versions.sh` to change the order or
add a new version. The script is idempotent — re-running overwrites.

## Gotchas

- Cpio extraction will silently truncate `udisk_s.exfat` **and drop
  `BT.bin`/`MCU.bin`/`DONGLE.bin`** (they come after it, past `bin_size`)
  if the input stream is capped to `bin_size`. When range-fetching, grab
  ~80 MB beyond `bin_size`. Always feed the entire SWU
  to cpio — it stops cleanly at TRAILER.
- The `appVersion` in the JSON header drives the actual on-device version
  string; don't trust the directory name or filename.
- Decompiled `.c` outputs in `decompiled/` are ~50 MB per version; the
  diffs add ~100 MB more. `firmwares/` is **not** gitignored — the existing
  `.gitignore` entries `fw_v1.0.13.6/` and `fw_v1.1.6.6/` are stale (they
  predate the move into `firmwares/`). Add `firmwares/` (or per-version
  `firmwares/<ver>/extracted/` + `decompiled/`) to `.gitignore` before
  committing if you don't want this in git.
- Re-decompiling a binary that has had Ghidra hand-renames applied will
  lose those names — the script imports a fresh program each run. If we
  start curating Ghidra projects, switch the script to keep them around
  and use `-process` instead of `-import`.
