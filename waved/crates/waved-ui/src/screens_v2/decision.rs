//! After stop (A3, docs/mockups/png/decision.png): "Send this recording?"
//! with Keep, Send and Delete on the arc where Mark, Stop and Pause were.
//! Shown while `MeetingState::pending_decision` is set ("Ask me each time").
//! The service keeps the file when nobody answers (D4, `ask_deadline_secs`,
//! C7); this screen only shows the countdown. Delete asks first, here.

use crate::kit::components::buttons::{BottomSpot, RoundButton, Tone};
use crate::kit::components::row::{confirm_dialog, ButtonRow, ConfirmSpec};
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::{Dispatcher, Severity};
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, SCREEN_H, SCREEN_W, TEXT, TEXT_2, TEXT_3, TITLE_Y};
use crate::screens_v2::main_parts::{format_countdown, format_size, format_timer, server_host};
use crate::screens_v2::messages::copy;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, PendingDecision};

/// docs/mockups/index.html #decision. The 48 px duration (CSS top 62) is
/// placed by its digits like `REC_TIMER_Y` (CSS top + 11).
const DURATION_Y: i32 = 73;
const DETAIL_Y: i32 = 116;
const SERVER_Y: i32 = 134;
const COUNTDOWN_Y: i32 = 152;
/// The detail lines can carry a long host: 280 px, truncated, centred.
const LINE_X: i32 = 60;
const LINE_W: i32 = 280;

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionView {
    pub title: &'static str,
    pub duration: String,
    pub detail: String,
    /// "To notes.example.com", or `None` without a server.
    pub server: Option<String>,
}

pub fn decision_view(decision: &PendingDecision) -> DecisionView {
    let server = server_host(&decision.endpoint);
    DecisionView {
        title: if server.is_some() {
            "Send this recording?"
        } else {
            "Keep this recording?"
        },
        duration: format_timer(decision.duration_secs),
        detail: decision_detail(decision.size_bytes, decision.speakers, decision.marks),
        server: server.map(|host| format!("To {host}")),
    }
}

/// "184 MB, 3 speakers, 4 marks"; unknown (zero) parts are left out.
pub fn decision_detail(size_bytes: u64, speakers: u8, marks: u32) -> String {
    let mut parts = vec![format_size(size_bytes)];
    match speakers {
        0 => {}
        1 => parts.push("1 speaker".into()),
        n => parts.push(format!("{n} speakers")),
    }
    match marks {
        0 => {}
        1 => parts.push("1 mark".into()),
        n => parts.push(format!("{n} marks")),
    }
    parts.join(", ")
}

pub fn countdown_text(secs: Option<u32>) -> Option<String> {
    secs.map(|secs| format!("Keeps it on Wave in {}", format_countdown(secs)))
}

/// Keep, Send or the confirmed Delete: the first choice wins.
fn choice(
    dispatch: &Dispatcher,
    decided: &Rc<Cell<bool>>,
    command: Command,
    severity: Severity,
    text: &'static str,
) -> impl FnMut() + 'static {
    let (dispatch, decided) = (dispatch.clone(), decided.clone());
    move || {
        if !decided.replace(true) {
            dispatch.command(command.clone());
            dispatch.show(severity, text);
        }
    }
}

#[derive(Default)]
pub struct DecisionScreen {
    parts: Option<Parts>,
}

#[cfg_attr(not(test), allow(dead_code))] // layers, title and the confirm row are read only by tests
struct Parts {
    status: StatusArea,
    title: Obj,
    /// The question and its buttons, and the delete confirmation: two
    /// full-screen layers, one visible at a time.
    main: Obj,
    confirm: Obj,
    countdown: Obj,
    keep: Option<RoundButton>,
    send: Option<RoundButton>,
    delete: Option<RoundButton>,
    confirm_row: Option<ButtonRow>,
    decided: Rc<Cell<bool>>,
    locked: bool,
    shown_countdown: Option<Option<u32>>,
}

