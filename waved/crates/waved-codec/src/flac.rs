//! FLAC, 16-bit mono 48 kHz, fixed 4096-sample blocks.

use crate::reader::PcmReader;
use crate::writer::RecordingWriter;
use crate::AudioFormat;
use claxon::frame::FrameReader;
use claxon::input::BufferedReader;
use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, StreamInfo};
use flacenc::error::{Verified, Verify};
use flacenc::source::{Fill, FrameBuf};
use md5::{Digest, Md5};
use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom, Write};

/// Samples per FLAC frame (85 ms), the reference encoder's default.
pub(crate) const BLOCK_SAMPLES: usize = 4_096;

/// Whole frames between two pushes to the file: about one second.
const FRAMES_PER_FLUSH: usize = 12;
/// "fLaC" and the STREAMINFO block header come first.
const STREAMINFO_OFFSET: u64 = 8;
const STREAMINFO_LEN: usize = 34;
/// Bytes scanned at a time for frame headers.
const SCAN_CHUNK: usize = 64 * 1024;
/// The longest frame header: sync, codes, a 7-byte frame number, a 16-bit
/// block size, a 16-bit sample rate and the CRC-8.
const MAX_HEADER_LEN: usize = 16;
/// How far back from the end a cut file's last frame header is looked for.
const TAIL_WINDOW: u64 = 256 * 1024;
/// The smallest frame: a constant block (header, one subframe, CRC-16).
const MIN_FRAME_BYTES: u64 = 10;
/// The largest frame: a verbatim block of 4096 16-bit samples plus headers.
const MAX_FRAME_BYTES: u64 = 4_096 * 2 + 64;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// STREAMINFO for our layout: fixed 4096-sample blocks, 48 kHz, mono,
/// 16-bit. Zero frame sizes, total and MD5 mean "unknown".
fn streaminfo(min_frame: u32, max_frame: u32, total: u64, md5: [u8; 16]) -> [u8; STREAMINFO_LEN] {
    let mut info = [0u8; STREAMINFO_LEN];
    let block = BLOCK_SAMPLES as u16;
    info[0..2].copy_from_slice(&block.to_be_bytes());
    info[2..4].copy_from_slice(&block.to_be_bytes());
    info[4..7].copy_from_slice(&min_frame.to_be_bytes()[1..]);
    info[7..10].copy_from_slice(&max_frame.to_be_bytes()[1..]);
    // Sample rate (20 bits), channels − 1 (3 bits, 0 = mono), bits per
    // sample − 1 (5 bits), total samples (36 bits).
    const MONO: u64 = 0;
    const BITS_PER_SAMPLE_MINUS_ONE: u64 = 15;
    let packed = (u64::from(crate::SAMPLE_RATE) << 44)
        | (MONO << 41)
        | (BITS_PER_SAMPLE_MINUS_ONE << 36)
        | (total & 0xF_FFFF_FFFF);
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    info[18..34].copy_from_slice(&md5);
    info
}

/// Encodes 4096-sample frames as they fill and writes them at once.
///
/// STREAMINFO goes out first with an unknown total; [`RecordingWriter::finish`]
/// patches in the total, the frame sizes and the MD5 of the samples.
pub(crate) struct FlacWriter<W: Write + Seek> {
    out: W,
    config: Verified<flacenc::config::Encoder>,
    info: StreamInfo,
    framebuf: FrameBuf,
    block: Vec<i32>,
    frames: usize,
    total: u64,
    min_frame: u32,
    max_frame: u32,
    md5: Md5,
    sink: ByteSink,
    unflushed: usize,
    finished: bool,
    bytes: Vec<u8>,
}

