# Display Redesign — Milestone 1.1: Kit on the Measured Panel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Move the milestone 1 UI kit onto the panel's measured visible area and the approved round bottom actions, so milestone 2 builds screens on the right geometry.

**Why:** The milestone 1 device check measured the visible image as a circle of r ≈ 198 around (200, 106), cut flat at framebuffer row 0 — not the documented r 190 around (200, 152). The user then re-approved the mockups on the panel (commit `c244c9f`): layouts moved up, dial r 70 at (200, 121), and every bottom action is a round button in the side-button style (Record and Stop share one spot).

**Spec:** `../docs/mockups/README.md` and `../docs/mockups/index.html` (commit `c244c9f`), `docs/DISPLAY_HARDWARE.md` ("Visible Area").

## Global Constraints

Everything in `docs/superpowers/plans/2026-10-04-display-redesign-m1-foundation.md` "Global Constraints" still holds, except these values, which replace the old geometry:

- Visible area: circle **r 198 centred (200, 106)**, clipped by the 400 × 304 framebuffer (flat top at y 0).
- Tap limit: every tappable pixel within **r 190 of (200, 106)** and at **y ≥ 6** (`TAP_MIN_Y`). Targets ≥ 44 px both ways; ext click area may provide size, the visible box must satisfy the limit.
- Status pill **240 × 28 at (80, 6)**, radius 14, hit area 240 × 44; status cluster **(80, 8, 240, 20)**; messages in the same slot, max width **240**.
- Dial **centre (200, 121), r 70**, stroke 3; talker arc 44°, stroke **6**, dot r 4 on **r 82**; tap circle r 71 (diameter 142). Gauge (stroke 5) and progress (stroke 6) use the same circle.
- Side buttons 60 × 60 at **(26, 91) / (314, 91)**; labels 90 wide at x 11 / 299, top **155**.
- Bottom actions are round buttons centred on an arc of **r 135 around (200, 106)**:
  - `BOTTOM_MAIN` (shared by Home Record and Recording Stop, also Send / Set up Wi-Fi): **64 × 64 at (168, 209)**, icon 26 px, label 90 wide at (155, 279).
  - `BOTTOM_LEFT` / `BOTTOM_RIGHT` companions (Mark, Pause/Resume, Keep, Delete): **56 × 56 at (85, 181) / (259, 181)**, icon 22 px, labels 90 wide at (68, 241) / (242, 241).
  - `BOTTOM_PAIR_LEFT` / `BOTTOM_PAIR_RIGHT` (Decline / Answer): **64 × 64 at (81, 177) / (255, 177)**, labels at (68, 245) / (242, 245).
  - Stop hold ring: r 35 around (200, 241), stroke 3, LINE track, REC progress clockwise over 1 s; the circle turns RAISED while held.
  - Style: fill SURFACE; pressed RAISED + 2 px LIVE ring; the accent is on the icon (and label where it carries meaning). **Red is never a button fill.**
  - `BOTTOM_PILL_WIDE` 184 × 52 at (108, 215), radius 26 (Stop USB storage); `BOTTOM_PILL_PAIR` 124 × 50 at (70, 196) / (206, 196), radius 25 (confirmation Cancel / action).
