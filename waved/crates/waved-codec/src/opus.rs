//! Ogg Opus (RFC 7845), 48 kHz mono.

use crate::ogg::{
    lacing_len, OggFile, Page, PageWriter, FLAG_BOS, FLAG_CONTINUED, FLAG_EOS, NO_GRANULE,
};
use crate::reader::PcmReader;
use crate::writer::RecordingWriter;
use crate::AudioFormat;
use opusic_c::{Application, Bitrate, Channels, Decoder, Encoder, ErrorCode, SampleRate};
use std::collections::VecDeque;
use std::io::{self, Read, Seek, Write};

/// 32 kbps: fullband speech that transcribes as well as the WAV did, at
/// ~14.5 MB per hour.
pub const OPUS_BITRATE_BPS: u32 = 32_000;
/// The encoder's best; one A55 core still encodes an hour in minutes
/// (docs/AUDIO_HARDWARE.md "Recording formats").
pub const OPUS_COMPLEXITY: u8 = 10;
/// 20 ms frames.
pub const OPUS_FRAME_SAMPLES: usize = 960;

/// Packets per Ogg page: a page reaches the file every second of audio.
const PACKETS_PER_PAGE: usize = 50;
/// RFC 7845 §4.6: start decoding at least 80 ms before a seek target.
const SEEK_PREROLL: u64 = 3_840;
/// Room for one encoded packet; 20 ms at 32 kbps is ~80 bytes.
const MAX_PACKET_BYTES: usize = 4_000;
/// Decoder output room: the longest Opus packet, 120 ms.
const MAX_FRAME_SAMPLES: usize = 5_760;
/// The stream's Ogg serial number ("Wave"); a recording has one stream.
const SERIAL: u32 = 0x5761_7665;

fn opus_error(error: ErrorCode) -> io::Error {
    io::Error::other(format!("opus: {error:?}"))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// RFC 7845 §5.1, mono, channel mapping family 0.
fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(1);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&crate::SAMPLE_RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

/// RFC 7845 §5.2: the libopus version as vendor, and who wrote the file.
fn opus_tags() -> Vec<u8> {
    let vendor = opusic_c::version();
    let comment = concat!("ENCODER=waved ", env!("CARGO_PKG_VERSION"));
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor.as_bytes());
    tags.extend_from_slice(&1u32.to_le_bytes());
    tags.extend_from_slice(&(comment.len() as u32).to_le_bytes());
    tags.extend_from_slice(comment.as_bytes());
    tags
}

/// Encodes 20 ms packets onto Ogg pages of one second each.
///
/// The newest packet is always held back from the pages until
/// [`RecordingWriter::finish`]: the end-trimming granule of the last page
/// must exceed the page before it (RFC 7845 §4.4), so the last page needs a
/// packet with real audio on it.
pub(crate) struct OpusWriter<W: Write> {
    out: W,
    encoder: Encoder,
    pages: PageWriter,
    pre_skip: u64,
    /// Input not yet a whole frame.
    frame: Vec<i16>,
    /// Encoded packets not on a page yet; the newest is held back.
    queued: Vec<Vec<u8>>,
    packets_on_pages: u64,
    samples_in: u64,
    finished: bool,
    packet: Vec<u8>,
    pcm: Vec<u16>,
}

