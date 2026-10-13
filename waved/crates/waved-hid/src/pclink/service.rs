//! Runs the PC link: an engine thread fed by `HidReader::spawn_with_vendor`,
//! and a writer thread that owns the PC-link writes to `/dev/hidg0` (the
//! consumer / telephony writer keeps its own handle; f_hid serialises them).
//! The writer opens the node again after errors and gadget switches, and
//! closes it while USB storage holds it for a switch (`crate::node`).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, watch};
use tracing::{info, warn};
use waved_state::{
    AudioState, BatteryState, Command, GadgetHold, GadgetHoldUser, MeetingState, SettingsState, StateBus, SystemState,
    WifiState,
};

use crate::node::{
    DevNode, Handle, Hold, Node, Opened, Pending, Reopening, Unavailable, HOLD_CHECK, REOPEN_EVERY, STALE_AFTER,
};
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
    /// USB storage's hold on the node (`StateBus::hid_hold`); `None`: no
    /// gadget switch to follow.
    pub hold: Option<GadgetHold>,
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
/// the reader's sender is dropped (the reader keeps it for the process
/// lifetime).
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
    // Registered before the thread runs: a switch from now on waits for it.
    let hold = cfg.hold.as_ref().map(|hold| hold.register("the PC link writer"));
    std::thread::Builder::new()
        .name("pclink-write".into())
        .spawn(move || run_writer_on(DevNode::writer(&device), &device, REOPEN_EVERY, write_rx, hold))
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
    let mut outbox = Outbox::default();
    loop {
        let report = match vendor_rx.recv_timeout(outbox.wait()) {
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
        let poll_due = elapsed.saturating_sub(last_poll) >= TICK;
        if report.is_none() && !poll_due {
            outbox.flush(&write_tx);
            continue;
        }
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
        if poll_due {
            last_poll = elapsed;
            outputs.push(link.poll(&mut ctx));
        }
        for out in outputs {
            deliver(out, &bus, prompt.as_ref(), &mut outbox);
        }
        outbox.flush(&write_tx);
    }
}

fn deliver(out: Output, bus: &PcLinkBus, prompt: Option<&PinPrompt>, outbox: &mut Outbox) {
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
    outbox.extend(out.reports);
}

/// Reports waiting for the writer, so a host that reads slower than we
/// answer loses nothing. Bounded: past `OUTBOX_MAX` the oldest go (the app
/// has not read for seconds; the transport's resend covers what it missed).
#[derive(Default)]
struct Outbox {
    queue: std::collections::VecDeque<Report>,
    dropped: u64,
    /// Flushes in a row that handed nothing over (the writer is stuck).
    stalls: u32,
}

/// About 13 s of a full-speed interrupt endpoint (1 report/ms).
pub const OUTBOX_MAX: usize = 8192;

impl Outbox {
    /// How long the engine may sleep before handing reports over again:
    /// 1 ms while the writer takes them, doubling up to the tick while it
    /// takes none (a write blocked on a host that stopped reading).
    fn wait(&self) -> Duration {
        if self.queue.is_empty() {
            return TICK;
        }
        Duration::from_millis(1 << self.stalls.min(5)).min(TICK)
    }

    fn extend(&mut self, reports: Vec<Report>) {
        self.queue.extend(reports);
        while self.queue.len() > OUTBOX_MAX {
            self.queue.pop_front();
            self.dropped += 1;
            if self.dropped.is_power_of_two() {
                warn!("pclink: the app is not reading, {} reports dropped", self.dropped);
            }
        }
    }

    /// Hand over as many reports as the writer takes now.
    fn flush(&mut self, write_tx: &SyncSender<Report>) {
        let before = self.queue.len();
        while let Some(report) = self.queue.pop_front() {
            match write_tx.try_send(report) {
                Ok(()) => {}
                Err(TrySendError::Full(report)) => {
                    self.queue.push_front(report);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.queue.clear();
                    break;
                }
            }
        }
        let stuck = !self.queue.is_empty() && self.queue.len() == before;
        self.stalls = if stuck { self.stalls.saturating_add(1) } else { 0 };
    }
}

