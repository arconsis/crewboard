//! The rules of `crewboard wake` that need no file, process or clock of their own: how a value
//! is read, how long a pause is, when a quiet window ends, when a stop is lifted. Every function
//! takes the time as an argument.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Seconds between two wakes for owned work, when nothing else is set.
pub const DEFAULT_WORK_BACKOFF: u64 = 900;
/// Delays before the retries of a call that exited with 3.
pub const DEFAULT_RETRY_DELAYS: &str = "10 30";
/// A wake lock that is held for longer than this is stale.
pub const DEFAULT_STALE_AFTER: i64 = 14_400;
/// A lock directory without a pid file is stale after this many seconds.
pub const DEFAULT_EMPTY_GRACE: i64 = 30;
/// The longest line of the first stderr line of a call, in characters.
pub const ERR_CUT: usize = 200;
/// The longest line of a tick, in characters.
pub const TICK_CUT: usize = 400;
/// A quiet window ends after this many seconds.
pub const QUIET_WINDOW: i64 = 3600;
/// The run of failed runs in a row at which the owner is told.
pub const NET_OWNER_RUN: u64 = 3;

/// Whole seconds: one or more digits and nothing else.
pub fn parse_whole_secs(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `--work-backoff`: whole seconds, or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBackoff {
    Off,
    Secs(u64),
}

impl WorkBackoff {
    pub fn parse(s: &str) -> Option<WorkBackoff> {
        if s == "off" {
            return Some(WorkBackoff::Off);
        }
        parse_whole_secs(s).map(WorkBackoff::Secs)
    }
}

/// `--max-wake`: `90`, `90s`, `45m`, `2h`; `off` and `0` give 0 (no budget).
#[cfg(test)]
pub fn parse_max_wake(s: &str) -> Option<u64> {
    if s == "off" {
        return Some(0);
    }
    crate::clock::parse_secs(s)
}

/// `--retry-delays`: whole seconds, separated by blanks. An empty list means no retry.
pub fn parse_delays(s: &str) -> Option<Vec<u64>> {
    s.split_whitespace().map(parse_whole_secs).collect()
}

/// Control characters (`\0` to `\x1f`, `\x7f`) out, then cut to `max` characters.
pub fn clean_line(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| (*c as u32) >= 0x20 && *c != '\u{7f}')
        .take(max)
        .collect()
}

/// The first line of the stderr of a call that is not blank, cleaned and cut to 200 characters.
/// `(no stderr)` when there is none.
pub fn first_stderr_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty());
    let clean = line.map(|l| clean_line(l, ERR_CUT)).unwrap_or_default();
    if clean.is_empty() {
        "(no stderr)".to_string()
    } else {
        clean
    }
}

/// The first ticket of a `mine` output: the first `In Progress` line, else the first line that
/// starts with `<repo>#<n> `. Its first word.
pub fn first_ticket(mine: &str) -> Option<String> {
    let word = |l: &str| l.split_whitespace().next().map(str::to_string);
    let t = mine
        .lines()
        .find(|l| l.contains("In Progress"))
        .and_then(word)
        .or_else(|| mine.lines().find(|l| starts_with_ticket(l)).and_then(word));
    t.filter(|t| has_ticket_ref(t))
}

/// `^[^ ]+#[0-9]+ `
fn starts_with_ticket(l: &str) -> bool {
    let Some(first) = l.split(' ').next() else {
        return false;
    };
    // a space must follow the first word
    if first.len() == l.len() || first.is_empty() {
        return false;
    }
    first.char_indices().any(|(i, c)| {
        c == '#' && i > 0 && {
            let digits = &first[i + 1..];
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        }
    })
}

/// A `#` that is followed by a digit.
fn has_ticket_ref(t: &str) -> bool {
    t.match_indices('#').any(|(i, _)| {
        t[i + 1..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit())
    })
}

// --- net back-off ----------------------------------------------------------------------------

/// The file `net-backoff`: `<failed runs in a row> <epoch before which runs are skipped>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetBackoff {
    pub fails: u64,
    pub until: i64,
}

