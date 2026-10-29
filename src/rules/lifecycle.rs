//! Agent lifecycle (spec/board#lifecycle), single coordinator and coordinator-cannot-take
//! (spec/board#coordinator-role).
//!
//! `stale` and `dead` are derived, never written, so they are not states here.

use super::{Refusal, Role, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Requested,
    Approved,
    Active,
    Paused,
    Draining,
    Left,
}

impl State {
    pub const ALL: [State; 6] = [
        State::Requested,
        State::Approved,
        State::Active,
        State::Paused,
        State::Draining,
        State::Left,
    ];

    /// The option name on the board (the `State` field).
    pub fn name(self) -> &'static str {
        match self {
            State::Requested => "requested",
            State::Approved => "approved",
            State::Active => "active",
            State::Paused => "paused",
            State::Draining => "draining",
            State::Left => "left",
        }
    }

    /// Read a `State` value from the board. The legacy value `working` of an old board reads as
    /// `active`: this binary never writes it (spec/board#lifecycle, "busy" is derived from the
    /// tickets). `migrate` sets the old items to `active` and removes the option.
    pub fn parse(s: &str) -> Option<State> {
        if s == "working" {
            return Some(State::Active);
        }
        State::ALL.into_iter().find(|x| x.name() == s)
    }
}

/// Who performs a lifecycle change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// The agent the item describes.
    SelfAgent,
    /// The owner.
    Owner,
    /// Another agent (including the coordinator). Never allowed to change a state.
    #[cfg(test)]
    OtherAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Allowed {
    SelfOnly,
    OwnerOnly,
    SelfOrOwner,
}

struct Edge {
    from: Option<State>,
    to: State,
    by: Allowed,
    command: &'static str,
}

use State::*;

const fn e(from: Option<State>, to: State, by: Allowed, command: &'static str) -> Edge {
    Edge {
        from,
        to,
        by,
        command,
    }
}

/// The lifecycle graph as data. `from = None` is "not yet on the board".
const EDGES: &[Edge] = &[
    e(None, Requested, Allowed::SelfOnly, "join"),
    e(
        Some(Requested),
        Approved,
        Allowed::OwnerOnly,
        "/crew approve",
    ),
    e(Some(Approved), Active, Allowed::SelfOnly, "start"),
    e(Some(Approved), Left, Allowed::SelfOrOwner, "stop"),
    e(Some(Paused), Active, Allowed::SelfOrOwner, "start/resume"),
    e(Some(Active), Paused, Allowed::SelfOrOwner, "pause"),
    e(Some(Active), Draining, Allowed::SelfOrOwner, "stop"),
    e(Some(Paused), Draining, Allowed::SelfOrOwner, "stop"),
    e(Some(Draining), Left, Allowed::SelfOrOwner, "stop (finish)"),
];

/// Check one lifecycle change.
pub fn check_lifecycle(from: Option<State>, to: State, who: Who) -> Verdict {
    let edge = EDGES
        .iter()
        .find(|x| x.from == from && x.to == to)
        .ok_or_else(|| {
            Refusal::new(
                "lifecycle-unknown-transition",
                format!("{from:?} -> {to:?} is not in the lifecycle"),
            )
        })?;
    let ok = matches!(
        (edge.by, who),
        (Allowed::SelfOnly, Who::SelfAgent)
            | (Allowed::OwnerOnly, Who::Owner)
            | (Allowed::SelfOrOwner, Who::SelfAgent | Who::Owner)
    );
    if !ok {
        return Err(Refusal::new(
            "lifecycle-wrong-actor",
            format!(
                "{from:?} -> {to:?} ('{}') is not allowed for {who:?}",
                edge.command
            ),
        ));
    }
    Ok(())
}

/// `resume` (spec/board#lifecycle: agent or owner). An agent that the owner paused with `/crew
/// pause` is released only by the owner (`/crew resume`): the agent's own `resume` would otherwise
/// undo the owner's command. Level: loud.
pub fn check_resume(who: Who, owner_paused: bool) -> Verdict {
    if owner_paused && who == Who::SelfAgent {
        return Err(Refusal::new(
            "resume-owner-paused",
            "the owner paused this agent (/crew pause); only the owner resumes it (/crew resume)",
        ));
    }
    Ok(())
}

/// `stop --force` (owner or coordinator): an agent that cannot drain goes to `left` at once.
/// It works for an agent that runs or is approved (`approved`, `active`, `paused`, `draining`).
/// A `requested` agent is not approved (`/crew reject` ends such a request); a `left` agent has
/// left already. Level: loud.
pub fn check_force_stop(state: Option<State>) -> Verdict {
    match state {
        Some(Approved | Active | Paused | Draining) => Ok(()),
        Some(Left) => Err(Refusal::new(
            "stop-force-already-left",
            "the agent has left already (State left); without a stop in the registry log its tickets stay, and the owner records the stop with `crewboard stop --agent <name> --force`",
        )),
        Some(Requested) | None => Err(Refusal::new(
            "stop-force-not-running",
            "the agent is not an approved, running agent: a join request is ended with /crew reject",
        )),
    }
}