impl Screen for DecisionScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        // Round buttons need the root or a full-screen container without a layout.
        let layer = |hidden: bool| {
            Obj::plain(root)
                .rect(Rect::new(0, 0, SCREEN_W, SCREEN_H))
                .hidden(hidden)
        };
        let (main, confirm) = (layer(false), layer(true));
        let decision = snap.meeting.pending_decision.clone();
        let view = decision.as_ref().map(decision_view);
        let has_server = view.as_ref().is_some_and(|view| view.server.is_some());
        let title = Obj::label(
            main,
            view.as_ref()
                .map_or("Send this recording?", |view| view.title),
            Font::Title,
            TEXT,
        )
        .centered_text(SCREEN_W)
        .pos(0, TITLE_Y);
        let countdown = Obj::label(main, "", Font::Micro, TEXT_3)
            .centered_text(LINE_W)
            .pos(LINE_X, if has_server { COUNTDOWN_Y } else { SERVER_Y })
            .hidden(true);
        let decided = Rc::new(Cell::new(false));
        let (mut keep, mut send, mut delete, mut confirm_row) = (None, None, None, None);
        if let (Some(decision), Some(view)) = (&decision, &view) {
            Obj::label(main, &view.duration, Font::Num48, TEXT)
                .centered_text(SCREEN_W)
                .pos(0, DURATION_Y);
            Obj::label(main, &view.detail, Font::Small, TEXT_2)
                .centered_text(LINE_W)
                .pos(LINE_X, DETAIL_Y);
            if let Some(server) = &view.server {
                Obj::label(main, server, Font::Small, TEXT_3)
                    .centered_text(LINE_W)
                    .pos(LINE_X, SERVER_Y);
            }
            let id = decision.id;
            keep = Some(RoundButton::new(
                main,
                BottomSpot::Left,
                icons::DEVICE,
                "Keep",
                Tone::Neutral,
                choice(
                    &ctx.dispatch,
                    &decided,
                    Command::SaveRecording(id),
                    Severity::Success,
                    copy::KEPT,
                ),
            ));
            if has_server {
                send = Some(RoundButton::new(
                    main,
                    BottomSpot::Main,
                    icons::SEND,
                    "Send",
                    Tone::Live,
                    choice(
                        &ctx.dispatch,
                        &decided,
                        Command::SendRecording(id),
                        Severity::Info,
                        copy::SENDING,
                    ),
                ));
            }
            delete = Some({
                let decided = decided.clone();
                RoundButton::new(
                    main,
                    BottomSpot::Right,
                    icons::TRASH,
                    "Delete",
                    Tone::RecText,
                    move || {
                        if !decided.get() {
                            main.hidden(true);
                            confirm.hidden(false);
                        }
                    },
                )
            });
            let spec = ConfirmSpec {
                icon: icons::TRASH,
                title: "Delete recording?".into(),
                body: format!(
                    "The {} recording is erased from Wave and can't be recovered.",
                    view.duration
                ),
                confirm: "Delete".into(),
                tone: Tone::RecText,
            };
            confirm_row = Some(confirm_dialog(
                confirm,
                &spec,
                choice(
                    &ctx.dispatch,
                    &decided,
                    Command::DiscardRecording(id),
                    Severity::Success,
                    copy::DELETED,
                ),
                {
                    let decided = decided.clone();
                    move || {
                        // Once Delete is confirmed the folder is going; the
                        // question must not come back with greyed buttons.
                        if !decided.get() {
                            confirm.hidden(true);
                            main.hidden(false);
                        }
                    }
                },
            ));
        }
        // Last, so its messages draw over both layers.
        let status = StatusArea::plain(root);
        self.parts = Some(Parts {
            status,
            title,
            main,
            confirm,
            countdown,
            keep,
            send,
            delete,
            confirm_row,
            decided,
            locked: false,
            shown_countdown: None,
        });
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        let Some(p) = self.parts.as_mut() else {
            return;
        };
        p.status.update(snap);
        let secs = snap.meeting.ask_deadline_secs;
        if p.shown_countdown != Some(secs) {
            match countdown_text(secs) {
                Some(text) => p.countdown.hidden(false).set_text(&text),
                None => {
                    p.countdown.hidden(true);
                }
            }
            p.shown_countdown = Some(secs);
        }
        if p.decided.get() && !p.locked {
            for button in [&p.keep, &p.send, &p.delete].into_iter().flatten() {
                button.set_enabled(false);
            }
            p.locked = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::{Message, UiEvent};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::main_parts::fixtures::{ctx, events, sent, snap};
    use uuid::Uuid;

    const ID: u128 = 42;

    /// decision.png: 42:17, 184 MB, 3 speakers, 4 marks, 52 s left.
    fn pending(endpoint: &str) -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "15:14".into();
        snap.battery.level = 79;
        snap.meeting.pending_decision = Some(PendingDecision {
            id: Uuid::from_u128(ID),
            audio_path: "/mnt/udisk/recordings/x/audio.wav".into(),
            duration_secs: 2_537,
            size_bytes: 184_000_000,
            endpoint: endpoint.into(),
            speakers: 3,
            marks: 4,
        });
        snap.meeting.ask_deadline_secs = Some(52);
        snap
    }

    #[test]
    fn decision_view_reads_the_pending_recording() {
        let snap = pending("https://user:secret@notes.example.com/hook");
        assert_eq!(
            decision_view(snap.meeting.pending_decision.as_ref().unwrap()),
            DecisionView {
                title: "Send this recording?",
                duration: "42:17".into(),
                detail: "184 MB, 3 speakers, 4 marks".into(),
                server: Some("To notes.example.com".into()),
            }
        );
        assert_eq!(decision_detail(2_400_000, 1, 0), "2 MB, 1 speaker");
        assert_eq!(decision_detail(2_400_000, 0, 1), "2 MB, 1 mark");
        assert_eq!(decision_detail(2_400_000, 0, 0), "2 MB");
        assert_eq!(
            countdown_text(Some(52)).as_deref(),
            Some("Keeps it on Wave in 0:52")
        );
        assert_eq!(countdown_text(None), None);
        let local = pending("");
        let view = decision_view(local.meeting.pending_decision.as_ref().unwrap());
        assert_eq!((view.title, view.server), ("Keep this recording?", None));
    }

    #[test]
    fn decision_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            DecisionScreen::default().build(root, &ctx, &pending("https://notes.example.com/hook"));
            assert_golden("decision", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn without_a_server_only_keep_and_delete_show() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &pending(""));
            let p = screen.parts.as_ref().unwrap();
            assert!(p.send.is_none() && p.keep.is_some() && p.delete.is_some());
            p.title.update_layout();
            assert_eq!(p.title.text(), "Keep this recording?");
            p.countdown.update_layout();
            assert_eq!(p.countdown.coords().y1, SERVER_Y, "the countdown moves up");
            assert_golden("decision_no_server", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn a_decision_is_sent_once_and_the_buttons_lock() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let snap = pending("https://notes.example.com/hook");
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &snap);
            let (keep, send, delete) = {
                let p = screen.parts.as_ref().unwrap();
                (
                    p.keep.as_ref().unwrap().obj(),
                    p.send.as_ref().unwrap().obj(),
                    p.delete.as_ref().unwrap().obj(),
                )
            };
            lv.click(send);
            lv.click(keep);
            lv.click(delete);
            let id = Uuid::from_u128(ID);
            assert_eq!(sent(&mut bus), [format!("SendRecording({id:?})")]);
            assert_eq!(
                events(&ui),
                [UiEvent::Show(Message::new(Severity::Info, copy::SENDING))]
            );
            assert!(
                screen.parts.as_ref().unwrap().confirm.is_hidden(),
                "Delete no longer asks"
            );
            screen.update(&snap);
            for button in [keep, send, delete] {
                assert!(!button.is_clickable(), "locked after the choice");
            }
        });
    }

    #[test]
    fn delete_asks_first_and_cancel_returns() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &pending("https://notes.example.com/hook"));
            let p = screen.parts.as_ref().unwrap();
            let (main, confirm, delete) = (p.main, p.confirm, p.delete.as_ref().unwrap().obj());
            let row = p.confirm_row.as_ref().unwrap();
            let (cancel, confirm_delete) = (row.obj(0), row.obj(1));
            lv.click(delete);
            assert!(main.is_hidden() && !confirm.is_hidden());
            assert_golden("decision_delete", &lv.render());
            assert_targets_ok(root);
            lv.click(cancel);
            assert!(!main.is_hidden() && confirm.is_hidden());
            assert!(sent(&mut bus).is_empty(), "cancel deletes nothing");
            lv.click(delete);
            lv.click(confirm_delete);
            let id = Uuid::from_u128(ID);
            assert_eq!(sent(&mut bus), [format!("DiscardRecording({id:?})")]);
            assert_eq!(
                events(&ui),
                [UiEvent::Show(Message::new(
                    Severity::Success,
                    copy::DELETED
                ))]
            );
        });
    }

    #[test]
    fn cancel_after_a_confirmed_delete_does_nothing() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, mut bus, ui) = ctx();
            let snap = pending("https://notes.example.com/hook");
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &snap);
            let p = screen.parts.as_ref().unwrap();
            let (main, confirm, delete) = (p.main, p.confirm, p.delete.as_ref().unwrap().obj());
            let row = p.confirm_row.as_ref().unwrap();
            let (cancel, confirm_delete) = (row.obj(0), row.obj(1));
            lv.click(delete);
            lv.click(confirm_delete);
            lv.click(cancel);
            lv.click(confirm_delete);
            assert!(
                main.is_hidden() && !confirm.is_hidden(),
                "the question screen does not come back"
            );
            let id = Uuid::from_u128(ID);
            assert_eq!(sent(&mut bus), [format!("DiscardRecording({id:?})")]);
            assert_eq!(events(&ui).len(), 1, "one message");
        });
    }

    #[test]
    fn the_countdown_follows_the_service_and_hides_without_one() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut snap = pending("https://notes.example.com/hook");
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &snap);
            let countdown = screen.parts.as_ref().unwrap().countdown;
            countdown.update_layout();
            assert_eq!(countdown.text(), "Keeps it on Wave in 0:52");
            snap.meeting.ask_deadline_secs = Some(51);
            screen.update(&snap);
            countdown.update_layout();
            assert_eq!(countdown.text(), "Keeps it on Wave in 0:51");
            snap.meeting.ask_deadline_secs = None;
            screen.update(&snap);
            assert!(countdown.is_hidden());
        });
    }

    #[test]
    fn a_screen_without_a_pending_decision_has_no_actions() {
        lvgl(|lv| {
            let root = lv.screen();
            let (ctx, _bus, _ui) = ctx();
            let mut screen = DecisionScreen::default();
            screen.build(root, &ctx, &snap());
            let p = screen.parts.as_ref().unwrap();
            assert!(p.keep.is_none() && p.send.is_none() && p.delete.is_none());
            assert_targets_ok(root);
        });
    }
}
