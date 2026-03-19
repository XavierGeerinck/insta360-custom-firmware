# Insta360 Wave — Security Questions & Answers

Based on deep firmware analysis of v1.1.6.6 (decompilation of 15,597 functions, full config/script review).

---

## 1. Does it send data to China?

**Yes.** Multiple channels send data to Chinese infrastructure:

### ByteDance Telemetry (always active)

The device embeds ByteDance's "Rangers AppLog" DataFinder SDK (`libapplogrs.so`, Rust-based):

| Property | Value |
|---|---|
| **Endpoint** | `https://snssdk.insta360.com` |
| **App ID** | `10000022` (hardcoded) |
| **User ID** | Device serial number |
| **Open UDID** | Derived from device MAC address |
| **SDK DB** | `/data/common_settings/finder/finder_sdk_dir` (SQLite) |

This SDK sends **35+ event types** regardless of which region you're in — every boot, shutdown, recording start/stop, mute toggle, file operation, Bluetooth connection, physical height adjustment, and more. ByteDance is a Chinese company (parent of TikTok). The data flows through `snssdk.insta360.com` which resolves to ByteDance-operated infrastructure.

### Meeting Data Geo-Routing

From decompiled `libins_entry.so` (line 117173), meeting audio, transcriptions, and speaker voiceprints are routed by region:

```c
if (country == "CN")
    url = "https://openapi-hz.insta360.com/summary-service"   // Hangzhou, China
else if (country == "US")
    url = "https://openapi-va.insta360.com/summary-service"   // Virginia, US
else
    url = "https://openapi-fra.insta360.com/summary-service"  // Frankfurt, EU
```

If your region is set to CN, your meeting recordings, ASR transcriptions, and speaker voiceprints go directly to servers in Hangzhou, China. For EU users, data goes to Frankfurt. For US users, Virginia.

### Other Chinese Infrastructure

| Service | Address | Usage |
|---|---|---|
| **Chinese DNS** | `114.114.114.114` | Hardcoded connectivity check fallback in `wifi_tool.sh` |
| **Chinese NTP** | `ntp.ntsc.ac.cn` | Time synchronization server |
| **Chinese NTP** | `ntp2.cstnet.cn` | Time synchronization server |
| **Primary DNS** | `8.8.8.8` | Google DNS, hardcoded in `/etc/inittab` |

### What Exactly Is Sent?

**Per telemetry event** (every event includes): `mic_serial_number`, `fw_ver`, `evt_language`, `evt_timezone`, `evt_time`

**Full event list:**

| Event | Trigger | Extra Data |
|---|---|---|
| `device_power_on` | Every boot | Storage sizes, language, timezone |
| `device_power_off` | Every shutdown | Battery level, power_off_type |
| `system_crash` | App crash | Crash details |
| `ai_record_start` | Recording starts | Transcription mode, language |
| `ai_record_end` | Recording ends | Duration, file size |
| `ai_record_pause` | Recording paused | - |
| `ai_record_mark` | Bookmark created | Record ID |
| `device_network_connect` | WiFi connects | Connection details |
| `device_network_disconnect` | WiFi drops | - |
| `bluetooth_connect` | BT pairs | BT details |
| `toggle_mic_mute` | Mic muted/unmuted | Mute state |
| `toggle_speaker_mute` | Speaker muted/unmuted | Mute state |
| `call_answer` | Phone call answered | - |
| `call_end` | Phone call ended | - |
| `open_record_file` | File opened | File details |
| `listen_file` | File played | File details |
| `delete_file` | File deleted | File details |
| `upload_cloud_file` | File uploaded | - |
| `firmware_download` | OTA download | Details |
| `firmware_install` | OTA install | Details |
| `password_lock_toggle` | Password toggled | State |
| `mechanical_height_adjust` | Motor moves | - |
| `enter_usb_storage_mode` | USB mode | - |
| `reverse_charging_start/end` | Charging other device | Details |
| `transcription_mode_set` | ASR mode changed | Mode |
| `transcription_language_set` | ASR language changed | Language |
| `bind_id` / `unbind_id` | Account linked/unlinked | - |
| `privacy_agreement` | Privacy terms accepted | - |
| `join_user_experience` | UX program opted in | - |
| `factory_reset` | Factory reset | - |
| `device_activation` | First setup | - |
| `init_start` / `init_end` | Boot timing | Duration |
| `check_update` | OTA check | - |
| `prompt_tone_setting` | Alert tone changed | Settings |
| `set_language_region` | Language changed | Language details |

