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

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alsa::mixer::{Mixer, SelemId, SelemChannelId};
use tokio::sync::{broadcast, watch};
use tokio::time::interval;
use tracing::{info, warn};

use waved_state::{AudioState, Command};

use super::UacStats;

#[derive(Debug, Clone)]
pub struct UacControlConfig {
    pub mixer_card: String,    // "hw:1"
}

/// Cell tracking what we last wrote to the mixer ourselves (to suppress the
/// resulting self-event from being re-dispatched as a host change).
#[derive(Default)]
struct LastWrite {
    speaker_volume: AtomicU16,
    mic_volume: AtomicU16,
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
    let mut prev_speaker_vol = read_volume(&mixer, "PCM", false).unwrap_or(0);
    let mut prev_mic_vol = read_volume(&mixer, "PCM", true).unwrap_or(0);
    let mut prev_speaker_muted = read_switch(&mixer, "PCM", false).unwrap_or(false);
    let mut prev_mic_muted = read_switch(&mixer, "PCM", true).unwrap_or(false);

    let mut tick = interval(Duration::from_millis(50));
    loop {
        tick.tick().await;
        // Drain pending mixer events. `handle_events` is non-blocking.
        let _ = mixer.handle_events();

        if let Some(v) = read_volume(&mixer, "PCM", false) {
            if v != prev_speaker_vol {
                prev_speaker_vol = v;
                if last_write.speaker_volume.load(Ordering::Relaxed) != v {
                    info!("uac-control: host set speaker volume → {v}");
                    let _ = commands.send(Command::SetSpeakerVolume(v));
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if let Some(v) = read_volume(&mixer, "PCM", true) {
            if v != prev_mic_vol {
                prev_mic_vol = v;
                if last_write.mic_volume.load(Ordering::Relaxed) != v {
                    info!("uac-control: host set mic volume → {v}");
                    let _ = commands.send(Command::SetMicVolume(v));
                    stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if let Some(muted) = read_switch(&mixer, "PCM", false) {
            if muted != prev_speaker_muted {
                prev_speaker_muted = muted;
                let _ = commands.send(Command::ToggleSpeakerMute);
                stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(muted) = read_switch(&mixer, "PCM", true) {
            if muted != prev_mic_muted {
                prev_mic_muted = muted;
                let _ = commands.send(Command::ToggleMicMute);
                stats.host_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
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

    while state_rx.changed().await.is_ok() {
        let s = state_rx.borrow().clone();
        if s.speaker_volume != prev_speaker_vol {
            prev_speaker_vol = s.speaker_volume;
            last_write.speaker_volume.store(s.speaker_volume, Ordering::Relaxed);
            // Open the mixer fresh on each write — short-lived, avoids
            // holding state across await points.
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", false, s.speaker_volume);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
        if s.mic_volume != prev_mic_vol {
            prev_mic_vol = s.mic_volume;
            last_write.mic_volume.store(s.mic_volume, Ordering::Relaxed);
            if let Ok(mixer) = Mixer::new(&cfg.mixer_card, false) {
                let _ = write_volume(&mixer, "PCM", true, s.mic_volume);
                stats.device_set_volume_events.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

fn read_volume(mixer: &Mixer, name: &str, capture: bool) -> Option<u16> {
    let id = SelemId::new(name, 0);
    let elem = mixer.find_selem(&id)?;
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = if capture {
        elem.get_capture_volume(SelemChannelId::mono()).ok()?
    } else {
        elem.get_playback_volume(SelemChannelId::mono()).ok()?
    };
    let pct = ((raw - min) as f64 / (max - min).max(1) as f64 * 100.0).round() as u16;
    Some(pct.min(100))
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

fn write_volume(mixer: &Mixer, name: &str, capture: bool, percent: u16) -> Result<(), alsa::Error> {
    let id = SelemId::new(name, 0);
    let elem = match mixer.find_selem(&id) {
        Some(e) => e,
        None => return Ok(()),
    };
    let (min, max) = if capture { elem.get_capture_volume_range() }
                     else       { elem.get_playback_volume_range() };
    let raw = (min + ((max - min) as f64 * percent as f64 / 100.0).round() as i64).clamp(min, max);
    if capture {
        elem.set_capture_volume_all(raw)?;
    } else {
        elem.set_playback_volume_all(raw)?;
    }
    Ok(())
}
