//! The replay window: a signed message is fresh only if its `ts` is inside
//! `[created_at - 15 min, created_at + 5 min]` (spec/messages#signature). A copy of an old
//! valid message gets a new `created_at` and keeps its old `ts`.

use crate::support::board::{Agent, FakeBoard, OWNER, REGISTRY, REPO, THREAD};
use crate::support::cli::{crew, run_ok};
use crewboard::protocol::names::Kind;
use crewboard::protocol::signature::generate_key;
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

/// Post `text` again as a new comment of the agent account, created 5 s ago.
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

/// The owner reports (`to=owner kind=info`) in the registry issue whose text has `needle`, by id.
fn owner_reports(fake: &FakeBoard, needle: &str) -> Vec<u64> {
    fake.comments_of(REGISTRY)
        .into_iter()
        .filter(|(_, _, b)| b.contains("to=owner kind=info") && b.contains(needle))
        .map(|(id, _, _)| id)
        .collect()
}

/// Move the cursor of `who` ten minutes back: the next poll reads the recent comments again,
/// as after a restart.
fn cursor_back(fake: &FakeBoard, who: &Agent) {
    let back = format!(
        "c1 ts={} seen=",
        crewboard::clock::format_ts(crewboard::clock::now_epoch() - 600)
    );
    fake.set_item(&who.item_id, "Cursor", &back);
}

#[test]
fn a_report_that_github_stamps_an_hour_late_is_not_repeated_and_not_reported_itself() {
    let (fake, c, b) = crew();
    let wrong = generate_key().unwrap();
    let (forged, _) = fake.signed(&wrong, &b.name, &c.name, Kind::Report, THREAD, "forged");
    let key = format!("event={REPO}#{THREAD}/c-{forged}:bad-signature");
    // from now on GitHub stamps every new comment an hour late: the report fails the window
    fake.post_late(3600);
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    let first = owner_reports(&fake, &key);
    assert_eq!(first.len(), 1, "one report of the event: {v}");

    // the coordinator sees its late report as a security event, and tells the owner nothing
    let (v, _) = fake.poll_json(&c);
    assert_eq!(reasons(&v, first[0]), vec!["replayed"], "{v}");
    for _ in 0..2 {
        let _ = fake.poll_json(&c);
    }
    cursor_back(&fake, &c);
    let _ = fake.poll_json(&c);
    let _ = fake.poll_json(&c);
    // (reports of the event, reports about a report)
    assert_eq!(
        (
            owner_reports(&fake, &key),
            owner_reports(&fake, "Security event: replayed-message")
        ),
        (first, Vec::new())
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_late_report_with_a_bad_signature_is_reported_and_hides_no_event() {
    let (fake, c, b) = crew();
    let wrong = generate_key().unwrap();
    let (forged, _) = fake.signed(&wrong, &b.name, &c.name, Kind::Report, THREAD, "forged");
    let key = format!("event={REPO}#{THREAD}/c-{forged}:bad-signature");
    // a report of that event in the name of the coordinator, an hour old, signed with a key
    // that is not the coordinator's
    let hour_old = crewboard::clock::now_epoch() - 3600;
    let fake_report = fake.signed_with_ts(
        &wrong,
        &c.name,
        "owner",
        Kind::Info,
        REGISTRY,
        &format!("Security event: bad-signature\n{key}\n"),
        hour_old,
    );
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert_eq!(reasons(&v, fake_report), vec!["bad_signature"], "{v}");
    // its key does not count: the coordinator reports the event itself
    let real: Vec<u64> = owner_reports(&fake, &key)
        .into_iter()
        .filter(|id| *id != fake_report)
        .collect();
    assert_eq!(real.len(), 1, "{v}");
    // and the forged report is an event of its own
    let of_report = format!("event={REPO}#{REGISTRY}/c-{fake_report}:bad-signature");
    assert_eq!(owner_reports(&fake, &of_report).len(), 1, "{v}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