/// A run of failures is summed up at most this often.
pub const FAILURE_LOG_EVERY: Duration = Duration::from_secs(60);

/// Open and write failures on the device, kept out of waved.log's way (it
/// only rotates on restart and `/data` is small; a pulled cable fails a
/// write about once a second): the first failure is logged, then a count at
/// most every `FAILURE_LOG_EVERY`, then one line when a write works again.
#[derive(Default)]
struct FailureLog {
    /// Failures since the last write that worked.
    total: u64,
    /// Failures not in a log line yet.
    unlogged: u64,
    last_line: Option<Instant>,
}

impl FailureLog {
    /// One failure; the line to log, if any.
    fn failed(&mut self, now: Instant, what: &str) -> Option<String> {
        self.total += 1;
        match self.last_line {
            None => {
                self.last_line = Some(now);
                Some(format!("pclink: {what}"))
            }
            Some(t) if now.saturating_duration_since(t) >= FAILURE_LOG_EVERY => {
                self.last_line = Some(now);
                let count = std::mem::take(&mut self.unlogged) + 1;
                let secs = now.saturating_duration_since(t).as_secs();
                Some(format!("pclink: {count} more failures in {secs} s, latest: {what}"))
            }
            Some(_) => {
                self.unlogged += 1;
                None
            }
        }
    }

    /// A write went through; the line to log after a run of failures.
    fn succeeded(&mut self) -> Option<String> {
        if self.total == 0 {
            return None;
        }
        let line = format!("pclink: writing again after {} failures", self.total);
        *self = Self::default();
        Some(line)
    }
}

/// Reports waiting for the node (it failed or went away); beyond this the
/// oldest go.
pub const REPORTS_WAITING: usize = 64;

/// The writer loop: one 64-byte report per write to `device`. Returns when
/// `rx` disconnects.
pub fn run_writer(device: String, rx: Receiver<Report>) {
    run_writer_on(DevNode::writer(&device), &device, REOPEN_EVERY, rx, None)
}

