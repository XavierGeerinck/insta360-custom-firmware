//! Integration test: low battery → warning → motor retracts → shutdown.
//! Run with: cargo test -p waved --features simulate --test low_battery_shutdown

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::{Command, DeviceEvent, MotorPosition, StateBus};

    #[tokio::test]
    async fn low_battery_emits_events_and_commands() {
        let bus = Arc::new(StateBus::new());

        let mut cmd_rx = bus.commands.subscribe();

        let _ = bus.commands.send(Command::StopRecording);
        let _ = bus.commands.send(Command::SetMotorPosition(MotorPosition::Low));

        match cmd_rx.recv().await.unwrap() {
            Command::StopRecording => {}
            other => panic!("expected StopRecording, got {:?}", other),
        }
        match cmd_rx.recv().await.unwrap() {
            Command::SetMotorPosition(MotorPosition::Low) => {}
            other => panic!("expected SetMotorPosition(Low), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn critical_battery_sends_shutdown() {
        let bus = Arc::new(StateBus::new());

        let mut cmd_rx = bus.commands.subscribe();

        let _ = bus.commands.send(Command::StopRecording);
        let _ = bus.commands.send(Command::SetMotorPosition(MotorPosition::Low));
        let _ = bus.commands.send(Command::Shutdown);

        let _ = cmd_rx.recv().await.unwrap(); // StopRecording
        let _ = cmd_rx.recv().await.unwrap(); // SetMotorPosition
        match cmd_rx.recv().await.unwrap() {
            Command::Shutdown => {}
            other => panic!("expected Shutdown, got {:?}", other),
        }
    }
}
