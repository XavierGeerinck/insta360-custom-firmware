# Display Redesign — Milestone 1: Shared Contracts and UI Kit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the shared foundation for the approved redesign: new shared state and commands, config v2 with runtime settings persistence, and a tested LVGL UI kit (fonts, icons, tokens, components, router, host snapshot tests). With that in place, milestone 2 can build screens and backends in parallel without touching shared files.

**Architecture:**
- **Contracts.** Shared contracts go first: `waved-state` (commands and watch channels), `waved-config` (v2 schema) and a new `waved-settings` crate. That crate seeds live state at boot and persists device-side changes with a debounced, delta-merged write.
- **UI kit.** The UI kit is a new `waved-ui::kit` module that sits next to the legacy screens, which keep running unchanged until milestone 2 swaps the runtime. It provides fonts, icons and tokens; `Obj` (a safe LVGL handle with handlers freed on delete); a dispatcher, routes, a router and snapshots; reusable components; and a headless LVGL test kit that renders frames to PNG and compares them with committed golden images.
- **Verification.** Everything is test-driven on the macOS host. The crates that need sqlx are verified in a native arm64 Linux container.

**Tech Stack:**
- Rust 2021, stable toolchain, tokio
- LVGL 9.2 through bindgen FFI
- `lv_font_conv` 1.5.3
- Barlow / Barlow Semi Condensed (OFL)
- Tabler Icons webfont 3.48.0 (MIT)
- the `png` crate 0.17 for golden images
- Docker (OrbStack) for Linux checks

**Spec:**
- `../docs/mockups/README.md` and `../docs/mockups/index.html` (approved revision 2, with the decisions recorded)
- `../TODO.md` (backlog: decisions D1–D10, the milestone plan, Phase 0 items P0.1–P0.6)

All code paths in this plan are relative to `waved/` (the Rust workspace). Paths that start with `../` are relative to `waved/` too, so they point into the monorepo root (`/Users/xaviergeerinck/Projects/insta360`).

---

## Global Constraints

These come from the spec. Every task's requirements include them.

- Framebuffer **400 × 304**. The visible panel is a circle of radius **190 px** centred on **(200, 152)**.
- No tappable pixel lies farther than **182 px** from (200, 152) (`TAP_LIMIT_R`).
- Tap targets are **at least 44 px** in both directions. Main actions are **at least 50 px** tall. `lv_obj_set_ext_click_area` may provide the size, but the visible box must sit inside the tap limit.
- Colours are exactly these 0xRRGGBB values:

  | Token | Hex | Token | Hex |
  |---|---|---|---|
  | BG | `000000` | TEXT_3 | `5d6a6a` |
  | SURFACE | `141c1e` | LIVE | `3fd6b1` |
  | RAISED | `1f292b` | ON_LIVE | `00140f` |
  | LINE | `2a3639` | REC | `ff4b3e` |
  | TEXT | `f2f5f4` | WARN | `ffb547` |
  | TEXT_2 | `9aa7a6` | | |

- Colour meaning:
  - teal (LIVE): live, selected or healthy
  - red (REC): recording, muted or destructive
  - amber (WARN): paused or needs attention
  - everything else is grey on black
- Type scale:

  | Role | Size / line height | Face |
  |---|---|---|
  | big | 48 / 52 | Barlow Medium, tabular figures |
  | dial | 26 / 30 | Barlow Semi Condensed SemiBold |
  | title | 20 / 24 | Barlow Semi Condensed SemiBold |
  | body | 15 / 18 | Barlow Semi Condensed Medium |
  | small | 13 / 16 | Barlow Semi Condensed Medium |
  | micro | 12 / 14 | Barlow Semi Condensed Medium |

- Fixed placements and timings:

  | Item | Value |
  |---|---|
  | Back button | 112 × 44 at (144, 252), radius 22, chevron plus "Back" |
  | Status pill | 200 × 28 at (100, 7), radius 14 |
  | Messages | in the status slot, at most 230 px wide, never clickable; success/info 2 s, warning/error 4 s |
  | Volume gauge | shown 1.5 s after the last step |
  | Hold to stop | 1 s |
  | Resting screen | after 2 min |
  | Screen off | after 10 min |
  | "Send this recording?" auto-Keep | 60 s |

- Copy:
  - sentence case and plain words;
  - a button names its action, and the message that follows uses the same verb;
  - errors say what happened and what to do next;
  - no "…" placeholder text in finished screens.
- Decisions D1–D10 in `../TODO.md` are settled. Don't reopen them.
- Screens and components call LVGL only through `kit::obj::Obj`, `kit::event::on*` and component APIs. The only raw `crate::bindings` calls allowed are in `kit/obj.rs`, `kit/event.rs`, `kit/testkit.rs` and `kit/components/*`.
- Legacy UI files (`src/screens/*`, `src/lib.rs` loop, `src/nav.rs`, `src/theme.rs`, `src/toast.rs`, `src/status_bar.rs`) are **not modified** in this milestone. The runtime switch happens at the end of milestone 2.
- Commits use the conventional format `type(scope): lowercase summary` with a bullet-list body, ending in:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- Verification commands always run with `set -o pipefail`.
  - **Host (macOS):** `cargo test -p <crate> --lib` for `waved-state`, `waved-config`, `waved-settings`, `waved-ui` and `waved-provision`.
  - **Linux container:** `scripts/linux-test.sh …` for `waved-meeting` and `waved`. macOS can't load the `sqlx-macros` dylib built by the Xcode 27 linker.
  - **Lint:** `cargo clippy -p <crate> --all-targets -- -D warnings` for each changed host crate.
- Pinned tool versions:
  - google/fonts commit `9710da1eacb3be272583c3224dcb70f9da6eadbb`
  - `lv_font_conv@1.5.3`
  - `@tabler/icons-webfont@3.48.0`
  - `fonttools==4.66.1`
  - `opentype-feature-freezer==1.32.2`
  - `png = "0.17"`

## Review Focus

These five inputs aren't exercised by the happy-path tests but are likely to hurt someone using the device. Each one has a test in the task named after it.

1. **A config file from another firmware version or edited by hand.** Example: a v1 file with `speaker_volume = 150`, or with sections missing. The file must still load, user values must survive, out-of-range values must be clamped, and newer versions must be rejected rather than half-read. *(Task 5: `v1_config_migrates_to_v2_and_keeps_user_values`)*
2. **The setup page saves a new config while a device-side change is still waiting to be written.** The settings service must not overwrite the fields provisioning just saved. *(Task 8: `a_config_saved_by_setup_keeps_its_fields`)*
3. **Bursts of commands**, for example holding Louder or the host volume slider (10+ commands per second). The result must be a single write that holds the final value. *(Task 8: `bursts_are_saved_once_with_the_final_value`)*
4. **Out-of-range or hostile values from the host or the UI.** Examples: `SetBrightness(0)`, `SetSpeakerVolume(250)`, `SetTimeZone("../etc")`, `SetBeamAim(-60.0)`, `SetBeamAim(NaN)`. These must be clamped or ignored, and no invalid config may ever reach disk. *(Task 7: `out_of_range_commands_are_clamped_or_ignored`; Task 8: `invalid_values_never_reach_the_file`)*
5. **Long text in fixed-width parts.** Examples: a long Wi-Fi name, a long upload host, long list titles. The text must truncate with an ellipsis inside its box and must not push a tap target past the tap limit. *(Task 12: `labels_truncate_inside_their_width`; Task 19: `long_titles_stay_inside_the_row`; Task 20: `long_values_stay_inside_the_row`)*

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `scripts/linux-test.sh` | Run cargo in a native arm64 Debian container | Create |
| `../.gitignore` | Ignore `target-linux/` | Modify |
| `CLAUDE.md` | Testing conventions: host vs Linux container | Modify |
| `crates/waved-state/src/lib.rs` | `UploadMode`, `SettingsState`, `FilesState`, `PlaybackState`, new `AudioState`/`MeetingState` fields, new commands, `StateBus` channels | Modify |
| `crates/waved-config/src/lib.rs` | Config v2 schema, defaults, re-exports | Modify |
| `crates/waved-config/src/migrate.rs` | v1 → v2 migration | Modify |
| `crates/waved-config/src/validate.rs` | v2 validation, `valid_time_zone` | Modify |
| `crates/waved-config/src/form.rs` | Setup-page fields for v2 settings | Modify |
| `config/default.toml` | Default v2 config | Modify |
| `crates/waved-provision/src/setup_form.rs` | Render values for the new fields | Modify |
| `crates/waved-settings/Cargo.toml` | New crate manifest | Create |
| `crates/waved-settings/src/lib.rs` | `seed_state`, `settings_from_config` | Create |
| `crates/waved-settings/src/change.rs` | `Change`: command → clamped setting, applied to config/state | Create |
| `crates/waved-settings/src/service.rs` | `SettingsService`: debounced, delta-merged persistence | Create |
| `crates/waved/Cargo.toml`, `crates/waved/src/main.rs` | Seed state at boot, spawn `SettingsService` | Modify |
| `crates/waved-ui/fonts/build_fonts.sh`, `fonts/icons.py`, `fonts/README.md`, `fonts/LICENSE-*.txt` | Pinned font/icon generation | Create |
| `crates/waved-ui/src/kit/fonts/*.c` | Generated LVGL fonts (committed) | Create |
| `crates/waved-ui/build.rs` | Compile kit fonts with LVGL | Modify |
| `crates/waved-ui/lv_conf.h` | `LV_THEME_DEFAULT_DARK 1` | Modify |
| `crates/waved-ui/Cargo.toml` | `png` dev-dependency | Modify |
| `crates/waved-ui/src/lib.rs` | `pub mod kit;` | Modify |
| `crates/waved-ui/src/kit/mod.rs` | Kit module root | Create |
| `crates/waved-ui/src/kit/fonts.rs` | `Font` roles → generated fonts | Create |
| `crates/waved-ui/src/kit/icons.rs` | Icon code points (generated) | Create |
| `crates/waved-ui/src/kit/tokens.rs` | Colours, geometry, layout rects, angle helpers | Create |
| `crates/waved-ui/src/kit/obj.rs` | `Obj` handle with chainable setters | Create |
| `crates/waved-ui/src/kit/event.rs` | `on`/`on_click` with free-on-delete | Create |
| `crates/waved-ui/src/kit/testkit.rs` | Headless LVGL, frames, golden PNGs, tap-target checks (test only) | Create |
| `crates/waved-ui/src/kit/route.rs` | `Route`, `ConfirmKind` | Create |
| `crates/waved-ui/src/kit/dispatch.rs` | `Dispatcher`, `UiEvent`, `Message`, `Severity` | Create |
| `crates/waved-ui/src/kit/snapshot.rs` | `UiSnapshot`, `format_clock`, connection line | Create |
| `crates/waved-ui/src/kit/screen.rs` | `Screen` trait, `ScreenCtx` | Create |
| `crates/waved-ui/src/kit/router.rs` | `Router` (stack, base routes, PIN, resting) | Create |
| `crates/waved-ui/src/kit/components/mod.rs` | Components module root | Create |
| `crates/waved-ui/src/kit/components/status.rs` | `StatusArea` (cluster, pill, message) | Create |
| `crates/waved-ui/src/kit/components/buttons.rs` | `Tone`, back/text buttons, `PrimaryButton`, `SideButton` | Create |
| `crates/waved-ui/src/kit/components/row.rs` | `ButtonRow`, `confirm_dialog` | Create |
| `crates/waved-ui/src/kit/components/dial.rs` | `Dial` (ring, talker arc, gauge, progress) | Create |
| `crates/waved-ui/src/kit/components/list.rs` | `RotaryList` with fades and arc scrollbar | Create |
| `crates/waved-ui/src/kit/components/controls.rs` | `RadioList`, `ValueRow`, `SwitchRow`, `ValueArc` | Create |
| `crates/waved-ui/src/kit/components/compositions.rs` | Home and After a recording built from kit parts (test only) | Create |
| `crates/waved-ui/src/kit/README.md` | How to build a screen with the kit | Create |
| `crates/waved-ui/golden/*.png` | Committed golden frames | Create |
| `../docs/mockups/README.md` | Correct the icon section to what shipped | Modify |
| `scripts/png-to-fb.py` | Golden PNG to a framebuffer image, for the device check | Create |
| `docs/DISPLAY_HARDWARE.md` | The panel is an LCD (after the device check) | Modify |

---

## Execution Rules

- **Order.** Two lanes run in parallel, each in its own worktree branched from `feat/display-redesign`:
  - **Lane A, contracts:** Tasks 1–9.
  - **Lane B, UI kit:** Tasks 10–12. Task 13 needs the state from Tasks 2 and 3, so lane B then rebases onto lane A's branch and continues with Tasks 13–21.
  - Task 22 runs last, once both lanes are merged into `feat/display-redesign`.
  - Within a lane, tasks run in order, because each depends on the interfaces of the ones before it.
  - Both lanes change `Cargo.lock`. On a rebase conflict, take either side and run `cargo check --workspace` to regenerate it.
- **Tests.** Every task starts with a failing test, and every task ends green and lint-clean, then commits.
- **Golden images.**
  - The first run of a new golden test fails with "missing golden".
  - Run it once with `UPDATE_GOLDEN=1` to write the PNG.
  - **Open the PNG** and compare it with the named mockup in `../docs/mockups/png/`. Fix the component until it matches, then rerun with `UPDATE_GOLDEN=1`.
  - Commit the golden together with the code.
  - Reviewers compare the golden against the mockup.
- **Running the test kit.** LVGL is a global singleton. All kit tests go through `testkit::lvgl(|lv| …)`, which serializes them, so `cargo test` can keep running in parallel threads.
- **Scope.** Don't edit legacy UI files. If a kit task appears to need a legacy change, stop and report it.

---

### Task 1: Linux container test runner

**Files:**
- Create: `scripts/linux-test.sh`
- Modify: `../.gitignore`
- Modify: `CLAUDE.md` (section "Testing Conventions")

**Interfaces:**
- Produces: `scripts/linux-test.sh <cargo args…>`, which runs `cargo <args>` in `rust:1-bookworm` with `libclang-dev`, `libasound2-dev` and `pkg-config`. The target dir is `target-linux/`, and the registry is cached in Docker volumes.

- [ ] **Step 1: Create the script**

```bash
#!/usr/bin/env bash
# Run cargo inside a native arm64 Debian container (OrbStack/Docker).
# macOS can't load the sqlx-macros dylib built by the Xcode 27 linker, so
# waved-meeting and the waved binary are built and tested here instead.
#
# Usage: scripts/linux-test.sh <cargo args...>
#   scripts/linux-test.sh test -p waved-meeting --lib
#   scripts/linux-test.sh check -p waved --features ui,capture-alsa
set -euo pipefail
WAVED_DIR="${WAVED_DIR:-$(cd "$(dirname "$0")/.." && pwd)}"
IMAGE="${WAVED_LINUX_IMAGE:-rust:1-bookworm}"
docker run --rm \
  -v "$WAVED_DIR":/work -w /work \
  -v waved-cargo-registry:/usr/local/cargo/registry \
  -v waved-cargo-git:/usr/local/cargo/git \
  -e CARGO_TARGET_DIR=/work/target-linux \
  -e RUSTC_WRAPPER= \
  "$IMAGE" bash -c '
    set -euo pipefail
    if ! dpkg -s libclang-dev libasound2-dev >/dev/null 2>&1; then
      apt-get update -qq && apt-get install -y -qq libclang-dev libasound2-dev pkg-config >/dev/null
    fi
    cargo "$@"' _ "$@"
```

Make it executable: `chmod +x scripts/linux-test.sh`.

- [ ] **Step 2: Run it against the crate that fails on macOS**

Run: `set -o pipefail; scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | tail -3`
Expected: `test result: ok. 67 passed; 0 failed`

Run: `set -o pipefail; scripts/linux-test.sh check -p waved --features ui,capture-alsa 2>&1 | tail -2`
Expected: `Finished \`dev\` profile`

- [ ] **Step 3: Ignore the container target dir and document the rule**

Append to `../.gitignore`:

```
**/target-linux/
```

In `CLAUDE.md`, under `## Testing Conventions`, add after the first bullet:

```markdown
- `waved-meeting` and the `waved` binary depend on sqlx, whose proc-macro dylib
  macOS (Xcode 27 linker) refuses to load. Build and test them in Linux:
  `scripts/linux-test.sh test -p waved-meeting --lib`,
  `scripts/linux-test.sh check -p waved --features ui,capture-alsa`.
  Every other crate tests on the host with `cargo test -p <crate> --lib`.
```

- [ ] **Step 4: Commit**

```bash
git add scripts/linux-test.sh ../.gitignore CLAUDE.md
git commit -m "chore(test): run sqlx crates in a linux container

- scripts/linux-test.sh runs cargo in rust:1-bookworm with libclang and alsa
- macOS cannot load the sqlx-macros dylib built by the Xcode 27 linker
- document which crates test on the host and which in the container

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Move `UploadMode` into `waved-state`

`SettingsState` (Task 3) and `Command::SetUploadMode` (Task 4) need `UploadMode`. `waved-config` depends on `waved-state`, not the other way round, so the enum moves down and `waved-config` re-exports it. Existing imports of `waved_config::UploadMode` keep working.

**Files:**
- Modify: `crates/waved-state/src/lib.rs` (after `PlaybackPreset`, about line 190)
- Modify: `crates/waved-config/src/lib.rs:59-66` (remove the enum, re-export it)
- Test: `crates/waved-state/src/lib.rs` (`mod tests`)

**Interfaces:**
- Produces: `waved_state::UploadMode { Automatic (default), Ask, LocalOnly }`, which derives `Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize` with `#[serde(rename_all = "snake_case")]`. `waved_config::UploadMode` is a re-export of it.

- [ ] **Step 1: Write the failing test** in `crates/waved-state/src/lib.rs` `mod tests`:

```rust
    #[test]
    fn upload_mode_serializes_snake_case_and_defaults_to_automatic() {
        assert_eq!(UploadMode::default(), UploadMode::Automatic);
        assert_eq!(
            serde_json::to_string(&UploadMode::LocalOnly).unwrap(),
            "\"local_only\""
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `set -o pipefail; cargo test -p waved-state --lib upload_mode 2>&1 | tail -5`
Expected: compile error `cannot find type \`UploadMode\``

- [ ] **Step 3: Add the enum to `waved-state`** directly after the `PlaybackPreset` enum:

```rust
/// What happens to a recording when it stops. Lives here (not in waved-config)
/// so state, commands and config share one type.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadMode {
    #[default]
    Automatic,
    Ask,
    LocalOnly,
}
```

In `crates/waved-config/src/lib.rs`, delete the `UploadMode` enum (lines 59-66) and add it to the existing `use waved_state::AudioMode;` line as a public re-export:

```rust
use waved_state::AudioMode;
pub use waved_state::UploadMode;
```

- [ ] **Step 4: Run the tests and the dependants**

Run: `set -o pipefail; cargo test -p waved-state -p waved-config --lib 2>&1 | grep "test result"`
Expected: two lines, `ok. 20 passed` (waved-state) and `ok. 11 passed` (waved-config)

Run: `set -o pipefail; cargo check -p waved-ui -p waved-provision 2>&1 | tail -1`
Expected: `Finished`

Run: `set -o pipefail; scripts/linux-test.sh check -p waved-meeting 2>&1 | tail -1`
Expected: `Finished`

- [ ] **Step 5: Lint and commit**

Run: `cargo clippy -p waved-state -p waved-config --all-targets -- -D warnings`
Expected: no warnings

```bash
git add crates/waved-state/src/lib.rs crates/waved-config/src/lib.rs
git commit -m "refactor(state): move UploadMode into waved-state

- settings state and commands need the type; waved-config depends on waved-state
- waved-config re-exports it so existing imports keep compiling

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Shared state for the redesign

**Files:**
- Modify: `crates/waved-state/src/lib.rs`
  - the `AudioState` struct and its `Default` (lines 16-98)
  - `MeetingState` (lines 245-269)
  - new types after `PendingDecision`
  - `StateBus` (lines 402-456)
- Test: `crates/waved-state/src/lib.rs` (`mod tests`)

**Interfaces:**
- Produces:
  - `AudioState.host_listening: bool`: the host has the USB microphone stream open (UAC "PCM Playback State"); workstream C wires it.
  - `AudioState.doa_confidence: f32` and `AudioState.beam_aim_deg: f32`.
  - `MeetingState.marks: u32`, `current_speaker: Option<u8>`, `ask_deadline_secs: Option<u32>` and `pre_recording_active: bool`.
  - `SettingsState { brightness_pct: u8, dim_after_s: u32, off_after_s: u32, time_zone: String, clock_24h: bool, pre_recording: bool, ask_timeout_s: u32, upload_mode: UploadMode, tones_enabled: bool, first_run_done: bool, pin_enabled: bool }`, with a `Default` of `80, 120, 600, "UTC", true, true, 60, Automatic, true, false, false`.
  - `RecordingStatus { AwaitingDecision, LocalOnly, Waiting, Uploading, Uploaded, Failed, Damaged }` with `needs_attention(&self) -> bool`.
  - `RecordingSummary { id: Uuid, started_at: Option<String>, duration_secs: u32, size_bytes: u64, status: RecordingStatus, marks: u32, last_error: Option<String> }`.
  - `FilesState { recordings: Vec<RecordingSummary>, free_bytes: u64, low_space: bool }`.
  - `PlaybackState { recording: Option<Uuid>, playing: bool, position_ms: u32, duration_ms: u32, marks_ms: Vec<u32> }`.
  - `StateBus.settings: Arc<watch::Sender<SettingsState>>`, `StateBus.files: Arc<watch::Sender<FilesState>>` and `StateBus.playback: Arc<watch::Sender<PlaybackState>>`.

- [ ] **Step 1: Write the failing tests** in `mod tests`:

```rust
    #[test]
    fn state_bus_carries_settings_files_and_playback() {
        let bus = StateBus::new();
        assert_eq!(bus.settings.borrow().brightness_pct, 80);
        assert!(bus.files.borrow().recordings.is_empty());
        assert!(bus.playback.borrow().recording.is_none());
    }

    #[test]
    fn settings_defaults_match_the_design_timings() {
        let s = SettingsState::default();
        assert_eq!((s.dim_after_s, s.off_after_s, s.ask_timeout_s), (120, 600, 60));
        assert!(s.pre_recording && s.clock_24h && s.tones_enabled);
        assert!(!s.first_run_done && !s.pin_enabled);
        assert_eq!(s.time_zone, "UTC");
        assert_eq!(s.upload_mode, UploadMode::Automatic);
    }

    #[test]
    fn new_audio_and_meeting_fields_default_to_idle() {
        let audio = AudioState::default();
        assert!(!audio.host_listening);
        assert_eq!((audio.doa_confidence, audio.beam_aim_deg), (0.0, 0.0));
        let meeting = MeetingState::default();
        assert_eq!(meeting.marks, 0);
        assert!(meeting.current_speaker.is_none());
        assert!(meeting.ask_deadline_secs.is_none());
        assert!(!meeting.pre_recording_active);
    }

    #[test]
    fn only_failed_or_damaged_recordings_need_attention() {
        assert!(RecordingStatus::Failed.needs_attention());
        assert!(RecordingStatus::Damaged.needs_attention());
        for ok in [
            RecordingStatus::AwaitingDecision,
            RecordingStatus::LocalOnly,
            RecordingStatus::Waiting,
            RecordingStatus::Uploading,
            RecordingStatus::Uploaded,
        ] {
            assert!(!ok.needs_attention(), "{ok:?}");
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-state --lib 2>&1 | tail -5`
Expected: compile errors (no field `settings` on `StateBus`, no `SettingsState`, …)

- [ ] **Step 3: Add the fields and types**

In `AudioState`, after `call_host_muted`:

```rust
    /// The host has the USB microphone stream open (UAC "PCM Playback State").
    /// Drives "Computer is using the mic" and the teal ring on Home.
    pub host_listening: bool,
    /// Confidence of the latest DoA estimate, 0..1.
    pub doa_confidence: f32,
    /// Fixed aim for Focused / Narrow / Two sides, degrees clockwise from the
    /// top of the screen (D2).
    pub beam_aim_deg: f32,
```

In `impl Default for AudioState`, after `call_host_muted: false,`:

```rust
            host_listening: false,
            doa_confidence: 0.0,
            beam_aim_deg: 0.0,
```

In `MeetingState`, after `pending_decision`:

```rust
    /// Bookmarks in the active recording.
    pub marks: u32,
    /// Per-meeting speaker number currently talking (diarization), if any.
    pub current_speaker: Option<u8>,
    /// Seconds left before an unanswered "Send this recording?" keeps the file (D4).
    pub ask_deadline_secs: Option<u32>,
    /// The pre-recording buffer is filling (setting on, not recording, not muted).
    pub pre_recording_active: bool,
```

After the `PendingDecision` struct, add:

```rust
/// User-facing settings mirrored from config.toml and persisted by waved-settings.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsState {
    /// Backlight brightness, 5..=100.
    pub brightness_pct: u8,
    /// Seconds without a touch before the resting screen; 0 = never.
    pub dim_after_s: u32,
    /// Seconds without a touch before the screen turns off; 0 = never.
    pub off_after_s: u32,
    /// IANA time zone name.
    pub time_zone: String,
    pub clock_24h: bool,
    pub pre_recording: bool,
    /// Seconds before an unanswered "Send this recording?" keeps the file.
    pub ask_timeout_s: u32,
    pub upload_mode: UploadMode,
    pub tones_enabled: bool,
    pub first_run_done: bool,
    /// A device password is set, so Files, Wi-Fi, Bluetooth, Uploads and System ask for it (D10).
    pub pin_enabled: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            brightness_pct: 80,
            dim_after_s: 120,
            off_after_s: 600,
            time_zone: "UTC".into(),
            clock_24h: true,
            pre_recording: true,
            ask_timeout_s: 60,
            upload_mode: UploadMode::Automatic,
            tones_enabled: true,
            first_run_done: false,
            pin_enabled: false,
        }
    }
}

/// Where a recording stands, as shown in Files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordingStatus {
    AwaitingDecision,
    LocalOnly,
    Waiting,
    Uploading,
    Uploaded,
    Failed,
    Damaged,
}

impl RecordingStatus {
    /// Sorted to the top of Files and shown in amber.
    pub fn needs_attention(&self) -> bool {
        matches!(self, Self::Failed | Self::Damaged)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingSummary {
    pub id: Uuid,
    /// RFC 3339 start time, when the clock was synchronized.
    pub started_at: Option<String>,
    pub duration_secs: u32,
    pub size_bytes: u64,
    pub status: RecordingStatus,
    pub marks: u32,
    /// Plain-language reason for Failed / Damaged.
    pub last_error: Option<String>,
}

/// Recordings on the device, newest first, published by waved-meeting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilesState {
    pub recordings: Vec<RecordingSummary>,
    pub free_bytes: u64,
    /// Free space is below the threshold that blocks new recordings.
    pub low_space: bool,
}

/// Local playback of a recording through the speaker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackState {
    pub recording: Option<Uuid>,
    pub playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
    /// Bookmark offsets from the start of the file.
    pub marks_ms: Vec<u32>,
}
```

In `StateBus`, after `pub puc: …`:

```rust
    pub settings: Arc<watch::Sender<SettingsState>>,
    pub files: Arc<watch::Sender<FilesState>>,
    pub playback: Arc<watch::Sender<PlaybackState>>,
```

In `StateBus::new`, after `let (puc_tx, _) = …;`:

```rust
        let (settings_tx, _) = watch::channel(SettingsState::default());
        let (files_tx, _) = watch::channel(FilesState::default());
        let (playback_tx, _) = watch::channel(PlaybackState::default());
```

and in the returned struct, after `puc: Arc::new(puc_tx),`:

```rust
            settings: Arc::new(settings_tx),
            files: Arc::new(files_tx),
            playback: Arc::new(playback_tx),
```

- [ ] **Step 4: Run the tests and check every dependant**

Run: `set -o pipefail; cargo test -p waved-state --lib 2>&1 | grep "test result"`
Expected: `ok. 24 passed`

Run: `set -o pipefail; cargo check -p waved-config -p waved-ui -p waved-provision -p waved-hid -p waved-wifi -p waved-mcu -p waved-bt 2>&1 | tail -1`
Expected: `Finished`

Run: `set -o pipefail; scripts/linux-test.sh check -p waved --features ui,capture-alsa 2>&1 | tail -1`
Expected: `Finished` (every `AudioState`/`MeetingState` literal uses `..`)

- [ ] **Step 5: Lint and commit**

Run: `cargo clippy -p waved-state --all-targets -- -D warnings`

```bash
git add crates/waved-state/src/lib.rs
git commit -m "feat(state): add settings, files and playback state for the redesign

- SettingsState mirrors the user-facing config (display, time, recording policy, pin)
- FilesState and PlaybackState feed Files and recording playback
- AudioState gains host_listening, doa_confidence, beam_aim_deg
- MeetingState gains marks, current_speaker, ask_deadline_secs, pre_recording_active

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Commands for milestone 2

Adding variants is safe: every match on `Command` has a `_` arm. The legacy UI-only `Open*` variants stay until the milestone-2 runtime switch.

How the rest of P0.1 in `../TODO.md` is covered:
- `Navigate` and `Back` are not bus commands. Navigation stays inside the UI as `UiEvent`s (Task 13) handled by the `Router` (Task 14).
- `WakeScreen` and `SleepScreen` become `UiEvent::Wake` and `UiEvent::Rest` (Task 13). If the power-key service in workstream E needs to reach the UI, the integrator adds that command in milestone 2.
- `SetAim(Option<f32>)` becomes `SetBeamAim(f32)`. "Follow talker" is its own audio mode (D2), so there's no aim to clear.
- The commands and state for USB storage, Bluetooth, calls and updates (`StartUsbStorage`, `StopUsbStorage`, `BtSetDiscoverable`, `BtConnect`, `BtDisconnect`, `BtForget`, `AnswerCall`, `DeclineCall`, `UsbMode`, the Bluetooth device list, `OtaState`) are added by the integrator when milestone 3 starts. That way their shapes match the backends that use them.

**Files:**
- Modify: `crates/waved-state/src/lib.rs` (`Command`, after `RestartForConfig`)
- Test: `crates/waved-state/src/lib.rs` (`mod tests`)

