//! The Ogg page layer (RFC 3533) for Ogg Opus: pages of whole packets out,
//! pages and packets back in, resynchronising over damage, and the page
//! lookups seeking and crash recovery need.

use std::io::{self, BufReader, Read, Seek, SeekFrom};

pub(crate) const FLAG_CONTINUED: u8 = 0x01;
pub(crate) const FLAG_BOS: u8 = 0x02;
pub(crate) const FLAG_EOS: u8 = 0x04;
/// Granule position of a page on which no packet ends.
pub(crate) const NO_GRANULE: u64 = u64::MAX;
const HEADER_LEN: usize = 27;
const CAPTURE: &[u8; 4] = b"OggS";

static CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = (index as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 0x8000_0000 != 0 {
                (value << 1) ^ 0x04C1_1DB7
            } else {
                value << 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    data.iter().fold(crc, |crc, &byte| {
        (crc << 8) ^ CRC_TABLE[((crc >> 24) as u8 ^ byte) as usize]
    })
}

/// The Ogg page checksum: CRC-32, polynomial 0x04C11DB7, no reflection,
/// initial value and final XOR 0.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

/// Lacing values a packet of `len` bytes takes.
pub(crate) fn lacing_len(len: usize) -> usize {
    len / 255 + 1
}

/// Builds the pages of one logical stream.
pub(crate) struct PageWriter {
    serial: u32,
    sequence: u32,
}

impl PageWriter {
    pub(crate) fn new(serial: u32) -> Self {
        Self {
            serial,
            sequence: 0,
        }
    }

    /// One page holding whole `packets` (at most 255 lacing values).
    pub(crate) fn page(&mut self, flags: u8, granule: u64, packets: &[&[u8]]) -> Vec<u8> {
        let mut lacing = Vec::with_capacity(packets.len());
        for packet in packets {
            lacing.extend(std::iter::repeat_n(255u8, packet.len() / 255));
            lacing.push((packet.len() % 255) as u8);
        }
        assert!(
            lacing.len() <= 255,
            "{} lacing values on one page",
            lacing.len()
        );
        let body: usize = packets.iter().map(|packet| packet.len()).sum();
        let mut page = Vec::with_capacity(HEADER_LEN + lacing.len() + body);
        page.extend_from_slice(CAPTURE);
        page.push(0);
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(lacing.len() as u8);
        page.extend_from_slice(&lacing);
        for packet in packets {
            page.extend_from_slice(packet);
        }
        let crc = crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.sequence = self.sequence.wrapping_add(1);
        page
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Page {
    pub(crate) offset: u64,
    pub(crate) flags: u8,
    pub(crate) granule: u64,
    pub(crate) serial: u32,
    pub(crate) sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
}

impl Page {
    pub(crate) fn len(&self) -> u64 {
        (HEADER_LEN + self.lacing.len() + self.body.len()) as u64
    }

    pub(crate) fn end(&self) -> u64 {
        self.offset + self.len()
    }

    /// The packet pieces on this page in order, each with whether it ends
    /// here. With [`FLAG_CONTINUED`] the first piece finishes a packet from
    /// the page before.
    pub(crate) fn pieces(&self) -> Vec<(&[u8], bool)> {
        let mut pieces = Vec::new();
        let (mut start, mut at, mut open) = (0usize, 0usize, false);
        for &value in &self.lacing {
            at += usize::from(value);
            open = value == 255;
            if !open {
                pieces.push((&self.body[start..at], true));
                start = at;
            }
        }
        if open {
            pieces.push((&self.body[start..at], false));
        }
        pieces
    }
}

/// The whole, valid (CRC-checked) page at the start of `buf`.
pub(crate) fn parse_page(buf: &[u8], offset: u64) -> Option<Page> {
    if buf.len() < HEADER_LEN || &buf[..4] != CAPTURE || buf[4] != 0 {
        return None;
    }
    let segments = usize::from(buf[26]);
    let lacing = buf.get(HEADER_LEN..HEADER_LEN + segments)?;
    let body_len: usize = lacing.iter().map(|&value| usize::from(value)).sum();
    let total = HEADER_LEN + segments + body_len;
    let page = buf.get(..total)?;
    let stored = u32::from_le_bytes(page[22..26].try_into().ok()?);
    let crc = crc32_update(
        crc32_update(crc32_update(0, &page[..22]), &[0; 4]),
        &page[26..],
    );
    if crc != stored {
        return None;
    }
    Some(Page {
        offset,
        flags: page[5],
        granule: u64::from_le_bytes(page[6..14].try_into().ok()?),
        serial: u32::from_le_bytes(page[14..18].try_into().ok()?),
        sequence: u32::from_le_bytes(page[18..22].try_into().ok()?),
        lacing: lacing.to_vec(),
        body: page[HEADER_LEN + segments..].to_vec(),
    })
}

/// Bytes read at a time when scanning for a capture pattern.
const SCAN_CHUNK: usize = 64 * 1024;
/// The first window [`OggFile::last_granule_page`] reads from the end.
const TAIL_WINDOW: u64 = 64 * 1024;

/// Page access to an Ogg file.
pub(crate) struct OggFile<R> {
    inner: BufReader<R>,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> OggFile<R> {
    pub(crate) fn new(mut inner: R) -> io::Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(0))?;
        Ok(Self {
            inner: BufReader::with_capacity(16 * 1024, inner),
            pos: 0,
            len,
        })
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    fn seek_to(&mut self, pos: u64) -> io::Result<()> {
        if pos != self.pos {
            self.inner.seek(SeekFrom::Start(pos))?;
            self.pos = pos;
        }
        Ok(())
    }

    /// Up to `n` bytes from the current position; fewer at the end.
    fn read_up_to(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(n);
        (&mut self.inner).take(n as u64).read_to_end(&mut buf)?;
        self.pos += buf.len() as u64;
        Ok(buf)
    }

    /// The valid page starting exactly at `offset`.
    pub(crate) fn page_at(&mut self, offset: u64) -> io::Result<Option<Page>> {
        self.seek_to(offset)?;
        let mut buf = self.read_up_to(HEADER_LEN)?;
        if buf.len() < HEADER_LEN || &buf[..4] != CAPTURE {
            return Ok(None);
        }
        let segments = usize::from(buf[26]);
        buf.extend(self.read_up_to(segments)?);
        if buf.len() < HEADER_LEN + segments {
            return Ok(None);
        }
        let body: usize = buf[HEADER_LEN..]
            .iter()
            .map(|&value| usize::from(value))
            .sum();
        buf.extend(self.read_up_to(body)?);
        Ok(parse_page(&buf, offset))
    }

    /// The first valid page starting at or after `offset` and before `limit`.
    pub(crate) fn next_page(&mut self, offset: u64, limit: u64) -> io::Result<Option<Page>> {
        let limit = limit.min(self.len);
        let mut at = offset;
        while at < limit {
            let Some(candidate) = self.find_capture(at, limit)? else {
                return Ok(None);
            };
            if let Some(page) = self.page_at(candidate)? {
                return Ok(Some(page));
            }
            at = candidate + 1;
        }
        Ok(None)
    }

    /// The first capture pattern starting in `from..limit`.
    fn find_capture(&mut self, from: u64, limit: u64) -> io::Result<Option<u64>> {
        let mut at = from;
        while at < limit {
            self.seek_to(at)?;
            // Read 3 bytes past the limit so a pattern starting just before it is seen.
            let want = (limit - at + 3).min(SCAN_CHUNK as u64) as usize;
            let chunk = self.read_up_to(want)?;
            if chunk.len() < CAPTURE.len() {
                return Ok(None);
            }
            if let Some(index) = chunk.windows(4).position(|window| window == CAPTURE) {
                let found = at + index as u64;
                return Ok((found < limit).then_some(found));
            }
            at += (chunk.len() - 3) as u64;
        }
        Ok(None)
    }

    /// The last valid page with a granule position that starts at or after
    /// `floor`, scanning back from the end of the file. A truncated or
    /// damaged tail (a crash mid-page) is skipped.
    pub(crate) fn last_granule_page(&mut self, floor: u64) -> io::Result<Option<Page>> {
        let mut window = TAIL_WINDOW;
        loop {
            let start = self.len.saturating_sub(window).max(floor);
            self.seek_to(start)?;
            let buf = self.read_up_to((self.len.saturating_sub(start)) as usize)?;
            for index in (0..buf.len().saturating_sub(3)).rev() {
                if &buf[index..index + 4] != CAPTURE {
                    continue;
                }
                if let Some(page) = parse_page(&buf[index..], start + index as u64) {
                    if page.granule != NO_GRANULE {
                        return Ok(Some(page));
                    }
                }
            }
            if start == floor || start == 0 {
                return Ok(None);
            }
            window = window.saturating_mul(4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn crc_is_the_ogg_polynomial_without_reflection() {
        // CRC-32/CKSUM's check value without its final inversion.
        assert_eq!(crc32(b"123456789"), 0x765E_7680 ^ 0xFFFF_FFFF);
        assert_eq!(crc32(b""), 0);
    }

    fn sample_packets() -> Vec<Vec<u8>> {
        vec![vec![7u8; 300], vec![1, 2, 3], vec![9u8; 255], Vec::new()]
    }

    #[test]
    fn a_page_round_trips_with_its_lacing() {
        let packets = sample_packets();
        let refs: Vec<&[u8]> = packets.iter().map(Vec::as_slice).collect();
        let mut writer = PageWriter::new(0x1234_5678);
        let bytes = writer.page(FLAG_BOS, 960, &refs);
        // 300 → 255,45; 3 → 3; 255 → 255,0; 0 → 0.
        assert_eq!(bytes[26], 6, "lacing values");
        let page = parse_page(&bytes, 0).expect("valid page");
        assert_eq!(page.len(), bytes.len() as u64);
        assert_eq!(page.flags, FLAG_BOS);
        assert_eq!(page.granule, 960);
        assert_eq!(page.serial, 0x1234_5678);
        assert_eq!(page.sequence, 0);
        let pieces = page.pieces();
        assert_eq!(pieces.len(), 4);
        for (piece, packet) in pieces.iter().zip(&packets) {
            assert_eq!(piece.0, packet.as_slice());
            assert!(piece.1, "whole packets only");
        }
        let second = writer.page(0, 1_920, &[&[5u8; 10]]);
        assert_eq!(parse_page(&second, 0).unwrap().sequence, 1);
    }

    #[test]
    fn a_damaged_or_truncated_page_is_rejected() {
        let bytes = PageWriter::new(1).page(0, 0, &[&[1u8; 100]]);
        let mut damaged = bytes.clone();
        damaged[60] ^= 0x40;
        assert_eq!(parse_page(&damaged, 0), None, "CRC mismatch");
        assert_eq!(parse_page(&bytes[..bytes.len() - 1], 0), None, "cut short");
        assert_eq!(parse_page(&bytes[1..], 0), None, "no capture pattern");
    }

    #[test]
    fn a_page_ending_on_a_full_lacing_value_continues() {
        // Built by hand: one 255-byte piece that does not end on this page.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OggS");
        bytes.extend_from_slice(&[0, FLAG_CONTINUED]);
        bytes.extend_from_slice(&NO_GRANULE.to_le_bytes());
        bytes.extend_from_slice(&[0; 12]);
        bytes.push(1);
        bytes.push(255);
        bytes.extend_from_slice(&[3u8; 255]);
        let crc = crc32(&bytes);
        bytes[22..26].copy_from_slice(&crc.to_le_bytes());
        let page = parse_page(&bytes, 0).unwrap();
        assert_eq!(page.pieces(), vec![(&[3u8; 255][..], false)]);
    }

    fn stream(pages: usize) -> Vec<u8> {
        let mut writer = PageWriter::new(9);
        let mut bytes = Vec::new();
        for index in 0..pages {
            let granule = (index as u64 + 1) * 960;
            bytes.extend(writer.page(0, granule, &[&[index as u8; 200]]));
        }
        bytes
    }

    #[test]
    fn pages_are_found_past_garbage_and_before_a_limit() {
        let mut bytes = b"garbage OggS not a page".to_vec();
        let start = bytes.len() as u64;
        bytes.extend(stream(3));
        let mut file = OggFile::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(file.len(), bytes.len() as u64);
        assert_eq!(file.page_at(0).unwrap(), None);
        let first = file.next_page(0, file.len()).unwrap().unwrap();
        assert_eq!(first.offset, start);
        assert_eq!(first.granule, 960);
        let second = file.page_at(first.end()).unwrap().unwrap();
        assert_eq!(second.granule, 1_920);
        assert_eq!(
            file.next_page(first.offset + 1, file.len()).unwrap(),
            Some(second.clone())
        );
        assert_eq!(
            file.next_page(first.offset + 1, second.offset).unwrap(),
            None
        );
    }

    #[test]
    fn the_last_granule_skips_a_cut_tail() {
        let mut bytes = stream(5);
        let whole = bytes.len();
        let last_page_len = whole / 5;
        let mut file = OggFile::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(file.last_granule_page(0).unwrap().unwrap().granule, 5 * 960);
        bytes.truncate(whole - last_page_len / 2);
        let mut cut = OggFile::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(cut.last_granule_page(0).unwrap().unwrap().granule, 4 * 960);
        bytes.extend_from_slice(&[0xAB; 100_000]);
        let mut garbage = OggFile::new(Cursor::new(bytes)).unwrap();
        assert_eq!(
            garbage.last_granule_page(0).unwrap().unwrap().granule,
            4 * 960,
            "found beyond the first tail window"
        );
        let mut empty = OggFile::new(Cursor::new(Vec::new())).unwrap();
        assert_eq!(empty.last_granule_page(0).unwrap(), None);
    }

    #[test]
    fn the_floor_hides_earlier_pages() {
        let bytes = stream(2);
        let mut file = OggFile::new(Cursor::new(bytes.clone())).unwrap();
        let second = file.next_page(1, file.len()).unwrap().unwrap();
        assert_eq!(file.last_granule_page(second.end()).unwrap(), None);
        assert_eq!(
            file.last_granule_page(second.offset)
                .unwrap()
                .map(|page| page.granule),
            Some(1_920)
        );
    }
}
