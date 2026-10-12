//! The MCU's serial line (`/dev/ttyS4`, 115200): a reader thread turns bytes
//! into messages; sends are framed with this side's sequence number.

use crate::mavlink::{encode, Message, Parser};
use crate::power_off::Transport;
use crate::protocol::Outgoing;
use crossbeam_channel::{bounded, Receiver};
use serialport::SerialPort;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::Duration;
use tracing::{error, warn};

pub struct Link {
    writer: Mutex<(Box<dyn SerialPort>, u8)>,
    messages: Receiver<Message>,
}

impl Link {
    pub fn open(path: &str, baud: u32) -> Result<Self, serialport::Error> {
        let port = serialport::new(path, baud).timeout(Duration::from_millis(100)).open()?;
        let mut reader = port.try_clone()?;
        let (tx, rx) = bounded::<Message>(64);
        std::thread::Builder::new()
            .name("mcu-reader".into())
            .spawn(move || {
                let mut parser = Parser::default();
                let mut buf = [0u8; 64];
                loop {
                    match reader.read(&mut buf) {
                        Ok(n) => {
                            for &b in &buf[..n] {
                                if let Some(m) = parser.feed(b) {
                                    if tx.send(m).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            error!("mcu: read failed: {e}");
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            })
            .expect("spawn mcu-reader");
        Ok(Self { writer: Mutex::new((port, 0)), messages: rx })
    }

    pub fn messages(&self) -> &Receiver<Message> {
        &self.messages
    }
}

impl Transport for Link {
    fn send(&self, out: &Outgoing) {
        let mut guard = self.writer.lock().unwrap();
        let (port, seq) = &mut *guard;
        let frame = encode(*seq, out.msgid, &out.payload);
        *seq = seq.wrapping_add(1);
        if let Err(e) = port.write_all(&frame).and_then(|()| port.flush()) {
            warn!("mcu: write failed: {e}");
        }
    }

    fn recv(&self, timeout: Duration) -> Option<Message> {
        self.messages.recv_timeout(timeout).ok()
    }
}
