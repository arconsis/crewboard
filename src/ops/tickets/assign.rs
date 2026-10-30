//! `assign`: the coordinator puts a ticket in the queue of an agent (Agent field, order, signed
//! message). On a ticket that is queued already, `assign` moves it: `--order` sets the position,
//! `--to <other agent>` moves it to that agent.

use super::assign_checks::{CapOverride, check_assignable, check_capabilities, post_assignment};
use super::board_put::put_on_board;
use super::order;
use super::order::parse_order;
use super::{
    Me, agent_label_lines, load_world_cached, me_of, remember, work_write_ctx, write_work,
};
use crate::base::ids;
use crate::board::FieldValue;
use crate::error::{CbError, CbResult};
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::names::AgentName;
use crate::rules::Refusal;
use crate::rules::fields::{Field, Item, check_field_write};
use crate::rules::limits::check_text_field;
use crate::rules::status::{Status, Transition, check_status_transition};
use crate::services::ticket_loader::{
    IssueInfo, Ticket, check_repo, check_work_issue, fetch_issue, read_messages,
};
use crate::ticket::{body_field, find_item_id};

// ---------------------------------------------------------------------------------------

/// The pure check of the arguments of `assign`, before any request. `--to` may be left out only
/// to set the position of a queued ticket, so `--order` must be there and nothing else that
/// needs an agent.
pub fn check_assign_args(
    to: Option<&str>,
    order: Option<&str>,
    ov: CapOverride<'_>,
) -> Result<(), Refusal> {
    if to.is_some() {
        return Ok(());
    }
    if order.is_none() || ov.flag {
        return Err(Refusal::new(
            ids::ASSIGN_NEEDS_TO,
            "assign needs --to <agent>; only `assign <n> --order <number>` may leave it out (it sets the queue position of a queued ticket)",
        ));
    }
    Ok(())
}

/// A Ready ticket that `assign` may still assign: it has no Agent, or its Agent field was set
/// by hand and no signed message for the item exists (board fields are not signed). With a
/// signed message, an Agent field counts as assigned.
fn ready_without_assignment(status: Status, agent: Option<&str>, signed: bool) -> bool {
    status == Status::Ready && (agent.is_none() || !signed)
}

/// What the coordinator asks for in `assign`, after the arguments are parsed.
struct Assignment<'a> {
    to: &'a AgentName,
    order: i64,
    /// The `Gate:` line of the issue.
    gate: &'a str,
    /// The ticket is Ready already and only lacks the Agent.
    ready_unassigned: bool,
    /// The Status that `assign` read; the Status write checks it again.
    from: Option<Status>,
    key: &'a str,
}

/// The issue of a ticket to assign: a work issue that is open and has a `Gate:` line. Returns
/// the issue and the gate.
fn read_assignable_issue(ac: &AgentCtx, repo: &str, n: u64) -> CbResult<(IssueInfo, String)> {
    let info = fetch_issue(&ac.ctx.client, repo, n)?;
    check_work_issue(&info, repo)?;
    if !info.open {
        return Err(CbError::refused(
            ids::TICKET_CLOSED,
            format!("{repo}#{n} is closed"),
        ));
    }
    let gate = body_field(&info.body, "Gate").ok_or_else(|| {
        CbError::refused(
            ids::STATUS_GATE_MISSING,
            format!(
                "{repo}#{n} has no 'Gate:' line in its body (docs/design/spec/tickets.md#ticket-content)"
            ),
        )
    })?;
    check_text_field(&gate)?;
    Ok((info, gate))
}

/// The ticket as a `Ticket` with its board item: it is put on the board if it is not there, and
/// gets the Status Backlog if it has none. Returns the ticket, its status, and whether a signed
/// info comment named the item.
fn ticket_on_board(
    ac: &mut AgentCtx,
    world: &World,
    me: &Me,
    info: IssueInfo,
    (repo, n): (&str, u64),
    out: &mut String,
) -> CbResult<(Ticket, Status, bool)> {
    let (comments, msgs) = read_messages(&ac.ctx.reader(), &world.trust, repo, n)?;
    // The signed info comment is the trust anchor (spec/board#item-ids-are-the-source-of-truth);
    // board fields are not signed.
    let signed = find_item_id(&msgs);
    let item_id = match &signed {
        Some(id) => id.clone(),
        None => put_on_board(ac, me, &info, repo, None, out)?,
    };
    let item = ac.ctx.board.read_item(&ac.ctx.client, &item_id)?;
    let t = Ticket {
        repo: repo.to_string(),
        issue: n,
        item_id: item_id.clone(),
        item,
        info,
        comments,
        msgs,
    };
    let status = match t.item.get(Field::Status) {
        None => {
            check_status_transition(None, Status::Backlog, me.actor, &Transition::Create)?;
            ac.ctx
                .board
                .write_status(&ac.ctx.client, &item_id, Status::Backlog)?;
            out.push_str("Status set to Backlog\n");
            Status::Backlog
        }
        Some(_) => t.status()?,
    };
    Ok((t, status, signed.is_some()))
}

