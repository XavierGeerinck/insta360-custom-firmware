#[cfg(all(feature = "simulate", feature = "capture-alsa"))]
compile_error!("features `simulate` and `capture-alsa` are mutually exclusive");

use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use waved_state::{Command, DeviceEvent, StateBus};

#[cfg(any(test, not(feature = "simulate")))]
mod mic_capture;

#[cfg(not(feature = "simulate"))]
const CONFIG_PATH: &str = "/data/waved/config.toml";
#[cfg(feature = "simulate")]
const CONFIG_PATH: &str = "./config.toml";

#[cfg(not(feature = "simulate"))]
const DB_PATH: &str = "/data/waved/meetings.db";
#[cfg(feature = "simulate")]
const DB_PATH: &str = "./meetings.db";

#[cfg(not(feature = "simulate"))]
const RECORDINGS_DIR: &str = "/mnt/udisk/recordings";
#[cfg(feature = "simulate")]
const RECORDINGS_DIR: &str = "./recordings";

const UART_PORT: &str = "/dev/ttyS1";
/// The Telink module's speed (docs/BLUETOOTH_PROTOCOL.md); frames verified on
/// the device 2026-10-13.
const UART_BAUD: u32 = waved_bt::BT_BAUD;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("waved=info".parse().unwrap())
                .add_directive("waved_ui=info".parse().unwrap())
                .add_directive("waved_audio=info".parse().unwrap()),
        )
        .init();

    // A factory reset requested before the last restart erases before
    // anything opens config.toml, the database or the recordings.
    let reset_paths = waved_system::reset::ResetPaths::new(CONFIG_PATH, DB_PATH, RECORDINGS_DIR);
    #[cfg(not(feature = "simulate"))]
    let reset_paths = reset_paths.on_device();
    match waved_system::reset::complete_pending(&reset_paths) {
        waved_system::reset::ResetOutcome::NothingPending => {}
        outcome => info!("factory reset at start-up: {outcome:?}"),
    }

    #[cfg(feature = "simulate")]
    info!("waved v{} starting [SIMULATION MODE]", env!("CARGO_PKG_VERSION"));
    #[cfg(not(feature = "simulate"))]
    info!("waved v{} starting", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = waved_config::WavedConfig::load_or_default(CONFIG_PATH.as_ref());
    info!("config loaded: device={} audio_mode={:?}", config.device.name, config.audio.mode);

    // Create state bus
    let bus = Arc::new(StateBus::new());

    // Seed live state from TOML: audio mode, noise reduction, volumes, aim and
    // the display/time/recording settings. Must run before any service is
    // spawned, because AudioService reads mode and NR from the watch when built.
    waved_settings::seed_state(&bus, &config);

    // --- Hardware services: real or simulated ---
    #[cfg(not(feature = "simulate"))]
    {
        let uart_result = waved_uart::UartMux::open(UART_PORT, UART_BAUD);

        match uart_result {
            Ok(mut uart) => {
                info!("UART opened: {UART_PORT} @ {UART_BAUD}");
                // ttyS1 is the Bluetooth module; its "MCU" route carries no
                // MCU traffic (docs/MCU_PROTOCOL.md). Keep draining it: a
                // dropped receiver stops the mux's reader, Bluetooth included.
                let mcu_rx = uart.take_mcu_receiver();
                std::thread::Builder::new()
                    .name("ttys1-drain".into())
                    .spawn(move || for _ in mcu_rx.iter() {})
                    .expect("spawn ttys1-drain");
                let bt_rx = uart.take_bt_receiver();
                let uart = Arc::new(uart);

                let bt_uart = Arc::clone(&uart);
                let bt_state_tx = Arc::clone(&bus.bt);
                let bt_events_tx = bus.events.clone();
                // Pair, disconnect, answer and decline (I1, I3).
                let bt_cmd_rx = bus.commands.subscribe();
                // The Bluetooth switch and the module's name from config;
                // the phone's volume on the speaker (docs/BLUETOOTH_PROTOCOL.md).
                let bt_settings =
                    waved_bt::Settings::new(config.bluetooth.enabled, &config.device.name);
                let bt_speaker = waved_bt::Speaker::from_bus(&bus);
                std::thread::Builder::new()
                    .name("bt-srv".into())
                    .spawn(move || {
                        let svc = waved_bt::BtService::new(bt_rx, bt_uart, bt_state_tx, bt_events_tx)
                            .with_commands(bt_cmd_rx)
                            .with_settings(bt_settings)
                            .with_speaker(bt_speaker);
                        svc.run();
                    })
                    .expect("failed to spawn bt thread");
            }
            Err(e) => {
                error!("failed to open UART {UART_PORT}: {e}");
                error!("running without Bluetooth (host development mode)");
            }
        }

        // The MCU: battery, the power key, the lift and power off.
        match waved_mcu::link::Link::open(waved_mcu::MCU_PORT, waved_mcu::MCU_BAUD) {
            Ok(link) => {
                info!("MCU link opened: {} @ {}", waved_mcu::MCU_PORT, waved_mcu::MCU_BAUD);
                let svc = waved_mcu::McuService::new(
                    link,
                    Arc::clone(&bus.battery),
                    Arc::clone(&bus.mcu),
                    bus.events.clone(),
                );
                // Status LEDs as stock drives them (F4); WAVED_LEDS=0 leaves them alone.
                let svc = if waved_mcu::leds_enabled() {
                    svc.with_leds(waved_mcu::LedSources::from_bus(&bus))
                } else {
                    info!("WAVED_LEDS=0: leaving the status LEDs alone");
                    svc
                };
                let cmd_rx = bus.commands.subscribe();
                std::thread::Builder::new()
                    .name("mcu-srv".into())
                    .spawn(move || svc.run(cmd_rx))
                    .expect("spawn mcu-srv");
            }
            Err(e) => error!("failed to open the MCU link {}: {e}", waved_mcu::MCU_PORT),
        }
    }

    #[cfg(feature = "simulate")]
    {
        // Simulated MCU
        let (sim_mcu, _mcu_handle) = waved_mcu::simulate::SimMcuService::new(
            Arc::clone(&bus.battery),
            Arc::clone(&bus.mcu),
            bus.events.clone(),
        );
        let sim_mcu = sim_mcu.with_leds(waved_mcu::LedSources::from_bus(&bus));
        let mcu_cmd_rx = bus.commands.subscribe();
        std::thread::Builder::new()
            .name("sim-mcu".into())
            .spawn(move || sim_mcu.run(mcu_cmd_rx))
            .expect("failed to spawn sim-mcu thread");

        // Simulated BT
        let sim_bt = waved_bt::simulate::SimBtService::new(
            Arc::clone(&bus.bt),
            bus.events.clone(),
        );
        std::thread::Builder::new()
            .name("sim-bt".into())
            .spawn(move || sim_bt.run())
            .expect("failed to spawn sim-bt thread");
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(run_async(config, bus, reset_paths));

    info!("waved shutdown complete");
}

async fn run_async(
    config: waved_config::WavedConfig,
    bus: Arc<StateBus>,
    reset_paths: waved_system::reset::ResetPaths,
) {
    // Persist device-side setting changes (volume, mode, display, time,
    // recording policy) back to config.toml. Subscribe before anything else
    // awaits: the host sends SET_CUR volume at USB enumeration, while audio
    // and meeting init are still running.
    tokio::spawn(
        waved_settings::SettingsService::new(CONFIG_PATH, Arc::clone(&bus.settings))
            .run(bus.commands.subscribe()),
    );

    // Factory reset (E6): on Command::FactoryReset, leave the marker and
    // restart; `complete_pending` erases at the next start-up.
    tokio::spawn(waved_system::reset::run(reset_paths, bus.commands.subscribe(), bus.commands.clone()));

    // Time zone, RTC and About (E3, E4, E7) on `bus.system`.
    #[cfg(not(feature = "simulate"))]
    let system = waved_system::SystemService::device(Arc::clone(&bus.system), RECORDINGS_DIR);
    #[cfg(feature = "simulate")]
    let system = waved_system::SystemService::new(Arc::clone(&bus.system), RECORDINGS_DIR);
    tokio::spawn(system.run(bus.settings.subscribe(), bus.wifi.subscribe()));
    // USB host connection from the device controller; the MCU never reports it.
    #[cfg(not(feature = "simulate"))]
    tokio::spawn(waved_system::usb::run(
        std::path::PathBuf::from(waved_system::usb::UDC_DIR),
        Arc::clone(&bus.mcu),
    ));

    // Self-hosted, signed updates (H2, docs/UPDATES.md). Not in simulate
    // builds: the installer writes /data and /etc and reboots.
    #[cfg(not(feature = "simulate"))]
    waved_update::device::spawn_device(
        &bus,
        &config.security.ota_server,
        config.security.ota_allow_downgrade,
        &waved_system::about::version(),
    );

    // Power key (E2, RS7): opt-in until the device pass confirms that the
    // `vad_keypad` node is the physical key and not voice activity.
    #[cfg(not(feature = "simulate"))]
    {
        if power_key_opted_in(std::env::var(POWER_KEY_ENV).ok().as_deref()) {
            log_power_key_start(waved_system::power_key::start(bus.events.clone()));
        } else {
            info!("power key: not read; set {POWER_KEY_ENV}=1 to enable it");
        }
    }

    // Meeting frame channel: DSP-processed frames for recording.
    // Non-simulate: ALSA capture → AudioFrame → AudioService (VU + AEC + NS) →
    //               ProcessedAudioFrame → meeting channel → MeetingService.
    // Simulate: SimAudioService writes silence ProcessedAudioFrames directly.
    let (meeting_tx, meeting_rx) = tokio::sync::mpsc::channel::<waved_audio::processed_frame::ProcessedAudioFrame>(32);
    let recording_integrity =
        Arc::new(waved_audio::integrity::RecordingIntegrity::default());
    let (integrity_tx, integrity_rx) = tokio::sync::mpsc::unbounded_channel();
    let (pipeline_fatal_tx, mut pipeline_fatal_rx) =
        tokio::sync::mpsc::unbounded_channel::<waved_state::RecordingIntegrityFault>();
    let pipeline_fault_claimed =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    #[cfg(not(any(
        feature = "capture-alsa",
        feature = "recording-integrity-test-hooks"
    )))]
    let _ = (&pipeline_fatal_tx, &pipeline_fault_claimed);

    // DoA receiver for MeetingService. Initialised to a closed channel so
    // the non-capture-alsa paths (simulate / bare) compile without a real
    // AudioService. The capture-alsa block below overwrites it with the live
    // receiver from AudioService.
    let mut meeting_doa_rx = {
        let (_, rx) = tokio::sync::broadcast::channel::<waved_audio::doa::DoaFrame>(1);
        rx
    };

    // Storage for the capture stop flag so the shutdown path can signal the ALSA thread.
    // The same cfg condition ensures this compiles away when capture-alsa is disabled.
    #[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
    let capture_stop_flag: Arc<std::sync::atomic::AtomicBool>;

    #[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
    {
        use std::path::Path;
        use std::sync::atomic::AtomicBool;
        use waved_audio::audio_config::{AudioConfig, ConfigSource};
        use waved_audio::capture::{
            alsa::{AlsaPcmSource, AlsaStreamConfig},
            alsa_dual_stream::AlsaDualStreamSource,
            mic_software_ref::MicWithSoftwareReferenceSource,
            single_stream::SingleStreamFrameSource,
            run_capture_loop_with_integrity, CaptureStats, ChannelMapSource,
        };
        use waved_audio::playback::{spawn_playback, PlaybackConfig};
        use waved_audio::stats_collector;

        let (cfg, cfg_src) = AudioConfig::load_or_default(Path::new("/etc/waved-audio-config.json"));
        let map_source = match cfg_src {
            ConfigSource::File => ChannelMapSource::File,
            ConfigSource::Fallback => ChannelMapSource::Fallback,
        };
        let cfg_src_str = match cfg_src {
            ConfigSource::File => "file",
            ConfigSource::Fallback => "fallback",
        };

        // Set the hardware preamp once. Static across the device's run
        // so DSP (AEC, DFN3) sees a stable input level. See
        // docs/superpowers/specs/2026-05-01-mic-volume-pipeline-design.md.
        waved_audio::capture::pdm_gain::init_or_warn("hw:0", cfg.capture.pdm_gain);

        let capture_stats = CaptureStats::new();
        let capture_stop = Arc::new(AtomicBool::new(false));
        capture_stop_flag = Arc::clone(&capture_stop);
        let (frame_tx, frame_rx) = waved_audio::callbacks::create_audio_channel();
        let mut audio_svc = waved_audio::AudioService::new_with_backend(
            Arc::clone(&bus.audio),
            frame_tx.clone(),
            frame_rx,
            meeting_tx,
            cfg.period_frames,
            cfg.capture.mic_sample_rate,
            cfg.mic_geometry.clone().unwrap_or_default(),
            cfg.doa_backend,
        );
        audio_svc.set_aec_config(cfg.aec.clone());
        audio_svc.set_recording_integrity(
            Arc::clone(&recording_integrity),
            integrity_tx.clone(),
        );
        let stats_rx = audio_svc.subscribe_frames();
        let stats_processed_rx = audio_svc.subscribe_processed();
        let mic_recorder = mic_capture::path_from_env()
            .map(|path| (path, audio_svc.subscribe_processed()));
        meeting_doa_rx = audio_svc.subscribe_doa();

        // Build the capture source. Three modes:
        //   1. Hardware loopback (cfg.reference_device): AlsaDualStreamSource.
        //   2. Software reference (cfg.playback):        PlaybackService + MicWithSoftwareReferenceSource.
        //   3. Mic-only:                                 SingleStreamFrameSource.
        enum SourceDyn {
            Dual(AlsaDualStreamSource),
            Single(SingleStreamFrameSource<AlsaPcmSource>),
            SoftwareRef(MicWithSoftwareReferenceSource<AlsaPcmSource>),
        }

        let open_mic = |cfg: &AudioConfig| {
            AlsaPcmSource::open(&AlsaStreamConfig {
                device: &cfg.capture.mic_device,
                channels: cfg.capture.mic_channels,
                sample_rate: cfg.capture.mic_sample_rate,
                format: cfg.capture.mic_format,
                period_frames: cfg.period_frames,
                buffer_periods: cfg.buffer_periods,
            })
            .unwrap_or_else(|e| {
                eprintln!("FATAL: ALSA mic open failed: {e}");
                let _ = std::io::Write::flush(&mut std::io::stderr());
                std::process::exit(1);
            })
        };

        let mut dsp_metrics_handle: Option<Arc<waved_audio::playback::DspMetrics>> = None;

        #[cfg(feature = "capture-alsa")]
        let mut uac_stats_handle: Option<Arc<waved_audio::uac::UacStats>> = None;

        let mut hid_stats_handle: Option<Arc<waved_hid::stats::HidStats>> = None;

        // I2: Bluetooth phone audio counters (`bt_audio` in the stats).
        let mut bt_audio_stats_handle: Option<Arc<waved_audio::bt_audio::BtAudioStats>> = None;

        let (source, alignment) = if cfg.reference_enabled() {
            match tokio::task::block_in_place(|| {
                open_retrying("ALSA dual stream", ALSA_OPEN_TRIES, ALSA_OPEN_RETRY, || {
                    AlsaDualStreamSource::from_capture_config(&cfg.capture, cfg.period_frames, cfg.buffer_periods)
                })
            }) {
                Ok(s) => {
                    let a = s.alignment();
                    (SourceDyn::Dual(s), a.as_str().to_string())
                }
                Err(e) => {
                    eprintln!("FATAL: ALSA dual-stream open failed: {e}");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    std::process::exit(1);
                }
            }
        } else if let Some(pb) = cfg.playback.clone() {
            // Spawn playback first so the broadcast receiver can be subscribed
            // before capture starts emitting frames.
            let playback = match tokio::task::block_in_place(|| {
                open_retrying("ALSA playback", ALSA_OPEN_TRIES, ALSA_OPEN_RETRY, || {
                    spawn_playback(
                        PlaybackConfig {
                            device: pb.device.clone(),
                            channels: pb.channels,
                            sample_rate: pb.sample_rate,
                            format: pb.format,
                            period_frames: pb.period_frames,
                            buffer_periods: pb.buffer_periods,
                            initial_preset: bus.audio.borrow().playback_preset,
                        },
                        bus.audio.subscribe(),
                    )
                })
            }) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("FATAL: ALSA playback open failed: {e}");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    std::process::exit(1);
                }
            };
            let ref_rx = playback.subscribe_reference();
            let ref_period_samples = playback.period_frames() * playback.channels() as usize;
            let mic = open_mic(&cfg);
            let src = MicWithSoftwareReferenceSource::new(
                mic,
                cfg.capture.mic_channels as usize,
                cfg.period_frames,
                playback.channels() as usize,
                ref_period_samples,
                ref_rx,
            );
            // Wrap in Arc so the SIGUSR1 test-tone task can submit playback
            // batches while keeping the service alive for the rest of the
            // process lifetime.
            let playback = Arc::new(playback);
            // Plays kept recordings from Files through the speaker path.
            tokio::spawn(
                waved_meeting::player::RecordingPlayer::new(
                    PathBuf::from(RECORDINGS_DIR),
                    Arc::clone(&playback) as Arc<dyn waved_meeting::player::SpeakerOutput>,
                    Arc::clone(&bus.playback),
                )
                .run(bus.commands.subscribe(), bus.audio.subscribe(), bus.meeting.subscribe()),
            );
            // Recording start and stop tones (E5) through the playback path.
            struct PlaybackTones(Arc<waved_audio::playback::PlaybackService>);
            impl waved_system::tones::ToneSink for PlaybackTones {
                fn format(&self) -> waved_system::tones::PcmFormat {
                    waved_system::tones::PcmFormat {
                        sample_rate: self.0.sample_rate(),
                        channels: self.0.channels() as usize,
                        period_frames: self.0.period_frames(),
                    }
                }
                // Mixed over the host's audio, like Files playback. Through
                // play_batch the chime took turns with a call's periods in
                // one queue, so both crackled, and the far-end tap put the
                // chime into the recording (device pass, 2026-10-12).
                fn submit(&self, period: Vec<i16>) -> bool {
                    self.0.play_local(period).is_ok()
                }
            }
            tokio::spawn(waved_system::tones::run(
                bus.meeting.subscribe(),
                bus.settings.subscribe(),
                Arc::new(PlaybackTones(Arc::clone(&playback))),
            ));
            dsp_metrics_handle = Some(playback.dsp_metrics());
            audio_svc.set_dsp_metrics(playback.dsp_metrics());
            // Recordings carry the far end of a call too (recording_mix).
            audio_svc.set_far_end(playback.subscribe_far_end(), playback.channels() as usize);
            // SP#5: forward preset commands from the bus to the playback loop.
            tokio::spawn({
                let playback = Arc::clone(&playback);
                let mut cmd_rx = bus.commands.subscribe();
                async move {
                    while let Ok(cmd) = cmd_rx.recv().await {
                        if let Command::SetPlaybackPreset(p) = cmd {
                            playback.set_preset(p);
                        }
                    }
                }
            });
            // Speaker scenes: the preset and the amplifier's Set EQ MODE follow
            // whether something reads the mic, as stock does
            // (waved_audio::playback::scene, docs/AUDIO_HARDWARE.md "Speaker scenes").
            {
                let amp_card = waved_audio::playback::scene::ctl_card_for(&pb.device);
                #[cfg(feature = "capture-alsa")]
                let amp = waved_audio::playback::scene::AlsaAmpEqControl::new(amp_card);
                #[cfg(not(feature = "capture-alsa"))]
                let amp = {
                    let _ = amp_card;
                    waved_audio::playback::scene::NoAmpEqControl
                };
                tokio::spawn(waved_audio::playback::scene::run_scene_follower(
                    bus.audio.subscribe(),
                    bus.bt.subscribe(),
                    bus.commands.subscribe(),
                    Arc::clone(&playback),
                    amp,
                    playback.dsp_metrics(),
                ));
            }
            // SP#6: UAC2 routing — host ↔ device speakerphone bridges.
            let uac_cfg = cfg.uac.clone().unwrap_or_default();
            let uac_stats: Arc<waved_audio::uac::UacStats> = Arc::new(waved_audio::uac::UacStats::default());
            #[cfg(feature = "capture-alsa")]
            {
                uac_stats_handle = Some(Arc::clone(&uac_stats));
            }

            if uac_cfg.enabled {
                let bridge_cfg = waved_audio::uac::bridge::UacBridgeConfig {
                    device: uac_cfg.downstream_device.clone(),
                    channels: 2,
                    sample_rate: 48_000,
                    period_frames: uac_cfg.period_frames,
                    buffer_periods: uac_cfg.buffer_periods,
                };
                let bridge = waved_audio::uac::bridge::UacBridge::spawn(
                    bridge_cfg,
                    Arc::clone(&playback),
                    Arc::clone(&uac_stats),
                );
                std::mem::forget(bridge); // run for process lifetime

                let upstream_cfg = waved_audio::uac::upstream::UacUpstreamConfig {
                    device: uac_cfg.upstream_device.clone(),
                    channels: 2,
                    sample_rate: 48_000,
                    period_frames: uac_cfg.period_frames,
                    buffer_periods: uac_cfg.buffer_periods,
                };
                let upstream = waved_audio::uac::upstream::UacUpstreamSink::spawn(
                    upstream_cfg,
                    audio_svc.subscribe_processed(),
                    Arc::clone(&uac_stats),
                );
                std::mem::forget(upstream);
                // "Computer is listening": the host is reading the UAC2 uplink.
                let probe_stats = Arc::clone(&uac_stats);
                audio_svc.set_host_listening_probe(move || {
                    probe_stats
                        .upstream_active
                        .load(std::sync::atomic::Ordering::Relaxed)
                });

                let control_cfg = waved_audio::uac::control::UacControlConfig {
                    mixer_card: uac_cfg.mixer_card.clone(),
                    host_mute_target: uac_cfg.host_mute_target,
                };
                let _control = waved_audio::uac::control::UacControl::spawn(
                    control_cfg,
                    bus.commands.clone(),
                    bus.audio.subscribe(),
                    Arc::clone(&uac_stats),
                    Arc::clone(&bus.host_pending_speaker_vol),
                );
                tracing::info!(
                    "uac: routing live (downstream={} upstream={} mixer={})",
                    uac_cfg.downstream_device, uac_cfg.upstream_device, uac_cfg.mixer_card
                );
            } else {
                tracing::info!("uac: disabled by config");
            }

            // I2: Bluetooth phone audio on TDM-A (hw:0,0). Phone → speaker
            // while BluetoothState::phone_audio, mixed with USB audio; the
            // processed uplink → phone while call_audio.
            let bt_cfg = cfg.bt.clone().unwrap_or_default();
            if bt_cfg.enabled {
                let bt_stats = waved_audio::bt_audio::BtAudioStats::new();
                match waved_audio::bt_audio::alsa::spawn(
                    bt_cfg,
                    Arc::clone(&playback),
                    audio_svc.subscribe_processed(),
                    bus.bt.subscribe(),
                    Arc::clone(&bt_stats),
                ) {
                    Ok(()) => bt_audio_stats_handle = Some(bt_stats),
                    Err(e) => tracing::error!("bt: phone audio paths failed to start: {e}"),
                }
            } else {
                tracing::info!("bt: phone audio disabled by config");
            }

            tokio::spawn(test_tone_handler(Arc::clone(&playback)));
            tokio::spawn(file_play_handler(Arc::clone(&playback)));
            // Keep one Arc alive for process lifetime — the playback thread
            // stops when the last reference drops.
            std::mem::forget(Arc::clone(&playback));
            (
                SourceDyn::SoftwareRef(src),
                waved_audio::capture::alsa_dual_stream::AlignmentMethod::SoftwareReference
                    .as_str()
                    .to_string(),
            )
        } else {
            let mic = open_mic(&cfg);
            (
                SourceDyn::Single(SingleStreamFrameSource::new(
                    mic,
                    cfg.period_frames,
                    cfg.capture.mic_channels as usize,
                )),
                waved_audio::capture::alsa_dual_stream::AlignmentMethod::MicOnly
                    .as_str()
                    .to_string(),
            )
        };

        // SP#7(d'): HID Consumer/Telephony control surface.
        // Placed at top-level scope so it activates on all audio source modes
        // (Dual / SoftwareRef / Single), not only when playback is configured.
        let hid_cfg = cfg.hid.clone().unwrap_or_default();
        if hid_cfg.enabled {
            let hid_stats = waved_hid::stats::HidStats::new();
            hid_stats_handle = Some(Arc::clone(&hid_stats));

            let (ev_tx, ev_rx) = tokio::sync::mpsc::channel::<waved_hid::events::HidEvent>(64);

            let bus_rx = bus.events.subscribe();
            let svc_cfg = waved_hid::HidServiceConfig {
                device: hid_cfg.device.clone(),
                press_release_gap_ms: hid_cfg.press_release_gap_ms,
            };
            tokio::spawn({
                let stats = Arc::clone(&hid_stats);
                async move { waved_hid::HidService::run(svc_cfg, bus_rx, ev_rx, stats).await }
            });

            // Reader for host LED writes — blocking std::thread. Vendor
            // report 0x03 (Wave Controller, TODO J) goes to the PC link.
            let (pclink_tx, pclink_rx) = std::sync::mpsc::channel();
            waved_hid::reader::HidReader::spawn_with_vendor(
                waved_hid::reader::HidReaderConfig { device: hid_cfg.device.clone() },
                Arc::clone(&bus.audio),
                Arc::clone(&hid_stats),
                Some(pclink_tx),
            );
            waved_hid::pclink::service::spawn(
                waved_hid::pclink::service::PcLinkConfig { device: hid_cfg.device.clone() },
                pclink_rx,
                waved_hid::pclink::service::PcLinkBus::from_bus(&bus),
                // The app's PIN goes through the same check as the keypad. No
                // on-device prompt yet: an empty PIN reads "not verified".
                Box::new(waved_hid::pclink::handler::ConfigPins::new(config.security.clone())),
                Box::new(waved_hid::pclink::handler::NoClock),
                None,
            );

            // State-driven event sources.
            let _vh = waved_hid::watch::spawn_volume_watch(
                bus.audio.subscribe(),
                ev_tx.clone(),
                Arc::clone(&hid_stats),
                hid_cfg.volume_step_rate_hz,
                Arc::clone(&bus.host_pending_speaker_vol),
            );
            let _mh = waved_hid::watch::spawn_mute_watch(
                bus.audio.subscribe(),
                ev_tx.clone(),
                Arc::clone(&hid_stats),
                // The phone's volume unmuting the speaker (waved-bt) sends no key.
                Arc::clone(&bus.pending_speaker_mute),
            );
            let _mmh = waved_hid::watch::spawn_mic_mute_watch(
                bus.audio.subscribe(),
                ev_tx,
                Arc::clone(&hid_stats),
            );

            tracing::info!(
                "hid: control surface live (device={} press_release_gap_ms={} step_rate_hz={})",
                hid_cfg.device, hid_cfg.press_release_gap_ms, hid_cfg.volume_step_rate_hz
            );
        } else {
            tracing::info!("hid: disabled by config");
        }

        tokio::spawn(stats_collector::run(
            stats_rx,
            stats_processed_rx,
            bus.audio.subscribe(),
            Arc::clone(&capture_stats),
            map_source,
            std::path::PathBuf::from("/tmp/audio_stats.json"),
            std::time::Duration::from_millis(500),
            alignment.clone(),
            dsp_metrics_handle,
            #[cfg(feature = "capture-alsa")]
            uac_stats_handle,
            hid_stats_handle,
            bt_audio_stats_handle,
        ));

        let stats_c = Arc::clone(&capture_stats);
        let stop_c = Arc::clone(&capture_stop);
        let capture_integrity = Arc::clone(&recording_integrity);
        let capture_integrity_tx = integrity_tx.clone();
        let capture_fatal_tx = pipeline_fatal_tx.clone();
        let capture_fault_claimed = Arc::clone(&pipeline_fault_claimed);
        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let exit = match source {
                    SourceDyn::Dual(s) => run_capture_loop_with_integrity(
                        s,
                        frame_tx,
                        stop_c,
                        stats_c,
                        Arc::clone(&capture_integrity),
                        capture_integrity_tx,
                    ),
                    SourceDyn::Single(s) => run_capture_loop_with_integrity(
                        s,
                        frame_tx,
                        stop_c,
                        stats_c,
                        Arc::clone(&capture_integrity),
                        capture_integrity_tx,
                    ),
                    SourceDyn::SoftwareRef(s) => run_capture_loop_with_integrity(
                        s,
                        frame_tx,
                        stop_c,
                        stats_c,
                        Arc::clone(&capture_integrity),
                        capture_integrity_tx,
                    ),
                };
                if let Some(fault) = exit.fault() {
                    publish_first_pipeline_fault(
                        &capture_fault_claimed,
                        &capture_integrity,
                        &capture_fatal_tx,
                        fault,
                    );
                }
            })
            .expect("spawn capture thread");

        let audio_cmd_rx = bus.commands.subscribe();
        let audio_integrity = Arc::clone(&recording_integrity);
        let audio_fatal_tx = pipeline_fatal_tx.clone();
        let audio_fault_claimed = Arc::clone(&pipeline_fault_claimed);
        tokio::spawn(async move {
            let exit = audio_svc.run(audio_cmd_rx).await;
            if let Some(fault) = exit.fault() {
                publish_first_pipeline_fault(
                    &audio_fault_claimed,
                    &audio_integrity,
                    &audio_fatal_tx,
                    fault,
                );
            }
        });

        if let Some((path, rx)) = mic_recorder {
            mic_capture::spawn(path, rx);
        }

        tracing::info!("capture wired: config={cfg_src_str} alignment={alignment}");
    }

    #[cfg(feature = "simulate")]
    {
        let sim_audio = waved_audio::simulate::SimAudioService::new(
            Arc::clone(&bus.audio),
            meeting_tx,
        );
        let audio_cmd_rx = bus.commands.subscribe();
        tokio::spawn(async move { sim_audio.run(audio_cmd_rx).await });
    }

    // Neither capture-alsa nor simulate: suppress unused warning on meeting_tx.
    #[cfg(not(any(feature = "simulate", feature = "capture-alsa")))]
    drop(meeting_tx);

    // HID service (simulate path only; the real device-side wiring lives
    // inside the `#[cfg(feature = "capture-alsa")]` block above as part of
    // SP#7(d')).
    #[cfg(feature = "simulate")]
    {
        let hid_events_rx = bus.events.subscribe();
        tokio::spawn(async move { waved_hid::simulate::SimHidService::run(hid_events_rx).await });
    }

    // WiFi service
    #[cfg(not(feature = "simulate"))]
    {
        let wifi_cmds = bus.commands.subscribe();
        let wifi_svc = waved_wifi::WifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
            &config,
        );
        tokio::spawn(async move { wifi_svc.run(wifi_cmds).await });

        // WiFi MAC drives the provisioning AP SSID. Read it from the interface;
        // fall back to a constant if unavailable (see docs/WIFI.md).
        let mac = std::fs::read_to_string("/sys/class/net/wlan0/address")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "00:00:00:00:00:00".to_string());
        let prov_cmds = bus.commands.subscribe();
        let prov_svc =
            waved_provision::ProvisioningService::new(Arc::clone(&bus), mac, CONFIG_PATH);
        tokio::spawn(async move { prov_svc.run(prov_cmds).await });
    }
    #[cfg(feature = "simulate")]
    {
        let sim_wifi = waved_wifi::simulate::SimWifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
        );
        let sim_cmds = bus.commands.subscribe();
        tokio::spawn(async move { sim_wifi.run(sim_cmds).await });
    }

    // USB storage (G1, docs/USB_STORAGE.md). First take back a drive that a
    // waved which exited while exported left with the computer, before the
    // meeting service reconciles the recordings; then serve Start/Stop.
    #[cfg(not(feature = "simulate"))]
    {
        let mut usb_storage = waved_system::usb_storage::UsbStorage::device(
            Arc::clone(&bus.usb_storage),
            bus.events.clone(),
            bus.commands.clone(),
        );
        let reclaimed = tokio::task::block_in_place(|| usb_storage.reclaim_at_start());
        if reclaimed != waved_system::usb_storage::Reclaim::Nothing {
            warn!("usb storage at start-up: {reclaimed:?}");
        }
        let usb_cmd_rx = bus.commands.subscribe();
        let usb_meeting_rx = bus.meeting.subscribe();
        std::thread::Builder::new()
            .name("usb-storage".into())
            .spawn(move || usb_storage.run(usb_cmd_rx, usb_meeting_rx))
            .expect("spawn usb-storage");
    }

    // Meeting service. Its task ends once a Shutdown finalized the take,
    // which a power off waits for.
    let mut meeting_task: Option<tokio::task::JoinHandle<()>> = None;
    match waved_meeting::MeetingService::new_with_integrity(
        config.clone(),
        DB_PATH,
        PathBuf::from(RECORDINGS_DIR),
        Arc::clone(&bus.meeting),
        bus.events.clone(),
        waved_meeting::MeetingPipelineInputs {
            audio_rx: meeting_rx,
            doa_rx: meeting_doa_rx,
            integrity: Arc::clone(&recording_integrity),
            integrity_rx,
        },
    ).await {
        Ok(meeting_svc) => {
            let meeting_svc = meeting_svc
                .with_live_inputs(waved_meeting::MeetingLiveInputs {
                    settings: bus.settings.subscribe(),
                    audio: bus.audio.subscribe(),
                })
                .with_files_publisher(Arc::clone(&bus.files))
                // No recording or upload while the computer has the volume (G1).
                .with_usb_storage(bus.usb_storage.subscribe());
            let meeting_cmd_rx = bus.commands.subscribe();
            let wifi_rx = bus.wifi.subscribe();
            meeting_task = Some(tokio::spawn(async move { meeting_svc.run(meeting_cmd_rx, wifi_rx).await }));
        }
        Err(e) => error!("meeting service init failed: {e}"),
    }

    info!("all services started, waiting for shutdown signal");

    // Low battery handler
    let low_battery_bus = Arc::clone(&bus);
    let mut battery_events_rx = bus.events.subscribe();
    tokio::spawn(async move {
        loop {
            match battery_events_rx.recv().await {
                Ok(DeviceEvent::LowBattery(level)) => {
                    warn!("low battery ({level}%)");
                    if let Some(command) = low_battery_command(level) {
                        warn!("critical battery ({level}%), powering off");
                        let _ = low_battery_bus.commands.send(command);
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("battery monitor lagged by {n} events");
                }
            }
        }
    });

    // Sim status line
    #[cfg(feature = "simulate")]
    {
        let status_bus = Arc::clone(&bus);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let battery = status_bus.battery.subscribe();
                let wifi = status_bus.wifi.subscribe();
                let audio = status_bus.audio.subscribe();
                let mcu = status_bus.mcu.subscribe();
                let wifi_state = wifi.borrow();
                let wifi_str = if wifi_state.connected { wifi_state.ssid.as_str() } else { "Disconnected" };
                let audio_state = audio.borrow();
                let mcu_state = mcu.borrow();
                info!(
                    "[SIM] Battery: {}% | WiFi: {} | Audio: {:?} | Motor: {:?} | Muted: {}",
                    battery.borrow().level,
                    wifi_str,
                    audio_state.mode,
                    mcu_state.motor_position,
                    audio_state.mic_muted,
                );
            }
        });
    }

    #[cfg(feature = "ui")]
    {
        use waved_ui::UiService;
        info!("Starting UI service");
        let ui = UiService::new(Arc::clone(&bus), config.clone());
        // Drop ui without joining — the thread runs until process exit.
        std::mem::forget(ui);
    }

    tokio::spawn(mode_switch_handler(bus.commands.clone()));
    #[cfg(feature = "recording-integrity-test-hooks")]
    tokio::spawn(recording_integrity_test_hook(
        Arc::clone(&recording_integrity),
        integrity_tx.clone(),
        Arc::clone(&pipeline_fault_claimed),
        pipeline_fatal_tx.clone(),
        bus.commands.clone(),
    ));

    // Wait for SIGTERM, SIGINT, or a controlled configuration restart.
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).unwrap();
    let mut sigint = signal(SignalKind::interrupt()).unwrap();
    let mut lifecycle_commands = bus.commands.subscribe();
    let lifecycle = async {
        loop {
            match lifecycle_commands.recv().await {
                Ok(Command::RestartForConfig) => {
                    info!("received configuration restart request");
                    return ExitCause::Other;
                }
                Ok(Command::PowerOff) => {
                    info!("received power off command");
                    return ExitCause::PowerOff;
                }
                Ok(Command::Shutdown) => {
                    info!("received shutdown command");
                    return ExitCause::Other;
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return ExitCause::Other,
            }
        }
    };
    let cause = tokio::select! {
        _ = sigterm.recv() => {
            info!("received SIGTERM");
            ExitCause::Other
        }
        _ = sigint.recv() => {
            info!("received SIGINT");
            ExitCause::Other
        }
        cause = lifecycle => cause,
        fault = pipeline_fatal_rx.recv() => {
            if let Some(fault) = fault {
                bus.audio.send_modify(|state| {
                    let snapshot = recording_integrity.snapshot();
                    state.capture_healthy = false;
                    state.capture_xruns = snapshot.xruns;
                    state.raw_frames_dropped = snapshot.raw_frames_dropped;
                    state.processed_frames_dropped = snapshot.processed_frames_dropped;
                });
                error!("fatal audio pipeline fault: {}; restarting process", fault.diagnostic());
                if !acknowledge_pipeline_fault(
                    &integrity_tx,
                    fault,
                    std::time::Duration::from_secs(2),
                )
                .await
                {
                    warn!("meeting service did not acknowledge audio fault before restart");
                }
            }
            ExitCause::Other
        }
    };

    info!("shutdown signal received");
    if cause == ExitCause::PowerOff {
        let meeting_finished = async move {
            if let Some(task) = meeting_task {
                let _ = task.await;
            }
        };
        power_off_in_order(bus.commands.clone(), meeting_finished, bus.mcu.subscribe()).await;
    } else {
        let _ = bus.commands.send(Command::Shutdown);
    }
    #[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
    capture_stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    // After ALL_SHUTDOWN the MCU cuts power within this wait; otherwise it
    // gives the services time to stop.
    tokio::time::sleep(EXIT_WAIT).await;
}

