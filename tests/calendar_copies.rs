//! The calendar rule has two copies: `crewboard_protocol::names::valid_civil_time` (the
//! timestamps of the wire) and `host_lock::clock::valid_civil_time` (`clock::parse_ts`). Neither
//! crate may depend on the other, so this test keeps them equal: for every day of the years 1899
//! to 2101, every month number from 0 to 13 and every day number from 0 to 32, at the edge
//! times of a day.

use crewboard::clock;
use crewboard::protocol::names;

/// The edge times: the first and last second of a day, and one past each limit.
const TIMES: &[(u32, u32, u32)] = &[
    (0, 0, 0),
    (23, 59, 59),
    (24, 0, 0),
    (0, 60, 0),
    (0, 0, 60),
    (12, 30, 30),
];

#[test]
fn both_copies_of_the_calendar_rule_agree_for_every_day_from_1899_to_2101() {
    let mut differ = Vec::new();
    let mut real_days = 0;
    for y in 1899..=2101 {
        for mo in 0..=13 {
            for d in 0..=32 {
                for &(h, mi, se) in TIMES {
                    let wire = names::valid_civil_time(y, mo, d, h, mi, se);
                    let host = clock::valid_civil_time(y, mo, d, h, mi, se);
                    if wire != host {
                        differ.push(format!(
                            "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}: wire {wire}, host {host}"
                        ));
                    }
                }
                real_days += u32::from(names::valid_civil_time(y, mo, d, 0, 0, 0));
            }
        }
    }
    assert!(
        differ.is_empty(),
        "the copies differ:\n{}",
        differ.join("\n")
    );
    // The loop must cover the whole range: 203 years of 365 days and 49 leap days (1900 and
    // 2100 are no leap years, 2000 is).
    assert_eq!(real_days, 203 * 365 + 49);
}

#[test]
fn the_parse_of_the_clock_uses_the_rule() {
    assert!(clock::parse_ts("2000-02-29T23:59:59Z").is_some());
    assert!(clock::parse_ts("1900-02-29T00:00:00Z").is_none());
    assert!(clock::parse_ts("2026-11-01T24:00:00Z").is_none());
}
