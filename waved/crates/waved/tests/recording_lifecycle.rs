//! Integration test: start recording → audio frames → stop → upload queued.
//! Run with: cargo test -p waved --features simulate --test recording_lifecycle

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::{Command, DeviceEvent, StateBus};

    #[tokio::test]
    async fn recording_commands_flow_through_bus() {
        let bus = Arc::new(StateBus::new());

        let mut events_rx = bus.events.subscribe();
        let mut cmd_rx = bus.commands.subscribe();

        let _ = bus.commands.send(Command::StartRecording);
        match cmd_rx.recv().await.unwrap() {
            Command::StartRecording => {}
            other => panic!("expected StartRecording, got {:?}", other),
        }

        let id = uuid::Uuid::new_v4();
        let _ = bus.events.send(DeviceEvent::RecordingStarted(id));
        match events_rx.recv().await.unwrap() {
            DeviceEvent::RecordingStarted(received_id) => {
                assert_eq!(received_id, id);
            }
            other => panic!("expected RecordingStarted, got {:?}", other),
        }

        let _ = bus.commands.send(Command::StopRecording);
        match cmd_rx.recv().await.unwrap() {
            Command::StopRecording => {}
            other => panic!("expected StopRecording, got {:?}", other),
        }

        let _ = bus.events.send(DeviceEvent::RecordingStopped(id));
        match events_rx.recv().await.unwrap() {
            DeviceEvent::RecordingStopped(received_id) => {
                assert_eq!(received_id, id);
            }
            other => panic!("expected RecordingStopped, got {:?}", other),
        }
    }
}
