//! `acted` (hidden, internal): the event receipts of `poll-and-wake.sh`.
//!
//! The wake script writes the events of a wake to the events file F (the output of
//! `poll`, and later sections that a hook appends). After a wake with exit 0 it asks this
//! command: which events that need an action did the agent not act on? The script then
//! hands each of them to the next wake once (spec/events#event-receipts).
//!
//! The parsing and the judgement are pure functions. Only [`run`] reads the board.
use super::AgentCtx;
use super::tickets::load_world_cached;
use crate::clock::parse_ts;
use crate::error::{CbError, CbResult};
use crate::services::ticket_loader::read_messages;
use crate::ticket::Sender;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The marker line before an event that the wake script hands to a wake a second time. The next
/// check reads it: an event with the marker is not handed over again.
pub const REDELIVERED_MARKER: &str = "(crewboard: this event was delivered to an earlier wake, which did not act on it. It is delivered again once.)";

/// At most this many tickets are read in one check.
pub const MAX_TICKETS: usize = 25;

/// One event of the events file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeEvent {
    /// The label of the block header (`MESSAGE`, `TICKET`, `OWNER MESSAGE`, ...), or
    /// `DETACHED` for a finished detached job.
    pub label: String,
    pub kind: String,
    pub to: String,
    /// `owner/repo`; `None` for a detached job (the main repo).
    pub repo: Option<String>,
    pub issue: Option<u64>,
    /// The time of the event, Unix seconds.
    pub ts: Option<i64>,
    /// The line before the block is the [`REDELIVERED_MARKER`].
    pub redelivered: bool,
    /// The lines of the block, as they stand in the file.
    pub text: String,
}

impl WakeEvent {
    /// `<repo>#<n> <kind>`, or `<kind>` when the event has no ticket.
    pub fn receipt(&self) -> String {
        match (&self.repo, self.issue) {
            (Some(r), Some(n)) => format!("{r}#{n} {}", self.kind),
            (None, Some(n)) => format!("#{n} {}", self.kind),
            _ => self.kind.clone(),
        }
    }
}

fn is_block_start(l: &str) -> bool {
    l.starts_with("== [") && l.trim_end().ends_with(" ==")
}

/// Is the line the end of an event block? Another section header, a security event, a
/// warning or the summary line of the next `poll` output.
fn ends_block(l: &str) -> bool {
    l.starts_with("== ")
        || l.starts_with("!! SECURITY EVENT")
        || l.starts_with("WARNING: not trusted")
        || l.starts_with("selftest-forgery: ")
        || l.starts_with("crewboard: ")
        || l.starts_with("detached job finished: ")
        || l.trim() == REDELIVERED_MARKER
}

fn parse_where(l: &str) -> Option<(String, u64)> {
    let w = l.strip_prefix("where: ")?.split_whitespace().next()?;
    let (repo, n) = w.rsplit_once('#')?;
    if !repo.contains('/') {
        return None;
    }
    Some((repo.to_string(), n.parse().ok()?))
}

fn parse_from_line(l: &str) -> Option<(String, String)> {
    let rest = l.strip_prefix("from: ")?;
    let (_, rest) = rest.split_once("  to: ")?;
    let (to, rest) = rest.split_once("  kind: ")?;
    let kind = rest.split_once("  re: ").map_or(rest, |(k, _)| k);
    Some((to.trim().to_string(), kind.trim().to_string()))
}

fn parse_detached(l: &str) -> Option<WakeEvent> {
    let rest = l.strip_prefix("detached job finished: ticket ")?;
    let n: u64 = rest.split_whitespace().next()?.parse().ok()?;
    Some(WakeEvent {
        label: "DETACHED".into(),
        kind: "detached-job-finished".into(),
        to: String::new(),
        repo: None,
        issue: Some(n),
        ts: None,
        redelivered: false,
        text: l.to_string(),
    })
}

