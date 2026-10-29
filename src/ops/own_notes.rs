//! The coordinator's memory of its own signed notes, and the check that one of them is gone
//! (spec/security-details#deleted-notes).
//!
//! The threat: a holder of a crew token deletes a signed note of the coordinator (most of all its
//! close note) and sets the board back. The signed log then agrees with the board, so `status` and
//! `audit` see nothing. An edit is detected (`edited-message`); a deletion leaves no comment to
//! judge. Only the sender knows what it posted, so the coordinator remembers it.
//!
//! - **Record.** Every signed post goes through `say::send_signed`, which calls [`record`]. A note
//!   of a kind of [`note_kind`] on a ticket is remembered in the state cache (`state::NoteState`,
//!   with its bound). No command can forget it.
//! - **Check.** A read of the COMPLETE comment list of a ticket is compared with the memory
//!   ([`missing`]). `feed::issue_comments` gives the whole list or an error (a list that is cut at
//!   the page limit is an error), so a cut list never reaches the compare. A failed read compares
//!   nothing and marks the ticket for a read at the next cycle. The comment feed of `poll` and
//!   `watch` is a time window: it is never compared.
//! - **When.** The cycle of the coordinator reads a ticket with remembered notes when its board row
//!   changed since the last cycle (not by the cycle itself), or when it is marked for a recheck
//!   (the last read failed, or the board Status contradicts the signed log while a close note of
//!   this coordinator is still there). `log_check` passes the
//!   lists that it reads anyway. `tick` passes the tickets that it loads and reports what is gone.
//! - **Once.** A gone note raises the security event `deleted-message` and is forgotten, so the
//!   next cycle does not raise it again. The owner hears of it once (the `event=` key).
//! - **Limits.** Only this agent's own notes. A fresh cache (another host, a lost state file)
//!   remembers nothing. `audit` and `status` from another host cannot see a deletion.

use super::{AgentCtx, World};
use crate::clock::{format_ts, now_epoch};
use crate::comments::Comment;
use crate::error::CbResult;
use crate::events::{Reason, SecurityEvent};
use crate::feed;
use crate::protocol::acts::{Act, parse};
use crate::protocol::names::Kind;
use crate::services::ticket_loader::Ticket;
use crate::state::{NoteState, PostedNote, TicketSnap};
use crate::ticket::TicketRow;
use crate::ticket::log::{fold_ticket, status_contradicts};
use crate::ticket::verified_messages;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// The kind of a close note (Done or Dropped).
const CLOSE: &str = "close";

/// What a signed note is, for the notes that are remembered: the close notes of every kind
/// (`dropped:` too), the verify reports, the assignments, the reassign notices and the creation
/// info. `None`: the note is not remembered.
pub fn note_kind(kind: Kind, body: &str) -> Option<&'static str> {
    match parse(Some(kind), body) {
        Act::Closed(_) => Some(CLOSE),
        Act::Verify { pass: true } => Some("verify-pass"),
        Act::Verify { pass: false } => Some("verify-fail"),
        Act::Assignment { .. } => Some("assignment"),
        Act::Reassigned => Some("reassign"),
        Act::BoardAnnounce => Some("creation"),
        _ => None,
    }
}

/// Remember a note that this agent just posted as comment `id` (called by `say::send_signed`).
/// The registry issue is no ticket. A failed save is a warning: the post stands.
pub(super) fn record(ac: &AgentCtx, repo: &str, issue: u64, kind: Kind, body: &str, id: u64) {
    let Some(k) = note_kind(kind, body) else {
        return;
    };
    if repo == ac.ctx.main_repo() && issue == ac.ctx.cfg.registry_issue {
        return;
    }
    let now = now_epoch();
    let mut cache = ac.load_cache();
    cache.notes.record(
        &format!("{repo}#{issue}"),
        PostedNote {
            id,
            kind: k.to_string(),
            at: now,
        },
        now,
    );
    ac.save_cache(&cache);
}