/// Power off in stock's order: stop the services, let the meeting finalize
/// its take (at most `MEETING_STOP_WAIT`), and only then have waved-mcu run
/// the MCU sequence, which may cut power within a second.
async fn power_off_in_order(
    commands: tokio::sync::broadcast::Sender<Command>,
    meeting_finished: impl std::future::Future<Output = ()>,
    mut mcu: tokio::sync::watch::Receiver<waved_state::McuState>,
) {
    let _ = commands.send(Command::Shutdown);
    if tokio::time::timeout(MEETING_STOP_WAIT, meeting_finished).await.is_err() {
        warn!("the meeting did not stop within {} s; powering off anyway", MEETING_STOP_WAIT.as_secs());
    }
    info!("waiting for the MCU to power the device off");
    let _ = commands.send(Command::PowerOffNow);
    if tokio::time::timeout(POWER_OFF_WAIT, mcu.wait_for(|s| power_off_finished(s.power_off)))
        .await
        .is_err()
    {
        warn!("power off did not finish within {} s", POWER_OFF_WAIT.as_secs());
    }
}

/// What a low-battery report asks for: power off at the critical level
/// (TODO F2), nothing above it. waved-mcu reports a level only once it held
/// on battery power, so one reading never powers off.
/// Tries before giving up on an ALSA device at start-up, and the wait between
/// them: 20 s. On 2026-10-13 the speaker (`hw:0,1`) was busy for the first
/// ~10 s of a boot, and waved exited and restarted three times.
const ALSA_OPEN_TRIES: u32 = 40;
const ALSA_OPEN_RETRY: std::time::Duration = std::time::Duration::from_millis(500);

