---
title: First setup
description: Join Wi-Fi, use the setup page, edit config.toml, and turn on uploads, updates and the PIN lock.
---

After the first reboot the Wave shows **Set up Wave**, with two choices:

- **Use without Wi-Fi**: the Wave works as a USB speakerphone and records to
  itself right away. You can set up Wi-Fi later.
- **Set up Wi-Fi**: Wi-Fi is needed to send recordings to your own server and
  to get updates.

## Join Wi-Fi

Tap **Set up Wi-Fi**. Later, you find the same thing in the Menu: swipe down
from the top of Home, or tap the status at the top, then **Wi-Fi** and
**Set up Wi-Fi**. When the Wave is already connected the button reads
**Change network**, and it asks first, because the Wave leaves its network
while setup runs.

The Wave then opens its own temporary setup network and shows a QR code:

![The Wi-Fi setup QR code](../../../assets/screens/wifi_qr.png)

1. Scan the code with your phone's camera. It joins the setup network.
2. The setup page opens. If it doesn't, tap the **Sign in to network** prompt
   your phone shows.
3. Pick your network, enter its password, and save. The Wave restarts and
   connects.

No camera? Under the code the Wave shows the network name (`Wave-` and four
characters), its password and the address `192.168.2.1`. Join that network and
open `http://192.168.2.1/` in a browser.

The setup network uses WPA2 with a new random password each time, so your
Wi-Fi password never crosses the air in the clear. It closes after five
minutes if you don't save. The Wave stores your Wi-Fi password as a hashed
key, not as typed. More: [Wi-Fi](/insta360-custom-firmware/guides/wifi/).

## The setup page

The setup page is only there while the setup network is up. It has sections
for **Device**, **Audio**, **Webhook**, **Storage**, **Advanced** and
**Wi-Fi**:

- **Save and connect to selected network** saves everything and joins the
  network you picked.
- **Save settings and reconnect** saves everything and goes back to a saved
  network. Use it to change settings without changing networks.

Secrets (the webhook token, the PIN) are write-only: the page never shows a
saved value. Leave the field blank to keep it, or tick **Clear saved value**.
Invalid input keeps the page open with an error next to the field.

## `config.toml`

Every setting lives in `/data/waved/config.toml` on the Wave. The setup page
and the Wave's own screens write to it. You can also edit it over ADB.

Edit it in place, in a shell on the Wave:

```bash
adb shell
vi /data/waved/config.toml   # on the Wave; type exit when done
```

Or copy it to your computer and back:

```bash
adb pull /data/waved/config.toml
# edit config.toml
adb push config.toml /data/waved/config.toml
adb shell chmod 600 /data/waved/config.toml
```

Then restart waved so it reads the file:

```bash
adb shell killall waved
```

The start-up script restarts waved about 3 seconds after it exits.

Every section rejects keys it doesn't know. If the file doesn't load, waved
falls back to `config.toml.bak`, then to the defaults, and says why in
`/data/waved/waved.log`. When you change a setting on the Wave, waved re-reads
the file before it saves, so your edits stay.

