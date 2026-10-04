# Firmware changelog: `1.2.0.6` → `1.2.2.6`

> Insta360 re-published the consumer firmware under the **same package label
> `3.0.0.1`**. Only the on-device version moved (`1.2.0.6` → `1.2.2.6`), so the
> download page looks unchanged. The vendor release notes are byte-identical to
> 1.2.0.6. This is a quiet maintenance build.

## Source

| Field | Value |
|---|---|
| Catalog | `https://openapi.insta360.com/app/appDownload/getGroupApp?group=insta360-wave&X-Language=en-us` (the `/website/` path now returns `Bad Request`) |
| File | `https://wassets.insta360.com/common/ff70a81254a945e2b7c36937f4f2ad78/Insta360WaveFW_model.bin` |
| Size / md5 | 4,285,517,828 B / `7396703ce92be6bd1cdf1730889e28d4` |
| Published | 2026-06-15 (`Last-Modified`) |
| BuildTime (`/etc/sw-versions`) | `2026-0612-1216` (1.2.0.6 was `2026-0417-1319`) |
| Jenkins build | `PSP_Releasebuild/733` (1.2.0.6 was `/693`) |

Only the SWU region was downloaded: `bin_size` = 480,600,064 B, plus about 80 MB past it.
**The cpio stream runs past `bin_size`.** `BT.bin`, `MCU.bin` and `DONGLE.bin` come
after the `udisk_s.exfat` image, so capping the stream at `bin_size` silently drops them.
The JSON header's NPU model checksum (`f3f12397…`) is unchanged from 1.2.0.6, so
the 3 GiB model region was skipped.

All seven payloads verify against `sw-description` sha256.

## Payload delta

| Payload | vs 1.2.0.6 |
|---|---|
| `rootfs.squashfs` | changed |
| `boot.img` | changed |
| `BT.bin` | **changed** (BT module firmware) |
| `DONGLE.bin` | **changed** (2.4 GHz dongle firmware) |
| `MCU.bin` | same |
| `dtb.img` | same |
| `udisk_s.exfat` | same |

## Rootfs delta

Most of the changed ELF files keep the exact same size and differ only in build-id or embedded
paths. Files that really changed:

| File | Change |
|---|---|
| `usr/lib/libins_entry.so` | +12.9 KB, see below |
| `usr/lib/libui.so` | +4.2 KB: Brazil ANATEL regulatory label |
| `etc/pspapp/lvgl/icon/msc/brasil.png` | **new**: ANATEL certificate `02717-26-15656` |
| `etc/shadow` | **new root password hash** (salt `.Dh9xp7p`); the 1.2.0.6 hash no longer applies |
| `etc/sw-versions` | `PSP_1.2.2.6` |
| `usr/lib/BT.bin`, `usr/lib/DONGLE.bin` | updated blobs (copies of the SWU payloads) |

### `libui.so`

- New `ui_supervision_brasil` widget on the regulatory ("supervision") page,
  which renders `A:/icon/msc/brasil.png`. This is a Brazil market-entry certification.
- No other functional change found. Apparent differences in functions such as `gui_set_spk_mute`
  and `gui_get_brightness` are Ghidra outlining artifacts: the same code sits behind an
  `e843419_*` veneer in one build and inline in the other.

### `libins_entry.so`

Each named function was diffed by its string literals and callees. Ghidra outlines code
differently because of the erratum-843419 veneers, which hides most of the rest.

- **`auto_sync_process_meeting`: the `qr_code_url == "NONE"` guard was removed.**
  In 1.2.0.6 a meeting whose cloud URL was set to `NONE` was skipped by
  cloud auto-upload. In 1.2.2.6 the only checks left are `upload_state ∈ {NONE, UPLOAD_FAIL}`
  and "no upload thread already exists". Our reading is that meetings
  previously marked as not to be uploaded become eligible for auto-upload. This is
  privacy-relevant for stock users and has no effect on waved, which replaces the upload path.
- `scfg_dev_occupy_space` replaces the `occupy_space` config key. This is a storage-quota
  key rename. It is the only new string literal in the library; the only removed ones are
  the `NONE` skip log line and `occupy_space`.
- Other per-function differences (`ota_thread`, `bt_ota_cmd_send`, `audio_server_request`, …)
  could not be told apart from veneer-outlining noise, because their strings exist in both builds.
  The remaining +12.9 KB is most likely codegen and veneer churn plus small logic edits.

The PC Link (vendor HID 0x03) handler table is unchanged. There are no new UI pages
beyond the regulatory label.

## Impact on waved

| Area | Action |
|---|---|
| `BT.bin` / `DONGLE.bin` | We reuse vendor peripheral blobs. Prefer the 1.2.2.6 copies when we build an image, because the dongle OTA fixes ship together with them. |
| Root password | Docs or scripts that assume the 1.2.0.6 root hash need updating for devices on 1.2.2.6. |
| Auto-upload change | Nothing to do: waved never uploads to Insta360. It's worth a line in the privacy comparison. |
| HID / Wave Controller protocol | Unchanged, so no new compatibility work. |
