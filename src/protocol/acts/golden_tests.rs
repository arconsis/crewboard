//! Golden texts of every message that carries an act: the exact bytes that each writer posts,
//! and the act that [`parse`] reads back from them. A change of wording fails here first; the
//! wire of protocol v1 stays byte for byte what boards already hold.
//!
//! A text that no act parses (`done`) is pinned the same way, with the parser of its lines.

use super::*;
use crate::protocol::names::Kind;

/// One golden case: the writer's output, the bytes it must be, the kind it is posted with and the
/// act that the parser reads.
struct Golden {
    name: &'static str,
    text: String,
    bytes: &'static str,
    kind: Kind,
    act: Act,
}

fn check(cases: Vec<Golden>) {
    for g in cases {
        assert_eq!(g.text, g.bytes, "bytes of {}", g.name);
        assert_eq!(parse(Some(g.kind), &g.text), g.act, "act of {}", g.name);
    }
}

fn assigned(gate: Option<&str>, ci_none: Option<bool>) -> Act {
    Act::Assignment {
        gate: gate.map(str::to_string),
        ci_none,
    }
}

const PR: &str = "https://github.com/o/r/pull/9";
const BRANCH: &str = "crew/a@h/5-x";

fn evidence() -> EvidenceMsg {
    EvidenceMsg {
        pr: PR.into(),
        sha: "abc".into(),
        ci: "success".into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: None,
    }
}

fn local_evidence() -> EvidenceMsg {
    EvidenceMsg {
        ci: "local-gate".into(),
        ci_sources: Some("workflow-runs, statuses".into()),
        ci_runs: Some("counted: 9 (push, success)".into()),
        local_gate: Some("host-a: make test rc=0, log /tmp/gate.log".into()),
        ..evidence()
    }
}

#[test]
fn golden_assignment() {
    check(vec![
        Golden {
            name: "assign with gate and ci: none",
            text: assignment_body(
                "Fix it",
                "o/r#4",
                "PVTI_x",
                Some("CI green"),
                Some(true),
                "priority: P1\norder: 2\n",
            ),
            bytes: "Assigned to you: Fix it\nticket: o/r#4\nitem=PVTI_x\ngate: CI green\nci: none\npriority: P1\norder: 2\n",
            kind: Kind::Request,
            act: assigned(Some("CI green"), Some(true)),
        },
        Golden {
            name: "reassign with ci: required",
            text: assignment_body(
                "Fix it",
                "o/r#4",
                "PVTI_x",
                Some("a | b"),
                Some(false),
                "reason: r\n",
            ),
            bytes: "Assigned to you: Fix it\nticket: o/r#4\nitem=PVTI_x\ngate: a | b\nci: required\nreason: r\n",
            kind: Kind::Request,
            act: assigned(Some("a | b"), Some(false)),
        },
        Golden {
            name: "assignment without gate and ci lines",
            text: assignment_body("T", "o/r#1", "x", None, None, ""),
            bytes: "Assigned to you: T\nticket: o/r#1\nitem=x\n",
            kind: Kind::Request,
            act: assigned(None, None),
        },
    ]);
    assert_eq!(ci_line(true), "ci: none\n");
    assert_eq!(ci_line(false), "ci: required\n");
}

#[test]
fn golden_board_announce() {
    assert_eq!(board_announce_head(), "Ticket is on the board.");
    check(vec![
        Golden {
            name: "creation info",
            text: info_body("PVTI_x", 15),
            bytes: "Ticket is on the board.\nitem=PVTI_x\nproject=15\n",
            kind: Kind::Info,
            act: Act::BoardAnnounce,
        },
        Golden {
            name: "creation info that adopts a PR",
            text: info_body_adopting("PVTI_x", 15, PR, BRANCH),
            bytes: "Ticket is on the board.\nitem=PVTI_x\nproject=15\nadopts-pr: https://github.com/o/r/pull/9 branch: crew/a@h/5-x\n",
            kind: Kind::Info,
            act: Act::BoardAnnounce,
        },
    ]);
}

