//! The checks before an assignment: can the agent take work, and does it meet the needs of the
//! ticket, and the signed assignment message. Shared by `assign`, `new`, `order` and `reassign`.

use super::{post, to_agent};
use crate::base::ids;
use crate::ctx::{AgentCtx, World};
use crate::error::{CbError, CbResult};
use crate::protocol::names::{AgentName, Kind, Re};
use crate::rules::Role;
use crate::rules::fields::Field;
use crate::services::ticket_loader::{IssueInfo, Ticket};
use crate::ticket::{
    VMsg, body_field, declares_ci_none, gate_line_of, signed_ci_none, signed_gate,
};

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
            ids::STATUS_AGENT_NOT_ACTIVE_DEV,
            format!("{agent} is not a dev agent"),
        ));
    }
    if state != "active" && state != "working" {
        // `working` is a legacy value (never written any more): it reads as `active`.
        return Err(CbError::refused(
            ids::STATUS_AGENT_NOT_ACTIVE_DEV,
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
            ids::STATUS_AGENT_NOT_ACTIVE_DEV,
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

/// The `gate:` and `ci:` values of a new assignment message.
pub(super) struct SignedLines {
    pub gate: Option<String>,
    /// `None`: no `ci:` line (the copy of an assignment that has none).
    pub ci_none: Option<bool>,
    /// The body has another Gate than the signed one.
    pub gate_differs: bool,
    /// One line for each signed value that differs from the body; empty when they agree.
    pub warnings: String,
}

/// What a new assignment message of the ticket `key` signs. The issue body is not signed: any
/// holder of an agent token can edit it. So the body is read only for a ticket that has no
/// signed assignment (`msgs`: [`signed_gate`] is `None`): `gate:` holds every `Gate:` line of the
/// body (`gate_field` is the fallback when the body has no such line), and `ci:` says whether
/// the body declares `CI: none`. A ticket with a signed assignment keeps both signed values on
/// every later assignment message (a move of a queued ticket, an `assign` after a `reassign`),
/// as `reassign --to` copies them. A body that differs then gives a warning line. Pure.
pub(super) fn assignment_lines(
    key: &str,
    msgs: &[VMsg],
    body: &str,
    gate_field: Option<String>,
) -> SignedLines {
    let live_gate = gate_line_of(body).or_else(|| body_field(body, "Gate"));
    let Some(signed) = signed_gate(msgs) else {
        return SignedLines {
            gate: live_gate.or(gate_field),
            ci_none: Some(declares_ci_none(body)),
            gate_differs: false,
            warnings: String::new(),
        };
    };
    let ci_none = signed_ci_none(msgs);
    let mut warnings = String::new();
    let gate_differs = live_gate.as_deref().map(str::trim) != Some(signed.trim());
    if gate_differs {
        warnings.push_str(&format!(
            "warning: the Gate line in the body of {key} differs from the signed Gate: the body was edited after the assignment. The signed Gate stays: '{signed}'\n"
        ));
    }
    if let Some(c) = ci_none.filter(|c| *c != declares_ci_none(body)) {
        warnings.push_str(&format!(
            "warning: the CI line in the body of {key} differs from the signed line: the body was edited after the assignment. The signed line stays: '{}'\n",
            crate::protocol::acts::ci_line(c).trim_end()
        ));
    }
    SignedLines {
        gate: Some(signed),
        ci_none,
        gate_differs,
        warnings,
    }
}

/// The signed assignment message. `lines` ([`assignment_lines`]) holds its `gate:` and `ci:`
/// values: the headless "CI only" judgement, `done`, `verify` and `merge` read these lines, not
/// the body (spec/tickets#ticket-content). `extra` holds the further lines (order, moved-from,
/// override), each with its final newline. `assign` and the move of a queued ticket both post it,
/// because the signed log takes the assignee only from this message.
pub(super) fn post_assignment(
    ac: &AgentCtx,
    t: &Ticket,
    to: &AgentName,
    lines: &SignedLines,
    extra: &str,
) -> CbResult<crate::ops::say::Posted> {
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
            lines.gate.as_deref(),
            lines.ci_none,
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

    fn assigned(id: u64, lines: &str) -> VMsg {
        crate::testkit::msg(
            id,
            crate::ticket::Sender::Agent {
                name: "coord@host".into(),
                role: Role::Coordinator,
            },
            "dev@host",
            Some(Kind::Request),
            &format!("Assigned to you: T\nticket: o/r#1\nitem=x\n{lines}"),
        )
    }

    #[test]
    fn a_ticket_without_a_signed_assignment_signs_the_body() {
        let l = assignment_lines("o/r#1", &[], "Goal\nGate: a\nGate: b\nCI: none\n", None);
        assert_eq!(l.gate.as_deref(), Some("a | b"));
        assert_eq!(l.ci_none, Some(true));
        assert!(!l.gate_differs && l.warnings.is_empty(), "{}", l.warnings);
        // no Gate line in the body: the fallback; no `CI: none`: `ci: required`
        let l = assignment_lines("o/r#1", &[], "Goal\n", Some("ci".into()));
        assert_eq!(l.gate.as_deref(), Some("ci"));
        assert_eq!(l.ci_none, Some(false));
        // an assignment without a gate line is no signed assignment: the body is read
        let old = [assigned(1, "")];
        let l = assignment_lines("o/r#1", &old, "Gate: a\n", None);
        assert_eq!(l.gate.as_deref(), Some("a"));
        assert!(l.warnings.is_empty(), "{}", l.warnings);
    }

    #[test]
    fn a_ticket_with_a_signed_assignment_keeps_the_signed_lines_and_warns_about_the_body() {
        let msgs = [assigned(1, "gate: checks: a, b\nci: required\n")];
        // the body agrees: the same lines, no warning
        let l = assignment_lines("o/r#1", &msgs, "Gate: checks: a, b\n", None);
        assert_eq!(l.gate.as_deref(), Some("checks: a, b"));
        assert_eq!(l.ci_none, Some(false));
        assert!(!l.gate_differs && l.warnings.is_empty(), "{}", l.warnings);
        // the body has a weaker Gate and `CI: none`: the signed lines stay, two warnings
        let l = assignment_lines("o/r#1", &msgs, "Gate: ci\nCI: none\n", Some("ci".into()));
        assert_eq!(l.gate.as_deref(), Some("checks: a, b"));
        assert_eq!(l.ci_none, Some(false));
        assert!(l.gate_differs);
        let w: Vec<&str> = l.warnings.lines().collect();
        assert_eq!(w.len(), 2, "{}", l.warnings);
        assert!(
            w[0].starts_with("warning: the Gate line in the body of o/r#1 differs")
                && w[0].ends_with("The signed Gate stays: 'checks: a, b'"),
            "{}",
            w[0]
        );
        assert!(
            w[1].starts_with("warning: the CI line in the body of o/r#1 differs")
                && w[1].ends_with("'ci: required'"),
            "{}",
            w[1]
        );
        // the body lost its Gate line: the signed Gate stays too
        let l = assignment_lines("o/r#1", &msgs, "Goal\n", None);
        assert_eq!(l.gate.as_deref(), Some("checks: a, b"));
        assert!(l.gate_differs);
        // a signed assignment without a `ci:` line: none is added, and the CI line gives no
        // warning (the body still decides for that ticket)
        let no_ci = [assigned(1, "gate: g\n")];
        let l = assignment_lines("o/r#1", &no_ci, "Gate: g\nCI: none\n", None);
        assert_eq!((l.gate.as_deref(), l.ci_none), (Some("g"), None));
        assert!(l.warnings.is_empty(), "{}", l.warnings);
        // the latest signed assignment decides
        let two = [
            assigned(1, "gate: g\nci: none\n"),
            assigned(2, "gate: h\nci: required\n"),
        ];
        let l = assignment_lines("o/r#1", &two, "Gate: g\nCI: none\n", None);
        assert_eq!((l.gate.as_deref(), l.ci_none), (Some("h"), Some(false)));
    }
}
