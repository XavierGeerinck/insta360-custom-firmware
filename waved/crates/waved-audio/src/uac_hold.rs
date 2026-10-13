//! USB storage's hold on the UAC card `hw:1` (`StateBus::uac_hold`,
//! `docs/USB_STORAGE.md`) for a thread that owns one handle on it.
//!
//! The f_uac2 unbind of a gadget switch (Linux 5.4 `g_audio_cleanup` →
//! `snd_card_free`) disconnects the card and then waits, uninterruptibly,
//! until every file on it is closed. ALSA's disconnect makes calls on those
//! files fail (no use-after-free, unlike `f_hid`), but a handle nobody calls
//! is never closed: the UAC upstream sits on its prepared playback PCM while
//! the computer does not read the microphone, so `echo none > UDC` would
//! wait for good. So the upstream follows the hold: when it is raised the
//! handle is closed, that is confirmed, and nothing is opened until it is
//! lowered ([`run`]).
//!
//! Not on the hold, because the disconnect closes them by itself: the
//! downstream bridge (its blocking read is woken with `EBADFD` and it drops
//! the PCM before its back-off) and the mixer poll (its 50 ms tick fails
//! with `ENODEV` and drops the mixer). The downstream could not confirm a
//! hold anyway while the computer plays nothing: its read blocks until then.

use std::time::Duration;

use tracing::{info, warn};
use waved_state::GadgetHoldUser;

/// How often a held thread looks at the hold again.
pub const HOLD_CHECK: Duration = Duration::from_millis(200);

/// How a pump on an open handle ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Pumped<E> {
    /// The hold is raised: close the handle now.
    Held,
    /// It stopped (with the error, if any): open again after the back-off.
    Stopped(Option<E>),
    /// Its source is gone for good: the thread ends.
    Done,
}

/// A thread's side of the hold (`None`: no switch to follow).
pub struct HoldFollower {
    user: Option<GadgetHoldUser>,
}

impl HoldFollower {
    pub fn new(user: Option<GadgetHoldUser>) -> Self {
        Self { user }
    }

    /// Raised now: look before each use of the handle.
    pub fn is_held(&self) -> bool {
        self.user.as_ref().is_some_and(|user| user.held().is_some())
    }

    /// With nothing open: while the hold is raised, confirm it (each raise)
    /// and look again every `every`. True if it waited.
    pub fn wait_out(&self, every: Duration, sleep: &mut impl FnMut(Duration)) -> bool {
        let Some(user) = &self.user else {
            return false;
        };
        let mut waited = false;
        while let Some(generation) = user.held() {
            user.confirm(generation);
            waited = true;
            sleep(every);
        }
        waited
    }
}

