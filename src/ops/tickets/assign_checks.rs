//! The checks before an assignment: can the agent take work, and does it meet the needs of the
//! ticket, and the signed assignment message. Shared by `assign`, `new`, `order` and `reassign`.

use super::{post, to_agent, write_work};
use crate::base::ids;
use crate::board::FieldValue;
use crate::ctx::{AgentCtx, World};
use crate::error::{CbError, CbResult};
use crate::protocol::acts::ci_line;
use crate::protocol::names::{AgentName, Kind, Re};
use crate::rules::Role;
use crate::rules::fields::{Field, WriteCtx};
use crate::rules::limits::check_text_field;
use crate::services::ticket_loader::{IssueInfo, Ticket};
use crate::ticket::{VMsg, declares_ci_none, gate_line_of, signed_ci_none, signed_gate};

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
    /// `None`: no `gate:` line (the copy of an assignment that has none).
    pub gate: Option<String>,
    /// `None`: no `ci:` line (the copy of an assignment that has none).
    pub ci_none: Option<bool>,
    /// The lines for the answer of the command, each with its final newline: a warning for each
    /// signed value that differs from the body ([`copy_signed`]), or what the message signs from
    /// the body ([`first_from_body`]). Empty when a copy agrees with the body.
    pub notes: String,
}

/// The lines of a later assignment message of the ticket `key`: the copy of the signed `gate:`
/// and `ci:` lines of `msgs` ([`signed_gate`], [`signed_ci_none`]). The issue body is not
/// signed: any holder of an agent token can edit it. So a ticket whose log has an assignment
/// ([`crate::ticket::has_assignment`]) keeps its signed lines on every later assignment message:
/// `reassign --to`, a move of a queued ticket, an `assign` after a `reassign`. A line that the
/// log does not have is not added from the body. `body` is read only for the warning: one line
/// for each signed value that differs from it. Pure.
pub(super) fn copy_signed(key: &str, msgs: &[VMsg], body: &str) -> SignedLines {
    let gate = signed_gate(msgs);
    let ci_none = signed_ci_none(msgs);
    let mut notes = String::new();
    let gate_differs = gate
        .as_deref()
        .is_some_and(|g| gate_line_of(body).as_deref().map(str::trim) != Some(g.trim()));
    if let Some(g) = gate.as_deref().filter(|_| gate_differs) {
        notes.push_str(&format!(
            "warning: the Gate line in the body of {key} differs from the signed Gate: the body was edited after the assignment. The signed Gate stays: '{g}'\n"
        ));
    }
    if let Some(c) = ci_none.filter(|c| *c != declares_ci_none(body)) {
        notes.push_str(&format!(
            "warning: the CI line in the body of {key} differs from the signed line: the body was edited after the assignment. The signed line stays: '{}'\n",
            ci_line(c).trim_end()
        ));
    }
    if gate.is_none() {
        notes.push_str(&format!(
            "note: {key} has no signed Gate in its log, so the new assignment message has no `gate:` line either: the headless coordinator leaves the ticket\n"
        ));
    }
    SignedLines {
        gate,
        ci_none,
        notes,
    }
}

/// The note of a `Gate` field that is not written: the Gate is too long for a text field.
pub(super) const GATE_FIELD_KEPT: &str = "Gate field kept (the Gate is too long for the field)";

/// The one rule for the value of the `Gate` field: the Gate as an assignment message signs it
/// (every Gate line, joined), when it fits a text field. `None`: it does not fit, and the field
/// stays as it is ([`GATE_FIELD_KEPT`]). Pure.
pub(super) fn gate_field_value(gate: &str) -> Option<&str> {
    check_text_field(gate).is_ok().then_some(gate)
}

/// Writes `gate` into the `Gate` field of the item when the field (`current`) has another
/// value, by the rule of [`gate_field_value`]. `source` names where the Gate comes from
/// (`signed Gate` or `body`). Returns the line for the answer; empty when the field agrees.
pub(super) fn sync_gate_field(
    ac: &AgentCtx,
    item_id: &str,
    current: Option<&str>,
    (gate, source): (&str, &str),
    who: &WriteCtx,
) -> CbResult<String> {
    let Some(value) = gate_field_value(gate) else {
        return Ok(format!("{GATE_FIELD_KEPT}\n"));
    };
    if current == Some(value) {
        return Ok(String::new());
    }
    write_work(ac, item_id, Field::Gate, FieldValue::Text(value), who)?;
    Ok(format!("Gate field set from the {source}\n"))
}

/// The lines of the first assignment message of the ticket `key`, from the body: `gate:` holds
/// every `Gate:` line of the body (`gate_field` is the fallback when the body has no such line),
/// and `ci:` says whether the body declares `CI: none`. The notes name what is signed, so the
/// coordinator sees it. `assigned_on_board`: the Status of the ticket is past Backlog or it has
/// an Agent, but the log has no valid assignment (never posted, deleted, edited or not valid);
/// the notes then start with a warning. Pure.
pub(super) fn first_from_body(
    key: &str,
    body: &str,
    gate_field: Option<String>,
    assigned_on_board: bool,
) -> SignedLines {
    let gate = gate_line_of(body).or(gate_field);
    let ci_none = declares_ci_none(body);
    let mut notes = String::new();
    if assigned_on_board {
        notes.push_str(&format!(
            "warning: {key} has an Agent or a Status past Backlog, but its log has no valid signed assignment (never posted, deleted, edited or not valid). The Gate now comes from the unsigned body: check it\n"
        ));
    }
    notes.push_str(&format!(
        "signed from the body: gate '{}'; {}",
        gate.as_deref().unwrap_or("-"),
        ci_line(ci_none)
    ));
    SignedLines {
        gate,
        ci_none: Some(ci_none),
        notes,
    }
}

