//! Calendar arithmetic and RFC 3339 parsing for the screens: civil dates from
//! unix seconds and back (H. Hinnant's algorithms, proleptic Gregorian), and
//! the timestamps the services write with chrono's `to_rfc3339`.

pub fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days in `month` (1-12) of `year`.
pub fn month_len(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a civil date (negative before it).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month 1-12, day 1-31) for days since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// All-ASCII-digit field `range` of `text`, as a number.
fn digits(text: &str, range: std::ops::Range<usize>) -> Option<i64> {
    let part = text.get(range)?;
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// Seconds since the epoch for an RFC 3339 time: "2026-10-05T14:02:31Z",
/// "...T14:02:31.123456789+00:00" (chrono's `to_rfc3339`) or "...-01:00".
/// The offset is required; the date must exist (no 30 February).
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (
        digits(text, 0..4)?,
        digits(text, 5..7)?,
        digits(text, 8..10)?,
    );
    let (hour, minute, second) = (
        digits(text, 11..13)?,
        digits(text, 14..16)?,
        digits(text, 17..19)?,
    );
    if !(1..=12).contains(&month)
        || day < 1
        || day > i64::from(month_len(year, month as u32))
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = text.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let n = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        rest = &fraction[n..];
    }
    let offset_s = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let (h, m) = (digits(rest, 1..3)?, digits(rest, 4..6)?);
            if h > 23 || m > 59 {
                return None;
            }
            sign * (h * 3_600 + m * 60)
        }
    };
    Some(
        days_from_civil(year, month as u32, day as u32) * 86_400
            + hour * 3_600
            + minute * 60
            + second
            - offset_s,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leap_years_follow_the_gregorian_rule() {
        assert!(is_leap(2024));
        assert!(is_leap(2000));
        assert!(!is_leap(1900));
        assert!(!is_leap(2100));
        assert!(!is_leap(2025));
        assert_eq!(month_len(2024, 2), 29);
        assert_eq!(month_len(2025, 2), 28);
        assert_eq!(month_len(2025, 4), 30);
        assert_eq!(month_len(2025, 12), 31);
    }

    #[test]
    fn civil_dates_and_day_counts_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(20_365), (2025, 10, 4));
        assert_eq!(civil_from_days(20_731), (2026, 10, 5));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 1, 1), 20_454);
        for days in -800_000..=800_000 {
            let (y, m, d) = civil_from_days(days);
            assert!((1..=12).contains(&m) && (1..=31).contains(&d));
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }

    #[test]
    fn leap_days_and_month_ends_are_consecutive() {
        // 2024-02-28 -> 02-29 -> 03-01, but 2025 skips the 29th and 1900 too.
        let next = |y, m, d| civil_from_days(days_from_civil(y, m, d) + 1);
        assert_eq!(next(2024, 2, 28), (2024, 2, 29));
        assert_eq!(next(2024, 2, 29), (2024, 3, 1));
        assert_eq!(next(2025, 2, 28), (2025, 3, 1));
        assert_eq!(next(1900, 2, 28), (1900, 3, 1));
        assert_eq!(next(2000, 2, 28), (2000, 2, 29));
        assert_eq!(next(2025, 4, 30), (2025, 5, 1));
        assert_eq!(next(2025, 12, 31), (2026, 1, 1));
        assert_eq!(next(1969, 12, 31), (1970, 1, 1));
    }

    #[test]
    fn dates_before_2020_convert() {
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(
            civil_from_days(days_from_civil(1999, 12, 31)),
            (1999, 12, 31)
        );
        assert_eq!(civil_from_days(days_from_civil(1600, 2, 29)), (1600, 2, 29));
        assert_eq!(parse_rfc3339("1969-12-31T23:59:59Z"), Some(-1));
        assert_eq!(parse_rfc3339("2019-12-31T23:59:59Z"), Some(1_577_836_799));
    }

    #[test]
    fn rfc3339_accepts_chrono_output_and_offsets() {
        let t = 1_759_588_320; // 2025-10-04 14:32:00 UTC
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2025-10-04T14:32:00Z"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04t14:32:00z"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04 14:32:00Z"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04T14:32:00+00:00"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04T16:32:00+02:00"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04T13:32:00-01:00"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04T20:02:00+05:30"), Some(t));
        assert_eq!(parse_rfc3339("2025-10-04T09:32:00-05:00"), Some(t));
        assert_eq!(
            parse_rfc3339("2025-10-04T14:32:00.123456789+00:00"),
            Some(t)
        );
        assert_eq!(parse_rfc3339("2025-10-04T14:32:00.5Z"), Some(t));
        assert_eq!(parse_rfc3339("2026-10-05T15:20:00Z"), Some(1_791_213_600));
        // The offset can move the date across a month and a year end.
        assert_eq!(
            parse_rfc3339("2026-01-01T00:30:00+01:00"),
            parse_rfc3339("2025-12-31T23:30:00Z")
        );
    }

    #[test]
    fn rfc3339_checks_the_calendar() {
        assert!(parse_rfc3339("2024-02-29T00:00:00Z").is_some());
        assert_eq!(parse_rfc3339("2025-02-29T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("1900-02-29T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2025-04-31T00:00:00Z"), None);
        assert!(
            parse_rfc3339("2025-12-31T23:59:60Z").is_some(),
            "leap second"
        );
        assert_eq!(parse_rfc3339("2025-00-10T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2025-10-00T00:00:00Z"), None);
    }

    #[test]
    fn rfc3339_rejects_what_it_cannot_read() {
        for bad in [
            "",
            "2025-10-04",
            "2025-10-04T14:32:00",
            "2025-1x-04T14:32:00Z",
            "2025-13-04T14:32:00Z",
            "2025-10-04T24:00:00Z",
            "2025-10-04T14:60:00Z",
            "2025-10-04T14:32:61Z",
            "2025-10-04T14:32:00.Z",
            "2025-10-04T14:32:00+0200",
            "2025-10-04T14:32:00+24:00",
            "2025-10-04T14:32:00+02:60",
            "2025-10-04T14:32:00+02:00 junk",
            "+025-10-04T14:32:00Z",
            "2025-10-04T+4:32:00Z",
            "+é:00 garbage text here",
            "2025-10-04T14:32:00é",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad:?}");
        }
    }
}
