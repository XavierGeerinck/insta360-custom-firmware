//! Firmware updates for the module (`BT.bin`) and, relayed through it, the
//! 2.4 GHz dongle (`DONGLE.bin`): TODO F6, docs/BLUETOOTH_PROTOCOL.md.
//!
//! Stock `bt_upgrade` / `bt_upgrade_dongle` (libins_entry.so.c 78031, 78420):
//! skip the image's first 0x8000 bytes (the Telink boot area), announce the
//! size (`u32` at 0x8004, plus 0x1000), send the rest in 64-byte packets, each
//! with its address and CRC, then ask for a CRC check and a reset.
//!
//! | Step | Module | Dongle | Reply |
//! |------|--------|--------|-------|
//! | request | `0x80 AB` | — | `0x80` |
//! | size | `0x9D` | `0xD9` | `0x9E` / `0xDA` |
//! | packet | `0x9F` | `0xDB` | `0xA0` (byte 0 = 3 is an error) / `0xDC` (non-zero = error) |
//! | CRC check | `0xA1 01` | `0xDD 01` | `0xA1` (byte 0 = 1 is a pass) / `0xDE` (non-zero = error) |
//! | reset | `0xA2 01` | `0xDF 01` | `0xA2` / — |
//!
//! [`flash`] runs that sequence with stock's waits and tries over a [`Link`].
//! Only the `waved-bt-flash` tool calls it, by hand, with waved stopped and
//! the owner's approval: never the service, never at boot
//! (docs/BLUETOOTH_PROTOCOL.md, "Firmware").

use crate::command::id;
use crate::frame::Frame;
use std::time::{Duration, Instant};

/// Bytes at the start of an image that are never sent (boot area).
pub const BOOT_AREA: usize = 0x8000;
/// Firmware bytes per packet.
pub const PACKET_LEN: usize = 64;
/// Stock refuses a module image smaller than this.
pub const MIN_MODULE_IMAGE: usize = 0x4B000;
/// Stock refuses a dongle image larger than this.
pub const MAX_DONGLE_IMAGE: usize = 0x4B000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The Bluetooth module on `/dev/ttyS1` (`BT.bin`).
    Module,
    /// The USB dongle, over the module's 2.4 GHz link (`DONGLE.bin`).
    Dongle,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ImageError {
    /// Outside stock's size bounds for the target.
    Size(usize),
    /// The size field at 0x8004 does not match the file.
    SizeField { field: u32, payload: usize },
}

/// A firmware image, checked like stock checks it.
#[derive(Debug)]
pub struct Image<'a> {
    bytes: &'a [u8],
    target: Target,
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8], target: Target) -> Result<Self, ImageError> {
        let len = bytes.len();
        let in_bounds = match target {
            Target::Module => len >= MIN_MODULE_IMAGE,
            Target::Dongle => len <= MAX_DONGLE_IMAGE,
        };
        if !in_bounds || len < BOOT_AREA + 8 {
            return Err(ImageError::Size(len));
        }
        let image = Self { bytes, target };
        let payload = len - BOOT_AREA;
        if image.fw_size() as usize != payload {
            let field = u32::from_le_bytes(bytes[0x8004..0x8008].try_into().unwrap());
            return Err(ImageError::SizeField { field, payload });
        }
        Ok(image)
    }

    /// The size announced to the target: `u32` at 0x8004, plus 0x1000.
    pub fn fw_size(&self) -> u32 {
        let field = u32::from_le_bytes(self.bytes[0x8004..0x8008].try_into().unwrap());
        field.wrapping_add(0x1000)
    }

    /// The bytes that are sent: everything after the boot area.
    pub fn payload(&self) -> &'a [u8] {
        &self.bytes[BOOT_AREA..]
    }

    /// Every packet, in order.
    pub fn packets(&self) -> impl Iterator<Item = Frame> + '_ {
        self.payload()
            .chunks(PACKET_LEN)
            .enumerate()
            .map(|(i, chunk)| packet(self.target, (i * PACKET_LEN) as u32, chunk))
    }
}

/// Stock `calculate_CRC32(0xFFFFFFFF, ..)`: the reflected CRC-32 table update
/// with no final XOR, i.e. the bitwise NOT of the usual CRC-32.
pub fn crc32(data: &[u8]) -> u32 {
    data.iter().fold(0xFFFF_FFFF, |crc, &b| {
        (0..8).fold(crc ^ u32::from(b), |c, _| if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 })
    })
}

/// `0x80 AB`: ask the module to accept an update (module only).
pub fn request() -> Frame {
    Frame::request(id::OTA_REQUEST, &[0xAB])
}