impl<W: Write> OpusWriter<W> {
    /// Configure the encoder (R-1 in the plan) and put the two header pages
    /// on the file.
    pub(crate) fn new(mut out: W) -> io::Result<Self> {
        let mut encoder = Encoder::new(Channels::Mono, SampleRate::Hz48000, Application::Audio)
            .map_err(opus_error)?;
        encoder
            .set_bitrate(Bitrate::Value(OPUS_BITRATE_BPS))
            .map_err(opus_error)?;
        encoder.set_vbr(true).map_err(opus_error)?;
        encoder.set_vbr_constraint(false).map_err(opus_error)?;
        encoder
            .set_complexity(OPUS_COMPLEXITY)
            .map_err(opus_error)?;
        let pre_skip = encoder.get_look_ahead().map_err(opus_error)?;
        let mut pages = PageWriter::new(SERIAL);
        out.write_all(&pages.page(FLAG_BOS, 0, &[&opus_head(pre_skip as u16)]))?;
        out.write_all(&pages.page(0, 0, &[&opus_tags()]))?;
        out.flush()?;
        Ok(Self {
            out,
            encoder,
            pages,
            pre_skip: u64::from(pre_skip),
            frame: Vec::with_capacity(OPUS_FRAME_SAMPLES),
            queued: Vec::with_capacity(PACKETS_PER_PAGE + 1),
            packets_on_pages: 0,
            samples_in: 0,
            finished: false,
            packet: vec![0; MAX_PACKET_BYTES],
            pcm: vec![0; OPUS_FRAME_SAMPLES],
        })
    }

    /// Encode the full `frame` buffer into one packet.
    fn encode_frame(&mut self) -> io::Result<()> {
        for (to, &from) in self.pcm.iter_mut().zip(&self.frame) {
            *to = from as u16;
        }
        let len = self
            .encoder
            .encode_to_slice(&self.pcm, &mut self.packet)
            .map_err(opus_error)?;
        self.queued.push(self.packet[..len].to_vec());
        self.frame.clear();
        if self.queued.len() > PACKETS_PER_PAGE {
            self.write_pages()?;
        }
        Ok(())
    }

    /// Put every queued packet but the newest on pages, then on the file.
    fn write_pages(&mut self) -> io::Result<()> {
        if self.queued.len() < 2 {
            return Ok(());
        }
        let held = self.queued.pop().expect("two packets queued");
        while !self.queued.is_empty() {
            let mut lacing = 0;
            let count = self
                .queued
                .iter()
                .take_while(|packet| {
                    lacing += lacing_len(packet.len());
                    lacing <= 255
                })
                .count()
                .max(1);
            let packets: Vec<Vec<u8>> = self.queued.drain(..count).collect();
            self.packets_on_pages += count as u64;
            let granule = self.packets_on_pages * OPUS_FRAME_SAMPLES as u64;
            let refs: Vec<&[u8]> = packets.iter().map(Vec::as_slice).collect();
            let page = self.pages.page(0, granule, &refs);
            self.out.write_all(&page)?;
        }
        self.queued.push(held);
        self.out.flush()
    }

    fn encoded_samples(&self) -> u64 {
        (self.packets_on_pages + self.queued.len() as u64) * OPUS_FRAME_SAMPLES as u64
    }
}

impl<W: Write + Send> RecordingWriter for OpusWriter<W> {
    fn format(&self) -> AudioFormat {
        AudioFormat::Opus
    }

    fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::other("opus stream already finished"));
        }
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (OPUS_FRAME_SAMPLES - self.frame.len()).min(rest.len());
            self.frame.extend_from_slice(&rest[..take]);
            self.samples_in += take as u64;
            rest = &rest[take..];
            if self.frame.len() == OPUS_FRAME_SAMPLES {
                self.encode_frame()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.write_pages()?;
        self.out.flush()
    }

    fn finish(&mut self) -> io::Result<u64> {
        if self.finished {
            return Ok(self.samples_in);
        }
        // Leave only the held packet queued, so the last page stays small.
        self.write_pages()?;
        // The decoder needs pre-skip more samples than were written before
        // the last real one comes out: pad with silence, then trim.
        while self.encoded_samples() < self.samples_in + self.pre_skip {
            self.frame.resize(OPUS_FRAME_SAMPLES, 0);
            self.encode_frame()?;
        }
        let packets: Vec<Vec<u8>> = self.queued.drain(..).collect();
        self.packets_on_pages += packets.len() as u64;
        let refs: Vec<&[u8]> = packets.iter().map(Vec::as_slice).collect();
        let page = self
            .pages
            .page(FLAG_EOS, self.pre_skip + self.samples_in, &refs);
        self.out.write_all(&page)?;
        self.out.flush()?;
        self.finished = true;
        Ok(self.samples_in)
    }
}

