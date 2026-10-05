//! The revision-2 UI runtime: one router, the screens_v2 registry, the
//! dispatcher, the message slot, screen power and the runtime's swipes, fed
//! by one snapshot per loop iteration. `step` and `touch` are host-testable;
//! the device loop (`run`) adds LVGL's tick, input and display around them.

pub mod device;
pub mod gesture;
pub mod power;
pub mod registry;
pub mod run;
pub mod slot;
pub mod touch;

use crate::kit::dispatch::{Dispatcher, UiEvent};
use crate::kit::obj::Obj;
use crate::kit::route::Route;
use crate::kit::router::Router;
use crate::kit::screen::{Screen, ScreenCtx};
use crate::kit::snapshot::UiSnapshot;
use gesture::{leave_commands, Gestures, SwipeTaken};
use power::PowerDriver;
use slot::MessageSlot;
use std::sync::{mpsc, Arc};
use tokio::sync::broadcast;
use touch::TouchSample;
use tracing::warn;
use waved_config::WavedConfig;
use waved_state::{Command, DeviceEvent};

/// The start-up screen shows at least this long (A5: the ring draws while
/// services start). The boot screen may leave earlier with
/// `UiEvent::BootDone`; the late BootDone then changes nothing (the router
/// ignores BootDone once it has left Boot).
pub const BOOT_MIN_MS: u64 = 1_500;

/// Builds the screen for a route; `None` for routes with no screen yet.
pub type Registry = Box<dyn Fn(&Route) -> Option<Box<dyn Screen>>>;

/// Back-outs per step when routes have no screen (a chain of M3 routes).
const MAX_BACK_OUTS: usize = 4;

/// What LVGL's pointer reads this iteration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchReport {
    /// The sample to hand LVGL: released while a press that only woke the
    /// screen is still down.
    pub sample: TouchSample,
    /// A swipe was taken (by the runtime or a screen): the device cancels the
    /// press (`lv_indev_wait_release`) so it never also fires a tap.
    pub cancel_press: bool,
}

struct Shown {
    route: Route,
    root: Obj,
    screen: Box<dyn Screen>,
}

pub struct Runtime {
    router: Router,
    registry: Registry,
    ctx: ScreenCtx,
    ui_rx: mpsc::Receiver<UiEvent>,
    slot: MessageSlot,
    power: PowerDriver,
    gestures: Gestures,
    boot_done: bool,
    shown: Option<Shown>,
    /// The screen the resting screen covers, kept alive for the wake (at
    /// most one; see `replace_shown`).
    parked: Option<Shown>,
    /// Routes built since they entered the router stack: building one of
    /// these again is a rebuild (`UiSnapshot::rebuilt`). Pruned to the stack
    /// before each show, so a route that left and comes back is new again.
    built: Vec<Route>,
    /// A decision was pending after the last step: a new one wakes the screen.
    had_decision: bool,
    /// The host signalled an incoming call after the last step.
    had_ringing: bool,
    /// The router stack after the last step: a route missing from it now has
    /// left, by whatever path, and sends its leave commands.
    last_stack: Vec<Route>,
    /// The last sample `touch` handed LVGL was pressed.
    pressed: bool,
    /// The screen changed under a press: the device loop cancels it.
    cancel_press: bool,
}

impl Runtime {
    pub fn new(
        bus: broadcast::Sender<Command>,
        config: Arc<WavedConfig>,
        registry: Registry,
        power: PowerDriver,
    ) -> Self {
        let (ui_tx, ui_rx) = mpsc::channel();
        Self {
            router: Router::new(),
            registry,
            ctx: ScreenCtx {
                dispatch: Dispatcher::new(bus, ui_tx),
                config,
            },
            ui_rx,
            slot: MessageSlot::default(),
            power,
            gestures: Gestures::default(),
            boot_done: false,
            shown: None,
            parked: None,
            built: Vec::new(),
            had_decision: false,
            had_ringing: false,
            last_stack: Vec::new(),
            pressed: false,
            cancel_press: false,
        }
    }

    pub fn dispatcher(&self) -> Dispatcher {
        self.ctx.dispatch.clone()
    }

    pub fn current(&self) -> &Route {
        self.router.current()
    }

    pub fn depth(&self) -> usize {
        self.router.depth()
    }

    /// The root of the screen on display.
    pub fn root(&self) -> Option<Obj> {
        self.shown.as_ref().map(|shown| shown.root)
    }

    /// The route of the screen on display.
    pub fn shown_route(&self) -> Option<&Route> {
        self.shown.as_ref().map(|shown| &shown.route)
    }

    /// Every Command seen on the bus (announces uploads the user started).
    pub fn observe_command(&mut self, command: &Command) {
        self.slot.on_command(command);
    }

    /// One raw touch sample, before LVGL reads it. The wake gate first (a
    /// press that only wakes the screen is reported released until it is
    /// lifted), then the swipes on the shown root. A wake and the runtime's
    /// own swipe (Menu, Back) go on the ui channel and are routed by the next
    /// `step`.
    pub fn touch(&mut self, raw: TouchSample, snap: &UiSnapshot) -> TouchReport {
        let gate = self.power.on_touch(raw.pressed, snap.now_ms);
        if let Some(event) = gate.event {
            self.ctx.dispatch.ui(event);
        }
        let sample = TouchSample {
            pressed: raw.pressed && gate.deliver,
            ..raw
        };
        self.pressed = sample.pressed;
        let root = self.root().unwrap_or_else(|| {
            // SAFETY: a null handle means "no screen loaded yet"; `Gestures`
            // checks for it before any LVGL call.
            unsafe { Obj::from_raw(std::ptr::null_mut()) }
        });
        let taken = self.gestures.sample(
            sample,
            snap.now_ms,
            root,
            self.router.current(),
            self.router.depth(),
        );
        let cancel_press = taken.is_some();
        if let Some(SwipeTaken::Runtime(event)) = taken {
            self.ctx.dispatch.ui(event);
        }
        TouchReport {
            sample,
            cancel_press,
        }
    }

