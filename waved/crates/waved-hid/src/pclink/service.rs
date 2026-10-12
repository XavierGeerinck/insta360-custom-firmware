//! Runs the PC link: an engine thread fed by `HidReader::spawn_with_vendor`,
//! and a writer thread that owns the PC-link writes to `/dev/hidg0` (the
//! consumer / telephony writer keeps its own handle; f_hid serialises them).

use std::io::Write;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_state::{AudioState, BatteryState, Command, MeetingState, SettingsState, StateBus, SystemState, WifiState};

use super::handler::{Context, PcLinkHost, PinVerifier};
use super::link::{Output, PcLink};
use super::snapshot::Snapshot;
use super::vendor::Report;

/// Housekeeping tick: resend requests, sync, heartbeat checks.
pub const TICK: Duration = Duration::from_millis(20);
/// Reports waiting for the writer; beyond this the newest are dropped.
pub const WRITE_QUEUE: usize = 512;

#[derive(Debug, Clone)]
pub struct PcLinkConfig {
    /// `/dev/hidg0`.
    pub device: String,
}

/// The on-device PIN prompt (K2's PIN screen): the link sends `()` on
/// `requests` when the app asks for it; the screen answers on `results`
/// (`true` = the right PIN was entered).
pub struct PinPrompt {
    pub requests: Sender<()>,
    pub results: Receiver<bool>,
}

/// The state the replies read and the bus the commands go to.
pub struct PcLinkBus {
    pub audio: watch::Receiver<AudioState>,
    pub meeting: watch::Receiver<MeetingState>,
    pub battery: watch::Receiver<BatteryState>,
    pub wifi: watch::Receiver<WifiState>,
    pub settings: watch::Receiver<SettingsState>,
    pub system: watch::Receiver<SystemState>,
    pub commands: broadcast::Sender<Command>,
}

