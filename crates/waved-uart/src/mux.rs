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
                    if let Some(result) = parser.feed(buf[0]) {
                        match result {
                            Ok(frame) => {
                                trace!("parsed frame: {:?} target={:?}", frame.data, frame.target);
                                let tx = match frame.target {
                                    FrameTarget::Bluetooth => &bt_tx,
                                    FrameTarget::Mcu => &mcu_tx,
                                };
                                if tx.send(frame).is_err() {
                                    debug!("receiver dropped, stopping reader");
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!("frame parse error: {e}");
                            }
                        }
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
