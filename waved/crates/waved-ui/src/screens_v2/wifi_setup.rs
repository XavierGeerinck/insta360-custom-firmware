//! Wi-Fi setup (B7, wifi-setup.png). Scan the code to join Wave's setup
//! network, pick a network on the page that opens, and Wave restarts and
//! connects. The steps are numbered because they are a sequence. The manual
//! details sit under the code, so a long password can't wrap into it.

use crate::kit::components::buttons::text_button;
use crate::kit::components::status::StatusArea;
use crate::kit::components::wifi::Qr;
use crate::kit::dispatch::Dispatcher;
use crate::kit::fonts::Font;
use crate::kit::obj::{text_width, Obj, SIZE_CONTENT};
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use crate::kit::tokens::{Rect, BACK, LIVE, SCREEN_W, TEXT, TEXT_2, TITLE_Y};
use std::cell::Cell;
use std::rc::Rc;
use waved_state::{Command, ProvisioningState};

const QR: Rect = Rect::new(68, 68, 120, 120);
const QR_QUIET: i32 = 8;
const STEPS: (i32, i32) = (202, 72);
const STEPS_W: i32 = 140;
const STEP_TEXT_W: i32 = 124;
const STEP_GAP: i32 = 8;
const LINE1_Y: i32 = 198;
const LINE2_Y: i32 = 216;
const LINE_GAP: i32 = 4;
const SSID_CAP: i32 = 160;
const PASS_CAP: i32 = 140;
const HOST_CAP: i32 = 160;
const MESSAGE: (i32, i32) = (70, 100);
const MESSAGE_W: i32 = 260;
const RETRY: Rect = Rect::new(120, 192, 160, 44);
const STEP_TEXT: [&str; 3] = [
    "Scan with your phone",
    "Pick your network on the page that opens",
    "Wave restarts and connects",
];

// Temporary duplicate of the legacy `screens/wifi_provisioning.rs` helpers; the
// legacy copy goes when the runtime switches to screens_v2.
fn wifi_qr_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | ';' | ',' | ':' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// The standard Wi-Fi join code for the setup network.
pub fn wifi_qr_payload(ssid: &str, passphrase: &str) -> String {
    format!(
        "WIFI:T:WPA;S:{};P:{};H:false;;",
        wifi_qr_escape(ssid),
        wifi_qr_escape(passphrase)
    )
}

#[derive(Clone, Debug, PartialEq)]
pub enum SetupView {
    Starting,
    Code {
        ssid: String,
        passphrase: String,
        host: String,
    },
    Busy(String),
    Failed(String),
}

/// Whether opening the screen starts provisioning: not while it already runs.
pub fn should_start(state: &ProvisioningState) -> bool {
    matches!(
        state,
        ProvisioningState::Idle
            | ProvisioningState::Connected { .. }
            | ProvisioningState::Failed { .. }
    )
}

/// What to show. `starting` is set from sending the start command until the
/// service reports progress, so a stale Idle / Connected / Failed isn't shown.
pub fn view(state: &ProvisioningState, starting: bool) -> SetupView {
    if starting && should_start(state) {
        return SetupView::Starting;
    }
    match state {
        ProvisioningState::Idle | ProvisioningState::Scanning => SetupView::Starting,
        ProvisioningState::ApActive {
            ssid,
            passphrase,
            url,
        } => SetupView::Code {
            ssid: ssid.clone(),
            passphrase: passphrase.clone(),
            host: url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string(),
        },
        ProvisioningState::Connecting { ssid } => SetupView::Busy(format!("Connecting to {ssid}")),
        ProvisioningState::Saving => SetupView::Busy("Saving the settings".into()),
        ProvisioningState::Reconnecting => {
            SetupView::Busy("Reconnecting to the saved network".into())
        }
        ProvisioningState::Restarting => {
            SetupView::Busy("Settings saved. Wave is restarting.".into())
        }
        ProvisioningState::Connected { ssid } => SetupView::Busy(format!("Connected to {ssid}")),
        ProvisioningState::Failed { reason } => SetupView::Failed(reason.clone()),
    }
}

