//! UTC time without a time crate: `YYYY-MM-DDTHH:MM:SSZ` (the only shape of the protocol)
//! to and from Unix seconds. Pure functions take the time as an argument; only `now_epoch`
//! and `now_nanos` read the wall clock and only `pause` sleeps, so a test can pass a fixed
//! clock. This module is the one place that may call `SystemTime::now` and `thread::sleep`
//! (`clippy.toml` disallows them everywhere else). `Instant::now` stays allowed where code
//! measures the real elapsed time of a child process or of a poll loop; each such site says so.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn now_epoch() -> i64 {
    #[allow(clippy::disallowed_methods, reason = "the wall clock itself")]
    let now = SystemTime::now();
    now.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The sub-second part of the wall clock in nanoseconds, for a name that must differ between two
/// runs in the same second (a temp dir, a lock suffix). 0 when the clock is before 1970.
pub fn now_nanos() -> u32 {
    #[allow(clippy::disallowed_methods, reason = "the wall clock itself")]
    let now = SystemTime::now();
    now.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos())
}

/// Block the thread for `d`. The one real sleep: [`Clock::sleep`] of the wall clock calls it, and
/// so does code that has no [`Clock`] to take (a retry pause, a poll between two reads of a child).
pub fn pause(d: Duration) {
    #[allow(clippy::disallowed_methods, reason = "the one real sleep")]
    std::thread::sleep(d);
}

pub fn now_ts() -> String {
    format_ts(now_epoch())
}

/// Split `90`, `90s`, `30m` or `2h` into the count and the seconds per unit. The count is
/// digits only (no sign, no fraction, no blank); leading zeros are decimal. This is the one
/// reader of the duration syntax; the functions below add their own limits.
fn split_unit(s: &str) -> Option<(u64, u64)> {
    let (num, mult) = match s.chars().last() {
        Some('s') => (&s[..s.len() - 1], 1),
        Some('m') => (&s[..s.len() - 1], 60),
        Some('h') => (&s[..s.len() - 1], 3600),
        _ => (s, 1),
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((num.parse().ok()?, mult))
}

/// `90`, `90s`, `10m`, `2h` -> seconds. The count is 1 to 86400.
pub fn parse_interval(s: &str) -> Result<i64, String> {
    let bad = || format!("bad heartbeat interval '{s}' (use 20s, 10m or 2h)");
    let (n, mult) = split_unit(s).ok_or_else(bad)?;
    if !(1..=86_400).contains(&n) {
        return Err(bad());
    }
    Ok(n as i64 * mult as i64)
}

/// `3600`, `90s`, `30m`, `2h`. At least 1 second.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let bad = || format!("bad duration '{s}' (use 90, 90s, 30m or 2h)");
    match parse_secs(s) {
        Some(n) if n > 0 => Ok(Duration::from_secs(n)),
        _ => Err(bad()),
    }
}

/// A source of time, so that code that waits can be tested without sleeping.
pub trait Clock {
    /// Unix seconds.
    fn now(&self) -> i64;
    /// Wait for `d`.
    fn sleep(&self, d: Duration);
}

/// The wall clock; `sleep` blocks the thread.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        now_epoch()
    }
    fn sleep(&self, d: Duration) {
        pause(d);
    }
}

/// A clock that a test moves by hand: `sleep` adds the time at once and returns.
#[cfg(test)]
pub struct FakeClock {
    millis: std::cell::Cell<i64>,
}

