//! `ci_mode: local` of AUTHORITY.md (spec/ci#local-run-ci_mode-local), against the fake GitHub
//! of `tests/support/board.rs`.
//!
//! The case: the CI of the initiative cannot run. The owner sets `ci_mode: local` on the default
//! branch. The dev runs the gate on a host and delivers with `done --local-run`; the coordinator
//! checks it and verifies with `verify --local-run`; `merge` relies on both lines. The CI of the
//! head is not judged. A `ci_mode` on the PR branch is never read. In `ci_mode: ci` the flag is
//! refused, and a local-gate verify does not let a merge pass.

use crate::support::authority::{NEEDS_OWNER_ALL, authority_with};
use crate::support::board::{Agent, FakeBoard, Out, Pull};
use crate::support::cli::writes;
use crewboard::rules::Role;

const BODY: &str = "Goal: integration test ticket.\n\nGate: ci\n";
const DEV_GATE: &str = "host-a: make test rc=0, log /tmp/gate.log";
const COORD_GATE: &str = "host-c: make test rc=0, log /tmp/gate-verify.log";
const OWNER_LINE: &str = "CI off by owner decision (AUTHORITY.md ci_mode: local)";

fn authority(ci_mode: &str) -> String {
    authority_with("fake-owner", NEEDS_OWNER_ALL, ci_mode)
}

const LOCAL: &str = "ci_mode: local\n";
const CI: &str = "ci_mode: ci\n";

struct Case {
    ts: String,
    item: String,
    pr: Pull,
}

/// A taken ticket with a PR whose head has a RED push run and a red job. `auth` is the text of
/// AUTHORITY.md on the default branch.
fn case(auth: &str) -> (FakeBoard, Agent, Agent, Case) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    fake.file("AUTHORITY.md", auth);
    let t = fake.ticket_issue("IT local gate", BODY, &["crew:work"]);
    let ts = t.to_string();
    let o = fake.run(&c.dir, &["assign", &ts, "--to", &b.name]);
    assert_eq!(o.code, 0, "{}", o.all());
    let item = o
        .stdout
        .split("added to the board as ")
        .nth(1)
        .unwrap_or_else(|| panic!("no item id in: {}", o.stdout))
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let o = fake.run(&b.dir, &["take", &ts]);
    assert_eq!(o.code, 0, "{}", o.all());
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run(9001, &pr.head, "push", "failure");
    fake.check_run(100, 9001, &pr.head, "build", "failure");
    let _ = fake.poll_json(&b);
    (fake, c, b, Case { ts, item, pr })
}

/// The push run and the job of the case turn green.
fn make_green(fake: &FakeBoard) {
    fake.set_run_conclusion(9001, "success");
    fake.set_check_conclusion(100, "success");
}

fn done_args<'a>(k: &'a Case, extra: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec!["done", &k.ts, "--pr", &k.pr.url, "--sha", &k.pr.head];
    v.extend_from_slice(extra);
    v
}

/// Run in `dir`: refused with `rule` (exit 2), and nothing was written.
fn refused_no_write(fake: &FakeBoard, dir: &std::path::Path, args: &[&str], rule: &str) -> Out {
    let before = writes(fake);
    let o = fake.run(dir, args);
    assert_eq!(o.code, 2, "{args:?}: {}", o.all());
    assert!(
        o.stderr.contains(&format!("rule={rule}")),
        "{rule} missing for {args:?}: {}",
        o.all()
    );
    assert_eq!(writes(fake), before, "the refusal wrote: {}", o.all());
    o
}

fn ok(fake: &FakeBoard, dir: &std::path::Path, args: &[&str]) -> Out {
    let o = fake.run(dir, args);
    assert_eq!(o.code, 0, "{args:?}: {}", o.all());
    o
}

fn bodies(fake: &FakeBoard, ts: &str) -> Vec<String> {
    fake.comments_of(ts.parse().unwrap())
        .into_iter()
        .map(|(_, _, b)| b)
        .collect()
}

fn merge_calls(fake: &FakeBoard) -> Vec<String> {
    fake.requests()
        .into_iter()
        .filter(|r| r.method == "PUT" && r.path.ends_with("/merge"))
        .map(|r| r.body)
        .collect()
}