/// A paused agent takes nothing new (spec/board#lifecycle). Level: loud (`take`), and `assign`
/// refuses it.
pub fn check_not_paused(state: Option<State>) -> Verdict {
    if state == Some(Paused) {
        return Err(Refusal::new(
            "agent-paused",
            "this agent is paused: it finishes its current step and takes nothing new; `crewboard resume` first",
        ));
    }
    Ok(())
}

/// The coordinator does not write code, so it cannot take a ticket (spec/board#coordinator-role).
/// Level: loud.
pub fn check_take(role: Option<Role>) -> Verdict {
    match role {
        Some(Role::Dev) => Ok(()),
        Some(Role::Coordinator) => Err(Refusal::new(
            "coordinator-cannot-take",
            "the coordinator does not take tickets",
        )),
        None => Err(Refusal::new(
            "take-needs-role",
            "an agent without a role cannot take a ticket",
        )),
    }
}

/// One row of the crew, for the single-coordinator check.
#[derive(Debug, Clone)]
pub struct CrewMember {
    pub name: String,
    pub role: Option<Role>,
    pub state: State,
}

/// Exactly one coordinator per initiative (spec/board#coordinator-role). A member in state `Left`
/// does not count, so a handover works once the old coordinator is `Dev` or has left.
pub fn check_single_coordinator(crew: &[CrewMember], candidate: &str, new_role: Role) -> Verdict {
    if new_role != Role::Coordinator {
        return Ok(());
    }
    match crew
        .iter()
        .find(|m| m.name != candidate && m.role == Some(Role::Coordinator) && m.state != Left)
    {
        Some(other) => Err(Refusal::new(
            "single-coordinator",
            format!(
                "{} is already the coordinator; set it to Dev or stop it first (handover)",
                other.name
            ),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(from: Option<State>, to: State, who: Who) -> &'static str {
        check_lifecycle(from, to, who)
            .expect_err("must be refused")
            .rule
    }

    #[test]
    fn a_forced_stop_needs_an_approved_agent_that_has_not_left() {
        for s in [Approved, Active, Paused, Draining] {
            assert!(check_force_stop(Some(s)).is_ok(), "{s:?}");
        }
        assert_eq!(
            check_force_stop(Some(Left)).unwrap_err().rule,
            "stop-force-already-left"
        );
        for s in [Some(Requested), None] {
            assert_eq!(
                check_force_stop(s).unwrap_err().rule,
                "stop-force-not-running"
            );
        }
    }

    #[test]
    fn join_by_the_agent_only() {
        assert!(check_lifecycle(None, Requested, Who::SelfAgent).is_ok());
        assert_eq!(rule(None, Requested, Who::Owner), "lifecycle-wrong-actor");
        assert_eq!(
            rule(None, Requested, Who::OtherAgent),
            "lifecycle-wrong-actor"
        );
    }

    #[test]
    fn approve_by_the_owner_only() {
        assert!(check_lifecycle(Some(Requested), Approved, Who::Owner).is_ok());
        for who in [Who::SelfAgent, Who::OtherAgent] {
            assert_eq!(
                rule(Some(Requested), Approved, who),
                "lifecycle-wrong-actor"
            );
        }
    }

    #[test]
    fn requested_to_left_is_unknown() {
        // `/crew reject` marks the registry only; nothing writes this edge.
        for who in [Who::Owner, Who::SelfAgent] {
            assert_eq!(
                rule(Some(Requested), Left, who),
                "lifecycle-unknown-transition"
            );
        }
    }

    #[test]
    fn approved_to_left_by_self_or_owner_and_not_by_another_agent() {
        assert!(check_lifecycle(Some(Approved), Left, Who::SelfAgent).is_ok());
        assert!(check_lifecycle(Some(Approved), Left, Who::Owner).is_ok());
        assert_eq!(
            rule(Some(Approved), Left, Who::OtherAgent),
            "lifecycle-wrong-actor"
        );
    }

    #[test]
    fn start_by_the_agent_only_and_only_when_approved() {
        assert!(check_lifecycle(Some(Approved), Active, Who::SelfAgent).is_ok());
        assert_eq!(
            rule(Some(Approved), Active, Who::Owner),
            "lifecycle-wrong-actor"
        );
        // A requested agent cannot start itself.
        assert_eq!(
            rule(Some(Requested), Active, Who::SelfAgent),
            "lifecycle-unknown-transition"
        );
        assert_eq!(
            rule(None, Active, Who::SelfAgent),
            "lifecycle-unknown-transition"
        );
    }

    #[test]
    fn the_lifecycle_has_no_working_state() {
        // `take` and `done` write no State: nothing leads to or from `working`.
        assert!(State::ALL.iter().all(|s| s.name() != "working"));
        assert_eq!(State::parse("working"), Some(Active));
        assert_eq!(State::parse("nonsense"), None);
    }

    #[test]
    fn coordinator_cannot_take() {
        assert!(check_take(Some(Role::Dev)).is_ok());
        assert_eq!(
            check_take(Some(Role::Coordinator)).unwrap_err().rule,
            "coordinator-cannot-take"
        );
        assert_eq!(check_take(None).unwrap_err().rule, "take-needs-role");
    }

    #[test]
    fn pause_resume_stop_by_agent_or_owner() {
        let cases = [
            (Active, Paused),
            (Approved, Left),
            (Paused, Active),
            (Active, Draining),
            (Paused, Draining),
            (Draining, Left),
        ];
        for (from, to) in cases {
            assert!(
                check_lifecycle(Some(from), to, Who::SelfAgent).is_ok(),
                "{from:?}->{to:?}"
            );
            assert!(
                check_lifecycle(Some(from), to, Who::Owner).is_ok(),
                "{from:?}->{to:?}"
            );
            assert_eq!(
                rule(Some(from), to, Who::OtherAgent),
                "lifecycle-wrong-actor",
                "{from:?}->{to:?}"
            );
        }
    }

    #[test]
    fn transitions_outside_the_graph_are_refused() {
        // An approved agent that never started does not drain or pause: it leaves or starts.
        for to in [Draining, Paused] {
            assert_eq!(
                rule(Some(Approved), to, Who::SelfAgent),
                "lifecycle-unknown-transition"
            );
        }
        for s in State::ALL {
            // Nothing leaves `Left`: a departed agent joins again as a new item.
            if s != Left {
                assert_eq!(
                    rule(Some(Left), s, Who::Owner),
                    "lifecycle-unknown-transition"
                );
            }
            // No self loops.
            assert_eq!(rule(Some(s), s, Who::Owner), "lifecycle-unknown-transition");
        }
    }

    #[test]
    fn every_edge_passes_for_its_actor_and_fails_for_an_other_agent() {
        for edge in EDGES {
            let who = match edge.by {
                Allowed::OwnerOnly => Who::Owner,
                _ => Who::SelfAgent,
            };
            assert!(
                check_lifecycle(edge.from, edge.to, who).is_ok(),
                "{}",
                edge.command
            );
            assert!(
                check_lifecycle(edge.from, edge.to, Who::OtherAgent).is_err(),
                "{}",
                edge.command
            );
        }
    }

    fn member(name: &str, role: Option<Role>, state: State) -> CrewMember {
        CrewMember {
            name: name.into(),
            role,
            state,
        }
    }

    #[test]
    fn single_coordinator() {
        let crew = vec![
            member("claude@host-a", Some(Role::Coordinator), Active),
            member("hermes@host-b", Some(Role::Dev), Paused),
        ];
        // Refused: a second coordinator.
        let e = check_single_coordinator(&crew, "hermes@host-b", Role::Coordinator).unwrap_err();
        assert_eq!(e.rule, "single-coordinator");
        // Passes: a dev role, or re-granting the same coordinator.
        assert!(check_single_coordinator(&crew, "hermes@host-b", Role::Dev).is_ok());
        assert!(check_single_coordinator(&crew, "claude@host-a", Role::Coordinator).is_ok());
        // Passes: the first coordinator on an empty crew, or after the old one left.
        assert!(check_single_coordinator(&[], "claude@host-a", Role::Coordinator).is_ok());
        let after = vec![member("claude@host-a", Some(Role::Coordinator), Left)];
        assert!(check_single_coordinator(&after, "hermes@host-b", Role::Coordinator).is_ok());
        // Handover: the old coordinator became Dev first.
        let handover = vec![member("claude@host-a", Some(Role::Dev), Active)];
        assert!(check_single_coordinator(&handover, "hermes@host-b", Role::Coordinator).is_ok());
    }

    #[test]
    fn a_paused_agent_takes_nothing_new_and_others_may() {
        assert_eq!(
            check_not_paused(Some(Paused)).unwrap_err().rule,
            "agent-paused"
        );
        for st in [Some(Active), Some(Draining), Some(Approved), None] {
            assert!(check_not_paused(st).is_ok(), "{st:?}");
        }
    }

    #[test]
    fn an_owner_pause_is_lifted_only_by_the_owner() {
        // not paused by the owner: the agent resumes itself, the owner may too
        assert!(check_resume(Who::SelfAgent, false).is_ok());
        assert!(check_resume(Who::Owner, false).is_ok());
        // paused by the owner: the owner's resume passes, the agent's own is refused
        assert!(check_resume(Who::Owner, true).is_ok());
        assert_eq!(
            check_resume(Who::SelfAgent, true).unwrap_err().rule,
            "resume-owner-paused"
        );
    }
}
