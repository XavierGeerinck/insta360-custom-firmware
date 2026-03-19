# Insta360 Wave — Model Partition Recovery Guide

How to back up, wipe, and restore the 3GB `/model` partition (AI/ASR models).

---

## Key Facts

| Property | Value |
|---|---|
| **Device** | `/dev/model` |
| **Filesystem** | ext4 (mounted read-only) |
| **Mount point** | `/model` |
| **Size** | 3.0 GB (3,221,225,472 bytes) |
| **Contents** | ASR models (WeNet, SenseVoice, Zipformer), VAD, speaker diarization, punctuation |
| **Boot dependency** | **None** — mount failure is non-fatal, boot continues |
| **SWU inclusion** | **Not included** in main firmware SWU — separate `software_model.swu` package |
| **MD5** | `a21d244e871aee5efb1ce581d771e2db` (v1.1.6.6) |

---

## Step 1: Back Up (Before Wiping)

### Via ADB (recommended)

```bash
# Enable ADB first (see GUIDE_DEVICE_ACCESS.md)
python3 enable_adb.py
sleep 3

# Pull the entire model directory (3GB, takes ~10 min over USB)
adb pull /model/ ./model_backup/

# Or make a raw partition image (exact byte-for-byte copy)
adb shell "dd if=/dev/model bs=1M" | dd of=./model_partition_backup.img bs=1M
```

### Via ADB Shell + USB Storage

```bash
adb shell
# Copy to user storage partition (if space available)
dd if=/dev/model of=/mnt/udisk/model_backup.img bs=1M
# Then pull via USB file transfer mode
```

### Verify Your Backup

```bash
# Check MD5 of the raw partition image
md5 model_partition_backup.img
# Should match: a21d244e871aee5efb1ce581d771e2db (for v1.1.6.6)

# Or verify the pulled directory has expected contents
ls model_backup/
# Expected: asr_ch/ asr_en/ asr_de/ asr_fr/ asr_ja/ vad/ diarization/ punc_model/ punc_mino/ model_version.txt
```

---

## Step 2: Wipe the Model Partition

### Option A: Format as empty ext4

```bash
adb shell
# Unmount first
umount /model
# Format
mkfs.ext4 /dev/model
# Remount (empty)
mount -t ext4 -o ro /dev/model /model
```

### Option B: Leave unmounted

```bash
adb shell
umount /model
# Edit datamount to skip the mount (persistent via overlay)
# The mount is in /etc/datamount line 87:
#   mount -t ext4 -o ro /dev/model /model
# Comment it out or the app will just see an empty /model — both are fine
```

**Note:** The `pspapp` application will log errors when it can't find ASR models, but it will continue running. Audio processing (TEA SDK), Bluetooth, WiFi, USB audio, UI — all continue working.

---

## Step 3: Restore from Backup

### Method 1: Raw partition write via ADB (recommended)

```bash
# Push the backup image and dd it back
adb push model_partition_backup.img /tmp/model_restore.img
adb shell "dd if=/tmp/model_restore.img of=/dev/model bs=1M"
adb shell "rm /tmp/model_restore.img"
adb shell "sync"
adb shell "reboot"
```

### Method 2: Restore from software_model.swu

The model SWU contains a raw ext4 image starting at offset `0x100004` (after the 4-byte magic + 1MB JSON header). The device's OTA code writes this directly to `/dev/model`.

```bash
# Extract the ext4 image from the SWU file
python3 -c "
import sys
with open('software_model_1_1_6_6.swu', 'rb') as f:
    f.seek(0x100004)  # Skip magic (4 bytes) + JSON header (1MB)
    with open('model_partition.ext4', 'wb') as out:
        while True:
            chunk = f.read(1024 * 1024)  # 1MB chunks
            if not chunk:
                break
            out.write(chunk)
print(f'Extracted model partition image')
"

# Verify MD5
md5 model_partition.ext4
# Should match: a21d244e871aee5efb1ce581d771e2db

# Write to device via ADB
adb push model_partition.ext4 /tmp/model_restore.img
adb shell "dd if=/tmp/model_restore.img of=/dev/model bs=1M"
adb shell "rm /tmp/model_restore.img"
adb shell "sync"
adb shell "reboot"
```

### Method 3: dd fallback (on-device)

The firmware also checks for a pre-placed image file:

```bash
adb shell
# Place the image on the user storage partition
adb push model_partition.ext4 /mnt/udisk/model.ext4.img2simg

# The OTA code will detect this file and dd it to /dev/model
# Or do it manually:
dd if=/mnt/udisk/model.ext4.img2simg of=/dev/model bs=1M
sync
reboot
```

### Method 4: Re-download via OTA endpoint

If you still have network access and the device is registered:

```bash
# The device checks this endpoint for model updates:
# https://openapi.insta360.com/app/v1/common/upgrade/psp_model
# With headers: X-Device-Serial, X-Device-Version

# You can trigger this from the device:
adb shell
curl -s \
  -H "X-Device-Serial: $(cat /sys/class/unifykeys/read)" \
  -H "X-Device-Version: 1.1.6.6" \
  https://openapi.insta360.com/app/v1/common/upgrade/psp_model \
  -o /tmp/psp_model.json

# Parse the download URL from the JSON response and download
```

---

## Verification After Restore

```bash
adb shell

# Check mount
mount | grep model
# Should show: /dev/model on /model type ext4 (ro,...)

# Check model version
cat /model/model_version.txt
# Should show: V2.0.0

# Check key files exist
ls /model/asr_en/reverb-asr-10s/
ls /model/vad/silero_vad_v5.onnx
ls /model/diarization/campp_118_cn_en_fp16.nb

# Check MD5 of the partition
md5sum_result=$(adb shell "dd if=/dev/model bs=1M | md5sum")
echo $md5sum_result
# Should match: a21d244e871aee5efb1ce581d771e2db
```

---

## What Happens If /model Is Empty or Missing

| Component | Behavior |
|---|---|
| **Boot** | Continues normally — mount failure is non-fatal |
| **Audio (TEA SDK)** | Works — TEA models are in `/usr/bin/tea_models/` on rootfs, NOT on /model |
| **USB Audio (UAC2)** | Works |
| **Bluetooth Audio** | Works |
| **UI (LVGL)** | Works — shows errors for ASR features |
| **ASR/Transcription** | **Fails** — models not found, logs errors |
| **Speaker Diarization** | **Fails** — CAM++ model missing |
| **Recording** | **Works** — audio recording doesn't require ASR |
| **Cloud Upload** | Works — if cloud server is configured |
| **Telemetry** | Still runs (unless removed from firmware) |

**Bottom line:** Wiping `/model` only breaks on-device ASR/transcription. Everything else works. Since you're offloading transcription to your own API, this is exactly what you want.

---

## SWU File Format Reference

```
software_model_*.swu:
  Offset 0x000000: [4 bytes]  Magic: 0x21 0x43 0x23 0x01
  Offset 0x000004: [1 MB]     JSON header (version, model_md5, model_size)
  Offset 0x100004: [~3 GB]    Raw ext4 filesystem image → written to /dev/model

Installation method: Direct write() in 1MB chunks, NOT via SWUpdate.
Verification: MD5 string comparison only (no cryptographic signature).
Retry logic: Up to 3 attempts with full rewrite on failure.
```

---

*Guide based on decompiled firmware analysis (libins_entry.so lines 127007-127850) and boot script review (datamount).*
