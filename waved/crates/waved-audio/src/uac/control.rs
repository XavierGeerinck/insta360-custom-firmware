//! UAC2 control — bridge `hw:1` ALSA mixer events to `Command` and back.
//!
//! Direction 1 (host → device): the kernel's `f_uac2` driver translates UAC2
//! SET_CUR class requests into ALSA mixer value events on card 1
//! (`PCM Playback Volume`, `PCM Capture Volume`, the matching switches, and
//! the read-only `*State` indicators). We poll for events at 50 ms cadence
//! and dispatch `Command::Set*Volume` / `Command::Toggle*Mute` onto the bus.
//!
//! Direction 2 (device → host): when `AudioState.speaker_volume` changes via
//! the touch UI or `/tmp/waved-volume`, we write the new value into the same
//! mixer control. The kernel reports it back to the host on the next GET_CUR.
//!
//! Both directions go through a shared `LastWrite` cell so a self-write
//! event isn't re-dispatched as a host event.

use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alsa::mixer::{Mixer, SelemId, SelemChannelId};
use tokio::sync::{broadcast, watch};
use tokio::time::interval;
use tracing::{info, warn};

use waved_state::{AudioState, Command};

use super::UacStats;
use crate::audio_config::HostMuteTarget;

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

/// Cell tracking what we last wrote to the mixer ourselves (to suppress the
/// resulting self-event from being re-dispatched as a host change).
#[derive(Default)]
struct LastWrite {
    speaker_volume: AtomicU16,
    mic_volume: AtomicU16,
    speaker_muted: AtomicBool,
    mic_muted: AtomicBool,
}

pub struct UacControl {
    _phantom: (),
}

