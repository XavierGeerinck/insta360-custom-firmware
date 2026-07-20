# Recording Stop and Webhook Feedback Design

**Date:** 2026-07-20
**Status:** Approved for implementation
**Scope:** Immediate Stop acknowledgement and truthful webhook-result toasts for
recordings created during the current UI session.

## 1. Context

On-device acceptance testing confirmed that Stop already works correctly:

- the recording is finalized as a durable WAV plus versioned metadata;
- the database transitions through `queued` to `uploaded`;
- the webhook receives the multipart request and returns `202`; and
- the worker persists the successful result.

The display does not acknowledge the Stop tap or the later webhook result.
This made a successful operation look inert and led to a second recording being
started while the first upload was still completing.

The UI already owns a top-layer timed `Toast`, and the shared state bus already
has a broadcast `DeviceEvent` channel. The feature should reuse both.

## 2. Goals

1. Acknowledge a Stop tap immediately, before WAV finalization or networking
   completes.
2. Distinguish durable local saving from confirmed remote delivery.
3. Show upload outcomes only for recordings stopped during the current UI
   session, not unrelated backlog restored at boot.
4. Keep recording finalization, navigation, queueing, retry, and retention
   behavior unchanged.
5. Make success, deferred delivery, and configuration failures truthful.

## 3. Non-goals

- A persistent notification center or recording-history screen.
- Progress percentages for WAV finalization or HTTP transfer.
- Cancelling an upload.
- Changing webhook retry or retention policy.
- Showing a toast for restored backlog that was not recorded in the current UI
  session.
- Redesigning the existing toast component.

## 4. User experience

### 4.1 Stop acknowledgement

When the UI receives `Command::StopRecording`, it immediately shows:

> Saving recording...

The toast lasts 1.5 seconds. Existing state-driven navigation remains
authoritative: Automatic mode returns to Home after durable finalization, Ask
mode opens the confirmation screen, and Local-only mode returns to Home.

The UI records the active meeting UUID in a small bounded notice tracker before
the meeting service clears `MeetingState.meeting_id`. The existing
`RecordingStopped` event supplies the same UUID as a race-safe fallback.

### 4.2 Delivery result

The upload worker emits a typed device event after it has persisted the
corresponding database state:

- successful `2xx` delivery followed by `mark_uploaded`:
  **Recording uploaded**;
- transient failure with retries remaining:
  **Saved - upload queued**;
- permanent configuration/artifact failure, or exhausted retries:
  **Saved - check Webhook**.

The UI displays an upload-result toast only when its UUID is in the notice
tracker. A confirmed success removes the UUID. Deferred and needs-attention
results retain it so a later manual or automatic retry can still produce the
success toast. The tracker keeps at most 16 UUIDs and evicts the oldest entry,
preventing unbounded UI state.

Toasts use the existing top LVGL layer, so a result can appear after navigation
from Recording to Home or while another screen is open.

## 5. Event model

`waved-state::DeviceEvent` gains three variants:

```rust
RecordingUploadSucceeded(Uuid),
RecordingUploadDeferred(Uuid),
RecordingUploadNeedsAttention(Uuid),
```

`UploadWorker` receives a clone of the existing device-event sender.

Events are emitted only after the matching database transition succeeds. A
webhook `2xx` response whose `mark_uploaded` call fails must not generate a
success toast. A transient failure at `retry_max` is
`RecordingUploadNeedsAttention`, because the worker will not retry it without
explicit user action.

These events are transient UI notifications. Durable truth remains in SQLite
and `MeetingState`; losing an event during shutdown does not affect delivery
state.

## 6. UI integration

The UI thread subscribes to `bus.events` alongside its existing command
receiver. It drains both receivers on every LVGL loop:

1. `Command::StopRecording` captures the current meeting UUID, adds it to the
   bounded tracker when still available, and always shows
   **Saving recording...**.
2. `RecordingStopped` adds the UUID as a fallback when finalization won the
   race and cleared the watched meeting state first.
3. Upload-result events are mapped to toast text only when the tracker contains
   their UUID.
4. Lagged event-channel errors are ignored for toast purposes because the
   notification is non-durable and queue/database state remains visible on the
   Webhook status screen.

The mapping and bounded correlation behavior live in a pure Rust helper so they
can be tested without LVGL.

## 7. Error and mode behavior

- Automatic mode receives the full two-stage experience.
- Ask mode receives immediate saving feedback; choosing Send later can produce
  the upload-result toast for the same UUID.
- Local-only and Save choices receive immediate saving feedback and no upload
  claim.
- Discard does not produce an upload toast.
- Missing endpoint, invalid authentication, missing artifacts, and exhausted
  retries use **Saved - check Webhook**.
- Offline or transient network failure with attempts remaining uses
  **Saved - upload queued**.
- Existing queue counters and the Webhook status screen remain the detailed
  diagnostic surface.

## 8. Testing and acceptance

Host tests cover:

- bounded UUID tracking and oldest-entry eviction;
- ignoring upload events for restored or unrelated meetings;
- exact success, deferred, and needs-attention message mapping;
- retaining tracked UUIDs after deferred/attention results;
- removing a tracked UUID after success;
- worker event emission only after successful database persistence; and
- exhausted transient retries mapping to needs-attention.

Regression verification runs the existing meeting, webhook-contract, state,
and UI tests plus strict Clippy for changed crates. The final ARM64 build must
remain compatible with GLIBC 2.17.

On-device acceptance:

1. Start a short Automatic-mode recording.
2. Tap Stop and observe **Saving recording...** immediately.
3. Confirm the UI returns to Home after durable finalization.
4. Let the receiver return `202`.
5. Observe **Recording uploaded**.
6. Confirm the WAV and metadata exist, SQLite says `uploaded`, and the receiver
   stored exactly one multipart request for that UUID.
