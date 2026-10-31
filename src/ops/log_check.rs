//! The coordinator's check of the board `Status` against the signed log.
//!
//! The agent token has Write on the board, so a holder can set `Status = Done` on any ticket
//! with one call. crewboard cannot prevent that. It detects it: the same judgement as the
//! `field-mismatch` of `audit` ([`crate::ticket::log::status_contradicts`]) runs for the tickets
//! that another writer moved to Done or Dropped since the last cycle, or moved back out of Done or
//! Dropped (an end is final, so these are the moves that matter at once: a false end, and a
//! ticket set back behind a close; `status` and `audit` check every Status). Reading the comments
//! of a ticket costs one comment-list read (up to 50 pages), so only such a ticket is read, never
//! the whole board. The agent's own writes are not in the list: its snapshot follows them.
//!
//! A legitimate writer changes the field a moment before it posts its message, so a mismatch
//! seen once may be a write in flight. A ticket is reported when the mismatch is still there at
//! the next cycle and the item has been quiet for [`QUIET_SECS`]. The report is ONE signed
//! `info` to the owner per ticket and state of its signed log (the line `event=` is the key,
//! read back from the registry issue like the security events, so a restart or a lost cache
//! repeats nothing, and a board that flips between statuses gives one report), and one
//! `status-mismatch` event for the coordinator.
//!
//! Limits of the reads: a move to Done or Dropped and a mismatch that waits are read at once. A
//! move out of Done or Dropped waits in a queue (`setback_queue`), and a cycle reads at most
//! [`SETBACKS_PER_CYCLE`] of them, the oldest first. A ticket whose comment list is too large
//! (more than `MAX_PAGES` pages) cannot be judged: one `notes-not-checked` event, and the
//! ticket stays under check (a false Done too). It is read again when its board row changes or
//! after the slow interval of `own_notes` (`NoteState::notes_too_large`), not in every cycle.

use super::own_notes::{NotChecked, ReadResult};
use super::registry_post::{liveness_event, post_registry};
use super::{AgentCtx, World};
use crate::audit::reported_events;
use crate::base::TicketRef;
use crate::events::{Event, SecurityEvent};
use crate::feed;
use crate::protocol::names::{Kind, Re, To};
use crate::state::{StateCache, TOO_LARGE_RECHECK_SECS, TicketSnap};
use crate::ticket::log::{fold_ticket, status_contradicts, status_mismatch_text};
use crate::ticket::{TicketRow, VMsg, verified_messages};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A mismatch counts only when the item has not changed for this long (seconds).
pub const QUIET_SECS: i64 = 60;

/// At most this many tickets that another writer moved out of Done or Dropped are read in one
/// cycle, the oldest move first; the others wait in `setback_queue` for a later cycle.
pub const SETBACKS_PER_CYCLE: usize = 5;

/// The tickets to check this cycle: a Status that changed to Done or Dropped since the last
/// snapshot, a mismatch that an earlier cycle saw and could not settle yet (`pending`; one that
/// `waiting` holds back is read only when its row moves), and at most [`SETBACKS_PER_CYCLE`]
/// tickets whose Status changed from Done or Dropped to another one (the board was set back
/// behind a close). A set-back goes into `queue` first, with `now`; the oldest leave it when they
/// are read. A ticket that this cycle changed itself (`own`) is skipped. Pure.
pub fn candidates<'a>(
    rows: &'a [TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    pending: &BTreeSet<String>,
    queue: &mut BTreeMap<String, i64>,
    own: &HashSet<&str>,
    waiting: impl Fn(&TicketRow) -> bool,
    now: i64,
) -> Vec<&'a TicketRow> {
    let was_ended = |p: &TicketSnap| {
        p.status
            .as_deref()
            .and_then(crate::rules::status::Status::parse)
            .is_some_and(crate::rules::status::Status::ended)
    };
    queue.retain(|i, _| rows.iter().any(|r| r.item_id == *i));
    let mut out = Vec::new();
    for r in rows.iter().filter(|r| !own.contains(r.item_id.as_str())) {
        let moved = prev
            .get(&r.item_id)
            .filter(|p| p.status.as_deref() != r.status_str());
        if (moved.is_some() && r.ended()) || (pending.contains(&r.item_id) && !waiting(r)) {
            queue.remove(&r.item_id);
            out.push(r);
        } else if moved.is_some_and(was_ended) {
            queue.entry(r.item_id.clone()).or_insert(now);
        }
    }
    let mut oldest: Vec<(i64, String)> = queue
        .iter()
        .filter(|(i, _)| !own.contains(i.as_str()))
        .map(|(i, at)| (*at, i.clone()))
        .collect();
    oldest.sort();
    for (_, item) in oldest.into_iter().take(SETBACKS_PER_CYCLE) {
        queue.remove(&item);
        out.extend(rows.iter().find(|r| r.item_id == item));
    }
    out
}

