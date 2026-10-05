//! Pieces shared by Home, Recording, After stop and the resting screen: ring
//! looks from the dial table (docs/mockups/README.md, "Dial states"),
//! Quieter / Louder steps, and the formats for timers, sizes and the upload
//! server. `MainDial` (Task 3) puts the dial, the side buttons and the volume
//! gauge together.

use crate::kit::components::buttons::{Side, SideButton};
use crate::kit::components::dial::{Dial, RingStyle};
use crate::kit::dispatch::Dispatcher;
use crate::kit::fonts::Font;
use crate::kit::icons;
use crate::kit::obj::{text_width, Obj};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{DIAL_CX, DIAL_CY, DIAL_R, GAUGE_MS, REC_TIMER_Y, SCREEN_W, TEXT, TEXT_2};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::Command;

/// One Quieter / Louder step ("Interaction model": speaker −/+ 10 %).
pub const VOLUME_STEP: u16 = 10;
pub const VOLUME_MAX: u16 = 100;
/// From an hour on, the timer (h:mm:ss) is drawn in Num26: "1:02:03" is
/// 161 px in Num48 and would cross the ring. Num26 digits fill the top 19 px
/// of its 24 px box, so top 112 centres them on the dial's centre line (121).
pub const TIMER_HOURS_Y: i32 = 112;
/// Widest text that stays clear of the r 70 ring (stroke 3) on the rows of
/// the timer and the status line: the ring's inner chord there is ~130 px.
pub const RING_TEXT_W: i32 = 124;

/// What the dial ring shows: a style and, while someone talks, the wedge's
/// direction in whole device degrees (whole degrees keep redraws rare).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingLook {
    pub style: RingStyle,
    pub talker: Option<i16>,
}

/// The wedge, while voice is detected and the dial style follows talkers.
fn talker_if(snap: &UiSnapshot, follows: bool) -> Option<i16> {
    (follows && snap.audio.doa_active).then(|| snap.audio.doa_horizontal.round() as i16)
}

/// Home's ring: red when muted, teal while a call or the computer uses the
/// microphone, grey otherwise.
pub fn home_ring(snap: &UiSnapshot) -> RingLook {
    let style = if snap.audio.mic_muted {
        RingStyle::Muted
    } else if snap.in_call() || snap.audio.host_listening {
        RingStyle::Live
    } else {
        RingStyle::Idle
    };
    RingLook {
        style,
        talker: talker_if(snap, style == RingStyle::Live),
    }
}

/// Recording's ring: solid red when muted (the recording holds silence),
/// dashed amber while paused, dim red with the talker wedge otherwise.
pub fn recording_ring(snap: &UiSnapshot) -> RingLook {
    let style = if snap.audio.mic_muted {
        RingStyle::RecordingMuted
    } else if snap.meeting.paused {
        RingStyle::Paused
    } else {
        RingStyle::Recording
    };
    RingLook {
        style,
        talker: talker_if(snap, style == RingStyle::Recording),
    }
}

/// The next speaker volume for Quieter (`up == false`) or Louder: the
/// neighbouring multiple of 10, within 0..=100.
pub fn step_volume(volume: u16, up: bool) -> u16 {
    let volume = volume.min(VOLUME_MAX);
    if up {
        ((volume / VOLUME_STEP + 1) * VOLUME_STEP).min(VOLUME_MAX)
    } else if volume == 0 {
        0
    } else {
        (volume - 1) / VOLUME_STEP * VOLUME_STEP
    }
}

