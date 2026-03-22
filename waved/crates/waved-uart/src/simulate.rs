//! Simulated UART mux for testing without hardware.
use crate::frame::{Frame, FrameTarget};
use crossbeam_channel::{bounded, Receiver, Sender};
use tracing::info;

pub struct SimUartMux {
    bt_rx: Option<Receiver<Frame>>,
    mcu_rx: Option<Receiver<Frame>>,
    write_tx: Sender<Vec<u8>>,
    write_rx: Receiver<Vec<u8>>,
    bt_inject_tx: Sender<Frame>,
    mcu_inject_tx: Sender<Frame>,
}

impl SimUartMux {
    pub fn new() -> Self {
        let (bt_tx, bt_rx) = bounded::<Frame>(64);
        let (mcu_tx, mcu_rx) = bounded::<Frame>(64);
        let (write_tx, write_rx) = bounded::<Vec<u8>>(64);
        info!("[SIM] UART mux created (in-memory channels)");
        Self {
            bt_rx: Some(bt_rx), mcu_rx: Some(mcu_rx),
            write_tx, write_rx,
            bt_inject_tx: bt_tx, mcu_inject_tx: mcu_tx,
        }
    }

    pub fn take_bt_receiver(&mut self) -> Receiver<Frame> {
        self.bt_rx.take().expect("bt_rx already taken")
    }

    pub fn take_mcu_receiver(&mut self) -> Receiver<Frame> {
        self.mcu_rx.take().expect("mcu_rx already taken")
    }

    pub fn send(&self, raw: Vec<u8>, _priority: bool) {
        let _ = self.write_tx.send(raw);
    }

    pub fn inject_mcu_frame(&self, data: Vec<u8>) {
        let _ = self.mcu_inject_tx.send(Frame {
            header: [0xDC, 0xCD], target: FrameTarget::Mcu, data,
        });
    }

    pub fn inject_bt_frame(&self, data: Vec<u8>) {
        let _ = self.bt_inject_tx.send(Frame {
            header: [0x75, 0x46], target: FrameTarget::Bluetooth, data,
        });
    }

    pub fn try_recv_written(&self) -> Option<Vec<u8>> {
        self.write_rx.try_recv().ok()
    }
}
