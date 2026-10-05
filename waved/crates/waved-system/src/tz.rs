//! Time zone offsets from the device's TZif files (/usr/share/zoneinfo). The
//! device ships the slim format: a few historic transitions, then a POSIX TZ
//! footer that carries the rules for every later date. No database is
//! embedded in the binary.

use crate::civil::{civil_from_days, days_from_civil, is_leap, month_len, weekday};
use std::path::Path;

pub const ZONEINFO_DIR: &str = "/usr/share/zoneinfo";

#[derive(Debug, thiserror::Error)]
pub enum TzError {
    #[error("time zone name {0:?} is not a plain IANA name")]
    BadName(String),
    #[error("cannot read the time zone file: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad TZif data: {0}")]
    Format(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimeZone {
    transitions: Vec<i64>,
    /// Index into `offsets` for each transition.
    kinds: Vec<u8>,
    /// UTC offset in seconds of each local time type.
    offsets: Vec<i32>,
    footer: Option<PosixTz>,
}

impl TimeZone {
    pub fn utc() -> Self {
        Self {
            transitions: Vec::new(),
            kinds: Vec::new(),
            offsets: vec![0],
            footer: None,
        }
    }

    /// `zoneinfo/name`. "UTC" needs no file. Names with `..`, empty parts or
    /// anything but `[A-Za-z0-9_+-]` are refused before any file is opened.
    pub fn load(zoneinfo: &Path, name: &str) -> Result<Self, TzError> {
        if name == "UTC" {
            return Ok(Self::utc());
        }
        if !plain_name(name) {
            return Err(TzError::BadName(name.into()));
        }
        Self::parse(&std::fs::read(zoneinfo.join(name))?)
    }

    /// RFC 8536 TZif, version 1 (32-bit times) or 2+ (64-bit times and footer).
    pub fn parse(data: &[u8]) -> Result<Self, TzError> {
        let first = Header::read(data)?;
        let (head, body, time_size) = if first.version >= b'2' {
            let second_at = 44 + first.data_len(4);
            let second = Header::read(data.get(second_at..).ok_or(TzError::Format("truncated v1 block"))?)?;
            (second, second_at + 44, 8)
        } else {
            (first, 44, 4)
        };
        let end = body + head.data_len(time_size);
        let block = data.get(body..end).ok_or(TzError::Format("truncated data block"))?;
        if head.typecnt == 0 {
            return Err(TzError::Format("no local time types"));
        }
        let mut at = 0;
        let mut transitions = Vec::with_capacity(head.timecnt);
        for _ in 0..head.timecnt {
            transitions.push(if time_size == 8 {
                i64::from_be_bytes(block[at..at + 8].try_into().expect("8 bytes"))
            } else {
                i64::from(i32::from_be_bytes(block[at..at + 4].try_into().expect("4 bytes")))
            });
            at += time_size;
        }
        let kinds = block[at..at + head.timecnt].to_vec();
        at += head.timecnt;
        let mut offsets = Vec::with_capacity(head.typecnt);
        for _ in 0..head.typecnt {
            offsets.push(i32::from_be_bytes(block[at..at + 4].try_into().expect("4 bytes")));
            at += 6;
        }
        if kinds.iter().any(|kind| usize::from(*kind) >= head.typecnt) {
            return Err(TzError::Format("transition type out of range"));
        }
        if transitions.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(TzError::Format("transitions out of order"));
        }
        let footer = if time_size == 8 { parse_footer(&data[end..])? } else { None };
        Ok(Self {
            transitions,
            kinds,
            offsets,
            footer,
        })
    }

    /// Seconds east of UTC at `unix_s`.
    pub fn offset_at(&self, unix_s: i64) -> i32 {
        let n = self.transitions.partition_point(|at| *at <= unix_s);
        if n == self.transitions.len() {
            if let Some(footer) = &self.footer {
                return footer.offset_at(unix_s);
            }
        }
        if n == 0 {
            return self.offsets[0];
        }
        self.offsets[usize::from(self.kinds[n - 1])]
    }
}

fn plain_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
        })
}

