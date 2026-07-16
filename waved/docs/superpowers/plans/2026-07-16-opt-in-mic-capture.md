# Opt-in diagnostic microphone capture implementation plan

**Design:** `docs/superpowers/specs/2026-07-16-opt-in-mic-capture-design.md`

## Task 1: Isolate diagnostic capture

- Add `crates/waved/src/mic_capture.rs`.
- Parse `WAVED_MIC_CAPTURE_PATH` as disabled when missing or empty.
- Move PCM conversion and writing into a recorder function that exits on its
  first create, write, or flush failure.
- Log successful opt-in once with the selected destination.

## Task 2: Make startup opt-in

- Register the new module in `main.rs`.
- Remove the unconditional processed-audio subscription and recorder task.
- Subscribe and spawn only when the environment parser returns a path.

## Task 3: Test

- Unit-test missing, empty, and non-empty environment values.
- Unit-test that a failing writer receives exactly one write attempt and the
  recorder returns the error.
- Run the `waved` tests and simulated build.
- Run formatting and clippy for the changed package.
- Cross-build the release ARM64 binary with `ui,capture-alsa`.

## Task 4: Deploy and accept on hardware

- Stop the existing watchdog and process so the binary can be replaced safely.
- Deploy the new ARM64 binary and restart the init service.
- Verify the process and UI stay healthy, the framebuffer contains the full
  home screen, and no default microphone capture file is created.
- Run an explicit short capture to `/mnt/udisk/debug/`, verify it grows, then
  restore the normal no-environment startup.
- Exercise a failing `/dev/full` destination and verify one warning without
  affecting the UI or main service.
