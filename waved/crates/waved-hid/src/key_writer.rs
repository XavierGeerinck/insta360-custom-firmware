//! The consumer / telephony key writer behind `HidService`: each key is a
//! press and a release report to `/dev/hidg0`, through a [`Reopening`]
//! handle, so it never stops for good: a failed open or write closes the
//! handle and the keys wait (bounded, `node::Pending`) for the node to come
//! back, tried once a second (`docs/USB_STORAGE.md`).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::events::HidEvent;
use crate::node::{Handle, Node, Opened, Outage, Pending, Reopening, Unavailable, STALE_AFTER};
use crate::report;
use crate::stats::HidStats;

/// Keys that wait for the node; beyond this the oldest go. A full volume
/// drag is 16 steps (`watch::spawn_volume_watch`).
pub const KEYS_WAITING: usize = 32;

pub struct KeyWriter<N: Node> {
    node: Reopening<N>,
    device: String,
    gap: Duration,
    stats: Arc<HidStats>,
    pending: Pending<HidEvent>,
    /// The key whose press went out and whose release has not, since when.
    half: Option<(Instant, HidEvent)>,
    outage: Outage,
}

impl<N: Node> KeyWriter<N> {
    /// `gap`: between a key's press and its release. `every`: the back-off
    /// after a failure (`node::REOPEN_EVERY`).
    pub fn new(node: N, device: &str, gap: Duration, stats: Arc<HidStats>, every: Duration) -> Self {
        Self {
            node: Reopening::new(node, every),
            device: device.to_owned(),
            gap,
            stats,
            pending: Pending::new(KEYS_WAITING, STALE_AFTER),
            half: None,
            outage: Outage::default(),
        }
    }

    /// Open the node at start, so the stats say whether it is there.
    pub fn open_at_start(&mut self) {
        match self.node.get(Instant::now()) {
            Ok(_) => self.stats.device_present.store(true, Ordering::Relaxed),
            Err(Unavailable::OpenFailed(e)) => self.failed(format!("cannot open {}: {e}", self.device)),
            Err(Unavailable::Waiting) => {}
        }
    }

    /// Queue a key; `flush` writes it.
    pub fn push(&mut self, ev: HidEvent) {
        let dropped = self.pending.push(Instant::now(), ev);
        if dropped > 0 {
            self.stats.glue_drops.fetch_add(dropped as u64, Ordering::Relaxed);
        }
    }

    /// When to try the node again, while keys wait for it.
    pub fn retry_at(&self) -> Option<Instant> {
        if self.half.is_none() && self.pending.is_empty() {
            return None;
        }
        Some(self.node.retry_at().unwrap_or_else(Instant::now))
    }

    /// Write the waiting keys in order, until the node fails or is not due.
    pub async fn flush(&mut self) {
        loop {
            let now = Instant::now();
            if self.half.is_some_and(|(t, _)| now.saturating_duration_since(t) > STALE_AFTER) {
                self.half = None;
                self.stats.glue_drops.fetch_add(1, Ordering::Relaxed);
            }
            let stale = self.pending.drop_stale(now);
            if stale > 0 {
                self.stats.glue_drops.fetch_add(stale as u64, Ordering::Relaxed);
            }
            // A release that failed goes first: the press went out.
            let (ev, release) = match (self.half, self.pending.front()) {
                (Some((_, ev)), _) => (ev, true),
                (None, Some(&ev)) => (ev, false),
                (None, None) => return,
            };
            let (press_report, release_report) = report::event_to_press_release(ev);
            let h = match self.node.get(now) {
                Ok((h, opened)) => {
                    if opened == Opened::Replaced {
                        info!("hid: {} is a new node (the gadget was switched); writing to it", self.device);
                    }
                    h
                }
                Err(Unavailable::Waiting) => return,
                Err(Unavailable::OpenFailed(e)) => {
                    self.stats.write_errors.fetch_add(1, Ordering::Relaxed);
                    self.failed(format!("cannot open {}: {e}", self.device));
                    return;
                }
            };
            let written = h.write_report(if release { &release_report } else { &press_report });
            if let Err(e) = written {
                self.node.failed(now);
                self.stats.write_errors.fetch_add(1, Ordering::Relaxed);
                let what = if release { "release" } else { "press" };
                self.failed(format!("write {what} to {} failed: {e}", self.device));
                return;
            }
            self.worked();
            if release {
                self.half = None;
                count(ev, &self.stats);
            } else {
                self.pending.pop();
                self.half = Some((now, ev));
                tokio::time::sleep(self.gap).await;
            }
        }
    }

    fn failed(&mut self, what: String) {
        self.stats.device_present.store(false, Ordering::Relaxed);
        if self.outage.failed() {
            warn!("hid: {what}; trying again every second while keys wait, quietly until it works");
        } else {
            debug!("hid: {what}");
        }
    }