#[test]
fn golden_reassign_notice() {
    let pr = EarlierWork {
        pr: Some(PR.into()),
        branch: BRANCH.into(),
    };
    let branch = EarlierWork {
        pr: None,
        branch: BRANCH.into(),
    };
    check(vec![
        Golden {
            name: "reassign notice",
            text: reassigned_body(" stuck \n"),
            bytes: "Reassigned. Reason: stuck",
            kind: Kind::Info,
            act: Act::Reassigned,
        },
        Golden {
            name: "reassign notice without earlier work",
            text: reassigned_body_with("dead", None),
            bytes: "Reassigned. Reason: dead",
            kind: Kind::Info,
            act: Act::Reassigned,
        },
        Golden {
            name: "reassign notice with an open PR",
            text: reassigned_body_with("dead", Some(&pr)),
            bytes: "Reassigned. Reason: dead\nopen-pr: https://github.com/o/r/pull/9 branch: crew/a@h/5-x\n",
            kind: Kind::Info,
            act: Act::Reassigned,
        },
        Golden {
            name: "reassign notice with a pushed branch",
            text: reassigned_body_with("dead", Some(&branch)),
            bytes: "Reassigned. Reason: dead\nearlier-branch: crew/a@h/5-x\n",
            kind: Kind::Info,
            act: Act::Reassigned,
        },
    ]);
    assert_eq!(
        earlier_work_of(&reassigned_body_with("dead", Some(&pr))),
        Some(pr)
    );
    assert_eq!(
        earlier_work_of(&reassigned_body_with("dead", Some(&branch))),
        Some(branch)
    );
}

#[test]
fn golden_close_notices() {
    check(vec![
        Golden {
            name: "close --verified",
            text: closed_verified_body(),
            bytes: "Closed after a passed verify.",
            kind: Kind::Info,
            act: Act::Closed(CloseKind::Verified),
        },
        Golden {
            name: "merge notice",
            text: merged_body(
                "https://github.com/o/r/pull/1",
                "deadbeef",
                "squash",
                "abc123",
                "crew/x",
                None,
            ),
            bytes: "Merged by the coordinator.\npr: https://github.com/o/r/pull/1\nsha: deadbeef\nmethod: squash\nmerge-commit: abc123\nbranch: crew/x",
            kind: Kind::Info,
            act: Act::Closed(CloseKind::Merged),
        },
        Golden {
            name: "merge notice of a local gate",
            text: merged_body(
                "https://github.com/o/r/pull/1",
                "deadbeef",
                "rebase",
                "abc123",
                "crew/x",
                Some(("host-a: make test rc=0", "host-c: make test rc=0")),
            ),
            bytes: "Merged by the coordinator.\npr: https://github.com/o/r/pull/1\nsha: deadbeef\nmethod: rebase\nmerge-commit: abc123\nbranch: crew/x\nci-mode: local\nlocal-gate: host-a: make test rc=0\ncoordinator-local-gate: host-c: make test rc=0",
            kind: Kind::Info,
            act: Act::Closed(CloseKind::Merged),
        },
        Golden {
            name: "owner merge notice",
            text: owner_merged_body(PR, Some("abc"), None),
            bytes: "The ticket is closed: the owner merged the PR by hand.\npr: https://github.com/o/r/pull/9\nmerge-commit: abc\nmerged by the owner\nclosed: owner merged https://github.com/o/r/pull/9\n",
            kind: Kind::Info,
            act: Act::Closed(CloseKind::OwnerMerged),
        },
        Golden {
            name: "owner merge notice with a reason and no merge commit",
            text: owner_merged_body(PR, None, Some("why")),
            bytes: "The ticket is closed: the owner merged the PR by hand.\npr: https://github.com/o/r/pull/9\nmerge-commit: unknown\nmerged by the owner\nclosed: owner merged https://github.com/o/r/pull/9\nreason: why\n",
            kind: Kind::Info,
            act: Act::Closed(CloseKind::OwnerMerged),
        },
        Golden {
            name: "drop",
            text: dropped_body("superseded by o/r#12"),
            bytes: "dropped: superseded by o/r#12\n",
            kind: Kind::Info,
            act: Act::Closed(CloseKind::Dropped),
        },
    ]);
}

