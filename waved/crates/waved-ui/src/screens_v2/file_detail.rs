//! Recording detail (docs/mockups file-detail.png): the dial becomes a
//! playback scrubber, Delete sits on the left, Send / Send again on the
//! right, and the upload state or failure reason under them.
//!
//! The first half is pure logic, tested on the host; `FileDetailScreen`
//! builds the screen on it.

use super::files::{recording_title, sentence, status_look};
use crate::kit::components::buttons::{back_button, RoundButton, Tone};
use crate::kit::components::scrubber::{ScrubEvent, Scrubber};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::{Dispatcher, Severity};
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::{ConfirmKind, Route};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{
    DETAIL_NOTE_Y, DETAIL_SIDE_LEFT, DETAIL_SIDE_LEFT_LABEL, DETAIL_SIDE_RIGHT,
    DETAIL_SIDE_RIGHT_LABEL, SCREEN_W, TEXT, TEXT_2, TITLE_Y, WARN,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use uuid::Uuid;
use waved_state::{Command, PlaybackState, RecordingStatus, RecordingSummary};

/// After a seek, show its target until playback reports a position within
/// `SEEK_TOLERANCE_MS` of it, or for at most `SEEK_SETTLE_MS`.
pub const SEEK_SETTLE_MS: u64 = 1_500;
pub const SEEK_TOLERANCE_MS: u32 = 1_000;
pub const SENDING: &str = "Sending the recording";
pub const BUSY_RECORDING: &str = "Can't play while recording";
pub const BUSY_CALL: &str = "Can't play during a call";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayView {
    /// The speaker is playing (or paused on) this recording.
    pub active: bool,
    pub playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingSeek {
    pub target_ms: u32,
    pub sent_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CentreTap {
    Play { from_ms: u32 },
    Pause,
    Resume,
    Refuse(&'static str),
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendAction {
    Send,
    SendAgain,
}

impl SendAction {
    pub fn icon(self) -> &'static str {
        match self {
            SendAction::Send => icons::SEND,
            SendAction::SendAgain => icons::REFRESH,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SendAction::Send => "Send",
            SendAction::SendAgain => "Send again",
        }
    }
}

/// What the scrubber shows for recording `id`. Playback of another recording
/// is ignored; until this one plays, the position is the locally chosen
/// start (`local_ms`) and the duration comes from the summary.
pub fn play_view(
    id: Uuid,
    rec: &RecordingSummary,
    playback: &PlaybackState,
    local_ms: u32,
) -> PlayView {
    let active = playback.recording == Some(id);
    let duration_ms = if active && playback.duration_ms > 0 {
        playback.duration_ms
    } else {
        rec.duration_secs.saturating_mul(1_000)
    };
    let position_ms = if active {
        playback.position_ms
    } else {
        local_ms
    }
    .min(duration_ms);
    PlayView {
        active,
        playing: active && playback.playing,
        position_ms,
        duration_ms,
    }
}

/// Marks from the playback when it is this recording's, else from the summary.
pub fn marks_for<'a>(
    id: Uuid,
    rec: &'a RecordingSummary,
    playback: &'a PlaybackState,
) -> &'a [u32] {
    if playback.recording == Some(id) && !playback.marks_ms.is_empty() {
        &playback.marks_ms
    } else {
        &rec.marks_ms
    }
}

pub fn shown_position(
    actual_ms: u32,
    pending: Option<PendingSeek>,
    now_ms: u64,
) -> (u32, Option<PendingSeek>) {
    match pending {
        Some(seek)
            if actual_ms.abs_diff(seek.target_ms) > SEEK_TOLERANCE_MS
                && now_ms.saturating_sub(seek.sent_at_ms) < SEEK_SETTLE_MS =>
        {
            (seek.target_ms, Some(seek))
        }
        _ => (actual_ms, None),
    }
}

pub fn playable(rec: &RecordingSummary) -> bool {
    rec.status != RecordingStatus::Damaged && rec.duration_secs > 0
}

/// Why playback can't start now: the speaker would play into a recording,
/// or over a call.
pub fn play_guard(snap: &UiSnapshot) -> Option<&'static str> {
    if snap.meeting.recording {
        Some(BUSY_RECORDING)
    } else if snap.in_call() {
        Some(BUSY_CALL)
    } else {
        None
    }
}

/// What a tap in the middle does. Pausing is always allowed.
pub fn centre_tap(
    view: &PlayView,
    local_ms: u32,
    playable: bool,
    guard: Option<&'static str>,
) -> CentreTap {
    if !playable {
        return CentreTap::Nothing;
    }
    if view.playing {
        return CentreTap::Pause;
    }
    if let Some(reason) = guard {
        return CentreTap::Refuse(reason);
    }
    if view.active {
        CentreTap::Resume
    } else {
        CentreTap::Play { from_ms: local_ms }
    }
}

/// "Send" for a recording kept on Wave, "Send again" after a failure; nothing
/// once it is sent or on its way, for damaged files, or without a server.
pub fn send_action(status: RecordingStatus, server_set: bool) -> Option<SendAction> {
    if !server_set {
        return None;
    }
    match status {
        RecordingStatus::LocalOnly => Some(SendAction::Send),
        RecordingStatus::Failed => Some(SendAction::SendAgain),
        _ => None,
    }
}

/// The line under the scrubber: the failure and its reason in amber, or the
/// upload state in Text 2.
pub fn note_line(rec: &RecordingSummary, wifi_connected: bool) -> (String, u32) {
    let (_, _, words) = status_look(rec.status, wifi_connected);
    let head = sentence(words);
    match rec.status {
        RecordingStatus::Failed | RecordingStatus::Damaged => {
            let reason = rec
                .last_error
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty());
            let text = match reason {
                Some(reason) => format!("{head}: {reason}"),
                None => head,
            };
            (text, WARN)
        }
        _ => (head, TEXT_2),
    }
}

