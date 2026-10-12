//! The PC-link engine: vendor reports in, vendor reports and bus commands
//! out. Pure (time comes in through the [`Context`]); `service` runs it.

use waved_mavlink::{Header as MavHeader, Parser, PC_LINK};
use waved_state::Command;

use super::handler::{Context, Handler};
use super::packet::{self, Reassembler};
use super::status::{short_status, worth_pushing};
use super::transport::Transport;
use super::vendor::{self, Incoming, Report, SHORT_STATUS_LEN};

/// MAVLink message carrying PC-link packets.
pub const PC_CMD_PKG: u32 = 1000;
/// Component id of the out-of-band (file-transfer resume) channel.
const COMPID_OB: u8 = 1;
/// Heartbeat pushes are checked this often (stock: 100 ms)…
pub const HEARTBEAT_POLL_MS: u64 = 100;
/// …and sent at least this often while the app is there (stock: 1 s)…
pub const HEARTBEAT_PUSH_MS: u64 = 1_000;
/// …until no heartbeat request came for this long (waved; stock: until unplugged).
pub const HOST_SILENT_MS: u64 = 30_000;

/// What one call produced.
#[derive(Debug, Default, PartialEq)]
pub struct Output {
    /// Reports to write to `/dev/hidg0`, in order.
    pub reports: Vec<Report>,
    pub commands: Vec<Command>,
    /// Show the on-device PIN prompt.
    pub prompt_pin: bool,
}

#[derive(Default)]
struct Heartbeat {
    host_seen_ms: Option<u64>,
    last_push_ms: u64,
    last_check_ms: u64,
    last_status: Option<[u8; SHORT_STATUS_LEN]>,
}

pub struct PcLink {
    transport: Transport,
    parser: Parser,
    tx_seq: u8,
    reassembler: Reassembler,
    handler: Handler,
    heartbeat: Heartbeat,
}

impl Default for PcLink {
    fn default() -> Self {
        Self {
            transport: Transport::new(),
            parser: Parser::new(PC_LINK),
            tx_seq: 0,
            reassembler: Reassembler::default(),
            handler: Handler::new(),
            heartbeat: Heartbeat::default(),
        }
    }
}

impl PcLink {
    pub fn new() -> Self {
        Self::default()
    }

    /// The on-device PIN prompt closed.
    pub fn pin_prompt_finished(&mut self, verified: bool) {
        self.handler.pin_prompt_finished(verified);
    }

    /// The app has polled the heartbeat recently.
    pub fn host_present(&self, now_ms: u64) -> bool {
        self.heartbeat.host_seen_ms.is_some_and(|t| now_ms.saturating_sub(t) < HOST_SILENT_MS)
    }

    /// One report read from `/dev/hidg0` (any report id; others are ignored).
    pub fn on_report(&mut self, report: &[u8], ctx: &mut Context) -> Output {
        let mut out = Output::default();
        match vendor::decode(report) {
            None => return out,
            Some(Incoming::Ignored(kind)) => {
                tracing::debug!("pclink: vendor message {kind:#04x} ignored");
                return out;
            }
            Some(Incoming::HeartbeatRequest) => {
                self.heartbeat.host_seen_ms = Some(ctx.now_ms);
                self.push_heartbeat(ctx, &mut out);
            }
            Some(Incoming::Transport(packet)) => {
                let received = self.transport.receive(packet, ctx.now_ms);
                if received.reset {
                    tracing::info!("pclink: the app reset the link");
                    self.parser.reset();
                    self.tx_seq = 0;
                    self.reassembler.reset();
                }
                self.feed(&received.stream, ctx, &mut out);
            }
        }
        self.drain(ctx.now_ms, &mut out);
        out
    }

    /// Call every few tens of milliseconds: resend requests, sync, heartbeat pushes.
    pub fn poll(&mut self, ctx: &mut Context) -> Output {
        let mut out = Output::default();
        let delivered = self.transport.poll(ctx.now_ms);
        self.feed(&delivered, ctx, &mut out);
        if self.host_present(ctx.now_ms)
            && ctx.now_ms.saturating_sub(self.heartbeat.last_check_ms) >= HEARTBEAT_POLL_MS
        {
            self.heartbeat.last_check_ms = ctx.now_ms;
            let status = short_status(ctx.snapshot);
            let changed = self.heartbeat.last_status.is_none_or(|old| worth_pushing(&old, &status));
            if changed || ctx.now_ms.saturating_sub(self.heartbeat.last_push_ms) >= HEARTBEAT_PUSH_MS {
                self.push_heartbeat(ctx, &mut out);
            }
        }
        self.drain(ctx.now_ms, &mut out);
        out
    }