#[test]
fn golden_verify_result() {
    assert_eq!(verify_head(true), "verify: pass");
    assert_eq!(verify_head(false), "verify: fail");
    check(vec![
        Golden {
            name: "verify pass",
            text: verify_body(true, &evidence(), "abc", &[], "", None, None),
            bytes: "verify: pass\npr: https://github.com/o/r/pull/9\nsha: abc\nhead_now: abc\nci: success\n",
            kind: Kind::Report,
            act: Act::Verify { pass: true },
        },
        Golden {
            name: "verify fail with every line",
            text: verify_body(
                false,
                &local_evidence(),
                "def",
                &["ci red".into(), "head moved".into()],
                " n \n",
                Some("c"),
                Some("host-c: make test rc=0"),
            ),
            bytes: "verify: fail\npr: https://github.com/o/r/pull/9\nsha: abc\nhead_now: def\nci: local-gate\nci-sources: workflow-runs, statuses\nci-runs: counted: 9 (push, success)\nlocal-gate: host-a: make test rc=0, log /tmp/gate.log\ncoordinator-local-gate: host-c: make test rc=0\nfailed: ci red\nfailed: head moved\ncase: c\nnote: n\n",
            kind: Kind::Report,
            act: Act::Verify { pass: false },
        },
    ]);
}

#[test]
fn golden_done_evidence() {
    check(vec![
        Golden {
            name: "done",
            text: done_body(&evidence(), "note: shipped\n"),
            bytes: "Done. Evidence follows.\npr: https://github.com/o/r/pull/9\nsha: abc\nci: success\nnote: shipped\n",
            kind: Kind::Done,
            act: Act::Other,
        },
        Golden {
            name: "done of a local gate",
            text: done_body(&local_evidence(), ""),
            bytes: "Done. Evidence follows.\npr: https://github.com/o/r/pull/9\nsha: abc\nci: local-gate\nci-sources: workflow-runs, statuses\nci-runs: counted: 9 (push, success)\nlocal-gate: host-a: make test rc=0, log /tmp/gate.log\nCI off by owner decision (AUTHORITY.md ci_mode: local)\n",
            kind: Kind::Done,
            act: Act::Other,
        },
    ]);
    for ev in [evidence(), local_evidence()] {
        assert_eq!(parse_evidence(&done_body(&ev, "")), Some(ev.clone()));
    }
}

#[test]
fn golden_question_park_and_lifecycle() {
    check(vec![
        Golden {
            name: "answer that takes a question back",
            text: withdraw_body("reassigned: new agent asks again"),
            bytes: "reassigned: new agent asks again\nwithdraws: question\n",
            kind: Kind::Answer,
            act: Act::WithdrawsQuestion,
        },
        Golden {
            name: "park",
            text: park_body(49, " P0 first "),
            bytes: "parked for #49: P0 first",
            kind: Kind::Blocked,
            act: Act::Parked { for_issue: 49 },
        },
        Golden {
            name: "left marker",
            text: left_body("claude@a"),
            bytes: "claude@a left\nlifecycle: left",
            kind: Kind::Info,
            act: Act::Left,
        },
        Golden {
            name: "stopping notice",
            text: stopping_body("claude@a", "o/r#1 (Ready), o/r#2 (Blocked)"),
            bytes: "claude@a is stopping (draining). Open tickets that the coordinator returns to Backlog: o/r#1 (Ready), o/r#2 (Blocked)",
            kind: Kind::Info,
            act: Act::Stopping {
                name: "claude@a".into(),
            },
        },
    ]);
}