/// What to do with one checked ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Board and log agree (or the log says nothing): forget the ticket.
    Clear,
    /// A mismatch that may be a write in flight: look again at the next cycle.
    Wait,
    /// A mismatch that stayed: tell the owner.
    Report,
}

/// `contradicts`: the board Status contradicts the log. `was_pending`: an earlier cycle saw the
/// mismatch. `quiet`: the item has not changed for [`QUIET_SECS`]. Pure.
pub fn judge(contradicts: bool, was_pending: bool, quiet: bool) -> Verdict {
    match (contradicts, was_pending && quiet) {
        (false, _) => Verdict::Clear,
        (true, true) => Verdict::Report,
        (true, false) => Verdict::Wait,
    }
}

/// The key of the report: one per ticket (the repo in lower case) and state of its signed log,
/// named by the id of the latest verified comment of the ticket (`0`: none). A board that flips
/// between statuses gives one report; the owner hears again only after a new signed message,
/// which no agent token can write.
pub fn event_key(t: &TicketRef, last_signed: u64) -> String {
    format!("{}/status@c-{last_signed}", t.key())
}

/// The signed `info` to the owner.
fn report_body(key: &str, ekey: &str, text: &str) -> String {
    format!(
        "Status mismatch: {key}\nevent={ekey}\n{text}.\nNo signed message supports this Status: somebody wrote the board field directly (the agent token can). A board Status without a signed message (a Done or Dropped without proof, a ticket set back behind its close) is detected, not prevented. Check the ticket and the board history.\n"
    )
}

/// What the check of one cycle found.
#[derive(Debug, Default)]
pub(super) struct LogCheck {
    pub(super) events: Vec<Event>,
    /// The `deleted-message` events of the lists that the check read (`own_notes`).
    pub(super) security: Vec<SecurityEvent>,
    /// The tickets (`owner/repo#n`) whose comment list the check read: the note check of the
    /// cycle does not read them again.
    pub(super) read: HashSet<String>,
    /// The tickets whose list was too large to judge (`own_notes::judge_read`): the caller
    /// reports, shows and settles them as the note check's.
    pub(super) not_checked: Vec<NotChecked>,
}

