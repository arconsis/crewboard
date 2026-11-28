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

// --- failed wakes ----------------------------------------------------------------------------

/// The run of failed wake commands at which the coordinator is told, once.
pub const WAKE_TELL_RUN: u64 = 3;

/// The file `wake-fail` after one more wake command that failed: the same count and the same
/// growing pause as the net back-off (none after the first, then 300 s doubled, at most 1 h).
/// During the pause the run still polls, without a heartbeat; it starts no wake command.
pub fn wake_failed(prev: NetBackoff, now: i64) -> NetBackoff {
    net_failed(prev, now)
}

/// The message to the coordinator at [`WAKE_TELL_RUN`]. It names no path of the host.
pub fn wake_fail_text(agent: &str, fails: u64, rc: u8) -> String {
    format!(
        "The wake command of {agent} failed {fails} times in a row (last exit {rc}). The agent cannot work until the owner repairs its runtime on the host (a login that ended, a missing program); the reason is in wake.log in the state dir of the agent. The wrapper tries again with a pause of up to 1 h and goes on by itself when the command works."
    )
}

// --- stop file: the hourly try ---------------------------------------------------------------

/// A stop that no file change lifted is tried again when it is this old: an exit 4 can be a
/// fault that ends by itself (GitHub answered 401 to a good token). One board call per hour.
pub const STOP_RETRY_SECS: i64 = 3600;

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

// --- the model wake of the unattended coordinator --------------------------------------------

/// How many model wakes one `--tick --runtime` unit starts in one UTC day.
pub const MAX_COORD_WAKES_PER_DAY: u32 = 6;
/// A judgment list that did not change starts the next model wake after this many seconds.
pub const COORD_BACKOFF_SECS: i64 = 7200;
/// The least time between two model starts of one UTC day, in seconds. A dev agent can change
/// the list at will (a block, a question), so a change alone must not start models in a row.
pub const COORD_MIN_DISTANCE_SECS: i64 = 1800;
/// The run of failed model wakes at which one line goes to standard error.
pub const COORD_TELL_RUN: u64 = WAKE_TELL_RUN;

/// The file `coordinator-wakes`: `<UTC day> <model wakes of that day> <epoch of the last start>
/// <hash of the list of the last tick> <mode> <runtime> <pending: 0 or 1>`. It is written
/// before a model starts, and when the list of a tick differs from the stored one
/// ([`coord_note`]). `pending`: the list changed since the last start and no model saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordWakes {
    /// `YYYY-MM-DD`.
    pub day: String,
    pub count: u32,
    pub last: i64,
    pub hash: String,
    /// `unattended_coordinator` of AUTHORITY.md at that start.
    pub mode: String,
    pub runtime: String,
    pub pending: bool,
}

impl CoordWakes {
    /// `None` for a file that is broken: it counts as lost.
    pub fn parse(text: &str) -> Option<CoordWakes> {
        let mut it = text.lines().next()?.split(' ');
        let day = it.next().filter(|d| is_day(d))?.to_string();
        let count = it
            .next()
            .and_then(parse_whole_secs)
            .and_then(|n| u32::try_from(n).ok())?;
        let last = it
            .next()
            .and_then(parse_whole_secs)
            .and_then(|n| i64::try_from(n).ok())?;
        let mut word = || it.next().filter(|w| !w.is_empty()).map(str::to_string);
        Some(CoordWakes {
            day,
            count,
            last,
            hash: word()?,
            mode: word()?,
            runtime: word()?,
            pending: match word()?.as_str() {
                "0" => false,
                "1" => true,
                _ => return None,
            },
        })
    }

    pub fn render(&self) -> String {
        format!(
            "{} {} {} {} {} {} {}\n",
            self.day,
            self.count,
            self.last,
            self.hash,
            self.mode,
            self.runtime,
            u8::from(self.pending)
        )
    }
}

/// `YYYY-MM-DD`: the shape only, so that two days compare as text.
fn is_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// Is `key` a ticket key in plain characters: `<owner>/<repo>#<n>` with letters, digits, `.`,
/// `_` and `-` in the two names and digits after the `#`?
pub fn is_plain_ticket_key(key: &str) -> bool {
    let name = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    key.split_once('#').is_some_and(|(repo, n)| {
        !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && repo
                .split_once('/')
                .is_some_and(|(owner, r)| name(owner) && name(r))
    })
}