/// `open` until it works or `attempts` tries have failed, `delay` apart.
fn open_retrying<T, E: std::fmt::Display>(
    what: &str,
    attempts: u32,
    delay: std::time::Duration,
    mut open: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut tried = 0;
    loop {
        tried += 1;
        match open() {
            Ok(opened) => {
                if tried > 1 {
                    info!("{what} opened after {tried} tries");
                }
                return Ok(opened);
            }
            Err(e) if tried >= attempts => return Err(e),
            Err(e) => {
                if tried == 1 {
                    warn!("{what} open failed ({e}); trying again for up to {:?}", delay * (attempts - 1));
                }
                std::thread::sleep(delay);
            }
        }
    }
}

fn low_battery_command(level: u8) -> Option<Command> {
    (level <= CRITICAL_BATTERY_LEVEL).then_some(Command::PowerOff)
}

/// The MCU sequence is over: power off was sent, or the lift was obstructed.
fn power_off_finished(phase: waved_state::PowerOffPhase) -> bool {
    matches!(phase, waved_state::PowerOffPhase::Sent | waved_state::PowerOffPhase::Aborted)
}

/// Why `run_async` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitCause {
    /// `Command::PowerOff`: the user powered off, or the battery is empty.
    PowerOff,
    /// SIGTERM, SIGINT, `RestartForConfig`, a pipeline fault, a closed bus.
    Other,
}

