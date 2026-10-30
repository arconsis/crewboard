//! Queue order per agent: `assign --order`, and the moves of a queued ticket that `assign` makes
//! (a new position, another agent).
//!
//! The board field `Order` (number) is the position of a Ready ticket in the queue of its
//! agent. The next ticket of an agent is its lowest `Order` among Ready (ties: lower issue
//! number; see `mine::select_mine`). Only the coordinator (and the owner) writes it.

use super::assign_checks::{
    CapOverride, check_assignable, check_assignable_state, check_capabilities, post_assignment,
};
use super::{agent_label_lines, load_world_cached, me_of, remember, work_write_ctx, write_work};
use crate::base::ids;
use crate::board::FieldValue;
use crate::error::{CbError, CbResult};
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::names::AgentName;
use crate::rules::Refusal;
use crate::rules::Role;
use crate::rules::fields::{Field, Item, WriteCtx, check_field_write};
use crate::rules::status::Status;
use crate::services::ticket_index::read_ticket_rows;
use crate::services::ticket_loader::{Ticket, load_ticket};
use crate::ticket::{TicketRow, body_field};

/// The largest `Order` that is accepted.
pub const MAX_ORDER: i64 = 1_000_000;

/// `--order`: a whole number from 0 to `MAX_ORDER`. Pure, checked before any request.
pub fn parse_order(s: &str) -> Result<i64, Refusal> {
    match s.trim().parse::<i64>() {
        Ok(n) if (0..=MAX_ORDER).contains(&n) => Ok(n),
        _ => Err(Refusal::new(
            ids::ORDER_INVALID,
            format!("order '{s}' is not a whole number from 0 to {MAX_ORDER} (lowest goes first)"),
        )),
    }
}

/// The `Order` after the last ticket in the Ready queue of `agent`: the highest order there
/// plus one, or 1 for an empty queue.
pub fn next_order(rows: &[TicketRow], agent: &str) -> i64 {
    rows.iter()
        .filter(|r| r.is_agent(agent) && r.in_status(Status::Ready))
        .filter_map(|r| r.order)
        .max()
        .map_or(1, |m| m + 1)
}

/// The `Order` at the front of the Ready queue of `agent`: one below the lowest
/// `Order` of its Ready tickets (negative when that is 0); 0 for a queue without an ordered
/// ticket. Only Ready tickets of that agent count.
pub fn queue_front_order(rows: &[TicketRow], agent: &str) -> i64 {
    crate::rules::status::front_order(
        rows.iter()
            .filter(|r| r.is_agent(agent) && r.in_status(Status::Ready))
            .filter_map(|r| r.order)
            .min(),
    )
}

/// The pure checks of `rebalance`: the ticket is Ready, it has an agent, the target is
/// another agent, and the target is a dev agent (State `active`).
pub fn check_rebalance(
    key: &str,
    status: Option<&str>,
    current: Option<&str>,
    to: &str,
    to_role: Option<Role>,
    to_state: &str,
) -> Result<(), CbError> {
    if !Status::Ready.is(status) {
        return Err(CbError::refused(
            ids::REBALANCE_NOT_READY,
            format!(
                "{key} is {}, not Ready: only a queued ticket moves between agents (use `reassign` for other states)",
                status.unwrap_or("without a Status")
            ),
        ));
    }
    let Some(cur) = current else {
        return Err(CbError::refused(
            ids::REBALANCE_NO_AGENT,
            format!("{key} has no agent; use `assign` or `reassign --to`"),
        ));
    };
    if cur == to {
        return Err(CbError::refused(
            ids::REBALANCE_SAME_AGENT,
            format!(
                "{key} is queued for {to} already; use `assign --order` to change its position"
            ),
        ));
    }
    check_assignable_state(to, to_role, to_state)
}

/// The position of a queued ticket: only a Ready ticket that has an agent has one.
fn check_queued(t: &Ticket) -> CbResult<()> {
    let status = t.item.get(Field::Status);
    if !Status::Ready.is(status) || t.agent().is_none() {
        return Err(CbError::refused(
            ids::REORDER_NOT_QUEUED,
            format!(
                "{} is {} with agent {}: only a Ready ticket that has an agent has a queue position",
                t.key(),
                status.unwrap_or("without a Status"),
                t.agent().unwrap_or("-")
            ),
        ));
    }
    Ok(())
}

