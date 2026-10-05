//! Files (docs/mockups files.png): recordings, attention first, then newest,
//! with the free space under the title. The copy helpers here are shared
//! with the recording detail.

use crate::kit::components::buttons::back_button;
use crate::kit::components::file_row::{FileList, RowLook};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::{format_clock, UiSnapshot};
use crate::kit::time::{civil_from_days, parse_rfc3339};
use crate::kit::tokens::{LIVE, SCREEN_W, SUBTITLE_Y, TEXT, TEXT_2, TEXT_3, TITLE_Y, WARN};
#[cfg(test)]
use uuid::Uuid;
use waved_state::{RecordingStatus, RecordingSummary, CLOCK_VALID_AFTER_UNIX_S};

/// Cards built at most. Attention rows always come first, so a failed upload
/// is never cut off; older delivered recordings are pruned by retention.
pub const MAX_ROWS: usize = 50;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Today, 14:02", "Yesterday, 16:45", "29 Sep, 09:15", or "31 Dec 2025" in
/// another year; "Date unknown" when the start is missing, unreadable or
/// stamped before the clock was set.
pub fn when_label(
    started_at: Option<&str>,
    now_unix_s: i64,
    offset_min: i32,
    clock_24h: bool,
) -> String {
    let start = match started_at.and_then(parse_rfc3339) {
        Some(start) if start >= CLOCK_VALID_AFTER_UNIX_S => start,
        _ => return "Date unknown".into(),
    };
    let offset_s = i64::from(offset_min) * 60;
    let day = start.saturating_add(offset_s).div_euclid(86_400);
    let today = now_unix_s.saturating_add(offset_s).div_euclid(86_400);
    let time = format_clock(start, offset_min, clock_24h);
    match today.saturating_sub(day) {
        0 => format!("Today, {time}"),
        1 => format!("Yesterday, {time}"),
        _ => {
            let (year, month, d) = civil_from_days(day);
            let (this_year, _, _) = civil_from_days(today);
            let month = MONTHS[month as usize - 1];
            if year == this_year {
                format!("{d} {month}, {time}")
            } else {
                format!("{d} {month} {year}")
            }
        }
    }
}

/// "45 s", "42 min", "1 h 05 min" (rounded to the nearest minute).
pub fn duration_label(secs: u32) -> String {
    if secs < 60 {
        return format!("{secs} s");
    }
    let minutes = (secs + 30) / 60;
    if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    }
}

/// "3.2 GB free", "12 GB free", "850 MB free" (decimal units, rounded down so
/// the line never promises space that is not there).
pub fn free_space_label(bytes: u64) -> String {
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    if bytes >= 10 * GB {
        format!("{} GB free", bytes / GB)
    } else if bytes >= GB {
        format!("{}.{} GB free", bytes / GB, (bytes % GB) / (GB / 10))
    } else {
        format!("{} MB free", bytes / MB)
    }
}

/// The icon, its colour and the lower-case phrase for one upload state.
pub fn status_look(
    status: RecordingStatus,
    wifi_connected: bool,
) -> (&'static str, u32, &'static str) {
    match status {
        RecordingStatus::Failed => (icons::ALERT, WARN, "upload failed"),
        RecordingStatus::Damaged => (icons::ALERT, WARN, "file damaged"),
        RecordingStatus::Waiting if wifi_connected => (icons::CLOCK, TEXT_2, "waiting to send"),
        RecordingStatus::Waiting => (icons::CLOCK, TEXT_2, "waiting for Wi-Fi"),
        RecordingStatus::Uploading => (icons::SEND, TEXT_2, "sending"),
        RecordingStatus::Uploaded => (icons::CHECK_CIRCLE, LIVE, "sent"),
        RecordingStatus::LocalOnly => (icons::DEVICE, TEXT_2, "kept on Wave"),
        RecordingStatus::AwaitingDecision => (icons::CLOCK, TEXT_2, "waiting for your choice"),
    }
}

