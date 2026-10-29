//! The assignee is decided by the signed log, not by the board `Agent` field
//! (spec/tickets#assignment): an `Agent` written by hand gives no right to `take`, `done`,
//! `block` or `ask`; `assign` repairs it with a signed assignment.

use crate::support::board::FakeBoard;
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok, run_refused};
use crewboard::rules::Role;

fn ticket(fake: &FakeBoard, title: &str) -> u64 {
    fake.ticket_issue(
        title,
        &format!("Goal: signed assignee test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    )
}

#[test]
fn an_agent_field_set_by_hand_is_refused_until_assign_repairs_it() {
    let (fake, c, b) = crew();
    let d = fake.agent("claude@fake-d", Role::Dev);
    let t = ticket(&fake, "IT hand agent");
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    // the board says d; the log still says b
    fake.set_item(&item, "Agent", &d.name);
    for who in [&d, &b] {
        let o = run_refused(&fake, who, &["take", &ts], "ticket-agent-not-signed");
        assert!(
            o.stderr.contains(&format!("crewboard assign {ts} --to")),
            "{}",
            o.all()
        );
    }
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("Ready"),
        "a refused take writes nothing"
    );
    // the coordinator repairs it: a signed assignment to d follows
    run_ok(&fake, &c, &["assign", &ts, "--to", &d.name]);
    run_refused(&fake, &b, &["take", &ts], "status-wrong-writer");
    run_ok(&fake, &d, &["take", &ts]);
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    no_unhandled(&fake);
}

#[test]
fn a_hand_edit_after_take_blocks_block_and_ask() {
    let (fake, c, b) = crew();
    let d = fake.agent("claude@fake-d", Role::Dev);
    let t = ticket(&fake, "IT hand agent later");
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    fake.set_item(&item, "Agent", &d.name);
    run_refused(
        &fake,
        &d,
        &["block", &ts, "--why", "x"],
        "ticket-agent-not-signed",
    );
    run_refused(
        &fake,
        &d,
        &["ask", &ts, "--body", "why?"],
        "ticket-agent-not-signed",
    );
    run_refused(
        &fake,
        &b,
        &["block", &ts, "--why", "x"],
        "ticket-agent-not-signed",
    );
    // the field set back: b may act again
    fake.set_item(&item, "Agent", &b.name);
    run_ok(&fake, &b, &["block", &ts, "--why", "x"]);
    no_unhandled(&fake);
}

#[test]
fn a_normal_assign_and_a_reassign_let_the_signed_agent_take() {
    let (fake, c, b) = crew();
    let d = fake.agent("claude@fake-d", Role::Dev);
    let t = ticket(&fake, "IT signed assign");
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    // reassign to d: the new signed assignment decides
    run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &ts,
            "--to",
            &d.name,
            "--reason",
            "late",
            "--why",
            "it: other agent",
        ],
    );
    run_refused(&fake, &b, &["take", &ts], "status-wrong-writer");
    run_ok(&fake, &d, &["take", &ts]);
    no_unhandled(&fake);
}