/// The firmware size announcement.
pub fn fw_info(target: Target, size: u32) -> Frame {
    let cmd = match target {
        Target::Module => id::OTA_FW_INFO,
        Target::Dongle => id::DONGLE_OTA_FW_INFO,
    };
    Frame::request(cmd, &size.to_le_bytes())
}

/// One packet: `addr u32 | crc u32 | len u16 | offset u16 | data`, all LE.
/// `addr` counts from the end of the boot area. The module's `offset` is the
/// length (0 for a short last packet); the dongle's is always 0.
///
/// Panics if `chunk` is longer than [`PACKET_LEN`] (a programming error).
pub fn packet(target: Target, addr: u32, chunk: &[u8]) -> Frame {
    assert!(chunk.len() <= PACKET_LEN, "OTA chunk too long: {}", chunk.len());
    let len = chunk.len() as u16;
    let (cmd, offset) = match target {
        Target::Module => (id::OTA_PACKET, if chunk.len() < PACKET_LEN { 0 } else { len }),
        Target::Dongle => (id::DONGLE_OTA_PACKET, 0),
    };
    let mut data = Vec::with_capacity(12 + chunk.len());
    data.extend_from_slice(&addr.to_le_bytes());
    data.extend_from_slice(&crc32(chunk).to_le_bytes());
    data.extend_from_slice(&len.to_le_bytes());
    data.extend_from_slice(&offset.to_le_bytes());
    data.extend_from_slice(chunk);
    Frame::request(cmd, &data)
}

/// Ask the target to check the whole image's CRC.
pub fn crc_check(target: Target) -> Frame {
    match target {
        Target::Module => Frame::request(id::OTA_CRC, &[1]),
        Target::Dongle => Frame::request(id::DONGLE_OTA_CRC, &[1]),
    }
}

/// Restart the target into the new firmware.
pub fn reset(target: Target) -> Frame {
    match target {
        Target::Module => Frame::request(id::OTA_RESET, &[1]),
        Target::Dongle => Frame::request(id::DONGLE_OTA_RESET, &[1]),
    }
}

/// Stock's size frame for the module as it reaches the wire. `bt_upgrade`
/// keeps the size in the same union as the request's `0xAB` key and builds
/// the request first, so the size's low byte is always `0xAB`
/// (`bt_ota_cmd_send` 74668: `boot_mode_key = 0xab` then `fw_size`; the
/// 1.2.2.6 image announces `9D AB FF 08 00`, not `9D 0A FF 08 00`). It is
/// the only size frame the module has been updated with; waved sends the
/// same. The dongle gets no request, so its size is exact.
pub fn fw_info_as_stock(target: Target, size: u32) -> Frame {
    match target {
        Target::Module => fw_info(target, (size & !0xFF) | 0xAB),
        Target::Dongle => fw_info(target, size),
    }
}

/// The version and build time an image carries, if it names them
/// (`0.0.0.33`, `2026-06-12 12:43:40`): what the module would report after
/// the update. Compare both: 3.0.0.1 and 1.2.2.6 both ship 0.0.0.33.
pub fn image_version(bytes: &[u8]) -> (Option<String>, Option<String>) {
    let version = bytes.split(|b| !b.is_ascii_graphic()).find_map(|run| {
        let run = std::str::from_utf8(run).ok()?;
        let at = run.find("0.0.0.")?;
        let digits: String = run[at + 6..].chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty()).then(|| format!("0.0.0.{digits}"))
    });
    let built = bytes.windows(19).find_map(|w| {
        let shape = w.iter().enumerate().all(|(i, &b)| match i {
            4 | 7 => b == b'-',
            10 => b == b' ',
            13 | 16 => b == b':',
            _ => b.is_ascii_digit(),
        });
        (shape && w.starts_with(b"20")).then(|| String::from_utf8_lossy(w).into_owned())
    });
    (version, built)
}