/// The phrase with its first letter capitalised ("Upload failed").
pub fn sentence(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Attention first, then the producer's newest-first order (a stable sort),
/// at most `MAX_ROWS`.
pub fn ordered(recordings: &[RecordingSummary]) -> Vec<&RecordingSummary> {
    let mut rows: Vec<&RecordingSummary> = recordings.iter().collect();
    rows.sort_by_key(|rec| !rec.status.needs_attention());
    rows.truncate(MAX_ROWS);
    rows
}

/// The recording's when label in the device's zone and clock format. The
/// list rows and the detail screen's title both use it.
pub fn recording_title(rec: &RecordingSummary, snap: &UiSnapshot) -> String {
    when_label(
        rec.started_at.as_deref(),
        snap.now_unix_s,
        snap.system.utc_offset_min,
        snap.settings.clock_24h,
    )
}

pub fn row_look(rec: &RecordingSummary, snap: &UiSnapshot) -> RowLook {
    let (icon, icon_color, words) = status_look(rec.status, snap.wifi.connected);
    RowLook {
        id: rec.id,
        icon,
        icon_color,
        title: recording_title(rec, snap),
        line: format!("{}, {words}", duration_label(rec.duration_secs)),
    }
}

pub fn rows_for(snap: &UiSnapshot) -> Vec<RowLook> {
    ordered(&snap.files.recordings)
        .into_iter()
        .map(|rec| row_look(rec, snap))
        .collect()
}

#[derive(Default)]
pub struct FilesScreen {
    status: Option<StatusArea>,
    free: Option<Obj>,
    list: Option<FileList>,
    /// The free line as last drawn (text, colour, shown), to skip restyling.
    free_shown: Option<(String, u32, bool)>,
}

impl Screen for FilesScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Files", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.free = Some(
            Obj::label(root, "", Font::Micro, TEXT_3)
                .centered_text(SCREEN_W)
                .pos(0, SUBTITLE_Y),
        );
        let dispatch = ctx.dispatch.clone();
        self.list = Some(FileList::new(root, move |id| {
            dispatch.navigate(Route::FileDetail(id))
        }));
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        if let Some(free) = self.free {
            // Before the first storage report free_bytes is 0 and low_space
            // false: show nothing rather than "0 MB free".
            let shown = snap.files.free_bytes > 0 || snap.files.low_space;
            let colour = if snap.files.low_space { WARN } else { TEXT_3 };
            let next = (free_space_label(snap.files.free_bytes), colour, shown);
            if self.free_shown.as_ref() != Some(&next) {
                free.hidden(!next.2).text_color(next.1).set_text(&next.0);
                self.free_shown = Some(next);
            }
        }
        if let Some(list) = self.list.as_mut() {
            list.set_rows(rows_for(snap));
        }
    }
}

