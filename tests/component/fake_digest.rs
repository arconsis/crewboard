//! The daily digest of the headless `tick` (spec/agents#headless-coordinator, 8a), against the
//! fake GitHub: one project status update per day when `daily_digest` of AUTHORITY.md is on,
//! with facts of the tick and a health value by rule.

use crate::support::authority::{NEEDS_OWNER_ALL, authority_with};
use crate::support::board::{Agent, FakeBoard, Out};
use crewboard::clock::{format_ts, now_epoch};
use crewboard::rules::Role;

struct Case {
    fake: FakeBoard,
    dev: Agent,
    headless: tempfile::TempDir,
}

/// A board with one ticket In Progress and a headless copy of the coordinator.
fn case(extra: &str) -> Case {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let dev = fake.agent("hermes@fake-b", Role::Dev);
    fake.file(
        "AUTHORITY.md",
        &authority_with("fake-owner", NEEDS_OWNER_ALL, extra),
    );
    let t = fake
        .ticket_issue("IT digest", "Goal: x.\n\nGate: ci\n", &["crew:work"])
        .to_string();
    assert_eq!(fake.run(&c.dir, &["assign", &t, "--to", &dev.name]).code, 0);
    assert_eq!(fake.run(&dev.dir, &["take", &t]).code, 0);
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    Case {
        fake,
        dev,
        headless,
    }
}

fn tick(k: &Case) -> Out {
    let o = k.fake.run(k.headless.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    o
}

#[test]
fn the_tick_posts_one_digest_per_day_with_the_counts_and_the_health() {
    let k = case("daily_digest: true\n");
    let o = tick(&k);
    assert!(
        o.stdout.contains("digest: posted (ON_TRACK)"),
        "{}",
        o.all()
    );
    let ups = k.fake.status_updates();
    assert_eq!(ups.len(), 1, "{ups:?}");
    let (_, status, body) = &ups[0];
    assert_eq!(status, "ON_TRACK");
    assert!(body.starts_with("from=codex@fake-c\n"), "{body}");
    assert!(
        body.contains("written by `crewboard tick`, no model"),
        "{body}"
    );
    assert!(body.contains("Tickets: In Progress 1"), "{body}");
    assert!(body.contains("Agents: dead none; late none"), "{body}");
    assert!(
        body.contains("In Review and waiting for a person: 0"),
        "{body}"
    );
    // the same day: no second update, and no line
    let o = tick(&k);
    assert!(!o.stdout.contains("digest"), "{}", o.all());
    assert_eq!(k.fake.status_updates().len(), 1);
}

#[test]
fn a_dead_agent_makes_the_digest_off_track_and_names_it() {
    let k = case("daily_digest: true\n");
    k.fake
        .set_item(&k.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 7_200));
    let o = tick(&k);
    assert!(
        o.stdout.contains("digest: posted (OFF_TRACK)"),
        "{}",
        o.all()
    );
    let ups = k.fake.status_updates();
    assert_eq!(ups[0].1, "OFF_TRACK");
    assert!(
        ups[0].2.contains("Agents: dead hermes@fake-b; late none"),
        "{}",
        ups[0].2
    );
}

#[test]
fn without_the_key_the_tick_posts_no_digest() {
    for extra in ["", "daily_digest: false\n"] {
        let k = case(extra);
        let o = tick(&k);
        assert!(!o.stdout.contains("digest"), "{}", o.all());
        assert!(k.fake.status_updates().is_empty());
    }
}