/// The hash of a judgment list: FNV-1a (64 bit) over its lines, as 16 hex digits. It is stable
/// across releases and hosts; it is no secret and protects nothing.
pub fn list_hash(lines: &[String]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in lines
        .iter()
        .flat_map(|l| l.bytes().chain(std::iter::once(b'\n')))
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// What the start rule of the model wake reads. All of it is data that crewboard made: the
/// values of the tick, the files of the state dir and the clock.
#[derive(Debug, Clone, Copy)]
pub struct CoordFacts<'a> {
    /// The mode of the tick; `None`: the mode is `off`, or the tick gave no values.
    pub mode: Option<&'a str>,
    /// The tickets on the judgment list.
    pub listed: usize,
    /// [`list_hash`] of the list.
    pub hash: &'a str,
    /// The verdicts of today and `unattended_budget`.
    pub used: u32,
    pub budget: u32,
    /// The model wakes pause after wakes that failed in a row.
    pub paused: bool,
    /// The file `coordinator-wakes`; `None`: missing or broken.
    pub stored: Option<&'a CoordWakes>,
    /// The UTC day of `now` (`YYYY-MM-DD`).
    pub today: &'a str,
    pub now: i64,
    pub runtime: &'a str,
}

/// Why no model starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordQuiet {
    /// The mode is `off`, or the tick gave no list.
    Off,
    /// No ticket needs a judgment.
    EmptyList,
    /// The verdicts of the day are used.
    BudgetSpent { used: u32, budget: u32 },
    /// The stored day is later than today: the clock went back, or it was ahead.
    ClockBack { stored: String },
    /// The day has its model wakes.
    DayLimit { count: u32 },
    /// Model wakes failed in a row: the pause of `wake-fail`.
    Paused,
    /// The last start of today is less than [`COORD_MIN_DISTANCE_SECS`] ago.
    TooSoon { next: i64 },
    /// The list did not change since the last model wake, and the back-off is not over.
    Unchanged { next: i64 },
}

impl CoordQuiet {
    /// The text for the log; `None` for the two cases that are no news (off, empty list).
    pub fn text(&self, format_ts: &dyn Fn(i64) -> String) -> Option<String> {
        Some(match self {
            CoordQuiet::Off | CoordQuiet::EmptyList => return None,
            CoordQuiet::BudgetSpent { used, budget } => {
                format!("the verdicts of today are used ({used}/{budget})")
            }
            CoordQuiet::ClockBack { stored } => format!(
                "the count of model wakes is of {stored}, a later day than today (UTC): the clock of this host went back, or it was ahead"
            ),
            CoordQuiet::DayLimit { count } => {
                format!("{count} model wakes today (UTC), the limit is {MAX_COORD_WAKES_PER_DAY}")
            }
            CoordQuiet::Paused => "the model wakes pause after wakes that failed in a row".into(),
            CoordQuiet::TooSoon { next } => format!(
                "the last model wake started less than {} min ago; the next one not before {}",
                COORD_MIN_DISTANCE_SECS / 60,
                format_ts(*next)
            ),
            CoordQuiet::Unchanged { next } => format!(
                "the list did not change since the last model wake; the next one not before {}",
                format_ts(*next)
            ),
        })
    }
}