impl NetBackoff {
    /// A field that is missing or not a number counts as 0.
    pub fn parse(text: &str) -> NetBackoff {
        let line = text.lines().next().unwrap_or("").trim();
        let (a, rest) = match line.split_once(char::is_whitespace) {
            Some((a, r)) => (a, r.trim()),
            None => (line, ""),
        };
        NetBackoff {
            fails: parse_whole_secs(a).unwrap_or(0),
            until: parse_whole_secs(rest)
                .and_then(|n| i64::try_from(n).ok())
                .unwrap_or(0),
        }
    }

    pub fn render(&self) -> String {
        format!("{} {}\n", self.fails, self.until)
    }

    /// Are the runs skipped now?
    pub fn pauses(&self, now: i64) -> bool {
        now < self.until
    }
}

/// The pause after `fails` failed runs in a row: none for the first, 300 s for the second,
/// doubled for each run after that, at most 3600 s.
pub fn net_pause_secs(fails: u64) -> i64 {
    if fails < 2 {
        return 0;
    }
    let mut pause: i64 = 300;
    let mut i = 2;
    while i < fails && pause < 3600 {
        pause *= 2;
        i += 1;
    }
    pause.min(3600)
}

/// The back-off after one more failed run (exit 3 after the retries).
pub fn net_failed(prev: NetBackoff, now: i64) -> NetBackoff {
    let fails = prev.fails.saturating_add(1);
    NetBackoff {
        fails,
        until: now + net_pause_secs(fails),
    }
}

/// The line for the owner, said at the third failed run in a row.
pub fn net_owner_text(fails: u64) -> String {
    format!(
        "GitHub or network error in {fails} runs in a row; the owner should check the network and the token. The next runs pause up to 1 h"
    )
}

// --- quiet counter ---------------------------------------------------------------------------

/// The file `<log>.quiet`: `<window start epoch> <window start ts> <polls>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quiet {
    pub start: i64,
    pub start_ts: String,
    pub n: u64,
}

impl Quiet {
    /// `None` for a file that is broken: it starts a new window.
    pub fn parse(text: &str) -> Option<Quiet> {
        let line = text.lines().next()?.trim();
        let (start, rest) = line.split_once(char::is_whitespace)?;
        let (ts, n) = rest.trim_start().split_once(char::is_whitespace)?;
        let start = parse_whole_secs(start).and_then(|n| i64::try_from(n).ok())?;
        let n = parse_whole_secs(n.trim())?;
        if ts.is_empty() {
            return None;
        }
        Some(Quiet {
            start,
            start_ts: ts.to_string(),
            n,
        })
    }

    pub fn render(&self) -> String {
        format!("{} {} {}\n", self.start, self.start_ts, self.n)
    }
}

/// Count one quiet poll. Returns the new state and, when the window of one hour ends with this
/// poll, the summary line for the log (the poll counts in the window that ends; a new window
/// starts).
pub fn quiet_note(prev: Option<Quiet>, now: i64, now_ts: &str) -> (Quiet, Option<String>) {
    let mut q = prev.unwrap_or(Quiet {
        start: now,
        start_ts: now_ts.to_string(),
        n: 0,
    });
    q.n += 1;
    if now - q.start >= QUIET_WINDOW {
        let line = format!("quiet: {} polls since {}", q.n, q.start_ts);
        let fresh = Quiet {
            start: now,
            start_ts: now_ts.to_string(),
            n: 0,
        };
        return (fresh, Some(line));
    }
    (q, None)
}

// --- stop file -------------------------------------------------------------------------------

/// The first of `files` that is newer than the stop file: the file whose change lifts the stop.
/// A file with no time (missing) never lifts it.
pub fn stop_lifted_by(stop: SystemTime, files: &[(PathBuf, Option<SystemTime>)]) -> Option<&Path> {
    files
        .iter()
        .find(|(_, m)| m.is_some_and(|m| m > stop))
        .map(|(p, _)| p.as_path())
}

// --- work wake -------------------------------------------------------------------------------

/// Is a wake for owned work due? Not with `off`; otherwise when the last wake ended at least the
/// back-off ago. A missing or broken `last-wake` counts as the epoch.
pub fn work_due(last_wake: Option<i64>, now: i64, backoff: WorkBackoff) -> bool {
    match backoff {
        WorkBackoff::Off => false,
        WorkBackoff::Secs(s) => {
            now - last_wake.unwrap_or(0) >= i64::try_from(s).unwrap_or(i64::MAX)
        }
    }
}

// --- tick ------------------------------------------------------------------------------------