impl<W: Write + Seek> FlacWriter<W> {
    pub(crate) fn new(mut out: W) -> io::Result<Self> {
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|(_, error)| io::Error::other(format!("flac config: {error:?}")))?;
        let info = StreamInfo::new(crate::SAMPLE_RATE as usize, 1, 16)
            .map_err(|error| io::Error::other(format!("flac stream info: {error:?}")))?;
        let framebuf = FrameBuf::with_size(1, BLOCK_SAMPLES)
            .map_err(|error| io::Error::other(format!("flac frame buffer: {error:?}")))?;
        out.write_all(b"fLaC")?;
        // Last metadata block, type 0 (STREAMINFO), 34 bytes.
        out.write_all(&[0x80, 0, 0, STREAMINFO_LEN as u8])?;
        out.write_all(&streaminfo(0, 0, 0, [0; 16]))?;
        out.flush()?;
        Ok(Self {
            out,
            config,
            info,
            framebuf,
            block: Vec::with_capacity(BLOCK_SAMPLES),
            frames: 0,
            total: 0,
            min_frame: u32::MAX,
            max_frame: 0,
            md5: Md5::new(),
            sink: ByteSink::new(),
            unflushed: 0,
            finished: false,
            bytes: Vec::new(),
        })
    }

    /// Encode `block` (a whole block, or the shorter last one) as the next frame.
    fn encode_block(&mut self) -> io::Result<()> {
        self.framebuf
            .fill_interleaved(&self.block)
            .map_err(|error| io::Error::other(format!("flac input: {error:?}")))?;
        let frame =
            flacenc::encode_fixed_size_frame(&self.config, &self.framebuf, self.frames, &self.info)
                .map_err(|error| io::Error::other(format!("flac encode: {error:?}")))?;
        self.sink.clear();
        frame
            .write(&mut self.sink)
            .map_err(|_| io::Error::other("flac: frame does not fit the bitstream"))?;
        let bytes = self.sink.as_slice();
        self.out.write_all(bytes)?;
        let size = bytes.len() as u32;
        self.min_frame = self.min_frame.min(size);
        self.max_frame = self.max_frame.max(size);
        self.frames += 1;
        self.block.clear();
        self.unflushed += 1;
        if self.unflushed >= FRAMES_PER_FLUSH {
            self.out.flush()?;
            self.unflushed = 0;
        }
        Ok(())
    }
}

impl<W: Write + Seek + Send> RecordingWriter for FlacWriter<W> {
    fn format(&self) -> AudioFormat {
        AudioFormat::Flac
    }

    fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::other("flac stream already finished"));
        }
        self.bytes.clear();
        self.bytes
            .extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
        self.md5.update(&self.bytes);
        self.total += samples.len() as u64;
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (BLOCK_SAMPLES - self.block.len()).min(rest.len());
            self.block
                .extend(rest[..take].iter().map(|&sample| i32::from(sample)));
            rest = &rest[take..];
            if self.block.len() == BLOCK_SAMPLES {
                self.encode_block()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.unflushed = 0;
        self.out.flush()
    }

    fn finish(&mut self) -> io::Result<u64> {
        if self.finished {
            return Ok(self.total);
        }
        if !self.block.is_empty() {
            self.encode_block()?;
        }
        let (min_frame, max_frame) = if self.frames == 0 {
            (0, 0)
        } else {
            (self.min_frame, self.max_frame)
        };
        let md5: [u8; 16] = self.md5.clone().finalize().into();
        self.out.seek(SeekFrom::Start(STREAMINFO_OFFSET))?;
        self.out
            .write_all(&streaminfo(min_frame, max_frame, self.total, md5))?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        self.finished = true;
        Ok(self.total)
    }
}

/// FLAC frame header CRC-8, polynomial 0x07.
fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |crc, &byte| {
        let mut crc = crc ^ byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
        crc
    })
}

/// The frame number of a fixed-blocksize, mono, 16-bit, 48 kHz frame header
/// at the start of `buf` (CRC-8 checked). Anything else is no header.
fn frame_number(buf: &[u8]) -> Option<u64> {
    if buf.len() < 6 || buf[0] != 0xFF || buf[1] != 0xF8 {
        return None;
    }
    let (block_code, rate_code) = (buf[2] >> 4, buf[2] & 0x0F);
    let (channels, size_code, reserved) = (buf[3] >> 4, (buf[3] >> 1) & 0x07, buf[3] & 1);
    if block_code == 0
        || !matches!(rate_code, 0 | 10)
        || channels != 0
        || !matches!(size_code, 0 | 4)
    {
        return None;
    }
    if reserved != 0 {
        return None;
    }
    // The frame number, UTF-8 style.
    let first = buf[4];
    let (mut value, extra) = match first.leading_ones() {
        0 => (u64::from(first), 0),
        n @ 2..=7 => (u64::from(first & (0x7F >> n)), n as usize - 1),
        _ => return None,
    };
    let mut at = 5;
    for _ in 0..extra {
        let byte = *buf.get(at)?;
        if byte & 0xC0 != 0x80 {
            return None;
        }
        value = (value << 6) | u64::from(byte & 0x3F);
        at += 1;
    }
    at += match block_code {
        6 => 1,
        7 => 2,
        _ => 0,
    };
    let crc = *buf.get(at)?;
    (crc8(&buf[..at]) == crc).then_some(value)
}

