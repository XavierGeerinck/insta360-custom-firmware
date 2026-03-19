# Insta360 Wave — Live Device Dump Report

Data collected via ADB root shell on March 19, 2026.
Device serial: `PSPLB0000AAAAA`, firmware `PSP_1.1.6.6`.

---

## 1. CPU

**ARM Cortex-A55** (not A53 as initially assumed from the kernel config name):
- CPU part: `0xd05` = Cortex-A55
- CPU variant: `0x2`
- CPU architecture: ARMv8-A
- 4 cores, 48 BogoMIPS each
- Features: `fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp asimdrdm lrcpc dcpop asimddp`
- **Hardware AES/SHA** acceleration in CPU

**Serial:** `3c0b0103000000000000000000000000`

---

## 2. Memory

| Resource | Size | Usage |
|---|---|---|
| **RAM** | 3,834 MB (~3.7 GB) | 1.5 GB active, 208 MB available |
| **zram swap** | 800 MB (`/dev/zram0`) | 367 MB used (45%) |
| **Total usable** | ~4.5 GB | Memory-constrained for AI workloads |

Kernel command line sets `otg_device=1` and `androidboot.slot_suffix=_b` (currently booted from slot B).

---

## 3. eMMC Partition Map (30 GB)

| Partition | Device | Size | Mount | Filesystem | Purpose |
|---|---|---|---|---|---|
| `bootloader` | `mmcblk0p1` | 4 MB | - | Raw | U-Boot |
| (unknown) | `mmcblk0p2` | 64 MB | - | - | Reserved |
| `mmcblk0p4` | `mmcblk0p4` | 8 MB | - | - | DTB/env |
| `mmcblk0p5` | `mmcblk0p5` | 8 MB | - | - | DTB/env backup |
| `mmcblk0p6` | `mmcblk0p6` | 8 MB | - | - | Misc |
| `mmcblk0p7` | `mmcblk0p7` | 8 MB | - | - | Misc |
| `mmcblk0p8` | `mmcblk0p8` | 8 MB | - | - | Misc |
| **boot_a** | `mmcblk0p9` | 64 MB | - | boot.img | Kernel slot A |
| **boot_b** | `mmcblk0p10` | 64 MB | - | boot.img | Kernel slot B (active) |
| **system_a** | `mmcblk0p11` | 512 MB | - | SquashFS | Rootfs slot A |
| **system_b** | `mmcblk0p12` | 512 MB | `/` (ro) | SquashFS | Rootfs slot B (active) |
| **data** | `mmcblk0p13` | 512 MB | `/data` (rw) | ext4 | Persistent data + overlayfs |
| **model** | `mmcblk0p14` | 4.6 GB | `/model` (ro) | ext4 | AI models |
| **no_use** | `mmcblk0p15` | 64 MB | `/unifykey` (rw) | ext4 | Key storage |
| **udisk** | `mmcblk0p16` | 21.5 GB | `/mnt/udisk` (rw) | exFAT | User recordings |
| **psp_udisk_s** | `mmcblk0p17` | 512 MB | `/udisk_s` (rw) | exFAT | System files |
| **psp_model_s** | `mmcblk0p18` | 1.2 GB | `/tmp/psp_model` (rw) | ext4 | Model staging |

**A/B partition scheme:** The device has dual boot/system slots. Currently running from slot B (`androidboot.slot_suffix=_b`). SWUpdate writes to the inactive slot, then switches on reboot.

---

## 4. Running Processes

| PID | Process | Purpose |
|---|---|---|
| 1 | `init` | BusyBox init |
| 286 | `dbus-daemon` (system) | Session bus |
| 313 | `dbus-daemon` (system) | System bus |
| 291 | `mdev -df` | Device manager |
| 323 | `pspapp_daemon` | App supervisor |
| 324 | `pspapp_watchdog` | Crash watchdog |
| **374** | **`pspapp`** | **Main application** |
| 367 | `display_daemon_fb` | Framebuffer display |
| 371 | `hid_manager_server` | USB HID manager |
| 389 | `galcore workqueue` | NPU driver worker |
| 390 | `galcore_deamon` | NPU daemon |
| 401-405 | `atbm_*` | WiFi driver threads |
| 587-588 | `getty` | TTY login (tty1, tty2) |
| 9676 | `rsyslogd` | System logging |
| 23836 | `adbd` | Android Debug Bridge (we enabled this) |
| 29194 | `wpa_supplicant` | WiFi management |
| 29234 | `udhcpc` | DHCP client |
| 32528 | `-/bin/sh` | Serial console shell (ttyS0) |

