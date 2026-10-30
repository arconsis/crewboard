use super::*;
use crate::protocol::acts::ci_line;
use crate::protocol::names::Kind;
use crate::rules::Role;

fn msg(id: u64, sender: Sender, kind: Kind, body: &str) -> VMsg {
    crate::testkit::msg(id, sender, "dev@host", Some(kind), body)
}

fn coordinator() -> Sender {
    Sender::Agent {
        name: "coord@host".into(),
        role: Role::Coordinator,
    }
}

fn dev() -> Sender {
    Sender::Agent {
        name: "dev@host".into(),
        role: Role::Dev,
    }
}

fn assigned(id: u64, gate: &str) -> VMsg {
    msg(
        id,
        coordinator(),
        Kind::Request,
        &format!("Assigned to you: T\nticket: o/r#1\nitem=x\ngate: {gate}\n"),
    )
}

#[test]
fn the_latest_coordinator_gate_wins() {
    let msgs = [assigned(1, "cargo test"), assigned(2, "CI green")];
    assert_eq!(signed_gate(&msgs).as_deref(), Some("CI green"));
}

#[test]
fn a_gate_line_of_a_dev_an_owner_or_another_kind_is_ignored() {
    let msgs = [
        assigned(1, "cargo test"),
        msg(2, dev(), Kind::Request, "x\ngate: CI green\n"),
        msg(3, Sender::Owner, Kind::Request, "gate: CI green"),
        msg(4, coordinator(), Kind::Info, "gate: CI green"),
    ];
    assert_eq!(signed_gate(&msgs).as_deref(), Some("cargo test"));
}

#[test]
fn no_assignment_gives_no_gate_and_a_later_message_without_the_line_keeps_the_old_one() {
    assert_eq!(signed_gate(&[]), None);
    let reassign_by_an_old_binary = msg(
        2,
        coordinator(),
        Kind::Request,
        "Assigned to you: T\nticket: o/r#1\nitem=x\nreason: r\n",
    );
    let msgs = [assigned(1, "CI green"), reassign_by_an_old_binary];
    assert_eq!(signed_gate(&msgs).as_deref(), Some("CI green"));
}

#[test]
fn the_signed_line_holds_every_gate_line_of_the_body() {
    assert_eq!(
        gate_line_of("Goal\nGate: CI green\n").as_deref(),
        Some("CI green")
    );
    assert_eq!(
        gate_line_of("Gate: CI green\n- **Gate:** demo\n").as_deref(),
        Some("CI green | demo")
    );
    assert_eq!(gate_line_of("Goal\nGate:\n"), None);
}
fn assigned_ci(id: u64, ci: &str) -> VMsg {
    msg(
        id,
        coordinator(),
        Kind::Request,
        &format!("Assigned to you: T\nticket: o/r#1\nitem=x\ngate: CI green\n{ci}"),
    )
}

#[test]
fn the_signed_ci_line_decides_and_the_body_is_the_fallback_for_an_old_assignment() {
    // signed `ci: none`: declared, whatever the body says
    let none = [assigned_ci(1, "ci: none\n")];
    assert_eq!(signed_ci_none(&none), Some(true));
    assert!(ci_none_declared(&none, "Gate: x\n"));
    // signed `ci: required`: not declared, also when the body was edited to say `CI: none`
    let req = [assigned_ci(1, "ci: required\n")];
    assert_eq!(signed_ci_none(&req), Some(false));
    assert!(!ci_none_declared(&req, "Gate: x\nCI: none\n"));
    // an old assignment (no ci line) and no assignment: the live body decides
    let old = [assigned_ci(1, "")];
    assert_eq!(signed_ci_none(&old), None);
    assert!(ci_none_declared(&old, "CI: none\n"));
    assert!(!ci_none_declared(&old, "Gate: x\n"));
    assert!(ci_none_declared(&[], "CI: none\n"));
    // a `ci:` line of a dev, or an unknown value, is no signed line
    let dev_line = [msg(2, dev(), Kind::Request, "gate: CI green\nci: none\n")];
    assert_eq!(signed_ci_none(&dev_line), None);
    assert_eq!(signed_ci_none(&[assigned_ci(3, "ci: maybe\n")]), None);
    // the latest assignment with a gate line wins
    let both = [
        assigned_ci(1, "ci: none\n"),
        assigned_ci(2, "ci: required\n"),
    ];
    assert_eq!(signed_ci_none(&both), Some(false));
}

#[test]
fn the_ci_line_is_what_assign_and_reassign_write() {
    assert_eq!(ci_line(true), "ci: none\n");
    assert_eq!(ci_line(false), "ci: required\n");
    let w = [assigned_ci(1, ci_line(true))];
    assert_eq!(signed_ci_none(&w), Some(true));
}