/// Read every event of an events file: the blocks `== [n] LABEL ==` of the `poll` output, in
/// the first section and in every appended one, and the lines of finished detached jobs. A block
/// that does not carry the lines `from:`, `where:` and `ts:` in its first lines is not an event.
pub fn parse_events(text: &str) -> Vec<WakeEvent> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut marked = false;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.trim() == REDELIVERED_MARKER {
            marked = true;
            i += 1;
            continue;
        }
        if let Some(mut d) = parse_detached(l) {
            // a finished job that goes again carries the marker like any other event
            d.redelivered = marked;
            marked = false;
            out.push(d);
            i += 1;
            continue;
        }
        if !is_block_start(l) {
            if !l.trim().is_empty() {
                marked = false;
            }
            i += 1;
            continue;
        }
        let label = l
            .trim_end()
            .trim_end_matches(" ==")
            .split_once("] ")
            .map_or("", |(_, t)| t)
            .to_string();
        let mut j = i + 1;
        while j < lines.len() && !ends_block(lines[j]) {
            j += 1;
        }
        let block = &lines[i..j];
        let head = &block[1..block.len().min(6)];
        let from = head.iter().find_map(|h| parse_from_line(h));
        let wh = head.iter().find_map(|h| parse_where(h));
        let ts = head
            .iter()
            .find_map(|h| h.strip_prefix("ts: "))
            .and_then(|t| parse_ts(t.trim()));
        if let (Some((to, kind)), Some((repo, issue))) = (from, wh) {
            let mut end = block.len();
            while end > 1 && block[end - 1].trim().is_empty() {
                end -= 1;
            }
            out.push(WakeEvent {
                label,
                kind,
                to,
                repo: Some(repo),
                issue: Some(issue),
                ts,
                redelivered: marked,
                text: block[..end].join("\n"),
            });
        }
        marked = false;
        i = j;
    }
    out
}

/// Does the event need an action of the agent? Informational kinds need none.
///
/// - needs one: `assignment` and `stale-work` (ticket events), a message to the agent of kind
///   `request` (a task, a failed verify, a re-delivery), `question`, `answer`, `done` or
///   `blocked`, an owner message, a finished detached job;
/// - needs none: `ack`, `report`, `info` (a merged PR, a closed ticket), `ping`, `pong`
///   (retired, old comments only), `status-changed`, `unassigned`, `ci-finished`, registry and liveness events, and a
///   message to `all`.
pub fn needs_action(e: &WakeEvent) -> bool {
    match e.label.as_str() {
        "DETACHED" | "OWNER MESSAGE" => true,
        "TICKET" => matches!(e.kind.as_str(), "assignment" | "stale-work"),
        "MESSAGE" => {
            e.to != "all"
                && matches!(
                    e.kind.as_str(),
                    "request" | "question" | "answer" | "done" | "blocked"
                )
        }
        _ => false,
    }
}

/// Did the agent act on the event? `own` are the creation times (Unix seconds) of the signed
/// messages of the agent on the ticket. Every command that writes a Status (`take`, `done`,
/// `block`, `ask`, `unblock`) and `say` post such a message, so one message at or after the
/// time of the event (and after the start of the wake, `since`) is the act.
pub fn acted(e: &WakeEvent, own: &[i64], since: i64) -> bool {
    let from = e.ts.unwrap_or(0).max(since);
    own.iter().any(|c| *c >= from)
}

/// `acted --list`: one line per event, `<repo>#<n> <kind>` (the receipts). No board read.
pub fn list(text: &str) -> String {
    parse_events(text)
        .iter()
        .map(|e| format!("{}\n", e.receipt()))
        .collect()
}

/// An event that was not acted on, and what to do with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miss {
    pub receipt: String,
    /// It was delivered again before: log it, do not deliver it again.
    pub twice: bool,
    pub text: String,
}

/// The check. `own_of` gives the creation times of the own messages on a ticket, or `None` when
/// the ticket cannot be read (the event then counts as handled, and the caller says so).
/// Pure: no board read here.
pub fn judge(
    events: &[WakeEvent],
    main_repo: &str,
    known_repos: &[String],
    since: i64,
    mut own_of: impl FnMut(&str, u64) -> Option<Vec<i64>>,
) -> Vec<Miss> {
    // one candidate per (ticket, kind, ts); it counts as delivered again when any copy is marked
    let mut cands: BTreeMap<(String, u64, String, Option<i64>), WakeEvent> = BTreeMap::new();
    for e in events.iter().filter(|e| needs_action(e)) {
        let repo = e.repo.clone().unwrap_or_else(|| main_repo.to_string());
        let Some(issue) = e.issue else { continue };
        if !known_repos.contains(&repo) {
            continue;
        }
        let key = (repo.clone(), issue, e.kind.clone(), e.ts);
        let mut c = e.clone();
        c.repo = Some(repo);
        match cands.get_mut(&key) {
            Some(old) => old.redelivered |= c.redelivered,
            None => {
                cands.insert(key, c);
            }
        }
    }
    let mut seen_tickets = BTreeSet::new();
    let mut out = Vec::new();
    for ((repo, issue, _, _), e) in cands {
        if !seen_tickets.contains(&(repo.clone(), issue)) && seen_tickets.len() >= MAX_TICKETS {
            continue;
        }
        seen_tickets.insert((repo.clone(), issue));
        let Some(own) = own_of(&repo, issue) else {
            continue;
        };
        if acted(&e, &own, since) {
            continue;
        }
        out.push(Miss {
            receipt: e.receipt(),
            twice: e.redelivered,
            text: e.text.clone(),
        });
    }
    out
}

