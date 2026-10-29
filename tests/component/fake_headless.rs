//! The headless coordinator against a fake GitHub (spec/agents#headless-coordinator).
//!
//! `verify` and `merge` of a CI-only ticket pass; everything else that writes is refused with
//! `headless-scope` before the first request; the interactive coordinator is not restricted.
//! The fake (ticket, PR, CI, AUTHORITY.md, merge call) is `tests/support/headless.rs`.

use crate::support::headless::{Hub, Scenario, TICKET};

const CI_ONLY: &str = "Goal: fix it\nGate: ci\n";
const BY_HAND: &str = "Goal: fix it\nGate: `cargo test` passes and the demo runs\n";

fn hub(body: &'static str, may_merge: bool) -> Hub {
    Hub::new(Scenario {
        body,
        may_merge,
        ci_green: true,
    })
}

fn t() -> String {
    TICKET.to_string()
}

#[test]
fn headless_verify_and_merge_of_a_ci_only_ticket_pass() {
    let h = hub(CI_ONLY, true);
    let v = h.headless(&["verify", &t()]);
    assert_eq!(v.code, 0, "{}", v.all());
    assert!(v.stdout.contains("verify PASS"), "{}", v.all());
    assert!(!h.merged());
    let m = h.headless(&["merge", &t()]);
    assert_eq!(m.code, 0, "{}", m.all());
    assert!(m.stdout.contains("merged"), "{}", m.all());
    assert!(h.merged());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Done"));
    assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());
}

#[test]
fn headless_merge_is_refused_when_authority_does_not_allow_it() {
    let h = hub(CI_ONLY, false);
    let v = h.headless(&["verify", &t()]);
    assert_eq!(v.code, 0, "{}", v.all());
    let writes = h.writes().len();
    let m = h.headless(&["merge", &t()]);
    assert_eq!(m.code, 2, "{}", m.all());
    assert!(
        m.stderr.contains("authority-merge-not-allowed"),
        "{}",
        m.all()
    );
    assert!(!h.merged());
    assert_eq!(
        h.writes().len(),
        writes,
        "the refused merge wrote something"
    );
    // control: the same ticket merges when AUTHORITY.md allows it
    let ok = hub(CI_ONLY, true);
    ok.headless(&["verify", &t()]);
    assert_eq!(ok.headless(&["merge", &t()]).code, 0);
}

#[test]
fn headless_merge_is_refused_for_a_gate_that_is_not_ci_only() {
    let h = hub(BY_HAND, true);
    let v = h.headless(&["verify", &t()]);
    assert_eq!(v.code, 0, "verify has no Gate limit: {}", v.all());
    let writes = h.writes().len();
    let m = h.headless(&["merge", &t()]);
    assert_eq!(m.code, 2, "{}", m.all());
    assert!(m.stderr.contains("[rule=headless-scope]"), "{}", m.all());
    assert!(m.stderr.contains("CI only"), "{}", m.all());
    assert!(!h.merged());
    assert_eq!(
        h.writes().len(),
        writes,
        "the refused merge wrote something"
    );
    assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
    // control: the interactive coordinator merges the same ticket (no scope there)
    let i = h.interactive(&["merge", &t()]);
    assert_eq!(i.code, 0, "{}", i.all());
    assert!(h.merged());
}

#[test]
fn headless_refuses_every_out_of_scope_command_before_the_first_request() {
    let h = hub(CI_ONLY, true);
    let body = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(body.path(), "Goal: x\nGate: ci\n").unwrap();
    let body = body.path().to_str().unwrap().to_string();
    let cases: Vec<Vec<&str>> = vec![
        vec!["assign", "5", "--to", "hermes@fake-b"],
        vec!["reassign", "5", "--reason", "late", "--why", "x"],
        vec!["ticket", "new", "--title", "t", "--body-file", &body],
        vec!["ticket", "adopt", "5"],
        vec!["ticket", "split", "5", "--title", "t", "--body-file", &body],
        vec!["say", "5", "--to", "owner", "--kind", "info", "--body", "x"],
        vec!["ask", "5", "--body", "x"],
        vec!["unblock", "5"],
        vec!["close", "5", "--verified"],
        vec!["close", "5", "--why", "x"],
        vec!["verify", "5", "--run", "9002"],
        vec!["report-status", "--health", "on-track", "--body", "x"],
        vec!["prune"],
        vec!["stop", "--agent", "hermes@fake-b", "--force"],
    ];
    for argv in &cases {
        let o = h.headless(argv);
        assert_eq!(o.code, 2, "{argv:?}: {}", o.all());
        assert!(
            o.stderr.contains("[rule=headless-scope]"),
            "{argv:?}: {}",
            o.all()
        );
    }
    // nothing was sent, not even a read
    assert!(h.requests().is_empty(), "{:?}", h.requests());
    assert!(h.writes().is_empty());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
}