**Key observation:** `pspapp` (PID 374) is the monolithic main application. All server modules (audio, BT, cloud, meeting, etc.) run as threads within this single process.

---

## 5. Loaded Kernel Modules

| Module | Size | Used by | Purpose |
|---|---|---|---|
| `atbm613x_wifi_sdio` | 905 KB | 0 | ATBM6031 WiFi driver |
| **`galcore`** | **856 KB** | **2** | **VeriSilicon NPU driver** |
| `aml_wifi` | 32 KB | 1 (by atbm) | Amlogic WiFi wrapper |
| `tea_license` | 24 KB | 0 | TEA SDK license enforcement |
| `ddr_access_priority` | 20 KB | 0 | DDR access priority |

---

## 6. Network Configuration

| Interface | IP | MAC | Notes |
|---|---|---|---|
| `lo` | 127.0.0.1 | - | Loopback |
| `wlan0` | 192.168.1.50 | `AA:BB:CC:DD:EE:02` | Connected to "Office 5G" |

**No firewall rules** — `iptables -L` returns empty.

**WiFi signal:** RSSI -58 dBm (signal quality 84/100).

---

## 7. Unifykeys (Hardware-Backed Secrets)

18 keys stored in Amlogic secure storage:

| # | Key Name | Value | Sensitivity |
|---|---|---|---|
| 0 | `usid` | `PSPLB0000AAAAA` | Device serial |
| 1 | `mac` | (binary) | Ethernet MAC |
| 2 | `secure_boot_set` | (efuse) | Secure boot config |
| 3 | `mac_bt` | (binary) | Bluetooth MAC |
| 4 | `mac_wifi` | (binary) | WiFi MAC |
| 5 | `deviceid` | (empty = release mode) | Debug flag |
| 6 | `region_code` | (empty) | Regional code |
| 7 | `oemkey` | (binary blob with TEA license) | **TEA SDK license key** |
| 8 | `bindflag` | (set) | Account bound flag |
| 9 | `binduserid` | `1234567` | Insta360 user ID |
| 10 | `bindusertoken` | (empty) | User auth token |
| 11 | `bindusername` | (empty) | Username |
| 12 | `binduserinfo` | (empty) | User info blob |
| 13 | `bindkey` | `AAAAAAAAAAAAAAAAAAAAAA==` | Device bind encryption key (base64) |
| 14 | `bindarea` | `https://openapi-fra.insta360.com/summary-service` | Cloud API endpoint (EU) |
| 15 | `bindcstoken` | JWT token (HS256) | Cloud session token |
| 16 | `devpassowrd` | (empty) | Device password (note: typo in key name) |
| 17 | `devmode` | (empty) | Developer mode flag |

### JWT Token Decode (bindcstoken)

```json
{
  "alg": "HS256",
  "expireTime": 1760788797661,  // 2025-10-18 13:59:57 (expired)
  "keyVersion": "0",
  "openId": "AAAAAAAAAAAAAAAAAAAAAAAAAAAA",
  "encrypt": "SFE3V0JH...",  // Encrypted user data
  "userRegion": 2,            // 2 = EU
  "timestamp": 1759924797661  // 2025-10-08 13:59:57
}
```

---

## 8. Telemetry Database (finder.db)

**118 telemetry events** stored locally, all with `upload_status=1` (uploaded to ByteDance).

