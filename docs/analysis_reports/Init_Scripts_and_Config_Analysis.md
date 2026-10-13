# Insta360 Wave Firmware v1.1.6.6 - Deep Analysis Report

**Firmware version:** PSP_1.1.6.6 (SOC), MCU 0.1.3.3, BT 0.0.0.28, MODEL 2.0.0.1
**Build date:** November 28, 2025
**Base OS:** Buildroot 2020.02.1 (Linux kernel 5.4.242)
**SoC:** Amlogic (likely A5/A113x2 family, based on code references)
**Internal codename:** "PSP" (referenced throughout as pspapp, psp_tool, etc.)

---

## 1. FULL BOOT SEQUENCE

The boot process is BusyBox init driven via `/etc/inittab`:

1. **Kernel init** -- mounts `/proc`, creates `/dev/shm` and `/dev/pts`, remounts `/` read-only, mounts all from `/etc/fstab`, sets hostname ("buildroot"), configures loopback, injects `nameserver 8.8.8.8` into `/etc/resolv.conf`

2. **Serial console** -- spawns a root shell on `ttyS0` (respawn, no login required -- see Security section)

3. **`/etc/init.d/rcS`** -- the master init script:
   - Sets LCD backlight to full brightness (255)
   - **S02overlayfs** -- Reads "deviceid" from Amlogic unifykeys to determine debug vs release mode. In debug mode, does a full pivot-root overlay (entire rootfs writable). In release mode, only overlays `/etc`, `/opt`, `/usr` with writable upperdir on `/data/overlay`. Mounts `/dev/data` (ext4), `/dev/model` (ext4 ro), `/dev/psp_udisk_s` (exfat), a `no_use` partition as key store at `/unifykey`
   - Launches **D-Bus session bus** and exports the address to `/tmp/dbus/`
   - Sets Qt framebuffer environment (`QT_QPA_PLATFORM=linuxfb:fb=/dev/fb0`)
   - Runs all S??* scripts in numerical order:

4. **S10mdev** -- starts mdev as device manager, coldplugs kernel modules

5. **S11rsyslogd** -- starts rsyslog, logging everything to `/opt/logs/psp.log`

6. **S30dbus** -- starts the D-Bus system bus daemon

7. **S45asound** -- initializes ALSA volume for two TAS5707 amplifier chips (A and B) to 150 (or reads stored volume from `/data/system_volume`); also sets permissions on `/usr/bin/tea_models`