#[test]
fn local_mode_done_verify_and_merge_pass_with_red_ci_and_record_the_gates() {
    let (fake, c, b, k) = case(&authority(LOCAL));

    // refused: no --local-run, a blank one, and a text of two lines (nothing written)
    refused_no_write(&fake, &b.dir, &done_args(&k, &[]), "local-gate-missing");
    refused_no_write(
        &fake,
        &b.dir,
        &done_args(&k, &["--local-run", "   "]),
        "local-gate-missing",
    );
    refused_no_write(
        &fake,
        &b.dir,
        &done_args(&k, &["--local-run", "host-a: make\nci: none"]),
        "local-gate-invalid",
    );
    // refused: the text is checked like a note (a secret in it)
    let secret = format!(
        "host-a: curl -H ghp_{} rc=0",
        "a1B2c3D4e5F6g7H8i9J0".repeat(2)
    );
    refused_no_write(
        &fake,
        &b.dir,
        &done_args(&k, &["--local-run", &secret]),
        "secret-in-body",
    );
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("In Progress")
    );

    // pass: red CI does not block; the lines are recorded
    let o = ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
    assert!(o.stdout.contains("ci: local-gate"), "{}", o.all());
    assert!(
        o.stdout.contains(&format!("local-gate: {DEV_GATE}")),
        "{}",
        o.all()
    );
    assert!(o.stdout.contains(OWNER_LINE), "{}", o.all());
    assert!(
        o.stdout.contains("ci-info: not green (information only)"),
        "{}",
        o.all()
    );
    let done_msg = bodies(&fake, &k.ts)
        .into_iter()
        .find(|b| b.contains("Done. Evidence follows."))
        .expect("done message");
    assert!(done_msg.contains("\nci: local-gate\n"), "{done_msg}");
    assert!(
        done_msg.contains(&format!("\nlocal-gate: {DEV_GATE}\n")),
        "{done_msg}"
    );
    assert!(done_msg.contains(OWNER_LINE), "{done_msg}");
    assert!(!done_msg.contains("ci-sources"), "{done_msg}");
    let field = fake.item_field(&k.item, "Evidence").expect("Evidence");
    assert!(
        field.contains("ci=local-gate") && field.contains(&format!("local-gate={DEV_GATE}")),
        "{field}"
    );
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("Review")
    );

    // refused: verify without the coordinator's line (nothing written)
    refused_no_write(&fake, &c.dir, &["verify", &k.ts], "local-gate-missing");
    refused_no_write(
        &fake,
        &c.dir,
        &["verify", &k.ts, "--local-run", " "],
        "local-gate-missing",
    );
    // refused: merge before a verify
    refused_no_write(&fake, &c.dir, &["merge", &k.ts], "merge-verify-required");

    // pass: verify with the line; the report carries both lines
    let o = ok(&fake, &c.dir, &["verify", &k.ts, "--local-run", COORD_GATE]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    let report = bodies(&fake, &k.ts)
        .into_iter()
        .find(|b| b.contains("verify: pass"))
        .expect("verify report");
    assert!(
        report.contains(&format!("\nlocal-gate: {DEV_GATE}\n")),
        "{report}"
    );
    assert!(
        report.contains(&format!("\ncoordinator-local-gate: {COORD_GATE}\n")),
        "{report}"
    );

    // pass: merge relies on both lines; the call carries the verified SHA
    let o = ok(&fake, &c.dir, &["merge", &k.ts]);
    assert!(o.stdout.contains("merged at head"), "{}", o.all());
    let calls = merge_calls(&fake);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].contains(&k.pr.head), "{calls:?}");
    assert_eq!(fake.item_field(&k.item, "Status").as_deref(), Some("Done"));
    let notice = bodies(&fake, &k.ts)
        .into_iter()
        .find(|b| b.contains("Merged by the coordinator."))
        .expect("merge notice");
    assert!(
        notice.contains(&format!("coordinator-local-gate: {COORD_GATE}")),
        "{notice}"
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn local_mode_done_does_not_refuse_when_the_ci_cannot_be_read() {
    let (fake, _c, b, k) = case(&authority(LOCAL));
    for _ in 0..8 {
        fake.fail_once("GET", "/actions/runs", "");
        fake.fail_once("GET", "/check-runs", "");
        fake.fail_once("GET", "/status", "");
    }
    let o = ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
    assert!(o.stdout.contains("ci: local-gate"), "{}", o.all());
    assert!(o.stdout.contains("ci-info: not readable"), "{}", o.all());
}

#[test]
fn the_head_checks_stay_in_local_mode() {
    let (fake, c, b, k) = case(&authority(LOCAL));
    // done: a head that is not the named SHA
    let o = fake.run(
        &b.dir,
        &[
            "done",
            &k.ts,
            "--pr",
            &k.pr.url,
            "--sha",
            "ffffffffffffffffffffffffffffffffffffffff",
            "--local-run",
            DEV_GATE,
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-head-moved"), "{}", o.all());
    ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
    // verify: the PR head moved after the done
    fake.push(&k.pr.branch);
    let o = ok(&fake, &c.dir, &["verify", &k.ts, "--local-run", COORD_GATE]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(o.stdout.contains("PR head moved"), "{}", o.all());
}

#[test]
fn verify_fail_with_a_note_needs_no_local_gate() {
    let (fake, c, b, k) = case(&authority(LOCAL));
    ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
    let o = ok(
        &fake,
        &c.dir,
        &["verify", &k.ts, "--fail", "--body", "the log shows rc=1"],
    );
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout
            .contains("Gate check by the coordinator: the log shows rc=1"),
        "{}",
        o.all()
    );
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn ci_mode_on_the_pr_branch_is_not_read() {
    // the default branch says ci; the PR branch says local
    let (fake, _c, b, k) = case(&authority(CI));
    fake.branch_file_set(&k.pr.branch, "AUTHORITY.md", &authority(LOCAL));
    refused_no_write(
        &fake,
        &b.dir,
        &done_args(&k, &["--local-run", DEV_GATE]),
        "local-gate-not-enabled",
    );
    // and without the flag the red CI still blocks
    let o = fake.run(&b.dir, &done_args(&k, &[]));
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-ci-not-green"), "{}", o.all());
    // no read of AUTHORITY.md names a branch
    let reads: Vec<_> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.path.contains("/contents/AUTHORITY.md"))
        .collect();
    assert!(!reads.is_empty());
    assert!(reads.iter().all(|r| !r.path.contains("ref=")), "{reads:?}");

    // the other way: the default branch says local, the PR branch says ci
    let (fake, _c, b, k) = case(&authority(LOCAL));
    fake.branch_file_set(&k.pr.branch, "AUTHORITY.md", &authority(CI));
    ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
}

#[test]
fn ci_mode_refuses_the_flag_on_done_and_verify_before_any_write() {
    // no ci_mode key = ci
    let (fake, c, b, k) = case(&authority(""));
    refused_no_write(
        &fake,
        &b.dir,
        &done_args(&k, &["--local-run", DEV_GATE]),
        "local-gate-not-enabled",
    );
    // a done with CI evidence (the CI is green now), then a verify with the flag
    make_green(&fake);
    ok(&fake, &b.dir, &done_args(&k, &[]));
    refused_no_write(
        &fake,
        &c.dir,
        &["verify", &k.ts, "--local-run", COORD_GATE],
        "local-gate-not-enabled",
    );
    // control: the same verify without the flag passes
    let o = ok(&fake, &c.dir, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn verify_with_a_gate_flag_for_a_done_with_ci_evidence_is_refused() {
    // local mode, but the done has CI evidence (green CI, delivered before the switch)
    let (fake, c, b, k) = case(&authority(CI));
    make_green(&fake);
    ok(&fake, &b.dir, &done_args(&k, &[]));
    fake.file("AUTHORITY.md", &authority(LOCAL));
    refused_no_write(
        &fake,
        &c.dir,
        &["verify", &k.ts, "--local-run", COORD_GATE],
        "local-gate-unexpected",
    );
    // control: without the flag the CI judges as before
    let o = ok(&fake, &c.dir, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn after_switching_back_to_ci_a_local_gate_verify_does_not_merge() {
    let (fake, c, b, k) = case(&authority(LOCAL));
    ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
    ok(&fake, &c.dir, &["verify", &k.ts, "--local-run", COORD_GATE]);

    // the owner switches back to ci: merge is refused, nothing is merged
    fake.file("AUTHORITY.md", &authority(CI));
    refused_no_write(&fake, &c.dir, &["merge", &k.ts], "local-gate-not-enabled");
    assert!(merge_calls(&fake).is_empty());
    // a new verify fails: the local-gate done needs CI now; the dev delivers again
    let o = ok(&fake, &c.dir, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(o.stdout.contains("local-gate-not-enabled"), "{}", o.all());
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn the_merge_works_again_when_the_mode_is_local_again() {
    // control for the test above: the same state merges while ci_mode is local
    let (fake, c, b, k) = case(&authority(LOCAL));
    ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
    ok(&fake, &c.dir, &["verify", &k.ts, "--local-run", COORD_GATE]);
    fake.file("AUTHORITY.md", &authority(CI));
    refused_no_write(&fake, &c.dir, &["merge", &k.ts], "local-gate-not-enabled");
    fake.file("AUTHORITY.md", &authority(LOCAL));
    let o = ok(&fake, &c.dir, &["merge", &k.ts]);
    assert!(o.stdout.contains("merged at head"), "{}", o.all());
}

#[test]
fn a_headless_coordinator_leaves_a_local_gate_ticket() {
    let (fake, c, b, k) = case(&authority(LOCAL));
    ok(&fake, &b.dir, &done_args(&k, &["--local-run", DEV_GATE]));
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());

    // verify --local-run is a judgment
    refused_no_write(
        &fake,
        headless.path(),
        &["verify", &k.ts, "--local-run", COORD_GATE],
        "headless-scope",
    );
    // tick leaves the ticket and writes nothing for it
    let before = writes(&fake);
    let o = ok(&fake, headless.path(), &["tick"]);
    assert!(
        o.stdout.contains("left for the interactive coordinator")
            && o.stdout.contains("local gate"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&fake), before, "tick wrote: {}", o.all());
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("Review")
    );

    // the interactive coordinator verifies; a headless merge is refused all the same
    ok(&fake, &c.dir, &["verify", &k.ts, "--local-run", COORD_GATE]);
    refused_no_write(&fake, headless.path(), &["merge", &k.ts], "headless-scope");
    assert!(merge_calls(&fake).is_empty());
    // control: the interactive coordinator merges
    ok(&fake, &c.dir, &["merge", &k.ts]);
    assert_eq!(merge_calls(&fake).len(), 1);
}