#[test]
fn the_interactive_coordinator_is_not_restricted() {
    let h = hub(CI_ONLY, true);
    // `say` is refused headless (above) and is a write here
    let o = h.interactive(&[
        "say",
        "5",
        "--to",
        "owner",
        "--kind",
        "info",
        "--body",
        "hello",
        "--override-unread",
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!h.writes().is_empty());
    // `reassign` of a Review ticket is a coordinator action: it reaches the rules, not the scope
    let o = h.interactive(&[
        "reassign",
        "5",
        "--reason",
        "late",
        "--why",
        "agent stopped",
    ]);
    assert!(!o.stderr.contains("headless-scope"), "{}", o.all());
}

/// The agent of the Review ticket works on another ticket.
fn busy_hub(ci_green: bool) -> Hub {
    let h = Hub::new(Scenario {
        body: CI_ONLY,
        may_merge: true,
        ci_green,
    });
    h.board.item(
        "PVTI_other",
        &[("Status", "In Progress"), ("Agent", "hermes@fake-b")],
    );
    h.board.issue(6, &["crew:work"]);
    h.board.signed(
        &h.coord.key,
        &h.coord.name,
        "owner",
        crewboard::protocol::names::Kind::Info,
        6,
        "Ticket is on the board.\nitem=PVTI_other\nproject=15\n",
    );
    h
}

#[test]
fn headless_failed_verify_of_a_busy_agent_queues_the_ticket_as_next() {
    let h = busy_hub(false);
    let v = h.headless(&["verify", &t()]);
    assert_eq!(v.code, 0, "{}", v.all());
    assert!(v.stdout.contains("verify FAIL"), "{}", v.all());
    // the headless verdict table allows the writes: Status Ready and the Order, same agent
    assert_eq!(
        h.status_of_ticket().as_deref(),
        Some("Ready"),
        "{}",
        v.all()
    );
    assert_eq!(
        h.board.item_field("PVTI_ticket", "Agent").as_deref(),
        Some("hermes@fake-b")
    );
    assert_eq!(
        h.board.item_field("PVTI_ticket", "Order").as_deref(),
        Some("0")
    );
    assert_eq!(
        h.board.item_field("PVTI_other", "Status").as_deref(),
        Some("In Progress")
    );
    assert!(v.stdout.contains("works on"), "{}", v.all());
    assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());
}

#[test]
fn headless_tick_follows_the_same_rule_and_an_idle_agent_gets_in_progress() {
    // tick of a busy agent: Ready (same rule as verify)
    let h = busy_hub(false);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        h.status_of_ticket().as_deref(),
        Some("Ready"),
        "{}",
        o.all()
    );
    // control: an idle agent, In Progress as before
    let idle = Hub::new(Scenario {
        body: CI_ONLY,
        may_merge: true,
        ci_green: false,
    });
    let o = idle.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        idle.status_of_ticket().as_deref(),
        Some("In Progress"),
        "{}",
        o.all()
    );
}

const LEFT: &str = "left for the interactive coordinator";

/// The ticket line of a `tick` that left the ticket.
fn left_line(o: &crate::support::board::Out) -> String {
    o.stdout
        .lines()
        .find(|l| l.contains(LEFT))
        .unwrap_or_else(|| panic!("no left line: {}", o.all()))
        .to_string()
}

#[test]
fn a_body_edit_after_the_assignment_does_not_make_the_gate_ci_only() {
    // signed gate: a command; the body is edited to `Gate: ci` afterwards
    let h = hub(BY_HAND, true);
    h.set_body("Goal: fix it\nGate: ci\n");
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        left_line(&o).contains("the Gate is not CI only"),
        "{}",
        o.all()
    );
    // the line names the typed Gate and the flag that writes it
    let l = left_line(&o);
    assert!(l.contains("a CI-only ticket has the Gate `ci`"), "{l}");
    assert!(l.contains("ticket new --gate-ci"), "{l}");
    assert!(
        l.contains("signed Gate: 'run the demo by hand'") || l.contains("signed Gate: '"),
        "{l}"
    );
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    assert!(!h.merged());
    // `merge` says which signed gate it read
    let m = h.headless(&["merge", &t()]);
    assert_eq!(m.code, 2, "{}", m.all());
    assert!(m.stderr.contains("[rule=headless-scope]"), "{}", m.all());
    assert!(
        m.stderr.contains("the signed gate of fake-owner/fake-repo#5 is '`cargo test` passes and the demo runs': it is not CI only"),
        "{}",
        m.all()
    );
    assert!(!h.merged());
}

