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

use crate::command::id;
use crate::frame::Frame;

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
}
