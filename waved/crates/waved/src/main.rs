#[cfg(all(feature = "simulate", feature = "capture-alsa"))]
compile_error!("features `simulate` and `capture-alsa` are mutually exclusive");

use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use waved_state::{Command, DeviceEvent, MotorPosition, StateBus};

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
const UART_BAUD: u32 = 115200;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("waved=info".parse().unwrap())
                .add_directive("waved_ui=info".parse().unwrap())
                .add_directive("waved_audio=info".parse().unwrap()),
        )
        .init();

    #[cfg(feature = "simulate")]
    info!("waved v{} starting [SIMULATION MODE]", env!("CARGO_PKG_VERSION"));
    #[cfg(not(feature = "simulate"))]
    info!("waved v{} starting", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = waved_config::WavedConfig::load_or_default(CONFIG_PATH.as_ref());
    info!("config loaded: device={} audio_mode={:?}", config.device.name, config.audio.mode);

    // Create state bus
    let bus = Arc::new(StateBus::new());

    // --- Hardware services: real or simulated ---
    #[cfg(not(feature = "simulate"))]
    {
        let uart_result = waved_uart::UartMux::open(UART_PORT, UART_BAUD);

        match uart_result {
            Ok(mut uart) => {
                info!("UART opened: {UART_PORT} @ {UART_BAUD}");
                let mcu_rx = uart.take_mcu_receiver();
                let bt_rx = uart.take_bt_receiver();
                let uart = Arc::new(uart);

                let mcu_uart = Arc::clone(&uart);
                let mcu_battery_tx = Arc::clone(&bus.battery);
                let mcu_state_tx = Arc::clone(&bus.mcu);
                let mcu_events_tx = bus.events.clone();
                let mcu_cmd_rx = bus.commands.subscribe();
                std::thread::Builder::new()
                    .name("mcu-srv".into())
                    .spawn(move || {
                        let svc = waved_mcu::McuService::new(
                            mcu_rx, mcu_uart, mcu_battery_tx, mcu_state_tx, mcu_events_tx,
                        );
                        svc.run(mcu_cmd_rx);
                    })
                    .expect("failed to spawn mcu thread");

                let bt_uart = Arc::clone(&uart);
                let bt_state_tx = Arc::clone(&bus.bt);
                let bt_events_tx = bus.events.clone();
                std::thread::Builder::new()
                    .name("bt-srv".into())
                    .spawn(move || {
                        let svc = waved_bt::BtService::new(bt_rx, bt_uart, bt_state_tx, bt_events_tx);
                        svc.run();
                    })
                    .expect("failed to spawn bt thread");
            }
            Err(e) => {
                error!("failed to open UART {UART_PORT}: {e}");
                error!("running without MCU/BT (host development mode)");
            }
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

    rt.block_on(run_async(config, bus));

    info!("waved shutdown complete");
}

async fn run_async(config: waved_config::WavedConfig, bus: Arc<StateBus>) {
    // Meeting frame channel: PCM frames for recording.
    // Non-simulate: C SDK callbacks → frame channel → AudioService (VU) → meeting channel → MeetingService
    // Simulate: SimAudioService writes silence frames directly to meeting channel.
    let (meeting_tx, meeting_rx) = waved_audio::callbacks::create_audio_channel();

    // Storage for the capture stop flag so the shutdown path can signal the ALSA thread.
    // The same cfg condition ensures this compiles away when capture-alsa is disabled.
    #[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
    let capture_stop_flag: Arc<std::sync::atomic::AtomicBool>;

    #[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
    {
        use std::sync::atomic::AtomicBool;
        use std::path::Path;
        use waved_audio::capture::{
            alsa::AlsaPcmSource, run_capture_loop, CaptureStats, ChannelMap,
        };
        use waved_audio::stats_collector;

        let (map, map_src) = ChannelMap::load_or_default(
            Path::new("/etc/channel_map_config_default.json")
        );
        let capture_stats = CaptureStats::new();
        let capture_stop = Arc::new(AtomicBool::new(false));
        // Keep a reference outside this block so the shutdown path can set the flag.
        capture_stop_flag = Arc::clone(&capture_stop);

        let (frame_tx, frame_rx) = waved_audio::callbacks::create_audio_channel();
        let audio_svc = waved_audio::AudioService::new(
            Arc::clone(&bus.audio), frame_tx.clone(), frame_rx, meeting_tx,
        );
        let stats_rx = audio_svc.subscribe_frames();
        tokio::spawn(stats_collector::run(
            stats_rx,
            Arc::clone(&capture_stats),
            map_src,
            std::path::PathBuf::from("/tmp/audio_stats.json"),
            std::time::Duration::from_millis(500),
        ));

        let src = match AlsaPcmSource::open("hw:0,0", 48000, map.total_channels as u32, 256, 4) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("ALSA capture open failed: {e}");
                std::process::exit(1);
            }
        };
        let stats_c = Arc::clone(&capture_stats);
        let stop_c = Arc::clone(&capture_stop);
        let map_c = map.clone();
        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                run_capture_loop(src, map_c, 256, frame_tx, stop_c, stats_c);
            })
            .expect("spawn capture thread");
        let audio_cmd_rx = bus.commands.subscribe();
        tokio::spawn(async move { audio_svc.run(audio_cmd_rx).await });
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

    // HID service
    let hid_events_rx = bus.events.subscribe();
    #[cfg(not(feature = "simulate"))]
    tokio::spawn(async move { waved_hid::HidService::run(hid_events_rx).await });
    #[cfg(feature = "simulate")]
    tokio::spawn(async move { waved_hid::simulate::SimHidService::run(hid_events_rx).await });

    // WiFi service
    #[cfg(not(feature = "simulate"))]
    {
        let wifi_svc = waved_wifi::WifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
            &config,
        );
        tokio::spawn(async move { wifi_svc.run().await });
    }
    #[cfg(feature = "simulate")]
    {
        let sim_wifi = waved_wifi::simulate::SimWifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
        );
        tokio::spawn(async move { sim_wifi.run().await });
    }

    // Meeting service
    match waved_meeting::MeetingService::new(
        config.clone(),
        DB_PATH,
        PathBuf::from(RECORDINGS_DIR),
        Arc::clone(&bus.meeting),
        bus.events.clone(),
        meeting_rx,
    ).await {
        Ok(meeting_svc) => {
            let meeting_cmd_rx = bus.commands.subscribe();
            let wifi_rx = bus.wifi.subscribe();
            tokio::spawn(async move { meeting_svc.run(meeting_cmd_rx, wifi_rx).await });
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
                    if level < 10 {
                        warn!("low battery ({level}%), stopping recording and retracting motor");
                        let _ = low_battery_bus.commands.send(Command::StopRecording);
                        let _ = low_battery_bus.commands.send(Command::SetMotorPosition(MotorPosition::Low));
                    }
                    if level < 5 {
                        warn!("critical battery ({level}%), shutting down");
                        let _ = low_battery_bus.commands.send(Command::Shutdown);
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

    // Wait for SIGTERM or SIGINT
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).unwrap();
    let mut sigint = signal(SignalKind::interrupt()).unwrap();
    tokio::select! {
        _ = sigterm.recv() => info!("received SIGTERM"),
        _ = sigint.recv() => info!("received SIGINT"),
    }

    info!("shutdown signal received");
    let _ = bus.commands.send(Command::Shutdown);
    // Signal the ALSA capture thread to stop before waiting for drain.
    #[cfg(all(not(feature = "simulate"), feature = "capture-alsa"))]
    capture_stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}
