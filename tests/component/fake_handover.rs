//! Trust is replayed per message (spec/identity#trust-at-signing-time), end to end against the
//! fake GitHub. After a handover (`/crew role old dev`, `/crew role new coordinator`) the old
//! coordinator's earlier messages still count as coordinator messages: a host with an empty cache
//! finds its tickets, and its answer to a block still unblocks. A message that the old
//! coordinator signs after the handover is a dev message.

use crate::support::board::{FakeBoard, REGISTRY};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok};
use crewboard::rules::Role;
use serde_json::Value;

#[test]
fn after_a_handover_the_old_coordinators_tickets_and_answers_still_count() {
    let (fake, old, b) = crew();
    let new = fake.agent("claude@fake-n", Role::Dev);
    let t = fake.ticket_issue(
        "IT handover",
        &format!("Goal: handover test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    // the old coordinator assigns; B takes, blocks, and gets an answer of the coordinator
    let item = item_in(&run_ok(&fake, &old, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["block", &ts, "--why", "need a decision"]);
    let _ = fake.poll_json(&old);
    run_ok(
        &fake,
        &old,
        &[
            "say",
            &ts,
            "--to",
            &b.name,
            "--kind",
            "answer",
            "--body",
            "use the old API",
        ],
    );

    // the handover: the owner makes the old coordinator a dev, then the new one the coordinator
    fake.owner_comment(REGISTRY, &format!("/crew role {} dev", old.name));
    fake.owner_comment(REGISTRY, &format!("/crew role {} coordinator", new.name));

    // the answer of the old coordinator, signed as the coordinator, still unblocks the ticket
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );

    // the new coordinator has no cache: it finds the ticket through the old coordinator's info
    let o = run_ok(&fake, &new, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());

    // nothing of the history is a security event
    let o = run_ok(&fake, &new, &["status", "--json"]);
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["security_event_count"], 0, "{v}");
    no_unhandled(&fake);
}

#[test]
fn a_message_of_the_old_coordinator_after_the_handover_is_a_dev_message() {
    let (fake, old, b) = crew();
    let new = fake.agent("claude@fake-n", Role::Dev);
    fake.owner_comment(REGISTRY, &format!("/crew role {} dev", old.name));
    fake.owner_comment(REGISTRY, &format!("/crew role {} coordinator", new.name));
    // after the handover the old coordinator cannot assign: its present role is dev
    let t = fake.ticket_issue(
        "IT after handover",
        &format!("Goal: x.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let o = fake.run(&old.dir, &["assign", &t.to_string(), "--to", &b.name]);
    assert_eq!(o.code, 2, "{}", o.all());
    // control: the new coordinator can
    run_ok(&fake, &new, &["assign", &t.to_string(), "--to", &b.name]);
    no_unhandled(&fake);
}

#[test]
fn a_new_board_without_a_handover_is_unchanged() {
    // control for the two tests above: nothing in the history is a security event
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let o = run_ok(&fake, &c, &["status", "--json"]);
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["security_event_count"], 0, "{v}");
}
