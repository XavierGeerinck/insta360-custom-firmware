//! `/dev/hidg0` handles that come back after a gadget switch.
//!
//! USB storage (`waved-system` `usb_storage`, `docs/USB_STORAGE.md`) unbinds
//! the gadget, unlinks `hid.0` and links it again, on the way in and on the
//! way out: the node goes away and a new `/dev/hidg0` appears a few seconds
//! later. A handle on the old node is dead: its writes fail, and Linux 5.4
//! `f_hid` (which frees the function under open files) may also leave a read
//! or a write on it waiting for good. So every user of the node (the key
//! writer, the reader, the PC link's writer) goes through [`Reopening`]:
//!
//! - before it uses its handle it checks that the path still names the node
//!   it opened ([`Handle::is_current`]: device and inode), and opens the path
//!   again when not;
//! - after an error it closes the handle and opens again no sooner than
//!   [`REOPEN_EVERY`] later, so nothing spins while the node is missing;
//! - reports that wait for the node are kept in a bounded [`Pending`] queue
//!   (the oldest go first, and none is written more than [`STALE_AFTER`]
//!   late);
//! - the first failure of an outage and its end are logged ([`Outage`]),
//!   nothing in between.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

/// After a failure, the node is opened again no sooner than this.
pub const REOPEN_EVERY: Duration = Duration::from_secs(1);

/// A report that waited longer than this for the node is dropped, not
/// written: a key from before a long outage (a pulled cable) must not reach
/// the computer when it comes back. A gadget switch's outage is shorter
/// (about 7 s on the way back, `docs/USB_STORAGE.md`).
pub const STALE_AFTER: Duration = Duration::from_secs(10);

/// How long a read waits before the reader checks the node again.
pub const READ_WAIT: Duration = Duration::from_secs(1);

/// `/dev/hidg0`, or a fake in the tests.
pub trait Node {
    type Handle: Handle;
    fn open(&self) -> io::Result<Self::Handle>;
}

/// An open node.
pub trait Handle {
    /// False once the path names another node (the gadget was switched
    /// under this handle) or none.
    fn is_current(&self) -> bool;
    /// One whole report.
    fn write_report(&mut self, report: &[u8]) -> io::Result<()>;
    /// One report from the host, waiting up to `wait`: `Ok(None)` when none
    /// came. An error (end of file included) means the handle is dead.
    fn read_report(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<Option<usize>>;
}

/// The node at `path`, opened for reading or for writing.
#[derive(Debug, Clone)]
pub struct DevNode {
    path: String,
    write: bool,
}

impl DevNode {
    pub fn reader(path: &str) -> Self {
        Self { path: path.to_owned(), write: false }
    }

    pub fn writer(path: &str) -> Self {
        Self { path: path.to_owned(), write: true }
    }
}

pub struct DevHandle {
    file: std::fs::File,
    path: String,
    /// Device and inode of the node this handle opened.
    id: (u64, u64),
}

impl Node for DevNode {
    type Handle = DevHandle;

    fn open(&self) -> io::Result<DevHandle> {
        let file = std::fs::OpenOptions::new().read(!self.write).write(self.write).open(&self.path)?;
        let m = file.metadata()?;
        Ok(DevHandle { file, path: self.path.clone(), id: (m.dev(), m.ino()) })
    }
}

impl Handle for DevHandle {
    fn is_current(&self) -> bool {
        std::fs::metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.id)
    }

    fn write_report(&mut self, report: &[u8]) -> io::Result<()> {
        self.file.write_all(report)
    }

    fn read_report(&mut self, buf: &mut [u8], wait: Duration) -> io::Result<Option<usize>> {
        let mut fd = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = wait.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
        // SAFETY: one valid pollfd, owned by this frame, for the whole call.
        let n = unsafe { libc::poll(&mut fd, 1, ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(None) } else { Err(e) };
        }
        if n == 0 {
            return Ok(None);
        }
        if fd.revents & libc::POLLIN == 0 {
            // POLLERR, POLLHUP or POLLNVAL alone: polling again would return
            // at once, forever.
            return Err(io::Error::other(format!("poll says {:#x}", fd.revents)));
        }
        match self.file.read(buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "end of file")),
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Why [`Reopening::get`] gives no handle.
#[derive(Debug)]
pub enum Unavailable {
    /// Backing off after a failure; try again at [`Reopening::retry_at`].
    Waiting,
    /// The open failed just now (it is tried again in [`REOPEN_EVERY`]).
    OpenFailed(io::Error),
}

/// How [`Reopening::get`] came by its handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// The handle it already had.
    Already,
    /// Opened now: the first time, or after a failure.
    Fresh,
    /// Opened now because the path names a new node (the gadget was switched).
    Replaced,
}

