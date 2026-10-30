//! Characterization of `check_status_transition`: every `(from, to, actor)` move under a set
//! of named fact profiles, compared with the recorded verdicts in `table.txt`.
//!
//! One line per `(profile, from)`: the verdicts for the seven targets in the order of
//! `Status::ALL`, each for the actors owner, coordinator, dev (`ok` or the rule name).

use super::*;
use crate::rules::Role;

/// The facts of a move, one flat record as the tests write them. `check` picks the typed
/// [`Transition`] of the move and hands it only the facts that this move reads.
#[derive(Clone, Copy, Default)]
pub(super) struct Profile {
    pub(super) gate_set: bool,
    pub(super) assignee_active_dev: bool,
    pub(super) actor_is_assignee: bool,
    pub(super) evidence_complete: bool,
    pub(super) verify: Option<bool>,
    pub(super) reassign_justified: bool,
    pub(super) answer_after_block: bool,
    pub(super) own_block: BlockReason,
    pub(super) split_off_proven: bool,
    pub(super) assignee_busy: bool,
    pub(super) new_head: bool,
    pub(super) done_reason: Option<DoneReason>,
}

const NONE: Profile = Profile {
    gate_set: false,
    assignee_active_dev: false,
    actor_is_assignee: false,
    evidence_complete: false,
    verify: None,
    reassign_justified: false,
    answer_after_block: false,
    own_block: BlockReason::Plain,
    split_off_proven: false,
    assignee_busy: false,
    new_head: false,
    done_reason: None,
};

const ALL_OK: Profile = Profile {
    gate_set: true,
    assignee_active_dev: true,
    actor_is_assignee: true,
    evidence_complete: true,
    verify: Some(true),
    reassign_justified: true,
    answer_after_block: true,
    ..NONE
};

fn fact_profiles() -> Vec<(&'static str, Profile)> {
    let parked = BlockReason::Parked { for_issue: 7 };
    vec![
        ("none", NONE),
        ("all_ok", ALL_OK),
        (
            "verify_unset",
            Profile {
                verify: None,
                ..ALL_OK
            },
        ),
        (
            "verify_failed",
            Profile {
                verify: Some(false),
                ..ALL_OK
            },
        ),
        (
            "verify_failed_busy",
            Profile {
                verify: Some(false),
                assignee_busy: true,
                ..ALL_OK
            },
        ),
        (
            "failed_busy_not_justified",
            Profile {
                verify: Some(false),
                assignee_busy: true,
                reassign_justified: false,
                ..ALL_OK
            },
        ),
        (
            "no_answer",
            Profile {
                answer_after_block: false,
                ..ALL_OK
            },
        ),
        (
            "parked_no_answer",
            Profile {
                answer_after_block: false,
                own_block: parked,
                ..ALL_OK
            },
        ),
        (
            "not_assignee",
            Profile {
                actor_is_assignee: false,
                ..ALL_OK
            },
        ),
        (
            "no_gate",
            Profile {
                gate_set: false,
                ..ALL_OK
            },
        ),
        (
            "no_active_dev",
            Profile {
                assignee_active_dev: false,
                ..ALL_OK
            },
        ),
        (
            "no_evidence",
            Profile {
                evidence_complete: false,
                ..ALL_OK
            },
        ),
        (
            "not_justified",
            Profile {
                reassign_justified: false,
                ..ALL_OK
            },
        ),
        (
            "split_proven",
            Profile {
                split_off_proven: true,
                ..ALL_OK
            },
        ),
        (
            "split_proven_no_evidence",
            Profile {
                split_off_proven: true,
                evidence_complete: false,
                ..ALL_OK
            },
        ),
        (
            "split_proven_not_assignee",
            Profile {
                split_off_proven: true,
                actor_is_assignee: false,
                ..ALL_OK
            },
        ),
        (
            "new_head",
            Profile {
                new_head: true,
                ..ALL_OK
            },
        ),
        (
            "new_head_no_evidence",
            Profile {
                new_head: true,
                evidence_complete: false,
                ..ALL_OK
            },
        ),
        (
            "new_head_not_assignee",
            Profile {
                new_head: true,
                actor_is_assignee: false,
                ..ALL_OK
            },
        ),
    ]
}

