//! The UAC2 mixer poll (`uac::control`, direction host → Wave) without ALSA:
//! what one 50 ms poll of the gadget card's mixer (`hw:1`) sends to the bus,
//! and how it gets the card back after a gadget switch (`MixerPoll`). Kept
//! out of the ALSA-gated `uac` module so it is tested on every host, like
//! `repeat_filter`.

use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::{Duration, Instant};

/// One read of the card's controls. `None`: the control is not on this card
/// (kernel 5.4 `f_uac2` surfaces no mic-side volume or switch).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MixerReading {
    /// `PCM Capture Volume`: the Mac's speaker slider (see the direction
    /// table in `uac::control`).
    pub speaker_volume: Option<u16>,
    /// `PCM Playback Volume`: the mic gain.
    pub mic_volume: Option<u16>,
    /// `PCM Capture Switch`: where macOS F10 lands.
    pub speaker_muted: Option<bool>,
    /// `PCM Playback Switch`.
    pub mic_muted: Option<bool>,
    /// `PCM Playback State`: the host reads the uplink.
    pub upstream_active: Option<bool>,
    /// `PCM Capture State`: the host writes the downlink.
    pub downstream_active: Option<bool>,
}

/// Wave's own levels as `AudioState` has them: what the state mirror
/// writes into the card.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WaveLevels {
    pub speaker_volume: u16,
    pub mic_volume: u16,
    pub speaker_muted: bool,
    pub mic_muted: bool,
}

/// An open mixer on the gadget card.
pub trait HostMixer {
    /// Take in the card's pending events (non-blocking). An error means this
    /// handle is dead: the card went away.
    fn handle_events(&mut self) -> Result<(), String>;
    /// The controls as the last events left them.
    fn read(&self) -> MixerReading;
    /// Write Wave's levels into the card, as the state mirror does.
    fn write_levels(&mut self, levels: WaveLevels);
}

/// The gadget card, by name (`hw:1`).
pub trait MixerCard {
    type Mixer: HostMixer;
    fn open(&mut self) -> Result<Self::Mixer, String>;
}

/// What waved last wrote to the mixer itself, so the event of its own write
/// is not dispatched again as the host's. Shared by the poll and the state
/// mirror.
#[derive(Debug, Default)]
pub struct LastWrite {
    pub speaker_volume: AtomicU16,
    pub mic_volume: AtomicU16,
    pub speaker_muted: AtomicBool,
    pub mic_muted: AtomicBool,
}

/// A change the host made, for the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostChange {
    SpeakerVolume(u16),
    MicVolume(u16),
    SpeakerMute(bool),
    MicMute(bool),
}

/// What the poll has to say for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollNote {
    /// The first failure of an outage (warn).
    Lost(String),
    /// Another failure of the same outage (debug).
    StillDown(String),
    /// The card is open again after an outage, with Wave's levels in it (info).
    Back,
}

/// One poll's results.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PollOutput {
    pub changes: Vec<HostChange>,
    pub upstream_active: Option<bool>,
    pub downstream_active: Option<bool>,
    pub note: Option<PollNote>,
}

/// The values the last poll saw, to emit only on change.
#[derive(Debug, Default, Clone, Copy)]
struct Seen {
    speaker_volume: u16,
    mic_volume: u16,
    speaker_muted: bool,
    mic_muted: bool,
}

impl Seen {
    fn from_reading(r: &MixerReading) -> Self {
        Self {
            speaker_volume: r.speaker_volume.unwrap_or(0),
            mic_volume: r.mic_volume.unwrap_or(0),
            speaker_muted: r.speaker_muted.unwrap_or(false),
            mic_muted: r.mic_muted.unwrap_or(false),
        }
    }
}

/// How long the poll waits after a failure before it opens the card again.
pub const REOPEN_EVERY: Duration = Duration::from_secs(1);

/// The poll: call `tick` every 50 ms.
///
/// A USB storage session (`waved-system` `usb_storage`) unbinds the gadget
/// and binds it again, on the way in and on the way out: f_uac2's card is
/// freed and a new one created. The old handle then fails every
/// `handle_events` (ENODEV) while its cached values stay as they were, so the
/// poll drops it, opens the card again once a second until there is one, and
/// writes Wave's levels into the new card (it starts with the kernel's
/// defaults, and the state mirror's writes went nowhere meanwhile) before it
/// reads what it holds. The new card's defaults are never taken for the
/// Mac's changes.
pub struct MixerPoll<C: MixerCard> {
    card: C,
    mixer: Option<C::Mixer>,
    seen: Seen,
    /// No open before this, after a failure.
    retry_at: Option<Instant>,
    /// Failures since the card last opened; 0 while it works.
    failures: u32,
}