/// Decodes a FLAC recording, seeking by bisection over frame numbers.
pub(crate) struct FlacReader<R: Read + Seek> {
    /// The file while no frame reader holds it.
    file: Option<R>,
    frames: Option<FrameReader<BufferedReader<R>>>,
    len: u64,
    frames_start: u64,
    total: u64,
    decoded: VecDeque<i16>,
    skip: u64,
    position: u64,
    buffer: Vec<i32>,
}

impl<R: Read + Seek> FlacReader<R> {
    pub(crate) fn new(mut inner: R) -> io::Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(0))?;
        let mut magic = [0u8; 4];
        inner.read_exact(&mut magic)?;
        if &magic != b"fLaC" {
            return Err(invalid("not a FLAC file"));
        }
        let mut info = None;
        loop {
            let mut header = [0u8; 4];
            inner.read_exact(&mut header)?;
            let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
            let mut block = vec![0u8; length];
            inner.read_exact(&mut block)?;
            if header[0] & 0x7F == 0 && length == STREAMINFO_LEN {
                info = Some(block);
            }
            if header[0] & 0x80 != 0 {
                break;
            }
        }
        let info = info.ok_or_else(|| invalid("FLAC file without STREAMINFO"))?;
        let min_block = u16::from_be_bytes([info[0], info[1]]) as usize;
        let max_block = u16::from_be_bytes([info[2], info[3]]) as usize;
        let packed = u64::from_be_bytes(info[10..18].try_into().expect("8 bytes"));
        let (rate, channels, bits) = (
            packed >> 44,
            ((packed >> 41) & 7) + 1,
            ((packed >> 36) & 31) + 1,
        );
        if (min_block, max_block, rate, channels, bits)
            != (BLOCK_SAMPLES, BLOCK_SAMPLES, 48_000, 1, 16)
        {
            return Err(invalid(
                "only 48 kHz mono 16-bit FLAC with 4096-sample blocks is supported",
            ));
        }
        let frames_start = inner.stream_position()?;
        let mut reader = Self {
            file: Some(inner),
            frames: None,
            len,
            frames_start,
            total: packed & 0xF_FFFF_FFFF,
            decoded: VecDeque::new(),
            skip: 0,
            position: 0,
            buffer: Vec::new(),
        };
        if reader.total == 0 {
            // Unknown: a take cut by a crash. Count the whole frames.
            reader.total = reader.count_samples()?;
        }
        reader.start_at(frames_start, 0)?;
        Ok(reader)
    }

    /// Take the file back from the frame reader, if it has it.
    fn file(&mut self) -> &mut R {
        if let Some(frames) = self.frames.take() {
            self.file = Some(frames.into_inner().into_inner());
        }
        self.file.as_mut().expect("the file is held by one side")
    }

    /// Decode from the frame at `offset`, dropping `skip` samples first.
    fn start_at(&mut self, offset: u64, skip: u64) -> io::Result<()> {
        let file = self.file();
        file.seek(SeekFrom::Start(offset))?;
        let file = self.file.take().expect("just taken back");
        self.frames = Some(FrameReader::new(BufferedReader::new(file)));
        self.decoded.clear();
        self.skip = skip;
        Ok(())
    }

    /// Whether frame `number` can start at `offset`: every frame before it
    /// takes between a constant block's and a verbatim block's bytes.
    fn plausible(&self, offset: u64, number: u64) -> bool {
        let span = offset - self.frames_start;
        number.saturating_mul(MIN_FRAME_BYTES) <= span
            && span <= number.saturating_mul(MAX_FRAME_BYTES)
    }

    /// The first frame header in `from..limit`: (offset, frame number).
    fn frame_at_or_after(&mut self, from: u64, limit: u64) -> io::Result<Option<(u64, u64)>> {
        let limit = limit.min(self.len);
        let mut at = from;
        while at < limit {
            let file = self.file();
            file.seek(SeekFrom::Start(at))?;
            let want = (limit - at + MAX_HEADER_LEN as u64).min(SCAN_CHUNK as u64);
            let mut chunk = Vec::with_capacity(want as usize);
            file.take(want).read_to_end(&mut chunk)?;
            if chunk.len() < 2 {
                return Ok(None);
            }
            let starts = chunk.len().saturating_sub(MAX_HEADER_LEN).max(1);
            for index in 0..starts {
                if at + index as u64 >= limit {
                    return Ok(None);
                }
                let offset = at + index as u64;
                match frame_number(&chunk[index..]) {
                    Some(number) if self.plausible(offset, number) => {
                        return Ok(Some((offset, number)));
                    }
                    _ => {}
                }
            }
            if chunk.len() < want as usize {
                return Ok(None);
            }
            at += starts as u64;
        }
        Ok(None)
    }

    /// Offset and number of the last frame numbered at most `target`.
    fn frame_before(&mut self, target: u64) -> io::Result<Option<(u64, u64)>> {
        let (mut low, mut high) = (self.frames_start, self.len);
        let mut best = None;
        while low < high {
            let middle = low + (high - low) / 2;
            match self.frame_at_or_after(middle, high)? {
                Some((offset, number)) if number <= target => {
                    best = Some((offset, number));
                    if number == target {
                        break;
                    }
                    low = offset + 1;
                }
                _ => high = middle,
            }
        }
        Ok(best)
    }

    /// The block length of the frame at `offset`, when it decodes whole
    /// (CRC-16 checked).
    fn whole_frame_at(&mut self, offset: u64) -> io::Result<Option<u32>> {
        self.start_at(offset, 0)?;
        let frames = self.frames.as_mut().expect("started");
        Ok(match frames.read_next_or_eof(Vec::new()) {
            Ok(Some(block)) => Some(block.duration()),
            _ => None,
        })
    }

    /// Samples in whole frames, for a file whose STREAMINFO has no total
    /// (a take cut by a crash): the frames before the last one, plus the
    /// last one if it is whole. A header-like pattern inside frame data is
    /// told apart because it does not decode or does not follow on.
    fn count_samples(&mut self) -> io::Result<u64> {
        let floor = self.len.saturating_sub(TAIL_WINDOW).max(self.frames_start);
        let mut candidates = Vec::new();
        let mut at = floor;
        while let Some((offset, number)) = self.frame_at_or_after(at, self.len)? {
            candidates.push((offset, number));
            at = offset + 1;
        }
        for index in (0..candidates.len()).rev() {
            let (offset, number) = candidates[index];
            let start = number * BLOCK_SAMPLES as u64;
            if let Some(duration) = self.whole_frame_at(offset)? {
                return Ok(start + u64::from(duration));
            }
            if index > 0 {
                let (before, previous) = candidates[index - 1];
                if previous + 1 == number && self.whole_frame_at(before)?.is_some() {
                    return Ok(start);
                }
            }
        }
        Ok(0)
    }
}

