//! Liveness of agents (spec/board#fields, spec/board#lifecycle): `late` and `dead` are derived,
//! never written. The one input is the age of the agent's `Heartbeat` field.
//!
//! - **alive**: the last sign of life is at most 3 intervals old.
//! - **late**: older than 3 intervals.
//! - **dead**: older than 5 intervals.
//!
//! The last sign of life is the `Heartbeat` value, capped at the `updatedAt` of the agent item
//! ([`last_seen`]). No message is needed: the coordinator sends no `ping`, and an agent sends no
//! `pong` (the kinds stay in the wire format, see spec/messages#kinds).
//!
//! Everything here is pure. The caller passes the time (`now`, Unix seconds) and the
//! interval. A test that uses a fixed clock passes times from that clock; nothing in this module
//! reads the wall clock.

use crate::clock::parse_interval;
use crate::error::{CbError, CbResult};
use crate::rules::status::Status;

/// spec/board#fields: default heartbeat interval, 10 minutes.
pub const DEFAULT_INTERVAL_SECS: i64 = 600;
/// An agent is late when its last sign of life is older than this many intervals.
pub const LATE_INTERVALS: i64 = 3;
/// An agent is dead when its last sign of life is older than this many intervals.
pub const DEAD_INTERVALS: i64 = 5;

/// An empty value is "not set".
fn non_empty(v: Option<&str>) -> Option<&str> {
    v.filter(|s| !s.is_empty())
}

/// The heartbeat interval in seconds (spec/board#fields): the hidden test variable
/// (`CREWBOARD_TEST_HEARTBEAT_INTERVAL`) if set, else `heartbeat_interval` of the initiative file
/// (`cfg`), else the default. The one value judges every agent (late, dead, the late scan) and
/// sets how often an agent writes its heartbeat.
pub fn resolve_interval(cfg: Option<&str>, test: Option<&str>) -> CbResult<i64> {
    let parse = |v: &str| parse_interval(v).map_err(CbError::config);
    match non_empty(test).or(non_empty(cfg)) {
        Some(v) => parse(v),
        None => Ok(DEFAULT_INTERVAL_SECS),
    }
}

/// The agent states in which an agent runs and must write heartbeats (spec/board#lifecycle). A
/// paused agent is alive: it keeps its heartbeat.
pub fn heartbeat_state(state: &str) -> bool {
    matches!(state, "active" | "working" | "paused" | "draining")
}

/// Is a new heartbeat due? One write per interval (GitHub cost: not one per poll). Late
/// comes only after 3 intervals, so a slow cycle does not make an agent look late.
pub fn heartbeat_due(hb: Option<i64>, now: i64, interval: i64) -> bool {
    match hb {
        None => true,
        Some(t) => now - t >= interval.max(1),
    }
}

/// The last sign of life of an agent: its `Heartbeat` value, capped at the `updatedAt` of its
/// board item. The heartbeat is the writer's clock as text; `updatedAt` is GitHub's clock at the
/// time of the write. A writer whose clock runs ahead cannot look alive for longer than GitHub saw
/// the item change. `updatedAt` alone is not used: any other write to the item (an owner command,
/// a `State` change) bumps it and would hide a silent agent. `None`: no heartbeat and no
/// `updatedAt`. A heartbeat that is later than `updatedAt` by seconds only is not a problem:
/// the write itself comes after the text.
pub fn last_seen(hb: Option<i64>, updated: Option<i64>) -> Option<i64> {
    match (hb, updated) {
        (Some(h), Some(u)) => Some(h.min(u)),
        (h, u) => h.or(u),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// The agent is not in a running state (requested, approved, left): nothing to judge.
    NotRunning,
    Alive {
        age: i64,
    },
    /// The last sign of life is over 3 intervals old. `age` is `None` if there is none.
    Late {
        age: Option<i64>,
    },
    /// The last sign of life is over 5 intervals old.
    Dead {
        age: i64,
    },
}

impl Liveness {
    pub fn word(&self) -> &'static str {
        match self {
            Liveness::NotRunning => "-",
            Liveness::Alive { .. } => "alive",
            Liveness::Late { .. } => "late",
            Liveness::Dead { .. } => "dead",
        }
    }
}

/// Judge one agent. `state` is the `State` field, `seen` the time of its last sign of life
/// ([`last_seen`], or the baseline of `ops::late::agent_since`).
pub fn derive(state: Option<&str>, seen: Option<i64>, now: i64, interval: i64) -> Liveness {
    if !state.is_some_and(heartbeat_state) {
        return Liveness::NotRunning;
    }
    let Some(age) = seen.map(|t| now - t) else {
        return Liveness::Late { age: None };
    };
    if age <= LATE_INTERVALS * interval {
        Liveness::Alive { age }
    } else if age <= DEAD_INTERVALS * interval {
        Liveness::Late { age: Some(age) }
    } else {
        Liveness::Dead { age }
    }
}