**Interfaces:**
- Produces the new variants `SetBrightness(u8)`, `SetDimAfter(u32)`, `SetTimeZone(String)`, `SetClock24h(bool)`, `SetPreRecording(bool)`, `SetUploadMode(UploadMode)`, `SetAlertTones(bool)`, `MarkFirstRunDone`, `SetBeamAim(f32)`, `DeleteRecording(Uuid)`, `PlayRecording(Uuid)`, `PausePlayback`, `ResumePlayback`, `SeekPlayback(u32)`, `StopPlayback` and `FactoryReset`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn redesign_commands_exist_and_clone() {
        let id = Uuid::new_v4();
        let commands = vec![
            Command::SetBrightness(80),
            Command::SetDimAfter(120),
            Command::SetTimeZone("Europe/Brussels".into()),
            Command::SetClock24h(true),
            Command::SetPreRecording(false),
            Command::SetUploadMode(UploadMode::Ask),
            Command::SetAlertTones(true),
            Command::MarkFirstRunDone,
            Command::SetBeamAim(300.0),
            Command::DeleteRecording(id),
            Command::PlayRecording(id),
            Command::PausePlayback,
            Command::ResumePlayback,
            Command::SeekPlayback(5_000),
            Command::StopPlayback,
            Command::FactoryReset,
        ];
        let cloned = commands.clone();
        assert_eq!(cloned.len(), 16);
        assert!(matches!(cloned[9], Command::DeleteRecording(x) if x == id));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `set -o pipefail; cargo test -p waved-state --lib redesign_commands 2>&1 | tail -3`
Expected: compile error `no variant named \`SetBrightness\``

- [ ] **Step 3: Add the variants** after `RestartForConfig,`:

```rust
    // --- Settings (persisted by waved-settings) ---
    /// Backlight brightness in percent; the settings service clamps to 5..=100.
    SetBrightness(u8),
    /// Seconds without a touch before the resting screen; 0 = never.
    SetDimAfter(u32),
    /// IANA time zone name, e.g. "Europe/Brussels".
    SetTimeZone(String),
    SetClock24h(bool),
    /// Keep the last 30 s of audio in RAM so Record can include it (D3).
    SetPreRecording(bool),
    SetUploadMode(UploadMode),
    SetAlertTones(bool),
    /// The first-start screen finished (Wi-Fi set up or skipped).
    MarkFirstRunDone,
    /// Fixed aim for Focused / Narrow / Two sides, degrees clockwise from the
    /// top of the screen (D2).
    SetBeamAim(f32),
    // --- Recordings ---
    /// Delete any kept or failed recording from Files. Unlike DiscardRecording
    /// this is not limited to the pending decision.
    DeleteRecording(Uuid),
    PlayRecording(Uuid),
    PausePlayback,
    ResumePlayback,
    /// Seek the active playback to this offset, in milliseconds from the start of the file.
    SeekPlayback(u32),
    StopPlayback,
    // --- System ---
    /// Erase config, recordings, Wi-Fi and Bluetooth pairings, then restart.
    FactoryReset,
```

- [ ] **Step 4: Run the tests and dependants**

Run: `set -o pipefail; cargo test -p waved-state --lib 2>&1 | grep "test result"`
Expected: `ok. 25 passed`

Run: `set -o pipefail; cargo check -p waved-ui -p waved-provision -p waved-mcu 2>&1 | tail -1 && scripts/linux-test.sh check -p waved --features ui,capture-alsa 2>&1 | tail -1`
Expected: `Finished` twice

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-state --all-targets -- -D warnings
git add crates/waved-state/src/lib.rs
git commit -m "feat(state): add commands for settings, playback and factory reset

- settings commands are persisted by the coming waved-settings service
- DeleteRecording works for any recording, not just the pending decision
- playback commands drive the Files detail scrubber

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Config v2 schema, migration and validation

**Files:**
- Modify: `crates/waved-config/src/lib.rs`
  - version constant: line 15
  - structs: lines 32-98
  - defaults: lines 162-224
  - re-exports: lines 6-9
  - tests: lines 226-324
- Modify: `crates/waved-config/src/migrate.rs`
- Modify: `crates/waved-config/src/validate.rs`
- Modify: `config/default.toml`
- Test: `crates/waved-config/src/lib.rs` and `crates/waved-config/src/validate.rs` (`mod tests`)

**Interfaces:**
- Consumes: `waved_state::{AudioMode, NrLevel, UploadMode}`.
- Produces:
  - `CURRENT_CONFIG_VERSION = 2`.
  - `WavedConfig { config_version, device, audio, display, time, recording, tones, api, storage, security, ntp }`.
  - `DeviceConfig.first_run_done: bool`.
  - `AudioConfig { mode, speaker_volume /* 0..=100 */, mic_volume, nr_level: NrLevel, beam_aim_deg: f32 }`.
  - `DisplayConfig { brightness_pct: u8, dim_after_s: u32, off_after_s: u32 }`.
  - `TimeConfig { zone: String, clock_24h: bool }`.
  - `RecordingConfig { pre_recording: bool, ask_timeout_s: u32 }`.
  - `TonesConfig { enabled: bool }`.
  - `pub fn valid_time_zone(zone: &str) -> bool`, re-exported from the crate root.

- [ ] **Step 1: Write the failing tests**

In `crates/waved-config/src/lib.rs` `mod tests`, change `unknown_future_version_is_rejected` to replace `"config_version = 2"` (instead of `= 1`), then add:

```rust
    #[test]
    fn v1_config_migrates_to_v2_and_keeps_user_values() {
        let source = r#"config_version = 1

[device]
name = "Board room"
language = "en"

[audio]
mode = "cardioid"
speaker_volume = 150
mic_volume = 300
"#;
        let config = WavedConfig::parse(source).unwrap();
        assert_eq!(config.config_version, 2);
        assert_eq!(config.device.name, "Board room");
        assert!(config.device.first_run_done, "an existing install skips first start");
        assert_eq!(config.audio.mode, AudioMode::Cardioid);
        assert_eq!(config.audio.speaker_volume, 100, "v1 0..=200 clamps to percent");
        assert_eq!(config.audio.mic_volume, 300);
        assert_eq!(config.display, DisplayConfig::default());
        assert_eq!(config.recording, RecordingConfig::default());
        assert_eq!(config.time, TimeConfig::default());
    }

    #[test]
    fn new_installs_show_first_start() {
        assert!(!WavedConfig::default().device.first_run_done);
        assert_eq!(WavedConfig::default().config_version, 2);
    }
```

In `crates/waved-config/src/validate.rs` `mod tests`, add:

```rust
    fn rejects(config: &WavedConfig, path: &str) -> bool {
        validate_config(config)
            .err()
            .is_some_and(|errors| errors.iter().any(|error| error.path == path))
    }

    #[test]
    fn display_time_recording_and_audio_ranges_are_enforced() {
        let mut c = WavedConfig::default();
        c.display.brightness_pct = 0;
        assert!(rejects(&c, "display.brightness_pct"));

        let mut c = WavedConfig::default();
        c.display.dim_after_s = 5;
        assert!(rejects(&c, "display.dim_after_s"));

        let mut c = WavedConfig::default();
        c.display.dim_after_s = 600;
        c.display.off_after_s = 300;
        assert!(rejects(&c, "display.off_after_s"));

        let mut c = WavedConfig::default();
        c.display.dim_after_s = 0;
        c.display.off_after_s = 0;
        assert!(validate_config(&c).is_ok(), "never dim and never off is allowed");

        let mut c = WavedConfig::default();
        c.time.zone = "../etc/passwd".into();
        assert!(rejects(&c, "time.zone"));

        let mut c = WavedConfig::default();
        c.time.zone = "America/Argentina/Buenos_Aires".into();
        assert!(validate_config(&c).is_ok());

        let mut c = WavedConfig::default();
        c.recording.ask_timeout_s = 5;
        assert!(rejects(&c, "recording.ask_timeout_s"));

        let mut c = WavedConfig::default();
        c.audio.speaker_volume = 101;
        assert!(rejects(&c, "audio.speaker_volume"));

        let mut c = WavedConfig::default();
        c.audio.beam_aim_deg = 360.0;
        assert!(rejects(&c, "audio.beam_aim_deg"));
    }

    #[test]
    fn time_zone_names_are_plain_iana_identifiers() {
        for good in ["UTC", "Europe/Brussels", "Etc/GMT+1", "America/Port-au-Prince"] {
            assert!(valid_time_zone(good), "{good}");
        }
        for bad in ["", "/etc/localtime", "Europe/../x", "Europe Brussels", &"A".repeat(65)] {
            assert!(!valid_time_zone(bad), "{bad}");
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-config --lib 2>&1 | tail -5`
Expected: compile errors (`DisplayConfig`, `valid_time_zone`, `first_run_done` unknown)

- [ ] **Step 3: Implement the v2 schema** in `crates/waved-config/src/lib.rs`

Change the re-export and imports at the top:

```rust
pub use validate::{valid_time_zone, validate_config};
```

```rust
use waved_state::{AudioMode, NrLevel};
pub use waved_state::UploadMode;
```

Set the version: `pub const CURRENT_CONFIG_VERSION: u32 = 2;`

Replace `WavedConfig`, `DeviceConfig` and `AudioConfig` with these, and add the four new sections right after `AudioConfig`:

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WavedConfig {
    pub config_version: u32,
    pub device: DeviceConfig,
    pub audio: AudioConfig,
    pub display: DisplayConfig,
    pub time: TimeConfig,
    pub recording: RecordingConfig,
    pub tones: TonesConfig,
    pub api: ApiConfig,
    pub storage: StorageConfig,
    pub security: SecurityConfig,
    pub ntp: NtpConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceConfig {
    pub name: String,
    pub language: String,
    /// The first-start screen has been finished or skipped.
    pub first_run_done: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioConfig {
    pub mode: AudioMode,
    /// Speaker volume in percent, 0..=100.
    pub speaker_volume: u16,
    /// Linear microphone gain in percent; 100 = unity.
    pub mic_volume: u16,
    pub nr_level: NrLevel,
    /// Fixed aim for Focused / Narrow / Two sides, degrees clockwise from the
    /// top of the screen, 0.0..360.0.
    pub beam_aim_deg: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    pub brightness_pct: u8,
    /// Seconds without a touch before the resting screen; 0 = never.
    pub dim_after_s: u32,
    /// Seconds without a touch before the screen turns off; 0 = never.
    pub off_after_s: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimeConfig {
    /// IANA time zone name.
    pub zone: String,
    pub clock_24h: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecordingConfig {
    /// Keep the last 30 s of audio in RAM so Record can include it.
    pub pre_recording: bool,
    /// Seconds before an unanswered "Send this recording?" keeps the file.
    pub ask_timeout_s: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TonesConfig {
    pub enabled: bool,
}
```

Replace `impl Default for WavedConfig`, `DeviceConfig` and `AudioConfig`, and add defaults for the new sections:

```rust
impl Default for WavedConfig {
    fn default() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            device: DeviceConfig::default(),
            audio: AudioConfig::default(),
            display: DisplayConfig::default(),
            time: TimeConfig::default(),
            recording: RecordingConfig::default(),
            tones: TonesConfig::default(),
            api: ApiConfig::default(),
            storage: StorageConfig::default(),
            security: SecurityConfig::default(),
            ntp: NtpConfig::default(),
        }
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: "Wave Open".into(),
            language: "en".into(),
            first_run_done: false,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mode: AudioMode::Omni,
            speaker_volume: 100,
            mic_volume: 200,
            nr_level: NrLevel::Off,
            beam_aim_deg: 0.0,
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            brightness_pct: 80,
            dim_after_s: 120,
            off_after_s: 600,
        }
    }
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            zone: "UTC".into(),
            clock_24h: true,
        }
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            pre_recording: true,
            ask_timeout_s: 60,
        }
    }
}

impl Default for TonesConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}
```

- [ ] **Step 4: Add the v1 → v2 migration** in `crates/waved-config/src/migrate.rs`

Replace the `if version == 0 { … }` block with:

```rust
    if version == 0 {
        migrate_v0_to_v1(root);
    }
    if version <= 1 {
        migrate_v1_to_v2(root);
    }
```

Add:

```rust
fn migrate_v1_to_v2(root: &mut toml::map::Map<String, toml::Value>) {
    // v1 allowed speaker_volume 0..=200 but never applied it; v2 is a percent.
    if let Some(audio) = root.get_mut("audio").and_then(toml::Value::as_table_mut) {
        if let Some(volume) = audio.get("speaker_volume").and_then(toml::Value::as_integer) {
            audio.insert(
                "speaker_volume".into(),
                toml::Value::Integer(volume.clamp(0, 100)),
            );
        }
    }
    // Anyone with a v1 file has already used the device: skip first start.
    table_or_insert(root, "device").insert("first_run_done".into(), toml::Value::Boolean(true));
}
```

The new sections need no migration: every struct is `#[serde(default)]`, so missing sections take their defaults.

- [ ] **Step 5: Validate the new fields** in `crates/waved-config/src/validate.rs`

Replace the `speaker_volume > 200` block with:

```rust
    if config.audio.speaker_volume > 100 {
        push(
            &mut errors,
            "audio.speaker_volume",
            "must be between 0 and 100",
        );
    }
    if !config.audio.beam_aim_deg.is_finite() || !(0.0..360.0).contains(&config.audio.beam_aim_deg) {
        push(
            &mut errors,
            "audio.beam_aim_deg",
            "must be at least 0 and below 360",
        );
    }
    if !(5..=100).contains(&config.display.brightness_pct) {
        push(&mut errors, "display.brightness_pct", "must be between 5 and 100");
    }
    let dim = config.display.dim_after_s;
    let off = config.display.off_after_s;
    if dim != 0 && !(15..=3_600).contains(&dim) {
        push(&mut errors, "display.dim_after_s", "must be 0 (never) or 15 to 3600");
    }
    if off != 0 && (!(60..=86_400).contains(&off) || (dim != 0 && off < dim)) {
        push(
            &mut errors,
            "display.off_after_s",
            "must be 0 (never) or 60 to 86400, and not before dimming",
        );
    }
    if !valid_time_zone(&config.time.zone) {
        push(&mut errors, "time.zone", "must be an IANA time zone such as Europe/Brussels");
    }
    if !(15..=600).contains(&config.recording.ask_timeout_s) {
        push(&mut errors, "recording.ask_timeout_s", "must be between 15 and 600");
    }
```

Add the public helper below `validate_config`:

```rust
/// A plain IANA identifier: letters, digits and `/ _ - +`, no path tricks.
pub fn valid_time_zone(zone: &str) -> bool {
    (1..=64).contains(&zone.len())
        && !zone.starts_with('/')
        && !zone.contains("..")
        && zone
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '+'))
}
```

- [ ] **Step 6: Update `config/default.toml`**

```toml
config_version = 2

[device]
name = "Wave Open"
language = "en"
first_run_done = false

[audio]
mode = "omni"
speaker_volume = 100
mic_volume = 200
nr_level = "off"
beam_aim_deg = 0.0

[display]
brightness_pct = 80
dim_after_s = 120
off_after_s = 600

[time]
zone = "UTC"
clock_24h = true

[recording]
pre_recording = true
ask_timeout_s = 60

[tones]
enabled = true

[api]
endpoint = ""
auth_token = ""
upload_mode = "automatic"
allow_insecure_http = false
retry_max = 5
retry_backoff_ms = 1000
retry_max_backoff_ms = 60000

[storage]
prune_uploaded_at_percent = 85
minimum_free_mb = 2048

[security]
device_password = ""
ota_server = ""

[ntp]
servers = ["pool.ntp.org", "time.google.com"]
```

- [ ] **Step 7: Run the tests**

Run: `set -o pipefail; cargo test -p waved-config --lib 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: `ok. 15 passed`. If `applies_webhook_and_steered_fields` fails on `speaker_volume`, check that no submission sets a value above 100; Task 6 lowers the form maximum.

Run: `set -o pipefail; cargo check -p waved-provision -p waved-ui -p waved-wifi 2>&1 | tail -1 && scripts/linux-test.sh test -p waved-meeting --lib 2>&1 | tail -2`
Expected: `Finished`, then `ok. 67 passed`

- [ ] **Step 8: Lint and commit**

```bash
cargo clippy -p waved-config --all-targets -- -D warnings
git add crates/waved-config/src config/default.toml
git commit -m "feat(config): v2 schema for display, time, recording and tones

- new sections with defaults; audio gains nr_level and beam_aim_deg
- v1 files migrate: speaker_volume clamps to percent, first start is skipped
- validation for brightness, dim/off timers, IANA zone, ask timeout, aim

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Setup-page fields for the new settings

**Files:**
- Modify: `crates/waved-config/src/form.rs`
  - option constants: lines 48-55
  - `FIELD_SPECS`: lines 56-228
  - `apply_form`: lines 246-354
  - tests: lines 436-512
- Modify: `crates/waved-provision/src/setup_form.rs` (`fn value`, lines 220-245, and its tests)
- Test: both files' `mod tests`

**Interfaces:**
- Consumes: Task 5's config sections.
- Produces registry paths `audio.nr_level`, `display.brightness_pct`, `display.dim_after_s`, `display.off_after_s`, `time.zone`, `time.clock_24h`, `tones.enabled`, `recording.pre_recording` and `recording.ask_timeout_s`. The `audio.speaker_volume` maximum becomes 100.

- [ ] **Step 1: Write the failing tests**

In `form.rs` `mod tests`, replace the `expected` array in `registry_covers_supported_config_paths` with:

```rust
        let expected = [
            "device.name",
            "device.language",
            "display.brightness_pct",
            "display.dim_after_s",
            "display.off_after_s",
            "time.zone",
            "time.clock_24h",
            "tones.enabled",
            "audio.mode",
            "audio.speaker_volume",
            "audio.mic_volume",
            "audio.nr_level",
            "api.endpoint",
            "api.auth_token",
            "api.upload_mode",
            "recording.pre_recording",
            "recording.ask_timeout_s",
            "api.allow_insecure_http",
            "api.retry_max",
            "api.retry_backoff_ms",
            "api.retry_max_backoff_ms",
            "storage.prune_uploaded_at_percent",
            "storage.minimum_free_mb",
            "security.device_password",
            "security.ota_server",
            "ntp.servers",
        ];
```

and add:

```rust
    #[test]
    fn applies_display_time_recording_and_noise_fields() {
        let mut submission = FormSubmission::default();
        for (path, value) in [
            ("display.brightness_pct", "60"),
            ("display.dim_after_s", "300"),
            ("display.off_after_s", "1800"),
            ("time.zone", "Europe/Brussels"),
            ("time.clock_24h", "false"),
            ("tones.enabled", "false"),
            ("audio.nr_level", "med"),
            ("recording.pre_recording", "false"),
            ("recording.ask_timeout_s", "120"),
        ] {
            submission.values.insert(path.into(), value.into());
        }
        let next = apply_form(&WavedConfig::default(), &submission).unwrap();
        assert_eq!(next.display.brightness_pct, 60);
        assert_eq!((next.display.dim_after_s, next.display.off_after_s), (300, 1800));
        assert_eq!(next.time.zone, "Europe/Brussels");
        assert!(!next.time.clock_24h && !next.tones.enabled && !next.recording.pre_recording);
        assert_eq!(next.audio.nr_level, NrLevel::Med);
        assert_eq!(next.recording.ask_timeout_s, 120);
    }

    #[test]
    fn rejects_an_unknown_noise_level() {
        let mut submission = FormSubmission::default();
        submission.values.insert("audio.nr_level".into(), "max".into());
        let errors = apply_form(&WavedConfig::default(), &submission).unwrap_err();
        assert!(errors.iter().any(|error| error.path == "audio.nr_level"));
    }
```

In `crates/waved-provision/src/setup_form.rs` `mod tests`, add a test that guards against the silent `_ => String::new()` fallback:

```rust
    #[test]
    fn every_registry_field_renders_its_saved_value() {
        let config = WavedConfig::default();
        for spec in field_specs() {
            let may_be_blank = matches!(spec.kind, FieldKind::Secret { .. })
                || matches!(spec.path, "audio.mode" | "api.endpoint" | "security.ota_server");
            if !may_be_blank {
                assert!(!value(&config, spec.path).is_empty(), "no value for {}", spec.path);
            }
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-config -p waved-provision --lib 2>&1 | grep -E "FAILED|panicked|error\[" | head`
Expected: the registry test fails, `NrLevel` isn't imported in `form.rs`, and `every_registry_field_renders_its_saved_value` fails once the specs exist.

- [ ] **Step 3: Add the option lists and field specs** in `form.rs`

Add `NrLevel` to the `waved_state` import at the top of `form.rs` (it already imports `AudioMode`):

```rust
use waved_state::{AudioMode, NrLevel};
```

Below `UPLOAD_MODE_OPTIONS`:

```rust
const NR_LEVEL_OPTIONS: &[(&str, &str)] = &[
    ("off", "Off"),
    ("low", "Low"),
    ("med", "Medium"),
    ("high", "High"),
];
const DIM_AFTER_OPTIONS: &[(&str, &str)] = &[
    ("0", "Never"),
    ("60", "1 min"),
    ("120", "2 min"),
    ("300", "5 min"),
    ("600", "10 min"),
];
const OFF_AFTER_OPTIONS: &[(&str, &str)] = &[
    ("0", "Never"),
    ("300", "5 min"),
    ("600", "10 min"),
    ("1800", "30 min"),
    ("3600", "1 hour"),
];
```

In `FIELD_SPECS`, insert after the `device.language` spec:

```rust
    FieldSpec {
        path: "display.brightness_pct",
        section: Section::Device,
        label: "Screen brightness",
        help: "Backlight level in percent.",
        kind: FieldKind::Integer { min: 5, max: 100, step: 5 },
        required: true,
    },
    FieldSpec {
        path: "display.dim_after_s",
        section: Section::Device,
        label: "Dim after",
        help: "Time without a touch before the resting screen.",
        kind: FieldKind::Select { options: DIM_AFTER_OPTIONS },
        required: true,
    },
    FieldSpec {
        path: "display.off_after_s",
        section: Section::Device,
        label: "Screen off after",
        help: "Never turns off while recording or in a call.",
        kind: FieldKind::Select { options: OFF_AFTER_OPTIONS },
        required: true,
    },
    FieldSpec {
        path: "time.zone",
        section: Section::Device,
        label: "Time zone",
        help: "IANA name, for example Europe/Brussels.",
        kind: FieldKind::Text { max_len: 64 },
        required: true,
    },
    FieldSpec {
        path: "time.clock_24h",
        section: Section::Device,
        label: "24-hour clock",
        help: "Show 14:30 instead of 2:30 pm.",
        kind: FieldKind::Boolean,
        required: false,
    },
    FieldSpec {
        path: "tones.enabled",
        section: Section::Device,
        label: "Alert tones",
        help: "Sounds for mute, volume and recording start and stop.",
        kind: FieldKind::Boolean,
        required: false,
    },
```

Insert after the `audio.mic_volume` spec, and change the `audio.speaker_volume` spec's `max: 200` to `max: 100`:

```rust
    FieldSpec {
        path: "audio.nr_level",
        section: Section::Audio,
        label: "Noise reduction",
        help: "How much background noise to remove.",
        kind: FieldKind::Select { options: NR_LEVEL_OPTIONS },
        required: true,
    },
```

Insert after the `api.upload_mode` spec:

```rust
    FieldSpec {
        path: "recording.pre_recording",
        section: Section::Webhook,
        label: "Pre-recording",
        help: "Keep the last 30 seconds in memory so Record includes them.",
        kind: FieldKind::Boolean,
        required: false,
    },
    FieldSpec {
        path: "recording.ask_timeout_s",
        section: Section::Webhook,
        label: "Keep after (seconds)",
        help: "With Ask, an unanswered recording is kept on the Wave after this time.",
        kind: FieldKind::Integer { min: 15, max: 600, step: 15 },
        required: true,
    },
```

- [ ] **Step 4: Parse the new fields** in `apply_form`, directly after the `device.language` `text(…)` call:

```rust
    parse_number(&submission.values, "display.brightness_pct", &mut next.display.brightness_pct, &mut errors);
    parse_number(&submission.values, "display.dim_after_s", &mut next.display.dim_after_s, &mut errors);
    parse_number(&submission.values, "display.off_after_s", &mut next.display.off_after_s, &mut errors);
    text(&submission.values, "time.zone", &mut next.time.zone);
    bool_value(&submission.values, "time.clock_24h", &mut next.time.clock_24h);
    bool_value(&submission.values, "tones.enabled", &mut next.tones.enabled);
    if let Some(value) = submission.values.get("audio.nr_level") {
        next.audio.nr_level = match value.as_str() {
            "off" => NrLevel::Off,
            "low" => NrLevel::Low,
            "med" => NrLevel::Med,
            "high" => NrLevel::High,
            _ => {
                errors.push(FieldError {
                    path: "audio.nr_level",
                    message: "unknown noise reduction level".into(),
                });
                next.audio.nr_level.clone()
            }
        };
    }
    bool_value(&submission.values, "recording.pre_recording", &mut next.recording.pre_recording);
    parse_number(&submission.values, "recording.ask_timeout_s", &mut next.recording.ask_timeout_s, &mut errors);
```

- [ ] **Step 5: Render the saved values** in `setup_form.rs` `fn value`, before the `_ => String::new(),` arm:

```rust
        "display.brightness_pct" => config.display.brightness_pct.to_string(),
        "display.dim_after_s" => config.display.dim_after_s.to_string(),
        "display.off_after_s" => config.display.off_after_s.to_string(),
        "time.zone" => config.time.zone.clone(),
        "time.clock_24h" => config.time.clock_24h.to_string(),
        "tones.enabled" => config.tones.enabled.to_string(),
        "audio.nr_level" => match config.audio.nr_level {
            NrLevel::Off => "off",
            NrLevel::Low => "low",
            NrLevel::Med => "med",
            NrLevel::High => "high",
        }
        .into(),
        "recording.pre_recording" => config.recording.pre_recording.to_string(),
        "recording.ask_timeout_s" => config.recording.ask_timeout_s.to_string(),
```

Add `NrLevel` to that file's `waved_state` import (it already imports `AudioMode`). Make sure the `tests` module imports `field_specs` and `FieldKind`, which the file uses already: `use super::*;`.

- [ ] **Step 6: Run the tests**

Run: `set -o pipefail; cargo test -p waved-config -p waved-provision --lib 2>&1 | grep -E "test result|FAILED"`
Expected: both report `ok`, with no `FAILED`

- [ ] **Step 7: Lint and commit**

```bash
cargo clippy -p waved-config -p waved-provision --all-targets -- -D warnings
git add crates/waved-config/src/form.rs crates/waved-provision/src/setup_form.rs
git commit -m "feat(setup): edit display, time, tones, noise and recording settings

- nine new setup-page fields; speaker volume maximum is now 100 percent
- guard test: every registry field renders its saved value

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `waved-settings` crate — boot seeding and the command-to-setting mapping

**Files:**
- Create: `crates/waved-settings/Cargo.toml`
- Create: `crates/waved-settings/src/lib.rs`
- Create: `crates/waved-settings/src/change.rs`
- Test: both source files (`mod tests`)

**Interfaces:**
- Consumes:
  - `waved_config::{WavedConfig, valid_time_zone}`
  - `waved_state::{StateBus, SettingsState, Command, AudioMode, NrLevel, UploadMode}`
- Produces:
  - `pub fn settings_from_config(config: &WavedConfig) -> SettingsState`
  - `pub fn seed_state(bus: &StateBus, config: &WavedConfig)`: copies `audio.{mode, nr_level, speaker_volume, mic_volume, beam_aim_deg}` into `AudioState` and the user-facing settings into `bus.settings`.
  - `pub enum Change { Brightness(u8), DimAfter(u32), TimeZone(String), Clock24h(bool), PreRecording(bool), UploadMode(UploadMode), Tones(bool), FirstRunDone, AudioMode(AudioMode), NrLevel(NrLevel), SpeakerVolume(u16), MicVolume(u16), BeamAim(f32) }`
  - `Change::from_command(&Command) -> Option<Change>`, which clamps out-of-range values and returns `None` for invalid time zones and non-finite aims.
  - `pub fn apply_to_config(&mut WavedConfig, &Change)`
  - `pub fn apply_to_settings(&mut SettingsState, &Change)`

- [ ] **Step 1: Create the crate manifest** `crates/waved-settings/Cargo.toml`:

```toml
[package]
name = "waved-settings"
version.workspace = true
edition.workspace = true

[dependencies]
tokio = { workspace = true }
tracing.workspace = true
waved-state = { path = "../waved-state" }
waved-config = { path = "../waved-config" }

[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
tempfile = "3"
uuid.workspace = true
```

The workspace uses `members = ["crates/*"]`, so the crate is picked up automatically.

- [ ] **Step 2: Write the failing tests**

`crates/waved-settings/src/lib.rs`:

```rust
//! Owns the user-facing settings: seeds live state from config.toml at boot
//! and persists changes made on the device back to it.

mod change;

pub use change::{apply_to_config, apply_to_settings, Change};

use waved_config::WavedConfig;
use waved_state::{SettingsState, StateBus};

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::{AudioMode, NrLevel, UploadMode};

    fn configured() -> WavedConfig {
        let mut config = WavedConfig::default();
        config.audio.mode = AudioMode::Cardioid;
        config.audio.nr_level = NrLevel::High;
        config.audio.speaker_volume = 40;
        config.audio.mic_volume = 300;
        config.audio.beam_aim_deg = 270.0;
        config.display.brightness_pct = 55;
        config.time.zone = "Europe/Brussels".into();
        config.recording.pre_recording = false;
        config.api.upload_mode = UploadMode::Ask;
        config.device.first_run_done = true;
        config.security.device_password = "4821".into();
        config
    }

    #[test]
    fn settings_mirror_the_config() {
        let settings = settings_from_config(&configured());
        assert_eq!(settings.brightness_pct, 55);
        assert_eq!(settings.time_zone, "Europe/Brussels");
        assert!(!settings.pre_recording);
        assert_eq!(settings.upload_mode, UploadMode::Ask);
        assert!(settings.first_run_done && settings.pin_enabled);
    }

    #[test]
    fn seed_copies_audio_and_settings_into_the_bus() {
        let bus = StateBus::new();
        let _keep_audio = bus.audio.subscribe();
        seed_state(&bus, &configured());
        let audio = bus.audio.borrow().clone();
        assert_eq!(audio.mode, AudioMode::Cardioid);
        assert_eq!(audio.nr_level, NrLevel::High);
        assert_eq!((audio.speaker_volume, audio.mic_volume), (40, 300));
        assert_eq!(audio.beam_aim_deg, 270.0);
        assert_eq!(bus.settings.borrow().brightness_pct, 55);
    }
}
```

`crates/waved-settings/src/change.rs`:

```rust
//! One persisted setting, mapped from a bus command and clamped to its range.