**Per meeting upload** (to Insta360 cloud):
- Full audio recording (`audio_media`)
- Playback audio (`audio_play`)
- Screenshots (`screen_shot`)
- On-device ASR results (`device_asr`)
- Meeting metadata (`meeting_info`)
- Speaker voiceprint/diarization data

### Bottom Line

Even if your meeting data routes to EU/US servers, the ByteDance telemetry SDK **always runs** and tracks virtually every interaction with the device. There is no opt-out mechanism visible in the firmware (the `system_join_improvement` flag controls a "User Experience Improvement Program" but the base telemetry SDK appears to run regardless).

---

## 2. Telnet — How to access? What's the password?

### How to Connect

The device runs a telnet daemon via `inetd` as root:

```
# From /etc/inetd.conf:
telnet stream tcp nowait root telnetd telnetd -i
```

Connect from any machine on the same network:

```bash
telnet <device-ip> 23
```

The device IP is visible on its touchscreen: **Settings → Wi-Fi → (tap connected network) → IP Address**. If the device is in AP mode, the IP is `192.168.2.1`.

### Password

Login is `root`. The password hash from `/etc/shadow`:

**v1.1.6.6:**
```
root:$5$f9rdYTjzvJ$tiE3E0n0jM1eaEOAbP3mAGdiHS862RHGr3ae3VglmG/:::::::
```

**v1.0.13.6:**
```
root:$5$6PnLbyshVm.$Q62hB3TlLFnlpuSWJooP6xINNc7vciuGVuORjx673zD:::::::
```

Both are SHA-256-crypt (`$5$`). **Both cracked via hashcat brute-force in under 3 minutes:**

```
Password: 123456
```

Yes — both firmware versions use `123456` as the root password. Hashcat mode 7400 (sha256crypt), 6-digit numeric brute-force, cracked at 237 kH/s on Apple Silicon.

### Full Access Procedure

```bash
# 1. Find device IP (check router DHCP list, or on touchscreen:
#    Settings → Wi-Fi → tap network → IP Address)

# 2. Connect
telnet <device-ip>

# 3. Login
# Login: root
# Password: 123456

# You now have a full root shell.
```

### No Firewall

There are **zero firewall rules** in the entire firmware — no iptables, no nftables, nothing. Any device on the same network can reach telnet on port 23.

---

## 3. What is that CGI web interface?

### Overview

The device contains a full web interface at `/var/www/` served by the `webshell_cli` binary (or potentially the main `pspapp` application). It uses **Bootstrap CSS** and **jQuery** and is titled **"Amlogic setting"** — revealing it's based on the stock Amlogic reference platform web UI.

The web server has **no authentication whatsoever**. Anyone on the same network can access all endpoints.

### How to Access

```
http://<device-ip>/
```

### Pages

| Page | URL | Purpose |
|---|---|---|
| **Home** | `/index.html` | Navigation hub with Amlogic branding |
| **WiFi** | `/wifilist.html` | Scan networks, connect, configure WiFi |
| **Spotify** | `/spotify.html` | Configure Spotify Connect credentials |
| **SWUpdate** | `/system.html` → `/swupdate/index.html` | Drag-and-drop firmware upload |
| **Record** | `/record.html` | Start/stop/play/delete audio recordings |
| **Download** | `/dl_record_file.html` | Download recorded files |

### CGI Endpoints (Shell Scripts, Running as Root)

#### Recording Control — `/cgi-bin/record/record_ctl.sh`