/// The wait before every exit. The `Command::Shutdown` that `run_async`
/// sends on the way out only stops the services; waved-mcu never powers off
/// for it, so `killall waved` (deploys, the S60waved respawn) is safe.
const EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// At or below this level the unit powers off (TODO F2).
const CRITICAL_BATTERY_LEVEL: u8 = 3;

/// Longest wait for the meeting to finalize its take before power is cut.
const MEETING_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Longest wait for waved-mcu's power-off sequence: the lift may take up to
/// 20 s to retract (`waved_mcu::power_off::Limits::STOCK`).
const POWER_OFF_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The env var that turns the power key reader on (RS7).
#[cfg(any(test, not(feature = "simulate")))]
const POWER_KEY_ENV: &str = "WAVED_POWER_KEY";

/// Only `WAVED_POWER_KEY=1` starts the power key reader (RS7).
#[cfg(any(test, not(feature = "simulate")))]
fn power_key_opted_in(value: Option<&str>) -> bool {
    value == Some("1")
}

/// After opting in: whether the power key reader started. Not starting is a
/// warning: the user asked for the key and it will not work.
#[cfg(any(test, not(feature = "simulate")))]
fn log_power_key_start(started: bool) {
    if started {
        info!("power key: {POWER_KEY_ENV}=1, reader started");
    } else {
        warn!("power key: {POWER_KEY_ENV}=1, but the reader did not start");
    }
}