/// The note box: room for "Upload failed: server didn't answer", narrow
/// enough to stay inside the panel at y 218 (about 314 px visible there).
const NOTE_W: i32 = 300;

/// What the scrubber's handlers need from the last snapshot.
#[derive(Default)]
struct Live {
    view: PlayView,
    playable: bool,
    guard: Option<&'static str>,
    /// The start chosen on the ring before this recording plays.
    local_ms: u32,
    /// A seek (or a start) sent to the player that it has not reached yet.
    pending: Option<PendingSeek>,
    now_ms: u64,
}

fn on_scrub(id: Uuid, live: &RefCell<Live>, dispatch: &Dispatcher, event: ScrubEvent) {
    let mut live = live.borrow_mut();
    match event {
        ScrubEvent::TogglePlay => {
            match centre_tap(&live.view, live.local_ms, live.playable, live.guard) {
                CentreTap::Play { from_ms } => {
                    dispatch.command(Command::PlayRecording(id));
                    // The player starts at 0 and seeks after: hold the knob on
                    // the chosen start meanwhile instead of flicking to 0:00.
                    live.pending = None;
                    if from_ms > 0 {
                        dispatch.command(Command::SeekPlayback(from_ms));
                        live.pending = Some(PendingSeek {
                            target_ms: from_ms,
                            sent_at_ms: live.now_ms,
                        });
                    }
                }
                CentreTap::Pause => dispatch.command(Command::PausePlayback),
                CentreTap::Resume => dispatch.command(Command::ResumePlayback),
                CentreTap::Refuse(text) => dispatch.show(Severity::Info, text),
                CentreTap::Nothing => {}
            }
        }
        ScrubEvent::Seek(ms) => {
            if live.view.active {
                dispatch.command(Command::SeekPlayback(ms));
                live.pending = Some(PendingSeek {
                    target_ms: ms,
                    sent_at_ms: live.now_ms,
                });
            } else {
                live.local_ms = ms;
            }
            live.view.position_ms = ms;
        }
    }
}

/// Recording detail (`Route::FileDetail(id)`): playback of this recording,
/// Delete (asks first) and Send / Send again.
pub struct FileDetailScreen {
    id: Uuid,
    root: Option<Obj>,
    dispatch: Option<Dispatcher>,
    /// An upload server is configured.
    server_set: bool,
    status: Option<StatusArea>,
    title: Option<Obj>,
    scrubber: Option<Scrubber>,
    delete: Option<RoundButton>,
    send: Option<(SendAction, RoundButton)>,
    note: Option<Obj>,
    back: Option<Obj>,
    live: Rc<RefCell<Live>>,
    /// Title and note as last drawn, to skip relabelling every frame.
    shown_title: String,
    shown_note: Option<(String, u32)>,
    /// Already went back because the recording disappeared: Delete does
    /// nothing more for it (Back is deleted then).
    left: Rc<Cell<bool>>,
}

