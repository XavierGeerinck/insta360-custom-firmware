//! Proleptic Gregorian calendar arithmetic on days since 1970-01-01
//! (Howard Hinnant's algorithms), shared by the time zone rules and the RTC.

pub(crate) fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

pub(crate) fn month_len(year: i64, month: u32) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month 1..=12, day 1..=31) for days since 1970-01-01.
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

/// 0 = Sunday.
pub(crate) fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates_map_to_their_day_numbers() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 1, 1), 20_454);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(civil_from_days(20_731), (2026, 10, 5));
    }

    #[test]
    fn day_numbers_round_trip() {
        for days in (-1_000..40_000).step_by(37) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }

    #[test]
    fn weekdays_and_month_lengths() {
        assert_eq!(weekday(0), 4, "1970-01-01 was a Thursday");
        assert_eq!(weekday(days_from_civil(2026, 10, 5)), 1, "Monday");
        assert_eq!((month_len(2024, 2), month_len(2026, 2), month_len(2100, 2)), (29, 28, 28));
        assert_eq!((month_len(2026, 4), month_len(2026, 12)), (30, 31));
        assert!(is_leap(2000) && !is_leap(1900));
    }
}