/// A node's handle, opened again after errors and gadget switches.
pub struct Reopening<N: Node> {
    node: N,
    handle: Option<N::Handle>,
    retry_at: Option<Instant>,
    every: Duration,
}

impl<N: Node> Reopening<N> {
    pub fn new(node: N, every: Duration) -> Self {
        Self { node, handle: None, retry_at: None, every }
    }

    /// The handle to use now: the open one while the path still names its
    /// node, else a new open when one is due.
    pub fn get(&mut self, now: Instant) -> Result<(&mut N::Handle, Opened), Unavailable> {
        let mut opened = Opened::Already;
        if self.handle.as_ref().is_some_and(|h| !h.is_current()) {
            self.handle = None;
            opened = Opened::Replaced;
        }
        if self.handle.is_none() {
            if opened == Opened::Already {
                if self.retry_at.is_some_and(|t| now < t) {
                    return Err(Unavailable::Waiting);
                }
                opened = Opened::Fresh;
            }
            match self.node.open() {
                Ok(h) => {
                    self.handle = Some(h);
                    self.retry_at = None;
                }
                Err(e) => {
                    self.retry_at = Some(now + self.every);
                    return Err(Unavailable::OpenFailed(e));
                }
            }
        }
        match self.handle.as_mut() {
            Some(h) => Ok((h, opened)),
            None => Err(Unavailable::Waiting),
        }
    }

    /// The open handle as it is, unchecked (the reader while reports flow).
    pub fn open_handle(&mut self) -> Option<&mut N::Handle> {
        self.handle.as_mut()
    }

    /// The handle failed: close it; the next open is no sooner than the
    /// back-off from `now`.
    pub fn failed(&mut self, now: Instant) {
        self.handle = None;
        self.retry_at = Some(now + self.every);
    }

    /// When the next open may happen, while backing off.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at
    }

    /// How long to wait from `now` before trying again.
    pub fn wait(&self, now: Instant) -> Duration {
        self.retry_at.map_or(Duration::ZERO, |t| t.saturating_duration_since(now))
    }
}

/// An outage: from the first failure to the next success. Its first failure
/// and its end are logged, nothing in between.
#[derive(Debug, Default)]
pub struct Outage {
    failures: u64,
}

impl Outage {
    /// One failure; true for the first of an outage (log it).
    pub fn failed(&mut self) -> bool {
        self.failures += 1;
        self.failures == 1
    }

    /// It works; the outage's failure count if this ends one (log it).
    pub fn ended(&mut self) -> Option<u64> {
        (self.failures > 0).then(|| std::mem::take(&mut self.failures))
    }
}

/// Reports waiting for the node, oldest first. Bounded: past `max` the
/// oldest go; one older than `stale_after` is dropped instead of written.
pub struct Pending<T> {
    queue: VecDeque<(Instant, T)>,
    max: usize,
    stale_after: Duration,
}

impl<T> Pending<T> {
    pub fn new(max: usize, stale_after: Duration) -> Self {
        Self { queue: VecDeque::new(), max: max.max(1), stale_after }
    }

    /// Queue `item`; how many old ones went to make room.
    pub fn push(&mut self, now: Instant, item: T) -> usize {
        self.queue.push_back((now, item));
        let mut dropped = 0;
        while self.queue.len() > self.max {
            self.queue.pop_front();
            dropped += 1;
        }
        dropped
    }