impl FileDetailScreen {
    pub fn new(id: Uuid) -> Self {
        Self {
            id,
            root: None,
            dispatch: None,
            server_set: false,
            status: None,
            title: None,
            scrubber: None,
            delete: None,
            send: None,
            note: None,
            back: None,
            live: Rc::default(),
            shown_title: String::new(),
            shown_note: None,
            left: Rc::default(),
        }
    }

    /// Show, swap or remove the right-hand button. A changed action rebuilds
    /// it; deleting the circle deletes its label too.
    fn sync_send(&mut self, wanted: Option<SendAction>) {
        if self.send.as_ref().map(|(action, _)| *action) == wanted {
            return;
        }
        if let Some((_, button)) = self.send.take() {
            button.obj().delete();
        }
        let (Some(action), Some(root), Some(dispatch)) = (wanted, self.root, self.dispatch.clone())
        else {
            return;
        };
        let id = self.id;
        let button = RoundButton::at(
            root,
            DETAIL_SIDE_RIGHT,
            DETAIL_SIDE_RIGHT_LABEL,
            action.icon(),
            action.label(),
            Tone::LiveIcon,
            move || {
                dispatch.command(Command::RetryUpload(id));
                dispatch.show(Severity::Info, SENDING);
            },
        );
        self.send = Some((action, button));
    }
}