| Key | Default | Meaning |
|---|---|---|
| `config_version` | `2` | Format version. Leave it. |
| `device.name` | `"Wave Open"` | Shown on the Wave and sent in webhook metadata. |
| `device.language` | `"en"` | Interface language. English is the only one so far. |
| `device.first_run_done` | `false` | Set once **Set up Wave** has been finished or skipped. |
| `device.mic_height` | `"medium"` | `"low"`, `"medium"` or `"high"`. waved also stores it in the MCU as the height the lift rises to at power-on. |
| `audio.mode` | `"omni"` | Pickup pattern: `"omni"` (Whole room), `"cardioid"` (Focused), `"supercardioid"` (Narrow), `"figure8"` (Two sides) or `"stereo"`. Pick Follow talker on the Wave, or as Steered on the setup page. |
| `audio.speaker_volume` | `100` | Speaker volume, 0 to 100. |
| `audio.mic_volume` | `200` | Microphone gain in percent, 0 to 800. 100 is unity. |
| `audio.nr_level` | `"off"` | Noise reduction: `"off"`, `"low"`, `"med"` or `"high"`. |
| `audio.beam_aim_deg` | `0.0` | Aim for Focused, Narrow and Two sides, in degrees clockwise from the top of the screen. |
| `display.brightness_pct` | `80` | Backlight, 5 to 100 percent. |
| `display.dim_after_s` | `120` | Seconds without a touch before the resting screen. `0` is never. |
| `display.off_after_s` | `600` | Seconds without a touch before the screen turns off. `0` is never. Never off while recording or in a call. |
| `time.zone` | `"UTC"` | IANA time zone name, for example `"Europe/Brussels"`. |
| `time.clock_24h` | `true` | Show 14:30 instead of 2:30 pm. |
| `recording.pre_recording` | `false` | Keep the last 30 seconds in memory so Record includes them. |
| `recording.format` | `"opus"` | `"opus"` (Ogg Opus, about 15 MB an hour) or `"flac"` (lossless, about 200 MB an hour). Applies to the next recording. |
| `recording.ask_timeout_s` | `60` | With `upload_mode = "ask"`, an unanswered recording is kept on the Wave after this many seconds (15 to 600). |
| `tones.enabled` | `true` | Sounds for mute, volume, and recording start and stop. |
| `api.endpoint` | `""` | Webhook URL for recordings. Empty keeps them on the Wave. |
| `api.auth_token` | `""` | Sent as `Authorization: Bearer …`. Empty sends no header. |
| `api.upload_mode` | `"automatic"` | `"automatic"`, `"ask"` or `"local_only"`. |
| `api.allow_insecure_http` | `false` | Allow a plain `http://` endpoint. For local testing only. |
| `api.retry_max` | `5` | Automatic attempts in one failure cycle. |
| `api.retry_backoff_ms` | `1000` | First delay before a retry. |
| `api.retry_max_backoff_ms` | `60000` | Longest delay between retries. |
| `storage.prune_uploaded_at_percent` | `85` | Recordings volume usage, in percent, that starts removing already-uploaded recordings. |
| `storage.minimum_free_mb` | `2048` | Below this much free space, the oldest uploaded recordings are removed. |
| `security.device_password` | `""` | The device PIN, stored as a salted hash. Empty is no PIN. |
| `security.ota_server` | `""` | Base URL of your update server. Empty turns updates off. |
| `security.ota_allow_downgrade` | `false` | Let a signed package install an older or the same version. Only in this file, not on the setup page. |
| `ntp.servers` | `["pool.ntp.org", "time.google.com"]` | Time servers. |

## Send recordings to your server

Set **Webhook URL** on the setup page (`api.endpoint`), and a **Bearer token**
if your server wants one. Use `https://`; a plain `http://` URL also needs
**Allow insecure HTTP**.

For each recording it uploads, the Wave sends one `POST` with three parts: the
meeting ID, a JSON metadata file and the audio, mono at 48 kHz, in the
recording's format: Ogg Opus (`audio/ogg; codecs=opus`) by default, or FLAC
(`audio/flac`) if you chose it. Take the format from the part's
`Content-Type` rather than assume one.
The meeting ID is also the `Idempotency-Key` header, so your server can drop
repeats. When you save the setup page with a webhook URL set, the Wave
restarts and sends a small `webhook.test` request.

**After recording** picks what happens when you stop:

- **Automatic**: queue the upload at once.
- **Ask after recording**: the Wave asks **Send this recording?**. If nobody
  answers, it keeps the recording on the Wave after `ask_timeout_s` seconds.
- **Keep locally**: never upload.

Without a URL, recordings stay on the Wave in `/mnt/udisk/recordings`, and
automatic ones wait in the queue until you set one. **Menu → Uploads** shows
how many were sent, are waiting and failed, with a retry button when something
failed.

To try it, run the test receiver from `waved/` on a computer on the same
network, and set the URL to `http://<your computer's address>:8787/meetings`
with **Allow insecure HTTP** on:

```bash
python3 scripts/webhook_receiver.py --port 8787 --output ./webhook-inbox
```

The full request format: [Recording webhooks](/insta360-custom-firmware/guides/webhooks/).

## Updates

:::note
Over-the-air updates are host-tested only. No part of them has run on a Wave
yet.
:::

The Wave never asks Insta360 for updates. It installs signed packages from a
server you run, and only if:

- your build has your update key's public half compiled in
  (`WAVED_UPDATE_PUBKEY`, see [Install](/insta360-custom-firmware/start/install/#2-build)), and
- `security.ota_server` points at your server (setup page, **Advanced**,
  **OTA server**).

A build without the key refuses every update; its **System → Firmware update**
row reads Off. The Wave checks two minutes after it starts and then every six
hours. Tapping **Firmware update** checks at once.

Making a key, packing a release and serving it:
[Updates, owner's quick start](/insta360-custom-firmware/guides/updates/#owners-quick-start).

## PIN lock

:::note
The PIN lock is host-tested. It has not run on a Wave yet.
:::

Set a PIN of 4 to 6 digits on the setup page, under **Advanced**, **Device
PIN**. With a PIN set, the Wave asks for it before it opens Files, Wi-Fi,
Bluetooth, Uploads and System. It never asks for mute, volume, Record, audio
mode or noise reduction.

- After you enter it, the Wave stays unlocked until the screen rests.
- After five wrong tries, the keypad waits one minute.
- `config.toml` keeps a salted hash, never the digits.

Forgot the PIN? Set `device_password = ""` under `[security]` in
`config.toml` over ADB, then run `adb shell killall waved`.
