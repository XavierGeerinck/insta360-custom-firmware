//! System services for the Wave (workstream E): display power, the power key,
//! time zone and RTC, alert tones, factory reset and About. Device access sits
//! behind small types and injected paths so everything runs in host tests.

pub mod about;
pub mod backlight;
mod civil;
pub mod display;
pub mod power_key;
pub mod rtc;
mod service;
pub mod tz;

pub use service::SystemService;