    /// One UI loop iteration, after the touch samples and before
    /// `lv_timer_handler`. In order: the message slot hears the snapshot,
    /// then the service events (power keys are held back); BootDone once
    /// `BOOT_MIN_MS` has passed; the ui channel (taps, touch wakes, swipes);
    /// then the power keys, so a key's Rest is not undone by a Wake or a tap
    /// from the same iteration; the power tick; the meeting sync; the leave
    /// commands of every route that left the stack; then the routed screen
    /// is shown and the top screen updated.
    pub fn step(&mut self, mut snap: UiSnapshot, events: &[DeviceEvent]) {
        // R13: no PIN screen until milestone 3, so routing treats the device
        // password as off. Screens get the real value back below (Menu's
        // lock icon shows it).
        let pin_enabled = std::mem::replace(&mut snap.settings.pin_enabled, false);
        // Before the events, so an event's message in this step wins.
        self.slot.on_snapshot(&snap);
        let mut keys = Vec::new();
        for event in events {
            match event {
                DeviceEvent::PowerKey(press) => keys.push(*press),
                other => self.slot.on_event(other, &snap),
            }
        }
        if !self.boot_done && snap.now_ms >= BOOT_MIN_MS {
            self.boot_done = true;
            self.router.handle(&UiEvent::BootDone, &snap);
        }
        self.wake_for_arrivals(&snap);
        while let Ok(event) = self.ui_rx.try_recv() {
            self.route(&event, &snap);
        }
        for press in keys {
            for event in self.power.on_power_key(press, &snap) {
                self.route(&event, &snap);
            }
        }
        // After keys and touches: the backlight is written only here.
        if let Some(event) = self.power.tick(&snap) {
            self.route(&event, &snap);
        }
        self.router.sync(&snap);
        snap.settings.pin_enabled = pin_enabled;
        snap.message = self.slot.current(snap.now_ms);
        self.send_leave_commands(&snap);
        self.show_current(&snap);
        self.last_stack = self.router.stack().to_vec();
        if let Some(shown) = self.shown.as_mut() {
            shown.screen.update(&snap);
        }
    }

    /// A new pending decision (a recording stopped) or an incoming call
    /// counts as activity: the screen wakes and the router shows what the
    /// state asks for. Only the rising edge counts, so a decision left
    /// unanswered does not keep the screen awake. No press is delivered.
    fn wake_for_arrivals(&mut self, snap: &UiSnapshot) {
        let decision = snap.meeting.pending_decision.is_some();
        let ringing = snap.audio.call_ringing;
        let arrived = (decision && !self.had_decision) || (ringing && !self.had_ringing);
        self.had_decision = decision;
        self.had_ringing = ringing;
        if arrived {
            if let Some(event) = self.power.on_activity(snap.now_ms) {
                self.route(&event, snap);
            }
        }
    }

    /// One UiEvent: a message goes to the slot, everything else to the router.
    fn route(&mut self, event: &UiEvent, snap: &UiSnapshot) {
        match event {
            UiEvent::Show(message) => self.slot.post(message.clone(), snap.now_ms),
            other => self.router.handle(other, snap),
        }
    }

    /// Every route that left the router stack since the last step sends what
    /// leaving it sends (`leave_commands`), exactly once, whatever removed it:
    /// a swipe or button Back, ToBase, a base Navigate, a meeting takeover.
    /// This is the one place leave commands are sent.
    fn send_leave_commands(&self, snap: &UiSnapshot) {
        let stack = self.router.stack();
        for route in self
            .last_stack
            .iter()
            .filter(|route| !stack.contains(route))
        {
            for command in leave_commands(route, snap) {
                self.ctx.dispatch.command(command);
            }
        }
    }

    /// Show the routed screen if it is not the one on display: the parked
    /// screen is re-loaded as it was (no rebuild, so a screen that acts in
    /// `build` does not act again on waking); any other route is built on a
    /// new root and loaded. A route with no screen yet (milestone 3) goes
    /// back. A parked screen whose route left the stack is deleted first.
    fn show_current(&mut self, snap: &UiSnapshot) {
        let stack = self.router.stack();
        self.built.retain(|route| stack.contains(route));
        if self
            .parked
            .as_ref()
            .is_some_and(|parked| !self.router.stack().contains(&parked.route))
        {
            if let Some(parked) = self.parked.take() {
                parked.root.delete();
            }
        }
        for _ in 0..MAX_BACK_OUTS {
            let route = self.router.current().clone();
            if self
                .shown
                .as_ref()
                .is_some_and(|shown| shown.route == route)
            {
                return;
            }
            if self
                .parked
                .as_ref()
                .is_some_and(|parked| parked.route == route)
            {
                if let Some(parked) = self.parked.take() {
                    load(parked.root);
                    self.replace_shown(parked);
                }
                return;
            }
            let Some(mut screen) = (self.registry)(&route) else {
                warn!("no screen for {route:?} yet; going back");
                if self.router.depth() > 1 {
                    // Routing sees the PIN as off (R13), here too.
                    let mut masked = snap.clone();
                    masked.settings.pin_enabled = false;
                    self.router.handle(&UiEvent::Back, &masked);
                    continue;
                }
                return;
            };
            let root = Obj::screen();
            let mut build_snap = snap.clone();
            build_snap.rebuilt = self.built.contains(&route);
            screen.build(root, &self.ctx, &build_snap);
            if !build_snap.rebuilt {
                self.built.push(route.clone());
            }
            load(root);
            self.replace_shown(Shown {
                route,
                root,
                screen,
            });
            return;
        }
    }

    /// `next` (already loaded) becomes the screen on display. The old one is
    /// parked when `next` is the resting screen pushed right on top of its
    /// route and nothing is parked yet; otherwise it is deleted with every
    /// handler. A press that is down while the screen changes is cancelled
    /// (`take_cancel_press`) and swipes nothing more: it began on another
    /// screen, and a parked screen's pressed object would still click.
    fn replace_shown(&mut self, next: Shown) {
        if let Some(old) = self.shown.take() {
            if self.pressed {
                self.cancel_press = true;
                self.gestures.ignore_press();
            }
            let stack = self.router.stack();
            let parks = next.route == Route::Resting
                && self.parked.is_none()
                && stack.len() >= 2
                && stack[stack.len() - 2] == old.route;
            if parks {
                self.parked = Some(old);
            } else {
                old.root.delete();
            }
        }
        self.shown = Some(next);
    }

    /// Whether a `step` since the last call changed the screen under a
    /// press. The device loop then cancels the press (`lv_indev_wait_release`)
    /// after `step` and before LVGL's timers run, so the press clicks nothing.
    pub fn take_cancel_press(&mut self) -> bool {
        std::mem::take(&mut self.cancel_press)
    }

