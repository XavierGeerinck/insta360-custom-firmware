//! Date and time, and Time zone (B6). Owned by workstream B; see kit/README.md, "Registering a screen".

use crate::kit::components::buttons::back_button;
use crate::kit::components::controls::{RadioList, RadioOption, SwitchRow, ValueRow};
use crate::kit::components::status::StatusArea;
use crate::kit::fonts::Font;
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, RADIO_TOP, SCREEN_W, TEXT, TEXT_2, TITLE_Y};
use crate::screens_v2::volume::BigNumber;
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, CLOCK_VALID_AFTER_UNIX_S};

const TIME_TOP: i32 = 62;
const DATE_Y: i32 = 114;
const ZONE_ROW: Rect = Rect::new(60, 140, 280, 44);
const CLOCK_ROW: Rect = Rect::new(60, 190, 280, 44);
const SUFFIX_GAP: i32 = 6;

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Zones offered on the device; a zone set on the setup page is added to them.
pub const ZONES: [&str; 29] = [
    "UTC",
    "Europe/London",
    "Europe/Dublin",
    "Europe/Lisbon",
    "Europe/Brussels",
    "Europe/Amsterdam",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/Madrid",
    "Europe/Rome",
    "Europe/Zurich",
    "Europe/Stockholm",
    "Europe/Warsaw",
    "Europe/Athens",
    "Europe/Helsinki",
    "Europe/Istanbul",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Singapore",
    "Asia/Hong_Kong",
    "Asia/Tokyo",
    "Australia/Sydney",
    "Pacific/Auckland",
    "America/Sao_Paulo",
    "America/New_York",
    "America/Toronto",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
];

/// (year, month 1-12, day 1-31) of a day count since 1970-01-01 (H. Hinnant).
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days since 1970-01-01 of a civil date (H. Hinnant); Uploads parses its timestamps with it.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "Saturday 4 October".
pub fn date_line(local_s: i64) -> String {
    let days = local_s.div_euclid(86_400);
    let (_, month, day) = civil(days);
    let weekday = WEEKDAYS[(days + 4).rem_euclid(7) as usize];
    let month_name = MONTHS[month as usize - 1];
    format!("{weekday} {day} {month_name}")
}

/// ("14:37", "") or ("2:37", "pm").
pub fn clock_parts(local_s: i64, clock_24h: bool) -> (String, &'static str) {
    let minutes = local_s.rem_euclid(86_400) / 60;
    let (hour, minute) = (minutes / 60, minutes % 60);
    if clock_24h {
        (format!("{hour:02}:{minute:02}"), "")
    } else {
        let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        (
            format!("{hour12}:{minute:02}"),
            if hour < 12 { "am" } else { "pm" },
        )
    }
}