/// Does a model start for the judgment list? `Ok` is the content of `coordinator-wakes` that the
/// caller must write before the start; a write that fails means no start.
///
/// A model starts only when all of this holds: the mode is not `off`; the list is not empty;
/// the verdicts of the day are not used; the stored day is not later than today; the day has
/// fewer than [`MAX_COORD_WAKES_PER_DAY`] model wakes; the wakes do not pause; the last start of
/// today is [`COORD_MIN_DISTANCE_SECS`] ago or more (the first start of a day needs no distance);
/// and the list changed since the last model wake, or [`COORD_BACKOFF_SECS`] passed since its
/// start (a last start in the future counts as "not passed"). "Changed": the list differs from
/// the list of the last tick, or an earlier tick noted a change that no model saw (`pending`,
/// [`coord_note`]). So a ticket that leaves the list and comes back with the same line is a
/// change. Without the file the count of the day is 0: a lost file allows at most
/// [`MAX_COORD_WAKES_PER_DAY`] more wakes.
pub fn coord_start(f: &CoordFacts<'_>) -> Result<CoordWakes, CoordQuiet> {
    let Some(mode) = f.mode else {
        return Err(CoordQuiet::Off);
    };
    if f.listed == 0 {
        return Err(CoordQuiet::EmptyList);
    }
    if f.used >= f.budget {
        return Err(CoordQuiet::BudgetSpent {
            used: f.used,
            budget: f.budget,
        });
    }
    if let Some(s) = f.stored.filter(|s| s.day.as_str() > f.today) {
        return Err(CoordQuiet::ClockBack {
            stored: s.day.clone(),
        });
    }
    let count = f.stored.filter(|s| s.day == f.today).map_or(0, |s| s.count);
    if count >= MAX_COORD_WAKES_PER_DAY {
        return Err(CoordQuiet::DayLimit { count });
    }
    if f.paused {
        return Err(CoordQuiet::Paused);
    }
    if let Some(s) = f.stored.filter(|_| count > 0) {
        let next = s.last.saturating_add(COORD_MIN_DISTANCE_SECS);
        if f.now < next {
            return Err(CoordQuiet::TooSoon { next });
        }
    }
    if let Some(s) = f.stored.filter(|s| s.hash == f.hash && !s.pending) {
        let next = s.last.saturating_add(COORD_BACKOFF_SECS);
        if f.now < next {
            return Err(CoordQuiet::Unchanged { next });
        }
    }
    Ok(CoordWakes {
        day: f.today.to_string(),
        count: count + 1,
        last: f.now,
        hash: f.hash.to_string(),
        mode: mode.to_string(),
        runtime: f.runtime.to_string(),
        pending: false,
    })
}

/// What a run that starts no model writes to `coordinator-wakes`, or `None` for no write: the
/// list of this tick, when the mode is on and the list differs from the stored one. The count,
/// its day and the time of the last start stay. `pending` is set for a list with a ticket: the
/// change waits for a model. An empty list ends a wait. Without a stored file the stored list is
/// the empty one. A stored day later than today is not touched (the clock rule).
pub fn coord_note(f: &CoordFacts<'_>) -> Option<CoordWakes> {
    let mode = f.mode?;
    if f.stored.is_some_and(|s| s.day.as_str() > f.today) {
        return None;
    }
    let (stored_hash, pending) = match f.stored {
        Some(s) => (s.hash.clone(), s.pending),
        None => (list_hash(&[]), false),
    };
    let now_pending = f.listed > 0 && (pending || stored_hash != f.hash);
    if stored_hash == f.hash && pending == now_pending {
        return None;
    }
    Some(CoordWakes {
        day: f.stored.map_or(f.today, |s| s.day.as_str()).to_string(),
        count: f.stored.map_or(0, |s| s.count),
        last: f.stored.map_or(0, |s| s.last),
        hash: f.hash.to_string(),
        mode: mode.to_string(),
        runtime: f.runtime.to_string(),
        pending: now_pending,
    })
}