/// The UART as the update needs it. [`flash`] acknowledges what the module
/// repeats (`75 45`) itself.
pub trait Link {
    fn send(&mut self, frame: &Frame);
    /// The next frame from the module, or `None` once `timeout` has passed.
    fn recv(&mut self, timeout: Duration) -> Option<Frame>;
    /// Stock's sleeps (1 s after a reset).
    fn pause(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// How long one try waits for its reply. Stock's `timeout_set` (77631)
/// counts 1 ms polls (`bt_ota_cmd_send` 74813).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waits {
    pub request: Duration,
    pub fw_info: Duration,
    pub packet: Duration,
    pub crc: Duration,
    pub reset: Duration,
    pub dongle_fw_info: Duration,
    pub dongle_packet: Duration,
    pub dongle_crc: Duration,
    pub dongle_reset: Duration,
}

pub const STOCK_WAITS: Waits = Waits {
    request: Duration::from_millis(3_000),
    fw_info: Duration::from_millis(3_000),
    packet: Duration::from_millis(3_000),
    crc: Duration::from_millis(5_000),
    reset: Duration::from_millis(1_000),
    dongle_fw_info: Duration::from_millis(1_000),
    dongle_packet: Duration::from_millis(1_000),
    dongle_crc: Duration::from_millis(10_000),
    dongle_reset: Duration::from_millis(1_000),
};

/// Stock's tries: request, size and CRC 20 each (`retry < 0x14`), each
/// packet 21 (`retry < 0x15`, 78214; dongle `0x14 < retry`, 78559), the
/// dongle's reset 3 (78649).
pub const STEP_TRIES: u32 = 20;
pub const PACKET_TRIES: u32 = 21;
pub const DONGLE_RESET_TRIES: u32 = 3;

/// How far an update got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// The module took `0x80 AB`: it is in update mode now.
    Requested,
    /// The size was answered (`0x9E` / `0xDA`).
    SizeTaken,
    /// The size went unanswered 20 times; stock goes on regardless.
    SizeUnanswered,
    Packet { sent: usize, of: usize },
    CrcPassed,
    /// The reset went out (module `A2 01`, dongle `DF 01`).
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlashError {
    /// The module never answered `0x80 AB`. Nothing was sent after it; the
    /// module may still wait in update mode: power-cycle it (btpower 1, 0).
    NoRequestAnswer,
    /// The dongle never took its size; nothing was written.
    NoSizeAnswer,
    /// Packet `index` failed every try. The module was reset (stock), the
    /// dongle was not (stock).
    Packet { index: usize, of: usize },
    /// The CRC check never passed. The target was reset (stock).
    Crc,
}

/// Flash `image` the way stock does (`bt_upgrade` 78031, `bt_upgrade_dongle`
/// 78420), with `waits` per try ([`STOCK_WAITS`]). Never called by the
/// service: only the `waved-bt-flash` tool runs it.
///
/// Where waved departs from stock, it is to stop earlier or to be stricter:
/// it stops when the module never takes the request (stock sends the whole
/// image anyway); the dongle's size is tried 20 times (stock's tries stop
/// counting after one failure, 78512); a CRC reply that fails is a failure
/// for the dongle too (stock takes it as a pass, 78624); a failing reply to
/// the CRC check does not cut its 5 s / 10 s wait short, so a later pass in
/// the same window still counts; and each packet gets its 21 tries (stock's
/// first packet inherits the size step's count).
pub fn flash(
    link: &mut impl Link,
    image: &Image,
    waits: &Waits,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), FlashError> {
    match image.target {
        Target::Module => flash_module(link, image, waits, progress),
        Target::Dongle => flash_dongle(link, image, waits, progress),
    }
}

fn flash_module(
    link: &mut impl Link,
    image: &Image,
    waits: &Waits,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), FlashError> {
    if !try_step(link, &request(), id::OTA_REQUEST, waits.request, STEP_TRIES, |_| true) {
        return Err(FlashError::NoRequestAnswer);
    }
    progress(Progress::Requested);
    let size = fw_info_as_stock(Target::Module, image.fw_size());
    // Stock goes on to the packets without an answer (78135).
    progress(if try_step(link, &size, id::OTA_FW_INFO_DONE, waits.fw_info, STEP_TRIES, |_| true) {
        Progress::SizeTaken
    } else {
        Progress::SizeUnanswered
    });
    let result = send_packets(link, image, id::OTA_PACKET_DONE, waits.packet, |f| f.data.first() != Some(&3), progress)
        .and_then(|()| {
            let passed = try_step(link, &crc_check(Target::Module), id::OTA_CRC, waits.crc, STEP_TRIES, |f| {
                f.data.first() == Some(&1)
            });
            if passed {
                progress(Progress::CrcPassed);
                Ok(())
            } else {
                Err(FlashError::Crc)
            }
        });
    // Stock resets the module on every path once it is in update mode
    // (78262), then sleeps 1 s.
    link.send(&reset(Target::Module));
    wait_for(link, id::OTA_RESET, waits.reset, |_| true);
    progress(Progress::Reset);
    link.pause(Duration::from_secs(1));
    result
}

fn flash_dongle(
    link: &mut impl Link,
    image: &Image,
    waits: &Waits,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), FlashError> {
    let size = fw_info_as_stock(Target::Dongle, image.fw_size());
    if !try_step(link, &size, id::DONGLE_OTA_FW_INFO_DONE, waits.dongle_fw_info, STEP_TRIES, |_| true) {
        return Err(FlashError::NoSizeAnswer);
    }
    progress(Progress::SizeTaken);
    // Stock reads the dongle's verdict from byte 4 (74854) and sends no
    // reset after a packet that failed every try.
    send_packets(link, image, id::DONGLE_OTA_PACKET_DONE, waits.dongle_packet, |f| f.data.get(4).is_none_or(|&b| b == 0), progress)?;
    let passed = try_step(link, &crc_check(Target::Dongle), id::DONGLE_OTA_CRC_DONE, waits.dongle_crc, STEP_TRIES, |f| {
        f.data.first().is_none_or(|&b| b == 0)
    });
    if passed {
        progress(Progress::CrcPassed);
    }
    // Stock: up to 3 resets, 1 s wait and 1 s sleep each, while the dongle
    // still shows (78649-78670). It never answers `DF`.
    for _ in 0..DONGLE_RESET_TRIES {
        link.send(&reset(Target::Dongle));
        let gone = wait_for(link, id::WORK_STATUS, waits.dongle_reset, |f| {
            f.data.first().is_some_and(|&bits| bits & 0x10 == 0)
        })
        .is_some();
        link.pause(Duration::from_secs(1));
        if gone {
            break;
        }
    }
    progress(Progress::Reset);
    if passed {
        Ok(())
    } else {
        Err(FlashError::Crc)
    }
}

/// Every packet in order, each tried up to [`PACKET_TRIES`] times.
fn send_packets(
    link: &mut impl Link,
    image: &Image,
    reply: u8,
    wait: Duration,
    ok: impl Fn(&Frame) -> bool + Copy,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), FlashError> {
    let of = image.payload().len().div_ceil(PACKET_LEN);
    for (index, packet) in image.packets().enumerate() {
        if !try_step(link, &packet, reply, wait, PACKET_TRIES, ok) {
            return Err(FlashError::Packet { index, of });
        }
        progress(Progress::Packet { sent: index + 1, of });
    }
    Ok(())
}

/// Send `frame` until a `reply` frame that `ok` accepts arrives, `tries`
/// times at most, waiting `wait` each time. A reply that `ok` refuses ends
/// the try at once (stock re-sends at once), except for the CRC check,
/// where the rest of the window is still read.
fn try_step(
    link: &mut impl Link,
    frame: &Frame,
    reply: u8,
    wait: Duration,
    tries: u32,
    ok: impl Fn(&Frame) -> bool + Copy,
) -> bool {
    let crc = matches!(frame.cmd, id::OTA_CRC | id::DONGLE_OTA_CRC);
    for _ in 0..tries {
        link.send(frame);
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match next(link, left) {
                Some(f) if f.cmd == reply && ok(&f) => return true,
                Some(f) if f.cmd == reply && !crc => break,
                Some(_) => continue,
                None => break,
            }
        }
    }
    false
}