use waved_config::WavedConfig;
use waved_state::{AudioMode, Command, NrLevel, SettingsState, UploadMode};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_commands_are_clamped_or_ignored() {
        assert_eq!(Change::from_command(&Command::SetBrightness(0)), Some(Change::Brightness(5)));
        assert_eq!(Change::from_command(&Command::SetBrightness(250)), Some(Change::Brightness(100)));
        assert_eq!(Change::from_command(&Command::SetSpeakerVolume(250)), Some(Change::SpeakerVolume(100)));
        assert_eq!(Change::from_command(&Command::SetMicVolume(5_000)), Some(Change::MicVolume(800)));
        assert_eq!(Change::from_command(&Command::SetDimAfter(5)), Some(Change::DimAfter(15)));
        assert_eq!(Change::from_command(&Command::SetDimAfter(0)), Some(Change::DimAfter(0)));
        assert_eq!(Change::from_command(&Command::SetBeamAim(-60.0)), Some(Change::BeamAim(300.0)));
        assert_eq!(Change::from_command(&Command::SetBeamAim(720.0)), Some(Change::BeamAim(0.0)));
        assert_eq!(Change::from_command(&Command::SetBeamAim(f32::NAN)), None);
        assert_eq!(Change::from_command(&Command::SetTimeZone("../etc".into())), None);
        assert_eq!(Change::from_command(&Command::ToggleMicMute), None);
    }

    #[test]
    fn longer_dimming_pushes_screen_off_later() {
        let mut config = WavedConfig::default();
        config.display.off_after_s = 300;
        apply_to_config(&mut config, &Change::DimAfter(600));
        assert_eq!(config.display.dim_after_s, 600);
        assert_eq!(config.display.off_after_s, 600, "never off before dimming");
        assert!(waved_config::validate_config(&config).is_ok());
    }

    #[test]
    fn audio_changes_touch_config_but_not_settings() {
        let mut config = WavedConfig::default();
        let mut settings = SettingsState::default();
        let change = Change::SpeakerVolume(30);
        apply_to_config(&mut config, &change);
        apply_to_settings(&mut settings, &change);
        assert_eq!(config.audio.speaker_volume, 30);
        assert_eq!(settings, SettingsState::default());
    }

    #[test]
    fn settings_changes_reach_both() {
        let mut config = WavedConfig::default();
        let mut settings = SettingsState::default();
        for change in [
            Change::UploadMode(UploadMode::LocalOnly),
            Change::PreRecording(false),
            Change::FirstRunDone,
        ] {
            apply_to_config(&mut config, &change);
            apply_to_settings(&mut settings, &change);
        }
        assert_eq!(config.api.upload_mode, UploadMode::LocalOnly);
        assert_eq!(settings.upload_mode, UploadMode::LocalOnly);
        assert!(!config.recording.pre_recording && !settings.pre_recording);
        assert!(config.device.first_run_done && settings.first_run_done);
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-settings --lib 2>&1 | tail -5`
Expected: compile errors (`settings_from_config`, `seed_state`, `Change` not defined)

- [ ] **Step 4: Implement** in `lib.rs`, above `#[cfg(test)]`:

```rust
/// User-facing settings derived from config. `pin_enabled` mirrors whether a
/// device password is set.
pub fn settings_from_config(config: &WavedConfig) -> SettingsState {
    SettingsState {
        brightness_pct: config.display.brightness_pct,
        dim_after_s: config.display.dim_after_s,
        off_after_s: config.display.off_after_s,
        time_zone: config.time.zone.clone(),
        clock_24h: config.time.clock_24h,
        pre_recording: config.recording.pre_recording,
        ask_timeout_s: config.recording.ask_timeout_s,
        upload_mode: config.api.upload_mode,
        tones_enabled: config.tones.enabled,
        first_run_done: config.device.first_run_done,
        pin_enabled: !config.security.device_password.is_empty(),
    }
}

/// Copy persisted values into live state. Call before any service is spawned:
/// AudioService reads mode and noise reduction from the watch when it is built.
pub fn seed_state(bus: &StateBus, config: &WavedConfig) {
    bus.audio.send_modify(|audio| {
        audio.mode = config.audio.mode.clone();
        audio.nr_level = config.audio.nr_level.clone();
        audio.speaker_volume = config.audio.speaker_volume;
        audio.mic_volume = config.audio.mic_volume;
        audio.beam_aim_deg = config.audio.beam_aim_deg;
    });
    bus.settings.send_replace(settings_from_config(config));
}
```

In `change.rs`, above `#[cfg(test)]`:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Brightness(u8),
    DimAfter(u32),
    TimeZone(String),
    Clock24h(bool),
    PreRecording(bool),
    UploadMode(UploadMode),
    Tones(bool),
    FirstRunDone,
    AudioMode(AudioMode),
    NrLevel(NrLevel),
    SpeakerVolume(u16),
    MicVolume(u16),
    BeamAim(f32),
}

impl Change {
    /// The setting a command changes, clamped to its valid range. `None` for
    /// commands that don't touch settings, invalid time zones and non-finite aims.
    pub fn from_command(command: &Command) -> Option<Self> {
        Some(match command {
            Command::SetBrightness(value) => Change::Brightness((*value).clamp(5, 100)),
            Command::SetDimAfter(0) => Change::DimAfter(0),
            Command::SetDimAfter(value) => Change::DimAfter((*value).clamp(15, 3_600)),
            Command::SetTimeZone(zone) if waved_config::valid_time_zone(zone) => {
                Change::TimeZone(zone.clone())
            }
            Command::SetClock24h(value) => Change::Clock24h(*value),
            Command::SetPreRecording(value) => Change::PreRecording(*value),
            Command::SetUploadMode(mode) => Change::UploadMode(*mode),
            Command::SetAlertTones(value) => Change::Tones(*value),
            Command::MarkFirstRunDone => Change::FirstRunDone,
            Command::SetAudioMode(mode) => Change::AudioMode(mode.clone()),
            Command::SetNrLevel(level) => Change::NrLevel(level.clone()),
            Command::SetSpeakerVolume(value) => Change::SpeakerVolume((*value).min(100)),
            Command::SetMicVolume(value) => Change::MicVolume((*value).min(800)),
            Command::SetBeamAim(degrees) if degrees.is_finite() => {
                let wrapped = degrees.rem_euclid(360.0);
                Change::BeamAim(if wrapped >= 360.0 { 0.0 } else { wrapped })
            }
            _ => return None,
        })
    }
}

pub fn apply_to_config(config: &mut WavedConfig, change: &Change) {
    match change {
        Change::Brightness(value) => config.display.brightness_pct = *value,
        Change::DimAfter(value) => {
            config.display.dim_after_s = *value;
            let off = config.display.off_after_s;
            if *value != 0 && off != 0 && off < *value {
                config.display.off_after_s = (*value).max(60);
            }
        }
        Change::TimeZone(zone) => config.time.zone.clone_from(zone),
        Change::Clock24h(value) => config.time.clock_24h = *value,
        Change::PreRecording(value) => config.recording.pre_recording = *value,
        Change::UploadMode(mode) => config.api.upload_mode = *mode,
        Change::Tones(value) => config.tones.enabled = *value,
        Change::FirstRunDone => config.device.first_run_done = true,
        Change::AudioMode(mode) => config.audio.mode = mode.clone(),
        Change::NrLevel(level) => config.audio.nr_level = level.clone(),
        Change::SpeakerVolume(value) => config.audio.speaker_volume = *value,
        Change::MicVolume(value) => config.audio.mic_volume = *value,
        Change::BeamAim(degrees) => config.audio.beam_aim_deg = *degrees,
    }
}

/// Audio changes are owned by AudioService (AudioState) and leave settings alone.
pub fn apply_to_settings(settings: &mut SettingsState, change: &Change) {
    match change {
        Change::Brightness(value) => settings.brightness_pct = *value,
        Change::DimAfter(value) => {
            settings.dim_after_s = *value;
            if *value != 0 && settings.off_after_s != 0 && settings.off_after_s < *value {
                settings.off_after_s = (*value).max(60);
            }
        }
        Change::TimeZone(zone) => settings.time_zone.clone_from(zone),
        Change::Clock24h(value) => settings.clock_24h = *value,
        Change::PreRecording(value) => settings.pre_recording = *value,
        Change::UploadMode(mode) => settings.upload_mode = *mode,
        Change::Tones(value) => settings.tones_enabled = *value,
        Change::FirstRunDone => settings.first_run_done = true,
        Change::AudioMode(_)
        | Change::NrLevel(_)
        | Change::SpeakerVolume(_)
        | Change::MicVolume(_)
        | Change::BeamAim(_) => {}
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `set -o pipefail; cargo test -p waved-settings --lib 2>&1 | grep "test result"`
Expected: `ok. 6 passed`

- [ ] **Step 6: Lint and commit**

```bash
cargo clippy -p waved-settings --all-targets -- -D warnings
git add crates/waved-settings
git commit -m "feat(settings): seed live state from config and map commands to settings

- seed_state copies audio mode, noise level, volumes, aim and settings at boot
- Change clamps out-of-range commands and ignores invalid zones and aims
- dimming later than screen-off pushes screen-off later so config stays valid

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `SettingsService` — debounced, delta-merged persistence

**Files:**
- Create: `crates/waved-settings/src/service.rs`
- Modify: `crates/waved-settings/src/lib.rs` (add `mod service; pub use service::{SettingsService, DEFAULT_DEBOUNCE};`)
- Test: `crates/waved-settings/src/service.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Change`, `apply_to_config`, `apply_to_settings` (Task 7) and `WavedConfig::{load_or_default, save}`.
- Produces:
  - `pub const DEFAULT_DEBOUNCE: Duration` (1 s)
  - `SettingsService::new(path: impl Into<PathBuf>, settings_tx: Arc<watch::Sender<SettingsState>>) -> Self`
  - `.with_debounce(Duration) -> Self`
  - `pub async fn run(self, commands: broadcast::Receiver<Command>)`
- Behaviour:
  - `bus.settings` updates immediately on each command.
  - Disk writes happen once per quiet period, counted from the first unsaved change.
  - Each write re-reads the file and applies only this service's changes.
  - The service flushes and returns on `Shutdown`, `RestartForConfig` or a closed channel.

- [ ] **Step 1: Write the failing tests** in `service.rs`:

```rust
//! Persists settings changes: updates the watch at once, writes config.toml
//! once per quiet period, and merges into the file as it is on disk so a
//! config saved by the setup page in the meantime keeps its fields.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use waved_state::StateBus;

    fn start(path: &Path) -> (StateBus, tokio::task::JoinHandle<()>) {
        let bus = StateBus::new();
        let service = SettingsService::new(path, Arc::clone(&bus.settings))
            .with_debounce(Duration::from_millis(500));
        let handle = tokio::spawn(service.run(bus.commands.subscribe()));
        (bus, handle)
    }

    fn saved(path: &Path) -> WavedConfig {
        WavedConfig::load(path).unwrap()
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn bursts_are_saved_once_with_the_final_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::SetBrightness(40)).unwrap();
        for volume in [50u16, 60, 70, 80, 90] {
            bus.commands.send(Command::SetSpeakerVolume(volume)).unwrap();
        }
        settle().await;
        assert_eq!(bus.settings.borrow().brightness_pct, 40, "watch updates at once");
        assert_eq!(saved(&path).display.brightness_pct, 80, "nothing written yet");

        tokio::time::sleep(Duration::from_millis(600)).await;
        let config = saved(&path);
        assert_eq!(config.display.brightness_pct, 40);
        assert_eq!(config.audio.speaker_volume, 90);

        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_config_saved_by_setup_keeps_its_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::SetSpeakerVolume(30)).unwrap();
        settle().await;
        let mut from_setup = saved(&path);
        from_setup.api.endpoint = "https://notes.example.com/hook".into();
        from_setup.save(&path).unwrap();

        tokio::time::sleep(Duration::from_millis(600)).await;
        let config = saved(&path);
        assert_eq!(config.api.endpoint, "https://notes.example.com/hook");
        assert_eq!(config.audio.speaker_volume, 30);

        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_flushes_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::SetPreRecording(false)).unwrap();
        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
        assert!(!saved(&path).recording.pre_recording);
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_values_never_reach_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::SetTimeZone("../etc".into())).unwrap();
        bus.commands.send(Command::SetBrightness(0)).unwrap();
        bus.commands.send(Command::SetBeamAim(f32::INFINITY)).unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;

        let config = saved(&path);
        assert_eq!(config.time.zone, "UTC");
        assert_eq!(config.display.brightness_pct, 5);
        assert_eq!(config.audio.beam_aim_deg, 0.0);
        bus.commands.send(Command::Shutdown).unwrap();
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn unrelated_commands_do_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        WavedConfig::default().save(&path).unwrap();
        let (bus, handle) = start(&path);

        bus.commands.send(Command::ToggleMicMute).unwrap();
        bus.commands.send(Command::StartRecording).unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!dir.path().join("config.toml.bak").exists(), "save() was never called");

        bus.commands.send(Command::RestartForConfig).unwrap();
        handle.await.unwrap();
    }
}
```

Register the module in `lib.rs` (next to `mod change;`):

```rust
mod service;

pub use service::{SettingsService, DEFAULT_DEBOUNCE};
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-settings --lib 2>&1 | tail -5`
Expected: compile error `cannot find type \`SettingsService\``

- [ ] **Step 3: Implement** in `service.rs`, above `#[cfg(test)]`:

```rust
use crate::change::{apply_to_config, apply_to_settings, Change};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_config::WavedConfig;
use waved_state::{Command, SettingsState};

/// Quiet period before writing. Holding Louder sends about three commands a second.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(1);

pub struct SettingsService {
    path: PathBuf,
    settings_tx: Arc<watch::Sender<SettingsState>>,
    debounce: Duration,
    pending: Vec<Change>,
}

impl SettingsService {
    pub fn new(path: impl Into<PathBuf>, settings_tx: Arc<watch::Sender<SettingsState>>) -> Self {
        Self {
            path: path.into(),
            settings_tx,
            debounce: DEFAULT_DEBOUNCE,
            pending: Vec::new(),
        }
    }

    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Runs until Shutdown, RestartForConfig or a closed channel, flushing first.
    pub async fn run(mut self, mut commands: broadcast::Receiver<Command>) {
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let wake_at = deadline;
            let quiet_period_over = async move {
                match wake_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                received = commands.recv() => match received {
                    Ok(Command::Shutdown) | Ok(Command::RestartForConfig) => {
                        self.flush();
                        return;
                    }
                    Ok(command) => {
                        if let Some(change) = Change::from_command(&command) {
                            self.settings_tx
                                .send_modify(|settings| apply_to_settings(settings, &change));
                            self.pending.push(change);
                            deadline.get_or_insert_with(|| tokio::time::Instant::now() + self.debounce);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("settings service lagged {skipped} commands");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        self.flush();
                        return;
                    }
                },
                _ = quiet_period_over => {
                    self.flush();
                    deadline = None;
                }
            }
        }
    }

    /// Re-read the file and apply only our changes, so fields written by the
    /// setup page in the meantime survive.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut config = WavedConfig::load_or_default(&self.path);
        for change in self.pending.drain(..) {
            apply_to_config(&mut config, &change);
        }
        match config.save(&self.path) {
            Ok(()) => info!("settings saved to {}", self.path.display()),
            Err(error) => warn!("settings not saved: {error}"),
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `set -o pipefail; cargo test -p waved-settings --lib 2>&1 | grep "test result"`
Expected: `ok. 11 passed`

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-settings --all-targets -- -D warnings
git add crates/waved-settings/src
git commit -m "feat(settings): persist device-side changes with a debounced merge

- watch updates at once; disk writes once per quiet period
- each write re-reads config.toml so setup-page fields saved meanwhile survive
- flushes on shutdown and config restart; invalid values never reach disk

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Wire settings into the `waved` binary

**Files:**
- Modify: `crates/waved/Cargo.toml` (dependencies)
- Modify: `crates/waved/src/main.rs`
  - lines 53-59: the boot seeding
  - before `    // Low battery handler` (about line 622): spawn the service
- Test: `scripts/linux-test.sh test -p waved` (existing integration tests) and the Linux check

**Interfaces:**
- Consumes: `waved_settings::{seed_state, SettingsService}`.

- [ ] **Step 1: Add the dependency** to `crates/waved/Cargo.toml` `[dependencies]`:

```toml
waved-settings = { path = "../waved-settings" }
```

- [ ] **Step 2: Seed live state from config at boot.** In `main.rs`, replace:

```rust
    // Seed mutable audio state from TOML so apply_mic_volume sees the
    // user-configured boot value before the first frame is processed.
    // This must run before any service thread is spawned — the Tokio
    // runtime is constructed below and all service spawns happen there.
    bus.audio.send_modify(|s| {
        s.mic_volume = config.audio.mic_volume;
    });
```

with:

```rust
    // Seed live state from TOML: audio mode, noise reduction, volumes, aim and
    // the display/time/recording settings. Must run before any service is
    // spawned, because AudioService reads mode and NR from the watch when built.
    waved_settings::seed_state(&bus, &config);
```

- [ ] **Step 3: Spawn the service.** Directly above `    // Low battery handler`:

```rust
    // Persist device-side setting changes (volume, mode, display, time,
    // recording policy) back to config.toml.
    tokio::spawn(
        waved_settings::SettingsService::new(CONFIG_PATH, Arc::clone(&bus.settings))
            .run(bus.commands.subscribe()),
    );
```

- [ ] **Step 4: Build and run the binary's tests in Linux**

Run: `set -o pipefail; scripts/linux-test.sh check -p waved --features ui,capture-alsa 2>&1 | tail -1`
Expected: `Finished`

Run: `set -o pipefail; scripts/linux-test.sh check -p waved --features simulate,ui 2>&1 | tail -1`
Expected: `Finished`

Run: `set -o pipefail; scripts/linux-test.sh test -p waved 2>&1 | grep "test result"`
Expected: every line `ok`

- [ ] **Step 5: Commit**

```bash
git add crates/waved/Cargo.toml crates/waved/src/main.rs
git commit -m "feat(waved): restore saved settings at boot and persist changes

- seed_state replaces the mic-volume-only seed (mode, NR, volumes now restore)
- SettingsService writes device-side changes back to config.toml

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Fonts and icons

The kit gets its own fonts: Barlow with tabular figures frozen in, plus a Tabler outline icon subset. They are generated once by a pinned script, and the generated C sources are committed. Cargo never needs node or Python.

**Files:**
- Create: `crates/waved-ui/fonts/build_fonts.sh`, `crates/waved-ui/fonts/icons.py`, `crates/waved-ui/fonts/README.md`
- Generate: `crates/waved-ui/fonts/LICENSE-Barlow.txt`, `crates/waved-ui/fonts/LICENSE-Tabler.txt`, `crates/waved-ui/src/kit/fonts/*.c` (13 files), `crates/waved-ui/src/kit/icons.rs`
- Create: `crates/waved-ui/src/kit/mod.rs`, `crates/waved-ui/src/kit/fonts.rs`
- Modify: `crates/waved-ui/build.rs` (compile the kit fonts)
- Modify: `crates/waved-ui/src/lib.rs` (add `pub mod kit;` after `pub mod input;`)
- Modify: `../docs/mockups/README.md` ("Icons" section)
- Test: `crates/waved-ui/src/kit/fonts.rs` (`mod tests`)

**Interfaces:**
- Produces:
  - `kit::fonts::Font { Micro, Small, Body, Title, Dial, Num13, Num20, Num26, Num48, Icon16, Icon20, Icon26, Icon44 }` with `ptr(self) -> *const lv_font_t` and `line_height(self) -> i32`.
  - `kit::icons::{MIC, MIC_OFF, MARK, VOLUME_LOW, VOLUME_HIGH, VOLUME_OFF, LOCK, WIFI, WIFI_OFF, BLUETOOTH, USB, BATTERY_0..BATTERY_4, BATTERY_CHARGING, PLAY, PAUSE, STOP, RECORD, LEFT, RIGHT, DOWN, CHECK_CIRCLE, CHECK, CLOCK, ALERT, INFO, DEVICE, SEND, TRASH, POWER, PHONE, PHONE_OFF, FOLDER, REFRESH, LAPTOP, MOBILE, SUN, BACKSPACE, MINUS, PLUS, NOISE, PATTERN, SETTINGS, HEIGHT, DOT, ALL}`, each a `&'static str`.

- [ ] **Step 1: Write the icon mapping** `crates/waved-ui/fonts/icons.py`:

```python
#!/usr/bin/env python3
"""Map Tabler icon names to code points.

    icons.py range <tabler-icons.css>   -> lv_font_conv range list
    icons.py rust  <tabler-icons.css>   -> src/kit/icons.rs
"""
import re
import sys

ICONS = [  # (Rust constant, Tabler name)
    ("MIC", "microphone"), ("MIC_OFF", "microphone-off"), ("MARK", "bookmark"),
    ("VOLUME_LOW", "volume-2"), ("VOLUME_HIGH", "volume"), ("VOLUME_OFF", "volume-off"),
    ("LOCK", "lock"), ("WIFI", "wifi"), ("WIFI_OFF", "wifi-off"),
    ("BLUETOOTH", "bluetooth"), ("USB", "usb"),
    ("BATTERY_0", "battery"), ("BATTERY_1", "battery-1"), ("BATTERY_2", "battery-2"),
    ("BATTERY_3", "battery-3"), ("BATTERY_4", "battery-4"), ("BATTERY_CHARGING", "battery-charging"),
    ("PLAY", "player-play"), ("PAUSE", "player-pause"), ("STOP", "player-stop"), ("RECORD", "player-record"),
    ("LEFT", "chevron-left"), ("RIGHT", "chevron-right"), ("DOWN", "chevron-down"),
    ("CHECK_CIRCLE", "circle-check"), ("CHECK", "check"), ("CLOCK", "clock"),
    ("ALERT", "alert-triangle"), ("INFO", "info-circle"),
    ("DEVICE", "database"), ("SEND", "upload"), ("TRASH", "trash"), ("POWER", "power"),
    ("PHONE", "phone"), ("PHONE_OFF", "phone-off"), ("FOLDER", "folder"), ("REFRESH", "refresh"),
    ("LAPTOP", "device-laptop"), ("MOBILE", "device-mobile"), ("SUN", "sun"),
    ("BACKSPACE", "backspace"), ("MINUS", "minus"), ("PLUS", "plus"),
    ("NOISE", "wave-sine"), ("PATTERN", "ear"), ("SETTINGS", "settings"),
    ("HEIGHT", "arrows-vertical"), ("DOT", "circle-dot"),
]


def codepoints(css):
    table = dict(re.findall(r'\.ti-([a-z0-9-]+):before\s*\{\s*content:\s*"\\([0-9a-f]+)";', css))
    found = []
    for const, name in ICONS:
        if name not in table:
            sys.exit(f"tabler icon not found: {name}")
        found.append((const, name, int(table[name], 16)))
    return found


def main():
    mode, css_path = sys.argv[1], sys.argv[2]
    with open(css_path, encoding="utf-8") as handle:
        icons = codepoints(handle.read())
    if mode == "range":
        print(",".join(f"0x{cp:X}" for _, _, cp in icons))
    elif mode == "rust":
        print("//! Tabler icon code points. Generated by fonts/icons.py; do not edit.")
        print("//! Render them with one of the `Font::Icon*` fonts.")
        print()
        for const, name, cp in icons:
            print(f"/// Tabler `{name}`")
            print(f'pub const {const}: &str = "\\u{{{cp:x}}}";')
        print()
        print("/// Every icon, for glyph-coverage tests.")
        print("pub const ALL: &[&str] = &[" + ", ".join(c for c, _, _ in icons) + "];")
    else:
        sys.exit("usage: icons.py range|rust <tabler-icons.css>")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Write the generator** `crates/waved-ui/fonts/build_fonts.sh` (`chmod +x`):

```bash
#!/usr/bin/env bash
# Regenerate the LVGL font sources in src/kit/fonts/ and src/kit/icons.rs from
# pinned upstream fonts. Needs curl, node (npx) and python3. Outputs are
# committed; cargo never runs this script.
set -euo pipefail
cd "$(dirname "$0")"

GOOGLE_FONTS=9710da1eacb3be272583c3224dcb70f9da6eadbb
TABLER=3.48.0
CONV="npx -y lv_font_conv@1.5.3"
OUT=../src/kit/fonts
TEXT_RANGE="0x20-0x7E,0xA0-0xFF,0x2013,0x2014,0x2018,0x2019,0x201C,0x201D,0x2022,0x2026,0x2212"
DIGIT_RANGE="0x20,0x25,0x2B,0x2D,0x2E,0x30-0x3A,0x2212"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

raw() { curl -fsSL -o "$2" "https://raw.githubusercontent.com/google/fonts/$GOOGLE_FONTS/ofl/$1"; }
raw barlowsemicondensed/BarlowSemiCondensed-Medium.ttf   "$WORK/bsc-medium.ttf"
raw barlowsemicondensed/BarlowSemiCondensed-SemiBold.ttf "$WORK/bsc-semibold.ttf"
raw barlow/Barlow-Medium.ttf                             "$WORK/b-medium.ttf"
raw barlow/OFL.txt                                       LICENSE-Barlow.txt
(cd "$WORK" && npm pack --silent "@tabler/icons-webfont@$TABLER" >/dev/null \
  && tar xzf "tabler-icons-webfont-$TABLER.tgz")
cp "$WORK/package/LICENSE" LICENSE-Tabler.txt

# Freeze tabular figures so digits keep their width while a timer runs.
python3 -m venv "$WORK/venv"
"$WORK/venv/bin/pip" -q install fonttools==4.66.1 opentype-feature-freezer==1.32.2
for face in bsc-medium bsc-semibold b-medium; do
  "$WORK/venv/bin/pyftfeatfreeze" -f tnum "$WORK/$face.ttf" "$WORK/$face-tnum.ttf" >/dev/null
done

mkdir -p "$OUT"
font() { # name file size range
  $CONV --font "$WORK/$2" --size "$3" --bpp 4 --format lvgl --no-compress \
    -r "$4" --lv-font-name "$1" -o "$OUT/$1.c" >/dev/null
}
font font_micro_12 bsc-medium-tnum.ttf   12 "$TEXT_RANGE"
font font_small_13 bsc-medium-tnum.ttf   13 "$TEXT_RANGE"
font font_body_15  bsc-medium-tnum.ttf   15 "$TEXT_RANGE"
font font_title_20 bsc-semibold-tnum.ttf 20 "$TEXT_RANGE"
font font_dial_26  bsc-semibold-tnum.ttf 26 "$TEXT_RANGE"
font font_num_13   b-medium-tnum.ttf     13 "$TEXT_RANGE"
font font_num_20   b-medium-tnum.ttf     20 "$TEXT_RANGE"
font font_num_26   b-medium-tnum.ttf     26 "$DIGIT_RANGE"
font font_num_48   b-medium-tnum.ttf     48 "$DIGIT_RANGE"

CSS="$WORK/package/dist/tabler-icons.css"
ICON_RANGE=$(python3 icons.py range "$CSS")
for size in 16 20 26 44; do
  $CONV --font "$WORK/package/dist/fonts/tabler-icons.ttf" --size "$size" --bpp 4 \
    --format lvgl --no-compress -r "$ICON_RANGE" --lv-font-name "font_icons_$size" \
    -o "$OUT/font_icons_$size.c" >/dev/null
done
python3 icons.py rust "$CSS" > ../src/kit/icons.rs
echo "fonts regenerated in $OUT"
```

- [ ] **Step 3: Run the generator**

Run: `crates/waved-ui/fonts/build_fonts.sh && ls crates/waved-ui/src/kit/fonts | wc -l`
Expected: `fonts regenerated in ../src/kit/fonts`, then `13`

Run: `grep -c "pub const" crates/waved-ui/src/kit/icons.rs`
Expected: `50` (49 icons plus `ALL`)

- [ ] **Step 4: Write `crates/waved-ui/fonts/README.md`**

```markdown
# Kit fonts

Generated LVGL fonts for `src/kit`. Run `./build_fonts.sh` to regenerate. It downloads
pinned sources, so the output is reproducible. The generated `.c` files are committed.

| Font | Source | Sizes |
|---|---|---|
| `font_micro_12`, `font_small_13`, `font_body_15` | Barlow Semi Condensed Medium | 12, 13, 15 |
| `font_title_20`, `font_dial_26` | Barlow Semi Condensed SemiBold | 20, 26 |
| `font_num_13`, `font_num_20` | Barlow Medium | 13, 20 (full text range) |
| `font_num_26`, `font_num_48` | Barlow Medium | 26, 48 (digits, `% + - . :` and minus only) |
| `font_icons_16/20/26/44` | Tabler Icons webfont 3.48.0 | the 49 icons in `icons.py` |

- **Tabular figures:** every Barlow face has its `tnum` feature frozen with
  `pyftfeatfreeze`, so digits have equal widths and timers don't jitter.
- **Text range:** covers ASCII, Latin-1, curly quotes, en/em dashes, bullet, `…` and `−`.
- **Licences:** Barlow is OFL (`LICENSE-Barlow.txt`); Tabler Icons is MIT (`LICENSE-Tabler.txt`).
- **Adding an icon:** add it to `ICONS` in `icons.py` and rerun the script.
```

- [ ] **Step 5: Compile the fonts with LVGL.** In `crates/waved-ui/build.rs`, after `let c_files = collect_c_files(&lvgl_src);`:

```rust
    // Kit fonts generated by fonts/build_fonts.sh (committed C sources).
    let mut kit_fonts = collect_c_files(&manifest.join("src/kit/fonts"));
    kit_fonts.sort();
```

Change `.files(&c_files)` to `.files(&c_files).files(&kit_fonts)`, and next to the other `rerun-if-changed` lines add:

```rust
    for f in &kit_fonts {
        println!("cargo:rerun-if-changed={}", f.display());
    }
```

- [ ] **Step 6: Write the failing tests.** Create `crates/waved-ui/src/kit/mod.rs`:

```rust
//! UI kit for the revision-2 design (`docs/mockups`). New screens build on
//! these pieces instead of raw LVGL calls; see `src/kit/README.md`.

pub mod fonts;
pub mod icons;
```

In `crates/waved-ui/src/lib.rs`, add `pub mod kit;` after `pub mod input;`.

Create `crates/waved-ui/src/kit/fonts.rs` containing only the tests for now:

```rust
//! Fonts generated by `fonts/build_fonts.sh` (Barlow, OFL; Tabler icons, MIT).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{lv_font_get_glyph_dsc, lv_font_glyph_dsc_t};

    fn glyph(font: Font, ch: char) -> Option<lv_font_glyph_dsc_t> {
        // SAFETY: zeroed is a valid empty descriptor; the font pointer is static.
        unsafe {
            let mut dsc: lv_font_glyph_dsc_t = std::mem::zeroed();
            lv_font_get_glyph_dsc(font.ptr(), &mut dsc, ch as u32, 0).then_some(dsc)
        }
    }

    #[test]
    fn text_fonts_cover_the_glyphs_the_copy_uses() {
        for font in [Font::Micro, Font::Small, Font::Body, Font::Title, Font::Dial, Font::Num13, Font::Num20] {
            for ch in ['A', 'z', '0', '…', '·', '’', '“', '−', 'é', '°', '%'] {
                assert!(glyph(font, ch).is_some(), "{font:?} lacks {ch:?}");
            }
        }
    }

    #[test]
    fn digits_are_tabular() {
        for font in [Font::Num48, Font::Num26, Font::Body] {
            let widths: Vec<u16> = ('0'..='9').map(|ch| glyph(font, ch).unwrap().adv_w).collect();
            assert!(widths.windows(2).all(|pair| pair[0] == pair[1]), "{font:?}: {widths:?}");
        }
    }

    #[test]
    fn icon_fonts_cover_every_icon() {
        for font in [Font::Icon16, Font::Icon20, Font::Icon26, Font::Icon44] {
            for icon in crate::kit::icons::ALL {
                let ch = icon.chars().next().unwrap();
                assert!(glyph(font, ch).is_some(), "{font:?} lacks U+{:X}", ch as u32);
            }
        }
    }

    #[test]
    fn line_heights_follow_the_type_scale() {
        assert!(Font::Num48.line_height() >= 48);
        assert!(Font::Body.line_height() < Font::Title.line_height());
    }
}
```

- [ ] **Step 7: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::fonts 2>&1 | tail -5`
Expected: compile error `cannot find type \`Font\``

- [ ] **Step 8: Implement `Font`** above the tests in `fonts.rs`:

```rust
use crate::bindings::{lv_font_get_line_height, lv_font_t};

extern "C" {
    static font_micro_12: lv_font_t;
    static font_small_13: lv_font_t;
    static font_body_15: lv_font_t;
    static font_title_20: lv_font_t;
    static font_dial_26: lv_font_t;
    static font_num_13: lv_font_t;
    static font_num_20: lv_font_t;
    static font_num_26: lv_font_t;
    static font_num_48: lv_font_t;
    static font_icons_16: lv_font_t;
    static font_icons_20: lv_font_t;
    static font_icons_26: lv_font_t;
    static font_icons_44: lv_font_t;
}

/// Type roles from `docs/mockups/README.md` ("Type"), plus icon sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// 12 px: side-button labels, hints.
    Micro,
    /// 13 px: descriptions, status, messages.
    Small,
    /// 15 px: buttons, primary lines.
    Body,
    /// 20 px semibold: screen titles, focused list row, mode names.
    Title,
    /// 26 px semibold: dial state, dialog titles.
    Dial,
    /// 13 px Barlow: status clock, small numbers.
    Num13,
    /// 20 px Barlow: phone numbers, PIN keys.
    Num20,
    /// 26 px Barlow digits: counts, minus/plus.
    Num26,
    /// 48 px Barlow digits: timer, clock, percentages.
    Num48,
    Icon16,
    Icon20,
    Icon26,
    Icon44,
}

impl Font {
    pub fn ptr(self) -> *const lv_font_t {
        // SAFETY: immutable font descriptors compiled into the binary.
        unsafe {
            match self {
                Font::Micro => &font_micro_12,
                Font::Small => &font_small_13,
                Font::Body => &font_body_15,
                Font::Title => &font_title_20,
                Font::Dial => &font_dial_26,
                Font::Num13 => &font_num_13,
                Font::Num20 => &font_num_20,
                Font::Num26 => &font_num_26,
                Font::Num48 => &font_num_48,
                Font::Icon16 => &font_icons_16,
                Font::Icon20 => &font_icons_20,
                Font::Icon26 => &font_icons_26,
                Font::Icon44 => &font_icons_44,
            }
        }
    }

    pub fn line_height(self) -> i32 {
        // SAFETY: valid static font.
        unsafe { lv_font_get_line_height(self.ptr()) }
    }
}
```

- [ ] **Step 9: Run the tests**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::fonts 2>&1 | grep "test result"`
Expected: `ok. 4 passed`

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`
Expected: `ok. 73 passed` (69 legacy plus 4)

- [ ] **Step 10: Correct the mockup README's icon section.** In `../docs/mockups/README.md`, replace the whole `### Icons` section with:

```markdown
### Icons

All icons come from one subset of the Tabler Icons webfont (MIT, outline style with 2 px
strokes), so they match the line icons in `index.html`. `crates/waved-ui/fonts/icons.py` lists
the 49 icons. They are rendered at 16, 20, 26 and 44 px. No PNGs need to be deployed, which
is the reason the Save/Send/Discard buttons render blank today.
```

- [ ] **Step 11: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/fonts crates/waved-ui/src/kit crates/waved-ui/build.rs crates/waved-ui/src/lib.rs ../docs/mockups/README.md
git commit -m "feat(ui): kit fonts and tabler icons from a pinned generator

- Barlow / Barlow Semi Condensed with tabular figures frozen in
- 49 Tabler outline icons at 16/20/26/44 px with generated Rust constants
- glyph coverage tests for the copy and every icon

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Design tokens and geometry

**Files:**
- Create: `crates/waved-ui/src/kit/tokens.rs`
- Modify: `crates/waved-ui/src/kit/mod.rs` (add `pub mod tokens;`)
- Test: `crates/waved-ui/src/kit/tokens.rs` (`mod tests`)

**Interfaces:**
- Produces:
  - **Colours:** `BG, SURFACE, RAISED, LINE, TEXT, TEXT_2, TEXT_3, LIVE, ON_LIVE, REC, WARN, WHITE: u32`.
  - **Panel geometry:** `SCREEN_W, SCREEN_H, PANEL_CX, PANEL_CY, PANEL_R, TAP_LIMIT_R, MIN_TARGET, MIN_PRIMARY: i32`.
  - **`Rect`:** `Rect { x, y, w, h }` with `new`, `max_dist2` and `within_tap_limit`.
  - **Layout rects:** `STATUS, STATUS_PILL, BACK, PRIMARY, SIDE_LEFT, SIDE_RIGHT: Rect`, `ROW2: [Rect; 2]` and `ROW3: [Rect; 3]`.
  - **Layout values:** `TITLE_Y, SIDE_LABEL_Y, DIAL_CX, DIAL_CY, DIAL_R, MESSAGE_MAX_W: i32`.
  - **Durations:** `MESSAGE_MS_SHORT, MESSAGE_MS_LONG, GAUGE_MS, HOLD_TO_STOP_MS: u64`.
  - **Angle helpers:** `lvgl_angle(device_deg: f32) -> i32` and `polar(cx: i32, cy: i32, r: f32, device_deg: f32) -> (i32, i32)`.

- [ ] **Step 1: Write the failing tests** (create `tokens.rs` with only this module and add `pub mod tokens;` to `kit/mod.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_layout_rect_respects_the_tap_limit() {
        let rects = [
            STATUS_PILL, BACK, PRIMARY, SIDE_LEFT, SIDE_RIGHT, ROW2[0], ROW2[1], ROW3[0], ROW3[1], ROW3[2],
        ];
        for rect in rects {
            assert!(rect.within_tap_limit(), "{rect:?}");
        }
    }

    #[test]
    fn targets_meet_the_minimum_size() {
        for rect in [BACK, SIDE_LEFT, SIDE_RIGHT, ROW2[0], ROW3[0]] {
            assert!(rect.w >= MIN_TARGET && rect.h >= MIN_TARGET, "{rect:?}");
        }
        assert!(PRIMARY.h >= MIN_PRIMARY);
    }

    #[test]
    fn the_old_back_button_fails_the_tap_limit() {
        assert!(!Rect::new(8, 8, 56, 64).within_tap_limit());
    }

    #[test]
    fn device_angles_map_to_lvgl_and_points() {
        assert_eq!(lvgl_angle(0.0), 270);
        assert_eq!(lvgl_angle(90.0), 0);
        assert_eq!(lvgl_angle(-90.0), 180);
        assert_eq!(lvgl_angle(450.0), 0);
        assert_eq!(polar(200, 132, 78.0, 0.0), (200, 54));
        assert_eq!(polar(200, 132, 78.0, 90.0), (278, 132));
        assert_eq!(polar(200, 132, 78.0, 180.0), (200, 210));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::tokens 2>&1 | tail -3`
Expected: compile errors (`STATUS_PILL` etc. not found)

- [ ] **Step 3: Implement** above the tests:

```rust
//! Design tokens from `docs/mockups/README.md` (revision 2). Colours are 0xRRGGBB.

pub const BG: u32 = 0x000000;
pub const SURFACE: u32 = 0x141c1e;
pub const RAISED: u32 = 0x1f292b;
pub const LINE: u32 = 0x2a3639;
pub const TEXT: u32 = 0xf2f5f4;
pub const TEXT_2: u32 = 0x9aa7a6;
pub const TEXT_3: u32 = 0x5d6a6a;
pub const LIVE: u32 = 0x3fd6b1;
pub const ON_LIVE: u32 = 0x00140f;
pub const REC: u32 = 0xff4b3e;
pub const WARN: u32 = 0xffb547;
pub const WHITE: u32 = 0xffffff;

pub const SCREEN_W: i32 = 400;
pub const SCREEN_H: i32 = 304;
pub const PANEL_CX: i32 = 200;
pub const PANEL_CY: i32 = 152;
pub const PANEL_R: i32 = 190;
/// No tappable pixel may lie farther than this from the panel centre.
pub const TAP_LIMIT_R: i32 = 182;
pub const MIN_TARGET: i32 = 44;
pub const MIN_PRIMARY: i32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Squared distance from the panel centre to the farthest corner.
    pub fn max_dist2(&self) -> i32 {
        let xs = [self.x, self.x + self.w - 1];
        let ys = [self.y, self.y + self.h - 1];
        xs.iter()
            .flat_map(|x| ys.iter().map(move |y| (x - PANEL_CX).pow(2) + (y - PANEL_CY).pow(2)))
            .max()
            .unwrap_or(0)
    }

    pub fn within_tap_limit(&self) -> bool {
        self.max_dist2() <= TAP_LIMIT_R * TAP_LIMIT_R
    }
}

// Placements from the component table in docs/mockups/README.md.
pub const STATUS: Rect = Rect::new(100, 8, 200, 20);
pub const STATUS_PILL: Rect = Rect::new(100, 7, 200, 28);
pub const TITLE_Y: i32 = 38;
pub const BACK: Rect = Rect::new(144, 252, 112, 44);
pub const PRIMARY: Rect = Rect::new(108, 216, 184, 56);
pub const SIDE_LEFT: Rect = Rect::new(28, 102, 60, 60);
pub const SIDE_RIGHT: Rect = Rect::new(312, 102, 60, 60);
pub const SIDE_LABEL_Y: i32 = 168;
pub const ROW2: [Rect; 2] = [Rect::new(86, 222, 108, 48), Rect::new(206, 222, 108, 48)];
pub const ROW3: [Rect; 3] = [
    Rect::new(72, 222, 80, 48),
    Rect::new(160, 222, 80, 48),
    Rect::new(248, 222, 80, 48),
];
pub const DIAL_CX: i32 = 200;
pub const DIAL_CY: i32 = 132;
pub const DIAL_R: i32 = 78;
pub const MESSAGE_MAX_W: i32 = 230;

pub const MESSAGE_MS_SHORT: u64 = 2_000;
pub const MESSAGE_MS_LONG: u64 = 4_000;
pub const GAUGE_MS: u64 = 1_500;
pub const HOLD_TO_STOP_MS: u64 = 1_000;

/// Device angles: 0° is the top of the screen, clockwise (the DoA convention).
/// LVGL arcs measure 0° at 3 o'clock, clockwise.
pub fn lvgl_angle(device_deg: f32) -> i32 {
    ((device_deg - 90.0).rem_euclid(360.0).round() as i32) % 360
}

/// The point `r` px from (cx, cy) at a device angle.
pub fn polar(cx: i32, cy: i32, r: f32, device_deg: f32) -> (i32, i32) {
    let t = device_deg.to_radians();
    (
        (cx as f32 + r * t.sin()).round() as i32,
        (cy as f32 - r * t.cos()).round() as i32,
    )
}
```

- [ ] **Step 4: Run the tests**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::tokens 2>&1 | grep "test result"`
Expected: `ok. 4 passed`

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit
git commit -m "feat(ui): design tokens, layout rects and angle helpers

- palette, type sizes and placements from the approved mockups
- tap-limit math proves every layout rect fits the round panel

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: `Obj`, event handlers and the headless test kit

**Files:**
- Create: `crates/waved-ui/src/kit/obj.rs`, `crates/waved-ui/src/kit/event.rs`, `crates/waved-ui/src/kit/testkit.rs`
- Modify: `crates/waved-ui/src/kit/mod.rs`, `crates/waved-ui/Cargo.toml` (add `[dev-dependencies] png = "0.17"`), `crates/waved-ui/lv_conf.h`
- Test: all three new files (`mod tests`)

**Interfaces:**
- Consumes: `Font` (Task 10), tokens (Task 11).
- Produces:
  - **`kit::obj::Obj`** (`Copy`, `Eq`, `!Send`) and `color(hex) -> lv_color_t`.
    - Constructors: `from_raw`, `raw`, `is_null`, `screen() -> Obj`, `plain(parent)`, `label(parent, text, Font, hex)`, `arc(parent)`, `line(parent)`.
    - Chainable setters (`self -> Self`): `pos`, `size`, `rect`, `width`, `bg`, `bg_opa`, `radius`, `round`, `border`, `font`, `text_color`, `opa`, `centered_text(width)`, `one_line()`, `pressed_bg(hex)`, `clickable`, `hidden`, `flag`, `ext_click`.
    - Other methods: `set_text`, `text`, `set_state(state, on)`, `has_state`, `has_flag`, `is_clickable`, `is_hidden`, `coords`, `click_area`, `child_count`, `child`, `update_layout`, `delete`.
  - **`kit::event`:** `on(obj, code, impl FnMut(&Event) + 'static)`, `on_click(obj, impl FnMut() + 'static)`, and `Event { code(), target() }`. Handlers are dropped when the object is deleted.
  - **`kit::testkit`** (test only):
    - `lvgl(|lv| …)`
    - `Lvgl::{screen, render, click, send, advance}`
    - `Frame::{pixel, to_png}`
    - `assert_golden(name, &Frame)`, `target_problems(root) -> Vec<String>`, `assert_targets_ok(root)`

- [ ] **Step 1: Dark default theme.** In `crates/waved-ui/lv_conf.h`, after the `LV_FONT_DEFAULT` line (line 68), add:

```c
/* Dark default theme: unstyled widgets render dark instead of white discs. */
#define LV_USE_THEME_DEFAULT 1
#define LV_THEME_DEFAULT_DARK 1
```

- [ ] **Step 2: Write the test kit and its failing self-tests.** Add `png = "0.17"` under a new `[dev-dependencies]` section in `crates/waved-ui/Cargo.toml`. Create `testkit.rs`:

```rust
//! Headless LVGL for unit tests. Renders 400×304 frames on the host and
//! compares them with committed golden PNGs in `crates/waved-ui/golden/`.
//!
//! LVGL is a process-wide singleton: every test goes through [`lvgl`], which
//! initializes it once and serializes access with a mutex.