fn start(dispatch: &Dispatcher, started: &Cell<bool>) {
    dispatch.command(Command::OpenWifiProvisioning);
    started.set(true);
}

/// A label capped at `cap` px, one line, ending in "..." when longer.
fn capped(parent: Obj, font: Font, hex: u32) -> Obj {
    Obj::label(parent, "", font, hex).one_line()
}

fn set_capped(label: Obj, text: &str, cap: i32) {
    label
        .width(text_width(text, Font::Small).min(cap))
        .set_text(text);
}

#[derive(Default)]
pub struct WifiSetupScreen {
    status: Option<StatusArea>,
    qr: Option<Qr>,
    steps: Option<Obj>,
    line1: Option<Obj>,
    line2: Option<Obj>,
    ssid: Option<Obj>,
    passphrase: Option<Obj>,
    host: Option<Obj>,
    message: Option<Obj>,
    retry: Option<Obj>,
    #[cfg(test)]
    cancel: Option<Obj>,
    started: Rc<Cell<bool>>,
    shown: Option<SetupView>,
}

impl Screen for WifiSetupScreen {
    fn build(&mut self, root: Obj, ctx: &ScreenCtx, snap: &UiSnapshot) {
        self.status = Some(StatusArea::plain(root));
        Obj::label(root, "Set up Wi-Fi", Font::Title, TEXT)
            .centered_text(SCREEN_W)
            .pos(0, TITLE_Y);
        self.qr = Some(Qr::new(root, QR, QR_QUIET));
        let steps = Obj::plain(root)
            .pos(STEPS.0, STEPS.1)
            .size(STEPS_W, SIZE_CONTENT)
            .column(STEP_GAP);
        for (number, text) in STEP_TEXT.iter().enumerate() {
            // No layout inside a step: the number stays on the first line of a wrapped step.
            let step = Obj::plain(steps).size(STEPS_W, SIZE_CONTENT);
            Obj::label(step, &(number + 1).to_string(), Font::Small, LIVE).pos(0, 0);
            Obj::label(step, text, Font::Small, TEXT)
                .width(STEP_TEXT_W)
                .pos(STEPS_W - STEP_TEXT_W, 0);
        }
        self.steps = Some(steps);
        let line1 = Obj::plain(root)
            .rect(Rect::new(0, LINE1_Y, SCREEN_W, Font::Small.line_height()))
            .row(LINE_GAP);
        Obj::label(line1, "No camera? Join", Font::Small, TEXT_2);
        self.ssid = Some(capped(line1, Font::Small, TEXT));
        let line2 = Obj::plain(root)
            .rect(Rect::new(0, LINE2_Y, SCREEN_W, Font::Small.line_height()))
            .row(LINE_GAP);
        Obj::label(line2, "password", Font::Small, TEXT_2);
        self.passphrase = Some(capped(line2, Font::Small, TEXT));
        self.host = Some(capped(line2, Font::Small, TEXT_2));
        self.line1 = Some(line1);
        self.line2 = Some(line2);
        self.message = Some(
            Obj::label(root, "", Font::Body, TEXT)
                .wrapped_text(MESSAGE_W)
                .max_lines(3)
                .pos(MESSAGE.0, MESSAGE.1),
        );
        let (dispatch, started) = (ctx.dispatch.clone(), self.started.clone());
        self.retry = Some(text_button(root, RETRY, "Try again", move || {
            start(&dispatch, &started)
        }));
        let dispatch = ctx.dispatch.clone();
        let cancel = text_button(root, BACK, "Cancel", move || {
            dispatch.command(Command::CancelWifiProvisioning);
            dispatch.back();
        });
        #[cfg(test)]
        {
            self.cancel = Some(cancel);
        }
        #[cfg(not(test))]
        let _ = cancel;
        if should_start(&snap.provisioning) {
            start(&ctx.dispatch, &self.started);
        }
        self.update(snap);
    }

