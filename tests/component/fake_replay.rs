//! The replay window: a signed message is fresh only if its `ts` is inside
//! `[created_at - 15 min, created_at + 5 min]` (spec/messages#signature). A copy of an old
//! valid message gets a new `created_at` and keeps its old `ts`.

use crate::support::board::{Agent, FakeBoard, OWNER, THREAD};
use crate::support::cli::{crew, run_ok};
use crewboard::protocol::names::Kind;
use serde_json::Value;

fn ticket(fake: &FakeBoard, title: &str) -> u64 {
    let body = "Goal: integration test ticket.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n";
    fake.ticket_issue(title, body, &["crew:work"])
}

fn mine(fake: &FakeBoard, who: &Agent) -> (i32, String) {
    let o = fake.run(&who.dir, &["mine"]);
    (o.code, o.stdout)
}

fn reasons(v: &Value, id: u64) -> Vec<String> {
    v["security"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["comment_id"].as_u64() == Some(id))
        .map(|e| e["reason"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_replayed_copy_of_an_old_coordinator_answer_does_not_unblock_a_ticket() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT replay");
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["block", &ts, "--why", "waits for a decision"]);
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Blocked, no answer yet"), "{out}");

    // somebody posts a day-old valid answer of the coordinator again
    let day_old = crewboard::clock::now_epoch() - 86_400;
    let copy = fake.signed_with_ts(
        &c.key,
        &c.name,
        &b.name,
        Kind::Answer,
        t,
        "go ahead",
        day_old,
    );
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "the copy unblocked the ticket: {out}");
    assert!(out.contains("Blocked, no answer yet"), "{out}");
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    assert_eq!(reasons(&v, copy), vec!["replayed"], "{v}");
    assert!(
        v["security"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("before the comment"),
        "{v}"
    );

    // control: a fresh answer of the same coordinator does unblock it
    let (fresh, _) = fake.signed(&c.key, &c.name, &b.name, Kind::Answer, t, "go ahead, now");
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&format!("crewboard take {t}")), "{out}");
    let (v, _) = fake.poll_json(&b);
    assert!(reasons(&v, fresh).is_empty(), "{v}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_message_signed_more_than_a_minute_after_its_comment_is_refused_too() {
    let (fake, c, b) = crew();
    let future = crewboard::clock::now_epoch() + 600;
    let id = fake.signed_with_ts(
        &c.key,
        &c.name,
        &b.name,
        Kind::Info,
        THREAD,
        "from the future",
        future,
    );
    let ok = fake.signed_with_ts(
        &c.key,
        &c.name,
        &b.name,
        Kind::Info,
        THREAD,
        "ten seconds old",
        crewboard::clock::now_epoch() - 40,
    );
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    assert_eq!(reasons(&v, id), vec!["replayed"], "{v}");
    assert!(reasons(&v, ok).is_empty(), "{v}");
}

#[test]
fn messages_that_the_cli_posts_pass_the_window_and_audit_finds_a_planted_copy() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT replay audit");
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["block", &ts, "--why", "hold"]);
    let _ = fake.poll_json(&c);
    run_ok(&fake, &c, &["unblock", &ts]);
    let replayed = |fake: &FakeBoard| -> Vec<u64> {
        let o = run_ok(
            fake,
            &c,
            &[
                "audit",
                "--json",
                "--initiative",
                crate::support::board::INITIATIVE,
            ],
        );
        let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
        assert!(v["report"]["scanned"].as_u64().unwrap() >= 4, "{v}");
        v["report"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["kind"] == "replayed")
            .map(|f| f["comment_id"].as_u64().unwrap())
            .collect()
    };
    // every message that the posting code signed is inside the window
    assert_eq!(replayed(&fake), Vec::<u64>::new());
    // the check can say yes: a planted old copy is found
    let day_old = crewboard::clock::now_epoch() - 86_400;
    let copy = fake.signed_with_ts(&c.key, &c.name, &b.name, Kind::Answer, t, "again", day_old);
    assert_eq!(replayed(&fake), vec![copy]);
}

/// The body of the latest crew comment of `kind` on `issue`.
fn latest_body(fake: &FakeBoard, issue: u64, kind: &str) -> String {
    fake.comments_of(issue)
        .into_iter()
        .rev()
        .map(|(_, _, body)| body)
        .find(|b| b.starts_with("<!-- crew") && b.contains(&format!(" kind={kind} ")))
        .expect("a comment of that kind")
}

/// Post `text` again as a new comment of the crew account, created 5 s ago.
fn post_copy(fake: &FakeBoard, issue: u64, text: &str) -> u64 {
    let created = crewboard::clock::now_epoch() - 5;
    fake.comment_at(issue, OWNER, text, created, created)
}

#[test]
fn a_copy_of_a_signed_answer_inside_the_window_is_refused_and_reported() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT replay copy");
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["block", &ts, "--why", "first question"]);
    let _ = fake.poll_json(&c);
    run_ok(&fake, &c, &["unblock", &ts]);
    let answer = latest_body(&fake, t, "answer");
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["block", &ts, "--why", "second question"]);
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Blocked, no answer yet"), "{out}");

    // somebody posts the first answer again: its `ts` is a few minutes old, so inside the window
    let copy = post_copy(&fake, t, &answer);
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "the copy answered the second question: {out}");
    assert!(out.contains("Blocked, no answer yet"), "{out}");
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    assert_eq!(reasons(&v, copy), vec!["replayed"], "{v}");
    assert!(
        v["security"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("earlier comment"),
        "{v}"
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
