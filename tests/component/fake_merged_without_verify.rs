//! A ticket PR that was merged without a passed `verify` (spec/tickets#a-pr-merged-without-a-passed-verify), against the fake GitHub of
//! `tests/support/board.rs`.
//!
//! Anyone with write access can merge a crew PR outside crewboard. `tick` (and `close`)
//! then posts ONE signed `info` on the ticket with the lines `pr:`, `merge-commit:`, `merged-by:`,
//! `head:` and `last-gate:`; `crewboard status` marks the ticket. Nothing is refused or undone.

use crate::support::authority::authority_with;
use crate::support::board::{Agent, FakeBoard, Pull};
use crate::support::cli::{GATE, item_in, run_ok};
use crewboard::rules::Role;

const LOCAL_BODY: &str = "Goal: integration test ticket.\n\nGate: ci\n";
const MARKER: &str = "merged without verify";
const RED_GATE: &str = "host-a: make test rc=1 (one end-to-end test failed), log /tmp/gate.log";

fn authority(ci_mode: &str) -> String {
    authority_with("fake-owner", "release", ci_mode)
}

struct Case {
    fake: FakeBoard,
    c: Agent,
    b: Agent,
    t: u64,
    item: String,
    pr: Pull,
}

/// A ticket in Review with a signed `done`. `local`: the gate is a red local gate
/// (`ci_mode: local`); else the ticket has no CI (`CI: none`).
fn case(local: bool) -> Case {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let body = if local {
        fake.file("AUTHORITY.md", &authority("ci_mode: local\n"));
        LOCAL_BODY.to_string()
    } else {
        format!("Goal: integration test ticket.\n\n{GATE}CI: none\n")
    };
    let t = fake.ticket_issue("IT 176", &body, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    if local {
        fake.workflow_run(9001, &pr.head, "push", "failure");
        fake.check_run(100, 9001, &pr.head, "build", "failure");
    }
    let _ = fake.poll_json(&b);
    if local {
        run_ok(
            &fake,
            &b,
            &[
                "done",
                &ts,
                "--pr",
                &pr.url,
                "--sha",
                &pr.head,
                "--local-run",
                RED_GATE,
            ],
        );
    } else {
        run_ok(
            &fake,
            &b,
            &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
        );
    }
    Case {
        fake,
        c,
        b,
        t,
        item,
        pr,
    }
}

fn headless(k: &Case) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&k.c, dir.path());
    dir
}

fn status(k: &Case) -> Option<String> {
    k.fake.item_field(&k.item, "Status")
}

/// The notes on the ticket.
fn notes(k: &Case) -> Vec<String> {
    k.fake
        .comments_of(k.t)
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.contains(&format!("{MARKER}: no passed verify")))
        .collect()
}

fn merge_sha(k: &Case) -> String {
    format!("{:040x}", 0xabc0_u128 + u128::from(k.pr.number))
}

fn status_text(k: &Case) -> String {
    run_ok(&k.fake, &k.c, &["status"]).stdout
}

/// The line of the ticket in `status` and the marker line below it, if any.
fn marked(k: &Case, text: &str) -> bool {
    let key = format!("#{}", k.t);
    let mut lines = text.lines().skip_while(|l| !l.contains(&key));
    lines.next().is_some() && lines.next().is_some_and(|l| l.contains(MARKER))
}

#[test]
fn the_tick_notes_an_owner_merge_without_verify_once() {
    let k = case(false);
    let dir = headless(&k);
    // control: no marker and no note before the merge
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(notes(&k).is_empty(), "{:?}", notes(&k));
    assert!(!marked(&k, &status_text(&k)));

    k.fake.merge_pr(k.pr.number);
    k.fake.close_issue(k.t);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(status(&k).as_deref(), Some("Done"), "{}", o.all());
    assert!(o.stdout.contains(MARKER), "{}", o.all());
    let n = notes(&k);
    assert_eq!(n.len(), 1, "{n:?}");
    for line in [
        format!("pr: {}", k.pr.url),
        format!("merge-commit: {}", merge_sha(&k)),
        "merged-by: fake-owner".to_string(),
        format!("head: {}", k.pr.head),
        "last-gate: none".to_string(),
    ] {
        assert!(n[0].contains(&line), "{line}: {}", n[0]);
    }
    // a second tick posts nothing
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(notes(&k).len(), 1);
    // status marks the ticket
    assert!(marked(&k, &status_text(&k)), "{}", status_text(&k));
    assert!(k.fake.unhandled().is_empty(), "{:?}", k.fake.unhandled());
}

#[test]
fn a_merge_by_someone_else_is_noted_and_the_ticket_stays_in_review() {
    let k = case(false);
    let dir = headless(&k);
    k.fake.merge_pr_by(k.pr.number, "fake-crew");
    k.fake.close_issue(k.t);
    for _ in 0..2 {
        let o = k.fake.run(dir.path(), &["tick"]);
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(notes(&k).len(), 1, "{}", o.all());
    }
    assert!(notes(&k)[0].contains("merged-by: fake-crew"));
    // no Status change: close needs the owner as the merge actor
    assert_eq!(status(&k).as_deref(), Some("Review"));
    assert!(marked(&k, &status_text(&k)), "{}", status_text(&k));
}

#[test]
fn a_merge_after_a_passed_verify_on_the_head_is_not_noted() {
    let k = case(false);
    let ts = k.t.to_string();
    let o = run_ok(&k.fake, &k.c, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    k.fake.merge_pr(k.pr.number);
    k.fake.close_issue(k.t);
    let dir = headless(&k);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(status(&k).as_deref(), Some("Done"), "{}", o.all());
    assert!(notes(&k).is_empty(), "{:?}", notes(&k));
    assert!(!marked(&k, &status_text(&k)));
    // the same through close
    let k = case(false);
    run_ok(&k.fake, &k.c, &["verify", &k.t.to_string()]);
    k.fake.merge_pr(k.pr.number);
    k.fake.close_issue(k.t);
    run_ok(&k.fake, &k.c, &["close", &k.t.to_string()]);
    assert!(notes(&k).is_empty(), "{:?}", notes(&k));
}

#[test]
fn close_notes_the_gap_once_and_still_ends_the_ticket() {
    let k = case(false);
    k.fake.merge_pr(k.pr.number);
    k.fake.close_issue(k.t);
    let o = run_ok(&k.fake, &k.c, &["close", &k.t.to_string()]);
    assert!(o.stdout.contains("closed: Status Done"), "{}", o.all());
    assert!(o.stdout.contains(MARKER), "{}", o.all());
    assert_eq!(status(&k).as_deref(), Some("Done"));
    assert_eq!(notes(&k).len(), 1);
    // the old notice to the assignee is still there
    assert!(
        k.fake
            .comments_of(k.t)
            .into_iter()
            .any(|(_, _, b)| b.contains("merged by the owner"))
    );
    assert!(marked(&k, &status_text(&k)));
    let _ = &k.b;
}

#[test]
fn a_red_local_gate_line_is_named_in_last_gate() {
    let k = case(true);
    let dir = headless(&k);
    k.fake.merge_pr_by(k.pr.number, "fake-crew");
    k.fake.close_issue(k.t);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let n = notes(&k);
    assert_eq!(n.len(), 1, "{}", o.all());
    assert!(
        n[0].contains(&format!(
            "last-gate: local-gate {RED_GATE}, head {}",
            k.pr.head
        )),
        "{}",
        n[0]
    );
}
