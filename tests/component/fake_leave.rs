//! The marker `lifecycle: left` (spec/identity#trust-at-signing-time), end to end against the
//! fake GitHub. An agent that leaves signs one `info` in the registry issue. Its old messages
//! are no security events in `status`; a message that it signs after the marker is untrusted.
//! `/crew stop` alone (draining) does not end the span.

use crate::support::board::{FakeBoard, INITIATIVE, REGISTRY, THREAD};
use crate::support::cli::{crew, no_unhandled, run_ok};
use crewboard::protocol::names::Kind;
use crewboard::rules::Role;
use serde_json::Value;

fn security(fake: &FakeBoard, who: &crate::support::board::Agent) -> Vec<(u64, String)> {
    let o = run_ok(fake, who, &["status", "--json"]);
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    v["security_event_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["comment_id"].as_u64().unwrap(),
                e["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn markers(fake: &FakeBoard) -> usize {
    fake.comments_of(REGISTRY)
        .iter()
        .filter(|(_, _, b)| b.contains("kind=info") && b.contains("\nlifecycle: left"))
        .count()
}

#[test]
fn a_left_agent_posts_one_marker_its_history_is_no_event_and_a_later_message_is() {
    let (fake, c, _b) = crew();
    let d = fake.agent("claude@fake-d", Role::Dev);
    let (early, _) = fake.signed(&d.key, &d.name, &c.name, Kind::Report, THREAD, "before");
    assert_eq!(markers(&fake), 0);
    assert!(security(&fake, &c).is_empty());

    // no open ticket: `stop` drains and leaves in one run, with the marker first
    let o = run_ok(&fake, &d, &["stop"]);
    assert!(o.stdout.contains("left"), "{}", o.all());
    assert_eq!(markers(&fake), 1, "{:?}", fake.comments_of(REGISTRY));
    assert_eq!(
        fake.item_field(&d.item_id, "State").as_deref(),
        Some("left")
    );

    // history of the left agent, marker included: no security event
    assert!(security(&fake, &c).is_empty());
    // a message signed after the marker is untrusted
    let (late, _) = fake.signed(&d.key, &d.name, &c.name, Kind::Report, THREAD, "after");
    assert_eq!(
        security(&fake, &c),
        vec![(late, "unknown-sender".to_string())]
    );
    // the audit lists the late one and not the early one
    let owner = fake.owner_dir();
    let o = run_ok(
        &fake,
        &c,
        &["audit", "--since", "30m", "--initiative", INITIATIVE],
    );
    let _ = owner;
    assert!(
        o.stdout.contains(&format!("comment {late} by")),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains(&format!("comment {early} by")),
        "{}",
        o.stdout
    );
    no_unhandled(&fake);
}

#[test]
fn a_failed_marker_post_keeps_the_agent_draining_and_a_retry_leaves() {
    let (fake, _c, _b) = crew();
    let d = fake.agent("claude@fake-d", Role::Dev);
    fake.fail_once("POST", "/comments", "lifecycle: left");
    let o = fake.run(&d.dir, &["stop"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert_eq!(markers(&fake), 0);
    assert_eq!(
        fake.item_field(&d.item_id, "State").as_deref(),
        Some("draining"),
        "no marker, no left"
    );
    let o = run_ok(&fake, &d, &["stop"]);
    assert!(o.stdout.contains("left"), "{}", o.all());
    assert_eq!(markers(&fake), 1);
    assert_eq!(
        fake.item_field(&d.item_id, "State").as_deref(),
        Some("left")
    );
}

#[test]
fn stop_with_an_open_ticket_drains_without_a_marker_and_the_agent_still_speaks() {
    let (fake, c, b) = crew();
    let t = fake.ticket_issue(
        "IT draining",
        "Goal: x.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n",
        &["crew:work"],
    );
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    let o = run_ok(&fake, &b, &["stop"]);
    assert!(o.stdout.contains("draining"), "{}", o.all());
    assert_eq!(markers(&fake), 0, "draining is not leaving");
    // a draining agent speaks until it leaves: no security event
    let (_, _) = fake.signed(&b.key, &b.name, &c.name, Kind::Report, THREAD, "still here");
    assert!(security(&fake, &c).is_empty());
    no_unhandled(&fake);
}

#[test]
fn an_own_stop_is_in_the_log_so_the_coordinator_returns_the_tickets_and_the_marker_ends_the_loop() {
    let (fake, c, b) = crew();
    let t = fake.ticket_issue(
        "IT own stop",
        "Goal: x.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n",
        &["crew:work"],
    );
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    run_ok(&fake, &b, &["stop"]);
    // the signed notice is the stop in the log
    assert!(
        fake.comments_of(REGISTRY)
            .iter()
            .any(|(_, _, body)| body.contains("hermes@fake-b is stopping (draining).")),
        "{:?}",
        fake.comments_of(REGISTRY)
    );
    let (v, _) = fake.poll_json(&c);
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "agent-stopped"
                && e["body"].as_str().unwrap().contains("its signed notice")),
        "{v}"
    );
    let item = fake.item_of_issue(t).unwrap();
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Backlog"));
    // the agent's next cycle leaves with the marker; the cycle after that stops the loop
    let _ = fake.poll_json(&b);
    assert_eq!(markers(&fake), 1);
    let o = fake.run(&b.dir, &["poll"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("agent-left"), "{}", o.all());
    assert!(o.stderr.contains("the registry log ended"), "{}", o.all());
}

#[test]
fn a_hand_written_state_draining_neither_returns_the_tickets_nor_makes_the_agent_leave() {
    let (fake, c, b) = crew();
    let t = fake.ticket_issue(
        "IT hand draining",
        "Goal: x.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n",
        &["crew:work"],
    );
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    let item = fake.item_of_issue(t).unwrap();
    fake.set_item(&b.item_id, "State", "draining");
    fake.touch_item_ago(&b.item_id, 300);
    let _ = fake.poll_json(&c);
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Ready"));
    // an idle agent with the field alone does not end its own span
    fake.clear_item(&item, "Agent");
    let o = fake.run(&b.dir, &["poll"]);
    assert_ne!(o.code, 2, "{}", o.all());
    assert_eq!(markers(&fake), 0, "{}", o.all());
}