/// Sets the queue position of a queued ticket (`assign <n> --order N`). No message is posted.
pub(super) fn set_position(
    ac: &mut AgentCtx,
    t: &Ticket,
    order: i64,
    who: &WriteCtx,
) -> CbResult<String> {
    check_queued(t)?;
    write_order(ac, &t.item_id, order, who)?;
    Ok(format!(
        "{} queue position of {}: Order {order}\n",
        t.key(),
        t.agent().unwrap_or("?")
    ))
}

/// `assign <issue> --order <order>` (coordinator) without `--to`.
pub(super) fn reorder(ac: &mut AgentCtx, issue_arg: &str, order: &str) -> CbResult<String> {
    let order = parse_order(order)?;
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let who = work_write_ctx(&me, false);
    check_field_write(Item::Work, Field::Order, &who)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &tref)?;
    set_position(ac, &t, order, &who)
}

/// What a move of a queued ticket to another agent asks for.
pub(super) struct Move<'a> {
    pub to: &'a AgentName,
    /// The position in the new queue; `None` appends to it.
    pub order: Option<i64>,
    pub ov: CapOverride<'a>,
}

/// Moves a Ready ticket to the queue of another agent: Agent field, agent label, Order and ONE
/// signed message to the new agent. The target may be `working` (it takes the ticket after its
/// current one).
pub(super) fn move_queued(
    ac: &mut AgentCtx,
    world: &World,
    t: &Ticket,
    mv: &Move<'_>,
    who: &WriteCtx,
) -> CbResult<String> {
    let to_name = mv.to;
    let target = world.trust.get(to_name.as_str());
    check_rebalance(
        &t.key(),
        t.item.get(Field::Status),
        t.agent(),
        to_name.as_str(),
        target.map(|x| x.role),
        target
            .and_then(|x| world.items.get(&x.item_id))
            .and_then(|i| i.get(Field::State))
            .unwrap_or("?"),
    )?;
    // approved, alive, dev, active or working
    check_assignable(ac, world, to_name.as_str())?;
    let (override_lines, cap_out) = check_capabilities(world, &t.info, to_name.as_str(), mv.ov)?;
    let old = t.agent().map(str::to_string);
    let order = match mv.order {
        Some(o) => o,
        None => {
            let mut cache = ac.load_cache();
            let rows = read_ticket_rows(&ac.ctx.reader(), &mut cache.index.tickets)?;
            ac.save_cache(&mut cache);
            next_order(&rows, to_name.as_str())
        }
    };
    let option_lines =
        crate::ops::approve::ensure_agent_option(&mut ac.ctx, world, to_name.as_str())?.text();
    write_work(
        ac,
        &t.item_id,
        Field::Agent,
        FieldValue::Option(to_name.as_str()),
        who,
    )?;
    write_order(ac, &t.item_id, order, who)?;
    // The signed log takes the assignee only from an assignment message: post that one.
    let posted = post_assignment(
        ac,
        t,
        to_name,
        body_field(&t.info.body, "Gate"),
        &format!(
            "order: {order}\nmoved-from: {}\n{override_lines}",
            old.as_deref().unwrap_or("-")
        ),
    )?;
    remember(ac, t, Status::Ready, Some(to_name.as_str()));
    let mut out = format!(
        "{} moved from {} to {to_name}; Status Ready; Order {order}\n{cap_out}{option_lines}",
        t.key(),
        old.as_deref().unwrap_or("-")
    );
    out.push_str(&agent_label_lines(
        ac,
        &t.repo,
        t.issue,
        &t.info.labels,
        Some(to_name.as_str()),
    ));
    out.push_str(&format!(
        "assignment message {} ({})\n",
        posted.id, posted.url
    ));
    Ok(out)
}

