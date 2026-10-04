//! Which screen is visible: a stack over a meeting-driven base.

use crate::kit::dispatch::UiEvent;
use crate::kit::route::Route;
use crate::kit::snapshot::UiSnapshot;

pub struct Router {
    stack: Vec<Route>,
    unlocked: bool,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Self {
        Self {
            stack: vec![Route::Boot],
            unlocked: false,
        }
    }

    pub fn current(&self) -> &Route {
        self.stack.last().expect("the stack always has a base")
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// The meeting-driven base screen.
    pub fn base_for(snap: &UiSnapshot) -> Route {
        if snap.meeting.recording {
            Route::Recording
        } else if snap.meeting.pending_decision.is_some() {
            Route::Decision
        } else {
            Route::Home
        }
    }

    pub fn handle(&mut self, event: &UiEvent, snap: &UiSnapshot) {
        if self.stack == [Route::Boot] {
            if *event == UiEvent::BootDone {
                self.stack[0] = if snap.settings.first_run_done {
                    Self::base_for(snap)
                } else {
                    Route::FirstRun
                };
            }
            return;
        }
        match event {
            UiEvent::Navigate(route) => {
                if route.is_base() {
                    self.stack.truncate(1);
                    return;
                }
                if matches!(
                    route,
                    Route::Resting | Route::Boot | Route::FirstRun | Route::Pin(_)
                ) {
                    return;
                }
                if *self.current() == Route::Resting {
                    self.stack.pop();
                }
                // A locked screen opens without the PIN only while unlocked
                // or when its entry point is already open below it.
                let guarded = snap.settings.pin_enabled
                    && !self.unlocked
                    && route
                        .guarded_by()
                        .is_some_and(|entry| !self.stack.contains(&entry));
                let target = if guarded {
                    Route::Pin(Box::new(route.clone()))
                } else {
                    route.clone()
                };
                if *self.current() != target {
                    self.stack.push(target);
                }
            }
            UiEvent::Back => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                }
            }
            UiEvent::ToBase => self.stack.truncate(1),
            UiEvent::PinAccepted => {
                if let Some(Route::Pin(target)) = self.stack.last().cloned() {
                    *self.stack.last_mut().expect("non-empty") = *target;
                    self.unlocked = true;
                }
            }
            UiEvent::Rest => {
                self.unlocked = false;
                // Waking must not reopen a locked screen: drop the first
                // PIN-guarded screen (entry point or child) and everything
                // above it. A PIN prompt stays and asks again on waking.
                if snap.settings.pin_enabled {
                    if let Some(at) = self
                        .stack
                        .iter()
                        .skip(1)
                        .position(|route| route.guarded_by().is_some())
                    {
                        self.stack.truncate(at + 1);
                    }
                }
                if *self.current() != Route::Resting {
                    self.stack.push(Route::Resting);
                }
            }
            UiEvent::Wake => {
                if *self.current() == Route::Resting {
                    self.stack.pop();
                }
            }
            UiEvent::BootDone | UiEvent::Show(_) => {}
        }
    }

    /// Follow the meeting state: swap the base and keep sub-screens on top,
    /// except a pending decision, which takes over the whole stack.
    pub fn sync(&mut self, snap: &UiSnapshot) {
        match self.stack[0] {
            Route::Boot => return,
            Route::FirstRun if !snap.settings.first_run_done => return,
            _ => {}
        }
        let base = Self::base_for(snap);
        if base == Route::Decision && self.stack[0] != Route::Decision {
            self.stack = vec![Route::Decision];
        } else if self.stack[0] != base {
            self.stack[0] = base;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::route::Route;
    use std::path::PathBuf;
    use uuid::Uuid;
    use waved_state::PendingDecision;

    fn ready() -> UiSnapshot {
        let mut snap = UiSnapshot::default();
        snap.settings.first_run_done = true;
        snap
    }

    fn started(snap: &UiSnapshot) -> Router {
        let mut router = Router::new();
        router.handle(&UiEvent::BootDone, snap);
        router
    }

    fn decision() -> PendingDecision {
        PendingDecision {
            id: Uuid::new_v4(),
            audio_path: PathBuf::from("/mnt/udisk/recordings/x/audio.wav"),
            duration_secs: 2537,
            size_bytes: 184_000_000,
            endpoint: "https://notes.example.com/hook".into(),
        }
    }

    #[test]
    fn boot_leads_to_first_start_until_it_is_done() {
        let mut snap = UiSnapshot::default();
        let mut router = Router::new();
        assert_eq!(router.current(), &Route::Boot);
        router.handle(&UiEvent::BootDone, &snap);
        assert_eq!(router.current(), &Route::FirstRun);
        snap.settings.first_run_done = true;
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Home);
    }

    #[test]
    fn navigation_walks_the_stack_and_back_stops_at_the_base() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Navigate(Route::AudioMode), &snap);
        router.handle(&UiEvent::Navigate(Route::AudioMode), &snap);
        assert_eq!((router.current(), router.depth()), (&Route::AudioMode, 3));
        router.handle(&UiEvent::Back, &snap);
        router.handle(&UiEvent::Back, &snap);
        router.handle(&UiEvent::Back, &snap);
        assert_eq!((router.current(), router.depth()), (&Route::Home, 1));
    }

    #[test]
    fn locked_routes_ask_for_the_pin_once_until_the_screen_rests() {
        let mut snap = ready();
        snap.settings.pin_enabled = true;
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        assert_eq!(router.current(), &Route::Pin(Box::new(Route::Files)));
        router.handle(&UiEvent::PinAccepted, &snap);
        assert_eq!(router.current(), &Route::Files);
        router.handle(&UiEvent::ToBase, &snap);
        router.handle(&UiEvent::Navigate(Route::System), &snap);
        assert_eq!(router.current(), &Route::System, "still unlocked");
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Wake, &snap);
        assert_eq!(router.current(), &Route::Home, "the locked screen is gone");
        router.handle(&UiEvent::Navigate(Route::Uploads), &snap);
        assert_eq!(
            router.current(),
            &Route::Pin(Box::new(Route::Uploads)),
            "resting relocks"
        );
    }

    #[test]
    fn no_pin_means_no_pin_screen() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        assert_eq!(router.current(), &Route::Files);
    }

    #[test]
    fn a_recording_that_starts_under_a_submenu_replaces_the_base() {
        let mut snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        snap.meeting.recording = true;
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Menu, "the open menu stays");
        router.handle(&UiEvent::Back, &snap);
        assert_eq!(router.current(), &Route::Recording);
    }

    #[test]
    fn a_pending_decision_preempts_everything() {
        let mut snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Rest, &snap);
        snap.meeting.pending_decision = Some(decision());
        router.sync(&snap);
        assert_eq!((router.current(), router.depth()), (&Route::Decision, 1));
        snap.meeting.pending_decision = None;
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Home);
    }

    #[test]
    fn resting_sits_on_top_and_waking_returns() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Rest, &snap);
        assert_eq!((router.current(), router.depth()), (&Route::Resting, 2));
        router.handle(&UiEvent::Wake, &snap);
        assert_eq!(router.current(), &Route::Home);
    }

    #[test]
    fn nothing_navigates_away_from_boot() {
        let snap = ready();
        let mut router = Router::new();
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Rest, &snap);
        router.sync(&snap);
        assert_eq!(router.current(), &Route::Boot);
    }

    #[test]
    fn waking_never_reopens_a_locked_screen() {
        let mut snap = ready();
        snap.settings.pin_enabled = true;
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Navigate(Route::System), &snap);
        router.handle(&UiEvent::PinAccepted, &snap);
        router.handle(&UiEvent::Navigate(Route::Display), &snap);
        assert_eq!(router.current(), &Route::Display);
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Wake, &snap);
        assert_eq!(router.current(), &Route::Menu);
        router.handle(&UiEvent::Navigate(Route::System), &snap);
        assert_eq!(router.current(), &Route::Pin(Box::new(Route::System)));
    }

    fn locked() -> UiSnapshot {
        let mut snap = ready();
        snap.settings.pin_enabled = true;
        snap
    }

    #[test]
    fn direct_navigation_to_a_child_asks_for_the_pin() {
        use crate::kit::route::ConfirmKind;
        let snap = locked();
        let id = Uuid::new_v4();
        for child in [
            Route::FileDetail(id),
            Route::Confirm(ConfirmKind::DeleteRecording(id)),
            Route::WifiSetup,
            Route::UploadMode,
            Route::Display,
            Route::DateTime,
            Route::About,
            Route::Update,
            Route::UsbStorage,
            Route::Confirm(ConfirmKind::FactoryReset),
        ] {
            for path in [vec![], vec![Route::Menu]] {
                let mut router = started(&snap);
                for route in path {
                    router.handle(&UiEvent::Navigate(route), &snap);
                }
                router.handle(&UiEvent::Navigate(child.clone()), &snap);
                assert_eq!(router.current(), &Route::Pin(Box::new(child.clone())));
            }
        }
    }

    #[test]
    fn a_child_opened_through_its_entry_point_does_not_ask_again() {
        let snap = locked();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        router.handle(&UiEvent::PinAccepted, &snap);
        router.handle(&UiEvent::Navigate(Route::FileDetail(Uuid::new_v4())), &snap);
        assert!(matches!(router.current(), Route::FileDetail(_)));
        assert_eq!(router.depth(), 3);
    }

    #[test]
    fn resting_drops_a_child_opened_without_its_entry_point() {
        let snap = locked();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Navigate(Route::FileDetail(Uuid::new_v4())), &snap);
        router.handle(&UiEvent::PinAccepted, &snap);
        assert!(matches!(router.current(), Route::FileDetail(_)));
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Wake, &snap);
        assert_eq!((router.current(), router.depth()), (&Route::Menu, 2));
    }

    #[test]
    fn a_pin_prompt_survives_rest_and_asks_again_on_wake() {
        let snap = locked();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Wake, &snap);
        assert_eq!(router.current(), &Route::Pin(Box::new(Route::Files)));
        router.handle(&UiEvent::PinAccepted, &snap);
        assert_eq!(router.current(), &Route::Files);
    }

    #[test]
    fn power_off_never_asks_for_the_pin() {
        use crate::kit::route::ConfirmKind;
        let snap = locked();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Confirm(ConfirmKind::PowerOff)), &snap);
        assert_eq!(router.current(), &Route::Confirm(ConfirmKind::PowerOff));
    }

    #[test]
    fn double_tapping_a_locked_entry_asks_for_the_pin_once() {
        let mut snap = ready();
        snap.settings.pin_enabled = true;
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        router.handle(&UiEvent::Navigate(Route::Files), &snap);
        assert_eq!(router.depth(), 2);
    }

    #[test]
    fn navigate_only_pushes_screens_that_belong_on_the_stack() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        router.handle(&UiEvent::Navigate(Route::Home), &snap);
        assert_eq!((router.current(), router.depth()), (&Route::Home, 1));
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        for route in [
            Route::Resting,
            Route::Boot,
            Route::FirstRun,
            Route::Pin(Box::new(Route::Files)),
        ] {
            router.handle(&UiEvent::Navigate(route), &snap);
            assert_eq!((router.current(), router.depth()), (&Route::Menu, 2));
        }
    }

    #[test]
    fn navigating_while_resting_leaves_the_resting_screen() {
        let snap = ready();
        let mut router = started(&snap);
        router.handle(&UiEvent::Rest, &snap);
        router.handle(&UiEvent::Navigate(Route::Menu), &snap);
        assert_eq!((router.current(), router.depth()), (&Route::Menu, 2));
        router.handle(&UiEvent::Back, &snap);
        assert_eq!(router.current(), &Route::Home);
    }
}