/// Decodes an Ogg Opus recording, seeking by bisection over the pages.
pub(crate) struct OpusReader<R> {
    file: OggFile<R>,
    decoder: Decoder,
    serial: u32,
    pre_skip: u64,
    /// Offset of the first audio page.
    data_start: u64,
    total: u64,
    next_page: u64,
    /// True right after a restart or resync: a continued first piece has
    /// lost its start.
    resumed: bool,
    packets: VecDeque<Vec<u8>>,
    partial: Option<Vec<u8>>,
    decoded: VecDeque<i16>,
    /// Decoded samples still to drop (pre-skip, seek pre-roll).
    skip: u64,
    position: u64,
    pcm: Vec<u16>,
}

impl<R: Read + Seek> OpusReader<R> {
    pub(crate) fn new(inner: R) -> io::Result<Self> {
        let mut file = OggFile::new(inner)?;
        let head = file.page_at(0)?.ok_or_else(|| invalid("not an Ogg file"))?;
        let packet = head
            .pieces()
            .first()
            .map(|piece| piece.0.to_vec())
            .unwrap_or_default();
        if head.flags & FLAG_BOS == 0 || packet.len() < 19 || &packet[..8] != b"OpusHead" {
            return Err(invalid("not an Ogg Opus file"));
        }
        if packet[8] >> 4 != 0 || packet[9] != 1 || packet[18] != 0 {
            return Err(invalid(
                "only mono Ogg Opus version 1 recordings are supported",
            ));
        }
        let pre_skip = u64::from(u16::from_le_bytes([packet[10], packet[11]]));
        // OpusTags may span pages: the audio starts after its last piece.
        let mut offset = head.end();
        loop {
            let page = file
                .page_at(offset)?
                .ok_or_else(|| invalid("Ogg Opus file without its comment header"))?;
            offset = page.end();
            if page.pieces().last().is_some_and(|piece| piece.1) {
                break;
            }
        }
        let total = file
            .last_granule_page(offset)?
            .map(|page| page.granule.saturating_sub(pre_skip))
            .unwrap_or(0);
        let decoder = Decoder::new(Channels::Mono, SampleRate::Hz48000).map_err(opus_error)?;
        let mut reader = Self {
            file,
            decoder,
            serial: head.serial,
            pre_skip,
            data_start: offset,
            total,
            next_page: offset,
            resumed: true,
            packets: VecDeque::new(),
            partial: None,
            decoded: VecDeque::new(),
            skip: 0,
            position: 0,
            pcm: vec![0; MAX_FRAME_SAMPLES],
        };
        reader.restart(offset, 0, 0)?;
        Ok(reader)
    }

    /// Decode from the page at `page`, whose first packet starts at
    /// `granule`, so that the next sample returned is `target`.
    fn restart(&mut self, page: u64, granule: u64, target: u64) -> io::Result<()> {
        self.decoder.reset().map_err(opus_error)?;
        self.packets.clear();
        self.partial = None;
        self.decoded.clear();
        self.next_page = page;
        self.resumed = true;
        self.skip = (target + self.pre_skip).saturating_sub(granule);
        self.position = target;
        Ok(())
    }

    /// The last audio page whose granule is at most `goal`.
    fn page_before(&mut self, goal: u64) -> io::Result<Option<Page>> {
        let (mut low, mut high) = (self.data_start, self.file.len());
        let mut best = None;
        while low < high {
            let middle = low + (high - low) / 2;
            match self.file.next_page(middle, high)? {
                Some(page)
                    if page.serial == self.serial
                        && page.granule != NO_GRANULE
                        && page.granule <= goal =>
                {
                    low = page.end();
                    best = Some(page);
                }
                _ => high = middle,
            }
        }
        Ok(best)
    }

