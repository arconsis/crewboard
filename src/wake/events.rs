//! The events file of a wake: what the model reads first. The text is built here; the flow writes
//! it to `<file>.tmp`, renames it, and only then removes `<file>.unhandled`.
//!
//! ```text
//! == An earlier wake ended early. ...        (only when `<file>.unhandled` has content)
//! <the content of `.unhandled`>
//!
//! == Events (crewboard poll, <ts>):
//! <the poll output | (no new events) | (crewboard poll failed with exit N: ...)>
//! <one line per finished detached job>
//! == Your tickets (crewboard mine):
//! <the mine text | (crewboard mine failed with exit N)>
//! ```

use std::path::{Path, PathBuf};

/// The warning above the events that an earlier wake kept.
pub const EARLIER: &str = "== An earlier wake ended early. Its events may be handled already: check the ticket thread before you act.";

/// What `poll` gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollPart {
    Events(String),
    NoEvents,
    Failed(u8),
}

/// What `mine` gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MinePart {
    Text(String),
    Failed(u8),
}

/// The header of the tickets part.
const MINE_HEADER: &str = "== Your tickets (crewboard mine):";

/// The line of a poll that returned nothing.
const NO_EVENTS: &str = "(no new events)";

/// The events file of a wake that failed, as the next wake gets it: only what a later wake cannot
/// read again. The tickets part goes (the next file has a new one), a poll part without events
/// goes, the warning of an earlier keep goes (`compose` writes it once), and runs of blank lines
/// become one. So the kept block does not grow with each failed wake.
pub fn compact_kept(text: &str) -> String {
    // 1. drop the warning lines and the tickets parts
    let mut lines: Vec<&str> = Vec::new();
    let mut in_mine = false;
    for l in text.lines() {
        if l == MINE_HEADER {
            in_mine = true;
            continue;
        }
        if in_mine && !l.starts_with("== ") {
            continue;
        }
        in_mine = false;
        if l != EARLIER {
            lines.push(l);
        }
    }
    // 2. drop a poll header whose part holds nothing but "(no new events)"
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("== Events (crewboard poll, ") {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].starts_with("== ") {
                j += 1;
            }
            let empty = lines[i + 1..j]
                .iter()
                .all(|l| l.trim().is_empty() || *l == NO_EVENTS);
            if empty {
                i = j;
                continue;
            }
        }
        out.push(lines[i]);
        i += 1;
    }
    // 3. one blank line at most, none at the start or the end
    let mut t = String::new();
    let mut blank = true;
    for l in out {
        if l.trim().is_empty() {
            if !blank {
                t.push('\n');
            }
            blank = true;
        } else {
            t.push_str(l);
            t.push('\n');
            blank = false;
        }
    }
    while t.ends_with("\n\n") {
        t.pop();
    }
    t
}

/// `<file>.unhandled`
pub fn unhandled_path(events: &Path) -> PathBuf {
    let mut s = events.as_os_str().to_owned();
    s.push(".unhandled");
    PathBuf::from(s)
}