/// Write `Order`. The field is created on first use, on a board that `init` did not make.
pub(super) fn write_order(
    ac: &mut AgentCtx,
    item_id: &str,
    order: i64,
    who: &WriteCtx,
) -> CbResult<()> {
    ac.ctx
        .board
        .ensure_number_field(&ac.ctx.client, Field::Order)?;
    write_work(ac, item_id, Field::Order, FieldValue::Number(order), who)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::row;

    #[test]
    fn an_order_is_a_whole_number_from_0_to_the_limit() {
        for ok in ["0", "1", " 42 ", "1000000"] {
            assert!(parse_order(ok).is_ok(), "{ok}");
        }
        assert_eq!(parse_order(" 7 ").unwrap(), 7);
        for bad in ["", "-1", "1.5", "x", "1000001", "1e3", "P1"] {
            assert_eq!(parse_order(bad).unwrap_err().rule, "order-invalid", "{bad}");
        }
    }

    #[test]
    fn the_next_order_follows_the_last_ready_ticket_of_that_agent_only() {
        let rows = vec![
            row(1, "Ready", Some("a@x"), Some(3)),
            row(2, "Ready", Some("a@x"), Some(7)),
            row(3, "In Progress", Some("a@x"), Some(50)),
            row(4, "Ready", Some("b@x"), Some(99)),
            row(5, "Ready", Some("a@x"), None),
        ];
        assert_eq!(next_order(&rows, "a@x"), 8);
        assert_eq!(next_order(&rows, "b@x"), 100);
        // control: an empty queue starts at 1
        assert_eq!(next_order(&rows, "c@x"), 1);
    }

    #[test]
    fn the_front_of_the_queue_is_below_the_lowest_ready_order_of_that_agent() {
        // empty queue (also: other agents and other states do not count)
        let others = vec![
            row(3, "In Progress", Some("a@x"), Some(2)),
            row(4, "Ready", Some("b@x"), Some(9)),
        ];
        assert_eq!(queue_front_order(&[], "a@x"), 0);
        assert_eq!(queue_front_order(&others, "a@x"), 0);
        // lowest 5 -> 4
        let q = vec![
            row(1, "Ready", Some("a@x"), Some(7)),
            row(2, "Ready", Some("a@x"), Some(5)),
            row(5, "Ready", Some("a@x"), None),
        ];
        assert_eq!(queue_front_order(&q, "a@x"), 4);
        // lowest 0 -> -1: only a value below 0 is strictly first
        let z = vec![
            row(1, "Ready", Some("a@x"), Some(0)),
            row(2, "Ready", Some("a@x"), Some(3)),
        ];
        assert_eq!(queue_front_order(&z, "a@x"), -1);
        // a queue of unordered tickets is also front at 0
        assert_eq!(
            queue_front_order(&[row(6, "Ready", Some("a@x"), None)], "a@x"),
            0
        );
    }

    /// The queue as `mine`, `resume` and `status --queue` list it (`select_mine`): issue numbers.
    fn listed(rows: &[TicketRow], agent: &str) -> Vec<u64> {
        crate::ops::mine::select_mine(rows, agent)
            .iter()
            .map(|r| r.issue)
            .collect()
    }

    /// Ticket 9 is not listed while it is Blocked; a requeue makes it Ready with the front `Order`.
    fn after_requeue(mut rows: Vec<TicketRow>, agent: &str) -> Vec<TicketRow> {
        let front = queue_front_order(&rows, agent);
        let mut t = row(9, "Blocked", Some(agent), Some(50));
        assert!(!listed(&[t.clone()], agent).contains(&9));
        t.status = Some("Ready".into());
        t.order = Some(front);
        rows.push(t);
        rows
    }

    #[test]
    fn mine_lists_the_requeued_ticket_first() {
        let q = vec![
            row(1, "Ready", Some("a@x"), Some(5)),
            row(2, "Ready", Some("a@x"), Some(1)),
        ];
        let got = listed(&after_requeue(q.clone(), "a@x"), "a@x");
        assert_eq!(got, [9, 2, 1]);
        // control: without the front value (the old Order 50) it comes last
        let mut late = q;
        late.push(row(9, "Ready", Some("a@x"), Some(50)));
        assert_eq!(listed(&late, "a@x"), [2, 1, 9]);
    }

    #[test]
    fn mine_lists_the_requeued_ticket_first_also_when_the_lowest_order_is_zero() {
        let p0 = row(2, "Ready", Some("a@x"), Some(0));
        // an equal Order 0 would lose to the lower issue number; the front value is -1
        let got = listed(&after_requeue(vec![p0.clone()], "a@x"), "a@x");
        assert_eq!(got, [9, 2]);
        let tie = row(9, "Ready", Some("a@x"), Some(0));
        assert_eq!(listed(&[p0, tie], "a@x"), [2, 9]);
    }

    #[test]
    fn mine_lists_the_requeued_ticket_first_in_an_unordered_or_empty_queue() {
        let unordered = vec![row(1, "Ready", Some("a@x"), None)];
        assert_eq!(listed(&after_requeue(unordered, "a@x"), "a@x"), [9, 1]);
        assert_eq!(listed(&after_requeue(vec![], "a@x"), "a@x"), [9]);
    }

    #[test]
    fn the_requeue_leaves_the_in_progress_ticket_of_the_agent_alone() {
        let ip = row(3, "In Progress", Some("a@x"), Some(0));
        let other = row(4, "Ready", Some("b@x"), Some(0));
        let before = vec![
            ip.clone(),
            other.clone(),
            row(1, "Ready", Some("a@x"), Some(4)),
        ];
        // the In Progress Order (0) and the other agent's Order do not move the front
        assert_eq!(queue_front_order(&before, "a@x"), 3);
        let after = after_requeue(before.clone(), "a@x");
        // the ticket stays In Progress, unchanged, and still comes first in `mine`
        let kept = after.iter().find(|r| r.issue == 3).expect("kept");
        assert_eq!(kept.status_str(), Some("In Progress"));
        assert_eq!(kept.order, ip.order);
        assert_eq!(kept.agent, ip.agent);
        assert_eq!(listed(&after, "a@x"), [3, 9, 1]);
        // the requeued ticket is Ready, never In Progress: one In Progress ticket remains
        let in_progress = after
            .iter()
            .filter(|r| r.agent.as_deref() == Some("a@x"))
            .filter(|r| r.status_str() == Some("In Progress"))
            .count();
        assert_eq!(in_progress, 1);
        // the other agent is untouched
        assert_eq!(listed(&after, "b@x"), [4]);
    }

    #[test]
    fn rebalance_refuses_a_ticket_that_is_not_ready_and_a_target_that_is_not_a_dev() {
        let ok =
            |status, cur, to, role, state| check_rebalance("a/b#1", status, cur, to, role, state);
        // the good case passes, also for a working target
        assert!(ok(Some("Ready"), Some("a@x"), "b@x", Some(Role::Dev), "active").is_ok());
        assert!(
            ok(
                Some("Ready"),
                Some("a@x"),
                "b@x",
                Some(Role::Dev),
                "working"
            )
            .is_ok()
        );
        // not Ready
        for s in [
            Some("In Progress"),
            Some("Review"),
            Some("Blocked"),
            Some("Done"),
            Some("Backlog"),
            None,
        ] {
            let e = ok(s, Some("a@x"), "b@x", Some(Role::Dev), "active").unwrap_err();
            assert!(e.to_string().contains("rebalance-not-ready"), "{s:?}: {e}");
        }
        // target is the coordinator, unknown, paused or left
        let e = ok(
            Some("Ready"),
            Some("a@x"),
            "c@x",
            Some(Role::Coordinator),
            "active",
        )
        .unwrap_err();
        assert!(e.to_string().contains("status-agent-not-active-dev"), "{e}");
        let e = ok(Some("Ready"), Some("a@x"), "z@x", None, "?").unwrap_err();
        assert!(e.to_string().contains("status-agent-not-active-dev"), "{e}");
        for st in ["paused", "draining", "left"] {
            let e = ok(Some("Ready"), Some("a@x"), "b@x", Some(Role::Dev), st).unwrap_err();
            assert!(
                e.to_string().contains("status-agent-not-active-dev"),
                "{st}: {e}"
            );
        }
        // no agent, same agent
        let e = ok(Some("Ready"), None, "b@x", Some(Role::Dev), "active").unwrap_err();
        assert!(e.to_string().contains("rebalance-no-agent"), "{e}");
        let e = ok(Some("Ready"), Some("b@x"), "b@x", Some(Role::Dev), "active").unwrap_err();
        assert!(e.to_string().contains("rebalance-same-agent"), "{e}");
    }

    #[test]
    fn only_the_coordinator_or_the_owner_may_write_the_order() {
        use crate::rules::{Actor, Role};
        let ctx = |actor| WriteCtx {
            is_assigned: true,
            approved: true,
            ..WriteCtx::by(actor)
        };
        assert!(
            check_field_write(
                Item::Work,
                Field::Order,
                &ctx(Actor::Agent(Role::Coordinator))
            )
            .is_ok()
        );
        assert!(check_field_write(Item::Work, Field::Order, &ctx(Actor::Owner)).is_ok());
        // control: the assigned dev agent may not reorder its own queue
        let e =
            check_field_write(Item::Work, Field::Order, &ctx(Actor::Agent(Role::Dev))).unwrap_err();
        assert_eq!(e.rule, "field-wrong-writer");
    }
}
