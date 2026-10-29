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
        PollPart::NoEvents => t.push_str("(no new events)\n"),
        PollPart::Failed(n) => t.push_str(&format!(
            "(crewboard poll failed with exit {n}: run crewboard watch --timeout 1m yourself)\n"
        )),
    }
    for l in results {
        t.push_str(l);
        t.push('\n');
    }
    t.push_str("== Your tickets (crewboard mine):\n");
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
    fn the_unhandled_file_sits_next_to_the_events_file() {
        let e = Path::new("/s/events.txt");
        assert_eq!(unhandled_path(e), PathBuf::from("/s/events.txt.unhandled"));
    }
}
