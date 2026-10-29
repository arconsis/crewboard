//! The label `needs-owner` (spec/tickets#agent-labels), against the fake GitHub of
//! `tests/support/board.rs`.
//!
//! It follows the question to the owner: `ask` sets it, `take` and `close` remove it once no
//! question is open. A label that goes stale outside crewboard is the job of the coordinator duty
//! (`fake_needs_owner_sync.rs`).

use crate::support::board::FakeBoard;
use crate::support::cli::run_ok;
use crewboard::rules::Role;

const GATE: &str = "Goal: a ticket.\n\n**Gate:** the PR of this ticket is open and closes it.\n";

fn needs_owner(fake: &FakeBoard, t: u64) -> bool {
    fake.labels_of(t).contains(&"needs-owner".to_string())
}

/// The number of `DELETE .../labels/needs-owner` requests so far.
fn label_deletes(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.method == "DELETE" && r.path.ends_with("/labels/needs-owner"))
        .count()
}

#[test]
fn the_label_follows_ask_take_and_close() {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let body = format!("{GATE}CI: none\n");
    let t = fake.ticket_issue("IT label follows", &body, &["crew:work"]);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let item = fake.item_of_issue(t).expect("ticket item");
    run_ok(&fake, &b, &["take", &ts]);
    // a take on a Ready ticket makes no label call
    assert_eq!(label_deletes(&fake), 0);
    // ask: the label is set and the In Progress ticket is Blocked
    run_ok(&fake, &b, &["ask", &ts, "--body", "Which layout?"]);
    assert!(needs_owner(&fake, t));
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Blocked"));
    // the owner answers on the ticket
    fake.owner_comment_now(t, "The second one.");
    let _ = fake.poll_json(&b);
    // take: the owner answered, so the label goes
    run_ok(&fake, &b, &["take", &ts]);
    assert!(!needs_owner(&fake, t), "{:?}", fake.labels_of(t));
    assert_eq!(label_deletes(&fake), 1);
    // deliver, ask again, and close by judgment with the question open: the label goes
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    run_ok(&fake, &b, &["ask", &ts, "--body", "Release go?"]);
    assert!(needs_owner(&fake, t));
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Review"));
    let _ = fake.poll_json(&c);
    run_ok(&fake, &c, &["close", &ts, "--why", "the owner decided"]);
    assert!(!needs_owner(&fake, t), "{:?}", fake.labels_of(t));
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Dropped"));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn ask_sets_the_label_before_a_failing_status_write() {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let body = format!("{GATE}CI: none\n");
    let t = fake.ticket_issue("IT label first", &body, &["crew:work"]);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let _ = fake.poll_json(&b);
    // the Status write of ask (In Progress -> Blocked) fails after the question is posted
    fake.fail_select_write(Some("Blocked"));
    let out = fake.run(&b.dir, &["ask", &ts, "--body", "Which layout?"]);
    assert_ne!(
        out.code,
        0,
        "ask must report the failed write: {}",
        out.all()
    );
    fake.fail_select_write(None);
    // the question is posted and the label is set, so the board view finds the ticket
    assert!(needs_owner(&fake, t), "{:?}", fake.labels_of(t));
    let posted = fake
        .comments_of(t)
        .iter()
        .filter(|c| c.2.contains("Which layout?"))
        .count();
    assert_eq!(posted, 1);
}