fn publish_first_pipeline_fault(
    claimed: &std::sync::atomic::AtomicBool,
    integrity: &waved_audio::integrity::RecordingIntegrity,
    fatal_tx: &tokio::sync::mpsc::UnboundedSender<waved_state::RecordingIntegrityFault>,
    fault: waved_state::RecordingIntegrityFault,
) -> bool {
    if claimed.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return false;
    }
    integrity.record_fault(fault);
    let _ = fatal_tx.send(fault);
    true
}

async fn acknowledge_pipeline_fault(
    integrity_tx: &tokio::sync::mpsc::UnboundedSender<
        waved_audio::integrity::IntegritySignal,
    >,
    fault: waved_state::RecordingIntegrityFault,
    timeout: std::time::Duration,
) -> bool {
    let (acknowledgement, acknowledged) = tokio::sync::oneshot::channel();
    if integrity_tx
        .send(waved_audio::integrity::IntegritySignal::with_acknowledgement(
            fault,
            acknowledgement,
        ))
        .is_err()
    {
        return false;
    }
    tokio::time::timeout(timeout, acknowledged)
        .await
        .is_ok_and(|result| result.is_ok())
}

#[cfg(feature = "recording-integrity-test-hooks")]
async fn recording_integrity_test_hook(
    integrity: Arc<waved_audio::integrity::RecordingIntegrity>,
    integrity_tx: tokio::sync::mpsc::UnboundedSender<
        waved_audio::integrity::IntegritySignal,
    >,
    pipeline_fault_claimed: Arc<std::sync::atomic::AtomicBool>,
    pipeline_fatal_tx: tokio::sync::mpsc::UnboundedSender<waved_state::RecordingIntegrityFault>,
    commands: tokio::sync::broadcast::Sender<Command>,
) {
    use tokio::signal::unix::{signal, SignalKind};
    const HOOK_PATH: &str = "/tmp/waved-integrity-fault";
    let mut sighup = match signal(SignalKind::hangup()) {
        Ok(signal) => signal,
        Err(error) => {
            warn!("recording integrity test hook unavailable: {error}");
            return;
        }
    };
    warn!("recording integrity TEST HOOK ready at {HOOK_PATH}");

    while sighup.recv().await.is_some() {
        let raw = match std::fs::read_to_string(HOOK_PATH) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warn!("integrity test hook read failed: {error}");
                continue;
            }
        };
        let _ = std::fs::remove_file(HOOK_PATH);
        let action = raw.trim().to_ascii_lowercase();
        match action.as_str() {
            "start" => {
                warn!("TEST HOOK: start recording");
                let _ = commands.send(Command::StartRecording);
            }
            "stop" => {
                warn!("TEST HOOK: stop recording");
                let _ = commands.send(Command::StopRecording);
            }
            _ => {
                let Some(fault) = parse_integrity_test_fault(&action) else {
                    warn!("TEST HOOK: unknown recording integrity action {action:?}");
                    continue;
                };
                warn!("TEST HOOK: injecting {}", fault.diagnostic());
                if fault.requires_restart() {
                    publish_first_pipeline_fault(
                        &pipeline_fault_claimed,
                        &integrity,
                        &pipeline_fatal_tx,
                        fault,
                    );
                } else {
                    integrity.record_fault(fault);
                    let _ = integrity_tx
                        .send(waved_audio::integrity::IntegritySignal::new(fault));
                }
            }
        }
    }
}