Schema:
```sql
CREATE TABLE event_data (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type INTEGER NOT NULL,
    event_name TEXT NOT NULL,
    parameters TEXT,           -- JSON blob with all tracking data
    upload_status INTEGER DEFAULT 0,  -- 0=pending, 1=uploaded
    upload_time INTEGER,
    force_upload INTEGER DEFAULT 0,
    remarks TEXT DEFAULT '',
    created_at INTEGER
);
```

Sample event:
```json
{
  "event_name": "device_power_on",
  "parameters": {
    "mic_serial_number": "PSPLB0000AAAAA",
    "fw_ver": "PSP_1.0.6.6",
    "evt_language": "EN",
    "evt_timezone": "Europe/Brussels",
    "total_storage_size": "32768 MB",
    "available_storage_size": "32768 MB",
    "evt_time": "2025-10-8 15:57:30"
  }
}
```

---

## 9. Recording Database (record.db)

**26 meetings** recorded. 4 tables:

- `meeting_projects` — 26 rows (meeting metadata)
- `project_files` — Audio files per meeting
- `project_marks` — Bookmarks/timestamps
- `speaker_mappings` — Speaker diarization labels

Sample:
```
local_meeting_id                    | name                  | lang | duration_ms | upload
20251008_160153_147_PSPLB0000AAAAA  | 160153_1008           | 1    | 89         | 2 (done)
20251009_103417_547_PSPLB0000AAAAA  | 2025_10_09 10:34:17   | 1    | 786        | 2 (done)
20260318_100155_394_PSPLB0000AAAAA  | 2026_03_18 10:01:55   | 1    | 3809000    | 2 (done)
```

---

## 10. Firmware Versions (from /etc/sw-versions)

```json
{
    "SOC": "PSP_1.1.6.6",
    "MCU": "0.1.3.3",
    "BT": "0.0.0.28",
    "MODEL": "2.0.0.1",
    "BuildTime": "2025-1128-1949",
    "audioserver_ver": "20251027",
    "teasdk_ver": "3.10.1.1153",
    "asr_ver": "1027_v0.4.23"
}
```

---

## 11. Thermal Status

- CPU temperature: **57.8°C** (during active use)
- Board temperature: 50°C (from logs)
- Battery temperature: 37°C (from logs)

---

## 12. Overlay Changes (Persistent Modifications)

The `/data/overlay` directory shows what has been modified from the base SquashFS rootfs:

- **TEA SDK models** — All 15 models have been copied to the overlay (suggesting they were updated OTA)
- **Log files** — 20+ rotated log files in `/opt/logs/bak/`
- **No system config changes** — No modified init scripts or configs in the overlay

---

## 13. WiFi Credentials (from /tmp/wpa_supplicant.conf)

**Note: Redacted in this report. The live device stores WiFi passwords in plaintext.**

```
network={
    ssid="<REDACTED>"
    scan_ssid=1
    pairwise=CCMP TKIP
    group=CCMP TKIP WEP104 WEP40
    psk="<REDACTED>"
}
```

---

## 14. Key Corrections from Live Device

| Previous assumption | Actual (from device) |
|---|---|
| CPU: Cortex-A53 | **Cortex-A55** (part 0xd05) |
| Root hash: SHA-256 ($5$) | Live device shows **MD5 ($1$)** — different from firmware image |
| `/etc` is read-only | **Writable** via overlayfs from `/data/overlay/upper/etc` |
| NPU SRAM: unknown | **2 MB** at 0xF7000000 |
| NPU cores: unknown | **16 compute cores** |
| ASR runs on CPU | **ASR runs primarily on NPU** (`.nb` format) |
| Model partition: 3 GB | **4.6 GB** partition, 4.0 GB used |

---

## Files Extracted

All extracted to `docs/device-dump/`:

| File | Size | Content |
|---|---|---|
| `common_settings/` | 58 MB | All persistent settings, ASR data, finder DB |
| `psp.log` | 23 MB | Current application log |
| `finder_telemetry.db` | 48 KB | ByteDance telemetry events (118 events) |
| `record.db` | 168 KB | Meeting recording database (26 meetings) |

Plus `docs/ai-models/asr-models/model/` (4.1 GB, 173 files) extracted separately.