    fn push_heartbeat(&mut self, ctx: &Context, out: &mut Output) {
        let status = short_status(ctx.snapshot);
        out.reports.push(vendor::heartbeat_response(&status));
        self.heartbeat.last_status = Some(status);
        self.heartbeat.last_push_ms = ctx.now_ms;
        self.heartbeat.last_check_ms = ctx.now_ms;
    }

    fn drain(&mut self, now_ms: u64, out: &mut Output) {
        while let Some(packet) = self.transport.next_outgoing(now_ms) {
            out.reports.push(packet.encode());
        }
    }

    /// Stream bytes → MAVLink messages → commands → acks queued on the transport.
    fn feed(&mut self, stream: &[u8], ctx: &mut Context, out: &mut Output) {
        for &byte in stream {
            let Some(message) = self.parser.feed(byte) else { continue };
            if message.msgid != PC_CMD_PKG {
                // HEARTBEAT_CONTROL: the stock device side ignores it too.
                continue;
            }
            if message.compid == COMPID_OB {
                let cmd = u16::from_le_bytes([message.payload[6], message.payload[7]]);
                if let Some(ack) = self.handler.handle_ob(cmd) {
                    self.send_frame(COMPID_OB, &packet::ob_ack(&ack));
                }
                continue;
            }
            let Some((header, payload)) = packet::parse(&message.payload) else { continue };
            if header.is_ack() {
                continue;
            }
            let Some(command) = self.reassembler.push(header, payload) else { continue };
            let reply = self.handler.handle(&command, ctx);
            out.commands.extend(reply.commands);
            out.prompt_pin |= reply.prompt_pin;
            if let Some(ack) = reply.ack {
                for p in packet::segment_ack(reply.token, header.cmd, &ack) {
                    self.send_frame(0, &p);
                }
            }
        }
    }