struct Header {
    version: u8,
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

impl Header {
    fn read(data: &[u8]) -> Result<Self, TzError> {
        if data.len() < 44 || &data[..4] != b"TZif" {
            return Err(TzError::Format("missing TZif header"));
        }
        let count = |i: usize| u32::from_be_bytes(data[20 + 4 * i..24 + 4 * i].try_into().expect("4 bytes")) as usize;
        Ok(Self {
            version: data[4],
            isutcnt: count(0),
            isstdcnt: count(1),
            leapcnt: count(2),
            timecnt: count(3),
            typecnt: count(4),
            charcnt: count(5),
        })
    }

    fn data_len(&self, time_size: usize) -> usize {
        self.timecnt * time_size
            + self.timecnt
            + self.typecnt * 6
            + self.charcnt
            + self.leapcnt * (time_size + 4)
            + self.isstdcnt
            + self.isutcnt
    }
}

/// The bytes after a v2+ data block must be exactly `\n<tz>\n`; an empty
/// `<tz>` means no rules. Anything shorter is a damaged file, not a zone
/// without daylight saving time.
fn parse_footer(rest: &[u8]) -> Result<Option<PosixTz>, TzError> {
    let text = std::str::from_utf8(rest).map_err(|_| TzError::Format("footer is not text"))?;
    let tz = text
        .strip_prefix('\n')
        .and_then(|inner| inner.strip_suffix('\n'))
        .filter(|tz| !tz.contains('\n'))
        .ok_or(TzError::Format("truncated footer"))?;
    if tz.is_empty() {
        return Ok(None);
    }
    PosixTz::parse(tz).map(Some).ok_or(TzError::Format("unsupported TZ footer"))
}

/// A POSIX TZ string: `std offset [dst [offset] ,start[/time],end[/time]]`.
#[derive(Clone, Debug, PartialEq)]
struct PosixTz {
    /// Seconds east of UTC (POSIX writes the opposite sign).
    std_offset: i32,
    dst: Option<Dst>,
}

#[derive(Clone, Debug, PartialEq)]
struct Dst {
    offset: i32,
    start: RuleDate,
    /// Local standard time of day, in seconds; may exceed a day or be negative.
    start_time: i32,
    end: RuleDate,
    /// Local daylight time of day, in seconds.
    end_time: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RuleDate {
    /// `Mm.w.d`: weekday `d` (0 = Sunday) of week `w` (5 = last) of month `m`.
    MonthWeekDay { month: u32, week: u32, weekday: u32 },
    /// `Jn`: day 1..=365, never counting 29 February.
    JulianNoLeap(u32),
    /// `n`: day 0..=365, counting 29 February.
    ZeroBased(u32),
}

impl RuleDate {
    /// The rule's date in `year`, as days since 1970-01-01.
    fn day(self, year: i64) -> i64 {
        match self {
            RuleDate::MonthWeekDay { month, week, weekday: wanted } => {
                let first = days_from_civil(year, month, 1);
                let offset = (i64::from(wanted) - i64::from(weekday(first))).rem_euclid(7);
                let mut day = 1 + offset + 7 * (i64::from(week) - 1);
                while day > month_len(year, month) {
                    day -= 7;
                }
                first + day - 1
            }
            RuleDate::JulianNoLeap(n) => {
                let n = i64::from(n);
                days_from_civil(year, 1, 1) + n - 1 + i64::from(is_leap(year) && n >= 60)
            }
            RuleDate::ZeroBased(n) => days_from_civil(year, 1, 1) + i64::from(n),
        }
    }
}

impl PosixTz {
    fn parse(text: &str) -> Option<Self> {
        let mut c = Cursor { s: text.as_bytes(), i: 0 };
        c.name()?;
        let std_offset = -c.time(24)?;
        if c.done() {
            return Some(Self { std_offset, dst: None });
        }
        c.name()?;
        let offset = if c.peek() == Some(b',') { std_offset + 3_600 } else { -c.time(24)? };
        if !c.eat(b',') {
            return None;
        }
        let (start, start_time) = c.rule()?;
        if !c.eat(b',') {
            return None;
        }
        let (end, end_time) = c.rule()?;
        c.done().then_some(Self {
            std_offset,
            dst: Some(Dst {
                offset,
                start,
                start_time,
                end,
                end_time,
            }),
        })
    }