    /// Delete the screen on display (tests only; the device never stops).
    #[cfg(test)]
    pub fn close(mut self) {
        for screen in [self.shown.take(), self.parked.take()]
            .into_iter()
            .flatten()
        {
            screen.root.delete();
        }
    }
}

fn load(root: Obj) {
    // SAFETY: `root` is a live screen created by `Obj::screen`.
    unsafe { crate::bindings::lv_screen_load(root.raw()) };
}

#[cfg(test)]
pub(crate) mod testing {
    use crate::bindings::{lv_label_class, lv_obj_check_type};
    use crate::kit::obj::Obj;

    /// A visible label under `root` reads exactly `text`. Refreshes the
    /// layout first, as LVGL does before drawing: until then a one-line label
    /// sized in the same step still holds its "..." placeholder.
    pub(crate) fn shows_text(root: Obj, text: &str) -> bool {
        root.update_layout();
        finds_text(root, text)
    }

    fn finds_text(root: Obj, text: &str) -> bool {
        if root.is_hidden() {
            return false;
        }
        // SAFETY: live object; `lv_label_class` is a static class descriptor.
        let label = unsafe { lv_obj_check_type(root.raw(), &raw const lv_label_class) };
        if label && root.text() == text {
            return true;
        }
        (0..root.child_count()).any(|i| finds_text(root.child(i as i32), text))
    }
}

#[cfg(test)]
mod tests {
    use super::device::Pointer;
    use super::gesture::press_target;
    use super::power::BacklightOut;
    use super::testing::shows_text;
    use super::*;
    use crate::bindings::{lv_obj_is_valid, lv_screen_active};
    use crate::kit::components::status::StatusArea;
    use crate::kit::dispatch::Severity;
    use crate::kit::fonts::Font;
    use crate::kit::route::ConfirmKind;
    use crate::kit::testkit::{lvgl, Lvgl};
    use crate::kit::tokens::{Rect, TEXT};
    use crate::screens_v2::messages::copy;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::rc::Rc;
    use uuid::Uuid;
    use waved_state::{PendingDecision, PowerKeyPress, ProvisioningState};

    const MIN: u64 = 60_000;
    /// A Probe's drag control (keeps its gestures).
    const KNOB: Rect = Rect::new(240, 180, 100, 60);

    thread_local! {
        static BUILDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static LAST_NOW: Cell<Option<u64>> = const { Cell::new(None) };
        /// `settings.pin_enabled` as the Probe saw it in (build, update).
        static PIN_SEEN: Cell<(Option<bool>, Option<bool>)> = const { Cell::new((None, None)) };
        /// Clicks on the TapTarget's button.
        static TAPS: Cell<u32> = const { Cell::new(0) };
    }

    fn builds() -> Vec<String> {
        BUILDS.with(|b| b.borrow().clone())
    }

    /// A screen that names its route, has a status area and a drag control.
    struct Probe {
        name: String,
        status: Option<StatusArea>,
    }

    impl Screen for Probe {
        fn build(&mut self, root: Obj, _ctx: &ScreenCtx, snap: &UiSnapshot) {
            BUILDS.with(|b| b.borrow_mut().push(self.name.clone()));
            PIN_SEEN.with(|p| p.set((Some(snap.settings.pin_enabled), p.get().1)));
            self.status = Some(StatusArea::plain(root));
            Obj::label(root, &self.name, Font::Body, TEXT).pos(100, 140);
            Obj::plain(root).rect(KNOB).clickable(true).keep_gestures();
            root.update_layout();
            self.update(snap);
        }

        fn update(&mut self, snap: &UiSnapshot) {
            LAST_NOW.with(|n| n.set(Some(snap.now_ms)));
            PIN_SEEN.with(|p| p.set((p.get().0, Some(snap.settings.pin_enabled))));
            if let Some(status) = self.status.as_mut() {
                status.update(snap);
            }
        }
    }

    fn probes(route: &Route) -> Option<Box<dyn Screen>> {
        match route {
            Route::Bluetooth | Route::MicHeight | Route::Pin(_) => None,
            other => Some(Box::new(Probe {
                name: format!("{other:?}"),
                status: None,
            })),
        }
    }

    struct NoPanel;

    impl BacklightOut for NoPanel {
        fn set_percent(&mut self, _pct: u8) -> std::io::Result<bool> {
            Ok(true)
        }
    }