impl Screen for FileDetailScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.root = Some(root);
        self.dispatch = Some(ctx.dispatch.clone());
        // Only the setup page changes the endpoint, and saving there restarts
        // waved (RestartForConfig), so the boot-time copy is current.
        self.server_set = !ctx.config.api.endpoint.trim().is_empty();
        self.status = Some(StatusArea::plain(root));
        let title = Obj::label(root, "", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        // The scrubber reads touches in screen coordinates: it goes on the root.
        let (id, live, dispatch) = (self.id, self.live.clone(), ctx.dispatch.clone());
        self.scrubber = Some(Scrubber::new(root, move |event| {
            on_scrub(id, &live, &dispatch, event)
        }));
        let (dispatch, left) = (ctx.dispatch.clone(), self.left.clone());
        self.delete = Some(RoundButton::at(
            root,
            DETAIL_SIDE_LEFT,
            DETAIL_SIDE_LEFT_LABEL,
            icons::TRASH,
            "Delete",
            Tone::Rec,
            move || {
                if !left.get() {
                    dispatch.navigate(Route::Confirm(ConfirmKind::DeleteRecording(id)));
                }
            },
        ));
        let note = Obj::label(root, "", Font::Small, TEXT_2)
            .centered_text(NOTE_W)
            .pos((SCREEN_W - NOTE_W) / 2, DETAIL_NOTE_Y);
        // A one-line label ends in "..." until it has its width: lay both
        // out once so every later `set_text` fits at once.
        title.update_layout();
        note.update_layout();
        self.title = Some(title);
        self.note = Some(note);
        let back = back_button(root, &ctx.dispatch);
        let (live, dispatch) = (self.live.clone(), ctx.dispatch.clone());
        on_click(back, move || {
            // Leaving with this recording playing or paused stops it; another
            // recording's playback is not this screen's.
            if live.borrow().view.active {
                dispatch.command(Command::StopPlayback);
            }
        });
        self.back = Some(back);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let Some(rec) = snap.files.recordings.iter().find(|rec| rec.id == self.id) else {
            // Deleted (after the confirmation) or pruned by retention: back
            // to Files, once. Until the router pops, taps send nothing for the
            // missing recording: the scrubber's handler sees nothing to play
            // (a seek only moves the local start), Send goes, Delete is
            // guarded, and Back goes too, since the kit's Back always sends
            // `UiEvent::Back` and a second one would pop past Files.
            if !self.left.replace(true) {
                *self.live.borrow_mut() = Live::default();
                self.sync_send(None);
                if let Some(back) = self.back.take() {
                    back.delete();
                }
                if let Some(dispatch) = &self.dispatch {
                    dispatch.back();
                }
            }
            return;
        };
        let title = recording_title(rec, snap);
        if let (Some(label), true) = (self.title, title != self.shown_title) {
            label.set_text(&title);
            self.shown_title = title;
        }
        let note = note_line(rec, snap.wifi.connected);
        if let (Some(label), true) = (self.note, self.shown_note.as_ref() != Some(&note)) {
            label.text_color(note.1).set_text(&note.0);
            self.shown_note = Some(note);
        }
        self.sync_send(send_action(rec.status, self.server_set));
        let (view, can_play) = {
            let mut live = self.live.borrow_mut();
            live.now_ms = snap.now_ms;
            let mut view = play_view(self.id, rec, &snap.playback, live.local_ms);
            // Until this recording plays the view shows the chosen start, so a
            // pending start is kept (it expires by time) for when it does.
            if view.active {
                let (shown, pending) = shown_position(view.position_ms, live.pending, snap.now_ms);
                view.position_ms = shown;
                live.pending = pending;
            }
            live.view = view;
            live.playable = playable(rec);
            live.guard = play_guard(snap);
            (view, live.playable)
        };
        if let Some(scrubber) = &self.scrubber {
            scrubber.set(
                view.position_ms,
                view.duration_ms,
                marks_for(self.id, rec, &snap.playback),
                view.playing,
                can_play,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::components::scrubber::{to_ms, CX, CY};
    use crate::kit::dispatch::{Dispatcher, Message, Severity, UiEvent};
    use crate::kit::icons;
    use crate::kit::obj::Obj;
    use crate::kit::route::{ConfirmKind, Route};
    use crate::kit::screen::{Screen, ScreenCtx};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl, Lvgl};
    use crate::kit::tokens::polar;
    use crate::kit::tokens::{TEXT_2, WARN};
    use crate::screens_v2::files::recording;
    use std::sync::{mpsc, Arc};
    use tokio::sync::broadcast;
    use waved_config::{ApiConfig, WavedConfig};
    use waved_state::{
        AudioState, BatteryState, Command, FilesState, McuState, MeetingState, WifiState,
    };

    const ID: Uuid = Uuid::from_u128(1);
    const OTHER: Uuid = Uuid::from_u128(2);

    fn failed() -> RecordingSummary {
        let mut rec = recording(ID, RecordingStatus::Failed, "2026-10-05T14:02:00Z", 2_537);
        rec.last_error = Some("server didn't answer".into());
        rec.marks_ms = vec![384_000];
        rec
    }

    fn playing(id: Uuid, position_ms: u32) -> PlaybackState {
        PlaybackState {
            recording: Some(id),
            playing: true,
            position_ms,
            duration_ms: 2_537_500,
            marks_ms: vec![384_000, 1_413_000],
        }
    }

    #[test]
    fn another_recordings_playback_is_not_shown_here() {
        let rec = failed();
        let view = play_view(ID, &rec, &playing(OTHER, 900_000), 12_000);
        assert_eq!(
            view,
            PlayView {
                active: false,
                playing: false,
                position_ms: 12_000,
                duration_ms: 2_537_000
            }
        );
        assert_eq!(marks_for(ID, &rec, &playing(OTHER, 0)), &[384_000]);
        let mine = play_view(ID, &rec, &playing(ID, 900_000), 12_000);
        assert_eq!(
            mine,
            PlayView {
                active: true,
                playing: true,
                position_ms: 900_000,
                duration_ms: 2_537_500
            }
        );
        assert_eq!(marks_for(ID, &rec, &playing(ID, 0)), &[384_000, 1_413_000]);
        // Before C8 reports a duration, the summary's stands in.
        let early = PlaybackState {
            duration_ms: 0,
            ..playing(ID, 0)
        };
        assert_eq!(play_view(ID, &rec, &early, 0).duration_ms, 2_537_000);
    }

    #[test]
    fn a_seek_holds_its_target_until_playback_catches_up() {
        let pending = Some(PendingSeek {
            target_ms: 600_000,
            sent_at_ms: 10_000,
        });
        // Playback still reports the old spot: keep showing the target.
        assert_eq!(shown_position(100_000, pending, 10_400), (600_000, pending));
        // Within a second of the target: follow playback and forget the seek.
        assert_eq!(shown_position(600_400, pending, 10_500), (600_400, None));
        // 1.5 s without catching up: give up and show what plays.
        assert_eq!(shown_position(100_000, pending, 11_500), (100_000, None));
        assert_eq!(shown_position(42, None, 0), (42, None));
    }

    #[test]
    fn the_middle_plays_pauses_resumes_or_explains() {
        let idle = PlayView {
            duration_ms: 1_000,
            ..PlayView::default()
        };
        assert_eq!(
            centre_tap(&idle, 0, true, None),
            CentreTap::Play { from_ms: 0 }
        );
        assert_eq!(
            centre_tap(&idle, 250, true, None),
            CentreTap::Play { from_ms: 250 }
        );
        let paused = PlayView {
            active: true,
            ..idle
        };
        assert_eq!(centre_tap(&paused, 0, true, None), CentreTap::Resume);
        let on = PlayView {
            playing: true,
            ..paused
        };
        assert_eq!(
            centre_tap(&on, 0, true, Some(BUSY_RECORDING)),
            CentreTap::Pause
        );
        assert_eq!(
            centre_tap(&idle, 0, true, Some(BUSY_CALL)),
            CentreTap::Refuse(BUSY_CALL)
        );
        assert_eq!(centre_tap(&idle, 0, false, None), CentreTap::Nothing);
    }

    #[test]
    fn play_is_refused_while_recording_or_in_a_call() {
        let mut snap = UiSnapshot::default();
        assert_eq!(play_guard(&snap), None);
        snap = UiSnapshot {
            audio: AudioState {
                call_off_hook: true,
                ..AudioState::default()
            },
            ..UiSnapshot::default()
        };
        assert_eq!(play_guard(&snap), Some(BUSY_CALL));
        snap = UiSnapshot {
            audio: AudioState {
                call_off_hook: true,
                ..AudioState::default()
            },
            meeting: MeetingState {
                recording: true,
                ..MeetingState::default()
            },
            ..UiSnapshot::default()
        };
        assert_eq!(play_guard(&snap), Some(BUSY_RECORDING));
    }

    #[test]
    fn the_right_button_follows_the_upload_state() {
        use RecordingStatus::*;
        assert_eq!(send_action(LocalOnly, true), Some(SendAction::Send));
        assert_eq!(send_action(Failed, true), Some(SendAction::SendAgain));
        for status in [Uploaded, Waiting, Uploading, Damaged, AwaitingDecision] {
            assert_eq!(send_action(status, true), None, "{status:?}");
        }
        assert_eq!(
            send_action(Failed, false),
            None,
            "no server, nothing to send to"
        );
        assert_eq!(
            (SendAction::Send.icon(), SendAction::Send.label()),
            (icons::SEND, "Send")
        );
        assert_eq!(
            (SendAction::SendAgain.icon(), SendAction::SendAgain.label()),
            (icons::REFRESH, "Send again")
        );
    }

    #[test]
    fn the_note_names_the_state_or_the_failure() {
        assert_eq!(
            note_line(&failed(), true),
            ("Upload failed: server didn't answer".into(), WARN)
        );
        let mut bare = failed();
        bare.last_error = Some("  ".into());
        assert_eq!(note_line(&bare, true), ("Upload failed".into(), WARN));
        let mut damaged = failed();
        damaged.status = RecordingStatus::Damaged;
        damaged.last_error = Some("audio file missing".into());
        assert_eq!(
            note_line(&damaged, true),
            ("File damaged: audio file missing".into(), WARN)
        );
        assert!(!playable(&damaged));
        let mut kept = failed();
        kept.status = RecordingStatus::LocalOnly;
        assert_eq!(note_line(&kept, true), ("Kept on Wave".into(), TEXT_2));
        kept.status = RecordingStatus::Waiting;
        assert_eq!(
            note_line(&kept, false),
            ("Waiting for Wi-Fi".into(), TEXT_2)
        );
        assert!(playable(&kept));
        kept.duration_secs = 0;
        assert!(!playable(&kept), "an empty file has nothing to play");
    }

    const ENDPOINT: &str = "https://notes.example.com/hook";

    struct Rig {
        screen: FileDetailScreen,
        root: Obj,
        bus: broadcast::Receiver<Command>,
        ui: mpsc::Receiver<UiEvent>,
    }

    impl Rig {
        fn commands(&mut self) -> Vec<Command> {
            std::iter::from_fn(|| self.bus.try_recv().ok()).collect()
        }

        fn events(&self) -> Vec<UiEvent> {
            self.ui.try_iter().collect()
        }

        fn tap_centre(&self) {
            let scrubber = self.screen.scrubber.as_ref().unwrap();
            scrubber.press(CX, CY);
            scrubber.release();
        }

        fn tap_ring(&self, deg: f32) {
            let scrubber = self.screen.scrubber.as_ref().unwrap();
            let (x, y) = polar(CX, CY, 60.0, deg);
            scrubber.press(x, y);
            scrubber.release();
        }

        fn caption(&self) -> String {
            self.screen.scrubber.as_ref().unwrap().time().text()
        }
    }

    fn rig(lv: &mut Lvgl, snap: &UiSnapshot, endpoint: &str) -> Rig {
        let root = lv.screen();
        let (dispatch, bus, ui) = Dispatcher::test();
        let config = WavedConfig {
            api: ApiConfig {
                endpoint: endpoint.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let ctx = ScreenCtx {
            dispatch,
            config: Arc::new(config),
        };
        let mut screen = FileDetailScreen::new(ID);
        screen.build(root, &ctx, snap);
        Rig {
            screen,
            root,
            bus,
            ui,
        }
    }

    /// file-detail.png: the failed 14:02 recording playing at 14:52 of 42:17.
    fn detail_snap() -> UiSnapshot {
        let rec = RecordingSummary {
            marks_ms: vec![384_000, 1_413_000],
            ..failed()
        };
        UiSnapshot {
            mcu: McuState {
                usb_connected: true,
                ..Default::default()
            },
            wifi: WifiState {
                connected: true,
                ..Default::default()
            },
            clock: "15:21".into(),
            battery: BatteryState {
                level: 79,
                ..Default::default()
            },
            now_unix_s: 1_791_213_660, // 2026-10-05 15:21 UTC
            now_ms: 50_000,
            files: FilesState {
                recordings: vec![rec],
                ..Default::default()
            },
            playback: PlaybackState {
                recording: Some(ID),
                playing: true,
                position_ms: 892_000,
                duration_ms: 2_537_000,
                marks_ms: vec![384_000, 1_413_000],
            },
            ..Default::default()
        }
    }

    /// The 10:30 recording kept on Wave (18 min, one mark), not playing.
    fn kept_snap() -> UiSnapshot {
        let rec = RecordingSummary {
            marks_ms: vec![300_000],
            ..recording(
                ID,
                RecordingStatus::LocalOnly,
                "2026-10-05T10:30:00Z",
                1_080,
            )
        };
        UiSnapshot {
            files: FilesState {
                recordings: vec![rec],
                ..Default::default()
            },
            playback: PlaybackState::default(),
            ..detail_snap()
        }
    }

    #[test]
    fn file_detail_matches_the_mockup() {
        lvgl(|lv| {
            let r = rig(lv, &detail_snap(), ENDPOINT);
            assert_eq!(r.screen.title.unwrap().text(), "Today, 14:02");
            assert_eq!(
                r.screen.note.unwrap().text(),
                "Upload failed: server didn't answer"
            );
            let scrubber = r.screen.scrubber.as_ref().unwrap();
            assert_eq!(scrubber.time().text(), "14:52 / 42:17");
            assert_eq!(scrubber.icon().text(), icons::PAUSE);
            assert_eq!(
                r.screen.send.as_ref().map(|(a, _)| *a),
                Some(SendAction::SendAgain)
            );
            assert_golden("file_detail", &lv.render());
            assert_targets_ok(r.root);
        });
    }

    #[test]
    fn a_kept_recording_offers_send_and_plays_from_the_start() {
        lvgl(|lv| {
            let mut r = rig(lv, &kept_snap(), ENDPOINT);
            assert_eq!(
                r.screen.send.as_ref().map(|(a, _)| *a),
                Some(SendAction::Send)
            );
            assert_eq!(r.screen.note.unwrap().text(), "Kept on Wave");
            let scrubber = r.screen.scrubber.as_ref().unwrap();
            assert_eq!(scrubber.time().text(), "0:00 / 18:00");
            assert_eq!(scrubber.icon().text(), icons::PLAY);
            assert_eq!(
                scrubber.tick_count(),
                1,
                "summary marks show before playback"
            );
            assert_golden("file_detail_kept", &lv.render());
            assert_targets_ok(r.root);
            r.tap_centre();
            let sent = r.commands();
            assert!(
                matches!(sent.as_slice(), [Command::PlayRecording(id)] if *id == ID),
                "{sent:?}"
            );
        });
    }

    #[test]
    fn a_start_chosen_before_playing_is_sent_after_play() {
        lvgl(|lv| {
            let mut r = rig(lv, &kept_snap(), ENDPOINT);
            // 270° lands on exact pixels and is far from the mark at 100°.
            r.tap_ring(270.0);
            assert!(
                r.commands().is_empty(),
                "not playing yet: only the start moves"
            );
            r.tap_centre();
            let three_quarters = to_ms(0.75, 1_080_000);
            let sent = r.commands();
            assert!(
                matches!(sent.as_slice(), [Command::PlayRecording(id), Command::SeekPlayback(ms)] if *id == ID && *ms == three_quarters),
                "{sent:?}"
            );
        });
    }

    #[test]
    fn the_chosen_start_holds_the_knob_until_playback_gets_there() {
        lvgl(|lv| {
            let mut snap = kept_snap();
            let mut r = rig(lv, &snap, ENDPOINT);
            r.tap_ring(270.0);
            r.tap_centre();
            r.commands();
            // A frame before the player starts: still the chosen start.
            snap.now_ms += 100;
            r.screen.update(&snap);
            assert_eq!(r.caption(), "13:30 / 18:00");
            // The player starts at 0 and has not seeked yet: no flicker to 0:00.
            snap.now_ms += 200;
            snap.playback = PlaybackState {
                recording: Some(ID),
                playing: true,
                position_ms: 0,
                duration_ms: 1_080_000,
                marks_ms: vec![300_000],
            };
            r.screen.update(&snap);
            assert_eq!(r.caption(), "13:30 / 18:00");
            assert_eq!(
                r.screen.scrubber.as_ref().unwrap().icon().text(),
                icons::PAUSE
            );
            // Once playback is there, the caption follows it.
            snap.now_ms += 200;
            snap.playback.position_ms = 811_000;
            r.screen.update(&snap);
            assert_eq!(r.caption(), "13:31 / 18:00");
        });
    }

    #[test]
    fn playing_from_the_start_holds_nothing() {
        lvgl(|lv| {
            let mut snap = kept_snap();
            let mut r = rig(lv, &snap, ENDPOINT);
            r.tap_centre();
            snap.now_ms += 300;
            snap.playback = PlaybackState {
                recording: Some(ID),
                playing: true,
                position_ms: 4_000,
                duration_ms: 1_080_000,
                marks_ms: vec![300_000],
            };
            r.screen.update(&snap);
            assert_eq!(r.caption(), "0:04 / 18:00");
        });
    }

    #[test]
    fn while_playing_the_middle_pauses_and_the_ring_seeks() {
        lvgl(|lv| {
            let mut snap = detail_snap();
            let mut r = rig(lv, &snap, ENDPOINT);
            r.tap_centre();
            r.tap_ring(180.0);
            let half = to_ms(0.5, 2_537_000);
            let sent = r.commands();
            assert!(
                matches!(sent.as_slice(), [Command::PausePlayback, Command::SeekPlayback(ms)] if *ms == half),
                "{sent:?}"
            );
            // Playback still reports 14:52: the caption keeps the target.
            snap.now_ms += 300;
            r.screen.update(&snap);
            assert_eq!(r.caption(), "21:08 / 42:17");
            // Paused on this recording: the middle resumes.
            snap.playback.playing = false;
            snap.playback.position_ms = half;
            r.screen.update(&snap);
            r.tap_centre();
            assert!(matches!(r.commands().as_slice(), [Command::ResumePlayback]));
        });
    }

    #[test]
    fn play_waits_for_the_recording_to_end() {
        lvgl(|lv| {
            let snap = UiSnapshot {
                meeting: MeetingState {
                    recording: true,
                    ..Default::default()
                },
                ..kept_snap()
            };
            let mut r = rig(lv, &snap, ENDPOINT);
            r.tap_centre();
            assert!(r.commands().is_empty());
            assert_eq!(
                r.events(),
                vec![UiEvent::Show(Message::new(Severity::Info, BUSY_RECORDING))]
            );
        });
    }

    #[test]
    fn send_again_queues_the_upload_and_says_so() {
        lvgl(|lv| {
            let mut r = rig(lv, &detail_snap(), ENDPOINT);
            let send = r.screen.send.as_ref().unwrap().1.obj();
            lv.click(send);
            assert!(matches!(r.commands().as_slice(), [Command::RetryUpload(id)] if *id == ID));
            assert_eq!(
                r.events(),
                vec![UiEvent::Show(Message::new(Severity::Info, SENDING))]
            );
        });
    }

    #[test]
    fn the_send_button_leaves_once_the_upload_starts() {
        lvgl(|lv| {
            let mut snap = detail_snap();
            let mut r = rig(lv, &snap, ENDPOINT);
            let label_count = r.root.child_count();
            snap.files.recordings[0].status = RecordingStatus::Uploading;
            r.screen.update(&snap);
            assert!(r.screen.send.is_none());
            // The circle and its label are both gone.
            assert_eq!(r.root.child_count(), label_count - 2);
            assert_eq!(r.screen.note.unwrap().text(), "Sending");
        });
    }

    #[test]
    fn without_a_server_there_is_nothing_to_send() {
        lvgl(|lv| {
            let r = rig(lv, &detail_snap(), "");
            assert!(r.screen.send.is_none());
            let r = rig(lv, &detail_snap(), "   ");
            assert!(r.screen.send.is_none(), "a blank endpoint is no server");
        });
    }

    #[test]
    fn delete_asks_first() {
        lvgl(|lv| {
            let mut r = rig(lv, &detail_snap(), ENDPOINT);
            lv.click(r.screen.delete.as_ref().unwrap().obj());
            assert_eq!(
                r.events(),
                vec![UiEvent::Navigate(Route::Confirm(
                    ConfirmKind::DeleteRecording(ID)
                ))]
            );
            assert!(
                r.commands().is_empty(),
                "the detail never deletes by itself"
            );
        });
    }

    #[test]
    fn back_stops_this_recordings_playback() {
        lvgl(|lv| {
            let mut r = rig(lv, &detail_snap(), ENDPOINT);
            lv.click(r.screen.back.unwrap());
            assert_eq!(r.events(), vec![UiEvent::Back]);
            assert!(matches!(r.commands().as_slice(), [Command::StopPlayback]));
            // Another recording's playback is left alone.
            let mut snap = detail_snap();
            snap.playback.recording = Some(OTHER);
            let mut r = rig(lv, &snap, ENDPOINT);
            lv.click(r.screen.back.unwrap());
            assert_eq!(r.events(), vec![UiEvent::Back]);
            assert!(r.commands().is_empty());
        });
    }

    #[test]
    fn a_deleted_recording_leaves_the_detail_once() {
        lvgl(|lv| {
            let mut snap = detail_snap();
            let mut r = rig(lv, &snap, ENDPOINT);
            snap.files.recordings.clear();
            r.screen.update(&snap);
            r.screen.update(&snap);
            // Taps in the frames before the router pops: nothing more goes out
            // for the missing recording, and Back is not sent twice.
            r.tap_centre();
            r.tap_ring(90.0);
            r.tap_ring(270.0);
            for index in 0..r.root.child_count() {
                let obj = r.root.child(index as i32);
                if obj.is_clickable() && !obj.is_hidden() {
                    lv.click(obj);
                }
            }
            assert!(r.screen.back.is_none() && r.screen.send.is_none());
            assert_eq!(r.events(), vec![UiEvent::Back]);
            assert!(r.commands().is_empty());
        });
    }

    #[test]
    fn a_detail_for_a_missing_recording_goes_back_without_panicking() {
        lvgl(|lv| {
            let snap = UiSnapshot {
                files: FilesState::default(),
                ..detail_snap()
            };
            let mut r = rig(lv, &snap, ENDPOINT);
            r.tap_centre();
            assert_eq!(r.events(), vec![UiEvent::Back]);
            assert!(r.commands().is_empty());
        });
    }

    #[test]
    fn a_long_failure_reason_stays_inside_the_note_box() {
        lvgl(|lv| {
            let mut snap = detail_snap();
            snap.files.recordings[0].last_error = Some(
                "server refused it (413 code) because the request body was far larger than allowed"
                    .into(),
            );
            let r = rig(lv, &snap, ENDPOINT);
            let note = r.screen.note.unwrap();
            note.update_layout();
            let a = note.coords();
            assert!(a.x1 >= 50 && a.x2 <= 349, "{a:?}");
            assert_eq!(a.y1, DETAIL_NOTE_Y);
            assert_eq!(a.y2 - a.y1 + 1, Font::Small.line_height(), "one line");
            assert!(note.text().ends_with("..."), "{:?}", note.text());
        });
    }
}