/// `assign` gives work only to an agent that answers: a late or dead agent is refused.
pub fn check_alive(agent: &str, live: &Liveness) -> Result<(), crate::rules::Refusal> {
    match live {
        Liveness::Late { .. } | Liveness::Dead { .. } => {
            let age = match live {
                Liveness::Late { age } => *age,
                Liveness::Dead { age } => Some(*age),
                _ => None,
            };
            Err(crate::rules::Refusal::new(
                "status-agent-not-alive",
                format!(
                    "{agent} is {}: {} (docs/design/spec/board.md#fields); it would not see the ticket",
                    live.word(),
                    match age {
                        Some(a) => format!("no heartbeat for {a} s"),
                        None => "it never wrote a heartbeat".to_string(),
                    }
                ),
            ))
        }
        Liveness::Alive { .. } | Liveness::NotRunning => Ok(()),
    }
}

/// Ticket statuses that a derived reassign (agent dead or stopped) returns to `Ready`.
/// `Review` stays (the evidence is delivered, `verify` does not need the dev), `Backlog` and
/// `Done` are not with an agent. A question of the old assignee to the owner does not count after
/// the Agent changes: the new assignee asks again if needed.
pub fn reassignable_status(status: &str) -> bool {
    Status::parse_board(status).is_some_and(reassignable_parsed)
}

/// [`reassignable_status`] for a parsed status.
pub fn reassignable_parsed(status: Status) -> bool {
    matches!(status, Status::Ready | Status::InProgress | Status::Blocked)
}

/// The keys of the Ready tickets that an agent in State `paused` still holds. A paused agent
/// takes nothing new, so nothing starts such a ticket until the agent resumes: the coordinator
/// must hear of it (it is not a late wait). Empty for any other State.
pub fn paused_ready(
    state: Option<&str>,
    agent: &str,
    rows: &[crate::ticket::TicketRow],
) -> Vec<String> {
    if state != Some("paused") {
        return Vec::new();
    }
    rows.iter()
        .filter(|r| r.is_agent(agent))
        .filter(|r| r.in_status(Status::Ready))
        .map(crate::ticket::TicketRow::key)
        .collect()
}

/// The warning of a paused agent that holds Ready tickets.
pub fn paused_ready_text(agent: &str, keys: &[String]) -> String {
    format!(
        "warning: {agent} is paused and holds {} Ready ticket(s): {}. Nothing starts them while it is paused: `crewboard resume` it (the owner's pause: `/crew resume {agent}`), or move them with `crewboard assign <ticket> --to <agent>`",
        keys.len(),
        keys.join(", ")
    )
}

