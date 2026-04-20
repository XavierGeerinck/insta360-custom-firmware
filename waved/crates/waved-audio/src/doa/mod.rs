//! Direction-of-arrival estimation.
pub mod gcc_phat;
pub mod solve;

pub use solve::{DoaAngles, PairObservation};
