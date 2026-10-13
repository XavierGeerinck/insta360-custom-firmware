//! UAC2 control — bridge `hw:1` ALSA mixer events to `Command` and back.
//!
//! Direction 1 (host → device): the kernel's `f_uac2` driver translates UAC2
//! SET_CUR class requests into ALSA mixer value events on card 1. We poll
//! for events at 50 ms cadence and dispatch `Command::Set*Volume` /
//! `Command::Toggle*Mute` onto the bus.
//!
//! **Naming gotcha**: the kernel's `f_uac2` exposes controls from the
//! *gadget's* perspective, which is inverted from the host's:
//!
//! | configfs           | ALSA control          | Direction       | Mac calls it |
//! |--------------------|-----------------------|-----------------|--------------|
//! | `c_volume_*`       | `PCM Capture Volume`  | host → gadget   | **speaker**  |
//! | `p_volume_*`       | `PCM Playback Volume` | gadget → host   | **mic**      |
//!
//! So in this file `read_volume(..., capture=true)` returns the *speaker*
//! volume that Mac set, NOT the mic volume. This matches what the stock
//! firmware does (`handle_usb_msg_threadLoop` in `libins_entry.so` reads
//! "PCM Capture Volume" and pushes it to `audio_server_spk_volume_policy`).
//!
//! Direction 2 (device → host): when `AudioState.speaker_volume` changes via
//! the touch UI or `/tmp/waved-volume`, we write the new value into the same
//! mixer control. The kernel reports it back to the host on the next GET_CUR.
//!
//! Both directions go through a shared `LastWrite` cell so a self-write
//! event isn't re-dispatched as a host event.
//!
//! The card is freed and created again whenever the gadget is unbound and
//! bound again (USB storage, `waved-system` `usb_storage`). The poll then
//! drops its dead handle, opens the card again once a second and writes
//! Wave's levels into the new one (`mixer_poll::MixerPoll`, tested without
//! ALSA). The state mirror opens the card for each write and follows by
//! itself.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alsa::mixer::{Mixer, SelemId, SelemChannelId};
use tokio::sync::{broadcast, watch};
use tokio::time::interval;
use tracing::{debug, info, warn};

use waved_state::{AudioState, Command};

use super::UacStats;
use crate::audio_config::HostMuteTarget;
use crate::mixer_poll::{
    HostChange, HostMixer, LastWrite, MixerCard, MixerPoll, MixerReading, PollNote, WaveLevels,
};

/// Internal percent ceiling for the mic gain path (0..=MIC_MAX_PCT).
/// 800 = 8× / +18 dB. Matches `apply_mic_volume`'s cap in `lib.rs`.
/// If you change this, also change the `.min(...)` in `apply_mic_volume`
/// and the `.min(...)` in `stats_collector::compute_stats_with_processed`.
const MIC_MAX_PCT: u16 = 800;

/// Internal percent ceiling for the speaker volume path (0..=SPEAKER_MAX_PCT).
/// 100 means the host slider's full-scale corresponds to internal 100.
const SPEAKER_MAX_PCT: u16 = 100;

#[derive(Debug, Clone)]
pub struct UacControlConfig {
    pub mixer_card: String,    // "hw:1"
    pub host_mute_target: HostMuteTarget,
}

pub struct UacControl {
    _phantom: (),
}

impl UacControl {
    /// Spawn the mixer event poll task plus the state mirror task.
    /// Both live in the existing tokio runtime; the alsa::mixer calls are
    /// short-lived and do not block long enough to need a dedicated thread.
    ///
    /// `host_pending_speaker_vol` is the StateBus echo-suppress marker —
    /// see `StateBus::host_pending_speaker_vol`. We stamp it before
    /// dispatching `SetSpeakerVolume(v)` from host-side events so the HID
    /// `volume_watch` doesn't echo the same value back to Mac.
    pub fn spawn(
        cfg: UacControlConfig,
        commands: broadcast::Sender<Command>,
        state_rx: watch::Receiver<AudioState>,
        stats: Arc<UacStats>,
        host_pending_speaker_vol: Arc<AtomicI32>,
    ) -> Self {
        let last_write = Arc::new(LastWrite::default());

        // Direction 1: poll mixer → emit Command. Runs for the process
        // lifetime: a card that goes away (a USB storage gadget switch) is
        // opened again, see `mixer_poll::MixerPoll`.
        tokio::spawn({
            let cfg = cfg.clone();
            let stats = Arc::clone(&stats);
            let last_write = Arc::clone(&last_write);
            let state_rx = state_rx.clone();
            async move {
                run_mixer_poll(cfg, commands, stats, last_write, host_pending_speaker_vol, state_rx).await
            }
        });

        // Direction 2: state changes → mirror into mixer.
        tokio::spawn({
            let stats = Arc::clone(&stats);
            let last_write = Arc::clone(&last_write);
            let cfg = cfg.clone();
            async move {
                if let Err(e) = run_state_mirror(cfg, state_rx, stats, last_write).await {
                    warn!("uac-control state mirror exited: {e}");
                }
            }
        });

        Self { _phantom: () }
    }
}

