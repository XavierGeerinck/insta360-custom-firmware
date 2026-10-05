//! Uploads (B8, uploads.png): counts first, then one action. "Retry" only
//! appears when something failed, and names how many. Times are relative
//! instead of raw ISO timestamps. A failure's reason lives on that recording
//! in Files.

use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::ValueRow;
use crate::kit::components::status::StatusArea;
use crate::kit::dispatch::Severity;
use crate::kit::event::on_click;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::time::parse_timestamp;
use crate::kit::tokens::{Rect, RAISED, SCREEN_W, SURFACE, TEXT, TEXT_2, TEXT_3, TITLE_Y, WARN};
use crate::screens_v2::menu::upload_mode_name;
use crate::screens_v2::messages::copy;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, RecordingStatus, CLOCK_VALID_AFTER_UNIX_S};

const TEXT_X: i32 = 50;
const TEXT_W: i32 = 300;
const HOST_Y: i32 = 62;
const LAST_Y: i32 = 80;
const MODE_ROW: Rect = Rect::new(76, 100, 248, 40);
const COLUMN_X: [i32; 3] = [80, 162, 244];
const COLUMN_W: i32 = 76;
const COUNT_Y: i32 = 150;
const COUNT_LABEL_Y: i32 = 174;
const RETRY: Rect = Rect::new(120, 192, 160, 44);
/// A timestamp up to this far in the future reads "just now" (clock skew).
const SKEW_S: i64 = 60;

/// The server's host (and port) for the panel. Never shows the userinfo
/// (`user:password@`), path, query or fragment, which can carry secrets.
pub fn host(endpoint: &str) -> Option<String> {
    let endpoint = endpoint.trim();
    let rest = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    (!host.is_empty()).then(|| host.to_string())
}

/// "12 min ago"; `None` when `then` lies in the future (the clock isn't set).
pub fn relative(now_s: i64, then_s: i64) -> Option<String> {
    let age = now_s - then_s;
    if age < -SKEW_S {
        return None;
    }
    Some(match age {
        a if a < 60 => "just now".into(),
        a if a < 3_600 => format!("{} min ago", a / 60),
        a if a < 86_400 => format!("{} h ago", a / 3_600),
        a if a < 2 * 86_400 => "yesterday".into(),
        a => format!("{} days ago", a / 86_400),
    })
}

