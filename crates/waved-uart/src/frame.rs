use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum FrameTarget {
    Bluetooth, // Headers: 0x75 0x46, 0x75 0x45
    Mcu,       // Header: 0xDC 0xCD
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub header: [u8; 2],
    pub target: FrameTarget,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("invalid header: {0:#04x} {1:#04x}")]
    InvalidHeader(u8, u8),
    #[error("checksum mismatch: expected {expected:#04x}, got {actual:#04x}")]
    ChecksumMismatch { expected: u8, actual: u8 },
    #[error("buffer too short")]
    BufferTooShort,
}

impl Frame {
    /// Build a frame ready to send (with checksum appended).
    pub fn build(header: [u8; 2], data: &[u8]) -> Result<Vec<u8>, FrameError> {
        let target = Self::classify_header(header)?;
        let _ = target; // validate header
        let len = data.len() as u8;
        let mut buf = Vec::with_capacity(4 + data.len());
        buf.extend_from_slice(&header);
        buf.push(len);
        buf.extend_from_slice(data);
        let checksum = buf.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        buf.push(checksum);
        Ok(buf)
    }

    pub fn classify_header(header: [u8; 2]) -> Result<FrameTarget, FrameError> {
        match header {
            [0x75, 0x46] | [0x75, 0x45] => Ok(FrameTarget::Bluetooth),
            [0xDC, 0xCD] => Ok(FrameTarget::Mcu),
            [a, b] => Err(FrameError::InvalidHeader(a, b)),
        }
    }
}

/// State machine parser that processes bytes one at a time.
/// Call `feed()` with each byte; returns `Some(Frame)` when a complete frame is parsed.
pub struct FrameParser {
    state: ParseState,
    header: [u8; 2],
    length: u8,
    data: Vec<u8>,
    checksum_accum: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ParseState {
    Header0,
    Header1,
    Length,
    Data,
    Checksum,
}

impl FrameParser {
    pub fn new() -> Self {
        Self {
            state: ParseState::Header0,
            header: [0; 2],
            length: 0,
            data: Vec::new(),
            checksum_accum: 0,
        }
    }

    pub fn feed(&mut self, byte: u8) -> Option<Result<Frame, FrameError>> {
        match self.state {
            ParseState::Header0 => {
                if byte == 0x75 || byte == 0xDC {
                    self.header[0] = byte;
                    self.checksum_accum = byte;
                    self.state = ParseState::Header1;
                }
                None
            }
            ParseState::Header1 => {
                let valid = match self.header[0] {
                    0x75 => byte == 0x46 || byte == 0x45,
                    0xDC => byte == 0xCD,
                    _ => false,
                };
                if valid {
                    self.header[1] = byte;
                    self.checksum_accum = self.checksum_accum.wrapping_add(byte);
                    self.state = ParseState::Length;
                    None
                } else {
                    self.reset();
                    None
                }
            }
            ParseState::Length => {
                self.length = byte;
                self.checksum_accum = self.checksum_accum.wrapping_add(byte);
                self.data.clear();
                if byte == 0 {
                    self.state = ParseState::Checksum;
                } else {
                    self.state = ParseState::Data;
                }
                None
            }
            ParseState::Data => {
                self.data.push(byte);
                self.checksum_accum = self.checksum_accum.wrapping_add(byte);
                if self.data.len() == self.length as usize {
                    self.state = ParseState::Checksum;
                }
                None
            }
            ParseState::Checksum => {
                let result = if byte == self.checksum_accum {
                    match Frame::classify_header(self.header) {
                        Ok(target) => Ok(Frame {
                            header: self.header,
                            target,
                            data: std::mem::take(&mut self.data),
                        }),
                        Err(e) => Err(e),
                    }
                } else {
                    Err(FrameError::ChecksumMismatch {
                        expected: self.checksum_accum,
                        actual: byte,
                    })
                };
                self.reset();
                Some(result)
            }
        }
    }

    fn reset(&mut self) {
        self.state = ParseState::Header0;
        self.header = [0; 2];
        self.length = 0;
        self.data.clear();
        self.checksum_accum = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_bt_frame() {
        let raw = Frame::build([0x75, 0x46], &[0x01, 0x02]).unwrap();
        assert_eq!(raw.len(), 6); // 2 header + 1 len + 2 data + 1 checksum
        assert_eq!(raw[0..2], [0x75, 0x46]);
        assert_eq!(raw[2], 2); // length
        assert_eq!(raw[3..5], [0x01, 0x02]);
        let expected_checksum = 0x75u8
            .wrapping_add(0x46)
            .wrapping_add(2)
            .wrapping_add(0x01)
            .wrapping_add(0x02);
        assert_eq!(raw[5], expected_checksum);
    }

    #[test]
    fn build_mcu_frame() {
        let raw = Frame::build([0xDC, 0xCD], &[0x10]).unwrap();
        assert_eq!(raw[0..2], [0xDC, 0xCD]);
        assert_eq!(raw[2], 1);
    }

    #[test]
    fn build_invalid_header() {
        assert!(Frame::build([0x00, 0x00], &[0x01]).is_err());
    }

    #[test]
    fn parse_bt_frame() {
        let raw = Frame::build([0x75, 0x46], &[0x0E]).unwrap();
        let mut parser = FrameParser::new();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        let frame = result.unwrap().unwrap();
        assert_eq!(frame.target, FrameTarget::Bluetooth);
        assert_eq!(frame.data, vec![0x0E]);
    }

    #[test]
    fn parse_mcu_frame() {
        let raw = Frame::build([0xDC, 0xCD], &[0x01, 0x55]).unwrap();
        let mut parser = FrameParser::new();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        let frame = result.unwrap().unwrap();
        assert_eq!(frame.target, FrameTarget::Mcu);
        assert_eq!(frame.data, vec![0x01, 0x55]);
    }

    #[test]
    fn parse_recovers_from_garbage() {
        let mut parser = FrameParser::new();
        // Feed garbage
        for &byte in &[0xFF, 0x00, 0x13, 0xAB] {
            assert!(parser.feed(byte).is_none());
        }
        // Then a valid frame
        let raw = Frame::build([0x75, 0x45], &[0x09]).unwrap();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        let frame = result.unwrap().unwrap();
        assert_eq!(frame.target, FrameTarget::Bluetooth);
        assert_eq!(frame.data, vec![0x09]);
    }

    #[test]
    fn checksum_mismatch_detected() {
        let mut raw = Frame::build([0x75, 0x46], &[0x01]).unwrap();
        *raw.last_mut().unwrap() = 0xFF; // corrupt checksum
        let mut parser = FrameParser::new();
        let mut result = None;
        for &byte in &raw {
            if let Some(r) = parser.feed(byte) {
                result = Some(r);
            }
        }
        assert!(result.unwrap().is_err());
    }
}
