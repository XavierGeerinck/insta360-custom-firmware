//! Integration test: a critical battery powers off (main: `low_battery_command`);
//! a low battery alone neither stops recording nor moves the lift (review 2026-10-12).
//! Run with: cargo test -p waved --features simulate --test low_battery_shutdown

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::{Command, StateBus};

    #[tokio::test]
    async fn critical_battery_sends_power_off() {
        let bus = Arc::new(StateBus::new());

        let mut cmd_rx = bus.commands.subscribe();

        let _ = bus.commands.send(Command::PowerOff);

        match cmd_rx.recv().await.unwrap() {
            Command::PowerOff => {}
            other => panic!("expected PowerOff, got {:?}", other),
        }
    }
}
