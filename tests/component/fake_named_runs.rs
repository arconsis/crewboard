//! The named CI runs of the evidence count again in `verify` and `merge` (spec/ci#which-ci-runs-count), against the fake GitHub of `tests/support/board.rs`.
//!
//! The case: a push started no CI run, so the dev started a `workflow_dispatch` run and named it
//! with `done --run <id>`. The coordinator does not repeat `--run`: `verify` and `merge` read the
//! run from the `ci-runs:` line of the done message and judge it like `done` did. A run by hand
//! is not CI only: the headless `merge` and `tick` leave such a ticket
//! (spec/agents#headless-coordinator).

use crate::support::authority::AUTHORITY;
use crate::support::board::{Agent, FakeBoard, Out, REPO};
use crate::support::cli::{crew, item_in, run_ok};
use crewboard::protocol::names::Kind;

/// A CI-only Gate, so that the headless tick merges.
const BODY: &str = "Goal: integration test ticket.\n\nGate: ci\n";

/// The other commit, for a run that is not a run of the PR head.
const OTHER_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

struct Case {
    t: u64,
    ts: String,
    item: String,
    pr: crate::support::board::Pull,
}

/// A ticket assigned to B and taken; its PR head has a `workflow_dispatch` run `run` that ended
/// with `conclusion`, and no push or pull_request run.
fn case(fake: &FakeBoard, c: &Agent, b: &Agent, run: u64, conclusion: &str) -> Case {
    let t = fake.ticket_issue("IT named run", BODY, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run(run, &pr.head, "workflow_dispatch", conclusion);
    let _ = fake.poll_json(b);
    Case { t, ts, item, pr }
}

/// `done --run <run>`: accepted, Status Review, and the done message names the run.
fn done_with_run(fake: &FakeBoard, b: &Agent, k: &Case, run: u64) -> Out {
    let o = run_ok(
        fake,
        b,
        &[
            "done",
            &k.ts,
            "--pr",
            &k.pr.url,
            "--sha",
            &k.pr.head,
            "--run",
            &run.to_string(),
        ],
    );
    assert!(
        o.stdout.contains(&format!(
            "ci-runs: counted: {run} (workflow_dispatch, named, success)"
        )),
        "{}",
        o.all()
    );
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("Review")
    );
    o
}

/// A later signed done message of B for the same PR head, with this `ci-runs:` text (the latest
/// done counts). It stands for a done message that `done` would not write today.
fn signed_done(fake: &FakeBoard, b: &Agent, c: &Agent, k: &Case, ci_runs: &str) {
    fake.signed_now(
        &b.key,
        &b.name,
        &c.name,
        Kind::Done,
        k.t,
        &format!(
            "Done. Evidence follows.\npr: {}\nsha: {}\nci: https://github.com/{REPO}/actions/runs/1\nci-sources: workflow-runs, check-runs, statuses, named-runs\nci-runs: {ci_runs}\n",
            k.pr.url, k.pr.head
        ),
    );
}

fn run_gets(fake: &FakeBoard, from: usize, run: u64) -> usize {
    fake.requests()[from..]
        .iter()
        .filter(|r| r.method == "GET" && r.path.ends_with(&format!("/actions/runs/{run}")))
        .count()
}

#[test]
fn a_plain_verify_counts_the_green_run_that_done_named_and_merge_merges() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = case(&fake, &c, &b, 9005, "success");
    done_with_run(&fake, &b, &k, 9005);
    let from = fake.requests().len();
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    // the run was read again, by its id, once
    assert_eq!(run_gets(&fake, from, 9005), 1);
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("Review")
    );
    // merge, also without --run
    let o = run_ok(&fake, &c, &["merge", &k.ts]);
    assert!(o.stdout.contains("merged at head"), "{}", o.all());
    assert!(fake.pr_now(k.pr.number).0);
}

#[test]
fn headless_verify_counts_the_named_run_but_headless_merge_and_tick_leave_it() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = case(&fake, &c, &b, 9005, "success");
    done_with_run(&fake, &b, &k, 9005);
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    // tick: a run by hand is not CI only; the line says so, nothing is verified or merged
    let o = fake.run(headless.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("left for the interactive coordinator")
            && o.stdout.contains("names a CI run by hand"),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("verify "), "{}", o.all());
    assert!(!fake.pr_now(k.pr.number).0);
    // the headless verify judges like the interactive one: it counts the run
    let o = fake.run(headless.path(), &["verify", &k.ts]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    // the headless merge refuses before any read of the PR or the CI
    let from = fake.requests().len();
    let o = fake.run(headless.path(), &["merge", &k.ts]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("[rule=headless-scope]") && o.stderr.contains("names a CI run by hand"),
        "{}",
        o.all()
    );
    assert!(
        fake.requests()[from..]
            .iter()
            .all(|r| !r.path.contains("/pulls/") && !r.path.contains("/actions/runs")),
        "{:?}",
        &fake.requests()[from..]
    );
    assert!(!fake.pr_now(k.pr.number).0);
    // control: the interactive coordinator merges it
    let o = run_ok(&fake, &c, &["merge", &k.ts]);
    assert!(o.stdout.contains("merged at head"), "{}", o.all());
}