#[cfg(test)]
impl FakeClock {
    pub fn at(epoch: i64) -> Self {
        FakeClock {
            millis: std::cell::Cell::new(epoch * 1000),
        }
    }
    pub fn advance(&self, secs: i64) {
        self.millis.set(self.millis.get() + secs * 1000);
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now(&self) -> i64 {
        self.millis.get().div_euclid(1000)
    }
    fn sleep(&self, d: Duration) {
        self.millis.set(self.millis.get() + d.as_millis() as i64);
    }
}

/// `90`, `90s`, `30m`, `2h` -> seconds, `0` -> 0. Leading zeros are decimal (`08` is 8). `off`,
/// an empty value, a sign, a fraction and any other unit give `None`.
pub fn parse_secs(s: &str) -> Option<u64> {
    let (n, mult) = split_unit(s)?;
    n.checked_mul(mult)
}

/// `90` -> `90s`, `300` -> `5m`, `7200` -> `2h`, `200000` -> `2d`.
pub fn fmt_age(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=119 => format!("{s}s"),
        120..=7199 => format!("{}m", s / 60),
        7200..=172_799 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

// Days from civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Format Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_ts(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let rem = epoch.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` (also what GitHub returns). Anything else is `None`.
pub fn parse_ts(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let shape = b"dddd-dd-ddTdd:dd:ddZ";
    if b.len() != shape.len()
        || !b.iter().zip(shape).all(|(c, p)| match p {
            b'd' => c.is_ascii_digit(),
            other => c == other,
        })
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    let small = |v: i64| u32::try_from(v).ok();
    if !crate::base::valid_civil_time(
        small(y)?,
        small(mo)?,
        small(d)?,
        small(h)?,
        small(mi)?,
        small(se)?,
    ) {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_epochs() {
        assert_eq!(format_ts(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_ts(1_760_537_100), "2025-10-15T14:05:00Z");
        assert_eq!(parse_ts("2026-10-15T14:05:00Z"), Some(1_792_073_100));
        assert_eq!(format_ts(1_792_073_100), "2026-10-15T14:05:00Z");
    }

    #[test]
    fn round_trip_over_leap_days_and_year_ends() {
        for e in [
            0,
            86_399,
            951_782_400,   // 2000-02-29
            1_709_164_800, // 2024-02-29
            1_798_761_599, // 2026-12-31T23:59:59Z
            4_102_444_800, // 2100-01-01
        ] {
            assert_eq!(parse_ts(&format_ts(e)), Some(e), "{e}");
        }
    }

    #[test]
    fn a_date_that_does_not_exist_is_refused_and_every_other_one_round_trips() {
        for bad in [
            "2026-02-31T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-10-15T14:05:60Z",
            "2026-00-15T14:05:00Z",
            "2026-13-15T14:05:00Z",
            "2026-10-00T14:05:00Z",
        ] {
            assert_eq!(parse_ts(bad), None, "{bad}");
        }
        assert!(parse_ts("2028-02-29T00:00:00Z").is_some());
        // every day of a leap year and of a common year: format(parse(s)) == s
        for y in [2028, 2026, 2100] {
            for mo in 1..=12 {
                for d in 1..=31 {
                    let s = format!("{y:04}-{mo:02}-{d:02}T23:59:59Z");
                    if let Some(e) = parse_ts(&s) {
                        assert_eq!(format_ts(e), s);
                    }
                }
            }
        }
    }

    #[test]
    fn e5_lock_durations_take_seconds_and_units_and_nothing_else() {
        for (v, want) in [
            ("90", 90),
            ("90s", 90),
            ("30m", 1800),
            ("2h", 7200),
            ("0", 0),
            ("08", 8),
        ] {
            assert_eq!(parse_secs(v), Some(want), "{v}");
        }
        for bad in ["", "off", "2d", "m", "soon", "-5", "1.5s", " 5", "5 "] {
            assert_eq!(parse_secs(bad), None, "{bad}");
        }
    }

    #[test]
    fn duration_and_interval_read_the_same_syntax() {
        // every format the watch, monitor, config and heartbeat options took before
        for (v, secs) in [
            ("90", 90),
            ("90s", 90),
            ("30m", 1800),
            ("2h", 7200),
            ("08", 8),
        ] {
            assert_eq!(parse_duration(v), Ok(Duration::from_secs(secs)), "{v}");
            assert_eq!(parse_interval(v), Ok(secs as i64), "{v}");
        }
        assert_eq!(parse_interval("86400"), Ok(86_400));
        assert_eq!(parse_interval("86400h"), Ok(86_400 * 3600));
        // zero is refused by both; the interval has an upper bound on the count
        for bad in ["0", "0s", "0m"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
            assert!(parse_interval(bad).is_err(), "{bad}");
        }
        assert!(parse_interval("86401").is_err());
        assert!(parse_duration("86401").is_ok());
        for bad in ["", "m", "1d", "-5", "+5", "1.5h", "abc", " 5", "5 ", "off"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
            assert!(parse_interval(bad).is_err(), "{bad}");
        }
        // overflow is an error, not a panic or a wrap
        assert!(parse_duration("18446744073709551615h").is_err());
        assert_eq!(parse_secs("18446744073709551615h"), None);
        assert!(parse_duration("18446744073709551616").is_err());
        assert!(
            parse_duration("1x")
                .unwrap_err()
                .starts_with("bad duration '1x'")
        );
        assert!(
            parse_interval("1x")
                .unwrap_err()
                .starts_with("bad heartbeat interval '1x'")
        );
    }

    #[test]
    fn the_fake_clock_moves_only_when_told() {
        let c = FakeClock::at(100);
        assert_eq!(c.now(), 100);
        c.sleep(Duration::from_millis(1500));
        assert_eq!(c.now(), 101);
        c.advance(10);
        assert_eq!(c.now(), 111);
    }

    #[test]
    fn bad_shapes_are_refused() {
        for bad in [
            "",
            "2026-10-15 14:05:00Z",
            "2026-10-15T14:05:00",
            "2026-13-15T14:05:00Z",
            "2026-10-15T25:05:00Z",
            "2026-10-15T14:05:00+02:00",
        ] {
            assert_eq!(parse_ts(bad), None, "{bad}");
        }
    }
}
