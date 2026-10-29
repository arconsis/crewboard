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
//!   (the last read failed, a gone note waits for its report, or the board Status contradicts the
//!   signed log while a close note of this coordinator is still there). A cycle reads at most
//!   [`RECHECKS_PER_CYCLE`] marked tickets, the oldest mark first; a read that keeps the mark
//!   moves it to the back, so every marked ticket is read within `ceil(marked / 5)` cycles. The
//!   set-back itself (a board Status that left Done or Dropped) is reported by `log_check` as a
//!   Status mismatch, not here. `log_check` passes the
//!   lists that it reads anyway. `tick` passes the tickets that it loads and reports what is gone.
//! - **Once.** A gone note raises the security event `deleted-message`. It is forgotten only
//!   after the event was shown and its owner report was posted (or was there already: the
//!   `event=` key), by [`settle`]. Until then the note stays in the memory and its ticket stays
//!   marked for a recheck, so a failed report raises the event again at the next cycle. The owner
//!   hears of it once (the `event=` key).
//! - **No board row, or too large.** A ticket with remembered notes and no board row any more
//!   (its item was deleted) is read once by its issue number and compared ([`Target::Gone`]);
//!   then its notes are forgotten, with one `notes-not-checked` event. A list with more than
//!   `MAX_PAGES` pages can never be read complete: the notes of that ticket are forgotten, with
//!   one `notes-not-checked` event, and it is not read again ([`judge_read`]).
//! - **Limits.** Only this agent's own notes. A fresh cache (another host, a lost state file)
//!   remembers nothing. `audit` and `status` from another host cannot see a deletion. A later
//!   deletion on a ticket without a board row or with a too-large list is not seen.

use super::{AgentCtx, World};
use crate::clock::{format_ts, now_epoch};
use crate::comments::Comment;
use crate::error::{CbError, CbResult};
use crate::events::{Reason, SecurityEvent};
use crate::feed;
use crate::github::GhError;
use crate::github::paging::MAX_PAGES;
use crate::protocol::acts::{Act, parse};
use crate::protocol::names::Kind;
use crate::services::ticket_loader::Ticket;
use crate::state::{NoteKind, NoteState, PostedNote, TicketSnap, note_key};
use crate::ticket::TicketRow;
use crate::ticket::log::{fold_ticket, status_contradicts};
use crate::ticket::verified_messages;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// At most this many tickets that are marked for a recheck are read in one cycle (a ticket whose
/// board row changed is read in any case).
pub const RECHECKS_PER_CYCLE: usize = 5;

