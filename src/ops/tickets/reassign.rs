//! `reassign`: the coordinator moves a ticket to another agent, or the duties loop reassigns it.

use super::assign_checks::{CapOverride, check_assignable, check_capabilities};
use super::{
    Me, agent_label_lines, load_world_cached, me_of, post, remember, sync_needs_owner, to_agent,
    work_write_ctx, work_write_ctx_of, write_work,
};
use crate::base::ids;
use crate::board::FieldValue;
use crate::clock::now_ts;
use crate::error::{CbError, CbResult};
use crate::ops::say::send_signed;
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::names::{AgentName, Kind, Re, To};
use crate::rules::fields::{Field, Item, check_field_write};
use crate::rules::reassign::{Reason, target_status};
use crate::rules::status::{Status, Transition, check_status_transition};
use crate::rules::{Actor, Refusal, Role};
use crate::services::ticket_loader::{Ticket, fetch_issue, load_ticket};
use crate::ticket::question_open;

/// How a reassign is justified (spec/tickets#status-flow-and-writers): the coordinator states a
/// typed reason with an optional free-text detail, or the board shows that the agent is dead or
/// stopping (`--derived`, and the coordinator's own loop).
#[derive(Debug, Clone, Copy)]
pub enum Why<'a> {
    Stated {
        reason: Reason,
        detail: Option<&'a str>,
    },
    Derived,
}

/// `reassign <issue> (--reason <word> [--why text] | --derived) [--to agent]` (coordinator):
/// with `--to` the ticket goes to Ready for that agent, without it to Backlog (a ticket in
/// Ready always has an agent).
pub fn reassign(
    ac: &mut AgentCtx,
    issue_arg: &str,
    why: Why<'_>,
    to: Option<&str>,
    override_capabilities: bool,
) -> CbResult<String> {
    // The reason of the reassign is the reason of the override
    // (spec/profiles#requirements-and-matching).
    let stated: Option<String> = match why {
        Why::Stated { reason, detail } => Some(reason.text(detail)),
        Why::Derived => None,
    };
    let ov = CapOverride {
        flag: override_capabilities,
        reason: stated.as_deref(),
    };
    crate::capabilities::check_override_args(ov.flag, ov.reason)?;
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let new_agent = to.map(AgentName::parse).transpose()?;
    if let Some(a) = &new_agent {
        check_assignable(ac, &world, a.as_str())?;
    }
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &tref)?;
    let (override_lines, cap_out) = match &new_agent {
        Some(a) => check_capabilities(&world, &t.info, a.as_str(), ov)?,
        None => (String::new(), String::new()),
    };
    let from = t.status()?;
    let old = t.agent().map(str::to_string);
    let reason = reassign_reason(ac, &world, &t, why, stated, old.as_deref())?;
    let justified = !reason.is_empty();
    let reassign = Transition::Reassign { justified };
    // A ticket in Ready has an agent: with `--to` the ticket goes to Ready, without it to Backlog.
    let target = target_status(new_agent.is_some());
    if from == Status::Ready && target == Status::Ready {
        // Within Ready only the Agent changes; the writer is still the coordinator.
        require_coordinator(me.actor)?;
        if !justified {
            return Err(
                Refusal::new(ids::STATUS_REASSIGN_REASON, "reassign needs a reason").into(),
            );
        }
    } else {
        check_status_transition(Some(from), target, me.actor, &reassign)?;
    }
    let agent_writer = work_write_ctx(&me, false);
    check_field_write(Item::Work, Field::Agent, &agent_writer)?;
    // The guard runs before the first write (the withdraw, the option, the Agent field).
    super::check_status_unchanged(ac, &t.item_id, &t.key(), from, Some(target))?;
    // A reassign takes an open question back: the answer withdraws it (the new agent asks again
    // if needed), and the generic path below moves the ticket to Ready.
    let withdrawn = old.is_some() && question_open(&t.msgs, t.agent()).is_some();
    let withdraw_lines = match (&old, withdrawn) {
        (Some(agent), true) => withdraw_question(ac, &t, &me, agent, &reason)?,
        _ => String::new(),
    };
    // The order of the writes: the signed notice to the old agent, the agent option, then Status,
    // then Agent. A failure after any step leaves a ticket that a second run of the command (or
    // `assign`) finishes: still with the old agent, or in Backlog or Ready with the old agent set.
    // Agent first would leave an In Progress ticket without an agent.
    let notice_lines = notice_old_agent(ac, &t, old.as_deref(), new_agent.as_ref(), &reason)?;
    let mut option_lines = String::new();
    if let Some(a) = &new_agent {
        option_lines =
            crate::ops::approve::ensure_agent_option(&mut ac.ctx, &world, a.as_str())?.text();
    }
    if from != target {
        super::set_select_status(ac, &t.item_id, target)?;
    }
    let agent_value = match &new_agent {
        Some(a) => FieldValue::Option(a.as_str()),
        None => FieldValue::Clear,
    };
    write_work(ac, &t.item_id, Field::Agent, agent_value, &agent_writer)?;
    let mut out = format!(
        "{} back to {}; Agent {}\nreason: {reason}\n{withdraw_lines}{cap_out}{option_lines}",
        t.key(),
        target.name(),
        new_agent
            .as_ref()
            .map_or("cleared".to_string(), |a| a.to_string())
    );
    if new_agent.is_none() {
        out.push_str(&format!(
            "no agent chosen, so the ticket waits in Backlog: crewboard assign {} --to <agent>\n",
            t.key()
        ));
    }
    out.push_str(&notice_lines);
    if let Some(a) = &new_agent {
        // Copy the signed gate forward: the new assignment message is the latest one, and the
        // headless judgement reads the latest signed gate.
        let p = post(
            ac,
            &t,
            to_agent(a.as_str())?,
            Kind::Request,
            Re::None,
            &crate::protocol::acts::assignment_body(
                &t.info.title,
                &t.key(),
                &t.item_id,
                crate::ticket::signed_gate(&t.msgs).as_deref(),
                crate::ticket::signed_ci_none(&t.msgs),
                &format!("reason: {reason}\n{override_lines}"),
            ),
        )?;
        out.push_str(&format!("assignment message {}\n", p.url));
    }
    remember(ac, &t, target, new_agent.as_ref().map(AgentName::as_str));
    out.push_str(&agent_label_lines(
        ac,
        &tref.repo,
        tref.issue,
        &t.info.labels,
        new_agent.as_ref().map(AgentName::as_str),
    ));
    out.push_str(&sync_needs_owner(
        ac,
        &t,
        !withdrawn && question_open(&t.msgs, t.agent()).is_some(),
    ));
    Ok(out)
}