use crate::bindings::*;
use crate::kit::obj::Obj;
use crate::kit::tokens::{
    Rect, MIN_TARGET, PANEL_CX, PANEL_CY, PANEL_R, SCREEN_H, SCREEN_W, TAP_LIMIT_R,
};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

struct Headless {
    disp: *mut lv_display_t,
    buf: *mut u16,
    screen: *mut lv_obj_t,
}

// SAFETY: only used while holding the mutex, by one test at a time.
unsafe impl Send for Headless {}

static LVGL: OnceLock<Mutex<Headless>> = OnceLock::new();

unsafe extern "C" fn flush_cb(disp: *mut lv_display_t, _area: *const lv_area_t, _px: *mut u8) {
    // FULL render mode: the draw buffer already holds the whole frame.
    lv_display_flush_ready(disp);
}

fn init() -> Mutex<Headless> {
    let pixels = (SCREEN_W * SCREEN_H) as usize;
    // u32 storage keeps the 4-byte alignment LVGL expects for draw buffers.
    let storage: &'static mut [u32] = Box::leak(vec![0u32; pixels / 2].into_boxed_slice());
    // SAFETY: LVGL is initialized exactly once, here, before any other call.
    unsafe {
        lv_init();
        let disp = lv_display_create(SCREEN_W, SCREEN_H);
        lv_display_set_color_format(disp, lv_color_format_t_LV_COLOR_FORMAT_RGB565);
        lv_display_set_buffers(
            disp,
            storage.as_mut_ptr().cast(),
            std::ptr::null_mut(),
            (pixels * 2) as u32,
            lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_FULL,
        );
        lv_display_set_flush_cb(disp, Some(flush_cb));
        Mutex::new(Headless {
            disp,
            buf: storage.as_mut_ptr().cast(),
            screen: std::ptr::null_mut(),
        })
    }
}

/// Exclusive access to the headless LVGL instance for one test.
pub struct Lvgl {
    inner: MutexGuard<'static, Headless>,
}

/// Run `test` with exclusive access to LVGL.
pub fn lvgl<R>(test: impl FnOnce(&mut Lvgl) -> R) -> R {
    let inner = LVGL
        .get_or_init(init)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test(&mut Lvgl { inner })
}

impl Lvgl {
    /// A fresh black screen, loaded and active. Deletes the previous test screen.
    pub fn screen(&mut self) -> Obj {
        let root = Obj::screen();
        // SAFETY: both screens are valid; the old one is no longer active.
        unsafe {
            lv_screen_load(root.raw());
            if !self.inner.screen.is_null() {
                lv_obj_delete(self.inner.screen);
            }
        }
        self.inner.screen = root.raw();
        root
    }

    /// Render the active screen.
    pub fn render(&mut self) -> Frame {
        // SAFETY: the buffer holds SCREEN_W * SCREEN_H RGB565 pixels.
        unsafe {
            lv_refr_now(self.inner.disp);
            let px = std::slice::from_raw_parts(self.inner.buf, (SCREEN_W * SCREEN_H) as usize);
            Frame::from_rgb565(px)
        }
    }

    /// Fire the events a finger tap produces.
    pub fn click(&mut self, obj: Obj) {
        for code in [
            lv_event_code_t_LV_EVENT_PRESSED,
            lv_event_code_t_LV_EVENT_RELEASED,
            lv_event_code_t_LV_EVENT_CLICKED,
        ] {
            self.send(obj, code);
        }
    }

    pub fn send(&mut self, obj: Obj, code: lv_event_code_t) {
        // SAFETY: obj belongs to the active test screen.
        unsafe {
            lv_obj_send_event(obj.raw(), code, std::ptr::null_mut());
        }
    }

    /// Advance LVGL's clock and run due timers.
    pub fn advance(&mut self, ms: u32) {
        // SAFETY: plain LVGL calls under the test lock.
        unsafe {
            lv_tick_inc(ms);
            lv_timer_handler();
        }
    }
}

/// A rendered frame as RGB888.
pub struct Frame {
    rgb: Vec<u8>,
}

impl Frame {
    fn from_rgb565(px: &[u16]) -> Self {
        let mut rgb = Vec::with_capacity(px.len() * 3);
        for &value in px {
            let r = u32::from((value >> 11) & 0x1f);
            let g = u32::from((value >> 5) & 0x3f);
            let b = u32::from(value & 0x1f);
            rgb.extend_from_slice(&[(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]);
        }
        Self { rgb }
    }

    pub fn pixel(&self, x: i32, y: i32) -> [u8; 3] {
        let i = ((y * SCREEN_W + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    pub fn to_png(&self) -> Vec<u8> {
        encode(&self.rgb)
    }

    /// The frame with everything outside the round panel dimmed, for review.
    fn masked(&self) -> Vec<u8> {
        let mut rgb = self.rgb.clone();
        for y in 0..SCREEN_H {
            for x in 0..SCREEN_W {
                if (x - PANEL_CX).pow(2) + (y - PANEL_CY).pow(2) > PANEL_R * PANEL_R {
                    let i = ((y * SCREEN_W + x) * 3) as usize;
                    for channel in &mut rgb[i..i + 3] {
                        *channel /= 4;
                    }
                }
            }
        }
        rgb
    }
}

fn encode(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, SCREEN_W as u32, SCREEN_H as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(rgb).expect("png data");
    }
    out
}

fn decode(bytes: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("png info");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    buf
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Compare `frame` with `golden/<name>.png`.
///
/// `UPDATE_GOLDEN=1` writes the golden instead. `UI_PREVIEW=1` also writes a
/// panel-masked copy to `target/ui-preview/` for side-by-side review.
pub fn assert_golden(name: &str, frame: &Frame) {
    let path = crate_dir().join("golden").join(format!("{name}.png"));
    if std::env::var_os("UI_PREVIEW").is_some() {
        let dir = crate_dir().join("../../target/ui-preview");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.png")), encode(&frame.masked())).unwrap();
    }
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, frame.to_png()).unwrap();
        return;
    }
    let expected = std::fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}: run with UPDATE_GOLDEN=1, then compare it with docs/mockups/png",
            path.display()
        )
    });
    if decode(&expected) != frame.rgb {
        let dir = crate_dir().join("../../target/golden-diff");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.actual.png")), frame.to_png()).unwrap();
        panic!("{name} differs from its golden; see target/golden-diff/{name}.actual.png");
    }
}

/// Clickable objects under `root` that reach past the tap limit or are
/// smaller than 44 px (counting any extended click area). Hidden subtrees are skipped.
pub fn target_problems(root: Obj) -> Vec<String> {
    root.update_layout();
    let mut problems = Vec::new();
    visit(root, &mut |obj| {
        if obj == root || obj.is_hidden() || !obj.is_clickable() {
            return;
        }
        let a = obj.coords();
        let visible = Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1);
        if !visible.within_tap_limit() {
            problems.push(format!("{visible:?} reaches past the r{TAP_LIMIT_R} tap limit"));
        }
        let c = obj.click_area();
        let (w, h) = (c.x2 - c.x1 + 1, c.y2 - c.y1 + 1);
        if w < MIN_TARGET || h < MIN_TARGET {
            problems.push(format!("{visible:?} is a {w}x{h} target, below {MIN_TARGET} px"));
        }
    });
    problems
}

pub fn assert_targets_ok(root: Obj) {
    let problems = target_problems(root);
    assert!(problems.is_empty(), "tap target problems:\n{}", problems.join("\n"));
}

fn visit(obj: Obj, f: &mut dyn FnMut(Obj)) {
    f(obj);
    if obj.is_hidden() {
        return;
    }
    for i in 0..obj.child_count() {
        visit(obj.child(i as i32), f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::tokens::Rect;

    #[test]
    fn renders_a_black_screen() {
        lvgl(|lv| {
            lv.screen();
            assert_eq!(lv.render().pixel(200, 152), [0, 0, 0]);
        });
    }

    #[test]
    fn renders_a_coloured_box() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root).rect(Rect::new(10, 20, 30, 40)).bg(0xff0000);
            let frame = lv.render();
            assert_eq!(frame.pixel(25, 40), [255, 0, 0]);
            assert_eq!(frame.pixel(5, 5), [0, 0, 0]);
        });
    }

    #[test]
    fn unstyled_theme_objects_render_dark() {
        lvgl(|lv| {
            let root = lv.screen();
            // SAFETY: a raw themed object, deliberately without kit styling.
            unsafe {
                let card = lv_obj_create(root.raw());
                lv_obj_set_pos(card, 100, 100);
                lv_obj_set_size(card, 100, 60);
            }
            let [r, g, b] = lv.render().pixel(150, 130);
            assert!(r < 100 && g < 100 && b < 100, "theme card is dark, got {r},{g},{b}");
        });
    }

    #[test]
    fn target_check_flags_far_and_small_targets() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root).rect(Rect::new(8, 8, 56, 64)).clickable(true);
            Obj::plain(root).rect(Rect::new(180, 140, 30, 30)).clickable(true);
            Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
            let problems = target_problems(root);
            assert_eq!(problems.len(), 2, "{problems:#?}");
        });
    }

    #[test]
    fn png_round_trips() {
        lvgl(|lv| {
            let root = lv.screen();
            Obj::plain(root).rect(Rect::new(0, 0, 50, 50)).bg(0x3fd6b1);
            let frame = lv.render();
            assert_eq!(decode(&frame.to_png()), frame.rgb);
        });
    }
}
```

Update `kit/mod.rs`:

```rust
pub mod event;
pub mod fonts;
pub mod icons;
pub mod obj;
pub mod tokens;
#[cfg(test)]
pub mod testkit;
```

- [ ] **Step 3: Write the failing `Obj` and event tests**

`obj.rs`, tests only for now:

```rust
//! A thin, copyable handle over `lv_obj_t` with chainable setters. Screens and
//! components use this instead of raw FFI. Call it on the UI thread only; the
//! raw pointer keeps `Obj` from being `Send`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::{Rect, TEXT};

    #[test]
    fn plain_objects_start_transparent_and_unclickable() {
        lvgl(|lv| {
            let root = lv.screen();
            let obj = Obj::plain(root).rect(Rect::new(0, 0, 20, 20));
            assert!(!obj.is_clickable());
            assert_eq!(lv.render().pixel(10, 10), [0, 0, 0]);
        });
    }

    #[test]
    fn labels_truncate_inside_their_width() {
        lvgl(|lv| {
            let root = lv.screen();
            let label = Obj::label(root, "Board room network with a very long name", Font::Body, TEXT)
                .centered_text(120)
                .pos(140, 100);
            label.update_layout();
            let a = label.coords();
            assert!(a.x2 - a.x1 + 1 <= 120, "label is {} px wide", a.x2 - a.x1 + 1);
            assert_eq!(a.y2 - a.y1 + 1, Font::Body.line_height(), "one line, not wrapped");
        });
    }

    #[test]
    fn text_with_a_nul_byte_is_kept_without_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let label = Obj::label(root, "Mic\0 on", Font::Body, TEXT);
            assert_eq!(label.text(), "Mic on");
        });
    }
}
```

`event.rs`, tests only for now:

```rust
//! Event handlers whose captured state is freed when LVGL deletes the object.
//! Replaces the legacy `Box::into_raw` user data that leaked on every navigation.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::Rect;
    use std::cell::Cell;
    use std::rc::Rc;

    struct DropCounter(Rc<Cell<u32>>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 100);
        }
    }

    #[test]
    fn clicks_run_the_handler_and_deleting_frees_it() {
        lvgl(|lv| {
            let root = lv.screen();
            let counter = Rc::new(Cell::new(0));
            let button = Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
            let guard = DropCounter(counter.clone());
            let clicks = counter.clone();
            on_click(button, move || {
                let _owned = &guard;
                clicks.set(clicks.get() + 1);
            });
            lv.click(button);
            lv.click(button);
            assert_eq!(counter.get(), 2);
            button.delete();
            assert_eq!(counter.get(), 102, "deleting the button drops the handler");
        });
    }

    #[test]
    fn handlers_see_the_event_code() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(Cell::new(0u32));
            let target = Obj::plain(root).rect(Rect::new(150, 120, 100, 60)).clickable(true);
            let record = seen.clone();
            on(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED, move |event| {
                record.set(event.code());
            });
            lv.send(target, crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(seen.get(), crate::bindings::lv_event_code_t_LV_EVENT_PRESSED);
        });
    }
}
```

- [ ] **Step 4: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit:: 2>&1 | tail -5`
Expected: compile errors (`Obj`, `on_click`, `on` not found)

- [ ] **Step 5: Implement `Obj`** above the tests in `obj.rs`:

```rust
use crate::bindings::*;
use crate::kit::fonts::Font;
use crate::kit::tokens::{Rect, BG, SCREEN_H, SCREEN_W};
use std::ffi::{CStr, CString};

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;

/// 0xRRGGBB to an LVGL colour.
pub fn color(hex: u32) -> lv_color_t {
    // SAFETY: pure conversion.
    unsafe { lv_color_hex(hex) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Obj(*mut lv_obj_t);

impl Obj {
    pub fn from_raw(raw: *mut lv_obj_t) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> *mut lv_obj_t {
        self.0
    }

    pub fn is_null(self) -> bool {
        self.0.is_null()
    }

    /// A new top-level screen: 400×304, black, no padding, not scrollable.
    pub fn screen() -> Obj {
        // SAFETY: a null parent creates a screen.
        unsafe { Obj(lv_obj_create(std::ptr::null_mut())) }
            .reset()
            .size(SCREEN_W, SCREEN_H)
            .bg(BG)
    }

    /// A container with every theme style removed: transparent, no padding,
    /// border or scrolling, not clickable.
    pub fn plain(parent: Obj) -> Obj {
        // SAFETY: parent is a live object.
        unsafe { Obj(lv_obj_create(parent.0)) }.reset()
    }

    pub fn label(parent: Obj, text: &str, font: Font, hex: u32) -> Obj {
        // SAFETY: parent is a live object.
        let label = unsafe { Obj(lv_label_create(parent.0)) }.reset();
        label.set_text(text);
        label.font(font).text_color(hex)
    }

    pub fn arc(parent: Obj) -> Obj {
        // SAFETY: parent is a live object.
        unsafe { Obj(lv_arc_create(parent.0)) }.reset()
    }

    pub fn line(parent: Obj) -> Obj {
        // SAFETY: parent is a live object.
        unsafe { Obj(lv_line_create(parent.0)) }.reset()
    }

    fn reset(self) -> Self {
        // SAFETY: self is a live object.
        unsafe {
            lv_obj_remove_style(
                self.0,
                std::ptr::null(),
                (LV_PART_ANY | LV_STATE_ANY) as lv_style_selector_t,
            );
            lv_obj_remove_flag(self.0, lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_remove_flag(self.0, lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE);
        }
        self
    }

    pub fn pos(self, x: i32, y: i32) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_pos(self.0, x, y) };
        self
    }

    pub fn size(self, w: i32, h: i32) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_size(self.0, w, h) };
        self
    }

    pub fn rect(self, r: Rect) -> Self {
        self.pos(r.x, r.y).size(r.w, r.h)
    }

    pub fn width(self, w: i32) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_width(self.0, w) };
        self
    }

    pub fn bg(self, hex: u32) -> Self {
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_bg_color(self.0, color(hex), MAIN);
            lv_obj_set_style_bg_opa(self.0, LV_OPA_COVER as lv_opa_t, MAIN);
        }
        self
    }

    pub fn bg_opa(self, opa: u8) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_style_bg_opa(self.0, opa, MAIN) };
        self
    }

    pub fn radius(self, r: i32) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_style_radius(self.0, r, MAIN) };
        self
    }

    pub fn round(self) -> Self {
        self.radius(LV_RADIUS_CIRCLE as i32)
    }

    pub fn border(self, width: i32, hex: u32) -> Self {
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_border_width(self.0, width, MAIN);
            lv_obj_set_style_border_color(self.0, color(hex), MAIN);
            lv_obj_set_style_border_opa(self.0, LV_OPA_COVER as lv_opa_t, MAIN);
        }
        self
    }

    pub fn font(self, font: Font) -> Self {
        // SAFETY: live object; static font.
        unsafe { lv_obj_set_style_text_font(self.0, font.ptr(), MAIN) };
        self
    }

    pub fn text_color(self, hex: u32) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_style_text_color(self.0, color(hex), MAIN) };
        self
    }

    pub fn opa(self, opa: u8) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_style_opa(self.0, opa, MAIN) };
        self
    }

    /// Centre the text across `width` on one line, ending in "..." if too long.
    pub fn centered_text(self, width: i32) -> Self {
        // SAFETY: live label.
        unsafe { lv_obj_set_style_text_align(self.0, lv_text_align_t_LV_TEXT_ALIGN_CENTER, MAIN) };
        self.width(width).one_line()
    }

    /// Keep a label on one line, ending in "..." when the text is wider than
    /// the label. LVGL only truncates a label whose height is fixed; with the
    /// default content height it wraps onto more lines instead. So this sets
    /// the height to one line of the current font: call it after `font`, and
    /// again whenever the font changes.
    pub fn one_line(self) -> Self {
        // SAFETY: live label. LV_STYLE_TEXT_FONT always resolves to a font
        // (inherited, or LV_FONT_DEFAULT).
        unsafe {
            let font = lv_obj_get_style_prop(self.0, MAIN, LV_STYLE_TEXT_FONT as lv_style_prop_t);
            lv_obj_set_height(self.0, lv_font_get_line_height(font.ptr.cast()));
            lv_label_set_long_mode(self.0, lv_label_long_mode_t_LV_LABEL_LONG_DOT);
        }
        self
    }

    /// Background while pressed.
    pub fn pressed_bg(self, hex: u32) -> Self {
        let pressed = MAIN | LV_STATE_PRESSED as lv_style_selector_t;
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_bg_color(self.0, color(hex), pressed);
            lv_obj_set_style_bg_opa(self.0, LV_OPA_COVER as lv_opa_t, pressed);
        }
        self
    }

    pub fn set_text(self, text: &str) {
        let text = CString::new(text.replace('\0', "")).expect("NUL bytes removed");
        // SAFETY: live label; LVGL copies the text.
        unsafe { lv_label_set_text(self.0, text.as_ptr()) };
    }

    pub fn text(self) -> String {
        // SAFETY: live label; LVGL returns its own NUL-terminated copy.
        unsafe { CStr::from_ptr(lv_label_get_text(self.0)).to_string_lossy().into_owned() }
    }

    pub fn clickable(self, on: bool) -> Self {
        self.flag(lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE, on)
    }

    pub fn hidden(self, on: bool) -> Self {
        self.flag(lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN, on)
    }

    pub fn flag(self, flag: lv_obj_flag_t, on: bool) -> Self {
        // SAFETY: live object.
        unsafe {
            if on {
                lv_obj_add_flag(self.0, flag)
            } else {
                lv_obj_remove_flag(self.0, flag)
            }
        }
        self
    }

    /// Grow the tap area by `px` on every side without changing the visible box.
    pub fn ext_click(self, px: i32) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_ext_click_area(self.0, px) };
        self
    }

    pub fn set_state(self, state: u32, on: bool) {
        // SAFETY: live object.
        unsafe {
            if on {
                lv_obj_add_state(self.0, state as lv_state_t)
            } else {
                lv_obj_remove_state(self.0, state as lv_state_t)
            }
        }
    }

    pub fn has_state(self, state: u32) -> bool {
        // SAFETY: live object.
        unsafe { lv_obj_has_state(self.0, state as lv_state_t) }
    }

    pub fn has_flag(self, flag: lv_obj_flag_t) -> bool {
        // SAFETY: live object.
        unsafe { lv_obj_has_flag(self.0, flag) }
    }

    pub fn is_clickable(self) -> bool {
        self.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE)
    }

    pub fn is_hidden(self) -> bool {
        self.has_flag(lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN)
    }

    pub fn coords(self) -> lv_area_t {
        let mut area = lv_area_t::default();
        // SAFETY: live object.
        unsafe { lv_obj_get_coords(self.0, &mut area) };
        area
    }

    pub fn click_area(self) -> lv_area_t {
        let mut area = lv_area_t::default();
        // SAFETY: live object.
        unsafe { lv_obj_get_click_area(self.0, &mut area) };
        area
    }

    pub fn child_count(self) -> u32 {
        // SAFETY: live object.
        unsafe { lv_obj_get_child_count(self.0) }
    }

    pub fn child(self, index: i32) -> Obj {
        // SAFETY: live object.
        unsafe { Obj(lv_obj_get_child(self.0, index)) }
    }

    pub fn update_layout(self) {
        // SAFETY: live object.
        unsafe { lv_obj_update_layout(self.0) };
    }

    /// Delete the object and its children; their handlers are dropped.
    pub fn delete(self) {
        // SAFETY: live object; not used afterwards by the caller.
        unsafe { lv_obj_delete(self.0) };
    }
}
```

