//! The UI thread: device setup and the loop that feeds the runtime. Kept
//! thin on purpose; everything it calls is host-tested in `runtime`.

use crate::display::FramebufferDisplay;
use crate::input::TouchInput;
use crate::kit::snapshot::UiSnapshot;
use crate::runtime::device::Device;
use crate::runtime::power::PowerDriver;
use crate::runtime::touch::{OpenRetry, TouchSample};
use crate::runtime::{registry, Runtime};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::error::TryRecvError;
use tracing::{info, warn};
use waved_config::WavedConfig;
use waved_state::StateBus;

/// Pause between loop iterations (about 100 iterations a second).
const LOOP_SLEEP: Duration = Duration::from_millis(10);

/// Runs the UI on the calling thread until the process exits. Returns only
/// when the display cannot be opened. A touch device that will not open does
/// not stop the UI: it keeps rendering and the open is retried every 2 s.
pub fn run(bus: Arc<StateBus>, config: WavedConfig) -> Result<(), Box<dyn std::error::Error>> {
    info!("UI thread starting");
    let device = Device::init(FramebufferDisplay::init()?);
    let mut touch: Option<TouchInput> = None;
    let mut touch_retry = OpenRetry::default();
    let mut events_rx = bus.events.subscribe();
    let mut commands_rx = bus.commands.subscribe();
    let mut runtime = Runtime::new(
        bus.commands.clone(),
        Arc::new(config),
        Box::new(registry::screen_for),
        PowerDriver::device(0),
    );
    let start = Instant::now();
    let mut last_tick = start;

    loop {
        let now = Instant::now();
        device.tick(now.duration_since(last_tick).as_millis() as u32);
        last_tick = now;
        let now_ms = now.duration_since(start).as_millis() as u64;
        // One snapshot for the touch sample and the step.
        let snap = UiSnapshot::capture(&bus, unix_now(), now_ms);

        if touch.is_none() && touch_retry.due(now_ms) {
            match TouchInput::init() {
                Ok(opened) => {
                    info!("touch input ready");
                    touch_retry.opened();
                    touch = Some(opened);
                }
                Err(error) => {
                    if touch_retry.failed(now_ms) {
                        warn!("touch input unavailable ({error}); the UI runs without it and retries every 2 s");
                    }
                }
            }
        }
        let sample = touch
            .as_mut()
            .map_or_else(TouchSample::default, TouchInput::poll);
        let report = runtime.touch(sample, &snap);
        device.report(report.sample);
        if report.cancel_press {
            device.cancel_press();
        }

        loop {
            match commands_rx.try_recv() {
                Ok(command) => runtime.observe_command(&command),
                Err(TryRecvError::Lagged(missed)) => warn!("UI missed {missed} commands"),
                Err(_) => break,
            }
        }
        let mut events = Vec::new();
        loop {
            match events_rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Lagged(missed)) => warn!("UI missed {missed} device events"),
                Err(_) => break,
            }
        }

        runtime.step(snap, &events);
        // The screen changed under a press: it clicks nothing on the new one.
        if runtime.take_cancel_press() {
            device.cancel_press();
        }
        device.run_timers();
        std::thread::sleep(LOOP_SLEEP);
    }
}

/// Wall-clock seconds since the Unix epoch (0 if the clock is before it).
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}
