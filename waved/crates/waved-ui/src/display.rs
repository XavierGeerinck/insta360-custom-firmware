//! Framebuffer display driver for /dev/fb0 (RGB565, 400×304).
//!
//! Uses write() syscalls instead of mmap because the fb_ili79400 fbtft
//! driver's deferred-I/O does not reliably flush mmap writes to SPI.

use crate::bindings::*;
#[cfg(not(feature = "simulate"))]
use std::os::unix::io::RawFd;

pub const SCREEN_W: u32 = 400;
pub const SCREEN_H: u32 = 304;
pub const BYTES_PER_PIXEL: usize = 2; // RGB565
const STRIDE: usize = SCREEN_W as usize * BYTES_PER_PIXEL; // 800 bytes per row

pub struct FramebufferDisplay {
    #[cfg(not(feature = "simulate"))]
    fd: RawFd,
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
            let fb_size = (SCREEN_W * SCREEN_H) as usize * BYTES_PER_PIXEL;
            // Kick the fb_ili79400 SPI refresh using dd — the only method
            // proven to reliably wake the display at boot.
            let _ = std::process::Command::new("dd")
                .args(["if=/dev/zero", "of=/dev/fb0", "bs=4096", "count=60"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            std::thread::sleep(std::time::Duration::from_millis(200));
            // Open fd for flush writes
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/fb0")?;
            let fd = std::os::unix::io::IntoRawFd::into_raw_fd(file);
            // Turn on backlight
            let _ = std::fs::write(
                "/sys/class/leds/lcd-backlight/brightness",
                "255",
            );
            return Ok(Self { fd, buf1, buf2 });
        }

        #[cfg(feature = "simulate")]
        Ok(Self { buf1, buf2 })
    }

    /// Called by LVGL flush callback — writes the dirty area to /dev/fb0.
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
            let x1 = area.x1 as usize;
            let y1 = area.y1 as usize;
            let x2 = area.x2 as usize;
            let y2 = area.y2 as usize;
            let w = x2 - x1 + 1;
            let h = y2 - y1 + 1;
            // 180° rotation + byte-swap for the ILI79400 panel mounting:
            //  • Vertical flip  : LVGL row r → physical row (H-1-y1-r)
            //  • Horizontal flip: LVGL col c → physical col (W-1-x1-c)
            //  • Byte-swap      : fb_ili79400 reads RGB565 MSB-first
            let phys_x_start = SCREEN_W as usize - 1 - x2;
            let mut row_buf = vec![0u16; w];
            for r in 0..h {
                let phys_y = SCREEN_H as usize - 1 - (y1 + r);
                let src = color_p.add(r * w * BYTES_PER_PIXEL) as *const u16;
                // Reverse columns + byte-swap into temp buffer
                for c in 0..w {
                    row_buf[w - 1 - c] = (*src.add(c)).swap_bytes();
                }
                let offset = (phys_y * SCREEN_W as usize + phys_x_start) * BYTES_PER_PIXEL;
                libc::lseek(self.fd, offset as libc::off_t, libc::SEEK_SET);
                libc::write(self.fd, row_buf.as_ptr() as *const _, w * BYTES_PER_PIXEL);
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
            libc::close(self.fd);
        }
    }
}
