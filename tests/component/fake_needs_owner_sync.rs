//! The coordinator duty that keeps the label `needs-owner` true (spec/tickets#agent-labels),
//! against the fake GitHub of `tests/support/board.rs`.
//!
//! One REST list of the labelled issues per repo, at most every 5 minutes, in the real cycle of
//! the interactive coordinator only. A stale label goes; the label of a ticket with an open
//! question stays.

use crate::support::board::{Agent, FakeBoard};
use crate::support::cli::{GATE, crew, item_in, run_ok};
use crewboard::config::ConfigDir;
use crewboard::protocol::names::AgentName;
use crewboard::state::StateCache;

fn needs_owner(fake: &FakeBoard, t: u64) -> bool {
    fake.labels_of(t).contains(&"needs-owner".to_string())
}

/// The number of list requests for the label so far.
fn lists(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains("labels=needs-owner"))
        .count()
}

/// Forget when the duty ran last (the 5 minute throttle).
fn lift_throttle(who: &Agent) {
    let cd = ConfigDir(who.dir.clone());
    let p = cd.state_path("fake", &AgentName::parse(&who.name).unwrap());
    let mut c = StateCache::load(&p);
    c.owner.needs_owner_checked = None;
    c.save(&p).unwrap();
}

/// A delivered ticket (Review).
fn delivered(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> (u64, String) {
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue(title, &body, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
    (t, item)
}

#[test]
fn a_stale_label_goes_one_list_per_repo_and_the_throttle_holds() {
    let (fake, c, b) = crew();
    let (t, item) = delivered(&fake, &c, &b, "IT sync stale");
    // a Done ticket with a stale label (an old run, or a change outside crewboard)
    fake.set_item(&item, "Status", "Done");
    fake.touch_item(&item);
    fake.add_label(t, "needs-owner");
    let (_, code) = fake.poll_json(&c);
    assert!(code == 0 || code == 1, "{code}");
    assert!(!needs_owner(&fake, t), "{:?}", fake.labels_of(t));
    assert_eq!(lists(&fake), 1);
    // the second cycle within 5 minutes sends no list
    fake.add_label(t, "needs-owner");
    let _ = fake.poll_json(&c);
    assert_eq!(lists(&fake), 1);
    assert!(needs_owner(&fake, t));
    // after the throttle the duty runs again
    lift_throttle(&c);
    let _ = fake.poll_json(&c);
    assert_eq!(lists(&fake), 2);
    assert!(!needs_owner(&fake, t));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_ticket_with_an_open_question_keeps_the_label_and_a_closed_question_loses_it() {
    let (fake, c, b) = crew();
    let (t, _item) = delivered(&fake, &c, &b, "IT sync open");
    run_ok(&fake, &b, &["ask", &t.to_string(), "--body", "Release go?"]);
    assert!(needs_owner(&fake, t));
    let _ = fake.poll_json(&c);
    assert_eq!(lists(&fake), 1);
    assert!(needs_owner(&fake, t), "the question is open");
    // the owner answers on the ticket; the label is still on it (nobody ran a command)
    fake.owner_comment_now(t, "Go.");
    lift_throttle(&c);
    let _ = fake.poll_json(&c);
    assert!(!needs_owner(&fake, t), "{:?}", fake.labels_of(t));
}

#[test]
fn a_quiet_poll_and_a_headless_cycle_send_no_list() {
    let (fake, c, b) = crew();
    let (t, _item) = delivered(&fake, &c, &b, "IT sync quiet");
    fake.add_label(t, "needs-owner");
    let _ = fake.run(&c.dir, &["poll", "--quiet"]);
    assert_eq!(lists(&fake), 0);
    let h = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, h.path());
    let _ = fake.run(h.path(), &["poll", "--json"]);
    assert_eq!(lists(&fake), 0);
    assert!(needs_owner(&fake, t));
}
