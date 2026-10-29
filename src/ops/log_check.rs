//! The coordinator's check of the board `Status` against the signed log.
//!
//! The crew token has Write on the board, so a holder can set `Status = Done` on any ticket
//! with one call. crewboard cannot prevent that. It detects it: the same judgement as the
//! `field-mismatch` of `audit` ([`crate::ticket::log::status_contradicts`]) runs for the tickets
//! that another writer moved to Done since the last cycle (Done is final, so it is the one move
//! that matters at once; `status` and `audit` check every Status). Reading the comments of a
//! ticket costs one REST call, so only such a ticket is read, never the whole board. The
//! agent's own writes are not in the list: its snapshot follows them.
//!
//! A legitimate writer changes the field a moment before it posts its message, so a mismatch
//! seen once may be a write in flight. A ticket is reported when the mismatch is still there at
//! the next cycle and the item has been quiet for [`QUIET_SECS`]. The report is ONE signed
//! `info` to the owner per ticket and board status (the line `event=` is the key, read back
//! from the registry issue like the security events, so a restart or a lost cache repeats
//! nothing), and one `status-mismatch` event for the coordinator.

use super::registry_post::{liveness_event, post_registry};
use super::{AgentCtx, World};
use crate::audit::reported_events;
use crate::events::{Event, SecurityEvent};
use crate::feed;
use crate::protocol::names::{Kind, Re, To};
use crate::state::{StateCache, TicketSnap};
use crate::ticket::log::{fold_ticket, status_contradicts, status_mismatch_text};
use crate::ticket::{TicketRow, VMsg, verified_messages};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A mismatch counts only when the item has not changed for this long (seconds).
pub const QUIET_SECS: i64 = 60;

/// The tickets to check this cycle: a Status that changed to Done or Dropped since the last
/// snapshot, or a
/// mismatch that an earlier cycle saw and could not report yet. A ticket that this cycle changed
/// itself (`own`) is skipped. Pure.
pub fn candidates<'a>(
    rows: &'a [TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    pending: &BTreeSet<String>,
    own: &HashSet<&str>,
) -> Vec<&'a TicketRow> {
    rows.iter()
        .filter(|r| !own.contains(r.item_id.as_str()))
        .filter(|r| {
            pending.contains(&r.item_id)
                || (r.ended()
                    && prev
                        .get(&r.item_id)
                        .is_some_and(|p| p.status.as_deref() != r.status_str()))
        })
        .collect()
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

/// The key of the report: one per ticket and board status.
pub fn event_key(repo: &str, issue: u64, board_status: &str) -> String {
    format!("{repo}#{issue}/status:{board_status}")
}

/// The signed `info` to the owner.
fn report_body(key: &str, ekey: &str, text: &str) -> String {
    format!(
        "Status mismatch: {key}\nevent={ekey}\n{text}.\nNo signed message supports this Status: somebody wrote the board field directly (the crew token can). A Done or Dropped without proof is detected, not prevented. Check the ticket and the board history.\n"
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
    let todo = candidates(
        rows,
        &cache.index.ticket_snaps,
        &cache.liveness.status_pending,
        &own_ids,
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
        let comments = match feed::issue_comments(
            &ac.ctx.client,
            &row.repo,
            row.issue,
            None,
            &ac.ctx.cfg.owner_login,
        ) {
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
                    &row.repo,
                    row.issue,
                    row.status_str(),
                    None,
                ));
                continue;
            }
        };
        out.security.extend(super::own_notes::check_read(
            ac,
            world,
            &mut cache.notes,
            &row.repo,
            row.issue,
            row.status_str(),
            Some(&comments),
        ));
        let log = fold_ticket(&verified_messages(&comments, &ac.ctx.cfg, &world.trust));
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
                let ekey = event_key(&row.repo, row.issue, board.unwrap_or("empty"));
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
            issue: 1,
            item_id: item.into(),
            status: Some(status.into()),
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
    fn only_a_ticket_that_moved_to_an_end_or_is_pending_is_a_candidate() {
        let rows = [
            row("same", "Review"),
            row("moved", "Done"),
            row("new", "Done"),
            row("late", "Done"),
            row("mine", "Done"),
            row("review", "Review"),
            row("dropped", "Dropped"),
        ];
        let prev: BTreeMap<_, _> = [
            ("same".to_string(), snap("Review")),
            ("moved".to_string(), snap("Review")),
            ("late".to_string(), snap("Done")),
            ("mine".to_string(), snap("Review")),
            ("review".to_string(), snap("In Progress")),
            ("dropped".to_string(), snap("Blocked")),
        ]
        .into();
        let pending: BTreeSet<_> = ["late".to_string()].into();
        let own: HashSet<&str> = ["mine"].into();
        let got: Vec<&str> = candidates(&rows, &prev, &pending, &own)
            .iter()
            .map(|r| r.item_id.as_str())
            .collect();
        // `new` has no snapshot (a fresh cache replays nothing); `mine` was changed by us;
        // `review` changed, but not to Done or Dropped (`status` and `audit` cover it); a move to
        // Dropped ends the ticket like Done
        assert_eq!(got, vec!["moved", "late", "dropped"]);
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
    fn the_key_names_the_ticket_and_the_board_status() {
        assert_eq!(event_key("o/r", 7, "Done"), "o/r#7/status:Done");
        let body = report_body(
            "o/r#7",
            "o/r#7/status:Done",
            "ticket o/r#7: board Status is Done, the log shows Review",
        );
        assert!(body.contains("\nevent=o/r#7/status:Done\n"), "{body}");
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
        assert!(reported_events(&[m]).contains("o/r#7/status:Done"));
    }
}