/// The city of an IANA zone: "Europe/Brussels" reads "Brussels".
pub fn zone_label(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

/// The choices with a zone set elsewhere (setup page) added first, and the
/// index of `current`.
pub fn zone_choices(current: &str) -> (Vec<String>, usize) {
    let mut zones: Vec<String> = ZONES.iter().map(|z| z.to_string()).collect();
    match zones.iter().position(|z| z == current) {
        Some(index) => (zones, index),
        None => {
            zones.insert(0, current.to_string());
            (zones, 0)
        }
    }
}

/// What the time and date read: dashes until the clock is believable.
fn time_and_date(snap: &UiSnapshot) -> (String, &'static str, String) {
    if snap.now_unix_s < CLOCK_VALID_AFTER_UNIX_S {
        return ("--:--".into(), "", "--".into());
    }
    let local = snap.local_unix_s();
    let (digits, suffix) = clock_parts(local, snap.settings.clock_24h);
    (digits, suffix, date_line(local))
}

#[derive(Default)]
pub struct DateTimeScreen {
    status: Option<StatusArea>,
    time: Option<BigNumber>,
    date: Option<Obj>,
    zone: Option<ValueRow>,
    clock: Option<SwitchRow>,
    /// The value a tap on the switch sent, until the service reports it.
    pending_24h: Rc<Cell<Option<bool>>>,
    shown: Option<(String, &'static str, String, String)>,
}

impl Screen for DateTimeScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Date and time", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.time = Some(BigNumber::new(root, TIME_TOP));
        self.date = Some(
            Obj::label(root, "", Font::Body, TEXT_2)
                .centered_text(SCREEN_W)
                .pos(0, DATE_Y),
        );
        let dispatch = ctx.dispatch.clone();
        self.zone = Some(ValueRow::new(
            root,
            ZONE_ROW,
            "Time zone",
            &zone_label(&snap.settings.time_zone),
            move || dispatch.navigate(Route::TimeZone),
        ));
        let (dispatch, pending) = (ctx.dispatch.clone(), self.pending_24h.clone());
        self.clock = Some(SwitchRow::new(
            root,
            CLOCK_ROW,
            "24-hour clock",
            snap.settings.clock_24h,
            move |on| {
                pending.set(Some(on));
                dispatch.command(Command::SetClock24h(on))
            },
        ));
        back_button(root, &ctx.dispatch);
        self.update(snap);
        root.update_layout();
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        // A tap moved the switch at once: ignore snapshots that still report
        // the old value. Once the service reports it (or a change made
        // elsewhere arrives) the snapshot is the truth again.
        let reported = snap.settings.clock_24h;
        let mut snap_24h = reported;
        match self.pending_24h.get() {
            Some(pending) if reported != pending => snap_24h = pending,
            _ => {
                self.pending_24h.set(None);
                if let Some(clock) = self.clock.as_ref() {
                    clock.set_on(reported);
                }
            }
        }
        let (digits, suffix, date) = {
            let mut shown = snap.clone();
            shown.settings.clock_24h = snap_24h;
            time_and_date(&shown)
        };
        let next = (digits, suffix, date, zone_label(&snap.settings.time_zone));
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        if let (Some(time), Some(date), Some(zone)) =
            (self.time.as_ref(), self.date, self.zone.as_ref())
        {
            time.set(&next.0, next.1, SUFFIX_GAP, TEXT);
            date.set_text(&next.2);
            zone.set_value(&next.3);
        }
        self.shown = Some(next);
    }
}

#[derive(Default)]
pub struct TimeZoneScreen {
    status: Option<StatusArea>,
    list: Option<RadioList>,
    zones: Vec<String>,
    seen: Option<String>,
}

