# Speaker scenes: stock's speaker tuning, amp EQ mode, automatic switching — Plan

> **For agentic workers:** host only. Never run `adb`, never open a device. TDD: a failing test
> first; the amplifier control sits behind a trait with a fake. Test with
> `cargo test -p waved-audio --lib`, `cargo test -p waved-state`,
> `cargo test -p waved-audio --test biquad_regression`.

**Problem:** music from a phone over Bluetooth sounds muffled on waved; stock sounds much better.
waved always runs its Voice preset (HPF → 4 peaking filters from `dynamic_eq_config_2.json` →
4-band MBAGC with up to +20 dB on 300 Hz–4 kHz → limiter) and never touches the amplifier's
`Set EQ MODE` control.

**What the stock firmware does (fw 1.2.2.6, `docs/AUDIO_HARDWARE.md` "Stock speaker tuning"):**

- The speaker path (`Insta360_AudioInRoute::run`) is input device → volume → `hw:0,1`. **No EQ, no
  MBAGC, no HPF, no limiter.** [code]
- Both `dynamic_eq_config*.json` files belong to the **microphone** path (`Insta360_EqProccess`
  sits between the 3A and the uplink volume in `Insta360_AudioOutRoute::run`), only
  `/etc/dynamic_eq_config.json` is loaded, and `/etc/audio_config.json` `insta_eq_en: 0` bypasses
  it. `dynamic_eq_config_2.json` is loaded by nothing. [code]
- The amplifier's own EQ (`Set EQ MODE`, card 0) is the only speaker tuning: `2` MUSIC when no
  microphone consumer is active, `1` MEETING while one is (USB host capturing, Bluetooth call,
  local recording/ASR, pre-record); fw 1.2.2.6 keeps `2` during calls too when its 3A algorithm
  is "SK". This unit logged `set downlink eq 1` on mic use and `2` after (stock syslog in the
  `/data` backup, 2026-03/04). `0` BYPASS is never used. [code + log]

## Decisions

