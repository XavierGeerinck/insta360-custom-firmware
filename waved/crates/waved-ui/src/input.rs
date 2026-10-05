//! Evdev touch input driver for /dev/input/event1 (ILITEK TDDI).
//!
//! Reads ABS_MT_POSITION_X/Y + BTN_TOUCH events and converts to LVGL lv_indev_data_t.

use crate::runtime::touch::TouchSample;
use tracing::info;

/// Evdev event structure (matches kernel input_event).
#[repr(C)]
struct InputEvent {
    tv_sec: libc::time_t,
    tv_usec: libc::suseconds_t,
    r#type: u16,
    code: u16,
    value: i32,
}

const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_TOUCH: u16 = 0x14a;
// The ILITEK TDDI driver uses the multi-touch (MT-B) protocol exclusively —
// it emits ABS_MT_POSITION_X/Y, NOT the legacy ABS_X/ABS_Y.
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;

/// ILITEK_TDDI reports native screen pixels (x 0-399), not 0-2047, and the
/// touch panel is mounted upside down like the display (`display.rs` flips
/// 180 degrees in `flush`).
pub fn flip_x(raw: i32) -> i32 {
    399 - raw.clamp(0, 399)
}

/// See `flip_x`; y runs 0-303.
pub fn flip_y(raw: i32) -> i32 {
    303 - raw.clamp(0, 303)
}

pub struct TouchInput {
    #[cfg(not(feature = "simulate"))]
    fd: std::os::unix::io::RawFd,
    sample: TouchSample,
}

impl TouchInput {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(not(feature = "simulate"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            use std::os::unix::io::IntoRawFd;
            // event1 = ILITEK_TDDI touch controller
            // event0 = vad_keypad (VAD microphone button, not touch)
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open("/dev/input/event1")?;
            let fd = file.into_raw_fd();
            info!(
                "[touch] opened /dev/input/event1 fd={} sizeof(InputEvent)={}",
                fd,
                std::mem::size_of::<InputEvent>()
            );

            // Log the device name from the kernel to confirm it's ILITEK_TDDI.
            unsafe {
                let mut name = [0u8; 256];
                // EVIOCGNAME(len) = _IOR('E', 0x06, char[len])
                let eviocgname: u64 = 0x80004506 | ((name.len() as u64) << 16);
                let ret = libc::ioctl(fd, eviocgname, name.as_mut_ptr() as *mut libc::c_void);
                if ret >= 0 {
                    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                    let dev_name = std::str::from_utf8(&name[..end]).unwrap_or("?");
                    info!("[touch] EVIOCGNAME = {:?}", dev_name);
                } else {
                    info!(
                        "[touch] EVIOCGNAME failed: {}",
                        std::io::Error::last_os_error()
                    );
                }

                // EVIOCGRAB grants exclusive access so no other process steals events.
                // If pspapp is still running and grabbed first, this returns EBUSY.
                // EVIOCGRAB = _IOW('E', 0x90, int) = 0x40044590
                const EVIOCGRAB: u64 = 0x40044590;
                let grab_ret = libc::ioctl(fd, EVIOCGRAB, 1usize);
                if grab_ret == 0 {
                    info!("[touch] EVIOCGRAB: exclusive access granted");
                } else {
                    let err = std::io::Error::last_os_error();
                    info!("[touch] EVIOCGRAB FAILED (errno={}): {} — another process may hold the device", err.raw_os_error().unwrap_or(0), err);
                }
            }

            Ok(Self {
                fd,
                sample: TouchSample::default(),
            })
        }
        #[cfg(feature = "simulate")]
        {
            info!("[touch] SIMULATE MODE — no real touch input");
            Ok(Self {
                sample: TouchSample::default(),
            })
        }
    }

    /// Read every pending event and return the latest touch in LVGL
    /// coordinates. Call once per loop iteration.
    pub fn poll(&mut self) -> TouchSample {
        #[cfg(not(feature = "simulate"))]
        // SAFETY: `fd` is the open, non-blocking evdev file; each read fills
        // at most one `InputEvent`.
        unsafe {
            let mut ev = std::mem::zeroed::<InputEvent>();
            let sz = std::mem::size_of::<InputEvent>();
            loop {
                let n = libc::read(self.fd, &mut ev as *mut _ as *mut libc::c_void, sz);
                if n < sz as isize {
                    break;
                }
                match ev.r#type {
                    EV_KEY if ev.code == BTN_TOUCH => self.sample.pressed = ev.value != 0,
                    EV_ABS if ev.code == ABS_MT_POSITION_X => self.sample.x = flip_x(ev.value),
                    EV_ABS if ev.code == ABS_MT_POSITION_Y => self.sample.y = flip_y(ev.value),
                    _ => {}
                }
            }
        }
        self.sample
    }
}

impl Drop for TouchInput {
    fn drop(&mut self) {
        #[cfg(not(feature = "simulate"))]
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_axes_flip_native_pixels() {
        // ILITEK_TDDI reports native pixels (0..399, 0..303), not 0..2047,
        // and the panel is mounted upside down like the display.
        assert_eq!((flip_x(0), flip_x(399)), (399, 0));
        assert_eq!((flip_y(0), flip_y(303)), (303, 0));
        assert_eq!((flip_x(200), flip_y(152)), (199, 151));
        assert_eq!((flip_x(2047), flip_y(-4)), (0, 303), "out-of-range values clamp");
    }
}