/// The first `reply` frame `ok` accepts within `wait`.
fn wait_for(link: &mut impl Link, reply: u8, wait: Duration, ok: impl Fn(&Frame) -> bool) -> Option<Frame> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let f = next(link, left)?;
        if f.cmd == reply && ok(&f) {
            return Some(f);
        }
    }
}

/// The next frame, acknowledged as the service acknowledges it (a `75 45`
/// frame the module would otherwise repeat every 500 ms, holding back the
/// rest of its queue).
fn next(link: &mut impl Link, timeout: Duration) -> Option<Frame> {
    let frame = link.recv(timeout)?;
    if let Some(ack) = crate::command::ack_for_frame(&frame) {
        link.send(&ack);
    }
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    /// A synthetic image: boot area, then `payload_len` bytes, size field set.
    fn image(payload_len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; BOOT_AREA + payload_len];
        let field = (payload_len as u32).wrapping_sub(0x1000);
        bytes[0x8004..0x8008].copy_from_slice(&field.to_le_bytes());
        for (i, b) in bytes[BOOT_AREA + 8..].iter_mut().enumerate() {
            *b = i as u8;
        }
        bytes
    }

    #[test]
    fn crc_matches_stock() {
        // Standard CRC-32 of "123456789" is 0xCBF43926; stock omits the final XOR.
        assert_eq!(crc32(b"123456789"), 0x340B_C6D9);
        assert_eq!(crc32(&[]), 0xFFFF_FFFF);
    }

    #[test]
    fn control_frames() {
        assert_eq!(hex(&request().encode()), "75 45 02 80 AB 2D");
        assert_eq!(hex(&crc_check(Target::Module).encode()), "75 45 02 A1 01 A4");
        assert_eq!(hex(&reset(Target::Module).encode()), "75 45 02 A2 01 A5");
        assert_eq!(hex(&crc_check(Target::Dongle).encode()), "75 45 02 DD 01 E0");
        assert_eq!(hex(&reset(Target::Dongle).encode()), "75 45 02 DF 01 E2");
    }

    #[test]
    fn fw_info_frames() {
        // 1.2.2.6 BT.bin: 622346 bytes, field 0x8EF0A -> 589578.
        assert_eq!(hex(&fw_info(Target::Module, 589_578).encode()), "75 45 05 9D 0A FF 08 00 B3");
        // 1.2.2.6 DONGLE.bin: 179212 bytes -> 146444.
        assert_eq!(hex(&fw_info(Target::Dongle, 146_444).encode()), "75 45 05 D9 0C 3C 02 00 28");
    }

    #[test]
    fn full_module_packet() {
        let chunk: Vec<u8> = (0..64).collect();
        let raw = packet(Target::Module, 0x40, &chunk).encode();
        assert_eq!(raw.len(), 81);
        assert_eq!(hex(&raw[..16]), "75 45 4D 9F 40 00 00 00 73 31 F1 EF 40 00 40 00");
        assert_eq!(&raw[16..80], &chunk[..]);
        assert_eq!(raw[80], 0x10);
    }

    #[test]
    fn short_last_module_packet_has_offset_zero() {
        assert_eq!(
            hex(&packet(Target::Module, 0x80, &[1, 2, 3, 4, 5]).encode()),
            "75 45 12 9F 80 00 00 00 0B 66 F4 B8 05 00 00 00 01 02 03 04 05 62"
        );
    }

    #[test]
    fn dongle_packets_have_offset_zero() {
        let chunk: Vec<u8> = (0..64).collect();
        let f = packet(Target::Dongle, 0, &chunk);
        assert_eq!(f.cmd, 0xDB);
        assert_eq!(&f.data[8..12], &[0x40, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn parses_a_module_image_and_splits_it() {
        let bytes = image(MIN_MODULE_IMAGE - BOOT_AREA + 100);
        let img = Image::parse(&bytes, Target::Module).unwrap();
        assert_eq!(img.fw_size() as usize, bytes.len() - BOOT_AREA);
        assert_eq!(img.payload(), &bytes[BOOT_AREA..]);

        let packets: Vec<Frame> = img.packets().collect();
        let n = img.payload().len();
        assert_eq!(packets.len(), n.div_ceil(PACKET_LEN));
        // Addresses advance by 64 and the data reassembles to the payload.
        let mut joined = Vec::new();
        for (i, p) in packets.iter().enumerate() {
            assert_eq!(p.cmd, 0x9F);
            let addr = u32::from_le_bytes(p.data[0..4].try_into().unwrap());
            assert_eq!(addr as usize, i * PACKET_LEN);
            let len = u16::from_le_bytes(p.data[8..10].try_into().unwrap()) as usize;
            let chunk = &p.data[12..12 + len];
            assert_eq!(u32::from_le_bytes(p.data[4..8].try_into().unwrap()), crc32(chunk));
            joined.extend_from_slice(chunk);
        }
        assert_eq!(joined, img.payload());
        // 100 extra bytes past a multiple of 64 leave a 36-byte last packet.
        let last = packets.last().unwrap();
        assert_eq!(u16::from_le_bytes(last.data[8..10].try_into().unwrap()), 36);
        assert_eq!(u16::from_le_bytes(last.data[10..12].try_into().unwrap()), 0);
    }

    #[test]
    fn refuses_images_outside_stock_bounds() {
        let small = image(MIN_MODULE_IMAGE - BOOT_AREA - 1);
        assert_eq!(
            Image::parse(&small, Target::Module).unwrap_err(),
            ImageError::Size(MIN_MODULE_IMAGE - 1)
        );
        let big = image(MAX_DONGLE_IMAGE - BOOT_AREA + 1);
        assert_eq!(
            Image::parse(&big, Target::Dongle).unwrap_err(),
            ImageError::Size(MAX_DONGLE_IMAGE + 1)
        );
        assert!(matches!(Image::parse(&[0u8; 16], Target::Dongle), Err(ImageError::Size(16))));
    }

    #[test]
    fn refuses_a_size_field_that_does_not_match() {
        let mut bytes = image(0x22C0C + 0x1000);
        bytes[0x8004] ^= 1;
        assert!(matches!(Image::parse(&bytes, Target::Dongle), Err(ImageError::SizeField { .. })));
    }

    #[test]
    fn the_modules_size_goes_out_as_stock_sends_it() {
        // 1.2.2.6 BT.bin announces 589 578 (0x08FF0A); stock's union puts the
        // request's 0xAB over the low byte.
        assert_eq!(hex(&fw_info_as_stock(Target::Module, 589_578).encode()), "75 45 05 9D AB FF 08 00 54");
        assert_eq!(hex(&fw_info_as_stock(Target::Dongle, 146_444).encode()), "75 45 05 D9 0C 3C 02 00 28");
    }

    #[test]
    fn an_image_names_its_version_and_build_time() {
        let mut bytes = image(0x22C0C + 0x1000);
        let at = BOOT_AREA + 0x100;
        bytes[at..at + 10].copy_from_slice(b"\0V0.0.0.33");
        bytes[at + 20..at + 39].copy_from_slice(b"2026-06-12 12:43:40");
        assert_eq!(
            image_version(&bytes),
            (Some("0.0.0.33".into()), Some("2026-06-12 12:43:40".into()))
        );
        assert_eq!(image_version(&[0u8; 64]), (None, None));
    }

    mod flashing {
        use super::super::*;
        use super::image;
        use crate::frame::Prefix;
        use std::collections::VecDeque;

        /// A module that answers as `script` says; records what it got.
        struct Fake<F: FnMut(&Frame) -> Vec<Frame>> {
            script: F,
            inbox: VecDeque<Frame>,
            sent: Vec<Frame>,
            pauses: Vec<Duration>,
        }

        impl<F: FnMut(&Frame) -> Vec<Frame>> Fake<F> {
            fn new(script: F) -> Self {
                Self { script, inbox: VecDeque::new(), sent: Vec::new(), pauses: Vec::new() }
            }

            /// Our requests (not our acknowledgements), as command bytes.
            fn requests(&self) -> Vec<u8> {
                self.sent.iter().filter(|f| f.prefix == Prefix::Request).map(|f| f.cmd).collect()
            }
        }

        impl<F: FnMut(&Frame) -> Vec<Frame>> Link for Fake<F> {
            fn send(&mut self, frame: &Frame) {
                self.sent.push(frame.clone());
                if frame.prefix == Prefix::Request {
                    let replies = (self.script)(frame);
                    self.inbox.extend(replies);
                }
            }
            fn recv(&mut self, _timeout: Duration) -> Option<Frame> {
                self.inbox.pop_front()
            }
            fn pause(&mut self, d: Duration) {
                self.pauses.push(d);
            }
        }

        fn reply(cmd: u8, data: &[u8]) -> Vec<Frame> {
            vec![Frame::request(cmd, data)]
        }

        /// A module that takes everything.
        fn willing(f: &Frame) -> Vec<Frame> {
            match f.cmd {
                0x80 => reply(0x80, &[0]),
                0x9D => reply(0x9E, &[0]),
                0x9F => reply(0xA0, &[0]),
                0xA1 => reply(0xA1, &[1]),
                0xA2 => reply(0xA2, &[0]),
                0xD9 => reply(0xDA, &[0]),
                0xDB => reply(0xDC, &[0, 0, 0, 0, 0]),
                0xDD => reply(0xDE, &[0]),
                0xDF => reply(0xB2, &[0x01]),
                _ => vec![],
            }
        }

        fn module_image() -> Vec<u8> {
            image(MIN_MODULE_IMAGE - BOOT_AREA + 100)
        }

        fn dongle_image() -> Vec<u8> {
            image(0x2000 + 30)
        }

        fn run<F: FnMut(&Frame) -> Vec<Frame>>(
            fake: &mut Fake<F>,
            bytes: &[u8],
            target: Target,
        ) -> (Result<(), FlashError>, Vec<Progress>) {
            let img = Image::parse(bytes, target).unwrap();
            let mut seen = Vec::new();
            let result = flash(fake, &img, &STOCK_WAITS, &mut |p| seen.push(p));
            (result, seen)
        }

        #[test]
        fn the_module_is_flashed_as_stock_does_it() {
            let bytes = module_image();
            let img = Image::parse(&bytes, Target::Module).unwrap();
            let packets = img.payload().len().div_ceil(PACKET_LEN);
            let mut fake = Fake::new(willing);
            let (result, seen) = run(&mut fake, &bytes, Target::Module);
            assert_eq!(result, Ok(()));
            let requests = fake.requests();
            assert_eq!(requests.len(), 2 + packets + 2);
            assert_eq!(&requests[..2], [0x80, 0x9D]);
            assert!(requests[2..2 + packets].iter().all(|&c| c == 0x9F));
            assert_eq!(&requests[2 + packets..], [0xA1, 0xA2]);
            // Byte for byte: the request, stock's size, the first packet.
            let sent: Vec<&Frame> = fake.sent.iter().filter(|f| f.prefix == Prefix::Request).collect();
            assert_eq!(sent[0], &request());
            assert_eq!(sent[1], &fw_info_as_stock(Target::Module, img.fw_size()));
            assert_eq!(sent[1].data[0], 0xAB);
            assert_eq!(sent[2], &img.packets().next().unwrap());
            assert_eq!(sent.last().unwrap(), &&reset(Target::Module));
            // Each module frame was acknowledged (75 45, stock's bytes).
            let acks = fake.sent.iter().filter(|f| f.prefix == Prefix::Ack).count();
            assert_eq!(acks, 2 + packets + 2);
            assert_eq!(fake.pauses, [Duration::from_secs(1)], "stock sleeps 1 s after the reset");
            assert_eq!(seen.first(), Some(&Progress::Requested));
            assert_eq!(seen[1], Progress::SizeTaken);
            assert_eq!(seen[2 + packets - 1], Progress::Packet { sent: packets, of: packets });
            assert_eq!(&seen[seen.len() - 2..], [Progress::CrcPassed, Progress::Reset]);
        }

        #[test]
        fn a_module_that_never_takes_the_request_gets_nothing_else() {
            let mut fake = Fake::new(|_: &Frame| vec![]);
            let (result, seen) = run(&mut fake, &module_image(), Target::Module);
            assert_eq!(result, Err(FlashError::NoRequestAnswer));
            assert_eq!(fake.requests(), vec![0x80; STEP_TRIES as usize]);
            assert!(seen.is_empty());
        }

        #[test]
        fn an_unanswered_size_does_not_stop_it_as_on_stock() {
            let mut fake = Fake::new(|f: &Frame| if f.cmd == 0x9D { vec![] } else { willing(f) });
            let (result, seen) = run(&mut fake, &module_image(), Target::Module);
            assert_eq!(result, Ok(()));
            assert_eq!(seen[1], Progress::SizeUnanswered);
            assert_eq!(fake.requests().iter().filter(|&&c| c == 0x9D).count(), STEP_TRIES as usize);
        }

        #[test]
        fn a_refused_packet_is_sent_again() {
            // Byte 0 = 3 is the module's error (bt_ota_cmd_send 74828).
            let mut refusals = 2;
            let mut fake = Fake::new(move |f: &Frame| {
                if f.cmd == 0x9F && f.data[..4] == [0, 0, 0, 0] && refusals > 0 {
                    refusals -= 1;
                    return reply(0xA0, &[3]);
                }
                willing(f)
            });
            let (result, _) = run(&mut fake, &module_image(), Target::Module);
            assert_eq!(result, Ok(()));
            let requests = fake.requests();
            assert_eq!(&requests[2..5], [0x9F, 0x9F, 0x9F], "the first packet three times");
            let firsts: Vec<&Frame> = fake.sent.iter().filter(|f| f.cmd == 0x9F).take(4).collect();
            assert_eq!(firsts[0], firsts[2]);
            assert_ne!(firsts[2], firsts[3], "then the next one");
        }

        #[test]
        fn a_packet_that_never_goes_through_stops_it_with_a_reset() {
            let mut fake = Fake::new(|f: &Frame| {
                if f.cmd == 0x9F && f.data[..4] == [64, 0, 0, 0] {
                    return vec![];
                }
                willing(f)
            });
            let (result, seen) = run(&mut fake, &module_image(), Target::Module);
            let of = Image::parse(&module_image(), Target::Module).unwrap().payload().len().div_ceil(PACKET_LEN);
            assert_eq!(result, Err(FlashError::Packet { index: 1, of }));
            let requests = fake.requests();
            assert_eq!(requests.iter().filter(|&&c| c == 0x9F).count(), 1 + PACKET_TRIES as usize);
            assert!(!requests.contains(&0xA1), "no CRC check");
            assert_eq!(requests.last(), Some(&0xA2), "stock resets the module on every path");
            assert_eq!(seen.last(), Some(&Progress::Reset));
        }

        #[test]
        fn a_crc_that_never_passes_fails_after_twenty_checks() {
            let mut fake = Fake::new(|f: &Frame| if f.cmd == 0xA1 { reply(0xA1, &[0]) } else { willing(f) });
            let (result, _) = run(&mut fake, &module_image(), Target::Module);
            assert_eq!(result, Err(FlashError::Crc));
            let requests = fake.requests();
            assert_eq!(requests.iter().filter(|&&c| c == 0xA1).count(), STEP_TRIES as usize);
            assert_eq!(requests.last(), Some(&0xA2));
        }

        #[test]
        fn a_crc_pass_after_a_plain_answer_in_the_same_window_counts() {
            // If the module answers the check at once with `A1 00` (as it
            // answers other requests) and the verdict after, the verdict wins.
            let mut fake = Fake::new(|f: &Frame| {
                if f.cmd == 0xA1 {
                    let mut out = reply(0xA1, &[0]);
                    out.extend(reply(0xA1, &[1]));
                    return out;
                }
                willing(f)
            });
            let (result, _) = run(&mut fake, &module_image(), Target::Module);
            assert_eq!(result, Ok(()));
            assert_eq!(fake.requests().iter().filter(|&&c| c == 0xA1).count(), 1);
        }

        #[test]
        fn the_dongle_is_flashed_as_stock_does_it() {
            let bytes = dongle_image();
            let img = Image::parse(&bytes, Target::Dongle).unwrap();
            let packets = img.payload().len().div_ceil(PACKET_LEN);
            let mut fake = Fake::new(willing);
            let (result, seen) = run(&mut fake, &bytes, Target::Dongle);
            assert_eq!(result, Ok(()));
            let requests = fake.requests();
            assert_eq!(requests[0], 0xD9, "no request for the dongle");
            assert_eq!(&requests[packets + 1..], [0xDD, 0xDF], "one reset: the dongle went away");
            let size = fake.sent.iter().find(|f| f.cmd == 0xD9).unwrap();
            assert_eq!(size, &fw_info(Target::Dongle, img.fw_size()), "the exact size");
            assert!(fake.sent.iter().filter(|f| f.cmd == 0xDB).all(|f| f.data[10..12] == [0, 0]));
            assert_eq!(fake.pauses, [Duration::from_secs(1)]);
            assert_eq!(seen.last(), Some(&Progress::Reset));
        }

        #[test]
        fn the_dongle_is_reset_three_times_while_it_stays() {
            let mut fake = Fake::new(|f: &Frame| if f.cmd == 0xDF { reply(0xB2, &[0x11]) } else { willing(f) });
            let (result, _) = run(&mut fake, &dongle_image(), Target::Dongle);
            assert_eq!(result, Ok(()));
            assert_eq!(fake.requests().iter().filter(|&&c| c == 0xDF).count(), DONGLE_RESET_TRIES as usize);
            assert_eq!(fake.pauses.len(), DONGLE_RESET_TRIES as usize);
        }

        #[test]
        fn the_dongles_verdict_is_byte_four() {
            // Stock 74854 reads byte 4 of 0xDC; a non-zero one is sent again.
            let mut refused = false;
            let mut fake = Fake::new(move |f: &Frame| {
                if f.cmd == 0xDB && !refused {
                    refused = true;
                    return reply(0xDC, &[0, 0, 0, 0, 1]);
                }
                if f.cmd == 0xDB {
                    return reply(0xDC, &[9, 9, 9, 9, 0]);
                }
                willing(f)
            });
            let (result, _) = run(&mut fake, &dongle_image(), Target::Dongle);
            assert_eq!(result, Ok(()));
            let first: Vec<&Frame> = fake.sent.iter().filter(|f| f.cmd == 0xDB).take(2).collect();
            assert_eq!(first[0], first[1]);
        }

        #[test]
        fn a_dongle_that_refuses_a_packet_is_left_alone() {
            // Stock returns without a reset (78559).
            let mut fake = Fake::new(|f: &Frame| if f.cmd == 0xDB { reply(0xDC, &[0, 0, 0, 0, 2]) } else { willing(f) });
            let (result, _) = run(&mut fake, &dongle_image(), Target::Dongle);
            assert!(matches!(result, Err(FlashError::Packet { index: 0, .. })));
            let requests = fake.requests();
            assert_eq!(requests.iter().filter(|&&c| c == 0xDB).count(), PACKET_TRIES as usize);
            assert!(!requests.contains(&0xDD) && !requests.contains(&0xDF));
        }

        #[test]
        fn a_dongle_crc_error_is_a_failure_but_still_resets() {
            // Stock counts a failing 0xDE as a pass (78624); waved does not.
            let mut fake = Fake::new(|f: &Frame| if f.cmd == 0xDD { reply(0xDE, &[1]) } else { willing(f) });
            let (result, _) = run(&mut fake, &dongle_image(), Target::Dongle);
            assert_eq!(result, Err(FlashError::Crc));
            assert_eq!(fake.requests().last(), Some(&0xDF));
        }

        #[test]
        fn a_dongle_that_never_takes_its_size_gets_nothing_else() {
            let mut fake = Fake::new(|_: &Frame| vec![]);
            let (result, _) = run(&mut fake, &dongle_image(), Target::Dongle);
            assert_eq!(result, Err(FlashError::NoSizeAnswer));
            assert_eq!(fake.requests(), vec![0xD9; STEP_TRIES as usize]);
        }

        #[test]
        fn module_frames_in_between_are_acknowledged_and_skipped() {
            // A log line in the middle of the update must not stall the
            // module's queue (it repeats unacknowledged frames).
            let mut fake = Fake::new(|f: &Frame| {
                let mut out = reply(0xF0, b"\x01app_ota_start");
                out.extend(willing(f));
                out
            });
            let (result, _) = run(&mut fake, &dongle_image(), Target::Dongle);
            assert_eq!(result, Ok(()));
            assert!(fake.sent.iter().any(|f| f.prefix == Prefix::Ack && f.cmd == 0xF0));
        }
    }
}