pub fn last_sent_line(last: Option<&str>, now_s: i64) -> String {
    match last {
        None => "Nothing sent yet".into(),
        Some(text) => {
            let ago = if now_s < CLOCK_VALID_AFTER_UNIX_S {
                None
            } else {
                parse_timestamp(text).and_then(|then| relative(now_s, then))
            };
            match ago {
                Some(ago) => format!("Last sent {ago}"),
                None => "Last upload time unknown".into(),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub sent: usize,
    pub waiting: u32,
    pub failed: u32,
}

pub fn counts(snap: &UiSnapshot) -> Counts {
    Counts {
        sent: snap
            .files
            .recordings
            .iter()
            .filter(|r| r.status == RecordingStatus::Uploaded)
            .count(),
        waiting: snap.meeting.pending_uploads,
        failed: snap.meeting.failed_uploads,
    }
}

pub fn retry_label(failed: u32) -> String {
    format!("Retry {failed} failed")
}

#[derive(Default)]
pub struct UploadsScreen {
    status: Option<StatusArea>,
    configured: bool,
    /// Read by the tests only.
    #[cfg(test)]
    host: Option<Obj>,
    last: Option<Obj>,
    mode: Option<ValueRow>,
    counts: Vec<(Obj, Obj)>,
    retry: Option<Obj>,
    retry_text: Option<Obj>,
    failed: Rc<Cell<u32>>,
    shown: Option<(Counts, String, &'static str)>,
}

impl Screen for UploadsScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Uploads", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        // api.endpoint only changes through the setup page, which restarts waved.
        let server = host(&ctx.config.api.endpoint);
        self.configured = server.is_some();
        let host_label = Obj::label(
            root,
            server.as_deref().unwrap_or("No upload server"),
            Font::Body,
            TEXT_2,
        )
        .centered_text(TEXT_W)
        .pos(TEXT_X, HOST_Y);
        #[cfg(test)]
        {
            self.host = Some(host_label);
        }
        #[cfg(not(test))]
        let _ = host_label;
        self.last = Some(
            Obj::label(root, "", Font::Small, TEXT_3)
                .centered_text(TEXT_W)
                .pos(TEXT_X, LAST_Y),
        );
        let dispatch = ctx.dispatch.clone();
        let mode = ValueRow::new(
            root,
            MODE_ROW,
            "After a recording",
            upload_mode_name(snap.settings.upload_mode),
            move || dispatch.navigate(Route::UploadMode),
        );
        mode.obj().ext_click((44 - MODE_ROW.h) / 2);
        self.mode = Some(mode);
        for (&x, word) in COLUMN_X.iter().zip(["sent", "waiting", "failed"]) {
            let value = Obj::label(root, "", Font::Num26, TEXT)
                .centered_text(COLUMN_W)
                .pos(x, COUNT_Y);
            let label = Obj::label(root, word, Font::Micro, TEXT_2)
                .centered_text(COLUMN_W)
                .pos(x, COUNT_LABEL_Y);
            value.hidden(!self.configured);
            label.hidden(!self.configured);
            self.counts.push((value, label));
        }
        let retry = Obj::plain(root)
            .rect(RETRY)
            .bg(SURFACE)
            .radius(RETRY.h / 2)
            .pressed_bg(RAISED)
            .clickable(true)
            .row(6);
        Obj::label(retry, icons::REFRESH, Font::Icon16, TEXT);
        self.retry_text = Some(Obj::label(retry, "", Font::Body, TEXT));
        let (dispatch, failed) = (ctx.dispatch.clone(), self.failed.clone());
        on_click(retry, move || {
            let n = failed.get();
            dispatch.command(Command::RetryFailedUploads);
            dispatch.show(Severity::Info, copy::retrying_uploads(n));
        });
        self.retry = Some(retry);
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        let now = counts(snap);
        self.failed.set(now.failed);
        let last = if self.configured {
            last_sent_line(
                snap.meeting.last_upload_success_at.as_deref(),
                snap.now_unix_s,
            )
        } else {
            "Add one on the setup page".into()
        };
        let next = (now, last, upload_mode_name(snap.settings.upload_mode));
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        if let (Some(last_label), Some(mode)) = (self.last, self.mode.as_ref()) {
            last_label.set_text(&next.1);
            mode.set_value(next.2);
        }
        let values = [
            now.sent.to_string(),
            now.waiting.to_string(),
            now.failed.to_string(),
        ];
        for (index, ((value, label), text)) in self.counts.iter().zip(&values).enumerate() {
            value.set_text(text);
            let alert = index == 2 && now.failed > 0;
            value.text_color(if alert { WARN } else { TEXT });
            label.text_color(if alert { WARN } else { TEXT_2 });
        }
        if let (Some(retry), Some(text)) = (self.retry, self.retry_text) {
            retry.hidden(!self.configured || now.failed == 0);
            text.set_text(&retry_label(now.failed));
        }
        self.shown = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::{Message, Severity, UiEvent};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture_with, recording, snap};
    use waved_config::WavedConfig;
    use waved_state::{RecordingStatus, UploadMode};

    fn config(endpoint: &str) -> WavedConfig {
        let mut config = WavedConfig::default();
        config.api.endpoint = endpoint.into();
        config
    }

    fn uploads_snap() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "15:22".into();
        snap.battery.level = 79;
        snap.now_unix_s = 1_759_588_320;
        snap.meeting.last_upload_success_at = Some("2025-10-04T14:20:00+00:00".into());
        snap.meeting.pending_uploads = 1;
        snap.meeting.failed_uploads = 1;
        snap.files.recordings = (0..38)
            .map(|_| recording(RecordingStatus::Uploaded))
            .collect();
        snap.settings.upload_mode = UploadMode::Ask;
        snap
    }

    #[test]
    fn uploads_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config("https://notes.example.com/hook"));
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &uploads_snap());
            assert_golden("screen_uploads", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.last.unwrap().text(), "Last sent 12 min ago");
        });
    }

    #[test]
    fn retry_appears_only_when_something_failed() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture_with(config("https://notes.example.com/hook"));
            let mut snap = uploads_snap();
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            let retry = screen.retry.unwrap();
            assert!(!retry.is_hidden());
            lv.click(retry);
            assert!(matches!(
                f.commands().as_slice(),
                [Command::RetryFailedUploads]
            ));
            assert_eq!(
                f.events(),
                vec![UiEvent::Show(Message::new(
                    Severity::Info,
                    copy::retrying_uploads(1)
                ))]
            );
            snap.meeting.failed_uploads = 0;
            screen.update(&snap);
            assert!(screen.retry.unwrap().is_hidden());
        });
    }

    #[test]
    fn no_server_says_where_to_add_one() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config(""));
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &uploads_snap());
            assert_golden("screen_uploads_no_server", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.host.unwrap().text(), "No upload server");
            assert!(screen.counts.iter().all(|(value, _)| value.is_hidden()));
            assert!(screen.retry.unwrap().is_hidden());
        });
    }

    #[test]
    fn a_long_host_stays_on_one_line() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture_with(config(
                "https://recordings.meeting-rooms.example-company.internal/hooks/wave",
            ));
            let mut screen = UploadsScreen::default();
            screen.build(root, &f.ctx, &uploads_snap());
            root.update_layout();
            assert_targets_ok(root);
            let host = screen.host.unwrap();
            assert!(host.text().ends_with("..."), "{}", host.text());
            assert_eq!(
                host.coords().y2 - host.coords().y1 + 1,
                Font::Body.line_height()
            );
        });
    }

    #[test]
    fn timestamps_parse_and_read_relatively() {
        let t = 1_759_587_600; // 2025-10-04 14:20:00 UTC
        assert_eq!(parse_timestamp("2025-10-04T14:20:00+00:00"), Some(t));
        assert_eq!(
            parse_timestamp("2025-10-04T16:20:00.123456789+02:00"),
            Some(t)
        );
        assert_eq!(parse_timestamp("2025-10-04T14:20:00Z"), Some(t));
        // What the database restores after a reboot (SQLite datetime('now')).
        assert_eq!(parse_timestamp("2025-10-04 14:20:00"), Some(t));
        assert_eq!(parse_timestamp("2025-02-30 14:20:00"), None);
        assert_eq!(parse_timestamp("2025-10-04 14:20:00 junk"), None);
        assert_eq!(relative(1_000, 970).as_deref(), Some("just now"));
        assert_eq!(
            relative(10_000, 10_000 - 720).as_deref(),
            Some("12 min ago")
        );
        assert_eq!(relative(10_000, 10_000 - 7_200).as_deref(), Some("2 h ago"));
        assert_eq!(
            relative(200_000, 200_000 - 90_000).as_deref(),
            Some("yesterday")
        );
        assert_eq!(
            relative(400_000, 400_000 - 3 * 86_400).as_deref(),
            Some("3 days ago")
        );
        assert_eq!(relative(1_000, 1_030).as_deref(), Some("just now")); // small clock skew
        assert_eq!(relative(1_000, 5_000), None); // the clock isn't set yet
        assert_eq!(last_sent_line(None, 0), "Nothing sent yet");
        // A clock that is set but not yet believable (1970 + uptime) is not trusted.
        assert_eq!(
            last_sent_line(Some("1970-01-01T00:00:10Z"), 100),
            "Last upload time unknown"
        );
        assert_eq!(
            last_sent_line(Some("2025-10-04T14:20:00Z"), 0),
            "Last upload time unknown"
        );
        // The value restored from the database after a reboot has no offset.
        assert_eq!(
            last_sent_line(Some("2025-10-04 14:20:00"), 1_759_587_600 + 720),
            "Last sent 12 min ago"
        );
        assert_eq!(
            host("https://notes.example.com/hook").as_deref(),
            Some("notes.example.com")
        );
        assert_eq!(host(""), None);
        assert_eq!(retry_label(3), "Retry 3 failed");
    }

    #[test]
    fn the_host_never_shows_credentials_or_query_strings() {
        for (endpoint, shown) in [
            (
                "https://user:pw@notes.example.com/x",
                Some("notes.example.com"),
            ),
            (
                "https://us@r:p@ss@notes.example.com:8443/x",
                Some("notes.example.com:8443"),
            ),
            ("user:pw@notes.example.com", Some("notes.example.com")),
            (
                "https://notes.example.com?token=s3cret",
                Some("notes.example.com"),
            ),
            (
                "https://notes.example.com#token=s3cret",
                Some("notes.example.com"),
            ),
            (
                "https://user:pw@notes.example.com?token=s3cret#x",
                Some("notes.example.com"),
            ),
            ("https://user:pw@/hook", None),
        ] {
            assert_eq!(host(endpoint).as_deref(), shown, "{endpoint}");
        }
    }
}