    /// The next complete packet, reading pages as needed; `None` at the end.
    fn next_packet(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(packet) = self.packets.pop_front() {
                return Ok(Some(packet));
            }
            let page = match self.file.page_at(self.next_page)? {
                Some(page) => page,
                // Damage: resynchronise on the next valid page.
                None => match self.file.next_page(self.next_page + 1, self.file.len())? {
                    Some(page) => {
                        self.partial = None;
                        self.resumed = true;
                        page
                    }
                    None => return Ok(None),
                },
            };
            self.next_page = page.end();
            if page.serial != self.serial {
                continue;
            }
            for (index, (bytes, complete)) in page.pieces().into_iter().enumerate() {
                let continued = index == 0 && page.flags & FLAG_CONTINUED != 0;
                let packet = if continued {
                    match self.partial.take() {
                        Some(mut packet) if !self.resumed => {
                            packet.extend_from_slice(bytes);
                            packet
                        }
                        _ => continue,
                    }
                } else {
                    bytes.to_vec()
                };
                if complete {
                    if !packet.is_empty() {
                        self.packets.push_back(packet);
                    }
                } else {
                    self.partial = Some(packet);
                }
            }
            self.resumed = false;
        }
    }
}

impl<R: Read + Seek + Send> PcmReader for OpusReader<R> {
    fn format(&self) -> AudioFormat {
        AudioFormat::Opus
    }

