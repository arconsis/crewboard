//! The checks before an assignment: can the agent take work, and does it meet the needs of the
//! ticket, and the signed assignment message. Shared by `assign`, `new`, `order` and `reassign`.

use super::{post, to_agent};
use crate::ctx::{AgentCtx, World};
use crate::error::{CbError, CbResult};
use crate::protocol::names::{AgentName, Kind, Re};
use crate::rules::Role;
use crate::rules::fields::Field;
use crate::services::ticket_loader::{IssueInfo, Ticket};
use crate::ticket::declares_ci_none;

/// The pure part of [`check_assignable`]: role and State. `active` takes work at once, also
/// when it has a ticket In Progress: the new ticket waits in Ready with the Agent set, and the
/// agent takes it after its current ticket (the queue). The legacy State `working` of
/// an old board counts as `active`. A paused, draining or departed agent takes nothing new
/// (spec/board#lifecycle).
pub(crate) fn check_assignable_state(
    agent: &str,
    role: Option<Role>,
    state: &str,
) -> Result<(), CbError> {
    if role != Some(Role::Dev) {
        return Err(CbError::refused(
            "status-agent-not-active-dev",
            format!("{agent} is not a dev agent"),
        ));
    }
    if state != "active" && state != "working" {
        // `working` is a legacy value (never written any more): it reads as `active`.
        return Err(CbError::refused(
            "status-agent-not-active-dev",
            format!(
                "{agent} is in State '{state}', not active{}",
                if state == "paused" {
                    " (a paused agent finishes its current step and takes nothing new)"
                } else {
                    ""
                }
            ),
        ));
    }
    Ok(())
}

/// Check that `agent` can take work: approved and trusted, role dev, State `active` (or the
/// legacy `working`), and alive. A late or dead agent does not answer.
pub(super) fn check_assignable(ac: &AgentCtx, world: &World, agent: &str) -> CbResult<()> {
    let t = world.trust.get(agent).ok_or_else(|| {
        CbError::refused(
            "status-agent-not-active-dev",
            format!("{agent} is not an approved agent"),
        )
    })?;
    if t.role != Role::Dev {
        return check_assignable_state(agent, Some(t.role), "");
    }
    let (_, _, live) =
        crate::ctx::agent_liveness(world, agent, crate::clock::now_epoch(), ac.ctx.hb_interval);
    crate::late::liveness::check_alive(agent, &live)?;
    let state = world
        .items
        .get(&t.item_id)
        .and_then(|i| i.get(Field::State))
        .unwrap_or("?");
    check_assignable_state(agent, Some(t.role), state)
}

/// `--override-capabilities` and its reason (spec/profiles#requirements-and-matching).
#[derive(Debug, Clone, Copy, Default)]
pub struct CapOverride<'a> {
    pub flag: bool,
    pub reason: Option<&'a str>,
}

/// The requirements of the ticket (`Needs:` lines, `needs:` labels) against the profile of
/// the agent that gets it. Returns the lines for the signed assignment message and for the
/// user: empty if there was nothing to record.
pub(super) fn check_capabilities(
    world: &World,
    info: &IssueInfo,
    agent: &str,
    ov: CapOverride<'_>,
) -> CbResult<(String, String)> {
    let needs = crate::capabilities::needs_of_ticket(&info.body, &info.labels)?;
    let caps = world
        .trust
        .get(agent)
        .and_then(|t| world.items.get(&t.item_id))
        .map(|i| crate::ops::profile::caps_of(Some(i)))
        .unwrap_or_default();
    let done = crate::capabilities::check_assign(agent, &needs, &caps, ov.flag, ov.reason)?;
    Ok(match done {
        Some(o) => (
            format!(
                "capability-override: {}\noverride-reason: {}\n",
                o.missing.join(", "),
                o.reason
            ),
            format!(
                "capabilities: OVERRIDDEN, {agent} lacks {} (reason: {})\n",
                o.missing.join(", "),
                o.reason
            ),
        ),
        None if needs.is_empty() => (String::new(), String::new()),
        None => (
            String::new(),
            format!("capabilities: {agent} meets {}\n", needs.join(", ")),
        ),
    })
}

/// The signed assignment message. Its `gate:` line holds every `Gate:` line of the body (one
/// line: that line): the headless "CI only" judgement reads it, not the body
/// (spec/tickets#ticket-content). `CI: none` is signed too (`ci: none`, else `ci: required`):
/// `done`, `verify` and `merge` read this line, not the live body. `gate_field` is the fallback
/// when the body has no such line. `extra` holds the further lines (order, moved-from,
/// override), each with its final newline. `assign` and the move of a queued ticket both post it,
/// because the signed log takes the assignee only from this message.
pub(super) fn post_assignment(
    ac: &AgentCtx,
    t: &Ticket,
    to: &AgentName,
    gate_field: Option<String>,
    extra: &str,
) -> CbResult<crate::ops::say::Posted> {
    let signed_gate = crate::ticket::gate_line_of(&t.info.body).or(gate_field);
    let ci_none = declares_ci_none(&t.info.body);
    post(
        ac,
        t,
        to_agent(to.as_str())?,
        Kind::Request,
        Re::None,
        &crate::protocol::acts::assignment_body(
            &t.info.title,
            &t.key(),
            &t.item_id,
            signed_gate.as_deref(),
            Some(ci_none),
            extra,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_dev_agent_can_be_assigned_to_and_the_others_are_refused() {
        // pass: active takes work at once, working takes it into the queue
        assert!(check_assignable_state("a@x", Some(Role::Dev), "active").is_ok());
        assert!(check_assignable_state("a@x", Some(Role::Dev), "working").is_ok());
        // refusal: a paused, draining, left or unknown State, and a non-dev role
        for st in ["paused", "draining", "left", "approved", "requested", "?"] {
            let e = check_assignable_state("a@x", Some(Role::Dev), st).unwrap_err();
            assert!(
                e.to_string().contains("status-agent-not-active-dev"),
                "{st}: {e}"
            );
        }
        for role in [Some(Role::Coordinator), None] {
            let e = check_assignable_state("a@x", role, "active").unwrap_err();
            assert!(e.to_string().contains("not a dev agent"), "{e}");
        }
    }
}
