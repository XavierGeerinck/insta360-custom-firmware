//! ALSA side of the Bluetooth paths: the `hw:0,0` streams and the two
//! threads (`bt-downstream`, `bt-uplink`), SCHED_FIFO 18 like the USB
//! bridges (one under the speaker's 20).

use std::sync::Arc;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};
use waved_state::BluetoothState;

use super::downstream::{self, DownstreamParams, PhoneDownstream, Timing};
use super::uplink::{self, PhoneUplink, UplinkParams, RETRY_FRAMES};
use super::{BtAudioStats, CaptureStream, PlaybackStream, StreamError};
use crate::audio_config::BtAudioConfig;
use crate::playback::PlaybackService;
use crate::processed_frame::ProcessedAudioFrame;

/// TDM-A carries 48 kHz stereo S16 both ways (`docs/BLUETOOTH_PROTOCOL.md`).
const CHANNELS: u32 = 2;
const RATE: u32 = 48_000;
const PRIORITY: i32 = 18;

fn classify(e: alsa::Error) -> StreamError {
    match e.errno() {
        libc::EAGAIN => StreamError::Again,
        libc::EPIPE | libc::ESTRPIPE => StreamError::Xrun,
        _ => StreamError::Fatal(e.to_string()),
    }
}

fn open_pcm(cfg: &BtAudioConfig, dir: Direction) -> Result<PCM, String> {
    let setup = || -> Result<PCM, alsa::Error> {
        let pcm = PCM::new(&cfg.device, dir, false)?;
        {
            let hwp = HwParams::any(&pcm)?;
            hwp.set_access(Access::RWInterleaved)?;
            hwp.set_format(Format::s16())?;
            hwp.set_channels(CHANNELS)?;
            hwp.set_rate(RATE, ValueOr::Nearest)?;
            hwp.set_period_size(cfg.period_frames as i64, ValueOr::Nearest)?;
            hwp.set_buffer_size_near((cfg.period_frames * cfg.buffer_periods as usize) as i64)?;
            pcm.hw_params(&hwp)?;
        }
        Ok(pcm)
    };
    let pcm = setup().map_err(|e| e.to_string())?;
    let hw = pcm.hw_params_current().map_err(|e| e.to_string())?;
    let rate = hw.get_rate().map_err(|e| e.to_string())?;
    if rate != RATE {
        return Err(format!("{} runs at {rate} Hz, not {RATE}", cfg.device));
    }
    debug!(
        "bt: {} {:?} open: {rate} Hz, period {} frames, buffer {} frames",
        cfg.device,
        dir,
        hw.get_period_size().unwrap_or(0),
        hw.get_buffer_size().unwrap_or(0)
    );
    drop(hw);
    pcm.prepare().map_err(|e| e.to_string())?;
    Ok(pcm)
}

/// `hw:0,0` capture: the phone's audio.
pub struct AlsaCapture {
    pcm: PCM,
}

impl AlsaCapture {
    pub fn open(cfg: &BtAudioConfig) -> Result<Self, String> {
        let pcm = open_pcm(cfg, Direction::Capture)?;
        pcm.start().map_err(|e| e.to_string())?;
        Ok(Self { pcm })
    }
}

impl CaptureStream for AlsaCapture {
    fn read(&mut self, buf: &mut [i16]) -> Result<usize, StreamError> {
        let io = self.pcm.io_i16().map_err(|e| StreamError::Fatal(e.to_string()))?;
        io.readi(buf).map_err(classify)
    }

    fn recover(&mut self) -> Result<(), String> {
        self.pcm.prepare().map_err(|e| e.to_string())?;
        self.pcm.start().map_err(|e| e.to_string())
    }
}

/// `hw:0,0` playback: the microphone to the phone.
pub struct AlsaPlayback {
    pcm: PCM,
}

impl AlsaPlayback {
    pub fn open(cfg: &BtAudioConfig) -> Result<Self, String> {
        Ok(Self { pcm: open_pcm(cfg, Direction::Playback)? })
    }
}

impl PlaybackStream for AlsaPlayback {
    fn write(&mut self, buf: &[i16]) -> Result<usize, StreamError> {
        let io = self.pcm.io_i16().map_err(|e| StreamError::Fatal(e.to_string()))?;
        let channels = CHANNELS as usize;
        // writei may be partial; loop until the whole buffer is out.
        let mut written = 0;
        while written < buf.len() {
            match io.writei(&buf[written..]) {
                Ok(0) => break,
                Ok(frames) => written += frames * channels,
                Err(e) => return Err(classify(e)),
            }
        }
        Ok(written / channels)
    }

    fn recover(&mut self) -> Result<(), String> {
        self.pcm.prepare().map_err(|e| e.to_string())
    }
}

/// Start both Bluetooth paths for the life of the process: phone audio to
/// `playback` while `phone_audio`, the processed uplink to the phone while
/// `call_audio`.
pub fn spawn(
    cfg: BtAudioConfig,
    playback: Arc<PlaybackService>,
    processed: broadcast::Receiver<ProcessedAudioFrame>,
    bt: watch::Receiver<BluetoothState>,
    stats: Arc<BtAudioStats>,
) -> std::io::Result<()> {
    let down_cfg = cfg.clone();
    let down_stats = Arc::clone(&stats);
    let down_state = bt.clone();
    std::thread::Builder::new()
        .name("bt-downstream".into())
        .spawn(move || {
            set_realtime_priority("bt-downstream");
            let params = DownstreamParams {
                channels: CHANNELS as usize,
                read_frames: down_cfg.period_frames,
                drop_at: down_cfg.buffer_periods as usize,
                prefill_periods: 1,
            };
            let down = PhoneDownstream::new(
                move || AlsaCapture::open(&down_cfg),
                playback,
                down_stats,
                params,
            );
            downstream::run(down, down_state, Timing::default());
        })?;

    let device = cfg.device.clone();
    let playback_clock = cfg.playback_clock;
    std::thread::Builder::new()
        .name("bt-uplink".into())
        .spawn(move || {
            set_realtime_priority("bt-uplink");
            let params = UplinkParams {
                channels: CHANNELS as usize,
                buffer_periods: cfg.buffer_periods,
                retry_frames: RETRY_FRAMES,
                playback_clock: cfg.playback_clock,
            };
            let up = PhoneUplink::new(move || AlsaPlayback::open(&cfg), stats, params);
            uplink::run(up, processed, bt);
        })?;
    info!("bt: phone audio paths ready on {device} (playback_clock={playback_clock})");
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_realtime_priority(name: &str) {
    let param = libc::sched_param { sched_priority: PRIORITY };
    // SAFETY: param lives for the duration of this call.
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        warn!("{name} SCHED_FIFO failed: {errno}");
    }
}

#[cfg(not(target_os = "linux"))]
fn set_realtime_priority(_name: &str) {}