/// What a signed note is, for the notes that are remembered: the close notes of every kind
/// (`dropped:` too), the verify reports, the assignments, the reassign notices and the creation
/// info. `None`: the note is not remembered.
pub fn note_kind(kind: Kind, body: &str) -> Option<NoteKind> {
    match parse(Some(kind), body) {
        Act::Closed(_) => Some(NoteKind::Close),
        Act::Verify { pass: true } => Some(NoteKind::VerifyPass),
        Act::Verify { pass: false } => Some(NoteKind::VerifyFail),
        Act::Assignment { .. } => Some(NoteKind::Assignment),
        Act::Reassigned => Some(NoteKind::Reassign),
        Act::BoardAnnounce => Some(NoteKind::Creation),
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
        &note_key(repo, issue),
        PostedNote {
            id,
            kind: k,
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

/// Compare one ticket with the memory. Every gone note gives one event. The note is NOT
/// forgotten here: the caller forgets it with [`settle`] after the event was shown and reported,
/// and until then the ticket is marked for a read at the next cycle, which raises the event
/// again. `contradicts`: the board Status contradicts the signed log of the read. While it does
/// and a remembered close note is still there, the board was set back behind the close, so the
/// ticket is read again at the next cycle (the note may be deleted next). A contradiction
/// without a close note of this coordinator (an owner merge that waits for its close, a board
/// drag) costs no read per cycle. A failed read (`None`) marks the ticket for a read at the next
/// cycle too. Pure.
pub fn check_ticket(
    notes: &mut NoteState,
    me: &str,
    repo: &str,
    issue: u64,
    read: Option<&[Comment]>,
    contradicts: bool,
    now: i64,
) -> Vec<SecurityEvent> {
    let key = note_key(repo, issue);
    if notes.of(&key).is_empty() {
        notes.notes_recheck.remove(&key);
        return Vec::new();
    }
    if read.is_none() {
        notes.mark(&key, now);
        return Vec::new();
    }
    let gone = missing(notes.of(&key), read);
    let close_there = notes
        .of(&key)
        .iter()
        .any(|n| n.kind == NoteKind::Close && !gone.contains(n));
    if !gone.is_empty() || (contradicts && close_there) {
        notes.mark(&key, now);
    } else {
        notes.notes_recheck.remove(&key);
    }
    gone.iter().map(|n| event(me, repo, issue, n)).collect()
}

/// Forget the notes of the `deleted-message` events in `shown`. The caller calls it only after
/// the events were shown (printed) AND reported to the owner (posted now, or found in the
/// registry issue already): an event whose report failed is not passed, so its note stays and
/// the next cycle raises it again. The ticket stays marked for one more read, which clears the
/// mark when nothing else is wrong. Other events are ignored. Pure.
pub fn settle(notes: &mut NoteState, shown: &[SecurityEvent]) {
    for s in shown.iter().filter(|s| s.reason == Reason::DeletedMessage) {
        notes.forget(&note_key(&s.repo, s.issue), s.comment_id);
    }
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
            n.kind.as_str(),
            n.id,
            format_ts(n.at)
        ),
    }
}

/// [`check_ticket`] for a read of the coordinator: the contradiction comes from the signed log of
/// the comments that were read and the board Status.
#[allow(
    clippy::too_many_arguments,
    reason = "the facts of one read; a struct would only rename them"
)]
pub(super) fn check_read(
    ac: &AgentCtx,
    world: &World,
    notes: &mut NoteState,
    repo: &str,
    issue: u64,
    board: Option<&str>,
    read: Option<&[Comment]>,
    now: i64,
) -> Vec<SecurityEvent> {
    let contra = read.is_some_and(|c| contradicts(ac, world, c, board));
    check_ticket(notes, ac.name.as_str(), repo, issue, read, contra, now)
}

/// Does the board Status contradict the signed log of the comments that were read?
fn contradicts(ac: &AgentCtx, world: &World, read: &[Comment], board: Option<&str>) -> bool {
    status_contradicts(
        &fold_ticket(&verified_messages(read, &ac.ctx.cfg, &world.trust)),
        board,
    )
}

/// One ticket that the cycle reads for the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    /// A ticket with a board row.
    Row(&'a TicketRow),
    /// A ticket with remembered notes and no board row any more (its item was deleted from the
    /// board, or GitHub cannot resolve it): the note key (`owner/repo#n`). It is read once by its
    /// issue number.
    Gone(&'a str),
}

impl Target<'_> {
    /// The repo and the issue number.
    pub fn ticket(&self) -> Option<(String, u64)> {
        match self {
            Target::Row(r) => Some((r.repo.clone(), r.issue)),
            Target::Gone(key) => {
                let (repo, n) = key.rsplit_once('#')?;
                Some((repo.to_string(), n.parse().ok()?))
            }
        }
    }
}

/// The tickets that the cycle reads for the check: a ticket with remembered notes whose board row
/// changed since the last cycle (`prev`) and not by this cycle (`own`); and at most
/// [`RECHECKS_PER_CYCLE`] of the tickets that are marked for a recheck or that have notes and no
/// board row, the oldest mark first (a ticket without a row and without a mark first of all;
/// ties by key). `done`: the tickets that the cycle read already (`log_check`). Pure.
pub fn to_read<'a>(
    rows: &'a [TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    own: &HashSet<&str>,
    done: &HashSet<String>,
    notes: &'a NoteState,
) -> Vec<Target<'a>> {
    let open: Vec<(&TicketRow, String)> = rows
        .iter()
        .map(|r| (r, note_key(&r.repo, r.issue)))
        .filter(|(r, key)| !done.contains(&r.key()) && !notes.of(key).is_empty())
        .collect();
    let changed = |r: &TicketRow| {
        !own.contains(r.item_id.as_str()) && prev.get(&r.item_id).is_some_and(|p| *p != r.snap())
    };
    let mut out: Vec<Target<'a>> = open
        .iter()
        .filter(|(r, _)| changed(r))
        .map(|(r, _)| Target::Row(r))
        .collect();
    let mut marked: Vec<(i64, &str, Target<'a>)> = open
        .iter()
        .filter(|(r, _)| !changed(r))
        .filter_map(|(r, key)| {
            notes
                .notes_recheck
                .get_key_value(key)
                .map(|(k, at)| (*at, k.as_str(), Target::Row(r)))
        })
        .collect();
    let on_board: BTreeSet<String> = rows.iter().map(|r| note_key(&r.repo, r.issue)).collect();
    marked.extend(
        notes
            .posted_notes
            .keys()
            .filter(|k| !on_board.contains(*k))
            .map(|k| {
                let at = notes.notes_recheck.get(k).copied().unwrap_or(i64::MIN);
                (at, k.as_str(), Target::Gone(k.as_str()))
            }),
    );
    marked.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    out.extend(
        marked
            .into_iter()
            .take(RECHECKS_PER_CYCLE)
            .map(|(_, _, t)| t),
    );
    out
}