    fn offset_at(&self, unix_s: i64) -> i32 {
        let Some(dst) = &self.dst else {
            return self.std_offset;
        };
        let (year, _, _) = civil_from_days((unix_s + i64::from(self.std_offset)).div_euclid(86_400));
        let start = dst.start.day(year) * 86_400 + i64::from(dst.start_time) - i64::from(self.std_offset);
        let end = dst.end.day(year) * 86_400 + i64::from(dst.end_time) - i64::from(dst.offset);
        let in_dst = if start < end {
            start <= unix_s && unix_s < end
        } else {
            !(end <= unix_s && unix_s < start)
        };
        if in_dst {
            dst.offset
        } else {
            self.std_offset
        }
    }
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.i += 1;
        }
        found
    }

    fn done(&self) -> bool {
        self.i == self.s.len()
    }

    /// One to three digits, at most `max`.
    fn number(&mut self, max: u32) -> Option<u32> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == start || self.i - start > 3 {
            return None;
        }
        let n: u32 = std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()?;
        (n <= max).then_some(n)
    }

    /// A zone abbreviation: three or more letters, or anything between < and >.
    fn name(&mut self) -> Option<()> {
        if self.eat(b'<') {
            while let Some(c) = self.peek() {
                self.i += 1;
                if c == b'>' {
                    return Some(());
                }
            }
            return None;
        }
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        (self.i - start >= 3).then_some(())
    }

    /// `[+-]hh[:mm[:ss]]` in seconds. `max_hours` is 24 for offsets and 167 for rule times.
    fn time(&mut self, max_hours: u32) -> Option<i32> {
        let sign = if self.eat(b'-') {
            -1
        } else {
            self.eat(b'+');
            1
        };
        let mut seconds = self.number(max_hours)? as i32 * 3_600;
        if self.eat(b':') {
            seconds += self.number(59)? as i32 * 60;
            if self.eat(b':') {
                seconds += self.number(59)? as i32;
            }
        }
        Some(sign * seconds)
    }

    fn rule(&mut self) -> Option<(RuleDate, i32)> {
        let date = if self.eat(b'M') {
            let month = self.number(12).filter(|m| *m >= 1)?;
            if !self.eat(b'.') {
                return None;
            }
            let week = self.number(5).filter(|w| *w >= 1)?;
            if !self.eat(b'.') {
                return None;
            }
            RuleDate::MonthWeekDay {
                month,
                week,
                weekday: self.number(6)?,
            }
        } else if self.eat(b'J') {
            RuleDate::JulianNoLeap(self.number(365).filter(|n| *n >= 1)?)
        } else {
            RuleDate::ZeroBased(self.number(365)?)
        };
        let time = if self.eat(b'/') { self.time(167)? } else { 7_200 };
        Some((date, time))
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    /// A TZif v2 file as `zic -b slim` writes it: a minimal v1 block, then the
    /// v2 transitions, local time types and the footer.
    pub(crate) fn tzif(transitions: &[(i64, u8)], offsets: &[(i32, bool)], footer: &str) -> Vec<u8> {
        fn header(out: &mut Vec<u8>, timecnt: usize, typecnt: usize, charcnt: usize) {
            out.extend_from_slice(b"TZif2");
            out.extend_from_slice(&[0; 15]);
            for count in [0, 0, 0, timecnt, typecnt, charcnt] {
                out.extend_from_slice(&(count as u32).to_be_bytes());
            }
        }
        let mut out = Vec::new();
        header(&mut out, 0, 1, 1);
        out.extend_from_slice(&[0; 7]); // one v1 type (6 bytes) and one designation byte
        header(&mut out, transitions.len(), offsets.len(), 1);
        for (at, _) in transitions {
            out.extend_from_slice(&at.to_be_bytes());
        }
        for (_, kind) in transitions {
            out.push(*kind);
        }
        for (offset, dst) in offsets {
            out.extend_from_slice(&offset.to_be_bytes());
            out.push(u8::from(*dst));
            out.push(0);
        }
        out.push(0);
        out.push(b'\n');
        out.extend_from_slice(footer.as_bytes());
        out.push(b'\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::tzif;
    use super::*;

    fn footer_only(std_offset: i32, footer: &str) -> TimeZone {
        TimeZone::parse(&tzif(&[], &[(std_offset, false)], footer)).unwrap()
    }

    const JAN_15_2026: i64 = 1_768_478_400; // 12:00 UTC
    const JUL_15_2026: i64 = 1_784_116_800; // 12:00 UTC

    #[test]
    fn brussels_switches_at_the_eu_dates() {
        let zone = footer_only(3_600, "CET-1CEST,M3.5.0,M10.5.0/3");
        assert_eq!(zone.offset_at(JAN_15_2026), 3_600);
        assert_eq!(zone.offset_at(JUL_15_2026), 7_200);
        // 2026-03-29 01:00 UTC and 2026-10-25 01:00 UTC.
        assert_eq!(zone.offset_at(1_774_745_999), 3_600);
        assert_eq!(zone.offset_at(1_774_746_000), 7_200);
        assert_eq!(zone.offset_at(1_792_889_999), 7_200);
        assert_eq!(zone.offset_at(1_792_890_000), 3_600);
    }

    #[test]
    fn southern_and_western_zones() {
        let sydney = footer_only(36_000, "AEST-10AEDT,M10.1.0,M4.1.0/3");
        assert_eq!(sydney.offset_at(JAN_15_2026), 39_600);
        assert_eq!(sydney.offset_at(JUL_15_2026), 36_000);
        let new_york = footer_only(-18_000, "EST5EDT,M3.2.0,M11.1.0");
        assert_eq!(new_york.offset_at(JAN_15_2026), -18_000);
        assert_eq!(new_york.offset_at(JUL_15_2026), -14_400);
        assert_eq!(footer_only(19_800, "IST-5:30").offset_at(JUL_15_2026), 19_800);
        assert_eq!(footer_only(-10_800, "<-03>3").offset_at(JUL_15_2026), -10_800);
        assert_eq!(footer_only(20_700, "<+0545>-5:45").offset_at(JUL_15_2026), 20_700);
        assert_eq!(footer_only(0, "UTC0").offset_at(JUL_15_2026), 0);
    }

    #[test]
    fn rule_times_past_midnight_and_julian_days() {
        // Asia/Jerusalem: the 4th Thursday of March at 26:00 = Friday 02:00 local.
        let jerusalem = footer_only(7_200, "IST-2IDT,M3.4.4/26,M10.5.0");
        assert_eq!(jerusalem.offset_at(1_774_569_599), 7_200); // 2026-03-26 23:59:59 UTC
        assert_eq!(jerusalem.offset_at(1_774_569_600), 10_800);
        // Jn ignores 29 February: J60 is 1 March even in 2024.
        let julian = footer_only(0, "AAA0BBB,J60/0,J300/0");
        assert_eq!(julian.offset_at(1_709_251_199), 0);
        assert_eq!(julian.offset_at(1_709_251_200), 3_600); // 2024-03-01 00:00 UTC
        // n counts from 0 and includes 29 February.
        let zero_based = footer_only(0, "AAA0BBB,59/0,299/0");
        assert_eq!(zero_based.offset_at(1_709_164_799), 0);
        assert_eq!(zero_based.offset_at(1_709_164_800), 3_600); // 2024-02-29 00:00 UTC
    }

    #[test]
    fn transitions_before_the_footer_win() {
        let table = TimeZone::parse(&tzif(&[(1_000_000_000, 1)], &[(0, false), (7_200, false)], "")).unwrap();
        assert_eq!(table.offset_at(999_999_999), 0, "type 0 before the first transition");
        assert_eq!(table.offset_at(1_000_000_000), 7_200);
        assert_eq!(table.offset_at(2_000_000_000), 7_200, "no footer: the last type holds");

        let both = TimeZone::parse(&tzif(&[(0, 1)], &[(1_000, false), (3_600, false)], "CET-1CEST,M3.5.0,M10.5.0/3")).unwrap();
        assert_eq!(both.offset_at(-1), 1_000);
        assert_eq!(both.offset_at(JUL_15_2026), 7_200, "the footer covers later dates");
    }

    #[test]
    fn hostile_names_and_files_are_rejected() {
        let dir = Path::new("/usr/share/zoneinfo");
        for name in ["../etc/passwd", "/etc/passwd", "Europe/../../etc", "Europe//Brussels", "", "Europe/Brus sels"] {
            assert!(matches!(TimeZone::load(dir, name), Err(TzError::BadName(_))), "{name:?}");
        }
        assert!(TimeZone::parse(b"nope").is_err());
        let good = tzif(&[], &[(3_600, false)], "CET-1");
        assert!(TimeZone::parse(&good[..good.len() - 20]).is_err(), "truncated");
        assert!(TimeZone::parse(&tzif(&[(0, 5)], &[(0, false)], "")).is_err(), "type index out of range");
        assert!(TimeZone::parse(&tzif(&[(5, 0), (1, 0)], &[(0, false)], "")).is_err(), "out of order");
        assert!(TimeZone::parse(&tzif(&[], &[(0, false)], "garbage!!")).is_err(), "bad footer");
        assert!(TimeZone::parse(&tzif(&[], &[], "UTC0")).is_err(), "no local time types");
    }

    #[test]
    fn a_cut_or_damaged_footer_is_an_error_never_a_wrong_zone() {
        let full = tzif(&[], &[(3_600, false)], "CET-1CEST,M3.5.0,M10.5.0/3");
        let want = TimeZone::parse(&full).unwrap();
        assert_eq!(want.offset_at(JUL_15_2026), 7_200);
        for len in 0..full.len() {
            if let Ok(zone) = TimeZone::parse(&full[..len]) {
                assert_eq!(zone, want, "a {len}-byte prefix parsed as a different zone");
            }
        }
        assert!(TimeZone::parse(&full[..full.len() - 1]).is_err(), "closing newline missing");
        let junk = &full[..full.len() - 28]; // the data block plus nothing
        assert!(TimeZone::parse(junk).is_err(), "no footer at all");
        let mut no_lead = junk.to_vec();
        no_lead.extend_from_slice(b"junk\n");
        assert!(TimeZone::parse(&no_lead).is_err(), "footer without its leading newline");
        let mut two_lines = full.clone();
        two_lines.extend_from_slice(b"extra\n");
        assert!(TimeZone::parse(&two_lines).is_err(), "text after the footer");
        assert!(TimeZone::parse(&tzif(&[], &[(0, false)], "")).unwrap().footer.is_none(), "an empty footer is allowed");
    }

    #[test]
    fn utc_needs_no_file_and_load_reads_the_named_file() {
        assert_eq!(TimeZone::load(Path::new("/nonexistent"), "UTC").unwrap().offset_at(JUL_15_2026), 0);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Europe")).unwrap();
        std::fs::write(dir.path().join("Europe/Brussels"), tzif(&[], &[(3_600, false)], "CET-1CEST,M3.5.0,M10.5.0/3")).unwrap();
        assert_eq!(TimeZone::load(dir.path(), "Europe/Brussels").unwrap().offset_at(JUL_15_2026), 7_200);
        assert!(matches!(TimeZone::load(dir.path(), "Mars/Olympus"), Err(TzError::Io(_))));
    }
}
