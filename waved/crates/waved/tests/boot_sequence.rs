//! Integration test: verify all simulated services start and state bus initializes.
//! Run with: cargo test -p waved --features simulate --test boot_sequence

#[cfg(feature = "simulate")]
mod tests {
    use std::sync::Arc;
    use waved_state::StateBus;

    #[tokio::test]
    async fn services_initialize_with_default_state() {
        let bus = Arc::new(StateBus::new());

        let battery = bus.battery.subscribe();
        assert_eq!(battery.borrow().level, 0);

        let wifi = bus.wifi.subscribe();
        assert!(!wifi.borrow().connected);

        let audio = bus.audio.subscribe();
        assert!(!audio.borrow().mic_muted);

        let meeting = bus.meeting.subscribe();
        assert!(!meeting.borrow().recording);
    }

    #[tokio::test]
    async fn sim_mcu_sets_default_battery() {
        let bus = Arc::new(StateBus::new());

        let (sim_mcu, _handle) = waved_mcu::simulate::SimMcuService::new(
            Arc::clone(&bus.battery),
            Arc::clone(&bus.mcu),
            bus.events.clone(),
        );

        let cmd_rx = bus.commands.subscribe();
        std::thread::spawn(move || sim_mcu.run(cmd_rx));

        // The simulated MCU sends heartbeats; waved-mcu trusts the level once
        // two of them agree (about a second).
        let mut battery = bus.battery.subscribe();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            battery.wait_for(|b| b.level == 75),
        )
        .await
        .expect("the simulated heartbeats set the battery")
        .unwrap();
        assert!(!battery.borrow().is_charging);
    }

    #[tokio::test]
    async fn sim_mcu_moves_the_lift_for_mic_height() {
        use waved_state::{Command, LiftState, MotorPosition};
        let bus = Arc::new(StateBus::new());
        let (sim_mcu, handle) = waved_mcu::simulate::SimMcuService::new(
            Arc::clone(&bus.battery),
            Arc::clone(&bus.mcu),
            bus.events.clone(),
        );
        let cmd_rx = bus.commands.subscribe();
        std::thread::spawn(move || sim_mcu.run(cmd_rx));

        let mut mcu = bus.mcu.subscribe();
        bus.commands.send(Command::SetMotorPosition(MotorPosition::High)).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            mcu.wait_for(|s| s.lift == LiftState::Idle && s.motor_position == MotorPosition::High),
        )
        .await
        .expect("the simulated lift reaches High")
        .unwrap();
        assert_eq!(handle.mcu().lift_mm(), 50);
    }

    #[tokio::test]
    async fn sim_wifi_reports_connected() {
        let bus = Arc::new(StateBus::new());

        let sim_wifi = waved_wifi::simulate::SimWifiService::new(
            Arc::clone(&bus.wifi),
            bus.events.clone(),
        );

        // Subscribe before spawning to avoid missing the send.
        let wifi_rx = bus.wifi.subscribe();

        let sim_cmds = bus.commands.subscribe();
        tokio::spawn(async move { sim_wifi.run(sim_cmds).await });

        // Yield to allow the spawned task to execute its initial send.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        assert!(wifi_rx.borrow().connected);
        assert_eq!(wifi_rx.borrow().ssid, "SimNetwork");
    }
}