- [ ] **Step 6: Implement the event layer** above the tests in `event.rs`:

```rust
use crate::bindings::*;
use crate::kit::obj::Obj;

type Handler = Box<dyn FnMut(&Event)>;

/// What a handler sees about the event that fired.
pub struct Event {
    raw: *mut lv_event_t,
}

impl Event {
    pub fn code(&self) -> lv_event_code_t {
        // SAFETY: valid for the duration of the callback.
        unsafe { lv_event_get_code(self.raw) }
    }

    pub fn target(&self) -> Obj {
        // SAFETY: valid for the duration of the callback.
        unsafe { Obj::from_raw(lv_event_get_target(self.raw).cast()) }
    }
}

/// Run `handler` whenever `obj` emits `code`. The closure is dropped when
/// `obj` is deleted, so captured state never leaks.
pub fn on(obj: Obj, code: lv_event_code_t, handler: impl FnMut(&Event) + 'static) {
    let boxed: *mut Handler = Box::into_raw(Box::new(Box::new(handler)));
    // SAFETY: `boxed` stays valid until `release` runs on LV_EVENT_DELETE.
    unsafe {
        lv_obj_add_event_cb(obj.raw(), Some(dispatch), code, boxed.cast());
        lv_obj_add_event_cb(obj.raw(), Some(release), lv_event_code_t_LV_EVENT_DELETE, boxed.cast());
    }
}

/// Run `handler` on every tap of `obj`.
pub fn on_click(obj: Obj, mut handler: impl FnMut() + 'static) {
    on(obj, lv_event_code_t_LV_EVENT_CLICKED, move |_| handler());
}

unsafe extern "C" fn dispatch(e: *mut lv_event_t) {
    let handler = lv_event_get_user_data(e) as *mut Handler;
    if !handler.is_null() {
        (*handler)(&Event { raw: e });
    }
}

unsafe extern "C" fn release(e: *mut lv_event_t) {
    let handler = lv_event_get_user_data(e) as *mut Handler;
    if !handler.is_null() {
        drop(Box::from_raw(handler));
    }
}
```

- [ ] **Step 7: Run the tests**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit:: 2>&1 | grep "test result"`
Expected: `ok. 18 passed` (4 fonts, 4 tokens, 5 testkit, 3 obj, 2 event)

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`
Expected: `ok. 87 passed`

- [ ] **Step 8: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit crates/waved-ui/Cargo.toml crates/waved-ui/lv_conf.h
git commit -m "feat(ui): kit Obj handle, leak-free event handlers, headless LVGL tests

- Obj wraps lv_obj_t with chainable setters; screens stop calling raw FFI
- on()/on_click() drop captured state on LV_EVENT_DELETE
- testkit renders frames on the host, compares golden PNGs, checks tap targets
- dark default theme so unstyled widgets never render as white discs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Routes, dispatcher, snapshot and the `Screen` trait

**Files:**
- Create: `crates/waved-ui/src/kit/route.rs`, `dispatch.rs`, `snapshot.rs`, `screen.rs`
- Modify: `crates/waved-ui/src/kit/mod.rs`
- Test: each new file (`mod tests`)

**Interfaces:**
- Consumes: `waved_state::{StateBus, Command, …State}` and `waved_config::WavedConfig`.
- Produces:
  - **`Route`:** `Route { Boot, FirstRun, Home, Recording, Decision, Resting, Menu, AudioMode, NoiseReduction, Volume, MicLevel, MicHeight, Files, FileDetail(Uuid), Wifi, WifiSetup, Bluetooth, Uploads, UploadMode, System, Display, DateTime, About, Confirm(ConfirmKind), Pin(Box<Route>), UsbStorage, Update, IncomingCall }` with `is_base()` and `requires_pin()`.
  - **`ConfirmKind`:** `ConfirmKind { PowerOff, FactoryReset, DeleteRecording(Uuid), StopUsbStorage }`.
  - **Messages:** `Severity { Success, Info, Warning, Error }` with `duration_ms()`, and `Message { text: String, severity: Severity }` with `new(severity, text)`.
  - **`UiEvent`:** `UiEvent { Navigate(Route), Back, ToBase, PinAccepted, BootDone, Rest, Wake, Show(Message) }`.
  - **`Dispatcher`** (`Clone`):
    - `new(bus: broadcast::Sender<Command>, ui: mpsc::Sender<UiEvent>)`
    - methods `command(Command)`, `ui(UiEvent)`, `navigate(Route)`, `back()`, `show(Severity, text)`
    - `#[cfg(test)] test() -> (Dispatcher, broadcast::Receiver<Command>, mpsc::Receiver<UiEvent>)`
  - **`UiSnapshot`:**
    - fields `audio, meeting, battery, bt, wifi, provisioning, mcu, settings, files, playback, now_unix_s: i64, now_ms: u64, clock: String`
    - constructor `capture(bus: &StateBus, now_unix_s: i64, now_ms: u64) -> Self`
    - helpers `in_call()`, `audio_in_use()`, `connection_line() -> String`
  - **Clock:** `format_clock(unix_s: i64, offset_minutes: i32, clock_24h: bool) -> String`.
  - **Screens:** `ScreenCtx { dispatch: Dispatcher, config: Arc<WavedConfig> }` and `trait Screen { fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot); fn update(&mut self, snap: &UiSnapshot); }`.

- [ ] **Step 1: Write the failing tests**

`route.rs`:

```rust
//! Every screen the redesign has, as a navigation target.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_guards_exactly_the_d10_entry_points() {
        let guarded = [Route::Files, Route::Wifi, Route::Bluetooth, Route::Uploads, Route::System];
        for route in guarded {
            assert!(route.requires_pin(), "{route:?}");
        }
        for open in [Route::Home, Route::Menu, Route::AudioMode, Route::NoiseReduction, Route::Volume, Route::MicHeight] {
            assert!(!open.requires_pin(), "{open:?}");
        }
    }

    #[test]
    fn only_meeting_screens_are_bases() {
        assert!(Route::Home.is_base() && Route::Recording.is_base() && Route::Decision.is_base());
        assert!(!Route::Menu.is_base() && !Route::Resting.is_base());
    }
}
```

`dispatch.rs`:

```rust
//! How screens talk to services (bus commands) and to the UI runtime (UiEvent).

#[cfg(test)]
mod tests {
    use super::*;
    use waved_state::Command;

    #[test]
    fn commands_reach_the_bus_and_ui_events_the_runtime() {
        let (dispatch, mut bus, ui) = Dispatcher::test();
        dispatch.command(Command::ToggleMicMute);
        dispatch.navigate(Route::Menu);
        dispatch.back();
        dispatch.show(Severity::Success, "Recording sent");
        assert!(matches!(bus.try_recv(), Ok(Command::ToggleMicMute)));
        assert_eq!(ui.try_recv().unwrap(), UiEvent::Navigate(Route::Menu));
        assert_eq!(ui.try_recv().unwrap(), UiEvent::Back);
        assert_eq!(
            ui.try_recv().unwrap(),
            UiEvent::Show(Message::new(Severity::Success, "Recording sent"))
        );
    }

    #[test]
    fn warnings_and_errors_stay_longer() {
        assert_eq!(Severity::Success.duration_ms(), 2_000);
        assert_eq!(Severity::Info.duration_ms(), 2_000);
        assert_eq!(Severity::Warning.duration_ms(), 4_000);
        assert_eq!(Severity::Error.duration_ms(), 4_000);
    }
}
```

`snapshot.rs`:

```rust
//! One consistent copy of every state channel per UI loop iteration.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_follow_the_setting() {
        let t = 1_759_588_320; // 2025-10-04 14:32:00 UTC
        assert_eq!(format_clock(t, 0, true), "14:32");
        assert_eq!(format_clock(t, 0, false), "2:32 pm");
        assert_eq!(format_clock(t, 120, true), "16:32");
        assert_eq!(format_clock(0, 0, false), "12:00 am");
        assert_eq!(format_clock(-60, 0, true), "23:59");
    }

    #[test]
    fn connection_line_matches_the_dial_table() {
        let mut snap = UiSnapshot::default();
        assert_eq!(snap.connection_line(), "Not connected");
        snap.mcu.usb_connected = true;
        assert_eq!(snap.connection_line(), "Connected by USB");
        snap.audio.host_listening = true;
        assert_eq!(snap.connection_line(), "Computer is using the mic");
        snap.audio.call_off_hook = true;
        assert_eq!(snap.connection_line(), "In a call");
        snap.audio.mic_muted = true;
        assert_eq!(snap.connection_line(), "Call can't hear you");

        let mut bt = UiSnapshot::default();
        bt.bt.connected = true;
        bt.bt.device_name = "iPhone".into();
        assert_eq!(bt.connection_line(), "Bluetooth: iPhone");
        bt.audio.mic_muted = true;
        assert_eq!(bt.connection_line(), "Mic is off");
    }

    #[test]
    fn audio_in_use_means_the_mic_signal_goes_somewhere() {
        let mut snap = UiSnapshot::default();
        assert!(!snap.audio_in_use());
        snap.meeting.recording = true;
        assert!(snap.audio_in_use());
    }

    #[test]
    fn capture_copies_the_bus_and_formats_the_clock() {
        let bus = StateBus::new();
        let _keep = bus.settings.subscribe();
        bus.settings.send_modify(|s| s.clock_24h = false);
        let snap = UiSnapshot::capture(&bus, 1_759_588_320, 42);
        assert!(!snap.settings.clock_24h);
        assert_eq!(snap.clock, "2:32 pm");
        assert_eq!(snap.now_ms, 42);
    }
}
```

`screen.rs`:

```rust
//! The contract every new screen implements.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fonts::Font;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::TEXT;

    struct Speaker {
        label: Option<Obj>,
    }

    impl Screen for Speaker {
        fn build(&mut self, root: Obj, _ctx: &ScreenCtx, snap: &UiSnapshot) {
            self.label = Some(Obj::label(root, "", Font::Body, TEXT).pos(100, 100));
            self.update(snap);
        }

        fn update(&mut self, snap: &UiSnapshot) {
            if let Some(label) = self.label {
                label.set_text(&format!("Volume {}%", snap.audio.speaker_volume));
            }
        }
    }

    #[test]
    fn screens_build_into_a_root_and_follow_snapshots() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let ctx = ScreenCtx { dispatch, config: Arc::new(WavedConfig::default()) };
            let mut snap = UiSnapshot::default();
            snap.audio.speaker_volume = 60;
            let mut screen = Speaker { label: None };
            screen.build(root, &ctx, &snap);
            assert_eq!(screen.label.unwrap().text(), "Volume 60%");
            snap.audio.speaker_volume = 70;
            screen.update(&snap);
            assert_eq!(screen.label.unwrap().text(), "Volume 70%");
        });
    }
}
```

Add to `kit/mod.rs`: `pub mod dispatch; pub mod route; pub mod screen; pub mod snapshot;` (keep the list sorted).

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit:: 2>&1 | tail -3`
Expected: compile errors (`Route`, `Dispatcher`, `UiSnapshot`, `Screen` missing)

- [ ] **Step 3: Implement `route.rs`** above its tests:

```rust
use uuid::Uuid;

/// What a confirmation dialog confirms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    PowerOff,
    FactoryReset,
    DeleteRecording(Uuid),
    StopUsbStorage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Boot,
    FirstRun,
    Home,
    Recording,
    Decision,
    Resting,
    Menu,
    AudioMode,
    NoiseReduction,
    Volume,
    MicLevel,
    MicHeight,
    Files,
    FileDetail(Uuid),
    Wifi,
    WifiSetup,
    Bluetooth,
    Uploads,
    UploadMode,
    System,
    Display,
    DateTime,
    About,
    Confirm(ConfirmKind),
    /// PIN entry in front of a locked route.
    Pin(Box<Route>),
    UsbStorage,
    Update,
    IncomingCall,
}

impl Route {
    /// The bottom of the navigation stack, chosen by the meeting state.
    pub fn is_base(&self) -> bool {
        matches!(self, Route::Home | Route::Recording | Route::Decision)
    }

    /// Entry points the PIN protects (D10). Screens opened from them inherit the unlock.
    pub fn requires_pin(&self) -> bool {
        matches!(
            self,
            Route::Files | Route::Wifi | Route::Bluetooth | Route::Uploads | Route::System
        )
    }
}
```

- [ ] **Step 4: Implement `dispatch.rs`** above its tests:

```rust
use crate::kit::route::Route;
use crate::kit::tokens::{MESSAGE_MS_LONG, MESSAGE_MS_SHORT};
use std::sync::mpsc;
use tokio::sync::broadcast;
use waved_state::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Success,
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn duration_ms(self) -> u64 {
        match self {
            Severity::Success | Severity::Info => MESSAGE_MS_SHORT,
            Severity::Warning | Severity::Error => MESSAGE_MS_LONG,
        }
    }
}

/// A one-line message shown in the status slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub severity: Severity,
}

impl Message {
    pub fn new(severity: Severity, text: impl Into<String>) -> Self {
        Self { text: text.into(), severity }
    }
}

/// Requests for the UI runtime (navigation, messages, power states).
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Navigate(Route),
    Back,
    /// Return to the meeting base (Home, Recording or Decision).
    ToBase,
    /// The PIN screen accepted the code; open the route it guarded.
    PinAccepted,
    /// Services are up; leave the start-up screen.
    BootDone,
    /// No touch for the dim timeout: show the resting screen and relock.
    Rest,
    /// A touch or the power key woke the screen.
    Wake,
    Show(Message),
}

#[derive(Clone)]
pub struct Dispatcher {
    bus: broadcast::Sender<Command>,
    ui: mpsc::Sender<UiEvent>,
}

impl Dispatcher {
    pub fn new(bus: broadcast::Sender<Command>, ui: mpsc::Sender<UiEvent>) -> Self {
        Self { bus, ui }
    }

    /// Send a command to the services. Like the legacy UI, a send with no
    /// subscribers is dropped.
    pub fn command(&self, command: Command) {
        let _ = self.bus.send(command);
    }

    pub fn ui(&self, event: UiEvent) {
        let _ = self.ui.send(event);
    }

    pub fn navigate(&self, route: Route) {
        self.ui(UiEvent::Navigate(route));
    }

    pub fn back(&self) {
        self.ui(UiEvent::Back);
    }

    pub fn show(&self, severity: Severity, text: impl Into<String>) {
        self.ui(UiEvent::Show(Message::new(severity, text)));
    }

    /// A dispatcher wired to fresh channels, for tests.
    #[cfg(test)]
    pub fn test() -> (Self, broadcast::Receiver<Command>, mpsc::Receiver<UiEvent>) {
        let (bus, bus_rx) = broadcast::channel(64);
        let (ui, ui_rx) = mpsc::channel();
        (Self::new(bus, ui), bus_rx, ui_rx)
    }
}
```

- [ ] **Step 5: Implement `snapshot.rs`** above its tests:

```rust
use waved_state::{
    AudioState, BatteryState, BluetoothState, FilesState, McuState, MeetingState, PlaybackState,
    ProvisioningState, SettingsState, StateBus, WifiState,
};

#[derive(Clone, Debug, Default)]
pub struct UiSnapshot {
    pub audio: AudioState,
    pub meeting: MeetingState,
    pub battery: BatteryState,
    pub bt: BluetoothState,
    pub wifi: WifiState,
    pub provisioning: ProvisioningState,
    pub mcu: McuState,
    pub settings: SettingsState,
    pub files: FilesState,
    pub playback: PlaybackState,
    /// Wall clock, seconds since the Unix epoch.
    pub now_unix_s: i64,
    /// Monotonic milliseconds for UI timers (messages, gauge, hold to stop).
    pub now_ms: u64,
    /// Status clock text in the configured format.
    pub clock: String,
}

impl UiSnapshot {
    pub fn capture(bus: &StateBus, now_unix_s: i64, now_ms: u64) -> Self {
        let settings = bus.settings.borrow().clone();
        // Time zones arrive with workstream E3; until then the clock shows UTC.
        let clock = format_clock(now_unix_s, 0, settings.clock_24h);
        Self {
            audio: bus.audio.borrow().clone(),
            meeting: bus.meeting.borrow().clone(),
            battery: bus.battery.borrow().clone(),
            bt: bus.bt.borrow().clone(),
            wifi: bus.wifi.borrow().clone(),
            provisioning: bus.provisioning.borrow().clone(),
            mcu: bus.mcu.borrow().clone(),
            settings,
            files: bus.files.borrow().clone(),
            playback: bus.playback.borrow().clone(),
            now_unix_s,
            now_ms,
            clock,
        }
    }

    /// The host signals an active call (HID off-hook).
    pub fn in_call(&self) -> bool {
        self.audio.call_off_hook
    }

    /// The microphone signal goes somewhere: a call, the computer or a recording.
    pub fn audio_in_use(&self) -> bool {
        self.audio.call_off_hook || self.audio.host_listening || self.meeting.recording
    }

    /// Second line of the Home dial ("Dial states" in docs/mockups/README.md).
    pub fn connection_line(&self) -> String {
        if self.audio.mic_muted {
            return if self.in_call() || self.audio.host_listening {
                "Call can't hear you".into()
            } else {
                "Mic is off".into()
            };
        }
        if self.in_call() {
            "In a call".into()
        } else if self.audio.host_listening {
            "Computer is using the mic".into()
        } else if self.mcu.usb_connected {
            "Connected by USB".into()
        } else if self.bt.connected && !self.bt.device_name.is_empty() {
            format!("Bluetooth: {}", self.bt.device_name)
        } else if self.bt.connected {
            "Connected by Bluetooth".into()
        } else {
            "Not connected".into()
        }
    }
}

/// "14:32" or "2:32 pm".
pub fn format_clock(unix_s: i64, offset_minutes: i32, clock_24h: bool) -> String {
    let minutes = (unix_s + i64::from(offset_minutes) * 60).rem_euclid(86_400) / 60;
    let (hour, minute) = (minutes / 60, minutes % 60);
    if clock_24h {
        format!("{hour:02}:{minute:02}")
    } else {
        let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        let suffix = if hour < 12 { "am" } else { "pm" };
        format!("{hour12}:{minute:02} {suffix}")
    }
}
```

- [ ] **Step 6: Implement `screen.rs`** above its tests:

```rust
use crate::kit::dispatch::Dispatcher;
use crate::kit::obj::Obj;
use crate::kit::snapshot::UiSnapshot;
use std::sync::Arc;
use waved_config::WavedConfig;

/// What every screen receives when it is built.
#[derive(Clone)]
pub struct ScreenCtx {
    pub dispatch: Dispatcher,
    pub config: Arc<WavedConfig>,
}

/// A full-screen view. The runtime builds a fresh instance for each navigation
/// and deletes `root`, with every handler, when navigating away.
pub trait Screen {
    /// Build widgets into `root`, a black 400×304 screen owned by the runtime.
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot);
    /// Refresh from the latest snapshot; called on every UI loop iteration.
    fn update(&mut self, snap: &UiSnapshot);
}
```

- [ ] **Step 7: Run the tests**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit:: 2>&1 | grep "test result"`
Expected: `ok. 27 passed`

- [ ] **Step 8: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit
git commit -m "feat(ui): kit routes, dispatcher, snapshot and Screen trait

- Route lists every redesign screen; requires_pin follows D10
- Dispatcher separates bus commands from UI navigation and messages
- UiSnapshot captures all channels once per loop; dial connection line and clock

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Router

Pure navigation logic with no LVGL. The milestone-2 runtime calls `handle` for each `UiEvent` and `sync` for each snapshot, then rebuilds the screen when `current()` changes.

**Files:**
- Create: `crates/waved-ui/src/kit/router.rs`
- Modify: `crates/waved-ui/src/kit/mod.rs` (`pub mod router;`)
- Test: `crates/waved-ui/src/kit/router.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Route`, `UiEvent`, `UiSnapshot`.
- Produces: `Router::new() -> Router` (starts at `Boot`), `current(&self) -> &Route`, `depth(&self) -> usize`, `base_for(&UiSnapshot) -> Route`, `handle(&mut self, &UiEvent, &UiSnapshot)` and `sync(&mut self, &UiSnapshot)`.
- Rules:
  - **Start-up.** `BootDone` leaves `Boot` for `FirstRun` until first start is done, and for the meeting base after that. `FirstRun` gives way to the base once `settings.first_run_done` turns true.
  - **Navigation.** `Navigate` pushes the route (and ignores a route that is already current). A PIN-guarded route is pushed as `Pin(route)` while the PIN is enabled and not yet unlocked. `PinAccepted` replaces `Pin(route)` with `route` and unlocks.
  - **Back.** `Back` pops but never below the base. `ToBase` keeps only the base.
  - **Resting.** `Rest` pushes `Resting` and relocks. `Wake` pops `Resting`.
  - **Meeting state.** `sync` swaps the base when the meeting state changes and keeps any sub-screens on top. A pending decision preempts everything.

- [ ] **Step 1: Write the failing tests**

```rust
//! Which screen is visible: a stack over a meeting-driven base.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::route::Route;
    use std::path::PathBuf;
    use uuid::Uuid;
    use waved_state::PendingDecision;

    fn ready() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.settings.first_run_done = true;
        snap
    }

    fn started(snap: &UiSnapshot) -> Router {
        let mut router = Router::new();
        router.handle(&UiEvent::BootDone, snap);
        router
    }

    fn decision() -> PendingDecision {
        PendingDecision {
            id: Uuid::new_v4(),
            audio_path: PathBuf::from("/mnt/udisk/recordings/x/audio.wav"),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            endpoint: "https://notes.example.com/hook".into(),
        }
    }

    #[test]
    fn boot_leads_to_first_start_until_it_is_done() {
        let mut snap = UiSnapshot::default();
        let mut router = Router::new();
        assert_eq!(router.current(), &Route::Boot);
        router.handle(&UiEvent::BootDone, &snap);
        assert_eq!(router.current(), &Route::FirstRun);
        snap.settings.first_run_done = true;
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Home);
    }

    #[test]
    fn navigation_walks_the_stack_and_back_stops_at_the_base() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Navigate(Route::AudioMode), &snap);
        router.handle(&UiEvent::Navigate(Route::AudioMode), &snap);
        assert_eq!((router.current(), router.depth()), (&Route::AudioMode, 3));
        router.handle(&UiEvent::Back, &snap);
        router.handle(&UiEvent::Back, &snap);
        router.handle(&UiEvent::Back, &snap);
        assert_eq!((router.current(), router.depth()), (&Route::Home, 1));
    }

    #[test]
    fn locked_routes_ask_for_the_pin_once_until_the_screen_rests() {
        let mut snap = ready();
        snap.settings.pin_enabled = true;
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        assert_eq!(router.current(), &Route::Pin(Box::new(Route::Files)));
        router.handle(&UiEvent::PinAccepted, &snap);
        assert_eq!(router.current(), &Route::Files);
        router.handle(&UiEvent::ToBase, &snap);
        router.handle(&UiEvent::Navigate(Route::System), &snap);
        assert_eq!(router.current(), &Route::System, "still unlocked");
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Wake, &snap);
        router.handle(&UiEvent::ToBase, &snap);
        router.handle(&UiEvent::Navigate(Route::Uploads), &snap);
        assert_eq!(router.current(), &Route::Pin(Box::new(Route::Uploads)), "resting relocks");
    }

    #[test]
    fn no_pin_means_no_pin_screen() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        assert_eq!(router.current(), &Route::Files);
    }

    #[test]
    fn a_recording_that_starts_under_a_submenu_replaces_the_base() {
        let mut snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        snap.meeting.recording = true;
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Menu, "the open menu stays");
        router.handle(&UiEvent::Back, &snap);
        assert_eq!(router.current(), &Route::Recording);
    }

    #[test]
    fn a_pending_decision_preempts_everything() {
        let mut snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Rest, &snap);
        snap.meeting.pending_decision = Some(decision());
        router.sync(&snap);
        assert_eq!((router.current(), router.depth()), (&Route::Decision, 1));
        snap.meeting.pending_decision = None;
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Home);
    }

    #[test]
    fn resting_sits_on_top_and_waking_returns() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Rest, &snap);
        assert_eq!((router.current(), router.depth()), (&Route::Resting, 2));
        router.handle(&UiEvent::Wake, &snap);
        assert_eq!(router.current(), &Route::Home);
    }

    #[test]
    fn nothing_navigates_away_from_boot() {
        let snap = ready();
        let mut router = Router::new();
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Rest, &snap);
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Boot);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::router 2>&1 | tail -3`
Expected: compile error `cannot find type \`Router\``

- [ ] **Step 3: Implement** above the tests:

```rust
use crate::kit::dispatch::UiEvent;
use crate::kit::route::Route;
use crate::kit::snapshot::UiSnapshot;

pub struct Router {
    stack: Vec<Route>,
    unlocked: bool,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Self {
        Self {
            stack: vec![Route::Boot],
            unlocked: false,
        }
    }

    pub fn current(&self) -> &Route {
        self.stack.last().expect("the stack always has a base")
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// The meeting-driven base screen.
    pub fn base_for(snap: &UiSnapshot) -> Route {
        if snap.meeting.recording {
            Route::Recording
        } else if snap.meeting.pending_decision.is_some() {
            Route::Decision
        } else {
            Route::Home
        }
    }

    pub fn handle(&mut self, event: &UiEvent, snap: &UiSnapshot) {
        if self.stack == [Route::Boot] {
            if *event == UiEvent::BootDone {
                self.stack[0] = if snap.settings.first_run_done {
                    Self::base_for(snap)
                } else {
                    Route::FirstRun
                };
            }
            return;
        }
        match event {
            UiEvent::Navigate(route) => {
                if self.current() == route {
                    return;
                }
                let guarded = route.requires_pin() && snap.settings.pin_enabled && !self.unlocked;
                self.stack.push(if guarded {
                    Route::Pin(Box::new(route.clone()))
                } else {
                    route.clone()
                });
            }
            UiEvent::Back => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                }
            }
            UiEvent::ToBase => self.stack.truncate(1),
            UiEvent::PinAccepted => {
                if let Some(Route::Pin(target)) = self.stack.last().cloned() {
                    *self.stack.last_mut().expect("non-empty") = *target;
                    self.unlocked = true;
                }
            }
            UiEvent::Rest => {
                self.unlocked = false;
                if *self.current() != Route::Resting {
                    self.stack.push(Route::Resting);
                }
            }
            UiEvent::Wake => {
                if *self.current() == Route::Resting {
                    self.stack.pop();
                }
            }
            UiEvent::BootDone | UiEvent::Show(_) => {}
        }
    }

    /// Follow the meeting state: swap the base and keep sub-screens on top,
    /// except a pending decision, which takes over the whole stack.
    pub fn sync(&mut self, snap: &UiSnapshot) {
        match self.stack[0] {
            Route::Boot => return,
            Route::FirstRun if !snap.settings.first_run_done => return,
            _ => {}
        }
        let base = Self::base_for(snap);
        if base == Route::Decision && self.stack[0] != Route::Decision {
            self.stack = vec![Route::Decision];
        } else if self.stack[0] != base {
            self.stack[0] = base;
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::router 2>&1 | grep "test result"`
Expected: `ok. 8 passed`

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit
git commit -m "feat(ui): router over a meeting-driven base with pin and resting rules

- Navigate/Back/ToBase with a base that follows recording and decisions
- PIN-guarded entry points unlock once until the screen rests (D10)
- a pending decision preempts any open screen

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Layout helpers and `StatusArea`

**Files:**
- Modify: `crates/waved-ui/src/kit/obj.rs` (layout helpers and their test)
- Create: `crates/waved-ui/src/kit/components/mod.rs`, `crates/waved-ui/src/kit/components/status.rs`
- Modify: `crates/waved-ui/src/kit/mod.rs` (`pub mod components;`)
- Create: `crates/waved-ui/golden/status_plain.png`, `status_pill.png`, `status_message_success.png`
- Test: `obj.rs`, `status.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Obj` with `one_line` (Task 12); `Dispatcher`, `Message`, `Severity`, `Route` and `UiSnapshot` (Task 13).
- Produces:
  - **`kit::obj::SIZE_CONTENT: i32`** and **`kit::obj::text_width(text, Font) -> i32`** (the width of one line of text).
  - **New `Obj` methods:** `fit`, `row(gap)`, `row_start(gap, pad_x)`, `column(gap)`, `column_centered(gap)`, `grow`, `center`, `pressed_border(width, hex)`, `wrapped_text(width)` and `max_width(width)`.
  - **`StatusArea`:** `StatusArea::plain(parent) -> StatusArea` and `StatusArea::pill(parent, &Dispatcher) -> StatusArea`, with methods `update(&mut self, &UiSnapshot)`, `show(&mut self, &Message, now_ms: u64)`, `cluster() -> Obj` and `message() -> Obj`.
  - **`battery_look`:** `battery_look(level: u8, charging: bool) -> (&'static str, u32)`.

