//! FFI stubs for libInstaAudio.so.
//! These will be replaced with real extern "C" declarations once
//! we extract headers from the device and validate on-device.

use tracing::info;

pub fn init_audio() -> Result<(), String> {
    #[cfg(target_arch = "aarch64")]
    {
        info!("audio init: would call Insta360_InitAudio() on device");
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        info!("audio init: stub (not on device)");
    }
    Ok(())
}

pub fn set_work_mode(mode: u32) -> Result<(), String> {
    info!("set_work_mode({mode}): stub");
    Ok(())
}

pub fn set_dump_enable(enable: bool) {
    info!("set_dump_enable({enable}): stub");
}
