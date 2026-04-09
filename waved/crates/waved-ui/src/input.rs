//! Evdev touch input driver for /dev/input/event0.
//!
//! Reads ABS_X / ABS_Y + BTN_TOUCH events and converts to LVGL lv_indev_data_t.

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
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

// Evdev axis ranges for the ilitek touch controller on the Wave.
// These were measured from /dev/input/event0 on device.
// If touch coordinates are off, adjust these constants.
const TOUCH_X_MIN: i32 = 0;
const TOUCH_X_MAX: i32 = 4095;
const TOUCH_Y_MIN: i32 = 0;
const TOUCH_Y_MAX: i32 = 4095;

pub struct TouchInput {
    #[cfg(not(feature = "simulate"))]
    fd: std::os::unix::io::RawFd,
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

impl TouchInput {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(not(feature = "simulate"))]
        {
            use std::os::unix::io::IntoRawFd;
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open("/dev/input/event0")?;
            let fd = file.into_raw_fd();
            return Ok(Self { fd, x: 0, y: 0, pressed: false });
        }
        #[cfg(feature = "simulate")]
        Ok(Self { x: 0, y: 0, pressed: false })
    }

    /// Poll for pending events. Call this once per LVGL tick before `lv_timer_handler`.
    pub fn poll(&mut self) {
        #[cfg(not(feature = "simulate"))]
        unsafe {
            let mut ev = std::mem::zeroed::<InputEvent>();
            let sz = std::mem::size_of::<InputEvent>();
            loop {
                let n = libc::read(self.fd, &mut ev as *mut _ as *mut libc::c_void, sz);
                if n < sz as isize { break; }
                match ev.r#type {
                    EV_KEY if ev.code == BTN_TOUCH => {
                        self.pressed = ev.value != 0;
                    }
                    EV_ABS if ev.code == ABS_X => {
                        self.x = (ev.value - TOUCH_X_MIN) * 400
                            / (TOUCH_X_MAX - TOUCH_X_MIN).max(1);
                    }
                    EV_ABS if ev.code == ABS_Y => {
                        self.y = (ev.value - TOUCH_Y_MIN) * 304
                            / (TOUCH_Y_MAX - TOUCH_Y_MIN).max(1);
                    }
                    _ => {}
                }
            }
        }
    }
}

impl Drop for TouchInput {
    fn drop(&mut self) {
        #[cfg(not(feature = "simulate"))]
        unsafe { libc::close(self.fd); }
    }
}
