//! The vendor transport: send ids, resend, sync and reset
//! (docs/WAVE_CONTROLLER_PROTOCOL.md §2, stock `hid_protocol.c`).
//!
//! Pure: the caller passes the time in. Outgoing MAVLink frames are cut into
//! 52-byte chunks with send ids 1, 2, 3, …; received chunks come back out as
//! one byte stream in send-id order, with gaps asked for again.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use super::vendor::{Kind, Packet, CHUNK_MAX};

/// Send queue length; stock drops the oldest entry beyond it.
pub const QUEUE_MAX: usize = 400;
/// Sent chunks kept for RESEND.
pub const POOL_MAX: usize = 1200;
/// A RESEND of the same id is answered at most this often.
pub const RESEND_RESP_GAP_MS: u64 = 30;
/// Missing ids are asked for at most this often (per round)…
pub const RESEND_ROUND_MS: u64 = 100;
/// …at most this many per round…
pub const RESEND_PER_ROUND: usize = 10;
/// …each id again only after this long…
pub const RESEND_RETRY_MS: u64 = 61;
/// …and at most this many times before it is skipped.
pub const RESEND_MAX_TRIES: u8 = 21;
/// Silence before we SYNC our last sent id.
pub const SYNC_IDLE_MS: u64 = 300;
/// SYNCs sent without hearing from the host before we stop (waved; stock
/// syncs until confirmed, which with the app gone is forever).
pub const SYNC_MAX_TRIES: u8 = 10;
/// A jump this far ahead restarts the receive side.
pub const MAX_GAP: u64 = 400;
/// The first chunk with an id in `2..FIRST_GAP_LIMIT` marks the ids before it missing.
const FIRST_GAP_LIMIT: u64 = 20;

#[derive(Clone, Copy, Debug)]
struct Retry {
    tries: u8,
    last_ms: u64,
}

/// What one received packet produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Received {
    /// Stream bytes now deliverable in order.
    pub stream: Vec<u8>,
    /// The host sent RESET: restart the layers above.
    pub reset: bool,
}

#[derive(Default)]
pub struct Transport {
    last_send_id: u64,
    queue: VecDeque<Packet>,
    pool: VecDeque<Packet>,
    resend_resp_at: HashMap<u64, u64>,
    last_synced_id: u64,
    /// SYNCs sent since the host last sent anything.
    sync_tries: u8,
    last_tx_ms: Option<u64>,
    last_rx_ms: Option<u64>,

    rx_next: Option<u64>,
    rx_highest: u64,
    rx_buffer: BTreeMap<u64, Vec<u8>>,
    missing: BTreeMap<u64, Retry>,
    given_up: BTreeSet<u64>,
    last_round_ms: Option<u64>,
}