/// The ticket is Ready and a signed assignment names its agent: it sits in a queue. An Agent
/// that was set by hand and has no signed message is not queued; `assign` repairs it.
fn is_queued(t: &Ticket, status: Status, signed: bool) -> bool {
    let signed = signed && t.check_agent_signed().is_ok();
    status == Status::Ready && !ready_without_assignment(status, t.agent(), signed)
}

/// Judges the move to Ready. Backlog -> Ready is the status move; Ready without an Agent (after
/// a reassign) only needs the Agent. Returns true for the second case. A queued ticket never
/// gets here: `assign` moves it ([`is_queued`]).
fn check_move_to_ready(
    t: &Ticket,
    me: &Me,
    status: Status,
    signed: bool,
    out: &mut String,
) -> CbResult<bool> {
    // `signed`: a signed assignment names the board Agent (an Agent set by hand, or one that the
    // signed log does not name, is repaired by the assignment that follows)
    let signed = signed && t.check_agent_signed().is_ok();
    let ready_unassigned = ready_without_assignment(status, t.agent(), signed);
    if status == Status::Ready && t.agent().is_some() {
        out.push_str(&format!(
            "repaired an unsigned assignment: the Agent field was set, but {} has no signed assignment message; posting it now\n",
            t.key()
        ));
    }
    if !ready_unassigned {
        let release = Transition::Release {
            gate_set: true,
            assignee_active_dev: true,
        };
        check_status_transition(Some(status), Status::Ready, me.actor, &release)?;
    }
    Ok(ready_unassigned)
}

/// The board writes of `assign`: the agent option, Order, Agent, Gate and, for a
/// Backlog ticket, Status Ready. Every field write is checked first.
fn write_assignment(
    ac: &mut AgentCtx,
    world: &World,
    me: &Me,
    item_id: &str,
    a: &Assignment<'_>,
    out: &mut String,
) -> CbResult<()> {
    // The guard runs before the first write (the option, Order, Agent, Gate).
    if let Some(from) = a.from {
        let wanted = (!a.ready_unassigned).then_some(Status::Ready);
        super::check_status_unchanged(ac, item_id, a.key, from, wanted)?;
    }
    out.push_str(
        &crate::ops::approve::ensure_agent_option(&mut ac.ctx, world, a.to.as_str())?.text(),
    );
    let wc = work_write_ctx(me, false);
    check_field_write(Item::Work, Field::Agent, &wc)?;
    check_field_write(Item::Work, Field::Gate, &wc)?;
    check_field_write(Item::Work, Field::Order, &wc)?;
    // the field is created on first use, on a board that init did not make
    order::write_order(ac, item_id, a.order, &wc)?;
    write_work(
        ac,
        item_id,
        Field::Agent,
        FieldValue::Option(a.to.as_str()),
        &wc,
    )?;
    write_work(ac, item_id, Field::Gate, FieldValue::Text(a.gate), &wc)?;
    if !a.ready_unassigned {
        super::set_select_status(ac, item_id, Status::Ready)?;
    }
    Ok(())
}

/// What `assign` does with a ticket that is queued already (Ready, with a signed assignment).
struct Queued<'a> {
    to: &'a AgentName,
    ov: CapOverride<'a>,
    order: Option<i64>,
}

/// A queued ticket: `--to` its own agent with `--order` sets the position; `--to` another agent
/// moves it to the queue of that agent (Agent, label, Order, one signed message). `--to` its own
/// agent without `--order` has nothing to do and is refused.
fn assign_queued(
    ac: &mut AgentCtx,
    world: &World,
    me: &Me,
    t: &Ticket,
    q: &Queued<'_>,
) -> CbResult<String> {
    let wc = work_write_ctx(me, false);
    check_field_write(Item::Work, Field::Order, &wc)?;
    let mut out = String::new();
    if t.agent() == Some(q.to.as_str()) {
        let Some(o) = q.order else {
            return Err(CbError::refused(
                ids::TICKET_ALREADY_ASSIGNED,
                format!(
                    "{} is Ready and assigned to {}; use `assign --order <n>` to change its position, or `assign --to <other agent>` to move it",
                    t.key(),
                    q.to
                ),
            ));
        };
        out.push_str(&order::set_position(ac, t, o, &wc)?);
    } else {
        let mv = order::Move {
            to: q.to,
            order: q.order,
            ov: q.ov,
        };
        out.push_str(&order::move_queued(ac, world, t, &mv, &wc)?);
    }
    Ok(out)
}

/// The `Order` that puts a new ticket at the end of the queue of `agent`. Reads the open
/// tickets of the agent.
fn end_of_queue(ac: &AgentCtx, world: &World, agent: &str) -> CbResult<i64> {
    let rows = crate::ops::lifecycle::rows_of(ac, world, agent)?;
    Ok(crate::ticket::append_order(&rows, agent))
}