- [ ] **Step 1: Write the failing layout test** in `obj.rs` `mod tests`:

```rust
    #[test]
    fn rows_centre_their_children() {
        lvgl(|lv| {
            let root = lv.screen();
            let row = Obj::plain(root).rect(Rect::new(100, 100, 200, 40)).row(10);
            let first = Obj::plain(row).size(20, 20);
            let second = Obj::plain(row).size(20, 20);
            row.update_layout();
            assert_eq!((first.coords().x1, second.coords().x1), (175, 205));
            assert_eq!(first.coords().y1, 110);
        });
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::obj::tests::rows 2>&1 | tail -3`
Expected: compile error `no method named \`row\``

- [ ] **Step 3: Add the helpers** to `obj.rs`, below `const MAIN`:

```rust
/// LVGL's "size to content" (LV_COORD_SET_SPEC(LV_COORD_MAX) in lv_area.h).
pub const SIZE_CONTENT: i32 = ((1 << 29) - 1) | (1 << 29);

/// Width of `text` on one line in `font`, in pixels.
pub fn text_width(text: &str, font: Font) -> i32 {
    let text = CString::new(text.replace('\0', "")).expect("NUL bytes removed");
    // SAFETY: NUL-terminated text and a static font.
    unsafe { lv_text_get_width(text.as_ptr(), text.as_bytes().len() as u32, font.ptr(), 0) }
}
```

and inside `impl Obj`:

```rust
    /// Size to the content (text or children).
    pub fn fit(self) -> Self {
        self.size(SIZE_CONTENT, SIZE_CONTENT)
    }

    /// Children in a row, centred both ways, `gap` px apart.
    pub fn row(self, gap: i32) -> Self {
        self.flex(lv_flex_flow_t_LV_FLEX_FLOW_ROW, lv_flex_align_t_LV_FLEX_ALIGN_CENTER, lv_flex_align_t_LV_FLEX_ALIGN_CENTER, gap)
    }

    /// Children in a row from the left, vertically centred, with side padding.
    pub fn row_start(self, gap: i32, pad_x: i32) -> Self {
        let row = self.flex(lv_flex_flow_t_LV_FLEX_FLOW_ROW, lv_flex_align_t_LV_FLEX_ALIGN_START, lv_flex_align_t_LV_FLEX_ALIGN_CENTER, gap);
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_pad_left(self.0, pad_x, MAIN);
            lv_obj_set_style_pad_right(self.0, pad_x, MAIN);
        }
        row
    }

    /// Children in a column, left-aligned, `gap` px apart.
    pub fn column(self, gap: i32) -> Self {
        self.flex(lv_flex_flow_t_LV_FLEX_FLOW_COLUMN, lv_flex_align_t_LV_FLEX_ALIGN_START, lv_flex_align_t_LV_FLEX_ALIGN_START, gap)
    }

    /// Children in a column, centred across, `gap` px apart.
    pub fn column_centered(self, gap: i32) -> Self {
        self.flex(lv_flex_flow_t_LV_FLEX_FLOW_COLUMN, lv_flex_align_t_LV_FLEX_ALIGN_START, lv_flex_align_t_LV_FLEX_ALIGN_CENTER, gap)
    }

    fn flex(self, flow: lv_flex_flow_t, main: lv_flex_align_t, cross: lv_flex_align_t, gap: i32) -> Self {
        // SAFETY: live object.
        unsafe {
            lv_obj_set_flex_flow(self.0, flow);
            lv_obj_set_flex_align(self.0, main, cross, cross);
            lv_obj_set_style_pad_row(self.0, gap, MAIN);
            lv_obj_set_style_pad_column(self.0, gap, MAIN);
        }
        self
    }

    /// Take the remaining space in a flex row.
    pub fn grow(self) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_flex_grow(self.0, 1) };
        self
    }

    /// Centre in the parent.
    pub fn center(self) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_center(self.0) };
        self
    }

    /// Border while pressed (for example the teal ring on side buttons).
    pub fn pressed_border(self, width: i32, hex: u32) -> Self {
        let pressed = MAIN | LV_STATE_PRESSED as lv_style_selector_t;
        // SAFETY: live object.
        unsafe {
            lv_obj_set_style_border_width(self.0, width, pressed);
            lv_obj_set_style_border_color(self.0, color(hex), pressed);
            lv_obj_set_style_border_opa(self.0, LV_OPA_COVER as lv_opa_t, pressed);
        }
        self
    }

    /// Centred text wrapping within `width`.
    pub fn wrapped_text(self, width: i32) -> Self {
        // SAFETY: live label.
        unsafe {
            lv_obj_set_width(self.0, width);
            lv_obj_set_style_text_align(self.0, lv_text_align_t_LV_TEXT_ALIGN_CENTER, MAIN);
            lv_label_set_long_mode(self.0, lv_label_long_mode_t_LV_LABEL_LONG_WRAP);
        }
        self
    }

    /// Let a content-sized label grow up to `width`. Together with `one_line`
    /// it then ends in "..." (values next to a title in a row).
    pub fn max_width(self, width: i32) -> Self {
        // SAFETY: live object.
        unsafe { lv_obj_set_style_max_width(self.0, width, MAIN) };
        self
    }
```

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::obj 2>&1 | grep "test result"`
Expected: `ok. 4 passed`

- [ ] **Step 4: Write the failing `StatusArea` tests**

`components/mod.rs`:

```rust
//! Reusable pieces from the component table in `docs/mockups/README.md`.

pub mod status;
```

Add `pub mod components;` to `kit/mod.rs`. Create `components/status.rs`:

```rust
//! The status slot at the top: a plain cluster on sub-screens, a tappable pill
//! on Home and Recording, and one-line messages that replace it for a few
//! seconds without covering a button.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};

    fn snap() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;
        snap
    }

    #[test]
    fn plain_status_shows_connections_time_and_battery() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            status.update(&snap());
            assert!(!status.cluster().is_clickable());
            assert_golden("status_plain", &lv.render());
        });
    }

    #[test]
    fn the_pill_opens_the_menu() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, ui) = Dispatcher::test();
            let mut status = StatusArea::pill(root, &dispatch);
            status.update(&snap());
            assert_golden("status_pill", &lv.render());
            assert_targets_ok(root);
            lv.click(status.cluster());
            assert_eq!(ui.try_recv().unwrap(), UiEvent::Navigate(Route::Menu));
        });
    }

    #[test]
    fn messages_replace_the_status_and_expire() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            let mut now = snap();
            now.now_ms = 1_000;
            status.update(&now);
            status.show(&Message::new(Severity::Success, "Recording sent"), 1_000);
            assert!(status.cluster().is_hidden());
            assert!(!status.message().is_clickable(), "messages never take taps");
            assert_golden("status_message_success", &lv.render());
            now.now_ms = 2_999;
            status.update(&now);
            assert!(status.cluster().is_hidden());
            now.now_ms = 3_000;
            status.update(&now);
            assert!(!status.cluster().is_hidden() && status.message().is_hidden());
        });
    }

    #[test]
    fn long_messages_stay_within_230_px() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut status = StatusArea::plain(root);
            status.show(
                &Message::new(Severity::Warning, "No Wi-Fi, so the upload of the 42-minute recording is waiting"),
                0,
            );
            status.message().update_layout();
            let a = status.message().coords();
            assert!(a.x2 - a.x1 + 1 <= MESSAGE_MAX_W);
            let text = status.message().child(1).coords();
            assert_eq!(text.y2 - text.y1 + 1, Font::Small.line_height(), "one line, not wrapped");
        });
    }

    #[test]
    fn battery_colours_follow_the_level() {
        assert_eq!(battery_look(82, false), (icons::BATTERY_3, TEXT_2));
        assert_eq!(battery_look(18, false).1, WARN);
        assert_eq!(battery_look(8, false), (icons::BATTERY_0, REC));
        assert_eq!(battery_look(8, true), (icons::BATTERY_CHARGING, LIVE));
    }
}
```

- [ ] **Step 5: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::components::status 2>&1 | tail -3`
Expected: compile error `cannot find type \`StatusArea\``

- [ ] **Step 6: Implement** above the tests in `status.rs`:

```rust
use crate::kit::dispatch::{Dispatcher, Message, Severity};
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::route::Route;
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    LIVE, MESSAGE_MAX_W, RAISED, REC, SCREEN_W, STATUS, STATUS_PILL, SURFACE, TEXT, TEXT_2, TEXT_3, WARN,
};

#[derive(Clone, Debug, PartialEq)]
struct Shown {
    usb: bool,
    wifi: bool,
    bt: bool,
    clock: String,
    battery: u8,
    charging: bool,
}

pub struct StatusArea {
    cluster: Obj,
    usb: Obj,
    wifi: Obj,
    bt: Obj,
    clock: Obj,
    battery_icon: Obj,
    battery_text: Obj,
    message: Obj,
    message_icon: Obj,
    message_text: Obj,
    message_until_ms: Option<u64>,
    shown: Option<Shown>,
}

impl StatusArea {
    /// The status cluster on sub-screens. Not clickable.
    pub fn plain(parent: Obj) -> Self {
        Self::build(parent, None)
    }

    /// The status pill on Home and Recording. Tapping it opens the menu.
    pub fn pill(parent: Obj, dispatch: &Dispatcher) -> Self {
        Self::build(parent, Some(dispatch.clone()))
    }

    fn build(parent: Obj, dispatch: Option<Dispatcher>) -> Self {
        let is_pill = dispatch.is_some();
        let cluster = Obj::plain(parent)
            .rect(if is_pill { STATUS_PILL } else { STATUS })
            .row(7);
        if let Some(dispatch) = dispatch {
            cluster.bg(SURFACE).radius(14).clickable(true).ext_click(8);
            on_click(cluster, move || dispatch.navigate(Route::Menu));
        }
        let icon = |glyph: &str| Obj::label(cluster, glyph, Font::Icon16, TEXT_2);
        let usb = icon(icons::USB);
        let wifi = icon(icons::WIFI);
        let bt = icon(icons::BLUETOOTH);
        let clock = Obj::label(cluster, "", Font::Num13, TEXT_2);
        let battery = Obj::plain(cluster).fit().row(3);
        let battery_icon = Obj::label(battery, icons::BATTERY_4, Font::Icon16, TEXT_2);
        let battery_text = Obj::label(battery, "", Font::Num13, TEXT_2);
        if is_pill {
            Obj::label(cluster, icons::DOWN, Font::Icon16, TEXT_3);
        }
        let message = Obj::plain(parent)
            .size(MESSAGE_MAX_W, 28)
            .pos((SCREEN_W - MESSAGE_MAX_W) / 2, 7)
            .bg(RAISED)
            .radius(14)
            .row(6)
            .hidden(true);
        let message_icon = Obj::label(message, icons::INFO, Font::Icon16, TEXT_2);
        let message_text = Obj::label(message, "", Font::Small, TEXT);
        Self {
            cluster,
            usb,
            wifi,
            bt,
            clock,
            battery_icon,
            battery_text,
            message,
            message_icon,
            message_text,
            message_until_ms: None,
            shown: None,
        }
    }

    /// Expire messages and refresh icons, clock and battery when they change.
    pub fn update(&mut self, snap: &UiSnapshot) {
        if self.message_until_ms.is_some_and(|until| snap.now_ms >= until) {
            self.message_until_ms = None;
            self.message.hidden(true);
            self.cluster.hidden(false);
        }
        let next = Shown {
            usb: snap.mcu.usb_connected,
            wifi: snap.wifi.connected,
            bt: snap.bt.connected,
            clock: snap.clock.clone(),
            battery: snap.battery.level,
            charging: snap.battery.is_charging,
        };
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        self.usb.hidden(!next.usb);
        self.wifi.hidden(!next.wifi);
        self.bt.hidden(!next.bt);
        self.clock.set_text(&next.clock);
        let (glyph, color) = battery_look(next.battery, next.charging);
        self.battery_icon.text_color(color).set_text(glyph);
        self.battery_text.text_color(color).set_text(&format!("{}%", next.battery));
        self.shown = Some(next);
    }

    /// Replace the status with `message` until its duration has passed.
    pub fn show(&mut self, message: &Message, now_ms: u64) {
        let (glyph, color) = match message.severity {
            Severity::Success => (icons::CHECK_CIRCLE, LIVE),
            Severity::Info => (icons::INFO, TEXT_2),
            Severity::Warning => (icons::ALERT, WARN),
            Severity::Error => (icons::ALERT, REC),
        };
        self.message_icon.text_color(color).set_text(glyph);
        // One spare pixel, so text that fits exactly is never cut to "...".
        let text_w = (text_width(&message.text, Font::Small) + 1).min(MESSAGE_MAX_W - 44);
        self.message_text.width(text_w).one_line().set_text(&message.text);
        let width = (9 + 16 + 6 + text_w + 13).min(MESSAGE_MAX_W);
        self.message
            .size(width, 28)
            .pos((SCREEN_W - width) / 2, 7)
            .hidden(false);
        self.cluster.hidden(true);
        self.message_until_ms = Some(now_ms + message.severity.duration_ms());
    }

    pub fn cluster(&self) -> Obj {
        self.cluster
    }

    pub fn message(&self) -> Obj {
        self.message
    }
}

/// Battery glyph and colour: teal while charging, amber at or below 20 %,
/// red at or below 10 %.
pub fn battery_look(level: u8, charging: bool) -> (&'static str, u32) {
    if charging {
        return (icons::BATTERY_CHARGING, LIVE);
    }
    let glyph = match level {
        0..=12 => icons::BATTERY_0,
        13..=37 => icons::BATTERY_1,
        38..=62 => icons::BATTERY_2,
        63..=87 => icons::BATTERY_3,
        _ => icons::BATTERY_4,
    };
    let color = match level {
        0..=10 => REC,
        11..=20 => WARN,
        _ => TEXT_2,
    };
    (glyph, color)
}
```

- [ ] **Step 7: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::status 2>&1 | grep "test result"`
Expected: `ok. 5 passed`

Open `target/ui-preview/status_plain.png`, `status_pill.png` and `status_message_success.png`. Compare them with the status areas in `../docs/mockups/png/menu.png`, `home.png` and `home-muted.png`. Check: icon order (USB, Wi-Fi, time, battery, then the chevron on the pill); the 28 px pill height; and the message with a teal check, centred. Fix any differences and regenerate.

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`
Expected: `ok` with no failures

- [ ] **Step 8: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit crates/waved-ui/golden
git commit -m "feat(ui): status cluster, status pill and status-slot messages

- pill opens the menu; messages replace the status and never take taps
- battery turns amber at 20 percent and red at 10 percent
- flex layout helpers on Obj

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: Buttons — Back, text, primary and side

**Files:**
- Create: `crates/waved-ui/src/kit/components/buttons.rs`
- Modify: `crates/waved-ui/src/kit/components/mod.rs` (`pub mod buttons;`)
- Create: `crates/waved-ui/golden/back_button.png`, `primary_record.png`, `primary_record_no_subtitle.png`, `side_buttons.png`
- Test: `buttons.rs` (`mod tests`)

**Interfaces:**
- Produces:
  - `Tone { Neutral, Live, Rec, RecText, WarnText }` with `colors() -> (fill, fg)` and `pressed() -> fill`.
  - `shade(hex) -> u32`, which gives 85 % brightness.
  - `back_button(parent, &Dispatcher) -> Obj`.
  - `text_button(parent, Rect, &str, impl FnMut() + 'static) -> Obj`.
  - `PrimaryButton::new(parent, Rect, Tone, icon, title, Option<&str>, impl FnMut() + 'static)` with `set_subtitle(Option<&str>)` and `obj()`.
  - `Side { Left, Right }` and `REPEAT_MS: u32 = 300`.
  - `SideButton::new(parent, Side, icon, label, impl FnMut() + 'static)` with `set_icon`, `set_label` and `obj()`.

- [ ] **Step 1: Write the failing tests**

```rust
//! Buttons: the one Back button, text buttons, the primary action and the
//! side buttons (Quieter and Louder).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, lv_event_code_t_LV_EVENT_PRESSED};
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::kit::tokens::PRIMARY;

    #[test]
    fn back_goes_back() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, ui) = Dispatcher::test();
            let back = back_button(root, &dispatch);
            assert_golden("back_button", &lv.render());
            assert_targets_ok(root);
            lv.click(back);
            assert_eq!(ui.try_recv().unwrap(), UiEvent::Back);
        });
    }

    #[test]
    fn primary_button_carries_the_pre_recording_line() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let counter = taps.clone();
            let record = PrimaryButton::new(
                root,
                PRIMARY,
                Tone::Rec,
                icons::RECORD,
                "Record",
                Some("includes last 30 s"),
                move || counter.set(counter.get() + 1),
            );
            assert_golden("primary_record", &lv.render());
            assert_targets_ok(root);
            lv.click(record.obj());
            assert_eq!(taps.get(), 1);
            record.set_subtitle(None);
            assert_golden("primary_record_no_subtitle", &lv.render());
        });
    }

    #[test]
    fn side_buttons_step_on_press_and_every_300_ms_while_held() {
        lvgl(|lv| {
            let root = lv.screen();
            let steps = Rc::new(Cell::new(0));
            let counter = steps.clone();
            SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", || {});
            let louder = SideButton::new(root, Side::Right, icons::VOLUME_HIGH, "Louder", move || {
                counter.set(counter.get() + 1)
            });
            assert_golden("side_buttons", &lv.render());
            assert_targets_ok(root);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_PRESSED);
            assert_eq!(steps.get(), 1);
            lv.advance(100);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            assert_eq!(steps.get(), 1, "repeats wait 300 ms");
            lv.advance(250);
            lv.send(louder.obj(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            assert_eq!(steps.get(), 2);
        });
    }

    #[test]
    fn shade_darkens_each_channel() {
        assert_eq!(shade(0xff4b3e), 0xd83f34);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::components::buttons 2>&1 | tail -3`
Expected: compile errors (`back_button`, `PrimaryButton`, `SideButton` not found)

- [ ] **Step 3: Implement** above the tests:

```rust
use crate::bindings::{
    lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, lv_event_code_t_LV_EVENT_PRESSED, lv_tick_elaps, lv_tick_get,
};
use crate::kit::dispatch::Dispatcher;
use crate::kit::event::{on, on_click};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::tokens::{
    Rect, BACK, LIVE, ON_LIVE, RAISED, REC, SIDE_LABEL_Y, SIDE_LEFT, SIDE_RIGHT, SURFACE, TEXT, TEXT_2, WARN, WHITE,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Colour roles for filled buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Live,
    Rec,
    /// Neutral fill, red text: Delete.
    RecText,
    /// Neutral fill, amber text: Resume.
    WarnText,
}

impl Tone {
    /// (fill, text and icon colour)
    pub fn colors(self) -> (u32, u32) {
        match self {
            Tone::Neutral => (SURFACE, TEXT),
            Tone::Live => (LIVE, ON_LIVE),
            Tone::Rec => (REC, WHITE),
            Tone::RecText => (SURFACE, REC),
            Tone::WarnText => (SURFACE, WARN),
        }
    }

    /// The fill while pressed.
    pub fn pressed(self) -> u32 {
        match self {
            Tone::Live | Tone::Rec => shade(self.colors().0),
            Tone::Neutral | Tone::RecText | Tone::WarnText => RAISED,
        }
    }
}

/// The pressed shade of a fill: 85 % brightness per channel.
pub fn shade(hex: u32) -> u32 {
    let channel = |shift: u32| (((hex >> shift) & 0xff) * 85 / 100) << shift;
    channel(16) | channel(8) | channel(0)
}

fn filled(parent: Obj, rect: Rect, tone: Tone) -> Obj {
    Obj::plain(parent)
        .rect(rect)
        .bg(tone.colors().0)
        .radius(rect.h / 2)
        .pressed_bg(tone.pressed())
        .clickable(true)
}

/// The one Back button: 112×44 at the bottom centre.
pub fn back_button(parent: Obj, dispatch: &Dispatcher) -> Obj {
    let button = filled(parent, BACK, Tone::Neutral).row(2);
    Obj::label(button, icons::LEFT, Font::Icon16, TEXT);
    Obj::label(button, "Back", Font::Body, TEXT);
    let dispatch = dispatch.clone();
    on_click(button, move || dispatch.back());
    button
}

/// A text button in the Back style: "Cancel", "Stop USB storage".
pub fn text_button(parent: Obj, rect: Rect, text: &str, on_tap: impl FnMut() + 'static) -> Obj {
    let button = filled(parent, rect, Tone::Neutral).row(0);
    Obj::label(button, text, Font::Body, TEXT);
    on_click(button, on_tap);
    button
}

pub struct PrimaryButton {
    obj: Obj,
    subtitle: Obj,
}

impl PrimaryButton {
    /// The main action: icon, title and an optional second line.
    pub fn new(
        parent: Obj,
        rect: Rect,
        tone: Tone,
        icon: &str,
        title: &str,
        subtitle: Option<&str>,
        on_tap: impl FnMut() + 'static,
    ) -> Self {
        let fg = tone.colors().1;
        let obj = filled(parent, rect, tone).row(10);
        Obj::label(obj, icon, Font::Icon20, fg);
        let stack = Obj::plain(obj).fit().column(2);
        Obj::label(stack, title, Font::Title, fg);
        let subtitle_label = Obj::label(stack, subtitle.unwrap_or(""), Font::Micro, fg)
            .opa(217)
            .hidden(subtitle.is_none());
        on_click(obj, on_tap);
        Self { obj, subtitle: subtitle_label }
    }

    pub fn set_subtitle(&self, subtitle: Option<&str>) {
        self.subtitle.hidden(subtitle.is_none()).set_text(subtitle.unwrap_or(""));
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Repeat interval while a side button is held.
pub const REPEAT_MS: u32 = 300;

pub struct SideButton {
    obj: Obj,
    icon: Obj,
    label: Obj,
}

impl SideButton {
    /// A 60 px circle at the left or right edge with a 12 px label below.
    /// `on_step` runs on press and every 300 ms while held.
    pub fn new(parent: Obj, side: Side, icon: &str, label: &str, on_step: impl FnMut() + 'static) -> Self {
        let rect = match side {
            Side::Left => SIDE_LEFT,
            Side::Right => SIDE_RIGHT,
        };
        let obj = Obj::plain(parent)
            .rect(rect)
            .bg(SURFACE)
            .round()
            .pressed_bg(RAISED)
            .pressed_border(2, LIVE)
            .clickable(true);
        let icon_label = Obj::label(obj, icon, Font::Icon26, TEXT).center();
        let label_obj = Obj::label(parent, label, Font::Micro, TEXT_2)
            .centered_text(90)
            .pos(rect.x + rect.w / 2 - 45, SIDE_LABEL_Y);
        let step = Rc::new(RefCell::new(on_step));
        let last = Rc::new(Cell::new(0u32));
        {
            let (step, last) = (step.clone(), last.clone());
            on(obj, lv_event_code_t_LV_EVENT_PRESSED, move |_| {
                // SAFETY: LVGL tick read on the UI thread.
                last.set(unsafe { lv_tick_get() });
                (step.borrow_mut())();
            });
        }
        on(obj, lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, move |_| {
            // SAFETY: LVGL tick reads on the UI thread.
            if unsafe { lv_tick_elaps(last.get()) } >= REPEAT_MS {
                last.set(unsafe { lv_tick_get() });
                (step.borrow_mut())();
            }
        });
        Self { obj, icon: icon_label, label: label_obj }
    }

    pub fn set_icon(&self, icon: &str) {
        self.icon.set_text(icon);
    }

    pub fn set_label(&self, text: &str) {
        self.label.set_text(text);
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}
```

