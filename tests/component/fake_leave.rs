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