impl<C: MixerCard> MixerPoll<C> {
    pub fn new(card: C) -> Self {
        Self { card, mixer: None, seen: Seen::default(), retry_at: None, failures: 0 }
    }

    /// One poll. `wave` gives Wave's levels (asked only when a card comes
    /// back).
    pub fn tick(
        &mut self,
        now: Instant,
        last_write: &LastWrite,
        wave: impl FnOnce() -> WaveLevels,
    ) -> PollOutput {
        let mut out = PollOutput::default();
        if self.mixer.is_none() {
            if self.retry_at.is_some_and(|t| now < t) {
                return out;
            }
            match self.card.open() {
                Ok(mut m) => {
                    if self.failures > 0 {
                        let levels = wave();
                        m.write_levels(levels);
                        last_write.speaker_volume.store(levels.speaker_volume, Ordering::Relaxed);
                        last_write.mic_volume.store(levels.mic_volume, Ordering::Relaxed);
                        last_write.speaker_muted.store(levels.speaker_muted, Ordering::Relaxed);
                        last_write.mic_muted.store(levels.mic_muted, Ordering::Relaxed);
                        self.failures = 0;
                        out.note = Some(PollNote::Back);
                    }
                    self.retry_at = None;
                    self.seen = Seen::from_reading(&m.read());
                    self.mixer = Some(m);
                }
                Err(e) => {
                    self.failed(now, format!("cannot be opened: {e}"), &mut out);
                    return out;
                }
            }
        }
        let Some(mixer) = self.mixer.as_mut() else { return out };
        if let Err(e) = mixer.handle_events() {
            // Closing the dead handle lets the kernel finish freeing the card.
            self.mixer = None;
            self.failed(now, format!("failed: {e}"), &mut out);
            out.upstream_active = Some(false);
            out.downstream_active = Some(false);
            return out;
        }
        let r = mixer.read();
        self.changes(&r, last_write, &mut out.changes);
        out.upstream_active = r.upstream_active;
        out.downstream_active = r.downstream_active;
        out
    }

    fn failed(&mut self, now: Instant, what: String, out: &mut PollOutput) {
        self.retry_at = Some(now + REOPEN_EVERY);
        self.failures += 1;
        out.note = Some(if self.failures == 1 { PollNote::Lost(what) } else { PollNote::StillDown(what) });
    }