- [ ] **Step 4: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::buttons 2>&1 | grep "test result"`
Expected: `ok. 4 passed`

Compare `target/ui-preview/back_button.png` with the Back pill in `../docs/mockups/png/noise.png`, `primary_record.png` with the Record button in `home.png`, and `side_buttons.png` with Quieter/Louder in `home.png`. Fix any differences and regenerate.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit crates/waved-ui/golden
git commit -m "feat(ui): back, text, primary and side buttons

- one Back button for every screen; primary button with a second line
- side buttons step on press and every 300 ms while held

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: Button rows and the confirmation dialog

**Files:**
- Create: `crates/waved-ui/src/kit/components/row.rs`
- Modify: `crates/waved-ui/src/kit/components/mod.rs` (`pub mod row;`)
- Create: `crates/waved-ui/golden/row_recording.png`, `row_paused.png`, `row_call.png`, `confirm_power_off.png`
- Test: `row.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Tone` with `colors()` and `pressed()` (Task 16).
- Produces:
  - `ButtonSpec { icon: Option<&'static str>, text: String, tone: Tone }` with `ButtonSpec::new(icon, text, tone)`.
  - `ButtonRow::two(parent, [ButtonSpec; 2], impl FnMut(usize) + 'static)` and `ButtonRow::three(parent, [ButtonSpec; 3], impl FnMut(usize) + 'static)`, with `set(index, &ButtonSpec)`, `set_enabled(index, bool)` and `obj(index) -> Obj`.
  - `ConfirmSpec { icon, title, body, confirm, tone }`.
  - `confirm_dialog(root, &ConfirmSpec, on_confirm, on_cancel) -> ButtonRow` (index 0 = Cancel, 1 = confirm).
  - `tint(hex, alpha: u32) -> u32`.

- [ ] **Step 1: Write the failing tests**

```rust
//! Rows of two or three equal buttons, and the shared confirmation dialog.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::icons;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use std::cell::Cell;

    #[test]
    fn the_recording_row_reports_taps_and_can_disable_a_button() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(RefCell::new(Vec::new()));
            let log = taps.clone();
            let row = ButtonRow::three(
                root,
                [
                    ButtonSpec::new(Some(icons::MARK), "Mark", Tone::Neutral),
                    ButtonSpec::new(Some(icons::PAUSE), "Pause", Tone::Neutral),
                    ButtonSpec::new(Some(icons::STOP), "Stop", Tone::Rec),
                ],
                move |index| log.borrow_mut().push(index),
            );
            assert_golden("row_recording", &lv.render());
            assert_targets_ok(root);
            lv.click(row.obj(2));
            lv.click(row.obj(0));
            assert_eq!(*taps.borrow(), vec![2, 0]);

            row.set_enabled(0, false);
            row.set(1, &ButtonSpec::new(Some(icons::PLAY), "Resume", Tone::WarnText));
            assert!(!row.obj(0).is_clickable());
            assert_golden("row_paused", &lv.render());
        });
    }

    #[test]
    fn the_call_row_has_decline_and_answer() {
        lvgl(|lv| {
            let root = lv.screen();
            ButtonRow::two(
                root,
                [
                    ButtonSpec::new(Some(icons::PHONE_OFF), "Decline", Tone::Rec),
                    ButtonSpec::new(Some(icons::PHONE), "Answer", Tone::Live),
                ],
                |_| {},
            );
            assert_golden("row_call", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_confirmation_names_the_action() {
        lvgl(|lv| {
            let root = lv.screen();
            let confirmed = Rc::new(Cell::new(false));
            let flag = confirmed.clone();
            let row = confirm_dialog(
                root,
                &ConfirmSpec {
                    icon: icons::POWER,
                    title: "Power off?".into(),
                    body: "A recording in progress is saved first. Turn Wave on again with the power button.".into(),
                    confirm: "Power off".into(),
                    tone: Tone::Rec,
                },
                move || flag.set(true),
                || {},
            );
            assert_golden("confirm_power_off", &lv.render());
            assert_targets_ok(root);
            lv.click(row.obj(1));
            assert!(confirmed.get());
        });
    }

    #[test]
    fn tint_blends_over_black() {
        assert_eq!(tint(0xff4b3e, 36), 0x240a08);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::components::row 2>&1 | tail -3`
Expected: compile errors (`ButtonRow`, `confirm_dialog` missing)

- [ ] **Step 3: Implement** above the tests:

```rust
use crate::kit::components::buttons::Tone;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::tokens::{Rect, LIVE, REC, ROW2, ROW3, SCREEN_W, TEXT, TEXT_2};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub struct ButtonSpec {
    pub icon: Option<&'static str>,
    pub text: String,
    pub tone: Tone,
}

impl ButtonSpec {
    pub fn new(icon: Option<&'static str>, text: impl Into<String>, tone: Tone) -> Self {
        Self { icon, text: text.into(), tone }
    }
}

struct RowButton {
    obj: Obj,
    icon: Obj,
    text: Obj,
}

impl RowButton {
    fn apply(&self, spec: &ButtonSpec) {
        let (fill, fg) = spec.tone.colors();
        self.obj.bg(fill).pressed_bg(spec.tone.pressed());
        self.icon
            .hidden(spec.icon.is_none())
            .text_color(fg)
            .set_text(spec.icon.unwrap_or(""));
        self.text.text_color(fg).set_text(&spec.text);
    }
}

pub struct ButtonRow {
    buttons: Vec<RowButton>,
}

impl ButtonRow {
    pub fn two(parent: Obj, specs: [ButtonSpec; 2], on_tap: impl FnMut(usize) + 'static) -> Self {
        Self::build(parent, &ROW2, &specs, on_tap)
    }

    pub fn three(parent: Obj, specs: [ButtonSpec; 3], on_tap: impl FnMut(usize) + 'static) -> Self {
        Self::build(parent, &ROW3, &specs, on_tap)
    }

    fn build(parent: Obj, rects: &[Rect], specs: &[ButtonSpec], on_tap: impl FnMut(usize) + 'static) -> Self {
        let handler = Rc::new(RefCell::new(on_tap));
        let buttons = rects
            .iter()
            .zip(specs)
            .enumerate()
            .map(|(index, (rect, spec))| {
                let obj = Obj::plain(parent).rect(*rect).radius(rect.h / 2).clickable(true).row(6);
                let icon = Obj::label(obj, "", Font::Icon20, TEXT);
                let text = Obj::label(obj, "", Font::Body, TEXT);
                let handler = handler.clone();
                on_click(obj, move || (handler.borrow_mut())(index));
                let button = RowButton { obj, icon, text };
                button.apply(spec);
                button
            })
            .collect();
        Self { buttons }
    }

    pub fn set(&self, index: usize, spec: &ButtonSpec) {
        self.buttons[index].apply(spec);
    }

    /// Disabled buttons fade to 35 % and ignore taps.
    pub fn set_enabled(&self, index: usize, enabled: bool) {
        self.buttons[index]
            .obj
            .clickable(enabled)
            .opa(if enabled { 255 } else { 89 });
    }

    pub fn obj(&self, index: usize) -> Obj {
        self.buttons[index].obj
    }
}

pub struct ConfirmSpec {
    pub icon: &'static str,
    pub title: String,
    pub body: String,
    pub confirm: String,
    pub tone: Tone,
}

/// A full-screen confirmation: icon, question, one line on the consequence,
/// then Cancel and the action (button 0 and button 1).
pub fn confirm_dialog(
    root: Obj,
    spec: &ConfirmSpec,
    on_confirm: impl FnMut() + 'static,
    on_cancel: impl FnMut() + 'static,
) -> ButtonRow {
    let accent = if spec.tone == Tone::Rec { REC } else { LIVE };
    let disc = Obj::plain(root).rect(Rect::new(172, 58, 56, 56)).round().bg(tint(accent, 36));
    Obj::label(disc, spec.icon, Font::Icon26, accent).center();
    Obj::label(root, &spec.title, Font::Dial, TEXT).centered_text(SCREEN_W).pos(0, 128);
    Obj::label(root, &spec.body, Font::Small, TEXT_2).wrapped_text(260).pos(70, 166);
    let (mut confirm, mut cancel) = (on_confirm, on_cancel);
    ButtonRow::two(
        root,
        [
            ButtonSpec::new(None, "Cancel", Tone::Neutral),
            ButtonSpec::new(None, spec.confirm.clone(), spec.tone),
        ],
        move |index| if index == 0 { cancel() } else { confirm() },
    )
}

/// `hex` at `alpha`/255 over black.
pub fn tint(hex: u32, alpha: u32) -> u32 {
    let channel = |shift: u32| (((hex >> shift) & 0xff) * alpha / 255) << shift;
    channel(16) | channel(8) | channel(0)
}
```

- [ ] **Step 4: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::row 2>&1 | grep "test result"`
Expected: `ok. 4 passed`

Compare against `../docs/mockups/png/recording.png` (row_recording), `paused.png` (row_paused), `call.png` (row_call) and `confirm.png` (confirm_power_off). Fix any differences and regenerate.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit crates/waved-ui/golden
git commit -m "feat(ui): button rows and the shared confirmation dialog

- two and three button rows with per-button tone and disabled state
- confirmation shows the question, the consequence, Cancel and the named action

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 18: The dial

**Files:**
- Create: `crates/waved-ui/src/kit/components/dial.rs`
- Modify: `crates/waved-ui/src/kit/components/mod.rs` (`pub mod dial;`)
- Create: goldens `dial_idle`, `dial_live_talker`, `dial_muted`, `dial_recording_talker`, `dial_paused`, `dial_gauge_60`, `dial_progress_64` (`.png`)
- Test: `dial.rs` (`mod tests`)

**Interfaces:**
- Consumes: `lvgl_angle`, `polar` (Task 11).
- Produces:
  - `RingStyle { Idle, Live, Muted, Recording, RecordingMuted, Paused }`.
  - `Dial::new(parent, cx, cy, r, width) -> Dial` (not tappable) and `.on_tap(self, impl FnMut() + 'static) -> Dial`.
  - Methods: `set_ring(&mut self, RingStyle)`, `set_talker(&self, Option<f32>)`, `set_gauge(&mut self, Option<u8>)`, `set_progress(&mut self, Option<f32>)`, `hit(&self) -> Option<Obj>` and `talker_visible(&self) -> bool`.

Ring styles follow the README dial table:

| Style | Ring |
|---|---|
| Idle | LINE 100 % |
| Live | LIVE 40 % (opacity 102) plus the teal talker wedge |
| Muted | REC 100 % |
| Recording | REC 35 % (opacity 89) plus the red talker wedge |
| RecordingMuted | REC 100 % |
| Paused | 30 amber dashes |

- The wedge is 44° wide and 7 px thick, with a dot 14 px outside the ring.
- The gauge spans −135°…135°, 2 px thicker than the ring, with a teal fill.
- Progress runs clockwise from the top.

- [ ] **Step 1: Write the failing tests**

```rust
//! The room dial: a ring for the table showing idle, live, muted, recording
//! and paused, a wedge toward whoever talks, and the volume gauge and progress
//! ring. Tapping it mutes on Home and Recording.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl, Lvgl};
    use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R};
    use std::cell::Cell;
    use std::rc::Rc;

    fn home_dial(lv: &mut Lvgl) -> (Obj, Dial) {
        let root = lv.screen();
        (root, Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3))
    }

    #[test]
    fn ring_styles_follow_the_dial_table() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            assert_golden("dial_idle", &lv.render());
            dial.set_ring(RingStyle::Live);
            dial.set_talker(Some(307.0));
            assert_golden("dial_live_talker", &lv.render());
            dial.set_ring(RingStyle::Muted);
            assert!(!dial.talker_visible(), "muted hides the talker wedge");
            assert_golden("dial_muted", &lv.render());
            dial.set_ring(RingStyle::Recording);
            dial.set_talker(Some(60.0));
            assert_golden("dial_recording_talker", &lv.render());
            dial.set_ring(RingStyle::Paused);
            assert_golden("dial_paused", &lv.render());
        });
    }

    #[test]
    fn gauge_and_progress_give_the_ring_back() {
        lvgl(|lv| {
            let (_root, mut dial) = home_dial(lv);
            dial.set_ring(RingStyle::Live);
            let ring = lv.render().to_png();
            dial.set_gauge(Some(60));
            assert_golden("dial_gauge_60", &lv.render());
            dial.set_gauge(None);
            assert!(lv.render().to_png() == ring, "the ring is back after the gauge");
            dial.set_progress(Some(0.64));
            assert_golden("dial_progress_64", &lv.render());
            dial.set_progress(None);
            assert!(lv.render().to_png() == ring, "the ring is back after progress");
        });
    }

    #[test]
    fn tapping_mutes_only_when_enabled() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let counter = taps.clone();
            let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(move || counter.set(counter.get() + 1));
            assert_targets_ok(root);
            lv.click(dial.hit().unwrap());
            assert_eq!(taps.get(), 1);
            let (_root, plain) = home_dial(lv);
            assert!(plain.hit().is_none());
        });
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::components::dial 2>&1 | tail -3`
Expected: compile error `cannot find type \`Dial\``

- [ ] **Step 3: Implement** above the tests:

```rust
use crate::bindings::*;
use crate::kit::event::on_click;
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{lvgl_angle, polar, LINE, LIVE, REC, WARN};

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;
const INDICATOR: lv_style_selector_t = LV_PART_INDICATOR as lv_style_selector_t;
const TALKER_WIDTH: i32 = 7;
const TALKER_SPAN: f32 = 44.0;
const DASHES: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingStyle {
    Idle,
    Live,
    Muted,
    Recording,
    RecordingMuted,
    Paused,
}

pub struct Dial {
    parent: Obj,
    cx: i32,
    cy: i32,
    r: i32,
    width: i32,
    hit: Option<Obj>,
    ring: Obj,
    talker: Obj,
    dot: Obj,
    dashes: Vec<Obj>,
    style: RingStyle,
}

impl Dial {
    /// A ring of radius `r` and stroke `width` centred on (cx, cy).
    pub fn new(parent: Obj, cx: i32, cy: i32, r: i32, width: i32) -> Self {
        let ring = arc(parent, cx, cy, r, width);
        let talker = arc(parent, cx, cy, r, TALKER_WIDTH).hidden(true);
        let dot = Obj::plain(parent).size(8, 8).round().hidden(true);
        let mut dial = Self {
            parent,
            cx,
            cy,
            r,
            width,
            hit: None,
            ring,
            talker,
            dot,
            dashes: Vec::new(),
            style: RingStyle::Idle,
        };
        dial.set_ring(RingStyle::Idle);
        dial
    }

    /// Make the whole disc one tap target.
    pub fn on_tap(mut self, mut tap: impl FnMut() + 'static) -> Self {
        let size = 2 * self.r + 8;
        let disc = Obj::plain(self.parent)
            .pos(self.cx - size / 2, self.cy - size / 2)
            .size(size, size)
            .round()
            .clickable(true);
        on_click(disc, move || tap());
        self.hit = Some(disc);
        self
    }

    pub fn set_ring(&mut self, style: RingStyle) {
        self.style = style;
        let (hex, opa) = match style {
            RingStyle::Idle => (LINE, 255),
            RingStyle::Live => (LIVE, 102),
            RingStyle::Muted | RingStyle::RecordingMuted => (REC, 255),
            RingStyle::Recording => (REC, 89),
            RingStyle::Paused => (WARN, 0),
        };
        full_ring(self.ring);
        arc_style(self.ring, MAIN, hex, opa, self.width);
        arc_style(self.ring, INDICATOR, hex, 0, self.width);
        self.show_dashes(style == RingStyle::Paused);
        if !matches!(style, RingStyle::Live | RingStyle::Recording) {
            self.set_talker(None);
        }
    }

    /// Light the wedge toward a talker (device degrees), or hide it.
    pub fn set_talker(&self, azimuth_deg: Option<f32>) {
        let hex = if self.style == RingStyle::Recording { REC } else { LIVE };
        match azimuth_deg {
            Some(az) if matches!(self.style, RingStyle::Live | RingStyle::Recording) => {
                // SAFETY: live arc.
                unsafe {
                    lv_arc_set_bg_angles(
                        self.talker.raw(),
                        lvgl_angle(az - TALKER_SPAN / 2.0),
                        lvgl_angle(az + TALKER_SPAN / 2.0),
                    );
                }
                arc_style(self.talker, MAIN, hex, 255, TALKER_WIDTH);
                arc_style(self.talker, INDICATOR, hex, 0, TALKER_WIDTH);
                let (x, y) = polar(self.cx, self.cy, (self.r + 14) as f32, az);
                self.dot.pos(x - 4, y - 4).bg(hex).hidden(false);
                self.talker.hidden(false);
            }
            _ => {
                self.talker.hidden(true);
                self.dot.hidden(true);
            }
        }
    }

    /// Show `percent` on a −135°…135° gauge in place of the ring, or restore it.
    pub fn set_gauge(&mut self, percent: Option<u8>) {
        let Some(value) = percent else {
            self.set_ring(self.style);
            return;
        };
        self.set_talker(None);
        self.show_dashes(false);
        // SAFETY: live arc.
        unsafe {
            lv_arc_set_rotation(self.ring.raw(), 0);
            lv_arc_set_bg_angles(self.ring.raw(), lvgl_angle(-135.0), lvgl_angle(135.0));
            lv_arc_set_range(self.ring.raw(), 0, 100);
            lv_arc_set_value(self.ring.raw(), i32::from(value.min(100)));
        }
        arc_style(self.ring, MAIN, LINE, 255, self.width + 2);
        arc_style(self.ring, INDICATOR, LIVE, 255, self.width + 2);
    }

    /// Show progress 0..1 clockwise from the top, or restore the ring.
    pub fn set_progress(&mut self, fraction: Option<f32>) {
        let Some(fraction) = fraction else {
            self.set_ring(self.style);
            return;
        };
        self.set_talker(None);
        self.show_dashes(false);
        full_ring(self.ring);
        // SAFETY: live arc.
        unsafe {
            lv_arc_set_value(self.ring.raw(), (fraction.clamp(0.0, 1.0) * 1000.0).round() as i32);
        }
        arc_style(self.ring, MAIN, LINE, 255, self.width);
        arc_style(self.ring, INDICATOR, LIVE, 255, self.width);
    }

    pub fn hit(&self) -> Option<Obj> {
        self.hit
    }

    pub fn talker_visible(&self) -> bool {
        !self.talker.is_hidden()
    }

    fn show_dashes(&mut self, on: bool) {
        if on && self.dashes.is_empty() {
            let pitch = 360.0 / DASHES as f32;
            self.dashes = (0..DASHES)
                .map(|i| {
                    let dash = arc(self.parent, self.cx, self.cy, self.r, self.width);
                    let start = i as f32 * pitch;
                    // SAFETY: live arc.
                    unsafe {
                        lv_arc_set_bg_angles(dash.raw(), lvgl_angle(start), lvgl_angle(start + pitch / 3.0));
                    }
                    arc_style(dash, MAIN, WARN, 255, self.width);
                    arc_style(dash, INDICATOR, WARN, 0, self.width);
                    dash
                })
                .collect();
        }
        for dash in &self.dashes {
            dash.hidden(!on);
        }
    }
}

/// A non-interactive arc whose stroke is centred on radius `r`, angles in
/// device-to-LVGL terms (rotation 0, full background, range 0..1000).
fn arc(parent: Obj, cx: i32, cy: i32, r: i32, width: i32) -> Obj {
    let size = 2 * r + width;
    let arc = Obj::arc(parent).pos(cx - size / 2, cy - size / 2).size(size, size);
    // SAFETY: live arc.
    unsafe {
        lv_arc_set_rotation(arc.raw(), 0);
        lv_arc_set_bg_angles(arc.raw(), 0, 360);
        lv_arc_set_range(arc.raw(), 0, 1000);
        lv_arc_set_value(arc.raw(), 0);
    }
    arc
}

/// Full circle with the indicator starting at the top.
fn full_ring(arc: Obj) {
    // SAFETY: live arc.
    unsafe {
        lv_arc_set_rotation(arc.raw(), 270);
        lv_arc_set_bg_angles(arc.raw(), 0, 360);
        lv_arc_set_range(arc.raw(), 0, 1000);
        lv_arc_set_value(arc.raw(), 0);
    }
}

fn arc_style(arc: Obj, part: lv_style_selector_t, hex: u32, opa: u8, width: i32) {
    // SAFETY: live arc.
    unsafe {
        lv_obj_set_style_arc_color(arc.raw(), color(hex), part);
        lv_obj_set_style_arc_opa(arc.raw(), opa, part);
        lv_obj_set_style_arc_width(arc.raw(), width, part);
        lv_obj_set_style_arc_rounded(arc.raw(), true, part);
    }
}
```

- [ ] **Step 4: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::dial 2>&1 | grep "test result"`
Expected: `ok. 3 passed`

Compare with the dials in `../docs/mockups/png/`:

| Golden | Mockup |
|---|---|
| `dial_live_talker` | `home.png` (wedge at the top-left) |
| `dial_muted` | `home-muted.png` |
| `dial_recording_talker` | `recording.png` |
| `dial_paused` | `paused.png` |
| `dial_gauge_60` | `home-volume.png` |
| `dial_progress_64` | `ota.png` (stroke 6 there, 3 here) |

The talker wedge is 44° wide, as `../docs/mockups/README.md` ("Dial states") specifies. `home.png` draws it at 38° and `recording.png` at 44°; the README wins. Fix any other differences and regenerate.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit crates/waved-ui/golden
git commit -m "feat(ui): room dial with talker wedge, gauge and progress modes

- ring styles from the dial table; paused draws amber dashes
- the wedge follows the talker in live and recording, hidden otherwise
- optional full-disc tap target for mute

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 19: Rotary list

**Files:**
- Create: `crates/waved-ui/src/kit/components/list.rs`
- Modify: `crates/waved-ui/src/kit/components/mod.rs` (`pub mod list;`)
- Create: goldens `list_menu.png`, `list_system.png`
- Test: `list.rs` (`mod tests`)

**Interfaces:**
- Produces:
  - `VIEW: Rect` (0, 34, 400, 218) and `ROW_H: i32 = 52`.
  - `ListItem { icon, title, value: Option<String>, danger: bool, locked: bool }` with `new`, `value`, `danger` and `locked`.
  - `Band { Focus, Near, Far }`.
  - `RotaryList::new(parent, Vec<ListItem>, impl FnMut(usize) + 'static)` with `scroll_to(index)`, `set_value(index, Option<&str>)`, `row(index) -> Obj` and `band(index) -> Band`.
- Behaviour:
  - Rows snap to the centre.
  - **Focus** (within 26 px of the centre): 320 wide, SURFACE fill, 20 px title.
  - **Near** (within 80 px): 296 wide, 15 px title.
  - **Far**: 220 wide, 13 px title at 50 % opacity.
  - Danger titles are red. Locked rows show a lock after the value.
  - Every row keeps a 52 px slot. Edge rows shrink their width and text, not their height, so snapping stays exact.
  - Titles and values end in "..." when they don't fit; values are capped per band so the title always keeps some room.
  - 18 px and 22 px fades cover the top and bottom edges.
  - An arc on the right (62°–118°, r 180) shows the scroll position.

- [ ] **Step 1: Write the failing tests**

```rust
//! The rotary list for round screens (LVGL's lv_example_scroll_6 pattern).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, lvgl};

    fn menu() -> Vec<ListItem> {
        vec![
            ListItem::new(icons::FOLDER, "Files").value("12").locked(true),
            ListItem::new(icons::PATTERN, "Audio mode").value("Follow talker"),
            ListItem::new(icons::NOISE, "Noise reduction").value("Medium"),
            ListItem::new(icons::VOLUME_HIGH, "Volume").value("60%"),
            ListItem::new(icons::HEIGHT, "Mic height").value("Medium"),
            ListItem::new(icons::WIFI, "Wi-Fi").value("Office-5G").locked(true),
            ListItem::new(icons::BLUETOOTH, "Bluetooth").value("iPhone").locked(true),
            ListItem::new(icons::SEND, "Uploads").value("Ask me").locked(true),
            ListItem::new(icons::SETTINGS, "System").locked(true),
        ]
    }

    #[test]
    fn the_middle_row_is_focused() {
        lvgl(|lv| {
            let root = lv.screen();
            let list = RotaryList::new(root, menu(), |_| {});
            list.scroll_to(2);
            assert_eq!(list.band(2), Band::Focus);
            assert_eq!(list.band(1), Band::Near);
            assert_eq!(list.band(3), Band::Near);
            assert_eq!(list.band(4), Band::Far);
            list.row(2).update_layout();
            let a = list.row(2).coords();
            assert_eq!(a.x2 - a.x1 + 1, 320);
            assert_golden("list_menu", &lv.render());
        });
    }

    #[test]
    fn taps_report_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let picked = Rc::new(Cell::new(usize::MAX));
            let chosen = picked.clone();
            let list = RotaryList::new(root, menu(), move |index| chosen.set(index));
            list.scroll_to(3);
            lv.click(list.row(3));
            assert_eq!(picked.get(), 3);
        });
    }

    #[test]
    fn visible_rows_stay_inside_the_tap_limit() {
        lvgl(|lv| {
            let root = lv.screen();
            let items = menu();
            let count = items.len();
            let list = RotaryList::new(root, items, |_| {});
            for focus in 0..count {
                list.scroll_to(focus);
                for index in 0..count {
                    let a = list.row(index).coords();
                    let centre = (a.y1 + a.y2) / 2;
                    if centre > VIEW.y && centre < VIEW.y + VIEW.h {
                        let rect = Rect::new(a.x1, a.y1, a.x2 - a.x1 + 1, a.y2 - a.y1 + 1);
                        assert!(rect.within_tap_limit(), "focus {focus}, row {index}: {rect:?}");
                    }
                }
            }
        });
    }

    #[test]
    fn long_titles_stay_inside_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let list = RotaryList::new(
                root,
                vec![
                    ListItem::new(icons::WIFI, "A Wi-Fi network with an unusually long name for a meeting room")
                        .value("Connected"),
                    ListItem::new(icons::WIFI, "Wi-Fi").value("Board room network with a long name").locked(true),
                    ListItem::new(icons::SEND, "Uploads to a server with a long name")
                        .value("recordings.example-company.internal")
                        .locked(true),
                ],
                |_| {},
            );
            // Each focus puts the rows in different bands (focus, near, far).
            for focus in 0..3 {
                list.scroll_to(focus);
                for index in 0..3 {
                    let row = list.row(index);
                    let bounds = row.coords();
                    for child in 0..row.child_count() {
                        let a = row.child(child as i32).coords();
                        assert!(
                            a.x1 >= bounds.x1 && a.x2 <= bounds.x2 && a.y1 >= bounds.y1 && a.y2 <= bounds.y2,
                            "focus {focus}, row {index}: child {child} overflows the row"
                        );
                    }
                }
            }
        });
    }

    #[test]
    fn the_end_of_the_system_list_shows_power_off_in_red() {
        lvgl(|lv| {
            let root = lv.screen();
            let list = RotaryList::new(
                root,
                vec![
                    ListItem::new(icons::CLOCK, "Pre-recording").value("On"),
                    ListItem::new(icons::INFO, "About").value("waved 0.9.3"),
                    ListItem::new(icons::REFRESH, "Firmware update").value("Up to date"),
                    ListItem::new(icons::TRASH, "Factory reset"),
                    ListItem::new(icons::POWER, "Power off").danger(),
                ],
                |_| {},
            );
            list.scroll_to(2);
            assert_golden("list_system", &lv.render());
        });
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::components::list 2>&1 | tail -3`
Expected: compile error `cannot find type \`RotaryList\``

- [ ] **Step 3: Implement** above the tests:

```rust
use crate::bindings::*;
use crate::kit::event::{on, on_click};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{lvgl_angle, Rect, BG, LINE, RAISED, REC, SURFACE, TEXT, TEXT_2, TEXT_3};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;
const INDICATOR: lv_style_selector_t = LV_PART_INDICATOR as lv_style_selector_t;

/// The list area, from under the status to just above Back.
pub const VIEW: Rect = Rect::new(0, 34, 400, 218);
/// Every row keeps a 52 px slot. Edge rows shrink their width and text, not
/// their height, so snapping and the scroll position stay exact.
pub const ROW_H: i32 = 52;

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub icon: &'static str,
    pub title: String,
    pub value: Option<String>,
    pub danger: bool,
    pub locked: bool,
}

impl ListItem {
    pub fn new(icon: &'static str, title: impl Into<String>) -> Self {
        Self { icon, title: title.into(), value: None, danger: false, locked: false }
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Focus,
    Near,
    Far,
}

/// How a row looks in each band ("Rotary list" in docs/mockups/README.md).
struct Look {
    width: i32,
    fill: bool,
    title_font: Font,
    title_color: u32,
    value_font: Font,
    value_color: u32,
    /// Leaves the title at least 30 px next to a capped value and a lock.
    value_max_w: i32,
    opa: u8,
}

impl Band {
    fn look(self) -> Look {
        match self {
            Band::Focus => Look {
                width: 320,
                fill: true,
                title_font: Font::Title,
                title_color: TEXT,
                value_font: Font::Body,
                value_color: TEXT_2,
                value_max_w: 128,
                opa: 255,
            },
            Band::Near => Look {
                width: 296,
                fill: false,
                title_font: Font::Body,
                title_color: TEXT_2,
                value_font: Font::Small,
                value_color: TEXT_3,
                value_max_w: 118,
                opa: 255,
            },
            Band::Far => Look {
                width: 220,
                fill: false,
                title_font: Font::Small,
                title_color: TEXT_2,
                value_font: Font::Small,
                value_color: TEXT_3,
                value_max_w: 88,
                opa: 128,
            },
        }
    }
}

struct Row {
    obj: Obj,
    title: Obj,
    value: Obj,
    danger: bool,
    /// The band last applied, so scrolling restyles only rows that cross a band edge.
    band: Cell<Option<Band>>,
}

struct Inner {
    viewport: Obj,
    rows: Vec<Row>,
    scrollbar: Obj,
}

pub struct RotaryList {
    inner: Rc<RefCell<Inner>>,
}

impl RotaryList {
    pub fn new(parent: Obj, items: Vec<ListItem>, on_select: impl FnMut(usize) + 'static) -> Self {
        let viewport = Obj::plain(parent).rect(VIEW).column_centered(0);
        let pad = (VIEW.h - ROW_H) / 2;
        // SAFETY: live object.
        unsafe {
            lv_obj_add_flag(viewport.raw(), lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_scroll_dir(viewport.raw(), lv_dir_t_LV_DIR_VER);
            lv_obj_set_scroll_snap_y(viewport.raw(), lv_scroll_snap_t_LV_SCROLL_SNAP_CENTER);
            lv_obj_set_scrollbar_mode(viewport.raw(), lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);
            lv_obj_set_style_pad_top(viewport.raw(), pad, MAIN);
            lv_obj_set_style_pad_bottom(viewport.raw(), pad, MAIN);
        }
        let select = Rc::new(RefCell::new(on_select));
        let rows = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let obj = Obj::plain(viewport)
                    .size(320, ROW_H)
                    .radius(24)
                    .pressed_bg(RAISED)
                    .clickable(true)
                    .row_start(12, 18);
                Obj::label(obj, item.icon, Font::Icon20, if item.danger { REC } else { TEXT_2 });
                let title = Obj::label(obj, &item.title, Font::Body, TEXT_2).grow().one_line();
                let tail = Obj::plain(obj).fit().row(6);
                let value = Obj::label(tail, item.value.as_deref().unwrap_or(""), Font::Small, TEXT_3);
                if item.locked {
                    Obj::label(tail, icons::LOCK, Font::Icon16, TEXT_3);
                }
                let select = select.clone();
                on_click(obj, move || (select.borrow_mut())(index));
                Row { obj, title, value, danger: item.danger, band: Cell::new(None) }
            })
            .collect();
        fade(parent, Rect::new(0, VIEW.y, VIEW.w, 18), true);
        fade(parent, Rect::new(0, VIEW.y + VIEW.h - 22, VIEW.w, 22), false);
        let scrollbar = scrollbar(parent);
        let inner = Rc::new(RefCell::new(Inner { viewport, rows, scrollbar }));
        {
            let inner = inner.clone();
            on(viewport, lv_event_code_t_LV_EVENT_SCROLL, move |_| inner.borrow().restyle());
        }
        inner.borrow().restyle();
        Self { inner }
    }

    /// Centre the row at `index` without animation.
    pub fn scroll_to(&self, index: usize) {
        let inner = self.inner.borrow();
        // SAFETY: live viewport.
        unsafe {
            lv_obj_scroll_to_y(inner.viewport.raw(), index as i32 * ROW_H, lv_anim_enable_t_LV_ANIM_OFF);
        }
        inner.restyle();
    }

    pub fn set_value(&self, index: usize, value: Option<&str>) {
        self.inner.borrow().rows[index].value.set_text(value.unwrap_or(""));
    }

    pub fn row(&self, index: usize) -> Obj {
        self.inner.borrow().rows[index].obj
    }

    pub fn band(&self, index: usize) -> Band {
        self.inner.borrow().band_of(index)
    }
}

impl Inner {
    fn band_of(&self, index: usize) -> Band {
        let a = self.rows[index].obj.coords();
        let distance = ((a.y1 + a.y2) / 2 - (VIEW.y + VIEW.h / 2)).abs();
        if distance < 26 {
            Band::Focus
        } else if distance < 80 {
            Band::Near
        } else {
            Band::Far
        }
    }

    fn restyle(&self) {
        self.viewport.update_layout();
        let mut changed = false;
        for (index, row) in self.rows.iter().enumerate() {
            let band = self.band_of(index);
            if row.band.replace(Some(band)) == Some(band) {
                continue;
            }
            changed = true;
            let look = band.look();
            row.obj.width(look.width).opa(look.opa);
            if look.fill {
                row.obj.bg(SURFACE);
            } else {
                row.obj.bg_opa(0);
            }
            row.title
                .font(look.title_font)
                .text_color(if row.danger { REC } else { look.title_color })
                .one_line();
            row.value
                .font(look.value_font)
                .text_color(look.value_color)
                .max_width(look.value_max_w)
                .one_line();
        }
        if changed {
            self.viewport.update_layout();
        }
        self.update_scrollbar();
    }

    fn update_scrollbar(&self) {
        let rows = self.rows.len().max(1) as f32;
        let thumb = (56.0 * (4.2 / rows).min(1.0)).max(10.0);
        let max_scroll = ((rows - 1.0) * ROW_H as f32).max(1.0);
        // SAFETY: live viewport.
        let scrolled = unsafe { lv_obj_get_scroll_y(self.viewport.raw()) } as f32;
        let start = 62.0 + (56.0 - thumb) * (scrolled / max_scroll).clamp(0.0, 1.0);
        // SAFETY: live arc.
        unsafe { lv_arc_set_angles(self.scrollbar.raw(), lvgl_angle(start), lvgl_angle(start + thumb)) };
    }
}

fn scrollbar(parent: Obj) -> Obj {
    let (r, width) = (180, 4);
    let size = 2 * r + width;
    let arc = Obj::arc(parent).pos(200 - size / 2, 152 - size / 2).size(size, size);
    // SAFETY: live arc.
    unsafe {
        lv_arc_set_rotation(arc.raw(), 0);
        lv_arc_set_bg_angles(arc.raw(), lvgl_angle(62.0), lvgl_angle(118.0));
        for (part, hex) in [(MAIN, LINE), (INDICATOR, TEXT_2)] {
            lv_obj_set_style_arc_color(arc.raw(), color(hex), part);
            lv_obj_set_style_arc_width(arc.raw(), width, part);
            lv_obj_set_style_arc_rounded(arc.raw(), true, part);
            lv_obj_set_style_arc_opa(arc.raw(), LV_OPA_COVER as lv_opa_t, part);
        }
    }
    arc
}

/// A black gradient over the list edge so rows fade out instead of being cut.
fn fade(parent: Obj, rect: Rect, top: bool) {
    let obj = Obj::plain(parent).rect(rect).bg(BG);
    let (from, to) = if top { (255, 0) } else { (0, 255) };
    // SAFETY: live object.
    unsafe {
        lv_obj_set_style_bg_grad_color(obj.raw(), color(BG), MAIN);
        lv_obj_set_style_bg_grad_dir(obj.raw(), lv_grad_dir_t_LV_GRAD_DIR_VER, MAIN);
        lv_obj_set_style_bg_main_opa(obj.raw(), from, MAIN);
        lv_obj_set_style_bg_grad_opa(obj.raw(), to, MAIN);
    }
}
```

- [ ] **Step 4: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::list 2>&1 | grep "test result"`
Expected: `ok. 5 passed`

Compare `list_menu` with `../docs/mockups/png/menu.png` and `list_system` with `system.png`. Check the focused row's surface fill, the lock after the Files count, the arc scrollbar on the right and the fades at the edges. One difference is accepted: the mockup draws edge rows 34 and 46 px tall, while every LVGL row keeps a 52 px slot (`ROW_H`), so edge rows sit a few pixels further out. Fix anything else and regenerate.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit crates/waved-ui/golden
git commit -m "feat(ui): rotary list with focus bands, fades and an arc scrollbar

- the middle row grows, edge rows shrink and fade, rows snap to the centre
- lock markers for pin-guarded rows; danger rows in red
- long titles truncate inside the row

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 20: Settings controls — radio list, value row, switch row, value arc

**Files:**
- Create: `crates/waved-ui/src/kit/components/controls.rs`
- Modify: `crates/waved-ui/src/kit/components/mod.rs` (`pub mod controls;`)
- Create: goldens `radio_after_recording.png`, `value_and_switch_rows.png`, `value_arc_60.png`
- Test: `controls.rs` (`mod tests`)

**Interfaces:**
- Produces:
  - `RadioOption { title, detail }` with `new`.
  - `RadioList::new(parent, top: i32, Vec<RadioOption>, selected: usize, impl FnMut(usize) + 'static)` with `set_selected`, `selected` and `row(index)`.
  - `ValueRow::new(parent, Rect, label, value, impl FnMut() + 'static)` with `set_value` and `obj`. Values longer than half the row end in "...".
  - `SwitchRow::new(parent, Rect, label, on: bool, impl FnMut(bool) + 'static)` with `set_on`, `is_on` and `obj`.
  - `ValueArc::new(parent, cx, cy, r, impl FnMut(u8) + 'static)` with `set_value`, `value` and `obj`.

- [ ] **Step 1: Write the failing tests**

```rust
//! Settings controls: radio lists, value rows, switch rows and the value arc.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};

    #[test]
    fn a_radio_list_selects_one_option() {
        lvgl(|lv| {
            let root = lv.screen();
            let picked = Rc::new(Cell::new(usize::MAX));
            let chosen = picked.clone();
            let list = RadioList::new(
                root,
                70,
                vec![
                    RadioOption::new("Send automatically", "As soon as Wi-Fi allows"),
                    RadioOption::new("Ask me each time", "Choose Send, Keep or Delete"),
                    RadioOption::new("Keep on Wave", "Nothing leaves the device"),
                ],
                1,
                move |index| chosen.set(index),
            );
            assert_golden("radio_after_recording", &lv.render());
            assert_targets_ok(root);
            lv.click(list.row(2));
            assert_eq!((picked.get(), list.selected()), (2, 2));
        });
    }

    #[test]
    fn value_and_switch_rows_report_taps() {
        lvgl(|lv| {
            let root = lv.screen();
            let taps = Rc::new(Cell::new(0));
            let counter = taps.clone();
            let zone = ValueRow::new(root, Rect::new(60, 146, 280, 44), "Time zone", "Brussels", move || {
                counter.set(counter.get() + 1)
            });
            let toggled = Rc::new(Cell::new(None));
            let record = toggled.clone();
            let clock = SwitchRow::new(root, Rect::new(60, 196, 280, 44), "24-hour clock", true, move |on| {
                record.set(Some(on))
            });
            assert_golden("value_and_switch_rows", &lv.render());
            assert_targets_ok(root);
            lv.click(zone.obj());
            assert_eq!(taps.get(), 1);
            lv.click(clock.obj());
            assert_eq!((toggled.get(), clock.is_on()), (Some(false), false));
        });
    }

    #[test]
    fn long_values_stay_inside_the_row() {
        lvgl(|lv| {
            let root = lv.screen();
            let host = ValueRow::new(
                root,
                Rect::new(60, 146, 280, 44),
                "Server",
                "recordings.meeting-rooms.example-company.internal",
                || {},
            );
            let row = host.obj();
            row.update_layout();
            let bounds = row.coords();
            for index in 0..row.child_count() {
                let a = row.child(index as i32).coords();
                assert!(
                    a.x1 >= bounds.x1 && a.x2 <= bounds.x2 && a.y1 >= bounds.y1 && a.y2 <= bounds.y2,
                    "child {index} overflows the row"
                );
            }
        });
    }

    #[test]
    fn the_value_arc_reports_changes() {
        lvgl(|lv| {
            let root = lv.screen();
            let seen = Rc::new(Cell::new(0u8));
            let record = seen.clone();
            let arc = ValueArc::new(root, 200, 146, 92, move |value| record.set(value));
            arc.set_value(60);
            assert_golden("value_arc_60", &lv.render());
            assert_targets_ok(root);
            arc.set_value(75);
            lv.send(arc.obj(), lv_event_code_t_LV_EVENT_VALUE_CHANGED);
            assert_eq!((seen.get(), arc.value()), (75, 75));
        });
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `set -o pipefail; cargo test -p waved-ui --lib kit::components::controls 2>&1 | tail -3`
Expected: compile errors (`RadioList`, `ValueRow`, `SwitchRow`, `ValueArc` missing)

- [ ] **Step 3: Implement** above the tests:

```rust
use crate::bindings::*;
use crate::kit::event::{on, on_click};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{color, Obj};
use crate::kit::tokens::{lvgl_angle, Rect, LINE, LIVE, RAISED, SURFACE, TEXT, TEXT_2, TEXT_3, WHITE};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const MAIN: lv_style_selector_t = LV_PART_MAIN as lv_style_selector_t;
const INDICATOR: lv_style_selector_t = LV_PART_INDICATOR as lv_style_selector_t;
const KNOB: lv_style_selector_t = LV_PART_KNOB as lv_style_selector_t;

#[derive(Clone, Debug, PartialEq)]
pub struct RadioOption {
    pub title: String,
    pub detail: String,
}

impl RadioOption {
    pub fn new(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { title: title.into(), detail: detail.into() }
    }
}

struct RadioRow {
    obj: Obj,
    ring: Obj,
    dot: Obj,
}

pub struct RadioList {
    rows: Rc<Vec<RadioRow>>,
    selected: Rc<Cell<usize>>,
}

impl RadioList {
    /// Option rows (296×52, 6 px apart) from `top`, scrolling if they don't
    /// fit above Back.
    pub fn new(
        parent: Obj,
        top: i32,
        options: Vec<RadioOption>,
        selected: usize,
        on_select: impl FnMut(usize) + 'static,
    ) -> Self {
        let viewport = Obj::plain(parent).rect(Rect::new(0, top, 400, 248 - top)).column_centered(6);
        // SAFETY: live object.
        unsafe {
            lv_obj_add_flag(viewport.raw(), lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_scroll_dir(viewport.raw(), lv_dir_t_LV_DIR_VER);
            lv_obj_set_scrollbar_mode(viewport.raw(), lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);
        }
        let rows: Vec<RadioRow> = options
            .iter()
            .map(|option| {
                let obj = Obj::plain(viewport).size(296, 52).radius(20).clickable(true).row_start(12, 16);
                let ring = Obj::plain(obj).size(20, 20).round().border(2, TEXT_3);
                let dot = Obj::plain(ring).size(10, 10).round().bg(LIVE).center().hidden(true);
                let text = Obj::plain(obj).fit().column(2);
                // 296 - 2 × 16 padding - 20 ring - 12 gap
                Obj::label(text, &option.title, Font::Body, TEXT).max_width(232).one_line();
                Obj::label(text, &option.detail, Font::Micro, TEXT_2).max_width(232).one_line();
                RadioRow { obj, ring, dot }
            })
            .collect();
        let list = Self { rows: Rc::new(rows), selected: Rc::new(Cell::new(selected)) };
        let handler = Rc::new(RefCell::new(on_select));
        for index in 0..list.rows.len() {
            let (rows, chosen, handler) = (list.rows.clone(), list.selected.clone(), handler.clone());
            on_click(list.rows[index].obj, move || {
                paint_radio(&rows, index);
                chosen.set(index);
                (handler.borrow_mut())(index);
            });
        }
        paint_radio(&list.rows, selected);
        list
    }

    pub fn set_selected(&self, index: usize) {
        paint_radio(&self.rows, index);
        self.selected.set(index);
    }

    pub fn selected(&self) -> usize {
        self.selected.get()
    }

    pub fn row(&self, index: usize) -> Obj {
        self.rows[index].obj
    }
}

fn paint_radio(rows: &[RadioRow], selected: usize) {
    for (index, row) in rows.iter().enumerate() {
        let on = index == selected;
        if on {
            row.obj.bg(SURFACE).border(2, LIVE);
        } else {
            row.obj.bg_opa(0).border(0, LIVE);
        }
        row.ring.border(2, if on { LIVE } else { TEXT_3 });
        row.dot.hidden(!on);
    }
}

pub struct ValueRow {
    obj: Obj,
    value: Obj,
}

impl ValueRow {
    /// A tappable row: label on the left, current value and a chevron on the right.
    pub fn new(parent: Obj, rect: Rect, label: &str, value: &str, on_tap: impl FnMut() + 'static) -> Self {
        let obj = Obj::plain(parent)
            .rect(rect)
            .bg(SURFACE)
            .radius(20)
            .pressed_bg(RAISED)
            .clickable(true)
            .row_start(8, 16);
        Obj::label(obj, label, Font::Small, TEXT_2).grow().one_line();
        let value_label = Obj::label(obj, value, Font::Body, TEXT).max_width(rect.w / 2).one_line();
        Obj::label(obj, icons::RIGHT, Font::Icon16, TEXT_3);
        on_click(obj, on_tap);
        Self { obj, value: value_label }
    }

    pub fn set_value(&self, value: &str) {
        self.value.set_text(value);
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}

pub struct SwitchRow {
    obj: Obj,
    track: Obj,
    knob: Obj,
    on: Rc<Cell<bool>>,
}

impl SwitchRow {
    /// A row that toggles a setting; the whole row is the target.
    pub fn new(parent: Obj, rect: Rect, label: &str, on: bool, on_toggle: impl FnMut(bool) + 'static) -> Self {
        let obj = Obj::plain(parent)
            .rect(rect)
            .bg(SURFACE)
            .radius(20)
            .pressed_bg(RAISED)
            .clickable(true)
            .row_start(8, 16);
        Obj::label(obj, label, Font::Small, TEXT_2).grow().one_line();
        let track = Obj::plain(obj).size(44, 26).radius(13);
        let knob = Obj::plain(track).size(20, 20).round().bg(WHITE);
        let state = Rc::new(Cell::new(on));
        paint_switch(track, knob, on);
        let shared = state.clone();
        let mut toggle = on_toggle;
        on_click(obj, move || {
            let next = !shared.get();
            shared.set(next);
            paint_switch(track, knob, next);
            toggle(next);
        });
        Self { obj, track, knob, on: state }
    }

    pub fn set_on(&self, on: bool) {
        self.on.set(on);
        paint_switch(self.track, self.knob, on);
    }

    pub fn is_on(&self) -> bool {
        self.on.get()
    }

    pub fn obj(&self) -> Obj {
        self.obj
    }
}

fn paint_switch(track: Obj, knob: Obj, on: bool) {
    track.bg(if on { LIVE } else { LINE });
    knob.pos(if on { 21 } else { 3 }, 3);
}

pub struct ValueArc {
    arc: Obj,
}

impl ValueArc {
    /// A 270° slider (−135°…135°) of radius `r` from 0 to 100 with a 22 px knob.
    pub fn new(parent: Obj, cx: i32, cy: i32, r: i32, on_change: impl FnMut(u8) + 'static) -> Self {
        let width = 8;
        let size = 2 * r + width;
        let arc = Obj::arc(parent)
            .pos(cx - size / 2, cy - size / 2)
            .size(size, size)
            .clickable(true);
        // SAFETY: live arc.
        unsafe {
            lv_arc_set_rotation(arc.raw(), 0);
            lv_arc_set_bg_angles(arc.raw(), lvgl_angle(-135.0), lvgl_angle(135.0));
            lv_arc_set_range(arc.raw(), 0, 100);
            for (part, hex) in [(MAIN, LINE), (INDICATOR, LIVE)] {
                lv_obj_set_style_arc_color(arc.raw(), color(hex), part);
                lv_obj_set_style_arc_width(arc.raw(), width, part);
                lv_obj_set_style_arc_rounded(arc.raw(), true, part);
                lv_obj_set_style_arc_opa(arc.raw(), LV_OPA_COVER as lv_opa_t, part);
            }
            lv_obj_set_style_bg_color(arc.raw(), color(WHITE), KNOB);
            lv_obj_set_style_bg_opa(arc.raw(), LV_OPA_COVER as lv_opa_t, KNOB);
            lv_obj_set_style_radius(arc.raw(), LV_RADIUS_CIRCLE as i32, KNOB);
            lv_obj_set_style_pad_top(arc.raw(), 7, KNOB);
            lv_obj_set_style_pad_bottom(arc.raw(), 7, KNOB);
            lv_obj_set_style_pad_left(arc.raw(), 7, KNOB);
            lv_obj_set_style_pad_right(arc.raw(), 7, KNOB);
        }
        let mut changed = on_change;
        on(arc, lv_event_code_t_LV_EVENT_VALUE_CHANGED, move |event| {
            // SAFETY: the target is this arc.
            let value = unsafe { lv_arc_get_value(event.target().raw()) };
            changed(value.clamp(0, 100) as u8);
        });
        Self { arc }
    }

    pub fn set_value(&self, percent: u8) {
        // SAFETY: live arc.
        unsafe { lv_arc_set_value(self.arc.raw(), i32::from(percent.min(100))) };
    }

    pub fn value(&self) -> u8 {
        // SAFETY: live arc.
        unsafe { lv_arc_get_value(self.arc.raw()) }.clamp(0, 100) as u8
    }

    pub fn obj(&self) -> Obj {
        self.arc
    }
}
```

- [ ] **Step 4: Generate and review the goldens**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::controls 2>&1 | grep "test result"`
Expected: `ok. 4 passed`

Compare `radio_after_recording` with `../docs/mockups/png/upload-mode.png`, `value_and_switch_rows` with `datetime.png` and `value_arc_60` with `volume.png`. Fix any differences and regenerate.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p waved-ui --all-targets -- -D warnings
git add crates/waved-ui/src/kit crates/waved-ui/golden
git commit -m "feat(ui): radio list, value row, switch row and value arc

- radio list scrolls when options don't fit above Back
- value arc is a 270 degree slider with a 22 px knob

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 21: Kit compositions and the screen-building guide

Two compositions assemble the kit the way real screens will. Their goldens are the final visual check against the approved mockups before milestone 2 starts.

**Files:**
- Create: `crates/waved-ui/src/kit/components/compositions.rs` (test only)
- Modify: `crates/waved-ui/src/kit/components/mod.rs` (`#[cfg(test)] mod compositions;`)
- Create: `crates/waved-ui/src/kit/README.md`
- Create: goldens `kit_home.png`, `kit_upload_mode.png`

**Interfaces:**
- Consumes: every component above.

- [ ] **Step 1: Write the composition tests**

```rust
//! Components assembled like real screens. The goldens are compared against
//! docs/mockups/png/home.png and upload-mode.png before milestone 2 starts.

use crate::kit::components::buttons::{back_button, PrimaryButton, Side, SideButton, Tone};
use crate::kit::components::controls::{RadioList, RadioOption};
use crate::kit::components::dial::{Dial, RingStyle};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Dispatcher;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::snapshot::UiSnapshot;
use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R, LIVE, PRIMARY, SCREEN_W, TEXT, TEXT_2, TEXT_3, TITLE_Y};

#[test]
fn home_from_kit_parts() {
    lvgl(|lv| {
        let root = lv.screen();
        let (dispatch, _bus, _ui) = Dispatcher::test();
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;
        snap.audio.call_off_hook = true;

        let mut status = StatusArea::pill(root, &dispatch);
        status.update(&snap);
        let mut dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(|| {});
        dial.set_ring(RingStyle::Live);
        dial.set_talker(Some(307.0));
        Obj::label(root, icons::MIC, Font::Icon20, TEXT_2).pos(190, 74);
        Obj::label(root, "Mic on", Font::Dial, TEXT).centered_text(SCREEN_W).pos(0, 100);
        Obj::label(root, &snap.connection_line(), Font::Body, LIVE).centered_text(SCREEN_W).pos(0, 134);
        Obj::label(root, "Tap to mute", Font::Micro, TEXT_3).centered_text(SCREEN_W).pos(0, 172);
        SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", || {});
        SideButton::new(root, Side::Right, icons::VOLUME_HIGH, "Louder", || {});
        PrimaryButton::new(root, PRIMARY, Tone::Rec, icons::RECORD, "Record", Some("includes last 30 s"), || {});

        assert_golden("kit_home", &lv.render());
        assert_targets_ok(root);
    });
}

#[test]
fn after_a_recording_from_kit_parts() {
    lvgl(|lv| {
        let root = lv.screen();
        let (dispatch, _bus, _ui) = Dispatcher::test();
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "15:22".into();
        snap.battery.level = 79;

        let mut status = StatusArea::plain(root);
        status.update(&snap);
        Obj::label(root, "After a recording", Font::Title, TEXT).centered_text(SCREEN_W).pos(0, TITLE_Y);
        RadioList::new(
            root,
            70,
            vec![
                RadioOption::new("Send automatically", "As soon as Wi-Fi allows"),
                RadioOption::new("Ask me each time", "Choose Send, Keep or Delete"),
                RadioOption::new("Keep on Wave", "Nothing leaves the device"),
            ],
            1,
            |_| {},
        );
        back_button(root, &dispatch);

        assert_golden("kit_upload_mode", &lv.render());
        assert_targets_ok(root);
    });
}
```

Register it in `components/mod.rs`:

```rust
#[cfg(test)]
mod compositions;
```

- [ ] **Step 2: Generate and review**

Run: `set -o pipefail; UPDATE_GOLDEN=1 UI_PREVIEW=1 cargo test -p waved-ui --lib kit::components::compositions 2>&1 | grep "test result"`
Expected: `ok. 2 passed`

Place `target/ui-preview/kit_home.png` next to `../docs/mockups/png/home.png`, and `kit_upload_mode.png` next to `upload-mode.png`. They must match in layout, colours, type sizes and spacing. Known, accepted differences:
- pickup patterns and icons are stroke-only Tabler glyphs;
- the primary title is 20 px (type-scale "title") where the mockup uses 18 px.

Anything else is a defect: fix the component and regenerate every affected golden.

- [ ] **Step 3: Write `crates/waved-ui/src/kit/README.md`**

````markdown
# UI kit

The building blocks for the revision-2 screens (`docs/mockups`). Milestone 2
builds every screen from these pieces. The legacy screens in `src/screens/` are
deleted when the runtime switches over.

## Rules

- Screens use `Obj`, `event::on`/`on_click` and the components. Raw `crate::bindings`
  calls belong only in `kit/obj.rs`, `kit/event.rs`, `kit/testkit.rs` and `kit/components/`.
- Every position and colour comes from `kit::tokens`. Every string is plain, sentence-case copy.
- A label that can show user data (names, hosts, values) stays on one line: `centered_text(width)`,
  or `one_line()` with a `width`, `grow()` or `max_width()`. Plain `Obj::label` wraps instead.
- Screens never navigate by sending bus commands. Use `ctx.dispatch.navigate(..)`,
  `.back()` or `.show(..)`. Services get `ctx.dispatch.command(..)`.
- Every screen test calls `assert_targets_ok(root)` and `assert_golden(..)`, and the
  golden is checked against the screen's mockup PNG.

## A screen

```rust
use crate::kit::components::buttons::back_button;
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{SCREEN_W, TEXT, TITLE_Y};

#[derive(Default)]
pub struct AboutScreen {
    status: Option<StatusArea>,
}

impl Screen for AboutScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "About", Font::Title, TEXT).centered_text(SCREEN_W).pos(0, TITLE_Y);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
    }
}
```

## Components

| Component | Constructor | Used by |
|---|---|---|
| Status cluster / pill / messages | `StatusArea::plain(root)`, `StatusArea::pill(root, &dispatch)`, `.show(&message, now_ms)` | every screen |
| Back | `back_button(root, &dispatch)` | every sub-screen |
| Text button | `text_button(root, rect, "Cancel", on_tap)` | Wi-Fi setup, USB storage |
| Primary | `PrimaryButton::new(root, PRIMARY, Tone::Rec, icons::RECORD, "Record", Some("includes last 30 s"), on_tap)` | Home, After stop, First start |
| Side buttons | `SideButton::new(root, Side::Left, icons::VOLUME_LOW, "Quieter", on_step)` | Home, Recording, File detail |
| Button rows | `ButtonRow::two/three(root, [ButtonSpec; N], on_tap)` | Recording, After stop, Call |
| Confirmation | `confirm_dialog(root, &ConfirmSpec { .. }, on_confirm, on_cancel)` | Power off, Factory reset, Delete, Stop USB storage |
| Dial | `Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3).on_tap(..)`, `set_ring`, `set_talker`, `set_gauge`, `set_progress` | Home, Recording, Resting, File detail, Update, Call, Start-up |
| Rotary list | `RotaryList::new(root, items, on_select)` | Menu, System |
| Radio list | `RadioList::new(root, top, options, selected, on_select)` | After a recording, Time zone, Dim after |
| Value / switch rows | `ValueRow::new(..)`, `SwitchRow::new(..)` | Uploads, Display, Date and time |
| Value arc | `ValueArc::new(root, cx, cy, r, on_change)` | Volume, Mic level, Display |

The radial audio-mode picker (B2), the PIN keypad (K2), file rows (D1) and Bluetooth device rows (I4)
are owned by their workstreams.

## Testing

```rust
use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};

#[test]
fn about_matches_the_mockup() {
    lvgl(|lv| {
        let root = lv.screen();
        // build the screen into `root` with a fixture UiSnapshot
        assert_golden("about", &lv.render());
        assert_targets_ok(root);
    });
}
```

- `UPDATE_GOLDEN=1 cargo test -p waved-ui --lib` writes the goldens to `golden/`.
- `UI_PREVIEW=1` also writes panel-masked copies to `target/ui-preview/` for comparison
  with `docs/mockups/png/`.
- A mismatch writes the actual frame to `target/golden-diff/<name>.actual.png`.
- Taps in tests: `lv.click(obj)`, `lv.send(obj, code)`, and `lv.advance(ms)` for held buttons.

## Fonts and icons

`fonts/build_fonts.sh` regenerates `src/kit/fonts/*.c` and `src/kit/icons.rs`; see
`fonts/README.md`. To add an icon, add it to `fonts/icons.py` and rerun the script.
````

- [ ] **Step 4: Run the whole crate and lint**

Run: `set -o pipefail; cargo test -p waved-ui --lib 2>&1 | grep "test result"`
Expected: `ok. 132 passed` (69 legacy plus 63 kit)

Run: `cargo clippy -p waved-ui --all-targets -- -D warnings`
Expected: no warnings

Run: `set -o pipefail; scripts/linux-test.sh test -p waved-ui --lib 2>&1 | grep "test result"`
Expected: `ok`. The goldens also match on Linux: LVGL's software renderer is deterministic.

- [ ] **Step 5: Commit**

```bash
git add crates/waved-ui/src/kit crates/waved-ui/golden
git commit -m "test(ui): kit compositions against the mockups, and the screen guide

- home and after-a-recording assembled from kit parts with goldens
- kit/README.md: rules, a screen skeleton, components and the golden workflow

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 22: Milestone 1 device check

This is the first device pass (D6, D8). Nothing in the legacy UI changes, so the device behaves as before, except that saved settings now restore on boot. The kit isn't wired into the runtime until milestone 2, so its golden frames are written straight to the framebuffer to judge colours and type on the real panel.

**Files:**
- Create: `scripts/png-to-fb.py` (golden PNG to framebuffer image)
- Modify: `docs/DISPLAY_HARDWARE.md`, `CLAUDE.md` (panel type, after the visual check)

- [ ] **Step 1: Build and deploy** (from `waved/`):

```bash
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
adb push target/aarch64-unknown-linux-gnu/release/waved /data/waved/waved
adb shell "killall waved 2>/dev/null; sleep 2; (setsid /data/waved/waved >/tmp/waved.log 2>&1 </dev/null &)"
```

Expected:
- the device boots to the legacy Home screen;
- `adb shell cat /data/waved/config.toml | head -1` prints `config_version = 2` after the first device-side setting change, or already does if the v1 file was migrated on save.

- [ ] **Step 2: Settings survive a restart**

On the device, change the speaker volume and the noise reduction level, wait 2 s, then restart waved with the `adb shell` command from Step 1.
Expected: both values come back, and the noise level appears in `adb shell grep nr_level /data/waved/config.toml`.

- [ ] **Step 3: Confirm the panel type (D8)**

```bash
adb shell "echo 0 > /sys/class/leds/lcd-backlight/brightness; sleep 3; echo 255 > /sys/class/leds/lcd-backlight/brightness"
```

Expected: the image disappears completely for three seconds, which confirms an LCD. Record the result:
- In `docs/DISPLAY_HARDWARE.md`, replace the title `# Display Hardware — fb_ili79400 Circular OLED` with `# Display Hardware — fb_ili79400 Circular LCD` and line 7, `- **Physical shape**: circular OLED, radius ≈190 px centred at **(200, 152)** in framebuffer coordinates`, with `- **Physical shape**: circular LCD with a PWM backlight (/sys/class/leds/lcd-backlight), radius ≈190 px centred at **(200, 152)** in framebuffer coordinates`.
- In `CLAUDE.md`, change `circular OLED` to `circular LCD` in the intro (line 4) and `Circular OLED` to `Circular LCD` in the hardware table (line 14).

- [ ] **Step 4: Write the framebuffer converter** `scripts/png-to-fb.py` (`chmod +x`). It needs only the Python standard library:

```python
#!/usr/bin/env python3
"""Convert a 400x304 golden PNG into a raw image for the Wave framebuffer.

The panel takes big-endian RGB565 and is mounted upside down, so the image is
rotated 180 degrees (docs/DISPLAY_HARDWARE.md, "flush() implementation").

    scripts/png-to-fb.py crates/waved-ui/golden/kit_home.png target/kit_home.fb
"""
import struct
import sys
import zlib

W, H = 400, 304


def read_rgb(path):
    """Rows of RGB bytes from an 8-bit, non-interlaced RGB PNG."""
    with open(path, "rb") as handle:
        data = handle.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        sys.exit(f"{path}: not a PNG")
    pos, idat, header = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    width, height, depth, color, _, _, interlace = header
    if (width, height, depth, color, interlace) != (W, H, 8, 2, 0):
        sys.exit(f"{path}: expected a {W}x{H} 8-bit RGB PNG, got {header}")
    raw = zlib.decompress(idat)
    stride = W * 3
    rows, prev = [], bytearray(stride)
    for y in range(H):
        start = y * (stride + 1)
        kind = raw[start]
        line = bytearray(raw[start + 1:start + 1 + stride])
        for i in range(stride):
            a = line[i - 3] if i >= 3 else 0
            b = prev[i]
            c = prev[i - 3] if i >= 3 else 0
            if kind == 1:
                line[i] = (line[i] + a) & 0xFF
            elif kind == 2:
                line[i] = (line[i] + b) & 0xFF
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 0xFF
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 0xFF
        rows.append(line)
        prev = line
    return rows


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: png-to-fb.py <golden.png> <out.fb>")
    rows = read_rgb(sys.argv[1])
    out = bytearray()
    for y in reversed(range(H)):
        line = rows[y]
        for x in reversed(range(W)):
            r, g, b = line[x * 3:x * 3 + 3]
            out += struct.pack(">H", ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3))
    with open(sys.argv[2], "wb") as handle:
        handle.write(out)


if __name__ == "__main__":
    main()
```

Run: `python3 scripts/png-to-fb.py crates/waved-ui/golden/kit_home.png target/kit_home.fb && wc -c target/kit_home.fb`
Expected: `243200 target/kit_home.fb` (400 × 304 × 2 bytes)

- [ ] **Step 5: Look at the kit on the panel**

```bash
for name in kit_home kit_upload_mode list_menu confirm_power_off; do
  python3 scripts/png-to-fb.py crates/waved-ui/golden/$name.png target/$name.fb
  adb push target/$name.fb /tmp/$name.fb
done
# Stop the respawn watchdog and waved, or waved repaints over the image.
adb shell "kill -9 \$(pidof S60waved 2>/dev/null || echo 330); killall waved 2>/dev/null; sleep 1; cat /tmp/kit_home.fb > /dev/fb0"
```

Show each image in turn with `adb shell "cat /tmp/<name>.fb > /dev/fb0"`. Look at it at arm's length, straight on and from about 45°. Expected:
- "Tap to mute" (TEXT_3) and the grey idle ring (LINE) are readable but quiet;
- teal and red are clearly different, also from the side;
- the 12 px side-button labels are legible;
- nothing that matters is cut off by the round bezel.

If a colour or size fails, write down which one and how in `../TODO.md` under Phase 0. Token changes are cheap now; after milestone 2 starts they invalidate goldens in every workstream.

Then restore the normal firmware:

```bash
adb shell "start-stop-daemon -S -b -x /bin/sh -- /etc/init.d/S60waved start"
```

Expected: waved is back on the legacy Home screen within about 3 s.

- [ ] **Step 6: Commit**

```bash
git add scripts/png-to-fb.py docs/DISPLAY_HARDWARE.md CLAUDE.md
git commit -m "docs(display): the panel is an LCD; golden frames can be shown on it

- confirmed on the device: backlight 0 blanks the image
- matches the device tree (ilitek,ili79400 TDDI, pwm-leds lcd-backlight)
- scripts/png-to-fb.py writes golden PNGs in the panel's framebuffer format

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## After milestone 1: how milestones 2–4 run

Each workstream gets its own plan, written at the start of its milestone against the interfaces this milestone produced. The plan lives at `docs/superpowers/plans/<date>-display-redesign-m<N>-<workstream>.md` and follows the same writing-plans format and Global Constraints. The workstreams then run in parallel, one git worktree each, branched from `feat/display-redesign`. A whole-branch review runs before each merge.

### Ownership, so parallel work doesn't collide

| Workstream | Creates or modifies | Never touches |
|---|---|---|
| A main screens | `waved-ui/src/screens_v2/{home,recording,decision,ambient,boot,first_run}.rs` | other screens, services |
| B settings screens | `waved-ui/src/screens_v2/{menu,audio_mode,noise,volume,mic_height,system,display,datetime,wifi,uploads,upload_mode}.rs`, plus the radial picker | main screens, services |
| C recording and audio | `waved-meeting/*`, `waved-audio/*` | UI, config schema |
| D Files | `waved-ui/src/screens_v2/{files,file_detail}.rs`, the meeting list/playback API added together with C | other screens |
| E system services | a new `waved-system` crate (backlight, power key, time zone, factory reset), `waved-ui` display power module | screens |
| F–K (milestone 3) | their own crates and screens, as listed in `../TODO.md` | shared contracts |
| **Integrator** (this session) | the hotspots `waved-state`, `waved-config`, `waved/src/main.rs`, `waved-ui/src/lib.rs`, `nav.rs`, `screens_v2/mod.rs` registry | — |

A workstream that needs a new command, state field or config key asks the integrator. The integrator adds it on `feat/display-redesign`, and the workstream rebases.

### End of milestone 2: runtime switch (integrator)

1. Replace the legacy loop in `waved-ui/src/lib.rs`:
   - one `Router`, a `screens_v2` registry (`Route` → `Box<dyn Screen>`) and a `Dispatcher` with a UI `mpsc` channel;
   - a `UiSnapshot` captured per iteration;
   - device events mapped to status-area messages (the upload and integrity notices become `Message`s);
   - swipe-down / swipe-right gestures and the wake guard (P0.5), driven by router events.
2. Delete the legacy pieces:
   - `src/screens/`, `nav.rs`, `theme.rs`, `toast.rs`, `status_bar.rs`, `control_center.rs`;
   - the PNG asset paths and the Montserrat fonts;
   - the `Open*`/`CloseSettings` commands (fixes BUG-11 and BUG-13).

   Then set `LV_FONT_DEFAULT` to `font_body_15`.
3. Run the milestone-2 device pass from `../TODO.md`.

### Execution method

Every milestone uses parallel subagents.
- **Milestone 1 (this plan):** two lanes (see Execution Rules). Each lane runs `superpowers:subagent-driven-development`: a fresh implementer per task and a reviewer per task. Both lanes are dispatched together through `superpowers:dispatching-parallel-agents`. The integrator (this session) rebases lane B after Task 3, merges both lanes, runs the whole-branch review and does Task 22 with the user, who has the device.
- **Milestones 2 and 3:** one lane per workstream, run the same way, each from its own plan.