/// The signed assignment message. `lines` ([`first_from_body`] or [`copy_signed`]) holds its
/// `gate:` and `ci:` values: the headless "CI only" judgement, `done`, `verify` and `merge` read these lines, not
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
    fn the_first_assignment_signs_the_body_and_says_what_it_signs() {
        let l = first_from_body("o/r#1", "Goal\nGate: a\nGate: b\nCI: none\n", None, false);
        assert_eq!(l.gate.as_deref(), Some("a | b"));
        assert_eq!(l.ci_none, Some(true));
        assert_eq!(l.notes, "signed from the body: gate 'a | b'; ci: none\n");
        // defence only: `assign` refuses a body without a Gate line, so no command gets here
        let l = first_from_body("o/r#1", "Goal\n", Some("ci".into()), false);
        assert_eq!(l.gate.as_deref(), Some("ci"));
        assert_eq!(l.ci_none, Some(false));
        assert_eq!(l.notes, "signed from the body: gate 'ci'; ci: required\n");
        // the board shows an assignment that the log does not have: a warning comes first
        let l = first_from_body("o/r#1", "Gate: a\n", None, true);
        let n: Vec<&str> = l.notes.lines().collect();
        assert_eq!(n.len(), 2, "{}", l.notes);
        assert!(
            n[0].contains("(never posted, deleted, edited or not valid)"),
            "{}",
            n[0]
        );
        assert!(
            n[0].starts_with("warning: o/r#1 has an Agent or a Status past Backlog")
                && n[0].ends_with("The Gate now comes from the unsigned body: check it"),
            "{}",
            n[0]
        );
        assert_eq!(n[1], "signed from the body: gate 'a'; ci: required");
    }

    #[test]
    fn a_later_assignment_copies_the_signed_lines_and_warns_about_the_body() {
        let msgs = [assigned(1, "gate: checks: a, b\nci: required\n")];
        // the body agrees: the same lines, no warning
        let l = copy_signed("o/r#1", &msgs, "Gate: checks: a, b\n");
        assert_eq!(l.gate.as_deref(), Some("checks: a, b"));
        assert_eq!(l.ci_none, Some(false));
        assert!(l.notes.is_empty(), "{}", l.notes);
        // the body has a weaker Gate and `CI: none`: the signed lines stay, two warnings
        let l = copy_signed("o/r#1", &msgs, "Gate: ci\nCI: none\n");
        assert_eq!(l.gate.as_deref(), Some("checks: a, b"));
        assert_eq!(l.ci_none, Some(false));
        let w: Vec<&str> = l.notes.lines().collect();
        assert_eq!(w.len(), 2, "{}", l.notes);
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
        let l = copy_signed("o/r#1", &msgs, "Goal\n");
        assert_eq!(l.gate.as_deref(), Some("checks: a, b"));
        assert!(l.notes.starts_with("warning: the Gate line"), "{}", l.notes);
        // two Gate lines of the body are one signed line: no warning
        let joined = [assigned(1, "gate: a | b\nci: required\n")];
        let l = copy_signed("o/r#1", &joined, "Gate: a\n- **Gate:** b\n");
        assert!(l.notes.is_empty(), "{}", l.notes);
        // a signed assignment without a `ci:` line: none is added, and the CI line gives no
        // warning (the body still decides for that ticket)
        let no_ci = [assigned(1, "gate: g\n")];
        let l = copy_signed("o/r#1", &no_ci, "Gate: g\nCI: none\n");
        assert_eq!((l.gate.as_deref(), l.ci_none), (Some("g"), None));
        assert!(l.notes.is_empty(), "{}", l.notes);
        // an assignment without a `gate:` line: the copy has no line either, whatever the body
        // says, and nothing is compared
        let old = [assigned(1, "")];
        assert!(crate::ticket::has_assignment(&old));
        let l = copy_signed("o/r#1", &old, "Gate: ci\nCI: none\n");
        assert_eq!((l.gate, l.ci_none), (None, None));
        assert_eq!(
            l.notes,
            "note: o/r#1 has no signed Gate in its log, so the new assignment message has no `gate:` line either: the headless coordinator leaves the ticket\n"
        );
        // the field rule: a Gate that fits is the value, a longer one is none
        assert_eq!(gate_field_value("a | b"), Some("a | b"));
        assert_eq!(gate_field_value(&"x".repeat(1025)), None);
        // the latest signed assignment decides
        let two = [
            assigned(1, "gate: g\nci: none\n"),
            assigned(2, "gate: h\nci: required\n"),
        ];
        let l = copy_signed("o/r#1", &two, "Gate: g\nCI: none\n");
        assert_eq!((l.gate.as_deref(), l.ci_none), (Some("h"), Some(false)));
        // no assignment in the log: nothing to copy
        assert!(!crate::ticket::has_assignment(&[]));
    }
}