/// The line of a tick that says there is nothing to do.
const TICK_IDLE: &str = "tick: no ticket in Review";

/// Does a tick count as a quiet poll? Yes when its output has no line besides blank lines and
/// the idle line, or equals the output of the last tick.
pub fn tick_is_quiet(out: &str, last: Option<&str>) -> bool {
    let empty = out.lines().all(|l| l.trim().is_empty() || l == TICK_IDLE);
    empty || last == Some(out)
}

/// The log lines of a tick: each line that is not empty, prefixed `tick: `.
pub fn tick_log_lines(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| !l.is_empty())
        .map(|l| format!("tick: {}", clean_line(l, TICK_CUT)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn values_are_read_like_the_shell_read_them() {
        assert_eq!(parse_whole_secs("30"), Some(30));
        for bad in ["", "x", "-1", "1.5", "5m", " 3"] {
            assert_eq!(parse_whole_secs(bad), None, "{bad:?}");
        }
        assert_eq!(WorkBackoff::parse("off"), Some(WorkBackoff::Off));
        assert_eq!(WorkBackoff::parse("5"), Some(WorkBackoff::Secs(5)));
        assert_eq!(WorkBackoff::parse("soon"), None);
        assert_eq!(parse_max_wake("off"), Some(0));
        assert_eq!(parse_max_wake("0"), Some(0));
        assert_eq!(parse_max_wake("45m"), Some(2700));
        assert_eq!(parse_max_wake("2h"), Some(7200));
        assert_eq!(parse_max_wake("08"), Some(8));
        // "1m30s" is not a duration
        for bad in ["", "1m30s", "m", "abc", "-5"] {
            assert_eq!(parse_max_wake(bad), None, "{bad:?}");
        }
        assert_eq!(parse_delays("10 30"), Some(vec![10, 30]));
        assert_eq!(parse_delays(""), Some(vec![]));
        assert_eq!(parse_delays("10 x"), None);
    }

    #[test]
    fn the_first_stderr_line_is_cleaned_and_cut() {
        let long = "Z".repeat(300);
        let text = format!("\n  \nconfig error\x1b[31m {long}\nhint: second line\n");
        let l = first_stderr_line(&text);
        assert!(l.starts_with("config error[31m ZZZ"), "{l}");
        assert!(!l.contains('\x1b') && !l.contains("hint"));
        assert_eq!(l.chars().count(), 200);
        // the other direction: nothing to show
        assert_eq!(first_stderr_line(""), "(no stderr)");
        assert_eq!(first_stderr_line("\n \t\n"), "(no stderr)");
        assert_eq!(first_stderr_line("\x1b\n"), "(no stderr)");
    }

    #[test]
    fn the_first_ticket_prefers_in_progress() {
        let mine = "  demo/app#4   Ready        P2\n  demo/app#3   In Progress  P1  order 1\n";
        assert_eq!(first_ticket(mine).as_deref(), Some("demo/app#3"));
        let ready = "demo/app#4 Ready P2\ndemo/app#5 Ready P3\n";
        assert_eq!(first_ticket(ready).as_deref(), Some("demo/app#4"));
        // the other direction: no ticket line
        assert_eq!(first_ticket("nothing to do\n"), None);
        assert_eq!(first_ticket(""), None);
        // an In Progress line without a ticket reference gives none
        assert_eq!(first_ticket("agent In Progress\n"), None);
    }

    #[test]
    fn the_net_pause_table() {
        let t: Vec<i64> = (0..8).map(net_pause_secs).collect();
        assert_eq!(t, vec![0, 0, 300, 600, 1200, 2400, 3600, 3600]);
    }

    #[test]
    fn a_failed_run_counts_and_the_second_starts_a_pause() {
        let a = net_failed(NetBackoff::default(), 1000);
        assert_eq!(
            a,
            NetBackoff {
                fails: 1,
                until: 1000
            }
        );
        assert!(!a.pauses(1000));
        let b = net_failed(a, 2000);
        assert_eq!(
            b,
            NetBackoff {
                fails: 2,
                until: 2300
            }
        );
        assert!(b.pauses(2299));
        // the other direction: the pause is over at `until`
        assert!(!b.pauses(2300));
        assert_eq!(net_failed(b, 3000).until, 3600);
    }

    #[test]
    fn the_net_file_is_read_leniently() {
        assert_eq!(
            NetBackoff::parse("2 1700\n"),
            NetBackoff {
                fails: 2,
                until: 1700
            }
        );
        assert_eq!(NetBackoff::parse("junk\n"), NetBackoff::default());
        assert_eq!(
            NetBackoff::parse("3 x\n"),
            NetBackoff { fails: 3, until: 0 }
        );
        assert_eq!(
            NetBackoff::parse("1 2 3\n"),
            NetBackoff { fails: 1, until: 0 }
        );
        assert_eq!(NetBackoff::parse(""), NetBackoff::default());
        assert_eq!(NetBackoff { fails: 4, until: 9 }.render(), "4 9\n");
    }

    #[test]
    fn the_quiet_window_writes_one_line_when_the_hour_ends() {
        let (q, line) = quiet_note(None, 10_000, "T0");
        assert_eq!((q.n, line), (1, None));
        let (q, line) = quiet_note(Some(q), 10_000 + 3599, "T1");
        assert_eq!((q.n, line), (2, None));
        // the poll that ends the window counts in it; a new window starts
        let (q, line) = quiet_note(Some(q), 10_000 + 3600, "T2");
        assert_eq!(line.as_deref(), Some("quiet: 3 polls since T0"));
        assert_eq!((q.start, q.start_ts.as_str(), q.n), (13_600, "T2", 0));
        let (q, line) = quiet_note(Some(q), 13_601, "T3");
        assert_eq!((q.n, line), (1, None));
    }

    #[test]
    fn a_broken_quiet_file_starts_a_new_window() {
        assert_eq!(
            Quiet::parse("1700000000 2026-01-01T00:00:00Z 5\n"),
            Some(Quiet {
                start: 1_700_000_000,
                start_ts: "2026-01-01T00:00:00Z".into(),
                n: 5
            })
        );
        for bad in ["junk\n", "", "x ts 5\n", "1 ts x\n", "1 ts\n", "1\n"] {
            assert_eq!(Quiet::parse(bad), None, "{bad:?}");
        }
        let q = Quiet {
            start: 5,
            start_ts: "T".into(),
            n: 2,
        };
        assert_eq!(Quiet::parse(&q.render()), Some(q));
    }

    #[test]
    fn a_stop_is_lifted_only_by_a_newer_file() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let older = t0 - Duration::from_secs(5);
        let newer = t0 + Duration::from_secs(5);
        let files = vec![
            (PathBuf::from("/c/missing"), None),
            (PathBuf::from("/c/old.env"), Some(older)),
            (PathBuf::from("/c/same.toml"), Some(t0)),
            (PathBuf::from("/c/new.toml"), Some(newer)),
        ];
        assert_eq!(stop_lifted_by(t0, &files), Some(Path::new("/c/new.toml")));
        // control: without the newer file, nothing lifts it (same time is not newer)
        assert_eq!(stop_lifted_by(t0, &files[..3]), None);
    }

    #[test]
    fn owned_work_wakes_after_the_back_off_only() {
        let b = WorkBackoff::Secs(900);
        assert!(!work_due(Some(1000 - 60), 1000, b));
        assert!(work_due(Some(1000 - 1000), 1000, b));
        assert!(work_due(Some(1000 - 900), 1000, b));
        assert!(work_due(None, 1000, b));
        assert!(!work_due(None, 1000, WorkBackoff::Off));
        assert!(work_due(Some(990), 1000, WorkBackoff::Secs(5)));
    }

    #[test]
    fn a_tick_is_quiet_when_idle_or_unchanged() {
        assert!(tick_is_quiet("", None));
        assert!(tick_is_quiet("tick: no ticket in Review\n", None));
        assert!(tick_is_quiet("\ntick: no ticket in Review\n\n", None));
        let busy = "tick: verified and merged demo/app#5\n";
        assert!(!tick_is_quiet(busy, None));
        assert!(!tick_is_quiet(busy, Some("tick: other\n")));
        assert!(tick_is_quiet(busy, Some(busy)));
    }

    #[test]
    fn tick_lines_are_prefixed_cleaned_and_cut() {
        let long = "x".repeat(500);
        let out = format!("tick: a\x1b[0m\n\n{long}\n");
        let l = tick_log_lines(&out);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0], "tick: tick: a[0m");
        assert_eq!(l[1].chars().count(), "tick: ".len() + 400);
    }
}