/// The remembered notes that a read of the comments of their ticket does not show. `read` is the
/// COMPLETE comment list of the issue, or `None` when the read failed or saw only a part: then
/// nothing is compared, because a missing id in a part of a list proves nothing. Pure.
pub fn missing(remembered: &[PostedNote], read: Option<&[Comment]>) -> Vec<PostedNote> {
    let Some(read) = read else {
        return Vec::new();
    };
    let present: BTreeSet<u64> = read.iter().map(|c| c.id).collect();
    remembered
        .iter()
        .filter(|n| !present.contains(&n.id))
        .cloned()
        .collect()
}

/// Compare one ticket with the memory. Every gone note is forgotten and gives one event, so a
/// later read does not raise it again. `contradicts`: the board Status contradicts the signed
/// log of the read. While it does and a remembered close note is still there, the board was set
/// back behind the close, so the ticket is read again at the next cycle (the note may be deleted
/// next). A contradiction without a close note of this coordinator (an owner merge that waits for
/// its close, a board drag) costs no read per cycle. A failed read (`None`) marks the ticket for
/// a read at the next cycle too. Pure.
pub fn check_ticket(
    notes: &mut NoteState,
    me: &str,
    repo: &str,
    issue: u64,
    read: Option<&[Comment]>,
    contradicts: bool,
) -> Vec<SecurityEvent> {
    let key = format!("{repo}#{issue}");
    if notes.of(&key).is_empty() {
        notes.notes_recheck.remove(&key);
        return Vec::new();
    }
    if read.is_none() {
        notes.notes_recheck.insert(key);
        return Vec::new();
    }
    let gone = missing(notes.of(&key), read);
    for n in &gone {
        notes.forget(&key, n.id);
    }
    if contradicts && notes.of(&key).iter().any(|n| n.kind == CLOSE) {
        notes.notes_recheck.insert(key);
    } else {
        notes.notes_recheck.remove(&key);
    }
    gone.iter().map(|n| event(me, repo, issue, n)).collect()
}

fn event(me: &str, repo: &str, issue: u64, n: &PostedNote) -> SecurityEvent {
    SecurityEvent {
        reason: Reason::DeletedMessage,
        comment_id: n.id,
        repo: repo.to_string(),
        issue,
        url: format!("https://github.com/{repo}/issues/{issue}"),
        author: "(deleted)".into(),
        created_at: format_ts(n.at),
        claimed_from: Some(me.to_string()),
        detail: format!(
            "the signed {} note (comment {}) that {me} posted on {repo}#{issue} at {} is not among the comments of the issue any more: somebody deleted it",
            n.kind,
            n.id,
            format_ts(n.at)
        ),
    }
}

/// [`check_ticket`] for a read of the coordinator: the contradiction comes from the signed log of
/// the comments that were read and the board Status.
pub(super) fn check_read(
    ac: &AgentCtx,
    world: &World,
    notes: &mut NoteState,
    repo: &str,
    issue: u64,
    board: Option<&str>,
    read: Option<&[Comment]>,
) -> Vec<SecurityEvent> {
    let contradicts = read.is_some_and(|c| {
        status_contradicts(
            &fold_ticket(&verified_messages(c, &ac.ctx.cfg, &world.trust)),
            board,
        )
    });
    check_ticket(notes, ac.name.as_str(), repo, issue, read, contradicts)
}

/// The tickets that the cycle reads for the check: a ticket with remembered notes whose board row
/// changed since the last cycle (`prev`) and not by this cycle (`own`), or that is marked for a
/// recheck. `done`: the tickets that the cycle read already (`log_check`). Pure.
pub fn to_read<'a>(
    rows: &'a [TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    own: &HashSet<&str>,
    done: &HashSet<String>,
    notes: &NoteState,
) -> Vec<&'a TicketRow> {
    rows.iter()
        .filter(|r| {
            let key = r.key();
            !done.contains(&key)
                && !notes.of(&key).is_empty()
                && (notes.notes_recheck.contains(&key)
                    || (!own.contains(r.item_id.as_str())
                        && prev.get(&r.item_id).is_some_and(|p| *p != r.snap())))
        })
        .collect()
}

