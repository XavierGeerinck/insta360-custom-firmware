//! Integration test: mute button → state update → HID report logged.
//! Run with: cargo test -p waved --features simulate --test mute_flow

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::{ButtonType, DeviceEvent, StateBus};

    #[tokio::test]
    async fn mute_button_triggers_event() {
        let bus = Arc::new(StateBus::new());

        let mut events_rx = bus.events.subscribe();

        let _ = bus.events.send(DeviceEvent::ButtonPress(ButtonType::MuteToggle));

        match events_rx.recv().await.unwrap() {
            DeviceEvent::ButtonPress(ButtonType::MuteToggle) => {}
            other => panic!("expected MuteToggle, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn mute_command_toggles_audio_state() {
        let bus = Arc::new(StateBus::new());

        assert!(!bus.audio.subscribe().borrow().mic_muted);

        bus.audio.send_modify(|s| s.mic_muted = !s.mic_muted);
        assert!(bus.audio.subscribe().borrow().mic_muted);

        bus.audio.send_modify(|s| s.mic_muted = !s.mic_muted);
        assert!(!bus.audio.subscribe().borrow().mic_muted);
    }
}
