# Recording formats: Opus by default, FLAC as a setting — Plan

> **For agentic workers:** host only. Never run `adb`, never open a device. TDD: a failing test
> first. Codecs live in a host-testable crate (`cargo test -p waved-codec --lib`);
> `waved-meeting` and the binary test in Linux (`scripts/linux-test.sh test -p waved-meeting
> --lib`, `scripts/linux-test.sh check -p waved --features ui,capture-alsa`).

**Problem:** recordings are WAV 48 kHz mono 16-bit (`hound`), about 345 MB per hour. A 1 h 30
meeting hit the owner's webhook 413 limit. The owner decided (2026-10-14): **Opus by default,
FLAC as a setting**, no WAV setting, existing WAV recordings keep working everywhere.

## Decisions

| ID | Decision | Why | Cost if wrong |
|---|---|---|---|
| R-1 | **Opus:** Ogg Opus (`.opus`), 48 kHz mono, 32 kbps VBR, `OPUS_APPLICATION_AUDIO`, complexity 10, 20 ms frames. Pre-skip = the encoder's lookahead (RFC 7845 §4.2), OpusHead and OpusTags on their own pages, the last page's granule trims the padding (§4.4). | 32 kbps is transparent-enough fullband speech for ASR and ~14.5 MB/h. AUDIO keeps the (already AEC'd, beamformed) input faithful: only DC rejection, where VOIP adds a pitch-tracking high-pass; its classifier still codes speech in SILK/hybrid at 32 kbps and music in CELT, where VOIP pushes music into the speech codec. | Speech quality at 32 kbps is near-identical in both modes; one constant changes it. |
| R-2 | **FLAC:** 16-bit mono 48 kHz (`.flac`), fixed 4096-sample blocks, `flacenc` default compression. STREAMINFO is written first with total samples 0 (unknown, valid) and patched at finalize with the total, min/max frame size and the MD5. | Lossless, every frame is on disk as soon as it is encoded. | Larger than Opus (~45–60 % of WAV, content dependent). |
| R-3 | **Libraries:** `opusic-c` 1.5 / `opusic-sys` 0.6 (libopus **1.5.2**, BSD-3-Clause, built from source and linked statically) with `no-simd` and `no-fortify-source`; `flacenc` 0.5 (Apache-2.0, default features off); `claxon` 0.4 (Apache-2.0) to decode FLAC; `hound` for WAV; our own small Ogg page layer (writer, reader, bisection seek). | Every libopus crate builds with CMake; the cross image has CMake 3.23 but GCC **4.8.5**: its `arm_neon.h` breaks libopus' NEON intrinsics, its sysroot breaks `_FORTIFY_SOURCE` (`MB_LEN_MAX`), and libopus 1.6 (`opusic-sys` 0.7) calls `vcvtns_s32_f32`, which GCC 4.8 lacks, on every aarch64 build. Verified by building each in the image. `rust:1-bookworm` (linux-test.sh) has no CMake: the script installs it. | Scalar libopus: measured below; NEON would only lower CPU further. |
| R-4 | **`RecordingWriter` trait** (`wav` for existing files and tests, `opus`, `flac`). The recorder converts to S16 and encodes on the meeting task as today. A pre-roll (up to 30 s) goes into a backlog that drains at most 100 ms of audio per incoming frame on top of the frame itself; finalize (blocking pool) drains the rest, an abort at most 3 s more. | Encoding 30 s at once could stall the meeting task past the 32-frame channel (~680 ms) and drop frames. Live frames encode in well under a period. | Up to ~6 s after Record the newest audio is still in RAM, not on disk. |
| R-5 | **Durability:** the writer pushes a finished Ogg page / FLAC frames to the file every 1 s of audio; a per-take helper thread `fdatasync`s every 2 s; a sync error fails the take closed like a write error. Abort and drop finish the stream (EOS page / STREAMINFO patch) under the `.partial` name. | `kill -9` keeps everything written; a power cut loses at most ~3 s; a cut file is valid up to its last page/frame. | One small eMMC write every 2 s while recording. |
| R-6 | **File names:** `audio.wav`, `audio.opus`, `audio.flac`, and `<name>.partial` while recording. One finder for every reader (reconcile, import, Files, player, upload). The Phase-1 placeholder `audio.opus` (always empty) stays `legacy_unsupported`: an empty `audio.opus` is not audio. | Additive; no database change. | — |
| R-7 | **Recovery parity:** unchanged policy. A partial stays `artifact_error`, never uploadable; WAV-finalized → `missing_artifact`; metadata-finalized → `local_only` (plays, can be sent). | M1 certified this policy; the formats must not change it. | — |
| R-8 | **Uploads:** `audio` part named `<meeting-id>.<ext>`, `Content-Type: audio/ogg; codecs=opus`, `audio/flac` or `audio/wav`. Metadata keeps `schema_version` 1 (additive): `audio.codec` `opus`/`flac`/`pcm_s16le`, `audio.content_type`, `audio.filename`, `bit_depth` 16 (the source PCM depth), new `audio.bitrate_bps` for Opus only. The webhook test sends a short silent clip in the chosen format. | Receivers keyed on the part name keep working. | A receiver that hard-codes `.wav` must follow `audio.filename`/content type (WEBHOOKS.md). |
| R-9 | **Setting:** `recording.format = "opus" \| "flac"`, default opus, saved only when not the default (older waved still loads a default config). System → **Recording format** (Opus / FLAC, a tap toggles), after Pre-recording; setup page select next to Pre-recording. Read at each Record: a change applies to the next recording. | Same additive rule as milestone 3 keys. | — |
| R-10 | **Storage:** `AudioFormat::estimated_bytes_per_hour` (Opus 14.5 MB, FLAC 210 MB, WAV 345.6 MB) for logs and docs. There is no "time left" display; the retention thresholds stay absolute MB. | Nothing to convert. | The 2 GiB free-space floor is ~140 h of Opus (was ~6 h of WAV). |

## File structure

| File | Change |
|---|---|
| `crates/waved-codec/` (new) | `AudioFormat`, finder, `RecordingWriter` + wav/opus/flac writers, `PcmReader` + wav/opus/flac readers with seek, Ogg pages + CRC, FLAC header parsing, host benchmark (`examples/encode_bench.rs`) |
| `crates/waved-meeting/src/recorder.rs` | Format per take, writer trait, backlog, periodic sync thread |
| `crates/waved-meeting/src/{reconcile,files,worker,upload,metadata,player,lib}.rs` | Finder instead of `audio.wav`; content types; metadata fields; decode any format; abort on the blocking pool |
| `crates/waved-state/src/lib.rs` | **Additive:** `RecordingFormat`, `SettingsState::recording_format`, `Command::SetRecordingFormat` |
| `crates/waved-config/src/{lib,form}.rs` | **Additive:** `recording.format` (skip when default), setup field |
| `crates/waved-settings/src/*` | `Change::RecordingFormat` |
| `crates/waved-provision/src/setup_form.rs` | current value of the select |
| `crates/waved-ui/src/screens_v2/system.rs` | Recording format row, goldens |
| `scripts/linux-test.sh` | install `cmake` |
| Docs | `AUDIO_HARDWARE.md` (format section), `WEBHOOKS.md`, root `TODO.md` |

## Tasks

1. `waved-codec`: Ogg CRC and pages (tests: CRC known vector, page round trip, cross-check with the
   `ogg` crate, truncated tail ignored); Opus writer/reader (round trip: length exact, SNR on
   speech-like test signal, OpusHead pre-skip, granule end trim, seek, cut file plays to its last
   page); FLAC writer/reader (bit-exact round trip, STREAMINFO patched, cut file decodes all
   complete frames, seek); WAV wrappers; finder; content types; benchmark.
2. `waved-state`/`waved-config`/`waved-settings`: format type, config default + serialization rule,
   form field, change mapping.
3. `waved-meeting`: recorder on the trait (all existing recorder tests per format), backlog,
   sync thread, reconcile/import/Files/player/upload/metadata/test payload for every format;
   service tests run Opus (default) and FLAC where values are compared.
4. UI: System row, goldens.
5. Docs, TODO, cross build proof.

## Device-pass checklist

- [ ] Deploy; System → Recording format shows **Opus**. Record 10 min (pre-recording on). Note the
      file size (expect ~2.4 MB) and `audio_duration_ms`.
- [ ] Switch to **FLAC**, record 10 min. Size (expect ~25–40 MB). Switch back to Opus.
- [ ] During both: `processed_frames_dropped`, `frames_dropped`, `xrun_count` unchanged in
      `/tmp/audio_stats.json`; `top` CPU of waved with and without a recording (note both).
- [ ] Files → play each (Opus, FLAC, and an old WAV): audio, seek to the middle and near the end,
      marks, pause/resume.
- [ ] Upload each to the local receiver (`scripts/webhook_receiver.py`): part filename `.opus` /
      `.flac`, `Content-Type`, metadata `codec`/`bitrate_bps`/`sha256`; `ffprobe`/`opusinfo` and
      `flac -t` on the received files; webhook test from the setup page in each format.
- [ ] `kill -9` waved 2 min into an Opus take (watchdog restarts it): the take is
      `artifact_error`, not uploadable, `audio.opus.partial` plays on a computer up to ~1–3 s
      before the kill. Same for FLAC (`flac -t` reports only the missing tail).
- [ ] Physical power cut 2 min into a take (Opus, then FLAC): after boot, same status and the
      partial plays up to ≤ ~3 s before the cut.
- [ ] `RUN_PHASES=endurance,active-partial,wav-finalized,metadata-finalized
      ./scripts/test-recording-resilience.sh` with Opus (the script's WAV checks need the format
      update noted in TODO.md).
- [ ] An existing WAV recording still plays, uploads (`audio/wav`) and survives reconcile.