/// The line for standard error at [`COORD_TELL_RUN`] failed model wakes in a row. The headless
/// scope refuses a `say`, so the log and the scheduler tell the owner.
pub fn coord_fail_text(fails: u64, rc: u8) -> String {
    format!(
        "the model wake of the unattended coordinator failed {fails} times in a row (last exit {rc}): repair its runtime on this host (a login that ended, a missing program). The tick goes on. Each failed start used one of the {MAX_COORD_WAKES_PER_DAY} model wakes of the day. The next start waits for the failure pause (up to 1 h), for {} min since this start, and for a change of the judgment list or {} h",
        COORD_MIN_DISTANCE_SECS / 60,
        COORD_BACKOFF_SECS / 3600
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_values_of_the_options_parse() {
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

#[cfg(test)]
mod coord_tests {
    use super::*;

    const TODAY: &str = "2026-11-28";
    const NOW: i64 = 1_795_860_000;

    fn stored(day: &str, count: u32, last: i64, hash: &str) -> CoordWakes {
        CoordWakes {
            day: day.into(),
            count,
            last,
            hash: hash.into(),
            mode: "verify".into(),
            runtime: "codex".into(),
            pending: false,
        }
    }

    /// Facts that start a model: the mode is on, one ticket is listed, the budget has room, no
    /// file is stored and nothing pauses.
    fn facts<'a>(s: Option<&'a CoordWakes>) -> CoordFacts<'a> {
        CoordFacts {
            mode: Some("verify"),
            listed: 1,
            hash: "h1",
            used: 0,
            budget: 10,
            paused: false,
            stored: s,
            today: TODAY,
            now: NOW,
            runtime: "hermes",
        }
    }

    #[test]
    fn a_model_starts_when_every_condition_holds_and_the_file_counts_the_start() {
        let next = coord_start(&facts(None)).unwrap();
        assert_eq!(
            next,
            CoordWakes {
                day: TODAY.into(),
                count: 1,
                last: NOW,
                hash: "h1".into(),
                mode: "verify".into(),
                runtime: "hermes".into(),
                pending: false,
            }
        );
        // the count of today goes on; the count of an earlier day does not
        let s = stored(TODAY, 4, NOW - COORD_MIN_DISTANCE_SECS, "h0");
        assert_eq!(coord_start(&facts(Some(&s))).unwrap().count, 5);
        let s = stored("2026-11-27", 6, NOW - 90_000, "h0");
        assert_eq!(coord_start(&facts(Some(&s))).unwrap().count, 1);
    }

    #[test]
    fn each_condition_alone_keeps_the_model_off() {
        // the mode is off, or the tick gave no values
        let off = CoordFacts {
            mode: None,
            ..facts(None)
        };
        assert_eq!(coord_start(&off), Err(CoordQuiet::Off));
        // the list is empty
        let empty = CoordFacts {
            listed: 0,
            ..facts(None)
        };
        assert_eq!(coord_start(&empty), Err(CoordQuiet::EmptyList));
        // the verdicts of the day are used (also above the budget: a floor can be higher)
        for used in [10, 11] {
            let spent = CoordFacts {
                used,
                ..facts(None)
            };
            assert_eq!(
                coord_start(&spent),
                Err(CoordQuiet::BudgetSpent { used, budget: 10 })
            );
        }
        let room = CoordFacts {
            used: 9,
            ..facts(None)
        };
        assert!(coord_start(&room).is_ok());
        // six wakes today; five leave room for one
        let six = stored(TODAY, 6, NOW - 9000, "h0");
        assert_eq!(
            coord_start(&facts(Some(&six))),
            Err(CoordQuiet::DayLimit { count: 6 })
        );
        let five = stored(TODAY, 5, NOW - 9000, "h0");
        assert_eq!(coord_start(&facts(Some(&five))).unwrap().count, 6);
        // the stored day is later than today
        let ahead = stored("2026-11-29", 1, NOW - 9000, "h0");
        assert_eq!(
            coord_start(&facts(Some(&ahead))),
            Err(CoordQuiet::ClockBack {
                stored: "2026-11-29".into()
            })
        );
        // the wakes pause
        let paused = CoordFacts {
            paused: true,
            ..facts(None)
        };
        assert_eq!(coord_start(&paused), Err(CoordQuiet::Paused));
    }

    #[test]
    fn an_unchanged_list_waits_for_the_back_off_and_a_changed_list_does_not() {
        let same = |last: i64| stored(TODAY, 1, last, "h1");
        let s = same(NOW - COORD_BACKOFF_SECS + 1);
        assert_eq!(
            coord_start(&facts(Some(&s))),
            Err(CoordQuiet::Unchanged { next: NOW + 1 })
        );
        let s = same(NOW - COORD_BACKOFF_SECS);
        assert_eq!(coord_start(&facts(Some(&s))).unwrap().count, 2);
        // the list of the last wake of an earlier day counts too: the back-off is 2 h, not a day
        let s = stored("2026-11-27", 3, NOW - 60, "h1");
        assert!(matches!(
            coord_start(&facts(Some(&s))),
            Err(CoordQuiet::Unchanged { .. })
        ));
        // a change that an earlier tick noted starts a model, also with the same list
        let s = CoordWakes {
            pending: true,
            ..same(NOW - COORD_MIN_DISTANCE_SECS)
        };
        assert_eq!(coord_start(&facts(Some(&s))).unwrap().count, 2);
        // a last start in the future (the clock went back inside the day) is "not over"
        let s = same(NOW + 500);
        assert!(matches!(
            coord_start(&facts(Some(&s))),
            Err(CoordQuiet::TooSoon { .. })
        ));
        // another list starts when the distance to the last start is kept
        let s = stored(TODAY, 1, NOW - COORD_MIN_DISTANCE_SECS, "h0");
        assert_eq!(coord_start(&facts(Some(&s))).unwrap().count, 2);
    }

    #[test]
    fn the_order_of_the_reasons_is_fixed() {
        // off and the empty list come before everything: they are no news
        let s = stored("2026-11-29", 9, NOW, "h1");
        let all = CoordFacts {
            mode: None,
            listed: 0,
            used: 10,
            paused: true,
            ..facts(Some(&s))
        };
        assert_eq!(coord_start(&all), Err(CoordQuiet::Off));
        let on = CoordFacts {
            mode: Some("verify"),
            ..all
        };
        assert_eq!(coord_start(&on), Err(CoordQuiet::EmptyList));
        let listed = CoordFacts { listed: 2, ..on };
        assert!(matches!(
            coord_start(&listed),
            Err(CoordQuiet::BudgetSpent { .. })
        ));
        let room = CoordFacts { used: 0, ..listed };
        assert!(matches!(
            coord_start(&room),
            Err(CoordQuiet::ClockBack { .. })
        ));
        assert_eq!(CoordQuiet::Off.text(&|_| String::new()), None);
        assert_eq!(CoordQuiet::EmptyList.text(&|_| String::new()), None);
        assert_eq!(
            CoordQuiet::Unchanged { next: 7 }
                .text(&|t| format!("T{t}"))
                .unwrap(),
            "the list did not change since the last model wake; the next one not before T7"
        );
    }

    #[test]
    fn the_count_file_reads_back_and_a_broken_one_is_lost() {
        let w = stored(TODAY, 3, NOW, "00ff");
        assert_eq!(w.render(), "2026-11-28 3 1795860000 00ff verify codex 0\n");
        assert_eq!(CoordWakes::parse(&w.render()), Some(w));
        for bad in [
            "",
            "\n",
            "2026-11-28",
            "2026-11-28 3 1795860000 00ff verify codex",
            "2026-11-28 3 1795860000 00ff verify codex yes",
            "28.11.2026 3 1795860000 00ff verify codex 0",
            "2026-11-28 -3 1795860000 00ff verify codex 0",
            "2026-11-28 three 1795860000 00ff verify codex 0",
            "2026-11-28 3 soon 00ff verify codex 0",
            "2026-11-28  3 1795860000 00ff verify codex 0",
        ] {
            assert_eq!(CoordWakes::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_list_hash_is_pinned_and_follows_the_lines() {
        let l = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        // FNV-1a 64 of no byte, and of "a\n"
        assert_eq!(list_hash(&[]), "cbf29ce484222325");
        assert_eq!(list_hash(&l(&["a"])), "089bdc07b544e7b2");
        let one = list_hash(&l(&["o/r#5, prose Gate"]));
        assert_eq!(one.len(), 16);
        assert_ne!(one, list_hash(&l(&["o/r#5, question"])));
        assert_ne!(one, list_hash(&l(&["o/r#5, prose Gate", "o/r#6, blocked"])));
        // the line ends count: two lines are not one line with the same letters
        assert_ne!(list_hash(&l(&["ab", "c"])), list_hash(&l(&["a", "bc"])));
    }

    #[test]
    fn the_line_after_three_failed_model_wakes_names_no_path_and_no_ticket() {
        let t = coord_fail_text(3, 9);
        assert!(t.starts_with(
            "the model wake of the unattended coordinator failed 3 times in a row (last exit 9)"
        ));
        assert!(!t.contains('/') && !t.contains('#'), "{t}");
    }

    #[test]
    fn two_starts_of_a_day_are_30_minutes_apart_and_the_first_start_needs_no_distance() {
        // a changed list one second before the distance is over
        let s = stored(TODAY, 1, NOW - COORD_MIN_DISTANCE_SECS + 1, "h0");
        assert_eq!(
            coord_start(&facts(Some(&s))),
            Err(CoordQuiet::TooSoon { next: NOW + 1 })
        );
        let s = stored(TODAY, 1, NOW - COORD_MIN_DISTANCE_SECS, "h0");
        assert!(coord_start(&facts(Some(&s))).is_ok());
        // the first start of a day: a start of yesterday one minute ago does not count, and a
        // record without a start of today does not
        let s = stored("2026-11-27", 2, NOW - 60, "h0");
        assert_eq!(coord_start(&facts(Some(&s))).unwrap().count, 1);
        let s = stored(TODAY, 0, NOW - 60, "h0");
        assert_eq!(coord_start(&facts(Some(&s))).unwrap().count, 1);
        // the limit of the day and the pause come first
        let s = stored(TODAY, 6, NOW - 60, "h0");
        assert!(matches!(
            coord_start(&facts(Some(&s))),
            Err(CoordQuiet::DayLimit { .. })
        ));
    }

    #[test]
    fn a_run_without_a_start_notes_a_list_that_changed_and_keeps_the_count() {
        let empty = list_hash(&[]);
        // the list is the stored one: nothing to write
        let s = stored(TODAY, 2, NOW - 60, "h1");
        assert_eq!(coord_note(&facts(Some(&s))), None);
        // another list: the hash and `pending`; the day, the count and the last start stay
        let s = stored(TODAY, 2, NOW - 60, "h0");
        let note = coord_note(&facts(Some(&s))).unwrap();
        assert_eq!(
            (
                note.day.as_str(),
                note.count,
                note.last,
                note.hash.as_str(),
                note.pending
            ),
            (TODAY, 2, NOW - 60, "h1", true)
        );
        assert_eq!(note.runtime, "hermes");
        // the list is empty now: the hash of no line, and nothing waits
        let gone = CoordFacts {
            listed: 0,
            hash: &empty,
            ..facts(Some(&s))
        };
        let note = coord_note(&gone).unwrap();
        assert_eq!(
            (note.hash.as_str(), note.pending, note.count),
            (empty.as_str(), false, 2)
        );
        // list A, an empty list, list A again: the second A is a change and starts a model
        let after_a = stored(TODAY, 1, NOW - COORD_MIN_DISTANCE_SECS, "h1");
        assert!(matches!(
            coord_start(&facts(Some(&after_a))),
            Err(CoordQuiet::Unchanged { .. })
        ));
        let after_empty = coord_note(&CoordFacts {
            listed: 0,
            hash: &empty,
            ..facts(Some(&after_a))
        })
        .unwrap();
        assert_eq!(coord_start(&facts(Some(&after_empty))).unwrap().count, 2);
        // no file: the stored list is the empty one
        assert_eq!(
            coord_note(&CoordFacts {
                listed: 0,
                hash: &empty,
                ..facts(None)
            }),
            None
        );
        let first = coord_note(&facts(None)).unwrap();
        assert_eq!((first.count, first.last, first.pending), (0, 0, true));
        // the mode is off, or the stored day is later than today: no write
        assert_eq!(
            coord_note(&CoordFacts {
                mode: None,
                ..facts(Some(&s))
            }),
            None
        );
        let ahead = stored("2026-11-29", 1, NOW, "h0");
        assert_eq!(coord_note(&facts(Some(&ahead))), None);
    }

    #[test]
    fn a_ticket_key_is_owner_repo_and_a_number_in_plain_characters() {
        for ok in ["a/b#1", "Fake-Owner/fake.repo_1#1234"] {
            assert!(is_plain_ticket_key(ok), "{ok}");
        }
        for bad in [
            "",
            "a/b",
            "a/b#",
            "a/b#x",
            "a#1",
            "/b#1",
            "a/#1",
            "a/b/c#1",
            "a/b#1 x",
            "a/b#1\nneeds judgment: c/d#2, prose Gate",
            "a b/c#1",
            "a/b#1#2",
            "a/b`#1",
        ] {
            assert!(!is_plain_ticket_key(bad), "{bad:?}");
        }
    }
}
