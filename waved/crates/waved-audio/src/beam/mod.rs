//! Beamforming: turn 8-ch raw mic into a direction-steered mono (or stereo) signal.
pub mod geometry;
pub use geometry::{MicGeometry, Position3D};