/// A recording fixture for this module's tests and the detail's.
#[cfg(test)]
pub(crate) fn recording(
    id: Uuid,
    status: RecordingStatus,
    started: &str,
    secs: u32,
) -> RecordingSummary {
    RecordingSummary {
        id,
        started_at: Some(started.into()),
        duration_secs: secs,
        size_bytes: u64::from(secs) * 32_000,
        status,
        marks: 0,
        marks_ms: Vec::new(),
        last_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::icons;
    use crate::kit::tokens::{LIVE, TEXT_2, WARN};
    use waved_state::FilesState;

    /// 2026-10-05 15:20:00 UTC, the clock in files.png.
    const NOW: i64 = 1_791_213_600;

    #[test]
    fn when_label_names_the_day_in_local_time() {
        let at = |s: &str| when_label(Some(s), NOW, 0, true);
        assert_eq!(at("2026-10-05T14:02:00Z"), "Today, 14:02");
        assert_eq!(at("2026-10-04T16:45:00+00:00"), "Yesterday, 16:45");
        assert_eq!(at("2026-09-29T09:15:00Z"), "29 Sep, 09:15");
        assert_eq!(at("2025-12-31T23:30:00Z"), "31 Dec 2025");
        // Two hours east: 23:30 UTC yesterday is 01:30 today.
        assert_eq!(
            when_label(Some("2026-10-04T23:30:00Z"), NOW, 120, true),
            "Today, 01:30"
        );
        assert_eq!(
            when_label(Some("2026-10-05T14:02:00Z"), NOW, 0, false),
            "Today, 2:02 pm"
        );
    }

    #[test]
    fn when_label_survives_missing_and_odd_times() {
        assert_eq!(when_label(None, NOW, 0, true), "Date unknown");
        assert_eq!(when_label(Some("not a time"), NOW, 0, true), "Date unknown");
        assert_eq!(
            when_label(Some("2026-10-05T14:02:31.5+00:00"), NOW, 0, true),
            "Today, 14:02"
        );
        // A start after "now" (the clock was corrected) still gets a date, not "Today".
        assert_eq!(
            when_label(Some("2026-10-07T10:00:00Z"), NOW, 0, true),
            "7 Oct, 10:00"
        );
        // Extreme values do not panic.
        when_label(Some("0001-01-01T00:00:00Z"), i64::MAX / 4, -720, false);
    }

    #[test]
    fn a_start_before_the_clock_was_set_is_date_unknown() {
        // Stamped by an unsynchronised clock: five minutes after the epoch.
        assert_eq!(
            when_label(Some("1970-01-01T00:05:00Z"), NOW, 0, true),
            "Date unknown"
        );
        assert_eq!(
            when_label(Some("0001-01-01T00:00:00Z"), NOW, 0, true),
            "Date unknown"
        );
        // The first valid instant still gets a date.
        assert_eq!(
            when_label(Some("2020-01-01T00:00:00Z"), NOW, 0, true),
            "1 Jan 2020"
        );
    }

    #[test]
    fn durations_read_like_the_mockup() {
        assert_eq!(duration_label(0), "0 s");
        assert_eq!(duration_label(59), "59 s");
        assert_eq!(duration_label(2_537), "42 min");
        assert_eq!(duration_label(1_080), "18 min");
        assert_eq!(duration_label(3_900), "1 h 05 min");
        assert_eq!(duration_label(3_599), "1 h 00 min");
    }

    #[test]
    fn free_space_rounds_down() {
        assert_eq!(free_space_label(3_200_000_000), "3.2 GB free");
        assert_eq!(free_space_label(3_299_999_999), "3.2 GB free");
        assert_eq!(free_space_label(12_700_000_000), "12 GB free");
        assert_eq!(free_space_label(999_999_999), "999 MB free");
        assert_eq!(free_space_label(0), "0 MB free");
    }

    #[test]
    fn every_status_has_one_icon_colour_and_phrase() {
        use RecordingStatus::*;
        assert_eq!(
            status_look(Failed, true),
            (icons::ALERT, WARN, "upload failed")
        );
        assert_eq!(
            status_look(Damaged, true),
            (icons::ALERT, WARN, "file damaged")
        );
        assert_eq!(
            status_look(Waiting, false),
            (icons::CLOCK, TEXT_2, "waiting for Wi-Fi")
        );
        assert_eq!(
            status_look(Waiting, true),
            (icons::CLOCK, TEXT_2, "waiting to send")
        );
        assert_eq!(
            status_look(Uploading, true),
            (icons::SEND, TEXT_2, "sending")
        );
        assert_eq!(
            status_look(Uploaded, true),
            (icons::CHECK_CIRCLE, LIVE, "sent")
        );
        assert_eq!(
            status_look(LocalOnly, true),
            (icons::DEVICE, TEXT_2, "kept on Wave")
        );
        assert_eq!(
            status_look(AwaitingDecision, true),
            (icons::CLOCK, TEXT_2, "waiting for your choice")
        );
        assert_eq!(sentence("upload failed"), "Upload failed");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn attention_first_then_newest_and_capped() {
        use RecordingStatus::*;
        let ids: Vec<Uuid> = (0..60).map(|n| Uuid::from_u128(n + 1)).collect();
        // Producer order is newest first; two failures sit further down.
        let mut list: Vec<RecordingSummary> = ids
            .iter()
            .map(|id| recording(*id, Uploaded, "2026-10-05T10:00:00Z", 60))
            .collect();
        list[3].status = Failed;
        list[40].status = Damaged;
        let rows = ordered(&list);
        assert_eq!(rows.len(), MAX_ROWS);
        assert_eq!((rows[0].id, rows[1].id), (ids[3], ids[40]));
        // The rest keep the producer's order.
        assert_eq!(
            (rows[2].id, rows[3].id, rows[4].id),
            (ids[0], ids[1], ids[2])
        );
        assert_eq!(rows[5].id, ids[4]);
    }

    fn fixture(rec: &RecordingSummary) -> UiSnapshot {
        UiSnapshot {
            now_unix_s: NOW,
            files: FilesState {
                recordings: vec![rec.clone()],
                ..FilesState::default()
            },
            ..UiSnapshot::default()
        }
    }

    #[test]
    fn rows_combine_time_duration_and_state() {
        let rec = recording(
            Uuid::from_u128(7),
            RecordingStatus::Failed,
            "2026-10-05T14:02:00Z",
            2_537,
        );
        let snap = fixture(&rec);
        let row = row_look(&rec, &snap);
        assert_eq!(row.id, rec.id);
        assert_eq!((row.icon, row.icon_color), (icons::ALERT, WARN));
        assert_eq!(row.title, "Today, 14:02");
        assert_eq!(row.line, "42 min, upload failed");
        assert_eq!(rows_for(&snap), vec![row]);
    }

    #[test]
    fn the_title_follows_the_zone_and_clock_setting() {
        let rec = recording(
            Uuid::from_u128(8),
            RecordingStatus::Uploaded,
            "2026-10-04T23:30:00Z",
            60,
        );
        let mut snap = fixture(&rec);
        assert_eq!(recording_title(&rec, &snap), "Yesterday, 23:30");
        snap.system.utc_offset_min = 120;
        snap.settings.clock_24h = false;
        assert_eq!(recording_title(&rec, &snap), "Today, 1:30 am");
        assert_eq!(row_look(&rec, &snap).title, "Today, 1:30 am");
    }

    use crate::kit::dispatch::{Dispatcher, UiEvent};
    use crate::kit::route::Route;
    use crate::kit::screen::{Screen, ScreenCtx};
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use std::sync::Arc;
    use waved_config::WavedConfig;

    /// files.png: three recordings at 15:20, 3.2 GB free. Wi-Fi is down so the
    /// second row reads "waiting for Wi-Fi" as in the mockup (which also draws
    /// a Wi-Fi glyph in the status; the kit hides it while disconnected).
    fn files_snap() -> UiSnapshot {
        let mut failed = recording(
            Uuid::from_u128(1),
            RecordingStatus::Failed,
            "2026-10-05T14:02:00Z",
            2_537,
        );
        failed.last_error = Some("server didn't answer".into());
        UiSnapshot {
            clock: "15:20".into(),
            now_unix_s: NOW,
            mcu: waved_state::McuState {
                usb_connected: true,
                ..Default::default()
            },
            battery: waved_state::BatteryState {
                level: 79,
                ..Default::default()
            },
            files: FilesState {
                free_bytes: 3_200_000_000,
                recordings: vec![
                    failed,
                    recording(
                        Uuid::from_u128(2),
                        RecordingStatus::Waiting,
                        "2026-10-05T10:30:00Z",
                        1_080,
                    ),
                    recording(
                        Uuid::from_u128(3),
                        RecordingStatus::Uploaded,
                        "2026-10-04T16:45:00Z",
                        3_900,
                    ),
                ],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn ctx(dispatch: Dispatcher) -> ScreenCtx {
        ScreenCtx {
            dispatch,
            config: Arc::new(WavedConfig::default()),
        }
    }

    #[test]
    fn files_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let mut screen = FilesScreen::default();
            screen.build(root, &ctx(dispatch), &files_snap());
            let list = screen.list.as_ref().unwrap();
            assert_eq!(list.len(), 3);
            assert_eq!(list.card(0).child(1).child(0).text(), "Today, 14:02");
            assert_eq!(
                list.card(1).child(1).child(1).text(),
                "18 min, waiting for Wi-Fi"
            );
            assert_eq!(list.card(2).child(1).child(1).text(), "1 h 05 min, sent");
            let free = screen.free.unwrap();
            assert_eq!(free.text(), "3.2 GB free");
            assert_golden("files", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn an_empty_device_says_how_to_start() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let mut snap = files_snap();
            snap.files.recordings.clear();
            let mut screen = FilesScreen::default();
            screen.build(root, &ctx(dispatch), &snap);
            assert!(!screen.list.as_ref().unwrap().empty_state().is_hidden());
            assert_golden("files_empty", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn low_space_turns_the_free_line_amber() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let mut snap = files_snap();
            snap.files.free_bytes = 850_000_000;
            snap.files.low_space = true;
            snap.files.recordings.truncate(1);
            let mut screen = FilesScreen::default();
            screen.build(root, &ctx(dispatch), &snap);
            assert_eq!(screen.free.unwrap().text(), "850 MB free");
            assert_golden("files_low_space", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn the_free_line_waits_for_the_first_storage_report() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let mut snap = files_snap();
            snap.files = FilesState::default();
            let mut screen = FilesScreen::default();
            screen.build(root, &ctx(dispatch), &snap);
            assert!(
                screen.free.unwrap().is_hidden(),
                "no report yet: no '0 MB free'"
            );
            snap.files.low_space = true;
            screen.update(&snap);
            assert!(!screen.free.unwrap().is_hidden(), "a full disk is reported");
            assert_eq!(screen.free.unwrap().text(), "0 MB free");
        });
    }

    #[test]
    fn tapping_a_card_opens_its_detail_and_updates_follow_the_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, ui) = Dispatcher::test();
            let mut snap = files_snap();
            let mut screen = FilesScreen::default();
            screen.build(root, &ctx(dispatch), &snap);
            lv.click(screen.list.as_ref().unwrap().card(2));
            assert_eq!(
                ui.try_recv().unwrap(),
                UiEvent::Navigate(Route::FileDetail(Uuid::from_u128(3)))
            );
            // An upload finishing re-sorts: the failed row becomes "sending".
            snap.files.recordings[0].status = RecordingStatus::Uploading;
            screen.update(&snap);
            let list = screen.list.as_ref().unwrap();
            assert_eq!(list.card(0).child(1).child(1).text(), "42 min, sending");
        });
    }
}
