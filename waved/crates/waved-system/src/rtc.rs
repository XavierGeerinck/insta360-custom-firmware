//! The battery-backed clock: a BCD chip at 0x68 on /dev/i2c-3, driven the way
//! the stock firmware drives it (registers 0..=6: seconds, minutes, hours,
//! weekday, day, month, year since 2000; UTC). waved restores the system clock
//! from it at start-up and writes it after each network time sync.

use crate::civil::{civil_from_days, days_from_civil, month_len, weekday};
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};
use waved_state::CLOCK_VALID_AFTER_UNIX_S;

pub const RTC_BUS: &str = "/dev/i2c-3";
pub const RTC_ADDRESS: u16 = 0x68;
/// `I2C_SLAVE` from linux/i2c-dev.h.
const I2C_SLAVE: u64 = 0x0703;

pub fn bcd(value: u8) -> Option<u8> {
    let (high, low) = (value >> 4, value & 0x0f);
    (high <= 9 && low <= 9).then_some(high * 10 + low)
}

pub fn to_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

/// Registers 0..=6 → Unix seconds. `None` for anything that is not a real date.
pub fn decode(regs: [u8; 7]) -> Option<i64> {
    if regs[2] & 0x40 != 0 {
        return None; // 12-hour mode, which the stock firmware never writes
    }
    let second = bcd(regs[0] & 0x7f)?; // bit 7: oscillator halt (DS1307-style)
    let minute = bcd(regs[1] & 0x7f)?;
    let hour = bcd(regs[2] & 0x3f)?;
    let day = bcd(regs[4] & 0x3f)?;
    let month = bcd(regs[5] & 0x1f)?; // bit 7: century (DS3231-style)
    let year = 2000 + i64::from(bcd(regs[6])?);
    if second > 59 || minute > 59 || hour > 23 || !(1..=12).contains(&month) {
        return None;
    }
    if day == 0 || i64::from(day) > month_len(year, u32::from(month)) {
        return None;
    }
    let days = days_from_civil(year, u32::from(month), u32::from(day));
    Some(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

/// Unix seconds → registers 0..=6, for 2000..=2099 only.
pub fn encode(unix_s: i64) -> Option<[u8; 7]> {
    let days = unix_s.div_euclid(86_400);
    let secs = unix_s.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(2000..=2099).contains(&year) {
        return None;
    }
    Some([
        to_bcd((secs % 60) as u8),
        to_bcd((secs / 60 % 60) as u8),
        to_bcd((secs / 3_600) as u8),
        weekday(days) as u8,
        to_bcd(day as u8),
        to_bcd(month as u8),
        to_bcd((year - 2000) as u8),
    ])
}

pub trait Rtc: Send {
    /// The chip's time, or `None` when it holds no valid date.
    fn read(&mut self) -> io::Result<Option<i64>>;
    fn write(&mut self, unix_s: i64) -> io::Result<()>;
}

pub struct I2cRtc {
    file: std::fs::File,
}

impl I2cRtc {
    pub fn open(bus: &str, address: u16) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        let file = std::fs::OpenOptions::new().read(true).write(true).open(bus)?;
        // SAFETY: I2C_SLAVE takes the 7-bit slave address as its integer argument.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), I2C_SLAVE as _, libc::c_ulong::from(address)) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }
}

impl Rtc for I2cRtc {
    fn read(&mut self) -> io::Result<Option<i64>> {
        use std::io::{Read, Write};
        self.file.write_all(&[0])?;
        let mut regs = [0u8; 7];
        self.file.read_exact(&mut regs)?;
        Ok(decode(regs))
    }

    fn write(&mut self, unix_s: i64) -> io::Result<()> {
        use std::io::Write;
        let regs = encode(unix_s)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "time outside the RTC's 2000..2099 range"))?;
        let mut frame = [0u8; 8]; // register address 0, then the seven registers
        frame[1..].copy_from_slice(&regs);
        self.file.write_all(&frame)
    }
}