/// The profiles of the moves to Done.
fn reason_profiles() -> Vec<(&'static str, Profile)> {
    vec![
        (
            "reason_verified_close",
            Profile {
                done_reason: Some(DoneReason::VerifiedClose),
                ..ALL_OK
            },
        ),
        (
            "reason_verified_merge",
            Profile {
                done_reason: Some(DoneReason::VerifiedMerge),
                ..ALL_OK
            },
        ),
        (
            "reason_owner_merged",
            Profile {
                done_reason: Some(DoneReason::OwnerMerged),
                ..NONE
            },
        ),
        (
            "reason_verified_close_unverified",
            Profile {
                done_reason: Some(DoneReason::VerifiedClose),
                verify: None,
                ..ALL_OK
            },
        ),
    ]
}

fn profiles() -> Vec<(&'static str, Profile)> {
    [fact_profiles(), reason_profiles()].concat()
}

const STARTS: [Option<Status>; 8] = [
    None,
    Some(Status::Backlog),
    Some(Status::Ready),
    Some(Status::InProgress),
    Some(Status::Review),
    Some(Status::Blocked),
    Some(Status::Done),
    Some(Status::Dropped),
];

const ACTORS: [Actor; 3] = [
    Actor::Owner,
    Actor::Agent(Role::Coordinator),
    Actor::Agent(Role::Dev),
];

/// The typed move for `from -> to`, as a caller of the status table builds it.
fn transition(p: &Profile, from: Option<Status>, to: Status) -> Transition {
    use Status::*;
    let facts = TicketFacts {
        actor_is_assignee: p.actor_is_assignee,
        answer_after_block: p.answer_after_block,
        own_block: p.own_block,
        split_off_proven: p.split_off_proven,
    };
    match (from, to) {
        (None, _) => Transition::Create,
        (Some(Backlog), Ready) => Transition::Release {
            gate_set: p.gate_set,
            assignee_active_dev: p.assignee_active_dev,
        },
        (Some(Blocked), InProgress) => facts.resume(),
        (_, Review) => facts.deliver(p.evidence_complete, p.new_head),
        (Some(_), Dropped) => Transition::Drop,
        (_, Done) => Transition::Close {
            reason: p.done_reason,
            verify: p.verify,
        },
        (Some(Review), InProgress) => Transition::AfterVerify {
            verify: p.verify,
            assignee_busy: p.assignee_busy,
        },
        (Some(Review), Ready) if p.verify == Some(false) => Transition::AfterVerify {
            verify: p.verify,
            assignee_busy: p.assignee_busy,
        },
        (Some(_), Ready | Backlog) => Transition::Reassign {
            justified: p.reassign_justified,
        },
        _ => facts.plain(),
    }
}

/// `check_status_transition` for a flat record of facts.
pub(super) fn check(p: &Profile, from: Option<Status>, to: Status, actor: Actor) -> Verdict {
    check_status_transition(from, to, actor, &transition(p, from, to))
}

fn word(v: &Verdict) -> &'static str {
    match v {
        Ok(()) => "ok",
        Err(r) => r.rule.as_str(),
    }
}

fn line(name: &str, p: &Profile, from: Option<Status>) -> String {
    let from_name = from.map_or("new", Status::name);
    let verdicts: Vec<&str> = Status::ALL
        .into_iter()
        .flat_map(|to| ACTORS.map(|a| word(&check(p, from, to, a))))
        .collect();
    format!("{name}|{from_name}|{}", verdicts.join(" "))
}

fn rendered() -> String {
    let mut out = String::new();
    for (name, p) in profiles() {
        for from in STARTS {
            out.push_str(&line(name, &p, from));
            out.push('\n');
        }
    }
    out
}

#[test]
fn every_move_keeps_the_recorded_verdict() {
    let want = include_str!("table.txt");
    let got = rendered();
    for (w, g) in want.lines().zip(got.lines()) {
        assert_eq!(g, w);
    }
    assert_eq!(got.lines().count(), want.lines().count());
}

/// Rewrites `table.txt` from the current code (a maintainer step, never part of a normal run).
#[test]
#[ignore = "writes table.txt"]
fn record_the_table() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/rules/status/table.txt");
    std::fs::write(path, rendered()).expect("write table.txt");
}
