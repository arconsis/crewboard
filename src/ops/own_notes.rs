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
//! - **No board row.** A ticket with remembered notes and no board row any more (its item was
//!   deleted) is read once by its issue number and compared ([`Target::Gone`]); then its notes
//!   are forgotten, with one `notes-not-checked` event. A ticket whose newest note is younger
//!   than [`GONE_GRACE_SECS`] is not judged yet: the ticket index of a long `watch` learns a new
//!   ticket from the comment feed, which can lag behind the note that `ticket new` recorded.
//! - **Too large.** A list with more than `MAX_PAGES` pages cannot be read complete. The notes of
//!   that ticket stay, with one `notes-not-checked` event ([`judge_read`]). The ticket is not read
//!   in every cycle (a read costs `MAX_PAGES` pages), but again when its board row changes, or
//!   `TOO_LARGE_RECHECK_SECS` after the last read (`NoteState::notes_too_large`). So a flood that
//!   is deleted again after the close note is seen at the next row change (the set-back).
//! - **Once, also here.** A `notes-not-checked` event changes the memory only after it was shown
//!   and its owner report was posted (or was there already: the `event=` key), by
//!   [`settle_not_checked`]. Until then the ticket stays marked, so a cycle that fails later
//!   raises the event again.
//! - **Limits.** Only this agent's own notes. A fresh cache (another host, a lost state file)
//!   remembers nothing. `audit` and `status` from another host cannot see a deletion. A later
//!   deletion on a ticket without a board row is not seen; on a too-large ticket it is seen only
//!   when its list fits again.

use super::{AgentCtx, World};
use crate::base::TicketRef;
use crate::clock::{format_ts, now_epoch};
use crate::comments::Comment;
use crate::error::{CbError, CbResult};
use crate::events::{Reason, SecurityEvent};
use crate::feed;
use crate::github::GhError;
use crate::github::paging::MAX_PAGES;
use crate::protocol::acts::{Act, parse};
use crate::protocol::names::Kind;
use crate::rules::fields::Field;
use crate::services::ticket_loader::Ticket;
use crate::state::{NoteKind, NoteState, PostedNote, TOO_LARGE_RECHECK_SECS, TicketSnap};
use crate::ticket::TicketRow;
use crate::ticket::log::{fold_ticket, status_contradicts};
use crate::ticket::verified_messages;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// At most this many tickets that are marked for a recheck are read in one cycle (a ticket whose
/// board row changed is read in any case).
pub const RECHECKS_PER_CYCLE: usize = 5;

/// A ticket with notes and no board row is judged as gone from the board only when its newest
/// note is older than this (10 minutes, in seconds): a long `watch` learns a new ticket from the
/// comment feed, which can lag behind the note that `ticket new` recorded.
pub const GONE_GRACE_SECS: i64 = 600;

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
pub(super) fn record(ac: &AgentCtx, t: &TicketRef, kind: Kind, body: &str, id: u64) {
    let Some(k) = note_kind(kind, body) else {
        return;
    };
    if t.repo == ac.ctx.main_repo() && t.issue == ac.ctx.cfg.registry_issue {
        return;
    }
    let now = now_epoch();
    let mut cache = ac.load_cache();
    cache.notes.record(
        &t.key(),
        PostedNote {
            id,
            kind: k,
            at: now,
        },
        now,
    );
    ac.save_cache(&mut cache);
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
    t: &TicketRef,
    read: Option<&[Comment]>,
    contradicts: bool,
    now: i64,
) -> Vec<SecurityEvent> {
    let key = t.key();
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
    gone.iter().map(|n| event(me, t, n)).collect()
}

/// Forget the notes of the `deleted-message` events in `shown`. The caller calls it only after
/// the events were shown (printed) AND reported to the owner (posted now, or found in the
/// registry issue already): an event whose report failed is not passed, so its note stays and
/// the next cycle raises it again. The ticket stays marked for one more read, which clears the
/// mark when nothing else is wrong. Other events are ignored. Pure.
pub fn settle(notes: &mut NoteState, shown: &[SecurityEvent]) {
    for s in shown.iter().filter(|s| s.reason == Reason::DeletedMessage) {
        notes.forget(&TicketRef::new(&s.repo, s.issue).key(), s.comment_id);
    }
}