```bash
#!/bin/sh
PATH=/bin:/sbin:/usr/bin:/usr/sbin
FILENAME=$2
RATE=$3
CHANNEL=$4
BITS=$5
case $1 in
    start)
    arecord -Dhw:0,3 -f $BITS -r $RATE -c $CHANNEL ../record/$FILENAME.wav &
    ;;
    done)
    killall -9 arecord
    ;;
    delete)
    rm ../record/$FILENAME
    ;;
    play)
    aplay -D hw:0,2 ../record/$FILENAME
    ;;
esac
```

**This is a remote eavesdropping vulnerability.** Anyone on the network can:
- **Start recording** the room's audio from all 8 microphones
- **Stop recording**
- **Play back** recordings through the speaker
- **Delete** recordings to cover tracks
- **Command injection**: `$FILENAME`, `$BITS`, `$RATE`, `$CHANNEL` are passed directly to shell commands without any sanitization. An attacker could inject arbitrary commands.

#### Recording List — `/cgi-bin/record/record_list.sh`

Lists all recorded audio files.

#### WiFi Configuration — `/cgi-bin/wifi/wifi_tool.sh`

Configures WiFi connections. Calls `wpa_supplicant` with user-supplied SSID/password.

#### WiFi Scan — `/cgi-bin/wifi/scan_list.sh`

Returns a list of available WiFi networks.

#### WiFi Status — `/cgi-bin/wifi/check_wifi.sh`

Checks current WiFi connection status.

#### Spotify Setup — `/cgi-bin/spotify/set_spotify.sh`

Configures Spotify Connect (librespot) with username and password. Credentials stored in **plaintext** at `/var/www/cgi-bin/spotify/spotify.conf`.

#### Spotify Status — `/cgi-bin/spotify/check_spotify.sh`

Checks if librespot is running.

#### Device Info — `/cgi-bin/swupdate/device_info.sh`

Discloses: kernel version, WiFi status, system architecture. Writes to `/swupdate/device_info.txt`.

#### Firmware Update — `/cgi-bin/swupdate/swupdate.sh`

Triggers SWUpdate firmware installation. The `/swupdate/index.html` page provides a drag-and-drop UI for uploading `.swu` firmware files.

### SWUpdate Web UI

The SWUpdate sub-site at `/swupdate/` is a full web-based firmware flasher with:
- Drag-and-drop `.swu` upload (via dropzone.js)
- Progress bar
- Status display
- Bootstrap + Font Awesome styling

This means anyone on the network can **upload and flash arbitrary firmware** onto the device.

### Security Assessment

| Vulnerability | Severity | Details |
|---|---|---|
| **No authentication** | Critical | All endpoints are world-accessible |
| **Remote audio recording** | Critical | Start/stop microphone recording remotely |
| **Command injection** | Critical | Unsanitized CGI parameters passed to shell |
| **Remote firmware flash** | Critical | Upload and install arbitrary firmware |
| **Credential disclosure** | High | Spotify credentials in plaintext |
| **Device info leak** | Medium | Kernel version, WiFi config exposed |
| **No HTTPS** | Medium | All traffic in cleartext |

### Note on Availability

The `webshell_cli` auto-start is **commented out** in `S99_PSPLauncher`:

```bash
chmod +x /usr/bin/webshell_cli
#/usr/bin/webshell_cli &
#echo "webshell start"
```

However, the web files and CGI scripts are fully present on the filesystem. The web server may be activated:
- By the main `pspapp` application in certain modes
- Via the debug screen (PAGE_DEBUG, ID 88)
- By manually starting it after gaining shell access
- It may have been enabled in earlier firmware versions and simply commented out

---

## 4. What is that serial console?

### What It Is

A **UART (Universal Asynchronous Receiver-Transmitter) serial console** that provides a direct root shell with **zero authentication** — no login prompt, no password, no username. Just an immediate `#` root prompt.

From `/etc/inittab`:
```
ttyS0::respawn:-/bin/sh
```

This means: on serial port `ttyS0`, respawn (restart if it dies) a `/bin/sh` shell. The `-` prefix makes it a login shell. There is **no `getty`** or **`login`** program in between — it's a raw shell.

### How to Access