impl<R: Read + Seek + Send> PcmReader for FlacReader<R> {
    fn format(&self) -> AudioFormat {
        AudioFormat::Flac
    }

    fn total_samples(&self) -> u64 {
        self.total
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn seek(&mut self, sample: u64) -> io::Result<()> {
        let target = sample.min(self.total);
        let wanted = target / BLOCK_SAMPLES as u64;
        let (offset, number) = if wanted == 0 {
            (self.frames_start, 0)
        } else {
            self.frame_before(wanted)?.unwrap_or((self.frames_start, 0))
        };
        self.start_at(offset, target - number * BLOCK_SAMPLES as u64)?;
        self.position = target;
        Ok(())
    }

    fn read(&mut self, out: &mut [i16]) -> io::Result<usize> {
        let mut filled = 0;
        while filled < out.len() && self.position < self.total {
            if let Some(sample) = self.decoded.pop_front() {
                out[filled] = sample;
                filled += 1;
                self.position += 1;
                continue;
            }
            let Some(frames) = self.frames.as_mut() else {
                break;
            };
            let buffer = std::mem::take(&mut self.buffer);
            // A damaged or cut frame ends playback where it starts.
            let Ok(Some(block)) = frames.read_next_or_eof(buffer) else {
                break;
            };
            let samples = block.channel(0);
            let drop = self.skip.min(samples.len() as u64) as usize;
            self.skip -= drop as u64;
            self.decoded
                .extend(samples[drop..].iter().map(|&sample| sample as i16));
            self.buffer = block.into_buffer();
        }
        Ok(filled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_to_end;
    use crate::testsignal::speech_like;

    use std::io::Cursor;

    fn encode(samples: &[i16]) -> Vec<u8> {
        let mut writer = FlacWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.write(samples).unwrap();
        assert_eq!(writer.finish().unwrap(), samples.len() as u64);
        assert_eq!(
            writer.finish().unwrap(),
            samples.len() as u64,
            "finish twice"
        );
        writer.out.into_inner()
    }

    fn md5_of(samples: &[i16]) -> [u8; 16] {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        Md5::digest(&bytes).into()
    }

    #[test]
    fn a_reference_decoder_reads_back_every_sample() {
        for len in [0usize, 1, 4_095, 4_096, 4_097, 48_000 * 3 + 17] {
            let samples = speech_like(len as f64 / 48_000.0, 11);
            let bytes = encode(&samples);
            let mut reader = claxon::FlacReader::new(Cursor::new(bytes)).unwrap();
            let info = reader.streaminfo();
            assert_eq!(info.sample_rate, 48_000);
            assert_eq!(info.channels, 1);
            assert_eq!(info.bits_per_sample, 16);
            assert_eq!(info.min_block_size, 4_096);
            assert_eq!(info.max_block_size, 4_096);
            assert_eq!(
                info.samples,
                (len > 0).then_some(len as u64),
                "length {len}"
            );
            assert_eq!(info.md5sum, md5_of(&samples), "length {len}");
            let decoded: Vec<i16> = reader
                .samples()
                .map(|sample| sample.unwrap() as i16)
                .collect();
            assert_eq!(decoded, samples, "length {len}");
        }
    }

    #[test]
    fn our_reader_is_bit_exact() {
        let samples = speech_like(2.0, 12);
        let bytes = encode(&samples);
        let mut reader = FlacReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.total_samples(), samples.len() as u64);
        assert_eq!(read_to_end(&mut reader).unwrap(), samples);
    }

    #[test]
    fn frames_reach_the_file_before_finish_with_the_total_still_unknown() {
        let samples = speech_like(2.2, 13);
        let mut writer = FlacWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.write(&samples).unwrap();
        // 25 whole frames are encoded; at least the first 12 (one second)
        // were pushed out already, all of them after a flush.
        writer.flush().unwrap();
        let bytes = writer.out.get_ref().clone();
        let reader = claxon::FlacReader::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(
            reader.streaminfo().samples,
            None,
            "0 = unknown until finish"
        );
        let mut ours = FlacReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(ours.total_samples(), 25 * 4_096);
        assert_eq!(read_to_end(&mut ours).unwrap(), samples[..25 * 4_096]);
    }

    #[test]
    fn a_file_cut_mid_frame_keeps_every_whole_frame() {
        let samples = speech_like(2.2, 14);
        let mut writer = FlacWriter::new(Cursor::new(Vec::new())).unwrap();
        writer.write(&samples).unwrap();
        writer.flush().unwrap();
        let mut bytes = writer.out.get_ref().clone();
        bytes.truncate(bytes.len() - 700);
        let mut reader = FlacReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.total_samples(), 24 * 4_096);
        assert_eq!(read_to_end(&mut reader).unwrap(), samples[..24 * 4_096]);
    }

    #[test]
    fn seeking_is_sample_exact() {
        let samples = speech_like(5.0, 15);
        let bytes = encode(&samples);
        let mut reader = FlacReader::new(Cursor::new(bytes)).unwrap();
        for target in [0u64, 1, 4_095, 4_096, 100_000, 200_123, 239_000] {
            reader.seek(target).unwrap();
            assert_eq!(reader.position(), target);
            let mut chunk = vec![0i16; 2_000];
            let read = reader.read(&mut chunk).unwrap();
            let end = (target as usize + 2_000).min(samples.len());
            assert_eq!(
                &chunk[..read],
                &samples[target as usize..end],
                "seek {target}"
            );
        }
        reader.seek(u64::MAX).unwrap();
        assert_eq!(reader.position(), samples.len() as u64);
        assert_eq!(reader.read(&mut [0i16; 4]).unwrap(), 0);
    }

    #[test]
    fn our_reader_refuses_other_layouts() {
        let mut bytes = encode(&speech_like(0.2, 16));
        // STREAMINFO byte 12 holds the channel count: make it stereo.
        bytes[8 + 12] |= 0b0000_0010;
        assert_eq!(
            FlacReader::new(Cursor::new(bytes))
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::InvalidData)
        );
    }
}
