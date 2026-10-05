# Wave display redesign, revision 2

Open [`index.html`](index.html) in a browser for the full spec. It shows every screen at 1:1
framebuffer scale, has a "Show layout zones" overlay, and lists the open decisions. PNG
renders are in [`png/`](png/).

Every mockup is positioned in **real framebuffer pixels**: 400 × 304, LVGL logical
coordinates are what the user sees (see `waved/docs/ui/COORDINATE_SYSTEM.md`). The
`left/top/width/height` values in `index.html` can be used directly in `lv_obj_set_pos` /
`lv_obj_set_size`.

| | | |
|---|---|---|
| ![Home](png/home.png) | ![Recording](png/recording.png) | ![Audio mode](png/audio-mode.png) |
| Home: tap the dial to mute | Recording: same layout, Stop takes the Record spot | Audio mode: six modes around a live preview |
| ![After stop](png/decision.png) | ![Menu](png/menu.png) | ![Files](png/files.png) |
| After stop: Send, Keep, Delete | Menu: opens from the status pill | Files: attention first |

## What changed since revision 1

| Change | Why |
|---|---|
| Tapping the dial mutes (Home, Recording) | Mute is the most frequent action in a meeting, so it gets the biggest target, and the ring turning red is visible from every seat. |
| Quieter / Louder on the sides of Home and Recording | Volume becomes one tap instead of a trip to another screen. The dial shows the level while it changes. |
| Menu opens from the status pill at the top | The old "Menu" handle sat 4 px under Record, where a slip started a recording. |
| Mark, Pause and Stop together at the bottom; Stop needs a 1 s hold | Stopping can't be undone. A short tap explains the hold instead of doing nothing. |
| "Send this recording?" with a 60 s auto-Keep | The old screen waited forever. Keep is the safe default: nothing leaves the device and nothing is deleted. |
| Audio modes as a radial picker with drag-to-aim | All options are visible at once. Directional modes get a direction you choose, which today's code can't express from the UI. |
| One Back button, 112 × 44 px at (144, 244) | Revision 1 still had two Back styles and a 36 px target. |
| Round bottom actions (5 October) | The panel was measured: the visible circle sits 46 px higher than assumed. Bottom actions are round buttons on an arc that follows the edge, in the same language as the dial and the side buttons. Record and Stop are one button in one spot. |
| New screens | Start-up, first start, resting screen, recording muted, holding Stop, Mic height, Bluetooth, After a recording, Display, Date and time, PIN. |
| PIN for Files and admin settings | Files, Wi-Fi, Bluetooth, Uploads and System ask for the PIN. Mute, volume, Record, audio mode and noise reduction never do. |

## Layout system