/// The reason text of the reassign. A derived reason comes from the liveness of the old agent.
fn reassign_reason(
    ac: &AgentCtx,
    world: &World,
    t: &Ticket,
    why: Why<'_>,
    stated: Option<String>,
    old: Option<&str>,
) -> CbResult<String> {
    match why {
        Why::Stated { .. } => Ok(stated.unwrap_or_default()),
        Why::Derived => {
            let Some(a) = old else {
                return Err(CbError::refused(
                    ids::STATUS_REASSIGN_NOT_JUSTIFIED,
                    format!("{} has no agent, so there is nothing to derive", t.key()),
                ));
            };
            let (state, _, live) =
                crate::ctx::agent_liveness(world, a, crate::clock::now_epoch(), ac.ctx.hb_interval);
            let logged = world
                .reg
                .agents
                .get(a)
                .and_then(|e| e.departure)
                .map(crate::registry::Departure::reason);
            Ok(crate::late::liveness::derived_reassign_reason(
                a,
                logged,
                state.as_deref(),
                &live,
                ac.ctx.hb_interval,
            )?)
        }
    }
}

/// The signed notice to the old agent, with what it left on GitHub (spec/tickets#work-that-an-earlier-agent-left),
/// and the output lines about it. Empty when the agent stays the same or there was none.
fn notice_old_agent(
    ac: &AgentCtx,
    t: &Ticket,
    old: Option<&str>,
    new_agent: Option<&AgentName>,
    reason: &str,
) -> CbResult<String> {
    let Some(o) = old else {
        return Ok(String::new());
    };
    if Some(o) == new_agent.map(AgentName::as_str) {
        return Ok(String::new());
    }
    let earlier = super::earlier_work::find(ac, &t.repo, t.issue, o);
    let p = post(
        ac,
        t,
        to_agent(o)?,
        Kind::Info,
        Re::None,
        &crate::protocol::acts::reassigned_body_with(reason, earlier.as_ref()),
    )?;
    Ok(format!(
        "notice to {o}: {}\n{}",
        p.url,
        earlier_lines(earlier.as_ref(), &t.key(), new_agent.is_some())
    ))
}

/// The lines of the output of a reassign that name the work of the old agent: the open PR (and
/// who continues on it) or the branch. Pure.
fn earlier_lines(
    earlier: Option<&crate::protocol::acts::EarlierWork>,
    key: &str,
    has_new_agent: bool,
) -> String {
    use crate::protocol::acts::EarlierWork;
    let next = if has_new_agent {
        "the new agent continues on it with `crewboard take --continue-pr <url>`, or the PR is closed"
    } else {
        "`crewboard assign` names it to the next agent; close the PR if the work is dropped"
    };
    match earlier {
        Some(EarlierWork {
            pr: Some(url),
            branch,
        }) => format!("earlier PR of the old agent: {url} (branch {branch}) for {key}; {next}\n"),
        Some(EarlierWork { pr: None, branch }) => {
            format!("earlier branch of the old agent: {branch} for {key}; it has no open PR\n")
        }
        None => String::new(),
    }
}