/// What one read of a comment list gave.
#[derive(Debug, Clone, Copy)]
pub enum ReadResult<'a> {
    /// The complete list.
    Complete(&'a [Comment]),
    /// The list has more pages than crewboard reads: it never fits, so a repeat is no use.
    TooLarge,
    /// Any other error: a later cycle reads again.
    Failed,
}

impl<'a> ReadResult<'a> {
    pub fn of(r: &'a CbResult<(Vec<Comment>, Option<String>)>) -> Self {
        match r {
            Ok((c, _)) => ReadResult::Complete(c),
            Err(CbError::Network(GhError::PageLimit(_))) => ReadResult::TooLarge,
            Err(_) => ReadResult::Failed,
        }
    }
}

/// Judge one read of a ticket for the check. `board`: `Some(status)` for a ticket with a board
/// row (its board Status), `None` for a ticket without one ([`Target::Gone`]). Returns the
/// `deleted-message` events and, for a ticket that is not checked any more, one note for the
/// coordinator:
///
/// - a list that is too large ([`ReadResult::TooLarge`]): the memory of the ticket is
///   forgotten, so no cycle reads it again (a repeat gives the same error);
/// - a ticket without a board row: its list is compared once, and the notes that are still
///   there are forgotten. A gone note stays until its event is settled ([`settle`]).
///
/// `contradicts` is asked only for a complete list of a ticket with a row. Pure.
#[allow(
    clippy::too_many_arguments,
    reason = "the facts of one read; a struct would only rename them"
)]
pub fn judge_read(
    notes: &mut NoteState,
    me: &str,
    repo: &str,
    issue: u64,
    read: ReadResult<'_>,
    board: Option<Option<&str>>,
    contradicts: impl FnOnce(&[Comment], Option<&str>) -> bool,
    now: i64,
) -> (Vec<SecurityEvent>, Option<String>) {
    let key = note_key(repo, issue);
    match (read, board) {
        (ReadResult::TooLarge, _) => {
            if notes.of(&key).is_empty() {
                return (Vec::new(), None);
            }
            for id in notes.of(&key).iter().map(|n| n.id).collect::<Vec<_>>() {
                notes.forget(&key, id);
            }
            notes.notes_recheck.remove(&key);
            (
                Vec::new(),
                Some(format!(
                    "{repo}#{issue}: the comment list has more than {MAX_PAGES} pages ({} comments), too large to check; the signed notes of this coordinator on it are not checked any more",
                    MAX_PAGES * 100
                )),
            )
        }
        (ReadResult::Failed, _) => (check_ticket(notes, me, repo, issue, None, false, now), None),
        (ReadResult::Complete(c), Some(status)) => {
            let contra = contradicts(c, status);
            (
                check_ticket(notes, me, repo, issue, Some(c), contra, now),
                None,
            )
        }
        (ReadResult::Complete(c), None) => {
            if notes.of(&key).is_empty() {
                return (Vec::new(), None);
            }
            let gone = check_ticket(notes, me, repo, issue, Some(c), false, now);
            let kept: Vec<u64> = notes
                .of(&key)
                .iter()
                .map(|n| n.id)
                .filter(|id| !gone.iter().any(|g| g.comment_id == *id))
                .collect();
            for id in kept {
                notes.forget(&key, id);
            }
            (
                gone,
                Some(format!(
                    "{repo}#{issue}: the ticket has no board item any more (deleted from the board?); its comments were compared once with the signed notes of this coordinator, which are not checked any more"
                )),
            )
        }
    }
}