#[test]
fn a_body_edit_to_a_command_does_not_stop_a_ticket_whose_signed_gate_is_ci_only() {
    let h = hub(CI_ONLY, true);
    h.set_body("Goal: fix it\nGate: `cargo test` passes and the demo runs\n");
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("merged"), "{}", o.all());
    assert!(h.merged());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Done"));
}

#[test]
fn the_typed_gate_ci_is_merged_by_tick_without_a_note() {
    let h = hub("Goal: fix it\nGate: ci\n", true);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("merged"), "{}", o.all());
    assert!(!o.stdout.contains("legacy phrase"), "{}", o.all());
    assert!(h.merged());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Done"));
}

#[test]
fn an_old_ci_phrase_is_not_ci_only_and_tick_leaves_the_ticket() {
    let h = hub("Goal: fix it\nGate: CI green\n", true);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("merged"), "{}", o.all());
    assert!(o.stdout.contains("the Gate is not CI only"), "{}", o.all());
    assert!(!h.merged());
}

#[test]
fn a_gate_that_adds_a_condition_to_ci_is_not_the_typed_gate() {
    // `ci` plus another Gate line is signed as `ci | <text>`: a text, never CI only
    let h = hub("Goal: fix it\nGate: ci\nGate: run the demo by hand\n", true);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        left_line(&o).contains("the Gate is not CI only"),
        "{}",
        o.all()
    );
    assert!(!h.merged());
}

#[test]
fn a_ticket_without_a_signed_gate_line_is_left_whatever_the_body_says() {
    // the assignment carried no gate line (an empty one is no line)
    let h = hub("Goal: fix it\n", true);
    h.set_body(CI_ONLY);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        left_line(&o).contains("no signed gate line: the Gate is not CI only"),
        "{}",
        o.all()
    );
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    let m = h.headless(&["merge", &t()]);
    assert_eq!(m.code, 2, "{}", m.all());
    assert!(
        m.stderr
            .contains("no signed gate line on fake-owner/fake-repo#5: the Gate is not CI only"),
        "{}",
        m.all()
    );
    assert!(!h.merged());
}

#[test]
fn a_later_assignment_without_a_gate_line_keeps_the_signed_gate_and_a_gate_of_a_dev_is_ignored() {
    let h = hub(CI_ONLY, true);
    // an assignment of an older binary after the first one: no gate line
    h.sign_assignment("reason: moved\n");
    // a dev message with a gate line is not the Gate
    h.board.signed(
        &h.dev.key,
        &h.dev.name,
        &h.coord.name,
        crewboard::protocol::names::Kind::Request,
        TICKET,
        "gate: run the demo by hand\n",
    );
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(h.merged(), "{}", o.all());
}

#[test]
fn a_ticket_that_declares_ci_none_in_the_body_is_not_ci_only() {
    let h = hub("Goal: fix it\nGate: ci\nCI: none\n", true);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        left_line(&o).contains("the Gate is not CI only"),
        "{}",
        o.all()
    );
    assert!(!h.merged());
}

#[test]
fn the_tick_leaves_a_ticket_whose_done_names_a_protected_path_without_any_read() {
    let h = hub(CI_ONLY, true);
    h.sign_done("protected-paths: .github/workflows/ci.yml\n");
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        left_line(&o).contains("protected path: the owner merges this PR"),
        "{}",
        o.all()
    );
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    // no read of the file list or of the CI was needed
    assert!(
        h.requests()
            .iter()
            .all(|r| !r.path.contains("/files") && !r.path.contains("/actions/")),
        "{:?}",
        h.requests().iter().map(|r| &r.path).collect::<Vec<_>>()
    );
    assert!(!h.merged());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
}

