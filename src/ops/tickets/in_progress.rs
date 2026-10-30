//! The In Progress bookkeeping shared by `take`, `done`, `block` and `ask`: which ticket of an agent is busy, when the agent goes idle, which ticket `take --interrupt` parks.

use crate::board::FieldValue;
use crate::error::CbResult;
use crate::ops::{AgentCtx, World};
use crate::rules::Refusal;
use crate::rules::fields::Field;
use crate::rules::status::Status;
use crate::services::ticket_loader::Ticket;

/// The key of a ticket of `me` that is In Progress, other than `except_item_id` (the ticket
/// about to be taken). An agent is busy when this returns a key (spec/board#lifecycle).
pub(crate) fn in_progress_key(
    rows: &[crate::ticket::TicketRow],
    me: &str,
    except_item_id: &str,
) -> Option<String> {
    rows.iter()
        .find(|r| r.item_id != except_item_id && r.is_agent(me) && r.in_status(Status::InProgress))
        .map(|r| r.key())
}

/// The agent `Doing` text after `block` / `ask` / `done` of the ticket `left_item_id`: `idle`
/// only for the assignee, and only when `rows` (the agent's own tickets) hold no other In
/// Progress ticket. `Doing` is a display text. The agent State is not written.
pub(crate) fn goes_idle(
    me: &str,
    assignee: Option<&str>,
    left_item_id: &str,
    rows: &[crate::ticket::TicketRow],
) -> bool {
    assignee == Some(me) && in_progress_key(rows, me, left_item_id).is_none()
}

/// Write `Doing = idle` when the agent has no other ticket In Progress. One read of the
/// agent's own rows, and only for the assignee.
pub(super) fn write_idle_when_free(ac: &AgentCtx, world: &World, t: &Ticket) -> CbResult<()> {
    if t.agent() != Some(ac.name.as_str()) {
        return Ok(());
    }
    let rows = crate::ops::lifecycle::my_rows(ac, world)?;
    if goes_idle(ac.name.as_str(), t.agent(), &t.item_id, &rows) {
        ac.write_own(Field::Doing, FieldValue::Text("idle"))?;
    }
    Ok(())
}

/// `take --interrupt`: the ticket to park, chosen from the agent's own rows by the board.
/// Zero In Progress: nothing to park. More than one: `--park` must name one of them.
/// `named` is the `--park` ticket as `(repo, issue)`. Pure.
pub(crate) fn choose_park<'a>(
    rows: &'a [crate::ticket::TicketRow],
    named: Option<(&str, u64)>,
) -> Result<&'a crate::ticket::TicketRow, Refusal> {
    let cur: Vec<&crate::ticket::TicketRow> = rows
        .iter()
        .filter(|r| r.in_status(Status::InProgress))
        .collect();
    let keys = cur.iter().map(|r| r.key()).collect::<Vec<_>>().join(", ");
    if cur.is_empty() {
        return Err(Refusal::new(
            "interrupt-nothing-to-park",
            "--interrupt parks a ticket of yours that is In Progress, and you have none: use a plain `take`",
        ));
    }
    match (named, cur.as_slice()) {
        (Some((repo, n)), _) => cur
            .iter()
            .find(|r| r.repo == repo && r.issue == n)
            .copied()
            .ok_or_else(|| {
                Refusal::new(
                    "interrupt-park-not-in-progress",
                    format!("--park {repo}#{n} is not a ticket of yours that is In Progress; your In Progress tickets: {keys}"),
                )
            }),
        (None, [only]) => Ok(only),
        (None, _) => Err(Refusal::new(
            "interrupt-park-ambiguous",
            format!(
                "you have {} tickets In Progress: {keys}; name the one to park with `--park <n>` \
                 (and repair the others with `block`, `ask` or `done`)",
                cur.len()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::row;

    #[test]
    fn block_ask_or_done_of_the_only_in_progress_ticket_writes_idle() {
        // the ticket row still reads In Progress when the decision is made
        let rows = vec![
            row(1, "In Progress", Some("me@x"), None),
            row(2, "Ready", Some("me@x"), None),
        ];
        assert!(goes_idle("me@x", Some("me@x"), "PVTI_1", &rows));
    }

    #[test]
    fn another_in_progress_ticket_or_another_assignee_keeps_doing() {
        let rows = vec![
            row(1, "In Progress", Some("me@x"), None),
            row(3, "In Progress", Some("me@x"), None),
        ];
        assert!(!goes_idle("me@x", Some("me@x"), "PVTI_1", &rows));
        // an In Progress ticket of another agent does not count
        let others = vec![
            row(1, "In Progress", Some("me@x"), None),
            row(4, "In Progress", Some("you@x"), None),
        ];
        assert!(goes_idle("me@x", Some("me@x"), "PVTI_1", &others));
        // not the assignee (coordinator `ask`): nothing to write
        assert!(!goes_idle("me@x", Some("you@x"), "PVTI_1", &others));
        assert!(!goes_idle("me@x", None, "PVTI_1", &others));
    }

    #[test]
    fn busy_is_judged_by_the_board_and_not_by_the_state() {
        // the only ticket is Blocked -> no busy ticket, take may go on
        let stale = vec![
            row(1, "Blocked", Some("me@x"), None),
            row(2, "Ready", Some("me@x"), None),
        ];
        assert_eq!(in_progress_key(&stale, "me@x", "PVTI_2"), None);
        // real: an In Progress ticket exists -> refused, the key is named
        let busy = vec![
            row(1, "In Progress", Some("me@x"), None),
            row(2, "Ready", Some("me@x"), None),
        ];
        assert_eq!(
            in_progress_key(&busy, "me@x", "PVTI_2"),
            Some("a/b#1".to_string())
        );
        // another agent's In Progress ticket does not count
        let other = vec![row(1, "In Progress", Some("you@x"), None)];
        assert_eq!(in_progress_key(&other, "me@x", "PVTI_2"), None);
    }

    #[test]
    fn choose_park_needs_a_name_only_for_more_than_one_in_progress() {
        let ip = |n: u64| {
            let mut r = row(n, "In Progress", Some("me@x"), None);
            r.issue = n;
            r
        };
        let rows = vec![ip(1), ip(2), row(3, "Ready", Some("me@x"), None)];
        // refusal: two candidates, no name -> ambiguous, lists both
        let e = choose_park(&rows, None).unwrap_err();
        assert_eq!(e.rule, "interrupt-park-ambiguous");
        assert!(
            e.message.contains("a/b#1")
                && e.message.contains("a/b#2")
                && e.message.contains("--park")
        );
        // pass: a name picks the ticket
        assert_eq!(choose_park(&rows, Some(("a/b", 2))).unwrap().issue, 2);
        // refusal: a Ready ticket is no candidate
        assert_eq!(
            choose_park(&rows, Some(("a/b", 3))).unwrap_err().rule,
            "interrupt-park-not-in-progress"
        );
        // one candidate: no name needed
        assert_eq!(choose_park(&rows[..1], None).unwrap().issue, 1);
        // none: its own refusal, with or without a name
        let none = vec![row(3, "Ready", Some("me@x"), None)];
        for named in [None, Some(("a/b", 3))] {
            assert_eq!(
                choose_park(&none, named).unwrap_err().rule,
                "interrupt-nothing-to-park"
            );
        }
    }
}