| ID | Decision | Why | Cost if wrong |
|---|---|---|---|
| S-1 | **Two scenes:** `Music` (default) and `Meeting`. Meeting while a microphone consumer is active: the host reads the USB microphone (`AudioState.host_listening`) or a Bluetooth call (`BluetoothState.call_audio`). Otherwise Music, so phone music over Bluetooth (`phone_audio && !call_audio`) and USB playback without a capture stream are Music. | Stock's rule (`is_cur_mic_work` in `audio_server_select_out_path_policy`). USB audio carries no "music vs call" flag; whether the host has the mic open is what stock uses. | A host that keeps the mic open all the time (some apps do) keeps USB music in the Meeting scene. Override file below; the device pass checks macOS. |
| S-2 | Not mirrored: local recording, ASR, the pre-record setting and the cascade/dongle paths as Meeting triggers. | Pre-record is a setting: stock (TEA rule) would keep the amp in MEETING all day while it is on. A local recording has no far end to protect. | Music played during a local recording keeps the music tuning. |
| S-3 | **Media preset = stock's speaker path: identity.** No HPF, no EQ, no MBAGC, no limiter; the chain hands the samples through untouched (bit-exact). | Stock runs nothing on the speaker path (S-0 facts). The task said "Media = `dynamic_eq_config.json` unless the disassembly says otherwise": it does, that file is the mic EQ and disabled. | If a source clips the amp, there is no limiter in Media; stock has none either. |
| S-4 | **Voice preset = `dynamic_eq_config_2.json` with its switches honoured:** `hpf_switch 0` → no HPF, `mbagc_switch 0` → no MBAGC, `agc_switch 0` → no AGC (a file with `agc_switch 1` is rejected: waved has no full-band AGC), `iir_eq_switch 1` → the 4 filters. The limiter stays because the EQ boosts (+5 dB at 3.5 kHz). | The user asked to keep the voice EQ and honour the switches. Stock never plays this EQ on the speaker; it stays a waved choice for calls. | Calls sound different from stock (stock = flat + amp MEETING). The A/B below covers "Media + amp 1" (stock parity for calls). |
| S-5 | **Decoded filter types** (`bva::asp::asp_iir_eq_init`, `libbva_audio.so` 0x2a5c0): 0 peaking `(s²+(G/Q)s+1)/(s²+s/Q+1)` (= RBJ peaking with Q·10^(−g/40)), 1 first-order low shelf, 2 first-order high shelf (Q ignored), 3 Butterworth low-pass, 4 Butterworth high-pass (gain and Q ignored), anything else passthrough. The parser maps them all; the voice filters now use stock's peaking form instead of RBJ with stock's Q. | Fidelity to the file we claim to parse. | A small tonal change in Voice (cuts a bit narrower, boosts a bit wider than before). |
| S-6 | **Amplifier EQ mode per scene, like stock's TEA/older rule:** Music → `2`, Meeting → `1`. Written to card 0 `Set EQ MODE` through `AmpEqControl` (trait; ALSA `ctl` element write on the device, a fake on the host), on start and on every scene change; failures are logged once and shown as `amp_eq_mode: null`. | This unit's stock logs show exactly this; it is the rule in every stock version for TEA. | 1.2.2.6 with SK keeps `2` in calls; mode 1 sounded "muffled, far away" on music. One constant (`amp_mode_for`) and the override file flip it. ERLE baselines were taken in mode 2: re-measure (checklist). |
| S-7 | **Switch transition:** fade the old chain out over 5 ms, swap and reset the filter state at silence, fade the new chain in over 5 ms. Replaces the held-sample crossfade (which also crossfaded into the new limiter's zero warm-up). A switch to the preset already running is ignored. | Removes clicks on mid-stream switches (calls start while music plays). The AEC reference is post-DSP, so it fades identically. | 10 ms of dip at a switch. |
| S-8 | **Manual overrides for A/B** (SIGHUP): `/tmp/waved-playback-preset` `voice` / `media` pins the preset (as before), new `auto` follows the scene again. New `/tmp/waved-amp-eq-mode`: `0`/`1`/`2` (or `bypass`/`meeting`/`music`) pins the amp mode, `auto` follows the scene. New commands `SetPlaybackPresetAuto`, `SetAmpEqMode(Option<AmpEqMode>)`; `SetPlaybackPreset` now means "pin". | Keeps the existing A/B path; adds the amp. | — |
| S-9 | **Stats** (`/tmp/audio_stats.json`): `playback_preset` is now the preset the playback thread runs (was: the last command); new `speaker_scene` (`music`/`meeting`), `speaker_scene_auto` (preset follows the scene), `amp_eq_mode` (0–2 last written, `null` until written or on failure), `amp_eq_mode_auto`. `mbagc_active_gain_db` stays (all 0 now). | Device-pass observability. | — |
| S-10 | **ERLE:** no code change in the AEC. MBAGC removal lowers the far-end level (up to −20 dB in 300 Hz–4 kHz for quiet speech) and the amp mode changes the echo path; both need a re-measure (checklist). `SpeexAec::reference_active` uses an absolute floor (mean power ≥ 100, ≈ −70 dBFS RMS), far below speech even without MBAGC. | — | — |

## File structure

| File | Change |
|---|---|
| `crates/waved-state/src/lib.rs` | **Additive:** `AmpEqMode` (stock `SPEAKER_PA_EQ_MODE` values), `Command::SetPlaybackPresetAuto`, `Command::SetAmpEqMode(Option<AmpEqMode>)` |
| `crates/waved-audio/src/playback/dsp/biquad.rs` | Stock peaking, first-order shelves, Butterworth low-pass designers |
| `crates/waved-audio/src/playback/dsp/eq.rs` | `BiquadKind`: `StockPeaking`, `LowShelf1`, `HighShelf1`, `LowPass`, `HighPass` |
| `crates/waved-audio/src/playback/dsp/chain.rs` | Optional HPF and limiter, identity fast path, fade-out/swap/fade-in transition; takes a `PlaybackPresetTuning` |
| `crates/waved-audio/src/playback/preset.rs` | Switches honoured, all type codes, `eq_filter_num` ≤ 4, Media = identity |
| `crates/waved-audio/data/dynamic_eq_config.json` (+ `.source`) | Stock's mic EQ, vendored as parser test data only |
| `crates/waved-audio/src/playback/scene.rs` (new) | `SpeakerScene`, `scene_for`, `preset_for`, `amp_mode_for`, `SceneController` (pins, overrides, change detection), `AmpEqControl` + ALSA impl, `run_scene_follower` |
| `crates/waved-audio/src/playback.rs` | Chain API, skip same-preset switches, active preset / scene / amp in `DspMetrics`, `PresetSink for PlaybackService` |
| `crates/waved-audio/src/stats_collector.rs` | Fields of S-9 |
| `crates/waved/src/main.rs` | **Additive:** spawn the follower; SIGHUP `auto` arm and the amp file |
| `docs/AUDIO_HARDWARE.md` | "Stock speaker tuning" (RE with addresses, [code]/[inferred]); playback chain section updated; ERLE notes |

## Tasks

1. **waved-state:** `AmpEqMode` (`value()`, `from_value()`, `parse()` for `0|1|2|bypass|meeting|music`), the two commands. Tests: values match stock's enum; parse.
2. **Designers + kinds:** tests first — stock peaking equals the decoded bilinear formula and peaks at `g` dB; first-order shelves: DC/Nyquist gains; LPF/HPF −3 dB at the corner. Biquad regression test stays green.
3. **Parser:** tests first — voice file → no HPF, MBAGC off, 4 `StockPeaking`, limiter on; music file → `[HighPass 150, HighShelf1 8 kHz +5, StockPeaking 90 Hz −6 Q2, Bypass]`; `agc_switch 1` rejected; `iir_eq_switch 0` → flat; unknown type → passthrough (as stock).
4. **Chain:** tests first — Media chain is bit-exact; disabled stages skip; switch fades to zero at the swap and back; same-preset switch is a no-op; limiter absent → no lookahead delay.
5. **Scene logic:** tests first — `scene_for` truth table; controller: auto scene changes emit preset + amp once; pin holds against scene changes; `auto` releases; amp override; amp failure reported.
6. **Follower + playback + stats:** follower with fake sink/amp over watch/broadcast channels (tokio test); stats fields serialise.
7. **main.rs wiring** (additive), docs, TODO row.

## Device-pass checklist

Prepare: deploy the build (the overrides below are SIGHUP files: no watchdog stop needed). Keep `/tmp/audio_stats.json` open in a second shell (`watch -n1 'grep -E "speaker_scene|amp_eq_mode|playback_preset" /tmp/audio_stats.json'`).
A/B helpers:

```sh
pp() { echo "$1" > /tmp/waved-playback-preset; killall -HUP waved; }   # voice | media | auto
amp() { echo "$1" > /tmp/waved-amp-eq-mode; killall -HUP waved; }      # 0 | 1 | 2 | auto
```

1. **Boot state.** Nothing playing, no host capture, no call: stats show `speaker_scene "music"`,
   `playback_preset "media"`, `amp_eq_mode 2`, both `*_auto true`. `amixer -c0 cget numid=2` → 2.
2. **Listening A/B with phone music over Bluetooth** (same track, same Wave volume, same phone
   volume; note each impression):
   - a. *Old voice* — not reproducible with this build (MBAGC + HPF are gone); compare against
     the previous binary or memory of it.
   - b. *New voice*: `pp voice; amp 2`.
   - c. *New music*: `pp media; amp 2` — expected closest to stock.
   - d. *Amp modes* on new music: `amp 0`, `amp 1`, `amp 2`.
   - e. *Stock* (if a stock unit or a reflash is at hand): same track, same level.
   - Back to automatic: `pp auto; amp auto`.
3. **Automatic switching.**
   - Phone music (A2DP): scene `music`, preset `media`, amp 2.
   - Bluetooth call (`call_audio`): scene `meeting`, preset `voice`, amp 1 within a period; ends →
     back to music. Listen for clicks at both switches.
   - USB: play music from the Mac with no app using the mic → scene `music`. Then open a call
     app / QuickTime audio recording on the Wave mic → `host_listening true`, scene `meeting`.
     **Check whether macOS keeps the input stream open when no app records** (if yes, USB music
     stays in Meeting; note it).
   - Pin test: `pp media` during a BT call → stays `media` (`speaker_scene_auto false`); `pp auto`
     → back to `voice`.
4. **Calls A/B (voice quality, far-end opinion):** during a BT or USB call compare
   `pp voice; amp 1` (auto default), `pp media; amp 1` (stock parity), `pp voice; amp 2`
   (1.2.2.6-SK behaviour).
5. **ERLE** (`scripts/test-aec-on-device.sh`, `LINEAR_ERLE_THRESHOLD_DB=20.7`; root `TODO.md`
   C14), each at the same speaker SPL (raise the test level/volume so the echo at the mics
   matches the old Voice+MBAGC run, otherwise a quieter echo lowers the measurable ERLE):
   - Meeting scene (Voice, amp 1) — the new default for calls.
   - Voice, amp 2 — compare with the SP#5 baseline (23.2 dB) and the 26.6 dB Apple Music median.
   - Media, amp 1 — stock-parity call setting.
   - Scene switch mid-far-end (start a BT call while USB far-end plays): echo burst length until
     ERLE recovers; `aec` stats in `/tmp/audio_stats.json`.
6. **Volume/limits:** loud music (0 dBFS master) in Media at 100 % volume: no amp clipping
   distortion compared with stock at its maximum. `limiter_reduction_db` stays 0 in Media.
7. **Failure path:** if `amp_eq_mode` stays `null`, read `/tmp/waved.log` for the `Set EQ MODE`
   warning (control name/card mismatch).
