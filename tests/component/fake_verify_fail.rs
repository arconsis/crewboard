//! `verify --fail --body` (spec/tickets#failed-gate-check), against the fake GitHub
//! of `tests/support/board.rs`.
//!
//! The case: the evidence of a `done` holds (PR head unchanged, CI accepted), but the
//! coordinator's own Gate check fails. `verify --fail` records that as a failed verify, with the
//! writes of any failed verify. It is refused without a note, for a dev agent, for a
//! headless coordinator and outside Review (or with an open question to the owner). A plain
//! `verify` does not change.

use crate::support::board::{Agent, FakeBoard, Out, REPO};
use crate::support::cli::{GATE, crew, item_in, run_ok, writes};

const NOTE: &str = "Gate part 2 (the migration test) is missing";

/// A ticket that declares `CI: none`, so its evidence holds without a CI run.
fn ticket(fake: &FakeBoard, title: &str) -> u64 {
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    fake.ticket_issue(title, &body, &["crew:work"])
}

/// Run in `dir`: refused with `rule` (exit 2), and nothing was written.
fn refused_no_write(fake: &FakeBoard, dir: &std::path::Path, args: &[&str], rule: &str) -> Out {
    let before = writes(fake);
    let o = fake.run(dir, args);
    assert_eq!(o.code, 2, "{args:?}: {}", o.all());
    assert!(
        o.stderr.contains(rule),
        "{rule} missing for {args:?}: {}",
        o.all()
    );
    assert_eq!(writes(fake), before, "the refusal wrote: {}", o.all());
    o
}

fn status(fake: &FakeBoard, item: &str) -> Option<String> {
    fake.item_field(item, "Status")
}

/// The latest verify report on ticket `t`.
fn verify_report(fake: &FakeBoard, t: u64) -> String {
    fake.comments_of(t)
        .into_iter()
        .map(|(_, _, body)| body)
        .rev()
        .find(|b| b.contains("verify: fail") || b.contains("verify: pass"))
        .expect("a verify report")
}

/// Assign, take, PR and done: the ticket is in Review. Returns `(issue, item)`.
fn delivered(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> (u64, String) {
    let t = ticket(fake, title);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
    assert_eq!(status(fake, &item).as_deref(), Some("Review"));
    (t, item)
}

#[test]
fn verify_fail_with_a_note_fails_green_evidence_and_moves_the_ticket_to_in_progress() {
    let (fake, c, b) = crew();
    let (t, item) = delivered(&fake, &c, &b, "IT 159 idle");
    let ts = t.to_string();

    // refused: --fail without a note, or with an empty one (before any read)
    refused_no_write(
        &fake,
        &c.dir,
        &["verify", &ts, "--fail"],
        "verify-fail-needs-note",
    );
    refused_no_write(
        &fake,
        &c.dir,
        &["verify", &ts, "--fail", "--body", "  "],
        "verify-fail-needs-note",
    );
    // refused: a dev agent
    refused_no_write(
        &fake,
        &b.dir,
        &["verify", &ts, "--fail", "--body", NOTE],
        "verify-coordinator-only",
    );
    // refused: a headless coordinator (the Gate check is a judgment)
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    refused_no_write(
        &fake,
        headless.path(),
        &["verify", &ts, "--fail", "--body", NOTE],
        "headless-scope",
    );
    assert_eq!(status(&fake, &item).as_deref(), Some("Review"));

    // pass: the evidence holds, the coordinator's Gate check failed
    let o = run_ok(&fake, &c, &["verify", &ts, "--fail", "--body", NOTE]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout
            .contains(&format!("failed: Gate check by the coordinator: {NOTE}")),
        "{}",
        o.all()
    );
    assert!(
        o.stdout.contains("Status moved back to In Progress"),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &item).as_deref(), Some("In Progress"));
    let report = verify_report(&fake, t);
    assert!(report.contains("verify: fail"), "{report}");
    assert!(
        report.contains(&format!("failed: Gate check by the coordinator: {NOTE}")),
        "{report}"
    );
    // the only failure is the note: the evidence itself held
    assert_eq!(report.matches("failed: ").count(), 1, "{report}");
    // the note appears once: no `note:` line repeats the `failed:` line
    assert_eq!(report.matches(NOTE).count(), 1, "{report}");

    // refused: the ticket is not in Review any more, as for a plain verify
    refused_no_write(
        &fake,
        &c.dir,
        &["verify", &ts, "--fail", "--body", NOTE],
        "verify-needs-review",
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_plain_verify_of_the_same_evidence_still_passes() {
    // control: without --fail the same green evidence passes and the ticket stays in Review
    let (fake, c, b) = crew();
    let (t, item) = delivered(&fake, &c, &b, "IT 159 plain");
    let o = run_ok(&fake, &c, &["verify", &t.to_string(), "--body", NOTE]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    assert_eq!(status(&fake, &item).as_deref(), Some("Review"));
    let report = verify_report(&fake, t);
    assert!(report.contains("verify: pass"), "{report}");
    assert!(
        !report.contains("Gate check by the coordinator"),
        "{report}"
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn verify_fail_of_a_busy_agent_queues_the_ticket_first_in_ready() {
    let (fake, c, b) = crew();
    let (a, a_item) = delivered(&fake, &c, &b, "IT 159 A");
    // the agent works on another ticket
    let other = ticket(&fake, "IT 159 other");
    let other_item = item_in(&run_ok(
        &fake,
        &c,
        &["assign", &other.to_string(), "--to", &b.name],
    ));
    run_ok(&fake, &b, &["take", &other.to_string()]);
    let o = run_ok(
        &fake,
        &c,
        &["verify", &a.to_string(), "--fail", "--body", NOTE],
    );
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert_eq!(status(&fake, &a_item).as_deref(), Some("Ready"));
    assert_eq!(
        fake.item_field(&a_item, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    assert_eq!(status(&fake, &other_item).as_deref(), Some("In Progress"));
    let report = verify_report(&fake, a);
    assert!(
        report.contains(&format!(
            "verify failed; {} works on {REPO}#{other}; queued as next",
            b.name
        )),
        "{report}"
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn verify_fail_records_a_failed_gate_check_also_with_an_open_question() {
    let (fake, c, b) = crew();
    let (t, item) = delivered(&fake, &c, &b, "IT 159 go");
    let ts = t.to_string();
    run_ok(
        &fake,
        &b,
        &["ask", &ts, "--body", "Release go for this change?"],
    );
    assert_eq!(status(&fake, &item).as_deref(), Some("Review"));
    // a plain verify is refused while the question is open
    refused_no_write(
        &fake,
        &c.dir,
        &["verify", &ts],
        "verify-owner-question-open",
    );
    // `verify --fail` is no exit to Done: it moves the ticket as from Review; the question
    // stays open, so the label stays
    let o = run_ok(&fake, &c, &["verify", &ts, "--fail", "--body", NOTE]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert_eq!(status(&fake, &item).as_deref(), Some("In Progress"));
    assert!(
        fake.labels_of(t).contains(&"needs-owner".to_string()),
        "{:?}",
        fake.labels_of(t)
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