/// The gadget card's mixer, opened by name (`hw:1`).
struct AlsaCard {
    name: String,
}

struct AlsaMixer(Mixer);

impl MixerCard for AlsaCard {
    type Mixer = AlsaMixer;
    fn open(&mut self) -> Result<AlsaMixer, String> {
        Mixer::new(&self.name, false).map(AlsaMixer).map_err(|e| e.to_string())
    }
}

impl HostMixer for AlsaMixer {
    fn handle_events(&mut self) -> Result<(), String> {
        // Non-blocking: alsa-lib attaches a mixer's ctl non-blocking. On a
        // card that went away it fails with ENODEV.
        self.0.handle_events().map(|_| ()).map_err(|e| e.to_string())
    }

    // Note the capture=true vs capture=false flip: see the gadget/host
    // inversion table in the module docstring.
    fn read(&self) -> MixerReading {
        MixerReading {
            // Mac → speaker volume lives on `PCM Capture Volume` (host writes
            // FU SET_CUR for its output stream; kernel exposes that as the
            // gadget-side capture control).
            speaker_volume: read_volume(&self.0, "PCM", true, SPEAKER_MAX_PCT),
            // Mac → mic gain on `PCM Playback Volume`. Kernel 5.4 `f_uac2`
            // doesn't expose this on the current build (p_volume_* configfs
            // sets it up but no ALSA control surfaces), so this is dormant —
            // `read_volume` returns None and we never dispatch. Kept here for
            // the eventual kernel patch that adds it.
            mic_volume: read_volume(&self.0, "PCM", false, MIC_MAX_PCT),
            // Mac → speaker mute on `PCM Capture Switch`. This is what macOS
            // F10 lands on for a UAC2 speakerphone. Redirected per
            // `host_mute_target` (`dispatch_host_mute`): default "speaker"
            // silences playback (music-listener UX), "mic" preserves the
            // headset semantic, "both" toggles both at once.
            speaker_muted: read_switch(&self.0, "PCM", true),
            // Mac → mic mute on `PCM Playback Switch`. Same kernel caveat as
            // `PCM Playback Volume` above — not surfaced on this build.
            mic_muted: read_switch(&self.0, "PCM", false),
            // SP#6: source-of-truth for host attachment is the kernel's PCM
            // Playback State / PCM Capture State switches.
            upstream_active: read_state_switch(&self.0, "PCM Playback State"),
            downstream_active: read_state_switch(&self.0, "PCM Capture State"),
        }
    }

    /// What the state mirror writes: speaker → capture side, mic → playback
    /// side.
    fn write_levels(&mut self, l: WaveLevels) {
        let _ = write_volume(&self.0, "PCM", true, l.speaker_volume, SPEAKER_MAX_PCT);
        let _ = write_volume(&self.0, "PCM", false, l.mic_volume, MIC_MAX_PCT);
        let _ = write_switch(&self.0, "PCM", true, l.speaker_muted);
        let _ = write_switch(&self.0, "PCM", false, l.mic_muted);
    }
}

async fn run_mixer_poll(
    cfg: UacControlConfig,
    commands: broadcast::Sender<Command>,
    stats: Arc<UacStats>,
    last_write: Arc<LastWrite>,
    host_pending_speaker_vol: Arc<AtomicI32>,
    state_rx: watch::Receiver<AudioState>,
) {
    info!("uac-control: opening mixer {}", cfg.mixer_card);
    let mut poll = MixerPoll::new(AlsaCard { name: cfg.mixer_card.clone() });
    let mut tick = interval(Duration::from_millis(50));
    loop {
        tick.tick().await;
        let out = poll.tick(Instant::now(), &last_write, || {
            let s = state_rx.borrow();
            WaveLevels {
                speaker_volume: s.speaker_volume,
                mic_volume: s.mic_volume,
                speaker_muted: s.speaker_muted,
                mic_muted: s.mic_muted,
            }
        });
        match out.note {
            Some(PollNote::Lost(what)) => warn!(
                "uac-control: mixer {} {what}; reopening every second, quietly until it is back",
                cfg.mixer_card
            ),
            Some(PollNote::StillDown(what)) => debug!("uac-control: mixer {} {what}", cfg.mixer_card),
            Some(PollNote::Back) => info!(
                "uac-control: mixer {} is back (a new card); Wave's levels written into it",
                cfg.mixer_card
            ),
            None => {}
        }
        for change in out.changes {
            match change {
                HostChange::SpeakerVolume(v) => {
                    info!("uac-control: host set speaker volume → {v}");
                    // Mark this value as "host-originated" so the HID
                    // volume_watch task skips echoing it back as
                    // ConsumerVolumeStep events (Mac F11 → us → Mac F11
                    // feedback loop). Stamped *before* dispatch so the
                    // marker is in place by the time AudioState changes.
                    host_pending_speaker_vol.store(v as i32, Ordering::Release);
                    let _ = commands.send(Command::SetSpeakerVolume(v));
                }
                HostChange::MicVolume(v) => {
                    info!("uac-control: host set mic volume → {v}");
                    let _ = commands.send(Command::SetMicVolume(v));
                }
                HostChange::SpeakerMute(muted) => {
                    info!("uac-control: host set speaker mute → {muted} (target={:?})", cfg.host_mute_target);
                    dispatch_host_mute(&commands, cfg.host_mute_target);
                }
                HostChange::MicMute(muted) => {
                    info!("uac-control: host set mic mute → {muted}");
                    dispatch_host_mute(&commands, cfg.host_mute_target);
                }
            }
            stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
        }
        // The *_active stats flags are updated here so the bridges don't
        // have to.
        if let Some(active) = out.upstream_active {
            stats.upstream_active.store(active, Ordering::Relaxed);
        }
        if let Some(active) = out.downstream_active {
            stats.downstream_active.store(active, Ordering::Relaxed);
        }
    }
}