impl PcLinkBus {
    pub fn from_bus(bus: &StateBus) -> Self {
        Self {
            audio: bus.audio.subscribe(),
            meeting: bus.meeting.subscribe(),
            battery: bus.battery.subscribe(),
            wifi: bus.wifi.subscribe(),
            settings: bus.settings.subscribe(),
            system: bus.system.subscribe(),
            commands: bus.commands.clone(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot::from_state(
            &self.audio.borrow(),
            &self.meeting.borrow(),
            &self.battery.borrow(),
            &self.wifi.borrow(),
            &self.settings.borrow(),
            &self.system.borrow(),
        )
    }
}

/// Start the PC link. `vendor_rx` is the channel given to
/// `HidReader::spawn_with_vendor`. Returns at once; the threads run until
/// the reader's sender is dropped.
pub fn spawn(
    cfg: PcLinkConfig,
    vendor_rx: Receiver<Vec<u8>>,
    bus: PcLinkBus,
    pins: Box<dyn PinVerifier>,
    host: Box<dyn PcLinkHost>,
    prompt: Option<PinPrompt>,
) {
    let (write_tx, write_rx) = mpsc::sync_channel(WRITE_QUEUE);
    let device = cfg.device.clone();
    std::thread::Builder::new()
        .name("pclink-write".into())
        .spawn(move || run_writer(device, write_rx))
        .expect("spawn pclink writer");
    std::thread::Builder::new()
        .name("pclink".into())
        .spawn(move || run_engine(vendor_rx, bus, pins, host, prompt, write_tx))
        .expect("spawn pclink engine");
    info!("pclink: Wave Controller link on {} (vendor report 0x03)", cfg.device);
}

/// The engine loop; returns when `vendor_rx` disconnects.
pub fn run_engine(
    vendor_rx: Receiver<Vec<u8>>,
    bus: PcLinkBus,
    mut pins: Box<dyn PinVerifier>,
    mut host: Box<dyn PcLinkHost>,
    prompt: Option<PinPrompt>,
    write_tx: SyncSender<Report>,
) {
    let start = Instant::now();
    let mut link = PcLink::new();
    let mut last_poll = Duration::ZERO;
    let mut dropped = 0u64;
    loop {
        let report = match vendor_rx.recv_timeout(TICK) {
            Ok(report) => Some(report),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                info!("pclink: reader gone, stopping");
                return;
            }
        };
        if let Some(prompt) = &prompt {
            while let Ok(verified) = prompt.results.try_recv() {
                link.pin_prompt_finished(verified);
            }
        }
        let elapsed = start.elapsed();
        let now_ms = elapsed.as_millis() as u64;
        let snapshot = bus.snapshot();
        let mut ctx = Context {
            snapshot: &snapshot,
            now_ms,
            pins: pins.as_mut(),
            host: host.as_mut(),
            prompt_available: prompt.is_some(),
        };
        let mut outputs = Vec::new();
        if let Some(report) = report {
            outputs.push(link.on_report(&report, &mut ctx));
        }
        if elapsed.saturating_sub(last_poll) >= TICK {
            last_poll = elapsed;
            outputs.push(link.poll(&mut ctx));
        }
        for out in outputs {
            deliver(out, &bus, prompt.as_ref(), &write_tx, &mut dropped);
        }
    }
}

fn deliver(out: Output, bus: &PcLinkBus, prompt: Option<&PinPrompt>, write_tx: &SyncSender<Report>, dropped: &mut u64) {
    // Act first, then ack: whoever sees the ack can rely on the command.
    for command in out.commands {
        info!("pclink: app sent {command:?}");
        let _ = bus.commands.send(command);
    }
    if out.prompt_pin {
        if let Some(prompt) = prompt {
            let _ = prompt.requests.send(());
        }
    }
    for report in out.reports {
        match write_tx.try_send(report) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                *dropped += 1;
                if dropped.is_power_of_two() {
                    warn!("pclink: writer behind, {dropped} reports dropped");
                }
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

/// The writer loop: one 64-byte report per write; reopens `device` after an
/// error. Returns when `rx` disconnects.
pub fn run_writer(device: String, rx: Receiver<Report>) {
    const RETRY: Duration = Duration::from_secs(1);
    let mut file: Option<std::fs::File> = None;
    let mut next_open = Instant::now();
    for report in rx {
        if file.is_none() && Instant::now() >= next_open {
            match std::fs::OpenOptions::new().write(true).open(&device) {
                Ok(f) => file = Some(f),
                Err(e) => {
                    warn!("pclink: cannot open {device}: {e}");
                    next_open = Instant::now() + RETRY;
                }
            }
        }
        let Some(f) = file.as_mut() else { continue };
        if let Err(e) = f.write_all(&report) {
            warn!("pclink: write to {device} failed: {e}");
            file = None;
            next_open = Instant::now() + RETRY;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pclink::handler::{DenyAllPins, NoClock};
    use crate::pclink::vendor::REPORT_LEN;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    struct Running {
        vendor_tx: Sender<Vec<u8>>,
        written: Receiver<Report>,
        commands: broadcast::Receiver<Command>,
        engine: std::thread::JoinHandle<()>,
        _bus: StateBus,
    }

    fn start(prompt: Option<PinPrompt>) -> Running {
        let bus = StateBus::new();
        let commands = bus.commands.subscribe();
        let (vendor_tx, vendor_rx) = mpsc::channel();
        let (write_tx, written) = mpsc::sync_channel(WRITE_QUEUE);
        let link_bus = PcLinkBus::from_bus(&bus);
        let engine = std::thread::spawn(move || {
            run_engine(vendor_rx, link_bus, Box::new(DenyAllPins), Box::new(NoClock), prompt, write_tx)
        });
        Running { vendor_tx, written, commands, engine, _bus: bus }
    }

    fn heartbeat_request() -> Vec<u8> {
        let mut r = vec![0u8; REPORT_LEN];
        r[..2].copy_from_slice(&[0x03, 0xEC]);
        r
    }

    #[test]
    fn a_heartbeat_request_is_written_back_and_the_engine_stops_with_the_reader() {
        let run = start(None);
        run.vendor_tx.send(heartbeat_request()).unwrap();
        let reply = run.written.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(&reply[..3], &[0x03, 0xEF, 0x3A]);
        drop(run.vendor_tx);
        run.engine.join().unwrap();
    }

    #[test]
    fn a_command_reaches_the_bus_and_its_ack_the_writer() {
        let mut run = start(None);
        // SET_AUDIO_MIC_MUTE(1), as the host sends it (reference model).
        let frame = {
            let mut p = hex("4A 49 48 47 01 24 2B 00 05 00 00 00 01 00 00 00 01");
            p.resize(128, 0xFF);
            waved_mavlink::encode(waved_mavlink::PC_LINK, waved_mavlink::Header::default(), 1000, &p).unwrap()
        };
        for (i, chunk) in frame.chunks(52).enumerate() {
            let r = crate::pclink::vendor::Packet::mavlink(i as u64 + 1, chunk).encode();
            run.vendor_tx.send(r.to_vec()).unwrap();
        }
        let mut acks = Vec::new();
        while acks.len() < 3 {
            acks.push(run.written.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        assert_eq!(acks[0][1], 0x00, "MAVLINK chunks");
        assert_eq!(run.commands.try_recv().unwrap(), Command::ToggleMicMute);
    }

    #[test]
    fn the_pin_prompt_is_requested_and_its_answer_used() {
        let (req_tx, req_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let run = start(Some(PinPrompt { requests: req_tx, results: res_rx }));
        // Give the device a PIN.
        run._bus.settings.send_modify(|s| s.pin_enabled = true);
        let verify = |op: u8| {
            let mut data = vec![0u8; 33];
            data[0] = op;
            let mut seq = 0;
            crate::pclink::packet::segment_command(crate::pclink::packet::DEFAULT_TOKEN, 0x11, &data, &mut seq)[0]
        };
        let mut send_id = 0;
        let mut mav_seq = 0;
        let mut send = |p: [u8; 128]| {
            let h = waved_mavlink::Header { seq: mav_seq, ..Default::default() };
            mav_seq += 1;
            let frame = waved_mavlink::encode(waved_mavlink::PC_LINK, h, 1000, &p).unwrap();
            for chunk in frame.chunks(52) {
                send_id += 1;
                run.vendor_tx.send(crate::pclink::vendor::Packet::mavlink(send_id, chunk).encode().to_vec()).unwrap();
            }
        };
        send(verify(0));
        req_rx.recv_timeout(Duration::from_secs(2)).expect("prompt requested");
        res_tx.send(true).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        send(verify(1));
        // Collect the second ack and find its result byte (packet payload byte 0).
        let mut stream = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut parser = waved_mavlink::Parser::new(waved_mavlink::PC_LINK);
        let mut results = Vec::new();
        while results.len() < 2 && Instant::now() < deadline {
            if let Ok(r) = run.written.recv_timeout(Duration::from_millis(100)) {
                if let Some(crate::pclink::vendor::Incoming::Transport(p)) = crate::pclink::vendor::decode(&r) {
                    if p.kind == crate::pclink::vendor::Kind::Mavlink {
                        stream.extend(p.payload);
                    }
                }
            }
            for b in std::mem::take(&mut stream) {
                if let Some(m) = parser.feed(b) {
                    results.push(m.payload[16]);
                }
            }
        }
        assert_eq!(results, vec![0, 1], "accepted, then verified");
    }

    /// The exact calls `crates/waved/src/main.rs` makes (the binary can't be
    /// built on macOS); with no device both threads wind down on their own.
    #[test]
    fn the_main_rs_wiring_type_checks_and_stops_without_a_device() {
        use crate::reader::{HidReader, HidReaderConfig};
        let bus = std::sync::Arc::new(StateBus::new());
        let hid_stats = crate::stats::HidStats::new();
        let device = "/nonexistent/hidg0".to_string();
        let (pclink_tx, pclink_rx) = std::sync::mpsc::channel();
        HidReader::spawn_with_vendor(
            HidReaderConfig { device: device.clone() },
            std::sync::Arc::clone(&bus.audio),
            std::sync::Arc::clone(&hid_stats),
            Some(pclink_tx),
        );
        spawn(
            PcLinkConfig { device },
            pclink_rx,
            PcLinkBus::from_bus(&bus),
            Box::new(DenyAllPins),
            Box::new(NoClock),
            None,
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn the_writer_writes_whole_reports_and_survives_a_missing_device() {
        let dir = std::env::temp_dir().join(format!("pclink-writer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hidg0");
        std::fs::File::create(&path).unwrap();
        let (tx, rx) = mpsc::sync_channel(4);
        let p = path.to_string_lossy().into_owned();
        let writer = std::thread::spawn(move || run_writer(p, rx));
        let mut r = [0u8; REPORT_LEN];
        r[0] = 0x03;
        r[1] = 0xEF;
        tx.send(r).unwrap();
        tx.send(r).unwrap();
        drop(tx);
        writer.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap().len(), 2 * REPORT_LEN);

        let (tx, rx) = mpsc::sync_channel(4);
        let missing = dir.join("nope").to_string_lossy().into_owned();
        let writer = std::thread::spawn(move || run_writer(missing, rx));
        tx.send(r).unwrap();
        drop(tx);
        writer.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
