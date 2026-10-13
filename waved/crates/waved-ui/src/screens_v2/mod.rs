//! Revision-2 screens (milestone 2). Each module implements
//! `kit::screen::Screen`; `runtime::registry` maps routes to them at the
//! runtime switch. The type and constructor each module exports are in
//! `kit/README.md`, "Registering a screen". Only the owning workstream edits
//! a module; this file belongs to the integrator.

// Workstream A: main screens.
pub mod ambient;
pub mod boot;
pub mod powering_off;
pub mod decision;
pub mod first_run;
pub mod home;
pub mod main_parts;
pub mod messages;
pub mod recording;

// Workstream B: settings screens.
pub mod about;
pub mod audio_mode;
pub mod confirm;
pub mod datetime;
pub mod display;
pub mod menu;
pub mod mic_height;
pub mod noise;
pub mod ota;
pub mod system;
pub mod upload_mode;
pub mod uploads;
pub mod volume;
pub mod wifi;
pub mod wifi_setup;

// Workstream D: Files.
pub mod file_detail;
pub mod files;

// Workstream K: security.
pub mod pin;

// Workstream I: Bluetooth.
pub mod bluetooth;
pub mod call;

// Workstream G: USB storage.
pub mod usb_storage;
