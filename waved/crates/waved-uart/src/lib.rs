pub mod frame;

#[cfg(not(feature = "simulate"))]
pub mod mux;

#[cfg(feature = "simulate")]
pub mod simulate;

pub use frame::{Frame, FrameError, FrameParser, FrameTarget};

#[cfg(not(feature = "simulate"))]
pub use mux::UartMux;

#[cfg(feature = "simulate")]
pub use simulate::SimUartMux as UartMux;