/// What the check of one coordinator cycle found.
#[derive(Debug, Default)]
pub(super) struct NoteCycle {
    pub(super) security: Vec<SecurityEvent>,
    /// One note text per ticket whose notes are not checked any more: the caller shows each as
    /// a [`NOT_CHECKED`] event (`duties`; this file does not post, so it does not import
    /// `registry_post`).
    pub(super) not_checked: Vec<String>,
}

/// The event kind of a ticket whose notes are not checked any more.
pub(super) const NOT_CHECKED: &str = "notes-not-checked";

/// The check of one coordinator cycle ([`to_read`]): one comment-list read per such ticket. A
/// read error is a line on stderr and a recheck; it never fails the cycle.
#[allow(
    clippy::too_many_arguments,
    reason = "the facts of one cycle; a struct would only rename them"
)]
pub(super) fn cycle_duty(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    own: &HashSet<&str>,
    done: &HashSet<String>,
    notes: &mut NoteState,
    now: i64,
) -> NoteCycle {
    let mut out = NoteCycle::default();
    let todo: Vec<(String, u64, Option<Option<String>>)> = to_read(rows, prev, own, done, notes)
        .into_iter()
        .filter_map(|t| {
            let (repo, issue) = t.ticket()?;
            let board = match t {
                Target::Row(r) => Some(r.status_str().map(str::to_string)),
                Target::Gone(_) => None,
            };
            Some((repo, issue, board))
        })
        .collect();
    for (repo, issue, board) in todo {
        let read =
            feed::issue_comments(&ac.ctx.client, &repo, issue, None, &ac.ctx.cfg.owner_login);
        if let Err(e) = &read {
            eprintln!(
                "crewboard: cannot read {repo}#{issue} to check the notes of this coordinator ({e}); {}",
                if matches!(ReadResult::of(&read), ReadResult::TooLarge) {
                    "not checked again"
                } else {
                    "next cycle"
                }
            );
        }
        let (security, note) = judge_read(
            notes,
            ac.name.as_str(),
            &repo,
            issue,
            ReadResult::of(&read),
            board.as_ref().map(Option::as_deref),
            |c, status| contradicts(ac, world, c, status),
            now,
        );
        out.security.extend(security);
        out.not_checked.extend(note);
    }
    out
}

/// The check for a ticket that `tick` loaded (its comments are the complete list). The state
/// cache is read and saved here. The caller reports the events to the owner and, after it
/// returned its output, forgets them with [`tick_settle`] (`tick`).
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
        now_epoch(),
    );
    ac.save_cache_required(&cache)?;
    Ok(gone)
}

