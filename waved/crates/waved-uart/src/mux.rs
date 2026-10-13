use crate::frame::{Frame, FrameParser, FrameTarget};
use crossbeam_channel::{bounded, Receiver, Sender};
use serialport::SerialPort;
use std::io::Read;
use std::time::Duration;
use tracing::{debug, error, trace, warn};

/// A frame with priority for the write queue.
pub struct PrioritizedFrame {
    pub raw: Vec<u8>,
    pub priority: bool, // true = MCU motor-stop etc, sent first
}

pub struct UartMux {
    // Errata #2: Use Option + take pattern since crossbeam Receiver is not Clone
    bt_rx: Option<Receiver<Frame>>,
    mcu_rx: Option<Receiver<Frame>>,
    write_tx: Sender<PrioritizedFrame>,
    _reader_handle: std::thread::JoinHandle<()>,
    _writer_handle: std::thread::JoinHandle<()>,
}

impl UartMux {
    // Errata #3: serialport::new().open() returns Box<dyn SerialPort>
    pub fn open(port_path: &str, baud_rate: u32) -> Result<Self, serialport::Error> {
        let port: Box<dyn SerialPort> = serialport::new(port_path, baud_rate)
            .timeout(Duration::from_millis(100))
            .open()?;
        Self::from_port(port)
    }

    /// The mux over an open port (tests hand it one end of a pty pair).
    ///
    /// The reader gets its own handle (`try_clone`), as `waved-mcu`'s link
    /// does. Sharing one handle behind a mutex starved the writer: the reader
    /// held it through each 100 ms read and took it back at once, so an
    /// acknowledgement could wait for seconds (1.4 s measured on a quiet pty)
    /// while the module repeated its frame every 500 ms.
    pub fn from_port(mut port: Box<dyn SerialPort>) -> Result<Self, serialport::Error> {
        port.set_timeout(Duration::from_millis(100))?;
        let reader_port = port.try_clone()?;

        let (bt_tx, bt_rx) = bounded::<Frame>(64);
        let (mcu_tx, mcu_rx) = bounded::<Frame>(64);
        let (write_tx, write_rx) = bounded::<PrioritizedFrame>(64);

        let reader_handle = std::thread::Builder::new()
            .name("uart-reader".into())
            .spawn(move || {
                Self::reader_loop(reader_port, bt_tx, mcu_tx);
            })
            .expect("failed to spawn uart reader thread");

        let writer_handle = std::thread::Builder::new()
            .name("uart-writer".into())
            .spawn(move || {
                Self::writer_loop(port, write_rx);
            })
            .expect("failed to spawn uart writer thread");

        Ok(Self {
            bt_rx: Some(bt_rx),
            mcu_rx: Some(mcu_rx),
            write_tx,
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
        })
    }

    /// Take the BT receiver. Must be called before wrapping in Arc.
    pub fn take_bt_receiver(&mut self) -> Receiver<Frame> {
        self.bt_rx.take().expect("bt_rx already taken")
    }

    /// Take the MCU receiver. Must be called before wrapping in Arc.
    pub fn take_mcu_receiver(&mut self) -> Receiver<Frame> {
        self.mcu_rx.take().expect("mcu_rx already taken")
    }

    pub fn send(&self, raw: Vec<u8>, priority: bool) {
        let _ = self.write_tx.send(PrioritizedFrame { raw, priority });
    }