8. **S50_ko_install** (run in background) -- the heavyweight init:
   - Loads `ddr_access_priority.ko`
   - Creates 800MB zram swap, enables with priority 10
   - Attaches Amlogic unifykeys
   - Sets `min_free_kbytes=12288`, `swappiness=10`
   - Starts **boot_animation** (raw RGB frames to framebuffer)
   - Loads `tea_license.ko` (Insta360's proprietary license enforcement kernel module)
   - Exports GPIOs 451, 453, 489, 490, 491 for WiFi power control
   - Loads `aml_wifi.ko` then `atbm613x_wifi_sdio.ko` (ATBM6031 WiFi SDIO)
   - Loads `galcore.ko` (Amlogic NPU driver) in background
   - WiFi install retries up to 5 times with GPIO power cycling; if all fail, reboots (up to 3 times tracked via `/data/common_settings/check_wifi_reboot_count`)
   - Mounts debugfs

9. **S60pspapp_daemon** -- starts `pspapp_daemon` and `pspapp_watchdog` (the main application supervisor)

10. **S89usbgadget** -- configures USB gadget via configfs:
    - Vendor ID: `0x18F0`, Product ID: `0x4e40`
    - Manufacturer: "insta360", Product: "psp"
    - Initializes UAC2 (USB Audio Class 2.0) at 48kHz/16-bit stereo, HID (Vendor+Telephone+Consumer composite descriptor), ADB, MSC, CDC-ACM
    - In normal mode: enables UAC + HID only
    - Creates named pipes `/tmp/hid_fifo_input` and `/tmp/hid_fifo_output`
    - ADB is initialized but NOT enabled by default (can be enabled via reconfiguration)

11. **S90_emmc_rcv** -- after 5-second delay, checks eMMC firmware revision and upgrades if below `0x1200000000000000` using `/usr/bin/emmc_tool`

12. **S99_PSPLauncher** -- reads hardware revision via ADC voltage on `iio:device0/in_voltage0_input`, writes to `/etc/hwrevision`

---

## 2. NETWORK SERVICES EXPOSED

### Active Services:

| Service | Port | Protocol | Notes |
|---------|------|----------|-------|
| **Telnet** | 23 | TCP | Via inetd, runs as root, no authentication beyond login |
| **Serial console** | ttyS0 | UART | Root shell, no password prompt |
| **Web UI (CGI)** | Unknown (likely 80) | HTTP | `/var/www/` with CGI scripts via `webshell_cli` |
| **wpa_supplicant** | D-Bus | local | WiFi management |
| **hostapd** | - | 802.11 | AP mode on wlan0/wlan1 |
| **dnsmasq** | 53,67 (DHCP) | UDP | Only in AP mode, subnet 192.168.2.0/24 |
| **ADB** | USB | - | Initialized but disabled in default mode |
| **USB Audio** | USB | UAC2 | 48kHz stereo capture+playback |
| **USB HID** | USB | - | Vendor+Telephone+Consumer composite |
| **gdbserver** | present | TCP | Binary exists but not auto-started |

### Web UI (CGI-bin endpoints):
- `/cgi-bin/wifi/wifi_tool.sh` -- WiFi setup
- `/cgi-bin/wifi/scan_list.sh` -- WiFi scan results
- `/cgi-bin/wifi/check_wifi.sh` -- WiFi status
- `/cgi-bin/record/record_ctl.sh` -- Audio recording control (start/stop/delete/play)
- `/cgi-bin/record/record_list.sh` -- List recordings
- `/cgi-bin/spotify/set_spotify.sh` -- Configure Spotify (librespot)
- `/cgi-bin/spotify/check_spotify.sh` -- Check Spotify status
- `/cgi-bin/swupdate/swupdate.sh` -- Trigger OTA update
- `/cgi-bin/swupdate/device_info.sh` -- Device info disclosure

---

## 3. SECURITY CONFIGURATION

### CRITICAL FINDINGS:

**Root shell on serial console with no authentication:**
`/etc/inittab` line 34: `ttyS0::respawn:-/bin/sh` -- an unrestricted root shell on serial, no login prompt.

**Telnet enabled by default (as root, no password check beyond shadow):**
`/etc/inetd.conf`: `telnet stream tcp nowait root telnetd telnetd -i`

**Root password is a SHA-256 hash (crackable):**
`/etc/shadow`: `root:$5$f9rdYTjzvJ$tiE3E0n0jM1eaEOAbP3mAGdiHS862RHGr3ae3VglmG/:::::::`
The `$5$` prefix indicates SHA-256-crypt. The salt is `f9rdYTjzvJ`. This is vulnerable to offline brute-force.

**No firewall:**
No iptables, nftables, or any firewall rules anywhere in the filesystem. Zero network access control.

**Hardcoded WiFi credentials:**
`/etc/wpa_supplicant.conf`:
```
network={
    ssid="insta360-guest"
    psk="insta360"
}
```

**Core dump collection enabled globally:**
`/etc/profile` line 71: `ulimit -c unlimited` -- every crash produces an unlimited core dump, potentially leaking memory contents.

**SWUpdate public key is weak RSA-512:**
`/etc/swupdate-public.pem` is a 512-bit RSA key (the `MFww...` header and 64-byte modulus confirm this). RSA-512 has been factorable since 1999 and can be broken in hours on commodity hardware. This means **firmware update signing can be bypassed** -- an attacker could forge signed firmware updates.

**CGI scripts run as root with no input validation:**
The CGI scripts in `/var/www/cgi-bin/` directly pass user input to shell commands. For example, `set_spotify.sh` reads username/password from a file and passes them to `librespot` without sanitization.

**Debug mode accessible via unifykey:**
If the "deviceid" unifykey is set to "debug", the entire rootfs becomes writable via full pivot-root overlay.

### User Accounts:
- `root` (uid 0) -- shell `/bin/sh`, password set (SHA-256 hash)
- `daemon`, `bin`, `sys`, `sync`, `mail`, `www-data`, `operator`, `nobody` -- all `/bin/false`
- `dbus` (uid 1000) -- `/bin/false`

### Permissions:
- umask: 022
- `/dev/random`, `/dev/urandom`: 444 (read-only, which is correct)
- Input devices: 640
- `/dev/ttyS*`: 660

---

## 4. AUDIO PIPELINE CONFIGURATION

### Hardware:
- **Two TAS5707 amplifier chips** (referred to as "5707_A" and "5707_B") with volume range 0-511
- **Amlogic SoC audio** with hardware codec at card 0
- **10-channel microphone array** (channel map config references mic indices 0-9)
- **NPU** (galcore.ko) for AI audio processing

### ALSA Configuration (`/etc/asound.conf`):
- Default PCM: `hw:0,1` (hardware playback)
- Microphone input: `hw:0,2`
- AVS input: `hw:0,4`
- `dmix` output mixer at 48kHz/16-bit stereo on `hw:1,0,0`
- 2-to-8 channel upmixing via route plugin with specific gain coefficients
- Voice prompt output as mono-to-stereo with equal gain
- Rate conversion using SpeexRate medium quality

### Audio Processing:
- **Dynamic EQ** (`/etc/dynamic_eq_config.json`): IIR EQ with 3 bands (150Hz, 8kHz, 90Hz), multi-band AGC with 4-band crossover at 300/4000/7000 Hz
- **Alternative EQ** (`/etc/dynamic_eq_config_2.json`): 4-band EQ at 150/800/400/3500 Hz
- **Reverb** (`/etc/reverb_config.json`): RT60=1.0s, room distance=0.2, gain=0.85, reverb rate=0.5
- **Hardware EQ/DRC** (`/etc/eq_drc_params.ini`): Amlogic SoC hardware EQ (16 bands), full-band DRC (3 knees), multi-band DRC (3 bands, crossover at 150Hz and 5000Hz), noise gate at -70dB
- **External amplifier** (`/etc/EXT_AMP.ini`): master volume 230/511

### AI/ML Audio Models:
15 neural network models in `/usr/bin/tea_models/`:
- Beamforming: Cardioid, Supercardioid, Figure-8 patterns (each with high/low/VAD rotate variants)
- `insta360_nnpost_solo_steedy.bin` -- solo speaker steady-state processing
- `insta360_psp_mchpre_chimera_model.bin` -- multi-channel pre-processing
- `insta360_psp_pre_chimera_model.bin` -- pre-processing
- `nnec_two_stage_32k.bin` -- neural network echo cancellation (32kHz)
- `premask_reverb_best_model.bin` -- reverb masking
- `rafa-best_model.bin` -- (unknown purpose, possibly noise reduction)

### Beamforming Modes (from UI strings):
- **Omni** -- all directions
- **Cardioid** -- front pickup
- **Supercardioid** -- narrow front pickup
- **Figure-8** -- front+back
- **Stereo** -- L/R channels (USB input only)
- **Noise Reduction** mode

---

## 5. WIFI AP MODE CONFIGURATION

`/etc/hostapd.conf` (template -- SSID "test" is a placeholder):
- Interface: `wlan0`
- Mode: 802.11g (2.4 GHz)
- Channel: 1
- Max stations: 255
- WMM enabled
- Auth algorithms: 3 (both Open and Shared Key)
- MAC ACL: accept all (macaddr_acl=0)
- SSID not hidden (ignore_broadcast_ssid=0)

When AP mode activates (via `hostapd_conf_pre`):
- Uses a temp config `/etc/hostapd_temp.conf` (not present in base rootfs, likely generated at runtime)
- AP IP: `192.168.2.1`
- DHCP range: `192.168.2.50` to `192.168.2.200`, 12-hour lease via dnsmasq on port 100

WiFi chip: **ATBM6031** (atbm613x_wifi_sdio.ko), with Amlogic WiFi wrapper (aml_wifi.ko).

---

## 6. SERVICES THAT RUN AUTOMATICALLY

| Service | Description |
|---------|-------------|
| `mdev` | Device manager |
| `rsyslogd` | System logging to `/opt/logs/psp.log` |
| `dbus-daemon` | System + session bus |
| `pspapp_daemon` | Main application daemon |
| `pspapp_watchdog` | Watchdog for main app |
| `boot_animation` | Boot splash (raw RGB frames to framebuffer) |
| `inetd` | Runs telnetd |
| USB gadget | UAC2 + HID composite |
| `logrotate` | Log rotation via cron (`/etc/cron.d/e2scrub_all`) |
| `remotecfg` | IR remote configuration |

---

## 7. HARDCODED CREDENTIALS AND SECRETS

| Item | Location | Value |
|------|----------|-------|
| WiFi PSK | `/etc/wpa_supplicant.conf` | SSID: `insta360-guest`, PSK: `insta360` |
| Root password hash | `/etc/shadow` | `$5$f9rdYTjzvJ$tiE3E0n0jM1eaEOAbP3mAGdiHS862RHGr3ae3VglmG/` |
| USB vendor/product | `S89usbgadget` | VID `0x18F0`, PID `0x4e40` |
| Fallback serial | `S89usbgadget` | `0123456789ABCDEF` (if cpuinfo serial unavailable) |
| DNS server | `/etc/inittab` | `8.8.8.8` (hardcoded) |
| Chinese DNS | `wifi_tool.sh` | `114.114.114.114` (used as ping fallback) |
| SWUpdate signing key | `/etc/swupdate-public.pem` | 512-bit RSA public key (trivially breakable) |
| Entropy seed | `/etc/entropy.bin` | Static entropy file for hostapd |

---

## 8. TELEMETRY / PHONE-HOME BEHAVIOR

**No explicit phone-home found in the shell scripts and configs.** However:

- The UI strings reference **"sync to cloud"** for AI recordings: "Files have been stored locally and will automatically sync to cloud when connected to the network" -- indicating the main `pspapp` binary (not analyzed here as it's a compiled binary) likely connects to Insta360 cloud services.
- **Auto-download firmware updates**: "When connected to Wi-Fi, the device will automatically download software updates during idle time" -- the `swupdate` system can fetch OTA updates from a URL written via `urlmisc write`.
- The `swupdate-url.sh` script pings an OTA server and writes the URL persistently.
- DNS is hardcoded to `8.8.8.8` (Google), with `114.114.114.114` (Chinese DNS) as a connectivity test fallback.
- No explicit analytics endpoints visible in config files; any telemetry would be in the compiled binaries (`pspapp`, `audioserver`).

---

## 9. INTERESTING / UNUSUAL CONFIGURATIONS

1. **Spotify Connect built-in**: The device has `librespot` (open-source Spotify client) support with a CGI endpoint to configure credentials. Credentials are stored in plaintext in `/var/www/cgi-bin/spotify/spotify.conf`.

2. **Proprietary kernel module `tea_license.ko`**: Loaded early in boot from `/lib/modules/5.4.242/kernel/insta360/`. This is a license enforcement mechanism for the TEA SDK (v3.10.1.1153) audio AI models.

3. **ASR (speech recognition)**: Version `1027_v0.4.23` noted in `sw-versions`, suggesting on-device voice recognition.

4. **800MB zram swap**: Aggressive memory management for an embedded device, suggesting the applications are memory-hungry (likely due to NPU inference workloads).

5. **OP-TEE**: Kernel modules `optee` and `optee_armtz` are loaded, indicating TrustZone/secure world usage (likely for key storage in the "unifykeys" system).

6. **Unifykey system**: Amlogic's key storage in eMMC RPMB (Replay Protected Memory Block), used for device ID, debug mode flag, and potentially DRM keys.

7. **Motorized mechanism**: The UI strings reference "Elevation obstructed. Descending again to shut down" and "Abnormal extension or retraction" -- the device has a motorized extending/retracting speaker column.

8. **USB HID telephony**: The active HID descriptor is a Vendor+Telephone+Consumer composite, enabling the device to function as a USB speakerphone with call control buttons (hook switch, off-hook, mute, ring indicators).

9. **Record CGI with no auth**: The recording endpoint allows anyone on the network to start/stop audio recording, list files, and play them back. Combined with no firewall, this is a remote eavesdropping vector.

10. **MTP support**: The device can present as a USB mass storage/MTP device, exposing `/mnt/udisk`.

11. **`/etc/entropy.bin`**: A static binary entropy file shipped with the firmware, used by hostapd. If this is the same across all devices, it weakens WPA key generation.

12. **Board name**: `AmlSpeaker` (`/etc/board_info`), confirming this is built on Amlogic's reference speaker platform.

13. **20 supported languages**: Full LVGL-based touchscreen UI with translations for Chinese, English, French, German, Spanish, Dutch, Italian, Polish, Portuguese, Russian, Japanese, Korean, Traditional Chinese, Arabic, Hindi, Indonesian, Malay, Thai, Vietnamese, and Turkish.

---

## Summary of Key Security Concerns

1. **Root shell on UART with zero authentication**
2. **Telnet service running as root (network-accessible)**
3. **No firewall at all**
4. **RSA-512 firmware signing key (trivially forgeable)**
5. **Hardcoded WiFi credentials**
6. **CGI scripts with no authentication or input sanitization**
7. **Unlimited core dumps enabled system-wide**
8. **Static entropy file for hostapd across devices**
9. **Remote audio recording control with no auth (potential eavesdropping vector)**
10. **Spotify credentials stored in plaintext**