/// The system wall clock, behind a trait so tests can replace it.
pub trait WallClock: Send + Sync {
    fn now_unix_s(&self) -> i64;
    fn set(&self, unix_s: i64) -> io::Result<()>;
}

pub struct SystemClock;

impl WallClock for SystemClock {
    fn now_unix_s(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0)
    }

    #[allow(clippy::unnecessary_cast)] // time_t is i64 on the device but not on every host
    fn set(&self, unix_s: i64) -> io::Result<()> {
        let time = libc::timeval {
            tv_sec: unix_s as libc::time_t,
            tv_usec: 0,
        };
        // SAFETY: `time` is a valid timeval for the call; a null time zone is allowed.
        if unsafe { libc::settimeofday(&time, std::ptr::null()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// What to set the system clock to at start-up: the RTC's time, when the
/// system clock is not believable yet and the RTC's is.
pub fn restore_time(system_now: i64, rtc_now: Option<i64>) -> Option<i64> {
    if system_now >= CLOCK_VALID_AFTER_UNIX_S {
        return None;
    }
    rtc_now.filter(|time| *time >= CLOCK_VALID_AFTER_UNIX_S)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-05 14:32:07 UTC, a Monday.
    const OCT_5: i64 = 1_791_210_727;
    const OCT_5_REGS: [u8; 7] = [0x07, 0x32, 0x14, 1, 0x05, 0x10, 0x26];

    #[test]
    fn bcd_round_trips_and_rejects_non_decimal() {
        assert_eq!(bcd(0x59), Some(59));
        assert_eq!(bcd(0x00), Some(0));
        assert_eq!(bcd(0x5a), None);
        assert_eq!(bcd(0xa0), None);
        assert_eq!(to_bcd(59), 0x59);
    }

    #[test]
    fn rtc_registers_decode_like_the_stock_firmware() {
        assert_eq!(decode(OCT_5_REGS), Some(OCT_5));
        assert_eq!(encode(OCT_5), Some(OCT_5_REGS));
    }

    #[test]
    fn oscillator_and_century_bits_are_ignored() {
        let mut regs = OCT_5_REGS;
        regs[0] |= 0x80; // DS1307 clock-halt
        regs[5] |= 0x80; // DS3231 century
        assert_eq!(decode(regs), Some(OCT_5));
    }

    #[test]
    fn impossible_dates_are_not_a_time() {
        let with = |i: usize, v: u8| {
            let mut regs = OCT_5_REGS;
            regs[i] = v;
            regs
        };
        assert_eq!(decode(with(5, 0x13)), None, "month 13");
        assert_eq!(decode(with(5, 0x00)), None, "month 0");
        assert_eq!(decode([0x00, 0x00, 0x00, 0, 0x30, 0x02, 0x26]), None, "30 February");
        assert_eq!(decode(with(4, 0x00)), None, "day 0");
        assert_eq!(decode(with(2, 0x24)), None, "hour 24");
        assert_eq!(decode(with(2, 0x40 | 0x02)), None, "12-hour mode");
        assert_eq!(decode(with(1, 0x60)), None, "minute 60");
        assert_eq!(decode([0xff; 7]), None, "erased chip");
    }

    #[test]
    fn encode_refuses_times_outside_the_chip_range() {
        assert_eq!(encode(0), None, "1970");
        assert_eq!(encode(4_102_444_800), None, "2100");
        assert_eq!(decode(encode(946_684_800).unwrap()), Some(946_684_800), "2000-01-01");
    }

    #[test]
    fn the_rtc_restores_only_an_unset_system_clock() {
        assert_eq!(restore_time(0, Some(OCT_5)), Some(OCT_5));
        assert_eq!(restore_time(1_791_000_000, Some(OCT_5)), None, "a valid clock is kept");
        assert_eq!(restore_time(0, Some(100)), None, "an RTC that was never set");
        assert_eq!(restore_time(0, None), None);
    }

    #[test]
    fn the_system_clock_reads_a_believable_time_on_the_host() {
        assert!(SystemClock.now_unix_s() >= CLOCK_VALID_AFTER_UNIX_S);
    }
}