/// The coordinator takes the open question of `agent` back: ONE signed `answer` to the agent with
/// the reason and the line `withdraws: question`, and ONE `info` to the owner whose `event=`
/// line names the coordinator. The signed `info` is the record of the take-back (`audit` lists
/// security findings only). Returns the output lines.
fn withdraw_question(
    ac: &AgentCtx,
    t: &Ticket,
    me: &Me,
    agent: &str,
    reason: &str,
) -> CbResult<String> {
    let answer = post(
        ac,
        t,
        to_agent(agent)?,
        Kind::Answer,
        Re::None,
        &crate::protocol::acts::withdraw_body(reason),
    )?;
    let info = post(
        ac,
        t,
        To::Owner,
        Kind::Info,
        Re::None,
        &format!(
            "question on #{} taken back by the coordinator: {reason}\nevent={}/question-withdrawn:{}",
            t.issue,
            t.key(),
            me.name
        ),
    )?;
    Ok(format!(
        "answer to {agent}: {}\ninfo to the owner: {}\n",
        answer.url, info.url
    ))
}

/// The coordinator's own reassign of a ticket whose agent is dead or stopping (`reason` comes
/// from [`crate::late::liveness::derived_reassign_reason`]): Agent cleared, Status `Backlog` (it
/// has no new agent, and a ticket in Ready always has one), a signed notice to the old agent.
/// The ticket is given as a board row, so no issue read is needed. The coordinator role is
/// checked by the caller's loop; the status table is checked here. The notice names the branch
/// and the open PR of the old agent, and so does the return value, for the report of the caller
/// (spec/tickets#work-that-an-earlier-agent-left).
pub fn reassign_derived(
    ac: &AgentCtx,
    row: &crate::ticket::TicketRow,
    reason: &str,
) -> CbResult<Option<crate::protocol::acts::EarlierWork>> {
    // The row comes from a listing that lags by minutes: the transition is judged on the Status
    // that the item has now, read by id. A ticket that another agent holds now is not touched.
    let item = ac.ctx.board.read_item(&ac.ctx.client, &row.item_id)?;
    if item.get(Field::Agent) != row.agent.as_deref() {
        return Ok(None);
    }
    let from = item
        .get(Field::Status)
        .and_then(Status::parse_board)
        .ok_or_else(|| {
            CbError::refused(
                ids::TICKET_STATUS_UNKNOWN,
                format!("{} has no known Status", row.key()),
            )
        })?;
    let coordinator = Actor::Agent(Role::Coordinator);
    let reassign = Transition::Reassign {
        justified: !reason.trim().is_empty(),
    };
    let target = target_status(false);
    // Backlog with the old agent still set is what an earlier run left when its last write
    // failed: the notice and the Status are done, only the Agent is left to clear.
    let finish_only = from == Status::Backlog;
    if !finish_only {
        check_status_transition(Some(from), target, coordinator, &reassign)?;
    }
    let agent_writer = work_write_ctx_of(coordinator, false);
    check_field_write(Item::Work, Field::Agent, &agent_writer)?;
    // The order of the writes: the signed notice, then Status, then Agent. A failure after any
    // step leaves a state that the next cycle repeats or finishes: the ticket still with its old
    // agent (the notice may repeat), or in Backlog with the old agent (only the clear is left).
    // Agent first would leave an In Progress ticket without an agent, which no duty finds.
    let mut earlier = None;
    if !finish_only && let Some(old) = row.agent.as_deref() {
        earlier = super::earlier_work::find(ac, &row.repo, row.issue, old);
        send_signed(
            ac,
            &row.repo,
            row.issue,
            to_agent(old)?,
            Kind::Info,
            Re::None,
            &crate::protocol::acts::reassigned_body_with(reason, earlier.as_ref()),
            &now_ts(),
        )?;
    }
    if !finish_only {
        super::set_select_status(ac, &row.item_id, target)?;
    }
    write_work(
        ac,
        &row.item_id,
        Field::Agent,
        FieldValue::Clear,
        &agent_writer,
    )?;
    // The ticket has no agent now: its agent label goes too (a help, so only a warning).
    match fetch_issue(&ac.ctx.client, &row.repo, row.issue) {
        Ok(info) => {
            let lines = agent_label_lines(ac, &row.repo, row.issue, &info.labels, None);
            if lines.starts_with("warning") {
                eprint!("crewboard: {lines}");
            }
        }
        Err(e) => eprintln!(
            "crewboard: warning: the agent label of {} was not updated: {e}",
            row.key()
        ),
    }
    Ok(earlier)
}

fn require_coordinator(actor: Actor) -> Result<(), Refusal> {
    if actor.is_coordinator() {
        Ok(())
    } else {
        Err(Refusal::new(
            ids::STATUS_WRONG_WRITER,
            "reassign is a coordinator action",
        ))
    }
}