    fn send_frame(&mut self, compid: u8, payload: &[u8]) {
        let header = MavHeader { seq: self.tx_seq, sysid: 0, compid };
        self.tx_seq = self.tx_seq.wrapping_add(1);
        match waved_mavlink::encode(PC_LINK, header, PC_CMD_PKG, payload) {
            Some(frame) => self.transport.send(&frame),
            None => tracing::error!("pclink: {} bytes do not fit one packet", payload.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pclink::cmd;
    use crate::pclink::handler::{NoClock, PinOutcome, PinVerifier};
    use crate::pclink::packet::{segment_command, DEFAULT_TOKEN};
    use crate::pclink::snapshot::Snapshot;
    use crate::pclink::status;
    use crate::pclink::vendor::{Kind, Packet, CHUNK_MAX, REPORT_LEN};

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    struct Accept;
    impl PinVerifier for Accept {
        fn check(&mut self, _: &str, _: u64) -> PinOutcome {
            PinOutcome::Accepted
        }
    }

    struct Rig {
        link: PcLink,
        snap: Snapshot,
        now: u64,
    }

    impl Rig {
        fn new() -> Self {
            let snap = Snapshot {
                speaker_volume: 40,
                mic_volume: 80,
                mic_mode: 2,
                time_zone: "Europe/Brussels".into(),
                battery_level: 64,
                ..Snapshot::default()
            };
            Self { link: PcLink::new(), snap, now: 1_000 }
        }

        fn report(&mut self, r: &[u8]) -> Output {
            let (mut pins, mut clock) = (Accept, NoClock);
            let mut ctx = Context { snapshot: &self.snap, now_ms: self.now, pins: &mut pins, host: &mut clock, prompt_available: false };
            self.link.on_report(r, &mut ctx)
        }

        fn poll(&mut self) -> Output {
            let (mut pins, mut clock) = (Accept, NoClock);
            let mut ctx = Context { snapshot: &self.snap, now_ms: self.now, pins: &mut pins, host: &mut clock, prompt_available: false };
            self.link.poll(&mut ctx)
        }
    }

    /// A host model built from the same layers, run the other way round.
    struct Host {
        send_id: u64,
        mav_seq: u8,
        pkt_seq: u32,
        rx: Transport,
        parser: Parser,
        reassembler: Reassembler,
    }

    impl Host {
        fn new() -> Self {
            Self { send_id: 0, mav_seq: 0, pkt_seq: 0, rx: Transport::new(), parser: Parser::new(PC_LINK), reassembler: Reassembler::default() }
        }

        fn command(&mut self, cmd: u16, data: &[u8], compid: u8) -> Vec<Report> {
            let mut reports = Vec::new();
            for p in segment_command(DEFAULT_TOKEN, cmd, data, &mut self.pkt_seq) {
                let frame = waved_mavlink::encode(PC_LINK, MavHeader { seq: self.mav_seq, sysid: 0, compid }, PC_CMD_PKG, &p).unwrap();
                self.mav_seq = self.mav_seq.wrapping_add(1);
                for chunk in frame.chunks(CHUNK_MAX) {
                    self.send_id += 1;
                    reports.push(Packet::mavlink(self.send_id, chunk).encode());
                }
            }
            reports
        }

        /// Decode device reports into (compid, cmd, ack bytes).
        fn acks(&mut self, reports: &[Report]) -> Vec<(u8, u16, Vec<u8>)> {
            let mut out = Vec::new();
            for r in reports {
                let Some(Incoming::Transport(p)) = vendor::decode(r) else { continue };
                for b in self.rx.receive(p, 0).stream {
                    let Some(m) = self.parser.feed(b) else { continue };
                    if m.compid == COMPID_OB {
                        let cmd = u16::from_le_bytes([m.payload[6], m.payload[7]]);
                        out.push((m.compid, cmd, m.payload[16..].to_vec()));
                        continue;
                    }
                    let (h, payload) = packet::parse(&m.payload).unwrap();
                    assert!(h.is_ack());
                    if let Some(msg) = self.reassembler.push(h, payload) {
                        out.push((m.compid, msg.header.cmd, msg.data));
                    }
                }
            }
            out
        }
    }

    // Reference model (stock 1.2.2.6 semantics): the host's SET_AUDIO_MIC_MUTE
    // (pkt seq 5, MAVLink seq 9) and the device's ack (seq 0, send ids 1–3).
    const HOST_MIC_MUTE: [&str; 3] = [
        "03 00 01 00 00 00 00 00 00 00 34 FD 80 00 00 09 00 00 E8 03 00 4A 49 48 47 01 24 2B 00 05 00 00 00 01 00 00 00 01 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF 00",
        "03 00 02 00 00 00 00 00 00 00 34 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF 00",
        "03 00 03 00 00 00 00 00 00 00 24 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF 39 D3 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    ];
    const DEVICE_ACK: [&str; 3] = [
        "03 00 01 00 00 00 00 00 00 00 34 FD 80 00 00 00 00 00 E8 03 00 4A 49 48 47 01 25 2B 00 FF FF FF FF 01 00 00 00 00 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF 00",
        "03 00 02 00 00 00 00 00 00 00 34 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF 00",
        "03 00 03 00 00 00 00 00 00 00 24 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF A9 8C 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    ];

    #[test]
    fn a_stock_mic_mute_command_gets_the_stock_ack_byte_for_byte() {
        let mut rig = Rig::new();
        let mut reports = Vec::new();
        let mut commands = Vec::new();
        for r in HOST_MIC_MUTE {
            let out = rig.report(&hex(r));
            reports.extend(out.reports);
            commands.extend(out.commands);
        }
        assert_eq!(commands, vec![Command::ToggleMicMute]);
        let expected: Vec<Vec<u8>> = DEVICE_ACK.iter().map(|r| hex(r)).collect();
        assert_eq!(reports.iter().map(|r| r.to_vec()).collect::<Vec<_>>(), expected);
    }

    #[test]
    fn a_heartbeat_request_is_answered_at_once_with_the_short_status() {
        let mut rig = Rig::new();
        let mut req = [0u8; REPORT_LEN];
        req[..2].copy_from_slice(&[0x03, 0xEC]);
        let out = rig.report(&req);
        assert_eq!(out.reports, vec![vendor::heartbeat_response(&status::short_status(&rig.snap))]);
        assert!(rig.link.host_present(rig.now));
    }

    #[test]
    fn heartbeats_are_pushed_on_change_and_every_second_until_the_app_goes_quiet() {
        let mut rig = Rig::new();
        let mut req = [0u8; REPORT_LEN];
        req[..2].copy_from_slice(&[0x03, 0xED]);
        rig.report(&req);
        rig.now += 100;
        assert!(rig.poll().reports.is_empty(), "nothing changed");
        rig.snap.speaker_volume = 41;
        rig.now += 50;
        assert!(rig.poll().reports.is_empty(), "checked every 100 ms");
        rig.now += 50;
        assert_eq!(rig.poll().reports.len(), 1, "changed");
        rig.now += 1_000;
        assert_eq!(rig.poll().reports.len(), 1, "a second without a push");
        rig.now += HOST_SILENT_MS;
        assert!(rig.poll().reports.is_empty(), "the app stopped asking");
        assert!(!rig.link.host_present(rig.now));
    }

    #[test]
    fn no_heartbeat_is_pushed_before_the_app_asks() {
        let mut rig = Rig::new();
        rig.now += 5_000;
        assert!(rig.poll().reports.is_empty());
    }

    #[test]
    fn a_two_packet_request_and_a_69_packet_ack_cross_the_whole_stack() {
        let mut rig = Rig::new();
        let mut host = Host::new();
        let mut out = Vec::new();
        for r in host.command(cmd::GET_SYS_COUNTRY_TIMEZONE_LIST, &[0; 128], 0) {
            out.extend(rig.report(&r).reports);
        }
        let acks = host.acks(&out);
        assert_eq!(acks.len(), 1);
        let (compid, id, data) = &acks[0];
        assert_eq!((*compid, *id, data.len()), (0, cmd::GET_SYS_COUNTRY_TIMEZONE_LIST, 0x1E02));
        assert_eq!(&data[..2], &[1, 0]);
        assert_eq!(&data[2..17], b"Europe/Brussels");
    }

    #[test]
    fn commands_arriving_out_of_order_are_answered_once_complete() {
        let mut rig = Rig::new();
        let mut host = Host::new();
        let reports = host.command(cmd::GET_DEV_STATUS, &[], 0);
        assert_eq!(reports.len(), 3);
        assert!(rig.report(&reports[0]).reports.is_empty());
        assert!(rig.report(&reports[2]).reports.is_empty(), "waits for the middle chunk");
        let out = rig.report(&reports[1]).reports;
        let acks = host.acks(&out);
        assert_eq!(acks[0].2, status::dev_status(&rig.snap).to_vec());
    }

    #[test]
    fn a_reset_is_answered_and_restarts_the_mavlink_sequence() {
        let mut rig = Rig::new();
        let mut host = Host::new();
        for r in host.command(cmd::SET_SYS_REBOOT, &[], 0) {
            rig.report(&r);
        }
        let reset = Packet { kind: Kind::Reset, send_id: 0, payload: vec![1] }.encode();
        let out = rig.report(&reset);
        assert_eq!(out.reports, vec![Packet::reset_resp().encode()]);
        let mut host = Host::new();
        let mut out = Vec::new();
        for r in host.command(cmd::SET_SYS_REBOOT, &[], 0) {
            out.extend(rig.report(&r).reports);
        }
        let Some(Incoming::Transport(first)) = vendor::decode(&out[0]) else { panic!() };
        assert_eq!(first.payload[4], 0, "MAVLink seq starts over");
        assert_eq!(first.send_id, 4, "send ids go on");
    }

    #[test]
    fn out_of_band_commands_answer_on_compid_1_with_a_zero_header() {
        let mut rig = Rig::new();
        let mut host = Host::new();
        let mut out = Vec::new();
        for r in host.command(cmd::OB_LAST_PKT_SEQ, &[], COMPID_OB) {
            out.extend(rig.report(&r).reports);
        }
        let acks = host.acks(&out);
        assert_eq!(acks.len(), 1);
        let (compid, id, data) = &acks[0];
        assert_eq!((*compid, *id), (COMPID_OB, 0), "stock's OB header is all zero");
        assert!(data.iter().all(|&b| b == 0), "last_pkt_seq 0");
        let mut out = Vec::new();
        for r in host.command(cmd::OB_BACKUP_OFFSET, &[0; 8], COMPID_OB) {
            out.extend(rig.report(&r).reports);
        }
        assert_eq!(host.acks(&out)[0].2[0], 1, "offset refused");
    }

    #[test]
    fn other_reports_and_vendor_types_produce_nothing() {
        let mut rig = Rig::new();
        assert_eq!(rig.report(&[0x05, 0x01]), Output::default());
        let mut cascade = [0u8; REPORT_LEN];
        cascade[..3].copy_from_slice(&[0x03, 0xA1, 0x01]);
        assert_eq!(rig.report(&cascade), Output::default());
    }

    #[test]
    fn a_stale_token_gets_no_ack() {
        let mut rig = Rig::new();
        let mut pkt_seq = 0;
        let p = &segment_command(0xDEAD, cmd::GET_DEV_STATUS, &[], &mut pkt_seq)[0];
        let frame = waved_mavlink::encode(PC_LINK, MavHeader::default(), PC_CMD_PKG, p).unwrap();
        let mut out = Vec::new();
        for (i, c) in frame.chunks(CHUNK_MAX).enumerate() {
            out.extend(rig.report(&Packet::mavlink(i as u64 + 1, c).encode()).reports);
        }
        assert!(out.is_empty());
    }
}