    fn worked(&mut self) {
        self.stats.device_present.store(true, Ordering::Relaxed);
        if let Some(n) = self.outage.ended() {
            info!("hid: writing to {} again after {n} failures", self.device);
        }
    }
}

fn count(ev: HidEvent, stats: &HidStats) {
    let counter = match ev {
        HidEvent::ConsumerVolumeStep { .. } => &stats.events_volume_step,
        HidEvent::ConsumerMute => &stats.events_mute,
        HidEvent::ConsumerPlayPause => &stats.events_play_pause,
        HidEvent::ConsumerNext => &stats.events_next,
        HidEvent::ConsumerPrev => &stats.events_prev,
        HidEvent::TelephonyPhoneMute => &stats.events_phone_mute,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::fake::FakeNode;
    use crate::node::REOPEN_EVERY;

    const LOUDER: HidEvent = HidEvent::ConsumerVolumeStep { up: true };
    const QUIETER: HidEvent = HidEvent::ConsumerVolumeStep { up: false };

    fn writer(node: &FakeNode, every: Duration) -> (KeyWriter<FakeNode>, Arc<HidStats>) {
        let stats = HidStats::new();
        (KeyWriter::new(node.clone(), "/dev/hidg0", Duration::ZERO, Arc::clone(&stats), every), stats)
    }

    fn reports(evs: &[HidEvent]) -> Vec<Vec<u8>> {
        evs.iter()
            .flat_map(|&ev| {
                let (p, r) = report::event_to_press_release(ev);
                [p.to_vec(), r.to_vec()]
            })
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_write_keeps_the_key_and_writes_it_to_the_node_that_comes_back() {
        let node = FakeNode::present();
        let (mut w, stats) = writer(&node, Duration::from_millis(30));
        w.open_at_start();
        w.push(LOUDER);
        w.flush().await;
        assert_eq!(node.written_to(0), reports(&[LOUDER]));

        // The old node fails its writes (ESHUTDOWN), as 5.4 f_hid does.
        node.state().write_errno = Some(libc::ESHUTDOWN);
        w.push(QUIETER);
        w.flush().await;
        assert!(!stats.device_present.load(Ordering::Relaxed));
        assert!(w.retry_at().is_some(), "the key waits");
        assert_eq!(stats.write_errors.load(Ordering::Relaxed), 1);
        node.unbind();
        node.bind();
        tokio::time::sleep(Duration::from_millis(40)).await;
        w.flush().await;
        assert_eq!(node.written_to(1), reports(&[QUIETER]));
        assert_eq!(w.retry_at(), None);
        assert!(stats.device_present.load(Ordering::Relaxed));
        assert_eq!(stats.events_volume_step.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_release_that_failed_goes_out_first_on_the_new_node() {
        let node = FakeNode::present();
        let (mut w, stats) = writer(&node, Duration::ZERO);
        // The press goes out, the release fails.
        node.state().writes_left = Some(1);
        w.push(HidEvent::TelephonyPhoneMute);
        w.flush().await;
        let (press, release) = report::event_to_press_release(HidEvent::TelephonyPhoneMute);
        assert_eq!(node.written_to(0), vec![press.to_vec()]);
        w.push(LOUDER);
        node.unbind();
        node.bind();
        w.flush().await;
        let mut want = vec![release.to_vec()];
        want.extend(reports(&[LOUDER]));
        assert_eq!(node.written_to(1), want, "no second press of the held key");
        assert_eq!(stats.events_phone_mute.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn while_the_node_is_missing_it_is_tried_once_a_second_and_the_keys_stay_bounded() {
        let node = FakeNode::default();
        let (mut w, stats) = writer(&node, REOPEN_EVERY);
        w.open_at_start();
        assert_eq!(node.opens(), 1);
        for _ in 0..(KEYS_WAITING + 8) {
            w.push(LOUDER);
            w.flush().await;
        }
        assert_eq!(node.opens(), 1, "no open before the back-off is over");
        assert_eq!(w.pending.len(), KEYS_WAITING);
        assert_eq!(stats.glue_drops.load(Ordering::Relaxed), 8, "the oldest went");
        let at = w.retry_at().unwrap();
        let wait = at.saturating_duration_since(Instant::now());
        assert!(wait > Duration::from_millis(900) && wait <= REOPEN_EVERY, "{wait:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn nothing_is_retried_when_no_key_waits() {
        let node = FakeNode::default();
        let (mut w, _) = writer(&node, REOPEN_EVERY);
        w.open_at_start();
        assert_eq!(w.retry_at(), None, "a missing node with no key to write is left alone");
    }
}