impl Transport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one MAVLink frame for sending.
    pub fn send(&mut self, frame: &[u8]) {
        for chunk in frame.chunks(CHUNK_MAX) {
            self.last_send_id += 1;
            self.queue.push_back(Packet::mavlink(self.last_send_id, chunk));
        }
        while self.queue.len() > QUEUE_MAX {
            self.queue.pop_front();
        }
    }

    /// Handle one received transport packet.
    pub fn receive(&mut self, packet: Packet, now_ms: u64) -> Received {
        self.last_rx_ms = Some(now_ms);
        self.sync_tries = 0;
        let mut out = Received::default();
        match packet.kind {
            Kind::Mavlink | Kind::ResendResp => out.stream = self.accept(packet.send_id, packet.payload),
            Kind::Resend => self.answer_resend(packet.send_id, now_ms),
            Kind::Reset => {
                self.reset_receive_side();
                self.queue.clear();
                self.last_synced_id = 0;
                self.queue.push_front(Packet::reset_resp());
                out.reset = true;
            }
            Kind::Sync => {
                self.expect_up_to(packet.send_id);
                self.queue.push_front(Packet::control(Kind::SyncResp, packet.send_id));
            }
            Kind::SyncResp => self.last_synced_id = packet.send_id,
            // Only the host sends RESET; a stray RESET_RESP means nothing to us.
            Kind::ResetResp => {}
        }
        out
    }

    /// Housekeeping: give up on ids asked for too often (their followers are
    /// returned), ask for missing ids, and SYNC after silence — at most
    /// `SYNC_MAX_TRIES` times until the host sends something again.
    pub fn poll(&mut self, now_ms: u64) -> Vec<u8> {
        let mut delivered = Vec::new();
        if !self.queue.is_empty() {
            return delivered;
        }
        if !self.missing.is_empty()
            && self.last_round_ms.is_none_or(|t| now_ms.saturating_sub(t) >= RESEND_ROUND_MS)
        {
            self.last_round_ms = Some(now_ms);
            delivered = self.resend_round(now_ms);
        }
        if self.queue.is_empty() && self.last_send_id != self.last_synced_id && self.sync_tries < SYNC_MAX_TRIES {
            let last_traffic = self.last_tx_ms.max(self.last_rx_ms);
            if last_traffic.is_some_and(|t| now_ms.saturating_sub(t) >= SYNC_IDLE_MS) {
                self.sync_tries += 1;
                self.queue.push_front(Packet::control(Kind::Sync, self.last_send_id));
            }
        }
        delivered
    }

    /// The next packet to write, if any.
    pub fn next_outgoing(&mut self, now_ms: u64) -> Option<Packet> {
        let packet = self.queue.pop_front()?;
        if packet.kind == Kind::Mavlink {
            self.pool.push_back(packet.clone());
            if self.pool.len() > POOL_MAX {
                self.pool.pop_front();
            }
        }
        self.last_tx_ms = Some(now_ms);
        Some(packet)
    }

    fn answer_resend(&mut self, id: u64, now_ms: u64) {
        // Still waiting in the queue: it goes out anyway.
        if self.queue.iter().any(|p| p.send_id == id && p.kind != Kind::Sync) {
            return;
        }
        let Some(sent) = self.pool.iter().find(|p| p.send_id == id) else {
            tracing::debug!("pclink: resend of {id}, never sent");
            return;
        };
        if self.resend_resp_at.get(&id).is_some_and(|&t| now_ms.saturating_sub(t) < RESEND_RESP_GAP_MS) {
            return;
        }
        if self.resend_resp_at.len() >= QUEUE_MAX {
            self.resend_resp_at.clear();
        }
        self.resend_resp_at.insert(id, now_ms);
        let mut again = sent.clone();
        again.kind = Kind::ResendResp;
        self.queue.push_front(again);
    }

    fn reset_receive_side(&mut self) {
        self.rx_next = None;
        self.rx_highest = 0;
        self.rx_buffer.clear();
        self.missing.clear();
        self.given_up.clear();
        self.last_round_ms = None;
    }

    /// Make sure every id up to `id` is either held, delivered or missing;
    /// returns the next id to deliver.
    fn expect_up_to(&mut self, id: u64) -> u64 {
        let next = match self.rx_next {
            Some(next) => {
                if id > self.rx_highest.saturating_add(MAX_GAP) {
                    self.reset_receive_side();
                    self.rx_next = Some(id);
                    self.rx_highest = id.saturating_sub(1);
                    id
                } else {
                    next
                }
            }
            None => {
                let start = if id > 1 && id < FIRST_GAP_LIMIT { 1 } else { id };
                self.rx_next = Some(start);
                self.rx_highest = start.saturating_sub(1);
                start
            }
        };
        for gap in (self.rx_highest + 1).max(next)..=id {
            if !self.rx_buffer.contains_key(&gap) {
                self.missing.entry(gap).or_insert(Retry { tries: 0, last_ms: 0 });
            }
        }
        self.rx_highest = self.rx_highest.max(id);
        next
    }

    fn accept(&mut self, id: u64, data: Vec<u8>) -> Vec<u8> {
        if self.rx_next.is_some_and(|next| id < next) || self.rx_buffer.contains_key(&id) {
            return Vec::new();
        }
        self.expect_up_to(id);
        self.missing.remove(&id);
        self.given_up.remove(&id);
        self.rx_buffer.insert(id, data);
        self.deliver()
    }

    fn deliver(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(next) = self.rx_next {
            if let Some(data) = self.rx_buffer.remove(&next) {
                out.extend(data);
            } else if self.given_up.remove(&next) {
                tracing::warn!("pclink: chunk {next} never came, skipped");
            } else {
                break;
            }
            self.rx_next = Some(next + 1);
        }
        out
    }

    fn resend_round(&mut self, now_ms: u64) -> Vec<u8> {
        let mut requests = Vec::new();
        let mut dropped = Vec::new();
        for (&id, retry) in self.missing.iter_mut() {
            if requests.len() >= RESEND_PER_ROUND {
                break;
            }
            if retry.tries == 0 || (retry.tries < RESEND_MAX_TRIES && now_ms.saturating_sub(retry.last_ms) >= RESEND_RETRY_MS) {
                retry.tries += 1;
                retry.last_ms = now_ms;
                requests.push(id);
            } else if retry.tries >= RESEND_MAX_TRIES {
                dropped.push(id);
            }
        }
        for id in requests {
            self.queue.push_back(Packet::control(Kind::Resend, id));
        }
        for id in dropped {
            self.missing.remove(&id);
            self.given_up.insert(id);
        }
        self.deliver()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn drain(t: &mut Transport, now: u64) -> Vec<Packet> {
        std::iter::from_fn(|| t.next_outgoing(now)).collect()
    }

    fn chunk(id: u64, data: &[u8]) -> Packet {
        Packet::mavlink(id, data)
    }

    fn kinds(p: &[Packet]) -> Vec<(Kind, u64)> {
        p.iter().map(|p| (p.kind, p.send_id)).collect()
    }

    // The device's ack to SET_AUDIO_MIC_MUTE as the reference model frames it.
    fn ack_frame() -> Vec<u8> {
        let mut f = hex("FD 80 00 00 00 00 00 E8 03 00 4A 49 48 47 01 25 2B 00 FF FF FF FF 01 00 00 00 00");
        f.resize(138, 0xFF);
        f.extend([0xA9, 0x8C]);
        f
    }

    #[test]
    fn a_frame_goes_out_in_52_byte_chunks_with_ids_from_1() {
        let mut t = Transport::new();
        t.send(&ack_frame());
        let out = drain(&mut t, 0);
        assert_eq!(kinds(&out), vec![(Kind::Mavlink, 1), (Kind::Mavlink, 2), (Kind::Mavlink, 3)]);
        assert_eq!(out.iter().map(|p| p.payload.len()).collect::<Vec<_>>(), vec![52, 52, 36]);
        let third = out[2].encode();
        assert_eq!(&third[..11], &hex("03 00 03 00 00 00 00 00 00 00 24")[..]);
        assert_eq!(&third[45..47], &[0xA9, 0x8C]);
        t.send(&[1, 2, 3]);
        assert_eq!(kinds(&drain(&mut t, 0)), vec![(Kind::Mavlink, 4)]);
    }

    #[test]
    fn a_resend_request_gets_the_chunk_back_at_most_every_30_ms() {
        let mut t = Transport::new();
        t.send(&ack_frame());
        let sent = drain(&mut t, 0);
        t.receive(Packet::control(Kind::Resend, 2), 10);
        let again = drain(&mut t, 10);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].kind, Kind::ResendResp);
        assert_eq!(again[0].send_id, 2);
        assert_eq!(again[0].payload, sent[1].payload);
        t.receive(Packet::control(Kind::Resend, 2), 20);
        assert!(drain(&mut t, 20).is_empty(), "too soon");
        t.receive(Packet::control(Kind::Resend, 2), 45);
        assert_eq!(kinds(&drain(&mut t, 45)), vec![(Kind::ResendResp, 2)]);
        t.receive(Packet::control(Kind::Resend, 99), 50);
        assert!(drain(&mut t, 50).is_empty(), "never sent");
    }

    #[test]
    fn chunks_in_order_come_out_as_one_stream() {
        let mut t = Transport::new();
        let frame = ack_frame();
        let mut stream = Vec::new();
        for (i, c) in frame.chunks(CHUNK_MAX).enumerate() {
            stream.extend(t.receive(chunk(i as u64 + 1, c), 0).stream);
        }
        assert_eq!(stream, frame);
    }

    #[test]
    fn a_gap_is_asked_for_and_filled() {
        let mut t = Transport::new();
        assert_eq!(t.receive(chunk(1, b"a"), 0).stream, b"a");
        assert!(t.receive(chunk(3, b"c"), 1).stream.is_empty(), "waits for 2");
        assert!(t.poll(5).is_empty());
        assert_eq!(kinds(&drain(&mut t, 5)), vec![(Kind::Resend, 2)]);
        assert_eq!(t.receive(Packet { kind: Kind::ResendResp, send_id: 2, payload: b"b".to_vec() }, 20).stream, b"bc");
        t.poll(200);
        assert!(drain(&mut t, 200).iter().all(|p| p.kind != Kind::Resend), "nothing missing any more");
    }

    #[test]
    fn a_missing_id_is_asked_for_21_times_then_skipped() {
        let mut t = Transport::new();
        t.receive(chunk(1, b"a"), 0);
        t.receive(chunk(3, b"c"), 0);
        let mut asked = 0;
        let mut delivered = Vec::new();
        let mut now = 0;
        while now < 10_000 && delivered.is_empty() {
            delivered = t.poll(now);
            asked += drain(&mut t, now).iter().filter(|p| p.kind == Kind::Resend).count();
            now += 10;
        }
        assert_eq!(asked, usize::from(RESEND_MAX_TRIES));
        assert_eq!(delivered, b"c");
        // Retries are spaced: 21 requests need 20 gaps of at least 100 ms.
        assert!(now >= 2_000, "gave up at {now} ms");
    }

    #[test]
    fn at_most_ten_ids_are_asked_for_per_round() {
        let mut t = Transport::new();
        t.receive(chunk(1, b"a"), 0);
        t.receive(chunk(30, b"z"), 0);
        t.poll(0);
        assert_eq!(drain(&mut t, 0).len(), RESEND_PER_ROUND);
        t.poll(50);
        assert!(drain(&mut t, 50).is_empty(), "rounds are 100 ms apart");
        t.poll(100);
        let second: Vec<u64> = drain(&mut t, 100).iter().map(|p| p.send_id).collect();
        assert_eq!(second.first(), Some(&2), "the oldest are asked again first");
    }

    #[test]
    fn the_first_chunk_below_20_marks_the_ids_before_it_missing() {
        let mut t = Transport::new();
        assert!(t.receive(chunk(4, b"d"), 0).stream.is_empty());
        t.poll(0);
        assert_eq!(drain(&mut t, 0).iter().map(|p| p.send_id).collect::<Vec<_>>(), vec![1, 2, 3]);

        let mut t = Transport::new();
        assert_eq!(t.receive(chunk(25, b"y"), 0).stream, b"y", "a later start is taken as is");
    }

    #[test]
    fn duplicates_and_old_ids_are_dropped() {
        let mut t = Transport::new();
        assert_eq!(t.receive(chunk(1, b"a"), 0).stream, b"a");
        assert!(t.receive(chunk(1, b"a"), 0).stream.is_empty());
        t.receive(chunk(3, b"c"), 0);
        assert!(t.receive(chunk(3, b"c"), 0).stream.is_empty());
        assert_eq!(t.receive(chunk(2, b"b"), 0).stream, b"bc");
    }

    #[test]
    fn a_jump_of_more_than_400_restarts_the_receive_side() {
        let mut t = Transport::new();
        t.receive(chunk(1, b"a"), 0);
        assert_eq!(t.receive(chunk(500, b"x"), 0).stream, b"x");
        t.poll(0);
        assert!(drain(&mut t, 0).is_empty(), "nothing in between is asked for");
    }

    #[test]
    fn a_sync_is_answered_and_marks_what_we_lack() {
        let mut t = Transport::new();
        t.receive(chunk(1, b"a"), 0);
        t.receive(Packet::control(Kind::Sync, 1), 1);
        assert_eq!(kinds(&drain(&mut t, 1)), vec![(Kind::SyncResp, 1)]);
        t.receive(Packet::control(Kind::Sync, 3), 2);
        assert_eq!(kinds(&drain(&mut t, 2)), vec![(Kind::SyncResp, 3)]);
        t.poll(3);
        assert_eq!(kinds(&drain(&mut t, 3)), vec![(Kind::Resend, 2), (Kind::Resend, 3)]);
    }

    #[test]
    fn we_sync_our_last_id_after_300_ms_of_silence_until_confirmed() {
        let mut t = Transport::new();
        t.poll(1_000);
        assert!(drain(&mut t, 1_000).is_empty(), "no traffic yet, no sync");
        t.send(&ack_frame());
        drain(&mut t, 0);
        t.poll(250);
        assert!(drain(&mut t, 250).is_empty());
        t.poll(300);
        assert_eq!(kinds(&drain(&mut t, 300)), vec![(Kind::Sync, 3)]);
        t.poll(400);
        assert!(drain(&mut t, 400).is_empty(), "300 ms after the sync itself");
        t.poll(600);
        assert_eq!(kinds(&drain(&mut t, 600)), vec![(Kind::Sync, 3)]);
        t.receive(Packet::control(Kind::SyncResp, 3), 610);
        t.poll(2_000);
        assert!(drain(&mut t, 2_000).is_empty(), "confirmed");
    }

    #[test]
    fn syncing_stops_when_the_host_stays_silent_and_resumes_when_it_speaks() {
        let mut t = Transport::new();
        t.send(&ack_frame());
        drain(&mut t, 0);
        let syncs_until = |t: &mut Transport, from: u64, to: u64| {
            (from..to).step_by(20).map(|now| {
                t.poll(now);
                drain(t, now).iter().filter(|p| p.kind == Kind::Sync).count()
            }).sum::<usize>()
        };
        assert_eq!(syncs_until(&mut t, 20, 600_000), usize::from(SYNC_MAX_TRIES), "ten minutes of silence");
        // The host is back: one chunk from it, and our sync starts over.
        t.receive(chunk(1, b"a"), 600_000);
        assert_eq!(syncs_until(&mut t, 600_020, 600_320), 1);
        assert_eq!(syncs_until(&mut t, 600_320, 700_000), usize::from(SYNC_MAX_TRIES) - 1);
    }

    #[test]
    fn reset_clears_both_sides_answers_first_and_keeps_the_send_ids() {
        let mut t = Transport::new();
        t.send(&ack_frame());
        drain(&mut t, 0);
        t.receive(chunk(1, b"a"), 0);
        t.receive(chunk(3, b"c"), 0);
        t.send(&[9; 10]);
        let r = t.receive(Packet { kind: Kind::Reset, send_id: 0, payload: vec![1] }, 5);
        assert!(r.reset);
        let out = drain(&mut t, 5);
        assert_eq!(out, vec![Packet::reset_resp()], "pending sends are dropped");
        assert_eq!(t.receive(chunk(1, b"n"), 6).stream, b"n", "receive side starts over");
        t.poll(6);
        assert!(drain(&mut t, 6).is_empty(), "old gaps are forgotten");
        t.send(&[1]);
        assert_eq!(kinds(&drain(&mut t, 7)), vec![(Kind::Mavlink, 5)], "send ids go on");
    }

    #[test]
    fn the_send_queue_keeps_the_newest_400() {
        let mut t = Transport::new();
        t.send(&vec![7; CHUNK_MAX * (QUEUE_MAX + 5)]);
        let out = drain(&mut t, 0);
        assert_eq!(out.len(), QUEUE_MAX);
        assert_eq!(out[0].send_id, 6);
    }
}