/// The loop of a thread that owns one handle on `hw:1`: open, pump until it
/// stops, back off `retry`, open again. While the hold is raised nothing is
/// open: a pump that sees it returns [`Pumped::Held`], the handle is closed,
/// and only then is the hold confirmed. Returns on [`Pumped::Done`].
pub fn run<T, E: std::fmt::Display>(
    name: &str,
    hold: &HoldFollower,
    retry: Duration,
    mut open: impl FnMut() -> Result<T, E>,
    mut pump: impl FnMut(&T) -> Pumped<E>,
    mut sleep: impl FnMut(Duration),
) {
    loop {
        // Nothing is open here.
        if hold.wait_out(HOLD_CHECK, &mut sleep) {
            info!("{name}: the USB switch is over; opening the card again");
        }
        let pumped = match open() {
            Ok(handle) => pump(&handle),
            Err(e) => {
                warn!("{name} open failed: {e}; retrying in {} s", retry.as_secs_f32());
                sleep(retry);
                continue;
            }
        };
        // The handle is closed: a hold is confirmed only from here on.
        match pumped {
            Pumped::Held => info!("{name}: USB is switching; the card is closed until it is back"),
            Pumped::Stopped(error) => {
                if let Some(e) = error {
                    warn!("{name} pump stopped: {e}");
                }
                sleep(retry);
            }
            Pumped::Done => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use waved_state::GadgetHold;

    /// A handle on the card that counts how many are open.
    struct FakePcm(Rc<Cell<u32>>);

    impl Drop for FakePcm {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    fn opener(open: &Rc<Cell<u32>>, opens: &Rc<Cell<u32>>) -> impl FnMut() -> Result<FakePcm, String> {
        let (open, opens) = (Rc::clone(open), Rc::clone(opens));
        move || {
            opens.set(opens.get() + 1);
            open.set(open.get() + 1);
            Ok(FakePcm(Rc::clone(&open)))
        }
    }

    #[test]
    fn a_raise_closes_the_handle_before_it_is_confirmed_and_nothing_opens_until_lowered() {
        let hold = GadgetHold::new();
        let follower = HoldFollower::new(Some(hold.register("the UAC upstream")));
        let (open, opens) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let pumps = Cell::new(0);
        let pump = |_: &FakePcm| {
            pumps.set(pumps.get() + 1);
            match pumps.get() {
                1 => {
                    // The switch raises the hold while the handle is open.
                    hold.raise();
                    assert_eq!(open.get(), 1);
                    assert_eq!(hold.waiting_for(), ["the UAC upstream"], "not confirmed while open");
                    assert!(follower.is_held());
                    Pumped::Held
                }
                _ => Pumped::Done,
            }
        };
        let sleeps = RefCell::new(Vec::new());
        let sleep = |d: Duration| {
            sleeps.borrow_mut().push(d);
            assert_eq!(open.get(), 0, "nothing open while held");
            assert!(hold.all_closed(), "confirmed");
            assert_eq!(opens.get(), 1, "no open while held");
            if sleeps.borrow().len() == 2 {
                hold.lower();
            }
        };
        run("uac-upstream", &follower, Duration::from_secs(1), opener(&open, &opens), pump, sleep);
        assert_eq!(*sleeps.borrow(), [HOLD_CHECK, HOLD_CHECK]);
        assert_eq!(opens.get(), 2, "opened again once lowered");
        assert_eq!(open.get(), 0);
    }

    #[test]
    fn a_hold_raised_during_the_back_off_is_confirmed_before_the_next_open() {
        let hold = GadgetHold::new();
        let follower = HoldFollower::new(Some(hold.register("the UAC upstream")));
        let opens = Cell::new(0);
        let open = || {
            opens.set(opens.get() + 1);
            if opens.get() == 1 {
                Err("no card".to_string())
            } else {
                assert!(!hold.signal().held, "never opened while held");
                Ok(())
            }
        };
        let sleeps = RefCell::new(Vec::new());
        let sleep = |d: Duration| {
            sleeps.borrow_mut().push(d);
            match sleeps.borrow().len() {
                1 => {
                    hold.raise();
                }
                2 => {
                    assert!(hold.all_closed());
                    hold.lower();
                }
                _ => {}
            }
        };
        run("uac-upstream", &follower, Duration::from_secs(1), open, |_: &()| Pumped::<String>::Done, sleep);
        assert_eq!(*sleeps.borrow(), [Duration::from_secs(1), HOLD_CHECK]);
        assert_eq!(opens.get(), 2);
    }

    #[test]
    fn each_new_raise_during_the_wait_is_confirmed() {
        let hold = GadgetHold::new();
        let follower = HoldFollower::new(Some(hold.register("the UAC upstream")));
        let first = hold.raise();
        let sleeps = Cell::new(0);
        let mut sleep = |_: Duration| {
            sleeps.set(sleeps.get() + 1);
            match sleeps.get() {
                1 => {
                    assert!(hold.all_closed());
                    hold.lower();
                    assert!(hold.raise() > first);
                    assert!(!hold.all_closed(), "a new raise");
                }
                2 => {
                    assert!(hold.all_closed(), "confirmed again");
                    hold.lower();
                }
                _ => panic!("waited past the lower"),
            }
        };
        assert!(follower.wait_out(HOLD_CHECK, &mut sleep));
        assert!(!follower.wait_out(HOLD_CHECK, &mut sleep), "not held: no wait");
    }

    #[test]
    fn without_a_hold_it_opens_and_pumps_as_before() {
        let follower = HoldFollower::new(None);
        assert!(!follower.is_held());
        let (open, opens) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let pumps = Cell::new(0);
        let pump = |_: &FakePcm| {
            pumps.set(pumps.get() + 1);
            if pumps.get() == 1 {
                Pumped::Stopped(Some("EPIPE".to_string()))
            } else {
                Pumped::Done
            }
        };
        let sleeps = RefCell::new(Vec::new());
        run(
            "uac-upstream",
            &follower,
            Duration::from_secs(1),
            opener(&open, &opens),
            pump,
            |d| sleeps.borrow_mut().push(d),
        );
        assert_eq!(*sleeps.borrow(), [Duration::from_secs(1)], "the back-off, nothing else");
        assert_eq!(opens.get(), 2);
        assert_eq!(open.get(), 0);
    }
}
