# Opt-in diagnostic microphone capture — design

**Date:** 2026-07-16  
**Status:** Approved for specification

## Problem

`waved` currently starts an unconditional diagnostic recorder that writes the
post-AEC mono stream to `/tmp/mic_capture.pcm` as 48 kHz S16LE audio. At roughly
96 kB/s, the file eventually fills the device's 1.8 GB `/tmp` tmpfs. The
recorder then logs a write failure for every processed frame. Those repeated
messages grow `/data/waved/waved.log` until the 440 MB `/data` partition is also
full, preventing the watchdog from launching `waved` and leaving the OLED
blank.

The recorder is useful during audio diagnostics, but it must not consume
storage during normal operation.

## Decision

Raw diagnostic microphone capture is disabled by default. It is enabled only
when the process starts with a non-empty `WAVED_MIC_CAPTURE_PATH` environment
variable.

Example:

```sh
WAVED_MIC_CAPTURE_PATH=/mnt/udisk/debug/mic_capture.pcm \
    /data/waved/waved
```

The path is operator-controlled. `/mnt/udisk` is recommended for intentional
longer captures because it has substantially more capacity than `/tmp` or
`/data`.

## Runtime behavior

At startup, `waved` reads `WAVED_MIC_CAPTURE_PATH` once:

- Missing or empty: do not subscribe a diagnostic recorder to the processed
  audio stream, do not spawn its task, and do not create a PCM file.
- Non-empty: subscribe to the existing post-AEC processed-audio broadcast,
  truncate or create the requested file, and write 48 kHz mono S16LE samples
  using the existing float-to-`i16` conversion.

Successful opt-in logs one informational message containing the destination
path so a diagnostic session is auditable.

## Failure handling

Diagnostic capture is non-essential and must never threaten the main service:

- If the output file cannot be created, log one warning and end the recorder
  task.
- If a write or flush fails, log one warning and end the recorder task. Do not
  retry per frame.
- A lagged broadcast receiver may continue to report dropped frames because
  that is useful diagnostic information and is not a storage-amplifying
  failure.
- Recorder failure does not stop capture, DSP, USB audio, meetings, or the UI.

The existing watchdog and application-log policy are outside this change, but
the terminal-on-write-error behavior removes the observed log-amplification
path.

## Code structure

The unconditional recorder block in `crates/waved/src/main.rs` becomes two
small responsibilities:

1. A pure environment-value parser maps a missing or empty value to `None` and
   a non-empty value to `PathBuf`. Keeping this pure makes the default/opt-in
   policy testable without mutating process-global environment variables.
2. A recorder task accepts the selected path and processed-audio receiver. It
   owns file creation, conversion, buffered writes, and terminal error
   handling.

The processed-audio receiver for this diagnostic path is created only when the
environment variable enables the recorder.

## Verification

Host tests cover:

- missing environment value disables capture;
- empty environment value disables capture;
- a non-empty value preserves the requested path;
- a forced writer failure terminates the recorder rather than attempting a
  write for every subsequent frame.

Build verification includes the simulated host build and the release ARM64
build with `ui,capture-alsa`.

On-device acceptance:

1. Boot normally and verify `/tmp/mic_capture.pcm` is not created and `/tmp`
   usage remains stable.
2. Start a controlled instance with
   `WAVED_MIC_CAPTURE_PATH=/mnt/udisk/debug/mic_capture.pcm`, speak into the
   microphones, and verify the file grows and decodes as 48 kHz mono S16LE.
3. Force the destination to fail and verify exactly one recorder write warning
   appears while `waved` and the OLED remain healthy.

## Non-goals

- No UI control, persistent configuration field, or runtime toggle.
- No automatic retention, rotation, or ring buffer.
- No change to meeting recordings or USB audio routing.
- No general application-log rotation work.