#[test]
fn the_headless_tick_still_merges_evidence_without_a_named_run() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let t = fake.ticket_issue("IT push run", BODY, &["crew:work"]);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run(9010, &pr.head, "push", "success");
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    let o = fake.run(headless.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    assert!(fake.pr_now(pr.number).0, "{}", o.all());
}

#[test]
fn a_named_run_that_is_red_at_verify_time_fails() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    // verify after the rerun failed
    let k = case(&fake, &c, &b, 9005, "success");
    done_with_run(&fake, &b, &k, 9005);
    fake.set_run_conclusion(9005, "failure");
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout.contains("is not green") && o.stdout.contains("named run 9005"),
        "{}",
        o.all()
    );
}

#[test]
fn a_named_run_that_went_red_after_the_verify_refuses_the_merge() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    // merge after a passed verify: the run went red since, merge judges it again
    let k2 = case(&fake, &c, &b, 9006, "success");
    done_with_run(&fake, &b, &k2, 9006);
    assert!(
        run_ok(&fake, &c, &["verify", &k2.ts])
            .stdout
            .contains("verify PASS")
    );
    fake.set_run_conclusion(9006, "failure");
    let o = fake.run(&c.dir, &["merge", &k2.ts]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-ci-not-green"), "{}", o.all());
    assert!(!fake.pr_now(k2.pr.number).0);
}

#[test]
fn a_named_run_of_another_sha_fails_verify_and_merge_refuses() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = case(&fake, &c, &b, 9005, "success");
    done_with_run(&fake, &b, &k, 9005);
    // a green dispatch run of another commit, named by a later done message
    fake.workflow_run(9007, OTHER_SHA, "workflow_dispatch", "success");
    signed_done(
        &fake,
        &b,
        &c,
        &k,
        "counted: 9007 (workflow_dispatch, named, success)",
    );
    // merge first: it refuses before the merge call (and without a verify of this done)
    let o = fake.run(&c.dir, &["merge", &k.ts]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-run-wrong-sha"), "{}", o.all());
    assert!(!fake.pr_now(k.pr.number).0);
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout.contains("named run of the evidence")
            && o.stdout.contains("evidence-run-wrong-sha")
            && o.stdout.contains(OTHER_SHA),
        "{}",
        o.all()
    );
}

#[test]
fn without_a_named_run_and_a_counted_run_verify_fails_as_before_and_run_adds_one() {
    let (fake, c, b) = crew();
    // the done message names the dispatch run only as information: it does not count
    let k = case(&fake, &c, &b, 9005, "success");
    done_with_run(&fake, &b, &k, 9005);
    signed_done(
        &fake,
        &b,
        &c,
        &k,
        "information only: 9005 (workflow_dispatch, success)",
    );
    let from = fake.requests().len();
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout.contains("no counted CI run") && o.stdout.contains("--run"),
        "{}",
        o.all()
    );
    assert_eq!(run_gets(&fake, from, 9005), 0, "no named run was read");
    // control: the same evidence with `--run` of the coordinator passes (it adds the run)
    let (fake, c, b) = crew();
    let k2 = case(&fake, &c, &b, 9006, "success");
    done_with_run(&fake, &b, &k2, 9006);
    signed_done(
        &fake,
        &b,
        &c,
        &k2,
        "information only: 9006 (workflow_dispatch, success)",
    );
    let o = run_ok(&fake, &c, &["verify", &k2.ts, "--run", "9006"]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn run_on_verify_adds_to_the_named_runs_without_a_duplicate() {
    let (fake, c, b) = crew();
    let k = case(&fake, &c, &b, 9005, "success");
    done_with_run(&fake, &b, &k, 9005);
    // a second green dispatch run of the head, named by the coordinator only
    fake.workflow_run(9008, &k.pr.head, "workflow_dispatch", "success");
    let from = fake.requests().len();
    let o = run_ok(
        &fake,
        &c,
        &["verify", &k.ts, "--run", "9005", "--run", "9008"],
    );
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    assert_eq!(run_gets(&fake, from, 9005), 1, "run 9005 read once");
    assert_eq!(run_gets(&fake, from, 9008), 1);
    // the verify result names both runs
    assert!(
        fake.comments_of(k.t)
            .iter()
            .any(|(_, _, body)| body.contains("verify: pass")
                && body.contains("9005 (workflow_dispatch, named, success)")
                && body.contains("9008 (workflow_dispatch, named, success)")),
        "{:?}",
        fake.comments_of(k.t)
    );
}