impl UacControl {
    /// Spawn the mixer event poll task plus the state mirror task.
    /// Both live in the existing tokio runtime; the alsa::mixer calls are
    /// short-lived and do not block long enough to need a dedicated thread.
    pub fn spawn(
        cfg: UacControlConfig,
        commands: broadcast::Sender<Command>,
        state_rx: watch::Receiver<AudioState>,
        stats: Arc<UacStats>,
    ) -> Self {
        let last_write = Arc::new(LastWrite::default());

        // Direction 1: poll mixer → emit Command.
        tokio::spawn({
            let cfg = cfg.clone();
            let stats = Arc::clone(&stats);
            let last_write = Arc::clone(&last_write);
            async move {
                if let Err(e) = run_mixer_poll(cfg, commands, stats, last_write).await {
                    warn!("uac-control mixer poll exited: {e}");
                }
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

async fn run_mixer_poll(
    cfg: UacControlConfig,
    commands: broadcast::Sender<Command>,
    stats: Arc<UacStats>,
    last_write: Arc<LastWrite>,
) -> Result<(), alsa::Error> {
    info!("uac-control: opening mixer {}", cfg.mixer_card);
    let mixer = Mixer::new(&cfg.mixer_card, false)?;

    // Cache the current values so we only emit on change.
    let mut prev_speaker_vol = read_volume(&mixer, "PCM", false, SPEAKER_MAX_PCT).unwrap_or(0);
    let mut prev_mic_vol = read_volume(&mixer, "PCM", true, MIC_MAX_PCT).unwrap_or(0);
    let mut prev_speaker_muted = read_switch(&mixer, "PCM", false).unwrap_or(false);
    let mut prev_mic_muted = read_switch(&mixer, "PCM", true).unwrap_or(false);

    let mut tick = interval(Duration::from_millis(50));
    loop {
        tick.tick().await;
        // Drain pending mixer events. `handle_events` is non-blocking.
        let _ = mixer.handle_events();

        if let Some(v) = read_volume(&mixer, "PCM", false, SPEAKER_MAX_PCT) {
            if v != prev_speaker_vol {
                prev_speaker_vol = v;
                if last_write.speaker_volume.load(Ordering::Relaxed) != v {
                    info!("uac-control: host set speaker volume → {v}");
                    let _ = commands.send(Command::SetSpeakerVolume(v));
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if let Some(v) = read_volume(&mixer, "PCM", true, MIC_MAX_PCT) {
            if v != prev_mic_vol {
                prev_mic_vol = v;
                if last_write.mic_volume.load(Ordering::Relaxed) != v {
                    info!("uac-control: host set mic volume → {v}");
                    let _ = commands.send(Command::SetMicVolume(v));
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        // Host mute on the playback FU. macOS rarely dispatches here (it
        // prefers the capture FU for system mute on bidirectional devices),
        // but stock UAC2 hosts may, so mirror it onto whatever target the
        // config selects. By default speaker → speaker (one-to-one), which
        // is the natural meaning.
        if let Some(muted) = read_switch(&mixer, "PCM", false) {
            if muted != prev_speaker_muted {
                prev_speaker_muted = muted;
                if last_write.speaker_muted.load(Ordering::Relaxed) != muted {
                    info!("uac-control: host set speaker mute → {muted}");
                    dispatch_host_mute(&commands, cfg.host_mute_target);
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                    // Track host's value so the next toggle is detected as
                    // a change vs. last_write (state-mirror won't write back
                    // when redirect target ≠ speaker).
                    last_write.speaker_muted.store(muted, Ordering::Relaxed);
                }
            }
        }
        // Host mute on the capture FU. This is what macOS F10 lands on for
        // a UAC2 speakerphone. Redirect per `host_mute_target`: default
        // "speaker" silences playback (music-listener UX), "mic" preserves
        // the headset semantic, "both" toggles both at once.
        if let Some(muted) = read_switch(&mixer, "PCM", true) {
            if muted != prev_mic_muted {
                prev_mic_muted = muted;
                if last_write.mic_muted.load(Ordering::Relaxed) != muted {
                    info!(
                        "uac-control: host set mic mute → {muted} (target={:?})",
                        cfg.host_mute_target
                    );
                    dispatch_host_mute(&commands, cfg.host_mute_target);
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                    // See speaker-mute branch above — sync last_write so a
                    // subsequent host toggle isn't echo-suppressed.
                    last_write.mic_muted.store(muted, Ordering::Relaxed);
                }
            }
        }

        // SP#6: source-of-truth for host attachment is the kernel's
        // PCM Playback State / PCM Capture State switches. Update the
        // *_active stats flags here so the bridges don't have to.
        if let Some(active) = read_state_switch(&mixer, "PCM Playback State") {
            stats.upstream_active.store(active, Ordering::Relaxed);
        }
        if let Some(active) = read_state_switch(&mixer, "PCM Capture State") {
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
    if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
        let _ = write_volume(&mixer, "PCM", true, prev_mic_vol, MIC_MAX_PCT);
        let _ = write_volume(&mixer, "PCM", false, prev_speaker_vol, SPEAKER_MAX_PCT);
        last_write.mic_volume.store(prev_mic_vol, Ordering::Relaxed);
        last_write.speaker_volume.store(prev_speaker_vol, Ordering::Relaxed);
    }

    while state_rx.changed().await.is_ok() {
        let s = state_rx.borrow().clone();
        if s.speaker_volume != prev_speaker_vol {
            prev_speaker_vol = s.speaker_volume;
            last_write.speaker_volume.store(s.speaker_volume, Ordering::Relaxed);
            // Open the mixer fresh on each write — short-lived, avoids
            // holding state across await points.
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", false, s.speaker_volume, SPEAKER_MAX_PCT);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.mic_volume != prev_mic_vol {
            prev_mic_vol = s.mic_volume;
            last_write.mic_volume.store(s.mic_volume, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", true, s.mic_volume, MIC_MAX_PCT);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.speaker_muted != prev_speaker_muted {
            prev_speaker_muted = s.speaker_muted;
            last_write.speaker_muted.store(s.speaker_muted, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_switch(&mixer, "PCM", false, s.speaker_muted);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.mic_muted != prev_mic_muted {
            prev_mic_muted = s.mic_muted;
            last_write.mic_muted.store(s.mic_muted, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_switch(&mixer, "PCM", true, s.mic_muted);
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