    /// Drop the ones too old to write; how many.
    pub fn drop_stale(&mut self, now: Instant) -> usize {
        let before = self.queue.len();
        while self.queue.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) > self.stale_after) {
            self.queue.pop_front();
        }
        before - self.queue.len()
    }

    pub fn front(&self) -> Option<&T> {
        self.queue.front().map(|(_, item)| item)
    }

    pub fn pop(&mut self) -> Option<T> {
        self.queue.pop_front().map(|(_, item)| item)
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

/// A fake `/dev/hidg0` the tests switch like the gadget does.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// What a fake read returns.
    pub enum Read {
        Report(Vec<u8>),
        Fail(i32),
    }

    #[derive(Default)]
    pub struct State {
        pub present: bool,
        /// Bumped by every bind: handles from before are on the old node.
        pub generation: u32,
        pub opens: u32,
        /// Every write that went through, with the node it went to.
        pub written: Vec<(u32, Vec<u8>)>,
        /// The current node fails its writes with this errno.
        pub write_errno: Option<i32>,
        /// Writes that still go through before `write_errno` (EIO if unset)
        /// applies; `None`: no limit.
        pub writes_left: Option<u32>,
        /// What reads on the current node return, in order (then nothing).
        pub reads: VecDeque<Read>,
    }

    #[derive(Clone, Default)]
    pub struct FakeNode(pub Arc<Mutex<State>>);

    impl FakeNode {
        pub fn present() -> Self {
            let n = Self::default();
            n.0.lock().unwrap().present = true;
            n
        }
        pub fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.lock().unwrap()
        }
        /// The unbind: the node goes away.
        pub fn unbind(&self) {
            let mut s = self.state();
            s.present = false;
            s.reads.clear();
        }
        /// The bind: a new node at the path.
        pub fn bind(&self) {
            let mut s = self.state();
            s.present = true;
            s.generation += 1;
            s.write_errno = None;
            s.writes_left = None;
        }
        pub fn opens(&self) -> u32 {
            self.state().opens
        }
        /// The reports written to the node of `generation`.
        pub fn written_to(&self, generation: u32) -> Vec<Vec<u8>> {
            self.state().written.iter().filter(|(g, _)| *g == generation).map(|(_, r)| r.clone()).collect()
        }
    }

    pub struct FakeHandle {
        node: Arc<Mutex<State>>,
        generation: u32,
    }

    impl Node for FakeNode {
        type Handle = FakeHandle;
        fn open(&self) -> io::Result<FakeHandle> {
            let mut s = self.state();
            s.opens += 1;
            if !s.present {
                return Err(io::Error::from_raw_os_error(libc::ENOENT));
            }
            Ok(FakeHandle { node: Arc::clone(&self.0), generation: s.generation })
        }
    }

    impl Handle for FakeHandle {
        fn is_current(&self) -> bool {
            let s = self.node.lock().unwrap();
            s.present && s.generation == self.generation
        }
        fn write_report(&mut self, report: &[u8]) -> io::Result<()> {
            let mut s = self.node.lock().unwrap();
            if !(s.present && s.generation == self.generation) {
                return Err(io::Error::from_raw_os_error(libc::ENODEV));
            }
            match s.writes_left {
                Some(0) => return Err(io::Error::from_raw_os_error(s.write_errno.unwrap_or(libc::EIO))),
                Some(n) => s.writes_left = Some(n - 1),
                None => {
                    if let Some(errno) = s.write_errno {
                        return Err(io::Error::from_raw_os_error(errno));
                    }
                }
            }
            let g = s.generation;
            s.written.push((g, report.to_vec()));
            Ok(())
        }
        fn read_report(&mut self, buf: &mut [u8], _wait: Duration) -> io::Result<Option<usize>> {
            let mut s = self.node.lock().unwrap();
            if !(s.present && s.generation == self.generation) {
                // Linux 5.4 f_hid: a read on a freed function just waits.
                return Ok(None);
            }
            match s.reads.pop_front() {
                None => Ok(None),
                Some(Read::Report(r)) => {
                    buf[..r.len()].copy_from_slice(&r);
                    Ok(Some(r.len()))
                }
                Some(Read::Fail(errno)) => Err(io::Error::from_raw_os_error(errno)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeNode;
    use super::*;

    #[test]
    fn a_missing_node_is_tried_once_per_back_off_not_on_every_call() {
        let node = FakeNode::default();
        let mut r = Reopening::new(node.clone(), REOPEN_EVERY);
        let t0 = Instant::now();
        assert!(matches!(r.get(t0), Err(Unavailable::OpenFailed(_))));
        for ms in (0..1000).step_by(10) {
            assert!(matches!(r.get(t0 + Duration::from_millis(ms)), Err(Unavailable::Waiting)), "{ms} ms");
        }
        assert_eq!(node.opens(), 1);
        assert_eq!(r.wait(t0 + Duration::from_millis(400)), Duration::from_millis(600));
        node.bind();
        let (_, opened) = r.get(t0 + REOPEN_EVERY).unwrap();
        assert_eq!(opened, Opened::Fresh);
        assert_eq!(node.opens(), 2);
        assert_eq!(r.retry_at(), None);
    }

    #[test]
    fn a_handle_on_a_switched_node_is_replaced_by_the_new_node() {
        let node = FakeNode::present();
        let mut r = Reopening::new(node.clone(), REOPEN_EVERY);
        let t0 = Instant::now();
        assert_eq!(r.get(t0).unwrap().1, Opened::Fresh);
        assert_eq!(r.get(t0).unwrap().1, Opened::Already);
        node.unbind();
        node.bind();
        let (h, opened) = r.get(t0).unwrap();
        assert_eq!(opened, Opened::Replaced, "no back-off when the new node is already there");
        h.write_report(&[1, 2]).unwrap();
        assert_eq!(node.written_to(1), vec![vec![1, 2]]);

        // Switched and not back yet: the open fails and backs off.
        node.unbind();
        assert!(matches!(r.get(t0), Err(Unavailable::OpenFailed(_))));
        assert!(matches!(r.get(t0 + Duration::from_millis(500)), Err(Unavailable::Waiting)));
    }

    #[test]
    fn a_failed_handle_is_closed_and_opened_again_after_the_back_off() {
        let node = FakeNode::present();
        let mut r = Reopening::new(node.clone(), REOPEN_EVERY);
        let t0 = Instant::now();
        r.get(t0).unwrap();
        r.failed(t0);
        assert!(r.open_handle().is_none());
        assert!(matches!(r.get(t0 + Duration::from_millis(999)), Err(Unavailable::Waiting)));
        assert_eq!(r.get(t0 + REOPEN_EVERY).unwrap().1, Opened::Fresh);
        assert_eq!(node.opens(), 2);
    }

    #[test]
    fn an_outage_is_logged_once_and_its_end_once() {
        let mut o = Outage::default();
        assert_eq!(o.ended(), None, "nothing to end");
        assert!(o.failed());
        assert!(!o.failed());
        assert!(!o.failed());
        assert_eq!(o.ended(), Some(3));
        assert_eq!(o.ended(), None);
        assert!(o.failed(), "the next outage is logged again");
    }

    #[test]
    fn waiting_reports_are_bounded_oldest_first_and_never_written_stale() {
        let t0 = Instant::now();
        let mut p = Pending::new(3, STALE_AFTER);
        for i in 0..3 {
            assert_eq!(p.push(t0, i), 0);
        }
        assert_eq!(p.push(t0 + Duration::from_secs(5), 3), 1, "the oldest goes");
        assert_eq!(p.front(), Some(&1));
        assert_eq!(p.len(), 3);
        assert_eq!(p.drop_stale(t0 + STALE_AFTER), 0, "exactly STALE_AFTER old is still written");
        assert_eq!(p.drop_stale(t0 + STALE_AFTER + Duration::from_millis(1)), 2);
        assert_eq!(p.pop(), Some(3));
        assert!(p.is_empty());
    }

    #[test]
    fn a_dev_node_notices_when_its_path_names_another_node() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::write(&path, b"").unwrap();
        let p = path.to_string_lossy().into_owned();
        let mut h = DevNode::writer(&p).open().unwrap();
        assert!(h.is_current());
        h.write_report(&[0x01, 0x02]).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!h.is_current(), "gone");
        std::fs::write(&path, b"").unwrap();
        assert!(!h.is_current(), "a new node at the same path");
        assert!(DevNode::writer(&p).open().unwrap().is_current());
    }

    #[test]
    fn a_dev_node_read_returns_a_report_then_fails_at_the_end_instead_of_spinning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hidg0");
        std::fs::write(&path, [0x06, 0x01]).unwrap();
        let mut h = DevNode::reader(&path.to_string_lossy()).open().unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(h.read_report(&mut buf, Duration::from_millis(10)).unwrap(), Some(2));
        assert_eq!(&buf[..2], &[0x06, 0x01]);
        assert!(h.read_report(&mut buf, Duration::from_millis(10)).is_err(), "end of file is an error");
    }
}
