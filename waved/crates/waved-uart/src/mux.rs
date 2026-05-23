use crate::frame::{Frame, FrameParser, FrameTarget};
use crossbeam_channel::{bounded, Receiver, Sender};
use serialport::SerialPort;
use std::io::Read;
use std::sync::{Arc, Mutex};
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

        let port: Arc<Mutex<Box<dyn SerialPort>>> = Arc::new(Mutex::new(port));

        let (bt_tx, bt_rx) = bounded::<Frame>(64);
        let (mcu_tx, mcu_rx) = bounded::<Frame>(64);
        let (write_tx, write_rx) = bounded::<PrioritizedFrame>(64);

        let reader_port = Arc::clone(&port);
        let reader_handle = std::thread::Builder::new()
            .name("uart-reader".into())
            .spawn(move || {
                Self::reader_loop(reader_port, bt_tx, mcu_tx);
            })
            .expect("failed to spawn uart reader thread");

        let writer_port = Arc::clone(&port);
        let writer_handle = std::thread::Builder::new()
            .name("uart-writer".into())
            .spawn(move || {
                Self::writer_loop(writer_port, write_rx);
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

    fn reader_loop(
        port: Arc<Mutex<Box<dyn SerialPort>>>,
        bt_tx: Sender<Frame>,
        mcu_tx: Sender<Frame>,
    ) {
        let mut parser = FrameParser::new();
        let mut buf = [0u8; 1];

        loop {
            let read_result = {
                let mut port = port.lock().unwrap();
                port.read(&mut buf)
            };

            match read_result {
                Ok(1) => {
                    if handle_byte(buf[0], &mut parser, &bt_tx, &mcu_tx).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
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

    fn writer_loop(
        port: Arc<Mutex<Box<dyn SerialPort>>>,
        write_rx: Receiver<PrioritizedFrame>,
    ) {
        use std::io::Write;

        for pframe in write_rx.iter() {
            let mut port = port.lock().unwrap();
            if let Err(e) = port.write_all(&pframe.raw) {
                error!("UART write error: {e}");
            }
            if let Err(e) = port.flush() {
                error!("UART flush error: {e}");
            }
        }
    }
}

/// Process one byte from the serial port: optionally inject a synthetic MCU
/// button frame, then feed the byte into the frame parser and dispatch any
/// completed frame to the appropriate receiver. Returns `Err(())` if a
/// receiver has been dropped (caller should stop the reader).
///
/// The button-byte interception is gated on `parser.is_idle()` because
/// `0x48..=0x4A` are also valid payload bytes of framed messages. A battery
/// frame carrying voltage 3658 mV (= `0x0E4A` little-endian) would otherwise
/// inject a phantom VolumeUp every time a battery report fell in that range,
/// producing the "ghost volume bumps" symptom on the Mac.
fn handle_byte(
    byte: u8,
    parser: &mut FrameParser,
    bt_tx: &Sender<Frame>,
    mcu_tx: &Sender<Frame>,
) -> Result<(), ()> {
    // Physical buttons arrive as raw bytes (not 0xDC/0xCD framed).
    // 0x48 = MuteToggle, 0x49 = VolumeDown, 0x4A = VolumeUp (on-device).
    // 0x4B is the MCU heartbeat; ignore it.
    if parser.is_idle() && matches!(byte, 0x48..=0x4A) {
        let frame = Frame {
            header: [0xDC, 0xCD],
            target: FrameTarget::Mcu,
            data: vec![byte],
        };
        if mcu_tx.send(frame).is_err() {
            debug!("mcu receiver dropped, stopping reader");
            return Err(());
        }
    }
    if let Some(result) = parser.feed(byte) {
        match result {
            Ok(frame) => {
                trace!("parsed frame: {:?} target={:?}", frame.data, frame.target);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn channels() -> (Sender<Frame>, Receiver<Frame>, Sender<Frame>, Receiver<Frame>) {
        let (bt_tx, bt_rx) = bounded::<Frame>(16);
        let (mcu_tx, mcu_rx) = bounded::<Frame>(16);
        (bt_tx, bt_rx, mcu_tx, mcu_rx)
    }

    #[test]
    fn standalone_button_byte_emits_synthetic_mcu_frame() {
        let (bt_tx, _bt_rx, mcu_tx, mcu_rx) = channels();
        let mut parser = FrameParser::new();
        handle_byte(0x4A, &mut parser, &bt_tx, &mcu_tx).unwrap();
        let frame = mcu_rx.try_recv().expect("expected synthetic button frame");
        assert_eq!(frame.target, FrameTarget::Mcu);
        assert_eq!(frame.data, vec![0x4A]);
        assert!(mcu_rx.try_recv().is_err(), "exactly one frame expected");
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
        assert!(parser.is_idle(), "heartbeat must not advance the parser");
    }

    #[test]
    fn button_press_between_real_frames_still_works() {
        // After a complete frame, parser returns to idle; a button byte that
        // immediately follows must still be intercepted.
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
        let second = mcu_rx.try_recv().expect("synthetic button frame");
        assert_eq!(second.data, vec![0x4A]);
    }
}