    fn ready(now_ms: u64) -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.settings.first_run_done = true;
        snap.now_ms = now_ms;
        snap
    }

    fn runtime_with_bus() -> (Runtime, broadcast::Receiver<Command>) {
        runtime_with(Box::new(probes))
    }

    fn runtime_with(registry: Registry) -> (Runtime, broadcast::Receiver<Command>) {
        BUILDS.with(|b| b.borrow_mut().clear());
        LAST_NOW.with(|n| n.set(None));
        PIN_SEEN.with(|p| p.set((None, None)));
        let (bus, bus_rx) = broadcast::channel(16);
        let rt = Runtime::new(
            bus,
            Arc::new(WavedConfig::default()),
            registry,
            PowerDriver::new(0, Box::new(NoPanel)),
        );
        (rt, bus_rx)
    }

    fn runtime() -> Runtime {
        runtime_with_bus().0
    }

    fn booted(lv: &mut Lvgl) -> Runtime {
        lv.screen();
        let mut rt = runtime();
        rt.step(ready(0), &[]);
        rt.step(ready(BOOT_MIN_MS), &[]);
        rt
    }

    fn send(rt: &mut Runtime, event: UiEvent, now_ms: u64) -> Option<Route> {
        rt.dispatcher().ui(event);
        rt.step(ready(now_ms), &[]);
        rt.shown_route().cloned()
    }

    fn decision() -> PendingDecision {
        PendingDecision {
            id: Uuid::new_v4(),
            audio_path: PathBuf::from("/mnt/udisk/recordings/x/audio.wav"),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            endpoint: "https://notes.example.com/hook".into(),
            speakers: 0,
            marks: 0,
        }
    }

    fn down(x: i32, y: i32) -> TouchSample {
        TouchSample {
            x,
            y,
            pressed: true,
        }
    }

    fn up() -> TouchSample {
        TouchSample::default()
    }

    /// Press at `from`, move 70 px right 100 ms later, lift: whether any
    /// sample asked to cancel the press.
    fn swipe_right(rt: &mut Runtime, from: (i32, i32), snap: &UiSnapshot) -> bool {
        let mut at = snap.clone();
        let mut cancel = rt.touch(down(from.0, from.1), &at).cancel_press;
        at.now_ms += 100;
        cancel |= rt.touch(down(from.0 + 70, from.1), &at).cancel_press;
        at.now_ms += 50;
        cancel |= rt.touch(up(), &at).cancel_press;
        cancel
    }

    fn drain(bus: &mut broadcast::Receiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| bus.try_recv().ok()).collect()
    }

    #[test]
    fn the_start_up_screen_gives_way_after_its_ring_or_when_it_says_so() {
        lvgl(|lv| {
            lv.screen();
            let mut rt = runtime();
            rt.step(ready(0), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Boot));
            rt.step(ready(BOOT_MIN_MS - 1), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Boot));
            rt.step(ready(BOOT_MIN_MS), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Home));
            rt.close();

            lv.screen();
            let mut early = runtime();
            early.step(ready(0), &[]);
            early.dispatcher().ui(UiEvent::BootDone);
            early.step(ready(400), &[]);
            assert_eq!(early.shown_route(), Some(&Route::Home));
            early.step(ready(BOOT_MIN_MS), &[]);
            assert_eq!(
                early.shown_route(),
                Some(&Route::Home),
                "the late BootDone changes nothing"
            );
            early.close();
        });
    }

    #[test]
    fn navigation_builds_the_routed_screen_and_deletes_the_last_one() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            assert_eq!(rt.shown_route(), Some(&Route::Home));
            let home = rt.root().unwrap();
            assert_eq!(
                send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000),
                Some(Route::Menu)
            );
            // SAFETY: only compares pointers against live objects.
            unsafe {
                assert!(
                    !lv_obj_is_valid(home.raw()),
                    "the old screen and its handlers are gone"
                );
                assert_eq!(lv_screen_active(), rt.root().unwrap().raw());
            }
            assert_eq!(builds(), ["Boot", "Home", "Menu"]);
            rt.step(ready(2_100), &[]);
            assert_eq!(builds().len(), 3, "an unchanged route is not rebuilt");
            rt.close();
        });
    }

    #[test]
    fn back_returns_to_the_screen_that_opened_it() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let nav = |route| UiEvent::Navigate(route);
            assert_eq!(send(&mut rt, nav(Route::Menu), 2_000), Some(Route::Menu));
            assert_eq!(
                send(&mut rt, nav(Route::AudioMode), 2_100),
                Some(Route::AudioMode)
            );
            assert_eq!(
                send(&mut rt, UiEvent::Back, 2_200),
                Some(Route::Menu),
                "BUG-11"
            );
            assert_eq!(
                send(&mut rt, nav(Route::System), 2_300),
                Some(Route::System)
            );
            assert_eq!(
                send(&mut rt, nav(Route::Display), 2_400),
                Some(Route::Display)
            );
            assert_eq!(send(&mut rt, UiEvent::Back, 2_500), Some(Route::System));
            assert_eq!(send(&mut rt, UiEvent::Back, 2_600), Some(Route::Menu));
            assert_eq!(send(&mut rt, UiEvent::Back, 2_700), Some(Route::Home));
            assert_eq!(
                send(&mut rt, UiEvent::Back, 2_800),
                Some(Route::Home),
                "Back stops at the base"
            );
            rt.close();
        });
    }

    #[test]
    fn meeting_state_moves_the_base_from_any_screen() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000);
            send(&mut rt, UiEvent::Navigate(Route::Volume), 2_100);
            let mut snap = ready(2_200);
            snap.meeting.recording = true;
            rt.step(snap.clone(), &[]);
            assert_eq!(
                rt.shown_route(),
                Some(&Route::Volume),
                "the open screen stays"
            );
            rt.dispatcher().back();
            rt.dispatcher().back();
            snap.now_ms = 2_300;
            rt.step(snap.clone(), &[]);
            assert_eq!(
                rt.shown_route(),
                Some(&Route::Recording),
                "BUG-13: the base followed the meeting"
            );
            rt.dispatcher().navigate(Route::Menu);
            snap.now_ms = 2_400;
            rt.step(snap.clone(), &[]);
            snap.meeting.recording = false;
            snap.meeting.pending_decision = Some(decision());
            snap.now_ms = 2_500;
            rt.step(snap, &[]);
            assert_eq!(
                (rt.shown_route(), rt.depth()),
                (Some(&Route::Decision), 1),
                "the decision takes over at once"
            );
            rt.close();
        });
    }

    #[test]
    fn a_route_without_a_screen_goes_back() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000);
            assert_eq!(
                send(&mut rt, UiEvent::Navigate(Route::Bluetooth), 2_100),
                Some(Route::Menu)
            );
            assert_eq!(rt.depth(), 2);
            assert_eq!(builds(), ["Boot", "Home", "Menu"], "Menu was not rebuilt");
            rt.close();
        });
    }

    #[test]
    fn a_device_password_locks_nothing_until_the_pin_screen_lands() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let mut snap = ready(2_000);
            snap.settings.pin_enabled = true;
            rt.dispatcher().navigate(Route::Files);
            rt.step(snap, &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Files));
            assert_eq!(
                PIN_SEEN.with(Cell::get),
                (Some(true), Some(true)),
                "R13: screens still see the setting (Menu's lock icon)"
            );
            rt.close();
        });
    }

    #[test]
    fn messages_reach_the_status_slot_and_outlive_navigation() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let id = Uuid::new_v4();
            rt.step(ready(2_000), &[DeviceEvent::RecordingStopped(id)]);
            rt.step(ready(2_100), &[DeviceEvent::RecordingUploadSucceeded(id)]);
            assert!(shows_text(rt.root().unwrap(), copy::RECORDING_SENT));
            rt.dispatcher().navigate(Route::Menu);
            rt.step(ready(3_000), &[]);
            assert!(
                shows_text(rt.root().unwrap(), copy::RECORDING_SENT),
                "the next screen keeps it"
            );
            rt.step(ready(4_100), &[]);
            assert!(
                !shows_text(rt.root().unwrap(), copy::RECORDING_SENT),
                "gone 2 s after it was posted"
            );
            rt.dispatcher().show(Severity::Warning, copy::HOLD_STOP);
            rt.step(ready(4_200), &[]);
            assert!(shows_text(rt.root().unwrap(), copy::HOLD_STOP));
            rt.close();
        });
    }

    #[test]
    fn a_resent_recording_is_announced() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let id = Uuid::new_v4();
            rt.observe_command(&Command::SendRecording(id));
            rt.step(ready(2_000), &[DeviceEvent::RecordingUploadSucceeded(id)]);
            assert!(shows_text(rt.root().unwrap(), copy::RECORDING_SENT));
            rt.close();
        });
    }

    #[test]
    fn an_events_message_wins_over_the_snapshots_in_the_same_step() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let id = Uuid::new_v4();
            rt.observe_command(&Command::SendRecording(id));
            let mut snap = ready(2_000);
            snap.battery.voltage = 3_600;
            snap.battery.level = 10;
            rt.step(snap, &[DeviceEvent::RecordingUploadSucceeded(id)]);
            let root = rt.root().unwrap();
            assert!(shows_text(root, copy::RECORDING_SENT));
            assert!(!shows_text(root, &copy::battery_low(10)));
            rt.close();
        });
    }

    struct Panel(Rc<RefCell<Vec<u8>>>);

    impl BacklightOut for Panel {
        fn set_percent(&mut self, pct: u8) -> std::io::Result<bool> {
            self.0.borrow_mut().push(pct);
            Ok(true)
        }
    }

    /// Booted, with every backlight level written so far recorded.
    fn booted_lit(lv: &mut Lvgl) -> (Runtime, Rc<RefCell<Vec<u8>>>) {
        lv.screen();
        BUILDS.with(|b| b.borrow_mut().clear());
        let writes = Rc::new(RefCell::new(Vec::new()));
        let (bus, _bus_rx) = broadcast::channel(16);
        let mut rt = Runtime::new(
            bus,
            Arc::new(WavedConfig::default()),
            Box::new(probes),
            PowerDriver::new(0, Box::new(Panel(writes.clone()))),
        );
        rt.step(ready(0), &[]);
        rt.step(ready(BOOT_MIN_MS), &[]);
        (rt, writes)
    }

    #[test]
    fn a_decision_arriving_while_the_screen_rests_wakes_it() {
        for off in [false, true] {
            lvgl(|lv| {
                let (mut rt, writes) = booted_lit(lv);
                let resting_at = if off { 10 * MIN } else { 2 * MIN };
                rt.step(ready(resting_at), &[]);
                assert_eq!(rt.shown_route(), Some(&Route::Resting));
                assert_ne!(writes.borrow().last(), Some(&80), "dim or dark");

                let mut arrived = ready(resting_at + 100);
                arrived.meeting.pending_decision = Some(decision());
                rt.step(arrived.clone(), &[]);
                assert_eq!(rt.shown_route(), Some(&Route::Decision));
                assert_eq!(writes.borrow().last(), Some(&80), "the screen is on");

                // Nothing was swallowed or delivered: the next press is a
                // normal press on the decision screen.
                arrived.now_ms += 100;
                let press = rt.touch(down(120, 160), &arrived);
                assert_eq!(press.sample, down(120, 160));
                rt.close();
            });
        }
    }

    #[test]
    fn a_decision_left_pending_does_not_keep_the_screen_awake() {
        lvgl(|lv| {
            let (mut rt, writes) = booted_lit(lv);
            rt.step(ready(2 * MIN), &[]);
            let mut arrived = ready(2 * MIN + 100);
            arrived.meeting.pending_decision = Some(decision());
            rt.step(arrived.clone(), &[]);
            assert_eq!(writes.borrow().last(), Some(&80));
            // Unanswered for the dim time: it rests again, once.
            arrived.now_ms = 2 * MIN + 100 + 2 * MIN;
            rt.step(arrived.clone(), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting));
            arrived.now_ms += 100;
            rt.step(arrived, &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting), "not re-woken");
            assert_ne!(writes.borrow().last(), Some(&80));
            rt.close();
        });
    }

    #[test]
    fn an_incoming_call_wakes_a_resting_screen() {
        lvgl(|lv| {
            let (mut rt, writes) = booted_lit(lv);
            rt.step(ready(2 * MIN), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting));
            let mut ringing = ready(2 * MIN + 100);
            ringing.audio.call_ringing = true;
            rt.step(ringing, &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Home));
            assert_eq!(writes.borrow().last(), Some(&80));
            rt.close();
        });
    }

    #[test]
    fn the_first_touch_on_the_resting_screen_only_wakes() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000);
            rt.step(ready(2 * MIN), &[]);
            assert_eq!(
                rt.shown_route(),
                Some(&Route::Resting),
                "idle for the dim time"
            );

            let at = 2 * MIN + 10;
            let wake = rt.touch(down(200, 150), &ready(at));
            assert!(!wake.sample.pressed, "the waking press never reaches LVGL");
            assert!(!wake.cancel_press);
            rt.step(ready(at + 10), &[]);
            assert_eq!(
                rt.shown_route(),
                Some(&Route::Menu),
                "the screen it rested on"
            );

            assert!(
                !rt.touch(down(200, 150), &ready(at + 50)).sample.pressed,
                "held"
            );
            assert!(!rt.touch(up(), &ready(at + 90)).sample.pressed, "released");
            let next = rt.touch(down(120, 160), &ready(at + 500));
            assert_eq!(next.sample, down(120, 160), "the next press is reported");
            assert!(!next.cancel_press);
            rt.close();
        });
    }

    #[test]
    fn the_power_key_rests_and_asks_to_power_off() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            rt.step(ready(2_000), &[DeviceEvent::PowerKey(PowerKeyPress::Short)]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting));
            rt.step(ready(3_000), &[DeviceEvent::PowerKey(PowerKeyPress::Short)]);
            assert_eq!(rt.shown_route(), Some(&Route::Home), "a second press wakes");
            rt.step(ready(4_000), &[DeviceEvent::PowerKey(PowerKeyPress::Long)]);
            assert_eq!(
                rt.shown_route(),
                Some(&Route::Confirm(ConfirmKind::PowerOff))
            );
            rt.close();
        });
    }

    #[test]
    fn the_power_key_wins_over_touches_queued_in_the_same_step() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            rt.dispatcher().navigate(Route::Menu);
            rt.step(ready(2_000), &[DeviceEvent::PowerKey(PowerKeyPress::Short)]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting), "a tap's Navigate");
            rt.close();

            let mut rt = booted(lv);
            rt.step(ready(2 * MIN), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting));
            rt.touch(down(200, 150), &ready(2 * MIN + 10));
            rt.touch(up(), &ready(2 * MIN + 60));
            rt.step(
                ready(2 * MIN + 100),
                &[DeviceEvent::PowerKey(PowerKeyPress::Short)],
            );
            assert_eq!(rt.shown_route(), Some(&Route::Resting), "a touch's Wake");
            rt.close();
        });
    }

    #[test]
    fn a_swipe_right_on_a_sub_screen_goes_back_and_cancels_the_press() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000);
            send(&mut rt, UiEvent::Navigate(Route::Volume), 2_100);
            assert!(swipe_right(&mut rt, (100, 150), &ready(2_200)));
            assert_eq!(
                rt.shown_route(),
                Some(&Route::Volume),
                "routed in the next step"
            );
            rt.step(ready(2_400), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Menu));

            let start = (KNOB.x + 10, KNOB.y + 20);
            assert!(
                !swipe_right(&mut rt, start, &ready(2_500)),
                "a drag on a control that keeps it is no swipe"
            );
            rt.step(ready(2_700), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Menu), "nothing changed");
            rt.close();
        });
    }

    fn booted_with_bus(lv: &mut Lvgl) -> (Runtime, broadcast::Receiver<Command>) {
        lv.screen();
        let (mut rt, bus) = runtime_with_bus();
        rt.step(ready(0), &[]);
        rt.step(ready(BOOT_MIN_MS), &[]);
        (rt, bus)
    }

    #[test]
    fn a_swipe_back_or_to_the_base_sends_the_leave_commands_once() {
        lvgl(|lv| {
            let (mut rt, mut bus) = booted_with_bus(lv);
            let id = Uuid::new_v4();
            send(&mut rt, UiEvent::Navigate(Route::Files), 2_000);
            send(&mut rt, UiEvent::Navigate(Route::FileDetail(id)), 2_100);
            let mut playing = ready(2_200);
            playing.playback.recording = Some(id);
            assert!(swipe_right(&mut rt, (100, 150), &playing));
            assert!(
                drain(&mut bus).is_empty(),
                "sent when the route leaves the stack, not by the swipe"
            );
            playing.now_ms = 2_400;
            rt.step(playing.clone(), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Files));
            assert_eq!(drain(&mut bus), [Command::StopPlayback]);
            playing.now_ms = 2_450;
            rt.step(playing, &[]);
            assert!(drain(&mut bus).is_empty(), "exactly once");

            send(&mut rt, UiEvent::Navigate(Route::Wifi), 2_500);
            send(&mut rt, UiEvent::Navigate(Route::WifiSetup), 2_600);
            drain(&mut bus);
            assert_eq!(send(&mut rt, UiEvent::ToBase, 2_700), Some(Route::Home));
            assert_eq!(drain(&mut bus), [Command::CancelWifiProvisioning]);
            rt.close();
        });
    }

    #[test]
    fn a_button_back_from_the_playing_file_detail_stops_playback_once() {
        lvgl(|lv| {
            let (mut rt, mut bus) = booted_with_bus(lv);
            let id = Uuid::new_v4();
            send(&mut rt, UiEvent::Navigate(Route::Files), 2_000);
            send(&mut rt, UiEvent::Navigate(Route::FileDetail(id)), 2_100);
            drain(&mut bus);
            // What the screen's Back button sends (the Probe sends no command).
            rt.dispatcher().back();
            let mut playing = ready(2_200);
            playing.playback.recording = Some(id);
            rt.step(playing.clone(), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Files));
            assert_eq!(drain(&mut bus), [Command::StopPlayback]);
            playing.now_ms = 2_300;
            rt.step(playing, &[]);
            assert!(drain(&mut bus).is_empty(), "exactly once");
            rt.close();
        });
    }

    #[test]
    fn a_takeover_or_a_base_navigate_cancels_wifi_setup() {
        lvgl(|lv| {
            let (mut rt, mut bus) = booted_with_bus(lv);
            send(&mut rt, UiEvent::Navigate(Route::Wifi), 2_000);
            send(&mut rt, UiEvent::Navigate(Route::WifiSetup), 2_100);
            drain(&mut bus);
            let mut takeover = ready(2_200);
            takeover.meeting.pending_decision = Some(decision());
            rt.step(takeover, &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Decision));
            assert_eq!(drain(&mut bus), [Command::CancelWifiProvisioning]);

            rt.step(ready(2_300), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Home));
            send(&mut rt, UiEvent::Navigate(Route::Wifi), 2_400);
            send(&mut rt, UiEvent::Navigate(Route::WifiSetup), 2_500);
            drain(&mut bus);
            assert_eq!(
                send(&mut rt, UiEvent::Navigate(Route::Home), 2_600),
                Some(Route::Home)
            );
            assert_eq!(drain(&mut bus), [Command::CancelWifiProvisioning]);
            rt.close();
        });
    }

    /// The real Wi-Fi setup screen (it starts provisioning from `build`),
    /// Probes elsewhere.
    fn with_setup(route: &Route) -> Option<Box<dyn Screen>> {
        match route {
            Route::WifiSetup => registry::screen_for(route),
            other => probes(other),
        }
    }

    fn setup_failed(now_ms: u64) -> UiSnapshot {
        let mut snap = ready(now_ms);
        snap.provisioning = ProvisioningState::Failed {
            reason: "wrong password".into(),
        };
        snap
    }

    /// Booted with the real Wi-Fi setup screen open; its start command drained.
    fn on_wifi_setup(lv: &mut Lvgl) -> (Runtime, broadcast::Receiver<Command>) {
        lv.screen();
        let (mut rt, mut bus) = runtime_with(Box::new(with_setup));
        rt.step(ready(0), &[]);
        rt.step(ready(BOOT_MIN_MS), &[]);
        send(&mut rt, UiEvent::Navigate(Route::Wifi), 2_000);
        send(&mut rt, UiEvent::Navigate(Route::WifiSetup), 2_100);
        assert_eq!(drain(&mut bus), [Command::OpenWifiProvisioning]);
        (rt, bus)
    }

    /// Alive while `obj` is: the kit drops a handler (and the clone it holds)
    /// when its object is deleted. Unlike `lv_obj_is_valid`, a new object at
    /// a reused address does not fool it.
    struct Life(Rc<()>);

    impl Life {
        fn of(obj: Obj) -> Self {
            let life = Rc::new(());
            let held = life.clone();
            crate::kit::event::on(
                obj,
                crate::bindings::lv_event_code_t_LV_EVENT_CLICKED,
                move |_| {
                    let _ = &held;
                },
            );
            Self(life)
        }

        fn alive(&self) -> bool {
            Rc::strong_count(&self.0) > 1
        }
    }

    #[test]
    fn waking_on_wifi_setup_shows_the_same_screen_without_restarting_provisioning() {
        lvgl(|lv| {
            let (mut rt, mut bus) = on_wifi_setup(lv);
            let mut serving = ready(2_200);
            serving.provisioning = ProvisioningState::ApActive {
                ssid: "Wave-setup".into(),
                passphrase: "12345678".into(),
                url: "http://192.168.4.1/".into(),
            };
            rt.step(serving, &[]);
            rt.step(setup_failed(3_000), &[]);
            let setup = rt.root().unwrap();
            let setup_life = Life::of(setup);
            let failure = "Couldn't connect: wrong password";
            assert!(shows_text(setup, failure));

            rt.step(setup_failed(2 * MIN), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting));
            assert!(setup_life.alive(), "parked under the resting screen");
            let ambient = Life::of(rt.root().unwrap());

            let at = 2 * MIN + 10;
            rt.touch(down(200, 150), &setup_failed(at));
            rt.step(setup_failed(at + 10), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::WifiSetup));
            assert!(setup_life.alive());
            assert_eq!(rt.root(), Some(setup), "re-loaded, not rebuilt");
            // SAFETY: reads LVGL's active screen pointer.
            assert_eq!(unsafe { lv_screen_active() }, setup.raw());
            assert!(!ambient.alive(), "the resting screen is gone");
            assert!(
                drain(&mut bus).is_empty(),
                "waking does not restart provisioning"
            );
            assert!(shows_text(setup, failure), "the failure still shows");
            rt.close();
        });
    }

    #[test]
    fn back_from_the_power_off_confirmation_rebuilds_wifi_setup_without_restarting_it() {
        lvgl(|lv| {
            let (mut rt, mut bus) = on_wifi_setup(lv);
            // The setup network timed out: Idle again, as on a first visit.
            rt.step(ready(2_200), &[]);
            let confirm = UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff));
            assert_eq!(
                send(&mut rt, confirm, 2_300),
                Some(Route::Confirm(ConfirmKind::PowerOff))
            );
            let before = builds().len();
            rt.dispatcher().back();
            rt.step(ready(2_400), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::WifiSetup));
            assert_eq!(builds().len(), before, "the real screen, not a probe");
            let closed = "The setup network closed. Try again to reopen it.";
            assert!(shows_text(rt.root().unwrap(), closed));
            assert!(drain(&mut bus).is_empty(), "no restart on a rebuild");

            // Leaving and opening it again is a first visit again.
            send(&mut rt, UiEvent::Back, 2_500);
            assert_eq!(rt.shown_route(), Some(&Route::Wifi));
            assert_eq!(drain(&mut bus), [Command::CancelWifiProvisioning]);
            send(&mut rt, UiEvent::Navigate(Route::WifiSetup), 2_600);
            assert_eq!(drain(&mut bus), [Command::OpenWifiProvisioning]);
            rt.close();
        });
    }

    #[test]
    fn a_parked_screen_whose_route_leaves_the_stack_is_deleted_and_sends_its_leave_commands() {
        lvgl(|lv| {
            let (mut rt, mut bus) = on_wifi_setup(lv);
            let setup = Life::of(rt.root().unwrap());
            rt.step(ready(2 * MIN), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Resting));
            assert!(setup.alive(), "parked");
            let ambient = Life::of(rt.root().unwrap());
            assert!(drain(&mut bus).is_empty(), "resting leaves nothing");

            let mut takeover = ready(2 * MIN + 100);
            takeover.meeting.pending_decision = Some(decision());
            rt.step(takeover, &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Decision));
            assert!(!setup.alive(), "the parked screen is deleted");
            assert!(!ambient.alive());
            assert_eq!(drain(&mut bus), [Command::CancelWifiProvisioning]);
            rt.close();
        });
    }

    /// The TapTarget's button: over the Probe's knob and the empty space
    /// left of it.
    const BUTTON: Rect = Rect::new(40, 170, 320, 100);
    /// Empty space on a Probe, inside BUTTON.
    const EMPTY: (i32, i32) = (80, 220);

    /// A screen with one big button that counts its clicks.
    struct TapTarget;

    impl Screen for TapTarget {
        fn build(&mut self, root: Obj, _ctx: &ScreenCtx, _snap: &UiSnapshot) {
            let button = Obj::plain(root).rect(BUTTON).clickable(true);
            crate::kit::event::on_click(button, || TAPS.with(|t| t.set(t.get() + 1)));
            root.update_layout();
        }

        fn update(&mut self, _snap: &UiSnapshot) {}
    }

    /// Recording and Menu are TapTargets, everything else a Probe.
    fn tap_targets(route: &Route) -> Option<Box<dyn Screen>> {
        match route {
            Route::Recording | Route::Menu => Some(Box::new(TapTarget)),
            other => probes(other),
        }
    }

    fn booted_tapping(lv: &mut Lvgl) -> Runtime {
        lv.screen();
        TAPS.with(|t| t.set(0));
        let (mut rt, _bus) = runtime_with(Box::new(tap_targets));
        rt.step(ready(0), &[]);
        rt.step(ready(BOOT_MIN_MS), &[]);
        rt
    }

    /// One iteration of `run::run` around a real LVGL pointer: the touch
    /// report reaches the pointer, the step, the cancel the step asks for
    /// (unless `cancel` is false), then LVGL reads the pointer. Returns
    /// whether the step asked for a cancel.
    fn iterate(
        rt: &mut Runtime,
        pointer: &Pointer,
        sample: TouchSample,
        snap: UiSnapshot,
        cancel: bool,
    ) -> bool {
        iterate_with(rt, pointer, sample, snap, &[], cancel)
    }

    fn iterate_with(
        rt: &mut Runtime,
        pointer: &Pointer,
        sample: TouchSample,
        snap: UiSnapshot,
        events: &[DeviceEvent],
        cancel: bool,
    ) -> bool {
        let report = rt.touch(sample, &snap);
        pointer.report(report.sample);
        if report.cancel_press {
            pointer.cancel_press();
        }
        rt.step(snap, events);
        let asked = rt.take_cancel_press();
        if asked && cancel {
            pointer.cancel_press();
        }
        pointer.read();
        asked
    }

    fn recording(now_ms: u64) -> UiSnapshot {
        let mut snap = ready(now_ms);
        snap.meeting.recording = true;
        snap
    }

    fn taps() -> u32 {
        TAPS.with(Cell::get)
    }

    #[test]
    fn a_press_on_empty_space_does_not_tap_the_screen_that_replaces_it() {
        lvgl(|lv| {
            let mut rt = booted_tapping(lv);
            let pointer = Pointer::create();
            let (x, y) = EMPTY;
            // In LVGL 9.2 such a press is the bottom layer's (clickable, no
            // press lock), so LVGL itself turns the new screen's button into
            // a PRESS_LOST; the runtime's cancel makes that explicit.
            assert_eq!(
                press_target(rt.root().unwrap(), x, y),
                None,
                "empty on Home"
            );
            assert!(!iterate(&mut rt, &pointer, down(x, y), ready(2_000), true));
            assert!(
                iterate(&mut rt, &pointer, down(x, y), recording(2_050), true),
                "the meeting change replaced the screen under the press"
            );
            assert_eq!(rt.shown_route(), Some(&Route::Recording));
            assert!(
                press_target(rt.root().unwrap(), x, y).is_some(),
                "a button now"
            );
            iterate(&mut rt, &pointer, down(x, y), recording(2_100), true);
            iterate(&mut rt, &pointer, up(), recording(2_150), true);
            assert_eq!(taps(), 0, "the press that began on Home taps nothing");

            assert!(!iterate(
                &mut rt,
                &pointer,
                down(x, y),
                recording(2_500),
                true
            ));
            assert!(!iterate(&mut rt, &pointer, up(), recording(2_550), true));
            assert_eq!(taps(), 1, "a press that starts after the release taps");
            drop(pointer);
            rt.close();
        });
    }

    #[test]
    fn a_press_held_into_a_rest_does_not_tap_the_parked_screen() {
        lvgl(|lv| {
            let mut rt = booted_tapping(lv);
            let pointer = Pointer::create();
            send(&mut rt, UiEvent::Navigate(Route::Menu), 2_000);
            let (x, y) = EMPTY;
            assert!(
                press_target(rt.root().unwrap(), x, y).is_some(),
                "Menu's button"
            );
            iterate(&mut rt, &pointer, down(x, y), ready(2_100), true);
            // The power key rests the screen while the finger is still down:
            // Menu is parked, so its pressed button is not deleted.
            let short = [DeviceEvent::PowerKey(PowerKeyPress::Short)];
            assert!(
                iterate_with(&mut rt, &pointer, down(x, y), ready(2_150), &short, true),
                "the resting screen replaced Menu under the press"
            );
            assert_eq!(rt.shown_route(), Some(&Route::Resting));
            iterate(&mut rt, &pointer, down(x, y), ready(2_200), true);
            iterate(&mut rt, &pointer, up(), ready(2_250), true);
            assert_eq!(taps(), 0, "the parked Menu's button is not tapped");
            drop(pointer);
            rt.close();
        });
    }

    #[test]
    fn a_pressed_object_deleted_with_its_screen_never_clicks() {
        lvgl(|lv| {
            let mut rt = booted_tapping(lv);
            let pointer = Pointer::create();
            let (x, y) = (KNOB.x + 50, KNOB.y + 30);
            assert!(press_target(rt.root().unwrap(), x, y).is_some(), "the knob");
            // The runtime's cancel is left out on purpose: this pins LVGL's
            // own guard (deleting the pressed object waits for the release).
            iterate(&mut rt, &pointer, down(x, y), ready(2_000), false);
            iterate(&mut rt, &pointer, down(x, y), recording(2_050), false);
            assert_eq!(rt.shown_route(), Some(&Route::Recording));
            assert!(
                press_target(rt.root().unwrap(), x, y).is_some(),
                "a button now"
            );
            iterate(&mut rt, &pointer, up(), recording(2_100), false);
            assert_eq!(taps(), 0);
            drop(pointer);
            rt.close();
        });
    }

    #[test]
    fn a_screen_change_under_a_press_cancels_it_and_its_swipe() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            let (x, y) = (200, 20);
            assert_eq!(press_target(rt.root().unwrap(), x, y), None);
            assert!(!rt.touch(down(x, y), &ready(2_000)).cancel_press);
            rt.step(recording(2_050), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Recording));
            assert!(rt.take_cancel_press(), "the screen changed under the press");
            assert!(!rt.take_cancel_press(), "asked once");
            // Down 70 px from the top edge would open Menu on Recording.
            assert!(
                !rt.touch(down(x, y + 70), &recording(2_100)).cancel_press,
                "no swipe for the rest of the press"
            );
            rt.touch(up(), &recording(2_150));
            rt.step(recording(2_200), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Recording));
            assert!(!rt.take_cancel_press());

            assert!(!rt.touch(down(x, y), &recording(2_500)).cancel_press);
            assert!(
                rt.touch(down(x, y + 70), &recording(2_600)).cancel_press,
                "a press that starts after the release swipes"
            );
            rt.touch(up(), &recording(2_650));
            rt.step(recording(2_700), &[]);
            assert_eq!(rt.shown_route(), Some(&Route::Menu));
            assert!(!rt.take_cancel_press(), "released before the change");
            rt.close();
        });
    }

    #[test]
    fn the_top_screen_updates_every_step_with_the_steps_time() {
        lvgl(|lv| {
            let mut rt = booted(lv);
            assert_eq!(LAST_NOW.with(Cell::get), Some(BOOT_MIN_MS));
            rt.step(ready(5_000), &[]);
            assert_eq!(LAST_NOW.with(Cell::get), Some(5_000));
            rt.step(ready(5_010), &[]);
            assert_eq!(
                LAST_NOW.with(Cell::get),
                Some(5_010),
                "an unchanged snapshot too"
            );
            rt.close();
        });
    }
}