#### Hardware Required
- USB-to-UART adapter (FTDI FT232, CP2102, CH340, or similar) — **must be 3.3V logic level**
- 3 jumper wires (TX, RX, GND)
- Soldering iron (if headers aren't populated)

#### Steps

1. **Open the device** — remove the bottom cover (screws or clips)

2. **Locate the UART pins** on the PCB. On Amlogic boards, look for:
   - A 3-pin or 4-pin unpopulated header
   - Labeled TX, RX, GND (or J1, UART, DEBUG)
   - Often near the SoC or at the board edge
   - The Amlogic A5/AV400 reference design typically has UART on specific GPIO pins

3. **Connect the USB-to-UART adapter:**
   ```
   Device GND  →  Adapter GND
   Device TX   →  Adapter RX
   Device RX   →  Adapter TX
   (Do NOT connect VCC — power the device normally)
   ```

4. **Open a serial terminal** at **115200 baud** (standard for Amlogic):
   ```bash
   # macOS
   screen /dev/tty.usbserial-* 115200

   # Linux
   screen /dev/ttyUSB0 115200

   # Or use minicom
   minicom -D /dev/ttyUSB0 -b 115200

   # Or use picocom
   picocom -b 115200 /dev/ttyUSB0
   ```

5. **Power on the device** — you'll see:
   - U-Boot bootloader messages
   - Linux kernel boot log
   - All init script output
   - Then a **direct root shell prompt: `#`**

### What You Can Do

With root shell access via UART, you have complete control:

```bash
# Read all files on the device
cat /etc/shadow                    # Password hashes
cat /etc/wpa_supplicant.conf       # Saved WiFi passwords

# Access the secure key storage
cat /sys/class/unifykeys/read      # Device secrets, encryption keys

# Dump the full eMMC
dd if=/dev/mmcblk0 of=/mnt/udisk/emmc_dump.bin

# Modify the running system
mount -o remount,rw /              # Make rootfs writable
vi /etc/init.d/S60pspapp_daemon    # Modify startup scripts

# Enable ADB over USB
echo "adb" > /sys/kernel/config/usb_gadget/amlogic/configs/b.1/strings/0x409/configuration

# Start/stop services
killall pspapp                     # Kill the main application
/usr/bin/webshell_cli &            # Start the web interface

# Access recordings
ls /mnt/udisk/AIRecord/            # All meeting recordings

# Network sniffing
tcpdump -i wlan0 -w /mnt/udisk/capture.pcap

# Install custom software
# (if rootfs is made writable via overlayfs or remount)
```

### Boot Log Visibility

The serial console also shows the full boot sequence, which reveals:
- U-Boot version and configuration
- Kernel command line parameters
- All kernel module loading
- WiFi initialization (including retry/failure details)
- Application startup logs
- Any crash dumps or error messages

### Security Implications

| Risk | Impact |
|---|---|
| **Full root access** | Complete device compromise |
| **No authentication** | Anyone with physical access + $5 UART adapter |
| **Persistent** | Survives reboots (respawn in inittab) |
| **Key extraction** | Amlogic unifykeys readable (device secrets, encryption keys) |
| **Firmware modification** | Can write to eMMC, install backdoors |
| **Data exfiltration** | Read all recordings, WiFi passwords, cloud tokens |

### Comparison with Other Devices

Most consumer IoT devices have a UART console for development/debugging, but typically:
- It's disabled in production firmware (this one is **enabled**)
- It requires login credentials (this one has **none**)
- It runs a limited shell (this one runs **full root /bin/sh**)

---

## Summary Table

| Question | Short Answer | Severity |
|---|---|---|
| **Sends data to China?** | Yes — ByteDance telemetry always runs, meeting data to CN if region=CN | High |
| **Telnet access?** | Port 23, login `root`, SHA-256 password (not yet cracked), no firewall | Critical |
| **CGI web interface?** | Unauthenticated web UI for WiFi, recording, firmware flash, Spotify | Critical |
| **Serial console?** | UART root shell, zero authentication, 115200 baud on ttyS0 | Critical (physical) |

---

*All findings based on static firmware analysis. No active exploitation was performed against a live device.*