/// `run_writer` on any node. Before each write it checks that the path
/// still names the node it opened (a gadget switch replaces it); after an
/// error it closes it and opens again no sooner than `every` later. Reports
/// that wait for the node meanwhile are kept (`REPORTS_WAITING`, oldest
/// first, none written more than `node::STALE_AFTER` late) and go out once
/// it opens. With `hold` (USB storage's, `node::Hold`) it looks at the hold
/// at least every `HOLD_CHECK`, idle or not: while raised the node is
/// closed (confirmed to the switch) and the reports wait the same way.
pub fn run_writer_on<N: Node>(
    node: N,
    device: &str,
    every: Duration,
    rx: Receiver<Report>,
    hold: Option<GadgetHoldUser>,
) {
    let follow_hold = hold.is_some();
    let mut node = Reopening::new(node, every).with_hold(hold);
    let mut pending: Pending<Report> = Pending::new(REPORTS_WAITING, STALE_AFTER);
    let mut failures = FailureLog::default();
    loop {
        let held = match node.hold() {
            Hold::Free => false,
            Hold::Began => {
                info!("pclink: USB is switching; {device} closed, replies wait until it is back");
                true
            }
            Hold::Held => true,
            Hold::Ended => {
                info!("pclink: the USB switch is over; writing to {device} again");
                false
            }
        };
        // While reports wait for the node, wake for its next try; with a
        // hold to follow, at least every HOLD_CHECK.
        let wait = if held {
            Some(HOLD_CHECK)
        } else if pending.is_empty() {
            follow_hold.then_some(HOLD_CHECK)
        } else {
            let wait = node.wait(Instant::now());
            Some(if follow_hold { wait.min(HOLD_CHECK) } else { wait })
        };
        let next = match wait {
            Some(wait) => rx.recv_timeout(wait),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match next {
            Ok(report) => {
                pending.push(Instant::now(), report);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        pending.drop_stale(now);
        while let Some(report) = pending.front() {
            let h = match node.get(now) {
                Ok((h, opened)) => {
                    if opened == Opened::Replaced {
                        info!("pclink: {device} is a new node (the gadget was switched); writing to it");
                    }
                    h
                }
                Err(Unavailable::Waiting | Unavailable::Held) => break,
                Err(Unavailable::OpenFailed(e)) => {
                    if let Some(line) = failures.failed(now, &format!("cannot open {device}: {e}")) {
                        warn!("{line}");
                    }
                    break;
                }
            };
            match h.write_report(report) {
                Ok(()) => {
                    pending.pop();
                    if let Some(line) = failures.succeeded() {
                        info!("{line}");
                    }
                }
                Err(e) => {
                    node.failed(now);
                    if let Some(line) = failures.failed(now, &format!("write to {device} failed: {e}")) {
                        warn!("{line}");
                    }
                    break;
                }
            }
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
        assert_eq!(run.commands.try_recv().unwrap(), Command::SetMicMute(true));
    }

    #[test]
    fn a_slow_writer_loses_nothing_of_a_large_ack() {
        let bus = StateBus::new();
        let (vendor_tx, vendor_rx) = mpsc::channel();
        let (write_tx, written) = mpsc::sync_channel(4);
        let link_bus = PcLinkBus::from_bus(&bus);
        let engine = std::thread::spawn(move || {
            run_engine(vendor_rx, link_bus, Box::new(DenyAllPins), Box::new(NoClock), None, write_tx)
        });
        let mut seq = 0;
        let p = crate::pclink::packet::segment_command(crate::pclink::packet::DEFAULT_TOKEN, 0x33, &[], &mut seq)[0];
        let frame = waved_mavlink::encode(waved_mavlink::PC_LINK, waved_mavlink::Header::default(), 1000, &p).unwrap();
        for (i, chunk) in frame.chunks(52).enumerate() {
            vendor_tx.send(crate::pclink::vendor::Packet::mavlink(i as u64 + 1, chunk).encode().to_vec()).unwrap();
        }
        // Read slowly through a 4-report channel until the whole 25 606-byte
        // ack has been reassembled (after it the link sends a few SYNCs).
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut parser = waved_mavlink::Parser::new(waved_mavlink::PC_LINK);
        let mut reassembler = crate::pclink::packet::Reassembler::default();
        let mut next_id = 1;
        let mut ack = None;
        while ack.is_none() && Instant::now() < deadline {
            let Ok(r) = written.recv_timeout(Duration::from_millis(100)) else { continue };
            std::thread::sleep(Duration::from_micros(200));
            let Some(crate::pclink::vendor::Incoming::Transport(p)) = crate::pclink::vendor::decode(&r) else { continue };
            if p.kind != crate::pclink::vendor::Kind::Mavlink {
                continue;
            }
            assert_eq!(p.send_id, next_id, "no chunk lost");
            next_id += 1;
            for b in p.payload {
                if let Some(m) = parser.feed(b) {
                    let (h, payload) = crate::pclink::packet::parse(&m.payload).unwrap();
                    ack = ack.or(reassembler.push(h, payload));
                }
            }
        }
        assert_eq!(ack.expect("the whole ack").data.len(), 0x6406);
        drop(vendor_tx);
        engine.join().unwrap();
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
    /// built on macOS). With no device the reader tries it once a second and
    /// the PC link waits for it; the threads outlive the test.
    #[test]
    fn the_main_rs_wiring_type_checks_without_a_device() {
        use crate::reader::{HidReader, HidReaderConfig};
        let bus = std::sync::Arc::new(StateBus::new());
        let hid_stats = crate::stats::HidStats::new();
        let device = "/nonexistent/hidg0".to_string();
        let (pclink_tx, pclink_rx) = std::sync::mpsc::channel();
        HidReader::spawn_with_vendor(
            HidReaderConfig { device: device.clone(), hold: Some(bus.hid_hold.clone()) },
            std::sync::Arc::clone(&bus.audio),
            std::sync::Arc::clone(&hid_stats),
            Some(pclink_tx),
        );
        spawn(
            PcLinkConfig { device, hold: Some(bus.hid_hold.clone()) },
            pclink_rx,
            PcLinkBus::from_bus(&bus),
            Box::new(DenyAllPins),
            Box::new(NoClock),
            None,
        );
        assert_eq!(bus.hid_hold.users(), 2, "the reader and the PC link's writer");
        std::thread::sleep(Duration::from_millis(50));
        // With no device they hold no file: both confirm a hold at once.
        bus.hid_hold.raise();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bus.hid_hold.all_closed() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(bus.hid_hold.all_closed(), "waiting for {:?}", bus.hid_hold.waiting_for());
    }

    #[test]
    fn a_writer_that_takes_nothing_backs_the_engine_off_to_the_tick() {
        // A blocked write (the host stopped reading) fills the channel: the
        // engine must not wake every millisecond to find it still full.
        let (tx, rx) = mpsc::sync_channel(1);
        let mut outbox = Outbox::default();
        assert_eq!(outbox.wait(), TICK, "nothing to hand over");
        outbox.extend(vec![[0u8; REPORT_LEN]; 3]);
        outbox.flush(&tx);
        assert_eq!(outbox.wait(), Duration::from_millis(1), "it took one: come back soon");
        let waits: Vec<u64> = (0..7)
            .map(|_| {
                outbox.flush(&tx);
                outbox.wait().as_millis() as u64
            })
            .collect();
        assert_eq!(waits, vec![2, 4, 8, 16, 20, 20, 20]);
        rx.recv().unwrap();
        outbox.flush(&tx);
        assert_eq!(outbox.wait(), Duration::from_millis(1), "it takes again");
        rx.recv().unwrap();
        outbox.flush(&tx);
        assert_eq!(outbox.wait(), TICK, "all handed over");
    }

    #[test]
    fn write_failures_are_logged_once_then_counted_once_a_minute() {
        // waved.log only rotates on restart and /data is small: a pulled
        // cable must not add a line every second.
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut log = FailureLog::default();
        assert_eq!(log.succeeded(), None, "nothing to recover from");
        assert_eq!(log.failed(at(0), "write to hidg0 failed: EIO").as_deref(), Some("pclink: write to hidg0 failed: EIO"));
        for s in 1..60 {
            assert_eq!(log.failed(at(s), "write to hidg0 failed: EIO"), None, "{s} s");
        }
        assert_eq!(
            log.failed(at(60), "cannot open hidg0: ENOENT").as_deref(),
            Some("pclink: 60 more failures in 60 s, latest: cannot open hidg0: ENOENT")
        );
        assert_eq!(log.failed(at(61), "write to hidg0 failed: EIO"), None);
        assert_eq!(log.succeeded().as_deref(), Some("pclink: writing again after 62 failures"));
        assert_eq!(log.succeeded(), None);
        assert!(log.failed(at(62), "write to hidg0 failed: EIO").is_some(), "a new run of failures shows at once");
    }

    /// Waits until the fake node of `generation` holds `n` reports.
    fn written(node: &crate::node::fake::FakeNode, generation: u32, n: usize) -> Vec<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while node.written_to(generation).len() < n && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        node.written_to(generation)
    }

    fn numbered(i: u8) -> Report {
        let mut r = [0u8; REPORT_LEN];
        r[0] = 0x03;
        r[2] = i;
        r
    }

    #[test]
    fn replies_wait_for_a_missing_node_bounded_and_go_out_in_order() {
        let node = crate::node::fake::FakeNode::default();
        let (tx, rx) = mpsc::sync_channel(WRITE_QUEUE);
        let n = node.clone();
        let writer = std::thread::spawn(move || run_writer_on(n, "hidg0", Duration::from_millis(50), rx, None));
        let extra = 5;
        for i in 0..(REPORTS_WAITING + extra) as u8 {
            tx.send(numbered(i)).unwrap();
        }
        std::thread::sleep(Duration::from_millis(120));
        let tries = node.opens();
        assert!((1..=4).contains(&tries), "{tries} opens in 120 ms with a 50 ms back-off");
        node.bind();
        let got = written(&node, 1, REPORTS_WAITING);
        let want: Vec<Vec<u8>> = (extra..REPORTS_WAITING + extra).map(|i| numbered(i as u8).to_vec()).collect();
        assert_eq!(got, want, "the newest REPORTS_WAITING, in order");
        drop(tx);
        writer.join().unwrap();
    }

    #[test]
    fn a_failed_write_is_retried_on_the_node_that_comes_back() {
        let node = crate::node::fake::FakeNode::present();
        node.state().write_errno = Some(libc::ESHUTDOWN);
        let (tx, rx) = mpsc::sync_channel(4);
        let n = node.clone();
        let writer = std::thread::spawn(move || run_writer_on(n, "hidg0", Duration::from_millis(20), rx, None));
        tx.send(numbered(1)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        node.unbind();
        node.bind();
        assert_eq!(written(&node, 1, 1), vec![numbered(1).to_vec()]);
        tx.send(numbered(2)).unwrap();
        assert_eq!(written(&node, 1, 2).len(), 2);
        drop(tx);
        writer.join().unwrap();
    }

    #[test]
    fn the_idle_writer_closes_the_node_for_a_hold_and_its_replies_wait_for_the_end() {
        use crate::node::HOLD_CHECK;
        let node = crate::node::fake::FakeNode::present();
        let hold = waved_state::GadgetHold::new();
        let (tx, rx) = mpsc::sync_channel(4);
        let n = node.clone();
        let user = hold.register("pclink");
        let writer =
            std::thread::spawn(move || run_writer_on(n, "hidg0", Duration::from_millis(20), rx, Some(user)));
        tx.send(numbered(1)).unwrap();
        assert_eq!(written(&node, 0, 1).len(), 1);
        assert_eq!(node.open_handles(), 1);

        // Idle in its wait for replies: the hold must still reach it.
        let raised = Instant::now();
        hold.raise();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !hold.all_closed() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(hold.all_closed(), "confirmed");
        assert!(raised.elapsed() <= HOLD_CHECK + Duration::from_millis(200), "{:?}", raised.elapsed());
        assert_eq!(node.open_handles(), 0);
        tx.send(numbered(2)).unwrap();
        std::thread::sleep(2 * HOLD_CHECK);
        assert_eq!(node.opens(), 1, "nothing opened while held");

        node.unbind();
        node.bind();
        hold.lower();
        assert_eq!(written(&node, 1, 1), vec![numbered(2).to_vec()], "the reply waited for the hold's end");
        drop(tx);
        writer.join().unwrap();
        assert_eq!(hold.users(), 0, "a writer that ends is not waited for");
    }

    #[test]
    fn the_writer_writes_to_the_new_node_after_a_gadget_switch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::File::create(&path).unwrap();
        let (tx, rx) = mpsc::sync_channel(4);
        let p = path.to_string_lossy().into_owned();
        std::thread::spawn(move || run_writer(p, rx));
        let holds = |n: usize| {
            let deadline = Instant::now() + Duration::from_secs(4);
            while std::fs::read(&path).map_or(0, |b| b.len()) < n && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            std::fs::read(&path).map_or(0, |b| b.len())
        };
        let mut r = [0u8; REPORT_LEN];
        r[..2].copy_from_slice(&[0x03, 0xEF]);
        tx.send(r).unwrap();
        assert_eq!(holds(REPORT_LEN), REPORT_LEN);

        // The switch: a new node at the same path.
        std::fs::remove_file(&path).unwrap();
        std::fs::File::create(&path).unwrap();
        tx.send(r).unwrap();
        assert_eq!(holds(REPORT_LEN), REPORT_LEN, "the reply goes to the new node");

        // A reply while the node is missing waits for the next one.
        std::fs::remove_file(&path).unwrap();
        tx.send(r).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::File::create(&path).unwrap();
        assert_eq!(holds(REPORT_LEN), REPORT_LEN, "the reply waited for the node");
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