/// The justification of a reassign that needs no stated reason
/// (spec/tickets#status-flow-and-writers: "agent is dead, stopped"): the text, or a refusal if the
/// agent is not dead and the registry log does not stop it. `logged` is what the log says
/// (`Departure::reason` of the registry: "is stopping (the owner's /crew stop)"); the board
/// `State` is not asked, since one field write must not take the tickets of an agent. `state`
/// only names the State in the refusal.
pub fn derived_reassign_reason(
    agent: &str,
    logged: Option<&str>,
    state: Option<&str>,
    live: &Liveness,
    interval: i64,
) -> Result<String, crate::rules::Refusal> {
    match (logged, live) {
        (Some(why), _) => Ok(format!("agent {agent} {why}")),
        (None, Liveness::Dead { age }) => Ok(format!(
            "agent {agent} is dead: no heartbeat for {age} s (late after {} s, dead after {} s)",
            LATE_INTERVALS * interval,
            DEAD_INTERVALS * interval
        )),
        (None, l) => Err(crate::rules::Refusal::new(
            "status-reassign-not-justified",
            format!(
                "agent {agent} is neither dead nor stopped in the registry log (State {}, liveness {}): give a reason with --reason or wait",
                state.unwrap_or("?"),
                l.word()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One clock for the whole module: every time below is an offset from T0.
    const T0: i64 = 1_792_000_000;
    const I: i64 = 20;

    #[test]
    fn intervals_parse_and_bad_ones_are_refused() {
        assert_eq!(parse_interval("20s"), Ok(20));
        assert_eq!(parse_interval("10m"), Ok(600));
        assert_eq!(parse_interval("2h"), Ok(7200));
        assert_eq!(parse_interval("45"), Ok(45));
        for bad in ["", "0", "0s", "-5", "1d", "abc", "1.5m", "90000"] {
            assert!(parse_interval(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_interval_is_the_test_variable_then_the_file_then_the_default() {
        assert_eq!(resolve_interval(None, None).unwrap(), 600);
        assert_eq!(resolve_interval(Some("5m"), None).unwrap(), 300);
        assert_eq!(resolve_interval(Some("5m"), Some("2s")).unwrap(), 2);
        // an empty value is "not set", a bad value is a configuration error
        assert_eq!(resolve_interval(Some(""), Some("")).unwrap(), 600);
        assert_eq!(resolve_interval(Some("5m"), Some("")).unwrap(), 300);
        assert!(resolve_interval(Some("soon"), None).is_err());
        assert!(resolve_interval(None, Some("soon")).is_err());
    }

    #[test]
    fn heartbeats_are_written_by_running_states_only() {
        for s in ["active", "working", "paused", "draining"] {
            assert!(heartbeat_state(s), "{s}");
        }
        for s in ["requested", "approved", "left", "?"] {
            assert!(!heartbeat_state(s), "{s}");
        }
    }

    #[test]
    fn a_heartbeat_is_due_once_per_interval_and_not_before() {
        assert!(heartbeat_due(None, T0, I));
        assert!(!heartbeat_due(Some(T0), T0 + 19, I));
        assert!(heartbeat_due(Some(T0), T0 + 20, I));
        // polls in between write nothing (one write per interval, not per poll)
        for dt in 0..20 {
            assert!(!heartbeat_due(Some(T0), T0 + dt, I));
        }
        assert!(!heartbeat_due(Some(T0), T0, 1));
        assert!(heartbeat_due(Some(T0), T0 + 1, 1));
    }

    #[test]
    fn the_thresholds_are_three_and_five_intervals() {
        assert_eq!((LATE_INTERVALS, DEAD_INTERVALS), (3, 5));
    }

    #[test]
    fn the_state_changes_at_exactly_three_and_five_intervals_of_age() {
        let l = |dt: i64| derive(Some("active"), Some(T0), T0 + dt, I);
        assert_eq!(l(0), Liveness::Alive { age: 0 });
        // at the late threshold (3 intervals = 60 s): still alive; one second more: late
        assert_eq!(l(59), Liveness::Alive { age: 59 });
        assert_eq!(l(60), Liveness::Alive { age: 60 });
        assert_eq!(l(61), Liveness::Late { age: Some(61) });
        // at the dead threshold (5 intervals = 100 s): still late; one second more: dead
        assert_eq!(l(99), Liveness::Late { age: Some(99) });
        assert_eq!(l(100), Liveness::Late { age: Some(100) });
        assert_eq!(l(101), Liveness::Dead { age: 101 });
        assert_eq!(l(100_000), Liveness::Dead { age: 100_000 });
    }

    #[test]
    fn the_thresholds_follow_the_interval() {
        // the same age, a longer interval: a different state
        let at = |interval: i64| derive(Some("working"), Some(T0), T0 + 700, interval);
        assert_eq!(at(600), Liveness::Alive { age: 700 });
        assert_eq!(at(200), Liveness::Late { age: Some(700) });
        assert_eq!(at(100), Liveness::Dead { age: 700 });
    }

    #[test]
    fn a_heartbeat_in_the_future_reads_as_alive() {
        assert_eq!(
            derive(Some("active"), Some(T0 + 500), T0, I),
            Liveness::Alive { age: -500 }
        );
    }

    #[test]
    fn an_agent_without_any_sign_of_life_is_late_never_dead() {
        assert_eq!(
            derive(Some("working"), None, T0, I),
            Liveness::Late { age: None }
        );
        assert_eq!(
            derive(Some("working"), None, T0 + 1_000_000, I),
            Liveness::Late { age: None }
        );
    }

    #[test]
    fn the_last_sign_of_life_is_the_heartbeat_capped_at_updated_at() {
        // normal: the write comes after the text
        assert_eq!(last_seen(Some(T0), Some(T0 + 1)), Some(T0));
        // a writer clock that runs ahead cannot make the agent younger than GitHub saw
        assert_eq!(last_seen(Some(T0 + 9_000), Some(T0)), Some(T0));
        // another write to the item does not hide a silent agent
        assert_eq!(last_seen(Some(T0), Some(T0 + 9_000)), Some(T0));
        // one source missing
        assert_eq!(last_seen(Some(T0), None), Some(T0));
        assert_eq!(last_seen(None, Some(T0)), Some(T0));
        assert_eq!(last_seen(None, None), None);
    }

    #[test]
    fn agents_that_do_not_run_are_not_judged() {
        for s in [Some("requested"), Some("approved"), Some("left"), None] {
            assert_eq!(derive(s, Some(T0), T0 + 9_999, I), Liveness::NotRunning);
        }
        // control: the same numbers for a running agent are not "not running"
        assert_ne!(
            derive(Some("paused"), Some(T0), T0 + 9_999, I),
            Liveness::NotRunning
        );
    }

    fn row(agent: &str, status: &str, issue: u64) -> crate::ticket::TicketRow {
        crate::ticket::TicketRow {
            repo: "o/r".into(),
            issue,
            item_id: format!("PVTI_{issue}"),
            status: Some(status.into()),
            agent: Some(agent.into()),
            order: None,
            updated: None,
        }
    }

    #[test]
    fn a_paused_agent_with_ready_tickets_is_named_with_its_ready_tickets_only() {
        let rows = [
            row("b@y", "Ready", 1),
            row("b@y", "In Progress", 2),
            row("c@y", "Ready", 3),
            row("b@y", "Ready", 4),
        ];
        assert_eq!(
            paused_ready(Some("paused"), "b@y", &rows),
            vec!["o/r#1".to_string(), "o/r#4".to_string()]
        );
        // control: any other State, or no Ready ticket, gives nothing
        for s in [Some("active"), Some("draining"), Some("left"), None] {
            assert!(paused_ready(s, "b@y", &rows).is_empty(), "{s:?}");
        }
        assert!(paused_ready(Some("paused"), "c@y", &rows[..2]).is_empty());
        let t = paused_ready_text("b@y", &["o/r#1".into(), "o/r#4".into()]);
        assert!(t.starts_with("warning: b@y is paused and holds 2 Ready ticket(s): o/r#1, o/r#4."));
    }

    #[test]
    fn a_derived_reassign_is_justified_for_a_dead_stopping_or_departed_agent_only() {
        let dead = Liveness::Dead { age: 150 };
        let why = derived_reassign_reason("b@y", None, Some("working"), &dead, I).unwrap();
        assert!(
            why.contains("dead") && why.contains("150 s") && !why.contains("ping"),
            "{why}"
        );
        assert!(
            why.contains("late after 60 s") && why.contains("dead after 100 s"),
            "{why}"
        );
        // agents that the log stops or ends are justified without a heartbeat judgement
        let alive = Liveness::Alive { age: 1 };
        let why = derived_reassign_reason(
            "b@y",
            Some("has left (its signed lifecycle: left marker)"),
            Some("active"),
            &alive,
            I,
        )
        .unwrap();
        assert_eq!(
            why,
            "agent b@y has left (its signed lifecycle: left marker)"
        );
        // the board State alone justifies nothing: one field write must not take the tickets
        for st in ["draining", "left"] {
            let e = derived_reassign_reason("b@y", None, Some(st), &alive, I).unwrap_err();
            assert_eq!(e.rule, "status-reassign-not-justified", "{st}");
        }
        // refused: alive, only late (not yet dead), not running
        for l in [
            alive.clone(),
            Liveness::Late { age: Some(70) },
            Liveness::NotRunning,
        ] {
            let e = derived_reassign_reason("b@y", None, Some("working"), &l, I).unwrap_err();
            assert_eq!(e.rule, "status-reassign-not-justified");
        }
        assert_eq!(
            derived_reassign_reason("b@y", None, Some("paused"), &alive, I)
                .unwrap_err()
                .rule,
            "status-reassign-not-justified"
        );
    }

    #[test]
    fn assign_refuses_a_late_or_dead_agent_and_accepts_a_live_one() {
        for l in [
            Liveness::Late { age: Some(70) },
            Liveness::Late { age: None },
            Liveness::Dead { age: 200 },
        ] {
            assert_eq!(
                check_alive("b@y", &l).unwrap_err().rule,
                "status-agent-not-alive"
            );
        }
        assert!(check_alive("b@y", &Liveness::Alive { age: 3 }).is_ok());
        assert!(check_alive("b@y", &Liveness::NotRunning).is_ok());
    }

    #[test]
    fn only_tickets_with_a_live_owner_of_the_work_are_returned_by_a_derived_reassign() {
        for s in ["Ready", "In Progress", "Blocked", "Needs Human"] {
            assert!(reassignable_status(s), "{s}");
        }
        for s in ["Review", "Backlog", "Done", "?"] {
            assert!(!reassignable_status(s), "{s}");
        }
    }
}