    fn total_samples(&self) -> u64 {
        self.total
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn seek(&mut self, sample: u64) -> io::Result<()> {
        let target = sample.min(self.total);
        let goal = (target + self.pre_skip).saturating_sub(SEEK_PREROLL);
        match self.page_before(goal)? {
            Some(page) => self.restart(page.end(), page.granule, target),
            None => self.restart(self.data_start, 0, target),
        }
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
            let Some(packet) = self.next_packet()? else {
                break;
            };
            let count = self
                .decoder
                .decode_to_slice(&packet, &mut self.pcm, false)
                .map_err(opus_error)?;
            let drop = self.skip.min(count as u64) as usize;
            self.skip -= drop as u64;
            self.decoded
                .extend(self.pcm[drop..count].iter().map(|&sample| sample as i16));
        }
        Ok(filled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ogg::parse_page;
    use crate::reader::read_to_end;
    use crate::testsignal::{best_lag, snr_db, speech_like};
    use std::io::Cursor;

    /// Encode `samples` into an in-memory Ogg Opus file.
    fn encode(samples: &[i16]) -> Vec<u8> {
        let mut writer = OpusWriter::new(Vec::new()).unwrap();
        writer.write(samples).unwrap();
        assert_eq!(writer.finish().unwrap(), samples.len() as u64);
        assert_eq!(
            writer.finish().unwrap(),
            samples.len() as u64,
            "finish twice"
        );
        writer.out
    }

    fn pages(bytes: &[u8]) -> Vec<crate::ogg::Page> {
        let mut file = OggFile::new(Cursor::new(bytes.to_vec())).unwrap();
        let mut pages = Vec::new();
        let mut offset = 0;
        while let Some(page) = file.page_at(offset).unwrap() {
            offset = page.end();
            pages.push(page);
        }
        assert_eq!(offset, bytes.len() as u64, "pages fill the file");
        pages
    }

    fn pre_skip(bytes: &[u8]) -> u64 {
        let head = parse_page(bytes, 0).unwrap();
        let packet = head.pieces()[0].0.to_vec();
        u64::from(u16::from_le_bytes([packet[10], packet[11]]))
    }

    #[test]
    fn the_stream_starts_with_opus_head_and_tags_on_their_own_pages() {
        let bytes = encode(&speech_like(0.5, 1));
        let pages = pages(&bytes);
        let head = pages[0].pieces();
        assert_eq!(pages[0].flags, FLAG_BOS);
        assert_eq!(pages[0].granule, 0);
        assert_eq!(head.len(), 1);
        let head = head[0].0;
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(head.len(), 19);
        assert_eq!(head[8], 1, "version");
        assert_eq!(head[9], 1, "mono");
        let skip = u16::from_le_bytes([head[10], head[11]]);
        assert!(
            (120..=1_000).contains(&skip),
            "pre-skip {skip} is the lookahead"
        );
        assert_eq!(u32::from_le_bytes(head[12..16].try_into().unwrap()), 48_000);
        assert_eq!(&head[16..19], &[0, 0, 0], "no gain, mapping family 0");
        let tags = pages[1].pieces();
        assert_eq!(tags.len(), 1);
        assert_eq!(&tags[0].0[..8], b"OpusTags");
        assert_eq!(pages[1].granule, 0);
        assert!(pages[2..].iter().all(|page| page.serial == pages[0].serial));
    }

    #[test]
    fn granules_count_20_ms_packets_and_the_last_page_trims_the_padding() {
        let samples = speech_like(2.6, 2);
        let bytes = encode(&samples);
        let skip = pre_skip(&bytes);
        let pages = pages(&bytes);
        let audio = &pages[2..];
        let (last, middle) = audio.split_last().unwrap();
        let mut packets = 0u64;
        for page in middle {
            packets += page.pieces().len() as u64;
            assert_eq!(page.granule, packets * 960);
            assert!(page.pieces().len() <= 50, "a page every second at most");
            assert_eq!(page.flags & FLAG_EOS, 0);
        }
        packets += last.pieces().len() as u64;
        assert_eq!(last.flags, FLAG_EOS);
        assert_eq!(last.granule, skip + samples.len() as u64);
        assert!(
            packets * 960 >= skip + samples.len() as u64,
            "enough audio to trim"
        );
        assert!(last.granule > middle.last().unwrap().granule);
    }

    #[test]
    fn an_independent_ogg_reader_accepts_the_stream() {
        let samples = speech_like(3.0, 4);
        let bytes = encode(&samples);
        let mut reader = ogg::reading::PacketReader::new(Cursor::new(bytes.clone()));
        let head = reader.read_packet_expected().unwrap();
        assert!(head.first_in_stream());
        assert_eq!(&head.data[..8], b"OpusHead");
        let tags = reader.read_packet_expected().unwrap();
        assert_eq!(&tags.data[..8], b"OpusTags");
        let mut audio = 0;
        let mut last = None;
        while let Some(packet) = reader.read_packet().unwrap() {
            audio += 1;
            last = Some(packet);
        }
        let last = last.unwrap();
        assert!(last.last_in_stream());
        assert_eq!(last.absgp_page(), pre_skip(&bytes) + samples.len() as u64);
        assert!(audio >= 150, "{audio} packets for 3 s");
    }

    #[test]
    fn decoding_gives_back_exactly_the_samples_written() {
        for len in [0usize, 1, 959, 960, 961, 48_000 * 3 + 123] {
            let samples = speech_like(len as f64 / 48_000.0, 5);
            assert_eq!(samples.len(), len);
            let bytes = encode(&samples);
            let mut reader = OpusReader::new(Cursor::new(bytes)).unwrap();
            assert_eq!(reader.total_samples(), len as u64, "length {len}");
            let decoded = read_to_end(&mut reader).unwrap();
            assert_eq!(decoded.len(), len, "length {len}");
        }
    }

    #[test]
    fn decoded_speech_is_aligned_and_close_to_the_input() {
        let samples = speech_like(6.0, 6);
        let bytes = encode(&samples);
        let mut reader = OpusReader::new(Cursor::new(bytes.clone())).unwrap();
        let decoded = read_to_end(&mut reader).unwrap();
        // Within a few samples: hybrid mode is not exactly phase-linear. A
        // wrong pre-skip would be off by hundreds (the lookahead is 312).
        let lag = best_lag(&samples, &decoded, 400);
        assert!(lag.abs() <= 4, "pre-skip aligns the output (lag {lag})");
        eprintln!(
            "opus 32 kbps: SNR {:.1} dB, {:.1} kbps",
            snr_db(&samples, &decoded),
            bytes.len() as f64 * 8.0 / (samples.len() as f64 / 48_000.0) / 1_000.0
        );
        let snr = snr_db(&samples, &decoded);
        assert!(snr > 12.0, "SNR {snr:.1} dB at 32 kbps");
        let seconds = samples.len() as f64 / 48_000.0;
        let kbps = bytes.len() as f64 * 8.0 / seconds / 1_000.0;
        assert!((20.0..44.0).contains(&kbps), "{kbps:.1} kbps");
    }

    #[test]
    fn pages_reach_the_file_every_second_before_finish() {
        let samples = speech_like(2.5, 7);
        let mut writer = OpusWriter::new(Vec::new()).unwrap();
        writer.write(&samples).unwrap();
        let on_file = pages(&writer.out);
        // Head, tags, and the two full seconds already on pages.
        assert_eq!(on_file.len(), 4, "pages before finish");
        assert_eq!(on_file[3].granule, 100 * 960);
        writer.flush().unwrap();
        let flushed = pages(&writer.out);
        assert_eq!(
            flushed.len(),
            5,
            "flush puts the rest but the newest packet on a page"
        );
        assert_eq!(flushed[4].granule, (2_500 / 20 - 1) * 960);
    }

    #[test]
    fn a_file_cut_mid_page_plays_up_to_its_last_whole_page() {
        let samples = speech_like(3.3, 8);
        let mut writer = OpusWriter::new(Vec::new()).unwrap();
        writer.write(&samples).unwrap();
        // A kill now leaves what is on the file; cut into the last page too.
        let mut bytes = writer.out.clone();
        let whole = pages(&bytes);
        let last = whole.last().unwrap().clone();
        bytes.truncate((last.offset + last.len() / 2) as usize);
        let skip = pre_skip(&bytes);
        let mut reader = OpusReader::new(Cursor::new(bytes)).unwrap();
        let expected = whole[whole.len() - 2].granule - skip;
        assert_eq!(reader.total_samples(), expected);
        let decoded = read_to_end(&mut reader).unwrap();
        assert_eq!(decoded.len() as u64, expected);
        assert!(best_lag(&samples[..decoded.len()], &decoded, 200).abs() <= 4);
        let snr = snr_db(&samples[..decoded.len()], &decoded);
        // Waveform SNR of SILK/hybrid speech varies with content (11-16 dB
        // here); misaligned or wrong audio is near 0 dB.
        assert!(snr > 8.0, "SNR {snr:.1} dB");
    }

    #[test]
    fn a_file_with_only_its_headers_is_empty() {
        let writer = OpusWriter::new(Vec::new()).unwrap();
        let mut reader = OpusReader::new(Cursor::new(writer.out)).unwrap();
        assert_eq!(reader.total_samples(), 0);
        assert!(read_to_end(&mut reader).unwrap().is_empty());
    }

    #[test]
    fn seeking_lands_on_the_sample_and_matches_a_straight_decode() {
        let samples = speech_like(8.0, 9);
        let bytes = encode(&samples);
        let mut straight = OpusReader::new(Cursor::new(bytes.clone())).unwrap();
        let all = read_to_end(&mut straight).unwrap();
        let mut reader = OpusReader::new(Cursor::new(bytes)).unwrap();
        for target in [
            0u64,
            100,
            3_000,
            48_000 * 2 + 17,
            48_000 * 5 + 960 * 7,
            383_000,
        ] {
            reader.seek(target).unwrap();
            assert_eq!(reader.position(), target);
            let mut chunk = vec![0i16; 4_800];
            let read = reader.read(&mut chunk).unwrap();
            let end = (target as usize + read).min(all.len());
            assert_eq!(read, end - target as usize);
            let reference = &all[target as usize..end];
            // The decoder restarts 80 ms early, so it has converged.
            assert!(
                snr_db(reference, &chunk[..read]) > 25.0,
                "seek to {target}: SNR {:.1}",
                snr_db(reference, &chunk[..read])
            );
        }
        reader.seek(10_000_000).unwrap();
        assert_eq!(
            reader.position(),
            samples.len() as u64,
            "clamped to the end"
        );
        assert_eq!(reader.read(&mut [0i16; 10]).unwrap(), 0);
        reader.seek(0).unwrap();
        assert_eq!(
            read_to_end(&mut reader).unwrap(),
            all,
            "back to the start is exact"
        );
    }
}