/// The check of one coordinator cycle ([`to_read`]): one comment-list read per such ticket. A
/// read error is a line on stderr and a recheck; it never fails the cycle.
pub(super) fn cycle_duty(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    own: &HashSet<&str>,
    done: &HashSet<String>,
    notes: &mut NoteState,
) -> Vec<SecurityEvent> {
    let mut out = Vec::new();
    for row in to_read(rows, prev, own, done, notes) {
        let read = feed::issue_comments(
            &ac.ctx.client,
            &row.repo,
            row.issue,
            None,
            &ac.ctx.cfg.owner_login,
        );
        if let Err(e) = &read {
            eprintln!(
                "crewboard: cannot read {} to check the notes of this coordinator ({e}); next cycle",
                row.key()
            );
        }
        let comments = read.ok().map(|(c, _)| c);
        out.extend(check_read(
            ac,
            world,
            notes,
            &row.repo,
            row.issue,
            row.status_str(),
            comments.as_deref(),
        ));
    }
    out
}

/// The check for a ticket that `tick` loaded (its comments are the complete list). The state
/// cache is read and saved here. The caller reports the events to the owner (`tick`).
pub(super) fn tick_check(ac: &AgentCtx, world: &World, t: &Ticket) -> CbResult<Vec<SecurityEvent>> {
    let mut cache = ac.load_cache();
    let gone = check_read(
        ac,
        world,
        &mut cache.notes,
        &t.repo,
        t.issue,
        t.item.field("Status"),
        Some(&t.comments),
    );
    ac.save_cache_required(&cache)?;
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u64, kind: &str) -> PostedNote {
        PostedNote {
            id,
            kind: kind.into(),
            at: 1_790_000_000,
        }
    }

    fn comment(id: u64) -> Comment {
        Comment {
            id,
            repo: "o/r".into(),
            issue: 7,
            author: "crew-bot".into(),
            created: String::new(),
            updated: String::new(),
            created_epoch: 0,
            updated_epoch: 0,
            body: String::new(),
            url: String::new(),
            node_id: String::new(),
            editors: None,
        }
    }

    fn notes(list: &[PostedNote]) -> NoteState {
        let mut n = NoteState::default();
        for x in list {
            n.record("o/r#7", x.clone(), 1_790_000_100);
        }
        n
    }

    #[test]
    fn the_kinds_of_the_coordinators_notes_are_remembered_and_others_are_not() {
        use crate::protocol::acts::*;
        let cases = [
            (Kind::Info, closed_verified_body(), Some("close")),
            (Kind::Info, dropped_body("no PR needed"), Some("close")),
            (
                Kind::Info,
                format!("{}x", merged_head("u", "s", "squash")),
                Some("close"),
            ),
            (Kind::Info, OWNER_MERGED_HEAD.to_string(), Some("close")),
            (Kind::Report, verify_head(true), Some("verify-pass")),
            (Kind::Report, verify_head(false), Some("verify-fail")),
            (
                Kind::Request,
                assignment_body("t", "o/r#7", "PVTI_1", None, None, ""),
                Some("assignment"),
            ),
            (Kind::Info, reassigned_body("dead"), Some("reassign")),
            (Kind::Info, board_announce_head(), Some("creation")),
            // control: plain messages are not remembered
            (Kind::Info, "CI is green".to_string(), None),
            (Kind::Done, "pr: x".to_string(), None),
            (Kind::Report, "verify: maybe".to_string(), None),
        ];
        for (kind, body, want) in cases {
            assert_eq!(note_kind(kind, &body), want, "{kind} {body:?}");
        }
    }

    #[test]
    fn a_remembered_note_that_the_read_does_not_show_is_deleted() {
        let remembered = [note(10, "assignment"), note(12, "close")];
        let read = [comment(9), comment(10), comment(11)];
        let gone = missing(&remembered, Some(&read));
        assert_eq!(gone, vec![note(12, "close")]);
        // all present: nothing
        let all = [comment(10), comment(11), comment(12)];
        assert!(missing(&remembered, Some(&all)).is_empty());
        // the control can fail: an empty list misses both
        assert_eq!(missing(&remembered, Some(&[])).len(), 2);
    }

    #[test]
    fn a_failed_or_partial_read_raises_nothing_and_asks_for_a_read_next_cycle() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        // `None`: the read failed or saw only a part of the list
        assert!(missing(n.of("o/r#7"), None).is_empty());
        let got = check_ticket(&mut n, "c@x", "o/r", 7, None, false);
        assert!(got.is_empty());
        assert_eq!(n.of("o/r#7").len(), 2, "nothing is forgotten");
        assert!(n.notes_recheck.contains("o/r#7"));
        // the next complete read finds the deletion and clears the recheck
        let got = check_ticket(&mut n, "c@x", "o/r", 7, Some(&[comment(10)]), false);
        assert_eq!(got.len(), 1);
        assert!(!n.notes_recheck.contains("o/r#7"));
    }

    #[test]
    fn a_deleted_note_is_raised_once_with_the_ticket_the_kind_and_the_id() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        let read = [comment(10), comment(11)];
        let got = check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false);
        assert_eq!(got.len(), 1);
        let s = &got[0];
        assert_eq!(s.reason, Reason::DeletedMessage);
        assert_eq!(s.reason.as_str(), "deleted-message");
        assert_eq!((s.repo.as_str(), s.issue, s.comment_id), ("o/r", 7, 12));
        assert!(
            s.detail.contains("signed close note (comment 12)"),
            "{}",
            s.detail
        );
        assert_eq!(s.claimed_from.as_deref(), Some("c@x"));
        // the second read of the same list raises nothing: the note is forgotten
        assert!(check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false).is_empty());
        assert_eq!(n.of("o/r#7"), &[note(10, "assignment")]);
        // a ticket without memory is not compared at all
        assert!(check_ticket(&mut n, "c@x", "o/r", 8, Some(&[]), true).is_empty());
        assert!(!n.notes_recheck.contains("o/r#8"));
    }

    #[test]
    fn a_board_that_contradicts_the_log_keeps_the_ticket_for_a_read_next_cycle() {
        let mut n = notes(&[note(12, "close")]);
        let read = [comment(12)];
        assert!(check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), true).is_empty());
        assert!(n.notes_recheck.contains("o/r#7"));
        // the board agrees again: no more reads
        assert!(check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false).is_empty());
        assert!(!n.notes_recheck.contains("o/r#7"));
        // a contradiction without a close note of this coordinator costs no read per cycle
        let mut a = notes(&[note(10, "assignment")]);
        assert!(check_ticket(&mut a, "c@x", "o/r", 7, Some(&[comment(10)]), true).is_empty());
        assert!(!a.notes_recheck.contains("o/r#7"));
    }

    fn row(item: &str, issue: u64, status: &str) -> TicketRow {
        TicketRow {
            repo: "o/r".into(),
            issue,
            item_id: item.into(),
            status: Some(crate::rules::status::BoardStatus::parse(status)),
            agent: None,
            order: None,
            updated: None,
        }
    }

    fn snap(status: &str) -> TicketSnap {
        TicketSnap {
            status: Some(status.into()),
            agent: None,
        }
    }

    #[test]
    fn only_a_ticket_with_notes_that_changed_or_waits_for_a_recheck_is_read() {
        let mut n = NoteState::default();
        for issue in [1, 2, 3, 4, 5] {
            n.record(
                &format!("o/r#{issue}"),
                note(issue * 10, "close"),
                1_790_000_100,
            );
        }
        n.notes_recheck.insert("o/r#4".into());
        let rows = [
            row("moved", 1, "Review"),
            row("same", 2, "Done"),
            row("mine", 3, "Review"),
            row("recheck", 4, "Done"),
            row("read", 5, "Review"),
            row("nonotes", 6, "Review"),
        ];
        let prev: BTreeMap<_, _> = [
            ("moved".to_string(), snap("Done")),
            ("same".to_string(), snap("Done")),
            ("mine".to_string(), snap("Done")),
            ("recheck".to_string(), snap("Done")),
            ("read".to_string(), snap("Done")),
            ("nonotes".to_string(), snap("Done")),
        ]
        .into();
        let own: HashSet<&str> = ["mine"].into();
        let done: HashSet<String> = ["o/r#5".to_string()].into();
        let got: Vec<&str> = to_read(&rows, &prev, &own, &done, &n)
            .iter()
            .map(|r| r.item_id.as_str())
            .collect();
        // `same` did not change, `mine` was changed by this cycle, `read` was read already,
        // `nonotes` has no memory
        assert_eq!(got, vec!["moved", "recheck"]);
    }
}