The visible panel, measured on the device (`waved/docs/DISPLAY_HARDWARE.md`, "Visible
Area"), is a **circle of r ≈ 198 centred at (200, 106)**, cut off flat at the top by
framebuffer row 0 where the stand's notch covers the glass. It is 335 px wide at y = 0,
almost the full 400 px at y = 106 (the widest row) and narrows quickly below: 325 px at
y = 220, 147 px at y = 290. The area-weighted centre of the visible shape is about y = 137.

The device frame in `index.html` draws that shape: the bezel circle is centred on the visible
circle, the strip above framebuffer row 0 is the notch, and `.fb` is clipped with
`circle(198px at 200px 106px)` (which, intersected with the 400 × 304 box, gives the flat top).

| Zone | y range | Contents |
|---|---|---|
| Status | 6–34 | Status cluster or status pill (connection, time, battery), 240 px wide on the flat top edge. Messages replace it for a few seconds. |
| Title | 38–62 | Sub-screen title, 20 px semibold. Not used on Home and Recording. |
| Core | 50–236 | Screen content. On Home and Recording: the dial, centre (200,121), r = 70. |
| Sides | centre y = 121 | Volume on Home and Recording: 60 px circles centred at x = 56 and x = 344, with a 12 px label below. |
| Base | 181–293 | Bottom actions on the r 135 arc: the main round button at (200,241) and up to two companions at (113,209) / (287,209); or a bottom pill; or the Back pill at y 244–288. |

### Dial geometry

The dial is a ring of **r 70 centred at (200,121)**, with a tap circle of r 71 (its outer edge,
y 50–192). It sits between the status pill (bottom y 34) and the main bottom button (top y 209),
with the same clear space on both sides: 15.5 px from the ring to the status pill and 16.5 px
to the main button. The companions are 24 px from the ring and 32 px from the main button. The
side buttons are 42 px from the ring. The status pill's 44 px hit area ends at y 50, where the
dial's tap circle begins.

Revision 2 used r 78, which left the ring nearly touching everything around it. r 66 was tried
first, but the 48 px timer is 119 px wide, so inside r 66 it came within about 4 px of the ring
on Recording. r 70 is the smallest ring that keeps the type scale and still leaves the timer
about 7 px of air on each side, with the digits centred on the ring's widest row. The talker arc
keeps its 44° span at stroke 6 (was 7), and the talker dot sits just outside the ring, on r 82.
The volume gauge (Home), the update progress ring, the start-up ring and the resting-screen ring
all use the same r 70 at (200,121). The call ripples are r 76/82/88 around (200,118), because
the phone number is 129 px wide.

Rules:

1. **Tap limit r = 190 px around (200,106), and y ≥ 6.** No tappable pixel is farther than
   190 px from (200,106) (about 8 px inside the curve). The flat top edge can be used, 6 px
   in from row 0.
2. **Targets ≥ 44 px, main actions ≥ 50 px.** Short bars and icons get invisible full-size hit areas.
3. **Bottom actions are round.** See "Bottom actions" below.
4. **Same place, same job.** Quieter left, Louder right, mute in the dial, the main action in
   the bottom-centre round button (Record becomes Stop there), menu at the top, Back bottom centre.
5. **Messages replace the status pill.** One line, ≤ 240 px, never clickable, never over a button.
6. **Colour carries meaning only:**
   - teal `#3fd6b1`: live, selected or healthy
   - red `#ff4b3e`: recording, muted or destructive
   - amber `#ffb547`: paused or needs attention
   - everything else is grey on `#000`
7. **Undo-proof actions are isolated.** Stop needs a hold. Delete, Factory reset, Power off
   and Stop USB storage go through the confirmation dialog.

### Bottom actions

The bottom of the screen speaks the same round language as the dial and the side buttons. There
are no bars, slabs or half-moons.

**Geometry.** Bottom buttons sit on an arc of **r 135 around the panel centre (200,106)**, so
they follow the edge of the visible circle. All of them stay within r 167 of that centre, well
inside the r 190 tap limit.

| Spot | Circle | Centre | Label (micro 12 px, 4–6 px below) |
|---|---|---|---|
| Main | 64 px | (200, 241), the bottom of the arc | 90 wide, top 279 |
| Companion left / right | 56 px | (113, 209) / (287, 209), ±40° on the arc | 90 wide, top 241 |
| Pair left / right | 64 px | (113, 209) / (287, 209), the companion spots | 90 wide, top 245 |

**Style.** Every bottom button is a Surface circle. Pressed is Raised with a 2 px teal ring, the
same as Quieter and Louder. Colour goes only on the icon (and the label where it carries the
meaning): the record dot is red, Stop is a red square with a red label, Send / Set up Wi-Fi /
Answer are teal, Delete / Decline are red, Resume is amber. Other labels are Text 2. Red is never
a button fill. It stays the state colour (recording and muted rings, the record dot).

**Record and Stop are one control.** Home's Record and Recording's Stop use the same Main spot
and size, so the button that starts a recording is the one that ends it. Stop's one-second hold
shows as a red progress ring (r 35, stroke 3, over a Line track) around the circle, while the
circle turns Raised.

**Which form per screen:**

| Screen | Bottom actions | Why |
|---|---|---|
| Home (all states) | Main: Record | The one action that isn't mute or volume. |
| Recording, muted, Holding Stop, Paused | Companions Mark / Pause (Resume) and Main Stop | The same three-button arc every time. Mark is dimmed while paused. |
| After stop | Companions Keep / Delete and Main Send | The same shape as Recording, so after Stop the choices appear in the same places. |
| First start | Main: Set up Wi-Fi; "Use without Wi-Fi" as a content pill above | One clear way forward. Skipping is quieter. |
| Incoming call | Pair: Decline (red) / Answer (teal) | Two equal choices with strong icons, mirrored around x 200. |
| Confirmation | Pill pair: Cancel / action, 124 × 50 at (70,196) / (206,196) | The words matter more than icons. Both pills are the same size, mirrored. |
| USB storage | Wide pill: "Stop USB storage", 184 × 52 centred on the Main spot (108,215) | A long label that no icon says on its own. |
| Sub-screens | Back pill at (144,244) | Unchanged. |

Content above the bottom buttons ends at y ≤ 166 where companions are used (their tops are at
y 181) and at y ≤ 193 otherwise (the main circle's top is at y 209).

## Interaction model

| Input | Where | Result |
|---|---|---|
| Tap the dial | Home, Recording | Toggle microphone mute |
| Tap the status pill, or swipe down from the top edge | Home, Recording | Open Menu |
| Tap Back, or swipe right | Any sub-screen | Go back one level |
| Tap Quieter / Louder | Home, Recording | Speaker −/+ 10 %, dial shows the gauge for 1.5 s |
| Hold Quieter / Louder | Home, Recording | Repeat the step every 300 ms |
| Short tap on Stop | Recording | Message "Hold Stop to end the recording" |
| Hold Stop for 1 s | Recording | Stop. Releasing early cancels. |
| Drag the pattern | Audio mode (Focused, Narrow, Two sides) | Aim the beam (`SetDoaOverride`) |
| First tap on the resting screen | Resting | Wake only, nothing else happens |
| Power key, short press | Anywhere | Screen to resting/off, or wake |
| Power key, hold 2 s | Anywhere | Power-off confirmation |

## Timings

| What | Value |
|---|---|
| Success and info messages | 2 s |
| Warning and error messages | 4 s |
| Volume gauge in the dial | 1.5 s after the last step |
| Hold to stop | 1 s |
| Resting screen ("Dim after") | 2 min without a touch, configurable |
| Screen off | 10 min without a touch, unless recording or in a call (then resting only) |
| "Send this recording?" auto-Keep | 60 s |
| Bluetooth pairing window | 2 min |

## Dial states

| State | Ring | Centre | Second line | Hint |
|---|---|---|---|---|
| Nothing connected | grey `#2a3639` | Mic on | Not connected | Tap to mute |
| Connected, mic not in use | grey | Mic on | Connected by USB / Bluetooth: *name* | Tap to mute |
| Computer is using the mic (`upstream_active`) | teal 40 % + talker arc | Mic on | Computer is using the mic | Tap to mute |
| In a call (HID off-hook) | teal 40 % + talker arc | Mic on | In a call | Tap to mute |
| Muted | solid red | Muted (mic-off icon) | Call can't hear you / Mic is off | Tap to unmute |
| Recording | red 35 % + red talker arc | timer | Speaker *n* talking | Tap to mute |
| Recording, muted | solid red | timer | Muted, not recorded | Tap to unmute |
| Paused | dashed amber | timer | Not recording | Tap to mute |
| Volume changing | gauge −135°…135°, teal fill | *n* % | — | — |

The talker arc is 44° wide (stroke 6, on the r 70 ring) and centred on the smoothed direction-of-arrival azimuth. It is only
drawn while voice activity is detected and audio is going somewhere (call, computer or recording).

## Tokens

```rust
// theme.rs — 0xRRGGBB, converted to RGB565 by rgb()
pub const CLR_BG: u32       = 0x000000;
pub const CLR_SURFACE: u32  = 0x141c1e; // buttons, focused rows, status pill
pub const CLR_RAISED: u32   = 0x1f292b; // messages, pressed state
pub const CLR_LINE: u32     = 0x2a3639; // idle ring, arc tracks, outlines
pub const CLR_TEXT: u32     = 0xf2f5f4;
pub const CLR_TEXT_2: u32   = 0x9aa7a6;
pub const CLR_TEXT_3: u32   = 0x5d6a6a;
pub const CLR_LIVE: u32     = 0x3fd6b1;
pub const CLR_ON_LIVE: u32  = 0x00140f; // text on teal fills
pub const CLR_REC: u32      = 0xff4b3e;
pub const CLR_WARN: u32     = 0xffb547;
```

These replace today's four greens and three reds. Also set `LV_THEME_DEFAULT_DARK 1`, or style
every object explicitly. Today the default light theme leaks white discs and 35 px padding
into unstyled widgets.

### Type

| Role | Font | Size / line height | Use |
|---|---|---|---|
| big | Barlow Medium, tabular figures | 48 / 52 | timer, clock, volume, update % |
| dial | Barlow Semi Condensed SemiBold | 26 / 30 | dial state, dialog titles |
| title | Barlow Semi Condensed SemiBold | 20 / 24 | screen titles, focused list row, mode names |
| body | Barlow Semi Condensed Medium | 15 / 18 | buttons, primary lines |
| small | Barlow Semi Condensed Medium | 13 / 16 | descriptions, status, messages |
| micro | Barlow Semi Condensed Medium | 12 / 14 | side-button labels, hints |

Both families are OFL. Generate them with `lv_font_conv` at 4 bpp:

```bash
npx lv_font_conv --font BarlowSemiCondensed-Medium.ttf --size 15 --bpp 4 --format lvgl \
  -r 0x20-0x7E,0xA0-0xFF,0x2013,0x2014,0x2018,0x2019,0x201C,0x201D,0x2022,0x2026,0x2212 \
  -o font_body_15.c
```

The range covers `…`, `·`, curly quotes, `−` and Latin-1 accents. The current Montserrat build
has none of them.

### Icons

All icons come from one subset of the Tabler Icons webfont (MIT, outline style with 2 px
strokes), so they match the line icons in `index.html`. `crates/waved-ui/fonts/icons.py` lists
the 48 icons. They are rendered at 16, 20, 26 and 44 px. No PNGs need to be deployed, which
is the reason the Save/Send/Discard buttons render blank today.

## Components

All positions are in framebuffer pixels (top-left, width × height) and match `index.html`.

| Component | Size and position | LVGL |
|---|---|---|
| Status cluster | 240 × 20 at (80,8) | top layer, not clickable |
| Status pill (Home, Recording) | 240 × 28 at (80,6), radius 14; hit area 240 × 44 (y 6–50); a 7 px teal dot leads the contents while pre-recording is on | clickable, opens Menu |
| Message | h 28, max w 240, centred, top 6, radius 14 | top layer, not clickable, hides the status for its duration |
| Title | 400 × 24 at (0,38), centred | label |
| Dial | r 70 at (200,121), stroke 3; talker arc 44°, stroke 6; talker dot r 4 on r 82; volume gauge and progress ring on the same r 70 | `lv_arc` × 2 plus a clickable round `lv_obj` (r 71, y 50–192) for mute |
| Dial content stack (Home) | icon 20 px at top 73; state 26 px at top 95; second line 15 px at top 127; hint 12 px at top 153 | labels, centred |
| Dial content stack (Recording) | state 13 px at top 76; timer 48 px at top 93 (digits centred on y 121); second line 13 px at top 140; hint 12 px at top 161 | labels, centred |
| Dial content stack (gauge, progress, resting) | caption 13 px at top 77; value 48 px at top 93; second line 13 px at top 145 | labels, centred |
| Side button | 60 × 60 at (26,91) / (314,91), centres (56,121) / (344,121); label 12 px at top 155, 90 px wide, centred on the button (File detail: top 104, label 168) | `lv_button`, hold-repeat 300 ms |
| Bottom main (Record / Stop / Send / Set up Wi-Fi) | 64 × 64 round at (168,209), centre (200,241); icon 26 px; label 90 wide at (155,279) | `lv_button`, radius `LV_RADIUS_CIRCLE`; Stop adds an `lv_arc` hold ring r 35 |
| Bottom companions (Mark / Pause, Keep / Delete) | 56 × 56 round at (85,181) / (259,181), centres (113,209) / (287,209); icon 22 px; labels 90 wide at (68,241) / (242,241) | `lv_button` × 2 |
| Bottom pair (Decline / Answer) | 64 × 64 round at (81,177) / (255,177), centres (113,209) / (287,209); labels at (68,245) / (242,245) | `lv_button` × 2 |
| Bottom pills | wide 184 × 52 at (108,215), radius 26 (USB storage); pair 124 × 50 at (70,196) / (206,196), radius 25 (Confirmation) | `lv_button` |
| Content pill | h 44, radius 22; e.g. 176 × 44 at (112,164), 160 × 44 at (120,192), 184 × 44 at (108,192), pair of 120 × 44 at (76,174) / (204,174) | `lv_button` |
| Back | 112 × 44 at (144,244), radius 22 | `lv_button`, calls `nav::back()` |
| Rotary list | viewport y 34–240; rows 32 / 44 / 52 / 44 / 32 px at viewport y 0 / 36 / 84 / 140 / 188; focused row 320 × 52 at (40,118), focus centre y 144; fades 14 px top, 16 px bottom | flex column, `LV_EVENT_SCROLL` scaling (`lv_example_scroll_6`) |
| Arc scrollbar | r 188 around (200,106), 62°–122° (y 18–206 on the right edge), stroke 4 | `lv_arc`, value tied to scroll position |
| File list | viewport y 80–222; cards 296 × 52 at x 52, every 58 px | flex column, bottom fade |
| Radio list | rows 296 × 52 at x 52, y 68 / 124 / 180 | `lv_obj` rows with a radio indicator |
| Value row | h 38–44, radius 20, label left, value right, chevron | `lv_button` |
| Switch | 44 × 26 | `lv_switch` |
| Value arc | Volume r 88 at (200,132); Display r 68 at (200,140); 270° sweep, stroke 8, knob 22; − / + 52 px circles centred at x 56 / 344 | `lv_arc` (Volume, Mic level, Display) |
| Radial mode picker | six 48 px buttons on r 132 around (200,134), at 300° / 270° / 240° and 60° / 90° / 120°; preview r 50 | `lv_button` × 6 + `lv_canvas` |
| Confirmation | icon 56 px at (172,40), title 26 px at top 104, body 13 px at top 142 (270 wide), pill pair | modal screen |
| PIN keypad | title at top 6, dots at top 36; 3 × 4 keys of 64 × 48 at x 96/168/240, y 56/110/164/218 | modal screen, no status |

## Screens

| Screen | File | Status |
|---|---|---|
| Start-up | `screens/boot.rs` | new |
| First start | `screens/first_run.rs` | new |
| Home | `screens/home.rs` | redesign |
| Home, muted + message | `screens/home.rs`, `toast.rs` | redesign |
| Home, changing volume | `screens/home.rs` | new state |
| Resting screen | `screens/ambient.rs` | new, plus backlight timers |
| Recording | `screens/recording.rs` | redesign |
| Recording, muted | `screens/recording.rs` | new state |
| Holding Stop | `screens/recording.rs` | new state |
| Recording, paused | `screens/recording.rs` | new state, needs `PauseRecording` handled |
| After stop | `screens/confirmation.rs` | redesign |
| Menu | `screens/settings.rs` | replaces the More hub |
| Audio mode | `screens/audio_mode.rs` | redesign, needs aim support |
| Noise reduction | `screens/noise_reduction.rs` | redesign |
| Volume / Mic level | `screens/volume.rs` | redesign |
| Mic height | `screens/mic_height.rs` | new, needs motor verification |
| Files | `screens/files.rs` | new |
| Recording detail | `screens/file_detail.rs` | new, needs playback |
| Wi-Fi, Wi-Fi setup | `screens/wifi_provisioning.rs` | redesign |
| Bluetooth | `screens/bluetooth.rs` | new, needs Bluetooth backend |
| Uploads | `screens/webhook.rs` | redesign |
| After a recording | `screens/upload_mode.rs` | new |
| Incoming call | `screens/call.rs` | new, needs Bluetooth calls |
| System | `screens/system.rs` | redesign |
| Display | `screens/display.rs` | new |
| Date and time | `screens/datetime.rs` | new |
| Confirmation | `screens/dialog.rs` | new, shared |
| PIN | `screens/pin.rs` | new, guards locked menu rows |
| USB storage | `screens/usb_mode.rs` | new, needs gadget switching |
| Firmware update | `screens/ota.rs` | new, needs OTA |

## Panel note

The panel is an LCD with a PWM backlight, confirmed on the device on 5 October 2026: writing
`0` to `/sys/class/leds/lcd-backlight/brightness` blanks the image completely. There is no
burn-in to manage, and brightness and the resting screen simply set the backlight level.

The visible image is not the 400 × 304 framebuffer but a circle of r ≈ 198 centred at
(200, 106), cut flat at the top by row 0. Rows 0–33 are usable along the flat top edge, and
everything below y ≈ 230 lies on the tight bottom curve. Measurements and the width table are
in `waved/docs/DISPLAY_HARDWARE.md`.

## Regenerating the PNGs

```bash
cd docs/mockups
for id in boot first-run home home-muted home-volume ambient recording recording-muted \
          hold-stop paused decision menu audio-mode noise volume mic-height files file-detail \
          wifi wifi-setup bluetooth uploads upload-mode call system display datetime confirm pin usb ota; do
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --hide-scrollbars \
    --force-device-scale-factor=2 --default-background-color=00000000 --window-size=440,440 \
    --virtual-time-budget=5000 --screenshot="png/$id.png" "file://$PWD/index.html?only=$id"
done
```

Add `&grid=1` to the URL to render the layout-zone overlay: the visible circle (r 198), the tap
limit (r 190), the widest row (y 106), the status zone, the dial tap circle and the r 135 bottom arc. The implementation backlog is in [`../../TODO.md`](../../TODO.md).