    fn reader_loop(mut port: Box<dyn SerialPort>, bt_tx: Sender<Frame>, mcu_tx: Sender<Frame>) {
        let mut parser = FrameParser::new();
        let mut buf = [0u8; 64];

        loop {
            match port.read(&mut buf) {
                Ok(n) => {
                    for &byte in &buf[..n] {
                        if handle_byte(byte, &mut parser, &bt_tx, &mcu_tx).is_err() {
                            return;
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    // Normal timeout, continue
                }
                Err(e) => {
                    error!("UART read error: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn writer_loop(mut port: Box<dyn SerialPort>, write_rx: Receiver<PrioritizedFrame>) {
        use std::io::Write;

        for pframe in write_rx.iter() {
            if let Err(e) = port.write_all(&pframe.raw) {
                error!("UART write error: {e}");
            }
            if let Err(e) = port.flush() {
                error!("UART flush error: {e}");
            }
        }
    }
}

/// Process one byte from the serial port: feed it into the frame parser and
/// dispatch any completed frame to the appropriate receiver. Returns `Err(())`
/// if a receiver has been dropped (caller should stop the reader).
///
/// Note: raw single bytes that are NOT part of a `0xDC/0xCD` or `0x75/..` frame
/// are discarded by the parser. In particular the MCU's `0x4B` heartbeat — and
/// its low-bit-corrupted variants `0x48`/`0x49`/`0x4A` — are intentionally
/// ignored here. An earlier revision treated those variants as physical
/// Mute/VolumeDown/VolumeUp presses, but on-device capture proved they are
/// heartbeat corruption, not button input (physical buttons do not reach waved
/// over ttyS1). Forwarding them produced the "ghost volume" bug on the Mac.
/// See `docs/AUDIO_HARDWARE.md`.
fn handle_byte(
    byte: u8,
    parser: &mut FrameParser,
    bt_tx: &Sender<Frame>,
    mcu_tx: &Sender<Frame>,
) -> Result<(), ()> {
    if let Some(result) = parser.feed(byte) {
        match result {
            Ok(frame) => {
                trace!("{}", describe(&frame));
                let tx = match frame.target {
                    FrameTarget::Bluetooth => bt_tx,
                    FrameTarget::Mcu => mcu_tx,
                };
                if tx.send(frame).is_err() {
                    debug!("receiver dropped, stopping reader");
                    return Err(());
                }
            }
            Err(e) => {
                warn!("frame parse error: {e}");
            }
        }
    }
    Ok(())
}

/// A frame for the trace log: its prefix, command byte and length, never its
/// bytes. The Bluetooth module sends the phone book, caller numbers and the
/// phone's name and MAC on this port.
fn describe(frame: &Frame) -> String {
    let [p0, p1] = frame.header;
    match frame.data.split_first() {
        Some((cmd, rest)) => format!(
            "parsed frame: {p0:02x} {p1:02x} cmd {cmd:#04x}, {} data bytes, target {:?}",
            rest.len(),
            frame.target
        ),
        None => format!("parsed frame: {p0:02x} {p1:02x} empty, target {:?}", frame.target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels() -> (Sender<Frame>, Receiver<Frame>, Sender<Frame>, Receiver<Frame>) {
        let (bt_tx, bt_rx) = bounded::<Frame>(16);
        let (mcu_tx, mcu_rx) = bounded::<Frame>(16);
        (bt_tx, bt_rx, mcu_tx, mcu_rx)
    }

    /// Stock acknowledges a frame at once, from its receive thread
    /// (`bt_msg_recv_thread`). Here the acknowledgement is a write: it must
    /// reach the wire while the reader sits in a read on a quiet line, not
    /// after the module has repeated its frame (every 500 ms until
    /// acknowledged, docs/BLUETOOTH_PROTOCOL.md).
    #[cfg(unix)]
    #[test]
    fn a_write_never_waits_for_the_reader() {
        use serialport::TTYPort;
        use std::io::Read;
        use std::time::{Duration, Instant};

        let (ours, mut module) = TTYPort::pair().expect("pty pair");
        let mux = UartMux::from_port(Box::new(ours)).expect("mux");
        module.set_timeout(Duration::from_secs(2)).unwrap();
        let ack = [0x75, 0x46, 0x02, 0xB2, 0x00, 0xB4];
        for _ in 0..5 {
            // A quiet line: the reader is inside a read that times out at 100 ms.
            std::thread::sleep(Duration::from_millis(30));
            let start = Instant::now();
            mux.send(ack.to_vec(), false);
            let mut got = [0u8; 6];
            module.read_exact(&mut got).expect("the ack reaches the module");
            let waited = start.elapsed();
            assert_eq!(got, ack);
            assert!(waited < Duration::from_millis(50), "the ack waited {waited:?}");
        }
    }

    #[test]
    fn the_trace_line_never_prints_a_payload() {
        // ttyS1 carries the phone book (0xB0), caller numbers (0xB1) and the
        // phone's name and MAC (docs/BLUETOOTH_PROTOCOL.md): log the command
        // byte and the length only.
        let frame = Frame {
            header: [0x75, 0x45],
            target: FrameTarget::Bluetooth,
            data: vec![0xB1, b'+', b'3', b'2', b'4', b'7', b'0'],
        };
        let line = describe(&frame);
        // The prefix tells the module's repeated frames (75 45) from its
        // one-shot answers (75 46); it carries nothing personal.
        assert_eq!(line, "parsed frame: 75 45 cmd 0xb1, 6 data bytes, target Bluetooth");
        assert!(!line.contains("43") && !line.contains("+32"), "{line}");
        let answer = Frame { header: [0x75, 0x46], target: FrameTarget::Bluetooth, data: vec![0xC5, 0x00] };
        assert_eq!(describe(&answer), "parsed frame: 75 46 cmd 0xc5, 1 data bytes, target Bluetooth");
        let empty = Frame { header: [0x75, 0x45], target: FrameTarget::Bluetooth, data: vec![] };
        assert_eq!(describe(&empty), "parsed frame: 75 45 empty, target Bluetooth");
    }

    #[test]
    fn standalone_button_bytes_are_ignored_as_heartbeat_corruption() {
        // On-device measurement (see AUDIO_HARDWARE.md): the MCU emits a 0x4B
        // heartbeat byte ~every 0.5 s and individual bytes get corrupted with
        // low bits cleared (0x4B -> 0x4A/0x49/0x48, also 0x09/0x08). These are
        // NOT button presses — physical buttons do not reach waved over ttyS1
        // at all. A standalone byte in 0x48..=0x4A must therefore produce no
        // event; otherwise it propagates to the Mac as a phantom volume/mute
        // HID key (the "ghost volume" bug).
        for byte in [0x48u8, 0x49, 0x4A] {
            let (bt_tx, _bt_rx, mcu_tx, mcu_rx) = channels();
            let mut parser = FrameParser::new();
            handle_byte(byte, &mut parser, &bt_tx, &mcu_tx).unwrap();
            assert!(
                mcu_rx.try_recv().is_err(),
                "standalone {byte:#04x} must not emit a frame",
            );
        }
    }

    #[test]
    fn battery_frame_with_button_byte_in_payload_does_not_emit_phantom_button() {
        // Regression: voltage 3658 mV serialises as low byte 0x4A, high byte 0x0E.
        // Pre-fix, the reader intercepted 0x4A mid-frame and injected a synthetic
        // VolumeUp button press, which propagated as a HID Consumer Volume Up to
        // the host Mac on every matching battery report.
        let payload = [0x10, 85, 0x4A, 0x0E, 25, 1];
        let raw = Frame::build([0xDC, 0xCD], &payload).unwrap();

        let (bt_tx, _bt_rx, mcu_tx, mcu_rx) = channels();
        let mut parser = FrameParser::new();
        for &b in &raw {
            handle_byte(b, &mut parser, &bt_tx, &mcu_tx).unwrap();
        }

        let frame = mcu_rx.try_recv().expect("expected the real battery frame");
        assert_eq!(frame.data, payload.to_vec());
        assert!(
            mcu_rx.try_recv().is_err(),
            "no second (synthetic) frame should be emitted from a mid-frame payload byte",
        );
    }

    #[test]
    fn all_three_button_bytes_in_payload_are_ignored() {
        // Construct a frame whose payload contains every interceptable button byte.
        let payload = [0x10, 0x48, 0x49, 0x4A, 0x00, 0x00];
        let raw = Frame::build([0xDC, 0xCD], &payload).unwrap();

        let (bt_tx, _bt_rx, mcu_tx, mcu_rx) = channels();
        let mut parser = FrameParser::new();
        for &b in &raw {
            handle_byte(b, &mut parser, &bt_tx, &mcu_tx).unwrap();
        }

        let frame = mcu_rx.try_recv().expect("expected the real frame");
        assert_eq!(frame.data, payload.to_vec());
        assert!(mcu_rx.try_recv().is_err());
    }

    #[test]
    fn heartbeat_byte_does_not_emit_anything() {
        let (bt_tx, _bt_rx, mcu_tx, mcu_rx) = channels();
        let mut parser = FrameParser::new();
        handle_byte(0x4B, &mut parser, &bt_tx, &mcu_tx).unwrap();
        assert!(mcu_rx.try_recv().is_err());
    }

    #[test]
    fn standalone_button_byte_after_frame_is_ignored() {
        // A complete frame is delivered, but a standalone button-range byte
        // that follows it (heartbeat corruption) must NOT emit a second frame.
        let payload = [0x10, 85, 0x20, 0x0F, 25, 1];
        let raw = Frame::build([0xDC, 0xCD], &payload).unwrap();

        let (bt_tx, _bt_rx, mcu_tx, mcu_rx) = channels();
        let mut parser = FrameParser::new();
        for &b in &raw {
            handle_byte(b, &mut parser, &bt_tx, &mcu_tx).unwrap();
        }
        handle_byte(0x4A, &mut parser, &bt_tx, &mcu_tx).unwrap();

        let first = mcu_rx.try_recv().expect("battery frame");
        assert_eq!(first.data, payload.to_vec());
        assert!(
            mcu_rx.try_recv().is_err(),
            "standalone 0x4A after a frame must not emit a synthetic button frame",
        );
    }
}