    /// The host's changes since the last poll, minus waved's own writes.
    fn changes(&mut self, r: &MixerReading, last_write: &LastWrite, out: &mut Vec<HostChange>) {
        if let Some(v) = r.speaker_volume {
            if v != self.seen.speaker_volume {
                self.seen.speaker_volume = v;
                if last_write.speaker_volume.load(Ordering::Relaxed) != v {
                    out.push(HostChange::SpeakerVolume(v));
                }
            }
        }
        if let Some(v) = r.mic_volume {
            if v != self.seen.mic_volume {
                self.seen.mic_volume = v;
                if last_write.mic_volume.load(Ordering::Relaxed) != v {
                    out.push(HostChange::MicVolume(v));
                }
            }
        }
        if let Some(muted) = r.speaker_muted {
            if muted != self.seen.speaker_muted {
                self.seen.speaker_muted = muted;
                if last_write.speaker_muted.load(Ordering::Relaxed) != muted {
                    out.push(HostChange::SpeakerMute(muted));
                    // Track the host's value so its next toggle is a change
                    // against last_write (the state mirror writes nothing back
                    // when the mute goes to the mic).
                    last_write.speaker_muted.store(muted, Ordering::Relaxed);
                }
            }
        }
        if let Some(muted) = r.mic_muted {
            if muted != self.seen.mic_muted {
                self.seen.mic_muted = muted;
                if last_write.mic_muted.load(Ordering::Relaxed) != muted {
                    out.push(HostChange::MicMute(muted));
                    last_write.mic_muted.store(muted, Ordering::Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const TICK: Duration = Duration::from_millis(50);

    /// The kernel's side: f_uac2's card, which a gadget switch frees and
    /// creates again with the kernel's defaults.
    struct CardState {
        present: bool,
        /// Bumped each time the card is created again.
        generation: u32,
        values: MixerReading,
        opens: u32,
        writes: Vec<WaveLevels>,
    }

    fn defaults() -> MixerReading {
        MixerReading {
            speaker_volume: Some(100),
            mic_volume: None,
            speaker_muted: Some(false),
            mic_muted: None,
            upstream_active: Some(false),
            downstream_active: Some(false),
        }
    }

    #[derive(Clone)]
    struct Card(Rc<RefCell<CardState>>);

    impl Card {
        fn present() -> Self {
            Card(Rc::new(RefCell::new(CardState {
                present: true,
                generation: 0,
                values: defaults(),
                opens: 0,
                writes: Vec::new(),
            })))
        }
        fn absent() -> Self {
            let c = Self::present();
            c.0.borrow_mut().present = false;
            c
        }
        /// The unbind: the card is freed.
        fn unbind(&self) {
            self.0.borrow_mut().present = false;
        }
        /// The bind: a new card with the kernel's defaults.
        fn bind(&self) {
            let mut s = self.0.borrow_mut();
            s.present = true;
            s.generation += 1;
            s.values = defaults();
        }
        /// The host's SET_CUR on the speaker volume.
        fn mac_sets_speaker(&self, v: u16) {
            self.0.borrow_mut().values.speaker_volume = Some(v);
        }
        fn mac_sets_speaker_mute(&self, muted: bool) {
            self.0.borrow_mut().values.speaker_muted = Some(muted);
        }
        fn host_attached(&self) {
            let mut s = self.0.borrow_mut();
            s.values.upstream_active = Some(true);
            s.values.downstream_active = Some(true);
        }
        fn opens(&self) -> u32 {
            self.0.borrow().opens
        }
        fn writes(&self) -> Vec<WaveLevels> {
            self.0.borrow().writes.clone()
        }
    }

    struct Mixer {
        card: Rc<RefCell<CardState>>,
        generation: u32,
        /// What the handle saw last (alsa-lib's cached simple elements).
        cached: MixerReading,
    }

    impl HostMixer for Mixer {
        fn handle_events(&mut self) -> Result<(), String> {
            let s = self.card.borrow();
            if !s.present || s.generation != self.generation {
                return Err("No such device (ENODEV)".into());
            }
            self.cached = s.values;
            Ok(())
        }
        fn read(&self) -> MixerReading {
            self.cached
        }
        fn write_levels(&mut self, l: WaveLevels) {
            let mut s = self.card.borrow_mut();
            s.writes.push(l);
            if s.present && s.generation == self.generation {
                s.values.speaker_volume = s.values.speaker_volume.map(|_| l.speaker_volume);
                s.values.mic_volume = s.values.mic_volume.map(|_| l.mic_volume);
                s.values.speaker_muted = s.values.speaker_muted.map(|_| l.speaker_muted);
                s.values.mic_muted = s.values.mic_muted.map(|_| l.mic_muted);
                self.cached = s.values;
            }
        }
    }

    impl MixerCard for Card {
        type Mixer = Mixer;
        fn open(&mut self) -> Result<Mixer, String> {
            let mut s = self.0.borrow_mut();
            s.opens += 1;
            if !s.present {
                return Err("No such file or directory (ENOENT)".into());
            }
            Ok(Mixer { card: Rc::clone(&self.0), generation: s.generation, cached: s.values })
        }
    }

    /// The poll and its clock.
    struct Rig {
        poll: MixerPoll<Card>,
        card: Card,
        last_write: LastWrite,
        wave: WaveLevels,
        now: Instant,
    }

    impl Rig {
        fn new(card: Card) -> Self {
            Self {
                poll: MixerPoll::new(card.clone()),
                card,
                last_write: LastWrite::default(),
                wave: WaveLevels { speaker_volume: 40, mic_volume: 100, speaker_muted: true, mic_muted: false },
                now: Instant::now(),
            }
        }
        fn tick(&mut self) -> PollOutput {
            let wave = self.wave;
            let out = self.poll.tick(self.now, &self.last_write, || wave);
            self.now += TICK;
            out
        }
        /// Polls for `d` and returns every output.
        fn run(&mut self, d: Duration) -> Vec<PollOutput> {
            let n = (d.as_millis() / TICK.as_millis()) as usize;
            (0..n).map(|_| self.tick()).collect()
        }
    }

    fn changes(outs: &[PollOutput]) -> Vec<HostChange> {
        outs.iter().flat_map(|o| o.changes.iter().copied()).collect()
    }

    fn notes(outs: &[PollOutput]) -> Vec<PollNote> {
        outs.iter().filter_map(|o| o.note.clone()).collect()
    }

    #[test]
    fn the_macs_changes_reach_the_bus_and_waves_own_writes_do_not() {
        let mut rig = Rig::new(Card::present());
        assert!(rig.run(Duration::from_millis(200)).iter().all(|o| o.changes.is_empty()));
        assert!(rig.card.writes().is_empty(), "at start the state mirror seeds the card, not the poll");

        rig.card.mac_sets_speaker(55);
        assert_eq!(changes(&rig.run(Duration::from_millis(100))), vec![HostChange::SpeakerVolume(55)]);

        // Wave's own write (the state mirror) comes back as an event: not the Mac's.
        rig.last_write.speaker_volume.store(70, Ordering::Relaxed);
        rig.card.mac_sets_speaker(70);
        assert!(changes(&rig.run(Duration::from_millis(100))).is_empty());

        rig.card.mac_sets_speaker_mute(true);
        assert_eq!(changes(&rig.run(Duration::from_millis(100))), vec![HostChange::SpeakerMute(true)]);
        assert!(rig.last_write.speaker_muted.load(Ordering::Relaxed), "the host's mute is tracked");
        rig.card.mac_sets_speaker_mute(false);
        assert_eq!(changes(&rig.run(Duration::from_millis(100))), vec![HostChange::SpeakerMute(false)]);

        rig.card.host_attached();
        let out = rig.tick();
        assert_eq!((out.upstream_active, out.downstream_active), (Some(true), Some(true)));
        assert_eq!(rig.card.opens(), 1);
        assert!(notes(&rig.run(Duration::from_secs(1))).is_empty());
    }

    #[test]
    fn a_card_replaced_by_a_gadget_switch_is_reopened_and_the_mac_reaches_wave_again() {
        let mut rig = Rig::new(Card::present());
        rig.card.host_attached();
        rig.run(Duration::from_millis(200));

        rig.card.unbind();
        let lost = rig.tick();
        assert!(matches!(lost.note, Some(PollNote::Lost(_))), "{:?}", lost.note);
        assert_eq!(
            (lost.upstream_active, lost.downstream_active),
            (Some(false), Some(false)),
            "no host is attached to a card that is gone"
        );
        rig.card.bind();
        // Not before a second has passed since the failure.
        rig.run(Duration::from_millis(900));
        assert_eq!(rig.card.opens(), 1, "no reopen within the second");
        let outs = rig.run(Duration::from_millis(200));
        assert_eq!(rig.card.opens(), 2);
        assert_eq!(notes(&outs), vec![PollNote::Back]);

        rig.card.mac_sets_speaker(30);
        assert_eq!(changes(&rig.run(Duration::from_millis(100))), vec![HostChange::SpeakerVolume(30)]);
    }

    #[test]
    fn the_new_card_gets_waves_levels_and_its_defaults_are_not_taken_for_the_macs() {
        let mut rig = Rig::new(Card::present());
        rig.run(Duration::from_millis(200));
        rig.card.unbind();
        rig.tick();
        rig.card.bind(); // speaker 100, unmuted: the kernel's defaults
        let outs = rig.run(Duration::from_secs(3));
        assert!(changes(&outs).is_empty(), "the new card's defaults are not the Mac's: {:?}", changes(&outs));
        assert_eq!(rig.card.writes(), vec![rig.wave], "Wave's levels go into the new card");
        assert_eq!(rig.last_write.speaker_volume.load(Ordering::Relaxed), 40);
        assert!(rig.last_write.speaker_muted.load(Ordering::Relaxed));

        // The Mac's next change still lands.
        rig.card.mac_sets_speaker(45);
        assert_eq!(changes(&rig.run(Duration::from_millis(100))), vec![HostChange::SpeakerVolume(45)]);
    }

    #[test]
    fn a_card_missing_at_start_is_tried_once_a_second_until_it_appears() {
        let mut rig = Rig::new(Card::absent());
        let outs = rig.run(Duration::from_secs(5));
        assert!((5..=6).contains(&rig.card.opens()), "{} opens in 5 s", rig.card.opens());
        let notes = notes(&outs);
        assert!(matches!(notes[0], PollNote::Lost(_)));
        assert!(notes[1..].iter().all(|n| matches!(n, PollNote::StillDown(_))), "logged once: {notes:?}");

        rig.card.bind();
        let outs = rig.run(Duration::from_millis(1100));
        assert_eq!(super::tests::notes(&outs), vec![PollNote::Back]);
        assert_eq!(rig.card.writes(), vec![rig.wave], "the state mirror's seed found no card: the poll seeds it");
        rig.card.mac_sets_speaker(20);
        assert_eq!(changes(&rig.run(Duration::from_millis(100))), vec![HostChange::SpeakerVolume(20)]);
    }

    #[test]
    fn a_whole_storage_session_is_logged_once_per_outage() {
        let mut rig = Rig::new(Card::present());
        rig.run(Duration::from_millis(200));
        let mut outs = Vec::new();
        // Into storage mode: unbind, 2 s, bind; out again: unbind, 4 s, bind.
        for gap in [2, 4] {
            rig.card.unbind();
            outs.extend(rig.run(Duration::from_secs(gap)));
            rig.card.bind();
            outs.extend(rig.run(Duration::from_secs(2)));
        }
        let loud: Vec<_> = notes(&outs).into_iter().filter(|n| !matches!(n, PollNote::StillDown(_))).collect();
        assert_eq!(loud.len(), 4, "{loud:?}");
        assert!(matches!(loud[0], PollNote::Lost(_)));
        assert_eq!(loud[1], PollNote::Back);
        assert!(matches!(loud[2], PollNote::Lost(_)));
        assert_eq!(loud[3], PollNote::Back);
        assert!(changes(&outs).is_empty());
    }
}