fn event(me: &str, t: &TicketRef, n: &PostedNote) -> SecurityEvent {
    SecurityEvent {
        reason: Reason::DeletedMessage,
        comment_id: n.id,
        repo: t.repo.clone(),
        issue: t.issue,
        url: format!("https://github.com/{}/issues/{}", t.repo, t.issue),
        author: "(deleted)".into(),
        created_at: format_ts(n.at),
        claimed_from: Some(me.to_string()),
        detail: format!(
            "the signed {} note (comment {}) that {me} posted on {t} at {} is not among the comments of the issue any more: somebody deleted it",
            n.kind.as_str(),
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
    t: &TicketRef,
    board: Option<&str>,
    read: Option<&[Comment]>,
    now: i64,
) -> Vec<SecurityEvent> {
    let contra = read.is_some_and(|c| contradicts(ac, world, c, board));
    check_ticket(notes, ac.name.as_str(), t, read, contra, now)
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
    /// The ticket.
    pub fn ticket(&self) -> Option<TicketRef> {
        match self {
            Target::Row(r) => Some(r.tref()),
            Target::Gone(key) => TicketRef::parse(key),
        }
    }
}

/// The tickets that the cycle reads for the check: a ticket with remembered notes whose board row
/// changed since the last cycle (`prev`) and not by this cycle (`own`); and at most
/// [`RECHECKS_PER_CYCLE`] of the tickets that are marked for a recheck, that have notes and no
/// board row, or whose too-large list is due for its slow read again
/// ([`TOO_LARGE_RECHECK_SECS`]), the oldest mark first (a ticket without a row and without a mark
/// first of all; ties by key). A too-large ticket that is not due is read only when its row
/// changed. A ticket without a row whose newest note is younger than [`GONE_GRACE_SECS`] waits.
/// `done`: the tickets that the cycle read already (`log_check`). Pure.
pub fn to_read<'a>(
    rows: &'a [TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    own: &HashSet<&str>,
    done: &HashSet<String>,
    notes: &'a NoteState,
    now: i64,
) -> Vec<Target<'a>> {
    let open: Vec<(&TicketRow, String)> = rows
        .iter()
        .map(|r| (r, r.tref().key()))
        .filter(|(r, key)| !done.contains(&r.key()) && !notes.of(key).is_empty())
        .collect();
    let changed = |r: &TicketRow| {
        !own.contains(r.item_id.as_str()) && prev.get(&r.item_id).is_some_and(|p| *p != r.snap())
    };
    // `Some(at)`: a too-large ticket, read last at `at`; `due` once the slow interval is over
    let large = |k: &str| notes.notes_too_large.get(k).copied();
    let due = |at: i64| now - at >= TOO_LARGE_RECHECK_SECS;
    let mut out: Vec<Target<'a>> = open
        .iter()
        .filter(|(r, _)| changed(r))
        .map(|(r, _)| Target::Row(r))
        .collect();
    let mut marked: Vec<(i64, &str, Target<'a>)> = open
        .iter()
        .filter(|(r, _)| !changed(r))
        .filter_map(|(r, key)| {
            let (k, mark) = notes
                .notes_recheck
                .get_key_value(key)
                .map_or_else(|| (key.as_str(), None), |(k, at)| (k.as_str(), Some(*at)));
            match (mark, large(key)) {
                (_, Some(at)) if !due(at) => None,
                (Some(at), _) | (None, Some(at)) => Some((at, k, Target::Row(r))),
                (None, None) => None,
            }
        })
        .collect();
    let on_board: BTreeSet<String> = rows.iter().map(|r| r.tref().key()).collect();
    let newest = |k: &str| notes.of(k).iter().map(|n| n.at).max().unwrap_or(i64::MIN);
    marked.extend(
        notes
            .posted_notes
            .keys()
            .filter(|k| !on_board.contains(*k))
            .filter(|k| large(k).is_none_or(due))
            .filter(|k| now - newest(k) >= GONE_GRACE_SECS)
            .map(|k| {
                let at = notes
                    .notes_recheck
                    .get(k)
                    .copied()
                    .or_else(|| large(k))
                    .unwrap_or(i64::MIN);
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

/// Why the check of one ticket stops or slows down: the event `notes-not-checked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotCheckedWhy {
    /// The comment list has more than `MAX_PAGES` pages.
    TooLarge,
    /// The ticket has no board row any more.
    NoItem,
}

impl NotCheckedWhy {
    pub fn as_str(self) -> &'static str {
        match self {
            NotCheckedWhy::TooLarge => "too-large",
            NotCheckedWhy::NoItem => "no-board-item",
        }
    }
}

/// One `notes-not-checked` event: the ticket (its note key), why, and the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotChecked {
    /// The note key (`owner/repo#n`, the repo in lower case).
    pub key: String,
    pub why: NotCheckedWhy,
    pub text: String,
}

impl NotChecked {
    /// The `event=` key of the owner report: one per ticket and reason.
    pub fn event_key(&self) -> String {
        format!("{}/{NOT_CHECKED}:{}", self.key, self.why.as_str())
    }

    fn too_large(t: &TicketRef, has_notes: bool) -> Self {
        let what = if has_notes {
            "the signed notes of this coordinator on it and its board Status"
        } else {
            "its board Status"
        };
        NotChecked {
            key: t.key(),
            why: NotCheckedWhy::TooLarge,
            text: format!(
                "{t}: the comment list has more than {MAX_PAGES} pages ({} comments), too large to read complete; {what} are checked again when its board row changes, or {} hours after the last read, and a deletion is seen only when the list fits again",
                MAX_PAGES * 100,
                TOO_LARGE_RECHECK_SECS / 3600
            ),
        }
    }

    fn no_item(t: &TicketRef) -> Self {
        NotChecked {
            key: t.key(),
            why: NotCheckedWhy::NoItem,
            text: format!(
                "{t}: the ticket has no board item any more (deleted from the board?); its comments were compared once with the signed notes of this coordinator, which are not checked any more"
            ),
        }
    }
}

/// Judge one read of a ticket for the check. `board`: `Some(status)` for a ticket with a board
/// row (its board Status), `None` for a ticket without one ([`Target::Gone`]). Returns the
/// `deleted-message` events and, for a ticket that is not checked as before, one
/// `notes-not-checked` event. Neither changes the memory for good: the caller does that with
/// [`settle`] and [`settle_not_checked`] after the events were shown and reported, and until
/// then the ticket stays marked, so the next cycle raises them again.
///
/// - a list that is too large ([`ReadResult::TooLarge`]): the notes stay. The first time, the
///   event, and the ticket stays marked until it is settled. After that (the ticket is in
///   `notes_too_large`) no event and no mark: the ticket is read again only when its board row
///   changes or the slow interval is over ([`to_read`]).
/// - a complete list removes the ticket from `notes_too_large`.
/// - a ticket without a board row: its list is compared once, with the event; its notes are
///   forgotten when the event is settled. A gone note stays until its own event is settled.
///
/// `contradicts` is asked only for a complete list of a ticket with a row. Pure.
pub fn judge_read(
    notes: &mut NoteState,
    me: &str,
    t: &TicketRef,
    read: ReadResult<'_>,
    board: Option<Option<&str>>,
    contradicts: impl FnOnce(&[Comment], Option<&str>) -> bool,
    now: i64,
) -> (Vec<SecurityEvent>, Option<NotChecked>) {
    let key = t.key();
    match (read, board) {
        (ReadResult::TooLarge, _) => {
            if let Some(at) = notes.notes_too_large.get_mut(&key) {
                // told already: the next read comes with a row change or the slow interval
                *at = now;
                notes.notes_recheck.remove(&key);
                return (Vec::new(), None);
            }
            let has_notes = !notes.of(&key).is_empty();
            if has_notes {
                notes.mark(&key, now);
            }
            (Vec::new(), Some(NotChecked::too_large(t, has_notes)))
        }
        (ReadResult::Failed, _) => (check_ticket(notes, me, t, None, false, now), None),
        (ReadResult::Complete(c), Some(status)) => {
            notes.notes_too_large.remove(&key);
            let contra = contradicts(c, status);
            (check_ticket(notes, me, t, Some(c), contra, now), None)
        }
        (ReadResult::Complete(c), None) => {
            notes.notes_too_large.remove(&key);
            if notes.of(&key).is_empty() {
                return (Vec::new(), None);
            }
            let gone = check_ticket(notes, me, t, Some(c), false, now);
            notes.mark(&key, now);
            (gone, Some(NotChecked::no_item(t)))
        }
    }
}

/// Apply the `notes-not-checked` events in `shown`. The caller calls it only after the events
/// were shown (printed) AND reported to the owner, as [`settle`]. A too-large ticket goes into
/// `notes_too_large` (its notes stay; no read per cycle any more); a ticket without a board row
/// forgets its notes. The mark of the ticket ends. Pure.
pub fn settle_not_checked(notes: &mut NoteState, shown: &[NotChecked], now: i64) {
    for n in shown {
        match n.why {
            NotCheckedWhy::TooLarge => {
                notes.notes_too_large.insert(n.key.clone(), now);
            }
            NotCheckedWhy::NoItem => {
                for id in notes.of(&n.key).iter().map(|x| x.id).collect::<Vec<_>>() {
                    notes.forget(&n.key, id);
                }
            }
        }
        notes.notes_recheck.remove(&n.key);
    }
}

/// What the check of one coordinator cycle found.
#[derive(Debug, Default)]
pub(super) struct NoteCycle {
    pub(super) security: Vec<SecurityEvent>,
    /// The tickets that are not checked as before: the caller reports each to the owner and
    /// shows it as a [`NOT_CHECKED`] event (`duties`; this file does not post, so it does not
    /// import `registry_post`), then settles it ([`settle_not_checked`]).
    pub(super) not_checked: Vec<NotChecked>,
}

/// The event kind of a ticket whose notes are not checked as before.
pub const NOT_CHECKED: &str = "notes-not-checked";

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
    let todo: Vec<(TicketRef, Option<Option<String>>)> = to_read(rows, prev, own, done, notes, now)
        .into_iter()
        .filter_map(|t| {
            let ticket = t.ticket()?;
            let board = match t {
                Target::Row(r) => Some(r.status_str().map(str::to_string)),
                Target::Gone(_) => None,
            };
            Some((ticket, board))
        })
        .collect();
    for (t, board) in todo {
        let read = feed::issue_comments(
            &ac.ctx.client,
            &t.repo,
            t.issue,
            None,
            &ac.ctx.cfg.owner_login,
        );
        if let Err(e) = &read {
            eprintln!(
                "crewboard: cannot read {t} to check the notes of this coordinator ({e}); {}",
                if matches!(ReadResult::of(&read), ReadResult::TooLarge) {
                    "read again when its board row changes"
                } else {
                    "next cycle"
                }
            );
        }
        let (security, note) = judge_read(
            notes,
            ac.name.as_str(),
            &t,
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
        &t.tref(),
        t.item.get(Field::Status),
        Some(&t.comments),
        now_epoch(),
    );
    ac.save_cache_required(&mut cache)?;
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
    ac.save_cache_required(&mut cache)
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

    /// Later than every note of the tests by more than [`GONE_GRACE_SECS`].
    const NOW: i64 = 1_790_100_000;

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
        let got = check_ticket(
            &mut n,
            "c@x",
            &TicketRef::new("o/r", 7),
            None,
            false,
            1_790_000_200,
        );
        assert!(got.is_empty());
        assert_eq!(n.of("o/r#7").len(), 2, "nothing is forgotten");
        assert!(n.notes_recheck.contains_key("o/r#7"));
        // the next complete read finds the deletion; the ticket stays marked until the event
        // is settled
        let got = check_ticket(
            &mut n,
            "c@x",
            &TicketRef::new("o/r", 7),
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
                &TicketRef::new("o/r", 7),
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
        let got = check_ticket(
            &mut n,
            "c@x",
            &TicketRef::new("o/r", 7),
            Some(&read),
            false,
            1_790_000_200,
        );
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
            check_ticket(
                &mut n,
                "c@x",
                &TicketRef::new("o/r", 7),
                Some(&read),
                false,
                1_790_000_200
            )
            .is_empty()
        );
        assert_eq!(n.of("o/r#7"), &[note(10, "assignment")]);
        // a ticket without memory is not compared at all
        assert!(
            check_ticket(
                &mut n,
                "c@x",
                &TicketRef::new("o/r", 8),
                Some(&[]),
                true,
                1_790_000_200
            )
            .is_empty()
        );
        assert!(!n.notes_recheck.contains_key("o/r#8"));
    }

    #[test]
    fn a_deleted_note_stays_remembered_and_is_raised_again_until_it_is_settled() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        let read = [comment(10)];
        let first = check_ticket(
            &mut n,
            "c@x",
            &TicketRef::new("o/r", 7),
            Some(&read),
            false,
            1_790_000_200,
        );
        assert_eq!(first.len(), 1);
        // the report failed: nothing is settled, so the note and the mark stay
        assert_eq!(n.of("o/r#7").len(), 2);
        assert!(n.notes_recheck.contains_key("o/r#7"));
        let again = check_ticket(
            &mut n,
            "c@x",
            &TicketRef::new("o/r", 7),
            Some(&read),
            false,
            1_790_000_200,
        );
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
            check_ticket(
                &mut n,
                "c@x",
                &TicketRef::new("o/r", 7),
                Some(&read),
                false,
                1_790_000_200
            )
            .is_empty()
        );
    }

    #[test]
    fn a_board_that_contradicts_the_log_keeps_the_ticket_for_a_read_next_cycle() {
        let mut n = notes(&[note(12, "close")]);
        let read = [comment(12)];
        assert!(
            check_ticket(
                &mut n,
                "c@x",
                &TicketRef::new("o/r", 7),
                Some(&read),
                true,
                1_790_000_200
            )
            .is_empty()
        );
        assert!(n.notes_recheck.contains_key("o/r#7"));
        // the board agrees again: no more reads
        assert!(
            check_ticket(
                &mut n,
                "c@x",
                &TicketRef::new("o/r", 7),
                Some(&read),
                false,
                1_790_000_200
            )
            .is_empty()
        );
        assert!(!n.notes_recheck.contains_key("o/r#7"));
        // a contradiction without a close note of this coordinator costs no read per cycle
        let mut a = notes(&[note(10, "assignment")]);
        assert!(
            check_ticket(
                &mut a,
                "c@x",
                &TicketRef::new("o/r", 7),
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
            item_id: item.into(),
            ..crate::testkit::row(issue, status, None, None)
        }
    }

    fn issue_of(t: &Target<'_>) -> u64 {
        t.ticket().unwrap().issue
    }

    fn snap(status: &str) -> TicketSnap {
        TicketSnap {
            status: Some(status.into()),
            agent: None,
        }
    }

    #[test]
    fn the_note_key_does_not_depend_on_the_spelling_of_the_repo() {
        assert_eq!(
            TicketRef::new("Org/Repo", 7).key(),
            TicketRef::new("org/repo", 7).key()
        );
        assert_eq!(TicketRef::new("Org/Repo", 7).key(), "org/repo#7");
        // recorded as typed (`Org/Repo#7`), compared with the config spelling of the rows
        let mut n = NoteState::default();
        n.record(
            &TicketRef::new("Org/Repo", 7).key(),
            note(12, "close"),
            1_790_000_100,
        );
        let got = check_ticket(
            &mut n,
            "c@x",
            &TicketRef::new("org/repo", 7),
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
        m.record(
            &TicketRef::new("Org/Repo", 7).key(),
            note(12, "close"),
            1_790_000_100,
        );
        let rows = [TicketRow {
            repo: "org/repo".into(),
            ..row("moved", 7, "Review")
        }];
        let prev: BTreeMap<_, _> = [("moved".to_string(), snap("Done"))].into();
        let got = to_read(&rows, &prev, &HashSet::new(), &HashSet::new(), &m, NOW);
        assert_eq!(got.len(), 1);
        // control: another issue number is another ticket
        assert!(
            check_ticket(
                &mut m,
                "c@x",
                &TicketRef::new("org/repo", 8),
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
            let got: Vec<u64> = to_read(&rows, &prev, &own, &done, &n, NOW)
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
                check_ticket(
                    &mut n,
                    "c@x",
                    &TicketRef::new("o/r", i),
                    Some(&read),
                    true,
                    2_000 + cycle,
                );
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
        let got = to_read(&rows, &moved, &own, &done, &n, NOW);
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
        let got: Vec<&str> = to_read(&rows, &prev, &own, &done, &n, NOW)
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
        let got = to_read(&rows, &prev, &none, &HashSet::new(), &n, NOW);
        // the row-less ticket has no mark: it goes first
        assert_eq!(got, vec![Target::Gone("o/r#7"), Target::Row(&rows[0])]);
        assert_eq!(got[0].ticket(), Some(TicketRef::new("o/r", 7)));
        // its read compares once: the deleted close note is raised, and one note
        let read = [comment(10)];
        let (ev, nc) = judge_read(
            &mut n,
            "c@x",
            &TicketRef::new("o/r", 7),
            ReadResult::Complete(&read),
            None,
            |_, _| unreachable!("a ticket without a row has no board Status"),
            NOW,
        );
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].comment_id, 12);
        let nc = nc.unwrap();
        assert!(nc.text.contains("no board item any more"), "{}", nc.text);
        assert_eq!(nc.event_key(), "o/r#7/notes-not-checked:no-board-item");
        // nothing is forgotten before both are settled (a cycle that fails later reads again)
        assert_eq!(n.of("o/r#7").len(), 2);
        let got = to_read(&rows, &prev, &none, &HashSet::new(), &n, NOW);
        assert!(got.contains(&Target::Gone("o/r#7")));
        settle(&mut n, &ev);
        settle_not_checked(&mut n, std::slice::from_ref(&nc), NOW);
        assert!(n.of("o/r#7").is_empty() && !n.notes_recheck.contains_key("o/r#7"));
        let got = to_read(&rows, &prev, &none, &HashSet::new(), &n, NOW);
        assert_eq!(
            got,
            vec![Target::Row(&rows[0])],
            "read once, then never again"
        );
        // control: without a deletion the read gives only the note, and the settle ends the
        // memory
        let mut m = notes(&[note(10, "assignment")]);
        let (ev, nc) = judge_read(
            &mut m,
            "c@x",
            &TicketRef::new("o/r", 7),
            ReadResult::Complete(&read),
            None,
            |_, _| false,
            NOW,
        );
        assert!(ev.is_empty() && nc.is_some());
        settle_not_checked(&mut m, &[nc.unwrap()], NOW);
        assert!(m.posted_notes.is_empty());
    }

    #[test]
    fn a_new_ticket_without_a_row_yet_is_not_taken_for_a_deleted_item() {
        // `ticket new` recorded the creation note a moment ago; the index of a long `watch` has
        // not learned the ticket yet, so it has no row
        let mut n = NoteState::default();
        n.record(
            "o/r#9",
            PostedNote {
                id: 90,
                kind: NoteKind::Creation,
                at: NOW - 30,
            },
            NOW,
        );
        assert!(
            to_read(
                &[],
                &BTreeMap::new(),
                &HashSet::new(),
                &HashSet::new(),
                &n,
                NOW
            )
            .is_empty()
        );
        // after the grace time it is judged as gone
        let later = NOW + GONE_GRACE_SECS;
        assert_eq!(
            to_read(
                &[],
                &BTreeMap::new(),
                &HashSet::new(),
                &HashSet::new(),
                &n,
                later
            ),
            vec![Target::Gone("o/r#9")]
        );
    }

    #[test]
    fn a_list_that_is_too_large_keeps_the_notes_and_is_read_again_when_the_row_changes() {
        let mut n = notes(&[note(10, "assignment"), note(12, "close")]);
        let rows = [row("seven", 7, "Dropped")];
        let same: BTreeMap<_, _> = [("seven".to_string(), snap("Dropped"))].into();
        let none = HashSet::new();
        let too_large = |n: &mut NoteState, now: i64| {
            judge_read(
                n,
                "c@x",
                &TicketRef::new("o/r", 7),
                ReadResult::TooLarge,
                Some(Some("Dropped")),
                |_, _| unreachable!(),
                now,
            )
        };
        // the flood: one event, the notes stay, and the ticket stays marked until it is settled
        let (ev, nc) = too_large(&mut n, NOW);
        assert!(ev.is_empty());
        let nc = nc.unwrap();
        assert!(nc.text.contains("more than 50 pages"), "{}", nc.text);
        assert_eq!(nc.event_key(), "o/r#7/notes-not-checked:too-large");
        assert_eq!(n.of("o/r#7").len(), 2);
        // a cycle that failed after it (not settled): the retry reads again and raises it again
        assert_eq!(to_read(&rows, &same, &none, &none_s(), &n, NOW).len(), 1);
        assert!(too_large(&mut n, NOW).1.is_some());
        // shown and reported: no read per cycle any more, and no second event
        settle_not_checked(&mut n, &[nc], NOW);
        assert_eq!(n.of("o/r#7").len(), 2, "the notes stay");
        assert!(to_read(&rows, &same, &none, &none_s(), &n, NOW + 60).is_empty());
        assert_eq!(too_large(&mut n, NOW + 60), (Vec::new(), None));
        // the slow interval: read again once it is over
        let due = NOW + 60 + TOO_LARGE_RECHECK_SECS;
        assert_eq!(to_read(&rows, &same, &none, &none_s(), &n, due).len(), 1);
        // the attacker deletes the close note and the flood, and sets the board back: the row
        // changed, so the ticket is read, the list is complete, and the deletion is seen
        let back = [row("seven", 7, "In Progress")];
        let got = to_read(&back, &same, &none, &none_s(), &n, NOW + 120);
        assert_eq!(got, vec![Target::Row(&back[0])]);
        let read = [comment(10)];
        let (ev, nc) = judge_read(
            &mut n,
            "c@x",
            &TicketRef::new("o/r", 7),
            ReadResult::Complete(&read),
            Some(Some("In Progress")),
            |_, _| false,
            NOW + 120,
        );
        assert!(nc.is_none());
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].comment_id, 12);
        assert!(
            !n.notes_too_large.contains_key("o/r#7"),
            "the list fits again"
        );
        // control: another error keeps the memory and marks a recheck
        let mut m = notes(&[note(12, "close")]);
        let (ev, nc) = judge_read(
            &mut m,
            "c@x",
            &TicketRef::new("o/r", 7),
            ReadResult::Failed,
            Some(None),
            |_, _| false,
            11,
        );
        assert!(ev.is_empty() && nc.is_none());
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

    fn none_s() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn an_unknown_note_kind_keeps_its_name_in_the_state_file() {
        let n: PostedNote =
            serde_json::from_str(r#"{"id":5,"kind":"merge-intent","at":1}"#).unwrap();
        assert_eq!(n.kind, NoteKind::Other("merge-intent".into()));
        let back = serde_json::to_string(&n).unwrap();
        assert!(back.contains(r#""kind":"merge-intent""#), "{back}");
        // control: a known name reads as its kind
        let k: PostedNote =
            serde_json::from_str(r#"{"id":5,"kind":"verify-pass","at":1}"#).unwrap();
        assert_eq!(k.kind, NoteKind::VerifyPass);
    }
}
