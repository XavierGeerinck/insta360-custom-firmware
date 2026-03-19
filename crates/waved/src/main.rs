use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use waved_state::{Command, DeviceEvent, MotorPosition, StateBus};

const CONFIG_PATH: &str = "/data/waved/config.toml";
const DB_PATH: &str = "/data/waved/meetings.db";
const RECORDINGS_DIR: &str = "/mnt/udisk/recordings";
const UART_PORT: &str = "/dev/ttyS1";
const UART_BAUD: u32 = 115200;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("waved=info".parse().unwrap()),
        )
        .init();

    info!("waved v{} starting", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = waved_config::WavedConfig::load_or_default(CONFIG_PATH.as_ref());
    info!("config loaded: device={} audio_mode={:?}", config.device.name, config.audio.mode);

    // Create state bus
    let bus = Arc::new(StateBus::new());

    // Open UART mux — errata #2: take receivers before wrapping in Arc
    let uart_result = waved_uart::UartMux::open(UART_PORT, UART_BAUD);

    match uart_result {
        Ok(mut uart) => {
            info!("UART opened: {UART_PORT} @ {UART_BAUD}");
            let mcu_rx = uart.take_mcu_receiver();
            let bt_rx = uart.take_bt_receiver();
            let uart = Arc::new(uart);

            // Spawn MCU service on dedicated thread
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

            // Spawn BT service on dedicated thread
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

    // Errata #10: single async runner (no duplicate functions)
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(run_async(config, bus));

    info!("waved shutdown complete");
}

async fn run_async(config: waved_config::WavedConfig, bus: Arc<StateBus>) {
    // Audio service
    let (frame_tx, frame_rx) = waved_audio::callbacks::create_audio_channel();
    let audio_svc = waved_audio::AudioService::new(Arc::clone(&bus.audio), frame_tx);
    let audio_cmd_rx = bus.commands.subscribe();
    tokio::spawn(async move { audio_svc.run(audio_cmd_rx).await });

    // HID service
    let hid_events_rx = bus.events.subscribe();
    tokio::spawn(async move { waved_hid::HidService::run(hid_events_rx).await });

    // WiFi service (errata #6: NTP sync via config)
    let wifi_svc = waved_wifi::WifiService::new(
        Arc::clone(&bus.wifi),
        bus.events.clone(),
        &config,
    );
    tokio::spawn(async move { wifi_svc.run().await });

    // Meeting service
    // Errata #4: pass audio frame receiver to meeting service
    match waved_meeting::MeetingService::new(
        config.clone(),
        DB_PATH,
        PathBuf::from(RECORDINGS_DIR),
        Arc::clone(&bus.meeting),
        bus.events.clone(),
        frame_rx,
    ).await {
        Ok(meeting_svc) => {
            let meeting_cmd_rx = bus.commands.subscribe();
            let wifi_rx = bus.wifi.subscribe();
            tokio::spawn(async move { meeting_svc.run(meeting_cmd_rx, wifi_rx).await });
        }
        Err(e) => error!("meeting service init failed: {e}"),
    }

    info!("all services started, waiting for shutdown signal");

    // Errata #9: low battery handler
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
                // Errata #11: handle broadcast::Lagged
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("battery monitor lagged by {n} events");
                }
            }
        }
    });

    // Errata #5: Wait for SIGTERM or SIGINT
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).unwrap();
    let mut sigint = signal(SignalKind::interrupt()).unwrap();
    tokio::select! {
        _ = sigterm.recv() => info!("received SIGTERM"),
        _ = sigint.recv() => info!("received SIGINT"),
    }

    info!("shutdown signal received");

    // Graceful shutdown: send shutdown command
    let _ = bus.commands.send(Command::Shutdown);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}
