//! Board and comment behaviour of the CLI against a fake GitHub.
//!
//! These tests run in plain `cargo test`: no token, no sandbox, no real GitHub. They replace
//! eight tests of `it_sandbox.rs` (see `CONTRIBUTING.md`). The fake lives in
//! `tests/support/board.rs`; the agents, their keys and the registry comments are made by
//! `FakeBoard::agent`.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REGISTRY, REPO, THREAD};
use crewboard::protocol::names::Kind;
use crewboard::protocol::signature::generate_key;
use crewboard::rules::Role;
use serde_json::Value;

fn ids(v: &Value, key: &str) -> Vec<u64> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array in {v}"))
        .iter()
        .map(|e| e["comment_id"].as_u64().unwrap())
        .collect()
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

/// A dev agent that sends to B and a dev agent B that polls.
fn pair(fake: &FakeBoard) -> (Agent, Agent) {
    (
        fake.agent("claude@fake-a", Role::Dev),
        fake.agent("hermes@fake-b", Role::Dev),
    )
}

#[test]
fn say_refuses_a_secret_without_leaking_it_and_a_stranger_cannot_say() {
    let fake = FakeBoard::new();
    let a = fake.agent("claude@fake-a", Role::Dev);
    let token: String = "aB3dE5gH7jK9mN1pQ3sT5vW7yZ"
        .chars()
        .cycle()
        .take(36)
        .collect();
    let o = fake.run(
        &a.dir,
        &[
            "say",
            "2",
            "--to",
            "owner",
            "--kind",
            "info",
            "--body",
            &format!("t ghp_{token}"),
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("secret-in-body") && !o.stderr.contains(&token),
        "{}",
        o.stderr
    );
    assert!(!o.stdout.contains(&token), "{}", o.stdout);
    // nothing was written: no comment on any issue
    assert!(
        fake.requests().iter().all(|r| r.method == "GET"),
        "{:?}",
        fake.requests()
    );
    assert!(fake.comments_of(THREAD).is_empty());

    // control: a clean body passes, and exactly one signed comment is posted
    let o = fake.run(
        &a.dir,
        &[
            "say",
            "2",
            "--to",
            "owner",
            "--kind",
            "info",
            "--body",
            "CI is green",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let posted = fake.comments_of(THREAD);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(posted[0].2.contains("CI is green") && posted[0].2.contains("claude@fake-a"));

    // An agent that never joined has no config: exit 4, and the fake is not called at all.
    let before = fake.requests().len();
    let empty = tempfile::tempdir().unwrap();
    let o = fake.run(
        empty.path(),
        &["say", "2", "--to", "owner", "--kind", "info", "--body", "x"],
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert_eq!(fake.requests().len(), before);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn forged_edited_and_tampered_messages_are_refused_and_a_valid_one_passes() {
    let fake = FakeBoard::new();
    let (a, b) = pair(&fake);
    let wrong = generate_key().unwrap();
    let kind = Kind::Report;
    // 1. wrong key: claims to be A, signed by a key nobody registered
    let (forged, _) = fake.signed(&wrong, &a.name, &b.name, kind, THREAD, "wrong key");
    // 2. tampered body: a real signed message from A with another body line
    let (original, text) = fake.signed(&a.key, &a.name, &b.name, kind, THREAD, "original");
    let t = crewboard::clock::now_epoch() - 30;
    let tampered = fake.comment_at(
        THREAD,
        crate::support::board::OWNER,
        &text.replace("original", "tampered"),
        t,
        t,
    );
    assert_ne!(fake.comment_body(original), fake.comment_body(tampered));
    // 3. edited: a valid message that is changed after posting
    let (edited, _) = fake.signed(&a.key, &a.name, &b.name, kind, THREAD, "to be edited");
    fake.edit_comment(
        edited,
        &format!("{}\nedited later", fake.comment_body(edited)),
    );
    // 4. control: a valid message from A must still arrive (the check can say yes)
    let (valid, _) = fake.signed(&a.key, &a.name, &b.name, kind, THREAD, "valid control");

    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    let delivered = ids(&v, "events");
    assert!(delivered.contains(&valid), "control not delivered: {v}");
    assert!(
        delivered.contains(&original),
        "the untouched original must be delivered: {v}"
    );
    for (id, what) in [
        (forged, "wrong key"),
        (tampered, "tampered"),
        (edited, "edited"),
    ] {
        assert!(!delivered.contains(&id), "{what} was DELIVERED: {v}");
    }
    assert_eq!(reasons(&v, forged), vec!["bad_signature"], "{v}");
    assert_eq!(reasons(&v, tampered), vec!["bad_signature"], "{v}");
    assert_eq!(reasons(&v, edited), vec!["edited"], "{v}");
    assert!(reasons(&v, valid).is_empty() && reasons(&v, original).is_empty());
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn an_owner_message_reaches_the_addressed_agent_first() {
    let fake = FakeBoard::new();
    let (a, b) = pair(&fake);
    // the agent message is posted FIRST (lower id), so only the class decides the order
    let (mid, _) = fake.signed(
        &a.key,
        &a.name,
        &b.name,
        Kind::Info,
        THREAD,
        "agent message",
    );
    // the owner's comment: the token user, no header
    let oid = fake.owner_comment(THREAD, &format!("-> {}\nowner says hello", b.name));
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    let order = ids(&v, "events");
    let (po, pm) = (
        order.iter().position(|i| *i == oid),
        order.iter().position(|i| *i == mid),
    );
    assert!(po.is_some() && pm.is_some(), "{v}");
    assert!(po < pm, "owner messages come first: {order:?}");
    let ev = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["comment_id"].as_u64() == Some(oid))
        .unwrap();
    assert_eq!(ev["class"], "owner");
    // A does not get a message that is addressed to B
    let (va, _) = fake.poll_json(&a);
    assert!(!ids(&va, "events").contains(&oid), "{va}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn the_registry_trusts_the_approved_agents_and_status_reports_them() {
    let fake = FakeBoard::new();
    let (a, b) = pair(&fake);
    let c = fake.agent("codex@fake-c", Role::Dev);
    // the Key field of C's item is not the key of its join
    let other = generate_key().unwrap().public_key().to_openssh().unwrap();
    fake.set_item(&c.item_id, "Key", &other);
    let o = fake.run(&a.dir, &["status", "--json"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    let row = |n: &str| {
        v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == n)
            .unwrap_or_else(|| panic!("{n} not listed: {v}"))
            .clone()
    };
    for n in [&a.name, &b.name] {
        assert_eq!(row(n)["trusted"], true, "{n} not trusted: {v}");
        assert_eq!(row(n)["approved"], true);
    }
    // the item Key is a mirror: the signed join decides, so C stays trusted; status reports it
    assert_eq!(row(&c.name)["trusted"], true, "{v}");
    let text = fake.run(&a.dir, &["status"]);
    assert!(
        text.stdout
            .contains("trust problem: codex@fake-c: Key field of the item differs"),
        "{}",
        text.all()
    );
    assert!(v["foreign_comments"].is_number() && v["rate_limit"]["core"]["remaining"].is_number());
    assert!(v["items_known_by_id_missing_from_listing"].is_array());
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn the_no_events_line_carries_cursor_polls_and_time_and_exit_1() {
    let fake = FakeBoard::new();
    let (a, b) = pair(&fake);
    let o = fake.run(&b.dir, &["poll"]);
    assert_eq!(o.code, 1, "{}", o.all());
    for needle in ["no events:", "cursor=c1 ts=", "polls=", "last_ok="] {
        assert!(o.stdout.contains(needle), "{needle} missing in {}", o.all());
    }
    let w = fake.run(&b.dir, &["watch", "--timeout", "3s", "--interval", "1s"]);
    assert_eq!(w.code, 1, "{}", w.all());
    assert!(
        w.stdout.contains("timeout") && w.stdout.contains("cursor="),
        "{}",
        w.all()
    );
    // control: with a message for B the same poll is exit 0 and has no "no events" line
    fake.signed(&a.key, &a.name, &b.name, Kind::Info, THREAD, "hello");
    let o = fake.run(&b.dir, &["poll"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("no events:"), "{}", o.all());
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn security_events_are_reported_to_the_owner_once_and_the_audit_lists_them() {
    let fake = FakeBoard::new();
    let a = fake.agent("claude@fake-a", Role::Dev);
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    // a comment signed with a key nobody registered, a valid one, and a headerless comment
    // by the owner's account
    let wrong = generate_key().unwrap();
    let (forged, _) = fake.signed(&wrong, &a.name, &c.name, Kind::Report, THREAD, "forged");
    let (valid, _) = fake.signed(
        &a.key,
        &a.name,
        &c.name,
        Kind::Info,
        THREAD,
        "valid control",
    );
    let plain = fake.owner_comment(THREAD, "IT headerless owner-account comment");
    let key = format!("{REPO}#{THREAD}/c-{forged}:bad-signature");
    let reports = || {
        fake.comments_of(REGISTRY)
            .iter()
            .filter(|(_, _, b)| {
                b.contains("to=owner kind=info") && b.contains(&format!("event={key}"))
            })
            .count()
    };

    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["security"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["comment_id"] == forged && e["reason"] == "bad_signature"),
        "the coordinator gets the security event: {v}"
    );
    assert_eq!(reports(), 1, "one signed kind=info to the owner");
    let (_, code) = fake.poll_json(&c);
    assert_eq!(code, 1, "delivered once");
    let _ = fake.poll_json(&c);
    assert_eq!(reports(), 1, "still one report after more polls");
    // a restart that reads the same comments again repeats nothing: the board knows the report
    let back = format!(
        "c1 ts={} seen=",
        crewboard::clock::format_ts(crewboard::clock::now_epoch() - 600)
    );
    fake.set_item(&c.item_id, "Cursor", &back);
    let (v, _) = fake.poll_json(&c);
    assert!(
        v["security"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["comment_id"] == forged),
        "the old event is fresh again after the cursor went back: {v}"
    );
    assert_eq!(reports(), 1, "still one report after a cursor reset");
    // the valid control produced neither a security event nor an owner message
    let event_of_valid = format!("event={REPO}#{THREAD}/c-{valid}:");
    assert!(
        !fake
            .comments_of(REGISTRY)
            .iter()
            .any(|(_, _, b)| b.contains(&event_of_valid))
    );

    // the audit lists the forged comment and the headerless one, and not the valid one
    let owner = fake.owner_dir();
    let args = ["audit", "--since", "30m", "--initiative", INITIATIVE];
    let o = fake.run(owner.path(), &args);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains(&format!("comment {forged} by")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!("comment {plain} by")),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("[bad-signature]"), "{}", o.stdout);
    assert!(
        o.stdout.contains("reported to the owner: yes"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains(&format!("comment {valid} by")),
        "a valid message is no finding: {}",
        o.stdout
    );
    let o = fake.run(
        owner.path(),
        &[
            "audit",
            "--json",
            "--since",
            "30m",
            "--initiative",
            INITIATIVE,
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let j: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    let kinds: Vec<(u64, String)> = j["report"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["comment_id"].as_u64().unwrap(),
                f["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        kinds.contains(&(forged, "bad_signature".into())),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(plain, "headerless_owner".into())),
        "{kinds:?}"
    );
    assert!(!kinds.iter().any(|(id, _)| *id == valid), "{kinds:?}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_lost_cache_rebuilds_the_ticket_index_from_the_signed_info_comments() {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    // B polls once: the one-time scan runs and finds no ticket yet
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 1, "{v}");
    // The coordinator makes a ticket long ago: an issue with the work label, a board item
    // that is assigned to B, and the signed info comment with the item id. The comment is
    // older than the feed window of B, so only the scan can find it.
    let ticket = fake.ticket_issue(
        "IT cache loss",
        "Goal: a ticket.\n\n**Gate:** the PR is open.\nCI: none\n",
        &["crew:work"],
    );
    let item = fake.item("PVTI_ticket3", &[("Status", "Ready"), ("Agent", &b.name)]);
    let old = crewboard::clock::now_epoch() - 3600;
    let info = crate::support::board::sign_ts(
        &c.key,
        ticket,
        &c.name,
        "all",
        Kind::Info,
        &format!("ticket info\nitem={item}\n"),
        old,
    );
    fake.comment_at(ticket, crate::support::board::OWNER, &info, old, old);
    // the signed assignment that `take` requires (the Agent field alone is not trusted)
    let assign = crate::support::board::sign_ts(
        &c.key,
        ticket,
        &c.name,
        &b.name,
        Kind::Request,
        &crewboard::protocol::acts::assignment_body(
            "IT cache loss",
            &format!("{}#{ticket}", crate::support::board::REPO),
            &item,
            None,
            None,
            "",
        ),
        old,
    );
    fake.comment_at(ticket, crate::support::board::OWNER, &assign, old, old);

    let is_assignment =
        |v: &Value| {
            v["events"].as_array().unwrap().iter().any(|e| {
                e["class"] == "ticket" && e["kind"] == "assignment" && e["issue"] == ticket
            })
        };
    // control: with a cache that says "scanned", the old ticket is not found
    let (v, _) = fake.poll_json(&b);
    assert!(!is_assignment(&v), "found without a scan: {v}");
    // B loses its local state: the ticket is found again by the one-time scan, by id
    std::fs::remove_dir_all(b.dir.join("state")).unwrap();
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    assert!(is_assignment(&v), "assignment found after cache loss: {v}");
    // control: the next poll does not repeat it (the snapshot is stored)
    let (v2, _) = fake.poll_json(&b);
    assert!(
        !v2["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["class"] == "ticket" && e["issue"] == ticket),
        "{v2}"
    );
    // the ticket found by the scan can be taken (the second half of the old sandbox test)
    let o = fake.run(&b.dir, &["take", &ticket.to_string()]);
    assert_eq!(o.code, 0, "take after cache loss: {}", o.all());
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