/// What `acted` prints: one line per event that was not acted on, `<receipt> again` or
/// `<receipt> twice`; and the text for the next wake of the events that go again.
pub fn render(misses: &[Miss]) -> (String, String) {
    let mut lines = String::new();
    let mut again = String::new();
    for m in misses {
        lines.push_str(&format!(
            "{} {}\n",
            m.receipt,
            if m.twice { "twice" } else { "again" }
        ));
        if !m.twice {
            again.push_str(&format!("{REDELIVERED_MARKER}\n{}\n\n", m.text));
        }
    }
    (lines, again)
}

/// The check against the board. Reads: nothing when no event needs an action; else the
/// world (the registry, for the keys) and ONE comment read per ticket that an event names (at
/// most [`MAX_TICKETS`]; a ticket of a repo outside the initiative is skipped).
pub fn run(
    ac: &AgentCtx,
    events_file: &Path,
    since: Option<&str>,
    redeliver_to: Option<&Path>,
) -> CbResult<String> {
    let since = match since {
        Some(s) => parse_ts(s).ok_or_else(|| {
            CbError::config(format!("--since: '{s}' is not YYYY-MM-DDTHH:MM:SSZ"))
        })?,
        None => 0,
    };
    let text = std::fs::read_to_string(events_file)
        .map_err(|e| CbError::config(format!("cannot read {}: {e}", events_file.display())))?;
    let events = parse_events(&text);
    if !events.iter().any(needs_action) {
        return Ok(String::new());
    }
    let world = load_world_cached(ac)?;
    let me = ac.name.as_str();
    let misses = judge(
        &events,
        ac.ctx.main_repo(),
        &ac.ctx.cfg.repos,
        since,
        |repo, issue| match read_messages(&ac.ctx.reader(), &world.trust, repo, issue) {
            Ok((_, msgs)) => Some(
                msgs.iter()
                    .filter(|m| matches!(&m.sender, Sender::Agent { name, .. } if name == me))
                    .map(|m| m.created)
                    .collect(),
            ),
            Err(e) => {
                eprintln!("crewboard: cannot read {repo}#{issue} for the event check: {e}");
                None
            }
        },
    );
    let (lines, again) = render(&misses);
    if !again.is_empty()
        && let Some(p) = redeliver_to
    {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .and_then(|mut f| f.write_all(again.as_bytes()))
            .map_err(|e| CbError::config(format!("cannot write {}: {e}", p.display())))?;
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLL: &str = "\
== Events (crewboard poll, 2026-10-23T10:00:00Z):
crewboard: 3 event(s), 0 security event(s) for dev@h; 0 foreign comment(s) dropped; cursor 2026-10-23T10:00:00Z

== [1] TICKET ==
from: board  to: dev@h  kind: assignment  re: none
where: a/b#12 (board)
ts: 2026-10-23T09:59:00Z
url: https://github.com/a/b/issues/12

assigned to you; status Ready

== [2] MESSAGE ==
from: coord@h (verified)  to: dev@h  kind: question  re: none
where: a/b#7 comment 900
ts: 2026-10-23T09:58:00Z
url: https://github.com/a/b/issues/7#issuecomment-900

Which API?
more text

== [3] MESSAGE ==
from: coord@h (verified)  to: all  kind: info  re: none
where: a/b#9 comment 901
ts: 2026-10-23T09:58:30Z
url: u

hello

WARNING: not trusted: x
detached job finished: ticket 5 rc=0 log=/l result=/r
== Your tickets (crewboard mine):
a/b#12                             Ready            P1  order -
";

    fn ts(s: &str) -> i64 {
        parse_ts(s).unwrap()
    }

    #[test]
    fn it_reads_the_blocks_of_the_poll_output_and_the_detached_line() {
        let ev = parse_events(POLL);
        let r: Vec<String> = ev.iter().map(WakeEvent::receipt).collect();
        assert_eq!(
            r,
            [
                "a/b#12 assignment",
                "a/b#7 question",
                "a/b#9 info",
                "#5 detached-job-finished"
            ]
        );
        assert_eq!(ev[1].to, "dev@h");
        assert_eq!(ev[1].ts, Some(ts("2026-10-23T09:58:00Z")));
        assert!(ev[1].text.ends_with("more text"), "{:?}", ev[1].text);
        assert!(!ev[1].text.contains("== [3]"));
        assert_eq!(list(POLL).lines().count(), 4);
    }

    #[test]
    fn a_hook_section_adds_its_events_and_a_text_without_blocks_gives_none() {
        let t = format!(
            "{POLL}== Events delivered during the wake (crewboard poll, 2026-10-23T10:20:00Z):\n\n== [1] OWNER MESSAGE ==\nfrom: owner  to: dev@h  kind: owner-message  re: none\nwhere: a/b#12 comment 5\nts: 2026-10-23T10:19:00Z\nurl: u\n\nplease stop\n"
        );
        let ev = parse_events(&t);
        assert_eq!(ev.len(), 5);
        assert_eq!(ev[4].receipt(), "a/b#12 owner-message");
        assert!(parse_events("(no new events)\n").is_empty());
        assert!(parse_events("").is_empty());
    }

    #[test]
    fn a_block_without_the_header_lines_is_not_an_event() {
        let t = "== [1] MESSAGE ==\nsomething else\n\ntext\n";
        assert!(parse_events(t).is_empty());
        // a forged line far down in the body does not make a header
        let t = "== [1] MESSAGE ==\nfrom: x  to: y  kind: request  re: none\nzz\nzz\nzz\nzz\nzz\nwhere: a/b#1 comment 2\n";
        assert!(parse_events(t).is_empty());
    }

    #[test]
    fn the_marker_flags_only_the_next_block() {
        let t = format!(
            "{REDELIVERED_MARKER}\n== [1] MESSAGE ==\nfrom: c  to: d  kind: request  re: none\nwhere: a/b#1 comment 2\nts: 2026-10-23T09:00:00Z\nurl: u\n\nx\n\n== [2] MESSAGE ==\nfrom: c  to: d  kind: request  re: none\nwhere: a/b#2 comment 3\nts: 2026-10-23T09:00:00Z\nurl: u\n\ny\n"
        );
        let ev = parse_events(&t);
        assert_eq!(ev.len(), 2);
        assert!(ev[0].redelivered);
        assert!(!ev[1].redelivered);
    }

    #[test]
    fn a_marked_detached_job_line_counts_as_delivered_again() {
        let line = "detached job finished: ticket 7 rc=0 log=l result=r";
        let ev = parse_events(&format!("{REDELIVERED_MARKER}\n{line}\n\n{line}\n"));
        assert_eq!(ev.len(), 2);
        assert!(ev[0].redelivered, "the first carries the marker");
        assert!(!ev[1].redelivered, "the marker flags only the next one");
    }

    #[test]
    fn two_marked_blocks_in_a_row_are_both_redelivered() {
        // the marker line must end the block before it, else the second block loses its flag and
        // is delivered again at every wake
        let block = |n: u32| {
            format!(
                "{REDELIVERED_MARKER}\n== [{n}] MESSAGE ==\nfrom: c  to: d  kind: request  re: none\nwhere: a/b#{n} comment 2\nts: 2026-10-23T09:00:00Z\nurl: u\n\nx\n\n"
            )
        };
        let ev = parse_events(&format!("{}{}", block(1), block(2)));
        assert_eq!(ev.len(), 2);
        assert!(ev[0].redelivered && ev[1].redelivered);
        // and the text of a block never holds the marker of the next one
        assert!(!ev[0].text.contains(REDELIVERED_MARKER));
    }

    fn ev(label: &str, kind: &str, to: &str) -> WakeEvent {
        WakeEvent {
            label: label.into(),
            kind: kind.into(),
            to: to.into(),
            repo: Some("a/b".into()),
            issue: Some(1),
            ts: Some(100),
            redelivered: false,
            text: "t".into(),
        }
    }

    #[test]
    fn the_action_kinds_are_fixed() {
        for k in ["request", "question", "answer", "done", "blocked"] {
            assert!(needs_action(&ev("MESSAGE", k, "d")), "{k}");
        }
        for k in ["ack", "report", "info", "ping", "pong"] {
            assert!(!needs_action(&ev("MESSAGE", k, "d")), "{k}");
        }
        assert!(!needs_action(&ev("MESSAGE", "request", "all")));
        assert!(needs_action(&ev("TICKET", "assignment", "d")));
        assert!(needs_action(&ev("TICKET", "stale-work", "d")));
        for k in ["status-changed", "unassigned", "ci-finished"] {
            assert!(!needs_action(&ev("TICKET", k, "d")), "{k}");
        }
        assert!(needs_action(&ev("OWNER MESSAGE", "owner-message", "d")));
        assert!(!needs_action(&ev("REGISTRY", "request", "owner")));
        assert!(!needs_action(&ev("LIVENESS", "agent-stale", "d")));
    }

    #[test]
    fn a_signed_message_at_or_after_the_event_and_the_wake_start_is_the_act() {
        let e = ev("TICKET", "assignment", "d");
        assert!(!acted(&e, &[], 0));
        assert!(!acted(&e, &[99], 0), "before the event");
        assert!(acted(&e, &[100], 0));
        assert!(!acted(&e, &[150], 200), "before the wake started");
        assert!(acted(&e, &[250], 200));
    }

    #[test]
    fn the_check_names_each_miss_once_and_marks_the_second() {
        let repos = vec!["a/b".to_string()];
        let mut a = ev("TICKET", "assignment", "d");
        a.issue = Some(1);
        let mut q = ev("MESSAGE", "question", "d");
        q.issue = Some(2);
        q.redelivered = true;
        let mut ok = ev("MESSAGE", "request", "d");
        ok.issue = Some(3);
        let mut reads = 0;
        let m = judge(&[a, q, ok], "a/b", &repos, 0, |_, n| {
            reads += 1;
            Some(if n == 3 { vec![500] } else { vec![] })
        });
        assert_eq!(reads, 3);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].receipt, "a/b#1 assignment");
        assert!(!m[0].twice);
        assert!(m[1].twice);
        let (lines, again) = render(&m);
        assert_eq!(lines, "a/b#1 assignment again\na/b#2 question twice\n");
        assert_eq!(again.matches(REDELIVERED_MARKER).count(), 1);
    }

    #[test]
    fn one_read_per_ticket_a_foreign_repo_and_an_unreadable_ticket_are_skipped() {
        let repos = vec!["a/b".to_string()];
        let mut x = ev("MESSAGE", "request", "d");
        x.issue = Some(1);
        let mut y = ev("MESSAGE", "question", "d");
        y.issue = Some(1);
        let mut f = ev("MESSAGE", "question", "d");
        f.repo = Some("evil/repo".into());
        let mut u = ev("MESSAGE", "question", "d");
        u.issue = Some(9);
        let mut reads = Vec::new();
        let m = judge(&[x, y, f, u], "a/b", &repos, 0, |r, n| {
            reads.push(format!("{r}#{n}"));
            if n == 9 { None } else { Some(vec![]) }
        });
        assert_eq!(reads, ["a/b#1", "a/b#1", "a/b#9"].map(String::from));
        assert_eq!(m.len(), 2, "{m:?}");
    }

    #[test]
    fn the_ticket_reads_are_capped() {
        let repos = vec!["a/b".to_string()];
        let many: Vec<WakeEvent> = (1..=40)
            .map(|n| {
                let mut e = ev("MESSAGE", "request", "d");
                e.issue = Some(n);
                e
            })
            .collect();
        let mut reads = 0;
        let m = judge(&many, "a/b", &repos, 0, |_, _| {
            reads += 1;
            Some(vec![])
        });
        assert_eq!(reads, MAX_TICKETS);
        assert_eq!(m.len(), MAX_TICKETS);
    }

    #[test]
    fn a_detached_job_uses_the_main_repo() {
        let repos = vec!["a/b".to_string()];
        let d = parse_detached("detached job finished: ticket 5 rc=0 log=/l result=/r").unwrap();
        let m = judge(&[d], "a/b", &repos, 0, |r, n| {
            assert_eq!((r, n), ("a/b", 5));
            Some(vec![])
        });
        assert_eq!(m[0].receipt, "a/b#5 detached-job-finished");
    }
}