/// `assign <issue> --to <agent> [--order N]` (coordinator). A busy dev agent gets the
/// ticket too: it waits in Ready with the Agent set (the queue). On a queued
/// ticket it moves the ticket: `assign <issue> --order N` (no `--to`) sets the position, and
/// `--to <other agent>` moves it to that agent.
pub fn assign(
    ac: &mut AgentCtx,
    issue_arg: &str,
    to: Option<&str>,
    ov: CapOverride<'_>,
    order: Option<&str>,
) -> CbResult<String> {
    crate::capabilities::check_override_args(ov.flag, ov.reason)?;
    check_assign_args(to, order, ov)?;
    let order_text = order.unwrap_or_default();
    let order = order.map(parse_order).transpose()?;
    let Some(to) = to else {
        // check_assign_args: without --to there is --order and nothing else
        return order::reorder(ac, issue_arg, order_text);
    };
    let to_name = AgentName::parse(to)?;
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    check_repo(&ac.ctx.cfg, &tref.repo)?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    // Writer first, before anything is added to the board.
    check_status_transition(
        Some(Status::Backlog),
        Status::Ready,
        me.actor,
        &Transition::Release {
            gate_set: true,
            assignee_active_dev: true,
        },
    )?;
    let (info, gate) = read_assignable_issue(ac, &tref.repo, tref.issue)?;
    check_assignable(ac, &world, to_name.as_str())?;
    // spec/profiles#requirements-and-matching: before anything is written to the board
    let (override_lines, cap_out) = check_capabilities(&world, &info, to_name.as_str(), ov)?;
    // spec/tickets#workflow-files-the-owner-route: a ticket for workflow files goes the owner
    // route; the dev agent must know.
    let (route_lines, route_out) = crate::owner_route::assign_lines(&info.body, tref.issue);
    let override_lines = format!("{override_lines}{route_lines}");
    let mut out = format!("{cap_out}{route_out}");

    let (t, status, signed) =
        ticket_on_board(ac, &world, &me, info, (&tref.repo, tref.issue), &mut out)?;
    if is_queued(&t, status, signed) {
        let q = Queued {
            to: &to_name,
            ov,
            order,
        };
        return assign_queued(ac, &world, &me, &t, &q);
    }
    let ready_unassigned = check_move_to_ready(&t, &me, status, signed, &mut out)?;
    // a repair keeps the position that the ticket has; any other assign goes to the end
    let kept = t
        .item
        .get(Field::Order)
        .filter(|_| ready_unassigned)
        .and_then(|o| o.parse::<i64>().ok());
    let (order, order_is_default) = match order.or(kept) {
        Some(o) => (o, false),
        None => (end_of_queue(ac, &world, to_name.as_str())?, true),
    };
    let a = Assignment {
        to: &to_name,
        order,
        gate: &gate,
        ready_unassigned,
        from: Some(status),
        key: &t.key(),
    };
    write_assignment(ac, &world, &me, &t.item_id, &a, &mut out)?;
    let posted = post_assignment(
        ac,
        &t,
        a.to,
        Some(a.gate.to_string()),
        &format!("order: {}\n{override_lines}", a.order),
    )?;
    remember(ac, &t, Status::Ready, Some(to_name.as_str()));
    out.push_str(&agent_label_lines(
        ac,
        &tref.repo,
        tref.issue,
        &t.info.labels,
        Some(to_name.as_str()),
    ));
    let note = if order_is_default {
        " (the end of the queue)"
    } else {
        ""
    };
    out.push_str(&format!("Order set to {order}{note}\n"));
    out.push_str(&format!(
        "{} assigned to {to_name}; Status Ready; Gate field set from the body\nassignment message {} ({})\n",
        t.key(),
        posted.id,
        posted.url
    ));
    // The work that an earlier agent left (spec/tickets#work-that-an-earlier-agent-left).
    out.push_str(&super::earlier_work::hint(ac, &t, to_name.as_str(), false));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_field_without_a_signed_message_is_not_assigned() {
        // (a) Agent set by hand, no signed message: assign may go on and post
        assert!(ready_without_assignment(Status::Ready, Some("a@x"), false));
        // no Agent at all stays assignable, signed or not
        assert!(ready_without_assignment(Status::Ready, None, true));
        assert!(ready_without_assignment(Status::Ready, None, false));
        // (b) Agent set and a signed message exists: still ticket-already-assigned
        assert!(!ready_without_assignment(Status::Ready, Some("a@x"), true));
        // not Ready: the question does not arise here
        assert!(!ready_without_assignment(
            Status::Backlog,
            Some("a@x"),
            false
        ));
    }

    #[test]
    fn without_to_only_an_order_is_allowed() {
        let none = CapOverride::default();
        // pass: --to given, whatever else; or --order alone (the position of a queued ticket)
        assert!(check_assign_args(Some("a@x"), None, none).is_ok());
        assert!(check_assign_args(Some("a@x"), Some("3"), none).is_ok());
        assert!(check_assign_args(None, Some("3"), none).is_ok());
        // refusal: nothing to do, or an option that needs the agent
        for (order, flag) in [(None, false), (Some("3"), true)] {
            let ov = CapOverride { flag, reason: None };
            let e = check_assign_args(None, order, ov).unwrap_err();
            assert_eq!(e.rule, "assign-needs-to", "{order:?} {flag}");
        }
    }
}