/// "42:17", or "1:02:03" from an hour on.
pub fn format_timer(secs: u32) -> String {
    let (hours, minutes, seconds) = (secs / 3_600, secs / 60 % 60, secs % 60);
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

/// The timer's font and label top: Num48 like the mockups, Num26 from an
/// hour on so h:mm:ss stays inside the ring.
pub fn timer_font(secs: u32) -> (Font, i32) {
    if secs < 3_600 {
        (Font::Num48, REC_TIMER_Y)
    } else {
        (Font::Num26, TIMER_HOURS_Y)
    }
}

/// "0:52": a countdown in minutes and seconds.
pub fn format_countdown(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// "184 MB", "1.3 GB", in decimal units like the file managers people use.
pub fn format_size(bytes: u64) -> String {
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    let megabytes = (bytes + MB / 2) / MB;
    if megabytes == 0 {
        "Under 1 MB".into()
    } else if megabytes < 1_000 {
        format!("{megabytes} MB")
    } else {
        let tenths = (bytes + GB / 20) / (GB / 10);
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}

/// The host of a webhook endpoint, for "To notes.example.com": no scheme,
/// credentials, port, path or query. `None` when there is no server.
pub fn server_host(endpoint: &str) -> Option<String> {
    let rest = endpoint.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, after)| after);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if host_port.starts_with('[') {
        host_port.split_inclusive(']').next().unwrap_or(host_port)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// The font for a line inside the ring: `preferred` when it fits
/// `RING_TEXT_W`, else Small (the label truncates beyond that).
pub fn ring_font(text: &str, preferred: Font) -> Font {
    if text_width(text, preferred) <= RING_TEXT_W {
        preferred
    } else {
        Font::Small
    }
}

/// Top of the gauge caption ("Speaker"), README "Dial content stack (gauge,
/// progress, resting)": caption 13 px at top 77.
pub const GAUGE_CAPTION_Y: i32 = 77;
/// The 20 px "%" shares the 48 px digits' baseline: 34 − 19 px below their top.
const SUFFIX_DY: i32 = 15;
const SUFFIX_GAP: i32 = 2;

/// The dial with Quieter and Louder, shared by Home and Recording: tapping
/// the dial toggles mute, the side buttons step the speaker volume, and the
/// dial turns into a volume gauge for 1.5 s after any change.
#[cfg_attr(not(test), allow(dead_code))] // side buttons are read only by test accessors
pub struct MainDial {
    dial: Dial,
    quieter: SideButton,
    louder: SideButton,
    caption: Obj,
    value: Obj,
    percent: Obj,
    /// The volume the next step starts from: the last value sent while
    /// stepping, else the snapshot's. The service applies a step a frame or
    /// more later, so stepping from the snapshot would repeat the same value.
    target: Rc<Cell<u16>>,
    /// A side button stepped since the last update.
    stepped: Rc<Cell<bool>>,
    /// Until then `target` wins over the snapshot.
    local_until_ms: u64,
    gauge_until_ms: u64,
    last_volume: Option<u16>,
    /// The gauge on screen: (volume, speaker muted).
    shown_gauge: Option<(u16, bool)>,
    /// The ring on screen; `None` forces a redraw.
    look: Option<RingLook>,
}

impl MainDial {
    pub fn new(root: Obj, dispatch: &Dispatcher, snap: &UiSnapshot) -> Self {
        let mute = dispatch.clone();
        let dial = Dial::new(root, DIAL_CX, DIAL_CY, DIAL_R, 3)
            .on_tap(move || mute.command(Command::ToggleMicMute));
        let target = Rc::new(Cell::new(snap.audio.speaker_volume.min(VOLUME_MAX)));
        let stepped = Rc::new(Cell::new(false));
        let stepper = |up: bool| {
            let (target, stepped, dispatch) = (target.clone(), stepped.clone(), dispatch.clone());
            move || {
                let next = step_volume(target.get(), up);
                if next != target.get() {
                    target.set(next);
                    dispatch.command(Command::SetSpeakerVolume(next));
                }
                // At 0 or 100 the gauge still shows, as feedback for the press.
                stepped.set(true);
            }
        };
        let quieter = SideButton::new(
            root,
            Side::Left,
            icons::VOLUME_LOW,
            "Quieter",
            stepper(false),
        );
        let louder = SideButton::new(
            root,
            Side::Right,
            icons::VOLUME_HIGH,
            "Louder",
            stepper(true),
        );
        let caption = Obj::label(root, "Speaker", Font::Small, TEXT_2)
            .centered_text(SCREEN_W)
            .pos(0, GAUGE_CAPTION_Y)
            .hidden(true);
        let value = Obj::label(root, "", Font::Num48, TEXT)
            .one_line()
            .hidden(true);
        let percent = Obj::label(root, "%", Font::Num20, TEXT_2)
            .one_line()
            .hidden(true);
        Self {
            dial,
            quieter,
            louder,
            caption,
            value,
            percent,
            target,
            stepped,
            local_until_ms: 0,
            gauge_until_ms: 0,
            last_volume: None,
            shown_gauge: None,
            look: None,
        }
    }

    /// Follow the snapshot and draw `look` whenever the gauge is not showing.
    /// Returns whether the gauge shows.
    pub fn update(&mut self, snap: &UiSnapshot, look: RingLook) -> bool {
        let now = snap.now_ms;
        let volume = snap.audio.speaker_volume.min(VOLUME_MAX);
        if self.stepped.replace(false) {
            self.local_until_ms = now + GAUGE_MS;
            self.gauge_until_ms = now + GAUGE_MS;
        } else if now >= self.local_until_ms && self.last_volume.is_some_and(|last| last != volume)
        {
            // The host, a physical button or another screen changed it.
            self.gauge_until_ms = now + GAUGE_MS;
        }
        self.last_volume = Some(volume);
        if now >= self.local_until_ms {
            self.target.set(volume);
        }
        let gauge = (now < self.gauge_until_ms).then(|| {
            let shown = if now < self.local_until_ms {
                self.target.get()
            } else {
                volume
            };
            (shown, snap.audio.speaker_muted)
        });
        if gauge != self.shown_gauge {
            match gauge {
                Some((shown, muted)) => {
                    self.dial.set_gauge(Some(shown as u8));
                    self.show_gauge(shown, muted);
                }
                None => {
                    for obj in [self.caption, self.value, self.percent] {
                        obj.hidden(true);
                    }
                    // set_gauge(None) restores the ring; redraw the look on it.
                    self.dial.set_gauge(None);
                    self.look = None;
                }
            }
            self.shown_gauge = gauge;
        }
        if gauge.is_none() && self.look != Some(look) {
            self.dial.set_ring(look.style);
            self.dial.set_talker(look.talker.map(f32::from));
            self.look = Some(look);
        }
        gauge.is_some()
    }

    /// "Speaker" over "60%", centred on the dial.
    fn show_gauge(&self, volume: u16, muted: bool) {
        let digits = volume.to_string();
        // One spare pixel, so text that fits exactly never ends in "...".
        let digits_w = text_width(&digits, Font::Num48) + 1;
        let percent_w = text_width("%", Font::Num20) + 1;
        let x = (SCREEN_W - (digits_w + SUFFIX_GAP + percent_w)) / 2;
        self.value
            .width(digits_w)
            .pos(x, REC_TIMER_Y)
            .hidden(false)
            .set_text(&digits);
        self.percent
            .width(percent_w)
            .pos(x + digits_w + SUFFIX_GAP, REC_TIMER_Y + SUFFIX_DY)
            .hidden(false);
        self.caption
            .hidden(false)
            .set_text(if muted { "Speaker muted" } else { "Speaker" });
    }

    #[cfg(test)]
    pub fn quieter(&self) -> Obj {
        self.quieter.obj()
    }

    #[cfg(test)]
    pub fn louder(&self) -> Obj {
        self.louder.obj()
    }

    #[cfg(test)]
    pub fn hit(&self) -> Obj {
        self.dial.hit().expect("the main dial is tappable")
    }

    #[cfg(test)]
    pub fn gauge_text(&self) -> String {
        self.value.text()
    }

    #[cfg(test)]
    pub fn talker_visible(&self) -> bool {
        self.dial.talker_visible()
    }
}

/// Shared fixtures for the screens_v2 tests: what the mockups show around
/// the dial (USB and Wi-Fi up, 14:32, battery 82 %).
#[cfg(test)]
pub mod fixtures {
    use crate::kit::dispatch::{Dispatcher, Message, Severity, UiEvent};
    use crate::kit::screen::ScreenCtx;
    use crate::kit::snapshot::{ActiveMessage, UiSnapshot};
    use std::sync::{mpsc, Arc};
    use tokio::sync::broadcast;
    use waved_config::WavedConfig;
    use waved_state::Command;

    pub fn ctx() -> (
        ScreenCtx,
        broadcast::Receiver<Command>,
        mpsc::Receiver<UiEvent>,
    ) {
        let (dispatch, bus, ui) = Dispatcher::test();
        let ctx = ScreenCtx {
            dispatch,
            config: Arc::new(WavedConfig::default()),
        };
        (ctx, bus, ui)
    }

    pub fn snap() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.mcu.usb_connected = true;
        snap.wifi.connected = true;
        snap.clock = "14:32".into();
        snap.battery.level = 82;
        snap.battery.voltage = 3_900;
        snap.settings.first_run_done = true;
        snap.now_ms = 10_000;
        snap
    }

    /// Commands sent so far, as their Debug text ("SetSpeakerVolume(70)").
    pub fn sent(bus: &mut broadcast::Receiver<Command>) -> Vec<String> {
        std::iter::from_fn(|| bus.try_recv().ok())
            .map(|command| format!("{command:?}"))
            .collect()
    }

    pub fn events(ui: &mpsc::Receiver<UiEvent>) -> Vec<UiEvent> {
        ui.try_iter().collect()
    }

    /// What the runtime does with `UiEvent::Show` (contract request 2).
    pub fn with_message(snap: &mut UiSnapshot, severity: Severity, text: &str) {
        let seq = snap.message.as_ref().map_or(1, |active| active.seq + 1);
        snap.message = Some(ActiveMessage {
            seq,
            message: Message::new(severity, text),
            since_ms: snap.now_ms,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testkit::lvgl;

    #[test]
    fn volume_steps_land_on_tens_inside_0_to_100() {
        assert_eq!(step_volume(60, true), 70);
        assert_eq!(step_volume(65, true), 70);
        assert_eq!(step_volume(95, true), 100);
        assert_eq!(step_volume(100, true), 100);
        assert_eq!(step_volume(60, false), 50);
        assert_eq!(step_volume(65, false), 60);
        assert_eq!(step_volume(1, false), 0);
        assert_eq!(step_volume(0, false), 0);
        assert_eq!(step_volume(250, true), 100, "host values past 100 clamp");
        assert_eq!(step_volume(250, false), 90);
    }

    #[test]
    fn timers_switch_to_hours_after_59_59() {
        assert_eq!(format_timer(0), "00:00");
        assert_eq!(format_timer(2_537), "42:17");
        assert_eq!(format_timer(3_599), "59:59");
        assert_eq!(format_timer(3_600), "1:00:00");
        assert_eq!(format_timer(3_723), "1:02:03");
        assert_eq!(format_timer(360_000), "100:00:00");
        assert_eq!(timer_font(3_599), (Font::Num48, REC_TIMER_Y));
        assert_eq!(timer_font(3_600), (Font::Num26, TIMER_HOURS_Y));
        assert_eq!(format_countdown(52), "0:52");
        assert_eq!(format_countdown(60), "1:00");
    }

    #[test]
    fn every_timer_fits_inside_the_ring() {
        lvgl(|_| {
            for secs in [0, 2_537, 3_599, 3_600, 35_999, 359_999] {
                let (font, _) = timer_font(secs);
                let width = text_width(&format_timer(secs), font);
                assert!(width <= RING_TEXT_W, "{secs} s is {width} px");
            }
        });
    }

    #[test]
    fn sizes_read_in_decimal_megabytes_then_gigabytes() {
        assert_eq!(format_size(184_000_000), "184 MB");
        assert_eq!(format_size(1_400_000), "1 MB");
        assert_eq!(format_size(120_000), "Under 1 MB");
        assert_eq!(format_size(999_600_000), "1.0 GB");
        assert_eq!(format_size(1_250_000_000), "1.3 GB");
    }

    #[test]
    fn the_server_line_shows_only_the_host() {
        for (endpoint, host) in [
            ("https://notes.example.com/hook", Some("notes.example.com")),
            (
                "https://user:secret@notes.example.com:8443/hook?token=x",
                Some("notes.example.com"),
            ),
            ("http://[fd00::1]:8080/hook", Some("[fd00::1]")),
            ("notes.example.com/hook", Some("notes.example.com")),
            ("", None),
            ("   ", None),
            ("https://", None),
            ("https://user@/x", None),
        ] {
            assert_eq!(server_host(endpoint).as_deref(), host, "{endpoint:?}");
        }
    }

    #[test]
    fn ring_looks_follow_the_dial_table() {
        let mut snap = UiSnapshot::default();
        assert_eq!(
            home_ring(&snap),
            RingLook {
                style: RingStyle::Idle,
                talker: None
            }
        );
        snap.audio.doa_active = true;
        snap.audio.doa_horizontal = 306.6;
        assert_eq!(
            home_ring(&snap).talker,
            None,
            "no wedge while nothing listens"
        );
        snap.audio.host_listening = true;
        assert_eq!(
            home_ring(&snap),
            RingLook {
                style: RingStyle::Live,
                talker: Some(307)
            }
        );
        snap.audio.host_listening = false;
        snap.audio.call_off_hook = true;
        assert_eq!(home_ring(&snap).style, RingStyle::Live, "in a call");
        snap.audio.mic_muted = true;
        assert_eq!(
            home_ring(&snap),
            RingLook {
                style: RingStyle::Muted,
                talker: None
            }
        );

        let mut rec = UiSnapshot::default();
        rec.meeting.recording = true;
        rec.audio.doa_active = true;
        rec.audio.doa_horizontal = 60.0;
        assert_eq!(
            recording_ring(&rec),
            RingLook {
                style: RingStyle::Recording,
                talker: Some(60)
            }
        );
        rec.audio.doa_active = false;
        assert_eq!(recording_ring(&rec).talker, None, "no wedge without voice");
        rec.meeting.paused = true;
        assert_eq!(
            recording_ring(&rec),
            RingLook {
                style: RingStyle::Paused,
                talker: None
            }
        );
        rec.audio.mic_muted = true;
        assert_eq!(
            recording_ring(&rec).style,
            RingStyle::RecordingMuted,
            "mute wins over pause"
        );
    }

    #[test]
    fn long_ring_lines_drop_to_small_type() {
        lvgl(|_| {
            assert_eq!(ring_font("In a call", Font::Body), Font::Body);
            assert_eq!(ring_font("Connected by Bluetooth", Font::Body), Font::Small);
        });
    }
}

#[cfg(test)]
mod dial_tests {
    use super::fixtures::{sent, snap};
    use super::*;
    use crate::bindings::{
        lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT, lv_event_code_t_LV_EVENT_PRESSED,
    };
    use crate::kit::dispatch::Dispatcher;
    use crate::kit::testkit::lvgl;
    use crate::kit::tokens::GAUGE_MS;

    #[test]
    fn holding_louder_climbs_from_the_local_target_not_the_stale_snapshot() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, mut bus, _ui) = Dispatcher::test();
            let mut now = snap();
            now.audio.speaker_volume = 60;
            let mut dial = MainDial::new(root, &dispatch, &now);
            dial.update(&now, home_ring(&now));
            lv.send(dial.louder(), lv_event_code_t_LV_EVENT_PRESSED);
            for _ in 0..4 {
                now.now_ms += 300;
                dial.update(&now, home_ring(&now)); // the service has not caught up: still 60
                lv.advance(300);
                lv.send(dial.louder(), lv_event_code_t_LV_EVENT_LONG_PRESSED_REPEAT);
            }
            assert_eq!(
                sent(&mut bus),
                [
                    "SetSpeakerVolume(70)",
                    "SetSpeakerVolume(80)",
                    "SetSpeakerVolume(90)",
                    "SetSpeakerVolume(100)"
                ]
            );
        });
    }

    #[test]
    fn the_gauge_shows_for_1_5_s_after_a_step_and_after_a_host_change() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, _bus, _ui) = Dispatcher::test();
            let look = RingLook {
                style: RingStyle::Live,
                talker: Some(307),
            };
            let mut now = snap();
            now.audio.speaker_volume = 60;
            let mut dial = MainDial::new(root, &dispatch, &now);
            assert!(!dial.update(&now, look), "no gauge when the screen opens");
            assert!(dial.talker_visible());
            lv.click(dial.louder());
            assert!(dial.update(&now, look));
            assert_eq!(dial.gauge_text(), "70");
            assert!(!dial.talker_visible(), "the gauge replaces the ring");
            now.now_ms += GAUGE_MS - 1;
            now.audio.speaker_volume = 70; // the service caught up
            assert!(dial.update(&now, look));
            now.now_ms += 1;
            assert!(!dial.update(&now, look), "gone 1.5 s after the last step");
            assert!(dial.talker_visible(), "the ring and wedge return");
            now.now_ms += 500;
            now.audio.speaker_volume = 40; // the host moved it
            assert!(dial.update(&now, look));
            assert_eq!(dial.gauge_text(), "40");
            now.now_ms += GAUGE_MS;
            assert!(!dial.update(&now, look));
        });
    }

    #[test]
    fn quieter_steps_down_and_the_dial_toggles_mute() {
        lvgl(|lv| {
            let root = lv.screen();
            let (dispatch, mut bus, _ui) = Dispatcher::test();
            let mut now = snap();
            now.audio.speaker_volume = 65;
            let mut dial = MainDial::new(root, &dispatch, &now);
            dial.update(&now, home_ring(&now));
            lv.click(dial.quieter());
            lv.click(dial.hit());
            assert_eq!(sent(&mut bus), ["SetSpeakerVolume(60)", "ToggleMicMute"]);
        });
    }
}
