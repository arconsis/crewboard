//! `assign` signs a `ci: none` line (else `ci: required`) in the assignment message,
//! and `done`, `verify` and `merge` read that line, not the live issue body. A ticket that was
//! assigned before the line existed has none: it falls back to the live body.

use crate::support::authority::AUTHORITY;
use crate::support::board::{Agent, FakeBoard, REPO};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok, run_refused};
use crewboard::protocol::names::Kind;

fn body(ci_none: bool) -> String {
    format!(
        "Goal: signed ci test.\n\n{GATE}{}",
        if ci_none { "CI: none\n" } else { "" }
    )
}

/// The "Assigned to you" messages that the poll of `who` delivers.
fn assignments(fake: &FakeBoard, who: &Agent) -> Vec<String> {
    let (v, _) = fake.poll_json(who);
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "message")
        .filter_map(|e| e["body"].as_str())
        .filter(|b| b.starts_with("Assigned to you"))
        .map(str::to_string)
        .collect()
}

fn has_line(msg: &str, line: &str) -> bool {
    msg.lines().any(|l| l == line)
}

/// A ticket that an OLD binary assigned: a signed assignment with the gate line and no `ci:`
/// line. Returns the issue number and the board item.
fn old_assigned(fake: &FakeBoard, c: &Agent, b: &Agent, body: &str) -> (u64, String) {
    let t = fake.ticket_issue("IT old ticket", body, &["crew:work"]);
    let item = format!("PVTI_old{t}");
    fake.item(
        &item,
        &[
            ("Status", "Ready"),
            ("Agent", &b.name),
            ("Gate", "the PR closes it"),
            ("Kind", "Work"),
        ],
    );
    fake.signed(
        &c.key,
        &c.name,
        "owner",
        Kind::Info,
        t,
        &format!("Ticket is on the board.\nitem={item}\nproject=15\n"),
    );
    fake.signed(
        &c.key,
        &c.name,
        &b.name,
        Kind::Request,
        t,
        &format!(
            "Assigned to you: IT old ticket\nticket: {REPO}#{t}\nitem={item}\ngate: the PR closes it\n"
        ),
    );
    (t, item)
}

#[test]
fn assign_signs_ci_none_or_ci_required_and_reassign_copies_the_line() {
    let (fake, c, b) = crew();
    let none = fake.ticket_issue("IT declared", &body(true), &["crew:work"]);
    let req = fake.ticket_issue("IT required", &body(false), &["crew:work"]);
    run_ok(&fake, &c, &["assign", &none.to_string(), "--to", &b.name]);
    run_ok(&fake, &c, &["assign", &req.to_string(), "--to", &b.name]);
    let msgs = assignments(&fake, &b);
    assert_eq!(msgs.len(), 2, "{msgs:?}");
    let m_none = msgs
        .iter()
        .find(|m| m.contains(&format!("#{none}")))
        .unwrap();
    let m_req = msgs
        .iter()
        .find(|m| m.contains(&format!("#{req}")))
        .unwrap();
    assert!(has_line(m_none, "ci: none"), "{m_none}");
    assert!(has_line(m_req, "ci: required"), "{m_req}");
    // reassign copies the signed line forward, also after a body edit
    fake.set_issue_body(none, &body(false));
    run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &none.to_string(),
            "--to",
            &b.name,
            "--reason",
            "late",
            "--why",
            "it: again",
        ],
    );
    let again = assignments(&fake, &b);
    assert_eq!(again.len(), 1, "{again:?}");
    assert!(has_line(&again[0], "ci: none"), "{}", again[0]);
    assert!(
        has_line(
            &again[0],
            "gate: the PR of this ticket is open and closes it."
        ),
        "{}",
        again[0]
    );
}

#[test]
fn a_body_edit_that_adds_ci_none_does_not_let_a_done_through() {
    let (fake, c, b) = crew();
    let t = fake.ticket_issue("IT undeclared", &body(false), &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url],
        "evidence-ci-missing",
    );
    // a crew-token holder edits the body after the assignment
    fake.set_issue_body(t, &body(true));
    run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url],
        "evidence-ci-missing",
    );
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    no_unhandled(&fake);
}

#[test]
fn a_ticket_that_signed_ci_none_keeps_it_when_the_body_loses_the_line() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let t = fake.ticket_issue("IT declared", &body(true), &["crew:work"]);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    // the body edit removes `CI: none` before `done`, `verify` and `merge`
    fake.set_issue_body(t, &body(false));
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    let m = run_ok(&fake, &c, &["merge", &ts]);
    assert!(m.stdout.contains("merged"), "{}", m.stdout);
    no_unhandled(&fake);
}

#[test]
fn an_old_ticket_without_a_signed_ci_line_still_works_through_the_body() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    // the body declares `CI: none`: the done passes, as it did before the signed line
    let (t, item) = old_assigned(&fake, &c, &b, &body(true));
    let ts = t.to_string();
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Review"));
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    let m = run_ok(&fake, &c, &["merge", &ts]);
    assert!(m.stdout.contains("merged"), "{}", m.stdout);

    // control: an old ticket whose body has no `CI: none` is refused, as before
    let (t2, _) = old_assigned(&fake, &c, &b, &body(false));
    let ts2 = t2.to_string();
    run_ok(&fake, &b, &["take", &ts2]);
    let pr2 = fake.pr_for(t2, &b.name);
    let _ = fake.poll_json(&b);
    run_refused(
        &fake,
        &b,
        &["done", &ts2, "--pr", &pr2.url],
        "evidence-ci-missing",
    );
    no_unhandled(&fake);
}