- Back **(144, 244, 112, 44)**, radius 22. `TITLE_Y` 38, `CONTENT_TOP` 70.
- Rotary list viewport **y 34–240** (fades 14 px top / 16 px bottom), focus centre **y 144**, uniform **48 px** slots (ruling from milestone 1: uniform slots keep snapping exact), arc scrollbar **r 188 around (200, 106), 62°–122°**, stroke 4.
- Radio list rows 296 × 52 at x 52 from y 68.
- Confirmation: icon disc 56 at (172, 40), title top 104, body top 142 (x 65, w 270), then `BOTTOM_PILL_PAIR`.
- Pre-recording indicator: a 7 px LIVE dot leading the status pill contents (replaces the primary button's "includes last 30 s" line).

## Execution

One worktree branch (`m1-relayout`) from `feat/display-redesign`; tasks run in order; each task ends green (host `cargo test -p waved-ui --lib`, clippy clean in `src/kit/**`) with goldens regenerated and visually compared with the re-approved mockup PNGs in `../docs/mockups/png/`.

### Task 1: Geometry tokens and the components that use them

**Files:** `crates/waved-ui/src/kit/tokens.rs`, `testkit.rs` (panel mask + tap check), `components/status.rs`, `components/dial.rs`, `components/list.rs`, `components/compositions.rs` (positions only), `components/controls.rs` (radio list top), `components/row.rs` (confirm layout), affected goldens, `kit/README.md` (geometry notes).

- Replace the panel geometry tokens (`PANEL_CY` 106, `PANEL_R` 198, `TAP_LIMIT_R` 190, new `TAP_MIN_Y` 6) and make `Rect::within_tap_limit` check both the circle and `TAP_MIN_Y`. Keep the old-back-button regression test meaningful (pick a rect that fails the new limit).
- Update layout tokens to the Global Constraints values: `STATUS`, `STATUS_PILL`, `MESSAGE_MAX_W`, `TITLE_Y`, `CONTENT_TOP`, `DIAL_CX/CY/R`, the dial content stack (`DIAL_ICON_Y` 73, `DIAL_TITLE_Y` 95, `DIAL_LINE_Y` 127, `DIAL_HINT_Y` 153), `SIDE_LEFT/RIGHT`, `SIDE_LABEL_Y`, `BACK`, and add the bottom-action rects (`BOTTOM_MAIN`, `BOTTOM_LEFT`, `BOTTOM_RIGHT`, `BOTTOM_PAIR_LEFT`, `BOTTOM_PAIR_RIGHT`, `BOTTOM_PILL_WIDE`, `BOTTOM_PILL_PAIR: [Rect; 2]`) plus their label positions. Remove `PRIMARY`, `ROW2`, `ROW3` only if nothing still needs them after Task 2 — in this task keep them so the build stays green, marked `#[deprecated]` is not needed; Task 2 removes them.
- testkit: the review mask (`masked()`) uses the measured circle; `target_problems` uses the new `within_tap_limit`.
- StatusArea: 240-wide pill/cluster/message slot; add the pre-recording dot (shown when `snap.meeting.pre_recording_active`, or `snap.settings.pre_recording` if that's the live signal — check waved-state and pick the field that reflects "buffering now"; document the choice).
- Dial: default geometry from the new tokens (r 70, talker stroke 6, dot on r + 12 = 82, tap circle r 71).
- RotaryList: viewport y 34–240, `FOCUS_Y` 144, 48 px slots (pad top = 144 − 34 − 24 = 86, pad bottom = 206 − 86 − 48 = 72), fades 14/16, scrollbar r 188 around (200, 106) over 62°–122°.
- RadioList default top 68 in callers/tests; confirmation layout per the Global Constraints (pill pair comes in Task 2 — keep ButtonRow::two for now but move it to `BOTTOM_PILL_PAIR` rects).
- Tests: update token tests for the new limit (every layout rect within the limit, minimum sizes, an angle/geometry test), keep every existing component test meaningful, regenerate goldens with `UPDATE_GOLDEN=1 UI_PREVIEW=1` and compare each with its re-approved mockup.

### Task 2: Round bottom actions

**Files:** `components/buttons.rs` (new `RoundButton`; remove `PrimaryButton` if unused), `components/row.rs` (remove `ButtonRow::three`; `ButtonRow::two` becomes the confirmation pill pair), `components/compositions.rs`, `kit/tokens.rs` (remove `PRIMARY`, `ROW2`, `ROW3` once unused), goldens, `kit/README.md` (components table).

- `RoundButton::new(parent, spot: BottomSpot, icon, label, accent: Tone, on_tap)` where `BottomSpot` covers `Main`, `Left`, `Right`, `PairLeft`, `PairRight` (rects and label positions from the tokens). Round SURFACE fill, pressed RAISED + 2 px LIVE ring (reuse the side-button styling — `SideButton` and `RoundButton` should share one implementation, e.g. `SideButton` becomes a `RoundButton` at the side spots, or both build on one private helper; no duplicated styling). Icon and label take the accent colour where it carries meaning (record dot red, Stop red, Send/Answer teal, Delete/Decline red, Resume amber); other labels TEXT_2 micro.
- `set_enabled(bool)` (35 % opacity, not tappable) and `set_icon/set_label/set_accent` so Recording ⇄ Paused can swap Pause/Resume in place.
- Hold-to-stop: `RoundButton::hold(duration_ms, on_complete)` (or a separate `HoldButton` wrapper) draws the r 35 ring around the main spot: LINE track, REC progress clockwise over `HOLD_TO_STOP_MS`; releasing early resets; completion calls back once. Test with `lv.send(PRESSED)` + `lv.advance(..)` + `RELEASED`/`PRESS_LOST`.
- Remove the pill `PrimaryButton` and `ButtonRow::three` (nothing in M2 should use them); keep `text_button` (wide pill) and `ButtonRow::two` (pill pair).
- Compositions: `kit_home` uses `RoundButton` Main for Record (red dot, "Record") and the new dial/status geometry; add `kit_recording` (Recording screen from kit parts: red recording ring + talker, timer, Mark / Stop (hold) / Pause round buttons, side buttons) and compare with `../docs/mockups/png/recording.png`; `kit_upload_mode` keeps radio list + Back at the new positions.
- Goldens for each RoundButton spot/state (Home record, Recording row, Paused row with amber Resume, call pair, Stop mid-hold) compared with the mockups.

### Task 3: Device check of the new kit frames

Convert the regenerated goldens with `scripts/png-to-fb.py` and show them on the panel with the user (waved stopped, watchdog restarted afterwards with `start-stop-daemon -S -b -m -p /tmp/S60waved.pid -x /bin/sh -- /etc/init.d/S60waved start`).