async fn run_state_mirror(
    cfg: UacControlConfig,
    mut state_rx: watch::Receiver<AudioState>,
    stats: Arc<UacStats>,
    last_write: Arc<LastWrite>,
) -> Result<(), alsa::Error> {
    let mut prev_speaker_vol = state_rx.borrow().speaker_volume;
    let mut prev_mic_vol = state_rx.borrow().mic_volume;
    let mut prev_speaker_muted = state_rx.borrow().speaker_muted;
    let mut prev_mic_muted = state_rx.borrow().mic_muted;

    // Push the seeded boot value into the UAC2 mixer so the host's view
    // starts in sync with AudioState.mic_volume (which is itself seeded
    // from TOML). Without this, the mixer keeps whatever the kernel
    // initialised it to and the host's first read will not match.
    //
    // Concurrency note: this opens a separate Mixer handle while
    // run_mixer_poll holds one for the same card. ALSA's snd_mixer is
    // not thread-safe across handles. This is safe here only because
    // both tasks are cooperative and run on the same tokio runtime —
    // this block completes synchronously before yielding, and
    // run_mixer_poll won't have ticked yet on first spawn. Do not
    // restructure either task into a multi-threaded executor without
    // adding a Mutex around the mixer.
    // Seed: speaker → capture side (host writes here, so the host's first
    // GET_CUR returns the boot value); mic → playback side. See the
    // gadget-vs-host direction table in the module docstring.
    if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
        let _ = write_volume(&mixer, "PCM", true,  prev_speaker_vol, SPEAKER_MAX_PCT);
        let _ = write_volume(&mixer, "PCM", false, prev_mic_vol,     MIC_MAX_PCT);
        last_write.speaker_volume.store(prev_speaker_vol, Ordering::Relaxed);
        last_write.mic_volume.store(prev_mic_vol, Ordering::Relaxed);
    }

    while state_rx.changed().await.is_ok() {
        let s = state_rx.borrow().clone();
        if s.speaker_volume != prev_speaker_vol {
            prev_speaker_vol = s.speaker_volume;
            last_write.speaker_volume.store(s.speaker_volume, Ordering::Relaxed);
            // Open the mixer fresh on each write — short-lived, avoids
            // holding state across await points.
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", true, s.speaker_volume, SPEAKER_MAX_PCT);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.mic_volume != prev_mic_vol {
            prev_mic_vol = s.mic_volume;
            last_write.mic_volume.store(s.mic_volume, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", false, s.mic_volume, MIC_MAX_PCT);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.speaker_muted != prev_speaker_muted {
            prev_speaker_muted = s.speaker_muted;
            last_write.speaker_muted.store(s.speaker_muted, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_switch(&mixer, "PCM", true, s.speaker_muted);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.mic_muted != prev_mic_muted {
            prev_mic_muted = s.mic_muted;
            last_write.mic_muted.store(s.mic_muted, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_switch(&mixer, "PCM", false, s.mic_muted);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

/// Convert ALSA raw mixer value to our percent space [0, max_pct].
fn raw_to_pct(raw: i64, min: i64, max: i64, max_pct: u16) -> u16 {
    let span = (max - min).max(1) as f64;
    let pct = ((raw - min) as f64 / span * max_pct as f64).round() as i64;
    pct.clamp(0, max_pct as i64) as u16
}

/// Convert our percent value [0, max_pct] back to ALSA raw range.
fn pct_to_raw(pct: u16, min: i64, max: i64, max_pct: u16) -> i64 {
    let span = (max - min) as f64;
    let raw = min + (span * pct as f64 / max_pct as f64).round() as i64;
    raw.clamp(min, max)
}

fn read_volume(mixer: &Mixer, name: &str, capture: bool, max_pct: u16) -> Option<u16> {
    let id = SelemId::new(name, 0);
    let elem = mixer.find_selem(&id)?;
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = if capture {
        elem.get_capture_volume(SelemChannelId::mono()).ok()?
    } else {
        elem.get_playback_volume(SelemChannelId::mono()).ok()?
    };
    Some(raw_to_pct(raw, min, max, max_pct))
}

fn read_switch(mixer: &Mixer, name: &str, capture: bool) -> Option<bool> {
    let id = SelemId::new(name, 0);
    let elem = mixer.find_selem(&id)?;
    let active = if capture {
        elem.get_capture_switch(SelemChannelId::mono()).ok()?
    } else {
        elem.get_playback_switch(SelemChannelId::mono()).ok()?
    };
    // ALSA: 1 = unmuted, 0 = muted. Our `*_muted` semantics: true = muted.
    Some(active == 0)
}

/// Read the value of a card-level switch control like "PCM Playback State"
/// or "PCM Capture State". These are *not* the per-element switches that
/// `read_switch` reads — they're separate `numid` entries the kernel
/// exposes for `f_uac2` host-attachment status. Returns `Some(true)` when
/// the host has the endpoint open, `Some(false)` when closed.
fn read_state_switch(mixer: &Mixer, name: &str) -> Option<bool> {
    let id = SelemId::new(name, 0);
    let elem = mixer.find_selem(&id)?;
    // The State controls are mono playback switches.
    let active = elem.get_playback_switch(SelemChannelId::mono()).ok()?;
    Some(active != 0)
}

fn write_volume(mixer: &Mixer, name: &str, capture: bool, percent: u16, max_pct: u16) -> Result<(), alsa::Error> {
    let id = SelemId::new(name, 0);
    let elem = match mixer.find_selem(&id) {
        Some(e) => e,
        None => return Ok(()),
    };
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = pct_to_raw(percent, min, max, max_pct);
    if capture {
        elem.set_capture_volume_all(raw)?;
    } else {
        elem.set_playback_volume_all(raw)?;
    }
    Ok(())
}

fn dispatch_host_mute(
    commands: &broadcast::Sender<Command>,
    target: HostMuteTarget,
) {
    match target {
        HostMuteTarget::Speaker => {
            let _ = commands.send(Command::ToggleSpeakerMute);
        }
        HostMuteTarget::Mic => {
            let _ = commands.send(Command::ToggleMicMute);
        }
        HostMuteTarget::Both => {
            let _ = commands.send(Command::ToggleSpeakerMute);
            let _ = commands.send(Command::ToggleMicMute);
        }
    }
}

fn write_switch(mixer: &Mixer, name: &str, capture: bool, muted: bool) -> Result<(), alsa::Error> {
    let id = SelemId::new(name, 0);
    let elem = match mixer.find_selem(&id) {
        Some(e) => e,
        None => return Ok(()),
    };
    // ALSA: 1 = unmuted, 0 = muted. Our `*_muted` semantics: true = muted.
    let active: i32 = if muted { 0 } else { 1 };
    if capture {
        elem.set_capture_switch_all(active)?;
    } else {
        elem.set_playback_switch_all(active)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_to_raw_unity_at_max_pct_is_max_raw() {
        // The conversion logic embedded in write_volume / read_volume:
        // pct ∈ [0, max_pct], raw ∈ [min_raw, max_raw].
        assert_eq!(pct_to_raw(0, 0, 64, 100), 0);
        assert_eq!(pct_to_raw(100, 0, 64, 100), 64);
        assert_eq!(pct_to_raw(50, 0, 64, 100), 32);

        assert_eq!(pct_to_raw(0, 0, 64, 800), 0);
        assert_eq!(pct_to_raw(800, 0, 64, 800), 64);
        assert_eq!(pct_to_raw(400, 0, 64, 800), 32);
        assert_eq!(pct_to_raw(200, 0, 64, 800), 16);
    }

    #[test]
    fn raw_to_pct_inverse() {
        assert_eq!(raw_to_pct(0, 0, 64, 100), 0);
        assert_eq!(raw_to_pct(64, 0, 64, 100), 100);
        assert_eq!(raw_to_pct(64, 0, 64, 800), 800);
        assert_eq!(raw_to_pct(32, 0, 64, 800), 400);
    }

    #[test]
    fn pct_to_raw_clamps_above_max_pct() {
        assert_eq!(pct_to_raw(900, 0, 64, 800), 64);
        assert_eq!(pct_to_raw(150, 0, 64, 100), 64);
    }
}