#[cfg(feature = "recording-integrity-test-hooks")]
fn parse_integrity_test_fault(
    value: &str,
) -> Option<waved_state::RecordingIntegrityFault> {
    use waved_state::RecordingIntegrityFault;
    Some(match value {
        "capture-ended" => RecordingIntegrityFault::CaptureEnded,
        "capture-read-failed" => RecordingIntegrityFault::CaptureReadFailed,
        "capture-xrun" => RecordingIntegrityFault::CaptureXrun,
        "raw-frame-dropped" => RecordingIntegrityFault::RawFrameDropped,
        "raw-channel-closed" => RecordingIntegrityFault::RawFrameChannelClosed,
        "processed-frame-dropped" => RecordingIntegrityFault::ProcessedFrameDropped,
        "processed-channel-closed" => {
            RecordingIntegrityFault::ProcessedFrameChannelClosed
        }
        "wav-write-failed" => RecordingIntegrityFault::WavWriteFailed,
        _ => return None,
    })
}

/// SIGHUP handler: reloads beamforming test hooks from `/tmp`.
/// - `/tmp/waved-mode` (one of `omni`, `cardioid`, `supercardioid`, `figure8`,
///   `stereo`) → `Command::SetAudioMode`
/// - `/tmp/waved-doa-override` (`az_deg,el_deg` or `none` or missing) →
///   `Command::SetDoaOverride`
///
/// Used by `scripts/test-beamforming.sh` to drive mode + fixed-beam steering
/// without restarting waved.
async fn mode_switch_handler(commands: tokio::sync::broadcast::Sender<Command>) {
    use tokio::signal::unix::{signal, SignalKind};
    use waved_state::{AudioMode, NrLevel};

    let mut sighup = match signal(SignalKind::hangup()) {
        Ok(s) => s,
        Err(e) => { warn!("SIGHUP mode-switch handler unavailable: {e}"); return; }
    };

    info!(
        "SIGHUP handler ready (sources: /tmp/waved-mode, /tmp/waved-doa-override, /tmp/waved-nr, /tmp/waved-playback-preset, /tmp/waved-volume)"
    );

    while sighup.recv().await.is_some() {
        const MODE_PATH: &str = "/tmp/waved-mode";
        match std::fs::read_to_string(MODE_PATH) {
            Ok(raw) => {
                let mode = match raw.trim() {
                    "omni" => Some(AudioMode::Omni),
                    "cardioid" => Some(AudioMode::Cardioid),
                    "supercardioid" => Some(AudioMode::Supercardioid),
                    "figure8" => Some(AudioMode::Figure8),
                    "stereo" => Some(AudioMode::Stereo),
                    "" => None,
                    other => { warn!("SIGHUP: unknown mode {other:?}"); None }
                };
                if let Some(m) = mode {
                    info!("SIGHUP: setting audio mode → {m:?}");
                    let _ = commands.send(Command::SetAudioMode(m));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("SIGHUP: {MODE_PATH}: {e}"),
        }

        const NR_PATH: &str = "/tmp/waved-nr";
        match std::fs::read_to_string(NR_PATH) {
            Ok(raw) => {
                let level = match raw.trim().to_ascii_lowercase().as_str() {
                    "off" => Some(NrLevel::Off),
                    "low" => Some(NrLevel::Low),
                    "med" | "medium" => Some(NrLevel::Med),
                    "high" => Some(NrLevel::High),
                    "" => None,
                    other => {
                        warn!("SIGHUP: unknown NR level {other:?}");
                        None
                    }
                };
                if let Some(l) = level {
                    info!("SIGHUP: setting NR level → {l:?}");
                    let _ = commands.send(Command::SetNrLevel(l));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("SIGHUP: {NR_PATH}: {e}"),
        }

        const PRESET_PATH: &str = "/tmp/waved-playback-preset";
        match std::fs::read_to_string(PRESET_PATH) {
            Ok(raw) => {
                let preset = match raw.trim().to_ascii_lowercase().as_str() {
                    "voice" => Some(waved_state::PlaybackPreset::Voice),
                    "media" => Some(waved_state::PlaybackPreset::Media),
                    // Release the pin: the preset follows the speaker scene.
                    "auto" => {
                        info!("SIGHUP: playback preset → auto");
                        let _ = commands.send(Command::SetPlaybackPresetAuto);
                        None
                    }
                    "" => None,
                    other => {
                        warn!("SIGHUP: unknown playback preset {other:?}");
                        None
                    }
                };
                if let Some(p) = preset {
                    info!("SIGHUP: setting playback preset → {p:?}");
                    let _ = commands.send(Command::SetPlaybackPreset(p));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("SIGHUP: {PRESET_PATH}: {e}"),
        }

        // Speaker amplifier Set EQ MODE for A/B tests: 0/1/2 (or
        // bypass/meeting/music) pins it, `auto` follows the speaker scene.
        const AMP_EQ_PATH: &str = "/tmp/waved-amp-eq-mode";
        match std::fs::read_to_string(AMP_EQ_PATH) {
            Ok(raw) => match raw.trim() {
                "" => {}
                r if r.eq_ignore_ascii_case("auto") => {
                    info!("SIGHUP: speaker amp EQ mode → auto");
                    let _ = commands.send(Command::SetAmpEqMode(None));
                }
                r => match waved_state::AmpEqMode::parse(r) {
                    Some(mode) => {
                        info!("SIGHUP: speaker amp EQ mode → {mode:?}");
                        let _ = commands.send(Command::SetAmpEqMode(Some(mode)));
                    }
                    None => warn!("SIGHUP: unknown amp EQ mode {r:?}"),
                },
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("SIGHUP: {AMP_EQ_PATH}: {e}"),
        }

        const VOLUME_PATH: &str = "/tmp/waved-volume";
        match std::fs::read_to_string(VOLUME_PATH) {
            Ok(raw) => {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    match trimmed.parse::<u16>() {
                        Ok(v) if v <= 100 => {
                            info!("SIGHUP: setting speaker volume → {v}%");
                            let _ = commands.send(Command::SetSpeakerVolume(v));
                        }
                        Ok(v) => warn!("SIGHUP: {VOLUME_PATH}: {v} out of range (0..=100)"),
                        Err(e) => warn!("SIGHUP: {VOLUME_PATH}: parse error: {e}"),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("SIGHUP: {VOLUME_PATH}: {e}"),
        }

        const DOA_PATH: &str = "/tmp/waved-doa-override";
        match std::fs::read_to_string(DOA_PATH) {
            Ok(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
                    info!("SIGHUP: clearing DOA override");
                    let _ = commands.send(Command::SetDoaOverride(None));
                } else {
                    match parse_doa_override(trimmed) {
                        Some((az, el)) => {
                            info!("SIGHUP: DOA override → ({az:.1}°, {el:.1}°)");
                            let _ = commands.send(Command::SetDoaOverride(Some((az, el))));
                        }
                        None => warn!("SIGHUP: malformed {DOA_PATH}: {trimmed:?} (expected `az,el` or `none`)"),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("SIGHUP: {DOA_PATH}: {e}"),
        }
    }
}

/// Parse `"az_deg,el_deg"` into a pair of floats. Returns `None` on malformed input.
fn parse_doa_override(s: &str) -> Option<(f32, f32)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

#[cfg(test)]
mod exit_tests {
    use super::*;

    #[test]
    fn a_busy_device_is_retried_before_giving_up() {
        let mut calls = 0;
        let opened = open_retrying("speaker", 5, std::time::Duration::ZERO, || {
            calls += 1;
            if calls < 3 { Err("busy") } else { Ok(calls) }
        });
        assert_eq!(opened, Ok(3));

        let mut calls = 0;
        let failed: Result<(), &str> = open_retrying("speaker", 4, std::time::Duration::ZERO, || {
            calls += 1;
            Err("busy")
        });
        assert_eq!((failed, calls), (Err("busy"), 4));
    }

    #[test]
    fn only_a_critical_battery_powers_off_and_nothing_moves_the_lift() {
        assert_eq!(low_battery_command(9), None);
        assert_eq!(low_battery_command(4), None);
        assert_eq!(low_battery_command(3), Some(Command::PowerOff));
        assert_eq!(low_battery_command(0), Some(Command::PowerOff));
    }

    #[tokio::test]
    async fn power_off_cuts_power_only_after_the_meeting_finished() {
        // Review 2026-10-12: stock ends the meeting before the MCU sequence;
        // the take must be finalized before power can be cut.
        let (commands, mut rx) = tokio::sync::broadcast::channel(8);
        let (mcu_tx, mcu_rx) = tokio::sync::watch::channel(waved_state::McuState::default());
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(power_off_in_order(
            commands.clone(),
            async move {
                let _ = done_rx.await;
            },
            mcu_rx,
        ));
        assert_eq!(rx.recv().await.unwrap(), Command::Shutdown);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(rx.try_recv().is_err(), "power is not cut while the meeting finishes");
        done_tx.send(()).unwrap();
        assert_eq!(rx.recv().await.unwrap(), Command::PowerOffNow);
        mcu_tx.send_modify(|s| s.power_off = waved_state::PowerOffPhase::Sent);
        task.await.unwrap();
    }

    #[test]
    fn a_power_off_waits_until_the_mcu_sent_it_or_gave_up() {
        use waved_state::PowerOffPhase::*;
        assert!(!power_off_finished(Idle));
        assert!(!power_off_finished(InProgress));
        assert!(power_off_finished(Sent));
        assert!(power_off_finished(Aborted));
    }

    #[test]
    fn only_waved_power_key_1_starts_the_power_key() {
        assert!(power_key_opted_in(Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some(" 1")] {
            assert!(!power_key_opted_in(value), "{value:?}");
        }
    }

    #[test]
    fn an_opted_in_power_key_that_does_not_start_is_a_warning() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::{Context, SubscriberExt};

        struct Levels(Arc<Mutex<Vec<tracing::Level>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Levels {
            fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
                self.0.lock().unwrap().push(*event.metadata().level());
            }
        }

        let levels = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Levels(levels.clone()));
        tracing::subscriber::with_default(subscriber, || {
            log_power_key_start(true);
            log_power_key_start(false);
        });
        assert_eq!(
            *levels.lock().unwrap(),
            [tracing::Level::INFO, tracing::Level::WARN]
        );
    }
}

#[cfg(test)]
mod integrity_supervisor_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use waved_audio::integrity::RecordingIntegrity;
    use waved_state::RecordingIntegrityFault;

    #[test]
    fn only_the_first_terminal_pipeline_fault_is_published() {
        let claimed = AtomicBool::new(false);
        let integrity = RecordingIntegrity::default();
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<RecordingIntegrityFault>();

        assert!(publish_first_pipeline_fault(
            &claimed,
            &integrity,
            &tx,
            RecordingIntegrityFault::CaptureReadFailed,
        ));
        assert!(!publish_first_pipeline_fault(
            &claimed,
            &integrity,
            &tx,
            RecordingIntegrityFault::RawFrameChannelClosed,
        ));
        assert_eq!(
            rx.try_recv().unwrap(),
            RecordingIntegrityFault::CaptureReadFailed
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(integrity.epoch(), 1);
    }

    #[tokio::test]
    async fn fatal_pipeline_fault_waits_for_actor_acknowledgement() {
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<waved_audio::integrity::IntegritySignal>();
        let responder = tokio::spawn(async move {
            let signal = rx.recv().await.unwrap();
            assert_eq!(
                signal.fault,
                RecordingIntegrityFault::ProcessedFrameChannelClosed
            );
            signal.acknowledgement.unwrap().send(()).unwrap();
        });

        assert!(
            acknowledge_pipeline_fault(
                &tx,
                RecordingIntegrityFault::ProcessedFrameChannelClosed,
                std::time::Duration::from_secs(1),
            )
            .await
        );
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn fatal_pipeline_fault_timeout_is_bounded() {
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<waved_audio::integrity::IntegritySignal>();
        let holder = tokio::spawn(async move {
            let _signal = rx.recv().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        });

        assert!(
            !acknowledge_pipeline_fault(
                &tx,
                RecordingIntegrityFault::CaptureEnded,
                std::time::Duration::from_millis(10),
            )
            .await
        );
        holder.abort();
    }

    #[cfg(feature = "recording-integrity-test-hooks")]
    #[test]
    fn test_hook_accepts_only_fixed_fault_names() {
        assert_eq!(
            parse_integrity_test_fault("wav-write-failed"),
            Some(RecordingIntegrityFault::WavWriteFailed)
        );
        assert_eq!(
            parse_integrity_test_fault("processed-frame-dropped"),
            Some(RecordingIntegrityFault::ProcessedFrameDropped)
        );
        assert_eq!(parse_integrity_test_fault("anything else"), None);
    }
}

/// SIGUSR2 handler: plays `/data/waved/playback.s16le` (raw S16LE stereo
/// 48 kHz) through waved's own PlaybackService so the reference broadcast
/// tap captures every frame.  Pairs with `scripts/test-aec-conferencing.sh`
/// which generates a speech-band multi-tone signal as the "far-end" stream.
///
/// Because every played sample flows through the reference broadcast, AEC
/// sees the same signal the speaker emits and can cancel its echo from the
/// mic capture — exactly the path a real conferencing call takes.
#[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
async fn file_play_handler(playback: Arc<waved_audio::playback::PlaybackService>) {
    use std::time::Duration;
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigusr2 = match signal(SignalKind::user_defined2()) {
        Ok(s) => s,
        Err(e) => { warn!("SIGUSR2 file-play handler unavailable: {e}"); return; }
    };

    let out_ch = playback.channels() as usize;
    let period_frames = playback.period_frames();
    let period_samples = period_frames * out_ch;
    let sample_rate = playback.sample_rate();
    let batch_period = Duration::from_micros(
        (period_frames as u64 * 1_000_000) / sample_rate as u64,
    );

    info!("SIGUSR2 file-play handler ready (source: /data/waved/playback.s16le)");

    const PLAY_FILE: &str = "/data/waved/playback.s16le";
    const FILE_CH: usize = 2; // test-signal generator always produces stereo

    while sigusr2.recv().await.is_some() {
        let raw = match std::fs::read(PLAY_FILE) {
            Ok(b) => b,
            Err(e) => { warn!("SIGUSR2: {PLAY_FILE}: {e}"); continue; }
        };
        let file_samples: Vec<i16> = raw
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let total_frames = file_samples.len() / FILE_CH;
        info!("SIGUSR2: playing {total_frames} frames from {PLAY_FILE}");

        for p in 0.. {
            let frame_start = p * period_frames;
            if frame_start >= total_frames { break; }
            let mut batch = vec![0i16; period_samples];
            for i in 0..period_frames {
                let ff = frame_start + i;
                if ff >= total_frames { break; }
                // Mix stereo → mono, then replicate to all output channels.
                let l = file_samples[ff * FILE_CH] as i32;
                let r = file_samples[ff * FILE_CH + 1] as i32;
                let mono = ((l + r) / 2) as i16;
                for ch in 0..out_ch {
                    batch[i * out_ch + ch] = mono;
                }
            }
            if let Err(e) = playback.play_batch(batch) {
                warn!("SIGUSR2: play_batch: {e}; aborting");
                break;
            }
            tokio::time::sleep(batch_period).await;
        }
        info!("SIGUSR2: file playback complete");
    }
}

/// SIGUSR1 test-tone handler for SP#2 AEC smoke tests. Plays a 1 kHz sine
/// through waved's own PlaybackService so the broadcast reference tap picks
/// it up and AEC has something to cancel. Defaults match what the smoke-test
/// script expects; env vars override: `WAVED_TONE_FREQ_HZ`,
/// `WAVED_TONE_DURATION_MS`, `WAVED_TONE_AMPLITUDE`.
#[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
async fn test_tone_handler(playback: Arc<waved_audio::playback::PlaybackService>) {
    use std::time::Duration;
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigusr1 = match signal(SignalKind::user_defined1()) {
        Ok(s) => s,
        Err(e) => {
            warn!("SIGUSR1 test-tone handler unavailable: {e}");
            return;
        }
    };

    let channels = playback.channels() as usize;
    let period_frames = playback.period_frames();
    let period_samples = period_frames * channels;
    let sample_rate = playback.sample_rate();
    let batch_period = Duration::from_micros(
        (period_frames as u64 * 1_000_000) / sample_rate as u64,
    );

    info!("SIGUSR1 test-tone handler ready");

    while sigusr1.recv().await.is_some() {
        let freq_hz: f32 = std::env::var("WAVED_TONE_FREQ_HZ")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000.0);
        let duration_ms: u32 = std::env::var("WAVED_TONE_DURATION_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);
        let amplitude: f32 = std::env::var("WAVED_TONE_AMPLITUDE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.3f32)
            .clamp(0.0, 1.0);

        info!(
            "SIGUSR1: playing {:.0} Hz for {} ms (amplitude {:.2})",
            freq_hz, duration_ms, amplitude
        );

        let peak = (amplitude * i16::MAX as f32) as i32;
        let step = 2.0 * std::f32::consts::PI * freq_hz / sample_rate as f32;
        let n_periods = ((duration_ms as u64 * sample_rate as u64) / 1000 / period_frames as u64) as usize;
        let mut frame_idx: usize = 0;

        for _ in 0..n_periods {
            let mut batch = vec![0i16; period_samples];
            for i in 0..period_frames {
                let s = (((frame_idx + i) as f32 * step).sin() * peak as f32) as i16;
                for ch in 0..channels {
                    batch[i * channels + ch] = s;
                }
            }
            frame_idx += period_frames;
            if let Err(e) = playback.play_batch(batch) {
                warn!("test-tone play_batch failed: {e}; aborting");
                break;
            }
            tokio::time::sleep(batch_period).await;
        }

        info!("SIGUSR1: test tone complete");
    }
}