    fn update(&mut self, snap: &UiSnapshot) {
        if let Some(status) = self.status.as_mut() {
            status.update(snap);
        }
        if self.started.get() && !should_start(&snap.provisioning) {
            self.started.set(false); // the service took over
        }
        let next = view(&snap.provisioning, self.started.get());
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        let code = matches!(next, SetupView::Code { .. });
        for part in [self.steps, self.line1, self.line2].into_iter().flatten() {
            part.hidden(!code);
        }
        if let Some(message) = self.message {
            message.hidden(code);
            match &next {
                SetupView::Starting => message.set_text("Starting the setup network"),
                SetupView::Busy(text) => message.set_text(text),
                SetupView::Failed(reason) => {
                    message.set_text(&format!("Couldn't connect: {reason}"))
                }
                SetupView::Code { .. } => {}
            }
        }
        if let Some(retry) = self.retry {
            retry.hidden(!matches!(next, SetupView::Failed(_)));
        }
        match (
            &next,
            self.qr.as_ref(),
            self.ssid,
            self.passphrase,
            self.host,
        ) {
            (
                SetupView::Code {
                    ssid,
                    passphrase,
                    host,
                },
                Some(qr),
                Some(s),
                Some(p),
                Some(h),
            ) => {
                // If the code can't be built, the manual details below still work.
                qr.obj()
                    .hidden(!qr.set_data(&wifi_qr_payload(ssid, passphrase)));
                set_capped(s, ssid, SSID_CAP);
                set_capped(p, &format!("{passphrase},"), PASS_CAP);
                set_capped(h, &format!("then open {host}"), HOST_CAP);
            }
            (_, Some(qr), ..) => {
                qr.obj().hidden(true);
            }
            _ => {}
        }
        self.shown = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::dispatch::UiEvent;
    use crate::kit::testkit::{assert_golden, assert_targets_ok, lvgl};
    use crate::screens_v2::menu::fixture::{fixture, snap};

    fn ap(ssid: &str, passphrase: &str) -> ProvisioningState {
        ProvisioningState::ApActive {
            ssid: ssid.into(),
            passphrase: passphrase.into(),
            url: "http://10.42.0.1/".into(),
        }
    }

    #[test]
    fn wifi_setup_matches_the_mockup() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.clock = "14:35".into();
            snap.wifi.connected = false;
            snap.provisioning = ap("Wave-7F2A", "k3x9p2vw");
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_golden("screen_wifi_setup", &lv.render());
            assert_targets_ok(root);
        });
    }

    #[test]
    fn rebuilding_during_setup_does_not_restart_it() {
        lvgl(|lv| {
            let mut f = fixture();
            let mut snap = snap();
            let mut screen = WifiSetupScreen::default();
            screen.build(lv.screen(), &f.ctx, &snap);
            assert_targets_ok(lv.screen());
            assert!(matches!(
                f.commands().as_slice(),
                [Command::OpenWifiProvisioning]
            ));
            // Starting until the service reports progress, even though the state is Idle.
            assert_eq!(screen.shown, Some(SetupView::Starting));
            for state in [
                ProvisioningState::Scanning,
                ap("Wave-7F2A", "k3x9p2vw"),
                ProvisioningState::Connecting {
                    ssid: "Office-5G".into(),
                },
                ProvisioningState::Saving,
                ProvisioningState::Reconnecting,
                ProvisioningState::Restarting,
            ] {
                snap.provisioning = state;
                let mut again = WifiSetupScreen::default();
                again.build(lv.screen(), &f.ctx, &snap);
                assert_targets_ok(lv.screen());
                assert!(f.commands().is_empty(), "{:?}", snap.provisioning);
            }
        });
    }

    #[test]
    fn cancel_stops_setup_and_goes_back() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.provisioning = ap("Wave-7F2A", "k3x9p2vw");
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert_targets_ok(root);
            lv.click(screen.cancel.unwrap());
            assert!(matches!(
                f.commands().as_slice(),
                [Command::CancelWifiProvisioning]
            ));
            assert_eq!(f.events(), vec![UiEvent::Back]);
        });
    }

    #[test]
    fn a_failure_offers_try_again() {
        lvgl(|lv| {
            let root = lv.screen();
            let mut f = fixture();
            let mut snap = snap();
            snap.provisioning = ProvisioningState::Failed {
                reason: "Wrong password".into(),
            };
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            // Opening on a stale Failed state retries at once.
            assert!(matches!(
                f.commands().as_slice(),
                [Command::OpenWifiProvisioning]
            ));
            screen.started.set(false);
            screen.update(&snap);
            assert_golden("screen_wifi_setup_failed", &lv.render());
            assert_targets_ok(root);
            assert_eq!(
                screen.message.unwrap().text(),
                "Couldn't connect: Wrong password"
            );
            assert!(!screen.retry.unwrap().is_hidden());
            lv.click(screen.retry.unwrap());
            assert!(matches!(
                f.commands().as_slice(),
                [Command::OpenWifiProvisioning]
            ));
        });
    }

    #[test]
    fn long_setup_details_stay_on_their_lines() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.provisioning = ap(
                "Wave-setup-network-with-long-nam",
                "a-sixty-three-character-passphrase-that-would-wrap-into-the-qr",
            );
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            root.update_layout();
            assert_targets_ok(root);
            for line in [screen.line1.unwrap(), screen.line2.unwrap()] {
                for i in 0..line.child_count() {
                    let a = line.child(i as i32).coords();
                    assert!(a.x1 >= 36 && a.x2 <= 364, "child {i} at {}..{}", a.x1, a.x2);
                }
            }
            assert!(screen.ssid.unwrap().text().ends_with("..."));
            assert!(screen.passphrase.unwrap().text().ends_with("..."));
        });
    }

    #[test]
    fn a_code_that_cannot_be_built_hides_the_blank_panel() {
        lvgl(|lv| {
            let root = lv.screen();
            let f = fixture();
            let mut snap = snap();
            snap.provisioning = ap("Wave-7F2A", "k3x9p2vw");
            let mut screen = WifiSetupScreen::default();
            screen.build(root, &f.ctx, &snap);
            assert!(!screen.qr.as_ref().unwrap().obj().is_hidden());
            // Too long for any QR code: lv_qrcode blanks its buffer, so the panel must go.
            snap.provisioning = ap(&"W".repeat(3000), "k3x9p2vw");
            screen.update(&snap);
            assert!(screen.qr.as_ref().unwrap().obj().is_hidden());
            // The manual details still show.
            root.update_layout();
            assert!(!screen.line1.unwrap().is_hidden());
            let t = screen.passphrase.unwrap().text();
            assert!(t.starts_with("k3x9p2vw"), "{t:?}");
            assert_targets_ok(root);
        });
    }

    #[test]
    fn views_and_payloads() {
        assert_eq!(
            wifi_qr_payload("Wave:guest;lab", "p,ass\\\"word"),
            "WIFI:T:WPA;S:Wave\\:guest\\;lab;P:p\\,ass\\\\\\\"word;H:false;;"
        );
        assert_eq!(
            view(&ap("W", "p"), false),
            SetupView::Code {
                ssid: "W".into(),
                passphrase: "p".into(),
                host: "10.42.0.1".into()
            }
        );
        assert_eq!(view(&ProvisioningState::Idle, true), SetupView::Starting);
        assert_eq!(
            view(
                &ProvisioningState::Connecting {
                    ssid: "Office".into()
                },
                false
            ),
            SetupView::Busy("Connecting to Office".into())
        );
        assert!(should_start(&ProvisioningState::Idle));
        assert!(should_start(&ProvisioningState::Failed {
            reason: "x".into()
        }));
        assert!(!should_start(&ProvisioningState::Scanning));
    }
}