#[test]
fn the_tick_leaves_a_pr_that_changes_a_path_of_protected_paths() {
    // AUTHORITY.md adds `tests/**`; the `done` has no protected-paths line, so verify refuses
    let h = hub(CI_ONLY, true);
    h.set_authority_extra("protected_paths: [tests/**]\n");
    h.set_files(&["src/it.rs", "tests/ci.toml"]);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let l = left_line(&o);
    assert!(l.contains("protected-path-owner-merge"), "{l}");
    assert!(l.contains("tests/ci.toml"), "{l}");
    assert!(!h.merged());
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    // control: the same PR merges when the list does not name tests/**
    let ok = hub(CI_ONLY, true);
    ok.set_authority_extra("protected_paths: [scripts/**]\n");
    ok.set_files(&["src/it.rs", "tests/ci.toml"]);
    ok.headless(&["tick"]);
    assert!(ok.merged());
}

#[test]
fn the_tick_leaves_a_pr_that_changes_a_protected_path_now_and_merges_nothing() {
    // the `done` has no protected-paths line (the PR changed after it): verify refuses
    let h = hub(CI_ONLY, true);
    h.set_files(&["src/it.rs", ".github/workflows/ci.yml"]);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let l = left_line(&o);
    assert!(l.contains("protected-path-owner-merge"), "{l}");
    assert!(!h.merged());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    // the headless merge refuses the same PR, with the same rule
    let m = h.headless(&["merge", &t()]);
    assert_eq!(m.code, 2, "{}", m.all());
    assert!(
        m.stderr.contains("protected-path-owner-merge"),
        "{}",
        m.all()
    );
    assert!(!h.merged());
    // control: the same ticket and PR without the workflow file merges
    let ok = hub(CI_ONLY, true);
    ok.set_files(&["src/it.rs", "docs/AUTHORITY.md"]);
    let o = ok.headless(&["tick"]);
    assert!(ok.merged(), "{}", o.all());
}

#[test]
fn the_signed_ci_line_decides_whether_a_ticket_declares_ci_none_and_an_old_ticket_uses_the_body() {
    // an old ticket (the assignment has a gate line and no `ci:` line): the live body decides
    let old = hub(CI_ONLY, true);
    old.set_body("Goal: fix it\nGate: ci\nCI: none\n");
    let o = old.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        left_line(&o).contains("the Gate is not CI only"),
        "{}",
        o.all()
    );
    assert!(!old.merged());

    // a new ticket signed `ci: required`: a body edit that adds `CI: none` changes nothing
    let new = hub(CI_ONLY, true);
    new.sign_assignment("gate: ci\nci: required\n");
    new.set_body("Goal: fix it\nGate: ci\nCI: none\n");
    let o = new.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(new.merged(), "{}", o.all());

    // a new ticket signed `ci: none`: the body without the line changes nothing either
    let none = hub(CI_ONLY, true);
    none.sign_assignment("gate: ci\nci: none\n");
    let o = none.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        left_line(&o).contains("the Gate is not CI only"),
        "{}",
        o.all()
    );
    assert!(!none.merged());
    let m = none.headless(&["merge", &t()]);
    assert_eq!(m.code, 2, "{}", m.all());
    assert!(m.stderr.contains("declares `CI: none`"), "{}", m.all());
}

#[test]
fn the_guard_refuses_a_write_command_in_a_headless_dir_and_lets_verify_pass() {
    let h = hub(CI_ONLY, true);
    let o = h.headless(&["assign", "5", "--to", "hermes@fake-b"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("[rule=headless-scope]") && o.stderr.contains("this config is headless"),
        "{}",
        o.all()
    );
    assert!(h.requests().is_empty(), "{:?}", h.requests());
    // the same dir still runs the allowed command
    let v = h.headless(&["verify", &t()]);
    assert_eq!(v.code, 0, "{}", v.all());
}

#[test]
fn the_tick_outside_a_headless_dir_gives_tick_headless_only() {
    let h = hub(CI_ONLY, true);
    let o = h.interactive(&["tick"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("[rule=tick-headless-only]"),
        "{}",
        o.all()
    );
    assert!(h.writes().is_empty());
}

#[test]
fn headless_may_print_the_ticket_template_and_no_other_ticket_new() {
    let h = hub(CI_ONLY, true);
    // the template reads nothing and writes nothing: allowed, exit 0, no request
    let o = h.headless(&["ticket", "new", "--template"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.starts_with("Goal: ") && o.stdout.contains("\nGate: "),
        "{}",
        o.stdout
    );
    assert!(h.requests().is_empty(), "{:?}", h.requests());
    // `ticket new` with a title stays a write: refused
    let o = h.headless(&["ticket", "new", "--title", "t", "--body-file", "-"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("[rule=headless-scope]"), "{}", o.all());
}