/// The text of the events file. `unhandled` is the content of `.unhandled` (empty: none), `ts` the
/// time of the poll, `results` one line per finished job.
pub fn compose(
    unhandled: &str,
    ts: &str,
    poll: &PollPart,
    results: &[String],
    mine: &MinePart,
) -> String {
    let mut t = String::new();
    if !unhandled.is_empty() {
        t.push_str(EARLIER);
        t.push('\n');
        t.push_str(unhandled);
        t.push('\n');
    }
    t.push_str(&format!("== Events (crewboard poll, {ts}):\n"));
    match poll {
        PollPart::Events(o) => t.push_str(o),
        PollPart::NoEvents => {
            t.push_str(NO_EVENTS);
            t.push('\n');
        }
        PollPart::Failed(n) => t.push_str(&format!(
            "(crewboard poll failed with exit {n}: run crewboard watch --timeout 1m yourself)\n"
        )),
    }
    for l in results {
        t.push_str(l);
        t.push('\n');
    }
    t.push_str(MINE_HEADER);
    t.push('\n');
    match mine {
        MinePart::Text(o) => t.push_str(o),
        MinePart::Failed(n) => t.push_str(&format!("(crewboard mine failed with exit {n})\n")),
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ev1_the_file_has_the_events_and_the_tickets() {
        let t = compose(
            "",
            "2026-10-24T10:00:00Z",
            &PollPart::Events("EVENT 1: assign #12\n".into()),
            &[],
            &MinePart::Text("demo/app#12 Ready P1\n".into()),
        );
        assert_eq!(
            t,
            "== Events (crewboard poll, 2026-10-24T10:00:00Z):\nEVENT 1: assign #12\n== Your tickets (crewboard mine):\ndemo/app#12 Ready P1\n"
        );
    }

    #[test]
    fn ev2_kept_events_come_first_with_a_warning() {
        let t = compose(
            "== Events (crewboard poll, T0):\nEVENT 7: question on #4\n",
            "T1",
            &PollPart::NoEvents,
            &[],
            &MinePart::Text(String::new()),
        );
        assert!(t.starts_with(EARLIER));
        let kept = t.find("EVENT 7: question on #4").unwrap();
        let new = t.find("== Events (crewboard poll, T1):").unwrap();
        assert!(kept < new, "kept events above the new ones");
        // one blank line between the kept block and the new header
        assert!(
            t.contains("on #4\n\n== Events (crewboard poll, T1):\n(no new events)\n"),
            "{t}"
        );
    }

    #[test]
    fn failures_and_results_have_their_lines() {
        let t = compose(
            "",
            "T",
            &PollPart::Failed(3),
            &["detached job finished: ticket 7 rc=0 log=l result=r".into()],
            &MinePart::Failed(2),
        );
        assert_eq!(
            t,
            "== Events (crewboard poll, T):\n(crewboard poll failed with exit 3: run crewboard watch --timeout 1m yourself)\ndetached job finished: ticket 7 rc=0 log=l result=r\n== Your tickets (crewboard mine):\n(crewboard mine failed with exit 2)\n"
        );
    }

    #[test]
    fn ev5_a_kept_file_loses_what_the_next_wake_reads_again() {
        let file = compose(
            "",
            "T0",
            &PollPart::Events("EVENT 1: assign #12\n".into()),
            &["detached job finished: ticket 7 rc=0 log=l result=r".into()],
            &MinePart::Text("demo/app#12 Ready P1\nnext: take\n".into()),
        );
        assert_eq!(
            compact_kept(&file),
            "== Events (crewboard poll, T0):\nEVENT 1: assign #12\ndetached job finished: ticket 7 rc=0 log=l result=r\n"
        );
    }

    /// The fault of a pilot host: 2571 failed wakes in a row made an events file of 15500 lines.
    #[test]
    fn ev6_failed_wakes_in_a_row_do_not_grow_the_kept_block() {
        let mut kept = String::new();
        let mut first = String::new();
        for i in 0..50 {
            let poll = if i == 0 {
                PollPart::Events("EVENT 1: assign #12\n".into())
            } else {
                PollPart::NoEvents
            };
            let file = compose(
                &kept,
                &format!("T{i}"),
                &poll,
                &[],
                &MinePart::Text("demo/app#12 Ready P1\n".into()),
            );
            // the wake failed: its file is kept for the next one
            kept = compact_kept(&file);
            if i == 0 {
                first = kept.clone();
            }
        }
        assert_eq!(
            kept, first,
            "the kept block is the same after 50 failed wakes"
        );
        assert_eq!(
            kept,
            "== Events (crewboard poll, T0):\nEVENT 1: assign #12\n"
        );
    }

    #[test]
    fn ev7_a_later_poll_with_events_and_a_budget_header_stay() {
        let kept = "The last wake hit the time budget. Uncommitted work in /w: commit or continue it first.\n== Events (crewboard poll, T0):\nEVENT 1: assign #12\n";
        let file = compose(
            kept,
            "T1",
            &PollPart::Events("EVENT 2: answer on #12\n".into()),
            &[],
            &MinePart::Failed(2),
        );
        assert_eq!(
            compact_kept(&file),
            "The last wake hit the time budget. Uncommitted work in /w: commit or continue it first.\n== Events (crewboard poll, T0):\nEVENT 1: assign #12\n\n== Events (crewboard poll, T1):\nEVENT 2: answer on #12\n"
        );
        // a poll that failed stays: the next wake must know that events may be missing
        let failed = compose(
            "",
            "T2",
            &PollPart::Failed(3),
            &[],
            &MinePart::Text(String::new()),
        );
        assert!(compact_kept(&failed).contains("crewboard poll failed with exit 3"));
        assert_eq!(compact_kept(""), "");
    }

    #[test]
    fn the_unhandled_file_sits_next_to_the_events_file() {
        let e = Path::new("/s/events.txt");
        assert_eq!(unhandled_path(e), PathBuf::from("/s/events.txt.unhandled"));
    }
}