/// Check the tickets that changed. Never fails the cycle: an unreadable ticket is looked at
/// again next time. `own`: the tickets that this cycle changed itself. The comment list that it
/// reads goes through the note check too (`own_notes::check_read`).
pub(super) fn status_log_duty(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    own: &[(String, TicketSnap)],
    reg_msgs: &[VMsg],
    cache: &mut StateCache,
    now: i64,
) -> LogCheck {
    let own_ids: HashSet<&str> = own.iter().map(|(i, _)| i.as_str()).collect();
    // a too-large ticket that waits for its slow read (`own_notes`)
    let large = cache.notes.notes_too_large.clone();
    let waiting = |r: &TicketRow| {
        large
            .get(&r.tref().key())
            .is_some_and(|at| now - at < TOO_LARGE_RECHECK_SECS)
    };
    let todo = candidates(
        rows,
        &cache.index.ticket_snaps,
        &cache.liveness.status_pending,
        &mut cache.liveness.setback_queue,
        &own_ids,
        waiting,
        now,
    );
    // A ticket that left the list (or is not checked now) keeps no memory.
    cache
        .liveness
        .status_pending
        .retain(|i| rows.iter().any(|r| r.item_id == *i));
    let mut reported = reported_events(reg_msgs);
    let mut out = LogCheck::default();
    for row in todo {
        let quiet = row.updated.is_none_or(|u| now - u >= QUIET_SECS);
        let was_pending = cache.liveness.status_pending.contains(&row.item_id);
        // A mismatch that waits for the item to be quiet needs no new read.
        if was_pending && !quiet {
            continue;
        }
        // read or not, the note check of this cycle does not read it again
        out.read.insert(row.key());
        let read = feed::issue_comments(
            &ac.ctx.client,
            &row.repo,
            row.issue,
            None,
            &ac.ctx.cfg.owner_login,
        );
        if let ReadResult::TooLarge = ReadResult::of(&read) {
            // The Status cannot be judged now, so it stays under check (a false Done too). It is
            // read again when its row moves or after the slow interval (`waiting`), not in
            // every cycle; the event comes once (`own_notes::judge_read`).
            eprintln!(
                "crewboard: cannot check {} against the log: its comment list is too large; read again when its board row changes",
                row.key()
            );
            cache.liveness.status_pending.insert(row.item_id.clone());
            let (_, note) = super::own_notes::judge_read(
                &mut cache.notes,
                ac.name.as_str(),
                &row.tref(),
                ReadResult::TooLarge,
                Some(row.status_str()),
                |_, _| false,
                now,
            );
            out.not_checked.extend(note);
            continue;
        }
        let comments = match read {
            Ok((c, _)) => c,
            Err(e) => {
                eprintln!(
                    "crewboard: cannot read {} to check its Status against the log ({e}); next cycle",
                    row.key()
                );
                cache.liveness.status_pending.insert(row.item_id.clone());
                out.security.extend(super::own_notes::check_read(
                    ac,
                    world,
                    &mut cache.notes,
                    &row.tref(),
                    row.status_str(),
                    None,
                    now,
                ));
                continue;
            }
        };
        out.security.extend(super::own_notes::check_read(
            ac,
            world,
            &mut cache.notes,
            &row.tref(),
            row.status_str(),
            Some(&comments),
            now,
        ));
        let msgs = verified_messages(&comments, &ac.ctx.cfg, &world.trust);
        let last_signed = msgs.last().map_or(0, |m| m.comment_id);
        let log = fold_ticket(&msgs);
        let board = row.status_str();
        match judge(status_contradicts(&log, board), was_pending, quiet) {
            Verdict::Clear => {
                cache.liveness.status_pending.remove(&row.item_id);
            }
            Verdict::Wait => {
                cache.liveness.status_pending.insert(row.item_id.clone());
            }
            Verdict::Report => {
                cache.liveness.status_pending.remove(&row.item_id);
                let ekey = event_key(&row.tref(), last_signed);
                if reported.contains(&ekey) {
                    continue;
                }
                let text = status_mismatch_text(&row.key(), board, &log);
                let body = report_body(&row.key(), &ekey, &text);
                match post_registry(ac, To::Owner, Kind::Info, Re::None, &body) {
                    Ok(p) => {
                        reported.insert(ekey);
                        out.events.push(liveness_event(
                            ac,
                            "status-mismatch",
                            format!("{text}; told the owner: {}", p.url),
                        ));
                    }
                    Err(e) => {
                        eprintln!(
                            "crewboard: cannot report the Status mismatch of {}: {e}",
                            row.key()
                        );
                        cache.liveness.status_pending.insert(row.item_id.clone());
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(item: &str, status: &str) -> TicketRow {
        TicketRow {
            repo: "o/r".into(),
            item_id: item.into(),
            ..crate::testkit::row(1, status, None, None)
        }
    }

    fn snap(status: &str) -> TicketSnap {
        TicketSnap {
            status: Some(status.into()),
            agent: None,
        }
    }

    #[test]
    fn only_a_ticket_that_moved_to_an_end_or_is_pending_is_a_candidate() {
        let rows = [
            row("same", "Review"),
            row("moved", "Done"),
            row("new", "Done"),
            row("late", "Done"),
            row("mine", "Done"),
            row("review", "Review"),
            row("dropped", "Dropped"),
            row("setback", "In Progress"),
            row("reopened", "Review"),
        ];
        let prev: BTreeMap<_, _> = [
            ("same".to_string(), snap("Review")),
            ("moved".to_string(), snap("Review")),
            ("late".to_string(), snap("Done")),
            ("mine".to_string(), snap("Review")),
            ("review".to_string(), snap("In Progress")),
            ("dropped".to_string(), snap("Blocked")),
            ("setback".to_string(), snap("Done")),
            ("reopened".to_string(), snap("Dropped")),
        ]
        .into();
        let pending: BTreeSet<_> = ["late".to_string()].into();
        let own: HashSet<&str> = ["mine"].into();
        let mut queue = BTreeMap::new();
        let got: Vec<&str> = candidates(&rows, &prev, &pending, &mut queue, &own, |_| false, 9)
            .iter()
            .map(|r| r.item_id.as_str())
            .collect();
        // `new` has no snapshot (a fresh cache replays nothing); `mine` was changed by us;
        // `review` changed, but not to or from Done or Dropped (`status` and `audit` cover it); a
        // move to Dropped ends the ticket like Done; a move out of Done or Dropped sets the board
        // back behind a close (through the queue, so they come last)
        assert_eq!(got, vec!["moved", "late", "dropped", "reopened", "setback"]);
        assert!(queue.is_empty(), "{queue:?}");
        // a pending ticket that waits (a too-large list) is read only when its row moves
        let got = candidates(
            &rows,
            &prev,
            &pending,
            &mut queue,
            &own,
            |r| r.item_id == "late",
            9,
        );
        assert!(!got.iter().any(|r| r.item_id == "late"));
        let moved: BTreeMap<_, _> = [("late".to_string(), snap("Review"))].into();
        let got = candidates(&rows, &moved, &pending, &mut queue, &own, |_| true, 9);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].item_id, "late");
    }

    #[test]
    fn at_most_five_set_backs_are_read_per_cycle_the_oldest_first() {
        let rows: Vec<TicketRow> = (1..=12)
            .map(|i| row(&format!("it{i:02}"), "Review"))
            .collect();
        let prev: BTreeMap<_, _> = rows
            .iter()
            .map(|r| (r.item_id.clone(), snap("Done")))
            .collect();
        let none = BTreeSet::new();
        let own = HashSet::new();
        let mut queue = BTreeMap::new();
        let ids = |v: Vec<&TicketRow>| v.iter().map(|r| r.item_id.clone()).collect::<Vec<_>>();
        let first = ids(candidates(
            &rows,
            &prev,
            &none,
            &mut queue,
            &own,
            |_| false,
            100,
        ));
        assert_eq!(first, ["it01", "it02", "it03", "it04", "it05"]);
        assert_eq!(queue.len(), 7);
        // the next cycles: the snapshot follows the board, so only the queue is left
        let now: BTreeMap<_, _> = rows
            .iter()
            .map(|r| (r.item_id.clone(), snap("Review")))
            .collect();
        let second = ids(candidates(
            &rows,
            &now,
            &none,
            &mut queue,
            &own,
            |_| false,
            160,
        ));
        assert_eq!(second, ["it06", "it07", "it08", "it09", "it10"]);
        let third = ids(candidates(
            &rows,
            &now,
            &none,
            &mut queue,
            &own,
            |_| false,
            220,
        ));
        assert_eq!(third, ["it11", "it12"]);
        assert!(queue.is_empty());
        // a queued ticket that left the board leaves the queue
        let mut q: BTreeMap<String, i64> = [("gone".to_string(), 1)].into();
        assert!(candidates(&rows, &now, &none, &mut q, &own, |_| false, 300).is_empty());
        assert!(q.is_empty());
    }

    #[test]
    fn a_mismatch_is_reported_only_when_it_stayed_and_the_item_is_quiet() {
        assert_eq!(judge(false, true, true), Verdict::Clear);
        assert_eq!(judge(false, false, false), Verdict::Clear);
        assert_eq!(judge(true, false, true), Verdict::Wait);
        assert_eq!(judge(true, true, false), Verdict::Wait);
        assert_eq!(judge(true, true, true), Verdict::Report);
    }

    #[test]
    fn the_key_names_the_ticket_and_the_state_of_its_signed_log() {
        // one key per ticket and signed log, whatever the board shows; the repo in lower case
        assert_eq!(
            event_key(&TicketRef::new("O/R", 7), 1234),
            "o/r#7/status@c-1234"
        );
        assert_eq!(
            event_key(&TicketRef::new("o/r", 7), 1234),
            event_key(&TicketRef::new("O/R", 7), 1234)
        );
        assert_ne!(
            event_key(&TicketRef::new("o/r", 7), 1234),
            event_key(&TicketRef::new("o/r", 7), 1300)
        );
        let body = report_body(
            "o/r#7",
            "o/r#7/status@c-1234",
            "ticket o/r#7: board Status is Done, the log shows Review",
        );
        assert!(body.contains("\nevent=o/r#7/status@c-1234\n"), "{body}");
        assert!(body.contains("detected, not prevented"), "{body}");
        // `reported_events` reads the key back from the posted text
        let m = VMsg {
            comment_id: 1,
            created: 0,
            ts: String::new(),
            sender: crate::ticket::Sender::Agent {
                name: "c@h".into(),
                role: crate::rules::Role::Coordinator,
            },
            to: "owner".into(),
            kind: Some(Kind::Info),
            re: Re::None,
            body,
        };
        assert!(reported_events(&[m]).contains("o/r#7/status@c-1234"));
    }
}
