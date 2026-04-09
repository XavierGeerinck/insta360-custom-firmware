//! Framebuffer display driver for /dev/fb0 (RGB565, 400×304).
//!
//! In simulate mode this is a no-op — the caller provides its own flush target.

use crate::bindings::*;
#[cfg(not(feature = "simulate"))]
use std::os::unix::io::RawFd;

pub const SCREEN_W: u32 = 400;
pub const SCREEN_H: u32 = 304;
pub const BYTES_PER_PIXEL: usize = 2; // RGB565

/// Manages the framebuffer mapping.
pub struct FramebufferDisplay {
    #[cfg(not(feature = "simulate"))]
    fb_ptr: *mut u8,
    #[cfg(not(feature = "simulate"))]
    fb_size: usize,
    #[cfg(not(feature = "simulate"))]
    _fd: RawFd,
    /// LVGL draw buffer (two half-screen buffers for DMA-style double-buffering).
    pub buf1: Box<[u8]>,
    pub buf2: Box<[u8]>,
}

impl FramebufferDisplay {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let buf_size = (SCREEN_W * SCREEN_H / 2) as usize * BYTES_PER_PIXEL;
        let buf1 = vec![0u8; buf_size].into_boxed_slice();
        let buf2 = vec![0u8; buf_size].into_boxed_slice();

        #[cfg(not(feature = "simulate"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/fb0")?;
            let fd = std::os::unix::io::IntoRawFd::into_raw_fd(file);
            let fb_size = (SCREEN_W * SCREEN_H) as usize * BYTES_PER_PIXEL;
            let fb_ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    fb_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                ) as *mut u8
            };
            if fb_ptr == libc::MAP_FAILED as *mut u8 {
                return Err("mmap /dev/fb0 failed".into());
            }
            // Turn on backlight
            let _ = std::fs::write(
                "/sys/class/leds/lcd-backlight/brightness",
                "255",
            );
            return Ok(Self { fb_ptr, fb_size, _fd: fd, buf1, buf2 });
        }

        #[cfg(feature = "simulate")]
        Ok(Self { buf1, buf2 })
    }

    /// Called by LVGL flush callback — copies the dirty area to /dev/fb0.
    ///
    /// # Safety
    /// `color_p` must point to at least `area.width * area.height` RGB565 pixels.
    pub unsafe fn flush(
        &self,
        area: &lv_area_t,
        color_p: *const u8,
    ) {
        #[cfg(not(feature = "simulate"))]
        {
            debug_assert!(area.x1 >= 0 && area.y1 >= 0 && area.x2 >= 0 && area.y2 >= 0,
                "LVGL flush area has negative coordinate: {:?}", (area.x1, area.y1, area.x2, area.y2));
            let x1 = area.x1 as usize;
            let y1 = area.y1 as usize;
            let x2 = area.x2 as usize;
            let y2 = area.y2 as usize;
            let w = x2 - x1 + 1;
            for row in y1..=y2 {
                let dst = self.fb_ptr.add((row * SCREEN_W as usize + x1) * BYTES_PER_PIXEL);
                let src = color_p.add((row - y1) * w * BYTES_PER_PIXEL);
                std::ptr::copy_nonoverlapping(src, dst, w * BYTES_PER_PIXEL);
            }
        }
        #[cfg(feature = "simulate")]
        let _ = (area, color_p);
    }
}

impl Drop for FramebufferDisplay {
    fn drop(&mut self) {
        #[cfg(not(feature = "simulate"))]
        unsafe {
            libc::munmap(self.fb_ptr as *mut _, self.fb_size);
            libc::close(self._fd);
        }
    }
}