impl Screen for TimeZoneScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Time zone", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        let (zones, selected) = zone_choices(&snap.settings.time_zone);
        let options = zones
            .iter()
            .map(|zone| RadioOption::new(zone_label(zone), zone.as_str()))
            .collect();
        let (dispatch, picks) = (ctx.dispatch.clone(), zones.clone());
        let list = RadioList::new(root, RADIO_TOP, options, selected, move |index| {
            dispatch.command(Command::SetTimeZone(picks[index].clone()))
        });
        list.scroll_to(selected);
        self.list = Some(list);
        self.zones = zones;
        self.seen = Some(snap.settings.time_zone.clone());
        back_button(root, &ctx.dispatch);
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        // Follow a change made elsewhere; a tap here already moved the radio.
        if self.seen.as_deref() != Some(snap.settings.time_zone.as_str()) {
            self.seen = Some(snap.settings.time_zone.clone());
            if let (Some(list), Some(index)) = (
                self.list.as_ref(),
                self.zones
                    .iter()
                    .position(|z| *z == snap.settings.time_zone),
            ) {
                list.set_selected(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    /// 12:37 UTC on Saturday 4 October 2025 is 14:37 in Brussels (UTC+2).
    fn brussels() -> UiSnapshot {
        let mut snap = snap();
        snap.clock = "14:37".into();
        snap.now_unix_s = 1_759_581_420;
        snap.system.utc_offset_min = 120;
        snap.settings.time_zone = "Europe/Brussels".into();
        snap.settings.clock_24h = true;
        snap
    }

    #[test]
    fn datetime_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut screen = DateTimeScreen::default();
            screen.build(root, &f.ctx, &brussels());
            assert_golden("screen_datetime", &lv.render());
            assert_targets_ok(root);
            assert_eq!(screen.time.as_ref().unwrap().digits().text(), "14:37");
            assert_eq!(screen.date.unwrap().text(), "Saturday 4 October");
        });
    }

    #[test]
    fn before_the_clock_is_set_time_and_date_read_dashes() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            for clock_24h in [true, false] {
                let mut snap = brussels();
                snap.now_unix_s = waved_state::CLOCK_VALID_AFTER_UNIX_S - 1;
                snap.settings.clock_24h = clock_24h;
                let mut screen = DateTimeScreen::default();
                screen.build(root, &f.ctx, &snap);
                assert_targets_ok(root);
                assert_eq!(screen.time.as_ref().unwrap().digits().text(), "--:--");
                assert_eq!(screen.date.unwrap().text(), "--");
                assert_eq!(screen.time.as_ref().unwrap().suffix_text(), "");
                // The clock arrives: the screen follows.
                screen.update(&brussels());
                assert_eq!(screen.date.unwrap().text(), "Saturday 4 October");
            }
        });
    }

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(20_365), (2025, 10, 4));
        assert_eq!(civil(19_782), (2024, 2, 29));
        for days in [-800_000, -1, 0, 59, 19_782, 20_365, 2_932_896] {
            let (y, m, d) = civil(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(date_line(1_759_588_620), "Saturday 4 October");
        assert_eq!(date_line(0), "Thursday 1 January");
    }

    #[test]
    fn clocks_follow_the_format() {
        let local = 1_759_588_620; // 14:37
        assert_eq!(clock_parts(local, true), ("14:37".to_string(), ""));
        assert_eq!(clock_parts(local, false), ("2:37".to_string(), "pm"));
        assert_eq!(clock_parts(0, false), ("12:00".to_string(), "am"));
        assert_eq!(clock_parts(-60, true), ("23:59".to_string(), ""));
    }

    #[test]
    fn the_switch_and_the_zone_row_work() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = DateTimeScreen::default();
            screen.build(root, &f.ctx, &brussels());
            lv.click(screen.clock.as_ref().unwrap().obj());
            assert!(matches!(
                f.commands().as_slice(),
                [Command::SetClock24h(false)]
            ));
            lv.click(screen.zone.as_ref().unwrap().obj());
            assert_eq!(f.events(), vec![UiEvent::Navigate(Route::TimeZone)]);
            // A stale snapshot does not flip the switch back.
            screen.update(&brussels());
            assert!(!screen.clock.as_ref().unwrap().is_on());
            // Once the service confirms, the snapshot is the truth again.
            let mut snap = brussels();
            snap.settings.clock_24h = false;
            screen.update(&snap);
            assert!(!screen.clock.as_ref().unwrap().is_on());
            assert_eq!(screen.time.as_ref().unwrap().digits().text(), "2:37");
            // A change made elsewhere is followed.
            snap.settings.clock_24h = true;
            screen.update(&snap);
            assert!(screen.clock.as_ref().unwrap().is_on());
            assert_eq!(screen.time.as_ref().unwrap().digits().text(), "14:37");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn time_zone_matches_its_radio_list() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut screen = TimeZoneScreen::default();
            screen.build(root, &f.ctx, &brussels());
            assert_golden("screen_time_zone", &lv.render());
            assert_targets_ok(root);
            let list = screen.list.as_ref().unwrap();
            assert_eq!(list.selected(), 4);
            lv.click(list.row(5));
            assert!(
                matches!(f.commands().as_slice(), [Command::SetTimeZone(z)] if z == "Europe/Amsterdam")
            );
            // A stale snapshot keeps the tapped zone; a change elsewhere is followed.
            screen.update(&brussels());
            assert_eq!(screen.list.as_ref().unwrap().selected(), 5);
            let mut snap = brussels();
            snap.settings.time_zone = "Asia/Tokyo".into();
            screen.update(&snap);
            assert_eq!(screen.list.as_ref().unwrap().selected(), 20);
        });
    }

    #[test]
    fn zones_read_as_cities() {
        assert_eq!(zone_label("Europe/Brussels"), "Brussels");
        assert_eq!(zone_label("America/New_York"), "New York");
        assert_eq!(zone_label("UTC"), "UTC");
        let (zones, index) = zone_choices("America/Argentina/Buenos_Aires");
        assert_eq!(
            (zones[0].as_str(), index),
            ("America/Argentina/Buenos_Aires", 0)
        );
        assert_eq!(zone_label(&zones[0]), "Buenos Aires");
        assert!(ZONES.iter().all(|z| waved_config::valid_time_zone(z)));
    }
}