/// [`settle`] for the events that `tick` showed and reported: the state cache is read and saved
/// here.
pub(super) fn tick_settle(ac: &AgentCtx, shown: &[SecurityEvent]) -> CbResult<()> {
    if shown.is_empty() {
        return Ok(());
    }
    let mut cache = ac.load_cache();
    settle(&mut cache.notes, shown);
    ac.save_cache_required(&cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u64, kind: &str) -> PostedNote {
        PostedNote {
            id,
            kind: serde_json::from_value(serde_json::json!(kind)).unwrap(),
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
            (Kind::Info, closed_verified_body(), Some(NoteKind::Close)),
            (
                Kind::Info,
                dropped_body("no PR needed"),
                Some(NoteKind::Close),
            ),
            (
                Kind::Info,
                format!("{}x", merged_head("u", "s", "squash")),
                Some(NoteKind::Close),
            ),
            (
                Kind::Info,
                OWNER_MERGED_HEAD.to_string(),
                Some(NoteKind::Close),
            ),
            (Kind::Report, verify_head(true), Some(NoteKind::VerifyPass)),
            (Kind::Report, verify_head(false), Some(NoteKind::VerifyFail)),
            (
                Kind::Request,
                assignment_body("t", "o/r#7", "PVTI_1", None, None, ""),
                Some(NoteKind::Assignment),
            ),
            (
                Kind::Info,
                reassigned_body("dead"),
                Some(NoteKind::Reassign),
            ),
            (Kind::Info, board_announce_head(), Some(NoteKind::Creation)),
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
        let got = check_ticket(&mut n, "c@x", "o/r", 7, None, false, 1_790_000_200);
        assert!(got.is_empty());
        assert_eq!(n.of("o/r#7").len(), 2, "nothing is forgotten");
        assert!(n.notes_recheck.contains_key("o/r#7"));
        // the next complete read finds the deletion; the ticket stays marked until the event
        // is settled
        let got = check_ticket(
            &mut n,
            "c@x",
            "o/r",
            7,
            Some(&[comment(10)]),
            false,
            1_790_000_200,
        );
        assert_eq!(got.len(), 1);
        assert!(n.notes_recheck.contains_key("o/r#7"));
        settle(&mut n, &got);
        // one more read finds nothing and clears the mark
        assert!(
            check_ticket(
                &mut n,
                "c@x",
                "o/r",
                7,
                Some(&[comment(10)]),
                false,
                1_790_000_200
            )
            .is_empty()
        );
        assert!(!n.notes_recheck.contains_key("o/r#7"));
    }

    #[test]
    fn a_deleted_note_is_raised_once_with_the_ticket_the_kind_and_the_id() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        let read = [comment(10), comment(11)];
        let got = check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false, 1_790_000_200);
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
        // the second read of the same list raises nothing: the note is forgotten once settled
        settle(&mut n, &got);
        assert!(
            check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false, 1_790_000_200).is_empty()
        );
        assert_eq!(n.of("o/r#7"), &[note(10, "assignment")]);
        // a ticket without memory is not compared at all
        assert!(check_ticket(&mut n, "c@x", "o/r", 8, Some(&[]), true, 1_790_000_200).is_empty());
        assert!(!n.notes_recheck.contains_key("o/r#8"));
    }

    #[test]
    fn a_deleted_note_stays_remembered_and_is_raised_again_until_it_is_settled() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        let read = [comment(10)];
        let first = check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false, 1_790_000_200);
        assert_eq!(first.len(), 1);
        // the report failed: nothing is settled, so the note and the mark stay
        assert_eq!(n.of("o/r#7").len(), 2);
        assert!(n.notes_recheck.contains_key("o/r#7"));
        let again = check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false, 1_790_000_200);
        assert_eq!(again, first, "the next cycle raises the same event");
        // only the shown and reported event is forgotten; other events change nothing
        let mut other = again[0].clone();
        other.reason = Reason::Edited;
        other.comment_id = 10;
        settle(&mut n, &[other]);
        assert_eq!(n.of("o/r#7").len(), 2);
        settle(&mut n, &again);
        assert_eq!(n.of("o/r#7"), &[note(10, "assignment")]);
        assert!(
            check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false, 1_790_000_200).is_empty()
        );
    }

    #[test]
    fn a_board_that_contradicts_the_log_keeps_the_ticket_for_a_read_next_cycle() {
        let mut n = notes(&[note(12, "close")]);
        let read = [comment(12)];
        assert!(check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), true, 1_790_000_200).is_empty());
        assert!(n.notes_recheck.contains_key("o/r#7"));
        // the board agrees again: no more reads
        assert!(
            check_ticket(&mut n, "c@x", "o/r", 7, Some(&read), false, 1_790_000_200).is_empty()
        );
        assert!(!n.notes_recheck.contains_key("o/r#7"));
        // a contradiction without a close note of this coordinator costs no read per cycle
        let mut a = notes(&[note(10, "assignment")]);
        assert!(
            check_ticket(
                &mut a,
                "c@x",
                "o/r",
                7,
                Some(&[comment(10)]),
                true,
                1_790_000_200
            )
            .is_empty()
        );
        assert!(!a.notes_recheck.contains_key("o/r#7"));
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

    fn issue_of(t: &Target<'_>) -> u64 {
        t.ticket().unwrap().1
    }

    fn snap(status: &str) -> TicketSnap {
        TicketSnap {
            status: Some(status.into()),
            agent: None,
        }
    }

    #[test]
    fn the_note_key_does_not_depend_on_the_spelling_of_the_repo() {
        assert_eq!(note_key("Org/Repo", 7), note_key("org/repo", 7));
        assert_eq!(note_key("Org/Repo", 7), "org/repo#7");
        // recorded as typed (`Org/Repo#7`), compared with the config spelling of the rows
        let mut n = NoteState::default();
        n.record(&note_key("Org/Repo", 7), note(12, "close"), 1_790_000_100);
        let got = check_ticket(
            &mut n,
            "c@x",
            "org/repo",
            7,
            Some(&[]),
            false,
            1_790_000_200,
        );
        assert_eq!(
            got.len(),
            1,
            "the lookup with the other spelling finds the note"
        );
        // a row of the config spelling is read for a note recorded with another spelling
        let mut m = NoteState::default();
        m.record(&note_key("Org/Repo", 7), note(12, "close"), 1_790_000_100);
        let rows = [TicketRow {
            repo: "org/repo".into(),
            ..row("moved", 7, "Review")
        }];
        let prev: BTreeMap<_, _> = [("moved".to_string(), snap("Done"))].into();
        let got = to_read(&rows, &prev, &HashSet::new(), &HashSet::new(), &m);
        assert_eq!(got.len(), 1);
        // control: another issue number is another ticket
        assert!(
            check_ticket(
                &mut m,
                "c@x",
                "org/repo",
                8,
                Some(&[]),
                false,
                1_790_000_200
            )
            .is_empty()
        );
        // a file of an older build with a mixed-case key loads under the lower-case key
        let mut old = NoteState::default();
        old.posted_notes
            .insert("Org/Repo#7".into(), vec![note(12, "close")]);
        old.posted_notes
            .insert("org/repo#7".into(), vec![note(13, "verify-pass")]);
        old.normalize_keys();
        assert_eq!(old.posted_notes.len(), 1);
        assert_eq!(old.of("org/repo#7").len(), 2);
    }

    #[test]
    fn at_most_five_marked_tickets_are_read_per_cycle_and_the_reads_rotate() {
        let mut n = NoteState::default();
        for i in 1..=12u64 {
            n.record(&format!("o/r#{i}"), note(i * 10, "close"), 1_790_000_100);
            // ticket 1 has the oldest mark
            n.mark(&format!("o/r#{i}"), 1_000 + i as i64);
        }
        let rows: Vec<TicketRow> = (1..=12u64)
            .map(|i| row(&format!("it{i}"), i, "Review"))
            .collect();
        let prev: BTreeMap<_, _> = rows
            .iter()
            .map(|r| (r.item_id.clone(), snap("Review")))
            .collect();
        let (own, done) = (HashSet::new(), HashSet::new());
        let mut seen = BTreeSet::new();
        for cycle in 0..3i64 {
            let got: Vec<u64> = to_read(&rows, &prev, &own, &done, &n)
                .iter()
                .map(issue_of)
                .collect();
            assert!(got.len() <= RECHECKS_PER_CYCLE, "{got:?}");
            match cycle {
                0 => assert_eq!(got, vec![1, 2, 3, 4, 5]),
                1 => assert_eq!(got, vec![6, 7, 8, 9, 10]),
                _ => assert_eq!(got, vec![11, 12, 1, 2, 3]),
            }
            for i in got {
                // the board still contradicts the log: the read keeps the mark and moves it
                // to the back
                let read = [comment(i * 10)];
                check_ticket(&mut n, "c@x", "o/r", i, Some(&read), true, 2_000 + cycle);
                seen.insert(i);
            }
        }
        assert_eq!(
            seen.len(),
            12,
            "every marked ticket is read within ceil(12 / 5) cycles"
        );
        // a ticket whose row changed is read on top of the cap
        let mut moved = prev.clone();
        moved.insert("it12".into(), snap("Done"));
        let got = to_read(&rows, &moved, &own, &done, &n);
        assert_eq!(got.len(), RECHECKS_PER_CYCLE + 1);
        assert_eq!(issue_of(&got[0]), 12);
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
        n.mark("o/r#4", 1_790_000_200);
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
            .map(|t| match t {
                Target::Row(r) => r.item_id.as_str(),
                Target::Gone(k) => k,
            })
            .collect();
        // `same` did not change, `mine` was changed by this cycle, `read` was read already,
        // `nonotes` has no memory
        assert_eq!(got, vec!["moved", "recheck"]);
    }

    #[test]
    fn a_ticket_with_notes_and_no_board_row_is_read_once_by_its_number() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        n.record("o/r#8", note(80, "close"), 1_790_000_100);
        n.mark("o/r#8", 5);
        // ticket 7 has no row (its item was deleted), ticket 8 has one
        let rows = [row("eight", 8, "Done")];
        let prev: BTreeMap<_, _> = [("eight".to_string(), snap("Done"))].into();
        let none = HashSet::new();
        let got = to_read(&rows, &prev, &none, &HashSet::new(), &n);
        // the row-less ticket has no mark: it goes first
        assert_eq!(got, vec![Target::Gone("o/r#7"), Target::Row(&rows[0])]);
        assert_eq!(got[0].ticket(), Some(("o/r".to_string(), 7)));
        // its read compares once: the deleted close note is raised, the rest is forgotten
        let read = [comment(10)];
        let (ev, note_text) = judge_read(
            &mut n,
            "c@x",
            "o/r",
            7,
            ReadResult::Complete(&read),
            None,
            |_, _| unreachable!("a ticket without a row has no board Status"),
            9,
        );
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].comment_id, 12);
        assert!(note_text.unwrap().contains("no board item any more"));
        assert_eq!(n.of("o/r#7"), &[note(12, "close")], "the gone note waits");
        // not settled (the report failed): read again next cycle
        let got = to_read(&rows, &prev, &none, &HashSet::new(), &n);
        assert!(got.contains(&Target::Gone("o/r#7")));
        settle(&mut n, &ev);
        assert!(n.of("o/r#7").is_empty() && !n.notes_recheck.contains_key("o/r#7"));
        let got = to_read(&rows, &prev, &none, &HashSet::new(), &n);
        assert_eq!(
            got,
            vec![Target::Row(&rows[0])],
            "read once, then never again"
        );
        // control: without a deletion the read gives only the note and ends the memory
        let mut m = notes(&[note(10, "assignment")]);
        let (ev, note_text) = judge_read(
            &mut m,
            "c@x",
            "o/r",
            7,
            ReadResult::Complete(&read),
            None,
            |_, _| false,
            9,
        );
        assert!(ev.is_empty() && note_text.is_some());
        assert!(m.posted_notes.is_empty());
    }

    #[test]
    fn a_list_that_is_too_large_ends_the_check_of_its_ticket_with_one_note() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        n.mark("o/r#7", 5);
        let (ev, note_text) = judge_read(
            &mut n,
            "c@x",
            "o/r",
            7,
            ReadResult::TooLarge,
            Some(Some("In Progress")),
            |_, _| unreachable!(),
            9,
        );
        assert!(ev.is_empty());
        let text = note_text.unwrap();
        assert!(text.contains("more than 50 pages"), "{text}");
        assert!(n.of("o/r#7").is_empty() && !n.notes_recheck.contains_key("o/r#7"));
        // nothing is marked, so no cycle reads it again; a second error gives no second note
        let rows = [row("seven", 7, "In Progress")];
        let prev: BTreeMap<_, _> = [("seven".to_string(), snap("In Progress"))].into();
        assert!(to_read(&rows, &prev, &HashSet::new(), &HashSet::new(), &n).is_empty());
        let again = judge_read(
            &mut n,
            "c@x",
            "o/r",
            7,
            ReadResult::TooLarge,
            Some(None),
            |_, _| false,
            10,
        );
        assert_eq!(again, (Vec::new(), None));
        // control: another error keeps the memory and marks a recheck
        let mut m = notes(&[note(12, "close")]);
        let (ev, note_text) = judge_read(
            &mut m,
            "c@x",
            "o/r",
            7,
            ReadResult::Failed,
            Some(None),
            |_, _| false,
            11,
        );
        assert!(ev.is_empty() && note_text.is_none());
        assert_eq!(m.of("o/r#7").len(), 1);
        assert_eq!(m.notes_recheck.get("o/r#7"), Some(&11));
        // the error kind decides: a page-limit error is TooLarge, any other is Failed
        let too: CbResult<(Vec<Comment>, Option<String>)> =
            Err(GhError::PageLimit("comments of o/r have more than 50 pages".into()).into());
        assert!(matches!(ReadResult::of(&too), ReadResult::TooLarge));
        let other: CbResult<(Vec<Comment>, Option<String>)> =
            Err(GhError::Shape("x".into()).into());
        assert!(matches!(ReadResult::of(&other), ReadResult::Failed));
    }
}
